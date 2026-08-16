import { useEffect, useMemo, useRef, useState } from "react";
import { chooseLocalAudioFile } from "../api/dialog";
import { deleteReminderRule, saveReminderRule } from "../api/commands";
import { parseCommandError } from "../api/commandError";
import type { AgentId, AgentTriggerStatus, CommandError, ReminderRule, ReminderSound } from "../api/contracts";
import { translateRegisteredMessage } from "../i18n/catalog";
import { useI18n } from "../i18n/I18nProvider";

export interface ReminderRuleDraft {
  agentIds: AgentId[];
  triggerStatuses: AgentTriggerStatus[];
  delaySeconds: number;
  sound: ReminderSound;
  toastEnabled: boolean;
  windowEnabled: boolean;
  enabled: boolean;
}

type ReminderSettingsProps = {
  rule: ReminderRule | null;
  onSaved(rule: ReminderRule): void;
  onDeleted(): void;
  onReload?: () => Promise<void>;
};

const AGENTS: readonly AgentId[] = ["codex", "hermes", "workbuddy", "claude"];
const TRIGGERS: readonly AgentTriggerStatus[] = ["completed", "failed", "waiting", "timeout"];

function newDraft(): ReminderRuleDraft {
  return {
    agentIds: ["codex"],
    triggerStatuses: ["completed"],
    delaySeconds: 0,
    sound: { kind: "builtin", soundId: "systemNotification" },
    toastEnabled: true,
    windowEnabled: true,
    enabled: true,
  };
}

function toDraft(rule: ReminderRule | null): ReminderRuleDraft {
  if (!rule) return newDraft();
  const { agentIds, triggerStatuses, delaySeconds, sound, toastEnabled, windowEnabled, enabled } = rule;
  return { agentIds, triggerStatuses, delaySeconds, sound, toastEnabled, windowEnabled, enabled };
}

function toggle<T>(values: readonly T[], value: T): T[] {
  return values.includes(value) ? values.filter((candidate) => candidate !== value) : [...values, value];
}

function canonical<T extends string>(values: readonly T[]): T[] {
  return [...new Set(values)].sort();
}

function localFileName(sound: ReminderSound): string | null {
  if (sound.kind !== "localFile") return null;
  return sound.canonicalPath.split(/[\\/]/).pop() || sound.canonicalPath;
}

export default function ReminderSettings({ rule, onSaved, onDeleted, onReload = async () => undefined }: ReminderSettingsProps) {
  const { language, t } = useI18n();
  const [draft, setDraft] = useState<ReminderRuleDraft>(() => toDraft(rule));
  const [pending, setPending] = useState<"save" | "delete" | null>(null);
  const [error, setError] = useState<CommandError | null>(null);
  const lifecycleGenerationRef = useRef(0);

  useEffect(() => setDraft(toDraft(rule)), [rule]);
  useEffect(() => () => { lifecycleGenerationRef.current += 1; }, []);

  const validation = useMemo(() => {
    if (draft.agentIds.length === 0) return t("reminders.validation.agents");
    if (draft.triggerStatuses.length === 0) return t("reminders.validation.triggers");
    if (!Number.isInteger(draft.delaySeconds) || draft.delaySeconds < 0 || draft.delaySeconds > 604800) return t("reminders.validation.delay");
    if (draft.sound.kind === "none" && !draft.toastEnabled && !draft.windowEnabled) return t("reminders.validation.channels");
    return null;
  }, [draft, t]);
  const errorMessage = (() => {
    if (!error) return null;
    try {
      return translateRegisteredMessage(language, error.messageKey, error.details);
    } catch {
      return t("reminders.error");
    }
  })();

  const chooseSound = async () => {
    if (pending) return;
    const canonicalPath = await chooseLocalAudioFile();
    if (canonicalPath) setDraft((current) => ({ ...current, sound: { kind: "localFile", canonicalPath } }));
  };

  const save = async () => {
    if (pending || validation) return;
    const generation = lifecycleGenerationRef.current;
    setPending("save");
    setError(null);
    try {
      const saved = await saveReminderRule({
        id: rule?.id ?? null,
        expectedRevision: rule?.revision ?? null,
        ...draft,
        agentIds: canonical(draft.agentIds),
        triggerStatuses: canonical(draft.triggerStatuses),
      });
      if (lifecycleGenerationRef.current === generation) onSaved(saved);
    } catch (cause) {
      if (lifecycleGenerationRef.current === generation) setError(parseCommandError(cause));
    } finally {
      if (lifecycleGenerationRef.current === generation) setPending(null);
    }
  };

  const remove = async () => {
    if (!rule || pending || !window.confirm(t("reminders.delete.confirm"))) return;
    const generation = lifecycleGenerationRef.current;
    setPending("delete");
    setError(null);
    try {
      await deleteReminderRule({ id: rule.id, expectedRevision: rule.revision });
      if (lifecycleGenerationRef.current === generation) onDeleted();
    } catch (cause) {
      if (lifecycleGenerationRef.current === generation) setError(parseCommandError(cause));
    } finally {
      if (lifecycleGenerationRef.current === generation) setPending(null);
    }
  };

  const setSound = (sound: ReminderSound) => setDraft((current) => ({ ...current, sound }));
  const soundEnabled = draft.sound.kind !== "none";

  return (
    <div className="reminder-editor" aria-busy={pending !== null || undefined}>
      <fieldset className="reminder-editor__form" disabled={pending !== null}>
      <div className="settings-control">
        <div className="settings-control__copy"><span>{t("reminders.agents")}</span></div>
        <div className="reminder-editor__checks">
          {AGENTS.map((agentId) => (
            <label key={agentId}><input type="checkbox" checked={draft.agentIds.includes(agentId)} onChange={() => setDraft((current) => ({ ...current, agentIds: toggle(current.agentIds, agentId) }))} />{agentId === "codex" ? "Codex" : agentId === "hermes" ? "Hermes" : agentId === "workbuddy" ? "WorkBuddy" : "claude"}</label>
          ))}
        </div>
      </div>
      <div className="settings-control">
        <div className="settings-control__copy"><span>{t("reminders.triggers")}</span></div>
        <div className="reminder-editor__checks">
          {TRIGGERS.map((status) => <label key={status}><input type="checkbox" checked={draft.triggerStatuses.includes(status)} onChange={() => setDraft((current) => ({ ...current, triggerStatuses: toggle(current.triggerStatuses, status) }))} />{t(`agents.status.${status}`)}</label>)}
        </div>
      </div>
      <div className="settings-control reminder-editor__delay">
        <label htmlFor="reminder-delay">{t("reminders.delay")}</label>
        <input id="reminder-delay" aria-label={t("reminders.delay")} type="number" min="0" max="604800" value={draft.delaySeconds} onChange={(event) => setDraft((current) => ({ ...current, delaySeconds: Number(event.target.value) }))} />
      </div>
      <div className="settings-control">
        <div className="settings-control__copy"><span>{t("reminders.channels")}</span></div>
        <div className="reminder-editor__checks">
          <label><input type="checkbox" checked={soundEnabled} onChange={() => setSound(soundEnabled ? { kind: "none" } : { kind: "builtin", soundId: "systemNotification" })} />{t("reminders.channels.sound")}</label>
          <label><input type="checkbox" checked={draft.toastEnabled} onChange={() => setDraft((current) => ({ ...current, toastEnabled: !current.toastEnabled }))} />{t("reminders.channels.toast")}</label>
          <label><input type="checkbox" checked={draft.windowEnabled} onChange={() => setDraft((current) => ({ ...current, windowEnabled: !current.windowEnabled }))} />{t("reminders.channels.window")}</label>
        </div>
        {soundEnabled && <div className="reminder-editor__sound" role="radiogroup" aria-label={t("reminders.channels.sound")}>
          <label><input type="radio" name="reminder-sound" checked={draft.sound.kind === "builtin"} onChange={() => setSound({ kind: "builtin", soundId: "systemNotification" })} />{t("reminders.sound.default")}</label>
          <label><input type="radio" name="reminder-sound" checked={draft.sound.kind === "localFile"} onChange={() => setSound({ kind: "localFile", canonicalPath: "" })} />{t("reminders.sound.local")}</label>
          <label><input type="radio" name="reminder-sound" checked={draft.sound.kind === "none"} onChange={() => setSound({ kind: "none" })} />{t("reminders.sound.none")}</label>
        </div>}
        {draft.sound.kind === "localFile" && <div className="reminder-editor__local"><button type="button" className="settings-choice" disabled={pending !== null} onClick={() => void chooseSound()}>{t("reminders.local.choose")}</button>{localFileName(draft.sound) && <span>{localFileName(draft.sound)}</span>}</div>}
      </div>
      <div className="settings-control reminder-editor__enabled">
        <label><input type="checkbox" checked={draft.enabled} onChange={() => setDraft((current) => ({ ...current, enabled: !current.enabled }))} />{t(draft.enabled ? "reminders.enabled" : "reminders.disabled")}</label>
        {!draft.enabled && <p>{t("reminders.disable.confirm")}</p>}
      </div>
      {validation && <p className="settings-error" role="alert">{validation}</p>}
      {errorMessage && <p className="settings-error" role="alert">{errorMessage}</p>}
      {error?.code === "conflict" && <button type="button" className="settings-choice" disabled={pending !== null} onClick={() => void onReload()}>{t("reminders.reload")}</button>}
      <div className="reminder-editor__actions">
        <button type="button" className="settings-choice" disabled={pending !== null || Boolean(validation)} onClick={() => void save()}>{t("action.save")}</button>
        {rule && <button type="button" className="settings-choice reminder-editor__delete" disabled={pending !== null} onClick={() => void remove()}>{t("reminders.delete")}</button>}
      </div>
      </fieldset>
    </div>
  );
}
