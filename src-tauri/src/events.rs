use crate::contracts::{AppErrorCode, CommandError, SafeParameterValue};
use crate::domain::agent_profiles::ValidatedAgentProfileEvent;
use crate::domain::agents::ValidatedAgentEvent;
use tauri::Emitter;

pub const SERVICE_HEALTH_CHANGED: &str = "serviceHealthChanged";
pub const AGENT_STATE_CHANGED: &str = "agentStateChanged";
pub const AGENT_PROFILE_STATE_CHANGED: &str = "agentProfileStateChanged";
pub const REMINDER_DISPATCH_READY: &str = "reminderDispatchReady";
pub const REMINDER_NAVIGATION_REQUESTED: &str = "reminderNavigationRequested";
pub const TO_DO_CHANGED: &str = "todoChanged";
pub const NOTE_CHANGED: &str = "noteChanged";
pub const CLIPBOARD_CHANGED: &str = "clipboardChanged";
pub const MEDIA_SESSION_CHANGED: &str = "mediaSessionChanged";
pub const MONITOR_METRICS_CHANGED: &str = "monitorMetricsChanged";
pub const NOTIFICATION_HISTORY_CHANGED: &str = "notificationHistoryChanged";
pub const FOUNDATION_STORAGE_SERVICE_ID: &str = "foundation-storage";

pub(crate) fn service_health_changed_payload(
    service_id: &str,
    checked_at: i64,
) -> serde_json::Value {
    serde_json::json!({ "serviceId": service_id, "checkedAt": checked_at })
}

pub fn agent_state_changed_payload(event: &ValidatedAgentEvent) -> serde_json::Value {
    serde_json::json!({
        "agentId": event.agent_id,
        "environment": event.environment,
        "sourceEventId": event.event_id,
        "occurredAt": event.occurred_at,
    })
}

pub fn agent_profile_state_changed_payload(
    event: &ValidatedAgentProfileEvent,
) -> serde_json::Value {
    agent_profile_change_payload(
        event.profile_id.as_str(),
        &event.event_id,
        event.occurred_at,
    )
}

pub fn agent_profile_change_payload(
    profile_id: &str,
    source_event_id: &str,
    occurred_at: i64,
) -> serde_json::Value {
    serde_json::json!({
        "profileId": profile_id,
        "sourceEventId": source_event_id,
        "occurredAt": occurred_at,
    })
}

pub fn reminder_dispatch_ready_payload(delivery_id: &str, dispatch_seq: i64) -> serde_json::Value {
    serde_json::json!({ "deliveryId": delivery_id, "dispatchSeq": dispatch_seq })
}

pub fn reminder_navigation_requested_payload(sequence: i64) -> serde_json::Value {
    serde_json::json!({ "sequence": sequence })
}

pub fn todo_changed_payload(entity_id: &str, revision: u64, changed_at: i64) -> serde_json::Value {
    serde_json::json!({ "entityId": entity_id, "revision": revision, "changedAt": changed_at })
}

pub fn note_changed_payload(entity_id: &str, revision: u64, changed_at: i64) -> serde_json::Value {
    serde_json::json!({ "entityId": entity_id, "revision": revision, "changedAt": changed_at })
}

pub fn clipboard_changed_payload(entity_id: &str, changed_at: i64) -> serde_json::Value {
    serde_json::json!({ "entityId": entity_id, "changedAt": changed_at })
}

pub fn media_session_changed_payload(
    session_id: Option<&str>,
    changed_at: i64,
) -> serde_json::Value {
    serde_json::json!({ "sessionId": session_id, "changedAt": changed_at })
}

pub fn monitor_metrics_changed_payload(sampled_at: i64) -> serde_json::Value {
    serde_json::json!({ "sampledAt": sampled_at })
}

pub fn notification_history_changed_payload(
    newest_received_at: i64,
    origin: &str,
) -> serde_json::Value {
    serde_json::json!({ "newestReceivedAt": newest_received_at, "origin": origin })
}
pub fn emit_note_changed(
    app: &tauri::AppHandle,
    entity_id: &str,
    revision: u64,
    changed_at: i64,
) -> Result<(), CommandError> {
    app.emit(
        NOTE_CHANGED,
        note_changed_payload(entity_id, revision, changed_at),
    )
    .map_err(|_| {
        CommandError::with_detail(
            AppErrorCode::SourceUnavailable,
            "errors.sourceUnavailable",
            "reasonCode",
            SafeParameterValue::String("emitFailed".into()),
            false,
        )
    })
}

pub fn emit_clipboard_changed(
    app: &tauri::AppHandle,
    entity_id: &str,
    changed_at: i64,
) -> Result<(), CommandError> {
    app.emit(
        CLIPBOARD_CHANGED,
        clipboard_changed_payload(entity_id, changed_at),
    )
    .map_err(|_| {
        CommandError::with_detail(
            AppErrorCode::SourceUnavailable,
            "errors.sourceUnavailable",
            "reasonCode",
            SafeParameterValue::String("emitFailed".into()),
            false,
        )
    })
}

#[cfg(test)]
pub fn emit_media_session_changed(
    app: &tauri::AppHandle,
    session_id: Option<&str>,
    changed_at: i64,
) -> Result<(), CommandError> {
    app.emit(
        MEDIA_SESSION_CHANGED,
        media_session_changed_payload(session_id, changed_at),
    )
    .map_err(|_| {
        CommandError::with_detail(
            AppErrorCode::SourceUnavailable,
            "errors.sourceUnavailable",
            "reasonCode",
            SafeParameterValue::String("emitFailed".into()),
            false,
        )
    })
}

pub fn emit_service_health_changed(
    app: &tauri::AppHandle,
    service_id: &str,
    checked_at: i64,
) -> Result<(), CommandError> {
    app.emit(
        SERVICE_HEALTH_CHANGED,
        service_health_changed_payload(service_id, checked_at),
    )
    .map_err(|_| {
        CommandError::with_detail(
            AppErrorCode::SourceUnavailable,
            "errors.sourceUnavailable",
            "reasonCode",
            SafeParameterValue::String("emitFailed".into()),
            false,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{
        clipboard_changed_payload, media_session_changed_payload, monitor_metrics_changed_payload,
        note_changed_payload, notification_history_changed_payload,
        reminder_navigation_requested_payload, service_health_changed_payload,
        todo_changed_payload, CLIPBOARD_CHANGED, MEDIA_SESSION_CHANGED, MONITOR_METRICS_CHANGED,
        NOTE_CHANGED, NOTIFICATION_HISTORY_CHANGED, REMINDER_NAVIGATION_REQUESTED,
        SERVICE_HEALTH_CHANGED, TO_DO_CHANGED,
    };

    #[test]
    fn notification_history_hint_has_the_exact_public_shape() {
        assert_eq!(NOTIFICATION_HISTORY_CHANGED, "notificationHistoryChanged");
        assert_eq!(
            notification_history_changed_payload(42, "windows"),
            serde_json::json!({"newestReceivedAt":42,"origin":"windows"})
        );
    }

    #[test]
    fn health_change_payload_has_the_exact_typed_boundary_shape() {
        assert_eq!(SERVICE_HEALTH_CHANGED, "serviceHealthChanged");
        assert_eq!(
            service_health_changed_payload("storage", 42),
            serde_json::json!({ "serviceId": "storage", "checkedAt": 42 })
        );
    }

    #[test]
    fn reminder_navigation_payload_is_only_the_durable_sequence_hint() {
        assert_eq!(REMINDER_NAVIGATION_REQUESTED, "reminderNavigationRequested");
        assert_eq!(
            reminder_navigation_requested_payload(17),
            serde_json::json!({ "sequence": 17 })
        );
    }

    #[test]
    fn todo_change_payload_has_exact_camel_case_keys() {
        assert_eq!(TO_DO_CHANGED, "todoChanged");
        assert_eq!(
            todo_changed_payload("todo-1", 2, 42),
            serde_json::json!({ "entityId": "todo-1", "revision": 2, "changedAt": 42 })
        );
    }

    #[test]
    fn note_change_payload_has_exact_camel_case_keys() {
        assert_eq!(NOTE_CHANGED, "noteChanged");
        assert_eq!(
            note_changed_payload("note-1", 2, 42),
            serde_json::json!({ "entityId": "note-1", "revision": 2, "changedAt": 42 })
        );
    }

    #[test]
    fn clipboard_change_payload_has_exact_small_shape() {
        assert_eq!(CLIPBOARD_CHANGED, "clipboardChanged");
        assert_eq!(
            clipboard_changed_payload("clipboard-1", 42),
            serde_json::json!({ "entityId": "clipboard-1", "changedAt": 42 })
        );
    }

    #[test]
    fn media_session_change_payload_has_exact_small_shape() {
        assert_eq!(MEDIA_SESSION_CHANGED, "mediaSessionChanged");
        assert_eq!(
            media_session_changed_payload(Some("app.session"), 42),
            serde_json::json!({ "sessionId": "app.session", "changedAt": 42 })
        );
        assert_eq!(
            media_session_changed_payload(None, 43),
            serde_json::json!({ "sessionId": null, "changedAt": 43 })
        );
    }

    #[test]
    fn monitor_metrics_change_payload_is_only_the_durable_sample_time_hint() {
        assert_eq!(MONITOR_METRICS_CHANGED, "monitorMetricsChanged");
        assert_eq!(
            monitor_metrics_changed_payload(42),
            serde_json::json!({ "sampledAt": 42 })
        );
    }
}
