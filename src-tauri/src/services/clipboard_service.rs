use super::clipboard_assets::ClipboardAssetStore;
use super::clipboard_listener::{
    read_with_locked_retry, ClipboardReadOutcome, ClipboardRetrySleeper, ClipboardSourceFactory,
    SystemClipboardRetrySleeper,
};
use super::EventEmitterPort;
use crate::contracts::{
    ClearResult, ClipboardAssetMimeType, ClipboardAssetPayload, ClipboardItem, CommandError,
    DeleteResult, DiagnosticEvent, DiagnosticLevel, ListClipboardItemsInput, SafeParameterValue,
    ServiceHealthSnapshot, ServiceHealthState,
};
use crate::domain::clipboard::{
    validate_image_capture, validate_text_capture, CaptureOutcome, CapturedClipboardContent,
    NewClipboardAsset,
};
use crate::events::{clipboard_changed_payload, CLIPBOARD_CHANGED};
use crate::repositories::clipboard::ClipboardRepository;
use crate::repositories::diagnostics::DiagnosticsRepository;
use crate::repositories::service_health::ServiceHealthRepository;
use std::collections::BTreeSet;
use std::sync::atomic::Ordering;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use uuid::Uuid;

trait ClipboardAssetPort: Send + Sync {
    fn write_png_atomic(&self, asset_id: Uuid, bytes: &[u8]) -> Result<String, CommandError>;
    fn read_owned_bounded(
        &self,
        asset_name: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, CommandError>;
    fn delete_owned(&self, asset_name: &str) -> Result<(), CommandError>;
    fn remove_orphans(&self, referenced_names: &BTreeSet<String>) -> Result<u64, CommandError>;
}

impl ClipboardAssetPort for ClipboardAssetStore {
    fn write_png_atomic(&self, asset_id: Uuid, bytes: &[u8]) -> Result<String, CommandError> {
        self.write_png_atomic(asset_id, bytes)
    }

    fn read_owned_bounded(
        &self,
        asset_name: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, CommandError> {
        self.read_owned_bounded(asset_name, max_bytes)
    }

    fn delete_owned(&self, asset_name: &str) -> Result<(), CommandError> {
        self.delete_owned(asset_name)
    }

    fn remove_orphans(&self, referenced_names: &BTreeSet<String>) -> Result<u64, CommandError> {
        self.remove_orphans(referenced_names)
    }
}

pub struct ClipboardService {
    repository: ClipboardRepository,
    assets: Arc<dyn ClipboardAssetPort>,
    capture_lock: Mutex<()>,
    source_factory: Arc<dyn ClipboardSourceFactory>,
    health: ServiceHealthRepository,
    diagnostics: DiagnosticsRepository,
    emitter: Arc<dyn EventEmitterPort>,
    sleeper: Arc<dyn ClipboardRetrySleeper>,
}

impl ClipboardService {
    pub fn new(
        repository: ClipboardRepository,
        assets: ClipboardAssetStore,
        source_factory: Arc<dyn ClipboardSourceFactory>,
        health: ServiceHealthRepository,
        diagnostics: DiagnosticsRepository,
        emitter: Arc<dyn EventEmitterPort>,
    ) -> Self {
        Self::new_with_asset_port(
            repository,
            Arc::new(assets),
            source_factory,
            health,
            diagnostics,
            emitter,
        )
    }

    fn new_with_asset_port(
        repository: ClipboardRepository,
        assets: Arc<dyn ClipboardAssetPort>,
        source_factory: Arc<dyn ClipboardSourceFactory>,
        health: ServiceHealthRepository,
        diagnostics: DiagnosticsRepository,
        emitter: Arc<dyn EventEmitterPort>,
    ) -> Self {
        Self {
            repository,
            assets,
            capture_lock: Mutex::new(()),
            source_factory,
            health,
            diagnostics,
            emitter,
            sleeper: Arc::new(SystemClipboardRetrySleeper),
        }
    }

    #[cfg(test)]
    fn with_sleeper(mut self, sleeper: Arc<dyn ClipboardRetrySleeper>) -> Self {
        self.sleeper = sleeper;
        self
    }

    pub fn capture(
        &self,
        source_app: Option<String>,
        content: CapturedClipboardContent,
        now: i64,
    ) -> Result<CaptureOutcome, CommandError> {
        let _capture_guard = self
            .capture_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.recover_orphan_assets(now);
        match content {
            CapturedClipboardContent::Text { text, .. } => {
                let (sha256, _) = validate_text_capture(&text)?;
                let outcome = self.repository.insert_text(
                    Uuid::new_v4(),
                    &text,
                    &sha256,
                    source_app.as_deref(),
                    now,
                )?;
                self.cleanup_assets(&outcome.removed_asset_names, now);
                Ok(outcome)
            }
            CapturedClipboardContent::Image {
                png, width, height, ..
            } => {
                let rgba_bytes = usize::try_from(width)
                    .ok()
                    .and_then(|width| {
                        usize::try_from(height)
                            .ok()
                            .and_then(|height| width.checked_mul(height))
                    })
                    .and_then(|pixels| pixels.checked_mul(4))
                    .ok_or_else(invalid_input)?;
                let (sha256, byte_size) = validate_image_capture(width, height, rgba_bytes, &png)?;
                let asset_id = Uuid::new_v4();
                let asset_name = self.assets.write_png_atomic(asset_id, &png)?;
                let outcome = self.repository.insert_image_metadata(
                    Uuid::new_v4(),
                    NewClipboardAsset {
                        id: asset_id,
                        asset_name: asset_name.clone(),
                        sha256,
                        width,
                        height,
                        byte_size,
                    },
                    source_app.as_deref(),
                    now,
                );
                let outcome = match outcome {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        if self.assets.delete_owned(&asset_name).is_err() {
                            self.record_asset_cleanup_failure("compensationDeleteFailed", now);
                        }
                        return Err(error);
                    }
                };
                self.cleanup_assets(&outcome.removed_asset_names, now);
                Ok(outcome)
            }
        }
    }

    #[cfg(windows)]
    pub fn start_worker(
        self: &Arc<Self>,
        _app: tauri::AppHandle,
        generation: u64,
        current_generation: Arc<AtomicU64>,
        cancel: watch::Receiver<bool>,
    ) -> Result<super::clipboard_listener::ClipboardListenerHandle, CommandError> {
        let service = Arc::clone(self);
        let source_factory = self.source_factory.clone();
        let callback_generation = current_generation.clone();
        let handle = super::clipboard_listener::start_message_listener(move |cancelled| {
            {
                let _capture_guard = service
                    .capture_lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                service.recover_orphan_assets(super::now_millis());
            }
            let mut source = source_factory.open()?;
            Ok(Box::new(move || {
                let _ = service.process_source_notification(
                    source.as_mut(),
                    generation,
                    callback_generation.as_ref(),
                    cancelled.as_ref(),
                    super::clipboard_listener::foreground_process_basename(),
                    super::now_millis(),
                );
            }))
        })?;
        let stop_signal = handle.stop_signal();
        spawn_cancellation_watcher(cancel, stop_signal);
        Ok(handle)
    }

    pub fn copy_item(&self, id: Uuid) -> Result<ClipboardItem, CommandError> {
        let item = self.repository.get(id)?;
        if let Some(text) = &item.text_content {
            let mut source = self.source_factory.open()?;
            source.write_text(text)?;
        } else {
            let asset_id = item
                .asset_id
                .as_deref()
                .and_then(|value| Uuid::parse_str(value).ok())
                .ok_or_else(invalid_input)?;
            let png = self.read_validated_asset(asset_id)?;
            let mut source = self.source_factory.open()?;
            source.write_png(&png)?;
        }
        Ok(item)
    }

    pub fn list_items(
        &self,
        input: ListClipboardItemsInput,
    ) -> Result<Vec<ClipboardItem>, CommandError> {
        self.repository.list(input)
    }

    pub fn set_pinned(
        &self,
        id: Uuid,
        pinned: bool,
        changed_at: i64,
    ) -> Result<ClipboardItem, CommandError> {
        let item = self.repository.set_pinned(id, pinned, changed_at)?;
        self.emit_or_record(&item.id, changed_at);
        Ok(item)
    }

    pub fn delete_item(&self, id: Uuid, changed_at: i64) -> Result<DeleteResult, CommandError> {
        let _capture_guard = self
            .capture_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (result, asset_name) = self.repository.delete(id)?;
        self.emit_or_record(&result.id, changed_at);
        if let Some(asset_name) = asset_name {
            self.cleanup_assets(&[asset_name], changed_at);
        }
        Ok(result)
    }

    pub fn clear_history(
        &self,
        keep_pinned: bool,
        changed_at: i64,
    ) -> Result<ClearResult, CommandError> {
        let _capture_guard = self
            .capture_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (result, asset_names) = self.repository.clear(keep_pinned)?;
        self.emit_or_record(&Uuid::new_v4().to_string(), changed_at);
        self.cleanup_assets(&asset_names, changed_at);
        Ok(result)
    }

    pub fn asset_payload(&self, asset_id: Uuid) -> Result<ClipboardAssetPayload, CommandError> {
        use base64::Engine;
        let bytes = self.read_validated_asset(asset_id)?;
        Ok(ClipboardAssetPayload {
            asset_id: asset_id.to_string(),
            mime_type: ClipboardAssetMimeType::ImagePng,
            base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        })
    }

    fn read_validated_asset(&self, asset_id: Uuid) -> Result<Vec<u8>, CommandError> {
        let asset = self.repository.get_asset(asset_id)?;
        let png = self.assets.read_owned_bounded(
            &asset.asset_name,
            crate::domain::clipboard::MAX_IMAGE_PNG_BYTES,
        )?;
        validate_stored_png_with_decoder(
            &png,
            asset.width,
            asset.height,
            &asset.sha256,
            asset.byte_size,
            |decoder| {
                image::DynamicImage::from_decoder(decoder)
                    .map_err(|_| io_failure())
                    .map(image::DynamicImage::into_rgba8)
            },
        )?;
        Ok(png)
    }

    pub(crate) fn process_notification(
        &self,
        generation: u64,
        current_generation: &AtomicU64,
        source_app: Option<String>,
        now: i64,
    ) -> Result<Option<ClipboardItem>, CommandError> {
        if current_generation.load(Ordering::Acquire) != generation {
            return Ok(None);
        }
        let mut source = self.source_factory.open()?;
        let cancelled = AtomicBool::new(false);
        self.process_source_notification(
            source.as_mut(),
            generation,
            current_generation,
            &cancelled,
            source_app,
            now,
        )
    }

    fn process_source_notification(
        &self,
        source: &mut dyn super::clipboard_listener::ClipboardSource,
        generation: u64,
        current_generation: &AtomicU64,
        cancelled: &AtomicBool,
        source_app: Option<String>,
        now: i64,
    ) -> Result<Option<ClipboardItem>, CommandError> {
        if !callback_is_current(generation, current_generation, cancelled) {
            return Ok(None);
        }
        let outcome = read_with_locked_retry(source, self.sleeper.as_ref());
        if !callback_is_current(generation, current_generation, cancelled) {
            return Ok(None);
        }
        match outcome {
            ClipboardReadOutcome::Content(None) => {
                self.record_healthy(now)?;
                Ok(None)
            }
            ClipboardReadOutcome::Content(Some(content)) => {
                let size = content_size(&content);
                let kind = content_kind(&content);
                let capture = self.capture(source_app, content, now);
                let outcome = match capture {
                    Ok(outcome) => outcome,
                    Err(error) if error.code == crate::contracts::AppErrorCode::InvalidInput => {
                        self.record_diagnostic(
                            "clipboard.captureTooLarge",
                            std::collections::BTreeMap::from([
                                ("kind".into(), SafeParameterValue::String(kind.into())),
                                (
                                    "byteCount".into(),
                                    SafeParameterValue::Number(serde_json::Number::from(size)),
                                ),
                            ]),
                            now,
                        );
                        return Ok(None);
                    }
                    Err(error) => return Err(error),
                };
                if !callback_is_current(generation, current_generation, cancelled) {
                    return Ok(None);
                }
                self.record_healthy(now)?;
                if !callback_is_current(generation, current_generation, cancelled) {
                    return Ok(None);
                }
                if self
                    .emitter
                    .emit(
                        CLIPBOARD_CHANGED,
                        clipboard_changed_payload(&outcome.item.id, now),
                    )
                    .is_err()
                {
                    self.record_diagnostic(
                        "events.clipboardChangedEmitFailed",
                        std::collections::BTreeMap::from([(
                            "entityId".into(),
                            SafeParameterValue::String(outcome.item.id.clone()),
                        )]),
                        now,
                    );
                }
                Ok(Some(outcome.item))
            }
            ClipboardReadOutcome::Locked { count } => {
                self.health.upsert(&ServiceHealthSnapshot {
                    service_id: "clipboard".into(),
                    state: ServiceHealthState::Degraded,
                    message_key: "services.clipboard.locked".into(),
                    parameters: std::collections::BTreeMap::from([(
                        "count".into(),
                        SafeParameterValue::Number(serde_json::Number::from(count)),
                    )]),
                    checked_at: now,
                })?;
                Ok(None)
            }
            ClipboardReadOutcome::Failed { reason_code } => {
                self.record_diagnostic(
                    "clipboard.readFailed",
                    std::collections::BTreeMap::from([(
                        "reasonCode".into(),
                        SafeParameterValue::String(reason_code.clone()),
                    )]),
                    now,
                );
                self.health.upsert(&ServiceHealthSnapshot {
                    service_id: "clipboard".into(),
                    state: ServiceHealthState::Degraded,
                    message_key: "services.degraded".into(),
                    parameters: std::collections::BTreeMap::from([
                        (
                            "serviceId".into(),
                            SafeParameterValue::String("clipboard".into()),
                        ),
                        ("reasonCode".into(), SafeParameterValue::String(reason_code)),
                    ]),
                    checked_at: now,
                })?;
                Ok(None)
            }
        }
    }

    fn cleanup_assets(&self, asset_names: &[String], now: i64) {
        for asset_name in asset_names {
            if self.assets.delete_owned(asset_name).is_err() {
                self.record_asset_cleanup_failure("deleteFailed", now);
            }
        }
    }

    fn recover_orphan_assets(&self, now: i64) {
        let referenced_names = match self.repository.referenced_asset_names() {
            Ok(referenced_names) => referenced_names,
            Err(_) => {
                self.record_asset_cleanup_failure("referenceReadFailed", now);
                return;
            }
        };
        if self.assets.remove_orphans(&referenced_names).is_err() {
            self.record_asset_cleanup_failure("orphanRecoveryFailed", now);
        }
    }

    fn record_asset_cleanup_failure(&self, reason_code: &str, now: i64) {
        self.record_diagnostic(
            "clipboard.assetCleanupFailed",
            std::collections::BTreeMap::from([(
                "reasonCode".into(),
                SafeParameterValue::String(reason_code.into()),
            )]),
            now,
        );
    }

    fn emit_or_record(&self, entity_id: &str, changed_at: i64) {
        if self
            .emitter
            .emit(
                CLIPBOARD_CHANGED,
                clipboard_changed_payload(entity_id, changed_at),
            )
            .is_err()
        {
            self.record_diagnostic(
                "events.clipboardChangedEmitFailed",
                std::collections::BTreeMap::from([(
                    "entityId".into(),
                    SafeParameterValue::String(entity_id.into()),
                )]),
                changed_at,
            );
        }
    }

    fn record_healthy(&self, checked_at: i64) -> Result<(), CommandError> {
        self.health.upsert(&ServiceHealthSnapshot {
            service_id: "clipboard".into(),
            state: ServiceHealthState::Healthy,
            message_key: "services.healthy".into(),
            parameters: std::collections::BTreeMap::from([(
                "serviceId".into(),
                SafeParameterValue::String("clipboard".into()),
            )]),
            checked_at,
        })
    }

    fn record_diagnostic(
        &self,
        code: &str,
        parameters: crate::contracts::SafeMessageParameters,
        created_at: i64,
    ) {
        let _ = self.diagnostics.record(&DiagnosticEvent {
            id: format!("clipboard-{}-{}", code.replace('.', "-"), Uuid::new_v4()),
            service_id: "clipboard".into(),
            level: DiagnosticLevel::Warning,
            code: code.into(),
            parameters,
            created_at,
        });
    }
}

#[cfg(windows)]
fn spawn_cancellation_watcher(
    mut cancel: watch::Receiver<bool>,
    stop_signal: super::clipboard_listener::ClipboardListenerStopSignal,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            if *cancel.borrow() {
                let _ = stop_signal.request_stop();
                break;
            }
            if cancel.changed().await.is_err() {
                let _ = stop_signal.request_stop();
                break;
            }
        }
    });
}

fn invalid_input() -> CommandError {
    CommandError {
        code: crate::contracts::AppErrorCode::InvalidInput,
        message_key: "errors.invalidInput".into(),
        details: crate::contracts::SafeMessageParameters::new(),
        retryable: false,
    }
}

fn io_failure() -> CommandError {
    CommandError {
        code: crate::contracts::AppErrorCode::IoFailure,
        message_key: "errors.ioFailure".into(),
        details: crate::contracts::SafeMessageParameters::new(),
        retryable: true,
    }
}

fn validate_stored_png_with_decoder<'a, F>(
    png: &'a [u8],
    expected_width: u32,
    expected_height: u32,
    expected_sha256: &str,
    expected_byte_size: u64,
    decode: F,
) -> Result<(), CommandError>
where
    F: FnOnce(
        image::codecs::png::PngDecoder<std::io::Cursor<&'a [u8]>>,
    ) -> Result<image::RgbaImage, CommandError>,
{
    let rgba_bytes = usize::try_from(expected_width)
        .ok()
        .and_then(|width| {
            usize::try_from(expected_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(io_failure)?;
    let (sha256, byte_size) =
        validate_image_capture(expected_width, expected_height, rgba_bytes, png)
            .map_err(|_| io_failure())?;
    if sha256 != expected_sha256 || byte_size != expected_byte_size {
        return Err(io_failure());
    }

    use image::ImageDecoder;
    let decoder =
        image::codecs::png::PngDecoder::new(std::io::Cursor::new(png)).map_err(|_| io_failure())?;
    if decoder.dimensions() != (expected_width, expected_height)
        || decoder.total_bytes() != rgba_bytes as u64
    {
        return Err(io_failure());
    }
    let image = decode(decoder)?;
    if image.dimensions() != (expected_width, expected_height) || image.as_raw().len() != rgba_bytes
    {
        return Err(io_failure());
    }
    Ok(())
}

fn content_kind(content: &CapturedClipboardContent) -> &'static str {
    match content {
        CapturedClipboardContent::Text { .. } => "text",
        CapturedClipboardContent::Image { .. } => "image",
    }
}

fn content_size(content: &CapturedClipboardContent) -> u64 {
    match content {
        CapturedClipboardContent::Text { byte_size, .. }
        | CapturedClipboardContent::Image { byte_size, .. } => *byte_size,
    }
}

fn callback_is_current(
    generation: u64,
    current_generation: &AtomicU64,
    cancelled: &AtomicBool,
) -> bool {
    !cancelled.load(Ordering::Acquire) && current_generation.load(Ordering::Acquire) == generation
}

#[cfg(test)]
mod tests {
    use super::{validate_stored_png_with_decoder, ClipboardAssetPort, ClipboardService};
    use crate::contracts::{
        ClipboardContentKindFilter, CommandError, ListClipboardItemsInput, ServiceHealthState,
    };
    use crate::domain::clipboard::{
        validate_text_capture, BootstrapClipboardRetentionPolicy, CapturedClipboardContent,
        ClipboardRetentionPolicy, MAX_TEXT_BYTES,
    };
    use crate::repositories::clipboard::ClipboardRepository;
    use crate::repositories::diagnostics::DiagnosticsRepository;
    use crate::repositories::service_health::ServiceHealthRepository;
    use crate::services::clipboard_assets::ClipboardAssetStore;
    use crate::services::clipboard_listener::{
        ClipboardReadError, ClipboardRetrySleeper, ClipboardSource, ClipboardSourceFactory,
    };
    use crate::services::EventEmitterPort;
    use crate::storage::Storage;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn stored_png_metadata_is_rejected_before_full_decode() {
        let png = crate::services::clipboard_listener::encode_rgba_png(
            2,
            2,
            &[1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255],
        )
        .unwrap();
        let (sha256, byte_size) =
            crate::domain::clipboard::validate_image_capture(2, 2, 16, &png).unwrap();
        let cases = [
            (1, 1, sha256.as_str(), byte_size),
            (
                2,
                2,
                "0000000000000000000000000000000000000000000000000000000000000000",
                byte_size,
            ),
            (2, 2, sha256.as_str(), byte_size + 1),
        ];

        for (width, height, expected_sha256, expected_byte_size) in cases {
            let decoded = std::cell::Cell::new(false);
            let error = validate_stored_png_with_decoder(
                &png,
                width,
                height,
                expected_sha256,
                expected_byte_size,
                |decoder| {
                    decoded.set(true);
                    Ok(image::DynamicImage::from_decoder(decoder)
                        .unwrap()
                        .into_rgba8())
                },
            )
            .unwrap_err();
            assert_eq!(error.code, crate::contracts::AppErrorCode::IoFailure);
            assert!(!decoded.get(), "invalid metadata reached full PNG decode");
        }
    }

    #[derive(Clone)]
    enum ReadStep {
        Content(CapturedText),
        Image {
            png: Vec<u8>,
            width: u32,
            height: u32,
        },
        Occupied,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum WriteStep {
        Text(String),
        Png(Vec<u8>),
    }

    #[derive(Clone)]
    struct CapturedText(String);

    struct SharedSource {
        steps: Arc<Mutex<VecDeque<ReadStep>>>,
        writes: Arc<Mutex<Vec<WriteStep>>>,
    }

    impl ClipboardSource for SharedSource {
        fn read(&mut self) -> Result<Option<CapturedClipboardContent>, ClipboardReadError> {
            match self.steps.lock().unwrap().pop_front() {
                Some(ReadStep::Content(CapturedText(text))) => {
                    let byte_size = text.len() as u64;
                    let sha256 = validate_text_capture(&text)
                        .map(|(sha256, _)| sha256)
                        .unwrap_or_default();
                    Ok(Some(CapturedClipboardContent::Text {
                        text,
                        sha256,
                        byte_size,
                    }))
                }
                Some(ReadStep::Occupied) => Err(ClipboardReadError::Occupied),
                Some(ReadStep::Image { png, width, height }) => {
                    Ok(Some(CapturedClipboardContent::Image {
                        byte_size: png.len() as u64,
                        png,
                        sha256: String::new(),
                        width,
                        height,
                    }))
                }
                None => Ok(None),
            }
        }

        fn write_text(&mut self, value: &str) -> Result<(), CommandError> {
            self.writes
                .lock()
                .unwrap()
                .push(WriteStep::Text(value.into()));
            Ok(())
        }

        fn write_png(&mut self, png: &[u8]) -> Result<(), CommandError> {
            self.writes
                .lock()
                .unwrap()
                .push(WriteStep::Png(png.to_vec()));
            Ok(())
        }
    }

    struct SharedFactory {
        steps: Arc<Mutex<VecDeque<ReadStep>>>,
        writes: Arc<Mutex<Vec<WriteStep>>>,
    }

    impl ClipboardSourceFactory for SharedFactory {
        fn open(&self) -> Result<Box<dyn ClipboardSource>, CommandError> {
            Ok(Box::new(SharedSource {
                steps: self.steps.clone(),
                writes: self.writes.clone(),
            }))
        }
    }

    #[derive(Default)]
    struct RecordingSleeper(Mutex<Vec<Duration>>);

    impl ClipboardRetrySleeper for RecordingSleeper {
        fn sleep(&self, duration: Duration) {
            self.0.lock().unwrap().push(duration);
        }
    }

    struct CommitObservingEmitter {
        repository: ClipboardRepository,
        events: Mutex<Vec<(&'static str, serde_json::Value)>>,
        reject: AtomicBool,
    }

    impl EventEmitterPort for CommitObservingEmitter {
        fn emit(
            &self,
            event_name: &'static str,
            payload: serde_json::Value,
        ) -> Result<(), CommandError> {
            assert_eq!(list_all(&self.repository).len(), 1, "event preceded commit");
            if self.reject.load(Ordering::Acquire) {
                return Err(CommandError {
                    code: crate::contracts::AppErrorCode::SourceUnavailable,
                    message_key: "errors.sourceUnavailable".into(),
                    details: crate::contracts::SafeMessageParameters::new(),
                    retryable: false,
                });
            }
            self.events.lock().unwrap().push((event_name, payload));
            Ok(())
        }
    }

    struct Fixture {
        service: Arc<ClipboardService>,
        repository: ClipboardRepository,
        health: ServiceHealthRepository,
        diagnostics: DiagnosticsRepository,
        emitter: Arc<CommitObservingEmitter>,
        sleeper: Arc<RecordingSleeper>,
        writes: Arc<Mutex<Vec<WriteStep>>>,
        asset_root: std::path::PathBuf,
    }

    fn fixture(steps: Vec<ReadStep>) -> Fixture {
        fixture_with_retention(steps, Arc::new(BootstrapClipboardRetentionPolicy))
    }

    struct FixedRetention(u32);

    impl ClipboardRetentionPolicy for FixedRetention {
        fn unpinned_limit(&self) -> Result<u32, CommandError> {
            Ok(self.0)
        }
    }

    fn fixture_with_retention(
        steps: Vec<ReadStep>,
        retention: Arc<dyn ClipboardRetentionPolicy>,
    ) -> Fixture {
        let directory = tempfile::tempdir().unwrap().keep();
        let storage = Arc::new(Storage::open(&directory).unwrap());
        let repository = ClipboardRepository::new(storage.clone(), retention);
        let health = ServiceHealthRepository::new(storage.clone());
        let diagnostics = DiagnosticsRepository::new(storage);
        let emitter = Arc::new(CommitObservingEmitter {
            repository: repository.clone(),
            events: Mutex::new(Vec::new()),
            reject: AtomicBool::new(false),
        });
        let sleeper = Arc::new(RecordingSleeper::default());
        let writes = Arc::new(Mutex::new(Vec::new()));
        let service = Arc::new(
            ClipboardService::new(
                repository.clone(),
                ClipboardAssetStore::new(&directory).unwrap(),
                Arc::new(SharedFactory {
                    steps: Arc::new(Mutex::new(steps.into())),
                    writes: writes.clone(),
                }),
                health.clone(),
                diagnostics.clone(),
                emitter.clone(),
            )
            .with_sleeper(sleeper.clone()),
        );
        Fixture {
            service,
            repository,
            health,
            diagnostics,
            emitter,
            sleeper,
            writes,
            asset_root: directory.join("clipboard-assets"),
        }
    }

    fn list_all(repository: &ClipboardRepository) -> Vec<crate::contracts::ClipboardItem> {
        repository
            .list(ListClipboardItemsInput {
                query: String::new(),
                content_kind: ClipboardContentKindFilter::All,
                limit: 500,
            })
            .unwrap()
    }

    #[test]
    fn notification_commits_text_before_emitting_the_exact_hint() {
        let fixture = fixture(vec![ReadStep::Content(CapturedText("alpha".into()))]);
        let current_generation = AtomicU64::new(1);
        let item = fixture
            .service
            .process_notification(1, &current_generation, Some("notepad.exe".into()), 41)
            .unwrap()
            .expect("notification should capture text");

        assert_eq!(item.text_content.as_deref(), Some("alpha"));
        assert_eq!(item.source_app.as_deref(), Some("notepad.exe"));
        assert_eq!(list_all(&fixture.repository), vec![item.clone()]);
        assert_eq!(
            fixture.emitter.events.lock().unwrap().as_slice(),
            [(
                "clipboardChanged",
                serde_json::json!({ "entityId": item.id, "changedAt": 41 })
            )]
        );
    }

    #[test]
    fn locked_notification_degrades_then_next_notification_restores_health() {
        let fixture = fixture(vec![
            ReadStep::Occupied,
            ReadStep::Occupied,
            ReadStep::Occupied,
            ReadStep::Content(CapturedText("recovered".into())),
        ]);
        let current_generation = AtomicU64::new(7);
        assert!(fixture
            .service
            .process_notification(7, &current_generation, None, 50)
            .unwrap()
            .is_none());
        let degraded = fixture.health.list().unwrap();
        assert_eq!(degraded.len(), 1);
        assert_eq!(degraded[0].state, ServiceHealthState::Degraded);
        assert_eq!(degraded[0].message_key, "services.clipboard.locked");
        assert_eq!(
            fixture.sleeper.0.lock().unwrap().as_slice(),
            [
                Duration::from_millis(25),
                Duration::from_millis(50),
                Duration::from_millis(100),
            ]
        );

        fixture
            .service
            .process_notification(7, &current_generation, None, 51)
            .unwrap()
            .expect("next notification should recover");
        let healthy = fixture.health.list().unwrap();
        assert_eq!(healthy.len(), 1);
        assert_eq!(healthy[0].state, ServiceHealthState::Healthy);
        assert_eq!(healthy[0].message_key, "services.healthy");
    }

    #[test]
    fn empty_success_after_locked_notification_restores_health() {
        let fixture = fixture(vec![
            ReadStep::Occupied,
            ReadStep::Occupied,
            ReadStep::Occupied,
        ]);
        let current_generation = AtomicU64::new(8);
        fixture
            .service
            .process_notification(8, &current_generation, None, 52)
            .unwrap();
        assert_eq!(
            fixture.health.list().unwrap()[0].state,
            ServiceHealthState::Degraded
        );

        assert!(fixture
            .service
            .process_notification(8, &current_generation, None, 53)
            .unwrap()
            .is_none());
        let recovered = fixture.health.list().unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].state, ServiceHealthState::Healthy);
        assert_eq!(recovered[0].checked_at, 53);
    }

    #[test]
    fn oversized_content_records_only_kind_and_byte_count() {
        let fixture = fixture(vec![ReadStep::Content(CapturedText(
            "x".repeat(MAX_TEXT_BYTES + 1),
        ))]);
        let current_generation = AtomicU64::new(1);
        assert!(fixture
            .service
            .process_notification(1, &current_generation, None, 60)
            .unwrap()
            .is_none());
        let diagnostics = fixture.diagnostics.list(10).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "clipboard.captureTooLarge");
        assert_eq!(
            diagnostics[0].parameters,
            std::collections::BTreeMap::from([
                (
                    "byteCount".into(),
                    crate::contracts::SafeParameterValue::Number(serde_json::Number::from(
                        (MAX_TEXT_BYTES + 1) as u64
                    ))
                ),
                (
                    "kind".into(),
                    crate::contracts::SafeParameterValue::String("text".into())
                ),
            ])
        );
    }

    #[test]
    fn clipboard_event_failure_preserves_the_committed_row_and_records_only_entity_id() {
        let fixture = fixture(vec![ReadStep::Content(CapturedText("durable".into()))]);
        fixture.emitter.reject.store(true, Ordering::Release);
        let current_generation = AtomicU64::new(1);
        let item = fixture
            .service
            .process_notification(1, &current_generation, None, 61)
            .unwrap()
            .expect("capture remains successful when the UI hint fails");

        assert_eq!(list_all(&fixture.repository), vec![item.clone()]);
        let diagnostics = fixture.diagnostics.list(10).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "events.clipboardChangedEmitFailed");
        assert_eq!(
            diagnostics[0].parameters,
            std::collections::BTreeMap::from([(
                "entityId".into(),
                crate::contracts::SafeParameterValue::String(item.id)
            )])
        );
    }

    #[cfg(windows)]
    #[test]
    fn committed_capture_survives_cleanup_failure_and_a_later_capture_recovers_the_orphan() {
        let first_png =
            crate::services::clipboard_listener::encode_rgba_png(1, 1, &[255, 0, 0, 255]).unwrap();
        let second_png =
            crate::services::clipboard_listener::encode_rgba_png(1, 1, &[0, 0, 255, 255]).unwrap();
        let fixture = fixture_with_retention(
            vec![
                ReadStep::Image {
                    png: first_png,
                    width: 1,
                    height: 1,
                },
                ReadStep::Image {
                    png: second_png,
                    width: 1,
                    height: 1,
                },
                ReadStep::Content(CapturedText("orphan recovery trigger".into())),
            ],
            Arc::new(FixedRetention(1)),
        );
        let current_generation = AtomicU64::new(1);
        let first = fixture
            .service
            .process_notification(1, &current_generation, None, 62)
            .unwrap()
            .unwrap();
        let first_asset = fixture
            .repository
            .get_asset(uuid::Uuid::parse_str(first.asset_id.as_deref().unwrap()).unwrap())
            .unwrap();
        let orphan_path = fixture.asset_root.join(first_asset.asset_name);
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
        let blocking_handle = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
            .open(&orphan_path)
            .unwrap();

        let second = fixture
            .service
            .process_notification(1, &current_generation, None, 63)
            .expect("post-commit cleanup failure must not undo semantic success")
            .unwrap();
        assert_eq!(list_all(&fixture.repository), vec![second.clone()]);
        assert_eq!(fixture.emitter.events.lock().unwrap().len(), 2);
        assert!(std::fs::exists(&orphan_path).unwrap());
        assert_eq!(
            fixture.diagnostics.list(10).unwrap()[0].code,
            "clipboard.assetCleanupFailed"
        );

        drop(blocking_handle);
        fixture
            .service
            .process_notification(1, &current_generation, None, 64)
            .unwrap()
            .unwrap();
        assert!(!std::fs::exists(orphan_path).unwrap());
    }

    #[test]
    fn stale_generation_cannot_capture_emit_or_write_health() {
        let fixture = fixture(vec![
            ReadStep::Content(CapturedText("stale".into())),
            ReadStep::Content(CapturedText("current".into())),
        ]);
        let current_generation = AtomicU64::new(2);
        assert!(fixture
            .service
            .process_notification(1, &current_generation, None, 70)
            .unwrap()
            .is_none());
        assert!(list_all(&fixture.repository).is_empty());
        assert!(fixture.health.list().unwrap().is_empty());
        assert!(fixture.emitter.events.lock().unwrap().is_empty());

        fixture
            .service
            .process_notification(2, &current_generation, None, 71)
            .unwrap()
            .expect("current generation should capture");
        let rows = list_all(&fixture.repository);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text_content.as_deref(), Some("stale"));
    }

    #[test]
    fn image_capture_exposes_padded_base64_and_copy_uses_the_owned_png() {
        use base64::Engine;
        let fixture = fixture(Vec::new());
        let rgba = [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 128,
        ];
        let png = crate::services::clipboard_listener::encode_rgba_png(2, 2, &rgba).unwrap();
        let item = fixture
            .service
            .capture(
                Some("paint.exe".into()),
                CapturedClipboardContent::Image {
                    png: png.clone(),
                    sha256: String::new(),
                    width: 2,
                    height: 2,
                    byte_size: png.len() as u64,
                },
                80,
            )
            .unwrap()
            .item;
        let asset_id = uuid::Uuid::parse_str(item.asset_id.as_deref().unwrap()).unwrap();
        let payload = fixture.service.asset_payload(asset_id).unwrap();
        assert_eq!(payload.asset_id, asset_id.to_string());
        assert_eq!(
            payload.mime_type,
            crate::contracts::ClipboardAssetMimeType::ImagePng
        );
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(payload.base64)
                .unwrap(),
            png
        );

        fixture
            .service
            .copy_item(uuid::Uuid::parse_str(&item.id).unwrap())
            .unwrap();
        assert_eq!(
            fixture.writes.lock().unwrap().as_slice(),
            [WriteStep::Png(png)]
        );
    }

    struct BlockingAssetPort {
        inner: ClipboardAssetStore,
        block_next_write: AtomicBool,
        write_in_flight: AtomicBool,
        written: Mutex<Option<std::sync::mpsc::SyncSender<()>>>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
        concurrent_recovery: std::sync::mpsc::SyncSender<()>,
    }

    impl ClipboardAssetPort for BlockingAssetPort {
        fn write_png_atomic(
            &self,
            asset_id: uuid::Uuid,
            bytes: &[u8],
        ) -> Result<String, CommandError> {
            let asset_name = self.inner.write_png_atomic(asset_id, bytes)?;
            if self.block_next_write.swap(false, Ordering::AcqRel) {
                self.write_in_flight.store(true, Ordering::Release);
                if let Some(written) = self.written.lock().unwrap().take() {
                    written.send(()).unwrap();
                }
                self.release.lock().unwrap().recv().unwrap();
                self.write_in_flight.store(false, Ordering::Release);
            }
            Ok(asset_name)
        }

        fn read_owned_bounded(
            &self,
            asset_name: &str,
            max_bytes: usize,
        ) -> Result<Vec<u8>, CommandError> {
            self.inner.read_owned_bounded(asset_name, max_bytes)
        }

        fn delete_owned(&self, asset_name: &str) -> Result<(), CommandError> {
            self.inner.delete_owned(asset_name)
        }

        fn remove_orphans(
            &self,
            referenced_names: &std::collections::BTreeSet<String>,
        ) -> Result<u64, CommandError> {
            if self.write_in_flight.load(Ordering::Acquire) {
                let _ = self.concurrent_recovery.try_send(());
            }
            self.inner.remove_orphans(referenced_names)
        }
    }

    #[test]
    fn concurrent_capture_cannot_recover_an_asset_before_its_metadata_commit() {
        let directory = tempfile::tempdir().unwrap().keep();
        let storage = Arc::new(Storage::open(&directory).unwrap());
        let repository =
            ClipboardRepository::new(storage.clone(), Arc::new(BootstrapClipboardRetentionPolicy));
        let health = ServiceHealthRepository::new(storage.clone());
        let diagnostics = DiagnosticsRepository::new(storage);
        let emitter = Arc::new(CommitObservingEmitter {
            repository: repository.clone(),
            events: Mutex::new(Vec::new()),
            reject: AtomicBool::new(false),
        });
        let (written_tx, written_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let (recovery_tx, recovery_rx) = std::sync::mpsc::sync_channel(1);
        let assets = Arc::new(BlockingAssetPort {
            inner: ClipboardAssetStore::new(&directory).unwrap(),
            block_next_write: AtomicBool::new(true),
            write_in_flight: AtomicBool::new(false),
            written: Mutex::new(Some(written_tx)),
            release: Mutex::new(release_rx),
            concurrent_recovery: recovery_tx,
        });
        let service = Arc::new(ClipboardService::new_with_asset_port(
            repository,
            assets,
            Arc::new(SharedFactory {
                steps: Arc::new(Mutex::new(VecDeque::new())),
                writes: Arc::new(Mutex::new(Vec::new())),
            }),
            health,
            diagnostics,
            emitter,
        ));

        let png =
            crate::services::clipboard_listener::encode_rgba_png(1, 1, &[255, 0, 0, 255]).unwrap();
        let image_service = service.clone();
        let image = std::thread::spawn(move || {
            image_service.capture(
                None,
                CapturedClipboardContent::Image {
                    byte_size: png.len() as u64,
                    png,
                    sha256: String::new(),
                    width: 1,
                    height: 1,
                },
                81,
            )
        });
        written_rx.recv().unwrap();

        let text_service = service.clone();
        let text = std::thread::spawn(move || {
            text_service.capture(
                None,
                CapturedClipboardContent::Text {
                    text: "concurrent".into(),
                    sha256: String::new(),
                    byte_size: 10,
                },
                82,
            )
        });
        let recovery_entered_during_write =
            recovery_rx.recv_timeout(Duration::from_millis(100)).is_ok();
        release_tx.send(()).unwrap();
        let image = image.join().unwrap().unwrap().item;
        text.join().unwrap().unwrap();

        assert!(!recovery_entered_during_write);
        let asset_id = uuid::Uuid::parse_str(image.asset_id.as_deref().unwrap()).unwrap();
        assert!(!service.asset_payload(asset_id).unwrap().base64.is_empty());
    }

    struct BlockingSource {
        entered: std::sync::mpsc::SyncSender<()>,
        release: std::sync::mpsc::Receiver<()>,
    }

    impl ClipboardSource for BlockingSource {
        fn read(&mut self) -> Result<Option<CapturedClipboardContent>, ClipboardReadError> {
            self.entered.send(()).unwrap();
            self.release.recv().unwrap();
            let (sha256, byte_size) = validate_text_capture("late").unwrap();
            Ok(Some(CapturedClipboardContent::Text {
                text: "late".into(),
                sha256,
                byte_size,
            }))
        }

        fn write_text(&mut self, _value: &str) -> Result<(), CommandError> {
            Ok(())
        }

        fn write_png(&mut self, _png: &[u8]) -> Result<(), CommandError> {
            Ok(())
        }
    }

    #[test]
    fn delayed_old_generation_is_rechecked_after_the_clipboard_read() {
        let fixture = fixture(Vec::new());
        let current_generation = Arc::new(AtomicU64::new(1));
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let service = fixture.service.clone();
        let callback_generation = current_generation.clone();
        let join = std::thread::spawn(move || {
            let mut source = BlockingSource {
                entered: entered_tx,
                release: release_rx,
            };
            service
                .process_source_notification(
                    &mut source,
                    1,
                    callback_generation.as_ref(),
                    &AtomicBool::new(false),
                    None,
                    90,
                )
                .unwrap()
        });
        entered_rx.recv().unwrap();
        current_generation.store(2, std::sync::atomic::Ordering::Release);
        release_tx.send(()).unwrap();

        assert!(join.join().unwrap().is_none());
        assert!(list_all(&fixture.repository).is_empty());
        assert!(fixture.health.list().unwrap().is_empty());
        assert!(fixture.emitter.events.lock().unwrap().is_empty());
    }

    #[test]
    fn cancellation_during_a_clipboard_read_fences_persistence_health_and_events() {
        let fixture = fixture(Vec::new());
        let current_generation = Arc::new(AtomicU64::new(1));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let service = fixture.service.clone();
        let callback_generation = current_generation.clone();
        let callback_cancelled = cancelled.clone();
        let join = std::thread::spawn(move || {
            let mut source = BlockingSource {
                entered: entered_tx,
                release: release_rx,
            };
            service
                .process_source_notification(
                    &mut source,
                    1,
                    callback_generation.as_ref(),
                    callback_cancelled.as_ref(),
                    None,
                    91,
                )
                .unwrap()
        });
        entered_rx.recv().unwrap();
        cancelled.store(true, Ordering::Release);
        release_tx.send(()).unwrap();

        assert!(join.join().unwrap().is_none());
        assert!(list_all(&fixture.repository).is_empty());
        assert!(fixture.health.list().unwrap().is_empty());
        assert!(fixture.emitter.events.lock().unwrap().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn dropping_the_cancel_sender_requests_the_same_listener_stop_signal() {
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let mut listener =
            crate::services::clipboard_listener::start_message_listener(|_| Ok(Box::new(|| {})))
                .unwrap();
        let stop_signal = listener.stop_signal();
        super::spawn_cancellation_watcher(cancel_rx, stop_signal.clone());
        drop(cancel_tx);

        for _ in 0..100 {
            if stop_signal.is_cancelled() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(stop_signal.is_cancelled());
        listener.stop().unwrap();
    }
}
