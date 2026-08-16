import { Copy, ImageOff, Pin, PinOff, Trash2 } from "lucide-react";
import {
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

import { parseCommandError } from "../../api/commandError";
import {
  clearClipboardHistory,
  copyClipboardItem,
  deleteClipboardItem,
  getClipboardAsset,
  setClipboardPinned,
} from "../../api/commands";
import type { ClipboardContentKind, ClipboardItem, CommandError } from "../../api/contracts";
import { beginClipboardItemsSubscription } from "../../api/events";
import { useI18n } from "../../i18n/I18nProvider";
import type { TranslationKey } from "../../i18n/catalog";
import "./clipboard.css";

export interface ClipboardPageProps {
  initialKind?: ClipboardContentKind | "all";
}

type ItemAction = "copy" | "pin" | "delete";
type NoticeKey =
  | "clipboard.state.copied"
  | "clipboard.error.captureUnavailable"
  | "clipboard.error.contentTooLarge"
  | "clipboard.error.actionFailed";

const MAX_ITEMS = 500;
const SEARCH_DEBOUNCE_MS = 250;

function sortedRows(rows: ClipboardItem[]): ClipboardItem[] {
  return rows
    .map((row, index) => ({ row, index }))
    .sort((left, right) => Number(right.row.pinned) - Number(left.row.pinned) || left.index - right.index)
    .map(({ row }) => row);
}

function replaceRow(rows: ClipboardItem[], replacement: ClipboardItem): ClipboardItem[] {
  const index = rows.findIndex((row) => row.id === replacement.id);
  if (index === -1) return rows;
  const next = [...rows];
  next[index] = replacement;
  return sortedRows(next);
}

function sameClipboardRow(left: ClipboardItem, right: ClipboardItem): boolean {
  return left.id === right.id
    && left.contentKind === right.contentKind
    && left.textContent === right.textContent
    && left.assetId === right.assetId
    && left.sourceApp === right.sourceApp
    && left.pinned === right.pinned
    && left.capturedAt === right.capturedAt
    && left.lastSeenAt === right.lastSeenAt
    && left.byteSize === right.byteSize;
}

export class ClipboardMutationCoordinator {
  private clock = 0;
  private readonly active = new Map<string, number>();

  get activeCount(): number {
    return this.active.size;
  }

  begin(id: string): number {
    this.clock += 1;
    this.active.set(id, this.clock);
    return this.clock;
  }

  isCurrent(id: string, token: number): boolean {
    return this.active.get(id) === token;
  }

  finish(id: string, token: number): void {
    if (this.isCurrent(id, token)) this.active.delete(id);
  }

  invalidate(ids: Iterable<string>): void {
    for (const id of ids) this.active.delete(id);
  }
}

function boundedAccessibleLabel(content: string): string {
  let result = "";
  let count = 0;
  for (const character of content) {
    if (count === 60) return `${result}…`;
    result += character;
    count += 1;
  }
  return result;
}

function listenerNotice(error: CommandError): NoticeKey {
  if (error.code === "sourceUnavailable" || error.code === "permissionDenied") {
    return "clipboard.error.captureUnavailable";
  }
  if (error.code === "invalidInput" && error.details.reasonCode === "contentTooLarge") {
    return "clipboard.error.contentTooLarge";
  }
  return "clipboard.error.actionFailed";
}

function decodeBase64(base64: string): Uint8Array {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

function ClipboardThumbnail({ assetId }: { assetId: string | null }) {
  const { t } = useI18n();
  const [retryVersion, setRetryVersion] = useState(0);
  const [objectUrl, setObjectUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let disposed = false;
    let ownedUrl: string | null = null;
    setObjectUrl(null);
    setFailed(assetId === null);
    if (assetId === null) return () => undefined;

    void getClipboardAsset({ assetId })
      .then((payload) => {
        if (disposed) return;
        const bytes = decodeBase64(payload.base64);
        ownedUrl = URL.createObjectURL(new Blob([bytes], { type: payload.mimeType }));
        setObjectUrl(ownedUrl);
        setFailed(false);
      })
      .catch(() => {
        if (!disposed) setFailed(true);
      });

    return () => {
      disposed = true;
      if (ownedUrl !== null) URL.revokeObjectURL(ownedUrl);
    };
  }, [assetId, retryVersion]);

  if (objectUrl !== null) {
    return <img className="clipboard-thumbnail__image" src={objectUrl} alt={t("clipboard.image.alt")} />;
  }
  if (failed) {
    return <div className="clipboard-thumbnail__fallback">
      <ImageOff size={22} aria-hidden="true" />
      <span>{t("clipboard.image.unavailable")}</span>
      <button type="button" onClick={() => setRetryVersion((version) => version + 1)}>{t("action.retry")}</button>
    </div>;
  }
  return <div className="clipboard-thumbnail__loading" aria-label={t("clipboard.image.alt")} />;
}

export default function ClipboardPage({ initialKind = "all" }: ClipboardPageProps): React.JSX.Element {
  const { language, t } = useI18n();
  const [query, setQuery] = useState("");
  const [debouncedQuery, setDebouncedQuery] = useState("");
  const [kind, setKind] = useState<ClipboardContentKind | "all">(initialKind);
  const [rows, setRows] = useState<ClipboardItem[]>([]);
  const [pending, setPending] = useState<Set<string>>(() => new Set());
  const [actionNotice, setActionNotice] = useState<NoticeKey | null>(null);
  const [subscriptionNotice, setSubscriptionNotice] = useState<NoticeKey | null>(null);
  const [subscriptionRetryPending, setSubscriptionRetryPending] = useState(false);
  const [subscriptionRetryVersion, setSubscriptionRetryVersion] = useState(0);
  const pendingRef = useRef(new Set<string>());
  const rowsRef = useRef<ClipboardItem[]>([]);
  const viewGenerationRef = useRef(0);
  const mutationCoordinatorRef = useRef(new ClipboardMutationCoordinator());
  const deletedIdsRef = useRef(new Set<string>());
  const confirmedRowsRef = useRef(new Map<string, ClipboardItem>());
  const subscriptionRetryRef = useRef<(() => Promise<void>) | null>(null);
  const subscriptionRetryAttemptRef = useRef(0);
  const subscriptionRetryPendingRef = useRef(false);

  useEffect(() => { rowsRef.current = rows; }, [rows]);

  useEffect(() => {
    const timer = setTimeout(() => setDebouncedQuery((current) => {
      if (current === query) return current;
      viewGenerationRef.current += 1;
      subscriptionRetryAttemptRef.current += 1;
      subscriptionRetryPendingRef.current = false;
      setSubscriptionRetryPending(false);
      setSubscriptionNotice(null);
      mutationCoordinatorRef.current.invalidate(rowsRef.current.map((row) => row.id));
      confirmedRowsRef.current.clear();
      return query;
    }), SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [query]);

  useEffect(() => {
    let disposed = false;
    const viewGeneration = viewGenerationRef.current;
    subscriptionRetryAttemptRef.current += 1;
    subscriptionRetryPendingRef.current = false;
    setSubscriptionRetryPending(false);
    subscriptionRetryRef.current = null;
    setSubscriptionNotice(null);
    const acceptSnapshot = (snapshot: ClipboardItem[]) => {
      if (disposed || viewGeneration !== viewGenerationRef.current) return;
      const snapshotIds = new Set(snapshot.map((row) => row.id));
      for (const deletedId of deletedIdsRef.current) {
        if (!snapshotIds.has(deletedId)) deletedIdsRef.current.delete(deletedId);
      }
      for (const confirmedId of confirmedRowsRef.current.keys()) {
        if (!snapshotIds.has(confirmedId)) confirmedRowsRef.current.delete(confirmedId);
      }
      const accepted = snapshot
        .filter((row) => !deletedIdsRef.current.has(row.id))
        .map((row) => {
          const confirmed = confirmedRowsRef.current.get(row.id);
          if (confirmed === undefined) return row;
          if (sameClipboardRow(row, confirmed) || row.lastSeenAt > confirmed.lastSeenAt) {
            confirmedRowsRef.current.delete(row.id);
            return row;
          }
          return confirmed;
        });
      setRows(sortedRows(accepted));
    };
    const handle = beginClipboardItemsSubscription(
      { query: debouncedQuery, contentKind: kind, limit: MAX_ITEMS },
      (error) => {
        if (disposed) return;
        setSubscriptionNotice(listenerNotice(error));
      },
      acceptSnapshot,
    );
    void handle.ready.then((ready) => {
      if (disposed || viewGeneration !== viewGenerationRef.current) return;
      subscriptionRetryRef.current = ready.retry;
      if (ready.listenerState === "degraded") {
        setSubscriptionNotice((current) => current ?? "clipboard.error.captureUnavailable");
      } else {
        setSubscriptionNotice(null);
      }
      acceptSnapshot(ready.initial);
    }).catch((cause) => {
      if (disposed || viewGeneration !== viewGenerationRef.current) return;
      subscriptionRetryRef.current = null;
      setSubscriptionNotice(listenerNotice(parseCommandError(cause)));
    });
    return () => {
      disposed = true;
      subscriptionRetryAttemptRef.current += 1;
      subscriptionRetryPendingRef.current = false;
      handle.dispose();
    };
  }, [debouncedQuery, kind, subscriptionRetryVersion]);

  const dateFormatter = useMemo(() => new Intl.DateTimeFormat(language, {
    dateStyle: "medium",
    timeStyle: "short",
  }), [language]);

  const beginPending = (key: string): boolean => {
    if (pendingRef.current.has(key)) return false;
    pendingRef.current.add(key);
    setPending(new Set(pendingRef.current));
    return true;
  };
  const finishPending = (key: string) => {
    pendingRef.current.delete(key);
    setPending(new Set(pendingRef.current));
  };

  const changeKind = (nextKind: ClipboardContentKind | "all") => {
    if (nextKind === kind) return;
    viewGenerationRef.current += 1;
    subscriptionRetryAttemptRef.current += 1;
    subscriptionRetryPendingRef.current = false;
    setSubscriptionRetryPending(false);
    setSubscriptionNotice(null);
    mutationCoordinatorRef.current.invalidate(rowsRef.current.map((row) => row.id));
    confirmedRowsRef.current.clear();
    setKind(nextKind);
  };

  const runItemAction = async (item: ClipboardItem, action: ItemAction) => {
    const key = `${item.id}:${action}`;
    if ([...pendingRef.current].some((pendingKey) => pendingKey.startsWith("clear:"))) return;
    if (action === "delete" && !confirm(t("clipboard.confirm.delete"))) return;
    if (!beginPending(key)) return;
    const viewGeneration = viewGenerationRef.current;
    const mutationToken = mutationCoordinatorRef.current.begin(item.id);
    setActionNotice(null);
    try {
      if (action === "copy") {
        const confirmed = await copyClipboardItem({ id: item.id });
        if (viewGenerationRef.current === viewGeneration
          && mutationCoordinatorRef.current.isCurrent(item.id, mutationToken)
          && !deletedIdsRef.current.has(item.id)) {
          confirmedRowsRef.current.set(item.id, confirmed);
          setRows((current) => replaceRow(current, confirmed));
        }
        setActionNotice("clipboard.state.copied");
      } else if (action === "pin") {
        const confirmed = await setClipboardPinned({ id: item.id, pinned: !item.pinned });
        if (viewGenerationRef.current === viewGeneration
          && mutationCoordinatorRef.current.isCurrent(item.id, mutationToken)
          && !deletedIdsRef.current.has(item.id)) {
          confirmedRowsRef.current.set(item.id, confirmed);
          setRows((current) => replaceRow(current, confirmed));
        }
      } else {
        await deleteClipboardItem({ id: item.id });
        deletedIdsRef.current.add(item.id);
        confirmedRowsRef.current.delete(item.id);
        mutationCoordinatorRef.current.invalidate([item.id]);
        setRows((current) => current.filter((row) => row.id !== item.id));
      }
    } catch (cause) {
      const error = parseCommandError(cause);
      setActionNotice(error.code === "invalidInput" && error.details.reasonCode === "contentTooLarge"
        ? "clipboard.error.contentTooLarge"
        : "clipboard.error.actionFailed");
    } finally {
      mutationCoordinatorRef.current.finish(item.id, mutationToken);
      finishPending(key);
    }
  };

  const clearRows = async (keepPinned: boolean) => {
    const key = keepPinned ? "clear:unpinned" : "clear:all";
    if (pendingRef.current.size > 0) return;
    if (!confirm(t("clipboard.confirm.clear")) || !beginPending(key)) return;
    const targetIds = new Set(rowsRef.current
      .filter((row) => !keepPinned || !row.pinned)
      .map((row) => row.id));
    setActionNotice(null);
    try {
      await clearClipboardHistory({ keepPinned });
      for (const id of targetIds) {
        deletedIdsRef.current.add(id);
        confirmedRowsRef.current.delete(id);
      }
      mutationCoordinatorRef.current.invalidate(targetIds);
      setRows((current) => current.filter((row) => !targetIds.has(row.id)));
    } catch {
      setActionNotice("clipboard.error.actionFailed");
    } finally {
      finishPending(key);
    }
  };

  const retrySubscription = async () => {
    if (subscriptionRetryPendingRef.current) return;
    subscriptionRetryPendingRef.current = true;
    setSubscriptionRetryPending(true);
    subscriptionRetryAttemptRef.current += 1;
    const attempt = subscriptionRetryAttemptRef.current;
    const viewGeneration = viewGenerationRef.current;
    const retry = subscriptionRetryRef.current;
    const isCurrent = () => subscriptionRetryAttemptRef.current === attempt
      && viewGenerationRef.current === viewGeneration;
    try {
      if (retry === null) {
        if (isCurrent()) {
          setSubscriptionNotice(null);
          setSubscriptionRetryVersion((version) => version + 1);
        }
      } else {
        await retry();
      }
    } catch (cause) {
      if (isCurrent()) setSubscriptionNotice(listenerNotice(parseCommandError(cause)));
    } finally {
      if (isCurrent()) {
        subscriptionRetryPendingRef.current = false;
        setSubscriptionRetryPending(false);
      }
    }
  };

  const clearPending = [...pending].some((key) => key.startsWith("clear:"));
  const anyPending = pending.size > 0;

  return <section className="clipboard-page">
    <header className="clipboard-header">
      <div>
        <h1>{t("clipboard.title")}</h1>
      </div>
      <div className="clipboard-clear-actions">
        <button type="button" disabled={anyPending} onClick={() => void clearRows(true)}>{t("clipboard.action.clearUnpinned")}</button>
        <button type="button" disabled={anyPending} onClick={() => void clearRows(false)}>{t("clipboard.action.clear")}</button>
      </div>
    </header>

    <div className="clipboard-controls">
      <label className="clipboard-search">
        <span className="sr-only">{t("clipboard.field.search")}</span>
        <input type="search" aria-label={t("clipboard.field.search")} placeholder={t("clipboard.field.search")} value={query} onChange={(event) => setQuery(event.target.value)} />
      </label>
      <div className="clipboard-kind-filter" role="group" aria-label={t("clipboard.title")}>
        {(["all", "text", "image"] as const).map((value) => {
          const labelKey: TranslationKey = value === "all" ? "clipboard.filter.all" : value === "text" ? "clipboard.filter.text" : "clipboard.filter.image";
          return <button key={value} type="button" aria-pressed={kind === value} onClick={() => changeKind(value)}>{t(labelKey)}</button>;
        })}
      </div>
    </div>

    {subscriptionNotice !== null && <div className="clipboard-notice" role="status">
      <span>{t(subscriptionNotice)}</span>
      <button type="button" disabled={subscriptionRetryPending} onClick={() => void retrySubscription()}>{t("action.retry")}</button>
    </div>}
    {actionNotice !== null && <p className={actionNotice === "clipboard.state.copied" ? "clipboard-notice clipboard-notice--success" : "clipboard-notice"} role="status">{t(actionNotice)}</p>}

    <div className="clipboard-scroll">
      {rows.length === 0
        ? <div className="clipboard-empty">{t("clipboard.empty")}</div>
        : <ol className="clipboard-timeline">
          {rows.map((item) => {
            const label = item.contentKind === "text"
              ? boundedAccessibleLabel(item.textContent ?? t("clipboard.filter.text"))
              : t("clipboard.image.alt");
            return <li key={item.id} className={`clipboard-card${item.pinned ? " clipboard-card--pinned" : ""}`} data-testid={`clipboard-item-${item.id}`}>
              <span className="clipboard-timeline__dot" aria-hidden="true" />
              <div className="clipboard-card__meta">
                <span>{item.sourceApp ?? t("clipboard.source.unknown")}</span>
                <time dateTime={new Date(item.capturedAt).toISOString()}>{dateFormatter.format(item.capturedAt)}</time>
              </div>
              <div className="clipboard-card__content">
                {item.contentKind === "image" ? <ClipboardThumbnail assetId={item.assetId} /> : <p>{item.textContent}</p>}
              </div>
              <div className="clipboard-card__actions">
                <TooltipButton label={`${t("clipboard.action.copy")} — ${label}`} tooltip={t("clipboard.action.copy")} disabled={clearPending || pending.has(`${item.id}:copy`)} onClick={() => void runItemAction(item, "copy")}><Copy size={14} /></TooltipButton>
                <TooltipButton label={`${t(item.pinned ? "clipboard.action.unpin" : "clipboard.action.pin")} — ${label}`} tooltip={t(item.pinned ? "clipboard.action.unpin" : "clipboard.action.pin")} disabled={clearPending || pending.has(`${item.id}:pin`)} onClick={() => void runItemAction(item, "pin")}>{item.pinned ? <PinOff size={14} /> : <Pin size={14} />}</TooltipButton>
                <TooltipButton label={`${t("clipboard.action.delete")} — ${label}`} tooltip={t("clipboard.action.delete")} disabled={clearPending || pending.has(`${item.id}:delete`)} onClick={() => void runItemAction(item, "delete")}><Trash2 size={14} /></TooltipButton>
              </div>
            </li>;
          })}
        </ol>}
    </div>
  </section>;
}

type TooltipButtonProps = {
  children: ReactNode;
  disabled: boolean;
  label: string;
  onClick: () => void;
  tooltip: string;
};

function TooltipButton({ children, disabled, label, onClick, tooltip }: TooltipButtonProps) {
  const buttonRef = useRef<HTMLButtonElement>(null);
  const tooltipRef = useRef<HTMLSpanElement>(null);
  const tooltipId = useId();
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null);
  const visible = hovered || focused;

  useLayoutEffect(() => {
    if (!visible || buttonRef.current === null || tooltipRef.current === null) return;
    const button = buttonRef.current;
    const tooltipElement = tooltipRef.current;
    const update = () => {
      const anchor = button.getBoundingClientRect();
      const box = tooltipElement.getBoundingClientRect();
      const left = Math.max(4, Math.min(anchor.right - box.width, window.innerWidth - box.width - 4));
      const preferredTop = anchor.top - box.height - 5;
      const top = preferredTop >= 4 ? preferredTop : Math.min(window.innerHeight - box.height - 12, anchor.bottom + 5);
      setPosition({ left, top });
    };
    update();
    const viewport = button.closest<HTMLElement>(".clipboard-scroll");
    viewport?.addEventListener("scroll", update, { passive: true });
    window.addEventListener("resize", update);
    return () => {
      viewport?.removeEventListener("scroll", update);
      window.removeEventListener("resize", update);
    };
  }, [visible]);

  return <>
    <button ref={buttonRef} type="button" className="clipboard-icon-button" aria-label={label} aria-describedby={visible ? tooltipId : undefined} title={tooltip} disabled={disabled} onClick={onClick} onMouseEnter={() => setHovered(true)} onMouseLeave={() => setHovered(false)} onFocus={() => setFocused(true)} onBlur={() => setFocused(false)}>{children}</button>
    {visible && createPortal(<span ref={tooltipRef} id={tooltipId} role="tooltip" className="clipboard-tooltip" style={{ position: "fixed", pointerEvents: "none", left: position?.left ?? -10_000, top: position?.top ?? -10_000, visibility: position ? "visible" : "hidden" }}>{tooltip}</span>, document.body)}
  </>;
}
