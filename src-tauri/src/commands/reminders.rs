use crate::contracts::{
    AgentId, AgentTriggerStatus, AppErrorCode, CommandError, CommitReminderReplayCursorInput,
    DeleteResult, PendingReminderNavigation, ReminderActionMember, ReminderAlertGroup,
    ReminderMergeIdentity, ReminderReplay, ReminderReplayCursor, ReminderRule, ReminderSound,
    ReplayReminderDeliveriesInput, SafeParameterValue, SaveReminderRuleInput,
};
use crate::services::{reminder_scheduler::ReminderGroupAction, AppServices};
use std::sync::Arc;

#[tauri::command(rename = "listReminderRules", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn listReminderRules(
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<Vec<ReminderRule>, CommandError> {
    services.reminder_service.list_rules()
}

#[tauri::command(rename = "saveReminderRule", rename_all = "camelCase")]
#[allow(non_snake_case, clippy::too_many_arguments)]
pub fn saveReminderRule(
    id: Option<String>,
    agent_ids: Vec<AgentId>,
    trigger_statuses: Vec<AgentTriggerStatus>,
    enabled: bool,
    delay_seconds: i64,
    sound: ReminderSound,
    toast_enabled: bool,
    window_enabled: bool,
    expected_revision: Option<i64>,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<ReminderRule, CommandError> {
    save_reminder_rule_with_services(
        SaveReminderRuleInput {
            id,
            agent_ids,
            trigger_statuses,
            enabled,
            delay_seconds,
            sound,
            toast_enabled,
            window_enabled,
            expected_revision,
        },
        services.inner().as_ref(),
    )
}

fn save_reminder_rule_with_services(
    input: SaveReminderRuleInput,
    services: &AppServices,
) -> Result<ReminderRule, CommandError> {
    services.reminder_service.save_rule(input, now_millis())
}

#[tauri::command(rename = "deleteReminderRule", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn deleteReminderRule(
    id: String,
    expected_revision: u64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<DeleteResult, CommandError> {
    services
        .reminder_service
        .delete_rule(&id, expected_revision, now_millis())
}

#[tauri::command(rename = "replayReminderDeliveries", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn replayReminderDeliveries(
    consumer_id: String,
    after_dispatch_seq: i64,
    limit: i64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<ReminderReplay, CommandError> {
    let input = ReplayReminderDeliveriesInput {
        consumer_id,
        after_dispatch_seq,
        limit,
    };
    validate_replay_input(&input)?;
    services.reminder_service.replay(
        &input.consumer_id,
        input.after_dispatch_seq as u64,
        input.limit as u32,
    )
}

#[tauri::command(rename = "commitReminderReplayCursor", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn commitReminderReplayCursor(
    consumer_id: String,
    last_dispatch_seq: i64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<ReminderReplayCursor, CommandError> {
    let input = CommitReminderReplayCursorInput {
        consumer_id,
        last_dispatch_seq,
    };
    validate_consumer_id(&input.consumer_id)?;
    let sequence = u64::try_from(input.last_dispatch_seq).map_err(|_| invalid_input())?;
    services
        .reminder_service
        .commit_cursor(&input.consumer_id, sequence, now_millis())
}

#[tauri::command(rename = "reloadReminderAlertGroup", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn reloadReminderAlertGroup(
    delivery_id: String,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<Option<ReminderAlertGroup>, CommandError> {
    reload_reminder_alert_group_with_services(&delivery_id, services.inner().as_ref())
}

fn reload_reminder_alert_group_with_services(
    delivery_id: &str,
    services: &AppServices,
) -> Result<Option<ReminderAlertGroup>, CommandError> {
    services.reminder_service.reload_alert_group(delivery_id)
}

#[tauri::command(rename = "acknowledgeReminder", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn acknowledgeReminder(
    merge_identity: ReminderMergeIdentity,
    expected_member_delivery_ids: Vec<String>,
    members: Vec<ReminderActionMember>,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<ReminderAlertGroup, CommandError> {
    apply_group_action(
        merge_identity,
        expected_member_delivery_ids,
        members,
        ReminderGroupAction::Acknowledge,
        services,
    )
}

#[tauri::command(rename = "completeReminder", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn completeReminder(
    merge_identity: ReminderMergeIdentity,
    expected_member_delivery_ids: Vec<String>,
    members: Vec<ReminderActionMember>,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<ReminderAlertGroup, CommandError> {
    apply_group_action(
        merge_identity,
        expected_member_delivery_ids,
        members,
        ReminderGroupAction::Complete,
        services,
    )
}

#[tauri::command(rename = "snoozeReminder", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn snoozeReminder(
    merge_identity: ReminderMergeIdentity,
    expected_member_delivery_ids: Vec<String>,
    members: Vec<ReminderActionMember>,
    snoozed_until: i64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<ReminderAlertGroup, CommandError> {
    apply_group_action(
        merge_identity,
        expected_member_delivery_ids,
        members,
        ReminderGroupAction::Snooze { snoozed_until },
        services,
    )
}

#[tauri::command(rename = "getPendingReminderNavigation", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn getPendingReminderNavigation(
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<Option<PendingReminderNavigation>, CommandError> {
    services.reminder_service.pending_navigation()
}

#[tauri::command(rename = "acknowledgeReminderNavigation", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn acknowledgeReminderNavigation(
    sequence: i64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<(), CommandError> {
    services
        .reminder_service
        .acknowledge_navigation(sequence, now_millis())
}

fn apply_group_action(
    merge_identity: ReminderMergeIdentity,
    expected_member_delivery_ids: Vec<String>,
    members: Vec<ReminderActionMember>,
    action: ReminderGroupAction,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<ReminderAlertGroup, CommandError> {
    apply_group_action_with_services(
        merge_identity,
        expected_member_delivery_ids,
        members,
        action,
        services.inner().as_ref(),
    )
}

fn apply_group_action_with_services(
    merge_identity: ReminderMergeIdentity,
    mut expected_member_delivery_ids: Vec<String>,
    members: Vec<ReminderActionMember>,
    action: ReminderGroupAction,
    services: &AppServices,
) -> Result<ReminderAlertGroup, CommandError> {
    expected_member_delivery_ids.sort();
    let mut member_ids = members
        .iter()
        .map(|member| member.id.clone())
        .collect::<Vec<_>>();
    member_ids.sort();
    if expected_member_delivery_ids.is_empty()
        || expected_member_delivery_ids != member_ids
        || expected_member_delivery_ids
            .windows(2)
            .any(|pair| pair[0] == pair[1])
    {
        return Err(invalid_input());
    }
    services.reminder_service.apply_group_action(
        merge_identity,
        expected_member_delivery_ids,
        members,
        action,
        now_millis(),
    )
}

fn validate_replay_input(input: &ReplayReminderDeliveriesInput) -> Result<(), CommandError> {
    validate_consumer_id(&input.consumer_id)?;
    if input.after_dispatch_seq < 0 || !(1..=200).contains(&input.limit) {
        return Err(invalid_input());
    }
    Ok(())
}

fn validate_consumer_id(consumer_id: &str) -> Result<(), CommandError> {
    if !(1..=64).contains(&consumer_id.len())
        || !consumer_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(invalid_input());
    }
    Ok(())
}

fn invalid_input() -> CommandError {
    CommandError::with_detail(
        AppErrorCode::InvalidInput,
        "errors.invalidInput",
        "reasonCode",
        SafeParameterValue::String("invalidReminderCommandInput".into()),
        false,
    )
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
        acknowledgeReminder, acknowledgeReminderNavigation, commitReminderReplayCursor,
        completeReminder, deleteReminderRule, getPendingReminderNavigation, listReminderRules,
        reloadReminderAlertGroup, replayReminderDeliveries, saveReminderRule, snoozeReminder,
    };
    use crate::contracts::{
        AgentEnvironment, AgentId, AgentTriggerStatus, AppErrorCode, ReminderActionMember,
        ReminderDeliveryState, ReminderMergeIdentity, ReminderSound, ReminderSourceContext,
        ReminderSourceKind, ReplayReminderDeliveriesInput, SafeParameterValue,
        SaveReminderRuleInput,
    };
    use crate::domain::reminders::{EnqueueOutcome, NewReminderDelivery};
    use crate::services::{
        reminder_scheduler::ReminderGroupAction, AppServices, BootstrapModuleStateProvider,
        EventEmitterPort, ModuleStateProvider, ShutdownPort, WalCheckpointPort,
    };
    use crate::storage::Storage;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    struct NoopShutdown;
    #[async_trait::async_trait]
    impl ShutdownPort for NoopShutdown {
        async fn stop_accepting_work(&self) -> Result<(), crate::contracts::CommandError> {
            Ok(())
        }
        async fn stop_optional_modules(&self) -> Result<(), crate::contracts::CommandError> {
            Ok(())
        }
        async fn cancel_core_workers(&self) -> Result<(), crate::contracts::CommandError> {
            Ok(())
        }
    }
    struct NoopCheckpoint;
    impl WalCheckpointPort for NoopCheckpoint {
        fn checkpoint_truncate(&self) -> Result<(), crate::contracts::CommandError> {
            Ok(())
        }
    }
    struct NoopEmitter;
    impl EventEmitterPort for NoopEmitter {
        fn emit(
            &self,
            _: &'static str,
            _: serde_json::Value,
        ) -> Result<(), crate::contracts::CommandError> {
            Ok(())
        }
    }

    fn services() -> (tempfile::TempDir, Arc<AppServices>) {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let services = AppServices::from_parts(
            storage,
            Arc::new(BootstrapModuleStateProvider) as Arc<dyn ModuleStateProvider>,
            Arc::new(NoopShutdown),
            Arc::new(NoopCheckpoint),
            Arc::new(NoopEmitter),
        );
        (directory, services)
    }

    fn rule_input() -> SaveReminderRuleInput {
        SaveReminderRuleInput {
            id: None,
            agent_ids: vec![AgentId::Codex],
            trigger_statuses: vec![AgentTriggerStatus::Completed],
            enabled: true,
            delay_seconds: 0,
            sound: ReminderSound::None,
            toast_enabled: true,
            window_enabled: true,
            expected_revision: None,
        }
    }

    fn group_fixture() -> (
        tempfile::TempDir,
        Arc<AppServices>,
        ReminderMergeIdentity,
        Vec<String>,
        Vec<ReminderActionMember>,
    ) {
        let (directory, services) = services();
        let rule = super::save_reminder_rule_with_services(rule_input(), &services).unwrap();
        let rule_id = uuid::Uuid::parse_str(&rule.id).unwrap();
        for index in [2, 1] {
            let source_event_id = format!("event-{index}");
            let outcome = services
                .reminder_service
                .enqueue(
                    NewReminderDelivery {
                        dedupe_key: format!("command-group-{index}"),
                        rule_id: Some(rule_id),
                        source_kind: ReminderSourceKind::Agent,
                        source_entity_id: "agent:rule:codex:windows:task:completed".into(),
                        message_key: "reminders.agent.status".into(),
                        message_parameters: BTreeMap::from([
                            (
                                "agentName".into(),
                                SafeParameterValue::String("Codex".into()),
                            ),
                            (
                                "environment".into(),
                                SafeParameterValue::String("windows".into()),
                            ),
                            ("taskId".into(), SafeParameterValue::String("task".into())),
                            (
                                "taskTitle".into(),
                                SafeParameterValue::String("Task".into()),
                            ),
                            (
                                "triggerStatus".into(),
                                SafeParameterValue::String("completed".into()),
                            ),
                        ]),
                        source_context: ReminderSourceContext::Agent {
                            agent_id: AgentId::Codex,
                            environment: AgentEnvironment::Windows,
                            task_id: "task".into(),
                            task_title: Some("Task".into()),
                            trigger_status: AgentTriggerStatus::Completed,
                            source_event_id,
                            source_occurred_at: index,
                        },
                        source_occurred_at: index,
                        sound: ReminderSound::None,
                        toast_enabled: true,
                        window_enabled: true,
                        due_at: 0,
                    },
                    index,
                )
                .unwrap();
            assert!(matches!(outcome, EnqueueOutcome::Inserted(_)));
        }
        let claimed = services.reminders.claim_due(10, 10).unwrap();
        let mut ids = claimed
            .iter()
            .map(|delivery| delivery.id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        let members = ids
            .iter()
            .rev()
            .map(|id| ReminderActionMember {
                id: id.clone(),
                expected_state: ReminderDeliveryState::Dispatched,
            })
            .collect();
        (
            directory,
            services,
            ReminderMergeIdentity::Agent {
                rule_id: rule_id.to_string(),
                agent_id: AgentId::Codex,
                environment: AgentEnvironment::Windows,
                task_id: "task".into(),
                trigger_status: AgentTriggerStatus::Completed,
            },
            ids,
            members,
        )
    }

    #[test]
    fn exports_the_eleven_exact_camel_case_reminder_commands() {
        let _ = listReminderRules;
        let _ = saveReminderRule;
        let _ = deleteReminderRule;
        let _ = replayReminderDeliveries;
        let _ = commitReminderReplayCursor;
        let _ = reloadReminderAlertGroup;
        let _ = acknowledgeReminder;
        let _ = completeReminder;
        let _ = snoozeReminder;
        let _ = getPendingReminderNavigation;
        let _ = acknowledgeReminderNavigation;
    }

    #[test]
    fn validates_replay_limit_and_consumer_id_at_the_command_boundary() {
        for input in [
            ReplayReminderDeliveriesInput {
                consumer_id: "main-alerts".into(),
                after_dispatch_seq: 0,
                limit: 0,
            },
            ReplayReminderDeliveriesInput {
                consumer_id: "main-alerts".into(),
                after_dispatch_seq: 0,
                limit: 201,
            },
            ReplayReminderDeliveriesInput {
                consumer_id: "bad consumer".into(),
                after_dispatch_seq: 0,
                limit: 1,
            },
        ] {
            let error =
                super::validate_replay_input(&input).expect_err("input must be bounded and stable");
            assert_eq!(error.code, crate::contracts::AppErrorCode::InvalidInput);
        }
        assert!(
            super::validate_replay_input(&ReplayReminderDeliveriesInput {
                consumer_id: "reminder-alert-window.1".into(),
                after_dispatch_seq: 0,
                limit: 200,
            })
            .is_ok()
        );
    }

    #[test]
    fn command_save_delegation_preserves_the_repository_stale_revision_conflict() {
        let (_directory, services) = services();
        let created = super::save_reminder_rule_with_services(rule_input(), &services).unwrap();
        let mut stale = rule_input();
        stale.id = Some(created.id);
        stale.expected_revision = Some(created.revision.saturating_add(1) as i64);
        let error = super::save_reminder_rule_with_services(stale, &services).unwrap_err();
        assert_eq!(error.code, AppErrorCode::Conflict);
    }

    #[test]
    fn command_group_action_seam_preserves_stale_delivery_conflict_without_rewriting_members() {
        let (_directory, services, identity, ids, members) = group_fixture();
        super::apply_group_action_with_services(
            identity.clone(),
            ids.clone(),
            members.clone(),
            ReminderGroupAction::Complete,
            &services,
        )
        .unwrap();

        let error = super::apply_group_action_with_services(
            identity,
            ids,
            members,
            ReminderGroupAction::Acknowledge,
            &services,
        )
        .expect_err("the command seam must preserve the repository's stale member conflict");
        assert_eq!(error.code, AppErrorCode::Conflict);
        assert!(error.retryable);
    }

    #[test]
    fn command_reload_group_bypasses_a_persisted_cursor_past_the_group() {
        // This seam covers command -> service -> repository: cursor replay can be empty while
        // the read-only identity reload still returns the live dispatched group.
        let (_directory, services, _identity, ids, _members) = group_fixture();
        services
            .reminder_service
            .commit_cursor("reminder-alert-window", 9_999_999, 1)
            .unwrap();
        let replay = services
            .reminder_service
            .replay("reminder-alert-window", 0, 200)
            .unwrap();
        assert!(replay.deliveries.is_empty());

        let reloaded = super::reload_reminder_alert_group_with_services(&ids[0], &services)
            .unwrap()
            .expect("identity reload must not be constrained by the replay cursor");
        assert_eq!(
            reloaded
                .members
                .iter()
                .map(|member| member.id.as_str())
                .collect::<Vec<_>>(),
            ids.iter().map(String::as_str).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn all_three_command_group_actions_sort_members_and_return_the_complete_group() {
        for (action, expected_state) in [
            (
                ReminderGroupAction::Acknowledge,
                ReminderDeliveryState::Acknowledged,
            ),
            (
                ReminderGroupAction::Complete,
                ReminderDeliveryState::Completed,
            ),
            (
                ReminderGroupAction::Snooze {
                    snoozed_until: 4_000_000_000_000,
                },
                ReminderDeliveryState::Snoozed,
            ),
        ] {
            let (_directory, services, identity, ids, members) = group_fixture();
            let group = super::apply_group_action_with_services(
                identity,
                ids.iter().rev().cloned().collect(),
                members,
                action,
                &services,
            )
            .unwrap();
            assert_eq!(group.members.len(), 2);
            assert!(group
                .members
                .iter()
                .all(|member| member.state == expected_state));
            let mut returned_ids = group
                .members
                .iter()
                .map(|member| member.id.clone())
                .collect::<Vec<_>>();
            returned_ids.sort();
            assert_eq!(returned_ids, ids);
        }
    }
}
