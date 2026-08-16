UPDATE agent_profile_observations
SET status = 'failed'
WHERE profile_id IN ('kimi-windows', 'kimi-wsl')
  AND status = 'idle'
  AND EXISTS (
      SELECT 1
      FROM agent_profile_events AS event
      WHERE event.profile_id = agent_profile_observations.profile_id
        AND event.event_id = agent_profile_observations.source_event_id
        AND event.native_event = 'Interrupt'
  );

UPDATE agent_profile_events
SET status = 'failed'
WHERE profile_id IN ('kimi-windows', 'kimi-wsl')
  AND native_event = 'Interrupt'
  AND status = 'idle';

UPDATE agent_integration_profiles
SET event_mapping_json = json_set(event_mapping_json, '$[5].normalizedStatus', 'failed'),
    revision = revision + 1
WHERE id IN ('kimi-windows', 'kimi-wsl')
  AND kind = 'preset'
  AND json_extract(event_mapping_json, '$[5].nativeEvent') = 'Interrupt'
  AND json_extract(event_mapping_json, '$[5].normalizedStatus') = 'idle';
