use crate::contracts::{
    AppErrorCode, AppSnapshot, CommandError, DiagnosticEvent, DiagnosticLevel, SafeParameterValue,
    ServiceHealthSnapshot, StorageIntegrity, StorageIntegrityResult,
};
use crate::events::FOUNDATION_STORAGE_SERVICE_ID;
use crate::services::{persist_foundation_storage_health, AppServices};
use crate::storage::Storage;
use std::collections::BTreeMap;
use std::sync::Arc;

macro_rules! define_foundation_commands {
    (
        $get_app_snapshot_wire:ident => $get_app_snapshot_implementation:ident,
        $list_service_health_wire:ident => $list_service_health_implementation:ident,
        $get_diagnostics_wire:ident => $get_diagnostics_implementation:ident,
        $check_storage_integrity_wire:ident => $check_storage_integrity_implementation:ident,
    ) => {
        #[tauri::command(rename_all = "camelCase")]
        #[allow(non_snake_case)]
        pub fn $get_app_snapshot_wire(
            services: tauri::State<'_, Arc<AppServices>>,
        ) -> Result<AppSnapshot, CommandError> {
            $get_app_snapshot_implementation(services)
        }

        #[tauri::command(rename_all = "camelCase")]
        #[allow(non_snake_case)]
        pub fn $list_service_health_wire(
            services: tauri::State<'_, Arc<AppServices>>,
        ) -> Result<Vec<ServiceHealthSnapshot>, CommandError> {
            $list_service_health_implementation(services)
        }

        #[tauri::command(rename_all = "camelCase")]
        #[allow(non_snake_case)]
        pub fn $get_diagnostics_wire(
            limit: u32,
            services: tauri::State<'_, Arc<AppServices>>,
        ) -> Result<Vec<DiagnosticEvent>, CommandError> {
            $get_diagnostics_implementation(limit, services)
        }

        #[tauri::command(rename_all = "camelCase")]
        #[allow(non_snake_case)]
        pub fn $check_storage_integrity_wire(
            services: tauri::State<'_, Arc<AppServices>>,
        ) -> Result<StorageIntegrityResult, CommandError> {
            $check_storage_integrity_implementation(services)
        }
    };
}

crate::commands::foundation_command_manifest!(define_foundation_commands;);

fn get_app_snapshot(
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<AppSnapshot, CommandError> {
    build_app_snapshot(services.inner().as_ref(), crate::native_locale()?)
}

fn list_service_health(
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<Vec<ServiceHealthSnapshot>, CommandError> {
    services.health.list()
}

fn get_diagnostics(
    limit: u32,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<Vec<DiagnosticEvent>, CommandError> {
    validate_diagnostic_limit(limit)?;
    services.diagnostics.list(limit)
}

fn check_storage_integrity(
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<StorageIntegrityResult, CommandError> {
    let checked_at = now_millis();
    let result = match run_integrity_check(&services.storage, checked_at) {
        Ok(result) => result,
        Err(_) => {
            record_integrity_failure(services.inner().as_ref(), checked_at);
            return Err(storage_integrity_error());
        }
    };
    persist_storage_health_then_emit(services.inner().as_ref(), checked_at)?;
    Ok(result)
}

fn build_app_snapshot(
    services: &AppServices,
    locale: crate::contracts::Locale,
) -> Result<AppSnapshot, CommandError> {
    Ok(AppSnapshot {
        locale,
        modules: services.modules.snapshot()?,
        services: services.health.list()?,
        storage_schema_version: i64::from(services.storage.schema_version()?),
    })
}

fn validate_diagnostic_limit(limit: u32) -> Result<(), CommandError> {
    if (1..=500).contains(&limit) {
        Ok(())
    } else {
        Err(CommandError::with_detail(
            AppErrorCode::InvalidInput,
            "errors.invalidInput",
            "reasonCode",
            SafeParameterValue::String("invalidDiagnosticLimit".into()),
            false,
        ))
    }
}

fn run_integrity_check(
    storage: &Storage,
    checked_at: i64,
) -> Result<StorageIntegrityResult, CommandError> {
    storage.integrity_check()?;
    Ok(StorageIntegrityResult {
        integrity: StorageIntegrity::Ok,
        schema_version: i64::from(storage.schema_version()?),
        checked_at,
    })
}

fn persist_storage_health_then_emit(
    services: &AppServices,
    checked_at: i64,
) -> Result<(), CommandError> {
    persist_foundation_storage_health(&services.health, checked_at)?;

    if services
        .emit_service_health_changed(FOUNDATION_STORAGE_SERVICE_ID, checked_at)
        .is_err()
    {
        record_health_emit_failure(services, checked_at);
    }
    Ok(())
}

fn record_integrity_failure(services: &AppServices, checked_at: i64) {
    let _ = services.diagnostics.record(&DiagnosticEvent {
        id: uuid::Uuid::new_v4().to_string(),
        service_id: FOUNDATION_STORAGE_SERVICE_ID.into(),
        level: DiagnosticLevel::Failure,
        code: "storage.integrityFailed".into(),
        parameters: BTreeMap::from([
            (
                "serviceId".into(),
                SafeParameterValue::String(FOUNDATION_STORAGE_SERVICE_ID.into()),
            ),
            (
                "reasonCode".into(),
                SafeParameterValue::String("integrityFailed".into()),
            ),
        ]),
        created_at: checked_at,
    });
}

fn record_health_emit_failure(services: &AppServices, checked_at: i64) {
    let _ = services.diagnostics.record(&DiagnosticEvent {
        id: uuid::Uuid::new_v4().to_string(),
        service_id: FOUNDATION_STORAGE_SERVICE_ID.into(),
        level: DiagnosticLevel::Failure,
        code: "events.serviceHealthEmitFailed".into(),
        parameters: BTreeMap::from([
            (
                "serviceId".into(),
                SafeParameterValue::String(FOUNDATION_STORAGE_SERVICE_ID.into()),
            ),
            (
                "reasonCode".into(),
                SafeParameterValue::String("emitFailed".into()),
            ),
            ("count".into(), SafeParameterValue::Number(1.into())),
        ]),
        created_at: checked_at,
    });
}

fn storage_integrity_error() -> CommandError {
    CommandError::with_detail(
        AppErrorCode::StorageUnavailable,
        "errors.storageUnavailable",
        "reasonCode",
        SafeParameterValue::String("integrityFailed".into()),
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
    use super::super::FOUNDATION_COMMAND_NAMES;
    use super::{
        build_app_snapshot, persist_storage_health_then_emit, run_integrity_check,
        validate_diagnostic_limit,
    };
    use crate::contracts::{
        AppErrorCode, Locale, ModuleId, SafeParameterValue, ServiceHealthSnapshot,
        ServiceHealthState,
    };
    use crate::repositories::service_health::ServiceHealthRepository;
    use crate::services::{
        AppServices, BootstrapModuleStateProvider, EventEmitterPort, ShutdownPort,
        WalCheckpointPort,
    };
    use crate::storage::Storage;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    struct TestShutdownPort;

    #[async_trait::async_trait]
    impl ShutdownPort for TestShutdownPort {
        async fn stop_accepting_work(&self) -> Result<(), crate::contracts::CommandError> {
            Ok(())
        }

        async fn stop_optional_modules(&self) -> Result<(), crate::contracts::CommandError> {
            Ok(())
        }

        async fn cancel_core_workers(&self) -> Result<(), crate::contracts::CommandError> {
            Ok(())
        }
    }

    struct TestCheckpointPort;

    impl WalCheckpointPort for TestCheckpointPort {
        fn checkpoint_truncate(&self) -> Result<(), crate::contracts::CommandError> {
            Ok(())
        }
    }

    struct TestEmitter;

    impl EventEmitterPort for TestEmitter {
        fn emit(
            &self,
            _: &'static str,
            _: serde_json::Value,
        ) -> Result<(), crate::contracts::CommandError> {
            Ok(())
        }
    }

    struct HealthAwareRejectingEmitter {
        health: ServiceHealthRepository,
        observed: Arc<Mutex<Vec<(&'static str, serde_json::Value)>>>,
    }

    impl EventEmitterPort for HealthAwareRejectingEmitter {
        fn emit(
            &self,
            event_name: &'static str,
            payload: serde_json::Value,
        ) -> Result<(), crate::contracts::CommandError> {
            assert_eq!(event_name, "serviceHealthChanged");
            assert_eq!(
                payload,
                serde_json::json!({
                    "serviceId": "foundation-storage",
                    "checkedAt": 77,
                })
            );
            assert_eq!(
                self.health
                    .list()
                    .unwrap()
                    .into_iter()
                    .map(|snapshot| (snapshot.service_id, snapshot.checked_at))
                    .collect::<Vec<_>>(),
                vec![("foundation-storage".into(), 77)]
            );
            self.observed.lock().unwrap().push((event_name, payload));
            Err(crate::contracts::CommandError::with_detail(
                AppErrorCode::SourceUnavailable,
                "errors.sourceUnavailable",
                "reasonCode",
                SafeParameterValue::String("emitFailed".into()),
                false,
            ))
        }
    }

    fn fixture_services() -> Arc<AppServices> {
        let directory = tempfile::tempdir().unwrap().keep();
        let storage = Arc::new(Storage::open(&directory).unwrap());
        AppServices::from_parts(
            storage,
            Arc::new(BootstrapModuleStateProvider),
            Arc::new(TestShutdownPort),
            Arc::new(TestCheckpointPort),
            Arc::new(TestEmitter),
        )
    }

    fn fixture_services_with_rejecting_emitter() -> (
        Arc<AppServices>,
        Arc<Mutex<Vec<(&'static str, serde_json::Value)>>>,
    ) {
        let directory = tempfile::tempdir().unwrap().keep();
        let storage = Arc::new(Storage::open(&directory).unwrap());
        let observed = Arc::new(Mutex::new(Vec::new()));
        let health = ServiceHealthRepository::new(storage.clone());
        let services = AppServices::from_parts(
            storage,
            Arc::new(BootstrapModuleStateProvider),
            Arc::new(TestShutdownPort),
            Arc::new(TestCheckpointPort),
            Arc::new(HealthAwareRejectingEmitter {
                health,
                observed: observed.clone(),
            }),
        );
        (services, observed)
    }

    #[test]
    fn app_snapshot_excludes_retired_products_and_uses_current_schema() {
        let services = fixture_services();
        services
            .health
            .upsert(&ServiceHealthSnapshot {
                service_id: "zeta".into(),
                state: ServiceHealthState::Healthy,
                message_key: "services.healthy".into(),
                parameters: BTreeMap::from([(
                    "serviceId".into(),
                    SafeParameterValue::String("zeta".into()),
                )]),
                checked_at: 20,
            })
            .unwrap();
        services
            .health
            .upsert(&ServiceHealthSnapshot {
                service_id: "alpha".into(),
                state: ServiceHealthState::Healthy,
                message_key: "services.healthy".into(),
                parameters: BTreeMap::from([(
                    "serviceId".into(),
                    SafeParameterValue::String("alpha".into()),
                )]),
                checked_at: 10,
            })
            .unwrap();

        let snapshot = build_app_snapshot(&services, Locale::ZhCn).unwrap();

        assert_eq!(snapshot.locale, Locale::ZhCn);
        assert_eq!(snapshot.modules.len(), 4);
        assert!(!snapshot.modules.contains_key(&ModuleId::Todo));
        assert!(!snapshot.modules.contains_key(&ModuleId::Media));
        assert!(snapshot
            .modules
            .values()
            .all(|preference| preference.visible));
        assert_eq!(
            snapshot
                .services
                .iter()
                .map(|health| health.service_id.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "zeta"]
        );
        assert_eq!(snapshot.storage_schema_version, 10);
    }

    #[test]
    fn diagnostics_limit_zero_is_rejected_before_repository_access() {
        let error = validate_diagnostic_limit(0).unwrap_err();

        assert_eq!(error.code, AppErrorCode::InvalidInput);
        assert_eq!(error.message_key, "errors.invalidInput");
        assert_eq!(
            error.details.get("reasonCode"),
            Some(&SafeParameterValue::String("invalidDiagnosticLimit".into()))
        );
        assert!(!error.retryable);
    }

    #[test]
    fn integrity_check_is_read_only_and_returns_the_typed_healthy_result() {
        let services = fixture_services();
        let changes_before = services
            .storage
            .with_connection(|connection| Ok(connection.total_changes()))
            .unwrap();

        let result = run_integrity_check(&services.storage, 42).unwrap();

        let changes_after = services
            .storage
            .with_connection(|connection| Ok(connection.total_changes()))
            .unwrap();
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::json!({
                "integrity": "ok",
                "schemaVersion": 10,
                "checkedAt": 42,
            })
        );
        assert_eq!(changes_after, changes_before);
    }

    #[test]
    fn fresh_storage_health_seed_is_replaced_by_integrity_check_not_duplicated() {
        let services = fixture_services();
        services
            .health
            .upsert(&ServiceHealthSnapshot {
                service_id: "foundation-storage".into(),
                state: ServiceHealthState::Healthy,
                message_key: "services.healthy".into(),
                parameters: BTreeMap::from([(
                    "serviceId".into(),
                    SafeParameterValue::String("foundation-storage".into()),
                )]),
                checked_at: 1,
            })
            .unwrap();

        persist_storage_health_then_emit(&services, 42).unwrap();

        assert_eq!(
            services
                .health
                .list()
                .unwrap()
                .into_iter()
                .map(|health| (health.service_id, health.checked_at))
                .collect::<Vec<_>>(),
            vec![("foundation-storage".into(), 42)]
        );
    }

    #[test]
    fn health_commit_survives_emit_failure_and_records_an_allowlisted_diagnostic() {
        let (services, observed) = fixture_services_with_rejecting_emitter();

        persist_storage_health_then_emit(&services, 77).unwrap();

        let health = services.health.list().unwrap();
        assert_eq!(health.len(), 1);
        assert_eq!(health[0].service_id, "foundation-storage");
        assert_eq!(health[0].checked_at, 77);
        assert_eq!(
            *observed.lock().unwrap(),
            vec![(
                "serviceHealthChanged",
                serde_json::json!({
                    "serviceId": "foundation-storage",
                    "checkedAt": 77,
                })
            )]
        );

        let diagnostics = services.diagnostics.list(1).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].service_id, "foundation-storage");
        assert_eq!(diagnostics[0].code, "events.serviceHealthEmitFailed");
        assert_eq!(
            diagnostics[0].parameters,
            BTreeMap::from([
                (
                    "serviceId".into(),
                    SafeParameterValue::String("foundation-storage".into()),
                ),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String("emitFailed".into()),
                ),
                ("count".into(), SafeParameterValue::Number(1.into())),
            ])
        );
    }

    #[test]
    fn foundation_command_names_are_the_exact_camel_case_boundary() {
        assert_eq!(
            FOUNDATION_COMMAND_NAMES,
            [
                "getAppSnapshot",
                "listServiceHealth",
                "getDiagnostics",
                "checkStorageIntegrity",
            ]
        );
    }

    #[test]
    fn command_manifest_keeps_camel_case_wrappers_and_implementations_together() {
        macro_rules! manifest_entries {
            ($($wire_name:ident => $implementation:ident),+ $(,)?) => {
                vec![$((stringify!($wire_name), stringify!($implementation))),+]
            };
        }

        let entries = crate::commands::foundation_command_manifest!(manifest_entries);

        assert_eq!(
            entries,
            vec![
                ("getAppSnapshot", "get_app_snapshot"),
                ("listServiceHealth", "list_service_health"),
                ("getDiagnostics", "get_diagnostics"),
                ("checkStorageIntegrity", "check_storage_integrity"),
            ]
        );
    }
}
