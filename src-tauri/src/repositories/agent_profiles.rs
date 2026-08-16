use crate::contracts::{
    AgentConfigTarget, AgentEnvironment, AgentIntegrationKind, AgentStatus, AppErrorCode,
    CommandError, DeleteResult, IntegrationState, SafeMessageParameters, TrueLiteral,
};
use crate::domain::agent_profiles::{
    AgentIntegrationId, AgentProfileInstallation, AgentProfileObservation,
    StoredAgentIntegrationProfile, ValidatedAgentProfileEvent,
};
use crate::storage::Storage;
use rusqlite::OptionalExtension;
use std::sync::Arc;

const PROFILE_FIELDS: &str = "id, kind, display_name, environment, config_target_json, event_mapping_json, enabled, revision, created_at, updated_at";
const PROFILE_EVENT_RETENTION: i64 = 1024;
const PROFILE_OBSERVATION_RETENTION: i64 = 1024;

#[derive(Clone)]
pub struct AgentProfileRepository {
    storage: Arc<Storage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentProfileProjectionOutcome {
    Duplicate,
    IgnoredOutOfOrder,
    Advanced,
}

impl AgentProfileRepository {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    pub fn list(&self) -> Result<Vec<StoredAgentIntegrationProfile>, CommandError> {
        self.storage.with_connection(|connection| {
            let query =
                format!("SELECT {PROFILE_FIELDS} FROM agent_integration_profiles ORDER BY id");
            let mut statement = connection.prepare(&query)?;
            let rows = statement
                .query_map([], row_to_profile)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn count_custom_profiles(&self) -> Result<usize, CommandError> {
        self.storage.with_connection(|connection| {
            let count = connection.query_row(
                "SELECT COUNT(*) FROM agent_integration_profiles WHERE kind = 'custom'",
                [],
                |row| row.get::<_, i64>(0),
            )?;
            usize::try_from(count).map_err(|_| database_failure())
        })
    }

    pub fn get(
        &self,
        id: &AgentIntegrationId,
    ) -> Result<StoredAgentIntegrationProfile, CommandError> {
        self.storage.with_connection(|connection| {
            let query =
                format!("SELECT {PROFILE_FIELDS} FROM agent_integration_profiles WHERE id = ?1");
            connection
                .query_row(&query, [id.as_str()], row_to_profile)
                .optional()?
                .ok_or_else(not_found)
        })
    }

    pub fn save(
        &self,
        profile: &StoredAgentIntegrationProfile,
        expected_revision: Option<i64>,
    ) -> Result<StoredAgentIntegrationProfile, CommandError> {
        validate_profile(profile)?;
        let config_target_json =
            serde_json::to_string(&profile.config_target).map_err(|_| database_failure())?;
        let event_mapping_json =
            serde_json::to_string(&profile.event_mapping).map_err(|_| database_failure())?;
        self.storage.with_transaction(|transaction| {
            let changed = match expected_revision {
                None => transaction.execute(
                    "INSERT OR IGNORE INTO agent_integration_profiles(
                        id, kind, display_name, environment, config_target_json,
                        event_mapping_json, enabled, revision, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9)",
                    rusqlite::params![
                        profile.id.as_str(),
                        kind_name(&profile.kind),
                        profile.display_name,
                        environment_name(&profile.environment),
                        config_target_json,
                        event_mapping_json,
                        profile.enabled,
                        profile.created_at,
                        profile.updated_at,
                    ],
                )?,
                Some(expected_revision) => transaction.execute(
                    "UPDATE agent_integration_profiles SET
                        kind = ?2, display_name = ?3, environment = ?4,
                        config_target_json = ?5, event_mapping_json = ?6, enabled = ?7,
                        revision = revision + 1, updated_at = ?8
                     WHERE id = ?1 AND revision = ?9",
                    rusqlite::params![
                        profile.id.as_str(),
                        kind_name(&profile.kind),
                        profile.display_name,
                        environment_name(&profile.environment),
                        config_target_json,
                        event_mapping_json,
                        profile.enabled,
                        profile.updated_at,
                        expected_revision,
                    ],
                )?,
            };
            if changed == 0 {
                return Err(mutation_miss(transaction, profile.id.as_str())?);
            }
            let query =
                format!("SELECT {PROFILE_FIELDS} FROM agent_integration_profiles WHERE id = ?1");
            transaction
                .query_row(&query, [profile.id.as_str()], row_to_profile)
                .map_err(Into::into)
        })
    }

    pub fn delete(
        &self,
        id: &AgentIntegrationId,
        expected_revision: i64,
    ) -> Result<DeleteResult, CommandError> {
        if expected_revision < 1 {
            return Err(invalid_input());
        }
        self.storage.with_transaction(|transaction| {
            let installed: bool = transaction.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM agent_profile_installations
                    WHERE profile_id = ?1 AND state IN ('installed', 'needsRepair')
                 )",
                [id.as_str()],
                |row| row.get(0),
            )?;
            if installed {
                return Err(conflict());
            }
            let changed = transaction.execute(
                "DELETE FROM agent_integration_profiles WHERE id = ?1 AND revision = ?2",
                rusqlite::params![id.as_str(), expected_revision],
            )?;
            if changed == 0 {
                return Err(mutation_miss(transaction, id.as_str())?);
            }
            Ok(DeleteResult {
                id: id.as_str().into(),
                deleted: TrueLiteral,
            })
        })
    }

    pub fn get_installation(
        &self,
        id: &AgentIntegrationId,
    ) -> Result<Option<AgentProfileInstallation>, CommandError> {
        self.storage.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT state, reason_code, owned_resource, owned_fingerprint, external_hash, updated_at
                     FROM agent_profile_installations WHERE profile_id = ?1",
                    [id.as_str()],
                    |row| {
                        Ok(AgentProfileInstallation {
                            profile_id: id.clone(),
                            state: parse_installation_state(&row.get::<_, String>(0)?)?,
                            reason_code: row.get(1)?,
                            owned_resource: row.get(2)?,
                            owned_fingerprint: row.get(3)?,
                            external_hash: row.get(4)?,
                            updated_at: row.get(5)?,
                        })
                    },
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn count_active_custom_installations_excluding(
        &self,
        excluded_id: &AgentIntegrationId,
    ) -> Result<usize, CommandError> {
        self.storage.with_connection(|connection| {
            let count = connection.query_row(
                "SELECT COUNT(*)
                 FROM agent_profile_installations AS installation
                 INNER JOIN agent_integration_profiles AS profile
                    ON profile.id = installation.profile_id
                 WHERE profile.kind = 'custom'
                   AND installation.state IN ('installed', 'needsRepair')
                   AND profile.id <> ?1",
                [excluded_id.as_str()],
                |row| row.get::<_, i64>(0),
            )?;
            usize::try_from(count).map_err(|_| database_failure())
        })
    }

    pub fn set_installation(
        &self,
        installation: &AgentProfileInstallation,
        expected_revision: i64,
        enabled: bool,
    ) -> Result<StoredAgentIntegrationProfile, CommandError> {
        if expected_revision < 1 || installation.updated_at < 0 {
            return Err(invalid_input());
        }
        self.storage.with_transaction(|transaction| {
            let changed = transaction.execute(
                "UPDATE agent_integration_profiles
                 SET enabled = ?2, revision = revision + 1, updated_at = ?3
                 WHERE id = ?1 AND revision = ?4",
                rusqlite::params![
                    installation.profile_id.as_str(),
                    enabled,
                    installation.updated_at,
                    expected_revision,
                ],
            )?;
            if changed == 0 {
                return Err(mutation_miss(
                    transaction,
                    installation.profile_id.as_str(),
                )?);
            }
            transaction.execute(
                "INSERT INTO agent_profile_installations(
                    profile_id, state, reason_code, owned_resource, owned_fingerprint,
                    external_hash, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(profile_id) DO UPDATE SET
                    state = excluded.state,
                    reason_code = excluded.reason_code,
                    owned_resource = excluded.owned_resource,
                    owned_fingerprint = excluded.owned_fingerprint,
                    external_hash = excluded.external_hash,
                    updated_at = excluded.updated_at",
                rusqlite::params![
                    installation.profile_id.as_str(),
                    installation_state_name(&installation.state),
                    installation.reason_code,
                    installation.owned_resource,
                    installation.owned_fingerprint,
                    installation.external_hash,
                    installation.updated_at,
                ],
            )?;
            let query =
                format!("SELECT {PROFILE_FIELDS} FROM agent_integration_profiles WHERE id = ?1");
            transaction
                .query_row(&query, [installation.profile_id.as_str()], row_to_profile)
                .map_err(Into::into)
        })
    }

    pub fn update_installation_health(
        &self,
        id: &AgentIntegrationId,
        state: IntegrationState,
        reason_code: Option<&str>,
        updated_at: i64,
    ) -> Result<(), CommandError> {
        if updated_at < 0 {
            return Err(invalid_input());
        }
        self.storage.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE agent_profile_installations
                 SET state = ?2, reason_code = ?3, updated_at = ?4
                 WHERE profile_id = ?1",
                rusqlite::params![
                    id.as_str(),
                    installation_state_name(&state),
                    reason_code,
                    updated_at,
                ],
            )?;
            if changed == 0 {
                return Err(not_found());
            }
            Ok(())
        })
    }

    pub fn project_event(
        &self,
        event: &ValidatedAgentProfileEvent,
        received_at: i64,
    ) -> Result<AgentProfileProjectionOutcome, CommandError> {
        self.project_event_with_reply(event, None, received_at)
    }

    pub fn project_event_with_reply(
        &self,
        event: &ValidatedAgentProfileEvent,
        latest_reply_preview: Option<&str>,
        received_at: i64,
    ) -> Result<AgentProfileProjectionOutcome, CommandError> {
        if received_at < 0 || event.occurred_at < 0 {
            return Err(invalid_input());
        }
        let latest_reply_preview = latest_reply_preview
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if latest_reply_preview.is_some_and(|value| value.as_bytes().len() > 1024) {
            return Err(invalid_input());
        }
        self.storage.with_transaction(|transaction| {
            let changed = transaction.execute(
                "INSERT OR IGNORE INTO agent_profile_events(
                    event_id, profile_id, native_event, task_id, status,
                    occurred_at, received_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    event.event_id,
                    event.profile_id.as_str(),
                    event.native_event,
                    event.task_id,
                    status_name(&event.status),
                    event.occurred_at,
                    received_at,
                ],
            )?;
            if changed == 0 {
                return Ok(AgentProfileProjectionOutcome::Duplicate);
            }
            transaction.execute(
                "DELETE FROM agent_profile_events
                 WHERE profile_id = ?1 AND rowid IN (
                    SELECT rowid FROM agent_profile_events
                    WHERE profile_id = ?1
                    ORDER BY occurred_at DESC, event_id DESC
                    LIMIT -1 OFFSET ?2
                 )",
                rusqlite::params![event.profile_id.as_str(), PROFILE_EVENT_RETENTION],
            )?;
            let current: Option<(i64, String)> = transaction
                .query_row(
                    "SELECT occurred_at, source_event_id FROM agent_profile_observations
                     WHERE profile_id = ?1 AND task_id = ?2",
                    rusqlite::params![event.profile_id.as_str(), event.task_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if current.is_some_and(|(occurred_at, source_event_id)| {
                event.occurred_at < occurred_at
                    || (event.occurred_at == occurred_at && event.event_id <= source_event_id)
            }) {
                return Ok(AgentProfileProjectionOutcome::IgnoredOutOfOrder);
            }
            transaction.execute(
                "INSERT INTO agent_profile_observations(
                    profile_id, task_id, status, latest_reply_preview, source_event_id,
                    occurred_at, received_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(profile_id, task_id) DO UPDATE SET
                    status = excluded.status,
                    latest_reply_preview = COALESCE(
                        excluded.latest_reply_preview,
                        agent_profile_observations.latest_reply_preview
                    ),
                    source_event_id = excluded.source_event_id,
                    occurred_at = excluded.occurred_at,
                    received_at = excluded.received_at",
                rusqlite::params![
                    event.profile_id.as_str(),
                    event.task_id,
                    status_name(&event.status),
                    latest_reply_preview,
                    event.event_id,
                    event.occurred_at,
                    received_at,
                ],
            )?;
            transaction.execute(
                "DELETE FROM agent_profile_observations
                 WHERE profile_id = ?1 AND rowid IN (
                    SELECT rowid FROM agent_profile_observations
                    WHERE profile_id = ?1
                    ORDER BY
                        CASE
                            WHEN status IN ('running', 'waiting') AND received_at >= ?3 THEN 0
                            ELSE 1
                        END,
                        received_at DESC, source_event_id DESC
                    LIMIT -1 OFFSET ?2
                 )",
                rusqlite::params![
                    event.profile_id.as_str(),
                    PROFILE_OBSERVATION_RETENTION,
                    received_at.saturating_sub(30_000),
                ],
            )?;
            Ok(AgentProfileProjectionOutcome::Advanced)
        })
    }

    pub fn list_observations(
        &self,
        id: &AgentIntegrationId,
    ) -> Result<Vec<AgentProfileObservation>, CommandError> {
        self.storage.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT task_id, status, latest_reply_preview, source_event_id,
                        occurred_at, received_at
                 FROM agent_profile_observations WHERE profile_id = ?1
                 ORDER BY received_at DESC, source_event_id DESC
                 LIMIT ?2",
            )?;
            let rows = statement
                .query_map(
                    rusqlite::params![id.as_str(), PROFILE_OBSERVATION_RETENTION],
                    |row| {
                        Ok(AgentProfileObservation {
                            profile_id: id.clone(),
                            task_id: row.get(0)?,
                            status: parse_status(&row.get::<_, String>(1)?)?,
                            latest_reply_preview: row.get(2)?,
                            source_event_id: row.get(3)?,
                            occurred_at: row.get(4)?,
                            received_at: row.get(5)?,
                        })
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }
}

fn row_to_profile(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredAgentIntegrationProfile> {
    let id =
        AgentIntegrationId::parse(row.get::<_, String>(0)?).ok_or(rusqlite::Error::InvalidQuery)?;
    let kind = match row.get::<_, String>(1)?.as_str() {
        "preset" => AgentIntegrationKind::Preset,
        "custom" => AgentIntegrationKind::Custom,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let environment = match row.get::<_, String>(3)?.as_str() {
        "windows" => AgentEnvironment::Windows,
        "wsl" => AgentEnvironment::Wsl,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let config_target = serde_json::from_str(&row.get::<_, String>(4)?)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let event_mapping = serde_json::from_str(&row.get::<_, String>(5)?)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(StoredAgentIntegrationProfile {
        id,
        kind,
        display_name: row.get(2)?,
        environment,
        config_target,
        event_mapping,
        enabled: row.get(6)?,
        revision: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn validate_profile(profile: &StoredAgentIntegrationProfile) -> Result<(), CommandError> {
    if profile.revision < 0
        || profile.created_at < 0
        || profile.updated_at < profile.created_at
        || profile.display_name.trim() != profile.display_name
        || !(1..=64).contains(&profile.display_name.chars().count())
        || profile.display_name.chars().any(char::is_control)
        || !matches!(
            (&profile.kind, &profile.config_target),
            (
                AgentIntegrationKind::Preset,
                AgentConfigTarget::Preset { .. }
            ) | (
                AgentIntegrationKind::Custom,
                AgentConfigTarget::CustomHook { .. }
            )
        )
    {
        return Err(invalid_input());
    }
    Ok(())
}

fn mutation_miss(
    transaction: &rusqlite::Transaction<'_>,
    id: &str,
) -> Result<CommandError, CommandError> {
    let exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_integration_profiles WHERE id = ?1)",
        [id],
        |row| row.get(0),
    )?;
    Ok(if exists { conflict() } else { not_found() })
}

fn kind_name(kind: &AgentIntegrationKind) -> &'static str {
    match kind {
        AgentIntegrationKind::Preset => "preset",
        AgentIntegrationKind::Custom => "custom",
    }
}

fn environment_name(environment: &AgentEnvironment) -> &'static str {
    match environment {
        AgentEnvironment::Windows => "windows",
        AgentEnvironment::Wsl => "wsl",
    }
}

fn installation_state_name(state: &IntegrationState) -> &'static str {
    match state {
        IntegrationState::NotInstalled => "notInstalled",
        IntegrationState::Installed => "installed",
        IntegrationState::NeedsRepair => "needsRepair",
        IntegrationState::Unsupported => "unsupported",
    }
}

fn parse_installation_state(value: &str) -> rusqlite::Result<IntegrationState> {
    match value {
        "notInstalled" => Ok(IntegrationState::NotInstalled),
        "installed" => Ok(IntegrationState::Installed),
        "needsRepair" => Ok(IntegrationState::NeedsRepair),
        "unsupported" => Ok(IntegrationState::Unsupported),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn status_name(status: &AgentStatus) -> &'static str {
    match status {
        AgentStatus::Idle => "idle",
        AgentStatus::Running => "running",
        AgentStatus::Completed => "completed",
        AgentStatus::Failed => "failed",
        AgentStatus::Waiting => "waiting",
        AgentStatus::Timeout => "timeout",
        AgentStatus::Offline => "offline",
    }
}

fn parse_status(value: &str) -> rusqlite::Result<AgentStatus> {
    match value {
        "idle" => Ok(AgentStatus::Idle),
        "running" => Ok(AgentStatus::Running),
        "completed" => Ok(AgentStatus::Completed),
        "failed" => Ok(AgentStatus::Failed),
        "waiting" => Ok(AgentStatus::Waiting),
        "timeout" => Ok(AgentStatus::Timeout),
        "offline" => Ok(AgentStatus::Offline),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn invalid_input() -> CommandError {
    command_error(AppErrorCode::InvalidInput, "errors.invalidInput", false)
}

fn not_found() -> CommandError {
    command_error(AppErrorCode::NotFound, "errors.notFound", false)
}

fn conflict() -> CommandError {
    command_error(AppErrorCode::Conflict, "errors.conflict", true)
}

fn database_failure() -> CommandError {
    command_error(
        AppErrorCode::DatabaseFailure,
        "errors.databaseFailure",
        false,
    )
}

fn command_error(code: AppErrorCode, message_key: &str, retryable: bool) -> CommandError {
    CommandError {
        code,
        message_key: message_key.into(),
        details: SafeMessageParameters::new(),
        retryable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{
        AgentConfigTarget, AgentEnvironment, AgentEventMapping, AgentIntegrationKind, AgentStatus,
    };

    fn repository() -> AgentProfileRepository {
        let directory = tempfile::tempdir().unwrap().keep();
        AgentProfileRepository::new(Arc::new(Storage::open(&directory).unwrap()))
    }

    #[test]
    fn migration_seeds_stable_preset_profiles_for_each_environment() {
        let profiles = repository().list().unwrap();
        assert_eq!(
            profiles
                .iter()
                .map(|profile| profile.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "cursor-windows",
                "cursor-wsl",
                "kimi-windows",
                "kimi-wsl",
                "qoderwork-windows",
                "qoderwork-wsl",
                "trae-windows",
                "trae-wsl",
            ]
        );
        assert!(profiles
            .iter()
            .all(|profile| profile.kind == AgentIntegrationKind::Preset));
    }

    #[test]
    fn preset_environment_rows_keep_independent_revisions_and_state() {
        let repository = repository();
        let windows_id = AgentIntegrationId::parse("kimi-windows").unwrap();
        let wsl_id = AgentIntegrationId::parse("kimi-wsl").unwrap();
        let mut windows = repository.get(&windows_id).unwrap();
        let wsl_before = repository.get(&wsl_id).unwrap();

        windows.enabled = true;
        windows.updated_at = 11;
        let windows = repository.save(&windows, Some(windows.revision)).unwrap();
        let wsl_after = repository.get(&wsl_id).unwrap();

        assert_eq!(windows.revision, wsl_before.revision + 1);
        assert!(windows.enabled);
        assert_eq!(wsl_after, wsl_before);
    }

    #[test]
    fn custom_profiles_round_trip_and_use_optimistic_revisions() {
        let repository = repository();
        let profile = StoredAgentIntegrationProfile {
            id: AgentIntegrationId::parse(uuid::Uuid::new_v4().to_string()).unwrap(),
            kind: AgentIntegrationKind::Custom,
            display_name: "My Hook".into(),
            environment: AgentEnvironment::Windows,
            config_target: AgentConfigTarget::CustomHook {
                executable: "C:\\tools\\hook.exe".into(),
                argv: vec!["--json".into()],
                working_directory: None,
                timeout_seconds: 10,
            },
            event_mapping: vec![AgentEventMapping {
                native_event: "done".into(),
                normalized_status: AgentStatus::Completed,
            }],
            enabled: false,
            revision: 0,
            created_at: 10,
            updated_at: 10,
        };
        let created = repository.save(&profile, None).unwrap();
        assert_eq!(created.revision, 1);
        assert_eq!(repository.get(&created.id).unwrap(), created);

        let changed = StoredAgentIntegrationProfile {
            enabled: true,
            updated_at: 20,
            ..created.clone()
        };
        let updated = repository.save(&changed, Some(1)).unwrap();
        assert_eq!(updated.revision, 2);
        assert_eq!(
            repository.save(&changed, Some(1)).unwrap_err().code,
            crate::contracts::AppErrorCode::Conflict
        );
        assert_eq!(
            repository.delete(&updated.id, 2).unwrap().deleted,
            crate::contracts::TrueLiteral
        );
    }

    #[test]
    fn dynamic_profile_event_projects_without_legacy_agent_id() {
        let repository = repository();
        let profile_id = AgentIntegrationId::parse("kimi-windows").unwrap();
        let event = ValidatedAgentProfileEvent {
            event_id: "profile-event-1".into(),
            profile_id: profile_id.clone(),
            native_event: "Notification".into(),
            task_id: "task-7".into(),
            status: AgentStatus::Completed,
            occurred_at: 100,
        };

        assert_eq!(
            repository
                .project_event_with_reply(&event, Some("Safe profile reply"), 101)
                .unwrap(),
            AgentProfileProjectionOutcome::Advanced
        );
        assert_eq!(
            repository.project_event(&event, 101).unwrap(),
            AgentProfileProjectionOutcome::Duplicate
        );
        let observations = repository.list_observations(&profile_id).unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].source_event_id, "profile-event-1");
        assert_eq!(observations[0].status, AgentStatus::Completed);
        assert_eq!(
            observations[0].latest_reply_preview.as_deref(),
            Some("Safe profile reply")
        );

        let running = ValidatedAgentProfileEvent {
            event_id: "profile-event-2".into(),
            status: AgentStatus::Running,
            occurred_at: 102,
            ..event
        };
        assert_eq!(
            repository.project_event(&running, 103).unwrap(),
            AgentProfileProjectionOutcome::Advanced
        );
        assert_eq!(
            repository.list_observations(&profile_id).unwrap()[0]
                .latest_reply_preview
                .as_deref(),
            Some("Safe profile reply")
        );
    }

    #[test]
    fn source_event_ids_are_scoped_to_each_profile() {
        let repository = repository();
        for profile in ["kimi-windows", "qoderwork-windows"] {
            let event = ValidatedAgentProfileEvent {
                event_id: "event-1".into(),
                profile_id: AgentIntegrationId::parse(profile).unwrap(),
                native_event: "Stop".into(),
                task_id: "task-1".into(),
                status: AgentStatus::Completed,
                occurred_at: 100,
            };
            assert_eq!(
                repository.project_event(&event, 101).unwrap(),
                AgentProfileProjectionOutcome::Advanced
            );
        }
    }

    #[test]
    fn event_ledger_is_retained_per_profile_without_losing_latest_observation() {
        let repository = repository();
        let profile_id = AgentIntegrationId::parse("kimi-windows").unwrap();
        for sequence in 0..(PROFILE_EVENT_RETENTION + 6) {
            let event = ValidatedAgentProfileEvent {
                event_id: format!("event-{sequence:04}"),
                profile_id: profile_id.clone(),
                native_event: "Notification".into(),
                task_id: "task-retained".into(),
                status: AgentStatus::Completed,
                occurred_at: sequence,
            };
            assert_eq!(
                repository.project_event(&event, sequence).unwrap(),
                AgentProfileProjectionOutcome::Advanced
            );
        }

        let count = repository
            .storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM agent_profile_events WHERE profile_id = ?1",
                        [profile_id.as_str()],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(count, PROFILE_EVENT_RETENTION);
        let observations = repository.list_observations(&profile_id).unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(
            observations[0].source_event_id,
            format!("event-{:04}", PROFILE_EVENT_RETENTION + 5)
        );
    }

    #[test]
    fn observations_are_bounded_and_prioritize_active_tasks() {
        let repository = repository();
        let profile_id = AgentIntegrationId::parse("kimi-windows").unwrap();
        let active = ValidatedAgentProfileEvent {
            event_id: "active-event".into(),
            profile_id: profile_id.clone(),
            native_event: "Notification".into(),
            task_id: "active-task".into(),
            status: AgentStatus::Running,
            occurred_at: 1,
        };
        repository.project_event(&active, 1).unwrap();
        for sequence in 0..(PROFILE_OBSERVATION_RETENTION + 6) {
            let event = ValidatedAgentProfileEvent {
                event_id: format!("terminal-event-{sequence:04}"),
                profile_id: profile_id.clone(),
                native_event: "Notification".into(),
                task_id: format!("terminal-task-{sequence:04}"),
                status: AgentStatus::Completed,
                occurred_at: sequence + 2,
            };
            repository.project_event(&event, sequence + 2).unwrap();
        }

        let observations = repository.list_observations(&profile_id).unwrap();
        assert_eq!(observations.len() as i64, PROFILE_OBSERVATION_RETENTION);
        assert!(observations
            .iter()
            .any(|observation| observation.task_id == "active-task"));
        assert!(observations.iter().any(|observation| {
            observation.source_event_id
                == format!("terminal-event-{:04}", PROFILE_OBSERVATION_RETENTION + 5)
        }));
    }

    #[test]
    fn stale_running_observations_cannot_evict_a_new_terminal_event() {
        let repository = repository();
        let profile_id = AgentIntegrationId::parse("kimi-windows").unwrap();
        for sequence in 0..PROFILE_OBSERVATION_RETENTION {
            let event = ValidatedAgentProfileEvent {
                event_id: format!("stale-running-{sequence:04}"),
                profile_id: profile_id.clone(),
                native_event: "Notification".into(),
                task_id: format!("stale-task-{sequence:04}"),
                status: AgentStatus::Running,
                occurred_at: sequence,
            };
            repository.project_event(&event, sequence).unwrap();
        }
        let current = ValidatedAgentProfileEvent {
            event_id: "current-completed".into(),
            profile_id: profile_id.clone(),
            native_event: "Notification".into(),
            task_id: "current-task".into(),
            status: AgentStatus::Completed,
            occurred_at: 100_000,
        };
        repository.project_event(&current, 100_000).unwrap();

        let observations = repository.list_observations(&profile_id).unwrap();
        assert_eq!(observations.len() as i64, PROFILE_OBSERVATION_RETENTION);
        assert!(observations
            .iter()
            .any(|observation| observation.source_event_id == "current-completed"));
    }

    #[test]
    fn install_state_atomically_controls_enabled_and_guards_delete() {
        let repository = repository();
        let id = AgentIntegrationId::parse("kimi-windows").unwrap();
        let before = repository.get(&id).unwrap();
        assert!(!before.enabled);
        let installation = AgentProfileInstallation {
            profile_id: id.clone(),
            state: IntegrationState::Installed,
            reason_code: None,
            owned_resource: Some("config".into()),
            owned_fingerprint: Some("owned".into()),
            external_hash: Some("external".into()),
            updated_at: 10,
        };
        let installed = repository
            .set_installation(&installation, before.revision, true)
            .unwrap();
        assert!(installed.enabled);
        assert_eq!(installed.revision, before.revision + 1);
        assert_eq!(
            repository.delete(&id, installed.revision).unwrap_err().code,
            AppErrorCode::Conflict
        );

        let removed = AgentProfileInstallation {
            state: IntegrationState::NotInstalled,
            reason_code: None,
            owned_resource: None,
            owned_fingerprint: None,
            external_hash: None,
            updated_at: 11,
            ..installation
        };
        let removed = repository
            .set_installation(&removed, installed.revision, false)
            .unwrap();
        assert!(!removed.enabled);
    }
}
