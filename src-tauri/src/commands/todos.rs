use crate::contracts::{
    AppErrorCode, CommandError, CompleteTodoInput, CreateTodoInput, DeleteResult, DiagnosticEvent,
    DiagnosticLevel, SafeMessageParameters, SafeParameterValue, SaveTodoReminderInput, TodoItem,
    TodoPriority, TodoReminder, TodoStatusFilter, UpdateTodoInput,
};
use crate::domain::todo::TodoListFilter;
use crate::services::AppServices;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

#[tauri::command(rename = "listTodos", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn listTodos(
    status: TodoStatusFilter,
    limit: i64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<Vec<TodoItem>, CommandError> {
    let limit = u32::try_from(limit).map_err(|_| invalid_input())?;
    let status = match status {
        TodoStatusFilter::All => TodoListFilter::All,
        TodoStatusFilter::Open => TodoListFilter::Open,
        TodoStatusFilter::Completed => TodoListFilter::Completed,
    };
    services.todos.list(status, limit)
}

#[tauri::command(rename = "createTodo", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn createTodo(
    title: String,
    description: String,
    due_at: Option<i64>,
    priority: TodoPriority,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<TodoItem, CommandError> {
    create_todo_with_services(
        CreateTodoInput {
            title,
            description,
            due_at,
            priority,
        },
        services.inner().as_ref(),
        now_millis(),
    )
}

fn create_todo_with_services(
    input: CreateTodoInput,
    services: &AppServices,
    now: i64,
) -> Result<TodoItem, CommandError> {
    let todo = services.todos.create(input, now)?;
    emit_or_record(services, &todo.id, todo.revision, now);
    Ok(todo)
}

#[tauri::command(rename = "updateTodo", rename_all = "camelCase")]
#[allow(non_snake_case, clippy::too_many_arguments)]
pub fn updateTodo(
    id: Uuid,
    title: String,
    description: String,
    due_at: Option<i64>,
    priority: TodoPriority,
    expected_revision: u64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<TodoItem, CommandError> {
    let expected_revision = i64::try_from(expected_revision).map_err(|_| invalid_input())?;
    update_todo_with_services(
        UpdateTodoInput {
            id: id.to_string(),
            title,
            description,
            due_at,
            priority,
            expected_revision,
        },
        services.inner().as_ref(),
        now_millis(),
    )
}

fn update_todo_with_services(
    input: UpdateTodoInput,
    services: &AppServices,
    now: i64,
) -> Result<TodoItem, CommandError> {
    let todo = services.todos.update(input, now)?;
    emit_or_record(services, &todo.id, todo.revision, now);
    Ok(todo)
}

#[tauri::command(rename = "completeTodo", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn completeTodo(
    id: Uuid,
    completed: bool,
    expected_revision: u64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<TodoItem, CommandError> {
    let expected_revision = i64::try_from(expected_revision).map_err(|_| invalid_input())?;
    complete_todo_with_services(
        CompleteTodoInput {
            id: id.to_string(),
            completed,
            expected_revision,
        },
        services.inner().as_ref(),
        now_millis(),
    )
}

fn complete_todo_with_services(
    input: CompleteTodoInput,
    services: &AppServices,
    now: i64,
) -> Result<TodoItem, CommandError> {
    let todo = services.todos.set_completed(input, now)?;
    emit_or_record(services, &todo.id, todo.revision, now);
    if let Some(reminder) = services
        .todo_reminders
        .get_for_todo(Uuid::parse_str(&todo.id).map_err(|_| invalid_input())?)?
    {
        let projected = if todo.completed_at.is_some() {
            services
                .todo_reminder_projector
                .cancel(Uuid::parse_str(&todo.id).map_err(|_| invalid_input())?, now)
                .map(|_| ())
        } else if reminder.enabled {
            services
                .todo_reminder_projector
                .project(&reminder, now)
                .map(|_| ())
        } else {
            Ok(())
        };
        if let Err(error) = projected {
            record_projection_failure(services, &reminder.todo_id, &reminder.id, now);
            return Err(error);
        }
    }
    Ok(todo)
}

#[tauri::command(rename = "deleteTodo", rename_all = "camelCase")]
#[allow(non_snake_case)]
pub fn deleteTodo(
    id: Uuid,
    expected_revision: u64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<DeleteResult, CommandError> {
    delete_todo_with_services(
        id,
        expected_revision,
        services.inner().as_ref(),
        now_millis(),
    )
}

fn delete_todo_with_services(
    id: Uuid,
    expected_revision: u64,
    services: &AppServices,
    changed_at: i64,
) -> Result<DeleteResult, CommandError> {
    let revision = expected_revision
        .checked_add(1)
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(invalid_input)?;
    let result = services.todos.delete(id, expected_revision)?;
    emit_or_record(services, &result.id, revision, changed_at);
    services.todo_reminder_projector.cancel(id, changed_at)?;
    Ok(result)
}

#[tauri::command(rename = "listTodoReminders", rename_all = "camelCase")]
pub fn list_todo_reminders(
    todo_id: Option<Uuid>,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<Vec<TodoReminder>, CommandError> {
    services.todo_reminders.list(todo_id)
}

#[tauri::command(rename = "saveTodoReminder", rename_all = "camelCase")]
pub fn save_todo_reminder(
    id: Option<Uuid>,
    todo_id: Uuid,
    remind_at: i64,
    enabled: bool,
    expected_revision: Option<u64>,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<TodoReminder, CommandError> {
    let expected_revision = expected_revision
        .map(|value| i64::try_from(value).map_err(|_| invalid_input()))
        .transpose()?;
    save_todo_reminder_with_services(
        SaveTodoReminderInput {
            id: id.map(|value| value.to_string()),
            todo_id: todo_id.to_string(),
            remind_at,
            enabled,
            expected_revision,
        },
        services.inner().as_ref(),
        now_millis(),
    )
}

fn save_todo_reminder_with_services(
    input: SaveTodoReminderInput,
    services: &AppServices,
    now: i64,
) -> Result<TodoReminder, CommandError> {
    let todo_id = Uuid::parse_str(&input.todo_id).map_err(|_| invalid_input())?;
    let before = services.todo_reminders.get_for_todo(todo_id)?;
    match services
        .todo_reminder_projector
        .save_and_project(input, now)
    {
        Ok(reminder) => {
            emit_or_record(services, &reminder.todo_id, reminder.revision, now);
            Ok(reminder)
        }
        Err(error) => {
            if let Ok(Some(after)) = services.todo_reminders.get_for_todo(todo_id) {
                if before.as_ref() != Some(&after) {
                    record_projection_failure(services, &after.todo_id, &after.id, now);
                }
            }
            Err(error)
        }
    }
}

#[tauri::command(rename = "deleteTodoReminder", rename_all = "camelCase")]
pub fn delete_todo_reminder(
    id: Uuid,
    expected_revision: u64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<DeleteResult, CommandError> {
    delete_todo_reminder_with_services(
        id,
        expected_revision,
        services.inner().as_ref(),
        now_millis(),
    )
}

fn delete_todo_reminder_with_services(
    id: Uuid,
    expected_revision: u64,
    services: &AppServices,
    now: i64,
) -> Result<DeleteResult, CommandError> {
    let reminder = services
        .todo_reminders
        .list(None)?
        .into_iter()
        .find(|reminder| reminder.id == id.to_string())
        .ok_or_else(|| CommandError {
            code: AppErrorCode::NotFound,
            message_key: "errors.notFound".into(),
            details: SafeMessageParameters::new(),
            retryable: false,
        })?;
    let revision = expected_revision.checked_add(1).ok_or_else(invalid_input)?;
    let result = services.todo_reminders.delete(id, expected_revision)?;
    emit_or_record(services, &reminder.todo_id, revision as i64, now);
    services.todo_reminder_projector.cancel(
        Uuid::parse_str(&reminder.todo_id).map_err(|_| invalid_input())?,
        now,
    )?;
    Ok(result)
}

fn record_projection_failure(services: &AppServices, todo_id: &str, reminder_id: &str, now: i64) {
    let _ = services.diagnostics.record(&DiagnosticEvent {
        id: Uuid::new_v4().to_string(),
        service_id: "todo".into(),
        level: DiagnosticLevel::Failure,
        code: "todo.reminderProjectionFailed".into(),
        parameters: BTreeMap::from([
            ("todoId".into(), SafeParameterValue::String(todo_id.into())),
            (
                "reminderId".into(),
                SafeParameterValue::String(reminder_id.into()),
            ),
        ]),
        created_at: now,
    });
}

fn emit_or_record(services: &AppServices, entity_id: &str, revision: i64, changed_at: i64) {
    let Ok(revision) = u64::try_from(revision) else {
        return;
    };
    if services
        .emit_todo_changed(entity_id, revision, changed_at)
        .is_err()
    {
        let _ = services.diagnostics.record(&DiagnosticEvent {
            id: Uuid::new_v4().to_string(),
            service_id: "todo".into(),
            level: DiagnosticLevel::Failure,
            code: "events.todoChangedEmitFailed".into(),
            parameters: BTreeMap::from([(
                "entityId".into(),
                SafeParameterValue::String(entity_id.into()),
            )]),
            created_at: changed_at,
        });
    }
}

fn invalid_input() -> CommandError {
    CommandError {
        code: AppErrorCode::InvalidInput,
        message_key: "errors.invalidInput".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
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
        completeTodo, complete_todo_with_services, createTodo, create_todo_with_services,
        deleteTodo, delete_todo_reminder, delete_todo_reminder_with_services,
        delete_todo_with_services, listTodos, list_todo_reminders, save_todo_reminder,
        save_todo_reminder_with_services, updateTodo, update_todo_with_services,
    };
    use crate::contracts::{
        AppErrorCode, CommandError, CompleteTodoInput, CreateTodoInput, SafeMessageParameters,
        SaveTodoReminderInput, TodoPriority, UpdateTodoInput,
    };
    use crate::events::TO_DO_CHANGED;
    use crate::services::{
        AppServices, BootstrapModuleStateProvider, EventEmitterPort, ModuleStateProvider,
        ShutdownPort, WalCheckpointPort,
    };
    use crate::storage::Storage;
    use std::sync::{Arc, Mutex};

    #[test]
    fn exports_the_five_exact_camel_case_todo_commands() {
        let _ = listTodos;
        let _ = createTodo;
        let _ = updateTodo;
        let _ = completeTodo;
        let _ = deleteTodo;
        assert_eq!(
            crate::commands::TODO_COMMAND_NAMES.as_slice(),
            &[
                "listTodos",
                "createTodo",
                "updateTodo",
                "completeTodo",
                "deleteTodo",
            ]
        );
    }

    #[test]
    fn todo_reminder_commands_are_registered_without_changing_the_task_two_crud_manifest() {
        let _ = list_todo_reminders;
        let _ = save_todo_reminder;
        let _ = delete_todo_reminder;
        assert_eq!(
            crate::commands::TODO_COMMAND_NAMES.as_slice(),
            &[
                "listTodos",
                "createTodo",
                "updateTodo",
                "completeTodo",
                "deleteTodo",
            ]
        );
    }

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

    #[test]
    fn todo_completion_reopen_and_delete_apply_the_locked_reminder_cancellation_matrix() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let services = AppServices::from_parts(
            storage.clone(),
            Arc::new(BootstrapModuleStateProvider) as Arc<dyn ModuleStateProvider>,
            Arc::new(NoopShutdown),
            Arc::new(NoopCheckpoint),
            Arc::new(NoopEmitter),
        );
        let todo = create_todo_with_services(
            CreateTodoInput {
                title: "Lifecycle".into(),
                description: String::new(),
                due_at: None,
                priority: TodoPriority::Normal,
            },
            &services,
            10,
        )
        .unwrap();
        let reminder = save_todo_reminder_with_services(
            SaveTodoReminderInput {
                id: None,
                todo_id: todo.id.clone(),
                remind_at: 5_000,
                enabled: true,
                expected_revision: None,
            },
            &services,
            20,
        )
        .unwrap();

        complete_todo_with_services(
            CompleteTodoInput {
                id: todo.id.clone(),
                completed: true,
                expected_revision: 1,
            },
            &services,
            30,
        )
        .unwrap();
        let after_completion = services
            .todo_reminders
            .get_for_todo(uuid::Uuid::parse_str(&todo.id).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(after_completion.revision, 2);
        let first_state: String = storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT state FROM reminder_deliveries WHERE dedupe_key = ?1",
                        [format!("todo:{}:{}", todo.id, reminder.revision)],
                        |row| row.get(0),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(first_state, "cancelled");

        complete_todo_with_services(
            CompleteTodoInput {
                id: todo.id.clone(),
                completed: false,
                expected_revision: 2,
            },
            &services,
            40,
        )
        .unwrap();
        let reopened = services
            .todo_reminders
            .get_for_todo(uuid::Uuid::parse_str(&todo.id).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(reopened.revision, 3);
        let reopened_due_and_state: (i64, String) = storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT due_at, state FROM reminder_deliveries WHERE dedupe_key = ?1",
                        [format!("todo:{}:{}", todo.id, reopened.revision)],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(reopened_due_and_state, (5_000, "pending".into()));

        delete_todo_with_services(uuid::Uuid::parse_str(&todo.id).unwrap(), 3, &services, 50)
            .unwrap();
        assert_eq!(
            services
                .todo_reminders
                .get_for_todo(uuid::Uuid::parse_str(&todo.id).unwrap())
                .unwrap(),
            None
        );
        let history: Vec<String> = storage
            .with_connection(|connection| {
                let mut statement = connection.prepare(
                    "SELECT state FROM reminder_deliveries WHERE source_kind = 'todo' AND source_entity_id = ?1 ORDER BY created_at",
                )?;
                let rows = statement
                    .query_map([todo.id], |row| row.get(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .unwrap();
        assert_eq!(history, vec!["cancelled", "cancelled"]);
    }
    struct CommitCheckingEmitter {
        storage: Arc<Storage>,
        observations: Arc<Mutex<Vec<(String, serde_json::Value, i64, Option<i64>)>>>,
    }
    impl EventEmitterPort for CommitCheckingEmitter {
        fn emit(
            &self,
            event_name: &'static str,
            payload: serde_json::Value,
        ) -> Result<(), crate::contracts::CommandError> {
            let id = payload["entityId"].as_str().unwrap();
            let (revision, completed_at) = self.storage.with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT revision, completed_at FROM todos WHERE id = ?1",
                        [id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(Into::into)
            })?;
            self.observations.lock().unwrap().push((
                event_name.into(),
                payload,
                revision,
                completed_at,
            ));
            Ok(())
        }
    }

    #[test]
    fn mutations_commit_before_the_typed_wake_hint_is_emitted() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let observations = Arc::new(Mutex::new(Vec::new()));
        let services = AppServices::from_parts(
            storage.clone(),
            Arc::new(BootstrapModuleStateProvider) as Arc<dyn ModuleStateProvider>,
            Arc::new(NoopShutdown),
            Arc::new(NoopCheckpoint),
            Arc::new(CommitCheckingEmitter {
                storage,
                observations: observations.clone(),
            }),
        );
        let created = create_todo_with_services(
            CreateTodoInput {
                title: "Ship".into(),
                description: String::new(),
                due_at: None,
                priority: TodoPriority::High,
            },
            &services,
            10,
        )
        .unwrap();
        let updated = update_todo_with_services(
            UpdateTodoInput {
                id: created.id.clone(),
                title: "Ship now".into(),
                description: String::new(),
                due_at: None,
                priority: TodoPriority::High,
                expected_revision: 1,
            },
            &services,
            20,
        )
        .unwrap();
        let completed = complete_todo_with_services(
            CompleteTodoInput {
                id: created.id.clone(),
                completed: true,
                expected_revision: 2,
            },
            &services,
            42,
        )
        .unwrap();
        assert_eq!(completed.completed_at, Some(42));
        assert_eq!(
            *observations.lock().unwrap(),
            vec![
                (
                    TO_DO_CHANGED.into(),
                    serde_json::json!({ "entityId": created.id, "revision": 1, "changedAt": 10 }),
                    1,
                    None
                ),
                (
                    TO_DO_CHANGED.into(),
                    serde_json::json!({ "entityId": updated.id, "revision": 2, "changedAt": 20 }),
                    2,
                    None
                ),
                (
                    TO_DO_CHANGED.into(),
                    serde_json::json!({ "entityId": completed.id, "revision": 3, "changedAt": 42 }),
                    3,
                    Some(42)
                ),
            ]
        );
    }

    struct FailingEmitter;
    impl EventEmitterPort for FailingEmitter {
        fn emit(&self, _: &'static str, _: serde_json::Value) -> Result<(), CommandError> {
            Err(CommandError {
                code: AppErrorCode::SourceUnavailable,
                message_key: "errors.sourceUnavailable".into(),
                details: SafeMessageParameters::new(),
                retryable: false,
            })
        }
    }

    #[test]
    fn emit_failure_keeps_the_commit_and_records_only_the_entity_id() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let services = AppServices::from_parts(
            storage,
            Arc::new(BootstrapModuleStateProvider) as Arc<dyn ModuleStateProvider>,
            Arc::new(NoopShutdown),
            Arc::new(NoopCheckpoint),
            Arc::new(FailingEmitter),
        );
        let created = create_todo_with_services(
            CreateTodoInput {
                title: "Durable".into(),
                description: String::new(),
                due_at: None,
                priority: TodoPriority::Normal,
            },
            &services,
            42,
        )
        .unwrap();
        assert_eq!(
            services
                .todos
                .get(uuid::Uuid::parse_str(&created.id).unwrap())
                .unwrap(),
            created
        );
        let events = services.diagnostics.list(1).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].code, "events.todoChangedEmitFailed");
        assert_eq!(
            events[0].parameters,
            std::collections::BTreeMap::from([(
                "entityId".into(),
                crate::contracts::SafeParameterValue::String(created.id),
            )])
        );
    }

    #[test]
    fn reminder_emit_failure_keeps_source_and_delivery_committed_with_only_entity_id() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let services = AppServices::from_parts(
            storage.clone(),
            Arc::new(BootstrapModuleStateProvider) as Arc<dyn ModuleStateProvider>,
            Arc::new(NoopShutdown),
            Arc::new(NoopCheckpoint),
            Arc::new(FailingEmitter),
        );
        let todo = services
            .todos
            .create(
                CreateTodoInput {
                    title: "durable reminder".into(),
                    description: String::new(),
                    due_at: None,
                    priority: TodoPriority::Normal,
                },
                10,
            )
            .unwrap();
        let reminder = save_todo_reminder_with_services(
            SaveTodoReminderInput {
                id: None,
                todo_id: todo.id.clone(),
                remind_at: 1_000,
                enabled: true,
                expected_revision: None,
            },
            &services,
            20,
        )
        .unwrap();
        assert_eq!(
            services
                .todo_reminders
                .get_for_todo(uuid::Uuid::parse_str(&todo.id).unwrap())
                .unwrap(),
            Some(reminder)
        );
        let delivery_count: i64 = storage
            .with_connection(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM reminder_deliveries", [], |row| {
                        row.get(0)
                    })
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(delivery_count, 1);
        let events = services.diagnostics.list(10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].code, "events.todoChangedEmitFailed");
        assert_eq!(
            events[0].parameters,
            std::collections::BTreeMap::from([(
                "entityId".into(),
                crate::contracts::SafeParameterValue::String(todo.id),
            )])
        );
    }

    struct ReminderDeleteCheckingEmitter {
        storage: Arc<Storage>,
        reminder_id: String,
        observations: Arc<Mutex<Vec<(serde_json::Value, bool, String)>>>,
    }
    impl EventEmitterPort for ReminderDeleteCheckingEmitter {
        fn emit(&self, _: &'static str, payload: serde_json::Value) -> Result<(), CommandError> {
            let source_exists = self.storage.with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM todo_reminders WHERE id = ?1)",
                        [&self.reminder_id],
                        |row| row.get(0),
                    )
                    .map_err(Into::into)
            })?;
            let delivery_state = self.storage.with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT state FROM reminder_deliveries WHERE source_kind = 'todo' AND source_entity_id = ?1",
                        [payload["entityId"].as_str().unwrap()],
                        |row| row.get(0),
                    )
                    .map_err(Into::into)
            })?;
            self.observations
                .lock()
                .unwrap()
                .push((payload, source_exists, delivery_state));
            Ok(())
        }
    }

    #[test]
    fn reminder_delete_commits_then_emits_while_pending_then_cancels() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let todo_repository = crate::repositories::todos::TodoRepository::new(storage.clone());
        let reminder_repository =
            crate::repositories::todos::TodoReminderRepository::new(storage.clone());
        let todo = todo_repository
            .create(
                CreateTodoInput {
                    title: "delete reminder in order".into(),
                    description: String::new(),
                    due_at: None,
                    priority: TodoPriority::Normal,
                },
                10,
            )
            .unwrap();
        let services_for_setup = AppServices::from_parts(
            storage.clone(),
            Arc::new(BootstrapModuleStateProvider) as Arc<dyn ModuleStateProvider>,
            Arc::new(NoopShutdown),
            Arc::new(NoopCheckpoint),
            Arc::new(NoopEmitter),
        );
        let reminder = services_for_setup
            .todo_reminder_projector
            .save_and_project(
                SaveTodoReminderInput {
                    id: None,
                    todo_id: todo.id.clone(),
                    remind_at: 1_000,
                    enabled: true,
                    expected_revision: None,
                },
                20,
            )
            .unwrap();
        drop(services_for_setup);

        let observations = Arc::new(Mutex::new(Vec::new()));
        let services = AppServices::from_parts(
            storage.clone(),
            Arc::new(BootstrapModuleStateProvider) as Arc<dyn ModuleStateProvider>,
            Arc::new(NoopShutdown),
            Arc::new(NoopCheckpoint),
            Arc::new(ReminderDeleteCheckingEmitter {
                storage: storage.clone(),
                reminder_id: reminder.id.clone(),
                observations: observations.clone(),
            }),
        );
        let result = delete_todo_reminder_with_services(
            uuid::Uuid::parse_str(&reminder.id).unwrap(),
            reminder.revision as u64,
            &services,
            30,
        )
        .unwrap();
        assert_eq!(result.id, reminder.id);
        assert_eq!(
            *observations.lock().unwrap(),
            vec![(
                serde_json::json!({
                    "entityId": todo.id,
                    "revision": reminder.revision + 1,
                    "changedAt": 30,
                }),
                false,
                "pending".into(),
            )]
        );
        assert_eq!(
            reminder_repository
                .get_for_todo(uuid::Uuid::parse_str(&todo.id).unwrap())
                .unwrap(),
            None
        );
        let final_state: String = storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT state FROM reminder_deliveries WHERE source_kind = 'todo' AND source_entity_id = ?1",
                        [&todo.id],
                        |row| row.get(0),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(final_state, "cancelled");
    }

    #[test]
    fn reminder_delete_cancel_failure_keeps_the_delete_and_emit_but_leaves_pending_history() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let services_for_setup = AppServices::from_parts(
            storage.clone(),
            Arc::new(BootstrapModuleStateProvider) as Arc<dyn ModuleStateProvider>,
            Arc::new(NoopShutdown),
            Arc::new(NoopCheckpoint),
            Arc::new(NoopEmitter),
        );
        let todo = services_for_setup
            .todos
            .create(
                CreateTodoInput {
                    title: "cancel failure".into(),
                    description: String::new(),
                    due_at: None,
                    priority: TodoPriority::Normal,
                },
                10,
            )
            .unwrap();
        let reminder = services_for_setup
            .todo_reminder_projector
            .save_and_project(
                SaveTodoReminderInput {
                    id: None,
                    todo_id: todo.id.clone(),
                    remind_at: 1_000,
                    enabled: true,
                    expected_revision: None,
                },
                20,
            )
            .unwrap();
        drop(services_for_setup);

        let observations = Arc::new(Mutex::new(Vec::new()));
        let services = AppServices::from_parts(
            storage.clone(),
            Arc::new(BootstrapModuleStateProvider) as Arc<dyn ModuleStateProvider>,
            Arc::new(NoopShutdown),
            Arc::new(NoopCheckpoint),
            Arc::new(ReminderDeleteCheckingEmitter {
                storage: storage.clone(),
                reminder_id: reminder.id.clone(),
                observations: observations.clone(),
            }),
        );
        let error = delete_todo_reminder_with_services(
            uuid::Uuid::parse_str(&reminder.id).unwrap(),
            reminder.revision as u64,
            &services,
            -1,
        )
        .unwrap_err();
        assert_eq!(error.code, AppErrorCode::InvalidInput);
        assert_eq!(observations.lock().unwrap().len(), 1);
        assert!(!observations.lock().unwrap()[0].1);
        assert_eq!(observations.lock().unwrap()[0].2, "pending");
        assert_eq!(
            services
                .todo_reminders
                .get_for_todo(uuid::Uuid::parse_str(&todo.id).unwrap())
                .unwrap(),
            None
        );
        let final_state: String = storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT state FROM reminder_deliveries WHERE source_kind = 'todo' AND source_entity_id = ?1",
                        [&todo.id],
                        |row| row.get(0),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(final_state, "pending");
    }

    #[test]
    fn projection_failure_after_source_commit_records_only_todo_and_reminder_ids() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let services = AppServices::from_parts(
            storage.clone(),
            Arc::new(BootstrapModuleStateProvider) as Arc<dyn ModuleStateProvider>,
            Arc::new(NoopShutdown),
            Arc::new(NoopCheckpoint),
            Arc::new(NoopEmitter),
        );
        let todo = services
            .todos
            .create(
                CreateTodoInput {
                    title: "completed source".into(),
                    description: String::new(),
                    due_at: None,
                    priority: TodoPriority::Normal,
                },
                10,
            )
            .unwrap();
        services
            .todos
            .set_completed(
                CompleteTodoInput {
                    id: todo.id.clone(),
                    completed: true,
                    expected_revision: 1,
                },
                20,
            )
            .unwrap();
        let error = save_todo_reminder_with_services(
            SaveTodoReminderInput {
                id: None,
                todo_id: todo.id.clone(),
                remind_at: 1_000,
                enabled: true,
                expected_revision: None,
            },
            &services,
            30,
        )
        .unwrap_err();
        assert_eq!(error.code, AppErrorCode::InvalidInput);
        let committed = services
            .todo_reminders
            .get_for_todo(uuid::Uuid::parse_str(&todo.id).unwrap())
            .unwrap()
            .unwrap();
        let delivery_count: i64 = storage
            .with_connection(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM reminder_deliveries", [], |row| {
                        row.get(0)
                    })
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(delivery_count, 0);
        let event = services.diagnostics.list(1).unwrap().pop().unwrap();
        assert_eq!(event.code, "todo.reminderProjectionFailed");
        assert_eq!(
            event.parameters,
            std::collections::BTreeMap::from([
                (
                    "todoId".into(),
                    crate::contracts::SafeParameterValue::String(todo.id),
                ),
                (
                    "reminderId".into(),
                    crate::contracts::SafeParameterValue::String(committed.id),
                ),
            ])
        );
    }

    struct DeleteCheckingEmitter {
        storage: Arc<Storage>,
        observations: Arc<Mutex<Vec<(serde_json::Value, bool)>>>,
    }
    impl EventEmitterPort for DeleteCheckingEmitter {
        fn emit(&self, _: &'static str, payload: serde_json::Value) -> Result<(), CommandError> {
            let exists = self.storage.with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM todos WHERE id = ?1)",
                        [payload["entityId"].as_str().unwrap()],
                        |row| row.get(0),
                    )
                    .map_err(Into::into)
            })?;
            self.observations.lock().unwrap().push((payload, exists));
            Ok(())
        }
    }

    #[test]
    fn delete_commits_before_emitting_expected_revision_plus_one() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let observations = Arc::new(Mutex::new(Vec::new()));
        let services = AppServices::from_parts(
            storage.clone(),
            Arc::new(BootstrapModuleStateProvider) as Arc<dyn ModuleStateProvider>,
            Arc::new(NoopShutdown),
            Arc::new(NoopCheckpoint),
            Arc::new(DeleteCheckingEmitter {
                storage,
                observations: observations.clone(),
            }),
        );
        let created = create_todo_with_services(
            CreateTodoInput {
                title: "Delete".into(),
                description: String::new(),
                due_at: None,
                priority: TodoPriority::Low,
            },
            &services,
            10,
        )
        .unwrap();
        delete_todo_with_services(
            uuid::Uuid::parse_str(&created.id).unwrap(),
            1,
            &services,
            20,
        )
        .unwrap();
        assert_eq!(
            *observations.lock().unwrap(),
            vec![
                (
                    serde_json::json!({ "entityId": created.id, "revision": 1, "changedAt": 10 }),
                    true
                ),
                (
                    serde_json::json!({ "entityId": created.id, "revision": 2, "changedAt": 20 }),
                    false
                ),
            ]
        );
    }
}
