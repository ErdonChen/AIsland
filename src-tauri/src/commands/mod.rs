pub mod agent_profiles;
pub mod agents;
pub mod clipboard;
pub mod foundation;
pub mod monitor;
pub mod notes;
pub mod notifications;
pub mod reminders;
pub mod settings_updates;

macro_rules! foundation_command_manifest {
    ($consumer:ident $($item_terminator:tt)?) => {
        $consumer!(
            getAppSnapshot => get_app_snapshot,
            listServiceHealth => list_service_health,
            getDiagnostics => get_diagnostics,
            checkStorageIntegrity => check_storage_integrity,
        ) $($item_terminator)?
    };
}

pub(crate) use foundation_command_manifest;

macro_rules! note_command_manifest {
    ($consumer:ident; $($prefix:tt)*) => {
        $consumer!(
            $($prefix)*
            ;
            listNotes => listNotes,
            getNote => getNote,
            getDailyNote => getDailyNote,
            startNoteRecording => startNoteRecording,
            appendNoteRecordingChunk => appendNoteRecordingChunk,
            finishNoteRecording => finishNoteRecording,
            listNoteRecordings => listNoteRecordings,
            listNoteContentDates => listNoteContentDates,
            readNoteRecording => readNoteRecording,
            abortNoteRecording => abortNoteRecording,
            deleteNoteRecording => deleteNoteRecording,
            recoverNoteRecordings => recoverNoteRecordings,
            createNote => createNote,
            updateNote => updateNote,
            deleteNote => deleteNote,
            exportNoteMarkdown => exportNoteMarkdown,
            openNoteDirectory => openNoteDirectory,
        )
    };
    ($consumer:ident $($item_terminator:tt)?) => {
        $consumer!(
            listNotes => listNotes,
            getNote => getNote,
            getDailyNote => getDailyNote,
            startNoteRecording => startNoteRecording,
            appendNoteRecordingChunk => appendNoteRecordingChunk,
            finishNoteRecording => finishNoteRecording,
            listNoteRecordings => listNoteRecordings,
            listNoteContentDates => listNoteContentDates,
            readNoteRecording => readNoteRecording,
            abortNoteRecording => abortNoteRecording,
            deleteNoteRecording => deleteNoteRecording,
            recoverNoteRecordings => recoverNoteRecordings,
            createNote => createNote,
            updateNote => updateNote,
            deleteNote => deleteNote,
            exportNoteMarkdown => exportNoteMarkdown,
            openNoteDirectory => openNoteDirectory,
        ) $($item_terminator)?
    };
}

pub(crate) use note_command_manifest;

macro_rules! clipboard_command_manifest {
    ($consumer:ident; $($prefix:tt)*) => {
        $consumer!(
            $($prefix)*
            ;
            listClipboardItems => list_clipboard_items,
            copyClipboardItem => copy_clipboard_item,
            setClipboardPinned => set_clipboard_pinned,
            deleteClipboardItem => delete_clipboard_item,
            clearClipboardHistory => clear_clipboard_history,
            getClipboardAsset => get_clipboard_asset,
        )
    };
    ($consumer:ident $($item_terminator:tt)?) => {
        $consumer!(
            listClipboardItems => list_clipboard_items,
            copyClipboardItem => copy_clipboard_item,
            setClipboardPinned => set_clipboard_pinned,
            deleteClipboardItem => delete_clipboard_item,
            clearClipboardHistory => clear_clipboard_history,
            getClipboardAsset => get_clipboard_asset,
        ) $($item_terminator)?
    };
}

pub(crate) use clipboard_command_manifest;

macro_rules! monitor_command_manifest {
    ($consumer:ident; $($prefix:tt)*) => {
        $consumer!(
            $($prefix)*
            ;
            getMonitorSnapshot => get_monitor_snapshot,
            listMonitorSamples => list_monitor_samples,
            listProcessMetrics => list_process_metrics,
            listProcessWatches => list_process_watches,
            saveProcessWatch => save_process_watch,
            deleteProcessWatch => delete_process_watch,
            listMonitorThresholds => list_monitor_thresholds,
            saveMonitorThreshold => save_monitor_threshold,
            deleteMonitorThreshold => delete_monitor_threshold,
        )
    };
    ($consumer:ident $($item_terminator:tt)?) => {
        $consumer!(
            getMonitorSnapshot => get_monitor_snapshot,
            listMonitorSamples => list_monitor_samples,
            listProcessMetrics => list_process_metrics,
            listProcessWatches => list_process_watches,
            saveProcessWatch => save_process_watch,
            deleteProcessWatch => delete_process_watch,
            listMonitorThresholds => list_monitor_thresholds,
            saveMonitorThreshold => save_monitor_threshold,
            deleteMonitorThreshold => delete_monitor_threshold,
        ) $($item_terminator)?
    };
}

pub(crate) use monitor_command_manifest;

macro_rules! notification_command_manifest {
    ($consumer:ident; $($prefix:tt)*) => {
        $consumer!(
            $($prefix)*
            ;
            listNotificationHistory => list_notification_history,
            setNotificationRead => set_notification_read,
            deleteNotificationHistory => delete_notification_history,
            clearNotificationHistory => clear_notification_history,
        )
    };
    ($consumer:ident $($item_terminator:tt)?) => {
        $consumer!(
            listNotificationHistory => list_notification_history,
            setNotificationRead => set_notification_read,
            deleteNotificationHistory => delete_notification_history,
            clearNotificationHistory => clear_notification_history,
        ) $($item_terminator)?
    };
}

pub(crate) use notification_command_manifest;
macro_rules! command_names {
    ($($wire_name:ident => $implementation:ident),+ $(,)?) => {
        [$(stringify!($wire_name)),+]
    };
}

pub const FOUNDATION_COMMAND_NAMES: [&str; 4] = foundation_command_manifest!(command_names);

pub const AGENT_COMMAND_NAMES: [&str; 4] = [
    "getAgentsSnapshot",
    "installAgentIntegration",
    "repairAgentIntegration",
    "uninstallAgentIntegration",
];

pub const AGENT_PROFILE_COMMAND_NAMES: [&str; 8] = [
    "listAgentIntegrationProfiles",
    "discoverAgentIntegrationCandidates",
    "getAgentProfilesSnapshot",
    "saveAgentIntegrationProfile",
    "installAgentIntegrationProfile",
    "repairAgentIntegrationProfile",
    "uninstallAgentIntegrationProfile",
    "deleteAgentIntegrationProfile",
];

pub const REMINDER_COMMAND_NAMES: [&str; 11] = [
    "listReminderRules",
    "saveReminderRule",
    "deleteReminderRule",
    "replayReminderDeliveries",
    "commitReminderReplayCursor",
    "reloadReminderAlertGroup",
    "acknowledgeReminder",
    "completeReminder",
    "snoozeReminder",
    "getPendingReminderNavigation",
    "acknowledgeReminderNavigation",
];

pub const NOTE_COMMAND_NAMES: [&str; 17] = note_command_manifest!(command_names);
pub const CLIPBOARD_COMMAND_NAMES: [&str; 6] = clipboard_command_manifest!(command_names);
pub const SETTINGS_UPDATE_COMMAND_NAMES: [&str; 4] = [
    "getGeneralSettings",
    "saveGeneralSettings",
    "checkForUpdate",
    "installUpdate",
];
pub const MONITOR_COMMAND_NAMES: [&str; 9] = monitor_command_manifest!(command_names);
pub const NOTIFICATION_COMMAND_NAMES: [&str; 4] = notification_command_manifest!(command_names);

#[cfg(test)]
mod monitor_notification_command_contract_tests {
    use super::{MONITOR_COMMAND_NAMES, NOTIFICATION_COMMAND_NAMES};

    #[test]
    fn manifests_lock_the_exact_thirteen_task_seven_wire_names() {
        assert_eq!(
            MONITOR_COMMAND_NAMES,
            [
                "getMonitorSnapshot",
                "listMonitorSamples",
                "listProcessMetrics",
                "listProcessWatches",
                "saveProcessWatch",
                "deleteProcessWatch",
                "listMonitorThresholds",
                "saveMonitorThreshold",
                "deleteMonitorThreshold",
            ]
        );
        assert_eq!(
            NOTIFICATION_COMMAND_NAMES,
            [
                "listNotificationHistory",
                "setNotificationRead",
                "deleteNotificationHistory",
                "clearNotificationHistory",
            ]
        );
        for (source, names) in [
            (include_str!("monitor.rs"), MONITOR_COMMAND_NAMES.as_slice()),
            (
                include_str!("notifications.rs"),
                NOTIFICATION_COMMAND_NAMES.as_slice(),
            ),
        ] {
            for name in names {
                let attribute =
                    format!("#[tauri::command(rename = \"{name}\", rename_all = \"camelCase\")]");
                assert_eq!(source.matches(&attribute).count(), 1, "{name}");
            }
        }
    }
}

#[cfg(test)]
mod retired_command_contract_tests {
    #[test]
    fn todo_and_media_are_absent_from_the_production_command_manifest_and_handler() {
        let module_source = include_str!("mod.rs");
        let production_source = module_source
            .split("#[cfg(test)]")
            .next()
            .expect("production command module prefix");
        for retired_surface in [
            "pub mod todos;",
            "pub mod media;",
            "todo_command_manifest",
            "todo_reminder_command_manifest",
            "registered_todo_command_manifest",
            "media_command_manifest",
            "TODO_COMMAND_NAMES",
            "TODO_REMINDER_COMMAND_NAMES",
            "MEDIA_COMMAND_NAMES",
        ] {
            assert!(
                !production_source.contains(retired_surface),
                "retired production command surface is still present: {retired_surface}"
            );
        }

        let handler_source = include_str!("../lib.rs");
        for retired_handler in ["commands::todos::", "commands::media::"] {
            assert!(
                !handler_source.contains(retired_handler),
                "retired invoke handler is still present: {retired_handler}"
            );
        }
    }
}
