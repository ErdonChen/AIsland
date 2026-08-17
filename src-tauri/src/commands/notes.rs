use crate::contracts::{
    AppErrorCode, CommandError, CreateNoteInput, DeleteResult, DiagnosticEvent, DiagnosticLevel,
    ExportNoteResult, NoteDocument, NoteSummary, SafeMessageParameters, SafeParameterValue,
    UpdateNoteInput,
};
use crate::services::AppServices;
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
        createNote, create_note_with_services, deleteNote, delete_note_with_services,
        exportNoteMarkdown, export_note_markdown_with_services, getDailyNote, getNote, listNotes,
        openNoteDirectory, open_note_directory_with, updateNote, update_note_with_services,
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
    fn canonical_manifest_registers_the_controlled_note_directory_command_once() {
        let _ = listNotes;
        let _ = getNote;
        let _ = getDailyNote;
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
