import { expect, test } from "vitest";
import { enUS, zhCN } from "../i18n/catalog";
import { SETTINGS_CATEGORIES, settingsCategoryById } from "./catalog";

test("lists settings categories in the approved information architecture order", () => {
  expect(SETTINGS_CATEGORIES.map((category) => category.id)).toEqual([
    "general",
    "display",
    "storage",
    "agents",
    "reminders",
    "modules",
    "diagnostics",
    "about",
  ]);
});

test("uses unique category ids with translated labels and summaries", () => {
  expect(new Set(SETTINGS_CATEGORIES.map((category) => category.id)).size).toBe(
    SETTINGS_CATEGORIES.length,
  );

  for (const category of SETTINGS_CATEGORIES) {
    expect(zhCN[category.labelKey]).toEqual(expect.any(String));
    expect(enUS[category.labelKey]).toEqual(expect.any(String));
    expect(zhCN[category.summaryKey]).toEqual(expect.any(String));
    expect(enUS[category.summaryKey]).toEqual(expect.any(String));
  }
});

test("looks up every category through the central catalog", () => {
  for (const category of SETTINGS_CATEGORIES) {
    expect(settingsCategoryById(category.id)).toBe(category);
  }
});
