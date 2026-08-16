import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  clearNotificationHistory,
  deleteNotificationHistory,
  setNotificationRead,
} from "../api/commands";
import { parseCommandError } from "../api/commandError";
import type {
  CommandError,
  ListNotificationHistoryInput,
  NotificationHistoryItem,
} from "../api/contracts";
import { beginNotificationHistorySubscription, type CommandSubscription } from "../api/events";
import { useI18n } from "../i18n/I18nProvider";
import { translateRegisteredMessage } from "../i18n/catalog";

type NotificationOrigin = ListNotificationHistoryInput["origin"];

const newestFirst = (items: NotificationHistoryItem[]) => [...items].sort((left, right) =>
  right.receivedAt - left.receivedAt || right.id.localeCompare(left.id),
);

function contextLabel(item: NotificationHistoryItem, label: (key: "todo.title" | "monitor.title") => string) {
  const context = item.sourceContext;
  if (context === null) return item.appId;
  if (context.kind === "todo") return `${label("todo.title")} · ${context.todoId}`;
  if (context.kind === "monitor") return `${label("monitor.title")} · ${context.metric}`;
  return `${context.agentId} · ${context.environment}`;
}

function NotificationCard({
  item,
  pending,
  onRead,
  onDelete,
}: {
  item: NotificationHistoryItem;
  pending: boolean;
  onRead(item: NotificationHistoryItem): void;
  onDelete(item: NotificationHistoryItem): void;
}) {
  const { language, t } = useI18n();
  const [expanded, setExpanded] = useState(false);
  const translated = useMemo(() => {
    if (item.origin === "windows" || item.messageKey === null) return null;
    try {
      return translateRegisteredMessage(language, item.messageKey, item.messageParameters);
    } catch {
      return t("notifications.messageUnavailable");
    }
  }, [item.messageKey, item.messageParameters, item.origin, language, t]);
  const title = translated ?? item.title;
  const body = item.origin === "windows" ? item.body : "";
  const longBody = Array.from(body).length > 180;
  const received = new Intl.DateTimeFormat(language, { dateStyle: "short", timeStyle: "short" }).format(item.receivedAt);
  const occurred = new Intl.DateTimeFormat(language, { dateStyle: "short", timeStyle: "short" }).format(item.sourceOccurredAt);
  const bodyId = `notification-body-${item.id}`;

  return (
    <article
      className={`notification-card notification-card--${item.origin}${item.readAt === null ? " notification-card--unread" : ""}`}
      data-notification-id={item.id}
      data-testid={`notification-${item.id}`}
      data-unread={item.readAt === null ? "true" : "false"}
      role="listitem"
    >
      <span className="notification-card__rail" aria-hidden="true" />
      <header className="notification-card__header">
        <span className="notification-card__origin" data-origin={item.origin} aria-label={t(`notifications.origin.${item.origin}`)}>{item.origin === "windows" ? "W" : "A"}</span>
        <span className="notification-card__app" title={item.appId}>{item.appId}</span>
        <time dateTime={new Date(item.receivedAt).toISOString()}>{received}</time>
        <span className="notification-card__read-state">{t(item.readAt === null ? "notifications.state.unread" : "notifications.state.read")}</span>
        {item.readAt === null && <span className="notification-card__unread" aria-hidden="true" />}
      </header>
      <h2>{title}</h2>
      {body && <p id={bodyId} className={`notification-card__body${expanded ? " notification-card__body--expanded" : ""}`}>{body}</p>}
      {longBody && (
        <button
          className="notification-card__expand"
          type="button"
          aria-expanded={expanded}
          aria-controls={bodyId}
          onClick={() => setExpanded((current) => !current)}
        >
          {t(expanded ? "notifications.collapse" : "notifications.expand")}
        </button>
      )}
      <footer>
        <span className="notification-card__context">{contextLabel(item, t)} · {occurred}</span>
        <div className="notification-card__actions">
          <button type="button" disabled={pending} onClick={() => onRead(item)}>
            {t(item.readAt === null ? "notifications.markRead" : "notifications.markUnread")}
          </button>
          <button type="button" disabled={pending} onClick={() => onDelete(item)}>{t("notifications.delete")}</button>
        </div>
      </footer>
    </article>
  );
}

export default function NotificationCenterPage() {
  const { t } = useI18n();
  const [origin, setOrigin] = useState<NotificationOrigin>("all");
  const [sourceApp, setSourceApp] = useState<string | null>(null);
  const [unreadOnly, setUnreadOnly] = useState(false);
  const [rows, setRows] = useState<NotificationHistoryItem[]>([]);
  const [sourceApps, setSourceApps] = useState<string[]>([]);
  const [subscriptionError, setSubscriptionError] = useState<CommandError | null>(null);
  const [listenerDegraded, setListenerDegraded] = useState(false);
  const [retryPending, setRetryPending] = useState(false);
  const [actionError, setActionError] = useState<CommandError | null>(null);
  const [pendingAction, setPendingAction] = useState<string | null>(null);
  const rowsRef = useRef<NotificationHistoryItem[]>([]);
  const subscriptionRef = useRef<CommandSubscription<NotificationHistoryItem[]> | null>(null);
  const lifecycleRef = useRef(0);
  const actionLockRef = useRef(false);
  const retryTokenRef = useRef<object | null>(null);
  const deletedIdsRef = useRef(new Set<string>());
  const confirmedRowsRef = useRef(new Map<string, NotificationHistoryItem>());

  const applyRows = useCallback((incoming: NotificationHistoryItem[]) => {
    setSourceApps((current) => [...new Set([...current, ...incoming.map((item) => item.appId).filter(Boolean)])]
      .sort((a, b) => a.localeCompare(b))
      .slice(0, 500));
    const incomingIds = new Set(incoming.map((item) => item.id));
    for (const id of [...deletedIdsRef.current]) {
      if (!incomingIds.has(id)) deletedIdsRef.current.delete(id);
    }
    const merged = incoming
      .filter((item) => !deletedIdsRef.current.has(item.id))
      .map((item) => {
        const confirmed = confirmedRowsRef.current.get(item.id);
        if (confirmed === undefined) return item;
        if (confirmed.readAt === item.readAt) {
          confirmedRowsRef.current.delete(item.id);
          return item;
        }
        return confirmed;
      })
      .filter((item) => !(unreadOnly && item.readAt !== null));
    for (const id of [...confirmedRowsRef.current.keys()]) {
      if (!incomingIds.has(id)) confirmedRowsRef.current.delete(id);
    }
    const sorted = newestFirst(merged);
    rowsRef.current = sorted;
    setRows(sorted);
  }, [unreadOnly]);

  useEffect(() => {
    const lifecycle = lifecycleRef.current + 1;
    lifecycleRef.current = lifecycle;
    subscriptionRef.current = null;
    retryTokenRef.current = null;
    setSubscriptionError(null);
    setListenerDegraded(false);
    setRetryPending(false);
    const input: ListNotificationHistoryInput = { origin, sourceApp, unreadOnly, limit: 500 };
    const handle = beginNotificationHistorySubscription(
      input,
      (error) => {
        if (lifecycleRef.current !== lifecycle) return;
        setSubscriptionError(error);
        setListenerDegraded(true);
      },
      (snapshot) => {
        if (lifecycleRef.current === lifecycle) applyRows(snapshot);
      },
    );
    void handle.ready.then((subscription) => {
      if (lifecycleRef.current !== lifecycle) return;
      subscriptionRef.current = subscription;
      setListenerDegraded(subscription.listenerState === "degraded");
      applyRows(subscription.initial);
    }).catch((error) => {
      if (lifecycleRef.current === lifecycle) setSubscriptionError(parseCommandError(error));
    });
    return () => {
      if (lifecycleRef.current === lifecycle) lifecycleRef.current += 1;
      subscriptionRef.current = null;
      handle.dispose();
    };
  }, [applyRows, origin, sourceApp, unreadOnly]);

  const runAction = useCallback(async (key: string, operation: (lifecycle: number) => Promise<void>) => {
    if (actionLockRef.current) return;
    actionLockRef.current = true;
    const lifecycle = lifecycleRef.current;
    setPendingAction(key);
    setActionError(null);
    try {
      await operation(lifecycle);
    } catch (error) {
      if (lifecycleRef.current === lifecycle) setActionError(parseCommandError(error));
    } finally {
      actionLockRef.current = false;
      if (lifecycleRef.current === lifecycle) setPendingAction(null);
    }
  }, []);

  const changeRead = useCallback((item: NotificationHistoryItem) => {
    void runAction(`read:${item.id}`, async (lifecycle) => {
      const confirmed = await setNotificationRead({ id: item.id, read: item.readAt === null });
      if (lifecycleRef.current !== lifecycle) return;
      confirmedRowsRef.current.set(confirmed.id, confirmed);
      const next = rowsRef.current
        .map((row) => row.id === confirmed.id ? confirmed : row)
        .filter((row) => !(unreadOnly && row.readAt !== null));
      rowsRef.current = next;
      setRows(next);
    });
  }, [runAction, unreadOnly]);

  const remove = useCallback((item: NotificationHistoryItem) => {
    if (!window.confirm(t("notifications.deleteConfirm"))) return;
    void runAction(`delete:${item.id}`, async (lifecycle) => {
      await deleteNotificationHistory({ id: item.id, confirmRemoval: true });
      if (lifecycleRef.current !== lifecycle) return;
      deletedIdsRef.current.add(item.id);
      confirmedRowsRef.current.delete(item.id);
      const next = rowsRef.current.filter((row) => row.id !== item.id);
      rowsRef.current = next;
      setRows(next);
    });
  }, [runAction, t]);

  const clear = useCallback(() => {
    if (!window.confirm(t("notifications.clearConfirm"))) return;
    const before = Date.now();
    const targetIds = rowsRef.current.filter((row) => row.receivedAt < before).map((row) => row.id);
    void runAction("clear", async (lifecycle) => {
      await clearNotificationHistory({ before, confirmRemoval: true });
      if (lifecycleRef.current !== lifecycle) return;
      for (const id of targetIds) {
        deletedIdsRef.current.add(id);
        confirmedRowsRef.current.delete(id);
      }
      const targets = new Set(targetIds);
      const next = rowsRef.current.filter((row) => !targets.has(row.id));
      rowsRef.current = next;
      setRows(next);
    });
  }, [runAction, t]);

  const retry = useCallback(async () => {
    const subscription = subscriptionRef.current;
    if (subscription === null || retryTokenRef.current !== null) return;
    const lifecycle = lifecycleRef.current;
    const token = {};
    retryTokenRef.current = token;
    setRetryPending(true);
    try {
      await subscription.retry();
      if (lifecycleRef.current === lifecycle) {
        const degraded = subscription.listenerState === "degraded";
        setListenerDegraded(degraded);
        if (!degraded) setSubscriptionError(null);
      }
    } catch (error) {
      if (lifecycleRef.current === lifecycle) setSubscriptionError(parseCommandError(error));
    } finally {
      if (retryTokenRef.current === token) {
        retryTokenRef.current = null;
        if (lifecycleRef.current === lifecycle) setRetryPending(false);
      }
    }
  }, []);

  const subscriptionCopy = subscriptionError?.code === "notificationUnavailable" && subscriptionError.details.reasonCode === "schemaIncompatible"
    ? t("notifications.schemaIncompatible")
    : t("notifications.sourceUnavailable");

  return (
    <section className="notification-center" onPointerDownCapture={(event) => event.stopPropagation()}>
      <header className="notification-center__header">
        <h1>{t("notifications.title")}</h1>
        <button type="button" disabled={pendingAction !== null || rows.length === 0} onClick={clear}>{t("notifications.clear")}</button>
      </header>
      <div className="notification-filters" aria-label={t("notifications.filter.label")}>
        <div className="notification-origin-filter" role="group" aria-label={t("notifications.filter.origin")}>
          {(["all", "windows", "aiceland"] as const).map((value) => (
            <button
              type="button"
              key={value}
              aria-pressed={origin === value}
              disabled={pendingAction !== null}
              onClick={() => setOrigin(value)}
            >
              {t(`notifications.origin.${value}`)}
            </button>
          ))}
        </div>
        <label>
          <span>{t("notifications.filter.source")}</span>
          <select disabled={pendingAction !== null} value={sourceApp ?? ""} onChange={(event) => setSourceApp(event.target.value || null)}>
            <option value="">{t("notifications.origin.all")}</option>
            {sourceApps.map((app) => <option key={app} value={app}>{app}</option>)}
          </select>
        </label>
        <label className="notification-filter-check">
          <input type="checkbox" disabled={pendingAction !== null} checked={unreadOnly} onChange={(event) => setUnreadOnly(event.target.checked)} />
          <span>{t("notifications.filter.unread")}</span>
        </label>
      </div>
      {(listenerDegraded || subscriptionError !== null) && (
        <div className="notification-source-notice" role="alert">
          <span>{subscriptionCopy}</span>
          <button type="button" disabled={retryPending} onClick={() => void retry()}>{t("action.retry")}</button>
        </div>
      )}
      {actionError !== null && <p className="notification-action-error" role="alert">{t("notifications.actionFailed")}</p>}
      {rows.length === 0 ? (
        <p className="notification-empty">{t("notifications.empty")}</p>
      ) : (
        <div className="notification-list" role="list" style={{ gridAutoRows: "max-content" }}>
          {rows.map((item) => (
            <NotificationCard
              key={item.id}
              item={item}
              pending={pendingAction !== null}
              onRead={changeRead}
              onDelete={remove}
            />
          ))}
        </div>
      )}
    </section>
  );
}
