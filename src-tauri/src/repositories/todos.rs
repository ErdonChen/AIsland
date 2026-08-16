use crate::contracts::{
    AppErrorCode, CommandError, CompleteTodoInput, CreateTodoInput, DeleteResult,
    SafeMessageParameters, SaveTodoReminderInput, TodoItem, TodoPriority, TodoReminder, TodoStatus,
    TrueLiteral, UpdateTodoInput,
};
use crate::domain::todo::TodoListFilter;
use crate::storage::Storage;
use rusqlite::OptionalExtension;
use std::sync::Arc;
use uuid::Uuid;

const TODO_FIELDS: &str =
    "id, title, description, due_at, priority, status, revision, created_at, updated_at, completed_at";
const TODO_REMINDER_FIELDS: &str =
    "id, todo_id, remind_at, enabled, revision, created_at, updated_at";

#[derive(Clone)]
pub struct TodoRepository {
    storage: Arc<Storage>,
}

#[derive(Clone)]
pub struct TodoReminderRepository {
    storage: Arc<Storage>,
}

impl TodoReminderRepository {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    pub fn get_for_todo(&self, todo_id: Uuid) -> Result<Option<TodoReminder>, CommandError> {
        let query = format!("SELECT {TODO_REMINDER_FIELDS} FROM todo_reminders WHERE todo_id = ?1");
        self.storage.with_connection(|connection| {
            connection
                .query_row(&query, [todo_id.to_string()], row_to_todo_reminder)
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn list(&self, todo_id: Option<Uuid>) -> Result<Vec<TodoReminder>, CommandError> {
        let query = format!(
            r#"SELECT {TODO_REMINDER_FIELDS} FROM todo_reminders
               WHERE (?1 IS NULL OR todo_id = ?1)
               ORDER BY updated_at DESC, id ASC"#
        );
        self.storage.with_connection(|connection| {
            let mut statement = connection.prepare(&query)?;
            let rows = statement
                .query_map(
                    [todo_id.map(|value| value.to_string())],
                    row_to_todo_reminder,
                )?
                .collect::<Result<Vec<_>, _>>()
                .map_err(CommandError::from)?;
            Ok(rows)
        })
    }

    pub fn list_enabled(&self) -> Result<Vec<TodoReminder>, CommandError> {
        let query = format!(
            r#"SELECT {TODO_REMINDER_FIELDS} FROM todo_reminders
               WHERE enabled = 1 ORDER BY remind_at ASC, todo_id ASC, id ASC"#
        );
        self.storage.with_connection(|connection| {
            let mut statement = connection.prepare(&query)?;
            let rows = statement
                .query_map([], row_to_todo_reminder)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(CommandError::from)?;
            Ok(rows)
        })
    }

    pub fn list_pending_delivery_source_ids(&self) -> Result<Vec<Uuid>, CommandError> {
        self.storage.with_connection(|connection| {
            let mut statement = connection.prepare(
                r#"SELECT DISTINCT source_entity_id FROM reminder_deliveries
                   WHERE source_kind = 'todo' AND state IN ('pending', 'snoozed')
                   ORDER BY source_entity_id"#,
            )?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .map(|row| {
                    let id = row?;
                    Uuid::parse_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(CommandError::from)?;
            Ok(rows)
        })
    }

    pub fn save(
        &self,
        input: SaveTodoReminderInput,
        now: i64,
    ) -> Result<TodoReminder, CommandError> {
        validate_reminder_input(&input, now)?;
        self.storage.with_transaction(|transaction| {
            let todo_exists = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM todos WHERE id = ?1)",
                [&input.todo_id],
                |row| row.get::<_, bool>(0),
            )?;
            if !todo_exists {
                return Err(not_found());
            }

            let id = input
                .id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            match (&input.id, input.expected_revision) {
                (None, None) => {
                    let current_revision = transaction.query_row(
                        "SELECT MAX(revision) FROM todo_reminders WHERE todo_id = ?1",
                        [&input.todo_id],
                        |row| row.get::<_, Option<i64>>(0),
                    )?;
                    let retained_revision = transaction.query_row(
                        r#"SELECT MAX(json_extract(source_context_json, '$.reminderRevision'))
                           FROM reminder_deliveries
                           WHERE source_kind = 'todo' AND source_entity_id = ?1
                             AND json_type(source_context_json, '$.reminderRevision') = 'integer'"#,
                        [&input.todo_id],
                        |row| row.get::<_, Option<i64>>(0),
                    )?;
                    let revision = current_revision
                        .into_iter()
                        .chain(retained_revision)
                        .max()
                        .unwrap_or(0)
                        .checked_add(1)
                        .filter(|revision| *revision > 0)
                        .ok_or_else(invalid_input)?;
                    transaction.execute(
                        r#"INSERT INTO todo_reminders(
                             id, todo_id, remind_at, enabled, revision, created_at, updated_at
                           ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)"#,
                        rusqlite::params![
                            id,
                            input.todo_id,
                            input.remind_at,
                            input.enabled,
                            revision,
                            now
                        ],
                    )?;
                }
                (Some(_), Some(expected_revision)) => {
                    let changed = transaction.execute(
                        r#"UPDATE todo_reminders SET
                             remind_at = ?3, enabled = ?4, revision = revision + 1, updated_at = ?5
                           WHERE id = ?1 AND todo_id = ?2 AND revision = ?6"#,
                        rusqlite::params![
                            id,
                            input.todo_id,
                            input.remind_at,
                            input.enabled,
                            now,
                            expected_revision
                        ],
                    )?;
                    if changed == 0 {
                        return reminder_mutation_miss(transaction, &id);
                    }
                }
                _ => return Err(invalid_input()),
            }
            let query = format!("SELECT {TODO_REMINDER_FIELDS} FROM todo_reminders WHERE id = ?1");
            transaction
                .query_row(&query, [id], row_to_todo_reminder)
                .map_err(Into::into)
        })
    }

    pub fn delete(&self, id: Uuid, expected_revision: u64) -> Result<DeleteResult, CommandError> {
        let expected_revision = i64::try_from(expected_revision).map_err(|_| invalid_input())?;
        if expected_revision < 1 {
            return Err(invalid_input());
        }
        let id = id.to_string();
        self.storage.with_transaction(|transaction| {
            let deleted = transaction.execute(
                "DELETE FROM todo_reminders WHERE id = ?1 AND revision = ?2",
                rusqlite::params![id, expected_revision],
            )?;
            if deleted == 0 {
                return reminder_mutation_miss(transaction, &id);
            }
            Ok(DeleteResult {
                id,
                deleted: TrueLiteral,
            })
        })
    }
}

impl TodoRepository {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    pub fn list(&self, status: TodoListFilter, limit: u32) -> Result<Vec<TodoItem>, CommandError> {
        if !(1..=500).contains(&limit) {
            return Err(invalid_input());
        }
        let status = match status {
            TodoListFilter::All => None,
            TodoListFilter::Open => Some("open"),
            TodoListFilter::Completed => Some("completed"),
        };
        let query = format!(
            r#"SELECT {TODO_FIELDS} FROM todos
               WHERE (?1 IS NULL OR status = ?1)
               ORDER BY
                 CASE WHEN status = 'open' THEN 0 ELSE 1 END,
                 CASE WHEN status = 'open' AND due_at IS NULL THEN 1 ELSE 0 END,
                 CASE WHEN status = 'open' THEN due_at END ASC,
                 CASE WHEN status = 'open' THEN CASE priority WHEN 'high' THEN 0 WHEN 'normal' THEN 1 ELSE 2 END END ASC,
                 CASE WHEN status = 'open' THEN updated_at END DESC,
                 CASE WHEN status = 'completed' THEN completed_at END DESC,
                 id ASC
               LIMIT ?2"#
        );
        self.storage.with_connection(|connection| {
            let mut statement = connection.prepare(&query)?;
            let rows = statement
                .query_map(rusqlite::params![status, limit], row_to_todo)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(Into::into);
            rows
        })
    }

    pub fn create(&self, input: CreateTodoInput, now: i64) -> Result<TodoItem, CommandError> {
        let input = validate_create(input, now)?;
        let id = Uuid::new_v4().to_string();
        let priority = priority_name(&input.priority);
        let query = format!(
            r#"INSERT INTO todos(id, title, description, due_at, priority, status, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, 'open', ?6, ?6)
               RETURNING {TODO_FIELDS}"#
        );
        self.storage.with_transaction(|transaction| {
            transaction
                .query_row(
                    &query,
                    rusqlite::params![
                        id,
                        input.title,
                        input.description,
                        input.due_at,
                        priority,
                        now
                    ],
                    row_to_todo,
                )
                .map_err(Into::into)
        })
    }

    pub fn update(&self, input: UpdateTodoInput, now: i64) -> Result<TodoItem, CommandError> {
        let input = validate_update(input, now)?;
        let priority = priority_name(&input.priority);
        let query = format!(
            r#"UPDATE todos SET
                 title = ?2, description = ?3, due_at = ?4, priority = ?5,
                 revision = revision + 1, updated_at = ?6
               WHERE id = ?1 AND revision = ?7
               RETURNING {TODO_FIELDS}"#
        );
        self.storage.with_transaction(|transaction| {
            let updated = transaction
                .query_row(
                    &query,
                    rusqlite::params![
                        input.id,
                        input.title,
                        input.description,
                        input.due_at,
                        priority,
                        now,
                        input.expected_revision
                    ],
                    row_to_todo,
                )
                .optional()?;
            updated.map_or_else(|| mutation_miss(transaction, &input.id), Ok)
        })
    }

    pub fn set_completed(
        &self,
        input: CompleteTodoInput,
        now: i64,
    ) -> Result<TodoItem, CommandError> {
        validate_mutation_identity(&input.id, input.expected_revision, now)?;
        let query = format!(
            r#"UPDATE todos SET
                 status = CASE WHEN ?2 THEN 'completed' ELSE 'open' END,
                 completed_at = CASE WHEN ?2 THEN ?3 ELSE NULL END,
                 revision = revision + 1, updated_at = ?3
               WHERE id = ?1 AND revision = ?4
               RETURNING {TODO_FIELDS}"#
        );
        self.storage.with_transaction(|transaction| {
            let updated = transaction
                .query_row(
                    &query,
                    rusqlite::params![input.id, input.completed, now, input.expected_revision],
                    row_to_todo,
                )
                .optional()?;
            let Some(updated) = updated else {
                return mutation_miss(transaction, &input.id);
            };
            transaction.execute(
                r#"UPDATE todo_reminders SET revision = revision + 1, updated_at = ?2
                   WHERE todo_id = ?1"#,
                rusqlite::params![input.id, now],
            )?;
            Ok(updated)
        })
    }

    pub fn delete(&self, id: Uuid, expected_revision: u64) -> Result<DeleteResult, CommandError> {
        let expected_revision = i64::try_from(expected_revision).map_err(|_| invalid_input())?;
        if expected_revision < 1 {
            return Err(invalid_input());
        }
        let id = id.to_string();
        self.storage.with_transaction(|transaction| {
            let deleted = transaction.execute(
                "DELETE FROM todos WHERE id = ?1 AND revision = ?2",
                rusqlite::params![id, expected_revision],
            )?;
            if deleted == 0 {
                return mutation_miss(transaction, &id);
            }
            Ok(DeleteResult {
                id,
                deleted: TrueLiteral,
            })
        })
    }

    pub fn get(&self, id: Uuid) -> Result<TodoItem, CommandError> {
        let query = format!("SELECT {TODO_FIELDS} FROM todos WHERE id = ?1");
        self.storage.with_connection(|connection| {
            connection
                .query_row(&query, [id.to_string()], row_to_todo)
                .optional()?
                .ok_or_else(not_found)
        })
    }
}

fn validate_reminder_input(input: &SaveTodoReminderInput, now: i64) -> Result<(), CommandError> {
    if now < 0
        || input.remind_at < 0
        || Uuid::parse_str(&input.todo_id).is_err()
        || input.expected_revision.is_some_and(|value| value < 1)
        || input
            .id
            .as_deref()
            .is_some_and(|value| Uuid::parse_str(value).is_err())
    {
        return Err(invalid_input());
    }
    Ok(())
}

fn reminder_mutation_miss<T>(
    transaction: &rusqlite::Transaction<'_>,
    id: &str,
) -> Result<T, CommandError> {
    let exists = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM todo_reminders WHERE id = ?1)",
        [id],
        |row| row.get::<_, bool>(0),
    )?;
    Err(if exists { conflict() } else { not_found() })
}

fn validate_create(input: CreateTodoInput, now: i64) -> Result<CreateTodoInput, CommandError> {
    let title = input.title.trim().to_owned();
    if now < 0
        || !(1..=200).contains(&title.chars().count())
        || input.description.chars().count() > 4_000
        || input.due_at.is_some_and(|value| value < 0)
    {
        return Err(invalid_input());
    }
    Ok(CreateTodoInput { title, ..input })
}

fn validate_update(input: UpdateTodoInput, now: i64) -> Result<UpdateTodoInput, CommandError> {
    validate_mutation_identity(&input.id, input.expected_revision, now)?;
    let create = validate_create(
        CreateTodoInput {
            title: input.title,
            description: input.description,
            due_at: input.due_at,
            priority: input.priority,
        },
        now,
    )?;
    Ok(UpdateTodoInput {
        id: input.id,
        title: create.title,
        description: create.description,
        due_at: create.due_at,
        priority: create.priority,
        expected_revision: input.expected_revision,
    })
}

fn validate_mutation_identity(
    id: &str,
    expected_revision: i64,
    now: i64,
) -> Result<(), CommandError> {
    if now < 0 || expected_revision < 1 || Uuid::parse_str(id).is_err() {
        return Err(invalid_input());
    }
    Ok(())
}

fn mutation_miss<T>(transaction: &rusqlite::Transaction<'_>, id: &str) -> Result<T, CommandError> {
    let exists = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM todos WHERE id = ?1)",
        [id],
        |row| row.get::<_, bool>(0),
    )?;
    Err(if exists { conflict() } else { not_found() })
}

fn row_to_todo(row: &rusqlite::Row<'_>) -> rusqlite::Result<TodoItem> {
    let priority = match row.get::<_, String>(4)?.as_str() {
        "low" => TodoPriority::Low,
        "normal" => TodoPriority::Normal,
        "high" => TodoPriority::High,
        value => return Err(invalid_column_value(4, value)),
    };
    let status = match row.get::<_, String>(5)?.as_str() {
        "open" => TodoStatus::Open,
        "completed" => TodoStatus::Completed,
        value => return Err(invalid_column_value(5, value)),
    };
    Ok(TodoItem {
        id: row.get(0)?,
        title: row.get(1)?,
        description: row.get(2)?,
        due_at: row.get(3)?,
        priority,
        status,
        revision: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        completed_at: row.get(9)?,
    })
}

fn row_to_todo_reminder(row: &rusqlite::Row<'_>) -> rusqlite::Result<TodoReminder> {
    Ok(TodoReminder {
        id: row.get(0)?,
        todo_id: row.get(1)?,
        remind_at: row.get(2)?,
        enabled: row.get(3)?,
        revision: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn invalid_column_value(index: usize, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        format!("invalid todo value {value}").into(),
    )
}

fn priority_name(priority: &TodoPriority) -> &'static str {
    match priority {
        TodoPriority::Low => "low",
        TodoPriority::Normal => "normal",
        TodoPriority::High => "high",
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

fn conflict() -> CommandError {
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

#[cfg(test)]
mod tests {
    use super::{TodoReminderRepository, TodoRepository};
    use crate::contracts::{
        AppErrorCode, CompleteTodoInput, CreateTodoInput, SaveTodoReminderInput, TodoPriority,
        TodoStatus, UpdateTodoInput,
    };
    use crate::domain::todo::TodoListFilter;
    use crate::storage::Storage;
    use std::sync::Arc;

    fn repository() -> TodoRepository {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.keep();
        TodoRepository::new(Arc::new(Storage::open(&path).unwrap()))
    }

    fn input(title: &str, due_at: Option<i64>, priority: TodoPriority) -> CreateTodoInput {
        CreateTodoInput {
            title: title.into(),
            description: String::new(),
            due_at,
            priority,
        }
    }

    #[test]
    fn todo_reminder_stale_save_and_delete_conflicts_leave_the_committed_row_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let todos = TodoRepository::new(storage.clone());
        let reminders = TodoReminderRepository::new(storage);
        let todo = todos
            .create(input("locked reminder", None, TodoPriority::Normal), 10)
            .unwrap();
        let created = reminders
            .save(
                SaveTodoReminderInput {
                    id: None,
                    todo_id: todo.id.clone(),
                    remind_at: 100,
                    enabled: true,
                    expected_revision: None,
                },
                10,
            )
            .unwrap();
        let stale_save = reminders
            .save(
                SaveTodoReminderInput {
                    id: Some(created.id.clone()),
                    todo_id: todo.id.clone(),
                    remind_at: 200,
                    enabled: false,
                    expected_revision: Some(2),
                },
                20,
            )
            .unwrap_err();
        assert_eq!(
            (stale_save.code, stale_save.retryable),
            (AppErrorCode::Conflict, true)
        );
        assert_eq!(
            reminders
                .get_for_todo(uuid::Uuid::parse_str(&todo.id).unwrap())
                .unwrap(),
            Some(created.clone())
        );

        let stale_delete = reminders
            .delete(uuid::Uuid::parse_str(&created.id).unwrap(), 2)
            .unwrap_err();
        assert_eq!(
            (stale_delete.code, stale_delete.retryable),
            (AppErrorCode::Conflict, true)
        );
        assert_eq!(reminders.list(None).unwrap(), vec![created]);
    }

    #[test]
    fn create_update_and_completion_are_revision_safe() {
        let repository = repository();
        let created = repository
            .create(input("  Ship V1  ", Some(100), TodoPriority::High), 10)
            .unwrap();
        assert!(
            uuid::Uuid::parse_str(&created.id)
                .unwrap()
                .get_version_num()
                == 4
        );
        assert_eq!((created.title.as_str(), created.revision), ("Ship V1", 1));

        let update = UpdateTodoInput {
            id: created.id.clone(),
            title: "Ship V1.1".into(),
            description: "ready".into(),
            due_at: Some(200),
            priority: TodoPriority::Normal,
            expected_revision: 1,
        };
        let updated = repository.update(update.clone(), 20).unwrap();
        assert_eq!((updated.title.as_str(), updated.revision), ("Ship V1.1", 2));
        let conflict = repository.update(update, 30).unwrap_err();
        assert_eq!(
            (conflict.code, conflict.retryable),
            (AppErrorCode::Conflict, true)
        );
        assert_eq!(
            repository
                .get(uuid::Uuid::parse_str(&created.id).unwrap())
                .unwrap(),
            updated
        );

        let completed = repository
            .set_completed(
                CompleteTodoInput {
                    id: created.id.clone(),
                    completed: true,
                    expected_revision: 2,
                },
                40,
            )
            .unwrap();
        assert_eq!((completed.revision, completed.completed_at), (3, Some(40)));
    }

    #[test]
    fn list_filters_and_uses_the_locked_deterministic_order() {
        let repository = repository();
        let no_due = repository
            .create(input("no due", None, TodoPriority::High), 30)
            .unwrap();
        let due_tie_a = repository
            .create(input("due tie a", Some(5), TodoPriority::High), 9)
            .unwrap();
        let due_tie_b = repository
            .create(input("due tie b", Some(5), TodoPriority::High), 9)
            .unwrap();
        let low = repository
            .create(input("low", Some(10), TodoPriority::Low), 20)
            .unwrap();
        let normal = repository
            .create(input("normal", Some(10), TodoPriority::Normal), 10)
            .unwrap();
        let high_old = repository
            .create(input("high old", Some(10), TodoPriority::High), 1)
            .unwrap();
        let high_new = repository
            .create(input("high new", Some(10), TodoPriority::High), 2)
            .unwrap();
        let completed_old = repository
            .create(input("completed old", None, TodoPriority::Normal), 3)
            .unwrap();
        let completed_new = repository
            .create(input("completed new", None, TodoPriority::Normal), 4)
            .unwrap();
        let completed_tie_a = repository
            .create(input("completed tie a", None, TodoPriority::Normal), 5)
            .unwrap();
        let completed_tie_b = repository
            .create(input("completed tie b", None, TodoPriority::Normal), 6)
            .unwrap();
        repository
            .set_completed(
                CompleteTodoInput {
                    id: completed_old.id.clone(),
                    completed: true,
                    expected_revision: 1,
                },
                50,
            )
            .unwrap();
        repository
            .set_completed(
                CompleteTodoInput {
                    id: completed_new.id.clone(),
                    completed: true,
                    expected_revision: 1,
                },
                60,
            )
            .unwrap();
        repository
            .set_completed(
                CompleteTodoInput {
                    id: completed_tie_a.id.clone(),
                    completed: true,
                    expected_revision: 1,
                },
                70,
            )
            .unwrap();
        repository
            .set_completed(
                CompleteTodoInput {
                    id: completed_tie_b.id.clone(),
                    completed: true,
                    expected_revision: 1,
                },
                70,
            )
            .unwrap();

        let open = repository.list(TodoListFilter::Open, 500).unwrap();
        let mut due_tie_ids = [due_tie_a.id.as_str(), due_tie_b.id.as_str()];
        due_tie_ids.sort();
        let mut expected_open = due_tie_ids.to_vec();
        expected_open.extend([
            high_new.id.as_str(),
            high_old.id.as_str(),
            normal.id.as_str(),
            low.id.as_str(),
            no_due.id.as_str(),
        ]);
        assert_eq!(
            open.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
            expected_open
        );
        let completed = repository.list(TodoListFilter::Completed, 500).unwrap();
        let mut completed_tie_ids = [completed_tie_a.id.as_str(), completed_tie_b.id.as_str()];
        completed_tie_ids.sort();
        let mut expected_completed = completed_tie_ids.to_vec();
        expected_completed.extend([completed_new.id.as_str(), completed_old.id.as_str()]);
        assert_eq!(
            completed
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            expected_completed
        );
        let all = repository.list(TodoListFilter::All, 500).unwrap();
        assert_eq!(
            all.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
            expected_open
                .into_iter()
                .chain(expected_completed)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn validation_conflict_not_found_and_delete_fail_without_mutation() {
        let repository = repository();
        for invalid in [
            input("   ", None, TodoPriority::Normal),
            input(&"x".repeat(201), None, TodoPriority::Normal),
            CreateTodoInput {
                title: "ok".into(),
                description: "x".repeat(4001),
                due_at: None,
                priority: TodoPriority::Normal,
            },
            input("ok", Some(-1), TodoPriority::Normal),
        ] {
            assert_eq!(
                repository.create(invalid, 1).unwrap_err().code,
                AppErrorCode::InvalidInput
            );
        }
        assert_eq!(
            repository
                .create(input("ok", None, TodoPriority::Normal), -1)
                .unwrap_err()
                .code,
            AppErrorCode::InvalidInput
        );
        assert_eq!(
            repository.list(TodoListFilter::All, 0).unwrap_err().code,
            AppErrorCode::InvalidInput
        );
        assert_eq!(
            repository.list(TodoListFilter::All, 501).unwrap_err().code,
            AppErrorCode::InvalidInput
        );

        let created = repository
            .create(input("keep", None, TodoPriority::Normal), 1)
            .unwrap();
        let id = uuid::Uuid::parse_str(&created.id).unwrap();
        assert_eq!(
            repository.delete(id, 2).unwrap_err().code,
            AppErrorCode::Conflict
        );
        assert_eq!(repository.get(id).unwrap(), created);
        let missing = uuid::Uuid::new_v4();
        assert_eq!(
            repository.delete(missing, 1).unwrap_err().code,
            AppErrorCode::NotFound
        );
        assert_eq!(
            repository.get(missing).unwrap_err().code,
            AppErrorCode::NotFound
        );
        let deleted = repository.delete(id, 1).unwrap();
        assert_eq!(deleted.id, created.id);
        assert_eq!(
            serde_json::to_value(deleted).unwrap(),
            serde_json::json!({ "id": created.id, "deleted": true })
        );
    }

    #[test]
    fn completion_reopen_and_stale_attempts_preserve_locked_state() {
        let repository = repository();
        let created = repository
            .create(input("toggle", None, TodoPriority::Normal), 10)
            .unwrap();
        let id = uuid::Uuid::parse_str(&created.id).unwrap();

        let stale = repository
            .set_completed(
                CompleteTodoInput {
                    id: created.id.clone(),
                    completed: true,
                    expected_revision: 2,
                },
                20,
            )
            .unwrap_err();
        assert_eq!(
            (stale.code, stale.retryable),
            (AppErrorCode::Conflict, true)
        );
        assert_eq!(repository.get(id).unwrap(), created);

        let completed = repository
            .set_completed(
                CompleteTodoInput {
                    id: created.id.clone(),
                    completed: true,
                    expected_revision: 1,
                },
                20,
            )
            .unwrap();
        assert_eq!(
            (
                completed.status.clone(),
                completed.completed_at,
                completed.revision
            ),
            (TodoStatus::Completed, Some(20), 2)
        );
        let repeated = repository
            .set_completed(
                CompleteTodoInput {
                    id: created.id.clone(),
                    completed: false,
                    expected_revision: 1,
                },
                30,
            )
            .unwrap_err();
        assert_eq!(
            (repeated.code, repeated.retryable),
            (AppErrorCode::Conflict, true)
        );
        assert_eq!(repository.get(id).unwrap(), completed);

        let reopened = repository
            .set_completed(
                CompleteTodoInput {
                    id: created.id.clone(),
                    completed: false,
                    expected_revision: 2,
                },
                30,
            )
            .unwrap();
        assert_eq!(
            (reopened.status, reopened.completed_at, reopened.revision),
            (TodoStatus::Open, None, 3)
        );
    }

    #[test]
    fn invalid_update_and_completion_do_not_mutate_and_delete_cascades_reminder() {
        let repository = repository();
        let created = repository
            .create(input("keep", None, TodoPriority::High), 10)
            .unwrap();
        let id = uuid::Uuid::parse_str(&created.id).unwrap();
        let invalid_update = UpdateTodoInput {
            id: created.id.clone(),
            title: " ".into(),
            description: String::new(),
            due_at: None,
            priority: TodoPriority::Low,
            expected_revision: 1,
        };
        assert_eq!(
            repository.update(invalid_update, 20).unwrap_err().code,
            AppErrorCode::InvalidInput
        );
        assert_eq!(
            repository
                .set_completed(
                    CompleteTodoInput {
                        id: created.id.clone(),
                        completed: true,
                        expected_revision: 1
                    },
                    -1
                )
                .unwrap_err()
                .code,
            AppErrorCode::InvalidInput
        );
        assert_eq!(repository.get(id).unwrap(), created);

        repository.storage.with_connection(|connection| {
            connection.execute(
                "INSERT INTO todo_reminders(id, todo_id, remind_at, enabled, created_at, updated_at) VALUES (?1, ?2, 30, 1, 10, 10)",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), created.id],
            )?;
            Ok(())
        }).unwrap();
        repository.delete(id, 1).unwrap();
        let reminder_count = repository
            .storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM todo_reminders WHERE todo_id = ?1",
                        [created.id],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(reminder_count, 0);
    }

    #[test]
    fn create_counts_unicode_scalars_at_exact_title_and_description_boundaries() {
        let repository = repository();
        let title_at_limit = "🧊".repeat(200);
        let title_over_limit = "🧊".repeat(201);
        let description_at_limit = "界".repeat(4_000);
        let description_over_limit = "界".repeat(4_001);
        assert_eq!(title_at_limit.chars().count(), 200);
        assert_eq!(title_over_limit.chars().count(), 201);
        assert_eq!(description_at_limit.chars().count(), 4_000);
        assert_eq!(description_over_limit.chars().count(), 4_001);

        let accepted = repository
            .create(
                CreateTodoInput {
                    title: title_at_limit,
                    description: description_at_limit,
                    due_at: None,
                    priority: TodoPriority::Normal,
                },
                10,
            )
            .unwrap();
        assert_eq!(accepted.title.chars().count(), 200);
        assert_eq!(accepted.description.chars().count(), 4_000);
        for invalid in [
            CreateTodoInput {
                title: title_over_limit,
                description: String::new(),
                due_at: None,
                priority: TodoPriority::Normal,
            },
            CreateTodoInput {
                title: "valid".into(),
                description: description_over_limit,
                due_at: None,
                priority: TodoPriority::Normal,
            },
        ] {
            assert_eq!(
                repository.create(invalid, 20).unwrap_err().code,
                AppErrorCode::InvalidInput
            );
        }
        assert_eq!(
            repository.list(TodoListFilter::All, 500).unwrap(),
            vec![accepted]
        );
    }

    #[test]
    fn update_reuses_unicode_scalar_boundaries_and_invalid_inputs_do_not_write() {
        let repository = repository();
        let created = repository
            .create(input("seed", None, TodoPriority::Low), 10)
            .unwrap();
        let id = uuid::Uuid::parse_str(&created.id).unwrap();
        let title_at_limit = "🧊".repeat(200);
        let title_over_limit = "🧊".repeat(201);
        let description_at_limit = "界".repeat(4_000);
        let description_over_limit = "界".repeat(4_001);
        assert_eq!(title_at_limit.chars().count(), 200);
        assert_eq!(title_over_limit.chars().count(), 201);
        assert_eq!(description_at_limit.chars().count(), 4_000);
        assert_eq!(description_over_limit.chars().count(), 4_001);

        let accepted = repository
            .update(
                UpdateTodoInput {
                    id: created.id.clone(),
                    title: title_at_limit,
                    description: description_at_limit,
                    due_at: None,
                    priority: TodoPriority::High,
                    expected_revision: 1,
                },
                20,
            )
            .unwrap();
        assert_eq!(accepted.revision, 2);
        for (title, description) in [
            (title_over_limit, String::new()),
            ("valid".into(), description_over_limit),
        ] {
            assert_eq!(
                repository
                    .update(
                        UpdateTodoInput {
                            id: created.id.clone(),
                            title,
                            description,
                            due_at: None,
                            priority: TodoPriority::Low,
                            expected_revision: 2,
                        },
                        30,
                    )
                    .unwrap_err()
                    .code,
                AppErrorCode::InvalidInput
            );
            assert_eq!(repository.get(id).unwrap(), accepted);
        }
    }
}
