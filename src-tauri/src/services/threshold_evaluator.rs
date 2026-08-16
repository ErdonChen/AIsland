use crate::contracts::{
    AppErrorCode, CommandError, DeleteResult, DiagnosticEvent, DiagnosticLevel, MonitorMetric,
    MonitorSnapshot, MonitorThreshold, ReminderSourceContext, ReminderSourceKind,
    SafeMessageParameters, SafeParameterValue, SaveMonitorThresholdInput, ThresholdComparator,
};
use crate::domain::monitor::{ThresholdBreach, ThresholdBreachUpdate};
use crate::domain::reminders::NewReminderDelivery;
use crate::repositories::{diagnostics::DiagnosticsRepository, monitor::MonitorRepository};
use crate::services::reminder_scheduler::ReminderService;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

const CANCELLATION_FAILED_DIAGNOSTIC: &str = "monitor.thresholdCancellationFailed";
const MONITOR_THRESHOLD_SERVICE_ID: &str = "monitorThresholds";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThresholdEvaluationOutcome {
    Below,
    Holding {
        breach_started_at: i64,
    },
    Enqueued {
        breach_started_at: i64,
        delivery_id: Uuid,
    },
    InCooldown {
        until: i64,
    },
    SourceUnavailable,
}

pub trait ThresholdEvaluationPort: Send + Sync {
    fn evaluate(
        &self,
        snapshot: &MonitorSnapshot,
        now: i64,
    ) -> Result<Vec<ThresholdEvaluationOutcome>, CommandError>;
}

#[derive(Default)]
struct ThresholdOperationGate(Mutex<()>);

#[derive(Clone)]
pub struct ThresholdEvaluator {
    repository: MonitorRepository,
    reminders: Arc<ReminderService>,
    gate: Arc<ThresholdOperationGate>,
    #[cfg(test)]
    before_enqueue: Arc<std::sync::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>,
    #[cfg(test)]
    after_enqueue:
        Arc<std::sync::Mutex<Option<Arc<dyn Fn() -> Result<(), CommandError> + Send + Sync>>>>,
}

impl ThresholdEvaluator {
    #[cfg(test)]
    fn new(repository: MonitorRepository, reminders: Arc<ReminderService>) -> Self {
        Self::with_gate(
            repository,
            reminders,
            Arc::new(ThresholdOperationGate::default()),
        )
    }

    fn with_gate(
        repository: MonitorRepository,
        reminders: Arc<ReminderService>,
        gate: Arc<ThresholdOperationGate>,
    ) -> Self {
        Self {
            repository,
            reminders,
            gate,
            #[cfg(test)]
            before_enqueue: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            after_enqueue: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    #[cfg(test)]
    fn set_before_enqueue(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        *self
            .before_enqueue
            .lock()
            .expect("threshold before-enqueue hook poisoned") = Some(hook);
    }

    #[cfg(test)]
    fn set_after_enqueue(&self, hook: Arc<dyn Fn() -> Result<(), CommandError> + Send + Sync>) {
        *self
            .after_enqueue
            .lock()
            .expect("threshold enqueue hook poisoned") = Some(hook);
    }

    pub fn evaluate(
        &self,
        snapshot: &MonitorSnapshot,
        now: i64,
    ) -> Result<Vec<ThresholdEvaluationOutcome>, CommandError> {
        let _operation = self
            .gate
            .0
            .lock()
            .map_err(|_| crate::services::service_stopping_error())?;
        if now < 0 {
            return Err(invalid_input());
        }
        self.repository
            .list_enabled_thresholds()?
            .iter()
            .map(|threshold| self.evaluate_threshold(threshold, snapshot, now))
            .collect()
    }

    fn evaluate_threshold(
        &self,
        threshold: &MonitorThreshold,
        snapshot: &MonitorSnapshot,
        now: i64,
    ) -> Result<ThresholdEvaluationOutcome, CommandError> {
        let threshold_id = Uuid::parse_str(&threshold.id).map_err(|_| storage_unavailable())?;
        let Some((comparison_value, current_value)) = metric_value(&threshold.metric, snapshot)
        else {
            return Ok(ThresholdEvaluationOutcome::SourceUnavailable);
        };
        let latest = self.repository.latest_breach(threshold_id)?;
        if !is_breached(threshold, comparison_value) {
            if let Some(active) = latest.filter(|breach| breach.cleared_at.is_none()) {
                self.repository.update_breach(ThresholdBreachUpdate {
                    threshold_id,
                    breach_started_at: active.breach_started_at,
                    last_triggered_at: active.last_triggered_at,
                    cleared_at: Some(now),
                    reminder_delivery_id: parse_delivery_id(&active)?,
                })?;
            }
            return Ok(ThresholdEvaluationOutcome::Below);
        }

        let breach = match latest.filter(|breach| breach.cleared_at.is_none()) {
            Some(active) => active,
            None => self.repository.update_breach(ThresholdBreachUpdate {
                threshold_id,
                breach_started_at: now,
                last_triggered_at: None,
                cleared_at: None,
                reminder_delivery_id: None,
            })?,
        };

        if let Some(delivery_id) = parse_delivery_id(&breach)? {
            return Ok(ThresholdEvaluationOutcome::Enqueued {
                breach_started_at: breach.breach_started_at,
                delivery_id,
            });
        }

        let hold_until = checked_seconds_after(breach.breach_started_at, threshold.hold_seconds)?;
        if now < hold_until {
            return Ok(ThresholdEvaluationOutcome::Holding {
                breach_started_at: breach.breach_started_at,
            });
        }

        if let Some(previous) = self
            .repository
            .latest_triggered_before(threshold_id, breach.breach_started_at)?
        {
            if let Some(last_triggered_at) = previous.last_triggered_at {
                let cooldown_until =
                    checked_seconds_after(last_triggered_at, threshold.cooldown_seconds)?;
                if now < cooldown_until {
                    return Ok(ThresholdEvaluationOutcome::InCooldown {
                        until: cooldown_until,
                    });
                }
            }
        }

        let threshold_value = safe_integer(threshold.threshold_value)?;
        #[cfg(test)]
        if let Some(hook) = self
            .before_enqueue
            .lock()
            .expect("threshold before-enqueue hook poisoned")
            .clone()
        {
            hook();
        }
        let outcome = self.reminders.enqueue(
            NewReminderDelivery {
                dedupe_key: format!("monitor:{}:{}", threshold.id, breach.breach_started_at),
                rule_id: None,
                source_kind: ReminderSourceKind::Monitor,
                source_entity_id: threshold.id.clone(),
                message_key: "reminders.monitor.threshold".into(),
                message_parameters: BTreeMap::from([
                    (
                        "metric".into(),
                        SafeParameterValue::String(metric_name(&threshold.metric).into()),
                    ),
                    (
                        "currentValue".into(),
                        SafeParameterValue::Number(current_value.into()),
                    ),
                    (
                        "thresholdValue".into(),
                        SafeParameterValue::Number(threshold_value.into()),
                    ),
                ]),
                source_context: ReminderSourceContext::Monitor {
                    threshold_id: threshold.id.clone(),
                    metric: threshold.metric.clone(),
                    current_value,
                    threshold_value,
                    breach_started_at: breach.breach_started_at,
                    source_occurred_at: breach.breach_started_at,
                },
                source_occurred_at: breach.breach_started_at,
                sound: threshold.sound.clone(),
                toast_enabled: threshold.toast_enabled,
                window_enabled: threshold.window_enabled,
                due_at: now,
            },
            now,
        )?;
        let delivery = match outcome {
            crate::domain::reminders::EnqueueOutcome::Inserted(delivery)
            | crate::domain::reminders::EnqueueOutcome::Duplicate(delivery) => delivery,
        };
        let delivery_id = Uuid::parse_str(&delivery.id).map_err(|_| storage_unavailable())?;
        #[cfg(test)]
        if let Some(hook) = self
            .after_enqueue
            .lock()
            .expect("threshold enqueue hook poisoned")
            .clone()
        {
            hook()?;
        }
        self.repository.update_breach(ThresholdBreachUpdate {
            threshold_id,
            breach_started_at: breach.breach_started_at,
            last_triggered_at: Some(delivery.created_at),
            cleared_at: None,
            reminder_delivery_id: Some(delivery_id),
        })?;
        Ok(ThresholdEvaluationOutcome::Enqueued {
            breach_started_at: breach.breach_started_at,
            delivery_id,
        })
    }
}

impl ThresholdEvaluationPort for ThresholdEvaluator {
    fn evaluate(
        &self,
        snapshot: &MonitorSnapshot,
        now: i64,
    ) -> Result<Vec<ThresholdEvaluationOutcome>, CommandError> {
        ThresholdEvaluator::evaluate(self, snapshot, now)
    }
}

#[derive(Clone)]
pub struct MonitorThresholdService {
    repository: MonitorRepository,
    reminders: Arc<ReminderService>,
    diagnostics: DiagnosticsRepository,
    gate: Arc<ThresholdOperationGate>,
}

impl MonitorThresholdService {
    pub(crate) fn compose(
        repository: MonitorRepository,
        reminders: Arc<ReminderService>,
        diagnostics: DiagnosticsRepository,
    ) -> (Arc<ThresholdEvaluator>, Self) {
        let gate = Arc::new(ThresholdOperationGate::default());
        (
            Arc::new(ThresholdEvaluator::with_gate(
                repository.clone(),
                reminders.clone(),
                gate.clone(),
            )),
            Self {
                repository,
                reminders,
                diagnostics,
                gate,
            },
        )
    }

    pub fn save(
        &self,
        input: SaveMonitorThresholdInput,
        now: i64,
    ) -> Result<MonitorThreshold, CommandError> {
        let _operation = self
            .gate
            .0
            .lock()
            .map_err(|_| crate::services::service_stopping_error())?;
        let threshold = self.repository.save_threshold(input, now)?;
        if !threshold.enabled {
            let id = Uuid::parse_str(&threshold.id).map_err(|_| storage_unavailable())?;
            self.cancel_pending_for_threshold_unlocked(id, now)?;
        }
        Ok(threshold)
    }

    pub fn delete(
        &self,
        id: Uuid,
        expected_revision: u64,
        now: i64,
    ) -> Result<DeleteResult, CommandError> {
        let _operation = self
            .gate
            .0
            .lock()
            .map_err(|_| crate::services::service_stopping_error())?;
        let deleted = self
            .repository
            .delete_threshold(id, expected_revision, now)?;
        self.cancel_pending_for_threshold_unlocked(id, now)?;
        Ok(deleted)
    }

    pub fn cancel_pending_for_threshold(
        &self,
        threshold_id: Uuid,
        now: i64,
    ) -> Result<u64, CommandError> {
        let _operation = self
            .gate
            .0
            .lock()
            .map_err(|_| crate::services::service_stopping_error())?;
        self.cancel_pending_for_threshold_unlocked(threshold_id, now)
    }

    fn cancel_pending_for_threshold_unlocked(
        &self,
        threshold_id: Uuid,
        now: i64,
    ) -> Result<u64, CommandError> {
        match self.reminders.cancel_pending(
            ReminderSourceKind::Monitor,
            &threshold_id.to_string(),
            now,
        ) {
            Ok(cancelled) => Ok(cancelled),
            Err(error) => {
                self.record_cancellation_failure(threshold_id, &error, now);
                Err(error)
            }
        }
    }

    pub fn reconcile_pending_cancellations(&self, now: i64) -> Result<u64, CommandError> {
        let _operation = self
            .gate
            .0
            .lock()
            .map_err(|_| crate::services::service_stopping_error())?;
        let enabled = self
            .repository
            .list_enabled_thresholds()?
            .into_iter()
            .map(|threshold| threshold.id)
            .collect::<BTreeSet<_>>();
        let mut cancelled = 0_u64;
        let mut first_error = None;
        for threshold_id in self.repository.list_pending_delivery_source_ids()? {
            if enabled.contains(&threshold_id.to_string()) {
                continue;
            }
            match self.cancel_pending_for_threshold_unlocked(threshold_id, now) {
                Ok(count) => cancelled = cancelled.saturating_add(count),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(cancelled), Err)
    }

    fn record_cancellation_failure(&self, threshold_id: Uuid, error: &CommandError, now: i64) {
        let _ = self.diagnostics.record(&DiagnosticEvent {
            id: Uuid::new_v4().to_string(),
            service_id: MONITOR_THRESHOLD_SERVICE_ID.into(),
            level: DiagnosticLevel::Failure,
            code: CANCELLATION_FAILED_DIAGNOSTIC.into(),
            parameters: BTreeMap::from([
                (
                    "thresholdId".into(),
                    SafeParameterValue::String(threshold_id.to_string()),
                ),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String(error_reason(error).into()),
                ),
            ]),
            created_at: now,
        });
    }
}

fn metric_value(metric: &MonitorMetric, snapshot: &MonitorSnapshot) -> Option<(f64, i64)> {
    Some(match metric {
        MonitorMetric::CpuPercent => (snapshot.cpu_percent as f64, snapshot.cpu_percent),
        MonitorMetric::MemoryPercent => {
            if snapshot.memory_total_bytes <= 0 {
                return None;
            }
            let exact =
                (snapshot.memory_used_bytes as f64 / snapshot.memory_total_bytes as f64) * 100.0;
            (exact, exact.round() as i64)
        }
        MonitorMetric::DiskReadBytesPerSecond => (
            snapshot.disk_read_bytes_per_second as f64,
            snapshot.disk_read_bytes_per_second,
        ),
        MonitorMetric::DiskWriteBytesPerSecond => (
            snapshot.disk_write_bytes_per_second as f64,
            snapshot.disk_write_bytes_per_second,
        ),
        MonitorMetric::NetworkReceiveBytesPerSecond => (
            snapshot.network_receive_bytes_per_second as f64,
            snapshot.network_receive_bytes_per_second,
        ),
        MonitorMetric::NetworkSendBytesPerSecond => (
            snapshot.network_send_bytes_per_second as f64,
            snapshot.network_send_bytes_per_second,
        ),
        MonitorMetric::GpuPercent => {
            let value = snapshot.gpu_percent?;
            (value as f64, value)
        }
    })
}

fn is_breached(threshold: &MonitorThreshold, current_value: f64) -> bool {
    match threshold.comparator {
        ThresholdComparator::GreaterThanOrEqual => current_value >= threshold.threshold_value,
        ThresholdComparator::LessThanOrEqual => current_value <= threshold.threshold_value,
    }
}

fn parse_delivery_id(breach: &ThresholdBreach) -> Result<Option<Uuid>, CommandError> {
    breach
        .reminder_delivery_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| storage_unavailable())
}

fn checked_seconds_after(at: i64, seconds: i64) -> Result<i64, CommandError> {
    at.checked_add(seconds.checked_mul(1_000).ok_or_else(invalid_input)?)
        .ok_or_else(invalid_input)
}

fn safe_integer(value: f64) -> Result<i64, CommandError> {
    if !value.is_finite() || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return Err(invalid_input());
    }
    Ok(value.round() as i64)
}

fn metric_name(metric: &MonitorMetric) -> &'static str {
    match metric {
        MonitorMetric::CpuPercent => "cpu",
        MonitorMetric::MemoryPercent => "memory",
        MonitorMetric::DiskReadBytesPerSecond => "diskRead",
        MonitorMetric::DiskWriteBytesPerSecond => "diskWrite",
        MonitorMetric::NetworkReceiveBytesPerSecond => "networkReceive",
        MonitorMetric::NetworkSendBytesPerSecond => "networkSend",
        MonitorMetric::GpuPercent => "gpu",
    }
}

fn error_reason(error: &CommandError) -> &'static str {
    match error.code {
        AppErrorCode::DatabaseFailure => "databaseFailure",
        AppErrorCode::StorageUnavailable => "storageUnavailable",
        AppErrorCode::PermissionDenied => "permissionDenied",
        AppErrorCode::Conflict => "conflict",
        AppErrorCode::NotFound => "notFound",
        _ => "cancelFailed",
    }
}

fn invalid_input() -> CommandError {
    CommandError {
        code: AppErrorCode::InvalidInput,
        message_key: "errors.invalidInput".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

fn storage_unavailable() -> CommandError {
    CommandError {
        code: AppErrorCode::StorageUnavailable,
        message_key: "errors.storageUnavailable".into(),
        details: SafeMessageParameters::new(),
        retryable: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{ReminderSound, SafeParameterValue};
    use crate::repositories::reminders::ReminderRepository;
    use crate::services::reminder_scheduler::SystemReminderClock;
    use crate::services::EventEmitterPort;
    use crate::storage::Storage;
    use std::sync::Arc;

    struct NoopEmitter;

    impl EventEmitterPort for NoopEmitter {
        fn emit(
            &self,
            _event_name: &'static str,
            _payload: serde_json::Value,
        ) -> Result<(), CommandError> {
            Ok(())
        }
    }

    struct Fixture {
        _directory: tempfile::TempDir,
        storage: Arc<Storage>,
        repository: MonitorRepository,
        reminders: Arc<ReminderService>,
        _reminder_worker: crate::services::reminder_scheduler::ReminderWorker,
        evaluator: ThresholdEvaluator,
        service: MonitorThresholdService,
        diagnostics: DiagnosticsRepository,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let storage = Arc::new(Storage::open(directory.path()).unwrap());
            let repository = MonitorRepository::new(storage.clone());
            let reminders_repository = ReminderRepository::new(storage.clone());
            let diagnostics = DiagnosticsRepository::new(storage.clone());
            let (reminders, reminder_worker) = ReminderService::new(
                reminders_repository.clone(),
                Arc::new(SystemReminderClock),
                Arc::new(NoopEmitter),
            );
            let (evaluator, service) = MonitorThresholdService::compose(
                repository.clone(),
                reminders.clone(),
                diagnostics.clone(),
            );
            Self {
                _directory: directory,
                storage,
                repository,
                reminders,
                _reminder_worker: reminder_worker,
                evaluator: (*evaluator).clone(),
                service,
                diagnostics,
            }
        }

        fn save_threshold(
            &self,
            metric: MonitorMetric,
            comparator: ThresholdComparator,
            value: f64,
            hold_seconds: i64,
            cooldown_seconds: i64,
        ) -> MonitorThreshold {
            self.repository
                .save_threshold(
                    SaveMonitorThresholdInput {
                        metric,
                        comparator,
                        threshold_value: value,
                        hold_seconds,
                        cooldown_seconds,
                        sound: ReminderSound::None,
                        toast_enabled: true,
                        window_enabled: false,
                        enabled: true,
                        id: None,
                        expected_revision: None,
                    },
                    1,
                )
                .unwrap()
        }

        fn snapshot(cpu: i64, gpu: Option<i64>, sampled_at: i64) -> MonitorSnapshot {
            MonitorSnapshot {
                cpu_percent: cpu,
                memory_used_bytes: 50,
                memory_total_bytes: 100,
                disk_read_bytes_per_second: 10,
                disk_write_bytes_per_second: 20,
                network_receive_bytes_per_second: 30,
                network_send_bytes_per_second: 40,
                gpu_percent: gpu,
                sampled_at,
            }
        }

        fn delivery_count(&self, threshold_id: &str) -> i64 {
            self.storage
                .with_connection(|connection| {
                    connection
                        .query_row(
                            "SELECT COUNT(*) FROM reminder_deliveries WHERE source_kind='monitor' AND source_entity_id=?1",
                            [threshold_id],
                            |row| row.get(0),
                        )
                        .map_err(Into::into)
                })
                .unwrap()
        }
    }

    #[test]
    fn equality_hold_clear_cooldown_restart_and_dedupe_use_persisted_first_breach() {
        let fixture = Fixture::new();
        let threshold = fixture.save_threshold(
            MonitorMetric::CpuPercent,
            ThresholdComparator::GreaterThanOrEqual,
            50.0,
            2,
            10,
        );

        assert_eq!(
            fixture
                .evaluator
                .evaluate(&Fixture::snapshot(50, Some(10), 1_000), 1_000)
                .unwrap(),
            vec![ThresholdEvaluationOutcome::Holding {
                breach_started_at: 1_000
            }]
        );
        assert_eq!(fixture.delivery_count(&threshold.id), 0);
        assert!(matches!(
            fixture
                .evaluator
                .evaluate(&Fixture::snapshot(51, Some(10), 2_999), 2_999)
                .unwrap()
                .as_slice(),
            [ThresholdEvaluationOutcome::Holding {
                breach_started_at: 1_000
            }]
        ));

        let first_delivery = match fixture
            .evaluator
            .evaluate(&Fixture::snapshot(51, Some(10), 3_000), 3_000)
            .unwrap()
            .remove(0)
        {
            ThresholdEvaluationOutcome::Enqueued {
                breach_started_at,
                delivery_id,
            } => {
                assert_eq!(breach_started_at, 1_000);
                delivery_id
            }
            other => panic!("unexpected outcome: {other:?}"),
        };
        assert_eq!(fixture.delivery_count(&threshold.id), 1);
        let delivery_contract: (String, String, String, String, String, i64, i64) = fixture
            .storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT dedupe_key,source_kind,source_entity_id,message_key,source_context_json,source_occurred_at,due_at FROM reminder_deliveries WHERE id=?1",
                        [first_delivery.to_string()],
                        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?)),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(
            delivery_contract.0,
            format!("monitor:{}:1000", threshold.id)
        );
        assert_eq!(delivery_contract.1, "monitor");
        assert_eq!(delivery_contract.2, threshold.id);
        assert_eq!(delivery_contract.3, "reminders.monitor.threshold");
        assert_eq!(delivery_contract.5, 1_000);
        assert_eq!(delivery_contract.6, 3_000);
        let context: serde_json::Value = serde_json::from_str(&delivery_contract.4).unwrap();
        assert_eq!(context["kind"], "monitor");
        assert_eq!(context["thresholdId"], threshold.id);
        assert_eq!(context["breachStartedAt"], 1_000);
        assert_eq!(context["sourceOccurredAt"], 1_000);
        let channel_contract: (String, String, bool, bool, String) = fixture
            .storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT message_parameters_json,sound_json,toast_enabled,window_enabled,state FROM reminder_deliveries WHERE id=?1",
                        [first_delivery.to_string()],
                        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        let parameters: SafeMessageParameters = serde_json::from_str(&channel_contract.0).unwrap();
        assert_eq!(
            parameters,
            BTreeMap::from([
                ("metric".into(), SafeParameterValue::String("cpu".into()),),
                ("currentValue".into(), SafeParameterValue::Number(51.into()),),
                (
                    "thresholdValue".into(),
                    SafeParameterValue::Number(50.into()),
                ),
            ])
        );
        assert_eq!(
            serde_json::from_str::<ReminderSound>(&channel_contract.1).unwrap(),
            ReminderSound::None
        );
        assert!(channel_contract.2);
        assert!(!channel_contract.3);
        assert_eq!(channel_contract.4, "pending");

        let restarted =
            ThresholdEvaluator::new(fixture.repository.clone(), fixture.reminders.clone());
        let replay = restarted
            .evaluate(&Fixture::snapshot(90, Some(10), 4_000), 4_000)
            .unwrap();
        assert_eq!(
            replay,
            vec![ThresholdEvaluationOutcome::Enqueued {
                breach_started_at: 1_000,
                delivery_id: first_delivery,
            }]
        );
        assert_eq!(fixture.delivery_count(&threshold.id), 1);

        assert_eq!(
            restarted
                .evaluate(&Fixture::snapshot(49, Some(10), 5_000), 5_000)
                .unwrap(),
            vec![ThresholdEvaluationOutcome::Below]
        );
        assert!(matches!(
            restarted
                .evaluate(&Fixture::snapshot(50, Some(10), 6_000), 6_000)
                .unwrap()
                .as_slice(),
            [ThresholdEvaluationOutcome::Holding {
                breach_started_at: 6_000
            }]
        ));
        assert_eq!(
            restarted
                .evaluate(&Fixture::snapshot(50, Some(10), 8_000), 8_000)
                .unwrap(),
            vec![ThresholdEvaluationOutcome::InCooldown { until: 13_000 }]
        );
        assert_eq!(
            restarted
                .evaluate(&Fixture::snapshot(50, Some(10), 12_999), 12_999)
                .unwrap(),
            vec![ThresholdEvaluationOutcome::InCooldown { until: 13_000 }]
        );
        let second = restarted
            .evaluate(&Fixture::snapshot(50, Some(10), 13_000), 13_000)
            .unwrap();
        assert!(matches!(
            second.as_slice(),
            [ThresholdEvaluationOutcome::Enqueued {
                breach_started_at: 6_000,
                ..
            }]
        ));
        assert_eq!(fixture.delivery_count(&threshold.id), 2);
        let breaches = fixture
            .repository
            .list_breaches(Uuid::parse_str(&threshold.id).unwrap())
            .unwrap();
        assert_eq!(breaches.len(), 2);
        assert_eq!(breaches[0].cleared_at, Some(5_000));
        assert!(breaches[1].reminder_delivery_id.is_some());
    }

    #[test]
    fn less_than_or_equal_also_treats_equality_as_breached() {
        let fixture = Fixture::new();
        let threshold = fixture.save_threshold(
            MonitorMetric::CpuPercent,
            ThresholdComparator::LessThanOrEqual,
            50.0,
            0,
            0,
        );

        assert_eq!(
            fixture
                .evaluator
                .evaluate(&Fixture::snapshot(51, Some(10), 1_000), 1_000)
                .unwrap(),
            vec![ThresholdEvaluationOutcome::Below]
        );
        assert!(matches!(
            fixture
                .evaluator
                .evaluate(&Fixture::snapshot(50, Some(10), 2_000), 2_000)
                .unwrap()
                .as_slice(),
            [ThresholdEvaluationOutcome::Enqueued {
                breach_started_at: 2_000,
                ..
            }]
        ));
        assert_eq!(fixture.delivery_count(&threshold.id), 1);
    }

    #[test]
    fn unavailable_gpu_neither_starts_nor_clears_a_breach() {
        let fixture = Fixture::new();
        let threshold = fixture.save_threshold(
            MonitorMetric::GpuPercent,
            ThresholdComparator::GreaterThanOrEqual,
            50.0,
            10,
            0,
        );
        assert_eq!(
            fixture
                .evaluator
                .evaluate(&Fixture::snapshot(0, None, 1_000), 1_000)
                .unwrap(),
            vec![ThresholdEvaluationOutcome::SourceUnavailable]
        );
        assert!(fixture
            .repository
            .list_breaches(Uuid::parse_str(&threshold.id).unwrap())
            .unwrap()
            .is_empty());
        fixture
            .evaluator
            .evaluate(&Fixture::snapshot(0, Some(70), 2_000), 2_000)
            .unwrap();
        fixture
            .evaluator
            .evaluate(&Fixture::snapshot(0, None, 3_000), 3_000)
            .unwrap();
        let breach = fixture
            .repository
            .latest_breach(Uuid::parse_str(&threshold.id).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(breach.breach_started_at, 2_000);
        assert_eq!(breach.cleared_at, None);
    }

    #[test]
    fn memory_threshold_compares_the_exact_ratio_instead_of_the_display_rounding() {
        let fixture = Fixture::new();
        let threshold = fixture.save_threshold(
            MonitorMetric::MemoryPercent,
            ThresholdComparator::GreaterThanOrEqual,
            50.0,
            0,
            0,
        );
        let mut below = Fixture::snapshot(0, Some(0), 1_000);
        below.memory_used_bytes = 496;
        below.memory_total_bytes = 1_000;
        assert_eq!(
            fixture.evaluator.evaluate(&below, 1_000).unwrap(),
            vec![ThresholdEvaluationOutcome::Below]
        );
        assert!(fixture
            .repository
            .list_breaches(Uuid::parse_str(&threshold.id).unwrap())
            .unwrap()
            .is_empty());
        below.memory_used_bytes = 500;
        assert!(matches!(
            fixture
                .evaluator
                .evaluate(&below, 2_000)
                .unwrap()
                .as_slice(),
            [ThresholdEvaluationOutcome::Enqueued {
                breach_started_at: 2_000,
                ..
            }]
        ));
    }

    #[test]
    fn restart_after_enqueue_before_breach_update_reuses_the_original_delivery() {
        let fixture = Fixture::new();
        let threshold = fixture.save_threshold(
            MonitorMetric::CpuPercent,
            ThresholdComparator::GreaterThanOrEqual,
            1.0,
            0,
            0,
        );
        fixture.evaluator.set_after_enqueue(Arc::new(|| {
            Err(CommandError {
                code: AppErrorCode::StorageUnavailable,
                message_key: "errors.storageUnavailable".into(),
                details: SafeMessageParameters::new(),
                retryable: true,
            })
        }));
        assert_eq!(
            fixture
                .evaluator
                .evaluate(&Fixture::snapshot(10, Some(0), 1_000), 1_000)
                .unwrap_err()
                .code,
            AppErrorCode::StorageUnavailable
        );
        assert_eq!(fixture.delivery_count(&threshold.id), 1);
        let threshold_id = Uuid::parse_str(&threshold.id).unwrap();
        assert_eq!(
            fixture
                .repository
                .latest_breach(threshold_id)
                .unwrap()
                .unwrap()
                .reminder_delivery_id,
            None
        );

        let restarted =
            ThresholdEvaluator::new(fixture.repository.clone(), fixture.reminders.clone());
        let recovered = restarted
            .evaluate(&Fixture::snapshot(10, Some(0), 2_000), 2_000)
            .unwrap();
        let recovered_id = match recovered.as_slice() {
            [ThresholdEvaluationOutcome::Enqueued { delivery_id, .. }] => *delivery_id,
            other => panic!("unexpected recovery outcome: {other:?}"),
        };
        assert_eq!(fixture.delivery_count(&threshold.id), 1);
        let stored = fixture
            .repository
            .latest_breach(threshold_id)
            .unwrap()
            .unwrap();
        assert_eq!(stored.reminder_delivery_id, Some(recovered_id.to_string()));
        let original_id: String = fixture
            .storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT id FROM reminder_deliveries WHERE source_entity_id=?1",
                        [threshold.id],
                        |row| row.get(0),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(recovered_id.to_string(), original_id);
    }

    #[test]
    fn disable_delete_history_and_startup_reconciliation_cancel_active_sources() {
        let fixture = Fixture::new();
        let threshold = fixture.save_threshold(
            MonitorMetric::CpuPercent,
            ThresholdComparator::GreaterThanOrEqual,
            1.0,
            0,
            0,
        );
        fixture
            .evaluator
            .evaluate(&Fixture::snapshot(10, Some(0), 100), 100)
            .unwrap();
        let disabled = fixture
            .service
            .save(
                SaveMonitorThresholdInput {
                    metric: threshold.metric.clone(),
                    comparator: threshold.comparator.clone(),
                    threshold_value: threshold.threshold_value,
                    hold_seconds: threshold.hold_seconds,
                    cooldown_seconds: threshold.cooldown_seconds,
                    sound: threshold.sound.clone(),
                    toast_enabled: threshold.toast_enabled,
                    window_enabled: threshold.window_enabled,
                    enabled: false,
                    id: Some(threshold.id.clone()),
                    expected_revision: Some(threshold.revision),
                },
                200,
            )
            .unwrap();
        assert!(!disabled.enabled);
        assert_eq!(
            fixture
                .service
                .cancel_pending_for_threshold(Uuid::parse_str(&threshold.id).unwrap(), 201)
                .unwrap(),
            0
        );

        let service_deleted = fixture.save_threshold(
            MonitorMetric::CpuPercent,
            ThresholdComparator::GreaterThanOrEqual,
            1.0,
            0,
            0,
        );
        fixture
            .evaluator
            .evaluate(&Fixture::snapshot(10, Some(0), 250), 250)
            .unwrap();
        fixture
            .service
            .delete(
                Uuid::parse_str(&service_deleted.id).unwrap(),
                service_deleted.revision.try_into().unwrap(),
                251,
            )
            .unwrap();
        assert_eq!(
            fixture
                .repository
                .list_breaches(Uuid::parse_str(&service_deleted.id).unwrap())
                .unwrap()
                .len(),
            1
        );
        let service_deleted_state: String = fixture
            .storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT state FROM reminder_deliveries WHERE source_entity_id=?1",
                        [service_deleted.id],
                        |row| row.get(0),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(service_deleted_state, "cancelled");

        let deleted_threshold = fixture.save_threshold(
            MonitorMetric::CpuPercent,
            ThresholdComparator::GreaterThanOrEqual,
            1.0,
            0,
            0,
        );
        fixture
            .evaluator
            .evaluate(&Fixture::snapshot(10, Some(0), 300), 300)
            .unwrap();
        fixture
            .repository
            .delete_threshold(
                Uuid::parse_str(&deleted_threshold.id).unwrap(),
                deleted_threshold.revision.try_into().unwrap(),
                301,
            )
            .unwrap();
        assert_eq!(
            fixture
                .service
                .reconcile_pending_cancellations(302)
                .unwrap(),
            1
        );
        assert_eq!(
            fixture
                .repository
                .list_breaches(Uuid::parse_str(&deleted_threshold.id).unwrap())
                .unwrap()
                .len(),
            1
        );
        assert!(fixture
            .repository
            .list_pending_delivery_source_ids()
            .unwrap()
            .is_empty());
        let threshold_fk_count: i64 = fixture
            .storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM pragma_foreign_key_list('threshold_breaches') WHERE \"from\"='threshold_id'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(threshold_fk_count, 0);
    }

    #[test]
    fn cancellation_failure_returns_error_after_source_commit_and_records_safe_diagnostic() {
        let fixture = Fixture::new();
        let threshold = fixture.save_threshold(
            MonitorMetric::CpuPercent,
            ThresholdComparator::GreaterThanOrEqual,
            1.0,
            0,
            0,
        );
        fixture
            .storage
            .with_connection(|connection| {
                connection.execute("DROP TABLE reminder_deliveries", [])?;
                Ok(())
            })
            .unwrap();
        let error = fixture
            .service
            .save(
                SaveMonitorThresholdInput {
                    metric: threshold.metric.clone(),
                    comparator: threshold.comparator.clone(),
                    threshold_value: threshold.threshold_value,
                    hold_seconds: threshold.hold_seconds,
                    cooldown_seconds: threshold.cooldown_seconds,
                    sound: threshold.sound.clone(),
                    toast_enabled: threshold.toast_enabled,
                    window_enabled: threshold.window_enabled,
                    enabled: false,
                    id: Some(threshold.id.clone()),
                    expected_revision: Some(threshold.revision),
                },
                500,
            )
            .unwrap_err();
        assert_eq!(error.code, AppErrorCode::DatabaseFailure);
        assert!(
            !fixture
                .repository
                .list_thresholds()
                .unwrap()
                .into_iter()
                .find(|value| value.id == threshold.id)
                .unwrap()
                .enabled
        );
        let diagnostic = fixture.diagnostics.list(1).unwrap().remove(0);
        assert_eq!(diagnostic.code, CANCELLATION_FAILED_DIAGNOSTIC);
        assert_eq!(
            diagnostic.parameters,
            BTreeMap::from([
                (
                    "thresholdId".into(),
                    SafeParameterValue::String(threshold.id),
                ),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String("databaseFailure".into()),
                ),
            ])
        );
    }

    #[test]
    fn concurrent_delete_waits_for_projection_then_cancels_the_exact_new_delivery() {
        let fixture = Fixture::new();
        let threshold = fixture.save_threshold(
            MonitorMetric::CpuPercent,
            ThresholdComparator::GreaterThanOrEqual,
            1.0,
            0,
            0,
        );
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        fixture.evaluator.set_before_enqueue(Arc::new({
            let entered = entered.clone();
            let release = release.clone();
            move || {
                entered.wait();
                release.wait();
            }
        }));
        let evaluator = fixture.evaluator.clone();
        let evaluation = std::thread::spawn(move || {
            evaluator.evaluate(&Fixture::snapshot(10, Some(0), 1_000), 1_000)
        });
        entered.wait();

        let service = fixture.service.clone();
        let threshold_id = Uuid::parse_str(&threshold.id).unwrap();
        let revision = threshold.revision.try_into().unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let deletion = std::thread::spawn(move || {
            let result = service.delete(threshold_id, revision, 1_001);
            done_tx.send(result.clone()).unwrap();
            result
        });
        assert!(done_rx
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err());
        release.wait();
        assert!(evaluation.join().unwrap().is_ok());
        assert!(deletion.join().unwrap().is_ok());
        assert!(fixture.repository.list_thresholds().unwrap().is_empty());
        assert_eq!(fixture.delivery_count(&threshold.id), 1);
        assert!(fixture
            .repository
            .list_pending_delivery_source_ids()
            .unwrap()
            .is_empty());
        let state: String = fixture
            .storage
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT state FROM reminder_deliveries WHERE source_entity_id=?1",
                        [threshold.id],
                        |row| row.get(0),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(state, "cancelled");
    }
}
