use crate::contracts::{
    AgentEnvironment, AgentId, AgentIntegrationRecord, AgentObservation, AgentStatus, AppErrorCode,
    CommandError, IntegrationState, SafeMessageParameters,
};
use crate::domain::agents::{
    agent_reply_preview_from_message, compare_task_event, projection_summary,
    AgentIntegrationEntity, AgentTaskRecord, EventOrder, ValidatedAgentEvent,
    AGENT_REPLY_MESSAGE_PREFIX,
};
use crate::storage::Storage;
use rusqlite::OptionalExtension;
use std::sync::Arc;

#[derive(Clone)]
pub struct AgentRepository {
    storage: Arc<Storage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectionOutcome {
    Duplicate,
    IgnoredOutOfOrder,
    Advanced { event: ValidatedAgentEvent },
}

impl AgentRepository {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    pub fn insert_event_and_project(
        &self,
        event: &ValidatedAgentEvent,
        received_at: i64,
    ) -> Result<ProjectionOutcome, CommandError> {
        if event.occurred_at < 0
            || received_at < 0
            || event.task_id.is_empty()
            || event.event_id.is_empty()
        {
            return Err(invalid_input());
        }
        let sequence = event
            .sequence
            .map(i64::try_from)
            .transpose()
            .map_err(|_| invalid_input())?;
        self.storage.with_transaction(|transaction| {
            let changed = transaction.execute(
                r#"INSERT OR IGNORE INTO agent_events(
                    event_id, agent_id, environment, task_id, status, sequence, task_title, project,
                    message, path, occurred_at, received_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"#,
                rusqlite::params![
                    event.event_id, agent_id(&event.agent_id), environment(&event.environment), event.task_id,
                    status(&event.status), sequence, event.task_title, event.project, event.message,
                    event.path, event.occurred_at, received_at
                ],
            )?;
            if changed == 0 {
                return Ok(ProjectionOutcome::Duplicate);
            }
            let current: Option<(Option<i64>, String, i64)> = transaction
                .query_row(
                    "SELECT latest_sequence, source_event_id, occurred_at FROM agent_tasks WHERE agent_id = ?1 AND environment = ?2 AND task_id = ?3",
                    rusqlite::params![agent_id(&event.agent_id), environment(&event.environment), event.task_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            if let Some((latest_sequence, source_event_id, occurred_at)) = current {
                let current = AgentTaskRecord {
                    latest_sequence: latest_sequence
                        .map(u64::try_from)
                        .transpose()
                        .map_err(|_| database_failure())?,
                    source_event_id,
                    occurred_at,
                };
                match compare_task_event(&current, event) {
                    EventOrder::Duplicate => return Ok(ProjectionOutcome::Duplicate),
                    EventOrder::OutOfOrder => return Ok(ProjectionOutcome::IgnoredOutOfOrder),
                    EventOrder::Advances => {}
                }
            }
            let summary = projection_summary(event);
            transaction.execute(
                r#"INSERT INTO agent_tasks(agent_id, environment, task_id, status, summary, latest_sequence,
                    source_event_id, occurred_at, received_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                   ON CONFLICT(agent_id, environment, task_id) DO UPDATE SET
                     status = excluded.status, summary = excluded.summary, latest_sequence = excluded.latest_sequence,
                     source_event_id = excluded.source_event_id, occurred_at = excluded.occurred_at,
                     received_at = excluded.received_at"#,
                rusqlite::params![
                    agent_id(&event.agent_id), environment(&event.environment), event.task_id, status(&event.status),
                    summary, sequence, event.event_id, event.occurred_at, received_at
                ],
            )?;
            Ok(ProjectionOutcome::Advanced { event: event.clone() })
        })
    }

    pub fn list_tasks(&self) -> Result<Vec<AgentObservation>, CommandError> {
        self.storage.with_connection(|connection| {
            let mut statement = connection.prepare(
                r#"SELECT task.agent_id, task.environment, task.task_id, task.status, task.summary,
                          (SELECT event.message
                             FROM agent_events event
                            WHERE event.agent_id = task.agent_id
                              AND event.environment = task.environment
                              AND event.message IS NOT NULL
                              AND substr(event.message, 1, length(?1)) = ?1
                              AND length(trim(substr(event.message, length(?1) + 1))) > 0
                            ORDER BY event.occurred_at DESC, event.event_id DESC
                            LIMIT 1) AS latest_reply_preview,
                          task.source_event_id, task.occurred_at, task.received_at
                     FROM agent_tasks task
                    ORDER BY task.agent_id, task.environment, task.task_id"#,
            )?;
            let rows = statement.query_map([AGENT_REPLY_MESSAGE_PREFIX], |row| {
                Ok(AgentObservation {
                    agent_id: parse_agent_id(&row.get::<_, String>(0)?)
                        .ok_or(rusqlite::Error::InvalidQuery)?,
                    environment: parse_environment(&row.get::<_, String>(1)?)
                        .ok_or(rusqlite::Error::InvalidQuery)?,
                    task_id: row.get(2)?,
                    status: parse_status(&row.get::<_, String>(3)?)
                        .ok_or(rusqlite::Error::InvalidQuery)?,
                    summary: row.get(4)?,
                    latest_reply_preview: bounded_reply_preview(row.get(5)?),
                    source_event_id: row.get(6)?,
                    occurred_at: row.get(7)?,
                    received_at: row.get(8)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(CommandError::from)
        })
    }

    pub fn get_event_by_id(
        &self,
        requested_event_id: &str,
    ) -> Result<Option<ValidatedAgentEvent>, CommandError> {
        if requested_event_id.is_empty() {
            return Err(invalid_input());
        }
        let row = self.storage.with_connection(|connection| {
            connection
                .query_row(
                    r#"SELECT event_id, agent_id, environment, task_id, status, sequence,
                              task_title, project, message, path, occurred_at
                       FROM agent_events WHERE event_id = ?1"#,
                    [requested_event_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, Option<i64>>(5)?,
                            row.get::<_, Option<String>>(6)?,
                            row.get::<_, Option<String>>(7)?,
                            row.get::<_, Option<String>>(8)?,
                            row.get::<_, Option<String>>(9)?,
                            row.get::<_, i64>(10)?,
                        ))
                    },
                )
                .optional()
                .map_err(CommandError::from)
        })?;
        row.map(
            |(
                event_id,
                stored_agent_id,
                stored_environment,
                task_id,
                stored_status,
                sequence,
                task_title,
                project,
                message,
                path,
                occurred_at,
            )| {
                Ok(ValidatedAgentEvent {
                    event_id,
                    agent_id: parse_agent_id(&stored_agent_id).ok_or_else(database_failure)?,
                    environment: parse_environment(&stored_environment)
                        .ok_or_else(database_failure)?,
                    task_id,
                    status: parse_status(&stored_status).ok_or_else(database_failure)?,
                    sequence: sequence
                        .map(u64::try_from)
                        .transpose()
                        .map_err(|_| database_failure())?,
                    task_title,
                    project,
                    message,
                    path,
                    occurred_at,
                })
            },
        )
        .transpose()
    }

    pub fn get_integration(
        &self,
        requested_agent_id: AgentId,
        requested_environment: AgentEnvironment,
    ) -> Result<Option<AgentIntegrationEntity>, CommandError> {
        self.storage.with_connection(|connection| {
            connection.query_row(
                "SELECT install_state, config_path, backup_path, owned_fingerprint, revision, updated_at FROM agent_integrations WHERE agent_id = ?1 AND environment = ?2",
                rusqlite::params![agent_id(&requested_agent_id), environment(&requested_environment)],
                |row| Ok(AgentIntegrationEntity {
                    agent_id: requested_agent_id.clone(), environment: requested_environment.clone(), install_state: row.get(0)?,
                    config_path: row.get(1)?, backup_path: row.get(2)?, owned_fingerprint: row.get(3)?,
                    revision: u64::try_from(row.get::<_, i64>(4)?).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(4, 0))?,
                    updated_at: row.get(5)?,
                }),
            ).optional().map_err(CommandError::from)
        })
    }

    pub fn put_integration(
        &self,
        record: &AgentIntegrationEntity,
        expected_revision: Option<u64>,
    ) -> Result<AgentIntegrationEntity, CommandError> {
        if record.config_path.is_empty()
            || record.updated_at < 0
            || !valid_install_state(&record.install_state)
        {
            return Err(invalid_input());
        }
        self.storage.with_transaction(|transaction| {
            let changed = match expected_revision {
                None => transaction.execute(
                    "INSERT OR IGNORE INTO agent_integrations(agent_id, environment, install_state, config_path, backup_path, owned_fingerprint, revision, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7)",
                    rusqlite::params![agent_id(&record.agent_id), environment(&record.environment), record.install_state, record.config_path, record.backup_path, record.owned_fingerprint, record.updated_at],
                )?,
                Some(expected) => transaction.execute(
                    r#"UPDATE agent_integrations SET install_state = ?3, config_path = ?4, backup_path = ?5,
                       owned_fingerprint = ?6, revision = revision + 1, updated_at = ?7
                       WHERE agent_id = ?1 AND environment = ?2 AND revision = ?8"#,
                    rusqlite::params![agent_id(&record.agent_id), environment(&record.environment), record.install_state, record.config_path, record.backup_path, record.owned_fingerprint, record.updated_at, i64::try_from(expected).map_err(|_| invalid_input())?],
                )?,
            };
            if changed == 0 {
                let exists = transaction.query_row("SELECT EXISTS(SELECT 1 FROM agent_integrations WHERE agent_id = ?1 AND environment = ?2)", rusqlite::params![agent_id(&record.agent_id), environment(&record.environment)], |row| row.get::<_, bool>(0))?;
                return Err(if exists { conflict() } else { not_found() });
            }
            let revision: i64 = transaction.query_row("SELECT revision FROM agent_integrations WHERE agent_id = ?1 AND environment = ?2", rusqlite::params![agent_id(&record.agent_id), environment(&record.environment)], |row| row.get(0))?;
            Ok(AgentIntegrationEntity { revision: u64::try_from(revision).map_err(|_| database_failure())?, ..record.clone() })
        })
    }

    pub fn boundary_integration(_record: &AgentIntegrationEntity) -> AgentIntegrationRecord {
        let state = match _record.install_state.as_str() {
            "notInstalled" => IntegrationState::NotInstalled,
            "installed" => IntegrationState::Installed,
            "needsRepair" => IntegrationState::NeedsRepair,
            _ => IntegrationState::Unsupported,
        };
        AgentIntegrationRecord {
            environment: _record.environment.clone(),
            supported: !matches!(
                (&_record.agent_id, &_record.environment),
                (AgentId::Workbuddy, AgentEnvironment::Wsl)
            ),
            required: false,
            state,
            reason_code: (_record.install_state == "unsupported").then_some("unsupported".into()),
        }
    }
}

fn agent_id(value: &AgentId) -> &'static str {
    match value {
        AgentId::Codex => "codex",
        AgentId::Hermes => "hermes",
        AgentId::Workbuddy => "workbuddy",
        AgentId::Claude => "claude",
    }
}
fn environment(value: &AgentEnvironment) -> &'static str {
    match value {
        AgentEnvironment::Windows => "windows",
        AgentEnvironment::Wsl => "wsl",
    }
}
fn status(value: &AgentStatus) -> &'static str {
    match value {
        AgentStatus::Idle => "idle",
        AgentStatus::Running => "running",
        AgentStatus::Completed => "completed",
        AgentStatus::Failed => "failed",
        AgentStatus::Waiting => "waiting",
        AgentStatus::Timeout => "timeout",
        AgentStatus::Offline => "offline",
    }
}
fn parse_agent_id(value: &str) -> Option<AgentId> {
    Some(match value {
        "codex" => AgentId::Codex,
        "hermes" => AgentId::Hermes,
        "workbuddy" => AgentId::Workbuddy,
        "claude" => AgentId::Claude,
        _ => return None,
    })
}
fn parse_environment(value: &str) -> Option<AgentEnvironment> {
    Some(match value {
        "windows" => AgentEnvironment::Windows,
        "wsl" => AgentEnvironment::Wsl,
        _ => return None,
    })
}
fn parse_status(value: &str) -> Option<AgentStatus> {
    Some(match value {
        "idle" => AgentStatus::Idle,
        "running" => AgentStatus::Running,
        "completed" => AgentStatus::Completed,
        "failed" => AgentStatus::Failed,
        "waiting" => AgentStatus::Waiting,
        "timeout" => AgentStatus::Timeout,
        "offline" => AgentStatus::Offline,
        _ => return None,
    })
}
fn valid_install_state(value: &str) -> bool {
    matches!(
        value,
        "notInstalled" | "installed" | "needsRepair" | "unsupported"
    )
}
fn bounded_reply_preview(value: Option<String>) -> Option<String> {
    const MAX_REPLY_PREVIEW_CHARS: usize = 320;
    let value = agent_reply_preview_from_message(value?.as_str())?
        .trim()
        .to_owned();
    if value.is_empty() {
        return None;
    }
    let mut characters = value.chars();
    let mut preview = characters
        .by_ref()
        .take(MAX_REPLY_PREVIEW_CHARS)
        .collect::<String>();
    if characters.next().is_some() {
        preview.push('…');
    }
    Some(preview)
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
    use crate::contracts::{AgentStatus, AppErrorCode};

    fn repository() -> AgentRepository {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.keep();
        AgentRepository::new(Arc::new(Storage::open(&path).unwrap()))
    }

    fn event(id: &str, status: AgentStatus, occurred_at: i64) -> ValidatedAgentEvent {
        ValidatedAgentEvent {
            event_id: id.into(),
            agent_id: AgentId::Codex,
            environment: AgentEnvironment::Windows,
            task_id: "task-1".into(),
            status,
            sequence: Some(1),
            task_title: None,
            project: None,
            message: None,
            path: None,
            occurred_at,
        }
    }

    #[test]
    fn registered_migrations_are_recorded_once_with_their_locked_names() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.keep();
        Storage::open(&path).unwrap();
        Storage::open(&path).unwrap();
        let storage = Storage::open(&path).unwrap();
        let ledger = storage
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare("SELECT version, name FROM schema_migrations ORDER BY version")?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(CommandError::from)?;
                Ok(rows)
            })
            .unwrap();
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
    fn duplicate_event_does_not_replace_the_projected_task() {
        let repository = repository();
        let original = event("event-1", AgentStatus::Running, 100);
        assert!(matches!(
            repository.insert_event_and_project(&original, 101).unwrap(),
            ProjectionOutcome::Advanced { .. }
        ));
        let duplicate = event("event-1", AgentStatus::Completed, 200);
        assert_eq!(
            repository
                .insert_event_and_project(&duplicate, 201)
                .unwrap(),
            ProjectionOutcome::Duplicate
        );
        assert_eq!(
            repository.list_tasks().unwrap()[0].status,
            AgentStatus::Running
        );
    }

    #[test]
    fn get_event_by_id_returns_the_complete_persisted_event() {
        let repository = repository();
        let mut original = event("event-authoritative", AgentStatus::Running, 100);
        original.sequence = Some(10);
        original.task_title = Some("Persisted title".into());
        original.project = Some("Persisted project".into());
        original.message = Some("Persisted message".into());
        original.path = Some("persisted/path".into());
        repository.insert_event_and_project(&original, 101).unwrap();

        assert_eq!(
            repository.get_event_by_id("event-authoritative").unwrap(),
            Some(original)
        );
        assert_eq!(repository.get_event_by_id("missing-event").unwrap(), None);
    }

    #[test]
    fn out_of_order_event_is_audited_without_replacing_the_projected_task() {
        let repository = repository();
        let mut current = event("event-10", AgentStatus::Running, 100);
        current.sequence = Some(10);
        assert!(matches!(
            repository.insert_event_and_project(&current, 101).unwrap(),
            ProjectionOutcome::Advanced { .. }
        ));

        let mut older = event("event-9", AgentStatus::Completed, 200);
        older.sequence = Some(9);
        assert_eq!(
            repository.insert_event_and_project(&older, 201).unwrap(),
            ProjectionOutcome::IgnoredOutOfOrder
        );

        let tasks = repository.list_tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, AgentStatus::Running);
        assert_eq!(tasks[0].source_event_id, "event-10");
        let event_count: i64 = repository
            .storage
            .with_connection(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM agent_events", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .map_err(CommandError::from)
            })
            .unwrap();
        assert_eq!(event_count, 2);
    }

    #[test]
    fn projection_summary_prefers_trimmed_metadata_and_keeps_empty_fallback_semantic() {
        let repository = repository();
        let mut first = event("event-1", AgentStatus::Running, 100);
        first.task_title = Some("   ".into());
        first.message = Some("  In progress  ".into());
        first.project = Some("  Project  ".into());
        assert!(matches!(
            repository.insert_event_and_project(&first, 101).unwrap(),
            ProjectionOutcome::Advanced { .. }
        ));
        assert_eq!(repository.list_tasks().unwrap()[0].summary, "In progress");

        let mut second = event("event-2", AgentStatus::Completed, 102);
        second.sequence = Some(2);
        second.task_title = Some("  Native title  ".into());
        second.message = Some("ignored".into());
        assert!(matches!(
            repository.insert_event_and_project(&second, 103).unwrap(),
            ProjectionOutcome::Advanced { .. }
        ));
        assert_eq!(repository.list_tasks().unwrap()[0].summary, "Native title");

        let mut fallback = event("event-3", AgentStatus::Failed, 104);
        fallback.task_id = "task-2".into();
        fallback.task_title = Some(" ".into());
        fallback.message = Some(" ".into());
        fallback.project = Some(" ".into());
        assert!(matches!(
            repository.insert_event_and_project(&fallback, 105).unwrap(),
            ProjectionOutcome::Advanced { .. }
        ));
        let fallback_row = repository
            .list_tasks()
            .unwrap()
            .into_iter()
            .find(|task| task.task_id == "task-2")
            .unwrap();
        assert_eq!(fallback_row.summary, "");
        assert_eq!(fallback_row.status, AgentStatus::Failed);
    }

    #[test]
    fn latest_reply_preview_uses_only_prefixed_agent_messages_including_running_replies() {
        let repository = repository();
        let mut legacy = event("legacy-reply-event", AgentStatus::Completed, 99);
        legacy.message = Some("ambiguous legacy message".into());
        repository.insert_event_and_project(&legacy, 100).unwrap();
        assert!(repository
            .list_tasks()
            .unwrap()
            .into_iter()
            .all(|row| row.latest_reply_preview.is_none()));

        let mut completed = event("reply-event", AgentStatus::Running, 101);
        completed.message = Some("aisland-agent-reply-v1:  Latest Agent reply  ".into());
        repository
            .insert_event_and_project(&completed, 102)
            .unwrap();

        let mut presence = event("presence-event", AgentStatus::Running, 103);
        presence.task_id = "process-presence".into();
        presence.sequence = Some(2);
        presence.message = None;
        repository.insert_event_and_project(&presence, 104).unwrap();

        let rows = repository.list_tasks().unwrap();
        let presence = rows
            .iter()
            .find(|row| row.task_id == "process-presence")
            .unwrap();
        assert_eq!(
            presence.latest_reply_preview.as_deref(),
            Some("Latest Agent reply")
        );

        let mut user_input = event("user-input", AgentStatus::Running, 105);
        user_input.task_id = "task-2".into();
        user_input.sequence = Some(3);
        user_input.message = Some("must not replace reply".into());
        repository
            .insert_event_and_project(&user_input, 106)
            .unwrap();
        let current = repository
            .list_tasks()
            .unwrap()
            .into_iter()
            .find(|row| row.task_id == "task-2")
            .unwrap();
        assert_eq!(
            current.latest_reply_preview.as_deref(),
            Some("Latest Agent reply")
        );
    }

    #[test]
    fn stored_integration_keeps_persistence_fields_separate_from_boundary_dto() {
        let repository = repository();
        let stored = AgentIntegrationEntity {
            agent_id: AgentId::Codex,
            environment: AgentEnvironment::Windows,
            install_state: "installed".into(),
            config_path: "C:\\Users\\A\\.codex\\hooks.json".into(),
            backup_path: Some("C:\\Users\\A\\.codex\\hooks.json.bak".into()),
            owned_fingerprint: Some("sha256".into()),
            revision: 1,
            updated_at: 100,
        };
        assert_eq!(repository.put_integration(&stored, None).unwrap(), stored);
        assert_eq!(
            repository
                .get_integration(AgentId::Codex, AgentEnvironment::Windows)
                .unwrap(),
            Some(stored.clone())
        );
        assert_eq!(
            AgentRepository::boundary_integration(&stored).state,
            crate::contracts::IntegrationState::Installed
        );
        let error = repository.put_integration(&stored, Some(99)).unwrap_err();
        assert_eq!(error.code, AppErrorCode::Conflict);
    }

    #[test]
    fn stale_integration_revision_returns_retryable_conflict_without_writing() {
        let repository = repository();
        let stored = AgentIntegrationEntity {
            agent_id: AgentId::Codex,
            environment: AgentEnvironment::Windows,
            install_state: "installed".into(),
            config_path: "C:\\Users\\A\\.codex\\hooks.json".into(),
            backup_path: None,
            owned_fingerprint: Some("original".into()),
            revision: 1,
            updated_at: 100,
        };
        let original = repository.put_integration(&stored, None).unwrap();
        let replacement = AgentIntegrationEntity {
            owned_fingerprint: Some("replacement".into()),
            updated_at: 200,
            ..original.clone()
        };

        let error = repository
            .put_integration(&replacement, Some(99))
            .unwrap_err();

        assert_eq!(error.code, AppErrorCode::Conflict);
        assert_eq!(error.message_key, "errors.conflict");
        assert!(error.retryable);
        assert_eq!(
            repository
                .get_integration(AgentId::Codex, AgentEnvironment::Windows)
                .unwrap(),
            Some(original)
        );
    }

    #[test]
    fn integration_create_race_returns_retryable_conflict_without_writing() {
        let repository = repository();
        let stored = AgentIntegrationEntity {
            agent_id: AgentId::Codex,
            environment: AgentEnvironment::Windows,
            install_state: "installed".into(),
            config_path: "C:\\Users\\A\\.codex\\hooks.json".into(),
            backup_path: None,
            owned_fingerprint: Some("winner".into()),
            revision: 1,
            updated_at: 100,
        };
        let winner = repository.put_integration(&stored, None).unwrap();
        let loser = AgentIntegrationEntity {
            owned_fingerprint: Some("loser".into()),
            updated_at: 200,
            ..winner.clone()
        };

        let error = repository.put_integration(&loser, None).unwrap_err();

        assert_eq!(error.code, AppErrorCode::Conflict);
        assert_eq!(error.message_key, "errors.conflict");
        assert!(error.retryable);
        assert_eq!(
            repository
                .get_integration(AgentId::Codex, AgentEnvironment::Windows)
                .unwrap(),
            Some(winner)
        );
    }
}
