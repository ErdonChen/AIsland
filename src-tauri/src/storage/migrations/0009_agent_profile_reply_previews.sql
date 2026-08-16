ALTER TABLE agent_profile_observations
ADD COLUMN latest_reply_preview TEXT
CHECK (
    latest_reply_preview IS NULL
    OR (
        length(trim(latest_reply_preview)) BETWEEN 1 AND 1024
        AND latest_reply_preview = trim(latest_reply_preview)
    )
);
