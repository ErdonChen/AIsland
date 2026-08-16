use crate::contracts::{
    AppErrorCode, CommandError, SafeParameterValue, UpdateCheckResult, UpdateCheckStatus,
    UpdateInstallEvent, UpdateInstallEventKind, UpdateInstallResult,
};
use std::sync::{Arc, Mutex};
use tauri::AppHandle;
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub version: String,
    pub notes: Option<String>,
}

pub trait UpdateEventSink: Send + Sync {
    fn send(&self, event: UpdateInstallEvent);
}

#[async_trait::async_trait]
pub trait UpdaterPort: Send + Sync {
    fn current_version(&self) -> String;
    async fn check(&self) -> Result<Option<AvailableUpdate>, CommandError>;
    async fn install(&self, sink: Arc<dyn UpdateEventSink>) -> Result<String, CommandError>;
}

pub struct TauriUpdaterPort {
    app: AppHandle,
    pending: tokio::sync::Mutex<Option<Update>>,
}

impl TauriUpdaterPort {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            pending: tokio::sync::Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl UpdaterPort for TauriUpdaterPort {
    fn current_version(&self) -> String {
        self.app.package_info().version.to_string()
    }

    async fn check(&self) -> Result<Option<AvailableUpdate>, CommandError> {
        let updater = self
            .app
            .updater()
            .map_err(|error| map_updater_error(error, UpdateOperation::Check))?;
        let update = updater
            .check()
            .await
            .map_err(|error| map_updater_error(error, UpdateOperation::Check))?;
        let Some(update) = update else {
            *self.pending.lock().await = None;
            return Ok(None);
        };
        let available = AvailableUpdate {
            version: update.version.clone(),
            notes: update.body.clone(),
        };
        *self.pending.lock().await = Some(update);
        Ok(Some(available))
    }

    async fn install(&self, sink: Arc<dyn UpdateEventSink>) -> Result<String, CommandError> {
        let update = self
            .pending
            .lock()
            .await
            .take()
            .ok_or_else(update_not_checked)?;
        let version = update.version.clone();
        let downloaded = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let latest_total = Arc::new(Mutex::new(None));
        let progress_downloaded = downloaded.clone();
        let progress_total = latest_total.clone();
        let progress_sink = sink.clone();
        let finish_downloaded = downloaded.clone();
        let finish_total = latest_total.clone();
        let finish_sink = sink;
        update
            .download_and_install(
                move |chunk_length, total| {
                    let chunk_length = u64::try_from(chunk_length).unwrap_or(u64::MAX);
                    let cumulative = progress_downloaded
                        .fetch_add(chunk_length, std::sync::atomic::Ordering::Relaxed)
                        .saturating_add(chunk_length);
                    *progress_total.lock().unwrap() = total;
                    progress_sink.send(UpdateInstallEvent {
                        event: UpdateInstallEventKind::Progress,
                        downloaded: cumulative,
                        total,
                    });
                },
                move || {
                    #[cfg(windows)]
                    finish_sink.send(UpdateInstallEvent {
                        event: UpdateInstallEventKind::Finished,
                        downloaded: finish_downloaded.load(std::sync::atomic::Ordering::Relaxed),
                        total: *finish_total.lock().unwrap(),
                    });
                },
            )
            .await
            .map_err(|error| map_updater_error(error, UpdateOperation::Install))?;
        Ok(version)
    }
}

pub struct AppUpdateService {
    updater: Arc<dyn UpdaterPort>,
}

impl AppUpdateService {
    pub fn new(updater: Arc<dyn UpdaterPort>) -> Self {
        Self { updater }
    }

    pub async fn check_for_update(&self) -> Result<UpdateCheckResult, CommandError> {
        let current_version = self.updater.current_version();
        Ok(match self.updater.check().await? {
            Some(update) => UpdateCheckResult {
                status: UpdateCheckStatus::Available,
                current_version,
                latest_version: Some(update.version),
                notes: update.notes,
            },
            None => UpdateCheckResult {
                status: UpdateCheckStatus::UpToDate,
                current_version,
                latest_version: None,
                notes: None,
            },
        })
    }

    pub async fn install_update(
        &self,
        sink: Arc<dyn UpdateEventSink>,
    ) -> Result<UpdateInstallResult, CommandError> {
        sink.send(UpdateInstallEvent {
            event: UpdateInstallEventKind::Started,
            downloaded: 0,
            total: None,
        });
        let tracking_sink = Arc::new(TrackingEventSink {
            downstream: sink.clone(),
            latest: Mutex::new((0, None)),
        });
        let installed_version = self.updater.install(tracking_sink.clone()).await?;
        let (downloaded, total) = *tracking_sink.latest.lock().unwrap();
        sink.send(UpdateInstallEvent {
            event: UpdateInstallEventKind::Finished,
            downloaded,
            total,
        });
        Ok(UpdateInstallResult {
            installed_version,
            restart_required: true,
        })
    }
}

struct TrackingEventSink {
    downstream: Arc<dyn UpdateEventSink>,
    latest: Mutex<(u64, Option<u64>)>,
}

impl UpdateEventSink for TrackingEventSink {
    fn send(&self, event: UpdateInstallEvent) {
        if event.event == UpdateInstallEventKind::Progress {
            *self.latest.lock().unwrap() = (event.downloaded, event.total);
        }
        self.downstream.send(event);
    }
}

#[derive(Clone, Copy)]
enum UpdateOperation {
    Check,
    Install,
}

fn map_updater_error(
    error: tauri_plugin_updater::Error,
    operation: UpdateOperation,
) -> CommandError {
    use tauri_plugin_updater::Error;

    match error {
        Error::EmptyEndpoints => updater_source_error("updaterNotConfigured", false),
        Error::UnsupportedArch | Error::UnsupportedOs => CommandError {
            code: AppErrorCode::PlatformUnsupported,
            message_key: "errors.platformUnsupported".into(),
            details: [
                (
                    "serviceId".into(),
                    SafeParameterValue::String("updater".into()),
                ),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String("updaterPlatformUnsupported".into()),
                ),
            ]
            .into(),
            retryable: false,
        },
        Error::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            update_permission_denied()
        }
        Error::AuthenticationFailed => update_permission_denied(),
        Error::Minisign(_)
        | Error::Base64(_)
        | Error::SignatureUtf8(_)
        | Error::InvalidUpdaterFormat => updater_source_error("updateVerificationFailed", false),
        _ => match operation {
            UpdateOperation::Check => updater_source_error("updateSourceUnavailable", true),
            UpdateOperation::Install => updater_source_error("updateInstallFailed", true),
        },
    }
}

fn updater_source_error(reason_code: &str, retryable: bool) -> CommandError {
    CommandError {
        code: AppErrorCode::SourceUnavailable,
        message_key: "errors.sourceUnavailable".into(),
        details: [
            (
                "serviceId".into(),
                SafeParameterValue::String("updater".into()),
            ),
            (
                "reasonCode".into(),
                SafeParameterValue::String(reason_code.into()),
            ),
        ]
        .into(),
        retryable,
    }
}

fn update_not_checked() -> CommandError {
    CommandError::with_detail(
        AppErrorCode::Conflict,
        "errors.conflict",
        "reasonCode",
        SafeParameterValue::String("updateNotChecked".into()),
        false,
    )
}

fn update_permission_denied() -> CommandError {
    CommandError::with_detail(
        AppErrorCode::PermissionDenied,
        "errors.permissionDenied",
        "reasonCode",
        SafeParameterValue::String("updateInstallPermissionDenied".into()),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{AppErrorCode, SafeMessageParameters, SafeParameterValue};

    struct FakeUpdater {
        current_version: String,
        available: Mutex<Result<Option<AvailableUpdate>, CommandError>>,
        install_result: Mutex<Result<String, CommandError>>,
        progress: Vec<(u64, Option<u64>)>,
    }

    #[async_trait::async_trait]
    impl UpdaterPort for FakeUpdater {
        fn current_version(&self) -> String {
            self.current_version.clone()
        }

        async fn check(&self) -> Result<Option<AvailableUpdate>, CommandError> {
            self.available.lock().unwrap().clone()
        }

        async fn install(&self, sink: Arc<dyn UpdateEventSink>) -> Result<String, CommandError> {
            for (downloaded, total) in &self.progress {
                sink.send(UpdateInstallEvent {
                    event: UpdateInstallEventKind::Progress,
                    downloaded: *downloaded,
                    total: *total,
                });
            }
            self.install_result.lock().unwrap().clone()
        }
    }

    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<UpdateInstallEvent>>);

    impl UpdateEventSink for RecordingSink {
        fn send(&self, event: UpdateInstallEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn source_unavailable(reason_code: &str, retryable: bool) -> CommandError {
        CommandError {
            code: AppErrorCode::SourceUnavailable,
            message_key: "errors.sourceUnavailable".into(),
            details: SafeMessageParameters::from([
                (
                    "serviceId".into(),
                    SafeParameterValue::String("updater".into()),
                ),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String(reason_code.into()),
                ),
            ]),
            retryable,
        }
    }

    fn service(
        available: Result<Option<AvailableUpdate>, CommandError>,
        install_result: Result<String, CommandError>,
        progress: Vec<(u64, Option<u64>)>,
    ) -> AppUpdateService {
        AppUpdateService::new(Arc::new(FakeUpdater {
            current_version: "0.1.0".into(),
            available: Mutex::new(available),
            install_result: Mutex::new(install_result),
            progress,
        }))
    }

    #[tokio::test]
    async fn up_to_date_and_available_results_match_the_frontend_contract() {
        let up_to_date = service(Ok(None), Ok("0.1.0".into()), Vec::new())
            .check_for_update()
            .await
            .unwrap();
        assert_eq!(
            up_to_date,
            UpdateCheckResult {
                status: UpdateCheckStatus::UpToDate,
                current_version: "0.1.0".into(),
                latest_version: None,
                notes: None,
            }
        );

        let available = service(
            Ok(Some(AvailableUpdate {
                version: "0.2.0".into(),
                notes: Some("Changes".into()),
            })),
            Ok("0.2.0".into()),
            Vec::new(),
        )
        .check_for_update()
        .await
        .unwrap();
        assert_eq!(available.status, UpdateCheckStatus::Available);
        assert_eq!(available.current_version, "0.1.0");
        assert_eq!(available.latest_version.as_deref(), Some("0.2.0"));
        assert_eq!(available.notes.as_deref(), Some("Changes"));
    }

    #[tokio::test]
    async fn check_failure_is_preserved_instead_of_claiming_up_to_date() {
        let expected = source_unavailable("updaterNotConfigured", false);
        let error = service(Err(expected.clone()), Ok("0.1.0".into()), Vec::new())
            .check_for_update()
            .await
            .unwrap_err();

        assert_eq!(error, expected);
    }

    #[tokio::test]
    async fn install_emits_started_cumulative_progress_and_finished_then_returns_version() {
        let service = service(
            Ok(Some(AvailableUpdate {
                version: "0.2.0".into(),
                notes: None,
            })),
            Ok("0.2.0".into()),
            vec![(4, Some(10)), (10, Some(10))],
        );
        let sink = Arc::new(RecordingSink::default());

        let result = service.install_update(sink.clone()).await.unwrap();

        assert_eq!(
            result,
            UpdateInstallResult {
                installed_version: "0.2.0".into(),
                restart_required: true,
            }
        );
        assert_eq!(
            *sink.0.lock().unwrap(),
            vec![
                UpdateInstallEvent {
                    event: UpdateInstallEventKind::Started,
                    downloaded: 0,
                    total: None,
                },
                UpdateInstallEvent {
                    event: UpdateInstallEventKind::Progress,
                    downloaded: 4,
                    total: Some(10),
                },
                UpdateInstallEvent {
                    event: UpdateInstallEventKind::Progress,
                    downloaded: 10,
                    total: Some(10),
                },
                UpdateInstallEvent {
                    event: UpdateInstallEventKind::Finished,
                    downloaded: 10,
                    total: Some(10),
                },
            ]
        );
    }

    #[tokio::test]
    async fn failed_install_never_emits_finished_or_rewrites_the_typed_error() {
        let expected = source_unavailable("updateVerificationFailed", false);
        let service = service(
            Ok(Some(AvailableUpdate {
                version: "0.2.0".into(),
                notes: None,
            })),
            Err(expected.clone()),
            vec![(4, Some(10))],
        );
        let sink = Arc::new(RecordingSink::default());

        let error = service.install_update(sink.clone()).await.unwrap_err();

        assert_eq!(error, expected);
        assert_eq!(
            sink.0
                .lock()
                .unwrap()
                .iter()
                .map(|event| event.event.clone())
                .collect::<Vec<_>>(),
            vec![
                UpdateInstallEventKind::Started,
                UpdateInstallEventKind::Progress,
            ]
        );
    }
}
