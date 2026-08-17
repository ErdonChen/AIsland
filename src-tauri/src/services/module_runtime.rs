use super::{RegisteredWorker, WorkerJoin, WorkerJoinRegistry, WorkerLease};
use crate::contracts::{CommandError, ModuleId, ModulePreference};
use crate::services::monitor_sampler::MonitorGenerationGate;
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

pub trait ModuleWorkerStarter: Send + Sync {
    fn start_clipboard(
        &self,
        generation: u64,
        current_generation: Arc<AtomicU64>,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<RegisteredWorker, CommandError>;
    fn start_monitor(
        &self,
        generation: u64,
        generation_gate: Arc<MonitorGenerationGate>,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<RegisteredWorker, CommandError>;
}

struct ModuleRegisteredWorker {
    module_id: ModuleId,
    worker: RegisteredWorker,
}

struct ModuleWorkerLease {
    module_id: ModuleId,
    lease: WorkerLease,
}

pub struct ModuleRuntimeCoordinator {
    started: Mutex<bool>,
    current_generation: Arc<AtomicU64>,
    monitor_generation_gate: Arc<MonitorGenerationGate>,
    worker_leases: Mutex<Vec<ModuleWorkerLease>>,
    restarting: std::sync::atomic::AtomicBool,
}

struct RestartPermit<'a> {
    restarting: &'a std::sync::atomic::AtomicBool,
}

impl Drop for RestartPermit<'_> {
    fn drop(&mut self) {
        self.restarting.store(false, Ordering::Release);
    }
}

impl ModuleRuntimeCoordinator {
    pub fn new() -> Self {
        let current_generation = Arc::new(AtomicU64::new(0));
        let monitor_generation = Arc::new(AtomicU64::new(0));
        Self {
            started: Mutex::new(false),
            current_generation,
            monitor_generation_gate: Arc::new(MonitorGenerationGate::new(monitor_generation)),
            worker_leases: Mutex::new(Vec::new()),
            restarting: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn start_once(
        &self,
        preferences: &BTreeMap<ModuleId, ModulePreference>,
        starter: &dyn ModuleWorkerStarter,
        registry: &WorkerJoinRegistry,
        cancel: tokio::sync::watch::Receiver<bool>,
        shutdown_started: bool,
    ) -> Result<(), CommandError> {
        let mut started = self
            .started
            .lock()
            .expect("module runtime start lock poisoned");
        if shutdown_started || self.restarting.load(Ordering::Acquire) {
            return Err(crate::services::service_stopping_error());
        }
        if *started {
            return Ok(());
        }

        let generation = self.current_generation.fetch_add(1, Ordering::AcqRel) + 1;
        let monitor_generation = self.monitor_generation_gate.advance();
        let workers =
            self.create_workers(preferences, starter, cancel, generation, monitor_generation);
        let leases = self.register_workers(workers, registry)?;
        *self
            .worker_leases
            .lock()
            .expect("module runtime lease lock poisoned") = leases;
        *started = true;
        Ok(())
    }

    pub async fn restart_monitor(
        &self,
        preferences: &BTreeMap<ModuleId, ModulePreference>,
        starter: &dyn ModuleWorkerStarter,
        registry: &WorkerJoinRegistry,
        cancel: tokio::sync::watch::Receiver<bool>,
        shutdown_started: bool,
    ) -> Result<(), CommandError> {
        if shutdown_started
            || self
                .restarting
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Err(crate::services::service_stopping_error());
        }
        let _permit = RestartPermit {
            restarting: &self.restarting,
        };
        self.restart_monitor_inner(preferences, starter, registry, cancel)
            .await
    }

    async fn restart_monitor_inner(
        &self,
        preferences: &BTreeMap<ModuleId, ModulePreference>,
        starter: &dyn ModuleWorkerStarter,
        registry: &WorkerJoinRegistry,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), CommandError> {
        if !*self
            .started
            .lock()
            .expect("module runtime start lock poisoned")
        {
            return Err(crate::services::service_stopping_error());
        }

        // The transition waits for any in-flight monitor side-effect chain. Once this returns,
        // the old monitor is stale before cancellation and no late side effect can start.
        let generation = self.monitor_generation_gate.advance();
        let old_monitor = {
            let leases = self
                .worker_leases
                .lock()
                .expect("module runtime lease lock poisoned");
            leases
                .iter()
                .find(|lease| lease.module_id == ModuleId::Monitor)
                .map(|lease| lease.lease.clone())
        };
        if let Some(lease) = old_monitor {
            let retirement = registry.retire(lease).await?;
            self.worker_leases
                .lock()
                .expect("module runtime lease lock poisoned")
                .retain(|lease| lease.module_id != ModuleId::Monitor);
            retirement.into_worker_result()?;
        }

        if module_requested(preferences, ModuleId::Monitor) {
            if let Ok(worker) =
                starter.start_monitor(generation, self.monitor_generation_gate.clone(), cancel)
            {
                let lease = match registry.register(worker) {
                    Ok(lease) => lease,
                    Err(rejected) => {
                        self.monitor_generation_gate.advance();
                        stop_unregistered(rejected);
                        return Err(crate::services::service_stopping_error());
                    }
                };
                self.worker_leases
                    .lock()
                    .expect("module runtime lease lock poisoned")
                    .push(ModuleWorkerLease {
                        module_id: ModuleId::Monitor,
                        lease,
                    });
            }
        }
        Ok(())
    }

    fn create_workers(
        &self,
        preferences: &BTreeMap<ModuleId, ModulePreference>,
        starter: &dyn ModuleWorkerStarter,
        cancel: tokio::sync::watch::Receiver<bool>,
        generation: u64,
        monitor_generation: u64,
    ) -> Vec<ModuleRegisteredWorker> {
        let mut workers = Vec::new();
        if module_requested(preferences, ModuleId::Clipboard) {
            if let Ok(worker) =
                starter.start_clipboard(generation, self.current_generation.clone(), cancel.clone())
            {
                workers.push(ModuleRegisteredWorker {
                    module_id: ModuleId::Clipboard,
                    worker,
                });
            }
        }
        if module_requested(preferences, ModuleId::Monitor) {
            if let Ok(worker) = starter.start_monitor(
                monitor_generation,
                self.monitor_generation_gate.clone(),
                cancel,
            ) {
                workers.push(ModuleRegisteredWorker {
                    module_id: ModuleId::Monitor,
                    worker,
                });
            }
        }

        workers
    }

    fn register_workers(
        &self,
        mut workers: Vec<ModuleRegisteredWorker>,
        registry: &WorkerJoinRegistry,
    ) -> Result<Vec<ModuleWorkerLease>, CommandError> {
        let mut leases = Vec::new();
        while let Some(module_worker) = workers.pop() {
            match registry.register(module_worker.worker) {
                Ok(lease) => leases.push(ModuleWorkerLease {
                    module_id: module_worker.module_id,
                    lease,
                }),
                Err(rejected) => {
                    self.current_generation.fetch_add(1, Ordering::AcqRel);
                    self.monitor_generation_gate.advance();
                    stop_unregistered(rejected);
                    for remaining in workers {
                        stop_unregistered(remaining.worker);
                    }
                    return Err(crate::services::service_stopping_error());
                }
            }
        }
        Ok(leases)
    }
}

fn module_requested(
    preferences: &BTreeMap<ModuleId, ModulePreference>,
    module_id: ModuleId,
) -> bool {
    preferences
        .get(&module_id)
        .is_some_and(|preference| preference.visible || preference.background_enabled)
}

fn stop_unregistered(worker: RegisteredWorker) {
    (worker.cancel)();
    match worker.join {
        WorkerJoin::Thread(join) => {
            let _ = join.join();
        }
        WorkerJoin::Async(join) => join.abort(),
    }
}

#[cfg(windows)]
pub struct WindowsModuleWorkerStarter {
    app: tauri::AppHandle,
    clipboard: Arc<super::clipboard_service::ClipboardService>,
    monitor: super::monitor_sampler::MonitorSamplerFactory,
    health: crate::repositories::service_health::ServiceHealthRepository,
}

#[cfg(windows)]
impl WindowsModuleWorkerStarter {
    pub fn new(
        app: tauri::AppHandle,
        clipboard: Arc<super::clipboard_service::ClipboardService>,
        monitor: super::monitor_sampler::MonitorSamplerFactory,
        health: crate::repositories::service_health::ServiceHealthRepository,
    ) -> Self {
        Self {
            app,
            clipboard,
            monitor,
            health,
        }
    }

    fn record_start_failure(&self, service_id: &str, checked_at: i64) {
        use crate::contracts::{SafeParameterValue, ServiceHealthSnapshot, ServiceHealthState};

        let _ = self.health.upsert(&ServiceHealthSnapshot {
            service_id: service_id.into(),
            state: ServiceHealthState::Degraded,
            message_key: "services.degraded".into(),
            parameters: BTreeMap::from([
                (
                    "serviceId".into(),
                    SafeParameterValue::String(service_id.into()),
                ),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String("workerStartFailed".into()),
                ),
            ]),
            checked_at,
        });
    }
}

#[cfg(windows)]
impl ModuleWorkerStarter for WindowsModuleWorkerStarter {
    fn start_clipboard(
        &self,
        generation: u64,
        current_generation: Arc<AtomicU64>,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<RegisteredWorker, CommandError> {
        let mut handle = match self.clipboard.start_worker(
            self.app.clone(),
            generation,
            current_generation,
            cancel,
        ) {
            Ok(handle) => handle,
            Err(error) => {
                self.record_start_failure("clipboard", now_millis());
                return Err(error);
            }
        };
        registered_native_worker("clipboardListener", move || handle.stop())
    }

    fn start_monitor(
        &self,
        generation: u64,
        generation_gate: Arc<MonitorGenerationGate>,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<RegisteredWorker, CommandError> {
        let current_generation = generation_gate.current_generation();
        let worker = match self.monitor.create_worker_with_gate(
            generation,
            current_generation,
            generation_gate,
        ) {
            Ok(worker) => worker,
            Err(error) => {
                self.record_start_failure("monitorCore", now_millis());
                return Err(error);
            }
        };
        Ok(registered_monitor_worker(worker, cancel))
    }
}

#[cfg(windows)]
pub(crate) fn registered_monitor_worker(
    worker: super::monitor_sampler::MonitorSamplerWorker,
    mut global_cancel: tokio::sync::watch::Receiver<bool>,
) -> RegisteredWorker {
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let cancel_tx = Arc::new(cancel_tx);
    let worker_cancel = cancel_tx.clone();
    let (completion_tx, completion_rx) = tokio::sync::watch::channel(None);
    let join = tauri::async_runtime::spawn(async move {
        let mut run = Box::pin(worker.run(cancel_rx));
        loop {
            tokio::select! {
                () = &mut run => break,
                changed = global_cancel.changed() => {
                    if changed.is_err() || *global_cancel.borrow() {
                        worker_cancel.send_replace(true);
                    }
                }
            }
        }
        let result = Ok(());
        completion_tx.send_replace(Some(result.clone()));
        result
    });
    RegisteredWorker {
        name: "monitorSampler",
        cancel: Arc::new(move || {
            cancel_tx.send_replace(true);
        }),
        join: WorkerJoin::Async(join),
        completion: completion_rx,
    }
}

#[cfg(windows)]
fn registered_native_worker(
    name: &'static str,
    stop: impl FnOnce() -> Result<(), CommandError> + Send + 'static,
) -> Result<RegisteredWorker, CommandError> {
    let (cancel_tx, cancel_rx) = std::sync::mpsc::channel();
    let (completion_tx, completion_rx) = tokio::sync::watch::channel(None);
    let join = std::thread::Builder::new()
        .name(format!("aisland-{name}-owner"))
        .spawn(move || {
            let _ = cancel_rx.recv();
            let result = stop();
            completion_tx.send_replace(Some(result.clone()));
            result
        })
        .map_err(|_| crate::services::worker_join_error(name))?;
    Ok(RegisteredWorker {
        name,
        cancel: Arc::new(move || {
            let _ = cancel_tx.send(());
        }),
        join: WorkerJoin::Thread(join),
        completion: completion_rx,
    })
}

#[cfg(windows)]
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::ModulePreference;
    use crate::repositories::diagnostics::DiagnosticsRepository;
    use crate::repositories::monitor::MonitorRepository;
    use crate::repositories::service_health::ServiceHealthRepository;
    use crate::services::monitor_sampler::MonitorSamplerWorker;
    use crate::services::system_metrics::{CoreMetricCapture, CoreMetricsSource, MetricFault};
    use crate::services::EventEmitterPort;
    use crate::services::{RegisteredWorker, WorkerJoin};
    use crate::storage::Storage;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct BaselineSource {
        captures: Arc<AtomicUsize>,
        drops: Arc<AtomicUsize>,
    }

    impl CoreMetricsSource for BaselineSource {
        fn capture(
            &mut self,
            _monotonic_now: std::time::Instant,
            _unix_now: i64,
        ) -> Result<CoreMetricCapture, MetricFault> {
            self.captures.fetch_add(1, Ordering::AcqRel);
            Err(MetricFault {
                metric: "core",
                reason_code: "baselinePending",
            })
        }
    }

    impl Drop for BaselineSource {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::AcqRel);
        }
    }

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

    struct RestartingSamplerStarter {
        repository: MonitorRepository,
        health: ServiceHealthRepository,
        diagnostics: DiagnosticsRepository,
        captures: Arc<AtomicUsize>,
        drops: Arc<AtomicUsize>,
        generations: Mutex<Vec<u64>>,
        registry: Arc<WorkerJoinRegistry>,
        registered_counts_at_start: Mutex<Vec<usize>>,
    }

    impl ModuleWorkerStarter for RestartingSamplerStarter {
        fn start_clipboard(
            &self,
            _generation: u64,
            _current_generation: Arc<AtomicU64>,
            _cancel: tokio::sync::watch::Receiver<bool>,
        ) -> Result<RegisteredWorker, CommandError> {
            Err(crate::services::service_stopping_error())
        }

        fn start_monitor(
            &self,
            generation: u64,
            generation_gate: Arc<MonitorGenerationGate>,
            cancel: tokio::sync::watch::Receiver<bool>,
        ) -> Result<RegisteredWorker, CommandError> {
            self.generations.lock().unwrap().push(generation);
            self.registered_counts_at_start
                .lock()
                .unwrap()
                .push(self.registry.registered_count());
            let worker = MonitorSamplerWorker::from_test_parts(
                Box::new(BaselineSource {
                    captures: self.captures.clone(),
                    drops: self.drops.clone(),
                }),
                generation,
                generation_gate,
                self.repository.clone(),
                self.health.clone(),
                self.diagnostics.clone(),
                Arc::new(NoopEmitter),
            );
            Ok(registered_monitor_worker(worker, cancel))
        }
    }

    struct FakeStarter {
        clipboard_starts: AtomicUsize,
        fail_clipboard: bool,
        monitor_starts: AtomicUsize,
        generations: Mutex<Vec<u64>>,
        stops: Arc<AtomicUsize>,
    }

    impl FakeStarter {
        fn worker(&self, name: &'static str) -> RegisteredWorker {
            let (cancel_tx, cancel_rx) = std::sync::mpsc::channel();
            let (completion_tx, completion_rx) = tokio::sync::watch::channel(None);
            let stops = self.stops.clone();
            let join = std::thread::spawn(move || {
                let _ = cancel_rx.recv();
                stops.fetch_add(1, Ordering::SeqCst);
                let result = Ok(());
                completion_tx.send_replace(Some(result.clone()));
                result
            });
            RegisteredWorker {
                name,
                cancel: Arc::new(move || {
                    let _ = cancel_tx.send(());
                }),
                join: WorkerJoin::Thread(join),
                completion: completion_rx,
            }
        }
    }

    impl ModuleWorkerStarter for FakeStarter {
        fn start_clipboard(
            &self,
            generation: u64,
            _current_generation: Arc<AtomicU64>,
            _cancel: tokio::sync::watch::Receiver<bool>,
        ) -> Result<RegisteredWorker, CommandError> {
            self.clipboard_starts.fetch_add(1, Ordering::SeqCst);
            self.generations.lock().unwrap().push(generation);
            if self.fail_clipboard {
                return Err(crate::services::service_stopping_error());
            }
            Ok(self.worker("clipboardListener"))
        }

        fn start_monitor(
            &self,
            generation: u64,
            _generation_gate: Arc<MonitorGenerationGate>,
            _cancel: tokio::sync::watch::Receiver<bool>,
        ) -> Result<RegisteredWorker, CommandError> {
            self.monitor_starts.fetch_add(1, Ordering::SeqCst);
            self.generations.lock().unwrap().push(generation);
            Ok(self.worker("monitorSampler"))
        }
    }

    struct RestartLifecycleStarter {
        starts: AtomicUsize,
        old_cancelled: Arc<AtomicBool>,
        old_return_release: tokio::sync::watch::Sender<bool>,
        old_result: Result<(), CommandError>,
        join_returns: Arc<AtomicUsize>,
        registry: Arc<WorkerJoinRegistry>,
        registered_counts_at_start: Mutex<Vec<usize>>,
    }

    impl RestartLifecycleStarter {
        fn worker(&self, blocks_after_cancel: bool) -> RegisteredWorker {
            let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
            let (completion_tx, completion_rx) = tokio::sync::watch::channel(None);
            let old_cancelled = self.old_cancelled.clone();
            let mut old_return_release = self.old_return_release.subscribe();
            let old_result = self.old_result.clone();
            let join_returns = self.join_returns.clone();
            let join = tauri::async_runtime::spawn(async move {
                while !*cancel_rx.borrow() {
                    cancel_rx.changed().await.unwrap();
                }
                if blocks_after_cancel {
                    old_cancelled.store(true, Ordering::Release);
                    while !*old_return_release.borrow() {
                        old_return_release.changed().await.unwrap();
                    }
                }
                join_returns.fetch_add(1, Ordering::AcqRel);
                let result = if blocks_after_cancel {
                    old_result
                } else {
                    Ok(())
                };
                completion_tx.send_replace(Some(result.clone()));
                result
            });
            RegisteredWorker {
                name: "monitorSampler",
                cancel: Arc::new(move || {
                    cancel_tx.send_replace(true);
                }),
                join: WorkerJoin::Async(join),
                completion: completion_rx,
            }
        }
    }

    impl ModuleWorkerStarter for RestartLifecycleStarter {
        fn start_clipboard(
            &self,
            _generation: u64,
            _current_generation: Arc<AtomicU64>,
            _cancel: tokio::sync::watch::Receiver<bool>,
        ) -> Result<RegisteredWorker, CommandError> {
            Err(crate::services::service_stopping_error())
        }

        fn start_monitor(
            &self,
            _generation: u64,
            _generation_gate: Arc<MonitorGenerationGate>,
            _cancel: tokio::sync::watch::Receiver<bool>,
        ) -> Result<RegisteredWorker, CommandError> {
            let start_index = self.starts.fetch_add(1, Ordering::AcqRel);
            self.registered_counts_at_start
                .lock()
                .unwrap()
                .push(self.registry.registered_count());
            Ok(self.worker(start_index == 0))
        }
    }

    fn preference(
        module_id: ModuleId,
        visible: bool,
        background_enabled: bool,
    ) -> ModulePreference {
        ModulePreference {
            module_id,
            visible,
            background_enabled,
            revision: 1,
            updated_at: 1,
        }
    }

    #[tokio::test]
    async fn starts_clipboard_once_and_ignores_retired_media_preference() {
        let coordinator = ModuleRuntimeCoordinator::new();
        let registry = WorkerJoinRegistry::new();
        let (_shutdown_tx, shutdown) = tokio::sync::watch::channel(false);
        let starter = FakeStarter {
            clipboard_starts: AtomicUsize::new(0),
            fail_clipboard: false,
            monitor_starts: AtomicUsize::new(0),
            generations: Mutex::new(Vec::new()),
            stops: Arc::new(AtomicUsize::new(0)),
        };
        let preferences = BTreeMap::from([
            (
                ModuleId::Clipboard,
                preference(ModuleId::Clipboard, true, true),
            ),
            (ModuleId::Media, preference(ModuleId::Media, true, true)),
        ]);

        coordinator
            .start_once(&preferences, &starter, &registry, shutdown.clone(), false)
            .unwrap();
        coordinator
            .start_once(&preferences, &starter, &registry, shutdown, false)
            .unwrap();

        assert_eq!(starter.clipboard_starts.load(Ordering::SeqCst), 1);
        assert_eq!(*starter.generations.lock().unwrap(), vec![1]);
        let batch = registry.stop_accepting_and_take();
        assert_eq!(batch.len(), 1);
        batch.cancel_all();
        batch.await_all().await.unwrap();
        assert_eq!(starter.stops.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn optional_source_failure_stays_local_and_does_not_block_the_other_worker() {
        let coordinator = ModuleRuntimeCoordinator::new();
        let registry = WorkerJoinRegistry::new();
        let (_shutdown_tx, shutdown) = tokio::sync::watch::channel(false);
        let starter = FakeStarter {
            clipboard_starts: AtomicUsize::new(0),
            fail_clipboard: true,
            monitor_starts: AtomicUsize::new(0),
            generations: Mutex::new(Vec::new()),
            stops: Arc::new(AtomicUsize::new(0)),
        };
        let preferences = BTreeMap::from([
            (
                ModuleId::Clipboard,
                preference(ModuleId::Clipboard, true, true),
            ),
            (ModuleId::Media, preference(ModuleId::Media, true, true)),
        ]);

        coordinator
            .start_once(&preferences, &starter, &registry, shutdown, false)
            .unwrap();
        assert_eq!(starter.clipboard_starts.load(Ordering::SeqCst), 1);
        let batch = registry.stop_accepting_and_take();
        assert_eq!(batch.len(), 0);
        batch.cancel_all();
        batch.await_all().await.unwrap();
        assert_eq!(starter.stops.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn starts_background_only_modules_and_skips_fully_disabled_modules() {
        let coordinator = ModuleRuntimeCoordinator::new();
        let registry = WorkerJoinRegistry::new();
        let (_shutdown_tx, shutdown) = tokio::sync::watch::channel(false);
        let starter = FakeStarter {
            clipboard_starts: AtomicUsize::new(0),
            fail_clipboard: false,
            monitor_starts: AtomicUsize::new(0),
            generations: Mutex::new(Vec::new()),
            stops: Arc::new(AtomicUsize::new(0)),
        };
        let preferences = BTreeMap::from([
            (
                ModuleId::Clipboard,
                preference(ModuleId::Clipboard, false, true),
            ),
            (ModuleId::Media, preference(ModuleId::Media, false, false)),
        ]);

        coordinator
            .start_once(&preferences, &starter, &registry, shutdown, false)
            .unwrap();

        assert_eq!(starter.clipboard_starts.load(Ordering::SeqCst), 1);
        let batch = registry.stop_accepting_and_take();
        assert_eq!(batch.len(), 1);
        batch.cancel_all();
        batch.await_all().await.unwrap();
        assert_eq!(starter.stops.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn monitor_uses_the_same_generation_registry_and_shutdown_join_path() {
        let coordinator = ModuleRuntimeCoordinator::new();
        let registry = WorkerJoinRegistry::new();
        let (_shutdown_tx, shutdown) = tokio::sync::watch::channel(false);
        let starter = FakeStarter {
            clipboard_starts: AtomicUsize::new(0),
            fail_clipboard: false,
            monitor_starts: AtomicUsize::new(0),
            generations: Mutex::new(Vec::new()),
            stops: Arc::new(AtomicUsize::new(0)),
        };
        let preferences = BTreeMap::from([(
            ModuleId::Monitor,
            preference(ModuleId::Monitor, false, true),
        )]);

        coordinator
            .start_once(&preferences, &starter, &registry, shutdown, false)
            .unwrap();
        assert_eq!(starter.monitor_starts.load(Ordering::SeqCst), 1);
        assert_eq!(*starter.generations.lock().unwrap(), vec![1]);
        let batch = registry.stop_accepting_and_take();
        assert_eq!(batch.len(), 1);
        batch.cancel_all();
        batch.await_all().await.unwrap();
        assert_eq!(starter.stops.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn monitor_restart_preserves_clipboard_and_keeps_registry_bounded() {
        let coordinator = ModuleRuntimeCoordinator::new();
        let registry = WorkerJoinRegistry::new();
        let (_shutdown_tx, shutdown) = tokio::sync::watch::channel(false);
        let starter = FakeStarter {
            clipboard_starts: AtomicUsize::new(0),
            fail_clipboard: false,
            monitor_starts: AtomicUsize::new(0),
            generations: Mutex::new(Vec::new()),
            stops: Arc::new(AtomicUsize::new(0)),
        };
        let preferences = BTreeMap::from([
            (
                ModuleId::Clipboard,
                preference(ModuleId::Clipboard, false, true),
            ),
            (
                ModuleId::Monitor,
                preference(ModuleId::Monitor, false, true),
            ),
        ]);

        coordinator
            .start_once(&preferences, &starter, &registry, shutdown.clone(), false)
            .unwrap();
        coordinator
            .restart_monitor(&preferences, &starter, &registry, shutdown.clone(), false)
            .await
            .unwrap();
        coordinator
            .restart_monitor(&preferences, &starter, &registry, shutdown, false)
            .await
            .unwrap();

        assert_eq!(starter.clipboard_starts.load(Ordering::SeqCst), 1);
        assert_eq!(starter.monitor_starts.load(Ordering::SeqCst), 3);
        assert_eq!(*starter.generations.lock().unwrap(), vec![1, 1, 2, 3]);
        assert_eq!(starter.stops.load(Ordering::SeqCst), 2);
        assert_eq!(registry.registered_count(), 2);
        let batch = registry.stop_accepting_and_take();
        assert_eq!(
            batch.len(),
            2,
            "clipboard plus only the current monitor remain"
        );
        batch.cancel_all();
        batch.await_all().await.unwrap();
        assert_eq!(starter.stops.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn cancelled_restart_before_old_completion_can_retry_without_losing_registry_ownership() {
        let coordinator = Arc::new(ModuleRuntimeCoordinator::new());
        let registry = Arc::new(WorkerJoinRegistry::new());
        let old_cancelled = Arc::new(AtomicBool::new(false));
        let (old_return_release, _) = tokio::sync::watch::channel(false);
        let join_returns = Arc::new(AtomicUsize::new(0));
        let starter = Arc::new(RestartLifecycleStarter {
            starts: AtomicUsize::new(0),
            old_cancelled: old_cancelled.clone(),
            old_return_release: old_return_release.clone(),
            old_result: Ok(()),
            join_returns: join_returns.clone(),
            registry: registry.clone(),
            registered_counts_at_start: Mutex::new(Vec::new()),
        });
        let preferences = BTreeMap::from([(
            ModuleId::Monitor,
            preference(ModuleId::Monitor, false, true),
        )]);
        let (_shutdown_tx, shutdown) = tokio::sync::watch::channel(false);

        coordinator
            .start_once(
                &preferences,
                starter.as_ref(),
                registry.as_ref(),
                shutdown.clone(),
                false,
            )
            .unwrap();
        let cancelled_restart = tauri::async_runtime::spawn({
            let coordinator = coordinator.clone();
            let registry = registry.clone();
            let starter = starter.clone();
            let preferences = preferences.clone();
            let shutdown = shutdown.clone();
            async move {
                coordinator
                    .restart_monitor(
                        &preferences,
                        starter.as_ref(),
                        registry.as_ref(),
                        shutdown,
                        false,
                    )
                    .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !old_cancelled.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("restart must reach the old worker's pre-completion barrier");
        cancelled_restart.abort();
        assert!(cancelled_restart.await.is_err());
        old_return_release.send_replace(true);

        let retry_result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            coordinator.restart_monitor(
                &preferences,
                starter.as_ref(),
                registry.as_ref(),
                shutdown,
                false,
            ),
        )
        .await
        .expect("a later restart must not remain wedged");
        let registered_counts = starter.registered_counts_at_start.lock().unwrap().clone();
        let starts = starter.starts.load(Ordering::Acquire);
        let batch = registry.stop_accepting_and_take();
        let shutdown_count = batch.len();
        batch.cancel_all();
        batch.await_all().await.unwrap();

        assert!(retry_result.is_ok(), "later restart must succeed");
        assert_eq!(starts, 2, "the retry must construct one fresh monitor");
        assert_eq!(
            registered_counts,
            vec![0, 0],
            "the old registration must be retired before its replacement is constructed"
        );
        assert_eq!(shutdown_count, 1, "only the fresh monitor remains");
        assert_eq!(
            join_returns.load(Ordering::Acquire),
            2,
            "old and fresh monitor join exactly once each"
        );
    }

    #[tokio::test]
    async fn terminal_worker_error_clears_retired_lease_before_retrying_fresh_monitor() {
        let coordinator = ModuleRuntimeCoordinator::new();
        let registry = Arc::new(WorkerJoinRegistry::new());
        let expected_error = crate::services::worker_join_error("monitorSampler");
        let (old_return_release, _) = tokio::sync::watch::channel(true);
        let join_returns = Arc::new(AtomicUsize::new(0));
        let starter = RestartLifecycleStarter {
            starts: AtomicUsize::new(0),
            old_cancelled: Arc::new(AtomicBool::new(false)),
            old_return_release,
            old_result: Err(expected_error.clone()),
            join_returns: join_returns.clone(),
            registry: registry.clone(),
            registered_counts_at_start: Mutex::new(Vec::new()),
        };
        let preferences = BTreeMap::from([(
            ModuleId::Monitor,
            preference(ModuleId::Monitor, false, true),
        )]);
        let (_shutdown_tx, shutdown) = tokio::sync::watch::channel(false);

        coordinator
            .start_once(
                &preferences,
                &starter,
                registry.as_ref(),
                shutdown.clone(),
                false,
            )
            .unwrap();
        let first_result = coordinator
            .restart_monitor(
                &preferences,
                &starter,
                registry.as_ref(),
                shutdown.clone(),
                false,
            )
            .await;
        let terminal_registry_count = registry.registered_count();
        let retry_result = coordinator
            .restart_monitor(&preferences, &starter, registry.as_ref(), shutdown, false)
            .await;
        let registered_counts = starter.registered_counts_at_start.lock().unwrap().clone();
        let starts = starter.starts.load(Ordering::Acquire);
        let batch = registry.stop_accepting_and_take();
        let shutdown_count = batch.len();
        batch.cancel_all();
        batch.await_all().await.unwrap();

        assert_eq!(first_result, Err(expected_error));
        assert_eq!(
            terminal_registry_count, 0,
            "the failed worker's terminal join must already be consumed"
        );
        assert!(
            retry_result.is_ok(),
            "a terminal worker error must not leave a stale lease"
        );
        assert_eq!(starts, 2, "the retry must construct one fresh monitor");
        assert_eq!(
            registered_counts,
            vec![0, 0],
            "the fresh monitor starts after terminal retirement ownership is complete"
        );
        assert_eq!(shutdown_count, 1, "only the fresh monitor remains");
        assert_eq!(
            join_returns.load(Ordering::Acquire),
            2,
            "failed old and fresh monitor join exactly once each"
        );
    }

    #[tokio::test]
    async fn repeated_restart_retires_each_old_monitor_before_starting_a_fresh_baseline() {
        let directory = tempfile::tempdir().unwrap().keep();
        let storage = Arc::new(Storage::open(&directory).unwrap());
        let captures = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let registry = Arc::new(WorkerJoinRegistry::new());
        let starter = RestartingSamplerStarter {
            repository: MonitorRepository::new(storage.clone()),
            health: ServiceHealthRepository::new(storage.clone()),
            diagnostics: DiagnosticsRepository::new(storage),
            captures: captures.clone(),
            drops: drops.clone(),
            generations: Mutex::new(Vec::new()),
            registry: registry.clone(),
            registered_counts_at_start: Mutex::new(Vec::new()),
        };
        let coordinator = ModuleRuntimeCoordinator::new();
        let (_shutdown_tx, shutdown) = tokio::sync::watch::channel(false);
        let preferences = BTreeMap::from([(
            ModuleId::Monitor,
            preference(ModuleId::Monitor, false, true),
        )]);

        coordinator
            .start_once(
                &preferences,
                &starter,
                registry.as_ref(),
                shutdown.clone(),
                false,
            )
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while captures.load(Ordering::Acquire) < 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        coordinator
            .restart_monitor(
                &preferences,
                &starter,
                registry.as_ref(),
                shutdown.clone(),
                false,
            )
            .await
            .unwrap();
        assert_eq!(drops.load(Ordering::Acquire), 1);
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while captures.load(Ordering::Acquire) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        coordinator
            .restart_monitor(&preferences, &starter, registry.as_ref(), shutdown, false)
            .await
            .unwrap();
        assert_eq!(drops.load(Ordering::Acquire), 2);
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while captures.load(Ordering::Acquire) < 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(*starter.generations.lock().unwrap(), vec![1, 2, 3]);
        assert_eq!(
            *starter.registered_counts_at_start.lock().unwrap(),
            vec![0, 0, 0],
            "each old monitor must be removed and joined before its replacement is constructed"
        );

        let batch = registry.stop_accepting_and_take();
        assert_eq!(batch.len(), 1, "only the current monitor may remain");
        batch.cancel_all();
        batch.await_all().await.unwrap();
        assert_eq!(drops.load(Ordering::Acquire), 3);
        assert_eq!(captures.load(Ordering::Acquire), 3);
    }
}
