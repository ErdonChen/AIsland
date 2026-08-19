export const MIN_WINDOW_SCALE = 0.8;
export const DEFAULT_WINDOW_SCALE = 1;
export const MAX_WINDOW_SCALE = 2.2;
export const MIN_COLLAPSED_WIDTH = 248;
export const MAX_COLLAPSED_WIDTH = 720;
export const DEFAULT_COLLAPSED_WIDTH = 248;
export const MIN_EXPANDED_WIDTH = 420;
export const MAX_EXPANDED_WIDTH = 960;
export const DEFAULT_EXPANDED_WIDTH = 560;

const SCALE_SLIDER_MIDPOINT = 50;

function clampSliderPosition(value: number) {
  return Number.isFinite(value) ? Math.min(100, Math.max(0, value)) : SCALE_SLIDER_MIDPOINT;
}

export function clampWindowScale(value: number) {
  return Number.isFinite(value)
    ? Math.min(MAX_WINDOW_SCALE, Math.max(MIN_WINDOW_SCALE, value))
    : DEFAULT_WINDOW_SCALE;
}

export function scaleFromSliderPosition(position: number) {
  const clamped = clampSliderPosition(position);
  const scale = clamped <= SCALE_SLIDER_MIDPOINT
    ? MIN_WINDOW_SCALE + clamped * 0.004
    : DEFAULT_WINDOW_SCALE + (clamped - SCALE_SLIDER_MIDPOINT) * 0.024;
  return Math.round(scale * 1_000) / 1_000;
}

export function sliderPositionFromScale(scale: number) {
  const clamped = clampWindowScale(scale);
  return clamped <= DEFAULT_WINDOW_SCALE
    ? (clamped - MIN_WINDOW_SCALE) / 0.004
    : SCALE_SLIDER_MIDPOINT + (clamped - DEFAULT_WINDOW_SCALE) / 0.024;
}

export function windowScalePercent(scale: number) {
  return Math.round(clampWindowScale(scale) * 100);
}

export function clampWindowWidth(mode: "collapsed" | "expanded", width: number) {
  const [minimum, maximum, fallback] = mode === "collapsed"
    ? [MIN_COLLAPSED_WIDTH, MAX_COLLAPSED_WIDTH, DEFAULT_COLLAPSED_WIDTH]
    : [MIN_EXPANDED_WIDTH, MAX_EXPANDED_WIDTH, DEFAULT_EXPANDED_WIDTH];
  return Number.isFinite(width) ? Math.min(maximum, Math.max(minimum, width)) : fallback;
}

export function horizontalDragWidth(
  startWidth: number,
  physicalDeltaX: number,
  scale: number,
  handle: "left" | "right",
) {
  const safeScale = clampWindowScale(scale);
  const logicalDelta = physicalDeltaX / safeScale;
  return startWidth + (handle === "right" ? logicalDelta : -logicalDelta);
}

export function shouldCollapseFromExpandedWidth(width: number) {
  return Number.isFinite(width) && width < MIN_EXPANDED_WIDTH;
}
