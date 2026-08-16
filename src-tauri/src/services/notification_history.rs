use crate::contracts::{
    AppErrorCode, CommandError, DiagnosticEvent, DiagnosticLevel, ListNotificationHistoryInput,
    Locale, NotificationHistoryItem, NotificationOrigin as ContractOrigin, SafeMessageParameters,
    SafeParameterValue, ServiceHealthSnapshot, ServiceHealthState,
};
use crate::events::{notification_history_changed_payload, NOTIFICATION_HISTORY_CHANGED};
use crate::message_catalog::NativeMessageCatalog;
use crate::repositories::{
    diagnostics::DiagnosticsRepository,
    notifications::{
        ImportedNotification, NotificationCursor, NotificationOrigin, NotificationRepository,
    },
    reminders::ReminderRepository,
    service_health::ServiceHealthRepository,
};
use crate::services::wpn_reader::{
    WpnBatch, WpnReader, WpnRowFaultReason, WpnSourceFault, WPN_SOURCE_ID,
};
use crate::services::{EventEmitterPort, TauriEventEmitter};
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
    Arc, Mutex, Weak,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use uuid::Uuid;

pub const AICELAND_REMINDERS_SOURCE_ID: &str = "aicelandReminders";
const WINDOWS_SERVICE_ID: &str = "windowsNotifications";
const AICELAND_SERVICE_ID: &str = "aicelandReminders";
const AICELAND_APP_ID: &str = "com.aiceland";
const BATCH_LIMIT: u32 = 200;
const MAX_BATCHES_PER_WAKE: usize = 10;
const SYNC_INTERVAL: Duration = Duration::from_secs(5);
const SYNC_FAILED_DIAGNOSTIC: &str = "notifications.syncFailed";
const EMIT_FAILED_DIAGNOSTIC: &str = "events.notificationHistoryChangedEmitFailed";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationSyncWake {
    SourceChanged,
    TrailingDrain,
}

pub struct NotificationWorkerDependencies {
    pub wpn: WpnReader,
    pub reminders: ReminderRepository,
    pub health: ServiceHealthRepository,
    pub diagnostics: DiagnosticsRepository,
    pub app: tauri::AppHandle,
}

pub struct NotificationHistoryService {
    notifications: NotificationRepository,
    current_wake_tx: Mutex<Option<(u64, mpsc::Sender<NotificationSyncWake>)>>,
    current_generation: Mutex<Option<Arc<AtomicU64>>>,
    generation_side_effects: Arc<Mutex<()>>,
    windows_schema_blocked: Arc<AtomicBool>,
    windows_retry_at: Arc<AtomicI64>,
}

pub(crate) trait WpnSourcePort: Send + Sync {
    fn read_after(
        &self,
        cursor: NotificationCursor,
        limit: u32,
        received_at: i64,
    ) -> Result<WpnBatch, WpnSourceFault>;
}

impl WpnSourcePort for WpnReader {
    fn read_after(
        &self,
        cursor: NotificationCursor,
        limit: u32,
        received_at: i64,
    ) -> Result<WpnBatch, WpnSourceFault> {
        WpnReader::read_after(self, cursor, limit, received_at)
    }
}

pub struct NotificationWorker {
    wpn: Arc<dyn WpnSourcePort>,
    notifications: NotificationRepository,
    reminders: ReminderRepository,
    health: ServiceHealthRepository,
    diagnostics: DiagnosticsRepository,
    emitter: Arc<dyn EventEmitterPort>,
    wake_rx: mpsc::Receiver<NotificationSyncWake>,
    service: Weak<NotificationHistoryService>,
    generation: u64,
    current_generation: Arc<AtomicU64>,
    generation_side_effects: Arc<Mutex<()>>,
    windows_schema_blocked: Arc<AtomicBool>,
    windows_retry_at: Arc<AtomicI64>,
}

impl NotificationHistoryService {
    pub fn new(notifications: NotificationRepository) -> Arc<Self> {
        Arc::new(Self {
            notifications,
            current_wake_tx: Mutex::new(None),
            current_generation: Mutex::new(None),
            generation_side_effects: Arc::new(Mutex::new(())),
            windows_schema_blocked: Arc::new(AtomicBool::new(false)),
            windows_retry_at: Arc::new(AtomicI64::new(0)),
        })
    }

    pub fn start_worker(
        self: &Arc<Self>,
        dependencies: NotificationWorkerDependencies,
        generation: u64,
        current_generation: Arc<AtomicU64>,
    ) -> NotificationWorker {
        self.start_worker_with_ports(
            Arc::new(dependencies.wpn),
            dependencies.reminders,
            dependencies.health,
            dependencies.diagnostics,
            Arc::new(TauriEventEmitter {
                app: dependencies.app,
            }),
            generation,
            current_generation,
        )
    }

    pub(crate) fn start_worker_with_ports(
        self: &Arc<Self>,
        wpn: Arc<dyn WpnSourcePort>,
        reminders: ReminderRepository,
        health: ServiceHealthRepository,
        diagnostics: DiagnosticsRepository,
        emitter: Arc<dyn EventEmitterPort>,
        generation: u64,
        current_generation: Arc<AtomicU64>,
    ) -> NotificationWorker {
        let (wake_tx, wake_rx) = mpsc::channel(1);
        let current_generation = {
            let _transition = self
                .generation_side_effects
                .lock()
                .expect("notification generation gate poisoned");
            let mut registered_generation = self
                .current_generation
                .lock()
                .expect("notification generation source poisoned");
            let current_generation = registered_generation
                .get_or_insert_with(|| current_generation.clone())
                .clone();
            current_generation.store(generation, Ordering::Release);
            *self
                .current_wake_tx
                .lock()
                .expect("notification wake sender poisoned") = Some((generation, wake_tx));
            current_generation
        };
        NotificationWorker {
            wpn,
            notifications: self.notifications.clone(),
            reminders,
            health,
            diagnostics,
            emitter,
            wake_rx,
            service: Arc::downgrade(self),
            generation,
            current_generation,
            generation_side_effects: self.generation_side_effects.clone(),
            windows_schema_blocked: self.windows_schema_blocked.clone(),
            windows_retry_at: self.windows_retry_at.clone(),
        }
    }

    pub fn wake(&self, wake: NotificationSyncWake) -> Result<(), CommandError> {
        let sender = self
            .current_wake_tx
            .lock()
            .map_err(|_| crate::services::service_stopping_error())?
            .as_ref()
            .map(|(_, sender)| sender.clone())
            .ok_or_else(crate::services::service_stopping_error)?;
        match sender.try_send(wake) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Err(crate::services::service_stopping_error())
            }
        }
    }

    pub fn list(
        &self,
        input: ListNotificationHistoryInput,
        locale: Locale,
    ) -> Result<Vec<NotificationHistoryItem>, CommandError> {
        let language = match locale {
            Locale::ZhCn => "zh-CN",
            Locale::EnUs => "en-US",
        };
        self.notifications
            .list(input)?
            .into_iter()
            .map(|mut item| {
                if item.origin == ContractOrigin::Aiceland {
                    let message_key = item.message_key.as_deref().ok_or_else(database_failure)?;
                    let rendered = NativeMessageCatalog::render(
                        language,
                        message_key,
                        item.message_parameters.clone(),
                    )?;
                    item.title = rendered.clone();
                    item.body = rendered;
                }
                Ok(item)
            })
            .collect()
    }

    fn clear_sender_if_current(&self, generation: u64) {
        let Ok(mut current) = self.current_wake_tx.lock() else {
            return;
        };
        if current
            .as_ref()
            .is_some_and(|(registered_generation, _)| *registered_generation == generation)
        {
            *current = None;
        }
    }
}

impl NotificationWorker {
    pub fn sync_windows_batch(&self, now: i64) -> Result<usize, CommandError> {
        self.sync_windows_batch_page(now).map(|(count, _)| count)
    }

    pub fn sync_aiceland_batch(&self, now: i64) -> Result<usize, CommandError> {
        self.sync_aiceland_batch_page(now).map(|(count, _)| count)
    }

    pub async fn run(mut self, mut cancel: tokio::sync::watch::Receiver<bool>) {
        if *cancel.borrow() {
            if let Some(service) = self.service.upgrade() {
                service.clear_sender_if_current(self.generation);
            }
            return;
        }
        self.drain_sources(unix_now_millis()).await;
        let mut ticker =
            tokio::time::interval_at(tokio::time::Instant::now() + SYNC_INTERVAL, SYNC_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                changed = cancel.changed() => {
                    if changed.is_err() || *cancel.borrow() {
                        break;
                    }
                }
                wake = self.wake_rx.recv() => {
                    if wake.is_none() {
                        break;
                    }
                    self.drain_sources(unix_now_millis()).await;
                }
                _ = ticker.tick() => {
                    self.drain_sources(unix_now_millis()).await;
                }
            }
        }
        if let Some(service) = self.service.upgrade() {
            service.clear_sender_if_current(self.generation);
        }
    }

    async fn drain_sources(&self, now: i64) {
        let mut windows_has_more = false;
        for _ in 0..MAX_BATCHES_PER_WAKE {
            match self.sync_windows_batch_page(now) {
                Ok((_, more)) => {
                    windows_has_more = more;
                    if !more {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let mut aiceland_has_more = false;
        for _ in 0..MAX_BATCHES_PER_WAKE {
            match self.sync_aiceland_batch_page(now) {
                Ok((_, more)) => {
                    aiceland_has_more = more;
                    if !more {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        if windows_has_more || aiceland_has_more {
            if let Some(service) = self.service.upgrade() {
                let _ = service.wake(NotificationSyncWake::TrailingDrain);
            }
            tokio::task::yield_now().await;
        }
    }

    fn sync_windows_batch_page(&self, now: i64) -> Result<(usize, bool), CommandError> {
        if now <= 0 {
            return Err(invalid_input());
        }
        self.ensure_current()?;
        if self.windows_schema_blocked.load(Ordering::Acquire) {
            return Ok((0, false));
        }
        if now < self.windows_retry_at.load(Ordering::Acquire) {
            return Ok((0, false));
        }
        let cursor = self.notifications.cursor(WPN_SOURCE_ID)?;
        let batch = match self.wpn.read_after(cursor.clone(), BATCH_LIMIT, now) {
            Ok(batch) => batch,
            Err(fault) => {
                self.persist_source_fault(WINDOWS_SERVICE_ID, &cursor, &fault, now)?;
                return Err(wpn_fault_error(&fault));
            }
        };
        let row_fault_count = batch.row_faults.len() as i64;
        let row_fault = batch
            .row_faults
            .first()
            .map(|fault| row_fault_reason(fault.reason));
        let cursor_value = batch.cursor.last_row_id;
        let has_more = batch.has_more;
        let _side_effect = self.enter_generation()?;
        let count = self.notifications.import(&batch.items, batch.cursor, now)?;
        self.windows_retry_at.store(0, Ordering::Release);
        self.persist_health(WINDOWS_SERVICE_ID, ServiceHealthState::Healthy, None, now)?;
        if row_fault_count > 0 {
            self.record_diagnostic(
                SYNC_FAILED_DIAGNOSTIC,
                WINDOWS_SERVICE_ID,
                row_fault.unwrap_or("rowRejected"),
                row_fault_count,
                cursor_value,
                now,
            )?;
        }
        if count > 0 {
            self.emit_after_commit(WINDOWS_SERVICE_ID, count as i64, cursor_value, now);
        }
        Ok((count, has_more))
    }

    fn sync_aiceland_batch_page(&self, now: i64) -> Result<(usize, bool), CommandError> {
        if now <= 0 {
            return Err(invalid_input());
        }
        self.ensure_current()?;
        let cursor = self.notifications.cursor(AICELAND_REMINDERS_SOURCE_ID)?;
        let page = match self.reminders.notification_history_page(
            u64::try_from(cursor.last_row_id).map_err(|_| database_failure())?,
            BATCH_LIMIT,
        ) {
            Ok(page) => page,
            Err(error) => {
                let _side_effect = self.enter_generation()?;
                self.persist_health(
                    AICELAND_SERVICE_ID,
                    ServiceHealthState::Degraded,
                    Some("queryFailed"),
                    now,
                )?;
                self.record_diagnostic(
                    SYNC_FAILED_DIAGNOSTIC,
                    AICELAND_SERVICE_ID,
                    "queryFailed",
                    0,
                    cursor.last_row_id,
                    now,
                )?;
                return Err(error);
            }
        };
        let items = page
            .deliveries
            .iter()
            .map(|delivery| ImportedNotification {
                origin: NotificationOrigin::Aiceland,
                app_id: AICELAND_APP_ID.into(),
                source_entity_id: delivery.id.clone(),
                source_row_id: Some(delivery.dispatch_seq),
                title: None,
                body: None,
                message_key: Some(delivery.message_key.clone()),
                message_parameters: Some(delivery.message_parameters.clone()),
                source_context: Some(delivery.source_context.clone()),
                source_occurred_at: delivery.source_occurred_at,
                received_at: now,
            })
            .collect::<Vec<_>>();
        let imported_cursor = NotificationCursor {
            source_id: AICELAND_REMINDERS_SOURCE_ID.into(),
            last_row_id: page.last_dispatch_seq,
            last_updated_at: now,
        };
        let _side_effect = self.enter_generation()?;
        let count = self.notifications.import(&items, imported_cursor, now)?;
        self.persist_health(AICELAND_SERVICE_ID, ServiceHealthState::Healthy, None, now)?;
        if count > 0 {
            self.emit_after_commit(
                AICELAND_SERVICE_ID,
                count as i64,
                page.last_dispatch_seq,
                now,
            );
        }
        Ok((count, page.has_more))
    }

    fn ensure_current(&self) -> Result<(), CommandError> {
        if self.current_generation.load(Ordering::Acquire) == self.generation {
            Ok(())
        } else {
            Err(crate::services::service_stopping_error())
        }
    }

    fn enter_generation(&self) -> Result<std::sync::MutexGuard<'_, ()>, CommandError> {
        let guard = self
            .generation_side_effects
            .lock()
            .map_err(|_| crate::services::service_stopping_error())?;
        self.ensure_current()?;
        Ok(guard)
    }

    fn persist_source_fault(
        &self,
        source: &str,
        cursor: &NotificationCursor,
        fault: &WpnSourceFault,
        now: i64,
    ) -> Result<(), CommandError> {
        let _side_effect = self.enter_generation()?;
        let (state, reason) = match fault {
            WpnSourceFault::Missing => (ServiceHealthState::Offline, "missing"),
            WpnSourceFault::SchemaIncompatible => {
                self.windows_schema_blocked.store(true, Ordering::Release);
                (ServiceHealthState::Blocked, "schemaIncompatible")
            }
            WpnSourceFault::AccessDenied => (ServiceHealthState::Degraded, "accessDenied"),
            WpnSourceFault::Locked => (ServiceHealthState::Degraded, "locked"),
            WpnSourceFault::InvalidInput => (ServiceHealthState::Degraded, "invalidInput"),
            WpnSourceFault::QueryFailed => (ServiceHealthState::Degraded, "queryFailed"),
        };
        if !matches!(fault, WpnSourceFault::SchemaIncompatible) {
            self.windows_retry_at.store(
                now.saturating_add(SYNC_INTERVAL.as_millis() as i64),
                Ordering::Release,
            );
        }
        self.persist_health(source, state, Some(reason), now)?;
        self.record_diagnostic(
            SYNC_FAILED_DIAGNOSTIC,
            source,
            reason,
            0,
            cursor.last_row_id,
            now,
        )
    }

    fn persist_health(
        &self,
        service_id: &str,
        state: ServiceHealthState,
        reason: Option<&str>,
        now: i64,
    ) -> Result<(), CommandError> {
        let mut parameters = BTreeMap::from([(
            "serviceId".into(),
            SafeParameterValue::String(service_id.into()),
        )]);
        if let Some(reason) = reason {
            parameters.insert(
                "reasonCode".into(),
                SafeParameterValue::String(reason.into()),
            );
        }
        let message_key = match state {
            ServiceHealthState::Healthy => "services.healthy",
            ServiceHealthState::Degraded => "services.degraded",
            ServiceHealthState::Blocked => "services.blocked",
            ServiceHealthState::Offline => "services.offline",
        };
        self.health.upsert(&ServiceHealthSnapshot {
            service_id: service_id.into(),
            state,
            message_key: message_key.into(),
            parameters,
            checked_at: now,
        })
    }

    fn emit_after_commit(&self, source: &str, row_count: i64, cursor: i64, now: i64) {
        if self.current_generation.load(Ordering::Acquire) != self.generation {
            return;
        }
        if self
            .emitter
            .emit(
                NOTIFICATION_HISTORY_CHANGED,
                notification_history_changed_payload(
                    now,
                    match source {
                        WINDOWS_SERVICE_ID => "windows",
                        AICELAND_SERVICE_ID => "aiceland",
                        _ => return,
                    },
                ),
            )
            .is_err()
        {
            let _ = self.record_diagnostic(
                EMIT_FAILED_DIAGNOSTIC,
                source,
                "emitFailed",
                row_count,
                cursor,
                now,
            );
        }
    }

    fn record_diagnostic(
        &self,
        code: &str,
        source: &str,
        reason: &str,
        row_count: i64,
        cursor: i64,
        now: i64,
    ) -> Result<(), CommandError> {
        self.diagnostics.record(&DiagnosticEvent {
            id: Uuid::new_v4().to_string(),
            service_id: "notificationHistory".into(),
            level: DiagnosticLevel::Warning,
            code: code.into(),
            parameters: BTreeMap::from([
                ("source".into(), SafeParameterValue::String(source.into())),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String(reason.into()),
                ),
                (
                    "rowCount".into(),
                    SafeParameterValue::Number(row_count.into()),
                ),
                ("cursor".into(), SafeParameterValue::Number(cursor.into())),
                ("checkedAt".into(), SafeParameterValue::Number(now.into())),
            ]),
            created_at: now,
        })
    }
}

fn wpn_fault_error(fault: &WpnSourceFault) -> CommandError {
    let reason = match fault {
        WpnSourceFault::Missing => "missing",
        WpnSourceFault::AccessDenied => "accessDenied",
        WpnSourceFault::Locked => "locked",
        WpnSourceFault::SchemaIncompatible => "schemaIncompatible",
        WpnSourceFault::InvalidInput => "invalidInput",
        WpnSourceFault::QueryFailed => "queryFailed",
    };
    CommandError {
        code: AppErrorCode::SourceUnavailable,
        message_key: "errors.sourceUnavailable".into(),
        details: BTreeMap::from([
            (
                "serviceId".into(),
                SafeParameterValue::String(WINDOWS_SERVICE_ID.into()),
            ),
            (
                "reasonCode".into(),
                SafeParameterValue::String(reason.into()),
            ),
        ]),
        retryable: *fault != WpnSourceFault::SchemaIncompatible,
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

fn database_failure() -> CommandError {
    CommandError {
        code: AppErrorCode::DatabaseFailure,
        message_key: "errors.databaseFailure".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

fn unix_now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(1)
        .max(1)
}

fn row_fault_reason(reason: WpnRowFaultReason) -> &'static str {
    match reason {
        WpnRowFaultReason::PayloadInvalid => "payloadInvalid",
        WpnRowFaultReason::PayloadTooLarge => "payloadTooLarge",
        WpnRowFaultReason::TextTooLarge => "textTooLarge",
        WpnRowFaultReason::ArrivalInvalid => "arrivalInvalid",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{
        BuiltinReminderSoundId, NotificationOriginFilter, ReminderSound, ReminderSourceContext,
        ReminderSourceKind,
    };
    use crate::domain::reminders::{EnqueueOutcome, NewReminderDelivery};
    use crate::storage::Storage;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;

    struct Fixture {
        path: PathBuf,
        notifications: NotificationRepository,
        reminders: ReminderRepository,
        health: ServiceHealthRepository,
        diagnostics: DiagnosticsRepository,
        service: Arc<NotificationHistoryService>,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap().keep();
            let storage = Arc::new(Storage::open(&directory).unwrap());
            let notifications = NotificationRepository::new(storage.clone());
            let reminders = ReminderRepository::new(storage.clone());
            let health = ServiceHealthRepository::new(storage.clone());
            let diagnostics = DiagnosticsRepository::new(storage);
            let service = NotificationHistoryService::new(notifications.clone());
            Self {
                path: directory,
                notifications,
                reminders,
                health,
                diagnostics,
                service,
            }
        }

        fn worker(
            &self,
            source: Arc<dyn WpnSourcePort>,
            emitter: Arc<dyn EventEmitterPort>,
            generation: u64,
            current: Arc<AtomicU64>,
        ) -> NotificationWorker {
            self.service.start_worker_with_ports(
                source,
                self.reminders.clone(),
                self.health.clone(),
                self.diagnostics.clone(),
                emitter,
                generation,
                current,
            )
        }
    }

    struct ScriptedWpn {
        results: Mutex<VecDeque<Result<WpnBatch, WpnSourceFault>>>,
        calls: AtomicUsize,
        cursors: Mutex<Vec<NotificationCursor>>,
        received_at: Mutex<Vec<i64>>,
    }

    impl ScriptedWpn {
        fn new(results: impl IntoIterator<Item = Result<WpnBatch, WpnSourceFault>>) -> Arc<Self> {
            Arc::new(Self {
                results: Mutex::new(results.into_iter().collect()),
                calls: AtomicUsize::new(0),
                cursors: Mutex::new(Vec::new()),
                received_at: Mutex::new(Vec::new()),
            })
        }
    }

    impl WpnSourcePort for ScriptedWpn {
        fn read_after(
            &self,
            cursor: NotificationCursor,
            _limit: u32,
            received_at: i64,
        ) -> Result<WpnBatch, WpnSourceFault> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            self.cursors.lock().unwrap().push(cursor);
            self.received_at.lock().unwrap().push(received_at);
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(empty_wpn_batch()))
        }
    }

    struct BlockingInitialWpn {
        calls: AtomicUsize,
        entered: AtomicBool,
        released: (Mutex<bool>, std::sync::Condvar),
    }

    impl BlockingInitialWpn {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                entered: AtomicBool::new(false),
                released: (Mutex::new(false), std::sync::Condvar::new()),
            })
        }

        fn release(&self) {
            let (released, wake) = &self.released;
            *released.lock().unwrap() = true;
            wake.notify_all();
        }
    }

    impl WpnSourcePort for BlockingInitialWpn {
        fn read_after(
            &self,
            _cursor: NotificationCursor,
            _limit: u32,
            _received_at: i64,
        ) -> Result<WpnBatch, WpnSourceFault> {
            let call = self.calls.fetch_add(1, Ordering::AcqRel);
            if call == 0 {
                self.entered.store(true, Ordering::Release);
                let (released, wake) = &self.released;
                let mut released = released.lock().unwrap();
                while !*released {
                    released = wake.wait(released).unwrap();
                }
                Ok(empty_wpn_batch())
            } else {
                Ok(windows_batch(1, 11, 100))
            }
        }
    }

    #[derive(Default)]
    struct RecordingEmitter {
        notifications: Option<NotificationRepository>,
        events: Mutex<Vec<(String, serde_json::Value, usize)>>,
        fail: AtomicBool,
    }

    impl EventEmitterPort for RecordingEmitter {
        fn emit(
            &self,
            event_name: &'static str,
            payload: serde_json::Value,
        ) -> Result<(), CommandError> {
            let committed_rows = self
                .notifications
                .as_ref()
                .map(|repository| repository.list(all_history()).unwrap().len())
                .unwrap_or_default();
            self.events
                .lock()
                .unwrap()
                .push((event_name.into(), payload, committed_rows));
            if self.fail.load(Ordering::Acquire) {
                Err(crate::services::service_stopping_error())
            } else {
                Ok(())
            }
        }
    }

    fn all_history() -> ListNotificationHistoryInput {
        ListNotificationHistoryInput {
            origin: NotificationOriginFilter::All,
            source_app: None,
            unread_only: false,
            limit: 500,
        }
    }

    fn empty_wpn_batch() -> WpnBatch {
        WpnBatch {
            items: Vec::new(),
            cursor: NotificationCursor {
                source_id: WPN_SOURCE_ID.into(),
                last_row_id: 0,
                last_updated_at: 0,
            },
            has_more: false,
            row_faults: Vec::new(),
        }
    }

    fn windows_batch(row_id: i64, source_occurred_at: i64, received_at: i64) -> WpnBatch {
        WpnBatch {
            items: vec![ImportedNotification {
                origin: NotificationOrigin::Windows,
                app_id: "windows.fixture".into(),
                source_entity_id: format!("wpn:{row_id}"),
                source_row_id: Some(row_id),
                title: Some("Fixture title".into()),
                body: Some("Fixture body".into()),
                message_key: None,
                message_parameters: None,
                source_context: None,
                source_occurred_at,
                received_at,
            }],
            cursor: NotificationCursor {
                source_id: WPN_SOURCE_ID.into(),
                last_row_id: row_id,
                last_updated_at: row_id + 10,
            },
            has_more: false,
            row_faults: Vec::new(),
        }
    }

    fn enqueue_dispatched_todo(reminders: &ReminderRepository) -> String {
        let todo_id = Uuid::new_v4().to_string();
        let request = NewReminderDelivery {
            dedupe_key: format!("notification-history:{todo_id}"),
            rule_id: None,
            source_kind: ReminderSourceKind::Todo,
            source_entity_id: todo_id.clone(),
            message_key: "reminders.todo.due".into(),
            message_parameters: BTreeMap::from([(
                "todoTitle".into(),
                SafeParameterValue::String("Release build".into()),
            )]),
            source_context: ReminderSourceContext::Todo {
                todo_id,
                reminder_revision: 1,
                todo_title: "Release build".into(),
                source_occurred_at: 50,
            },
            source_occurred_at: 50,
            sound: ReminderSound::Builtin {
                sound_id: BuiltinReminderSoundId::SystemNotification,
            },
            toast_enabled: true,
            window_enabled: true,
            due_at: 50,
        };
        let id = match reminders.enqueue(request, 10).unwrap() {
            EnqueueOutcome::Inserted(delivery) | EnqueueOutcome::Duplicate(delivery) => delivery.id,
        };
        assert_eq!(reminders.claim_due(60, 10).unwrap().len(), 1);
        id
    }

    #[test]
    fn windows_rows_and_cursor_commit_before_one_history_hint() {
        let fixture = Fixture::new();
        let source = ScriptedWpn::new([Ok(windows_batch(7, 80, 100))]);
        let emitter = Arc::new(RecordingEmitter {
            notifications: Some(fixture.notifications.clone()),
            ..RecordingEmitter::default()
        });
        let worker = fixture.worker(
            source.clone(),
            emitter.clone(),
            1,
            Arc::new(AtomicU64::new(1)),
        );

        assert_eq!(worker.sync_windows_batch(100).unwrap(), 1);
        assert_eq!(*source.received_at.lock().unwrap(), vec![100]);
        let rows = fixture.notifications.list(all_history()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source_occurred_at, 80);
        assert_eq!(rows[0].received_at, 100);
        for locale in [Locale::ZhCn, Locale::EnUs] {
            let localized = fixture.service.list(all_history(), locale).unwrap();
            assert_eq!(localized[0].title, "Fixture title");
            assert_eq!(localized[0].body, "Fixture body");
            assert_eq!(localized[0].message_key, None);
        }
        assert_eq!(
            fixture.notifications.cursor(WPN_SOURCE_ID).unwrap(),
            NotificationCursor {
                source_id: WPN_SOURCE_ID.into(),
                last_row_id: 7,
                last_updated_at: 17,
            }
        );
        assert_eq!(
            *emitter.events.lock().unwrap(),
            vec![(
                NOTIFICATION_HISTORY_CHANGED.into(),
                serde_json::json!({"newestReceivedAt":100,"origin":"windows"}),
                1,
            )]
        );
    }

    #[test]
    fn reminder_projection_uses_an_independent_cursor_and_renders_each_locale_at_read_time() {
        let fixture = Fixture::new();
        let delivery_id = enqueue_dispatched_todo(&fixture.reminders);
        let worker = fixture.worker(
            ScriptedWpn::new([]),
            Arc::new(RecordingEmitter::default()),
            1,
            Arc::new(AtomicU64::new(1)),
        );

        assert_eq!(worker.sync_aiceland_batch(100).unwrap(), 1);
        assert_eq!(worker.sync_aiceland_batch(101).unwrap(), 0);
        assert_eq!(
            fixture
                .notifications
                .cursor(AICELAND_REMINDERS_SOURCE_ID)
                .unwrap()
                .last_row_id,
            1
        );
        assert_eq!(
            fixture
                .notifications
                .cursor(WPN_SOURCE_ID)
                .unwrap()
                .last_row_id,
            0
        );
        let persisted = fixture.notifications.list(all_history()).unwrap();
        assert_eq!(persisted[0].source_entity_id, delivery_id);
        assert_eq!(persisted[0].title, "");
        assert_eq!(persisted[0].body, "");
        assert_eq!(persisted[0].source_occurred_at, 50);
        assert_eq!(persisted[0].received_at, 100);

        let zh = fixture.service.list(all_history(), Locale::ZhCn).unwrap();
        let en = fixture.service.list(all_history(), Locale::EnUs).unwrap();
        assert_ne!(zh[0].title, en[0].title);
        assert_eq!(zh[0].title, zh[0].body);
        assert_eq!(en[0].title, en[0].body);
        assert_eq!(zh[0].message_key.as_deref(), Some("reminders.todo.due"));
    }

    #[test]
    fn retained_aiceland_history_renders_without_a_retained_reminder_row() {
        let fixture = Fixture::new();
        let delivery_id = Uuid::new_v4().to_string();
        let todo_id = Uuid::new_v4().to_string();
        fixture
            .notifications
            .import(
                &[ImportedNotification {
                    origin: NotificationOrigin::Aiceland,
                    app_id: AICELAND_APP_ID.into(),
                    source_entity_id: delivery_id.clone(),
                    source_row_id: Some(3),
                    title: None,
                    body: None,
                    message_key: Some("reminders.todo.due".into()),
                    message_parameters: Some(BTreeMap::from([(
                        "todoTitle".into(),
                        SafeParameterValue::String("Retained history".into()),
                    )])),
                    source_context: Some(ReminderSourceContext::Todo {
                        todo_id,
                        reminder_revision: 4,
                        todo_title: "Retained history".into(),
                        source_occurred_at: 70,
                    }),
                    source_occurred_at: 70,
                    received_at: 80,
                }],
                NotificationCursor {
                    source_id: AICELAND_REMINDERS_SOURCE_ID.into(),
                    last_row_id: 3,
                    last_updated_at: 80,
                },
                80,
            )
            .unwrap();

        let row = fixture
            .service
            .list(all_history(), Locale::EnUs)
            .unwrap()
            .remove(0);
        assert_eq!(row.source_entity_id, delivery_id);
        assert!(row.body.contains("Retained history"));
        assert_eq!(row.message_key.as_deref(), Some("reminders.todo.due"));
    }

    #[test]
    fn reopened_storage_resumes_from_the_committed_windows_cursor_without_duplicates() {
        let fixture = Fixture::new();
        let first = fixture.worker(
            ScriptedWpn::new([Ok(windows_batch(5, 80, 100))]),
            Arc::new(RecordingEmitter::default()),
            1,
            Arc::new(AtomicU64::new(1)),
        );
        assert_eq!(first.sync_windows_batch(100).unwrap(), 1);

        let reopened_storage = Arc::new(Storage::open(&fixture.path).unwrap());
        let reopened_notifications = NotificationRepository::new(reopened_storage.clone());
        let reopened_service = NotificationHistoryService::new(reopened_notifications.clone());
        let source = ScriptedWpn::new([Ok(WpnBatch {
            cursor: NotificationCursor {
                source_id: WPN_SOURCE_ID.into(),
                last_row_id: 5,
                last_updated_at: 15,
            },
            ..empty_wpn_batch()
        })]);
        let second = reopened_service.start_worker_with_ports(
            source.clone(),
            ReminderRepository::new(reopened_storage.clone()),
            ServiceHealthRepository::new(reopened_storage.clone()),
            DiagnosticsRepository::new(reopened_storage),
            Arc::new(RecordingEmitter::default()),
            2,
            Arc::new(AtomicU64::new(2)),
        );

        assert_eq!(second.sync_windows_batch(200).unwrap(), 0);
        assert_eq!(source.cursors.lock().unwrap()[0].last_row_id, 5);
        assert_eq!(reopened_notifications.list(all_history()).unwrap().len(), 1);
    }

    #[test]
    fn a_fresh_generation_owns_a_fresh_capacity_one_receiver_and_fences_old_results() {
        let fixture = Fixture::new();
        let current = Arc::new(AtomicU64::new(1));
        let mut first = fixture.worker(
            ScriptedWpn::new([Ok(windows_batch(1, 10, 20))]),
            Arc::new(RecordingEmitter::default()),
            1,
            current.clone(),
        );
        fixture
            .service
            .wake(NotificationSyncWake::SourceChanged)
            .unwrap();
        fixture
            .service
            .wake(NotificationSyncWake::SourceChanged)
            .unwrap();
        assert_eq!(
            first.wake_rx.try_recv().unwrap(),
            NotificationSyncWake::SourceChanged
        );
        assert!(first.wake_rx.try_recv().is_err());

        let mut second = fixture.worker(
            ScriptedWpn::new([]),
            Arc::new(RecordingEmitter::default()),
            2,
            Arc::new(AtomicU64::new(2)),
        );
        fixture
            .service
            .wake(NotificationSyncWake::TrailingDrain)
            .unwrap();
        assert_eq!(
            second.wake_rx.try_recv().unwrap(),
            NotificationSyncWake::TrailingDrain
        );
        assert!(first.sync_windows_batch(20).is_err());
        assert!(fixture
            .notifications
            .list(all_history())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn incompatible_windows_schema_blocks_reprobe_but_does_not_block_aiceland_projection() {
        let fixture = Fixture::new();
        enqueue_dispatched_todo(&fixture.reminders);
        let source = ScriptedWpn::new([
            Err(WpnSourceFault::SchemaIncompatible),
            Ok(windows_batch(2, 20, 30)),
        ]);
        let worker = fixture.worker(
            source.clone(),
            Arc::new(RecordingEmitter::default()),
            1,
            Arc::new(AtomicU64::new(1)),
        );

        assert!(worker.sync_windows_batch(30).is_err());
        assert_eq!(worker.sync_windows_batch(35).unwrap(), 0);
        assert_eq!(source.calls.load(Ordering::Acquire), 1);
        assert_eq!(worker.sync_aiceland_batch(40).unwrap(), 1);
        let health = fixture.health.list().unwrap();
        assert!(health.iter().any(|snapshot| {
            snapshot.service_id == WINDOWS_SERVICE_ID
                && snapshot.state == ServiceHealthState::Blocked
        }));
        assert!(fixture
            .notifications
            .list(all_history())
            .unwrap()
            .iter()
            .any(|row| row.origin == ContractOrigin::Aiceland));
    }

    #[test]
    fn transient_windows_fault_retries_only_after_the_next_five_second_boundary() {
        let fixture = Fixture::new();
        let source =
            ScriptedWpn::new([Err(WpnSourceFault::Locked), Ok(windows_batch(2, 20, 5_030))]);
        let worker = fixture.worker(
            source.clone(),
            Arc::new(RecordingEmitter::default()),
            1,
            Arc::new(AtomicU64::new(1)),
        );

        assert!(worker.sync_windows_batch(30).is_err());
        assert_eq!(worker.sync_windows_batch(5_029).unwrap(), 0);
        assert_eq!(source.calls.load(Ordering::Acquire), 1);
        assert_eq!(worker.sync_windows_batch(5_030).unwrap(), 1);
        assert_eq!(source.calls.load(Ordering::Acquire), 2);
    }

    #[test]
    fn event_failure_keeps_committed_rows_and_records_only_safe_batch_metadata() {
        let fixture = Fixture::new();
        let emitter = Arc::new(RecordingEmitter {
            notifications: Some(fixture.notifications.clone()),
            fail: AtomicBool::new(true),
            ..RecordingEmitter::default()
        });
        let worker = fixture.worker(
            ScriptedWpn::new([Ok(windows_batch(9, 80, 100))]),
            emitter,
            1,
            Arc::new(AtomicU64::new(1)),
        );

        assert_eq!(worker.sync_windows_batch(100).unwrap(), 1);
        assert_eq!(fixture.notifications.list(all_history()).unwrap().len(), 1);
        let diagnostics = fixture.diagnostics.list(10).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, EMIT_FAILED_DIAGNOSTIC);
        assert_eq!(
            diagnostics[0]
                .parameters
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["checkedAt", "cursor", "reasonCode", "rowCount", "source"]
        );
        let serialized = serde_json::to_string(&diagnostics).unwrap();
        assert!(!serialized.contains("Fixture title"));
        assert!(!serialized.contains("Fixture body"));
    }

    #[tokio::test]
    async fn cancellation_clears_only_its_own_generation_sender() {
        let fixture = Fixture::new();
        let current = Arc::new(AtomicU64::new(1));
        let first = fixture.worker(
            ScriptedWpn::new([]),
            Arc::new(RecordingEmitter::default()),
            1,
            current.clone(),
        );
        let second = fixture.worker(
            ScriptedWpn::new([]),
            Arc::new(RecordingEmitter::default()),
            2,
            current,
        );
        let (_cancel_tx, cancelled) = tokio::sync::watch::channel(true);

        first.run(cancelled).await;

        fixture
            .service
            .wake(NotificationSyncWake::SourceChanged)
            .unwrap();
        drop(second);
    }

    #[tokio::test(start_paused = true)]
    async fn worker_drains_immediately_then_only_on_each_five_second_boundary() {
        let fixture = Fixture::new();
        let source = ScriptedWpn::new([]);
        let worker = fixture.worker(
            source.clone(),
            Arc::new(RecordingEmitter::default()),
            1,
            Arc::new(AtomicU64::new(1)),
        );
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let join = tokio::spawn(worker.run(cancel_rx));

        while source.calls.load(Ordering::Acquire) < 1 {
            tokio::task::yield_now().await;
        }
        assert_eq!(source.calls.load(Ordering::Acquire), 1);

        tokio::time::advance(Duration::from_millis(4_999)).await;
        tokio::task::yield_now().await;
        assert_eq!(source.calls.load(Ordering::Acquire), 1);

        tokio::time::advance(Duration::from_millis(1)).await;
        while source.calls.load(Ordering::Acquire) < 2 {
            tokio::task::yield_now().await;
        }
        assert_eq!(source.calls.load(Ordering::Acquire), 2);

        cancel_tx.send_replace(true);
        join.await.unwrap();
    }

    #[tokio::test]
    async fn a_full_dirty_channel_still_drains_the_trailing_batch() {
        let fixture = Fixture::new();
        let batches = (1..=11)
            .map(|row| {
                let mut batch = windows_batch(row, row + 10, 100);
                batch.has_more = row < 11;
                Ok(batch)
            })
            .collect::<Vec<_>>();
        let source = ScriptedWpn::new(batches);
        let worker = fixture.worker(
            source.clone(),
            Arc::new(RecordingEmitter::default()),
            1,
            Arc::new(AtomicU64::new(1)),
        );
        fixture
            .service
            .wake(NotificationSyncWake::SourceChanged)
            .unwrap();
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let join = tokio::spawn(worker.run(cancel_rx));

        tokio::time::timeout(Duration::from_secs(1), async {
            while fixture
                .notifications
                .cursor(WPN_SOURCE_ID)
                .unwrap()
                .last_row_id
                < 11
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(source.calls.load(Ordering::Acquire), 11);
        cancel_tx.send_replace(true);
        join.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_wins_when_a_dirty_wake_is_ready_after_the_initial_drain() {
        for iteration in 0..32 {
            let cancelled_fixture = Fixture::new();
            let cancelled_source = BlockingInitialWpn::new();
            let cancelled_emitter = Arc::new(RecordingEmitter::default());
            let cancelled_worker = cancelled_fixture.worker(
                cancelled_source.clone(),
                cancelled_emitter.clone(),
                1,
                Arc::new(AtomicU64::new(1)),
            );
            cancelled_fixture
                .service
                .wake(NotificationSyncWake::SourceChanged)
                .unwrap();
            let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
            let join = tokio::spawn(cancelled_worker.run(cancel_rx));

            tokio::time::timeout(Duration::from_secs(1), async {
                while !cancelled_source.entered.load(Ordering::Acquire) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            cancel_tx.send_replace(true);
            cancelled_source.release();
            join.await.unwrap();

            assert_eq!(
                cancelled_source.calls.load(Ordering::Acquire),
                1,
                "dirty wake won over cancellation on iteration {iteration}"
            );
            assert!(cancelled_emitter.events.lock().unwrap().is_empty());
            assert!(cancelled_fixture
                .notifications
                .list(all_history())
                .unwrap()
                .is_empty());
        }
    }

    #[tokio::test]
    async fn one_wake_drains_at_most_ten_source_batches_and_queues_one_trailing_wake() {
        let fixture = Fixture::new();
        let batches = (1..=11)
            .map(|row| {
                let mut batch = windows_batch(row, row + 10, 100);
                batch.has_more = true;
                Ok(batch)
            })
            .collect::<Vec<_>>();
        let source = ScriptedWpn::new(batches);
        let mut worker = fixture.worker(
            source.clone(),
            Arc::new(RecordingEmitter::default()),
            1,
            Arc::new(AtomicU64::new(1)),
        );

        worker.drain_sources(100).await;

        assert_eq!(source.calls.load(Ordering::Acquire), 10);
        assert_eq!(
            worker.wake_rx.try_recv().unwrap(),
            NotificationSyncWake::TrailingDrain
        );
        assert_eq!(
            fixture
                .notifications
                .cursor(WPN_SOURCE_ID)
                .unwrap()
                .last_row_id,
            10
        );
    }
}
