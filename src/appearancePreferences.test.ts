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
import { ISLAND_BACKGROUND_OPTIONS } from "./backgroundPalette";

function storageWith(entries: Record<string, string | null>): Pick<Storage, "getItem"> {
  return {
    getItem: (key) => entries[key] ?? null,
  };
}

function channelToLinear(channel: number) {
  const normalized = channel / 255;
  return normalized <= 0.04045 ? normalized / 12.92 : ((normalized + 0.055) / 1.055) ** 2.4;
}

function luminance(rgb: readonly number[]) {
  return 0.2126 * channelToLinear(rgb[0]) + 0.7152 * channelToLinear(rgb[1]) + 0.0722 * channelToLinear(rgb[2]);
}

function contrastRatio(foreground: readonly number[], background: readonly number[]) {
  const foregroundLuminance = luminance(foreground);
  const backgroundLuminance = luminance(background);
  return (Math.max(foregroundLuminance, backgroundLuminance) + 0.05) / (Math.min(foregroundLuminance, backgroundLuminance) + 0.05);
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

  test("maps one custom intervention color to failed, waiting, and timeout without affecting offline", () => {
    expect(resolveAgentStatusColors({
      running: "yellow",
      idle: "blue",
      completed: "green",
      attention: "pink",
    })).toMatchObject({
      failed: "#F472B6",
      waiting: "#F472B6",
      timeout: "#F472B6",
      offline: "#888780",
    });
  });

  test("keeps normal text at 4.5:1 through the full white panel-overlay range", () => {
    APPEARANCE_COLOR_OPTIONS.forEach((option) => {
      const readableAlpha = (option as typeof option & { readableAlpha?: number }).readableAlpha;
      expect(readableAlpha, option.value).toBeTypeOf("number");
      expect(readableAlpha, option.value).toBeGreaterThan(0);
      expect(readableAlpha, option.value).toBeLessThanOrEqual(1);

      const foreground = option.textRgb.split(" ").map(Number);
      ISLAND_BACKGROUND_OPTIONS.forEach((backgroundOption) => {
        const solidBackground = backgroundOption.rgb.split(" ").map(Number);
        [0, 0.055, 0.1].forEach((cardLayerAlpha) => {
          const background = solidBackground.map((channel) => channel * (1 - cardLayerAlpha) + 255 * cardLayerAlpha);
          const composited = foreground.map((channel, index) => channel * readableAlpha! + background[index] * (1 - readableAlpha!));
          expect(
            contrastRatio(composited, background),
            `${option.value} on ${backgroundOption.value} at card alpha ${cardLayerAlpha}`,
          ).toBeGreaterThanOrEqual(4.5);
        });
        const hoverBackground = solidBackground.map((channel) => channel * 0.89 + 255 * 0.11);
        expect(
          contrastRatio(foreground, hoverBackground),
          `${option.value} primary text on ${backgroundOption.value} hover layer`,
        ).toBeGreaterThanOrEqual(4.5);
        const notificationOverlay = [55, 138, 221];
        const notificationBackground = solidBackground.map((channel, index) => channel * 0.76 + notificationOverlay[index] * 0.24);
        expect(
          contrastRatio(foreground, notificationBackground),
          `${option.value} primary text on ${backgroundOption.value} notification layer`,
        ).toBeGreaterThanOrEqual(4.5);
      });
    });
  });

});
