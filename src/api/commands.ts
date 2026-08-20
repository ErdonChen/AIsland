import { Channel, invoke } from "@tauri-apps/api/core";
import { parseCommandError } from "./commandError";
import type {
  AcknowledgeReminderNavigationInput,
  AgentIntegrationProfile,
  AgentIntegrationDiscoveryResult,
  AgentProfilesSnapshot,
  AgentIntegrationInput,
  AgentIntegrationResult,
  AgentsSnapshot,
  AppSnapshot,
  ClearResult,
  ClearNotificationHistoryInput,
  ClipboardAssetPayload,
  ClipboardItem,
  CommitReminderReplayCursorInput,
  CompleteTodoInput,
  CreateNoteInput,
  CreateTodoInput,
  DeleteAgentIntegrationProfileInput,
  DeleteReminderRuleInput,
  DeleteNotificationHistoryInput,
  DeleteResult,
  DiagnosticEvent,
  ExportNoteResult,
  GeneralSettings,
  InstallAgentIntegrationProfileInput,
  ListClipboardItemsInput,
  ListNotificationHistoryInput,
  ListTodosInput,
  MediaControlInput,
  MediaSnapshot,
  MonitorSnapshot,
  MonitorThreshold,
  NotificationHistoryItem,
  ListNotesInput,
  NoteDocument,
  NoteDateContentSummary,
  NoteRecording,
  NoteRecordingPayload,
  NoteSummary,
  PendingReminderNavigation,
  ReminderActionInput,
  ReminderAlertGroup,
  ReminderReplay,
  ReminderReplayCursor,
  ReminderRule,
  ProcessMetric,
  ProcessWatch,
  ReplayReminderDeliveriesInput,
  SaveReminderRuleInput,
  SaveAgentIntegrationProfileInput,
  SaveGeneralSettingsInput,
  SaveMonitorThresholdInput,
  SaveProcessWatchInput,
  SetNotificationReadInput,
  SaveTodoReminderInput,
  ServiceHealthSnapshot,
  SnoozeReminderInput,
  StorageIntegrityResult,
  TodoItem,
  TodoReminder,
  UninstallAgentIntegrationInput,
  UninstallAgentIntegrationProfileInput,
  RepairAgentIntegrationProfileInput,
  UpdateTodoInput,
  UpdateNoteInput,
  UpdateCheckResult,
  UpdateInstallEvent,
  UpdateInstallResult,
} from "./contracts";

async function invokeCommand<T>(command: string, payload?: Record<string, unknown>): Promise<T> {
  try {
    return payload === undefined ? await invoke<T>(command) : await invoke<T>(command, payload);
  } catch (error) {
    throw parseCommandError(error);
  }
}

export function getAppSnapshot(): Promise<AppSnapshot> {
  return invokeCommand("getAppSnapshot");
}

export function listServiceHealth(): Promise<ServiceHealthSnapshot[]> {
  return invokeCommand("listServiceHealth");
}

export function getDiagnostics(input: { limit: number }): Promise<DiagnosticEvent[]> {
  return invokeCommand("getDiagnostics", input);
}

export function checkStorageIntegrity(): Promise<StorageIntegrityResult> {
  return invokeCommand("checkStorageIntegrity");
}

export function getGeneralSettings(): Promise<GeneralSettings> {
  return invokeCommand("getGeneralSettings");
}

export function saveGeneralSettings(input: SaveGeneralSettingsInput): Promise<GeneralSettings> {
  return invokeCommand("saveGeneralSettings", { ...input });
}

export function checkForUpdate(): Promise<UpdateCheckResult> {
  return invokeCommand("checkForUpdate");
}

export function installUpdate(onEvent: (event: UpdateInstallEvent) => void): Promise<UpdateInstallResult> {
  const channel = new Channel<UpdateInstallEvent>();
  channel.onmessage = onEvent;
  return invokeCommand("installUpdate", { onEvent: channel });
}

export function getAgentsSnapshot(): Promise<AgentsSnapshot> {
  return invokeCommand("getAgentsSnapshot");
}

export function installAgentIntegration(input: AgentIntegrationInput): Promise<AgentIntegrationResult> {
  return invokeCommand("installAgentIntegration", { ...input });
}

export function repairAgentIntegration(input: AgentIntegrationInput): Promise<AgentIntegrationResult> {
  return invokeCommand("repairAgentIntegration", { ...input });
}

export function uninstallAgentIntegration(input: UninstallAgentIntegrationInput): Promise<AgentIntegrationResult> {
  return invokeCommand("uninstallAgentIntegration", { ...input });
}

export function listAgentIntegrationProfiles(): Promise<AgentIntegrationProfile[]> {
  return invokeCommand("listAgentIntegrationProfiles");
}

export function discoverAgentIntegrationCandidates(): Promise<AgentIntegrationDiscoveryResult> {
  return invokeCommand("discoverAgentIntegrationCandidates");
}

export function getAgentProfilesSnapshot(): Promise<AgentProfilesSnapshot> {
  return invokeCommand("getAgentProfilesSnapshot");
}

export function saveAgentIntegrationProfile(input: SaveAgentIntegrationProfileInput): Promise<AgentIntegrationProfile> {
  return invokeCommand("saveAgentIntegrationProfile", { ...input });
}

export function installAgentIntegrationProfile(input: InstallAgentIntegrationProfileInput): Promise<AgentIntegrationProfile> {
  return invokeCommand("installAgentIntegrationProfile", { ...input });
}

export function repairAgentIntegrationProfile(input: RepairAgentIntegrationProfileInput): Promise<AgentIntegrationProfile> {
  return invokeCommand("repairAgentIntegrationProfile", { ...input });
}

export function uninstallAgentIntegrationProfile(input: UninstallAgentIntegrationProfileInput): Promise<AgentIntegrationProfile> {
  return invokeCommand("uninstallAgentIntegrationProfile", { ...input });
}

export function deleteAgentIntegrationProfile(input: DeleteAgentIntegrationProfileInput): Promise<DeleteResult> {
  return invokeCommand("deleteAgentIntegrationProfile", { ...input });
}

export function listReminderRules(): Promise<ReminderRule[]> {
  return invokeCommand("listReminderRules");
}

export function saveReminderRule(input: SaveReminderRuleInput): Promise<ReminderRule> {
  return invokeCommand("saveReminderRule", { ...input });
}

export function deleteReminderRule(input: DeleteReminderRuleInput): Promise<DeleteResult> {
  return invokeCommand("deleteReminderRule", { ...input });
}

export function replayReminderDeliveries(input: ReplayReminderDeliveriesInput): Promise<ReminderReplay> {
  return invokeCommand("replayReminderDeliveries", { ...input });
}

export function commitReminderReplayCursor(input: CommitReminderReplayCursorInput): Promise<ReminderReplayCursor> {
  return invokeCommand("commitReminderReplayCursor", { ...input });
}

export function reloadReminderAlertGroup(input: { deliveryId: string }): Promise<ReminderAlertGroup | null> {
  return invokeCommand("reloadReminderAlertGroup", { ...input });
}

export function acknowledgeReminder(input: ReminderActionInput): Promise<ReminderAlertGroup> {
  return invokeCommand("acknowledgeReminder", { ...input });
}

export function completeReminder(input: ReminderActionInput): Promise<ReminderAlertGroup> {
  return invokeCommand("completeReminder", { ...input });
}

export function snoozeReminder(input: SnoozeReminderInput): Promise<ReminderAlertGroup> {
  return invokeCommand("snoozeReminder", { ...input });
}

export function getPendingReminderNavigation(): Promise<PendingReminderNavigation | null> {
  return invokeCommand("getPendingReminderNavigation");
}

export function acknowledgeReminderNavigation(input: AcknowledgeReminderNavigationInput): Promise<void> {
  return invokeCommand("acknowledgeReminderNavigation", { ...input });
}

export function listTodos(input: ListTodosInput): Promise<TodoItem[]> {
  return invokeCommand("listTodos", { ...input });
}

export function createTodo(input: CreateTodoInput): Promise<TodoItem> {
  return invokeCommand("createTodo", { ...input });
}

export function updateTodo(input: UpdateTodoInput): Promise<TodoItem> {
  return invokeCommand("updateTodo", { ...input });
}

export function completeTodo(input: CompleteTodoInput): Promise<TodoItem> {
  return invokeCommand("completeTodo", { ...input });
}

export function deleteTodo(input: { id: string; expectedRevision: number }): Promise<DeleteResult> {
  return invokeCommand("deleteTodo", { ...input });
}

export function saveTodoReminder(input: SaveTodoReminderInput): Promise<TodoReminder> {
  return invokeCommand("saveTodoReminder", { ...input });
}

export function listTodoReminders(input: { todoId: string | null }): Promise<TodoReminder[]> {
  return invokeCommand("listTodoReminders", { ...input });
}

export function deleteTodoReminder(input: { id: string; expectedRevision: number }): Promise<DeleteResult> {
  return invokeCommand("deleteTodoReminder", { ...input });
}

export function listNotes(input: ListNotesInput): Promise<NoteSummary[]> {
  return invokeCommand("listNotes", { ...input });
}

export function getNote(input: { id: string }): Promise<NoteDocument> {
  return invokeCommand("getNote", { ...input });
}

export function getDailyNote(input: { noteDate: string }): Promise<NoteDocument | null> {
  return invokeCommand("getDailyNote", { ...input });
}

export function startNoteRecording(input: { noteDate: string; mimeType: string; fileExtension: string; startedAt: number }): Promise<NoteRecording> {
  return invokeCommand("startNoteRecording", { ...input });
}

export function appendNoteRecordingChunk(input: { id: string; chunk: number[] }): Promise<void> {
  return invokeCommand("appendNoteRecordingChunk", { ...input });
}

export function finishNoteRecording(input: { id: string; durationMs: number; expectedRevision: number }): Promise<NoteRecording> {
  return invokeCommand("finishNoteRecording", { ...input });
}

export function listNoteRecordings(input: { noteDate: string }): Promise<NoteRecording[]> {
  return invokeCommand("listNoteRecordings", { ...input });
}

export function listNoteContentDates(input: { startDate: string; endDate: string }): Promise<NoteDateContentSummary[]> {
  return invokeCommand("listNoteContentDates", { ...input });
}

export function readNoteRecording(input: { id: string }): Promise<NoteRecordingPayload> {
  return invokeCommand("readNoteRecording", { ...input });
}

export function abortNoteRecording(input: { id: string; expectedRevision: number }): Promise<DeleteResult> {
  return invokeCommand("abortNoteRecording", { ...input });
}

export function deleteNoteRecording(input: { id: string; expectedRevision: number }): Promise<DeleteResult> {
  return invokeCommand("deleteNoteRecording", { ...input });
}

export function recoverNoteRecordings(): Promise<number> {
  return invokeCommand("recoverNoteRecordings");
}

export function createNote(input: CreateNoteInput): Promise<NoteDocument> {
  return invokeCommand("createNote", { ...input });
}

export function updateNote(input: UpdateNoteInput): Promise<NoteDocument> {
  return invokeCommand("updateNote", { ...input });
}

export function deleteNote(input: { id: string; expectedRevision: number }): Promise<DeleteResult> {
  return invokeCommand("deleteNote", { ...input });
}

export function exportNoteMarkdown(input: { id: string; directory: string; expectedRevision: number }): Promise<ExportNoteResult> {
  return invokeCommand("exportNoteMarkdown", { ...input });
}

export function openNoteDirectory(): Promise<void> {
  return invokeCommand("openNoteDirectory");
}

export function listClipboardItems(input: ListClipboardItemsInput): Promise<ClipboardItem[]> {
  return invokeCommand("listClipboardItems", { ...input });
}

export function copyClipboardItem(input: { id: string }): Promise<ClipboardItem> {
  return invokeCommand("copyClipboardItem", { ...input });
}

export function setClipboardPinned(input: { id: string; pinned: boolean }): Promise<ClipboardItem> {
  return invokeCommand("setClipboardPinned", { ...input });
}

export function deleteClipboardItem(input: { id: string }): Promise<DeleteResult> {
  return invokeCommand("deleteClipboardItem", { ...input });
}

export function clearClipboardHistory(input: { keepPinned: boolean }): Promise<ClearResult> {
  return invokeCommand("clearClipboardHistory", { ...input });
}

export function getClipboardAsset(input: { assetId: string }): Promise<ClipboardAssetPayload> {
  return invokeCommand("getClipboardAsset", { ...input });
}

export function getMediaSnapshot(): Promise<MediaSnapshot> {
  return invokeCommand("getMediaSnapshot");
}

export function sendMediaCommand(input: MediaControlInput): Promise<MediaSnapshot> {
  return invokeCommand("sendMediaCommand", { ...input });
}

export function getMonitorSnapshot(): Promise<MonitorSnapshot> {
  return invokeCommand("getMonitorSnapshot");
}

export function listMonitorSamples(input: { since: number; limit: number }): Promise<MonitorSnapshot[]> {
  return invokeCommand("listMonitorSamples", { ...input });
}

export function listProcessMetrics(input: { limit: number }): Promise<ProcessMetric[]> {
  return invokeCommand("listProcessMetrics", { ...input });
}

export function listProcessWatches(): Promise<ProcessWatch[]> {
  return invokeCommand("listProcessWatches");
}

export function saveProcessWatch(input: SaveProcessWatchInput): Promise<ProcessWatch> {
  return invokeCommand("saveProcessWatch", { ...input });
}

export function deleteProcessWatch(input: { id: string; expectedRevision: number }): Promise<DeleteResult> {
  return invokeCommand("deleteProcessWatch", { ...input });
}

export function listMonitorThresholds(): Promise<MonitorThreshold[]> {
  return invokeCommand("listMonitorThresholds");
}

export function saveMonitorThreshold(input: SaveMonitorThresholdInput): Promise<MonitorThreshold> {
  return invokeCommand("saveMonitorThreshold", { ...input });
}

export function deleteMonitorThreshold(input: { id: string; expectedRevision: number }): Promise<DeleteResult> {
  return invokeCommand("deleteMonitorThreshold", { ...input });
}

export function listNotificationHistory(input: ListNotificationHistoryInput): Promise<NotificationHistoryItem[]> {
  return invokeCommand("listNotificationHistory", { ...input });
}

export function setNotificationRead(input: SetNotificationReadInput): Promise<NotificationHistoryItem> {
  return invokeCommand("setNotificationRead", { ...input });
}

export function deleteNotificationHistory(input: DeleteNotificationHistoryInput): Promise<DeleteResult> {
  return invokeCommand("deleteNotificationHistory", { ...input });
}

export function clearNotificationHistory(input: ClearNotificationHistoryInput): Promise<ClearResult> {
  return invokeCommand("clearNotificationHistory", { ...input });
}
