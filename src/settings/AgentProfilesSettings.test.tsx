import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

const { getAgentsSnapshotMock, installIntegrationMock, repairIntegrationMock, listProfilesMock, discoverCandidatesMock, saveProfileMock, installProfileMock, repairProfileMock, uninstallProfileMock, deleteProfileMock } = vi.hoisted(() => ({
  getAgentsSnapshotMock: vi.fn(),
  installIntegrationMock: vi.fn(),
  repairIntegrationMock: vi.fn(),
  listProfilesMock: vi.fn(),
  discoverCandidatesMock: vi.fn(),
  saveProfileMock: vi.fn(),
  installProfileMock: vi.fn(),
  repairProfileMock: vi.fn(),
  uninstallProfileMock: vi.fn(),
  deleteProfileMock: vi.fn(),
}));

vi.mock("../api/commands", () => ({
  getAgentsSnapshot: getAgentsSnapshotMock,
  installAgentIntegration: installIntegrationMock,
  repairAgentIntegration: repairIntegrationMock,
  listAgentIntegrationProfiles: listProfilesMock,
  discoverAgentIntegrationCandidates: discoverCandidatesMock,
  saveAgentIntegrationProfile: saveProfileMock,
  installAgentIntegrationProfile: installProfileMock,
  repairAgentIntegrationProfile: repairProfileMock,
  uninstallAgentIntegrationProfile: uninstallProfileMock,
  deleteAgentIntegrationProfile: deleteProfileMock,
}));

import { I18nProvider } from "../i18n/I18nProvider";
import { zhCN } from "../i18n/catalog";
import AgentProfilesSettings from "./AgentProfilesSettings";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

const kimiDraft = {
  id: "kimi-windows",
  kind: "preset" as const,
  displayName: "Kimi Code",
  environment: "windows" as const,
  configTarget: { kind: "preset" as const, adapterId: "kimi" as const },
  eventMapping: [],
  enabled: true,
  installationState: "notInstalled" as const,
  reasonCode: null,
  revision: 1,
  updatedAt: 1,
};

const qoderDraft = {
  ...kimiDraft,
  id: "qoderwork-windows",
  displayName: "QoderWork",
  configTarget: { kind: "preset" as const, adapterId: "qoderwork" as const },
};

const cursorDraft = {
  ...kimiDraft,
  id: "cursor-windows",
  displayName: "Cursor",
  configTarget: { kind: "preset" as const, adapterId: "cursor" as const },
};

function renderProfiles(focusProfileId?: string | null) {
  return render(
    <I18nProvider>
      <AgentProfilesSettings focusProfileId={focusProfileId} />
    </I18nProvider>,
  );
}

beforeEach(() => {
  getAgentsSnapshotMock.mockResolvedValue({ agents: [], generatedAt: 1 });
  installIntegrationMock.mockResolvedValue({ state: "installed", changed: true });
  repairIntegrationMock.mockResolvedValue({ state: "installed", changed: true });
  listProfilesMock.mockResolvedValue([]);
  discoverCandidatesMock.mockResolvedValue({ candidates: [], scannedAt: 1 });
  saveProfileMock.mockResolvedValue(kimiDraft);
  installProfileMock.mockImplementation(({ id }: { id: string }) => Promise.resolve({
    ...(id === cursorDraft.id ? cursorDraft : kimiDraft),
    installationState: "installed",
    revision: 2,
  }));
});

afterEach(() => {
  cleanup();
  getAgentsSnapshotMock.mockReset();
  installIntegrationMock.mockReset();
  repairIntegrationMock.mockReset();
  listProfilesMock.mockReset();
  discoverCandidatesMock.mockReset();
  saveProfileMock.mockReset();
  installProfileMock.mockReset();
  repairProfileMock.mockReset();
  uninstallProfileMock.mockReset();
  deleteProfileMock.mockReset();
});

test("shows the new Agent choices within the existing settings visual surface", async () => {
  renderProfiles();

  expect(await screen.findByText("Kimi Code")).toBeInTheDocument();
  expect(screen.getByText("TRAE")).toBeInTheDocument();
  expect(screen.getByText("QoderWork")).toBeInTheDocument();
  expect(screen.getByText("Cursor")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "添加 Custom Hook" })).toBeInTheDocument();
});

test("one-click Hook setup connects running built-ins and verified presets including Cursor", async () => {
  const user = userEvent.setup();
  listProfilesMock.mockResolvedValue([kimiDraft, cursorDraft]);
  getAgentsSnapshotMock.mockResolvedValue({
    generatedAt: 1_233,
    agents: [{
      agentId: "codex",
      displayName: "Codex",
      aggregateStatus: "running",
      environments: [],
      integrations: [{ environment: "windows", supported: true, required: false, state: "notInstalled", reasonCode: null }],
    }],
  });
  discoverCandidatesMock.mockResolvedValue({
    scannedAt: 1_234,
    candidates: [
      { id: "codex", displayName: "Codex", environment: "windows", integrationKind: "builtIn", state: "automatic", presetId: null, evidence: ["runningProcess"], reasonCode: null },
      { id: "kimi", displayName: "Kimi Code", environment: "windows", integrationKind: "preset", state: "readyToInstall", presetId: "kimi", evidence: ["runningProcess", "configuration"], reasonCode: null },
      { id: "cursor", displayName: "Cursor", environment: "windows", integrationKind: "preset", state: "readyToInstall", presetId: "cursor", evidence: ["runningProcess", "installedApplication"], reasonCode: null },
    ],
  });
  renderProfiles();

  await user.click(await screen.findByRole("button", { name: "一键检测并配置 Hook" }));

  expect(discoverCandidatesMock).toHaveBeenCalledTimes(1);
  expect(installIntegrationMock).toHaveBeenCalledWith({ agentId: "codex", environment: "windows" });
  expect(installProfileMock).toHaveBeenCalledWith({ id: "kimi-windows", expectedRevision: 1, confirmInstallation: true });
  expect(installProfileMock).toHaveBeenCalledWith({ id: "cursor-windows", expectedRevision: 1, confirmInstallation: true });
  expect((await screen.findAllByText("Hook 已自动配置")).length).toBeGreaterThanOrEqual(2);
  await user.click(screen.getByRole("button", { name: "前往配置 Kimi Code" }));
  await waitFor(() => expect(document.querySelector('article[data-profile-id="kimi-windows"]')).toHaveFocus());
  expect(saveProfileMock).not.toHaveBeenCalled();
  expect(installProfileMock).toHaveBeenCalledTimes(2);
});

test("one-click Hook setup repairs running Agents whose cached installation is stale", async () => {
  const user = userEvent.setup();
  const installedCursor = {
    ...cursorDraft,
    installationState: "installed" as const,
    revision: 7,
  };
  listProfilesMock.mockResolvedValue([installedCursor]);
  getAgentsSnapshotMock.mockResolvedValue({
    generatedAt: 1_240,
    agents: [{
      agentId: "hermes",
      displayName: "Hermes",
      aggregateStatus: "running",
      environments: [],
      integrations: [{ environment: "windows", supported: true, required: false, state: "installed", reasonCode: null }],
    }],
  });
  discoverCandidatesMock.mockResolvedValue({
    scannedAt: 1_241,
    candidates: [
      { id: "hermes", displayName: "Hermes", environment: "windows", integrationKind: "builtIn", state: "automatic", presetId: null, evidence: ["runningProcess"], reasonCode: null },
      { id: "cursor", displayName: "Cursor", environment: "windows", integrationKind: "preset", state: "readyToInstall", presetId: "cursor", evidence: ["runningProcess", "installedApplication"], reasonCode: null },
    ],
  });
  repairProfileMock.mockResolvedValue({
    ...installedCursor,
    installationState: "installed",
    revision: 8,
  });
  renderProfiles();

  await user.click(await screen.findByRole("button", { name: "一键检测并配置 Hook" }));

  await waitFor(() => expect(repairIntegrationMock).toHaveBeenCalledWith({ agentId: "hermes", environment: "windows" }));
  expect(repairProfileMock).toHaveBeenCalledWith({
    id: "cursor-windows",
    expectedRevision: 7,
    confirmRepair: true,
  });
  expect(installIntegrationMock).not.toHaveBeenCalled();
  expect(installProfileMock).not.toHaveBeenCalled();
});

test("one-click discovery continues connecting later running Agents when an earlier preset fails", async () => {
  const user = userEvent.setup();
  listProfilesMock.mockResolvedValue([kimiDraft, qoderDraft]);
  discoverCandidatesMock.mockResolvedValue({
    scannedAt: 1_235,
    candidates: [
      { id: "kimi", displayName: "Kimi Code", environment: "windows", integrationKind: "preset", state: "readyToInstall", presetId: "kimi", evidence: ["runningProcess"], reasonCode: null },
      { id: "qoderwork", displayName: "QoderWork", environment: "windows", integrationKind: "preset", state: "readyToInstall", presetId: "qoderwork", evidence: ["runningProcess"], reasonCode: null },
    ],
  });
  installProfileMock
    .mockRejectedValueOnce({ code: "ioFailure", messageKey: "errors.ioFailure", parameters: { reasonCode: "fixture" } })
    .mockResolvedValueOnce({ ...qoderDraft, installationState: "installed", revision: 2 });
  renderProfiles();

  await user.click(await screen.findByRole("button", { name: "一键检测并配置 Hook" }));

  await waitFor(() => expect(installProfileMock).toHaveBeenCalledTimes(2));
  expect(installProfileMock).toHaveBeenNthCalledWith(2, {
    id: "qoderwork-windows",
    expectedRevision: 1,
    confirmInstallation: true,
  });
  expect(await screen.findByRole("alert")).toHaveTextContent("无法完成此 Agent Profile 操作。");
});

test("locks manual Profile mutations while one-click discovery is still in flight", async () => {
  const user = userEvent.setup();
  const request = deferred<{ candidates: []; scannedAt: number }>();
  listProfilesMock.mockResolvedValue([kimiDraft]);
  discoverCandidatesMock.mockReturnValue(request.promise);
  renderProfiles();
  const scan = await screen.findByRole("button", { name: "一键检测并配置 Hook" });
  const install = screen.getByRole("button", { name: "安装集成 Kimi Code" });

  await user.click(scan);

  expect(scan).toBeDisabled();
  expect(install).toBeDisabled();
  request.resolve({ candidates: [], scannedAt: 1_236 });
  await waitFor(() => expect(scan).toBeEnabled());
  expect(install).toBeEnabled();
});

test("installs an existing seeded preset through the revision-aware Profile bridge", async () => {
  const user = userEvent.setup();
  listProfilesMock.mockResolvedValue([kimiDraft]);
  renderProfiles();

  await user.click(await screen.findByRole("button", { name: "安装集成 Kimi Code" }));

  expect(installProfileMock).toHaveBeenCalledWith({ id: "kimi-windows", expectedRevision: 1, confirmInstallation: true });
  expect(saveProfileMock).not.toHaveBeenCalled();
});

test("does not invent or save a missing seeded preset profile, but refresh can restore its authoritative card", async () => {
  const user = userEvent.setup();
  listProfilesMock.mockResolvedValueOnce([]).mockResolvedValueOnce([kimiDraft]);
  renderProfiles();

  const installButton = await screen.findByRole("button", { name: "安装集成 Kimi Code" });
  expect(installButton).toBeDisabled();
  expect(saveProfileMock).not.toHaveBeenCalled();
  expect(installProfileMock).not.toHaveBeenCalled();

  const retry = screen.getAllByRole("button", { name: /Kimi Code/ }).find((button) => !button.hasAttribute("disabled"));
  await user.click(retry as HTMLButtonElement);
  await waitFor(() => expect(screen.getByRole("button", { name: "安装集成 Kimi Code" })).toBeEnabled());
  await user.click(screen.getByRole("button", { name: "安装集成 Kimi Code" }));
  expect(installProfileMock).toHaveBeenCalledWith({ id: "kimi-windows", expectedRevision: 1, confirmInstallation: true });
  expect(saveProfileMock).not.toHaveBeenCalled();
});

test("keeps preset install disabled when its authoritative list request fails", async () => {
  listProfilesMock.mockRejectedValueOnce({ code: "sourceUnavailable", messageKey: "errors.sourceUnavailable", details: {}, retryable: true });
  renderProfiles();

  expect(await screen.findByRole("button", { name: "安装集成 Kimi Code" })).toBeDisabled();
  expect(saveProfileMock).not.toHaveBeenCalled();
  expect(installProfileMock).not.toHaveBeenCalled();
});

test("keeps the selected preset environment independent when its other environment is installed", async () => {
  const user = userEvent.setup();
  listProfilesMock.mockResolvedValue([
    { ...kimiDraft, installationState: "installed", revision: 7 },
    { ...kimiDraft, id: "kimi-wsl", environment: "wsl", installationState: "unsupported", reasonCode: "profileWslNotSupported", revision: 3 },
  ]);
  renderProfiles();

  expect(await screen.findByText("已安装")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "WSL" }));

  expect(await screen.findByText("此环境不受支持")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "安装集成 Kimi Code" })).toBeDisabled();
});

test("keeps the frozen TRAE detection reason retryable without presenting it as permanently unsupported", async () => {
  const user = userEvent.setup();
  listProfilesMock.mockResolvedValue([
    { ...kimiDraft, id: "kimi-wsl", environment: "wsl", installationState: "unsupported", reasonCode: "profileWslNotSupported" },
    {
      ...kimiDraft,
      id: "trae-windows",
      displayName: "TRAE",
      configTarget: { kind: "preset", adapterId: "trae" },
      installationState: "unsupported",
      reasonCode: "traeHooksVersionOrConfigUnavailable",
    },
  ]);
  renderProfiles();

  expect(await screen.findByText(zhCN["agentProfiles.reason.traeTargetMissing"])).toBeInTheDocument();
  expect(screen.getByText("等待检测")).toBeInTheDocument();
  expect(screen.queryByText(zhCN["agents.integration.unsupported"])).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: `重试 TRAE` }));
  await waitFor(() => expect(listProfilesMock).toHaveBeenCalledTimes(2));
  expect(saveProfileMock).not.toHaveBeenCalled();
  expect(installProfileMock).not.toHaveBeenCalled();

  await user.click(screen.getByRole("button", { name: "WSL" }));
  expect(await screen.findByText(zhCN["agentProfiles.reason.wslUnsupported"])).toBeInTheDocument();
});

test("switches to and focuses the selected WSL Profile before presenting its card", async () => {
  listProfilesMock.mockResolvedValue([
    { ...kimiDraft, id: "kimi-wsl", environment: "wsl", installationState: "unsupported", reasonCode: "profileWslNotSupported" },
  ]);
  renderProfiles("kimi-wsl");

  const wsl = await screen.findByRole("button", { name: "WSL" });
  expect(wsl).toHaveAttribute("aria-pressed", "true");
  await waitFor(() => expect(document.querySelector('article[data-profile-id="kimi-wsl"]')).toHaveFocus());
});

test("opens a Custom Hook editor with executable and argv kept separate", async () => {
  const user = userEvent.setup();
  renderProfiles();

  await user.click(await screen.findByRole("button", { name: "添加 Custom Hook" }));

  expect(screen.getByLabelText("可执行文件（Windows .exe）")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "添加参数" }));
  expect(screen.getByLabelText(/参数.*1/)).toBeInTheDocument();
  expect(screen.queryByLabelText("命令行")).not.toBeInTheDocument();

  await user.selectOptions(screen.getByRole("combobox", { name: zhCN["agentProfiles.field.environment"] }), "wsl");
  expect(screen.getByLabelText("可执行文件（WSL 暂不支持接入）")).toBeInTheDocument();
});

test("keeps WSL Custom Hooks read-only until the controlled adapter is available", async () => {
  const user = userEvent.setup();
  listProfilesMock.mockResolvedValue([{
    id: "custom-wsl", kind: "custom", displayName: "WSL hook", environment: "wsl",
    configTarget: { kind: "customHook", executable: "/opt/aiceland/hook", argv: [], workingDirectory: null, timeoutSeconds: 30 },
    eventMapping: [{ nativeEvent: "completed", normalizedStatus: "completed" }], enabled: true,
    installationState: "unsupported", reasonCode: "profileWslNotSupported", revision: 1, updatedAt: 1,
  }]);
  renderProfiles();

  await screen.findByText("WSL hook");
  await user.click(screen.getByRole("button", { name: "WSL" }));
  expect(screen.getByRole("button", { name: zhCN["agentProfiles.addCustom"] })).toBeDisabled();
  expect(screen.getByRole("button", { name: `${zhCN["agentProfiles.action.edit"]} WSL hook` })).toBeDisabled();

  await user.click(screen.getByRole("button", { name: "Windows" }));
  await user.click(screen.getByRole("button", { name: zhCN["agentProfiles.addCustom"] }));
  await user.selectOptions(screen.getByRole("combobox", { name: zhCN["agentProfiles.field.environment"] }), "wsl");
  const save = screen.getByRole("button", { name: zhCN["action.save"] });
  expect(save).toBeDisabled();
  await user.click(save);
  expect(saveProfileMock).not.toHaveBeenCalled();
});

test("reloads the authoritative NeedsRepair revision after a Custom install side effect rejects", async () => {
  const user = userEvent.setup();
  const mutation = deferred<typeof kimiDraft>();
  const notInstalled = {
    id: "custom-1", kind: "custom" as const, displayName: "Build hook", environment: "windows" as const,
    configTarget: { kind: "customHook" as const, executable: "C:\\Tools\\hook.exe", argv: [], workingDirectory: null, timeoutSeconds: 30 },
    eventMapping: [{ nativeEvent: "completed", normalizedStatus: "completed" as const }], enabled: false,
    installationState: "notInstalled" as const, reasonCode: null, revision: 1, updatedAt: 1,
  };
  const needsRepair = {
    ...notInstalled,
    enabled: true,
    installationState: "needsRepair" as const,
    reasonCode: "hookExitedBeforeActivation",
    revision: 2,
    updatedAt: 2,
  };
  listProfilesMock.mockResolvedValueOnce([notInstalled]).mockResolvedValue([needsRepair]);
  installProfileMock.mockReturnValue(mutation.promise);
  repairProfileMock.mockResolvedValue({ ...needsRepair, installationState: "installed", reasonCode: null, revision: 3, updatedAt: 3 });
  renderProfiles();

  await user.click(await screen.findByRole("button", { name: `${zhCN["agents.integration.install"]} Build hook` }));
  expect(installProfileMock).toHaveBeenCalledWith({ id: "custom-1", expectedRevision: 1, confirmInstallation: true });

  mutation.reject({ code: "ioFailure", messageKey: "errors.ioFailure", details: { reasonCode: "hookExitedBeforeActivation" }, retryable: true });

  expect(await screen.findByText(zhCN["agents.integration.needsRepair"])).toBeInTheDocument();
  expect(screen.getByRole("alert")).toHaveTextContent("ioFailure");
  await user.click(screen.getByRole("button", { name: `${zhCN["agents.integration.repair"]} Build hook` }));
  expect(repairProfileMock).toHaveBeenCalledWith({ id: "custom-1", expectedRevision: 2, confirmRepair: true });
});

test.each(["installed", "needsRepair"] as const)("locks Custom Hook editing and deletion while a %s profile is supervised", async (installationState) => {
  const user = userEvent.setup();
  listProfilesMock.mockResolvedValue([{
    id: "custom-1", kind: "custom", displayName: "Build hook", environment: "windows",
    configTarget: { kind: "customHook", executable: "C:\\Tools\\hook.exe", argv: [], workingDirectory: null, timeoutSeconds: 30 },
    eventMapping: [{ nativeEvent: "completed", normalizedStatus: "completed" }], enabled: true,
    installationState, reasonCode: null, revision: 1, updatedAt: 1,
  }]);
  renderProfiles();

  await screen.findByText("Build hook");
  const actions = screen.getAllByRole("button").filter((button) => button.getAttribute("aria-label")?.endsWith("Build hook"));
  expect(actions).toHaveLength(3);
  expect(actions.filter((button) => button.hasAttribute("disabled"))).toHaveLength(2);
  const deleteButton = screen.getByRole("button", { name: `${zhCN["agentProfiles.action.delete"]} Build hook` });
  expect(deleteButton).toBeDisabled();
  await user.click(deleteButton);
  expect(deleteProfileMock).not.toHaveBeenCalled();
});

test("rejects an invalid Custom Hook locally and shows its safe field boundary", async () => {
  const user = userEvent.setup();
  renderProfiles();

  await user.click(await screen.findByRole("button", { name: "添加 Custom Hook" }));
  await user.click(screen.getByRole("button", { name: "保存" }));

  expect(saveProfileMock).not.toHaveBeenCalled();
  expect(await screen.findByText("可执行文件和工作目录必须使用绝对路径。")).toBeInTheDocument();
});

test("does not save a Custom Hook with empty or duplicate event mappings", async () => {
  const user = userEvent.setup();
  renderProfiles();

  await user.click(await screen.findByRole("button", { name: "添加 Custom Hook" }));
  await user.type(screen.getByLabelText("可执行文件（Windows .exe）"), "C:\\Tools\\agent-hook.exe");
  await user.click(screen.getByRole("button", { name: "保存" }));
  expect(saveProfileMock).not.toHaveBeenCalled();
  expect(await screen.findByText("至少配置一个事件映射。")).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "添加映射" }));
  await user.type(screen.getByLabelText("原始事件"), "Completed");
  await user.click(screen.getByRole("button", { name: "添加映射" }));
  await user.type(screen.getAllByLabelText("原始事件")[1], "completed");
  await user.click(screen.getByRole("button", { name: "保存" }));

  expect(saveProfileMock).not.toHaveBeenCalled();
  expect(await screen.findByText("原始事件不能重复。")).toBeInTheDocument();
});
