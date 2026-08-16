CREATE TABLE todos (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL CHECK (length(trim(title)) BETWEEN 1 AND 200),
    description TEXT NOT NULL CHECK (length(description) <= 4000),
    due_at INTEGER CHECK (due_at IS NULL OR due_at >= 0),
    priority TEXT NOT NULL CHECK (priority IN ('low','normal','high')),
    status TEXT NOT NULL CHECK (status IN ('open','completed')),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
    completed_at INTEGER CHECK (completed_at IS NULL OR completed_at >= created_at),
    CHECK (
        (status = 'open' AND completed_at IS NULL) OR
        (status = 'completed' AND completed_at IS NOT NULL)
    )
);

CREATE INDEX todos_status_due_idx
ON todos(status, due_at, updated_at DESC, id);

CREATE TABLE todo_reminders (
    id TEXT PRIMARY KEY,
    todo_id TEXT NOT NULL UNIQUE REFERENCES todos(id) ON DELETE CASCADE,
    remind_at INTEGER NOT NULL CHECK (remind_at >= 0),
    enabled INTEGER NOT NULL CHECK (enabled IN (0,1)),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at)
);

CREATE INDEX todo_reminders_due_idx
ON todo_reminders(enabled, remind_at, todo_id);

CREATE TABLE notes (
    id TEXT PRIMARY KEY,
    note_date TEXT NOT NULL UNIQUE CHECK (
        length(note_date) = 10 AND
        note_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'
    ),
    body_markdown TEXT NOT NULL CHECK (length(body_markdown) <= 262144),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    export_history_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(export_history_json)),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= created_at)
);

CREATE INDEX notes_updated_idx
ON notes(updated_at DESC, note_date DESC, id);
