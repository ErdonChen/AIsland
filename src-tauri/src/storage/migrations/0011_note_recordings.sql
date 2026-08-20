CREATE TABLE note_recordings (
    id TEXT PRIMARY KEY NOT NULL,
    note_date TEXT NOT NULL CHECK (
        length(note_date) = 10
        AND substr(note_date, 5, 1) = '-'
        AND substr(note_date, 8, 1) = '-'
    ),
    asset_name TEXT NOT NULL UNIQUE,
    mime_type TEXT NOT NULL,
    file_extension TEXT NOT NULL,
    byte_size INTEGER NOT NULL DEFAULT 0 CHECK (byte_size >= 0),
    started_at INTEGER NOT NULL CHECK (started_at >= 0),
    duration_ms INTEGER NOT NULL DEFAULT 0 CHECK (duration_ms >= 0),
    status TEXT NOT NULL DEFAULT 'recording' CHECK (status IN ('recording', 'completed')),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
);

CREATE INDEX idx_note_recordings_date_started
    ON note_recordings(note_date, started_at, id);
