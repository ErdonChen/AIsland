use crate::contracts::{
    AppErrorCode, CommandError, DiagnosticEvent, DiagnosticLevel, MonitorSnapshot,
    SafeMessageParameters, SafeParameterValue, ServiceHealthSnapshot, ServiceHealthState,
};
use crate::events::{monitor_metrics_changed_payload, MONITOR_METRICS_CHANGED};
use crate::repositories::{
    diagnostics::DiagnosticsRepository, monitor::MonitorRepository,
    service_health::ServiceHealthRepository,
};
use crate::services::system_metrics::{CoreMetricsSource, MetricFault, WindowsCoreMetricsSource};
use crate::services::threshold_evaluator::ThresholdEvaluationPort;
use crate::services::{
    gpu_metrics::{GpuMetricsSource, WindowsGpuMetricsSource},
    process_metrics::{
        ProcessMetricsSource, ProcessSkip, ProcessSkipCollector, WindowsProcessMetricsSource,
    },
};
use crate::services::{EventEmitterPort, TauriEventEmitter};
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex, MutexGuard,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const MONITOR_CORE_SERVICE_ID: &str = "monitorCore";
const MONITOR_GPU_SERVICE_ID: &str = "monitorGpu";
const MONITOR_PROCESSES_SERVICE_ID: &str = "monitorProcesses";
const SAMPLE_FAILURE_DIAGNOSTIC: &str = "monitor.sampleFailed";
const PROCESS_SKIPPED_DIAGNOSTIC: &str = "monitor.processSkipped";
const EMIT_FAILURE_DIAGNOSTIC: &str = "events.monitorMetricsChangedEmitFailed";

pub struct OptionalMetricSources {
    pub processes: Mutex<Box<dyn ProcessMetricsSource>>,
    pub gpu: Mutex<Box<dyn GpuMetricsSource>>,
}

#[derive(Clone)]
pub struct MonitorSamplerFactory {
    repository: MonitorRepository,
    health: ServiceHealthRepository,
    diagnostics: DiagnosticsRepository,
    thresholds: Arc<dyn ThresholdEvaluationPort>,
    app: tauri::AppHandle,
}

impl MonitorSamplerFactory {
    pub fn new(
        repository: MonitorRepository,
        health: ServiceHealthRepository,
        diagnostics: DiagnosticsRepository,
        thresholds: Arc<dyn ThresholdEvaluationPort>,
        app: tauri::AppHandle,
    ) -> Self {
        Self {
            repository,
            health,
            diagnostics,
            thresholds,
            app,
        }
    }

    pub fn create_worker(
        &self,
        generation: u64,
        current_generation: Arc<AtomicU64>,
    ) -> Result<MonitorSamplerWorker, CommandError> {
        let generation_gate = Arc::new(MonitorGenerationGate::new(current_generation.clone()));
        self.create_worker_with_gate(generation, current_generation, generation_gate)
    }

    pub(crate) fn create_worker_with_gate(
        &self,
        generation: u64,
        current_generation: Arc<AtomicU64>,
        generation_gate: Arc<MonitorGenerationGate>,
    ) -> Result<MonitorSamplerWorker, CommandError> {
        debug_assert!(Arc::ptr_eq(
            &current_generation,
            &generation_gate.current_generation
        ));
        let source = WindowsCoreMetricsSource::new().map_err(metric_fault_error)?;
        let process_skips = Arc::new(Mutex::new(Vec::new()));
        let processes = WindowsProcessMetricsSource::with_skip_collector(process_skips.clone());
        Ok(MonitorSamplerWorker {
            source: Box::new(source),
            optional_sources: Arc::new(OptionalMetricSources {
                processes: Mutex::new(Box::new(processes)),
                gpu: Mutex::new(Box::new(WindowsGpuMetricsSource::new())),
            }),
            process_skips,
            previous_optional_at: None,
            generation,
            current_generation,
            generation_gate,
            interval: Duration::from_secs(2),
            repository: self.repository.clone(),
            health: self.health.clone(),
            diagnostics: self.diagnostics.clone(),
            thresholds: Some(self.thresholds.clone()),
            emitter: Arc::new(TauriEventEmitter {
                app: self.app.clone(),
            }),
            #[cfg(test)]
            side_effect_hook: None,
        })
    }
}

/// Serializes generation transitions with monitor side effects.
///
/// Invariant: production code may change `current_generation` only through
/// `advance`. A worker must hold the guard returned by `enter` for the entire
/// persistence/health/diagnostic/event chain. Therefore a transition either
/// linearizes before an old worker enters (and rejects it) or after every old
/// side effect has completed; it can never split that chain.
pub(crate) struct MonitorGenerationGate {
    current_generation: Arc<AtomicU64>,
    side_effects: Mutex<()>,
}

impl MonitorGenerationGate {
    pub(crate) fn new(current_generation: Arc<AtomicU64>) -> Self {
        Self {
            current_generation,
            side_effects: Mutex::new(()),
        }
    }

    pub(crate) fn current_generation(&self) -> Arc<AtomicU64> {
        self.current_generation.clone()
    }

    pub(crate) fn advance(&self) -> u64 {
        let _transition = self
            .side_effects
            .lock()
            .expect("monitor generation gate poisoned");
        self.current_generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn enter(&self, generation: u64) -> Result<MutexGuard<'_, ()>, CommandError> {
        let guard = self
            .side_effects
            .lock()
            .map_err(|_| crate::services::service_stopping_error())?;
        if self.current_generation.load(Ordering::Acquire) != generation {
            return Err(crate::services::service_stopping_error());
        }
        Ok(guard)
    }
}

pub struct MonitorSamplerWorker {
    source: Box<dyn CoreMetricsSource>,
    optional_sources: Arc<OptionalMetricSources>,
    process_skips: ProcessSkipCollector,
    previous_optional_at: Option<Instant>,
    generation: u64,
    current_generation: Arc<AtomicU64>,
    generation_gate: Arc<MonitorGenerationGate>,
    interval: Duration,
    repository: MonitorRepository,
    health: ServiceHealthRepository,
    diagnostics: DiagnosticsRepository,
    thresholds: Option<Arc<dyn ThresholdEvaluationPort>>,
    emitter: Arc<dyn EventEmitterPort>,
    #[cfg(test)]
    side_effect_hook: Option<Arc<dyn Fn(SideEffectBoundary) + Send + Sync>>,
}

#[cfg(test)]
struct TestProcessSource;

#[cfg(test)]
impl ProcessMetricsSource for TestProcessSource {
    fn capture(
        &mut self,
        _watches: &[crate::contracts::ProcessWatch],
        _elapsed: Duration,
        _sampled_at: i64,
    ) -> Result<Vec<crate::domain::monitor::NewProcessSample>, MetricFault> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
struct TestGpuSource;

#[cfg(test)]
impl GpuMetricsSource for TestGpuSource {
    fn capture_percent(&mut self) -> Result<Option<f64>, MetricFault> {
        Ok(Some(0.0))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SideEffectBoundary {
    Persist,
    Health,
    Emit,
    Diagnostic,
}

impl MonitorSamplerWorker {
    #[cfg(test)]
    pub(crate) fn from_test_parts(
        source: Box<dyn CoreMetricsSource>,
        generation: u64,
        generation_gate: Arc<MonitorGenerationGate>,
        repository: MonitorRepository,
        health: ServiceHealthRepository,
        diagnostics: DiagnosticsRepository,
        emitter: Arc<dyn EventEmitterPort>,
    ) -> Self {
        Self {
            source,
            optional_sources: Arc::new(OptionalMetricSources {
                processes: Mutex::new(Box::new(TestProcessSource)),
                gpu: Mutex::new(Box::new(TestGpuSource)),
            }),
            process_skips: Arc::new(Mutex::new(Vec::new())),
            previous_optional_at: None,
            generation,
            current_generation: generation_gate.current_generation(),
            generation_gate,
            // Tokio intervals tick immediately; keep later ticks out of the
            // coordinator restart test so each source contributes one baseline.
            interval: Duration::from_secs(60),
            repository,
            health,
            diagnostics,
            thresholds: None,
            emitter,
            side_effect_hook: None,
        }
    }

    pub async fn run(mut self, mut cancel: tokio::sync::watch::Receiver<bool>) {
        let mut ticker = tokio::time::interval(self.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                changed = cancel.changed() => {
                    if changed.is_err() || *cancel.borrow() {
                        break;
                    }
                }
                _ = ticker.tick() => {
                    let sampled_at = unix_now_millis();
                    if self
                        .sample_once(Instant::now(), sampled_at)
                        .is_err_and(|error| error.message_key == "errors.serviceStopping")
                    {
                        break;
                    }
                }
            }
        }
    }

    pub fn sample_once(
        &mut self,
        monotonic_now: Instant,
        unix_now: i64,
    ) -> Result<MonitorSnapshot, CommandError> {
        if !self.is_current() {
            return Err(crate::services::service_stopping_error());
        }
        let mut capture = match self.source.capture(monotonic_now, unix_now) {
            Ok(capture) => capture,
            Err(fault) => {
                let _side_effect = self.generation_gate.enter(self.generation)?;
                self.record_failure(fault.metric, fault.reason_code, unix_now)?;
                return Err(metric_fault_error(fault));
            }
        };
        let optional = self.capture_optional_sources(&mut capture, monotonic_now, unix_now);
        let _side_effect = self.generation_gate.enter(self.generation)?;
        self.call_side_effect_hook(SideEffectBoundary::Persist);
        if let Err(error) = self
            .repository
            .insert_sample(&capture.sample, &capture.processes)
        {
            self.record_failure("repository", error_reason(&error), unix_now)?;
            return Err(error);
        }
        let snapshot = snapshot_from_sample(&capture.sample);
        let threshold_error = self
            .thresholds
            .as_ref()
            .and_then(|thresholds| thresholds.evaluate(&snapshot, unix_now).err());
        self.call_side_effect_hook(SideEffectBoundary::Health);
        if let Err(error) = self.health.upsert(&ServiceHealthSnapshot {
            service_id: MONITOR_CORE_SERVICE_ID.into(),
            state: ServiceHealthState::Healthy,
            message_key: "services.healthy".into(),
            parameters: BTreeMap::from([(
                "serviceId".into(),
                SafeParameterValue::String(MONITOR_CORE_SERVICE_ID.into()),
            )]),
            checked_at: unix_now,
        }) {
            self.record_diagnostic(
                "health",
                error_reason(&error),
                unix_now,
                SAMPLE_FAILURE_DIAGNOSTIC,
            );
            return Err(error);
        }
        self.apply_optional_outcome(optional, unix_now);
        self.call_side_effect_hook(SideEffectBoundary::Emit);
        if self
            .emitter
            .emit(
                MONITOR_METRICS_CHANGED,
                monitor_metrics_changed_payload(unix_now),
            )
            .is_err()
        {
            self.record_diagnostic("event", "emitFailed", unix_now, EMIT_FAILURE_DIAGNOSTIC);
        }
        if let Some(error) = threshold_error {
            return Err(error);
        }
        Ok(snapshot)
    }

    fn capture_optional_sources(
        &mut self,
        capture: &mut crate::services::system_metrics::CoreMetricCapture,
        monotonic_now: Instant,
        sampled_at: i64,
    ) -> OptionalCaptureOutcome {
        let elapsed = self
            .previous_optional_at
            .and_then(|prior| monotonic_now.checked_duration_since(prior))
            .filter(|elapsed| !elapsed.is_zero())
            .unwrap_or(self.interval);

        let gpu = match self.optional_sources.gpu.lock() {
            Ok(mut source) => match source.capture_percent() {
                Ok(Some(value)) if value.is_finite() && (0.0..=100.0).contains(&value) => {
                    capture.sample.gpu_percent = Some(value);
                    OptionalHealth::healthy(MONITOR_GPU_SERVICE_ID)
                }
                Ok(Some(_)) => {
                    capture.sample.gpu_percent = None;
                    OptionalHealth::degraded(MONITOR_GPU_SERVICE_ID, "gpu", "counterInvalid")
                }
                Ok(None) => {
                    capture.sample.gpu_percent = None;
                    OptionalHealth::degraded(MONITOR_GPU_SERVICE_ID, "gpu", "counterUnavailable")
                }
                Err(fault) => {
                    capture.sample.gpu_percent = None;
                    OptionalHealth::degraded(
                        MONITOR_GPU_SERVICE_ID,
                        fault.metric,
                        fault.reason_code,
                    )
                }
            },
            Err(_) => OptionalHealth::degraded(MONITOR_GPU_SERVICE_ID, "gpu", "lockPoisoned"),
        };

        let processes = match self.repository.list_process_watches() {
            Ok(watches) => match self.optional_sources.processes.lock() {
                Ok(mut source) => match source.capture(&watches, elapsed, sampled_at) {
                    Ok(rows) => {
                        self.previous_optional_at = Some(monotonic_now);
                        capture.processes = rows;
                        OptionalHealth::healthy(MONITOR_PROCESSES_SERVICE_ID)
                    }
                    Err(fault) => {
                        capture.processes.clear();
                        OptionalHealth::degraded(
                            MONITOR_PROCESSES_SERVICE_ID,
                            fault.metric,
                            fault.reason_code,
                        )
                    }
                },
                Err(_) => OptionalHealth::degraded(
                    MONITOR_PROCESSES_SERVICE_ID,
                    "processes",
                    "lockPoisoned",
                ),
            },
            Err(error) => OptionalHealth::degraded(
                MONITOR_PROCESSES_SERVICE_ID,
                "processes",
                error_reason(&error),
            ),
        };
        let skips = self
            .process_skips
            .lock()
            .map(|mut values| std::mem::take(&mut *values))
            .unwrap_or_default();
        OptionalCaptureOutcome {
            gpu,
            processes,
            skips,
        }
    }

    fn apply_optional_outcome(&self, outcome: OptionalCaptureOutcome, sampled_at: i64) {
        for optional in [outcome.gpu, outcome.processes] {
            self.call_side_effect_hook(SideEffectBoundary::Health);
            let health_result = self.health.upsert(&ServiceHealthSnapshot {
                service_id: optional.service_id.into(),
                state: optional.state.clone(),
                message_key: if optional.state == ServiceHealthState::Healthy {
                    "services.healthy"
                } else {
                    "services.degraded"
                }
                .into(),
                parameters: if optional.state == ServiceHealthState::Healthy {
                    BTreeMap::from([(
                        "serviceId".into(),
                        SafeParameterValue::String(optional.service_id.into()),
                    )])
                } else {
                    BTreeMap::from([
                        (
                            "serviceId".into(),
                            SafeParameterValue::String(optional.service_id.into()),
                        ),
                        (
                            "reasonCode".into(),
                            SafeParameterValue::String(optional.reason_code.into()),
                        ),
                    ])
                },
                checked_at: sampled_at,
            });
            if let Err(error) = health_result {
                self.record_diagnostic(
                    "health",
                    error_reason(&error),
                    sampled_at,
                    SAMPLE_FAILURE_DIAGNOSTIC,
                );
            }
            if optional.state == ServiceHealthState::Degraded {
                self.record_optional_diagnostic(optional, sampled_at);
            }
        }
        for skip in outcome.skips {
            self.record_process_skip(skip);
        }
    }

    fn record_optional_diagnostic(&self, optional: OptionalHealth, sampled_at: i64) {
        self.call_side_effect_hook(SideEffectBoundary::Diagnostic);
        let _ = self.diagnostics.record(&DiagnosticEvent {
            id: Uuid::new_v4().to_string(),
            service_id: optional.service_id.into(),
            level: DiagnosticLevel::Failure,
            code: SAMPLE_FAILURE_DIAGNOSTIC.into(),
            parameters: BTreeMap::from([
                (
                    "metric".into(),
                    SafeParameterValue::String(optional.metric.into()),
                ),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String(optional.reason_code.into()),
                ),
                (
                    "sampledAt".into(),
                    SafeParameterValue::Number(sampled_at.into()),
                ),
            ]),
            created_at: sampled_at,
        });
    }

    fn record_process_skip(&self, skip: ProcessSkip) {
        self.call_side_effect_hook(SideEffectBoundary::Diagnostic);
        let _ = self.diagnostics.record(&DiagnosticEvent {
            id: Uuid::new_v4().to_string(),
            service_id: MONITOR_PROCESSES_SERVICE_ID.into(),
            level: DiagnosticLevel::Failure,
            code: PROCESS_SKIPPED_DIAGNOSTIC.into(),
            parameters: BTreeMap::from([
                ("watchId".into(), SafeParameterValue::String(skip.watch_id)),
                (
                    "skippedCount".into(),
                    SafeParameterValue::Number(skip.skipped_count.into()),
                ),
            ]),
            created_at: skip.sampled_at,
        });
    }

    fn is_current(&self) -> bool {
        self.current_generation.load(Ordering::Acquire) == self.generation
    }

    fn record_failure(
        &self,
        metric: &'static str,
        reason_code: &'static str,
        sampled_at: i64,
    ) -> Result<(), CommandError> {
        self.call_side_effect_hook(SideEffectBoundary::Health);
        if let Err(error) = self.health.upsert(&ServiceHealthSnapshot {
            service_id: MONITOR_CORE_SERVICE_ID.into(),
            state: ServiceHealthState::Degraded,
            message_key: "services.degraded".into(),
            parameters: BTreeMap::from([
                (
                    "serviceId".into(),
                    SafeParameterValue::String(MONITOR_CORE_SERVICE_ID.into()),
                ),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String(reason_code.into()),
                ),
            ]),
            checked_at: sampled_at,
        }) {
            self.record_diagnostic(
                "health",
                error_reason(&error),
                sampled_at,
                SAMPLE_FAILURE_DIAGNOSTIC,
            );
            return Err(error);
        }
        self.record_diagnostic(metric, reason_code, sampled_at, SAMPLE_FAILURE_DIAGNOSTIC);
        Ok(())
    }

    fn record_diagnostic(
        &self,
        metric: &'static str,
        reason_code: &'static str,
        sampled_at: i64,
        code: &'static str,
    ) {
        self.call_side_effect_hook(SideEffectBoundary::Diagnostic);
        let _ = self.diagnostics.record(&DiagnosticEvent {
            id: Uuid::new_v4().to_string(),
            service_id: MONITOR_CORE_SERVICE_ID.into(),
            level: DiagnosticLevel::Failure,
            code: code.into(),
            parameters: BTreeMap::from([
                ("metric".into(), SafeParameterValue::String(metric.into())),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String(reason_code.into()),
                ),
                (
                    "sampledAt".into(),
                    SafeParameterValue::Number(sampled_at.into()),
                ),
            ]),
            created_at: sampled_at,
        });
    }

    fn call_side_effect_hook(&self, boundary: SideEffectBoundary) {
        #[cfg(test)]
        if let Some(hook) = &self.side_effect_hook {
            hook(boundary);
        }
        #[cfg(not(test))]
        let _ = boundary;
    }
}

struct OptionalCaptureOutcome {
    gpu: OptionalHealth,
    processes: OptionalHealth,
    skips: Vec<ProcessSkip>,
}

#[derive(Clone)]
struct OptionalHealth {
    service_id: &'static str,
    state: ServiceHealthState,
    metric: &'static str,
    reason_code: &'static str,
}

impl OptionalHealth {
    fn healthy(service_id: &'static str) -> Self {
        Self {
            service_id,
            state: ServiceHealthState::Healthy,
            metric: "optional",
            reason_code: "none",
        }
    }

    fn degraded(service_id: &'static str, metric: &'static str, reason_code: &'static str) -> Self {
        Self {
            service_id,
            state: ServiceHealthState::Degraded,
            metric,
            reason_code,
        }
    }
}

pub(crate) fn metric_fault_error(fault: MetricFault) -> CommandError {
    CommandError {
        code: AppErrorCode::SourceUnavailable,
        message_key: "errors.sourceUnavailable".into(),
        details: SafeMessageParameters::from([
            (
                "serviceId".into(),
                SafeParameterValue::String(MONITOR_CORE_SERVICE_ID.into()),
            ),
            (
                "reasonCode".into(),
                SafeParameterValue::String(fault.reason_code.into()),
            ),
        ]),
        retryable: true,
    }
}

fn snapshot_from_sample(sample: &crate::domain::monitor::NewMonitorSample) -> MonitorSnapshot {
    MonitorSnapshot {
        cpu_percent: sample.cpu_percent as i64,
        memory_used_bytes: sample.memory_used_bytes,
        memory_total_bytes: sample.memory_total_bytes,
        disk_read_bytes_per_second: sample.disk_read_bps as i64,
        disk_write_bytes_per_second: sample.disk_write_bps as i64,
        network_receive_bytes_per_second: sample.network_rx_bps as i64,
        network_send_bytes_per_second: sample.network_tx_bps as i64,
        gpu_percent: sample.gpu_percent.map(|value| value as i64),
        sampled_at: sample.sampled_at,
    }
}

fn error_reason(error: &CommandError) -> &'static str {
    match error.code {
        AppErrorCode::DatabaseFailure => "databaseFailure",
        AppErrorCode::InvalidInput => "invalidSample",
        _ => "persistFailed",
    }
}

fn unix_now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::monitor::NewMonitorSample;
    use crate::services::system_metrics::CoreMetricCapture;
    use crate::storage::Storage;
    use std::sync::Mutex;

    struct QueueSource {
        captures: Vec<Result<CoreMetricCapture, MetricFault>>,
        switch_generation: Option<(Arc<AtomicU64>, u64)>,
    }

    impl CoreMetricsSource for QueueSource {
        fn capture(
            &mut self,
            _monotonic_now: Instant,
            _unix_now: i64,
        ) -> Result<CoreMetricCapture, MetricFault> {
            if let Some((generation, value)) = self.switch_generation.take() {
                generation.store(value, Ordering::Release);
            }
            self.captures.remove(0)
        }
    }

    #[derive(Default)]
    struct RecordingEmitter {
        events: Mutex<Vec<(&'static str, serde_json::Value)>>,
        reject: bool,
        expect_committed: Mutex<Option<(MonitorRepository, i64)>>,
    }

    impl EventEmitterPort for RecordingEmitter {
        fn emit(&self, name: &'static str, payload: serde_json::Value) -> Result<(), CommandError> {
            if let Some((repository, sampled_at)) = self.expect_committed.lock().unwrap().take() {
                assert_eq!(
                    repository.latest().unwrap().map(|sample| sample.sampled_at),
                    Some(sampled_at),
                    "monitor event must be emitted only after the sample transaction commits"
                );
            }
            if self.reject {
                return Err(metric_fault_error(MetricFault {
                    metric: "event",
                    reason_code: "emitFailed",
                }));
            }
            self.events.lock().unwrap().push((name, payload));
            Ok(())
        }
    }

    struct Fixture {
        worker: MonitorSamplerWorker,
        storage: Arc<Storage>,
        generation_gate: Arc<MonitorGenerationGate>,
        repository: MonitorRepository,
        health: ServiceHealthRepository,
        diagnostics: DiagnosticsRepository,
        emitter: Arc<RecordingEmitter>,
    }

    fn capture(sampled_at: i64) -> CoreMetricCapture {
        CoreMetricCapture {
            sample: NewMonitorSample {
                cpu_percent: 25.0,
                memory_used_bytes: 400,
                memory_total_bytes: 1_000,
                disk_read_bps: 10.0,
                disk_write_bps: 20.0,
                network_rx_bps: 30.0,
                network_tx_bps: 40.0,
                gpu_percent: None,
                sampled_at,
            },
            processes: Vec::new(),
        }
    }

    fn fixture(
        captures: Vec<Result<CoreMetricCapture, MetricFault>>,
        generation: Arc<AtomicU64>,
        switch_generation: Option<(Arc<AtomicU64>, u64)>,
        reject_emit: bool,
    ) -> Fixture {
        let directory = tempfile::tempdir().unwrap().keep();
        let storage = Arc::new(Storage::open(&directory).unwrap());
        let repository = MonitorRepository::new(storage.clone());
        let health = ServiceHealthRepository::new(storage.clone());
        let diagnostics = DiagnosticsRepository::new(storage.clone());
        let emitter = Arc::new(RecordingEmitter {
            reject: reject_emit,
            ..Default::default()
        });
        let generation_gate = Arc::new(MonitorGenerationGate::new(generation.clone()));
        Fixture {
            worker: MonitorSamplerWorker {
                source: Box::new(QueueSource {
                    captures,
                    switch_generation,
                }),
                optional_sources: Arc::new(OptionalMetricSources {
                    processes: Mutex::new(Box::new(TestProcessSource)),
                    gpu: Mutex::new(Box::new(TestGpuSource)),
                }),
                process_skips: Arc::new(Mutex::new(Vec::new())),
                previous_optional_at: None,
                generation: 1,
                current_generation: generation,
                generation_gate: generation_gate.clone(),
                interval: Duration::from_secs(2),
                repository: repository.clone(),
                health: health.clone(),
                diagnostics: diagnostics.clone(),
                thresholds: None,
                emitter: emitter.clone(),
                side_effect_hook: None,
            },
            storage,
            generation_gate,
            repository,
            health,
            diagnostics,
            emitter,
        }
    }

    struct FaultGpuSource;

    impl GpuMetricsSource for FaultGpuSource {
        fn capture_percent(&mut self) -> Result<Option<f64>, MetricFault> {
            Err(MetricFault {
                metric: "gpu",
                reason_code: "queryFailed",
            })
        }
    }

    struct MissingGpuSource;

    impl GpuMetricsSource for MissingGpuSource {
        fn capture_percent(&mut self) -> Result<Option<f64>, MetricFault> {
            Ok(None)
        }
    }

    struct FixedGpuSource(f64);

    impl GpuMetricsSource for FixedGpuSource {
        fn capture_percent(&mut self) -> Result<Option<f64>, MetricFault> {
            Ok(Some(self.0))
        }
    }

    struct FaultProcessSource;

    impl ProcessMetricsSource for FaultProcessSource {
        fn capture(
            &mut self,
            _watches: &[crate::contracts::ProcessWatch],
            _elapsed: Duration,
            _sampled_at: i64,
        ) -> Result<Vec<crate::domain::monitor::NewProcessSample>, MetricFault> {
            Err(MetricFault {
                metric: "processes",
                reason_code: "snapshotFailed",
            })
        }
    }

    struct FixedProcessSource(Vec<crate::domain::monitor::NewProcessSample>);

    impl ProcessMetricsSource for FixedProcessSource {
        fn capture(
            &mut self,
            _watches: &[crate::contracts::ProcessWatch],
            _elapsed: Duration,
            _sampled_at: i64,
        ) -> Result<Vec<crate::domain::monitor::NewProcessSample>, MetricFault> {
            Ok(self.0.clone())
        }
    }

    struct RecordingProcessSource {
        elapsed: Arc<Mutex<Vec<Duration>>>,
        outcomes: Vec<Result<Vec<crate::domain::monitor::NewProcessSample>, MetricFault>>,
    }

    impl ProcessMetricsSource for RecordingProcessSource {
        fn capture(
            &mut self,
            _watches: &[crate::contracts::ProcessWatch],
            elapsed: Duration,
            _sampled_at: i64,
        ) -> Result<Vec<crate::domain::monitor::NewProcessSample>, MetricFault> {
            self.elapsed.lock().unwrap().push(elapsed);
            self.outcomes.remove(0)
        }
    }

    struct QueueGpuSource(Vec<Result<Option<f64>, MetricFault>>);

    impl GpuMetricsSource for QueueGpuSource {
        fn capture_percent(&mut self) -> Result<Option<f64>, MetricFault> {
            self.0.remove(0)
        }
    }

    #[test]
    fn optional_sources_gpu_fault_keeps_core_persistence_event_and_local_health() {
        let generation = Arc::new(AtomicU64::new(1));
        let mut fixture = fixture(vec![Ok(capture(77))], generation, None, false);
        fixture.worker.optional_sources = Arc::new(OptionalMetricSources {
            processes: Mutex::new(Box::new(TestProcessSource)),
            gpu: Mutex::new(Box::new(FaultGpuSource)),
        });

        let snapshot = fixture.worker.sample_once(Instant::now(), 77).unwrap();
        assert_eq!(snapshot.gpu_percent, None);
        assert_eq!(fixture.repository.latest().unwrap(), Some(snapshot));
        assert_eq!(fixture.emitter.events.lock().unwrap().len(), 1);
        let health = fixture.health.list().unwrap();
        assert!(health.iter().any(|entry| {
            entry.service_id == "monitorGpu" && entry.state == ServiceHealthState::Degraded
        }));
        assert!(!health.iter().any(|entry| {
            entry.service_id == "monitorCore" && entry.state == ServiceHealthState::Degraded
        }));
    }

    #[test]
    fn optional_sources_missing_gpu_persists_null_and_marks_only_gpu_degraded() {
        let generation = Arc::new(AtomicU64::new(1));
        let mut fixture = fixture(vec![Ok(capture(78))], generation, None, false);
        fixture.worker.optional_sources = Arc::new(OptionalMetricSources {
            processes: Mutex::new(Box::new(TestProcessSource)),
            gpu: Mutex::new(Box::new(MissingGpuSource)),
        });

        let snapshot = fixture.worker.sample_once(Instant::now(), 78).unwrap();
        assert_eq!(snapshot.gpu_percent, None);
        assert_eq!(fixture.emitter.events.lock().unwrap().len(), 1);
        let health = fixture.health.list().unwrap();
        assert!(health.iter().any(|entry| {
            entry.service_id == "monitorGpu" && entry.state == ServiceHealthState::Degraded
        }));
        assert!(!health.iter().any(|entry| {
            entry.service_id == "monitorProcesses" && entry.state == ServiceHealthState::Degraded
        }));
    }

    #[test]
    fn optional_sources_process_fault_keeps_core_gpu_event_and_no_process_rows() {
        let generation = Arc::new(AtomicU64::new(1));
        let mut fixture = fixture(vec![Ok(capture(79))], generation, None, false);
        fixture.worker.optional_sources = Arc::new(OptionalMetricSources {
            processes: Mutex::new(Box::new(FaultProcessSource)),
            gpu: Mutex::new(Box::new(FixedGpuSource(44.0))),
        });

        let snapshot = fixture.worker.sample_once(Instant::now(), 79).unwrap();
        assert_eq!(snapshot.gpu_percent, Some(44));
        assert!(fixture
            .repository
            .list_process_metrics(10)
            .unwrap()
            .is_empty());
        assert_eq!(fixture.emitter.events.lock().unwrap().len(), 1);
        let health = fixture.health.list().unwrap();
        assert!(health.iter().any(|entry| {
            entry.service_id == "monitorProcesses" && entry.state == ServiceHealthState::Degraded
        }));
        assert!(!health.iter().any(|entry| {
            entry.service_id == "monitorGpu" && entry.state == ServiceHealthState::Degraded
        }));
    }

    #[test]
    fn process_elapsed_uses_last_success_across_a_fault() {
        let generation = Arc::new(AtomicU64::new(1));
        let mut fixture = fixture(
            vec![Ok(capture(79)), Ok(capture(81)), Ok(capture(83))],
            generation,
            None,
            false,
        );
        let elapsed = Arc::new(Mutex::new(Vec::new()));
        fixture.worker.optional_sources = Arc::new(OptionalMetricSources {
            processes: Mutex::new(Box::new(RecordingProcessSource {
                elapsed: elapsed.clone(),
                outcomes: vec![
                    Ok(Vec::new()),
                    Err(MetricFault {
                        metric: "processes",
                        reason_code: "snapshotFailed",
                    }),
                    Ok(Vec::new()),
                ],
            })),
            gpu: Mutex::new(Box::new(FixedGpuSource(44.0))),
        });
        let start = Instant::now();

        fixture.worker.sample_once(start, 79).unwrap();
        fixture
            .worker
            .sample_once(start + Duration::from_secs(2), 81)
            .unwrap();
        fixture
            .worker
            .sample_once(start + Duration::from_secs(4), 83)
            .unwrap();

        assert_eq!(
            *elapsed.lock().unwrap(),
            vec![
                Duration::from_secs(2),
                Duration::from_secs(2),
                Duration::from_secs(4),
            ]
        );
    }

    #[test]
    fn optional_sources_process_skip_diagnostic_has_only_safe_watch_and_count() {
        let generation = Arc::new(AtomicU64::new(1));
        let mut fixture = fixture(vec![Ok(capture(80))], generation, None, false);
        fixture
            .worker
            .process_skips
            .lock()
            .unwrap()
            .push(ProcessSkip {
                watch_id: "00000000-0000-0000-0000-000000000001".into(),
                skipped_count: 3,
                sampled_at: 80,
            });

        fixture.worker.sample_once(Instant::now(), 80).unwrap();
        let diagnostic = fixture.diagnostics.list(1).unwrap().remove(0);
        assert_eq!(diagnostic.code, PROCESS_SKIPPED_DIAGNOSTIC);
        assert_eq!(
            diagnostic.parameters,
            BTreeMap::from([
                (
                    "watchId".into(),
                    SafeParameterValue::String("00000000-0000-0000-0000-000000000001".into(),),
                ),
                ("skippedCount".into(), SafeParameterValue::Number(3.into()),),
            ])
        );
    }

    #[test]
    fn optional_sources_successful_gpu_retry_restores_healthy_state() {
        let generation = Arc::new(AtomicU64::new(1));
        let mut fixture = fixture(
            vec![Ok(capture(81)), Ok(capture(82))],
            generation,
            None,
            false,
        );
        fixture.worker.optional_sources = Arc::new(OptionalMetricSources {
            processes: Mutex::new(Box::new(TestProcessSource)),
            gpu: Mutex::new(Box::new(QueueGpuSource(vec![Ok(None), Ok(Some(55.0))]))),
        });

        assert_eq!(
            fixture
                .worker
                .sample_once(Instant::now(), 81)
                .unwrap()
                .gpu_percent,
            None
        );
        assert_eq!(
            fixture
                .worker
                .sample_once(Instant::now() + Duration::from_secs(2), 82)
                .unwrap()
                .gpu_percent,
            Some(55)
        );
        let gpu = fixture
            .health
            .list()
            .unwrap()
            .into_iter()
            .find(|entry| entry.service_id == "monitorGpu")
            .unwrap();
        assert_eq!(gpu.state, ServiceHealthState::Healthy);
        assert_eq!(gpu.checked_at, 82);
    }

    #[test]
    fn optional_sources_process_rows_commit_with_the_core_sample() {
        let generation = Arc::new(AtomicU64::new(1));
        let mut fixture = fixture(vec![Ok(capture(83))], generation, None, false);
        let watch = fixture
            .repository
            .save_process_watch(
                crate::contracts::SaveProcessWatchInput {
                    id: None,
                    process_name: "editor.exe".into(),
                    enabled: true,
                    expected_revision: None,
                },
                1,
            )
            .unwrap();
        fixture.worker.optional_sources = Arc::new(OptionalMetricSources {
            processes: Mutex::new(Box::new(FixedProcessSource(vec![
                crate::domain::monitor::NewProcessSample {
                    process_watch_id: Uuid::parse_str(&watch.id).unwrap(),
                    pid: 42,
                    process_name: "editor.exe".into(),
                    cpu_percent: 12.5,
                    memory_bytes: 4_096,
                },
            ]))),
            gpu: Mutex::new(Box::new(FixedGpuSource(20.0))),
        });

        fixture.worker.sample_once(Instant::now(), 83).unwrap();
        let rows = fixture.repository.list_process_metrics(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, 42);
        assert_eq!(rows[0].cpu_percent, 12.5);
        assert_eq!(rows[0].memory_bytes, 4_096);
        assert_eq!(rows[0].sampled_at, 83);
    }

    #[test]
    fn first_source_baseline_does_not_insert_or_emit_and_records_safe_fault_state() {
        let generation = Arc::new(AtomicU64::new(1));
        let mut fixture = fixture(
            vec![Err(MetricFault {
                metric: "core",
                reason_code: "baselinePending",
            })],
            generation,
            None,
            false,
        );
        let error = fixture.worker.sample_once(Instant::now(), 10).unwrap_err();
        assert_eq!(error.code, AppErrorCode::SourceUnavailable);
        assert_eq!(fixture.repository.latest().unwrap(), None);
        assert!(fixture.emitter.events.lock().unwrap().is_empty());
        let event = fixture.diagnostics.list(1).unwrap().remove(0);
        assert_eq!(event.code, SAMPLE_FAILURE_DIAGNOSTIC);
        assert_eq!(
            event.parameters.keys().cloned().collect::<Vec<_>>(),
            vec!["metric", "reasonCode", "sampledAt"]
        );
    }

    #[test]
    fn valid_capture_commits_before_emitting_the_exact_typed_hint_and_marks_healthy() {
        let generation = Arc::new(AtomicU64::new(1));
        let mut fixture = fixture(vec![Ok(capture(42))], generation, None, false);
        *fixture.emitter.expect_committed.lock().unwrap() = Some((fixture.repository.clone(), 42));
        let snapshot = fixture.worker.sample_once(Instant::now(), 42).unwrap();
        assert_eq!(fixture.repository.latest().unwrap(), Some(snapshot));
        assert_eq!(
            fixture.emitter.events.lock().unwrap().as_slice(),
            &[(
                MONITOR_METRICS_CHANGED,
                serde_json::json!({ "sampledAt": 42 })
            )]
        );
        let health = fixture.health.list().unwrap().remove(0);
        assert_eq!(health.state, ServiceHealthState::Healthy);
        assert_eq!(health.message_key, "services.healthy");
    }

    #[test]
    fn repository_or_event_fault_emits_no_fabricated_hint_and_records_only_safe_parameters() {
        let generation = Arc::new(AtomicU64::new(1));
        let mut invalid = capture(20);
        invalid.sample.memory_total_bytes = 0;
        let mut repository_fixture = fixture(vec![Ok(invalid)], generation, None, false);
        assert_eq!(
            repository_fixture
                .worker
                .sample_once(Instant::now(), 20)
                .unwrap_err()
                .code,
            AppErrorCode::InvalidInput
        );
        assert!(repository_fixture.emitter.events.lock().unwrap().is_empty());
        assert_eq!(
            repository_fixture.diagnostics.list(1).unwrap()[0]
                .parameters
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["metric", "reasonCode", "sampledAt"]
        );

        let generation = Arc::new(AtomicU64::new(1));
        let mut emit_fixture = fixture(vec![Ok(capture(21))], generation, None, true);
        emit_fixture.worker.sample_once(Instant::now(), 21).unwrap();
        assert_eq!(
            emit_fixture.diagnostics.list(1).unwrap()[0].code,
            EMIT_FAILURE_DIAGNOSTIC
        );
    }

    #[test]
    fn delayed_stale_generation_cannot_persist_emit_or_overwrite_health() {
        let generation = Arc::new(AtomicU64::new(1));
        let mut fixture = fixture(
            vec![Ok(capture(99))],
            generation.clone(),
            Some((generation.clone(), 2)),
            false,
        );
        let error = fixture.worker.sample_once(Instant::now(), 99).unwrap_err();
        assert_eq!(error.message_key, "errors.serviceStopping");
        assert_eq!(fixture.repository.latest().unwrap(), None);
        assert!(fixture.health.list().unwrap().is_empty());
        assert!(fixture.diagnostics.list(10).unwrap().is_empty());
        assert!(fixture.emitter.events.lock().unwrap().is_empty());
    }

    #[test]
    fn generation_transition_is_serialized_at_every_side_effect_boundary() {
        for (boundary, reject_emit) in [
            (SideEffectBoundary::Persist, false),
            (SideEffectBoundary::Health, false),
            (SideEffectBoundary::Emit, false),
            (SideEffectBoundary::Diagnostic, true),
        ] {
            let generation = Arc::new(AtomicU64::new(1));
            let mut fixture = fixture(
                vec![Ok(capture(100)), Ok(capture(101))],
                generation,
                None,
                reject_emit,
            );
            let entered = Arc::new(std::sync::Barrier::new(2));
            let release = Arc::new(std::sync::Barrier::new(2));
            let blocked_once = Arc::new(std::sync::atomic::AtomicBool::new(false));
            fixture.worker.side_effect_hook = Some(Arc::new({
                let entered = entered.clone();
                let release = release.clone();
                let blocked_once = blocked_once.clone();
                move |observed| {
                    if observed == boundary
                        && blocked_once
                            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                            .is_ok()
                    {
                        entered.wait();
                        release.wait();
                    }
                }
            }));
            let gate = fixture.generation_gate.clone();
            let sample = std::thread::spawn(move || {
                let result = fixture.worker.sample_once(Instant::now(), 100);
                (fixture, result)
            });
            entered.wait();
            let (blocked_tx, blocked_rx) = std::sync::mpsc::channel();
            let (advanced_tx, advanced_rx) = std::sync::mpsc::channel();
            let transition = std::thread::spawn(move || {
                blocked_tx
                    .send(matches!(
                        gate.side_effects.try_lock(),
                        Err(std::sync::TryLockError::WouldBlock)
                    ))
                    .unwrap();
                advanced_tx.send(gate.advance()).unwrap();
            });
            assert!(
                blocked_rx.recv().unwrap(),
                "transition gate must already be held at {boundary:?}"
            );
            release.wait();
            let (mut fixture, result) = sample.join().unwrap();
            assert!(result.is_ok(), "old effects linearize before transition");
            assert_eq!(advanced_rx.recv().unwrap(), 2);
            transition.join().unwrap();
            let persisted_before = fixture.repository.latest().unwrap();
            let health_before = fixture.health.list().unwrap();
            let diagnostics_before = fixture.diagnostics.list(10).unwrap();
            let events_before = fixture.emitter.events.lock().unwrap().clone();
            assert_eq!(
                fixture
                    .worker
                    .sample_once(Instant::now(), 101)
                    .unwrap_err()
                    .message_key,
                "errors.serviceStopping"
            );
            assert_eq!(fixture.repository.latest().unwrap(), persisted_before);
            assert_eq!(fixture.health.list().unwrap(), health_before);
            assert_eq!(fixture.diagnostics.list(10).unwrap(), diagnostics_before);
            assert_eq!(*fixture.emitter.events.lock().unwrap(), events_before);
        }
    }

    #[test]
    fn health_persist_failure_is_typed_emits_no_hint_and_the_next_cycle_recovers() {
        let generation = Arc::new(AtomicU64::new(1));
        let mut fixture = fixture(
            vec![Ok(capture(102)), Ok(capture(103))],
            generation,
            None,
            false,
        );
        fixture
            .storage
            .with_connection(|connection| {
                connection.execute_batch(
                    "ALTER TABLE service_health RENAME TO service_health_unavailable;",
                )?;
                Ok(())
            })
            .unwrap();
        let error = fixture.worker.sample_once(Instant::now(), 102).unwrap_err();
        assert_eq!(error.code, AppErrorCode::DatabaseFailure);
        assert!(fixture.emitter.events.lock().unwrap().is_empty());
        let diagnostic = fixture.diagnostics.list(1).unwrap().remove(0);
        assert_eq!(diagnostic.code, SAMPLE_FAILURE_DIAGNOSTIC);
        assert_eq!(
            diagnostic.parameters,
            SafeMessageParameters::from([
                ("metric".into(), SafeParameterValue::String("health".into()),),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String("databaseFailure".into()),
                ),
                (
                    "sampledAt".into(),
                    SafeParameterValue::Number(serde_json::Number::from(102)),
                ),
            ])
        );
        fixture
            .storage
            .with_connection(|connection| {
                connection.execute_batch(
                    "ALTER TABLE service_health_unavailable RENAME TO service_health;",
                )?;
                Ok(())
            })
            .unwrap();
        let recovered = fixture.worker.sample_once(Instant::now(), 103).unwrap();
        assert_eq!(recovered.sampled_at, 103);
        assert_eq!(
            fixture.emitter.events.lock().unwrap().as_slice(),
            &[(
                MONITOR_METRICS_CHANGED,
                serde_json::json!({ "sampledAt": 103 })
            )]
        );
    }

    #[test]
    fn a_metric_fault_does_not_kill_the_worker_and_the_next_valid_capture_commits() {
        let generation = Arc::new(AtomicU64::new(1));
        let mut fixture = fixture(
            vec![
                Err(MetricFault {
                    metric: "disk",
                    reason_code: "queryFailed",
                }),
                Ok(capture(101)),
            ],
            generation,
            None,
            false,
        );
        assert_eq!(
            fixture
                .worker
                .sample_once(Instant::now(), 100)
                .unwrap_err()
                .code,
            AppErrorCode::SourceUnavailable
        );
        let snapshot = fixture.worker.sample_once(Instant::now(), 101).unwrap();
        assert_eq!(fixture.repository.latest().unwrap(), Some(snapshot));
        assert_eq!(fixture.emitter.events.lock().unwrap().len(), 1);
    }

    #[test]
    fn threshold_failure_happens_after_sample_commit_and_does_not_suppress_the_metric_event() {
        struct PersistCheckingThresholds {
            repository: MonitorRepository,
            calls: Arc<std::sync::atomic::AtomicUsize>,
        }

        impl ThresholdEvaluationPort for PersistCheckingThresholds {
            fn evaluate(
                &self,
                snapshot: &MonitorSnapshot,
                _now: i64,
            ) -> Result<
                Vec<crate::services::threshold_evaluator::ThresholdEvaluationOutcome>,
                CommandError,
            > {
                assert_eq!(self.repository.latest().unwrap(), Some(snapshot.clone()));
                self.calls.fetch_add(1, Ordering::AcqRel);
                Err(CommandError {
                    code: AppErrorCode::DatabaseFailure,
                    message_key: "errors.databaseFailure".into(),
                    details: SafeMessageParameters::new(),
                    retryable: true,
                })
            }
        }

        let generation = Arc::new(AtomicU64::new(1));
        let mut fixture = fixture(vec![Ok(capture(700))], generation, None, false);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        fixture.worker.thresholds = Some(Arc::new(PersistCheckingThresholds {
            repository: fixture.repository.clone(),
            calls: calls.clone(),
        }));
        let error = fixture.worker.sample_once(Instant::now(), 700).unwrap_err();
        assert_eq!(error.code, AppErrorCode::DatabaseFailure);
        assert_eq!(calls.load(Ordering::Acquire), 1);
        assert_eq!(
            fixture.repository.latest().unwrap().unwrap().sampled_at,
            700
        );
        assert_eq!(
            fixture.emitter.events.lock().unwrap().as_slice(),
            &[(
                MONITOR_METRICS_CHANGED,
                serde_json::json!({ "sampledAt": 700 })
            )]
        );
    }

    #[tokio::test]
    async fn cancel_stops_the_two_second_skip_loop_and_drops_the_worker() {
        let generation = Arc::new(AtomicU64::new(1));
        let fixture = fixture(
            vec![Err(MetricFault {
                metric: "core",
                reason_code: "baselinePending",
            })],
            generation,
            None,
            false,
        );
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let join = tokio::spawn(fixture.worker.run(cancel_rx));
        cancel_tx.send_replace(true);
        tokio::time::timeout(Duration::from_secs(1), join)
            .await
            .unwrap()
            .unwrap();
    }
    #[tokio::test]
    async fn production_shaped_worker_lease_cancel_stops_and_joins_without_global_shutdown() {
        let generation = Arc::new(AtomicU64::new(1));
        let fixture = fixture(
            vec![Err(MetricFault {
                metric: "core",
                reason_code: "baselinePending",
            })],
            generation,
            None,
            false,
        );
        let (_global_tx, global_rx) = tokio::sync::watch::channel(false);
        let registry = crate::services::WorkerJoinRegistry::new();
        let mut lease = match registry.register(
            crate::services::module_runtime::registered_monitor_worker(fixture.worker, global_rx),
        ) {
            Ok(lease) => lease,
            Err(_) => panic!("test registry must accept the monitor worker"),
        };
        let cancelled =
            tokio::time::timeout(Duration::from_millis(100), lease.cancel_and_wait()).await;
        let batch = registry.stop_accepting_and_take();
        batch.cancel_all();
        batch.await_all().await.unwrap();
        assert!(
            cancelled.is_ok(),
            "lease cancellation must stop the exact worker"
        );
    }
}
