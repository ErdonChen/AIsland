use crate::contracts::{
    AppErrorCode, CommandError, DeleteResult, NoteDateContentSummary, NoteRecording,
    SafeMessageParameters, TrueLiteral,
};
use crate::domain::notes::validate_local_date;
use crate::storage::Storage;
use rusqlite::OptionalExtension;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteRecordingStatus {
    Recording,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteRecordingRecord {
    pub recording: NoteRecording,
    pub asset_name: String,
    pub file_extension: String,
    pub status: NoteRecordingStatus,
}

#[derive(Clone)]
pub struct NoteRecordingRepository {
    storage: Arc<Storage>,
}

impl NoteRecordingRepository {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &self,
        id: Uuid,
        note_date: &str,
        asset_name: &str,
        mime_type: &str,
        file_extension: &str,
        started_at: i64,
        now: i64,
    ) -> Result<NoteRecording, CommandError> {
        validate_local_date(note_date)?;
        validate_media_type(mime_type, file_extension)?;
        if started_at < 0 || now < 0 || asset_name != format!("{id}.{file_extension}") {
            return Err(invalid_input());
        }
        self.storage.with_connection(|connection| {
            connection
                .query_row(
                    r#"INSERT INTO note_recordings(
                           id, note_date, asset_name, mime_type, file_extension,
                           started_at, created_at, updated_at
                       ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
                       RETURNING id, note_date, mime_type, byte_size, started_at,
                                 duration_ms, revision, created_at, updated_at"#,
                    rusqlite::params![
                        id.to_string(),
                        note_date,
                        asset_name,
                        mime_type,
                        file_extension,
                        started_at,
                        now
                    ],
                    row_to_recording,
                )
                .map_err(Into::into)
        })
    }

    pub fn get_record(&self, id: Uuid) -> Result<NoteRecordingRecord, CommandError> {
        self.storage.with_connection(|connection| {
            connection
                .query_row(
                    r#"SELECT id, note_date, mime_type, byte_size, started_at,
                              duration_ms, revision, created_at, updated_at,
                              asset_name, file_extension, status
                       FROM note_recordings WHERE id = ?1"#,
                    [id.to_string()],
                    row_to_record,
                )
                .optional()?
                .ok_or_else(not_found)
        })
    }

    pub fn complete(
        &self,
        id: Uuid,
        duration_ms: i64,
        byte_size: i64,
        expected_revision: u64,
        now: i64,
    ) -> Result<NoteRecording, CommandError> {
        let expected_revision = i64::try_from(expected_revision).map_err(|_| invalid_input())?;
        if duration_ms < 0 || byte_size <= 0 || expected_revision < 1 || now < 0 {
            return Err(invalid_input());
        }
        self.storage.with_transaction(|transaction| {
            let updated = transaction
                .query_row(
                    r#"UPDATE note_recordings
                       SET status = 'completed', byte_size = ?2, duration_ms = ?3,
                           revision = revision + 1, updated_at = ?4
                       WHERE id = ?1 AND status = 'recording' AND revision = ?5
                       RETURNING id, note_date, mime_type, byte_size, started_at,
                                 duration_ms, revision, created_at, updated_at"#,
                    rusqlite::params![
                        id.to_string(),
                        byte_size,
                        duration_ms,
                        now,
                        expected_revision
                    ],
                    row_to_recording,
                )
                .optional()?;
            updated.map_or_else(
                || mutation_miss(transaction, &id.to_string(), NoteRecordingStatus::Recording),
                Ok,
            )
        })
    }

    pub fn list_completed(&self, note_date: &str) -> Result<Vec<NoteRecording>, CommandError> {
        validate_local_date(note_date)?;
        self.storage.with_connection(|connection| {
            let mut statement = connection.prepare(
                r#"SELECT id, note_date, mime_type, byte_size, started_at,
                          duration_ms, revision, created_at, updated_at
                   FROM note_recordings
                   WHERE note_date = ?1 AND status = 'completed'
                   ORDER BY started_at ASC, id ASC"#,
            )?;
            let recordings = statement
                .query_map([note_date], row_to_recording)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(CommandError::from)?;
            Ok(recordings)
        })
    }

    pub fn list_drafts(&self) -> Result<Vec<NoteRecordingRecord>, CommandError> {
        self.storage.with_connection(|connection| {
            let mut statement = connection.prepare(
                r#"SELECT id, note_date, mime_type, byte_size, started_at,
                          duration_ms, revision, created_at, updated_at,
                          asset_name, file_extension, status
                   FROM note_recordings
                   WHERE status = 'recording'
                   ORDER BY created_at ASC, id ASC"#,
            )?;
            let recordings = statement
                .query_map([], row_to_record)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(CommandError::from)?;
            Ok(recordings)
        })
    }

    pub fn list_content_dates(
        &self,
        start_date: &str,
        end_date: &str,
    ) -> Result<Vec<NoteDateContentSummary>, CommandError> {
        validate_local_date(start_date)?;
        validate_local_date(end_date)?;
        if start_date > end_date {
            return Err(invalid_input());
        }
        self.storage.with_connection(|connection| {
            let mut statement = connection.prepare(
                r#"SELECT dates.note_date,
                          EXISTS(SELECT 1 FROM notes WHERE notes.note_date = dates.note_date),
                          EXISTS(SELECT 1 FROM note_recordings
                                 WHERE note_recordings.note_date = dates.note_date
                                   AND note_recordings.status = 'completed')
                   FROM (
                       SELECT note_date FROM notes WHERE note_date BETWEEN ?1 AND ?2
                       UNION
                       SELECT note_date FROM note_recordings
                       WHERE status = 'completed' AND note_date BETWEEN ?1 AND ?2
                   ) AS dates
                   ORDER BY dates.note_date ASC"#,
            )?;
            let dates = statement
                .query_map(rusqlite::params![start_date, end_date], |row| {
                    Ok(NoteDateContentSummary {
                        note_date: row.get(0)?,
                        has_text: row.get(1)?,
                        has_recordings: row.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(CommandError::from)?;
            Ok(dates)
        })
    }

    pub fn delete(
        &self,
        id: Uuid,
        expected_revision: u64,
        required_status: NoteRecordingStatus,
    ) -> Result<DeleteResult, CommandError> {
        let expected_revision = i64::try_from(expected_revision).map_err(|_| invalid_input())?;
        if expected_revision < 1 {
            return Err(invalid_input());
        }
        let status = match required_status {
            NoteRecordingStatus::Recording => "recording",
            NoteRecordingStatus::Completed => "completed",
        };
        self.storage.with_transaction(|transaction| {
            let id_string = id.to_string();
            let deleted = transaction.execute(
                "DELETE FROM note_recordings WHERE id = ?1 AND revision = ?2 AND status = ?3",
                rusqlite::params![id_string, expected_revision, status],
            )?;
            if deleted != 1 {
                return mutation_miss(transaction, &id_string, required_status);
            }
            Ok(DeleteResult {
                id: id_string,
                deleted: TrueLiteral,
            })
        })
    }
}

fn row_to_recording(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteRecording> {
    Ok(NoteRecording {
        id: row.get(0)?,
        note_date: row.get(1)?,
        mime_type: row.get(2)?,
        byte_size: row.get(3)?,
        started_at: row.get(4)?,
        duration_ms: row.get(5)?,
        revision: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteRecordingRecord> {
    let status = match row.get::<_, String>(11)?.as_str() {
        "recording" => NoteRecordingStatus::Recording,
        "completed" => NoteRecordingStatus::Completed,
        value => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                11,
                rusqlite::types::Type::Text,
                format!("invalid recording status: {value}").into(),
            ))
        }
    };
    Ok(NoteRecordingRecord {
        recording: NoteRecording {
            id: row.get(0)?,
            note_date: row.get(1)?,
            mime_type: row.get(2)?,
            byte_size: row.get(3)?,
            started_at: row.get(4)?,
            duration_ms: row.get(5)?,
            revision: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
        },
        asset_name: row.get(9)?,
        file_extension: row.get(10)?,
        status,
    })
}

fn validate_media_type(mime_type: &str, file_extension: &str) -> Result<(), CommandError> {
    let accepted = matches!(
        (mime_type, file_extension),
        ("audio/webm", "webm")
            | ("audio/webm;codecs=opus", "webm")
            | ("audio/ogg", "ogg")
            | ("audio/ogg;codecs=opus", "ogg")
            | ("audio/mp4", "mp4")
    );
    if accepted {
        Ok(())
    } else {
        Err(invalid_input())
    }
}

fn mutation_miss<T>(
    transaction: &rusqlite::Transaction<'_>,
    id: &str,
    required_status: NoteRecordingStatus,
) -> Result<T, CommandError> {
    let status = transaction
        .query_row(
            "SELECT status FROM note_recordings WHERE id = ?1",
            [id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Err(match status {
        None => not_found(),
        Some(value)
            if (value == "recording" && required_status == NoteRecordingStatus::Recording)
                || (value == "completed" && required_status == NoteRecordingStatus::Completed) =>
        {
            conflict()
        }
        Some(_) => conflict(),
    })
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

fn not_found() -> CommandError {
    CommandError {
        code: AppErrorCode::NotFound,
        message_key: "errors.notFound".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}
