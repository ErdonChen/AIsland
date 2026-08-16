use crate::contracts::{
    AgentConfigTarget, AgentEnvironment, AgentEventMapping, AgentIntegrationKind,
    AgentIntegrationProfile, AgentProfileObservation as AgentProfileObservationDto,
    AgentProfileStatusSummary, AgentProfilesSnapshot, AgentStatus, AppErrorCode, CommandError,
    DeleteResult, IntegrationState, PresetAgentAdapterId, SafeMessageParameters,
    SafeParameterValue, SaveAgentIntegrationProfileInput,
};
use crate::domain::agent_profiles::{
    AgentIntegrationId, AgentProfileInstallation, StoredAgentIntegrationProfile,
    ValidatedAgentProfileEvent,
};
use crate::domain::agents::COMPLETION_FLASH_MILLIS;
use crate::events::{
    agent_profile_change_payload, agent_profile_state_changed_payload, AGENT_PROFILE_STATE_CHANGED,
};
use crate::repositories::agent_profiles::{AgentProfileProjectionOutcome, AgentProfileRepository};
use crate::services::agent_profile_spool::{
    ConfigMutation, PresetInstallOutcome, PresetProfileBridge,
};
use crate::services::native_profile_activity::{
    NativeProfileActivityReader, NativeProfileActivitySource,
};
use crate::services::EventEmitterPort;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{mpsc, Arc, Mutex, RwLock, RwLockReadGuard};
use std::thread;
use std::time::{Duration, Instant};

const MAX_DISPLAY_CHARS: usize = 64;
const MAX_EVENT_MAPPINGS: usize = 32;
const MAX_NATIVE_EVENT_CHARS: usize = 64;
const MAX_ARG_COUNT: usize = 32;
const MAX_ARG_CHARS: usize = 1024;
const MAX_PATH_CHARS: usize = 4096;
const MAX_PROFILE_PAYLOAD_BYTES: usize = 64 * 1024;
const MAX_EVENT_LINE_BYTES: usize = 16 * 1024;
const MAX_TASK_ID_CHARS: usize = 128;
const MAX_EVENT_ID_CHARS: usize = 128;
const TERMINAL_STATUS_VISIBLE_MILLIS: i64 = 10_000;
const ACTIVE_STATUS_STALE_MILLIS: i64 = 30_000;
const MAX_CUSTOM_PROFILES: usize = 32;
const MAX_INSTALLED_CUSTOM_PROFILES: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedCustomHookTarget {
    pub executable: PathBuf,
    pub argv: Vec<String>,
    pub working_directory: Option<PathBuf>,
    pub timeout_seconds: u64,
}

pub struct AgentProfileService {
    repository: AgentProfileRepository,
    emitter: Arc<dyn EventEmitterPort>,
    preset_bridge: Arc<PresetProfileBridge>,
    native_profile_activity: Arc<dyn NativeProfileActivitySource>,
    runtimes: Mutex<HashMap<String, CustomRuntimeHandle>>,
    mutation_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    installation_capacity: Mutex<()>,
    admission: RwLock<()>,
    accepting: AtomicBool,
    shutdown_complete: AtomicBool,
    #[cfg(test)]
    lifecycle_hook: Mutex<Option<Arc<dyn Fn(&'static str) + Send + Sync>>>,
    #[cfg(test)]
    admission_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

struct CustomRuntimeHandle {
    control: mpsc::Sender<RuntimeControl>,
    join: Option<thread::JoinHandle<()>>,
    activated_at: Option<i64>,
}

enum RuntimeControl {
    Activate(mpsc::SyncSender<Result<(), &'static str>>),
    Stop,
}

impl CustomRuntimeHandle {
    fn activate(&mut self) -> Result<(), CommandError> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.control
            .send(RuntimeControl::Activate(reply_tx))
            .map_err(|_| io_error("hookExitedBeforeActivation"))?;
        await_activation(reply_rx, Duration::from_secs(2))?;
        self.activated_at = Some(now_millis());
        Ok(())
    }

    fn stop(mut self) {
        let _ = self.control.send(RuntimeControl::Stop);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl AgentProfileService {
    pub fn new(
        repository: AgentProfileRepository,
        windows_home: PathBuf,
        app_data_dir: PathBuf,
        emitter: Arc<dyn EventEmitterPort>,
    ) -> Self {
        let preset_bridge = Arc::new(PresetProfileBridge::new(
            repository.clone(),
            emitter.clone(),
            windows_home.clone(),
            app_data_dir.clone(),
        ));
        let roaming_app_data = app_data_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| app_data_dir.clone());
        let native_profile_activity = Arc::new(NativeProfileActivityReader::new(
            windows_home,
            roaming_app_data,
        ));
        Self {
            repository,
            emitter,
            preset_bridge,
            native_profile_activity,
            runtimes: Mutex::new(HashMap::new()),
            mutation_locks: Mutex::new(HashMap::new()),
            installation_capacity: Mutex::new(()),
            admission: RwLock::new(()),
            accepting: AtomicBool::new(true),
            shutdown_complete: AtomicBool::new(false),
            #[cfg(test)]
            lifecycle_hook: Mutex::new(None),
            #[cfg(test)]
            admission_hook: Mutex::new(None),
        }
    }

    pub fn list_profiles(&self) -> Result<Vec<AgentIntegrationProfile>, CommandError> {
        self.repository
            .list()?
            .into_iter()
            .map(|profile| self.profile_contract(profile))
            .collect()
    }

    pub fn snapshot(&self, generated_at: i64) -> Result<AgentProfilesSnapshot, CommandError> {
        let running_processes =
            crate::services::agent_status_watcher::running_process_base_names().unwrap_or_default();
        let running_processes = running_processes
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        self.snapshot_with_running_process_names(generated_at, &running_processes)
    }

    fn snapshot_with_running_process_names(
        &self,
        generated_at: i64,
        process_names: &[&str],
    ) -> Result<AgentProfilesSnapshot, CommandError> {
        let detected_presets = detected_preset_apps(process_names);
        let native_activities = detected_presets
            .iter()
            .filter_map(|adapter_id| {
                match self
                    .native_profile_activity
                    .latest_activity(adapter_id.clone(), generated_at)
                {
                    Ok(Some(activity)) => Some((adapter_id.clone(), activity)),
                    Ok(None) => None,
                    Err(error) => {
                        log::warn!(
                            "native profile activity unavailable: adapter={adapter_id:?} code={:?}",
                            error.code
                        );
                        None
                    }
                }
            })
            .collect::<BTreeMap<_, _>>();
        let profiles = self
            .repository
            .list()?
            .into_iter()
            .map(|stored| {
                let observations = self.repository.list_observations(&stored.id)?;
                let installation = self.repository.get_installation(&stored.id)?;
                let environment = stored.environment.clone();
                let profile_id = stored.id.clone();
                let profile = self.profile_contract(stored)?;
                let (custom_runtime_active, runtime_epoch) = self
                    .runtimes
                    .lock()
                    .expect("custom runtime map lock poisoned")
                    .get(&profile.id)
                    .map(|runtime| (true, runtime.activated_at))
                    .unwrap_or((false, None));
                let runtime_active = match profile.kind {
                    AgentIntegrationKind::Custom => custom_runtime_active,
                    AgentIntegrationKind::Preset => self.preset_bridge.is_running(),
                };
                let observation_epoch = match profile.kind {
                    AgentIntegrationKind::Custom => runtime_epoch,
                    AgentIntegrationKind::Preset => {
                        installation.as_ref().map(|receipt| receipt.updated_at)
                    }
                };
                let installed_and_enabled =
                    profile.enabled && profile.installation_state == IntegrationState::Installed;
                let process_detected = profile.environment == AgentEnvironment::Windows
                    && matches!(
                        &profile.config_target,
                        AgentConfigTarget::Preset { adapter_id }
                            if detected_presets.contains(adapter_id)
                    );
                let mut observations = observations
                    .into_iter()
                    .filter(|observation| {
                        installed_and_enabled
                            && observation_is_current(observation, observation_epoch, generated_at)
                    })
                    .collect::<Vec<_>>();
                let native_activity = match &profile.config_target {
                    AgentConfigTarget::Preset { adapter_id } if process_detected => {
                        native_activities.get(adapter_id)
                    }
                    _ => None,
                };
                if let Some(native) = native_activity {
                    // A verified read-only native source is authoritative. Managed Hook events
                    // remain the fallback only when the native source is unavailable.
                    observations.clear();
                    let received_at = if native.status == AgentStatus::Running {
                        generated_at
                    } else {
                        native.occurred_at.min(generated_at)
                    };
                    observations.push(crate::domain::agent_profiles::AgentProfileObservation {
                        profile_id: profile_id.clone(),
                        task_id: native.task_id.clone(),
                        status: native.status.clone(),
                        latest_reply_preview: native.latest_reply.clone(),
                        source_event_id: native.source_event_id.clone(),
                        occurred_at: native.occurred_at,
                        received_at,
                    });
                }
                let aggregate_status = aggregate_profile_status(
                    &observations,
                    installed_and_enabled || process_detected,
                    runtime_active || process_detected,
                    None,
                    generated_at,
                );
                let observations = observations
                    .into_iter()
                    .map(|observation| AgentProfileObservationDto {
                        profile_id: observation.profile_id.as_str().into(),
                        environment: environment.clone(),
                        task_id: observation.task_id,
                        status: observation.status,
                        latest_reply_preview: observation.latest_reply_preview,
                        source_event_id: observation.source_event_id,
                        occurred_at: observation.occurred_at,
                        received_at: observation.received_at,
                    })
                    .collect();
                Ok(AgentProfileStatusSummary {
                    profile,
                    aggregate_status,
                    observations,
                })
            })
            .collect::<Result<Vec<_>, CommandError>>()?;
        Ok(AgentProfilesSnapshot {
            profiles,
            generated_at,
        })
    }

    pub fn save_profile(
        &self,
        input: SaveAgentIntegrationProfileInput,
        now: i64,
    ) -> Result<AgentIntegrationProfile, CommandError> {
        let _admission = self.begin_mutation()?;
        validate_profile_text_and_mapping(
            &input.display_name,
            &input.event_mapping,
            &input.config_target,
        )?;
        match input.kind {
            AgentIntegrationKind::Preset => self.save_preset_profile(input),
            AgentIntegrationKind::Custom => self.save_custom_profile(input, now),
        }
    }

    pub fn install_profile(
        &self,
        id: &str,
        expected_revision: i64,
        now: i64,
    ) -> Result<AgentIntegrationProfile, CommandError> {
        let _admission = self.begin_mutation()?;
        self.activate_profile(id, expected_revision, now, false)
    }

    pub fn repair_profile(
        &self,
        id: &str,
        expected_revision: i64,
        now: i64,
    ) -> Result<AgentIntegrationProfile, CommandError> {
        let _admission = self.begin_mutation()?;
        self.activate_profile(id, expected_revision, now, true)
    }

    pub fn uninstall_profile(
        &self,
        id: &str,
        expected_revision: i64,
        now: i64,
    ) -> Result<AgentIntegrationProfile, CommandError> {
        let _admission = self.begin_mutation()?;
        let id = parse_profile_id(id)?;
        let _capacity = self
            .installation_capacity
            .lock()
            .expect("profile installation capacity lock poisoned");
        let lock = self.profile_lock(&id);
        let _guard = lock.lock().expect("profile mutation lock poisoned");
        let stored = self.repository.get(&id)?;
        if stored.revision != expected_revision {
            return Err(conflict(&id));
        }
        let preset_mutation = if stored.kind == AgentIntegrationKind::Preset {
            self.uninstall_preset(&stored, now)?
        } else {
            None
        };
        let installation = AgentProfileInstallation {
            profile_id: id.clone(),
            state: IntegrationState::NotInstalled,
            reason_code: None,
            owned_resource: None,
            owned_fingerprint: None,
            external_hash: None,
            updated_at: now,
        };
        let stored = match self
            .repository
            .set_installation(&installation, expected_revision, false)
        {
            Ok(stored) => stored,
            Err(error) => {
                if let Some(mutation) = preset_mutation {
                    if let Err(rollback_error) = mutation.rollback() {
                        return Err(rollback_error);
                    }
                }
                return Err(error);
            }
        };
        if stored.kind == AgentIntegrationKind::Custom {
            self.stop_runtime(id.as_str());
        }
        self.profile_contract(stored)
    }

    pub fn delete_profile(
        &self,
        id: &str,
        expected_revision: i64,
    ) -> Result<DeleteResult, CommandError> {
        let _admission = self.begin_mutation()?;
        let id = parse_profile_id(id)?;
        let lock = self.profile_lock(&id);
        let guard = lock.lock().expect("profile mutation lock poisoned");
        let stored = self.repository.get(&id)?;
        if stored.kind != AgentIntegrationKind::Custom {
            return Err(invalid("presetProfileCannotBeDeleted"));
        }
        if self
            .repository
            .get_installation(&id)?
            .is_some_and(|installation| {
                matches!(
                    installation.state,
                    IntegrationState::Installed | IntegrationState::NeedsRepair
                )
            })
        {
            return Err(conflict(&id));
        }
        self.stop_runtime(id.as_str());
        let result = self.repository.delete(&id, expected_revision);
        drop(guard);
        if result.is_ok() {
            self.mutation_locks
                .lock()
                .expect("profile lock map poisoned")
                .remove(id.as_str());
        }
        result
    }

    pub fn restore_installed_custom_profiles(&self) -> Result<usize, CommandError> {
        let _admission = self.begin_mutation()?;
        let _capacity = self
            .installation_capacity
            .lock()
            .expect("profile installation capacity lock poisoned");
        self.preset_bridge.ensure_started()?;
        #[cfg(test)]
        self.call_lifecycle_hook("agentProfilesRestore");
        let mut started = 0;
        for profile in self.repository.list()? {
            if profile.environment != AgentEnvironment::Windows || !profile.enabled {
                continue;
            }
            let Some(installation) = self.repository.get_installation(&profile.id)? else {
                continue;
            };
            if installation.state != IntegrationState::Installed {
                continue;
            }
            let lock = self.profile_lock(&profile.id);
            let _profile_guard = lock.lock().expect("profile mutation lock poisoned");
            if profile.kind == AgentIntegrationKind::Preset {
                if self
                    .preset_bridge
                    .validate_installation(&profile, &installation)
                    .is_err()
                {
                    self.mark_needs_repair(
                        &profile.id,
                        "presetReceiptOrConfigMismatch",
                        now_millis(),
                    );
                }
                continue;
            }
            if started >= MAX_INSTALLED_CUSTOM_PROFILES {
                self.mark_needs_repair(
                    &profile.id,
                    "agentProfileInstallLimitReached",
                    now_millis(),
                );
                continue;
            }
            let restored = validate_and_canonicalize_custom_target(
                &profile.environment,
                &profile.config_target,
            )
            .and_then(|target| {
                let fingerprint_matches = installation
                    .owned_fingerprint
                    .as_deref()
                    .is_some_and(|fingerprint| fingerprint == custom_target_fingerprint(&target));
                let executable_matches =
                    installation
                        .external_hash
                        .as_deref()
                        .is_some_and(|external_hash| {
                            hash_file(&target.executable).ok().as_deref() == Some(external_hash)
                        });
                if !fingerprint_matches || !executable_matches {
                    return Err(conflict(&profile.id));
                }
                self.start_runtime(&profile, target)
            });
            match restored {
                Ok(()) => started += 1,
                Err(_) => {
                    self.mark_needs_repair(&profile.id, "customHookReceiptMismatch", now_millis())
                }
            }
        }
        Ok(started)
    }

    pub fn shutdown(&self) {
        self.stop_accepting();
        let _admission = self
            .admission
            .write()
            .expect("agent profile admission lock poisoned");
        if self
            .shutdown_complete
            .compare_exchange(false, true, AtomicOrdering::AcqRel, AtomicOrdering::Acquire)
            .is_err()
        {
            return;
        }
        #[cfg(test)]
        self.call_lifecycle_hook("agentProfilesShutdown");
        let handles = self
            .runtimes
            .lock()
            .expect("custom runtime map lock poisoned")
            .drain()
            .map(|(_, handle)| handle)
            .collect::<Vec<_>>();
        for handle in handles {
            handle.stop();
        }
        self.preset_bridge.shutdown();
    }

    pub fn stop_accepting(&self) {
        self.accepting.store(false, AtomicOrdering::Release);
    }

    fn begin_mutation(&self) -> Result<RwLockReadGuard<'_, ()>, CommandError> {
        let admission = self
            .admission
            .read()
            .expect("agent profile admission lock poisoned");
        if !self.accepting.load(AtomicOrdering::Acquire) {
            return Err(service_stopping_error());
        }
        #[cfg(test)]
        if let Some(hook) = self
            .admission_hook
            .lock()
            .expect("agent profile admission hook lock poisoned")
            .clone()
        {
            hook();
        }
        Ok(admission)
    }

    #[cfg(test)]
    pub(crate) fn set_lifecycle_hook(&self, hook: Arc<dyn Fn(&'static str) + Send + Sync>) {
        *self
            .lifecycle_hook
            .lock()
            .expect("agent profile lifecycle hook lock poisoned") = Some(hook);
    }

    #[cfg(test)]
    fn set_admission_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        *self
            .admission_hook
            .lock()
            .expect("agent profile admission hook lock poisoned") = Some(hook);
    }

    #[cfg(test)]
    fn call_lifecycle_hook(&self, phase: &'static str) {
        if let Some(hook) = self
            .lifecycle_hook
            .lock()
            .expect("agent profile lifecycle hook lock poisoned")
            .clone()
        {
            hook(phase);
        }
    }

    fn save_preset_profile(
        &self,
        input: SaveAgentIntegrationProfileInput,
    ) -> Result<AgentIntegrationProfile, CommandError> {
        let id = parse_profile_id(
            input
                .id
                .as_deref()
                .ok_or_else(|| invalid("presetIdRequired"))?,
        )?;
        let lock = self.profile_lock(&id);
        let _guard = lock.lock().expect("profile mutation lock poisoned");
        let stored = self.repository.get(&id)?;
        let expected = preset_identity(&stored)?;
        if input.expected_revision != Some(stored.revision)
            || input.display_name != stored.display_name
            || input.environment != stored.environment
            || input.config_target != stored.config_target
            || input.event_mapping != stored.event_mapping
            || input.enabled != stored.enabled
            || input.kind != stored.kind
            || id.as_str() != expected.as_str()
        {
            return Err(invalid("presetProfileImmutable"));
        }
        self.profile_contract(stored)
    }

    fn save_custom_profile(
        &self,
        input: SaveAgentIntegrationProfileInput,
        now: i64,
    ) -> Result<AgentIntegrationProfile, CommandError> {
        let target =
            validate_and_canonicalize_custom_target(&input.environment, &input.config_target)?;
        let config_target = AgentConfigTarget::CustomHook {
            executable: target.executable.display().to_string(),
            argv: target.argv,
            working_directory: target
                .working_directory
                .map(|directory| directory.display().to_string()),
            timeout_seconds: i64::try_from(target.timeout_seconds)
                .map_err(|_| invalid("timeoutOutOfRange"))?,
        };
        let id = match input.id.as_deref() {
            Some(id) => {
                let id = parse_profile_id(id)?;
                uuid::Uuid::parse_str(id.as_str()).map_err(|_| invalid("customIdInvalid"))?;
                id
            }
            None => AgentIntegrationId::parse(uuid::Uuid::new_v4().to_string())
                .expect("UUID is a valid profile id"),
        };
        let _capacity = self
            .installation_capacity
            .lock()
            .expect("profile installation capacity lock poisoned");
        let lock = self.profile_lock(&id);
        let _guard = lock.lock().expect("profile mutation lock poisoned");
        if input.id.is_none() && self.repository.count_custom_profiles()? >= MAX_CUSTOM_PROFILES {
            return Err(profile_capacity_error());
        }
        let (revision, created_at) = match input.id.as_deref() {
            Some(_) => {
                let existing = self.repository.get(&id)?;
                if existing.kind != AgentIntegrationKind::Custom {
                    return Err(invalid("customIdInvalid"));
                }
                if self
                    .repository
                    .get_installation(&id)?
                    .is_some_and(|installation| {
                        matches!(
                            installation.state,
                            IntegrationState::Installed | IntegrationState::NeedsRepair
                        )
                    })
                {
                    return Err(conflict(&id));
                }
                if input.expected_revision != Some(existing.revision) {
                    return Err(conflict(&id));
                }
                (existing.revision, existing.created_at)
            }
            None => {
                if input.expected_revision.is_some() {
                    return Err(invalid("customRevisionOnCreate"));
                }
                (0, now)
            }
        };
        let stored = StoredAgentIntegrationProfile {
            id,
            kind: AgentIntegrationKind::Custom,
            display_name: input.display_name,
            environment: input.environment,
            config_target,
            event_mapping: input.event_mapping,
            enabled: false,
            revision,
            created_at,
            updated_at: now,
        };
        let stored = self.repository.save(&stored, input.expected_revision)?;
        self.profile_contract(stored)
    }

    fn activate_profile(
        &self,
        id: &str,
        expected_revision: i64,
        now: i64,
        repair: bool,
    ) -> Result<AgentIntegrationProfile, CommandError> {
        let id = parse_profile_id(id)?;
        let _capacity = self
            .installation_capacity
            .lock()
            .expect("profile installation capacity lock poisoned");
        let lock = self.profile_lock(&id);
        let _guard = lock.lock().expect("profile mutation lock poisoned");
        let stored = self.repository.get(&id)?;
        if stored.revision != expected_revision {
            return Err(conflict(&id));
        }
        if !repair {
            if let Some(existing) = self.repository.get_installation(&id)? {
                match existing.state {
                    IntegrationState::Installed => return self.profile_contract(stored),
                    IntegrationState::NeedsRepair => return Err(conflict(&id)),
                    IntegrationState::NotInstalled | IntegrationState::Unsupported => {}
                }
            }
        }
        let mut runtime = None;
        let mut preset_mutation = None;
        let installation = match stored.kind {
            AgentIntegrationKind::Custom => {
                if self
                    .repository
                    .count_active_custom_installations_excluding(&id)?
                    >= MAX_INSTALLED_CUSTOM_PROFILES
                {
                    return Err(install_capacity_error());
                }
                if stored.environment != AgentEnvironment::Windows {
                    return Err(unsupported("customHookWslNotSupported"));
                }
                if repair {
                    self.stop_runtime(id.as_str());
                }
                let target = validate_and_canonicalize_custom_target(
                    &stored.environment,
                    &stored.config_target,
                )?;
                let fingerprint = custom_target_fingerprint(&target);
                let executable_hash = hash_file(&target.executable)?;
                runtime = Some(self.prepare_runtime(&stored, target.clone())?);
                AgentProfileInstallation {
                    profile_id: id.clone(),
                    state: IntegrationState::Installed,
                    reason_code: None,
                    owned_resource: Some(format!("custom-process:{}", target.executable.display())),
                    owned_fingerprint: Some(fingerprint),
                    external_hash: Some(executable_hash),
                    updated_at: now,
                }
            }
            AgentIntegrationKind::Preset => {
                let outcome = self.install_preset(&stored, now)?;
                preset_mutation = Some(outcome.mutation);
                outcome.installation
            }
        };
        match self
            .repository
            .set_installation(&installation, expected_revision, true)
        {
            Ok(stored) => {
                if matches!(stored.kind, AgentIntegrationKind::Custom) {
                    let mut runtime = runtime.ok_or_else(|| io_error("hookRuntimeMissing"))?;
                    if let Err(error) = runtime.activate() {
                        runtime.stop();
                        self.mark_needs_repair(&id, "hookExitedBeforeActivation", now_millis());
                        return Err(error);
                    }
                    self.runtimes
                        .lock()
                        .expect("custom runtime map lock poisoned")
                        .insert(id.as_str().into(), runtime);
                } else {
                    self.preset_bridge.finish_install(&id, true);
                }
                self.profile_contract(stored)
            }
            Err(error) => {
                if let Some(runtime) = runtime {
                    runtime.stop();
                }
                let rollback = preset_mutation.map(ConfigMutation::rollback).transpose();
                if matches!(stored.kind, AgentIntegrationKind::Preset) {
                    self.preset_bridge.finish_install(&id, false);
                }
                if let Err(rollback_error) = rollback {
                    return Err(rollback_error);
                }
                Err(error)
            }
        }
    }

    fn prepare_runtime(
        &self,
        profile: &StoredAgentIntegrationProfile,
        target: ValidatedCustomHookTarget,
    ) -> Result<CustomRuntimeHandle, CommandError> {
        if self
            .runtimes
            .lock()
            .expect("custom runtime map lock poisoned")
            .contains_key(profile.id.as_str())
        {
            return Err(conflict(&profile.id));
        }
        spawn_custom_runtime(
            profile.id.clone(),
            profile.event_mapping.clone(),
            target,
            self.repository.clone(),
            self.emitter.clone(),
        )
    }

    fn start_runtime(
        &self,
        profile: &StoredAgentIntegrationProfile,
        target: ValidatedCustomHookTarget,
    ) -> Result<(), CommandError> {
        if self
            .runtimes
            .lock()
            .expect("custom runtime map lock poisoned")
            .contains_key(profile.id.as_str())
        {
            return Ok(());
        }
        let mut handle = self.prepare_runtime(profile, target)?;
        handle.activate()?;
        self.runtimes
            .lock()
            .expect("custom runtime map lock poisoned")
            .insert(profile.id.as_str().into(), handle);
        Ok(())
    }

    fn stop_runtime(&self, id: &str) {
        let handle = self
            .runtimes
            .lock()
            .expect("custom runtime map lock poisoned")
            .remove(id);
        if let Some(handle) = handle {
            handle.stop();
        }
    }

    fn mark_needs_repair(&self, id: &AgentIntegrationId, reason: &str, now: i64) {
        if self
            .repository
            .update_installation_health(id, IntegrationState::NeedsRepair, Some(reason), now)
            .is_ok()
        {
            let _ = self.emitter.emit(
                AGENT_PROFILE_STATE_CHANGED,
                agent_profile_change_payload(id.as_str(), &format!("health-{now}"), now),
            );
        }
    }

    fn profile_contract(
        &self,
        stored: StoredAgentIntegrationProfile,
    ) -> Result<AgentIntegrationProfile, CommandError> {
        let (installation_state, reason_code) = match (&stored.kind, &stored.environment) {
            (_, AgentEnvironment::Wsl) => (
                IntegrationState::Unsupported,
                Some("profileWslNotSupported".into()),
            ),
            (AgentIntegrationKind::Preset, AgentEnvironment::Windows)
                if matches!(
                    stored.config_target,
                    AgentConfigTarget::Preset {
                        adapter_id: PresetAgentAdapterId::Trae
                    }
                ) =>
            {
                (
                    IntegrationState::Unsupported,
                    Some("traeHooksVersionOrConfigUnavailable".into()),
                )
            }
            _ => self
                .repository
                .get_installation(&stored.id)?
                .map(|installation| (installation.state, installation.reason_code))
                .unwrap_or((IntegrationState::NotInstalled, None)),
        };
        Ok(AgentIntegrationProfile {
            id: stored.id.as_str().into(),
            kind: stored.kind,
            display_name: stored.display_name,
            environment: stored.environment,
            config_target: stored.config_target,
            event_mapping: stored.event_mapping,
            enabled: stored.enabled,
            installation_state,
            reason_code,
            revision: (stored.revision >= 1)
                .then_some(stored.revision)
                .ok_or_else(|| invalid("profileRevision"))?,
            updated_at: stored.updated_at,
        })
    }

    fn profile_lock(&self, id: &AgentIntegrationId) -> Arc<Mutex<()>> {
        self.mutation_locks
            .lock()
            .expect("profile lock map poisoned")
            .entry(id.as_str().into())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn install_preset(
        &self,
        stored: &StoredAgentIntegrationProfile,
        now: i64,
    ) -> Result<PresetInstallOutcome, CommandError> {
        self.preset_bridge.install(stored, now)
    }

    fn uninstall_preset(
        &self,
        stored: &StoredAgentIntegrationProfile,
        now: i64,
    ) -> Result<Option<ConfigMutation>, CommandError> {
        let Some(installation) = self.repository.get_installation(&stored.id)? else {
            return Ok(None);
        };
        if installation.state == IntegrationState::NotInstalled {
            return Ok(None);
        }
        self.preset_bridge
            .uninstall(stored, &installation, now)
            .map(Some)
    }
}

fn spawn_custom_runtime(
    profile_id: AgentIntegrationId,
    mapping: Vec<AgentEventMapping>,
    target: ValidatedCustomHookTarget,
    repository: AgentProfileRepository,
    emitter: Arc<dyn EventEmitterPort>,
) -> Result<CustomRuntimeHandle, CommandError> {
    let activation_timeout = Duration::from_secs(target.timeout_seconds);
    let mut command = Command::new(&target.executable);
    command.args(&target.argv);
    if let Some(directory) = &target.working_directory {
        command.current_dir(directory);
    }
    command
        .env("AICELAND_PROFILE_ADAPTER", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let child = command.spawn().map_err(map_spawn_error)?;
    let mut process = OwnedChildProcess::new(child)?;
    let handshake = serde_json::json!({
        "protocolVersion": 1,
        "profileId": profile_id.as_str(),
    });
    let mut stdin = process
        .child
        .stdin
        .take()
        .ok_or_else(|| io_error("hookStdin"))?;
    serde_json::to_writer(&mut stdin, &handshake).map_err(|_| io_error("hookHandshake"))?;
    stdin
        .write_all(b"\n")
        .map_err(|_| io_error("hookHandshake"))?;
    stdin.flush().map_err(|_| io_error("hookHandshake"))?;
    drop(stdin);
    let stdout = process
        .child
        .stdout
        .take()
        .ok_or_else(|| io_error("hookStdout"))?;
    let stderr = process
        .child
        .stderr
        .take()
        .ok_or_else(|| io_error("hookStderr"))?;
    let (control_tx, control_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let join = thread::Builder::new()
        .name(format!("agent-profile-{}", profile_id.as_str()))
        .spawn(move || {
            run_custom_runtime_session(
                process, stdout, stderr, control_rx, ready_tx, profile_id, mapping, repository,
                emitter,
            );
        })
        .map_err(|_| io_error("hookWorker"))?;
    let runtime = CustomRuntimeHandle {
        control: control_tx,
        join: Some(join),
        activated_at: None,
    };
    if let Err(error) = await_ready(ready_rx, activation_timeout) {
        runtime.stop();
        return Err(error);
    }
    Ok(runtime)
}

struct OwnedChildProcess {
    child: Child,
    #[cfg(windows)]
    job: Option<std::os::windows::io::OwnedHandle>,
}

impl OwnedChildProcess {
    fn new(child: Child) -> Result<Self, CommandError> {
        let mut owned = Self {
            child,
            #[cfg(windows)]
            job: None,
        };
        #[cfg(windows)]
        {
            owned.job = Some(create_kill_on_close_job(&owned.child)?);
        }
        Ok(owned)
    }

    fn terminate_and_wait(&mut self) {
        #[cfg(windows)]
        if let Some(job) = self.job.take() {
            use std::os::windows::io::AsRawHandle;
            use windows::Win32::Foundation::HANDLE;
            use windows::Win32::System::JobObjects::TerminateJobObject;

            let handle = HANDLE(job.as_raw_handle());
            let _ = unsafe { TerminateJobObject(handle, 1) };
            drop(job);
        }
        let _ = self.child.kill();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for OwnedChildProcess {
    fn drop(&mut self) {
        self.terminate_and_wait();
    }
}

#[cfg(windows)]
fn create_kill_on_close_job(
    child: &Child,
) -> Result<std::os::windows::io::OwnedHandle, CommandError> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    let raw_job =
        unsafe { CreateJobObjectW(None, PCWSTR::null()) }.map_err(|_| io_error("hookJobCreate"))?;
    let job = unsafe { OwnedHandle::from_raw_handle(raw_job.0) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let job_handle = HANDLE(job.as_raw_handle());
    unsafe {
        SetInformationJobObject(
            job_handle,
            JobObjectExtendedLimitInformation,
            std::ptr::addr_of!(limits).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .map_err(|_| io_error("hookJobConfigure"))?;
        AssignProcessToJobObject(job_handle, HANDLE(child.as_raw_handle()))
            .map_err(|_| io_error("hookJobAssign"))?;
    }
    Ok(job)
}

struct BoundedReaderThread {
    done: mpsc::Receiver<()>,
    join: Option<thread::JoinHandle<()>>,
}

impl BoundedReaderThread {
    fn finish(mut self) {
        if self.done.recv_timeout(Duration::from_secs(2)).is_ok() {
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }
}

fn read_bounded_line<R: BufRead>(reader: &mut R) -> Result<Option<Vec<u8>>, &'static str> {
    let mut line = Vec::with_capacity(MAX_EVENT_LINE_BYTES.min(8 * 1024));
    loop {
        let available = reader.fill_buf().map_err(|_| "hookStdoutRead")?;
        if available.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(Some(line));
        }
        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            if line.len().saturating_add(newline) > MAX_EVENT_LINE_BYTES {
                return Err("eventLineTooLarge");
            }
            line.extend_from_slice(&available[..newline]);
            reader.consume(newline + 1);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(Some(line));
        }
        if line.len().saturating_add(available.len()) > MAX_EVENT_LINE_BYTES {
            return Err("eventLineTooLarge");
        }
        let consumed = available.len();
        line.extend_from_slice(available);
        reader.consume(consumed);
    }
}

#[allow(clippy::too_many_arguments)]
fn run_custom_runtime_session(
    mut process: OwnedChildProcess,
    stdout: impl Read + Send + 'static,
    stderr: impl Read + Send + 'static,
    control: mpsc::Receiver<RuntimeControl>,
    ready: mpsc::SyncSender<Result<(), &'static str>>,
    profile_id: AgentIntegrationId,
    mapping: Vec<AgentEventMapping>,
    repository: AgentProfileRepository,
    emitter: Arc<dyn EventEmitterPort>,
) {
    let (line_tx, line_rx) = mpsc::sync_channel::<Result<Vec<u8>, &'static str>>(64);
    let (stdout_done_tx, stdout_done_rx) = mpsc::channel();
    let reader = BoundedReaderThread {
        done: stdout_done_rx,
        join: Some(thread::spawn(move || {
            let _done = ScopeDone(stdout_done_tx);
            let mut reader = BufReader::new(stdout);
            loop {
                match read_bounded_line(&mut reader) {
                    Ok(None) => break,
                    Ok(Some(line)) => {
                        if line_tx.send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Err(reason) => {
                        let _ = line_tx.send(Err(reason));
                        break;
                    }
                }
            }
        })),
    };
    let (stderr_done_tx, stderr_done_rx) = mpsc::channel();
    let stderr_reader = BoundedReaderThread {
        done: stderr_done_rx,
        join: Some(thread::spawn(move || {
            let _done = ScopeDone(stderr_done_tx);
            discard_stream(stderr);
        })),
    };
    let mut ready_sent = false;
    loop {
        match control.try_recv() {
            Ok(RuntimeControl::Stop) | Err(mpsc::TryRecvError::Disconnected) => break,
            Ok(RuntimeControl::Activate(reply)) => {
                let _ = reply.send(Err("hookReadyPending"));
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        match line_rx.recv_timeout(Duration::from_millis(25)) {
            Ok(Ok(line)) => {
                let result = parse_custom_ready_line(&line);
                let accepted = result.is_ok();
                let _ = ready.send(result);
                ready_sent = true;
                if accepted {
                    break;
                }
                break;
            }
            Ok(Err(reason)) => {
                let _ = ready.send(Err(reason));
                ready_sent = true;
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = ready.send(Err("hookExitedBeforeReady"));
                ready_sent = true;
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if process.child.try_wait().ok().flatten().is_some() {
                    let _ = ready.send(Err("hookExitedBeforeReady"));
                    ready_sent = true;
                    break;
                }
            }
        }
    }
    if !ready_sent || parse_control_until_activation(&control, &mut process).is_err() {
        drop(line_rx);
        process.terminate_and_wait();
        reader.finish();
        stderr_reader.finish();
        return;
    }
    let mut stopped = false;
    let mut failure = None;
    loop {
        match control.try_recv() {
            Ok(RuntimeControl::Stop) | Err(mpsc::TryRecvError::Disconnected) => {
                stopped = true;
                break;
            }
            Ok(RuntimeControl::Activate(reply)) => {
                let _ = reply.send(Err("hookAlreadyActive"));
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        match line_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Ok(line)) => {
                let received_at = now_millis();
                match parse_custom_event_line(&profile_id, &mapping, &line, received_at).and_then(
                    |event| {
                        project_and_emit_profile_event(
                            &repository,
                            emitter.as_ref(),
                            &event,
                            received_at,
                        )
                    },
                ) {
                    Ok(()) => {}
                    Err(_) => {
                        failure = Some("hookEventRejected");
                        break;
                    }
                }
            }
            Ok(Err(reason)) => {
                failure = Some(reason);
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                failure = Some("hookExited");
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if process.child.try_wait().ok().flatten().is_some() {
                    failure = Some("hookExited");
                    break;
                }
            }
        }
    }
    drop(line_rx);
    process.terminate_and_wait();
    reader.finish();
    stderr_reader.finish();
    if !stopped {
        let now = now_millis();
        if repository
            .update_installation_health(&profile_id, IntegrationState::NeedsRepair, failure, now)
            .is_ok()
        {
            let _ = emitter.emit(
                AGENT_PROFILE_STATE_CHANGED,
                agent_profile_change_payload(profile_id.as_str(), &format!("health-{now}"), now),
            );
        }
    }
}

fn parse_control_until_activation(
    control: &mpsc::Receiver<RuntimeControl>,
    process: &mut OwnedChildProcess,
) -> Result<(), ()> {
    loop {
        match control.recv_timeout(Duration::from_millis(25)) {
            Ok(RuntimeControl::Activate(reply)) => {
                let result = if process.child.try_wait().ok().flatten().is_some() {
                    Err("hookExitedBeforeActivation")
                } else {
                    Ok(())
                };
                let activated = result.is_ok();
                let _ = reply.send(result);
                return activated.then_some(()).ok_or(());
            }
            Ok(RuntimeControl::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return Err(()),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if process.child.try_wait().ok().flatten().is_some() {
                    return Err(());
                }
            }
        }
    }
}

fn await_activation(
    reply: mpsc::Receiver<Result<(), &'static str>>,
    timeout: Duration,
) -> Result<(), CommandError> {
    reply
        .recv_timeout(timeout)
        .map_err(|_| io_error("hookActivationTimeout"))?
        .map_err(io_error)
}

fn await_ready(
    reply: mpsc::Receiver<Result<(), &'static str>>,
    timeout: Duration,
) -> Result<(), CommandError> {
    reply
        .recv_timeout(timeout)
        .map_err(|_| io_error("hookReadyTimeout"))?
        .map_err(io_error)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CustomReadyWire {
    #[serde(rename = "type")]
    kind: String,
    protocol_version: u8,
}

fn parse_custom_ready_line(line: &[u8]) -> Result<(), &'static str> {
    let ready: CustomReadyWire = serde_json::from_slice(line).map_err(|_| "hookReadyInvalid")?;
    if ready.kind == "ready" && ready.protocol_version == 1 {
        Ok(())
    } else {
        Err("hookReadyInvalid")
    }
}

fn discard_stream(mut reader: impl Read) {
    let mut discard = [0u8; 8 * 1024];
    while reader.read(&mut discard).is_ok_and(|read| read > 0) {}
}

struct ScopeDone(mpsc::Sender<()>);

impl Drop for ScopeDone {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

fn project_and_emit_profile_event(
    repository: &AgentProfileRepository,
    emitter: &dyn EventEmitterPort,
    event: &ValidatedAgentProfileEvent,
    received_at: i64,
) -> Result<(), CommandError> {
    let outcome = repository.project_event(event, received_at)?;
    if outcome == AgentProfileProjectionOutcome::Advanced {
        let _ = emitter.emit(
            AGENT_PROFILE_STATE_CHANGED,
            agent_profile_state_changed_payload(event),
        );
    }
    Ok(())
}

fn preset_identity(profile: &StoredAgentIntegrationProfile) -> Result<String, CommandError> {
    let AgentConfigTarget::Preset { adapter_id } = &profile.config_target else {
        return Err(invalid("presetTargetRequired"));
    };
    let adapter = match adapter_id {
        PresetAgentAdapterId::Kimi => "kimi",
        PresetAgentAdapterId::Trae => "trae",
        PresetAgentAdapterId::Qoderwork => "qoderwork",
        PresetAgentAdapterId::Cursor => "cursor",
    };
    let environment = match profile.environment {
        AgentEnvironment::Windows => "windows",
        AgentEnvironment::Wsl => "wsl",
    };
    Ok(format!("{adapter}-{environment}"))
}

fn parse_profile_id(value: &str) -> Result<AgentIntegrationId, CommandError> {
    AgentIntegrationId::parse(value.to_owned()).ok_or_else(|| invalid("profileIdInvalid"))
}

fn custom_target_fingerprint(target: &ValidatedCustomHookTarget) -> String {
    let material = serde_json::json!({
        "executable": target.executable,
        "argv": target.argv,
        "workingDirectory": target.working_directory,
        "timeoutSeconds": target.timeout_seconds,
    });
    sha256_hex(material.to_string().as_bytes())
}

fn hash_file(path: &Path) -> Result<String, CommandError> {
    let mut file = std::fs::File::open(path).map_err(|_| io_error("executableRead"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| io_error("executableRead"))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn detected_preset_apps(process_names: &[&str]) -> BTreeSet<PresetAgentAdapterId> {
    process_names
        .iter()
        .filter_map(
            |process_name| match process_name.to_ascii_lowercase().as_str() {
                "kimi.exe" | "kimi-code.exe" | "kimicode.exe" | "kimiwork.exe"
                | "kimi work.exe" => Some(PresetAgentAdapterId::Kimi),
                "trae.exe" | "trae cn.exe" | "trae solo.exe" | "trae solo cn.exe"
                | "traework.exe" | "traework cn.exe" | "trae work.exe" | "trae work cn.exe" => {
                    Some(PresetAgentAdapterId::Trae)
                }
                "qoder.exe" | "qoderwork.exe" | "qwenworkcn.exe" => {
                    Some(PresetAgentAdapterId::Qoderwork)
                }
                "cursor.exe" => Some(PresetAgentAdapterId::Cursor),
                _ => None,
            },
        )
        .collect()
}

fn aggregate_profile_status(
    observations: &[crate::domain::agent_profiles::AgentProfileObservation],
    installed_and_enabled: bool,
    runtime_active: bool,
    observation_epoch: Option<i64>,
    now: i64,
) -> AgentStatus {
    if !installed_and_enabled {
        return AgentStatus::Offline;
    }
    let observation = observations
        .iter()
        .filter(|observation| {
            if observation_epoch.is_some_and(|epoch| observation.received_at < epoch) {
                return false;
            }
            let age = now.saturating_sub(observation.received_at).max(0);
            age <= match observation.status {
                AgentStatus::Completed | AgentStatus::Failed | AgentStatus::Timeout => {
                    TERMINAL_STATUS_VISIBLE_MILLIS
                }
                _ => ACTIVE_STATUS_STALE_MILLIS,
            }
        })
        .max_by_key(|observation| {
            (
                aggregate_status_rank(&observation.status),
                observation.received_at,
                observation.occurred_at,
            )
        });
    let Some(observation) = observation else {
        return if runtime_active {
            AgentStatus::Idle
        } else {
            AgentStatus::Offline
        };
    };
    if observation.status == AgentStatus::Running
        && observations.iter().any(|candidate| {
            candidate.status == AgentStatus::Completed
                && !observation_epoch.is_some_and(|epoch| candidate.received_at < epoch)
                && candidate.received_at <= now
                && now.saturating_sub(candidate.received_at) < COMPLETION_FLASH_MILLIS
        })
    {
        AgentStatus::Completed
    } else {
        observation.status.clone()
    }
}

fn observation_is_current(
    observation: &crate::domain::agent_profiles::AgentProfileObservation,
    observation_epoch: Option<i64>,
    now: i64,
) -> bool {
    let Some(epoch) = observation_epoch else {
        return false;
    };
    if observation.received_at < epoch {
        return false;
    }
    let age = now.saturating_sub(observation.received_at).max(0);
    age <= match observation.status {
        AgentStatus::Completed | AgentStatus::Failed | AgentStatus::Timeout => {
            TERMINAL_STATUS_VISIBLE_MILLIS
        }
        _ => ACTIVE_STATUS_STALE_MILLIS,
    }
}

fn aggregate_status_rank(status: &AgentStatus) -> u8 {
    match status {
        AgentStatus::Running => 7,
        AgentStatus::Waiting => 6,
        AgentStatus::Failed => 5,
        AgentStatus::Timeout => 4,
        AgentStatus::Completed => 3,
        AgentStatus::Idle => 2,
        AgentStatus::Offline => 1,
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn map_spawn_error(error: std::io::Error) -> CommandError {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        CommandError::with_detail(
            AppErrorCode::PermissionDenied,
            "errors.permissionDenied",
            "reasonCode",
            SafeParameterValue::String("hookSpawnPermissionDenied".into()),
            false,
        )
    } else {
        io_error("hookSpawn")
    }
}

fn io_error(reason: &str) -> CommandError {
    CommandError::with_detail(
        AppErrorCode::SourceUnavailable,
        "errors.sourceUnavailable",
        "reasonCode",
        SafeParameterValue::String(reason.into()),
        true,
    )
}

fn service_stopping_error() -> CommandError {
    CommandError {
        code: AppErrorCode::SourceUnavailable,
        message_key: "errors.serviceStopping".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

fn install_capacity_error() -> CommandError {
    CommandError::with_detail(
        AppErrorCode::Conflict,
        "errors.conflict",
        "reasonCode",
        SafeParameterValue::String("agentProfileInstallLimitReached".into()),
        false,
    )
}

fn profile_capacity_error() -> CommandError {
    CommandError::with_detail(
        AppErrorCode::Conflict,
        "errors.conflict",
        "reasonCode",
        SafeParameterValue::String("agentProfileLimitReached".into()),
        false,
    )
}

fn conflict(id: &AgentIntegrationId) -> CommandError {
    CommandError::with_detail(
        AppErrorCode::Conflict,
        "errors.conflict",
        "entityId",
        SafeParameterValue::String(id.as_str().into()),
        true,
    )
}

fn validate_and_canonicalize_custom_target(
    environment: &AgentEnvironment,
    target: &AgentConfigTarget,
) -> Result<ValidatedCustomHookTarget, CommandError> {
    if !matches!(environment, AgentEnvironment::Windows) {
        return Err(unsupported("customHookWslNotSupported"));
    }
    let AgentConfigTarget::CustomHook {
        executable,
        argv,
        working_directory,
        timeout_seconds,
    } = target
    else {
        return Err(invalid("customHookTargetRequired"));
    };
    validate_custom_target_fields(
        executable,
        argv,
        working_directory.as_deref(),
        *timeout_seconds,
    )?;
    let executable =
        std::fs::canonicalize(executable).map_err(|_| invalid("executableNotFound"))?;
    if !executable.is_absolute() || !executable.is_file() {
        return Err(invalid("executableMustBeFile"));
    }
    let file_name = executable
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| invalid("executableFileName"))?;
    if executable
        .extension()
        .and_then(|extension| extension.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("exe"))
    {
        return Err(invalid("executableMustBeExe"));
    }
    if [
        "cmd.exe",
        "powershell.exe",
        "pwsh.exe",
        "wscript.exe",
        "cscript.exe",
        "mshta.exe",
        "rundll32.exe",
    ]
    .contains(&file_name.as_str())
    {
        return Err(invalid("shellExecutableDenied"));
    }
    let working_directory = working_directory
        .as_ref()
        .map(|path| {
            std::fs::canonicalize(path)
                .map_err(|_| invalid("workingDirectoryNotFound"))
                .and_then(|canonical| {
                    canonical
                        .is_dir()
                        .then_some(canonical)
                        .ok_or_else(|| invalid("workingDirectoryMustBeDirectory"))
                })
        })
        .transpose()?;
    Ok(ValidatedCustomHookTarget {
        executable,
        argv: argv.clone(),
        working_directory,
        timeout_seconds: u64::try_from(*timeout_seconds)
            .map_err(|_| invalid("timeoutOutOfRange"))?,
    })
}

fn validate_profile_text_and_mapping(
    display_name: &str,
    event_mapping: &[AgentEventMapping],
    target: &AgentConfigTarget,
) -> Result<(), CommandError> {
    if display_name.trim() != display_name
        || !(1..=MAX_DISPLAY_CHARS).contains(&display_name.chars().count())
        || has_control(display_name)
    {
        return Err(invalid("invalidDisplayName"));
    }
    if !(1..=MAX_EVENT_MAPPINGS).contains(&event_mapping.len()) {
        return Err(invalid("invalidEventMappingCount"));
    }
    let mut native_events = BTreeSet::new();
    for mapping in event_mapping {
        if mapping.native_event.trim() != mapping.native_event
            || !(1..=MAX_NATIVE_EVENT_CHARS).contains(&mapping.native_event.chars().count())
            || has_control(&mapping.native_event)
            || !native_events.insert(mapping.native_event.to_ascii_lowercase())
        {
            return Err(invalid("invalidNativeEvent"));
        }
    }
    if let AgentConfigTarget::CustomHook {
        executable,
        argv,
        working_directory,
        timeout_seconds,
    } = target
    {
        validate_custom_target_fields(
            executable,
            argv,
            working_directory.as_deref(),
            *timeout_seconds,
        )?;
    }
    let payload_bytes = serde_json::to_vec(&(display_name, event_mapping, target))
        .map_err(|_| invalid("profilePayload"))?;
    if payload_bytes.len() > MAX_PROFILE_PAYLOAD_BYTES {
        return Err(invalid("profilePayloadTooLarge"));
    }
    Ok(())
}

fn parse_custom_event_line(
    profile_id: &AgentIntegrationId,
    mapping: &[AgentEventMapping],
    line: &[u8],
    received_at: i64,
) -> Result<ValidatedAgentProfileEvent, CommandError> {
    if line.is_empty()
        || line.len() > MAX_EVENT_LINE_BYTES
        || line.contains(&b'\n')
        || line.contains(&b'\r')
    {
        return Err(invalid("eventLineTooLarge"));
    }
    let mut deserializer = serde_json::Deserializer::from_slice(line);
    let wire = CustomHookEventWire::deserialize(&mut deserializer)
        .map_err(|_| invalid("invalidHookEvent"))?;
    deserializer
        .end()
        .map_err(|_| invalid("invalidHookEvent"))?;
    if !valid_identifier(&wire.task_id, MAX_TASK_ID_CHARS)
        || !valid_identifier(&wire.source_event_id, MAX_EVENT_ID_CHARS)
        || !valid_identifier(&wire.native_event, MAX_NATIVE_EVENT_CHARS)
        || wire.occurred_at < received_at.saturating_sub(24 * 60 * 60 * 1000)
        || wire.occurred_at > received_at.saturating_add(5 * 60 * 1000)
        || received_at < 0
    {
        return Err(invalid("invalidHookEvent"));
    }
    let status = mapping
        .iter()
        .find(|candidate| {
            candidate
                .native_event
                .eq_ignore_ascii_case(&wire.native_event)
        })
        .map(|candidate| candidate.normalized_status.clone())
        .ok_or_else(|| invalid("unknownNativeEvent"))?;
    Ok(ValidatedAgentProfileEvent {
        event_id: wire.source_event_id,
        profile_id: profile_id.clone(),
        native_event: wire.native_event,
        task_id: wire.task_id,
        status,
        occurred_at: wire.occurred_at,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CustomHookEventWire {
    native_event: String,
    task_id: String,
    source_event_id: String,
    occurred_at: i64,
}

fn validate_custom_target_fields(
    executable: &str,
    argv: &[String],
    working_directory: Option<&str>,
    timeout_seconds: i64,
) -> Result<(), CommandError> {
    if executable.is_empty()
        || executable.chars().count() > MAX_PATH_CHARS
        || has_control(executable)
        || argv.len() > MAX_ARG_COUNT
        || argv
            .iter()
            .any(|arg| arg.chars().count() > MAX_ARG_CHARS || has_control(arg))
        || working_directory.is_some_and(|path| {
            path.is_empty() || path.chars().count() > MAX_PATH_CHARS || has_control(path)
        })
        || !(1..=600).contains(&timeout_seconds)
    {
        return Err(invalid("invalidCustomHookTarget"));
    }
    Ok(())
}

fn valid_identifier(value: &str, max_chars: usize) -> bool {
    value.trim() == value
        && (1..=max_chars).contains(&value.chars().count())
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':' | '@')
        })
}

fn has_control(value: &str) -> bool {
    value.chars().any(char::is_control)
}

fn invalid(reason: &str) -> CommandError {
    CommandError::with_detail(
        AppErrorCode::InvalidInput,
        "errors.invalidInput",
        "reasonCode",
        SafeParameterValue::String(reason.into()),
        false,
    )
}

fn unsupported(reason: &str) -> CommandError {
    CommandError::with_detail(
        AppErrorCode::IntegrationUnsupported,
        "errors.integrationUnsupported",
        "reasonCode",
        SafeParameterValue::String(reason.into()),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Storage;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct TestEmitter {
        failures: bool,
        calls: AtomicUsize,
    }

    impl EventEmitterPort for TestEmitter {
        fn emit(
            &self,
            _event_name: &'static str,
            _payload: serde_json::Value,
        ) -> Result<(), CommandError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.failures {
                Err(io_error("testEmitterFailure"))
            } else {
                Ok(())
            }
        }
    }

    fn repository() -> AgentProfileRepository {
        let directory = tempfile::tempdir().unwrap().keep();
        AgentProfileRepository::new(Arc::new(Storage::open(&directory).unwrap()))
    }

    fn service(
        repository: AgentProfileRepository,
        emitter: Arc<dyn EventEmitterPort>,
    ) -> AgentProfileService {
        let directory = tempfile::tempdir().unwrap().keep();
        AgentProfileService::new(
            repository,
            directory.join("home"),
            directory.join("data"),
            emitter,
        )
    }

    fn stored_custom_profile(executable: &Path) -> StoredAgentIntegrationProfile {
        StoredAgentIntegrationProfile {
            id: AgentIntegrationId::parse(uuid::Uuid::new_v4().to_string()).unwrap(),
            kind: AgentIntegrationKind::Custom,
            display_name: "Custom Adapter".into(),
            environment: AgentEnvironment::Windows,
            config_target: AgentConfigTarget::CustomHook {
                executable: executable.display().to_string(),
                argv: Vec::new(),
                working_directory: None,
                timeout_seconds: 10,
            },
            event_mapping: vec![AgentEventMapping {
                native_event: "done".into(),
                normalized_status: AgentStatus::Completed,
            }],
            enabled: false,
            revision: 0,
            created_at: 1,
            updated_at: 1,
        }
    }

    fn custom_target(executable: String) -> AgentConfigTarget {
        AgentConfigTarget::CustomHook {
            executable,
            argv: vec!["--stream".into()],
            working_directory: None,
            timeout_seconds: 10,
        }
    }

    fn save_custom_input(executable: &Path) -> SaveAgentIntegrationProfileInput {
        SaveAgentIntegrationProfileInput {
            id: None,
            kind: AgentIntegrationKind::Custom,
            display_name: "Custom Adapter".into(),
            environment: AgentEnvironment::Windows,
            config_target: AgentConfigTarget::CustomHook {
                executable: executable.display().to_string(),
                argv: Vec::new(),
                working_directory: None,
                timeout_seconds: 10,
            },
            event_mapping: vec![AgentEventMapping {
                native_event: "done".into(),
                normalized_status: AgentStatus::Completed,
            }],
            enabled: false,
            expected_revision: None,
        }
    }

    fn compiled_adapter_fixture() -> PathBuf {
        static FIXTURE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        FIXTURE
            .get_or_init(|| {
                let directory = tempfile::tempdir().unwrap().keep();
                let executable = directory.join("agent-profile-adapter.exe");
                let source = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests")
                    .join("fixtures")
                    .join("agent_profile_adapter.rs");
                let output = Command::new("rustc")
                    .arg("--edition=2021")
                    .arg(&source)
                    .arg("-o")
                    .arg(&executable)
                    .output()
                    .expect("rustc must compile the native adapter fixture");
                assert!(
                    output.status.success(),
                    "fixture compilation failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                executable
            })
            .clone()
    }

    fn stored_runtime_profile(mode: &str) -> StoredAgentIntegrationProfile {
        let executable = compiled_adapter_fixture();
        let mut profile = stored_custom_profile(&executable);
        profile.config_target = AgentConfigTarget::CustomHook {
            executable: executable.display().to_string(),
            argv: vec![mode.into()],
            working_directory: None,
            timeout_seconds: 1,
        };
        profile
    }

    fn wait_for_condition(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if condition() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("condition was not reached before the deadline");
    }

    #[test]
    fn trae_windows_capability_uses_the_retryable_detection_reason_contract() {
        let repository = repository();
        let service = service(repository, Arc::new(TestEmitter::default()));

        let trae = service
            .list_profiles()
            .unwrap()
            .into_iter()
            .find(|profile| profile.id == "trae-windows")
            .unwrap();

        assert_eq!(trae.installation_state, IntegrationState::Unsupported);
        assert_eq!(
            trae.reason_code.as_deref(),
            Some("traeHooksVersionOrConfigUnavailable")
        );
    }

    #[test]
    fn custom_windows_target_requires_a_real_non_shell_executable() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("adapter.exe");
        std::fs::write(&executable, b"fixture").unwrap();
        let target = validate_and_canonicalize_custom_target(
            &AgentEnvironment::Windows,
            &custom_target(executable.display().to_string()),
        )
        .unwrap();
        assert!(target.executable.is_absolute());

        for dangerous in ["cmd.exe", "powershell.exe", "pwsh.exe"] {
            let path = directory.path().join(dangerous);
            std::fs::write(&path, b"fixture").unwrap();
            assert!(validate_and_canonicalize_custom_target(
                &AgentEnvironment::Windows,
                &custom_target(path.display().to_string()),
            )
            .is_err());
        }
        for script in ["adapter.bat", "adapter.cmd", "adapter.ps1"] {
            let path = directory.path().join(script);
            std::fs::write(&path, b"fixture").unwrap();
            assert!(validate_and_canonicalize_custom_target(
                &AgentEnvironment::Windows,
                &custom_target(path.display().to_string()),
            )
            .is_err());
        }
    }

    #[test]
    fn profile_payload_rejects_controls_duplicates_and_unbounded_values() {
        let target = AgentConfigTarget::CustomHook {
            executable: "C:\\adapter.exe".into(),
            argv: vec![],
            working_directory: None,
            timeout_seconds: 10,
        };
        assert!(validate_profile_text_and_mapping(
            "Valid",
            &[
                AgentEventMapping {
                    native_event: "done".into(),
                    normalized_status: AgentStatus::Completed,
                },
                AgentEventMapping {
                    native_event: "done".into(),
                    normalized_status: AgentStatus::Failed,
                },
            ],
            &target,
        )
        .is_err());
        assert!(validate_profile_text_and_mapping(
            "Valid",
            &[
                AgentEventMapping {
                    native_event: "Done".into(),
                    normalized_status: AgentStatus::Completed,
                },
                AgentEventMapping {
                    native_event: "done".into(),
                    normalized_status: AgentStatus::Failed,
                },
            ],
            &target,
        )
        .is_err());
        assert!(validate_profile_text_and_mapping("bad\0name", &[], &target).is_err());
    }

    #[test]
    fn bounded_custom_event_maps_native_status_without_persisting_unknown_payload() {
        let profile_id = AgentIntegrationId::parse("custom-1").unwrap();
        let event = parse_custom_event_line(
            &profile_id,
            &[AgentEventMapping {
                native_event: "done".into(),
                normalized_status: AgentStatus::Completed,
            }],
            br#"{"nativeEvent":"DONE","taskId":"task-1","sourceEventId":"event-1","occurredAt":100}"#,
            101,
        )
        .unwrap();
        assert_eq!(event.status, AgentStatus::Completed);
        assert_eq!(event.profile_id, profile_id);
        assert_eq!(event.native_event, "DONE");

        let unknown = br#"{"nativeEvent":"done","taskId":"task-1","sourceEventId":"event-2","occurredAt":100,"prompt":"secret"}"#;
        assert!(parse_custom_event_line(&event.profile_id, &[], unknown, 101).is_err());
        let disguised_prompt = br#"{"nativeEvent":"done","taskId":"task-1","sourceEventId":"event-2","occurredAt":100,"summary":"secret"}"#;
        assert!(parse_custom_event_line(&event.profile_id, &[], disguised_prompt, 101).is_err());
        let path_like_id = br#"{"nativeEvent":"done","taskId":"C:\\\\prompt.txt","sourceEventId":"event-2","occurredAt":100}"#;
        assert!(parse_custom_event_line(&event.profile_id, &[], path_like_id, 101).is_err());
        assert!(parse_custom_event_line(
            &event.profile_id,
            &[],
            br#"{"nativeEvent":"done","taskId":"task-1","sourceEventId":"event-2","occurredAt":100}"#,
            -1,
        )
        .is_err());
    }

    #[test]
    fn bounded_line_reader_rejects_overlong_data_without_a_newline() {
        let bytes = vec![b'x'; MAX_EVENT_LINE_BYTES + 1];
        let mut reader = BufReader::with_capacity(128, std::io::Cursor::new(bytes));
        assert_eq!(read_bounded_line(&mut reader), Err("eventLineTooLarge"));

        let mut exact = vec![b'x'; MAX_EVENT_LINE_BYTES];
        exact.push(b'\n');
        let mut reader = BufReader::with_capacity(128, std::io::Cursor::new(exact));
        assert_eq!(
            read_bounded_line(&mut reader).unwrap().unwrap().len(),
            MAX_EVENT_LINE_BYTES
        );
    }

    #[test]
    fn snapshot_forces_inactive_profiles_offline_and_expires_terminal_state() {
        let repository = repository();
        let profile_id = AgentIntegrationId::parse("kimi-windows").unwrap();
        let event = ValidatedAgentProfileEvent {
            event_id: "event-1".into(),
            profile_id: profile_id.clone(),
            native_event: "Notification".into(),
            task_id: "task-1".into(),
            status: AgentStatus::Completed,
            occurred_at: 100,
        };
        repository.project_event(&event, 100).unwrap();
        let service = service(repository.clone(), Arc::new(TestEmitter::default()));
        let inactive = service.snapshot(101).unwrap();
        assert_eq!(
            inactive
                .profiles
                .iter()
                .find(|summary| summary.profile.id == "kimi-windows")
                .unwrap()
                .aggregate_status,
            AgentStatus::Offline
        );
        assert!(inactive
            .profiles
            .iter()
            .find(|summary| summary.profile.id == "kimi-windows")
            .unwrap()
            .observations
            .is_empty());

        let before = repository.get(&profile_id).unwrap();
        repository
            .set_installation(
                &AgentProfileInstallation {
                    profile_id: profile_id.clone(),
                    state: IntegrationState::Installed,
                    reason_code: None,
                    owned_resource: Some("test".into()),
                    owned_fingerprint: Some("owned".into()),
                    external_hash: Some("external".into()),
                    updated_at: 101,
                },
                before.revision,
                true,
            )
            .unwrap();
        let current_event = ValidatedAgentProfileEvent {
            event_id: "event-2".into(),
            occurred_at: 102,
            ..event
        };
        repository.project_event(&current_event, 102).unwrap();
        let current = service.snapshot(102).unwrap();
        assert_eq!(
            current
                .profiles
                .iter()
                .find(|summary| summary.profile.id == "kimi-windows")
                .unwrap()
                .aggregate_status,
            AgentStatus::Completed
        );
        assert_eq!(
            current
                .profiles
                .iter()
                .find(|summary| summary.profile.id == "kimi-windows")
                .unwrap()
                .observations
                .iter()
                .map(|observation| observation.source_event_id.as_str())
                .collect::<Vec<_>>(),
            vec!["event-2"]
        );
        let stale = service
            .snapshot(102 + TERMINAL_STATUS_VISIBLE_MILLIS + 1)
            .unwrap();
        assert_eq!(
            stale
                .profiles
                .iter()
                .find(|summary| summary.profile.id == "kimi-windows")
                .unwrap()
                .aggregate_status,
            AgentStatus::Offline
        );
        assert!(stale
            .profiles
            .iter()
            .find(|summary| summary.profile.id == "kimi-windows")
            .unwrap()
            .observations
            .is_empty());
    }

    #[test]
    fn running_preset_apps_are_detected_before_hooks_are_installed() {
        let service = service(repository(), Arc::new(TestEmitter::default()));

        let snapshot = service
            .snapshot_with_running_process_names(
                1_000,
                &["Kimi.exe", "TRAE SOLO CN.exe", "qoder.exe"],
            )
            .unwrap();

        for (profile_id, installation_state) in [
            ("kimi-windows", IntegrationState::NotInstalled),
            ("trae-windows", IntegrationState::Unsupported),
            ("qoderwork-windows", IntegrationState::NotInstalled),
        ] {
            let profile = snapshot
                .profiles
                .iter()
                .find(|summary| summary.profile.id == profile_id)
                .unwrap();
            assert_eq!(profile.aggregate_status, AgentStatus::Idle, "{profile_id}");
            assert_eq!(profile.profile.installation_state, installation_state);
        }
        assert_eq!(
            snapshot
                .profiles
                .iter()
                .find(|summary| summary.profile.id == "kimi-wsl")
                .unwrap()
                .aggregate_status,
            AgentStatus::Offline
        );
    }

    #[test]
    fn qwenworkcn_process_uses_native_assistant_activity_when_hook_is_missing() {
        let root = tempfile::tempdir().unwrap();
        let repository =
            AgentProfileRepository::new(Arc::new(Storage::open(&root.path().join("db")).unwrap()));
        let roaming = root.path().join("roaming");
        let database = roaming.join("QwenWorkCN/data/agents.db");
        std::fs::create_dir_all(database.parent().unwrap()).unwrap();
        let connection = rusqlite::Connection::open(database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE messages(
                    id TEXT NOT NULL, message_id TEXT NOT NULL, chat_id TEXT NOT NULL,
                    sub_chat_id TEXT, sequence INTEGER NOT NULL, role TEXT NOT NULL,
                    parts TEXT NOT NULL, search_status TEXT, created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages VALUES ('row-1', 'message-1', 'chat-1', NULL, 1,
                    'assistant', ?1, 'ready', 10, 10)",
                [serde_json::json!([{"type":"text","text":"千问办公原生回复"}]).to_string()],
            )
            .unwrap();
        let profile_id = AgentIntegrationId::parse("qoderwork-windows").unwrap();
        let profile = repository.get(&profile_id).unwrap();
        repository
            .set_installation(
                &AgentProfileInstallation {
                    profile_id: profile_id.clone(),
                    state: IntegrationState::Installed,
                    reason_code: None,
                    owned_resource: Some("test".into()),
                    owned_fingerprint: Some("test".into()),
                    external_hash: Some("test".into()),
                    updated_at: 9_000,
                },
                profile.revision,
                true,
            )
            .unwrap();
        repository
            .project_event(
                &ValidatedAgentProfileEvent {
                    event_id: "newer-hook-running".into(),
                    profile_id,
                    native_event: "UserPromptSubmit".into(),
                    task_id: "hook-task".into(),
                    status: AgentStatus::Running,
                    occurred_at: 10_050,
                },
                10_050,
            )
            .unwrap();
        let service = AgentProfileService::new(
            repository,
            root.path().join("home"),
            roaming.join("com.aiceland.app"),
            Arc::new(TestEmitter::default()),
        );

        let summary = service
            .snapshot_with_running_process_names(10_100, &["QwenWorkCN.exe"])
            .unwrap()
            .profiles
            .into_iter()
            .find(|summary| summary.profile.id == "qoderwork-windows")
            .unwrap();

        assert_eq!(summary.aggregate_status, AgentStatus::Completed);
        assert_eq!(summary.observations.len(), 1);
        assert_eq!(
            summary.observations[0].latest_reply_preview.as_deref(),
            Some("千问办公原生回复")
        );
    }

    #[test]
    fn profile_snapshot_shows_completion_for_two_seconds_then_restores_running() {
        let repository = repository();
        let profile_id = AgentIntegrationId::parse("kimi-windows").unwrap();
        let before = repository.get(&profile_id).unwrap();
        repository
            .set_installation(
                &AgentProfileInstallation {
                    profile_id: profile_id.clone(),
                    state: IntegrationState::Installed,
                    reason_code: None,
                    owned_resource: Some("test".into()),
                    owned_fingerprint: Some("owned".into()),
                    external_hash: Some("external".into()),
                    updated_at: 50,
                },
                before.revision,
                true,
            )
            .unwrap();
        repository
            .project_event(
                &ValidatedAgentProfileEvent {
                    event_id: "running-1".into(),
                    profile_id: profile_id.clone(),
                    native_event: "UserPromptSubmit".into(),
                    task_id: "task-running".into(),
                    status: AgentStatus::Running,
                    occurred_at: 100,
                },
                100,
            )
            .unwrap();
        repository
            .project_event(
                &ValidatedAgentProfileEvent {
                    event_id: "completed-1".into(),
                    profile_id: profile_id.clone(),
                    native_event: "Stop".into(),
                    task_id: "task-completed".into(),
                    status: AgentStatus::Completed,
                    occurred_at: 101,
                },
                101,
            )
            .unwrap();
        let service = service(repository.clone(), Arc::new(TestEmitter::default()));
        let profile_status = |now| {
            service
                .snapshot_with_running_process_names(now, &["kimi.exe"])
                .unwrap()
                .profiles
                .into_iter()
                .find(|summary| summary.profile.id == "kimi-windows")
                .unwrap()
                .aggregate_status
        };

        assert_eq!(profile_status(101), AgentStatus::Completed);
        assert_eq!(profile_status(2_101), AgentStatus::Running);

        repository
            .project_event(
                &ValidatedAgentProfileEvent {
                    event_id: "running-2".into(),
                    profile_id,
                    native_event: "Stop".into(),
                    task_id: "task-running".into(),
                    status: AgentStatus::Completed,
                    occurred_at: 2_102,
                },
                2_102,
            )
            .unwrap();
        assert_eq!(profile_status(4_102), AgentStatus::Completed);
    }

    #[test]
    fn aggregate_prioritizes_concurrent_active_work_then_becomes_idle() {
        let profile_id = AgentIntegrationId::parse("custom-1").unwrap();
        let observations = vec![
            crate::domain::agent_profiles::AgentProfileObservation {
                profile_id: profile_id.clone(),
                task_id: "task-running".into(),
                status: AgentStatus::Running,
                latest_reply_preview: None,
                source_event_id: "event-running".into(),
                occurred_at: 100,
                received_at: 100,
            },
            crate::domain::agent_profiles::AgentProfileObservation {
                profile_id,
                task_id: "task-completed".into(),
                status: AgentStatus::Completed,
                latest_reply_preview: None,
                source_event_id: "event-completed".into(),
                occurred_at: 101,
                received_at: 101,
            },
        ];
        assert_eq!(
            aggregate_profile_status(&observations, true, true, Some(0), 102),
            AgentStatus::Completed
        );
        assert_eq!(
            aggregate_profile_status(&observations, true, true, Some(0), 2_101),
            AgentStatus::Running
        );
        assert_eq!(
            aggregate_profile_status(
                &observations,
                true,
                true,
                Some(0),
                100 + ACTIVE_STATUS_STALE_MILLIS + 1,
            ),
            AgentStatus::Idle
        );
        assert_eq!(
            aggregate_profile_status(&observations, false, true, Some(0), 102),
            AgentStatus::Offline
        );
        assert_eq!(
            aggregate_profile_status(&observations, true, true, Some(102), 102),
            AgentStatus::Idle
        );
        assert!(!observation_is_current(&observations[0], Some(101), 101));
    }

    #[test]
    fn activation_wait_uses_the_supplied_timeout() {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let sender = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            reply_tx.send(Ok(())).unwrap();
        });
        assert!(await_activation(reply_rx, Duration::from_millis(100)).is_ok());
        sender.join().unwrap();

        let (_held_tx, held_rx) = mpsc::sync_channel(1);
        let started = Instant::now();
        let error = await_activation(held_rx, Duration::from_millis(20)).unwrap_err();
        assert_eq!(error.code, AppErrorCode::SourceUnavailable);
        assert!(started.elapsed() >= Duration::from_millis(20));
    }

    #[test]
    fn protocol_v1_ready_is_strict_and_is_not_a_business_event() {
        assert!(parse_custom_ready_line(br#"{"type":"ready","protocolVersion":1}"#).is_ok());
        for invalid_ready in [
            br#"{"type":"ready","protocolVersion":2}"#.as_slice(),
            br#"{"type":"event","protocolVersion":1}"#.as_slice(),
            br#"{"type":"ready","protocolVersion":1,"extra":true}"#.as_slice(),
            br#"{"nativeEvent":"done"}"#.as_slice(),
        ] {
            assert_eq!(
                parse_custom_ready_line(invalid_ready),
                Err("hookReadyInvalid")
            );
        }
    }

    #[test]
    fn native_custom_process_must_ack_ready_before_the_configured_timeout() {
        let repository = repository();
        let created = repository
            .save(&stored_runtime_profile("hold-pipes"), None)
            .unwrap();
        let service = service(repository.clone(), Arc::new(TestEmitter::default()));

        let started = Instant::now();
        let error = service
            .install_profile(created.id.as_str(), created.revision, now_millis())
            .unwrap_err();

        assert_eq!(error.code, AppErrorCode::SourceUnavailable);
        assert_eq!(
            error.details.get("reasonCode"),
            Some(&SafeParameterValue::String("hookReadyTimeout".into()))
        );
        assert!(started.elapsed() >= Duration::from_secs(1));
        assert!(started.elapsed() < Duration::from_secs(4));
        assert!(repository.get_installation(&created.id).unwrap().is_none());
        assert!(service
            .runtimes
            .lock()
            .expect("runtime map lock poisoned")
            .is_empty());
        service.shutdown();
    }

    #[test]
    fn readers_allow_long_lived_bounded_lines_and_drain_large_stderr() {
        let line = vec![b'x'; MAX_EVENT_LINE_BYTES - 1];
        let mut stream = Vec::new();
        for _ in 0..80 {
            stream.extend_from_slice(&line);
            stream.push(b'\n');
        }
        assert!(stream.len() > 1024 * 1024);
        let mut reader = BufReader::with_capacity(1024, std::io::Cursor::new(stream));
        let mut count = 0;
        while read_bounded_line(&mut reader).unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 80);

        let bytes = vec![b'e'; 2 * 1024 * 1024];
        let mut stderr = std::io::Cursor::new(bytes);
        discard_stream(&mut stderr);
        assert_eq!(stderr.position(), 2 * 1024 * 1024);
    }

    #[test]
    fn native_custom_process_projects_an_event_and_idle_is_not_a_failure_timeout() {
        let repository = repository();
        let created = repository
            .save(&stored_runtime_profile("event-idle"), None)
            .unwrap();
        let emitter = Arc::new(TestEmitter::default());
        let service = service(repository.clone(), emitter.clone());
        service
            .install_profile(created.id.as_str(), created.revision, now_millis())
            .unwrap();
        wait_for_condition(|| {
            repository
                .list_observations(&created.id)
                .is_ok_and(|observations| observations.len() == 1)
        });
        let observations = repository.list_observations(&created.id).unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].source_event_id, "event-1");
        assert_eq!(emitter.calls.load(Ordering::Relaxed), 1);
        thread::sleep(Duration::from_millis(1_200));
        assert_eq!(
            repository
                .get_installation(&created.id)
                .unwrap()
                .unwrap()
                .state,
            IntegrationState::Installed
        );
        service.shutdown();
    }

    #[test]
    fn native_custom_process_has_bounded_stdout_and_drains_large_stderr() {
        for (mode, expected_observations) in [("large-stdout", 80), ("large-stderr", 1)] {
            let repository = repository();
            let created = repository
                .save(&stored_runtime_profile(mode), None)
                .unwrap();
            let service = service(repository.clone(), Arc::new(TestEmitter::default()));
            service
                .install_profile(created.id.as_str(), created.revision, now_millis())
                .unwrap();
            wait_for_condition(|| {
                repository
                    .list_observations(&created.id)
                    .is_ok_and(|observations| observations.len() == expected_observations)
            });
            assert_eq!(
                repository
                    .get_installation(&created.id)
                    .unwrap()
                    .unwrap()
                    .state,
                IntegrationState::Installed
            );
            service.shutdown();
        }

        let repository = repository();
        let created = repository
            .save(&stored_runtime_profile("overlong"), None)
            .unwrap();
        let service = service(repository.clone(), Arc::new(TestEmitter::default()));
        service
            .install_profile(created.id.as_str(), created.revision, now_millis())
            .unwrap();
        wait_for_condition(|| {
            repository
                .get_installation(&created.id)
                .ok()
                .flatten()
                .is_some_and(|installation| installation.state == IntegrationState::NeedsRepair)
        });
        assert_eq!(
            repository
                .get_installation(&created.id)
                .unwrap()
                .unwrap()
                .reason_code
                .as_deref(),
            Some("eventLineTooLarge")
        );
        service.shutdown();
    }

    #[test]
    fn native_custom_process_exit_emits_health_and_job_shutdown_kills_descendants() {
        {
            let repository = repository();
            let created = repository
                .save(&stored_runtime_profile("exit"), None)
                .unwrap();
            let emitter = Arc::new(TestEmitter::default());
            let service = service(repository.clone(), emitter.clone());
            service
                .install_profile(created.id.as_str(), created.revision, now_millis())
                .unwrap();
            wait_for_condition(|| {
                repository
                    .get_installation(&created.id)
                    .ok()
                    .flatten()
                    .is_some_and(|installation| installation.state == IntegrationState::NeedsRepair)
            });
            assert!(emitter.calls.load(Ordering::Relaxed) >= 1);
            service.shutdown();
        }

        let repository = repository();
        let created = repository
            .save(&stored_runtime_profile("spawn-descendant"), None)
            .unwrap();
        let service = service(repository, Arc::new(TestEmitter::default()));
        service
            .install_profile(created.id.as_str(), created.revision, now_millis())
            .unwrap();
        let started = Instant::now();
        service.shutdown();
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "process tree shutdown took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn repeated_install_is_idempotent_without_revision_bump_or_runtime_spawn() {
        let repository = repository();
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("adapter.exe");
        std::fs::write(&executable, b"fixture").unwrap();
        let created = repository
            .save(&stored_custom_profile(&executable), None)
            .unwrap();
        let installed = repository
            .set_installation(
                &AgentProfileInstallation {
                    profile_id: created.id.clone(),
                    state: IntegrationState::Installed,
                    reason_code: None,
                    owned_resource: Some("custom-process".into()),
                    owned_fingerprint: Some("owned".into()),
                    external_hash: Some("external".into()),
                    updated_at: 2,
                },
                created.revision,
                true,
            )
            .unwrap();
        let service = service(repository.clone(), Arc::new(TestEmitter::default()));

        let result = service
            .install_profile(installed.id.as_str(), installed.revision, 3)
            .unwrap();
        assert_eq!(result.revision, installed.revision);
        assert_eq!(
            repository.get(&installed.id).unwrap().revision,
            installed.revision
        );
        assert!(service
            .runtimes
            .lock()
            .expect("runtime map lock poisoned")
            .is_empty());
    }

    #[test]
    fn shutdown_linearizes_admitted_mutation_then_rejects_all_later_mutations() {
        let repository = repository();
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("adapter.exe");
        std::fs::write(&executable, b"fixture").unwrap();
        let service = Arc::new(service(
            repository.clone(),
            Arc::new(TestEmitter::default()),
        ));
        let admitted = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        service.set_admission_hook(Arc::new({
            let admitted = admitted.clone();
            let release = release.clone();
            move || {
                admitted.wait();
                release.wait();
            }
        }));

        let mutation = thread::spawn({
            let service = service.clone();
            let input = save_custom_input(&executable);
            move || service.save_profile(input, 1)
        });
        admitted.wait();
        let (shutdown_done_tx, shutdown_done_rx) = mpsc::channel();
        let shutdown = thread::spawn({
            let service = service.clone();
            move || {
                service.shutdown();
                shutdown_done_tx.send(()).unwrap();
            }
        });
        for _ in 0..100 {
            if !service.accepting.load(AtomicOrdering::Acquire) {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(!service.accepting.load(AtomicOrdering::Acquire));
        assert!(shutdown_done_rx
            .recv_timeout(Duration::from_millis(25))
            .is_err());

        release.wait();
        mutation.join().unwrap().unwrap();
        shutdown_done_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        shutdown.join().unwrap();
        assert_eq!(repository.count_custom_profiles().unwrap(), 1);

        let rejected = service
            .save_profile(save_custom_input(&executable), 2)
            .unwrap_err();
        assert_eq!(rejected.code, AppErrorCode::SourceUnavailable);
        assert_eq!(rejected.message_key, "errors.serviceStopping");
        assert_eq!(repository.count_custom_profiles().unwrap(), 1);
        service.shutdown();
    }

    #[test]
    fn custom_profile_and_installed_runtime_counts_are_bounded() {
        let repository = repository();
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("adapter.exe");
        std::fs::write(&executable, b"fixture").unwrap();
        let mut profiles = Vec::new();
        for _ in 0..MAX_CUSTOM_PROFILES {
            profiles.push(
                repository
                    .save(&stored_custom_profile(&executable), None)
                    .unwrap(),
            );
        }
        let service = service(repository.clone(), Arc::new(TestEmitter::default()));
        let profile_limit = service
            .save_profile(save_custom_input(&executable), 2)
            .unwrap_err();
        assert_eq!(
            profile_limit.details.get("reasonCode"),
            Some(&SafeParameterValue::String(
                "agentProfileLimitReached".into()
            ))
        );

        for (index, profile) in profiles.iter().enumerate() {
            repository
                .set_installation(
                    &AgentProfileInstallation {
                        profile_id: profile.id.clone(),
                        state: IntegrationState::Installed,
                        reason_code: None,
                        owned_resource: Some("fixture".into()),
                        owned_fingerprint: Some("fixture".into()),
                        external_hash: Some("fixture".into()),
                        updated_at: index as i64 + 10,
                    },
                    profile.revision,
                    true,
                )
                .unwrap();
        }
        let candidate = repository
            .save(&stored_custom_profile(&executable), None)
            .unwrap();
        let install_limit = service
            .install_profile(candidate.id.as_str(), candidate.revision, 100)
            .unwrap_err();
        assert_eq!(
            install_limit.details.get("reasonCode"),
            Some(&SafeParameterValue::String(
                "agentProfileInstallLimitReached".into()
            ))
        );
        assert!(repository
            .get_installation(&candidate.id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn restore_marks_receipt_or_executable_drift_for_repair_and_continues() {
        let repository = repository();
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("adapter.exe");
        std::fs::write(&executable, b"fixture").unwrap();
        let created = repository
            .save(&stored_custom_profile(&executable), None)
            .unwrap();
        repository
            .set_installation(
                &AgentProfileInstallation {
                    profile_id: created.id.clone(),
                    state: IntegrationState::Installed,
                    reason_code: None,
                    owned_resource: Some("custom-process".into()),
                    owned_fingerprint: Some("wrong".into()),
                    external_hash: Some("wrong".into()),
                    updated_at: 2,
                },
                created.revision,
                true,
            )
            .unwrap();
        let emitter = Arc::new(TestEmitter::default());
        let service = service(repository.clone(), emitter.clone());
        assert_eq!(service.restore_installed_custom_profiles().unwrap(), 0);
        let installation = repository.get_installation(&created.id).unwrap().unwrap();
        assert_eq!(installation.state, IntegrationState::NeedsRepair);
        assert_eq!(
            installation.reason_code.as_deref(),
            Some("customHookReceiptMismatch")
        );
        assert_eq!(emitter.calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn emitter_failure_does_not_reject_projected_events_or_damage_installation() {
        let repository = repository();
        let profile_id = AgentIntegrationId::parse("kimi-windows").unwrap();
        let before = repository.get(&profile_id).unwrap();
        repository
            .set_installation(
                &AgentProfileInstallation {
                    profile_id: profile_id.clone(),
                    state: IntegrationState::Installed,
                    reason_code: None,
                    owned_resource: Some("test".into()),
                    owned_fingerprint: Some("owned".into()),
                    external_hash: Some("external".into()),
                    updated_at: 1,
                },
                before.revision,
                true,
            )
            .unwrap();
        let emitter = TestEmitter {
            failures: true,
            ..TestEmitter::default()
        };
        for sequence in 1..=2 {
            let event = ValidatedAgentProfileEvent {
                event_id: format!("event-{sequence}"),
                profile_id: profile_id.clone(),
                native_event: "Notification".into(),
                task_id: format!("task-{sequence}"),
                status: AgentStatus::Completed,
                occurred_at: sequence,
            };
            project_and_emit_profile_event(&repository, &emitter, &event, sequence).unwrap();
        }
        assert_eq!(repository.list_observations(&profile_id).unwrap().len(), 2);
        assert_eq!(
            repository
                .get_installation(&profile_id)
                .unwrap()
                .unwrap()
                .state,
            IntegrationState::Installed
        );
        assert_eq!(emitter.calls.load(Ordering::Relaxed), 2);
    }
}
