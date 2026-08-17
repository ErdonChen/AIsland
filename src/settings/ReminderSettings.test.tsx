import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";

const { saveReminderRuleMock, deleteReminderRuleMock, chooseLocalAudioFileMock } = vi.hoisted(() => ({
  saveReminderRuleMock: vi.fn(),
  deleteReminderRuleMock: vi.fn(),
  chooseLocalAudioFileMock: vi.fn(),
}));

vi.mock("../api/commands", () => ({
  saveReminderRule: saveReminderRuleMock,
  deleteReminderRule: deleteReminderRuleMock,
}));
vi.mock("../api/dialog", () => ({ chooseLocalAudioFile: chooseLocalAudioFileMock }));

import { I18nProvider } from "../i18n/I18nProvider";

async function loadReminderSettings() {
  const componentPath = "./Reminder" + (window.location.pathname ? "Settings" : "Settings");
  return (await import(/* @vite-ignore */ componentPath)).default;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
}

afterEach(() => {
  cleanup();
  saveReminderRuleMock.mockReset();
  deleteReminderRuleMock.mockReset();
  chooseLocalAudioFileMock.mockReset();
  localStorage.clear();
});

test("blocks a rule without an Agent, trigger, valid delay, or delivery channel", async () => {
  const user = userEvent.setup();
  const ReminderSettings = await loadReminderSettings();
  render(<I18nProvider><ReminderSettings rule={null} onSaved={vi.fn()} onDeleted={vi.fn()} /></I18nProvider>);

  await user.click(screen.getByRole("checkbox", { name: "Codex" }));
  expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
  expect(screen.getByText("至少选择一个 Agent")).toBeInTheDocument();

  await user.click(screen.getByRole("checkbox", { name: "Codex" }));
  await user.click(screen.getByRole("checkbox", { name: "已完成" }));
  expect(screen.getByText("至少选择一个触发条件")).toBeInTheDocument();
  await user.click(screen.getByRole("checkbox", { name: "已完成" }));
  await user.clear(screen.getByRole("spinbutton", { name: "延迟" }));
  await user.type(screen.getByRole("spinbutton", { name: "延迟" }), "604801");
  expect(screen.getByText("延迟必须在 0 到 604800 秒之间")).toBeInTheDocument();

  await user.clear(screen.getByRole("spinbutton", { name: "延迟" }));
  await user.type(screen.getByRole("spinbutton", { name: "延迟" }), "0");
  await user.click(screen.getByRole("checkbox", { name: "声音" }));
  await user.click(screen.getByRole("checkbox", { name: "Windows 通知" }));
  await user.click(screen.getByRole("checkbox", { name: "独立提醒窗口" }));
  expect(screen.getByText("至少选择一种提醒方式")).toBeInTheDocument();

  await user.click(screen.getByRole("checkbox", { name: "已启用" }));
  expect(screen.getByText("停用后将取消尚未触发的计划，历史记录会保留。")).toBeInTheDocument();
});

test("keeps a selected local path and draft while backend validation rejects it", async () => {
  const user = userEvent.setup();
  const ReminderSettings = await loadReminderSettings();
  chooseLocalAudioFileMock.mockResolvedValue("C:\\sounds\\too-large.mp3");
  saveReminderRuleMock.mockRejectedValue({
    code: "invalidInput",
    messageKey: "errors.invalidInput",
    details: { field: "sound", reasonCode: "fileTooLarge" },
    retryable: false,
  });
  render(<I18nProvider><ReminderSettings rule={null} onSaved={vi.fn()} onDeleted={vi.fn()} /></I18nProvider>);

  await user.click(screen.getByRole("radio", { name: "本地音频" }));
  await user.click(screen.getByRole("button", { name: "选择本地音频" }));
  await user.click(screen.getByRole("button", { name: "保存" }));

  await waitFor(() => expect(saveReminderRuleMock).toHaveBeenCalledWith(expect.objectContaining({
    sound: { kind: "localFile", canonicalPath: "C:\\sounds\\too-large.mp3" },
  })));
  expect(await screen.findByRole("alert")).toBeInTheDocument();
  expect(screen.getByText("too-large.mp3")).toBeInTheDocument();
  expect(localStorage.getItem("aisland.reminders.soundPath")).toBeNull();
});

test("keeps save and delete pending, and preserves a conflicting edit until reload", async () => {
  const user = userEvent.setup();
  const ReminderSettings = await loadReminderSettings();
  const save = deferred<unknown>();
  saveReminderRuleMock.mockReturnValue(save.promise);
  render(<I18nProvider><ReminderSettings rule={null} onSaved={vi.fn()} onDeleted={vi.fn()} /></I18nProvider>);

  const saveButton = screen.getByRole("button", { name: "保存" });
  await user.click(saveButton);
  expect(saveButton).toBeDisabled();
  save.reject({ code: "conflict", messageKey: "errors.conflict", details: { entityId: "rule-1" }, retryable: true });
  await screen.findByRole("alert");
  expect(screen.getByRole("button", { name: "重新加载" })).toBeInTheDocument();
  expect(screen.getByRole("checkbox", { name: "Codex" })).toBeChecked();
});

test("keeps deletion pending until the backend resolves", async () => {
  const user = userEvent.setup();
  const ReminderSettings = await loadReminderSettings();
  const deletion = deferred<unknown>();
  deleteReminderRuleMock.mockReturnValue(deletion.promise);
  vi.stubGlobal("confirm", vi.fn().mockReturnValue(true));
  render(<I18nProvider><ReminderSettings rule={{ id: "7ac3ef17-5e3f-4b6a-af0a-8b907703e768", agentIds: ["codex"], triggerStatuses: ["completed"], delaySeconds: 0, sound: { kind: "none" }, toastEnabled: true, windowEnabled: false, enabled: true, revision: 4, createdAt: 1, updatedAt: 1 }} onSaved={vi.fn()} onDeleted={vi.fn()} /></I18nProvider>);

  const remove = screen.getByRole("button", { name: "删除规则" });
  await user.click(remove);
  expect(remove).toBeDisabled();
  expect(deleteReminderRuleMock).toHaveBeenCalledWith({ id: "7ac3ef17-5e3f-4b6a-af0a-8b907703e768", expectedRevision: 4 });
  deletion.resolve({ id: "7ac3ef17-5e3f-4b6a-af0a-8b907703e768", deleted: true });
});

test("submits multiple agents and triggers as unique, canonical string-sorted arrays", async () => {
  const user = userEvent.setup();
  const ReminderSettings = await loadReminderSettings();
  saveReminderRuleMock.mockResolvedValue({ id: "7ac3ef17-5e3f-4b6a-af0a-8b907703e768" });
  render(<I18nProvider><ReminderSettings rule={null} onSaved={vi.fn()} onDeleted={vi.fn()} /></I18nProvider>);

  await user.click(screen.getByRole("checkbox", { name: "Codex" }));
  await user.click(screen.getByRole("checkbox", { name: "Hermes" }));
  await user.click(screen.getByRole("checkbox", { name: "Codex" }));
  await user.click(screen.getByRole("checkbox", { name: "已完成" }));
  await user.click(screen.getByRole("checkbox", { name: "失败" }));
  await user.click(screen.getByRole("checkbox", { name: "已完成" }));
  await user.click(screen.getByRole("button", { name: "保存" }));

  await waitFor(() => expect(saveReminderRuleMock).toHaveBeenCalledWith(expect.objectContaining({
    agentIds: ["codex", "hermes"],
    triggerStatuses: ["completed", "failed"],
  })));
});

test("disables every editable control while a save is pending", async () => {
  const user = userEvent.setup();
  const ReminderSettings = await loadReminderSettings();
  const save = deferred<unknown>();
  chooseLocalAudioFileMock.mockResolvedValue("C:\\sounds\\ready.mp3");
  saveReminderRuleMock.mockReturnValue(save.promise);
  render(<I18nProvider><ReminderSettings rule={null} onSaved={vi.fn()} onDeleted={vi.fn()} /></I18nProvider>);

  await user.click(screen.getByRole("radio", { name: "本地音频" }));
  await user.click(screen.getByRole("button", { name: "选择本地音频" }));
  await user.click(screen.getByRole("button", { name: "保存" }));

  for (const control of [...screen.getAllByRole("checkbox"), ...screen.getAllByRole("radio"), screen.getByRole("spinbutton", { name: "延迟" }), screen.getByRole("button", { name: "选择本地音频" })]) {
    expect(control).toBeDisabled();
  }
  expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
  save.resolve({ id: "7ac3ef17-5e3f-4b6a-af0a-8b907703e768" });
});

test("does not call an obsolete save callback after the editor unmounts", async () => {
  const user = userEvent.setup();
  const ReminderSettings = await loadReminderSettings();
  const save = deferred<unknown>();
  const onSaved = vi.fn();
  saveReminderRuleMock.mockReturnValue(save.promise);
  const view = render(<I18nProvider><ReminderSettings rule={null} onSaved={onSaved} onDeleted={vi.fn()} /></I18nProvider>);

  await user.click(screen.getByRole("button", { name: "保存" }));
  view.unmount();
  save.resolve({ id: "7ac3ef17-5e3f-4b6a-af0a-8b907703e768" });
  await save.promise;
  expect(onSaved).not.toHaveBeenCalled();
});
