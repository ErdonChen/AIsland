#!/usr/bin/env bash
set -eu
umask 077

if [ "$#" -ne 8 ] || [ "$1" != "--agent" ] || [ "$3" != "--environment" ] || [ "$5" != "--native-event" ] || [ "$7" != "--output-path" ]; then
  echo 'invalidArguments' >&2; exit 1
fi
agent=$2; environment=$4; native_event=$6; output_path=$8
case "$agent:$environment" in codex:wsl|hermes:wsl|claude:wsl) ;; *) echo 'invalidIdentity' >&2; exit 1;; esac
for tool in jq mktemp sync mv sha256sum; do command -v "$tool" >/dev/null 2>&1 || { echo 'integrationUnsupported' >&2; exit 1; }; done

LC_ALL=C IFS= read -r -N 1048576 payload || true
if LC_ALL=C IFS= read -r -n 1 extra; then echo 'payloadTooLarge' >&2; exit 1; fi
if ! normalized=$(printf '%s' "$payload" | jq -e -c -s 'if length == 1 and (.[0] | type == "object") then .[0] else error("invalid") end' 2>/dev/null); then echo 'invalidPayload' >&2; exit 1; fi
task_id=$(printf '%s' "$normalized" | jq -er 'if (.task_id | type) == "string" then .task_id elif (.extra.task_id | type) == "string" then .extra.task_id elif (.session_id | type) == "string" then "session:" + .session_id else empty end | select(utf8bytelength > 0 and utf8bytelength <= 256)' 2>/dev/null) || { echo 'invalidIdentifier' >&2; exit 1; }
sequence=$(printf '%s' "$normalized" | jq -er 'if (.sequence | type) == "number" and (.sequence|floor) == .sequence and .sequence >= 0 then tostring else "" end' 2>/dev/null) || { echo 'invalidIdentifier' >&2; exit 1; }
source_occurred_at=$(printf '%s' "$normalized" | jq -er 'if (.occurred_at | type) == "number" and (.occurred_at|floor) == .occurred_at then .occurred_at else empty end' 2>/dev/null || true)
if [ -z "$source_occurred_at" ]; then occurred_at=$(date -u +%s%3N); source_occurred_at=missing-occurred-at; else occurred_at=$source_occurred_at; fi
native_id=$(printf '%s' "$normalized" | jq -er --arg native_event "$native_event" '(.event_id // .native_event_id // (if (.extra.turn_id | type) == "string" and (.extra.turn_id | utf8bytelength) > 0 and (.extra.turn_id | utf8bytelength) <= 96 then ($native_event + "\n" + .extra.turn_id) else empty end)) | strings | select(utf8bytelength > 0 and utf8bytelength <= 128)' 2>/dev/null || true)
if [ -n "$native_id" ]; then material=$native_id; else material="$agent
$environment
$task_id
$native_event
$sequence
$source_occurred_at"; fi
digest=$(printf '%s' "$material" | sha256sum); digest=${digest%% *}
event_id="aiceland-$agent-$environment-$digest"
case "$agent:$native_event" in
  *:PermissionRequest|*:pre_approval_request) status=waiting;;
  *:SessionStart|*:UserPromptSubmit|*:on_session_start|*:pre_llm_call) status=running;;
  *:StopFailure) status=$(printf '%s' "$normalized" | jq -r 'if .failure_reason == "timeout" or .timeout == true then "timeout" else "failed" end');;
  *:Stop|*:post_llm_call) status=completed;;
  *:SessionEnd|*:on_session_end) status=$(printf '%s' "$normalized" | jq -r 'if .failure_reason == "timeout" or .timeout == true then "timeout" elif .success == false or .failed == true then "failed" else "idle" end');;
  *:post_approval_response) status=$(printf '%s' "$normalized" | jq -r 'if .extra.choice == "timeout" or .timeout == true then "timeout" else "running" end');;
  *) status=running;;
esac
wire=$(printf '%s' "$normalized" | jq -c --arg agent "$agent" --arg environment "$environment" --arg native_event "$native_event" --arg event_id "$event_id" --arg task_id "$task_id" --arg status "$status" --argjson occurred_at "$occurred_at" '
  def clip: if type == "string" then reduce explode[] as $cp ({text:"", bytes:0}; ([ $cp ] | implode) as $char | ($char | utf8bytelength) as $bytes | if .bytes + $bytes <= 1024 then .text += $char | .bytes += $bytes else . end) | .text else empty end;
  def display_value: if type == "string" then gsub("^\\s+|\\s+$"; "") | clip | if length > 0 then . else null end else null end;
  def display($name): .[$name] | display_value;
  def assistant_reply: if $native_event == "post_llm_call" then ((.extra.assistant_response | display_value) // (.last_assistant_message | display_value)) else (.last_assistant_message | display_value) end;
  {schema_version:1,event_id:$event_id,agent:$agent,environment:$environment,task_id:$task_id,status:$status,occurred_at:$occurred_at,sequence:(.sequence | if type == "number" and floor == . and . >= 0 then . else null end),task_title:(display("task_title")),project:(display("project")),message:(if ($native_event == "Stop" or $native_event == "SubagentStop" or $native_event == "post_llm_call") then (assistant_reply as $reply | if $reply == null then null else ("aiceland-agent-reply-v1:" + $reply) end) else null end),path:(display("path"))} | with_entries(select(.value != null)) | select(type == "object" and length > 0)' ) || { echo 'invalidPayload' >&2; exit 1; }
target_dir=$(dirname -- "$output_path"); target_name=$(basename -- "$output_path")
mkdir -p -- "$target_dir"
temporary=$(mktemp "$target_dir/.${target_name}.XXXXXX")
cleanup() { rm -f -- "$temporary"; }
trap cleanup EXIT HUP INT TERM
printf '%s' "$wire" > "$temporary"
sync -f "$temporary"
mv -f -- "$temporary" "$output_path"
trap - EXIT HUP INT TERM
