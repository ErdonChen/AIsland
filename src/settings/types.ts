import type { TranslationKey } from "../i18n/catalog";
import type { AgentEnvironment, AgentId, EntityId } from "../api/contracts";

export type SettingsCategory =
  | "general"
  | "display"
  | "storage"
  | "agents"
  | "reminders"
  | "modules"
  | "diagnostics"
  | "about";

export type SettingsDetailId =
  | `agentIntegration:${AgentId}:${AgentEnvironment}`
  | `reminderRule:${"new" | EntityId}`
  | MonitorSettingsDetailId;

export type MonitorSettingsDetailId = `monitorThreshold:${"new" | EntityId}`;

export type SettingsRoute =
  | { level: "root" }
  | { level: "category"; category: SettingsCategory }
  | { level: "detail"; category: SettingsCategory; detail: SettingsDetailId };

export type ParsedSettingsDetailId =
  | { agentId: AgentId; environment: AgentEnvironment }
  | { reminderId: "new" | EntityId }
  | { thresholdId: "new" | EntityId };

const AGENT_IDS: readonly AgentId[] = ["codex", "hermes", "workbuddy", "claude"];
const AGENT_ENVIRONMENTS: readonly AgentEnvironment[] = ["windows", "wsl"];
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

export function parseSettingsDetailId(value: unknown): ParsedSettingsDetailId | null {
  if (typeof value !== "string") return null;
  const agentMatch = /^agentIntegration:([^:]+):([^:]+)$/.exec(value);
  if (agentMatch) {
    const agentId = agentMatch[1] as AgentId;
    const environment = agentMatch[2] as AgentEnvironment;
    if (
      AGENT_IDS.includes(agentId)
      && AGENT_ENVIRONMENTS.includes(environment)
      && !(agentId === "workbuddy" && environment === "wsl")
    ) {
      return { agentId, environment };
    }
    return null;
  }

  const reminderMatch = /^reminderRule:(.+)$/.exec(value);
  if (reminderMatch) {
    const reminderId = reminderMatch[1];
    return reminderId === "new" || UUID_PATTERN.test(reminderId) ? { reminderId } : null;
  }
  const thresholdMatch = /^monitorThreshold:(.+)$/.exec(value);
  if (!thresholdMatch) return null;
  const thresholdId = thresholdMatch[1];
  return thresholdId === "new" || UUID_PATTERN.test(thresholdId) ? { thresholdId } : null;
}

export type SettingsCategoryEntry = {
  id: SettingsCategory;
  labelKey: TranslationKey;
  summaryKey: TranslationKey;
  availability: "available" | "coming-soon";
};
