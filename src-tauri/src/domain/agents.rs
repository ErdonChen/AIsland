use crate::contracts::{
    AgentDisplayName, AgentEnvironment, AgentId, AgentIntegrationRecord, AgentObservation,
    AgentStatus, AgentSummary,
};
use serde::Deserialize;
use std::cmp::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_STATUS_FILE_BYTES: usize = 64 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 256;
const MAX_DISPLAY_BYTES: usize = 1024;
const MAX_PAST_MILLIS: i64 = 24 * 60 * 60 * 1000;
const MAX_FUTURE_MILLIS: i64 = 5 * 60 * 1000;
pub(crate) const COMPLETION_FLASH_MILLIS: i64 = 2_000;

#[derive(Clone, Debug, Deserialize)]
pub struct AgentStatusFileV1 {
    pub schema_version: u32,
    pub event_id: String,
    pub agent: AgentId,
    pub environment: AgentEnvironment,
    pub task_id: String,
    pub status: AgentStatus,
    pub occurred_at: i64,
    pub sequence: Option<u64>,
    pub task_title: Option<String>,
    pub project: Option<String>,
    pub message: Option<String>,
    pub path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusFileFault {
    pub code: &'static str,
    pub agent_id: Option<AgentId>,
    pub environment: Option<AgentEnvironment>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentTaskRecord {
    pub latest_sequence: Option<u64>,
    pub source_event_id: String,
    pub occurred_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventOrder {
    Duplicate,
    Advances,
    OutOfOrder,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedAgentEvent {
    pub event_id: String,
    pub agent_id: AgentId,
    pub environment: AgentEnvironment,
    pub task_id: String,
    pub status: AgentStatus,
    pub sequence: Option<u64>,
    pub task_title: Option<String>,
    pub project: Option<String>,
    pub message: Option<String>,
    pub path: Option<String>,
    pub occurred_at: i64,
}

pub(crate) const AGENT_REPLY_MESSAGE_PREFIX: &str = "aisland-agent-reply-v1:";

pub(crate) fn agent_reply_preview_from_message(message: &str) -> Option<&str> {
    message.strip_prefix(AGENT_REPLY_MESSAGE_PREFIX)
}

pub fn parse_status_file(
    file_name: &str,
    bytes: &[u8],
) -> Result<ValidatedAgentEvent, StatusFileFault> {
    parse_status_file_at(file_name, bytes, utc_unix_millis())
}

pub(crate) fn parse_status_file_at(
    file_name: &str,
    bytes: &[u8],
    received_at: i64,
) -> Result<ValidatedAgentEvent, StatusFileFault> {
    let expected_identity =
        expected_identity(file_name).ok_or_else(|| fault("unknownFile", None, None))?;
    if bytes.len() > MAX_STATUS_FILE_BYTES {
        return Err(fault("payloadTooLarge", None, None));
    }

    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let status_file = AgentStatusFileV1::deserialize(&mut deserializer)
        .map_err(|_| fault("invalidPayload", None, None))?;
    deserializer
        .end()
        .map_err(|_| fault("invalidPayload", None, None))?;

    let validated_identity = Some((status_file.agent.clone(), status_file.environment.clone()));
    if status_file.schema_version != 1 {
        return Err(fault(
            "unsupportedSchema",
            validated_identity.as_ref().map(|(agent, _)| agent.clone()),
            validated_identity
                .as_ref()
                .map(|(_, environment)| environment.clone()),
        ));
    }
    if expected_identity != (status_file.agent.clone(), status_file.environment.clone()) {
        return Err(fault(
            "filenameIdentityMismatch",
            Some(status_file.agent),
            Some(status_file.environment),
        ));
    }
    if status_file.event_id.is_empty()
        || status_file.task_id.is_empty()
        || status_file.event_id.len() > MAX_IDENTIFIER_BYTES
        || status_file.task_id.len() > MAX_IDENTIFIER_BYTES
    {
        return Err(fault(
            "invalidIdentifier",
            validated_identity.as_ref().map(|(agent, _)| agent.clone()),
            validated_identity
                .as_ref()
                .map(|(_, environment)| environment.clone()),
        ));
    }
    if status_file.occurred_at < received_at.saturating_sub(MAX_PAST_MILLIS)
        || status_file.occurred_at > received_at.saturating_add(MAX_FUTURE_MILLIS)
    {
        return Err(fault(
            "timestampOutOfRange",
            validated_identity.as_ref().map(|(agent, _)| agent.clone()),
            validated_identity
                .as_ref()
                .map(|(_, environment)| environment.clone()),
        ));
    }

    let task_title = normalize_display(status_file.task_title, &validated_identity)?;
    let project = normalize_display(status_file.project, &validated_identity)?;
    let message = normalize_display(status_file.message, &validated_identity)?;
    let path = normalize_display(status_file.path, &validated_identity)?;
    Ok(ValidatedAgentEvent {
        event_id: status_file.event_id,
        agent_id: status_file.agent,
        environment: status_file.environment,
        task_id: status_file.task_id,
        status: status_file.status,
        sequence: status_file.sequence,
        task_title,
        project,
        message,
        path,
        occurred_at: status_file.occurred_at,
    })
}

pub fn compare_task_event(current: &AgentTaskRecord, incoming: &ValidatedAgentEvent) -> EventOrder {
    if current.source_event_id == incoming.event_id {
        return EventOrder::Duplicate;
    }
    match (current.latest_sequence, incoming.sequence) {
        (Some(current_sequence), Some(incoming_sequence)) => {
            if incoming_sequence > current_sequence {
                EventOrder::Advances
            } else {
                EventOrder::OutOfOrder
            }
        }
        (None, Some(_)) => EventOrder::Advances,
        (Some(_), None) => EventOrder::OutOfOrder,
        (None, None) => match incoming.occurred_at.cmp(&current.occurred_at) {
            Ordering::Greater => EventOrder::Advances,
            Ordering::Less => EventOrder::OutOfOrder,
            Ordering::Equal if incoming.event_id > current.source_event_id => EventOrder::Advances,
            Ordering::Equal => EventOrder::OutOfOrder,
        },
    }
}

pub fn aggregate_agent(
    agent_id: AgentId,
    observations: &[AgentObservation],
    integrations: &[AgentIntegrationRecord],
) -> AgentSummary {
    aggregate_agent_with_clock(agent_id, observations, integrations, None)
}

pub fn aggregate_agent_at(
    agent_id: AgentId,
    observations: &[AgentObservation],
    integrations: &[AgentIntegrationRecord],
    generated_at: i64,
) -> AgentSummary {
    aggregate_agent_with_clock(agent_id, observations, integrations, Some(generated_at))
}

fn aggregate_agent_with_clock(
    agent_id: AgentId,
    observations: &[AgentObservation],
    integrations: &[AgentIntegrationRecord],
    generated_at: Option<i64>,
) -> AgentSummary {
    let environments = observations
        .iter()
        .filter(|observation| observation.agent_id == agent_id)
        .cloned()
        .collect::<Vec<_>>();
    let steady_status = environments
        .iter()
        .reduce(|current, incoming| {
            if observation_precedes(incoming, current) {
                incoming
            } else {
                current
            }
        })
        .map(|observation| observation.status.clone())
        .unwrap_or(AgentStatus::Offline);
    let aggregate_status = if steady_status == AgentStatus::Running
        && generated_at.is_some_and(|now| {
            environments.iter().any(|observation| {
                observation.status == AgentStatus::Completed
                    && observation.received_at <= now
                    && now.saturating_sub(observation.received_at) < COMPLETION_FLASH_MILLIS
            })
        }) {
        AgentStatus::Completed
    } else {
        steady_status
    };
    AgentSummary {
        display_name: display_name(&agent_id),
        agent_id,
        aggregate_status,
        environments,
        integrations: integrations.to_vec(),
    }
}

pub fn sort_collapsed_agents(agents: &mut [AgentSummary]) {
    agents.sort_by(|left, right| {
        status_rank(&left.aggregate_status)
            .cmp(&status_rank(&right.aggregate_status))
            .then_with(|| newest_observation(right).cmp(&newest_observation(left)))
            .then_with(|| agent_rank(&left.agent_id).cmp(&agent_rank(&right.agent_id)))
    });
}

pub(crate) fn projection_summary(event: &ValidatedAgentEvent) -> String {
    [
        event.task_title.as_deref(),
        event
            .message
            .as_deref()
            .map(|message| agent_reply_preview_from_message(message).unwrap_or(message)),
        event.project.as_deref(),
    ]
    .into_iter()
    .find_map(|value| {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
    .unwrap_or_default()
}

fn expected_identity(file_name: &str) -> Option<(AgentId, AgentEnvironment)> {
    Some(match file_name {
        "codex-windows.json" => (AgentId::Codex, AgentEnvironment::Windows),
        "codex-wsl.json" => (AgentId::Codex, AgentEnvironment::Wsl),
        "hermes-windows.json" => (AgentId::Hermes, AgentEnvironment::Windows),
        "hermes-wsl.json" => (AgentId::Hermes, AgentEnvironment::Wsl),
        "workbuddy-windows.json" => (AgentId::Workbuddy, AgentEnvironment::Windows),
        "claude-windows.json" => (AgentId::Claude, AgentEnvironment::Windows),
        "claude-wsl.json" => (AgentId::Claude, AgentEnvironment::Wsl),
        _ => return None,
    })
}

fn normalize_display(
    value: Option<String>,
    identity: &Option<(AgentId, AgentEnvironment)>,
) -> Result<Option<String>, StatusFileFault> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim().to_owned();
    if value.len() > MAX_DISPLAY_BYTES {
        return Err(fault(
            "displayValueTooLarge",
            identity.as_ref().map(|(agent, _)| agent.clone()),
            identity
                .as_ref()
                .map(|(_, environment)| environment.clone()),
        ));
    }
    Ok((!value.is_empty()).then_some(value))
}

fn fault(
    code: &'static str,
    agent_id: Option<AgentId>,
    environment: Option<AgentEnvironment>,
) -> StatusFileFault {
    StatusFileFault {
        code,
        agent_id,
        environment,
    }
}

fn utc_unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn display_name(agent_id: &AgentId) -> AgentDisplayName {
    match agent_id {
        AgentId::Codex => AgentDisplayName::Codex,
        AgentId::Hermes => AgentDisplayName::Hermes,
        AgentId::Workbuddy => AgentDisplayName::WorkBuddy,
        AgentId::Claude => AgentDisplayName::Claude,
    }
}

fn status_rank(status: &AgentStatus) -> u8 {
    match status {
        AgentStatus::Failed | AgentStatus::Timeout => 0,
        AgentStatus::Waiting => 1,
        AgentStatus::Running => 2,
        AgentStatus::Completed => 3,
        AgentStatus::Idle => 4,
        AgentStatus::Offline => 5,
    }
}

fn observation_precedes(left: &AgentObservation, right: &AgentObservation) -> bool {
    match status_rank(&left.status).cmp(&status_rank(&right.status)) {
        Ordering::Less => true,
        Ordering::Greater => false,
        Ordering::Equal
            if left.status == AgentStatus::Failed && right.status == AgentStatus::Timeout =>
        {
            true
        }
        Ordering::Equal
            if left.status == AgentStatus::Timeout && right.status == AgentStatus::Failed =>
        {
            false
        }
        Ordering::Equal => left
            .occurred_at
            .cmp(&right.occurred_at)
            .then_with(|| left.source_event_id.cmp(&right.source_event_id))
            .is_gt(),
    }
}

fn newest_observation(agent: &AgentSummary) -> i64 {
    agent
        .environments
        .iter()
        .map(|observation| observation.occurred_at)
        .max()
        .unwrap_or(i64::MIN)
}

fn agent_rank(agent_id: &AgentId) -> u8 {
    match agent_id {
        AgentId::Codex => 0,
        AgentId::Hermes => 1,
        AgentId::Workbuddy => 2,
        AgentId::Claude => 3,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentIntegrationEntity {
    pub agent_id: AgentId,
    pub environment: AgentEnvironment,
    pub install_state: String,
    pub config_path: String,
    pub backup_path: Option<String>,
    pub owned_fingerprint: Option<String>,
    pub revision: u64,
    pub updated_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{
        AgentDisplayName, AgentIntegrationRecord, AgentObservation, IntegrationState,
    };

    const RECEIVED_AT: i64 = 1_786_118_400_000;

    fn matching_payload(agent: &str, environment: &str) -> Vec<u8> {
        serde_json::json!({
            "schema_version": 1,
            "event_id": format!("{agent}-{environment}-event"),
            "agent": agent,
            "environment": environment,
            "task_id": "task-1",
            "status": "running",
            "occurred_at": RECEIVED_AT,
            "task_title": "  Native task  ",
            "project": "  Project  ",
            "message": "  Working  ",
            "path": "  C:\\work  "
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn accepts_each_approved_file_with_matching_schema_one_payload() {
        let cases = [
            ("codex-windows.json", "codex", "windows"),
            ("codex-wsl.json", "codex", "wsl"),
            ("hermes-windows.json", "hermes", "windows"),
            ("hermes-wsl.json", "hermes", "wsl"),
            ("workbuddy-windows.json", "workbuddy", "windows"),
            ("claude-windows.json", "claude", "windows"),
            ("claude-wsl.json", "claude", "wsl"),
        ];

        for (file_name, agent, environment) in cases {
            let event = parse_status_file_at(
                file_name,
                &matching_payload(agent, environment),
                RECEIVED_AT,
            )
            .unwrap();
            assert_eq!(event.task_title.as_deref(), Some("Native task"));
            assert_eq!(event.project.as_deref(), Some("Project"));
            assert_eq!(event.message.as_deref(), Some("Working"));
            assert_eq!(event.path.as_deref(), Some("C:\\work"));
        }
    }

    #[test]
    fn rejects_invalid_identity_schema_size_shape_and_timestamp_without_projection() {
        let invalid_cases = [
            (
                "workbuddy-wsl.json",
                matching_payload("workbuddy", "wsl"),
            ),
            (
                "codex-windows.json",
                matching_payload("unknown", "windows"),
            ),
            (
                "codex-windows.json",
                matching_payload("hermes", "windows"),
            ),
            (
                "codex-windows.json",
                matching_payload("codex", "unknown"),
            ),
            (
                "codex-windows.json",
                br#"{"schema_version":1,"event_id":"event","agent":"codex","environment":"windows","task_id":"task","status":"running"}"#.to_vec(),
            ),
            (
                "codex-windows.json",
                br#"{"schema_version":2,"event_id":"event","agent":"codex","environment":"windows","task_id":"task","status":"running","occurred_at":1786118400000}"#.to_vec(),
            ),
        ];

        for (file_name, bytes) in invalid_cases {
            assert!(parse_status_file_at(file_name, &bytes, RECEIVED_AT).is_err());
        }
        assert!(parse_status_file_at(
            "codex-windows.json",
            &vec![b'x'; 64 * 1024 + 1],
            RECEIVED_AT,
        )
        .is_err());
        assert!(parse_status_file_at(
            "codex-windows.json",
            b"{\"schema_version\":1,\"event_id\":\"event\",\"agent\":\"codex\",\"environment\":\"windows\",\"task_id\":\"task\",\"status\":\"running\",\"occurred_at\":1786118400000} {}",
            RECEIVED_AT,
        )
        .is_err());
    }

    #[test]
    fn validates_field_limits_and_fixed_timestamp_window() {
        let mut value: serde_json::Value =
            serde_json::from_slice(&matching_payload("codex", "windows")).unwrap();
        value["event_id"] = serde_json::Value::String("x".repeat(257));
        assert!(parse_status_file_at(
            "codex-windows.json",
            value.to_string().as_bytes(),
            RECEIVED_AT
        )
        .is_err());
        value["event_id"] = serde_json::Value::String("event".into());
        value["task_id"] = serde_json::Value::String("x".repeat(257));
        assert!(parse_status_file_at(
            "codex-windows.json",
            value.to_string().as_bytes(),
            RECEIVED_AT
        )
        .is_err());
        value["task_id"] = serde_json::Value::String("task".into());
        value["message"] = serde_json::Value::String("x".repeat(1025));
        assert!(parse_status_file_at(
            "codex-windows.json",
            value.to_string().as_bytes(),
            RECEIVED_AT
        )
        .is_err());
        value["message"] = serde_json::Value::String("message".into());
        value["occurred_at"] = serde_json::json!(RECEIVED_AT - 86_400_001);
        assert!(parse_status_file_at(
            "codex-windows.json",
            value.to_string().as_bytes(),
            RECEIVED_AT
        )
        .is_err());
        value["occurred_at"] = serde_json::json!(RECEIVED_AT + 300_001);
        assert!(parse_status_file_at(
            "codex-windows.json",
            value.to_string().as_bytes(),
            RECEIVED_AT
        )
        .is_err());
    }

    fn event(
        id: &str,
        sequence: Option<u64>,
        occurred_at: i64,
        status: AgentStatus,
    ) -> ValidatedAgentEvent {
        ValidatedAgentEvent {
            event_id: id.into(),
            agent_id: AgentId::Codex,
            environment: AgentEnvironment::Windows,
            task_id: "task-1".into(),
            status,
            sequence,
            task_title: None,
            project: None,
            message: None,
            path: None,
            occurred_at,
        }
    }

    fn current(sequence: Option<u64>, event_id: &str, occurred_at: i64) -> AgentTaskRecord {
        AgentTaskRecord {
            latest_sequence: sequence,
            source_event_id: event_id.into(),
            occurred_at,
        }
    }

    #[test]
    fn orders_sequence_then_timestamp_and_event_id_deterministically() {
        assert_eq!(
            compare_task_event(
                &current(Some(3), "event-3", 100),
                &event("event-4", Some(4), 1, AgentStatus::Running)
            ),
            EventOrder::Advances
        );
        assert_eq!(
            compare_task_event(
                &current(Some(3), "event-3", 100),
                &event("event-2", Some(3), 200, AgentStatus::Running)
            ),
            EventOrder::OutOfOrder
        );
        assert_eq!(
            compare_task_event(
                &current(Some(3), "event-3", 100),
                &event("event-2", Some(2), 200, AgentStatus::Running)
            ),
            EventOrder::OutOfOrder
        );
        assert_eq!(
            compare_task_event(
                &current(None, "event-1", 100),
                &event("event-2", Some(0), 1, AgentStatus::Running)
            ),
            EventOrder::Advances
        );
        assert_eq!(
            compare_task_event(
                &current(Some(1), "event-1", 100),
                &event("event-2", None, 200, AgentStatus::Running)
            ),
            EventOrder::OutOfOrder
        );
        assert_eq!(
            compare_task_event(
                &current(None, "event-1", 100),
                &event("event-2", None, 101, AgentStatus::Running)
            ),
            EventOrder::Advances
        );
        assert_eq!(
            compare_task_event(
                &current(None, "event-2", 100),
                &event("event-1", None, 100, AgentStatus::Running)
            ),
            EventOrder::OutOfOrder
        );
        assert_eq!(
            compare_task_event(
                &current(None, "event-1", 100),
                &event("event-2", None, 100, AgentStatus::Running)
            ),
            EventOrder::Advances
        );
    }

    fn observation(
        agent_id: AgentId,
        status: AgentStatus,
        occurred_at: i64,
        event_id: &str,
    ) -> AgentObservation {
        AgentObservation {
            agent_id,
            environment: AgentEnvironment::Windows,
            task_id: "task-1".into(),
            status,
            summary: String::new(),
            latest_reply_preview: None,
            source_event_id: event_id.into(),
            occurred_at,
            received_at: occurred_at,
        }
    }

    #[test]
    fn aggregates_by_status_rank_then_timestamp_then_event_id_and_keeps_display_names() {
        let observations = [
            observation(AgentId::Codex, AgentStatus::Timeout, 10, "z"),
            observation(AgentId::Codex, AgentStatus::Failed, 1, "a"),
            observation(AgentId::Codex, AgentStatus::Waiting, 100, "x"),
        ];
        let summary = aggregate_agent(AgentId::Codex, &observations, &[]);
        assert_eq!(summary.aggregate_status, AgentStatus::Failed);
        assert_eq!(summary.display_name, AgentDisplayName::Codex);

        let statuses = [
            AgentStatus::Waiting,
            AgentStatus::Running,
            AgentStatus::Completed,
            AgentStatus::Idle,
            AgentStatus::Offline,
        ];
        for (index, status) in statuses.into_iter().enumerate() {
            let left = aggregate_agent(
                AgentId::Hermes,
                &[observation(AgentId::Hermes, status.clone(), 1, "a")],
                &[],
            );
            let right = aggregate_agent(
                AgentId::Hermes,
                &[observation(AgentId::Hermes, status, 2, "b")],
                &[],
            );
            assert_eq!(
                left.aggregate_status, right.aggregate_status,
                "rank {index}"
            );
        }

        let same_rank = aggregate_agent(
            AgentId::Claude,
            &[
                observation(AgentId::Claude, AgentStatus::Running, 10, "a"),
                observation(AgentId::Claude, AgentStatus::Running, 10, "b"),
            ],
            &[],
        );
        assert_eq!(same_rank.environments.len(), 2);
        assert_eq!(same_rank.aggregate_status, AgentStatus::Running);
        assert_eq!(AgentId::Codex.display_name(), "Codex");
        assert_eq!(AgentId::Hermes.display_name(), "Hermes");
        assert_eq!(AgentId::Workbuddy.display_name(), "WorkBuddy");
        assert_eq!(AgentId::Claude.display_name(), "claude");

        let integration = AgentIntegrationRecord {
            environment: AgentEnvironment::Windows,
            supported: true,
            required: false,
            state: IntegrationState::Installed,
            reason_code: None,
        };
        assert_eq!(
            aggregate_agent(AgentId::Codex, &[], &[integration])
                .integrations
                .len(),
            1
        );
    }

    #[test]
    fn sorts_collapsed_agents_by_state_newest_observation_then_fixed_agent_order() {
        let mut agents = vec![
            aggregate_agent(
                AgentId::Claude,
                &[observation(AgentId::Claude, AgentStatus::Running, 5, "c")],
                &[],
            ),
            aggregate_agent(
                AgentId::Workbuddy,
                &[observation(
                    AgentId::Workbuddy,
                    AgentStatus::Running,
                    5,
                    "w",
                )],
                &[],
            ),
            aggregate_agent(
                AgentId::Hermes,
                &[observation(AgentId::Hermes, AgentStatus::Running, 5, "h")],
                &[],
            ),
            aggregate_agent(
                AgentId::Codex,
                &[observation(AgentId::Codex, AgentStatus::Failed, 1, "x")],
                &[],
            ),
        ];
        sort_collapsed_agents(&mut agents);
        assert_eq!(
            agents
                .into_iter()
                .map(|agent| agent.agent_id)
                .collect::<Vec<_>>(),
            vec![
                AgentId::Codex,
                AgentId::Hermes,
                AgentId::Workbuddy,
                AgentId::Claude
            ]
        );
    }
}
