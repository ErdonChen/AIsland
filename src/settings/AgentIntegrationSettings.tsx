import { useState } from "react";
import {
  installAgentIntegration,
  repairAgentIntegration,
  uninstallAgentIntegration,
} from "../api/commands";
import { parseCommandError } from "../api/commandError";
import type { AgentEnvironment, AgentSummary, CommandError, IntegrationState } from "../api/contracts";
import { translateRegisteredMessage } from "../i18n/catalog";
import { useI18n } from "../i18n/I18nProvider";

type AgentIntegrationSettingsProps = {
  agent: AgentSummary;
  environment: AgentEnvironment;
  onRefresh(): Promise<void>;
};

function stateKey(state: IntegrationState) {
  switch (state) {
    case "notInstalled": return "agents.integration.notInstalled";
    case "installed": return "agents.integration.installed";
    case "needsRepair": return "agents.integration.needsRepair";
    case "unsupported": return "agents.integration.unsupported";
  }
}

export default function AgentIntegrationSettings({ agent, environment, onRefresh }: AgentIntegrationSettingsProps) {
  const { language, t } = useI18n();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<CommandError | null>(null);
  const integration = agent.integrations.find((candidate) => candidate.environment === environment);
  const state = integration?.state ?? "unsupported";

  const errorMessage = (() => {
    if (!error) return null;
    try {
      return translateRegisteredMessage(language, error.messageKey, error.details);
    } catch {
      return t("agents.integration.error");
    }
  })();

  const run = async () => {
    if (pending || state === "unsupported") return;
    if (state === "installed" && !window.confirm(t("agents.integration.confirmBody"))) return;
    setPending(true);
    setError(null);
    try {
      const input = { agentId: agent.agentId, environment };
      if (state === "notInstalled") await installAgentIntegration(input);
      if (state === "needsRepair") await repairAgentIntegration(input);
      if (state === "installed") await uninstallAgentIntegration({ ...input, confirmOwnedRemoval: true });
      await onRefresh();
    } catch (cause) {
      setError(parseCommandError(cause));
    } finally {
      setPending(false);
    }
  };

  const actionKey = state === "notInstalled"
    ? "agents.integration.install"
    : state === "needsRepair"
      ? "agents.integration.repair"
      : "agents.integration.uninstall";

  return (
    <div className="settings-control">
      <div className="settings-control__copy">
        <span>{t(stateKey(state))}</span>
        {integration?.reasonCode && <span>{integration.reasonCode}</span>}
      </div>
      <button
        className="settings-choice"
        type="button"
        disabled={pending || state === "unsupported"}
        onPointerDown={(event) => event.stopPropagation()}
        onClick={() => void run()}
      >
        {t(actionKey)}
      </button>
      {errorMessage && <p className="settings-error" role="alert">{errorMessage}</p>}
    </div>
  );
}
