use crate::repositories::notifications::{
    ImportedNotification, NotificationCursor, NotificationOrigin,
};
use quick_xml::events::{BytesRef, Event};
use quick_xml::reader::Reader;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const WPN_SOURCE_ID: &str = "windowsWpn";
const MAX_BATCH_SIZE: u32 = 200;
const MAX_XML_BYTES: usize = 256 * 1024;
const MAX_TEXT_CHARS: usize = 4_096;
const WINDOWS_UNKNOWN_APP: &str = "windows.unknown";
const FILETIME_UNIX_OFFSET_TICKS: i64 = 116_444_736_000_000_000;
const FILETIME_TICKS_PER_MILLISECOND: i64 = 10_000;
const MIN_SOURCE_UNIX_MILLIS: i64 = 946_684_800_000;
const MAX_SOURCE_UNIX_MILLIS_EXCLUSIVE: i64 = 7_289_654_400_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WpnSchema {
    NotificationV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WpnSourceFault {
    Missing,
    AccessDenied,
    Locked,
    SchemaIncompatible,
    InvalidInput,
    QueryFailed,
}

#[derive(Clone, Debug)]
pub struct WpnBatch {
    pub items: Vec<ImportedNotification>,
    pub cursor: NotificationCursor,
    pub has_more: bool,
    pub row_faults: Vec<WpnRowFault>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WpnRowFault {
    pub row_id: i64,
    pub reason: WpnRowFaultReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WpnRowFaultReason {
    PayloadInvalid,
    PayloadTooLarge,
    TextTooLarge,
    ArrivalInvalid,
}

#[derive(Clone, Debug)]
pub struct WpnReader {
    path: PathBuf,
}

impl WpnReader {
    pub fn from_local_app_data(local_app_data: &Path) -> Self {
        Self {
            path: local_app_data
                .join("Microsoft")
                .join("Windows")
                .join("Notifications")
                .join("wpndatabase.db"),
        }
    }

    #[cfg(test)]
    fn from_path(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn probe(&self) -> Result<WpnSchema, WpnSourceFault> {
        let connection = self.open_read_only()?;
        probe_connection(&connection)
    }

    pub fn read_after(
        &self,
        cursor: NotificationCursor,
        limit: u32,
        received_at: i64,
    ) -> Result<WpnBatch, WpnSourceFault> {
        if cursor.source_id != WPN_SOURCE_ID
            || cursor.last_row_id < 0
            || cursor.last_updated_at < 0
            || !(1..=MAX_BATCH_SIZE).contains(&limit)
            || received_at <= 0
        {
            return Err(WpnSourceFault::InvalidInput);
        }

        let connection = self.open_read_only()?;
        probe_connection(&connection)?;
        let changes_before = connection.total_changes();
        let data_version = connection
            .query_row("PRAGMA data_version", [], |row| row.get::<_, i64>(0))
            .map_err(map_sqlite_error)?;
        if data_version < 0 {
            return Err(WpnSourceFault::QueryFailed);
        }
        let mut statement = connection
            .prepare(
                r#"SELECT n.Id, n.HandlerId, n.Payload, n.ArrivalTime, COALESCE(h.PrimaryId, '')
                   FROM Notification AS n
                   LEFT JOIN NotificationHandler AS h ON h.RecordId = n.HandlerId
                   WHERE n.Id > ?1
                   ORDER BY n.Id ASC
                   LIMIT ?2"#,
            )
            .map_err(map_sqlite_error)?;
        let rows = statement
            .query_map(
                rusqlite::params![cursor.last_row_id, i64::from(limit)],
                raw_row,
            )
            .map_err(map_sqlite_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite_error)?;
        let batch_was_full = rows.len() == limit as usize;
        let mut last_row_id = cursor.last_row_id;
        let mut items = Vec::with_capacity(rows.len());
        let mut row_faults = Vec::new();
        for row in rows {
            last_row_id = last_row_id.max(row.id);
            let parsed = (|| {
                let payload = row.payload?;
                let arrival = row.arrival?;
                let (title, body) = parse_toast_payload(&payload)?;
                let source_occurred_at = parse_arrival_time(arrival, received_at)?;
                Ok::<_, WpnRowFaultReason>((title, body, source_occurred_at))
            })();
            match parsed {
                Ok((title, body, source_occurred_at)) => {
                    items.push(ImportedNotification {
                        origin: NotificationOrigin::Windows,
                        app_id: row.app_id,
                        source_entity_id: format!("wpn:{}", row.id),
                        source_row_id: Some(row.id),
                        title: Some(title),
                        body: Some(body),
                        message_key: None,
                        message_parameters: None,
                        source_context: None,
                        source_occurred_at,
                        received_at,
                    });
                }
                Err(reason) => row_faults.push(WpnRowFault {
                    row_id: row.id,
                    reason,
                }),
            }
        }
        drop(statement);
        let has_more = if batch_was_full {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM Notification WHERE Id > ?1)",
                    [last_row_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(map_sqlite_error)?
        } else {
            false
        };
        if connection.total_changes() != changes_before {
            return Err(WpnSourceFault::QueryFailed);
        }
        Ok(WpnBatch {
            items,
            cursor: NotificationCursor {
                source_id: WPN_SOURCE_ID.into(),
                last_row_id,
                last_updated_at: data_version,
            },
            has_more,
            row_faults,
        })
    }

    fn open_read_only(&self) -> Result<Connection, WpnSourceFault> {
        match std::fs::metadata(&self.path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Err(WpnSourceFault::Missing),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(WpnSourceFault::Missing);
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(WpnSourceFault::AccessDenied);
            }
            Err(_) => return Err(WpnSourceFault::QueryFailed),
        }
        let connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(map_open_error)?;
        connection
            .busy_timeout(Duration::from_millis(250))
            .map_err(map_sqlite_error)?;
        connection
            .pragma_update(None, "query_only", "ON")
            .map_err(map_sqlite_error)?;
        Ok(connection)
    }
}

fn probe_connection(connection: &Connection) -> Result<WpnSchema, WpnSourceFault> {
    let notification = table_columns(connection, "PRAGMA table_info('Notification')")?;
    let handler = table_columns(connection, "PRAGMA table_info('NotificationHandler')")?;
    if ["Id", "HandlerId", "Payload", "ArrivalTime"]
        .iter()
        .all(|column| notification.contains(*column))
        && ["RecordId", "PrimaryId"]
            .iter()
            .all(|column| handler.contains(*column))
    {
        Ok(WpnSchema::NotificationV1)
    } else {
        Err(WpnSourceFault::SchemaIncompatible)
    }
}

fn table_columns(
    connection: &Connection,
    pragma: &str,
) -> Result<BTreeSet<String>, WpnSourceFault> {
    let mut statement = connection.prepare(pragma).map_err(map_sqlite_error)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(map_sqlite_error)?
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(map_sqlite_error)?;
    Ok(columns)
}

fn map_open_error(error: rusqlite::Error) -> WpnSourceFault {
    match error.sqlite_error_code() {
        Some(rusqlite::ffi::ErrorCode::DatabaseBusy)
        | Some(rusqlite::ffi::ErrorCode::DatabaseLocked) => WpnSourceFault::Locked,
        Some(rusqlite::ffi::ErrorCode::PermissionDenied)
        | Some(rusqlite::ffi::ErrorCode::CannotOpen) => WpnSourceFault::AccessDenied,
        _ => WpnSourceFault::QueryFailed,
    }
}

fn map_sqlite_error(error: rusqlite::Error) -> WpnSourceFault {
    match error.sqlite_error_code() {
        Some(rusqlite::ffi::ErrorCode::DatabaseBusy)
        | Some(rusqlite::ffi::ErrorCode::DatabaseLocked) => WpnSourceFault::Locked,
        Some(rusqlite::ffi::ErrorCode::PermissionDenied) => WpnSourceFault::AccessDenied,
        _ => WpnSourceFault::QueryFailed,
    }
}

struct RawWpnRow {
    id: i64,
    payload: Result<Vec<u8>, WpnRowFaultReason>,
    arrival: Result<Option<RawArrival>, WpnRowFaultReason>,
    app_id: String,
}

fn raw_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawWpnRow> {
    let payload = match row.get_ref(2)? {
        ValueRef::Blob(value) | ValueRef::Text(value) => Ok(value.to_vec()),
        _ => Err(WpnRowFaultReason::PayloadInvalid),
    };
    let arrival = match row.get_ref(3)? {
        ValueRef::Null => Ok(None),
        ValueRef::Integer(value) => Ok(Some(RawArrival::Integer(value))),
        ValueRef::Real(value) => Ok(Some(RawArrival::Real(value))),
        ValueRef::Text(value) => std::str::from_utf8(value)
            .map(|value| Some(RawArrival::Text(value.to_owned())))
            .map_err(|_| WpnRowFaultReason::ArrivalInvalid),
        ValueRef::Blob(_) => Err(WpnRowFaultReason::ArrivalInvalid),
    };
    let app_id = match row.get_ref(4)? {
        ValueRef::Text(value) => std::str::from_utf8(value).ok(),
        _ => None,
    }
    .filter(|value| !value.is_empty())
    .unwrap_or(WINDOWS_UNKNOWN_APP)
    .to_owned();
    Ok(RawWpnRow {
        id: row.get(0)?,
        payload,
        arrival,
        app_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repositories::notifications::NotificationOrigin;
    use rusqlite::{params, Connection};
    use serde::Deserialize;
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::time::SystemTime;

    const KNOWN_SCHEMA: &str = include_str!("../../tests/fixtures/wpn/known-schema.sql");
    const INCOMPATIBLE_SCHEMA: &str =
        include_str!("../../tests/fixtures/wpn/incompatible-schema.sql");
    const TOAST_PAYLOADS: &str = include_str!("../../tests/fixtures/wpn/toast-payloads.json");
    const IMPORT_TIME: i64 = 2_000_000_000_000;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ToastFixture {
        name: String,
        payload: String,
        title: Option<String>,
        body: Option<String>,
    }

    fn database(schema: &str) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("wpndatabase.db");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(schema).unwrap();
        drop(connection);
        (directory, path)
    }

    fn cursor(last_row_id: i64) -> NotificationCursor {
        NotificationCursor {
            source_id: WPN_SOURCE_ID.into(),
            last_row_id,
            last_updated_at: 0,
        }
    }

    fn file_hash(path: &Path) -> Vec<u8> {
        Sha256::digest(fs::read(path).unwrap()).to_vec()
    }

    fn filetime_from_unix_millis(unix_millis: i64) -> i64 {
        (unix_millis + 11_644_473_600_000) * 10_000
    }

    #[test]
    fn resolves_only_the_known_local_wpn_path() {
        let local = Path::new(r"C:\Users\Fixture\AppData\Local");
        assert_eq!(
            WpnReader::from_local_app_data(local).path,
            local
                .join("Microsoft")
                .join("Windows")
                .join("Notifications")
                .join("wpndatabase.db")
        );
    }

    #[test]
    fn probes_required_columns_while_allowing_schema_extensions() {
        let (_known, known_path) = database(KNOWN_SCHEMA);
        assert_eq!(
            WpnReader::from_path(known_path).probe(),
            Ok(WpnSchema::NotificationV1)
        );

        let (_incompatible, incompatible_path) = database(INCOMPATIBLE_SCHEMA);
        assert_eq!(
            WpnReader::from_path(incompatible_path).probe(),
            Err(WpnSourceFault::SchemaIncompatible)
        );

        for (required, renamed) in [
            (
                "CREATE TABLE Notification (\n    Id INTEGER PRIMARY KEY,",
                "CREATE TABLE Notification (\n    RenamedId INTEGER PRIMARY KEY,",
            ),
            (
                "    HandlerId INTEGER NOT NULL,",
                "    RenamedHandlerId INTEGER NOT NULL,",
            ),
            (
                "    Payload BLOB NOT NULL,",
                "    RenamedPayload BLOB NOT NULL,",
            ),
            ("    ArrivalTime\n);", "    RenamedArrivalTime\n);"),
            (
                "    RecordId INTEGER PRIMARY KEY,",
                "    RenamedRecordId INTEGER PRIMARY KEY,",
            ),
            ("    PrimaryId TEXT,", "    RenamedPrimaryId TEXT,"),
        ] {
            let schema = KNOWN_SCHEMA.replacen(required, renamed, 1);
            assert_ne!(
                schema, KNOWN_SCHEMA,
                "fixture contract drifted for {required}"
            );
            let (_directory, path) = database(&schema);
            assert_eq!(
                WpnReader::from_path(path).probe(),
                Err(WpnSourceFault::SchemaIncompatible),
                "renaming {required} must be rejected"
            );
        }
    }

    #[test]
    fn missing_source_is_typed_and_never_created() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing.db");
        assert_eq!(
            WpnReader::from_path(path.clone()).probe(),
            Err(WpnSourceFault::Missing)
        );
        assert!(!path.exists());
    }

    #[test]
    fn parses_bounded_text_and_builtin_entities_but_rejects_dtd_and_custom_entities() {
        let fixtures: Vec<ToastFixture> = serde_json::from_str(TOAST_PAYLOADS).unwrap();
        let valid = fixtures
            .iter()
            .find(|fixture| fixture.name == "textAndEntities")
            .unwrap();
        assert_eq!(
            parse_toast_payload(valid.payload.as_bytes()),
            Ok((valid.title.clone().unwrap(), valid.body.clone().unwrap()))
        );
        for name in ["doctype", "customEntity"] {
            let fixture = fixtures
                .iter()
                .find(|fixture| fixture.name == name)
                .unwrap();
            assert_eq!(
                parse_toast_payload(fixture.payload.as_bytes()),
                Err(WpnRowFaultReason::PayloadInvalid)
            );
        }
        let oversized_payload = vec![b'x'; MAX_XML_BYTES + 1];
        assert_eq!(
            parse_toast_payload(&oversized_payload),
            Err(WpnRowFaultReason::PayloadTooLarge)
        );
        let oversized_title = format!("<toast><text>{}</text></toast>", "界".repeat(4_097));
        assert_eq!(
            parse_toast_payload(oversized_title.as_bytes()),
            Err(WpnRowFaultReason::TextTooLarge)
        );
        let oversized_body = format!(
            "<toast><text>Title</text><text>{}</text></toast>",
            "界".repeat(4_097)
        );
        assert_eq!(
            parse_toast_payload(oversized_body.as_bytes()),
            Err(WpnRowFaultReason::TextTooLarge)
        );
    }

    #[test]
    fn converts_filetime_and_fixture_unix_millis_but_rejects_every_present_invalid_value() {
        let source_time = 1_700_000_000_123;
        assert_eq!(
            parse_arrival_time(
                Some(RawArrival::Integer(filetime_from_unix_millis(source_time))),
                IMPORT_TIME
            ),
            Ok(source_time)
        );
        assert_eq!(
            parse_arrival_time(Some(RawArrival::Integer(source_time)), IMPORT_TIME),
            Ok(source_time)
        );
        for boundary in [MIN_SOURCE_UNIX_MILLIS, MAX_SOURCE_UNIX_MILLIS_EXCLUSIVE - 1] {
            assert_eq!(
                parse_arrival_time(Some(RawArrival::Integer(boundary)), IMPORT_TIME),
                Ok(boundary)
            );
        }
        assert_eq!(parse_arrival_time(None, IMPORT_TIME), Ok(IMPORT_TIME));
        for invalid in [
            RawArrival::Integer(0),
            RawArrival::Integer(42),
            RawArrival::Integer(MAX_SOURCE_UNIX_MILLIS_EXCLUSIVE),
            RawArrival::Text("not-a-time".into()),
            RawArrival::Real(1_700_000_000_123.0),
        ] {
            assert_eq!(
                parse_arrival_time(Some(invalid), IMPORT_TIME),
                Err(WpnRowFaultReason::ArrivalInvalid)
            );
        }
        assert_eq!(
            parse_arrival_time(None, 0),
            Err(WpnRowFaultReason::ArrivalInvalid)
        );
    }

    #[test]
    fn imports_valid_rows_advances_past_corrupt_rows_and_keeps_import_time_distinct() {
        let (_directory, path) = database(KNOWN_SCHEMA);
        let fixtures: Vec<ToastFixture> = serde_json::from_str(TOAST_PAYLOADS).unwrap();
        let valid = fixtures
            .iter()
            .find(|fixture| fixture.name == "textAndEntities")
            .unwrap();
        let dtd = fixtures
            .iter()
            .find(|fixture| fixture.name == "doctype")
            .unwrap();
        let source_time = 1_700_000_000_123;
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO NotificationHandler(RecordId,PrimaryId) VALUES(1,'com.fixture.app')",
                [],
            )
            .unwrap();
        for (id, payload, arrival) in [
            (
                1_i64,
                valid.payload.as_str(),
                rusqlite::types::Value::Integer(filetime_from_unix_millis(source_time)),
            ),
            (2, valid.payload.as_str(), rusqlite::types::Value::Null),
            (
                3,
                valid.payload.as_str(),
                rusqlite::types::Value::Text("malformed".into()),
            ),
            (
                4,
                dtd.payload.as_str(),
                rusqlite::types::Value::Integer(source_time),
            ),
        ] {
            connection
                .execute(
                    "INSERT INTO Notification(Id,HandlerId,Payload,ArrivalTime) VALUES(?1,1,?2,?3)",
                    params![id, payload, arrival],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO Notification(Id,HandlerId,Payload,ArrivalTime) VALUES(5,1,?1,?2)",
                params![vec![b'x'; MAX_XML_BYTES + 1], source_time],
            )
            .unwrap();
        connection
            .execute("UPDATE Notification SET HandlerId=99 WHERE Id=2", [])
            .unwrap();
        drop(connection);

        let data_version = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap()
        .query_row("PRAGMA data_version", [], |row| row.get::<_, i64>(0))
        .unwrap();

        let batch = WpnReader::from_path(path)
            .read_after(cursor(0), 200, IMPORT_TIME)
            .unwrap();
        assert_eq!(batch.cursor.source_id, WPN_SOURCE_ID);
        assert_eq!(batch.cursor.last_row_id, 5);
        assert_eq!(batch.cursor.last_updated_at, data_version);
        assert!(!batch.has_more);
        assert_eq!(
            batch.row_faults,
            vec![
                WpnRowFault {
                    row_id: 3,
                    reason: WpnRowFaultReason::ArrivalInvalid,
                },
                WpnRowFault {
                    row_id: 4,
                    reason: WpnRowFaultReason::PayloadInvalid,
                },
                WpnRowFault {
                    row_id: 5,
                    reason: WpnRowFaultReason::PayloadTooLarge,
                },
            ]
        );
        assert_eq!(batch.items.len(), 2);
        assert_eq!(batch.items[0].origin, NotificationOrigin::Windows);
        assert_eq!(batch.items[0].source_entity_id, "wpn:1");
        assert_eq!(batch.items[0].source_row_id, Some(1));
        assert_eq!(batch.items[0].app_id, "com.fixture.app");
        assert_eq!(batch.items[0].source_occurred_at, source_time);
        assert_eq!(batch.items[0].received_at, IMPORT_TIME);
        assert_eq!(batch.items[1].source_entity_id, "wpn:2");
        assert_eq!(batch.items[1].app_id, WINDOWS_UNKNOWN_APP);
        assert_eq!(batch.items[1].source_occurred_at, IMPORT_TIME);
        assert_eq!(batch.items[1].received_at, IMPORT_TIME);
    }

    #[test]
    fn bounds_batches_to_two_hundred_and_resumes_from_the_highest_observed_id() {
        let (_directory, path) = database(KNOWN_SCHEMA);
        let connection = Connection::open(&path).unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        for id in 1_i64..=201 {
            transaction
                .execute(
                    "INSERT INTO Notification(Id,HandlerId,Payload,ArrivalTime) VALUES(?1,1,'<toast><text>Title</text></toast>',?2)",
                    params![id, 1_700_000_000_000_i64],
                )
                .unwrap();
        }
        transaction.commit().unwrap();
        drop(connection);
        let reader = WpnReader::from_path(path);

        let first = reader.read_after(cursor(0), 200, IMPORT_TIME).unwrap();
        assert_eq!(first.items.len(), 200);
        assert_eq!(first.cursor.last_row_id, 200);
        assert!(first.has_more);
        let second = reader
            .read_after(first.cursor, 200, IMPORT_TIME + 1)
            .unwrap();
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.cursor.last_row_id, 201);
        assert!(!second.has_more);

        let connection = Connection::open(&reader.path).unwrap();
        connection
            .execute("DELETE FROM Notification WHERE Id=201", [])
            .unwrap();
        drop(connection);
        let exactly_full = reader.read_after(cursor(0), 200, IMPORT_TIME).unwrap();
        assert_eq!(exactly_full.items.len(), 200);
        assert!(!exactly_full.has_more);
    }

    #[test]
    fn rejects_invalid_batch_inputs_before_touching_the_source() {
        let directory = tempfile::tempdir().unwrap();
        let reader = WpnReader::from_path(directory.path().join("missing.db"));
        assert!(matches!(
            reader.read_after(cursor(0), 0, IMPORT_TIME),
            Err(WpnSourceFault::InvalidInput)
        ));
        assert!(matches!(
            reader.read_after(cursor(0), 201, IMPORT_TIME),
            Err(WpnSourceFault::InvalidInput)
        ));
        assert!(matches!(
            reader.read_after(cursor(0), 200, 0),
            Err(WpnSourceFault::InvalidInput)
        ));
        let mut wrong_source = cursor(0);
        wrong_source.source_id = "other".into();
        assert!(matches!(
            reader.read_after(wrong_source, 200, IMPORT_TIME),
            Err(WpnSourceFault::InvalidInput)
        ));
    }

    #[test]
    fn locked_source_returns_a_typed_local_fault_after_the_bounded_wait() {
        let (_directory, path) = database(KNOWN_SCHEMA);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("BEGIN EXCLUSIVE").unwrap();
        assert!(matches!(
            WpnReader::from_path(path).read_after(cursor(0), 200, IMPORT_TIME),
            Err(WpnSourceFault::Locked)
        ));
        connection.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn reader_never_mutates_the_source_file_or_creates_sidecars() {
        let (directory, path) = database(KNOWN_SCHEMA);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO Notification(Id,HandlerId,Payload,ArrivalTime) VALUES(1,1,'<toast><text>Title</text></toast>',NULL)",
                [],
            )
            .unwrap();
        drop(connection);
        let before_hash = file_hash(&path);
        let before_modified = fs::metadata(&path)
            .unwrap()
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH);

        let reader = WpnReader::from_path(path.clone());
        assert_eq!(reader.probe(), Ok(WpnSchema::NotificationV1));
        assert_eq!(
            reader
                .read_after(cursor(0), 200, IMPORT_TIME)
                .unwrap()
                .items
                .len(),
            1
        );

        assert_eq!(file_hash(&path), before_hash);
        assert_eq!(
            fs::metadata(&path)
                .unwrap()
                .modified()
                .unwrap_or(SystemTime::UNIX_EPOCH),
            before_modified
        );
        let read_only = Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        assert_eq!(read_only.total_changes(), 0);
        drop(read_only);
        for suffix in ["-journal", "-wal", "-shm"] {
            assert!(!directory
                .path()
                .join(format!("wpndatabase.db{suffix}"))
                .exists());
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum RawArrival {
    Integer(i64),
    Real(f64),
    Text(String),
}

fn parse_toast_payload(payload: &[u8]) -> Result<(String, String), WpnRowFaultReason> {
    if payload.is_empty() {
        return Err(WpnRowFaultReason::PayloadInvalid);
    }
    if payload.len() > MAX_XML_BYTES {
        return Err(WpnRowFaultReason::PayloadTooLarge);
    }
    let mut reader = Reader::from_reader(payload);
    reader.config_mut().check_end_names = true;
    let mut nodes = Vec::new();
    let mut current = None::<String>;
    let mut text_depth = 0_usize;
    loop {
        match reader
            .read_event()
            .map_err(|_| WpnRowFaultReason::PayloadInvalid)?
        {
            Event::Start(event) => {
                if current.is_some() {
                    text_depth = text_depth.saturating_add(1);
                } else if event.local_name().as_ref() == b"text" {
                    current = Some(String::new());
                    text_depth = 1;
                }
            }
            Event::Empty(_) => {}
            Event::End(_) if current.is_some() => {
                text_depth = text_depth
                    .checked_sub(1)
                    .ok_or(WpnRowFaultReason::PayloadInvalid)?;
                if text_depth == 0 {
                    let value = current
                        .take()
                        .ok_or(WpnRowFaultReason::PayloadInvalid)?
                        .trim()
                        .to_owned();
                    if !value.is_empty() {
                        nodes.push(value);
                    }
                }
            }
            Event::Text(event) if current.is_some() => {
                let decoded = event
                    .xml10_content()
                    .map_err(|_| WpnRowFaultReason::PayloadInvalid)?;
                append_text(current.as_mut().unwrap(), &decoded)?;
            }
            Event::CData(event) if current.is_some() => {
                let decoded = event
                    .decode()
                    .map_err(|_| WpnRowFaultReason::PayloadInvalid)?;
                append_text(current.as_mut().unwrap(), &decoded)?;
            }
            Event::GeneralRef(reference) => {
                let decoded = decode_reference(&reference)?;
                if let Some(current) = current.as_mut() {
                    append_text(current, &decoded)?;
                }
            }
            Event::DocType(_) => return Err(WpnRowFaultReason::PayloadInvalid),
            Event::Eof => break,
            _ => {}
        }
    }
    if current.is_some() || nodes.is_empty() {
        return Err(WpnRowFaultReason::PayloadInvalid);
    }
    let title = nodes.remove(0);
    let body = nodes.join("\n");
    if title.chars().count() > MAX_TEXT_CHARS || body.chars().count() > MAX_TEXT_CHARS {
        return Err(WpnRowFaultReason::TextTooLarge);
    }
    Ok((title, body))
}

fn parse_arrival_time(
    arrival: Option<RawArrival>,
    received_at: i64,
) -> Result<i64, WpnRowFaultReason> {
    let Some(arrival) = arrival else {
        return if received_at > 0 {
            Ok(received_at)
        } else {
            Err(WpnRowFaultReason::ArrivalInvalid)
        };
    };
    let value = match arrival {
        RawArrival::Integer(value) => value,
        RawArrival::Text(value) => value
            .trim()
            .parse::<i64>()
            .map_err(|_| WpnRowFaultReason::ArrivalInvalid)?,
        RawArrival::Real(_) => return Err(WpnRowFaultReason::ArrivalInvalid),
    };
    if (MIN_SOURCE_UNIX_MILLIS..MAX_SOURCE_UNIX_MILLIS_EXCLUSIVE).contains(&value) {
        return Ok(value);
    }
    let unix_millis = value
        .checked_sub(FILETIME_UNIX_OFFSET_TICKS)
        .map(|ticks| ticks / FILETIME_TICKS_PER_MILLISECOND)
        .ok_or(WpnRowFaultReason::ArrivalInvalid)?;
    if (MIN_SOURCE_UNIX_MILLIS..MAX_SOURCE_UNIX_MILLIS_EXCLUSIVE).contains(&unix_millis) {
        Ok(unix_millis)
    } else {
        Err(WpnRowFaultReason::ArrivalInvalid)
    }
}

fn append_text(target: &mut String, value: &str) -> Result<(), WpnRowFaultReason> {
    if !value.chars().all(valid_xml_character) {
        return Err(WpnRowFaultReason::PayloadInvalid);
    }
    target.push_str(value);
    if target.chars().count() > MAX_TEXT_CHARS {
        return Err(WpnRowFaultReason::TextTooLarge);
    }
    Ok(())
}

fn decode_reference(reference: &BytesRef<'_>) -> Result<String, WpnRowFaultReason> {
    if let Some(character) = reference
        .resolve_char_ref()
        .map_err(|_| WpnRowFaultReason::PayloadInvalid)?
    {
        return if valid_xml_character(character) {
            Ok(character.to_string())
        } else {
            Err(WpnRowFaultReason::PayloadInvalid)
        };
    }
    let name = reference
        .decode()
        .map_err(|_| WpnRowFaultReason::PayloadInvalid)?;
    match name.as_ref() {
        "amp" => Ok("&".into()),
        "lt" => Ok("<".into()),
        "gt" => Ok(">".into()),
        "apos" => Ok("'".into()),
        "quot" => Ok("\"".into()),
        _ => Err(WpnRowFaultReason::PayloadInvalid),
    }
}

fn valid_xml_character(value: char) -> bool {
    matches!(value, '\u{9}' | '\u{A}' | '\u{D}')
        || ('\u{20}'..='\u{D7FF}').contains(&value)
        || ('\u{E000}'..='\u{FFFD}').contains(&value)
        || ('\u{10000}'..='\u{10FFFF}').contains(&value)
}
