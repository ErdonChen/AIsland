use crate::contracts::{
    AppErrorCode, CommandError, DiagnosticEvent, DiagnosticLevel, SafeMessageParameters,
    SafeParameterValue,
};
use crate::repositories::allowed_diagnostic_parameters;
use crate::storage::Storage;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct DiagnosticsRepository {
    storage: Arc<Storage>,
}

impl DiagnosticsRepository {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    pub fn record(&self, event: &DiagnosticEvent) -> Result<(), CommandError> {
        validate_event(event)?;
        let parameters_json =
            serde_json::to_string(&event.parameters).map_err(|_| database_failure())?;
        let level = diagnostic_level_name(&event.level);

        self.storage.with_transaction(|transaction| {
            transaction
                .execute(
                    r#"INSERT INTO diagnostic_events(id, service_id, level, code, parameters_json, created_at)
                       VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
                    rusqlite::params![
                        event.id,
                        event.service_id,
                        level,
                        event.code,
                        parameters_json,
                        event.created_at
                    ],
                )
                .map_err(CommandError::from)?;
            transaction
                .execute(
                    r#"DELETE FROM diagnostic_events
                       WHERE id IN (
                         SELECT id FROM diagnostic_events
                         ORDER BY created_at DESC, id DESC
                         LIMIT -1 OFFSET 2000
                       )"#,
                    [],
                )
                .map_err(CommandError::from)?;
            Ok(())
        })
    }

    pub fn list(&self, limit: u32) -> Result<Vec<DiagnosticEvent>, CommandError> {
        if !(1..=500).contains(&limit) {
            return Err(invalid_input("invalidDiagnosticLimit"));
        }
        let rows: Vec<(String, String, String, String, String, i64)> =
            self.storage.with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        r#"SELECT id, service_id, level, code, parameters_json, created_at
                       FROM diagnostic_events
                       ORDER BY created_at DESC, id DESC LIMIT ?1"#,
                    )
                    .map_err(CommandError::from)?;
                let rows = statement
                    .query_map([limit], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    })
                    .map_err(CommandError::from)?;
                rows.map(|row| row.map_err(CommandError::from)).collect()
            })?;

        rows.into_iter()
            .map(
                |(id, service_id, level, code, parameters_json, created_at)| {
                    Ok(DiagnosticEvent {
                        id,
                        service_id,
                        level: parse_diagnostic_level(&level)?,
                        code,
                        parameters: serde_json::from_str::<SafeMessageParameters>(&parameters_json)
                            .map_err(|_| database_failure())?,
                        created_at,
                    })
                },
            )
            .collect()
    }
}

fn validate_event(event: &DiagnosticEvent) -> Result<(), CommandError> {
    let allowed = allowed_diagnostic_parameters(&event.code)
        .ok_or_else(|| invalid_input("diagnosticContractViolation"))?;
    if event.parameters.len() != allowed.len()
        || !allowed
            .iter()
            .all(|name| event.parameters.contains_key(name))
        || !valid_identifier_grammar(&event.id)
        || !valid_identifier_grammar(&event.service_id)
    {
        return Err(invalid_input("diagnosticContractViolation"));
    }

    for (name, value) in &event.parameters {
        let valid = match (name.as_str(), value) {
            (
                "count" | "receivedAt" | "byteCount" | "sampledAt" | "skippedCount" | "rowCount"
                | "cursor" | "checkedAt",
                SafeParameterValue::Number(value),
            ) => value.as_i64().is_some_and(|value| value >= 0),
            (
                "serviceId" | "reasonCode" | "entityId" | "todoId" | "reminderId" | "metric"
                | "watchId" | "thresholdId" | "source",
                SafeParameterValue::String(value),
            ) => {
                let valid_grammar = valid_identifier_grammar(value);
                let sensitive = sensitive_diagnostic_value(value);
                valid_grammar && !sensitive
            }
            ("agentName", SafeParameterValue::String(value)) => {
                matches!(value.as_str(), "Codex" | "Hermes" | "WorkBuddy" | "claude")
            }
            ("environment", SafeParameterValue::String(value)) => {
                matches!(value.as_str(), "windows" | "wsl")
            }
            ("kind", SafeParameterValue::String(value)) => {
                matches!(value.as_str(), "text" | "image")
            }
            ("fileNameHash", SafeParameterValue::String(value)) => {
                value.len() == 16 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
            }
            _ => false,
        };
        if !valid {
            return Err(invalid_input("diagnosticContractViolation"));
        }
    }
    Ok(())
}

fn valid_identifier_grammar(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn sensitive_diagnostic_value(value: &str) -> bool {
    is_absolute_path(value) || contains_sensitive_token_sequence(value)
}

fn is_absolute_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    value.starts_with('/')
        || value.starts_with("\\\\")
        || matches!(bytes, [drive, b':', separator, ..] if drive.is_ascii_alphabetic() && matches!(separator, b'\\' | b'/'))
}

fn contains_sensitive_token_sequence(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let tokens = lower
        .split(['.', '_', '-'])
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    tokens.windows(2).any(|pair| {
        matches!(
            pair,
            ["clipboard", "text"]
                | ["note", "markdown"]
                | ["notification", "body"]
                | ["reminder", "body"]
                | ["prompt", "input"]
                | ["tool", "output"]
                | ["raw", "xml"]
                | ["auth", "token"]
        )
    }) || tokens
        .windows(3)
        .any(|sequence| matches!(sequence, ["local", "audio", "path"]))
}

fn diagnostic_level_name(level: &DiagnosticLevel) -> &'static str {
    match level {
        DiagnosticLevel::Info => "info",
        DiagnosticLevel::Warning => "warning",
        DiagnosticLevel::Failure => "failure",
    }
}

fn parse_diagnostic_level(value: &str) -> Result<DiagnosticLevel, CommandError> {
    match value {
        "info" => Ok(DiagnosticLevel::Info),
        "warning" => Ok(DiagnosticLevel::Warning),
        "failure" => Ok(DiagnosticLevel::Failure),
        _ => Err(database_failure()),
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
    use crate::contracts::{AppErrorCode, DiagnosticLevel, SafeParameterValue};
    use crate::storage::Storage;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn repository() -> DiagnosticsRepository {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.keep();
        DiagnosticsRepository::new(Arc::new(Storage::open(&path).unwrap()))
    }

    fn safe_event(id: &str, created_at: i64) -> DiagnosticEvent {
        DiagnosticEvent {
            id: id.into(),
            service_id: "health".into(),
            level: DiagnosticLevel::Failure,
            code: "events.serviceHealthEmitFailed".into(),
            parameters: BTreeMap::from([
                (
                    "serviceId".into(),
                    SafeParameterValue::String("health".into()),
                ),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String("emitFailed".into()),
                ),
                ("count".into(), SafeParameterValue::Number(3.into())),
            ]),
            created_at,
        }
    }

    fn row_count(repository: &DiagnosticsRepository) -> i64 {
        repository
            .storage
            .with_connection(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM diagnostic_events", [], |row| {
                        row.get(0)
                    })
                    .map_err(Into::into)
            })
            .unwrap()
    }

    fn total_changes(repository: &DiagnosticsRepository) -> u64 {
        repository
            .storage
            .with_connection(|connection| Ok(connection.total_changes()))
            .unwrap()
    }

    #[test]
    fn diagnostic_code_allowlist_is_closed_and_exact() {
        assert_eq!(
            crate::repositories::allowed_diagnostic_parameters("storage.integrityFailed")
                .map(<[_]>::to_vec),
            Some(vec!["serviceId".into(), "reasonCode".into()])
        );
        assert_eq!(
            crate::repositories::allowed_diagnostic_parameters("events.serviceHealthEmitFailed")
                .map(<[_]>::to_vec),
            Some(vec![
                "serviceId".into(),
                "reasonCode".into(),
                "count".into()
            ])
        );
        assert_eq!(
            crate::repositories::allowed_diagnostic_parameters("integration.rollbackFailed")
                .map(<[_]>::to_vec),
            Some(vec![
                "agentName".into(),
                "environment".into(),
                "reasonCode".into()
            ])
        );
        assert_eq!(
            crate::repositories::allowed_diagnostic_parameters("events.todoChangedEmitFailed")
                .map(<[_]>::to_vec),
            Some(vec!["entityId".into()])
        );
        assert_eq!(
            crate::repositories::allowed_diagnostic_parameters("todo.reminderProjectionFailed")
                .map(<[_]>::to_vec),
            Some(vec!["todoId".into(), "reminderId".into()])
        );
        assert_eq!(
            crate::repositories::allowed_diagnostic_parameters("events.unknown"),
            None
        );
    }

    #[test]
    fn todo_emit_failure_accepts_only_one_safe_entity_id() {
        let repository = repository();
        let event = DiagnosticEvent {
            id: "todo-emit-1".into(),
            service_id: "todo".into(),
            level: DiagnosticLevel::Failure,
            code: "events.todoChangedEmitFailed".into(),
            parameters: BTreeMap::from([(
                "entityId".into(),
                SafeParameterValue::String("5cfe2e77-71ec-46fe-bb72-0f05ace3a218".into()),
            )]),
            created_at: 42,
        };
        repository.record(&event).unwrap();
        assert_eq!(repository.list(1).unwrap(), vec![event]);
    }

    #[test]
    fn todo_reminder_projection_failure_accepts_only_safe_todo_and_reminder_ids() {
        let repository = repository();
        let event = DiagnosticEvent {
            id: "todo-reminder-projection-1".into(),
            service_id: "todo".into(),
            level: DiagnosticLevel::Failure,
            code: "todo.reminderProjectionFailed".into(),
            parameters: BTreeMap::from([
                (
                    "todoId".into(),
                    SafeParameterValue::String("5cfe2e77-71ec-46fe-bb72-0f05ace3a218".into()),
                ),
                (
                    "reminderId".into(),
                    SafeParameterValue::String("6d8d216b-87ad-43a8-a95a-68b3c00691bb".into()),
                ),
            ]),
            created_at: 42,
        };
        repository.record(&event).unwrap();
        assert_eq!(repository.list(1).unwrap(), vec![event]);
    }

    #[test]
    fn rollback_diagnostic_accepts_only_fixed_agent_and_environment_values() {
        let repository = repository();
        let event = DiagnosticEvent {
            id: "rollback-1".into(),
            service_id: "agent-integrations".into(),
            level: DiagnosticLevel::Failure,
            code: "integration.rollbackFailed".into(),
            parameters: BTreeMap::from([
                (
                    "agentName".into(),
                    SafeParameterValue::String("Codex".into()),
                ),
                (
                    "environment".into(),
                    SafeParameterValue::String("windows".into()),
                ),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String("rollbackFailed".into()),
                ),
            ]),
            created_at: 1,
        };
        repository.record(&event).unwrap();
        for (index, (name, value)) in [
            ("agentName", "C:\\Users\\Alice"),
            ("agentName", "UnknownAgent"),
            ("environment", "/home/alice"),
            ("environment", "linux"),
        ]
        .into_iter()
        .enumerate()
        {
            let mut invalid = event.clone();
            invalid.id = format!("invalid-{index}");
            invalid
                .parameters
                .insert(name.into(), SafeParameterValue::String(value.into()));
            assert_eq!(
                repository.record(&invalid).unwrap_err().code,
                AppErrorCode::InvalidInput
            );
        }
        assert_eq!(repository.list(10).unwrap(), vec![event]);
    }

    #[test]
    fn list_orders_newest_created_at_then_id_and_rejects_out_of_range_limits() {
        let repository = repository();
        repository.record(&safe_event("a", 20)).unwrap();
        repository.record(&safe_event("z", 20)).unwrap();
        repository.record(&safe_event("middle", 10)).unwrap();
        assert_eq!(
            repository.list(2).unwrap(),
            vec![safe_event("z", 20), safe_event("a", 20)]
        );
        assert_eq!(
            repository.list(0).unwrap_err().code,
            AppErrorCode::InvalidInput
        );
        assert_eq!(
            repository.list(501).unwrap_err().code,
            AppErrorCode::InvalidInput
        );
    }

    #[test]
    fn record_accepts_safe_words_that_only_contain_sensitive_substrings() {
        let repository = repository();
        let fixtures = [
            ("legal-0", "serviceId", "audio-engine", 10),
            ("legal-1", "reasonCode", "audio-engine", 11),
            ("legal-2", "serviceId", "bodyguard", 12),
            ("legal-3", "reasonCode", "bodyguard", 13),
            ("legal-4", "serviceId", "tooling", 14),
            ("legal-5", "reasonCode", "tooling", 15),
            ("legal-6", "serviceId", "clipboard", 16),
            ("legal-7", "reasonCode", "clipboard", 17),
        ];
        let mut outcomes = Vec::new();
        for (id, name, value, created_at) in fixtures {
            let mut event = safe_event(id, created_at);
            event
                .parameters
                .insert(name.into(), SafeParameterValue::String(value.into()));
            outcomes.push(repository.record(&event).err().map(|error| error.code));
        }
        let stored_ids = repository
            .list(8)
            .unwrap()
            .into_iter()
            .map(|event| event.id)
            .collect::<Vec<_>>();
        assert_eq!(
            (outcomes, stored_ids),
            (
                vec![None, None, None, None, None, None, None, None],
                vec![
                    "legal-7".to_string(),
                    "legal-6".to_string(),
                    "legal-5".to_string(),
                    "legal-4".to_string(),
                    "legal-3".to_string(),
                    "legal-2".to_string(),
                    "legal-1".to_string(),
                    "legal-0".to_string(),
                ],
            )
        );
    }

    #[test]
    fn record_rejects_unregistered_and_sensitive_diagnostics_without_writing() {
        let repository = repository();
        repository.record(&safe_event("prior", 1)).unwrap();
        let mut invalid = Vec::new();
        let mut unknown = safe_event("unknown", 2);
        unknown.code = "events.unknown".into();
        invalid.push(unknown);
        let mut missing = safe_event("missing", 2);
        missing.parameters.remove("count");
        invalid.push(missing);
        let mut extra = safe_event("extra", 2);
        extra
            .parameters
            .insert("body".into(), SafeParameterValue::String("secret".into()));
        invalid.push(extra);
        let mut wrong_type = safe_event("wrong-type", 2);
        wrong_type
            .parameters
            .insert("count".into(), SafeParameterValue::String("3".into()));
        invalid.push(wrong_type);
        for (index, value) in [
            "clipboard-text",
            "note-markdown",
            "notification-body",
            "reminder-body",
            "prompt-input",
            "tool-output",
            "raw-xml",
            "auth-token",
            "local-audio-path",
            "C:\\Build\\release",
            "\\\\server\\share\\release",
            "/opt/build/release",
        ]
        .into_iter()
        .enumerate()
        {
            let mut event = safe_event(&format!("sensitive-{index}"), 2);
            event.parameters.insert(
                "reasonCode".into(),
                SafeParameterValue::String(value.into()),
            );
            invalid.push(event);
        }
        let changes_before = total_changes(&repository);
        let mut outcomes = Vec::new();
        for event in invalid {
            let code = repository.record(&event).err().map(|error| error.code);
            outcomes.push((
                code,
                row_count(&repository) == 1,
                total_changes(&repository) == changes_before,
            ));
        }
        assert_eq!(
            outcomes,
            vec![(Some(AppErrorCode::InvalidInput), true, true); 16]
        );
        assert_eq!(repository.list(500).unwrap(), vec![safe_event("prior", 1)]);
    }

    #[test]
    fn record_retains_only_the_newest_two_thousand_events() {
        let repository = repository();
        for number in 0..2001 {
            repository
                .record(&safe_event(&format!("id-{number:04}"), number))
                .unwrap();
        }
        assert_eq!(row_count(&repository), 2000);
        let all = repository.list(500).unwrap();
        assert_eq!(all.len(), 500);
        assert_eq!(all.first(), Some(&safe_event("id-2000", 2000)));
        assert_eq!(all.last(), Some(&safe_event("id-1501", 1501)));
    }
}
