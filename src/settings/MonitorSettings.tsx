import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  deleteMonitorThreshold,
  deleteProcessWatch,
  listMonitorThresholds,
  listProcessWatches,
  saveMonitorThreshold,
  saveProcessWatch,
} from "../api/commands";
import { parseCommandError } from "../api/commandError";
import type {
  CommandError,
  MonitorMetric,
  MonitorThreshold,
  ProcessWatch,
  SaveMonitorThresholdInput,
} from "../api/contracts";
import { useI18n } from "../i18n/I18nProvider";

type MonitorSettingsProps = {
  thresholdId?: "new" | string;
  onSelectThreshold?: (id: "new" | string) => void;
  onThresholdSaved?: (threshold: MonitorThreshold) => void;
  onThresholdDeleted?: () => void;
  onThresholdMissing?: () => void;
};

type ThresholdDraft = Omit<SaveMonitorThresholdInput, "id" | "expectedRevision">;

const METRICS: readonly MonitorMetric[] = [
  "cpuPercent", "memoryPercent", "diskReadBytesPerSecond", "diskWriteBytesPerSecond",
  "networkReceiveBytesPerSecond", "networkSendBytesPerSecond", "gpuPercent",
];

const PROCESS_NAME = /^[^\\/:*?"<>|\u0000-\u001f]{1,260}$/u;

function newThreshold(): ThresholdDraft {
  return {
    metric: "cpuPercent",
    comparator: "greaterThanOrEqual",
    thresholdValue: 90,
    holdSeconds: 30,
    cooldownSeconds: 300,
    sound: { kind: "builtin", soundId: "systemNotification" },
    toastEnabled: true,
    windowEnabled: true,
    enabled: true,
  };
}

function thresholdDraft(row: MonitorThreshold | null): ThresholdDraft {
  if (!row) return newThreshold();
  return {
    metric: row.metric,
    comparator: row.comparator,
    thresholdValue: row.thresholdValue,
    holdSeconds: row.holdSeconds,
    cooldownSeconds: row.cooldownSeconds,
    sound: row.sound,
    toastEnabled: row.toastEnabled,
    windowEnabled: row.windowEnabled,
    enabled: row.enabled,
  };
}

function metricLabel(metric: MonitorMetric, t: (key: never) => string) {
  const keys: Record<MonitorMetric, string> = {
    cpuPercent: "monitor.cpu",
    memoryPercent: "monitor.memory",
    diskReadBytesPerSecond: "monitor.diskRead",
    diskWriteBytesPerSecond: "monitor.diskWrite",
    networkReceiveBytesPerSecond: "monitor.networkReceive",
    networkSendBytesPerSecond: "monitor.networkSend",
    gpuPercent: "monitor.gpu",
  };
  return t(keys[metric] as never);
}

function replaceById<T extends { id: string }>(rows: T[], row: T) {
  return [...rows.filter((candidate) => candidate.id !== row.id), row];
}

export default function MonitorSettings({ thresholdId, onSelectThreshold, onThresholdSaved, onThresholdDeleted, onThresholdMissing }: MonitorSettingsProps) {
  const { t } = useI18n();
  const [watches, setWatches] = useState<ProcessWatch[]>([]);
  const [thresholds, setThresholds] = useState<MonitorThreshold[]>([]);
  const [thresholdsLoaded, setThresholdsLoaded] = useState(false);
  const [processName, setProcessName] = useState("");
  const [draft, setDraft] = useState<ThresholdDraft>(newThreshold);
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<CommandError | null>(null);
  const [localError, setLocalError] = useState<string | null>(null);
  const lifecycleRef = useRef(0);
  const watchesRequestRef = useRef(0);
  const thresholdsRequestRef = useRef(0);
  const draftDirtyRef = useRef(false);
  const missingReportedRef = useRef<string | null>(null);

  const reloadWatches = useCallback(async () => {
    const lifecycle = lifecycleRef.current;
    const request = ++watchesRequestRef.current;
    try {
      const nextWatches = await listProcessWatches();
      if (lifecycleRef.current !== lifecycle || watchesRequestRef.current !== request) return;
      setWatches(Array.isArray(nextWatches) ? nextWatches : []);
      setError(null);
    } catch (cause) {
      if (lifecycleRef.current === lifecycle && watchesRequestRef.current === request) setError(parseCommandError(cause));
    }
  }, []);

  const reloadThresholds = useCallback(async () => {
    const lifecycle = lifecycleRef.current;
    const request = ++thresholdsRequestRef.current;
    try {
      const nextThresholds = await listMonitorThresholds();
      if (lifecycleRef.current !== lifecycle || thresholdsRequestRef.current !== request) return;
      setThresholds(Array.isArray(nextThresholds) ? nextThresholds : []);
      setThresholdsLoaded(true);
      setError(null);
    } catch (cause) {
      if (lifecycleRef.current === lifecycle && thresholdsRequestRef.current === request) setError(parseCommandError(cause));
    }
  }, []);

  const reload = useCallback(async () => { await Promise.all([reloadWatches(), reloadThresholds()]); }, [reloadThresholds, reloadWatches]);

  useEffect(() => {
    const lifecycle = ++lifecycleRef.current;
    void reload();
    return () => { if (lifecycleRef.current === lifecycle) lifecycleRef.current += 1; };
  }, [reload]);

  const selectedThreshold = thresholdId && thresholdId !== "new"
    ? thresholds.find((row) => row.id === thresholdId) ?? null
    : null;
  const thresholdIdentityReady = thresholdId === "new" || selectedThreshold !== null;

  useEffect(() => {
    if (!thresholdId || thresholdId === "new" || !thresholdsLoaded || selectedThreshold || missingReportedRef.current === thresholdId) return;
    missingReportedRef.current = thresholdId;
    onThresholdMissing?.();
  }, [onThresholdMissing, selectedThreshold, thresholdId, thresholdsLoaded]);

  useEffect(() => {
    if (!thresholdId || draftDirtyRef.current) return;
    setDraft(thresholdDraft(selectedThreshold));
  }, [selectedThreshold, thresholdId]);

  const updateDraft = <K extends keyof ThresholdDraft>(key: K, value: ThresholdDraft[K]) => {
    draftDirtyRef.current = true;
    setDraft((current) => ({ ...current, [key]: value }));
  };

  const thresholdValidation = useMemo(() => {
    const percent = draft.metric === "cpuPercent" || draft.metric === "memoryPercent" || draft.metric === "gpuPercent";
    if (!Number.isFinite(draft.thresholdValue) || (percent ? draft.thresholdValue < 0 || draft.thresholdValue > 100 : draft.thresholdValue < 0)) return t("monitor.validation.threshold");
    if (!Number.isInteger(draft.holdSeconds) || draft.holdSeconds < 0 || draft.holdSeconds > 86400) return t("monitor.validation.hold");
    if (!Number.isInteger(draft.cooldownSeconds) || draft.cooldownSeconds < 0 || draft.cooldownSeconds > 604800) return t("monitor.validation.cooldown");
    if (draft.sound.kind === "none" && !draft.toastEnabled && !draft.windowEnabled) return t("reminders.validation.channels");
    return null;
  }, [draft, t]);

  const addWatch = async () => {
    const canonical = processName.trim();
    if (!PROCESS_NAME.test(canonical) || canonical === "." || canonical === "..") {
      setLocalError(t("monitor.processWatch.invalid"));
      return;
    }
    if (pending) return;
    const lifecycle = lifecycleRef.current;
    setPending("process:new"); setLocalError(null); setError(null);
    try {
      const saved = await saveProcessWatch({ id: null, processName: canonical, enabled: true, expectedRevision: null });
      if (lifecycleRef.current !== lifecycle) return;
      watchesRequestRef.current += 1;
      setWatches((current) => replaceById(current, saved));
      setProcessName("");
    } catch (cause) { if (lifecycleRef.current === lifecycle) { const next = parseCommandError(cause); setError(next); if (next.code === "conflict") await reloadWatches(); } }
    finally { if (lifecycleRef.current === lifecycle) setPending(null); }
  };

  const toggleWatch = async (watch: ProcessWatch) => {
    if (pending) return;
    const lifecycle = lifecycleRef.current;
    setPending(`process:${watch.id}`); setError(null);
    try {
      const saved = await saveProcessWatch({ id: watch.id, processName: watch.processName, enabled: !watch.enabled, expectedRevision: watch.revision });
      if (lifecycleRef.current !== lifecycle) return;
      watchesRequestRef.current += 1;
      setWatches((current) => replaceById(current, saved));
    } catch (cause) { if (lifecycleRef.current === lifecycle) { const next = parseCommandError(cause); setError(next); if (next.code === "conflict") await reloadWatches(); } }
    finally { if (lifecycleRef.current === lifecycle) setPending(null); }
  };

  const removeWatch = async (watch: ProcessWatch) => {
    if (pending || !window.confirm(t("monitor.processWatch.deleteConfirm"))) return;
    const lifecycle = lifecycleRef.current;
    setPending(`process:${watch.id}`); setError(null);
    try {
      await deleteProcessWatch({ id: watch.id, expectedRevision: watch.revision });
      if (lifecycleRef.current !== lifecycle) return;
      watchesRequestRef.current += 1;
      setWatches((current) => current.filter((candidate) => candidate.id !== watch.id));
    } catch (cause) { if (lifecycleRef.current === lifecycle) { const next = parseCommandError(cause); setError(next); if (next.code === "conflict") await reloadWatches(); } }
    finally { if (lifecycleRef.current === lifecycle) setPending(null); }
  };

  const saveThreshold = async () => {
    if (!thresholdId || !thresholdIdentityReady || pending || thresholdValidation) return;
    const lifecycle = lifecycleRef.current;
    setPending("threshold:save"); setError(null);
    try {
      const saved = await saveMonitorThreshold({
        id: selectedThreshold?.id ?? null,
        expectedRevision: selectedThreshold?.revision ?? null,
        ...draft,
      });
      if (lifecycleRef.current !== lifecycle) return;
      thresholdsRequestRef.current += 1;
      setThresholdsLoaded(true);
      draftDirtyRef.current = false;
      setThresholds((current) => replaceById(current, saved));
      setDraft(thresholdDraft(saved));
      onThresholdSaved?.(saved);
    } catch (cause) {
      if (lifecycleRef.current !== lifecycle) return;
      const next = parseCommandError(cause); setError(next);
      if (next.code === "conflict") await reloadThresholds();
    } finally { if (lifecycleRef.current === lifecycle) setPending(null); }
  };

  const removeThreshold = async () => {
    if (!selectedThreshold || pending || !window.confirm(t("monitor.threshold.deleteConfirm"))) return;
    const lifecycle = lifecycleRef.current;
    setPending("threshold:delete"); setError(null);
    try {
      await deleteMonitorThreshold({ id: selectedThreshold.id, expectedRevision: selectedThreshold.revision });
      if (lifecycleRef.current !== lifecycle) return;
      thresholdsRequestRef.current += 1;
      setThresholds((current) => current.filter((candidate) => candidate.id !== selectedThreshold.id));
      onThresholdDeleted?.();
    } catch (cause) {
      if (lifecycleRef.current !== lifecycle) return;
      const next = parseCommandError(cause); setError(next);
      if (next.code === "conflict") await reloadThresholds();
    } finally { if (lifecycleRef.current === lifecycle) setPending(null); }
  };

  if (thresholdId) {
    const soundEnabled = draft.sound.kind !== "none";
    return (
      <div className="monitor-settings monitor-threshold-editor" aria-busy={pending !== null || undefined}>
        <fieldset disabled={pending !== null}>
          <label>{t("monitor.threshold.metric")}<select value={draft.metric} onChange={(event) => updateDraft("metric", event.target.value as MonitorMetric)}>{METRICS.map((metric) => <option key={metric} value={metric}>{metricLabel(metric, t as never)}</option>)}</select></label>
          <label>{t("monitor.threshold.comparator")}<select value={draft.comparator} onChange={(event) => updateDraft("comparator", event.target.value as ThresholdDraft["comparator"])}><option value="greaterThanOrEqual">≥</option><option value="lessThanOrEqual">≤</option></select></label>
          <label>{t("monitor.threshold.value")}<input type="number" value={draft.thresholdValue} min="0" max={draft.metric.endsWith("Percent") ? 100 : undefined} onChange={(event) => updateDraft("thresholdValue", Number(event.target.value))} /></label>
          <label>{t("monitor.threshold.hold")}<input type="number" min="0" max="86400" value={draft.holdSeconds} onChange={(event) => updateDraft("holdSeconds", Number(event.target.value))} /></label>
          <label>{t("monitor.threshold.cooldown")}<input type="number" min="0" max="604800" value={draft.cooldownSeconds} onChange={(event) => updateDraft("cooldownSeconds", Number(event.target.value))} /></label>
          <div className="reminder-editor__checks">
            <label><input type="checkbox" checked={soundEnabled} onChange={() => updateDraft("sound", soundEnabled ? { kind: "none" } : { kind: "builtin", soundId: "systemNotification" })} />{t("reminders.channels.sound")}</label>
            <label><input type="checkbox" checked={draft.toastEnabled} onChange={() => updateDraft("toastEnabled", !draft.toastEnabled)} />{t("reminders.channels.toast")}</label>
            <label><input type="checkbox" checked={draft.windowEnabled} onChange={() => updateDraft("windowEnabled", !draft.windowEnabled)} />{t("reminders.channels.window")}</label>
          </div>
          <label><input type="checkbox" checked={draft.enabled} onChange={() => updateDraft("enabled", !draft.enabled)} />{t(draft.enabled ? "reminders.enabled" : "reminders.disabled")}</label>
          {!draft.enabled && <p>{t("monitor.threshold.disableConfirm")}</p>}
          {thresholdValidation && <p role="alert" className="settings-error">{thresholdValidation}</p>}
          {error && <p role="alert" className="settings-error">{t("monitor.error.save")}</p>}
          <div className="monitor-settings__actions">
            <button type="button" className="settings-choice" disabled={!thresholdIdentityReady || Boolean(thresholdValidation) || pending !== null} onClick={() => void saveThreshold()}>{t("action.save")}</button>
            {selectedThreshold && <button type="button" className="settings-choice" onClick={() => void removeThreshold()}>{t("monitor.threshold.delete")}</button>}
          </div>
        </fieldset>
      </div>
    );
  }

  return (
    <div className="monitor-settings">
      <section aria-label={t("monitor.processes")}>
        <h3>{t("monitor.processes")}</h3>
        <div className="monitor-settings__add">
          <label htmlFor="monitor-process-name">{t("monitor.processWatch.name")}</label>
          <input id="monitor-process-name" value={processName} onChange={(event) => setProcessName(event.target.value)} />
          <button type="button" className="settings-choice" disabled={pending !== null} onClick={() => void addWatch()}>{t("monitor.processWatch.add")}</button>
        </div>
        {localError && <p role="alert" className="settings-error">{localError}</p>}
        {watches.length === 0 ? <p>{t("monitor.processWatch.empty")}</p> : watches.map((watch) => (
          <div key={watch.id} className="monitor-settings__row">
            <span title={watch.processName}>{watch.processName}</span>
            <button type="button" className="settings-choice" disabled={pending !== null} onClick={() => void toggleWatch(watch)}>{t(watch.enabled ? "reminders.enabled" : "reminders.disabled")}</button>
            <button type="button" className="settings-choice" disabled={pending !== null} onClick={() => void removeWatch(watch)}>{t("monitor.processWatch.delete")}</button>
          </div>
        ))}
      </section>
      <section aria-label={t("monitor.thresholds")}>
        <h3>{t("monitor.thresholds")}</h3>
        <button type="button" className="settings-choice" onClick={() => onSelectThreshold?.("new")}>{t("monitor.threshold.new")}</button>
        {thresholds.map((threshold) => <button key={threshold.id} type="button" className="monitor-settings__row monitor-settings__threshold" onClick={() => onSelectThreshold?.(threshold.id)}><span>{metricLabel(threshold.metric, t as never)}</span><span>{threshold.thresholdValue}</span></button>)}
      </section>
      {error && <p role="alert" className="settings-error">{t("monitor.error.save")}</p>}
    </div>
  );
}
