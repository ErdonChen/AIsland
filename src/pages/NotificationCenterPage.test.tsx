import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

import type { CommandError, NotificationHistoryItem } from "../api/contracts";
import { I18nProvider, useI18n } from "../i18n/I18nProvider";
import NotificationCenterPage from "./NotificationCenterPage";

const mocks = vi.hoisted(() => ({
  beginSubscription: vi.fn(),
  clearHistory: vi.fn(),
  confirm: vi.fn(),
  deleteHistory: vi.fn(),
  dispose: vi.fn(),
  invoke: vi.fn(),
  retry: vi.fn(),
  setRead: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("../api/events", () => ({ beginNotificationHistorySubscription: mocks.beginSubscription }));
vi.mock("../api/commands", () => ({
  clearNotificationHistory: mocks.clearHistory,
  deleteNotificationHistory: mocks.deleteHistory,
  setNotificationRead: mocks.setRead,
}));

const windowsItem = (overrides: Partial<NotificationHistoryItem> = {}): NotificationHistoryItem => ({
  id: "windows-1",
  origin: "windows",
  appId: "Microsoft.WindowsStore",
  sourceEntityId: "wpn-1",
  title: "C:\\Build\\release",
  body: "\\\\server\\share\\artifact",
  messageKey: null,
  messageParameters: {},
  sourceContext: null,
  sourceOccurredAt: 2_000,
  receivedAt: 2_100,
  readAt: null,
  ...overrides,
});

const aicelandItem = (overrides: Partial<NotificationHistoryItem> = {}): NotificationHistoryItem => ({
  id: "aiceland-1",
  origin: "aiceland",
  appId: "AIceLand",
  sourceEntityId: "todo-1",
  title: "",
  body: "",
  messageKey: "reminders.todo.due",
  messageParameters: { todoTitle: "Ship build" },
  sourceContext: { kind: "todo", todoId: "todo-1", reminderRevision: 2, todoTitle: "Ship build", sourceOccurredAt: 3_000 },
  sourceOccurredAt: 3_000,
  receivedAt: 3_100,
  readAt: 3_200,
  ...overrides,
});

const deferred = <T,>() => {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
};

let rows: NotificationHistoryItem[];
let deliverSnapshot: ((rows: NotificationHistoryItem[]) => void) | undefined;

function LanguageToggle() {
  const { language, setLanguage } = useI18n();
  return <button type="button" onClick={() => void setLanguage(language === "zh-CN" ? "en-US" : "zh-CN")}>language</button>;
}

function renderPage() {
  const user = userEvent.setup();
  return {
    user,
    ...render(<I18nProvider><LanguageToggle /><NotificationCenterPage /></I18nProvider>),
  };
}

beforeEach(() => {
  rows = [windowsItem(), aicelandItem()];
  deliverSnapshot = undefined;
  for (const mock of Object.values(mocks)) mock.mockReset();
  mocks.invoke.mockResolvedValue(undefined);
  mocks.confirm.mockReturnValue(true);
  mocks.retry.mockResolvedValue(undefined);
  mocks.setRead.mockImplementation(async ({ id, read }: { id: string; read: boolean }) => ({
    ...(rows.find((row) => row.id === id) ?? windowsItem({ id })),
    readAt: read ? 9_000 : null,
  }));
  mocks.deleteHistory.mockResolvedValue({ id: "windows-1", deleted: true });
  mocks.clearHistory.mockResolvedValue({ removedCount: 2 });
  mocks.beginSubscription.mockImplementation((_input, _onError, onSnapshot) => {
    deliverSnapshot = onSnapshot;
    return {
      ready: Promise.resolve({ initial: rows, listenerState: "active", retry: mocks.retry, dispose: mocks.dispose }),
      dispose: mocks.dispose,
    };
  });
  vi.stubGlobal("confirm", mocks.confirm);
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

test("starts the authoritative subscription with all filters and renders newest raw and translated rows", async () => {
  renderPage();

  expect(mocks.beginSubscription).toHaveBeenCalledWith(
    { origin: "all", sourceApp: null, unreadOnly: false, limit: 500 },
    expect.any(Function),
    expect.any(Function),
  );
  expect(await screen.findByRole("heading", { name: "通知中心" })).toBeVisible();
  const cards = screen.getAllByRole("listitem");
  expect(cards.map((card) => card.getAttribute("data-notification-id"))).toEqual(["aiceland-1", "windows-1"]);
  expect(screen.getByText("待办到期：Ship build")).toBeVisible();
  expect(screen.getByText("C:\\Build\\release")).toBeVisible();
  expect(screen.getByText("\\\\server\\share\\artifact")).toBeVisible();
  expect(screen.getByRole("option", { name: "Microsoft.WindowsStore" })).toBeInTheDocument();
  expect(screen.getByRole("option", { name: "AIceLand" })).toBeInTheDocument();
});

test("keeps every notification card at content height while the list owns scrolling", async () => {
  renderPage();
  const list = await screen.findByRole("list");
  expect(list).toHaveStyle({ gridAutoRows: "max-content" });
});

test("matches the repository newest-first id-descending tie break", async () => {
  rows = [
    windowsItem({ id: "a", receivedAt: 5_000, sourceOccurredAt: 9_000 }),
    windowsItem({ id: "b", receivedAt: 5_000, sourceOccurredAt: 1_000 }),
  ];
  renderPage();

  await screen.findByRole("heading", { name: "通知中心" });
  expect(screen.getAllByRole("listitem").map((card) => card.getAttribute("data-notification-id"))).toEqual(["b", "a"]);
});

test("recreates the subscription with the combined origin source and unread filters", async () => {
  const { user } = renderPage();
  await screen.findByRole("heading", { name: "通知中心" });

  await user.selectOptions(screen.getByRole("combobox", { name: "来源" }), "Microsoft.WindowsStore");
  await user.click(screen.getByRole("checkbox", { name: "仅未读" }));
  await user.click(screen.getByRole("button", { name: "Windows" }));

  await waitFor(() => expect(mocks.beginSubscription).toHaveBeenLastCalledWith(
    { origin: "windows", sourceApp: "Microsoft.WindowsStore", unreadOnly: true, limit: 500 },
    expect.any(Function),
    expect.any(Function),
  ));
  expect(mocks.dispose.mock.calls.length).toBeGreaterThanOrEqual(3);
});

test("keeps Windows text raw while locale changes reproject AIceLand messages without resetting filters", async () => {
  rows = [windowsItem(), aicelandItem({ readAt: null })];
  const { user } = renderPage();
  await screen.findByText("待办到期：Ship build");
  await user.click(screen.getByRole("checkbox", { name: "仅未读" }));
  await user.click(screen.getByRole("button", { name: "language" }));

  expect(await screen.findByText("To-do due: Ship build")).toBeVisible();
  expect(screen.getByText("C:\\Build\\release")).toBeVisible();
  expect(screen.getByText("\\\\server\\share\\artifact")).toBeVisible();
  expect(screen.getByRole("checkbox", { name: "Unread only" })).toBeChecked();
});

test("marks read only after the backend-confirmed row arrives", async () => {
  const result = deferred<NotificationHistoryItem>();
  mocks.setRead.mockReturnValue(result.promise);
  const { user } = renderPage();
  const card = await screen.findByTestId("notification-windows-1");

  await user.click(within(card).getByRole("button", { name: "标为已读" }));
  expect(within(card).getByRole("button", { name: "标为已读" })).toBeDisabled();
  expect(card).toHaveAttribute("data-unread", "true");
  result.resolve(windowsItem({ readAt: 9_000 }));
  await waitFor(() => expect(within(card).getByRole("button", { name: "标为未读" })).toBeEnabled());
  expect(card).toHaveAttribute("data-unread", "false");
});

test("does not let an older snapshot revert a confirmed read or resurrect a confirmed deletion", async () => {
  const { user } = renderPage();
  const card = await screen.findByTestId("notification-windows-1");

  await user.click(within(card).getByRole("button", { name: "标为已读" }));
  await waitFor(() => expect(within(card).getByRole("button", { name: "标为未读" })).toBeEnabled());
  await act(async () => deliverSnapshot?.([windowsItem(), aicelandItem()]));
  expect(within(card).getByRole("button", { name: "标为未读" })).toBeEnabled();

  await user.click(within(card).getByRole("button", { name: "删除此条" }));
  await waitFor(() => expect(screen.queryByTestId("notification-windows-1")).not.toBeInTheDocument());
  await act(async () => deliverSnapshot?.([windowsItem(), aicelandItem()]));
  expect(screen.queryByTestId("notification-windows-1")).not.toBeInTheDocument();
});

test("keeps a notification captured after clear started when the clear result arrives", async () => {
  vi.spyOn(Date, "now").mockReturnValue(3_500);
  const result = deferred<{ removedCount: number }>();
  mocks.clearHistory.mockReturnValue(result.promise);
  const { user } = renderPage();
  await screen.findByTestId("notification-windows-1");

  await user.click(screen.getByRole("button", { name: "清空记录" }));
  expect(mocks.clearHistory).toHaveBeenCalledWith({ before: 3_500, confirmRemoval: true });
  const later = windowsItem({ id: "windows-later", sourceEntityId: "wpn-later", title: "Captured later", receivedAt: 4_000 });
  await act(async () => deliverSnapshot?.([...rows, later]));
  result.resolve({ removedCount: 2 });

  expect(await screen.findByText("Captured later")).toBeVisible();
  await waitFor(() => expect(screen.queryByTestId("notification-windows-1")).not.toBeInTheDocument());
  expect(screen.getByTestId("notification-windows-later")).toBeInTheDocument();
});

test("uses the exact delete confirmation and literal payload without optimistic removal", async () => {
  const result = deferred<{ id: string; deleted: true }>();
  mocks.deleteHistory.mockReturnValue(result.promise);
  const { user } = renderPage();
  const card = await screen.findByTestId("notification-windows-1");

  await user.click(within(card).getByRole("button", { name: "删除此条" }));
  expect(mocks.confirm).toHaveBeenCalledWith("仅从 AIceLand 通知中心移除此条记录？Windows 原通知不会被修改。");
  expect(mocks.deleteHistory).toHaveBeenCalledWith({ id: "windows-1", confirmRemoval: true });
  expect(card).toBeInTheDocument();
  result.resolve({ id: "windows-1", deleted: true });
  await waitFor(() => expect(screen.queryByTestId("notification-windows-1")).not.toBeInTheDocument());
});

test("clears only after exact confirmation and retains all rows when the command fails", async () => {
  vi.spyOn(Date, "now").mockReturnValue(3_500);
  mocks.clearHistory.mockRejectedValue({ code: "databaseFailure", messageKey: "errors.databaseFailure", details: { reasonCode: "failed" }, retryable: true });
  const { user } = renderPage();
  await screen.findByTestId("notification-windows-1");

  await user.click(screen.getByRole("button", { name: "清空记录" }));
  expect(mocks.confirm).toHaveBeenCalledWith("仅清空 AIceLand 保存的通知记录？Windows 原通知和提醒历史不会被删除。");
  expect(mocks.clearHistory).toHaveBeenCalledWith({ before: 3_500, confirmRemoval: true });
  expect(screen.getByTestId("notification-windows-1")).toBeInTheDocument();
  expect(screen.getByTestId("notification-aiceland-1")).toBeInTheDocument();
  expect(screen.getByRole("alert")).toBeVisible();
});

test("keeps AIceLand rows and the warning visible until Retry reports an active listener", async () => {
  const error: CommandError = { code: "notificationUnavailable", messageKey: "errors.notificationUnavailable", details: { reasonCode: "schemaIncompatible" }, retryable: true };
  const subscription = { initial: [aicelandItem()], listenerState: "degraded" as "active" | "degraded", retry: mocks.retry, dispose: mocks.dispose };
  mocks.beginSubscription.mockImplementation((_input, onError) => {
    queueMicrotask(() => onError(error));
    return { ready: Promise.resolve(subscription), dispose: mocks.dispose };
  });
  const { user } = renderPage();

  expect(await screen.findByText("此 Windows 通知格式暂不兼容")).toBeVisible();
  expect(screen.getByText("待办到期：Ship build")).toBeVisible();
  await user.click(screen.getByRole("button", { name: "重试" }));
  expect(mocks.retry).toHaveBeenCalledTimes(1);
  expect(screen.getByText("此 Windows 通知格式暂不兼容")).toBeVisible();
});

test("expands a long body accessibly and disposes a pending subscription on unmount", async () => {
  const ready = deferred<{ initial: NotificationHistoryItem[]; listenerState: "active"; retry: () => Promise<void>; dispose: () => void }>();
  const pendingDispose = vi.fn();
  mocks.beginSubscription.mockReturnValue({ ready: ready.promise, dispose: pendingDispose });
  const view = renderPage();
  view.unmount();
  expect(pendingDispose).toHaveBeenCalledTimes(1);

  rows = [windowsItem({ body: "A".repeat(480) })];
  mocks.beginSubscription.mockImplementation(() => ({ ready: Promise.resolve({ initial: rows, listenerState: "active", retry: mocks.retry, dispose: mocks.dispose }), dispose: mocks.dispose }));
  const second = renderPage();
  const expand = await screen.findByRole("button", { name: "展开" });
  expect(expand).toHaveAttribute("aria-expanded", "false");
  await second.user.click(expand);
  expect(expand).toHaveAttribute("aria-expanded", "true");
  expect(screen.getByText("A".repeat(480))).toBeVisible();
  await act(async () => ready.resolve({ initial: [aicelandItem()], listenerState: "active", retry: mocks.retry, dispose: mocks.dispose }));
  expect(screen.getAllByRole("listitem")).toHaveLength(1);
});
