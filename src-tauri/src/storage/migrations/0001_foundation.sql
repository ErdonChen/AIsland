CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY CHECK (version > 0),
    name TEXT NOT NULL UNIQUE,
    applied_at INTEGER NOT NULL CHECK (applied_at >= 0)
);

CREATE TABLE app_settings (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL CHECK (json_valid(value_json)),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
);

CREATE TABLE service_health (
    service_id TEXT PRIMARY KEY,
    state TEXT NOT NULL CHECK (state IN ('healthy', 'degraded', 'blocked', 'offline')),
    message_key TEXT NOT NULL,
    parameters_json TEXT NOT NULL CHECK (json_valid(parameters_json)),
    checked_at INTEGER NOT NULL CHECK (checked_at >= 0)
);

CREATE TABLE diagnostic_events (
    id TEXT PRIMARY KEY,
    service_id TEXT NOT NULL,
    level TEXT NOT NULL CHECK (level IN ('info', 'warning', 'failure')),
    code TEXT NOT NULL,
    parameters_json TEXT NOT NULL CHECK (json_valid(parameters_json)),
    created_at INTEGER NOT NULL CHECK (created_at >= 0)
);

CREATE INDEX diagnostic_events_created_at_idx
ON diagnostic_events(created_at DESC, id DESC);
