import type { AgentStatus } from "../api/contracts";

export const AGENT_STATUS_COLOR: Record<AgentStatus, string> = {
  running: "#EF9F27",
  completed: "#639922",
  failed: "#E24B4A",
  waiting: "#E24B4A",
  timeout: "#E24B4A",
  idle: "#72BCFF",
  offline: "#888780",
};

export function isAgentAttentionStatus(status: AgentStatus): boolean {
  return status === "waiting" || status === "failed" || status === "timeout";
}
