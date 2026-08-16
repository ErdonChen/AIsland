import { useCallback, useEffect, useRef, useState } from "react";
import { flushSync } from "react-dom";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { acknowledgeReminder, completeReminder, reloadReminderAlertGroup, snoozeReminder } from "../api/commands";
import { beginReminderDispatchSubscription } from "../api/events";
import type { ReminderAlertGroup, ReminderDelivery, ReminderMergeIdentity } from "../api/contracts";
import { translateRegisteredMessage } from "../i18n/catalog";
import { useI18n } from "../i18n/I18nProvider";

export interface ReminderAlertAppProps { consumerId: "reminder-alert-window"; }

export function isReminderAlertWindow(label: string) { return label === "reminder-alert"; }

function identityFor(delivery: ReminderDelivery): ReminderMergeIdentity {
  const source = delivery.sourceContext;
  if (source.kind === "agent") return { kind: "agent", ruleId: delivery.ruleId ?? "", agentId: source.agentId, environment: source.environment, taskId: source.taskId, triggerStatus: source.triggerStatus };
  if (source.kind === "todo") return { kind: "todo", todoId: source.todoId, reminderRevision: source.reminderRevision, deliveryId: delivery.id };
  return { kind: "monitor", thresholdId: source.thresholdId, breachStartedAt: source.breachStartedAt, deliveryId: delivery.id };
}

function mergeKeyFor(delivery: ReminderDelivery) {
  // Agent task IDs may contain colons, so the native encoded source entity is the exact grouping key.
  return delivery.sourceKind === "agent" ? `agent:${delivery.sourceEntityId}` : `${delivery.sourceKind}:${delivery.id}`;
}

function sourceKeyFor(delivery: ReminderDelivery) {
  return `${delivery.sourceKind}\u0000${delivery.sourceEntityId}`;
}

export function groupReminderDeliveries(deliveries: ReminderDelivery[]): ReminderAlertGroup[] {
  const result = new Map<string, ReminderAlertGroup>();
  for (const delivery of [...deliveries].sort((a, b) => a.dispatchSeq - b.dispatchSeq || a.id.localeCompare(b.id))) {
    if (delivery.state !== "dispatched" && delivery.state !== "acknowledged") continue;
    const key = mergeKeyFor(delivery);
    const existing = result.get(key);
    if (existing) {
      existing.members.push(delivery);
    } else result.set(key, { mergeKey: key, mergeIdentity: identityFor(delivery), members: [delivery], sourceContext: delivery.sourceContext, newestSourceOccurredAt: delivery.sourceOccurredAt });
  }
  return [...result.values()].map((group) => {
    const members = group.members.sort((a, b) => a.id.localeCompare(b.id));
    const representative = representativeFor(members);
    return { ...group, members, sourceContext: representative.sourceContext, newestSourceOccurredAt: representative.sourceOccurredAt };
  });
}

function representativeFor(members: ReminderDelivery[]) {
  return members.reduce((newest, candidate) => candidate.sourceOccurredAt > newest.sourceOccurredAt || (candidate.sourceOccurredAt === newest.sourceOccurredAt && candidate.id > newest.id) ? candidate : newest);
}

function actionInput(group: ReminderAlertGroup) {
  const members = [...group.members].sort((a, b) => a.id.localeCompare(b.id)).map(({ id, state }) => ({ id, expectedState: state }));
  return { mergeIdentity: group.mergeIdentity, expectedMemberDeliveryIds: members.map(({ id }) => id), members };
}

export interface ReminderAlertRecovery {
  sourceKey: string;
  generation: number;
  reloadStartedVersion: number;
}

export interface ReminderAlertRecoveryOutcome {
  applied: boolean;
  groups: ReminderAlertGroup[];
}

export interface ReminderAlertStateStats {
  activeDeliveryCount: number;
  tombstoneCount: number;
  recoveringSourceCount: number;
}

function isActionable(delivery: ReminderDelivery) {
  return delivery.state === "dispatched" || delivery.state === "acknowledged";
}

export class ReminderAlertStateCoordinator {
  private readonly deliveries = new Map<string, ReminderDelivery>();
  private readonly deliveryVersions = new Map<string, number>();
  private readonly tombstones = new Map<string, { sourceKey: string; version: number }>();
  private readonly sourceRecoveries = new Map<string, { latestStarted: number; inFlight: Set<number> }>();
  private deliveryVersion = 0;
  private recoveryGeneration = 0;

  render(rows: ReminderDelivery[]) {
    return this.write(rows);
  }

  applyAction(rows: ReminderDelivery[]) {
    return this.write(rows);
  }

  groups() {
    return groupReminderDeliveries([...this.deliveries.values()]);
  }

  stats(): ReminderAlertStateStats {
    return {
      activeDeliveryCount: this.deliveries.size,
      tombstoneCount: this.tombstones.size,
      recoveringSourceCount: this.sourceRecoveries.size,
    };
  }

  beginRecovery(group: ReminderAlertGroup): ReminderAlertRecovery {
    const representative = group.members[0];
    if (representative === undefined) throw new Error("cannot recover an empty reminder group");
    const sourceKey = sourceKeyFor(representative);
    const generation = ++this.recoveryGeneration;
    const recovery = this.sourceRecoveries.get(sourceKey) ?? { latestStarted: generation, inFlight: new Set<number>() };
    recovery.latestStarted = generation;
    recovery.inFlight.add(generation);
    this.sourceRecoveries.set(sourceKey, recovery);
    return { sourceKey, generation, reloadStartedVersion: this.deliveryVersion };
  }

  resolveRecovery(token: ReminderAlertRecovery, fresh: ReminderAlertGroup | null): ReminderAlertRecoveryOutcome {
    const recovery = this.sourceRecoveries.get(token.sourceKey);
    const applied = recovery?.inFlight.has(token.generation) === true
      && recovery.latestStarted === token.generation;
    if (applied) this.replaceSource(token, fresh === null ? [] : fresh.members);
    this.finishRecovery(token);
    return { applied, groups: this.groups() };
  }

  rejectRecovery(token: ReminderAlertRecovery) {
    this.finishRecovery(token);
  }

  private write(rows: ReminderDelivery[]) {
    const version = ++this.deliveryVersion;
    rows.forEach((row) => {
      const sourceKey = sourceKeyFor(row);
      if (isActionable(row)) {
        this.deliveries.set(row.id, row);
        this.deliveryVersions.set(row.id, version);
        this.tombstones.delete(row.id);
      } else {
        this.deliveries.delete(row.id);
        this.deliveryVersions.delete(row.id);
        if (this.sourceRecoveries.has(sourceKey)) this.tombstones.set(row.id, { sourceKey, version });
        else this.tombstones.delete(row.id);
      }
    });
    return this.groups();
  }

  private replaceSource(token: ReminderAlertRecovery, refreshedMembers: ReminderDelivery[]) {
    for (const [id, delivery] of this.deliveries) {
      if (
        sourceKeyFor(delivery) === token.sourceKey
        && (this.deliveryVersions.get(id) ?? 0) <= token.reloadStartedVersion
      ) {
        this.deliveries.delete(id);
        this.deliveryVersions.delete(id);
      }
    }
    refreshedMembers.forEach((member) => {
      if (sourceKeyFor(member) !== token.sourceKey || !isActionable(member)) return;
      if ((this.tombstones.get(member.id)?.version ?? 0) > token.reloadStartedVersion) return;
      const currentVersion = this.deliveryVersions.get(member.id);
      if ((currentVersion ?? 0) > token.reloadStartedVersion) return;
      this.deliveries.set(member.id, member);
      this.deliveryVersions.set(member.id, currentVersion ?? token.reloadStartedVersion);
      this.tombstones.delete(member.id);
    });
  }

  private finishRecovery(token: ReminderAlertRecovery) {
    const recovery = this.sourceRecoveries.get(token.sourceKey);
    if (recovery === undefined || !recovery.inFlight.delete(token.generation)) return;
    if (recovery.inFlight.size > 0) return;
    this.sourceRecoveries.delete(token.sourceKey);
    for (const [id, tombstone] of this.tombstones) {
      if (tombstone.sourceKey === token.sourceKey) this.tombstones.delete(id);
    }
  }
}

export function ReminderAlertApp({ consumerId }: ReminderAlertAppProps) {
  const { language, t } = useI18n();
  const [groups, setGroups] = useState<ReminderAlertGroup[]>([]);
  const alertState = useRef<ReminderAlertStateCoordinator | null>(null);
  if (alertState.current === null) alertState.current = new ReminderAlertStateCoordinator();
  const render = useCallback(async (rows: ReminderDelivery[]) => {
    // The shared replay consumer awaits this callback before committing its cursor.
    await Promise.resolve();
    const next = alertState.current!.render(rows);
    flushSync(() => setGroups(next));
    if (next.length === 0) void getCurrentWindow().hide();
  }, []);
  useEffect(() => {
    const handle = beginReminderDispatchSubscription({ consumerId, render, onListenerFailure: (error) => console.error("Failed to listen for reminder dispatch", error) });
    void handle.ready.catch((error) => console.error("Failed to replay reminder alerts", error));
    return () => handle.dispose();
  }, [consumerId, render]);
  const apply = useCallback(async (group: ReminderAlertGroup, action: "acknowledge" | "complete" | "snooze") => {
    try {
      const input = actionInput(group);
      const updated = action === "acknowledge" ? await acknowledgeReminder(input) : action === "complete" ? await completeReminder(input) : await snoozeReminder({ ...input, snoozedUntil: Date.now() + 300_000 });
      const next = alertState.current!.applyAction(updated.members);
      setGroups(next);
      if (next.length === 0) void getCurrentWindow().hide();
    } catch (error) {
      // No optimistic disappearance: replace only after the cursor-independent authoritative read resolves.
      console.error("Failed to apply reminder action", error);
      const recovery = alertState.current!.beginRecovery(group);
      try {
        const fresh = await reloadReminderAlertGroup({ deliveryId: group.members[0].id });
        const recovered = alertState.current!.resolveRecovery(recovery, fresh);
        if (recovered.applied) setGroups(recovered.groups);
      } catch (reloadError) {
        alertState.current!.rejectRecovery(recovery);
        console.error("Failed to reload reminder alert group", reloadError);
      }
    }
  }, []);
  return <main className="reminder-alert-window" onKeyDown={(event) => {
    if (event.key === "Escape") { event.preventDefault(); void getCurrentWindow().hide(); }
  }}>
    {groups.map((group) => {
      const delivery = representativeFor(group.members);
      const message = translateRegisteredMessage(language, delivery.messageKey, delivery.messageParameters);
      const source = group.sourceContext;
      const label = `${message} ${source.kind === "agent" ? source.taskTitle ?? source.taskId : message}`;
      return <article key={group.mergeKey} className="reminder-alert-card" aria-label={label} tabIndex={-1}>
        <p className="reminder-alert-card__message">{message}</p>
        <time className="reminder-alert-card__occurred" dateTime={String(source.sourceOccurredAt)}>{t("alert.occurredAt").replace("{time}", new Date(source.sourceOccurredAt).toLocaleTimeString())}</time>
        {source.kind !== "agent" && <p className="reminder-alert-card__context">{t("alert.unknownContext")}</p>}
        {group.members.length > 1 && <p className="reminder-alert-card__merged">{t("alert.mergedCount").replace("{count}", String(group.members.length))}</p>}
        <div className="reminder-alert-card__actions">
          <button type="button" title={t("alert.acknowledge")} onClick={() => void apply(group, "acknowledge")}>{t("alert.acknowledge")}</button>
          <button type="button" title={t("alert.complete")} onClick={() => void apply(group, "complete")}>{t("alert.complete")}</button>
          <button type="button" title={t("alert.snooze")} onClick={() => void apply(group, "snooze")}>{t("alert.snooze")}</button>
        </div>
      </article>;
    })}
  </main>;
}

export default ReminderAlertApp;
