import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { NoteRecording } from "../../api/contracts";
import { I18nProvider } from "../../i18n/I18nProvider";
import NoteRecordingPanel from "./NoteRecordingPanel";

const mocks = vi.hoisted(() => ({
  abort: vi.fn(), append: vi.fn(), delete: vi.fn(), finish: vi.fn(), list: vi.fn(), read: vi.fn(), recover: vi.fn(), start: vi.fn(),
  getUserMedia: vi.fn(), invoke: vi.fn(), trackStop: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("../../api/commands", () => ({
  abortNoteRecording: mocks.abort,
  appendNoteRecordingChunk: mocks.append,
  deleteNoteRecording: mocks.delete,
  finishNoteRecording: mocks.finish,
  listNoteRecordings: mocks.list,
  readNoteRecording: mocks.read,
  recoverNoteRecordings: mocks.recover,
  startNoteRecording: mocks.start,
}));

const draft: NoteRecording = {
  id: "11111111-1111-4111-8111-111111111111", noteDate: "2026-08-08",
  mimeType: "audio/webm;codecs=opus", byteSize: 0, startedAt: 1_000,
  durationMs: 0, revision: 1, createdAt: 1_000, updatedAt: 1_000,
};
const completed: NoteRecording = { ...draft, byteSize: 4, durationMs: 1_250, revision: 2, updatedAt: 2_250 };

class FakeMediaRecorder {
  static supportsPreferred = true;
  static last: FakeMediaRecorder | null = null;
  static lastOptions: MediaRecorderOptions | undefined;
  static isTypeSupported(type: string) { return this.supportsPreferred && type === "audio/webm;codecs=opus"; }
  readonly mimeType = "audio/webm;codecs=opus";
  state: RecordingState = "inactive";
  ondataavailable: ((event: BlobEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  onstop: ((event: Event) => void) | null = null;
  constructor(_stream: MediaStream, options?: MediaRecorderOptions) {
    FakeMediaRecorder.last = this;
    FakeMediaRecorder.lastOptions = options;
  }
  start() { this.state = "recording"; }
  requestData() {
    this.ondataavailable?.({ data: new Blob([new Uint8Array([1, 2, 3, 4])], { type: this.mimeType }) } as BlobEvent);
  }
  stop() {
    this.state = "inactive";
    queueMicrotask(() => this.onstop?.(new Event("stop")));
  }
}

function renderPanel() {
  localStorage.setItem("aisland.ui.language", "en-US");
  return render(<I18nProvider><NoteRecordingPanel noteDate="2026-08-08" active /></I18nProvider>);
}

beforeEach(() => {
  mocks.invoke.mockReset().mockResolvedValue(undefined);
  vi.stubGlobal("MediaRecorder", FakeMediaRecorder);
  FakeMediaRecorder.supportsPreferred = true;
  FakeMediaRecorder.last = null;
  FakeMediaRecorder.lastOptions = undefined;
  Object.defineProperty(navigator, "mediaDevices", {
    configurable: true,
    value: { getUserMedia: mocks.getUserMedia },
  });
  mocks.getUserMedia.mockReset().mockResolvedValue({ getTracks: () => [{ stop: mocks.trackStop }] });
  mocks.trackStop.mockReset();
  mocks.start.mockReset().mockResolvedValue(draft);
  mocks.abort.mockReset().mockResolvedValue({ id: draft.id, deleted: true });
  mocks.append.mockReset().mockResolvedValue(undefined);
  mocks.finish.mockReset().mockResolvedValue(completed);
  mocks.delete.mockReset().mockResolvedValue({ id: completed.id, deleted: true });
  mocks.recover.mockReset().mockResolvedValue(0);
  mocks.list.mockReset().mockResolvedValueOnce([]).mockResolvedValue([completed]);
  mocks.read.mockReset().mockResolvedValue({ id: completed.id, mimeType: completed.mimeType, base64: "AQIDBA==" });
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.unstubAllGlobals();
});

describe("daily-note recording", () => {
  it("manually records from the default microphone, streams a chunk, and lists the completed clip", async () => {
    const user = userEvent.setup();
    renderPanel();
    await waitFor(() => expect(mocks.list).toHaveBeenCalledWith({ noteDate: "2026-08-08" }));

    await user.click(screen.getByRole("button", { name: "Start recording" }));
    expect(mocks.getUserMedia).toHaveBeenCalledWith({ audio: true });
    expect(mocks.start).toHaveBeenCalledWith(expect.objectContaining({
      noteDate: "2026-08-08", mimeType: "audio/webm;codecs=opus", fileExtension: "webm",
    }));
    expect(screen.getByRole("button", { name: "Stop recording" })).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Stop recording" }));
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    await waitFor(() => expect(mocks.append).toHaveBeenCalledWith({ id: draft.id, chunk: [1, 2, 3, 4] }));
    await waitFor(() => expect(mocks.finish).toHaveBeenCalledWith(expect.objectContaining({ id: draft.id, expectedRevision: 1 })));
    expect(mocks.trackStop).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("00:01")).toBeVisible();
  });

  it("loads a recording through the ID-only payload before playback", async () => {
    mocks.list.mockReset().mockResolvedValue([completed]);
    const user = userEvent.setup();
    renderPanel();
    await screen.findByText("00:01");

    await user.click(screen.getByRole("button", { name: "Load recording" }));

    expect(mocks.read).toHaveBeenCalledWith({ id: completed.id });
    const audio = await screen.findByLabelText("Recording 1");
    expect(audio).toHaveAttribute("src", "data:audio/webm;codecs=opus;base64,AQIDBA==");
  });

  it("deletes only the selected local recording after confirmation", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
    mocks.list.mockReset().mockResolvedValueOnce([completed]).mockResolvedValueOnce([]);
    const user = userEvent.setup();
    renderPanel();
    await screen.findByText("00:01");

    await user.click(screen.getByRole("button", { name: "Delete recording" }));

    expect(window.confirm).toHaveBeenCalledWith("Delete this local recording?");
    expect(mocks.delete).toHaveBeenCalledWith({ id: completed.id, expectedRevision: 2 });
    await waitFor(() => expect(screen.getByText("No recordings for this day")).toBeVisible());
  });

  it("falls back to the system MediaRecorder format when no preferred candidate is supported", async () => {
    FakeMediaRecorder.supportsPreferred = false;
    const user = userEvent.setup();
    renderPanel();
    await waitFor(() => expect(mocks.list).toHaveBeenCalled());

    await user.click(screen.getByRole("button", { name: "Start recording" }));

    expect(FakeMediaRecorder.lastOptions).toBeUndefined();
    expect(mocks.start).toHaveBeenCalledWith(expect.objectContaining({
      mimeType: "audio/webm;codecs=opus",
      fileExtension: "webm",
    }));
  });

  it("aborts an encoder failure instead of completing a false playable recording", async () => {
    const user = userEvent.setup();
    renderPanel();
    await waitFor(() => expect(mocks.list).toHaveBeenCalled());
    await user.click(screen.getByRole("button", { name: "Start recording" }));

    await act(async () => {
      FakeMediaRecorder.last?.onerror?.(new Event("error"));
      await Promise.resolve();
      await Promise.resolve();
    });

    await waitFor(() => expect(mocks.abort).toHaveBeenCalledWith({ id: draft.id, expectedRevision: 1 }));
    expect(mocks.finish).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent("Recording encoding failed");
  });
});
