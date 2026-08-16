import { listen } from "@tauri-apps/api/event";
import { parseCommandError } from "./commandError";
import {
  acknowledgeReminderNavigation,
  commitReminderReplayCursor,
  getAgentProfilesSnapshot,
  getAgentsSnapshot,
  getPendingReminderNavigation,
  getMediaSnapshot,
  getMonitorSnapshot,
  listClipboardItems,
  listTodos,
  listNotes,
  listServiceHealth,
  listNotificationHistory,
  replayReminderDeliveries,
} from "./commands";
import type {
  AgentStateChangedPayload,
  AgentProfileStateChangedPayload,
  AgentProfilesSnapshot,
  AgentsSnapshot,
  BoundaryListenerState,
  CommandError,
  ClipboardItem,
  PendingReminderNavigation,
  ReminderDelivery,
  ReminderDispatchReadyPayload,
  ReminderNavigationRequestedPayload,
  ListTodosInput,
  ListNotesInput,
  ListClipboardItemsInput,
  MediaSnapshot,
  MonitorMetricsChangedPayload,
  MonitorSnapshot,
  NotificationHistoryChangedPayload,
  NotificationHistoryItem,
  ListNotificationHistoryInput,
  NoteSummary,
  ServiceHealthSnapshot,
  TodoItem,
  EntityId,
  Revision,
  UnixMillis,
} from "./contracts";

export type V1EventName =
  | "serviceHealthChanged"
  | "agentStateChanged"
  | "agentProfileStateChanged"
  | "reminderDispatchReady"
  | "reminderNavigationRequested"
  | "todoChanged"
  | "noteChanged"
  | "clipboardChanged"
  | "mediaSessionChanged"
  | "monitorMetricsChanged"
  | "notificationHistoryChanged"
  | "moduleStateChanged"
  | "onboardingStateChanged";

type ServiceHealthChangedPayload = { serviceId: string; checkedAt: UnixMillis };
export type TodoChangedPayload = { entityId: EntityId; revision: Revision; changedAt: UnixMillis };
export type NoteChangedPayload = { entityId: EntityId; revision: Revision; changedAt: UnixMillis };
export type ClipboardChangedPayload = { entityId: EntityId; changedAt: UnixMillis };
export type MediaSessionChangedPayload = { sessionId: string | null; changedAt: UnixMillis };

async function listenBoundaryEvent<P>(
  name: V1EventName,
  handler: (payload: P) => void,
): Promise<() => void> {
  try {
    const unlisten = await listen<P>(name, (event) => handler(event.payload));
    return () => {
      try {
        const result = (unlisten as unknown as () => unknown)();
        void Promise.resolve(result).catch((error) => parseCommandError(error));
      } catch (error) {
        parseCommandError(error);
      }
    };
  } catch (error) {
    throw parseCommandError(error);
  }
}

export function listenMonitorMetricsChanged(
  handler: (payload: MonitorMetricsChangedPayload) => void,
): Promise<() => void> {
  return listenBoundaryEvent("monitorMetricsChanged", handler);
}

export function listenNotificationHistoryChanged(
  handler: (payload: NotificationHistoryChangedPayload) => void,
): Promise<() => void> {
  return listenBoundaryEvent("notificationHistoryChanged", handler);
}

export async function listenTodoChanged(
  handler: (payload: TodoChangedPayload) => void,
): Promise<() => void> {
  try {
    const unlisten = await listen<TodoChangedPayload>("todoChanged", (event) => handler(event.payload));
    return () => {
      try {
        const result = (unlisten as unknown as () => unknown)();
        void Promise.resolve(result).catch((error) => parseCommandError(error));
      } catch (error) {
        parseCommandError(error);
      }
    };
  } catch (error) {
    throw parseCommandError(error);
  }
}

export async function listenNoteChanged(
  handler: (payload: NoteChangedPayload) => void,
): Promise<() => void> {
  try {
    const unlisten = await listen<NoteChangedPayload>("noteChanged", (event) => handler(event.payload));
    return () => {
      try {
        const result = (unlisten as unknown as () => unknown)();
        void Promise.resolve(result).catch((error) => parseCommandError(error));
      } catch (error) {
        parseCommandError(error);
      }
    };
  } catch (error) {
    throw parseCommandError(error);
  }
}

export async function listenClipboardChanged(
  handler: (payload: ClipboardChangedPayload) => void,
): Promise<() => void> {
  try {
    const unlisten = await listen<ClipboardChangedPayload>("clipboardChanged", (event) => handler(event.payload));
    return () => {
      try {
        const result = (unlisten as unknown as () => unknown)();
        void Promise.resolve(result).catch((error) => parseCommandError(error));
      } catch (error) {
        parseCommandError(error);
      }
    };
  } catch (error) {
    throw parseCommandError(error);
  }
}

export async function listenMediaSessionChanged(
  handler: (payload: MediaSessionChangedPayload) => void,
): Promise<() => void> {
  try {
    const unlisten = await listen<MediaSessionChangedPayload>("mediaSessionChanged", (event) => handler(event.payload));
    return () => {
      try {
        const result = (unlisten as unknown as () => unknown)();
        void Promise.resolve(result).catch((error) => parseCommandError(error));
      } catch (error) {
        parseCommandError(error);
      }
    };
  } catch (error) {
    throw parseCommandError(error);
  }
}

export interface ServiceHealthSubscription {
  initial: ServiceHealthSnapshot[];
  dispose(): void;
}

export interface ServiceHealthSubscriptionHandle {
  ready: Promise<ServiceHealthSubscription>;
  dispose(): void;
}

export async function listenServiceHealthChanged(
  handler: (payload: ServiceHealthChangedPayload) => void,
): Promise<() => void> {
  try {
    const unlisten = await listen<ServiceHealthChangedPayload>("serviceHealthChanged", (event) => {
      handler(event.payload);
    });
    return () => {
      try {
        const result = (unlisten as unknown as () => unknown)();
        void Promise.resolve(result).catch((error) => parseCommandError(error));
      } catch (error) {
        parseCommandError(error);
      }
    };
  } catch (error) {
    throw parseCommandError(error);
  }
}

export function beginServiceHealthSubscription(
  onListenerFailure: (error: CommandError) => void,
  onSnapshot?: (snapshot: ServiceHealthSnapshot[]) => void,
): ServiceHealthSubscriptionHandle {
  let disposed = false;
  let unlisten: (() => void) | undefined;
  let pollingTimer: ReturnType<typeof setInterval> | undefined;
  let latestSnapshot: ServiceHealthSnapshot[] = [];
  let reloadFlight: Promise<void> | undefined;
  let reloadDirty = false;

  const dispose = () => {
    if (disposed) return;
    disposed = true;
    reloadDirty = false;
    if (pollingTimer !== undefined) clearInterval(pollingTimer);
    pollingTimer = undefined;
    const cleanup = unlisten;
    unlisten = undefined;
    cleanup?.();
  };

  const subscription: ServiceHealthSubscription = {
    initial: latestSnapshot,
    dispose,
  };

  const reload = (): Promise<void> => {
    if (disposed) return Promise.resolve();
    if (reloadFlight) {
      reloadDirty = true;
      return reloadFlight;
    }
    const flight = (async () => {
      try {
        do {
          reloadDirty = false;
          const snapshot = await listServiceHealth();
          if (!disposed) {
            latestSnapshot = snapshot;
            subscription.initial = snapshot;
            onSnapshot?.(snapshot);
          }
        } while (reloadDirty && !disposed);
      } finally {
        reloadFlight = undefined;
      }
    })();
    reloadFlight = flight;
    return flight;
  };

  const reloadInBackground = () => {
    void reload().catch((error) => {
      parseCommandError(error);
    });
  };

  const ready = (async () => {
    try {
      const cleanup = await listenServiceHealthChanged(() => {
        reloadInBackground();
      });
      if (disposed) {
        cleanup();
        return subscription;
      }
      unlisten = cleanup;
    } catch (error) {
      if (disposed) return subscription;
      onListenerFailure(parseCommandError(error));
      pollingTimer = setInterval(() => {
        reloadInBackground();
      }, 30_000);
    }

    if (disposed) return subscription;
    try {
      await reload();
      return subscription;
    } catch (error) {
      dispose();
      throw parseCommandError(error);
    }
  })();

  return { ready, dispose };
}

export async function subscribeServiceHealth(
  onListenerFailure: (error: CommandError) => void,
  onSnapshot?: (snapshot: ServiceHealthSnapshot[]) => void,
): Promise<ServiceHealthSubscription> {
  return beginServiceHealthSubscription(onListenerFailure, onSnapshot).ready;
}

export interface CommandSubscription<T> {
  initial: T;
  listenerState: BoundaryListenerState;
  retry(): Promise<void>;
  dispose(): void;
}

export interface CommandSubscriptionHandle<T> {
  ready: Promise<CommandSubscription<T>>;
  dispose(): void;
}

function beginCommandSubscription<T, P>(
  eventName: V1EventName,
  load: (isDisposed: () => boolean) => Promise<T>,
  empty: T,
  onListenerFailure: (error: CommandError) => void,
  onValue?: (value: T) => void,
  pollIntervalMs = 30_000,
  retainInitialLoadFailure = false,
  reportBackgroundFailure = false,
  listenerBootstrapTimeoutMs?: number,
  pollWhileActive = false,
  eventFollowupDelayMs?: number,
): CommandSubscriptionHandle<T> {
  let disposed = false;
  let unlisten: (() => void) | undefined;
  let listenerFlight: Promise<void> | undefined;
  let pollingTimer: ReturnType<typeof setInterval> | undefined;
  let eventFollowupTimer: ReturnType<typeof setTimeout> | undefined;
  let eventFollowupDeadline: number | undefined;
  let flight: Promise<void> | undefined;
  let dirty = false;
  const subscription: CommandSubscription<T> = {
    initial: empty,
    listenerState: "active",
    retry: async () => {
      if (subscription.listenerState === "degraded" && listenerFlight === undefined) {
        await installListener();
      }
      await reload();
    },
    dispose: () => {
      if (disposed) return;
      disposed = true;
      dirty = false;
      if (pollingTimer !== undefined) clearInterval(pollingTimer);
      pollingTimer = undefined;
      if (eventFollowupTimer !== undefined) clearTimeout(eventFollowupTimer);
      eventFollowupTimer = undefined;
      eventFollowupDeadline = undefined;
      const cleanup = unlisten;
      unlisten = undefined;
      cleanup?.();
    },
  };

  const reload = (): Promise<void> => {
    if (disposed) return Promise.resolve();
    if (flight) {
      dirty = true;
      return flight;
    }
    const active = (async () => {
      try {
        do {
          dirty = false;
          const value = await load(() => disposed);
          if (!disposed) {
            subscription.initial = value;
            onValue?.(value);
          }
        } while (dirty && !disposed);
      } finally {
        flight = undefined;
      }
    })();
    flight = active;
    return active;
  };

  const reloadInBackground = () => {
    void reload().catch((error) => {
      const parsed = parseCommandError(error);
      if (reportBackgroundFailure && !disposed) onListenerFailure(parsed);
    });
  };

  const stopPolling = () => {
    if (pollingTimer !== undefined) clearInterval(pollingTimer);
    pollingTimer = undefined;
  };

  const startPolling = () => {
    if (pollingTimer === undefined) pollingTimer = setInterval(reloadInBackground, pollIntervalMs);
  };

  const runEventFollowup = () => {
    eventFollowupTimer = undefined;
    if (disposed) return;
    reloadInBackground();
    const remaining = (eventFollowupDeadline ?? 0) - Date.now();
    if (remaining > 0) {
      eventFollowupTimer = setTimeout(runEventFollowup, remaining);
    } else {
      eventFollowupDeadline = undefined;
    }
  };

  const scheduleEventFollowup = () => {
    if (eventFollowupDelayMs === undefined || disposed) return;
    eventFollowupDeadline = Math.max(
      eventFollowupDeadline ?? Number.NEGATIVE_INFINITY,
      Date.now() + eventFollowupDelayMs,
    );
    if (eventFollowupTimer === undefined) {
      eventFollowupTimer = setTimeout(runEventFollowup, eventFollowupDelayMs);
    }
  };

  const installListener = (): Promise<void> => {
    if (disposed || unlisten !== undefined) return Promise.resolve();
    if (listenerFlight !== undefined) return listenerFlight;
    const active = (async () => {
      const cleanup = await listen<P>(eventName, () => {
        reloadInBackground();
        scheduleEventFollowup();
      });
      if (disposed) {
        cleanup();
        return;
      }
      unlisten = cleanup;
      subscription.listenerState = "active";
      if (pollWhileActive) startPolling();
      else stopPolling();
    })();
    listenerFlight = active;
    void active.then(
      () => { if (listenerFlight === active) listenerFlight = undefined; },
      () => { if (listenerFlight === active) listenerFlight = undefined; },
    );
    return active;
  };

  const ready = (async () => {
    let eagerReload: Promise<void> | undefined;
    try {
      const registration = installListener();
      if (listenerBootstrapTimeoutMs === undefined) {
        await registration;
      } else {
        eagerReload = reload();
        void eagerReload.catch(() => undefined);
        let timeout: ReturnType<typeof setTimeout> | undefined;
        const outcome = await Promise.race([
          registration.then(
            () => ({ kind: "active" as const }),
            (error) => ({ kind: "failed" as const, error }),
          ),
          new Promise<{ kind: "timeout" }>((resolve) => {
            timeout = setTimeout(() => resolve({ kind: "timeout" }), listenerBootstrapTimeoutMs);
          }),
        ]);
        if (timeout !== undefined) clearTimeout(timeout);
        if (outcome.kind === "failed") throw outcome.error;
        if (outcome.kind === "timeout") {
          subscription.listenerState = "degraded";
          startPolling();
          void registration.then(
            () => {
              if (!disposed) reloadInBackground();
            },
            (error) => {
              if (disposed) return;
              subscription.listenerState = "degraded";
              onListenerFailure(parseCommandError(error));
              startPolling();
            },
          );
        }
      }
      if (disposed) return subscription;
    } catch (error) {
      if (disposed) return subscription;
      subscription.listenerState = "degraded";
      onListenerFailure(parseCommandError(error));
      startPolling();
    }
    if (disposed) return subscription;
    try {
      await (eagerReload ?? reload());
      return subscription;
    } catch (error) {
      if (retainInitialLoadFailure && !disposed) {
        onListenerFailure(parseCommandError(error));
        return subscription;
      }
      subscription.dispose();
      throw parseCommandError(error);
    }
  })();

  return { ready, dispose: subscription.dispose };
}

export function beginAgentStateSubscription(
  onListenerFailure: (error: CommandError) => void,
  onSnapshot?: (snapshot: AgentsSnapshot) => void,
): CommandSubscriptionHandle<AgentsSnapshot> {
  return beginCommandSubscription<AgentsSnapshot, AgentStateChangedPayload>(
    "agentStateChanged",
    getAgentsSnapshot,
    { agents: [], generatedAt: 0 },
    onListenerFailure,
    onSnapshot,
    2_000,
    true,
    false,
    500,
    true,
    2_000,
  );
}

export function beginAgentProfileStateSubscription(
  onListenerFailure: (error: CommandError) => void,
  onSnapshot?: (snapshot: AgentProfilesSnapshot) => void,
): CommandSubscriptionHandle<AgentProfilesSnapshot> {
  return beginCommandSubscription<AgentProfilesSnapshot, AgentProfileStateChangedPayload>(
    "agentProfileStateChanged",
    getAgentProfilesSnapshot,
    { profiles: [], generatedAt: 0 },
    onListenerFailure,
    onSnapshot,
    2_000,
    true,
    false,
    500,
    true,
    2_000,
  );
}

export async function subscribeAgentProfileState(
  onListenerFailure: (error: CommandError) => void,
  onSnapshot?: (snapshot: AgentProfilesSnapshot) => void,
): Promise<CommandSubscription<AgentProfilesSnapshot>> {
  return beginAgentProfileStateSubscription(onListenerFailure, onSnapshot).ready;
}

export async function subscribeAgentState(
  onListenerFailure: (error: CommandError) => void,
  onSnapshot?: (snapshot: AgentsSnapshot) => void,
): Promise<CommandSubscription<AgentsSnapshot>> {
  return beginAgentStateSubscription(onListenerFailure, onSnapshot).ready;
}

export function beginTodosSubscription(
  input: ListTodosInput,
  onListenerFailure: (error: CommandError) => void,
  onSnapshot?: (snapshot: TodoItem[]) => void,
): CommandSubscriptionHandle<TodoItem[]> {
  return beginCommandSubscription<TodoItem[], TodoChangedPayload>(
    "todoChanged",
    () => listTodos(input),
    [],
    onListenerFailure,
    onSnapshot,
  );
}

export async function subscribeTodos(
  input: ListTodosInput,
  onListenerFailure: (error: CommandError) => void,
  onSnapshot?: (snapshot: TodoItem[]) => void,
): Promise<CommandSubscription<TodoItem[]>> {
  return beginTodosSubscription(input, onListenerFailure, onSnapshot).ready;
}

export function beginNotesSubscription(
  input: ListNotesInput,
  onListenerFailure: (error: CommandError) => void,
  onSnapshot?: (snapshot: NoteSummary[]) => void,
): CommandSubscriptionHandle<NoteSummary[]> {
  return beginCommandSubscription<NoteSummary[], NoteChangedPayload>(
    "noteChanged",
    () => listNotes(input),
    [],
    onListenerFailure,
    onSnapshot,
  );
}

export async function subscribeNotes(
  input: ListNotesInput,
  onListenerFailure: (error: CommandError) => void,
  onSnapshot?: (snapshot: NoteSummary[]) => void,
): Promise<CommandSubscription<NoteSummary[]>> {
  return beginNotesSubscription(input, onListenerFailure, onSnapshot).ready;
}

export function beginClipboardItemsSubscription(
  input: ListClipboardItemsInput,
  onListenerFailure: (error: CommandError) => void,
  onSnapshot?: (snapshot: ClipboardItem[]) => void,
): CommandSubscriptionHandle<ClipboardItem[]> {
  return beginCommandSubscription<ClipboardItem[], ClipboardChangedPayload>(
    "clipboardChanged",
    () => listClipboardItems(input),
    [],
    onListenerFailure,
    onSnapshot,
  );
}

export async function subscribeClipboardItems(
  input: ListClipboardItemsInput,
  onListenerFailure: (error: CommandError) => void,
  onSnapshot?: (snapshot: ClipboardItem[]) => void,
): Promise<CommandSubscription<ClipboardItem[]>> {
  return beginClipboardItemsSubscription(input, onListenerFailure, onSnapshot).ready;
}

export function beginMediaSnapshotSubscription(
  onListenerFailure: (error: CommandError) => void,
  onSnapshot?: (snapshot: MediaSnapshot) => void,
): CommandSubscriptionHandle<MediaSnapshot> {
  const empty: MediaSnapshot = {
    sessionId: null,
    title: "",
    artist: "",
    playbackState: "unavailable",
    positionSeconds: 0,
    durationSeconds: null,
    volumePercent: null,
    canPlay: false,
    canPause: false,
    canPrevious: false,
    canNext: false,
    canSeek: false,
    canSetVolume: false,
    updatedAt: 0,
  };
  return beginCommandSubscription<MediaSnapshot, MediaSessionChangedPayload>(
    "mediaSessionChanged",
    () => getMediaSnapshot(),
    empty,
    onListenerFailure,
    onSnapshot,
  );
}

export async function subscribeMediaSnapshot(
  onListenerFailure: (error: CommandError) => void,
  onSnapshot?: (snapshot: MediaSnapshot) => void,
): Promise<CommandSubscription<MediaSnapshot>> {
  return beginMediaSnapshotSubscription(onListenerFailure, onSnapshot).ready;
}

export function beginMonitorMetricsSubscription(
  onListenerFailure: (error: CommandError) => void,
  onSnapshot?: (snapshot: MonitorSnapshot) => void,
): CommandSubscriptionHandle<MonitorSnapshot | null> {
  return beginCommandSubscription<MonitorSnapshot | null, MonitorMetricsChangedPayload>(
    "monitorMetricsChanged",
    () => getMonitorSnapshot(),
    null,
    onListenerFailure,
    (snapshot) => {
      if (snapshot !== null) onSnapshot?.(snapshot);
    },
    2_000,
    true,
    true,
  );
}

export async function subscribeMonitorMetrics(
  onListenerFailure: (error: CommandError) => void,
  onSnapshot?: (snapshot: MonitorSnapshot) => void,
): Promise<CommandSubscription<MonitorSnapshot | null>> {
  return beginMonitorMetricsSubscription(onListenerFailure, onSnapshot).ready;
}

export function beginNotificationHistorySubscription(
  input: ListNotificationHistoryInput,
  onListenerFailure: (error: CommandError) => void,
  onSnapshot?: (snapshot: NotificationHistoryItem[]) => void,
): CommandSubscriptionHandle<NotificationHistoryItem[]> {
  return beginCommandSubscription<NotificationHistoryItem[], NotificationHistoryChangedPayload>(
    "notificationHistoryChanged",
    () => listNotificationHistory(input),
    [],
    onListenerFailure,
    onSnapshot,
    30_000,
    true,
    true,
  );
}

export async function subscribeNotificationHistory(
  input: ListNotificationHistoryInput,
  onListenerFailure: (error: CommandError) => void,
  onSnapshot?: (snapshot: NotificationHistoryItem[]) => void,
): Promise<CommandSubscription<NotificationHistoryItem[]>> {
  return beginNotificationHistorySubscription(input, onListenerFailure, onSnapshot).ready;
}

export type ReminderConsumerId = "main-alerts" | "reminder-alert-window";

export interface ReminderDispatchSubscriptionOptions {
  consumerId: ReminderConsumerId;
  afterDispatchSeq?: number;
  render(deliveries: ReminderDelivery[]): void | Promise<void>;
  onListenerFailure(error: CommandError): void;
}

export interface ReminderDispatchSubscription extends CommandSubscription<ReminderDelivery[]> {
  lastDispatchSeq: number;
}

export interface ReminderDispatchSubscriptionHandle {
  ready: Promise<ReminderDispatchSubscription>;
  dispose(): void;
}

export function beginReminderDispatchSubscription(
  options: ReminderDispatchSubscriptionOptions,
): ReminderDispatchSubscriptionHandle {
  let cursor = options.afterDispatchSeq ?? 0;
  let subscription: ReminderDispatchSubscription | undefined;
  const load = async (isDisposed: () => boolean): Promise<ReminderDelivery[]> => {
    const byDeliveryId = new Map<string, ReminderDelivery>();
    let pageCursor = cursor;
    let highestFullyRendered = cursor;
    while (true) {
      const page = await replayReminderDeliveries({
        consumerId: options.consumerId,
        afterDispatchSeq: pageCursor,
        limit: 200,
      });
      if (isDisposed()) return [];
      for (const delivery of page.deliveries) byDeliveryId.set(delivery.id, delivery);
      highestFullyRendered = Math.max(highestFullyRendered, page.lastDispatchSeq);
      if (!page.hasMore) break;
      if (page.lastDispatchSeq <= pageCursor) throw new Error("reminder replay cursor did not advance");
      pageCursor = page.lastDispatchSeq;
    }
    const deliveries = [...byDeliveryId.values()].sort((left, right) => left.dispatchSeq - right.dispatchSeq);
    await options.render(deliveries);
    if (isDisposed()) return deliveries;
    if (deliveries.length > 0) {
      const committed = await commitReminderReplayCursor({
        consumerId: options.consumerId,
        lastDispatchSeq: highestFullyRendered,
      });
      cursor = Math.max(cursor, committed.lastDispatchSeq);
      if (subscription) subscription.lastDispatchSeq = cursor;
    } else {
      cursor = Math.max(cursor, highestFullyRendered);
      if (subscription) subscription.lastDispatchSeq = cursor;
    }
    return deliveries;
  };
  const handle = beginCommandSubscription<ReminderDelivery[], ReminderDispatchReadyPayload>(
    "reminderDispatchReady",
    load,
    [],
    options.onListenerFailure,
  );
  const ready = handle.ready.then((base) => {
    subscription = Object.assign(base, { lastDispatchSeq: cursor });
    return subscription;
  });
  return { ready, dispose: handle.dispose };
}

export async function subscribeReminderDispatch(
  options: ReminderDispatchSubscriptionOptions,
): Promise<ReminderDispatchSubscription> {
  return beginReminderDispatchSubscription(options).ready;
}

export const subscribeReminderDeliveries = subscribeReminderDispatch;

export function beginReminderNavigationSubscription(
  route: (pending: PendingReminderNavigation) => void | Promise<void>,
  onListenerFailure: (error: CommandError) => void,
): CommandSubscriptionHandle<PendingReminderNavigation | null> {
  const load = async (isDisposed: () => boolean) => {
    const pending = await getPendingReminderNavigation();
    if (isDisposed()) return pending;
    if (pending !== null) {
      await route(pending);
      if (isDisposed()) return pending;
      await acknowledgeReminderNavigation({ sequence: pending.sequence });
    }
    return pending;
  };
  return beginCommandSubscription<PendingReminderNavigation | null, ReminderNavigationRequestedPayload>(
    "reminderNavigationRequested",
    load,
    null,
    onListenerFailure,
  );
}

export async function subscribeReminderNavigation(
  route: (pending: PendingReminderNavigation) => void | Promise<void>,
  onListenerFailure: (error: CommandError) => void,
): Promise<CommandSubscription<PendingReminderNavigation | null>> {
  return beginReminderNavigationSubscription(route, onListenerFailure).ready;
}
