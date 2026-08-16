use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type UnixMillis = i64;
pub type EntityId = String;
pub type Revision = i64;
pub type LocalDate = String;
pub type SafeParameterName = String;
pub type SafeMessageParameters = BTreeMap<SafeParameterName, SafeParameterValue>;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Locale {
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "en-US")]
    EnUs,
}

#[cfg(test)]
mod agent_profile_contract_tests {
    use super::*;

    #[test]
    fn save_profile_accepts_the_exact_preset_and_custom_hook_targets() {
        let preset =
            serde_json::from_value::<SaveAgentIntegrationProfileInput>(serde_json::json!({
                "id": "kimi",
                "kind": "preset",
                "displayName": "Kimi Code",
                "environment": "windows",
                "configTarget": {"kind": "preset", "adapterId": "kimi"},
                "eventMapping": [{"nativeEvent": "Notification", "normalizedStatus": "completed"}],
                "enabled": true,
                "expectedRevision": 1
            }))
            .unwrap();
        assert!(matches!(
            preset.config_target,
            AgentConfigTarget::Preset { .. }
        ));

        let custom =
            serde_json::from_value::<SaveAgentIntegrationProfileInput>(serde_json::json!({
                "id": null,
                "kind": "custom",
                "displayName": "My Hook",
                "environment": "windows",
                "configTarget": {
                    "kind": "customHook",
                    "executable": "C:\\tools\\hook.exe",
                    "argv": ["--json"],
                    "workingDirectory": null,
                    "timeoutSeconds": 10
                },
                "eventMapping": [{"nativeEvent": "done", "normalizedStatus": "completed"}],
                "enabled": false,
                "expectedRevision": null
            }))
            .unwrap();
        assert!(matches!(
            custom.config_target,
            AgentConfigTarget::CustomHook { .. }
        ));
    }

    #[test]
    fn profile_actions_require_their_exact_true_literal_fields() {
        for (field, value) in [
            ("confirmInstallation", "install"),
            ("confirmRepair", "repair"),
            ("confirmOwnedRemoval", "uninstall"),
            ("confirmDeletion", "delete"),
        ] {
            let mut payload = serde_json::json!({"id":"kimi","expectedRevision":1});
            payload[field] = serde_json::Value::Bool(true);
            let accepted = match value {
                "install" => {
                    serde_json::from_value::<InstallAgentIntegrationProfileInput>(payload.clone())
                        .is_ok()
                }
                "repair" => {
                    serde_json::from_value::<RepairAgentIntegrationProfileInput>(payload.clone())
                        .is_ok()
                }
                "uninstall" => {
                    serde_json::from_value::<UninstallAgentIntegrationProfileInput>(payload.clone())
                        .is_ok()
                }
                _ => serde_json::from_value::<DeleteAgentIntegrationProfileInput>(payload.clone())
                    .is_ok(),
            };
            assert!(accepted, "true {field} must be accepted");
            payload[field] = serde_json::Value::Bool(false);
            let rejected = match value {
                "install" => {
                    serde_json::from_value::<InstallAgentIntegrationProfileInput>(payload.clone())
                        .is_err()
                }
                "repair" => {
                    serde_json::from_value::<RepairAgentIntegrationProfileInput>(payload.clone())
                        .is_err()
                }
                "uninstall" => {
                    serde_json::from_value::<UninstallAgentIntegrationProfileInput>(payload.clone())
                        .is_err()
                }
                _ => serde_json::from_value::<DeleteAgentIntegrationProfileInput>(payload.clone())
                    .is_err(),
            };
            assert!(rejected, "false {field} must be rejected");
        }
    }
}

macro_rules! boundary_enum { ($name:ident { $($variant:ident),+ $(,)? }) => { #[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)] #[serde(rename_all = "camelCase")] pub enum $name { $($variant),+ } }; }
boundary_enum!(AgentEnvironment { Windows, Wsl });
boundary_enum!(AgentStatus {
    Idle,
    Running,
    Completed,
    Failed,
    Waiting,
    Timeout,
    Offline
});
boundary_enum!(AgentTriggerStatus {
    Completed,
    Failed,
    Waiting,
    Timeout
});
boundary_enum!(IntegrationState {
    NotInstalled,
    Installed,
    NeedsRepair,
    Unsupported
});
boundary_enum!(AgentIntegrationKind { Preset, Custom });
boundary_enum!(PresetAgentAdapterId {
    Kimi,
    Trae,
    Qoderwork,
    Cursor
});
boundary_enum!(AgentIntegrationDiscoveryKind {
    BuiltIn,
    Preset,
    Custom
});
boundary_enum!(AgentIntegrationDiscoveryState {
    Automatic,
    ReadyToInstall,
    DetectionPending,
    AdapterRequired
});
boundary_enum!(AgentIntegrationDiscoveryEvidence {
    RunningProcess,
    Configuration,
    InstalledApplication
});
boundary_enum!(ModuleId {
    Todo,
    Notes,
    Clipboard,
    Media,
    Monitor,
    Notifications
});
boundary_enum!(ServiceHealthState {
    Healthy,
    Degraded,
    Blocked,
    Offline
});
boundary_enum!(ReminderSourceKind {
    Agent,
    Todo,
    Monitor
});
boundary_enum!(ReminderDeliveryState {
    Pending,
    Dispatched,
    Acknowledged,
    Snoozed,
    Cancelled,
    Completed
});
boundary_enum!(TodoPriority { Low, Normal, High });
boundary_enum!(TodoStatus { Open, Completed });
boundary_enum!(ClipboardContentKind { Text, Image });
boundary_enum!(MonitorMetric {
    CpuPercent,
    MemoryPercent,
    DiskReadBytesPerSecond,
    DiskWriteBytesPerSecond,
    NetworkReceiveBytesPerSecond,
    NetworkSendBytesPerSecond,
    GpuPercent
});
boundary_enum!(ThresholdComparator {
    GreaterThanOrEqual,
    LessThanOrEqual
});
boundary_enum!(MediaPlaybackState {
    Playing,
    Paused,
    Stopped,
    Unavailable
});
boundary_enum!(MediaCommand {
    Play,
    Pause,
    Previous,
    Next,
    Seek,
    SetVolume
});
boundary_enum!(OnboardingStep {
    Language,
    Modules,
    Agents,
    Ready
});
boundary_enum!(ThemeMode {
    System,
    Dark,
    Light
});
boundary_enum!(AccentChoice {
    Ice,
    Blue,
    Violet,
    Teal
});
boundary_enum!(AppErrorCode {
    InvalidInput,
    NotFound,
    Conflict,
    StorageUnavailable,
    DatabaseFailure,
    IoFailure,
    PermissionDenied,
    SourceUnavailable,
    PlatformUnsupported,
    IntegrationUnsupported,
    IntegrationNotInstalled,
    IntegrationConfigInvalid,
    NotificationUnavailable
});

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentId {
    Codex,
    Hermes,
    Workbuddy,
    Claude,
}
impl AgentId {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Hermes => "Hermes",
            Self::Workbuddy => "WorkBuddy",
            Self::Claude => "claude",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AgentDisplayName {
    #[serde(rename = "Codex")]
    Codex,
    #[serde(rename = "Hermes")]
    Hermes,
    #[serde(rename = "WorkBuddy")]
    WorkBuddy,
    #[serde(rename = "claude")]
    Claude,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BuiltinReminderSoundId {
    SystemNotification,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ReminderSound {
    None,
    Builtin { sound_id: BuiltinReminderSoundId },
    LocalFile { canonical_path: String },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum SafeParameterValue {
    String(String),
    Number(serde_json::Number),
    Boolean(bool),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageUsage {
    CommandError,
    ServiceHealth,
    ReminderDisplay,
    UiDisplay,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SafeParameterPolicy {
    SafeScalar,
    AgentTriggerStatus,
    MonitorMetric,
    Number,
    ReminderDisplayText { max_chars: usize },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: AppErrorCode,
    pub message_key: String,
    pub details: SafeMessageParameters,
    pub retryable: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceHealthSnapshot {
    pub service_id: String,
    pub state: ServiceHealthState,
    pub message_key: String,
    pub parameters: SafeMessageParameters,
    pub checked_at: UnixMillis,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub locale: Locale,
    pub modules: BTreeMap<ModuleId, ModulePreference>,
    pub services: Vec<ServiceHealthSnapshot>,
    pub storage_schema_version: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentObservation {
    pub agent_id: AgentId,
    pub environment: AgentEnvironment,
    pub task_id: String,
    pub status: AgentStatus,
    pub summary: String,
    pub latest_reply_preview: Option<String>,
    pub source_event_id: String,
    pub occurred_at: UnixMillis,
    pub received_at: UnixMillis,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSummary {
    pub agent_id: AgentId,
    pub display_name: AgentDisplayName,
    pub aggregate_status: AgentStatus,
    pub environments: Vec<AgentObservation>,
    pub integrations: Vec<AgentIntegrationRecord>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentIntegrationRecord {
    pub environment: AgentEnvironment,
    pub supported: bool,
    pub required: bool,
    pub state: IntegrationState,
    pub reason_code: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentsSnapshot {
    pub agents: Vec<AgentSummary>,
    pub generated_at: UnixMillis,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReminderRule {
    pub id: EntityId,
    pub agent_ids: Vec<AgentId>,
    pub trigger_statuses: Vec<AgentTriggerStatus>,
    pub enabled: bool,
    pub delay_seconds: i64,
    pub sound: ReminderSound,
    pub toast_enabled: bool,
    pub window_enabled: bool,
    pub revision: Revision,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReminderDelivery {
    pub id: EntityId,
    pub dedupe_key: String,
    pub rule_id: Option<EntityId>,
    pub source_kind: ReminderSourceKind,
    pub source_entity_id: String,
    pub message_key: String,
    pub message_parameters: SafeMessageParameters,
    pub source_context: ReminderSourceContext,
    pub source_occurred_at: UnixMillis,
    pub sound: ReminderSound,
    pub state: ReminderDeliveryState,
    pub due_at: UnixMillis,
    pub dispatch_seq: i64,
    pub first_dispatched_at: Option<UnixMillis>,
    pub last_dispatched_at: Option<UnixMillis>,
    pub acknowledged_at: Option<UnixMillis>,
    pub completed_at: Option<UnixMillis>,
    pub snoozed_until: Option<UnixMillis>,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ReminderSourceContext {
    Agent {
        agent_id: AgentId,
        environment: AgentEnvironment,
        task_id: String,
        task_title: Option<String>,
        trigger_status: AgentTriggerStatus,
        source_event_id: String,
        source_occurred_at: UnixMillis,
    },
    Todo {
        todo_id: EntityId,
        reminder_revision: Revision,
        todo_title: String,
        source_occurred_at: UnixMillis,
    },
    Monitor {
        threshold_id: EntityId,
        metric: MonitorMetric,
        current_value: i64,
        threshold_value: i64,
        breach_started_at: UnixMillis,
        source_occurred_at: UnixMillis,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReminderAlertGroup {
    pub merge_key: String,
    pub merge_identity: ReminderMergeIdentity,
    pub members: Vec<ReminderDelivery>,
    pub source_context: ReminderSourceContext,
    pub newest_source_occurred_at: UnixMillis,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ReminderMergeIdentity {
    Agent {
        rule_id: EntityId,
        agent_id: AgentId,
        environment: AgentEnvironment,
        task_id: String,
        trigger_status: AgentTriggerStatus,
    },
    Todo {
        todo_id: EntityId,
        reminder_revision: Revision,
        delivery_id: EntityId,
    },
    Monitor {
        threshold_id: EntityId,
        breach_started_at: UnixMillis,
        delivery_id: EntityId,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PendingReminderNavigation {
    pub sequence: i64,
    pub delivery_id: EntityId,
    pub source_kind: ReminderSourceKind,
    pub source_entity_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub id: EntityId,
    pub title: String,
    pub description: String,
    pub due_at: Option<UnixMillis>,
    pub priority: TodoPriority,
    pub status: TodoStatus,
    pub revision: Revision,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
    pub completed_at: Option<UnixMillis>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TodoReminder {
    pub id: EntityId,
    pub todo_id: EntityId,
    pub remind_at: UnixMillis,
    pub enabled: bool,
    pub revision: Revision,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NoteSummary {
    pub id: EntityId,
    pub note_date: LocalDate,
    pub excerpt: String,
    pub revision: Revision,
    pub updated_at: UnixMillis,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NoteDocument {
    pub id: EntityId,
    pub note_date: LocalDate,
    pub body_markdown: String,
    pub revision: Revision,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardItem {
    pub id: EntityId,
    pub content_kind: ClipboardContentKind,
    pub text_content: Option<String>,
    pub asset_id: Option<EntityId>,
    pub source_app: Option<String>,
    pub pinned: bool,
    pub captured_at: UnixMillis,
    pub last_seen_at: UnixMillis,
    pub byte_size: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MediaSnapshot {
    pub session_id: Option<String>,
    pub title: String,
    pub artist: String,
    pub playback_state: MediaPlaybackState,
    pub position_seconds: i64,
    pub duration_seconds: Option<i64>,
    pub volume_percent: Option<i64>,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_previous: bool,
    pub can_next: bool,
    pub can_seek: bool,
    pub can_set_volume: bool,
    pub updated_at: UnixMillis,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MonitorSnapshot {
    pub cpu_percent: i64,
    pub memory_used_bytes: i64,
    pub memory_total_bytes: i64,
    pub disk_read_bytes_per_second: i64,
    pub disk_write_bytes_per_second: i64,
    pub network_receive_bytes_per_second: i64,
    pub network_send_bytes_per_second: i64,
    pub gpu_percent: Option<i64>,
    pub sampled_at: UnixMillis,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessWatch {
    pub id: EntityId,
    pub process_name: String,
    pub enabled: bool,
    pub revision: Revision,
    pub updated_at: UnixMillis,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NotificationHistoryItem {
    pub id: EntityId,
    pub origin: NotificationOrigin,
    pub app_id: String,
    pub source_entity_id: String,
    pub title: String,
    pub body: String,
    pub message_key: Option<String>,
    pub message_parameters: SafeMessageParameters,
    pub source_context: Option<ReminderSourceContext>,
    pub source_occurred_at: UnixMillis,
    pub received_at: UnixMillis,
    pub read_at: Option<UnixMillis>,
}
boundary_enum!(NotificationOrigin { Windows, Aiceland });
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModulePreference {
    pub module_id: ModuleId,
    pub visible: bool,
    pub background_enabled: bool,
    pub revision: Revision,
    pub updated_at: UnixMillis,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GeneralSettings {
    pub launch_at_startup: bool,
    pub revision: Revision,
    pub updated_at: UnixMillis,
}
boundary_enum!(UpdateCheckStatus {
    UpToDate,
    Available
});
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResult {
    pub status: UpdateCheckStatus,
    pub current_version: String,
    pub latest_version: Option<String>,
    pub notes: Option<String>,
}
boundary_enum!(UpdateInstallEventKind {
    Started,
    Progress,
    Finished
});
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInstallEvent {
    pub event: UpdateInstallEventKind,
    pub downloaded: u64,
    pub total: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInstallResult {
    pub installed_version: String,
    pub restart_required: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StorageSettings {
    pub clipboard_retention_items: i64,
    pub notification_retention_items: i64,
    pub markdown_export_directory: Option<String>,
    pub revision: Revision,
    pub updated_at: UnixMillis,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DisplayPreferences {
    pub theme: ThemeMode,
    pub opacity_percent: i64,
    pub accent: AccentChoice,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingState {
    pub current_step: OnboardingStep,
    pub completed: bool,
    pub locale: Locale,
    pub privacy_consent_at: Option<UnixMillis>,
    pub module_preferences: Vec<ModulePreference>,
    pub revision: Revision,
    pub updated_at: UnixMillis,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrivacyConsent {
    pub clipboard_capture: bool,
    pub notification_import: bool,
    pub system_monitoring: bool,
    pub media_session_read: bool,
    pub background_reminders: bool,
}

boundary_enum!(DiagnosticLevel {
    Info,
    Warning,
    Failure
});
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticEvent {
    pub id: EntityId,
    pub service_id: String,
    pub level: DiagnosticLevel,
    pub code: String,
    pub parameters: SafeMessageParameters,
    pub created_at: UnixMillis,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum StorageIntegrity {
    #[serde(rename = "ok")]
    Ok,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StorageIntegrityResult {
    pub integrity: StorageIntegrity,
    pub schema_version: i64,
    pub checked_at: UnixMillis,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrueLiteral;
impl Serialize for TrueLiteral {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(true)
    }
}
impl<'de> Deserialize<'de> for TrueLiteral {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if bool::deserialize(deserializer)? {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom("expected true"))
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeleteResult {
    pub id: EntityId,
    pub deleted: TrueLiteral,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClearResult {
    pub removed_count: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentIntegrationInput {
    pub agent_id: AgentId,
    pub environment: AgentEnvironment,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UninstallAgentIntegrationInput {
    pub agent_id: AgentId,
    pub environment: AgentEnvironment,
    pub confirm_owned_removal: TrueLiteral,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentIntegrationResult {
    pub agent_id: AgentId,
    pub environment: AgentEnvironment,
    pub state: IntegrationState,
    pub config_path: String,
    pub backup_path: Option<String>,
    pub changed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AgentConfigTarget {
    Preset {
        adapter_id: PresetAgentAdapterId,
    },
    CustomHook {
        executable: String,
        argv: Vec<String>,
        working_directory: Option<String>,
        timeout_seconds: i64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentEventMapping {
    pub native_event: String,
    pub normalized_status: AgentStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentIntegrationProfile {
    pub id: String,
    pub kind: AgentIntegrationKind,
    pub display_name: String,
    pub environment: AgentEnvironment,
    pub config_target: AgentConfigTarget,
    pub event_mapping: Vec<AgentEventMapping>,
    pub enabled: bool,
    pub installation_state: IntegrationState,
    pub reason_code: Option<String>,
    pub revision: Revision,
    pub updated_at: UnixMillis,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfileObservation {
    pub profile_id: String,
    pub environment: AgentEnvironment,
    pub task_id: String,
    pub status: AgentStatus,
    pub latest_reply_preview: Option<String>,
    pub source_event_id: String,
    pub occurred_at: UnixMillis,
    pub received_at: UnixMillis,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfileStatusSummary {
    pub profile: AgentIntegrationProfile,
    pub aggregate_status: AgentStatus,
    pub observations: Vec<AgentProfileObservation>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfilesSnapshot {
    pub profiles: Vec<AgentProfileStatusSummary>,
    pub generated_at: UnixMillis,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentIntegrationDiscoveryCandidate {
    pub id: String,
    pub display_name: String,
    pub environment: AgentEnvironment,
    pub integration_kind: AgentIntegrationDiscoveryKind,
    pub state: AgentIntegrationDiscoveryState,
    pub preset_id: Option<PresetAgentAdapterId>,
    pub evidence: Vec<AgentIntegrationDiscoveryEvidence>,
    pub reason_code: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentIntegrationDiscoveryResult {
    pub candidates: Vec<AgentIntegrationDiscoveryCandidate>,
    pub scanned_at: UnixMillis,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SaveAgentIntegrationProfileInput {
    pub id: Option<String>,
    pub kind: AgentIntegrationKind,
    pub display_name: String,
    pub environment: AgentEnvironment,
    pub config_target: AgentConfigTarget,
    pub event_mapping: Vec<AgentEventMapping>,
    pub enabled: bool,
    pub expected_revision: Option<Revision>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstallAgentIntegrationProfileInput {
    pub id: String,
    pub expected_revision: Revision,
    pub confirm_installation: TrueLiteral,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepairAgentIntegrationProfileInput {
    pub id: String,
    pub expected_revision: Revision,
    pub confirm_repair: TrueLiteral,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UninstallAgentIntegrationProfileInput {
    pub id: String,
    pub expected_revision: Revision,
    pub confirm_owned_removal: TrueLiteral,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeleteAgentIntegrationProfileInput {
    pub id: String,
    pub expected_revision: Revision,
    pub confirm_deletion: TrueLiteral,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SaveReminderRuleInput {
    pub id: Option<EntityId>,
    pub agent_ids: Vec<AgentId>,
    pub trigger_statuses: Vec<AgentTriggerStatus>,
    pub enabled: bool,
    pub delay_seconds: i64,
    pub sound: ReminderSound,
    pub toast_enabled: bool,
    pub window_enabled: bool,
    pub expected_revision: Option<Revision>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReplayReminderDeliveriesInput {
    pub consumer_id: String,
    pub after_dispatch_seq: i64,
    pub limit: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReminderReplay {
    pub deliveries: Vec<ReminderDelivery>,
    pub last_dispatch_seq: i64,
    pub has_more: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommitReminderReplayCursorInput {
    pub consumer_id: String,
    pub last_dispatch_seq: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReminderReplayCursor {
    pub consumer_id: String,
    pub last_dispatch_seq: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReminderActionMember {
    pub id: EntityId,
    pub expected_state: ReminderDeliveryState,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReminderActionInput {
    pub merge_identity: ReminderMergeIdentity,
    pub expected_member_delivery_ids: Vec<EntityId>,
    pub members: Vec<ReminderActionMember>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SnoozeReminderInput {
    pub merge_identity: ReminderMergeIdentity,
    pub expected_member_delivery_ids: Vec<EntityId>,
    pub members: Vec<ReminderActionMember>,
    pub snoozed_until: UnixMillis,
}
boundary_enum!(TodoStatusFilter {
    Open,
    Completed,
    All
});
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ListTodosInput {
    pub status: TodoStatusFilter,
    pub limit: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateTodoInput {
    pub title: String,
    pub description: String,
    pub due_at: Option<UnixMillis>,
    pub priority: TodoPriority,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTodoInput {
    pub title: String,
    pub description: String,
    pub due_at: Option<UnixMillis>,
    pub priority: TodoPriority,
    pub id: EntityId,
    pub expected_revision: Revision,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompleteTodoInput {
    pub id: EntityId,
    pub completed: bool,
    pub expected_revision: Revision,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SaveTodoReminderInput {
    pub id: Option<EntityId>,
    pub todo_id: EntityId,
    pub remind_at: UnixMillis,
    pub enabled: bool,
    pub expected_revision: Option<Revision>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ListNotesInput {
    pub query: String,
    pub limit: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateNoteInput {
    pub note_date: LocalDate,
    pub body_markdown: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateNoteInput {
    pub id: EntityId,
    pub note_date: LocalDate,
    pub body_markdown: String,
    pub expected_revision: Revision,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExportNoteMarkdownInput {
    pub id: EntityId,
    pub directory: String,
    pub expected_revision: Revision,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExportNoteResult {
    pub id: EntityId,
    pub path: String,
    pub bytes_written: i64,
}
boundary_enum!(ClipboardContentKindFilter { Text, Image, All });
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ListClipboardItemsInput {
    pub query: String,
    pub content_kind: ClipboardContentKindFilter,
    pub limit: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SetClipboardPinnedInput {
    pub id: EntityId,
    pub pinned: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClipboardAssetMimeType {
    #[serde(rename = "image/png")]
    ImagePng,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardAssetPayload {
    pub asset_id: EntityId,
    pub mime_type: ClipboardAssetMimeType,
    pub base64: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "command",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MediaControlInput {
    Play,
    Pause,
    Previous,
    Next,
    Seek { position_seconds: f64 },
    SetVolume { volume_percent: f64 },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessMetric {
    pub pid: i64,
    pub process_name: String,
    pub cpu_percent: f64,
    pub memory_bytes: i64,
    pub sampled_at: UnixMillis,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SaveProcessWatchInput {
    pub id: Option<EntityId>,
    pub process_name: String,
    pub enabled: bool,
    pub expected_revision: Option<Revision>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MonitorThreshold {
    pub id: EntityId,
    pub metric: MonitorMetric,
    pub comparator: ThresholdComparator,
    pub threshold_value: f64,
    pub hold_seconds: i64,
    pub cooldown_seconds: i64,
    pub sound: ReminderSound,
    pub toast_enabled: bool,
    pub window_enabled: bool,
    pub enabled: bool,
    pub revision: Revision,
    pub updated_at: UnixMillis,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SaveMonitorThresholdInput {
    pub metric: MonitorMetric,
    pub comparator: ThresholdComparator,
    pub threshold_value: f64,
    pub hold_seconds: i64,
    pub cooldown_seconds: i64,
    pub sound: ReminderSound,
    pub toast_enabled: bool,
    pub window_enabled: bool,
    pub enabled: bool,
    pub id: Option<EntityId>,
    pub expected_revision: Option<Revision>,
}
boundary_enum!(NotificationOriginFilter {
    All,
    Windows,
    Aiceland
});
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ListNotificationHistoryInput {
    pub origin: NotificationOriginFilter,
    pub source_app: Option<String>,
    pub unread_only: bool,
    pub limit: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SetNotificationReadInput {
    pub id: EntityId,
    pub read: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AdvanceOnboardingInput {
    pub next_step: OnboardingStep,
    pub locale: Locale,
    pub module_preferences: Vec<ModulePreference>,
    pub privacy_consent: Option<PrivacyConsent>,
    pub expected_revision: Revision,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SetModulePreferenceInput {
    pub module_id: ModuleId,
    pub visible: bool,
    pub background_enabled: bool,
    pub expected_revision: Revision,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SaveGeneralSettingsInput {
    pub launch_at_startup: bool,
    pub expected_revision: Revision,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SaveStorageSettingsInput {
    pub clipboard_retention_items: i64,
    pub notification_retention_items: i64,
    pub markdown_export_directory: Option<String>,
    pub confirm_retention_removal: bool,
    pub expected_revision: Revision,
}

const KEYS: [&str; 25] = [
    "errors.invalidInput",
    "errors.notFound",
    "errors.conflict",
    "errors.storageUnavailable",
    "errors.databaseFailure",
    "errors.ioFailure",
    "errors.permissionDenied",
    "errors.sourceUnavailable",
    "errors.platformUnsupported",
    "errors.integrationUnsupported",
    "errors.integrationNotInstalled",
    "errors.integrationConfigInvalid",
    "errors.notificationUnavailable",
    "errors.serviceStopping",
    "settings.storage.retentionConfirmationRequired",
    "services.healthy",
    "services.degraded",
    "services.blocked",
    "services.offline",
    "services.clipboard.locked",
    "reminders.agent.status",
    "reminders.todo.due",
    "reminders.monitor.threshold",
    "home.agents.more",
    "onboarding.consentRequired",
];
pub struct MessageParameterContract;
impl MessageParameterContract {
    pub fn message_keys() -> Vec<String> {
        let mut keys = KEYS
            .iter()
            .map(|key| (*key).to_string())
            .collect::<Vec<_>>();
        keys.sort();
        keys
    }
    fn usage(key: &str) -> Option<MessageUsage> {
        if key.starts_with("errors.")
            || key.starts_with("settings.")
            || key == "onboarding.consentRequired"
        {
            Some(MessageUsage::CommandError)
        } else if key.starts_with("services.") {
            Some(MessageUsage::ServiceHealth)
        } else if key.starts_with("reminders.") {
            Some(MessageUsage::ReminderDisplay)
        } else if key == "home.agents.more" {
            Some(MessageUsage::UiDisplay)
        } else {
            None
        }
    }
    pub fn policy(key: &str, name: &str) -> Option<(MessageUsage, SafeParameterPolicy)> {
        let usage = Self::usage(key)?;
        let policy = match (key, name) {
            ("errors.invalidInput", "reasonCode" | "entityId" | "serviceId" | "field")
            | ("errors.notFound", "entityId")
            | ("errors.conflict", "entityId" | "reasonCode")
            | (
                "errors.storageUnavailable"
                | "errors.databaseFailure"
                | "errors.ioFailure"
                | "errors.permissionDenied"
                | "errors.notificationUnavailable",
                "reasonCode",
            )
            | (
                "errors.sourceUnavailable" | "errors.platformUnsupported",
                "serviceId" | "reasonCode",
            )
            | (
                "errors.integrationUnsupported" | "errors.integrationConfigInvalid",
                "agentName" | "environment" | "reasonCode",
            )
            | ("errors.integrationNotInstalled", "agentName" | "environment")
            | ("services.healthy", "serviceId")
            | (
                "services.degraded" | "services.blocked" | "services.offline",
                "serviceId" | "reasonCode",
            )
            | ("reminders.agent.status", "environment") => SafeParameterPolicy::SafeScalar,
            (
                "settings.storage.retentionConfirmationRequired",
                "clipboardRemovalCount" | "notificationRemovalCount",
            )
            | ("services.clipboard.locked" | "home.agents.more", "count")
            | ("reminders.monitor.threshold", "currentValue" | "thresholdValue") => {
                SafeParameterPolicy::Number
            }
            ("reminders.agent.status", "agentName") => {
                SafeParameterPolicy::ReminderDisplayText { max_chars: 64 }
            }
            ("reminders.agent.status", "taskId") => {
                SafeParameterPolicy::ReminderDisplayText { max_chars: 1024 }
            }
            ("reminders.agent.status", "taskTitle") | ("reminders.todo.due", "todoTitle") => {
                SafeParameterPolicy::ReminderDisplayText { max_chars: 512 }
            }
            ("reminders.agent.status", "triggerStatus") => SafeParameterPolicy::AgentTriggerStatus,
            ("reminders.monitor.threshold", "metric") => SafeParameterPolicy::MonitorMetric,
            _ => return None,
        };
        Some((usage, policy))
    }
    pub fn validate_for(
        usage: MessageUsage,
        key: &str,
        parameters: &SafeMessageParameters,
    ) -> Result<(), CommandError> {
        if !KEYS.contains(&key) || Self::usage(key) != Some(usage) {
            return Err(contract_error());
        };
        for (name, value) in parameters {
            let Some((_, policy)) = Self::policy(key, name) else {
                return Err(contract_error());
            };
            if !valid(policy, value) {
                return Err(contract_error());
            }
        }
        Ok(())
    }
}
fn contract_error() -> CommandError {
    CommandError {
        code: AppErrorCode::InvalidInput,
        message_key: "errors.invalidInput".into(),
        details: BTreeMap::from([(
            "reasonCode".into(),
            SafeParameterValue::String("messageContractViolation".into()),
        )]),
        retryable: false,
    }
}
fn valid(policy: SafeParameterPolicy, value: &SafeParameterValue) -> bool {
    match policy {
        SafeParameterPolicy::Number => matches!(value, SafeParameterValue::Number(_)),
        SafeParameterPolicy::AgentTriggerStatus => {
            matches!(value,SafeParameterValue::String(s) if ["completed","failed","waiting","timeout"].contains(&s.as_str()))
        }
        SafeParameterPolicy::MonitorMetric => {
            matches!(value,SafeParameterValue::String(s) if ["cpu","memory","diskRead","diskWrite","networkReceive","networkSend","gpu"].contains(&s.as_str()))
        }
        SafeParameterPolicy::ReminderDisplayText { max_chars } => {
            matches!(value,SafeParameterValue::String(s) if !s.is_empty()&&s.chars().count()<=max_chars&&!s.chars().any(|c|c=='\u{7f}'||(c as u32)<=0x1f||((c as u32)>=0x80&&(c as u32)<=0x9f)))
        }
        SafeParameterPolicy::SafeScalar => {
            matches!(value,SafeParameterValue::String(s) if !unsafe_text(s))
                || matches!(
                    value,
                    SafeParameterValue::Number(_) | SafeParameterValue::Boolean(_)
                )
        }
    }
}
fn unsafe_text(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.starts_with('/')
        || value.starts_with("\\\\")
        || (value.as_bytes().get(1) == Some(&b':')
            && value
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic))
        || ["body", "token", "prompt", "tool", "raw xml", "audio"]
            .iter()
            .any(|needle| lower.contains(needle))
        || value.contains('<') && value.contains('>')
}
impl CommandError {
    pub fn new(
        code: AppErrorCode,
        message_key: impl Into<String>,
        details: SafeMessageParameters,
        retryable: bool,
    ) -> Result<Self, CommandError> {
        let message_key = message_key.into();
        MessageParameterContract::validate_for(MessageUsage::CommandError, &message_key, &details)?;
        Ok(Self {
            code,
            message_key,
            details,
            retryable,
        })
    }
    pub fn with_detail(
        code: AppErrorCode,
        message_key: impl Into<String>,
        name: impl Into<String>,
        value: SafeParameterValue,
        retryable: bool,
    ) -> Self {
        let mut details = BTreeMap::new();
        details.insert(name.into(), value);
        Self::new(code, message_key, details, retryable).unwrap_or_else(|error| error)
    }
}
impl From<rusqlite::Error> for CommandError {
    fn from(error: rusqlite::Error) -> Self {
        use rusqlite::ErrorCode;

        let (code, message_key, retryable) = match error {
            rusqlite::Error::SqliteFailure(error, _) => match error.code {
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => (
                    AppErrorCode::StorageUnavailable,
                    "errors.storageUnavailable",
                    true,
                ),
                ErrorCode::ConstraintViolation => {
                    (AppErrorCode::Conflict, "errors.conflict", false)
                }
                _ => (
                    AppErrorCode::DatabaseFailure,
                    "errors.databaseFailure",
                    false,
                ),
            },
            _ => (
                AppErrorCode::DatabaseFailure,
                "errors.databaseFailure",
                false,
            ),
        };
        Self {
            code,
            message_key: message_key.into(),
            details: BTreeMap::new(),
            retryable,
        }
    }
}

#[cfg(test)]
mod contract_mirror_tests {
    use super::*;
    #[test]
    fn serializes_claude_and_camel_case_nested_contracts() {
        assert_eq!(
            serde_json::to_string(&AgentId::Claude).unwrap(),
            "\"claude\""
        );
        assert_eq!(AgentId::Claude.display_name(), "claude");
        let sound = ReminderSound::LocalFile {
            canonical_path: "C:\\sound.wav".into(),
        };
        assert_eq!(
            serde_json::to_value(sound).unwrap(),
            serde_json::json!({ "kind": "localFile", "canonicalPath": "C:\\sound.wav" })
        );
        let context = ReminderSourceContext::Agent {
            agent_id: AgentId::Claude,
            environment: AgentEnvironment::Windows,
            task_id: "task-1".into(),
            task_title: None,
            trigger_status: AgentTriggerStatus::Failed,
            source_event_id: "evt-1".into(),
            source_occurred_at: 12,
        };
        let json = serde_json::to_value(&context).unwrap();
        assert_eq!(json["agentId"], "claude");
        assert_eq!(json["sourceOccurredAt"], 12);
        let round_trip: ReminderSourceContext = serde_json::from_value(json).unwrap();
        assert_eq!(round_trip, context);
    }
    #[test]
    fn locale_is_closed_on_every_boundary_snapshot() {
        assert_eq!(serde_json::to_string(&Locale::ZhCn).unwrap(), "\"zh-CN\"");
        assert_eq!(serde_json::from_str::<Locale>("\"fr-FR\"").is_err(), true);
    }
    #[test]
    fn command_payloads_serialize_exact_boundary_shapes() {
        let event = DiagnosticEvent {
            id: "diag-1".into(),
            service_id: "clipboard".into(),
            level: DiagnosticLevel::Warning,
            code: "locked".into(),
            parameters: BTreeMap::new(),
            created_at: 12,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["serviceId"], "clipboard");
        assert_eq!(json["level"], "warning");
        let round_trip: DiagnosticEvent = serde_json::from_value(json).unwrap();
        assert_eq!(round_trip, event);
        assert!(serde_json::from_str::<DiagnosticLevel>("\"debug\"").is_err());
        let media = MediaControlInput::Seek {
            position_seconds: 3.0,
        };
        assert_eq!(
            serde_json::to_value(media).unwrap(),
            serde_json::json!({"command":"seek","positionSeconds":3.0})
        );
        let integrity = StorageIntegrityResult {
            integrity: StorageIntegrity::Ok,
            schema_version: 1,
            checked_at: 12,
        };
        assert_eq!(serde_json::to_value(integrity).unwrap()["integrity"], "ok");
        assert!(serde_json::from_value::<DeleteResult>(
            serde_json::json!({"id":"x","deleted":false})
        )
        .is_err());
        assert!(serde_json::from_value::<UninstallAgentIntegrationInput>(serde_json::json!({"agentId":"codex","environment":"windows","confirmOwnedRemoval":false})).is_err());
    }
}
