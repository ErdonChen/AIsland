use crate::contracts::{
    AgentStatus, AppErrorCode, CommandError, PresetAgentAdapterId, SafeMessageParameters,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

const MAX_STATUS_BYTES: u64 = 256 * 1024;
const MAX_WIRE_TAIL_BYTES: u64 = 512 * 1024;
const MAX_CURSOR_TAIL_BYTES: u64 = 512 * 1024;
const MAX_CURSOR_LINE_BYTES: usize = 1024 * 1024;
const MAX_REPLY_BYTES: usize = 1_024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NativeProfileActivity {
    pub adapter_id: PresetAgentAdapterId,
    pub task_id: String,
    pub status: AgentStatus,
    pub latest_reply: Option<String>,
    pub source_event_id: String,
    pub occurred_at: i64,
}

pub(crate) trait NativeProfileActivitySource: Send + Sync {
    fn latest_activity(
        &self,
        adapter_id: PresetAgentAdapterId,
        now: i64,
    ) -> Result<Option<NativeProfileActivity>, CommandError>;
}

pub(crate) struct NativeProfileActivityReader {
    windows_home: PathBuf,
    roaming_app_data: PathBuf,
    cache: Mutex<NativeProfileActivityCache>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified_nanos: u128,
    created_nanos: u128,
}

#[derive(Clone, Debug, Default)]
struct CachedProfileActivity {
    sources: Vec<(PathBuf, FileStamp)>,
    activity: Option<NativeProfileActivity>,
}

#[derive(Debug, Default)]
struct NativeProfileActivityCache {
    kimi: CachedProfileActivity,
    qoderwork: CachedProfileActivity,
    cursor: CursorActivityCache,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CursorScanMetrics {
    bytes_read: u64,
    parser_calls: u64,
}

#[derive(Clone, Debug, Default)]
struct CursorActivityCache {
    path: Option<PathBuf>,
    stamp: Option<FileStamp>,
    offset: u64,
    pending: Vec<u8>,
    discard_incomplete_line: bool,
    status: Option<AgentStatus>,
    turn_reply: Option<String>,
    latest_reply: Option<String>,
    activity: Option<NativeProfileActivity>,
    metrics: CursorScanMetrics,
}

impl NativeProfileActivityReader {
    pub(crate) fn new(windows_home: PathBuf, roaming_app_data: PathBuf) -> Self {
        Self {
            windows_home,
            roaming_app_data,
            cache: Mutex::new(NativeProfileActivityCache::default()),
        }
    }

    #[cfg(test)]
    fn cursor_scan_metrics(&self) -> CursorScanMetrics {
        self.cache
            .lock()
            .expect("native profile cache lock poisoned")
            .cursor
            .metrics
    }

    fn read_kimi(&self) -> Result<(Option<NativeProfileActivity>, Vec<PathBuf>), CommandError> {
        let root = self.roaming_app_data.join("kimi-desktop");
        let status_path = root.join("kimi-agent/conversation-statuses.json");
        let database_path = root
            .join("daimon-share/daimon/agents/main/sessions/hosted-logical/conversations.sqlite");
        if !database_path.is_file() {
            return Ok((None, vec![status_path, database_path]));
        }
        let connection = open_read_only(&database_path)?;
        let conversation = connection
            .query_row(
                "SELECT conversation_id, kernel_records_path, updated_at_ms
                   FROM conversations
                  ORDER BY updated_at_ms DESC, conversation_id DESC
                  LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(CommandError::from)?;
        let Some((conversation_id, wire_path, updated_at)) = conversation else {
            return Ok((None, vec![status_path, database_path]));
        };
        let wire_path = PathBuf::from(wire_path);
        if !safe_identifier(&conversation_id)
            || updated_at < 0
            || !is_owned_regular_file(&root, &wire_path)
        {
            return Ok((None, vec![status_path, database_path]));
        }
        let status = read_kimi_status(&status_path, &conversation_id)?;
        let (wire_status, latest_reply) = read_kimi_wire_tail(&wire_path)?;
        let status = status.or(wire_status).unwrap_or(AgentStatus::Idle);
        let occurred_at = updated_at.max(file_modified_millis(&status_path).unwrap_or(0));
        let task_id = hashed_id("native-kimi-task", &conversation_id);
        let source_event_id = hashed_id(
            "native-kimi-event",
            &format!(
                "{conversation_id}|{status:?}|{occurred_at}|{}",
                latest_reply.as_deref().unwrap_or_default()
            ),
        );
        Ok((
            Some(NativeProfileActivity {
                adapter_id: PresetAgentAdapterId::Kimi,
                task_id,
                status,
                latest_reply,
                source_event_id,
                occurred_at,
            }),
            vec![status_path, database_path, wire_path],
        ))
    }

    fn read_qoderwork(
        &self,
    ) -> Result<(Option<NativeProfileActivity>, Vec<PathBuf>), CommandError> {
        let database_path = self.roaming_app_data.join("QwenWorkCN/data/agents.db");
        if !database_path.is_file() {
            return Ok((None, vec![database_path]));
        }
        let connection = open_read_only(&database_path)?;
        let message = connection
            .query_row(
                "SELECT message_id, chat_id, COALESCE(sub_chat_id, ''), role, updated_at
                   FROM messages
                  ORDER BY updated_at DESC, sequence DESC, message_id DESC
                  LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(CommandError::from)?;
        let Some((message_id, chat_id, sub_chat_id, role, updated_at)) = message else {
            return Ok((None, vec![database_path]));
        };
        if !safe_identifier(&message_id) || !safe_identifier(&chat_id) || updated_at < 0 {
            return Ok((None, vec![database_path]));
        }
        let role = role.to_ascii_lowercase();
        let status = match role.as_str() {
            "user" | "tool" | "system" => AgentStatus::Running,
            "assistant" => AgentStatus::Completed,
            _ => AgentStatus::Idle,
        };
        // The WHERE clause is the privacy boundary: user/tool rows never load their parts column.
        let assistant_parts = connection
            .query_row(
                "SELECT parts
                   FROM messages
                  WHERE role = 'assistant' AND chat_id = ?1
                  ORDER BY updated_at DESC, sequence DESC, message_id DESC
                  LIMIT 1",
                [&chat_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(CommandError::from)?;
        let latest_reply = assistant_parts.as_deref().and_then(qoder_assistant_reply);
        let occurred_at = normalize_timestamp_millis(updated_at);
        let task_material = if sub_chat_id.is_empty() {
            chat_id.clone()
        } else {
            format!("{chat_id}|{sub_chat_id}")
        };
        let task_id = hashed_id("native-qoderwork-task", &task_material);
        let source_event_id = hashed_id(
            "native-qoderwork-event",
            &format!(
                "{message_id}|{role}|{updated_at}|{}",
                latest_reply.as_deref().unwrap_or_default()
            ),
        );
        Ok((
            Some(NativeProfileActivity {
                adapter_id: PresetAgentAdapterId::Qoderwork,
                task_id,
                status,
                latest_reply,
                source_event_id,
                occurred_at,
            }),
            vec![database_path],
        ))
    }

    fn read_cursor(&self) -> Result<Option<NativeProfileActivity>, CommandError> {
        let root = self.windows_home.join(".cursor/projects");
        let Some((path, stamp)) = latest_cursor_transcript(&root)? else {
            self.cache
                .lock()
                .expect("native profile cache lock poisoned")
                .cursor = CursorActivityCache::default();
            return Ok(None);
        };
        let mut cache = self
            .cache
            .lock()
            .expect("native profile cache lock poisoned");
        let cursor = &mut cache.cursor;
        if cursor.path.as_ref() == Some(&path) && cursor.stamp == Some(stamp) {
            return Ok(cursor.activity.clone());
        }

        let same_file = cursor.path.as_ref() == Some(&path)
            && cursor
                .stamp
                .is_some_and(|previous| previous.created_nanos == stamp.created_nanos);
        let append_only = same_file
            && stamp.len > cursor.offset
            && cursor
                .stamp
                .is_some_and(|previous| stamp.len >= previous.len);
        let start = if append_only {
            cursor.offset
        } else {
            reset_cursor_cache(cursor, path.clone(), stamp);
            stamp.len.saturating_sub(MAX_CURSOR_TAIL_BYTES)
        };
        let bytes = read_range(&path, start, stamp.len)?;
        cursor.metrics.bytes_read = cursor.metrics.bytes_read.saturating_add(bytes.len() as u64);
        cursor.path = Some(path.clone());
        cursor.stamp = Some(stamp);
        cursor.offset = stamp.len;
        scan_cursor_bytes(cursor, bytes, !append_only && start > 0);

        let task_material = path
            .file_stem()
            .and_then(|value| value.to_str())
            .filter(|value| safe_identifier(value))
            .map(str::to_owned)
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let status = cursor.status.clone().unwrap_or(AgentStatus::Idle);
        let occurred_at = (stamp.modified_nanos / 1_000_000).min(i64::MAX as u128) as i64;
        let task_id = hashed_id("native-cursor-task", &task_material);
        let source_event_id = hashed_id(
            "native-cursor-event",
            &format!(
                "{task_material}|{}|{status:?}|{}",
                cursor.offset,
                cursor.latest_reply.as_deref().unwrap_or_default()
            ),
        );
        let activity = NativeProfileActivity {
            adapter_id: PresetAgentAdapterId::Cursor,
            task_id,
            status,
            latest_reply: cursor.latest_reply.clone(),
            source_event_id,
            occurred_at,
        };
        cursor.activity = Some(activity.clone());
        Ok(Some(activity))
    }

    fn cached_or_refresh(
        &self,
        adapter_id: PresetAgentAdapterId,
    ) -> Result<Option<NativeProfileActivity>, CommandError> {
        if adapter_id == PresetAgentAdapterId::Cursor {
            return self.read_cursor();
        }
        let cached = {
            let cache = self
                .cache
                .lock()
                .expect("native profile cache lock poisoned");
            cache_entry(&cache, &adapter_id).cloned()
        };
        if cached
            .as_ref()
            .is_some_and(|entry| !entry.sources.is_empty() && sources_unchanged(&entry.sources))
        {
            return Ok(cached.and_then(|entry| entry.activity));
        }
        let (activity, mut source_paths) = match adapter_id {
            PresetAgentAdapterId::Kimi => self.read_kimi()?,
            PresetAgentAdapterId::Qoderwork => self.read_qoderwork()?,
            PresetAgentAdapterId::Trae => return Ok(None),
            PresetAgentAdapterId::Cursor => unreachable!("Cursor returns before generic cache"),
        };
        source_paths.extend(
            source_paths
                .iter()
                .map(|path| PathBuf::from(format!("{}-wal", path.display())))
                .collect::<Vec<_>>(),
        );
        let sources = source_paths
            .into_iter()
            .filter_map(|path| file_stamp(&path).map(|stamp| (path, stamp)))
            .collect();
        let mut cache = self
            .cache
            .lock()
            .expect("native profile cache lock poisoned");
        *cache_entry_mut(&mut cache, &adapter_id) = CachedProfileActivity {
            sources,
            activity: activity.clone(),
        };
        Ok(activity)
    }
}

fn latest_cursor_transcript(root: &Path) -> Result<Option<(PathBuf, FileStamp)>, CommandError> {
    if !root.is_dir() {
        return Ok(None);
    }
    let mut latest: Option<(PathBuf, FileStamp)> = None;
    let projects = std::fs::read_dir(root).map_err(|_| io_failure())?;
    for project in projects.flatten() {
        if !project.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let transcripts = project.path().join("agent-transcripts");
        let Ok(sessions) = std::fs::read_dir(transcripts) else {
            continue;
        };
        for session in sessions.flatten() {
            if !session.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let Ok(files) = std::fs::read_dir(session.path()) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().and_then(|value| value.to_str()) != Some("jsonl")
                    || !file.file_type().is_ok_and(|kind| kind.is_file())
                {
                    continue;
                }
                let Some(stamp) = file_stamp(&path) else {
                    continue;
                };
                let replace = latest.as_ref().is_none_or(|(current_path, current_stamp)| {
                    (stamp.modified_nanos, &path) > (current_stamp.modified_nanos, current_path)
                });
                if replace {
                    latest = Some((path, stamp));
                }
            }
        }
    }
    Ok(latest)
}

fn reset_cursor_cache(cursor: &mut CursorActivityCache, path: PathBuf, stamp: FileStamp) {
    let metrics = cursor.metrics;
    *cursor = CursorActivityCache {
        path: Some(path),
        stamp: Some(stamp),
        metrics,
        ..CursorActivityCache::default()
    };
}

fn read_range(path: &Path, start: u64, end: u64) -> Result<Vec<u8>, CommandError> {
    if end < start || end - start > usize::MAX as u64 {
        return Err(io_failure());
    }
    let mut file = File::open(path).map_err(|_| io_failure())?;
    file.seek(SeekFrom::Start(start))
        .map_err(|_| io_failure())?;
    let mut bytes = vec![0; (end - start) as usize];
    file.read_exact(&mut bytes).map_err(|_| io_failure())?;
    Ok(bytes)
}

fn scan_cursor_bytes(cursor: &mut CursorActivityCache, mut bytes: Vec<u8>, starts_mid_file: bool) {
    if starts_mid_file {
        cursor.pending.clear();
        cursor.discard_incomplete_line = false;
        let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') else {
            return;
        };
        bytes.drain(..=newline);
    } else if cursor.discard_incomplete_line {
        let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') else {
            return;
        };
        bytes.drain(..=newline);
        cursor.discard_incomplete_line = false;
    } else if !cursor.pending.is_empty() {
        let mut combined = std::mem::take(&mut cursor.pending);
        combined.extend(bytes);
        bytes = combined;
    }

    let complete_end = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let trailing = bytes.split_off(complete_end);
    if trailing.len() <= MAX_CURSOR_LINE_BYTES {
        cursor.pending = trailing;
    } else {
        cursor.pending.clear();
        cursor.discard_incomplete_line = true;
    }
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.is_empty() || line.len() > MAX_CURSOR_LINE_BYTES {
            continue;
        }
        if top_level_json_string_field_equals(line, b"role", b"user") {
            cursor.status = Some(AgentStatus::Running);
            cursor.turn_reply = None;
            continue;
        }
        if top_level_json_string_field_equals(line, b"role", b"assistant") {
            cursor.metrics.parser_calls = cursor.metrics.parser_calls.saturating_add(1);
            let Ok(value) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            cursor.status = Some(AgentStatus::Running);
            if let Some(text) = cursor_assistant_text(&value) {
                let combined = match cursor.turn_reply.take() {
                    Some(previous) => bounded_text(&format!("{previous}{text}"), MAX_REPLY_BYTES),
                    None => Some(text),
                };
                cursor.turn_reply = combined.clone();
                cursor.latest_reply = combined;
            }
            continue;
        }
        if top_level_json_string_field_equals(line, b"type", b"turn_ended") {
            cursor.metrics.parser_calls = cursor.metrics.parser_calls.saturating_add(1);
            let Ok(value) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            cursor.status = value
                .get("status")
                .and_then(Value::as_str)
                .map(normalize_status)
                .or(Some(AgentStatus::Completed));
        }
    }
}

fn cursor_assistant_text(value: &Value) -> Option<String> {
    if value.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let content = value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)?;
    let text = content
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<String>();
    bounded_text(&text, MAX_REPLY_BYTES)
}

fn top_level_json_string_field_equals(line: &[u8], key: &[u8], expected: &[u8]) -> bool {
    let mut index = 0;
    let mut depth = 0_u32;
    while index < line.len() {
        match line[index] {
            b'{' | b'[' => {
                depth = depth.saturating_add(1);
                index += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            b'"' => {
                let string_start = index + 1;
                index = string_start;
                let mut escaped = false;
                while index < line.len() {
                    let byte = line[index];
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        break;
                    }
                    index += 1;
                }
                if index >= line.len() {
                    return false;
                }
                let string_end = index;
                index += 1;
                if depth != 1 || line.get(string_start..string_end) != Some(key) {
                    continue;
                }
                while line.get(index).is_some_and(u8::is_ascii_whitespace) {
                    index += 1;
                }
                if line.get(index) != Some(&b':') {
                    continue;
                }
                index += 1;
                while line.get(index).is_some_and(u8::is_ascii_whitespace) {
                    index += 1;
                }
                if line.get(index) != Some(&b'"') {
                    continue;
                }
                let value_start = index + 1;
                let value_end = value_start.saturating_add(expected.len());
                return line.get(value_start..value_end) == Some(expected)
                    && line.get(value_end) == Some(&b'"');
            }
            _ => index += 1,
        }
    }
    false
}

impl NativeProfileActivitySource for NativeProfileActivityReader {
    fn latest_activity(
        &self,
        adapter_id: PresetAgentAdapterId,
        _now: i64,
    ) -> Result<Option<NativeProfileActivity>, CommandError> {
        self.cached_or_refresh(adapter_id)
    }
}

fn cache_entry<'a>(
    cache: &'a NativeProfileActivityCache,
    adapter_id: &PresetAgentAdapterId,
) -> Option<&'a CachedProfileActivity> {
    match adapter_id {
        PresetAgentAdapterId::Kimi => Some(&cache.kimi),
        PresetAgentAdapterId::Qoderwork => Some(&cache.qoderwork),
        PresetAgentAdapterId::Trae | PresetAgentAdapterId::Cursor => None,
    }
}

fn cache_entry_mut<'a>(
    cache: &'a mut NativeProfileActivityCache,
    adapter_id: &PresetAgentAdapterId,
) -> &'a mut CachedProfileActivity {
    match adapter_id {
        PresetAgentAdapterId::Kimi => &mut cache.kimi,
        PresetAgentAdapterId::Qoderwork => &mut cache.qoderwork,
        PresetAgentAdapterId::Trae | PresetAgentAdapterId::Cursor => {
            unreachable!("unsupported adapters are returned before cache mutation")
        }
    }
}

fn open_read_only(path: &Path) -> Result<Connection, CommandError> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(CommandError::from)
}

fn read_kimi_status(
    path: &Path,
    conversation_id: &str,
) -> Result<Option<AgentStatus>, CommandError> {
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = read_bounded(path, MAX_STATUS_BYTES)?;
    let root: Value = serde_json::from_slice(&bytes).map_err(|_| io_failure())?;
    let value = root.get(conversation_id);
    let status = value.and_then(Value::as_str).or_else(|| {
        value
            .and_then(|value| value.get("status"))
            .and_then(Value::as_str)
    });
    Ok(status.map(normalize_status))
}

fn read_kimi_wire_tail(path: &Path) -> Result<(Option<AgentStatus>, Option<String>), CommandError> {
    let mut file = File::open(path).map_err(|_| io_failure())?;
    let len = file.metadata().map_err(|_| io_failure())?.len();
    let start = len.saturating_sub(MAX_WIRE_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))
        .map_err(|_| io_failure())?;
    let mut bytes = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut bytes).map_err(|_| io_failure())?;
    if start > 0 {
        let Some(first_newline) = bytes.iter().position(|byte| *byte == b'\n') else {
            return Ok((None, None));
        };
        bytes.drain(..=first_newline);
    }
    let mut status = None;
    let mut latest_reply = None;
    for line in bytes.split(|byte| *byte == b'\n') {
        let content_part = line
            .windows(b"content.part".len())
            .any(|part| part == b"content.part");
        let assistant_text = content_part && json_string_field_equals(line, b"type", b"text");
        let lifecycle = line
            .windows(b"step.begin".len())
            .any(|part| part == b"step.begin")
            || line
                .windows(b"step.end".len())
                .any(|part| part == b"step.end");
        if line.is_empty() || (!assistant_text && !lifecycle) {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("context.append_loop_event") {
            continue;
        }
        let Some(event) = value.get("event") else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("step.begin") => status = Some(AgentStatus::Running),
            Some("step.end") => status = Some(AgentStatus::Completed),
            Some("content.part") => {
                let part = event.get("part");
                if part
                    .and_then(|part| part.get("type"))
                    .and_then(Value::as_str)
                    == Some("text")
                {
                    latest_reply = part
                        .and_then(|part| part.get("text"))
                        .and_then(Value::as_str)
                        .and_then(|text| bounded_text(text, MAX_REPLY_BYTES));
                }
            }
            _ => {}
        }
    }
    Ok((status, latest_reply))
}

fn json_string_field_equals(line: &[u8], key: &[u8], expected: &[u8]) -> bool {
    let mut start = 0;
    while let Some(relative_quote) = line[start..].iter().position(|byte| *byte == b'"') {
        let key_start = start + relative_quote + 1;
        let key_end = key_start.saturating_add(key.len());
        if line.get(key_start..key_end) == Some(key) && line.get(key_end) == Some(&b'"') {
            let mut cursor = key_end + 1;
            while line.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
            if line.get(cursor) != Some(&b':') {
                start = key_start;
                continue;
            }
            cursor += 1;
            while line.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
            if line.get(cursor) == Some(&b'"') {
                let value_start = cursor + 1;
                let value_end = value_start.saturating_add(expected.len());
                if line.get(value_start..value_end) == Some(expected)
                    && line.get(value_end) == Some(&b'"')
                {
                    return true;
                }
            }
        }
        start = key_start;
    }
    false
}

fn qoder_assistant_reply(parts: &str) -> Option<String> {
    let parts: Value = serde_json::from_str(parts).ok()?;
    let items = parts.as_array()?;
    items
        .iter()
        .rev()
        .find(|part| part.get("type").and_then(Value::as_str) == Some("text"))
        .and_then(|part| part.get("text"))
        .and_then(Value::as_str)
        .and_then(|text| bounded_text(text, MAX_REPLY_BYTES))
}

fn normalize_status(status: &str) -> AgentStatus {
    match status.to_ascii_lowercase().as_str() {
        "running" | "in_progress" | "inprogress" | "pending" | "streaming" => AgentStatus::Running,
        "completed" | "complete" | "success" | "succeeded" => AgentStatus::Completed,
        "failed" | "error" | "cancelled" | "canceled" => AgentStatus::Failed,
        "waiting" => AgentStatus::Waiting,
        "timeout" | "timed_out" => AgentStatus::Timeout,
        _ => AgentStatus::Idle,
    }
}

fn bounded_text(text: &str, max_bytes: usize) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text.len() <= max_bytes {
        return Some(text.to_owned());
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(text[..end].to_owned())
}

fn read_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>, CommandError> {
    let mut file = File::open(path).map_err(|_| io_failure())?;
    if file.metadata().map_err(|_| io_failure())?.len() > max_bytes {
        return Err(io_failure());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|_| io_failure())?;
    Ok(bytes)
}

fn is_owned_regular_file(root: &Path, path: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    path.starts_with(root) && path.metadata().is_ok_and(|metadata| metadata.is_file())
}

fn safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn hashed_id(prefix: &str, material: &str) -> String {
    let digest = format!("{:x}", Sha256::digest(material.as_bytes()));
    format!("{prefix}-{}", &digest[..24])
}

fn normalize_timestamp_millis(value: i64) -> i64 {
    if value < 10_000_000_000 {
        value.saturating_mul(1_000)
    } else {
        value
    }
}

fn file_modified_millis(path: &Path) -> Option<i64> {
    path.metadata()
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
}

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let metadata = path.metadata().ok()?;
    let modified_nanos = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let created_nanos = metadata
        .created()
        .ok()
        .and_then(|created| created.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos());
    Some(FileStamp {
        len: metadata.len(),
        modified_nanos,
        created_nanos,
    })
}

fn sources_unchanged(sources: &[(PathBuf, FileStamp)]) -> bool {
    sources
        .iter()
        .all(|(path, stamp)| file_stamp(path).as_ref() == Some(stamp))
}

fn io_failure() -> CommandError {
    CommandError {
        code: AppErrorCode::IoFailure,
        message_key: "errors.ioFailure".into(),
        details: SafeMessageParameters::new(),
        retryable: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};
    use std::fs;
    use std::fs::OpenOptions;
    use std::io::Write;

    fn kimi_fixture(root: &std::path::Path, status: &str) -> PathBuf {
        let kimi = root.join("roaming/kimi-desktop");
        let database = kimi
            .join("daimon-share/daimon/agents/main/sessions/hosted-logical/conversations.sqlite");
        let wire = kimi.join(
            "daimon-share/daimon/runtime/kimi-code/home/sessions/workspace/conversation/agents/main/wire.jsonl",
        );
        fs::create_dir_all(database.parent().unwrap()).unwrap();
        fs::create_dir_all(wire.parent().unwrap()).unwrap();
        fs::create_dir_all(kimi.join("kimi-agent")).unwrap();
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE conversations(
                    conversation_id TEXT NOT NULL,
                    kernel_records_path TEXT NOT NULL,
                    updated_at_ms INTEGER NOT NULL
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO conversations VALUES (?1, ?2, ?3)",
                params!["conversation-safe-id", wire.to_string_lossy(), 10_000_i64],
            )
            .unwrap();
        fs::write(
            kimi.join("kimi-agent/conversation-statuses.json"),
            serde_json::json!({"conversation-safe-id": status}).to_string(),
        )
        .unwrap();
        fs::write(
            &wire,
            [
                serde_json::json!({"type":"context.append_message","message":{"role":"user","content":"private user prompt"}}).to_string(),
                serde_json::json!({"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"think","think":"private reasoning"}}}).to_string(),
                serde_json::json!({"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"text","text":"Kimi 安全回复"}}}).to_string(),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        wire
    }

    fn qwen_fixture(root: &std::path::Path) -> Connection {
        let database = root.join("roaming/QwenWorkCN/data/agents.db");
        fs::create_dir_all(database.parent().unwrap()).unwrap();
        let connection = Connection::open(database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE messages(
                    id TEXT NOT NULL,
                    message_id TEXT NOT NULL,
                    chat_id TEXT NOT NULL,
                    sub_chat_id TEXT,
                    sequence INTEGER NOT NULL,
                    role TEXT NOT NULL,
                    parts TEXT NOT NULL,
                    search_status TEXT,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );",
            )
            .unwrap();
        connection
    }

    fn cursor_transcript_fixture(root: &std::path::Path) -> PathBuf {
        let transcript = root.join(
            "home/.cursor/projects/workspace/agent-transcripts/session-safe-id/session-safe-id.jsonl",
        );
        fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        fs::write(
            &transcript,
            [
                serde_json::json!({
                    "role": "user",
                    "message": {"content": [{"type": "text", "text": "private user prompt"}]}
                })
                .to_string(),
                serde_json::json!({
                    "role": "assistant",
                    "message": {"content": [
                        {"type": "tool_use", "name": "Read", "input": {"path": "private tool input"}},
                        {"type": "text", "text": "Cursor 安全回复"}
                    ]}
                })
                .to_string(),
                serde_json::json!({"type": "turn_ended", "status": "success"}).to_string(),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        transcript
    }

    #[test]
    fn kimi_family_reads_only_text_parts_and_projects_lifecycle_status() {
        let directory = tempfile::tempdir().unwrap();
        kimi_fixture(directory.path(), "running");
        let reader = NativeProfileActivityReader::new(
            directory.path().join("home"),
            directory.path().join("roaming"),
        );

        let running = reader
            .latest_activity(PresetAgentAdapterId::Kimi, 10_100)
            .unwrap()
            .expect("Kimi desktop activity should be detected");
        assert_eq!(running.status, AgentStatus::Running);
        assert_eq!(running.latest_reply.as_deref(), Some("Kimi 安全回复"));
        assert!(!running.latest_reply.unwrap().contains("private"));
    }

    #[test]
    fn qwenworkcn_user_then_assistant_projects_running_then_completed_without_tool_output() {
        let directory = tempfile::tempdir().unwrap();
        let connection = qwen_fixture(directory.path());
        connection
            .execute(
                "INSERT INTO messages VALUES (?1, ?2, ?3, NULL, ?4, 'user', ?5, 'ready', ?6, ?6)",
                params![
                    "row-user",
                    "message-user",
                    "chat-safe-id",
                    1_i64,
                    "private user prompt",
                    20_i64
                ],
            )
            .unwrap();
        let reader = NativeProfileActivityReader::new(
            directory.path().join("home"),
            directory.path().join("roaming"),
        );

        let running = reader
            .latest_activity(PresetAgentAdapterId::Qoderwork, 20_100)
            .unwrap()
            .expect("QwenWorkCN activity should be detected");
        assert_eq!(running.status, AgentStatus::Running);
        assert_eq!(running.latest_reply, None);

        connection
            .execute(
                "INSERT INTO messages VALUES (?1, ?2, ?3, NULL, ?4, 'assistant', ?5, 'ready', ?6, ?6)",
                params![
                    "row-assistant",
                    "message-assistant",
                    "chat-safe-id",
                    2_i64,
                    serde_json::json!([
                        {"type":"tool-Bash","output":"private tool output"},
                        {"type":"text","text":"千问办公安全回复"}
                    ]).to_string(),
                    21_i64
                ],
            )
            .unwrap();

        let completed = reader
            .latest_activity(PresetAgentAdapterId::Qoderwork, 21_100)
            .unwrap()
            .unwrap();
        assert_eq!(completed.status, AgentStatus::Completed);
        assert_eq!(completed.latest_reply.as_deref(), Some("千问办公安全回复"));
        assert!(!completed.latest_reply.unwrap().contains("private"));
    }

    #[test]
    fn cursor_transcript_projects_completion_and_only_assistant_text() {
        let directory = tempfile::tempdir().unwrap();
        cursor_transcript_fixture(directory.path());
        let reader = NativeProfileActivityReader::new(
            directory.path().join("home"),
            directory.path().join("roaming"),
        );

        let activity = reader
            .latest_activity(PresetAgentAdapterId::Cursor, 30_100)
            .unwrap()
            .expect("Cursor transcript activity should be detected");

        assert_eq!(activity.status, AgentStatus::Completed);
        assert_eq!(activity.latest_reply.as_deref(), Some("Cursor 安全回复"));
        assert!(!activity.latest_reply.unwrap().contains("private"));
    }

    #[test]
    fn cursor_large_initial_scan_is_bounded_and_unchanged_scan_reads_and_parses_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let transcript = cursor_transcript_fixture(directory.path());
        let original = fs::read(&transcript).unwrap();
        let mut large = Vec::with_capacity((MAX_CURSOR_TAIL_BYTES as usize) + original.len() + 64);
        while large.len() <= MAX_CURSOR_TAIL_BYTES as usize + 32 {
            large.extend_from_slice(b"{}\n");
        }
        large.extend(original);
        fs::write(&transcript, large).unwrap();
        let reader = NativeProfileActivityReader::new(
            directory.path().join("home"),
            directory.path().join("roaming"),
        );

        let first = reader
            .latest_activity(PresetAgentAdapterId::Cursor, 31_100)
            .unwrap()
            .unwrap();
        let after_first = reader.cursor_scan_metrics();
        let second = reader
            .latest_activity(PresetAgentAdapterId::Cursor, 31_200)
            .unwrap()
            .unwrap();
        let after_second = reader.cursor_scan_metrics();

        assert_eq!(first.status, AgentStatus::Completed);
        assert_eq!(first.latest_reply.as_deref(), Some("Cursor 安全回复"));
        assert!(after_first.bytes_read <= MAX_CURSOR_TAIL_BYTES);
        assert_eq!(after_first.parser_calls, 2);
        assert_eq!(after_second.bytes_read - after_first.bytes_read, 0);
        assert_eq!(after_second.parser_calls - after_first.parser_calls, 0);
        assert_eq!(second.source_event_id, first.source_event_id);
    }

    #[test]
    fn cursor_growth_reads_only_new_bytes_and_defers_an_incomplete_line() {
        let directory = tempfile::tempdir().unwrap();
        let transcript = cursor_transcript_fixture(directory.path());
        let reader = NativeProfileActivityReader::new(
            directory.path().join("home"),
            directory.path().join("roaming"),
        );
        reader
            .latest_activity(PresetAgentAdapterId::Cursor, 32_100)
            .unwrap();
        let before = reader.cursor_scan_metrics();
        let user_line = serde_json::json!({
            "role": "user",
            "message": {"content": [{"type": "text", "text": "private next prompt"}]}
        })
        .to_string()
            + "\n";
        OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap()
            .write_all(user_line.as_bytes())
            .unwrap();

        let running = reader
            .latest_activity(PresetAgentAdapterId::Cursor, 32_200)
            .unwrap()
            .unwrap();
        let after_user = reader.cursor_scan_metrics();
        assert_eq!(running.status, AgentStatus::Running);
        assert_eq!(
            after_user.bytes_read - before.bytes_read,
            user_line.len() as u64
        );
        assert_eq!(after_user.parser_calls - before.parser_calls, 0);

        let assistant_line = serde_json::json!({
            "role": "assistant",
            "message": {"content": [{"type": "text", "text": "增量回复"}]}
        })
        .to_string();
        let split = assistant_line.len() / 2;
        OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap()
            .write_all(&assistant_line.as_bytes()[..split])
            .unwrap();
        let half = reader
            .latest_activity(PresetAgentAdapterId::Cursor, 32_300)
            .unwrap()
            .unwrap();
        let after_half = reader.cursor_scan_metrics();
        assert_eq!(half.status, AgentStatus::Running);
        assert_eq!(half.latest_reply.as_deref(), Some("Cursor 安全回复"));
        assert_eq!(after_half.bytes_read - after_user.bytes_read, split as u64);
        assert_eq!(after_half.parser_calls - after_user.parser_calls, 0);

        let turn_end = serde_json::json!({"type": "turn_ended", "status": "success"}).to_string();
        let mut suffix = assistant_line.as_bytes()[split..].to_vec();
        suffix.push(b'\n');
        suffix.extend_from_slice(turn_end.as_bytes());
        suffix.push(b'\n');
        OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap()
            .write_all(&suffix)
            .unwrap();
        let completed = reader
            .latest_activity(PresetAgentAdapterId::Cursor, 32_400)
            .unwrap()
            .unwrap();
        let after_complete = reader.cursor_scan_metrics();
        assert_eq!(completed.status, AgentStatus::Completed);
        assert_eq!(completed.latest_reply.as_deref(), Some("增量回复"));
        assert_eq!(
            after_complete.bytes_read - after_half.bytes_read,
            suffix.len() as u64
        );
        assert_eq!(after_complete.parser_calls - after_half.parser_calls, 2);
    }

    #[test]
    fn cursor_truncation_and_rotation_rebuild_state_without_reusing_old_reply() {
        let directory = tempfile::tempdir().unwrap();
        let transcript = cursor_transcript_fixture(directory.path());
        let reader = NativeProfileActivityReader::new(
            directory.path().join("home"),
            directory.path().join("roaming"),
        );
        let original = reader
            .latest_activity(PresetAgentAdapterId::Cursor, 33_100)
            .unwrap()
            .unwrap();
        let truncated = serde_json::json!({
            "role": "user",
            "message": {"content": [{"type": "text", "text": "private replacement prompt"}]}
        })
        .to_string()
            + "\n";
        fs::write(&transcript, truncated).unwrap();

        let rebuilt = reader
            .latest_activity(PresetAgentAdapterId::Cursor, 33_200)
            .unwrap()
            .unwrap();
        assert_eq!(rebuilt.status, AgentStatus::Running);
        assert_eq!(rebuilt.latest_reply, None);
        assert_ne!(rebuilt.source_event_id, original.source_event_id);

        let rotated = directory
            .path()
            .join("home/.cursor/projects/workspace/agent-transcripts/session-z/session-z.jsonl");
        fs::create_dir_all(rotated.parent().unwrap()).unwrap();
        fs::write(
            &rotated,
            [
                serde_json::json!({
                    "role": "assistant",
                    "message": {"content": [{"type": "text", "text": "轮转回复"}]}
                })
                .to_string(),
                serde_json::json!({"type": "turn_ended", "status": "success"}).to_string(),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();

        let switched = reader
            .latest_activity(PresetAgentAdapterId::Cursor, 33_300)
            .unwrap()
            .unwrap();
        assert_eq!(switched.status, AgentStatus::Completed);
        assert_eq!(switched.latest_reply.as_deref(), Some("轮转回复"));
        assert_ne!(switched.task_id, rebuilt.task_id);
    }
}
