import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { Bell, ChevronDown, ChevronUp, Minus, X } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import aislandMark from "../assets/brand/aisland-mark.svg";
import { beginAgentProfileStateSubscription, beginAgentStateSubscription, beginReminderDispatchSubscription } from "../api/events";
import { acknowledgeReminderNavigation, getPendingReminderNavigation, listNotificationHistory } from "../api/commands";
import { useI18n } from "../i18n/I18nProvider";
import { translateRegisteredMessage, type TranslationKey } from "../i18n/catalog";
import type { AgentEnvironment, AgentId, AgentProfilesSnapshot, AgentsSnapshot, AgentTriggerStatus, NotificationHistoryChangedPayload, NotificationHistoryItem, PendingReminderNavigation, ReminderDelivery } from "../api/contracts";
import AgentStatusSlots, { prioritizedAgentStatuses, sortAgentsByPriority, visibleAgentSummaries } from "./AgentStatusSlots";
import AgentsPage from "../pages/AgentsPage";
import type { CommittedAgentContext } from "../pages/AgentsPage";
import SettingsView from "./settings/SettingsView";
import DailyNotesPage from "../features/notes/DailyNotesPage";
import ClipboardPage from "../features/clipboard/ClipboardPage";
import MonitorPage from "../pages/MonitorPage";
import NotificationCenterPage from "../pages/NotificationCenterPage";
import TabBar from "./TabBar";
import StatusDot from "./StatusDot";
import { AGENT_STATUS_COLOR } from "./agentStatusPresentation";
import type { InitialState, IslandExpansionMotion, IslandMode, IslandPage } from "../types";

const SCALE_KEY = "aisland.window.scale";
const GLASS_TRANSPARENCY_KEY = "aisland.display.glassTransparency.v1";
const EXPANSION_MOTION_KEY = "aisland.display.expansionMotion.v1";
const COMPACT_WINDOW_KEY = "aisland.display.compactWindow.v1";
const NOTIFICATION_POPUP_KEY = "aisland.notifications.popup.v1";
const DEFAULT_GLASS_TRANSPARENCY = 58;
const COMPACT_EXPAND_DELAY_MS = 140;
const COMPACT_COLLAPSE_DELAY_MS = 280;
const NOTIFICATION_VISIBLE_MS = 8_000;
const PAGE_LABEL_KEYS: Record<IslandPage, TranslationKey> = {
  home: "tab.home",
  note: "tab.notes",
  clipboard: "tab.clipboard",
  monitor: "tab.monitor",
  notify: "tab.notifications",
  settings: "tab.settings",
};
const MIN_EXPANDED_HEIGHT = 306;
const MAX_EXPANDED_HEIGHT = 640;

interface PendingTrayNavigation {
  page: "settings";
  sequence: number;
}

type AgentReminderContext = {
  environment: AgentEnvironment;
  taskId: string;
  triggerStatus: AgentTriggerStatus;
};

type PendingAgentRoute = AgentReminderContext & {
  sequence: number;
  agentId: AgentId;
};

type SettingsRouteEntrySource = "tab" | "tray";

type SettingsRouteEntry = {
  token: number;
  source: SettingsRouteEntrySource;
};

type IslandNotification =
  | { kind: "reminder"; delivery: ReminderDelivery }
  | { kind: "system"; item: NotificationHistoryItem };

function clampScale(value: number) {
  return Number.isFinite(value) ? Math.min(1.4, Math.max(0.75, value)) : 1;
}

function clampGlassTransparency(value: number) {
  return Number.isFinite(value) ? Math.min(100, Math.max(0, Math.round(value))) : DEFAULT_GLASS_TRANSPARENCY;
}

function loadGlassTransparency() {
  try {
    const stored = localStorage.getItem(GLASS_TRANSPARENCY_KEY);
    return stored === null ? DEFAULT_GLASS_TRANSPARENCY : clampGlassTransparency(Number(stored));
  } catch (error) {
    console.error("Failed to load glass transparency", error);
    return DEFAULT_GLASS_TRANSPARENCY;
  }
}

function loadBooleanPreference(key: string, fallback: boolean) {
  try {
    const stored = localStorage.getItem(key);
    return stored === null ? fallback : stored === "true";
  } catch (error) {
    console.error(`Failed to load ${key}`, error);
    return fallback;
  }
}

function loadExpansionMotion(): IslandExpansionMotion {
  try {
    const stored = localStorage.getItem(EXPANSION_MOTION_KEY);
    return stored === "smooth" || stored === "swift" || stored === "elastic" ? stored : "elastic";
  } catch (error) {
    console.error("Failed to load expansion motion", error);
    return "elastic";
  }
}

export class LatestWinsSingleFlight<T> {
  private desired: T;
  private inFlight = false;
  private forcePending = false;
  private idleWaiters: Array<(confirmed: T) => void> = [];

  constructor(
    private confirmed: T,
    private readonly perform: (value: T) => Promise<void>,
    private readonly onCommitted: (value: T) => void,
    private readonly onFailed: (error: unknown) => void,
    private readonly onIdle?: (confirmed: T) => void,
  ) {
    this.desired = confirmed;
  }

  resetConfirmed(value: T) {
    this.confirmed = value;
    this.desired = value;
    this.forcePending = false;
  }

  confirmedValue() {
    return this.confirmed;
  }

  request(value: T, force = false): Promise<T> {
    return new Promise((resolve) => {
      this.idleWaiters.push(resolve);
      this.desired = value;
      this.forcePending = force;
      if (!this.inFlight && this.desired === this.confirmed && !this.forcePending) {
        this.finishIdle();
        return;
      }
      void this.drain();
    });
  }

  private finishIdle() {
    this.onIdle?.(this.confirmed);
    const waiters = this.idleWaiters.splice(0);
    waiters.forEach((resolve) => resolve(this.confirmed));
  }

  private async drain() {
    if (this.inFlight || (this.desired === this.confirmed && !this.forcePending)) return;

    const target = this.desired;
    this.forcePending = false;
    this.inFlight = true;
    try {
      await this.perform(target);
      this.confirmed = target;
      if (this.desired === target) this.onCommitted(target);
    } catch (error) {
      if (this.desired === target) {
        this.desired = this.confirmed;
        this.onCommitted(this.confirmed);
      }
      this.onFailed(error);
    } finally {
      this.inFlight = false;
      if (this.desired !== this.confirmed || this.forcePending) {
        void this.drain();
      } else {
        this.finishIdle();
      }
    }
  }
}

export class LatestWinsImmediate<T> {
  private generation = 0;
  private latestPending = false;
  private idleWaiters: Array<(confirmed: T) => void> = [];

  constructor(
    private confirmed: T,
    private readonly perform: (value: T) => Promise<void>,
    private readonly onCommitted: (value: T) => void,
    private readonly onFailed: (error: unknown) => void,
    private readonly onIdle?: (confirmed: T) => void,
    private readonly recoverConfirmed?: () => Promise<T>,
  ) {}

  resetConfirmed(value: T) {
    this.generation += 1;
    this.confirmed = value;
    this.latestPending = false;
    this.finishLatest();
  }

  confirmedValue() {
    return this.confirmed;
  }

  request(value: T, force = false): Promise<T> {
    return new Promise((resolve) => {
      this.idleWaiters.push(resolve);
      if (!this.latestPending && value === this.confirmed && !force) {
        this.finishLatest();
        return;
      }

      this.generation += 1;
      const requestGeneration = this.generation;
      this.latestPending = true;
      void this.performLatest(requestGeneration, value);
    });
  }

  private finishLatest() {
    this.onIdle?.(this.confirmed);
    const waiters = this.idleWaiters.splice(0);
    waiters.forEach((resolve) => resolve(this.confirmed));
  }

  private async performLatest(requestGeneration: number, target: T) {
    try {
      await this.perform(target);
      if (requestGeneration !== this.generation) return;
      this.confirmed = target;
      this.onCommitted(target);
    } catch (error) {
      if (requestGeneration !== this.generation) return;
      this.onFailed(error);
      if (this.recoverConfirmed) {
        try {
          const recovered = await this.recoverConfirmed();
          if (requestGeneration !== this.generation) return;
          this.confirmed = recovered;
        } catch (recoveryError) {
          if (requestGeneration !== this.generation) return;
          this.onFailed(recoveryError);
        }
      }
      if (requestGeneration !== this.generation) return;
      this.onCommitted(this.confirmed);
    } finally {
      if (requestGeneration === this.generation) {
        this.latestPending = false;
        this.finishLatest();
      }
    }
  }
}

export class ConfirmedDesiredSingleFlight {
  private desired: number;
  private inFlight = false;

  constructor(
    private confirmed: number,
    private readonly perform: (value: number) => Promise<void>,
    private readonly onCommitted: (value: number) => void,
    private readonly onFailed: (error: unknown) => void,
  ) {
    this.desired = confirmed;
  }

  resetConfirmed(value: number) {
    this.confirmed = value;
    this.desired = value;
  }

  confirmedValue() {
    return this.confirmed;
  }

  request(value: number) {
    this.desired = value;
    void this.drain();
  }

  private async drain() {
    if (this.inFlight || this.desired === this.confirmed) return;

    const target = this.desired;
    this.inFlight = true;
    try {
      await this.perform(target);
      this.confirmed = target;
      if (this.desired === target) this.onCommitted(target);
    } catch (error) {
      if (this.desired === target) {
        this.desired = this.confirmed;
        this.onCommitted(this.confirmed);
      }
      this.onFailed(error);
    } finally {
      this.inFlight = false;
      if (this.desired !== this.confirmed) void this.drain();
    }
  }
}

export default function IslandShell() {
  const { language, t } = useI18n();
  const [mode, setMode] = useState<IslandMode>("collapsed");
  const [page, setPage] = useState<IslandPage>("home");
  const [scale, setScale] = useState(1);
  const [glassTransparency, setGlassTransparency] = useState(loadGlassTransparency);
  const [expansionMotion, setExpansionMotion] = useState<IslandExpansionMotion>(loadExpansionMotion);
  const expansionMotionRef = useRef(expansionMotion);
  const [compactWindowEnabled, setCompactWindowEnabled] = useState(() => loadBooleanPreference(COMPACT_WINDOW_KEY, true));
  const [notificationPopupEnabled, setNotificationPopupEnabled] = useState(() => loadBooleanPreference(NOTIFICATION_POPUP_KEY, true));
  const [activeNotification, setActiveNotification] = useState<IslandNotification | null>(null);
  const [expandedHeight, setExpandedHeight] = useState(306);
  const [tucked, setTucked] = useState(false);
  const [initialized, setInitialized] = useState(false);
  const [pending, setPending] = useState(false);
  const [completedSettingsSequence, setCompletedSettingsSequence] = useState<number | null>(null);
  const [settingsRouteEntry, setSettingsRouteEntry] = useState<SettingsRouteEntry | null>(null);
  const [agentsSnapshot, setAgentsSnapshot] = useState<AgentsSnapshot>({ agents: [], generatedAt: 0 });
  const [agentProfilesSnapshot, setAgentProfilesSnapshot] = useState<AgentProfilesSnapshot>({ profiles: [], generatedAt: 0 });
  const [agentProfileFocusId, setAgentProfileFocusId] = useState<string | null>(null);
  const [selectedAgentId, setSelectedAgentId] = useState<AgentId | null>(null);
  const [selectedAgentContext, setSelectedAgentContext] = useState<AgentReminderContext | null>(null);
  const [pendingAgentRoute, setPendingAgentRoute] = useState<PendingAgentRoute | null>(null);
  const [committedAgentContext, setCommittedAgentContext] = useState<CommittedAgentContext | null>(null);
  const mountedRef = useRef(false);
  const modeRef = useRef<IslandMode>("collapsed");
  const initializedRef = useRef(false);
  const lifecycleRef = useRef(0);
  const settingsQueuedSequenceRef = useRef<number | null>(null);
  const pendingReminderNavigationRef = useRef<PendingReminderNavigation | null>(null);
  const reminderNavigationInFlightRef = useRef(false);
  const reminderNavigationQueryInFlightRef = useRef(false);
  const reminderNavigationReplayDirtyRef = useRef(false);
  const reminderNavigationRequestedSequenceRef = useRef(0);
  const settingsInFlightRef = useRef(false);
  const nextSettingsRouteEntryRef = useRef(0);
  const untuckInFlightRef = useRef(false);
  const scaleCoordinatorRef = useRef<LatestWinsSingleFlight<number> | null>(null);
  const heightCoordinatorRef = useRef<ConfirmedDesiredSingleFlight | null>(null);
  const modeCoordinatorRef = useRef<LatestWinsImmediate<IslandMode> | null>(null);
  const heightDragCleanupRef = useRef<(() => void) | null>(null);
  const compactWindowEnabledRef = useRef(compactWindowEnabled);
  const notificationPopupEnabledRef = useRef(notificationPopupEnabled);
  const activeNotificationRef = useRef<IslandNotification | null>(activeNotification);
  const isHoveredRef = useRef(false);
  const pinnedExpandedRef = useRef(false);
  const hoverExpandTimerRef = useRef<number | undefined>(undefined);
  const hoverCollapseTimerRef = useRef<number | undefined>(undefined);
  const notificationTimerRef = useRef<number | undefined>(undefined);
  modeRef.current = mode;
  compactWindowEnabledRef.current = compactWindowEnabled;
  notificationPopupEnabledRef.current = notificationPopupEnabled;
  activeNotificationRef.current = activeNotification;
  expansionMotionRef.current = expansionMotion;

  if (modeCoordinatorRef.current === null) {
    modeCoordinatorRef.current = new LatestWinsImmediate<IslandMode>(
      "collapsed",
      async (value) => {
        if (mountedRef.current) setPending(true);
        await invoke("set_island_mode", { mode: value, motion: expansionMotionRef.current });
      },
      (value) => {
        modeRef.current = value;
        if (mountedRef.current) {
          setMode(value);
        }
      },
      (error) => console.error("Failed to set island mode", error),
      () => {
        if (mountedRef.current) setPending(false);
      },
      async () => (await invoke<InitialState>("get_initial_state")).mode,
    );
  }

  if (scaleCoordinatorRef.current === null) {
    scaleCoordinatorRef.current = new LatestWinsSingleFlight(
      1,
      async (value) => {
        await invoke("set_island_scale", { scale: value });
      },
      (value) => {
        if (!mountedRef.current) return;
        try {
          localStorage.setItem(SCALE_KEY, String(value));
        } catch (error) {
          console.error("Failed to persist island scale", error);
        }
        setScale(value);
      },
      (error) => console.error("Failed to set island scale", error),
    );
  }

  if (heightCoordinatorRef.current === null) {
    heightCoordinatorRef.current = new ConfirmedDesiredSingleFlight(
      306,
      async (value) => {
        await invoke("set_island_expanded_height", { height: value });
      },
      (value) => {
        if (mountedRef.current) setExpandedHeight(value);
      },
      (error) => console.error("Failed to set island expanded height", error),
    );
  }

  const requestMode = useCallback((nextMode: IslandMode, force = false): Promise<IslandMode> => {
    const coordinator = modeCoordinatorRef.current;
    if (!initializedRef.current || coordinator === null) return Promise.resolve(modeRef.current);
    modeRef.current = nextMode;
    setMode(nextMode);
    setPending(true);
    return coordinator.request(nextMode, force);
  }, []);

  const recordSettingsRouteEntry = useCallback((source: SettingsRouteEntrySource) => {
    nextSettingsRouteEntryRef.current += 1;
    setSettingsRouteEntry({ token: nextSettingsRouteEntryRef.current, source });
  }, []);

  const drainSettingsNavigation = useCallback(async function drainSettingsNavigationTask() {
    if (
      !mountedRef.current ||
      !initializedRef.current ||
      settingsInFlightRef.current ||
      settingsQueuedSequenceRef.current === null
    ) {
      return;
    }

    const lifecycle = lifecycleRef.current;
    settingsInFlightRef.current = true;
    let continueDraining = true;
    try {
      while (
        mountedRef.current &&
        initializedRef.current &&
        settingsQueuedSequenceRef.current !== null
      ) {
        const sequence = settingsQueuedSequenceRef.current;
        settingsQueuedSequenceRef.current = null;
        try {
          await invoke("set_island_tucked", { tucked: false });
          if (!mountedRef.current || lifecycleRef.current !== lifecycle) {
            settingsQueuedSequenceRef.current = Math.max(
              settingsQueuedSequenceRef.current ?? 0,
              sequence,
            );
            continueDraining = false;
            return;
          }
          setTucked(false);
          const confirmedMode = await requestMode("expanded", true);
          if (confirmedMode !== "expanded") throw new Error("Island expansion was not confirmed");
          if (!mountedRef.current || lifecycleRef.current !== lifecycle) {
            settingsQueuedSequenceRef.current = Math.max(
              settingsQueuedSequenceRef.current ?? 0,
              sequence,
            );
            continueDraining = false;
            return;
          }
          setAgentProfileFocusId(null);
          setPage("settings");
          setCompletedSettingsSequence((current) =>
            current === null ? sequence : Math.max(current, sequence),
          );
          recordSettingsRouteEntry("tray");
        } catch (error) {
          if (mountedRef.current && lifecycleRef.current === lifecycle) {
            settingsQueuedSequenceRef.current = Math.max(
              settingsQueuedSequenceRef.current ?? 0,
              sequence,
            );
            console.error("Failed to navigate to settings from tray", error);
          }
          continueDraining = false;
          return;
        }
      }
    } finally {
      settingsInFlightRef.current = false;
      if (
        continueDraining &&
        mountedRef.current &&
        initializedRef.current &&
        settingsQueuedSequenceRef.current !== null
      ) {
        void drainSettingsNavigationTask();
      }
    }
  }, [recordSettingsRouteEntry, requestMode]);

  const queueSettingsNavigation = useCallback((sequence: number) => {
    if (!mountedRef.current) return;
    settingsQueuedSequenceRef.current = Math.max(
      settingsQueuedSequenceRef.current ?? 0,
      sequence,
    );
    void drainSettingsNavigation();
  }, [drainSettingsNavigation]);

  const replayPendingSettingsNavigation = useCallback(
    async (lifecycle: number): Promise<boolean> => {
      let pendingNavigation: PendingTrayNavigation | null;
      try {
        pendingNavigation = await invoke<PendingTrayNavigation | null>(
          "get_pending_tray_navigation",
        );
      } catch (error) {
        if (mountedRef.current && lifecycleRef.current === lifecycle) {
          console.error("Failed to query pending tray navigation", error);
        }
        return false;
      }

      if (
        pendingNavigation?.page !== "settings" ||
        !mountedRef.current ||
        lifecycleRef.current !== lifecycle
      ) {
        return true;
      }

      queueSettingsNavigation(pendingNavigation.sequence);
      return true;
    },
    [queueSettingsNavigation],
  );

  const drainReminderNavigation = useCallback(async () => {
    if (!mountedRef.current || !initializedRef.current || reminderNavigationInFlightRef.current) return;
    const pendingNavigation = pendingReminderNavigationRef.current;
    if (pendingNavigation?.sourceKind !== "agent") return;
    const encoded = pendingNavigation.sourceEntityId;
    const prefix = "agent:";
    if (!encoded.startsWith(prefix)) return;
    const ruleEnd = encoded.indexOf(":", prefix.length);
    const agentEnd = ruleEnd < 0 ? -1 : encoded.indexOf(":", ruleEnd + 1);
    const environmentEnd = agentEnd < 0 ? -1 : encoded.indexOf(":", agentEnd + 1);
    const statusStart = encoded.lastIndexOf(":");
    if (ruleEnd < 0 || agentEnd < 0 || environmentEnd < 0 || statusStart <= environmentEnd) return;
    const agentId = encoded.slice(ruleEnd + 1, agentEnd);
    const environment = encoded.slice(agentEnd + 1, environmentEnd);
    const taskId = encoded.slice(environmentEnd + 1, statusStart);
    const triggerStatus = encoded.slice(statusStart + 1);
    if (agentId !== "codex" && agentId !== "hermes" && agentId !== "workbuddy" && agentId !== "claude") return;
    if ((environment !== "windows" && environment !== "wsl") || !taskId || !["completed", "failed", "waiting", "timeout"].includes(triggerStatus)) return;
    reminderNavigationInFlightRef.current = true;
    try {
      setCommittedAgentContext(null);
      setSelectedAgentId(agentId);
      const context: AgentReminderContext = {
        environment: environment as AgentEnvironment,
        taskId,
        triggerStatus: triggerStatus as AgentTriggerStatus,
      };
      setSelectedAgentContext(context);
      setPage("home");
      if (modeRef.current === "collapsed") {
        const confirmedMode = await requestMode("expanded");
        if (confirmedMode !== "expanded") throw new Error("Island expansion was not confirmed");
      }
      setPendingAgentRoute({ sequence: pendingNavigation.sequence, agentId, ...context });
    } catch (error) {
      console.error("Failed to open Agent reminder context", error);
    } finally {
      reminderNavigationInFlightRef.current = false;
      if (pendingReminderNavigationRef.current?.sequence !== pendingNavigation.sequence) {
        void drainReminderNavigation();
      }
    }
  }, [requestMode]);

  useEffect(() => {
    if (
      pendingAgentRoute === null ||
      committedAgentContext === null ||
      page !== "home" ||
      selectedAgentId !== pendingAgentRoute.agentId ||
      committedAgentContext.sequence !== pendingAgentRoute.sequence ||
      committedAgentContext.agentId !== pendingAgentRoute.agentId ||
      committedAgentContext.environment !== pendingAgentRoute.environment ||
      committedAgentContext.taskId !== pendingAgentRoute.taskId ||
      committedAgentContext.triggerStatus !== pendingAgentRoute.triggerStatus ||
      pendingReminderNavigationRef.current?.sequence !== pendingAgentRoute.sequence
    ) return;
    void acknowledgeReminderNavigation({ sequence: pendingAgentRoute.sequence }).then(() => {
      if (pendingReminderNavigationRef.current?.sequence === pendingAgentRoute.sequence) pendingReminderNavigationRef.current = null;
      setPendingAgentRoute((current) => current?.sequence === pendingAgentRoute.sequence ? null : current);
      setCommittedAgentContext((current) => current?.sequence === pendingAgentRoute.sequence ? null : current);
    }).catch((error) => console.error("Failed to acknowledge Agent reminder context", error));
  }, [committedAgentContext, page, pendingAgentRoute, selectedAgentId]);

  const replayPendingReminderNavigation = useCallback(async (requestedSequence?: number) => {
    if (requestedSequence !== undefined) {
      reminderNavigationRequestedSequenceRef.current = Math.max(
        reminderNavigationRequestedSequenceRef.current,
        requestedSequence,
      );
    }
    if (reminderNavigationQueryInFlightRef.current) {
      reminderNavigationReplayDirtyRef.current = true;
      return;
    }
    reminderNavigationQueryInFlightRef.current = true;
    try {
      do {
        reminderNavigationReplayDirtyRef.current = false;
        const pendingNavigation = await getPendingReminderNavigation();
        if (!mountedRef.current || pendingNavigation === null) continue;
        const current = pendingReminderNavigationRef.current;
        if (current === null || pendingNavigation.sequence >= current.sequence) {
          pendingReminderNavigationRef.current = pendingNavigation;
          void drainReminderNavigation();
        }
        if (reminderNavigationRequestedSequenceRef.current > pendingNavigation.sequence) {
          reminderNavigationReplayDirtyRef.current = true;
        }
      } while (reminderNavigationReplayDirtyRef.current && mountedRef.current);
    } catch (error) {
      console.error("Failed to query reminder navigation", error);
    } finally {
      reminderNavigationQueryInFlightRef.current = false;
      if (reminderNavigationReplayDirtyRef.current && mountedRef.current) {
        void replayPendingReminderNavigation();
      }
    }
  }, [drainReminderNavigation]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    let retryTimer: number | undefined;
    const lifecycle = lifecycleRef.current + 1;
    lifecycleRef.current = lifecycle;
    mountedRef.current = true;
    initializedRef.current = false;

    const initialize = async () => {
      try {
        const initial = await invoke<InitialState>("get_initial_state");
        if (!active) return;

        if (initial.rasterizationError) {
          console.warn("AIsland rasterization fallback", initial.rasterizationError);
        }

        setMode(initial.mode);
        modeRef.current = initial.mode;
        modeCoordinatorRef.current?.resetConfirmed(initial.mode);
        setScale(initial.scale);
        setExpandedHeight(initial.expandedHeight);
        scaleCoordinatorRef.current?.resetConfirmed(initial.scale);
        heightCoordinatorRef.current?.resetConfirmed(initial.expandedHeight);
        setTucked(initial.tucked);

        const saved = Number(localStorage.getItem(SCALE_KEY) ?? initial.scale);
        const nextScale = clampScale(saved);
        if (active) scaleCoordinatorRef.current?.request(nextScale);
      } catch (error) {
        if (active) console.error("Failed to initialize island window", error);
      } finally {
        if (active) {
          initializedRef.current = true;
          setInitialized(true);
          void drainSettingsNavigation();
          void drainReminderNavigation();
        }
      }
    };

    const scheduleRetry = (callback: () => void, attempt: number) => {
      if (
        !active ||
        lifecycleRef.current !== lifecycle ||
        retryTimer !== undefined
      ) {
        return;
      }

      const delay = Math.min(5_000, 250 * 2 ** Math.min(attempt, 5));
      retryTimer = window.setTimeout(() => {
        retryTimer = undefined;
        callback();
      }, delay);
    };

    const retryPendingReplay = async (attempt = 0) => {
      const replayed = await replayPendingSettingsNavigation(lifecycle);
      if (!replayed && active && lifecycleRef.current === lifecycle) {
        scheduleRetry(() => {
          void retryPendingReplay(attempt + 1);
        }, attempt);
      }
    };

    const registerListener = async (attempt = 0) => {
      try {
        const stopListening = await listen<string>("tray-navigate", (event) => {
          if (event.payload === "settings") {
            void retryPendingReplay();
          }
        });
        if (!active) {
          stopListening();
          return;
        }
        unlisten = stopListening;
        await retryPendingReplay();
      } catch (error) {
        if (!active || lifecycleRef.current !== lifecycle) return;
        console.error("Failed to listen for tray navigation", error);
        await replayPendingSettingsNavigation(lifecycle);
        if (!active || lifecycleRef.current !== lifecycle) return;

        scheduleRetry(() => {
          void registerListener(attempt + 1);
        }, attempt);
      }
    };

    void initialize();
    void registerListener();
    return () => {
      active = false;
      mountedRef.current = false;
      initializedRef.current = false;
      if (lifecycleRef.current === lifecycle) lifecycleRef.current += 1;
      if (retryTimer !== undefined) window.clearTimeout(retryTimer);
      unlisten?.();
    };
  }, [drainReminderNavigation, drainSettingsNavigation, queueSettingsNavigation, replayPendingSettingsNavigation]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    void listen<{ sequence: number }>("reminderNavigationRequested", (event) => {
      void replayPendingReminderNavigation(event.payload.sequence);
    }).then((stop) => {
      if (!active) { stop(); return; }
      unlisten = stop;
      void replayPendingReminderNavigation();
    }).catch((error) => console.error("Failed to listen for reminder navigation", error));
    return () => { active = false; unlisten?.(); };
  }, [replayPendingReminderNavigation]);

  useEffect(() => {
    let active = true;
    const handle = beginAgentStateSubscription(
      (error) => console.error("Failed to listen for agent state", error),
      (snapshot) => {
        if (active) setAgentsSnapshot(snapshot);
      },
    );

    void handle.ready
      .then((subscription) => {
        if (active) setAgentsSnapshot(subscription.initial);
      })
      .catch((error) => {
        if (active) console.error("Failed to load agent state", error);
      });

    return () => {
      active = false;
      handle.dispose();
    };
  }, []);

  useEffect(() => {
    let active = true;
    const handle = beginAgentProfileStateSubscription(
      (error) => console.error("Failed to listen for Agent Profile state", error),
      (snapshot) => {
        if (active) setAgentProfilesSnapshot(snapshot);
      },
    );

    void handle.ready
      .then((subscription) => {
        if (active) setAgentProfilesSnapshot(subscription.initial);
      })
      .catch((error) => {
        if (active) console.error("Failed to load Agent Profile state", error);
      });

    return () => {
      active = false;
      handle.dispose();
    };
  }, []);

  const clearHoverTimers = useCallback(() => {
    if (hoverExpandTimerRef.current !== undefined) window.clearTimeout(hoverExpandTimerRef.current);
    if (hoverCollapseTimerRef.current !== undefined) window.clearTimeout(hoverCollapseTimerRef.current);
    hoverExpandTimerRef.current = undefined;
    hoverCollapseTimerRef.current = undefined;
  }, []);

  const scheduleCompactCollapse = useCallback(() => {
    if (
      !compactWindowEnabledRef.current
      || isHoveredRef.current
      || pinnedExpandedRef.current
      || activeNotificationRef.current !== null
    ) return;
    if (hoverCollapseTimerRef.current !== undefined) window.clearTimeout(hoverCollapseTimerRef.current);
    hoverCollapseTimerRef.current = window.setTimeout(() => {
      hoverCollapseTimerRef.current = undefined;
      if (
        compactWindowEnabledRef.current
        && !isHoveredRef.current
        && !pinnedExpandedRef.current
        && activeNotificationRef.current === null
      ) requestMode("collapsed");
    }, COMPACT_COLLAPSE_DELAY_MS);
  }, [requestMode]);

  const showNotification = useCallback((notification: IslandNotification, receivedAt: number) => {
    if (
      !compactWindowEnabledRef.current
      || !notificationPopupEnabledRef.current
      || Date.now() - receivedAt > 60_000
    ) return;
    if (notificationTimerRef.current !== undefined) window.clearTimeout(notificationTimerRef.current);
    activeNotificationRef.current = notification;
    setActiveNotification(notification);
    requestMode("expanded");
    notificationTimerRef.current = window.setTimeout(() => {
      notificationTimerRef.current = undefined;
      activeNotificationRef.current = null;
      setActiveNotification(null);
      scheduleCompactCollapse();
    }, NOTIFICATION_VISIBLE_MS);
  }, [requestMode, scheduleCompactCollapse]);

  useEffect(() => {
    const handle = beginReminderDispatchSubscription({
      consumerId: "main-alerts",
      onListenerFailure: (error) => console.error("Failed to listen for island notifications", error),
      render: (deliveries) => {
        const latest = deliveries.filter((delivery) => delivery.sourceKind === "agent").at(-1);
        if (!latest) return;
        const dispatchedAt = latest.lastDispatchedAt ?? latest.firstDispatchedAt ?? latest.dueAt;
        showNotification({ kind: "reminder", delivery: latest }, dispatchedAt);
      },
    });
    void handle.ready.catch((error) => console.error("Failed to initialize island notifications", error));
    return () => handle.dispose();
  }, [showNotification]);

  useEffect(() => {
    let active = true;
    let reloadDirty = false;
    let reloadInFlight: Promise<void> | null = null;
    let reloadGeneration = 0;

    const reloadLatestWindowsNotification = () => {
      if (!active || reloadInFlight !== null) return;
      const requestedGeneration = reloadGeneration;
      const flight = (async () => {
        try {
          const [latest] = await listNotificationHistory({
            origin: "windows",
            sourceApp: null,
            unreadOnly: false,
            limit: 1,
          });
          if (!active || reloadDirty || reloadGeneration !== requestedGeneration || latest?.origin !== "windows") return;
          showNotification({ kind: "system", item: latest }, latest.receivedAt);
        } catch (error) {
          if (active) console.error("Failed to load the latest Windows notification", error);
        }
      })();
      reloadInFlight = flight;
      void flight.then(() => {
        if (reloadInFlight !== flight) return;
        reloadInFlight = null;
        if (!active || !reloadDirty) return;
        reloadDirty = false;
        reloadLatestWindowsNotification();
      });
    };

    const pendingListener = listen<NotificationHistoryChangedPayload>("notificationHistoryChanged", (event) => {
      if (
        event.payload.origin !== "windows"
        || !compactWindowEnabledRef.current
        || !notificationPopupEnabledRef.current
      ) return;
      reloadGeneration += 1;
      if (reloadInFlight !== null) {
        reloadDirty = true;
        return;
      }
      reloadLatestWindowsNotification();
    });

    return () => {
      active = false;
      void pendingListener.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, [showNotification]);

  useEffect(() => {
    const onBlur = () => {
      if (!compactWindowEnabledRef.current || !pinnedExpandedRef.current) return;
      pinnedExpandedRef.current = false;
      if (activeNotificationRef.current === null) requestMode("collapsed");
    };
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, [requestMode]);

  useEffect(() => {
    if (!initialized) return;
    if (!compactWindowEnabled) {
      clearHoverTimers();
      pinnedExpandedRef.current = false;
      requestMode("expanded");
    }
  }, [clearHoverTimers, compactWindowEnabled, initialized, requestMode]);

  useEffect(() => () => {
    clearHoverTimers();
    if (notificationTimerRef.current !== undefined) window.clearTimeout(notificationTimerRef.current);
  }, [clearHoverTimers]);

  useEffect(() => {
    const pendingTuckedListener = listen<boolean>("island-tucked-changed", (event) => {
      setTucked(event.payload);
    });

    return () => {
      void pendingTuckedListener.then((unlisten) => unlisten());
    };
  }, []);

  useEffect(() => () => {
    heightDragCleanupRef.current?.();
  }, []);

  const untuck = useCallback(async () => {
    if (untuckInFlightRef.current) return;

    untuckInFlightRef.current = true;
    try {
      await invoke("set_island_tucked", { tucked: false });
      if (mountedRef.current) setTucked(false);
    } catch (error) {
      console.error("Failed to restore tucked island", error);
    } finally {
      untuckInFlightRef.current = false;
    }
  }, []);

  const applyScale = useCallback((value: number) => {
    scaleCoordinatorRef.current?.request(clampScale(value));
  }, []);

  const applyGlassTransparency = useCallback((value: number) => {
    const nextValue = clampGlassTransparency(value);
    setGlassTransparency(nextValue);
    try {
      localStorage.setItem(GLASS_TRANSPARENCY_KEY, String(nextValue));
    } catch (error) {
      console.error("Failed to persist glass transparency", error);
    }
  }, []);

  useEffect(() => {
    if (!initialized) return;
    void invoke("set_island_glass_transparency", { transparency: glassTransparency }).catch((error) => {
      console.error("Failed to set native glass transparency", error);
    });
  }, [glassTransparency, initialized]);

  const applyExpansionMotion = useCallback((motion: IslandExpansionMotion) => {
    setExpansionMotion(motion);
    try {
      localStorage.setItem(EXPANSION_MOTION_KEY, motion);
    } catch (error) {
      console.error("Failed to persist expansion motion", error);
    }
  }, []);

  const applyCompactWindowEnabled = useCallback((enabled: boolean) => {
    compactWindowEnabledRef.current = enabled;
    setCompactWindowEnabled(enabled);
    try {
      localStorage.setItem(COMPACT_WINDOW_KEY, String(enabled));
    } catch (error) {
      console.error("Failed to persist compact-window preference", error);
    }
    if (!enabled) {
      clearHoverTimers();
      pinnedExpandedRef.current = false;
      requestMode("expanded");
    } else {
      scheduleCompactCollapse();
    }
  }, [clearHoverTimers, requestMode, scheduleCompactCollapse]);

  const applyNotificationPopupEnabled = useCallback((enabled: boolean) => {
    notificationPopupEnabledRef.current = enabled;
    setNotificationPopupEnabled(enabled);
    try {
      localStorage.setItem(NOTIFICATION_POPUP_KEY, String(enabled));
    } catch (error) {
      console.error("Failed to persist notification-popup preference", error);
    }
    if (!enabled) {
      if (notificationTimerRef.current !== undefined) window.clearTimeout(notificationTimerRef.current);
      notificationTimerRef.current = undefined;
      activeNotificationRef.current = null;
      setActiveNotification(null);
      scheduleCompactCollapse();
    }
  }, [scheduleCompactCollapse]);

  const handlePointerEnter = useCallback(() => {
    isHoveredRef.current = true;
    if (hoverCollapseTimerRef.current !== undefined) window.clearTimeout(hoverCollapseTimerRef.current);
    hoverCollapseTimerRef.current = undefined;
    if (!compactWindowEnabledRef.current || modeRef.current !== "collapsed") return;
    if (hoverExpandTimerRef.current !== undefined) window.clearTimeout(hoverExpandTimerRef.current);
    hoverExpandTimerRef.current = window.setTimeout(() => {
      hoverExpandTimerRef.current = undefined;
      if (isHoveredRef.current && compactWindowEnabledRef.current) requestMode("expanded");
    }, COMPACT_EXPAND_DELAY_MS);
  }, [requestMode]);

  const handlePointerLeave = useCallback(() => {
    isHoveredRef.current = false;
    if (hoverExpandTimerRef.current !== undefined) window.clearTimeout(hoverExpandTimerRef.current);
    hoverExpandTimerRef.current = undefined;
    scheduleCompactCollapse();
  }, [scheduleCompactCollapse]);

  const pinExpanded = useCallback(() => {
    if (!compactWindowEnabledRef.current) return;
    clearHoverTimers();
    pinnedExpandedRef.current = true;
    requestMode("expanded");
  }, [clearHoverTimers, requestMode]);

  const dismissNotification = useCallback(() => {
    if (notificationTimerRef.current !== undefined) window.clearTimeout(notificationTimerRef.current);
    notificationTimerRef.current = undefined;
    activeNotificationRef.current = null;
    setActiveNotification(null);
    scheduleCompactCollapse();
  }, [scheduleCompactCollapse]);

  const selectPage = useCallback((nextPage: IslandPage) => {
    setAgentProfileFocusId(null);
    setPage(nextPage);
    if (nextPage === "settings") {
      recordSettingsRouteEntry("tab");
    }
  }, [recordSettingsRouteEntry]);

  const acknowledgeSettingsAtRoot = useCallback(async (sequence: number) => {
    const lifecycle = lifecycleRef.current;

    try {
      await invoke("acknowledge_tray_navigation", { sequence });
      if (mountedRef.current && lifecycleRef.current === lifecycle) {
        setCompletedSettingsSequence((current) =>
          current === sequence ? null : current,
        );
      }
    } catch (error) {
      if (mountedRef.current && lifecycleRef.current === lifecycle) {
        console.error("Failed to acknowledge tray navigation", error);
      }
    }
  }, []);

  const beginHeightDrag = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || mode !== "expanded" || !initialized) return;

    event.stopPropagation();
    heightDragCleanupRef.current?.();
    const handle = event.currentTarget;
    const pointerId = event.pointerId;
    const startY = event.clientY;
    const startHeight = heightCoordinatorRef.current?.confirmedValue() ?? expandedHeight;
    handle.setPointerCapture(pointerId);

    const move = (nextEvent: PointerEvent) => {
      const nextHeight = Math.min(
        MAX_EXPANDED_HEIGHT,
        Math.max(
          MIN_EXPANDED_HEIGHT,
          startHeight + (nextEvent.clientY - startY) / scale,
        ),
      );
      heightCoordinatorRef.current?.request(nextHeight);
    };

    const stop = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", stop);
      handle.removeEventListener("pointercancel", stop);
      if (handle.hasPointerCapture(pointerId)) handle.releasePointerCapture(pointerId);
      if (heightDragCleanupRef.current === stop) heightDragCleanupRef.current = null;
    };

    heightDragCleanupRef.current = stop;
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", stop);
    handle.addEventListener("pointercancel", stop);
  }, [expandedHeight, initialized, mode, scale]);

  const toggleMode = useCallback(() => {
    if (!initialized || pending) return;
    if (mode === "expanded") pinnedExpandedRef.current = false;
    requestMode(mode === "collapsed" ? "expanded" : "collapsed");
  }, [initialized, mode, pending, requestMode]);

  const openAgent = useCallback(async (agentId: AgentId) => {
    setAgentProfileFocusId(null);
    setSelectedAgentId(agentId);
    setPage("home");
    if (mode !== "collapsed" || !initialized || pending) return;
    requestMode("expanded");
  }, [initialized, mode, pending, requestMode]);

  const openAgentProfile = useCallback((profileId: string) => {
    setAgentProfileFocusId(profileId);
    setSelectedAgentId(null);
    setPage("settings");
    if (mode !== "collapsed" || !initialized || pending) return;
    requestMode("expanded");
  }, [initialized, mode, pending, requestMode]);

  const startDrag = useCallback(async (event: React.PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || !initialized || pending) return;

    try {
      await invoke("start_island_drag");
    } catch (error) {
      console.error("Failed to start island drag", error);
    }
  }, [initialized, pending]);

  const canvasSize = useMemo(
    () => ({
      width: mode === "collapsed" ? 248 : 560,
      height: mode === "collapsed" ? 46 : expandedHeight,
    }),
    [expandedHeight, mode],
  );
  const sortedAgents = useMemo(
    () => sortAgentsByPriority(visibleAgentSummaries(agentsSnapshot.agents)),
    [agentsSnapshot.agents],
  );
  const homeAgents = useMemo(() => {
    if (selectedAgentId === null || selectedAgentContext === null) return sortedAgents;
    if (sortedAgents.some((agent) => agent.agentId === selectedAgentId)) return sortedAgents;
    const selected = agentsSnapshot.agents.find((agent) => agent.agentId === selectedAgentId);
    return selected === undefined ? sortedAgents : [...sortedAgents, selected];
  }, [agentsSnapshot.agents, selectedAgentContext, selectedAgentId, sortedAgents]);
  const prioritizedStatuses = useMemo(
    () => prioritizedAgentStatuses(sortedAgents, agentProfilesSnapshot.profiles),
    [agentProfilesSnapshot.profiles, sortedAgents],
  );
  const activeNotificationText = activeNotification?.kind === "system"
    ? [activeNotification.item.title, activeNotification.item.body].filter(Boolean).join(" · ")
    : activeNotification?.kind === "reminder"
      ? translateRegisteredMessage(language, activeNotification.delivery.messageKey, activeNotification.delivery.messageParameters)
      : "";
  const glassRatio = glassTransparency / 100;
  const glassMaterialRatio = Math.sqrt(1 - glassRatio);
  const glassStyle = {
    width: canvasSize.width,
    height: canvasSize.height,
    transform: `scale(${scale})`,
    "--glass-shell-alpha": String(Number((1 - glassRatio).toFixed(2))),
    "--glass-panel-alpha": String(Number((0.1 * glassMaterialRatio).toFixed(3))),
    "--glass-popover-alpha": String(Number((0.96 * glassMaterialRatio).toFixed(3))),
    "--glass-blur": `${Math.round(glassRatio * 24)}px`,
    "--glass-saturation": `${Math.round(100 + glassRatio * 45)}%`,
  } as CSSProperties;
  const viewportStyle = {
    "--island-window-radius": `${(mode === "collapsed" ? 23 : 24) * scale}px`,
    "--island-compact-width": `${248 * scale}px`,
    "--island-compact-height": `${46 * scale}px`,
  } as CSSProperties;

  return (
    <div className={`island-viewport island-viewport--${mode}`} data-expansion-motion={expansionMotion} style={viewportStyle}>
      <div
        className={`island-canvas island-canvas--${mode}`}
        data-glass-transparency={glassTransparency}
        style={glassStyle}
        onPointerEnter={handlePointerEnter}
        onPointerLeave={handlePointerLeave}
        onDoubleClick={pinExpanded}
      >
        <header className="island-topbar">
          <div className="drag-region" onPointerDown={startDrag}>
            {mode === "collapsed" ? (
              <AgentStatusSlots
                agents={sortedAgents}
                profileSummaries={agentProfilesSnapshot.profiles}
                onOpenAgent={(agentId) => void openAgent(agentId)}
                onOpenProfile={openAgentProfile}
              />
            ) : (
              <>
                <div className="island-brand">
                  <img className="island-brand__mark" src={aislandMark} alt="AIsland" />
                  <span className="island-title" aria-hidden="true">AIsland</span>
                </div>
                <div className="island-mini-status" aria-label={t("aria.agentStatus")}>
                  {prioritizedStatuses.slice(0, 3).map((status, index) => (
                    <StatusDot key={`${status}-${index}`} color={AGENT_STATUS_COLOR[status]} pulse={status === "running"} />
                  ))}
                </div>
              </>
            )}
          </div>
          {mode === "expanded" && (
            <div onPointerDownCapture={(event) => event.stopPropagation()}>
              <TabBar page={page} onSelect={selectPage} />
            </div>
          )}
          {mode === "expanded" && (
            <button
              className="tab-toggle tab-minimize"
              title={t("action.minimizeToTray")}
              aria-label={t("action.minimizeToTray")}
              disabled={pending}
              onPointerDown={(event) => event.stopPropagation()}
              onClick={() => {
                if (pending) return;
                void invoke("hide_island_to_tray").catch((error) => {
                  console.error("Failed to hide AIsland to the system tray", error);
                });
              }}
            >
              <Minus size={16} />
            </button>
          )}
          <button
            className="tab-toggle"
            title={t(mode === "collapsed" ? "action.expand" : "action.collapse")}
            aria-label={t(mode === "collapsed" ? "action.expand" : "action.collapse")}
            disabled={!initialized || pending}
            onPointerDown={(event) => event.stopPropagation()}
            onClick={toggleMode}
          >
            {mode === "collapsed" ? <ChevronDown size={16} /> : <ChevronUp size={16} />}
          </button>
        </header>
        {mode === "expanded" && activeNotification && (
          <div className="island-notification" role="status" aria-live="polite">
            <span className="island-notification__icon" aria-hidden="true"><Bell size={15} /></span>
            <span className="island-notification__message">
              {activeNotificationText}
            </span>
            <button
              type="button"
              className="island-notification__close"
              aria-label={t("action.close")}
              title={t("action.close")}
              onClick={dismissNotification}
            >
              <X size={14} />
            </button>
          </div>
        )}
        {mode === "expanded" && page !== "note" && (
          <main className="island-content" key={page}>
            {page === "settings" ? (
              <SettingsView
                scale={scale}
                onScaleChange={applyScale}
                glassTransparency={glassTransparency}
                onGlassTransparencyChange={applyGlassTransparency}
                expansionMotion={expansionMotion}
                onExpansionMotionChange={applyExpansionMotion}
                compactWindowEnabled={compactWindowEnabled}
                onCompactWindowEnabledChange={applyCompactWindowEnabled}
                notificationPopupEnabled={notificationPopupEnabled}
                onNotificationPopupEnabledChange={applyNotificationPopupEnabled}
                onExitSettings={() => setPage("home")}
                routeResetToken={settingsRouteEntry?.token ?? null}
                entrySequence={completedSettingsSequence}
                onEntryHandled={acknowledgeSettingsAtRoot}
                agentProfileFocusId={agentProfileFocusId}
              />
            ) : page === "home" ? (
              <AgentsPage agents={homeAgents} profileSummaries={agentProfilesSnapshot.profiles} selectedAgentId={selectedAgentId} selectedContext={selectedAgentContext} selectedContextSequence={pendingAgentRoute?.sequence ?? null} onSelectedContextCommitted={setCommittedAgentContext} />
            ) : page === "clipboard" ? (
              <ClipboardPage />
            ) : page === "monitor" ? (
              <MonitorPage />
            ) : page === "notify" ? (
              <NotificationCenterPage />
            ) : (
              <div className="page-preview">{t(PAGE_LABEL_KEYS[page])}</div>
            )}
          </main>
        )}
        <main
          className="island-content"
          hidden={mode !== "expanded" || page !== "note"}
        >
          <DailyNotesPage />
        </main>
        {mode === "expanded" && (
          <div
            className="height-handle"
            role="separator"
            aria-label={t("aria.windowHeight")}
            aria-orientation="horizontal"
            onPointerDown={beginHeightDrag}
          />
        )}
      </div>
      {tucked && (
        <div
          className="tuck-strip"
          aria-label={t("aria.expandIsland")}
          onPointerEnter={() => void untuck()}
        />
      )}
    </div>
  );
}
