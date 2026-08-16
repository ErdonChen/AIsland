import { describe, expect, it } from "vitest";
import tauriConfig from "../src-tauri/tauri.conf.json";

describe("fixed desktop webviews", () => {
  it("keeps text entry from changing the page zoom", () => {
    const windows = tauriConfig.app.windows as Array<{ label: string; zoomHotkeysEnabled?: boolean }>;

    expect(windows.find((window) => window.label === "main")?.zoomHotkeysEnabled).toBe(false);
    expect(windows.find((window) => window.label === "reminder-alert")?.zoomHotkeysEnabled).toBe(false);
  });
});
