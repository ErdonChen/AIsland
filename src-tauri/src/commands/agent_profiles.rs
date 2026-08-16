use crate::contracts::{
    AgentConfigTarget, AgentEnvironment, AgentEventMapping, AgentIntegrationDiscoveryResult,
    AgentIntegrationKind, AgentIntegrationProfile, AgentProfilesSnapshot, CommandError,
    DeleteResult, TrueLiteral,
};
use crate::services::AppServices;
use std::sync::Arc;

#[tauri::command(rename = "listAgentIntegrationProfiles", rename_all = "camelCase")]
pub fn list_agent_integration_profiles(
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<Vec<AgentIntegrationProfile>, CommandError> {
    services.agent_profiles.list_profiles()
}

#[tauri::command(
    rename = "discoverAgentIntegrationCandidates",
    rename_all = "camelCase"
)]
pub fn discover_agent_integration_candidates(
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<AgentIntegrationDiscoveryResult, CommandError> {
    services.agent_integration_discovery.discover(now_millis())
}

#[tauri::command(rename = "getAgentProfilesSnapshot", rename_all = "camelCase")]
pub fn get_agent_profiles_snapshot(
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<AgentProfilesSnapshot, CommandError> {
    services.agent_profiles.snapshot(now_millis())
}

#[allow(clippy::too_many_arguments)]
#[tauri::command(rename = "saveAgentIntegrationProfile", rename_all = "camelCase")]
pub fn save_agent_integration_profile(
    id: Option<String>,
    kind: AgentIntegrationKind,
    display_name: String,
    environment: AgentEnvironment,
    config_target: AgentConfigTarget,
    event_mapping: Vec<AgentEventMapping>,
    enabled: bool,
    expected_revision: Option<i64>,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<AgentIntegrationProfile, CommandError> {
    services.agent_profiles.save_profile(
        crate::contracts::SaveAgentIntegrationProfileInput {
            id,
            kind,
            display_name,
            environment,
            config_target,
            event_mapping,
            enabled,
            expected_revision,
        },
        now_millis(),
    )
}

#[tauri::command(rename = "installAgentIntegrationProfile", rename_all = "camelCase")]
pub fn install_agent_integration_profile(
    id: String,
    expected_revision: i64,
    confirm_installation: TrueLiteral,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<AgentIntegrationProfile, CommandError> {
    let _ = confirm_installation;
    services
        .agent_profiles
        .install_profile(&id, expected_revision, now_millis())
}

#[tauri::command(rename = "repairAgentIntegrationProfile", rename_all = "camelCase")]
pub fn repair_agent_integration_profile(
    id: String,
    expected_revision: i64,
    confirm_repair: TrueLiteral,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<AgentIntegrationProfile, CommandError> {
    let _ = confirm_repair;
    services
        .agent_profiles
        .repair_profile(&id, expected_revision, now_millis())
}

#[tauri::command(rename = "uninstallAgentIntegrationProfile", rename_all = "camelCase")]
pub fn uninstall_agent_integration_profile(
    id: String,
    expected_revision: i64,
    confirm_owned_removal: TrueLiteral,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<AgentIntegrationProfile, CommandError> {
    let _ = confirm_owned_removal;
    services
        .agent_profiles
        .uninstall_profile(&id, expected_revision, now_millis())
}

#[tauri::command(rename = "deleteAgentIntegrationProfile", rename_all = "camelCase")]
pub fn delete_agent_integration_profile(
    id: String,
    expected_revision: i64,
    confirm_deletion: TrueLiteral,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<DeleteResult, CommandError> {
    let _ = confirm_deletion;
    services
        .agent_profiles
        .delete_profile(&id, expected_revision)
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use crate::commands::AGENT_PROFILE_COMMAND_NAMES;
    use crate::contracts::{
        DeleteAgentIntegrationProfileInput, InstallAgentIntegrationProfileInput,
        RepairAgentIntegrationProfileInput, UninstallAgentIntegrationProfileInput,
    };

    #[test]
    fn manifest_locks_the_eight_exact_camel_case_commands() {
        assert_eq!(
            AGENT_PROFILE_COMMAND_NAMES,
            [
                "listAgentIntegrationProfiles",
                "discoverAgentIntegrationCandidates",
                "getAgentProfilesSnapshot",
                "saveAgentIntegrationProfile",
                "installAgentIntegrationProfile",
                "repairAgentIntegrationProfile",
                "uninstallAgentIntegrationProfile",
                "deleteAgentIntegrationProfile",
            ]
        );
        let source = include_str!("agent_profiles.rs");
        let compact_source = source
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        for wire_name in AGENT_PROFILE_COMMAND_NAMES {
            assert_eq!(
                compact_source
                    .matches(&format!("#[tauri::command(rename=\"{wire_name}\""))
                    .count(),
                1
            );
        }
    }

    #[test]
    fn confirmation_fields_reject_false_at_deserialization_boundary() {
        for value in [
            serde_json::from_value::<InstallAgentIntegrationProfileInput>(serde_json::json!({
                "id":"x", "expectedRevision":1, "confirmInstallation":false
            }))
            .is_err(),
            serde_json::from_value::<RepairAgentIntegrationProfileInput>(serde_json::json!({
                "id":"x", "expectedRevision":1, "confirmRepair":false
            }))
            .is_err(),
            serde_json::from_value::<UninstallAgentIntegrationProfileInput>(serde_json::json!({
                "id":"x", "expectedRevision":1, "confirmOwnedRemoval":false
            }))
            .is_err(),
            serde_json::from_value::<DeleteAgentIntegrationProfileInput>(serde_json::json!({
                "id":"x", "expectedRevision":1, "confirmDeletion":false
            }))
            .is_err(),
        ] {
            assert!(value);
        }
    }
}
