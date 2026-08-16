import { cleanup, screen } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

const { getCurrentWindowMock } = vi.hoisted(() => ({ getCurrentWindowMock: vi.fn() }));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: getCurrentWindowMock,
}));

vi.mock("./App", () => ({
  default: () => <main>interactive island prototype</main>,
}));

vi.mock("./reminders/ReminderAlertApp", () => ({
  default: () => <main>reminder alert window</main>,
  isReminderAlertWindow: (label: string) => label === "reminder-alert",
}));

vi.mock("./i18n/I18nProvider", () => ({
  I18nProvider: ({ children }: { children: React.ReactNode }) => children,
}));

afterEach(() => {
  cleanup();
  document.body.innerHTML = "";
  delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  getCurrentWindowMock.mockReset();
  vi.resetModules();
});

test("browser preview boots the main island when Tauri window metadata is unavailable", async () => {
  document.body.innerHTML = '<div id="root"></div>';
  getCurrentWindowMock.mockImplementation(() => {
    throw new Error("must not query Tauri metadata without a Tauri runtime");
  });

  await import("./main");

  expect(await screen.findByText("interactive island prototype")).toBeInTheDocument();
  expect(screen.queryByText("reminder alert window")).not.toBeInTheDocument();
  expect(getCurrentWindowMock).not.toHaveBeenCalled();
});

test("Tauri main window boots the island", async () => {
  document.body.innerHTML = '<div id="root"></div>';
  (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
  getCurrentWindowMock.mockReturnValue({ label: "main" });

  await import("./main");

  expect(await screen.findByText("interactive island prototype")).toBeInTheDocument();
  expect(getCurrentWindowMock).toHaveBeenCalledOnce();
});

test("Tauri reminder window boots only the reminder surface", async () => {
  document.body.innerHTML = '<div id="root"></div>';
  (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
  getCurrentWindowMock.mockReturnValue({ label: "reminder-alert" });

  await import("./main");

  expect(await screen.findByText("reminder alert window")).toBeInTheDocument();
  expect(screen.queryByText("interactive island prototype")).not.toBeInTheDocument();
});

test("Tauri metadata failure does not fall back to the main island", async () => {
  document.body.innerHTML = '<div id="root"></div>';
  (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
  getCurrentWindowMock.mockImplementation(() => {
    throw new Error("native window metadata failed");
  });

  await expect(import("./main")).rejects.toThrow("native window metadata failed");
  expect(screen.queryByText("interactive island prototype")).not.toBeInTheDocument();
  expect(screen.queryByText("reminder alert window")).not.toBeInTheDocument();
});
