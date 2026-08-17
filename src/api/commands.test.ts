import { beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock, ChannelMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  ChannelMock: class<T> {
    onmessage: (message: T) => void = () => undefined;
  },
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock, Channel: ChannelMock }));

const commandsModulePath = "./commands";

describe("foundation command bridge", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("invokes each no-input foundation command with its exact camelCase name", async () => {
    invokeMock.mockResolvedValue({});
    const { checkStorageIntegrity, getAppSnapshot, listServiceHealth } = await import(/* @vite-ignore */ commandsModulePath);

    await getAppSnapshot();
    await listServiceHealth();
    await checkStorageIntegrity();

    expect(invokeMock.mock.calls).toEqual([
      ["getAppSnapshot"],
      ["listServiceHealth"],
      ["checkStorageIntegrity"],
    ]);
  });

  it("invokes diagnostics with its exact camelCase payload", async () => {
    invokeMock.mockResolvedValue([]);
    const { getDiagnostics } = await import(/* @vite-ignore */ commandsModulePath);

    await getDiagnostics({ limit: 25 });

    expect(invokeMock).toHaveBeenCalledWith("getDiagnostics", { limit: 25 });
  });

  it("bridges versioned startup settings and the signed update channel", async () => {
    invokeMock.mockResolvedValue({});
    const commands = await import(/* @vite-ignore */ commandsModulePath);
    const onEvent = vi.fn();

    await commands.getGeneralSettings();
    await commands.saveGeneralSettings({ launchAtStartup: true, expectedRevision: 4 });
    await commands.checkForUpdate();
    await commands.installUpdate(onEvent);

    expect(invokeMock.mock.calls.slice(0, 3)).toEqual([
      ["getGeneralSettings"],
      ["saveGeneralSettings", { launchAtStartup: true, expectedRevision: 4 }],
      ["checkForUpdate"],
    ]);
    const [installCommand, installPayload] = invokeMock.mock.calls[3] as [string, { onEvent: InstanceType<typeof ChannelMock> }];
    expect(installCommand).toBe("installUpdate");
    expect(installPayload.onEvent).toBeInstanceOf(ChannelMock);
    installPayload.onEvent.onmessage({ event: "finished", downloaded: 10, total: 10 });
    expect(onEvent).toHaveBeenCalledWith({ event: "finished", downloaded: 10, total: 10 });
  });

  it("loads the newest authoritative Windows notification after an invalidation event", async () => {
    invokeMock.mockResolvedValue([]);
    const { listNotificationHistory } = await import(/* @vite-ignore */ commandsModulePath);

    await listNotificationHistory({ origin: "windows", sourceApp: null, unreadOnly: false, limit: 1 });

    expect(invokeMock).toHaveBeenCalledWith("listNotificationHistory", {
      origin: "windows",
      sourceApp: null,
      unreadOnly: false,
      limit: 1,
    });
  });

  it("rethrows command rejection as the parsed typed envelope", async () => {
    invokeMock.mockRejectedValueOnce("database closed");
    const { getAppSnapshot } = await import(/* @vite-ignore */ commandsModulePath);

    await expect(getAppSnapshot()).rejects.toEqual({
      code: "ioFailure",
      messageKey: "errors.ioFailure",
      details: {},
      retryable: false,
    });
  });
});

describe("agent and reminder command bridge", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("forwards all fourteen agent and reminder commands with exact payloads", async () => {
    invokeMock.mockResolvedValue({});
    const commands = await import(/* @vite-ignore */ commandsModulePath);

    const mergeIdentity = {
      kind: "agent" as const,
      ruleId: "rule-1",
      agentId: "codex" as const,
      environment: "windows" as const,
      taskId: "task-1",
      triggerStatus: "completed" as const,
    };
    const action = {
      mergeIdentity,
      expectedMemberDeliveryIds: ["delivery-2", "delivery-1"],
      members: [
        { id: "delivery-2", expectedState: "dispatched" as const },
        { id: "delivery-1", expectedState: "dispatched" as const },
      ],
    };
    await commands.getAgentsSnapshot();
    await commands.installAgentIntegration({ agentId: "codex", environment: "windows" });
    await commands.repairAgentIntegration({ agentId: "hermes", environment: "wsl" });
    await commands.uninstallAgentIntegration({ agentId: "claude", environment: "wsl", confirmOwnedRemoval: true });
    await commands.listReminderRules();
    await commands.saveReminderRule({ id: null, agentIds: ["codex"], triggerStatuses: ["completed"], enabled: true, delaySeconds: 0, sound: { kind: "builtin", soundId: "systemNotification" }, toastEnabled: true, windowEnabled: true, expectedRevision: null });
    await commands.deleteReminderRule({ id: "rule-1", expectedRevision: 3 });
    await commands.replayReminderDeliveries({ consumerId: "main-alerts", afterDispatchSeq: 4, limit: 200 });
    await commands.commitReminderReplayCursor({ consumerId: "main-alerts", lastDispatchSeq: 42 });
    await commands.acknowledgeReminder(action);
    await commands.completeReminder(action);
    await commands.snoozeReminder({ ...action, snoozedUntil: 9_000 });
    await commands.reloadReminderAlertGroup({ deliveryId: "delivery-1" });
    await commands.getPendingReminderNavigation();
    await commands.acknowledgeReminderNavigation({ sequence: 8 });

    expect(invokeMock.mock.calls).toEqual([
      ["getAgentsSnapshot"],
      ["installAgentIntegration", { agentId: "codex", environment: "windows" }],
      ["repairAgentIntegration", { agentId: "hermes", environment: "wsl" }],
      ["uninstallAgentIntegration", { agentId: "claude", environment: "wsl", confirmOwnedRemoval: true }],
      ["listReminderRules"],
      ["saveReminderRule", { id: null, agentIds: ["codex"], triggerStatuses: ["completed"], enabled: true, delaySeconds: 0, sound: { kind: "builtin", soundId: "systemNotification" }, toastEnabled: true, windowEnabled: true, expectedRevision: null }],
      ["deleteReminderRule", { id: "rule-1", expectedRevision: 3 }],
      ["replayReminderDeliveries", { consumerId: "main-alerts", afterDispatchSeq: 4, limit: 200 }],
      ["commitReminderReplayCursor", { consumerId: "main-alerts", lastDispatchSeq: 42 }],
      ["acknowledgeReminder", action],
      ["completeReminder", action],
      ["snoozeReminder", { ...action, snoozedUntil: 9_000 }],
      ["reloadReminderAlertGroup", { deliveryId: "delivery-1" }],
      ["getPendingReminderNavigation"],
      ["acknowledgeReminderNavigation", { sequence: 8 }],
    ]);
  });
});

describe("agent profile command bridge", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("keeps Profile actions flat, revision-checked, and explicitly confirmed", async () => {
    invokeMock.mockResolvedValue({});
    const commands = await import(/* @vite-ignore */ commandsModulePath);
    const customProfile = {
      id: null,
      kind: "custom" as const,
      displayName: "Build hook",
      environment: "windows" as const,
      configTarget: {
        kind: "customHook" as const,
        executable: "C:\\Tools\\agent-hook.exe",
        argv: ["--event", "completed"],
        workingDirectory: "C:\\Tools",
        timeoutSeconds: 30,
      },
      eventMapping: [{ nativeEvent: "completed", normalizedStatus: "completed" as const }],
      enabled: true,
      expectedRevision: null,
    };

    await commands.listAgentIntegrationProfiles();
    await commands.discoverAgentIntegrationCandidates();
    await commands.saveAgentIntegrationProfile(customProfile);
    await commands.installAgentIntegrationProfile({ id: "profile-1", expectedRevision: 2, confirmInstallation: true });
    await commands.repairAgentIntegrationProfile({ id: "profile-1", expectedRevision: 3, confirmRepair: true });
    await commands.uninstallAgentIntegrationProfile({ id: "profile-1", expectedRevision: 4, confirmOwnedRemoval: true });
    await commands.deleteAgentIntegrationProfile({ id: "profile-1", expectedRevision: 5, confirmDeletion: true });

    expect(invokeMock.mock.calls).toEqual([
      ["listAgentIntegrationProfiles"],
      ["discoverAgentIntegrationCandidates"],
      ["saveAgentIntegrationProfile", customProfile],
      ["installAgentIntegrationProfile", { id: "profile-1", expectedRevision: 2, confirmInstallation: true }],
      ["repairAgentIntegrationProfile", { id: "profile-1", expectedRevision: 3, confirmRepair: true }],
      ["uninstallAgentIntegrationProfile", { id: "profile-1", expectedRevision: 4, confirmOwnedRemoval: true }],
      ["deleteAgentIntegrationProfile", { id: "profile-1", expectedRevision: 5, confirmDeletion: true }],
    ]);
  });

  it("loads the authoritative dynamic Profile state snapshot without reusing legacy Agent ids", async () => {
    invokeMock.mockResolvedValue({ profiles: [], generatedAt: 1 });
    const commands = await import(/* @vite-ignore */ commandsModulePath);

    await commands.getAgentProfilesSnapshot();

    expect(invokeMock).toHaveBeenCalledWith("getAgentProfilesSnapshot");
  });
});

describe("todo command bridge", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("uses the locked to-do command names and flat camelCase payloads", async () => {
    invokeMock.mockResolvedValue({ id: "todo-1", revision: 1 });
    const { createTodo, completeTodo } = await import(/* @vite-ignore */ commandsModulePath);
    await createTodo({ title: "Ship V1", description: "", dueAt: 1786204800000, priority: "high" });
    expect(invokeMock).toHaveBeenLastCalledWith("createTodo", {
      title: "Ship V1",
      description: "",
      dueAt: 1786204800000,
      priority: "high",
    });
    await completeTodo({ id: "todo-1", completed: true, expectedRevision: 1 });
    expect(invokeMock).toHaveBeenLastCalledWith("completeTodo", {
      id: "todo-1",
      completed: true,
      expectedRevision: 1,
    });
  });

  it("forwards all five CRUD wrappers without nesting their payloads", async () => {
    invokeMock.mockResolvedValue({});
    const commands = await import(/* @vite-ignore */ commandsModulePath);
    const create = { title: "Ship", description: "now", dueAt: null, priority: "normal" as const };
    const update = { ...create, id: "todo-1", expectedRevision: 1 };
    await commands.listTodos({ status: "open", limit: 50 });
    await commands.createTodo(create);
    await commands.updateTodo(update);
    await commands.completeTodo({ id: "todo-1", completed: false, expectedRevision: 2 });
    await commands.deleteTodo({ id: "todo-1", expectedRevision: 3 });
    expect(invokeMock.mock.calls).toEqual([
      ["listTodos", { status: "open", limit: 50 }],
      ["createTodo", create],
      ["updateTodo", update],
      ["completeTodo", { id: "todo-1", completed: false, expectedRevision: 2 }],
      ["deleteTodo", { id: "todo-1", expectedRevision: 3 }],
    ]);
  });

  it("forwards the three todo reminder wrappers with exact flat payloads", async () => {
    invokeMock.mockResolvedValue({});
    const commands = await import(/* @vite-ignore */ commandsModulePath);
    const save = { id: null, todoId: "todo-1", remindAt: 1_000, enabled: true, expectedRevision: null };
    await commands.saveTodoReminder(save);
    await commands.listTodoReminders({ todoId: "todo-1" });
    await commands.listTodoReminders({ todoId: null });
    await commands.deleteTodoReminder({ id: "reminder-1", expectedRevision: 3 });
    expect(invokeMock.mock.calls).toEqual([
      ["saveTodoReminder", save],
      ["listTodoReminders", { todoId: "todo-1" }],
      ["listTodoReminders", { todoId: null }],
      ["deleteTodoReminder", { id: "reminder-1", expectedRevision: 3 }],
    ]);
  });
});

describe("note command bridge", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("invokes getDailyNote with its exact flat camelCase payload", async () => {
    invokeMock.mockResolvedValue(null);
    const commands = await import(/* @vite-ignore */ commandsModulePath);

    await commands.getDailyNote({ noteDate: "2026-08-08" });

    expect(invokeMock).toHaveBeenCalledWith("getDailyNote", { noteDate: "2026-08-08" });
  });

  it("forwards exactly six note CRUD and search wrappers without nesting", async () => {
    invokeMock.mockResolvedValue({});
    const commands = await import(/* @vite-ignore */ commandsModulePath);
    const create = { noteDate: "2026-08-08", bodyMarkdown: "one" };
    const update = { id: "note-1", noteDate: "2026-08-09", bodyMarkdown: "two", expectedRevision: 1 };

    await commands.listNotes({ query: "literal%_", limit: 50 });
    await commands.getNote({ id: "note-1" });
    await commands.getDailyNote({ noteDate: "2026-08-08" });
    await commands.createNote(create);
    await commands.updateNote(update);
    await commands.deleteNote({ id: "note-1", expectedRevision: 2 });

    expect(invokeMock.mock.calls).toEqual([
      ["listNotes", { query: "literal%_", limit: 50 }],
      ["getNote", { id: "note-1" }],
      ["getDailyNote", { noteDate: "2026-08-08" }],
      ["createNote", create],
      ["updateNote", update],
      ["deleteNote", { id: "note-1", expectedRevision: 2 }],
    ]);
  });

  it("appends exportNoteMarkdown with the exact flat payload", async () => {
    invokeMock.mockResolvedValue({ id: "note-1", path: "C:\\Exports\\2026-08-08.md", bytesWritten: 7 });
    const commands = await import(/* @vite-ignore */ commandsModulePath);

    await commands.exportNoteMarkdown({
      id: "note-1",
      directory: "C:\\Exports",
      expectedRevision: 4,
    });

    expect(invokeMock).toHaveBeenCalledWith("exportNoteMarkdown", {
      id: "note-1",
      directory: "C:\\Exports",
      expectedRevision: 4,
    });
  });

  it("opens the controlled note directory without accepting a frontend path", async () => {
    invokeMock.mockResolvedValue(undefined);
    const commands = await import(/* @vite-ignore */ commandsModulePath);

    await commands.openNoteDirectory();

    expect(invokeMock).toHaveBeenCalledWith("openNoteDirectory");
  });
});

describe("clipboard command bridge", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("passes all six locked clipboard command payloads without nesting", async () => {
    invokeMock.mockResolvedValue({});
    const commands = await import(/* @vite-ignore */ commandsModulePath);

    await commands.listClipboardItems({ query: "build", contentKind: "text", limit: 100 });
    await commands.copyClipboardItem({ id: "item-1" });
    await commands.setClipboardPinned({ id: "item-1", pinned: true });
    await commands.deleteClipboardItem({ id: "item-1" });
    await commands.clearClipboardHistory({ keepPinned: true });
    await commands.getClipboardAsset({ assetId: "asset-1" });

    expect(invokeMock.mock.calls).toEqual([
      ["listClipboardItems", { query: "build", contentKind: "text", limit: 100 }],
      ["copyClipboardItem", { id: "item-1" }],
      ["setClipboardPinned", { id: "item-1", pinned: true }],
      ["deleteClipboardItem", { id: "item-1" }],
      ["clearClipboardHistory", { keepPinned: true }],
      ["getClipboardAsset", { assetId: "asset-1" }],
    ]);
  });
});

describe("media command bridge", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("uses the exact flat media command union payload", async () => {
    invokeMock.mockResolvedValue({});
    const commands = await import(/* @vite-ignore */ commandsModulePath);

    await commands.getMediaSnapshot();
    await commands.sendMediaCommand({ command: "play" });
    await commands.sendMediaCommand({ command: "pause" });
    await commands.sendMediaCommand({ command: "previous" });
    await commands.sendMediaCommand({ command: "next" });
    await commands.sendMediaCommand({ command: "seek", positionSeconds: 42.5 });
    await commands.sendMediaCommand({ command: "setVolume", volumePercent: 35 });

    expect(invokeMock.mock.calls).toEqual([
      ["getMediaSnapshot"],
      ["sendMediaCommand", { command: "play" }],
      ["sendMediaCommand", { command: "pause" }],
      ["sendMediaCommand", { command: "previous" }],
      ["sendMediaCommand", { command: "next" }],
      ["sendMediaCommand", { command: "seek", positionSeconds: 42.5 }],
      ["sendMediaCommand", { command: "setVolume", volumePercent: 35 }],
    ]);
  });
});

describe("monitor and notification command bridge", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("uses all thirteen locked names and flat camelCase payloads", async () => {
    invokeMock.mockResolvedValue({});
    const commands = await import(/* @vite-ignore */ commandsModulePath);
    const watch = { id: null, processName: "aisland.exe", enabled: true, expectedRevision: null };
    const threshold = {
      id: null,
      metric: "cpuPercent" as const,
      comparator: "greaterThanOrEqual" as const,
      thresholdValue: 80,
      holdSeconds: 10,
      cooldownSeconds: 60,
      sound: { kind: "none" as const },
      toastEnabled: true,
      windowEnabled: false,
      enabled: true,
      expectedRevision: null,
    };

    await commands.getMonitorSnapshot();
    await commands.listMonitorSamples({ since: 100, limit: 450 });
    await commands.listProcessMetrics({ limit: 100 });
    await commands.listProcessWatches();
    await commands.saveProcessWatch(watch);
    await commands.deleteProcessWatch({ id: "watch-1", expectedRevision: 2 });
    await commands.listMonitorThresholds();
    await commands.saveMonitorThreshold(threshold);
    await commands.deleteMonitorThreshold({ id: "threshold-1", expectedRevision: 3 });
    await commands.listNotificationHistory({ origin: "windows", sourceApp: "Microsoft.WindowsStore", unreadOnly: true, limit: 100 });
    await commands.setNotificationRead({ id: "history-1", read: true });
    await commands.deleteNotificationHistory({ id: "history-1", confirmRemoval: true });
    await commands.clearNotificationHistory({ before: null, confirmRemoval: true });

    expect(invokeMock.mock.calls).toEqual([
      ["getMonitorSnapshot"],
      ["listMonitorSamples", { since: 100, limit: 450 }],
      ["listProcessMetrics", { limit: 100 }],
      ["listProcessWatches"],
      ["saveProcessWatch", watch],
      ["deleteProcessWatch", { id: "watch-1", expectedRevision: 2 }],
      ["listMonitorThresholds"],
      ["saveMonitorThreshold", threshold],
      ["deleteMonitorThreshold", { id: "threshold-1", expectedRevision: 3 }],
      ["listNotificationHistory", { origin: "windows", sourceApp: "Microsoft.WindowsStore", unreadOnly: true, limit: 100 }],
      ["setNotificationRead", { id: "history-1", read: true }],
      ["deleteNotificationHistory", { id: "history-1", confirmRemoval: true }],
      ["clearNotificationHistory", { before: null, confirmRemoval: true }],
    ]);
  });
});
