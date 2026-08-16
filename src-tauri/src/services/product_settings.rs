use crate::contracts::{
    AppErrorCode, CommandError, GeneralSettings, SafeParameterValue, SaveGeneralSettingsInput,
};
use crate::repositories::app_settings::AppSettingsRepository;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

const GENERAL_SETTINGS_KEY: &str = "settings.general";

pub trait AutostartPort: Send + Sync {
    fn is_enabled(&self) -> Result<bool, CommandError>;
    fn enable(&self) -> Result<(), CommandError>;
    fn disable(&self) -> Result<(), CommandError>;
}

pub struct TauriAutostartPort {
    app: AppHandle,
}

impl TauriAutostartPort {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl AutostartPort for TauriAutostartPort {
    fn is_enabled(&self) -> Result<bool, CommandError> {
        self.app
            .autolaunch()
            .is_enabled()
            .map_err(map_autostart_error)
    }

    fn enable(&self) -> Result<(), CommandError> {
        self.app.autolaunch().enable().map_err(map_autostart_error)
    }

    fn disable(&self) -> Result<(), CommandError> {
        self.app.autolaunch().disable().map_err(map_autostart_error)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct GeneralSettingsValue {
    launch_at_startup: bool,
}

pub struct ProductSettingsService {
    settings: AppSettingsRepository,
    autostart: Arc<dyn AutostartPort>,
    mutation_lock: Mutex<()>,
}

impl ProductSettingsService {
    pub fn new(settings: AppSettingsRepository, autostart: Arc<dyn AutostartPort>) -> Self {
        Self {
            settings,
            autostart,
            mutation_lock: Mutex::new(()),
        }
    }

    pub fn get_general(&self, now: i64) -> Result<GeneralSettings, CommandError> {
        if now < 0 {
            return Err(invalid_input("invalidTimestamp"));
        }
        if let Some(stored) = self.read_general()? {
            return Ok(stored);
        }

        let value = GeneralSettingsValue {
            launch_at_startup: false,
        };
        match self.settings.put(GENERAL_SETTINGS_KEY, &value, None, now) {
            Ok(revision) => general_settings(value, revision, now),
            Err(error) if error.code == AppErrorCode::Conflict => self
                .read_general()?
                .ok_or_else(|| database_failure("defaultCreationRaceLost")),
            Err(error) => Err(error),
        }
    }

    pub fn save_general(
        &self,
        input: SaveGeneralSettingsInput,
        now: i64,
    ) -> Result<GeneralSettings, CommandError> {
        let _mutation = self
            .mutation_lock
            .lock()
            .map_err(|_| database_failure("generalSettingsMutationLockPoisoned"))?;
        let current = self.get_general(now)?;
        if current.revision != input.expected_revision {
            return Err(conflict());
        }

        let actual_enabled = self.autostart.is_enabled()?;
        let changed_autostart = actual_enabled != input.launch_at_startup;
        if changed_autostart {
            self.apply_autostart(input.launch_at_startup)?;
        }

        let expected_revision =
            u64::try_from(input.expected_revision).map_err(|_| invalid_input("invalidRevision"))?;
        let value = GeneralSettingsValue {
            launch_at_startup: input.launch_at_startup,
        };
        match self
            .settings
            .put(GENERAL_SETTINGS_KEY, &value, Some(expected_revision), now)
        {
            Ok(revision) => general_settings(value, revision, now),
            Err(commit_error) => {
                if changed_autostart {
                    if let Err(compensation_error) = self.apply_autostart(actual_enabled) {
                        log::error!(
                            target: "aiceland::autostart",
                            "stage=compensation status=failed commit_error={} compensation_error={}",
                            commit_error.message_key,
                            compensation_error.message_key
                        );
                        return Err(autostart_compensation_failed());
                    }
                }
                Err(commit_error)
            }
        }
    }

    pub fn reconcile_startup(&self, now: i64) -> Result<(), CommandError> {
        let _mutation = self
            .mutation_lock
            .lock()
            .map_err(|_| database_failure("generalSettingsMutationLockPoisoned"))?;
        let desired = self.get_general(now)?.launch_at_startup;
        if self.autostart.is_enabled()? != desired {
            self.apply_autostart(desired)?;
        }
        Ok(())
    }

    fn read_general(&self) -> Result<Option<GeneralSettings>, CommandError> {
        self.settings
            .get_with_metadata::<GeneralSettingsValue>(GENERAL_SETTINGS_KEY)?
            .map(|(value, revision, updated_at)| general_settings(value, revision, updated_at))
            .transpose()
    }

    fn apply_autostart(&self, enabled: bool) -> Result<(), CommandError> {
        if enabled {
            self.autostart.enable()
        } else {
            self.autostart.disable()
        }
    }
}

fn general_settings(
    value: GeneralSettingsValue,
    revision: u64,
    updated_at: i64,
) -> Result<GeneralSettings, CommandError> {
    Ok(GeneralSettings {
        launch_at_startup: value.launch_at_startup,
        revision: i64::try_from(revision).map_err(|_| database_failure("invalidRevision"))?,
        updated_at,
    })
}

fn invalid_input(reason_code: &str) -> CommandError {
    CommandError::with_detail(
        AppErrorCode::InvalidInput,
        "errors.invalidInput",
        "reasonCode",
        SafeParameterValue::String(reason_code.into()),
        false,
    )
}

fn conflict() -> CommandError {
    CommandError {
        code: AppErrorCode::Conflict,
        message_key: "errors.conflict".into(),
        details: Default::default(),
        retryable: false,
    }
}

fn database_failure(reason_code: &str) -> CommandError {
    CommandError::with_detail(
        AppErrorCode::DatabaseFailure,
        "errors.databaseFailure",
        "reasonCode",
        SafeParameterValue::String(reason_code.into()),
        false,
    )
}

fn autostart_compensation_failed() -> CommandError {
    CommandError::with_detail(
        AppErrorCode::SourceUnavailable,
        "errors.sourceUnavailable",
        "reasonCode",
        SafeParameterValue::String("autostartCompensationFailed".into()),
        false,
    )
}

fn map_autostart_error(error: tauri_plugin_autostart::Error) -> CommandError {
    match error {
        tauri_plugin_autostart::Error::Io(error)
            if error.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            CommandError::with_detail(
                AppErrorCode::PermissionDenied,
                "errors.permissionDenied",
                "reasonCode",
                SafeParameterValue::String("autostartPermissionDenied".into()),
                false,
            )
        }
        _ => CommandError {
            code: AppErrorCode::SourceUnavailable,
            message_key: "errors.sourceUnavailable".into(),
            details: [
                (
                    "serviceId".into(),
                    SafeParameterValue::String("autostart".into()),
                ),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String("autostartUnavailable".into()),
                ),
            ]
            .into(),
            retryable: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{AppErrorCode, SafeParameterValue};
    use crate::storage::Storage;
    use std::sync::{mpsc, Barrier, Mutex};
    use std::thread;
    use std::time::Duration;

    type Hook = Arc<dyn Fn() + Send + Sync>;

    #[derive(Default)]
    struct FakeAutostart {
        enabled: Mutex<bool>,
        calls: Mutex<Vec<&'static str>>,
        fail_action: Mutex<Option<&'static str>>,
        enable_hook: Mutex<Option<Hook>>,
    }

    impl FakeAutostart {
        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().unwrap().clone()
        }

        fn set_fail_action(&self, action: &'static str) {
            *self.fail_action.lock().unwrap() = Some(action);
        }

        fn set_enable_hook(&self, hook: Hook) {
            *self.enable_hook.lock().unwrap() = Some(hook);
        }

        fn maybe_fail(&self, action: &'static str) -> Result<(), CommandError> {
            if *self.fail_action.lock().unwrap() == Some(action) {
                return Err(CommandError::with_detail(
                    AppErrorCode::SourceUnavailable,
                    "errors.sourceUnavailable",
                    "reasonCode",
                    SafeParameterValue::String("autostartUnavailable".into()),
                    true,
                ));
            }
            Ok(())
        }
    }

    impl AutostartPort for FakeAutostart {
        fn is_enabled(&self) -> Result<bool, CommandError> {
            self.calls.lock().unwrap().push("is_enabled");
            self.maybe_fail("is_enabled")?;
            Ok(*self.enabled.lock().unwrap())
        }

        fn enable(&self) -> Result<(), CommandError> {
            self.calls.lock().unwrap().push("enable");
            self.maybe_fail("enable")?;
            *self.enabled.lock().unwrap() = true;
            if let Some(hook) = self.enable_hook.lock().unwrap().take() {
                hook();
            }
            Ok(())
        }

        fn disable(&self) -> Result<(), CommandError> {
            self.calls.lock().unwrap().push("disable");
            self.maybe_fail("disable")?;
            *self.enabled.lock().unwrap() = false;
            Ok(())
        }
    }

    fn fixture() -> (
        ProductSettingsService,
        AppSettingsRepository,
        Arc<FakeAutostart>,
    ) {
        let directory = tempfile::tempdir().unwrap().keep();
        let repository = AppSettingsRepository::new(Arc::new(Storage::open(&directory).unwrap()));
        let autostart = Arc::new(FakeAutostart::default());
        (
            ProductSettingsService::new(repository.clone(), autostart.clone()),
            repository,
            autostart,
        )
    }

    #[test]
    fn first_read_creates_the_false_default_once_without_touching_windows() {
        let (service, repository, autostart) = fixture();

        assert_eq!(
            service.get_general(10).unwrap(),
            GeneralSettings {
                launch_at_startup: false,
                revision: 1,
                updated_at: 10,
            }
        );
        assert_eq!(service.get_general(20).unwrap().updated_at, 10);
        assert_eq!(autostart.calls(), Vec::<&str>::new());
        assert_eq!(
            repository
                .get_with_metadata::<GeneralSettingsValue>(GENERAL_SETTINGS_KEY)
                .unwrap(),
            Some((
                GeneralSettingsValue {
                    launch_at_startup: false,
                },
                1,
                10,
            ))
        );
    }

    #[test]
    fn stale_revision_is_rejected_before_any_autostart_side_effect() {
        let (service, _, autostart) = fixture();
        service.get_general(10).unwrap();

        let error = service
            .save_general(
                SaveGeneralSettingsInput {
                    launch_at_startup: true,
                    expected_revision: 0,
                },
                11,
            )
            .unwrap_err();

        assert_eq!(error.code, AppErrorCode::Conflict);
        assert!(autostart.calls().is_empty());
        assert!(!service.get_general(12).unwrap().launch_at_startup);
    }

    #[test]
    fn enable_happens_before_commit_and_side_effect_failure_leaves_row_unchanged() {
        let (service, _, autostart) = fixture();
        service.get_general(10).unwrap();
        autostart.set_fail_action("enable");

        let error = service
            .save_general(
                SaveGeneralSettingsInput {
                    launch_at_startup: true,
                    expected_revision: 1,
                },
                11,
            )
            .unwrap_err();

        assert_eq!(error.code, AppErrorCode::SourceUnavailable);
        assert_eq!(autostart.calls(), vec!["is_enabled", "enable"]);
        assert_eq!(service.get_general(12).unwrap().revision, 1);
        assert!(!service.get_general(12).unwrap().launch_at_startup);
    }

    #[test]
    fn a_competing_commit_after_enable_causes_inverse_compensation() {
        let (service, repository, autostart) = fixture();
        service.get_general(10).unwrap();
        let competing_repository = repository.clone();
        autostart.set_enable_hook(Arc::new(move || {
            competing_repository
                .put(
                    GENERAL_SETTINGS_KEY,
                    &GeneralSettingsValue {
                        launch_at_startup: false,
                    },
                    Some(1),
                    11,
                )
                .unwrap();
        }));

        let error = service
            .save_general(
                SaveGeneralSettingsInput {
                    launch_at_startup: true,
                    expected_revision: 1,
                },
                12,
            )
            .unwrap_err();

        assert_eq!(error.code, AppErrorCode::Conflict);
        assert_eq!(autostart.calls(), vec!["is_enabled", "enable", "disable"]);
        assert!(!*autostart.enabled.lock().unwrap());
        let persisted = service.get_general(13).unwrap();
        assert!(!persisted.launch_at_startup);
        assert_eq!(persisted.revision, 2);
    }

    #[test]
    fn concurrent_true_saves_keep_the_persisted_winner_and_windows_state_aligned() {
        let (service, _, autostart) = fixture();
        let service = Arc::new(service);
        service.get_general(10).unwrap();
        let enabled = Arc::new(Barrier::new(2));
        let release_first = Arc::new(Barrier::new(2));
        let hook_enabled = enabled.clone();
        let hook_release = release_first.clone();
        autostart.set_enable_hook(Arc::new(move || {
            hook_enabled.wait();
            hook_release.wait();
        }));

        let first_service = service.clone();
        let first = thread::spawn(move || {
            first_service.save_general(
                SaveGeneralSettingsInput {
                    launch_at_startup: true,
                    expected_revision: 1,
                },
                11,
            )
        });
        enabled.wait();

        let second_service = service.clone();
        let (second_done_tx, second_done_rx) = mpsc::channel();
        let second = thread::spawn(move || {
            let result = second_service.save_general(
                SaveGeneralSettingsInput {
                    launch_at_startup: true,
                    expected_revision: 1,
                },
                12,
            );
            second_done_tx.send(result).unwrap();
        });
        let second_before_release = second_done_rx.recv_timeout(Duration::from_secs(2)).ok();
        release_first.wait();

        let first_result = first.join().unwrap();
        let second_result = second_before_release
            .unwrap_or_else(|| second_done_rx.recv_timeout(Duration::from_secs(2)).unwrap());
        second.join().unwrap();
        let results = [first_result, second_result];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| result
                    .as_ref()
                    .is_err_and(|error| error.code == AppErrorCode::Conflict))
                .count(),
            1
        );
        let persisted = service.get_general(13).unwrap();
        assert!(persisted.launch_at_startup);
        assert_eq!(persisted.revision, 2);
        assert!(*autostart.enabled.lock().unwrap());
    }

    #[test]
    fn startup_reconcile_applies_the_persisted_desired_state_once() {
        let (service, repository, autostart) = fixture();
        repository
            .put(
                GENERAL_SETTINGS_KEY,
                &GeneralSettingsValue {
                    launch_at_startup: true,
                },
                None,
                10,
            )
            .unwrap();

        service.reconcile_startup(11).unwrap();

        assert_eq!(autostart.calls(), vec!["is_enabled", "enable"]);
        assert!(*autostart.enabled.lock().unwrap());
    }
}
