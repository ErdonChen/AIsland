import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { I18nProvider } from "../../i18n/I18nProvider";
import NoteCalendar from "./NoteCalendar";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), list: vi.fn(), select: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("../../api/commands", () => ({ listNoteContentDates: mocks.list }));

beforeEach(() => {
  localStorage.setItem("aisland.ui.language", "en-US");
  mocks.invoke.mockReset().mockResolvedValue(undefined);
  mocks.list.mockReset().mockImplementation(async ({ startDate }) => startDate === "2026-07-27" ? [
    { noteDate: "2026-08-08", hasText: true, hasRecordings: false },
    { noteDate: "2026-08-09", hasText: false, hasRecordings: true },
  ] : []);
  mocks.select.mockReset().mockResolvedValue(true);
});

afterEach(() => { cleanup(); localStorage.clear(); vi.restoreAllMocks(); });

describe("daily-note calendar", () => {
  it("opens on the selected month, marks content dates, and switches by clicking a day", async () => {
    const user = userEvent.setup();
    render(<I18nProvider><NoteCalendar selectedDate="2026-08-08" today="2026-08-20" onSelectDate={mocks.select} /></I18nProvider>);

    expect(screen.queryByText("August 2026")).not.toBeInTheDocument();
    await user.click(await screen.findByRole("button", { name: "Show calendar" }));
    expect(await screen.findByText("August 2026")).toBeVisible();
    await waitFor(() => expect(mocks.list).toHaveBeenCalledWith({ startDate: "2026-07-27", endDate: "2026-09-06" }));

    const recordingOnlyDay = screen.getByRole("gridcell", { name: "2026-08-09, has content" });
    expect(recordingOnlyDay).toHaveAttribute("data-has-content", "true");
    expect(recordingOnlyDay).toHaveAttribute("data-has-recordings", "true");
    expect(screen.getAllByRole("gridcell")).toHaveLength(42);
    expect(screen.getByRole("gridcell", { name: "2026-07-27" })).toHaveAttribute("data-outside-month", "true");
    await user.click(recordingOnlyDay);
    expect(mocks.select).toHaveBeenCalledWith("2026-08-09");
  });

  it("navigates months without changing the selected note", async () => {
    const user = userEvent.setup();
    render(<I18nProvider><NoteCalendar selectedDate="2026-08-08" today="2026-08-20" onSelectDate={mocks.select} /></I18nProvider>);
    await user.click(await screen.findByRole("button", { name: "Show calendar" }));
    await screen.findByText("August 2026");

    await user.click(screen.getByRole("button", { name: "Previous month" }));

    expect(await screen.findByText("July 2026")).toBeVisible();
    expect(mocks.list).toHaveBeenCalledWith({ startDate: "2026-06-29", endDate: "2026-08-09" });
    expect(mocks.select).not.toHaveBeenCalled();
  });

  it("supports keyboard day navigation and a safe return-to-today action", async () => {
    const user = userEvent.setup();
    render(<I18nProvider><NoteCalendar selectedDate="2026-08-08" today="2026-08-20" onSelectDate={mocks.select} /></I18nProvider>);
    await user.click(await screen.findByRole("button", { name: "Show calendar" }));
    const selected = await screen.findByRole("gridcell", { name: "2026-08-08, has content" });
    selected.focus();

    await user.keyboard("{ArrowRight}{Enter}");
    expect(mocks.select).toHaveBeenCalledWith("2026-08-09");

    await user.click(screen.getByRole("button", { name: "Today" }));
    expect(mocks.select).toHaveBeenLastCalledWith("2026-08-20");
  });

  it("reloads authoritative content markers when the parent invalidates the month", async () => {
    mocks.list.mockReset()
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([{ noteDate: "2026-08-08", hasText: false, hasRecordings: true }]);
    const user = userEvent.setup();
    const view = render(<I18nProvider><NoteCalendar selectedDate="2026-08-08" contentVersion={0} onSelectDate={mocks.select} /></I18nProvider>);
    await user.click(await screen.findByRole("button", { name: "Show calendar" }));
    await waitFor(() => expect(mocks.list).toHaveBeenCalledTimes(1));
    expect(screen.getByRole("gridcell", { name: "2026-08-08" })).not.toHaveAttribute("data-has-content");

    view.rerender(<I18nProvider><NoteCalendar selectedDate="2026-08-08" contentVersion={1} onSelectDate={mocks.select} /></I18nProvider>);

    expect(await screen.findByRole("gridcell", { name: "2026-08-08, has content" })).toHaveAttribute("data-has-recordings", "true");
  });
});
