CREATE TABLE agent_integrations (
    agent_id TEXT NOT NULL CHECK (agent_id IN ('codex','hermes','workbuddy','claude')),
    environment TEXT NOT NULL CHECK (environment IN ('windows','wsl')),
    install_state TEXT NOT NULL CHECK (install_state IN ('notInstalled','installed','needsRepair','unsupported')),
    config_path TEXT NOT NULL,
    backup_path TEXT,
    owned_fingerprint TEXT,
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
    PRIMARY KEY (agent_id, environment),
    CHECK (NOT (agent_id = 'workbuddy' AND environment = 'wsl' AND install_state <> 'unsupported'))
);

CREATE TABLE agent_events (
    event_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL CHECK (agent_id IN ('codex','hermes','workbuddy','claude')),
    environment TEXT NOT NULL CHECK (environment IN ('windows','wsl')),
    task_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('idle','running','completed','failed','waiting','timeout','offline')),
    sequence INTEGER CHECK (sequence >= 0),
    task_title TEXT,
    project TEXT,
    message TEXT,
    path TEXT,
    occurred_at INTEGER NOT NULL CHECK (occurred_at >= 0),
    received_at INTEGER NOT NULL CHECK (received_at >= 0)
);
CREATE INDEX agent_events_source_time_idx ON agent_events(agent_id, environment, occurred_at DESC, event_id DESC);

CREATE TABLE agent_tasks (
    agent_id TEXT NOT NULL,
    environment TEXT NOT NULL,
    task_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('idle','running','completed','failed','waiting','timeout','offline')),
    summary TEXT NOT NULL,
    latest_sequence INTEGER CHECK (latest_sequence >= 0),
    source_event_id TEXT NOT NULL,
    occurred_at INTEGER NOT NULL CHECK (occurred_at >= 0),
    received_at INTEGER NOT NULL CHECK (received_at >= 0),
    PRIMARY KEY (agent_id, environment, task_id)
);

CREATE TABLE event_cursors (
    source_id TEXT PRIMARY KEY,
    last_sequence INTEGER CHECK (last_sequence >= 0),
    last_occurred_at INTEGER NOT NULL CHECK (last_occurred_at >= 0),
    last_event_id TEXT NOT NULL,
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
);

CREATE TABLE reminder_rules (
    id TEXT PRIMARY KEY,
    agent_ids_json TEXT NOT NULL CHECK (json_valid(agent_ids_json)),
    trigger_statuses_json TEXT NOT NULL CHECK (json_valid(trigger_statuses_json)),
    enabled INTEGER NOT NULL CHECK (enabled IN (0,1)),
    delay_seconds INTEGER NOT NULL CHECK (delay_seconds BETWEEN 0 AND 604800),
    sound_json TEXT NOT NULL CHECK (json_valid(sound_json)),
    toast_enabled INTEGER NOT NULL CHECK (toast_enabled IN (0,1)),
    window_enabled INTEGER NOT NULL CHECK (window_enabled IN (0,1)),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
);

CREATE TABLE reminder_dispatch_counter (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    next_dispatch_seq INTEGER NOT NULL CHECK (next_dispatch_seq > 0)
);
INSERT INTO reminder_dispatch_counter(singleton_id, next_dispatch_seq) VALUES (1, 1);

CREATE TABLE reminder_deliveries (
    id TEXT PRIMARY KEY,
    dedupe_key TEXT NOT NULL UNIQUE,
    rule_id TEXT REFERENCES reminder_rules(id) ON DELETE SET NULL,
    source_kind TEXT NOT NULL CHECK (source_kind IN ('agent','todo','monitor')),
    source_entity_id TEXT NOT NULL,
    message_key TEXT NOT NULL,
    message_parameters_json TEXT NOT NULL CHECK (json_valid(message_parameters_json)),
    source_context_json TEXT NOT NULL CHECK (json_valid(source_context_json)),
    source_occurred_at INTEGER NOT NULL CHECK (source_occurred_at >= 0),
    sound_json TEXT NOT NULL CHECK (json_valid(sound_json)),
    toast_enabled INTEGER NOT NULL CHECK (toast_enabled IN (0,1)),
    window_enabled INTEGER NOT NULL CHECK (window_enabled IN (0,1)),
    state TEXT NOT NULL CHECK (state IN ('pending','dispatched','acknowledged','snoozed','cancelled','completed')),
    due_at INTEGER NOT NULL CHECK (due_at >= 0),
    dispatch_seq INTEGER UNIQUE CHECK (dispatch_seq > 0),
    sound_state TEXT NOT NULL CHECK (sound_state IN ('pending','skipped','succeeded','failed')),
    sound_error_code TEXT,
    toast_state TEXT NOT NULL CHECK (toast_state IN ('pending','skipped','succeeded','failed')),
    toast_error_code TEXT,
    window_state TEXT NOT NULL CHECK (window_state IN ('pending','skipped','succeeded','failed')),
    window_error_code TEXT,
    first_dispatched_at INTEGER,
    last_dispatched_at INTEGER,
    acknowledged_at INTEGER,
    completed_at INTEGER,
    snoozed_until INTEGER,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
);
CREATE INDEX reminder_deliveries_due_idx ON reminder_deliveries(state, due_at, created_at, id);
CREATE INDEX reminder_deliveries_dispatch_idx ON reminder_deliveries(dispatch_seq);

CREATE TABLE reminder_consumer_cursors (
    consumer_id TEXT PRIMARY KEY,
    last_dispatch_seq INTEGER NOT NULL CHECK (last_dispatch_seq >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
);
