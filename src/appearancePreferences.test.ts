import { describe, expect, test } from "vitest";
import {
  APPEARANCE_COLOR_OPTIONS,
  DEFAULT_STATUS_COLOR_PREFERENCES,
  DEFAULT_TEXT_COLOR,
  STATUS_COLOR_PREFERENCES_KEY,
  TEXT_COLOR_PREFERENCE_KEY,
  loadStatusColorPreferences,
  loadTextColorPreference,
  resolveAgentStatusColors,
} from "./appearancePreferences";

function storageWith(entries: Record<string, string | null>): Pick<Storage, "getItem"> {
  return {
    getItem: (key) => entries[key] ?? null,
  };
}

describe("appearance preferences", () => {
  test("uses the four established status colors and white text by default", () => {
    expect(loadStatusColorPreferences(storageWith({}))).toEqual(DEFAULT_STATUS_COLOR_PREFERENCES);
    expect(loadTextColorPreference(storageWith({}))).toBe(DEFAULT_TEXT_COLOR);
    expect(resolveAgentStatusColors(DEFAULT_STATUS_COLOR_PREFERENCES)).toMatchObject({
      running: "#EF9F27",
      idle: "#72BCFF",
      completed: "#639922",
      failed: "#E24B4A",
      waiting: "#E24B4A",
      timeout: "#E24B4A",
    });
  });

  test("keeps valid saved roles and repairs only corrupt fields", () => {
    const storage = storageWith({
      [STATUS_COLOR_PREFERENCES_KEY]: JSON.stringify({
        running: "purple",
        idle: "not-a-color",
        completed: "pink",
      }),
    });

    expect(loadStatusColorPreferences(storage)).toEqual({
      running: "purple",
      idle: "blue",
      completed: "pink",
      attention: "red",
    });
  });

  test("falls back safely for invalid JSON and unknown text colors", () => {
    expect(loadStatusColorPreferences(storageWith({ [STATUS_COLOR_PREFERENCES_KEY]: "{" }))).toEqual(
      DEFAULT_STATUS_COLOR_PREFERENCES,
    );
    expect(loadTextColorPreference(storageWith({ [TEXT_COLOR_PREFERENCE_KEY]: "orange" }))).toBe("white");
  });

  test("offers exactly the seven requested named colors", () => {
    expect(APPEARANCE_COLOR_OPTIONS.map(({ value }) => value)).toEqual([
      "red",
      "yellow",
      "blue",
      "green",
      "white",
      "purple",
      "pink",
    ]);
  });
});
