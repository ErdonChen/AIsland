use crate::contracts::{
    AgentEnvironment, AgentId, AgentIntegrationInput, AgentIntegrationResult, AgentsSnapshot,
    AppErrorCode, CommandError, SafeParameterValue, TrueLiteral,
};
use crate::services::AppServices;
use std::sync::Arc;

#[tauri::command(rename = "getAgentsSnapshot", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn getAgentsSnapshot(
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<AgentsSnapshot, CommandError> {
    services.agents_snapshot(now_millis())
}

#[tauri::command(rename = "installAgentIntegration", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn installAgentIntegration(
    agent_id: AgentId,
    environment: AgentEnvironment,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<AgentIntegrationResult, CommandError> {
    validate_agent_integration_input(&AgentIntegrationInput {
        agent_id: agent_id.clone(),
        environment: environment.clone(),
    })?;
    services
        .agent_integrations
        .install(agent_id, environment, now_millis())
}

#[tauri::command(rename = "repairAgentIntegration", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn repairAgentIntegration(
    agent_id: AgentId,
    environment: AgentEnvironment,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<AgentIntegrationResult, CommandError> {
    validate_agent_integration_input(&AgentIntegrationInput {
        agent_id: agent_id.clone(),
        environment: environment.clone(),
    })?;
    services
        .agent_integrations
        .repair(agent_id, environment, now_millis())
}

#[tauri::command(rename = "uninstallAgentIntegration", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn uninstallAgentIntegration(
    agent_id: AgentId,
    environment: AgentEnvironment,
    confirm_owned_removal: TrueLiteral,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<AgentIntegrationResult, CommandError> {
    validate_agent_integration_input(&AgentIntegrationInput {
        agent_id: agent_id.clone(),
        environment: environment.clone(),
    })?;
    let _ = confirm_owned_removal;
    services
        .agent_integrations
        .uninstall(agent_id, environment, true, now_millis())
}

fn validate_agent_integration_input(input: &AgentIntegrationInput) -> Result<(), CommandError> {
    if matches!(
        (&input.agent_id, &input.environment),
        (AgentId::Workbuddy, AgentEnvironment::Wsl)
    ) {
        return Err(CommandError::with_detail(
            AppErrorCode::IntegrationUnsupported,
            "errors.integrationUnsupported",
            "agentName",
            SafeParameterValue::String(input.agent_id.display_name().into()),
            false,
        ));
    }
    Ok(())
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{
        getAgentsSnapshot, installAgentIntegration, repairAgentIntegration,
        uninstallAgentIntegration,
    };
    use crate::contracts::{
        AgentEnvironment, AgentId, AgentIntegrationInput, UninstallAgentIntegrationInput,
    };

    #[test]
    fn exports_the_four_exact_camel_case_agent_commands() {
        let _ = getAgentsSnapshot;
        let _ = installAgentIntegration;
        let _ = repairAgentIntegration;
        let _ = uninstallAgentIntegration;
    }

    #[test]
    fn rejects_workbuddy_wsl_before_an_integration_adapter_is_selected() {
        let error = super::validate_agent_integration_input(&AgentIntegrationInput {
            agent_id: AgentId::Workbuddy,
            environment: AgentEnvironment::Wsl,
        })
        .expect_err("WorkBuddy on WSL is unsupported");
        assert_eq!(
            error.code,
            crate::contracts::AppErrorCode::IntegrationUnsupported
        );
    }

    #[test]
    fn accepts_only_the_true_literal_for_owned_integration_removal() {
        let decoded = serde_json::from_value::<UninstallAgentIntegrationInput>(serde_json::json!({
            "agentId": "claude", "environment": "wsl", "confirmOwnedRemoval": true
        }));
        assert!(decoded.is_ok());
        let rejected =
            serde_json::from_value::<UninstallAgentIntegrationInput>(serde_json::json!({
                "agentId": "claude", "environment": "wsl", "confirmOwnedRemoval": false
            }));
        assert!(rejected.is_err());
    }
}
