import type { AgentStatus } from "./api/contracts";

export type AppearanceColor = "red" | "yellow" | "blue" | "green" | "white" | "purple" | "pink";
export type StatusColorRole = "running" | "idle" | "completed" | "attention";
export type StatusColorPreferences = Record<StatusColorRole, AppearanceColor>;
export type AgentStatusColorMap = Record<AgentStatus, string>;

export type AppearanceColorOption = {
  value: AppearanceColor;
  signalHex: string;
  textHex: string;
  textRgb: string;
};

export const STATUS_COLOR_PREFERENCES_KEY = "aisland.display.statusColors.v1";
export const TEXT_COLOR_PREFERENCE_KEY = "aisland.display.textColor.v1";

export const APPEARANCE_COLOR_OPTIONS: readonly AppearanceColorOption[] = [
  { value: "red", signalHex: "#E24B4A", textHex: "#FF8A85", textRgb: "255 138 133" },
  { value: "yellow", signalHex: "#EF9F27", textHex: "#FFD166", textRgb: "255 209 102" },
  { value: "blue", signalHex: "#72BCFF", textHex: "#8AC8FF", textRgb: "138 200 255" },
  { value: "green", signalHex: "#639922", textHex: "#8ED06C", textRgb: "142 208 108" },
  { value: "white", signalHex: "#F4F7FB", textHex: "#F4F7FB", textRgb: "244 247 251" },
  { value: "purple", signalHex: "#A78BFA", textHex: "#C4B5FD", textRgb: "196 181 253" },
  { value: "pink", signalHex: "#F472B6", textHex: "#FF9CCB", textRgb: "255 156 203" },
] as const;

export const DEFAULT_STATUS_COLOR_PREFERENCES: StatusColorPreferences = {
  running: "yellow",
  idle: "blue",
  completed: "green",
  attention: "red",
};

export const DEFAULT_TEXT_COLOR: AppearanceColor = "white";

const COLOR_BY_NAME = new Map(APPEARANCE_COLOR_OPTIONS.map((option) => [option.value, option]));

export function isAppearanceColor(value: unknown): value is AppearanceColor {
  return typeof value === "string" && COLOR_BY_NAME.has(value as AppearanceColor);
}

export function appearanceColorOption(color: AppearanceColor): AppearanceColorOption {
  return COLOR_BY_NAME.get(color) ?? COLOR_BY_NAME.get(DEFAULT_TEXT_COLOR)!;
}

export function resolveAgentStatusColors(preferences: StatusColorPreferences): AgentStatusColorMap {
  return {
    running: appearanceColorOption(preferences.running).signalHex,
    idle: appearanceColorOption(preferences.idle).signalHex,
    completed: appearanceColorOption(preferences.completed).signalHex,
    failed: appearanceColorOption(preferences.attention).signalHex,
    waiting: appearanceColorOption(preferences.attention).signalHex,
    timeout: appearanceColorOption(preferences.attention).signalHex,
    offline: "#888780",
  };
}

export function loadStatusColorPreferences(storage: Pick<Storage, "getItem"> = localStorage): StatusColorPreferences {
  try {
    const raw = storage.getItem(STATUS_COLOR_PREFERENCES_KEY);
    if (raw === null) return { ...DEFAULT_STATUS_COLOR_PREFERENCES };
    const parsed = JSON.parse(raw) as Partial<Record<StatusColorRole, unknown>> | null;
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
      return { ...DEFAULT_STATUS_COLOR_PREFERENCES };
    }
    return {
      running: isAppearanceColor(parsed.running) ? parsed.running : DEFAULT_STATUS_COLOR_PREFERENCES.running,
      idle: isAppearanceColor(parsed.idle) ? parsed.idle : DEFAULT_STATUS_COLOR_PREFERENCES.idle,
      completed: isAppearanceColor(parsed.completed) ? parsed.completed : DEFAULT_STATUS_COLOR_PREFERENCES.completed,
      attention: isAppearanceColor(parsed.attention) ? parsed.attention : DEFAULT_STATUS_COLOR_PREFERENCES.attention,
    };
  } catch {
    return { ...DEFAULT_STATUS_COLOR_PREFERENCES };
  }
}

export function loadTextColorPreference(storage: Pick<Storage, "getItem"> = localStorage): AppearanceColor {
  try {
    const stored = storage.getItem(TEXT_COLOR_PREFERENCE_KEY);
    return isAppearanceColor(stored) ? stored : DEFAULT_TEXT_COLOR;
  } catch {
    return DEFAULT_TEXT_COLOR;
  }
}
