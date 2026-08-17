import { expect, test } from "vitest";
import messageCatalog from "../shared/messageCatalog.json";

import {
  DEFAULT_LANGUAGE,
  enUS,
  parseUiLanguage,
  translate,
  type TranslationKey,
  zhCN,
} from "./catalog";

const registeredMessageKeys = (await import("./catalog")).registeredMessageKeys as unknown as () => string[];
const translateRegisteredMessage = (await import("./catalog")).translateRegisteredMessage as unknown as (language: "zh-CN" | "en-US", key: string, parameters: Record<string, string | number | boolean>) => string;

test("keeps the registered React message catalog total and safe", () => {
  expect(registeredMessageKeys()).toEqual(Object.keys(messageCatalog.messages).sort());
  expect(translateRegisteredMessage("zh-CN", "reminders.agent.status", { agentName: "Codex", environment: "windows", taskId: "C:\\Build\\release", taskTitle: "\\\\server\\share\\release", triggerStatus: "failed" })).toContain("失败");
  expect(translateRegisteredMessage("en-US", "reminders.monitor.threshold", { metric: "networkReceive", currentValue: 12, thresholdValue: 10 })).toContain("Network receive");
  expect(() => translateRegisteredMessage("zh-CN", "reminders.todo.due", { todoTitle: "x".repeat(513) })).toThrow();
  expect(() => translateRegisteredMessage("zh-CN", "errors.conflict", { entityId: "C:\\Build\\release" })).toThrow();
});

test("renders every registered key in both locales and enforces the shared boundary", () => {
  const fixture = (key: string): Record<string, string | number | boolean> => {
    if (key === "settings.storage.retentionConfirmationRequired") return { clipboardRemovalCount: 12, notificationRemovalCount: 4 };
    if (key === "services.healthy") return { serviceId: "clipboard" };
    if (["services.degraded", "services.blocked", "services.offline"].includes(key)) return { serviceId: "clipboard", reasonCode: "failed" };
    if (["services.clipboard.locked", "home.agents.more"].includes(key)) return { count: 2 };
    if (key === "reminders.agent.status") return { agentName: "Codex", environment: "windows", taskId: "C:\\Build\\release", taskTitle: "\\\\server\\share\\release", triggerStatus: "failed" };
    if (key === "reminders.todo.due") return { todoTitle: "/opt/build/release" };
    if (key === "reminders.monitor.threshold") return { metric: "networkReceive", currentValue: 12, thresholdValue: 10 };
    if (["errors.notFound", "errors.conflict"].includes(key)) return { entityId: "item-1" };
    if (["errors.sourceUnavailable", "errors.platformUnsupported"].includes(key)) return { serviceId: "service", reasonCode: "failed" };
    if (["errors.integrationUnsupported", "errors.integrationConfigInvalid"].includes(key)) return { agentName: "Codex", environment: "windows", reasonCode: "failed" };
    if (key === "errors.integrationNotInstalled") return { agentName: "Codex", environment: "windows" };
    if (key.startsWith("errors.") && key !== "errors.serviceStopping") return { reasonCode: "failed" };
    return {};
  };
  for (const key of registeredMessageKeys()) {
    for (const language of ["zh-CN", "en-US"] as const) {
      expect(translateRegisteredMessage(language, key, fixture(key))).not.toMatch(/\{[^}]+\}/);
    }
  }
  for (const language of ["zh-CN", "en-US"] as const) {
    const agent = translateRegisteredMessage(language, "reminders.agent.status", { agentName: "Codex", environment: "windows", taskId: "C:\\Build\\release", taskTitle: "\\\\server\\share\\release", triggerStatus: "failed" });
    expect(agent).toContain("C:\\Build\\release");
    expect(agent).toContain("\\\\server\\share\\release");
    expect(translateRegisteredMessage(language, "reminders.todo.due", { todoTitle: "/opt/build/release" })).toContain("/opt/build/release");
  }
  for (const title of ["", null]) {
    const projected = { agentName: "Codex", environment: "windows", taskId: "task-1", taskTitle: title || "task-1", triggerStatus: "failed" };
    for (const language of ["zh-CN", "en-US"] as const) {
      const rendered = translateRegisteredMessage(language, "reminders.agent.status", projected);
      expect(rendered).toContain("task-1");
      expect(rendered).not.toMatch(/\{[^}]+\}/);
    }
  }
  expect(() => translateRegisteredMessage("zh-CN", "reminders.agent.status", { agentName: "Codex", environment: "windows", taskId: "task-1", taskTitle: "bad\ntext", triggerStatus: "failed" })).toThrow();
  expect(() => translateRegisteredMessage("zh-CN", "services.healthy", { serviceId: "/opt/build/release" })).toThrow();
  expect(() => translateRegisteredMessage("zh-CN", "errors.ioFailure", { body: "secret" } as never)).toThrow();
});

const requiredTranslationKeys = [
  "tab.home",
  "tab.notes",
  "tab.clipboard",
  "tab.monitor",
  "tab.notifications",
  "tab.settings",
  "aria.tabList",
  "aria.agentStatus",
  "aria.windowScale",
  "aria.windowHeight",
  "aria.expandIsland",
  "action.back",
  "action.cancel",
  "action.close",
  "action.collapse",
  "action.expand",
  "action.reset",
  "action.retry",
  "action.save",
  "settings.category.general",
  "settings.category.display",
  "settings.category.storage",
  "settings.category.agents",
  "settings.category.reminders",
  "settings.category.modules",
  "settings.category.diagnostics",
  "settings.categories.diagnostics.title",
  "settings.categories.diagnostics.description",
  "diagnostics.title",
  "diagnostics.storage.title",
  "diagnostics.storage.healthy",
  "diagnostics.storage.check",
  "diagnostics.services.title",
  "diagnostics.events.title",
  "diagnostics.events.empty",
  "diagnostics.states.healthy",
  "diagnostics.states.degraded",
  "diagnostics.states.blocked",
  "diagnostics.states.offline",
  "diagnostics.actions.retry",
  "settings.language",
  "settings.language.zhCN",
  "settings.language.enUS",
  "settings.scale",
  "settings.scale.small",
  "settings.scale.medium",
  "settings.scale.large",
  "settings.scale.xlarge",
  "common.comingSoon",
  "error.languageStorage",
  "error.languageNative",
  "home.agents.title",
  "agents.environments.windows",
  "agents.environments.wsl",
  "agents.status.idle",
  "agents.status.running",
  "agents.status.completed",
  "agents.status.failed",
  "agents.status.waiting",
  "agents.status.timeout",
  "agents.status.offline",
  "agents.tasks.empty",
  "agents.reply.latest",
  "agents.reply.empty",
  "agents.activity.latest",
  "agentProfiles.discovery.scan",
  "agentProfiles.discovery.title",
  "agentProfiles.discovery.readOnly",
  "agentProfiles.discovery.empty",
  "agentProfiles.discovery.state.automatic",
  "agentProfiles.discovery.state.hookConfigured",
  "agentProfiles.discovery.state.readyToInstall",
  "agentProfiles.discovery.state.detectionPending",
  "agentProfiles.discovery.state.adapterRequired",
  "agentProfiles.discovery.evidence.runningProcess",
  "agentProfiles.discovery.evidence.configuration",
  "agentProfiles.discovery.evidence.installedApplication",
  "agentProfiles.discovery.configurePreset",
  "agentProfiles.discovery.configureCustom",
] as const satisfies readonly TranslationKey[];

test("keeps the fixed built-in-agent Home copy exact in both locales", () => {
  expect(zhCN["home.agents.title"]).toBe("Agent 状态");
  expect(enUS["home.agents.title"]).toBe("Agent status");
  expect(zhCN["agents.status.running"]).toBe("运行中");
  expect(enUS["agents.status.running"]).toBe("Running");
  expect(zhCN["agents.tasks.empty"]).toBe("暂无任务状态");
  expect(enUS["agents.tasks.empty"]).toBe("No task status yet");
  expect(zhCN["agents.reply.latest"]).toBe("最近回复");
  expect(enUS["agents.reply.latest"]).toBe("Latest reply");
  expect(zhCN["agents.reply.empty"]).toBe("暂无最近回复");
  expect(enUS["agents.reply.empty"]).toBe("No recent reply");
  expect(zhCN["agents.activity.latest"]).toBe("最新动态");
  expect(enUS["agents.activity.latest"]).toBe("Latest activity");
});

test("describes the available Agent integration category in both locales", () => {
  expect(zhCN["settings.summary.agents"]).toBe("管理固定 Agent 接入");
  expect(enUS["settings.summary.agents"]).toBe("Manage fixed Agent integrations");
});

test("describes the Custom Hook timeout as startup-only rather than idle shutdown", () => {
  expect(zhCN["agentProfiles.field.timeout"]).toBe("启动超时（秒）");
  expect(enUS["agentProfiles.field.timeout"]).toBe("Startup timeout (seconds)");
});

test("documents the direct Windows executable boundary for Custom Hooks", () => {
  expect(zhCN["agentProfiles.field.executable"]).toBe("可执行文件（Windows .exe）");
  expect(enUS["agentProfiles.field.executable"]).toBe("Executable (Windows .exe)");
  expect((zhCN as Record<string, string>)["agentProfiles.field.executableWslUnsupported"]).toBe("可执行文件（WSL 暂不支持接入）");
  expect((enUS as Record<string, string>)["agentProfiles.field.executableWslUnsupported"]).toBe("Executable (WSL integration is not supported yet)");
});

test("includes the exact clipboard history copy in both locales", () => {
  const approved: Record<string, readonly [string, string]> = {
    "clipboard.title": ["剪贴板", "Clipboard"],
    "clipboard.field.search": ["搜索剪贴板历史", "Search clipboard history"],
    "clipboard.filter.all": ["全部", "All"],
    "clipboard.filter.text": ["文本", "Text"],
    "clipboard.filter.image": ["图片", "Images"],
    "clipboard.empty": ["暂无剪贴板历史", "No clipboard history"],
    "clipboard.action.copy": ["复制", "Copy"],
    "clipboard.action.pin": ["置顶", "Pin"],
    "clipboard.action.unpin": ["取消置顶", "Unpin"],
    "clipboard.action.delete": ["删除", "Delete"],
    "clipboard.action.clear": ["清空历史", "Clear history"],
    "clipboard.action.clearUnpinned": ["仅清空未置顶", "Clear unpinned only"],
    "clipboard.confirm.delete": ["删除这条 AIsland 剪贴板记录？", "Delete this AIsland clipboard item?"],
    "clipboard.confirm.clear": ["清空所选范围内的 AIsland 剪贴板历史？", "Clear AIsland clipboard history in the selected scope?"],
    "clipboard.source.unknown": ["未知来源", "Unknown source"],
    "clipboard.image.alt": ["剪贴板图片", "Clipboard image"],
    "clipboard.image.unavailable": ["无法读取图片，原记录仍已保留", "Unable to read the image. The original record was kept."],
    "clipboard.state.copied": ["已复制到剪贴板", "Copied to clipboard"],
    "clipboard.error.captureUnavailable": ["剪贴板监听暂不可用", "Clipboard monitoring is temporarily unavailable"],
    "clipboard.error.contentTooLarge": ["此内容超过剪贴板历史大小限制", "This content exceeds the clipboard history size limit"],
    "clipboard.error.actionFailed": ["剪贴板操作失败，请重试", "Clipboard action failed. Try again."],
  };
  for (const [key, [zh, en]] of Object.entries(approved)) {
    expect((zhCN as Record<string, string>)[key]).toBe(zh);
    expect((enUS as Record<string, string>)[key]).toBe(en);
  }
});

test("includes the exact media copy in both locales", () => {
  const exact = {
    "media.title": ["媒体", "Media"],
    "media.state.noSession": ["当前没有可用的媒体会话", "No media session is available"],
    "media.state.unavailable": ["Windows 媒体控制不可用", "Windows media controls are unavailable"],
    "media.state.playing": ["正在播放", "Playing"],
    "media.state.paused": ["已暂停", "Paused"],
    "media.state.stopped": ["已停止", "Stopped"],
    "media.action.previous": ["上一首", "Previous"],
    "media.action.play": ["播放", "Play"],
    "media.action.pause": ["暂停", "Pause"],
    "media.action.next": ["下一首", "Next"],
    "media.field.progress": ["播放进度", "Playback progress"],
    "media.field.volume": ["系统音量", "System volume"],
    "media.hint.systemVolume": ["控制 Windows 默认输出设备的主音量", "Controls the master volume of the default Windows output device"],
    "media.state.readOnly": ["当前会话不支持此操作", "The current session does not support this action"],
    "media.error.controlFailed": ["媒体控制失败，请重试", "Media control failed. Try again."],
  } as const;
  for (const [key, [zh, en]] of Object.entries(exact)) {
    expect(translate("zh-CN", key as TranslationKey)).toBe(zh);
    expect(translate("en-US", key as TranslationKey)).toBe(en);
  }
});

test("includes the fixed monitor catalog table exactly in both locales", () => {
  const exact = {
    "monitor.title": ["系统监控", "System monitor"],
    "monitor.cpu": ["CPU", "CPU"],
    "monitor.memory": ["内存", "Memory"],
    "monitor.diskRead": ["磁盘读取", "Disk read"],
    "monitor.diskWrite": ["磁盘写入", "Disk write"],
    "monitor.networkReceive": ["网络接收", "Network receive"],
    "monitor.networkSend": ["网络发送", "Network send"],
    "monitor.gpu": ["GPU", "GPU"],
    "monitor.processes": ["指定进程", "Watched processes"],
    "monitor.trend15m": ["最近 15 分钟", "Last 15 minutes"],
    "monitor.noSamples": ["等待首个有效采样", "Waiting for the first valid sample"],
    "monitor.gpuUnavailable": ["GPU 数据源不可用", "GPU data source unavailable"],
    "monitor.thresholds": ["阈值", "Thresholds"],
    "monitor.threshold.new": ["新建阈值", "New threshold"],
    "monitor.threshold.hold": ["持续时间", "Hold time"],
    "monitor.threshold.cooldown": ["冷却时间", "Cooldown"],
    "monitor.processWatch.add": ["添加进程", "Add process"],
    "monitor.processWatch.empty": ["尚未指定进程", "No watched processes"],
  } as const;
  for (const [key, [zh, en]] of Object.entries(exact)) {
    expect((zhCN as Record<string, string>)[key]).toBe(zh);
    expect((enUS as Record<string, string>)[key]).toBe(en);
  }
});

test("includes the fixed notification center catalog table exactly in both locales", () => {
  const exact = {
    "notifications.title": ["通知中心", "Notification center"],
    "notifications.origin.all": ["全部", "All"],
    "notifications.origin.windows": ["Windows", "Windows"],
    "notifications.origin.aisland": ["AIsland", "AIsland"],
    "notifications.filter.source": ["来源", "Source"],
    "notifications.filter.unread": ["仅未读", "Unread only"],
    "notifications.markRead": ["标为已读", "Mark as read"],
    "notifications.markUnread": ["标为未读", "Mark as unread"],
    "notifications.delete": ["删除此条", "Delete this item"],
    "notifications.clear": ["清空记录", "Clear history"],
    "notifications.deleteConfirm": ["仅从 AIsland 通知中心移除此条记录？Windows 原通知不会被修改。", "Remove this item only from AIsland history? The original Windows notification will not be changed."],
    "notifications.clearConfirm": ["仅清空 AIsland 保存的通知记录？Windows 原通知和提醒历史不会被删除。", "Clear only notification records saved by AIsland? Original Windows notifications and reminder history will not be deleted."],
    "notifications.empty": ["没有符合条件的通知", "No notifications match the filters"],
    "notifications.sourceUnavailable": ["Windows 通知记录不可用", "Windows notification history is unavailable"],
    "notifications.schemaIncompatible": ["此 Windows 通知格式暂不兼容", "This Windows notification format is not supported"],
  } as const;
  for (const [key, [zh, en]] of Object.entries(exact)) {
    expect((zhCN as Record<string, string>)[key]).toBe(zh);
    expect((enUS as Record<string, string>)[key]).toBe(en);
  }
});

test("includes every approved built-in agent and reminder literal exactly", () => {
  const approved = {
    "home.agents.title": ["Agent 状态", "Agent status"], "home.agents.more": ["+{count}", "+{count}"],
    "agents.environments.windows": ["Windows", "Windows"], "agents.environments.wsl": ["WSL", "WSL"],
    "agents.status.idle": ["空闲", "Idle"], "agents.status.running": ["运行中", "Running"], "agents.status.completed": ["已完成", "Completed"], "agents.status.failed": ["失败", "Failed"], "agents.status.waiting": ["等待操作", "Waiting"], "agents.status.timeout": ["已超时", "Timed out"], "agents.status.offline": ["离线", "Offline"], "agents.tasks.empty": ["暂无任务状态", "No task status yet"],
    "agents.integration.install": ["安装集成", "Install integration"], "agents.integration.repair": ["修复集成", "Repair integration"], "agents.integration.uninstall": ["卸载集成", "Uninstall integration"], "agents.integration.installed": ["已安装", "Installed"], "agents.integration.needsRepair": ["需要修复", "Needs repair"], "agents.integration.unsupported": ["此环境不受支持", "This environment is not supported"], "agents.integration.confirmTitle": ["仅移除 AIsland 集成", "Remove only the AIsland integration"], "agents.integration.confirmBody": ["将备份当前配置，并仅移除 AIsland 管理的条目。", "The current configuration will be backed up, and only AIsland-managed entries will be removed."],
    "reminders.title": ["提醒规则", "Reminder rules"], "reminders.new": ["新建规则", "New rule"], "reminders.agents": ["Agent", "Agents"], "reminders.triggers": ["触发条件", "Triggers"], "reminders.delay": ["延迟", "Delay"], "reminders.delay.immediate": ["立即", "Immediately"], "reminders.channels": ["提醒方式", "Channels"], "reminders.channels.sound": ["声音", "Sound"], "reminders.channels.toast": ["Windows 通知", "Windows notification"], "reminders.channels.window": ["独立提醒窗口", "Standalone alert window"], "reminders.sound.default": ["内置提示音", "Built-in alert sound"], "reminders.sound.local": ["本地音频", "Local audio file"], "reminders.sound.none": ["无声音", "No sound"], "reminders.enabled": ["已启用", "Enabled"], "reminders.disabled": ["已停用", "Disabled"], "reminders.delete.confirm": ["删除规则并取消尚未触发的计划？历史记录会保留。", "Delete the rule and cancel pending schedules? History will be retained."],
    "alert.acknowledge": ["知道了", "Acknowledge"], "alert.complete": ["完成", "Complete"], "alert.snooze": ["稍后提醒", "Snooze"], "alert.openContext": ["打开相关内容", "Open context"], "alert.mergedCount": ["已合并 {count} 条", "{count} alerts merged"], "alert.occurredAt": ["发生于 {time}", "Occurred at {time}"],
  } as const;
  for (const [key, [zh, en]] of Object.entries(approved)) {
    expect(zhCN[key as TranslationKey]).toBe(zh);
    expect(enUS[key as TranslationKey]).toBe(en);
    expect([...zh.matchAll(/\{([^}]+)\}/g)].map((match) => match[1])).toEqual([...en.matchAll(/\{([^}]+)\}/g)].map((match) => match[1]));
  }
});

test("includes typed reminder editor copy in both locales", () => {
  const copy = {
    "reminders.validation.agents": ["至少选择一个 Agent", "Select at least one Agent"],
    "reminders.validation.triggers": ["至少选择一个触发条件", "Select at least one trigger"],
    "reminders.validation.delay": ["延迟必须在 0 到 604800 秒之间", "Delay must be between 0 and 604800 seconds"],
    "reminders.validation.channels": ["至少选择一种提醒方式", "Select at least one reminder channel"],
    "reminders.local.choose": ["选择本地音频", "Choose local audio"],
    "reminders.reload": ["重新加载", "Reload"],
    "reminders.disable.confirm": ["停用后将取消尚未触发的计划，历史记录会保留。", "Disabling cancels pending schedules. History will be retained."],
  } as const;
  for (const [key, [zh, en]] of Object.entries(copy)) {
    expect(zhCN[key as TranslationKey]).toBe(zh);
    expect(enUS[key as TranslationKey]).toBe(en);
  }
});

test("includes standalone alert focus and unknown-context guidance in both locales", () => {
  expect(zhCN["alert.focus" as TranslationKey]).toBe("聚焦提醒");
  expect(enUS["alert.focus" as TranslationKey]).toBe("Focus alert");
  expect(zhCN["alert.unknownContext" as TranslationKey]).toBe("相关内容将在主窗口中显示。");
  expect(enUS["alert.unknownContext" as TranslationKey]).toBe("Related content will appear in the main window.");
});

test.each([
  ["zh-CN", "zh-CN"],
  ["en-US", "en-US"],
  [undefined, "zh-CN"],
  [null, "zh-CN"],
  ["", "zh-CN"],
  ["fr-FR", "zh-CN"],
  [{ language: "en-US" }, "zh-CN"],
])("parseUiLanguage(%o) returns %s", (value, expected) => {
  expect(parseUiLanguage(value)).toBe(expected);
});

test("uses Chinese as the default UI language", () => {
  expect(DEFAULT_LANGUAGE).toBe("zh-CN");
});

test("keeps the Chinese and English catalog keys identical", () => {
  expect(Object.keys(enUS).sort()).toEqual(Object.keys(zhCN).sort());
});

test("includes every fixed UI label needed by the bilingual settings flow", () => {
  for (const key of requiredTranslationKeys) {
    expect(zhCN[key]).toEqual(expect.any(String));
    expect(enUS[key]).toEqual(expect.any(String));
  }
});

test("never renders a translation key as its fallback label", () => {
  for (const language of ["zh-CN", "en-US"] as const) {
    for (const key of Object.keys(zhCN) as TranslationKey[]) {
      expect(translate(language, key)).not.toBe(key);
      expect(translate(language, key).trim()).not.toBe("");
    }
  }
});

test("includes every locked to-do literal exactly in both locales", () => {
  const copy = {
    "todo.title": ["待办", "To-dos"],
    "todo.filter.all": ["全部", "All"],
    "todo.filter.open": ["未完成", "Open"],
    "todo.filter.completed": ["已完成", "Completed"],
    "todo.empty.open": ["暂无未完成待办", "No open to-dos"],
    "todo.empty.completed": ["暂无已完成待办", "No completed to-dos"],
    "todo.action.create": ["新建待办", "New to-do"],
    "todo.action.edit": ["编辑", "Edit"],
    "todo.action.complete": ["完成", "Complete"],
    "todo.action.reopen": ["重新打开", "Reopen"],
    "todo.action.delete": ["删除", "Delete"],
    "todo.action.saveReminder": ["保存提醒", "Save reminder"],
    "todo.action.deleteReminder": ["删除提醒", "Delete reminder"],
    "todo.field.title": ["标题", "Title"],
    "todo.field.description": ["说明", "Description"],
    "todo.field.dueAt": ["截止时间", "Due date"],
    "todo.field.priority": ["优先级", "Priority"],
    "todo.priority.low": ["低", "Low"],
    "todo.priority.normal": ["普通", "Normal"],
    "todo.priority.high": ["高", "High"],
    "todo.reminder.title": ["待办提醒", "To-do reminder"],
    "todo.reminder.enabled": ["启用提醒", "Enable reminder"],
    "todo.reminder.remindAt": ["提醒时间", "Remind at"],
    "todo.confirm.delete": ["删除这个待办及其提醒？", "Delete this to-do and its reminder?"],
    "todo.confirm.deleteReminder": ["删除这个待办提醒？", "Delete this to-do reminder?"],
    "todo.error.titleRequired": ["请输入待办标题", "Enter a to-do title"],
    "todo.error.saveFailed": ["待办未保存，请重试", "The to-do was not saved. Try again."],
    "todo.error.reminderFailed": ["提醒未保存，请重试", "The reminder was not saved. Try again."],
  } as const;

  for (const [key, [zh, en]] of Object.entries(copy)) {
    expect(zhCN[key as TranslationKey]).toBe(zh);
    expect(enUS[key as TranslationKey]).toBe(en);
  }
});

test("matches the complete authoritative daily-note table exactly in both locales", () => {
  const copy = {
    "notes.title": ["每日笔记", "Daily Notes"],
    "notes.field.date": ["日期", "Date"],
    "notes.field.search": ["搜索笔记", "Search notes"],
    "notes.search.placeholder": ["搜索日期或正文", "Search dates or note text"],
    "notes.editor.placeholder": ["用 Markdown 记录今天的内容", "Write today's note in Markdown"],
    "notes.state.saved": ["已保存", "Saved"],
    "notes.state.saving": ["正在保存", "Saving"],
    "notes.state.unsaved": ["尚未保存", "Not saved"],
    "notes.empty.search": ["没有匹配的笔记", "No matching notes"],
    "notes.action.copy": ["复制", "Copy"],
    "notes.action.export": ["导出 Markdown", "Export Markdown"],
    "notes.action.openFolder": ["打开笔记文件夹", "Open notes folder"],
    "notes.action.delete": ["删除当天笔记", "Delete this day's note"],
    "notes.confirm.delete": ["删除这一天的笔记？", "Delete this day's note?"],
    "notes.copy.success": ["笔记已复制", "Note copied"],
    "notes.export.success": ["已导出", "Exported"],
    "notes.error.autosave": ["自动保存失败，编辑内容仍保留在此窗口", "Autosave failed. Your edits remain in this window."],
    "notes.error.copy": ["无法复制笔记", "Unable to copy the note"],
    "notes.error.exportExists": ["目标文件已存在，请选择其他目录或文件名", "The target file already exists. Choose another directory or file name."],
    "notes.error.exportFailed": ["无法导出笔记", "Unable to export the note"],
  } as const;

  const actualKeys = Object.keys(zhCN).filter((key) => key.startsWith("notes.")).sort();
  expect(actualKeys).toEqual(Object.keys(copy).sort());

  for (const [key, [zh, en]] of Object.entries(copy)) {
    expect(zhCN[key as TranslationKey]).toBe(zh);
    expect(enUS[key as TranslationKey]).toBe(en);
    expect(Object.keys(zhCN).filter((candidate) => candidate === key)).toHaveLength(1);
    expect(Object.keys(enUS).filter((candidate) => candidate === key)).toHaveLength(1);
  }
});
