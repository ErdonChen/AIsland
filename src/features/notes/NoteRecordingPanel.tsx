import { Mic, Play, Square, Trash2 } from "lucide-react";
import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef, useState } from "react";

import {
  abortNoteRecording,
  appendNoteRecordingChunk,
  deleteNoteRecording,
  finishNoteRecording,
  listNoteRecordings,
  readNoteRecording,
  recoverNoteRecordings,
  startNoteRecording,
} from "../../api/commands";
import type { LocalDate, NoteRecording } from "../../api/contracts";
import { useI18n } from "../../i18n/I18nProvider";

export interface NoteRecordingPanelHandle {
  stop(): Promise<boolean>;
}

interface NoteRecordingPanelProps {
  noteDate: LocalDate;
  active: boolean;
  onContentChanged?(): void;
}

type RecordingSession = {
  completion: Promise<boolean>;
  draft: NoteRecording;
  recorder: MediaRecorder;
  failed: boolean;
  startedAt: number;
  stream: MediaStream;
  writes: Promise<void>;
};

const MIME_CANDIDATES = [
  { extension: "webm", mimeType: "audio/webm;codecs=opus" },
  { extension: "webm", mimeType: "audio/webm" },
  { extension: "ogg", mimeType: "audio/ogg;codecs=opus" },
  { extension: "ogg", mimeType: "audio/ogg" },
  { extension: "mp4", mimeType: "audio/mp4" },
] as const;

function selectedMimeType(): (typeof MIME_CANDIDATES)[number] | null {
  if (typeof MediaRecorder === "undefined") return null;
  return MIME_CANDIDATES.find(({ mimeType }) => MediaRecorder.isTypeSupported(mimeType)) ?? null;
}

function extensionForMimeType(mimeType: string): "webm" | "ogg" | "mp4" | null {
  const normalized = mimeType.toLowerCase().split(";", 1)[0].trim();
  if (normalized === "audio/webm") return "webm";
  if (normalized === "audio/ogg") return "ogg";
  if (normalized === "audio/mp4") return "mp4";
  return null;
}

function formatDuration(durationMs: number): string {
  const seconds = Math.max(0, Math.floor(durationMs / 1_000));
  return `${String(Math.floor(seconds / 60)).padStart(2, "0")}:${String(seconds % 60).padStart(2, "0")}`;
}

const NoteRecordingPanel = forwardRef<NoteRecordingPanelHandle, NoteRecordingPanelProps>(
  function NoteRecordingPanel({ noteDate, active, onContentChanged }, forwardedRef) {
    const { language, t } = useI18n();
    const mountedRef = useRef(true);
    const sessionRef = useRef<RecordingSession | null>(null);
    const recoveryRef = useRef<Promise<void> | null>(null);
    const loadTokenRef = useRef(0);
    const [recordings, setRecordings] = useState<NoteRecording[]>([]);
    const [audioSources, setAudioSources] = useState<Record<string, string>>({});
    const [recording, setRecording] = useState(false);
    const [elapsedMs, setElapsedMs] = useState(0);
    const [pending, setPending] = useState(false);
    const [error, setError] = useState<"permission" | "encoding" | "storage" | "unsupported" | null>(null);

    const loadRecordings = useCallback(async (date: LocalDate) => {
      const token = loadTokenRef.current + 1;
      loadTokenRef.current = token;
      try {
        const result = await listNoteRecordings({ noteDate: date });
        if (!mountedRef.current || loadTokenRef.current !== token || date !== noteDate) return;
        setRecordings(Array.isArray(result) ? result : []);
        setError(null);
      } catch {
        if (mountedRef.current && loadTokenRef.current === token) setError("storage");
      }
    }, [noteDate]);

    const finishSession = useCallback(async (session: RecordingSession): Promise<boolean> => {
      try {
        await session.writes;
        if (session.failed) {
          await abortNoteRecording({ id: session.draft.id, expectedRevision: session.draft.revision });
          if (mountedRef.current) setError("encoding");
          return false;
        }
        await finishNoteRecording({
          id: session.draft.id,
          durationMs: Math.max(0, Date.now() - session.startedAt),
          expectedRevision: session.draft.revision,
        });
        await loadRecordings(session.draft.noteDate);
        onContentChanged?.();
        if (mountedRef.current) setError(null);
        return true;
      } catch {
        try {
          await abortNoteRecording({ id: session.draft.id, expectedRevision: session.draft.revision });
        } catch {
          // Startup recovery owns any draft that cannot be removed immediately.
        }
        if (mountedRef.current) setError(session.failed ? "encoding" : "storage");
        return false;
      } finally {
        session.stream.getTracks().forEach((track) => track.stop());
        if (sessionRef.current === session) sessionRef.current = null;
        if (mountedRef.current) { setRecording(false); setPending(false); }
      }
    }, [loadRecordings, onContentChanged]);

    const stop = useCallback(async () => {
      const session = sessionRef.current;
      if (!session) return true;
      if (session.recorder.state === "recording") {
        setPending(true);
        session.recorder.requestData();
        session.recorder.stop();
      }
      return session.completion;
    }, []);
    useImperativeHandle(forwardedRef, () => ({ stop }), [stop]);

    const start = async () => {
      if (pending || sessionRef.current) return;
      const media = selectedMimeType();
      if (typeof MediaRecorder === "undefined" || !navigator.mediaDevices?.getUserMedia) { setError("unsupported"); return; }
      setPending(true);
      setError(null);
      let stream: MediaStream | null = null;
      let draft: NoteRecording | null = null;
      try {
        stream = await navigator.mediaDevices.getUserMedia({ audio: true });
        const recorder = media
          ? new MediaRecorder(stream, { mimeType: media.mimeType })
          : new MediaRecorder(stream);
        const mimeType = recorder.mimeType || media?.mimeType || "";
        const fileExtension = extensionForMimeType(mimeType);
        if (!fileExtension) throw new Error("unsupported-recording-format");
        const startedAt = Date.now();
        const startedDraft = await startNoteRecording({
          noteDate,
          mimeType,
          fileExtension,
          startedAt,
        });
        draft = startedDraft;
        let resolveCompletion!: (saved: boolean) => void;
        const session: RecordingSession = {
          completion: new Promise((resolve) => { resolveCompletion = resolve; }),
          draft: startedDraft,
          failed: false,
          recorder,
          startedAt,
          stream,
          writes: Promise.resolve(),
        };
        recorder.ondataavailable = (event) => {
          if (event.data.size === 0) return;
          session.writes = session.writes.then(async () => {
            const bytes = Array.from(new Uint8Array(await event.data.arrayBuffer()));
            await appendNoteRecordingChunk({ id: startedDraft.id, chunk: bytes });
          });
        };
        recorder.onerror = () => {
          session.failed = true;
          setError("encoding");
          if (recorder.state === "recording") recorder.stop();
        };
        recorder.onstop = () => { void finishSession(session).then(resolveCompletion); };
        sessionRef.current = session;
        recorder.start(1_000);
        setRecording(true);
        setPending(false);
      } catch (cause) {
        stream?.getTracks().forEach((track) => track.stop());
        if (draft) void abortNoteRecording({ id: draft.id, expectedRevision: draft.revision });
        setPending(false);
        setRecording(false);
        setError(cause instanceof DOMException && ["NotAllowedError", "SecurityError"].includes(cause.name)
          ? "permission"
          : cause instanceof Error && cause.message === "unsupported-recording-format"
            ? "unsupported"
            : "storage");
      }
    };

    const loadAudio = async (recordingItem: NoteRecording) => {
      try {
        const payload = await readNoteRecording({ id: recordingItem.id });
        if (!mountedRef.current || payload.id !== recordingItem.id) return;
        setAudioSources((current) => ({
          ...current,
          [payload.id]: `data:${payload.mimeType};base64,${payload.base64}`,
        }));
        setError(null);
      } catch { if (mountedRef.current) setError("storage"); }
    };

    const removeRecording = async (recordingItem: NoteRecording) => {
      if (!window.confirm(t("notes.recording.deleteConfirm"))) return;
      setPending(true);
      try {
        await deleteNoteRecording({ id: recordingItem.id, expectedRevision: recordingItem.revision });
        setAudioSources((current) => {
          const next = { ...current };
          delete next[recordingItem.id];
          return next;
        });
        await loadRecordings(noteDate);
        onContentChanged?.();
        setError(null);
      } catch { setError("storage"); }
      finally { if (mountedRef.current) setPending(false); }
    };

    useEffect(() => {
      setAudioSources({});
      recoveryRef.current ??= recoverNoteRecordings().then(() => undefined);
      void recoveryRef.current
        .then(() => loadRecordings(noteDate))
        .catch(() => { if (mountedRef.current) setError("storage"); });
    }, [loadRecordings, noteDate]);

    useEffect(() => {
      if (!recording) { setElapsedMs(0); return; }
      const update = () => setElapsedMs(Math.max(0, Date.now() - (sessionRef.current?.startedAt ?? Date.now())));
      update();
      const timer = window.setInterval(update, 250);
      return () => window.clearInterval(timer);
    }, [recording]);

    useEffect(() => {
      if (!active) void stop();
    }, [active, stop]);

    useEffect(() => {
      const stopBeforeExit = (event: BeforeUnloadEvent) => {
        if (!sessionRef.current) return;
        event.preventDefault();
        event.returnValue = "";
        void stop();
      };
      window.addEventListener("beforeunload", stopBeforeExit);
      return () => window.removeEventListener("beforeunload", stopBeforeExit);
    }, [stop]);

    useEffect(() => () => {
      mountedRef.current = false;
      loadTokenRef.current += 1;
      void stop();
    }, [stop]);

    return <section className="notes-recordings" aria-label={t("notes.recording.title")}>
      <div className="notes-recordings__header">
        <strong>{t("notes.recording.title")}</strong>
        <button
          type="button"
          className={recording ? "notes-recordings__stop" : undefined}
          disabled={pending || !active}
          onClick={() => void (recording ? stop() : start())}
        >
          {recording ? <Square size={11} fill="currentColor" /> : <Mic size={12} />}
          {t(recording ? "notes.recording.stop" : "notes.recording.start")}
        </button>
        {recording && <span className="notes-recordings__live" role="status">{t("notes.recording.active")} · {formatDuration(elapsedMs)}</span>}
      </div>
      {error && <p className="notes-recordings__error" role="alert">{t(`notes.recording.error.${error}`)}</p>}
      {recordings.length === 0 && !recording
        ? <p className="notes-recordings__empty">{t("notes.recording.empty")}</p>
        : <ol className="notes-recordings__list">{recordings.map((item, index) => <li key={item.id}>
            <span>{t("notes.recording.item").replace("{count}", String(index + 1))}</span>
            <time>{new Intl.DateTimeFormat(language, { dateStyle: "short", timeStyle: "short" }).format(new Date(item.startedAt))}</time>
            <time>{formatDuration(item.durationMs)}</time>
            {audioSources[item.id]
              ? <audio controls preload="metadata" aria-label={t("notes.recording.item").replace("{count}", String(index + 1))} src={audioSources[item.id]} />
              : <button type="button" disabled={pending} aria-label={t("notes.recording.load")} onClick={() => void loadAudio(item)}><Play size={11} fill="currentColor" /></button>}
            <button type="button" disabled={pending} aria-label={t("notes.recording.delete")} onClick={() => void removeRecording(item)}><Trash2 size={11} /></button>
          </li>)}</ol>}
    </section>;
  },
);

export default NoteRecordingPanel;
