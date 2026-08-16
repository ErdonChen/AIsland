import { useMemo } from "react";
import type { UnixMillis } from "../api/contracts";
import { useI18n } from "../i18n/I18nProvider";

export interface MetricTrendProps {
  label: string;
  unit: "%" | "bytesPerSecond" | "bytes";
  points: Array<{ sampledAt: UnixMillis; value: number }>;
}

const WIDTH = 320;
const HEIGHT = 92;
const PADDING = 8;

function formatBytes(language: "zh-CN" | "en-US", value: number, perSecond: boolean) {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let scaled = value;
  let index = 0;
  while (scaled >= 1024 && index < units.length - 1) {
    scaled /= 1024;
    index += 1;
  }
  const number = new Intl.NumberFormat(language, { maximumFractionDigits: 1, minimumFractionDigits: index === 0 ? 0 : 1 }).format(scaled);
  return `${number} ${units[index]}${perSecond ? "/s" : ""}`;
}

export function formatMonitorValue(language: "zh-CN" | "en-US", value: number, unit: MetricTrendProps["unit"]) {
  if (unit === "%") return `${new Intl.NumberFormat(language, { minimumFractionDigits: 1, maximumFractionDigits: 1 }).format(value)}%`;
  return formatBytes(language, value, unit === "bytesPerSecond");
}

export default function MetricTrend({ label, unit, points }: MetricTrendProps) {
  const { language, t } = useI18n();
  const valid = useMemo(() => points
    .filter((point) => Number.isFinite(point.value) && point.value >= 0 && Number.isFinite(point.sampledAt))
    .sort((left, right) => left.sampledAt - right.sampledAt), [points]);

  if (valid.length === 0) {
    return (
      <figure className="metric-trend metric-trend--empty" aria-label={`${label} — ${t("monitor.trend15m")}`}>
        <figcaption><strong>{label}</strong><span>{t("monitor.trend15m")}</span></figcaption>
        <p>{t("monitor.noSamples")}</p>
      </figure>
    );
  }

  const values = valid.map((point) => point.value);
  const minimum = Math.min(...values);
  const maximum = Math.max(...values);
  const range = maximum - minimum;
  const xFor = (index: number) => valid.length === 1
    ? WIDTH / 2
    : PADDING + (index / (valid.length - 1)) * (WIDTH - PADDING * 2);
  const yFor = (value: number) => range === 0
    ? HEIGHT / 2
    : PADDING + ((maximum - value) / range) * (HEIGHT - PADDING * 2);
  const geometry = valid.map((point, index) => `${xFor(index)},${yFor(point.value)}`).join(" ");
  const minLabel = language === "zh-CN" ? "最低" : "Min";
  const currentLabel = language === "zh-CN" ? "当前" : "Current";
  const maxLabel = language === "zh-CN" ? "最高" : "Max";

  return (
    <figure className="metric-trend" aria-label={`${label} — ${t("monitor.trend15m")}`}>
      <figcaption><strong>{label}</strong><span>{t("monitor.trend15m")}</span></figcaption>
      <svg viewBox={`0 0 ${WIDTH} ${HEIGHT}`} role="img" aria-label={`${label} trend`} preserveAspectRatio="none">
        {valid.length === 1
          ? <circle cx={xFor(0)} cy={yFor(valid[0].value)} r="3" data-trend-point />
          : <polyline points={geometry} vectorEffect="non-scaling-stroke" />}
      </svg>
      <div className="metric-trend__points" aria-hidden="true">
        {valid.map((point) => <span key={`${point.sampledAt}:${point.value}`} data-trend-point />)}
      </div>
      <dl className="metric-trend__summary">
        <div><dt>{minLabel}</dt><dd>{formatMonitorValue(language, minimum, unit)}</dd></div>
        <div><dt>{currentLabel}</dt><dd>{formatMonitorValue(language, valid.at(-1)!.value, unit)}</dd></div>
        <div><dt>{maxLabel}</dt><dd>{formatMonitorValue(language, maximum, unit)}</dd></div>
      </dl>
    </figure>
  );
}
