import { Bell, Check, Pencil, RotateCcw, Trash2 } from "lucide-react";
import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { parseCommandError } from "../../api/commandError";
import { completeTodo, createTodo, deleteTodo, deleteTodoReminder, listTodoReminders, saveTodoReminder, updateTodo } from "../../api/commands";
import type { CommandError, TodoItem, TodoPriority, TodoReminder, TodoStatus } from "../../api/contracts";
import { subscribeTodos, type CommandSubscription } from "../../api/events";
import { useI18n } from "../../i18n/I18nProvider";
import { translateRegisteredMessage } from "../../i18n/catalog";
import "./todo.css";

export interface TodoPageProps { initialStatus?: TodoStatus | "all"; }
type MutationState = "idle" | "saving" | "deleting" | "completing";
type Draft = { title: string; description: string; dueAt: string; priority: TodoPriority };
type ReminderDraft = { dirty: boolean; enabled: boolean; expectedRevision: number | null; id: string | null; remindAt: string };
const EMPTY: Draft = { title: "", description: "", dueAt: "", priority: "normal" };
const EMPTY_REMINDER: ReminderDraft = { dirty: false, enabled: true, expectedRevision: null, id: null, remindAt: "" };

function localInput(value: number | null) {
  if (value === null) return "";
  const date = new Date(value);
  return new Date(value - date.getTimezoneOffset() * 60_000).toISOString().slice(0, 16);
}
function unixMillis(value: string) { const result = value ? new Date(value).getTime() : NaN; return Number.isFinite(result) ? result : null; }
function upsert(rows: TodoItem[], row: TodoItem) { return rows.some((item) => item.id === row.id) ? rows.map((item) => item.id === row.id ? row : item) : [row, ...rows]; }
function reminderForm(reminder?: TodoReminder): ReminderDraft {
  return reminder
    ? { dirty: false, enabled: reminder.enabled, expectedRevision: reminder.revision, id: reminder.id, remindAt: localInput(reminder.remindAt) }
    : { ...EMPTY_REMINDER };
}

export default function TodoPage({ initialStatus = "open" }: TodoPageProps): React.JSX.Element {
  const { language, t } = useI18n();
  const [status, setStatus] = useState<TodoStatus | "all">(initialStatus);
  const [rows, setRows] = useState<TodoItem[]>([]);
  const [reminders, setReminders] = useState<Record<string, TodoReminder>>({});
  const [reminderDrafts, setReminderDrafts] = useState<Record<string, ReminderDraft>>({});
  const [draft, setDraft] = useState<Draft>(EMPTY);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editingRevision, setEditingRevision] = useState<number | null>(null);
  const [mutation, setMutation] = useState<MutationState>("idle");
  const [error, setError] = useState<CommandError | null>(null);
  const subscriptionRef = useRef<CommandSubscription<TodoItem[]> | null>(null);
  const lifecycleRef = useRef(0);
  const reminderRequestRef = useRef(0);

  const loadReminders = useCallback(async (lifecycle: number) => {
    const request = reminderRequestRef.current + 1;
    reminderRequestRef.current = request;
    try {
      const loaded = await listTodoReminders({ todoId: null });
      if (lifecycleRef.current !== lifecycle || reminderRequestRef.current !== request) return;
      const indexed: Record<string, TodoReminder> = {};
      for (const reminder of loaded) indexed[reminder.todoId] = reminder;
      setReminders(indexed);
      setReminderDrafts((current) => {
        const next = { ...current };
        for (const [todoId, draft] of Object.entries(current)) {
          if (!indexed[todoId]) {
            next[todoId] = draft.dirty
              ? { ...draft, expectedRevision: null, id: null }
              : reminderForm();
          }
        }
        for (const reminder of loaded) {
          const draft = current[reminder.todoId];
          next[reminder.todoId] = draft?.dirty
            ? { ...draft, expectedRevision: reminder.revision, id: reminder.id }
            : reminderForm(reminder);
        }
        return next;
      });
    } catch (cause) { if (lifecycleRef.current === lifecycle && reminderRequestRef.current === request) setError(parseCommandError(cause)); }
  }, []);

  const reload = useCallback(async () => { const lifecycle = lifecycleRef.current; await Promise.all([subscriptionRef.current?.retry(), loadReminders(lifecycle)]); }, [loadReminders]);

  useEffect(() => {
    const lifecycle = lifecycleRef.current + 1;
    lifecycleRef.current = lifecycle;
    let disposed = false;
    let subscription: CommandSubscription<TodoItem[]> | null = null;
    let initialSeen = false;
    void (async () => {
      try {
        subscription = await subscribeTodos({ status, limit: 500 }, (nextError) => { if (!disposed && lifecycleRef.current === lifecycle) setError(nextError); }, (snapshot) => {
          if (disposed || lifecycleRef.current !== lifecycle) return;
          setRows(snapshot);
          if (initialSeen) void loadReminders(lifecycle);
          initialSeen = true;
        });
        if (disposed || lifecycleRef.current !== lifecycle) { subscription.dispose(); return; }
        subscriptionRef.current = subscription;
        setRows(subscription.initial);
        await loadReminders(lifecycle);
      } catch (cause) { if (!disposed && lifecycleRef.current === lifecycle) setError(parseCommandError(cause)); }
    })();
    return () => { disposed = true; subscription?.dispose(); if (subscriptionRef.current === subscription) subscriptionRef.current = null; if (lifecycleRef.current === lifecycle) lifecycleRef.current += 1; reminderRequestRef.current += 1; };
  }, [loadReminders, status]);

  const beginEdit = (todo: TodoItem) => { setEditingId(todo.id); setEditingRevision(todo.revision); setDraft({ title: todo.title, description: todo.description, dueAt: localInput(todo.dueAt), priority: todo.priority }); setError(null); };

  const save = async () => {
    if (!draft.title.trim()) { setError({ code: "invalidInput", messageKey: "todo.error.titleRequired", details: {}, retryable: false }); return; }
    setMutation("saving"); setError(null);
    try {
      const input = { title: draft.title, description: draft.description, dueAt: unixMillis(draft.dueAt), priority: draft.priority };
      const saved = editingId === null ? await createTodo(input) : await updateTodo({ ...input, id: editingId, expectedRevision: editingRevision ?? 0 });
      setRows((current) => upsert(current, saved)); setDraft(EMPTY); setEditingId(null); setEditingRevision(null);
    } catch (cause) {
      const nextError = parseCommandError(cause); setError(nextError);
      if (nextError.code === "conflict") { await reload(); const latest = subscriptionRef.current?.initial.find((row) => row.id === editingId); if (latest) setEditingRevision(latest.revision); }
    } finally { setMutation("idle"); }
  };

  const complete = async (todo: TodoItem) => {
    setMutation("completing"); setError(null);
    try { const saved = await completeTodo({ id: todo.id, completed: todo.status === "open", expectedRevision: todo.revision }); setRows((current) => upsert(current, saved)); }
    catch (cause) { setError(parseCommandError(cause)); } finally { setMutation("idle"); }
  };
  const remove = async (todo: TodoItem) => {
    if (!confirm(t("todo.confirm.delete"))) return;
    setMutation("deleting"); setError(null);
    try { await deleteTodo({ id: todo.id, expectedRevision: todo.revision }); setRows((current) => current.filter((item) => item.id !== todo.id)); setReminders((current) => { const next = { ...current }; delete next[todo.id]; return next; }); }
    catch (cause) { setError(parseCommandError(cause)); } finally { setMutation("idle"); }
  };
  const saveReminderFor = async (todoId: string) => {
    const reminderDraft = reminderDrafts[todoId] ?? reminderForm(reminders[todoId]); const remindAt = unixMillis(reminderDraft.remindAt);
    if (remindAt === null) { setError({ code: "invalidInput", messageKey: "todo.error.reminderFailed", details: {}, retryable: false }); return; }
    setMutation("saving"); setError(null);
    try { const saved = await saveTodoReminder({ id: reminderDraft.id, todoId, remindAt, enabled: reminderDraft.enabled, expectedRevision: reminderDraft.expectedRevision }); setReminders((items) => ({ ...items, [todoId]: saved })); setReminderDrafts((items) => ({ ...items, [todoId]: reminderForm(saved) })); }
    catch (cause) { const nextError = parseCommandError(cause); setError(nextError); if (nextError.code === "conflict") await reload(); } finally { setMutation("idle"); }
  };
  const removeReminderFor = async (todoId: string) => {
    const reminderDraft = reminderDrafts[todoId] ?? reminderForm(reminders[todoId]);
    if (reminderDraft.id === null || reminderDraft.expectedRevision === null || !confirm(t("todo.confirm.deleteReminder"))) return;
    setMutation("deleting"); setError(null);
    try { await deleteTodoReminder({ id: reminderDraft.id, expectedRevision: reminderDraft.expectedRevision }); setReminders((current) => { const next = { ...current }; delete next[todoId]; return next; }); setReminderDrafts((current) => ({ ...current, [todoId]: { ...reminderForm(), enabled: false } })); }
    catch (cause) { setError(parseCommandError(cause)); } finally { setMutation("idle"); }
  };

  const renderedError = useMemo(() => {
    if (!error) return null;
    if (error.messageKey === "todo.error.titleRequired") return t("todo.error.titleRequired");
    if (error.messageKey === "todo.error.reminderFailed") return t("todo.error.reminderFailed");
    try { return translateRegisteredMessage(language, error.messageKey, error.details); }
    catch { return t("todo.error.saveFailed"); }
  }, [error, language, t]);
  const visible = rows.filter((todo) => status === "all" || todo.status === status);
  const busy = mutation !== "idle";

  return <section className="todo-page" aria-labelledby="todo-title">
    <header className="todo-page__header"><h2 id="todo-title">{t("todo.title")}</h2><div className="todo-filters" role="group" aria-label={t("todo.title")}>{(["open", "completed", "all"] as const).map((value) => <button key={value} type="button" className={status === value ? "todo-filter todo-filter--active" : "todo-filter"} aria-pressed={status === value} disabled={busy} onClick={() => setStatus(value)}>{t(`todo.filter.${value}`)}</button>)}</div></header>
    <form className="todo-compose" onSubmit={(event) => { event.preventDefault(); void save(); }}>
      <label><span>{t("todo.field.title")}</span><input value={draft.title} disabled={busy} onChange={(event) => setDraft((current) => ({ ...current, title: event.target.value }))} /></label>
      <label className="todo-compose__description"><span>{t("todo.field.description")}</span><input value={draft.description} disabled={busy} onChange={(event) => setDraft((current) => ({ ...current, description: event.target.value }))} /></label>
      <label><span>{t("todo.field.dueAt")}</span><input type="datetime-local" value={draft.dueAt} disabled={busy} onChange={(event) => setDraft((current) => ({ ...current, dueAt: event.target.value }))} /></label>
      <label><span>{t("todo.field.priority")}</span><select value={draft.priority} disabled={busy} onChange={(event) => setDraft((current) => ({ ...current, priority: event.target.value as TodoPriority }))}><option value="low">{t("todo.priority.low")}</option><option value="normal">{t("todo.priority.normal")}</option><option value="high">{t("todo.priority.high")}</option></select></label>
      <button className="todo-primary" type="submit" disabled={busy}>{editingId === null ? t("todo.action.create") : t("action.save")}</button>
    </form>
    {renderedError && <p className="todo-error" role="alert">{renderedError}</p>}
    <div className="todo-scroll">{visible.length === 0 ? <p className="todo-empty">{t(status === "completed" ? "todo.empty.completed" : "todo.empty.open")}</p> : visible.map((todo) => {
      const reminder = reminders[todo.id]; const reminderDraft = reminderDrafts[todo.id] ?? reminderForm(reminder);
      return <article key={todo.id} className={`todo-card todo-card--${todo.priority}`} aria-label={todo.title}>
        <div className="todo-card__main"><div className="todo-card__copy"><h3>{todo.title}</h3>{todo.description && <p>{todo.description}</p>}{todo.dueAt !== null && <time dateTime={new Date(todo.dueAt).toISOString()}>{new Intl.DateTimeFormat(language, { dateStyle: "medium", timeStyle: "short" }).format(new Date(todo.dueAt))}</time>}</div><div className="todo-card__actions">
          <TooltipButton label={`${t("todo.action.edit")} ${todo.title}`} tooltip={t("todo.action.edit")} disabled={busy} onClick={() => beginEdit(todo)}><Pencil size={14} /></TooltipButton>
          <TooltipButton label={`${t(todo.status === "open" ? "todo.action.complete" : "todo.action.reopen")} ${todo.title}`} tooltip={t(todo.status === "open" ? "todo.action.complete" : "todo.action.reopen")} disabled={busy} onClick={() => void complete(todo)}>{todo.status === "open" ? <Check size={14} /> : <RotateCcw size={14} />}</TooltipButton>
          <TooltipButton label={`${t("todo.action.delete")} ${todo.title}`} tooltip={t("todo.action.delete")} disabled={busy} onClick={() => void remove(todo)}><Trash2 size={14} /></TooltipButton>
        </div></div>
        <fieldset className="todo-reminder" disabled={busy}><legend><Bell size={12} />{t("todo.reminder.title")}</legend><label className="todo-reminder__enabled"><input type="checkbox" checked={reminderDraft.enabled} onChange={(event) => setReminderDrafts((current) => ({ ...current, [todo.id]: { ...reminderDraft, dirty: true, enabled: event.target.checked } }))} />{t("todo.reminder.enabled")}</label><label><span>{t("todo.reminder.remindAt")}</span><input type="datetime-local" value={reminderDraft.remindAt} onChange={(event) => setReminderDrafts((current) => ({ ...current, [todo.id]: { ...reminderDraft, dirty: true, remindAt: event.target.value } }))} /></label><button type="button" onClick={() => void saveReminderFor(todo.id)}>{t("todo.action.saveReminder")}</button>{reminderDraft.id !== null && <TooltipButton className="todo-reminder__delete" label={`${t("todo.action.deleteReminder")} — ${todo.title}`} tooltip={t("todo.action.deleteReminder")} disabled={busy} onClick={() => void removeReminderFor(todo.id)}>{t("todo.action.deleteReminder")}</TooltipButton>}</fieldset>
      </article>;
    })}</div>
  </section>;
}

type TooltipButtonProps = {
  children: React.ReactNode;
  className?: string;
  disabled: boolean;
  label: string;
  onClick: () => void;
  tooltip: string;
};

const TOOLTIP_EDGE_GAP = 4;
const TOOLTIP_ANCHOR_GAP = 5;
const HEIGHT_STRIP_CLEARANCE = 12;

function TooltipButton({ children, className = "todo-icon-button", disabled, label, onClick, tooltip }: TooltipButtonProps) {
  const buttonRef = useRef<HTMLButtonElement>(null);
  const tooltipRef = useRef<HTMLSpanElement>(null);
  const tooltipId = useId();
  const [focused, setFocused] = useState(false);
  const [hovered, setHovered] = useState(false);
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null);
  const visible = focused || hovered;

  useLayoutEffect(() => {
    if (!visible) return;
    const button = buttonRef.current;
    const tooltipElement = tooltipRef.current;
    const scrollViewport = button?.closest<HTMLElement>(".todo-scroll");
    if (!button || !tooltipElement || !scrollViewport) return;

    const updatePosition = () => {
      const anchor = button.getBoundingClientRect();
      const viewport = scrollViewport.getBoundingClientRect();
      const tooltipRect = tooltipElement.getBoundingClientRect();
      const safeLeft = viewport.left + TOOLTIP_EDGE_GAP;
      const safeRight = viewport.right - TOOLTIP_EDGE_GAP;
      const safeTop = viewport.top + TOOLTIP_EDGE_GAP;
      const safeBottom = viewport.bottom - HEIGHT_STRIP_CLEARANCE;
      const above = anchor.top - TOOLTIP_ANCHOR_GAP - tooltipRect.height;
      const below = anchor.bottom + TOOLTIP_ANCHOR_GAP;
      const top = above >= safeTop
        ? above
        : below + tooltipRect.height <= safeBottom
          ? below
          : Math.max(safeTop, Math.min(anchor.top, safeBottom - tooltipRect.height));
      const maxLeft = Math.max(safeLeft, safeRight - tooltipRect.width);
      const left = Math.max(safeLeft, Math.min(anchor.right - tooltipRect.width, maxLeft));
      setPosition((current) => current?.left === left && current.top === top ? current : { left, top });
    };

    updatePosition();
    scrollViewport.addEventListener("scroll", updatePosition, { passive: true });
    window.addEventListener("resize", updatePosition);
    return () => {
      scrollViewport.removeEventListener("scroll", updatePosition);
      window.removeEventListener("resize", updatePosition);
    };
  }, [tooltip, visible]);

  return <>
    <button ref={buttonRef} type="button" className={className} aria-label={label} aria-describedby={visible ? tooltipId : undefined} title={tooltip} data-tooltip={tooltip} disabled={disabled} onClick={onClick} onFocus={() => setFocused(true)} onBlur={() => setFocused(false)} onMouseEnter={() => setHovered(true)} onMouseLeave={() => setHovered(false)}>{children}</button>
    {visible && createPortal(<span ref={tooltipRef} id={tooltipId} role="tooltip" className="todo-tooltip" style={{ position: "fixed", pointerEvents: "none", left: position?.left ?? -10_000, top: position?.top ?? -10_000, visibility: position ? "visible" : "hidden" }}>{tooltip}</span>, document.body)}
  </>;
}
