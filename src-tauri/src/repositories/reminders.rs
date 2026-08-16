use crate::contracts::{
    AppErrorCode, BuiltinReminderSoundId, CommandError, DeleteResult, MessageParameterContract,
    MessageUsage, PendingReminderNavigation, ReminderActionInput, ReminderAlertGroup,
    ReminderDelivery, ReminderDeliveryState, ReminderMergeIdentity, ReminderReplay,
    ReminderReplayCursor, ReminderRule, ReminderSound, ReminderSourceContext, ReminderSourceKind,
    SafeMessageParameters, SafeParameterValue, SaveReminderRuleInput, SnoozeReminderInput,
    TrueLiteral,
};
use crate::domain::reminders::{
    reminder_delivery_payload_is_valid, EnqueueOutcome, NewReminderDelivery,
};
use crate::storage::Storage;
use rusqlite::OptionalExtension;
use std::fs;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct ReminderRepository {
    storage: Arc<Storage>,
}

impl ReminderRepository {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    pub fn list_rules(&self) -> Result<Vec<ReminderRule>, CommandError> {
        self.storage.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, agent_ids_json, trigger_statuses_json, enabled, delay_seconds, sound_json, toast_enabled, window_enabled, revision, created_at, updated_at FROM reminder_rules ORDER BY created_at, id",
            )?;
            let rows = statement.query_map([], row_to_rule)?.collect::<Result<Vec<_>, _>>().map_err(CommandError::from)?;
            Ok(rows)
        })
    }

    pub fn save_rule(
        &self,
        input: SaveReminderRuleInput,
        now: i64,
    ) -> Result<ReminderRule, CommandError> {
        validate_rule_input(&input, now)?;
        let agent_ids_json =
            serde_json::to_string(&input.agent_ids).map_err(|_| invalid_input())?;
        let trigger_statuses_json =
            serde_json::to_string(&input.trigger_statuses).map_err(|_| invalid_input())?;
        let sound = canonical_sound(&input.sound)?;
        let sound_json = serde_json::to_string(&sound).map_err(|_| invalid_input())?;
        self.storage.with_transaction(|transaction| {
            let id = input.id.clone().unwrap_or_else(|| Uuid::new_v4().to_string());
            let changed = match (input.id.as_ref(), input.expected_revision) {
                (None, None) | (Some(_), None) => transaction.execute(
                    "INSERT OR IGNORE INTO reminder_rules(id, agent_ids_json, trigger_statuses_json, enabled, delay_seconds, sound_json, toast_enabled, window_enabled, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                    rusqlite::params![id, agent_ids_json, trigger_statuses_json, input.enabled, input.delay_seconds, sound_json, input.toast_enabled, input.window_enabled, now],
                )?,
                (Some(_), Some(_expected)) => transaction.execute(
                    r#"UPDATE reminder_rules SET agent_ids_json = ?2, trigger_statuses_json = ?3, enabled = ?4,
                       delay_seconds = ?5, sound_json = ?6, toast_enabled = ?7, window_enabled = ?8,
                       revision = revision + 1, updated_at = ?9 WHERE id = ?1 AND revision = ?10"#,
                    rusqlite::params![id, agent_ids_json, trigger_statuses_json, input.enabled, input.delay_seconds, sound_json, input.toast_enabled, input.window_enabled, now, input.expected_revision],
                )?,
                (None, Some(_)) => return Err(invalid_input()),
            };
            if changed == 0 { return Err(retryable_conflict()); }
            if !input.enabled {
                transaction.execute("UPDATE reminder_deliveries SET state = 'cancelled', updated_at = ?2 WHERE rule_id = ?1 AND state = 'pending'", rusqlite::params![id, now])?;
                retain_terminal(transaction)?;
            }
            transaction.query_row(
                "SELECT id, agent_ids_json, trigger_statuses_json, enabled, delay_seconds, sound_json, toast_enabled, window_enabled, revision, created_at, updated_at FROM reminder_rules WHERE id = ?1", [id], row_to_rule,
            ).map_err(CommandError::from)
        })
    }

    pub fn delete_rule(
        &self,
        id: Uuid,
        expected_revision: u64,
        now: i64,
    ) -> Result<DeleteResult, CommandError> {
        self.storage.with_transaction(|transaction| {
            let id = id.to_string();
            let changed = transaction.execute("UPDATE reminder_deliveries SET state = 'cancelled', updated_at = ?2 WHERE rule_id = ?1 AND state = 'pending'", rusqlite::params![id, now])?;
            let deleted = transaction.execute("DELETE FROM reminder_rules WHERE id = ?1 AND revision = ?2", rusqlite::params![id, i64::try_from(expected_revision).map_err(|_| invalid_input())?])?;
            if deleted == 0 {
                let exists = transaction.query_row("SELECT EXISTS(SELECT 1 FROM reminder_rules WHERE id = ?1)", [id.as_str()], |row| row.get::<_, bool>(0))?;
                return Err(if exists {
                    retryable_conflict()
                } else {
                    not_found()
                });
            }
            if changed > 0 { retain_terminal(transaction)?; }
            Ok(DeleteResult { id, deleted: TrueLiteral })
        })
    }

    pub fn enqueue(
        &self,
        request: NewReminderDelivery,
        now: i64,
    ) -> Result<EnqueueOutcome, CommandError> {
        validate_delivery(&request, now)?;
        let sound = canonical_sound(&request.sound)?;
        let parameters_json =
            serde_json::to_string(&request.message_parameters).map_err(|_| invalid_input())?;
        let context_json =
            serde_json::to_string(&request.source_context).map_err(|_| invalid_input())?;
        let sound_json = serde_json::to_string(&sound).map_err(|_| invalid_input())?;
        self.storage.with_transaction(|transaction| {
            let id = Uuid::new_v4().to_string();
            let changed = transaction.execute(
                r#"INSERT OR IGNORE INTO reminder_deliveries(
                    id, dedupe_key, rule_id, source_kind, source_entity_id, message_key,
                    message_parameters_json, source_context_json, source_occurred_at, sound_json,
                    toast_enabled, window_enabled, state, due_at, sound_state, toast_state,
                    window_state, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'pending', ?13,
                    ?14, ?15, ?16, ?17, ?17)"#,
                rusqlite::params![id, request.dedupe_key, request.rule_id.map(|value| value.to_string()), source_kind(&request.source_kind), request.source_entity_id, request.message_key, parameters_json, context_json, request.source_occurred_at, sound_json, request.toast_enabled, request.window_enabled, request.due_at, sound_state(&sound), channel_state(request.toast_enabled), channel_state(request.window_enabled), now],
            )?;
            let delivery = transaction.query_row(
                "SELECT id, dedupe_key, rule_id, source_kind, source_entity_id, message_key, message_parameters_json, source_context_json, source_occurred_at, sound_json, state, due_at, dispatch_seq, first_dispatched_at, last_dispatched_at, acknowledged_at, completed_at, snoozed_until, created_at, updated_at FROM reminder_deliveries WHERE dedupe_key = ?1", [&request.dedupe_key], row_to_delivery,
            )?;
            Ok(if changed == 1 { EnqueueOutcome::Inserted(delivery) } else { EnqueueOutcome::Duplicate(delivery) })
        })
    }

    pub(crate) fn project_current_todo(
        &self,
        reminder_id: &str,
        expected_todo_id: &str,
        expected_revision: i64,
        now: i64,
    ) -> Result<(Option<EnqueueOutcome>, Option<CommandError>, bool), CommandError> {
        if Uuid::parse_str(reminder_id).is_err()
            || Uuid::parse_str(expected_todo_id).is_err()
            || expected_revision < 1
            || now < 0
        {
            return Err(invalid_input());
        }
        self.storage.with_transaction(|transaction| {
            let current = transaction
                .query_row(
                    r#"SELECT todo_reminders.id, todo_reminders.todo_id, todo_reminders.remind_at,
                              todo_reminders.enabled, todo_reminders.revision,
                              todos.title, todos.status
                       FROM todo_reminders
                       JOIN todos ON todos.id = todo_reminders.todo_id
                       WHERE todo_reminders.todo_id = ?1"#,
                    [expected_todo_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, bool>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                        ))
                    },
                )
                .optional()?;
            let expected_source_is_current = current.as_ref().is_some_and(
                |(current_id, todo_id, _, _, revision, _, _)| {
                    current_id == reminder_id
                        && todo_id == expected_todo_id
                        && *revision == expected_revision
                },
            );
            let eligible = current.filter(|(_, todo_id, _, enabled, _, _, todo_status)| {
                todo_id == expected_todo_id && *enabled && todo_status == "open"
            });
            let Some((_, todo_id, remind_at, _, revision, todo_title, _)) = eligible else {
                let cancelled = transaction.execute(
                    r#"UPDATE reminder_deliveries
                       SET state = 'cancelled', updated_at = ?2
                       WHERE source_kind = 'todo' AND source_entity_id = ?1
                         AND state IN ('pending', 'snoozed')"#,
                    rusqlite::params![expected_todo_id, now],
                )?;
                if cancelled > 0 {
                    retain_terminal(transaction)?;
                }
                return Ok((
                    None,
                    Some(if expected_source_is_current {
                        invalid_input()
                    } else {
                        retryable_conflict()
                    }),
                    cancelled > 0,
                ));
            };

            let request = NewReminderDelivery {
                dedupe_key: format!("todo:{todo_id}:{revision}"),
                rule_id: None,
                source_kind: ReminderSourceKind::Todo,
                source_entity_id: todo_id.clone(),
                message_key: "reminders.todo.due".into(),
                message_parameters: std::collections::BTreeMap::from([(
                    "todoTitle".into(),
                    SafeParameterValue::String(todo_title.clone()),
                )]),
                source_context: ReminderSourceContext::Todo {
                    todo_id: todo_id.clone(),
                    reminder_revision: revision,
                    todo_title,
                    source_occurred_at: remind_at,
                },
                source_occurred_at: remind_at,
                sound: ReminderSound::Builtin {
                    sound_id: BuiltinReminderSoundId::SystemNotification,
                },
                toast_enabled: true,
                window_enabled: true,
                due_at: remind_at,
            };
            let cancelled = transaction.execute(
                r#"UPDATE reminder_deliveries
                   SET state = 'cancelled', updated_at = ?3
                   WHERE source_kind = 'todo' AND source_entity_id = ?1
                     AND state IN ('pending', 'snoozed') AND dedupe_key <> ?2"#,
                rusqlite::params![todo_id, request.dedupe_key, now],
            )?;
            if cancelled > 0 {
                retain_terminal(transaction)?;
            }

            let prepared = (|| {
                validate_delivery(&request, now)?;
                let sound = canonical_sound(&request.sound)?;
                let parameters_json = serde_json::to_string(&request.message_parameters)
                    .map_err(|_| invalid_input())?;
                let context_json = serde_json::to_string(&request.source_context)
                    .map_err(|_| invalid_input())?;
                let sound_json = serde_json::to_string(&sound).map_err(|_| invalid_input())?;
                Ok::<_, CommandError>((sound, parameters_json, context_json, sound_json))
            })();
            let (sound, parameters_json, context_json, sound_json) = match prepared {
                Ok(prepared) => prepared,
                Err(error) => {
                    return Ok((
                        None,
                        Some(if expected_source_is_current {
                            error
                        } else {
                            retryable_conflict()
                        }),
                        cancelled > 0,
                    ));
                }
            };
            let id = Uuid::new_v4().to_string();
            let changed = transaction.execute(
                r#"INSERT OR IGNORE INTO reminder_deliveries(
                    id, dedupe_key, rule_id, source_kind, source_entity_id, message_key,
                    message_parameters_json, source_context_json, source_occurred_at, sound_json,
                    toast_enabled, window_enabled, state, due_at, sound_state, toast_state,
                    window_state, created_at, updated_at
                ) VALUES (?1, ?2, NULL, 'todo', ?3, ?4, ?5, ?6, ?7, ?8, 1, 1, 'pending', ?9,
                    ?10, 'pending', 'pending', ?11, ?11)"#,
                rusqlite::params![
                    id,
                    request.dedupe_key,
                    request.source_entity_id,
                    request.message_key,
                    parameters_json,
                    context_json,
                    request.source_occurred_at,
                    sound_json,
                    request.due_at,
                    sound_state(&sound),
                    now,
                ],
            )?;
            let delivery = transaction.query_row(
                "SELECT id, dedupe_key, rule_id, source_kind, source_entity_id, message_key, message_parameters_json, source_context_json, source_occurred_at, sound_json, state, due_at, dispatch_seq, first_dispatched_at, last_dispatched_at, acknowledged_at, completed_at, snoozed_until, created_at, updated_at FROM reminder_deliveries WHERE dedupe_key = ?1",
                [&request.dedupe_key],
                row_to_delivery,
            )?;
            let outcome = if changed == 1 {
                EnqueueOutcome::Inserted(delivery)
            } else {
                EnqueueOutcome::Duplicate(delivery)
            };
            Ok((
                Some(outcome),
                (!expected_source_is_current).then(retryable_conflict),
                cancelled > 0 || changed == 1,
            ))
        })
    }

    pub fn cancel_pending(
        &self,
        kind: ReminderSourceKind,
        source_entity_id: &str,
        now: i64,
    ) -> Result<u64, CommandError> {
        if source_entity_id.is_empty() || now < 0 {
            return Err(invalid_input());
        }
        self.storage.with_transaction(|transaction| {
            let changed = match kind {
                ReminderSourceKind::Todo => transaction.execute(
                    r#"UPDATE reminder_deliveries
                       SET state = 'cancelled', updated_at = ?3
                       WHERE source_kind = 'todo' AND source_entity_id = ?2
                         AND state IN ('pending', 'snoozed')
                         AND dedupe_key <> COALESCE(
                           (
                             SELECT 'todo:' || todo_reminders.todo_id || ':' || todo_reminders.revision
                             FROM todo_reminders
                             JOIN todos ON todos.id = todo_reminders.todo_id
                             WHERE todo_reminders.todo_id = ?2
                               AND todo_reminders.enabled = 1
                               AND todos.status = 'open'
                           ),
                           ''
                         )"#,
                    rusqlite::params![source_kind(&kind), source_entity_id, now],
                )?,
                ReminderSourceKind::Agent | ReminderSourceKind::Monitor => transaction.execute(
                    r#"UPDATE reminder_deliveries SET state = 'cancelled', updated_at = ?3
                       WHERE source_kind = ?1 AND source_entity_id = ?2
                         AND state IN ('pending', 'snoozed')"#,
                    rusqlite::params![source_kind(&kind), source_entity_id, now],
                )?,
            };
            if changed > 0 {
                retain_terminal(transaction)?;
            }
            u64::try_from(changed).map_err(|_| database_failure())
        })
    }

    pub fn claim_due(&self, now: i64, limit: u32) -> Result<Vec<ReminderDelivery>, CommandError> {
        if now < 0 || !(1..=100).contains(&limit) {
            return Err(invalid_input());
        }
        self.storage.with_transaction(|transaction| {
            let mut statement = transaction.prepare("SELECT id FROM reminder_deliveries WHERE state IN ('pending', 'snoozed') AND due_at <= ?1 ORDER BY due_at, created_at, id LIMIT ?2")?;
            let ids = statement.query_map(rusqlite::params![now, limit], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            if ids.is_empty() { return Ok(Vec::new()); }
            let next: i64 = transaction.query_row("SELECT next_dispatch_seq FROM reminder_dispatch_counter WHERE singleton_id = 1", [], |row| row.get(0))?;
            for (offset, id) in ids.iter().enumerate() {
                let sequence = next.checked_add(i64::try_from(offset).map_err(|_| database_failure())?).ok_or_else(database_failure)?;
                transaction.execute(
                    "UPDATE reminder_deliveries SET state = 'dispatched', dispatch_seq = ?2, first_dispatched_at = COALESCE(first_dispatched_at, ?3), last_dispatched_at = ?3, updated_at = ?3 WHERE id = ?1 AND state IN ('pending', 'snoozed')",
                    rusqlite::params![id, sequence, now],
                )?;
            }
            transaction.execute("UPDATE reminder_dispatch_counter SET next_dispatch_seq = ?1 WHERE singleton_id = 1", [next + i64::try_from(ids.len()).map_err(|_| database_failure())?])?;
            ids.into_iter().map(|id| transaction.query_row("SELECT id, dedupe_key, rule_id, source_kind, source_entity_id, message_key, message_parameters_json, source_context_json, source_occurred_at, sound_json, state, due_at, dispatch_seq, first_dispatched_at, last_dispatched_at, acknowledged_at, completed_at, snoozed_until, created_at, updated_at FROM reminder_deliveries WHERE id = ?1", [id], row_to_delivery).map_err(CommandError::from)).collect()
        })
    }

    pub fn persist_channel_result(
        &self,
        delivery_id: &str,
        dispatch_seq: i64,
        channel: &str,
        succeeded: bool,
        error_code: Option<&str>,
        now: i64,
    ) -> Result<(), CommandError> {
        let (state_column, error_column) = match channel {
            "sound" => ("sound_state", "sound_error_code"),
            "toast" => ("toast_state", "toast_error_code"),
            "window" => ("window_state", "window_error_code"),
            _ => return Err(invalid_input()),
        };
        let state = if succeeded { "succeeded" } else { "failed" };
        let query = format!(
            "UPDATE reminder_deliveries SET {state_column} = ?1, {error_column} = ?2, updated_at = ?3 \
             WHERE id = ?4 AND dispatch_seq = ?5 AND {state_column} = 'pending'"
        );
        self.storage.with_connection(|connection| {
            connection.execute(
                &query,
                rusqlite::params![state, error_code, now, delivery_id, dispatch_seq],
            )?;
            Ok(())
        })
    }

    pub fn is_channel_pending(
        &self,
        delivery_id: &str,
        dispatch_seq: i64,
        channel: &str,
    ) -> Result<bool, CommandError> {
        let state_column = match channel {
            "sound" => "sound_state",
            "toast" => "toast_state",
            "window" => "window_state",
            _ => return Err(invalid_input()),
        };
        let query = format!(
            "SELECT EXISTS(SELECT 1 FROM reminder_deliveries WHERE id = ?1 AND dispatch_seq = ?2 AND {state_column} = 'pending')"
        );
        self.storage.with_connection(|connection| {
            connection
                .query_row(
                    &query,
                    rusqlite::params![delivery_id, dispatch_seq],
                    |row| row.get(0),
                )
                .map_err(Into::into)
        })
    }

    pub fn dispatched_delivery(
        &self,
        delivery_id: &str,
        dispatch_seq: i64,
    ) -> Result<Option<ReminderDelivery>, CommandError> {
        self.storage.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, dedupe_key, rule_id, source_kind, source_entity_id, message_key, message_parameters_json, source_context_json, source_occurred_at, sound_json, state, due_at, dispatch_seq, first_dispatched_at, last_dispatched_at, acknowledged_at, completed_at, snoozed_until, created_at, updated_at FROM reminder_deliveries WHERE id = ?1 AND dispatch_seq = ?2 AND state = 'dispatched'",
                    rusqlite::params![delivery_id, dispatch_seq],
                    row_to_delivery,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    /// Toast activation is durable-delivery based.  Channel states are delivery attempts, not
    /// authorization for a user to activate an already displayed Toast.
    pub fn dispatched_delivery_by_id(
        &self,
        delivery_id: &str,
    ) -> Result<Option<ReminderDelivery>, CommandError> {
        self.storage.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, dedupe_key, rule_id, source_kind, source_entity_id, message_key, message_parameters_json, source_context_json, source_occurred_at, sound_json, state, due_at, dispatch_seq, first_dispatched_at, last_dispatched_at, acknowledged_at, completed_at, snoozed_until, created_at, updated_at FROM reminder_deliveries WHERE id = ?1 AND state = 'dispatched'",
                    [delivery_id],
                    row_to_delivery,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn dispatched_with_pending_channels(&self) -> Result<Vec<ReminderDelivery>, CommandError> {
        self.storage.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, dedupe_key, rule_id, source_kind, source_entity_id, message_key, message_parameters_json, source_context_json, source_occurred_at, sound_json, state, due_at, dispatch_seq, first_dispatched_at, last_dispatched_at, acknowledged_at, completed_at, snoozed_until, created_at, updated_at FROM reminder_deliveries WHERE state = 'dispatched' AND (sound_state = 'pending' OR toast_state = 'pending' OR window_state = 'pending') ORDER BY dispatch_seq",
            )?;
            let rows = statement
                .query_map([], row_to_delivery)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(Into::into);
            rows
        })
    }

    pub fn alert_group_for_delivery(
        &self,
        delivery_id: &str,
        dispatch_seq: i64,
    ) -> Result<Option<ReminderAlertGroup>, CommandError> {
        self.storage.with_connection(|connection| {
            let Some(delivery) = connection
                .query_row(
                    "SELECT id, dedupe_key, rule_id, source_kind, source_entity_id, message_key, message_parameters_json, source_context_json, source_occurred_at, sound_json, state, due_at, dispatch_seq, first_dispatched_at, last_dispatched_at, acknowledged_at, completed_at, snoozed_until, created_at, updated_at FROM reminder_deliveries WHERE id = ?1 AND dispatch_seq = ?2 AND state = 'dispatched'",
                    rusqlite::params![delivery_id, dispatch_seq],
                    row_to_delivery,
                )
                .optional()?
            else {
                return Ok(None);
            };
            let identity = delivery_merge_identity(&delivery).ok_or_else(invalid_input)?;
            let mut statement = connection.prepare(
                "SELECT id, dedupe_key, rule_id, source_kind, source_entity_id, message_key, message_parameters_json, source_context_json, source_occurred_at, sound_json, state, due_at, dispatch_seq, first_dispatched_at, last_dispatched_at, acknowledged_at, completed_at, snoozed_until, created_at, updated_at FROM reminder_deliveries WHERE state IN ('dispatched', 'acknowledged')",
            )?;
            let members = statement
                .query_map([], row_to_delivery)?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .filter(|candidate| delivery_merge_identity(candidate).as_ref() == Some(&identity))
                .collect();
            alert_group(identity, members).map(Some)
        })
    }

    pub fn reload_alert_group(
        &self,
        delivery_id: &str,
    ) -> Result<Option<ReminderAlertGroup>, CommandError> {
        self.storage.with_connection(|connection| {
            let Some(member) = connection.query_row(
                "SELECT id, dedupe_key, rule_id, source_kind, source_entity_id, message_key, message_parameters_json, source_context_json, source_occurred_at, sound_json, state, due_at, dispatch_seq, first_dispatched_at, last_dispatched_at, acknowledged_at, completed_at, snoozed_until, created_at, updated_at FROM reminder_deliveries WHERE id = ?1 AND state IN ('dispatched', 'acknowledged')",
                [delivery_id], row_to_delivery,
            ).optional()? else { return Ok(None); };
            let mut statement = connection.prepare(
                "SELECT id, dedupe_key, rule_id, source_kind, source_entity_id, message_key, message_parameters_json, source_context_json, source_occurred_at, sound_json, state, due_at, dispatch_seq, first_dispatched_at, last_dispatched_at, acknowledged_at, completed_at, snoozed_until, created_at, updated_at FROM reminder_deliveries WHERE state IN ('dispatched', 'acknowledged')",
            )?;
            let actionable = statement
                .query_map([], row_to_delivery)?
                .collect::<Result<Vec<_>, _>>()?;
            let initial_identity = delivery_merge_identity(&member).ok_or_else(invalid_input)?;
            let members = match initial_identity {
                ReminderMergeIdentity::Agent { .. } => actionable.into_iter()
                    .filter(|delivery| delivery_merge_identity(delivery).as_ref() == Some(&initial_identity))
                    .collect::<Vec<_>>(),
                ReminderMergeIdentity::Todo { .. } | ReminderMergeIdentity::Monitor { .. } => {
                    let current = actionable.iter()
                        .filter(|delivery| delivery.source_kind == member.source_kind && delivery.source_entity_id == member.source_entity_id)
                        .max_by(|left, right| left.dispatch_seq.cmp(&right.dispatch_seq)
                            .then_with(|| left.source_occurred_at.cmp(&right.source_occurred_at))
                            .then_with(|| left.id.cmp(&right.id)))
                        .ok_or_else(invalid_input)?;
                    let current_identity = delivery_merge_identity(current).ok_or_else(invalid_input)?;
                    actionable.into_iter()
                        .filter(|delivery| delivery_merge_identity(delivery).as_ref() == Some(&current_identity))
                        .collect::<Vec<_>>()
                }
            };
            if members.is_empty() { return Ok(None); }
            let identity = delivery_merge_identity(&members[0]).ok_or_else(invalid_input)?;
            alert_group(identity, members).map(Some)
        })
    }

    pub fn persist_pending_navigation(
        &self,
        pending: &PendingReminderNavigation,
        now: i64,
    ) -> Result<PendingReminderNavigation, CommandError> {
        self.storage.with_transaction(|transaction| {
            let existing = transaction
                .query_row(
                    "SELECT value_json FROM app_settings WHERE key = 'navigation.reminder.pending'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if let Some(value_json) = existing {
                let existing: PendingReminderNavigation =
                    serde_json::from_str(&value_json).map_err(|_| database_failure())?;
                if existing.sequence > pending.sequence {
                    return Ok(existing);
                }
            }
            let value_json = serde_json::to_string(pending).map_err(|_| database_failure())?;
            transaction.execute(
                r#"INSERT INTO app_settings(key, value_json, revision, updated_at) VALUES ('navigation.reminder.pending', ?1, 1, ?2)
                   ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, revision = app_settings.revision + 1, updated_at = excluded.updated_at"#,
                rusqlite::params![value_json, now],
            )?;
            Ok(pending.clone())
        })
    }

    pub fn pending_navigation(&self) -> Result<Option<PendingReminderNavigation>, CommandError> {
        self.storage.with_connection(|connection| {
            let pending = connection
                .query_row(
                    "SELECT value_json FROM app_settings WHERE key = 'navigation.reminder.pending'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .map(|value| serde_json::from_str(&value).map_err(|_| database_failure()))
                .transpose()?;
            let acknowledged = connection
                .query_row(
                    "SELECT value_json FROM app_settings WHERE key = 'navigation.reminder.acknowledged'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .map(|value| serde_json::from_str::<i64>(&value).map_err(|_| database_failure()))
                .transpose()?
                .unwrap_or(0);
            Ok(pending.filter(|value: &PendingReminderNavigation| value.sequence > acknowledged))
        })
    }

    pub fn acknowledge_navigation(&self, sequence: i64, now: i64) -> Result<(), CommandError> {
        if sequence < 0 || now < 0 {
            return Err(invalid_input());
        }
        self.storage.with_transaction(|transaction| {
            let current = transaction
                .query_row(
                    "SELECT value_json FROM app_settings WHERE key = 'navigation.reminder.acknowledged'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .map(|value| serde_json::from_str::<i64>(&value).map_err(|_| database_failure()))
                .transpose()?
                .unwrap_or(0);
            let acknowledged = current.max(sequence);
            let value_json = serde_json::to_string(&acknowledged).map_err(|_| database_failure())?;
            transaction.execute(
                r#"INSERT INTO app_settings(key, value_json, revision, updated_at) VALUES ('navigation.reminder.acknowledged', ?1, 1, ?2)
                   ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, revision = app_settings.revision + 1, updated_at = excluded.updated_at"#,
                rusqlite::params![value_json, now],
            )?;
            Ok(())
        })
    }

    pub fn earliest_due_at(&self) -> Result<Option<i64>, CommandError> {
        self.storage.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT MIN(due_at) FROM reminder_deliveries WHERE state IN ('pending', 'snoozed')",
                    [],
                    |row| row.get(0),
                )
                .map_err(Into::into)
        })
    }

    pub fn replay(
        &self,
        consumer_id: &str,
        after_dispatch_seq: u64,
        limit: u32,
    ) -> Result<ReminderReplay, CommandError> {
        if consumer_id.is_empty() || !(1..=500).contains(&limit) {
            return Err(invalid_input());
        }
        self.storage.with_connection(|connection| {
            let persisted_cursor = connection
                .query_row(
                    "SELECT last_dispatch_seq FROM reminder_consumer_cursors WHERE consumer_id = ?1",
                    [consumer_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(0);
            let supplied_cursor = i64::try_from(after_dispatch_seq).map_err(|_| invalid_input())?;
            let effective_cursor = persisted_cursor.max(supplied_cursor);
            let mut statement = connection.prepare("SELECT id, dedupe_key, rule_id, source_kind, source_entity_id, message_key, message_parameters_json, source_context_json, source_occurred_at, sound_json, state, due_at, dispatch_seq, first_dispatched_at, last_dispatched_at, acknowledged_at, completed_at, snoozed_until, created_at, updated_at FROM reminder_deliveries WHERE dispatch_seq > ?1 ORDER BY dispatch_seq LIMIT ?2")?;
            let mut deliveries = statement.query_map(rusqlite::params![effective_cursor, limit + 1], row_to_delivery)?.collect::<Result<Vec<_>, _>>()?;
            let has_more = deliveries.len() > limit as usize;
            deliveries.truncate(limit as usize);
            let last_dispatch_seq = deliveries.last().map(|value| value.dispatch_seq).unwrap_or(effective_cursor);
            Ok(ReminderReplay { deliveries, last_dispatch_seq, has_more })
        })
    }

    /// Pages the dispatch stream for notification-history projection.
    ///
    /// The returned cursor advances across every observed dispatch sequence,
    /// while only user-visible historical states are projected. That prevents a
    /// cancelled or currently snoozed row from pinning the importer forever.
    pub fn notification_history_page(
        &self,
        after_dispatch_seq: u64,
        limit: u32,
    ) -> Result<ReminderReplay, CommandError> {
        if !(1..=500).contains(&limit) {
            return Err(invalid_input());
        }
        let after_dispatch_seq = i64::try_from(after_dispatch_seq).map_err(|_| invalid_input())?;
        self.storage.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT id, dedupe_key, rule_id, source_kind, source_entity_id, message_key, message_parameters_json, source_context_json, source_occurred_at, sound_json, state, due_at, dispatch_seq, first_dispatched_at, last_dispatched_at, acknowledged_at, completed_at, snoozed_until, created_at, updated_at FROM reminder_deliveries WHERE dispatch_seq > ?1 ORDER BY dispatch_seq LIMIT ?2")?;
            let mut observed = statement
                .query_map(
                    rusqlite::params![after_dispatch_seq, i64::from(limit) + 1],
                    row_to_delivery,
                )?
                .collect::<Result<Vec<_>, _>>()?;
            let has_more = observed.len() > limit as usize;
            observed.truncate(limit as usize);
            let last_dispatch_seq = observed
                .last()
                .map(|delivery| delivery.dispatch_seq)
                .unwrap_or(after_dispatch_seq);
            let deliveries = observed
                .into_iter()
                .filter(|delivery| {
                    matches!(
                        delivery.state,
                        ReminderDeliveryState::Dispatched
                            | ReminderDeliveryState::Acknowledged
                            | ReminderDeliveryState::Completed
                    )
                })
                .collect();
            Ok(ReminderReplay {
                deliveries,
                last_dispatch_seq,
                has_more,
            })
        })
    }

    pub fn commit_cursor(
        &self,
        consumer_id: &str,
        last_dispatch_seq: u64,
        now: i64,
    ) -> Result<ReminderReplayCursor, CommandError> {
        if consumer_id.is_empty() || now < 0 {
            return Err(invalid_input());
        }
        let last_dispatch_seq = i64::try_from(last_dispatch_seq).map_err(|_| invalid_input())?;
        self.storage.with_transaction(|transaction| {
            transaction.execute(
                r#"INSERT INTO reminder_consumer_cursors(consumer_id, last_dispatch_seq, updated_at) VALUES (?1, ?2, ?3)
                   ON CONFLICT(consumer_id) DO UPDATE SET last_dispatch_seq = MAX(reminder_consumer_cursors.last_dispatch_seq, excluded.last_dispatch_seq), updated_at = excluded.updated_at"#,
                rusqlite::params![consumer_id, last_dispatch_seq, now],
            )?;
            let value: i64 = transaction.query_row("SELECT last_dispatch_seq FROM reminder_consumer_cursors WHERE consumer_id = ?1", [consumer_id], |row| row.get(0))?;
            Ok(ReminderReplayCursor { consumer_id: consumer_id.into(), last_dispatch_seq: value })
        })
    }

    pub fn acknowledge(
        &self,
        input: ReminderActionInput,
        now: i64,
    ) -> Result<ReminderAlertGroup, CommandError> {
        self.apply_group_action(
            &input.merge_identity,
            &input.expected_member_delivery_ids,
            &input.members,
            GroupAction::Acknowledge,
            now,
        )
    }

    pub fn complete(
        &self,
        input: ReminderActionInput,
        now: i64,
    ) -> Result<ReminderAlertGroup, CommandError> {
        self.apply_group_action(
            &input.merge_identity,
            &input.expected_member_delivery_ids,
            &input.members,
            GroupAction::Complete,
            now,
        )
    }

    pub fn snooze(
        &self,
        input: SnoozeReminderInput,
        now: i64,
    ) -> Result<ReminderAlertGroup, CommandError> {
        if now < 0
            || input.snoozed_until <= now
            || !valid_action_members(&input.expected_member_delivery_ids, &input.members)
            || input.members.iter().any(|member| {
                !matches!(
                    member.expected_state,
                    ReminderDeliveryState::Dispatched | ReminderDeliveryState::Acknowledged
                )
            })
        {
            return Err(invalid_input());
        }
        self.apply_group_action(
            &input.merge_identity,
            &input.expected_member_delivery_ids,
            &input.members,
            GroupAction::Snooze(input.snoozed_until),
            now,
        )
    }

    fn apply_group_action(
        &self,
        merge_identity: &ReminderMergeIdentity,
        expected_member_delivery_ids: &[String],
        members: &[crate::contracts::ReminderActionMember],
        action: GroupAction,
        now: i64,
    ) -> Result<ReminderAlertGroup, CommandError> {
        if now < 0 || !valid_action_members(expected_member_delivery_ids, members) {
            return Err(invalid_input());
        }
        self.storage.with_transaction(|transaction| {
            let current_group = verify_action_group(
                transaction,
                merge_identity,
                expected_member_delivery_ids,
                members,
            )?;
            match action {
                GroupAction::Snooze(snoozed_until) => {
                    for member in members {
                        transaction.execute(
                            r#"UPDATE reminder_deliveries
                               SET state = 'snoozed', due_at = ?2, dispatch_seq = NULL, snoozed_until = ?2,
                                   sound_state = CASE WHEN json_extract(sound_json, '$.kind') = 'none' THEN 'skipped' ELSE 'pending' END,
                                   sound_error_code = NULL,
                                   toast_state = CASE WHEN toast_enabled = 1 THEN 'pending' ELSE 'skipped' END,
                                   toast_error_code = NULL,
                                   window_state = CASE WHEN window_enabled = 1 THEN 'pending' ELSE 'skipped' END,
                                   window_error_code = NULL, updated_at = ?3
                               WHERE id = ?1"#,
                            rusqlite::params![member.id, snoozed_until, now],
                        )?;
                    }
                }
                GroupAction::Acknowledge | GroupAction::Complete => {
                    let (state, timestamp_column) = match action {
                        GroupAction::Acknowledge => ("acknowledged", "acknowledged_at"),
                        GroupAction::Complete => ("completed", "completed_at"),
                        GroupAction::Snooze(_) => unreachable!(),
                    };
                    let sql = format!("UPDATE reminder_deliveries SET state = '{state}', {timestamp_column} = ?2, updated_at = ?2 WHERE id = ?1");
                    for member in members {
                        transaction.execute(&sql, rusqlite::params![member.id, now])?;
                    }
                }
            }
            let reloaded = reload_deliveries(transaction, &current_group)?;
            let group = alert_group(merge_identity.clone(), reloaded)?;
            retain_terminal(transaction)?;
            Ok(group)
        })
    }
}

#[derive(Clone, Copy)]
enum GroupAction {
    Acknowledge,
    Complete,
    Snooze(i64),
}

fn valid_action_members(
    expected_member_delivery_ids: &[String],
    members: &[crate::contracts::ReminderActionMember],
) -> bool {
    if members.is_empty() || expected_member_delivery_ids.len() != members.len() {
        return false;
    }
    let mut expected = expected_member_delivery_ids.to_vec();
    let mut actual = members
        .iter()
        .map(|member| member.id.clone())
        .collect::<Vec<_>>();
    expected.sort();
    actual.sort();
    expected == actual && actual.windows(2).all(|pair| pair[0] != pair[1])
}

fn verify_action_group(
    transaction: &rusqlite::Transaction<'_>,
    merge_identity: &ReminderMergeIdentity,
    expected_member_delivery_ids: &[String],
    members: &[crate::contracts::ReminderActionMember],
) -> Result<Vec<ReminderDelivery>, CommandError> {
    if !valid_merge_identity_shape(merge_identity, expected_member_delivery_ids) {
        return Err(invalid_input());
    }
    let mut statement = transaction.prepare(
        "SELECT id, dedupe_key, rule_id, source_kind, source_entity_id, message_key, message_parameters_json, source_context_json, source_occurred_at, sound_json, state, due_at, dispatch_seq, first_dispatched_at, last_dispatched_at, acknowledged_at, completed_at, snoozed_until, created_at, updated_at FROM reminder_deliveries WHERE state IN ('dispatched', 'acknowledged')",
    )?;
    let current_group = statement
        .query_map([], row_to_delivery)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|delivery| delivery_merge_identity(delivery).as_ref() == Some(merge_identity))
        .collect::<Vec<_>>();
    let mut actual_ids = current_group
        .iter()
        .map(|delivery| delivery.id.clone())
        .collect::<Vec<_>>();
    let mut expected_ids = expected_member_delivery_ids.to_vec();
    actual_ids.sort();
    expected_ids.sort();
    if actual_ids != expected_ids {
        return Err(retryable_conflict());
    }
    for member in members {
        let Some(delivery) = current_group
            .iter()
            .find(|delivery| delivery.id == member.id)
        else {
            return Err(retryable_conflict());
        };
        if delivery.state != member.expected_state {
            return Err(retryable_conflict());
        }
    }
    Ok(current_group)
}

fn reload_deliveries(
    transaction: &rusqlite::Transaction<'_>,
    current_group: &[ReminderDelivery],
) -> Result<Vec<ReminderDelivery>, CommandError> {
    current_group
        .iter()
        .map(|delivery| {
            transaction
                .query_row(
                    "SELECT id, dedupe_key, rule_id, source_kind, source_entity_id, message_key, message_parameters_json, source_context_json, source_occurred_at, sound_json, state, due_at, dispatch_seq, first_dispatched_at, last_dispatched_at, acknowledged_at, completed_at, snoozed_until, created_at, updated_at FROM reminder_deliveries WHERE id = ?1",
                    [&delivery.id],
                    row_to_delivery,
                )
                .map_err(Into::into)
        })
        .collect()
}

fn alert_group(
    merge_identity: ReminderMergeIdentity,
    mut members: Vec<ReminderDelivery>,
) -> Result<ReminderAlertGroup, CommandError> {
    members.sort_by(|left, right| left.id.cmp(&right.id));
    let source_context = members
        .iter()
        .max_by(|left, right| {
            left.source_occurred_at
                .cmp(&right.source_occurred_at)
                .then_with(|| left.id.cmp(&right.id))
        })
        .map(|delivery| delivery.source_context.clone())
        .ok_or_else(invalid_input)?;
    let newest_source_occurred_at = members
        .iter()
        .map(|delivery| delivery.source_occurred_at)
        .max()
        .ok_or_else(invalid_input)?;
    let merge_key = serde_json::to_string(&merge_identity).map_err(|_| database_failure())?;
    Ok(ReminderAlertGroup {
        merge_key,
        merge_identity,
        members,
        source_context,
        newest_source_occurred_at,
    })
}

fn valid_merge_identity_shape(
    merge_identity: &ReminderMergeIdentity,
    expected_member_delivery_ids: &[String],
) -> bool {
    match merge_identity {
        ReminderMergeIdentity::Agent {
            rule_id, task_id, ..
        } => !rule_id.is_empty() && !task_id.is_empty(),
        ReminderMergeIdentity::Todo { delivery_id, .. }
        | ReminderMergeIdentity::Monitor { delivery_id, .. } => {
            expected_member_delivery_ids.len() == 1
                && expected_member_delivery_ids[0] == *delivery_id
        }
    }
}

fn delivery_merge_identity(delivery: &ReminderDelivery) -> Option<ReminderMergeIdentity> {
    match &delivery.source_context {
        ReminderSourceContext::Agent {
            agent_id,
            environment,
            task_id,
            trigger_status,
            ..
        } => Some(ReminderMergeIdentity::Agent {
            rule_id: delivery.rule_id.clone()?,
            agent_id: agent_id.clone(),
            environment: environment.clone(),
            task_id: task_id.clone(),
            trigger_status: trigger_status.clone(),
        }),
        ReminderSourceContext::Todo {
            todo_id,
            reminder_revision,
            ..
        } => Some(ReminderMergeIdentity::Todo {
            todo_id: todo_id.clone(),
            reminder_revision: *reminder_revision,
            delivery_id: delivery.id.clone(),
        }),
        ReminderSourceContext::Monitor {
            threshold_id,
            breach_started_at,
            ..
        } => Some(ReminderMergeIdentity::Monitor {
            threshold_id: threshold_id.clone(),
            breach_started_at: *breach_started_at,
            delivery_id: delivery.id.clone(),
        }),
    }
}

fn row_to_rule(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReminderRule> {
    let agent_ids_json: String = row.get(1)?;
    let trigger_statuses_json: String = row.get(2)?;
    let sound_json: String = row.get(5)?;
    Ok(ReminderRule {
        id: row.get(0)?,
        agent_ids: serde_json::from_str(&agent_ids_json)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        trigger_statuses: serde_json::from_str(&trigger_statuses_json)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        enabled: row.get(3)?,
        delay_seconds: row.get(4)?,
        sound: serde_json::from_str(&sound_json).map_err(|_| rusqlite::Error::InvalidQuery)?,
        toast_enabled: row.get(6)?,
        window_enabled: row.get(7)?,
        revision: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn row_to_delivery(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReminderDelivery> {
    let rule_id: Option<String> = row.get(2)?;
    let parameters: String = row.get(6)?;
    let context: String = row.get(7)?;
    let sound: String = row.get(9)?;
    Ok(ReminderDelivery {
        id: row.get(0)?,
        dedupe_key: row.get(1)?,
        rule_id,
        source_kind: parse_source_kind(&row.get::<_, String>(3)?)
            .ok_or(rusqlite::Error::InvalidQuery)?,
        source_entity_id: row.get(4)?,
        message_key: row.get(5)?,
        message_parameters: serde_json::from_str(&parameters)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        source_context: serde_json::from_str(&context)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        source_occurred_at: row.get(8)?,
        sound: serde_json::from_str(&sound).map_err(|_| rusqlite::Error::InvalidQuery)?,
        state: parse_delivery_state(&row.get::<_, String>(10)?)
            .ok_or(rusqlite::Error::InvalidQuery)?,
        due_at: row.get(11)?,
        dispatch_seq: row.get::<_, Option<i64>>(12)?.unwrap_or(0),
        first_dispatched_at: row.get(13)?,
        last_dispatched_at: row.get(14)?,
        acknowledged_at: row.get(15)?,
        completed_at: row.get(16)?,
        snoozed_until: row.get(17)?,
        created_at: row.get(18)?,
        updated_at: row.get(19)?,
    })
}

fn validate_rule_input(input: &SaveReminderRuleInput, now: i64) -> Result<(), CommandError> {
    if now < 0
        || !(0..=604800).contains(&input.delay_seconds)
        || !has_enabled_channel(&input.sound, input.toast_enabled, input.window_enabled)
        || !sorted_agent_ids(&input.agent_ids)
        || !sorted_trigger_statuses(&input.trigger_statuses)
    {
        return Err(invalid_input());
    }
    if let Some(id) = &input.id {
        Uuid::parse_str(id).map_err(|_| invalid_input())?;
    }
    Ok(())
}
fn sorted_agent_ids(values: &[crate::contracts::AgentId]) -> bool {
    (1..=4).contains(&values.len())
        && values
            .windows(2)
            .all(|pair| agent_id_name(&pair[0]) < agent_id_name(&pair[1]))
}
fn sorted_trigger_statuses(values: &[crate::contracts::AgentTriggerStatus]) -> bool {
    (1..=4).contains(&values.len())
        && values
            .windows(2)
            .all(|pair| trigger_status_name(&pair[0]) < trigger_status_name(&pair[1]))
}
fn validate_delivery(request: &NewReminderDelivery, now: i64) -> Result<(), CommandError> {
    if now < 0
        || request.dedupe_key.is_empty()
        || request.source_entity_id.is_empty()
        || request.due_at < 0
        || request.source_occurred_at < 0
        || !has_enabled_channel(
            &request.sound,
            request.toast_enabled,
            request.window_enabled,
        )
    {
        return Err(invalid_input());
    }
    MessageParameterContract::validate_for(
        MessageUsage::ReminderDisplay,
        &request.message_key,
        &request.message_parameters,
    )?;
    if !reminder_delivery_payload_is_valid(request) {
        return Err(invalid_input());
    }
    Ok(())
}
pub(crate) fn has_enabled_channel(
    sound: &ReminderSound,
    toast_enabled: bool,
    window_enabled: bool,
) -> bool {
    !matches!(sound, ReminderSound::None) || toast_enabled || window_enabled
}
pub(crate) fn canonical_sound(sound: &ReminderSound) -> Result<ReminderSound, CommandError> {
    match sound {
        ReminderSound::None | ReminderSound::Builtin { .. } => Ok(sound.clone()),
        ReminderSound::LocalFile { canonical_path } => {
            let path = fs::canonicalize(canonical_path).map_err(|_| invalid_input())?;
            let metadata = fs::metadata(&path).map_err(|_| invalid_input())?;
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .map(str::to_ascii_lowercase);
            if !metadata.is_file()
                || metadata.len() > 50 * 1024 * 1024
                || !matches!(extension.as_deref(), Some("wav" | "mp3" | "flac" | "ogg"))
            {
                return Err(invalid_input());
            }
            Ok(ReminderSound::LocalFile {
                canonical_path: path.to_string_lossy().into_owned(),
            })
        }
    }
}
fn source_kind(value: &ReminderSourceKind) -> &'static str {
    match value {
        ReminderSourceKind::Agent => "agent",
        ReminderSourceKind::Todo => "todo",
        ReminderSourceKind::Monitor => "monitor",
    }
}
fn agent_id_name(value: &crate::contracts::AgentId) -> &'static str {
    match value {
        crate::contracts::AgentId::Codex => "codex",
        crate::contracts::AgentId::Hermes => "hermes",
        crate::contracts::AgentId::Workbuddy => "workbuddy",
        crate::contracts::AgentId::Claude => "claude",
    }
}
fn trigger_status_name(value: &crate::contracts::AgentTriggerStatus) -> &'static str {
    match value {
        crate::contracts::AgentTriggerStatus::Completed => "completed",
        crate::contracts::AgentTriggerStatus::Failed => "failed",
        crate::contracts::AgentTriggerStatus::Waiting => "waiting",
        crate::contracts::AgentTriggerStatus::Timeout => "timeout",
    }
}
fn parse_source_kind(value: &str) -> Option<ReminderSourceKind> {
    Some(match value {
        "agent" => ReminderSourceKind::Agent,
        "todo" => ReminderSourceKind::Todo,
        "monitor" => ReminderSourceKind::Monitor,
        _ => return None,
    })
}
fn parse_delivery_state(value: &str) -> Option<ReminderDeliveryState> {
    Some(match value {
        "pending" => ReminderDeliveryState::Pending,
        "dispatched" => ReminderDeliveryState::Dispatched,
        "acknowledged" => ReminderDeliveryState::Acknowledged,
        "snoozed" => ReminderDeliveryState::Snoozed,
        "cancelled" => ReminderDeliveryState::Cancelled,
        "completed" => ReminderDeliveryState::Completed,
        _ => return None,
    })
}
fn sound_state(sound: &ReminderSound) -> &'static str {
    if matches!(sound, ReminderSound::None) {
        "skipped"
    } else {
        "pending"
    }
}
fn channel_state(enabled: bool) -> &'static str {
    if enabled {
        "pending"
    } else {
        "skipped"
    }
}
fn retain_terminal(transaction: &rusqlite::Transaction<'_>) -> Result<(), CommandError> {
    transaction.execute("DELETE FROM reminder_deliveries WHERE id IN (SELECT id FROM reminder_deliveries WHERE state IN ('acknowledged', 'cancelled', 'completed') ORDER BY updated_at DESC, id DESC LIMIT -1 OFFSET 5000)", [])?;
    Ok(())
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
        retryable: false,
    }
}
fn retryable_conflict() -> CommandError {
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
    use crate::contracts::{
        AgentEnvironment, AgentId, AgentTriggerStatus, BuiltinReminderSoundId, MonitorMetric,
        ReminderActionInput, ReminderActionMember, ReminderAlertGroup, ReminderMergeIdentity,
        ReminderSound, ReminderSourceContext, ReminderSourceKind, SafeMessageParameters,
        SafeParameterValue,
    };

    fn repository() -> ReminderRepository {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.keep();
        ReminderRepository::new(Arc::new(Storage::open(&path).unwrap()))
    }

    fn request(key: &str, rule_id: Option<Uuid>, due_at: i64) -> NewReminderDelivery {
        NewReminderDelivery {
            dedupe_key: key.into(),
            rule_id,
            source_kind: ReminderSourceKind::Agent,
            source_entity_id: "task-1".into(),
            message_key: "reminders.agent.status".into(),
            message_parameters: SafeMessageParameters::from([
                (
                    "agentName".into(),
                    SafeParameterValue::String("Codex".into()),
                ),
                (
                    "environment".into(),
                    SafeParameterValue::String("windows".into()),
                ),
                ("taskId".into(), SafeParameterValue::String("task-1".into())),
                (
                    "taskTitle".into(),
                    SafeParameterValue::String("Task 1".into()),
                ),
                (
                    "triggerStatus".into(),
                    SafeParameterValue::String("completed".into()),
                ),
            ]),
            source_context: ReminderSourceContext::Agent {
                agent_id: AgentId::Codex,
                environment: AgentEnvironment::Windows,
                task_id: "task-1".into(),
                task_title: Some("Task 1".into()),
                trigger_status: AgentTriggerStatus::Completed,
                source_event_id: "event-1".into(),
                source_occurred_at: 10,
            },
            source_occurred_at: 10,
            sound: ReminderSound::None,
            toast_enabled: true,
            window_enabled: false,
            due_at,
        }
    }

    fn todo_request(key: &str, todo_id: &str, revision: i64, due_at: i64) -> NewReminderDelivery {
        NewReminderDelivery {
            dedupe_key: key.into(),
            rule_id: None,
            source_kind: ReminderSourceKind::Todo,
            source_entity_id: todo_id.into(),
            message_key: "reminders.todo.due".into(),
            message_parameters: SafeMessageParameters::from([(
                "todoTitle".into(),
                SafeParameterValue::String("Task 8".into()),
            )]),
            source_context: ReminderSourceContext::Todo {
                todo_id: todo_id.into(),
                reminder_revision: revision,
                todo_title: "Task 8".into(),
                source_occurred_at: 10,
            },
            source_occurred_at: 10,
            sound: ReminderSound::None,
            toast_enabled: true,
            window_enabled: false,
            due_at,
        }
    }

    fn monitor_request(
        key: &str,
        threshold_id: &str,
        breach_started_at: i64,
        due_at: i64,
    ) -> NewReminderDelivery {
        NewReminderDelivery {
            dedupe_key: key.into(),
            rule_id: None,
            source_kind: ReminderSourceKind::Monitor,
            source_entity_id: threshold_id.into(),
            message_key: "reminders.monitor.threshold".into(),
            message_parameters: SafeMessageParameters::from([
                ("metric".into(), SafeParameterValue::String("cpu".into())),
                ("currentValue".into(), SafeParameterValue::Number(95.into())),
                (
                    "thresholdValue".into(),
                    SafeParameterValue::Number(90.into()),
                ),
            ]),
            source_context: ReminderSourceContext::Monitor {
                threshold_id: threshold_id.into(),
                metric: MonitorMetric::CpuPercent,
                current_value: 95,
                threshold_value: 90,
                breach_started_at,
                source_occurred_at: 10,
            },
            source_occurred_at: 10,
            sound: ReminderSound::None,
            toast_enabled: true,
            window_enabled: false,
            due_at,
        }
    }

    fn inserted(outcome: EnqueueOutcome) -> ReminderDelivery {
        match outcome {
            EnqueueOutcome::Inserted(delivery) => delivery,
            EnqueueOutcome::Duplicate(_) => panic!("delivery unexpectedly deduplicated"),
        }
    }

    fn rule_input(
        id: Option<String>,
        enabled: bool,
        expected_revision: Option<i64>,
    ) -> SaveReminderRuleInput {
        SaveReminderRuleInput {
            id,
            agent_ids: vec![AgentId::Codex],
            trigger_statuses: vec![AgentTriggerStatus::Completed],
            enabled,
            delay_seconds: 0,
            sound: ReminderSound::None,
            toast_enabled: true,
            window_enabled: false,
            expected_revision,
        }
    }

    #[test]
    fn duplicate_dedupe_key_returns_the_original_delivery_without_a_second_row() {
        let repository = repository();
        let original = inserted(
            repository
                .enqueue(request("agent:1", None, 20), 11)
                .unwrap(),
        );
        let duplicate = repository
            .enqueue(request("agent:1", None, 99), 12)
            .unwrap();
        assert_eq!(duplicate, EnqueueOutcome::Duplicate(original.clone()));
        let count = repository
            .storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM reminder_deliveries WHERE dedupe_key = 'agent:1'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn builtin_sound_is_a_complete_rule_and_delivery_channel_without_toast_or_window() {
        let repository = repository();
        let sound = ReminderSound::Builtin {
            sound_id: BuiltinReminderSoundId::SystemNotification,
        };
        let mut input = rule_input(None, true, None);
        input.sound = sound.clone();
        input.toast_enabled = false;
        input.window_enabled = false;
        let rule = repository.save_rule(input, 10).unwrap();
        assert_eq!(rule.sound, sound);

        let mut delivery = request(
            "agent:builtin-sound-only",
            Some(Uuid::parse_str(&rule.id).unwrap()),
            20,
        );
        delivery.sound = sound.clone();
        delivery.toast_enabled = false;
        delivery.window_enabled = false;
        assert_eq!(
            inserted(repository.enqueue(delivery, 11).unwrap()).sound,
            sound
        );
    }

    #[test]
    fn local_file_sound_is_a_complete_rule_and_delivery_channel_without_toast_or_window() {
        let repository = repository();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("alert.wav");
        std::fs::write(&path, b"RIFF-test").unwrap();
        let canonical_path = std::fs::canonicalize(&path)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let sound = ReminderSound::LocalFile {
            canonical_path: path.to_string_lossy().into_owned(),
        };
        let expected_sound = ReminderSound::LocalFile {
            canonical_path: canonical_path.clone(),
        };
        let mut input = rule_input(None, true, None);
        input.sound = sound.clone();
        input.toast_enabled = false;
        input.window_enabled = false;
        let rule = repository.save_rule(input, 10).unwrap();
        assert_eq!(rule.sound, expected_sound);

        let mut delivery = request(
            "agent:local-sound-only",
            Some(Uuid::parse_str(&rule.id).unwrap()),
            20,
        );
        delivery.sound = sound;
        delivery.toast_enabled = false;
        delivery.window_enabled = false;
        assert_eq!(
            inserted(repository.enqueue(delivery, 11).unwrap()).sound,
            ReminderSound::LocalFile { canonical_path }
        );
    }

    #[test]
    fn due_claims_allocate_one_monotonic_sequence_in_due_created_id_order() {
        let repository = repository();
        for key in ["agent:c", "agent:a", "agent:b"] {
            inserted(repository.enqueue(request(key, None, 20), 11).unwrap());
        }
        repository
            .storage
            .with_transaction(|transaction| {
                for (dedupe_key, id) in [
                    ("agent:c", "delivery-c"),
                    ("agent:a", "delivery-a"),
                    ("agent:b", "delivery-b"),
                ] {
                    transaction.execute(
                        "UPDATE reminder_deliveries SET id = ?2 WHERE dedupe_key = ?1",
                        rusqlite::params![dedupe_key, id],
                    )?;
                }
                Ok(())
            })
            .unwrap();
        let claimed = repository.claim_due(20, 100).unwrap();
        let rows = repository
            .storage
            .with_connection(|connection| {
                let mut statement = connection.prepare(
                    "SELECT due_at, created_at, id, dispatch_seq FROM reminder_deliveries ORDER BY due_at, created_at, id",
                )?;
                let rows = statement
                    .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(3)?)))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(CommandError::from)?;
                Ok(rows)
            })
            .unwrap();
        assert_eq!(
            rows,
            vec![
                (20, 11, "delivery-a".into(), 1),
                (20, 11, "delivery-b".into(), 2),
                (20, 11, "delivery-c".into(), 3),
            ]
        );
        assert_eq!(
            claimed
                .iter()
                .map(|delivery| (delivery.id.as_str(), delivery.dispatch_seq))
                .collect::<Vec<_>>(),
            vec![("delivery-a", 1), ("delivery-b", 2), ("delivery-c", 3)]
        );
    }

    #[test]
    fn replay_cursor_never_moves_backward() {
        let repository = repository();
        assert_eq!(
            repository
                .commit_cursor("alert", 9, 100)
                .unwrap()
                .last_dispatch_seq,
            9
        );
        assert_eq!(
            repository
                .commit_cursor("alert", 3, 101)
                .unwrap()
                .last_dispatch_seq,
            9
        );
        assert_eq!(
            repository
                .commit_cursor("alert", 11, 102)
                .unwrap()
                .last_dispatch_seq,
            11
        );
    }

    // Break caught: a snoozed delivery is still schedulable and must become due again without
    // allocating a new delivery id.
    #[test]
    fn claim_due_claims_snoozed_rows_at_their_updated_due_time() {
        let repository = repository();
        let delivery = inserted(
            repository
                .enqueue(request("agent:snoozed-due", None, 20), 11)
                .unwrap(),
        );
        repository.claim_due(20, 10).unwrap();
        repository
            .storage
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE reminder_deliveries SET state = 'snoozed', due_at = 40, dispatch_seq = NULL WHERE id = ?1",
                    [&delivery.id],
                )?;
                Ok(())
            })
            .unwrap();

        assert!(repository.claim_due(39, 10).unwrap().is_empty());
        let redispatched = repository.claim_due(40, 10).unwrap();

        assert_eq!(redispatched.len(), 1);
        assert_eq!(redispatched[0].id, delivery.id);
        assert_eq!(redispatched[0].state, ReminderDeliveryState::Dispatched);
        assert!(redispatched[0].dispatch_seq > 1);
    }

    // Break caught: callers that restart with zero must not replay deliveries already committed
    // by the durable consumer cursor.
    #[test]
    fn replay_uses_the_effective_persisted_consumer_cursor() {
        let repository = repository();
        inserted(
            repository
                .enqueue(request("agent:cursor-one", None, 20), 11)
                .unwrap(),
        );
        inserted(
            repository
                .enqueue(request("agent:cursor-two", None, 21), 11)
                .unwrap(),
        );
        let claimed = repository.claim_due(21, 10).unwrap();
        repository
            .commit_cursor("toast", claimed[0].dispatch_seq as u64, 22)
            .unwrap();

        let replay = repository.replay("toast", 0, 10).unwrap();

        assert_eq!(replay.deliveries.len(), 1);
        assert_eq!(replay.deliveries[0].id, claimed[1].id);
        assert_eq!(replay.last_dispatch_seq, claimed[1].dispatch_seq);
        assert!(!replay.has_more);
    }

    // Break caught: delivery is at-least-once until a cursor commit; after that durable commit a
    // restart with caller cursor zero resumes after the persisted page.
    #[test]
    fn replay_repeats_an_uncommitted_page_then_resumes_from_the_persisted_cursor() {
        let repository = repository();
        for key in ["agent:crash-one", "agent:crash-two", "agent:crash-three"] {
            inserted(repository.enqueue(request(key, None, 20), 11).unwrap());
        }
        let claimed = repository.claim_due(20, 10).unwrap();
        let first = repository.replay("toast", 0, 2).unwrap();
        let after_crash = repository.replay("toast", 0, 2).unwrap();
        assert_eq!(first.deliveries, after_crash.deliveries);
        assert!(first.has_more);
        repository
            .commit_cursor("toast", first.last_dispatch_seq as u64, 21)
            .unwrap();
        let resumed = repository.replay("toast", 0, 2).unwrap();
        assert_eq!(
            resumed
                .deliveries
                .iter()
                .map(|delivery| delivery.id.as_str())
                .collect::<Vec<_>>(),
            vec![claimed[2].id.as_str()]
        );
        assert!(!resumed.has_more);
    }

    // Break caught: a stale exact-state action must be retryable and may not partially update a
    // delivery which has already moved beyond the caller's observed state.
    #[test]
    fn stale_acknowledge_expected_state_returns_retryable_conflict_without_writes() {
        let repository = repository();
        let delivery = inserted(
            repository
                .enqueue(request("agent:stale-action", None, 20), 11)
                .unwrap(),
        );
        let dispatched = repository.claim_due(20, 10).unwrap().pop().unwrap();
        let input = ReminderActionInput {
            merge_identity: ReminderMergeIdentity::Todo {
                todo_id: "todo".into(),
                reminder_revision: 1,
                delivery_id: delivery.id.clone(),
            },
            expected_member_delivery_ids: vec![delivery.id.clone()],
            members: vec![ReminderActionMember {
                id: delivery.id.clone(),
                expected_state: ReminderDeliveryState::Pending,
            }],
        };

        let error = repository.acknowledge(input, 30).unwrap_err();

        assert_eq!(error.message_key, "errors.conflict");
        assert!(error.retryable);
        assert_eq!(
            repository.replay("stale-action", 0, 10).unwrap().deliveries,
            vec![dispatched]
        );
    }

    // Break caught: only an already surfaced dispatched/acknowledged delivery may be snoozed;
    // a matching stale/live state must not turn pending, cancelled, or completed history into a
    // new scheduled delivery.
    #[test]
    fn snooze_rejects_non_actionable_expected_states_without_writes() {
        let repository = repository();
        for (index, state) in [
            ReminderDeliveryState::Pending,
            ReminderDeliveryState::Cancelled,
            ReminderDeliveryState::Completed,
        ]
        .into_iter()
        .enumerate()
        {
            let delivery = inserted(
                repository
                    .enqueue(
                        request(&format!("agent:invalid-snooze-{index}"), None, 50),
                        11,
                    )
                    .unwrap(),
            );
            repository
                .storage
                .with_connection(|connection| {
                    let state_name = match state {
                        ReminderDeliveryState::Pending => "pending",
                        ReminderDeliveryState::Cancelled => "cancelled",
                        ReminderDeliveryState::Completed => "completed",
                        _ => unreachable!(),
                    };
                    connection.execute(
                        "UPDATE reminder_deliveries SET state = ?2 WHERE id = ?1",
                        rusqlite::params![delivery.id, state_name],
                    )?;
                    Ok(())
                })
                .unwrap();
            let error = repository
                .snooze(
                    SnoozeReminderInput {
                        merge_identity: ReminderMergeIdentity::Todo {
                            todo_id: "todo".into(),
                            reminder_revision: 1,
                            delivery_id: delivery.id.clone(),
                        },
                        expected_member_delivery_ids: vec![delivery.id.clone()],
                        members: vec![ReminderActionMember {
                            id: delivery.id.clone(),
                            expected_state: state.clone(),
                        }],
                        snoozed_until: 80,
                    },
                    20,
                )
                .unwrap_err();
            assert_eq!(error.message_key, "errors.invalidInput");
            let stored_state = repository
                .storage
                .with_connection(|connection| {
                    connection
                        .query_row(
                            "SELECT state FROM reminder_deliveries WHERE id = ?1",
                            [&delivery.id],
                            |row| row.get::<_, String>(0),
                        )
                        .map_err(CommandError::from)
                })
                .unwrap();
            assert_eq!(
                stored_state,
                match state {
                    ReminderDeliveryState::Pending => "pending",
                    ReminderDeliveryState::Cancelled => "cancelled",
                    ReminderDeliveryState::Completed => "completed",
                    _ => unreachable!(),
                }
            );
        }
    }

    // Break caught: stale complete and snooze actions must verify every expected state before
    // their transaction writes any row.
    #[test]
    fn stale_complete_and_snooze_return_retryable_conflict_without_writes() {
        let repository = repository();
        for action in ["complete", "snooze"] {
            let delivery = inserted(
                repository
                    .enqueue(request(&format!("agent:stale-{action}"), None, 50), 11)
                    .unwrap(),
            );
            let member = ReminderActionMember {
                id: delivery.id.clone(),
                expected_state: ReminderDeliveryState::Dispatched,
            };
            let result = match action {
                "complete" => repository.complete(
                    ReminderActionInput {
                        merge_identity: ReminderMergeIdentity::Todo {
                            todo_id: "todo".into(),
                            reminder_revision: 1,
                            delivery_id: delivery.id.clone(),
                        },
                        expected_member_delivery_ids: vec![delivery.id.clone()],
                        members: vec![member],
                    },
                    20,
                ),
                "snooze" => repository.snooze(
                    SnoozeReminderInput {
                        merge_identity: ReminderMergeIdentity::Todo {
                            todo_id: "todo".into(),
                            reminder_revision: 1,
                            delivery_id: delivery.id.clone(),
                        },
                        expected_member_delivery_ids: vec![delivery.id.clone()],
                        members: vec![member],
                        snoozed_until: 80,
                    },
                    20,
                ),
                _ => unreachable!(),
            };
            let error = result.unwrap_err();
            assert_eq!(error.message_key, "errors.conflict");
            assert!(error.retryable);
            let row = repository.storage.with_connection(|connection| {
                connection.query_row("SELECT state, dispatch_seq, due_at FROM reminder_deliveries WHERE id = ?1", [&delivery.id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?, row.get::<_, i64>(2)?))).map_err(CommandError::from)
            }).unwrap();
            assert_eq!(row, ("pending".into(), None, 50));
        }
    }

    // Break caught: snooze must preserve identity and first dispatch while clearing the old
    // sequence/channel attempts, then redispatch the same row with a greater sequence.
    #[test]
    fn snooze_resets_channel_attempts_and_redispatches_same_delivery_with_new_sequence() {
        let repository = repository();
        let rule = repository
            .save_rule(rule_input(None, true, None), 10)
            .unwrap();
        let rule_id = Uuid::parse_str(&rule.id).unwrap();
        let delivery = inserted(
            repository
                .enqueue(request("agent:snooze-retry", Some(rule_id), 20), 11)
                .unwrap(),
        );
        let first = repository.claim_due(20, 10).unwrap().pop().unwrap();
        repository
            .storage
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE reminder_deliveries SET sound_state = 'failed', sound_error_code = 'playFailed', toast_state = 'succeeded', window_state = 'failed', window_error_code = 'showFailed' WHERE id = ?1",
                    [&delivery.id],
                )?;
                Ok(())
            })
            .unwrap();
        repository
            .snooze(
                SnoozeReminderInput {
                    merge_identity: ReminderMergeIdentity::Agent {
                        rule_id: rule.id,
                        agent_id: AgentId::Codex,
                        environment: AgentEnvironment::Windows,
                        task_id: "task-1".into(),
                        trigger_status: AgentTriggerStatus::Completed,
                    },
                    expected_member_delivery_ids: vec![delivery.id.clone()],
                    members: vec![ReminderActionMember {
                        id: delivery.id.clone(),
                        expected_state: ReminderDeliveryState::Dispatched,
                    }],
                    snoozed_until: 40,
                },
                21,
            )
            .unwrap();
        let snoozed = repository
            .storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT state, dispatch_seq, sound_state, toast_state, window_state, first_dispatched_at, last_dispatched_at FROM reminder_deliveries WHERE id = ?1",
                        [&delivery.id],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?, row.get::<_, Option<i64>>(5)?, row.get::<_, Option<i64>>(6)?)),
                    )
                    .map_err(CommandError::from)
            })
            .unwrap();
        assert_eq!(
            snoozed,
            (
                "snoozed".into(),
                None,
                "skipped".into(),
                "pending".into(),
                "skipped".into(),
                first.first_dispatched_at,
                first.last_dispatched_at
            )
        );
        assert!(repository.claim_due(39, 10).unwrap().is_empty());
        let second = repository.claim_due(40, 10).unwrap().pop().unwrap();
        assert_eq!(second.id, delivery.id);
        assert!(second.dispatch_seq > first.dispatch_seq);
        assert_eq!(second.first_dispatched_at, first.first_dispatched_at);
        assert_eq!(second.last_dispatched_at, Some(40));
    }

    // Coverage hardening: deleting one rule only cancels its still-pending delivery and retains
    // that rule's terminal history while another rule's pending row is untouched.
    #[test]
    fn deleting_one_rule_cancels_only_its_pending_delivery_and_retains_terminal_history() {
        let repository = repository();
        let first_rule = repository
            .save_rule(rule_input(None, true, None), 10)
            .unwrap();
        let second_rule = repository
            .save_rule(rule_input(None, true, None), 10)
            .unwrap();
        let first_id = Uuid::parse_str(&first_rule.id).unwrap();
        let second_id = Uuid::parse_str(&second_rule.id).unwrap();
        let pending_first = inserted(
            repository
                .enqueue(
                    request("agent:delete-first-pending", Some(first_id), 50),
                    11,
                )
                .unwrap(),
        );
        let pending_second = inserted(
            repository
                .enqueue(
                    request("agent:delete-second-pending", Some(second_id), 50),
                    11,
                )
                .unwrap(),
        );
        let acknowledged = inserted(
            repository
                .enqueue(request("agent:delete-first-ack", Some(first_id), 20), 11)
                .unwrap(),
        );
        let completed = inserted(
            repository
                .enqueue(
                    request("agent:delete-first-complete", Some(first_id), 20),
                    11,
                )
                .unwrap(),
        );
        repository.claim_due(20, 10).unwrap();
        repository.storage.with_connection(|connection| {
            connection.execute("UPDATE reminder_deliveries SET state = 'acknowledged', acknowledged_at = 21 WHERE id = ?1", [&acknowledged.id])?;
            connection.execute("UPDATE reminder_deliveries SET state = 'completed', completed_at = 21 WHERE id = ?1", [&completed.id])?;
            Ok(())
        }).unwrap();
        repository
            .delete_rule(first_id, first_rule.revision as u64, 30)
            .unwrap();
        let rows = repository
            .storage
            .with_connection(|connection| {
                let mut statement =
                    connection.prepare("SELECT id, state FROM reminder_deliveries ORDER BY id")?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(CommandError::from)?;
                Ok(rows)
            })
            .unwrap();
        assert!(rows.contains(&(pending_first.id, "cancelled".into())));
        assert!(rows.contains(&(pending_second.id, "pending".into())));
        assert!(rows.contains(&(acknowledged.id, "acknowledged".into())));
        assert!(rows.contains(&(completed.id, "completed".into())));
    }

    #[test]
    fn stale_rule_revision_returns_retryable_conflict_without_writing() {
        let repository = repository();
        let original = repository
            .save_rule(rule_input(None, true, None), 10)
            .unwrap();
        let mut stale = rule_input(Some(original.id.clone()), true, Some(original.revision + 1));
        stale.delay_seconds = 99;

        let error = repository.save_rule(stale, 20).unwrap_err();

        assert_eq!(error.code, crate::contracts::AppErrorCode::Conflict);
        assert_eq!(error.message_key, "errors.conflict");
        assert!(error.retryable);
        assert_eq!(repository.list_rules().unwrap(), vec![original]);
    }

    #[test]
    fn rule_create_race_returns_retryable_conflict_without_writing() {
        let repository = repository();
        let id = Uuid::new_v4().to_string();
        let winner = repository
            .save_rule(rule_input(Some(id.clone()), true, None), 10)
            .unwrap();
        let mut loser = rule_input(Some(id), true, None);
        loser.delay_seconds = 99;

        let error = repository.save_rule(loser, 20).unwrap_err();

        assert_eq!(error.code, crate::contracts::AppErrorCode::Conflict);
        assert_eq!(error.message_key, "errors.conflict");
        assert!(error.retryable);
        assert_eq!(repository.list_rules().unwrap(), vec![winner]);
    }

    #[test]
    fn stale_rule_delete_returns_retryable_conflict_without_writing() {
        let repository = repository();
        let rule = repository
            .save_rule(rule_input(None, true, None), 10)
            .unwrap();
        let rule_id = Uuid::parse_str(&rule.id).unwrap();
        let pending = inserted(
            repository
                .enqueue(request("agent:stale-delete", Some(rule_id), 50), 11)
                .unwrap(),
        );

        let error = repository
            .delete_rule(rule_id, u64::try_from(rule.revision + 1).unwrap(), 20)
            .unwrap_err();

        assert_eq!(error.code, crate::contracts::AppErrorCode::Conflict);
        assert_eq!(error.message_key, "errors.conflict");
        assert!(error.retryable);
        assert_eq!(repository.list_rules().unwrap(), vec![rule]);
        let stored_delivery = repository
            .storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT id, dedupe_key, rule_id, source_kind, source_entity_id, message_key, message_parameters_json, source_context_json, source_occurred_at, sound_json, state, due_at, dispatch_seq, first_dispatched_at, last_dispatched_at, acknowledged_at, completed_at, snoozed_until, created_at, updated_at FROM reminder_deliveries WHERE id = ?1",
                        [&pending.id],
                        row_to_delivery,
                    )
                    .map_err(CommandError::from)
            })
            .unwrap();
        assert_eq!(stored_delivery, pending);
    }

    #[test]
    fn disabling_a_rule_cancels_only_its_pending_deliveries_and_retains_dispatched_history() {
        let repository = repository();
        let rule = repository
            .save_rule(rule_input(None, true, None), 10)
            .unwrap();
        let rule_id = Uuid::parse_str(&rule.id).unwrap();
        let pending = inserted(
            repository
                .enqueue(request("agent:pending", Some(rule_id), 50), 11)
                .unwrap(),
        );
        let dispatched = inserted(
            repository
                .enqueue(request("agent:dispatched", Some(rule_id), 12), 11)
                .unwrap(),
        );
        repository.claim_due(12, 10).unwrap();
        let disabled = repository
            .save_rule(
                rule_input(Some(rule.id.clone()), false, Some(rule.revision)),
                20,
            )
            .unwrap();
        assert!(!disabled.enabled);
        let states = repository
            .storage
            .with_connection(|connection| {
                let mut statement =
                    connection.prepare("SELECT id, state FROM reminder_deliveries ORDER BY id")?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(CommandError::from)?;
                Ok(rows)
            })
            .unwrap();
        assert!(states.contains(&(pending.id, "cancelled".into())));
        assert!(states.contains(&(dispatched.id, "dispatched".into())));
    }

    #[test]
    fn retention_preserves_live_rows_and_only_the_newest_five_thousand_terminal_rows() {
        let repository = repository();
        let rule = repository
            .save_rule(rule_input(None, true, None), 10)
            .unwrap();
        let rule_id = Uuid::parse_str(&rule.id).unwrap();
        repository.storage.with_transaction(|transaction| {
            for number in 0..5001 {
                transaction.execute(
                    r#"INSERT INTO reminder_deliveries(
                        id, dedupe_key, rule_id, source_kind, source_entity_id, message_key,
                        message_parameters_json, source_context_json, source_occurred_at, sound_json,
                        toast_enabled, window_enabled, state, due_at, sound_state, toast_state,
                        window_state, created_at, updated_at
                    ) VALUES (?1, ?2, ?3, 'agent', 'task', 'reminders.agent.status', '{}',
                        '{"kind":"agent","agentId":"codex","environment":"windows","taskId":"task","taskTitle":null,"triggerStatus":"completed","sourceEventId":"event","sourceOccurredAt":10}',
                        10, '{"kind":"none"}', 1, 0, 'completed', 10, 'skipped', 'pending',
                        'skipped', ?4, ?4)"#,
                    rusqlite::params![format!("terminal-{number:04}"), format!("terminal-key-{number:04}"), rule.id, number],
                )?;
            }
            Ok(())
        }).unwrap();
        let pending = inserted(
            repository
                .enqueue(request("agent:pending-retention", Some(rule_id), 50), 20)
                .unwrap(),
        );
        let snoozed = inserted(
            repository
                .enqueue(request("agent:snoozed-retention", Some(rule_id), 60), 20)
                .unwrap(),
        );
        let dispatched = inserted(
            repository
                .enqueue(request("agent:dispatched-retention", Some(rule_id), 20), 20)
                .unwrap(),
        );
        repository.claim_due(20, 10).unwrap();
        repository
            .storage
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE reminder_deliveries SET state = 'snoozed' WHERE id = ?1",
                    [&snoozed.id],
                )?;
                Ok(())
            })
            .unwrap();
        repository
            .save_rule(rule_input(Some(rule.id), false, Some(rule.revision)), 30)
            .unwrap();
        let live_rows = repository.storage.with_connection(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM reminder_deliveries WHERE state IN ('pending', 'snoozed', 'dispatched')",
                [],
                |row| row.get::<_, i64>(0),
            ).map_err(Into::into)
        }).unwrap();
        let terminal_rows = repository.storage.with_connection(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM reminder_deliveries WHERE state IN ('acknowledged', 'cancelled', 'completed')",
                [],
                |row| row.get::<_, i64>(0),
            ).map_err(Into::into)
        }).unwrap();
        assert_eq!(live_rows, 2, "snoozed and dispatched rows stay live");
        assert_eq!(terminal_rows, 5000);
        for (id, expected_state) in [
            (&pending.id, "cancelled"),
            (&snoozed.id, "snoozed"),
            (&dispatched.id, "dispatched"),
        ] {
            let state = repository
                .storage
                .with_connection(|connection| {
                    connection
                        .query_row(
                            "SELECT state FROM reminder_deliveries WHERE id = ?1",
                            [id],
                            |row| row.get::<_, String>(0),
                        )
                        .map_err(Into::into)
                })
                .unwrap();
            assert_eq!(state, expected_state);
        }
        let retained_bounds = repository
            .storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM reminder_deliveries WHERE id = 'terminal-0000'), EXISTS(SELECT 1 FROM reminder_deliveries WHERE id = 'terminal-5000')",
                        [],
                        |row| Ok((row.get::<_, bool>(0)?, row.get::<_, bool>(1)?)),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(retained_bounds, (false, true));
    }

    #[test]
    fn mismatched_todo_identity_conflicts_without_acknowledging_agent_delivery() {
        let repository = repository();
        let delivery = inserted(
            repository
                .enqueue(request("agent:identity-mismatch", None, 10), 10)
                .unwrap(),
        );
        let dispatched = repository.claim_due(10, 1).unwrap().pop().unwrap();
        let error = repository
            .acknowledge(
                ReminderActionInput {
                    merge_identity: ReminderMergeIdentity::Todo {
                        todo_id: "wrong-todo".into(),
                        reminder_revision: 1,
                        delivery_id: dispatched.id.clone(),
                    },
                    expected_member_delivery_ids: vec![dispatched.id.clone()],
                    members: vec![ReminderActionMember {
                        id: dispatched.id.clone(),
                        expected_state: ReminderDeliveryState::Dispatched,
                    }],
                },
                20,
            )
            .expect_err("identity mismatch must conflict before writes");
        assert_eq!(error.code, AppErrorCode::Conflict);
        let state = repository
            .storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT state FROM reminder_deliveries WHERE id = ?1",
                        [&delivery.id],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(state, "dispatched");
    }

    // Break caught: an Agent action rendered before another matching delivery is dispatched must
    // not acknowledge the stale subset; retrying with the refreshed full group is atomic.
    #[test]
    fn agent_action_rejects_a_concurrently_inserted_matching_delivery_then_accepts_the_full_group()
    {
        let repository = repository();
        let rule = repository
            .save_rule(rule_input(None, true, None), 9)
            .unwrap();
        let rule_id = Uuid::parse_str(&rule.id).unwrap();
        inserted(
            repository
                .enqueue(request("agent:group-first", Some(rule_id), 10), 10)
                .unwrap(),
        );
        let first = repository.claim_due(10, 10).unwrap().pop().unwrap();
        let identity = ReminderMergeIdentity::Agent {
            rule_id: rule_id.to_string(),
            agent_id: AgentId::Codex,
            environment: AgentEnvironment::Windows,
            task_id: "task-1".into(),
            trigger_status: AgentTriggerStatus::Completed,
        };
        let stale_input = ReminderActionInput {
            merge_identity: identity.clone(),
            expected_member_delivery_ids: vec![first.id.clone()],
            members: vec![ReminderActionMember {
                id: first.id.clone(),
                expected_state: ReminderDeliveryState::Dispatched,
            }],
        };

        let second = inserted(
            repository
                .enqueue(request("agent:group-second", Some(rule_id), 10), 11)
                .unwrap(),
        );
        let second = repository
            .claim_due(10, 10)
            .unwrap()
            .into_iter()
            .find(|delivery| delivery.id == second.id)
            .unwrap();

        let error = repository.acknowledge(stale_input, 20).unwrap_err();
        assert_eq!(error.code, AppErrorCode::Conflict);
        assert!(error.retryable);
        let states = repository
            .storage
            .with_connection(|connection| {
                let mut statement = connection.prepare(
                    "SELECT id, state FROM reminder_deliveries WHERE id IN (?1, ?2) ORDER BY id",
                )?;
                let rows = statement
                    .query_map(rusqlite::params![first.id, second.id], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(CommandError::from)?;
                Ok(rows)
            })
            .unwrap();
        assert!(states.iter().all(|(_, state)| state == "dispatched"));

        let mut ids = vec![first.id.clone(), second.id.clone()];
        ids.sort();
        repository
            .acknowledge(
                ReminderActionInput {
                    merge_identity: identity,
                    expected_member_delivery_ids: ids.clone(),
                    members: ids
                        .iter()
                        .map(|id| ReminderActionMember {
                            id: id.clone(),
                            expected_state: ReminderDeliveryState::Dispatched,
                        })
                        .collect(),
                },
                21,
            )
            .unwrap();
        let acknowledged = repository.storage.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM reminder_deliveries WHERE id IN (?1, ?2) AND state = 'acknowledged'",
                    rusqlite::params![first.id, second.id],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(Into::into)
        }).unwrap();
        assert_eq!(acknowledged, 2);
    }

    // Mutation RED: returning unit, an unsorted snapshot, or a group loaded after commit would
    // let a caller render a partial/stale action result.  The action must return the rows it
    // reloaded inside its successful transaction.
    #[test]
    fn agent_acknowledgement_returns_the_complete_sorted_reloaded_alert_group() {
        let repository = repository();
        let rule = repository
            .save_rule(rule_input(None, true, None), 9)
            .unwrap();
        let rule_id = Uuid::parse_str(&rule.id).unwrap();
        let first = inserted(
            repository
                .enqueue(request("agent:group-return-first", Some(rule_id), 10), 10)
                .unwrap(),
        );
        let second = inserted(
            repository
                .enqueue(request("agent:group-return-second", Some(rule_id), 10), 10)
                .unwrap(),
        );
        let dispatched = repository.claim_due(10, 10).unwrap();
        let mut ids = dispatched
            .iter()
            .map(|delivery| delivery.id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        let group: ReminderAlertGroup = repository
            .acknowledge(
                ReminderActionInput {
                    merge_identity: ReminderMergeIdentity::Agent {
                        rule_id: rule_id.to_string(),
                        agent_id: AgentId::Codex,
                        environment: AgentEnvironment::Windows,
                        task_id: "task-1".into(),
                        trigger_status: AgentTriggerStatus::Completed,
                    },
                    expected_member_delivery_ids: ids.clone(),
                    members: ids
                        .iter()
                        .map(|id| ReminderActionMember {
                            id: id.clone(),
                            expected_state: ReminderDeliveryState::Dispatched,
                        })
                        .collect(),
                },
                20,
            )
            .unwrap();

        assert_eq!(
            group
                .members
                .iter()
                .map(|member| &member.id)
                .collect::<Vec<_>>(),
            ids.iter().collect::<Vec<_>>()
        );
        assert!(group
            .members
            .iter()
            .all(|member| member.state == ReminderDeliveryState::Acknowledged));
        assert_eq!(group.members.len(), 2);
        assert!(group.members.iter().any(|member| member.id == first.id));
        assert!(group.members.iter().any(|member| member.id == second.id));
    }

    // Break caught: clients act on an emitted group snapshot.  Once the real Todo delivery has
    // changed state, that old snapshot must conflict without a second write.
    #[test]
    fn todo_old_group_snapshot_conflicts_after_real_delivery_state_change_without_writes() {
        let repository = repository();
        inserted(
            repository
                .enqueue(todo_request("todo:revision-two", "todo-1", 2, 10), 10)
                .unwrap(),
        );
        let delivery = repository.claim_due(10, 1).unwrap().pop().unwrap();
        let snapshot = repository
            .alert_group_for_delivery(&delivery.id, delivery.dispatch_seq)
            .unwrap()
            .expect("dispatched Todo must produce its real alert group snapshot");
        let snapshot_action = ReminderActionInput {
            merge_identity: snapshot.merge_identity.clone(),
            expected_member_delivery_ids: snapshot
                .members
                .iter()
                .map(|member| member.id.clone())
                .collect(),
            members: snapshot
                .members
                .iter()
                .map(|member| ReminderActionMember {
                    id: member.id.clone(),
                    expected_state: member.state.clone(),
                })
                .collect(),
        };
        repository.acknowledge(snapshot_action.clone(), 20).unwrap();
        let after_first_action = repository.storage.with_connection(|connection| {
            connection.query_row(
                "SELECT state, acknowledged_at, completed_at, updated_at FROM reminder_deliveries WHERE id = ?1",
                [&delivery.id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?, row.get::<_, Option<i64>>(2)?, row.get::<_, i64>(3)?)),
            ).map_err(Into::into)
        }).unwrap();
        let error = repository.complete(snapshot_action, 21).unwrap_err();
        assert_eq!(error.code, AppErrorCode::Conflict);
        assert!(error.retryable);
        let after_stale_action = repository.storage.with_connection(|connection| {
            connection.query_row(
                "SELECT state, acknowledged_at, completed_at, updated_at FROM reminder_deliveries WHERE id = ?1",
                [&delivery.id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?, row.get::<_, Option<i64>>(2)?, row.get::<_, i64>(3)?)),
            ).map_err(Into::into)
        }).unwrap();
        assert_eq!(after_stale_action, after_first_action);
    }

    // Break caught: the same stale-snapshot rule applies to a Monitor breach group.
    #[test]
    fn monitor_old_group_snapshot_conflicts_after_real_delivery_state_change_without_writes() {
        let repository = repository();
        inserted(
            repository
                .enqueue(
                    monitor_request("monitor:breach-eleven", "threshold-1", 11, 10),
                    10,
                )
                .unwrap(),
        );
        let delivery = repository.claim_due(10, 1).unwrap().pop().unwrap();
        let snapshot = repository
            .alert_group_for_delivery(&delivery.id, delivery.dispatch_seq)
            .unwrap()
            .expect("dispatched Monitor must produce its real alert group snapshot");
        let snapshot_action = ReminderActionInput {
            merge_identity: snapshot.merge_identity.clone(),
            expected_member_delivery_ids: snapshot
                .members
                .iter()
                .map(|member| member.id.clone())
                .collect(),
            members: snapshot
                .members
                .iter()
                .map(|member| ReminderActionMember {
                    id: member.id.clone(),
                    expected_state: member.state.clone(),
                })
                .collect(),
        };
        repository.complete(snapshot_action.clone(), 20).unwrap();
        let after_first_action = repository.storage.with_connection(|connection| {
            connection.query_row(
                "SELECT state, acknowledged_at, completed_at, updated_at FROM reminder_deliveries WHERE id = ?1",
                [&delivery.id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?, row.get::<_, Option<i64>>(2)?, row.get::<_, i64>(3)?)),
            ).map_err(Into::into)
        }).unwrap();
        let error = repository.acknowledge(snapshot_action, 21).unwrap_err();
        assert_eq!(error.code, AppErrorCode::Conflict);
        assert!(error.retryable);
        let after_stale_action = repository.storage.with_connection(|connection| {
            connection.query_row(
                "SELECT state, acknowledged_at, completed_at, updated_at FROM reminder_deliveries WHERE id = ?1",
                [&delivery.id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?, row.get::<_, Option<i64>>(2)?, row.get::<_, i64>(3)?)),
            ).map_err(Into::into)
        }).unwrap();
        assert_eq!(after_stale_action, after_first_action);
    }

    #[test]
    fn reload_from_an_old_todo_member_id_returns_the_current_revision_group_past_the_cursor() {
        let repository = repository();
        inserted(
            repository
                .enqueue(todo_request("todo-old", "todo-1", 4, 10), 10)
                .unwrap(),
        );
        let old = repository.claim_due(10, 10).unwrap().pop().unwrap();
        inserted(
            repository
                .enqueue(todo_request("todo-current", "todo-1", 5, 11), 11)
                .unwrap(),
        );
        let current = repository.claim_due(11, 10).unwrap().pop().unwrap();
        repository
            .commit_cursor("reminder-alert-window", 9_999_999, 12)
            .unwrap();
        let reloaded = repository.reload_alert_group(&old.id).unwrap().unwrap();
        assert_eq!(
            reloaded.merge_identity,
            ReminderMergeIdentity::Todo {
                todo_id: "todo-1".into(),
                reminder_revision: 5,
                delivery_id: current.id.clone(),
            }
        );
        assert_eq!(
            reloaded
                .members
                .iter()
                .map(|member| &member.id)
                .collect::<Vec<_>>(),
            vec![&current.id]
        );
        assert_ne!(old.id, current.id);
    }

    #[test]
    fn reload_from_an_old_monitor_member_id_returns_the_current_breach_group_past_the_cursor() {
        let repository = repository();
        inserted(
            repository
                .enqueue(monitor_request("monitor-old", "threshold-1", 4, 10), 10)
                .unwrap(),
        );
        let old = repository.claim_due(10, 10).unwrap().pop().unwrap();
        inserted(
            repository
                .enqueue(monitor_request("monitor-current", "threshold-1", 5, 11), 11)
                .unwrap(),
        );
        let current = repository.claim_due(11, 10).unwrap().pop().unwrap();
        repository
            .commit_cursor("reminder-alert-window", 9_999_999, 12)
            .unwrap();
        let reloaded = repository.reload_alert_group(&old.id).unwrap().unwrap();
        assert_eq!(
            reloaded.merge_identity,
            ReminderMergeIdentity::Monitor {
                threshold_id: "threshold-1".into(),
                breach_started_at: 5,
                delivery_id: current.id.clone(),
            }
        );
        assert_eq!(
            reloaded
                .members
                .iter()
                .map(|member| &member.id)
                .collect::<Vec<_>>(),
            vec![&current.id]
        );
        assert_ne!(old.id, current.id);
    }
}
