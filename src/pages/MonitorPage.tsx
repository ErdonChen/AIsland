import { useCallback, useEffect, useRef, useState } from "react";
import { listMonitorSamples, listProcessMetrics, listProcessWatches } from "../api/commands";
import { parseCommandError } from "../api/commandError";
import type { CommandError, MonitorSnapshot, ProcessMetric, ProcessWatch } from "../api/contracts";
import { beginMonitorMetricsSubscription, type CommandSubscription } from "../api/events";
import MetricTrend, { formatMonitorValue } from "../components/MetricTrend";
import { useI18n } from "../i18n/I18nProvider";

const FIFTEEN_MINUTES = 15 * 60 * 1000;
const MAX_POINTS = 450;

type MetricCardProps = { label: string; value: string; metric?: string };
function MetricCard({ label, value, metric }: MetricCardProps) {
  return <article className="monitor-card" aria-label={label} data-metric={metric}><span>{label}</span><strong>{value}</strong></article>;
}

export default function MonitorPage() {
  const { language, t } = useI18n();
  const [snapshot, setSnapshot] = useState<MonitorSnapshot | null>(null);
  const [samples, setSamples] = useState<MonitorSnapshot[]>([]);
  const [processes, setProcesses] = useState<ProcessMetric[]>([]);
  const [watches, setWatches] = useState<ProcessWatch[]>([]);
  const [subscriptionError, setSubscriptionError] = useState<CommandError | null>(null);
  const [auxiliaryError, setAuxiliaryError] = useState<CommandError | null>(null);
  const [listenerDegraded, setListenerDegraded] = useState(false);
  const [retryAvailable, setRetryAvailable] = useState(false);
  const [retryPending, setRetryPending] = useState(false);
  const lifecycleRef = useRef(0);
  const auxiliaryRequestRef = useRef(0);
  const subscriptionRef = useRef<CommandSubscription<MonitorSnapshot | null> | null>(null);

  const refreshAuxiliary = useCallback(async () => {
    const lifecycle = lifecycleRef.current;
    const request = ++auxiliaryRequestRef.current;
    try {
      const [nextSamples, nextProcesses, nextWatches] = await Promise.all([
        listMonitorSamples({ since: Date.now() - FIFTEEN_MINUTES, limit: MAX_POINTS }),
        listProcessMetrics({ limit: MAX_POINTS }),
        listProcessWatches(),
      ]);
      if (lifecycleRef.current !== lifecycle || auxiliaryRequestRef.current !== request) return;
      setSamples(nextSamples
        .filter((sample) => Number.isFinite(sample.sampledAt))
        .sort((left, right) => left.sampledAt - right.sampledAt)
        .slice(-MAX_POINTS));
      setProcesses(nextProcesses.slice(0, MAX_POINTS));
      setWatches(nextWatches);
      setAuxiliaryError(null);
    } catch (cause) {
      if (lifecycleRef.current === lifecycle && auxiliaryRequestRef.current === request) setAuxiliaryError(parseCommandError(cause));
    }
  }, []);

  useEffect(() => {
    const lifecycle = ++lifecycleRef.current;
    const handle = beginMonitorMetricsSubscription(
      (nextError) => { if (lifecycleRef.current === lifecycle) setSubscriptionError(nextError); },
      (nextSnapshot) => {
        if (lifecycleRef.current !== lifecycle) return;
        setSnapshot(nextSnapshot);
        setSubscriptionError(null);
        void refreshAuxiliary();
      },
    );
    void handle.ready.then((subscription) => {
      if (lifecycleRef.current !== lifecycle) return;
      subscriptionRef.current = subscription;
      setRetryAvailable(true);
      setListenerDegraded(subscription.listenerState === "degraded");
      setSnapshot(subscription.initial);
      if (subscription.initial === null) void refreshAuxiliary();
    }).catch((cause) => {
      if (lifecycleRef.current === lifecycle) setSubscriptionError(parseCommandError(cause));
    });
    return () => {
      if (lifecycleRef.current === lifecycle) lifecycleRef.current += 1;
      auxiliaryRequestRef.current += 1;
      subscriptionRef.current = null;
      setRetryAvailable(false);
      setListenerDegraded(false);
      handle.dispose();
    };
  }, [refreshAuxiliary]);

  const memoryPercent = snapshot && snapshot.memoryTotalBytes > 0 ? snapshot.memoryUsedBytes / snapshot.memoryTotalBytes * 100 : 0;
  const error = subscriptionError ?? auxiliaryError;
  const trend = (select: (sample: MonitorSnapshot) => number) => samples.map((sample) => ({ sampledAt: sample.sampledAt, value: select(sample) }));
  const retry = async () => {
    const subscription = subscriptionRef.current;
    if (!subscription || retryPending) return;
    const lifecycle = lifecycleRef.current;
    setRetryPending(true);
    try {
      await subscription.retry();
      if (lifecycleRef.current === lifecycle) {
        const degraded = subscription.listenerState === "degraded";
        setListenerDegraded(degraded);
        if (!degraded) setSubscriptionError(null);
      }
    } catch (cause) {
      if (lifecycleRef.current === lifecycle) setSubscriptionError(parseCommandError(cause));
    } finally {
      if (lifecycleRef.current === lifecycle) setRetryPending(false);
    }
  };

  return (
    <section className="monitor-page" aria-label={t("monitor.title")}>
      <header className="monitor-page__header"><h1>{t("monitor.title")}</h1>{(listenerDegraded || error) && <div className="monitor-page__error"><p role="alert">{t("monitor.error.load")}</p>{retryAvailable && <button type="button" className="settings-choice" disabled={retryPending} onClick={() => void retry()}>{t("action.retry")}</button>}</div>}</header>
      {snapshot === null ? <p className="monitor-empty">{t("monitor.noSamples")}</p> : (
        <div className="monitor-card-grid">
          <MetricCard label={t("monitor.cpu")} value={formatMonitorValue(language, snapshot.cpuPercent, "%")} />
          <MetricCard label={t("monitor.memory")} value={formatMonitorValue(language, memoryPercent, "%")} />
          <MetricCard label={t("monitor.diskRead")} value={formatMonitorValue(language, snapshot.diskReadBytesPerSecond, "bytesPerSecond")} />
          <MetricCard label={t("monitor.diskWrite")} value={formatMonitorValue(language, snapshot.diskWriteBytesPerSecond, "bytesPerSecond")} />
          <MetricCard label={t("monitor.networkReceive")} value={formatMonitorValue(language, snapshot.networkReceiveBytesPerSecond, "bytesPerSecond")} />
          <MetricCard label={t("monitor.networkSend")} value={formatMonitorValue(language, snapshot.networkSendBytesPerSecond, "bytesPerSecond")} />
          <MetricCard label={t("monitor.gpu")} metric="gpu" value={snapshot.gpuPercent === null ? t("monitor.gpuUnavailable") : formatMonitorValue(language, snapshot.gpuPercent, "%")} />
        </div>
      )}
      <div className="monitor-trends">
        <MetricTrend label={t("monitor.cpu")} unit="%" points={trend((sample) => sample.cpuPercent)} />
        <MetricTrend label={t("monitor.memory")} unit="%" points={trend((sample) => sample.memoryTotalBytes > 0 ? sample.memoryUsedBytes / sample.memoryTotalBytes * 100 : Number.NaN)} />
        <MetricTrend label={t("monitor.diskRead")} unit="bytesPerSecond" points={trend((sample) => sample.diskReadBytesPerSecond)} />
        <MetricTrend label={t("monitor.diskWrite")} unit="bytesPerSecond" points={trend((sample) => sample.diskWriteBytesPerSecond)} />
        <MetricTrend label={t("monitor.networkReceive")} unit="bytesPerSecond" points={trend((sample) => sample.networkReceiveBytesPerSecond)} />
        <MetricTrend label={t("monitor.networkSend")} unit="bytesPerSecond" points={trend((sample) => sample.networkSendBytesPerSecond)} />
        {samples.some((sample) => sample.gpuPercent !== null) && <MetricTrend label={t("monitor.gpu")} unit="%" points={samples.map((sample) => ({ sampledAt: sample.sampledAt, value: sample.gpuPercent ?? Number.NaN }))} />}
      </div>
      <section className="monitor-processes" aria-label={t("monitor.processes")}>
        <h2>{t("monitor.processes")}</h2>
        {watches.length === 0 ? <p>{t("monitor.processWatch.empty")}</p> : watches.flatMap((watch) => {
          const latestByPid = new Map<number, ProcessMetric>();
          for (const process of processes) {
            if (process.processName.toLocaleLowerCase("en-US") !== watch.processName.toLocaleLowerCase("en-US")) continue;
            const current = latestByPid.get(process.pid);
            if (!current || process.sampledAt > current.sampledAt) latestByPid.set(process.pid, process);
          }
          const matches = [...latestByPid.values()].sort((left, right) => left.pid - right.pid);
          if (matches.length === 0) return [<article key={watch.id} className="monitor-process-row"><span className="monitor-process-name" title={watch.processName}>{watch.processName}</span><span>—</span><span>{t("monitor.noSamples")}</span><span>—</span></article>];
          return matches.map((process) => <article key={`${watch.id}:${process.pid}`} className="monitor-process-row">
            <span className="monitor-process-name" title={watch.processName}>{watch.processName}</span>
            <span>PID {process.pid}</span>
            <span>{formatMonitorValue(language, process.cpuPercent, "%")}</span>
            <span>{formatMonitorValue(language, process.memoryBytes, "bytes")}</span>
          </article>);
        })}
      </section>
    </section>
  );
}
