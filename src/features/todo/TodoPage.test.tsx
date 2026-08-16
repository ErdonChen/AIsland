import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { CommandError, TodoItem, TodoReminder } from "../../api/contracts";
import { I18nProvider } from "../../i18n/I18nProvider";
import TodoPage from "./TodoPage";

const {
  completeTodoMock,
  createTodoMock,
  deleteTodoMock,
  deleteTodoReminderMock,
  invokeMock,
  listTodoRemindersMock,
  listTodosMock,
  listenMock,
  saveTodoReminderMock,
  updateTodoMock,
} = vi.hoisted(() => ({
  completeTodoMock: vi.fn(),
  createTodoMock: vi.fn(),
  deleteTodoMock: vi.fn(),
  deleteTodoReminderMock: vi.fn(),
  invokeMock: vi.fn(),
  listTodoRemindersMock: vi.fn(),
  listTodosMock: vi.fn(),
  listenMock: vi.fn(),
  saveTodoReminderMock: vi.fn(),
  updateTodoMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));
vi.mock("../../api/commands", () => ({
  completeTodo: completeTodoMock,
  createTodo: createTodoMock,
  deleteTodo: deleteTodoMock,
  deleteTodoReminder: deleteTodoReminderMock,
  listTodoReminders: listTodoRemindersMock,
  listTodos: listTodosMock,
  saveTodoReminder: saveTodoReminderMock,
  updateTodo: updateTodoMock,
}));

type Deferred<T> = {
  promise: Promise<T>;
  reject: (error: unknown) => void;
  resolve: (value: T) => void;
};

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((onResolve, onReject) => {
    resolve = onResolve;
    reject = onReject;
  });
  return { promise, reject, resolve };
}

function todoFixture(overrides: Partial<TodoItem> = {}): TodoItem {
  return {
    id: "todo-1",
    title: "Ship V1",
    description: "Prepare release notes",
    dueAt: null,
    priority: "normal",
    status: "open",
    revision: 1,
    createdAt: 1_786_204_000_000,
    updatedAt: 1_786_204_000_000,
    completedAt: null,
    ...overrides,
  };
}

function reminderFixture(overrides: Partial<TodoReminder> = {}): TodoReminder {
  return {
    id: "reminder-1",
    todoId: "todo-1",
    remindAt: 1_786_204_800_000,
    enabled: true,
    revision: 3,
    createdAt: 1_786_204_000_000,
    updatedAt: 1_786_204_000_000,
    ...overrides,
  };
}

function clientRect(left: number, top: number, width: number, height: number): DOMRect {
  return {
    x: left,
    y: top,
    left,
    top,
    right: left + width,
    bottom: top + height,
    width,
    height,
    toJSON: () => ({}),
  } as DOMRect;
}

function commandError(code: CommandError["code"], messageKey: string): CommandError {
  return { code, messageKey, details: { entityId: "todo-1" }, retryable: code === "conflict" };
}

async function renderTodoPage(options: { initialStatus?: "open" | "completed" | "all"; language?: "zh-CN" | "en-US" } = {}) {
  localStorage.setItem("aiceland.ui.language", options.language ?? "en-US");
  const user = userEvent.setup();
  render(
    <I18nProvider>
      <TodoPage initialStatus={options.initialStatus} />
    </I18nProvider>,
  );
  await screen.findByRole("heading", { name: options.language === "zh-CN" ? "待办" : "To-dos" });
  return user;
}

let todoChanged: (() => void) | undefined;

beforeEach(() => {
  localStorage.clear();
  invokeMock.mockReset().mockResolvedValue(undefined);
  listTodosMock.mockReset().mockResolvedValue([]);
  listTodoRemindersMock.mockReset().mockResolvedValue([]);
  createTodoMock.mockReset();
  updateTodoMock.mockReset();
  completeTodoMock.mockReset();
  deleteTodoMock.mockReset();
  saveTodoReminderMock.mockReset();
  deleteTodoReminderMock.mockReset();
  todoChanged = undefined;
  listenMock.mockReset().mockImplementation(async (eventName: string, handler: () => void) => {
    if (eventName === "todoChanged") todoChanged = handler;
    return vi.fn();
  });
  vi.stubGlobal("confirm", vi.fn());
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("TodoPage backend-confirmed workflow", () => {
  it("registers the listener before the first list and sends exact filter values", async () => {
    const user = await renderTodoPage({ initialStatus: "open" });

    expect(listenMock).toHaveBeenCalledWith("todoChanged", expect.any(Function));
    expect(listenMock.mock.invocationCallOrder[0]).toBeLessThan(listTodosMock.mock.invocationCallOrder[0]);
    expect(listTodosMock).toHaveBeenNthCalledWith(1, { status: "open", limit: 500 });
    expect(listTodoRemindersMock).toHaveBeenNthCalledWith(1, { todoId: null });

    await user.click(screen.getByRole("button", { name: "Completed" }));
    await waitFor(() => expect(listTodosMock).toHaveBeenLastCalledWith({ status: "completed", limit: 500 }));
    await user.click(screen.getByRole("button", { name: "All" }));
    await waitFor(() => expect(listTodosMock).toHaveBeenLastCalledWith({ status: "all", limit: 500 }));
  });

  it("waits for backend success before showing a created to-do and disables the form", async () => {
    const pending = deferred<TodoItem>();
    createTodoMock.mockReturnValue(pending.promise);
    const user = await renderTodoPage({ initialStatus: "open" });

    await user.type(screen.getByLabelText("Title"), "Ship V1");
    const create = screen.getByRole("button", { name: "New to-do" });
    await user.click(create);

    expect(screen.queryByText("Ship V1")).not.toBeInTheDocument();
    expect(create).toBeDisabled();
    expect(screen.getByLabelText("Title")).toBeDisabled();
    pending.resolve(todoFixture({ title: "Ship V1", revision: 1 }));
    expect(await screen.findByText("Ship V1")).toBeInTheDocument();
    expect(createTodoMock).toHaveBeenCalledWith({
      title: "Ship V1",
      description: "",
      dueAt: null,
      priority: "normal",
    });
  });

  it("retains a stale edit, renders the translated conflict, reloads, and retries the new revision", async () => {
    const stale = todoFixture({ revision: 1 });
    const current = todoFixture({ title: "Server title", revision: 2, updatedAt: 1_786_204_000_100 });
    listTodosMock.mockResolvedValueOnce([stale]).mockResolvedValueOnce([current]);
    updateTodoMock
      .mockRejectedValueOnce(commandError("conflict", "errors.conflict"))
      .mockResolvedValueOnce(todoFixture({ title: "My draft", revision: 3 }));
    const user = await renderTodoPage();

    await user.click(await screen.findByRole("button", { name: "Edit Ship V1" }));
    const title = screen.getByLabelText("Title");
    await user.clear(title);
    await user.type(title, "My draft");
    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("The item changed elsewhere. Refresh and try again.");
    expect(screen.getByLabelText("Title")).toHaveValue("My draft");
    await waitFor(() => expect(listTodosMock).toHaveBeenCalledTimes(2));
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(updateTodoMock).toHaveBeenLastCalledWith(expect.objectContaining({
      id: "todo-1",
      title: "My draft",
      expectedRevision: 2,
    })));
  });

  it("keeps the authoritative row unchanged until a successful edit resolves", async () => {
    const pending = deferred<TodoItem>();
    listTodosMock.mockResolvedValue([todoFixture()]);
    updateTodoMock.mockReturnValue(pending.promise);
    const user = await renderTodoPage();

    await user.click(await screen.findByRole("button", { name: "Edit Ship V1" }));
    await user.clear(screen.getByLabelText("Title"));
    await user.type(screen.getByLabelText("Title"), "Ship V2");
    const save = screen.getByRole("button", { name: "Save" });
    await user.click(save);

    expect(screen.getByRole("heading", { name: "Ship V1" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Ship V2" })).not.toBeInTheDocument();
    expect(save).toBeDisabled();
    pending.resolve(todoFixture({ title: "Ship V2", revision: 2 }));
    expect(await screen.findByRole("heading", { name: "Ship V2" })).toBeInTheDocument();
  });

  it("locks draft-invalidating controls across rows until the active edit resolves", async () => {
    const pending = deferred<TodoItem>();
    const todoA = todoFixture({ id: "todo-a", title: "Alpha", revision: 1 });
    const todoB = todoFixture({ id: "todo-b", title: "Beta", revision: 4 });
    listTodosMock.mockResolvedValue([todoA, todoB]);
    updateTodoMock.mockReturnValue(pending.promise);
    const user = await renderTodoPage();

    const beta = await screen.findByRole("article", { name: "Beta" });
    fireEvent.change(within(beta).getByLabelText("Remind at"), { target: { value: "2026-08-09T09:15" } });
    await user.click(screen.getByRole("button", { name: "Edit Alpha" }));
    await user.clear(screen.getByLabelText("Title"));
    await user.type(screen.getByLabelText("Title"), "Alpha draft");
    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(screen.getByRole("button", { name: "Edit Beta" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Delete Beta" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Completed" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Edit Beta" }));
    expect(screen.getByLabelText("Title")).toHaveValue("Alpha draft");

    pending.resolve(todoFixture({ ...todoA, title: "Alpha draft", revision: 2 }));
    expect(await screen.findByRole("heading", { name: "Alpha draft" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Edit Beta" })).toBeEnabled();
    expect(within(screen.getByRole("article", { name: "Beta" })).getByLabelText("Remind at")).toHaveValue("2026-08-09T09:15");
  });

  it("changes completion and deletion UI only after backend confirmation", async () => {
    const pendingCompletion = deferred<TodoItem>();
    const pendingDelete = deferred<{ id: string; deleted: true }>();
    listTodosMock.mockResolvedValue([todoFixture()]);
    completeTodoMock.mockReturnValue(pendingCompletion.promise);
    deleteTodoMock.mockReturnValue(pendingDelete.promise);
    const confirmMock = vi.mocked(confirm).mockReturnValue(true);
    const user = await renderTodoPage({ initialStatus: "all" });

    const complete = await screen.findByRole("button", { name: "Complete Ship V1" });
    await user.click(complete);
    expect(screen.getByRole("button", { name: "Complete Ship V1" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Reopen Ship V1" })).not.toBeInTheDocument();
    pendingCompletion.resolve(todoFixture({ status: "completed", revision: 2, completedAt: 1_786_204_900_000 }));
    expect(await screen.findByRole("button", { name: "Reopen Ship V1" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Delete Ship V1" }));
    expect(confirmMock).toHaveBeenCalledWith("Delete this to-do and its reminder?");
    expect(screen.getByText("Ship V1")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete Ship V1" })).toBeDisabled();
    pendingDelete.resolve({ id: "todo-1", deleted: true });
    await waitFor(() => expect(screen.queryByText("Ship V1")).not.toBeInTheDocument());
  });

  it("does not delete when the exact confirmation is declined", async () => {
    listTodosMock.mockResolvedValue([todoFixture()]);
    const confirmMock = vi.mocked(confirm).mockReturnValue(false);
    const user = await renderTodoPage();

    await user.click(await screen.findByRole("button", { name: "Delete Ship V1" }));
    expect(confirmMock).toHaveBeenCalledWith("Delete this to-do and its reminder?");
    expect(deleteTodoMock).not.toHaveBeenCalled();
  });

  it("converts local due and reminder inputs to milliseconds and renders due dates in the active locale", async () => {
    const dueAt = new Date(2026, 7, 8, 16, 30).getTime();
    const remindAt = new Date(2026, 7, 8, 15, 45).getTime();
    listTodosMock.mockResolvedValue([todoFixture({ dueAt })]);
    updateTodoMock.mockResolvedValue(todoFixture({ dueAt: new Date(2026, 7, 9, 10, 15).getTime(), revision: 2 }));
    saveTodoReminderMock.mockResolvedValue(reminderFixture({ remindAt }));
    const user = await renderTodoPage();

    const expectedDue = new Intl.DateTimeFormat("en-US", { dateStyle: "medium", timeStyle: "short" }).format(new Date(dueAt));
    expect(await screen.findByText(expectedDue)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Edit Ship V1" }));
    fireEvent.change(screen.getByLabelText("Due date"), { target: { value: "2026-08-09T10:15" } });
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(updateTodoMock).toHaveBeenCalledWith(expect.objectContaining({
      dueAt: new Date(2026, 7, 9, 10, 15).getTime(),
    })));

    fireEvent.change(screen.getByLabelText("Remind at"), { target: { value: "2026-08-08T15:45" } });
    await user.click(screen.getByRole("button", { name: "Save reminder" }));
    expect(saveTodoReminderMock).toHaveBeenCalledWith({
      id: null,
      todoId: "todo-1",
      remindAt,
      enabled: true,
      expectedRevision: null,
    });
  });

  it("reloads both authoritative lists on a todoChanged hint", async () => {
    listTodosMock.mockResolvedValue([todoFixture()]);
    await renderTodoPage();
    await screen.findByText("Ship V1");
    expect(todoChanged).toEqual(expect.any(Function));

    todoChanged?.();
    await waitFor(() => {
      expect(listTodosMock).toHaveBeenCalledTimes(2);
      expect(listTodoRemindersMock).toHaveBeenCalledTimes(2);
    });
  });

  it("preserves dirty reminder values while rebasing identity across an unrelated reload", async () => {
    const todoA = todoFixture({ id: "todo-a", title: "Alpha" });
    const todoB = todoFixture({ id: "todo-b", title: "Beta" });
    const initialA = reminderFixture({ id: "reminder-a", todoId: "todo-a", remindAt: new Date(2026, 7, 8, 8, 0).getTime(), enabled: true, revision: 3 });
    const initialB = reminderFixture({ id: "reminder-b", todoId: "todo-b", revision: 7 });
    const remoteA = reminderFixture({ id: "reminder-a", todoId: "todo-a", remindAt: new Date(2026, 7, 8, 16, 30).getTime(), enabled: true, revision: 4 });
    listTodosMock.mockResolvedValue([todoA, todoB]);
    listTodoRemindersMock.mockResolvedValueOnce([initialA, initialB]).mockResolvedValueOnce([remoteA, initialB]);
    saveTodoReminderMock.mockImplementation(async (input) => reminderFixture({
      id: input.id ?? "created-reminder",
      todoId: input.todoId,
      remindAt: input.remindAt,
      enabled: input.enabled,
      revision: (input.expectedRevision ?? 0) + 1,
    }));
    const user = await renderTodoPage();
    const alpha = await screen.findByRole("article", { name: "Alpha" });
    const remindAt = within(alpha).getByLabelText("Remind at");
    const enabled = within(alpha).getByLabelText("Enable reminder");
    fireEvent.change(remindAt, { target: { value: "2026-08-08T09:15" } });
    await user.click(enabled);

    todoChanged?.();
    await waitFor(() => expect(listTodoRemindersMock).toHaveBeenCalledTimes(2));
    expect(remindAt).toHaveValue("2026-08-08T09:15");
    expect(enabled).not.toBeChecked();

    await user.click(within(alpha).getByRole("button", { name: "Save reminder" }));
    expect(saveTodoReminderMock).toHaveBeenCalledWith({
      id: "reminder-a",
      todoId: "todo-a",
      remindAt: new Date(2026, 7, 8, 9, 15).getTime(),
      enabled: false,
      expectedRevision: 4,
    });
  });

  it("rebases a reminder draft through conflict reload and adopts snapshots again after its own successful save", async () => {
    const initial = reminderFixture({ remindAt: new Date(2026, 7, 8, 8, 0).getTime(), enabled: true, revision: 3 });
    const conflicted = reminderFixture({ id: "reminder-rebased", remindAt: new Date(2026, 7, 8, 16, 30).getTime(), enabled: true, revision: 4 });
    const saved = reminderFixture({ id: "reminder-rebased", remindAt: new Date(2026, 7, 8, 9, 15).getTime(), enabled: false, revision: 5 });
    const afterSave = reminderFixture({ id: "reminder-rebased", remindAt: new Date(2026, 7, 8, 18, 0).getTime(), enabled: true, revision: 6 });
    listTodosMock.mockResolvedValue([todoFixture()]);
    listTodoRemindersMock.mockResolvedValueOnce([initial]).mockResolvedValue([conflicted]);
    saveTodoReminderMock.mockRejectedValueOnce(commandError("conflict", "errors.conflict")).mockResolvedValueOnce(saved);
    const user = await renderTodoPage();
    const item = await screen.findByRole("article", { name: "Ship V1" });
    const remindAt = within(item).getByLabelText("Remind at");
    const enabled = within(item).getByLabelText("Enable reminder");
    const save = within(item).getByRole("button", { name: "Save reminder" });
    fireEvent.change(remindAt, { target: { value: "2026-08-08T09:15" } });
    await user.click(enabled);

    await user.click(save);
    expect(await screen.findByRole("alert")).toHaveTextContent("The item changed elsewhere. Refresh and try again.");
    await waitFor(() => expect(save).toBeEnabled());
    expect(remindAt).toHaveValue("2026-08-08T09:15");
    expect(enabled).not.toBeChecked();
    expect(saveTodoReminderMock).toHaveBeenNthCalledWith(1, {
      id: "reminder-1",
      todoId: "todo-1",
      remindAt: new Date(2026, 7, 8, 9, 15).getTime(),
      enabled: false,
      expectedRevision: 3,
    });

    await user.click(save);
    await waitFor(() => expect(saveTodoReminderMock).toHaveBeenCalledTimes(2));
    expect(saveTodoReminderMock).toHaveBeenNthCalledWith(2, {
      id: "reminder-rebased",
      todoId: "todo-1",
      remindAt: new Date(2026, 7, 8, 9, 15).getTime(),
      enabled: false,
      expectedRevision: 4,
    });
    await waitFor(() => expect(save).toBeEnabled());
    listTodoRemindersMock.mockResolvedValue([afterSave]);
    todoChanged?.();
    await waitFor(() => expect(remindAt).toHaveValue("2026-08-08T18:00"));
    expect(enabled).toBeChecked();
  });

  it("rebases a dirty reminder to a create after conflict reload finds it deleted", async () => {
    const initial = reminderFixture({ remindAt: new Date(2026, 7, 8, 8, 0).getTime(), enabled: true, revision: 3 });
    const recreated = reminderFixture({ id: "reminder-recreated", remindAt: new Date(2026, 7, 8, 9, 15).getTime(), enabled: false, revision: 1 });
    listTodosMock.mockResolvedValue([todoFixture()]);
    listTodoRemindersMock.mockResolvedValueOnce([initial]).mockResolvedValue([]);
    saveTodoReminderMock.mockRejectedValueOnce(commandError("conflict", "errors.conflict")).mockResolvedValueOnce(recreated);
    const user = await renderTodoPage();
    const item = await screen.findByRole("article", { name: "Ship V1" });
    const remindAt = within(item).getByLabelText("Remind at");
    const enabled = within(item).getByLabelText("Enable reminder");
    const save = within(item).getByRole("button", { name: "Save reminder" });
    fireEvent.change(remindAt, { target: { value: "2026-08-08T09:15" } });
    await user.click(enabled);

    await user.click(save);
    expect(await screen.findByRole("alert")).toHaveTextContent("The item changed elsewhere. Refresh and try again.");
    await waitFor(() => expect(save).toBeEnabled());
    expect(remindAt).toHaveValue("2026-08-08T09:15");
    expect(enabled).not.toBeChecked();

    await user.click(save);
    await waitFor(() => expect(saveTodoReminderMock).toHaveBeenCalledTimes(2));
    expect(saveTodoReminderMock).toHaveBeenNthCalledWith(2, {
      id: null,
      todoId: "todo-1",
      remindAt: new Date(2026, 7, 8, 9, 15).getTime(),
      enabled: false,
      expectedRevision: null,
    });
  });

  it("keeps the latest reminder snapshot when an older reload resolves last", async () => {
    const older = deferred<TodoReminder[]>();
    const newer = deferred<TodoReminder[]>();
    const olderTime = new Date(2026, 7, 8, 8, 0).getTime();
    const newerTime = new Date(2026, 7, 8, 16, 30).getTime();
    listTodosMock.mockResolvedValue([todoFixture()]);
    listTodoRemindersMock.mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise);
    await renderTodoPage();
    await screen.findByText("Ship V1");
    await waitFor(() => expect(listTodoRemindersMock).toHaveBeenCalledTimes(1));

    todoChanged?.();
    await waitFor(() => expect(listTodoRemindersMock).toHaveBeenCalledTimes(2));
    newer.resolve([reminderFixture({ remindAt: newerTime, revision: 4 })]);
    await waitFor(() => expect(screen.getByLabelText("Remind at")).toHaveValue("2026-08-08T16:30"));

    await act(async () => {
      older.resolve([reminderFixture({ remindAt: olderTime, revision: 3 })]);
      await older.promise;
    });
    expect(screen.getByLabelText("Remind at")).toHaveValue("2026-08-08T16:30");
  });

  it("waits for reminder save/delete confirmation and submits the locked flat payload", async () => {
    const reminder = reminderFixture();
    const pendingDelete = deferred<{ id: string; deleted: true }>();
    listTodosMock.mockResolvedValue([todoFixture()]);
    listTodoRemindersMock.mockResolvedValue([reminder]);
    deleteTodoReminderMock.mockReturnValue(pendingDelete.promise);
    const confirmMock = vi.mocked(confirm).mockReturnValue(true);
    const user = await renderTodoPage();

    const enabled = await screen.findByLabelText("Enable reminder");
    expect(enabled).toBeChecked();
    await user.click(screen.getByRole("button", { name: "Delete reminder — Ship V1" }));
    expect(confirmMock).toHaveBeenCalledWith("Delete this to-do reminder?");
    expect(deleteTodoReminderMock).toHaveBeenCalledWith({ id: "reminder-1", expectedRevision: 3 });
    expect(screen.getByRole("button", { name: "Delete reminder — Ship V1" })).toBeDisabled();
    expect(screen.getByLabelText("Enable reminder")).toBeChecked();
    pendingDelete.resolve({ id: "reminder-1", deleted: true });
    await waitFor(() => expect(screen.getByLabelText("Enable reminder")).not.toBeChecked());
  });

  it("gives compact action buttons keyboard names, titles, tooltips, and disabled mutation states", async () => {
    listTodosMock.mockResolvedValue([todoFixture()]);
    const user = await renderTodoPage();
    const item = await screen.findByRole("article", { name: "Ship V1" });

    for (const [name, title] of [
      ["Edit Ship V1", "Edit"],
      ["Complete Ship V1", "Complete"],
      ["Delete Ship V1", "Delete"],
    ] as const) {
      const button = within(item).getByRole("button", { name });
      expect(button).toHaveAttribute("title", title);
      expect(button).toHaveAttribute("data-tooltip", title);
      button.focus();
      expect(button).toHaveFocus();
    }

    await user.tab();
    expect(document.activeElement).not.toBe(document.body);
  });

  it("keeps focused and hovered tooltips inside the current scroll viewport", async () => {
    listTodosMock.mockResolvedValue([
      todoFixture({ id: "todo-a", title: "Alpha" }),
      todoFixture({ id: "todo-b", title: "Beta" }),
    ]);
    listTodoRemindersMock.mockResolvedValue([reminderFixture({ todoId: "todo-b" })]);
    await renderTodoPage();

    const scroll = document.querySelector<HTMLElement>(".todo-scroll");
    const edit = await screen.findByRole("button", { name: "Edit Beta" });
    const removeReminder = screen.getByRole("button", { name: "Delete reminder — Beta" });
    expect(scroll).not.toBeNull();
    const scrollRemoveListener = vi.spyOn(scroll!, "removeEventListener");
    const windowRemoveListener = vi.spyOn(window, "removeEventListener");
    vi.spyOn(scroll!, "getBoundingClientRect").mockReturnValue(clientRect(100, 100, 300, 200));
    let editRect = clientRect(350, 102, 26, 26);
    vi.spyOn(edit, "getBoundingClientRect").mockImplementation(() => editRect);
    vi.spyOn(removeReminder, "getBoundingClientRect").mockReturnValue(clientRect(260, 270, 120, 26));
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function measureTooltip(this: HTMLElement) {
      return this.getAttribute("role") === "tooltip" ? clientRect(0, 0, 80, 24) : clientRect(0, 0, 0, 0);
    });

    fireEvent.focus(edit);
    const focusTooltip = await screen.findByRole("tooltip", { name: "Edit" });
    expect(focusTooltip.parentElement).toBe(document.body);
    expect(focusTooltip).toHaveStyle({ position: "fixed", pointerEvents: "none" });
    expect(Number.parseFloat(focusTooltip.style.left)).toBeGreaterThanOrEqual(104);
    expect(Number.parseFloat(focusTooltip.style.left) + 80).toBeLessThanOrEqual(396);
    expect(Number.parseFloat(focusTooltip.style.top)).toBeGreaterThanOrEqual(104);
    expect(Number.parseFloat(focusTooltip.style.top) + 24).toBeLessThanOrEqual(288);
    expect(edit).toHaveAttribute("title", "Edit");

    const firstTop = focusTooltip.style.top;
    editRect = clientRect(350, 180, 26, 26);
    fireEvent.scroll(scroll!);
    await waitFor(() => expect(focusTooltip.style.top).not.toBe(firstTop));
    editRect = clientRect(180, 180, 26, 26);
    fireEvent(window, new Event("resize"));
    await waitFor(() => expect(Number.parseFloat(focusTooltip.style.left)).toBeLessThan(200));

    fireEvent.blur(edit);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    expect(scrollRemoveListener).toHaveBeenCalledWith("scroll", expect.any(Function));
    expect(windowRemoveListener).toHaveBeenCalledWith("resize", expect.any(Function));
    fireEvent.mouseEnter(removeReminder);
    const hoverTooltip = await screen.findByRole("tooltip", { name: "Delete reminder" });
    expect(Number.parseFloat(hoverTooltip.style.top)).toBeGreaterThanOrEqual(104);
    expect(Number.parseFloat(hoverTooltip.style.top) + 24).toBeLessThanOrEqual(288);
    fireEvent.mouseLeave(removeReminder);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();

    fireEvent.focus(edit);
    expect(await screen.findByRole("tooltip", { name: "Edit" })).toBeInTheDocument();
    const scrollCleanupCount = scrollRemoveListener.mock.calls.filter(([type]) => type === "scroll").length;
    const windowCleanupCount = windowRemoveListener.mock.calls.filter(([type]) => type === "resize").length;
    cleanup();
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    expect(scrollRemoveListener.mock.calls.filter(([type]) => type === "scroll")).toHaveLength(scrollCleanupCount + 1);
    expect(windowRemoveListener.mock.calls.filter(([type]) => type === "resize")).toHaveLength(windowCleanupCount + 1);
  });

  it("uses a locale-neutral reminder delete name in Chinese", async () => {
    listTodosMock.mockResolvedValue([todoFixture()]);
    listTodoRemindersMock.mockResolvedValue([reminderFixture()]);
    await renderTodoPage({ language: "zh-CN" });

    expect(await screen.findByRole("button", { name: "删除提醒 — Ship V1" })).toBeInTheDocument();
  });

  it("uses only the typed API modules and never calls invoke directly", async () => {
    await renderTodoPage({ language: "zh-CN" });
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("shows a newer backend error instead of retaining an older local validation error", async () => {
    const pendingListener = deferred<() => void>();
    listenMock.mockReturnValueOnce(pendingListener.promise);
    const user = await renderTodoPage();

    await user.click(screen.getByRole("button", { name: "New to-do" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Enter a to-do title");

    await act(async () => {
      pendingListener.reject({
        code: "databaseFailure",
        messageKey: "errors.databaseFailure",
        details: { reasonCode: "locked" },
        retryable: true,
      } satisfies CommandError);
      await pendingListener.promise.catch(() => undefined);
    });
    expect(await screen.findByRole("alert")).toHaveTextContent("Database operation failed");
    expect(screen.getByRole("alert")).not.toHaveTextContent("Enter a to-do title");
  });
});
