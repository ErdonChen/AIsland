import { useEffect, useMemo } from "react";
import type { AgentEnvironment, AgentId, AgentProfileStatusSummary, AgentStatus, AgentSummary, AgentTriggerStatus } from "../api/contracts";
import { useI18n } from "../i18n/I18nProvider";
import StatusDot from "../components/StatusDot";
import { AGENT_STATUS_COLOR } from "../components/agentStatusPresentation";

export interface AgentsPageProps {
  agents: AgentSummary[];
  profileSummaries?: AgentProfileStatusSummary[];
  selectedAgentId?: AgentId | null;
  selectedContext?: { environment: AgentEnvironment; taskId: string; triggerStatus: AgentTriggerStatus } | null;
  selectedContextSequence?: number | null;
  onSelectedContextCommitted?: (context: CommittedAgentContext) => void;
}

type StatusCard =
  | { kind: "agent"; agent: AgentSummary; latestReceivedAt: number; order: number }
  | { kind: "profile"; summary: AgentProfileStatusSummary; latestReceivedAt: number; order: number };

export type CommittedAgentContext = {
  agentId: AgentId;
  environment: AgentEnvironment;
  taskId: string;
  triggerStatus: AgentTriggerStatus;
  sequence: number;
};

function statusKey(status: AgentStatus) {
  return `agents.status.${status}` as const;
}

export default function AgentsPage({ agents, profileSummaries = [], selectedAgentId = null, selectedContext = null, selectedContextSequence = null, onSelectedContextCommitted }: AgentsPageProps) {
  const { t } = useI18n();
  const selectedAgent = agents.find((agent) => agent.agentId === selectedAgentId) ?? null;
  const statusCards = useMemo<StatusCard[]>(() => [
    ...agents.map((agent, order) => ({
      kind: "agent" as const,
      agent,
      latestReceivedAt: Math.max(...agent.environments.map((observation) => observation.receivedAt), Number.NEGATIVE_INFINITY),
      order,
    })),
    ...profileSummaries
      .filter(({ aggregateStatus }) => aggregateStatus !== "offline")
      .map((summary, index) => ({
        kind: "profile" as const,
        summary,
        latestReceivedAt: Math.max(...summary.observations.map((observation) => observation.receivedAt), summary.profile.updatedAt),
        order: agents.length + index,
      })),
  ].sort((left, right) => right.latestReceivedAt - left.latestReceivedAt || left.order - right.order), [agents, profileSummaries]);
  const selectedObservation = selectedAgent?.environments.find((observation) => selectedContext !== null && observation.environment === selectedContext.environment && observation.taskId === selectedContext.taskId && observation.status === selectedContext.triggerStatus);
  useEffect(() => {
    if (selectedAgent === null || selectedContext === null || selectedContextSequence === null) return;
    onSelectedContextCommitted?.({ agentId: selectedAgent.agentId, ...selectedContext, sequence: selectedContextSequence });
  }, [onSelectedContextCommitted, selectedAgent?.agentId, selectedContext, selectedContextSequence]);

  return (
    <section className="agents-page" aria-label={t("home.agents.title")}>
      <h1 className="agents-page__title">{t("home.agents.title")}</h1>
      <div className="agents-grid" style={{ gridTemplateColumns: "minmax(0, 1fr)", overflowY: "auto" }}>
        {statusCards.length === 0 && <p className="agents-empty">{t("agents.empty.active")}</p>}
        {statusCards.map((card) => {
          if (card.kind === "profile") {
            const { profile, aggregateStatus, observations } = card.summary;
            const environments = [...new Set(observations.map((observation) => observation.environment))];
            const latestReply = observations
              .filter((observation) => observation.latestReplyPreview?.trim())
              .sort((left, right) => right.receivedAt - left.receivedAt)[0]
              ?.latestReplyPreview?.trim();
            if (environments.length === 0) environments.push(profile.environment);
            const ariaLabel = [
              profile.displayName,
              t(statusKey(aggregateStatus)),
              String(environments.length),
              ...environments.map((environment) => t(`agents.environments.${environment}` as const)),
            ].join(" ");
            return (
              <article
                key={profile.id}
                className="agent-card"
                style={{ overflow: "hidden" }}
                data-profile-id={profile.id}
                data-status={aggregateStatus}
                data-status-source={`profile:${profile.id}`}
                aria-label={ariaLabel}
              >
                <div className="agent-card__heading">
                  <span className="agent-card__identity"><StatusDot color={AGENT_STATUS_COLOR[aggregateStatus]} pulse={aggregateStatus === "running"} />{profile.displayName}</span>
                  <span className="agent-card__aggregate">{t(statusKey(aggregateStatus))}</span>
                </div>
                <div className="agent-card__sources" aria-label={`${environments.length}`}>
                  {environments.map((environment) => <div key={environment} className="agent-card__environment"><span className="agent-card__badge">{t(`agents.environments.${environment}` as const)}</span></div>)}
                </div>
                <div className="agent-card__reply">
                  <span className="agent-card__reply-label">{t("agents.reply.latest")}</span>
                  <span className="agent-card__reply-text" style={{ height: "2.9em" }}>{latestReply || t("agents.reply.empty")}</span>
                </div>
              </article>
            );
          }
          const { agent } = card;
          const sources = agent.environments.map((observation) => observation.environment);
          const latestReply = agent.environments
            .filter((observation) => observation.latestReplyPreview?.trim())
            .sort((left, right) => right.receivedAt - left.receivedAt)[0]
            ?.latestReplyPreview?.trim();
          const latestActivity = agent.environments
            .filter((observation) => observation.taskId !== "process-presence" && observation.summary.trim())
            .sort((left, right) => right.receivedAt - left.receivedAt)[0];
          const activityText = latestActivity === undefined
            ? null
            : `${latestActivity.summary.trim()} · ${t(statusKey(latestActivity.status))}`;
          const ariaLabel = [
            agent.displayName,
            t(statusKey(agent.aggregateStatus)),
            String(agent.environments.length),
            ...sources.map((environment) => t(`agents.environments.${environment}` as const)),
          ].join(" ");
          return (
            <article
              key={agent.agentId}
              className={`agent-card${selectedAgentId === agent.agentId ? " agent-card--selected" : ""}`}
              style={{ overflow: "hidden" }}
              data-agent-id={agent.agentId}
              data-status={agent.aggregateStatus}
              data-status-source={`agent:${agent.agentId}`}
              aria-label={ariaLabel}
              aria-current={selectedAgentId === agent.agentId ? "true" : undefined}
            >
              <div className="agent-card__heading">
                <span className="agent-card__identity"><StatusDot color={AGENT_STATUS_COLOR[agent.aggregateStatus]} pulse={agent.aggregateStatus === "running"} />{agent.displayName}</span>
                <span className="agent-card__aggregate">{t(statusKey(agent.aggregateStatus))}</span>
              </div>
              <div className="agent-card__sources" aria-label={`${agent.environments.length}`}>
                {agent.environments.map((observation) => (
                  <div key={`${observation.environment}-${observation.sourceEventId}`} className="agent-card__environment" data-testid={`agent-environment-${agent.agentId}-${observation.environment}`}>
                    <span className="agent-card__badge">{t(`agents.environments.${observation.environment}` as const)}</span>
                  </div>
                ))}
                {agent.environments.length === 0 && <span className="agent-card__empty">{t("agents.tasks.empty")}</span>}
              </div>
              <div className="agent-card__reply">
                <span className="agent-card__reply-label">{latestReply ? t("agents.reply.latest") : activityText ? t("agents.activity.latest") : t("agents.reply.latest")}</span>
                <span className="agent-card__reply-text" style={{ height: "2.9em" }}>{latestReply || activityText || t("agents.reply.empty")}</span>
              </div>
            </article>
          );
        })}
      </div>
      {selectedAgent && (
        <section
          className="agent-detail"
          aria-label={selectedAgent.displayName}
          style={{ maxHeight: "96px", overflowY: "auto" }}
        >
          {selectedAgent.environments.length === 0 && <span className="agent-detail__empty">{t("agents.tasks.empty")}</span>}
          <>
            {selectedAgent.environments.map((observation) => (
              <div key={`${observation.environment}-${observation.sourceEventId}`} className="agent-detail__row" aria-current={selectedContext !== null && observation.environment === selectedContext.environment && observation.taskId === selectedContext.taskId && observation.status === selectedContext.triggerStatus ? "true" : undefined}>
                <span>{t(`agents.environments.${observation.environment}` as const)}</span>
                <code>{observation.taskId}</code>
                <span>{observation.summary || t("agents.tasks.empty")}</span>
              </div>
            ))}
            {selectedContext !== null && selectedObservation === undefined && (
              <div className="agent-detail__row agent-detail__row--reminder" data-testid={`agent-reminder-context-${selectedAgent.agentId}`} data-environment={selectedContext.environment} data-task-id={selectedContext.taskId} data-trigger-status={selectedContext.triggerStatus} aria-current="true">
                <span>{t(`agents.environments.${selectedContext.environment}` as const)}</span>
                <code>{selectedContext.taskId}</code>
                <span>{t(statusKey(selectedContext.triggerStatus))}</span>
              </div>
            )}
          </>
        </section>
      )}
    </section>
  );
}
