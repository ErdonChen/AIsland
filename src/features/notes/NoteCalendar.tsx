import { CalendarDays, ChevronDown, ChevronLeft, ChevronRight } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { listNoteContentDates } from "../../api/commands";
import type { LocalDate, NoteDateContentSummary } from "../../api/contracts";
import { useI18n } from "../../i18n/I18nProvider";

interface NoteCalendarProps {
  selectedDate: LocalDate;
  onSelectDate(date: LocalDate): Promise<boolean>;
  today?: LocalDate;
  contentVersion?: number;
}

function calendarToday(): LocalDate {
  const now = new Date();
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;
}

function monthFromDate(date: LocalDate): string { return date.slice(0, 7); }
function monthParts(month: string): [number, number] { return [Number(month.slice(0, 4)), Number(month.slice(5, 7))]; }
function dateFromUtc(date: Date): LocalDate {
  return `${date.getUTCFullYear()}-${String(date.getUTCMonth() + 1).padStart(2, "0")}-${String(date.getUTCDate()).padStart(2, "0")}`;
}
function parseDate(date: LocalDate): Date {
  return new Date(Date.UTC(Number(date.slice(0, 4)), Number(date.slice(5, 7)) - 1, Number(date.slice(8, 10))));
}
function shiftDate(date: LocalDate, days: number): LocalDate {
  const shifted = parseDate(date);
  shifted.setUTCDate(shifted.getUTCDate() + days);
  return dateFromUtc(shifted);
}
function shiftMonth(month: string, delta: number): string {
  const [year, oneBasedMonth] = monthParts(month);
  return monthFromDate(dateFromUtc(new Date(Date.UTC(year, oneBasedMonth - 1 + delta, 1))));
}
function sixWeekDates(month: string): LocalDate[] {
  const [year, oneBasedMonth] = monthParts(month);
  const first = new Date(Date.UTC(year, oneBasedMonth - 1, 1));
  const daysSinceMonday = (first.getUTCDay() + 6) % 7;
  first.setUTCDate(first.getUTCDate() - daysSinceMonday);
  return Array.from({ length: 42 }, (_, index) => {
    const cell = new Date(first);
    cell.setUTCDate(first.getUTCDate() + index);
    return dateFromUtc(cell);
  });
}

export default function NoteCalendar({ selectedDate, onSelectDate, today = calendarToday(), contentVersion = 0 }: NoteCalendarProps): React.JSX.Element {
  const { language, t } = useI18n();
  const gridRef = useRef<HTMLDivElement>(null);
  const [expanded, setExpanded] = useState(false);
  const [month, setMonth] = useState(() => monthFromDate(selectedDate));
  const [focusedDate, setFocusedDate] = useState(selectedDate);
  const [content, setContent] = useState<Map<string, NoteDateContentSummary>>(new Map());
  const [pending, setPending] = useState(false);
  const [error, setError] = useState(false);
  const dates = useMemo(() => sixWeekDates(month), [month]);
  const startDate = dates[0];
  const endDate = dates[dates.length - 1];

  useEffect(() => {
    setMonth(monthFromDate(selectedDate));
    setFocusedDate(selectedDate);
  }, [selectedDate]);
  useEffect(() => {
    if (!expanded) return;
    let active = true;
    setPending(true);
    void listNoteContentDates({ startDate, endDate })
      .then((items) => { if (active) { setContent(new Map(items.map((item) => [item.noteDate, item]))); setError(false); } })
      .catch(() => { if (active) { setContent(new Map()); setError(true); } })
      .finally(() => { if (active) setPending(false); });
    return () => { active = false; };
  }, [contentVersion, endDate, expanded, startDate]);
  useEffect(() => {
    if (!expanded) return;
    gridRef.current?.querySelector<HTMLElement>(`[data-date="${focusedDate}"]`)?.focus();
  }, [expanded, focusedDate, month]);

  const [year, oneBasedMonth] = monthParts(month);
  const monthLabel = new Intl.DateTimeFormat(language, { month: "long", year: "numeric", timeZone: "UTC" })
    .format(new Date(Date.UTC(year, oneBasedMonth - 1, 1)));
  const weekdayLabels = useMemo(() => Array.from({ length: 7 }, (_, index) =>
    new Intl.DateTimeFormat(language, { weekday: "short", timeZone: "UTC" })
      .format(new Date(Date.UTC(2026, 7, 3 + index)))), [language]);

  const selectDate = async (date: LocalDate) => {
    setPending(true);
    try {
      if (await onSelectDate(date)) {
        setMonth(monthFromDate(date));
        setFocusedDate(date);
      }
    } finally { setPending(false); }
  };

  const moveFocus = (date: LocalDate, event: React.KeyboardEvent, days: number) => {
    event.preventDefault();
    const next = shiftDate(date, days);
    if (!dates.includes(next)) setMonth(monthFromDate(next));
    setFocusedDate(next);
  };

  return <section className="notes-calendar">
    <button type="button" className="notes-calendar__toggle" aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}>
      <CalendarDays size={12} />
      {t(expanded ? "notes.calendar.hide" : "notes.calendar.show")}
      <ChevronDown size={11} className={expanded ? "is-expanded" : undefined} />
    </button>
    {expanded && <div className="notes-calendar__panel" aria-busy={pending || undefined}>
      <header>
        <button type="button" aria-label={t("notes.calendar.previous")} disabled={pending} onClick={() => setMonth((value) => shiftMonth(value, -1))}><ChevronLeft size={12} /></button>
        <strong>{monthLabel}</strong>
        <button type="button" className="notes-calendar__today" disabled={pending} onClick={() => void selectDate(today)}>{t("notes.calendar.today")}</button>
        <button type="button" aria-label={t("notes.calendar.next")} disabled={pending} onClick={() => setMonth((value) => shiftMonth(value, 1))}><ChevronRight size={12} /></button>
      </header>
      {error && <p role="alert">{t("notes.calendar.error")}</p>}
      <div className="notes-calendar__weekdays" aria-hidden="true">{weekdayLabels.map((label) => <span key={label}>{label}</span>)}</div>
      <div className="notes-calendar__grid" role="grid" ref={gridRef}>
        {dates.map((date) => {
          const summary = content.get(date);
          const hasContent = Boolean(summary?.hasText || summary?.hasRecordings);
          return <button
            type="button"
            role="gridcell"
            key={date}
            aria-label={`${date}${hasContent ? `, ${t("notes.calendar.hasContent")}` : ""}`}
            aria-pressed={date === selectedDate}
            data-date={date}
            data-has-content={hasContent || undefined}
            data-has-text={summary?.hasText || undefined}
            data-has-recordings={summary?.hasRecordings || undefined}
            data-outside-month={monthFromDate(date) !== month || undefined}
            data-today={date === today || undefined}
            disabled={pending}
            tabIndex={date === focusedDate ? 0 : -1}
            onFocus={() => setFocusedDate(date)}
            onKeyDown={(event) => {
              if (event.key === "ArrowLeft") moveFocus(date, event, -1);
              else if (event.key === "ArrowRight") moveFocus(date, event, 1);
              else if (event.key === "ArrowUp") moveFocus(date, event, -7);
              else if (event.key === "ArrowDown") moveFocus(date, event, 7);
            }}
            onClick={() => void selectDate(date)}
          >{Number(date.slice(8, 10))}</button>;
        })}
      </div>
    </div>}
  </section>;
}
