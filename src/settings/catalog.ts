import type { SettingsCategory, SettingsCategoryEntry } from "./types";

export const SETTINGS_CATEGORIES: readonly SettingsCategoryEntry[] = [
  {
    id: "general",
    labelKey: "settings.category.general",
    summaryKey: "settings.summary.general",
    availability: "available",
  },
  {
    id: "display",
    labelKey: "settings.category.display",
    summaryKey: "settings.summary.display",
    availability: "available",
  },
  {
    id: "storage",
    labelKey: "settings.category.storage",
    summaryKey: "settings.summary.storage",
    availability: "coming-soon",
  },
  {
    id: "agents",
    labelKey: "settings.category.agents",
    summaryKey: "settings.summary.agents",
    availability: "available",
  },
  {
    id: "reminders",
    labelKey: "settings.category.reminders",
    summaryKey: "settings.summary.reminders",
    availability: "available",
  },
  {
    id: "modules",
    labelKey: "settings.category.modules",
    summaryKey: "settings.summary.modules",
    availability: "coming-soon",
  },
  {
    id: "diagnostics",
    labelKey: "settings.categories.diagnostics.title",
    summaryKey: "settings.categories.diagnostics.description",
    availability: "available",
  },
  {
    id: "about",
    labelKey: "settings.category.about",
    summaryKey: "settings.summary.about",
    availability: "available",
  },
];

export function settingsCategoryById(category: SettingsCategory) {
  return SETTINGS_CATEGORIES.find((entry) => entry.id === category);
}
