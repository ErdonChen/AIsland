export type Locale = "zh-CN" | "en-US";
export type UnixMillis = number;
export type EntityId = string;
export type Revision = number;
export type LocalDate = string;

export type AgentId = "codex" | "hermes" | "workbuddy" | "claude";
export type AgentEnvironment = "windows" | "wsl";
export type AgentStatus = "idle" | "running" | "completed" | "failed" | "waiting" | "timeout" | "offline";
export type AgentTriggerStatus = "completed" | "failed" | "waiting" | "timeout";
export type IntegrationState = "notInstalled" | "installed" | "needsRepair" | "unsupported";
export type ModuleId = "todo" | "notes" | "clipboard" | "media" | "monitor" | "notifications";
export type ServiceHealthState = "healthy" | "degraded" | "blocked" | "offline";
export type ReminderSourceKind = "agent" | "todo" | "monitor";
export type ReminderDeliveryState = "pending" | "dispatched" | "acknowledged" | "snoozed" | "cancelled" | "completed";
export type ReminderSound = { kind: "none" } | { kind: "builtin"; soundId: "systemNotification" } | { kind: "localFile"; canonicalPath: string };
export type TodoPriority = "low" | "normal" | "high";
export type TodoStatus = "open" | "completed";
export type ClipboardContentKind = "text" | "image";
export type MonitorMetric = "cpuPercent" | "memoryPercent" | "diskReadBytesPerSecond" | "diskWriteBytesPerSecond" | "networkReceiveBytesPerSecond" | "networkSendBytesPerSecond" | "gpuPercent";
export type ThresholdComparator = "greaterThanOrEqual" | "lessThanOrEqual";
export type MediaPlaybackState = "playing" | "paused" | "stopped" | "unavailable";
export type MediaCommand = "play" | "pause" | "previous" | "next" | "seek" | "setVolume";
export type OnboardingStep = "language" | "modules" | "agents" | "ready";
export type ThemeMode = "system" | "dark" | "light";
export type AccentChoice = "ice" | "blue" | "violet" | "teal";
export type SafeParameterName = "agentName" | "environment" | "taskId" | "taskTitle" | "triggerStatus" | "todoTitle" | "metric" | "currentValue" | "thresholdValue" | "entityId" | "serviceId" | "reasonCode" | "field" | "contentKind" | "byteSize" | "sampledAt" | "count" | "limit" | "clipboardRemovalCount" | "notificationRemovalCount" | "sequence";
export type SafeParameterValue = string | number | boolean;
export type SafeMessageParameters = Partial<Record<SafeParameterName, SafeParameterValue>>;
export type AppErrorCode = "invalidInput" | "notFound" | "conflict" | "storageUnavailable" | "databaseFailure" | "ioFailure" | "permissionDenied" | "sourceUnavailable" | "platformUnsupported" | "integrationUnsupported" | "integrationNotInstalled" | "integrationConfigInvalid" | "notificationUnavailable";

export interface CommandError { code: AppErrorCode; messageKey: string; details: SafeMessageParameters; retryable: boolean; }
export interface ServiceHealthSnapshot { serviceId: string; state: ServiceHealthState; messageKey: string; parameters: SafeMessageParameters; checkedAt: UnixMillis; }
export interface AppSnapshot { locale: Locale; modules: Record<ModuleId, ModulePreference>; services: ServiceHealthSnapshot[]; storageSchemaVersion: number; }
export interface AgentObservation { agentId: AgentId; environment: AgentEnvironment; taskId: string; status: AgentStatus; summary: string; latestReplyPreview?: string | null; sourceEventId: string; occurredAt: UnixMillis; receivedAt: UnixMillis; }
export interface AgentSummary { agentId: AgentId; displayName: "Codex" | "Hermes" | "WorkBuddy" | "claude"; aggregateStatus: AgentStatus; environments: AgentObservation[]; integrations: AgentIntegrationRecord[]; }
export interface AgentIntegrationRecord { environment: AgentEnvironment; supported: boolean; required: boolean; state: IntegrationState; reasonCode: string | null; }
export interface AgentsSnapshot { agents: AgentSummary[]; generatedAt: UnixMillis; }
export interface ReminderRule { id: EntityId; agentIds: AgentId[]; triggerStatuses: AgentTriggerStatus[]; enabled: boolean; delaySeconds: number; sound: ReminderSound; toastEnabled: boolean; windowEnabled: boolean; revision: Revision; createdAt: UnixMillis; updatedAt: UnixMillis; }
export interface ReminderDelivery { id: EntityId; dedupeKey: string; ruleId: EntityId | null; sourceKind: ReminderSourceKind; sourceEntityId: string; messageKey: string; messageParameters: SafeMessageParameters; sourceContext: ReminderSourceContext; sourceOccurredAt: UnixMillis; sound: ReminderSound; state: ReminderDeliveryState; dueAt: UnixMillis; dispatchSeq: number; firstDispatchedAt: UnixMillis | null; lastDispatchedAt: UnixMillis | null; acknowledgedAt: UnixMillis | null; completedAt: UnixMillis | null; snoozedUntil: UnixMillis | null; createdAt: UnixMillis; updatedAt: UnixMillis; }
export type ReminderSourceContext =
  | { kind: "agent"; agentId: AgentId; environment: AgentEnvironment; taskId: string; taskTitle: string | null; triggerStatus: AgentTriggerStatus; sourceEventId: string; sourceOccurredAt: UnixMillis }
  | { kind: "todo"; todoId: EntityId; reminderRevision: Revision; todoTitle: string; sourceOccurredAt: UnixMillis }
  | { kind: "monitor"; thresholdId: EntityId; metric: MonitorMetric; currentValue: number; thresholdValue: number; breachStartedAt: UnixMillis; sourceOccurredAt: UnixMillis };
export interface ReminderAlertGroup { mergeKey: string; mergeIdentity: ReminderMergeIdentity; members: ReminderDelivery[]; sourceContext: ReminderSourceContext; newestSourceOccurredAt: UnixMillis; }
export type ReminderMergeIdentity =
  | { kind: "agent"; ruleId: EntityId; agentId: AgentId; environment: AgentEnvironment; taskId: string; triggerStatus: AgentTriggerStatus }
  | { kind: "todo"; todoId: EntityId; reminderRevision: Revision; deliveryId: EntityId }
  | { kind: "monitor"; thresholdId: EntityId; breachStartedAt: UnixMillis; deliveryId: EntityId };
export interface PendingReminderNavigation { sequence: number; deliveryId: EntityId; sourceKind: ReminderSourceKind; sourceEntityId: string; }
export interface TodoItem { id: EntityId; title: string; description: string; dueAt: UnixMillis | null; priority: TodoPriority; status: TodoStatus; revision: Revision; createdAt: UnixMillis; updatedAt: UnixMillis; completedAt: UnixMillis | null; }
export interface TodoReminder { id: EntityId; todoId: EntityId; remindAt: UnixMillis; enabled: boolean; revision: Revision; createdAt: UnixMillis; updatedAt: UnixMillis; }
export interface NoteSummary { id: EntityId; noteDate: LocalDate; excerpt: string; revision: Revision; updatedAt: UnixMillis; }
export interface NoteDocument { id: EntityId; noteDate: LocalDate; bodyMarkdown: string; revision: Revision; createdAt: UnixMillis; updatedAt: UnixMillis; }
export interface ClipboardItem { id: EntityId; contentKind: ClipboardContentKind; textContent: string | null; assetId: EntityId | null; sourceApp: string | null; pinned: boolean; capturedAt: UnixMillis; lastSeenAt: UnixMillis; byteSize: number; }
export interface MediaSnapshot { sessionId: string | null; title: string; artist: string; playbackState: MediaPlaybackState; positionSeconds: number; durationSeconds: number | null; volumePercent: number | null; canPlay: boolean; canPause: boolean; canPrevious: boolean; canNext: boolean; canSeek: boolean; canSetVolume: boolean; updatedAt: UnixMillis; }
// MediaSnapshot.volumePercent, MediaSnapshot.canSetVolume, and setVolume refer only to the Windows default render endpoint master volume.
export interface MonitorSnapshot { cpuPercent: number; memoryUsedBytes: number; memoryTotalBytes: number; diskReadBytesPerSecond: number; diskWriteBytesPerSecond: number; networkReceiveBytesPerSecond: number; networkSendBytesPerSecond: number; gpuPercent: number | null; sampledAt: UnixMillis; }
export interface ProcessWatch { id: EntityId; processName: string; enabled: boolean; revision: Revision; updatedAt: UnixMillis; }
export interface NotificationHistoryItem { id: EntityId; origin: "windows" | "aiceland"; appId: string; sourceEntityId: string; title: string; body: string; messageKey: string | null; messageParameters: SafeMessageParameters; sourceContext: ReminderSourceContext | null; sourceOccurredAt: UnixMillis; receivedAt: UnixMillis; readAt: UnixMillis | null; }
export interface ModulePreference { moduleId: ModuleId; visible: boolean; backgroundEnabled: boolean; revision: Revision; updatedAt: UnixMillis; }
export interface GeneralSettings { launchAtStartup: boolean; revision: Revision; updatedAt: UnixMillis; }
export type UpdateCheckStatus = "upToDate" | "available";
export interface UpdateCheckResult { status: UpdateCheckStatus; currentVersion: string; latestVersion: string | null; notes: string | null; }
export type UpdateInstallEvent =
  | { event: "started"; downloaded: number; total: number | null }
  | { event: "progress"; downloaded: number; total: number | null }
  | { event: "finished"; downloaded: number; total: number | null };
export interface UpdateInstallResult { installedVersion: string; restartRequired: boolean; }
export interface StorageSettings { clipboardRetentionItems: number; notificationRetentionItems: number; markdownExportDirectory: string | null; revision: Revision; updatedAt: UnixMillis; }
export interface DisplayPreferences { theme: ThemeMode; opacityPercent: number; accent: AccentChoice; }
export interface OnboardingState { currentStep: OnboardingStep; completed: boolean; locale: Locale; privacyConsentAt: UnixMillis | null; modulePreferences: ModulePreference[]; revision: Revision; updatedAt: UnixMillis; }
export interface PrivacyConsent { clipboardCapture: boolean; notificationImport: boolean; systemMonitoring: boolean; mediaSessionRead: boolean; backgroundReminders: boolean; }

export interface DiagnosticEvent { id: EntityId; serviceId: string; level: "info" | "warning" | "failure"; code: string; parameters: SafeMessageParameters; createdAt: UnixMillis; }
export interface StorageIntegrityResult { integrity: "ok"; schemaVersion: number; checkedAt: UnixMillis; }
export interface DeleteResult { id: EntityId; deleted: true; }
export interface ClearResult { removedCount: number; }
export interface AgentIntegrationInput { agentId: AgentId; environment: AgentEnvironment; }
export interface UninstallAgentIntegrationInput extends AgentIntegrationInput { confirmOwnedRemoval: true; }
export interface AgentIntegrationResult { agentId: AgentId; environment: AgentEnvironment; state: IntegrationState; configPath: string; backupPath: string | null; changed: boolean; }
export type AgentProfileEnvironment = "windows" | "wsl";
export type AgentProfileKind = "preset" | "custom";
export type AgentProfilePresetId = "kimi" | "trae" | "qoderwork" | "cursor";
export type AgentProfileInstallationState = "notInstalled" | "installed" | "needsRepair" | "unsupported";
export type AgentConfigTarget =
  | { kind: "preset"; adapterId: AgentProfilePresetId }
  | { kind: "customHook"; executable: string; argv: string[]; workingDirectory: string | null; timeoutSeconds: number; };
export interface AgentEventMapping { nativeEvent: string; normalizedStatus: AgentStatus; }
export interface AgentIntegrationProfile {
  id: EntityId;
  kind: AgentProfileKind;
  displayName: string;
  environment: AgentProfileEnvironment;
  configTarget: AgentConfigTarget;
  eventMapping: AgentEventMapping[];
  enabled: boolean;
  installationState: AgentProfileInstallationState;
  reasonCode: string | null;
  revision: Revision;
  updatedAt: UnixMillis;
}
export interface AgentProfileObservation {
  profileId: EntityId;
  environment: AgentProfileEnvironment;
  taskId: string;
  status: AgentStatus;
  latestReplyPreview?: string | null;
  sourceEventId: string;
  occurredAt: UnixMillis;
  receivedAt: UnixMillis;
}
export interface AgentProfileStatusSummary {
  profile: AgentIntegrationProfile;
  aggregateStatus: AgentStatus;
  observations: AgentProfileObservation[];
}
export interface AgentProfilesSnapshot {
  profiles: AgentProfileStatusSummary[];
  generatedAt: UnixMillis;
}
export type AgentIntegrationDiscoveryKind = "builtIn" | "preset" | "custom";
export type AgentIntegrationDiscoveryState = "automatic" | "readyToInstall" | "detectionPending" | "adapterRequired";
export type AgentIntegrationDiscoveryEvidence = "runningProcess" | "configuration" | "installedApplication";
export interface AgentIntegrationDiscoveryCandidate {
  id: string;
  displayName: string;
  environment: AgentProfileEnvironment;
  integrationKind: AgentIntegrationDiscoveryKind;
  state: AgentIntegrationDiscoveryState;
  presetId: AgentProfilePresetId | null;
  evidence: AgentIntegrationDiscoveryEvidence[];
  reasonCode: string | null;
}
export interface AgentIntegrationDiscoveryResult {
  candidates: AgentIntegrationDiscoveryCandidate[];
  scannedAt: UnixMillis;
}
export interface SaveAgentIntegrationProfileInput {
  id: EntityId | null;
  kind: AgentProfileKind;
  displayName: string;
  environment: AgentProfileEnvironment;
  configTarget: AgentConfigTarget;
  eventMapping: AgentEventMapping[];
  enabled: boolean;
  expectedRevision: Revision | null;
}
export interface InstallAgentIntegrationProfileInput { id: EntityId; expectedRevision: Revision; confirmInstallation: true; }
export interface RepairAgentIntegrationProfileInput { id: EntityId; expectedRevision: Revision; confirmRepair: true; }
export interface UninstallAgentIntegrationProfileInput { id: EntityId; expectedRevision: Revision; confirmOwnedRemoval: true; }
export interface DeleteAgentIntegrationProfileInput { id: EntityId; expectedRevision: Revision; confirmDeletion: true; }
export interface SaveReminderRuleInput { id: EntityId | null; agentIds: AgentId[]; triggerStatuses: AgentTriggerStatus[]; enabled: boolean; delaySeconds: number; sound: ReminderSound; toastEnabled: boolean; windowEnabled: boolean; expectedRevision: Revision | null; }
export interface ReplayReminderDeliveriesInput { consumerId: string; afterDispatchSeq: number; limit: number; }
export interface ReminderReplay { deliveries: ReminderDelivery[]; lastDispatchSeq: number; hasMore: boolean; }
export interface CommitReminderReplayCursorInput { consumerId: string; lastDispatchSeq: number; }
export interface ReminderReplayCursor { consumerId: string; lastDispatchSeq: number; }
export interface ReminderActionMember { id: EntityId; expectedState: ReminderDeliveryState; }
export interface ReminderActionInput { mergeIdentity: ReminderMergeIdentity; expectedMemberDeliveryIds: EntityId[]; members: ReminderActionMember[]; }
export interface SnoozeReminderInput extends ReminderActionInput { snoozedUntil: UnixMillis; }
export interface DeleteReminderRuleInput { id: EntityId; expectedRevision: Revision; }
export interface AcknowledgeReminderNavigationInput { sequence: number; }
export interface AgentStateChangedPayload { agentId: AgentId; environment: AgentEnvironment; sourceEventId: string; occurredAt: UnixMillis; }
export interface AgentProfileStateChangedPayload { profileId: EntityId; sourceEventId: string; occurredAt: UnixMillis; }
export interface ReminderDispatchReadyPayload { dispatchSeq: number; deliveryId: EntityId; }
export interface ReminderNavigationRequestedPayload { sequence: number; }
export type BoundaryListenerState = "active" | "degraded";
export interface ListTodosInput { status: TodoStatus | "all"; limit: number; }
export interface CreateTodoInput { title: string; description: string; dueAt: UnixMillis | null; priority: TodoPriority; }
export interface UpdateTodoInput extends CreateTodoInput { id: EntityId; expectedRevision: Revision; }
export interface CompleteTodoInput { id: EntityId; completed: boolean; expectedRevision: Revision; }
export interface SaveTodoReminderInput { id: EntityId | null; todoId: EntityId; remindAt: UnixMillis; enabled: boolean; expectedRevision: Revision | null; }
export interface ListNotesInput { query: string; limit: number; }
export interface CreateNoteInput { noteDate: LocalDate; bodyMarkdown: string; }
export interface UpdateNoteInput { id: EntityId; noteDate: LocalDate; bodyMarkdown: string; expectedRevision: Revision; }
export interface ExportNoteMarkdownInput { id: EntityId; directory: string; expectedRevision: Revision; }
export interface ExportNoteResult { id: EntityId; path: string; bytesWritten: number; }
export interface ListClipboardItemsInput { query: string; contentKind: ClipboardContentKind | "all"; limit: number; }
export interface SetClipboardPinnedInput { id: EntityId; pinned: boolean; }
export interface ClipboardAssetPayload { assetId: EntityId; mimeType: "image/png"; base64: string; }
export type MediaControlInput = { command: "play" | "pause" | "previous" | "next" } | { command: "seek"; positionSeconds: number } | { command: "setVolume"; volumePercent: number };
export interface ProcessMetric { pid: number; processName: string; cpuPercent: number; memoryBytes: number; sampledAt: UnixMillis; }
export interface SaveProcessWatchInput { id: EntityId | null; processName: string; enabled: boolean; expectedRevision: Revision | null; }
export interface MonitorThreshold { id: EntityId; metric: MonitorMetric; comparator: ThresholdComparator; thresholdValue: number; holdSeconds: number; cooldownSeconds: number; sound: ReminderSound; toastEnabled: boolean; windowEnabled: boolean; enabled: boolean; revision: Revision; updatedAt: UnixMillis; }
export interface SaveMonitorThresholdInput extends Omit<MonitorThreshold, "id" | "revision" | "updatedAt"> { id: EntityId | null; expectedRevision: Revision | null; }
export interface ListNotificationHistoryInput { origin: "all" | "windows" | "aiceland"; sourceApp: string | null; unreadOnly: boolean; limit: number; }
export interface SetNotificationReadInput { id: EntityId; read: boolean; }
export interface DeleteNotificationHistoryInput { id: EntityId; confirmRemoval: true; }
export interface ClearNotificationHistoryInput { before: UnixMillis | null; confirmRemoval: true; }
export type MonitorMetricsChangedPayload = { sampledAt: UnixMillis };
export type NotificationHistoryChangedPayload = { newestReceivedAt: UnixMillis; origin: "windows" | "aiceland" };
export interface AdvanceOnboardingInput { nextStep: OnboardingStep; locale: Locale; modulePreferences: ModulePreference[]; privacyConsent: PrivacyConsent | null; expectedRevision: Revision; }
export interface SetModulePreferenceInput { moduleId: ModuleId; visible: boolean; backgroundEnabled: boolean; expectedRevision: Revision; }
export interface SaveGeneralSettingsInput { launchAtStartup: boolean; expectedRevision: Revision; }
export interface SaveStorageSettingsInput { clipboardRetentionItems: number; notificationRetentionItems: number; markdownExportDirectory: string | null; confirmRetentionRemoval: boolean; expectedRevision: Revision; }

export type MessageUsage = "commandError" | "serviceHealth" | "reminderDisplay" | "uiDisplay";
export type SafeParameterPolicy = "safeScalar" | "agentTriggerStatus" | "monitorMetric" | "number" | { reminderDisplayText: number };
type Contract = Record<string, { usage: MessageUsage; parameters: Partial<Record<SafeParameterName, SafeParameterPolicy>> }>;
const scalar = "safeScalar" as const;
const number = "number" as const;
export const MESSAGE_PARAMETER_CONTRACT: Contract = {
  "errors.invalidInput": { usage: "commandError", parameters: { reasonCode: scalar, entityId: scalar, serviceId: scalar, field: scalar } }, "errors.notFound": { usage: "commandError", parameters: { entityId: scalar } }, "errors.conflict": { usage: "commandError", parameters: { entityId: scalar } }, "errors.storageUnavailable": { usage: "commandError", parameters: { reasonCode: scalar } }, "errors.databaseFailure": { usage: "commandError", parameters: { reasonCode: scalar } }, "errors.ioFailure": { usage: "commandError", parameters: { reasonCode: scalar } }, "errors.permissionDenied": { usage: "commandError", parameters: { reasonCode: scalar } }, "errors.sourceUnavailable": { usage: "commandError", parameters: { serviceId: scalar, reasonCode: scalar } }, "errors.platformUnsupported": { usage: "commandError", parameters: { serviceId: scalar, reasonCode: scalar } }, "errors.integrationUnsupported": { usage: "commandError", parameters: { agentName: scalar, environment: scalar, reasonCode: scalar } }, "errors.integrationNotInstalled": { usage: "commandError", parameters: { agentName: scalar, environment: scalar } }, "errors.integrationConfigInvalid": { usage: "commandError", parameters: { agentName: scalar, environment: scalar, reasonCode: scalar } }, "errors.notificationUnavailable": { usage: "commandError", parameters: { reasonCode: scalar } }, "errors.serviceStopping": { usage: "commandError", parameters: {} }, "settings.storage.retentionConfirmationRequired": { usage: "commandError", parameters: { clipboardRemovalCount: number, notificationRemovalCount: number } }, "services.healthy": { usage: "serviceHealth", parameters: { serviceId: scalar } }, "services.degraded": { usage: "serviceHealth", parameters: { serviceId: scalar, reasonCode: scalar } }, "services.blocked": { usage: "serviceHealth", parameters: { serviceId: scalar, reasonCode: scalar } }, "services.offline": { usage: "serviceHealth", parameters: { serviceId: scalar, reasonCode: scalar } }, "services.clipboard.locked": { usage: "serviceHealth", parameters: { count: number } }, "reminders.agent.status": { usage: "reminderDisplay", parameters: { agentName: { reminderDisplayText: 64 }, environment: scalar, taskId: { reminderDisplayText: 1024 }, taskTitle: { reminderDisplayText: 512 }, triggerStatus: "agentTriggerStatus" } }, "reminders.todo.due": { usage: "reminderDisplay", parameters: { todoTitle: { reminderDisplayText: 512 } } }, "reminders.monitor.threshold": { usage: "reminderDisplay", parameters: { metric: "monitorMetric", currentValue: number, thresholdValue: number } }, "home.agents.more": { usage: "uiDisplay", parameters: { count: number } }, "onboarding.consentRequired": { usage: "commandError", parameters: {} },
};
export const registeredContractMessageKeys = (): string[] => Object.keys(MESSAGE_PARAMETER_CONTRACT).sort();
const pathLike = (value: string): boolean => /^[a-zA-Z]:[\\/]|^\\\\|^\//.test(value);
const sensitive = (value: string): boolean => /(body|token|prompt|tool(?:Input|Output)?|raw\s*xml|audio(?:Path|Content)?)/i.test(value) || /<[^>]+>/.test(value);
export function validateMessageParameters(usage: MessageUsage, key: string, parameters: unknown): parameters is SafeMessageParameters {
  const contract = MESSAGE_PARAMETER_CONTRACT[key];
  if (!contract || contract.usage !== usage || !parameters || typeof parameters !== "object" || Array.isArray(parameters)) return false;
  for (const [name, value] of Object.entries(parameters)) {
    const policy = contract.parameters[name as SafeParameterName];
    if (!policy || !["string", "number", "boolean"].includes(typeof value)) return false;
    if (policy === number && typeof value !== "number") return false;
    if (policy === "agentTriggerStatus" && (typeof value !== "string" || !["completed", "failed", "waiting", "timeout"].includes(value))) return false;
    if (policy === "monitorMetric" && (typeof value !== "string" || !["cpu", "memory", "diskRead", "diskWrite", "networkReceive", "networkSend", "gpu"].includes(value))) return false;
    if (typeof policy === "object" && (typeof value !== "string" || !value || Array.from(value).length > policy.reminderDisplayText || /[\u0000-\u001f\u007f-\u009f]/.test(value))) return false;
    if (policy === scalar && typeof value === "string" && (pathLike(value) || sensitive(value))) return false;
  }
  return true;
}
