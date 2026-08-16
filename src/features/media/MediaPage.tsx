import { Pause, Play, SkipBack, SkipForward } from "lucide-react";
import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

import { sendMediaCommand } from "../../api/commands";
import type { MediaControlInput, MediaSnapshot } from "../../api/contracts";
import { beginMediaSnapshotSubscription } from "../../api/events";
import { useI18n } from "../../i18n/I18nProvider";
import type { TranslationKey } from "../../i18n/catalog";
import "./media.css";

export interface MediaPageProps {
  progressTickMs?: number;
}

const unavailableSnapshot = (): MediaSnapshot => ({
  sessionId: null,
  title: "",
  artist: "",
  playbackState: "unavailable",
  positionSeconds: 0,
  durationSeconds: null,
  volumePercent: null,
  canPlay: false,
  canPause: false,
  canPrevious: false,
  canNext: false,
  canSeek: false,
  canSetVolume: false,
  updatedAt: 0,
});

const clamp = (value: number, minimum: number, maximum: number) => Math.min(maximum, Math.max(minimum, value));

function formatDuration(seconds: number | null): string {
  if (seconds === null) return "--:--";
  const bounded = Math.max(0, Math.floor(seconds));
  const minutes = Math.floor(bounded / 60);
  return `${minutes}:${String(bounded % 60).padStart(2, "0")}`;
}

export default function MediaPage({ progressTickMs = 1_000 }: MediaPageProps): React.JSX.Element {
  const { t } = useI18n();
  const [confirmed, setConfirmed] = useState<MediaSnapshot>(unavailableSnapshot);
  const [seekDraft, setSeekDraft] = useState(0);
  const [volumeDraft, setVolumeDraft] = useState(0);
  const [clock, setClock] = useState(() => Date.now());
  const [controlPending, setControlPending] = useState(false);
  const [controlFailed, setControlFailed] = useState(false);
  const [listenerDegraded, setListenerDegraded] = useState(false);
  const [subscriptionRetryPending, setSubscriptionRetryPending] = useState(false);
  const [subscriptionRetryVersion, setSubscriptionRetryVersion] = useState(0);
  const lifecycleRef = useRef(0);
  const pendingRef = useRef(false);
  const seekDirtyRef = useRef(false);
  const volumeDirtyRef = useRef(false);
  const subscriptionRetryRef = useRef<(() => Promise<void>) | null>(null);
  const subscriptionRetryPendingRef = useRef(false);
  const subscriptionRetryAttemptRef = useRef(0);

  const acceptSnapshot = (snapshot: MediaSnapshot) => {
    setConfirmed(snapshot);
    setClock(Date.now());
    if (!seekDirtyRef.current) setSeekDraft(snapshot.positionSeconds);
    if (!volumeDirtyRef.current) setVolumeDraft(snapshot.volumePercent ?? 0);
  };

  useEffect(() => {
    const lifecycle = lifecycleRef.current + 1;
    lifecycleRef.current = lifecycle;
    subscriptionRetryAttemptRef.current += 1;
    subscriptionRetryPendingRef.current = false;
    setSubscriptionRetryPending(false);
    subscriptionRetryRef.current = null;
    const current = () => lifecycleRef.current === lifecycle;
    const handle = beginMediaSnapshotSubscription(
      () => {
        if (current()) setListenerDegraded(true);
      },
      (snapshot) => {
        if (current()) acceptSnapshot(snapshot);
      },
    );
    void handle.ready.then((ready) => {
      if (!current()) return;
      subscriptionRetryRef.current = ready.retry;
      setListenerDegraded(ready.listenerState === "degraded");
      acceptSnapshot(ready.initial);
    }).catch(() => {
      if (!current()) return;
      subscriptionRetryRef.current = null;
      setListenerDegraded(true);
    });
    return () => {
      lifecycleRef.current += 1;
      subscriptionRetryAttemptRef.current += 1;
      subscriptionRetryPendingRef.current = false;
      handle.dispose();
    };
  }, [subscriptionRetryVersion]);

  useEffect(() => {
    const timer = setInterval(() => setClock(Date.now()), progressTickMs);
    return () => clearInterval(timer);
  }, [progressTickMs]);

  const displayedPosition = useMemo(() => {
    if (seekDirtyRef.current) return seekDraft;
    const elapsed = confirmed.playbackState === "playing"
      ? Math.max(0, clock - confirmed.updatedAt) / 1_000
      : 0;
    const maximum = confirmed.durationSeconds ?? Number.MAX_SAFE_INTEGER;
    return clamp(confirmed.positionSeconds + elapsed, 0, maximum);
  }, [clock, confirmed, seekDraft]);

  const runControl = async (input: MediaControlInput) => {
    if (pendingRef.current) return;
    pendingRef.current = true;
    setControlPending(true);
    setControlFailed(false);
    try {
      const snapshot = await sendMediaCommand(input);
      seekDirtyRef.current = false;
      volumeDirtyRef.current = false;
      acceptSnapshot(snapshot);
    } catch {
      seekDirtyRef.current = false;
      volumeDirtyRef.current = false;
      setSeekDraft(confirmed.positionSeconds);
      setVolumeDraft(confirmed.volumePercent ?? 0);
      setControlFailed(true);
    } finally {
      pendingRef.current = false;
      setControlPending(false);
    }
  };

  const commitSeek = () => {
    if (!seekDirtyRef.current || !confirmed.canSeek || controlPending) return;
    const value = clamp(seekDraft, 0, confirmed.durationSeconds ?? seekDraft);
    seekDirtyRef.current = false;
    void runControl({ command: "seek", positionSeconds: value });
  };
  const commitVolume = () => {
    if (!volumeDirtyRef.current || !confirmed.canSetVolume || controlPending) return;
    const value = Math.round(clamp(volumeDraft, 0, 100));
    volumeDirtyRef.current = false;
    void runControl({ command: "setVolume", volumePercent: value });
  };
  const commitOnKey = (event: React.KeyboardEvent<HTMLInputElement>, commit: () => void) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    event.preventDefault();
    commit();
  };

  const retrySubscription = async () => {
    if (subscriptionRetryPendingRef.current) return;
    subscriptionRetryPendingRef.current = true;
    setSubscriptionRetryPending(true);
    subscriptionRetryAttemptRef.current += 1;
    const attempt = subscriptionRetryAttemptRef.current;
    const lifecycle = lifecycleRef.current;
    const retry = subscriptionRetryRef.current;
    const current = () => subscriptionRetryAttemptRef.current === attempt
      && lifecycleRef.current === lifecycle;
    if (retry === null) {
      if (current()) {
        subscriptionRetryPendingRef.current = false;
        setSubscriptionRetryPending(false);
        setSubscriptionRetryVersion((version) => version + 1);
      }
      return;
    }
    try {
      await retry();
    } catch {
      if (current()) setListenerDegraded(true);
    } finally {
      if (current()) {
        subscriptionRetryPendingRef.current = false;
        setSubscriptionRetryPending(false);
      }
    }
  };

  const stateKey: TranslationKey = confirmed.playbackState === "playing"
    ? "media.state.playing"
    : confirmed.playbackState === "paused"
      ? "media.state.paused"
      : confirmed.playbackState === "stopped"
        ? "media.state.stopped"
        : "media.state.unavailable";
  const readOnly = confirmed.sessionId !== null
    && (!confirmed.canPlay || !confirmed.canPause || !confirmed.canPrevious || !confirmed.canNext || !confirmed.canSeek);

  return <section className="media-page">
    <header className="media-header">
      <div>
        <h1>{t("media.title")}</h1>
        <p className="media-state">{t(stateKey)}</p>
      </div>
    </header>

    {listenerDegraded && <div className="media-notice" role="status">
      <span>{t("media.state.unavailable")}</span>
      <button type="button" disabled={subscriptionRetryPending} onClick={() => void retrySubscription()}>{t("action.retry")}</button>
    </div>}
    {controlFailed && <p className="media-notice" role="alert">{t("media.error.controlFailed")}</p>}

    <div className="media-scroll">
      <section className="media-session" aria-live="polite">
        {confirmed.sessionId === null
          ? <div className="media-empty">{t("media.state.noSession")}</div>
          : <div className="media-metadata">
            <strong>{confirmed.title}</strong>
            {confirmed.artist && <span>{confirmed.artist}</span>}
          </div>}

        <div className="media-playback-actions">
          <IconButton label={t("media.action.previous")} disabled={controlPending || !confirmed.canPrevious} onClick={() => void runControl({ command: "previous" })}><SkipBack size={17} /></IconButton>
          <IconButton label={t("media.action.play")} disabled={controlPending || !confirmed.canPlay} onClick={() => void runControl({ command: "play" })}><Play size={17} /></IconButton>
          <IconButton label={t("media.action.pause")} disabled={controlPending || !confirmed.canPause} onClick={() => void runControl({ command: "pause" })}><Pause size={17} /></IconButton>
          <IconButton label={t("media.action.next")} disabled={controlPending || !confirmed.canNext} onClick={() => void runControl({ command: "next" })}><SkipForward size={17} /></IconButton>
        </div>

        <label className="media-slider-row">
          <span>{t("media.field.progress")}</span>
          <input
            type="range"
            aria-label={t("media.field.progress")}
            min={0}
            max={confirmed.durationSeconds ?? 0}
            step={1}
            value={displayedPosition}
            disabled={controlPending || !confirmed.canSeek || confirmed.durationSeconds === null}
            onInput={(event) => {
              seekDirtyRef.current = true;
              setSeekDraft(Number(event.currentTarget.value));
            }}
            onPointerUp={commitSeek}
            onBlur={commitSeek}
            onKeyDown={(event) => commitOnKey(event, commitSeek)}
          />
          <output>{formatDuration(displayedPosition)} / {formatDuration(confirmed.durationSeconds)}</output>
        </label>
        {readOnly && <p className="media-readonly">{t("media.state.readOnly")}</p>}
      </section>

      <section className="media-volume-panel">
        <label className="media-slider-row">
          <span>{t("media.field.volume")}</span>
          <input
            type="range"
            aria-label={t("media.field.volume")}
            min={0}
            max={100}
            step={1}
            value={volumeDirtyRef.current ? volumeDraft : (confirmed.volumePercent ?? 0)}
            disabled={controlPending || !confirmed.canSetVolume}
            onInput={(event) => {
              volumeDirtyRef.current = true;
              setVolumeDraft(Number(event.currentTarget.value));
            }}
            onPointerUp={commitVolume}
            onBlur={commitVolume}
            onKeyDown={(event) => commitOnKey(event, commitVolume)}
          />
          <output>{Math.round(volumeDirtyRef.current ? volumeDraft : (confirmed.volumePercent ?? 0))}%</output>
        </label>
        <p>{t("media.hint.systemVolume")}</p>
      </section>
    </div>
  </section>;
}

type IconButtonProps = {
  children: ReactNode;
  disabled: boolean;
  label: string;
  onClick: () => void;
};

function IconButton({ children, disabled, label, onClick }: IconButtonProps) {
  const buttonRef = useRef<HTMLButtonElement>(null);
  const [visible, setVisible] = useState(false);
  const [position, setPosition] = useState({ left: 0, top: 0 });

  useLayoutEffect(() => {
    if (!visible || buttonRef.current === null) return undefined;
    const update = () => {
      const rect = buttonRef.current?.getBoundingClientRect();
      if (rect === undefined) return;
      setPosition({
        left: clamp(rect.left + rect.width / 2, 48, window.innerWidth - 48),
        top: clamp(rect.bottom + 7, 8, window.innerHeight - 28),
      });
    };
    update();
    window.addEventListener("scroll", update, true);
    window.addEventListener("resize", update);
    return () => {
      window.removeEventListener("scroll", update, true);
      window.removeEventListener("resize", update);
    };
  }, [visible]);

  return <>
    <button
      ref={buttonRef}
      type="button"
      className="media-icon-button"
      aria-label={label}
      title={label}
      disabled={disabled}
      onClick={onClick}
      onMouseEnter={() => setVisible(true)}
      onMouseLeave={() => setVisible(false)}
      onFocus={() => setVisible(true)}
      onBlur={() => setVisible(false)}
    >{children}</button>
    {visible && createPortal(<span className="media-tooltip" role="tooltip" style={position}>{label}</span>, document.body)}
  </>;
}
