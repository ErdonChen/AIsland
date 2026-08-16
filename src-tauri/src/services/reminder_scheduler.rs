use crate::contracts::{
    AgentStatus, AgentTriggerStatus, AppErrorCode, CommandError, DeleteResult,
    MessageParameterContract, MessageUsage, PendingReminderNavigation, ReminderActionInput,
    ReminderActionMember, ReminderAlertGroup, ReminderMergeIdentity, ReminderReplay,
    ReminderReplayCursor, ReminderRule, ReminderSourceContext, ReminderSourceKind,
    SafeMessageParameters, SafeParameterValue, SaveReminderRuleInput, SnoozeReminderInput,
};
use crate::domain::agents::ValidatedAgentEvent;
use crate::domain::reminders::{
    reminder_delivery_payload_is_valid, EnqueueOutcome, NewReminderDelivery,
};
use crate::repositories::reminders::ReminderRepository;
use crate::services::EventEmitterPort;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub trait ReminderClock: Send + Sync {
    fn now(&self) -> i64;
    fn sleep_until(&self, due_at: i64) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

pub struct SystemReminderClock;

impl ReminderClock for SystemReminderClock {
    fn now(&self) -> i64 {
        crate::services::now_millis()
    }

    fn sleep_until(&self, due_at: i64) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let delay = due_at.saturating_sub(self.now()).max(0) as u64;
        Box::pin(tokio::time::sleep(std::time::Duration::from_millis(delay)))
    }
}

#[derive(Clone)]
pub struct ReminderService {
    repository: ReminderRepository,
    wake_tx: tokio::sync::mpsc::Sender<()>,
    #[cfg(test)]
    enqueue_observer: Arc<std::sync::Mutex<Option<Arc<dyn Fn(&ReminderRepository) + Send + Sync>>>>,
    #[cfg(test)]
    todo_projection_wake_observer: Arc<std::sync::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>,
}

pub struct ReminderWorker {
    repository: ReminderRepository,
    wake_rx: tokio::sync::mpsc::Receiver<()>,
    clock: Arc<dyn ReminderClock>,
    emitter: Arc<dyn EventEmitterPort>,
}

pub enum ReminderGroupAction {
    Acknowledge,
    Complete,
    Snooze { snoozed_until: i64 },
}

impl ReminderService {
    pub fn new(
        repository: ReminderRepository,
        clock: Arc<dyn ReminderClock>,
        emitter: Arc<dyn EventEmitterPort>,
    ) -> (Arc<Self>, ReminderWorker) {
        let (wake_tx, wake_rx) = tokio::sync::mpsc::channel(1);
        (
            Arc::new(Self {
                repository: repository.clone(),
                wake_tx,
                #[cfg(test)]
                enqueue_observer: Arc::new(std::sync::Mutex::new(None)),
                #[cfg(test)]
                todo_projection_wake_observer: Arc::new(std::sync::Mutex::new(None)),
            }),
            ReminderWorker {
                repository,
                wake_rx,
                clock,
                emitter,
            },
        )
    }

    pub fn enqueue(
        &self,
        request: NewReminderDelivery,
        now: i64,
    ) -> Result<EnqueueOutcome, CommandError> {
        MessageParameterContract::validate_for(
            MessageUsage::ReminderDisplay,
            &request.message_key,
            &request.message_parameters,
        )?;
        if !reminder_delivery_payload_is_valid(&request) {
            return Err(invalid_input());
        }
        let outcome = self.repository.enqueue(request, now)?;
        #[cfg(test)]
        if let Some(observer) = self
            .enqueue_observer
            .lock()
            .expect("reminder enqueue observer lock poisoned")
            .clone()
        {
            observer(&self.repository);
        }
        match self.wake_tx.try_send(()) {
            Ok(()) | Err(tokio::sync::mpsc::error::TrySendError::Full(())) => Ok(outcome),
            Err(tokio::sync::mpsc::error::TrySendError::Closed(())) => Err(storage_unavailable()),
        }
    }

    pub fn project_current_todo(
        &self,
        reminder_id: &str,
        expected_todo_id: &str,
        expected_revision: i64,
        now: i64,
    ) -> Result<EnqueueOutcome, CommandError> {
        let (outcome, projection_error, wake_required) = self.repository.project_current_todo(
            reminder_id,
            expected_todo_id,
            expected_revision,
            now,
        )?;
        #[cfg(test)]
        if outcome.is_some() {
            if let Some(observer) = self
                .enqueue_observer
                .lock()
                .expect("reminder enqueue observer lock poisoned")
                .clone()
            {
                observer(&self.repository);
            }
        }
        if wake_required {
            #[cfg(test)]
            if let Some(observer) = self
                .todo_projection_wake_observer
                .lock()
                .expect("Todo projection wake observer lock poisoned")
                .clone()
            {
                observer();
            }
            match self.wake_tx.try_send(()) {
                Ok(()) | Err(tokio::sync::mpsc::error::TrySendError::Full(())) => {}
                Err(tokio::sync::mpsc::error::TrySendError::Closed(())) => {
                    return Err(storage_unavailable());
                }
            }
        }
        if let Some(error) = projection_error {
            return Err(error);
        }
        outcome.ok_or_else(retryable_conflict)
    }

    pub fn enqueue_agent_event(
        &self,
        event: &ValidatedAgentEvent,
        received_at: i64,
    ) -> Result<Vec<EnqueueOutcome>, CommandError> {
        let Some(trigger_status) = trigger_status(&event.status) else {
            return Ok(Vec::new());
        };
        if event
            .task_title
            .as_deref()
            .is_some_and(has_sensitive_task_title_prefix)
        {
            return Err(invalid_input());
        }
        let mut outcomes = Vec::new();
        for rule in self.repository.list_rules()? {
            if !rule.enabled
                || !rule.agent_ids.contains(&event.agent_id)
                || !rule.trigger_statuses.contains(&trigger_status)
            {
                continue;
            }
            let rule_id = uuid::Uuid::parse_str(&rule.id).map_err(|_| storage_unavailable())?;
            let due_at = std::cmp::max(received_at, event.occurred_at)
                .checked_add(
                    rule.delay_seconds
                        .checked_mul(1_000)
                        .ok_or_else(invalid_input)?,
                )
                .ok_or_else(invalid_input)?;
            let task_title = event
                .task_title
                .as_deref()
                .filter(|title| !title.is_empty())
                .unwrap_or(&event.task_id)
                .to_owned();
            let source_context = ReminderSourceContext::Agent {
                agent_id: event.agent_id.clone(),
                environment: event.environment.clone(),
                task_id: event.task_id.clone(),
                task_title: event.task_title.clone(),
                trigger_status: trigger_status.clone(),
                source_event_id: event.event_id.clone(),
                source_occurred_at: event.occurred_at,
            };
            outcomes.push(self.enqueue(
                NewReminderDelivery {
                    dedupe_key: format!(
                        "agent:{}:{}:{}:{}:{}",
                        rule.id,
                        agent_id_name(&event.agent_id),
                        environment_name(&event.environment),
                        event.task_id,
                        event.event_id
                    ),
                    rule_id: Some(rule_id),
                    source_kind: ReminderSourceKind::Agent,
                    source_entity_id: format!(
                        "agent:{}:{}:{}:{}:{}",
                        rule.id,
                        agent_id_name(&event.agent_id),
                        environment_name(&event.environment),
                        event.task_id,
                        trigger_status_name(&trigger_status),
                    ),
                    message_key: "reminders.agent.status".into(),
                    message_parameters: BTreeMap::from([
                        (
                            "agentName".into(),
                            SafeParameterValue::String(event.agent_id.display_name().into()),
                        ),
                        (
                            "environment".into(),
                            SafeParameterValue::String(environment_name(&event.environment).into()),
                        ),
                        (
                            "taskId".into(),
                            SafeParameterValue::String(event.task_id.clone()),
                        ),
                        ("taskTitle".into(), SafeParameterValue::String(task_title)),
                        (
                            "triggerStatus".into(),
                            SafeParameterValue::String(trigger_status_name(&trigger_status).into()),
                        ),
                    ]),
                    source_context,
                    source_occurred_at: event.occurred_at,
                    sound: rule.sound,
                    toast_enabled: rule.toast_enabled,
                    window_enabled: rule.window_enabled,
                    due_at,
                },
                received_at,
            )?);
        }
        Ok(outcomes)
    }

    pub fn list_rules(&self) -> Result<Vec<ReminderRule>, CommandError> {
        self.repository.list_rules()
    }

    pub fn cancel_pending(
        &self,
        source_kind: ReminderSourceKind,
        source_entity_id: &str,
        now: i64,
    ) -> Result<u64, CommandError> {
        let cancelled = self
            .repository
            .cancel_pending(source_kind, source_entity_id, now)?;
        if cancelled > 0 {
            self.wake();
        }
        Ok(cancelled)
    }

    pub fn save_rule(
        &self,
        input: SaveReminderRuleInput,
        now: i64,
    ) -> Result<ReminderRule, CommandError> {
        let rule = self.repository.save_rule(input, now)?;
        self.wake();
        Ok(rule)
    }

    pub fn delete_rule(
        &self,
        id: &str,
        expected_revision: u64,
        now: i64,
    ) -> Result<DeleteResult, CommandError> {
        let id = uuid::Uuid::parse_str(id).map_err(|_| invalid_input())?;
        let result = self.repository.delete_rule(id, expected_revision, now)?;
        self.wake();
        Ok(result)
    }

    pub fn replay(
        &self,
        consumer_id: &str,
        after_dispatch_seq: u64,
        limit: u32,
    ) -> Result<ReminderReplay, CommandError> {
        self.repository
            .replay(consumer_id, after_dispatch_seq, limit)
    }

    pub fn notification_history_page(
        &self,
        after_dispatch_seq: u64,
        limit: u32,
    ) -> Result<ReminderReplay, CommandError> {
        self.repository
            .notification_history_page(after_dispatch_seq, limit)
    }

    pub fn commit_cursor(
        &self,
        consumer_id: &str,
        last_dispatch_seq: u64,
        now: i64,
    ) -> Result<ReminderReplayCursor, CommandError> {
        self.repository
            .commit_cursor(consumer_id, last_dispatch_seq, now)
    }

    pub fn reload_alert_group(
        &self,
        delivery_id: &str,
    ) -> Result<Option<ReminderAlertGroup>, CommandError> {
        self.repository.reload_alert_group(delivery_id)
    }

    pub fn apply_group_action(
        &self,
        merge_identity: ReminderMergeIdentity,
        expected_member_delivery_ids: Vec<String>,
        members: Vec<ReminderActionMember>,
        action: ReminderGroupAction,
        now: i64,
    ) -> Result<ReminderAlertGroup, CommandError> {
        match action {
            ReminderGroupAction::Acknowledge => self.repository.acknowledge(
                ReminderActionInput {
                    merge_identity,
                    expected_member_delivery_ids,
                    members,
                },
                now,
            ),
            ReminderGroupAction::Complete => self.repository.complete(
                ReminderActionInput {
                    merge_identity,
                    expected_member_delivery_ids,
                    members,
                },
                now,
            ),
            ReminderGroupAction::Snooze { snoozed_until } => self.repository.snooze(
                SnoozeReminderInput {
                    merge_identity,
                    expected_member_delivery_ids,
                    members,
                    snoozed_until,
                },
                now,
            ),
        }
    }

    pub fn pending_navigation(&self) -> Result<Option<PendingReminderNavigation>, CommandError> {
        self.repository.pending_navigation()
    }

    pub fn acknowledge_navigation(&self, sequence: i64, now: i64) -> Result<(), CommandError> {
        self.repository.acknowledge_navigation(sequence, now)
    }

    pub fn wake(&self) {
        let _ = self.wake_tx.try_send(());
    }

    #[cfg(test)]
    pub(crate) fn set_enqueue_observer(
        &self,
        observer: Arc<dyn Fn(&ReminderRepository) + Send + Sync>,
    ) {
        *self
            .enqueue_observer
            .lock()
            .expect("reminder enqueue observer lock poisoned") = Some(observer);
    }

    #[cfg(test)]
    pub(crate) fn set_todo_projection_wake_observer(&self, observer: Arc<dyn Fn() + Send + Sync>) {
        *self
            .todo_projection_wake_observer
            .lock()
            .expect("Todo projection wake observer lock poisoned") = Some(observer);
    }
}

impl ReminderWorker {
    #[cfg(test)]
    fn receiver_capacity_for_test(&self) -> usize {
        self.wake_rx.capacity()
    }

    pub async fn run(mut self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        loop {
            if *shutdown.borrow() {
                return;
            }
            let now = self.clock.now();
            let claimed = match self.repository.claim_due(now, 100) {
                Ok(deliveries) => deliveries,
                Err(_) => return,
            };
            for delivery in claimed {
                let _ = self.emitter.emit(
                    crate::events::REMINDER_DISPATCH_READY,
                    crate::events::reminder_dispatch_ready_payload(
                        &delivery.id,
                        delivery.dispatch_seq,
                    ),
                );
            }

            match self.repository.earliest_due_at() {
                Ok(Some(due_at)) => {
                    tokio::select! {
                        changed = shutdown.changed() => { if changed.is_err() || *shutdown.borrow() { return; } }
                        wake = self.wake_rx.recv() => { if wake.is_none() { return; } }
                        _ = self.clock.sleep_until(due_at) => {}
                    }
                }
                Ok(None) => {
                    tokio::select! {
                        changed = shutdown.changed() => { if changed.is_err() || *shutdown.borrow() { return; } }
                        wake = self.wake_rx.recv() => { if wake.is_none() { return; } }
                    }
                }
                Err(_) => return,
            }
        }
    }
}

fn trigger_status(status: &AgentStatus) -> Option<AgentTriggerStatus> {
    Some(match status {
        AgentStatus::Completed => AgentTriggerStatus::Completed,
        AgentStatus::Failed => AgentTriggerStatus::Failed,
        AgentStatus::Waiting => AgentTriggerStatus::Waiting,
        AgentStatus::Timeout => AgentTriggerStatus::Timeout,
        AgentStatus::Idle | AgentStatus::Running | AgentStatus::Offline => return None,
    })
}

fn agent_id_name(agent_id: &crate::contracts::AgentId) -> &'static str {
    match agent_id {
        crate::contracts::AgentId::Codex => "codex",
        crate::contracts::AgentId::Hermes => "hermes",
        crate::contracts::AgentId::Workbuddy => "workbuddy",
        crate::contracts::AgentId::Claude => "claude",
    }
}

fn environment_name(environment: &crate::contracts::AgentEnvironment) -> &'static str {
    match environment {
        crate::contracts::AgentEnvironment::Windows => "windows",
        crate::contracts::AgentEnvironment::Wsl => "wsl",
    }
}

fn trigger_status_name(status: &AgentTriggerStatus) -> &'static str {
    match status {
        AgentTriggerStatus::Completed => "completed",
        AgentTriggerStatus::Failed => "failed",
        AgentTriggerStatus::Waiting => "waiting",
        AgentTriggerStatus::Timeout => "timeout",
    }
}

fn has_sensitive_task_title_prefix(title: &str) -> bool {
    const STRUCTURED_SOURCE_PREFIXES: [&str; 5] =
        ["prompt:", "tool:", "body:", "token:", "message:"];
    let title = title.trim_start();
    STRUCTURED_SOURCE_PREFIXES.iter().any(|prefix| {
        title
            .get(..prefix.len())
            .is_some_and(|value| value.eq_ignore_ascii_case(prefix))
    })
}

fn invalid_input() -> CommandError {
    CommandError {
        code: AppErrorCode::InvalidInput,
        message_key: "errors.invalidInput".into(),
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

fn storage_unavailable() -> CommandError {
    CommandError {
        code: AppErrorCode::StorageUnavailable,
        message_key: "errors.storageUnavailable".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::{ReminderClock, ReminderService, ReminderWorker, SystemReminderClock};
    use crate::contracts::{
        AgentEnvironment, AgentId, AgentStatus, AgentTriggerStatus, MonitorMetric, ReminderSound,
        ReminderSourceContext, ReminderSourceKind, SafeParameterValue, SaveReminderRuleInput,
    };
    use crate::domain::agents::ValidatedAgentEvent;
    use crate::domain::reminders::{EnqueueOutcome, NewReminderDelivery};
    use crate::repositories::reminders::ReminderRepository;
    use crate::storage::Storage;
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
    use std::sync::{Arc, Mutex};

    struct TestClock;

    impl ReminderClock for TestClock {
        fn now(&self) -> i64 {
            0
        }

        fn sleep_until(&self, _due_at: i64) -> Pin<Box<dyn Future<Output = ()> + Send>> {
            Box::pin(async {})
        }
    }

    struct TestEmitter;

    impl crate::services::EventEmitterPort for TestEmitter {
        fn emit(
            &self,
            _event_name: &'static str,
            _payload: serde_json::Value,
        ) -> Result<(), crate::contracts::CommandError> {
            Ok(())
        }
    }

    struct ManualClockInner {
        now: AtomicI64,
        changed: tokio::sync::Notify,
    }

    #[derive(Clone)]
    struct ManualClock(Arc<ManualClockInner>);

    impl ManualClock {
        fn new(now: i64) -> Self {
            Self(Arc::new(ManualClockInner {
                now: AtomicI64::new(now),
                changed: tokio::sync::Notify::new(),
            }))
        }

        fn advance_to(&self, now: i64) {
            self.0.now.store(now, Ordering::Release);
            self.0.changed.notify_waiters();
        }
    }

    impl ReminderClock for ManualClock {
        fn now(&self) -> i64 {
            self.0.now.load(Ordering::Acquire)
        }

        fn sleep_until(&self, due_at: i64) -> Pin<Box<dyn Future<Output = ()> + Send>> {
            let clock = self.clone();
            Box::pin(async move {
                while clock.now() < due_at {
                    clock.0.changed.notified().await;
                }
            })
        }
    }

    struct RecordingEmitter {
        events: Mutex<Vec<(&'static str, serde_json::Value)>>,
        fail: AtomicBool,
        changed: tokio::sync::Notify,
    }

    impl RecordingEmitter {
        fn new(fail: bool) -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                fail: AtomicBool::new(fail),
                changed: tokio::sync::Notify::new(),
            }
        }

        async fn next(&self) -> (&'static str, serde_json::Value) {
            loop {
                if let Some(event) = self.events.lock().unwrap().pop() {
                    return event;
                }
                self.changed.notified().await;
            }
        }
    }

    impl crate::services::EventEmitterPort for RecordingEmitter {
        fn emit(
            &self,
            event_name: &'static str,
            payload: serde_json::Value,
        ) -> Result<(), crate::contracts::CommandError> {
            if self.fail.load(Ordering::Acquire) {
                return Err(super::storage_unavailable());
            }
            self.events.lock().unwrap().push((event_name, payload));
            self.changed.notify_waiters();
            Ok(())
        }
    }

    struct CommitObservingEmitter {
        repository: ReminderRepository,
        observed: AtomicBool,
    }

    impl crate::services::EventEmitterPort for CommitObservingEmitter {
        fn emit(
            &self,
            event_name: &'static str,
            payload: serde_json::Value,
        ) -> Result<(), crate::contracts::CommandError> {
            assert_eq!(event_name, crate::events::REMINDER_DISPATCH_READY);
            let id = payload["deliveryId"].as_str().expect("typed delivery id");
            let sequence = payload["dispatchSeq"].as_i64().expect("typed sequence");
            let delivery = self
                .repository
                .replay("emit-observer", 0, 100)?
                .deliveries
                .into_iter()
                .find(|delivery| delivery.id == id)
                .expect("claim must commit before synchronous emit");
            assert_eq!(
                delivery.state,
                crate::contracts::ReminderDeliveryState::Dispatched
            );
            assert_eq!(delivery.dispatch_seq, sequence);
            self.observed.store(true, Ordering::Release);
            Ok(())
        }
    }

    fn repository() -> ReminderRepository {
        let directory = tempfile::tempdir().unwrap().keep();
        ReminderRepository::new(Arc::new(Storage::open(&directory).unwrap()))
    }

    fn event(task_title: Option<&str>) -> ValidatedAgentEvent {
        ValidatedAgentEvent {
            event_id: "event-1".into(),
            agent_id: AgentId::Codex,
            environment: AgentEnvironment::Windows,
            task_id: "C:\\Build\\release".into(),
            status: AgentStatus::Failed,
            sequence: Some(1),
            task_title: task_title.map(str::to_owned),
            project: None,
            message: None,
            path: None,
            occurred_at: 100,
        }
    }

    fn rule_input(
        agent_ids: Vec<AgentId>,
        statuses: Vec<AgentTriggerStatus>,
    ) -> SaveReminderRuleInput {
        SaveReminderRuleInput {
            id: None,
            agent_ids,
            trigger_statuses: statuses,
            enabled: true,
            delay_seconds: 3,
            sound: ReminderSound::None,
            toast_enabled: true,
            window_enabled: false,
            expected_revision: None,
        }
    }

    fn agent_delivery() -> NewReminderDelivery {
        NewReminderDelivery {
            dedupe_key: "agent-generic".into(),
            rule_id: None,
            source_kind: ReminderSourceKind::Agent,
            source_entity_id: "agent:rule:codex:windows:task:failed".into(),
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
                    SafeParameterValue::String("task title".into()),
                ),
                (
                    "triggerStatus".into(),
                    SafeParameterValue::String("failed".into()),
                ),
            ]),
            source_context: ReminderSourceContext::Agent {
                agent_id: AgentId::Codex,
                environment: AgentEnvironment::Windows,
                task_id: "task".into(),
                task_title: Some("task title".into()),
                trigger_status: AgentTriggerStatus::Failed,
                source_event_id: "event".into(),
                source_occurred_at: 10,
            },
            source_occurred_at: 10,
            sound: ReminderSound::None,
            toast_enabled: true,
            window_enabled: false,
            due_at: 20,
        }
    }

    fn todo_delivery() -> NewReminderDelivery {
        NewReminderDelivery {
            dedupe_key: "todo-generic".into(),
            rule_id: None,
            source_kind: ReminderSourceKind::Todo,
            source_entity_id: "todo-1".into(),
            message_key: "reminders.todo.due".into(),
            message_parameters: BTreeMap::from([(
                "todoTitle".into(),
                SafeParameterValue::String("todo title".into()),
            )]),
            source_context: ReminderSourceContext::Todo {
                todo_id: "todo-1".into(),
                reminder_revision: 1,
                todo_title: "todo title".into(),
                source_occurred_at: 10,
            },
            source_occurred_at: 10,
            sound: ReminderSound::None,
            toast_enabled: true,
            window_enabled: false,
            due_at: 20,
        }
    }

    fn monitor_delivery() -> NewReminderDelivery {
        NewReminderDelivery {
            dedupe_key: "monitor-generic".into(),
            rule_id: None,
            source_kind: ReminderSourceKind::Monitor,
            source_entity_id: "threshold-1".into(),
            message_key: "reminders.monitor.threshold".into(),
            message_parameters: BTreeMap::from([
                ("metric".into(), SafeParameterValue::String("cpu".into())),
                ("currentValue".into(), SafeParameterValue::Number(90.into())),
                (
                    "thresholdValue".into(),
                    SafeParameterValue::Number(80.into()),
                ),
            ]),
            source_context: ReminderSourceContext::Monitor {
                threshold_id: "threshold-1".into(),
                metric: MonitorMetric::CpuPercent,
                current_value: 90,
                threshold_value: 80,
                breach_started_at: 9,
                source_occurred_at: 10,
            },
            source_occurred_at: 10,
            sound: ReminderSound::None,
            toast_enabled: true,
            window_enabled: false,
            due_at: 20,
        }
    }

    fn assert_no_deliveries(repository: &ReminderRepository) {
        assert!(repository
            .replay("contract-test", 0, 100)
            .unwrap()
            .deliveries
            .is_empty());
    }

    #[test]
    fn handle_ownership_is_clonable_and_worker_is_the_only_receiver_owner() {
        let repository = repository();
        let (handle, worker): (Arc<ReminderService>, ReminderWorker) =
            ReminderService::new(repository, Arc::new(TestClock), Arc::new(TestEmitter));
        let cloned = handle.clone();
        assert!(Arc::ptr_eq(&handle, &cloned));
        assert_eq!(worker.receiver_capacity_for_test(), 1);
    }

    // Break caught: an overdue persisted deadline must be immediately eligible; converting its
    // lateness into a positive sleep defers recovery instead of dispatching the backlog.
    #[tokio::test]
    async fn system_clock_resolves_an_overdue_deadline_without_waiting_for_its_lateness() {
        let clock = SystemReminderClock;
        let overdue_at = clock.now().saturating_sub(1_000);

        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            clock.sleep_until(overdue_at),
        )
        .await
        .expect("overdue deadline must resolve immediately");
    }

    #[test]
    fn enqueue_agent_event_persists_exact_matched_rule_payload_after_commit() {
        let repository = repository();
        let matching_rule = repository
            .save_rule(
                rule_input(
                    vec![AgentId::Codex, AgentId::Hermes],
                    vec![AgentTriggerStatus::Completed, AgentTriggerStatus::Failed],
                ),
                1,
            )
            .unwrap();
        repository
            .save_rule(
                rule_input(vec![AgentId::Claude], vec![AgentTriggerStatus::Failed]),
                1,
            )
            .unwrap();
        let mut disabled = rule_input(vec![AgentId::Codex], vec![AgentTriggerStatus::Failed]);
        disabled.enabled = false;
        repository.save_rule(disabled, 1).unwrap();
        let (service, _worker) = ReminderService::new(
            repository.clone(),
            Arc::new(TestClock),
            Arc::new(TestEmitter),
        );

        let outcomes = service
            .enqueue_agent_event(&event(Some("\\\\server\\share\\release")), 200)
            .unwrap();

        assert_eq!(outcomes.len(), 1);
        let EnqueueOutcome::Inserted(delivery) = &outcomes[0] else {
            panic!("the first matching event must create a delivery");
        };
        assert_eq!(
            delivery.dedupe_key,
            format!(
                "agent:{}:codex:windows:C:\\Build\\release:event-1",
                matching_rule.id
            )
        );
        assert_eq!(delivery.message_key, "reminders.agent.status");
        assert_eq!(
            delivery.source_entity_id,
            format!(
                "agent:{}:codex:windows:C:\\Build\\release:failed",
                matching_rule.id
            )
        );
        assert_eq!(delivery.due_at, 3_200);
        assert_eq!(delivery.source_occurred_at, 100);
        assert_eq!(
            delivery.message_parameters,
            BTreeMap::from([
                (
                    "agentName".into(),
                    SafeParameterValue::String("Codex".into())
                ),
                (
                    "environment".into(),
                    SafeParameterValue::String("windows".into())
                ),
                (
                    "taskId".into(),
                    SafeParameterValue::String("C:\\Build\\release".into())
                ),
                (
                    "taskTitle".into(),
                    SafeParameterValue::String("\\\\server\\share\\release".into())
                ),
                (
                    "triggerStatus".into(),
                    SafeParameterValue::String("failed".into())
                ),
            ])
        );
        let claimed = repository.claim_due(3_200, 10).unwrap();
        assert_eq!(claimed.len(), 1);
        let reloaded = repository.replay("scheduler-test", 0, 10).unwrap();
        assert_eq!(reloaded.deliveries, claimed);
        assert_eq!(reloaded.deliveries[0].dedupe_key, delivery.dedupe_key);
        assert_eq!(
            reloaded.deliveries[0].message_parameters,
            delivery.message_parameters
        );
        assert_eq!(
            reloaded.deliveries[0].source_context,
            delivery.source_context
        );
        assert_eq!(
            reloaded.deliveries[0].source_entity_id,
            delivery.source_entity_id
        );
        assert_eq!(
            reloaded.deliveries[0].source_entity_id.as_bytes(),
            format!(
                "agent:{}:codex:windows:C:\\Build\\release:failed",
                matching_rule.id
            )
            .as_bytes()
        );
        assert_eq!(
            match &reloaded.deliveries[0].source_context {
                ReminderSourceContext::Agent {
                    task_id,
                    task_title,
                    ..
                } => {
                    (
                        task_id.as_bytes(),
                        task_title.as_deref().unwrap().as_bytes(),
                    )
                }
                _ => panic!("agent reminder must reload as an agent context"),
            },
            (
                b"C:\\Build\\release".as_slice(),
                b"\\\\server\\share\\release".as_slice()
            )
        );
    }

    #[test]
    fn enqueue_requires_exact_message_parameters_and_matching_source_context_arm() {
        let repository = repository();
        let (service, _worker) = ReminderService::new(
            repository.clone(),
            Arc::new(TestClock),
            Arc::new(TestEmitter),
        );

        for request in [agent_delivery(), todo_delivery(), monitor_delivery()] {
            service.enqueue(request, 10).unwrap();
        }

        for (index, parameter) in [
            "agentName",
            "environment",
            "taskId",
            "taskTitle",
            "triggerStatus",
        ]
        .into_iter()
        .enumerate()
        {
            let mut request = agent_delivery();
            request.dedupe_key = format!("agent-missing-{index}");
            request.message_parameters.remove(parameter);
            assert!(service.enqueue(request, 10).is_err());
        }
        for (index, parameter) in ["todoTitle"].into_iter().enumerate() {
            let mut request = todo_delivery();
            request.dedupe_key = format!("todo-missing-{index}");
            request.message_parameters.remove(parameter);
            assert!(service.enqueue(request, 10).is_err());
        }
        for (index, parameter) in ["metric", "currentValue", "thresholdValue"]
            .into_iter()
            .enumerate()
        {
            let mut request = monitor_delivery();
            request.dedupe_key = format!("monitor-missing-{index}");
            request.message_parameters.remove(parameter);
            assert!(service.enqueue(request, 10).is_err());
        }
        for (index, mut request) in [agent_delivery(), todo_delivery(), monitor_delivery()]
            .into_iter()
            .enumerate()
        {
            request.dedupe_key = format!("extra-{index}");
            request.message_parameters.insert(
                "unexpected".into(),
                SafeParameterValue::String("value".into()),
            );
            assert!(service.enqueue(request, 10).is_err());
        }
        for (index, (source_kind, message_key, message_parameters, source_context)) in [
            (
                ReminderSourceKind::Agent,
                "reminders.agent.status",
                agent_delivery().message_parameters,
                todo_delivery().source_context,
            ),
            (
                ReminderSourceKind::Todo,
                "reminders.todo.due",
                todo_delivery().message_parameters,
                monitor_delivery().source_context,
            ),
            (
                ReminderSourceKind::Monitor,
                "reminders.monitor.threshold",
                monitor_delivery().message_parameters,
                agent_delivery().source_context,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let mut request = agent_delivery();
            request.dedupe_key = format!("arm-mismatch-{index}");
            request.source_kind = source_kind;
            request.message_key = message_key.into();
            request.message_parameters = message_parameters;
            request.source_context = source_context;
            assert!(service.enqueue(request, 10).is_err());
        }
        let mut request = todo_delivery();
        request.dedupe_key = "occurred-at-mismatch".into();
        request.source_occurred_at = 11;
        assert!(service.enqueue(request, 10).is_err());
        assert_eq!(
            repository.claim_due(20, 100).unwrap().len(),
            3,
            "invalid requests must make zero writes"
        );
    }

    #[test]
    fn enqueue_agent_event_uses_task_id_when_source_title_is_null_or_empty() {
        let repository = repository();
        repository
            .save_rule(
                rule_input(vec![AgentId::Codex], vec![AgentTriggerStatus::Failed]),
                1,
            )
            .unwrap();
        let (service, _worker) =
            ReminderService::new(repository, Arc::new(TestClock), Arc::new(TestEmitter));

        for (source_title, expected_source_title) in [(None, None), (Some(""), Some(""))] {
            let mut source = event(source_title);
            source.event_id = match source_title {
                None => "event-null".into(),
                Some(_) => "event-empty".into(),
            };
            let outcomes = service.enqueue_agent_event(&source, 200).unwrap();
            let EnqueueOutcome::Inserted(delivery) = &outcomes[0] else {
                panic!("the first matching event must create a delivery");
            };
            assert_eq!(
                delivery.message_parameters["taskTitle"],
                SafeParameterValue::String("C:\\Build\\release".into())
            );
            assert!(matches!(
                delivery.source_context,
                crate::contracts::ReminderSourceContext::Agent { ref task_title, .. }
                    if task_title.as_deref() == expected_source_title
            ));
        }
    }

    #[test]
    fn enqueue_agent_event_ignores_sensitive_message_text_when_title_is_absent() {
        let repository = repository();
        repository
            .save_rule(
                rule_input(vec![AgentId::Codex], vec![AgentTriggerStatus::Failed]),
                1,
            )
            .unwrap();
        let (service, _worker) =
            ReminderService::new(repository, Arc::new(TestClock), Arc::new(TestEmitter));
        let mut source = event(None);
        source.message = Some("prompt: authorize tool with token body".into());

        let outcomes = service.enqueue_agent_event(&source, 200).unwrap();
        let EnqueueOutcome::Inserted(delivery) = &outcomes[0] else {
            panic!("the event must create a delivery using the safe fallback");
        };
        assert_eq!(
            delivery.message_parameters["taskTitle"],
            SafeParameterValue::String("C:\\Build\\release".into())
        );
        assert!(matches!(
            delivery.source_context,
            crate::contracts::ReminderSourceContext::Agent {
                task_title: None,
                ..
            }
        ));
    }

    #[test]
    fn enqueue_agent_event_rejects_explicit_sensitive_title_prefixes_before_writing() {
        let repository = repository();
        repository
            .save_rule(
                rule_input(vec![AgentId::Codex], vec![AgentTriggerStatus::Failed]),
                1,
            )
            .unwrap();
        let (service, _worker) = ReminderService::new(
            repository.clone(),
            Arc::new(TestClock),
            Arc::new(TestEmitter),
        );

        for (index, title) in [
            "prompt: authorize tool with token body",
            "TOOL: invoke",
            "body: secret",
            "Token: credential",
        ]
        .into_iter()
        .enumerate()
        {
            let mut source = event(Some(title));
            source.event_id = format!("sensitive-{index}");
            assert!(service.enqueue_agent_event(&source, 200).is_err());
        }
        assert_no_deliveries(&repository);

        let mut ordinary = event(Some("token refresh"));
        ordinary.event_id = "ordinary-title".into();
        assert!(service.enqueue_agent_event(&ordinary, 200).is_ok());
    }

    #[test]
    fn closed_wake_returns_storage_unavailable_without_undoing_the_committed_delivery() {
        let repository = repository();
        repository
            .save_rule(
                rule_input(vec![AgentId::Codex], vec![AgentTriggerStatus::Failed]),
                1,
            )
            .unwrap();
        let (service, worker) = ReminderService::new(
            repository.clone(),
            Arc::new(TestClock),
            Arc::new(TestEmitter),
        );
        drop(worker);

        let error = service.enqueue_agent_event(&event(None), 200).unwrap_err();
        assert_eq!(error.message_key, "errors.storageUnavailable");
        let claimed = repository.claim_due(3_200, 10).unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(
            claimed[0].state,
            crate::contracts::ReminderDeliveryState::Dispatched
        );
    }

    // Break caught: a delayed row must not be claimed or emitted merely because its enqueue wake
    // arrives before the durable due timestamp.
    #[tokio::test]
    async fn worker_dispatches_only_when_the_manual_clock_reaches_due_at() {
        let repository = repository();
        let clock = Arc::new(ManualClock::new(19));
        let emitter = Arc::new(RecordingEmitter::new(false));
        let (service, worker) =
            ReminderService::new(repository.clone(), clock.clone(), emitter.clone());
        let mut request = agent_delivery();
        request.dedupe_key = "worker-due-gate".into();
        request.due_at = 20;
        let delivery = match service.enqueue(request, 19).unwrap() {
            EnqueueOutcome::Inserted(delivery) => delivery,
            EnqueueOutcome::Duplicate(_) => panic!("expected a newly persisted delivery"),
        };
        let (shutdown_tx, shutdown) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(worker.run(shutdown));

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), emitter.next())
                .await
                .is_err()
        );
        assert!(repository
            .replay("due-gate", 0, 10)
            .unwrap()
            .deliveries
            .is_empty());
        clock.advance_to(20);
        let (event_name, payload) =
            tokio::time::timeout(std::time::Duration::from_secs(1), emitter.next())
                .await
                .unwrap();
        assert_eq!(event_name, crate::events::REMINDER_DISPATCH_READY);
        assert_eq!(
            payload,
            serde_json::json!({ "deliveryId": delivery.id, "dispatchSeq": 1 })
        );
        shutdown_tx.send_replace(true);
        task.await.unwrap();
    }

    // Break caught: both enqueue and dispatch boundaries expose only committed SQLite state to
    // their observers; moving emit before the durable claim makes this emitter fail synchronously.
    #[tokio::test]
    async fn enqueue_and_emit_observers_see_committed_pending_then_dispatched_state() {
        let repository = repository();
        let emitter = Arc::new(CommitObservingEmitter {
            repository: repository.clone(),
            observed: AtomicBool::new(false),
        });
        let (service, worker) = ReminderService::new(
            repository.clone(),
            Arc::new(ManualClock::new(20)),
            emitter.clone(),
        );
        let mut request = agent_delivery();
        request.dedupe_key = "commit-observer".into();
        request.due_at = 20;
        let delivery = match service.enqueue(request, 20).unwrap() {
            EnqueueOutcome::Inserted(delivery) => delivery,
            EnqueueOutcome::Duplicate(_) => unreachable!(),
        };
        assert_eq!(repository.earliest_due_at().unwrap(), Some(20));
        assert!(repository
            .replay("before-wake", 0, 10)
            .unwrap()
            .deliveries
            .is_empty());
        let (shutdown_tx, shutdown) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(worker.run(shutdown));
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !emitter.observed.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let dispatched = repository.replay("after-emit", 0, 10).unwrap().deliveries;
        assert_eq!(dispatched.len(), 1);
        assert_eq!(dispatched[0].id, delivery.id);
        shutdown_tx.send_replace(true);
        task.await.unwrap();
    }

    // Break caught: the enqueue wake hint may only be observed after the new row is committed as
    // pending; placing the observer after try_send makes this assertion unable to protect order.
    #[test]
    fn enqueue_observer_sees_the_pending_row_before_the_wake_hint() {
        let repository = repository();
        let (service, _worker) = ReminderService::new(
            repository.clone(),
            Arc::new(TestClock),
            Arc::new(TestEmitter),
        );
        let observed = Arc::new(AtomicBool::new(false));
        service.set_enqueue_observer(Arc::new({
            let observed = observed.clone();
            move |repository| {
                assert_eq!(repository.earliest_due_at().unwrap(), Some(20));
                assert!(repository
                    .replay("wake-observer", 0, 10)
                    .unwrap()
                    .deliveries
                    .is_empty());
                observed.store(true, Ordering::Release);
            }
        }));
        let mut request = agent_delivery();
        request.dedupe_key = "wake-commit-observer".into();
        request.due_at = 20;
        service.enqueue(request, 10).unwrap();
        assert!(observed.load(Ordering::Acquire));
    }

    // Coverage hardening: an emission failure happens after the durable claim, so the delivery
    // remains available to a restarting consumer and no acknowledgement is manufactured.
    #[tokio::test]
    async fn emitter_failure_keeps_the_durable_dispatch_replayable_without_an_acknowledgement() {
        let repository = repository();
        let clock = Arc::new(ManualClock::new(20));
        let emitter = Arc::new(RecordingEmitter::new(true));
        let (service, worker) = ReminderService::new(repository.clone(), clock, emitter.clone());
        let mut request = agent_delivery();
        request.dedupe_key = "worker-emitter-failure".into();
        request.due_at = 20;
        service.enqueue(request, 20).unwrap();
        let (shutdown_tx, shutdown) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(worker.run(shutdown));
        for _ in 0..20 {
            if !repository
                .replay("recovery", 0, 10)
                .unwrap()
                .deliveries
                .is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        let replay = repository.replay("recovery", 0, 10).unwrap();
        assert_eq!(replay.deliveries.len(), 1);
        assert_eq!(
            replay.deliveries[0].state,
            crate::contracts::ReminderDeliveryState::Dispatched
        );
        assert_eq!(replay.deliveries[0].acknowledged_at, None);
        assert!(emitter.events.lock().unwrap().is_empty());
        shutdown_tx.send_replace(true);
        task.await.unwrap();
    }

    // Break caught: an interruption before the deadline must leave the one persisted row for a
    // newly constructed worker to claim exactly once after restart.
    #[tokio::test]
    async fn restart_after_shutdown_before_due_claims_the_same_persisted_delivery_once() {
        let directory = tempfile::tempdir().unwrap().keep();
        let first_repository =
            ReminderRepository::new(Arc::new(Storage::open(&directory).unwrap()));
        let first_clock = Arc::new(ManualClock::new(19));
        let first_emitter = Arc::new(RecordingEmitter::new(false));
        let (service, worker) =
            ReminderService::new(first_repository.clone(), first_clock, first_emitter);
        let mut request = agent_delivery();
        request.dedupe_key = "worker-restart".into();
        request.due_at = 20;
        let id = match service.enqueue(request, 19).unwrap() {
            EnqueueOutcome::Inserted(delivery) => delivery.id,
            EnqueueOutcome::Duplicate(_) => panic!("first enqueue must create the delivery"),
        };
        let (first_shutdown_tx, first_shutdown) = tokio::sync::watch::channel(false);
        let first_task = tokio::spawn(worker.run(first_shutdown));
        first_shutdown_tx.send_replace(true);
        first_task.await.unwrap();

        let restarted_repository =
            ReminderRepository::new(Arc::new(Storage::open(&directory).unwrap()));
        let restarted_emitter = Arc::new(RecordingEmitter::new(false));
        let (_handle, restarted_worker) = ReminderService::new(
            restarted_repository.clone(),
            Arc::new(ManualClock::new(20)),
            restarted_emitter.clone(),
        );
        let (second_shutdown_tx, second_shutdown) = tokio::sync::watch::channel(false);
        let second_task = tokio::spawn(restarted_worker.run(second_shutdown));
        let (_, payload) =
            tokio::time::timeout(std::time::Duration::from_secs(1), restarted_emitter.next())
                .await
                .unwrap();
        assert_eq!(
            payload,
            serde_json::json!({ "deliveryId": id, "dispatchSeq": 1 })
        );
        assert_eq!(
            restarted_repository
                .replay("restart", 0, 10)
                .unwrap()
                .deliveries
                .len(),
            1
        );
        second_shutdown_tx.send_replace(true);
        second_task.await.unwrap();
    }

    // Coverage hardening: coalesced wake hints and a repeated dedupe key cannot create a second
    // persistent row or a second dispatch event.
    #[tokio::test]
    async fn duplicate_wakes_and_dedupe_key_dispatch_only_one_delivery() {
        let repository = repository();
        let clock = Arc::new(ManualClock::new(20));
        let emitter = Arc::new(RecordingEmitter::new(false));
        let (service, worker) = ReminderService::new(repository.clone(), clock, emitter.clone());
        let mut request = agent_delivery();
        request.dedupe_key = "worker-dedupe-wake".into();
        request.due_at = 20;
        service.enqueue(request.clone(), 20).unwrap();
        assert!(matches!(
            service.enqueue(request, 20).unwrap(),
            EnqueueOutcome::Duplicate(_)
        ));
        for _ in 0..5 {
            service.wake();
        }
        let (shutdown_tx, shutdown) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(worker.run(shutdown));
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), emitter.next())
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), emitter.next())
                .await
                .is_err()
        );
        assert_eq!(
            repository.replay("dedupe", 0, 10).unwrap().deliveries.len(),
            1
        );
        shutdown_tx.send_replace(true);
        task.await.unwrap();
    }

    // Break caught: shutdown must cancel a far-future manual-clock wait promptly, without a
    // post-completion event or loss of its pending durable row.
    #[tokio::test]
    async fn shutdown_interrupts_far_future_wait_without_emitting_or_losing_the_delivery() {
        let repository = repository();
        let emitter = Arc::new(RecordingEmitter::new(false));
        let (service, worker) = ReminderService::new(
            repository.clone(),
            Arc::new(ManualClock::new(0)),
            emitter.clone(),
        );
        let mut request = agent_delivery();
        request.dedupe_key = "worker-future-shutdown".into();
        request.due_at = 9_999_999;
        service.enqueue(request, 0).unwrap();
        let (shutdown_tx, shutdown) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(worker.run(shutdown));
        tokio::task::yield_now().await;
        shutdown_tx.send_replace(true);
        tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert!(emitter.events.lock().unwrap().is_empty());
        assert!(repository
            .replay("future", 0, 10)
            .unwrap()
            .deliveries
            .is_empty());
    }

    // Coverage hardening: with no due row the worker is parked only on the wake receiver, and
    // shutdown still joins it immediately without synthesizing a dispatch event.
    #[tokio::test]
    async fn shutdown_interrupts_empty_wake_wait_without_emitting() {
        let repository = repository();
        let emitter = Arc::new(RecordingEmitter::new(false));
        let (_service, worker) =
            ReminderService::new(repository, Arc::new(ManualClock::new(0)), emitter.clone());
        let (shutdown_tx, shutdown) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(worker.run(shutdown));
        tokio::task::yield_now().await;
        shutdown_tx.send_replace(true);
        tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert!(emitter.events.lock().unwrap().is_empty());
    }
}
