use crate::contracts::{
    AppErrorCode, CommandError, CreateNoteInput, DeleteResult, DiagnosticEvent, DiagnosticLevel,
    ExportNoteResult, NoteDateContentSummary, NoteDocument, NoteRecording, NoteRecordingPayload,
    NoteSummary, SafeMessageParameters, SafeParameterValue, UpdateNoteInput,
};
use crate::repositories::note_recordings::NoteRecordingStatus;
use crate::services::AppServices;
use base64::Engine as _;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

#[tauri::command(rename = "listNotes", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn listNotes(
    query: String,
    limit: i64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<Vec<NoteSummary>, CommandError> {
    let limit = u32::try_from(limit).map_err(|_| invalid_input())?;
    services.notes.list(&query, limit)
}

#[tauri::command(rename = "getNote", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn getNote(
    id: Uuid,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<NoteDocument, CommandError> {
    services.notes.get(id)
}

#[tauri::command(rename = "getDailyNote", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn getDailyNote(
    note_date: String,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<Option<NoteDocument>, CommandError> {
    services
        .notes
        .get_daily(&note_date)
        .inspect_err(|error| log_note_command_failure("getDailyNote", error))
}

#[tauri::command(rename = "startNoteRecording", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn startNoteRecording(
    note_date: String,
    mime_type: String,
    file_extension: String,
    started_at: i64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<NoteRecording, CommandError> {
    start_note_recording_with_services(
        note_date,
        mime_type,
        file_extension,
        started_at,
        services.inner().as_ref(),
        now_millis(),
    )
}

#[tauri::command(rename = "appendNoteRecordingChunk", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn appendNoteRecordingChunk(
    id: Uuid,
    chunk: Vec<u8>,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<(), CommandError> {
    append_note_recording_chunk_with_services(id, chunk, services.inner().as_ref())
}

#[tauri::command(rename = "finishNoteRecording", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn finishNoteRecording(
    id: Uuid,
    duration_ms: i64,
    expected_revision: u64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<NoteRecording, CommandError> {
    finish_note_recording_with_services(
        id,
        duration_ms,
        expected_revision,
        services.inner().as_ref(),
        now_millis(),
    )
}

#[tauri::command(rename = "listNoteRecordings", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn listNoteRecordings(
    note_date: String,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<Vec<NoteRecording>, CommandError> {
    list_note_recordings_with_services(note_date, services.inner().as_ref())
}

#[tauri::command(rename = "listNoteContentDates", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn listNoteContentDates(
    start_date: String,
    end_date: String,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<Vec<NoteDateContentSummary>, CommandError> {
    list_note_content_dates_with_services(start_date, end_date, services.inner().as_ref())
}

#[tauri::command(rename = "readNoteRecording", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn readNoteRecording(
    id: Uuid,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<NoteRecordingPayload, CommandError> {
    read_note_recording_with_services(id, services.inner().as_ref())
}

#[tauri::command(rename = "abortNoteRecording", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn abortNoteRecording(
    id: Uuid,
    expected_revision: u64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<DeleteResult, CommandError> {
    abort_note_recording_with_services(id, expected_revision, services.inner().as_ref())
}

#[tauri::command(rename = "deleteNoteRecording", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn deleteNoteRecording(
    id: Uuid,
    expected_revision: u64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<DeleteResult, CommandError> {
    delete_note_recording_with_services(id, expected_revision, services.inner().as_ref())
}

#[tauri::command(rename = "recoverNoteRecordings", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn recoverNoteRecordings(
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<u64, CommandError> {
    recover_note_recordings_with_services(services.inner().as_ref())
}

#[tauri::command(rename = "createNote", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn createNote(
    note_date: String,
    body_markdown: String,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<NoteDocument, CommandError> {
    create_note_with_services(
        CreateNoteInput {
            note_date,
            body_markdown,
        },
        services.inner().as_ref(),
        now_millis(),
    )
    .inspect_err(|error| log_note_command_failure("createNote", error))
}

#[tauri::command(rename = "updateNote", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn updateNote(
    id: Uuid,
    note_date: String,
    body_markdown: String,
    expected_revision: u64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<NoteDocument, CommandError> {
    let expected_revision = i64::try_from(expected_revision).map_err(|_| invalid_input())?;
    update_note_with_services(
        UpdateNoteInput {
            id: id.to_string(),
            note_date,
            body_markdown,
            expected_revision,
        },
        services.inner().as_ref(),
        now_millis(),
    )
    .inspect_err(|error| log_note_command_failure("updateNote", error))
}

fn log_note_command_failure(command: &str, error: &CommandError) {
    log::warn!(
        target: "aisland::notes",
        "event=command_failed command={command} code={:?} message_key={}",
        error.code,
        error.message_key
    );
}

#[tauri::command(rename = "deleteNote", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn deleteNote(
    id: Uuid,
    expected_revision: u64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<DeleteResult, CommandError> {
    delete_note_with_services(
        id,
        expected_revision,
        services.inner().as_ref(),
        now_millis(),
    )
}

#[tauri::command(rename = "exportNoteMarkdown", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn exportNoteMarkdown(
    id: Uuid,
    directory: String,
    expected_revision: u64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<ExportNoteResult, CommandError> {
    export_note_markdown_with_services(
        id,
        directory,
        expected_revision,
        services.inner().as_ref(),
        now_millis(),
    )
}

trait NoteDirectoryOpener {
    fn open(&self, directory: &Path) -> Result<(), CommandError>;
}

struct SystemNoteDirectoryOpener;

#[cfg(windows)]
impl NoteDirectoryOpener for SystemNoteDirectoryOpener {
    fn open(&self, directory: &Path) -> Result<(), CommandError> {
        std::process::Command::new("explorer.exe")
            .arg(directory)
            .spawn()
            .map(|_| ())
            .map_err(|_| io_failure())
    }
}

#[cfg(not(windows))]
impl NoteDirectoryOpener for SystemNoteDirectoryOpener {
    fn open(&self, _directory: &Path) -> Result<(), CommandError> {
        Err(CommandError {
            code: AppErrorCode::PlatformUnsupported,
            message_key: "errors.platformUnsupported".into(),
            details: SafeMessageParameters::new(),
            retryable: false,
        })
    }
}

#[tauri::command(rename = "openNoteDirectory", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn openNoteDirectory(services: tauri::State<'_, Arc<AppServices>>) -> Result<(), CommandError> {
    open_note_directory_with(
        services.markdown_export_directory.as_ref(),
        &SystemNoteDirectoryOpener,
    )
}

fn open_note_directory_with(
    directory_provider: &dyn crate::services::note_export_directory::MarkdownExportDirectoryProvider,
    opener: &dyn NoteDirectoryOpener,
) -> Result<(), CommandError> {
    let directory = directory_provider.default_directory()?;
    opener.open(&directory)
}

fn export_note_markdown_with_services(
    id: Uuid,
    directory: String,
    expected_revision: u64,
    services: &AppServices,
    now: i64,
) -> Result<ExportNoteResult, CommandError> {
    let directory = if directory.is_empty() {
        services.markdown_export_directory.default_directory()?
    } else {
        PathBuf::from(directory)
    };
    let (result, revision) =
        services
            .notes
            .export_markdown(id, &directory, expected_revision, now)?;
    emit_or_record(
        services,
        &result.id,
        i64::try_from(revision).map_err(|_| invalid_input())?,
        now,
    );
    Ok(result)
}

fn create_note_with_services(
    input: CreateNoteInput,
    services: &AppServices,
    now: i64,
) -> Result<NoteDocument, CommandError> {
    let note = services.notes.create(input, now)?;
    emit_or_record(services, &note.id, note.revision, now);
    Ok(note)
}

fn start_note_recording_with_services(
    note_date: String,
    mime_type: String,
    file_extension: String,
    started_at: i64,
    services: &AppServices,
    now: i64,
) -> Result<NoteRecording, CommandError> {
    let id = Uuid::new_v4();
    services
        .note_recording_assets
        .create_temporary(&note_date, id, &file_extension)?;
    let asset_name = format!("{id}.{file_extension}");
    let result = services.note_recordings.start(
        id,
        &note_date,
        &asset_name,
        &mime_type,
        &file_extension,
        started_at,
        now,
    );
    if result.is_err() {
        services
            .note_recording_assets
            .discard_temporary(&note_date, id, &file_extension)?;
    }
    result
}

fn append_note_recording_chunk_with_services(
    id: Uuid,
    chunk: Vec<u8>,
    services: &AppServices,
) -> Result<(), CommandError> {
    let record = services.note_recordings.get_record(id)?;
    if record.status != NoteRecordingStatus::Recording {
        return Err(invalid_input());
    }
    services.note_recording_assets.append_temporary(
        &record.recording.note_date,
        id,
        &record.file_extension,
        &chunk,
    )
}

fn finish_note_recording_with_services(
    id: Uuid,
    duration_ms: i64,
    expected_revision: u64,
    services: &AppServices,
    now: i64,
) -> Result<NoteRecording, CommandError> {
    let record = services.note_recordings.get_record(id)?;
    if record.status != NoteRecordingStatus::Recording || record.recording.revision < 1 {
        return Err(invalid_input());
    }
    let (asset_name, byte_size) = services.note_recording_assets.finalize(
        &record.recording.note_date,
        id,
        &record.file_extension,
    )?;
    if asset_name != record.asset_name {
        services.note_recording_assets.rollback_finalize(
            &record.recording.note_date,
            id,
            &record.file_extension,
        )?;
        return Err(invalid_input());
    }
    let result =
        services
            .note_recordings
            .complete(id, duration_ms, byte_size, expected_revision, now);
    if result.is_err() {
        services.note_recording_assets.rollback_finalize(
            &record.recording.note_date,
            id,
            &record.file_extension,
        )?;
    }
    result
}

fn list_note_recordings_with_services(
    note_date: String,
    services: &AppServices,
) -> Result<Vec<NoteRecording>, CommandError> {
    services.note_recordings.list_completed(&note_date)
}

fn list_note_content_dates_with_services(
    start_date: String,
    end_date: String,
    services: &AppServices,
) -> Result<Vec<NoteDateContentSummary>, CommandError> {
    services
        .note_recordings
        .list_content_dates(&start_date, &end_date)
}

fn read_note_recording_with_services(
    id: Uuid,
    services: &AppServices,
) -> Result<NoteRecordingPayload, CommandError> {
    let record = services.note_recordings.get_record(id)?;
    if record.status != NoteRecordingStatus::Completed {
        return Err(invalid_input());
    }
    let bytes = services
        .note_recording_assets
        .read_completed(&record.recording.note_date, &record.asset_name)?;
    Ok(NoteRecordingPayload {
        id: record.recording.id,
        mime_type: record.recording.mime_type,
        base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

fn abort_note_recording_with_services(
    id: Uuid,
    expected_revision: u64,
    services: &AppServices,
) -> Result<DeleteResult, CommandError> {
    remove_recording_with_services(
        id,
        expected_revision,
        services,
        NoteRecordingStatus::Recording,
    )
}

fn delete_note_recording_with_services(
    id: Uuid,
    expected_revision: u64,
    services: &AppServices,
) -> Result<DeleteResult, CommandError> {
    remove_recording_with_services(
        id,
        expected_revision,
        services,
        NoteRecordingStatus::Completed,
    )
}

fn remove_recording_with_services(
    id: Uuid,
    expected_revision: u64,
    services: &AppServices,
    required_status: NoteRecordingStatus,
) -> Result<DeleteResult, CommandError> {
    let record = services.note_recordings.get_record(id)?;
    if record.status != required_status {
        return Err(invalid_input());
    }
    let staged = match required_status {
        NoteRecordingStatus::Recording => services.note_recording_assets.stage_temporary_deletion(
            &record.recording.note_date,
            id,
            &record.file_extension,
        )?,
        NoteRecordingStatus::Completed => services.note_recording_assets.stage_completed_deletion(
            &record.recording.note_date,
            id,
            &record.file_extension,
        )?,
    };
    match services
        .note_recordings
        .delete(id, expected_revision, required_status)
    {
        Ok(result) => {
            // The row is authoritative once committed. A failed best-effort unlink leaves only
            // an inaccessible staged file, never a completed recording that the UI can address.
            let _ = services
                .note_recording_assets
                .commit_staged_deletion(staged);
            Ok(result)
        }
        Err(error) => {
            services
                .note_recording_assets
                .rollback_staged_deletion(staged)?;
            Err(error)
        }
    }
}

fn recover_note_recordings_with_services(services: &AppServices) -> Result<u64, CommandError> {
    let drafts = services.note_recordings.list_drafts()?;
    let mut removed = 0_u64;
    for draft in drafts {
        let id = Uuid::parse_str(&draft.recording.id).map_err(|_| invalid_input())?;
        abort_note_recording_with_services(id, draft.recording.revision as u64, services)?;
        removed = removed.checked_add(1).ok_or_else(invalid_input)?;
    }
    Ok(removed)
}

fn update_note_with_services(
    input: UpdateNoteInput,
    services: &AppServices,
    now: i64,
) -> Result<NoteDocument, CommandError> {
    let note = services.notes.update(input, now)?;
    emit_or_record(services, &note.id, note.revision, now);
    Ok(note)
}

fn delete_note_with_services(
    id: Uuid,
    expected_revision: u64,
    services: &AppServices,
    changed_at: i64,
) -> Result<DeleteResult, CommandError> {
    let revision = expected_revision.checked_add(1).ok_or_else(invalid_input)?;
    let result = services.notes.delete(id, expected_revision)?;
    emit_or_record(services, &result.id, revision as i64, changed_at);
    Ok(result)
}

fn emit_or_record(services: &AppServices, entity_id: &str, revision: i64, changed_at: i64) {
    let Ok(revision) = u64::try_from(revision) else {
        return;
    };
    if services
        .emit_note_changed(entity_id, revision, changed_at)
        .is_err()
    {
        let _ = services.diagnostics.record(&DiagnosticEvent {
            id: Uuid::new_v4().to_string(),
            service_id: "notes".into(),
            level: DiagnosticLevel::Failure,
            code: "events.noteChangedEmitFailed".into(),
            parameters: BTreeMap::from([(
                "entityId".into(),
                SafeParameterValue::String(entity_id.into()),
            )]),
            created_at: changed_at,
        });
    }
}

fn invalid_input() -> CommandError {
    CommandError {
        code: AppErrorCode::InvalidInput,
        message_key: "errors.invalidInput".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

fn io_failure() -> CommandError {
    CommandError {
        code: AppErrorCode::IoFailure,
        message_key: "errors.ioFailure".into(),
        details: SafeMessageParameters::new(),
        retryable: true,
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
    use super::{
        abortNoteRecording, appendNoteRecordingChunk, append_note_recording_chunk_with_services,
        createNote, create_note_with_services, deleteNote, deleteNoteRecording,
        delete_note_recording_with_services, delete_note_with_services, exportNoteMarkdown,
        export_note_markdown_with_services, finishNoteRecording,
        finish_note_recording_with_services, getDailyNote, getNote, listNoteContentDates,
        listNoteRecordings, listNotes, list_note_content_dates_with_services,
        list_note_recordings_with_services, openNoteDirectory, open_note_directory_with,
        readNoteRecording, read_note_recording_with_services, recoverNoteRecordings,
        recover_note_recordings_with_services, startNoteRecording,
        start_note_recording_with_services, updateNote, update_note_with_services,
        NoteDirectoryOpener,
    };
    use crate::contracts::{
        AppErrorCode, CommandError, CreateNoteInput, SafeMessageParameters, SafeParameterValue,
        UpdateNoteInput,
    };
    use crate::events::NOTE_CHANGED;
    use crate::repositories::notes::NoteRepository;
    use crate::services::{
        note_export_directory::MarkdownExportDirectoryProvider, AppServices,
        BootstrapModuleStateProvider, EventEmitterPort, ModuleStateProvider, ShutdownPort,
        WalCheckpointPort,
    };
    use crate::storage::Storage;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use uuid::Uuid;

    struct NoopShutdown;
    #[async_trait::async_trait]
    impl ShutdownPort for NoopShutdown {
        async fn stop_accepting_work(&self) -> Result<(), CommandError> {
            Ok(())
        }
        async fn stop_optional_modules(&self) -> Result<(), CommandError> {
            Ok(())
        }
        async fn cancel_core_workers(&self) -> Result<(), CommandError> {
            Ok(())
        }
    }

    struct NoopCheckpoint;
    impl WalCheckpointPort for NoopCheckpoint {
        fn checkpoint_truncate(&self) -> Result<(), CommandError> {
            Ok(())
        }
    }

    struct CommitObservingEmitter {
        repository: NoteRepository,
        events: Mutex<Vec<(&'static str, serde_json::Value)>>,
    }
    impl EventEmitterPort for CommitObservingEmitter {
        fn emit(&self, name: &'static str, payload: serde_json::Value) -> Result<(), CommandError> {
            let entity_id = payload["entityId"].as_str().unwrap();
            let revision = payload["revision"].as_i64().unwrap();
            let persisted = self.repository.get(Uuid::parse_str(entity_id).unwrap());
            if revision == 3 {
                assert_eq!(persisted.unwrap_err().code, AppErrorCode::NotFound);
            } else {
                assert_eq!(persisted.unwrap().revision, revision);
            }
            self.events.lock().unwrap().push((name, payload));
            Ok(())
        }
    }

    struct FailingEmitter;
    impl EventEmitterPort for FailingEmitter {
        fn emit(&self, _: &'static str, _: serde_json::Value) -> Result<(), CommandError> {
            Err(CommandError {
                code: AppErrorCode::SourceUnavailable,
                message_key: "errors.sourceUnavailable".into(),
                details: SafeMessageParameters::new(),
                retryable: false,
            })
        }
    }

    struct FixedExportDirectory(PathBuf);

    impl MarkdownExportDirectoryProvider for FixedExportDirectory {
        fn default_directory(&self) -> Result<PathBuf, CommandError> {
            Ok(self.0.clone())
        }
    }

    #[derive(Default)]
    struct RecordingDirectoryOpener {
        opened: Mutex<Vec<PathBuf>>,
    }

    impl NoteDirectoryOpener for RecordingDirectoryOpener {
        fn open(&self, directory: &Path) -> Result<(), CommandError> {
            self.opened.lock().unwrap().push(directory.to_path_buf());
            Ok(())
        }
    }

    fn services(emitter: Arc<dyn EventEmitterPort>) -> Arc<AppServices> {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.keep();
        AppServices::from_parts(
            Arc::new(Storage::open(&path).unwrap()),
            Arc::new(BootstrapModuleStateProvider) as Arc<dyn ModuleStateProvider>,
            Arc::new(NoopShutdown),
            Arc::new(NoopCheckpoint),
            emitter,
        )
    }

    #[test]
    fn recording_chunks_complete_without_creating_a_text_note() {
        use base64::Engine as _;

        let services = services(Arc::new(FailingEmitter));
        let draft = start_note_recording_with_services(
            "2026-08-08".into(),
            "audio/webm;codecs=opus".into(),
            "webm".into(),
            10,
            services.as_ref(),
            10,
        )
        .unwrap();

        append_note_recording_chunk_with_services(
            Uuid::parse_str(&draft.id).unwrap(),
            vec![1, 2],
            services.as_ref(),
        )
        .unwrap();
        append_note_recording_chunk_with_services(
            Uuid::parse_str(&draft.id).unwrap(),
            vec![3, 4],
            services.as_ref(),
        )
        .unwrap();
        let completed = finish_note_recording_with_services(
            Uuid::parse_str(&draft.id).unwrap(),
            1_250,
            1,
            services.as_ref(),
            20,
        )
        .unwrap();

        assert_eq!(completed.byte_size, 4);
        assert_eq!(completed.duration_ms, 1_250);
        assert_eq!(completed.revision, 2);
        assert_eq!(
            list_note_recordings_with_services("2026-08-08".into(), services.as_ref()).unwrap(),
            vec![completed.clone()]
        );
        let payload = read_note_recording_with_services(
            Uuid::parse_str(&completed.id).unwrap(),
            services.as_ref(),
        )
        .unwrap();
        assert_eq!(payload.mime_type, "audio/webm;codecs=opus");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(payload.base64)
                .unwrap(),
            vec![1, 2, 3, 4]
        );
        assert_eq!(services.notes.get_daily("2026-08-08").unwrap(), None);
    }

    #[test]
    fn completed_recordings_delete_and_unfinished_recordings_recover_locally() {
        let services = services(Arc::new(FailingEmitter));
        let draft = start_note_recording_with_services(
            "2026-08-08".into(),
            "audio/webm".into(),
            "webm".into(),
            10,
            services.as_ref(),
            10,
        )
        .unwrap();
        append_note_recording_chunk_with_services(
            Uuid::parse_str(&draft.id).unwrap(),
            vec![1, 2, 3],
            services.as_ref(),
        )
        .unwrap();
        let completed = finish_note_recording_with_services(
            Uuid::parse_str(&draft.id).unwrap(),
            300,
            1,
            services.as_ref(),
            20,
        )
        .unwrap();
        delete_note_recording_with_services(
            Uuid::parse_str(&completed.id).unwrap(),
            completed.revision as u64,
            services.as_ref(),
        )
        .unwrap();
        assert!(
            list_note_recordings_with_services("2026-08-08".into(), services.as_ref())
                .unwrap()
                .is_empty()
        );

        start_note_recording_with_services(
            "2026-08-09".into(),
            "audio/webm".into(),
            "webm".into(),
            30,
            services.as_ref(),
            30,
        )
        .unwrap();
        assert_eq!(
            recover_note_recordings_with_services(services.as_ref()).unwrap(),
            1
        );
        assert_eq!(
            recover_note_recordings_with_services(services.as_ref()).unwrap(),
            0
        );
    }

    #[test]
    fn calendar_content_dates_include_text_and_recording_only_days() {
        let services = services(Arc::new(FailingEmitter));
        services
            .notes
            .create(
                CreateNoteInput {
                    note_date: "2026-08-08".into(),
                    body_markdown: "text".into(),
                },
                10,
            )
            .unwrap();
        let draft = start_note_recording_with_services(
            "2026-08-09".into(),
            "audio/webm".into(),
            "webm".into(),
            20,
            services.as_ref(),
            20,
        )
        .unwrap();
        append_note_recording_chunk_with_services(
            Uuid::parse_str(&draft.id).unwrap(),
            vec![1],
            services.as_ref(),
        )
        .unwrap();
        finish_note_recording_with_services(
            Uuid::parse_str(&draft.id).unwrap(),
            100,
            1,
            services.as_ref(),
            30,
        )
        .unwrap();

        assert_eq!(
            list_note_content_dates_with_services(
                "2026-08-01".into(),
                "2026-08-31".into(),
                services.as_ref(),
            )
            .unwrap()
            .into_iter()
            .map(|item| (item.note_date, item.has_text, item.has_recordings))
            .collect::<Vec<_>>(),
            vec![
                ("2026-08-08".to_string(), true, false),
                ("2026-08-09".to_string(), false, true),
            ]
        );
    }

    #[test]
    fn canonical_manifest_registers_the_controlled_note_directory_command_once() {
        let _ = listNotes;
        let _ = getNote;
        let _ = getDailyNote;
        let _ = startNoteRecording;
        let _ = appendNoteRecordingChunk;
        let _ = finishNoteRecording;
        let _ = listNoteRecordings;
        let _ = listNoteContentDates;
        let _ = readNoteRecording;
        let _ = abortNoteRecording;
        let _ = deleteNoteRecording;
        let _ = recoverNoteRecordings;
        let _ = createNote;
        let _ = updateNote;
        let _ = deleteNote;
        let _ = exportNoteMarkdown;
        let _ = openNoteDirectory;
        assert_eq!(
            crate::commands::NOTE_COMMAND_NAMES,
            [
                "listNotes",
                "getNote",
                "getDailyNote",
                "startNoteRecording",
                "appendNoteRecordingChunk",
                "finishNoteRecording",
                "listNoteRecordings",
                "listNoteContentDates",
                "readNoteRecording",
                "abortNoteRecording",
                "deleteNoteRecording",
                "recoverNoteRecordings",
                "createNote",
                "updateNote",
                "deleteNote",
                "exportNoteMarkdown",
                "openNoteDirectory"
            ]
        );
        let directory = tempfile::tempdir().unwrap();
        let repository = NoteRepository::new(Arc::new(Storage::open(directory.path()).unwrap()));
        assert_eq!(repository.get_daily("2026-08-08").unwrap(), None);

        let source = include_str!("notes.rs");
        for wire_name in crate::commands::NOTE_COMMAND_NAMES {
            let attribute =
                format!("#[tauri::command(rename = \"{wire_name}\", rename_all = \"camelCase\")]");
            assert_eq!(source.matches(&attribute).count(), 1);
        }
    }

    #[test]
    fn open_note_directory_uses_only_the_provider_owned_directory() {
        let directory = tempfile::tempdir().unwrap();
        let expected = directory.path().join("owned-notes");
        fs::create_dir(&expected).unwrap();
        let provider = FixedExportDirectory(expected.clone());
        let opener = RecordingDirectoryOpener::default();

        open_note_directory_with(&provider, &opener).unwrap();

        assert_eq!(*opener.opened.lock().unwrap(), vec![expected]);
    }

    #[test]
    fn export_empty_directory_uses_provider_and_emits_after_revision_commit() {
        let directory = tempfile::tempdir().unwrap();
        let export_directory = directory.path().join("exports");
        fs::create_dir(&export_directory).unwrap();
        let storage = Arc::new(Storage::open(&directory.path().join("database")).unwrap());
        let repository = NoteRepository::new(storage.clone());
        let note = repository
            .create(
                CreateNoteInput {
                    note_date: "2026-08-08".into(),
                    body_markdown: "private markdown".into(),
                },
                10,
            )
            .unwrap();
        let emitter = Arc::new(CommitObservingEmitter {
            repository,
            events: Mutex::new(Vec::new()),
        });
        let services = AppServices::from_parts_with_export_directory(
            storage,
            Arc::new(BootstrapModuleStateProvider) as Arc<dyn ModuleStateProvider>,
            Arc::new(NoopShutdown),
            Arc::new(NoopCheckpoint),
            emitter.clone(),
            Arc::new(FixedExportDirectory(export_directory.clone())),
        );

        let result = export_note_markdown_with_services(
            Uuid::parse_str(&note.id).unwrap(),
            String::new(),
            1,
            &services,
            20,
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&result.path).unwrap(),
            "private markdown"
        );
        assert_eq!(
            *emitter.events.lock().unwrap(),
            vec![(
                NOTE_CHANGED,
                serde_json::json!({ "entityId": note.id, "revision": 2, "changedAt": 20 })
            )]
        );
    }

    #[test]
    fn export_emit_failure_keeps_commit_and_diagnostic_excludes_path_and_markdown() {
        let directory = tempfile::tempdir().unwrap();
        let export_directory = directory.path().join("exports");
        fs::create_dir(&export_directory).unwrap();
        let services = services(Arc::new(FailingEmitter));
        let note = services
            .notes
            .create(
                CreateNoteInput {
                    note_date: "2026-08-08".into(),
                    body_markdown: "private markdown".into(),
                },
                10,
            )
            .unwrap();

        let result = export_note_markdown_with_services(
            Uuid::parse_str(&note.id).unwrap(),
            export_directory.to_string_lossy().into_owned(),
            1,
            &services,
            20,
        )
        .unwrap();

        assert_eq!(
            services
                .notes
                .get(Uuid::parse_str(&note.id).unwrap())
                .unwrap()
                .revision,
            2
        );
        let diagnostics = services.diagnostics.list(10).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].parameters,
            std::collections::BTreeMap::from([(
                "entityId".into(),
                SafeParameterValue::String(note.id),
            )])
        );
        let serialized = serde_json::to_string(&diagnostics).unwrap();
        assert!(!serialized.contains(&result.path));
        assert!(!serialized.contains("private markdown"));
    }

    #[test]
    fn successful_mutations_emit_exact_note_hints_after_repository_commit() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let emitter = Arc::new(CommitObservingEmitter {
            repository: NoteRepository::new(storage.clone()),
            events: Mutex::new(Vec::new()),
        });
        let services = AppServices::from_parts(
            storage,
            Arc::new(BootstrapModuleStateProvider) as Arc<dyn ModuleStateProvider>,
            Arc::new(NoopShutdown),
            Arc::new(NoopCheckpoint),
            emitter.clone(),
        );
        let created = create_note_with_services(
            CreateNoteInput {
                note_date: "2026-08-08".into(),
                body_markdown: "one".into(),
            },
            &services,
            10,
        )
        .unwrap();
        let updated = update_note_with_services(
            UpdateNoteInput {
                id: created.id.clone(),
                note_date: created.note_date.clone(),
                body_markdown: "two".into(),
                expected_revision: 1,
            },
            &services,
            20,
        )
        .unwrap();
        delete_note_with_services(Uuid::parse_str(&created.id).unwrap(), 2, &services, 30).unwrap();

        assert_eq!(updated.revision, 2);
        assert_eq!(
            *emitter.events.lock().unwrap(),
            vec![
                (
                    NOTE_CHANGED,
                    serde_json::json!({ "entityId": created.id, "revision": 1, "changedAt": 10 })
                ),
                (
                    NOTE_CHANGED,
                    serde_json::json!({ "entityId": updated.id, "revision": 2, "changedAt": 20 })
                ),
                (
                    NOTE_CHANGED,
                    serde_json::json!({ "entityId": updated.id, "revision": 3, "changedAt": 30 })
                ),
            ]
        );
        assert_eq!(
            NoteRepository::new(services.storage.clone())
                .get(Uuid::parse_str(&updated.id).unwrap())
                .unwrap_err()
                .code,
            AppErrorCode::NotFound
        );
    }

    #[test]
    fn emit_failure_keeps_commit_and_records_only_entity_id() {
        let services = services(Arc::new(FailingEmitter));
        let created = create_note_with_services(
            CreateNoteInput {
                note_date: "2026-08-08".into(),
                body_markdown: "durable".into(),
            },
            &services,
            42,
        )
        .unwrap();

        assert_eq!(
            NoteRepository::new(services.storage.clone())
                .get(Uuid::parse_str(&created.id).unwrap())
                .unwrap(),
            created
        );
        let diagnostics = services.diagnostics.list(1).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "events.noteChangedEmitFailed");
        assert_eq!(
            diagnostics[0].parameters,
            std::collections::BTreeMap::from([(
                "entityId".into(),
                SafeParameterValue::String(created.id),
            )])
        );
    }
}
