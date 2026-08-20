import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode, useState } from "react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

const { invokeMock, subscribeServiceHealthMock, beginServiceHealthSubscriptionMock, getDiagnosticsMock, checkStorageIntegrityMock, listServiceHealthMock, listAgentIntegrationProfilesMock, saveAgentIntegrationProfileMock, installAgentIntegrationProfileMock, repairAgentIntegrationProfileMock, uninstallAgentIntegrationProfileMock, deleteAgentIntegrationProfileMock, listReminderRulesMock, saveReminderRuleMock, deleteReminderRuleMock, getGeneralSettingsMock, saveGeneralSettingsMock, checkForUpdateMock, installUpdateMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  subscribeServiceHealthMock: vi.fn(),
  beginServiceHealthSubscriptionMock: vi.fn(),
  getDiagnosticsMock: vi.fn(),
  checkStorageIntegrityMock: vi.fn(),
  listServiceHealthMock: vi.fn(),
  listAgentIntegrationProfilesMock: vi.fn(),
  saveAgentIntegrationProfileMock: vi.fn(),
  installAgentIntegrationProfileMock: vi.fn(),
  repairAgentIntegrationProfileMock: vi.fn(),
  uninstallAgentIntegrationProfileMock: vi.fn(),
  deleteAgentIntegrationProfileMock: vi.fn(),
  listReminderRulesMock: vi.fn(),
  saveReminderRuleMock: vi.fn(),
  deleteReminderRuleMock: vi.fn(),
  getGeneralSettingsMock: vi.fn(),
  saveGeneralSettingsMock: vi.fn(),
  checkForUpdateMock: vi.fn(),
  installUpdateMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("../../api/events", () => ({
  subscribeServiceHealth: subscribeServiceHealthMock,
  beginServiceHealthSubscription: beginServiceHealthSubscriptionMock,
}));
vi.mock("../../api/commands", () => ({
  getDiagnostics: getDiagnosticsMock,
  checkStorageIntegrity: checkStorageIntegrityMock,
  listServiceHealth: listServiceHealthMock,
  listAgentIntegrationProfiles: listAgentIntegrationProfilesMock,
  saveAgentIntegrationProfile: saveAgentIntegrationProfileMock,
  installAgentIntegrationProfile: installAgentIntegrationProfileMock,
  repairAgentIntegrationProfile: repairAgentIntegrationProfileMock,
  uninstallAgentIntegrationProfile: uninstallAgentIntegrationProfileMock,
  deleteAgentIntegrationProfile: deleteAgentIntegrationProfileMock,
  listReminderRules: listReminderRulesMock,
  saveReminderRule: saveReminderRuleMock,
  deleteReminderRule: deleteReminderRuleMock,
  getGeneralSettings: getGeneralSettingsMock,
  saveGeneralSettings: saveGeneralSettingsMock,
  checkForUpdate: checkForUpdateMock,
  installUpdate: installUpdateMock,
}));
vi.mock("../../api/dialog", () => ({ chooseLocalAudioFile: vi.fn() }));
vi.mock("../../settings/MonitorSettings", () => ({
  default: ({ thresholdId, onSelectThreshold }: { thresholdId?: string; onSelectThreshold?: (id: string) => void }) => thresholdId
    ? <div data-testid="monitor-threshold-detail">{thresholdId}</div>
    : <button type="button" onClick={() => onSelectThreshold?.("new")}>New monitor threshold</button>,
}));

import { I18nProvider, useI18n } from "../../i18n/I18nProvider";
import SettingRow from "./SettingRow";
import SettingsView from "./SettingsView";

type SettingsHarnessProps = {
  onExitSettings?: () => void;
  routeResetToken?: number | null;
  entrySequence?: number | null;
  onEntryHandled?: (sequence: number) => void;
};

function SettingsHarness({
  onExitSettings = vi.fn(),
  routeResetToken,
  entrySequence,
  onEntryHandled,
}: SettingsHarnessProps) {
  const [scale, setScale] = useState(1);
  const [glassTransparency, setGlassTransparency] = useState(58);
  const [backgroundColor, setBackgroundColor] = useState<"midnight" | "ocean" | "graphite" | "pine" | "nebula" | "rock">("midnight");
  const [expansionMotion, setExpansionMotion] = useState<"elastic" | "smooth" | "swift">("elastic");
  const [compactWindowEnabled, setCompactWindowEnabled] = useState(true);
  const [notificationPopupEnabled, setNotificationPopupEnabled] = useState(true);

  return (
    <SettingsView
      scale={scale}
      onScaleChange={setScale}
      glassTransparency={glassTransparency}
      onGlassTransparencyChange={setGlassTransparency}
      backgroundColor={backgroundColor}
      onBackgroundColorChange={setBackgroundColor}
      expansionMotion={expansionMotion}
      onExpansionMotionChange={setExpansionMotion}
      onPreviewExpansionMotion={() => Promise.resolve()}
      compactWindowEnabled={compactWindowEnabled}
      onCompactWindowEnabledChange={setCompactWindowEnabled}
      notificationPopupEnabled={notificationPopupEnabled}
      onNotificationPopupEnabledChange={setNotificationPopupEnabled}
      onExitSettings={onExitSettings}
      routeResetToken={routeResetToken}
      entrySequence={entrySequence}
      onEntryHandled={onEntryHandled}
    />
  );
}

function renderSettings(onExitSettings = vi.fn()) {
  return render(
    <I18nProvider>
      <SettingsHarness onExitSettings={onExitSettings} />
    </I18nProvider>,
  );
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
}

function ExternalValueFixture() {
  const { setLanguage, t } = useI18n();
  const agentName = "WorkBuddy-Pro";

  return (
    <>
      <SettingRow label={t("settings.category.agents")} summary={agentName} readOnly />
      <button type="button" onClick={() => void setLanguage("en-US")}>
        English
      </button>
    </>
  );
}

afterEach(() => {
  cleanup();
  invokeMock.mockReset();
  subscribeServiceHealthMock.mockReset();
  beginServiceHealthSubscriptionMock.mockReset();
  getDiagnosticsMock.mockReset();
  checkStorageIntegrityMock.mockReset();
  listServiceHealthMock.mockReset();
  listAgentIntegrationProfilesMock.mockReset();
  saveAgentIntegrationProfileMock.mockReset();
  installAgentIntegrationProfileMock.mockReset();
  repairAgentIntegrationProfileMock.mockReset();
  uninstallAgentIntegrationProfileMock.mockReset();
  deleteAgentIntegrationProfileMock.mockReset();
  listReminderRulesMock.mockReset();
  saveReminderRuleMock.mockReset();
  deleteReminderRuleMock.mockReset();
  getGeneralSettingsMock.mockReset();
  saveGeneralSettingsMock.mockReset();
  checkForUpdateMock.mockReset();
  installUpdateMock.mockReset();
  localStorage.clear();
});

beforeEach(() => {
  getGeneralSettingsMock.mockResolvedValue({ launchAtStartup: false, revision: 1, updatedAt: 1 });
  listAgentIntegrationProfilesMock.mockResolvedValue([]);
  beginServiceHealthSubscriptionMock.mockImplementation((onListenerFailure: unknown, onSnapshot: unknown) => ({
    dispose: vi.fn(),
    ready: subscribeServiceHealthMock(onListenerFailure, onSnapshot),
  }));
});

test("commits launch-at-startup through versioned general settings", async () => {
  saveGeneralSettingsMock.mockResolvedValue({ launchAtStartup: true, revision: 2, updatedAt: 2 });
  const user = userEvent.setup();
  renderSettings();

  await user.click(screen.getByRole("button", { name: "通用" }));
  const startupSwitch = await screen.findByRole("switch", { name: "开机启动" });
  expect(startupSwitch).toHaveAttribute("aria-checked", "false");
  await user.click(startupSwitch);

  expect(saveGeneralSettingsMock).toHaveBeenCalledWith({ launchAtStartup: true, expectedRevision: 1 });
  await waitFor(() => expect(startupSwitch).toHaveAttribute("aria-checked", "true"));
});

test("checks and installs an available signed update from the About page", async () => {
  checkForUpdateMock.mockResolvedValue({ status: "available", currentVersion: "0.1.0", latestVersion: "0.2.0", notes: "Release" });
  installUpdateMock.mockImplementation(async (onEvent: (event: { event: string; downloaded?: number; total?: number }) => void) => {
    onEvent({ event: "started", downloaded: 0, total: 100 });
    onEvent({ event: "progress", downloaded: 100, total: 100 });
    onEvent({ event: "finished", downloaded: 100, total: 100 });
    return { installedVersion: "0.2.0", restartRequired: true };
  });
  const user = userEvent.setup();
  renderSettings();

  await user.click(screen.getByRole("button", { name: "关于 AIsland" }));
  await user.click(screen.getByRole("button", { name: "检查更新" }));

  await waitFor(() => expect(installUpdateMock).toHaveBeenCalledTimes(1));
  expect(await screen.findByText("0.2.0 已安装，重启后生效")).toBeInTheDocument();
});

test("opens a category and returns exactly one level with the back control or Escape", async () => {
  const user = userEvent.setup();
  renderSettings();

  await user.click(screen.getByRole("button", { name: "通用" }));
  expect(screen.getByRole("heading", { name: "通用" })).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "返回" }));
  expect(screen.getByRole("button", { name: "通用" })).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "显示与外观" }));
  await user.keyboard("{Escape}");
  expect(screen.getByRole("button", { name: "显示与外观" })).toBeInTheDocument();
});

test("updates the glass transparency control immediately", async () => {
  const user = userEvent.setup();
  renderSettings();

  await user.click(screen.getByRole("button", { name: "显示与外观" }));
  const slider = screen.getByRole("slider", { name: "玻璃透明度" });
  expect(slider).toHaveValue("58");
  expect(slider).toHaveStyle({ "--range-progress": "58%" });

  fireEvent.change(slider, { target: { value: "82" } });

  expect(slider).toHaveValue("82");
  expect(slider).toHaveStyle({ "--range-progress": "82%" });
  expect(screen.getByText("82%")).toBeInTheDocument();
});

test("keeps glass transparency first and motion preview compact until requested", async () => {
  const user = userEvent.setup();
  renderSettings();

  await user.click(screen.getByRole("button", { name: "显示与外观" }));

  const glassSlider = screen.getByRole("slider", { name: "玻璃透明度" });
  const scaleSlider = screen.getByRole("slider", { name: "窗口缩放" });
  expect(glassSlider.compareDocumentPosition(scaleSlider) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(screen.queryByTestId("agent-state-motion-preview")).not.toBeInTheDocument();

  const previewToggle = screen.getByRole("button", { name: "状态动效预览" });
  expect(previewToggle).toHaveAttribute("aria-expanded", "false");
  await user.click(previewToggle);
  expect(previewToggle).toHaveAttribute("aria-expanded", "true");
  expect(screen.getByTestId("agent-state-motion-preview")).toBeInTheDocument();
});

test("selects a production expansion motion and previews working or idle state", async () => {
  const user = userEvent.setup();
  renderSettings();

  await user.click(screen.getByRole("button", { name: "显示与外观" }));

  const motionGroup = screen.getByRole("group", { name: "展开动效" });
  expect(screen.getByRole("button", { name: "iOS 弹性" })).toHaveAttribute("aria-pressed", "true");
  await user.click(screen.getByRole("button", { name: "柔和舒展" }));
  expect(motionGroup).toContainElement(screen.getByRole("button", { name: "柔和舒展" }));
  expect(screen.getByRole("button", { name: "柔和舒展" })).toHaveAttribute("aria-pressed", "true");

  await user.click(screen.getByRole("button", { name: "状态动效预览" }));
  const preview = screen.getByTestId("agent-state-motion-preview");
  expect(preview).toHaveAttribute("data-preview-status", "idle");
  await user.click(screen.getByRole("button", { name: "工作中" }));
  expect(preview).toHaveAttribute("data-preview-status", "working");
  await user.click(screen.getByRole("button", { name: "空闲" }));
  expect(preview).toHaveAttribute("data-preview-status", "idle");
});

test("exposes independent compact-window and notification-popup switches", async () => {
  listReminderRulesMock.mockResolvedValue([]);
  const user = userEvent.setup();
  renderSettings();

  await user.click(screen.getByRole("button", { name: "显示与外观" }));
  const compactSwitch = screen.getByRole("switch", { name: "收缩小窗" });
  expect(compactSwitch).toHaveAttribute("aria-checked", "true");
  await user.click(compactSwitch);
  expect(compactSwitch).toHaveAttribute("aria-checked", "false");

  await user.click(screen.getByRole("button", { name: "返回" }));
  await user.click(screen.getByRole("button", { name: "提醒与通知" }));
  const popupSwitch = screen.getByRole("switch", { name: "系统通知弹窗" });
  expect(popupSwitch).toHaveAttribute("aria-checked", "true");
  await user.click(popupSwitch);
  expect(popupSwitch).toHaveAttribute("aria-checked", "false");
});

test("opens About AIsland with GitHub and local README actions", async () => {
  invokeMock.mockResolvedValue(undefined);
  const user = userEvent.setup();
  renderSettings();

  await user.click(screen.getByRole("button", { name: "关于 AIsland" }));

  expect(screen.getByRole("heading", { name: "关于 AIsland" })).toBeInTheDocument();
  expect(screen.getByRole("img", { name: "AIsland" })).toBeVisible();
  expect(screen.getByText("https://github.com/ErdonChen/AIsland")).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "打开 GitHub" }));
  expect(invokeMock).toHaveBeenCalledWith("open_aisland_github");

  await user.click(screen.getByRole("button", { name: "使用说明" }));
  expect(invokeMock).toHaveBeenCalledWith("open_project_readme");
});

test("routes monitor thresholds through the existing diagnostics hierarchy", async () => {
  const user = userEvent.setup();
  subscribeServiceHealthMock.mockResolvedValue({ initial: [], listenerState: "active", retry: vi.fn(), dispose: vi.fn() });
  getDiagnosticsMock.mockResolvedValue([]);
  checkStorageIntegrityMock.mockResolvedValue({ integrity: "ok", schemaVersion: 5, checkedAt: 1 });
  renderSettings();
  await user.click(screen.getByRole("button", { name: "诊断" }));
  await user.click(await screen.findByRole("button", { name: "New monitor threshold" }));
  expect(screen.getByRole("heading", { name: "新建阈值" })).toBeInTheDocument();
  expect(screen.getByTestId("monitor-threshold-detail")).toHaveTextContent("new");
  await user.keyboard("{Escape}");
  expect(screen.getByRole("heading", { name: "诊断" })).toBeInTheDocument();
});

test("routes available reminder rules through the central category and typed new-rule detail", async () => {
  const user = userEvent.setup();
  listReminderRulesMock.mockResolvedValue([]);
  renderSettings();

  await user.click(screen.getByRole("button", { name: "提醒与通知" }));
  expect(await screen.findByRole("button", { name: "新建规则" })).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "新建规则" }));
  expect(screen.getByRole("heading", { name: "新建规则" })).toBeInTheDocument();
});

test("does not let an older reminder-list result erase a successful save", async () => {
  const user = userEvent.setup();
  const initialList = deferred<unknown[]>();
  listReminderRulesMock.mockReturnValue(initialList.promise);
  const savedRule = { id: "7ac3ef17-5e3f-4b6a-af0a-8b907703e768", agentIds: ["codex"], triggerStatuses: ["completed"], delaySeconds: 0, sound: { kind: "none" }, toastEnabled: true, windowEnabled: false, enabled: true, revision: 1, createdAt: 1, updatedAt: 1 };
  saveReminderRuleMock.mockResolvedValue(savedRule);
  renderSettings();

  await user.click(screen.getByRole("button", { name: "提醒与通知" }));
  await user.click(await screen.findByRole("button", { name: "新建规则" }));
  await user.click(screen.getByRole("button", { name: "保存" }));
  expect(await screen.findByRole("heading", { name: "提醒规则" })).toBeInTheDocument();

  await act(async () => {
    initialList.resolve([]);
    await initialList.promise;
  });
  expect(screen.getByRole("heading", { name: "提醒规则" })).toBeInTheDocument();
});

test("ignores a deferred new-rule save after Escape navigates away", async () => {
  const user = userEvent.setup();
  const save = deferred<unknown>();
  listReminderRulesMock.mockResolvedValue([]);
  saveReminderRuleMock.mockReturnValue(save.promise);
  renderSettings();

  await user.click(screen.getByRole("button", { name: "提醒与通知" }));
  await user.click(await screen.findByRole("button", { name: "新建规则" }));
  await user.click(screen.getByRole("button", { name: "保存" }));
  await user.keyboard("{Escape}");
  expect(screen.getByRole("heading", { name: "提醒与通知" })).toBeInTheDocument();
  await act(async () => {
    save.resolve({ id: "7ac3ef17-5e3f-4b6a-af0a-8b907703e768", agentIds: ["codex"], triggerStatuses: ["completed"], delaySeconds: 0, sound: { kind: "none" }, toastEnabled: true, windowEnabled: false, enabled: true, revision: 1, createdAt: 1, updatedAt: 1 });
    await save.promise;
  });
  expect(screen.getByRole("heading", { name: "提醒与通知" })).toBeInTheDocument();
});

test("ignores a deferred delete after Escape leaves reminder settings", async () => {
  const user = userEvent.setup();
  const deletion = deferred<unknown>();
  const rule = { id: "7ac3ef17-5e3f-4b6a-af0a-8b907703e768", agentIds: ["codex"], triggerStatuses: ["completed"], delaySeconds: 0, sound: { kind: "none" }, toastEnabled: true, windowEnabled: false, enabled: true, revision: 1, createdAt: 1, updatedAt: 1 };
  listReminderRulesMock.mockResolvedValue([rule]);
  deleteReminderRuleMock.mockReturnValue(deletion.promise);
  vi.stubGlobal("confirm", vi.fn().mockReturnValue(true));
  renderSettings();

  await user.click(screen.getByRole("button", { name: "提醒与通知" }));
  await user.click(await screen.findByRole("button", { name: "codex" }));
  await user.click(screen.getByRole("button", { name: "删除规则" }));
  await user.keyboard("{Escape}");
  await user.keyboard("{Escape}");
  expect(screen.getByRole("button", { name: "通用" })).toBeInTheDocument();
  await act(async () => {
    deletion.resolve({ id: rule.id, deleted: true });
    await deletion.promise;
  });
  expect(screen.getByRole("button", { name: "通用" })).toBeInTheDocument();
});

test("gives the icon-only Back control the localized tooltip and accessible name", async () => {
  const user = userEvent.setup();
  renderSettings();

  await user.click(screen.getByRole("button", { name: "通用" }));
  expect(screen.getByRole("button", { name: "返回" })).toHaveAttribute("title", "返回");
});

test("exits settings from the root on Escape without changing shell state", async () => {
  const user = userEvent.setup();
  const onExitSettings = vi.fn();
  renderSettings(onExitSettings);

  await user.keyboard("{Escape}");

  expect(onExitSettings).toHaveBeenCalledTimes(1);
});

test("keeps the current category route while a real language selection updates labels", async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValue(undefined);
  renderSettings();

  await user.click(screen.getByRole("button", { name: "通用" }));
  await user.click(screen.getByRole("button", { name: "English" }));

  await waitFor(() => {
    expect(screen.getByRole("heading", { name: "General" })).toBeInTheDocument();
  });
  expect(invokeMock).toHaveBeenCalledWith("set_ui_language", { language: "en-US" });

  await user.click(screen.getByRole("button", { name: "Back" }));
  const display = screen.getByRole("button", { name: "Display & Appearance" });
  expect(display).toHaveAttribute("title", "Display & Appearance");
  expect(display).toHaveAttribute("aria-label", "Display & Appearance");
});

test("changes the animation node for a route transition but not for a language rerender", async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValue(undefined);
  renderSettings();

  const rootPage = screen.getByRole("region", { name: "设置" });
  await user.click(screen.getByRole("button", { name: "通用" }));
  const generalPage = screen.getByRole("region", { name: "通用" });
  expect(generalPage).not.toBe(rootPage);

  await user.click(screen.getByRole("button", { name: "English" }));
  await waitFor(() => {
    expect(screen.getByRole("heading", { name: "General" })).toBeInTheDocument();
  });
  expect(screen.getByRole("region", { name: "General" })).toBe(generalPage);
});

test("a new tray settings entry resets a nested route and only acknowledges at root", async () => {
  const user = userEvent.setup();
  const rootWhenHandled: boolean[] = [];
  const onEntryHandled = vi.fn(() => {
    rootWhenHandled.push(screen.queryByRole("button", { name: "通用" }) !== null);
  });
  const onExitSettings = vi.fn();
  const view = (entrySequence: number | null) => (
    <I18nProvider>
      <SettingsHarness
        routeResetToken={entrySequence}
        entrySequence={entrySequence}
        onEntryHandled={onEntryHandled}
        onExitSettings={onExitSettings}
      />
    </I18nProvider>
  );
  const { rerender } = render(view(null));

  await user.click(screen.getByRole("button", { name: "显示与外观" }));
  expect(screen.getByRole("heading", { name: "显示与外观" })).toBeInTheDocument();

  rerender(view(42));
  await waitFor(() => {
    expect(screen.getByRole("button", { name: "通用" })).toBeInTheDocument();
  });
  expect(onEntryHandled).toHaveBeenCalledTimes(1);
  expect(onEntryHandled).toHaveBeenCalledWith(42);
  expect(rootWhenHandled).toEqual([true]);
});

test("renders health pushed by the live service-health observer", async () => {
  const user = userEvent.setup();
  let onSnapshot: ((snapshot: unknown[]) => void) | undefined;
  subscribeServiceHealthMock.mockImplementation(async (_failure: unknown, observer?: (snapshot: unknown[]) => void) => {
    onSnapshot = observer;
    return {
      initial: [{ serviceId: "initial-service", state: "healthy", messageKey: "services.healthy", parameters: { serviceId: "initial-service" }, checkedAt: 1 }],
      dispose: vi.fn(),
    };
  });
  getDiagnosticsMock.mockResolvedValue([]);
  renderSettings();

  await user.click(screen.getByRole("button", { name: "诊断" }));
  await screen.findByText("initial-service 运行正常");
  expect(onSnapshot).toEqual(expect.any(Function));

  act(() => onSnapshot?.([{ serviceId: "event-service", state: "degraded", messageKey: "services.degraded", parameters: { serviceId: "event-service", reasonCode: "slow" }, checkedAt: 2 }]));
  expect(await screen.findByText("event-service 受限：slow")).toBeInTheDocument();
});

test("StrictMode disposes a pending diagnostics bootstrap before the next live session can load", async () => {
  const user = userEvent.setup();
  const staleReady = deferred<{ initial: unknown[]; dispose(): void }>();
  const activeReady = deferred<{ initial: unknown[]; dispose(): void }>();
  const staleDispose = vi.fn();
  const activeDispose = vi.fn();
  beginServiceHealthSubscriptionMock
    .mockReturnValueOnce({ dispose: staleDispose, ready: staleReady.promise })
    .mockReturnValueOnce({ dispose: activeDispose, ready: activeReady.promise });
  subscribeServiceHealthMock
    .mockReturnValueOnce(staleReady.promise)
    .mockReturnValueOnce(activeReady.promise);
  getDiagnosticsMock.mockResolvedValue([]);
  render(
    <StrictMode>
      <I18nProvider>
        <SettingsHarness />
      </I18nProvider>
    </StrictMode>,
  );

  await user.click(screen.getByRole("button", { name: "诊断" }));
  await waitFor(() => expect(beginServiceHealthSubscriptionMock).toHaveBeenCalledTimes(1));
  await user.keyboard("{Escape}");
  await user.click(screen.getByRole("button", { name: "诊断" }));
  await waitFor(() => expect(beginServiceHealthSubscriptionMock).toHaveBeenCalledTimes(2));
  expect(staleDispose).toHaveBeenCalledTimes(1);

  await act(async () => {
    staleReady.resolve({ initial: [], dispose: staleDispose });
    await staleReady.promise;
  });
  expect(getDiagnosticsMock).not.toHaveBeenCalled();
  activeReady.resolve({ initial: [], dispose: activeDispose });
  await activeReady.promise;
});

test("ignores a second Retry click while its replacement diagnostics bootstrap is pending", async () => {
  const user = userEvent.setup();
  const initialReady = deferred<{ initial: unknown[]; dispose(): void }>();
  const retryReady = deferred<{ initial: unknown[]; dispose(): void }>();
  beginServiceHealthSubscriptionMock
    .mockReturnValueOnce({
      dispose: vi.fn(),
      ready: initialReady.promise,
    })
    .mockReturnValueOnce({ dispose: vi.fn(), ready: retryReady.promise });
  subscribeServiceHealthMock.mockRejectedValueOnce({
    code: "storageUnavailable",
    messageKey: "errors.storageUnavailable",
    details: {},
    retryable: true,
  });
  getDiagnosticsMock.mockResolvedValue([]);
  renderSettings();

  await user.click(screen.getByRole("button", { name: "诊断" }));
  initialReady.reject({
    code: "storageUnavailable",
    messageKey: "errors.storageUnavailable",
    details: {},
    retryable: true,
  });
  await initialReady.promise.catch(() => undefined);
  await screen.findByRole("alert");
  const retry = screen.getByRole("button", { name: "重试" });
  await user.click(retry);
  expect(retry).toBeDisabled();
  await user.click(retry);
  expect(beginServiceHealthSubscriptionMock).toHaveBeenCalledTimes(2);

  retryReady.resolve({ initial: [], dispose: vi.fn() });
  await retryReady.promise;
});

test("restarts listener-first diagnostics bootstrap on Retry and keeps receiving observer snapshots", async () => {
  const user = userEvent.setup();
  let retryObserver: ((snapshot: unknown[]) => void) | undefined;
  const initialFailure = Promise.reject({
    code: "storageUnavailable",
    messageKey: "errors.storageUnavailable",
    details: { reasonCode: "initialLoad" },
    retryable: true,
  });
  void initialFailure.catch(() => undefined);
  beginServiceHealthSubscriptionMock.mockReturnValueOnce({
    dispose: vi.fn(),
    ready: initialFailure,
  });
  beginServiceHealthSubscriptionMock.mockImplementationOnce((_failure: unknown, observer?: (snapshot: unknown[]) => void) => {
    retryObserver = observer;
    return {
      dispose: vi.fn(),
      ready: Promise.resolve({
        initial: [{ serviceId: "recovered-service", state: "healthy", messageKey: "services.healthy", parameters: { serviceId: "recovered-service" }, checkedAt: 2 }],
        dispose: vi.fn(),
      }),
    };
  });
  listServiceHealthMock.mockResolvedValue([]);
  getDiagnosticsMock.mockResolvedValue([]);
  renderSettings();

  await user.click(screen.getByRole("button", { name: "诊断" }));
  await screen.findByRole("alert");
  expect(screen.getByRole("alert")).toHaveTextContent("本地存储不可用");
  await user.click(screen.getByRole("button", { name: "重试" }));

  await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
  expect(beginServiceHealthSubscriptionMock).toHaveBeenCalledTimes(2);
  expect(retryObserver).toEqual(expect.any(Function));
  expect(screen.getByRole("heading", { name: "诊断" })).toBeInTheDocument();
  expect(screen.getByText("recovered-service 运行正常")).toBeInTheDocument();

  act(() => retryObserver?.([{ serviceId: "post-retry-service", state: "degraded", messageKey: "services.degraded", parameters: { serviceId: "post-retry-service", reasonCode: "poll" }, checkedAt: 3 }]));
  expect(await screen.findByText("post-retry-service 受限：poll")).toBeInTheDocument();
});

test("clears an old bootstrap error after Escape and a successful new diagnostics session", async () => {
  const user = userEvent.setup();
  subscribeServiceHealthMock
    .mockRejectedValueOnce({
      code: "storageUnavailable",
      messageKey: "errors.storageUnavailable",
      details: { reasonCode: "initialLoad" },
      retryable: true,
    })
    .mockResolvedValueOnce({
      initial: [{ serviceId: "fresh-service", state: "healthy", messageKey: "services.healthy", parameters: { serviceId: "fresh-service" }, checkedAt: 2 }],
      dispose: vi.fn(),
    });
  getDiagnosticsMock.mockResolvedValue([]);
  renderSettings();

  await user.click(screen.getByRole("button", { name: "诊断" }));
  await screen.findByRole("alert");
  await user.keyboard("{Escape}");
  await user.click(screen.getByRole("button", { name: "诊断" }));

  expect(await screen.findByText("fresh-service 运行正常")).toBeInTheDocument();
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

test("does not let an old diagnostics session overwrite a newer route session", async () => {
  const user = userEvent.setup();
  const oldEvents = deferred<unknown[]>();
  subscribeServiceHealthMock
    .mockResolvedValueOnce({ initial: [], dispose: vi.fn() })
    .mockResolvedValueOnce({ initial: [], dispose: vi.fn() });
  getDiagnosticsMock
    .mockImplementationOnce(() => oldEvents.promise)
    .mockResolvedValueOnce([{ id: "fresh", serviceId: "fresh-service", level: "info", code: "fresh", parameters: {}, createdAt: 2 }]);
  renderSettings();

  await user.click(screen.getByRole("button", { name: "诊断" }));
  await waitFor(() => expect(getDiagnosticsMock).toHaveBeenCalledTimes(1));
  await user.keyboard("{Escape}");
  await user.click(screen.getByRole("button", { name: "诊断" }));
  await screen.findByText("fresh-service: fresh");

  await act(async () => {
    oldEvents.resolve([{ id: "stale", serviceId: "stale-service", level: "info", code: "stale", parameters: {}, createdAt: 1 }]);
    await oldEvents.promise;
  });
  expect(screen.getByText("fresh-service: fresh")).toBeInTheDocument();
  expect(screen.queryByText("stale-service: stale")).not.toBeInTheDocument();
});

test("refreshes service health and runtime records after an integrity check without a repair action", async () => {
  const user = userEvent.setup();
  subscribeServiceHealthMock.mockResolvedValue({
    initial: [{
      serviceId: "initial-service",
      state: "healthy",
      messageKey: "services.healthy",
      parameters: { serviceId: "initial-service" },
      checkedAt: 1,
    }],
    dispose: vi.fn(),
  });
  getDiagnosticsMock
    .mockResolvedValueOnce([{ id: "initial", serviceId: "initial-service", level: "info", code: "initial", parameters: {}, createdAt: 1 }])
    .mockResolvedValueOnce([{ id: "refreshed", serviceId: "refreshed-service", level: "info", code: "refreshed", parameters: {}, createdAt: 2 }]);
  checkStorageIntegrityMock.mockResolvedValue({ integrity: "ok", schemaVersion: 1, checkedAt: 2 });
  listServiceHealthMock.mockResolvedValue([{
    serviceId: "refreshed-service",
    state: "healthy",
    messageKey: "services.healthy",
    parameters: { serviceId: "refreshed-service" },
    checkedAt: 2,
  }]);
  renderSettings();

  await user.click(screen.getByRole("button", { name: "诊断" }));
  await screen.findByText("initial-service: initial");
  await user.click(screen.getByRole("button", { name: "检查完整性" }));

  expect(await screen.findByText("refreshed-service 运行正常")).toBeInTheDocument();
  expect(screen.getByText("refreshed-service: refreshed")).toBeInTheDocument();
});

test("renders the confirmed 0-100 scale slider and previews the mapped percentage", async () => {
  const user = userEvent.setup();
  renderSettings();

  await user.click(screen.getByRole("button", { name: "显示与外观" }));
  const slider = screen.getByRole("slider", { name: "窗口缩放" });
  expect(slider).toHaveAttribute("min", "0");
  expect(slider).toHaveAttribute("max", "100");
  expect(slider).toHaveValue("50");
  expect(screen.getByText("100%", { selector: "output" })).toBeInTheDocument();

  fireEvent.change(slider, { target: { value: "75" } });
  expect(slider).toHaveValue("75");
  expect(slider).toHaveAttribute("aria-valuetext", "160%");
  expect(screen.getByText("160%", { selector: "output" })).toBeInTheDocument();
});

test("opens the AIsland Agent settings surface with the new preset and Custom Hook choices", async () => {
  const user = userEvent.setup();
  renderSettings();

  await user.click(screen.getByRole("button", { name: /Agent/ }));
  expect(await screen.findByText("Kimi Code")).toBeInTheDocument();
  expect(screen.getByText("TRAE")).toBeInTheDocument();
  expect(screen.getByText("QoderWork")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: /Custom Hook/ })).toBeInTheDocument();
});

test("renders external Agent values unchanged in both interface languages", async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValue(undefined);
  render(
    <I18nProvider>
      <ExternalValueFixture />
    </I18nProvider>,
  );

  expect(screen.getByText("WorkBuddy-Pro")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "English" }));
  await waitFor(() => {
    expect(screen.getByText("Agents & Integrations")).toBeInTheDocument();
  });
  expect(screen.getByText("WorkBuddy-Pro")).toBeInTheDocument();
});
