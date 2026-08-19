import { useCallback, useEffect, useMemo, useReducer, useRef, useState, type CSSProperties } from "react";
import {
  ArrowLeft,
  BookOpen,
  Bot,
  Code2,
  Database,
  FolderCog,
  Info,
  PanelTop,
  Settings2,
  ShieldAlert,
  SlidersHorizontal,
  type LucideIcon,
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import aislandLogoLight from "../../assets/brand/aisland-logo-light.svg";
import { useI18n } from "../../i18n/I18nProvider";
import { type TranslationKey } from "../../i18n/catalog";
import { checkForUpdate, checkStorageIntegrity, getDiagnostics, getGeneralSettings, installUpdate, listReminderRules, listServiceHealth, saveGeneralSettings } from "../../api/commands";
import { parseCommandError } from "../../api/commandError";
import type { CommandError, DiagnosticEvent, GeneralSettings, ReminderRule, ServiceHealthSnapshot, UpdateInstallEvent } from "../../api/contracts";
import { beginServiceHealthSubscription, type ServiceHealthSubscriptionHandle } from "../../api/events";
import { SETTINGS_CATEGORIES, settingsCategoryById } from "../../settings/catalog";
import { parseSettingsDetailId, type SettingsCategory, type SettingsRoute } from "../../settings/types";
import AgentProfilesSettings from "../../settings/AgentProfilesSettings";
import DiagnosticsSettings from "../../settings/DiagnosticsSettings";
import ReminderSettings from "../../settings/ReminderSettings";
import MonitorSettings from "../../settings/MonitorSettings";
import type { IslandExpansionMotion } from "../../types";
import { scaleFromSliderPosition, sliderPositionFromScale, windowScalePercent } from "../../windowGeometry";
import StatusDot from "../StatusDot";
import { AGENT_STATUS_COLOR } from "../agentStatusPresentation";
import SettingRow from "./SettingRow";

const EXPANSION_MOTION_OPTIONS: readonly { value: IslandExpansionMotion; key: TranslationKey }[] = [
  { value: "elastic", key: "settings.expansionMotion.elastic" },
  { value: "smooth", key: "settings.expansionMotion.smooth" },
  { value: "swift", key: "settings.expansionMotion.swift" },
];

const CATEGORY_ICONS = {
  general: Settings2,
  display: PanelTop,
  storage: Database,
  agents: Bot,
  reminders: ShieldAlert,
  modules: FolderCog,
  diagnostics: SlidersHorizontal,
  about: Info,
} satisfies Record<SettingsCategory, LucideIcon>;

function parentRoute(route: SettingsRoute): SettingsRoute | null {
  if (route.level === "detail") return { level: "category", category: route.category };
  if (route.level === "category") return { level: "root" };
  return null;
}

type SettingsViewProps = {
  scale: number;
  onScaleChange: (scale: number) => void;
  glassTransparency: number;
  onGlassTransparencyChange: (transparency: number) => void;
  expansionMotion: IslandExpansionMotion;
  onExpansionMotionChange: (motion: IslandExpansionMotion) => void;
  compactWindowEnabled: boolean;
  onCompactWindowEnabledChange: (enabled: boolean) => void;
  notificationPopupEnabled: boolean;
  onNotificationPopupEnabledChange: (enabled: boolean) => void;
  onExitSettings: () => void;
  routeResetToken?: number | null;
  entrySequence?: number | null;
  onEntryHandled?: (sequence: number) => void;
  agentProfileFocusId?: string | null;
};

export default function SettingsView({
  scale,
  onScaleChange,
  glassTransparency,
  onGlassTransparencyChange,
  expansionMotion,
  onExpansionMotionChange,
  compactWindowEnabled,
  onCompactWindowEnabledChange,
  notificationPopupEnabled,
  onNotificationPopupEnabledChange,
  onExitSettings,
  routeResetToken,
  entrySequence = null,
  onEntryHandled,
  agentProfileFocusId = null,
}: SettingsViewProps) {
  const { language, languageError, languagePending, setLanguage, t } = useI18n();
  const [route, setRoute] = useState<SettingsRoute>({ level: "root" });
  const [diagnosticsHealth, setDiagnosticsHealth] = useState<ServiceHealthSnapshot[]>([]);
  const [diagnosticsEvents, setDiagnosticsEvents] = useState<DiagnosticEvent[]>([]);
  const [storageIntegrity, setStorageIntegrity] = useState<"unknown" | "checking" | "ok" | "failed">("unknown");
  const [diagnosticsError, setDiagnosticsError] = useState<CommandError | null>(null);
  const [listenerError, setListenerError] = useState<CommandError | null>(null);
  const [diagnosticsLoading, setDiagnosticsLoading] = useState(false);
  const [retryPending, setRetryPending] = useState(false);
  const [reminderRules, setReminderRules] = useState<ReminderRule[]>([]);
  const [remindersLoading, setRemindersLoading] = useState(false);
  const [generalSettings, setGeneralSettings] = useState<GeneralSettings | null>(null);
  const [generalSettingsLoading, setGeneralSettingsLoading] = useState(false);
  const [generalSettingsPending, setGeneralSettingsPending] = useState(false);
  const [generalSettingsError, setGeneralSettingsError] = useState<CommandError | null>(null);
  const [updateState, setUpdateState] = useState<"idle" | "checking" | "installing" | "upToDate" | "installed" | "failed">("idle");
  const [updateVersion, setUpdateVersion] = useState<string | null>(null);
  const [updateProgress, setUpdateProgress] = useState<number | null>(null);
  const [updateError, setUpdateError] = useState<CommandError | null>(null);
  const [statusPreview, setStatusPreview] = useState<"working" | "idle">("idle");
  const [statusPreviewExpanded, setStatusPreviewExpanded] = useState(false);
  const [diagnosticsSession, restartDiagnosticsSession] = useReducer((value: number) => value + 1, 0);
  const handledRouteResetTokenRef = useRef<number | null>(null);
  const handledTraySequenceRef = useRef<number | null>(null);
  const diagnosticsGenerationRef = useRef(0);
  const diagnosticsActiveRef = useRef(false);
  const diagnosticsSubscriptionRef = useRef<ServiceHealthSubscriptionHandle | null>(null);
  const retryInFlightRef = useRef(false);
  const remindersActiveRef = useRef(false);
  const reminderRulesGenerationRef = useRef(0);
  const reminderRouteGenerationRef = useRef(0);
  const category = route.level === "root" ? null : settingsCategoryById(route.category);
  const scaleSliderPosition = Math.round(sliderPositionFromScale(scale));
  const scalePercent = windowScalePercent(scale);
  const routeKey = route.level === "root"
    ? "root"
    : route.level === "category"
      ? route.category
      : `${route.category}/${route.detail}`;

  const summaries = useMemo<Record<SettingsCategory, string>>(() => ({
    general: t(language === "zh-CN" ? "settings.language.zhCN" : "settings.language.enUS"),
    display: `${scalePercent}% · ${glassTransparency}%`,
    storage: t("settings.summary.storage"),
    agents: t("settings.summary.agents"),
    reminders: t("settings.summary.reminders"),
    modules: t("settings.summary.modules"),
    diagnostics: t("settings.categories.diagnostics.description"),
    about: t("settings.summary.about"),
  }), [glassTransparency, language, scalePercent, t]);
  const diagnosticsActive = route.level === "category" && route.category === "diagnostics";
  const remindersActive = route.level !== "root" && route.category === "reminders";
  const generalActive = route.level === "category" && route.category === "general";
  const ownsDiagnosticsGeneration = useCallback(
    (generation: number) =>
      diagnosticsActiveRef.current && diagnosticsGenerationRef.current === generation,
    [],
  );

  useEffect(() => {
    if (!generalActive) return;
    let active = true;
    setGeneralSettingsLoading(true);
    setGeneralSettingsError(null);
    void getGeneralSettings()
      .then((settings) => {
        if (active) setGeneralSettings(settings);
      })
      .catch((error) => {
        if (active) setGeneralSettingsError(parseCommandError(error));
      })
      .finally(() => {
        if (active) setGeneralSettingsLoading(false);
      });
    return () => { active = false; };
  }, [generalActive]);

  const toggleLaunchAtStartup = useCallback(async () => {
    if (generalSettings === null || generalSettingsPending) return;
    setGeneralSettingsPending(true);
    setGeneralSettingsError(null);
    try {
      const saved = await saveGeneralSettings({
        launchAtStartup: !generalSettings.launchAtStartup,
        expectedRevision: generalSettings.revision,
      });
      setGeneralSettings(saved);
    } catch (error) {
      setGeneralSettingsError(parseCommandError(error));
    } finally {
      setGeneralSettingsPending(false);
    }
  }, [generalSettings, generalSettingsPending]);

  const synchronizeUpdate = useCallback(async () => {
    if (updateState === "checking" || updateState === "installing") return;
    setUpdateState("checking");
    setUpdateError(null);
    setUpdateProgress(null);
    try {
      const available = await checkForUpdate();
      if (available.status === "upToDate") {
        setUpdateVersion(available.currentVersion);
        setUpdateState("upToDate");
        return;
      }
      setUpdateVersion(available.latestVersion);
      setUpdateState("installing");
      const installed = await installUpdate((event: UpdateInstallEvent) => {
        if (event.event !== "progress" || event.total === null || event.total <= 0) return;
        setUpdateProgress(Math.min(100, Math.round((event.downloaded / event.total) * 100)));
      });
      setUpdateVersion(installed.installedVersion);
      setUpdateProgress(100);
      setUpdateState("installed");
    } catch (error) {
      setUpdateError(parseCommandError(error));
      setUpdateState("failed");
    }
  }, [updateState]);

  useEffect(() => {
    if (!diagnosticsActive) return;
    let active = true;
    let dispose: () => void = () => undefined;
    const generation = diagnosticsGenerationRef.current + 1;
    diagnosticsGenerationRef.current = generation;
    diagnosticsActiveRef.current = true;
    const isCurrent = () => active && ownsDiagnosticsGeneration(generation);
    setDiagnosticsLoading(true);
    setStorageIntegrity("unknown");
    setListenerError(null);

    const finishRetry = () => {
      if (isCurrent() && retryInFlightRef.current) {
        retryInFlightRef.current = false;
        setRetryPending(false);
      }
    };

    const handle = beginServiceHealthSubscription(
      (error) => {
        if (isCurrent()) setListenerError(error);
      },
      (snapshot) => {
        if (isCurrent()) setDiagnosticsHealth(snapshot);
      },
    );
    diagnosticsSubscriptionRef.current = handle;

    const load = async () => {
      try {
        const subscription = await handle.ready;
        dispose = handle.dispose;
        if (!isCurrent()) {
          dispose();
          return;
        }
        setDiagnosticsHealth(subscription.initial);
        const events = await getDiagnostics({ limit: 50 });
        if (isCurrent()) {
          setDiagnosticsEvents(events);
          setDiagnosticsError(null);
          setDiagnosticsLoading(false);
          finishRetry();
        }
      } catch (error) {
        if (isCurrent()) {
          setDiagnosticsError(parseCommandError(error));
          setDiagnosticsLoading(false);
          finishRetry();
        }
      }
    };

    void load();
    return () => {
      active = false;
      if (diagnosticsGenerationRef.current === generation) {
        diagnosticsGenerationRef.current += 1;
        diagnosticsActiveRef.current = false;
      }
      if (diagnosticsSubscriptionRef.current === handle) {
        diagnosticsSubscriptionRef.current = null;
      }
      handle.dispose();
    };
  }, [diagnosticsActive, diagnosticsSession, ownsDiagnosticsGeneration]);

  const checkIntegrity = useCallback(async () => {
    const generation = diagnosticsGenerationRef.current;
    if (!ownsDiagnosticsGeneration(generation)) return;
    setStorageIntegrity("checking");
    try {
      await checkStorageIntegrity();
      if (!ownsDiagnosticsGeneration(generation)) return;
      const [health, events] = await Promise.all([
        listServiceHealth(),
        getDiagnostics({ limit: 50 }),
      ]);
      if (!ownsDiagnosticsGeneration(generation)) return;
      setStorageIntegrity("ok");
      setDiagnosticsHealth(health);
      setDiagnosticsEvents(events);
      setDiagnosticsError(null);
    } catch (error) {
      const typedError = parseCommandError(error);
      if (ownsDiagnosticsGeneration(generation)) {
        setStorageIntegrity("failed");
        setDiagnosticsError(typedError);
      }
      throw typedError;
    }
  }, [ownsDiagnosticsGeneration]);

  const retryDiagnostics = useCallback(async () => {
    if (!diagnosticsActiveRef.current || retryInFlightRef.current) return;
    retryInFlightRef.current = true;
    setRetryPending(true);
    diagnosticsSubscriptionRef.current?.dispose();
    diagnosticsSubscriptionRef.current = null;
    diagnosticsGenerationRef.current += 1;
    diagnosticsActiveRef.current = false;
    restartDiagnosticsSession();
  }, []);

  const refreshReminderRules = useCallback(async () => {
    const generation = reminderRulesGenerationRef.current + 1;
    reminderRulesGenerationRef.current = generation;
    const rules = await listReminderRules();
    if (remindersActiveRef.current && reminderRulesGenerationRef.current === generation) setReminderRules(rules);
  }, []);

  useEffect(() => {
    if (!remindersActive) {
      remindersActiveRef.current = false;
      reminderRulesGenerationRef.current += 1;
      return;
    }
    let active = true;
    remindersActiveRef.current = true;
    setRemindersLoading(true);
    void refreshReminderRules().catch(() => undefined).finally(() => { if (active) setRemindersLoading(false); });
    return () => {
      active = false;
      remindersActiveRef.current = false;
      reminderRulesGenerationRef.current += 1;
    };
  }, [refreshReminderRules, remindersActive]);

  const goBack = () => {
    const parent = parentRoute(route);
    if (parent) {
      if (route.level !== "root" && route.category === "reminders") reminderRouteGenerationRef.current += 1;
      setRoute(parent);
    }
  };

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      const parent = parentRoute(route);
      if (parent) {
        if (route.level !== "root" && route.category === "reminders") reminderRouteGenerationRef.current += 1;
        setRoute(parent);
      } else {
        onExitSettings();
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onExitSettings, route]);

  useEffect(() => {
    if (agentProfileFocusId === null) return;
    setRoute((current) => current.level === "category" && current.category === "agents"
      ? current
      : { level: "category", category: "agents" });
  }, [agentProfileFocusId]);

  useEffect(() => {
    if (agentProfileFocusId !== null) {
      handledRouteResetTokenRef.current = routeResetToken ?? null;
      return;
    }
    if (
      routeResetToken == null ||
      handledRouteResetTokenRef.current === routeResetToken
    ) {
      return;
    }

    if (route.level !== "root") {
      if (route.category === "reminders") reminderRouteGenerationRef.current += 1;
      setRoute({ level: "root" });
      return;
    }

    handledRouteResetTokenRef.current = routeResetToken;
  }, [agentProfileFocusId, routeResetToken, route.level]);

  useEffect(() => {
    if (
      entrySequence === null ||
      handledTraySequenceRef.current === entrySequence ||
      route.level !== "root"
    ) {
      return;
    }

    handledTraySequenceRef.current = entrySequence;
    onEntryHandled?.(entrySequence);
  }, [entrySequence, onEntryHandled, route.level]);

  if (route.level === "root") {
    return (
      <section key={routeKey} className="settings-view settings-page" aria-label={t("tab.settings")}>
        <div className="settings-root" role="list">
          {SETTINGS_CATEGORIES.map((entry) => (
            <SettingRow
              key={entry.id}
              icon={CATEGORY_ICONS[entry.id]}
              label={t(entry.labelKey)}
              summary={
                entry.availability === "available" ? (
                  <span className="setting-value-pill">{summaries[entry.id]}</span>
                ) : summaries[entry.id]
              }
              onActivate={() => setRoute({ level: "category", category: entry.id })}
            />
          ))}
        </div>
      </section>
    );
  }

  if (!category) return null;

  if (route.level === "detail") {
    const detail = parseSettingsDetailId(route.detail);
    if (!detail) return null;
    if ("thresholdId" in detail && route.category === "diagnostics") {
      const detailTitle = detail.thresholdId === "new" ? t("monitor.threshold.new") : t("monitor.thresholds");
      return (
        <section key={routeKey} className="settings-view settings-page" aria-label={detailTitle}>
          <header className="settings-header">
            <button className="settings-back" type="button" aria-label={t("action.back")} title={t("action.back")} onPointerDown={(event) => event.stopPropagation()} onClick={goBack}><ArrowLeft size={16} strokeWidth={1.5} aria-hidden="true" /></button>
            <h2 title={detailTitle}>{detailTitle}</h2>
          </header>
          <div className="settings-body">
            <MonitorSettings
              thresholdId={detail.thresholdId}
              onThresholdSaved={(saved) => {
                if (detail.thresholdId === "new") setRoute({ level: "detail", category: "diagnostics", detail: `monitorThreshold:${saved.id}` });
              }}
              onThresholdDeleted={() => setRoute({ level: "category", category: "diagnostics" })}
              onThresholdMissing={() => setRoute({ level: "category", category: "diagnostics" })}
            />
          </div>
        </section>
      );
    }
    if ("reminderId" in detail && route.category === "reminders") {
      const rule = detail.reminderId === "new" ? null : reminderRules.find((candidate) => candidate.id === detail.reminderId) ?? null;
      if (detail.reminderId !== "new" && !rule) return null;
      const detailTitle = detail.reminderId === "new" ? t("reminders.new") : t("reminders.title");
      const detailRouteGeneration = reminderRouteGenerationRef.current;
      return (
        <section key={routeKey} className="settings-view settings-page" aria-label={detailTitle}>
          <header className="settings-header">
            <button className="settings-back" type="button" aria-label={t("action.back")} title={t("action.back")} onPointerDown={(event) => event.stopPropagation()} onClick={goBack}><ArrowLeft size={16} strokeWidth={1.5} aria-hidden="true" /></button>
            <h2 title={detailTitle}>{detailTitle}</h2>
          </header>
          <div className="settings-body">
            <ReminderSettings
              rule={rule}
              onSaved={(saved) => {
                if (reminderRouteGenerationRef.current !== detailRouteGeneration) return;
                reminderRulesGenerationRef.current += 1;
                setReminderRules((current) => [...current.filter((candidate) => candidate.id !== saved.id), saved]);
                if (detail.reminderId === "new") setRoute({ level: "detail", category: "reminders", detail: `reminderRule:${saved.id}` });
              }}
              onDeleted={() => {
                if (reminderRouteGenerationRef.current !== detailRouteGeneration) return;
                reminderRulesGenerationRef.current += 1;
                setRoute({ level: "category", category: "reminders" });
                void refreshReminderRules();
              }}
              onReload={refreshReminderRules}
            />
          </div>
        </section>
      );
    }
    return null;
  }

  return (
    <section key={routeKey} className="settings-view settings-page" aria-label={t(category.labelKey)}>
      <header className="settings-header">
        <button
          className="settings-back"
          type="button"
          aria-label={t("action.back")}
          title={t("action.back")}
          onPointerDown={(event) => event.stopPropagation()}
          onClick={goBack}
        >
          <ArrowLeft size={16} strokeWidth={1.5} aria-hidden="true" />
        </button>
        <h2 title={t(category.labelKey)}>{t(category.labelKey)}</h2>
      </header>
      <div className="settings-body">
        {category.id === "general" && (
          <>
            <div className="settings-control">
              <div className="settings-control__copy">
                <span>{t("settings.language")}</span>
                <span>{t("settings.languageHint")}</span>
              </div>
              <div className="settings-choice-group" role="group" aria-label={t("settings.language")}>
                {(["zh-CN", "en-US"] as const).map((candidate) => (
                  <button
                    key={candidate}
                    type="button"
                    className={`settings-choice${language === candidate ? " settings-choice--active" : ""}`}
                    aria-pressed={language === candidate}
                    title={t(candidate === "zh-CN" ? "settings.language.zhCN" : "settings.language.enUS")}
                    disabled={languagePending}
                    onPointerDown={(event) => event.stopPropagation()}
                    onClick={() => void setLanguage(candidate)}
                  >
                    {t(candidate === "zh-CN" ? "settings.language.zhCN" : "settings.language.enUS")}
                  </button>
                ))}
              </div>
              {languageError && <p className="settings-error">{languageError}</p>}
            </div>
            <div className="settings-control settings-toggle-row" aria-busy={generalSettingsLoading || undefined}>
              <div className="settings-control__copy">
                <span>{t("settings.launchAtStartup")}</span>
                <span>{t("settings.launchAtStartupHint")}</span>
              </div>
              <button
                type="button"
                className="settings-switch"
                role="switch"
                aria-label={t("settings.launchAtStartup")}
                aria-checked={generalSettings?.launchAtStartup ?? false}
                disabled={generalSettings === null || generalSettingsLoading || generalSettingsPending}
                title={t(generalSettings?.launchAtStartup ? "settings.state.enabled" : "settings.state.disabled")}
                onClick={() => void toggleLaunchAtStartup()}
              >
                <span className="settings-switch__thumb" aria-hidden="true" />
              </button>
            </div>
            {generalSettingsError && <p className="settings-error">{t("settings.generalSaveFailed")}</p>}
          </>
        )}
        {category.id === "display" && (
          <>
            <div className="settings-control settings-glass-control">
              <div className="settings-control__heading">
                <div className="settings-control__copy">
                  <span>{t("settings.glassTransparency")}</span>
                  <span>{t("settings.glassTransparencyHint")}</span>
                </div>
                <output className="settings-range__value" htmlFor="glass-transparency">
                  {glassTransparency}%
                </output>
              </div>
              <div className="settings-range">
                <input
                  id="glass-transparency"
                  type="range"
                  min="0"
                  max="100"
                  step="1"
                  value={glassTransparency}
                  aria-label={t("settings.glassTransparency")}
                  aria-valuetext={`${glassTransparency}%`}
                  style={{ "--range-progress": `${glassTransparency}%` } as CSSProperties}
                  onChange={(event) => onGlassTransparencyChange(Number(event.currentTarget.value))}
                />
                <div className="settings-range__labels" aria-hidden="true">
                  <span>{t("settings.glassSolid")}</span>
                  <span>{t("settings.glassClear")}</span>
                </div>
              </div>
            </div>
            <div className="settings-control settings-scale-control">
              <div className="settings-control__heading">
                <div className="settings-control__copy">
                  <span>{t("settings.scale")}</span>
                  <span>{t("settings.scaleHint")}</span>
                </div>
                <output className="settings-range__value" htmlFor="window-scale">
                  {scalePercent}%
                </output>
              </div>
              <div className="settings-range">
                <input
                  id="window-scale"
                  type="range"
                  min="0"
                  max="100"
                  step="1"
                  value={scaleSliderPosition}
                  aria-label={t("settings.scale")}
                  aria-valuetext={`${scalePercent}%`}
                  style={{ "--range-progress": `${scaleSliderPosition}%` } as CSSProperties}
                  onPointerDown={(event) => event.stopPropagation()}
                  onChange={(event) => onScaleChange(scaleFromSliderPosition(Number(event.currentTarget.value)))}
                />
                <div className="settings-range__labels" aria-hidden="true">
                  <span>80%</span>
                  <span>220%</span>
                </div>
              </div>
            </div>
            <div className="settings-control settings-motion-control">
              <div className="settings-control__copy">
                <span>{t("settings.expansionMotion")}</span>
                <span>{t("settings.expansionMotionHint")}</span>
              </div>
              <div className="settings-motion-control__toolbar">
                <div className="settings-choice-group" role="group" aria-label={t("settings.expansionMotion")}>
                  {EXPANSION_MOTION_OPTIONS.map((option) => (
                    <button
                      key={option.value}
                      type="button"
                      className={`settings-choice${expansionMotion === option.value ? " settings-choice--active" : ""}`}
                      aria-pressed={expansionMotion === option.value}
                      onClick={() => onExpansionMotionChange(option.value)}
                    >
                      {t(option.key)}
                    </button>
                  ))}
                </div>
                <button
                  type="button"
                  className="settings-motion-preview-toggle"
                  aria-expanded={statusPreviewExpanded}
                  onClick={() => setStatusPreviewExpanded((expanded) => !expanded)}
                >
                  {t("settings.statusPreview")}
                </button>
              </div>
              {statusPreviewExpanded && (
                <div className="settings-motion-preview" data-testid="agent-state-motion-preview" data-preview-status={statusPreview}>
                  <div className="settings-motion-preview__sample" role="status" aria-live="polite">
                    <span className="settings-motion-preview__orb" aria-hidden="true">
                      <StatusDot color={statusPreview === "working" ? AGENT_STATUS_COLOR.running : AGENT_STATUS_COLOR.idle} pulse={statusPreview === "working"} />
                    </span>
                    <span>{t(statusPreview === "working" ? "settings.statusPreview.working" : "settings.statusPreview.idle")}</span>
                  </div>
                  <div className="settings-choice-group" role="group" aria-label={t("settings.statusPreview")} title={t("settings.statusPreviewHint")}>
                    {(["working", "idle"] as const).map((status) => (
                      <button
                        key={status}
                        type="button"
                        className={`settings-choice${statusPreview === status ? " settings-choice--active" : ""}`}
                        aria-pressed={statusPreview === status}
                        onClick={() => setStatusPreview(status)}
                      >
                        {t(status === "working" ? "settings.statusPreview.working" : "settings.statusPreview.idle")}
                      </button>
                    ))}
                  </div>
                </div>
              )}
            </div>
            <div className="settings-control settings-toggle-row">
              <div className="settings-control__copy">
                <span>{t("settings.compactWindow")}</span>
                <span>{t("settings.compactWindowHint")}</span>
              </div>
              <button
                type="button"
                className="settings-switch"
                role="switch"
                aria-label={t("settings.compactWindow")}
                aria-checked={compactWindowEnabled}
                title={t(compactWindowEnabled ? "settings.state.enabled" : "settings.state.disabled")}
                onClick={() => onCompactWindowEnabledChange(!compactWindowEnabled)}
              >
                <span className="settings-switch__thumb" aria-hidden="true" />
              </button>
            </div>
          </>
        )}
        {category.id === "diagnostics" && (
          <div className="settings-diagnostics-stack" aria-busy={diagnosticsLoading || undefined}>
            <DiagnosticsSettings health={diagnosticsHealth} events={diagnosticsEvents} storageIntegrity={storageIntegrity} onCheckStorageIntegrity={checkIntegrity} onRetry={retryDiagnostics} retryPending={retryPending} error={diagnosticsError ?? listenerError} />
            <MonitorSettings onSelectThreshold={(id) => setRoute({ level: "detail", category: "diagnostics", detail: `monitorThreshold:${id}` })} />
          </div>
        )}
        {category.id === "agents" && <AgentProfilesSettings focusProfileId={agentProfileFocusId} />}
        {category.id === "reminders" && (
          <>
            <div className="settings-control settings-toggle-row">
              <div className="settings-control__copy">
                <span>{t("settings.notificationPopup")}</span>
                <span>{t("settings.notificationPopupHint")}</span>
              </div>
              <button
                type="button"
                className="settings-switch"
                role="switch"
                aria-label={t("settings.notificationPopup")}
                aria-checked={notificationPopupEnabled}
                title={t(notificationPopupEnabled ? "settings.state.enabled" : "settings.state.disabled")}
                onClick={() => onNotificationPopupEnabledChange(!notificationPopupEnabled)}
              >
                <span className="settings-switch__thumb" aria-hidden="true" />
              </button>
            </div>
            <div className="settings-root" role="list" aria-busy={remindersLoading || undefined}>
              <button type="button" className="settings-choice reminder-list__new" onClick={() => setRoute({ level: "detail", category: "reminders", detail: "reminderRule:new" })}>{t("reminders.new")}</button>
              {reminderRules.map((rule) => (
                <SettingRow key={rule.id} label={rule.agentIds.join(", ")} summary={<span className="setting-value-pill">{t(rule.enabled ? "reminders.enabled" : "reminders.disabled")}</span>} onActivate={() => setRoute({ level: "detail", category: "reminders", detail: `reminderRule:${rule.id}` })} />
              ))}
            </div>
          </>
        )}
        {category.id === "about" && (
          <div className="settings-about">
            <div className="settings-about__hero">
              <img className="settings-about__logo" src={aislandLogoLight} alt="AIsland" />
              <div>
                <span>{t("settings.aboutDescription")}</span>
              </div>
            </div>
            <div className="settings-about__link-row">
              <Code2 size={17} strokeWidth={1.6} aria-hidden="true" />
              <div className="settings-about__link-copy">
                <span>GitHub</span>
                <span>https://github.com/ErdonChen/AIsland</span>
              </div>
              <button
                type="button"
                className="settings-choice settings-about__action"
                onClick={() => void invoke("open_aisland_github")}
              >
                {t("settings.openGithub")}
              </button>
            </div>
            <button
              type="button"
              className="settings-about__guide"
              aria-label={t("settings.userGuide")}
              onClick={() => void invoke("open_project_readme")}
            >
              <BookOpen size={17} strokeWidth={1.6} aria-hidden="true" />
              <span>
                <strong>{t("settings.userGuide")}</strong>
                <small>{t("settings.userGuideHint")}</small>
              </span>
            </button>
            <div className="settings-about__update" aria-live="polite">
              <div className="settings-control__copy">
                <span>{t("settings.checkUpdate")}</span>
                <span>
                  {updateState === "checking" ? t("settings.updateChecking")
                      : updateState === "installing" ? t("settings.updateInstalling").replace("{progress}", String(updateProgress ?? 0))
                      : updateState === "upToDate" ? t("settings.updateCurrent").replace("{version}", updateVersion ?? "")
                        : updateState === "installed" ? t("settings.updateInstalled").replace("{version}", updateVersion ?? "")
                          : updateState === "failed" ? t("settings.updateUnavailable")
                            : t("settings.checkUpdateHint")}
                </span>
              </div>
              <button
                type="button"
                className="settings-choice settings-about__action"
                disabled={updateState === "checking" || updateState === "installing"}
                onClick={() => void synchronizeUpdate()}
              >
                {t("settings.checkUpdate")}
              </button>
            </div>
            {updateError && <p className="settings-error">{t("settings.updateTypedError").replace("{code}", updateError.code)}</p>}
          </div>
        )}
        {category.availability === "coming-soon" && (
          <div className="settings-coming-soon" aria-disabled="true">
            <span>{t("common.comingSoon")}</span>
            <span>{t("settings.readOnlyHint")}</span>
          </div>
        )}
      </div>
    </section>
  );
}
