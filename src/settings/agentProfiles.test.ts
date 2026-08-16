import { describe, expect, it } from "vitest";
import {
  AGENT_PROFILE_PRESETS,
  createCustomHookDraft,
  presetProfileId,
  validateCustomHookProfile,
  validateCustomHookTarget,
} from "./agentProfiles";

describe("agent integration profile catalog", () => {
  it("uses the locked stable ids and official display names", () => {
    expect(AGENT_PROFILE_PRESETS.map(({ id, displayName }) => ({ id, displayName }))).toEqual([
      { id: "kimi", displayName: "Kimi Code" },
      { id: "trae", displayName: "TRAE" },
      { id: "qoderwork", displayName: "QoderWork" },
      { id: "cursor", displayName: "Cursor" },
    ]);
  });

  it("keeps each preset environment independent from backend-owned config paths", () => {
    expect(presetProfileId("trae", "windows")).toBe("trae-windows");
    expect(presetProfileId("trae", "wsl")).toBe("trae-wsl");
  });

  it("represents a custom hook as executable plus argv instead of a shell command", () => {
    const draft = createCustomHookDraft("windows");
    expect(draft).toMatchObject({
      id: null,
      kind: "custom",
      displayName: "Custom Hook",
      configTarget: {
        kind: "customHook",
        executable: "",
        argv: [],
        workingDirectory: null,
        timeoutSeconds: 30,
      },
    });
    expect(draft.configTarget).not.toHaveProperty("command");
  });
});

describe("custom hook target validation", () => {
  it("accepts absolute Windows executables with separate safe arguments", () => {
    expect(validateCustomHookTarget("windows", {
      kind: "customHook",
      executable: "C:\\Tools\\agent-hook.exe",
      argv: ["--event", "completed"],
      workingDirectory: "C:\\Tools",
      timeoutSeconds: 30,
    })).toEqual({});
  });

  it("rejects relative executables, control characters, and unsafe timeouts", () => {
    expect(validateCustomHookTarget("windows", {
      kind: "customHook",
      executable: "agent-hook.exe",
      argv: ["--event\ncompleted"],
      workingDirectory: null,
      timeoutSeconds: 0,
    })).toEqual({
      executable: "absolutePathRequired",
      argv: "controlCharactersNotAllowed",
      timeoutSeconds: "timeoutOutOfRange",
    });
  });

  it("enforces visible Custom Hook path, argument-count, and timeout limits", () => {
    expect(validateCustomHookTarget("windows", {
      kind: "customHook",
      executable: `C:\\${"x".repeat(4096)}`,
      argv: Array.from({ length: 33 }, () => "--safe"),
      workingDirectory: null,
      timeoutSeconds: 601,
    })).toEqual({
      executable: "pathTooLong",
      argv: "tooManyArguments",
      timeoutSeconds: "timeoutOutOfRange",
    });
  });
});

describe("custom hook profile validation", () => {
  it("requires a bounded display name and at least one valid event mapping", () => {
    expect(validateCustomHookProfile({ displayName: "   ", eventMapping: [] })).toEqual({
      displayName: "displayNameMustBeTrimmed",
      eventMapping: "eventMappingRequired",
    });
  });

  it("requires exact trimmed event names and rejects only exact duplicate identifiers", () => {
    expect(validateCustomHookProfile({
      displayName: "Build hook",
      eventMapping: [
        { nativeEvent: "Completed", normalizedStatus: "completed" },
        { nativeEvent: "completed", normalizedStatus: "idle" },
      ],
    })).toEqual({ eventMapping: "duplicateNativeEvent" });
    expect(validateCustomHookProfile({
      displayName: "Build hook",
      eventMapping: [{ nativeEvent: " completed ", normalizedStatus: "idle" }],
    })).toEqual({ eventMapping: "nativeEventMustBeTrimmed" });
  });
});
