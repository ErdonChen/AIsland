use crate::contracts::{
    AppErrorCode, ClearResult, CommandError, DeleteResult, ListNotificationHistoryInput,
    MessageParameterContract, MessageUsage, NotificationHistoryItem,
    NotificationOrigin as ContractOrigin, ReminderSourceContext, SafeMessageParameters,
    TrueLiteral,
};
use crate::domain::reminders::source_context_is_valid;
use crate::storage::Storage;
use rusqlite::{params, OptionalExtension, Row};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct NotificationRepository {
    storage: Arc<Storage>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationOrigin {
    Windows,
    Aiceland,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationCursor {
    pub source_id: String,
    pub last_row_id: i64,
    pub last_updated_at: i64,
}
#[derive(Clone, Debug)]
pub struct ImportedNotification {
    pub origin: NotificationOrigin,
    pub app_id: String,
    pub source_entity_id: String,
    pub source_row_id: Option<i64>,
    pub title: Option<String>,
    pub body: Option<String>,
    pub message_key: Option<String>,
    pub message_parameters: Option<SafeMessageParameters>,
    pub source_context: Option<ReminderSourceContext>,
    pub source_occurred_at: i64,
    pub received_at: i64,
}
impl NotificationRepository {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    pub fn cursor(&self, source_id: &str) -> Result<NotificationCursor, CommandError> {
        if !valid_identifier(source_id, 100) {
            return Err(invalid_input());
        }
        self.storage.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT source_id, last_row_id, last_updated_at FROM notification_cursors WHERE source_id = ?1",
                    [source_id],
                    |row| {
                        Ok(NotificationCursor {
                            source_id: row.get(0)?,
                            last_row_id: row.get(1)?,
                            last_updated_at: row.get(2)?,
                        })
                    },
                )
                .optional()
                .map(|cursor| {
                    cursor.unwrap_or_else(|| NotificationCursor {
                        source_id: source_id.to_string(),
                        last_row_id: 0,
                        last_updated_at: 0,
                    })
                })
                .map_err(Into::into)
        })
    }
    pub fn import(
        &self,
        items: &[ImportedNotification],
        cursor: NotificationCursor,
        now: i64,
    ) -> Result<usize, CommandError> {
        validate_import(items, &cursor, now)?;
        self.storage.with_transaction(|tx| {
            let mut count = 0;
            for item in items {
                let parameters = item
                    .message_parameters
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|_| invalid_input())?;
                let context = item
                    .source_context
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|_| invalid_input())?;
                count += tx.execute(
                    "INSERT INTO notification_history(id,origin,app_id,source_entity_id,source_row_id,title,body,message_key,message_parameters_json,source_context_json,source_occurred_at,received_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12) ON CONFLICT(origin,source_entity_id) DO UPDATE SET app_id=excluded.app_id,source_row_id=excluded.source_row_id,title=excluded.title,body=excluded.body,message_key=excluded.message_key,message_parameters_json=excluded.message_parameters_json,source_context_json=excluded.source_context_json,source_occurred_at=excluded.source_occurred_at,received_at=excluded.received_at",
                    params![Uuid::new_v4().to_string(),origin_name(item.origin),item.app_id,item.source_entity_id,item.source_row_id,item.title,item.body,item.message_key,parameters,context,item.source_occurred_at,item.received_at],
                )?;
            }
            tx.execute(
                "INSERT INTO notification_cursors(source_id,last_row_id,last_updated_at,updated_at) VALUES(?1,?2,?3,?4) ON CONFLICT(source_id) DO UPDATE SET last_row_id=excluded.last_row_id,last_updated_at=excluded.last_updated_at,updated_at=excluded.updated_at",
                params![cursor.source_id,cursor.last_row_id,cursor.last_updated_at,now],
            )?;
            Ok(count)
        })
    }
    pub fn list(
        &self,
        input: ListNotificationHistoryInput,
    ) -> Result<Vec<NotificationHistoryItem>, CommandError> {
        if input.limit < 1
            || input.limit > 500
            || input
                .source_app
                .as_deref()
                .is_some_and(|value| !valid_identifier(value, 260) || value.trim() != value)
        {
            return Err(invalid_input());
        };
        let origin = match input.origin {
            crate::contracts::NotificationOriginFilter::All => "all",
            crate::contracts::NotificationOriginFilter::Windows => "windows",
            crate::contracts::NotificationOriginFilter::Aiceland => "aiceland",
        };
        self.storage.with_connection(|c|{let mut s=c.prepare("SELECT id,origin,app_id,source_entity_id,title,body,message_key,message_parameters_json,source_context_json,source_occurred_at,received_at,read_at FROM notification_history WHERE removed_at IS NULL AND (?1='all' OR origin=?1) AND (?2 IS NULL OR app_id=?2) AND (?3=0 OR read_at IS NULL) ORDER BY received_at DESC,id DESC LIMIT ?4")?;let rows=s.query_map(params![origin,input.source_app,input.unread_only,input.limit],row_to_item)?.collect::<Result<Vec<_>,_>>()?;Ok(rows)})
    }
    pub fn set_read(
        &self,
        id: Uuid,
        read: bool,
        now: i64,
    ) -> Result<NotificationHistoryItem, CommandError> {
        if now < 0 {
            return Err(invalid_input());
        };
        self.storage.with_transaction(|tx|{let value=id.to_string();let n=tx.execute("UPDATE notification_history SET read_at=CASE WHEN ?2 THEN ?3 ELSE NULL END WHERE id=?1 AND removed_at IS NULL",params![value,read,now])?;if n==0{return Err(not_found())};tx.query_row("SELECT id,origin,app_id,source_entity_id,title,body,message_key,message_parameters_json,source_context_json,source_occurred_at,received_at,read_at FROM notification_history WHERE id=?1",[value],row_to_item).map_err(Into::into)})
    }
    pub fn mark_removed(&self, id: Uuid, now: i64) -> Result<DeleteResult, CommandError> {
        if now < 0 {
            return Err(invalid_input());
        };
        self.storage.with_transaction(|tx| {
            let value = id.to_string();
            let n = tx.execute(
                "UPDATE notification_history SET removed_at=?2 WHERE id=?1 AND removed_at IS NULL",
                params![value, now],
            )?;
            if n == 0 {
                return Err(not_found());
            };
            Ok(DeleteResult {
                id: value,
                deleted: TrueLiteral,
            })
        })
    }
    pub fn clear(&self, before: Option<i64>, now: i64) -> Result<ClearResult, CommandError> {
        if now < 0 || before.is_some_and(|v| v < 0) {
            return Err(invalid_input());
        };
        self.storage.with_transaction(|tx|{let n=tx.execute("UPDATE notification_history SET removed_at=?2 WHERE removed_at IS NULL AND (?1 IS NULL OR received_at < ?1)",params![before,now])?;Ok(ClearResult{removed_count:n as i64})})
    }
}
fn validate_import(
    items: &[ImportedNotification],
    cursor: &NotificationCursor,
    now: i64,
) -> Result<(), CommandError> {
    if now < 0
        || !valid_identifier(&cursor.source_id, 100)
        || cursor.last_row_id < 0
        || cursor.last_updated_at < 0
        || items.iter().any(|item| !valid_import(item))
    {
        return Err(invalid_input());
    }
    for item in items {
        if let NotificationOrigin::Aiceland = item.origin {
            MessageParameterContract::validate_for(
                MessageUsage::ReminderDisplay,
                item.message_key.as_deref().ok_or_else(invalid_input)?,
                item.message_parameters.as_ref().ok_or_else(invalid_input)?,
            )?;
        }
    }
    Ok(())
}

fn valid_import(i: &ImportedNotification) -> bool {
    if !valid_identifier(&i.app_id, 512)
        || !valid_identifier(&i.source_entity_id, 512)
        || i.source_row_id.is_some_and(|v| v < 0)
        || i.source_occurred_at <= 0
        || i.received_at < 0
    {
        return false;
    }
    match i.origin {
        NotificationOrigin::Windows => {
            i.title.as_deref().is_some_and(valid_display_text)
                && i.body.as_deref().is_some_and(valid_display_text)
                && i.message_key.is_none()
                && i.message_parameters.is_none()
                && i.source_context.is_none()
        }
        NotificationOrigin::Aiceland => {
            let (Some(key), Some(parameters), Some(context)) =
                (&i.message_key, &i.message_parameters, &i.source_context)
            else {
                return false;
            };
            i.title.is_none()
                && i.body.is_none()
                && valid_identifier(key, 200)
                && source_context_is_valid(context)
                && source_context_occurred_at(context) == i.source_occurred_at
                && MessageParameterContract::validate_for(
                    MessageUsage::ReminderDisplay,
                    key,
                    parameters,
                )
                .is_ok()
        }
    }
}

fn valid_identifier(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && !value.chars().any(|character| character.is_control())
}

fn valid_display_text(value: &str) -> bool {
    value.chars().count() <= 4_096 && !value.chars().any(|character| character == '\0')
}

fn source_context_occurred_at(context: &ReminderSourceContext) -> i64 {
    match context {
        ReminderSourceContext::Agent {
            source_occurred_at, ..
        }
        | ReminderSourceContext::Todo {
            source_occurred_at, ..
        }
        | ReminderSourceContext::Monitor {
            source_occurred_at, ..
        } => *source_occurred_at,
    }
}
fn origin_name(v: NotificationOrigin) -> &'static str {
    match v {
        NotificationOrigin::Windows => "windows",
        NotificationOrigin::Aiceland => "aiceland",
    }
}
fn row_to_item(r: &Row<'_>) -> rusqlite::Result<NotificationHistoryItem> {
    let origin: String = r.get(1)?;
    let params_json: Option<String> = r.get(7)?;
    let context_json: Option<String> = r.get(8)?;
    Ok(NotificationHistoryItem {
        id: r.get(0)?,
        origin: match origin.as_str() {
            "windows" => ContractOrigin::Windows,
            "aiceland" => ContractOrigin::Aiceland,
            _ => return Err(rusqlite::Error::InvalidQuery),
        },
        app_id: r.get(2)?,
        source_entity_id: r.get(3)?,
        title: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
        body: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
        message_key: r.get(6)?,
        message_parameters: params_json
            .map(|v| serde_json::from_str(&v).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?
            .unwrap_or_default(),
        source_context: context_json
            .map(|v| serde_json::from_str(&v).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        source_occurred_at: r.get(9)?,
        received_at: r.get(10)?,
        read_at: r.get(11)?,
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
    use super::*;
    use crate::contracts::{ListNotificationHistoryInput, NotificationOriginFilter};
    use crate::storage::Storage;
    use std::sync::Arc;

    fn repository() -> NotificationRepository {
        let d = tempfile::tempdir().unwrap();
        let repository = NotificationRepository::new(Arc::new(Storage::open(d.path()).unwrap()));
        std::mem::forget(d);
        repository
    }
    fn windows(id: &str, app: &str, occurred: i64, received: i64) -> ImportedNotification {
        ImportedNotification {
            origin: NotificationOrigin::Windows,
            app_id: app.into(),
            source_entity_id: id.into(),
            source_row_id: Some(1),
            title: Some("title".into()),
            body: Some("body".into()),
            message_key: None,
            message_parameters: None,
            source_context: None,
            source_occurred_at: occurred,
            received_at: received,
        }
    }

    #[test]
    fn migration_five_creates_notification_history() {
        let d = tempfile::tempdir().unwrap();
        let s = Storage::open(d.path()).unwrap();
        s.with_connection(|c|{let e:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='notification_history')",[],|r|r.get(0))?;assert!(e);Ok(())}).unwrap();
    }

    // Break caught: invalid imported timestamps must fail before the row or cursor reaches SQLite.
    #[test]
    fn invalid_import_leaves_history_and_cursor_unchanged() {
        let repo = repository();
        assert!(repo
            .import(
                &[windows("one", "alpha", 0, 2)],
                NotificationCursor {
                    source_id: "windows".into(),
                    last_row_id: 1,
                    last_updated_at: 2
                },
                2
            )
            .is_err());
        repo.storage
            .with_connection(|c| {
                assert_eq!(
                    c.query_row("SELECT COUNT(*) FROM notification_history", [], |r| r
                        .get::<_, i64>(0))?,
                    0
                );
                assert_eq!(
                    c.query_row("SELECT COUNT(*) FROM notification_cursors", [], |r| r
                        .get::<_, i64>(0))?,
                    0
                );
                Ok(())
            })
            .unwrap();
    }

    // Break caught: duplicate imports preserve local read markers while filters combine.
    #[test]
    fn duplicate_import_preserves_read_and_combines_filters() {
        let repo = repository();
        repo.import(
            &[
                windows("one", "alpha", 10, 20),
                windows("two", "beta", 30, 40),
            ],
            NotificationCursor {
                source_id: "windows".into(),
                last_row_id: 1,
                last_updated_at: 2,
            },
            2,
        )
        .unwrap();
        let item = repo
            .list(ListNotificationHistoryInput {
                origin: NotificationOriginFilter::Windows,
                source_app: Some("alpha".into()),
                unread_only: false,
                limit: 10,
            })
            .unwrap()
            .pop()
            .unwrap();
        repo.set_read(Uuid::parse_str(&item.id).unwrap(), true, 60)
            .unwrap();
        let mut replacement = windows("one", "alpha", 11, 21);
        replacement.title = Some("updated".into());
        repo.import(
            &[replacement],
            NotificationCursor {
                source_id: "windows".into(),
                last_row_id: 2,
                last_updated_at: 3,
            },
            3,
        )
        .unwrap();
        assert!(repo
            .list(ListNotificationHistoryInput {
                origin: NotificationOriginFilter::Windows,
                source_app: Some("alpha".into()),
                unread_only: true,
                limit: 10
            })
            .unwrap()
            .is_empty());
        let saved = repo
            .list(ListNotificationHistoryInput {
                origin: NotificationOriginFilter::Windows,
                source_app: Some("alpha".into()),
                unread_only: false,
                limit: 10,
            })
            .unwrap();
        assert_eq!(
            (
                saved[0].title.as_str(),
                saved[0].source_occurred_at,
                saved[0].received_at,
                saved[0].read_at
            ),
            ("updated", 11, 21, Some(60))
        );
    }

    #[test]
    fn timestamps_and_local_markers_survive_imports_without_touching_source_database() {
        let repo = repository();
        let source_dir = tempfile::tempdir().unwrap();
        let source_path = source_dir.path().join("source.sqlite3");
        let source = rusqlite::Connection::open(&source_path).unwrap();
        source.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE source_rows(id TEXT PRIMARY KEY, app TEXT NOT NULL, arrival INTEGER); INSERT INTO source_rows VALUES ('arrival','alpha',1700000000000),('missing','alpha',NULL);").unwrap();
        let source_artifacts = || {
            [
                source_path.clone(),
                std::path::PathBuf::from(format!("{}-wal", source_path.display())),
                std::path::PathBuf::from(format!("{}-shm", source_path.display())),
            ]
            .into_iter()
            .map(|path| {
                let metadata = std::fs::metadata(&path).unwrap();
                let contents = std::fs::read(&path).unwrap();
                (path, contents, metadata.len(), metadata.modified().unwrap())
            })
            .collect::<Vec<_>>()
        };
        let source_rows: Vec<(String, String, Option<i64>)> = {
            let mut statement = source
                .prepare("SELECT id, app, arrival FROM source_rows ORDER BY id")
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        let source_before = (source_artifacts(), source.total_changes());
        let arrival_converted = windows(
            &source_rows[0].0,
            &source_rows[0].1,
            source_rows[0].2.unwrap(),
            10,
        );
        let missing_arrival = windows(
            &source_rows[1].0,
            &source_rows[1].1,
            source_rows[1].2.unwrap_or(20),
            20,
        );
        repo.import(
            &[arrival_converted, missing_arrival],
            NotificationCursor {
                source_id: "windows".into(),
                last_row_id: 2,
                last_updated_at: 20,
            },
            20,
        )
        .unwrap();
        assert_eq!((source_artifacts(), source.total_changes()), source_before);
        let items = repo
            .list(ListNotificationHistoryInput {
                origin: NotificationOriginFilter::Windows,
                source_app: Some("alpha".into()),
                unread_only: false,
                limit: 10,
            })
            .unwrap();
        assert!(items
            .iter()
            .any(|v| v.source_occurred_at == 1_700_000_000_000 && v.received_at == 10));
        assert!(items
            .iter()
            .any(|v| v.source_occurred_at == v.received_at && v.source_occurred_at == 20));
        let arrival = items
            .iter()
            .find(|item| item.source_entity_id == "arrival")
            .unwrap();
        repo.mark_removed(Uuid::parse_str(&arrival.id).unwrap(), 30)
            .unwrap();
        let mut duplicate = windows("arrival", "alpha", 1_700_000_000_001, 31);
        duplicate.title = Some("changed".into());
        repo.import(
            &[duplicate],
            NotificationCursor {
                source_id: "windows".into(),
                last_row_id: 3,
                last_updated_at: 31,
            },
            31,
        )
        .unwrap();
        repo.storage.with_connection(|c| { assert_eq!(c.query_row("SELECT removed_at FROM notification_history WHERE source_entity_id='arrival'", [], |r| r.get::<_, Option<i64>>(0))?, Some(30)); Ok(()) }).unwrap();
        let storage_path = repo.storage.path().to_path_buf();
        drop(repo);
        let reopened = NotificationRepository::new(Arc::new(
            Storage::open(storage_path.parent().unwrap()).unwrap(),
        ));
        let reopened_items = reopened
            .list(ListNotificationHistoryInput {
                origin: NotificationOriginFilter::Windows,
                source_app: Some("alpha".into()),
                unread_only: false,
                limit: 10,
            })
            .unwrap();
        assert!(reopened_items
            .iter()
            .any(|item| item.source_entity_id == "missing"
                && item.source_occurred_at == 20
                && item.received_at == 20));
        assert_eq!(reopened.clear(None, 32).unwrap().removed_count, 1);
        reopened
            .storage
            .with_connection(|c| {
                assert_eq!(
                    c.query_row("SELECT COUNT(*) FROM notification_history", [], |r| r
                        .get::<_, i64>(0))?,
                    2
                );
                assert_eq!(
                    c.query_row(
                        "SELECT COUNT(*) FROM notification_history WHERE removed_at IS NOT NULL",
                        [],
                        |r| r.get::<_, i64>(0)
                    )?,
                    2
                );
                Ok(())
            })
            .unwrap();
        for (occurred, received) in [(0, 1), (-1, 1), (1, -1)] {
            assert!(reopened
                .import(
                    &[windows("bad", "alpha", occurred, received)],
                    NotificationCursor {
                        source_id: "bad".into(),
                        last_row_id: 0,
                        last_updated_at: 0
                    },
                    33
                )
                .is_err());
        }
    }
}
