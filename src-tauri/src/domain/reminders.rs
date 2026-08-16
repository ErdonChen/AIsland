use crate::contracts::{
    ReminderDelivery, ReminderSound, ReminderSourceContext, ReminderSourceKind,
    SafeMessageParameters,
};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewReminderDelivery {
    pub dedupe_key: String,
    pub rule_id: Option<Uuid>,
    pub source_kind: ReminderSourceKind,
    pub source_entity_id: String,
    pub message_key: String,
    pub message_parameters: SafeMessageParameters,
    pub source_context: ReminderSourceContext,
    pub source_occurred_at: i64,
    pub sound: ReminderSound,
    pub toast_enabled: bool,
    pub window_enabled: bool,
    pub due_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnqueueOutcome {
    Inserted(ReminderDelivery),
    Duplicate(ReminderDelivery),
}

pub fn source_context_is_valid(context: &ReminderSourceContext) -> bool {
    match context {
        ReminderSourceContext::Agent {
            task_id,
            source_event_id,
            source_occurred_at,
            ..
        } => !task_id.is_empty() && !source_event_id.is_empty() && *source_occurred_at >= 0,
        ReminderSourceContext::Todo {
            todo_id,
            source_occurred_at,
            ..
        } => !todo_id.is_empty() && *source_occurred_at >= 0,
        ReminderSourceContext::Monitor {
            threshold_id,
            source_occurred_at,
            ..
        } => !threshold_id.is_empty() && *source_occurred_at >= 0,
    }
}

pub fn reminder_delivery_payload_is_valid(request: &NewReminderDelivery) -> bool {
    source_context_is_valid(&request.source_context)
        && source_context_occurred_at(&request.source_context) == request.source_occurred_at
        && match (
            &request.source_kind,
            request.message_key.as_str(),
            &request.source_context,
        ) {
            (
                ReminderSourceKind::Agent,
                "reminders.agent.status",
                ReminderSourceContext::Agent { .. },
            ) => has_exact_parameter_names(
                &request.message_parameters,
                &[
                    "agentName",
                    "environment",
                    "taskId",
                    "taskTitle",
                    "triggerStatus",
                ],
            ),
            (
                ReminderSourceKind::Todo,
                "reminders.todo.due",
                ReminderSourceContext::Todo { .. },
            ) => has_exact_parameter_names(&request.message_parameters, &["todoTitle"]),
            (
                ReminderSourceKind::Monitor,
                "reminders.monitor.threshold",
                ReminderSourceContext::Monitor { .. },
            ) => has_exact_parameter_names(
                &request.message_parameters,
                &["metric", "currentValue", "thresholdValue"],
            ),
            _ => false,
        }
}

fn source_context_occurred_at(context: &ReminderSourceContext) -> i64 {
    match context {
        ReminderSourceContext::Agent {
            source_occurred_at, ..
        }
        | ReminderSourceContext::Todo {
            source_occurred_at, ..
        }
        | ReminderSourceContext::Monitor {
            source_occurred_at, ..
        } => *source_occurred_at,
    }
}

fn has_exact_parameter_names(parameters: &SafeMessageParameters, expected: &[&str]) -> bool {
    parameters.len() == expected.len() && expected.iter().all(|name| parameters.contains_key(*name))
}
