import { describe, expect, test } from "vitest";
import {
  clampWindowWidth,
  clampWindowScale,
  horizontalDragWidth,
  scaleFromSliderPosition,
  shouldCollapseFromExpandedWidth,
  sliderPositionFromScale,
} from "./windowGeometry";

describe("window scale geometry", () => {
  test.each([
    [0, 0.8],
    [25, 0.9],
    [50, 1],
    [75, 1.6],
    [100, 2.2],
  ])("maps slider position %s to scale %s", (position, scale) => {
    expect(scaleFromSliderPosition(position)).toBeCloseTo(scale);
  });

  test.each([0.8, 0.9, 1, 1.6, 2.2])(
    "round-trips scale %s through the slider",
    (scale) => {
      expect(scaleFromSliderPosition(sliderPositionFromScale(scale))).toBeCloseTo(scale);
    },
  );

  test("clamps non-finite and out-of-range values", () => {
    expect(clampWindowScale(0.5)).toBe(0.8);
    expect(clampWindowScale(3)).toBe(2.2);
    expect(clampWindowScale(Number.NaN)).toBe(1);
  });
});

describe("horizontal window resizing", () => {
  test("keeps independent collapsed and expanded limits", () => {
    expect(clampWindowWidth("collapsed", 100)).toBe(248);
    expect(clampWindowWidth("collapsed", 900)).toBe(720);
    expect(clampWindowWidth("expanded", 100)).toBe(420);
    expect(clampWindowWidth("expanded", 1_200)).toBe(960);
  });

  test("converts physical pointer movement back through application scale", () => {
    expect(horizontalDragWidth(560, 120, 2, "right")).toBe(620);
    expect(horizontalDragWidth(560, -120, 2, "left")).toBe(620);
  });

  test("switches to a capsule only when released below the expanded minimum", () => {
    expect(shouldCollapseFromExpandedWidth(419.9)).toBe(true);
    expect(shouldCollapseFromExpandedWidth(420)).toBe(false);
  });
});
