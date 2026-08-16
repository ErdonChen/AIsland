use crate::contracts::{
    AgentEnvironment, AgentId, AgentIntegrationResult, AppErrorCode, CommandError, DiagnosticEvent,
    DiagnosticLevel, SafeParameterValue,
};
use crate::domain::agents::AgentIntegrationEntity;
use crate::repositories::{agents::AgentRepository, diagnostics::DiagnosticsRepository};
use crate::services::agent_hook_assets::{windows_hook_command, wsl_hook_command, HookInvocation};
use crate::services::config_merge::{
    inspect_config, merge_config, ConfigFormat, MergeAction, OwnedHookFragment,
};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct FixedIntegrationDescriptor {
    pub agent_id: AgentId,
    pub environment: AgentEnvironment,
    pub config_format: ConfigFormat,
    pub config_path: PathBuf,
    pub owned_hooks: Vec<OwnedHookFragment>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntegrationInspection {
    NotInstalled,
    Installed { fingerprint: String },
    NeedsRepair { reason_code: String },
    Unsupported,
}

pub trait FixedAgentAdapter: Send + Sync {
    fn descriptor(&self) -> &FixedIntegrationDescriptor;
    fn inspect(&self) -> Result<IntegrationInspection, CommandError>;
    fn install(&self, now: i64) -> Result<AgentIntegrationResult, CommandError>;
    fn repair(&self, now: i64) -> Result<AgentIntegrationResult, CommandError>;
    fn uninstall(
        &self,
        confirm_owned_removal: bool,
        now: i64,
    ) -> Result<AgentIntegrationResult, CommandError>;
}

pub struct AgentIntegrationService {
    adapters: Vec<Arc<dyn FixedAgentAdapter>>,
    pub repository: AgentRepository,
}
impl AgentIntegrationService {
    pub fn new(
        repository: AgentRepository,
        diagnostics: DiagnosticsRepository,
        windows_home: &Path,
        app_data_dir: &Path,
        wsl_home: &str,
        wsl_status_dir: &str,
        wsl_helper: String,
    ) -> Self {
        let local_filesystem: Arc<dyn ConfigFilesystem> = Arc::new(LocalConfigFilesystem::new());
        let wsl_filesystem: Arc<dyn ConfigFilesystem> =
            Arc::new(WslConfigFilesystem::new(wsl_helper));
        let store: Arc<dyn IntegrationStore> = Arc::new(repository.clone());
        let diagnostics: Arc<dyn IntegrationDiagnosticPort> = Arc::new(diagnostics);
        let adapters = fixed_descriptors(windows_home, app_data_dir, wsl_home, wsl_status_dir)
            .into_iter()
            .map(|descriptor| {
                let filesystem = match descriptor.environment {
                    AgentEnvironment::Windows => local_filesystem.clone(),
                    AgentEnvironment::Wsl => wsl_filesystem.clone(),
                };
                Arc::new(FileAdapter::new(
                    descriptor,
                    filesystem,
                    store.clone(),
                    diagnostics.clone(),
                )) as Arc<dyn FixedAgentAdapter>
            })
            .collect();
        Self {
            adapters,
            repository,
        }
    }
    pub fn adapter(
        &self,
        agent: AgentId,
        environment: AgentEnvironment,
    ) -> Result<&Arc<dyn FixedAgentAdapter>, CommandError> {
        self.adapters
            .iter()
            .find(|adapter| {
                adapter.descriptor().agent_id == agent
                    && adapter.descriptor().environment == environment
            })
            .ok_or_else(unsupported)
    }

    pub fn install(
        &self,
        agent: AgentId,
        environment: AgentEnvironment,
        now: i64,
    ) -> Result<AgentIntegrationResult, CommandError> {
        if unsupported_pair(&agent, &environment) {
            return Ok(unsupported_result_for(agent, environment));
        }
        self.adapter(agent, environment)?.install(now)
    }

    pub fn repair(
        &self,
        agent: AgentId,
        environment: AgentEnvironment,
        now: i64,
    ) -> Result<AgentIntegrationResult, CommandError> {
        if unsupported_pair(&agent, &environment) {
            return Ok(unsupported_result_for(agent, environment));
        }
        self.adapter(agent, environment)?.repair(now)
    }

    pub fn uninstall(
        &self,
        agent: AgentId,
        environment: AgentEnvironment,
        confirm_owned_removal: bool,
        now: i64,
    ) -> Result<AgentIntegrationResult, CommandError> {
        if unsupported_pair(&agent, &environment) {
            return Ok(unsupported_result_for(agent, environment));
        }
        self.adapter(agent, environment)?
            .uninstall(confirm_owned_removal, now)
    }
}

pub trait ConfigFilesystem: Send + Sync {
    fn read(&self, path: &Path) -> Result<Vec<u8>, CommandError>;
    fn backup_create(&self, path: &Path, now: i64) -> Result<PathBuf, CommandError>;
    fn backup_write(&self, backup: &Path, bytes: &[u8]) -> Result<(), CommandError>;
    fn backup_flush(&self, backup: &Path) -> Result<(), CommandError>;
    fn temp_create(&self, path: &Path) -> Result<PathBuf, CommandError>;
    fn temp_write(&self, temporary: &Path, bytes: &[u8]) -> Result<(), CommandError>;
    fn temp_flush(&self, temporary: &Path) -> Result<(), CommandError>;
    fn replace(&self, temporary: &Path, path: &Path) -> Result<(), CommandError>;
    fn restore(&self, path: &Path, bytes: &[u8]) -> Result<(), CommandError>;
}
pub trait IntegrationStore: Send + Sync {
    fn get(
        &self,
        agent: AgentId,
        environment: AgentEnvironment,
    ) -> Result<Option<AgentIntegrationEntity>, CommandError>;
    fn put(
        &self,
        record: &AgentIntegrationEntity,
        expected: Option<u64>,
    ) -> Result<AgentIntegrationEntity, CommandError>;
}
pub trait IntegrationDiagnosticPort: Send + Sync {
    fn rollback_failed(&self, agent: &AgentId, environment: &AgentEnvironment, now: i64);
}
impl IntegrationDiagnosticPort for DiagnosticsRepository {
    fn rollback_failed(&self, agent: &AgentId, environment: &AgentEnvironment, now: i64) {
        let mut parameters = std::collections::BTreeMap::new();
        parameters.insert(
            "agentName".into(),
            SafeParameterValue::String(agent.display_name().into()),
        );
        parameters.insert(
            "environment".into(),
            SafeParameterValue::String(environment_name(environment).into()),
        );
        parameters.insert(
            "reasonCode".into(),
            SafeParameterValue::String("rollbackFailed".into()),
        );
        let _ = self.record(&DiagnosticEvent {
            id: format!("integration-rollback-{now}"),
            service_id: "agent-integrations".into(),
            level: DiagnosticLevel::Failure,
            code: "integration.rollbackFailed".into(),
            parameters,
            created_at: now,
        });
    }
}
impl IntegrationStore for AgentRepository {
    fn get(
        &self,
        a: AgentId,
        e: AgentEnvironment,
    ) -> Result<Option<AgentIntegrationEntity>, CommandError> {
        self.get_integration(a, e)
    }
    fn put(
        &self,
        r: &AgentIntegrationEntity,
        x: Option<u64>,
    ) -> Result<AgentIntegrationEntity, CommandError> {
        self.put_integration(r, x)
    }
}

#[derive(Default)]
pub struct LocalConfigFilesystem {
    open_files: Mutex<HashMap<PathBuf, File>>,
}
impl LocalConfigFilesystem {
    pub fn new() -> Self {
        Self::default()
    }
    fn create_new(&self, path: &Path, reason: &str) -> Result<(), CommandError> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|_| io(reason))?;
        self.open_files
            .lock()
            .unwrap()
            .insert(path.to_owned(), file);
        Ok(())
    }
    fn write_open(&self, path: &Path, bytes: &[u8], reason: &str) -> Result<(), CommandError> {
        self.open_files
            .lock()
            .unwrap()
            .get_mut(path)
            .ok_or_else(|| io(reason))?
            .write_all(bytes)
            .map_err(|_| io(reason))
    }
    fn flush_open(&self, path: &Path, reason: &str) -> Result<(), CommandError> {
        let file = self
            .open_files
            .lock()
            .unwrap()
            .remove(path)
            .ok_or_else(|| io(reason))?;
        file.sync_all().map_err(|_| io(reason))
    }
}
impl ConfigFilesystem for LocalConfigFilesystem {
    fn read(&self, path: &Path) -> Result<Vec<u8>, CommandError> {
        fs::read(path).map_err(|_| io("read"))
    }
    fn backup_create(&self, path: &Path, now: i64) -> Result<PathBuf, CommandError> {
        let suffix = format!(".aiceland-backup-{}", timestamp(now));
        let backup = PathBuf::from(format!("{}{}", path.display(), suffix));
        self.create_new(&backup, "backupCreate")?;
        Ok(backup)
    }
    fn backup_write(&self, backup: &Path, bytes: &[u8]) -> Result<(), CommandError> {
        self.write_open(backup, bytes, "backupWrite")
    }
    fn backup_flush(&self, backup: &Path) -> Result<(), CommandError> {
        self.flush_open(backup, "backupFlush")
    }
    fn temp_create(&self, path: &Path) -> Result<PathBuf, CommandError> {
        let parent = path.parent().ok_or_else(|| io("parent"))?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| io("name"))?;
        let temporary = parent.join(format!(".{name}.aiceland-tmp-{}", unique_nonce()));
        self.create_new(&temporary, "tempCreate")?;
        Ok(temporary)
    }
    fn temp_write(&self, temporary: &Path, bytes: &[u8]) -> Result<(), CommandError> {
        self.write_open(temporary, bytes, "tempWrite")
    }
    fn temp_flush(&self, temporary: &Path) -> Result<(), CommandError> {
        self.flush_open(temporary, "tempFlush")
    }
    fn replace(&self, temporary: &Path, path: &Path) -> Result<(), CommandError> {
        fs::rename(temporary, path).map_err(|_| io("replace"))
    }
    fn restore(&self, path: &Path, bytes: &[u8]) -> Result<(), CommandError> {
        let temporary = self.temp_create(path)?;
        self.temp_write(&temporary, bytes)?;
        self.temp_flush(&temporary)?;
        self.replace(&temporary, path)
    }
}

/// WSL config operations are delegated to the package-owned closed helper. Paths and action are
/// separate process arguments; no configuration value is evaluated as shell text on Windows.
pub struct WslConfigFilesystem {
    helper: String,
    pending_backups: Mutex<HashMap<PathBuf, PathBuf>>,
    pending_temps: Mutex<HashMap<PathBuf, Vec<u8>>>,
}
impl WslConfigFilesystem {
    pub fn new(helper: String) -> Self {
        Self {
            helper,
            pending_backups: Mutex::new(HashMap::new()),
            pending_temps: Mutex::new(HashMap::new()),
        }
    }
    fn run(
        &self,
        action: &str,
        path: &Path,
        input: Option<&[u8]>,
        extra: Option<&Path>,
    ) -> Result<Vec<u8>, CommandError> {
        let mut command = std::process::Command::new("wsl.exe");
        command
            .args(["--exec", "sh", self.helper.as_str(), action])
            .arg(path);
        if let Some(extra) = extra {
            command.arg(extra);
        }
        if input.is_some() {
            command.stdin(std::process::Stdio::piped());
        }
        command.stdout(std::process::Stdio::piped());
        let mut child = command.spawn().map_err(|_| io("wslUnavailable"))?;
        if let Some(input) = input {
            child
                .stdin
                .as_mut()
                .ok_or_else(|| io("wslStdin"))?
                .write_all(input)
                .map_err(|_| io("wslWrite"))?;
        }
        let output = child.wait_with_output().map_err(|_| io("wslWait"))?;
        if !output.status.success() {
            return Err(io("wslHelper"));
        };
        Ok(output.stdout)
    }
}
impl ConfigFilesystem for WslConfigFilesystem {
    fn read(&self, path: &Path) -> Result<Vec<u8>, CommandError> {
        self.run("read", path, None, None)
    }
    fn backup_create(&self, path: &Path, now: i64) -> Result<PathBuf, CommandError> {
        let backup = PathBuf::from(format!(
            "{}{}",
            path.display(),
            format!(".aiceland-backup-{}", timestamp(now))
        ));
        self.pending_backups
            .lock()
            .unwrap()
            .insert(backup.clone(), path.to_owned());
        Ok(backup)
    }
    fn backup_write(&self, backup: &Path, bytes: &[u8]) -> Result<(), CommandError> {
        let source = self
            .pending_backups
            .lock()
            .unwrap()
            .get(backup)
            .cloned()
            .ok_or_else(|| io("wslBackupCreate"))?;
        self.run("backup", &source, None, Some(backup))?;
        if self.read(&backup)? != bytes {
            return Err(io("wslBackupVerify"));
        };
        Ok(())
    }
    fn backup_flush(&self, backup: &Path) -> Result<(), CommandError> {
        self.pending_backups.lock().unwrap().remove(backup);
        Ok(())
    }
    fn temp_create(&self, path: &Path) -> Result<PathBuf, CommandError> {
        let parent = path.parent().ok_or_else(|| io("wslTempParent"))?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| io("wslTempName"))?;
        let temporary = parent.join(format!(".{name}.aiceland-tmp-{}", unique_nonce()));
        self.pending_temps
            .lock()
            .unwrap()
            .insert(temporary.clone(), Vec::new());
        Ok(temporary)
    }
    fn temp_write(&self, temporary: &Path, bytes: &[u8]) -> Result<(), CommandError> {
        *self
            .pending_temps
            .lock()
            .unwrap()
            .get_mut(temporary)
            .ok_or_else(|| io("wslTempCreate"))? = bytes.to_vec();
        Ok(())
    }
    fn temp_flush(&self, temporary: &Path) -> Result<(), CommandError> {
        let bytes = self
            .pending_temps
            .lock()
            .unwrap()
            .remove(temporary)
            .ok_or_else(|| io("wslTempWrite"))?;
        self.run("stage", temporary, Some(&bytes), None)?;
        Ok(())
    }
    fn replace(&self, temporary: &Path, path: &Path) -> Result<(), CommandError> {
        self.run("replace", path, None, Some(temporary))?;
        Ok(())
    }
    fn restore(&self, path: &Path, bytes: &[u8]) -> Result<(), CommandError> {
        self.run("atomic-replace", path, Some(bytes), None)?;
        Ok(())
    }
}

struct FileAdapter {
    descriptor: FixedIntegrationDescriptor,
    filesystem: Arc<dyn ConfigFilesystem>,
    store: Arc<dyn IntegrationStore>,
    diagnostics: Arc<dyn IntegrationDiagnosticPort>,
    mutation_lock: Mutex<()>,
}
impl FileAdapter {
    fn new(
        descriptor: FixedIntegrationDescriptor,
        filesystem: Arc<dyn ConfigFilesystem>,
        store: Arc<dyn IntegrationStore>,
        diagnostics: Arc<dyn IntegrationDiagnosticPort>,
    ) -> Self {
        Self {
            descriptor,
            filesystem,
            store,
            diagnostics,
            mutation_lock: Mutex::new(()),
        }
    }
    fn mutate(
        &self,
        action: MergeAction,
        now: i64,
    ) -> Result<AgentIntegrationResult, CommandError> {
        if unsupported_descriptor(&self.descriptor) {
            return Ok(unsupported_result(&self.descriptor));
        }
        if matches!(action, MergeAction::Uninstall) {
            return Err(invalid());
        }
        let _mutation = self.mutation_lock.lock().unwrap();
        let before = self.filesystem.read(&self.descriptor.config_path)?;
        let (after, changed) = merge_config(
            &before,
            self.descriptor.config_format.clone(),
            &self.descriptor.owned_hooks,
            action,
        )?;
        if !changed {
            return Ok(self.result("installed", None, false));
        }
        let prior = self.store.get(
            self.descriptor.agent_id.clone(),
            self.descriptor.environment.clone(),
        )?;
        let backup = self.write_replacement(&before, &after, now)?;
        let written = match self.filesystem.read(&self.descriptor.config_path) {
            Ok(written) => written,
            Err(error) => return Err(self.restore_after_replace(&before, error, now)),
        };
        let installed = match inspect_config(
            &written,
            self.descriptor.config_format.clone(),
            &self.descriptor.owned_hooks,
        ) {
            Ok(installed) => installed,
            Err(error) => return Err(self.restore_after_replace(&before, error, now)),
        };
        if !installed {
            return Err(self.restore_after_replace(
                &before,
                integration_config_invalid(&self.descriptor, "verification"),
                now,
            ));
        }
        let record = AgentIntegrationEntity {
            agent_id: self.descriptor.agent_id.clone(),
            environment: self.descriptor.environment.clone(),
            install_state: "installed".into(),
            config_path: self.descriptor.config_path.display().to_string(),
            backup_path: Some(backup.display().to_string()),
            owned_fingerprint: Some(sha256_hex(&managed_fingerprint_material(
                &written,
                &self.descriptor,
            )?)),
            revision: prior.as_ref().map_or(0, |r| r.revision),
            updated_at: now,
        };
        if self
            .store
            .put(&record, prior.as_ref().map(|r| r.revision))
            .is_err()
        {
            return Err(self.restore_after_replace(&before, database_failure(), now));
        }
        Ok(self.result("installed", Some(backup), true))
    }

    fn write_replacement(
        &self,
        before: &[u8],
        after: &[u8],
        now: i64,
    ) -> Result<PathBuf, CommandError> {
        let path = &self.descriptor.config_path;
        let backup = self.filesystem.backup_create(path, now)?;
        self.filesystem.backup_write(&backup, before)?;
        self.filesystem.backup_flush(&backup)?;
        let temporary = self.filesystem.temp_create(path)?;
        self.filesystem.temp_write(&temporary, after)?;
        self.filesystem.temp_flush(&temporary)?;
        self.filesystem.replace(&temporary, path)?;
        Ok(backup)
    }
    fn result(
        &self,
        state: &str,
        backup: Option<PathBuf>,
        changed: bool,
    ) -> AgentIntegrationResult {
        AgentIntegrationResult {
            agent_id: self.descriptor.agent_id.clone(),
            environment: self.descriptor.environment.clone(),
            state: match state {
                "installed" => crate::contracts::IntegrationState::Installed,
                "needsRepair" => crate::contracts::IntegrationState::NeedsRepair,
                "unsupported" => crate::contracts::IntegrationState::Unsupported,
                _ => crate::contracts::IntegrationState::NotInstalled,
            },
            config_path: self.descriptor.config_path.display().to_string(),
            backup_path: backup.map(|p| p.display().to_string()),
            changed,
        }
    }

    fn restore_after_replace(
        &self,
        before: &[u8],
        original: CommandError,
        now: i64,
    ) -> CommandError {
        match self
            .filesystem
            .restore(&self.descriptor.config_path, before)
        {
            Ok(()) => original,
            Err(_) => {
                self.persist_needs_repair(now);
                self.diagnostics.rollback_failed(
                    &self.descriptor.agent_id,
                    &self.descriptor.environment,
                    now,
                );
                integration_config_invalid(&self.descriptor, "rollbackFailed")
            }
        }
    }
    fn persist_needs_repair(&self, now: i64) {
        if let Ok(prior) = self.store.get(
            self.descriptor.agent_id.clone(),
            self.descriptor.environment.clone(),
        ) {
            let record = AgentIntegrationEntity {
                agent_id: self.descriptor.agent_id.clone(),
                environment: self.descriptor.environment.clone(),
                install_state: "needsRepair".into(),
                config_path: self.descriptor.config_path.display().to_string(),
                backup_path: prior.as_ref().and_then(|row| row.backup_path.clone()),
                owned_fingerprint: None,
                revision: prior.as_ref().map_or(0, |row| row.revision),
                updated_at: now,
            };
            let _ = self.store.put(&record, prior.map(|row| row.revision));
        }
    }
}
impl FixedAgentAdapter for FileAdapter {
    fn descriptor(&self) -> &FixedIntegrationDescriptor {
        &self.descriptor
    }
    fn inspect(&self) -> Result<IntegrationInspection, CommandError> {
        if unsupported_descriptor(&self.descriptor) {
            return Ok(IntegrationInspection::Unsupported);
        };
        let bytes = match self.filesystem.read(&self.descriptor.config_path) {
            Ok(v) => v,
            Err(_) => return Ok(IntegrationInspection::NotInstalled),
        };
        if inspect_config(
            &bytes,
            self.descriptor.config_format.clone(),
            &self.descriptor.owned_hooks,
        )? {
            Ok(IntegrationInspection::Installed {
                fingerprint: sha256_hex(&managed_fingerprint_material(&bytes, &self.descriptor)?),
            })
        } else {
            Ok(IntegrationInspection::NeedsRepair {
                reason_code: "ownedHookMissingOrDrifted".into(),
            })
        }
    }
    fn install(&self, now: i64) -> Result<AgentIntegrationResult, CommandError> {
        self.mutate(MergeAction::Install, now)
    }
    fn repair(&self, now: i64) -> Result<AgentIntegrationResult, CommandError> {
        self.mutate(MergeAction::Install, now)
    }
    fn uninstall(&self, confirm: bool, now: i64) -> Result<AgentIntegrationResult, CommandError> {
        if !confirm {
            return Err(invalid());
        };
        if unsupported_descriptor(&self.descriptor) {
            return Ok(unsupported_result(&self.descriptor));
        };
        let _mutation = self.mutation_lock.lock().unwrap();
        let before = self.filesystem.read(&self.descriptor.config_path)?;
        let (after, changed) = merge_config(
            &before,
            self.descriptor.config_format.clone(),
            &self.descriptor.owned_hooks,
            MergeAction::Uninstall,
        )?;
        if !changed {
            return Ok(self.result("notInstalled", None, false));
        };
        let prior = self.store.get(
            self.descriptor.agent_id.clone(),
            self.descriptor.environment.clone(),
        )?;
        let backup = self.write_replacement(&before, &after, now)?;
        let written = match self.filesystem.read(&self.descriptor.config_path) {
            Ok(written) => written,
            Err(error) => return Err(self.restore_after_replace(&before, error, now)),
        };
        let (_, still_changed) = match merge_config(
            &written,
            self.descriptor.config_format.clone(),
            &self.descriptor.owned_hooks,
            MergeAction::Uninstall,
        ) {
            Ok(value) => value,
            Err(error) => return Err(self.restore_after_replace(&before, error, now)),
        };
        if still_changed {
            return Err(self.restore_after_replace(
                &before,
                integration_config_invalid(&self.descriptor, "verification"),
                now,
            ));
        }
        let record = AgentIntegrationEntity {
            agent_id: self.descriptor.agent_id.clone(),
            environment: self.descriptor.environment.clone(),
            install_state: "notInstalled".into(),
            config_path: self.descriptor.config_path.display().to_string(),
            backup_path: Some(backup.display().to_string()),
            owned_fingerprint: None,
            revision: prior.as_ref().map_or(0, |r| r.revision),
            updated_at: now,
        };
        if self
            .store
            .put(&record, prior.as_ref().map(|r| r.revision))
            .is_err()
        {
            return Err(self.restore_after_replace(&before, database_failure(), now));
        };
        Ok(self.result("notInstalled", Some(backup), true))
    }
}

pub fn fixed_descriptors(
    windows_home: &Path,
    app_data_dir: &Path,
    wsl_home: &str,
    wsl_status_dir: &str,
) -> Vec<FixedIntegrationDescriptor> {
    let hermes_desktop_config = windows_home.join("AppData/Local/hermes/config.yaml");
    let hermes_windows_config = if hermes_desktop_config.exists()
        || hermes_desktop_config.parent().is_some_and(Path::exists)
    {
        hermes_desktop_config
    } else {
        windows_home.join(".hermes/config.yaml")
    };
    let windows_script = |agent: &str| {
        app_data_dir
            .join("agent-hooks")
            .join(format!("{agent}-windows.ps1"))
    };
    let wsl_script = |agent: &str| {
        format!(
            "{}/.local/share/aiceland/agent-hooks/{agent}-wsl.sh",
            wsl_home.trim_end_matches('/')
        )
    };
    let windows_owned = |agent: AgentId, events: Vec<&str>| {
        let script = windows_script(agent_id_name(&agent));
        events
            .into_iter()
            .map(|event| OwnedHookFragment {
                event: event.into(),
                command: windows_hook_command(
                    &HookInvocation {
                        agent_id: agent.clone(),
                        environment: AgentEnvironment::Windows,
                        native_event: event.into(),
                        output_path: app_data_dir
                            .join("agent-status")
                            .join(format!("{}-windows.json", agent_id_name(&agent))),
                    },
                    &script,
                ),
            })
            .collect::<Vec<_>>()
    };
    let wsl_owned = |agent: AgentId, events: Vec<&str>| {
        let script = wsl_script(agent_id_name(&agent));
        events
            .into_iter()
            .map(|event| OwnedHookFragment {
                event: event.into(),
                command: wsl_hook_command(
                    &HookInvocation {
                        agent_id: agent.clone(),
                        environment: AgentEnvironment::Wsl,
                        native_event: event.into(),
                        output_path: PathBuf::from(format!(
                            "{}/{}-wsl.json",
                            wsl_status_dir.trim_end_matches('/'),
                            agent_id_name(&agent)
                        )),
                    },
                    &script,
                ),
            })
            .collect::<Vec<_>>()
    };
    let descriptor =
        |agent, environment, config_format, config_path, owned_hooks| FixedIntegrationDescriptor {
            agent_id: agent,
            environment,
            config_format,
            config_path,
            owned_hooks,
        };
    let codex_events = vec![
        "SessionStart",
        "UserPromptSubmit",
        "PermissionRequest",
        "Stop",
        "SessionEnd",
    ];
    let all = vec![
        "SessionStart",
        "UserPromptSubmit",
        "PermissionRequest",
        "Stop",
        "StopFailure",
        "SessionEnd",
    ];
    let hermes_events = vec![
        "on_session_start",
        "pre_llm_call",
        "pre_approval_request",
        "post_approval_response",
        "post_llm_call",
        "on_session_end",
    ];
    vec![
        descriptor(
            AgentId::Codex,
            AgentEnvironment::Windows,
            ConfigFormat::JsonHooks,
            windows_home.join(".codex/hooks.json"),
            windows_owned(AgentId::Codex, codex_events.clone()),
        ),
        descriptor(
            AgentId::Codex,
            AgentEnvironment::Wsl,
            ConfigFormat::JsonHooks,
            PathBuf::from(format!("{}/.codex/hooks.json", wsl_home)),
            wsl_owned(AgentId::Codex, codex_events),
        ),
        descriptor(
            AgentId::Hermes,
            AgentEnvironment::Windows,
            ConfigFormat::HermesYaml,
            hermes_windows_config,
            windows_owned(AgentId::Hermes, hermes_events.clone()),
        ),
        descriptor(
            AgentId::Hermes,
            AgentEnvironment::Wsl,
            ConfigFormat::HermesYaml,
            PathBuf::from(format!("{}/.hermes/config.yaml", wsl_home)),
            wsl_owned(AgentId::Hermes, hermes_events),
        ),
        descriptor(
            AgentId::Workbuddy,
            AgentEnvironment::Windows,
            ConfigFormat::JsonHooks,
            windows_home.join(".workbuddy-ai/settings.json"),
            windows_owned(AgentId::Workbuddy, all.clone()),
        ),
        descriptor(
            AgentId::Claude,
            AgentEnvironment::Windows,
            ConfigFormat::JsonHooks,
            windows_home.join(".claude/settings.json"),
            windows_owned(AgentId::Claude, all.clone()),
        ),
        descriptor(
            AgentId::Claude,
            AgentEnvironment::Wsl,
            ConfigFormat::JsonHooks,
            PathBuf::from(format!("{}/.claude/settings.json", wsl_home)),
            wsl_owned(AgentId::Claude, all),
        ),
    ]
}
fn agent_id_name(agent: &AgentId) -> &'static str {
    match agent {
        AgentId::Codex => "codex",
        AgentId::Hermes => "hermes",
        AgentId::Workbuddy => "workbuddy",
        AgentId::Claude => "claude",
    }
}
fn environment_name(environment: &AgentEnvironment) -> &'static str {
    match environment {
        AgentEnvironment::Windows => "windows",
        AgentEnvironment::Wsl => "wsl",
    }
}
fn unsupported_descriptor(d: &FixedIntegrationDescriptor) -> bool {
    unsupported_pair(&d.agent_id, &d.environment)
}
fn unsupported_pair(agent: &AgentId, environment: &AgentEnvironment) -> bool {
    matches!(
        (agent, environment),
        (AgentId::Workbuddy, AgentEnvironment::Wsl)
    )
}
fn unsupported_result(d: &FixedIntegrationDescriptor) -> AgentIntegrationResult {
    AgentIntegrationResult {
        agent_id: d.agent_id.clone(),
        environment: d.environment.clone(),
        state: crate::contracts::IntegrationState::Unsupported,
        config_path: d.config_path.display().to_string(),
        backup_path: None,
        changed: false,
    }
}
fn unsupported_result_for(
    agent_id: AgentId,
    environment: AgentEnvironment,
) -> AgentIntegrationResult {
    AgentIntegrationResult {
        agent_id,
        environment,
        state: crate::contracts::IntegrationState::Unsupported,
        config_path: String::new(),
        backup_path: None,
        changed: false,
    }
}
fn invalid() -> CommandError {
    CommandError::new(
        AppErrorCode::InvalidInput,
        "errors.invalidInput",
        Default::default(),
        false,
    )
    .unwrap()
}
fn unsupported() -> CommandError {
    CommandError::new(
        AppErrorCode::IntegrationUnsupported,
        "errors.integrationUnsupported",
        Default::default(),
        false,
    )
    .unwrap_or_else(|e| e)
}
fn io(reason: &str) -> CommandError {
    CommandError::with_detail(
        AppErrorCode::IoFailure,
        "errors.ioFailure",
        "reasonCode",
        SafeParameterValue::String(reason.into()),
        false,
    )
}
fn integration_config_invalid(
    descriptor: &FixedIntegrationDescriptor,
    reason: &str,
) -> CommandError {
    CommandError::new(
        AppErrorCode::IntegrationConfigInvalid,
        "errors.integrationConfigInvalid",
        std::collections::BTreeMap::from([
            (
                "agentName".into(),
                SafeParameterValue::String(descriptor.agent_id.display_name().into()),
            ),
            (
                "environment".into(),
                SafeParameterValue::String(environment_name(&descriptor.environment).into()),
            ),
            (
                "reasonCode".into(),
                SafeParameterValue::String(reason.into()),
            ),
        ]),
        false,
    )
    .unwrap()
}
fn database_failure() -> CommandError {
    CommandError::new(
        AppErrorCode::DatabaseFailure,
        "errors.databaseFailure",
        Default::default(),
        false,
    )
    .unwrap()
}
fn managed_fingerprint_material(
    _bytes: &[u8],
    descriptor: &FixedIntegrationDescriptor,
) -> Result<Vec<u8>, CommandError> {
    // The managed identity is the canonical fixed fragment set, not an entire user-owned config.
    let mut entries = descriptor
        .owned_hooks
        .iter()
        .map(|fragment| {
            format!(
                "{}\n{}\n{}",
                fragment.event,
                fragment.command,
                match descriptor.config_format {
                    ConfigFormat::JsonHooks => "json",
                    ConfigFormat::HermesYaml => "yaml",
                }
            )
        })
        .collect::<Vec<_>>();
    entries.sort();
    Ok(entries.join("\n").into_bytes())
}
fn timestamp(now: i64) -> String {
    let day_millis = 86_400_000i64;
    let days = now.div_euclid(day_millis);
    let time = now.rem_euclid(day_millis);
    // Gregorian civil date from days since Unix epoch, using integer arithmetic in UTC.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let month_index = (5 * doy + 2) / 153;
    let day = doy - (153 * month_index + 2) / 5 + 1;
    let month = month_index + if month_index < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let hour = time / 3_600_000;
    let minute = (time / 60_000) % 60;
    let second = (time / 1_000) % 60;
    let millis = time % 1_000;
    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}{millis:03}Z")
}
fn unique_nonce() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}
fn sha256_hex(bytes: &[u8]) -> String {
    crate::services::agent_hook_assets::sha256_hex(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::agent_hook_assets::{windows_hook_command, HookInvocation};
    use std::sync::{Barrier, Condvar};
    use std::time::Duration;
    struct MemFs {
        bytes: Mutex<Vec<u8>>,
        fail: Mutex<Option<&'static str>>,
        reads: Mutex<usize>,
        backups: Mutex<HashMap<PathBuf, Vec<u8>>>,
        temporaries: Mutex<HashMap<PathBuf, Vec<u8>>>,
    }
    impl MemFs {
        fn new(bytes: Vec<u8>, fail: Option<&'static str>) -> Self {
            Self {
                bytes: Mutex::new(bytes),
                fail: Mutex::new(fail),
                reads: Mutex::new(0),
                backups: Mutex::new(HashMap::new()),
                temporaries: Mutex::new(HashMap::new()),
            }
        }
        fn fails(&self, stage: &str) -> bool {
            self.fail.lock().unwrap().as_ref().is_some_and(|value| {
                *value == stage || (*value == "backup" && stage == "backup_create")
            })
        }
    }
    impl ConfigFilesystem for MemFs {
        fn read(&self, _: &Path) -> Result<Vec<u8>, CommandError> {
            let mut reads = self.reads.lock().unwrap();
            if *reads > 0 && self.fails("post_read") {
                return Err(io("postRead"));
            }
            *reads += 1;
            Ok(self.bytes.lock().unwrap().clone())
        }
        fn backup_create(&self, _: &Path, _: i64) -> Result<PathBuf, CommandError> {
            if self.fails("backup_create") {
                return Err(io("backupCreate"));
            }
            let path = PathBuf::from("backup");
            self.backups
                .lock()
                .unwrap()
                .insert(path.clone(), Vec::new());
            Ok(path)
        }
        fn backup_write(&self, backup: &Path, bytes: &[u8]) -> Result<(), CommandError> {
            if self.fails("backup_write") {
                return Err(io("backupWrite"));
            }
            *self.backups.lock().unwrap().get_mut(backup).unwrap() = bytes.to_vec();
            Ok(())
        }
        fn backup_flush(&self, _: &Path) -> Result<(), CommandError> {
            if self.fails("backup_flush") {
                return Err(io("backupFlush"));
            }
            Ok(())
        }
        fn temp_create(&self, _: &Path) -> Result<PathBuf, CommandError> {
            if self.fails("temp_create") {
                return Err(io("tempCreate"));
            }
            let path = PathBuf::from("temporary");
            self.temporaries
                .lock()
                .unwrap()
                .insert(path.clone(), Vec::new());
            Ok(path)
        }
        fn temp_write(&self, temporary: &Path, bytes: &[u8]) -> Result<(), CommandError> {
            if self.fails("temp_write") {
                return Err(io("tempWrite"));
            }
            *self.temporaries.lock().unwrap().get_mut(temporary).unwrap() = bytes.to_vec();
            Ok(())
        }
        fn temp_flush(&self, _: &Path) -> Result<(), CommandError> {
            if self.fails("temp_flush") {
                return Err(io("tempFlush"));
            }
            Ok(())
        }
        fn replace(&self, temporary: &Path, _: &Path) -> Result<(), CommandError> {
            if self.fails("replace") {
                return Err(io("replace"));
            }
            let mut bytes = self.temporaries.lock().unwrap().remove(temporary).unwrap();
            if self.fails("verify") {
                bytes = br#"{"hooks":{"Stop":[]}}"#.to_vec();
            }
            *self.bytes.lock().unwrap() = bytes;
            Ok(())
        }
        fn restore(&self, _: &Path, bytes: &[u8]) -> Result<(), CommandError> {
            if self.fails("restore") {
                return Err(io("restore"));
            }
            *self.bytes.lock().unwrap() = bytes.to_vec();
            Ok(())
        }
    }
    struct MemStore;
    impl IntegrationStore for MemStore {
        fn get(
            &self,
            _: AgentId,
            _: AgentEnvironment,
        ) -> Result<Option<AgentIntegrationEntity>, CommandError> {
            Ok(None)
        }
        fn put(
            &self,
            _: &AgentIntegrationEntity,
            _: Option<u64>,
        ) -> Result<AgentIntegrationEntity, CommandError> {
            Err(CommandError::new(
                AppErrorCode::DatabaseFailure,
                "errors.databaseFailure",
                Default::default(),
                false,
            )
            .unwrap())
        }
    }
    struct StatefulStore {
        row: Mutex<Option<AgentIntegrationEntity>>,
        fail_puts: Mutex<usize>,
    }
    struct RendezvousStore {
        row: Mutex<Option<AgentIntegrationEntity>>,
        reads: Mutex<usize>,
        reads_ready: Condvar,
    }
    impl IntegrationStore for RendezvousStore {
        fn get(
            &self,
            _: AgentId,
            _: AgentEnvironment,
        ) -> Result<Option<AgentIntegrationEntity>, CommandError> {
            let mut reads = self.reads.lock().unwrap();
            *reads += 1;
            if *reads == 1 {
                let (next, _) = self
                    .reads_ready
                    .wait_timeout_while(reads, Duration::from_millis(200), |reads| *reads < 2)
                    .unwrap();
                reads = next;
            } else {
                self.reads_ready.notify_all();
            }
            drop(reads);
            Ok(self.row.lock().unwrap().clone())
        }
        fn put(
            &self,
            record: &AgentIntegrationEntity,
            expected: Option<u64>,
        ) -> Result<AgentIntegrationEntity, CommandError> {
            let mut row = self.row.lock().unwrap();
            let current = row.as_ref().map(|value| value.revision);
            if current != expected {
                return Err(database_failure());
            }
            let mut committed = record.clone();
            committed.revision = expected.map_or(0, |revision| revision + 1);
            *row = Some(committed.clone());
            Ok(committed)
        }
    }
    #[derive(Default)]
    struct RecordingDiagnostics {
        rollbacks: Mutex<Vec<(AgentId, AgentEnvironment, i64)>>,
    }
    impl IntegrationDiagnosticPort for RecordingDiagnostics {
        fn rollback_failed(&self, agent: &AgentId, environment: &AgentEnvironment, now: i64) {
            self.rollbacks
                .lock()
                .unwrap()
                .push((agent.clone(), environment.clone(), now));
        }
    }
    fn fake_diagnostics() -> Arc<dyn IntegrationDiagnosticPort> {
        Arc::new(RecordingDiagnostics::default())
    }
    impl IntegrationStore for StatefulStore {
        fn get(
            &self,
            _: AgentId,
            _: AgentEnvironment,
        ) -> Result<Option<AgentIntegrationEntity>, CommandError> {
            Ok(self.row.lock().unwrap().clone())
        }
        fn put(
            &self,
            record: &AgentIntegrationEntity,
            _: Option<u64>,
        ) -> Result<AgentIntegrationEntity, CommandError> {
            let mut fail_puts = self.fail_puts.lock().unwrap();
            if *fail_puts > 0 {
                *fail_puts -= 1;
                return Err(database_failure());
            }
            *self.row.lock().unwrap() = Some(record.clone());
            Ok(record.clone())
        }
    }
    fn descriptor() -> FixedIntegrationDescriptor {
        FixedIntegrationDescriptor {
            agent_id: AgentId::Codex,
            environment: AgentEnvironment::Windows,
            config_format: ConfigFormat::JsonHooks,
            config_path: "x".into(),
            owned_hooks: vec![OwnedHookFragment {
                event: "Stop".into(),
                command: "owned".into(),
            }],
        }
    }
    fn original_row() -> AgentIntegrationEntity {
        AgentIntegrationEntity {
            agent_id: AgentId::Codex,
            environment: AgentEnvironment::Windows,
            install_state: "notInstalled".into(),
            config_path: "x".into(),
            backup_path: None,
            owned_fingerprint: None,
            revision: 7,
            updated_at: 1,
        }
    }
    #[test]
    fn failure_after_replace_restores_original_bytes() {
        let fs = Arc::new(MemFs::new(br#"{"hooks":{"Stop":[]}}"#.to_vec(), None));
        let a = FileAdapter::new(
            descriptor(),
            fs.clone(),
            Arc::new(MemStore),
            fake_diagnostics(),
        );
        assert!(a.install(1).is_err());
        assert_eq!(*fs.bytes.lock().unwrap(), br#"{"hooks":{"Stop":[]}}"#);
    }
    #[test]
    fn workbuddy_wsl_is_explicitly_unsupported() {
        let mut d = descriptor();
        d.agent_id = AgentId::Workbuddy;
        d.environment = AgentEnvironment::Wsl;
        let a = FileAdapter::new(
            d,
            Arc::new(MemFs::new(vec![], None)),
            Arc::new(MemStore),
            fake_diagnostics(),
        );
        assert_eq!(
            a.install(1).unwrap().state,
            crate::contracts::IntegrationState::Unsupported
        );
    }

    #[test]
    fn backup_suffix_uses_the_required_utc_timestamp_shape() {
        assert_eq!(timestamp(0), "19700101T000000000Z");
    }

    #[test]
    fn descriptors_use_one_safe_canonical_hook_invocation_per_owned_event() {
        let home = Path::new(r"C:\Users\Alice Smith");
        let app_data_dir = Path::new(r"C:\Users\Alice Smith\AppData\Roaming\com.aiceland.app");
        let wsl_status_dir =
            "/mnt/c/Users/Alice Smith/AppData/Roaming/com.aiceland.app/agent-status";
        let descriptors =
            fixed_descriptors(home, app_data_dir, "/home/alice smith", wsl_status_dir);
        assert_eq!(descriptors.len(), 7);
        let codex = descriptors
            .iter()
            .find(|descriptor| {
                descriptor.agent_id == AgentId::Codex
                    && descriptor.environment == AgentEnvironment::Windows
            })
            .unwrap();
        let stop = codex
            .owned_hooks
            .iter()
            .find(|hook| hook.event == "Stop")
            .unwrap();
        assert_eq!(
            stop.command,
            windows_hook_command(
                &HookInvocation {
                    agent_id: AgentId::Codex,
                    environment: AgentEnvironment::Windows,
                    native_event: "Stop".into(),
                    output_path: app_data_dir.join("agent-status").join("codex-windows.json"),
                },
                &app_data_dir.join("agent-hooks").join("codex-windows.ps1"),
            )
        );
        let wsl_codex = descriptors
            .iter()
            .find(|descriptor| {
                descriptor.agent_id == AgentId::Codex
                    && descriptor.environment == AgentEnvironment::Wsl
            })
            .unwrap();
        let wsl_stop = wsl_codex
            .owned_hooks
            .iter()
            .find(|hook| hook.event == "Stop")
            .unwrap();
        assert_eq!(
            wsl_stop.command,
            wsl_hook_command(
                &HookInvocation {
                    agent_id: AgentId::Codex,
                    environment: AgentEnvironment::Wsl,
                    native_event: "Stop".into(),
                    output_path: PathBuf::from(format!("{wsl_status_dir}/codex-wsl.json")),
                },
                "/home/alice smith/.local/share/aiceland/agent-hooks/codex-wsl.sh",
            )
        );
    }

    #[test]
    fn hermes_windows_descriptor_prefers_the_desktop_runtime_home_when_present() {
        let root = tempfile::tempdir().unwrap();
        let windows_home = root.path().join("Users/Ada");
        let desktop_home = windows_home.join("AppData/Local/hermes");
        std::fs::create_dir_all(&desktop_home).unwrap();
        std::fs::write(desktop_home.join("config.yaml"), b"model: {}\n").unwrap();

        let descriptors = fixed_descriptors(
            &windows_home,
            &windows_home.join("AppData/Roaming/com.aiceland.app"),
            "/home/ada",
            "/mnt/c/Users/Ada/AppData/Roaming/com.aiceland.app/agent-status",
        );
        let hermes = descriptors
            .iter()
            .find(|descriptor| {
                descriptor.agent_id == AgentId::Hermes
                    && descriptor.environment == AgentEnvironment::Windows
            })
            .unwrap();

        assert_eq!(hermes.config_path, desktop_home.join("config.yaml"));
    }

    #[test]
    fn fingerprint_ignores_unrelated_user_config_bytes() {
        let descriptor = descriptor();
        assert_eq!(
            sha256_hex(&managed_fingerprint_material(b"{\"user\":1}", &descriptor).unwrap()),
            sha256_hex(
                &managed_fingerprint_material(b"{\"user\":2,\"other\":true}", &descriptor).unwrap()
            )
        );
    }

    #[test]
    fn seven_descriptor_service_returns_typed_workbuddy_wsl_unsupported_result() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(crate::storage::Storage::open(directory.path()).unwrap());
        let repository = AgentRepository::new(storage.clone());
        let diagnostics = DiagnosticsRepository::new(storage);
        let service = AgentIntegrationService::new(
            repository,
            diagnostics,
            Path::new(r"C:\Users\Alice"),
            Path::new(r"C:\Users\Alice\AppData\Roaming\com.aiceland.app"),
            "/home/alice",
            "/mnt/c/Users/Alice/AppData/Roaming/com.aiceland.app/agent-status",
            "/home/alice/.local/share/aiceland/agent-hooks/aiceland-config-wsl.sh".into(),
        );

        let result = service
            .install(AgentId::Workbuddy, AgentEnvironment::Wsl, 1)
            .unwrap();

        assert_eq!(
            result.state,
            crate::contracts::IntegrationState::Unsupported
        );
        assert!(!result.changed);
        assert_eq!(service.adapters.len(), 7);
    }

    #[test]
    fn wsl_backup_uses_one_noclobber_group_for_full_copy_and_flush() {
        let helper = include_str!("../../agent-hooks/aiceland-config-wsl.sh");
        let normalized = helper.lines().map(str::trim).collect::<Vec<_>>().join("\n");
        assert!(normalized.contains(
            "( set -C\ncat -- \"$target\" > \"$backup\"\nsync -f \"$backup\" 2>/dev/null || sync\n) 2>/dev/null || exit 65"
        ));
        assert!(!helper.contains(": > \"$backup\""));
    }

    #[test]
    fn every_granular_precommit_fault_preserves_original_bytes_and_repository_row() {
        let original = br#"{"hooks":{"Stop":[]},"user":true}"#.to_vec();
        for stage in [
            "backup_create",
            "backup_write",
            "backup_flush",
            "temp_create",
            "temp_write",
            "temp_flush",
            "replace",
            "post_read",
            "verify",
        ] {
            let filesystem = Arc::new(MemFs::new(original.clone(), Some(stage)));
            let initial_row = original_row();
            let store = Arc::new(StatefulStore {
                row: Mutex::new(Some(initial_row.clone())),
                fail_puts: Mutex::new(0),
            });
            let adapter = FileAdapter::new(
                descriptor(),
                filesystem.clone(),
                store.clone(),
                fake_diagnostics(),
            );

            assert!(adapter.install(10).is_err(), "stage {stage} did not fail");
            assert_eq!(
                *filesystem.bytes.lock().unwrap(),
                original,
                "stage {stage} changed config bytes"
            );
            assert_eq!(
                *store.row.lock().unwrap(),
                Some(initial_row),
                "stage {stage} changed repository row"
            );
        }
    }

    #[test]
    fn local_filesystem_atomically_replaces_an_existing_windows_config() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.json");
        fs::write(&config, b"before").unwrap();
        let filesystem = LocalConfigFilesystem::new();
        let temporary = filesystem.temp_create(&config).unwrap();
        filesystem.temp_write(&temporary, b"after").unwrap();
        filesystem.temp_flush(&temporary).unwrap();

        filesystem.replace(&temporary, &config).unwrap();

        assert_eq!(fs::read(&config).unwrap(), b"after");
        assert!(!temporary.exists());
    }

    #[test]
    fn parse_and_repository_faults_preserve_original_bytes_and_repository_row() {
        let initial_row = original_row();
        for (bytes, fail_puts) in [(b"not json".to_vec(), 0), (br#"{"hooks":{}}"#.to_vec(), 1)] {
            let filesystem = Arc::new(MemFs::new(bytes.clone(), None));
            let store = Arc::new(StatefulStore {
                row: Mutex::new(Some(initial_row.clone())),
                fail_puts: Mutex::new(fail_puts),
            });
            let adapter = FileAdapter::new(
                descriptor(),
                filesystem.clone(),
                store.clone(),
                fake_diagnostics(),
            );

            assert!(adapter.install(10).is_err());
            assert_eq!(*filesystem.bytes.lock().unwrap(), bytes);
            assert_eq!(*store.row.lock().unwrap(), Some(initial_row.clone()));
        }
    }

    #[test]
    fn failed_restore_records_safe_diagnostic_and_persists_needs_repair() {
        let original = br#"{"hooks":{"Stop":[]}}"#.to_vec();
        let filesystem = Arc::new(MemFs::new(original.clone(), Some("restore")));
        let store = Arc::new(StatefulStore {
            row: Mutex::new(Some(original_row())),
            fail_puts: Mutex::new(1),
        });
        let diagnostics = Arc::new(RecordingDiagnostics::default());
        let adapter = FileAdapter::new(
            descriptor(),
            filesystem.clone(),
            store.clone(),
            diagnostics.clone(),
        );

        let error = adapter.install(10).unwrap_err();

        assert_eq!(error.code, AppErrorCode::IntegrationConfigInvalid);
        assert_eq!(error.message_key, "errors.integrationConfigInvalid");
        assert_eq!(
            error.details,
            std::collections::BTreeMap::from([
                (
                    "agentName".into(),
                    SafeParameterValue::String("Codex".into()),
                ),
                (
                    "environment".into(),
                    SafeParameterValue::String("windows".into()),
                ),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String("rollbackFailed".into()),
                ),
            ])
        );
        assert_ne!(*filesystem.bytes.lock().unwrap(), original);
        assert_eq!(
            store.row.lock().unwrap().as_ref().unwrap().install_state,
            "needsRepair"
        );
        assert_eq!(
            *diagnostics.rollbacks.lock().unwrap(),
            vec![(AgentId::Codex, AgentEnvironment::Windows, 10)]
        );
    }

    #[test]
    fn production_diagnostics_repository_records_repo_and_restore_failure() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(crate::storage::Storage::open(directory.path()).unwrap());
        let diagnostics = DiagnosticsRepository::new(storage);
        let original = br#"{"hooks":{"Stop":[]}}"#.to_vec();
        let filesystem = Arc::new(MemFs::new(original, Some("restore")));
        let store = Arc::new(StatefulStore {
            row: Mutex::new(Some(original_row())),
            fail_puts: Mutex::new(1),
        });
        let adapter = FileAdapter::new(
            descriptor(),
            filesystem,
            store.clone(),
            Arc::new(diagnostics.clone()),
        );

        assert_eq!(
            adapter.install(30).unwrap_err().code,
            AppErrorCode::IntegrationConfigInvalid
        );
        let events = diagnostics.list(10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].code, "integration.rollbackFailed");
        assert_eq!(events[0].service_id, "agent-integrations");
        assert_eq!(events[0].created_at, 30);
        assert_eq!(
            store.row.lock().unwrap().as_ref().unwrap().install_state,
            "needsRepair"
        );
        assert_eq!(
            events[0].parameters,
            std::collections::BTreeMap::from([
                (
                    "agentName".into(),
                    SafeParameterValue::String("Codex".into()),
                ),
                (
                    "environment".into(),
                    SafeParameterValue::String("windows".into()),
                ),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String("rollbackFailed".into()),
                ),
            ])
        );
    }

    #[test]
    fn concurrent_installs_serialize_the_file_and_repository_transaction() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("hooks.json");
        let original = br#"{"hooks":{"Stop":[]},"user":true}"#;
        fs::write(&config, original).unwrap();
        let mut fixed = descriptor();
        fixed.config_path = config.clone();
        let store = Arc::new(RendezvousStore {
            row: Mutex::new(None),
            reads: Mutex::new(0),
            reads_ready: Condvar::new(),
        });
        let adapter = Arc::new(FileAdapter::new(
            fixed.clone(),
            Arc::new(LocalConfigFilesystem::new()),
            store.clone(),
            fake_diagnostics(),
        ));
        let start = Arc::new(Barrier::new(3));
        let threads = [10, 20].map(|now| {
            let adapter = adapter.clone();
            let start = start.clone();
            std::thread::spawn(move || {
                start.wait();
                adapter.install(now)
            })
        });
        start.wait();
        let results = threads.map(|thread| thread.join().unwrap());

        assert!(results.iter().all(Result::is_ok), "{results:?}");
        let bytes = fs::read(&config).unwrap();
        assert!(inspect_config(&bytes, fixed.config_format.clone(), &fixed.owned_hooks).unwrap());
        let row = store.row.lock().unwrap().clone().unwrap();
        assert_eq!(row.install_state, "installed");
        assert_eq!(
            row.owned_fingerprint,
            Some(sha256_hex(
                &managed_fingerprint_material(&bytes, &fixed).unwrap()
            ))
        );
    }

    #[test]
    fn concurrent_repair_and_uninstall_finish_in_one_serial_order() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("hooks.json");
        let drifted = br#"{"hooks":{"Stop":[{"matcher":"*","hooks":[{"type":"command","command":"owned","extra":"drift"}]}]}}"#;
        fs::write(&config, drifted).unwrap();
        let mut fixed = descriptor();
        fixed.config_path = config.clone();
        let mut initial = original_row();
        initial.config_path = config.display().to_string();
        initial.install_state = "needsRepair".into();
        let store = Arc::new(RendezvousStore {
            row: Mutex::new(Some(initial)),
            reads: Mutex::new(0),
            reads_ready: Condvar::new(),
        });
        let adapter = Arc::new(FileAdapter::new(
            fixed.clone(),
            Arc::new(LocalConfigFilesystem::new()),
            store.clone(),
            fake_diagnostics(),
        ));
        let start = Arc::new(Barrier::new(3));
        let repair = {
            let adapter = adapter.clone();
            let start = start.clone();
            std::thread::spawn(move || {
                start.wait();
                adapter.repair(40)
            })
        };
        let uninstall = {
            let adapter = adapter.clone();
            let start = start.clone();
            std::thread::spawn(move || {
                start.wait();
                adapter.uninstall(true, 50)
            })
        };
        start.wait();
        let results = [repair.join().unwrap(), uninstall.join().unwrap()];

        assert!(results.iter().all(Result::is_ok), "{results:?}");
        let bytes = fs::read(&config).unwrap();
        let installed =
            inspect_config(&bytes, fixed.config_format.clone(), &fixed.owned_hooks).unwrap();
        let row = store.row.lock().unwrap().clone().unwrap();
        assert_eq!(
            row.install_state,
            if installed {
                "installed"
            } else {
                "notInstalled"
            }
        );
    }
}
