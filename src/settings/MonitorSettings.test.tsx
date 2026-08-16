import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

const { listWatches, saveWatch, deleteWatch, listThresholds, saveThreshold, deleteThreshold } = vi.hoisted(() => ({
  listWatches: vi.fn(), saveWatch: vi.fn(), deleteWatch: vi.fn(), listThresholds: vi.fn(), saveThreshold: vi.fn(), deleteThreshold: vi.fn(),
}));
vi.mock("../api/commands", () => ({
  listProcessWatches: listWatches,
  saveProcessWatch: saveWatch,
  deleteProcessWatch: deleteWatch,
  listMonitorThresholds: listThresholds,
  saveMonitorThreshold: saveThreshold,
  deleteMonitorThreshold: deleteThreshold,
}));

import { I18nProvider } from "../i18n/I18nProvider";
import MonitorSettings from "./MonitorSettings";
import { parseSettingsDetailId } from "./types";

beforeEach(() => { listWatches.mockResolvedValue([]); listThresholds.mockResolvedValue([]); });
afterEach(() => { cleanup(); vi.clearAllMocks(); vi.restoreAllMocks(); });

test("adds only base executable names after backend confirmation", async () => {
  const user = userEvent.setup();
  let resolveSave!: (value: unknown) => void;
  saveWatch.mockReturnValue(new Promise((resolve) => { resolveSave = resolve; }));
  render(<I18nProvider><MonitorSettings /></I18nProvider>);
  await waitFor(() => expect(listWatches).toHaveBeenCalledTimes(1));
  const input = screen.getByRole("textbox", { name: "进程名称" });
  await user.type(input, "C:\\bad.exe");
  await user.click(screen.getByRole("button", { name: "添加进程" }));
  expect(saveWatch).not.toHaveBeenCalled();
  await user.clear(input);
  await user.type(input, "worker.exe");
  await user.click(screen.getByRole("button", { name: "添加进程" }));
  expect(screen.queryByText("worker.exe")).not.toBeInTheDocument();
  resolveSave({ id: "11111111-1111-4111-8111-111111111111", processName: "worker.exe", enabled: true, revision: 1, updatedAt: 1 });
  expect(await screen.findByText("worker.exe")).toBeInTheDocument();
});

test("validates threshold route and at least one notification channel", async () => {
  expect(parseSettingsDetailId("monitorThreshold:new")).toEqual({ thresholdId: "new" });
  expect(parseSettingsDetailId("monitorThreshold:11111111-1111-4111-8111-111111111111")).toEqual({ thresholdId: "11111111-1111-4111-8111-111111111111" });
  expect(parseSettingsDetailId("monitorThreshold:C:\\bad")).toBeNull();
  render(<I18nProvider><MonitorSettings thresholdId="new" /></I18nProvider>);
  await waitFor(() => expect(listThresholds).toHaveBeenCalledTimes(1));
  await userEvent.click(screen.getByRole("checkbox", { name: "声音" }));
  await userEvent.click(screen.getByRole("checkbox", { name: "Windows 通知" }));
  await userEvent.click(screen.getByRole("checkbox", { name: "独立提醒窗口" }));
  expect(screen.getByRole("alert")).toHaveTextContent("至少选择一种提醒方式");
  expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
});

test("keeps process mutations backend-confirmed and requires delete confirmation", async () => {
  const user = userEvent.setup();
  const watch = { id: "11111111-1111-4111-8111-111111111111", processName: "worker.exe", enabled: true, revision: 1, updatedAt: 1 };
  listWatches.mockResolvedValue([watch]);
  let resolveToggle!: (value: unknown) => void;
  saveWatch.mockReturnValue(new Promise((resolve) => { resolveToggle = resolve; }));
  const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
  render(<I18nProvider><MonitorSettings /></I18nProvider>);
  expect(await screen.findByText("worker.exe")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "已启用" }));
  expect(screen.queryByRole("button", { name: "已停用" })).not.toBeInTheDocument();
  resolveToggle({ ...watch, enabled: false, revision: 2, updatedAt: 2 });
  expect(await screen.findByRole("button", { name: "已停用" })).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "删除进程" }));
  expect(deleteWatch).not.toHaveBeenCalled();
  confirm.mockReturnValue(true);
  deleteWatch.mockResolvedValue({ id: watch.id, deleted: true });
  await user.click(screen.getByRole("button", { name: "删除进程" }));
  await waitFor(() => expect(screen.queryByText("worker.exe")).not.toBeInTheDocument());
});

test("rebases a threshold conflict without overwriting the edited draft", async () => {
  const user = userEvent.setup();
  const id = "11111111-1111-4111-8111-111111111111";
  const row = { id, metric: "cpuPercent", comparator: "greaterThanOrEqual", thresholdValue: 90, holdSeconds: 30, cooldownSeconds: 300, sound: { kind: "builtin", soundId: "systemNotification" }, toastEnabled: true, windowEnabled: true, enabled: true, revision: 1, updatedAt: 1 } as const;
  const rebased = { ...row, thresholdValue: 77, revision: 2, updatedAt: 2 };
  listThresholds.mockResolvedValueOnce([row]).mockResolvedValueOnce([rebased]);
  saveThreshold.mockRejectedValueOnce({ code: "conflict", messageKey: "errors.conflict", details: { entityId: id }, retryable: true });
  render(<I18nProvider><MonitorSettings thresholdId={id} /></I18nProvider>);
  const value = await screen.findByRole("spinbutton", { name: "阈值" });
  await user.clear(value);
  await user.type(value, "88");
  await user.click(screen.getByRole("button", { name: "保存" }));
  await waitFor(() => expect(listThresholds).toHaveBeenCalledTimes(2));
  expect(value).toHaveValue(88);
  const saved = { ...rebased, thresholdValue: 88, revision: 3, updatedAt: 3 };
  saveThreshold.mockResolvedValueOnce(saved);
  await waitFor(() => expect(screen.getByRole("button", { name: "保存" })).toBeEnabled());
  await user.click(screen.getByRole("button", { name: "保存" }));
  await waitFor(() => expect(saveThreshold).toHaveBeenLastCalledWith(expect.objectContaining({ id, expectedRevision: 2, thresholdValue: 88 })));
});

test("does not allow an existing threshold route to create before identity is loaded", async () => {
  const id = "11111111-1111-4111-8111-111111111111";
  let resolveThresholds!: (value: unknown[]) => void;
  listThresholds.mockReturnValue(new Promise((resolve) => { resolveThresholds = resolve; }));
  render(<I18nProvider><MonitorSettings thresholdId={id} /></I18nProvider>);
  expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
  resolveThresholds([]);
  await waitFor(() => expect(screen.getByRole("button", { name: "保存" })).toBeDisabled());
  expect(saveThreshold).not.toHaveBeenCalled();
});

test("does not let a stale initial list erase a confirmed process creation", async () => {
  const user = userEvent.setup();
  let resolveInitial!: (value: unknown[]) => void;
  listWatches.mockReturnValue(new Promise((resolve) => { resolveInitial = resolve; }));
  saveWatch.mockResolvedValue({ id: "11111111-1111-4111-8111-111111111111", processName: "worker.exe", enabled: true, revision: 1, updatedAt: 1 });
  render(<I18nProvider><MonitorSettings /></I18nProvider>);
  await user.type(screen.getByRole("textbox", { name: "进程名称" }), "worker.exe");
  await user.click(screen.getByRole("button", { name: "添加进程" }));
  expect(await screen.findByText("worker.exe")).toBeInTheDocument();
  await act(async () => {
    resolveInitial([]);
    await Promise.resolve();
  });
  expect(screen.getByText("worker.exe")).toBeInTheDocument();
});
