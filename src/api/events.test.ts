import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentProfilesSnapshot, ReminderDelivery, ServiceHealthSnapshot } from "./contracts";

type ImmediateHealthSubscription = {
  ready: Promise<{ initial: ServiceHealthSnapshot[]; dispose(): void }>;
  dispose(): void;
};

const { invokeMock, listenMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  listenMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

const eventsModulePath = "./events";
const commandsModulePath = "./commands";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
}

function reminderDelivery(id: string, dispatchSeq: number): ReminderDelivery {
  return {
    id,
    dedupeKey: `dedupe-${id}`,
    ruleId: "rule-1",
    sourceKind: "agent",
    sourceEntityId: "agent:rule-1:codex:windows:task-1:completed",
    messageKey: "reminders.agent.status",
    messageParameters: {
      agentName: "Codex",
      environment: "windows",
      taskId: "task-1",
      taskTitle: "Task 1",
      triggerStatus: "completed",
    },
    sourceContext: {
      kind: "agent",
      agentId: "codex",
      environment: "windows",
      taskId: "task-1",
      taskTitle: "Task 1",
      triggerStatus: "completed",
      sourceEventId: `event-${id}`,
      sourceOccurredAt: 1_000,
    },
    sourceOccurredAt: 1_000,
    sound: { kind: "none" },
    state: "dispatched",
    dueAt: 1_000,
    dispatchSeq,
    firstDispatchedAt: 1_001,
    lastDispatchedAt: 1_001,
    acknowledgedAt: null,
    completedAt: null,
    snoozedUntil: null,
    createdAt: 999,
    updatedAt: 1_001,
  };
}

describe("service health subscription", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    invokeMock.mockReset();
    listenMock.mockReset();
  });

  afterEach(() => {
    vi.doUnmock(commandsModulePath);
    vi.resetModules();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("registers the listener before loading the initial snapshot and unlistens once", async () => {
    const order: string[] = [];
    const unlisten = vi.fn();
    listenMock.mockImplementation(async () => {
      order.push("listen");
      return unlisten;
    });
    invokeMock.mockImplementation(async () => {
      order.push("snapshot");
      return [];
    });

    const { subscribeServiceHealth } = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await subscribeServiceHealth(() => undefined);

    expect(order).toEqual(["listen", "snapshot"]);
    expect(subscription.initial).toEqual([]);
    subscription.dispose();
    subscription.dispose();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("does not load a snapshot until listener registration has settled", async () => {
    const registration = deferred<() => void>();
    const unlisten = vi.fn();
    listenMock.mockReturnValue(registration.promise);
    invokeMock.mockResolvedValue([]);

    const { subscribeServiceHealth } = await import(/* @vite-ignore */ eventsModulePath);
    const subscriptionPromise = subscribeServiceHealth(() => undefined);
    await Promise.resolve();

    expect(invokeMock).not.toHaveBeenCalled();
    registration.resolve(unlisten);
    const subscription = await subscriptionPromise;

    expect(invokeMock).toHaveBeenCalledTimes(1);
    subscription.dispose();
  });

  it("cancels a pending listener before it can bootstrap while a newer handle stays active", async () => {
    const staleRegistration = deferred<() => void>();
    const staleUnlisten = vi.fn();
    const activeUnlisten = vi.fn();
    const snapshots: ServiceHealthSnapshot[][] = [];
    listenMock
      .mockReturnValueOnce(staleRegistration.promise)
      .mockResolvedValueOnce(activeUnlisten);
    invokeMock.mockResolvedValue([{ serviceId: "active", checkedAt: 2 }]);

    const module = await import(/* @vite-ignore */ eventsModulePath) as typeof import("./events") & {
      beginServiceHealthSubscription?: (
        onListenerFailure: (error: unknown) => void,
        onSnapshot?: (snapshot: ServiceHealthSnapshot[]) => void,
      ) => ImmediateHealthSubscription;
    };
    expect(module.beginServiceHealthSubscription).toEqual(expect.any(Function));
    const oldHandle = module.beginServiceHealthSubscription?.(() => undefined, (snapshot) => snapshots.push(snapshot));
    oldHandle?.dispose();
    const activeHandle = module.beginServiceHealthSubscription?.(() => undefined, (snapshot) => snapshots.push(snapshot));
    await activeHandle?.ready;

    staleRegistration.resolve(staleUnlisten);
    await staleRegistration.promise;
    await Promise.resolve();

    expect(staleUnlisten).toHaveBeenCalledTimes(1);
    expect(activeUnlisten).not.toHaveBeenCalled();
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(snapshots).toEqual([[{ serviceId: "active", checkedAt: 2 }]]);
    activeHandle?.dispose();
  });

  it("serializes initial and hinted snapshots so the subscription exposes the latest snapshot", async () => {
    let hint: (() => void) | undefined;
    const initial = deferred<unknown[]>();
    const trailing = deferred<unknown[]>();
    const stale = [{ serviceId: "storage", checkedAt: 1 }];
    const fresh = [{ serviceId: "storage", checkedAt: 2 }];
    const unlisten = vi.fn();
    listenMock.mockImplementation(async (_event: string, handler: (event: { payload: unknown }) => void) => {
      hint = () => handler({ payload: { serviceId: "storage", checkedAt: 2 } });
      return unlisten;
    });
    invokeMock
      .mockImplementationOnce(() => initial.promise)
      .mockImplementationOnce(() => trailing.promise);

    const { subscribeServiceHealth } = await import(/* @vite-ignore */ eventsModulePath);
    const subscriptionPromise = subscribeServiceHealth(() => undefined);
    await Promise.resolve();
    await Promise.resolve();
    expect(invokeMock).toHaveBeenCalledTimes(1);

    hint?.();
    hint?.();
    hint?.();
    await Promise.resolve();
    expect(invokeMock).toHaveBeenCalledTimes(1);

    initial.resolve(stale);
    await initial.promise;
    await Promise.resolve();
    await Promise.resolve();
    expect(invokeMock).toHaveBeenCalledTimes(2);

    trailing.resolve(fresh);
    await trailing.promise;
    const subscription = await subscriptionPromise;

    expect(subscription.initial).toEqual(fresh);
    subscription.dispose();
  });

  it("cleans up the installed listener once when the initial snapshot rejects with a typed error", async () => {
    const unlisten = vi.fn();
    listenMock.mockResolvedValue(unlisten);
    invokeMock.mockRejectedValueOnce("database closed");

    const { subscribeServiceHealth } = await import(/* @vite-ignore */ eventsModulePath);

    await expect(subscribeServiceHealth(() => undefined)).rejects.toEqual({
      code: "ioFailure",
      messageKey: "errors.ioFailure",
      details: {},
      retryable: false,
    });
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("coalesces hints received during one reload into one trailing reload", async () => {
    let hint: (() => void) | undefined;
    const reload = deferred<unknown[]>();
    const unlisten = vi.fn();
    listenMock.mockImplementation(async (_event: string, handler: (event: { payload: unknown }) => void) => {
      hint = () => handler({ payload: { serviceId: "storage", checkedAt: 1 } });
      return unlisten;
    });
    invokeMock
      .mockResolvedValueOnce([])
      .mockImplementationOnce(() => reload.promise)
      .mockResolvedValueOnce([]);

    const { subscribeServiceHealth } = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await subscribeServiceHealth(() => undefined);
    hint?.();
    hint?.();
    hint?.();
    await Promise.resolve();
    expect(invokeMock).toHaveBeenCalledTimes(2);

    reload.resolve([]);
    await reload.promise;
    await Promise.resolve();
    await Promise.resolve();

    expect(invokeMock).toHaveBeenCalledTimes(3);
    subscription.dispose();
  });

  it("starts a fresh flight when a hint lands between drain completion and flight cleanup", async () => {
    let hint: (() => void) | undefined;
    const sourceReload = deferred<unknown[]>();
    const finalReload = deferred<unknown[]>();
    const listServiceHealthMock = vi.fn();
    const unlisten = vi.fn();
    vi.doMock(commandsModulePath, () => ({ listServiceHealth: listServiceHealthMock }));
    vi.resetModules();
    listenMock.mockImplementation(async (_event: string, handler: (event: { payload: unknown }) => void) => {
      hint = () => handler({ payload: { serviceId: "storage", checkedAt: 3 } });
      return unlisten;
    });
    listServiceHealthMock
      .mockResolvedValueOnce([{ serviceId: "storage", checkedAt: 1 }])
      .mockImplementationOnce(() => sourceReload.promise)
      .mockImplementationOnce(() => finalReload.promise);

    const { subscribeServiceHealth } = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await subscribeServiceHealth(() => undefined);
    hint?.();
    await Promise.resolve();
    expect(listServiceHealthMock).toHaveBeenCalledTimes(2);

    void sourceReload.promise.then(() => hint?.());
    sourceReload.resolve([{ serviceId: "storage", checkedAt: 2 }]);
    await sourceReload.promise;
    await Promise.resolve();
    await Promise.resolve();

    expect(listServiceHealthMock).toHaveBeenCalledTimes(3);
    finalReload.resolve([{ serviceId: "storage", checkedAt: 3 }]);
    await finalReload.promise;
    await Promise.resolve();
    expect(subscription.initial).toEqual([{ serviceId: "storage", checkedAt: 3 }]);
    subscription.dispose();
  });

  it("reports a typed listener failure and polls once every thirty seconds without reconnecting", async () => {
    listenMock.mockRejectedValueOnce("event transport closed");
    invokeMock.mockResolvedValue([]);
    const report = vi.fn();

    const { subscribeServiceHealth } = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await subscribeServiceHealth(report);

    expect(report).toHaveBeenCalledWith({
      code: "ioFailure",
      messageKey: "errors.ioFailure",
      details: {},
      retryable: false,
    });
    expect(invokeMock).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(listenMock).toHaveBeenCalledTimes(1);

    subscription.dispose();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });

  it("publishes the initial and event-refreshed snapshots to an optional observer", async () => {
    let hint: (() => void) | undefined;
    const eventReload = deferred<unknown[]>();
    listenMock.mockImplementation(async (_event: string, handler: (event: { payload: unknown }) => void) => {
      hint = () => handler({ payload: { serviceId: "storage", checkedAt: 2 } });
      return vi.fn();
    });
    invokeMock
      .mockResolvedValueOnce([{ serviceId: "storage", checkedAt: 1 }])
      .mockImplementationOnce(() => eventReload.promise);
    const snapshots: ServiceHealthSnapshot[][] = [];

    const { subscribeServiceHealth } = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await subscribeServiceHealth(() => undefined, (snapshot: ServiceHealthSnapshot[]) => snapshots.push(snapshot));

    expect(snapshots).toEqual([[{ serviceId: "storage", checkedAt: 1 }]]);
    hint?.();
    eventReload.resolve([{ serviceId: "storage", checkedAt: 2 }]);
    await eventReload.promise;
    await Promise.resolve();
    expect(snapshots).toEqual([
      [{ serviceId: "storage", checkedAt: 1 }],
      [{ serviceId: "storage", checkedAt: 2 }],
    ]);

    subscription.dispose();
    hint?.();
    await Promise.resolve();
    expect(snapshots).toHaveLength(2);
  });

  it("publishes polling snapshots but never notifies after disposal", async () => {
    listenMock.mockRejectedValueOnce("event transport closed");
    invokeMock
      .mockResolvedValueOnce([{ serviceId: "storage", checkedAt: 1 }])
      .mockResolvedValueOnce([{ serviceId: "storage", checkedAt: 2 }]);
    const snapshots: ServiceHealthSnapshot[][] = [];

    const { subscribeServiceHealth } = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await subscribeServiceHealth(() => undefined, (snapshot: ServiceHealthSnapshot[]) => snapshots.push(snapshot));
    await vi.advanceTimersByTimeAsync(30_000);
    expect(snapshots).toEqual([
      [{ serviceId: "storage", checkedAt: 1 }],
      [{ serviceId: "storage", checkedAt: 2 }],
    ]);

    subscription.dispose();
    await vi.advanceTimersByTimeAsync(30_000);
    expect(snapshots).toHaveLength(2);
  });

  it("prevents late reload work after disposal while asynchronous unlisten settles", async () => {
    let hint: (() => void) | undefined;
    const reload = deferred<unknown[]>();
    const unlisten = vi.fn(() => deferred<void>().promise);
    listenMock.mockImplementation(async (_event: string, handler: (event: { payload: unknown }) => void) => {
      hint = () => handler({ payload: { serviceId: "storage", checkedAt: 1 } });
      return unlisten;
    });
    invokeMock
      .mockResolvedValueOnce([])
      .mockImplementationOnce(() => reload.promise)
      .mockResolvedValueOnce([]);

    const { subscribeServiceHealth } = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await subscribeServiceHealth(() => undefined);
    hint?.();
    await Promise.resolve();
    subscription.dispose();
    reload.resolve([]);
    await reload.promise;
    await Promise.resolve();
    await Promise.resolve();

    expect(unlisten).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });

  it("keeps parallel subscriptions isolated across listener failure, polling, and hints", async () => {
    let healthyHint: (() => void) | undefined;
    const healthyUnlisten = vi.fn();
    listenMock
      .mockRejectedValueOnce("event transport closed")
      .mockImplementationOnce(async (_event: string, handler: (event: { payload: unknown }) => void) => {
        healthyHint = () => handler({ payload: { serviceId: "storage", checkedAt: 1 } });
        return healthyUnlisten;
      });
    invokeMock.mockResolvedValue([]);
    const report = vi.fn();
    const { subscribeServiceHealth } = await import(/* @vite-ignore */ eventsModulePath);

    const failedListener = await subscribeServiceHealth(report);
    const healthyListener = await subscribeServiceHealth(() => undefined);
    await vi.advanceTimersByTimeAsync(30_000);
    healthyHint?.();
    await Promise.resolve();

    expect(report).toHaveBeenCalledTimes(1);
    expect(listenMock).toHaveBeenCalledTimes(2);
    expect(invokeMock).toHaveBeenCalledTimes(4);

    failedListener.dispose();
    await vi.advanceTimersByTimeAsync(30_000);
    expect(invokeMock).toHaveBeenCalledTimes(4);
    healthyListener.dispose();
    expect(healthyUnlisten).toHaveBeenCalledTimes(1);
  });
});

describe("agent and reminder subscriptions", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    invokeMock.mockReset();
    listenMock.mockReset();
  });

  afterEach(() => {
    vi.resetModules();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("registers every listener before its initial command refresh", async () => {
    const order: string[] = [];
    const unlisten = vi.fn();
    listenMock.mockImplementation(async () => {
      order.push("listen");
      return unlisten;
    });
    invokeMock.mockImplementation(async () => {
      order.push("command");
      return { agents: [], generatedAt: 1 };
    });

    const { subscribeAgentState } = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await subscribeAgentState(() => undefined);

    expect(order).toEqual(["listen", "command"]);
    subscription.dispose();
    subscription.dispose();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("listens first, then reloads the authoritative dynamic Profile snapshot after a persisted profile event", async () => {
    const order: string[] = [];
    let hint: (() => void) | undefined;
    const nextSnapshot = {
      generatedAt: 2,
      profiles: [{
        profile: {
          id: "kimi-windows",
          kind: "preset" as const,
          displayName: "Kimi Code",
          environment: "windows" as const,
          configTarget: { kind: "preset" as const, adapterId: "kimi" as const },
          eventMapping: [],
          enabled: true,
          installationState: "installed" as const,
          reasonCode: null,
          revision: 1,
          updatedAt: 1,
        },
        aggregateStatus: "running" as const,
        observations: [{
          profileId: "kimi-windows",
          environment: "windows" as const,
          taskId: "ship-profile-ui",
          status: "running" as const,
          sourceEventId: "kimi-event-2",
          occurredAt: 2,
          receivedAt: 2,
        }],
      }],
    };
    listenMock.mockImplementation(async (name: string, handler: (event: { payload: unknown }) => void) => {
      expect(name).toBe("agentProfileStateChanged");
      order.push("listen");
      hint = () => handler({ payload: { profileId: "kimi-windows", sourceEventId: "kimi-event-2", occurredAt: 2 } });
      return vi.fn();
    });
    invokeMock
      .mockImplementationOnce(async () => {
        order.push("initial");
        return { profiles: [], generatedAt: 1 };
      })
      .mockImplementationOnce(async () => {
        order.push("event");
        return nextSnapshot;
      });
    const received: AgentProfilesSnapshot[] = [];

    const { subscribeAgentProfileState } = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await subscribeAgentProfileState(() => undefined, (snapshot: AgentProfilesSnapshot) => received.push(snapshot));
    expect(order).toEqual(["listen", "initial"]);

    hint?.();
    await Promise.resolve();
    await Promise.resolve();

    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual([
      "getAgentProfilesSnapshot",
      "getAgentProfilesSnapshot",
    ]);
    expect(received.at(-1)).toEqual(nextSnapshot);
    subscription.dispose();
  });

  it("refreshes dynamic Profile process presence every two seconds without a Hook event", async () => {
    const detected = {
      generatedAt: 2,
      profiles: [{
        profile: {
          id: "kimi-windows",
          kind: "preset" as const,
          displayName: "Kimi Code",
          environment: "windows" as const,
          configTarget: { kind: "preset" as const, adapterId: "kimi" as const },
          eventMapping: [],
          enabled: false,
          installationState: "notInstalled" as const,
          reasonCode: null,
          revision: 1,
          updatedAt: 1,
        },
        aggregateStatus: "idle" as const,
        observations: [],
      }],
    };
    listenMock.mockResolvedValue(vi.fn());
    invokeMock
      .mockResolvedValueOnce({ profiles: [], generatedAt: 1 })
      .mockResolvedValueOnce(detected);
    const render = vi.fn();

    const { subscribeAgentProfileState } = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await subscribeAgentProfileState(() => undefined, render);

    await vi.advanceTimersByTimeAsync(1_999);
    expect(invokeMock).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual([
      "getAgentProfilesSnapshot",
      "getAgentProfilesSnapshot",
    ]);
    expect(render).toHaveBeenLastCalledWith(detected);

    subscription.dispose();
    await vi.advanceTimersByTimeAsync(2_000);
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });

  it("does not commit a replay cursor after its subscription is disposed during render", async () => {
    const rendering = deferred<void>();
    const renderStarted = deferred<void>();
    listenMock.mockResolvedValue(vi.fn());
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "replayReminderDeliveries") {
        return {
          deliveries: [reminderDelivery("delivery-1", 7)],
          lastDispatchSeq: 7,
          hasMore: false,
        };
      }
      if (command === "commitReminderReplayCursor") {
        return { consumerId: "main-alerts", lastDispatchSeq: 7 };
      }
      throw new Error(`unexpected command ${command}`);
    });

    const { beginReminderDispatchSubscription } = await import(/* @vite-ignore */ eventsModulePath);
    const handle = beginReminderDispatchSubscription({
      consumerId: "main-alerts",
      render: async () => {
        renderStarted.resolve();
        await rendering.promise;
      },
      onListenerFailure: () => undefined,
    });
    await renderStarted.promise;
    handle.dispose();
    rendering.resolve();
    await handle.ready;

    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual([
      "replayReminderDeliveries",
    ]);
  });

  it("does not request another replay page or render after disposal while the first page is pending", async () => {
    const firstPage = deferred<{
      deliveries: ReminderDelivery[];
      lastDispatchSeq: number;
      hasMore: boolean;
    }>();
    const render = vi.fn();
    listenMock.mockResolvedValue(vi.fn());
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "replayReminderDeliveries") {
        if (invokeMock.mock.calls.length === 1) return firstPage.promise;
        return { deliveries: [], lastDispatchSeq: 8, hasMore: false };
      }
      if (command === "commitReminderReplayCursor") {
        return { consumerId: "main-alerts", lastDispatchSeq: 8 };
      }
      throw new Error(`unexpected command ${command}`);
    });

    const { beginReminderDispatchSubscription } = await import(/* @vite-ignore */ eventsModulePath);
    const handle = beginReminderDispatchSubscription({
      consumerId: "main-alerts",
      render,
      onListenerFailure: () => undefined,
    });
    await Promise.resolve();
    await Promise.resolve();
    handle.dispose();
    firstPage.resolve({
      deliveries: [reminderDelivery("delivery-8", 8)],
      lastDispatchSeq: 8,
      hasMore: true,
    });
    await handle.ready;

    expect(render).not.toHaveBeenCalled();
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual([
      "replayReminderDeliveries",
    ]);
  });

  it("does not route or acknowledge after disposal while pending navigation is loading", async () => {
    const pending = deferred<{
      sequence: number;
      deliveryId: string;
      sourceKind: "agent";
      sourceEntityId: string;
    }>();
    const route = vi.fn();
    listenMock.mockResolvedValue(vi.fn());
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "getPendingReminderNavigation") return pending.promise;
      if (command === "acknowledgeReminderNavigation") return undefined;
      throw new Error(`unexpected command ${command}`);
    });

    const { beginReminderNavigationSubscription } = await import(/* @vite-ignore */ eventsModulePath);
    const handle = beginReminderNavigationSubscription(route, () => undefined);
    await Promise.resolve();
    await Promise.resolve();
    handle.dispose();
    pending.resolve({
      sequence: 22,
      deliveryId: "delivery-22",
      sourceKind: "agent",
      sourceEntityId: "agent:rule-1:codex:windows:task-1:completed",
    });
    await handle.ready;

    expect(route).not.toHaveBeenCalled();
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual([
      "getPendingReminderNavigation",
    ]);
  });

  it("coalesces agent hints into one trailing snapshot and never invokes Rust health", async () => {
    let hint: (() => void) | undefined;
    const eventReload = deferred<{ agents: never[]; generatedAt: number }>();
    listenMock.mockImplementation(async (name: string, handler: (event: { payload: unknown }) => void) => {
      expect(name).toBe("agentStateChanged");
      hint = () => handler({ payload: { agentId: "codex", environment: "windows", sourceEventId: "event-2", occurredAt: 2 } });
      return vi.fn();
    });
    invokeMock
      .mockResolvedValueOnce({ agents: [], generatedAt: 1 })
      .mockImplementationOnce(() => eventReload.promise)
      .mockResolvedValueOnce({ agents: [], generatedAt: 3 });

    const { subscribeAgentState } = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await subscribeAgentState(() => undefined);
    hint?.();
    hint?.();
    hint?.();
    await Promise.resolve();
    expect(invokeMock).toHaveBeenCalledTimes(2);
    eventReload.resolve({ agents: [], generatedAt: 2 });
    await eventReload.promise;
    await Promise.resolve();
    await Promise.resolve();

    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual([
      "getAgentsSnapshot",
      "getAgentsSnapshot",
      "getAgentsSnapshot",
    ]);
    subscription.dispose();
  });

  it("reloads exactly two seconds after an Agent hint so a completion flash can restore running", async () => {
    let hint: (() => void) | undefined;
    listenMock.mockImplementation(async (name: string, handler: (event: { payload: unknown }) => void) => {
      expect(name).toBe("agentStateChanged");
      hint = () => handler({ payload: { agentId: "codex", environment: "windows", sourceEventId: "completed-1", occurredAt: 1_000 } });
      return vi.fn();
    });
    invokeMock.mockResolvedValue({ agents: [], generatedAt: 1 });

    const { subscribeAgentState } = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await subscribeAgentState(() => undefined);
    await vi.advanceTimersByTimeAsync(1_000);
    hint?.();
    await Promise.resolve();
    expect(invokeMock).toHaveBeenCalledTimes(2);

    await vi.advanceTimersByTimeAsync(1_000);
    expect(invokeMock).toHaveBeenCalledTimes(3);
    await vi.advanceTimersByTimeAsync(999);
    expect(invokeMock).toHaveBeenCalledTimes(3);
    await vi.advanceTimersByTimeAsync(1);
    expect(invokeMock).toHaveBeenCalledTimes(4);

    subscription.dispose();
  });

  it("loads the Agent snapshot immediately when listener registration stalls, then returns to event-driven updates", async () => {
    const registration = deferred<() => void>();
    const unlisten = vi.fn();
    const snapshot = {
      agents: [{
        agentId: "codex",
        displayName: "Codex",
        aggregateStatus: "running",
        environments: [{
          agentId: "codex",
          environment: "windows",
          taskId: "process-presence",
          status: "running",
          summary: "",
          sourceEventId: "presence-1",
          occurredAt: 1,
          receivedAt: 1,
        }],
        integrations: [],
      }],
      generatedAt: 1,
    };
    listenMock.mockReturnValue(registration.promise);
    invokeMock.mockResolvedValue(snapshot);
    const render = vi.fn();

    const { beginAgentStateSubscription } = await import(/* @vite-ignore */ eventsModulePath);
    const handle = beginAgentStateSubscription(() => undefined, render);
    await Promise.resolve();
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual(["getAgentsSnapshot"]);
    await Promise.resolve();
    await Promise.resolve();
    expect(render).toHaveBeenCalledWith(snapshot);

    await vi.advanceTimersByTimeAsync(499);
    expect(invokeMock).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(1);
    const subscription = await handle.ready;
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual(["getAgentsSnapshot"]);
    expect(subscription.listenerState).toBe("degraded");

    await subscription.retry();
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual([
      "getAgentsSnapshot",
      "getAgentsSnapshot",
    ]);
    expect(subscription.listenerState).toBe("degraded");

    registration.resolve(unlisten);
    await registration.promise;
    await Promise.resolve();
    expect(subscription.listenerState).toBe("active");
    subscription.dispose();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("degrades to one two-second Agent poll without reconnecting and stops on dispose", async () => {
    listenMock.mockRejectedValueOnce("closed");
    invokeMock.mockResolvedValue({ agents: [], generatedAt: 1 });
    const failure = vi.fn();

    const { subscribeAgentState } = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await subscribeAgentState(failure);
    expect(subscription.listenerState).toBe("degraded");
    expect(listenMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(2_000);
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(listenMock).toHaveBeenCalledTimes(1);
    subscription.dispose();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });

  it("heals a missed process-presence transition with an authoritative poll while the listener is active", async () => {
    const empty = { agents: [], generatedAt: 1 };
    const running = {
      agents: [{
        agentId: "codex",
        displayName: "Codex",
        aggregateStatus: "running",
        environments: [{
          agentId: "codex",
          environment: "windows",
          taskId: "process-presence",
          status: "running",
          summary: "",
          sourceEventId: "presence-2",
          occurredAt: 2,
          receivedAt: 2,
        }],
        integrations: [],
      }],
      generatedAt: 2,
    };
    listenMock.mockResolvedValue(vi.fn());
    invokeMock.mockResolvedValueOnce(empty).mockResolvedValueOnce(running);
    const render = vi.fn();

    const { subscribeAgentState } = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await subscribeAgentState(() => undefined, render);
    expect(subscription.listenerState).toBe("active");
    expect(render).toHaveBeenLastCalledWith(empty);

    await vi.advanceTimersByTimeAsync(2_000);

    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual([
      "getAgentsSnapshot",
      "getAgentsSnapshot",
    ]);
    expect(render).toHaveBeenLastCalledWith(running);
    subscription.dispose();
  });

  it("keeps retrying when the initial Agent snapshot races native service startup", async () => {
    const running = {
      agents: [{
        agentId: "codex",
        displayName: "Codex",
        aggregateStatus: "running",
        environments: [{
          agentId: "codex",
          environment: "windows",
          taskId: "process-presence",
          status: "running",
          summary: "",
          latestReplyPreview: null,
          sourceEventId: "presence-2",
          occurredAt: 2,
          receivedAt: 2,
        }],
        integrations: [],
      }],
      generatedAt: 2,
    };
    listenMock.mockResolvedValue(vi.fn());
    invokeMock
      .mockRejectedValueOnce("state() called before manage()")
      .mockResolvedValueOnce(running);
    const render = vi.fn();
    const failure = vi.fn();

    const { subscribeAgentState } = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await subscribeAgentState(failure, render);

    expect(failure).toHaveBeenCalledWith({
      code: "ioFailure",
      messageKey: "errors.ioFailure",
      details: {},
      retryable: false,
    });
    expect(subscription.listenerState).toBe("active");
    expect(render).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(2_000);

    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual([
      "getAgentsSnapshot",
      "getAgentsSnapshot",
    ]);
    expect(render).toHaveBeenLastCalledWith(running);
    subscription.dispose();
  });

  it("paginates and deduplicates replay before render, then commits the rendered cursor", async () => {
    const order: string[] = [];
    let replayCall = 0;
    listenMock.mockImplementation(async (name: string) => {
      expect(name).toBe("reminderDispatchReady");
      order.push("listen");
      return vi.fn();
    });
    invokeMock.mockImplementation(async (command: string, payload?: Record<string, unknown>) => {
      if (command === "replayReminderDeliveries") {
        replayCall += 1;
        if (replayCall === 1) {
          order.push("command");
          expect(payload).toEqual({ consumerId: "main-alerts", afterDispatchSeq: 0, limit: 200 });
          return { deliveries: [reminderDelivery("d2", 2), reminderDelivery("d1", 1)], lastDispatchSeq: 2, hasMore: true };
        }
        expect(payload).toEqual({ consumerId: "main-alerts", afterDispatchSeq: 2, limit: 200 });
        return { deliveries: [reminderDelivery("d2", 2), reminderDelivery("d3", 3)], lastDispatchSeq: 3, hasMore: false };
      }
      if (command === "commitReminderReplayCursor") {
        order.push("commit");
        expect(payload).toEqual({ consumerId: "main-alerts", lastDispatchSeq: 3 });
        return { consumerId: "main-alerts", lastDispatchSeq: 3 };
      }
      throw new Error(`unexpected command ${command}`);
    });

    const { subscribeReminderDispatch } = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await subscribeReminderDispatch({
      consumerId: "main-alerts",
      render: (deliveries: ReminderDelivery[]) => {
        order.push("render");
        expect(deliveries.map((delivery) => delivery.id)).toEqual(["d1", "d2", "d3"]);
      },
      onListenerFailure: () => undefined,
    });

    expect(order).toEqual(["listen", "command", "render", "commit"]);
    expect(subscription.lastDispatchSeq).toBe(3);
    subscription.dispose();
  });

  it("routes durable navigation before acknowledging its exact sequence", async () => {
    const order: string[] = [];
    listenMock.mockImplementation(async (name: string) => {
      expect(name).toBe("reminderNavigationRequested");
      order.push("listen");
      return vi.fn();
    });
    invokeMock.mockImplementation(async (command: string, payload?: Record<string, unknown>) => {
      if (command === "getPendingReminderNavigation") {
        order.push("command");
        return { sequence: 12, deliveryId: "d1", sourceKind: "agent", sourceEntityId: "agent:1" };
      }
      if (command === "acknowledgeReminderNavigation") {
        order.push("ack");
        expect(payload).toEqual({ sequence: 12 });
        return undefined;
      }
      throw new Error(`unexpected command ${command}`);
    });

    const { subscribeReminderNavigation } = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await subscribeReminderNavigation(async () => {
      order.push("route");
    }, () => undefined);
    expect(order).toEqual(["listen", "command", "route", "ack"]);
    subscription.dispose();
  });

  it("resumes replay from the backend-persisted cursor after remount", async () => {
    let hint: (() => void) | undefined;
    const replayPayloads: unknown[] = [];
    listenMock.mockImplementation(async (_name: string, handler: (event: { payload: unknown }) => void) => {
      hint = () => handler({ payload: { dispatchSeq: 10, deliveryId: "d10" } });
      return vi.fn();
    });
    invokeMock.mockImplementation(async (command: string, payload?: Record<string, unknown>) => {
      if (command !== "replayReminderDeliveries") throw new Error(`unexpected command ${command}`);
      replayPayloads.push(payload);
      return { deliveries: [], lastDispatchSeq: 9, hasMore: false };
    });

    const { subscribeReminderDispatch } = await import(/* @vite-ignore */ eventsModulePath);
    const first = await subscribeReminderDispatch({
      consumerId: "main-alerts",
      render: () => undefined,
      onListenerFailure: () => undefined,
    });
    first.dispose();
    const remounted = await subscribeReminderDispatch({
      consumerId: "main-alerts",
      render: () => undefined,
      onListenerFailure: () => undefined,
    });
    hint?.();
    await Promise.resolve();
    await Promise.resolve();

    expect(replayPayloads).toEqual([
      { consumerId: "main-alerts", afterDispatchSeq: 0, limit: 200 },
      { consumerId: "main-alerts", afterDispatchSeq: 0, limit: 200 },
      { consumerId: "main-alerts", afterDispatchSeq: 9, limit: 200 },
    ]);
    remounted.dispose();
  });
});

describe("todo subscription", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    invokeMock.mockReset();
    listenMock.mockReset();
  });

  afterEach(() => {
    vi.resetModules();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("listens for exact todoChanged payloads and coalesces hints into one trailing load", async () => {
    let hint: (() => void) | undefined;
    const reload = deferred<unknown[]>();
    const unlisten = vi.fn();
    const order: string[] = [];
    listenMock.mockImplementation(async (name: string, handler: (event: { payload: unknown }) => void) => {
      expect(name).toBe("todoChanged");
      order.push("listen");
      hint = () => handler({ payload: { entityId: "todo-1", revision: 2, changedAt: 42 } });
      return unlisten;
    });
    invokeMock
      .mockImplementationOnce(async () => {
        order.push("list");
        return [];
      })
      .mockImplementationOnce(() => reload.promise)
      .mockResolvedValueOnce([]);
    const { listenTodoChanged, subscribeTodos } = await import(/* @vite-ignore */ eventsModulePath);
    const payloads: unknown[] = [];
    const stop = await listenTodoChanged((payload: unknown) => payloads.push(payload));
    hint?.();
    expect(payloads).toEqual([{ entityId: "todo-1", revision: 2, changedAt: 42 }]);
    stop();

    const subscription = await subscribeTodos({ status: "open", limit: 50 }, () => undefined);
    expect(order.slice(-2)).toEqual(["listen", "list"]);
    hint?.();
    hint?.();
    hint?.();
    await Promise.resolve();
    expect(invokeMock).toHaveBeenCalledTimes(2);
    reload.resolve([]);
    await reload.promise;
    await Promise.resolve();
    await Promise.resolve();
    expect(invokeMock).toHaveBeenCalledTimes(3);
    subscription.dispose();
    subscription.dispose();
    expect(unlisten).toHaveBeenCalledTimes(2);
  });

  it("reports listener rejection once, polls every thirty seconds, and stops on disposal", async () => {
    listenMock.mockRejectedValueOnce("closed");
    invokeMock.mockResolvedValue([]);
    const failure = vi.fn();
    const { subscribeTodos } = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await subscribeTodos({ status: "all", limit: 500 }, failure);
    expect(failure).toHaveBeenCalledTimes(1);
    expect(failure).toHaveBeenCalledWith({ code: "ioFailure", messageKey: "errors.ioFailure", details: {}, retryable: false });
    expect(invokeMock).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(listenMock).toHaveBeenCalledTimes(1);
    subscription.dispose();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });
});

describe("note subscription", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    invokeMock.mockReset();
    listenMock.mockReset();
  });

  afterEach(() => {
    vi.resetModules();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("listens for the exact noteChanged payload", async () => {
    let deliver: ((payload: unknown) => void) | undefined;
    listenMock.mockImplementation(async (name: string, handler: (event: { payload: unknown }) => void) => {
      expect(name).toBe("noteChanged");
      deliver = (payload) => handler({ payload });
      return vi.fn();
    });
    const events = await import(/* @vite-ignore */ eventsModulePath);
    const payloads: unknown[] = [];

    const stop = await events.listenNoteChanged((payload: unknown) => payloads.push(payload));
    deliver?.({ entityId: "note-1", revision: 2, changedAt: 42 });

    expect(payloads).toEqual([{ entityId: "note-1", revision: 2, changedAt: 42 }]);
    stop();
  });

  it("registers first, coalesces hints, and disposes idempotently", async () => {
    let hint: (() => void) | undefined;
    const reload = deferred<unknown[]>();
    const unlisten = vi.fn();
    const order: string[] = [];
    listenMock.mockImplementation(async (name: string, handler: (event: { payload: unknown }) => void) => {
      expect(name).toBe("noteChanged");
      order.push("listen");
      hint = () => handler({ payload: { entityId: "note-1", revision: 2, changedAt: 42 } });
      return unlisten;
    });
    invokeMock
      .mockImplementationOnce(async () => { order.push("list"); return []; })
      .mockImplementationOnce(() => reload.promise)
      .mockResolvedValueOnce([]);
    const events = await import(/* @vite-ignore */ eventsModulePath);

    const subscription = await events.subscribeNotes({ query: "", limit: 50 }, () => undefined);
    expect(order).toEqual(["listen", "list"]);
    hint?.();
    hint?.();
    hint?.();
    await Promise.resolve();
    expect(invokeMock).toHaveBeenCalledTimes(2);
    reload.resolve([]);
    await reload.promise;
    await Promise.resolve();
    await Promise.resolve();
    expect(invokeMock).toHaveBeenCalledTimes(3);
    subscription.dispose();
    subscription.dispose();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("reports one listener failure, polls every thirty seconds, and stops after disposal", async () => {
    listenMock.mockRejectedValueOnce("closed");
    invokeMock.mockResolvedValue([]);
    const failure = vi.fn();
    const events = await import(/* @vite-ignore */ eventsModulePath);

    const subscription = await events.subscribeNotes({ query: "needle", limit: 500 }, failure);
    expect(failure).toHaveBeenCalledTimes(1);
    expect(listenMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(listenMock).toHaveBeenCalledTimes(1);
    subscription.dispose();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });
});

describe("clipboard subscription", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    invokeMock.mockReset();
    listenMock.mockReset();
  });

  afterEach(() => {
    vi.resetModules();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("registers first and coalesces burst hints into one trailing authoritative reload", async () => {
    let hint: (() => void) | undefined;
    const reload = deferred<unknown[]>();
    const order: string[] = [];
    const unlisten = vi.fn();
    listenMock.mockImplementation(async (name: string, handler: (event: { payload: unknown }) => void) => {
      expect(name).toBe("clipboardChanged");
      order.push("listen");
      hint = () => handler({ payload: { entityId: "item-1", changedAt: 42 } });
      return unlisten;
    });
    invokeMock
      .mockImplementationOnce(async () => { order.push("list"); return []; })
      .mockImplementationOnce(() => reload.promise)
      .mockResolvedValueOnce([]);
    const events = await import(/* @vite-ignore */ eventsModulePath);

    const payloads: unknown[] = [];
    const stop = await events.listenClipboardChanged((payload: unknown) => payloads.push(payload));
    hint?.();
    expect(payloads).toEqual([{ entityId: "item-1", changedAt: 42 }]);
    stop();

    const subscription = await events.subscribeClipboardItems(
      { query: "build", contentKind: "all", limit: 500 },
      () => undefined,
    );
    expect(order.slice(-2)).toEqual(["listen", "list"]);
    hint?.();
    hint?.();
    hint?.();
    await Promise.resolve();
    expect(invokeMock).toHaveBeenCalledTimes(2);
    reload.resolve([]);
    await reload.promise;
    await Promise.resolve();
    await Promise.resolve();
    expect(invokeMock).toHaveBeenCalledTimes(3);
    subscription.dispose();
    subscription.dispose();
    expect(unlisten).toHaveBeenCalledTimes(2);
  });

  it("polls every thirty seconds after one registration failure and disposal clears it", async () => {
    listenMock.mockRejectedValueOnce("closed");
    invokeMock.mockResolvedValue([]);
    const failure = vi.fn();
    const events = await import(/* @vite-ignore */ eventsModulePath);

    const subscription = await events.subscribeClipboardItems(
      { query: "", contentKind: "image", limit: 100 },
      failure,
    );
    expect(failure).toHaveBeenCalledTimes(1);
    expect(listenMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(listenMock).toHaveBeenCalledTimes(1);
    subscription.dispose();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });
});

describe("media session subscription", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    invokeMock.mockReset();
    listenMock.mockReset();
  });

  afterEach(() => {
    vi.resetModules();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("emits the exact hint and listens before the initial authoritative snapshot", async () => {
    const order: string[] = [];
    let hint: ((payload: unknown) => void) | undefined;
    const unlisten = vi.fn();
    const snapshot = {
      sessionId: "app.session",
      title: "Track",
      artist: "Artist",
      playbackState: "playing",
      positionSeconds: 4,
      durationSeconds: 8,
      volumePercent: 35,
      canPlay: true,
      canPause: true,
      canPrevious: true,
      canNext: true,
      canSeek: true,
      canSetVolume: true,
      updatedAt: 42,
    };
    listenMock.mockImplementation(async (name: string, handler: (event: { payload: unknown }) => void) => {
      expect(name).toBe("mediaSessionChanged");
      order.push("listen");
      hint = (payload) => handler({ payload });
      return unlisten;
    });
    invokeMock.mockImplementation(async (name: string) => {
      expect(name).toBe("getMediaSnapshot");
      order.push("snapshot");
      return snapshot;
    });
    const events = await import(/* @vite-ignore */ eventsModulePath);

    const payloads: unknown[] = [];
    const stop = await events.listenMediaSessionChanged((payload: unknown) => payloads.push(payload));
    hint?.({ sessionId: "app.session", changedAt: 42 });
    expect(payloads).toEqual([{ sessionId: "app.session", changedAt: 42 }]);
    stop();

    const subscription = await events.subscribeMediaSnapshot(() => undefined);
    expect(order.slice(-2)).toEqual(["listen", "snapshot"]);
    subscription.dispose();
    expect(unlisten).toHaveBeenCalledTimes(2);
  });

  it("coalesces hints, polls after one listener failure, and stops on dispose", async () => {
    listenMock.mockRejectedValueOnce("closed");
    invokeMock.mockResolvedValue({ sessionId: null, playbackState: "unavailable" });
    const failure = vi.fn();
    const events = await import(/* @vite-ignore */ eventsModulePath);

    const subscription = await events.subscribeMediaSnapshot(failure);
    expect(failure).toHaveBeenCalledTimes(1);
    expect(listenMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(listenMock).toHaveBeenCalledTimes(1);
    subscription.dispose();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });
});

describe("monitor and notification history subscriptions", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    invokeMock.mockReset();
    listenMock.mockReset();
  });

  afterEach(() => {
    vi.resetModules();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("registers exact listeners before authoritative loads", async () => {
    const order: string[] = [];
    listenMock.mockImplementation(async (name: string) => {
      order.push(`listen:${name}`);
      return vi.fn();
    });
    invokeMock.mockImplementation(async (name: string) => {
      order.push(`load:${name}`);
      return name === "getMonitorSnapshot" ? { sampledAt: 1 } : [];
    });
    const events = await import(/* @vite-ignore */ eventsModulePath);

    const monitor = await events.subscribeMonitorMetrics(() => undefined);
    const history = await events.subscribeNotificationHistory(
      { origin: "all", sourceApp: null, unreadOnly: false, limit: 100 },
      () => undefined,
    );

    expect(order).toEqual([
      "listen:monitorMetricsChanged",
      "load:getMonitorSnapshot",
      "listen:notificationHistoryChanged",
      "load:listNotificationHistory",
    ]);
    monitor.dispose();
    history.dispose();
  });

  it("forwards exact event payloads and coalesces in-flight monitor hints into one trailing reload", async () => {
    let monitorHint: ((payload: unknown) => void) | undefined;
    let historyHint: ((payload: unknown) => void) | undefined;
    const unlisten = vi.fn();
    listenMock.mockImplementation(async (name: string, handler: (event: { payload: unknown }) => void) => {
      if (name === "monitorMetricsChanged") monitorHint = (payload) => handler({ payload });
      if (name === "notificationHistoryChanged") historyHint = (payload) => handler({ payload });
      return unlisten;
    });
    const events = await import(/* @vite-ignore */ eventsModulePath);
    const monitorPayloads: unknown[] = [];
    const historyPayloads: unknown[] = [];
    const stopMonitor = await events.listenMonitorMetricsChanged((payload: unknown) => monitorPayloads.push(payload));
    monitorHint?.({ sampledAt: 42 });
    const stopHistory = await events.listenNotificationHistoryChanged((payload: unknown) => historyPayloads.push(payload));
    historyHint?.({ newestReceivedAt: 43, origin: "windows" });
    expect(monitorPayloads).toEqual([{ sampledAt: 42 }]);
    expect(historyPayloads).toEqual([{ newestReceivedAt: 43, origin: "windows" }]);
    stopMonitor();
    stopHistory();

    const active = deferred<{ sampledAt: number }>();
    invokeMock.mockResolvedValueOnce({ sampledAt: 1 }).mockReturnValueOnce(active.promise).mockResolvedValueOnce({ sampledAt: 3 });
    const subscription = await events.subscribeMonitorMetrics(() => undefined);
    monitorHint?.({ sampledAt: 2 });
    monitorHint?.({ sampledAt: 2 });
    monitorHint?.({ sampledAt: 2 });
    await Promise.resolve();
    expect(invokeMock).toHaveBeenCalledTimes(2);
    active.resolve({ sampledAt: 2 });
    await active.promise;
    await Promise.resolve();
    await Promise.resolve();
    expect(invokeMock).toHaveBeenCalledTimes(3);
    subscription.dispose();
  });

  it("uses one two-second and one thirty-second degraded poll with exact cleanup", async () => {
    listenMock.mockRejectedValue("closed");
    invokeMock.mockImplementation(async (name: string) => name === "getMonitorSnapshot" ? { sampledAt: 1 } : []);
    const failure = vi.fn();
    const events = await import(/* @vite-ignore */ eventsModulePath);

    const monitor = await events.subscribeMonitorMetrics(failure);
    const history = await events.subscribeNotificationHistory(
      { origin: "all", sourceApp: null, unreadOnly: false, limit: 100 },
      failure,
    );
    expect(invokeMock).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(2_000);
    expect(invokeMock.mock.calls.filter(([name]) => name === "getMonitorSnapshot")).toHaveLength(2);
    expect(invokeMock.mock.calls.filter(([name]) => name === "listNotificationHistory")).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(28_000);
    expect(invokeMock.mock.calls.filter(([name]) => name === "listNotificationHistory")).toHaveLength(2);
    monitor.dispose();
    history.dispose();
    const count = invokeMock.mock.calls.length;
    await vi.advanceTimersByTimeAsync(60_000);
    expect(invokeMock).toHaveBeenCalledTimes(count);
    expect(listenMock).toHaveBeenCalledTimes(2);
  });

  it("reinstalls a failed monitor listener before Retry returns to event-driven updates", async () => {
    let recoveredHint: (() => void) | undefined;
    const recoveredUnlisten = vi.fn();
    listenMock
      .mockRejectedValueOnce("closed")
      .mockImplementationOnce(async (_name: string, handler: (event: { payload: unknown }) => void) => {
        recoveredHint = () => handler({ payload: { sampledAt: 3 } });
        return recoveredUnlisten;
      });
    invokeMock
      .mockResolvedValueOnce({ sampledAt: 1 })
      .mockResolvedValueOnce({ sampledAt: 2 })
      .mockResolvedValueOnce({ sampledAt: 3 });
    const events = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await events.subscribeMonitorMetrics(() => undefined);
    expect(subscription.listenerState).toBe("degraded");
    expect(subscription.initial).toEqual({ sampledAt: 1 });

    await subscription.retry();
    expect(listenMock).toHaveBeenCalledTimes(2);
    expect(subscription.listenerState).toBe("active");
    expect(subscription.initial).toEqual({ sampledAt: 2 });

    recoveredHint?.();
    await vi.waitFor(() => expect(subscription.initial).toEqual({ sampledAt: 3 }));
    const invokeCount = invokeMock.mock.calls.length;
    await vi.advanceTimersByTimeAsync(2_000);
    expect(invokeMock).toHaveBeenCalledTimes(invokeCount);
    subscription.dispose();
    expect(recoveredUnlisten).toHaveBeenCalledTimes(1);
  });

  it("reinstalls a failed notification listener before Retry stops degraded polling", async () => {
    let recoveredHint: (() => void) | undefined;
    const recoveredUnlisten = vi.fn();
    listenMock
      .mockRejectedValueOnce("closed")
      .mockImplementationOnce(async (_name: string, handler: (event: { payload: unknown }) => void) => {
        recoveredHint = () => handler({ payload: { newestReceivedAt: 3, origin: "windows" } });
        return recoveredUnlisten;
      });
    invokeMock
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([{ id: "notification-2" }])
      .mockResolvedValueOnce([{ id: "notification-3" }]);
    const events = await import(/* @vite-ignore */ eventsModulePath);
    const subscription = await events.subscribeNotificationHistory(
      { origin: "all", sourceApp: null, unreadOnly: false, limit: 100 },
      () => undefined,
    );
    expect(subscription.listenerState).toBe("degraded");

    await subscription.retry();
    expect(listenMock).toHaveBeenCalledTimes(2);
    expect(subscription.listenerState).toBe("active");
    expect(subscription.initial).toEqual([{ id: "notification-2" }]);

    recoveredHint?.();
    await vi.waitFor(() => expect(subscription.initial).toEqual([{ id: "notification-3" }]));
    const invokeCount = invokeMock.mock.calls.length;
    await vi.advanceTimersByTimeAsync(30_000);
    expect(invokeMock).toHaveBeenCalledTimes(invokeCount);
    subscription.dispose();
    expect(recoveredUnlisten).toHaveBeenCalledTimes(1);
  });

  it("keeps the listener and fallback alive when the first authoritative load fails", async () => {
    const monitorUnlisten = vi.fn();
    listenMock
      .mockResolvedValueOnce(monitorUnlisten)
      .mockRejectedValueOnce("listener closed");
    invokeMock
      .mockRejectedValueOnce({
        code: "sourceUnavailable",
        messageKey: "errors.sourceUnavailable",
        details: { serviceId: "monitorCore" },
        retryable: true,
      })
      .mockRejectedValueOnce({
        code: "databaseFailure",
        messageKey: "errors.databaseFailure",
        details: {},
        retryable: true,
      })
      .mockResolvedValueOnce([]);
    const failure = vi.fn();
    const events = await import(/* @vite-ignore */ eventsModulePath);

    const monitor = await events.subscribeMonitorMetrics(failure);
    expect(monitor.initial).toBeNull();
    expect(monitor.listenerState).toBe("active");
    expect(monitorUnlisten).not.toHaveBeenCalled();

    const history = await events.subscribeNotificationHistory(
      { origin: "all", sourceApp: null, unreadOnly: false, limit: 100 },
      failure,
    );
    expect(history.initial).toEqual([]);
    expect(history.listenerState).toBe("degraded");
    await vi.advanceTimersByTimeAsync(30_000);
    expect(invokeMock.mock.calls.filter(([name]) => name === "listNotificationHistory")).toHaveLength(2);
    expect(listenMock).toHaveBeenCalledTimes(2);
    monitor.dispose();
    history.dispose();
    expect(monitorUnlisten).toHaveBeenCalledTimes(1);
  });
});
