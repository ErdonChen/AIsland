import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

const { invokeMock, listenMock, beginAgentProfileStateSubscriptionMock, beginAgentStateSubscriptionMock, beginReminderDispatchSubscriptionMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  listenMock: vi.fn(),
  beginAgentProfileStateSubscriptionMock: vi.fn(),
  beginAgentStateSubscriptionMock: vi.fn(),
  beginReminderDispatchSubscriptionMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));
vi.mock("../api/events", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../api/events")>()),
  beginAgentProfileStateSubscription: beginAgentProfileStateSubscriptionMock,
  beginAgentStateSubscription: beginAgentStateSubscriptionMock,
  beginReminderDispatchSubscription: beginReminderDispatchSubscriptionMock,
}));
vi.mock("../pages/MonitorPage", () => ({ default: () => <section aria-label="monitor-page">Monitor surface</section> }));
vi.mock("../pages/NotificationCenterPage", () => ({ default: () => <section aria-label="notification-center-page">Notification surface</section> }));

import IslandShell from "./IslandShell";
import { I18nProvider } from "../i18n/I18nProvider";

let renderReminderDeliveries: ((deliveries: import("../api/contracts").ReminderDelivery[]) => void | Promise<void>) | undefined;

test("subscribes once and renders its authoritative agent snapshot on Home", async () => {
  // Removing the lifecycle subscription or bypassing the shared snapshot must fail this integration contract.
  beginAgentStateSubscriptionMock.mockReturnValue({
    ready: Promise.resolve({
      initial: {
        generatedAt: 1,
        agents: [{
          agentId: "codex", displayName: "Codex", aggregateStatus: "running", integrations: [],
          environments: [{ agentId: "codex", environment: "windows", taskId: "task-1", status: "running", summary: "Build", sourceEventId: "event-1", occurredAt: 1, receivedAt: 1 }],
        }],
      },
      dispose: vi.fn(),
    }),
    dispose: vi.fn(),
  });
  renderShell();

  await waitFor(() => {
    expect(beginAgentStateSubscriptionMock).toHaveBeenCalledTimes(1);
  });
  expect(screen.getByRole("article", { name: /Codex.*运行中/ })).toBeInTheDocument();
});

test("does not render unopened fixed Agents from an empty-source snapshot", async () => {
  beginAgentStateSubscriptionMock.mockReturnValue({
    ready: Promise.resolve({
      initial: {
        generatedAt: 1,
        agents: [
          { agentId: "codex", displayName: "Codex", aggregateStatus: "offline", integrations: [], environments: [] },
          {
            agentId: "hermes",
            displayName: "Hermes",
            aggregateStatus: "offline",
            integrations: [],
            environments: [{
              agentId: "hermes",
              environment: "windows",
              taskId: "process-presence",
              status: "offline",
              summary: "",
              latestReplyPreview: null,
              sourceEventId: "presence-offline",
              occurredAt: 1,
              receivedAt: 1,
            }],
          },
        ],
      },
      dispose: vi.fn(),
    }),
    dispose: vi.fn(),
  });
  renderShell();

  expect(await screen.findByText("当前没有运行中的 Agent")).toBeInTheDocument();
  expect(screen.queryByRole("article", { name: /Codex/ })).not.toBeInTheDocument();
  expect(screen.queryByRole("article", { name: /Hermes/ })).not.toBeInTheDocument();
});

test("opens an installed Kimi Profile, then lets tray Settings clear focus and acknowledge root once", async () => {
  const user = userEvent.setup();
  const nativeAcknowledge = deferred<void>();
  const acknowledgementRoots: boolean[] = [];
  let pendingNavigation: { page: "settings"; sequence: number } | null = null;
  let trayNavigateListener: ((event: { payload: string }) => void) | undefined;
  const kimiProfile = {
    id: "kimi-windows", kind: "preset" as const, displayName: "Kimi Code", environment: "windows" as const,
    configTarget: { kind: "preset" as const, adapterId: "kimi" as const }, eventMapping: [], enabled: true,
    installationState: "installed" as const, reasonCode: null, revision: 1, updatedAt: 2,
  };
  let onProfileSnapshot: ((snapshot: import("../api/contracts").AgentProfilesSnapshot) => void) | undefined;
  beginAgentProfileStateSubscriptionMock.mockImplementation((_: unknown, onSnapshot: typeof onProfileSnapshot) => {
    onProfileSnapshot = onSnapshot;
    return {
      ready: Promise.resolve({ initial: { profiles: [], generatedAt: 1 }, dispose: vi.fn() }),
      dispose: vi.fn(),
    };
  });
  listenMock.mockImplementation(async (event: string, handler: (event: { payload: string }) => void) => {
    if (event === "tray-navigate") trayNavigateListener = handler;
    return vi.fn();
  });
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve({ ...INITIAL_STATE, mode: "collapsed" as const });
    if (command === "get_pending_tray_navigation") return Promise.resolve(pendingNavigation);
    if (command === "getPendingReminderNavigation") return Promise.resolve(null);
    if (command === "listAgentIntegrationProfiles") return Promise.resolve([kimiProfile]);
    if (command === "acknowledge_tray_navigation") {
      acknowledgementRoots.push(screen.queryByRole("button", { name: "通用" }) !== null);
      return nativeAcknowledge.promise;
    }
    return Promise.resolve(undefined);
  });
  renderShell();

  await waitFor(() => expect(beginAgentProfileStateSubscriptionMock).toHaveBeenCalledTimes(1));
  await act(async () => {
    onProfileSnapshot?.({
      generatedAt: 2,
      profiles: [{
        profile: kimiProfile,
        aggregateStatus: "running",
        observations: [{
          profileId: "kimi-windows", environment: "windows", taskId: "ship", status: "running",
          sourceEventId: "kimi-event-2", occurredAt: 2, receivedAt: 2,
        }],
      }],
    });
  });

  const kimi = document.querySelector('[data-profile-id="kimi-windows"]') as HTMLButtonElement;
  expect(kimi).toHaveAttribute("aria-label", expect.stringContaining("Kimi Code"));
  expect(screen.getByLabelText("工作中")).toBeInTheDocument();

  await user.click(kimi);
  await waitFor(() => {
    const focusedProfile = document.querySelector('article[data-profile-id="kimi-windows"]');
    expect(focusedProfile).toHaveFocus();
  });
  expect(document.querySelector(".island-mini-status .status-dot--pulse")).toBeInTheDocument();

  pendingNavigation = { page: "settings", sequence: 41 };
  await act(async () => {
    trayNavigateListener?.({ payload: "settings" });
    await Promise.resolve();
  });
  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("acknowledge_tray_navigation", { sequence: 41 }));
  expect(screen.getByRole("button", { name: "通用" })).toBeInTheDocument();
  expect(document.querySelector('article[data-profile-id="kimi-windows"]')).not.toBeInTheDocument();
  expect(acknowledgementRoots).toEqual([true]);

  await act(async () => {
    trayNavigateListener?.({ payload: "settings" });
    await Promise.resolve();
  });
  expect(invokeMock.mock.calls.filter(([command]) => command === "acknowledge_tray_navigation")).toHaveLength(1);
  pendingNavigation = null;
  await act(async () => {
    nativeAcknowledge.resolve();
    await nativeAcknowledge.promise;
  });
});

test("keeps Todo and Monitor reminder navigation durable while Agent navigation opens Home then acknowledges", async () => {
  // Acknowleding an unknown context, or acknowledging before routing its Agent, must fail this durable-navigation contract.
  beginAgentStateSubscriptionMock.mockReturnValue({
    ready: Promise.resolve({ initial: { generatedAt: 1, agents: [{ agentId: "codex", displayName: "Codex", aggregateStatus: "failed", integrations: [], environments: [
      { agentId: "codex", environment: "windows", taskId: "task", status: "failed", summary: "Task", sourceEventId: "task", occurredAt: 1, receivedAt: 1 },
    ] }] }, dispose: vi.fn() }),
    dispose: vi.fn(),
  });
  let reminderListener: ((event: { payload: { sequence: number } }) => void) | undefined;
  listenMock.mockImplementation((eventName: string, handler: (event: { payload: { sequence: number } }) => void) => {
    if (eventName === "reminderNavigationRequested") reminderListener = handler;
    return Promise.resolve(vi.fn());
  });
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve(INITIAL_STATE);
    if (command === "get_pending_tray_navigation") return Promise.resolve(null);
    if (command === "getPendingReminderNavigation") return Promise.resolve({ sequence: 7, deliveryId: "d-1", sourceKind: "agent", sourceEntityId: "agent:rule:codex:windows:task:failed" });
    return Promise.resolve(undefined);
  });
  renderShell();
  await waitFor(() => expect(reminderListener).toBeTypeOf("function"));
  await act(async () => reminderListener?.({ payload: { sequence: 7 } }));
  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("acknowledgeReminderNavigation", { sequence: 7 }));
});

test("commits the exact colon-containing Agent context before acknowledging its reminder", async () => {
  // The acknowledgement is an external side effect: it must not outrun React's selected row commit.
  let reminderListener: ((event: { payload: { sequence: number } }) => void) | undefined;
  let pending: { sequence: number; deliveryId: string; sourceKind: "agent"; sourceEntityId: string } | null = null;
  const committedRowsAtAcknowledgement: string[] = [];
  beginAgentStateSubscriptionMock.mockReturnValue({
    ready: Promise.resolve({ initial: {
      generatedAt: 1,
      agents: [{ agentId: "codex", displayName: "Codex", aggregateStatus: "failed", integrations: [], environments: [
        { agentId: "codex", environment: "windows", taskId: "task:part:42", status: "failed", summary: "Exact task", sourceEventId: "source-1", occurredAt: 1, receivedAt: 1 },
      ] }],
    }, dispose: vi.fn() }),
    dispose: vi.fn(),
  });
  listenMock.mockImplementation((eventName: string, handler: (event: { payload: { sequence: number } }) => void) => {
    if (eventName === "reminderNavigationRequested") reminderListener = handler;
    return Promise.resolve(vi.fn());
  });
  invokeMock.mockImplementation((command: string, args?: { sequence?: number }) => {
    if (command === "get_initial_state") return Promise.resolve(INITIAL_STATE);
    if (command === "get_pending_tray_navigation") return Promise.resolve(null);
    if (command === "getPendingReminderNavigation") return Promise.resolve(pending);
    if (command === "acknowledgeReminderNavigation") {
      committedRowsAtAcknowledgement.push(
        screen.queryByText("task:part:42")?.closest(".agent-detail__row")?.getAttribute("aria-current") ?? "missing",
      );
      expect(args).toEqual({ sequence: 17 });
    }
    return Promise.resolve(undefined);
  });
  renderShell();
  await waitFor(() => expect(reminderListener).toBeTypeOf("function"));
  pending = { sequence: 17, deliveryId: "delivery-17", sourceKind: "agent", sourceEntityId: "agent:rule-17:codex:windows:task:part:42:failed" };
  await act(async () => reminderListener?.({ payload: { sequence: 17 } }));

  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("acknowledgeReminderNavigation", { sequence: 17 }));
  expect(committedRowsAtAcknowledgement).toEqual(["true"]);
});

test("renders a durable failed Agent reminder fallback before acknowledging when live state already advanced", async () => {
  let reminderListener: ((event: { payload: { sequence: number } }) => void) | undefined;
  let pending: { sequence: number; deliveryId: string; sourceKind: "agent"; sourceEntityId: string } | null = null;
  const fallbackAtAcknowledgement: Array<{ environment: string | null; taskId: string | null; status: string | null }> = [];
  beginAgentStateSubscriptionMock.mockReturnValue({
    ready: Promise.resolve({ initial: { generatedAt: 2, agents: [{ agentId: "codex", displayName: "Codex", aggregateStatus: "completed", integrations: [], environments: [
      { agentId: "codex", environment: "windows", taskId: "task-advanced", status: "completed", summary: "Live state advanced", sourceEventId: "completed", occurredAt: 2, receivedAt: 2 },
    ] }] }, dispose: vi.fn() }),
    dispose: vi.fn(),
  });
  listenMock.mockImplementation((eventName: string, handler: (event: { payload: { sequence: number } }) => void) => {
    if (eventName === "reminderNavigationRequested") reminderListener = handler;
    return Promise.resolve(vi.fn());
  });
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve(INITIAL_STATE);
    if (command === "get_pending_tray_navigation") return Promise.resolve(null);
    if (command === "getPendingReminderNavigation") return Promise.resolve(pending);
    if (command === "acknowledgeReminderNavigation") {
      const fallback = screen.getByTestId("agent-reminder-context-codex");
      fallbackAtAcknowledgement.push({
        environment: fallback.getAttribute("data-environment"),
        taskId: fallback.getAttribute("data-task-id"),
        status: fallback.getAttribute("data-trigger-status"),
      });
    }
    return Promise.resolve(undefined);
  });
  renderShell();
  await waitFor(() => expect(reminderListener).toBeTypeOf("function"));
  pending = { sequence: 18, deliveryId: "delivery-18", sourceKind: "agent", sourceEntityId: "agent:rule-18:codex:windows:task-advanced:failed" };
  await act(async () => reminderListener?.({ payload: { sequence: 18 } }));

  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("acknowledgeReminderNavigation", { sequence: 18 }));
  expect(fallbackAtAcknowledgement).toEqual([{ environment: "windows", taskId: "task-advanced", status: "failed" }]);
});

test("commits the exact reminder fallback before acknowledging a selected Agent with no live observations", async () => {
  let reminderListener: ((event: { payload: { sequence: number } }) => void) | undefined;
  let pending: { sequence: number; deliveryId: string; sourceKind: "agent"; sourceEntityId: string } | null = null;
  const markerAtAcknowledgement: string[] = [];
  beginAgentStateSubscriptionMock.mockReturnValue({
    ready: Promise.resolve({ initial: { generatedAt: 3, agents: [{ agentId: "codex", displayName: "Codex", aggregateStatus: "offline", integrations: [], environments: [] }] }, dispose: vi.fn() }),
    dispose: vi.fn(),
  });
  listenMock.mockImplementation((eventName: string, handler: (event: { payload: { sequence: number } }) => void) => {
    if (eventName === "reminderNavigationRequested") reminderListener = handler;
    return Promise.resolve(vi.fn());
  });
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve(INITIAL_STATE);
    if (command === "get_pending_tray_navigation") return Promise.resolve(null);
    if (command === "getPendingReminderNavigation") return Promise.resolve(pending);
    if (command === "acknowledgeReminderNavigation") {
      const marker = screen.queryByTestId("agent-reminder-context-codex");
      markerAtAcknowledgement.push(marker === null ? "missing" : [
        marker.getAttribute("data-environment"),
        marker.getAttribute("data-task-id"),
        marker.getAttribute("data-trigger-status"),
      ].join(":"));
    }
    return Promise.resolve(undefined);
  });
  renderShell();
  await waitFor(() => expect(reminderListener).toBeTypeOf("function"));
  pending = { sequence: 19, deliveryId: "delivery-19", sourceKind: "agent", sourceEntityId: "agent:rule-19:codex:wsl:task-empty:timeout" };
  await act(async () => reminderListener?.({ payload: { sequence: 19 } }));

  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("acknowledgeReminderNavigation", { sequence: 19 }));
  expect(markerAtAcknowledgement).toEqual(["wsl:task-empty:timeout"]);
  expect(screen.getByTestId("agent-reminder-context-codex")).toHaveTextContent("task-empty");
});

test("keeps Agent reminder navigation durable while the decoded Agent is absent", async () => {
  let reminderListener: ((event: { payload: { sequence: number } }) => void) | undefined;
  let pending: { sequence: number; deliveryId: string; sourceKind: "agent"; sourceEntityId: string } | null = null;
  beginAgentStateSubscriptionMock.mockReturnValue({
    ready: Promise.resolve({ initial: { generatedAt: 4, agents: [] }, dispose: vi.fn() }),
    dispose: vi.fn(),
  });
  listenMock.mockImplementation((eventName: string, handler: (event: { payload: { sequence: number } }) => void) => {
    if (eventName === "reminderNavigationRequested") reminderListener = handler;
    return Promise.resolve(vi.fn());
  });
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve(INITIAL_STATE);
    if (command === "get_pending_tray_navigation") return Promise.resolve(null);
    if (command === "getPendingReminderNavigation") return Promise.resolve(pending);
    return Promise.resolve(undefined);
  });
  renderShell();
  await waitFor(() => expect(reminderListener).toBeTypeOf("function"));
  pending = { sequence: 20, deliveryId: "delivery-20", sourceKind: "agent", sourceEntityId: "agent:rule-20:codex:windows:missing:failed" };
  await act(async () => reminderListener?.({ payload: { sequence: 20 } }));
  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("getPendingReminderNavigation"));
  await act(async () => Promise.resolve());
  expect(invokeMock).not.toHaveBeenCalledWith("acknowledgeReminderNavigation", { sequence: 20 });
});

test("serializes a newer reminder navigation query behind an in-flight older query", async () => {
  // A newer event must set the dirty bit, not issue a concurrent query that can resolve out of order.
  const pendingA = deferred<{ sequence: number; deliveryId: string; sourceKind: "agent"; sourceEntityId: string } | null>();
  const pendingB = deferred<{ sequence: number; deliveryId: string; sourceKind: "agent"; sourceEntityId: string } | null>();
  let reminderListener: ((event: { payload: { sequence: number } }) => void) | undefined;
  let queryCount = 0;
  beginAgentStateSubscriptionMock.mockReturnValue({
    ready: Promise.resolve({ initial: { generatedAt: 1, agents: [{ agentId: "codex", displayName: "Codex", aggregateStatus: "failed", integrations: [], environments: [
      { agentId: "codex", environment: "windows", taskId: "task-a", status: "failed", summary: "A", sourceEventId: "a", occurredAt: 1, receivedAt: 1 },
      { agentId: "codex", environment: "wsl", taskId: "task-b", status: "timeout", summary: "B", sourceEventId: "b", occurredAt: 2, receivedAt: 2 },
    ] }] }, dispose: vi.fn() }),
    dispose: vi.fn(),
  });
  listenMock.mockImplementation((eventName: string, handler: (event: { payload: { sequence: number } }) => void) => {
    if (eventName === "reminderNavigationRequested") reminderListener = handler;
    return Promise.resolve(vi.fn());
  });
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve(INITIAL_STATE);
    if (command === "get_pending_tray_navigation") return Promise.resolve(null);
    if (command === "getPendingReminderNavigation") {
      queryCount += 1;
      if (queryCount === 1) return Promise.resolve(null);
      if (queryCount === 2) return pendingA.promise;
      return pendingB.promise;
    }
    return Promise.resolve(undefined);
  });
  renderShell();
  await waitFor(() => expect(reminderListener).toBeTypeOf("function"));
  await waitFor(() => expect(queryCount).toBe(1));

  await act(async () => reminderListener?.({ payload: { sequence: 20 } }));
  await waitFor(() => expect(queryCount).toBe(2));
  await act(async () => reminderListener?.({ payload: { sequence: 21 } }));
  expect(queryCount).toBe(2);

  await act(async () => {
    pendingA.resolve({ sequence: 20, deliveryId: "delivery-a", sourceKind: "agent", sourceEntityId: "agent:rule-a:codex:windows:task-a:failed" });
    await pendingA.promise;
  });
  await waitFor(() => expect(queryCount).toBe(3));
  await act(async () => {
    pendingB.resolve({ sequence: 21, deliveryId: "delivery-b", sourceKind: "agent", sourceEntityId: "agent:rule-b:codex:wsl:task-b:timeout" });
    await pendingB.promise;
  });
  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("acknowledgeReminderNavigation", { sequence: 21 }));
});

test.each([
  { sourceKind: "todo", sourceEntityId: "todo-1" },
  { sourceKind: "monitor", sourceEntityId: "threshold-1" },
])("does not acknowledge an unknown $sourceKind reminder context", async (pending) => {
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve(INITIAL_STATE);
    if (command === "get_pending_tray_navigation") return Promise.resolve(null);
    if (command === "getPendingReminderNavigation") return Promise.resolve({ sequence: 9, deliveryId: "d-9", ...pending });
    return Promise.resolve(undefined);
  });
  renderShell();
  await waitFor(() => expect(listenMock).toHaveBeenCalledWith("reminderNavigationRequested", expect.any(Function)));
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(invokeMock).not.toHaveBeenCalledWith("acknowledgeReminderNavigation", { sequence: 9 });
});

const INITIAL_STATE = {
  mode: "expanded" as const,
  scale: 1,
  dpi: 96,
  expandedHeight: 306,
  tucked: false,
  rasterizationError: null,
};

function renderShell() {
  return render(
    <I18nProvider>
      <IslandShell />
    </I18nProvider>,
  );
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
}

test("hides the expanded island to the Windows tray without closing the application", async () => {
  const user = userEvent.setup();
  renderShell();

  const minimize = await screen.findByRole("button", { name: "最小化到系统托盘" });
  await user.click(minimize);

  expect(invokeMock).toHaveBeenCalledWith("hide_island_to_tray");
  expect(invokeMock).not.toHaveBeenCalledWith("set_island_mode", expect.anything());
});

test("does not minimize while a native mode transition can still reshow the window", async () => {
  const nativeExpand = deferred<void>();
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") {
      return Promise.resolve({ ...INITIAL_STATE, mode: "collapsed" as const });
    }
    if (command === "get_pending_tray_navigation" || command === "getPendingReminderNavigation") {
      return Promise.resolve(null);
    }
    if (command === "set_island_mode") return nativeExpand.promise;
    return Promise.resolve(undefined);
  });
  const user = userEvent.setup();
  const { container } = renderShell();
  const canvas = container.querySelector<HTMLElement>(".island-canvas");
  await waitFor(() => expect(screen.getByRole("button", { name: "展开" })).toBeEnabled());

  fireEvent.pointerEnter(canvas!);
  await act(async () => new Promise((resolve) => setTimeout(resolve, 170)));
  const minimize = await screen.findByRole("button", { name: "最小化到系统托盘" });
  expect(minimize).toBeDisabled();
  await user.click(minimize);
  expect(invokeMock).not.toHaveBeenCalledWith("hide_island_to_tray");

  await act(async () => {
    nativeExpand.resolve();
    await nativeExpand.promise;
  });
  await waitFor(() => expect(minimize).toBeEnabled());
});

beforeEach(() => {
  beginAgentProfileStateSubscriptionMock.mockReturnValue({
    ready: Promise.resolve({ initial: { profiles: [], generatedAt: 0 }, dispose: vi.fn() }),
    dispose: vi.fn(),
  });
  beginAgentStateSubscriptionMock.mockReturnValue({
    ready: Promise.resolve({ initial: { agents: [], generatedAt: 0 }, dispose: vi.fn() }),
    dispose: vi.fn(),
  });
  beginReminderDispatchSubscriptionMock.mockImplementation((options: { render: typeof renderReminderDeliveries }) => {
    renderReminderDeliveries = options.render;
    return {
      ready: Promise.resolve({ initial: [], lastDispatchSeq: 0, listenerState: "active", retry: vi.fn(), dispose: vi.fn() }),
      dispose: vi.fn(),
    };
  });
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve(INITIAL_STATE);
    if (command === "get_pending_tray_navigation") return Promise.resolve(null);
    if (command === "getPendingReminderNavigation") return Promise.resolve(null);
    return Promise.resolve(undefined);
  });
  listenMock.mockResolvedValue(vi.fn());
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  invokeMock.mockReset();
  listenMock.mockReset();
  beginAgentProfileStateSubscriptionMock.mockReset();
  beginAgentStateSubscriptionMock.mockReset();
  beginReminderDispatchSubscriptionMock.mockReset();
  renderReminderDeliveries = undefined;
  vi.restoreAllMocks();
});

test("hover expands the compact island while double-click pins it until the app loses focus", async () => {
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve({ ...INITIAL_STATE, mode: "collapsed" as const });
    if (command === "get_pending_tray_navigation") return Promise.resolve(null);
    if (command === "getPendingReminderNavigation") return Promise.resolve(null);
    return Promise.resolve(undefined);
  });
  const { container } = renderShell();
  const canvas = container.querySelector<HTMLElement>(".island-canvas");
  const viewport = container.querySelector<HTMLElement>(".island-viewport");
  expect(canvas).not.toBeNull();
  expect(viewport).toHaveStyle({ "--island-window-radius": "23px" });
  await waitFor(() => expect(screen.getByRole("button", { name: "展开" })).toBeEnabled());

  fireEvent.pointerEnter(canvas!);
  await act(async () => new Promise((resolve) => setTimeout(resolve, 170)));
  await waitFor(() => expect(canvas).toHaveClass("island-canvas--expanded"));
  expect(screen.getByRole("img", { name: "AIsland" })).toBeVisible();
  expect(viewport).toHaveClass("island-viewport--expanded");
  expect(viewport).toHaveStyle({
    "--island-window-radius": "24px",
    "--island-compact-width": "248px",
    "--island-compact-height": "46px",
  });

  fireEvent.pointerLeave(canvas!);
  await act(async () => new Promise((resolve) => setTimeout(resolve, 320)));
  await waitFor(() => expect(canvas).toHaveClass("island-canvas--collapsed"));

  fireEvent.pointerEnter(canvas!);
  await act(async () => new Promise((resolve) => setTimeout(resolve, 170)));
  await waitFor(() => expect(canvas).toHaveClass("island-canvas--expanded"));
  fireEvent.doubleClick(canvas!);
  fireEvent.pointerLeave(canvas!);
  await act(async () => new Promise((resolve) => setTimeout(resolve, 320)));
  expect(canvas).toHaveClass("island-canvas--expanded");

  act(() => window.dispatchEvent(new Event("blur")));
  await waitFor(() => expect(canvas).toHaveClass("island-canvas--collapsed"));
});

test("renders the target visual mode while native animation is pending and rolls back on failure", async () => {
  vi.spyOn(console, "error").mockImplementation(() => undefined);
  const nativeExpand = deferred<void>();
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve({ ...INITIAL_STATE, mode: "collapsed" as const });
    if (command === "get_pending_tray_navigation" || command === "getPendingReminderNavigation") return Promise.resolve(null);
    if (command === "set_island_mode") return nativeExpand.promise;
    return Promise.resolve(undefined);
  });
  const { container } = renderShell();
  const canvas = container.querySelector<HTMLElement>(".island-canvas");
  await waitFor(() => expect(screen.getByRole("button", { name: "展开" })).toBeEnabled());

  fireEvent.pointerEnter(canvas!);
  await act(async () => new Promise((resolve) => setTimeout(resolve, 170)));
  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("set_island_mode", { mode: "expanded", motion: "elastic" }));
  expect(canvas).toHaveClass("island-canvas--expanded");

  await act(async () => {
    nativeExpand.reject(new Error("native animation failed"));
    await nativeExpand.promise.catch(() => undefined);
  });
  await waitFor(() => expect(canvas).toHaveClass("island-canvas--collapsed"));
  expect(screen.getByRole("button", { name: "展开" })).toBeEnabled();
});

test("keeps the latest visual mode and clears pending when a superseded native animation fails", async () => {
  vi.spyOn(console, "error").mockImplementation(() => undefined);
  const nativeExpand = deferred<void>();
  const nativeCollapse = deferred<void>();
  invokeMock.mockImplementation((command: string, args?: { mode?: string }) => {
    if (command === "get_initial_state") return Promise.resolve({ ...INITIAL_STATE, mode: "collapsed" as const });
    if (command === "get_pending_tray_navigation" || command === "getPendingReminderNavigation") return Promise.resolve(null);
    if (command === "set_island_mode" && args?.mode === "expanded") return nativeExpand.promise;
    if (command === "set_island_mode" && args?.mode === "collapsed") return nativeCollapse.promise;
    return Promise.resolve(undefined);
  });
  const { container } = renderShell();
  const canvas = container.querySelector<HTMLElement>(".island-canvas");
  await waitFor(() => expect(screen.getByRole("button", { name: "展开" })).toBeEnabled());

  fireEvent.pointerEnter(canvas!);
  await act(async () => new Promise((resolve) => setTimeout(resolve, 170)));
  await waitFor(() => expect(canvas).toHaveClass("island-canvas--expanded"));
  fireEvent.pointerLeave(canvas!);
  await act(async () => new Promise((resolve) => setTimeout(resolve, 320)));
  expect(canvas).toHaveClass("island-canvas--collapsed");
  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("set_island_mode", { mode: "collapsed", motion: "elastic" }));

  await act(async () => {
    nativeExpand.reject(new Error("superseded native animation failed"));
    await nativeExpand.promise.catch(() => undefined);
  });
  expect(canvas).toHaveClass("island-canvas--collapsed");
  await act(async () => {
    nativeCollapse.resolve();
    await nativeCollapse.promise;
  });
  await waitFor(() => expect(screen.getByRole("button", { name: "展开" })).toBeEnabled());
});

test("recovers the authoritative native mode when a newer animation fails after an older success", async () => {
  vi.spyOn(console, "error").mockImplementation(() => undefined);
  const nativeExpand = deferred<void>();
  const nativeCollapse = deferred<void>();
  let initialStateReads = 0;
  invokeMock.mockImplementation((command: string, args?: { mode?: string }) => {
    if (command === "get_initial_state") {
      initialStateReads += 1;
      return Promise.resolve({
        ...INITIAL_STATE,
        mode: initialStateReads === 1 ? "collapsed" as const : "expanded" as const,
      });
    }
    if (command === "get_pending_tray_navigation" || command === "getPendingReminderNavigation") return Promise.resolve(null);
    if (command === "set_island_mode" && args?.mode === "expanded") return nativeExpand.promise;
    if (command === "set_island_mode" && args?.mode === "collapsed") return nativeCollapse.promise;
    return Promise.resolve(undefined);
  });
  const { container } = renderShell();
  const canvas = container.querySelector<HTMLElement>(".island-canvas");
  await waitFor(() => expect(screen.getByRole("button", { name: /展开|Expand/i })).toBeEnabled());

  fireEvent.pointerEnter(canvas!);
  await act(async () => new Promise((resolve) => setTimeout(resolve, 170)));
  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("set_island_mode", { mode: "expanded", motion: "elastic" }));
  fireEvent.pointerLeave(canvas!);
  await act(async () => new Promise((resolve) => setTimeout(resolve, 320)));
  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("set_island_mode", { mode: "collapsed", motion: "elastic" }));

  await act(async () => {
    nativeExpand.resolve();
    await nativeExpand.promise;
  });
  expect(canvas).toHaveClass("island-canvas--collapsed");

  await act(async () => {
    nativeCollapse.reject(new Error("latest native animation failed"));
    await nativeCollapse.promise.catch(() => undefined);
  });
  await waitFor(() => expect(initialStateReads).toBe(2));
  await waitFor(() => expect(canvas).toHaveClass("island-canvas--expanded"));
});

test("a fresh Agent notification expands the compact island only when notification pop-ups are enabled", async () => {
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve({ ...INITIAL_STATE, mode: "collapsed" as const });
    if (command === "get_pending_tray_navigation") return Promise.resolve(null);
    if (command === "getPendingReminderNavigation") return Promise.resolve(null);
    return Promise.resolve(undefined);
  });
  const { container } = renderShell();
  await waitFor(() => expect(renderReminderDeliveries).toBeTypeOf("function"));
  const now = Date.now();
  await act(async () => renderReminderDeliveries?.([{
    id: "delivery-1",
    dedupeKey: "agent-1",
    ruleId: "rule-1",
    sourceKind: "agent",
    sourceEntityId: "agent:rule-1:codex:windows:task-1:completed",
    messageKey: "reminders.agent.status",
    messageParameters: { agentName: "Codex", environment: "windows", taskId: "task-1", taskTitle: "Release", triggerStatus: "completed" },
    sourceContext: { kind: "agent", agentId: "codex", environment: "windows", taskId: "task-1", taskTitle: "Release", triggerStatus: "completed", sourceEventId: "event-1", sourceOccurredAt: now },
    sourceOccurredAt: now,
    sound: { kind: "none" },
    state: "dispatched",
    dueAt: now,
    dispatchSeq: 1,
    firstDispatchedAt: now,
    lastDispatchedAt: now,
    acknowledgedAt: null,
    completedAt: null,
    snoozedUntil: null,
    createdAt: now,
    updatedAt: now,
  }]));

  await waitFor(() => expect(container.querySelector(".island-canvas")).toHaveClass("island-canvas--expanded"));
  expect(screen.getByRole("status")).toHaveTextContent("Release");
});

test("keeps non-Agent reminder deliveries out of the island Agent notification surface", async () => {
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve({ ...INITIAL_STATE, mode: "collapsed" as const });
    if (command === "get_pending_tray_navigation" || command === "getPendingReminderNavigation") return Promise.resolve(null);
    return Promise.resolve(undefined);
  });
  const { container } = renderShell();
  await waitFor(() => expect(renderReminderDeliveries).toBeTypeOf("function"));
  const now = Date.now();
  await act(async () => renderReminderDeliveries?.([{
    id: "todo-delivery", dedupeKey: "todo-1", ruleId: null, sourceKind: "todo", sourceEntityId: "todo-1",
    messageKey: "reminders.todo.due", messageParameters: { todoTitle: "Do not show here" },
    sourceContext: { kind: "todo", todoId: "todo-1", reminderRevision: 1, todoTitle: "Do not show here", sourceOccurredAt: now },
    sourceOccurredAt: now, sound: { kind: "none" }, state: "dispatched", dueAt: now, dispatchSeq: 2,
    firstDispatchedAt: now, lastDispatchedAt: now, acknowledgedAt: null, completedAt: null,
    snoozedUntil: null, createdAt: now, updatedAt: now,
  }]));

  expect(container.querySelector(".island-canvas")).toHaveClass("island-canvas--collapsed");
  expect(screen.queryByText("Do not show here")).not.toBeInTheDocument();
});

test("keeps the compact island collapsed when notification pop-ups are disabled", async () => {
  localStorage.setItem("aisland.notifications.popup.v1", "false");
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve({ ...INITIAL_STATE, mode: "collapsed" as const });
    if (command === "get_pending_tray_navigation") return Promise.resolve(null);
    if (command === "getPendingReminderNavigation") return Promise.resolve(null);
    return Promise.resolve(undefined);
  });
  const { container } = renderShell();
  await waitFor(() => expect(renderReminderDeliveries).toBeTypeOf("function"));
  const now = Date.now();
  await act(async () => renderReminderDeliveries?.([{
    id: "delivery-2", dedupeKey: "agent-2", ruleId: "rule-1", sourceKind: "agent", sourceEntityId: "agent:rule-1:codex:windows:task-2:completed",
    messageKey: "reminders.agent.status", messageParameters: { agentName: "Codex", environment: "windows", taskId: "task-2", taskTitle: "Hidden", triggerStatus: "completed" },
    sourceContext: { kind: "agent", agentId: "codex", environment: "windows", taskId: "task-2", taskTitle: "Hidden", triggerStatus: "completed", sourceEventId: "event-2", sourceOccurredAt: now },
    sourceOccurredAt: now, sound: { kind: "none" }, state: "dispatched", dueAt: now, dispatchSeq: 2,
    firstDispatchedAt: now, lastDispatchedAt: now, acknowledgedAt: null, completedAt: null,
    snoozedUntil: null, createdAt: now, updatedAt: now,
  }]));

  expect(container.querySelector(".island-canvas")).toHaveClass("island-canvas--collapsed");
  expect(screen.queryByText("Hidden")).not.toBeInTheDocument();
});

test("loads authoritative Windows notification content and expands only after the history invalidation", async () => {
  let notificationHistoryChanged: ((event: { payload: { newestReceivedAt: number; origin: "windows" | "aisland" } }) => void) | undefined;
  const now = Date.now();
  listenMock.mockImplementation((eventName: string, handler: typeof notificationHistoryChanged) => {
    if (eventName === "notificationHistoryChanged") notificationHistoryChanged = handler;
    return Promise.resolve(vi.fn());
  });
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve({ ...INITIAL_STATE, mode: "collapsed" as const });
    if (command === "get_pending_tray_navigation") return Promise.resolve(null);
    if (command === "getPendingReminderNavigation") return Promise.resolve(null);
    if (command === "listNotificationHistory") return Promise.resolve([{
      id: "notification-1",
      origin: "windows",
      appId: "com.example.mail",
      sourceEntityId: "mail-42",
      title: "New message",
      body: "The build has finished.",
      messageKey: null,
      messageParameters: {},
      sourceContext: null,
      sourceOccurredAt: now,
      receivedAt: now,
      readAt: null,
    }]);
    return Promise.resolve(undefined);
  });

  const { container } = renderShell();
  await waitFor(() => expect(notificationHistoryChanged).toBeTypeOf("function"));
  expect(container.querySelector(".island-canvas")).toHaveClass("island-canvas--collapsed");

  await act(async () => notificationHistoryChanged?.({ payload: { newestReceivedAt: now, origin: "windows" } }));

  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("listNotificationHistory", {
    origin: "windows", sourceApp: null, unreadOnly: false, limit: 1,
  }));
  await waitFor(() => expect(container.querySelector(".island-canvas")).toHaveClass("island-canvas--expanded"));
  expect(screen.getByRole("status")).toHaveTextContent("New message");
  expect(screen.getByRole("status")).toHaveTextContent("The build has finished.");
});

test("coalesces burst Windows notification invalidations and displays only the latest authoritative row", async () => {
  const first = deferred<import("../api/contracts").NotificationHistoryItem[]>();
  const second = deferred<import("../api/contracts").NotificationHistoryItem[]>();
  let notificationHistoryChanged: ((event: { payload: { newestReceivedAt: number; origin: "windows" | "aisland" } }) => void) | undefined;
  let historyQueries = 0;
  const now = Date.now();
  const historyItem = (id: string, title: string, receivedAt: number): import("../api/contracts").NotificationHistoryItem => ({
    id, origin: "windows", appId: "com.example.mail", sourceEntityId: id, title, body: `${title} body`,
    messageKey: null, messageParameters: {}, sourceContext: null, sourceOccurredAt: receivedAt, receivedAt, readAt: null,
  });
  listenMock.mockImplementation((eventName: string, handler: typeof notificationHistoryChanged) => {
    if (eventName === "notificationHistoryChanged") notificationHistoryChanged = handler;
    return Promise.resolve(vi.fn());
  });
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve({ ...INITIAL_STATE, mode: "collapsed" as const });
    if (command === "get_pending_tray_navigation" || command === "getPendingReminderNavigation") return Promise.resolve(null);
    if (command === "listNotificationHistory") {
      historyQueries += 1;
      return historyQueries === 1 ? first.promise : second.promise;
    }
    return Promise.resolve(undefined);
  });
  renderShell();
  await waitFor(() => expect(notificationHistoryChanged).toBeTypeOf("function"));

  act(() => {
    notificationHistoryChanged?.({ payload: { newestReceivedAt: now, origin: "windows" } });
    notificationHistoryChanged?.({ payload: { newestReceivedAt: now, origin: "windows" } });
  });
  await waitFor(() => expect(historyQueries).toBe(1));

  await act(async () => {
    first.resolve([historyItem("old", "Older message", now)]);
    await first.promise;
  });
  await waitFor(() => expect(historyQueries).toBe(2));
  expect(screen.queryByText("Older message")).not.toBeInTheDocument();

  await act(async () => {
    second.resolve([historyItem("latest", "Latest message", now)]);
    await second.promise;
  });
  expect(await screen.findByRole("status")).toHaveTextContent("Latest message");
  expect(screen.queryByText("Older message")).not.toBeInTheDocument();
});

test("does not display or expand for a Windows notification history result that resolves after unmount", async () => {
  const history = deferred<import("../api/contracts").NotificationHistoryItem[]>();
  const unlisten = vi.fn();
  let notificationHistoryChanged: ((event: { payload: { newestReceivedAt: number; origin: "windows" | "aisland" } }) => void) | undefined;
  const now = Date.now();
  listenMock.mockImplementation((eventName: string, handler: typeof notificationHistoryChanged) => {
    if (eventName === "notificationHistoryChanged") {
      notificationHistoryChanged = handler;
      return Promise.resolve(unlisten);
    }
    return Promise.resolve(vi.fn());
  });
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve({ ...INITIAL_STATE, mode: "collapsed" as const });
    if (command === "get_pending_tray_navigation" || command === "getPendingReminderNavigation") return Promise.resolve(null);
    if (command === "listNotificationHistory") return history.promise;
    return Promise.resolve(undefined);
  });
  const view = renderShell();
  await waitFor(() => expect(notificationHistoryChanged).toBeTypeOf("function"));
  act(() => {
    notificationHistoryChanged?.({ payload: { newestReceivedAt: now, origin: "windows" } });
  });
  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("listNotificationHistory", {
    origin: "windows", sourceApp: null, unreadOnly: false, limit: 1,
  }));

  view.unmount();
  await act(async () => {
    history.resolve([{
      id: "late", origin: "windows", appId: "com.example.mail", sourceEntityId: "late", title: "Too late", body: "Hidden",
      messageKey: null, messageParameters: {}, sourceContext: null, sourceOccurredAt: now, receivedAt: now, readAt: null,
    }]);
    await history.promise;
  });
  expect(invokeMock.mock.calls.filter(([command]) => command === "set_island_mode")).toHaveLength(0);
  expect(unlisten).toHaveBeenCalledTimes(1);
});

test("does not query or expand for Windows notification invalidations when notification pop-ups are disabled", async () => {
  localStorage.setItem("aisland.notifications.popup.v1", "false");
  let notificationHistoryChanged: ((event: { payload: { newestReceivedAt: number; origin: "windows" | "aisland" } }) => void) | undefined;
  listenMock.mockImplementation((eventName: string, handler: typeof notificationHistoryChanged) => {
    if (eventName === "notificationHistoryChanged") notificationHistoryChanged = handler;
    return Promise.resolve(vi.fn());
  });
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve({ ...INITIAL_STATE, mode: "collapsed" as const });
    if (command === "get_pending_tray_navigation") return Promise.resolve(null);
    if (command === "getPendingReminderNavigation") return Promise.resolve(null);
    return Promise.resolve(undefined);
  });

  const { container } = renderShell();
  await waitFor(() => expect(notificationHistoryChanged).toBeTypeOf("function"));
  await act(async () => notificationHistoryChanged?.({ payload: { newestReceivedAt: Date.now(), origin: "windows" } }));

  expect(invokeMock.mock.calls.some(([command]) => command === "listNotificationHistory")).toBe(false);
  expect(container.querySelector(".island-canvas")).toHaveClass("island-canvas--collapsed");
});

test("mounts the real monitor page branch instead of the generic preview", async () => {
  const user = userEvent.setup();
  renderShell();
  await waitFor(() => expect(screen.getByRole("tab", { name: "系统监控" })).toBeEnabled());
  await user.click(screen.getByRole("tab", { name: "系统监控" }));
  expect(screen.getByRole("region", { name: "monitor-page" })).toHaveTextContent("Monitor surface");
  expect(screen.queryByText("系统监控", { selector: ".page-preview" })).not.toBeInTheDocument();
});

test("mounts the notification center branch instead of the generic preview", async () => {
  const user = userEvent.setup();
  renderShell();
  await waitFor(() => expect(screen.getByRole("tab", { name: "通知中心" })).toBeEnabled());
  await user.click(screen.getByRole("tab", { name: "通知中心" }));
  expect(screen.getByRole("region", { name: "notification-center-page" })).toHaveTextContent("Notification surface");
  expect(screen.queryByText("通知中心", { selector: ".page-preview" })).not.toBeInTheDocument();
});

test("opens any compact Agent logo into expanded Home with its selected task context", async () => {
  // Leaving a compact logo selection as a highlight rather than a Home detail must fail this integration contract.
  beginAgentStateSubscriptionMock.mockReturnValue({
    ready: Promise.resolve({
      initial: {
        generatedAt: 1,
        agents: [
          { agentId: "codex", displayName: "Codex", aggregateStatus: "failed", integrations: [], environments: [{ agentId: "codex", environment: "windows", taskId: "codex-task", status: "failed", summary: "", sourceEventId: "c", occurredAt: 4, receivedAt: 4 }] },
          { agentId: "hermes", displayName: "Hermes", aggregateStatus: "running", integrations: [], environments: [{ agentId: "hermes", environment: "windows", taskId: "hermes-task", status: "running", summary: "", sourceEventId: "h", occurredAt: 3, receivedAt: 3 }] },
          { agentId: "workbuddy", displayName: "WorkBuddy", aggregateStatus: "running", integrations: [], environments: [{ agentId: "workbuddy", environment: "windows", taskId: "work-task", status: "running", summary: "", sourceEventId: "w", occurredAt: 2, receivedAt: 2 }] },
          { agentId: "claude", displayName: "claude", aggregateStatus: "idle", integrations: [], environments: [{ agentId: "claude", environment: "windows", taskId: "claude-task", status: "idle", summary: "", sourceEventId: "l", occurredAt: 1, receivedAt: 1 }] },
        ],
      },
      dispose: vi.fn(),
    }),
    dispose: vi.fn(),
  });
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve({ ...INITIAL_STATE, mode: "collapsed" as const });
    if (command === "get_pending_tray_navigation") return Promise.resolve(null);
    if (command === "getPendingReminderNavigation") return Promise.resolve(null);
    return Promise.resolve(undefined);
  });
  const user = userEvent.setup();
  renderShell();

  await user.click(await screen.findByRole("button", { name: /^claude/ }));
  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("set_island_mode", { mode: "expanded", motion: "elastic" }));
  expect(screen.getByRole("region", { name: "claude" })).toHaveTextContent("claude-task");
});

test("re-entering Settings through its selected tab resets a nested settings route", async () => {
  const user = userEvent.setup();
  renderShell();

  await waitFor(() => {
    expect(screen.getByRole("tab", { name: "设置" })).toBeEnabled();
  });
  await user.click(screen.getByRole("tab", { name: "设置" }));
  await user.click(screen.getByRole("button", { name: "通用" }));
  expect(screen.getByRole("heading", { name: "通用" })).toBeInTheDocument();

  await user.click(screen.getByRole("tab", { name: "设置" }));
  expect(screen.getByRole("button", { name: "通用" })).toBeInTheDocument();
});

test("keeps Todo retired while Daily Notes remains mounted without touching geometry", async () => {
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve(INITIAL_STATE);
    if (command === "get_pending_tray_navigation") return Promise.resolve(null);
    if (command === "getPendingReminderNavigation") return Promise.resolve(null);
    if (command === "listNotes") return Promise.resolve([]);
    if (command === "getDailyNote") return Promise.resolve(null);
    return Promise.resolve(undefined);
  });
  const user = userEvent.setup();
  renderShell();
  await waitFor(() => expect(screen.getByRole("tab", { name: "每日笔记" })).toBeEnabled());
  expect(screen.queryByRole("tab", { name: "待办" })).not.toBeInTheDocument();
  const geometryCallsBefore = invokeMock.mock.calls.filter(([command]) => command === "set_island_scale" || command === "set_island_expanded_height").length;

  await user.click(screen.getByRole("tab", { name: "每日笔记" }));
  expect(await screen.findByRole("heading", { name: "每日笔记" })).toBeVisible();
  expect(screen.getByRole("textbox", { name: "每日笔记" })).toBeVisible();
  expect(invokeMock.mock.calls.some(([command]) => command === "listTodos" || command === "listTodoReminders")).toBe(false);
  expect(invokeMock.mock.calls.filter(([command]) => command === "set_island_scale" || command === "set_island_expanded_height")).toHaveLength(geometryCallsBefore);
});

test("keeps Media retired while the real Clipboard page remains mounted", async () => {
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve(INITIAL_STATE);
    if (command === "get_pending_tray_navigation") return Promise.resolve(null);
    if (command === "getPendingReminderNavigation") return Promise.resolve(null);
    if (command === "listClipboardItems") return Promise.resolve([]);
    return Promise.resolve(undefined);
  });
  listenMock.mockResolvedValue(vi.fn());
  const user = userEvent.setup();
  renderShell();
  await waitFor(() => expect(screen.getByRole("tab", { name: "剪贴板" })).toBeEnabled());

  await user.click(screen.getByRole("tab", { name: "剪贴板" }));
  expect(await screen.findByRole("heading", { name: "剪贴板" })).toBeVisible();
  expect(screen.queryByRole("tab", { name: "媒体" })).not.toBeInTheDocument();
  expect(invokeMock.mock.calls.some(([command]) => command === "getMediaSnapshot")).toBe(false);
});

test("keeps a pending daily-note draft alive while another shell tab is active", async () => {
  // Unmounting the note surface on tab changes must fail this mounted-session draft contract.
  vi.useFakeTimers({ shouldAdvanceTime: true });
  const create = deferred<{ id: string; noteDate: string; bodyMarkdown: string; revision: number; createdAt: number; updatedAt: number }>();
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve(INITIAL_STATE);
    if (command === "get_pending_tray_navigation" || command === "getPendingReminderNavigation") return Promise.resolve(null);
    if (command === "getDailyNote") return Promise.resolve(null);
    if (command === "createNote") return create.promise;
    return Promise.resolve(undefined);
  });
  renderShell();
  await waitFor(() => expect(screen.getByRole("tab", { name: "每日笔记" })).toBeEnabled());

  fireEvent.click(screen.getByRole("tab", { name: "每日笔记" }));
  const editor = await screen.findByRole("textbox", { name: "每日笔记" });
  fireEvent.change(editor, { target: { value: "kept while hidden" } });
  fireEvent.click(screen.getByRole("tab", { name: "主页" }));
  await act(async () => { vi.advanceTimersByTime(600); await Promise.resolve(); });

  expect(invokeMock).toHaveBeenCalledWith("createNote", expect.objectContaining({ bodyMarkdown: "kept while hidden" }));
  fireEvent.click(screen.getByRole("tab", { name: "每日笔记" }));
  expect(screen.getByRole("textbox", { name: "每日笔记" })).toHaveValue("kept while hidden");

  await act(async () => {
    create.resolve({ id: "note-kept", noteDate: "2026-08-13", bodyMarkdown: "kept while hidden", revision: 1, createdAt: 1, updatedAt: 2 });
    await create.promise;
  });
  vi.useRealTimers();
});

test("keeps a failed daily-note draft across tab changes and shell collapse", async () => {
  // A failed draft is session state and must remain retryable even while the note surface is hidden.
  vi.useFakeTimers({ shouldAdvanceTime: true });
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve(INITIAL_STATE);
    if (command === "get_pending_tray_navigation" || command === "getPendingReminderNavigation") return Promise.resolve(null);
    if (command === "getDailyNote") return Promise.resolve(null);
    if (command === "createNote") return Promise.reject({ code: "databaseFailure", messageKey: "errors.databaseFailure", details: { reasonCode: "failed" }, retryable: true });
    return Promise.resolve(undefined);
  });
  renderShell();
  await waitFor(() => expect(screen.getByRole("tab", { name: "每日笔记" })).toBeEnabled());

  fireEvent.click(screen.getByRole("tab", { name: "每日笔记" }));
  fireEvent.change(await screen.findByRole("textbox", { name: "每日笔记" }), { target: { value: "retry after returning" } });
  fireEvent.click(screen.getByRole("tab", { name: "主页" }));
  await act(async () => { vi.advanceTimersByTime(600); await Promise.resolve(); await Promise.resolve(); });
  fireEvent.click(screen.getByRole("button", { name: "折叠" }));
  await waitFor(() => expect(screen.getByRole("button", { name: "展开" })).toBeEnabled());
  expect(screen.queryByRole("textbox", { name: "每日笔记" })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "展开" }));
  await waitFor(() => expect(screen.getByRole("tab", { name: "每日笔记" })).toBeEnabled());
  fireEvent.click(screen.getByRole("tab", { name: "每日笔记" }));

  expect(screen.getByRole("textbox", { name: "每日笔记" })).toHaveValue("retry after returning");
  expect(screen.getByRole("alert")).toHaveTextContent("自动保存失败，编辑内容仍保留在此窗口");
  expect(screen.getByRole("button", { name: "重试" })).toBeVisible();
  vi.useRealTimers();
});

test("keeps the confirmed scale active until the native scale transaction succeeds", async () => {
  const nativeScale = deferred<void>();
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve(INITIAL_STATE);
    if (command === "get_pending_tray_navigation") return Promise.resolve(null);
    if (command === "getPendingReminderNavigation") return Promise.resolve(null);
    if (command === "set_island_scale") return nativeScale.promise;
    return Promise.resolve(undefined);
  });
  const user = userEvent.setup();
  renderShell();

  await waitFor(() => {
    expect(screen.getByRole("tab", { name: "设置" })).toBeEnabled();
  });
  await user.click(screen.getByRole("tab", { name: "设置" }));
  await user.click(screen.getByRole("button", { name: "显示与外观" }));
  await user.click(screen.getByRole("button", { name: "大" }));

  expect(screen.getByRole("button", { name: "中" })).toHaveAttribute("aria-pressed", "true");
  expect(screen.getByRole("button", { name: "大" })).toHaveAttribute("aria-pressed", "false");

  await act(async () => {
    nativeScale.resolve();
    await nativeScale.promise;
  });

  expect(screen.getByRole("button", { name: "大" })).toHaveAttribute("aria-pressed", "true");
});

test("keeps the prior confirmed scale active when the native scale transaction rejects", async () => {
  vi.spyOn(console, "error").mockImplementation(() => undefined);
  const nativeScale = deferred<void>();
  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return Promise.resolve(INITIAL_STATE);
    if (command === "get_pending_tray_navigation") return Promise.resolve(null);
    if (command === "set_island_scale") return nativeScale.promise;
    return Promise.resolve(undefined);
  });
  const user = userEvent.setup();
  renderShell();

  await waitFor(() => {
    expect(screen.getByRole("tab", { name: "设置" })).toBeEnabled();
  });
  await user.click(screen.getByRole("tab", { name: "设置" }));
  await user.click(screen.getByRole("button", { name: "显示与外观" }));
  await user.click(screen.getByRole("button", { name: "大" }));

  await act(async () => {
    nativeScale.reject(new Error("native rejected scale"));
    await nativeScale.promise.catch(() => undefined);
  });

  expect(screen.getByRole("button", { name: "中" })).toHaveAttribute("aria-pressed", "true");
  expect(screen.getByRole("button", { name: "大" })).toHaveAttribute("aria-pressed", "false");
});

test("restores and persists glass transparency while updating the shell material", async () => {
  localStorage.setItem("aisland.display.glassTransparency.v1", "86");
  const user = userEvent.setup();
  const { container } = renderShell();

  await waitFor(() => {
    expect(screen.getByRole("tab", { name: "设置" })).toBeEnabled();
  });
  const canvas = container.querySelector<HTMLElement>(".island-canvas");
  expect(canvas).toHaveAttribute("data-glass-transparency", "86");
  expect(canvas?.style.getPropertyValue("--glass-shell-alpha")).toBe("0.14");

  await user.click(screen.getByRole("tab", { name: "设置" }));
  await user.click(screen.getByRole("button", { name: "显示与外观" }));
  fireEvent.change(screen.getByRole("slider", { name: "玻璃透明度" }), { target: { value: "100" } });

  expect(canvas).toHaveAttribute("data-glass-transparency", "100");
  expect(canvas?.style.getPropertyValue("--glass-shell-alpha")).toBe("0");
  expect(canvas?.style.getPropertyValue("--glass-panel-alpha")).toBe("0");
  expect(canvas?.style.getPropertyValue("--glass-popover-alpha")).toBe("0");
  expect(localStorage.getItem("aisland.display.glassTransparency.v1")).toBe("100");
  await waitFor(() => {
    expect(invokeMock).toHaveBeenCalledWith("set_island_glass_transparency", { transparency: 100 });
  });
});

test("restores and persists the selected production expansion motion", async () => {
  localStorage.setItem("aisland.display.expansionMotion.v1", "smooth");
  const user = userEvent.setup();
  const { container } = renderShell();

  await waitFor(() => expect(screen.getByRole("tab", { name: "设置" })).toBeEnabled());
  const viewport = container.querySelector<HTMLElement>(".island-viewport");
  expect(viewport).toHaveAttribute("data-expansion-motion", "smooth");

  await user.click(screen.getByRole("button", { name: "折叠" }));
  await waitFor(() => {
    expect(invokeMock).toHaveBeenCalledWith("set_island_mode", { mode: "collapsed", motion: "smooth" });
  });
  await user.click(screen.getByRole("button", { name: "展开" }));
  await waitFor(() => {
    expect(invokeMock).toHaveBeenCalledWith("set_island_mode", { mode: "expanded", motion: "smooth" });
  });
  await user.click(screen.getByRole("tab", { name: "设置" }));
  await user.click(screen.getByRole("button", { name: "显示与外观" }));
  await user.click(screen.getByRole("button", { name: "快速展开" }));

  expect(viewport).toHaveAttribute("data-expansion-motion", "swift");
  expect(localStorage.getItem("aisland.display.expansionMotion.v1")).toBe("swift");

  await user.click(screen.getByRole("button", { name: "折叠" }));
  await waitFor(() => {
    expect(invokeMock).toHaveBeenCalledWith("set_island_mode", { mode: "collapsed", motion: "swift" });
  });
});

test("confirms only the latest rapid scale selection after the single-flight requests settle", async () => {
  const firstNativeScale = deferred<void>();
  const secondNativeScale = deferred<void>();
  const requestedScales: number[] = [];
  invokeMock.mockImplementation((command: string, args?: { scale?: number }) => {
    if (command === "get_initial_state") return Promise.resolve(INITIAL_STATE);
    if (command === "get_pending_tray_navigation") return Promise.resolve(null);
    if (command === "getPendingReminderNavigation") return Promise.resolve(null);
    if (command === "set_island_scale") {
      requestedScales.push(args?.scale ?? Number.NaN);
      return requestedScales.length === 1 ? firstNativeScale.promise : secondNativeScale.promise;
    }
    return Promise.resolve(undefined);
  });
  const user = userEvent.setup();
  renderShell();

  await waitFor(() => {
    expect(screen.getByRole("tab", { name: "设置" })).toBeEnabled();
  });
  await user.click(screen.getByRole("tab", { name: "设置" }));
  await user.click(screen.getByRole("button", { name: "显示与外观" }));
  await user.click(screen.getByRole("button", { name: "大" }));
  await user.click(screen.getByRole("button", { name: "特大" }));

  expect(screen.getByRole("button", { name: "中" })).toHaveAttribute("aria-pressed", "true");
  expect(requestedScales).toEqual([1.15]);

  await act(async () => {
    firstNativeScale.resolve();
    await firstNativeScale.promise;
  });
  await waitFor(() => {
    expect(requestedScales).toEqual([1.15, 1.3]);
  });
  expect(screen.getByRole("button", { name: "中" })).toHaveAttribute("aria-pressed", "true");

  await act(async () => {
    secondNativeScale.resolve();
    await secondNativeScale.promise;
  });
  expect(screen.getByRole("button", { name: "特大" })).toHaveAttribute("aria-pressed", "true");
});

test("acknowledges a real pending diagnostics tray sequence once after returning its nested route to root", async () => {
  const nativeExpand = deferred<void>();
  const nativeAcknowledge = deferred<void>();
  const commands: string[] = [];
  const acknowledgementRoots: boolean[] = [];
  const acknowledgementSequences: number[] = [];
  let pendingNavigation: { page: "settings"; sequence: number } | null = null;
  let trayNavigateListener: ((event: { payload: string }) => void) | undefined;
  const initialWithTuck = { ...INITIAL_STATE, tucked: true };

  invokeMock.mockImplementation((command: string, args?: { sequence?: number }) => {
    commands.push(command);
    if (command === "get_initial_state") return Promise.resolve(initialWithTuck);
    if (command === "get_pending_tray_navigation") return Promise.resolve(pendingNavigation);
    if (command === "getPendingReminderNavigation") return Promise.resolve(null);
    if (command === "listServiceHealth") return Promise.resolve([]);
    if (command === "getDiagnostics") return Promise.resolve([]);
    if (command === "set_island_tucked") return Promise.resolve(undefined);
    if (command === "set_island_mode") return nativeExpand.promise;
    if (command === "acknowledge_tray_navigation") {
      acknowledgementRoots.push(screen.queryByRole("button", { name: "通用" }) !== null);
      acknowledgementSequences.push(args?.sequence ?? Number.NaN);
      return nativeAcknowledge.promise;
    }
    return Promise.resolve(undefined);
  });
  listenMock.mockImplementation(async (event: string, handler: (event: { payload: string }) => void) => {
    if (event === "tray-navigate") trayNavigateListener = handler;
    return vi.fn();
  });
  const user = userEvent.setup();
  renderShell();

  await waitFor(() => {
    expect(trayNavigateListener).toBeDefined();
  });
  await waitFor(() => {
    expect(screen.getByRole("tab", { name: "设置" })).toBeInTheDocument();
  });
  await user.click(screen.getByRole("tab", { name: "设置" }));
  await user.click(screen.getByRole("button", { name: "诊断" }));
  expect(screen.getByRole("heading", { name: "诊断" })).toBeInTheDocument();

  pendingNavigation = { page: "settings", sequence: 73 };
  await act(async () => {
    trayNavigateListener?.({ payload: "settings" });
    await Promise.resolve();
  });
  await waitFor(() => {
    expect(commands).toContain("set_island_mode");
  });
  expect(commands).toContain("set_island_tucked");
  expect(commands).not.toContain("acknowledge_tray_navigation");
  expect(screen.getByRole("heading", { name: "诊断" })).toBeInTheDocument();

  await act(async () => {
    nativeExpand.resolve();
    await nativeExpand.promise;
  });
  await waitFor(() => {
    expect(commands.filter((command) => command === "acknowledge_tray_navigation")).toHaveLength(1);
  });
  expect(screen.getByRole("button", { name: "通用" })).toBeInTheDocument();
  expect(acknowledgementRoots).toEqual([true]);
  expect(acknowledgementSequences).toEqual([73]);
  expect(commands.indexOf("acknowledge_tray_navigation")).toBeGreaterThan(commands.indexOf("set_island_mode"));

  await act(async () => {
    trayNavigateListener?.({ payload: "settings" });
    await Promise.resolve();
  });
  await waitFor(() => {
    expect(commands.filter((command) => command === "get_pending_tray_navigation").length).toBeGreaterThan(1);
  });
  expect(commands.filter((command) => command === "acknowledge_tray_navigation")).toHaveLength(1);

  await user.click(screen.getByRole("button", { name: "诊断" }));
  expect(screen.getByRole("heading", { name: "诊断" })).toBeInTheDocument();

  pendingNavigation = null;
  await act(async () => {
    nativeAcknowledge.resolve();
    await nativeAcknowledge.promise;
  });
  expect(screen.getByRole("heading", { name: "诊断" })).toBeInTheDocument();
});

test("treats a Settings tab click as a new route entry while tray acknowledgement is pending", async () => {
  const nativeAcknowledge = deferred<void>();
  const initialState = deferred<typeof INITIAL_STATE>();
  let pendingNavigation: { page: "settings"; sequence: number } | null = null;
  let trayNavigateListener: ((event: { payload: string }) => void) | undefined;

  invokeMock.mockImplementation((command: string) => {
    if (command === "get_initial_state") return initialState.promise;
    if (command === "get_pending_tray_navigation") return Promise.resolve(pendingNavigation);
    if (command === "getPendingReminderNavigation") return Promise.resolve(null);
    if (command === "acknowledge_tray_navigation") return nativeAcknowledge.promise;
    return Promise.resolve(undefined);
  });
  listenMock.mockImplementation(async (event: string, handler: (event: { payload: string }) => void) => {
    if (event === "tray-navigate") trayNavigateListener = handler;
    return vi.fn();
  });
  const user = userEvent.setup();
  renderShell();

  await waitFor(() => {
    expect(trayNavigateListener).toBeDefined();
  });
  await act(async () => {
    initialState.resolve(INITIAL_STATE);
    await initialState.promise;
  });
  await waitFor(() => {
    expect(screen.getByRole("tab", { name: "设置" })).toBeEnabled();
  });
  await user.click(screen.getByRole("tab", { name: "设置" }));
  await user.click(screen.getByRole("button", { name: "通用" }));

  pendingNavigation = { page: "settings", sequence: 91 };
  await act(async () => {
    trayNavigateListener?.({ payload: "settings" });
    await Promise.resolve();
  });
  await waitFor(() => {
    expect(invokeMock).toHaveBeenCalledWith("acknowledge_tray_navigation", { sequence: 91 });
  });
  expect(screen.getByRole("button", { name: "通用" })).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "通用" }));
  expect(screen.getByRole("heading", { name: "通用" })).toBeInTheDocument();
  await user.click(screen.getByRole("tab", { name: "设置" }));
  expect(screen.getByRole("button", { name: "通用" })).toBeInTheDocument();

  pendingNavigation = null;
  await act(async () => {
    nativeAcknowledge.resolve();
    await nativeAcknowledge.promise;
  });
});
