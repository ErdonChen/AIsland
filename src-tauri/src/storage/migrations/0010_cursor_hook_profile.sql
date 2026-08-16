INSERT OR IGNORE INTO agent_integration_profiles(
    id, kind, display_name, environment, config_target_json,
    event_mapping_json, enabled, revision, created_at, updated_at
) VALUES
    ('cursor-windows', 'preset', 'Cursor', 'windows',
     '{"kind":"preset","adapterId":"cursor"}',
     '[{"nativeEvent":"beforeSubmitPrompt","normalizedStatus":"running"},{"nativeEvent":"afterAgentResponse","normalizedStatus":"completed"}]',
     0, 1, 0, 0),
    ('cursor-wsl', 'preset', 'Cursor', 'wsl',
     '{"kind":"preset","adapterId":"cursor"}',
     '[{"nativeEvent":"beforeSubmitPrompt","normalizedStatus":"running"},{"nativeEvent":"afterAgentResponse","normalizedStatus":"completed"}]',
     0, 1, 0, 0);
