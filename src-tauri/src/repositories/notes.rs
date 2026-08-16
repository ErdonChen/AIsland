use crate::contracts::{
    AppErrorCode, CommandError, CreateNoteInput, DeleteResult, ExportNoteResult, NoteDocument,
    NoteSummary, SafeMessageParameters, SafeParameterValue, TrueLiteral, UpdateNoteInput,
};
use crate::domain::notes::{note_excerpt, validate_local_date};
use crate::storage::Storage;
use rusqlite::OptionalExtension;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

#[cfg(windows)]
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
#[cfg(not(windows))]
use std::{fs, path::PathBuf};
#[cfg(windows)]
use windows::Win32::{
    Foundation::{GENERIC_WRITE, HANDLE},
    Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, DELETE, FILE_DISPOSITION_INFO,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    },
};

trait MarkdownFileCreator: Send + Sync {
    fn create_new(&self, path: &Path) -> io::Result<Box<dyn CreatedMarkdownFile>>;
}

trait CreatedMarkdownFile: Write + Send {
    fn compensate(&mut self) -> io::Result<()>;
}

struct SystemMarkdownFileCreator;

struct SystemCreatedMarkdownFile {
    file: File,
    #[cfg(not(windows))]
    path: PathBuf,
}

impl Write for SystemCreatedMarkdownFile {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.file.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl CreatedMarkdownFile for SystemCreatedMarkdownFile {
    fn compensate(&mut self) -> io::Result<()> {
        #[cfg(windows)]
        {
            let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
            unsafe {
                SetFileInformationByHandle(
                    HANDLE(self.file.as_raw_handle()),
                    FileDispositionInfo,
                    std::ptr::from_ref(&disposition).cast(),
                    std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
                )
            }
            .map_err(|_| io::Error::other("file compensation failed"))
        }
        #[cfg(not(windows))]
        {
            fs::remove_file(&self.path)
        }
    }
}

impl MarkdownFileCreator for SystemMarkdownFileCreator {
    fn create_new(&self, path: &Path) -> io::Result<Box<dyn CreatedMarkdownFile>> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(windows)]
        options
            .access_mode(GENERIC_WRITE.0 | DELETE.0)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0);
        let file = options.open(path)?;
        Ok(Box::new(SystemCreatedMarkdownFile {
            file,
            #[cfg(not(windows))]
            path: path.to_path_buf(),
        }))
    }
}

#[derive(Clone)]
pub struct NoteRepository {
    storage: Arc<Storage>,
    file_creator: Arc<dyn MarkdownFileCreator>,
}

impl NoteRepository {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self {
            storage,
            file_creator: Arc::new(SystemMarkdownFileCreator),
        }
    }

    #[cfg(test)]
    fn with_file_creator(
        storage: Arc<Storage>,
        file_creator: Arc<dyn MarkdownFileCreator>,
    ) -> Self {
        Self {
            storage,
            file_creator,
        }
    }

    pub fn export_markdown(
        &self,
        id: Uuid,
        directory: &Path,
        expected_revision: u64,
        now: i64,
    ) -> Result<(ExportNoteResult, u64), CommandError> {
        let expected_revision = i64::try_from(expected_revision).map_err(|_| invalid_input())?;
        if expected_revision < 1 || now < 0 {
            return Err(invalid_input());
        }
        let directory = directory.canonicalize().map_err(|_| io_failure())?;
        let id = id.to_string();
        let mut created_file = None;
        let result = self.storage.with_transaction(|transaction| {
            let current = transaction
                .query_row(
                    r#"SELECT note_date, body_markdown, revision, export_history_json
                       FROM notes WHERE id = ?1"#,
                    [&id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()?;
            let Some((note_date, body_markdown, revision, history_json)) = current else {
                return Err(not_found());
            };
            if revision != expected_revision {
                return Err(conflict_for_entity(&id));
            }

            let target = directory.join(format!("{note_date}.md"));
            let path = target.to_str().map(str::to_owned).ok_or_else(io_failure)?;
            let file = match self.file_creator.create_new(&target) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    return Err(conflict_for_entity(&id));
                }
                Err(_) => return Err(io_failure()),
            };
            created_file = Some(file);
            let file = created_file.as_mut().expect("created file was just stored");
            file.write_all(body_markdown.as_bytes())
                .map_err(|_| io_failure())?;
            file.flush().map_err(|_| io_failure())?;

            let mut history = serde_json::from_str::<Vec<serde_json::Value>>(&history_json)
                .map_err(|_| database_failure())?;
            history.push(serde_json::json!({
                "path": path,
                "revision": revision,
                "exportedAt": now,
            }));
            if history.len() > 50 {
                history.drain(..history.len() - 50);
            }
            let history_json = serde_json::to_string(&history).map_err(|_| database_failure())?;
            let next_revision = revision.checked_add(1).ok_or_else(database_failure)?;
            let changed = transaction.execute(
                r#"UPDATE notes
                   SET export_history_json = ?2, revision = ?3, updated_at = ?4
                   WHERE id = ?1 AND revision = ?5"#,
                rusqlite::params![id, history_json, next_revision, now, revision],
            )?;
            if changed != 1 {
                return Err(conflict_for_entity(&id));
            }
            let bytes_written =
                i64::try_from(body_markdown.len()).map_err(|_| database_failure())?;
            let event_revision = u64::try_from(next_revision).map_err(|_| database_failure())?;
            Ok((
                ExportNoteResult {
                    id: id.clone(),
                    path,
                    bytes_written,
                },
                event_revision,
            ))
        });
        if result.is_err() {
            if let Some(mut file) = created_file {
                if file.compensate().is_err() {
                    return Err(io_failure());
                }
            }
        }
        result
    }

    pub fn list(&self, query: &str, limit: u32) -> Result<Vec<NoteSummary>, CommandError> {
        if query.chars().count() > 200 || !(1..=500).contains(&limit) {
            return Err(invalid_input());
        }
        self.storage.with_connection(|connection| {
            let mut statement = connection.prepare(
                r#"SELECT id, note_date, body_markdown, revision, updated_at
                   FROM notes
                   WHERE instr(lower(note_date || char(10) || body_markdown), lower(?1)) > 0
                   ORDER BY updated_at DESC, note_date DESC, id ASC
                   LIMIT ?2"#,
            )?;
            let notes = statement
                .query_map(rusqlite::params![query, limit], |row| {
                    let body = row.get::<_, String>(2)?;
                    Ok(NoteSummary {
                        id: row.get(0)?,
                        note_date: row.get(1)?,
                        excerpt: note_excerpt(&body),
                        revision: row.get(3)?,
                        updated_at: row.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(CommandError::from)?;
            Ok(notes)
        })
    }

    pub fn get(&self, id: Uuid) -> Result<NoteDocument, CommandError> {
        self.storage.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, note_date, body_markdown, revision, created_at, updated_at FROM notes WHERE id = ?1",
                    [id.to_string()],
                    row_to_note,
                )
                .optional()?
                .ok_or_else(not_found)
        })
    }

    pub fn get_daily(&self, note_date: &str) -> Result<Option<NoteDocument>, CommandError> {
        validate_local_date(note_date)?;
        self.storage.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, note_date, body_markdown, revision, created_at, updated_at FROM notes WHERE note_date = ?1",
                    [note_date],
                    row_to_note,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn create(&self, input: CreateNoteInput, now: i64) -> Result<NoteDocument, CommandError> {
        validate_note_fields(&input.note_date, &input.body_markdown, now)?;
        let id = Uuid::new_v4().to_string();
        self.storage.with_transaction(|transaction| {
            let duplicate = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM notes WHERE note_date = ?1)",
                [&input.note_date],
                |row| row.get::<_, bool>(0),
            )?;
            if duplicate {
                return Err(conflict());
            }
            transaction
                .query_row(
                    r#"INSERT INTO notes(id, note_date, body_markdown, created_at, updated_at)
                       VALUES (?1, ?2, ?3, ?4, ?4)
                       RETURNING id, note_date, body_markdown, revision, created_at, updated_at"#,
                    rusqlite::params![id, input.note_date, input.body_markdown, now],
                    row_to_note,
                )
                .map_err(Into::into)
        })
    }

    pub fn update(&self, input: UpdateNoteInput, now: i64) -> Result<NoteDocument, CommandError> {
        validate_note_fields(&input.note_date, &input.body_markdown, now)?;
        if input.expected_revision < 1 || Uuid::parse_str(&input.id).is_err() {
            return Err(invalid_input());
        }
        self.storage.with_transaction(|transaction| {
            let duplicate = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM notes WHERE note_date = ?1 AND id <> ?2)",
                rusqlite::params![input.note_date, input.id],
                |row| row.get::<_, bool>(0),
            )?;
            if duplicate {
                return Err(conflict());
            }
            let updated = transaction
                .query_row(
                    r#"UPDATE notes SET
                         note_date = ?2, body_markdown = ?3,
                         revision = revision + 1, updated_at = ?4
                       WHERE id = ?1 AND revision = ?5
                       RETURNING id, note_date, body_markdown, revision, created_at, updated_at"#,
                    rusqlite::params![
                        input.id,
                        input.note_date,
                        input.body_markdown,
                        now,
                        input.expected_revision
                    ],
                    row_to_note,
                )
                .optional()?;
            updated.map_or_else(|| mutation_miss(transaction, &input.id), Ok)
        })
    }

    pub fn delete(&self, id: Uuid, expected_revision: u64) -> Result<DeleteResult, CommandError> {
        let expected_revision = i64::try_from(expected_revision).map_err(|_| invalid_input())?;
        if expected_revision < 1 {
            return Err(invalid_input());
        }
        let id = id.to_string();
        self.storage.with_transaction(|transaction| {
            let deleted = transaction.execute(
                "DELETE FROM notes WHERE id = ?1 AND revision = ?2",
                rusqlite::params![id, expected_revision],
            )?;
            if deleted == 0 {
                return mutation_miss(transaction, &id);
            }
            Ok(DeleteResult {
                id,
                deleted: TrueLiteral,
            })
        })
    }
}

fn validate_note_fields(
    note_date: &str,
    body_markdown: &str,
    now: i64,
) -> Result<(), CommandError> {
    validate_local_date(note_date)?;
    if now < 0 || body_markdown.chars().count() > 262_144 {
        return Err(invalid_input());
    }
    Ok(())
}

fn row_to_note(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteDocument> {
    Ok(NoteDocument {
        id: row.get(0)?,
        note_date: row.get(1)?,
        body_markdown: row.get(2)?,
        revision: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn mutation_miss<T>(transaction: &rusqlite::Transaction<'_>, id: &str) -> Result<T, CommandError> {
    let exists = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM notes WHERE id = ?1)",
        [id],
        |row| row.get::<_, bool>(0),
    )?;
    Err(if exists { conflict() } else { not_found() })
}

fn invalid_input() -> CommandError {
    CommandError {
        code: AppErrorCode::InvalidInput,
        message_key: "errors.invalidInput".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

fn conflict() -> CommandError {
    CommandError {
        code: AppErrorCode::Conflict,
        message_key: "errors.conflict".into(),
        details: SafeMessageParameters::new(),
        retryable: true,
    }
}

fn conflict_for_entity(entity_id: &str) -> CommandError {
    CommandError {
        code: AppErrorCode::Conflict,
        message_key: "errors.conflict".into(),
        details: SafeMessageParameters::from([(
            "entityId".into(),
            SafeParameterValue::String(entity_id.into()),
        )]),
        retryable: true,
    }
}

fn io_failure() -> CommandError {
    CommandError {
        code: AppErrorCode::IoFailure,
        message_key: "errors.ioFailure".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

fn database_failure() -> CommandError {
    CommandError {
        code: AppErrorCode::DatabaseFailure,
        message_key: "errors.databaseFailure".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

fn not_found() -> CommandError {
    CommandError {
        code: AppErrorCode::NotFound,
        message_key: "errors.notFound".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CreatedMarkdownFile, MarkdownFileCreator, NoteRepository, SystemMarkdownFileCreator,
    };
    use crate::contracts::{AppErrorCode, CreateNoteInput, UpdateNoteInput};
    use crate::storage::Storage;
    use std::fs::{self, File};
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use uuid::Uuid;

    struct FailingWriteFileCreator;

    impl MarkdownFileCreator for FailingWriteFileCreator {
        fn create_new(&self, path: &Path) -> io::Result<Box<dyn CreatedMarkdownFile>> {
            let file = File::options().write(true).create_new(true).open(path)?;
            Ok(Box::new(FailingWriter {
                _file: file,
                path: path.to_path_buf(),
            }))
        }
    }

    struct FailingWriter {
        _file: File,
        path: PathBuf,
    }

    impl CreatedMarkdownFile for FailingWriter {
        fn compensate(&mut self) -> io::Result<()> {
            fs::remove_file(&self.path)
        }
    }

    struct ReplacingFileCreator {
        displaced: PathBuf,
    }

    impl MarkdownFileCreator for ReplacingFileCreator {
        fn create_new(&self, path: &Path) -> io::Result<Box<dyn CreatedMarkdownFile>> {
            let inner = SystemMarkdownFileCreator.create_new(path)?;
            Ok(Box::new(ReplacingCreatedFile {
                inner,
                target: path.to_path_buf(),
                displaced: self.displaced.clone(),
            }))
        }
    }

    struct ReplacingCreatedFile {
        inner: Box<dyn CreatedMarkdownFile>,
        target: PathBuf,
        displaced: PathBuf,
    }

    impl Write for ReplacingCreatedFile {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.inner.write(buffer)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.inner.flush()
        }
    }

    impl CreatedMarkdownFile for ReplacingCreatedFile {
        fn compensate(&mut self) -> io::Result<()> {
            fs::rename(&self.target, &self.displaced)?;
            fs::write(&self.target, "replacement")?;
            self.inner.compensate()
        }
    }

    struct CleanupFailingFileCreator;

    impl MarkdownFileCreator for CleanupFailingFileCreator {
        fn create_new(&self, path: &Path) -> io::Result<Box<dyn CreatedMarkdownFile>> {
            let file = File::options().write(true).create_new(true).open(path)?;
            Ok(Box::new(CleanupFailingCreatedFile { file }))
        }
    }

    struct CleanupFailingCreatedFile {
        file: File,
    }

    impl Write for CleanupFailingCreatedFile {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.file.write(buffer)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.file.flush()
        }
    }

    impl CreatedMarkdownFile for CleanupFailingCreatedFile {
        fn compensate(&mut self) -> io::Result<()> {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "simulated compensation failure",
            ))
        }
    }

    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::WriteZero, "simulated"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn repository() -> NoteRepository {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.keep();
        NoteRepository::new(Arc::new(Storage::open(&path).unwrap()))
    }

    fn create(
        repository: &NoteRepository,
        date: &str,
        body: &str,
        now: i64,
    ) -> crate::contracts::NoteDocument {
        repository
            .create(
                CreateNoteInput {
                    note_date: date.into(),
                    body_markdown: body.into(),
                },
                now,
            )
            .unwrap()
    }

    fn export_fixture() -> (tempfile::TempDir, Arc<Storage>, NoteRepository) {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(&directory.path().join("database")).unwrap());
        let repository = NoteRepository::new(storage.clone());
        (directory, storage, repository)
    }

    fn export_state(storage: &Storage, id: &str) -> (i64, serde_json::Value) {
        storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT revision, export_history_json FROM notes WHERE id = ?1",
                        [id],
                        |row| {
                            let revision = row.get(0)?;
                            let history = row.get::<_, String>(1)?;
                            Ok((revision, history))
                        },
                    )
                    .map_err(Into::into)
            })
            .map(|(revision, history)| (revision, serde_json::from_str(&history).unwrap()))
            .unwrap()
    }

    fn canonical_target(directory: &Path, note_date: &str) -> PathBuf {
        directory
            .canonicalize()
            .unwrap()
            .join(format!("{note_date}.md"))
    }

    fn install_commit_failure(storage: &Storage) {
        storage
            .with_connection(|connection| {
                connection.execute_batch(
                    r#"CREATE TABLE export_commit_parent(id INTEGER PRIMARY KEY);
                       CREATE TABLE export_commit_child(
                           parent_id INTEGER REFERENCES export_commit_parent(id)
                           DEFERRABLE INITIALLY DEFERRED
                       );
                       CREATE TRIGGER fail_export_commit
                       AFTER UPDATE OF export_history_json ON notes
                       BEGIN
                           INSERT INTO export_commit_child(parent_id) VALUES (404);
                       END;"#,
                )?;
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn export_existing_target_conflicts_without_clobber_revision_or_path_leakage() {
        let (directory, storage, repository) = export_fixture();
        let export_directory = directory.path().join("exports");
        fs::create_dir(&export_directory).unwrap();
        let note = create(&repository, "2026-08-08", "private markdown", 10);
        let target = export_directory.join("2026-08-08.md");
        fs::write(&target, "existing").unwrap();

        let error = repository
            .export_markdown(Uuid::parse_str(&note.id).unwrap(), &export_directory, 1, 20)
            .unwrap_err();

        assert_eq!(error.code, AppErrorCode::Conflict);
        assert_eq!(
            error.details,
            crate::contracts::SafeMessageParameters::from([(
                "entityId".into(),
                crate::contracts::SafeParameterValue::String(note.id.clone()),
            )])
        );
        let serialized = serde_json::to_string(&error).unwrap();
        assert!(!serialized.contains(&target.to_string_lossy().to_string()));
        assert!(!serialized.contains("private markdown"));
        assert_eq!(fs::read_to_string(target).unwrap(), "existing");
        assert_eq!(export_state(&storage, &note.id), (1, serde_json::json!([])));
    }

    #[test]
    fn export_success_writes_utf8_appends_history_and_increments_revision() {
        let (directory, storage, repository) = export_fixture();
        let export_directory = directory.path().join("exports");
        fs::create_dir(&export_directory).unwrap();
        let body = "# 日记\n你好 😀";
        let note = create(&repository, "2026-08-08", body, 10);
        let expected_path = canonical_target(&export_directory, "2026-08-08");

        let (result, revision) = repository
            .export_markdown(Uuid::parse_str(&note.id).unwrap(), &export_directory, 1, 20)
            .unwrap();

        assert_eq!(result.id, note.id);
        assert_eq!(result.path, expected_path.to_str().unwrap());
        assert_eq!(result.bytes_written, body.len() as i64);
        assert_eq!(fs::read(&expected_path).unwrap(), body.as_bytes());
        assert_eq!(revision, 2);
        assert_eq!(
            export_state(&storage, &result.id),
            (
                2,
                serde_json::json!([{
                    "path": expected_path.to_str().unwrap(),
                    "revision": 1,
                    "exportedAt": 20
                }])
            )
        );
    }

    #[test]
    fn export_write_failure_removes_partial_file_and_leaves_sqlite_unchanged() {
        let (directory, storage, _) = export_fixture();
        let export_directory = directory.path().join("exports");
        fs::create_dir(&export_directory).unwrap();
        let repository =
            NoteRepository::with_file_creator(storage.clone(), Arc::new(FailingWriteFileCreator));
        let note = create(&repository, "2026-08-08", "durable", 10);
        let target = canonical_target(&export_directory, "2026-08-08");

        let error = repository
            .export_markdown(Uuid::parse_str(&note.id).unwrap(), &export_directory, 1, 20)
            .unwrap_err();

        assert_eq!(error.code, AppErrorCode::IoFailure);
        assert!(!target.exists());
        assert_eq!(export_state(&storage, &note.id), (1, serde_json::json!([])));
    }

    #[test]
    fn export_commit_failure_removes_only_the_file_created_by_the_operation() {
        let (directory, storage, repository) = export_fixture();
        let export_directory = directory.path().join("exports");
        fs::create_dir(&export_directory).unwrap();
        let sentinel = export_directory.join("keep.txt");
        fs::write(&sentinel, "keep").unwrap();
        let note = create(&repository, "2026-08-08", "durable", 10);
        install_commit_failure(&storage);
        let target = canonical_target(&export_directory, "2026-08-08");

        let error = repository
            .export_markdown(Uuid::parse_str(&note.id).unwrap(), &export_directory, 1, 20)
            .unwrap_err();

        assert_eq!(error.code, AppErrorCode::Conflict);
        assert!(!target.exists());
        assert_eq!(fs::read_to_string(sentinel).unwrap(), "keep");
        assert_eq!(export_state(&storage, &note.id), (1, serde_json::json!([])));
    }

    #[test]
    fn export_commit_failure_compensates_created_identity_not_same_path_replacement() {
        let (directory, storage, _) = export_fixture();
        let export_directory = directory.path().join("exports");
        fs::create_dir(&export_directory).unwrap();
        let displaced = export_directory.join("operation-created.md");
        let repository = NoteRepository::with_file_creator(
            storage.clone(),
            Arc::new(ReplacingFileCreator {
                displaced: displaced.clone(),
            }),
        );
        let note = create(&repository, "2026-08-08", "private markdown", 10);
        install_commit_failure(&storage);
        let target = canonical_target(&export_directory, "2026-08-08");

        let error = repository
            .export_markdown(Uuid::parse_str(&note.id).unwrap(), &export_directory, 1, 20)
            .unwrap_err();

        assert_eq!(error.code, AppErrorCode::Conflict);
        assert_eq!(fs::read_to_string(&target).unwrap(), "replacement");
        assert!(!displaced.exists());
        assert_eq!(export_state(&storage, &note.id), (1, serde_json::json!([])));
    }

    #[test]
    fn export_compensation_failure_returns_safe_io_failure_after_database_rollback() {
        let (directory, storage, _) = export_fixture();
        let export_directory = directory.path().join("exports");
        fs::create_dir(&export_directory).unwrap();
        let repository =
            NoteRepository::with_file_creator(storage.clone(), Arc::new(CleanupFailingFileCreator));
        let note = create(&repository, "2026-08-08", "private markdown", 10);
        install_commit_failure(&storage);
        let target = canonical_target(&export_directory, "2026-08-08");

        let error = repository
            .export_markdown(Uuid::parse_str(&note.id).unwrap(), &export_directory, 1, 20)
            .unwrap_err();

        assert_eq!(error.code, AppErrorCode::IoFailure);
        assert!(error.details.is_empty());
        let serialized = serde_json::to_string(&error).unwrap();
        assert!(!serialized.contains(&target.to_string_lossy().to_string()));
        assert!(!serialized.contains("private markdown"));
        assert_eq!(export_state(&storage, &note.id), (1, serde_json::json!([])));
    }

    #[test]
    fn export_history_retains_only_the_newest_fifty_records() {
        let (directory, storage, repository) = export_fixture();
        let note = create(&repository, "2026-08-08", "history", 10);
        for index in 0..51 {
            let export_directory = directory.path().join(format!("export-{index:02}"));
            fs::create_dir(&export_directory).unwrap();
            repository
                .export_markdown(
                    Uuid::parse_str(&note.id).unwrap(),
                    &export_directory,
                    index + 1,
                    20 + index as i64,
                )
                .unwrap();
        }

        let (revision, history) = export_state(&storage, &note.id);
        let history = history.as_array().unwrap();
        assert_eq!(revision, 52);
        assert_eq!(history.len(), 50);
        assert_eq!(history[0]["revision"], 2);
        assert_eq!(history[49]["revision"], 51);
    }

    #[test]
    fn export_stale_revision_conflicts_before_creating_a_file() {
        let (directory, storage, repository) = export_fixture();
        let export_directory = directory.path().join("exports");
        fs::create_dir(&export_directory).unwrap();
        let note = create(&repository, "2026-08-08", "durable", 10);
        let target = canonical_target(&export_directory, "2026-08-08");

        let error = repository
            .export_markdown(Uuid::parse_str(&note.id).unwrap(), &export_directory, 2, 20)
            .unwrap_err();

        assert_eq!(error.code, AppErrorCode::Conflict);
        assert!(!target.exists());
        assert_eq!(export_state(&storage, &note.id), (1, serde_json::json!([])));
    }

    #[test]
    fn absent_daily_note_returns_none() {
        assert_eq!(repository().get_daily("2026-08-08").unwrap(), None);
    }

    #[test]
    fn duplicate_note_date_conflicts_without_replacing_the_original() {
        let repository = repository();
        let original = create(&repository, "2026-08-08", "first", 10);
        let error = repository
            .create(
                CreateNoteInput {
                    note_date: "2026-08-08".into(),
                    body_markdown: "second".into(),
                },
                20,
            )
            .unwrap_err();
        assert_eq!(error.code, AppErrorCode::Conflict);
        assert_eq!(
            repository
                .get(Uuid::parse_str(&original.id).unwrap())
                .unwrap()
                .body_markdown,
            "first"
        );
    }

    #[test]
    fn stale_update_conflicts_and_leaves_revision_two_body_unchanged() {
        let repository = repository();
        let created = create(&repository, "2026-08-08", "revision one", 10);
        let revision_two = repository
            .update(
                UpdateNoteInput {
                    id: created.id.clone(),
                    note_date: created.note_date.clone(),
                    body_markdown: "revision two".into(),
                    expected_revision: 1,
                },
                20,
            )
            .unwrap();
        let error = repository
            .update(
                UpdateNoteInput {
                    id: created.id.clone(),
                    note_date: created.note_date,
                    body_markdown: "stale overwrite".into(),
                    expected_revision: 1,
                },
                30,
            )
            .unwrap_err();
        assert_eq!(error.code, AppErrorCode::Conflict);
        assert_eq!(
            repository
                .get(Uuid::parse_str(&created.id).unwrap())
                .unwrap(),
            revision_two
        );
    }

    #[test]
    fn search_is_literal_case_insensitive_deterministic_and_uses_unicode_excerpts() {
        let repository = repository();
        let older = create(&repository, "2026-08-08", "Needle 100%_literal", 10);
        let newer_date = create(
            &repository,
            "2026-08-10",
            &format!("needle\n\t{} tail", "界".repeat(170)),
            20,
        );
        let lower_id = create(&repository, "2026-08-09", "needle third", 20);

        let matches = repository.list("NEEDLE", 10).unwrap();
        assert_eq!(
            matches
                .iter()
                .map(|note| note.id.as_str())
                .collect::<Vec<_>>(),
            [
                newer_date.id.as_str(),
                lower_id.id.as_str(),
                older.id.as_str()
            ]
        );
        assert_eq!(matches[0].excerpt.chars().count(), 160);
        assert!(!matches[0].excerpt.contains('\n'));
        assert_eq!(
            repository
                .list("100%_LITERAL", 10)
                .unwrap()
                .iter()
                .map(|note| note.id.as_str())
                .collect::<Vec<_>>(),
            [older.id.as_str()]
        );
        assert_eq!(
            repository
                .list("2026-08-09", 10)
                .unwrap()
                .iter()
                .map(|note| note.id.as_str())
                .collect::<Vec<_>>(),
            [lower_id.id.as_str()]
        );
    }

    #[test]
    fn unicode_scalar_and_list_bounds_are_enforced_without_byte_counting() {
        let repository = repository();
        let valid_body = "😀".repeat(262_144);
        let created = create(&repository, "2028-02-29", &valid_body, 10);
        assert_eq!(created.body_markdown.chars().count(), 262_144);

        for (body, date) in [
            ("😀".repeat(262_145), "2026-08-08"),
            (String::new(), "2026-02-30"),
        ] {
            assert_eq!(
                repository
                    .create(
                        CreateNoteInput {
                            note_date: date.into(),
                            body_markdown: body
                        },
                        20
                    )
                    .unwrap_err()
                    .code,
                AppErrorCode::InvalidInput
            );
        }
        assert_eq!(repository.list(&"界".repeat(200), 1).unwrap(), Vec::new());
        for (query, limit) in [
            (&"界".repeat(201), 1),
            (&String::new(), 0),
            (&String::new(), 501),
        ] {
            assert_eq!(
                repository.list(query, limit).unwrap_err().code,
                AppErrorCode::InvalidInput
            );
        }
    }

    #[test]
    fn duplicate_date_update_and_stale_delete_leave_rows_unchanged() {
        let repository = repository();
        let first = create(&repository, "2026-08-08", "first", 10);
        let second = create(&repository, "2026-08-09", "second", 20);
        let duplicate = repository
            .update(
                UpdateNoteInput {
                    id: second.id.clone(),
                    note_date: first.note_date.clone(),
                    body_markdown: "replacement".into(),
                    expected_revision: 1,
                },
                30,
            )
            .unwrap_err();
        assert_eq!(duplicate.code, AppErrorCode::Conflict);
        assert_eq!(
            repository
                .get(Uuid::parse_str(&second.id).unwrap())
                .unwrap(),
            second
        );

        let updated = repository
            .update(
                UpdateNoteInput {
                    id: first.id.clone(),
                    note_date: first.note_date,
                    body_markdown: "updated".into(),
                    expected_revision: 1,
                },
                40,
            )
            .unwrap();
        assert_eq!(
            repository
                .delete(Uuid::parse_str(&first.id).unwrap(), 1)
                .unwrap_err()
                .code,
            AppErrorCode::Conflict
        );
        assert_eq!(
            repository.get(Uuid::parse_str(&first.id).unwrap()).unwrap(),
            updated
        );
    }
}
