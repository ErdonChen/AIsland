use crate::contracts::{
    CommandError, GeneralSettings, SaveGeneralSettingsInput, UpdateCheckResult, UpdateInstallEvent,
    UpdateInstallResult,
};
use crate::services::{
    app_updates::{AppUpdateService, UpdateEventSink},
    product_settings::ProductSettingsService,
};
use std::sync::Arc;
use tauri::ipc::Channel;

#[tauri::command(rename = "getGeneralSettings", rename_all = "camelCase")]
pub fn get_general_settings(
    settings: tauri::State<'_, Arc<ProductSettingsService>>,
) -> Result<GeneralSettings, CommandError> {
    settings.get_general(now_millis())
}

#[tauri::command(rename = "saveGeneralSettings", rename_all = "camelCase")]
pub fn save_general_settings(
    launch_at_startup: bool,
    expected_revision: i64,
    settings: tauri::State<'_, Arc<ProductSettingsService>>,
) -> Result<GeneralSettings, CommandError> {
    settings.save_general(
        SaveGeneralSettingsInput {
            launch_at_startup,
            expected_revision,
        },
        now_millis(),
    )
}

#[tauri::command(rename = "checkForUpdate", rename_all = "camelCase")]
pub async fn check_for_update(
    updates: tauri::State<'_, Arc<AppUpdateService>>,
) -> Result<UpdateCheckResult, CommandError> {
    updates.check_for_update().await
}

#[tauri::command(rename = "installUpdate", rename_all = "camelCase")]
pub async fn install_update(
    on_event: Channel<UpdateInstallEvent>,
    updates: tauri::State<'_, Arc<AppUpdateService>>,
) -> Result<UpdateInstallResult, CommandError> {
    updates
        .install_update(Arc::new(ChannelEventSink(on_event)))
        .await
}

struct ChannelEventSink(Channel<UpdateInstallEvent>);

impl UpdateEventSink for ChannelEventSink {
    fn send(&self, event: UpdateInstallEvent) {
        if self.0.send(event).is_err() {
            log::warn!(target: "aisland::updater", "stage=progress_channel status=closed");
        }
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use crate::commands::SETTINGS_UPDATE_COMMAND_NAMES;
    use crate::contracts::{
        UpdateCheckResult, UpdateCheckStatus, UpdateInstallEvent, UpdateInstallEventKind,
        UpdateInstallResult,
    };

    #[test]
    fn manifest_and_source_lock_the_four_exact_camel_case_commands() {
        assert_eq!(
            SETTINGS_UPDATE_COMMAND_NAMES,
            [
                "getGeneralSettings",
                "saveGeneralSettings",
                "checkForUpdate",
                "installUpdate",
            ]
        );
        let source = include_str!("settings_updates.rs");
        for wire_name in SETTINGS_UPDATE_COMMAND_NAMES {
            assert_eq!(
                source
                    .matches(&format!("#[tauri::command(rename = \"{wire_name}\""))
                    .count(),
                1
            );
        }
    }

    #[test]
    fn updater_contracts_serialize_exactly_for_the_frontend_bridge() {
        assert_eq!(
            serde_json::to_value(UpdateCheckResult {
                status: UpdateCheckStatus::Available,
                current_version: "0.1.0".into(),
                latest_version: Some("0.2.0".into()),
                notes: None,
            })
            .unwrap(),
            serde_json::json!({
                "status": "available",
                "currentVersion": "0.1.0",
                "latestVersion": "0.2.0",
                "notes": null,
            })
        );
        assert_eq!(
            serde_json::to_value(UpdateInstallEvent {
                event: UpdateInstallEventKind::Progress,
                downloaded: 5,
                total: Some(10),
            })
            .unwrap(),
            serde_json::json!({"event": "progress", "downloaded": 5, "total": 10})
        );
        assert_eq!(
            serde_json::to_value(UpdateInstallResult {
                installed_version: "0.2.0".into(),
                restart_required: true,
            })
            .unwrap(),
            serde_json::json!({"installedVersion": "0.2.0", "restartRequired": true})
        );
    }
}
