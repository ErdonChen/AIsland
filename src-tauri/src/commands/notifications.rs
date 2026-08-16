use crate::contracts::{
    AppErrorCode, ClearResult, CommandError, DeleteResult, ListNotificationHistoryInput,
    NotificationHistoryItem, NotificationOriginFilter, SafeParameterValue,
};
use crate::repositories::notifications::NotificationRepository;
use crate::services::AppServices;
use std::sync::Arc;
use uuid::Uuid;

#[tauri::command(rename = "listNotificationHistory", rename_all = "camelCase")]
pub fn list_notification_history(
    origin: NotificationOriginFilter,
    source_app: Option<String>,
    unread_only: bool,
    limit: i64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<Vec<NotificationHistoryItem>, CommandError> {
    let input = history_input(origin, source_app, unread_only, limit)?;
    services.notifications.list(input)
}

#[tauri::command(rename = "setNotificationRead", rename_all = "camelCase")]
pub fn set_notification_read(
    id: Uuid,
    read: bool,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<NotificationHistoryItem, CommandError> {
    services.notifications.set_read(id, read, now_millis())
}

#[tauri::command(rename = "deleteNotificationHistory", rename_all = "camelCase")]
pub fn delete_notification_history(
    id: Uuid,
    confirm_removal: Option<bool>,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<DeleteResult, CommandError> {
    delete_notification_history_with(&services.notifications, id, confirm_removal, now_millis())
}

#[tauri::command(rename = "clearNotificationHistory", rename_all = "camelCase")]
pub fn clear_notification_history(
    before: Option<i64>,
    confirm_removal: Option<bool>,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<ClearResult, CommandError> {
    clear_notification_history_with(
        &services.notifications,
        before,
        confirm_removal,
        now_millis(),
    )
}

fn history_input(
    origin: NotificationOriginFilter,
    source_app: Option<String>,
    unread_only: bool,
    limit: i64,
) -> Result<ListNotificationHistoryInput, CommandError> {
    validate_history_limit(limit)?;
    if source_app.as_ref().is_some_and(|value| value.is_empty()) {
        return Err(invalid_input("sourceApp"));
    }
    Ok(ListNotificationHistoryInput {
        origin,
        source_app,
        unread_only,
        limit,
    })
}

fn delete_notification_history_with(
    repository: &NotificationRepository,
    id: Uuid,
    confirm_removal: Option<bool>,
    now: i64,
) -> Result<DeleteResult, CommandError> {
    require_confirmation(confirm_removal)?;
    repository.mark_removed(id, now)
}

fn clear_notification_history_with(
    repository: &NotificationRepository,
    before: Option<i64>,
    confirm_removal: Option<bool>,
    now: i64,
) -> Result<ClearResult, CommandError> {
    require_confirmation(confirm_removal)?;
    validate_before(before)?;
    repository.clear(before, now)
}

fn validate_history_limit(limit: i64) -> Result<(), CommandError> {
    if !(1..=500).contains(&limit) {
        return Err(invalid_input("historyLimit"));
    }
    Ok(())
}

fn require_confirmation(confirm_removal: Option<bool>) -> Result<(), CommandError> {
    if confirm_removal != Some(true) {
        return Err(invalid_input("confirmRemoval"));
    }
    Ok(())
}

fn validate_before(before: Option<i64>) -> Result<(), CommandError> {
    if before.is_some_and(|value| value < 0) {
        return Err(invalid_input("before"));
    }
    Ok(())
}

fn invalid_input(reason: &str) -> CommandError {
    CommandError::with_detail(
        AppErrorCode::InvalidInput,
        "errors.invalidInput",
        "reasonCode",
        SafeParameterValue::String(reason.into()),
        false,
    )
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repositories::notifications::{
        ImportedNotification, NotificationCursor, NotificationOrigin,
    };
    use crate::storage::Storage;

    fn fixture() -> (tempfile::TempDir, NotificationRepository, Uuid) {
        let directory = tempfile::tempdir().unwrap();
        let repository =
            NotificationRepository::new(Arc::new(Storage::open(directory.path()).unwrap()));
        repository
            .import(
                &[ImportedNotification {
                    origin: NotificationOrigin::Windows,
                    app_id: "app".into(),
                    source_entity_id: "source-1".into(),
                    source_row_id: Some(1),
                    title: Some("Title".into()),
                    body: Some("Body".into()),
                    message_key: None,
                    message_parameters: None,
                    source_context: None,
                    source_occurred_at: 10,
                    received_at: 20,
                }],
                NotificationCursor {
                    source_id: "windowsWpn".into(),
                    last_row_id: 1,
                    last_updated_at: 10,
                },
                20,
            )
            .unwrap();
        let id = Uuid::parse_str(
            &repository
                .list(ListNotificationHistoryInput {
                    origin: NotificationOriginFilter::All,
                    source_app: None,
                    unread_only: false,
                    limit: 10,
                })
                .unwrap()[0]
                .id,
        )
        .unwrap();
        (directory, repository, id)
    }

    #[test]
    fn history_bounds_and_destructive_confirmation_are_literal() {
        assert!(validate_history_limit(1).is_ok());
        assert!(validate_history_limit(500).is_ok());
        assert_eq!(
            validate_history_limit(0).unwrap_err().code,
            AppErrorCode::InvalidInput
        );
        assert_eq!(
            validate_history_limit(501).unwrap_err().code,
            AppErrorCode::InvalidInput
        );
        assert!(require_confirmation(Some(true)).is_ok());
        assert_eq!(
            require_confirmation(Some(false)).unwrap_err().code,
            AppErrorCode::InvalidInput
        );
        assert_eq!(
            require_confirmation(None).unwrap_err().code,
            AppErrorCode::InvalidInput
        );
        assert!(validate_before(None).is_ok());
        assert!(validate_before(Some(0)).is_ok());
        assert_eq!(
            validate_before(Some(-1)).unwrap_err().code,
            AppErrorCode::InvalidInput
        );
    }

    #[test]
    fn rejected_removal_confirmation_leaves_history_unchanged() {
        let (_directory, repository, id) = fixture();
        assert_eq!(
            delete_notification_history_with(&repository, id, Some(false), 30)
                .unwrap_err()
                .code,
            AppErrorCode::InvalidInput
        );
        assert_eq!(
            clear_notification_history_with(&repository, None, None, 30)
                .unwrap_err()
                .code,
            AppErrorCode::InvalidInput
        );
        assert_eq!(
            repository
                .list(ListNotificationHistoryInput {
                    origin: NotificationOriginFilter::All,
                    source_app: None,
                    unread_only: false,
                    limit: 10,
                })
                .unwrap()
                .len(),
            1
        );
    }
}
