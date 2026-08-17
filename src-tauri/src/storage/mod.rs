use crate::contracts::{AppErrorCode, CommandError, SafeMessageParameters, SafeParameterValue};
use rusqlite::{Connection, ErrorCode, Transaction, TransactionBehavior};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const DATABASE_FILE_NAME: &str = "aisland.sqlite3";

#[derive(Clone, Copy)]
struct Migration {
    version: u32,
    name: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "foundation",
        sql: include_str!("migrations/0001_foundation.sql"),
    },
    Migration {
        version: 2,
        name: "builtin_agents_reminders",
        sql: include_str!("migrations/0002_builtin_agents_reminders.sql"),
    },
    Migration {
        version: 3,
        name: "todo_notes",
        sql: include_str!("migrations/0003_todo_notes.sql"),
    },
    Migration {
        version: 4,
        name: "clipboard_media",
        sql: include_str!("migrations/0004_clipboard_media.sql"),
    },
    Migration {
        version: 5,
        name: "monitor_notifications",
        sql: include_str!("migrations/0005_monitor_notifications.sql"),
    },
    Migration {
        version: 6,
        name: "agent_integration_profiles",
        sql: include_str!("migrations/0006_agent_integration_profiles.sql"),
    },
    Migration {
        version: 7,
        name: "retain_threshold_breach_history",
        sql: include_str!("migrations/0007_retain_threshold_breach_history.sql"),
    },
    Migration {
        version: 8,
        name: "agent_attention_statuses",
        sql: include_str!("migrations/0008_agent_attention_statuses.sql"),
    },
    Migration {
        version: 9,
        name: "agent_profile_reply_previews",
        sql: include_str!("migrations/0009_agent_profile_reply_previews.sql"),
    },
    Migration {
        version: 10,
        name: "cursor_hook_profile",
        sql: include_str!("migrations/0010_cursor_hook_profile.sql"),
    },
];

pub struct Storage {
    path: PathBuf,
    connection: Mutex<Connection>,
}

impl Storage {
    pub fn open(directory: &Path) -> Result<Self, CommandError> {
        fs::create_dir_all(directory).map_err(map_io_error)?;
        let path = directory.join(DATABASE_FILE_NAME);
        let mut connection = Connection::open(&path).map_err(map_sqlite_error)?;
        connection
            .execute_batch("PRAGMA busy_timeout=5000;\nPRAGMA foreign_keys=ON;")
            .map_err(map_sqlite_error)?;
        run_migrations(&mut connection)?;
        connection
            .execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(map_sqlite_error)?;

        Ok(Self {
            path,
            connection: Mutex::new(connection),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn schema_version(&self) -> Result<u32, CommandError> {
        self.with_connection(|connection| {
            let version: i64 = connection
                .query_row(
                    "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                    [],
                    |row| row.get(0),
                )
                .map_err(map_sqlite_error)?;
            u32::try_from(version).map_err(|_| database_failure())
        })
    }

    pub fn integrity_check(&self) -> Result<(), CommandError> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare("PRAGMA integrity_check")
                .map_err(map_sqlite_error)?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(map_sqlite_error)?;
            let rows = rows
                .map(|row| row.map_err(map_sqlite_error))
                .collect::<Result<Vec<_>, _>>()?;
            validate_integrity_rows(&rows)
        })
    }

    pub fn with_connection<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        let connection = self.connection.lock().map_err(|_| database_failure())?;
        operation(&connection)
    }

    pub fn with_transaction<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        let mut connection = self.connection.lock().map_err(|_| database_failure())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_error)?;
        let value = operation(&transaction)?;
        transaction.commit().map_err(map_sqlite_error)?;
        Ok(value)
    }
}

fn run_migrations(connection: &mut Connection) -> Result<(), CommandError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_error)?;
    apply_pending_migrations(&transaction)?;
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(())
}

fn apply_pending_migrations(transaction: &Transaction<'_>) -> Result<(), CommandError> {
    let mut applied_versions = BTreeSet::new();

    if schema_migrations_exists(transaction)? {
        let mut statement = transaction
            .prepare("SELECT version, name FROM schema_migrations ORDER BY version")
            .map_err(map_sqlite_error)?;
        let ledger = statement
            .query_map([], |row| {
                Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(map_sqlite_error)?;

        for entry in ledger {
            let (version, actual_name) = entry.map_err(map_sqlite_error)?;
            match MIGRATIONS
                .iter()
                .find(|migration| migration.version == version)
            {
                Some(migration) if migration.name == actual_name => {
                    applied_versions.insert(version);
                }
                Some(migration) => {
                    return Err(migration_mismatch(version, migration.name, &actual_name));
                }
                None => return Err(database_failure()),
            }
        }
    }

    for migration in MIGRATIONS {
        if !applied_versions.contains(&migration.version) {
            apply_migration_in_transaction(transaction, *migration)?;
        }
    }

    Ok(())
}

fn schema_migrations_exists(connection: &Connection) -> Result<bool, CommandError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations')",
            [],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)
}

fn apply_migration(connection: &mut Connection, migration: Migration) -> Result<(), CommandError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_error)?;
    apply_migration_in_transaction(&transaction, migration)?;
    transaction.commit().map_err(map_sqlite_error)
}

fn apply_migration_in_transaction(
    transaction: &Transaction<'_>,
    migration: Migration,
) -> Result<(), CommandError> {
    transaction
        .execute_batch(migration.sql)
        .map_err(map_sqlite_error)?;
    transaction
        .execute(
            "INSERT INTO schema_migrations(version, name, applied_at) VALUES (?1, ?2, ?3)",
            (migration.version, migration.name, utc_unix_millis()),
        )
        .map_err(map_sqlite_error)?;
    Ok(())
}

fn validate_integrity_rows(rows: &[String]) -> Result<(), CommandError> {
    if matches!(rows, [row] if row == "ok") {
        Ok(())
    } else {
        Err(database_failure_with_reason("integrityCheckFailed"))
    }
}

fn utc_unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn map_io_error(_: std::io::Error) -> CommandError {
    CommandError {
        code: AppErrorCode::IoFailure,
        message_key: "errors.ioFailure".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

fn map_sqlite_error(error: rusqlite::Error) -> CommandError {
    match error {
        rusqlite::Error::SqliteFailure(error, _) => match error.code {
            ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => CommandError {
                code: AppErrorCode::StorageUnavailable,
                message_key: "errors.storageUnavailable".into(),
                details: SafeMessageParameters::new(),
                retryable: true,
            },
            ErrorCode::ConstraintViolation => CommandError {
                code: AppErrorCode::Conflict,
                message_key: "errors.conflict".into(),
                details: SafeMessageParameters::new(),
                retryable: false,
            },
            _ => database_failure(),
        },
        _ => database_failure(),
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

fn database_failure_with_reason(reason_code: &str) -> CommandError {
    CommandError {
        code: AppErrorCode::DatabaseFailure,
        message_key: "errors.databaseFailure".into(),
        details: BTreeMap::from([(
            "reasonCode".into(),
            SafeParameterValue::String(reason_code.into()),
        )]),
        retryable: false,
    }
}

fn migration_mismatch(version: u32, expected_name: &str, actual_name: &str) -> CommandError {
    CommandError {
        code: AppErrorCode::DatabaseFailure,
        message_key: "errors.databaseFailure".into(),
        details: BTreeMap::from([
            (
                "version".into(),
                SafeParameterValue::String(version.to_string()),
            ),
            (
                "expectedName".into(),
                SafeParameterValue::String(expected_name.into()),
            ),
            (
                "actualName".into(),
                SafeParameterValue::String(actual_name.into()),
            ),
        ]),
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{AppErrorCode, CommandError, SafeParameterValue};
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn fixture_storage_at_latest_version() -> Storage {
        let mut connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        run_migrations(&mut connection).unwrap();
        Storage {
            path: PathBuf::from(":memory:"),
            connection: Mutex::new(connection),
        }
    }

    fn connection_at_version_two() -> Connection {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        for migration in &MIGRATIONS[..2] {
            apply_migration(&mut connection, *migration).unwrap();
        }
        connection
    }

    fn connection_at_version_three() -> Connection {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        for migration in &MIGRATIONS[..3] {
            apply_migration(&mut connection, *migration).unwrap();
        }
        connection
    }

    fn schema_names(connection: &Connection, query: &str) -> BTreeSet<String> {
        let mut statement = connection.prepare(query).unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<BTreeSet<_>, _>>()
            .unwrap()
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ColumnMetadata {
        position: i64,
        name: String,
        declared_type: String,
        not_null: bool,
        default_sql: Option<String>,
        primary_key_position: i64,
    }

    fn column_metadata(connection: &Connection, table: &str) -> Vec<ColumnMetadata> {
        let mut statement = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap();
        statement
            .query_map([], |row| {
                Ok(ColumnMetadata {
                    position: row.get(0)?,
                    name: row.get(1)?,
                    declared_type: row.get(2)?,
                    not_null: row.get::<_, i64>(3)? != 0,
                    default_sql: row.get(4)?,
                    primary_key_position: row.get(5)?,
                })
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ExplicitIndexMetadata {
        name: String,
        unique: bool,
        origin: String,
        partial: bool,
    }

    fn explicit_index_metadata(connection: &Connection, table: &str) -> Vec<ExplicitIndexMetadata> {
        let mut statement = connection
            .prepare(&format!("PRAGMA index_list({table})"))
            .unwrap();
        let mut indexes = statement
            .query_map([], |row| {
                Ok(ExplicitIndexMetadata {
                    name: row.get(1)?,
                    unique: row.get::<_, i64>(2)? != 0,
                    origin: row.get(3)?,
                    partial: row.get::<_, i64>(4)? != 0,
                })
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .filter(|index| index.origin == "c")
            .collect::<Vec<_>>();
        indexes.sort_by(|left, right| left.name.cmp(&right.name));
        indexes
    }

    #[derive(Debug, PartialEq, Eq)]
    struct IndexKeyMetadata {
        position: i64,
        column_position: i64,
        column_name: String,
        descending: bool,
        collation: String,
    }

    fn index_key_metadata(connection: &Connection, index: &str) -> Vec<IndexKeyMetadata> {
        let mut statement = connection
            .prepare(&format!("PRAGMA index_xinfo({index})"))
            .unwrap();
        statement
            .query_map([], |row| {
                let is_key = row.get::<_, i64>(5)? != 0;
                Ok(is_key.then(|| IndexKeyMetadata {
                    position: row.get(0).unwrap(),
                    column_position: row.get(1).unwrap(),
                    column_name: row.get(2).unwrap(),
                    descending: row.get::<_, i64>(3).unwrap() != 0,
                    collation: row.get(4).unwrap(),
                }))
            })
            .unwrap()
            .filter_map(Result::transpose)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ForeignKeyMetadata {
        id: i64,
        sequence: i64,
        referenced_table: String,
        from_column: String,
        to_column: String,
        on_update: String,
        on_delete: String,
        match_clause: String,
    }

    fn foreign_key_metadata(connection: &Connection, table: &str) -> Vec<ForeignKeyMetadata> {
        let mut statement = connection
            .prepare(&format!("PRAGMA foreign_key_list({table})"))
            .unwrap();
        statement
            .query_map([], |row| {
                Ok(ForeignKeyMetadata {
                    id: row.get(0)?,
                    sequence: row.get(1)?,
                    referenced_table: row.get(2)?,
                    from_column: row.get(3)?,
                    to_column: row.get(4)?,
                    on_update: row.get(5)?,
                    on_delete: row.get(6)?,
                    match_clause: row.get(7)?,
                })
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_clipboard_item_row(
        connection: &Connection,
        id: &str,
        content_kind: &str,
        text_content: Option<&str>,
        content_sha256: &str,
        source_app: Option<&str>,
        pinned: i64,
        captured_at: i64,
        last_seen_at: i64,
        byte_size: i64,
    ) -> rusqlite::Result<usize> {
        connection.execute(
            "INSERT INTO clipboard_items(
                id, content_kind, text_content, content_sha256, source_app,
                pinned, captured_at, last_seen_at, byte_size
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                id,
                content_kind,
                text_content,
                content_sha256,
                source_app,
                pinned,
                captured_at,
                last_seen_at,
                byte_size,
            ],
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_clipboard_asset_row(
        connection: &Connection,
        id: &str,
        clipboard_item_id: &str,
        asset_name: &str,
        mime_type: &str,
        width: i64,
        height: i64,
        sha256: &str,
        byte_size: i64,
        created_at: i64,
    ) -> rusqlite::Result<usize> {
        connection.execute(
            "INSERT INTO clipboard_assets(
                id, clipboard_item_id, asset_name, mime_type, width, height,
                sha256, byte_size, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                id,
                clipboard_item_id,
                asset_name,
                mime_type,
                width,
                height,
                sha256,
                byte_size,
                created_at,
            ],
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_todo_row(
        connection: &Connection,
        id: &str,
        title: &str,
        description: &str,
        due_at: Option<i64>,
        priority: &str,
        status: &str,
        revision: i64,
        created_at: i64,
        updated_at: i64,
        completed_at: Option<i64>,
    ) -> rusqlite::Result<usize> {
        connection.execute(
            "INSERT INTO todos(
                id, title, description, due_at, priority, status, revision,
                created_at, updated_at, completed_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                id,
                title,
                description,
                due_at,
                priority,
                status,
                revision,
                created_at,
                updated_at,
                completed_at,
            ],
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_reminder_row(
        connection: &Connection,
        id: &str,
        todo_id: &str,
        remind_at: i64,
        enabled: i64,
        revision: i64,
        created_at: i64,
        updated_at: i64,
    ) -> rusqlite::Result<usize> {
        connection.execute(
            "INSERT INTO todo_reminders(
                id, todo_id, remind_at, enabled, revision, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![id, todo_id, remind_at, enabled, revision, created_at, updated_at,],
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_note_row(
        connection: &Connection,
        id: &str,
        note_date: &str,
        body_markdown: &str,
        revision: i64,
        export_history_json: &str,
        created_at: i64,
        updated_at: i64,
    ) -> rusqlite::Result<usize> {
        connection.execute(
            "INSERT INTO notes(
                id, note_date, body_markdown, revision, export_history_json,
                created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                id,
                note_date,
                body_markdown,
                revision,
                export_history_json,
                created_at,
                updated_at,
            ],
        )
    }

    fn assert_constraint_violation(label: &str, result: rusqlite::Result<usize>) {
        let error = result.unwrap_err();
        assert!(
            matches!(
                error,
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error {
                        code: ErrorCode::ConstraintViolation,
                        ..
                    },
                    _
                )
            ),
            "{label}: expected a SQLite constraint violation, got {error:?}"
        );
    }

    fn insert_todo_and_reminder(storage: &Storage, todo_id: &str, reminder_id: &str) {
        storage
            .with_connection(|connection| {
                insert_todo_row(
                    connection,
                    todo_id,
                    "Task",
                    "",
                    Some(1000),
                    "normal",
                    "open",
                    1,
                    0,
                    0,
                    None,
                )
                .map_err(CommandError::from)?;
                insert_reminder_row(connection, reminder_id, todo_id, 500, 1, 1, 0, 0)
                    .map_err(CommandError::from)?;
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn fresh_database_applies_todo_notes_once() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).unwrap();
        assert_eq!(storage.schema_version().unwrap(), 10);
        drop(storage);
        let reopened = Storage::open(dir.path()).unwrap();
        let rows: Vec<(i64, String)> = reopened
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare("SELECT version, name FROM schema_migrations ORDER BY version")
                    .map_err(CommandError::from)?;
                let rows = statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                    .map_err(CommandError::from)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(CommandError::from)?;
                Ok(rows)
            })
            .unwrap();
        assert_eq!(
            rows,
            vec![
                (1, "foundation".into()),
                (2, "builtin_agents_reminders".into()),
                (3, "todo_notes".into()),
                (4, "clipboard_media".into()),
                (5, "monitor_notifications".into()),
                (6, "agent_integration_profiles".into()),
                (7, "retain_threshold_breach_history".into()),
                (8, "agent_attention_statuses".into()),
                (9, "agent_profile_reply_previews".into()),
                (10, "cursor_hook_profile".into()),
            ]
        );
    }

    #[test]
    fn fresh_database_applies_clipboard_media_once() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).unwrap();
        assert_eq!(storage.schema_version().unwrap(), 10);
        let tables: Vec<String> = storage
            .with_connection(|connection| {
                let mut statement = connection.prepare(
                    "SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'clipboard_%' ORDER BY name",
                )?;
                let rows = statement
                    .query_map([], |row| row.get(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .unwrap();
        assert_eq!(
            tables,
            vec![
                "clipboard_assets".to_string(),
                "clipboard_items".to_string()
            ]
        );
        drop(storage);

        let reopened = Storage::open(dir.path()).unwrap();
        let rows: Vec<(i64, String)> = reopened
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare("SELECT version, name FROM schema_migrations ORDER BY version")?;
                let rows = statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .unwrap();
        assert_eq!(
            rows,
            vec![
                (1, "foundation".into()),
                (2, "builtin_agents_reminders".into()),
                (3, "todo_notes".into()),
                (4, "clipboard_media".into()),
                (5, "monitor_notifications".into()),
                (6, "agent_integration_profiles".into()),
                (7, "retain_threshold_breach_history".into()),
                (8, "agent_attention_statuses".into()),
                (9, "agent_profile_reply_previews".into()),
                (10, "cursor_hook_profile".into()),
            ]
        );
    }

    #[test]
    fn clipboard_media_migration_adds_only_the_locked_tables_and_index() {
        const USER_TABLES: &str =
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'";
        const EXPLICIT_INDEXES: &str =
            "SELECT name FROM sqlite_master WHERE type = 'index' AND sql IS NOT NULL";

        let mut connection = connection_at_version_three();
        connection
            .execute(
                "INSERT INTO notes(id, note_date, body_markdown, revision, created_at, updated_at)
                 VALUES ('note-before-clipboard', '2026-08-14', 'body', 1, 1, 1)",
                [],
            )
            .unwrap();
        let tables_before = schema_names(&connection, USER_TABLES);
        let indexes_before = schema_names(&connection, EXPLICIT_INDEXES);

        apply_migration(&mut connection, MIGRATIONS[3]).unwrap();

        let added_tables = schema_names(&connection, USER_TABLES)
            .difference(&tables_before)
            .cloned()
            .collect::<BTreeSet<_>>();
        let added_indexes = schema_names(&connection, EXPLICIT_INDEXES)
            .difference(&indexes_before)
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            added_tables,
            BTreeSet::from(["clipboard_assets".into(), "clipboard_items".into()])
        );
        assert_eq!(
            added_indexes,
            BTreeSet::from(["clipboard_items_list_idx".into()])
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT body_markdown FROM notes WHERE id = 'note-before-clipboard'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "body"
        );
    }

    #[test]
    fn clipboard_media_schema_metadata_matches_the_locked_contract() {
        let connection = connection_at_version_three();
        let mut connection = connection;
        apply_migration(&mut connection, MIGRATIONS[3]).unwrap();

        let item_columns = column_metadata(&connection, "clipboard_items")
            .into_iter()
            .map(|column| {
                (
                    column.position,
                    column.name,
                    column.declared_type,
                    column.not_null,
                    column.default_sql,
                    column.primary_key_position,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            item_columns,
            vec![
                (0, "id".into(), "TEXT".into(), false, None, 1),
                (1, "content_kind".into(), "TEXT".into(), true, None, 0),
                (2, "text_content".into(), "TEXT".into(), false, None, 0),
                (3, "content_sha256".into(), "TEXT".into(), true, None, 0),
                (4, "source_app".into(), "TEXT".into(), false, None, 0),
                (
                    5,
                    "pinned".into(),
                    "INTEGER".into(),
                    true,
                    Some("0".into()),
                    0
                ),
                (6, "captured_at".into(), "INTEGER".into(), true, None, 0),
                (7, "last_seen_at".into(), "INTEGER".into(), true, None, 0),
                (8, "byte_size".into(), "INTEGER".into(), true, None, 0),
            ]
        );

        let asset_columns = column_metadata(&connection, "clipboard_assets")
            .into_iter()
            .map(|column| {
                (
                    column.position,
                    column.name,
                    column.declared_type,
                    column.not_null,
                    column.default_sql,
                    column.primary_key_position,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            asset_columns,
            vec![
                (0, "id".into(), "TEXT".into(), false, None, 1),
                (1, "clipboard_item_id".into(), "TEXT".into(), true, None, 0),
                (2, "asset_name".into(), "TEXT".into(), true, None, 0),
                (3, "mime_type".into(), "TEXT".into(), true, None, 0),
                (4, "width".into(), "INTEGER".into(), true, None, 0),
                (5, "height".into(), "INTEGER".into(), true, None, 0),
                (6, "sha256".into(), "TEXT".into(), true, None, 0),
                (7, "byte_size".into(), "INTEGER".into(), true, None, 0),
                (8, "created_at".into(), "INTEGER".into(), true, None, 0),
            ]
        );

        assert_eq!(
            explicit_index_metadata(&connection, "clipboard_items"),
            vec![ExplicitIndexMetadata {
                name: "clipboard_items_list_idx".into(),
                unique: false,
                origin: "c".into(),
                partial: false,
            }]
        );
        assert_eq!(
            index_key_metadata(&connection, "clipboard_items_list_idx"),
            vec![
                IndexKeyMetadata {
                    position: 0,
                    column_position: 5,
                    column_name: "pinned".into(),
                    descending: true,
                    collation: "BINARY".into(),
                },
                IndexKeyMetadata {
                    position: 1,
                    column_position: 7,
                    column_name: "last_seen_at".into(),
                    descending: true,
                    collation: "BINARY".into(),
                },
                IndexKeyMetadata {
                    position: 2,
                    column_position: 0,
                    column_name: "id".into(),
                    descending: false,
                    collation: "BINARY".into(),
                },
            ]
        );
        assert_eq!(
            foreign_key_metadata(&connection, "clipboard_assets"),
            vec![ForeignKeyMetadata {
                id: 0,
                sequence: 0,
                referenced_table: "clipboard_items".into(),
                from_column: "clipboard_item_id".into(),
                to_column: "id".into(),
                on_update: "NO ACTION".into(),
                on_delete: "CASCADE".into(),
                match_clause: "NONE".into(),
            }]
        );
    }

    #[test]
    fn clipboard_media_check_and_unique_constraints_reject_invalid_rows() {
        let mut connection = connection_at_version_three();
        apply_migration(&mut connection, MIGRATIONS[3]).unwrap();
        let hash = "a".repeat(64);
        insert_clipboard_item_row(
            &connection,
            "valid-text",
            "text",
            Some("text"),
            &hash,
            Some("app.exe"),
            0,
            1,
            1,
            4,
        )
        .unwrap();

        for result in [
            insert_clipboard_item_row(
                &connection,
                "bad-kind",
                "other",
                None,
                &"1".repeat(64),
                None,
                0,
                0,
                0,
                0,
            ),
            insert_clipboard_item_row(
                &connection,
                "text-without-body",
                "text",
                None,
                &"2".repeat(64),
                None,
                0,
                0,
                0,
                0,
            ),
            insert_clipboard_item_row(
                &connection,
                "image-with-body",
                "image",
                Some("forbidden"),
                &"3".repeat(64),
                None,
                0,
                0,
                0,
                1,
            ),
            insert_clipboard_item_row(
                &connection,
                "short-hash",
                "text",
                Some("text"),
                &"4".repeat(63),
                None,
                0,
                0,
                0,
                4,
            ),
            insert_clipboard_item_row(
                &connection,
                "long-source",
                "text",
                Some("text"),
                &"5".repeat(64),
                Some(&"界".repeat(261)),
                0,
                0,
                0,
                4,
            ),
            insert_clipboard_item_row(
                &connection,
                "bad-pin",
                "text",
                Some("text"),
                &"6".repeat(64),
                None,
                2,
                0,
                0,
                4,
            ),
            insert_clipboard_item_row(
                &connection,
                "negative-capture",
                "text",
                Some("text"),
                &"7".repeat(64),
                None,
                0,
                -1,
                0,
                4,
            ),
            insert_clipboard_item_row(
                &connection,
                "time-reversal",
                "text",
                Some("text"),
                &"8".repeat(64),
                None,
                0,
                2,
                1,
                4,
            ),
            insert_clipboard_item_row(
                &connection,
                "negative-size",
                "text",
                Some("text"),
                &"9".repeat(64),
                None,
                0,
                0,
                0,
                -1,
            ),
            insert_clipboard_item_row(
                &connection,
                "duplicate-hash",
                "text",
                Some("other"),
                &hash,
                None,
                0,
                0,
                0,
                5,
            ),
        ] {
            assert!(result.is_err());
        }

        let mut asset_case = 0_u64;
        let mut assert_asset_rejected = |suffix: &str,
                                         mime_type: &str,
                                         width: i64,
                                         height: i64,
                                         asset_hash: String,
                                         byte_size: i64,
                                         created_at: i64| {
            asset_case += 1;
            let parent_id = format!("parent-{suffix}");
            insert_clipboard_item_row(
                &connection,
                &parent_id,
                "image",
                None,
                &format!("{asset_case:064x}"),
                None,
                0,
                0,
                0,
                1,
            )
            .unwrap();
            assert!(insert_clipboard_asset_row(
                &connection,
                &format!("asset-{suffix}"),
                &parent_id,
                &format!("asset-{suffix}.png"),
                mime_type,
                width,
                height,
                &asset_hash,
                byte_size,
                created_at,
            )
            .is_err());
        };
        let valid_asset_hash = "b".repeat(64);
        assert_asset_rejected("mime", "image/jpeg", 1, 1, valid_asset_hash.clone(), 1, 0);
        assert_asset_rejected(
            "width-low",
            "image/png",
            0,
            1,
            valid_asset_hash.clone(),
            1,
            0,
        );
        assert_asset_rejected(
            "width-high",
            "image/png",
            8193,
            1,
            valid_asset_hash.clone(),
            1,
            0,
        );
        assert_asset_rejected(
            "height-low",
            "image/png",
            1,
            0,
            valid_asset_hash.clone(),
            1,
            0,
        );
        assert_asset_rejected(
            "height-high",
            "image/png",
            1,
            8193,
            valid_asset_hash.clone(),
            1,
            0,
        );
        assert_asset_rejected("hash", "image/png", 1, 1, "b".repeat(63), 1, 0);
        assert_asset_rejected(
            "size-low",
            "image/png",
            1,
            1,
            valid_asset_hash.clone(),
            0,
            0,
        );
        assert_asset_rejected(
            "size-high",
            "image/png",
            1,
            1,
            valid_asset_hash.clone(),
            20_971_521,
            0,
        );
        assert_asset_rejected("created", "image/png", 1, 1, valid_asset_hash, 1, -1);

        for (parent_id, parent_hash) in [
            ("unique-parent-one", "e".repeat(64)),
            ("unique-parent-two", "f".repeat(64)),
        ] {
            insert_clipboard_item_row(
                &connection,
                parent_id,
                "image",
                None,
                &parent_hash,
                None,
                0,
                0,
                0,
                1,
            )
            .unwrap();
        }
        insert_clipboard_asset_row(
            &connection,
            "unique-asset-one",
            "unique-parent-one",
            "owned.png",
            "image/png",
            1,
            1,
            &"0".repeat(64),
            1,
            0,
        )
        .unwrap();
        assert!(insert_clipboard_asset_row(
            &connection,
            "duplicate-parent-asset",
            "unique-parent-one",
            "other.png",
            "image/png",
            1,
            1,
            &"1".repeat(64),
            1,
            0,
        )
        .is_err());
        assert!(insert_clipboard_asset_row(
            &connection,
            "duplicate-name-asset",
            "unique-parent-two",
            "owned.png",
            "image/png",
            1,
            1,
            &"2".repeat(64),
            1,
            0,
        )
        .is_err());
    }

    #[test]
    fn todo_reminder_is_deleted_with_its_todo() {
        let storage = fixture_storage_at_latest_version();
        insert_todo_and_reminder(&storage, "todo-1", "reminder-1");
        storage
            .with_connection(|connection| {
                connection
                    .execute("DELETE FROM todos WHERE id = 'todo-1'", [])
                    .map_err(CommandError::from)?;
                let count: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM todo_reminders WHERE id = 'reminder-1'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(CommandError::from)?;
                assert_eq!(count, 0);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn todo_notes_migration_adds_exact_locked_tables_and_indexes() {
        const USER_TABLES: &str =
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'";
        const EXPLICIT_INDEXES: &str =
            "SELECT name FROM sqlite_master WHERE type = 'index' AND sql IS NOT NULL";

        let mut connection = connection_at_version_two();
        let tables_before = schema_names(&connection, USER_TABLES);
        let indexes_before = schema_names(&connection, EXPLICIT_INDEXES);

        apply_migration(&mut connection, MIGRATIONS[2]).unwrap();

        let added_tables = schema_names(&connection, USER_TABLES)
            .difference(&tables_before)
            .cloned()
            .collect::<BTreeSet<_>>();
        let added_indexes = schema_names(&connection, EXPLICIT_INDEXES)
            .difference(&indexes_before)
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            added_tables,
            BTreeSet::from(["notes".into(), "todo_reminders".into(), "todos".into()])
        );
        assert_eq!(
            added_indexes,
            BTreeSet::from([
                "notes_updated_idx".into(),
                "todo_reminders_due_idx".into(),
                "todos_status_due_idx".into(),
            ])
        );
    }

    #[test]
    fn todo_notes_schema_metadata_matches_locked_contract() {
        let storage = fixture_storage_at_latest_version();
        storage
            .with_connection(|connection| {
                assert_eq!(
                    column_metadata(connection, "todos"),
                    vec![
                        ColumnMetadata {
                            position: 0,
                            name: "id".into(),
                            declared_type: "TEXT".into(),
                            not_null: false,
                            default_sql: None,
                            primary_key_position: 1,
                        },
                        ColumnMetadata {
                            position: 1,
                            name: "title".into(),
                            declared_type: "TEXT".into(),
                            not_null: true,
                            default_sql: None,
                            primary_key_position: 0,
                        },
                        ColumnMetadata {
                            position: 2,
                            name: "description".into(),
                            declared_type: "TEXT".into(),
                            not_null: true,
                            default_sql: None,
                            primary_key_position: 0,
                        },
                        ColumnMetadata {
                            position: 3,
                            name: "due_at".into(),
                            declared_type: "INTEGER".into(),
                            not_null: false,
                            default_sql: None,
                            primary_key_position: 0,
                        },
                        ColumnMetadata {
                            position: 4,
                            name: "priority".into(),
                            declared_type: "TEXT".into(),
                            not_null: true,
                            default_sql: None,
                            primary_key_position: 0,
                        },
                        ColumnMetadata {
                            position: 5,
                            name: "status".into(),
                            declared_type: "TEXT".into(),
                            not_null: true,
                            default_sql: None,
                            primary_key_position: 0,
                        },
                        ColumnMetadata {
                            position: 6,
                            name: "revision".into(),
                            declared_type: "INTEGER".into(),
                            not_null: true,
                            default_sql: Some("1".into()),
                            primary_key_position: 0,
                        },
                        ColumnMetadata {
                            position: 7,
                            name: "created_at".into(),
                            declared_type: "INTEGER".into(),
                            not_null: true,
                            default_sql: None,
                            primary_key_position: 0,
                        },
                        ColumnMetadata {
                            position: 8,
                            name: "updated_at".into(),
                            declared_type: "INTEGER".into(),
                            not_null: true,
                            default_sql: None,
                            primary_key_position: 0,
                        },
                        ColumnMetadata {
                            position: 9,
                            name: "completed_at".into(),
                            declared_type: "INTEGER".into(),
                            not_null: false,
                            default_sql: None,
                            primary_key_position: 0,
                        },
                    ]
                );
                assert_eq!(
                    column_metadata(connection, "todo_reminders"),
                    vec![
                        ColumnMetadata {
                            position: 0,
                            name: "id".into(),
                            declared_type: "TEXT".into(),
                            not_null: false,
                            default_sql: None,
                            primary_key_position: 1,
                        },
                        ColumnMetadata {
                            position: 1,
                            name: "todo_id".into(),
                            declared_type: "TEXT".into(),
                            not_null: true,
                            default_sql: None,
                            primary_key_position: 0,
                        },
                        ColumnMetadata {
                            position: 2,
                            name: "remind_at".into(),
                            declared_type: "INTEGER".into(),
                            not_null: true,
                            default_sql: None,
                            primary_key_position: 0,
                        },
                        ColumnMetadata {
                            position: 3,
                            name: "enabled".into(),
                            declared_type: "INTEGER".into(),
                            not_null: true,
                            default_sql: None,
                            primary_key_position: 0,
                        },
                        ColumnMetadata {
                            position: 4,
                            name: "revision".into(),
                            declared_type: "INTEGER".into(),
                            not_null: true,
                            default_sql: Some("1".into()),
                            primary_key_position: 0,
                        },
                        ColumnMetadata {
                            position: 5,
                            name: "created_at".into(),
                            declared_type: "INTEGER".into(),
                            not_null: true,
                            default_sql: None,
                            primary_key_position: 0,
                        },
                        ColumnMetadata {
                            position: 6,
                            name: "updated_at".into(),
                            declared_type: "INTEGER".into(),
                            not_null: true,
                            default_sql: None,
                            primary_key_position: 0,
                        },
                    ]
                );
                assert_eq!(
                    column_metadata(connection, "notes"),
                    vec![
                        ColumnMetadata {
                            position: 0,
                            name: "id".into(),
                            declared_type: "TEXT".into(),
                            not_null: false,
                            default_sql: None,
                            primary_key_position: 1,
                        },
                        ColumnMetadata {
                            position: 1,
                            name: "note_date".into(),
                            declared_type: "TEXT".into(),
                            not_null: true,
                            default_sql: None,
                            primary_key_position: 0,
                        },
                        ColumnMetadata {
                            position: 2,
                            name: "body_markdown".into(),
                            declared_type: "TEXT".into(),
                            not_null: true,
                            default_sql: None,
                            primary_key_position: 0,
                        },
                        ColumnMetadata {
                            position: 3,
                            name: "revision".into(),
                            declared_type: "INTEGER".into(),
                            not_null: true,
                            default_sql: Some("1".into()),
                            primary_key_position: 0,
                        },
                        ColumnMetadata {
                            position: 4,
                            name: "export_history_json".into(),
                            declared_type: "TEXT".into(),
                            not_null: true,
                            default_sql: Some("'[]'".into()),
                            primary_key_position: 0,
                        },
                        ColumnMetadata {
                            position: 5,
                            name: "created_at".into(),
                            declared_type: "INTEGER".into(),
                            not_null: true,
                            default_sql: None,
                            primary_key_position: 0,
                        },
                        ColumnMetadata {
                            position: 6,
                            name: "updated_at".into(),
                            declared_type: "INTEGER".into(),
                            not_null: true,
                            default_sql: None,
                            primary_key_position: 0,
                        },
                    ]
                );

                assert_eq!(
                    explicit_index_metadata(connection, "todos"),
                    vec![ExplicitIndexMetadata {
                        name: "todos_status_due_idx".into(),
                        unique: false,
                        origin: "c".into(),
                        partial: false,
                    }]
                );
                assert_eq!(
                    explicit_index_metadata(connection, "todo_reminders"),
                    vec![ExplicitIndexMetadata {
                        name: "todo_reminders_due_idx".into(),
                        unique: false,
                        origin: "c".into(),
                        partial: false,
                    }]
                );
                assert_eq!(
                    explicit_index_metadata(connection, "notes"),
                    vec![ExplicitIndexMetadata {
                        name: "notes_updated_idx".into(),
                        unique: false,
                        origin: "c".into(),
                        partial: false,
                    }]
                );

                assert_eq!(
                    index_key_metadata(connection, "todos_status_due_idx"),
                    vec![
                        IndexKeyMetadata {
                            position: 0,
                            column_position: 5,
                            column_name: "status".into(),
                            descending: false,
                            collation: "BINARY".into(),
                        },
                        IndexKeyMetadata {
                            position: 1,
                            column_position: 3,
                            column_name: "due_at".into(),
                            descending: false,
                            collation: "BINARY".into(),
                        },
                        IndexKeyMetadata {
                            position: 2,
                            column_position: 8,
                            column_name: "updated_at".into(),
                            descending: true,
                            collation: "BINARY".into(),
                        },
                        IndexKeyMetadata {
                            position: 3,
                            column_position: 0,
                            column_name: "id".into(),
                            descending: false,
                            collation: "BINARY".into(),
                        },
                    ]
                );
                assert_eq!(
                    index_key_metadata(connection, "todo_reminders_due_idx"),
                    vec![
                        IndexKeyMetadata {
                            position: 0,
                            column_position: 3,
                            column_name: "enabled".into(),
                            descending: false,
                            collation: "BINARY".into(),
                        },
                        IndexKeyMetadata {
                            position: 1,
                            column_position: 2,
                            column_name: "remind_at".into(),
                            descending: false,
                            collation: "BINARY".into(),
                        },
                        IndexKeyMetadata {
                            position: 2,
                            column_position: 1,
                            column_name: "todo_id".into(),
                            descending: false,
                            collation: "BINARY".into(),
                        },
                    ]
                );
                assert_eq!(
                    index_key_metadata(connection, "notes_updated_idx"),
                    vec![
                        IndexKeyMetadata {
                            position: 0,
                            column_position: 6,
                            column_name: "updated_at".into(),
                            descending: true,
                            collation: "BINARY".into(),
                        },
                        IndexKeyMetadata {
                            position: 1,
                            column_position: 1,
                            column_name: "note_date".into(),
                            descending: true,
                            collation: "BINARY".into(),
                        },
                        IndexKeyMetadata {
                            position: 2,
                            column_position: 0,
                            column_name: "id".into(),
                            descending: false,
                            collation: "BINARY".into(),
                        },
                    ]
                );

                assert_eq!(
                    foreign_key_metadata(connection, "todo_reminders"),
                    vec![ForeignKeyMetadata {
                        id: 0,
                        sequence: 0,
                        referenced_table: "todos".into(),
                        from_column: "todo_id".into(),
                        to_column: "id".into(),
                        on_update: "NO ACTION".into(),
                        on_delete: "CASCADE".into(),
                        match_clause: "NONE".into(),
                    }]
                );
                assert!(foreign_key_metadata(connection, "todos").is_empty());
                assert!(foreign_key_metadata(connection, "notes").is_empty());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn todo_notes_upgrade_from_schema_two_preserves_prior_data() {
        let mut connection = connection_at_version_two();
        connection
            .execute(
                "INSERT INTO app_settings(key, value_json, revision, updated_at)
                 VALUES ('locale', '\"zh-CN\"', 4, 11)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO agent_integrations(
                    agent_id, environment, install_state, config_path, revision, updated_at
                 ) VALUES ('codex', 'windows', 'installed', 'C:/codex/config.toml', 2, 12)",
                [],
            )
            .unwrap();

        run_migrations(&mut connection).unwrap();
        run_migrations(&mut connection).unwrap();

        let setting: (String, i64) = connection
            .query_row(
                "SELECT value_json, revision FROM app_settings WHERE key = 'locale'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let integration: (String, i64) = connection
            .query_row(
                "SELECT config_path, revision FROM agent_integrations
                 WHERE agent_id = 'codex' AND environment = 'windows'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let ledger = {
            let mut statement = connection
                .prepare("SELECT version, name FROM schema_migrations ORDER BY version")
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<Result<Vec<(i64, String)>, _>>()
                .unwrap()
        };
        assert_eq!(setting, ("\"zh-CN\"".into(), 4));
        assert_eq!(integration, ("C:/codex/config.toml".into(), 2));
        assert_eq!(
            ledger,
            vec![
                (1, "foundation".into()),
                (2, "builtin_agents_reminders".into()),
                (3, "todo_notes".into()),
                (4, "clipboard_media".into()),
                (5, "monitor_notifications".into()),
                (6, "agent_integration_profiles".into()),
                (7, "retain_threshold_breach_history".into()),
                (8, "agent_attention_statuses".into()),
                (9, "agent_profile_reply_previews".into()),
                (10, "cursor_hook_profile".into()),
            ]
        );
    }

    #[test]
    fn todo_notes_todo_constraints_reject_invalid_rows() {
        let long_title = "x".repeat(201);
        let long_description = "x".repeat(4001);
        let cases = vec![
            (
                "trimmed title is empty",
                " ".to_string(),
                String::new(),
                Some(1),
                "normal",
                "open",
                1,
                0,
                0,
                None,
            ),
            (
                "title exceeds 200 characters",
                long_title,
                String::new(),
                Some(1),
                "normal",
                "open",
                1,
                0,
                0,
                None,
            ),
            (
                "description exceeds 4000 characters",
                "Task".into(),
                long_description,
                Some(1),
                "normal",
                "open",
                1,
                0,
                0,
                None,
            ),
            (
                "due_at is negative",
                "Task".into(),
                String::new(),
                Some(-1),
                "normal",
                "open",
                1,
                0,
                0,
                None,
            ),
            (
                "priority is outside the locked enum",
                "Task".into(),
                String::new(),
                Some(1),
                "urgent",
                "open",
                1,
                0,
                0,
                None,
            ),
            (
                "status is outside the locked enum",
                "Task".into(),
                String::new(),
                Some(1),
                "normal",
                "cancelled",
                1,
                0,
                0,
                None,
            ),
            (
                "open todo has completed_at",
                "Task".into(),
                String::new(),
                Some(1),
                "normal",
                "open",
                1,
                0,
                0,
                Some(0),
            ),
            (
                "completed todo lacks completed_at",
                "Task".into(),
                String::new(),
                Some(1),
                "normal",
                "completed",
                1,
                0,
                0,
                None,
            ),
            (
                "revision is not positive",
                "Task".into(),
                String::new(),
                Some(1),
                "normal",
                "open",
                0,
                0,
                0,
                None,
            ),
            (
                "created_at is negative",
                "Task".into(),
                String::new(),
                Some(1),
                "normal",
                "open",
                1,
                -1,
                0,
                None,
            ),
            (
                "updated_at precedes created_at",
                "Task".into(),
                String::new(),
                Some(1),
                "normal",
                "open",
                1,
                10,
                9,
                None,
            ),
            (
                "completed_at precedes created_at",
                "Task".into(),
                String::new(),
                Some(1),
                "normal",
                "completed",
                1,
                10,
                10,
                Some(9),
            ),
        ];

        for (
            label,
            title,
            description,
            due_at,
            priority,
            status,
            revision,
            created_at,
            updated_at,
            completed_at,
        ) in cases
        {
            let storage = fixture_storage_at_latest_version();
            storage
                .with_connection(|connection| {
                    assert_constraint_violation(
                        label,
                        insert_todo_row(
                            connection,
                            "todo-invalid",
                            &title,
                            &description,
                            due_at,
                            priority,
                            status,
                            revision,
                            created_at,
                            updated_at,
                            completed_at,
                        ),
                    );
                    Ok(())
                })
                .unwrap();
        }
    }

    #[test]
    fn todo_notes_note_date_is_unique() {
        let storage = fixture_storage_at_latest_version();
        storage
            .with_connection(|connection| {
                insert_note_row(connection, "note-1", "2026-08-11", "first", 1, "[]", 0, 0)
                    .map_err(CommandError::from)?;
                assert_constraint_violation(
                    "note_date is unique",
                    insert_note_row(connection, "note-2", "2026-08-11", "second", 1, "[]", 0, 0),
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn todo_notes_note_constraints_reject_invalid_rows() {
        let long_body = "x".repeat(262_145);
        let cases = vec![
            (
                "note_date does not match the locked GLOB",
                "2026/08/11",
                String::new(),
                1,
                "[]",
                0,
                0,
            ),
            (
                "body_markdown exceeds 262144 characters",
                "2026-08-11",
                long_body,
                1,
                "[]",
                0,
                0,
            ),
            (
                "export_history_json is invalid JSON",
                "2026-08-11",
                String::new(),
                1,
                "{",
                0,
                0,
            ),
            (
                "revision is not positive",
                "2026-08-11",
                String::new(),
                0,
                "[]",
                0,
                0,
            ),
            (
                "created_at is negative",
                "2026-08-11",
                String::new(),
                1,
                "[]",
                -1,
                0,
            ),
            (
                "updated_at precedes created_at",
                "2026-08-11",
                String::new(),
                1,
                "[]",
                10,
                9,
            ),
        ];

        for (label, note_date, body, revision, history, created_at, updated_at) in cases {
            let storage = fixture_storage_at_latest_version();
            storage
                .with_connection(|connection| {
                    assert_constraint_violation(
                        label,
                        insert_note_row(
                            connection,
                            "note-invalid",
                            note_date,
                            &body,
                            revision,
                            history,
                            created_at,
                            updated_at,
                        ),
                    );
                    Ok(())
                })
                .unwrap();
        }
    }

    #[test]
    fn todo_notes_reminder_constraints_reject_invalid_rows() {
        let storage = fixture_storage_at_latest_version();
        storage
            .with_connection(|connection| {
                insert_todo_row(
                    connection, "todo-1", "Task", "", None, "normal", "open", 1, 0, 0, None,
                )
                .map_err(CommandError::from)?;
                insert_reminder_row(connection, "reminder-1", "todo-1", 1, 1, 1, 0, 0)
                    .map_err(CommandError::from)?;
                assert_constraint_violation(
                    "todo_id is unique",
                    insert_reminder_row(connection, "reminder-2", "todo-1", 2, 1, 1, 0, 0),
                );
                assert_constraint_violation(
                    "todo_id is a foreign key",
                    insert_reminder_row(connection, "reminder-3", "missing", 2, 1, 1, 0, 0),
                );
                Ok(())
            })
            .unwrap();

        for (label, remind_at, enabled, revision, created_at, updated_at) in [
            ("remind_at is negative", -1, 1, 1, 0, 0),
            ("enabled is not boolean", 1, 2, 1, 0, 0),
            ("revision is not positive", 1, 1, 0, 0, 0),
            ("created_at is negative", 1, 1, 1, -1, 0),
            ("updated_at precedes created_at", 1, 1, 1, 10, 9),
        ] {
            let storage = fixture_storage_at_latest_version();
            storage
                .with_connection(|connection| {
                    insert_todo_row(
                        connection, "todo-1", "Task", "", None, "normal", "open", 1, 0, 0, None,
                    )
                    .map_err(CommandError::from)?;
                    assert_constraint_violation(
                        label,
                        insert_reminder_row(
                            connection,
                            "reminder-invalid",
                            "todo-1",
                            remind_at,
                            enabled,
                            revision,
                            created_at,
                            updated_at,
                        ),
                    );
                    Ok(())
                })
                .unwrap();
        }
    }

    #[test]
    fn fresh_database_applies_registered_migrations_once() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).unwrap();
        assert_eq!(storage.schema_version().unwrap(), 10);
        drop(storage);
        let reopened = Storage::open(dir.path()).unwrap();
        assert_eq!(reopened.schema_version().unwrap(), 10);
        let count: i64 = reopened
            .with_connection(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                        row.get(0)
                    })
                    .map_err(CommandError::from)
            })
            .unwrap();
        assert_eq!(count, 10);
    }

    #[test]
    fn schema_seven_upgrade_marks_kimi_interrupts_as_attention() {
        let dir = tempfile::tempdir().unwrap();
        let database_path = dir.path().join(DATABASE_FILE_NAME);
        let mut connection = Connection::open(&database_path).unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        for migration in &MIGRATIONS[..7] {
            apply_migration(&mut connection, *migration).unwrap();
        }
        connection
            .execute_batch(
                "INSERT INTO agent_profile_events(
                    profile_id,event_id,native_event,task_id,status,occurred_at,received_at
                 ) VALUES ('kimi-windows','interrupt-1','Interrupt','task-1','idle',10,10);
                 INSERT INTO agent_profile_observations(
                    profile_id,task_id,status,source_event_id,occurred_at,received_at
                 ) VALUES ('kimi-windows','task-1','idle','interrupt-1',10,10);",
            )
            .unwrap();
        drop(connection);

        let storage = Storage::open(dir.path()).unwrap();
        assert_eq!(storage.schema_version().unwrap(), 10);
        storage
            .with_connection(|connection| {
                let mapping_status: String = connection.query_row(
                    "SELECT json_extract(event_mapping_json, '$[5].normalizedStatus')
                     FROM agent_integration_profiles WHERE id='kimi-windows'",
                    [],
                    |row| row.get(0),
                )?;
                let event_status: String = connection.query_row(
                    "SELECT status FROM agent_profile_events
                     WHERE profile_id='kimi-windows' AND event_id='interrupt-1'",
                    [],
                    |row| row.get(0),
                )?;
                let observation_status: String = connection.query_row(
                    "SELECT status FROM agent_profile_observations
                     WHERE profile_id='kimi-windows' AND task_id='task-1'",
                    [],
                    |row| row.get(0),
                )?;
                let revision: i64 = connection.query_row(
                    "SELECT revision FROM agent_integration_profiles WHERE id='kimi-windows'",
                    [],
                    |row| row.get(0),
                )?;

                assert_eq!(mapping_status, "failed");
                assert_eq!(event_status, "failed");
                assert_eq!(observation_status, "failed");
                assert_eq!(revision, 2);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn schema_six_upgrade_retains_threshold_breaches_after_threshold_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let database_path = dir.path().join(DATABASE_FILE_NAME);
        let mut connection = Connection::open(&database_path).unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        for migration in &MIGRATIONS[..6] {
            apply_migration(&mut connection, *migration).unwrap();
        }
        connection
            .execute_batch(
                "INSERT INTO monitor_thresholds(
                    id,metric,comparator,threshold_value,hold_seconds,cooldown_seconds,
                    sound_json,toast_enabled,window_enabled,enabled,revision,updated_at
                 ) VALUES (
                    'threshold-before-seven','cpuPercent','greaterThanOrEqual',80,0,0,
                    '{\"kind\":\"none\"}',1,0,1,1,10
                 );
                 INSERT INTO threshold_breaches(
                    id,threshold_id,breach_started_at,last_triggered_at,cleared_at,reminder_delivery_id
                 ) VALUES (
                    'breach-before-seven','threshold-before-seven',11,12,NULL,NULL
                 );",
            )
            .unwrap();
        drop(connection);

        let storage = Storage::open(dir.path()).unwrap();
        assert_eq!(storage.schema_version().unwrap(), 10);
        storage
            .with_connection(|connection| {
                assert_eq!(
                    foreign_key_metadata(connection, "threshold_breaches"),
                    vec![ForeignKeyMetadata {
                        id: 0,
                        sequence: 0,
                        referenced_table: "reminder_deliveries".into(),
                        from_column: "reminder_delivery_id".into(),
                        to_column: "id".into(),
                        on_update: "NO ACTION".into(),
                        on_delete: "SET NULL".into(),
                        match_clause: "NONE".into(),
                    }]
                );
                connection.execute(
                    "DELETE FROM monitor_thresholds WHERE id='threshold-before-seven'",
                    [],
                )?;
                assert_eq!(
                    connection.query_row(
                        "SELECT COUNT(*) FROM threshold_breaches WHERE id='breach-before-seven'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )?,
                    1
                );
                assert!(connection
                    .execute(
                        "INSERT INTO threshold_breaches(
                            id,threshold_id,breach_started_at,last_triggered_at,cleared_at,reminder_delivery_id
                         ) VALUES ('duplicate-breach','threshold-before-seven',11,NULL,NULL,NULL)",
                        [],
                    )
                    .is_err());
                assert!(connection
                    .execute(
                        "INSERT INTO threshold_breaches(
                            id,threshold_id,breach_started_at,last_triggered_at,cleared_at,reminder_delivery_id
                         ) VALUES ('negative-breach','threshold-before-seven',-1,NULL,NULL,NULL)",
                        [],
                    )
                    .is_err());
                assert_eq!(
                    connection.query_row(
                        "SELECT COUNT(*) FROM schema_migrations
                         WHERE version=7 AND name='retain_threshold_breach_history'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )?,
                    1
                );
                Ok(())
            })
            .unwrap();
        drop(storage);

        let reopened = Storage::open(dir.path()).unwrap();
        assert_eq!(reopened.schema_version().unwrap(), 10);
        reopened
            .with_connection(|connection| {
                assert_eq!(
                    connection.query_row(
                        "SELECT COUNT(*) FROM threshold_breaches WHERE id='breach-before-seven'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )?,
                    1
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn migration_is_atomic_when_sql_fails() {
        let mut connection = rusqlite::Connection::open_in_memory().unwrap();
        let broken = Migration {
            version: 1,
            name: "broken",
            sql: "CREATE TABLE partial(id INTEGER); INVALID SQL;",
        };
        assert!(apply_migration(&mut connection, broken).is_err());
        let exists: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='partial'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 0);
    }

    #[test]
    fn integrity_check_returns_ok_for_a_healthy_database() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).unwrap();

        let (): () = storage.integrity_check().unwrap();
    }

    #[test]
    fn integrity_check_rejects_empty_multiple_and_non_ok_rows() {
        validate_integrity_rows(&["ok".to_string()]).unwrap();

        for rows in [
            vec![],
            vec!["ok".to_string(), "ok".to_string()],
            vec!["OK".to_string()],
            vec![" ok ".to_string()],
            vec!["database corruption".to_string()],
        ] {
            let error = validate_integrity_rows(&rows).unwrap_err();
            assert_eq!(error.code, AppErrorCode::DatabaseFailure);
            assert!(!error.retryable);
            assert_eq!(
                error.details.get("reasonCode"),
                Some(&SafeParameterValue::String("integrityCheckFailed".into()))
            );
        }
    }

    #[test]
    fn concurrent_first_opens_apply_registered_migrations_once() {
        let dir = tempfile::tempdir().unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let mut opens = Vec::new();

        for _ in 0..2 {
            let root = dir.path().to_path_buf();
            let start = Arc::clone(&barrier);
            opens.push(thread::spawn(move || {
                start.wait();
                Storage::open(&root).and_then(|storage| storage.schema_version())
            }));
        }

        for open in opens {
            assert_eq!(open.join().unwrap().unwrap(), 10);
        }

        let storage = Storage::open(dir.path()).unwrap();
        let count: i64 = storage
            .with_connection(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                        row.get(0)
                    })
                    .map_err(CommandError::from)
            })
            .unwrap();
        assert_eq!(count, 10);
    }
}
