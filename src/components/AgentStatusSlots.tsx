import { Bot, Code2, Feather, Orbit, Sparkles, type LucideIcon } from "lucide-react";
import { useMemo } from "react";
import claudeDesktopIcon from "../assets/agents/claude.png";
import codexDesktopIcon from "../assets/agents/codex.png";
import hermesDesktopIcon from "../assets/agents/hermes.png";
import kimiDesktopIcon from "../assets/agents/kimi.png";
import traeDesktopIcon from "../assets/agents/trae.png";
import workbuddyDesktopIcon from "../assets/agents/workbuddy.png";
import { useI18n } from "../i18n/I18nProvider";
import type { AgentId, AgentProfilePresetId, AgentProfileStatusSummary, AgentStatus, AgentSummary } from "../api/contracts";
import StatusDot from "./StatusDot";
import { AGENT_STATUS_COLOR, isAgentAttentionStatus } from "./agentStatusPresentation";

export interface AgentStatusSlotsProps {
  agents: AgentSummary[];
  compactWidth?: number;
  onOpenAgent(agentId: AgentId): void;
  profileSummaries?: AgentProfileStatusSummary[];
  onOpenProfile?(profileId: string): void;
}

const STATUS_RANK: Record<AgentStatus, number> = {
  failed: 0,
  timeout: 0,
  waiting: 1,
  running: 2,
  completed: 3,
  idle: 4,
  offline: 5,
};

const AGENT_RANK: Record<AgentId, number> = { codex: 0, hermes: 1, workbuddy: 2, claude: 3 };

type AgentLogoAsset = {
  fallback: LucideIcon;
  desktopIcon?: string;
};

const AGENT_LOGOS: Record<AgentId, AgentLogoAsset> = {
  codex: { fallback: Orbit, desktopIcon: codexDesktopIcon },
  hermes: { fallback: Feather, desktopIcon: hermesDesktopIcon },
  workbuddy: { fallback: Bot, desktopIcon: workbuddyDesktopIcon },
  claude: { fallback: Sparkles, desktopIcon: claudeDesktopIcon },
};

const PROFILE_LOGOS: Record<AgentProfilePresetId | "custom", AgentLogoAsset> = {
  kimi: { fallback: Sparkles, desktopIcon: kimiDesktopIcon },
  trae: { fallback: Orbit, desktopIcon: traeDesktopIcon },
  qoderwork: { fallback: Code2 },
  cursor: { fallback: Code2 },
  custom: { fallback: Bot },
};

type LegacyStatusSlot = {
  kind: "legacy";
  agent: AgentSummary;
  status: AgentStatus;
  occurredAt: number;
};

type ProfileStatusSlot = {
  kind: "profile";
  profile: AgentProfileStatusSummary;
  status: AgentStatus;
  occurredAt: number;
  logo: AgentProfilePresetId | "custom";
};

type StatusSlot = LegacyStatusSlot | ProfileStatusSlot;

export function sortAgentsByPriority(agents: AgentSummary[]): AgentSummary[] {
  return [...agents].sort((left, right) => {
    const status = STATUS_RANK[left.aggregateStatus] - STATUS_RANK[right.aggregateStatus];
    if (status !== 0) return status;
    const newestLeft = Math.max(...left.environments.map((observation) => observation.occurredAt), Number.NEGATIVE_INFINITY);
    const newestRight = Math.max(...right.environments.map((observation) => observation.occurredAt), Number.NEGATIVE_INFINITY);
    if (newestLeft !== newestRight) return newestRight - newestLeft;
    return AGENT_RANK[left.agentId] - AGENT_RANK[right.agentId];
  });
}

export function visibleAgentSummaries(agents: AgentSummary[]): AgentSummary[] {
  return agents.filter((agent) => agent.environments.some((observation) => observation.status !== "offline"));
}

export function visibleProfileStatusSlots(profiles: AgentProfileStatusSummary[]): ProfileStatusSlot[] {
  return profiles
    .filter(({ aggregateStatus }) => aggregateStatus !== "offline")
    .map((profile) => ({
      kind: "profile" as const,
      status: profile.aggregateStatus,
      occurredAt: Math.max(...profile.observations.map((observation) => observation.occurredAt), Number.NEGATIVE_INFINITY),
      logo: profile.profile.configTarget.kind === "preset" ? profile.profile.configTarget.adapterId : "custom",
      profile,
    }));
}

function sortStatusSlots(slots: StatusSlot[]) {
  return [...slots].sort((left, right) => {
    const status = STATUS_RANK[left.status] - STATUS_RANK[right.status];
    if (status !== 0) return status;
    if (left.occurredAt !== right.occurredAt) return right.occurredAt - left.occurredAt;
    if (left.kind === "legacy" && right.kind === "legacy") return AGENT_RANK[left.agent.agentId] - AGENT_RANK[right.agent.agentId];
    if (left.kind === "legacy") return -1;
    if (right.kind === "legacy") return 1;
    return left.profile.profile.displayName.localeCompare(right.profile.profile.displayName);
  });
}

function collectStatusSlots(agents: AgentSummary[], profileSummaries: AgentProfileStatusSummary[]) {
  return sortStatusSlots([
    ...visibleAgentSummaries(agents).map((agent) => ({
      kind: "legacy" as const,
      agent,
      status: agent.aggregateStatus,
      occurredAt: Math.max(...agent.environments.map((observation) => observation.occurredAt), Number.NEGATIVE_INFINITY),
    })),
    ...visibleProfileStatusSlots(profileSummaries),
  ]);
}

export function prioritizedAgentStatuses(agents: AgentSummary[], profileSummaries: AgentProfileStatusSummary[]) {
  return collectStatusSlots(agents, profileSummaries).map((slot) => slot.status);
}

function statusKey(status: AgentStatus) {
  return `agents.status.${status}` as const;
}

function stopDrag(event: React.PointerEvent<HTMLElement>) {
  event.stopPropagation();
}

function AgentLogo({ asset }: { asset: AgentLogoAsset }) {
  if (asset.desktopIcon) {
    return <img className="agent-gui-logo__image" src={asset.desktopIcon} alt="" draggable={false} />;
  }
  const Fallback = asset.fallback;
  return <Fallback size={15} strokeWidth={1.7} />;
}

export function visibleAgentCapacityForWidth(width: number) {
  if (!Number.isFinite(width)) return 4;
  return Math.max(4, Math.floor((width - 124) / 31));
}

export default function AgentStatusSlots({ agents, compactWidth = 720, profileSummaries = [], onOpenAgent, onOpenProfile }: AgentStatusSlotsProps) {
  const { t } = useI18n();
  const sorted = useMemo<StatusSlot[]>(() => collectStatusSlots(agents, profileSummaries), [agents, profileSummaries]);
  const visibleCapacity = visibleAgentCapacityForWidth(compactWidth);
  const hiddenCount = Math.max(0, sorted.length - visibleCapacity);
  const running = sorted.some((slot) => slot.status === "running");
  const attention = sorted.some((slot) => isAgentAttentionStatus(slot.status));
  const completed = sorted.some((slot) => slot.status === "completed");
  const overallSignal: AgentStatus = attention
    ? "waiting"
    : running
      ? "running"
      : completed
        ? "completed"
        : sorted.length > 0
          ? "idle"
          : "offline";
  const overallStatus = overallSignal === "running" || attention
    ? t("agents.compact.working")
    : overallSignal === "idle"
      ? t("agents.compact.idle")
      : t(statusKey(overallSignal));

  return (
    <div className="agent-status-slots" aria-label={t("aria.agentStatus")}>
      <div className="agent-logo-strip">
        {sorted.slice(0, visibleCapacity).map((slot) => {
          if (slot.kind === "profile") {
            const { profile } = slot.profile;
            const logo = PROFILE_LOGOS[slot.logo];
            const status = t(statusKey(slot.status));
            const environment = t(`agents.environments.${profile.environment}`);
            return (
              <button
                key={profile.id}
                className="agent-logo-button"
                data-profile-id={profile.id}
                aria-label={`${profile.displayName} · ${environment} · ${status}`}
                title={`${profile.displayName} · ${environment} · ${status}`}
                onPointerDown={stopDrag}
                onClick={() => onOpenProfile?.(profile.id)}
              >
                <span className={`agent-gui-logo agent-gui-logo--profile agent-gui-logo--profile-${slot.logo}`} data-testid="agent-gui-logo" aria-hidden="true">
                  <AgentLogo asset={logo} />
                </span>
                <StatusDot color={AGENT_STATUS_COLOR[slot.status]} pulse={slot.status === "running"} />
              </button>
            );
          }
          const { agent } = slot;
          const logo = AGENT_LOGOS[agent.agentId];
          const status = t(statusKey(slot.status));
          return (
            <button
              key={agent.agentId}
              className="agent-logo-button"
              data-agent-id={agent.agentId}
              aria-label={`${agent.displayName} · ${status}`}
              title={`${agent.displayName} · ${status}`}
              onPointerDown={stopDrag}
              onClick={() => onOpenAgent(agent.agentId)}
            >
              <span className={`agent-gui-logo agent-gui-logo--${agent.agentId}`} data-testid="agent-gui-logo" aria-hidden="true">
                <AgentLogo asset={logo} />
              </span>
              <StatusDot color={AGENT_STATUS_COLOR[slot.status]} pulse={slot.status === "running"} />
            </button>
          );
        })}
        {hiddenCount > 0 && <span className="agent-overflow-count">+{hiddenCount}</span>}
      </div>
      <div className="agent-compact-state" aria-label={overallStatus}>
        <StatusDot
          color={AGENT_STATUS_COLOR[overallSignal]}
          pulse={overallSignal === "running"}
        />
        <span>{overallStatus}</span>
      </div>
    </div>
  );
}
