import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

const { beginSubscription, getSnapshot, listSamples, listMetrics, listWatches } = vi.hoisted(() => ({
  beginSubscription: vi.fn(),
  getSnapshot: vi.fn(),
  listSamples: vi.fn(),
  listMetrics: vi.fn(),
  listWatches: vi.fn(),
}));

vi.mock("../api/events", () => ({ beginMonitorMetricsSubscription: beginSubscription }));
vi.mock("../api/commands", () => ({
  getMonitorSnapshot: getSnapshot,
  listMonitorSamples: listSamples,
  listProcessMetrics: listMetrics,
  listProcessWatches: listWatches,
}));

import { I18nProvider } from "../i18n/I18nProvider";
import MonitorPage from "./MonitorPage";

const snapshot = {
  cpuPercent: 25.55,
  memoryUsedBytes: 4_000,
  memoryTotalBytes: 8_000,
  diskReadBytesPerSecond: 100,
  diskWriteBytesPerSecond: 200,
  networkReceiveBytesPerSecond: 300,
  networkSendBytesPerSecond: 400,
  gpuPercent: null,
  sampledAt: 2_000_000,
};

beforeEach(() => {
  vi.spyOn(Date, "now").mockReturnValue(2_000_000);
  getSnapshot.mockResolvedValue(snapshot);
  listSamples.mockResolvedValue([snapshot]);
  listMetrics.mockResolvedValue([{ pid: 42, processName: "very-long-process-name.exe", cpuPercent: 4, memoryBytes: 2048, sampledAt: 2_000_000 }]);
  listWatches.mockResolvedValue([{ id: "watch-1", processName: "very-long-process-name.exe", enabled: true, revision: 1, updatedAt: 1 }]);
  beginSubscription.mockImplementation((_onError, onSnapshot) => {
    queueMicrotask(() => onSnapshot?.(snapshot));
    return { ready: Promise.resolve({ initial: snapshot, listenerState: "active", retry: vi.fn(), dispose: vi.fn() }), dispose: vi.fn() };
  });
});

afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.clearAllMocks(); });

test("subscribes before querying and renders current cards with explicit unavailable GPU", async () => {
  render(<I18nProvider><MonitorPage /></I18nProvider>);
  await waitFor(() => expect(listSamples).toHaveBeenCalledWith({ since: 1_100_000, limit: 450 }));
  expect(beginSubscription.mock.invocationCallOrder[0]).toBeLessThan(listSamples.mock.invocationCallOrder[0]);
  expect(listMetrics).toHaveBeenCalledWith({ limit: 450 });
  expect(screen.getByRole("heading", { name: "系统监控" })).toBeInTheDocument();
  expect(within(screen.getByRole("article", { name: "CPU" })).getByText("25.6%")).toBeInTheDocument();
  expect(screen.getByText("GPU 数据源不可用")).toBeInTheDocument();
  expect(screen.queryByText("0.0%", { selector: "[data-metric='gpu'] *" })).not.toBeInTheDocument();
  expect(screen.getByText("very-long-process-name.exe")).toBeInTheDocument();
});

test("caps and sorts authoritative trend samples and disposes its subscription", async () => {
  const dispose = vi.fn();
  beginSubscription.mockImplementation((_onError, onSnapshot) => {
    queueMicrotask(() => onSnapshot?.(snapshot));
    return { ready: Promise.resolve({ initial: snapshot, listenerState: "active", retry: vi.fn(), dispose }), dispose };
  });
  listSamples.mockResolvedValue(Array.from({ length: 451 }, (_, index) => ({ ...snapshot, sampledAt: 2_000_000 - index, cpuPercent: index })));
  const view = render(<I18nProvider><MonitorPage /></I18nProvider>);
  await waitFor(() => expect(listSamples).toHaveBeenCalled());
  const firstTrend = screen.getByRole("figure", { name: "CPU — 最近 15 分钟" });
  expect(firstTrend.querySelectorAll(".metric-trend__points [data-trend-point]")).toHaveLength(450);
  view.unmount();
  expect(dispose).toHaveBeenCalledTimes(1);
});

test("retains every latest PID for a configured same-name process", async () => {
  listMetrics.mockResolvedValue([
    { pid: 10, processName: "worker.exe", cpuPercent: 1, memoryBytes: 100, sampledAt: 2_000_000 },
    { pid: 20, processName: "worker.exe", cpuPercent: 2, memoryBytes: 200, sampledAt: 2_000_000 },
  ]);
  listWatches.mockResolvedValue([{ id: "watch-1", processName: "worker.exe", enabled: true, revision: 1, updatedAt: 1 }]);
  render(<I18nProvider><MonitorPage /></I18nProvider>);
  expect(await screen.findByText("PID 10")).toBeInTheDocument();
  expect(screen.getByText("PID 20")).toBeInTheDocument();
});

test("offers a lifecycle-safe local Retry after an initial snapshot failure", async () => {
  const retry = vi.fn().mockResolvedValue(undefined);
  beginSubscription.mockImplementation((onError) => {
    queueMicrotask(() => onError({ code: "sourceUnavailable", messageKey: "errors.sourceUnavailable", details: { serviceId: "monitor", reasonCode: "empty" }, retryable: true }));
    return { ready: Promise.resolve({ initial: null, listenerState: "active", retry, dispose: vi.fn() }), dispose: vi.fn() };
  });
  render(<I18nProvider><MonitorPage /></I18nProvider>);
  const button = await screen.findByRole("button", { name: "重试" });
  await button.click();
  await waitFor(() => expect(retry).toHaveBeenCalledTimes(1));
});

test("keeps degraded listener recovery visible until Retry reports an active listener", async () => {
  const retry = vi.fn().mockResolvedValue(undefined);
  const subscription = { initial: snapshot, listenerState: "degraded" as "active" | "degraded", retry, dispose: vi.fn() };
  beginSubscription.mockImplementation((onError, onSnapshot) => {
    queueMicrotask(() => {
      onError({ code: "sourceUnavailable", messageKey: "errors.sourceUnavailable", details: { serviceId: "monitor", reasonCode: "listener" }, retryable: true });
      onSnapshot?.(snapshot);
    });
    return { ready: Promise.resolve(subscription), dispose: vi.fn() };
  });
  render(<I18nProvider><MonitorPage /></I18nProvider>);
  const button = await screen.findByRole("button", { name: "重试" });
  await button.click();
  await waitFor(() => expect(retry).toHaveBeenCalledTimes(1));
  expect(screen.getByRole("button", { name: "重试" })).toBeInTheDocument();
});
