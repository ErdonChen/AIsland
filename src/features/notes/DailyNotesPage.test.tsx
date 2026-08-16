import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { CommandError, NoteDocument, NoteSummary } from "../../api/contracts";
import { I18nProvider } from "../../i18n/I18nProvider";
import DailyNotesPage from "./DailyNotesPage";

const mocks = vi.hoisted(() => ({
  createNote: vi.fn(), deleteNote: vi.fn(), exportNoteMarkdown: vi.fn(), getDailyNote: vi.fn(), getNote: vi.fn(),
  listNotes: vi.fn(), listenNoteChanged: vi.fn(), openNoteDirectory: vi.fn(), updateNote: vi.fn(), unlisten: vi.fn(), invoke: vi.fn(),
  clipboardWrite: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("../../api/commands", () => ({
  createNote: mocks.createNote, deleteNote: mocks.deleteNote, exportNoteMarkdown: mocks.exportNoteMarkdown,
  getDailyNote: mocks.getDailyNote, getNote: mocks.getNote, listNotes: mocks.listNotes,
  openNoteDirectory: mocks.openNoteDirectory, updateNote: mocks.updateNote,
}));
vi.mock("../../api/events", () => ({ listenNoteChanged: mocks.listenNoteChanged }));

const noteFixture = (overrides: Partial<NoteDocument> = {}): NoteDocument => ({
  id: "note-1", noteDate: "2026-08-08", bodyMarkdown: "old", revision: 3,
  createdAt: 1, updatedAt: 2, ...overrides,
});
const summaryFixture = (overrides: Partial<NoteSummary> = {}): NoteSummary => ({
  id: "note-1", noteDate: "2026-08-08", excerpt: "old", revision: 3, updatedAt: 2, ...overrides,
});
const commandError = (code: CommandError["code"]): CommandError => ({
  code, messageKey: `errors.${code}`, details: code === "conflict" ? { entityId: "note-1" } : { reasonCode: "failed" }, retryable: true,
});
const deferred = <T,>() => {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => { resolve = resolvePromise; reject = rejectPromise; });
  return { promise, reject, resolve };
};

async function renderNotes(date = "2026-08-08", autosaveDelayMs = 600) {
  localStorage.setItem("aiceland.ui.language", "en-US");
  mocks.invoke.mockResolvedValue(undefined);
  render(<I18nProvider><DailyNotesPage initialDate={date} autosaveDelayMs={autosaveDelayMs} /></I18nProvider>);
  await act(async () => { await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); });
  const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: mocks.clipboardWrite } });
  return { editor: screen.getByLabelText("Daily Notes"), user };
}

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  mocks.unlisten.mockReset();
  mocks.listenNoteChanged.mockReset().mockResolvedValue(mocks.unlisten);
  mocks.getDailyNote.mockReset().mockResolvedValue(noteFixture());
  mocks.getNote.mockReset().mockResolvedValue(noteFixture());
  mocks.listNotes.mockReset().mockResolvedValue([]);
  mocks.createNote.mockReset().mockImplementation(async (input) => noteFixture({ id: "created-note", bodyMarkdown: input.bodyMarkdown, noteDate: input.noteDate, revision: 1 }));
  mocks.updateNote.mockReset().mockImplementation(async (input) => noteFixture({ bodyMarkdown: input.bodyMarkdown, noteDate: input.noteDate, revision: input.expectedRevision + 1 }));
  mocks.deleteNote.mockReset().mockResolvedValue({ id: "note-1", deleted: true });
  mocks.exportNoteMarkdown.mockReset().mockResolvedValue({ id: "note-1", path: "C:\\Users\\Me\\Documents\\AIceLand\\2026-08-08.md", bytesWritten: 3 });
  mocks.openNoteDirectory.mockReset().mockResolvedValue(undefined);
  mocks.invoke.mockReset();
  mocks.clipboardWrite.mockReset().mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.clearAllTimers();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("daily note lifecycle and autosave", () => {
  it("uses the authoritative Notes placeholders, states, actions, and no uncatalogued kicker", async () => {
    localStorage.setItem("aiceland.ui.language", "en-US");
    render(<I18nProvider><DailyNotesPage initialDate="2026-08-08" /></I18nProvider>);
    await act(async () => { await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); });

    expect(screen.getByRole("heading", { name: "Daily Notes" })).toBeVisible();
    expect(screen.getByPlaceholderText("Search dates or note text")).toHaveAccessibleName("Search notes");
    expect(screen.getByPlaceholderText("Write today's note in Markdown")).toHaveAccessibleName("Daily Notes");
    expect(screen.getByText("Saved")).toBeVisible();
    expect(screen.getByRole("button", { name: "Copy" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Export Markdown" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Delete this day's note" })).toBeVisible();
    expect(screen.queryByText(/MARKDOWN/)).not.toBeInTheDocument();
  });

  it("opens the controlled notes directory from the daily-note toolbar", async () => {
    const { user } = await renderNotes();

    await user.click(screen.getByRole("button", { name: "Open notes folder" }));

    expect(mocks.openNoteDirectory).toHaveBeenCalledTimes(1);
  });

  it("registers noteChanged before the initial daily-note read", async () => {
    await renderNotes();
    expect(mocks.listenNoteChanged.mock.invocationCallOrder[0]).toBeLessThan(mocks.getDailyNote.mock.invocationCallOrder[0]);
  });

  it("waits for the current daily-note identity before accepting an autosaved draft", async () => {
    const initialLoad = deferred<NoteDocument | null>();
    mocks.getDailyNote.mockReturnValue(initialLoad.promise);
    localStorage.setItem("aiceland.ui.language", "en-US");
    render(<I18nProvider><DailyNotesPage initialDate="2026-08-08" /></I18nProvider>);
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    const editor = screen.getByLabelText("Daily Notes");
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });

    expect(editor).toBeDisabled();
    await user.type(editor, "must not race the initial load");
    expect(editor).toHaveValue("");

    await act(async () => {
      initialLoad.resolve(noteFixture({ bodyMarkdown: "existing note", revision: 7 }));
      await initialLoad.promise;
    });
    expect(editor).toBeEnabled();
    expect(editor).toHaveValue("existing note");

    await user.clear(editor);
    await user.type(editor, "saved against the loaded revision");
    await act(async () => { await vi.advanceTimersByTimeAsync(600); });

    expect(mocks.createNote).not.toHaveBeenCalled();
    expect(mocks.updateNote).toHaveBeenCalledWith({
      id: "note-1",
      noteDate: "2026-08-08",
      bodyMarkdown: "saved against the loaded revision",
      expectedRevision: 7,
    });
  });

  it("retries a transient startup read before enabling autosave against the authoritative revision", async () => {
    vi.useFakeTimers();
    mocks.getDailyNote
      .mockRejectedValueOnce("state not managed yet")
      .mockResolvedValueOnce(noteFixture({ bodyMarkdown: "authoritative", revision: 7 }));
    const { editor } = await renderNotes();

    expect(editor).toBeDisabled();
    await act(async () => { await vi.advanceTimersByTimeAsync(999); });
    expect(mocks.getDailyNote).toHaveBeenCalledTimes(1);
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(mocks.getDailyNote).toHaveBeenCalledTimes(2);
    expect(editor).toBeEnabled();
    expect(editor).toHaveValue("authoritative");

    fireEvent.change(editor, { target: { value: "saved after startup recovery" } });
    await act(async () => { await vi.advanceTimersByTimeAsync(600); });
    expect(mocks.createNote).not.toHaveBeenCalled();
    expect(mocks.updateNote).toHaveBeenLastCalledWith({
      id: "note-1",
      noteDate: "2026-08-08",
      bodyMarkdown: "saved after startup recovery",
      expectedRevision: 7,
    });

    cleanup();
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(mocks.getDailyNote).toHaveBeenCalledTimes(2);
  });

  it("polls one clean daily-note reload every 30 seconds after listener rejection and stops on disposal", async () => {
    vi.useFakeTimers();
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    mocks.listenNoteChanged.mockRejectedValueOnce(commandError("databaseFailure"));
    mocks.getDailyNote
      .mockResolvedValueOnce(noteFixture({ bodyMarkdown: "loaded without listener", revision: 3 }))
      .mockResolvedValueOnce(noteFixture({ bodyMarkdown: "first poll", revision: 4 }))
      .mockResolvedValueOnce(noteFixture({ bodyMarkdown: "second poll", revision: 5 }));
    mocks.updateNote.mockRejectedValueOnce(commandError("databaseFailure"));
    const { editor } = await renderNotes("2026-08-08", 120_000);
    const advance = async (milliseconds: number) => {
      await act(async () => { await vi.advanceTimersByTimeAsync(milliseconds); });
    };

    expect(mocks.listenNoteChanged).toHaveBeenCalledTimes(1);
    expect(mocks.getDailyNote).toHaveBeenCalledWith({ noteDate: "2026-08-08" });
    expect(editor).toHaveValue("loaded without listener");

    await advance(29_999);
    expect(mocks.getDailyNote).toHaveBeenCalledTimes(1);
    await advance(1);
    expect(mocks.getDailyNote).toHaveBeenCalledTimes(2);
    expect(editor).toHaveValue("first poll");
    await advance(30_000);
    expect(mocks.getDailyNote).toHaveBeenCalledTimes(3);
    expect(editor).toHaveValue("second poll");

    fireEvent.change(editor, { target: { value: "dirty local draft" } });
    await advance(30_000);
    expect(mocks.getDailyNote).toHaveBeenCalledTimes(3);
    expect(editor).toHaveValue("dirty local draft");

    await advance(90_000);
    expect(screen.getByText("Autosave failed. Your edits remain in this window.")).toBeVisible();
    await advance(30_000);
    expect(mocks.getDailyNote).toHaveBeenCalledTimes(3);
    expect(editor).toHaveValue("dirty local draft");

    cleanup();
    await advance(60_000);
    expect(mocks.getDailyNote).toHaveBeenCalledTimes(3);
    expect(mocks.listenNoteChanged).toHaveBeenCalledTimes(1);
  });

  it("retains the draft when autosave fails and retries with the same revision", async () => {
    mocks.getDailyNote.mockResolvedValue(noteFixture({ revision: 3, bodyMarkdown: "old" }));
    mocks.updateNote.mockRejectedValueOnce(commandError("databaseFailure"));
    const { editor, user } = await renderNotes();
    await user.clear(editor);
    await user.type(editor, "new draft");
    await vi.advanceTimersByTimeAsync(600);
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    expect(editor).toHaveValue("new draft");
    expect(screen.getByText("Autosave failed. Your edits remain in this window.")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(mocks.updateNote).toHaveBeenLastCalledWith({
      id: expect.any(String), noteDate: "2026-08-08", bodyMarkdown: "new draft", expectedRevision: 3,
    });
  });

  it("flushes before a date switch and keeps the old date selected when the flush fails", async () => {
    mocks.updateNote.mockRejectedValueOnce(commandError("databaseFailure"));
    const { editor, user } = await renderNotes();
    await user.clear(editor);
    await user.type(editor, "stay here");
    fireEvent.change(screen.getByLabelText("Date"), { target: { value: "2026-08-09" } });
    await waitFor(() => expect(mocks.updateNote).toHaveBeenCalledWith({ id: "note-1", noteDate: "2026-08-08", bodyMarkdown: "stay here", expectedRevision: 3 }));
    expect(screen.getByLabelText("Date")).toHaveValue("2026-08-08");
    expect(editor).toHaveValue("stay here");
  });

  it("creates an absent date only after its first non-empty debounced draft", async () => {
    mocks.getDailyNote.mockResolvedValue(null);
    const { editor } = await renderNotes();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(mocks.createNote).not.toHaveBeenCalled();
    fireEvent.change(editor, { target: { value: "# First day" } });
    await vi.advanceTimersByTimeAsync(599);
    expect(mocks.createNote).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(mocks.createNote).toHaveBeenCalledWith({ noteDate: "2026-08-08", bodyMarkdown: "# First day" });
  });

  it("never lets a noteChanged hint overwrite a dirty or failed draft", async () => {
    let hint: (() => void) | undefined;
    mocks.listenNoteChanged.mockImplementation(async (handler) => { hint = handler; return mocks.unlisten; });
    const { editor, user } = await renderNotes();
    await user.clear(editor);
    await user.type(editor, "private draft");
    mocks.getDailyNote.mockResolvedValue(noteFixture({ bodyMarkdown: "remote", revision: 9 }));
    await act(async () => { hint?.(); await Promise.resolve(); });
    expect(editor).toHaveValue("private draft");
    expect(mocks.getDailyNote).toHaveBeenCalledTimes(1);

    mocks.updateNote.mockRejectedValueOnce(commandError("conflict"));
    await vi.advanceTimersByTimeAsync(600);
    await act(async () => { hint?.(); await Promise.resolve(); });
    expect(editor).toHaveValue("private draft");
    expect(mocks.getDailyNote).toHaveBeenCalledTimes(1);
  });

  it("keeps a newer draft when an older autosave succeeds and saves it against the returned revision", async () => {
    const firstSave = deferred<NoteDocument>();
    mocks.updateNote.mockReturnValueOnce(firstSave.promise);
    const { editor } = await renderNotes();
    fireEvent.change(editor, { target: { value: "first draft" } });
    await vi.advanceTimersByTimeAsync(600);
    fireEvent.change(editor, { target: { value: "newer draft" } });
    await act(async () => { firstSave.resolve(noteFixture({ bodyMarkdown: "first draft", revision: 4 })); await firstSave.promise; });
    expect(editor).toHaveValue("newer draft");
    await vi.advanceTimersByTimeAsync(600);
    expect(mocks.updateNote).toHaveBeenLastCalledWith({ id: "note-1", noteDate: "2026-08-08", bodyMarkdown: "newer draft", expectedRevision: 4 });
  });

  it("discards a stale load response after the selected date changes", async () => {
    const slow = deferred<NoteDocument | null>();
    mocks.getDailyNote.mockReturnValueOnce(Promise.resolve(noteFixture())).mockReturnValueOnce(slow.promise).mockResolvedValueOnce(noteFixture({ id: "note-3", noteDate: "2026-08-10", bodyMarkdown: "newest" }));
    await renderNotes();
    fireEvent.change(screen.getByLabelText("Date"), { target: { value: "2026-08-09" } });
    await act(async () => Promise.resolve());
    fireEvent.change(screen.getByLabelText("Date"), { target: { value: "2026-08-10" } });
    expect(await screen.findByDisplayValue("newest")).toBeVisible();
    await act(async () => { slow.resolve(noteFixture({ id: "note-2", noteDate: "2026-08-09", bodyMarkdown: "stale" })); await slow.promise; });
    expect(screen.getByLabelText("Date")).toHaveValue("2026-08-10");
    expect(screen.getByLabelText("Daily Notes")).toHaveValue("newest");
  });

  it("cancels autosave and the listener when disposed", async () => {
    const { editor, user } = await renderNotes();
    await user.clear(editor);
    await user.type(editor, "do not save late");
    cleanup();
    await vi.advanceTimersByTimeAsync(600);
    expect(mocks.updateNote).not.toHaveBeenCalled();
    expect(mocks.unlisten).toHaveBeenCalledTimes(1);
  });
});

describe("daily note search and explicit actions", () => {
  it("keeps backend search order and opens a result through getNote", async () => {
    mocks.listNotes.mockResolvedValue([
      summaryFixture({ id: "second", noteDate: "2026-08-10", excerpt: "Second backend result" }),
      summaryFixture({ id: "first", noteDate: "2026-08-09", excerpt: "First backend result" }),
    ]);
    mocks.getNote.mockResolvedValue(noteFixture({ id: "second", noteDate: "2026-08-10", bodyMarkdown: "opened body", revision: 7 }));
    const { user } = await renderNotes();
    await user.type(screen.getByLabelText("Search notes"), "result");
    await vi.advanceTimersByTimeAsync(250);
    const results = await screen.findAllByRole("button", { name: /backend result/ });
    expect(results.map((item) => item.textContent)).toEqual([expect.stringContaining("Second backend result"), expect.stringContaining("First backend result")]);
    await user.click(results[0]);
    expect(mocks.getNote).toHaveBeenCalledWith({ id: "second" });
    expect(await screen.findByDisplayValue("opened body")).toBeVisible();
  });

  it("does not let a same-date search result overwrite typing started while getNote is pending", async () => {
    const resultLoad = deferred<NoteDocument>();
    mocks.listNotes.mockResolvedValue([summaryFixture({ excerpt: "Same day" })]);
    mocks.getNote.mockReturnValue(resultLoad.promise);
    const { editor } = await renderNotes();
    fireEvent.change(screen.getByLabelText("Search notes"), { target: { value: "same" } });
    await vi.advanceTimersByTimeAsync(250);
    fireEvent.click(await screen.findByRole("button", { name: /Same day/ }));
    await waitFor(() => expect(mocks.getNote).toHaveBeenCalledWith({ id: "note-1" }));
    fireEvent.change(editor, { target: { value: "typed during load" } });

    await act(async () => {
      resultLoad.resolve(noteFixture({ bodyMarkdown: "stale result body", revision: 3 }));
      await resultLoad.promise;
    });

    expect(editor).toHaveValue("typed during load");
    expect(screen.getByLabelText("Date")).toHaveValue("2026-08-08");
  });

  it("does not switch dates when typing starts while a different-date search result is loading", async () => {
    const resultLoad = deferred<NoteDocument>();
    mocks.listNotes.mockResolvedValue([summaryFixture({ id: "note-2", noteDate: "2026-08-09", excerpt: "Next day" })]);
    mocks.getNote.mockReturnValue(resultLoad.promise);
    const { editor } = await renderNotes();
    fireEvent.change(screen.getByLabelText("Search notes"), { target: { value: "next" } });
    await vi.advanceTimersByTimeAsync(250);
    fireEvent.click(await screen.findByRole("button", { name: /Next day/ }));
    await waitFor(() => expect(mocks.getNote).toHaveBeenCalledWith({ id: "note-2" }));
    fireEvent.change(editor, { target: { value: "stay on current day" } });

    await act(async () => {
      resultLoad.resolve(noteFixture({ id: "note-2", noteDate: "2026-08-09", bodyMarkdown: "next day body", revision: 4 }));
      await resultLoad.promise;
    });

    expect(screen.getByLabelText("Date")).toHaveValue("2026-08-08");
    expect(editor).toHaveValue("stay on current day");
    await vi.advanceTimersByTimeAsync(600);
    expect(mocks.updateNote).toHaveBeenLastCalledWith({ id: "note-1", noteDate: "2026-08-08", bodyMarkdown: "stay on current day", expectedRevision: 3 });
  });

  it("discards older search results that resolve after the latest query", async () => {
    const stale = deferred<NoteSummary[]>();
    mocks.listNotes.mockReturnValueOnce(stale.promise).mockResolvedValueOnce([summaryFixture({ id: "latest", excerpt: "Latest result" })]);
    await renderNotes();
    fireEvent.change(screen.getByLabelText("Search notes"), { target: { value: "old" } });
    await vi.advanceTimersByTimeAsync(250);
    fireEvent.change(screen.getByLabelText("Search notes"), { target: { value: "new" } });
    await vi.advanceTimersByTimeAsync(250);
    expect(await screen.findByRole("button", { name: /Latest result/ })).toBeVisible();
    await act(async () => { stale.resolve([summaryFixture({ id: "stale", excerpt: "Stale result" })]); await stale.promise; });
    expect(screen.queryByRole("button", { name: /Stale result/ })).not.toBeInTheDocument();
  });

  it("uses the exact empty-search and persistence-state copy", async () => {
    const pending = deferred<NoteDocument>();
    mocks.updateNote.mockReturnValue(pending.promise);
    const { editor } = await renderNotes();
    fireEvent.change(screen.getByLabelText("Search notes"), { target: { value: "missing" } });
    await vi.advanceTimersByTimeAsync(250);
    expect(await screen.findByText("No matching notes")).toBeVisible();

    fireEvent.change(editor, { target: { value: "local draft" } });
    expect(screen.getByText("Not saved")).toBeVisible();
    await act(async () => { await vi.advanceTimersByTimeAsync(600); });
    expect(screen.getByText("Saving")).toBeVisible();
    await act(async () => { pending.resolve(noteFixture({ bodyMarkdown: "local draft", revision: 4 })); await pending.promise; });
    expect(await screen.findByText("Saved")).toBeVisible();
  });

  it("copies exact Markdown only from a user click and retains it on Clipboard rejection", async () => {
    const { editor, user } = await renderNotes();
    const writeText = mocks.clipboardWrite;
    expect(writeText).not.toHaveBeenCalled();
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Copy" })); await Promise.resolve(); });
    expect(writeText).toHaveBeenLastCalledWith("old");
    expect(screen.getByText("Note copied")).toBeVisible();
    writeText.mockRejectedValueOnce(new Error("denied"));
    await user.clear(editor);
    await user.type(editor, "**exact**\nmarkdown");
    await user.click(screen.getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenLastCalledWith("**exact**\nmarkdown");
    expect(editor).toHaveValue("**exact**\nmarkdown");
    expect(screen.getByRole("alert")).toHaveTextContent("Unable to copy the note");
    expect(screen.queryByText("Note copied")).not.toBeInTheDocument();
  });

  it("blocks opening a search result until a successful export is reconciled", async () => {
    const exported = deferred<{ id: string; path: string; bytesWritten: number }>();
    mocks.listNotes.mockResolvedValue([summaryFixture({ id: "note-2", noteDate: "2026-08-09", excerpt: "Other note" })]);
    mocks.exportNoteMarkdown.mockReturnValue(exported.promise);
    mocks.getNote.mockResolvedValue(noteFixture({ revision: 4 }));
    const { user } = await renderNotes();
    await user.type(screen.getByLabelText("Search notes"), "other");
    await vi.advanceTimersByTimeAsync(250);
    const result = await screen.findByRole("button", { name: /Other note/ });
    await user.click(screen.getByRole("button", { name: "Export Markdown" }));
    expect(result).toBeDisabled();
    fireEvent.click(result);
    expect(mocks.getNote).not.toHaveBeenCalledWith({ id: "note-2" });

    await act(async () => {
      exported.resolve({ id: "note-1", path: "C:\\Users\\Me\\Documents\\AIceLand\\2026-08-08.md", bytesWritten: 3 });
      await exported.promise;
    });

    expect(await screen.findByText("C:\\Users\\Me\\Documents\\AIceLand\\2026-08-08.md")).toBeVisible();
    expect(screen.getByRole("button", { name: /Other note/ })).toBeEnabled();
  });

  it("locks all conflicting controls before flushing a dirty draft for export", async () => {
    const save = deferred<NoteDocument>();
    mocks.updateNote.mockReturnValue(save.promise);
    mocks.listNotes.mockResolvedValue([summaryFixture({ id: "note-2", noteDate: "2026-08-09", excerpt: "Other note" })]);
    mocks.getNote.mockResolvedValue(noteFixture({ bodyMarkdown: "dirty before export", revision: 5 }));
    const { editor, user } = await renderNotes();
    await user.type(screen.getByLabelText("Search notes"), "other");
    await vi.advanceTimersByTimeAsync(250);
    const result = await screen.findByRole("button", { name: /Other note/ });
    fireEvent.change(editor, { target: { value: "dirty before export" } });

    await user.click(screen.getByRole("button", { name: "Export Markdown" }));
    await waitFor(() => expect(mocks.updateNote).toHaveBeenCalledWith({ id: "note-1", noteDate: "2026-08-08", bodyMarkdown: "dirty before export", expectedRevision: 3 }));
    expect(result).toBeDisabled();
    expect(editor).toBeDisabled();
    expect(screen.getByLabelText("Date")).toBeDisabled();
    expect(screen.getByRole("button", { name: "Copy" })).toBeDisabled();
    fireEvent.click(result);
    expect(mocks.getNote).not.toHaveBeenCalledWith({ id: "note-2" });

    await act(async () => {
      save.resolve(noteFixture({ bodyMarkdown: "dirty before export", revision: 4 }));
      await save.promise;
    });

    await waitFor(() => expect(mocks.exportNoteMarkdown).toHaveBeenCalledWith({ id: "note-1", directory: "", expectedRevision: 4 }));
    expect(await screen.findByText("C:\\Users\\Me\\Documents\\AIceLand\\2026-08-08.md")).toBeVisible();
    expect(result).toBeEnabled();
    expect(editor).toBeEnabled();
  });

  it("releases the full action lock when a dirty export flush fails", async () => {
    mocks.updateNote.mockRejectedValueOnce(commandError("databaseFailure"));
    mocks.listNotes.mockResolvedValue([summaryFixture({ id: "note-2", noteDate: "2026-08-09", excerpt: "Other note" })]);
    mocks.getNote.mockResolvedValue(noteFixture({ id: "note-2", noteDate: "2026-08-09", bodyMarkdown: "other body" }));
    const { editor, user } = await renderNotes();
    await user.type(screen.getByLabelText("Search notes"), "other");
    await vi.advanceTimersByTimeAsync(250);
    const result = await screen.findByRole("button", { name: /Other note/ });
    fireEvent.change(editor, { target: { value: "dirty before failed export" } });

    await user.click(screen.getByRole("button", { name: "Export Markdown" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Autosave failed. Your edits remain in this window.");
    expect(result).toBeEnabled();
    expect(editor).toBeEnabled();
    expect(screen.getByLabelText("Date")).toBeEnabled();
    expect(screen.getByRole("button", { name: "Copy" })).toBeEnabled();
    await user.click(result);
    expect(mocks.getNote).toHaveBeenCalledWith({ id: "note-2" });
  });

  it("displays the exported path while the revision refresh is still pending", async () => {
    const refreshed = deferred<NoteDocument>();
    mocks.getNote.mockReturnValue(refreshed.promise);
    const { user } = await renderNotes();
    await user.click(screen.getByRole("button", { name: "Export Markdown" }));
    await waitFor(() => expect(mocks.getNote).toHaveBeenCalledWith({ id: "note-1" }));

    expect(screen.getByText("C:\\Users\\Me\\Documents\\AIceLand\\2026-08-08.md")).toBeVisible();

    await act(async () => {
      refreshed.resolve(noteFixture({ revision: 4 }));
      await refreshed.promise;
    });
  });

  it("does not adopt a refreshed export revision after the user starts a newer draft", async () => {
    const refreshed = deferred<NoteDocument>();
    mocks.getNote.mockReturnValue(refreshed.promise);
    mocks.updateNote.mockRejectedValueOnce(commandError("conflict"));
    const { editor, user } = await renderNotes();
    await user.click(screen.getByRole("button", { name: "Export Markdown" }));
    expect(await screen.findByText("C:\\Users\\Me\\Documents\\AIceLand\\2026-08-08.md")).toBeVisible();
    expect(editor).toBeEnabled();
    fireEvent.change(editor, { target: { value: "newer local draft" } });

    await act(async () => {
      refreshed.resolve(noteFixture({ bodyMarkdown: "remote after export", revision: 9 }));
      await refreshed.promise;
    });
    await vi.advanceTimersByTimeAsync(600);
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });

    expect(mocks.updateNote).toHaveBeenLastCalledWith({ id: "note-1", noteDate: "2026-08-08", bodyMarkdown: "newer local draft", expectedRevision: 3 });
    expect(editor).toHaveValue("newer local draft");
    expect(screen.getByText("Autosave failed. Your edits remain in this window.")).toBeVisible();
  });

  it("keeps export success separate from a failed revision refresh and retains conflict protection", async () => {
    mocks.getNote.mockRejectedValueOnce(commandError("databaseFailure"));
    mocks.updateNote.mockRejectedValueOnce(commandError("conflict"));
    const { editor, user } = await renderNotes();
    await user.click(screen.getByRole("button", { name: "Export Markdown" }));

    expect(await screen.findByText("C:\\Users\\Me\\Documents\\AIceLand\\2026-08-08.md")).toBeVisible();
    expect(screen.getByRole("alert")).toHaveTextContent("Database operation failed");

    fireEvent.change(editor, { target: { value: "safe local draft" } });
    await vi.advanceTimersByTimeAsync(600);
    expect(mocks.updateNote).toHaveBeenLastCalledWith({ id: "note-1", noteDate: "2026-08-08", bodyMarkdown: "safe local draft", expectedRevision: 3 });
    expect(editor).toHaveValue("safe local draft");
  });

  it("exports to the default directory, displays the path, and refetches the incremented revision", async () => {
    mocks.getNote.mockResolvedValue(noteFixture({ revision: 4 }));
    const { user } = await renderNotes();
    await user.click(screen.getByRole("button", { name: "Export Markdown" }));
    expect(mocks.exportNoteMarkdown).toHaveBeenCalledWith({ id: "note-1", directory: "", expectedRevision: 3 });
    expect(await screen.findByText("Exported")).toBeVisible();
    expect(await screen.findByText("C:\\Users\\Me\\Documents\\AIceLand\\2026-08-08.md")).toBeVisible();
    expect(mocks.getNote).toHaveBeenCalledWith({ id: "note-1" });
    await user.clear(screen.getByLabelText("Daily Notes"));
    await user.type(screen.getByLabelText("Daily Notes"), "after export");
    await vi.advanceTimersByTimeAsync(600);
    expect(mocks.updateNote).toHaveBeenLastCalledWith({ id: "note-1", noteDate: "2026-08-08", bodyMarkdown: "after export", expectedRevision: 4 });
  });

  it("renders fixed no-overwrite guidance for an export conflict", async () => {
    mocks.exportNoteMarkdown.mockRejectedValueOnce(commandError("conflict"));
    const { user } = await renderNotes();
    await user.click(screen.getByRole("button", { name: "Export Markdown" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The target file already exists. Choose another directory or file name.");
  });

  it("renders the exact export-failure copy for other export errors", async () => {
    mocks.exportNoteMarkdown.mockRejectedValueOnce(commandError("databaseFailure"));
    const { user } = await renderNotes();
    await user.click(screen.getByRole("button", { name: "Export Markdown" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Unable to export the note");
  });

  it("deletes only after confirmation and clears only after backend success", async () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValueOnce(false).mockReturnValueOnce(true);
    const removal = deferred<{ id: string; deleted: true }>();
    mocks.deleteNote.mockReturnValue(removal.promise);
    const { editor, user } = await renderNotes();
    await user.click(screen.getByRole("button", { name: "Delete this day's note" }));
    expect(mocks.deleteNote).not.toHaveBeenCalled();
    expect(editor).toHaveValue("old");
    await user.click(screen.getByRole("button", { name: "Delete this day's note" }));
    expect(confirm).toHaveBeenLastCalledWith("Delete this day's note?");
    expect(mocks.deleteNote).toHaveBeenCalledWith({ id: "note-1", expectedRevision: 3 });
    expect(editor).toHaveValue("old");
    await act(async () => { removal.resolve({ id: "note-1", deleted: true }); await removal.promise; });
    expect(editor).toHaveValue("");
  });

  it("blocks opening a search result until a successful delete is reconciled", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
    const removal = deferred<{ id: string; deleted: true }>();
    mocks.deleteNote.mockReturnValue(removal.promise);
    mocks.listNotes.mockResolvedValue([summaryFixture({ id: "note-2", noteDate: "2026-08-09", excerpt: "Other note" })]);
    const { editor, user } = await renderNotes();
    await user.type(screen.getByLabelText("Search notes"), "other");
    await vi.advanceTimersByTimeAsync(250);
    const result = await screen.findByRole("button", { name: /Other note/ });
    await user.click(screen.getByRole("button", { name: "Delete this day's note" }));
    expect(result).toBeDisabled();
    fireEvent.click(result);
    expect(mocks.getNote).not.toHaveBeenCalledWith({ id: "note-2" });

    await act(async () => {
      removal.resolve({ id: "note-1", deleted: true });
      await removal.promise;
    });

    expect(editor).toHaveValue("");
    expect(screen.getByRole("button", { name: /Other note/ })).toBeEnabled();
  });

  it("locks result navigation before flushing a dirty draft for delete", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
    const save = deferred<NoteDocument>();
    mocks.updateNote.mockReturnValue(save.promise);
    mocks.listNotes.mockResolvedValue([summaryFixture({ id: "note-2", noteDate: "2026-08-09", excerpt: "Other note" })]);
    const { editor, user } = await renderNotes();
    await user.type(screen.getByLabelText("Search notes"), "other");
    await vi.advanceTimersByTimeAsync(250);
    const result = await screen.findByRole("button", { name: /Other note/ });
    fireEvent.change(editor, { target: { value: "dirty before delete" } });

    await user.click(screen.getByRole("button", { name: "Delete this day's note" }));
    await waitFor(() => expect(mocks.updateNote).toHaveBeenCalledWith({ id: "note-1", noteDate: "2026-08-08", bodyMarkdown: "dirty before delete", expectedRevision: 3 }));
    expect(result).toBeDisabled();
    expect(editor).toBeDisabled();
    fireEvent.click(result);
    expect(mocks.getNote).not.toHaveBeenCalledWith({ id: "note-2" });

    await act(async () => {
      save.resolve(noteFixture({ bodyMarkdown: "dirty before delete", revision: 4 }));
      await save.promise;
    });

    await waitFor(() => expect(mocks.deleteNote).toHaveBeenCalledWith({ id: "note-1", expectedRevision: 4 }));
    expect(editor).toHaveValue("");
    expect(result).toBeEnabled();
  });

  it("does not let a late delete response clear a newer in-memory draft", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
    const removal = deferred<{ id: string; deleted: true }>();
    mocks.deleteNote.mockReturnValue(removal.promise);
    const { editor, user } = await renderNotes();
    await user.click(screen.getByRole("button", { name: "Delete this day's note" }));
    fireEvent.change(editor, { target: { value: "written while delete was pending" } });
    await act(async () => { removal.resolve({ id: "note-1", deleted: true }); await removal.promise; });
    expect(editor).toHaveValue("written while delete was pending");
  });
});
