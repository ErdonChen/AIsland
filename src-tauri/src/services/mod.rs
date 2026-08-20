pub mod agent_hook_assets;
pub mod agent_integration_discovery;
pub mod agent_integrations;
pub mod agent_profile_spool;
pub mod agent_profiles;
pub mod agent_status_watcher;
pub mod app_updates;
pub mod clipboard_assets;
pub mod clipboard_listener;
pub mod clipboard_service;
pub mod config_merge;
pub mod gpu_metrics;
#[cfg(test)]
mod media_service;
pub mod module_runtime;
pub mod monitor_sampler;
pub mod native_agent_activity;
pub mod native_profile_activity;
pub mod note_export_directory;
pub mod note_recording_assets;
pub mod notification_history;
pub mod process_metrics;
pub mod product_settings;
pub mod reminder_channels;
pub mod reminder_scheduler;
pub mod system_metrics;
pub mod threshold_evaluator;
#[cfg(test)]
mod todo_reminders;
pub mod wpn_reader;

use crate::contracts::{
    AgentsSnapshot, AppErrorCode, CommandError, ModuleId, ModulePreference, SafeMessageParameters,
    SafeParameterValue, ServiceHealthSnapshot, ServiceHealthState,
};
use crate::events::{
    note_changed_payload, reminder_navigation_requested_payload, service_health_changed_payload,
    FOUNDATION_STORAGE_SERVICE_ID, NOTE_CHANGED, REMINDER_DISPATCH_READY,
    REMINDER_NAVIGATION_REQUESTED, SERVICE_HEALTH_CHANGED,
};
use crate::repositories::{
    agents::AgentRepository, app_settings::AppSettingsRepository,
    diagnostics::DiagnosticsRepository, monitor::MonitorRepository,
    note_recordings::NoteRecordingRepository, notes::NoteRepository,
    notifications::NotificationRepository, reminders::ReminderRepository,
    service_health::ServiceHealthRepository,
};
use crate::storage::Storage;
use note_export_directory::{
    BootstrapMarkdownExportDirectoryProvider, MarkdownExportDirectoryProvider,
};
use std::collections::BTreeMap;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};

pub trait ModuleStateProvider: Send + Sync {
    fn snapshot(&self) -> Result<BTreeMap<ModuleId, ModulePreference>, CommandError>;
}

pub struct BootstrapModuleStateProvider;

impl ModuleStateProvider for BootstrapModuleStateProvider {
    fn snapshot(&self) -> Result<BTreeMap<ModuleId, ModulePreference>, CommandError> {
        Ok([
            ModuleId::Notes,
            ModuleId::Clipboard,
            ModuleId::Monitor,
            ModuleId::Notifications,
        ]
        .into_iter()
        .map(|module_id| {
            let background_enabled = module_id != ModuleId::Notes;
            (
                module_id.clone(),
                ModulePreference {
                    module_id,
                    visible: true,
                    background_enabled,
                    revision: 0,
                    updated_at: 0,
                },
            )
        })
        .collect())
    }
}

#[async_trait::async_trait]
pub trait ShutdownPort: Send + Sync {
    async fn stop_accepting_work(&self) -> Result<(), CommandError>;
    async fn stop_optional_modules(&self) -> Result<(), CommandError>;
    async fn cancel_core_workers(&self) -> Result<(), CommandError>;
}

pub trait WalCheckpointPort: Send + Sync {
    fn checkpoint_truncate(&self) -> Result<(), CommandError>;
}

pub trait EventEmitterPort: Send + Sync {
    fn emit(
        &self,
        event_name: &'static str,
        payload: serde_json::Value,
    ) -> Result<(), CommandError>;
}

#[derive(Clone, Debug)]
struct AgentIntegrationAssembly {
    windows_home: PathBuf,
    app_data_root: PathBuf,
    app_data_dir: PathBuf,
    wsl_home: String,
    wsl_status_dir: String,
    wsl_helper: String,
}

impl AgentIntegrationAssembly {
    fn production(app: &tauri::AppHandle) -> Result<Self, CommandError> {
        let installed = agent_hook_assets::install_hook_assets(app)?;
        let windows_home = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .ok_or_else(storage_error)?;
        let app_data_root = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .ok_or_else(storage_error)?;
        let app_data_dir = app.path().app_data_dir().map_err(|_| storage_error())?;
        Self::from_installed(windows_home, app_data_root, app_data_dir, &installed)
    }

    fn from_installed(
        windows_home: PathBuf,
        app_data_root: PathBuf,
        app_data_dir: PathBuf,
        installed: &agent_hook_assets::HookAssetPaths,
    ) -> Result<Self, CommandError> {
        let wsl_home = installed
            .paths
            .iter()
            .find_map(|entry| match &entry.destination {
                agent_hook_assets::HookAssetDestination::Wsl(destination) => destination
                    .rsplit_once("/.local/share/aisland/agent-hooks/")
                    .map(|(home, _)| home.to_owned()),
                agent_hook_assets::HookAssetDestination::Windows(_) => None,
            })
            .unwrap_or_else(|| "/aisland-wsl-unavailable".into());
        if !wsl_home.starts_with('/') || wsl_home.contains('\0') || wsl_home.contains('\n') {
            return Err(storage_error());
        }
        let wsl_helper = format!(
            "{}/.local/share/aisland/agent-hooks/aisland-config-wsl.sh",
            wsl_home.trim_end_matches('/')
        );
        let wsl_status_dir = match (&installed.wsl_status_dir, installed.wsl_available) {
            (Some(status_dir), true) => status_dir.clone(),
            (None, false) => "/aisland-wsl-unavailable/agent-status".into(),
            _ => return Err(storage_error()),
        };
        if !wsl_status_dir.starts_with('/')
            || wsl_status_dir.contains('\\')
            || wsl_status_dir.contains('\0')
            || wsl_status_dir.contains('\n')
            || wsl_status_dir.contains('\r')
        {
            return Err(storage_error());
        }
        Ok(Self {
            windows_home,
            app_data_root,
            app_data_dir,
            wsl_home,
            wsl_status_dir,
            wsl_helper,
        })
    }

    #[cfg(test)]
    fn isolated(app_storage: &Path) -> Self {
        let app_data_root = app_storage.parent().unwrap_or(app_storage).to_path_buf();
        Self {
            windows_home: app_storage.join("test-home"),
            app_data_root,
            app_data_dir: app_storage.to_path_buf(),
            wsl_home: "/home/aisland-test".into(),
            wsl_status_dir: "/mnt/c/aisland-test/agent-status".into(),
            wsl_helper: "/home/aisland-test/.local/share/aisland/agent-hooks/aisland-config-wsl.sh"
                .into(),
        }
    }

    #[cfg(test)]
    fn wsl_argv(&self, action: &str, path: &str) -> Vec<String> {
        vec![
            "--exec".into(),
            "sh".into(),
            self.wsl_helper.clone(),
            action.into(),
            path.into(),
        ]
    }
}

pub enum WorkerJoin {
    Async(tauri::async_runtime::JoinHandle<Result<(), CommandError>>),
    Thread(std::thread::JoinHandle<Result<(), CommandError>>),
}

pub struct RegisteredWorker {
    pub name: &'static str,
    pub cancel: Arc<dyn Fn() + Send + Sync>,
    pub join: WorkerJoin,
    pub completion: tokio::sync::watch::Receiver<Option<Result<(), CommandError>>>,
}

impl RegisteredWorker {
    pub async fn cancel_and_join(self) -> Result<(), CommandError> {
        (self.cancel)();
        await_worker_join(self.name, self.join).await
    }
}

#[derive(Clone)]
pub struct WorkerLease {
    registration: Arc<WorkerRegistration>,
    name: &'static str,
    cancel: Arc<dyn Fn() + Send + Sync>,
    completion: tokio::sync::watch::Receiver<Option<Result<(), CommandError>>>,
}

struct WorkerRegistration;

impl WorkerLease {
    pub async fn cancel_and_wait(&mut self) -> Result<(), CommandError> {
        (self.cancel)();
        loop {
            if let Some(result) = self.completion.borrow().clone() {
                return result;
            }
            self.completion
                .changed()
                .await
                .map_err(|_| worker_join_error(self.name))?;
        }
    }
}

pub struct WorkerJoinRegistry {
    state: Arc<Mutex<WorkerJoinRegistryState>>,
    take_count: AtomicUsize,
}

struct WorkerJoinRegistryState {
    accepting: bool,
    workers: Option<Vec<RegisteredWorkerEntry>>,
    retirements: Vec<WorkerRetirement>,
}

struct RegisteredWorkerEntry {
    registration: Arc<WorkerRegistration>,
    worker: RegisteredWorker,
}

struct WorkerRetirement {
    registration: Arc<WorkerRegistration>,
    name: &'static str,
    cancel: Arc<dyn Fn() + Send + Sync>,
    completion: tokio::sync::watch::Receiver<Option<Result<(), CommandError>>>,
    driver: tauri::async_runtime::JoinHandle<Result<(), CommandError>>,
}

enum RetirementFinish {
    Pending,
    Finished,
    Missing,
}

#[derive(Debug)]
pub(crate) struct WorkerRetirementOutcome {
    worker_result: Result<(), CommandError>,
}

impl WorkerRetirementOutcome {
    pub(crate) fn into_worker_result(self) -> Result<(), CommandError> {
        self.worker_result
    }
}

pub struct WorkerJoinBatch {
    workers: Vec<RegisteredWorker>,
    retirements: Vec<WorkerRetirement>,
}

struct RejectedWorkerCleanupRegistry {
    state: Mutex<RejectedWorkerCleanupState>,
}

struct RejectedWorkerCleanupState {
    finalized: bool,
    tickets: Vec<tokio::sync::oneshot::Receiver<Result<(), CommandError>>>,
}

struct RejectedWorkerCleanupBatch {
    tickets: Vec<tokio::sync::oneshot::Receiver<Result<(), CommandError>>>,
}

enum RejectedWorkerCleanupReservation {
    ShutdownOwned(tokio::sync::oneshot::Sender<Result<(), CommandError>>),
    Finalized,
}

impl WorkerJoinRegistry {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(WorkerJoinRegistryState {
                accepting: true,
                workers: Some(Vec::new()),
                retirements: Vec::new(),
            })),
            take_count: AtomicUsize::new(0),
        }
    }

    pub fn register(&self, worker: RegisteredWorker) -> Result<WorkerLease, RegisteredWorker> {
        let registration = Arc::new(WorkerRegistration);
        let lease = WorkerLease {
            registration: registration.clone(),
            name: worker.name,
            cancel: worker.cancel.clone(),
            completion: worker.completion.clone(),
        };
        let mut state = self.state.lock().expect("worker registry lock poisoned");
        if state.accepting {
            if let Some(workers) = state.workers.as_mut() {
                workers.push(RegisteredWorkerEntry {
                    registration,
                    worker,
                });
                return Ok(lease);
            }
        }
        Err(worker)
    }

    pub(crate) async fn retire(
        &self,
        lease: WorkerLease,
    ) -> Result<WorkerRetirementOutcome, CommandError> {
        let mut retirement_rx = self.begin_retirement(&lease)?;
        let completion_result = loop {
            if let Some(result) = retirement_rx.borrow().clone() {
                break result;
            }
            if retirement_rx.changed().await.is_err() {
                break Err(worker_join_error(lease.name));
            }
        };
        loop {
            match self.finish_retirement(&lease.registration) {
                RetirementFinish::Pending => tokio::task::yield_now().await,
                RetirementFinish::Finished => {
                    return Ok(WorkerRetirementOutcome {
                        worker_result: completion_result,
                    });
                }
                RetirementFinish::Missing => return Err(service_stopping_error()),
            }
        }
    }

    fn begin_retirement(
        &self,
        lease: &WorkerLease,
    ) -> Result<tokio::sync::watch::Receiver<Option<Result<(), CommandError>>>, CommandError> {
        let (retirement_rx, cancel) = {
            let mut state = self.state.lock().expect("worker registry lock poisoned");
            if let Some(retirement) = state
                .retirements
                .iter()
                .find(|retirement| Arc::ptr_eq(&retirement.registration, &lease.registration))
            {
                (retirement.completion.clone(), retirement.cancel.clone())
            } else {
                let worker = state.workers.as_mut().and_then(|workers| {
                    workers
                        .iter()
                        .position(|entry| Arc::ptr_eq(&entry.registration, &lease.registration))
                        .map(|index| workers.remove(index).worker)
                });
                let worker = worker.ok_or_else(service_stopping_error)?;
                let name = worker.name;
                let cancel = worker.cancel.clone();
                let (retirement_tx, retirement_rx) = tokio::sync::watch::channel(None);
                let driver = tauri::async_runtime::spawn(async move {
                    let result = await_worker_join(name, worker.join).await;
                    retirement_tx.send_replace(Some(result.clone()));
                    result
                });
                state.retirements.push(WorkerRetirement {
                    registration: lease.registration.clone(),
                    name,
                    cancel: cancel.clone(),
                    completion: retirement_rx.clone(),
                    driver,
                });
                (retirement_rx, cancel)
            }
        };

        // The exact worker and its join driver are registry-owned before cancellation and before
        // this method returns. An async caller can therefore be dropped only after the handoff.
        (cancel)();
        Ok(retirement_rx)
    }

    fn finish_retirement(&self, registration: &Arc<WorkerRegistration>) -> RetirementFinish {
        let mut state = self.state.lock().expect("worker registry lock poisoned");
        let Some(index) = state
            .retirements
            .iter()
            .position(|retirement| Arc::ptr_eq(&retirement.registration, registration))
        else {
            return RetirementFinish::Missing;
        };
        if !state.retirements[index].driver.inner().is_finished() {
            return RetirementFinish::Pending;
        }

        // A completed driver has already awaited the exact worker join. Dropping its finished
        // task handle here cannot detach running join work and keeps repeated retirement bounded.
        state.retirements.remove(index);
        RetirementFinish::Finished
    }

    pub fn stop_accepting_and_take(&self) -> WorkerJoinBatch {
        let mut state = self.state.lock().expect("worker registry lock poisoned");
        state.accepting = false;
        let retirements = std::mem::take(&mut state.retirements);
        match state.workers.take() {
            Some(workers) => {
                self.take_count.fetch_add(1, Ordering::AcqRel);
                WorkerJoinBatch {
                    workers: workers.into_iter().map(|entry| entry.worker).collect(),
                    retirements,
                }
            }
            None => WorkerJoinBatch {
                workers: Vec::new(),
                retirements,
            },
        }
    }

    #[cfg(test)]
    pub fn is_accepting(&self) -> bool {
        self.state
            .lock()
            .expect("worker registry lock poisoned")
            .accepting
    }

    #[cfg(test)]
    pub fn take_count(&self) -> usize {
        self.take_count.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub fn registered_count(&self) -> usize {
        self.state
            .lock()
            .expect("worker registry lock poisoned")
            .workers
            .as_ref()
            .map_or(0, Vec::len)
    }
}

impl WorkerJoinBatch {
    pub fn len(&self) -> usize {
        self.workers.len() + self.retirements.len()
    }

    pub fn is_empty(&self) -> bool {
        self.workers.is_empty() && self.retirements.is_empty()
    }

    pub fn cancel_all(&self) {
        for worker in &self.workers {
            (worker.cancel)();
        }
        for retirement in &self.retirements {
            (retirement.cancel)();
        }
    }

    pub async fn await_all(self) -> Result<(), CommandError> {
        let mut first_error = None;
        for worker in self.workers {
            if let Err(error) = await_worker_join(worker.name, worker.join).await {
                first_error.get_or_insert(error);
            }
        }
        for retirement in self.retirements {
            let result = retirement
                .driver
                .await
                .map_err(|_| worker_join_error(retirement.name))
                .and_then(|result| result);
            if let Err(error) = result {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl RejectedWorkerCleanupRegistry {
    fn new() -> Self {
        Self {
            state: Mutex::new(RejectedWorkerCleanupState {
                finalized: false,
                tickets: Vec::new(),
            }),
        }
    }

    fn reserve_cleanup(&self) -> RejectedWorkerCleanupReservation {
        let mut state = self
            .state
            .lock()
            .expect("rejected worker cleanup registry lock poisoned");
        if state.finalized {
            RejectedWorkerCleanupReservation::Finalized
        } else {
            let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
            state.tickets.push(completion_rx);
            RejectedWorkerCleanupReservation::ShutdownOwned(completion_tx)
        }
    }

    fn take_batch(&self) -> Option<RejectedWorkerCleanupBatch> {
        let mut state = self
            .state
            .lock()
            .expect("rejected worker cleanup registry lock poisoned");
        if state.tickets.is_empty() {
            None
        } else {
            Some(RejectedWorkerCleanupBatch {
                tickets: std::mem::take(&mut state.tickets),
            })
        }
    }

    fn take_batch_or_finalize(
        &self,
        finalize: impl FnOnce(),
    ) -> Option<RejectedWorkerCleanupBatch> {
        let mut state = self
            .state
            .lock()
            .expect("rejected worker cleanup registry lock poisoned");
        if state.tickets.is_empty() {
            state.finalized = true;
            finalize();
            None
        } else {
            Some(RejectedWorkerCleanupBatch {
                tickets: std::mem::take(&mut state.tickets),
            })
        }
    }
}

impl RejectedWorkerCleanupBatch {
    async fn await_all(self) -> Result<(), CommandError> {
        let mut first_error = None;
        for ticket in self.tickets {
            match ticket.await {
                Ok(Err(error)) => {
                    first_error.get_or_insert(error);
                }
                Err(_) => {
                    first_error.get_or_insert_with(|| worker_join_error("rejectedWorkerCleanup"));
                }
                Ok(Ok(())) => {}
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

pub struct AppServices {
    pub storage: Arc<Storage>,
    pub settings: AppSettingsRepository,
    pub agents: AgentRepository,
    pub agent_integrations: Arc<agent_integrations::AgentIntegrationService>,
    pub agent_integration_discovery:
        Arc<agent_integration_discovery::AgentIntegrationDiscoveryService>,
    pub agent_profiles: Arc<agent_profiles::AgentProfileService>,
    pub reminders: ReminderRepository,
    pub notes: NoteRepository,
    pub note_recordings: NoteRecordingRepository,
    pub note_recording_assets: note_recording_assets::NoteRecordingAssetStore,
    pub monitor: MonitorRepository,
    pub notifications: NotificationRepository,
    pub notification_history: Arc<notification_history::NotificationHistoryService>,
    pub clipboard: Arc<clipboard_service::ClipboardService>,
    module_runtime: module_runtime::ModuleRuntimeCoordinator,
    pub markdown_export_directory: Arc<dyn MarkdownExportDirectoryProvider>,
    pub monitor_thresholds: threshold_evaluator::MonitorThresholdService,
    pub reminder_service: Arc<reminder_scheduler::ReminderService>,
    threshold_evaluator: Arc<threshold_evaluator::ThresholdEvaluator>,
    pub reminder_channels: Arc<reminder_channels::ReminderChannelService>,
    toast_activation: Arc<reminder_channels::ToastActivationPort>,
    toast_registration: Arc<reminder_channels::ToastRegistrationState>,
    toast_activation_router: Mutex<Option<Arc<dyn reminder_channels::ToastActivationHandler>>>,
    #[cfg(windows)]
    cold_start_activation: Mutex<Option<reminder_channels::ColdStartActivationRegistration>>,
    pub health: ServiceHealthRepository,
    pub diagnostics: DiagnosticsRepository,
    pub modules: Arc<dyn ModuleStateProvider>,
    shutdown_port: Arc<dyn ShutdownPort>,
    checkpoint: Arc<dyn WalCheckpointPort>,
    emitter: Arc<dyn EventEmitterPort>,
    reminder_worker: Mutex<Option<reminder_scheduler::ReminderWorker>>,
    reminder_channel_worker: Mutex<Option<reminder_channels::ReminderChannelWorker>>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    worker_joins: WorkerJoinRegistry,
    rejected_worker_cleanups: RejectedWorkerCleanupRegistry,
    reminder_worker_started: Mutex<bool>,
    reminder_channel_worker_started: Mutex<bool>,
    notification_worker_started: Mutex<bool>,
    #[cfg(test)]
    reminder_worker_completion_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    reminder_channel_worker_completion_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    agent_watcher_started: Mutex<bool>,
    agent_watcher: Mutex<Option<Arc<agent_status_watcher::AgentStatusWatcher>>>,
    agent_profiles_restored: Mutex<bool>,
    shutdown_started: AtomicBool,
    shutdown_completion: tokio::sync::watch::Sender<Option<Result<(), CommandError>>>,
}

impl AppServices {
    pub fn new(app: &tauri::AppHandle) -> Result<Arc<Self>, CommandError> {
        let app_data_dir = app.path().app_data_dir().map_err(|_| storage_error())?;
        let integration_assembly = AgentIntegrationAssembly::production(app)?;
        let storage = Arc::new(Storage::open(&app_data_dir)?);
        let health = ServiceHealthRepository::new(storage.clone());
        persist_foundation_storage_health(&health, now_millis())?;
        let settings = AppSettingsRepository::new(storage.clone());
        crate::restore_native_ui_language(&settings)?;
        let reminders = ReminderRepository::new(storage.clone());
        let toast_activation = Arc::new(reminder_channels::ToastActivationPort::default());
        let toast_registration = Arc::new(reminder_channels::ToastRegistrationState::default());
        let (channels, channel_worker) = reminder_channels::ReminderChannelService::new(
            Arc::new(reminder_channels::RodioReminderChannel::default()),
            Arc::new(
                reminder_channels::WindowsToastReminderChannel::with_health_and_registration(
                    toast_activation.clone(),
                    Arc::new(reminder_channels::RepositoryNotificationHealthPort(
                        health.clone(),
                    )),
                    toast_registration.clone(),
                ),
            ),
            Arc::new(reminder_channels::AlertWindowReminderChannel::new(
                Arc::new(reminder_channels::TauriAlertWindowPort::new(app.clone())),
                reminders.clone(),
            )),
            reminders,
        );
        let router = Arc::new(reminder_channels::ToastActivationRouter::new(
            channels.clone(),
            Arc::new(TauriMainWindowPort { app: app.clone() }),
            Arc::new(TauriReminderNavigationEmitter { app: app.clone() }),
            Arc::new(now_millis),
        ));
        let installed = toast_activation.install_once(&router);
        debug_assert!(
            installed,
            "the application must install one Toast activation handler"
        );
        let clipboard_assets = clipboard_assets::ClipboardAssetStore::new(&app_data_dir)?;
        let note_recording_assets =
            note_recording_assets::NoteRecordingAssetStore::new(&app_data_dir)?;
        let services = Self::from_parts_internal(
            storage.clone(),
            Arc::new(BootstrapModuleStateProvider),
            Arc::new(NoopShutdownPort),
            Arc::new(StorageWalCheckpoint { storage }),
            Arc::new(TauriEventEmitter { app: app.clone() }),
            Arc::new(BootstrapMarkdownExportDirectoryProvider),
            health.clone(),
            clipboard_assets,
            note_recording_assets,
            Some((channels, channel_worker)),
            toast_activation.clone(),
            toast_registration.clone(),
            integration_assembly,
        );
        *services
            .toast_activation_router
            .lock()
            .expect("toast activation router lock poisoned") = Some(router);
        #[cfg(windows)]
        match reminder_channels::register_windows_cold_start_activation(toast_activation) {
            Ok(registration) => {
                toast_registration.mark_ready();
                *services
                    .cold_start_activation
                    .lock()
                    .expect("cold-start activation lock poisoned") = Some(registration);
            }
            Err(failure) => {
                let _ = health.upsert(&crate::contracts::ServiceHealthSnapshot {
                    service_id: "notifications".into(),
                    state: crate::contracts::ServiceHealthState::Degraded,
                    message_key: "services.degraded".into(),
                    parameters: crate::contracts::SafeMessageParameters::from([
                        (
                            "serviceId".into(),
                            crate::contracts::SafeParameterValue::String("notifications".into()),
                        ),
                        (
                            "reasonCode".into(),
                            crate::contracts::SafeParameterValue::String(failure.code.into()),
                        ),
                    ]),
                    checked_at: now_millis(),
                });
            }
        }
        Ok(services)
    }

    pub fn subscribe_shutdown(&self) -> tokio::sync::watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }

    pub(crate) fn toast_activation_port(&self) -> Arc<reminder_channels::ToastActivationPort> {
        self.toast_activation.clone()
    }

    pub fn agent_status_watcher(
        &self,
        status_dir: std::path::PathBuf,
    ) -> Arc<agent_status_watcher::AgentStatusWatcher> {
        Arc::new(agent_status_watcher::AgentStatusWatcher::new(
            self.agents.clone(),
            self.reminder_service.clone(),
            self.health.clone(),
            self.diagnostics.clone(),
            self.emitter.clone(),
            status_dir,
        ))
    }

    pub fn agents_snapshot(&self, generated_at: i64) -> Result<AgentsSnapshot, CommandError> {
        if let Some(watcher) = self
            .agent_watcher
            .lock()
            .expect("agent watcher lock poisoned")
            .clone()
        {
            watcher.snapshot(generated_at)
        } else {
            self.agent_status_watcher(std::path::PathBuf::new())
                .snapshot(generated_at)
        }
    }

    pub fn start_agent_status_watcher_once(
        self: &Arc<Self>,
        status_dir: std::path::PathBuf,
    ) -> Result<(), CommandError> {
        let mut started = self
            .agent_watcher_started
            .lock()
            .expect("agent watcher start lock poisoned");
        if self.shutdown_started.load(Ordering::Acquire) {
            return Err(service_stopping_error());
        }
        if *started {
            return Ok(());
        }
        let watcher = self.agent_status_watcher(status_dir);
        let worker_watcher = watcher.clone();
        let shutdown = self.subscribe_shutdown();
        let (completion_tx, completion_rx) = tokio::sync::watch::channel(None);
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let join = tauri::async_runtime::spawn(async move {
            if start_rx.await.is_err() {
                let result = Ok(());
                completion_tx.send_replace(Some(result.clone()));
                return result;
            }
            worker_watcher.run(shutdown).await;
            let result = Ok(());
            completion_tx.send_replace(Some(result.clone()));
            result
        });
        let worker = RegisteredWorker {
            name: "agentStatusWatcher",
            cancel: Arc::new(|| {}),
            join: WorkerJoin::Async(join),
            completion: completion_rx,
        };
        match self.worker_joins.register(worker) {
            Ok(_) => {
                *self
                    .agent_watcher
                    .lock()
                    .expect("agent watcher lock poisoned") = Some(watcher);
                *started = true;
                let _ = start_tx.send(());
                Ok(())
            }
            Err(worker) => {
                drop(start_tx);
                if let WorkerJoin::Async(join) = worker.join {
                    join.abort();
                }
                Err(service_stopping_error())
            }
        }
    }

    pub fn restore_agent_profiles_once(&self) -> Result<usize, CommandError> {
        let mut restored = self
            .agent_profiles_restored
            .lock()
            .expect("agent profile restore lock poisoned");
        if self.shutdown_started.load(Ordering::Acquire) {
            return Err(service_stopping_error());
        }
        if *restored {
            return Ok(0);
        }
        let started = self.agent_profiles.restore_installed_custom_profiles()?;
        *restored = true;
        Ok(started)
    }

    #[cfg(windows)]
    pub fn start_notification_history_worker_once(
        self: &Arc<Self>,
        app: tauri::AppHandle,
    ) -> Result<(), CommandError> {
        let local_app_data = app.path().local_data_dir().map_err(|_| storage_error())?;
        let notification_history = self.notification_history.clone();
        let reminders = self.reminders.clone();
        let health = self.health.clone();
        let diagnostics = self.diagnostics.clone();
        self.start_notification_history_worker_once_with(move |shutdown| async move {
            let worker = notification_history.start_worker(
                notification_history::NotificationWorkerDependencies {
                    wpn: wpn_reader::WpnReader::from_local_app_data(&local_app_data),
                    reminders,
                    health,
                    diagnostics,
                    app,
                },
                1,
                Arc::new(AtomicU64::new(0)),
            );
            worker.run(shutdown).await;
            Ok(())
        })
    }

    fn start_notification_history_worker_once_with<Factory, WorkerFuture>(
        self: &Arc<Self>,
        factory: Factory,
    ) -> Result<(), CommandError>
    where
        Factory: FnOnce(tokio::sync::watch::Receiver<bool>) -> WorkerFuture + Send + 'static,
        WorkerFuture: std::future::Future<Output = Result<(), CommandError>> + Send + 'static,
    {
        let mut started = self
            .notification_worker_started
            .lock()
            .expect("notification history worker start lock poisoned");
        if self.shutdown_started.load(Ordering::Acquire) {
            return Err(service_stopping_error());
        }
        if *started {
            return Ok(());
        }

        let shutdown = self.subscribe_shutdown();
        let (completion_tx, completion_rx) = tokio::sync::watch::channel(None);
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let join = tauri::async_runtime::spawn(async move {
            let result = if start_rx.await.is_ok() {
                factory(shutdown).await
            } else {
                Ok(())
            };
            completion_tx.send_replace(Some(result.clone()));
            result
        });
        let registered = RegisteredWorker {
            name: "notificationHistory",
            cancel: Arc::new(|| {}),
            join: WorkerJoin::Async(join),
            completion: completion_rx,
        };
        match self.worker_joins.register(registered) {
            Ok(_) => {
                *started = true;
                let _ = start_tx.send(());
                Ok(())
            }
            Err(worker) => {
                drop(start_tx);
                if let WorkerJoin::Async(join) = worker.join {
                    join.abort();
                }
                Err(service_stopping_error())
            }
        }
    }

    #[cfg(windows)]
    pub fn start_optional_modules_once(
        self: &Arc<Self>,
        app: tauri::AppHandle,
    ) -> Result<(), CommandError> {
        let preferences = self.modules.snapshot()?;
        let starter = module_runtime::WindowsModuleWorkerStarter::new(
            app.clone(),
            self.clipboard.clone(),
            monitor_sampler::MonitorSamplerFactory::new(
                self.monitor.clone(),
                self.health.clone(),
                self.diagnostics.clone(),
                self.threshold_evaluator.clone(),
                app,
            ),
            self.health.clone(),
        );
        self.module_runtime.start_once(
            &preferences,
            &starter,
            &self.worker_joins,
            self.subscribe_shutdown(),
            self.shutdown_started.load(Ordering::Acquire),
        )
    }

    #[cfg(windows)]
    pub async fn restart_monitor(
        self: &Arc<Self>,
        app: tauri::AppHandle,
    ) -> Result<(), CommandError> {
        let preferences = self.modules.snapshot()?;
        let starter = module_runtime::WindowsModuleWorkerStarter::new(
            app.clone(),
            self.clipboard.clone(),
            monitor_sampler::MonitorSamplerFactory::new(
                self.monitor.clone(),
                self.health.clone(),
                self.diagnostics.clone(),
                self.threshold_evaluator.clone(),
                app,
            ),
            self.health.clone(),
        );
        self.module_runtime
            .restart_monitor(
                &preferences,
                &starter,
                &self.worker_joins,
                self.subscribe_shutdown(),
                self.shutdown_started.load(Ordering::Acquire),
            )
            .await
    }

    pub fn start_reminder_worker_once(self: &Arc<Self>) -> Result<(), CommandError> {
        let mut started = self
            .reminder_worker_started
            .lock()
            .expect("reminder worker start lock poisoned");
        if self.shutdown_started.load(Ordering::Acquire) {
            return Err(service_stopping_error());
        }
        if *started {
            return Ok(());
        }
        self.monitor_thresholds
            .reconcile_pending_cancellations(now_millis())?;
        let worker = self
            .reminder_worker
            .lock()
            .expect("reminder worker lock poisoned")
            .take()
            .ok_or_else(service_stopping_error)?;
        let shutdown = self.subscribe_shutdown();
        let (completion_tx, completion_rx) = tokio::sync::watch::channel(None);
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        #[cfg(test)]
        let completion_hook = self
            .reminder_worker_completion_hook
            .lock()
            .expect("reminder completion hook lock poisoned")
            .clone();
        let join = tauri::async_runtime::spawn(async move {
            if start_rx.await.is_ok() {
                worker.run(shutdown).await;
            }
            #[cfg(test)]
            if let Some(hook) = completion_hook {
                hook();
            }
            let result = Ok(());
            completion_tx.send_replace(Some(result.clone()));
            result
        });
        let registered = RegisteredWorker {
            name: "reminderScheduler",
            cancel: Arc::new(|| {}),
            join: WorkerJoin::Async(join),
            completion: completion_rx,
        };
        match self.worker_joins.register(registered) {
            Ok(_) => {
                *started = true;
                let _ = start_tx.send(());
                Ok(())
            }
            Err(registered) => {
                drop(start_tx);
                if let WorkerJoin::Async(join) = registered.join {
                    join.abort();
                }
                Err(service_stopping_error())
            }
        }
    }

    pub fn start_reminder_channel_worker_once(self: &Arc<Self>) -> Result<(), CommandError> {
        let mut started = self
            .reminder_channel_worker_started
            .lock()
            .expect("reminder channel worker start lock poisoned");
        if self.shutdown_started.load(Ordering::Acquire) {
            return Err(service_stopping_error());
        }
        if *started {
            return Ok(());
        }
        let worker = self
            .reminder_channel_worker
            .lock()
            .expect("reminder channel worker lock poisoned")
            .take()
            .ok_or_else(service_stopping_error)?;
        let shutdown = self.subscribe_shutdown();
        let (completion_tx, completion_rx) = tokio::sync::watch::channel(None);
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        #[cfg(test)]
        let completion_hook = self
            .reminder_channel_worker_completion_hook
            .lock()
            .expect("reminder channel completion hook lock poisoned")
            .clone();
        let join = tauri::async_runtime::spawn(async move {
            if start_rx.await.is_ok() {
                worker.run(shutdown).await;
            }
            #[cfg(test)]
            if let Some(hook) = completion_hook {
                hook();
            }
            let result = Ok(());
            completion_tx.send_replace(Some(result.clone()));
            result
        });
        let registered = RegisteredWorker {
            name: "reminderChannels",
            cancel: Arc::new(|| {}),
            join: WorkerJoin::Async(join),
            completion: completion_rx,
        };
        match self.worker_joins.register(registered) {
            Ok(_) => {
                *started = true;
                let _ = start_tx.send(());
                Ok(())
            }
            Err(registered) => {
                drop(start_tx);
                if let WorkerJoin::Async(join) = registered.join {
                    join.abort();
                }
                Err(service_stopping_error())
            }
        }
    }

    pub(crate) fn take_reminder_worker(&self) -> Option<reminder_scheduler::ReminderWorker> {
        self.reminder_worker
            .lock()
            .expect("reminder worker lock poisoned")
            .take()
    }

    #[cfg(test)]
    fn set_reminder_worker_completion_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        *self
            .reminder_worker_completion_hook
            .lock()
            .expect("reminder completion hook lock poisoned") = Some(hook);
    }

    #[cfg(test)]
    fn set_reminder_channel_worker_completion_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        *self
            .reminder_channel_worker_completion_hook
            .lock()
            .expect("reminder channel completion hook lock poisoned") = Some(hook);
    }

    pub(crate) fn emit_service_health_changed(
        &self,
        service_id: &str,
        checked_at: i64,
    ) -> Result<(), CommandError> {
        self.emitter.emit(
            SERVICE_HEALTH_CHANGED,
            service_health_changed_payload(service_id, checked_at),
        )
    }

    pub(crate) fn emit_note_changed(
        &self,
        entity_id: &str,
        revision: u64,
        changed_at: i64,
    ) -> Result<(), CommandError> {
        self.emitter.emit(
            NOTE_CHANGED,
            note_changed_payload(entity_id, revision, changed_at),
        )
    }

    pub fn register_worker(
        &self,
        worker: RegisteredWorker,
    ) -> impl std::future::Future<Output = Result<WorkerLease, CommandError>> + Send + 'static {
        let registration = match self.worker_joins.register(worker) {
            Ok(lease) => Ok(lease),
            Err(worker) => match self.rejected_worker_cleanups.reserve_cleanup() {
                RejectedWorkerCleanupReservation::ShutdownOwned(cleanup_result_tx) => {
                    let (completion_tx, completion_rx) = tokio::sync::watch::channel(None);
                    let cleanup_driver = tauri::async_runtime::spawn(async move {
                        let result = worker.cancel_and_join().await;
                        cleanup_result_tx.send(result.clone()).ok();
                        completion_tx.send_replace(Some(result.clone()));
                    });
                    drop(cleanup_driver);
                    Err(completion_rx)
                }
                RejectedWorkerCleanupReservation::Finalized => {
                    Err(cleanup_rejected_worker_after_finalize(worker))
                }
            },
        };
        async move {
            match registration {
                Ok(lease) => Ok(lease),
                Err(mut completion_rx) => {
                    loop {
                        if completion_rx.borrow().is_some() {
                            break;
                        }
                        if completion_rx.changed().await.is_err() {
                            break;
                        }
                    }
                    Err(service_stopping_error())
                }
            }
        }
    }

    pub async fn shutdown(&self) -> Result<(), CommandError> {
        let mut completion = self.shutdown_completion.subscribe();
        if self
            .shutdown_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            loop {
                if let Some(result) = completion.borrow().clone() {
                    return result;
                }
                completion
                    .changed()
                    .await
                    .map_err(|_| service_stopping_error())?;
            }
        }

        // Close admission synchronously with the winning CAS.  Native activation/COM teardown
        // may block, but no late worker may enter the batch while that teardown is in progress.
        self.agent_profiles.stop_accepting();
        let worker_batch = self.worker_joins.stop_accepting_and_take();
        self.toast_registration.mark_unavailable();
        self.shutdown_tx.send_replace(true);
        self.agent_profiles.shutdown();
        let mut first_result = Ok(());
        record_first(
            &mut first_result,
            self.shutdown_port.stop_accepting_work().await,
        );
        record_first(
            &mut first_result,
            self.shutdown_port.stop_optional_modules().await,
        );
        record_first(
            &mut first_result,
            self.shutdown_port.cancel_core_workers().await,
        );
        worker_batch.cancel_all();
        record_first(&mut first_result, worker_batch.await_all().await);
        // Keep activation routing alive until every registered channel worker has observed the
        // shutdown broadcast and exited; registration state already rejects any new Toast show.
        self.toast_activation.uninstall();
        self.toast_activation_router
            .lock()
            .expect("toast activation router lock poisoned")
            .take();
        #[cfg(windows)]
        self.cold_start_activation
            .lock()
            .expect("cold-start activation lock poisoned")
            .take();
        loop {
            match self.rejected_worker_cleanups.take_batch_or_finalize(|| {
                record_first(&mut first_result, self.checkpoint.checkpoint_truncate());
                self.shutdown_completion
                    .send_replace(Some(first_result.clone()));
            }) {
                Some(cleanup_batch) => {
                    record_first(&mut first_result, cleanup_batch.await_all().await);
                }
                None => break,
            }
        }
        first_result
    }

    #[cfg(test)]
    pub fn worker_take_count(&self) -> usize {
        self.worker_joins.take_count()
    }

    #[cfg(test)]
    pub fn accepts_workers(&self) -> bool {
        self.worker_joins.is_accepting()
    }

    #[cfg(test)]
    pub fn from_parts(
        storage: Arc<Storage>,
        modules: Arc<dyn ModuleStateProvider>,
        shutdown_port: Arc<dyn ShutdownPort>,
        checkpoint: Arc<dyn WalCheckpointPort>,
        emitter: Arc<dyn EventEmitterPort>,
    ) -> Arc<Self> {
        let app_storage = storage
            .path()
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        Self::from_parts_internal(
            storage.clone(),
            modules,
            shutdown_port,
            checkpoint,
            emitter,
            Arc::new(BootstrapMarkdownExportDirectoryProvider),
            ServiceHealthRepository::new(storage),
            clipboard_assets::ClipboardAssetStore::new(&app_storage)
                .expect("test storage must accept its clipboard asset directory"),
            note_recording_assets::NoteRecordingAssetStore::new(&app_storage)
                .expect("test storage must accept its note recording directory"),
            None,
            Arc::new(reminder_channels::ToastActivationPort::default()),
            Arc::new(reminder_channels::ToastRegistrationState::default()),
            AgentIntegrationAssembly::isolated(&app_storage),
        )
    }

    #[cfg(test)]
    pub fn from_parts_with_export_directory(
        storage: Arc<Storage>,
        modules: Arc<dyn ModuleStateProvider>,
        shutdown_port: Arc<dyn ShutdownPort>,
        checkpoint: Arc<dyn WalCheckpointPort>,
        emitter: Arc<dyn EventEmitterPort>,
        markdown_export_directory: Arc<dyn MarkdownExportDirectoryProvider>,
    ) -> Arc<Self> {
        let app_storage = storage
            .path()
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        Self::from_parts_internal(
            storage.clone(),
            modules,
            shutdown_port,
            checkpoint,
            emitter,
            markdown_export_directory,
            ServiceHealthRepository::new(storage),
            clipboard_assets::ClipboardAssetStore::new(&app_storage)
                .expect("test storage must accept its clipboard asset directory"),
            note_recording_assets::NoteRecordingAssetStore::new(&app_storage)
                .expect("test storage must accept its note recording directory"),
            None,
            Arc::new(reminder_channels::ToastActivationPort::default()),
            Arc::new(reminder_channels::ToastRegistrationState::default()),
            AgentIntegrationAssembly::isolated(&app_storage),
        )
    }

    fn from_parts_internal(
        storage: Arc<Storage>,
        modules: Arc<dyn ModuleStateProvider>,
        shutdown_port: Arc<dyn ShutdownPort>,
        checkpoint: Arc<dyn WalCheckpointPort>,
        emitter: Arc<dyn EventEmitterPort>,
        markdown_export_directory: Arc<dyn MarkdownExportDirectoryProvider>,
        health: ServiceHealthRepository,
        clipboard_assets: clipboard_assets::ClipboardAssetStore,
        note_recording_assets: note_recording_assets::NoteRecordingAssetStore,
        channel_bundle: Option<(
            Arc<reminder_channels::ReminderChannelService>,
            reminder_channels::ReminderChannelWorker,
        )>,
        toast_activation: Arc<reminder_channels::ToastActivationPort>,
        toast_registration: Arc<reminder_channels::ToastRegistrationState>,
        integration_assembly: AgentIntegrationAssembly,
    ) -> Arc<Self> {
        let (shutdown_tx, _) = tokio::sync::watch::channel(false);
        let (shutdown_completion, _) = tokio::sync::watch::channel(None);
        let reminders = ReminderRepository::new(storage.clone());
        let notes = NoteRepository::new(storage.clone());
        let note_recordings = NoteRecordingRepository::new(storage.clone());
        let monitor = MonitorRepository::new(storage.clone());
        let notifications = NotificationRepository::new(storage.clone());
        let notification_history =
            notification_history::NotificationHistoryService::new(notifications.clone());
        let agents = AgentRepository::new(storage.clone());
        let diagnostics = DiagnosticsRepository::new(storage.clone());
        let clipboard_repository = crate::repositories::clipboard::ClipboardRepository::new(
            storage.clone(),
            Arc::new(crate::domain::clipboard::BootstrapClipboardRetentionPolicy),
        );
        let clipboard = Arc::new(clipboard_service::ClipboardService::new(
            clipboard_repository,
            clipboard_assets,
            Arc::new(clipboard_listener::ArboardClipboardSourceFactory),
            health.clone(),
            diagnostics.clone(),
            emitter.clone(),
        ));
        let agent_integrations = Arc::new(agent_integrations::AgentIntegrationService::new(
            agents.clone(),
            diagnostics.clone(),
            &integration_assembly.windows_home,
            &integration_assembly.app_data_dir,
            &integration_assembly.wsl_home,
            &integration_assembly.wsl_status_dir,
            integration_assembly.wsl_helper,
        ));
        let agent_integration_discovery = Arc::new(
            agent_integration_discovery::AgentIntegrationDiscoveryService::new(
                integration_assembly.windows_home.clone(),
                integration_assembly.app_data_root.clone(),
                integration_assembly.windows_home.join("AppData/Local"),
                Arc::new(agent_integration_discovery::SystemAgentIntegrationDiscoveryProbe),
            ),
        );
        let agent_profiles = Arc::new(agent_profiles::AgentProfileService::new(
            crate::repositories::agent_profiles::AgentProfileRepository::new(storage.clone()),
            integration_assembly.windows_home.clone(),
            integration_assembly.app_data_dir.clone(),
            emitter.clone(),
        ));
        let (reminder_channels, reminder_channel_worker) = channel_bundle.unwrap_or_else(|| {
            reminder_channels::ReminderChannelService::new(
                Arc::new(reminder_channels::UnavailableReminderChannel(
                    reminder_channels::ReminderChannelName::Sound,
                )),
                Arc::new(reminder_channels::UnavailableReminderChannel(
                    reminder_channels::ReminderChannelName::Toast,
                )),
                Arc::new(reminder_channels::UnavailableReminderChannel(
                    reminder_channels::ReminderChannelName::Window,
                )),
                reminders.clone(),
            )
        });
        let (reminder_service, reminder_worker) = reminder_scheduler::ReminderService::new(
            reminders.clone(),
            Arc::new(reminder_scheduler::SystemReminderClock),
            Arc::new(ReminderDispatchEmitter {
                inner: emitter.clone(),
                channels: reminder_channels.clone(),
            }),
        );
        let (threshold_evaluator, monitor_thresholds) =
            threshold_evaluator::MonitorThresholdService::compose(
                monitor.clone(),
                reminder_service.clone(),
                diagnostics.clone(),
            );
        Arc::new(Self {
            settings: AppSettingsRepository::new(storage.clone()),
            agents,
            agent_integrations,
            agent_integration_discovery,
            agent_profiles,
            reminders,
            notes,
            note_recordings,
            note_recording_assets,
            monitor,
            notifications,
            notification_history,
            clipboard,
            module_runtime: module_runtime::ModuleRuntimeCoordinator::new(),
            markdown_export_directory,
            monitor_thresholds,
            reminder_service,
            threshold_evaluator,
            reminder_channels,
            toast_activation,
            toast_registration,
            toast_activation_router: Mutex::new(None),
            #[cfg(windows)]
            cold_start_activation: Mutex::new(None),
            diagnostics,
            storage,
            health,
            modules,
            shutdown_port,
            checkpoint,
            emitter,
            reminder_worker: Mutex::new(Some(reminder_worker)),
            reminder_channel_worker: Mutex::new(Some(reminder_channel_worker)),
            shutdown_tx,
            worker_joins: WorkerJoinRegistry::new(),
            rejected_worker_cleanups: RejectedWorkerCleanupRegistry::new(),
            reminder_worker_started: Mutex::new(false),
            reminder_channel_worker_started: Mutex::new(false),
            notification_worker_started: Mutex::new(false),
            #[cfg(test)]
            reminder_worker_completion_hook: Mutex::new(None),
            #[cfg(test)]
            reminder_channel_worker_completion_hook: Mutex::new(None),
            agent_watcher_started: Mutex::new(false),
            agent_watcher: Mutex::new(None),
            agent_profiles_restored: Mutex::new(false),
            shutdown_started: AtomicBool::new(false),
            shutdown_completion,
        })
    }
}

pub(crate) fn persist_foundation_storage_health(
    health: &ServiceHealthRepository,
    checked_at: i64,
) -> Result<(), CommandError> {
    health.upsert(&ServiceHealthSnapshot {
        service_id: FOUNDATION_STORAGE_SERVICE_ID.into(),
        state: ServiceHealthState::Healthy,
        message_key: "services.healthy".into(),
        parameters: BTreeMap::from([(
            "serviceId".into(),
            SafeParameterValue::String(FOUNDATION_STORAGE_SERVICE_ID.into()),
        )]),
        checked_at,
    })
}

struct NoopShutdownPort;

#[async_trait::async_trait]
impl ShutdownPort for NoopShutdownPort {
    async fn stop_accepting_work(&self) -> Result<(), CommandError> {
        Ok(())
    }

    async fn stop_optional_modules(&self) -> Result<(), CommandError> {
        Ok(())
    }

    async fn cancel_core_workers(&self) -> Result<(), CommandError> {
        Ok(())
    }
}

struct StorageWalCheckpoint {
    storage: Arc<Storage>,
}

impl WalCheckpointPort for StorageWalCheckpoint {
    fn checkpoint_truncate(&self) -> Result<(), CommandError> {
        self.storage.with_connection(|connection| {
            connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .map_err(Into::into)
        })
    }
}

struct TauriEventEmitter {
    app: tauri::AppHandle,
}

struct TauriMainWindowPort {
    app: tauri::AppHandle,
}

impl reminder_channels::MainWindowPort for TauriMainWindowPort {
    fn show_main(&self) -> Result<(), reminder_channels::ChannelFailure> {
        let window =
            self.app
                .get_webview_window("main")
                .ok_or(reminder_channels::ChannelFailure {
                    code: "mainWindowUnavailable",
                })?;
        window
            .unminimize()
            .map_err(|_| reminder_channels::ChannelFailure {
                code: "mainWindowShowFailed",
            })?;
        window
            .show()
            .map_err(|_| reminder_channels::ChannelFailure {
                code: "mainWindowShowFailed",
            })?;
        window
            .set_focus()
            .map_err(|_| reminder_channels::ChannelFailure {
                code: "mainWindowFocusFailed",
            })
    }
}

struct TauriReminderNavigationEmitter {
    app: tauri::AppHandle,
}

impl reminder_channels::ReminderNavigationEmitter for TauriReminderNavigationEmitter {
    fn emit_navigation(
        &self,
        navigation: &crate::contracts::PendingReminderNavigation,
    ) -> Result<(), reminder_channels::ChannelFailure> {
        self.app
            .emit_to(
                "main",
                REMINDER_NAVIGATION_REQUESTED,
                reminder_navigation_requested_payload(navigation.sequence),
            )
            .map_err(|_| reminder_channels::ChannelFailure {
                code: "navigationEmitFailed",
            })
    }
}

/// The scheduler's durable claim happens before this fan-out.  Tauri emission is retained for
/// the UI while the channel queue is only a wake hint; restart recovery reads SQLite again.
struct ReminderDispatchEmitter {
    inner: Arc<dyn EventEmitterPort>,
    channels: Arc<reminder_channels::ReminderChannelService>,
}

impl EventEmitterPort for ReminderDispatchEmitter {
    fn emit(
        &self,
        event_name: &'static str,
        payload: serde_json::Value,
    ) -> Result<(), CommandError> {
        if event_name == REMINDER_DISPATCH_READY {
            let delivery_id = payload.get("deliveryId").and_then(|value| value.as_str());
            let dispatch_seq = payload.get("dispatchSeq").and_then(|value| value.as_i64());
            if let (Some(delivery_id), Some(dispatch_seq)) = (delivery_id, dispatch_seq) {
                self.channels.wake(delivery_id, dispatch_seq);
            }
        }
        self.inner.emit(event_name, payload)
    }
}

impl EventEmitterPort for TauriEventEmitter {
    fn emit(
        &self,
        event_name: &'static str,
        payload: serde_json::Value,
    ) -> Result<(), CommandError> {
        self.app
            .emit(event_name, payload)
            .map_err(|_| service_stopping_error())
    }
}

async fn await_worker_join(name: &'static str, join: WorkerJoin) -> Result<(), CommandError> {
    match join {
        WorkerJoin::Async(join) => join.await.map_err(|_| worker_join_error(name))?,
        WorkerJoin::Thread(join) => tauri::async_runtime::spawn_blocking(move || {
            join.join().map_err(|_| worker_join_error(name))?
        })
        .await
        .map_err(|_| worker_join_error(name))?,
    }
}

fn cleanup_rejected_worker_after_finalize(
    worker: RegisteredWorker,
) -> tokio::sync::watch::Receiver<Option<Result<(), CommandError>>> {
    let (completion_tx, completion_rx) = tokio::sync::watch::channel(None);
    let cleanup_owner = std::thread::spawn(move || {
        let result = tauri::async_runtime::block_on(worker.cancel_and_join());
        completion_tx.send_replace(Some(result));
    });
    drop(cleanup_owner);
    completion_rx
}

fn record_first(first_result: &mut Result<(), CommandError>, result: Result<(), CommandError>) {
    if first_result.is_ok() {
        if let Err(error) = result {
            *first_result = Err(error);
        }
    }
}

fn service_stopping_error() -> CommandError {
    CommandError {
        code: AppErrorCode::SourceUnavailable,
        message_key: "errors.serviceStopping".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

fn worker_join_error(name: &'static str) -> CommandError {
    CommandError {
        code: AppErrorCode::SourceUnavailable,
        message_key: "errors.sourceUnavailable".into(),
        details: BTreeMap::from([
            ("serviceId".into(), SafeParameterValue::String(name.into())),
            (
                "reasonCode".into(),
                SafeParameterValue::String("joinFailed".into()),
            ),
        ]),
        retryable: false,
    }
}

fn storage_error() -> CommandError {
    CommandError {
        code: AppErrorCode::StorageUnavailable,
        message_key: "errors.storageUnavailable".into(),
        details: BTreeMap::from([(
            "reasonCode".into(),
            SafeParameterValue::String("appDataDirectory".into()),
        )]),
        retryable: false,
    }
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
    use crate::contracts::{
        AgentEnvironment, AgentId, AgentTriggerStatus, AppErrorCode, BuiltinReminderSoundId,
        CommandError, CreateTodoInput, ModuleId, ReminderSound, ReminderSourceContext,
        ReminderSourceKind, SafeParameterValue, SaveTodoReminderInput, TodoPriority,
    };
    use crate::domain::reminders::{EnqueueOutcome, NewReminderDelivery};
    use std::collections::BTreeMap;

    type PhaseLog = std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>;

    struct FakeShutdownPort {
        phases: PhaseLog,
        stop_entered: Option<std::sync::Arc<tokio::sync::Barrier>>,
        stop_release: Option<std::sync::Arc<tokio::sync::Barrier>>,
    }

    #[async_trait::async_trait]
    impl ShutdownPort for FakeShutdownPort {
        async fn stop_accepting_work(&self) -> Result<(), CommandError> {
            self.phases.lock().unwrap().push("stopAccepting");
            if let (Some(entered), Some(release)) = (&self.stop_entered, &self.stop_release) {
                entered.wait().await;
                release.wait().await;
            }
            Ok(())
        }

        async fn stop_optional_modules(&self) -> Result<(), CommandError> {
            self.phases.lock().unwrap().push("stopOptional");
            Ok(())
        }

        async fn cancel_core_workers(&self) -> Result<(), CommandError> {
            self.phases.lock().unwrap().push("cancelCore");
            Ok(())
        }
    }

    struct FakeCheckpointPort {
        phases: PhaseLog,
    }

    impl WalCheckpointPort for FakeCheckpointPort {
        fn checkpoint_truncate(&self) -> Result<(), CommandError> {
            self.phases.lock().unwrap().push("checkpoint");
            Ok(())
        }
    }

    struct RejectingEmitter;

    struct NotificationPhaseEmitter {
        notifications: NotificationRepository,
        phases: PhaseLog,
        calls: AtomicU64,
    }

    impl EventEmitterPort for NotificationPhaseEmitter {
        fn emit(&self, _: &'static str, _: serde_json::Value) -> Result<(), CommandError> {
            let rows = self
                .notifications
                .list(crate::contracts::ListNotificationHistoryInput {
                    origin: crate::contracts::NotificationOriginFilter::All,
                    source_app: None,
                    unread_only: false,
                    limit: 500,
                })?;
            assert_eq!(rows.len(), 1, "notification commit must precede its hint");
            self.calls.fetch_add(1, Ordering::AcqRel);
            self.phases.lock().unwrap().push("notificationEmit");
            Ok(())
        }
    }

    struct BlockingWpnSource {
        entered: AtomicBool,
        released: (Mutex<bool>, std::sync::Condvar),
        calls: AtomicU64,
        phases: PhaseLog,
    }

    impl BlockingWpnSource {
        fn new(phases: PhaseLog) -> Arc<Self> {
            Arc::new(Self {
                entered: AtomicBool::new(false),
                released: (Mutex::new(false), std::sync::Condvar::new()),
                calls: AtomicU64::new(0),
                phases,
            })
        }

        fn release(&self) {
            let (released, wake) = &self.released;
            *released.lock().unwrap() = true;
            wake.notify_all();
        }
    }

    impl notification_history::WpnSourcePort for BlockingWpnSource {
        fn read_after(
            &self,
            _cursor: crate::repositories::notifications::NotificationCursor,
            _limit: u32,
            received_at: i64,
        ) -> Result<wpn_reader::WpnBatch, wpn_reader::WpnSourceFault> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            self.phases.lock().unwrap().push("notificationReadEntered");
            self.entered.store(true, Ordering::Release);
            let (released, wake) = &self.released;
            let mut released = released.lock().unwrap();
            while !*released {
                released = wake.wait(released).unwrap();
            }
            self.phases.lock().unwrap().push("notificationReadReleased");
            Ok(wpn_reader::WpnBatch {
                items: vec![crate::repositories::notifications::ImportedNotification {
                    origin: crate::repositories::notifications::NotificationOrigin::Windows,
                    app_id: "windows.blocking-fixture".into(),
                    source_entity_id: "wpn:1".into(),
                    source_row_id: Some(1),
                    title: Some("Fixture title".into()),
                    body: Some("Fixture body".into()),
                    message_key: None,
                    message_parameters: None,
                    source_context: None,
                    source_occurred_at: received_at,
                    received_at,
                }],
                cursor: crate::repositories::notifications::NotificationCursor {
                    source_id: wpn_reader::WPN_SOURCE_ID.into(),
                    last_row_id: 1,
                    last_updated_at: received_at,
                },
                has_more: false,
                row_faults: Vec::new(),
            })
        }
    }

    struct ActivationMainWindow {
        storage: Arc<Storage>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl reminder_channels::MainWindowPort for ActivationMainWindow {
        fn show_main(&self) -> Result<(), reminder_channels::ChannelFailure> {
            let stored = self
                .storage
                .with_connection(|connection| {
                    connection
                        .query_row(
                            "SELECT COUNT(*) FROM app_settings WHERE key = 'navigation.reminder.pending'",
                            [],
                            |row| row.get::<_, i64>(0),
                        )
                        .map_err(Into::into)
                })
                .unwrap();
            assert_eq!(stored, 1, "AppServices must persist before showing main");
            self.calls.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    struct ActivationEmitter {
        storage: Arc<Storage>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl reminder_channels::ReminderNavigationEmitter for ActivationEmitter {
        fn emit_navigation(
            &self,
            navigation: &crate::contracts::PendingReminderNavigation,
        ) -> Result<(), reminder_channels::ChannelFailure> {
            let json = self
                .storage
                .with_connection(|connection| {
                    connection
                        .query_row(
                            "SELECT value_json FROM app_settings WHERE key = 'navigation.reminder.pending'",
                            [],
                            |row| row.get::<_, String>(0),
                        )
                        .map_err(Into::into)
                })
                .unwrap();
            assert!(json.contains(&navigation.delivery_id));
            self.calls.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    impl EventEmitterPort for RejectingEmitter {
        fn emit(&self, _: &'static str, _: serde_json::Value) -> Result<(), CommandError> {
            Err(service_stopping_error(false))
        }
    }

    fn service_stopping_error(retryable: bool) -> CommandError {
        CommandError {
            code: AppErrorCode::SourceUnavailable,
            message_key: "errors.serviceStopping".into(),
            details: Default::default(),
            retryable,
        }
    }

    fn registered_test_workers(
        phases: PhaseLog,
        async_result: Result<(), CommandError>,
    ) -> (RegisteredWorker, RegisteredWorker) {
        let async_cancel = std::sync::Arc::new(tokio::sync::Notify::new());
        let thread_cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let async_joined =
            std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));

        let (async_completion_tx, async_completion_rx) = tokio::sync::watch::channel(None);
        let async_join = {
            let cancel = async_cancel.clone();
            let thread_cancelled = thread_cancelled.clone();
            let async_joined = async_joined.clone();
            let phases = phases.clone();
            tauri::async_runtime::spawn(async move {
                cancel.notified().await;
                while !thread_cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    tokio::task::yield_now().await;
                }
                phases.lock().unwrap().push("joinAsync");
                let (joined, wake_thread) = &*async_joined;
                *joined.lock().unwrap() = true;
                wake_thread.notify_one();
                async_completion_tx.send_replace(Some(async_result.clone()));
                async_result
            })
        };
        let async_worker = RegisteredWorker {
            name: "testAsync",
            cancel: std::sync::Arc::new({
                let phases = phases.clone();
                move || {
                    phases.lock().unwrap().push("cancelAsync");
                    async_cancel.notify_one();
                }
            }),
            join: WorkerJoin::Async(async_join),
            completion: async_completion_rx,
        };

        let (thread_completion_tx, thread_completion_rx) = tokio::sync::watch::channel(None);
        let thread_join = {
            let thread_cancelled = thread_cancelled.clone();
            let async_joined = async_joined.clone();
            let phases = phases.clone();
            std::thread::spawn(move || {
                while !thread_cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    std::thread::yield_now();
                }
                let (joined, wake_thread) = &*async_joined;
                let mut joined = joined.lock().unwrap();
                while !*joined {
                    joined = wake_thread.wait(joined).unwrap();
                }
                phases.lock().unwrap().push("joinThread");
                let result = Ok(());
                thread_completion_tx.send_replace(Some(result.clone()));
                result
            })
        };
        let thread_worker = RegisteredWorker {
            name: "testThread",
            cancel: std::sync::Arc::new({
                let phases = phases.clone();
                move || {
                    phases.lock().unwrap().push("cancelThread");
                    thread_cancelled.store(true, std::sync::atomic::Ordering::Release);
                }
            }),
            join: WorkerJoin::Thread(thread_join),
            completion: thread_completion_rx,
        };
        (async_worker, thread_worker)
    }

    fn late_worker(phases: PhaseLog) -> RegisteredWorker {
        let cancelled = std::sync::Arc::new(tokio::sync::Notify::new());
        let (completion_tx, completion_rx) = tokio::sync::watch::channel(None);
        let join = tauri::async_runtime::spawn({
            let cancelled = cancelled.clone();
            let phases = phases.clone();
            async move {
                cancelled.notified().await;
                phases.lock().unwrap().push("joinLate");
                completion_tx.send_replace(Some(Ok(())));
                Ok(())
            }
        });
        RegisteredWorker {
            name: "late",
            cancel: std::sync::Arc::new(move || {
                phases.lock().unwrap().push("cancelLate");
                cancelled.notify_one();
            }),
            join: WorkerJoin::Async(join),
            completion: completion_rx,
        }
    }

    fn test_services(
        phases: PhaseLog,
        stop_gate: Option<(
            std::sync::Arc<tokio::sync::Barrier>,
            std::sync::Arc<tokio::sync::Barrier>,
        )>,
    ) -> std::sync::Arc<AppServices> {
        let temp = tempfile::tempdir().unwrap().keep();
        let (stop_entered, stop_release) = stop_gate
            .map(|(entered, release)| (Some(entered), Some(release)))
            .unwrap_or((None, None));
        AppServices::from_parts(
            std::sync::Arc::new(Storage::open(&temp).unwrap()),
            std::sync::Arc::new(BootstrapModuleStateProvider),
            std::sync::Arc::new(FakeShutdownPort {
                phases: phases.clone(),
                stop_entered,
                stop_release,
            }),
            std::sync::Arc::new(FakeCheckpointPort { phases }),
            std::sync::Arc::new(RejectingEmitter),
        )
    }

    #[test]
    fn app_services_integration_assembly_uses_the_exact_tauri_app_data_directory() {
        let directory = tempfile::tempdir().unwrap();
        let app_data_root = directory.path().join("APPDATA");
        let app_storage = app_data_root.join("com.aisland.app");
        let services = AppServices::from_parts(
            Arc::new(Storage::open(&app_storage).unwrap()),
            Arc::new(BootstrapModuleStateProvider),
            Arc::new(FakeShutdownPort {
                phases: Arc::new(Mutex::new(Vec::new())),
                stop_entered: None,
                stop_release: None,
            }),
            Arc::new(FakeCheckpointPort {
                phases: Arc::new(Mutex::new(Vec::new())),
            }),
            Arc::new(RejectingEmitter),
        );

        let descriptor = services
            .agent_integrations
            .adapter(AgentId::Codex, AgentEnvironment::Windows)
            .unwrap()
            .descriptor();
        let hook_command = &descriptor.owned_hooks[0].command;
        let expected_hook = app_storage.join("agent-hooks").join("codex-windows.ps1");
        let expected_status = app_storage.join("agent-status").join("codex-windows.json");
        assert!(
            hook_command.contains(expected_hook.to_string_lossy().as_ref()),
            "expected {} in {hook_command}",
            expected_hook.display()
        );
        assert!(
            hook_command.contains(expected_status.to_string_lossy().as_ref()),
            "expected {} in {hook_command}",
            expected_status.display()
        );
        assert!(!hook_command.contains("com.aisland.app\\com.aisland"));
    }

    #[test]
    fn production_integration_assembly_uses_seven_exact_config_paths_and_installed_wsl_helper() {
        let directory = tempfile::tempdir().unwrap();
        let windows_home = directory.path().join("Users/Ada");
        let app_data_root = directory.path().join("AppData/Roaming");
        let app_data_dir = app_data_root.join("com.aisland.app");
        let installed = agent_hook_assets::HookAssetPaths {
            paths: vec![agent_hook_assets::HookAssetPath {
                agent_id: AgentId::Codex,
                environment: AgentEnvironment::Wsl,
                destination: agent_hook_assets::HookAssetDestination::Wsl(
                    "/srv/ada/.local/share/aisland/agent-hooks/codex-wsl.sh".into(),
                ),
            }],
            wsl_available: true,
            wsl_status_dir: Some(
                "/mnt/c/Users/Ada/AppData/Roaming/com.aisland.app/agent-status".into(),
            ),
        };
        let assembly = AgentIntegrationAssembly::from_installed(
            windows_home.clone(),
            app_data_root,
            app_data_dir,
            &installed,
        )
        .unwrap();
        let storage = Arc::new(Storage::open(&directory.path().join("db")).unwrap());
        let service = agent_integrations::AgentIntegrationService::new(
            AgentRepository::new(storage.clone()),
            DiagnosticsRepository::new(storage),
            &assembly.windows_home,
            &assembly.app_data_dir,
            &assembly.wsl_home,
            &assembly.wsl_status_dir,
            assembly.wsl_helper.clone(),
        );
        let cases = [
            (
                AgentId::Codex,
                AgentEnvironment::Windows,
                windows_home.join(".codex/hooks.json"),
            ),
            (
                AgentId::Codex,
                AgentEnvironment::Wsl,
                PathBuf::from("/srv/ada/.codex/hooks.json"),
            ),
            (
                AgentId::Hermes,
                AgentEnvironment::Windows,
                windows_home.join(".hermes/config.yaml"),
            ),
            (
                AgentId::Hermes,
                AgentEnvironment::Wsl,
                PathBuf::from("/srv/ada/.hermes/config.yaml"),
            ),
            (
                AgentId::Workbuddy,
                AgentEnvironment::Windows,
                windows_home.join(".workbuddy-ai/settings.json"),
            ),
            (
                AgentId::Claude,
                AgentEnvironment::Windows,
                windows_home.join(".claude/settings.json"),
            ),
            (
                AgentId::Claude,
                AgentEnvironment::Wsl,
                PathBuf::from("/srv/ada/.claude/settings.json"),
            ),
        ];
        for (agent_id, environment, expected) in cases {
            assert_eq!(
                service
                    .adapter(agent_id, environment)
                    .unwrap()
                    .descriptor()
                    .config_path,
                expected
            );
        }
        assert_eq!(
            assembly.wsl_argv("read", "/srv/ada/.codex/hooks.json"),
            vec![
                "--exec",
                "sh",
                "/srv/ada/.local/share/aisland/agent-hooks/aisland-config-wsl.sh",
                "read",
                "/srv/ada/.codex/hooks.json",
            ]
        );
        let wsl_codex = service
            .adapter(AgentId::Codex, AgentEnvironment::Wsl)
            .unwrap()
            .descriptor();
        assert!(wsl_codex
            .owned_hooks
            .iter()
            .all(|hook| hook.command.contains(
                "/mnt/c/Users/Ada/AppData/Roaming/com.aisland.app/agent-status/codex-wsl.json"
            )));
        assert!(wsl_codex
            .owned_hooks
            .iter()
            .all(|hook| !hook.command.contains(".local/share/aisland/agent-status")));
    }

    fn activation_delivery(services: &AppServices) -> crate::contracts::ReminderDelivery {
        let request = NewReminderDelivery {
            dedupe_key: "activation-app-services".into(),
            rule_id: None,
            source_kind: ReminderSourceKind::Agent,
            source_entity_id: "agent:rule:codex:windows:task:completed".into(),
            message_key: "reminders.agent.status".into(),
            message_parameters: BTreeMap::from([
                (
                    "agentName".into(),
                    SafeParameterValue::String("Codex".into()),
                ),
                (
                    "environment".into(),
                    SafeParameterValue::String("windows".into()),
                ),
                ("taskId".into(), SafeParameterValue::String("task".into())),
                (
                    "taskTitle".into(),
                    SafeParameterValue::String("task".into()),
                ),
                (
                    "triggerStatus".into(),
                    SafeParameterValue::String("completed".into()),
                ),
            ]),
            source_context: ReminderSourceContext::Agent {
                agent_id: AgentId::Codex,
                environment: AgentEnvironment::Windows,
                task_id: "task".into(),
                task_title: None,
                trigger_status: AgentTriggerStatus::Completed,
                source_event_id: "event".into(),
                source_occurred_at: 10,
            },
            source_occurred_at: 10,
            sound: ReminderSound::Builtin {
                sound_id: BuiltinReminderSoundId::SystemNotification,
            },
            toast_enabled: true,
            window_enabled: true,
            due_at: 10,
        };
        let EnqueueOutcome::Inserted(_) = services.reminders.enqueue(request, 10).unwrap() else {
            panic!("activation fixture must insert")
        };
        services.reminders.claim_due(10, 1).unwrap().pop().unwrap()
    }

    #[test]
    fn app_services_toast_callback_persists_before_show_and_emit() {
        let phases: PhaseLog = Default::default();
        let services = test_services(phases, None);
        let delivery = activation_delivery(&services);
        let main = Arc::new(ActivationMainWindow {
            storage: services.storage.clone(),
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let emitter = Arc::new(ActivationEmitter {
            storage: services.storage.clone(),
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let port = services.toast_activation_port();
        let router = Arc::new(reminder_channels::ToastActivationRouter::new(
            services.reminder_channels.clone(),
            main.clone(),
            emitter.clone(),
            Arc::new(|| 20),
        ));
        assert!(port.install_once(&router));

        port.dispatch_uuid_only(&delivery.id);

        assert_eq!(main.calls.load(Ordering::Acquire), 1);
        assert_eq!(emitter.calls.load(Ordering::Acquire), 1);
    }

    // Break caught: UI event emission is only a hint.  Its failure must reach the scheduler but
    // cannot prevent the independent channel wake from attempting the already durable row.
    #[tokio::test]
    async fn dispatch_wakes_channels_even_when_the_ui_emit_returns_an_error() {
        let phases: PhaseLog = Default::default();
        let services = test_services(phases, None);
        services.start_reminder_channel_worker_once().unwrap();
        let delivery = activation_delivery(&services);
        let emitter = ReminderDispatchEmitter {
            inner: Arc::new(RejectingEmitter),
            channels: services.reminder_channels.clone(),
        };

        let error = emitter
            .emit(
                REMINDER_DISPATCH_READY,
                crate::events::reminder_dispatch_ready_payload(&delivery.id, delivery.dispatch_seq),
            )
            .expect_err("UI emission failure remains visible to the scheduler");
        assert_eq!(error.message_key, "errors.serviceStopping");
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if !services
                    .reminders
                    .is_channel_pending(&delivery.id, delivery.dispatch_seq, "sound")
                    .unwrap()
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("channel wake must survive failed UI emission");
        services.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_registers_both_join_kinds_and_shares_first_error() {
        let phases: PhaseLog = Default::default();
        let stop_entered = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let stop_release = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let services = test_services(
            phases.clone(),
            Some((stop_entered.clone(), stop_release.clone())),
        );
        let injected = service_stopping_error(true);
        let (async_worker, thread_worker) =
            registered_test_workers(phases.clone(), Err(injected.clone()));
        services.register_worker(async_worker).await.unwrap();
        services.register_worker(thread_worker).await.unwrap();

        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));
        let first = tauri::async_runtime::spawn({
            let services = services.clone();
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                services.shutdown().await
            }
        });
        let second = tauri::async_runtime::spawn({
            let services = services.clone();
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                services.shutdown().await
            }
        });
        barrier.wait().await;

        stop_entered.wait().await;
        assert!(!services.accepts_workers());
        assert_eq!(services.worker_take_count(), 1);
        let late_phases: PhaseLog = Default::default();
        let error = match services
            .register_worker(late_worker(late_phases.clone()))
            .await
        {
            Ok(_) => panic!("late worker registration unexpectedly returned a lease"),
            Err(error) => error,
        };
        assert_eq!(error.code, AppErrorCode::SourceUnavailable);
        assert_eq!(error.message_key, "errors.serviceStopping");
        assert_eq!(*late_phases.lock().unwrap(), vec!["cancelLate", "joinLate"]);
        assert_eq!(services.worker_take_count(), 1);
        stop_release.wait().await;

        let first = first.await.unwrap();
        let second = second.await.unwrap();
        assert_eq!(first, Err(injected.clone()));
        assert_eq!(second, Err(injected));
        assert_eq!(services.worker_take_count(), 1);
        assert_eq!(
            *phases.lock().unwrap(),
            vec![
                "stopAccepting",
                "stopOptional",
                "cancelCore",
                "cancelAsync",
                "cancelThread",
                "joinAsync",
                "joinThread",
                "checkpoint",
            ]
        );
    }

    // Break caught: setup re-entry must not create a second watcher, and shutdown must join the one owner.
    #[tokio::test]
    async fn agent_watcher_starts_once_and_shutdown_joins_the_registered_worker() {
        let phases: PhaseLog = Default::default();
        let services = test_services(phases.clone(), None);
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first-status");
        let second = directory.path().join("second-status");

        services
            .start_agent_status_watcher_once(first.clone())
            .unwrap();
        services
            .start_agent_status_watcher_once(second.clone())
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !first.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the registered watcher must start");
        assert!(!second.exists());

        tokio::time::timeout(std::time::Duration::from_secs(1), services.shutdown())
            .await
            .expect("shutdown must join the watcher")
            .unwrap();
        assert_eq!(services.worker_take_count(), 1);
        assert_eq!(phases.lock().unwrap().last(), Some(&"checkpoint"));
    }

    // Break caught: application setup may be re-entered by a test harness, while shutdown can be
    // requested concurrently by more than one window event.  Dynamic agent processes must still
    // have one restore owner and must be stopped before SQLite is checkpointed.
    #[tokio::test]
    async fn agent_profiles_restore_once_and_shutdown_before_checkpoint() {
        let phases: PhaseLog = Default::default();
        let services = test_services(phases.clone(), None);
        services.agent_profiles.set_lifecycle_hook(Arc::new({
            let phases = phases.clone();
            move |phase| phases.lock().unwrap().push(phase)
        }));

        assert_eq!(services.restore_agent_profiles_once().unwrap(), 0);
        assert_eq!(services.restore_agent_profiles_once().unwrap(), 0);
        services.shutdown().await.unwrap();
        services.shutdown().await.unwrap();

        let phases = phases.lock().unwrap();
        assert_eq!(
            phases
                .iter()
                .filter(|phase| **phase == "agentProfilesRestore")
                .count(),
            1
        );
        assert_eq!(
            phases
                .iter()
                .filter(|phase| **phase == "agentProfilesShutdown")
                .count(),
            1
        );
        assert!(
            phases
                .iter()
                .position(|phase| *phase == "agentProfilesShutdown")
                .unwrap()
                < phases
                    .iter()
                    .position(|phase| *phase == "checkpoint")
                    .unwrap()
        );
    }

    // Break caught: a start racing after shutdown closes registration must not spawn watcher side effects.
    #[tokio::test]
    async fn shutdown_in_progress_rejects_first_watcher_start_before_directory_creation() {
        let phases: PhaseLog = Default::default();
        let stop_entered = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let stop_release = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let services = test_services(phases, Some((stop_entered.clone(), stop_release.clone())));
        let directory = tempfile::tempdir().unwrap();
        let status_dir = directory.path().join("must-not-start");
        let shutdown = tauri::async_runtime::spawn({
            let services = services.clone();
            async move { services.shutdown().await }
        });
        stop_entered.wait().await;

        let error = services
            .start_agent_status_watcher_once(status_dir.clone())
            .unwrap_err();
        assert_eq!(error.code, AppErrorCode::SourceUnavailable);
        assert_eq!(error.message_key, "errors.serviceStopping");
        assert!(!status_dir.exists());

        stop_release.wait().await;
        shutdown.await.unwrap().unwrap();
        assert!(!status_dir.exists());
    }

    // Break caught: a service whose shutdown checkpoint completed must permanently reject its first watcher start.
    #[tokio::test]
    async fn completed_shutdown_rejects_first_watcher_start_without_side_effects() {
        let phases: PhaseLog = Default::default();
        let services = test_services(phases.clone(), None);
        services.shutdown().await.unwrap();
        assert_eq!(phases.lock().unwrap().last(), Some(&"checkpoint"));
        let directory = tempfile::tempdir().unwrap();
        let status_dir = directory.path().join("after-checkpoint");

        let error = services
            .start_agent_status_watcher_once(status_dir.clone())
            .unwrap_err();
        assert_eq!(error.code, AppErrorCode::SourceUnavailable);
        assert_eq!(error.message_key, "errors.serviceStopping");
        assert!(!status_dir.exists());
    }

    #[tokio::test]
    async fn aborted_late_registration_cleanup_finishes_before_checkpoint() {
        let phases: PhaseLog = Default::default();
        let stop_entered = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let stop_release = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let services = test_services(
            phases.clone(),
            Some((stop_entered.clone(), stop_release.clone())),
        );

        let shutdown = tauri::async_runtime::spawn({
            let services = services.clone();
            async move { services.shutdown().await }
        });
        stop_entered.wait().await;

        let cancelled = std::sync::Arc::new(tokio::sync::Notify::new());
        let join_entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let join_release = std::sync::Arc::new(tokio::sync::Notify::new());
        let (completion_tx, completion_rx) = tokio::sync::watch::channel(None);
        let join = tauri::async_runtime::spawn({
            let cancelled = cancelled.clone();
            let join_entered = join_entered.clone();
            let join_release = join_release.clone();
            let phases = phases.clone();
            async move {
                cancelled.notified().await;
                join_entered.notify_one();
                join_release.notified().await;
                phases.lock().unwrap().push("joinAbortedLate");
                completion_tx.send_replace(Some(Ok(())));
                Ok(())
            }
        });
        let late_worker = RegisteredWorker {
            name: "abortedLate",
            cancel: std::sync::Arc::new({
                let cancelled = cancelled.clone();
                let phases = phases.clone();
                move || {
                    phases.lock().unwrap().push("cancelAbortedLate");
                    cancelled.notify_one();
                }
            }),
            join: WorkerJoin::Async(join),
            completion: completion_rx.clone(),
        };
        let registration = tauri::async_runtime::spawn({
            let services = services.clone();
            async move { services.register_worker(late_worker).await }
        });

        join_entered.notified().await;
        registration.abort();
        assert!(registration.await.is_err());
        stop_release.wait().await;

        let mut shutdown = shutdown;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut shutdown)
                .await
                .is_err(),
            "shutdown reached WAL before the independently owned rejected cleanup finished"
        );
        assert!(!phases.lock().unwrap().contains(&"checkpoint"));

        join_release.notify_one();
        shutdown.await.unwrap().unwrap();
        assert_eq!(*completion_rx.borrow(), Some(Ok(())));
        assert_eq!(
            *phases.lock().unwrap(),
            vec![
                "stopAccepting",
                "cancelAbortedLate",
                "stopOptional",
                "cancelCore",
                "joinAbortedLate",
                "checkpoint",
            ]
        );
    }

    #[tokio::test]
    async fn empty_cleanup_observation_keeps_late_worker_owned_before_checkpoint() {
        let registry = RejectedWorkerCleanupRegistry::new();
        assert!(registry.take_batch().is_none());

        let phases: PhaseLog = Default::default();
        let cancelled = std::sync::Arc::new(tokio::sync::Notify::new());
        let join_entered = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let join_release = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let (completion_tx, completion_rx) = tokio::sync::watch::channel(None);
        let join = tauri::async_runtime::spawn({
            let cancelled = cancelled.clone();
            let join_entered = join_entered.clone();
            let join_release = join_release.clone();
            let phases = phases.clone();
            async move {
                cancelled.notified().await;
                join_entered.wait().await;
                join_release.wait().await;
                phases.lock().unwrap().push("joinAfterEmpty");
                completion_tx.send_replace(Some(Ok(())));
                Ok(())
            }
        });
        let worker = RegisteredWorker {
            name: "afterEmpty",
            cancel: std::sync::Arc::new({
                let cancelled = cancelled.clone();
                let phases = phases.clone();
                move || {
                    phases.lock().unwrap().push("cancelAfterEmpty");
                    cancelled.notify_one();
                }
            }),
            join: WorkerJoin::Async(join),
            completion: completion_rx,
        };
        let cleanup_result = match registry.reserve_cleanup() {
            RejectedWorkerCleanupReservation::ShutdownOwned(sender) => sender,
            RejectedWorkerCleanupReservation::Finalized => {
                panic!("empty observation unexpectedly finalized cleanup ownership")
            }
        };
        let cleanup_driver = tauri::async_runtime::spawn(async move {
            let result = worker.cancel_and_join().await;
            cleanup_result.send(result).ok();
        });
        drop(cleanup_driver);

        let checkpoint = FakeCheckpointPort {
            phases: phases.clone(),
        };
        let cleanup_batch = registry
            .take_batch_or_finalize(|| {
                checkpoint.checkpoint_truncate().unwrap();
            })
            .expect("late cleanup must remain owned by the shutdown drain");
        let drain = tauri::async_runtime::spawn(cleanup_batch.await_all());
        join_entered.wait().await;
        assert_eq!(*phases.lock().unwrap(), vec!["cancelAfterEmpty"]);
        assert!(!phases.lock().unwrap().contains(&"checkpoint"));

        join_release.wait().await;
        drain.await.unwrap().unwrap();
        assert!(registry
            .take_batch_or_finalize(|| {
                checkpoint.checkpoint_truncate().unwrap();
            })
            .is_none());

        assert_eq!(
            *phases.lock().unwrap(),
            vec!["cancelAfterEmpty", "joinAfterEmpty", "checkpoint"]
        );
    }

    #[tokio::test]
    async fn reserved_cleanup_ticket_blocks_finalize_while_spawn_is_paused() {
        let registry = std::sync::Arc::new(RejectedWorkerCleanupRegistry::new());
        let cleanup_result = match registry.reserve_cleanup() {
            RejectedWorkerCleanupReservation::ShutdownOwned(sender) => sender,
            RejectedWorkerCleanupReservation::Finalized => {
                panic!("cleanup registry finalized before shutdown")
            }
        };
        let phases: PhaseLog = Default::default();
        let checkpoint = std::sync::Arc::new(FakeCheckpointPort {
            phases: phases.clone(),
        });
        let finalizer = tauri::async_runtime::spawn({
            let registry = registry.clone();
            let checkpoint = checkpoint.clone();
            async move {
                let mut first_result = Ok(());
                loop {
                    match registry.take_batch_or_finalize(|| {
                        record_first(&mut first_result, checkpoint.checkpoint_truncate());
                    }) {
                        Some(cleanup_batch) => {
                            record_first(&mut first_result, cleanup_batch.await_all().await);
                        }
                        None => return first_result,
                    }
                }
            }
        });

        let spawn_entered = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let spawn_release = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let cleanup = tauri::async_runtime::spawn({
            let spawn_entered = spawn_entered.clone();
            let spawn_release = spawn_release.clone();
            let phases = phases.clone();
            async move {
                spawn_entered.wait().await;
                spawn_release.wait().await;
                phases.lock().unwrap().push("cancelTicketed");
                phases.lock().unwrap().push("joinTicketed");
                cleanup_result.send(Ok(())).ok();
            }
        });
        drop(cleanup);

        spawn_entered.wait().await;
        assert!(
            !phases.lock().unwrap().contains(&"checkpoint"),
            "finalizer crossed WAL while a pre-owned cleanup spawn was paused"
        );
        spawn_release.wait().await;
        finalizer.await.unwrap().unwrap();

        assert_eq!(
            *phases.lock().unwrap(),
            vec!["cancelTicketed", "joinTicketed", "checkpoint"]
        );
    }

    #[tokio::test]
    async fn post_completion_registration_joins_without_leaking_or_republishing_shutdown() {
        let phases: PhaseLog = Default::default();
        let services = test_services(phases.clone(), None);
        services.shutdown().await.unwrap();

        let cancelled = std::sync::Arc::new(tokio::sync::Notify::new());
        let join_entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let join_release = std::sync::Arc::new(tokio::sync::Notify::new());
        let (completion_tx, completion_rx) = tokio::sync::watch::channel(None);
        let join = tauri::async_runtime::spawn({
            let cancelled = cancelled.clone();
            let join_entered = join_entered.clone();
            let join_release = join_release.clone();
            let phases = phases.clone();
            async move {
                cancelled.notified().await;
                join_entered.notify_one();
                join_release.notified().await;
                phases.lock().unwrap().push("joinAfterComplete");
                completion_tx.send_replace(Some(Ok(())));
                Ok(())
            }
        });
        let worker = RegisteredWorker {
            name: "afterComplete",
            cancel: std::sync::Arc::new({
                let cancelled = cancelled.clone();
                let phases = phases.clone();
                move || {
                    phases.lock().unwrap().push("cancelAfterComplete");
                    cancelled.notify_one();
                }
            }),
            join: WorkerJoin::Async(join),
            completion: completion_rx,
        };
        let registration = tauri::async_runtime::spawn({
            let services = services.clone();
            async move { services.register_worker(worker).await }
        });

        join_entered.notified().await;
        join_release.notify_one();
        let error = match registration.await.unwrap() {
            Ok(_) => panic!("post-completion registration unexpectedly returned a lease"),
            Err(error) => error,
        };
        assert_eq!(error.code, AppErrorCode::SourceUnavailable);
        assert_eq!(error.message_key, "errors.serviceStopping");
        assert!(
            services.rejected_worker_cleanups.take_batch().is_none(),
            "post-completion registration left a cleanup handle with no future drain"
        );

        services.shutdown().await.unwrap();
        assert_eq!(
            *phases.lock().unwrap(),
            vec![
                "stopAccepting",
                "stopOptional",
                "cancelCore",
                "checkpoint",
                "cancelAfterComplete",
                "joinAfterComplete"
            ]
        );
    }

    #[test]
    fn post_completion_async_cleanup_outlives_aborted_caller_on_single_worker_runtime() {
        let phases: PhaseLog = Default::default();
        let services = test_services(phases.clone(), None);
        tauri::async_runtime::block_on(services.shutdown()).unwrap();

        let runtime = tauri::async_runtime::Runtime::Tokio(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .unwrap(),
        );
        let cancelled = std::sync::Arc::new(tokio::sync::Notify::new());
        let (cancel_entered_tx, cancel_entered_rx) = std::sync::mpsc::sync_channel(1);
        let (joined_tx, joined_rx) = std::sync::mpsc::sync_channel(1);
        let (completion_tx, completion_rx) = tokio::sync::watch::channel(None);
        let join = runtime.spawn({
            let cancelled = cancelled.clone();
            let phases = phases.clone();
            async move {
                cancelled.notified().await;
                phases.lock().unwrap().push("joinAsyncAfterComplete");
                completion_tx.send_replace(Some(Ok(())));
                joined_tx.send(()).unwrap();
                Ok(())
            }
        });
        let worker = RegisteredWorker {
            name: "asyncAfterComplete",
            cancel: std::sync::Arc::new({
                let cancelled = cancelled.clone();
                let phases = phases.clone();
                move || {
                    phases.lock().unwrap().push("cancelAsyncAfterComplete");
                    cancel_entered_tx.send(()).ok();
                    cancelled.notify_one();
                }
            }),
            join: WorkerJoin::Async(join),
            completion: completion_rx.clone(),
        };
        let registration = runtime.spawn({
            let services = services.clone();
            async move { services.register_worker(worker).await }
        });

        cancel_entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("post-completion async cleanup never started cancellation");
        registration.abort();
        if joined_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .is_err()
        {
            std::mem::forget(registration);
            std::mem::forget(runtime);
            panic!("post-completion cleanup blocked the only runtime worker after caller abort");
        }
        assert!(tauri::async_runtime::block_on(registration).is_err());
        assert_eq!(*completion_rx.borrow(), Some(Ok(())));
        assert_eq!(
            *phases.lock().unwrap(),
            vec![
                "stopAccepting",
                "stopOptional",
                "cancelCore",
                "checkpoint",
                "cancelAsyncAfterComplete",
                "joinAsyncAfterComplete"
            ]
        );
    }

    #[test]
    fn post_completion_thread_cleanup_returns_waiter_before_join_finishes() {
        let phases: PhaseLog = Default::default();
        let services = test_services(phases.clone(), None);
        tauri::async_runtime::block_on(services.shutdown()).unwrap();

        let (cancel_entered_tx, cancel_entered_rx) = std::sync::mpsc::sync_channel(1);
        let (join_release_tx, join_release_rx) = std::sync::mpsc::sync_channel(1);
        let (completion_tx, completion_rx) = tokio::sync::watch::channel(None);
        let join = std::thread::spawn({
            let phases = phases.clone();
            move || {
                join_release_rx.recv().unwrap();
                phases.lock().unwrap().push("joinThreadAfterComplete");
                completion_tx.send_replace(Some(Ok(())));
                Ok(())
            }
        });
        let worker = RegisteredWorker {
            name: "threadAfterComplete",
            cancel: std::sync::Arc::new({
                let phases = phases.clone();
                move || {
                    phases.lock().unwrap().push("cancelThreadAfterComplete");
                    cancel_entered_tx.send(()).ok();
                }
            }),
            join: WorkerJoin::Thread(join),
            completion: completion_rx,
        };
        let (waiter_created_tx, waiter_created_rx) = std::sync::mpsc::sync_channel(1);
        let (registration_tx, registration_rx) = std::sync::mpsc::sync_channel(1);
        let registration = tauri::async_runtime::spawn({
            let services = services.clone();
            async move {
                let waiter = services.register_worker(worker);
                waiter_created_tx.send(()).unwrap();
                registration_tx.send(waiter.await).unwrap();
            }
        });
        drop(registration);

        cancel_entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("post-completion thread cleanup never started cancellation");
        let waiter_returned_before_release = waiter_created_rx
            .recv_timeout(std::time::Duration::from_millis(200))
            .is_ok();
        join_release_tx.send(()).unwrap();
        let error = match registration_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("post-completion thread registration never finished")
        {
            Ok(_) => panic!("post-completion thread registration unexpectedly returned a lease"),
            Err(error) => error,
        };

        assert!(
            waiter_returned_before_release,
            "register_worker synchronously joined the post-completion thread before returning its waiter"
        );
        assert_eq!(error.code, AppErrorCode::SourceUnavailable);
        assert_eq!(error.message_key, "errors.serviceStopping");
        assert_eq!(
            *phases.lock().unwrap(),
            vec![
                "stopAccepting",
                "stopOptional",
                "cancelCore",
                "checkpoint",
                "cancelThreadAfterComplete",
                "joinThreadAfterComplete"
            ]
        );
    }

    #[tokio::test]
    async fn worker_registry_takes_once_and_later_shutdown_callers_reuse_ok() {
        let registry = WorkerJoinRegistry::new();
        let registry_phases: PhaseLog = Default::default();
        let (async_worker, thread_worker) = registered_test_workers(registry_phases, Ok(()));
        assert!(registry.register(async_worker).is_ok());
        assert!(registry.register(thread_worker).is_ok());
        let first_batch = registry.stop_accepting_and_take();
        let second_batch = registry.stop_accepting_and_take();
        assert_eq!(first_batch.len(), 2);
        assert!(second_batch.is_empty());
        assert_eq!(registry.take_count(), 1);
        first_batch.cancel_all();
        first_batch.await_all().await.unwrap();

        let phases: PhaseLog = Default::default();
        let services = test_services(phases.clone(), None);
        let (async_worker, thread_worker) = registered_test_workers(phases.clone(), Ok(()));
        services.register_worker(async_worker).await.unwrap();
        services.register_worker(thread_worker).await.unwrap();
        services.shutdown().await.unwrap();
        let before = phases.lock().unwrap().clone();
        let (later_1, later_2, later_3) = tokio::join!(
            services.shutdown(),
            services.shutdown(),
            services.shutdown()
        );
        assert_eq!((later_1, later_2, later_3), (Ok(()), Ok(()), Ok(())));
        assert_eq!(*phases.lock().unwrap(), before);
        assert_eq!(services.worker_take_count(), 1);
    }

    #[tokio::test]
    async fn worker_registry_retires_and_joins_only_the_exact_registration_once() {
        let registry = Arc::new(WorkerJoinRegistry::new());
        let phases: PhaseLog = Default::default();
        let old_cancelled = Arc::new(tokio::sync::Notify::new());
        let old_return_release = Arc::new(tokio::sync::Notify::new());
        let (old_completion_tx, old_completion_rx) = tokio::sync::watch::channel(None);
        let old_join = tauri::async_runtime::spawn({
            let phases = phases.clone();
            let old_cancelled = old_cancelled.clone();
            let old_return_release = old_return_release.clone();
            async move {
                old_cancelled.notified().await;
                phases.lock().unwrap().push("completeOld");
                old_completion_tx.send_replace(Some(Ok(())));
                old_return_release.notified().await;
                phases.lock().unwrap().push("returnOld");
                Ok(())
            }
        });
        let old = RegisteredWorker {
            name: "sameName",
            cancel: Arc::new({
                let phases = phases.clone();
                move || {
                    phases.lock().unwrap().push("cancelOld");
                    old_cancelled.notify_one();
                }
            }),
            join: WorkerJoin::Async(old_join),
            completion: old_completion_rx,
        };
        let current_cancelled = Arc::new(tokio::sync::Notify::new());
        let (current_completion_tx, current_completion_rx) = tokio::sync::watch::channel(None);
        let current_join = tauri::async_runtime::spawn({
            let phases = phases.clone();
            let current_cancelled = current_cancelled.clone();
            async move {
                current_cancelled.notified().await;
                phases.lock().unwrap().push("returnCurrent");
                current_completion_tx.send_replace(Some(Ok(())));
                Ok(())
            }
        });
        let current = RegisteredWorker {
            name: "sameName",
            cancel: Arc::new({
                let phases = phases.clone();
                move || {
                    phases.lock().unwrap().push("cancelCurrent");
                    current_cancelled.notify_one();
                }
            }),
            join: WorkerJoin::Async(current_join),
            completion: current_completion_rx,
        };
        let old_lease = match registry.register(old) {
            Ok(lease) => lease,
            Err(_) => panic!("retirement test registry must accept the old worker"),
        };
        let duplicate_old_lease = old_lease.clone();
        if registry.register(current).is_err() {
            panic!("retirement test registry must accept the current worker");
        }
        let (retired_tx, mut retired_rx) = tokio::sync::oneshot::channel();
        let retirement = tauri::async_runtime::spawn({
            let registry = registry.clone();
            async move {
                retired_tx.send(registry.retire(old_lease).await).ok();
            }
        });

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while registry.registered_count() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("completed old registration must be removed before join returns");
        assert!(
            matches!(
                retired_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "retirement must await the owned join handle, not only completion"
        );
        old_return_release.notify_one();
        retired_rx.await.unwrap().unwrap();
        retirement.await.unwrap();
        let duplicate_error = registry.retire(duplicate_old_lease).await.unwrap_err();
        assert_eq!(duplicate_error.message_key, "errors.serviceStopping");
        assert_eq!(
            phases
                .lock()
                .unwrap()
                .iter()
                .filter(|phase| **phase == "returnOld")
                .count(),
            1,
            "the removed join handle can be consumed exactly once"
        );

        let batch = registry.stop_accepting_and_take();
        assert_eq!(batch.len(), 1, "same worker name must not retire its peer");
        batch.cancel_all();
        batch.await_all().await.unwrap();
        assert_eq!(phases.lock().unwrap().last(), Some(&"returnCurrent"));
    }

    #[tokio::test]
    async fn shutdown_awaits_an_inflight_retirement_after_its_caller_is_cancelled() {
        let registry = Arc::new(WorkerJoinRegistry::new());
        let returned = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(tokio::sync::Notify::new());
        let return_release = Arc::new(tokio::sync::Notify::new());
        let (completion_tx, completion_rx) = tokio::sync::watch::channel(None);
        let join = tauri::async_runtime::spawn({
            let returned = returned.clone();
            let cancelled = cancelled.clone();
            let return_release = return_release.clone();
            async move {
                cancelled.notified().await;
                completion_tx.send_replace(Some(Ok(())));
                return_release.notified().await;
                returned.store(true, Ordering::Release);
                Ok(())
            }
        });
        let worker = RegisteredWorker {
            name: "retiringDuringShutdown",
            cancel: Arc::new(move || cancelled.notify_one()),
            join: WorkerJoin::Async(join),
            completion: completion_rx,
        };
        let lease = match registry.register(worker) {
            Ok(lease) => lease,
            Err(_) => panic!("retirement shutdown test registry must accept its worker"),
        };
        let retirement = tauri::async_runtime::spawn({
            let registry = registry.clone();
            async move { registry.retire(lease).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while registry.registered_count() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("retirement must transfer the join to registry-owned in-flight state");
        retirement.abort();
        assert!(retirement.await.is_err());

        let batch = registry.stop_accepting_and_take();
        assert_eq!(batch.len(), 1, "shutdown must own the in-flight retirement");
        let (shutdown_done_tx, mut shutdown_done_rx) = tokio::sync::oneshot::channel();
        let shutdown_wait = tauri::async_runtime::spawn(async move {
            shutdown_done_tx.send(batch.await_all().await).ok();
        });
        assert!(
            matches!(
                shutdown_done_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "shutdown must not finish before the retirement join"
        );
        return_release.notify_one();
        shutdown_done_rx.await.unwrap().unwrap();
        shutdown_wait.await.unwrap();
        assert!(returned.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn shutdown_stops_worker_registration_before_waiting_for_activation_teardown() {
        let phases = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let services = test_services(phases.clone(), None);
        let router_guard = services
            .toast_activation_router
            .lock()
            .expect("test holds activation teardown lock");
        let shutdown_services = services.clone();
        let shutdown = std::thread::spawn(move || {
            tauri::async_runtime::block_on(shutdown_services.shutdown())
        });

        tokio::time::timeout(std::time::Duration::from_millis(100), async {
            while !services.shutdown_started.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("shutdown CAS must complete before teardown waits");

        let registration = services.register_worker(late_worker(phases)).await;
        let rejected =
            matches!(registration, Err(error) if error.message_key == "errors.serviceStopping");
        drop(router_guard);
        shutdown.join().unwrap().unwrap();
        assert!(
            rejected,
            "no worker may be accepted after shutdown begins, even while activation teardown blocks"
        );
    }

    // Break caught: startup must register exactly one reminder worker and shutdown must reject
    // any later start before it checkpoints the database.
    #[tokio::test]
    async fn reminder_worker_starts_once_and_shutdown_rejects_later_start() {
        let phases: PhaseLog = Default::default();
        let services = test_services(phases.clone(), None);

        services.start_reminder_worker_once().unwrap();
        services.start_reminder_worker_once().unwrap();
        services.shutdown().await.unwrap();

        let error = services.start_reminder_worker_once().unwrap_err();
        assert_eq!(error.message_key, "errors.serviceStopping");
        assert_eq!(services.worker_take_count(), 1);
        let phases = phases.lock().unwrap().clone();
        assert!(phases.ends_with(&["checkpoint".into()]));
    }

    #[tokio::test]
    async fn notification_worker_starts_once_and_joins_before_checkpoint() {
        let phases: PhaseLog = Default::default();
        let services = test_services(phases.clone(), None);
        let constructions = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        services
            .start_notification_history_worker_once_with({
                let constructions = constructions.clone();
                let phases = phases.clone();
                move |mut shutdown| async move {
                    constructions.fetch_add(1, Ordering::AcqRel);
                    phases.lock().unwrap().push("notificationInitialDrain");
                    while !*shutdown.borrow() {
                        if shutdown.changed().await.is_err() {
                            break;
                        }
                    }
                    phases.lock().unwrap().push("notificationCompletion");
                    Ok(())
                }
            })
            .unwrap();
        services
            .start_notification_history_worker_once_with({
                let constructions = constructions.clone();
                move |_| async move {
                    constructions.fetch_add(1, Ordering::AcqRel);
                    Ok(())
                }
            })
            .unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !phases.lock().unwrap().contains(&"notificationInitialDrain") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(constructions.load(Ordering::Acquire), 1);

        services.shutdown().await.unwrap();
        assert_eq!(services.worker_take_count(), 1);
        let phases = phases.lock().unwrap().clone();
        assert!(
            phases
                .iter()
                .position(|phase| *phase == "notificationInitialDrain")
                < phases
                    .iter()
                    .position(|phase| *phase == "notificationCompletion")
        );
        assert!(
            phases
                .iter()
                .position(|phase| *phase == "notificationCompletion")
                < phases.iter().position(|phase| *phase == "checkpoint")
        );

        let error = services
            .start_notification_history_worker_once_with(move |_| async move { Ok(()) })
            .unwrap_err();
        assert_eq!(error.message_key, "errors.serviceStopping");
        assert_eq!(constructions.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn rejected_notification_registration_never_constructs_or_publishes_the_worker() {
        let phases: PhaseLog = Default::default();
        let services = test_services(phases, None);
        let constructions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let rejected_batch = services.worker_joins.stop_accepting_and_take();
        assert!(rejected_batch.is_empty());

        let error = services
            .start_notification_history_worker_once_with({
                let constructions = constructions.clone();
                move |_| async move {
                    constructions.fetch_add(1, Ordering::AcqRel);
                    Ok(())
                }
            })
            .unwrap_err();

        assert_eq!(error.message_key, "errors.serviceStopping");
        assert_eq!(constructions.load(Ordering::Acquire), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn notification_shutdown_waits_for_blocked_source_and_never_writes_after_checkpoint() {
        let phases: PhaseLog = Default::default();
        let services = test_services(phases.clone(), None);
        let source = BlockingWpnSource::new(phases.clone());
        let emitter = Arc::new(NotificationPhaseEmitter {
            notifications: services.notifications.clone(),
            phases: phases.clone(),
            calls: AtomicU64::new(0),
        });

        services
            .start_notification_history_worker_once_with({
                let notification_history = services.notification_history.clone();
                let reminders = services.reminders.clone();
                let health = services.health.clone();
                let diagnostics = services.diagnostics.clone();
                let source = source.clone();
                let emitter = emitter.clone();
                let phases = phases.clone();
                move |shutdown| async move {
                    let worker = notification_history.start_worker_with_ports(
                        source,
                        reminders,
                        health,
                        diagnostics,
                        emitter,
                        1,
                        Arc::new(AtomicU64::new(0)),
                    );
                    worker.run(shutdown).await;
                    phases.lock().unwrap().push("notificationCompletion");
                    Ok(())
                }
            })
            .unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !source.entered.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let shutdown = tauri::async_runtime::spawn({
            let services = services.clone();
            async move { services.shutdown().await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !phases.lock().unwrap().contains(&"cancelCore") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!phases.lock().unwrap().contains(&"checkpoint"));

        source.release();
        shutdown.await.unwrap().unwrap();

        assert_eq!(source.calls.load(Ordering::Acquire), 1);
        assert_eq!(emitter.calls.load(Ordering::Acquire), 1);
        let committed = services
            .notifications
            .list(crate::contracts::ListNotificationHistoryInput {
                origin: crate::contracts::NotificationOriginFilter::All,
                source_app: None,
                unread_only: false,
                limit: 500,
            })
            .unwrap();
        assert_eq!(committed.len(), 1);
        let phases_after_checkpoint = phases.lock().unwrap().clone();
        assert!(
            phases_after_checkpoint
                .iter()
                .position(|phase| *phase == "notificationReadReleased")
                < phases_after_checkpoint
                    .iter()
                    .position(|phase| *phase == "notificationEmit")
        );
        assert!(
            phases_after_checkpoint
                .iter()
                .position(|phase| *phase == "notificationEmit")
                < phases_after_checkpoint
                    .iter()
                    .position(|phase| *phase == "notificationCompletion")
        );
        assert!(
            phases_after_checkpoint
                .iter()
                .position(|phase| *phase == "notificationCompletion")
                < phases_after_checkpoint
                    .iter()
                    .position(|phase| *phase == "checkpoint")
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert_eq!(*phases.lock().unwrap(), phases_after_checkpoint);
        assert_eq!(emitter.calls.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn retired_todo_projector_does_not_reconcile_legacy_rows_at_scheduler_start() {
        let phases: PhaseLog = Default::default();
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let todos = crate::repositories::todos::TodoRepository::new(storage.clone());
        let todo_reminders =
            crate::repositories::todos::TodoReminderRepository::new(storage.clone());
        let todo = todos
            .create(
                CreateTodoInput {
                    title: "retired startup data".into(),
                    description: String::new(),
                    due_at: None,
                    priority: TodoPriority::Normal,
                },
                1,
            )
            .unwrap();
        todo_reminders
            .save(
                SaveTodoReminderInput {
                    id: None,
                    todo_id: todo.id,
                    remind_at: 2,
                    enabled: true,
                    expected_revision: None,
                },
                1,
            )
            .unwrap();
        let services = AppServices::from_parts(
            storage.clone(),
            Arc::new(BootstrapModuleStateProvider),
            Arc::new(FakeShutdownPort {
                phases: phases.clone(),
                stop_entered: None,
                stop_release: None,
            }),
            Arc::new(FakeCheckpointPort {
                phases: phases.clone(),
            }),
            Arc::new(RejectingEmitter),
        );
        let before: i64 = storage
            .with_connection(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM reminder_deliveries", [], |row| {
                        row.get(0)
                    })
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(
            before, 0,
            "legacy source rows remain readable before startup"
        );

        services.start_reminder_worker_once().unwrap();
        services.start_reminder_worker_once().unwrap();
        let delivery_count: i64 = storage
            .with_connection(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM reminder_deliveries", [], |row| {
                        row.get(0)
                    })
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(
            delivery_count, 0,
            "retired todo projector must not produce deliveries"
        );

        services.shutdown().await.unwrap();
        assert_eq!(services.worker_take_count(), 1);
        assert!(phases.lock().unwrap().ends_with(&["checkpoint"]));
    }

    // Break caught: the accepted reminder worker, not an empty registry, must hold shutdown
    // before WAL checkpoint until its externally observable completion boundary releases.
    #[tokio::test]
    async fn reminder_worker_completion_blocks_checkpoint_and_marks_before_it() {
        let phases: PhaseLog = Default::default();
        let services = test_services(phases.clone(), None);
        let completion_gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        services.set_reminder_worker_completion_hook(std::sync::Arc::new({
            let phases = phases.clone();
            let completion_gate = completion_gate.clone();
            move || {
                phases.lock().unwrap().push("reminderCompletion");
                completion_gate.wait();
            }
        }));

        services.start_reminder_worker_once().unwrap();
        services.start_reminder_worker_once().unwrap();
        let shutdown = tauri::async_runtime::spawn({
            let services = services.clone();
            async move { services.shutdown().await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if phases.lock().unwrap().contains(&"reminderCompletion") {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!phases.lock().unwrap().contains(&"checkpoint"));
        completion_gate.wait();
        shutdown.await.unwrap().unwrap();
        let phases = phases.lock().unwrap().clone();
        assert_eq!(
            phases
                .iter()
                .filter(|phase| **phase == "reminderCompletion")
                .count(),
            1
        );
        assert!(
            phases
                .iter()
                .position(|phase| *phase == "reminderCompletion")
                < phases.iter().position(|phase| *phase == "checkpoint")
        );
    }

    #[tokio::test]
    async fn channel_worker_starts_once_rejects_restart_and_completes_before_checkpoint() {
        let phases: PhaseLog = Default::default();
        let services = test_services(phases.clone(), None);
        let completion_gate = Arc::new(std::sync::Barrier::new(2));
        services.set_reminder_channel_worker_completion_hook(Arc::new({
            let phases = phases.clone();
            let completion_gate = completion_gate.clone();
            move || {
                phases.lock().unwrap().push("channelCompletion");
                completion_gate.wait();
            }
        }));

        services.start_reminder_channel_worker_once().unwrap();
        services.start_reminder_channel_worker_once().unwrap();
        let shutdown = tauri::async_runtime::spawn({
            let services = services.clone();
            async move { services.shutdown().await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if phases.lock().unwrap().contains(&"channelCompletion") {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!phases.lock().unwrap().contains(&"checkpoint"));
        completion_gate.wait();
        shutdown.await.unwrap().unwrap();

        let error = services.start_reminder_channel_worker_once().unwrap_err();
        assert_eq!(error.code, AppErrorCode::SourceUnavailable);
        assert_eq!(error.message_key, "errors.serviceStopping");
        assert_eq!(services.worker_take_count(), 1);
        let phases = phases.lock().unwrap().clone();
        assert_eq!(
            phases
                .iter()
                .filter(|phase| **phase == "channelCompletion")
                .count(),
            1
        );
        assert!(
            phases
                .iter()
                .position(|phase| *phase == "channelCompletion")
                < phases.iter().position(|phase| *phase == "checkpoint")
        );
    }

    #[tokio::test]
    async fn shutdown_keeps_activation_routing_until_the_channel_worker_has_exited() {
        struct RecordingActivation(std::sync::atomic::AtomicUsize);
        impl reminder_channels::ToastActivationHandler for RecordingActivation {
            fn activate(&self, _: &str) {
                self.0.fetch_add(1, Ordering::AcqRel);
            }
        }

        let phases: PhaseLog = Default::default();
        let services = test_services(phases.clone(), None);
        services.toast_registration.mark_ready();
        let handler = Arc::new(RecordingActivation(std::sync::atomic::AtomicUsize::new(0)));
        let activation = services.toast_activation_port();
        assert!(activation.install_once(&handler));
        let completion_gate = Arc::new(std::sync::Barrier::new(2));
        services.set_reminder_channel_worker_completion_hook(Arc::new({
            let phases = phases.clone();
            let completion_gate = completion_gate.clone();
            move || {
                phases.lock().unwrap().push("channelCompletionBlocked");
                completion_gate.wait();
            }
        }));
        services.start_reminder_channel_worker_once().unwrap();
        let shutdown = tauri::async_runtime::spawn({
            let services = services.clone();
            async move { services.shutdown().await }
        });

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !phases.lock().unwrap().contains(&"channelCompletionBlocked") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!services.toast_registration.is_ready());
        activation.dispatch_uuid_only(&uuid::Uuid::new_v4().to_string());
        assert_eq!(handler.0.load(Ordering::Acquire), 1);

        completion_gate.wait();
        shutdown.await.unwrap().unwrap();
        activation.dispatch_uuid_only(&uuid::Uuid::new_v4().to_string());
        assert_eq!(handler.0.load(Ordering::Acquire), 1);
    }

    #[test]
    fn bootstrap_provider_excludes_retired_todo_and_media_products() {
        let modules = BootstrapModuleStateProvider.snapshot().unwrap();
        assert_eq!(modules.len(), 4);
        assert!(modules.values().all(|preference| preference.visible));
        assert!(!modules.contains_key(&ModuleId::Todo));
        assert!(!modules.contains_key(&ModuleId::Media));
        assert!(!modules[&ModuleId::Notes].background_enabled);
    }
}
