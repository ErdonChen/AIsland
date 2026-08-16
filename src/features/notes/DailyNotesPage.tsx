import { Copy, Download, FolderOpen, RotateCcw, Search, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { parseCommandError } from "../../api/commandError";
import { createNote, deleteNote, exportNoteMarkdown, getDailyNote, getNote, listNotes, openNoteDirectory, updateNote } from "../../api/commands";
import type { CommandError, LocalDate, NoteDocument, NoteSummary } from "../../api/contracts";
import { listenNoteChanged } from "../../api/events";
import { useI18n } from "../../i18n/I18nProvider";
import { translateRegisteredMessage } from "../../i18n/catalog";
import "./notes.css";

export interface DailyNotesPageProps {
  initialDate?: LocalDate;
  autosaveDelayMs?: number;
}

type AutosaveState = "clean" | "dirty" | "saving" | "failed";
type ErrorContext = "autosave" | "copy" | "exportExists" | "exportFailed" | "action" | null;
type NoteEntry = {
  baseRevision: number | null;
  draft: string;
  error: CommandError | null;
  errorContext: ErrorContext;
  editGeneration: number;
  id: string | null;
  loadToken: number;
  loading: boolean;
  pendingSave: Promise<boolean> | null;
  persistedBody: string;
  saveToken: number;
  state: AutosaveState;
};

const emptyEntry = (): NoteEntry => ({
  baseRevision: null, draft: "", editGeneration: 0, error: null, errorContext: null, id: null,
  loadToken: 0, loading: true, pendingSave: null, persistedBody: "", saveToken: 0, state: "clean",
});

function localToday(): LocalDate {
  const now = new Date();
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;
}

function applyDocument(entry: NoteEntry, document: NoteDocument | null) {
  entry.id = document?.id ?? null;
  entry.baseRevision = document?.revision ?? null;
  entry.draft = document?.bodyMarkdown ?? "";
  entry.persistedBody = entry.draft;
  entry.state = "clean";
  entry.error = null;
  entry.errorContext = null;
  entry.loading = false;
}

export default function DailyNotesPage({ initialDate = localToday(), autosaveDelayMs = 600 }: DailyNotesPageProps): React.JSX.Element {
  const { language, t } = useI18n();
  const entriesRef = useRef(new Map<LocalDate, NoteEntry>());
  const selectedDateRef = useRef<LocalDate>(initialDate);
  const lifecycleRef = useRef(0);
  const autosaveTimerRef = useRef<number | null>(null);
  const searchTimerRef = useRef<number | null>(null);
  const searchTokenRef = useRef(0);
  const actionTokenRef = useRef(0);
  const actionInFlightRef = useRef(false);
  const [selectedDate, setSelectedDate] = useState<LocalDate>(initialDate);
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<NoteSummary[]>([]);
  const [searchPending, setSearchPending] = useState(false);
  const [actionPending, setActionPending] = useState(false);
  const [editorLocked, setEditorLocked] = useState(false);
  const [copySucceeded, setCopySucceeded] = useState(false);
  const [exportedPath, setExportedPath] = useState<string | null>(null);
  const [, setVersion] = useState(0);

  const entryFor = useCallback((date: LocalDate) => {
    let entry = entriesRef.current.get(date);
    if (!entry) { entry = emptyEntry(); entriesRef.current.set(date, entry); }
    return entry;
  }, []);
  const refresh = useCallback(() => setVersion((value) => value + 1), []);
  const clearAutosave = useCallback(() => {
    if (autosaveTimerRef.current !== null) { window.clearTimeout(autosaveTimerRef.current); autosaveTimerRef.current = null; }
  }, []);
  const beginAction = useCallback((): number | null => {
    if (actionInFlightRef.current) return null;
    actionInFlightRef.current = true;
    const token = actionTokenRef.current + 1;
    actionTokenRef.current = token;
    setActionPending(true);
    setEditorLocked(true);
    return token;
  }, []);
  const finishAction = useCallback((token: number) => {
    if (actionTokenRef.current !== token) return;
    actionInFlightRef.current = false;
    setActionPending(false);
    setEditorLocked(false);
  }, []);

  const scheduleAutosaveRef = useRef<(date: LocalDate) => void>(() => undefined);
  const saveEntry = useCallback(async (date: LocalDate, entry: NoteEntry): Promise<boolean> => {
    if (entry.pendingSave) return entry.pendingSave;
    if (entry.id === null && entry.draft.length === 0) {
      entry.state = "clean";
      entry.error = null;
      entry.errorContext = null;
      refresh();
      return true;
    }

    const lifecycle = lifecycleRef.current;
    const capturedDraft = entry.draft;
    const capturedId = entry.id;
    const capturedRevision = entry.baseRevision;
    const saveToken = entry.saveToken + 1;
    entry.saveToken = saveToken;
    entry.state = "saving";
    entry.error = null;
    entry.errorContext = null;
    refresh();

    const operation = (async () => {
      try {
        const saved = capturedId === null
          ? await createNote({ noteDate: date, bodyMarkdown: capturedDraft })
          : await updateNote({ id: capturedId, noteDate: date, bodyMarkdown: capturedDraft, expectedRevision: capturedRevision ?? 0 });
        if (lifecycleRef.current !== lifecycle || entry.saveToken !== saveToken) return false;
        entry.id = saved.id;
        entry.baseRevision = saved.revision;
        entry.persistedBody = capturedDraft;
        entry.error = null;
        entry.errorContext = null;
        entry.state = entry.draft === capturedDraft ? "clean" : "dirty";
        refresh();
        if (entry.state === "dirty" && selectedDateRef.current === date) scheduleAutosaveRef.current(date);
        return true;
      } catch (cause) {
        if (lifecycleRef.current !== lifecycle || entry.saveToken !== saveToken) return false;
        entry.state = "failed";
        entry.error = parseCommandError(cause);
        entry.errorContext = "autosave";
        refresh();
        return false;
      } finally {
        if (entry.saveToken === saveToken) entry.pendingSave = null;
      }
    })();
    entry.pendingSave = operation;
    return operation;
  }, [refresh]);

  const scheduleAutosave = useCallback((date: LocalDate) => {
    clearAutosave();
    autosaveTimerRef.current = window.setTimeout(() => {
      autosaveTimerRef.current = null;
      const entry = entriesRef.current.get(date);
      if (entry && selectedDateRef.current === date && (entry.state === "dirty" || entry.state === "failed")) void saveEntry(date, entry);
    }, autosaveDelayMs);
  }, [autosaveDelayMs, clearAutosave, saveEntry]);
  scheduleAutosaveRef.current = scheduleAutosave;

  const flushEntry = useCallback(async (date: LocalDate, entry: NoteEntry): Promise<boolean> => {
    clearAutosave();
    for (;;) {
      if (entry.state === "clean") return true;
      if (entry.state === "saving" && entry.pendingSave) {
        if (!await entry.pendingSave) return false;
        continue;
      }
      if (!await saveEntry(date, entry)) return false;
    }
  }, [clearAutosave, saveEntry]);

  const loadDate = useCallback(async (date: LocalDate, cleanOnly = false): Promise<boolean> => {
    const entry = entryFor(date);
    if (cleanOnly && entry.state !== "clean") return false;
    const lifecycle = lifecycleRef.current;
    const loadToken = entry.loadToken + 1;
    entry.loadToken = loadToken;
    try {
      const document = await getDailyNote({ noteDate: date });
      if (lifecycleRef.current !== lifecycle || entry.loadToken !== loadToken || selectedDateRef.current !== date || entry.state !== "clean") return false;
      applyDocument(entry, document);
      refresh();
      return true;
    } catch (cause) {
      if (lifecycleRef.current !== lifecycle || entry.loadToken !== loadToken || selectedDateRef.current !== date) return false;
      entry.error = parseCommandError(cause);
      entry.errorContext = "action";
      refresh();
      return false;
    }
  }, [entryFor, refresh]);

  useEffect(() => {
    const lifecycle = lifecycleRef.current + 1;
    lifecycleRef.current = lifecycle;
    let active = true;
    let unlisten: (() => void) | undefined;
    let degradedPollingTimer: number | null = null;
    let initialLoadRetryTimer: number | null = null;
    let initialLoadRetryDelayMs = 1_000;
    const recoverInitialLoad = async (): Promise<void> => {
      const date = selectedDateRef.current;
      if (await loadDate(date)) return;
      const entry = entriesRef.current.get(date);
      if (!active || lifecycleRef.current !== lifecycle || selectedDateRef.current !== date || !entry?.loading || entry.state !== "clean") return;
      const delay = initialLoadRetryDelayMs;
      initialLoadRetryDelayMs = Math.min(initialLoadRetryDelayMs * 2, 30_000);
      initialLoadRetryTimer = window.setTimeout(() => {
        initialLoadRetryTimer = null;
        void recoverInitialLoad();
      }, delay);
    };
    void (async () => {
      try {
        const stop = await listenNoteChanged(() => {
          const date = selectedDateRef.current;
          const entry = entriesRef.current.get(date);
          if (active && lifecycleRef.current === lifecycle && entry?.state === "clean") void loadDate(date, true);
        });
        if (!active || lifecycleRef.current !== lifecycle) { stop(); return; }
        unlisten = stop;
      } catch (cause) {
        if (!active || lifecycleRef.current !== lifecycle) return;
        console.error("Failed to listen for note changes", cause);
        degradedPollingTimer = window.setInterval(() => {
          const date = selectedDateRef.current;
          const entry = entriesRef.current.get(date);
          if (active && lifecycleRef.current === lifecycle && entry?.state === "clean") void loadDate(date, true);
        }, 30_000);
      }
      if (active && lifecycleRef.current === lifecycle) await recoverInitialLoad();
    })();
    return () => {
      active = false;
      if (lifecycleRef.current === lifecycle) lifecycleRef.current += 1;
      if (degradedPollingTimer !== null) window.clearInterval(degradedPollingTimer);
      if (initialLoadRetryTimer !== null) window.clearTimeout(initialLoadRetryTimer);
      clearAutosave();
      if (searchTimerRef.current !== null) window.clearTimeout(searchTimerRef.current);
      searchTokenRef.current += 1;
      actionTokenRef.current += 1;
      actionInFlightRef.current = false;
      for (const entry of entriesRef.current.values()) { entry.loadToken += 1; entry.saveToken += 1; }
      unlisten?.();
    };
  }, [clearAutosave, entryFor, loadDate, refresh]);

  const changeDate = async (nextDate: LocalDate) => {
    if (actionInFlightRef.current || !nextDate || nextDate === selectedDateRef.current) return;
    const currentDate = selectedDateRef.current;
    if (!await flushEntry(currentDate, entryFor(currentDate))) return;
    selectedDateRef.current = nextDate;
    setSelectedDate(nextDate);
    setExportedPath(null);
    entryFor(nextDate);
    refresh();
    void loadDate(nextDate);
  };

  const changeDraft = (draft: string) => {
    const entry = entryFor(selectedDateRef.current);
    entry.draft = draft;
    entry.editGeneration += 1;
    entry.state = "dirty";
    entry.error = null;
    entry.errorContext = null;
    setExportedPath(null);
    setCopySucceeded(false);
    refresh();
    scheduleAutosave(selectedDateRef.current);
  };

  useEffect(() => {
    if (searchTimerRef.current !== null) window.clearTimeout(searchTimerRef.current);
    const query = searchQuery;
    const token = searchTokenRef.current + 1;
    searchTokenRef.current = token;
    if (!query) { setSearchResults([]); setSearchPending(false); return; }
    setSearchPending(true);
    searchTimerRef.current = window.setTimeout(() => {
      searchTimerRef.current = null;
      void listNotes({ query, limit: 100 }).then((results) => {
        if (searchTokenRef.current !== token) return;
        setSearchResults(results);
        setSearchPending(false);
      }).catch((cause) => {
        if (searchTokenRef.current !== token) return;
        const entry = entryFor(selectedDateRef.current);
        entry.error = parseCommandError(cause);
        entry.errorContext = "action";
        setSearchPending(false);
        refresh();
      });
    }, 250);
    return () => { if (searchTimerRef.current !== null) window.clearTimeout(searchTimerRef.current); };
  }, [entryFor, refresh, searchQuery]);

  const openResult = async (summary: NoteSummary) => {
    const token = beginAction();
    if (token === null) return;
    const priorDate = selectedDateRef.current;
    const priorEntry = entryFor(priorDate);
    try {
      if (!await flushEntry(priorDate, priorEntry)) return;
      const priorEditGeneration = priorEntry.editGeneration;
      const document = await getNote({ id: summary.id });
      if (actionTokenRef.current !== token || priorEntry.editGeneration !== priorEditGeneration) return;
      const entry = entryFor(document.noteDate);
      entry.loadToken += 1;
      applyDocument(entry, document);
      selectedDateRef.current = document.noteDate;
      setSelectedDate(document.noteDate);
      setExportedPath(null);
      refresh();
    } catch (cause) {
      if (actionTokenRef.current === token) { const entry = entryFor(selectedDateRef.current); entry.error = parseCommandError(cause); entry.errorContext = "action"; refresh(); }
    } finally { finishAction(token); }
  };

  const copyMarkdown = async () => {
    if (actionInFlightRef.current) return;
    const entry = entryFor(selectedDateRef.current);
    try { await navigator.clipboard.writeText(entry.draft); entry.error = null; entry.errorContext = null; setCopySucceeded(true); }
    catch { entry.error = { code: "ioFailure", messageKey: "notes.error.copy", details: {}, retryable: true }; entry.errorContext = "copy"; setCopySucceeded(false); }
    refresh();
  };

  const openNotesDirectory = async () => {
    const token = beginAction();
    if (token === null) return;
    const entry = entryFor(selectedDateRef.current);
    try {
      await openNoteDirectory();
      if (actionTokenRef.current !== token) return;
      entry.error = null;
      entry.errorContext = null;
      refresh();
    } catch (cause) {
      if (actionTokenRef.current !== token) return;
      entry.error = parseCommandError(cause);
      entry.errorContext = "action";
      refresh();
    } finally {
      finishAction(token);
    }
  };

  const exportMarkdown = async () => {
    const token = beginAction();
    if (token === null) return;
    const date = selectedDateRef.current;
    const entry = entryFor(date);
    let exportCompleted = false;
    try {
      if (!await flushEntry(date, entry) || entry.id === null || entry.baseRevision === null) return;
      entry.error = null;
      entry.errorContext = null;
      refresh();
      const result = await exportNoteMarkdown({ id: entry.id, directory: "", expectedRevision: entry.baseRevision });
      if (actionTokenRef.current !== token || selectedDateRef.current !== date || entry.id !== result.id) return;
      setExportedPath(result.path);
      exportCompleted = true;
      const editGenerationAtRefresh = entry.editGeneration;
      setEditorLocked(false);
      refresh();
      const document = await getNote({ id: entry.id });
      if (actionTokenRef.current !== token || selectedDateRef.current !== date || entry.id !== document.id) return;
      if (entry.editGeneration === editGenerationAtRefresh) {
        entry.baseRevision = document.revision;
        entry.persistedBody = document.bodyMarkdown;
        if (entry.draft === document.bodyMarkdown) entry.state = "clean";
      }
      refresh();
    } catch (cause) {
      if (actionTokenRef.current !== token) return;
      const error = parseCommandError(cause);
      entry.error = error;
      entry.errorContext = exportCompleted ? "action" : error.code === "conflict" ? "exportExists" : "exportFailed";
      refresh();
    } finally { finishAction(token); }
  };

  const removeNote = async () => {
    if (!window.confirm(t("notes.confirm.delete"))) return;
    const token = beginAction();
    if (token === null) return;
    const date = selectedDateRef.current;
    const entry = entryFor(date);
    try {
      if (!await flushEntry(date, entry) || entry.id === null || entry.baseRevision === null) return;
      const draftAtDelete = entry.draft;
      await deleteNote({ id: entry.id, expectedRevision: entry.baseRevision });
      if (actionTokenRef.current !== token || selectedDateRef.current !== date) return;
      entry.loadToken += 1;
      entry.saveToken += 1;
      if (entry.draft === draftAtDelete) {
        applyDocument(entry, null);
      } else {
        entry.id = null;
        entry.baseRevision = null;
        entry.persistedBody = "";
        entry.state = "dirty";
        entry.error = null;
        entry.errorContext = null;
        scheduleAutosave(date);
      }
      setExportedPath(null);
      refresh();
    } catch (cause) {
      if (actionTokenRef.current === token) { entry.error = parseCommandError(cause); entry.errorContext = "action"; refresh(); }
    } finally { finishAction(token); }
  };

  const entry = entryFor(selectedDate);
  const stateKey = entry.state === "clean" ? "notes.state.saved" : entry.state === "saving" ? "notes.state.saving" : "notes.state.unsaved";
  const renderedError = useMemo(() => {
    if (!entry.error) return null;
    if (entry.errorContext === "autosave") return t("notes.error.autosave");
    if (entry.errorContext === "copy") return t("notes.error.copy");
    if (entry.errorContext === "exportExists") return t("notes.error.exportExists");
    if (entry.errorContext === "exportFailed") return t("notes.error.exportFailed");
    try { return translateRegisteredMessage(language, entry.error.messageKey, entry.error.details); }
    catch { return t("notes.error.exportFailed"); }
  }, [entry.error, entry.errorContext, language, t]);

  return <section className="notes-page">
    <header className="notes-header">
      <h2 id="notes-title">{t("notes.title")}</h2>
      <span className={`notes-state notes-state--${entry.state}`} aria-live="polite">{t(stateKey)}</span>
    </header>
    <div className="notes-toolbar">
      <label className="notes-date"><span>{t("notes.field.date")}</span><input type="date" value={selectedDate} disabled={actionPending} onChange={(event) => void changeDate(event.target.value)} /></label>
      <label className="notes-search"><Search size={13} aria-hidden="true" /><span className="sr-only">{t("notes.field.search")}</span><input type="search" aria-label={t("notes.field.search")} value={searchQuery} placeholder={t("notes.search.placeholder")} onChange={(event) => setSearchQuery(event.target.value)} /></label>
    </div>
    {searchQuery && <div className="notes-results" aria-busy={searchPending || undefined}>{!searchPending && searchResults.length === 0 ? <p>{t("notes.empty.search")}</p> : searchResults.map((result) => <button type="button" key={result.id} disabled={actionPending} onClick={() => void openResult(result)}><time>{result.noteDate}</time><span>{result.excerpt}</span></button>)}</div>}
    <label className="notes-editor"><span className="sr-only">{t("notes.title")}</span><textarea aria-label={t("notes.title")} placeholder={t("notes.editor.placeholder")} value={entry.draft} disabled={editorLocked || entry.loading} spellCheck="true" onChange={(event) => changeDraft(event.target.value)} /></label>
    {renderedError && <div className="notes-error" role="alert"><span>{renderedError}</span>{(entry.state === "failed" || entry.loading) && <button type="button" disabled={actionPending} onClick={() => void (entry.loading ? loadDate(selectedDate) : saveEntry(selectedDate, entry))}><RotateCcw size={12} />{t("action.retry")}</button>}</div>}
    {copySucceeded && <p className="notes-copy-success" role="status">{t("notes.copy.success")}</p>}
    {exportedPath && <p className="notes-export-path"><span>{t("notes.export.success")}</span><code>{exportedPath}</code></p>}
    <footer className="notes-actions">
      <button type="button" disabled={actionPending} onClick={() => void copyMarkdown()}><Copy size={13} />{t("notes.action.copy")}</button>
      <button type="button" disabled={actionPending || entry.id === null} onClick={() => void exportMarkdown()}><Download size={13} />{t("notes.action.export")}</button>
      <button type="button" disabled={actionPending} onClick={() => void openNotesDirectory()}><FolderOpen size={13} />{t("notes.action.openFolder")}</button>
      <button className="notes-delete" type="button" disabled={actionPending || entry.id === null} onClick={() => void removeNote()}><Trash2 size={13} />{t("notes.action.delete")}</button>
    </footer>
  </section>;
}
