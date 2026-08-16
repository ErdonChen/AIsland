use crate::contracts::{
    AppErrorCode, CommandError, DeleteResult, MonitorMetric, MonitorSnapshot, MonitorThreshold,
    ProcessMetric, ProcessWatch, ReminderSound, SafeMessageParameters, SafeParameterValue,
    SaveMonitorThresholdInput, SaveProcessWatchInput, ThresholdComparator,
};
use crate::repositories::monitor::MonitorRepository;
use crate::services::{threshold_evaluator::MonitorThresholdService, AppServices};
use std::sync::Arc;
use uuid::Uuid;

#[tauri::command(rename = "getMonitorSnapshot", rename_all = "camelCase")]
pub fn get_monitor_snapshot(
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<MonitorSnapshot, CommandError> {
    get_monitor_snapshot_with(&services.monitor)
}

#[tauri::command(rename = "listMonitorSamples", rename_all = "camelCase")]
pub fn list_monitor_samples(
    since: i64,
    limit: i64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<Vec<MonitorSnapshot>, CommandError> {
    let (since, limit) = validate_sample_query(since, limit)?;
    services.monitor.list_samples(since, limit)
}

#[tauri::command(rename = "listProcessMetrics", rename_all = "camelCase")]
pub fn list_process_metrics(
    limit: i64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<Vec<ProcessMetric>, CommandError> {
    services
        .monitor
        .list_process_metrics(validate_process_limit(limit)?)
}

#[tauri::command(rename = "listProcessWatches", rename_all = "camelCase")]
pub fn list_process_watches(
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<Vec<ProcessWatch>, CommandError> {
    services.monitor.list_process_watches()
}

#[tauri::command(rename = "saveProcessWatch", rename_all = "camelCase")]
pub fn save_process_watch(
    id: Option<Uuid>,
    process_name: String,
    enabled: bool,
    expected_revision: Option<u64>,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<ProcessWatch, CommandError> {
    let input = process_watch_input(id, process_name, enabled, expected_revision)?;
    services.monitor.save_process_watch(input, now_millis())
}

#[tauri::command(rename = "deleteProcessWatch", rename_all = "camelCase")]
pub fn delete_process_watch(
    id: Uuid,
    expected_revision: u64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<DeleteResult, CommandError> {
    validate_revision(expected_revision)?;
    services.monitor.delete_process_watch(id, expected_revision)
}

#[tauri::command(rename = "listMonitorThresholds", rename_all = "camelCase")]
pub fn list_monitor_thresholds(
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<Vec<MonitorThreshold>, CommandError> {
    services.monitor.list_thresholds()
}

#[allow(clippy::too_many_arguments)]
#[tauri::command(rename = "saveMonitorThreshold", rename_all = "camelCase")]
pub fn save_monitor_threshold(
    metric: MonitorMetric,
    comparator: ThresholdComparator,
    threshold_value: f64,
    hold_seconds: i64,
    cooldown_seconds: i64,
    sound: ReminderSound,
    toast_enabled: bool,
    window_enabled: bool,
    enabled: bool,
    id: Option<Uuid>,
    expected_revision: Option<u64>,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<MonitorThreshold, CommandError> {
    let input = threshold_input(
        metric,
        comparator,
        threshold_value,
        hold_seconds,
        cooldown_seconds,
        sound,
        toast_enabled,
        window_enabled,
        enabled,
        id,
        expected_revision,
    )?;
    save_monitor_threshold_with(&services.monitor_thresholds, input, now_millis())
}

#[tauri::command(rename = "deleteMonitorThreshold", rename_all = "camelCase")]
pub fn delete_monitor_threshold(
    id: Uuid,
    expected_revision: u64,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<DeleteResult, CommandError> {
    validate_revision(expected_revision)?;
    services
        .monitor_thresholds
        .delete(id, expected_revision, now_millis())
}

fn get_monitor_snapshot_with(
    repository: &MonitorRepository,
) -> Result<MonitorSnapshot, CommandError> {
    repository.latest()?.ok_or_else(no_snapshot_error)
}

fn save_monitor_threshold_with(
    service: &MonitorThresholdService,
    input: SaveMonitorThresholdInput,
    now: i64,
) -> Result<MonitorThreshold, CommandError> {
    service.save(input, now)
}

fn validate_sample_query(since: i64, limit: i64) -> Result<(i64, u32), CommandError> {
    if since < 0 || !(1..=3_600).contains(&limit) {
        return Err(invalid_input("sampleQuery"));
    }
    Ok((since, limit as u32))
}

fn validate_process_limit(limit: i64) -> Result<u32, CommandError> {
    if !(1..=500).contains(&limit) {
        return Err(invalid_input("processLimit"));
    }
    Ok(limit as u32)
}

fn process_watch_input(
    id: Option<Uuid>,
    process_name: String,
    enabled: bool,
    expected_revision: Option<u64>,
) -> Result<SaveProcessWatchInput, CommandError> {
    if id.is_none() != expected_revision.is_none()
        || !valid_process_name(&process_name)
        || expected_revision.is_some_and(|revision| validate_revision(revision).is_err())
    {
        return Err(invalid_input("processWatch"));
    }
    Ok(SaveProcessWatchInput {
        id: id.map(|value| value.to_string()),
        process_name,
        enabled,
        expected_revision: expected_revision.map(|value| value as i64),
    })
}

#[allow(clippy::too_many_arguments)]
fn threshold_input(
    metric: MonitorMetric,
    comparator: ThresholdComparator,
    threshold_value: f64,
    hold_seconds: i64,
    cooldown_seconds: i64,
    sound: ReminderSound,
    toast_enabled: bool,
    window_enabled: bool,
    enabled: bool,
    id: Option<Uuid>,
    expected_revision: Option<u64>,
) -> Result<SaveMonitorThresholdInput, CommandError> {
    if id.is_none() != expected_revision.is_none()
        || expected_revision.is_some_and(|revision| validate_revision(revision).is_err())
        || !threshold_value.is_finite()
        || !(0..=86_400).contains(&hold_seconds)
        || !(0..=604_800).contains(&cooldown_seconds)
        || (!toast_enabled && !window_enabled && matches!(sound, ReminderSound::None))
        || (matches!(
            metric,
            MonitorMetric::CpuPercent | MonitorMetric::MemoryPercent | MonitorMetric::GpuPercent
        ) && !(0.0..=100.0).contains(&threshold_value))
    {
        return Err(invalid_input("monitorThreshold"));
    }
    Ok(SaveMonitorThresholdInput {
        metric,
        comparator,
        threshold_value,
        hold_seconds,
        cooldown_seconds,
        sound,
        toast_enabled,
        window_enabled,
        enabled,
        id: id.map(|value| value.to_string()),
        expected_revision: expected_revision.map(|value| value as i64),
    })
}

fn valid_process_name(value: &str) -> bool {
    (1..=260).contains(&value.len())
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | '*' | '?'))
        && !value.contains(':')
}

fn validate_revision(revision: u64) -> Result<(), CommandError> {
    if revision == 0 {
        return Err(invalid_input("expectedRevision"));
    }
    i64::try_from(revision)
        .map(|_| ())
        .map_err(|_| invalid_input("expectedRevision"))
}

fn no_snapshot_error() -> CommandError {
    let mut details = SafeMessageParameters::new();
    details.insert(
        "serviceId".into(),
        SafeParameterValue::String("monitorCore".into()),
    );
    details.insert(
        "reasonCode".into(),
        SafeParameterValue::String("noPersistedSample".into()),
    );
    CommandError {
        code: AppErrorCode::SourceUnavailable,
        message_key: "errors.sourceUnavailable".into(),
        details,
        retryable: true,
    }
}

fn invalid_input(reason: &str) -> CommandError {
    CommandError::with_detail(
        AppErrorCode::InvalidInput,
        "errors.invalidInput",
        "reasonCode",
        SafeParameterValue::String(reason.into()),
        false,
    )
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::monitor::NewMonitorSample;
    use crate::storage::Storage;

    #[test]
    fn monitor_query_boundaries_accept_only_locked_ranges() {
        assert_eq!(validate_sample_query(0, 1).unwrap(), (0, 1));
        assert_eq!(validate_sample_query(42, 3_600).unwrap(), (42, 3_600));
        for invalid in [
            validate_sample_query(-1, 1),
            validate_sample_query(0, 0),
            validate_sample_query(0, 3_601),
        ] {
            assert_eq!(invalid.unwrap_err().code, AppErrorCode::InvalidInput);
        }
        assert_eq!(validate_process_limit(1).unwrap(), 1);
        assert_eq!(validate_process_limit(500).unwrap(), 500);
        for limit in [0, 501] {
            assert_eq!(
                validate_process_limit(limit).unwrap_err().code,
                AppErrorCode::InvalidInput
            );
        }
    }

    #[test]
    fn missing_snapshot_is_source_unavailable_and_persisted_snapshot_is_returned() {
        let directory = tempfile::tempdir().unwrap();
        let repository = MonitorRepository::new(Arc::new(Storage::open(directory.path()).unwrap()));
        assert_eq!(
            get_monitor_snapshot_with(&repository).unwrap_err().code,
            AppErrorCode::SourceUnavailable
        );
        repository
            .insert_sample(
                &NewMonitorSample {
                    cpu_percent: 25.0,
                    memory_used_bytes: 1,
                    memory_total_bytes: 2,
                    disk_read_bps: 3.0,
                    disk_write_bps: 4.0,
                    network_rx_bps: 5.0,
                    network_tx_bps: 6.0,
                    gpu_percent: None,
                    sampled_at: 42,
                },
                &[],
            )
            .unwrap();
        assert_eq!(
            get_monitor_snapshot_with(&repository).unwrap().sampled_at,
            42
        );
    }

    #[test]
    fn process_and_threshold_inputs_reject_invalid_shapes_before_services() {
        let id = Uuid::new_v4();
        for name in ["", "C:\\bad.exe", "bad*.exe", "bad\n.exe"] {
            assert_eq!(
                process_watch_input(None, name.into(), true, None)
                    .unwrap_err()
                    .code,
                AppErrorCode::InvalidInput
            );
        }
        assert!(process_watch_input(None, "aiceland.exe".into(), true, None).is_ok());
        assert_eq!(
            process_watch_input(Some(id), "aiceland.exe".into(), true, None)
                .unwrap_err()
                .code,
            AppErrorCode::InvalidInput
        );
        assert_eq!(
            process_watch_input(Some(id), "aiceland.exe".into(), true, Some(0))
                .unwrap_err()
                .code,
            AppErrorCode::InvalidInput
        );
        assert_eq!(
            validate_revision(0).unwrap_err().code,
            AppErrorCode::InvalidInput
        );
        assert_eq!(
            validate_revision(u64::MAX).unwrap_err().code,
            AppErrorCode::InvalidInput
        );
        for value in [f64::NAN, f64::INFINITY, -1.0, 101.0] {
            assert_eq!(
                threshold_input(
                    MonitorMetric::CpuPercent,
                    ThresholdComparator::GreaterThanOrEqual,
                    value,
                    0,
                    0,
                    ReminderSound::None,
                    true,
                    false,
                    true,
                    None,
                    None,
                )
                .unwrap_err()
                .code,
                AppErrorCode::InvalidInput
            );
        }
        assert_eq!(
            threshold_input(
                MonitorMetric::CpuPercent,
                ThresholdComparator::GreaterThanOrEqual,
                80.0,
                0,
                0,
                ReminderSound::None,
                true,
                false,
                true,
                Some(id),
                Some(0),
            )
            .unwrap_err()
            .code,
            AppErrorCode::InvalidInput
        );
    }
}
