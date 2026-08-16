CREATE TABLE threshold_breaches_v7 (
    id TEXT PRIMARY KEY,
    threshold_id TEXT NOT NULL,
    breach_started_at INTEGER NOT NULL CHECK (breach_started_at >= 0),
    last_triggered_at INTEGER,
    cleared_at INTEGER,
    reminder_delivery_id TEXT REFERENCES reminder_deliveries(id) ON DELETE SET NULL,
    UNIQUE(threshold_id, breach_started_at)
);

INSERT INTO threshold_breaches_v7(
    id,
    threshold_id,
    breach_started_at,
    last_triggered_at,
    cleared_at,
    reminder_delivery_id
)
SELECT
    id,
    threshold_id,
    breach_started_at,
    last_triggered_at,
    cleared_at,
    reminder_delivery_id
FROM threshold_breaches;

DROP TABLE threshold_breaches;
ALTER TABLE threshold_breaches_v7 RENAME TO threshold_breaches;
