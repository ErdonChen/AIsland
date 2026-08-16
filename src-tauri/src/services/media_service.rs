use crate::contracts::{
    AppErrorCode, CommandError, MediaControlInput, MediaSnapshot, SafeParameterValue,
    ServiceHealthSnapshot, ServiceHealthState,
};
use crate::domain::media::{
    map_native_snapshot, seconds_to_windows_ticks, unavailable_snapshot, NativeMediaSnapshot,
    NativePlaybackState,
};
use std::sync::{atomic::Ordering, mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const MEDIA_NOTIFICATION_COALESCE: Duration = Duration::from_millis(50);

struct PendingMediaChange {
    first_seen: Instant,
    session_id: Option<String>,
    changed_at: i64,
}

pub trait MediaBackend: Send + Sync {
    fn snapshot(&self, now: i64) -> Result<MediaSnapshot, CommandError>;
    fn control(&self, input: MediaControlInput, now: i64) -> Result<MediaSnapshot, CommandError>;
    fn start_notifications(
        &self,
        changed: Arc<dyn Fn(Option<String>, i64) + Send + Sync>,
    ) -> Result<MediaSubscription, CommandError>;
}

pub trait MediaBackendFactory: Send + Sync {
    fn create(&self) -> Result<Box<dyn MediaBackend>, CommandError>;
}

pub struct MediaSubscription {
    unsubscribe: Option<Box<dyn FnOnce() + Send>>,
}

impl MediaSubscription {
    pub fn new(unsubscribe: impl FnOnce() + Send + 'static) -> Self {
        Self {
            unsubscribe: Some(Box::new(unsubscribe)),
        }
    }

    pub fn unsubscribe(&mut self) {
        if let Some(unsubscribe) = self.unsubscribe.take() {
            unsubscribe();
        }
    }
}

impl Drop for MediaSubscription {
    fn drop(&mut self) {
        self.unsubscribe();
    }
}

pub enum MediaRequest {
    Snapshot {
        now: i64,
        reply: mpsc::Sender<Result<MediaSnapshot, CommandError>>,
    },
    Control {
        input: MediaControlInput,
        now: i64,
        reply: mpsc::Sender<Result<MediaSnapshot, CommandError>>,
    },
    Shutdown,
}

pub trait MediaEventPort: Send + Sync {
    fn changed(&self, session_id: Option<&str>, changed_at: i64) -> Result<(), CommandError>;
    fn health(
        &self,
        healthy: bool,
        reason_code: Option<&str>,
        checked_at: i64,
    ) -> Result<(), CommandError>;
}

pub struct MediaService {
    current_tx: Mutex<Option<mpsc::Sender<MediaRequest>>>,
    current_generation: Mutex<Option<u64>>,
}

impl MediaService {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            current_tx: Mutex::new(None),
            current_generation: Mutex::new(None),
        })
    }

    pub fn start_worker(
        self: &Arc<Self>,
        factory: Arc<dyn MediaBackendFactory>,
        app: tauri::AppHandle,
        generation: u64,
        current_generation: Arc<std::sync::atomic::AtomicU64>,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<MediaWorkerHandle, CommandError> {
        self.start_worker_with_port(
            factory,
            Arc::new(TauriMediaEventPort { app }),
            generation,
            current_generation,
            cancel,
        )
    }

    pub(crate) fn start_worker_with_port(
        self: &Arc<Self>,
        factory: Arc<dyn MediaBackendFactory>,
        events: Arc<dyn MediaEventPort>,
        generation: u64,
        current_generation: Arc<std::sync::atomic::AtomicU64>,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<MediaWorkerHandle, CommandError> {
        if current_generation.load(Ordering::Acquire) != generation {
            return Err(source_error("staleMediaGeneration"));
        }
        let (tx, rx) = mpsc::channel();
        *self.current_tx.lock().expect("media sender lock poisoned") = Some(tx.clone());
        *self
            .current_generation
            .lock()
            .expect("media generation lock poisoned") = Some(generation);
        let service = self.clone();
        let callback_generation = current_generation.clone();
        let pending_change = Arc::new(Mutex::new(None::<PendingMediaChange>));
        let join = std::thread::Builder::new()
            .name(format!("aiceland-media-{generation}"))
            .spawn(move || {
                let backend = factory.create();
                if backend.is_err() && current_generation.load(Ordering::Acquire) == generation {
                    let _ = events.health(false, Some("mediaManagerUnavailable"), now_millis());
                }
                let mut subscription = backend.as_ref().ok().and_then(|backend| {
                    let callback_generation = callback_generation.clone();
                    let pending_change = pending_change.clone();
                    backend
                        .start_notifications(Arc::new(move |session_id, changed_at| {
                            if callback_generation.load(Ordering::Acquire) == generation {
                                let mut pending = pending_change
                                    .lock()
                                    .expect("media notification lock poisoned");
                                if let Some(pending) = pending.as_mut() {
                                    pending.session_id = session_id;
                                    pending.changed_at = pending.changed_at.max(changed_at);
                                } else {
                                    *pending = Some(PendingMediaChange {
                                        first_seen: Instant::now(),
                                        session_id,
                                        changed_at,
                                    });
                                }
                            }
                        }))
                        .ok()
                });
                loop {
                    if *cancel.borrow() || current_generation.load(Ordering::Acquire) != generation
                    {
                        break;
                    }
                    match rx.recv_timeout(Duration::from_millis(20)) {
                        Ok(MediaRequest::Snapshot { now, reply }) => {
                            let result = if let Ok(backend) = backend.as_ref() {
                                let result = backend.snapshot(now);
                                record_media_health(events.as_ref(), &result, now);
                                result
                            } else {
                                let _ = events.health(false, Some("mediaManagerUnavailable"), now);
                                Ok(unavailable_snapshot(now, None, false))
                            };
                            let _ = reply.send(result);
                        }
                        Ok(MediaRequest::Control { input, now, reply }) => {
                            let result = match backend.as_ref() {
                                Ok(backend) => {
                                    let result = backend.control(input, now);
                                    record_media_control_health(events.as_ref(), &result, now);
                                    result
                                }
                                Err(error) => {
                                    let _ =
                                        events.health(false, Some("mediaManagerUnavailable"), now);
                                    Err((*error).clone())
                                }
                            };
                            let _ = reply.send(result);
                        }
                        Ok(MediaRequest::Shutdown) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    let ready_change = {
                        let mut pending = pending_change
                            .lock()
                            .expect("media notification lock poisoned");
                        if pending.as_ref().is_some_and(|change| {
                            change.first_seen.elapsed() >= MEDIA_NOTIFICATION_COALESCE
                        }) {
                            pending.take()
                        } else {
                            None
                        }
                    };
                    if let Some(change) = ready_change {
                        if current_generation.load(Ordering::Acquire) == generation {
                            if let Ok(backend) = backend.as_ref() {
                                let snapshot = backend.snapshot(change.changed_at);
                                record_media_health(events.as_ref(), &snapshot, change.changed_at);
                                if let Ok(snapshot) = snapshot {
                                    let _ = events
                                        .changed(snapshot.session_id.as_deref(), change.changed_at);
                                }
                            }
                        }
                    }
                }
                subscription.as_mut().map(MediaSubscription::unsubscribe);
                let mut installed = service
                    .current_generation
                    .lock()
                    .expect("media generation lock poisoned");
                if *installed == Some(generation) {
                    *installed = None;
                    *service
                        .current_tx
                        .lock()
                        .expect("media sender lock poisoned") = None;
                }
            })
            .map_err(|_| source_error("mediaWorkerSpawnFailed"))?;
        Ok(MediaWorkerHandle {
            generation,
            shutdown: Some(tx),
            join: Some(join),
        })
    }

    pub fn snapshot(&self, now: i64) -> Result<MediaSnapshot, CommandError> {
        let sender = self
            .current_tx
            .lock()
            .expect("media sender lock poisoned")
            .clone();
        let Some(sender) = sender else {
            return Ok(unavailable_snapshot(now, None, false));
        };
        let (reply, response) = mpsc::channel();
        sender
            .send(MediaRequest::Snapshot { now, reply })
            .map_err(|_| source_error("mediaWorkerStopped"))?;
        response
            .recv()
            .map_err(|_| source_error("mediaWorkerStopped"))?
    }

    pub fn control(
        &self,
        input: MediaControlInput,
        now: i64,
    ) -> Result<MediaSnapshot, CommandError> {
        let sender = self
            .current_tx
            .lock()
            .expect("media sender lock poisoned")
            .clone();
        let Some(sender) = sender else {
            return Err(source_error("mediaWorkerStopped"));
        };
        let (reply, response) = mpsc::channel();
        sender
            .send(MediaRequest::Control { input, now, reply })
            .map_err(|_| source_error("mediaWorkerStopped"))?;
        response
            .recv()
            .map_err(|_| source_error("mediaWorkerStopped"))?
    }
}

pub struct MediaWorkerHandle {
    generation: u64,
    shutdown: Option<mpsc::Sender<MediaRequest>>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl MediaWorkerHandle {
    pub fn stop_and_join(mut self) -> Result<(), CommandError> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(MediaRequest::Shutdown);
        }
        if let Some(join) = self.join.take() {
            join.join()
                .map_err(|_| source_error("mediaWorkerJoinFailed"))?;
        }
        Ok(())
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }
}

impl Drop for MediaWorkerHandle {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(MediaRequest::Shutdown);
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

struct TauriMediaEventPort {
    app: tauri::AppHandle,
}

impl MediaEventPort for TauriMediaEventPort {
    fn changed(&self, session_id: Option<&str>, changed_at: i64) -> Result<(), CommandError> {
        crate::events::emit_media_session_changed(&self.app, session_id, changed_at)
    }

    fn health(
        &self,
        healthy: bool,
        reason_code: Option<&str>,
        checked_at: i64,
    ) -> Result<(), CommandError> {
        use tauri::Manager;

        let services = self.app.state::<Arc<crate::services::AppServices>>();
        let (state, message_key, parameters) = if healthy {
            (
                ServiceHealthState::Healthy,
                "services.healthy",
                std::collections::BTreeMap::from([(
                    "serviceId".into(),
                    SafeParameterValue::String("media".into()),
                )]),
            )
        } else {
            (
                ServiceHealthState::Degraded,
                "services.degraded",
                std::collections::BTreeMap::from([
                    (
                        "serviceId".into(),
                        SafeParameterValue::String("media".into()),
                    ),
                    (
                        "reasonCode".into(),
                        SafeParameterValue::String(
                            reason_code.unwrap_or("mediaUnavailable").into(),
                        ),
                    ),
                ]),
            )
        };
        services.health.upsert(&ServiceHealthSnapshot {
            service_id: "media".into(),
            state,
            message_key: message_key.into(),
            parameters,
            checked_at,
        })
    }
}

fn record_media_health(
    events: &dyn MediaEventPort,
    snapshot: &Result<MediaSnapshot, CommandError>,
    checked_at: i64,
) {
    match snapshot {
        Ok(snapshot) if snapshot.can_set_volume => {
            let _ = events.health(true, None, checked_at);
        }
        Ok(_) => {
            let _ = events.health(false, Some("coreAudioUnavailable"), checked_at);
        }
        Err(_) => {
            let _ = events.health(false, Some("mediaSnapshotFailed"), checked_at);
        }
    }
}

fn record_media_control_health(
    events: &dyn MediaEventPort,
    result: &Result<MediaSnapshot, CommandError>,
    checked_at: i64,
) {
    match result {
        Ok(_) => record_media_health(events, result, checked_at),
        Err(error)
            if error.details.get("reasonCode")
                == Some(&SafeParameterValue::String("controlRejected".into())) => {}
        Err(_) => {
            let _ = events.health(false, Some("mediaControlFailed"), checked_at);
        }
    }
}

fn source_error(reason: &str) -> CommandError {
    CommandError::with_detail(
        AppErrorCode::SourceUnavailable,
        "errors.sourceUnavailable",
        "reasonCode",
        SafeParameterValue::String(reason.into()),
        true,
    )
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(windows)]
pub struct WindowsMediaBackendFactory;

#[cfg(windows)]
impl MediaBackendFactory for WindowsMediaBackendFactory {
    fn create(&self) -> Result<Box<dyn MediaBackend>, CommandError> {
        Ok(Box::new(WindowsMediaBackend::new()?))
    }
}

#[cfg(windows)]
struct ComApartment;

#[cfg(windows)]
impl ComApartment {
    fn initialize() -> Result<Self, CommandError> {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|_| source_error("mediaComInitializationFailed"))?;
        Ok(Self)
    }
}

#[cfg(windows)]
impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { windows::Win32::System::Com::CoUninitialize() };
    }
}

#[cfg(windows)]
struct WindowsMediaBackend {
    _apartment: ComApartment,
    manager: windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager,
    notification_callback: Arc<Mutex<Option<Arc<dyn Fn(Option<String>, i64) + Send + Sync>>>>,
    session_registrations: Arc<Mutex<Option<SessionNotificationRegistration>>>,
}

#[cfg(windows)]
impl WindowsMediaBackend {
    fn new() -> Result<Self, CommandError> {
        use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;

        let apartment = ComApartment::initialize()?;
        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
            .and_then(|operation| operation.get())
            .map_err(|_| source_error("mediaManagerUnavailable"))?;
        Ok(Self {
            _apartment: apartment,
            manager,
            notification_callback: Arc::new(Mutex::new(None)),
            session_registrations: Arc::new(Mutex::new(None)),
        })
    }

    fn current_session(
        &self,
    ) -> Option<windows::Media::Control::GlobalSystemMediaTransportControlsSession> {
        self.manager.GetCurrentSession().ok()
    }

    fn volume_state(&self) -> (Option<f64>, bool) {
        let Ok(volume) = core_audio_endpoint() else {
            return (None, false);
        };
        match unsafe { volume.GetMasterVolumeLevelScalar() } {
            Ok(value) => (Some(f64::from(value)), true),
            Err(_) => (None, false),
        }
    }

    fn refresh_session_notifications(&self) -> Result<(), CommandError> {
        let callback = self
            .notification_callback
            .lock()
            .expect("media callback lock poisoned")
            .clone();
        if let Some(callback) = callback {
            replace_session_registrations(&self.manager, &self.session_registrations, callback)?;
        }
        Ok(())
    }
}

#[cfg(windows)]
impl MediaBackend for WindowsMediaBackend {
    fn snapshot(&self, now: i64) -> Result<MediaSnapshot, CommandError> {
        use windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status;

        self.refresh_session_notifications()?;
        let (volume_scalar, can_set_volume) = self.volume_state();
        let Some(session) = self.current_session() else {
            return Ok(unavailable_snapshot(
                now,
                volume_scalar.map(|value| (value.clamp(0.0, 1.0) * 100.0).round() as i64),
                can_set_volume,
            ));
        };
        let session_id = session
            .SourceAppUserModelId()
            .map(|value| value.to_string())
            .map_err(|_| source_error("mediaSessionReadFailed"))?;
        let properties = session
            .TryGetMediaPropertiesAsync()
            .and_then(|operation| operation.get())
            .map_err(|_| source_error("mediaPropertiesReadFailed"))?;
        let playback = session
            .GetPlaybackInfo()
            .map_err(|_| source_error("mediaPlaybackReadFailed"))?;
        let controls = playback
            .Controls()
            .map_err(|_| source_error("mediaControlsReadFailed"))?;
        let timeline = session
            .GetTimelineProperties()
            .map_err(|_| source_error("mediaTimelineReadFailed"))?;
        let playback_state = match playback.PlaybackStatus().ok() {
            Some(Status::Playing) => NativePlaybackState::Playing,
            Some(Status::Paused) => NativePlaybackState::Paused,
            Some(Status::Closed) => NativePlaybackState::Closed,
            _ => NativePlaybackState::Other,
        };
        Ok(map_native_snapshot(
            NativeMediaSnapshot {
                session_id: Some(session_id),
                title: properties
                    .Title()
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
                artist: properties
                    .Artist()
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
                playback_state,
                position_ticks: timeline.Position().map(|value| value.Duration).unwrap_or(0),
                duration_ticks: timeline.EndTime().map(|value| value.Duration).unwrap_or(0),
                volume_scalar,
                can_play: controls.IsPlayEnabled().unwrap_or(false),
                can_pause: controls.IsPauseEnabled().unwrap_or(false),
                can_previous: controls.IsPreviousEnabled().unwrap_or(false),
                can_next: controls.IsNextEnabled().unwrap_or(false),
                can_seek: controls.IsPlaybackPositionEnabled().unwrap_or(false),
                can_set_volume,
            },
            now,
        ))
    }

    fn control(&self, input: MediaControlInput, now: i64) -> Result<MediaSnapshot, CommandError> {
        match input {
            MediaControlInput::SetVolume { volume_percent } => {
                let volume =
                    core_audio_endpoint().map_err(|_| source_error("coreAudioUnavailable"))?;
                unsafe {
                    volume.SetMasterVolumeLevelScalar(
                        (volume_percent / 100.0) as f32,
                        std::ptr::null(),
                    )
                }
                .map_err(|_| source_error("volumeControlFailed"))?;
            }
            command => {
                let Some(session) = self.current_session() else {
                    return Err(source_error("mediaSessionUnavailable"));
                };
                let controls = session
                    .GetPlaybackInfo()
                    .and_then(|info| info.Controls())
                    .map_err(|_| source_error("mediaControlsReadFailed"))?;
                let accepted = match command {
                    MediaControlInput::Play if controls.IsPlayEnabled().unwrap_or(false) => {
                        session.TryPlayAsync().and_then(|operation| operation.get())
                    }
                    MediaControlInput::Pause if controls.IsPauseEnabled().unwrap_or(false) => {
                        session
                            .TryPauseAsync()
                            .and_then(|operation| operation.get())
                    }
                    MediaControlInput::Previous
                        if controls.IsPreviousEnabled().unwrap_or(false) =>
                    {
                        session
                            .TrySkipPreviousAsync()
                            .and_then(|operation| operation.get())
                    }
                    MediaControlInput::Next if controls.IsNextEnabled().unwrap_or(false) => session
                        .TrySkipNextAsync()
                        .and_then(|operation| operation.get()),
                    MediaControlInput::Seek { position_seconds }
                        if controls.IsPlaybackPositionEnabled().unwrap_or(false) =>
                    {
                        let ticks = seconds_to_windows_ticks(position_seconds)
                            .ok_or_else(|| source_error("controlRejected"))?;
                        session
                            .TryChangePlaybackPositionAsync(ticks)
                            .and_then(|operation| operation.get())
                    }
                    _ => return Err(source_error("controlRejected")),
                }
                .map_err(|_| source_error("mediaControlFailed"))?;
                if !accepted {
                    return Err(source_error("controlRejected"));
                }
            }
        }
        self.snapshot(now)
    }

    fn start_notifications(
        &self,
        changed: Arc<dyn Fn(Option<String>, i64) + Send + Sync>,
    ) -> Result<MediaSubscription, CommandError> {
        use windows::Foundation::TypedEventHandler;
        use windows::Media::Control::{
            CurrentSessionChangedEventArgs, GlobalSystemMediaTransportControlsSessionManager,
        };

        *self
            .notification_callback
            .lock()
            .expect("media callback lock poisoned") = Some(changed.clone());
        let manager = self.manager.clone();
        let registrations = self.session_registrations.clone();
        let callback_slot = self.notification_callback.clone();
        let manager_changed = changed.clone();
        let handler = TypedEventHandler::<
            GlobalSystemMediaTransportControlsSessionManager,
            CurrentSessionChangedEventArgs,
        >::new(move |_, _| {
            manager_changed(None, now_millis());
            Ok(())
        });
        let manager_token = match manager.CurrentSessionChanged(&handler) {
            Ok(token) => token,
            Err(_) => {
                *self
                    .notification_callback
                    .lock()
                    .expect("media callback lock poisoned") = None;
                return Err(source_error("mediaNotificationRegistrationFailed"));
            }
        };
        if let Err(error) =
            replace_session_registrations(&self.manager, &self.session_registrations, changed)
        {
            let _ = manager.RemoveCurrentSessionChanged(manager_token);
            *self
                .notification_callback
                .lock()
                .expect("media callback lock poisoned") = None;
            return Err(error);
        }
        Ok(MediaSubscription::new(move || {
            let _ = manager.RemoveCurrentSessionChanged(manager_token);
            *callback_slot.lock().expect("media callback lock poisoned") = None;
            if let Some(mut registration) = registrations
                .lock()
                .expect("media registration lock poisoned")
                .take()
            {
                registration.unsubscribe();
            }
        }))
    }
}

#[cfg(windows)]
fn core_audio_endpoint(
) -> windows::core::Result<windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume> {
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{
        eMultimedia, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
        device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
    }
}

#[cfg(windows)]
struct SessionNotificationRegistration {
    session: windows::Media::Control::GlobalSystemMediaTransportControlsSession,
    media_token: i64,
    playback_token: i64,
    timeline_token: i64,
}

#[cfg(windows)]
impl SessionNotificationRegistration {
    fn unsubscribe(&mut self) {
        let _ = self.session.RemoveMediaPropertiesChanged(self.media_token);
        let _ = self.session.RemovePlaybackInfoChanged(self.playback_token);
        let _ = self
            .session
            .RemoveTimelinePropertiesChanged(self.timeline_token);
    }
}

#[cfg(windows)]
fn replace_session_registrations(
    manager: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager,
    registrations: &Arc<Mutex<Option<SessionNotificationRegistration>>>,
    changed: Arc<dyn Fn(Option<String>, i64) + Send + Sync>,
) -> Result<(), CommandError> {
    use windows::Foundation::TypedEventHandler;
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession, MediaPropertiesChangedEventArgs,
        PlaybackInfoChangedEventArgs, TimelinePropertiesChangedEventArgs,
    };

    let next = if let Ok(session) = manager.GetCurrentSession() {
        let session_id = session
            .SourceAppUserModelId()
            .ok()
            .map(|value| value.to_string());
        if registrations
            .lock()
            .expect("media registration lock poisoned")
            .as_ref()
            .is_some_and(|registration| registration.session == session)
        {
            return Ok(());
        }
        let media_changed = changed.clone();
        let media_id = session_id.clone();
        let media_handler = TypedEventHandler::<
            GlobalSystemMediaTransportControlsSession,
            MediaPropertiesChangedEventArgs,
        >::new(move |_, _| {
            media_changed(media_id.clone(), now_millis());
            Ok(())
        });
        let media_token = session
            .MediaPropertiesChanged(&media_handler)
            .map_err(|_| source_error("mediaNotificationRegistrationFailed"))?;

        let playback_changed = changed.clone();
        let playback_id = session_id.clone();
        let playback_handler = TypedEventHandler::<
            GlobalSystemMediaTransportControlsSession,
            PlaybackInfoChangedEventArgs,
        >::new(move |_, _| {
            playback_changed(playback_id.clone(), now_millis());
            Ok(())
        });
        let playback_token = match session.PlaybackInfoChanged(&playback_handler) {
            Ok(token) => token,
            Err(_) => {
                let _ = session.RemoveMediaPropertiesChanged(media_token);
                return Err(source_error("mediaNotificationRegistrationFailed"));
            }
        };

        let timeline_id = session_id;
        let timeline_handler = TypedEventHandler::<
            GlobalSystemMediaTransportControlsSession,
            TimelinePropertiesChangedEventArgs,
        >::new(move |_, _| {
            changed(timeline_id.clone(), now_millis());
            Ok(())
        });
        let timeline_token = match session.TimelinePropertiesChanged(&timeline_handler) {
            Ok(token) => token,
            Err(_) => {
                let _ = session.RemoveMediaPropertiesChanged(media_token);
                let _ = session.RemovePlaybackInfoChanged(playback_token);
                return Err(source_error("mediaNotificationRegistrationFailed"));
            }
        };
        Some(SessionNotificationRegistration {
            session,
            media_token,
            playback_token,
            timeline_token,
        })
    } else {
        None
    };

    let mut installed = registrations
        .lock()
        .expect("media registration lock poisoned");
    if let Some(mut previous) = installed.take() {
        previous.unsubscribe();
    }
    *installed = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{MediaControlInput, MediaPlaybackState};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    struct FakeBackend;

    impl MediaBackend for FakeBackend {
        fn snapshot(&self, now: i64) -> Result<MediaSnapshot, CommandError> {
            Ok(MediaSnapshot {
                session_id: Some("fake.session".into()),
                title: "Fake".into(),
                artist: "Artist".into(),
                playback_state: MediaPlaybackState::Paused,
                position_seconds: 4,
                duration_seconds: Some(8),
                volume_percent: Some(50),
                can_play: true,
                can_pause: false,
                can_previous: true,
                can_next: true,
                can_seek: true,
                can_set_volume: true,
                updated_at: now,
            })
        }

        fn control(
            &self,
            _input: MediaControlInput,
            now: i64,
        ) -> Result<MediaSnapshot, CommandError> {
            self.snapshot(now)
        }

        fn start_notifications(
            &self,
            _changed: Arc<dyn Fn(Option<String>, i64) + Send + Sync>,
        ) -> Result<MediaSubscription, CommandError> {
            Ok(MediaSubscription::new(|| {}))
        }
    }

    struct FakeFactory;
    impl MediaBackendFactory for FakeFactory {
        fn create(&self) -> Result<Box<dyn MediaBackend>, CommandError> {
            Ok(Box::new(FakeBackend))
        }
    }

    struct RecordingEvents(AtomicU64);
    impl MediaEventPort for RecordingEvents {
        fn changed(&self, _session_id: Option<&str>, _changed_at: i64) -> Result<(), CommandError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn health(
            &self,
            _healthy: bool,
            _reason_code: Option<&str>,
            _checked_at: i64,
        ) -> Result<(), CommandError> {
            Ok(())
        }
    }

    type NotificationCallback = Arc<dyn Fn(Option<String>, i64) + Send + Sync>;

    struct NotifyingBackend {
        callback: Arc<Mutex<Option<NotificationCallback>>>,
        snapshots: Arc<AtomicU64>,
    }

    impl MediaBackend for NotifyingBackend {
        fn snapshot(&self, now: i64) -> Result<MediaSnapshot, CommandError> {
            self.snapshots.fetch_add(1, Ordering::SeqCst);
            FakeBackend.snapshot(now)
        }

        fn control(
            &self,
            input: MediaControlInput,
            now: i64,
        ) -> Result<MediaSnapshot, CommandError> {
            FakeBackend.control(input, now)
        }

        fn start_notifications(
            &self,
            changed: NotificationCallback,
        ) -> Result<MediaSubscription, CommandError> {
            *self.callback.lock().expect("callback lock poisoned") = Some(changed);
            Ok(MediaSubscription::new(|| {}))
        }
    }

    struct NotifyingFactory {
        callback: Arc<Mutex<Option<NotificationCallback>>>,
        snapshots: Arc<AtomicU64>,
    }

    struct FailingFactory;

    impl MediaBackendFactory for FailingFactory {
        fn create(&self) -> Result<Box<dyn MediaBackend>, CommandError> {
            Err(source_error("mediaManagerUnavailable"))
        }
    }

    struct RecordingControlBackend {
        controls: Arc<Mutex<Vec<MediaControlInput>>>,
    }

    impl MediaBackend for RecordingControlBackend {
        fn snapshot(&self, now: i64) -> Result<MediaSnapshot, CommandError> {
            FakeBackend.snapshot(now)
        }

        fn control(
            &self,
            input: MediaControlInput,
            now: i64,
        ) -> Result<MediaSnapshot, CommandError> {
            self.controls
                .lock()
                .expect("control lock poisoned")
                .push(input);
            FakeBackend.snapshot(now)
        }

        fn start_notifications(
            &self,
            _changed: NotificationCallback,
        ) -> Result<MediaSubscription, CommandError> {
            Ok(MediaSubscription::new(|| {}))
        }
    }

    struct RecordingControlFactory(Arc<Mutex<Vec<MediaControlInput>>>);

    impl MediaBackendFactory for RecordingControlFactory {
        fn create(&self) -> Result<Box<dyn MediaBackend>, CommandError> {
            Ok(Box::new(RecordingControlBackend {
                controls: self.0.clone(),
            }))
        }
    }

    struct RejectingControlBackend;

    impl MediaBackend for RejectingControlBackend {
        fn snapshot(&self, now: i64) -> Result<MediaSnapshot, CommandError> {
            FakeBackend.snapshot(now)
        }

        fn control(
            &self,
            _input: MediaControlInput,
            _now: i64,
        ) -> Result<MediaSnapshot, CommandError> {
            Err(source_error("controlRejected"))
        }

        fn start_notifications(
            &self,
            _changed: NotificationCallback,
        ) -> Result<MediaSubscription, CommandError> {
            Ok(MediaSubscription::new(|| {}))
        }
    }

    struct RejectingControlFactory;

    impl MediaBackendFactory for RejectingControlFactory {
        fn create(&self) -> Result<Box<dyn MediaBackend>, CommandError> {
            Ok(Box::new(RejectingControlBackend))
        }
    }

    #[derive(Default)]
    struct DetailedEvents {
        health: Mutex<Vec<(bool, Option<String>)>>,
    }

    impl MediaEventPort for DetailedEvents {
        fn changed(&self, _session_id: Option<&str>, _changed_at: i64) -> Result<(), CommandError> {
            Ok(())
        }

        fn health(
            &self,
            healthy: bool,
            reason_code: Option<&str>,
            _checked_at: i64,
        ) -> Result<(), CommandError> {
            self.health
                .lock()
                .expect("health lock poisoned")
                .push((healthy, reason_code.map(str::to_owned)));
            Ok(())
        }
    }

    impl MediaBackendFactory for NotifyingFactory {
        fn create(&self) -> Result<Box<dyn MediaBackend>, CommandError> {
            Ok(Box::new(NotifyingBackend {
                callback: self.callback.clone(),
                snapshots: self.snapshots.clone(),
            }))
        }
    }

    #[test]
    fn restartable_worker_routes_snapshot_control_and_shutdown() {
        let service = MediaService::new();
        let generation = Arc::new(AtomicU64::new(1));
        let (_cancel_tx, cancel) = tokio::sync::watch::channel(false);
        let worker = service
            .start_worker_with_port(
                Arc::new(FakeFactory),
                Arc::new(RecordingEvents(AtomicU64::new(0))),
                1,
                generation,
                cancel,
            )
            .expect("media worker should start");
        assert_eq!(worker.generation(), 1);
        assert_eq!(
            service.snapshot(42).unwrap().session_id.as_deref(),
            Some("fake.session")
        );
        assert_eq!(
            service
                .control(MediaControlInput::Play, 43)
                .unwrap()
                .updated_at,
            43
        );
        worker.stop_and_join().unwrap();
        assert_eq!(
            service.snapshot(44).unwrap().playback_state,
            MediaPlaybackState::Unavailable
        );
    }

    #[test]
    fn stale_generation_cannot_install_over_the_current_worker() {
        let service = MediaService::new();
        let generation = Arc::new(AtomicU64::new(2));
        let (_cancel_tx, cancel) = tokio::sync::watch::channel(false);
        let result = service.start_worker_with_port(
            Arc::new(FakeFactory),
            Arc::new(RecordingEvents(AtomicU64::new(0))),
            1,
            generation,
            cancel,
        );
        assert!(result.is_err());
        assert_eq!(
            service.snapshot(45).unwrap().playback_state,
            MediaPlaybackState::Unavailable
        );
    }

    #[test]
    fn notification_burst_waits_fifty_millis_then_emits_once_after_a_fresh_snapshot() {
        let service = MediaService::new();
        let generation = Arc::new(AtomicU64::new(1));
        let (_cancel_tx, cancel) = tokio::sync::watch::channel(false);
        let callback = Arc::new(Mutex::new(None));
        let snapshots = Arc::new(AtomicU64::new(0));
        let events = Arc::new(RecordingEvents(AtomicU64::new(0)));
        let worker = service
            .start_worker_with_port(
                Arc::new(NotifyingFactory {
                    callback: callback.clone(),
                    snapshots: snapshots.clone(),
                }),
                events.clone(),
                1,
                generation,
                cancel,
            )
            .expect("media worker should start");

        let started = Instant::now();
        let callback = loop {
            if let Some(callback) = callback.lock().expect("callback lock poisoned").clone() {
                break callback;
            }
            assert!(started.elapsed() < Duration::from_secs(1));
            std::thread::sleep(Duration::from_millis(5));
        };
        callback(Some("first.session".into()), 100);
        callback(Some("latest.session".into()), 101);
        assert_eq!(
            events.0.load(Ordering::SeqCst),
            0,
            "burst must not emit immediately"
        );
        std::thread::sleep(Duration::from_millis(90));
        assert_eq!(events.0.load(Ordering::SeqCst), 1);
        assert_eq!(
            snapshots.load(Ordering::SeqCst),
            1,
            "event requires one fresh snapshot"
        );

        worker.stop_and_join().unwrap();
    }

    #[test]
    fn manager_acquisition_failure_returns_exact_unavailable_and_keeps_media_health_degraded() {
        let service = MediaService::new();
        let generation = Arc::new(AtomicU64::new(1));
        let (_cancel_tx, cancel) = tokio::sync::watch::channel(false);
        let events = Arc::new(DetailedEvents::default());
        let worker = service
            .start_worker_with_port(
                Arc::new(FailingFactory),
                events.clone(),
                1,
                generation,
                cancel,
            )
            .expect("worker itself should start");

        assert_eq!(
            service.snapshot(88).unwrap(),
            unavailable_snapshot(88, None, false)
        );
        let health = events.health.lock().expect("health lock poisoned").clone();
        assert_eq!(
            health.last(),
            Some(&(false, Some("mediaManagerUnavailable".into())))
        );

        worker.stop_and_join().unwrap();
    }

    #[test]
    fn worker_routes_all_six_typed_controls_without_reinterpreting_values() {
        let service = MediaService::new();
        let generation = Arc::new(AtomicU64::new(1));
        let (_cancel_tx, cancel) = tokio::sync::watch::channel(false);
        let controls = Arc::new(Mutex::new(Vec::new()));
        let worker = service
            .start_worker_with_port(
                Arc::new(RecordingControlFactory(controls.clone())),
                Arc::new(RecordingEvents(AtomicU64::new(0))),
                1,
                generation,
                cancel,
            )
            .expect("media worker should start");

        let expected = vec![
            MediaControlInput::Play,
            MediaControlInput::Pause,
            MediaControlInput::Previous,
            MediaControlInput::Next,
            MediaControlInput::Seek {
                position_seconds: 42.5,
            },
            MediaControlInput::SetVolume {
                volume_percent: 35.0,
            },
        ];
        for input in expected.clone() {
            service.control(input, 90).unwrap();
        }
        assert_eq!(*controls.lock().expect("control lock poisoned"), expected);

        worker.stop_and_join().unwrap();
    }

    #[test]
    fn stale_generation_callback_cannot_emit_or_snapshot_after_generation_two_starts() {
        let service = MediaService::new();
        let generation = Arc::new(AtomicU64::new(1));
        let (_cancel_one_tx, cancel_one) = tokio::sync::watch::channel(false);
        let callback_one = Arc::new(Mutex::new(None));
        let snapshots_one = Arc::new(AtomicU64::new(0));
        let events = Arc::new(RecordingEvents(AtomicU64::new(0)));
        let worker_one = service
            .start_worker_with_port(
                Arc::new(NotifyingFactory {
                    callback: callback_one.clone(),
                    snapshots: snapshots_one.clone(),
                }),
                events.clone(),
                1,
                generation.clone(),
                cancel_one,
            )
            .expect("generation one should start");
        let first = wait_for_callback(&callback_one);

        generation.store(2, Ordering::Release);
        let (_cancel_two_tx, cancel_two) = tokio::sync::watch::channel(false);
        let callback_two = Arc::new(Mutex::new(None));
        let snapshots_two = Arc::new(AtomicU64::new(0));
        let worker_two = service
            .start_worker_with_port(
                Arc::new(NotifyingFactory {
                    callback: callback_two.clone(),
                    snapshots: snapshots_two.clone(),
                }),
                events.clone(),
                2,
                generation,
                cancel_two,
            )
            .expect("generation two should start");
        let second = wait_for_callback(&callback_two);

        first(Some("stale.session".into()), 200);
        second(Some("current.session".into()), 201);
        std::thread::sleep(Duration::from_millis(90));
        assert_eq!(events.0.load(Ordering::SeqCst), 1);
        assert_eq!(snapshots_one.load(Ordering::SeqCst), 0);
        assert_eq!(snapshots_two.load(Ordering::SeqCst), 1);

        worker_one.stop_and_join().unwrap();
        worker_two.stop_and_join().unwrap();
    }

    #[test]
    fn unsupported_control_preserves_the_last_snapshot_and_does_not_degrade_media_health() {
        let service = MediaService::new();
        let generation = Arc::new(AtomicU64::new(1));
        let (_cancel_tx, cancel) = tokio::sync::watch::channel(false);
        let events = Arc::new(DetailedEvents::default());
        let worker = service
            .start_worker_with_port(
                Arc::new(RejectingControlFactory),
                events.clone(),
                1,
                generation,
                cancel,
            )
            .expect("media worker should start");

        let before = service.snapshot(300).unwrap();
        let error = service
            .control(
                MediaControlInput::Seek {
                    position_seconds: 42.5,
                },
                301,
            )
            .unwrap_err();
        assert_eq!(error.code, AppErrorCode::SourceUnavailable);
        assert_eq!(
            error.details.get("reasonCode"),
            Some(&SafeParameterValue::String("controlRejected".into())),
        );
        assert_eq!(
            events.health.lock().expect("health lock poisoned").last(),
            Some(&(true, None)),
        );
        let after = service.snapshot(302).unwrap();
        assert_eq!(after.session_id, before.session_id);
        assert_eq!(after.title, before.title);
        assert_eq!(after.playback_state, before.playback_state);

        worker.stop_and_join().unwrap();
    }

    fn wait_for_callback(
        callback: &Arc<Mutex<Option<NotificationCallback>>>,
    ) -> NotificationCallback {
        let started = Instant::now();
        loop {
            if let Some(callback) = callback.lock().expect("callback lock poisoned").clone() {
                return callback;
            }
            assert!(started.elapsed() < Duration::from_secs(1));
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
