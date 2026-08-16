CREATE TABLE monitor_samples (
    id TEXT PRIMARY KEY,
    cpu_percent REAL NOT NULL CHECK (cpu_percent BETWEEN 0 AND 100),
    memory_used_bytes INTEGER NOT NULL CHECK (memory_used_bytes >= 0),
    memory_total_bytes INTEGER NOT NULL CHECK (memory_total_bytes > 0),
    disk_read_bps REAL NOT NULL CHECK (disk_read_bps >= 0),
    disk_write_bps REAL NOT NULL CHECK (disk_write_bps >= 0),
    network_rx_bps REAL NOT NULL CHECK (network_rx_bps >= 0),
    network_tx_bps REAL NOT NULL CHECK (network_tx_bps >= 0),
    gpu_percent REAL CHECK (gpu_percent BETWEEN 0 AND 100),
    sampled_at INTEGER NOT NULL CHECK (sampled_at >= 0)
);

CREATE INDEX monitor_samples_time_idx ON monitor_samples(sampled_at DESC, id DESC);

CREATE TABLE process_watches (
    id TEXT PRIMARY KEY,
    process_name TEXT NOT NULL COLLATE NOCASE UNIQUE,
    enabled INTEGER NOT NULL CHECK (enabled IN (0,1)),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
);

CREATE TABLE process_samples (
    id TEXT PRIMARY KEY,
    sample_id TEXT NOT NULL REFERENCES monitor_samples(id) ON DELETE CASCADE,
    process_watch_id TEXT NOT NULL REFERENCES process_watches(id) ON DELETE CASCADE,
    pid INTEGER NOT NULL CHECK (pid > 0),
    process_name TEXT NOT NULL,
    cpu_percent REAL NOT NULL CHECK (cpu_percent >= 0),
    memory_bytes INTEGER NOT NULL CHECK (memory_bytes >= 0),
    UNIQUE(sample_id, pid)
);

CREATE TABLE monitor_thresholds (
    id TEXT PRIMARY KEY,
    metric TEXT NOT NULL CHECK (metric IN ('cpuPercent','memoryPercent','diskReadBytesPerSecond','diskWriteBytesPerSecond','networkReceiveBytesPerSecond','networkSendBytesPerSecond','gpuPercent')),
    comparator TEXT NOT NULL CHECK (comparator IN ('greaterThanOrEqual','lessThanOrEqual')),
    threshold_value REAL NOT NULL,
    hold_seconds INTEGER NOT NULL CHECK (hold_seconds BETWEEN 0 AND 86400),
    cooldown_seconds INTEGER NOT NULL CHECK (cooldown_seconds BETWEEN 0 AND 604800),
    sound_json TEXT NOT NULL CHECK (json_valid(sound_json)),
    toast_enabled INTEGER NOT NULL CHECK (toast_enabled IN (0,1)),
    window_enabled INTEGER NOT NULL CHECK (window_enabled IN (0,1)),
    enabled INTEGER NOT NULL CHECK (enabled IN (0,1)),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
);

CREATE TABLE threshold_breaches (
    id TEXT PRIMARY KEY,
    threshold_id TEXT NOT NULL REFERENCES monitor_thresholds(id) ON DELETE CASCADE,
    breach_started_at INTEGER NOT NULL CHECK (breach_started_at >= 0),
    last_triggered_at INTEGER,
    cleared_at INTEGER,
    reminder_delivery_id TEXT REFERENCES reminder_deliveries(id) ON DELETE SET NULL,
    UNIQUE(threshold_id, breach_started_at)
);

CREATE TABLE notification_history (
    id TEXT PRIMARY KEY,
    origin TEXT NOT NULL CHECK (origin IN ('windows','aiceland')),
    app_id TEXT NOT NULL,
    source_entity_id TEXT NOT NULL,
    source_row_id INTEGER,
    title TEXT,
    body TEXT,
    message_key TEXT,
    message_parameters_json TEXT CHECK (message_parameters_json IS NULL OR json_valid(message_parameters_json)),
    source_context_json TEXT CHECK (source_context_json IS NULL OR json_valid(source_context_json)),
    source_occurred_at INTEGER NOT NULL CHECK (source_occurred_at > 0),
    received_at INTEGER NOT NULL CHECK (received_at >= 0),
    read_at INTEGER,
    removed_at INTEGER,
    UNIQUE(origin, source_entity_id),
    CHECK (
      (origin = 'windows' AND title IS NOT NULL AND body IS NOT NULL AND message_key IS NULL)
      OR
      (origin = 'aiceland' AND title IS NULL AND body IS NULL AND message_key IS NOT NULL
       AND message_parameters_json IS NOT NULL AND source_context_json IS NOT NULL)
    )
);

CREATE INDEX notification_history_visible_idx ON notification_history(removed_at, received_at DESC, id DESC);

CREATE TABLE notification_cursors (
    source_id TEXT PRIMARY KEY,
    last_row_id INTEGER NOT NULL CHECK (last_row_id >= 0),
    last_updated_at INTEGER NOT NULL CHECK (last_updated_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
);
