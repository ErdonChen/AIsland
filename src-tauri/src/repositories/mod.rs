pub mod agents;
pub mod app_settings;
pub mod clipboard;
pub mod diagnostics;
pub mod monitor;
pub mod notes;
pub mod notifications;
pub mod reminders;
pub mod service_health;
pub mod todos;

use crate::contracts::{
    CommandError, MessageParameterContract, MessageUsage, SafeMessageParameters, SafeParameterName,
};
use std::sync::OnceLock;

pub type MessageContractViolation = CommandError;

pub fn allowed_diagnostic_parameters(code: &str) -> Option<&'static [SafeParameterName]> {
    static STORAGE_INTEGRITY_FAILED: OnceLock<Vec<SafeParameterName>> = OnceLock::new();
    static SERVICE_HEALTH_EMIT_FAILED: OnceLock<Vec<SafeParameterName>> = OnceLock::new();
    static INTEGRATION_ROLLBACK_FAILED: OnceLock<Vec<SafeParameterName>> = OnceLock::new();
    static WATCHER_REGISTRATION_FAILED: OnceLock<Vec<SafeParameterName>> = OnceLock::new();
    static WATCHER_FILE_INVALID: OnceLock<Vec<SafeParameterName>> = OnceLock::new();
    static TODO_CHANGED_EMIT_FAILED: OnceLock<Vec<SafeParameterName>> = OnceLock::new();
    static NOTE_CHANGED_EMIT_FAILED: OnceLock<Vec<SafeParameterName>> = OnceLock::new();
    static TODO_REMINDER_PROJECTION_FAILED: OnceLock<Vec<SafeParameterName>> = OnceLock::new();
    static CLIPBOARD_CAPTURE_TOO_LARGE: OnceLock<Vec<SafeParameterName>> = OnceLock::new();
    static CLIPBOARD_READ_FAILED: OnceLock<Vec<SafeParameterName>> = OnceLock::new();
    static CLIPBOARD_CHANGED_EMIT_FAILED: OnceLock<Vec<SafeParameterName>> = OnceLock::new();
    static CLIPBOARD_ASSET_CLEANUP_FAILED: OnceLock<Vec<SafeParameterName>> = OnceLock::new();
    static MONITOR_SAMPLE_FAILED: OnceLock<Vec<SafeParameterName>> = OnceLock::new();
    static MONITOR_METRICS_CHANGED_EMIT_FAILED: OnceLock<Vec<SafeParameterName>> = OnceLock::new();
    static MONITOR_PROCESS_SKIPPED: OnceLock<Vec<SafeParameterName>> = OnceLock::new();
    static MONITOR_THRESHOLD_CANCELLATION_FAILED: OnceLock<Vec<SafeParameterName>> =
        OnceLock::new();
    static NOTIFICATION_SYNC_FAILED: OnceLock<Vec<SafeParameterName>> = OnceLock::new();
    static NOTIFICATION_HISTORY_EMIT_FAILED: OnceLock<Vec<SafeParameterName>> = OnceLock::new();

    match code {
        "storage.integrityFailed" => Some(
            STORAGE_INTEGRITY_FAILED.get_or_init(|| vec!["serviceId".into(), "reasonCode".into()]),
        ),
        "events.serviceHealthEmitFailed" => Some(
            SERVICE_HEALTH_EMIT_FAILED
                .get_or_init(|| vec!["serviceId".into(), "reasonCode".into(), "count".into()]),
        ),
        "integration.rollbackFailed" => Some(INTEGRATION_ROLLBACK_FAILED.get_or_init(|| {
            vec![
                "agentName".into(),
                "environment".into(),
                "reasonCode".into(),
            ]
        })),
        "watcher.registrationFailed" => Some(
            WATCHER_REGISTRATION_FAILED
                .get_or_init(|| vec!["serviceId".into(), "reasonCode".into()]),
        ),
        "watcher.fileInvalid" | "reminder.enqueueFailed" => {
            Some(WATCHER_FILE_INVALID.get_or_init(|| {
                vec![
                    "agentName".into(),
                    "environment".into(),
                    "fileNameHash".into(),
                    "reasonCode".into(),
                    "receivedAt".into(),
                ]
            }))
        }
        "events.todoChangedEmitFailed" => {
            Some(TODO_CHANGED_EMIT_FAILED.get_or_init(|| vec!["entityId".into()]))
        }
        "events.noteChangedEmitFailed" => {
            Some(NOTE_CHANGED_EMIT_FAILED.get_or_init(|| vec!["entityId".into()]))
        }
        "todo.reminderProjectionFailed" => Some(
            TODO_REMINDER_PROJECTION_FAILED
                .get_or_init(|| vec!["todoId".into(), "reminderId".into()]),
        ),
        "clipboard.captureTooLarge" => Some(
            CLIPBOARD_CAPTURE_TOO_LARGE.get_or_init(|| vec!["kind".into(), "byteCount".into()]),
        ),
        "clipboard.readFailed" => {
            Some(CLIPBOARD_READ_FAILED.get_or_init(|| vec!["reasonCode".into()]))
        }
        "events.clipboardChangedEmitFailed" => {
            Some(CLIPBOARD_CHANGED_EMIT_FAILED.get_or_init(|| vec!["entityId".into()]))
        }
        "clipboard.assetCleanupFailed" => {
            Some(CLIPBOARD_ASSET_CLEANUP_FAILED.get_or_init(|| vec!["reasonCode".into()]))
        }
        "monitor.sampleFailed" => Some(
            MONITOR_SAMPLE_FAILED
                .get_or_init(|| vec!["metric".into(), "reasonCode".into(), "sampledAt".into()]),
        ),
        "events.monitorMetricsChangedEmitFailed" => Some(
            MONITOR_METRICS_CHANGED_EMIT_FAILED
                .get_or_init(|| vec!["metric".into(), "reasonCode".into(), "sampledAt".into()]),
        ),
        "monitor.processSkipped" => Some(
            MONITOR_PROCESS_SKIPPED.get_or_init(|| vec!["watchId".into(), "skippedCount".into()]),
        ),
        "monitor.thresholdCancellationFailed" => Some(
            MONITOR_THRESHOLD_CANCELLATION_FAILED
                .get_or_init(|| vec!["thresholdId".into(), "reasonCode".into()]),
        ),
        "notifications.syncFailed" => Some(NOTIFICATION_SYNC_FAILED.get_or_init(|| {
            vec![
                "source".into(),
                "reasonCode".into(),
                "rowCount".into(),
                "cursor".into(),
                "checkedAt".into(),
            ]
        })),
        "events.notificationHistoryChangedEmitFailed" => {
            Some(NOTIFICATION_HISTORY_EMIT_FAILED.get_or_init(|| {
                vec![
                    "source".into(),
                    "reasonCode".into(),
                    "rowCount".into(),
                    "cursor".into(),
                    "checkedAt".into(),
                ]
            }))
        }
        _ => None,
    }
}

pub fn validate_message_parameters(
    message_key: &str,
    parameters: &SafeMessageParameters,
) -> Result<(), MessageContractViolation> {
    MessageParameterContract::validate_for(MessageUsage::ServiceHealth, message_key, parameters)
}

#[cfg(test)]
mod note_diagnostic_contract_tests {
    #[test]
    fn note_emit_failure_allowlist_is_closed_to_entity_id() {
        assert_eq!(
            super::allowed_diagnostic_parameters("events.noteChangedEmitFailed"),
            Some(["entityId".to_string()].as_slice())
        );
        assert_eq!(
            super::allowed_diagnostic_parameters("events.noteChangedEmitFailed.extra"),
            None
        );
    }

    #[test]
    fn clipboard_diagnostic_allowlists_are_exact_and_closed() {
        assert_eq!(
            super::allowed_diagnostic_parameters("clipboard.captureTooLarge"),
            Some(["kind".to_string(), "byteCount".to_string()].as_slice())
        );
        assert_eq!(
            super::allowed_diagnostic_parameters("clipboard.readFailed"),
            Some(["reasonCode".to_string()].as_slice())
        );
        assert_eq!(
            super::allowed_diagnostic_parameters("events.clipboardChangedEmitFailed"),
            Some(["entityId".to_string()].as_slice())
        );
        assert_eq!(
            super::allowed_diagnostic_parameters("clipboard.assetCleanupFailed"),
            Some(["reasonCode".to_string()].as_slice())
        );
        assert_eq!(
            super::allowed_diagnostic_parameters("clipboard.captureTooLarge.extra"),
            None
        );
    }

    #[test]
    fn monitor_diagnostic_allowlists_are_exact_and_closed() {
        let expected = [
            "metric".to_string(),
            "reasonCode".to_string(),
            "sampledAt".to_string(),
        ];
        assert_eq!(
            super::allowed_diagnostic_parameters("monitor.sampleFailed"),
            Some(expected.as_slice())
        );
        assert_eq!(
            super::allowed_diagnostic_parameters("events.monitorMetricsChangedEmitFailed"),
            Some(expected.as_slice())
        );
        assert_eq!(
            super::allowed_diagnostic_parameters("monitor.processSkipped"),
            Some(["watchId".to_string(), "skippedCount".to_string()].as_slice())
        );
        assert_eq!(
            super::allowed_diagnostic_parameters("monitor.thresholdCancellationFailed"),
            Some(["thresholdId".to_string(), "reasonCode".to_string()].as_slice())
        );
        assert_eq!(
            super::allowed_diagnostic_parameters("monitor.sampleFailed.extra"),
            None
        );
        assert_eq!(
            super::allowed_diagnostic_parameters("monitor.thresholdCancellationFailed.extra"),
            None
        );
    }

    #[test]
    fn notification_sync_diagnostic_allowlists_are_exact_and_closed() {
        let expected = [
            "source".to_string(),
            "reasonCode".to_string(),
            "rowCount".to_string(),
            "cursor".to_string(),
            "checkedAt".to_string(),
        ];
        assert_eq!(
            super::allowed_diagnostic_parameters("notifications.syncFailed"),
            Some(expected.as_slice())
        );
        assert_eq!(
            super::allowed_diagnostic_parameters("events.notificationHistoryChangedEmitFailed"),
            Some(expected.as_slice())
        );
        assert_eq!(
            super::allowed_diagnostic_parameters("notifications.syncFailed.extra"),
            None
        );
    }
}
pub mod agent_profiles;
