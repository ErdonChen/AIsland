import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { CommandError, MediaSnapshot } from "../../api/contracts";
import { I18nProvider } from "../../i18n/I18nProvider";
import MediaPage from "./MediaPage";

const mocks = vi.hoisted(() => ({
  beginMediaSnapshotSubscription: vi.fn(),
  dispose: vi.fn(),
  invoke: vi.fn(),
  retry: vi.fn(),
  sendMediaCommand: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("../../api/commands", () => ({ sendMediaCommand: mocks.sendMediaCommand }));
vi.mock("../../api/events", () => ({ beginMediaSnapshotSubscription: mocks.beginMediaSnapshotSubscription }));

const mediaFixture = (overrides: Partial<MediaSnapshot> = {}): MediaSnapshot => ({
  sessionId: "app.session",
  title: "Untranslated track title",
  artist: "Untranslated artist",
  playbackState: "playing",
  positionSeconds: 10,
  durationSeconds: 120,
  volumePercent: 35,
  canPlay: true,
  canPause: true,
  canPrevious: true,
  canNext: true,
  canSeek: true,
  canSetVolume: true,
  updatedAt: 1_000,
  ...overrides,
});

const commandError = (): CommandError => ({
  code: "sourceUnavailable",
  messageKey: "errors.sourceUnavailable",
  details: { reasonCode: "controlRejected" },
  retryable: true,
});

let initial: MediaSnapshot;
let deliverSnapshot: ((snapshot: MediaSnapshot) => void) | undefined;

async function renderMedia(progressTickMs = 1_000) {
  localStorage.setItem("aiceland.ui.language", "en-US");
  const user = vi.isFakeTimers()
    ? userEvent.setup({ advanceTimers: vi.advanceTimersByTime })
    : userEvent.setup();
  const view = render(<I18nProvider><MediaPage progressTickMs={progressTickMs} /></I18nProvider>);
  await act(async () => { await Promise.resolve(); await Promise.resolve(); });
  return { ...view, user };
}

beforeEach(() => {
  initial = mediaFixture();
  deliverSnapshot = undefined;
  for (const mock of Object.values(mocks)) mock.mockReset();
  mocks.invoke.mockResolvedValue(undefined);
  mocks.retry.mockResolvedValue(initial);
  mocks.sendMediaCommand.mockImplementation(async () => initial);
  mocks.beginMediaSnapshotSubscription.mockImplementation((_failure, snapshot) => {
    deliverSnapshot = snapshot;
    return {
      ready: Promise.resolve({ initial, listenerState: "active", retry: mocks.retry, dispose: mocks.dispose }),
      dispose: mocks.dispose,
    };
  });
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.useRealTimers();
});

describe("MediaPage", () => {
  it("subscribes listener-first, renders external metadata, and disposes its lifecycle", async () => {
    const view = await renderMedia();
    expect(mocks.beginMediaSnapshotSubscription).toHaveBeenCalledTimes(1);
    expect(await screen.findByRole("heading", { name: "Media" })).toBeVisible();
    expect(screen.getByText("Untranslated track title")).toBeVisible();
    expect(screen.getByText("Untranslated artist")).toBeVisible();
    view.unmount();
    expect(mocks.dispose).toHaveBeenCalledTimes(1);
  });

  it("renders the exact no-session state while keeping independent system volume usable", async () => {
    initial = mediaFixture({ sessionId: null, title: "", artist: "", playbackState: "unavailable", durationSeconds: null, volumePercent: 61, canPlay: false, canPause: false, canPrevious: false, canNext: false, canSeek: false, canSetVolume: true });
    await renderMedia();
    expect(screen.getByText("No media session is available")).toBeVisible();
    expect(screen.getByRole("slider", { name: "System volume" })).toBeEnabled();
    expect(screen.getByText("Controls the master volume of the default Windows output device")).toBeVisible();
    for (const action of ["Previous", "Play", "Pause", "Next"]) {
      expect(screen.getByRole("button", { name: action })).toBeDisabled();
    }
  });

  it("routes every capability-driven playback action through the exact union", async () => {
    const { user } = await renderMedia();
    for (const command of ["previous", "play", "pause", "next"] as const) {
      const label = command[0].toUpperCase() + command.slice(1);
      const button = screen.getByRole("button", { name: label });
      expect(button).toHaveAttribute("title", label);
      await user.click(button);
      expect(mocks.sendMediaCommand).toHaveBeenLastCalledWith({ command });
    }
  });

  it("disables each playback action from only its matching capability and exposes a tooltip", async () => {
    initial = mediaFixture({ canPrevious: false, canPlay: true, canPause: false, canNext: true });
    await renderMedia();
    expect(screen.getByRole("button", { name: "Previous" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Play" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Pause" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Next" })).toBeEnabled();
    fireEvent.mouseEnter(screen.getByRole("button", { name: "Play" }));
    expect(screen.getByRole("tooltip", { name: "Play" })).toBeVisible();
  });

  it("extrapolates progress locally and sends an exact seek only on commit", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(1_000));
    await renderMedia();
    await act(async () => { await vi.advanceTimersByTimeAsync(3_000); });
    const progress = screen.getByRole("slider", { name: "Playback progress" });
    expect(progress).toHaveValue("13");
    fireEvent.input(progress, { target: { value: "30" } });
    expect(mocks.sendMediaCommand).not.toHaveBeenCalled();
    await act(async () => {
      fireEvent.pointerUp(progress);
      await Promise.resolve();
    });
    expect(mocks.sendMediaCommand).toHaveBeenCalledWith({ command: "seek", positionSeconds: 30 });
  });

  it("commits integer system volume on blur, Enter, or Space but never on input", async () => {
    await renderMedia();
    const volume = screen.getByRole("slider", { name: "System volume" });
    fireEvent.input(volume, { target: { value: "47" } });
    expect(mocks.sendMediaCommand).not.toHaveBeenCalled();
    fireEvent.blur(volume);
    await waitFor(() => expect(mocks.sendMediaCommand).toHaveBeenLastCalledWith({ command: "setVolume", volumePercent: 47 }));
    fireEvent.input(volume, { target: { value: "48" } });
    fireEvent.keyDown(volume, { key: "Enter" });
    await waitFor(() => expect(mocks.sendMediaCommand).toHaveBeenLastCalledWith({ command: "setVolume", volumePercent: 48 }));
    fireEvent.input(volume, { target: { value: "49" } });
    fireEvent.keyDown(volume, { key: " " });
    await waitFor(() => expect(mocks.sendMediaCommand).toHaveBeenLastCalledWith({ command: "setVolume", volumePercent: 49 }));
    fireEvent.input(volume, { target: { value: "50" } });
    fireEvent.pointerUp(volume);
    await waitFor(() => expect(mocks.sendMediaCommand).toHaveBeenLastCalledWith({ command: "setVolume", volumePercent: 50 }));
  });

  it("commits seek drafts on blur, Enter, or Space without duplicate calls", async () => {
    await renderMedia();
    const progress = screen.getByRole("slider", { name: "Playback progress" });
    fireEvent.input(progress, { target: { value: "31" } });
    fireEvent.blur(progress);
    await waitFor(() => expect(mocks.sendMediaCommand).toHaveBeenLastCalledWith({ command: "seek", positionSeconds: 31 }));
    fireEvent.input(progress, { target: { value: "32" } });
    fireEvent.keyDown(progress, { key: "Enter" });
    await waitFor(() => expect(mocks.sendMediaCommand).toHaveBeenLastCalledWith({ command: "seek", positionSeconds: 32 }));
    fireEvent.input(progress, { target: { value: "33" } });
    fireEvent.keyDown(progress, { key: " " });
    await waitFor(() => expect(mocks.sendMediaCommand).toHaveBeenLastCalledWith({ command: "seek", positionSeconds: 33 }));
  });

  it("keeps the confirmed snapshot and restores slider drafts after a rejected control", async () => {
    initial = mediaFixture({ playbackState: "paused" });
    mocks.sendMediaCommand.mockRejectedValue(commandError());
    await renderMedia();
    const progress = screen.getByRole("slider", { name: "Playback progress" });
    fireEvent.input(progress, { target: { value: "80" } });
    fireEvent.pointerUp(progress);
    expect(await screen.findByText("Media control failed. Try again.")).toBeVisible();
    expect(progress).toHaveValue("10");
    expect(screen.getByText("Untranslated track title")).toBeVisible();
  });

  it("uses authoritative subscription hints and reports listener degradation locally", async () => {
    await renderMedia();
    act(() => deliverSnapshot?.(mediaFixture({ title: "Fresh external title", positionSeconds: 44 })));
    expect(screen.getByText("Fresh external title")).toBeVisible();
    const failure = mocks.beginMediaSnapshotSubscription.mock.calls[0][0] as (error: CommandError) => void;
    act(() => failure(commandError()));
    expect(screen.getByText("Windows media controls are unavailable")).toBeVisible();
  });

  it("offers a local retry for a degraded listener without reconnecting it", async () => {
    mocks.beginMediaSnapshotSubscription.mockImplementation((_failure, snapshot) => {
      deliverSnapshot = snapshot;
      return {
        ready: Promise.resolve({ initial, listenerState: "degraded", retry: mocks.retry, dispose: mocks.dispose }),
        dispose: mocks.dispose,
      };
    });
    const { user } = await renderMedia();
    expect(screen.getByText("Windows media controls are unavailable")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(mocks.retry).toHaveBeenCalledTimes(1);
    expect(mocks.beginMediaSnapshotSubscription).toHaveBeenCalledTimes(1);
  });

  it("rebuilds the current subscription from local retry when initial loading rejects", async () => {
    mocks.beginMediaSnapshotSubscription
      .mockImplementationOnce(() => ({ ready: Promise.reject(commandError()), dispose: mocks.dispose }))
      .mockImplementationOnce((_failure, snapshot) => {
        deliverSnapshot = snapshot;
        return {
          ready: Promise.resolve({ initial, listenerState: "active", retry: mocks.retry, dispose: mocks.dispose }),
          dispose: mocks.dispose,
        };
      });
    const { user } = await renderMedia();
    expect(await screen.findByText("Windows media controls are unavailable", { selector: ".media-notice span" })).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(mocks.beginMediaSnapshotSubscription).toHaveBeenCalledTimes(2));
    expect(await screen.findByText("Untranslated track title")).toBeVisible();
  });
});
