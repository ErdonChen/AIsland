use crate::contracts::{
    CommandError, SafeMessageParameters, ServiceHealthSnapshot, ServiceHealthState,
};
use crate::repositories::validate_message_parameters;
use crate::storage::Storage;
use std::sync::Arc;

#[derive(Clone)]
pub struct ServiceHealthRepository {
    storage: Arc<Storage>,
}

impl ServiceHealthRepository {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    pub fn upsert(&self, snapshot: &ServiceHealthSnapshot) -> Result<(), CommandError> {
        validate_message_parameters(&snapshot.message_key, &snapshot.parameters)?;
        let parameters_json =
            serde_json::to_string(&snapshot.parameters).map_err(|_| database_failure())?;
        let state = health_state_name(&snapshot.state);

        self.storage.with_transaction(|transaction| {
            transaction
                .execute(
                    r#"INSERT INTO service_health(service_id, state, message_key, parameters_json, checked_at)
                       VALUES (?1, ?2, ?3, ?4, ?5)
                       ON CONFLICT(service_id) DO UPDATE SET
                         state = excluded.state,
                         message_key = excluded.message_key,
                         parameters_json = excluded.parameters_json,
                         checked_at = excluded.checked_at"#,
                    rusqlite::params![
                        snapshot.service_id,
                        state,
                        snapshot.message_key,
                        parameters_json,
                        snapshot.checked_at
                    ],
                )
                .map_err(CommandError::from)?;
            Ok(())
        })
    }

    pub fn list(&self) -> Result<Vec<ServiceHealthSnapshot>, CommandError> {
        let rows: Vec<(String, String, String, String, i64)> =
            self.storage.with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        r#"SELECT service_id, state, message_key, parameters_json, checked_at
                       FROM service_health ORDER BY service_id ASC"#,
                    )
                    .map_err(CommandError::from)?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    })
                    .map_err(CommandError::from)?;
                rows.map(|row| row.map_err(CommandError::from)).collect()
            })?;

        rows.into_iter()
            .map(
                |(service_id, state, message_key, parameters_json, checked_at)| {
                    Ok(ServiceHealthSnapshot {
                        service_id,
                        state: parse_health_state(&state)?,
                        message_key,
                        parameters: serde_json::from_str::<SafeMessageParameters>(&parameters_json)
                            .map_err(|_| database_failure())?,
                        checked_at,
                    })
                },
            )
            .collect()
    }
}

fn health_state_name(state: &ServiceHealthState) -> &'static str {
    match state {
        ServiceHealthState::Healthy => "healthy",
        ServiceHealthState::Degraded => "degraded",
        ServiceHealthState::Blocked => "blocked",
        ServiceHealthState::Offline => "offline",
    }
}

fn parse_health_state(value: &str) -> Result<ServiceHealthState, CommandError> {
    match value {
        "healthy" => Ok(ServiceHealthState::Healthy),
        "degraded" => Ok(ServiceHealthState::Degraded),
        "blocked" => Ok(ServiceHealthState::Blocked),
        "offline" => Ok(ServiceHealthState::Offline),
        _ => Err(database_failure()),
    }
}

fn database_failure() -> CommandError {
    CommandError {
        code: crate::contracts::AppErrorCode::DatabaseFailure,
        message_key: "errors.databaseFailure".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{
        AppErrorCode, SafeMessageParameters, SafeParameterValue, ServiceHealthState,
    };
    use crate::storage::Storage;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn repository() -> ServiceHealthRepository {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.keep();
        ServiceHealthRepository::new(Arc::new(Storage::open(&path).unwrap()))
    }

    fn snapshot(
        service_id: &str,
        message_key: &str,
        parameters: SafeMessageParameters,
    ) -> ServiceHealthSnapshot {
        ServiceHealthSnapshot {
            service_id: service_id.into(),
            state: ServiceHealthState::Blocked,
            message_key: message_key.into(),
            parameters,
            checked_at: 10,
        }
    }

    fn raw_row(
        repository: &ServiceHealthRepository,
        service_id: &str,
    ) -> Option<(String, String, String, i64)> {
        use rusqlite::OptionalExtension;
        repository.storage.with_connection(|connection| {
            connection.query_row("SELECT state, message_key, parameters_json, checked_at FROM service_health WHERE service_id = ?1", [service_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).optional().map_err(Into::into)
        }).unwrap()
    }

    fn total_changes(repository: &ServiceHealthRepository) -> u64 {
        repository
            .storage
            .with_connection(|connection| Ok(connection.total_changes()))
            .unwrap()
    }

    #[test]
    fn upsert_replaces_a_service_and_lists_by_service_id() {
        let repository = repository();
        repository
            .upsert(&snapshot(
                "zeta",
                "services.clipboard.locked",
                BTreeMap::from([("count".into(), SafeParameterValue::Number(3.into()))]),
            ))
            .unwrap();
        repository
            .upsert(&snapshot(
                "alpha",
                "services.clipboard.locked",
                BTreeMap::from([("count".into(), SafeParameterValue::Number(3.into()))]),
            ))
            .unwrap();
        let mut replacement = snapshot(
            "zeta",
            "services.degraded",
            BTreeMap::from([
                (
                    "serviceId".into(),
                    SafeParameterValue::String("zeta".into()),
                ),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String("locked".into()),
                ),
            ]),
        );
        replacement.state = ServiceHealthState::Degraded;
        replacement.checked_at = 11;
        repository.upsert(&replacement).unwrap();
        assert_eq!(
            repository.list().unwrap(),
            vec![
                snapshot(
                    "alpha",
                    "services.clipboard.locked",
                    BTreeMap::from([("count".into(), SafeParameterValue::Number(3.into()))])
                ),
                replacement
            ]
        );
    }

    #[test]
    fn rejects_message_contract_violations_before_changing_existing_health() {
        let repository = repository();
        let prior = snapshot(
            "clipboard",
            "services.clipboard.locked",
            BTreeMap::from([("count".into(), SafeParameterValue::Number(3.into()))]),
        );
        repository.upsert(&prior).unwrap();
        let invalid = [
            snapshot(
                "clipboard",
                "services.unknown",
                SafeMessageParameters::new(),
            ),
            snapshot(
                "clipboard",
                "errors.invalidInput",
                BTreeMap::from([(
                    "reasonCode".into(),
                    SafeParameterValue::String("bad".into()),
                )]),
            ),
            snapshot(
                "clipboard",
                "services.clipboard.locked",
                BTreeMap::from([(
                    "entityId".into(),
                    SafeParameterValue::String("entity-1".into()),
                )]),
            ),
            snapshot(
                "clipboard",
                "services.clipboard.locked",
                BTreeMap::from([("body".into(), SafeParameterValue::String("secret".into()))]),
            ),
            snapshot(
                "clipboard",
                "services.clipboard.locked",
                BTreeMap::from([("token".into(), SafeParameterValue::String("secret".into()))]),
            ),
            snapshot(
                "clipboard",
                "services.clipboard.locked",
                BTreeMap::from([(
                    "rawXml".into(),
                    SafeParameterValue::String("<secret/>".into()),
                )]),
            ),
            snapshot(
                "clipboard",
                "services.degraded",
                BTreeMap::from([
                    (
                        "serviceId".into(),
                        SafeParameterValue::String("C:\\Build\\release".into()),
                    ),
                    (
                        "reasonCode".into(),
                        SafeParameterValue::String("locked".into()),
                    ),
                ]),
            ),
            snapshot(
                "clipboard",
                "services.degraded",
                BTreeMap::from([
                    (
                        "serviceId".into(),
                        SafeParameterValue::String("\\\\server\\share\\release".into()),
                    ),
                    (
                        "reasonCode".into(),
                        SafeParameterValue::String("locked".into()),
                    ),
                ]),
            ),
            snapshot(
                "clipboard",
                "services.degraded",
                BTreeMap::from([
                    (
                        "serviceId".into(),
                        SafeParameterValue::String("/opt/build/release".into()),
                    ),
                    (
                        "reasonCode".into(),
                        SafeParameterValue::String("locked".into()),
                    ),
                ]),
            ),
            snapshot(
                "clipboard",
                "services.degraded",
                BTreeMap::from([
                    (
                        "serviceId".into(),
                        SafeParameterValue::String("clipboard".into()),
                    ),
                    (
                        "reasonCode".into(),
                        SafeParameterValue::String("C:\\Build\\release".into()),
                    ),
                ]),
            ),
            snapshot(
                "clipboard",
                "services.degraded",
                BTreeMap::from([
                    (
                        "serviceId".into(),
                        SafeParameterValue::String("clipboard".into()),
                    ),
                    (
                        "reasonCode".into(),
                        SafeParameterValue::String("\\\\server\\share\\release".into()),
                    ),
                ]),
            ),
            snapshot(
                "clipboard",
                "services.degraded",
                BTreeMap::from([
                    (
                        "serviceId".into(),
                        SafeParameterValue::String("clipboard".into()),
                    ),
                    (
                        "reasonCode".into(),
                        SafeParameterValue::String("/opt/build/release".into()),
                    ),
                ]),
            ),
        ];
        let raw_prior = raw_row(&repository, "clipboard");
        let changes_before = total_changes(&repository);
        let mut outcomes = Vec::new();
        for candidate in invalid {
            let code = repository.upsert(&candidate).err().map(|error| error.code);
            outcomes.push((
                code,
                raw_row(&repository, "clipboard") == raw_prior,
                total_changes(&repository) == changes_before,
            ));
        }
        assert_eq!(
            outcomes,
            vec![(Some(AppErrorCode::InvalidInput), true, true); 12]
        );
        assert_eq!(repository.list().unwrap(), vec![prior]);
    }
}
