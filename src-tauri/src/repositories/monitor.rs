use crate::contracts::{
    AppErrorCode, CommandError, DeleteResult, MonitorMetric, MonitorSnapshot, MonitorThreshold,
    ProcessMetric, ProcessWatch, ReminderSound, SafeMessageParameters, SaveMonitorThresholdInput,
    SaveProcessWatchInput, ThresholdComparator, TrueLiteral,
};
use crate::domain::monitor::{
    NewMonitorSample, NewProcessSample, ThresholdBreach, ThresholdBreachUpdate,
};
use crate::repositories::reminders::canonical_sound;
use crate::storage::Storage;
use rusqlite::{params, OptionalExtension, Row};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct MonitorRepository {
    storage: Arc<Storage>,
}

impl MonitorRepository {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    pub fn insert_sample(
        &self,
        sample: &NewMonitorSample,
        processes: &[NewProcessSample],
    ) -> Result<(), CommandError> {
        validate_sample(sample)?;
        if processes
            .iter()
            .any(|process| !valid_process_sample(process))
        {
            return Err(invalid_input());
        }
        self.storage.with_transaction(|tx| {
            let id = Uuid::new_v4().to_string();
            tx.execute("INSERT INTO monitor_samples(id,cpu_percent,memory_used_bytes,memory_total_bytes,disk_read_bps,disk_write_bps,network_rx_bps,network_tx_bps,gpu_percent,sampled_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", params![id, sample.cpu_percent, sample.memory_used_bytes, sample.memory_total_bytes, sample.disk_read_bps, sample.disk_write_bps, sample.network_rx_bps, sample.network_tx_bps, sample.gpu_percent, sample.sampled_at])?;
            for process in processes { tx.execute("INSERT INTO process_samples(id,sample_id,process_watch_id,pid,process_name,cpu_percent,memory_bytes) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![Uuid::new_v4().to_string(), id, process.process_watch_id.to_string(), process.pid, process.process_name, process.cpu_percent, process.memory_bytes])?; }
            tx.execute("DELETE FROM monitor_samples WHERE id IN (SELECT id FROM monitor_samples ORDER BY sampled_at DESC, id DESC LIMIT -1 OFFSET 3600)", [])?;
            Ok(())
        })
    }
    pub fn latest(&self) -> Result<Option<MonitorSnapshot>, CommandError> {
        self.storage.with_connection(|c| c.query_row("SELECT cpu_percent,memory_used_bytes,memory_total_bytes,disk_read_bps,disk_write_bps,network_rx_bps,network_tx_bps,gpu_percent,sampled_at FROM monitor_samples ORDER BY sampled_at DESC,id DESC LIMIT 1", [], row_to_snapshot).optional().map_err(Into::into))
    }
    pub fn list_samples(
        &self,
        since: i64,
        limit: u32,
    ) -> Result<Vec<MonitorSnapshot>, CommandError> {
        if since < 0 || limit == 0 || limit > 3600 {
            return Err(invalid_input());
        }
        self.storage.with_connection(|c| { let mut s=c.prepare("SELECT cpu_percent,memory_used_bytes,memory_total_bytes,disk_read_bps,disk_write_bps,network_rx_bps,network_tx_bps,gpu_percent,sampled_at FROM monitor_samples WHERE sampled_at >= ?1 ORDER BY sampled_at ASC,id ASC LIMIT ?2")?; let rows=s.query_map(params![since, limit], row_to_snapshot)?.collect::<Result<Vec<_>,_>>()?; Ok(rows) })
    }
    pub fn list_process_metrics(&self, limit: u32) -> Result<Vec<ProcessMetric>, CommandError> {
        if limit == 0 || limit > 3600 {
            return Err(invalid_input());
        }
        self.storage.with_connection(|c| { let mut s=c.prepare("SELECT p.pid,p.process_name,p.cpu_percent,p.memory_bytes,m.sampled_at FROM process_samples p JOIN monitor_samples m ON m.id=p.sample_id ORDER BY p.cpu_percent DESC,p.memory_bytes DESC,p.pid ASC LIMIT ?1")?; let rows=s.query_map([limit], |r| Ok(ProcessMetric {pid:r.get(0)?,process_name:r.get(1)?,cpu_percent:r.get(2)?,memory_bytes:r.get(3)?,sampled_at:r.get(4)?}))?.collect::<Result<Vec<_>,_>>()?; Ok(rows) })
    }
    pub fn list_process_watches(&self) -> Result<Vec<ProcessWatch>, CommandError> {
        self.storage.with_connection(|c| {let mut s=c.prepare("SELECT id,process_name,enabled,revision,updated_at FROM process_watches ORDER BY process_name COLLATE NOCASE,id")?; let rows=s.query_map([], row_to_watch)?.collect::<Result<Vec<_>,_>>()?; Ok(rows)})
    }
    pub fn save_process_watch(
        &self,
        input: SaveProcessWatchInput,
        now: i64,
    ) -> Result<ProcessWatch, CommandError> {
        if now < 0
            || !valid_watch_name(&input.process_name)
            || input.id.is_none() != input.expected_revision.is_none()
        {
            return Err(invalid_input());
        }
        self.storage.with_transaction(|tx| { let id=input.id.clone().unwrap_or_else(||Uuid::new_v4().to_string()); let changed=if let Some(revision)=input.expected_revision { tx.execute("UPDATE process_watches SET process_name=?2,enabled=?3,revision=revision+1,updated_at=?4 WHERE id=?1 AND revision=?5",params![id,input.process_name,input.enabled,now,revision])? } else { tx.execute("INSERT INTO process_watches(id,process_name,enabled,updated_at) VALUES(?1,?2,?3,?4)",params![id,input.process_name,input.enabled,now])? }; if changed==0{return Err(conflict())}; tx.query_row("SELECT id,process_name,enabled,revision,updated_at FROM process_watches WHERE id=?1",[id],row_to_watch).map_err(Into::into) })
    }
    pub fn delete_process_watch(
        &self,
        id: Uuid,
        expected_revision: u64,
    ) -> Result<DeleteResult, CommandError> {
        self.storage.with_transaction(|tx| {
            let value = id.to_string();
            let n = tx.execute(
                "DELETE FROM process_watches WHERE id=?1 AND revision=?2",
                params![
                    value,
                    i64::try_from(expected_revision).map_err(|_| invalid_input())?
                ],
            )?;
            if n == 0 {
                return Err(
                    if tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM process_watches WHERE id=?1)",
                        [value.as_str()],
                        |r| r.get(0),
                    )? {
                        conflict()
                    } else {
                        not_found()
                    },
                );
            };
            Ok(DeleteResult {
                id: value,
                deleted: TrueLiteral,
            })
        })
    }
    pub fn list_thresholds(&self) -> Result<Vec<MonitorThreshold>, CommandError> {
        self.storage.with_connection(|c|{let mut s=c.prepare("SELECT id,metric,comparator,threshold_value,hold_seconds,cooldown_seconds,sound_json,toast_enabled,window_enabled,enabled,revision,updated_at FROM monitor_thresholds ORDER BY updated_at DESC,id DESC")?;let rows=s.query_map([],row_to_threshold)?.collect::<Result<Vec<_>,_>>()?;Ok(rows)})
    }
    pub fn list_enabled_thresholds(&self) -> Result<Vec<MonitorThreshold>, CommandError> {
        self.storage.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id,metric,comparator,threshold_value,hold_seconds,cooldown_seconds,sound_json,toast_enabled,window_enabled,enabled,revision,updated_at FROM monitor_thresholds WHERE enabled=1 ORDER BY id ASC",
            )?;
            let rows = statement
                .query_map([], row_to_threshold)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }
    pub fn save_threshold(
        &self,
        input: SaveMonitorThresholdInput,
        now: i64,
    ) -> Result<MonitorThreshold, CommandError> {
        validate_threshold(&input, now)?;
        let sound = canonical_sound(&input.sound)?;
        let sound_json = serde_json::to_string(&sound).map_err(|_| invalid_input())?;
        self.storage.with_transaction(|tx|{let id=input.id.clone().unwrap_or_else(||Uuid::new_v4().to_string());let n=if let Some(revision)=input.expected_revision{tx.execute("UPDATE monitor_thresholds SET metric=?2,comparator=?3,threshold_value=?4,hold_seconds=?5,cooldown_seconds=?6,sound_json=?7,toast_enabled=?8,window_enabled=?9,enabled=?10,revision=revision+1,updated_at=?11 WHERE id=?1 AND revision=?12",params![id,metric_name(&input.metric),comparator_name(&input.comparator),input.threshold_value,input.hold_seconds,input.cooldown_seconds,sound_json,input.toast_enabled,input.window_enabled,input.enabled,now,revision])?}else{tx.execute("INSERT INTO monitor_thresholds(id,metric,comparator,threshold_value,hold_seconds,cooldown_seconds,sound_json,toast_enabled,window_enabled,enabled,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![id,metric_name(&input.metric),comparator_name(&input.comparator),input.threshold_value,input.hold_seconds,input.cooldown_seconds,sound_json,input.toast_enabled,input.window_enabled,input.enabled,now])?};if n==0{return Err(conflict())};tx.query_row("SELECT id,metric,comparator,threshold_value,hold_seconds,cooldown_seconds,sound_json,toast_enabled,window_enabled,enabled,revision,updated_at FROM monitor_thresholds WHERE id=?1",[id],row_to_threshold).map_err(Into::into)})
    }
    pub fn delete_threshold(
        &self,
        id: Uuid,
        expected_revision: u64,
        _now: i64,
    ) -> Result<DeleteResult, CommandError> {
        self.storage.with_transaction(|tx| {
            let value = id.to_string();
            let n = tx.execute(
                "DELETE FROM monitor_thresholds WHERE id=?1 AND revision=?2",
                params![
                    value,
                    i64::try_from(expected_revision).map_err(|_| invalid_input())?
                ],
            )?;
            if n == 0 {
                return Err(
                    if tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM monitor_thresholds WHERE id=?1)",
                        [value.as_str()],
                        |r| r.get(0),
                    )? {
                        conflict()
                    } else {
                        not_found()
                    },
                );
            };
            Ok(DeleteResult {
                id: value,
                deleted: TrueLiteral,
            })
        })
    }
    pub fn update_breach(
        &self,
        update: ThresholdBreachUpdate,
    ) -> Result<ThresholdBreach, CommandError> {
        if update.breach_started_at < 0
            || update.last_triggered_at.is_some_and(|v| v < 0)
            || update.cleared_at.is_some_and(|v| v < 0)
        {
            return Err(invalid_input());
        };
        self.storage.with_transaction(|tx| {
            let threshold_id = update.threshold_id.to_string();
            let threshold_exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM monitor_thresholds WHERE id=?1)",
                [threshold_id.as_str()],
                |row| row.get(0),
            )?;
            if !threshold_exists {
                return Err(not_found());
            }

            let id = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO threshold_breaches(
                    id,threshold_id,breach_started_at,last_triggered_at,cleared_at,reminder_delivery_id
                 ) VALUES(?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(threshold_id,breach_started_at) DO UPDATE SET
                    last_triggered_at=excluded.last_triggered_at,
                    cleared_at=excluded.cleared_at,
                    reminder_delivery_id=excluded.reminder_delivery_id",
                params![
                    id,
                    threshold_id,
                    update.breach_started_at,
                    update.last_triggered_at,
                    update.cleared_at,
                    update.reminder_delivery_id.map(|value| value.to_string())
                ],
            )?;
            tx.query_row(
                "SELECT id,threshold_id,breach_started_at,last_triggered_at,cleared_at,reminder_delivery_id
                 FROM threshold_breaches WHERE threshold_id=?1 AND breach_started_at=?2",
                params![threshold_id, update.breach_started_at],
                row_to_breach,
            )
            .map_err(Into::into)
        })
    }

    pub fn latest_breach(
        &self,
        threshold_id: Uuid,
    ) -> Result<Option<ThresholdBreach>, CommandError> {
        self.storage.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id,threshold_id,breach_started_at,last_triggered_at,cleared_at,reminder_delivery_id FROM threshold_breaches WHERE threshold_id=?1 ORDER BY breach_started_at DESC,id DESC LIMIT 1",
                    [threshold_id.to_string()],
                    row_to_breach,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn latest_triggered_before(
        &self,
        threshold_id: Uuid,
        breach_started_at: i64,
    ) -> Result<Option<ThresholdBreach>, CommandError> {
        self.storage.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id,threshold_id,breach_started_at,last_triggered_at,cleared_at,reminder_delivery_id FROM threshold_breaches WHERE threshold_id=?1 AND breach_started_at<?2 AND last_triggered_at IS NOT NULL ORDER BY breach_started_at DESC,id DESC LIMIT 1",
                    params![threshold_id.to_string(), breach_started_at],
                    row_to_breach,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn list_breaches(&self, threshold_id: Uuid) -> Result<Vec<ThresholdBreach>, CommandError> {
        self.storage.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id,threshold_id,breach_started_at,last_triggered_at,cleared_at,reminder_delivery_id FROM threshold_breaches WHERE threshold_id=?1 ORDER BY breach_started_at ASC,id ASC",
            )?;
            let rows = statement
                .query_map([threshold_id.to_string()], row_to_breach)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn list_pending_delivery_source_ids(&self) -> Result<Vec<Uuid>, CommandError> {
        self.storage.with_connection(|connection| {
            let mut statement = connection.prepare(
                r#"SELECT DISTINCT source_entity_id FROM reminder_deliveries
                   WHERE source_kind='monitor' AND state IN ('pending','snoozed')
                   ORDER BY source_entity_id"#,
            )?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .map(|row| {
                    let id = row?;
                    Uuid::parse_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(CommandError::from)?;
            Ok(rows)
        })
    }
}
fn row_to_snapshot(r: &Row<'_>) -> rusqlite::Result<MonitorSnapshot> {
    Ok(MonitorSnapshot {
        cpu_percent: r.get::<_, f64>(0)? as i64,
        memory_used_bytes: r.get(1)?,
        memory_total_bytes: r.get(2)?,
        disk_read_bytes_per_second: r.get::<_, f64>(3)? as i64,
        disk_write_bytes_per_second: r.get::<_, f64>(4)? as i64,
        network_receive_bytes_per_second: r.get::<_, f64>(5)? as i64,
        network_send_bytes_per_second: r.get::<_, f64>(6)? as i64,
        gpu_percent: r.get::<_, Option<f64>>(7)?.map(|v| v as i64),
        sampled_at: r.get(8)?,
    })
}
fn row_to_watch(r: &Row<'_>) -> rusqlite::Result<ProcessWatch> {
    Ok(ProcessWatch {
        id: r.get(0)?,
        process_name: r.get(1)?,
        enabled: r.get(2)?,
        revision: r.get(3)?,
        updated_at: r.get(4)?,
    })
}
fn row_to_threshold(r: &Row<'_>) -> rusqlite::Result<MonitorThreshold> {
    let metric: String = r.get(1)?;
    let comparator: String = r.get(2)?;
    let sound: String = r.get(6)?;
    Ok(MonitorThreshold {
        id: r.get(0)?,
        metric: parse_metric(&metric).ok_or(rusqlite::Error::InvalidQuery)?,
        comparator: parse_comparator(&comparator).ok_or(rusqlite::Error::InvalidQuery)?,
        threshold_value: r.get(3)?,
        hold_seconds: r.get(4)?,
        cooldown_seconds: r.get(5)?,
        sound: serde_json::from_str(&sound).map_err(|_| rusqlite::Error::InvalidQuery)?,
        toast_enabled: r.get(7)?,
        window_enabled: r.get(8)?,
        enabled: r.get(9)?,
        revision: r.get(10)?,
        updated_at: r.get(11)?,
    })
}
fn row_to_breach(r: &Row<'_>) -> rusqlite::Result<ThresholdBreach> {
    Ok(ThresholdBreach {
        id: r.get(0)?,
        threshold_id: r.get(1)?,
        breach_started_at: r.get(2)?,
        last_triggered_at: r.get(3)?,
        cleared_at: r.get(4)?,
        reminder_delivery_id: r.get(5)?,
    })
}
fn validate_sample(s: &NewMonitorSample) -> Result<(), CommandError> {
    if !s.cpu_percent.is_finite()
        || !(0.0..=100.0).contains(&s.cpu_percent)
        || s.memory_used_bytes < 0
        || s.memory_total_bytes <= 0
        || s.sampled_at < 0
        || [
            s.disk_read_bps,
            s.disk_write_bps,
            s.network_rx_bps,
            s.network_tx_bps,
        ]
        .into_iter()
        .any(|v| !v.is_finite() || v < 0.0)
        || s.gpu_percent
            .is_some_and(|v| !v.is_finite() || !(0.0..=100.0).contains(&v))
    {
        Err(invalid_input())
    } else {
        Ok(())
    }
}
fn valid_process_sample(p: &NewProcessSample) -> bool {
    p.pid > 0
        && valid_watch_name(&p.process_name)
        && p.cpu_percent.is_finite()
        && p.cpu_percent >= 0.0
        && p.memory_bytes >= 0
}
fn valid_watch_name(v: &str) -> bool {
    let bytes = v.len();
    (1..=260).contains(&bytes)
        && !v
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | '*' | '?'))
        && !v.contains(':')
}
fn validate_threshold(i: &SaveMonitorThresholdInput, now: i64) -> Result<(), CommandError> {
    if now < 0
        || i.id.is_none() != i.expected_revision.is_none()
        || !i.threshold_value.is_finite()
        || i.hold_seconds < 0
        || i.hold_seconds > 86400
        || i.cooldown_seconds < 0
        || i.cooldown_seconds > 604800
        || (!i.toast_enabled && !i.window_enabled && matches!(i.sound, ReminderSound::None))
        || matches!(
            i.metric,
            MonitorMetric::CpuPercent | MonitorMetric::MemoryPercent | MonitorMetric::GpuPercent
        ) && !(0.0..=100.0).contains(&i.threshold_value)
    {
        Err(invalid_input())
    } else {
        Ok(())
    }
}
fn metric_name(v: &MonitorMetric) -> &'static str {
    match v {
        MonitorMetric::CpuPercent => "cpuPercent",
        MonitorMetric::MemoryPercent => "memoryPercent",
        MonitorMetric::DiskReadBytesPerSecond => "diskReadBytesPerSecond",
        MonitorMetric::DiskWriteBytesPerSecond => "diskWriteBytesPerSecond",
        MonitorMetric::NetworkReceiveBytesPerSecond => "networkReceiveBytesPerSecond",
        MonitorMetric::NetworkSendBytesPerSecond => "networkSendBytesPerSecond",
        MonitorMetric::GpuPercent => "gpuPercent",
    }
}
fn parse_metric(v: &str) -> Option<MonitorMetric> {
    Some(match v {
        "cpuPercent" => MonitorMetric::CpuPercent,
        "memoryPercent" => MonitorMetric::MemoryPercent,
        "diskReadBytesPerSecond" => MonitorMetric::DiskReadBytesPerSecond,
        "diskWriteBytesPerSecond" => MonitorMetric::DiskWriteBytesPerSecond,
        "networkReceiveBytesPerSecond" => MonitorMetric::NetworkReceiveBytesPerSecond,
        "networkSendBytesPerSecond" => MonitorMetric::NetworkSendBytesPerSecond,
        "gpuPercent" => MonitorMetric::GpuPercent,
        _ => return None,
    })
}
fn comparator_name(v: &ThresholdComparator) -> &'static str {
    match v {
        ThresholdComparator::GreaterThanOrEqual => "greaterThanOrEqual",
        ThresholdComparator::LessThanOrEqual => "lessThanOrEqual",
    }
}
fn parse_comparator(v: &str) -> Option<ThresholdComparator> {
    Some(match v {
        "greaterThanOrEqual" => ThresholdComparator::GreaterThanOrEqual,
        "lessThanOrEqual" => ThresholdComparator::LessThanOrEqual,
        _ => return None,
    })
}
fn invalid_input() -> CommandError {
    CommandError {
        code: AppErrorCode::InvalidInput,
        message_key: "errors.invalidInput".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}
fn conflict() -> CommandError {
    CommandError {
        code: AppErrorCode::Conflict,
        message_key: "errors.conflict".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}
fn not_found() -> CommandError {
    CommandError {
        code: AppErrorCode::NotFound,
        message_key: "errors.notFound".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::SaveProcessWatchInput;
    use crate::storage::Storage;
    use std::sync::Arc;

    fn repository() -> MonitorRepository {
        let d = tempfile::tempdir().unwrap();
        let repository = MonitorRepository::new(Arc::new(Storage::open(d.path()).unwrap()));
        std::mem::forget(d);
        repository
    }

    // Break caught: migration five must apply once and remain in its ledger on reopen.
    #[test]
    fn migration_five_is_recorded_when_storage_opens() {
        let d = tempfile::tempdir().unwrap();
        let s = Storage::open(d.path()).unwrap();
        assert_eq!(s.schema_version().unwrap(), 11);
        drop(s);
        Storage::open(d.path()).unwrap().with_connection(|c| {
            assert_eq!(c.query_row("SELECT COUNT(*) FROM schema_migrations WHERE version=5 AND name='monitor_notifications'", [], |r| r.get::<_, i64>(0))?, 1);
            Ok(())
        }).unwrap();
    }

    // Break caught: a failing process child insert must not commit the sample parent.
    #[test]
    fn sample_and_process_insert_is_atomic() {
        let repo = repository();
        let sample = NewMonitorSample {
            cpu_percent: 1.0,
            memory_used_bytes: 1,
            memory_total_bytes: 2,
            disk_read_bps: 0.0,
            disk_write_bps: 0.0,
            network_rx_bps: 0.0,
            network_tx_bps: 0.0,
            gpu_percent: None,
            sampled_at: 1,
        };
        assert!(repo
            .insert_sample(
                &sample,
                &[NewProcessSample {
                    process_watch_id: Uuid::new_v4(),
                    pid: 1,
                    process_name: "app.exe".into(),
                    cpu_percent: 1.0,
                    memory_bytes: 1
                }]
            )
            .is_err());
        repo.storage
            .with_connection(|c| {
                assert_eq!(
                    c.query_row("SELECT COUNT(*) FROM monitor_samples", [], |r| r
                        .get::<_, i64>(0))?,
                    0
                );
                Ok(())
            })
            .unwrap();
    }

    // Break caught: process samples must reject every non-base-executable name before the
    // parent sample, child rows, or retention query can mutate storage.
    #[test]
    fn invalid_process_sample_names_leave_storage_unchanged() {
        let repo = repository();
        let watch = repo
            .save_process_watch(
                SaveProcessWatchInput {
                    id: None,
                    process_name: "app.exe".into(),
                    enabled: true,
                    expected_revision: None,
                },
                1,
            )
            .unwrap();
        let sample = NewMonitorSample {
            cpu_percent: 1.0,
            memory_used_bytes: 1,
            memory_total_bytes: 2,
            disk_read_bps: 0.0,
            disk_write_bps: 0.0,
            network_rx_bps: 0.0,
            network_tx_bps: 0.0,
            gpu_percent: None,
            sampled_at: 1,
        };
        let baseline = || {
            repo.storage
                .with_connection(|c| {
                    Ok((
                        c.query_row("SELECT COUNT(*) FROM monitor_samples", [], |r| {
                            r.get::<_, i64>(0)
                        })?,
                        c.query_row("SELECT COUNT(*) FROM process_samples", [], |r| {
                            r.get::<_, i64>(0)
                        })?,
                    ))
                })
                .unwrap()
        };
        for name in [
            "C:\\bad.exe".to_owned(),
            "bad*.exe".to_owned(),
            "bad\n.exe".to_owned(),
            "x".repeat(261),
        ] {
            assert!(repo
                .insert_sample(
                    &sample,
                    &[NewProcessSample {
                        process_watch_id: Uuid::parse_str(&watch.id).unwrap(),
                        pid: 1,
                        process_name: name,
                        cpu_percent: 1.0,
                        memory_bytes: 1
                    }]
                )
                .is_err());
            assert_eq!(baseline(), (0, 0));
        }
    }

    // Break caught: stale watch updates must conflict and ordering must be case-insensitive.
    #[test]
    fn watch_updates_are_revision_safe_and_ordered() {
        let repo = repository();
        let z = repo
            .save_process_watch(
                SaveProcessWatchInput {
                    id: None,
                    process_name: "z.exe".into(),
                    enabled: true,
                    expected_revision: None,
                },
                1,
            )
            .unwrap();
        let a = repo
            .save_process_watch(
                SaveProcessWatchInput {
                    id: None,
                    process_name: "Alpha.exe".into(),
                    enabled: true,
                    expected_revision: None,
                },
                2,
            )
            .unwrap();
        let changed = repo
            .save_process_watch(
                SaveProcessWatchInput {
                    id: Some(z.id.clone()),
                    process_name: "z.exe".into(),
                    enabled: false,
                    expected_revision: Some(z.revision),
                },
                3,
            )
            .unwrap();
        assert!(repo
            .save_process_watch(
                SaveProcessWatchInput {
                    id: Some(z.id),
                    process_name: "z.exe".into(),
                    enabled: true,
                    expected_revision: Some(1)
                },
                4
            )
            .is_err());
        assert_eq!(
            repo.list_process_watches()
                .unwrap()
                .into_iter()
                .map(|v| v.id)
                .collect::<Vec<_>>(),
            vec![a.id, changed.id]
        );
    }

    #[test]
    fn retention_process_order_thresholds_and_breaches_follow_the_persistence_contract() {
        let repo = repository();
        let watch = repo
            .save_process_watch(
                SaveProcessWatchInput {
                    id: None,
                    process_name: "app.exe".into(),
                    enabled: true,
                    expected_revision: None,
                },
                1,
            )
            .unwrap();
        let watch_id = Uuid::parse_str(&watch.id).unwrap();
        for time in 0..3601 {
            repo.insert_sample(
                &NewMonitorSample {
                    cpu_percent: 1.0,
                    memory_used_bytes: 1,
                    memory_total_bytes: 2,
                    disk_read_bps: 0.0,
                    disk_write_bps: 0.0,
                    network_rx_bps: 0.0,
                    network_tx_bps: 0.0,
                    gpu_percent: None,
                    sampled_at: time,
                },
                &[NewProcessSample {
                    process_watch_id: watch_id,
                    pid: 1,
                    process_name: "app.exe".into(),
                    cpu_percent: time as f64,
                    memory_bytes: time,
                }],
            )
            .unwrap();
        }
        let retained = repo.list_samples(0, 3600).unwrap();
        assert_eq!(
            (
                retained.len(),
                retained[0].sampled_at,
                retained.last().unwrap().sampled_at
            ),
            (3600, 1, 3600)
        );
        let metrics = repo.list_process_metrics(3).unwrap();
        assert!(metrics
            .windows(2)
            .all(|pair| pair[0].cpu_percent >= pair[1].cpu_percent));
        let sound = ReminderSound::Builtin {
            sound_id: crate::contracts::BuiltinReminderSoundId::SystemNotification,
        };
        let create = |value, _now| SaveMonitorThresholdInput {
            metric: MonitorMetric::CpuPercent,
            comparator: ThresholdComparator::GreaterThanOrEqual,
            threshold_value: value,
            hold_seconds: 0,
            cooldown_seconds: 0,
            sound: sound.clone(),
            toast_enabled: false,
            window_enabled: false,
            enabled: true,
            id: None,
            expected_revision: None,
        };
        let first = repo.save_threshold(create(90.0, 10), 10).unwrap();
        let second = repo.save_threshold(create(80.0, 11), 11).unwrap();
        assert_eq!(
            repo.list_thresholds()
                .unwrap()
                .into_iter()
                .map(|v| v.id)
                .collect::<Vec<_>>(),
            vec![second.id.clone(), first.id.clone()]
        );
        let updated = repo
            .save_threshold(
                SaveMonitorThresholdInput {
                    id: Some(first.id.clone()),
                    expected_revision: Some(first.revision),
                    ..create(91.0, 12)
                },
                12,
            )
            .unwrap();
        assert!(repo
            .save_threshold(
                SaveMonitorThresholdInput {
                    id: Some(first.id.clone()),
                    expected_revision: Some(first.revision),
                    ..create(92.0, 13)
                },
                13
            )
            .is_err());
        let breach = repo
            .update_breach(ThresholdBreachUpdate {
                threshold_id: Uuid::parse_str(&updated.id).unwrap(),
                breach_started_at: 12,
                last_triggered_at: Some(13),
                cleared_at: None,
                reminder_delivery_id: None,
            })
            .unwrap();
        let cleared = repo
            .update_breach(ThresholdBreachUpdate {
                threshold_id: Uuid::parse_str(&updated.id).unwrap(),
                breach_started_at: 12,
                last_triggered_at: Some(14),
                cleared_at: Some(15),
                reminder_delivery_id: None,
            })
            .unwrap();
        assert_eq!(
            (breach.id, cleared.last_triggered_at, cleared.cleared_at),
            (cleared.id, Some(14), Some(15))
        );
        assert!(repo
            .delete_threshold(Uuid::parse_str(&updated.id).unwrap(), 1, 16)
            .is_err());
        repo.delete_threshold(
            Uuid::parse_str(&updated.id).unwrap(),
            updated.revision as u64,
            16,
        )
        .unwrap();
        repo.delete_process_watch(watch_id, watch.revision as u64)
            .unwrap();
        repo.storage
            .with_connection(|c| {
                assert_eq!(
                    c.query_row("SELECT COUNT(*) FROM process_samples", [], |r| r
                        .get::<_, i64>(0))?,
                    0
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn deleted_threshold_rejects_new_breaches_without_removing_history() {
        let repo = repository();
        let threshold = repo
            .save_threshold(
                SaveMonitorThresholdInput {
                    metric: MonitorMetric::CpuPercent,
                    comparator: ThresholdComparator::GreaterThanOrEqual,
                    threshold_value: 80.0,
                    hold_seconds: 0,
                    cooldown_seconds: 0,
                    sound: ReminderSound::Builtin {
                        sound_id: crate::contracts::BuiltinReminderSoundId::SystemNotification,
                    },
                    toast_enabled: false,
                    window_enabled: true,
                    enabled: true,
                    id: None,
                    expected_revision: None,
                },
                10,
            )
            .unwrap();
        let threshold_id = Uuid::parse_str(&threshold.id).unwrap();
        repo.update_breach(ThresholdBreachUpdate {
            threshold_id,
            breach_started_at: 11,
            last_triggered_at: Some(12),
            cleared_at: None,
            reminder_delivery_id: None,
        })
        .unwrap();
        repo.delete_threshold(threshold_id, threshold.revision as u64, 13)
            .unwrap();

        assert_eq!(repo.list_breaches(threshold_id).unwrap().len(), 1);
        let error = repo
            .update_breach(ThresholdBreachUpdate {
                threshold_id,
                breach_started_at: 14,
                last_triggered_at: Some(15),
                cleared_at: None,
                reminder_delivery_id: None,
            })
            .unwrap_err();
        assert_eq!(error.code, AppErrorCode::NotFound);
        assert_eq!(repo.list_breaches(threshold_id).unwrap().len(), 1);
    }
}
