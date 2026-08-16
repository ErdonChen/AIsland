use crate::contracts::{
    AgentConfigTarget, AgentEnvironment, AgentEventMapping, AgentIntegrationKind, AgentStatus,
    IntegrationState,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AgentIntegrationId(String);

impl AgentIntegrationId {
    pub fn parse(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        let valid = (1..=64).contains(&value.len())
            && value.starts_with(|character: char| {
                character.is_ascii_lowercase() || character.is_ascii_digit()
            })
            && value.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
            });
        valid.then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredAgentIntegrationProfile {
    pub id: AgentIntegrationId,
    pub kind: AgentIntegrationKind,
    pub display_name: String,
    pub environment: AgentEnvironment,
    pub config_target: AgentConfigTarget,
    pub event_mapping: Vec<AgentEventMapping>,
    pub enabled: bool,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentProfileInstallation {
    pub profile_id: AgentIntegrationId,
    pub state: IntegrationState,
    pub reason_code: Option<String>,
    pub owned_resource: Option<String>,
    pub owned_fingerprint: Option<String>,
    pub external_hash: Option<String>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedAgentProfileEvent {
    pub event_id: String,
    pub profile_id: AgentIntegrationId,
    pub native_event: String,
    pub task_id: String,
    pub status: AgentStatus,
    pub occurred_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentProfileObservation {
    pub profile_id: AgentIntegrationId,
    pub task_id: String,
    pub status: AgentStatus,
    pub latest_reply_preview: Option<String>,
    pub source_event_id: String,
    pub occurred_at: i64,
    pub received_at: i64,
}
