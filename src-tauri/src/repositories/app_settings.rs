use crate::contracts::{AppErrorCode, CommandError, SafeMessageParameters, SafeParameterValue};
use crate::storage::Storage;
use rusqlite::OptionalExtension;
use serde::{de::DeserializeOwned, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppSettingsRepository {
    storage: Arc<Storage>,
}

impl AppSettingsRepository {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<(T, u64)>, CommandError> {
        Ok(self
            .get_with_metadata(key)?
            .map(|(value, revision, _updated_at)| (value, revision)))
    }

    pub fn get_with_metadata<T: DeserializeOwned>(
        &self,
        key: &str,
    ) -> Result<Option<(T, u64, i64)>, CommandError> {
        let stored = self.storage.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT value_json, revision, updated_at FROM app_settings WHERE key = ?1",
                    [key],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(Into::into)
        })?;

        stored
            .map(|(value_json, revision, updated_at)| {
                let value = serde_json::from_str(&value_json)
                    .map_err(|_| invalid_input("settingsValueTypeMismatch"))?;
                let revision = u64::try_from(revision).map_err(|_| database_failure())?;
                Ok((value, revision, updated_at))
            })
            .transpose()
    }

    pub fn put<T: Serialize>(
        &self,
        key: &str,
        value: &T,
        expected_revision: Option<u64>,
        now: i64,
    ) -> Result<u64, CommandError> {
        if now < 0 {
            return Err(invalid_input("invalidTimestamp"));
        }
        let value_json = serde_json::to_string(value)
            .map_err(|_| invalid_input("settingsSerializationFailed"))?;

        self.storage.with_transaction(|transaction| {
            let changed_rows = match expected_revision {
                None => transaction.execute(
                    "INSERT OR IGNORE INTO app_settings(key, value_json, revision, updated_at) VALUES (?1, ?2, 1, ?3)",
                    rusqlite::params![key, value_json, now],
                ),
                Some(expected) => {
                    let expected = i64::try_from(expected)
                        .map_err(|_| invalid_input("invalidRevision"))?;
                    transaction.execute(
                        r#"INSERT INTO app_settings(key, value_json, revision, updated_at)
                           SELECT ?1, ?2, 1, ?3
                           WHERE EXISTS (SELECT 1 FROM app_settings WHERE key = ?1)
                           ON CONFLICT(key) DO UPDATE SET
                             value_json = excluded.value_json,
                             revision = app_settings.revision + 1,
                             updated_at = excluded.updated_at
                           WHERE app_settings.revision = ?4"#,
                        rusqlite::params![key, value_json, now, expected],
                    )
                }
            }
            .map_err(CommandError::from)?;

            if changed_rows == 0 {
                let revision = transaction
                    .query_row(
                        "SELECT revision FROM app_settings WHERE key = ?1",
                        [key],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()
                    .map_err(CommandError::from)?;
                return Err(match revision {
                    Some(_) => conflict(),
                    None => not_found(),
                });
            }

            let revision = transaction
                .query_row(
                    "SELECT revision FROM app_settings WHERE key = ?1",
                    [key],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(CommandError::from)?;
            u64::try_from(revision).map_err(|_| database_failure())
        })
    }
}

fn invalid_input(reason_code: &str) -> CommandError {
    CommandError {
        code: AppErrorCode::InvalidInput,
        message_key: "errors.invalidInput".into(),
        details: BTreeMap::from([(
            "reasonCode".into(),
            SafeParameterValue::String(reason_code.into()),
        )]),
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

fn conflict() -> CommandError {
    CommandError {
        code: AppErrorCode::Conflict,
        message_key: "errors.conflict".into(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::AppErrorCode;
    use crate::storage::Storage;
    use std::sync::Arc;

    fn repository() -> AppSettingsRepository {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.keep();
        AppSettingsRepository::new(Arc::new(Storage::open(&path).unwrap()))
    }

    #[test]
    fn put_creates_then_compares_and_swaps_without_overwriting_on_conflict() {
        let repository = repository();
        assert_eq!(repository.put("ui.locale", &"zh-CN", None, 10).unwrap(), 1);
        assert_eq!(
            repository.put("ui.locale", &"en-US", Some(1), 11).unwrap(),
            2
        );
        let row_before_conflict = repository
            .storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT value_json, revision, updated_at FROM app_settings WHERE key = 'ui.locale'",
                        [],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        let error = repository
            .put("ui.locale", &"zh-CN", Some(1), 12)
            .unwrap_err();
        assert_eq!(error.code, AppErrorCode::Conflict);
        let row_after_conflict = repository
            .storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT value_json, revision, updated_at FROM app_settings WHERE key = 'ui.locale'",
                        [],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(row_after_conflict, row_before_conflict);
        assert_eq!(
            repository.get::<String>("ui.locale").unwrap(),
            Some(("en-US".into(), 2))
        );
    }

    #[test]
    fn compare_and_swap_distinguishes_a_missing_key_without_creating_it() {
        let repository = repository();
        let result = repository.put("ui.locale", &"zh-CN", Some(1), 10);
        assert!(matches!(result, Err(error) if error.code == AppErrorCode::NotFound));
        assert_eq!(repository.get::<String>("ui.locale").unwrap(), None);
    }
}
