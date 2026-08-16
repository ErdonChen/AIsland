CREATE TABLE agent_integration_profiles (
    id TEXT PRIMARY KEY NOT NULL
        CHECK(length(id) BETWEEN 1 AND 64)
        CHECK(id = lower(id)),
    kind TEXT NOT NULL CHECK(kind IN ('preset', 'custom')),
    display_name TEXT NOT NULL
        CHECK(length(trim(display_name)) BETWEEN 1 AND 64)
        CHECK(display_name = trim(display_name)),
    environment TEXT NOT NULL CHECK(environment IN ('windows', 'wsl')),
    config_target_json TEXT NOT NULL CHECK(json_valid(config_target_json)),
    event_mapping_json TEXT NOT NULL CHECK(json_valid(event_mapping_json)),
    enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
    revision INTEGER NOT NULL CHECK(revision >= 1),
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    updated_at INTEGER NOT NULL CHECK(updated_at >= created_at)
);

CREATE TABLE agent_profile_installations (
    profile_id TEXT PRIMARY KEY NOT NULL REFERENCES agent_integration_profiles(id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK(state IN ('notInstalled', 'installed', 'needsRepair', 'unsupported')),
    reason_code TEXT,
    owned_resource TEXT,
    owned_fingerprint TEXT,
    external_hash TEXT,
    updated_at INTEGER NOT NULL CHECK(updated_at >= 0)
);

CREATE TABLE agent_profile_events (
    profile_id TEXT NOT NULL REFERENCES agent_integration_profiles(id) ON DELETE CASCADE,
    event_id TEXT NOT NULL,
    native_event TEXT NOT NULL,
    task_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('idle','running','completed','failed','waiting','timeout','offline')),
    occurred_at INTEGER NOT NULL CHECK(occurred_at >= 0),
    received_at INTEGER NOT NULL CHECK(received_at >= 0),
    PRIMARY KEY(profile_id, event_id)
);
CREATE INDEX agent_profile_events_profile_time_idx
    ON agent_profile_events(profile_id, occurred_at DESC, event_id DESC);

CREATE TABLE agent_profile_observations (
    profile_id TEXT NOT NULL REFERENCES agent_integration_profiles(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('idle','running','completed','failed','waiting','timeout','offline')),
    source_event_id TEXT NOT NULL,
    occurred_at INTEGER NOT NULL CHECK(occurred_at >= 0),
    received_at INTEGER NOT NULL CHECK(received_at >= 0),
    PRIMARY KEY(profile_id, task_id)
);

INSERT INTO agent_integration_profiles(
    id, kind, display_name, environment, config_target_json,
    event_mapping_json, enabled, revision, created_at, updated_at
) VALUES
    ('kimi-windows', 'preset', 'Kimi Code', 'windows',
     '{"kind":"preset","adapterId":"kimi"}',
     '[{"nativeEvent":"UserPromptSubmit","normalizedStatus":"running"},{"nativeEvent":"PermissionRequest","normalizedStatus":"waiting"},{"nativeEvent":"PermissionResult","normalizedStatus":"running"},{"nativeEvent":"Stop","normalizedStatus":"completed"},{"nativeEvent":"StopFailure","normalizedStatus":"failed"},{"nativeEvent":"Interrupt","normalizedStatus":"idle"},{"nativeEvent":"SessionEnd","normalizedStatus":"offline"}]',
     0, 1, 0, 0),
    ('kimi-wsl', 'preset', 'Kimi Code', 'wsl',
     '{"kind":"preset","adapterId":"kimi"}',
     '[{"nativeEvent":"UserPromptSubmit","normalizedStatus":"running"},{"nativeEvent":"PermissionRequest","normalizedStatus":"waiting"},{"nativeEvent":"PermissionResult","normalizedStatus":"running"},{"nativeEvent":"Stop","normalizedStatus":"completed"},{"nativeEvent":"StopFailure","normalizedStatus":"failed"},{"nativeEvent":"Interrupt","normalizedStatus":"idle"},{"nativeEvent":"SessionEnd","normalizedStatus":"offline"}]',
     0, 1, 0, 0),
    ('qoderwork-windows', 'preset', 'QoderWork', 'windows',
     '{"kind":"preset","adapterId":"qoderwork"}',
     '[{"nativeEvent":"UserPromptSubmit","normalizedStatus":"running"},{"nativeEvent":"Stop","normalizedStatus":"completed"},{"nativeEvent":"SessionEnd","normalizedStatus":"offline"}]',
     0, 1, 0, 0),
    ('qoderwork-wsl', 'preset', 'QoderWork', 'wsl',
     '{"kind":"preset","adapterId":"qoderwork"}',
     '[{"nativeEvent":"UserPromptSubmit","normalizedStatus":"running"},{"nativeEvent":"Stop","normalizedStatus":"completed"},{"nativeEvent":"SessionEnd","normalizedStatus":"offline"}]',
     0, 1, 0, 0),
    ('trae-windows', 'preset', 'TRAE', 'windows',
     '{"kind":"preset","adapterId":"trae"}',
     '[{"nativeEvent":"Stop","normalizedStatus":"completed"}]',
     0, 1, 0, 0),
    ('trae-wsl', 'preset', 'TRAE', 'wsl',
     '{"kind":"preset","adapterId":"trae"}',
     '[{"nativeEvent":"Stop","normalizedStatus":"completed"}]',
     0, 1, 0, 0);
