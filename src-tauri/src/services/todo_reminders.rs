use crate::contracts::{
    AppErrorCode, CommandError, DeleteResult, ReminderSourceKind, SafeMessageParameters,
    SaveTodoReminderInput, TodoReminder, TodoStatus,
};
use crate::domain::reminders::EnqueueOutcome;
use crate::repositories::todos::{TodoReminderRepository, TodoRepository};
use crate::services::reminder_scheduler::ReminderService;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct TodoReminderProjector {
    todos: TodoRepository,
    reminders: TodoReminderRepository,
    scheduler: Arc<ReminderService>,
    #[cfg(test)]
    before_projection_commit: Arc<std::sync::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReminderReconcileResult {
    pub enqueued: u64,
    pub cancelled: u64,
}

impl TodoReminderProjector {
    pub fn new(
        todos: TodoRepository,
        reminders: TodoReminderRepository,
        scheduler: Arc<ReminderService>,
    ) -> Self {
        Self {
            todos,
            reminders,
            scheduler,
            #[cfg(test)]
            before_projection_commit: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    #[cfg(test)]
    fn set_before_projection_commit(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        *self
            .before_projection_commit
            .lock()
            .expect("Todo projection hook lock poisoned") = Some(hook);
    }

    pub fn save_and_project(
        &self,
        input: SaveTodoReminderInput,
        now: i64,
    ) -> Result<TodoReminder, CommandError> {
        let reminder = self.reminders.save(input, now)?;
        if reminder.enabled {
            self.project(&reminder, now)?;
        } else {
            self.cancel(parse_id(&reminder.todo_id)?, now)?;
        }
        Ok(reminder)
    }

    // Retained as the Task 3 service interface; the Tauri command performs the
    // same two operations around its required commit-then-emit ordering.
    #[allow(dead_code)]
    pub fn delete_and_cancel(
        &self,
        id: Uuid,
        expected_revision: u64,
        now: i64,
    ) -> Result<DeleteResult, CommandError> {
        let reminder = self
            .reminders
            .list(None)?
            .into_iter()
            .find(|reminder| reminder.id == id.to_string())
            .ok_or_else(not_found)?;
        let result = self.reminders.delete(id, expected_revision)?;
        self.cancel(parse_id(&reminder.todo_id)?, now)?;
        Ok(result)
    }

    pub fn reconcile(&self, now: i64) -> Result<ReminderReconcileResult, CommandError> {
        if now < 0 {
            return Err(invalid_input());
        }
        let pending_sources = self
            .reminders
            .list_pending_delivery_source_ids()?
            .into_iter()
            .collect::<BTreeSet<_>>();
        let enabled = self.reminders.list_enabled()?;
        let enabled_by_source = enabled
            .iter()
            .map(|reminder| Ok((parse_id(&reminder.todo_id)?, reminder)))
            .collect::<Result<BTreeMap<_, _>, CommandError>>()?;
        let mut cancelled = 0_u64;

        for source_id in pending_sources {
            let keep_current = enabled_by_source
                .get(&source_id)
                .map(|_| self.todos.get(source_id))
                .transpose()?
                .is_some_and(|todo| todo.status == TodoStatus::Open);
            if !keep_current {
                cancelled = cancelled
                    .checked_add(self.cancel(source_id, now)?)
                    .ok_or_else(database_failure)?;
            }
        }

        let mut enqueued = 0_u64;
        for reminder in enabled {
            let todo_id = parse_id(&reminder.todo_id)?;
            let todo = self.todos.get(todo_id)?;
            if todo.status == TodoStatus::Completed {
                cancelled = cancelled
                    .checked_add(self.cancel(todo_id, now)?)
                    .ok_or_else(database_failure)?;
                continue;
            }
            cancelled = cancelled
                .checked_add(self.cancel(todo_id, now)?)
                .ok_or_else(database_failure)?;
            if matches!(self.project(&reminder, now)?, EnqueueOutcome::Inserted(_)) {
                enqueued = enqueued.checked_add(1).ok_or_else(database_failure)?;
            }
        }
        if enqueued > 0 || cancelled > 0 {
            self.scheduler.wake();
        }
        Ok(ReminderReconcileResult {
            enqueued,
            cancelled,
        })
    }

    pub fn project(
        &self,
        reminder: &TodoReminder,
        now: i64,
    ) -> Result<EnqueueOutcome, CommandError> {
        if !reminder.enabled || now < 0 {
            return Err(invalid_input());
        }
        parse_id(&reminder.todo_id)?;
        #[cfg(test)]
        if let Some(hook) = self
            .before_projection_commit
            .lock()
            .expect("Todo projection hook lock poisoned")
            .clone()
        {
            hook();
        }
        self.scheduler
            .project_current_todo(&reminder.id, &reminder.todo_id, reminder.revision, now)
    }

    pub fn cancel(&self, todo_id: Uuid, now: i64) -> Result<u64, CommandError> {
        self.scheduler
            .cancel_pending(ReminderSourceKind::Todo, &todo_id.to_string(), now)
    }
}

fn parse_id(value: &str) -> Result<Uuid, CommandError> {
    Uuid::parse_str(value).map_err(|_| invalid_input())
}

fn invalid_input() -> CommandError {
    CommandError {
        code: AppErrorCode::InvalidInput,
        message_key: "errors.invalidInput".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

#[allow(dead_code)]
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
    use super::{ReminderReconcileResult, TodoReminderProjector};
    use crate::contracts::{
        AppErrorCode, BuiltinReminderSoundId, CreateTodoInput, ReminderActionMember,
        ReminderDeliveryState, ReminderMergeIdentity, ReminderSound, ReminderSourceContext,
        SafeParameterValue, SaveTodoReminderInput, SnoozeReminderInput, TodoPriority,
        UpdateTodoInput,
    };
    use crate::domain::reminders::EnqueueOutcome;
    use crate::repositories::reminders::ReminderRepository;
    use crate::repositories::todos::{TodoReminderRepository, TodoRepository};
    use crate::services::reminder_scheduler::{ReminderClock, ReminderService, ReminderWorker};
    use crate::services::EventEmitterPort;
    use crate::storage::Storage;
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
    use std::sync::{Arc, Mutex};

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
        changed: tokio::sync::Notify,
    }

    impl RecordingEmitter {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
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

    impl EventEmitterPort for RecordingEmitter {
        fn emit(
            &self,
            event_name: &'static str,
            payload: serde_json::Value,
        ) -> Result<(), crate::contracts::CommandError> {
            self.events.lock().unwrap().push((event_name, payload));
            self.changed.notify_waiters();
            Ok(())
        }
    }

    struct Fixture {
        storage: Arc<Storage>,
        todos: TodoRepository,
        reminders: TodoReminderRepository,
        projector: TodoReminderProjector,
        scheduler: Arc<ReminderService>,
        worker: Option<ReminderWorker>,
        emitter: Arc<RecordingEmitter>,
        path: std::path::PathBuf,
    }

    impl Fixture {
        fn at(now: i64) -> Self {
            let path = tempfile::tempdir().unwrap().keep();
            Self::open(path, now)
        }

        fn open(path: std::path::PathBuf, now: i64) -> Self {
            let storage = Arc::new(Storage::open(&path).unwrap());
            let todos = TodoRepository::new(storage.clone());
            let reminders = TodoReminderRepository::new(storage.clone());
            let emitter = Arc::new(RecordingEmitter::new());
            let (scheduler, worker) = ReminderService::new(
                ReminderRepository::new(storage.clone()),
                Arc::new(ManualClock::new(now)),
                emitter.clone(),
            );
            let projector =
                TodoReminderProjector::new(todos.clone(), reminders.clone(), scheduler.clone());
            Self {
                storage,
                todos,
                reminders,
                projector,
                scheduler,
                worker: Some(worker),
                emitter,
                path,
            }
        }

        fn todo(&self, title: &str, description: &str, now: i64) -> crate::contracts::TodoItem {
            self.todos
                .create(
                    CreateTodoInput {
                        title: title.into(),
                        description: description.into(),
                        due_at: None,
                        priority: TodoPriority::Normal,
                    },
                    now,
                )
                .unwrap()
        }

        fn delivery(&self, dedupe_key: &str) -> Option<PersistedDelivery> {
            self.storage
                .with_connection(|connection| {
                    connection
                        .query_row(
                            r#"SELECT id, rule_id, source_kind, source_entity_id, message_key,
                                      message_parameters_json, source_context_json, source_occurred_at,
                                      sound_json, state, due_at
                               FROM reminder_deliveries WHERE dedupe_key = ?1"#,
                            [dedupe_key],
                            |row| {
                                let parameters: String = row.get(5)?;
                                let context: String = row.get(6)?;
                                let sound: String = row.get(8)?;
                                Ok(PersistedDelivery {
                                    id: row.get(0)?,
                                    rule_id: row.get(1)?,
                                    source_kind: row.get(2)?,
                                    source_entity_id: row.get(3)?,
                                    message_key: row.get(4)?,
                                    message_parameters: serde_json::from_str(&parameters).unwrap(),
                                    source_context: serde_json::from_str(&context).unwrap(),
                                    source_occurred_at: row.get(7)?,
                                    sound: serde_json::from_str(&sound).unwrap(),
                                    state: row.get(9)?,
                                    due_at: row.get(10)?,
                                })
                            },
                        )
                        .optional()
                        .map_err(Into::into)
                })
                .unwrap()
        }

        fn delivery_count(&self) -> i64 {
            self.storage
                .with_connection(|connection| {
                    connection
                        .query_row("SELECT COUNT(*) FROM reminder_deliveries", [], |row| {
                            row.get(0)
                        })
                        .map_err(Into::into)
                })
                .unwrap()
        }

        fn active_todo_delivery_keys(&self, todo_id: &str) -> Vec<String> {
            self.storage
                .with_connection(|connection| {
                    let mut statement = connection.prepare(
                        "SELECT dedupe_key FROM reminder_deliveries WHERE source_kind = 'todo' AND source_entity_id = ?1 AND state IN ('pending', 'snoozed') ORDER BY dedupe_key",
                    )?;
                    let rows = statement
                        .query_map([todo_id], |row| row.get(0))?
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(rows)
                })
                .unwrap()
        }
    }

    use rusqlite::OptionalExtension;

    struct PersistedDelivery {
        id: String,
        rule_id: Option<String>,
        source_kind: String,
        source_entity_id: String,
        message_key: String,
        message_parameters: BTreeMap<String, SafeParameterValue>,
        source_context: ReminderSourceContext,
        source_occurred_at: i64,
        sound: ReminderSound,
        state: String,
        due_at: i64,
    }

    fn blocking_projector(fixture: &Fixture) -> (TodoReminderProjector, Arc<std::sync::Barrier>) {
        let projector = TodoReminderProjector::new(
            fixture.todos.clone(),
            fixture.reminders.clone(),
            fixture.scheduler.clone(),
        );
        let barrier = Arc::new(std::sync::Barrier::new(2));
        projector.set_before_projection_commit({
            let barrier = barrier.clone();
            Arc::new(move || {
                barrier.wait();
                barrier.wait();
            })
        });
        (projector, barrier)
    }

    fn snooze_todo_delivery(
        fixture: &Fixture,
        reminder: &crate::contracts::TodoReminder,
    ) -> String {
        let repository = ReminderRepository::new(fixture.storage.clone());
        let delivery = repository
            .claim_due(reminder.remind_at, 10)
            .unwrap()
            .into_iter()
            .find(|delivery| delivery.source_entity_id == reminder.todo_id)
            .unwrap();
        repository
            .snooze(
                SnoozeReminderInput {
                    merge_identity: ReminderMergeIdentity::Todo {
                        todo_id: reminder.todo_id.clone(),
                        reminder_revision: reminder.revision,
                        delivery_id: delivery.id.clone(),
                    },
                    expected_member_delivery_ids: vec![delivery.id.clone()],
                    members: vec![ReminderActionMember {
                        id: delivery.id.clone(),
                        expected_state: ReminderDeliveryState::Dispatched,
                    }],
                    snoozed_until: reminder.remind_at + 10_000,
                },
                reminder.remind_at + 1,
            )
            .unwrap();
        delivery.id
    }

    fn saved_snoozed_reminder(
        fixture: &Fixture,
        title: &str,
    ) -> (crate::contracts::TodoItem, crate::contracts::TodoReminder) {
        let todo = fixture.todo(title, "", 100);
        let reminder = fixture
            .projector
            .save_and_project(
                SaveTodoReminderInput {
                    id: None,
                    todo_id: todo.id.clone(),
                    remind_at: 1_000,
                    enabled: true,
                    expected_revision: None,
                },
                100,
            )
            .unwrap();
        snooze_todo_delivery(fixture, &reminder);
        (todo, reminder)
    }

    #[test]
    fn snoozed_todo_delivery_is_cancelled_by_reminder_edit() {
        let fixture = Fixture::at(100);
        let (todo, first) = saved_snoozed_reminder(&fixture, "snooze edit");
        let current = fixture
            .projector
            .save_and_project(
                SaveTodoReminderInput {
                    id: Some(first.id),
                    todo_id: todo.id.clone(),
                    remind_at: 2_000,
                    enabled: true,
                    expected_revision: Some(first.revision),
                },
                200,
            )
            .unwrap();
        assert_eq!(
            fixture.active_todo_delivery_keys(&todo.id),
            vec![format!("todo:{}:{}", todo.id, current.revision)]
        );
    }

    #[test]
    fn snoozed_todo_delivery_is_cancelled_when_reminder_is_disabled() {
        let fixture = Fixture::at(100);
        let (todo, first) = saved_snoozed_reminder(&fixture, "snooze disable");
        fixture
            .projector
            .save_and_project(
                SaveTodoReminderInput {
                    id: Some(first.id),
                    todo_id: todo.id.clone(),
                    remind_at: first.remind_at,
                    enabled: false,
                    expected_revision: Some(first.revision),
                },
                200,
            )
            .unwrap();
        assert!(fixture.active_todo_delivery_keys(&todo.id).is_empty());
    }

    #[test]
    fn snoozed_todo_delivery_is_cancelled_when_todo_is_completed() {
        let fixture = Fixture::at(100);
        let (todo, _) = saved_snoozed_reminder(&fixture, "snooze complete");
        fixture
            .todos
            .set_completed(
                crate::contracts::CompleteTodoInput {
                    id: todo.id.clone(),
                    completed: true,
                    expected_revision: todo.revision,
                },
                200,
            )
            .unwrap();
        fixture
            .projector
            .cancel(uuid::Uuid::parse_str(&todo.id).unwrap(), 200)
            .unwrap();
        assert!(fixture.active_todo_delivery_keys(&todo.id).is_empty());
    }

    #[test]
    fn snoozed_todo_delivery_is_cancelled_when_reminder_is_deleted() {
        let fixture = Fixture::at(100);
        let (todo, reminder) = saved_snoozed_reminder(&fixture, "snooze delete");
        fixture
            .projector
            .delete_and_cancel(
                uuid::Uuid::parse_str(&reminder.id).unwrap(),
                reminder.revision as u64,
                200,
            )
            .unwrap();
        assert!(fixture.active_todo_delivery_keys(&todo.id).is_empty());
    }

    #[test]
    fn restart_reconciliation_discovers_and_cancels_a_snoozed_orphan() {
        let first = Fixture::at(100);
        let (todo, _) = saved_snoozed_reminder(&first, "snooze orphan");
        first
            .todos
            .delete(
                uuid::Uuid::parse_str(&todo.id).unwrap(),
                todo.revision as u64,
            )
            .unwrap();
        assert_eq!(
            first.active_todo_delivery_keys(&todo.id).len(),
            1,
            "crash window retains the snoozed orphan before restart"
        );
        let path = first.path.clone();
        drop(first);

        let restarted = Fixture::open(path, 500);
        restarted.projector.reconcile(500).unwrap();
        assert!(restarted.active_todo_delivery_keys(&todo.id).is_empty());
    }

    #[test]
    fn concurrent_reminder_edit_cannot_leave_the_first_saved_revision_active() {
        let fixture = Fixture::at(100);
        let todo = fixture.todo("race edit", "", 100);
        let (blocked, barrier) = blocking_projector(&fixture);
        let todo_id = todo.id.clone();
        let first = std::thread::spawn(move || {
            blocked.save_and_project(
                SaveTodoReminderInput {
                    id: None,
                    todo_id,
                    remind_at: 1_000,
                    enabled: true,
                    expected_revision: None,
                },
                110,
            )
        });
        barrier.wait();
        let committed_first = fixture
            .reminders
            .get_for_todo(uuid::Uuid::parse_str(&todo.id).unwrap())
            .unwrap()
            .unwrap();
        let current = fixture
            .projector
            .save_and_project(
                SaveTodoReminderInput {
                    id: Some(committed_first.id.clone()),
                    todo_id: todo.id.clone(),
                    remind_at: 2_000,
                    enabled: true,
                    expected_revision: Some(committed_first.revision),
                },
                120,
            )
            .unwrap();
        barrier.wait();
        let _ = first.join().unwrap();
        assert_eq!(
            fixture.active_todo_delivery_keys(&todo.id),
            vec![format!("todo:{}:{}", todo.id, current.revision)]
        );
    }

    #[test]
    fn stale_projection_repairs_a_source_only_edit_to_the_current_revision() {
        let fixture = Fixture::at(100);
        let todo = fixture.todo("race source-only edit", "", 100);
        let first = fixture
            .projector
            .save_and_project(
                SaveTodoReminderInput {
                    id: None,
                    todo_id: todo.id.clone(),
                    remind_at: 1_000,
                    enabled: true,
                    expected_revision: None,
                },
                110,
            )
            .unwrap();
        let (blocked, barrier) = blocking_projector(&fixture);
        let stale = first.clone();
        let projection = std::thread::spawn(move || blocked.project(&stale, 120));
        barrier.wait();
        let current = fixture
            .reminders
            .save(
                SaveTodoReminderInput {
                    id: Some(first.id),
                    todo_id: todo.id.clone(),
                    remind_at: 2_000,
                    enabled: true,
                    expected_revision: Some(first.revision),
                },
                130,
            )
            .unwrap();
        barrier.wait();
        let error = projection.join().unwrap().unwrap_err();
        assert_eq!(error.code, AppErrorCode::Conflict);
        assert_eq!(
            fixture.active_todo_delivery_keys(&todo.id),
            vec![format!("todo:{}:{}", todo.id, current.revision)]
        );
    }

    #[test]
    fn stale_deleted_reminder_projection_preserves_the_recreated_canonical_delivery_without_wake() {
        let fixture = Fixture::at(100);
        let todo = fixture.todo("delete and recreate race", "", 100);
        let wake_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        fixture.scheduler.set_todo_projection_wake_observer({
            let wake_attempts = wake_attempts.clone();
            Arc::new(move || {
                wake_attempts.fetch_add(1, Ordering::AcqRel);
            })
        });
        let (blocked, barrier) = blocking_projector(&fixture);
        let todo_id = todo.id.clone();
        let stale = std::thread::spawn(move || {
            blocked.save_and_project(
                SaveTodoReminderInput {
                    id: None,
                    todo_id,
                    remind_at: 1_000,
                    enabled: true,
                    expected_revision: None,
                },
                110,
            )
        });
        barrier.wait();
        let reminder_a = fixture
            .reminders
            .get_for_todo(uuid::Uuid::parse_str(&todo.id).unwrap())
            .unwrap()
            .unwrap();
        fixture
            .reminders
            .delete(
                uuid::Uuid::parse_str(&reminder_a.id).unwrap(),
                reminder_a.revision as u64,
            )
            .unwrap();
        let reminder_b = fixture
            .projector
            .save_and_project(
                SaveTodoReminderInput {
                    id: None,
                    todo_id: todo.id.clone(),
                    remind_at: 2_000,
                    enabled: true,
                    expected_revision: None,
                },
                120,
            )
            .unwrap();
        assert_eq!(wake_attempts.load(Ordering::Acquire), 1);
        barrier.wait();
        let error = stale.join().unwrap().unwrap_err();
        assert_eq!(error.code, AppErrorCode::Conflict);
        assert!(error.retryable);
        assert_eq!(
            fixture.active_todo_delivery_keys(&todo.id),
            vec![format!("todo:{}:{}", todo.id, reminder_b.revision)]
        );
        assert_eq!(wake_attempts.load(Ordering::Acquire), 1);
    }

    #[test]
    fn invalid_current_title_cancels_old_pending_and_snoozed_revisions_across_restart() {
        for active_state in ["pending", "snoozed"] {
            let fixture = Fixture::at(100);
            let todo = fixture.todo(&format!("valid {active_state}"), "", 100);
            let first = fixture
                .projector
                .save_and_project(
                    SaveTodoReminderInput {
                        id: None,
                        todo_id: todo.id.clone(),
                        remind_at: 1_000,
                        enabled: true,
                        expected_revision: None,
                    },
                    110,
                )
                .unwrap();
            if active_state == "snoozed" {
                snooze_todo_delivery(&fixture, &first);
            }
            fixture
                .todos
                .update(
                    UpdateTodoInput {
                        id: todo.id.clone(),
                        title: "invalid\u{0007}reminder title".into(),
                        description: String::new(),
                        due_at: None,
                        priority: TodoPriority::Normal,
                        expected_revision: todo.revision,
                    },
                    120,
                )
                .unwrap();
            let error = fixture
                .projector
                .save_and_project(
                    SaveTodoReminderInput {
                        id: Some(first.id.clone()),
                        todo_id: todo.id.clone(),
                        remind_at: 2_000,
                        enabled: true,
                        expected_revision: Some(first.revision),
                    },
                    130,
                )
                .unwrap_err();
            assert_eq!(error.code, AppErrorCode::InvalidInput);
            assert_eq!(
                fixture
                    .reminders
                    .get_for_todo(uuid::Uuid::parse_str(&todo.id).unwrap())
                    .unwrap()
                    .unwrap()
                    .revision,
                first.revision + 1,
                "source revision remains committed for {active_state}"
            );
            assert!(
                fixture.active_todo_delivery_keys(&todo.id).is_empty(),
                "old {active_state} delivery must be durably cancelled"
            );
            let path = fixture.path.clone();
            drop(fixture);

            let restarted = Fixture::open(path, 20_000);
            let restart_error = restarted.projector.reconcile(20_000).unwrap_err();
            assert_eq!(restart_error.code, AppErrorCode::InvalidInput);
            assert!(restarted.active_todo_delivery_keys(&todo.id).is_empty());
            assert!(
                ReminderRepository::new(restarted.storage.clone())
                    .claim_due(20_000, 10)
                    .unwrap()
                    .is_empty(),
                "restart must not dispatch revision 1 from {active_state}"
            );
        }
    }

    #[tokio::test]
    async fn recreated_reminder_advances_past_cancelled_history_and_dispatches_after_restart() {
        for retained_state in ["pending", "snoozed"] {
            let fixture = Fixture::at(100);
            let todo = fixture.todo(&format!("recreate after {retained_state}"), "", 100);
            let first = fixture
                .projector
                .save_and_project(
                    SaveTodoReminderInput {
                        id: None,
                        todo_id: todo.id.clone(),
                        remind_at: 500,
                        enabled: true,
                        expected_revision: None,
                    },
                    110,
                )
                .unwrap();
            assert_eq!(first.revision, 1);
            if retained_state == "snoozed" {
                snooze_todo_delivery(&fixture, &first);
            }
            fixture
                .projector
                .delete_and_cancel(
                    uuid::Uuid::parse_str(&first.id).unwrap(),
                    first.revision as u64,
                    600,
                )
                .unwrap();
            let first_key = format!("todo:{}:{}", todo.id, first.revision);
            assert_eq!(fixture.delivery(&first_key).unwrap().state, "cancelled");

            let recreated = fixture
                .projector
                .save_and_project(
                    SaveTodoReminderInput {
                        id: None,
                        todo_id: todo.id.clone(),
                        remind_at: 800,
                        enabled: true,
                        expected_revision: None,
                    },
                    700,
                )
                .unwrap();
            let recreated_key = format!("todo:{}:{}", todo.id, recreated.revision);
            assert_eq!(recreated.revision, 2);
            assert!(recreated.revision > first.revision);
            assert_ne!(recreated_key, first_key);
            assert_eq!(fixture.delivery(&first_key).unwrap().state, "cancelled");
            assert_eq!(fixture.delivery(&recreated_key).unwrap().state, "pending");
            assert_eq!(fixture.delivery_count(), 2);
            let path = fixture.path.clone();
            drop(fixture);

            let mut restarted = Fixture::open(path, 1_000);
            restarted.projector.reconcile(1_000).unwrap();
            assert_eq!(restarted.delivery_count(), 2);
            assert_eq!(
                restarted.active_todo_delivery_keys(&todo.id),
                vec![recreated_key.clone()]
            );
            let worker = restarted.worker.take().unwrap();
            let (shutdown_tx, shutdown) = tokio::sync::watch::channel(false);
            let task = tokio::spawn(worker.run(shutdown));
            let (event_name, payload) =
                tokio::time::timeout(std::time::Duration::from_secs(1), restarted.emitter.next())
                    .await
                    .unwrap();
            assert_eq!(event_name, crate::events::REMINDER_DISPATCH_READY);
            assert!(payload["dispatchSeq"].as_i64().unwrap() > 0);
            assert_eq!(
                restarted.delivery(&recreated_key).unwrap().state,
                "dispatched"
            );
            assert_eq!(restarted.delivery_count(), 2);
            shutdown_tx.send_replace(true);
            task.await.unwrap();
        }
    }

    #[test]
    fn concurrent_todo_completion_cannot_leave_the_first_saved_revision_active() {
        let fixture = Fixture::at(100);
        let todo = fixture.todo("race complete", "", 100);
        let (blocked, barrier) = blocking_projector(&fixture);
        let todo_id = todo.id.clone();
        let first = std::thread::spawn(move || {
            blocked.save_and_project(
                SaveTodoReminderInput {
                    id: None,
                    todo_id,
                    remind_at: 1_000,
                    enabled: true,
                    expected_revision: None,
                },
                110,
            )
        });
        barrier.wait();
        fixture
            .todos
            .set_completed(
                crate::contracts::CompleteTodoInput {
                    id: todo.id.clone(),
                    completed: true,
                    expected_revision: todo.revision,
                },
                120,
            )
            .unwrap();
        fixture
            .projector
            .cancel(uuid::Uuid::parse_str(&todo.id).unwrap(), 120)
            .unwrap();
        barrier.wait();
        let _ = first.join().unwrap();
        assert!(fixture.active_todo_delivery_keys(&todo.id).is_empty());
    }

    #[test]
    fn concurrent_todo_delete_cannot_insert_an_orphan_first_saved_revision() {
        let fixture = Fixture::at(100);
        let todo = fixture.todo("race delete", "", 100);
        let (blocked, barrier) = blocking_projector(&fixture);
        let todo_id = todo.id.clone();
        let first = std::thread::spawn(move || {
            blocked.save_and_project(
                SaveTodoReminderInput {
                    id: None,
                    todo_id,
                    remind_at: 1_000,
                    enabled: true,
                    expected_revision: None,
                },
                110,
            )
        });
        barrier.wait();
        fixture
            .todos
            .delete(
                uuid::Uuid::parse_str(&todo.id).unwrap(),
                todo.revision as u64,
            )
            .unwrap();
        fixture
            .projector
            .cancel(uuid::Uuid::parse_str(&todo.id).unwrap(), 120)
            .unwrap();
        barrier.wait();
        let _ = first.join().unwrap();
        assert!(fixture.active_todo_delivery_keys(&todo.id).is_empty());
    }

    #[test]
    fn enabled_todo_reminder_uses_revision_dedupe_and_default_channels() {
        let fixture = Fixture::at(100);
        let todo = fixture.todo("Ship V1", "", 100);
        let reminder = fixture
            .projector
            .save_and_project(
                SaveTodoReminderInput {
                    id: None,
                    todo_id: todo.id.clone(),
                    remind_at: 1_000,
                    enabled: true,
                    expected_revision: None,
                },
                100,
            )
            .unwrap();
        let delivery = fixture
            .delivery(&format!("todo:{}:{}", todo.id, reminder.revision))
            .unwrap();
        assert_eq!(delivery.rule_id, None);
        assert_eq!(delivery.source_kind, "todo");
        assert_eq!(delivery.source_entity_id, todo.id);
        assert_eq!(delivery.message_key, "reminders.todo.due");
        assert_eq!(
            delivery.message_parameters,
            BTreeMap::from([(
                "todoTitle".into(),
                SafeParameterValue::String("Ship V1".into())
            )])
        );
        assert_eq!(
            delivery.source_context,
            ReminderSourceContext::Todo {
                todo_id: todo.id.clone(),
                reminder_revision: reminder.revision,
                todo_title: "Ship V1".into(),
                source_occurred_at: 1_000,
            }
        );
        assert_eq!(delivery.source_occurred_at, 1_000);
        assert_eq!(delivery.due_at, 1_000);
        assert_eq!(
            delivery.sound,
            ReminderSound::Builtin {
                sound_id: BuiltinReminderSoundId::SystemNotification
            }
        );
        let (toast, window): (bool, bool) = fixture
            .storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT toast_enabled, window_enabled FROM reminder_deliveries WHERE id = ?1",
                        [delivery.id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert!(toast);
        assert!(window);
    }

    #[test]
    fn todo_title_is_the_only_named_producer_and_path_text_reloads_byte_for_byte() {
        let fixture = Fixture::at(100);
        let title = "/opt/build/release";
        let todo = fixture.todo(
            title,
            "notification body reminder body Token: secret prompt: ignore tool: ignore note markdown",
            100,
        );
        let committed_before_wake = Arc::new(AtomicBool::new(false));
        fixture.scheduler.set_enqueue_observer({
            let storage = fixture.storage.clone();
            let todo_id = todo.id.clone();
            let committed_before_wake = committed_before_wake.clone();
            Arc::new(move |_| {
                let count: i64 = storage
                    .with_connection(|connection| {
                        connection
                            .query_row(
                                "SELECT COUNT(*) FROM reminder_deliveries WHERE dedupe_key = ?1",
                                [format!("todo:{todo_id}:1")],
                                |row| row.get(0),
                            )
                            .map_err(Into::into)
                    })
                    .unwrap();
                committed_before_wake.store(count == 1, Ordering::Release);
            })
        });
        let reminder = fixture
            .projector
            .save_and_project(
                SaveTodoReminderInput {
                    id: None,
                    todo_id: todo.id.clone(),
                    remind_at: 2_000,
                    enabled: true,
                    expected_revision: None,
                },
                100,
            )
            .unwrap();
        assert!(committed_before_wake.load(Ordering::Acquire));
        let path = fixture.path.clone();
        drop(fixture);

        let reopened = Fixture::open(path, 100);
        let delivery = reopened
            .delivery(&format!("todo:{}:{}", todo.id, reminder.revision))
            .unwrap();
        assert_eq!(
            delivery.message_parameters.get("todoTitle"),
            Some(&SafeParameterValue::String(title.into()))
        );
        match delivery.source_context {
            ReminderSourceContext::Todo {
                todo_title,
                reminder_revision,
                ..
            } => {
                assert_eq!(todo_title.as_bytes(), title.as_bytes());
                assert_eq!(reminder_revision, reminder.revision);
            }
            _ => panic!("Todo projection must persist only Todo source context"),
        }
    }

    #[test]
    fn same_revision_is_idempotent_and_edit_toggle_cancel_prior_pending_rows() {
        let fixture = Fixture::at(100);
        let todo = fixture.todo("Revise", "", 100);
        let first = fixture
            .projector
            .save_and_project(
                SaveTodoReminderInput {
                    id: None,
                    todo_id: todo.id.clone(),
                    remind_at: 1_000,
                    enabled: true,
                    expected_revision: None,
                },
                100,
            )
            .unwrap();
        assert!(matches!(
            fixture.projector.project(&first, 101).unwrap(),
            EnqueueOutcome::Duplicate(_)
        ));
        assert_eq!(fixture.delivery_count(), 1);

        let second = fixture
            .projector
            .save_and_project(
                SaveTodoReminderInput {
                    id: Some(first.id.clone()),
                    todo_id: todo.id.clone(),
                    remind_at: 2_000,
                    enabled: true,
                    expected_revision: Some(first.revision),
                },
                200,
            )
            .unwrap();
        assert_eq!(second.revision, first.revision + 1);
        assert_eq!(
            fixture
                .delivery(&format!("todo:{}:{}", todo.id, first.revision))
                .unwrap()
                .state,
            "cancelled"
        );
        assert_eq!(
            fixture
                .delivery(&format!("todo:{}:{}", todo.id, second.revision))
                .unwrap()
                .state,
            "pending"
        );

        let disabled = fixture
            .projector
            .save_and_project(
                SaveTodoReminderInput {
                    id: Some(second.id.clone()),
                    todo_id: todo.id,
                    remind_at: 2_000,
                    enabled: false,
                    expected_revision: Some(second.revision),
                },
                300,
            )
            .unwrap();
        assert_eq!(disabled.revision, second.revision + 1);
        assert_eq!(fixture.delivery_count(), 2);
        assert_eq!(
            fixture
                .delivery(&format!("todo:{}:{}", disabled.todo_id, second.revision))
                .unwrap()
                .state,
            "cancelled"
        );
    }

    #[test]
    fn committed_source_row_is_reconciled_idempotently_after_restart() {
        let first = Fixture::at(100);
        let todo = first.todo("Recover", "", 100);
        let reminder = first
            .reminders
            .save(
                SaveTodoReminderInput {
                    id: None,
                    todo_id: todo.id.clone(),
                    remind_at: 1_000,
                    enabled: true,
                    expected_revision: None,
                },
                100,
            )
            .unwrap();
        assert_eq!(first.delivery_count(), 0);
        let path = first.path.clone();
        drop(first);

        let restarted = Fixture::open(path, 500);
        assert_eq!(
            restarted.projector.reconcile(500).unwrap(),
            ReminderReconcileResult {
                enqueued: 1,
                cancelled: 0
            }
        );
        restarted.projector.reconcile(500).unwrap();
        assert_eq!(restarted.delivery_count(), 1);
        assert_eq!(
            restarted
                .delivery(&format!("todo:{}:{}", todo.id, reminder.revision))
                .unwrap()
                .state,
            "pending"
        );
    }

    #[tokio::test]
    async fn overdue_enabled_reminder_dispatches_after_restart_with_monotonic_sequence() {
        let mut fixture = Fixture::at(1_000);
        let todo = fixture.todo("Overdue", "", 100);
        fixture
            .reminders
            .save(
                SaveTodoReminderInput {
                    id: None,
                    todo_id: todo.id,
                    remind_at: 500,
                    enabled: true,
                    expected_revision: None,
                },
                100,
            )
            .unwrap();
        fixture.projector.reconcile(1_000).unwrap();
        let worker = fixture.worker.take().unwrap();
        let (shutdown_tx, shutdown) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(worker.run(shutdown));
        let (event_name, payload) =
            tokio::time::timeout(std::time::Duration::from_secs(1), fixture.emitter.next())
                .await
                .unwrap();
        assert_eq!(event_name, crate::events::REMINDER_DISPATCH_READY);
        assert_eq!(payload["dispatchSeq"], 1);
        let replay = ReminderRepository::new(fixture.storage.clone())
            .replay("todo-restart", 0, 10)
            .unwrap();
        assert_eq!(replay.deliveries.len(), 1);
        assert_eq!(
            replay.deliveries[0].state,
            ReminderDeliveryState::Dispatched
        );
        assert_eq!(replay.deliveries[0].due_at, 500);
        shutdown_tx.send_replace(true);
        task.await.unwrap();
    }

    #[test]
    fn nullable_list_is_deterministic_and_delete_retains_delivery_history() {
        let fixture = Fixture::at(100);
        let todo_a = fixture.todo("A", "", 100);
        let todo_b = fixture.todo("B", "", 100);
        let a = fixture
            .projector
            .save_and_project(
                SaveTodoReminderInput {
                    id: None,
                    todo_id: todo_a.id.clone(),
                    remind_at: 1_000,
                    enabled: true,
                    expected_revision: None,
                },
                100,
            )
            .unwrap();
        let b = fixture
            .projector
            .save_and_project(
                SaveTodoReminderInput {
                    id: None,
                    todo_id: todo_b.id,
                    remind_at: 2_000,
                    enabled: true,
                    expected_revision: None,
                },
                200,
            )
            .unwrap();
        assert_eq!(
            fixture.reminders.list(None).unwrap(),
            vec![b.clone(), a.clone()]
        );
        assert_eq!(
            fixture
                .reminders
                .list(Some(uuid::Uuid::parse_str(&a.todo_id).unwrap()))
                .unwrap(),
            vec![a.clone()]
        );
        fixture
            .projector
            .delete_and_cancel(
                uuid::Uuid::parse_str(&a.id).unwrap(),
                a.revision as u64,
                300,
            )
            .unwrap();
        assert_eq!(
            fixture
                .reminders
                .get_for_todo(uuid::Uuid::parse_str(&a.todo_id).unwrap())
                .unwrap(),
            None
        );
        assert_eq!(fixture.delivery_count(), 2);
        assert_eq!(
            fixture
                .delivery(&format!("todo:{}:{}", a.todo_id, a.revision))
                .unwrap()
                .state,
            "cancelled"
        );
    }
}
