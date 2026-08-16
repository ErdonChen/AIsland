import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { I18nProvider } from "../i18n/I18nProvider";
import type { ReminderDelivery } from "../api/contracts";

const { beginReminderDispatchSubscriptionMock, acknowledgeMock, completeMock, snoozeMock, reloadGroupMock, hideMock } = vi.hoisted(() => ({
  beginReminderDispatchSubscriptionMock: vi.fn(), acknowledgeMock: vi.fn(), completeMock: vi.fn(), snoozeMock: vi.fn(), reloadGroupMock: vi.fn(), hideMock: vi.fn(),
}));

vi.mock("../api/events", () => ({ beginReminderDispatchSubscription: beginReminderDispatchSubscriptionMock }));
vi.mock("../api/commands", () => ({ acknowledgeReminder: acknowledgeMock, completeReminder: completeMock, snoozeReminder: snoozeMock, reloadReminderAlertGroup: reloadGroupMock }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ hide: hideMock }) }));

const agent = (id: string, dispatchSeq: number): ReminderDelivery => ({
  id, dedupeKey: id, ruleId: "rule-1", sourceKind: "agent", sourceEntityId: "agent:rule-1:codex:windows:task-7:failed",
  messageKey: "reminders.agent.status", messageParameters: { agentName: "Codex", environment: "windows", taskId: "task-7", taskTitle: "Release", triggerStatus: "failed" },
  sourceContext: { kind: "agent", agentId: "codex", environment: "windows", taskId: "task-7", taskTitle: "Release", triggerStatus: "failed", sourceEventId: `event-${id}`, sourceOccurredAt: dispatchSeq },
  sourceOccurredAt: dispatchSeq, sound: { kind: "none" }, state: "dispatched", dueAt: dispatchSeq, dispatchSeq, firstDispatchedAt: dispatchSeq, lastDispatchedAt: dispatchSeq, acknowledgedAt: null, completedAt: null, snoozedUntil: null, createdAt: dispatchSeq, updatedAt: dispatchSeq,
});

const todo = (id: string, dispatchSeq: number): ReminderDelivery => ({
  ...agent(id, dispatchSeq), ruleId: null, sourceKind: "todo", sourceEntityId: "todo-8", messageKey: "reminders.todo.due", messageParameters: { todoTitle: "Review release" },
  sourceContext: { kind: "todo", todoId: "todo-8", reminderRevision: 4, todoTitle: "Review release", sourceOccurredAt: dispatchSeq },
});

const monitor = (id: string, dispatchSeq: number): ReminderDelivery => ({
  ...agent(id, dispatchSeq), ruleId: null, sourceKind: "monitor", sourceEntityId: "threshold-8", messageKey: "reminders.monitor.threshold", messageParameters: { metric: "cpu", currentValue: 90, thresholdValue: 80 },
  sourceContext: { kind: "monitor", thresholdId: "threshold-8", metric: "cpuPercent", currentValue: 90, thresholdValue: 80, breachStartedAt: 4, sourceOccurredAt: dispatchSeq },
});

async function renderAlert(deliveries: ReminderDelivery[]) {
  beginReminderDispatchSubscriptionMock.mockImplementation(({ render: renderDeliveries }: { render: (items: ReminderDelivery[]) => Promise<void> }) => {
    void renderDeliveries(deliveries);
    return { ready: Promise.resolve({ initial: deliveries, dispose: vi.fn(), retry: vi.fn(), lastDispatchSeq: deliveries.at(-1)?.dispatchSeq ?? 0 }), dispose: vi.fn() };
  });
  const modulePath = "./ReminderAlertApp";
  const { ReminderAlertApp } = await import(/* @vite-ignore */ modulePath);
  render(<I18nProvider><ReminderAlertApp consumerId="reminder-alert-window" /></I18nProvider>);
}

function terminalGroup(delivery: ReminderDelivery, state: "completed" | "snoozed"): object {
  return { mergeKey: `agent:${delivery.sourceEntityId}`, mergeIdentity: { kind: "agent", ruleId: "rule-1", agentId: "codex", environment: "windows", taskId: "task-7", triggerStatus: "failed" }, members: [{ ...delivery, state }], sourceContext: delivery.sourceContext, newestSourceOccurredAt: delivery.sourceOccurredAt };
}

test("recognizes only the exact reminder-alert window label", async () => {
  const { isReminderAlertWindow } = await import(/* @vite-ignore */ "./ReminderAlertApp");
  expect(isReminderAlertWindow("reminder-alert")).toBe(true);
  expect(isReminderAlertWindow("main")).toBe(false);
  expect(isReminderAlertWindow("reminder-alert-window")).toBe(false);
});

test("hides the standalone alert window when the authoritative replay is empty", async () => {
  await renderAlert([]);

  await waitFor(() => expect(hideMock).toHaveBeenCalledTimes(1));
  expect(screen.queryByRole("article")).not.toBeInTheDocument();
});

test("replays the alert consumer first and merges only matching Agent source identities", async () => {
  // Removing listener-first replay, identity grouping, or source-context display must fail this alert boundary.
  await renderAlert([agent("delivery-2", 2), agent("delivery-1", 1)]);
  await waitFor(() => expect(beginReminderDispatchSubscriptionMock).toHaveBeenCalledWith(expect.objectContaining({ consumerId: "reminder-alert-window" })));
  expect(screen.getByRole("article", { name: /Codex.*Release/ })).toHaveTextContent("2");
  expect(screen.getByText(/Release/)).toBeInTheDocument();
  expect(document.querySelector("time")).toHaveAttribute("datetime", "2");
});

test("uses the newest source occurrence rather than delivery-id order for message, context, and time", async () => {
  const older = { ...agent("z-old", 1), messageParameters: { ...agent("z-old", 1).messageParameters, taskTitle: "Old task" }, sourceContext: { ...agent("z-old", 1).sourceContext, taskTitle: "Old task", sourceOccurredAt: 10 }, sourceOccurredAt: 10 };
  const newer = { ...agent("a-new", 2), messageParameters: { ...agent("a-new", 2).messageParameters, taskTitle: "Newest task" }, sourceContext: { ...agent("a-new", 2).sourceContext, taskTitle: "Newest task", sourceOccurredAt: 20 }, sourceOccurredAt: 20 };
  await renderAlert([older, newer]);
  expect(await screen.findByRole("article", { name: /Newest task/ })).toHaveTextContent("Newest task");
  expect(document.querySelector("time")).toHaveAttribute("datetime", "20");
});

test("uses the same highest delivery ID representative for equal-time message, context, and time regardless of insertion order", async () => {
  const alpha = { ...agent("a-alpha", 1), messageParameters: { ...agent("a-alpha", 1).messageParameters, taskTitle: "Alpha task" }, sourceContext: { ...agent("a-alpha", 1).sourceContext, taskTitle: "Alpha task", sourceOccurredAt: 20 }, sourceOccurredAt: 20 };
  const zulu = { ...agent("z-zulu", 2), messageParameters: { ...agent("z-zulu", 2).messageParameters, taskTitle: "Zulu task" }, sourceContext: { ...agent("z-zulu", 2).sourceContext, taskTitle: "Zulu task", sourceOccurredAt: 20 }, sourceOccurredAt: 20 };
  const yankee = { ...agent("y-yankee", 3), messageParameters: { ...agent("y-yankee", 3).messageParameters, taskTitle: "Yankee task" }, sourceContext: { ...agent("y-yankee", 3).sourceContext, taskTitle: "Yankee task", sourceOccurredAt: 20 }, sourceOccurredAt: 20 };
  await renderAlert([alpha, zulu, yankee]);

  expect(await screen.findByRole("article", { name: /Zulu task/ })).toBeInTheDocument();
  expect(screen.queryByRole("article", { name: /Yankee task/ })).not.toBeInTheDocument();
  expect(document.querySelector("time")).toHaveAttribute("datetime", "20");
});

test("keeps acknowledged and dispatched group members actionable when completing after an authoritative reload", async () => {
  const acknowledged = { ...agent("delivery-a", 1), state: "acknowledged" as const };
  const dispatched = agent("delivery-b", 2);
  const response = {
    mergeKey: `agent:${dispatched.sourceEntityId}`,
    mergeIdentity: { kind: "agent" as const, ruleId: "rule-1", agentId: "codex" as const, environment: "windows" as const, taskId: "task-7", triggerStatus: "failed" as const },
    members: [{ ...acknowledged, state: "completed" as const }, { ...dispatched, state: "completed" as const }],
    sourceContext: dispatched.sourceContext,
    newestSourceOccurredAt: dispatched.sourceOccurredAt,
  };
  completeMock.mockResolvedValueOnce(response);
  await renderAlert([acknowledged, dispatched]);
  await userEvent.click(await screen.findByRole("button", { name: "完成" }));

  await waitFor(() => expect(completeMock).toHaveBeenCalledWith(expect.objectContaining({
    expectedMemberDeliveryIds: ["delivery-a", "delivery-b"],
    members: [{ id: "delivery-a", expectedState: "acknowledged" }, { id: "delivery-b", expectedState: "dispatched" }],
  })));
  expect(completeMock).toHaveBeenCalledTimes(1);
});

test("submits the complete sorted group identity without optimistic removal", async () => {
  // Dropping a rendered member or submitting an unsorted identity must fail this atomic-action contract.
  const items = [agent("delivery-b", 2), agent("delivery-a", 1)];
  acknowledgeMock.mockRejectedValueOnce({ code: "conflict", retryable: true });
  await renderAlert(items);
  await userEvent.click(await screen.findByRole("button", { name: "知道了" }));
  await waitFor(() => expect(acknowledgeMock).toHaveBeenCalledWith(expect.objectContaining({
    mergeIdentity: { kind: "agent", ruleId: "rule-1", agentId: "codex", environment: "windows", taskId: "task-7", triggerStatus: "failed" },
    expectedMemberDeliveryIds: ["delivery-a", "delivery-b"],
    members: [{ id: "delivery-a", expectedState: "dispatched" }, { id: "delivery-b", expectedState: "dispatched" }],
  })));
  await waitFor(() => expect(reloadGroupMock).toHaveBeenCalledWith({ deliveryId: "delivery-a" }));
  expect(screen.getByRole("article", { name: /Codex.*Release/ })).toBeInTheDocument();
});

test("keeps automatic display unfocused while deliberate keyboard focus can dismiss", async () => {
  // Introducing auto-focus or removing keyboard Escape handling must fail this accessibility contract.
  await renderAlert([agent("delivery-1", 1)]);
  const card = await screen.findByRole("article", { name: /Codex.*Release/ });
  expect(card).not.toHaveFocus();
  card.focus();
  await userEvent.keyboard("{Escape}");
  expect(hideMock).toHaveBeenCalledTimes(1);
});

test("keeps a conflicted Agent card, replays the expanded membership, and retries with all members", async () => {
  // Removing a card before authority responds, or retrying the stale two-member payload, must fail this race contract.
  const retry = vi.fn();
  let renderDeliveries!: (items: ReminderDelivery[]) => Promise<void>;
  beginReminderDispatchSubscriptionMock.mockImplementation((options: { render: (items: ReminderDelivery[]) => Promise<void> }) => {
    renderDeliveries = options.render;
    void renderDeliveries([agent("delivery-b", 2), agent("delivery-a", 1)]);
    retry.mockImplementation(() => renderDeliveries([agent("delivery-c", 3)]));
    return { ready: Promise.resolve({ initial: [], dispose: vi.fn(), retry, lastDispatchSeq: 2 }), dispose: vi.fn() };
  });
  acknowledgeMock.mockRejectedValueOnce({ code: "conflict", retryable: true });
  const members = [agent("delivery-a", 1), agent("delivery-b", 2), agent("delivery-c", 3)];
  reloadGroupMock.mockResolvedValueOnce({ mergeKey: "agent:agent:rule-1:codex:windows:task-7:failed", mergeIdentity: { kind: "agent", ruleId: "rule-1", agentId: "codex", environment: "windows", taskId: "task-7", triggerStatus: "failed" }, members, sourceContext: members[2].sourceContext, newestSourceOccurredAt: 3 });
  const { ReminderAlertApp } = await import(/* @vite-ignore */ "./ReminderAlertApp");
  render(<I18nProvider><ReminderAlertApp consumerId="reminder-alert-window" /></I18nProvider>);
  await userEvent.click(await screen.findByRole("button", { name: "知道了" }));
  expect(screen.getByRole("article", { name: /Codex.*Release/ })).toBeInTheDocument();
  await waitFor(() => expect(reloadGroupMock).toHaveBeenCalledTimes(1));
  await userEvent.click(screen.getByRole("button", { name: "知道了" }));
  await waitFor(() => expect(acknowledgeMock).toHaveBeenLastCalledWith(expect.objectContaining({ expectedMemberDeliveryIds: ["delivery-a", "delivery-b", "delivery-c"] })));
});

test("preserves a same-source dispatch that commits while the conflict reload is pending", async () => {
  let renderDeliveries!: (items: ReminderDelivery[]) => Promise<void>;
  let resolveReload!: (group: object) => void;
  const a = agent("delivery-a", 1);
  const b = agent("delivery-b", 2);
  const removedByServer = agent("delivery-d", 3);
  const concurrent = {
    ...agent("delivery-c", 5),
    messageParameters: { ...agent("delivery-c", 5).messageParameters, taskTitle: "Concurrent C" },
    sourceContext: { ...agent("delivery-c", 5).sourceContext, taskTitle: "Concurrent C" },
  } as ReminderDelivery;
  const unrelated = {
    ...agent("delivery-u", 4),
    sourceEntityId: "agent:rule-1:codex:windows:task-unrelated:failed",
    messageParameters: { ...agent("delivery-u", 4).messageParameters, taskId: "task-unrelated", taskTitle: "Unrelated U" },
    sourceContext: { ...agent("delivery-u", 4).sourceContext, taskId: "task-unrelated", taskTitle: "Unrelated U" },
  } as ReminderDelivery;
  beginReminderDispatchSubscriptionMock.mockImplementation((options: { render: (items: ReminderDelivery[]) => Promise<void> }) => {
    renderDeliveries = options.render;
    void renderDeliveries([a, b, removedByServer, unrelated]);
    return { ready: Promise.resolve({ initial: [], dispose: vi.fn(), retry: vi.fn(), lastDispatchSeq: 4 }), dispose: vi.fn() };
  });
  acknowledgeMock
    .mockRejectedValueOnce({ code: "conflict", retryable: true })
    .mockResolvedValueOnce({
      mergeKey: `agent:${a.sourceEntityId}`,
      mergeIdentity: { kind: "agent", ruleId: "rule-1", agentId: "codex", environment: "windows", taskId: "task-7", triggerStatus: "failed" },
      members: [a, b, concurrent].map((member) => ({ ...member, state: "acknowledged" })),
      sourceContext: concurrent.sourceContext,
      newestSourceOccurredAt: concurrent.sourceOccurredAt,
    });
  reloadGroupMock.mockImplementationOnce(() => new Promise((resolve) => { resolveReload = resolve; }));
  const { ReminderAlertApp } = await import(/* @vite-ignore */ "./ReminderAlertApp");
  render(<I18nProvider><ReminderAlertApp consumerId="reminder-alert-window" /></I18nProvider>);

  await userEvent.click(within(await screen.findByRole("article", { name: /Release/ })).getByRole("button", { name: "知道了" }));
  await waitFor(() => expect(reloadGroupMock).toHaveBeenCalledTimes(1));
  await renderDeliveries([concurrent]);
  expect(screen.getByRole("article", { name: /Concurrent C/ })).toHaveTextContent("4");
  expect(screen.getByRole("article", { name: /Unrelated U/ })).toBeInTheDocument();

  resolveReload({
    mergeKey: `agent:${a.sourceEntityId}`,
    mergeIdentity: { kind: "agent", ruleId: "rule-1", agentId: "codex", environment: "windows", taskId: "task-7", triggerStatus: "failed" },
    members: [a, b],
    sourceContext: b.sourceContext,
    newestSourceOccurredAt: b.sourceOccurredAt,
  });

  const concurrentCard = await screen.findByRole("article", { name: /Concurrent C/ });
  expect(concurrentCard).toHaveTextContent("3");
  expect(screen.getByRole("article", { name: /Unrelated U/ })).toBeInTheDocument();
  await userEvent.click(within(concurrentCard).getByRole("button", { name: "知道了" }));
  await waitFor(() => expect(acknowledgeMock).toHaveBeenLastCalledWith(expect.objectContaining({
    expectedMemberDeliveryIds: ["delivery-a", "delivery-b", "delivery-c"],
    members: [
      { id: "delivery-a", expectedState: "dispatched" },
      { id: "delivery-b", expectedState: "dispatched" },
      { id: "delivery-c", expectedState: "dispatched" },
    ],
  })));
});

test.each([
  ["older response resolves first", "older-first", false],
  ["newer response resolves first", "newer-first", false],
  ["a real-time member arrives after both reloads start", "older-first", true],
] as const)("applies only the latest-started same-source reload when %s", async (_name, responseOrder, includeConcurrent) => {
  let resolveOlder!: (group: object) => void;
  let resolveNewer!: (group: object) => void;
  let renderDeliveries!: (items: ReminderDelivery[]) => Promise<void>;
  const a = agent("delivery-a", 1);
  const b = agent("delivery-b", 2);
  const removedByNewerSnapshot = agent("delivery-d", 3);
  const concurrent = agent("delivery-c", 4);
  beginReminderDispatchSubscriptionMock.mockImplementation((options: { render: (items: ReminderDelivery[]) => Promise<void> }) => {
    renderDeliveries = options.render;
    void renderDeliveries([a, b, removedByNewerSnapshot]);
    return { ready: Promise.resolve({ initial: [], dispose: vi.fn(), retry: vi.fn(), lastDispatchSeq: 3 }), dispose: vi.fn() };
  });
  acknowledgeMock
    .mockRejectedValueOnce({ code: "conflict", retryable: true })
    .mockRejectedValueOnce({ code: "conflict", retryable: true })
    .mockImplementationOnce(() => new Promise(() => undefined));
  reloadGroupMock
    .mockImplementationOnce(() => new Promise((resolve) => { resolveOlder = resolve; }))
    .mockImplementationOnce(() => new Promise((resolve) => { resolveNewer = resolve; }));
  const response = (members: ReminderDelivery[]) => ({
    mergeKey: `agent:${a.sourceEntityId}`,
    mergeIdentity: { kind: "agent", ruleId: "rule-1", agentId: "codex", environment: "windows", taskId: "task-7", triggerStatus: "failed" },
    members,
    sourceContext: members.at(-1)!.sourceContext,
    newestSourceOccurredAt: members.at(-1)!.sourceOccurredAt,
  });
  const { ReminderAlertApp } = await import(/* @vite-ignore */ "./ReminderAlertApp");
  render(<I18nProvider><ReminderAlertApp consumerId="reminder-alert-window" /></I18nProvider>);

  const card = await screen.findByRole("article", { name: /Release/ });
  await userEvent.click(within(card).getByRole("button", { name: "知道了" }));
  await waitFor(() => expect(reloadGroupMock).toHaveBeenCalledTimes(1));
  await userEvent.click(within(card).getByRole("button", { name: "知道了" }));
  await waitFor(() => expect(reloadGroupMock).toHaveBeenCalledTimes(2));
  if (includeConcurrent) await renderDeliveries([concurrent]);
  if (responseOrder === "older-first") {
    await act(async () => { resolveOlder(response([a, b, removedByNewerSnapshot])); });
    await act(async () => { resolveNewer(response([a, b])); });
  } else {
    await act(async () => { resolveNewer(response([a, b])); });
    await act(async () => { resolveOlder(response([a, b, removedByNewerSnapshot])); });
  }

  const recoveredCard = screen.getByRole("article", { name: /Release/ });
  expect(recoveredCard).toHaveTextContent(includeConcurrent ? "3" : "2");
  await userEvent.click(within(recoveredCard).getByRole("button", { name: "知道了" }));
  await waitFor(() => expect(acknowledgeMock).toHaveBeenLastCalledWith(expect.objectContaining({
    expectedMemberDeliveryIds: includeConcurrent
      ? ["delivery-a", "delivery-b", "delivery-c"]
      : ["delivery-a", "delivery-b"],
  })));
  expect(reloadGroupMock).toHaveBeenCalledTimes(2);
});

test("uses independent one-member Todo and Monitor identities and leaves unknown context durable", async () => {
  // Accidentally merging Todo/Monitor or treating their context as acknowledged must fail this isolation contract.
  completeMock.mockRejectedValueOnce({ code: "conflict", retryable: true });
  await renderAlert([todo("todo-delivery", 1), monitor("monitor-delivery", 2)]);
  expect(screen.getAllByText("相关内容将在主窗口中显示。")).toHaveLength(2);
  await userEvent.click((await screen.findAllByRole("button", { name: "完成" }))[1]);
  await waitFor(() => expect(completeMock).toHaveBeenCalledWith(expect.objectContaining({
    mergeIdentity: { kind: "monitor", thresholdId: "threshold-8", breachStartedAt: 4, deliveryId: "monitor-delivery" },
    expectedMemberDeliveryIds: ["monitor-delivery"], members: [{ id: "monitor-delivery", expectedState: "dispatched" }],
  })));
  expect(screen.getAllByText("相关内容将在主窗口中显示。")).toHaveLength(2);
});

test("exposes all action buttons to keyboard interaction without auto-focus", async () => {
  // Removing native button semantics or the Escape handler must fail this deliberate-focus contract.
  vi.spyOn(console, "error").mockImplementation(() => undefined);
  acknowledgeMock.mockRejectedValue({ code: "conflict", retryable: true });
  await renderAlert([agent("delivery-1", 1)]);
  await screen.findByRole("article", { name: /Codex.*Release/ });
  await userEvent.tab();
  expect(screen.getByRole("button", { name: "知道了" })).toHaveFocus();
  await userEvent.keyboard("{Enter}");
  expect(acknowledgeMock).toHaveBeenCalledTimes(1);
  await userEvent.keyboard(" ");
  expect(acknowledgeMock).toHaveBeenCalledTimes(2);
  await userEvent.keyboard("{Shift>}{Tab}{/Shift}");
  screen.getByRole("article", { name: /Codex.*Release/ }).focus();
  await userEvent.keyboard("{Escape}");
  expect(hideMock).toHaveBeenCalled();
});

test("submits complete's sorted group and accepts its full terminal response", async () => {
  const row = agent("delivery-1", 1);
  completeMock.mockResolvedValueOnce(terminalGroup(row, "completed"));
  await renderAlert([row]);
  await userEvent.click(await screen.findByRole("button", { name: "完成" }));
  await waitFor(() => expect(completeMock).toHaveBeenCalledWith(expect.objectContaining({ expectedMemberDeliveryIds: ["delivery-1"], members: [{ id: "delivery-1", expectedState: "dispatched" }] })));
  await waitFor(() => expect(screen.queryByRole("article")).not.toBeInTheDocument());
});

test("submits snooze's sorted group and accepts its full terminal response", async () => {
  const row = agent("delivery-1", 1);
  snoozeMock.mockResolvedValueOnce(terminalGroup(row, "snoozed"));
  await renderAlert([row]);
  await userEvent.click(await screen.findByRole("button", { name: "稍后提醒" }));
  await waitFor(() => expect(snoozeMock).toHaveBeenCalledWith(expect.objectContaining({ expectedMemberDeliveryIds: ["delivery-1"], members: [{ id: "delivery-1", expectedState: "dispatched" }], snoozedUntil: expect.any(Number) })));
  await waitFor(() => expect(screen.queryByRole("article")).not.toBeInTheDocument());
});

test.each(["completed", "snoozed"] as const)("releases a %s member and its recovery tombstone after an overlapping reload settles", async (terminalState) => {
  // Terminal rows are intentionally invisible in the UI, so the production state coordinator is the stable retention seam.
  const { ReminderAlertStateCoordinator } = await import(/* @vite-ignore */ "./ReminderAlertApp");
  const state = new ReminderAlertStateCoordinator();
  const original = agent("delivery-a", 1);
  state.render([original]);
  const originalGroup = state.groups()[0];
  const olderRecovery = state.beginRecovery(originalGroup);
  const latestRecovery = state.beginRecovery(originalGroup);

  state.applyAction([{ ...original, state: terminalState }]);
  expect(state.groups()).toEqual([]);
  expect(state.stats()).toEqual({ activeDeliveryCount: 0, tombstoneCount: 1, recoveringSourceCount: 1 });

  const staleOutcome = state.resolveRecovery(olderRecovery, {
    ...originalGroup,
    members: [original],
  });
  expect(staleOutcome.applied).toBe(false);
  expect(staleOutcome.groups).toEqual([]);
  expect(state.stats()).toEqual({ activeDeliveryCount: 0, tombstoneCount: 1, recoveringSourceCount: 1 });

  const latestOutcome = state.resolveRecovery(latestRecovery, {
    ...originalGroup,
    members: [original],
  });
  expect(latestOutcome.applied).toBe(true);
  expect(latestOutcome.groups).toEqual([]);
  expect(state.stats()).toEqual({ activeDeliveryCount: 0, tombstoneCount: 0, recoveringSourceCount: 0 });

  const current = agent("delivery-b", 2);
  expect(state.render([current])[0].members.map(({ id }) => id)).toEqual(["delivery-b"]);
  expect(state.stats()).toEqual({ activeDeliveryCount: 1, tombstoneCount: 0, recoveringSourceCount: 0 });
});

test("keeps a post-boundary same-ID render when an older recovery snapshot resolves", async () => {
  const { ReminderAlertStateCoordinator } = await import(/* @vite-ignore */ "./ReminderAlertApp");
  const state = new ReminderAlertStateCoordinator();
  const original = agent("delivery-a", 1);
  state.render([original]);
  const recovery = state.beginRecovery(state.groups()[0]);
  const updated = {
    ...agent("delivery-a", 2),
    state: "acknowledged",
    messageParameters: { ...original.messageParameters, taskTitle: "Live newer title" },
    sourceContext: { ...original.sourceContext, taskTitle: "Live newer title", sourceOccurredAt: 20 },
    sourceOccurredAt: 20,
  } as ReminderDelivery;
  state.render([updated]);

  const outcome = state.resolveRecovery(recovery, {
    ...state.groups()[0],
    members: [original],
    sourceContext: original.sourceContext,
    newestSourceOccurredAt: original.sourceOccurredAt,
  });

  expect(outcome.applied).toBe(true);
  expect(outcome.groups).toHaveLength(1);
  expect(outcome.groups[0].members).toEqual([updated]);
  expect(outcome.groups[0].sourceContext).toEqual(updated.sourceContext);
  expect(outcome.groups[0].newestSourceOccurredAt).toBe(20);
});

test.each([
  "reject stale then resolve null latest",
  "resolve stale then reject latest",
] as const)("cleans terminal recovery state after %s", async (settlementOrder) => {
  const { ReminderAlertStateCoordinator } = await import(/* @vite-ignore */ "./ReminderAlertApp");
  const state = new ReminderAlertStateCoordinator();
  const original = agent("delivery-a", 1);
  state.render([original]);
  const originalGroup = state.groups()[0];
  const staleRecovery = state.beginRecovery(originalGroup);
  const latestRecovery = state.beginRecovery(originalGroup);
  state.applyAction([{ ...original, state: "completed" }]);

  if (settlementOrder === "reject stale then resolve null latest") {
    state.rejectRecovery(staleRecovery);
    expect(state.stats()).toEqual({ activeDeliveryCount: 0, tombstoneCount: 1, recoveringSourceCount: 1 });
    const latestOutcome = state.resolveRecovery(latestRecovery, null);
    expect(latestOutcome.applied).toBe(true);
    expect(latestOutcome.groups).toEqual([]);
  } else {
    const staleOutcome = state.resolveRecovery(staleRecovery, { ...originalGroup, members: [original] });
    expect(staleOutcome.applied).toBe(false);
    expect(staleOutcome.groups).toEqual([]);
    expect(state.stats()).toEqual({ activeDeliveryCount: 0, tombstoneCount: 1, recoveringSourceCount: 1 });
    state.rejectRecovery(latestRecovery);
  }

  expect(state.groups()).toEqual([]);
  expect(state.stats()).toEqual({ activeDeliveryCount: 0, tombstoneCount: 0, recoveringSourceCount: 0 });
});

test("rejects an undefined latest reload after terminal completion before rendering only the next delivery", async () => {
  vi.spyOn(console, "error").mockImplementation(() => undefined);
  let renderDeliveries!: (items: ReminderDelivery[]) => Promise<void>;
  let resolveReload!: () => void;
  const original = agent("delivery-a", 1);
  const current = {
    ...agent("delivery-b", 2),
    messageParameters: { ...agent("delivery-b", 2).messageParameters, taskTitle: "Current B" },
    sourceContext: { ...agent("delivery-b", 2).sourceContext, taskTitle: "Current B" },
  } as ReminderDelivery;
  beginReminderDispatchSubscriptionMock.mockImplementation((options: { render: (items: ReminderDelivery[]) => Promise<void> }) => {
    renderDeliveries = options.render;
    void renderDeliveries([original]);
    return { ready: Promise.resolve({ initial: [], dispose: vi.fn(), retry: vi.fn(), lastDispatchSeq: 1 }), dispose: vi.fn() };
  });
  acknowledgeMock.mockRejectedValueOnce({ code: "conflict", retryable: true });
  completeMock.mockResolvedValueOnce(terminalGroup(original, "completed"));
  reloadGroupMock.mockImplementationOnce(() => new Promise((resolve) => { resolveReload = () => resolve(undefined); }));
  const { ReminderAlertApp } = await import(/* @vite-ignore */ "./ReminderAlertApp");
  render(<I18nProvider><ReminderAlertApp consumerId="reminder-alert-window" /></I18nProvider>);

  const card = await screen.findByRole("article", { name: /Release/ });
  await userEvent.click(within(card).getByRole("button", { name: "知道了" }));
  await waitFor(() => expect(reloadGroupMock).toHaveBeenCalledTimes(1));
  await userEvent.click(within(card).getByRole("button", { name: "完成" }));
  await waitFor(() => expect(screen.queryByRole("article")).not.toBeInTheDocument());
  await act(async () => { resolveReload(); });

  await renderDeliveries([current]);
  const currentCard = screen.getByRole("article", { name: /Current B/ });
  expect(currentCard).toBeInTheDocument();
  expect(within(currentCard).queryByText(/已合并/)).not.toBeInTheDocument();
});

test.each([
  ["Todo", todo("todo-delivery", 1), { kind: "todo", todoId: "todo-8", reminderRevision: 5, deliveryId: "todo-delivery" }],
  ["Monitor", monitor("monitor-delivery", 1), { kind: "monitor", thresholdId: "threshold-8", breachStartedAt: 5, deliveryId: "monitor-delivery" }],
])("reloads and retries a stale %s identity without removing its card", async (_name, stale, freshIdentity) => {
  const retry = vi.fn();
  let renderDeliveries!: (rows: ReminderDelivery[]) => Promise<void>;
  beginReminderDispatchSubscriptionMock.mockImplementation((options: { render: (rows: ReminderDelivery[]) => Promise<void> }) => {
    renderDeliveries = options.render;
    void renderDeliveries([stale]);
    retry.mockImplementation(() => renderDeliveries([{ ...stale, sourceContext: stale.sourceKind === "todo" ? { ...stale.sourceContext, reminderRevision: 5 } : { ...stale.sourceContext, breachStartedAt: 5 } } as ReminderDelivery]));
    return { ready: Promise.resolve({ initial: [], dispose: vi.fn(), retry, lastDispatchSeq: 1 }), dispose: vi.fn() };
  });
  completeMock.mockRejectedValueOnce({ code: "conflict", retryable: true });
  const fresh = { ...stale, sourceContext: stale.sourceKind === "todo" ? { ...stale.sourceContext, reminderRevision: 5 } : { ...stale.sourceContext, breachStartedAt: 5 } } as ReminderDelivery;
  reloadGroupMock.mockResolvedValueOnce({ mergeKey: `${fresh.sourceKind}:${fresh.id}`, mergeIdentity: freshIdentity, members: [fresh], sourceContext: fresh.sourceContext, newestSourceOccurredAt: fresh.sourceOccurredAt });
  const { ReminderAlertApp } = await import(/* @vite-ignore */ "./ReminderAlertApp");
  render(<I18nProvider><ReminderAlertApp consumerId="reminder-alert-window" /></I18nProvider>);
  await userEvent.click(await screen.findByRole("button", { name: "完成" }));
  expect(screen.getByRole("article")).toBeInTheDocument();
  await waitFor(() => expect(reloadGroupMock).toHaveBeenCalledTimes(1));
  await userEvent.click(screen.getByRole("button", { name: "完成" }));
  await waitFor(() => expect(completeMock).toHaveBeenLastCalledWith(expect.objectContaining({ mergeIdentity: freshIdentity })));
});

test.each([
  ["Todo", todo("todo-old", 1), todo("todo-new", 2)],
  ["Monitor", monitor("monitor-old", 1), monitor("monitor-new", 2)],
])("replaces a stale %s delivery ID without resurrecting it after terminal retry", async (_name, stale, replacementBase) => {
  let renderDeliveries!: (rows: ReminderDelivery[]) => Promise<void>;
  const replacement = {
    ...replacementBase,
    sourceContext: replacementBase.sourceKind === "todo"
      ? { ...replacementBase.sourceContext, reminderRevision: 5 }
      : { ...replacementBase.sourceContext, breachStartedAt: 5 },
  } as ReminderDelivery;
  const replacementIdentity = replacement.sourceKind === "todo"
    ? { kind: "todo" as const, todoId: "todo-8", reminderRevision: 5, deliveryId: replacement.id }
    : { kind: "monitor" as const, thresholdId: "threshold-8", breachStartedAt: 5, deliveryId: replacement.id };
  const unrelated = replacement.sourceKind === "todo"
    ? { ...todo("todo-unrelated", 3), sourceEntityId: "todo-other", sourceContext: { ...todo("todo-unrelated", 3).sourceContext, todoId: "todo-other" } } as ReminderDelivery
    : { ...monitor("monitor-unrelated", 3), sourceEntityId: "threshold-other", sourceContext: { ...monitor("monitor-unrelated", 3).sourceContext, thresholdId: "threshold-other" } } as ReminderDelivery;
  beginReminderDispatchSubscriptionMock.mockImplementation((options: { render: (rows: ReminderDelivery[]) => Promise<void> }) => {
    renderDeliveries = options.render;
    void renderDeliveries([stale, unrelated]);
    return { ready: Promise.resolve({ initial: [], dispose: vi.fn(), retry: vi.fn(), lastDispatchSeq: 3 }), dispose: vi.fn() };
  });
  completeMock
    .mockRejectedValueOnce({ code: "conflict", retryable: true })
    .mockResolvedValueOnce({
      mergeKey: `${replacement.sourceKind}:${replacement.id}`,
      mergeIdentity: replacementIdentity,
      members: [{ ...replacement, state: "completed" }],
      sourceContext: replacement.sourceContext,
      newestSourceOccurredAt: replacement.sourceOccurredAt,
    });
  reloadGroupMock.mockResolvedValueOnce({
    mergeKey: `${replacement.sourceKind}:${replacement.id}`,
    mergeIdentity: replacementIdentity,
    members: [replacement],
    sourceContext: replacement.sourceContext,
    newestSourceOccurredAt: replacement.sourceOccurredAt,
  });
  const { ReminderAlertApp } = await import(/* @vite-ignore */ "./ReminderAlertApp");
  render(<I18nProvider><ReminderAlertApp consumerId="reminder-alert-window" /></I18nProvider>);

  expect(await screen.findAllByRole("article")).toHaveLength(2);
  await userEvent.click(screen.getAllByRole("button", { name: "完成" })[0]);
  await waitFor(() => expect(reloadGroupMock).toHaveBeenCalledWith({ deliveryId: stale.id }));
  await userEvent.click(screen.getAllByRole("button", { name: "完成" })[0]);
  await waitFor(() => expect(completeMock).toHaveBeenLastCalledWith(expect.objectContaining({
    mergeIdentity: replacementIdentity,
    expectedMemberDeliveryIds: [replacement.id],
  })));
  await waitFor(() => expect(screen.getAllByRole("article")).toHaveLength(1));

  await renderDeliveries([unrelated]);
  expect(screen.getAllByRole("article")).toHaveLength(1);
  expect(screen.getByRole("article")).toHaveTextContent("相关内容将在主窗口中显示。");
});

beforeEach(() => {
  acknowledgeMock.mockReset(); completeMock.mockReset(); snoozeMock.mockReset(); hideMock.mockReset(); beginReminderDispatchSubscriptionMock.mockReset();
  reloadGroupMock.mockReset();
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });
