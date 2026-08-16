import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ComponentType } from "react";
import { afterEach, expect, test, vi } from "vitest";

import type { DiagnosticEvent, ServiceHealthSnapshot } from "../api/contracts";
import { I18nProvider } from "../i18n/I18nProvider";

type DiagnosticsSettingsProps = {
  health: ServiceHealthSnapshot[];
  events: DiagnosticEvent[];
  storageIntegrity: "unknown" | "checking" | "ok" | "failed";
  onCheckStorageIntegrity(): Promise<void>;
  onRetry(): Promise<void>;
};

type DiagnosticsSettingsComponent = ComponentType<DiagnosticsSettingsProps>;

async function loadDiagnosticsSettings(): Promise<DiagnosticsSettingsComponent> {
  try {
    const module = await import(/* @vite-ignore */ "./Diagnostics" + "Settings") as { default?: DiagnosticsSettingsComponent };
    expect(module.default).toBeDefined();
    return module.default!;
  } catch (error) {
    expect(error).toBeUndefined();
    throw error;
  }
}

const health: ServiceHealthSnapshot[] = [
  {
    serviceId: "zeta-service",
    state: "degraded",
    messageKey: "services.degraded",
    parameters: { serviceId: "zeta-service", reasonCode: "slow" },
    checkedAt: 2,
  },
  {
    serviceId: "alpha-service",
    state: "healthy",
    messageKey: "services.healthy",
    parameters: { serviceId: "alpha-service" },
    checkedAt: 1,
  },
];

afterEach(() => {
  cleanup();
  localStorage.clear();
});

test("sorts service health by serviceId and gives each state an accessible bilingual label", async () => {
  const DiagnosticsSettings = await loadDiagnosticsSettings();

  render(
    <I18nProvider>
      <DiagnosticsSettings
        health={health}
        events={[]}
        storageIntegrity="ok"
        onCheckStorageIntegrity={vi.fn().mockResolvedValue(undefined)}
        onRetry={vi.fn().mockResolvedValue(undefined)}
      />
    </I18nProvider>,
  );

  expect(screen.getAllByRole("listitem").map((item) => item.textContent)).toEqual([
    expect.stringContaining("alpha-service"),
    expect.stringContaining("zeta-service"),
  ]);
  expect(screen.getByRole("status", { name: "alpha-service：正常" })).toBeInTheDocument();
  expect(screen.getByRole("status", { name: "zeta-service：受限" })).toBeInTheDocument();
  expect(screen.getByText("alpha-service 运行正常")).toBeInTheDocument();
  expect(screen.getByText("zeta-service 受限：slow")).toBeInTheDocument();
});

test("shows the empty runtime-record state and disables the integrity action while checking", async () => {
  const DiagnosticsSettings = await loadDiagnosticsSettings();

  render(
    <I18nProvider>
      <DiagnosticsSettings
        health={[]}
        events={[]}
        storageIntegrity="checking"
        onCheckStorageIntegrity={vi.fn().mockResolvedValue(undefined)}
        onRetry={vi.fn().mockResolvedValue(undefined)}
      />
    </I18nProvider>,
  );

  expect(screen.getByText("暂无运行记录")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "检查完整性" })).toBeDisabled();
});

test("renders a typed command error through its message key and retries only after request", async () => {
  const user = userEvent.setup();
  const DiagnosticsSettings = await loadDiagnosticsSettings();
  const onRetry = vi.fn().mockResolvedValue(undefined);

  render(
    <I18nProvider>
      <DiagnosticsSettings
        health={[]}
        events={[]}
        storageIntegrity="unknown"
        onCheckStorageIntegrity={vi.fn().mockRejectedValue({
          code: "storageUnavailable",
          messageKey: "errors.storageUnavailable",
          details: { reasonCode: "integrityFailed" },
          retryable: true,
        })}
        onRetry={onRetry}
      />
    </I18nProvider>,
  );

  await user.click(screen.getByRole("button", { name: "检查完整性" }));
  await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("本地存储不可用"));

  await user.click(screen.getByRole("button", { name: "重试" }));
  expect(onRetry).toHaveBeenCalledTimes(1);
});
