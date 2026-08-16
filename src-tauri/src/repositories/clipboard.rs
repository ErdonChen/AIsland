use crate::contracts::{
    AppErrorCode, ClearResult, ClipboardContentKind, ClipboardContentKindFilter, ClipboardItem,
    CommandError, DeleteResult, ListClipboardItemsInput, SafeMessageParameters, TrueLiteral,
};
use crate::domain::clipboard::{
    content_kind_name, validate_sha256_hex, CaptureOutcome, ClipboardAssetRecord,
    ClipboardRetentionPolicy, NewClipboardAsset, MAX_IMAGE_DIMENSION, MAX_IMAGE_PNG_BYTES,
    MAX_TEXT_BYTES,
};
use crate::storage::Storage;
use rusqlite::{params, OptionalExtension, Row};
use std::collections::BTreeSet;
use std::sync::Arc;
use uuid::Uuid;

const ITEM_FIELDS: &str = "i.id, i.content_kind, i.text_content, a.id, i.source_app, i.pinned, i.captured_at, i.last_seen_at, i.byte_size";

#[derive(Clone)]
pub struct ClipboardRepository {
    storage: Arc<Storage>,
    retention: Arc<dyn ClipboardRetentionPolicy>,
}

impl ClipboardRepository {
    pub fn new(storage: Arc<Storage>, retention: Arc<dyn ClipboardRetentionPolicy>) -> Self {
        Self { storage, retention }
    }

    pub fn list(&self, input: ListClipboardItemsInput) -> Result<Vec<ClipboardItem>, CommandError> {
        if input.query.chars().count() > 500 || !(1..=500).contains(&input.limit) {
            return Err(invalid_input());
        }
        let kind = match input.content_kind {
            ClipboardContentKindFilter::All => "all",
            ClipboardContentKindFilter::Text => "text",
            ClipboardContentKindFilter::Image => "image",
        };
        let query = format!(
            r#"SELECT {ITEM_FIELDS}
               FROM clipboard_items i
               LEFT JOIN clipboard_assets a ON a.clipboard_item_id = i.id
               WHERE (?1 = '' OR instr(lower(COALESCE(i.text_content, '')), lower(?1)) > 0
                      OR instr(lower(COALESCE(i.source_app, '')), lower(?1)) > 0)
                 AND (?2 = 'all' OR i.content_kind = ?2)
               ORDER BY i.pinned DESC, i.last_seen_at DESC, i.id ASC
               LIMIT ?3"#
        );
        self.storage.with_connection(|connection| {
            let mut statement = connection.prepare(&query)?;
            let rows = statement
                .query_map(params![input.query, kind, input.limit], row_to_item)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn get(&self, id: Uuid) -> Result<ClipboardItem, CommandError> {
        self.storage.with_connection(|connection| {
            get_item(connection, &id.to_string())?.ok_or_else(not_found)
        })
    }

    pub fn get_asset(&self, asset_id: Uuid) -> Result<ClipboardAssetRecord, CommandError> {
        self.storage.with_connection(|connection| {
            connection
                .query_row(
                    r#"SELECT id, clipboard_item_id, asset_name, width, height, sha256, byte_size
                       FROM clipboard_assets WHERE id = ?1"#,
                    [asset_id.to_string()],
                    row_to_asset,
                )
                .optional()?
                .ok_or_else(not_found)
        })
    }

    pub fn referenced_asset_names(&self) -> Result<BTreeSet<String>, CommandError> {
        self.storage.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT asset_name FROM clipboard_assets ORDER BY asset_name")?;
            let rows = statement
                .query_map([], |row| row.get(0))?
                .collect::<Result<BTreeSet<String>, _>>()?;
            Ok(rows)
        })
    }

    pub fn find_by_hash(
        &self,
        kind: ClipboardContentKind,
        sha256: &str,
    ) -> Result<Option<ClipboardItem>, CommandError> {
        validate_sha256_hex(sha256)?;
        self.storage.with_connection(|connection| {
            find_item_by_hash(connection, content_kind_name(kind), sha256)
        })
    }

    pub fn insert_text(
        &self,
        id: Uuid,
        text: &str,
        sha256: &str,
        source_app: Option<&str>,
        now: i64,
    ) -> Result<CaptureOutcome, CommandError> {
        if now < 0 || text.len() > MAX_TEXT_BYTES {
            return Err(invalid_input());
        }
        validate_source_app(source_app)?;
        validate_sha256_hex(sha256)?;
        let id = id.to_string();
        self.storage.with_transaction(|transaction| {
            if let Some(existing) = find_item_by_hash(transaction, "text", sha256)? {
                transaction.execute(
                    r#"UPDATE clipboard_items
                       SET last_seen_at = ?2, source_app = COALESCE(?3, source_app)
                       WHERE id = ?1"#,
                    params![existing.id, now, source_app],
                )?;
                let item = get_item(transaction, &existing.id)?.ok_or_else(database_failure)?;
                let limit = self.retention.unpinned_limit()?;
                let removed_asset_names = apply_retention(transaction, limit)?;
                return Ok(CaptureOutcome {
                    item,
                    inserted: false,
                    removed_asset_names,
                });
            }
            transaction.execute(
                r#"INSERT INTO clipboard_items(
                       id, content_kind, text_content, content_sha256, source_app,
                       pinned, captured_at, last_seen_at, byte_size
                   ) VALUES (?1, 'text', ?2, ?3, ?4, 0, ?5, ?5, ?6)"#,
                params![id, text, sha256, source_app, now, text.len() as i64],
            )?;
            let item = get_item(transaction, &id)?.ok_or_else(database_failure)?;
            let limit = self.retention.unpinned_limit()?;
            let removed_asset_names = apply_retention(transaction, limit)?;
            Ok(CaptureOutcome {
                item,
                inserted: true,
                removed_asset_names,
            })
        })
    }

    pub fn insert_image_metadata(
        &self,
        item_id: Uuid,
        asset: NewClipboardAsset,
        source_app: Option<&str>,
        now: i64,
    ) -> Result<CaptureOutcome, CommandError> {
        if now < 0
            || asset.width == 0
            || asset.height == 0
            || asset.width > MAX_IMAGE_DIMENSION
            || asset.height > MAX_IMAGE_DIMENSION
            || asset.byte_size == 0
            || asset.byte_size > MAX_IMAGE_PNG_BYTES as u64
            || asset.asset_name != format!("{}.png", asset.id)
        {
            return Err(invalid_input());
        }
        validate_source_app(source_app)?;
        validate_sha256_hex(&asset.sha256)?;
        let item_id = item_id.to_string();
        self.storage.with_transaction(|transaction| {
            if let Some(existing) = find_item_by_hash(transaction, "image", &asset.sha256)? {
                transaction.execute(
                    r#"UPDATE clipboard_items
                       SET last_seen_at = ?2, source_app = COALESCE(?3, source_app)
                       WHERE id = ?1"#,
                    params![existing.id, now, source_app],
                )?;
                let item = get_item(transaction, &existing.id)?.ok_or_else(database_failure)?;
                let mut removed_asset_names = vec![asset.asset_name.clone()];
                let limit = self.retention.unpinned_limit()?;
                removed_asset_names.extend(apply_retention(transaction, limit)?);
                return Ok(CaptureOutcome {
                    item,
                    inserted: false,
                    removed_asset_names,
                });
            }
            transaction.execute(
                r#"INSERT INTO clipboard_items(
                       id, content_kind, text_content, content_sha256, source_app,
                       pinned, captured_at, last_seen_at, byte_size
                   ) VALUES (?1, 'image', NULL, ?2, ?3, 0, ?4, ?4, ?5)"#,
                params![
                    item_id,
                    asset.sha256,
                    source_app,
                    now,
                    asset.byte_size as i64
                ],
            )?;
            transaction.execute(
                r#"INSERT INTO clipboard_assets(
                       id, clipboard_item_id, asset_name, mime_type, width, height,
                       sha256, byte_size, created_at
                   ) VALUES (?1, ?2, ?3, 'image/png', ?4, ?5, ?6, ?7, ?8)"#,
                params![
                    asset.id.to_string(),
                    item_id,
                    asset.asset_name,
                    asset.width,
                    asset.height,
                    asset.sha256,
                    asset.byte_size as i64,
                    now,
                ],
            )?;
            let item = get_item(transaction, &item_id)?.ok_or_else(database_failure)?;
            let limit = self.retention.unpinned_limit()?;
            let removed_asset_names = apply_retention(transaction, limit)?;
            Ok(CaptureOutcome {
                item,
                inserted: true,
                removed_asset_names,
            })
        })
    }

    pub fn set_pinned(
        &self,
        id: Uuid,
        pinned: bool,
        now: i64,
    ) -> Result<ClipboardItem, CommandError> {
        if now < 0 {
            return Err(invalid_input());
        }
        let id = id.to_string();
        self.storage.with_transaction(|transaction| {
            let changed = transaction.execute(
                "UPDATE clipboard_items SET pinned = ?2, last_seen_at = MAX(last_seen_at, ?3) WHERE id = ?1",
                params![id, pinned, now],
            )?;
            if changed == 0 {
                return Err(not_found());
            }
            get_item(transaction, &id)?.ok_or_else(database_failure)
        })
    }

    pub fn delete(&self, id: Uuid) -> Result<(DeleteResult, Option<String>), CommandError> {
        let id = id.to_string();
        self.storage.with_transaction(|transaction| {
            let asset_name = transaction
                .query_row(
                    "SELECT asset_name FROM clipboard_assets WHERE clipboard_item_id = ?1",
                    [&id],
                    |row| row.get(0),
                )
                .optional()?;
            let changed =
                transaction.execute("DELETE FROM clipboard_items WHERE id = ?1", [&id])?;
            if changed == 0 {
                return Err(not_found());
            }
            Ok((
                DeleteResult {
                    id,
                    deleted: TrueLiteral,
                },
                asset_name,
            ))
        })
    }

    pub fn clear(&self, keep_pinned: bool) -> Result<(ClearResult, Vec<String>), CommandError> {
        self.storage.with_transaction(|transaction| {
            let condition = if keep_pinned { "WHERE i.pinned = 0" } else { "" };
            let query = format!(
                "SELECT a.asset_name FROM clipboard_items i JOIN clipboard_assets a ON a.clipboard_item_id = i.id {condition} ORDER BY a.asset_name"
            );
            let asset_names = {
                let mut statement = transaction.prepare(&query)?;
                let rows = statement
                    .query_map([], |row| row.get(0))?
                    .collect::<Result<Vec<String>, _>>()?;
                rows
            };
            let removed_count = transaction.execute(
                if keep_pinned {
                    "DELETE FROM clipboard_items WHERE pinned = 0"
                } else {
                    "DELETE FROM clipboard_items"
                },
                [],
            )?;
            Ok((
                ClearResult {
                    removed_count: removed_count as i64,
                },
                asset_names,
            ))
        })
    }
}

fn row_to_item(row: &Row<'_>) -> rusqlite::Result<ClipboardItem> {
    let content_kind = match row.get::<_, String>(1)?.as_str() {
        "text" => ClipboardContentKind::Text,
        "image" => ClipboardContentKind::Image,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(ClipboardItem {
        id: row.get(0)?,
        content_kind,
        text_content: row.get(2)?,
        asset_id: row.get(3)?,
        source_app: row.get(4)?,
        pinned: row.get(5)?,
        captured_at: row.get(6)?,
        last_seen_at: row.get(7)?,
        byte_size: row.get(8)?,
    })
}

fn row_to_asset(row: &Row<'_>) -> rusqlite::Result<ClipboardAssetRecord> {
    let byte_size = u64::try_from(row.get::<_, i64>(6)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            6,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })?;
    Ok(ClipboardAssetRecord {
        id: parse_uuid_column(row, 0)?,
        clipboard_item_id: parse_uuid_column(row, 1)?,
        asset_name: row.get(2)?,
        width: row.get(3)?,
        height: row.get(4)?,
        sha256: row.get(5)?,
        byte_size,
    })
}

fn parse_uuid_column(row: &Row<'_>, index: usize) -> rusqlite::Result<Uuid> {
    let value = row.get::<_, String>(index)?;
    Uuid::parse_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn get_item(
    connection: &rusqlite::Connection,
    id: &str,
) -> Result<Option<ClipboardItem>, CommandError> {
    let query = format!(
        "SELECT {ITEM_FIELDS} FROM clipboard_items i LEFT JOIN clipboard_assets a ON a.clipboard_item_id = i.id WHERE i.id = ?1"
    );
    connection
        .query_row(&query, [id], row_to_item)
        .optional()
        .map_err(Into::into)
}

fn find_item_by_hash(
    connection: &rusqlite::Connection,
    kind: &str,
    sha256: &str,
) -> Result<Option<ClipboardItem>, CommandError> {
    let query = format!(
        "SELECT {ITEM_FIELDS} FROM clipboard_items i LEFT JOIN clipboard_assets a ON a.clipboard_item_id = i.id WHERE i.content_kind = ?1 AND i.content_sha256 = ?2"
    );
    connection
        .query_row(&query, params![kind, sha256], row_to_item)
        .optional()
        .map_err(Into::into)
}

fn apply_retention(
    transaction: &rusqlite::Transaction<'_>,
    limit: u32,
) -> Result<Vec<String>, CommandError> {
    let expired = {
        let mut statement = transaction.prepare(
            r#"SELECT i.id, a.asset_name
               FROM clipboard_items i
               LEFT JOIN clipboard_assets a ON a.clipboard_item_id = i.id
               WHERE i.pinned = 0
               ORDER BY i.last_seen_at DESC, i.id ASC
               LIMIT -1 OFFSET ?1"#,
        )?;
        let rows = statement
            .query_map([i64::from(limit)], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    let mut removed_asset_names = Vec::new();
    for (id, asset_name) in expired {
        transaction.execute("DELETE FROM clipboard_items WHERE id = ?1", [&id])?;
        if let Some(asset_name) = asset_name {
            removed_asset_names.push(asset_name);
        }
    }
    Ok(removed_asset_names)
}

fn validate_source_app(source_app: Option<&str>) -> Result<(), CommandError> {
    if source_app.is_some_and(|value| value.chars().count() > 260) {
        Err(invalid_input())
    } else {
        Ok(())
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

fn not_found() -> CommandError {
    CommandError {
        code: AppErrorCode::NotFound,
        message_key: "errors.notFound".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

fn database_failure() -> CommandError {
    CommandError {
        code: AppErrorCode::DatabaseFailure,
        message_key: "errors.databaseFailure".into(),
        details: SafeMessageParameters::new(),
        retryable: true,
    }
}

#[cfg(test)]
mod tests {
    use super::ClipboardRepository;
    use crate::contracts::{
        AppErrorCode, ClipboardContentKind, ClipboardContentKindFilter, CommandError,
        ListClipboardItemsInput,
    };
    use crate::domain::clipboard::{
        validate_image_capture, validate_text_capture, BootstrapClipboardRetentionPolicy,
        ClipboardRetentionPolicy, NewClipboardAsset, MAX_IMAGE_DIMENSION, MAX_IMAGE_PNG_BYTES,
        MAX_IMAGE_RGBA_BYTES, MAX_TEXT_BYTES,
    };
    use crate::storage::Storage;
    use std::sync::Arc;
    use uuid::Uuid;

    fn fixture() -> (tempfile::TempDir, Arc<Storage>, ClipboardRepository) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(dir.path()).unwrap());
        let repository = ClipboardRepository::new(
            Arc::clone(&storage),
            Arc::new(BootstrapClipboardRetentionPolicy),
        );
        (dir, storage, repository)
    }

    #[test]
    fn duplicate_text_updates_last_seen_without_replacing_identity_or_pin() {
        let (_dir, _storage, repository) = fixture();
        let id = Uuid::new_v4();
        let first = repository
            .insert_text(id, "hello", &"a".repeat(64), Some("first.exe"), 10)
            .unwrap();
        let pinned = repository.set_pinned(id, true, 11).unwrap();
        assert!(pinned.pinned);

        let duplicate = repository
            .insert_text(
                Uuid::new_v4(),
                "hello",
                &"a".repeat(64),
                Some("second.exe"),
                20,
            )
            .unwrap();
        assert_eq!(duplicate.item.id, first.item.id);
        assert!(!duplicate.inserted);
        assert_eq!(duplicate.item.captured_at, 10);
        assert_eq!(duplicate.item.last_seen_at, 20);
        assert!(duplicate.item.pinned);
        assert_eq!(duplicate.item.source_app.as_deref(), Some("second.exe"));
    }

    #[test]
    fn image_hash_dedupes_and_direct_item_delete_cascades_asset_metadata() {
        let (_dir, storage, repository) = fixture();
        let item_id = Uuid::new_v4();
        let asset_id = Uuid::new_v4();
        let first = repository
            .insert_image_metadata(
                item_id,
                NewClipboardAsset {
                    id: asset_id,
                    asset_name: format!("{asset_id}.png"),
                    sha256: "b".repeat(64),
                    width: 2,
                    height: 2,
                    byte_size: 16,
                },
                Some("capture.exe"),
                10,
            )
            .unwrap();
        let duplicate_asset_id = Uuid::new_v4();
        let duplicate = repository
            .insert_image_metadata(
                Uuid::new_v4(),
                NewClipboardAsset {
                    id: duplicate_asset_id,
                    asset_name: format!("{duplicate_asset_id}.png"),
                    sha256: "b".repeat(64),
                    width: 2,
                    height: 2,
                    byte_size: 16,
                },
                Some("second.exe"),
                20,
            )
            .unwrap();
        assert_eq!(duplicate.item.id, first.item.id);
        assert!(!duplicate.inserted);
        assert_eq!(duplicate.item.captured_at, 10);
        assert_eq!(duplicate.item.last_seen_at, 20);
        assert_eq!(duplicate.item.source_app.as_deref(), Some("second.exe"));
        assert_eq!(
            duplicate.removed_asset_names,
            vec![format!("{duplicate_asset_id}.png")]
        );

        storage
            .with_connection(|connection| {
                connection.execute(
                    "DELETE FROM clipboard_items WHERE id = ?1",
                    [&first.item.id],
                )?;
                let count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM clipboard_assets WHERE clipboard_item_id = ?1",
                    [&first.item.id],
                    |row| row.get(0),
                )?;
                assert_eq!(count, 0);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn retention_keeps_every_pin_and_only_the_newest_five_hundred_unpinned() {
        let (_dir, storage, repository) = fixture();
        let pinned_id = Uuid::new_v4();
        repository
            .insert_text(pinned_id, "pinned", &"f".repeat(64), None, 1)
            .unwrap();
        repository.set_pinned(pinned_id, true, 2).unwrap();

        for index in 0..501_u64 {
            repository
                .insert_text(
                    Uuid::new_v4(),
                    &format!("item-{index}"),
                    &format!("{index:064x}"),
                    None,
                    10 + index as i64,
                )
                .unwrap();
        }

        storage
            .with_connection(|connection| {
                let total: i64 =
                    connection
                        .query_row("SELECT COUNT(*) FROM clipboard_items", [], |row| row.get(0))?;
                let unpinned: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM clipboard_items WHERE pinned = 0",
                    [],
                    |row| row.get(0),
                )?;
                let oldest: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM clipboard_items WHERE text_content = 'item-0'",
                    [],
                    |row| row.get(0),
                )?;
                let newest: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM clipboard_items WHERE text_content = 'item-500'",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!((total, unpinned, oldest, newest), (501, 500, 0, 1));
                Ok(())
            })
            .unwrap();
        assert!(repository.get(pinned_id).unwrap().pinned);
    }

    #[test]
    fn exact_capture_limits_accept_maximum_and_reject_plus_one() {
        let text = "x".repeat(MAX_TEXT_BYTES);
        assert_eq!(
            validate_text_capture(&text).unwrap().1,
            MAX_TEXT_BYTES as u64
        );
        assert_eq!(
            validate_text_capture(&(text + "x")).unwrap_err().code,
            AppErrorCode::InvalidInput
        );

        let png = vec![1; MAX_IMAGE_PNG_BYTES];
        assert_eq!(
            validate_image_capture(
                MAX_IMAGE_DIMENSION,
                MAX_IMAGE_DIMENSION,
                MAX_IMAGE_RGBA_BYTES,
                &png,
            )
            .unwrap()
            .1,
            MAX_IMAGE_PNG_BYTES as u64
        );
        for invalid in [
            validate_image_capture(MAX_IMAGE_DIMENSION + 1, 1, MAX_IMAGE_RGBA_BYTES, &png),
            validate_image_capture(1, MAX_IMAGE_DIMENSION + 1, MAX_IMAGE_RGBA_BYTES, &png),
            validate_image_capture(1, 1, MAX_IMAGE_RGBA_BYTES + 1, &png),
            validate_image_capture(1, 1, 4, &vec![1; MAX_IMAGE_PNG_BYTES + 1]),
        ] {
            assert_eq!(invalid.unwrap_err().code, AppErrorCode::InvalidInput);
        }
    }

    #[test]
    fn list_pin_delete_and_clear_return_committed_authoritative_results() {
        let (_dir, _storage, repository) = fixture();
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        repository
            .insert_text(first_id, "alpha", &"1".repeat(64), Some("App.exe"), 1)
            .unwrap();
        repository
            .insert_text(second_id, "beta", &"2".repeat(64), None, 2)
            .unwrap();
        let pinned = repository.set_pinned(first_id, true, 3).unwrap();
        assert!(pinned.pinned);
        assert_eq!(
            repository
                .find_by_hash(ClipboardContentKind::Text, &"1".repeat(64))
                .unwrap()
                .unwrap()
                .id,
            first_id.to_string()
        );
        assert_eq!(repository.get(first_id).unwrap(), pinned);
        let (deleted, asset) = repository.delete(second_id).unwrap();
        assert_eq!(deleted.id, second_id.to_string());
        assert!(asset.is_none());
        let (cleared, assets) = repository.clear(true).unwrap();
        assert_eq!(cleared.removed_count, 0);
        assert!(assets.is_empty());
        assert_eq!(
            repository
                .list(ListClipboardItemsInput {
                    query: "app".into(),
                    content_kind: ClipboardContentKindFilter::Text,
                    limit: 10,
                })
                .unwrap(),
            vec![pinned]
        );
    }

    #[test]
    fn list_validates_unicode_bounds_and_uses_literal_case_insensitive_search_and_id_ties() {
        let (_dir, _storage, repository) = fixture();
        let first_id = Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap();
        let second_id = Uuid::parse_str("00000000-0000-4000-8000-000000000002").unwrap();
        repository
            .insert_text(
                second_id,
                "literal%_needle",
                &"2".repeat(64),
                Some("Second.exe"),
                10,
            )
            .unwrap();
        repository
            .insert_text(first_id, "first", &"1".repeat(64), Some("First.exe"), 10)
            .unwrap();
        let image_id = Uuid::new_v4();
        let asset_id = Uuid::new_v4();
        let image = repository
            .insert_image_metadata(
                image_id,
                NewClipboardAsset {
                    id: asset_id,
                    asset_name: format!("{asset_id}.png"),
                    sha256: "3".repeat(64),
                    width: 1,
                    height: 1,
                    byte_size: 10,
                },
                Some("ImageApp.EXE"),
                11,
            )
            .unwrap()
            .item;

        assert_eq!(
            repository
                .list(ListClipboardItemsInput {
                    query: "%_".into(),
                    content_kind: ClipboardContentKindFilter::Text,
                    limit: 500,
                })
                .unwrap()[0]
                .id,
            second_id.to_string()
        );
        assert_eq!(
            repository
                .list(ListClipboardItemsInput {
                    query: "imageapp".into(),
                    content_kind: ClipboardContentKindFilter::Image,
                    limit: 500,
                })
                .unwrap(),
            vec![image]
        );
        assert_eq!(
            repository
                .list(ListClipboardItemsInput {
                    query: String::new(),
                    content_kind: ClipboardContentKindFilter::Text,
                    limit: 500,
                })
                .unwrap()
                .into_iter()
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            vec![first_id.to_string(), second_id.to_string()]
        );
        assert!(repository
            .list(ListClipboardItemsInput {
                query: "😀".repeat(500),
                content_kind: ClipboardContentKindFilter::All,
                limit: 1,
            })
            .is_ok());
        for invalid in [
            ListClipboardItemsInput {
                query: "😀".repeat(501),
                content_kind: ClipboardContentKindFilter::All,
                limit: 1,
            },
            ListClipboardItemsInput {
                query: String::new(),
                content_kind: ClipboardContentKindFilter::All,
                limit: 0,
            },
            ListClipboardItemsInput {
                query: String::new(),
                content_kind: ClipboardContentKindFilter::All,
                limit: 501,
            },
        ] {
            assert_eq!(
                repository.list(invalid).unwrap_err().code,
                AppErrorCode::InvalidInput
            );
        }
    }

    struct OneItemRetention;

    impl ClipboardRetentionPolicy for OneItemRetention {
        fn unpinned_limit(&self) -> Result<u32, CommandError> {
            Ok(1)
        }
    }

    #[test]
    fn image_asset_metadata_and_cleanup_handoffs_follow_dedupe_retention_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(dir.path()).unwrap());
        let repository = ClipboardRepository::new(Arc::clone(&storage), Arc::new(OneItemRetention));

        let first_item_id = Uuid::new_v4();
        let first_asset_id = Uuid::new_v4();
        let first_asset_name = format!("{first_asset_id}.png");
        repository
            .insert_image_metadata(
                first_item_id,
                NewClipboardAsset {
                    id: first_asset_id,
                    asset_name: first_asset_name.clone(),
                    sha256: "3".repeat(64),
                    width: 4,
                    height: 5,
                    byte_size: 20,
                },
                None,
                1,
            )
            .unwrap();
        let stored = repository.get_asset(first_asset_id).unwrap();
        assert_eq!(stored.clipboard_item_id, first_item_id);
        assert_eq!((stored.width, stored.height, stored.byte_size), (4, 5, 20));

        let second_item_id = Uuid::new_v4();
        let second_asset_id = Uuid::new_v4();
        let second_asset_name = format!("{second_asset_id}.png");
        let retained = repository
            .insert_image_metadata(
                second_item_id,
                NewClipboardAsset {
                    id: second_asset_id,
                    asset_name: second_asset_name.clone(),
                    sha256: "4".repeat(64),
                    width: 6,
                    height: 7,
                    byte_size: 42,
                },
                None,
                2,
            )
            .unwrap();
        assert_eq!(retained.removed_asset_names, vec![first_asset_name]);
        assert_eq!(
            repository.get_asset(first_asset_id).unwrap_err().code,
            AppErrorCode::NotFound
        );

        let duplicate_asset_id = Uuid::new_v4();
        let duplicate_asset_name = format!("{duplicate_asset_id}.png");
        let duplicate = repository
            .insert_image_metadata(
                Uuid::new_v4(),
                NewClipboardAsset {
                    id: duplicate_asset_id,
                    asset_name: duplicate_asset_name.clone(),
                    sha256: "4".repeat(64),
                    width: 6,
                    height: 7,
                    byte_size: 42,
                },
                None,
                3,
            )
            .unwrap();
        assert!(!duplicate.inserted);
        assert_eq!(duplicate.item.id, second_item_id.to_string());
        assert_eq!(duplicate.removed_asset_names, vec![duplicate_asset_name]);

        let (_, removed_asset_name) = repository.delete(second_item_id).unwrap();
        assert_eq!(
            removed_asset_name.as_deref(),
            Some(second_asset_name.as_str())
        );
    }
}
