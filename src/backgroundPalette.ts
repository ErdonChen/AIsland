import type { TranslationKey } from "./i18n/catalog";
import type { IslandBackgroundColor } from "./types";

export const ISLAND_BACKGROUND_OPTIONS: readonly {
  value: IslandBackgroundColor;
  hex: string;
  rgb: string;
  labelKey: TranslationKey;
}[] = [
  { value: "midnight", hex: "#17171C", rgb: "23 23 28", labelKey: "settings.backgroundColor.midnight" },
  { value: "ocean", hex: "#13243A", rgb: "19 36 58", labelKey: "settings.backgroundColor.ocean" },
  { value: "graphite", hex: "#202936", rgb: "32 41 54", labelKey: "settings.backgroundColor.graphite" },
  { value: "pine", hex: "#172A24", rgb: "23 42 36", labelKey: "settings.backgroundColor.pine" },
  { value: "nebula", hex: "#281D35", rgb: "40 29 53", labelKey: "settings.backgroundColor.nebula" },
  { value: "rock", hex: "#30251F", rgb: "48 37 31", labelKey: "settings.backgroundColor.rock" },
] as const;

export function isIslandBackgroundColor(value: unknown): value is IslandBackgroundColor {
  return ISLAND_BACKGROUND_OPTIONS.some((option) => option.value === value);
}

export function islandBackgroundRgb(value: IslandBackgroundColor): string {
  return ISLAND_BACKGROUND_OPTIONS.find((option) => option.value === value)?.rgb ?? ISLAND_BACKGROUND_OPTIONS[0].rgb;
}
