import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";

const { installMock, repairMock, uninstallMock } = vi.hoisted(() => ({
  installMock: vi.fn(),
  repairMock: vi.fn(),
  uninstallMock: vi.fn(),
}));

vi.mock("../api/commands", () => ({
  installAgentIntegration: installMock,
  repairAgentIntegration: repairMock,
  uninstallAgentIntegration: uninstallMock,
}));

import type { AgentSummary } from "../api/contracts";
import { I18nProvider } from "../i18n/I18nProvider";

const codex: AgentSummary = {
  agentId: "codex",
  displayName: "Codex",
  aggregateStatus: "idle",
  environments: [],
  integrations: [
    { environment: "windows", supported: true, required: false, state: "notInstalled", reasonCode: null },
    { environment: "wsl", supported: true, required: false, state: "needsRepair", reasonCode: "fingerprintMismatch" },
  ],
};

async function loadAgentIntegrationSettings() {
  const componentPath = "./AgentIntegration" + (window.location.pathname ? "Settings" : "");
  const module = await import(/* @vite-ignore */ componentPath);
  return module.default;
}

afterEach(() => {
  cleanup();
  installMock.mockReset();
  repairMock.mockReset();
  uninstallMock.mockReset();
  vi.unstubAllGlobals();
});

test("parses only fixed Agent/environment details and new or UUID reminder details", async () => {
  const { parseSettingsDetailId } = await import("./types");

  expect(parseSettingsDetailId("agentIntegration:codex:windows")).toEqual({ agentId: "codex", environment: "windows" });
  expect(parseSettingsDetailId("agentIntegration:workbuddy:wsl")).toBeNull();
  expect(parseSettingsDetailId("agentIntegration:unknown:windows")).toBeNull();
  expect(parseSettingsDetailId("reminderRule:new")).toEqual({ reminderId: "new" });
  expect(parseSettingsDetailId("reminderRule:cc0e3e38-e3c1-4a3f-b69f-62e5d4c46a57")).toEqual({ reminderId: "cc0e3e38-e3c1-4a3f-b69f-62e5d4c46a57" });
  expect(parseSettingsDetailId("reminderRule:not-a-uuid")).toBeNull();
});

test("keeps each environment record independent and refreshes only after a successful action", async () => {
  const user = userEvent.setup();
  const AgentIntegrationSettings = await loadAgentIntegrationSettings();
  let resolveInstall!: () => void;
  installMock.mockReturnValue(new Promise<void>((resolve) => { resolveInstall = resolve; }));
  const onRefresh = vi.fn().mockResolvedValue(undefined);

  render(
    <I18nProvider>
      <AgentIntegrationSettings agent={codex} environment="windows" onRefresh={onRefresh} />
    </I18nProvider>,
  );

  expect(screen.getByText("未安装")).toBeInTheDocument();
  expect(screen.queryByText("需要修复")).not.toBeInTheDocument();
  const install = screen.getByRole("button", { name: "安装集成" });
  await user.click(install);
  expect(install).toBeDisabled();
  expect(onRefresh).not.toHaveBeenCalled();

  resolveInstall();
  await waitFor(() => expect(onRefresh).toHaveBeenCalledTimes(1));
  expect(installMock).toHaveBeenCalledWith({ agentId: "codex", environment: "windows" });
});

test("requires an explicit confirmation before passing literal true to uninstall", async () => {
  const user = userEvent.setup();
  const AgentIntegrationSettings = await loadAgentIntegrationSettings();
  const onRefresh = vi.fn().mockResolvedValue(undefined);
  uninstallMock.mockResolvedValue(undefined);
  vi.stubGlobal("confirm", vi.fn().mockReturnValue(false));

  render(
    <I18nProvider>
      <AgentIntegrationSettings
        agent={{ ...codex, integrations: [{ environment: "windows", supported: true, required: false, state: "installed", reasonCode: null }] }}
        environment="windows"
        onRefresh={onRefresh}
      />
    </I18nProvider>,
  );

  await user.click(screen.getByRole("button", { name: "卸载集成" }));
  expect(uninstallMock).not.toHaveBeenCalled();

  vi.stubGlobal("confirm", vi.fn().mockReturnValue(true));
  await user.click(screen.getByRole("button", { name: "卸载集成" }));
  await waitFor(() => expect(uninstallMock).toHaveBeenCalledWith({
    agentId: "codex",
    environment: "windows",
    confirmOwnedRemoval: true,
  }));
  expect(onRefresh).toHaveBeenCalledTimes(1);
});

test("renders a registered typed command failure without exposing raw command data", async () => {
  const user = userEvent.setup();
  const AgentIntegrationSettings = await loadAgentIntegrationSettings();
  installMock.mockRejectedValue({
    code: "integrationConfigInvalid",
    messageKey: "errors.integrationConfigInvalid",
    details: { agentName: "Codex", environment: "windows", reasonCode: "fingerprintMismatch" },
    retryable: true,
  });

  render(
    <I18nProvider>
      <AgentIntegrationSettings agent={codex} environment="windows" onRefresh={vi.fn()} />
    </I18nProvider>,
  );

  await user.click(screen.getByRole("button", { name: "安装集成" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("代理集成配置无效");
});
