import type { AgentStatus } from "../api/contracts";
import { DEFAULT_STATUS_COLOR_PREFERENCES, resolveAgentStatusColors } from "../appearancePreferences";

export const AGENT_STATUS_COLOR: Record<AgentStatus, string> = resolveAgentStatusColors(DEFAULT_STATUS_COLOR_PREFERENCES);

export function isAgentAttentionStatus(status: AgentStatus): boolean {
  return status === "waiting" || status === "failed" || status === "timeout";
}
