use crate::contracts::{
    AgentConfigTarget, AgentEnvironment, AgentIntegrationKind, AgentStatus, AppErrorCode,
    CommandError, IntegrationState, PresetAgentAdapterId, SafeParameterValue,
};
use crate::domain::agent_profiles::{
    AgentIntegrationId, AgentProfileInstallation, StoredAgentIntegrationProfile,
    ValidatedAgentProfileEvent,
};
use crate::events::{agent_profile_state_changed_payload, AGENT_PROFILE_STATE_CHANGED};
use crate::repositories::agent_profiles::{AgentProfileProjectionOutcome, AgentProfileRepository};
use crate::services::config_merge::{
    inspect_config, merge_config, same_aisland_managed_script, ConfigFormat, MergeAction,
    OwnedHookFragment,
};
use crate::services::EventEmitterPort;
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;
use toml_edit::{value, ArrayOfTables, DocumentMut, Item, Table};

#[cfg(all(test, windows))]
thread_local! {
    static SIMULATED_REPLACE_ERROR: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

const PROFILE_EVENT_SCRIPT: &[u8] =
    include_bytes!("../../agent-hooks/aisland-profile-event-windows.ps1");
const PROFILE_EVENT_SCRIPT_NAME: &str = "aisland-profile-event-windows.ps1";
const MAX_SPOOL_EVENT_BYTES: u64 = 16 * 1024;
const MAX_PRESET_CONFIG_BYTES: u64 = 4 * 1024 * 1024;
const MAX_STARTUP_SPOOL_FILES: usize = 4096;
const MAX_DURABLE_EVENT_AGE_MILLIS: i64 = 30 * 24 * 60 * 60 * 1000;
const MAX_BACKUPS_PER_CONFIG: usize = 3;
const MAX_ROLLBACK_RETRIES: usize = 3;
const ROLLBACK_JOURNAL_VERSION: u32 = 1;
const MAX_ROLLBACK_JOURNAL_BYTES: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RollbackPhase {
    BeforeLock,
    AfterLockBeforeCommit,
    AfterJournalPrepared,
    AfterCandidateSyncBeforeIdentityJournal,
    AfterCandidatePreparedBeforeReplace,
    AfterPreflightBeforeReplace,
    AfterReplaceBeforeRecovery,
    AfterRolloverJournalBeforeNormalize,
    BeforeMissingTargetCommit,
    AfterMissingPreflightBeforeMove,
    AfterMissingMoveBeforePostValidation,
    AfterCleanupCandidate,
    AfterCleanupDisplaced,
    AfterCleanupRescue,
    AfterCleanupRolloverRescue,
    AfterDeleteIntentPrepared,
    AfterDelete,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RollbackJournal {
    version: u32,
    profile_id: String,
    adapter_id: PresetAgentAdapterId,
    action: RollbackAction,
    originally_existed: bool,
    delete_intent: bool,
    expected_hash: String,
    expected_target_identity: Option<WindowsFileIdentity>,
    desired_hash: Option<String>,
    candidate_identity: Option<WindowsFileIdentity>,
    candidate_name: String,
    displaced_name: String,
    rescue_name: String,
    rollover_rescue_name: String,
    preserved_hash: Option<String>,
    preserved_identity: Option<WindowsFileIdentity>,
    rollover_preserved_hash: Option<String>,
    rollover_preserved_identity: Option<WindowsFileIdentity>,
    delete_target_identity: Option<WindowsFileIdentity>,
    cleanup_phase: Option<RollbackCleanupPhase>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
enum RollbackAction {
    Install,
    Uninstall,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
enum RollbackCleanupPhase {
    Candidate,
    Displaced,
    Rescue,
    RolloverRescue,
    Journal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WindowsFileIdentity {
    volume_serial_number: u32,
    file_index: u64,
}

impl RollbackAction {
    fn merge_action(self) -> MergeAction {
        match self {
            Self::Install => MergeAction::Install,
            Self::Uninstall => MergeAction::Uninstall,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresetEventSpec {
    pub native_event: &'static str,
    pub status: AgentStatus,
}

const KIMI_EVENTS: [PresetEventSpec; 7] = [
    PresetEventSpec {
        native_event: "UserPromptSubmit",
        status: AgentStatus::Running,
    },
    PresetEventSpec {
        native_event: "PermissionRequest",
        status: AgentStatus::Waiting,
    },
    PresetEventSpec {
        native_event: "PermissionResult",
        status: AgentStatus::Running,
    },
    PresetEventSpec {
        native_event: "Stop",
        status: AgentStatus::Completed,
    },
    PresetEventSpec {
        native_event: "StopFailure",
        status: AgentStatus::Failed,
    },
    PresetEventSpec {
        native_event: "Interrupt",
        status: AgentStatus::Failed,
    },
    PresetEventSpec {
        native_event: "SessionEnd",
        status: AgentStatus::Offline,
    },
];
const QODERWORK_EVENTS: [PresetEventSpec; 3] = [
    PresetEventSpec {
        native_event: "UserPromptSubmit",
        status: AgentStatus::Running,
    },
    PresetEventSpec {
        native_event: "Stop",
        status: AgentStatus::Completed,
    },
    PresetEventSpec {
        native_event: "SessionEnd",
        status: AgentStatus::Offline,
    },
];
const CURSOR_EVENTS: [PresetEventSpec; 2] = [
    PresetEventSpec {
        native_event: "beforeSubmitPrompt",
        status: AgentStatus::Running,
    },
    PresetEventSpec {
        native_event: "afterAgentResponse",
        status: AgentStatus::Completed,
    },
];

pub fn preset_event_specs(adapter: &PresetAgentAdapterId) -> Option<&'static [PresetEventSpec]> {
    match adapter {
        PresetAgentAdapterId::Kimi => Some(&KIMI_EVENTS),
        PresetAgentAdapterId::Qoderwork => Some(&QODERWORK_EVENTS),
        PresetAgentAdapterId::Cursor => Some(&CURSOR_EVENTS),
        PresetAgentAdapterId::Trae => None,
    }
}

pub struct PresetProfileBridge {
    repository: AgentProfileRepository,
    emitter: Arc<dyn EventEmitterPort>,
    windows_home: PathBuf,
    roaming_app_data: PathBuf,
    spool_dir: PathBuf,
    script_path: PathBuf,
    watcher: Mutex<Option<SpoolWatcherRuntime>>,
    installing: Mutex<HashSet<String>>,
}

pub struct PresetInstallOutcome {
    pub installation: AgentProfileInstallation,
    pub mutation: ConfigMutation,
}

#[derive(Debug)]
pub struct ConfigMutation {
    profile_id: AgentIntegrationId,
    path: PathBuf,
    adapter_id: PresetAgentAdapterId,
    owned_hooks: Vec<OwnedHookFragment>,
    rollback_action: MergeAction,
    originally_existed: bool,
    changed: bool,
}

impl ConfigMutation {
    pub fn rollback(self) -> Result<(), CommandError> {
        self.rollback_with_observer(|_, _| {})
    }

    fn rollback_with_observer(
        self,
        mut observer: impl FnMut(usize, RollbackPhase),
    ) -> Result<(), CommandError> {
        if !self.changed {
            return Ok(());
        }
        let descriptor = PresetDescriptor {
            profile_id: self.profile_id,
            adapter_id: self.adapter_id,
            config_path: self.path,
            owned_hooks: self.owned_hooks,
        };
        recover_rollback_journal(&descriptor)?;
        prepare_and_execute_rollback(
            &descriptor,
            self.rollback_action,
            self.originally_existed,
            &mut observer,
        )
    }
}

#[cfg(windows)]
fn open_exclusive_rollback_file(path: &Path) -> std::io::Result<Option<File>> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{DELETE, FILE_SHARE_READ};

    match OpenOptions::new()
        .access_mode(GENERIC_READ.0 | GENERIC_WRITE.0 | DELETE.0)
        .share_mode(FILE_SHARE_READ.0)
        .open(path)
    {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(not(windows))]
fn open_exclusive_rollback_file(path: &Path) -> std::io::Result<Option<File>> {
    match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_bounded_open_file(file: &mut File, limit: u64) -> Result<Vec<u8>, CommandError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| io_error("profileFileRead"))?;
    let mut bytes = Vec::new();
    (&mut *file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| io_error("profileFileRead"))?;
    if bytes.len() as u64 > limit {
        return Err(config_error("profileConfigSizeExceeded"));
    }
    Ok(bytes)
}

struct GuardedFileSnapshot {
    file: File,
    bytes: Vec<u8>,
    hash: String,
    identity: Option<WindowsFileIdentity>,
}

#[derive(Clone)]
struct FileGeneration {
    bytes: Vec<u8>,
    hash: String,
    identity: Option<WindowsFileIdentity>,
}

impl GuardedFileSnapshot {
    fn generation(&self) -> FileGeneration {
        FileGeneration {
            bytes: self.bytes.clone(),
            hash: self.hash.clone(),
            identity: self.identity.clone(),
        }
    }
}

enum GuardedFileState {
    Missing,
    Busy,
    Present(GuardedFileSnapshot),
}

fn guarded_file_state(path: &Path, limit: u64) -> Result<GuardedFileState, CommandError> {
    let mut file = match open_exclusive_rollback_file(path) {
        Ok(Some(file)) => file,
        Ok(None) => return Ok(GuardedFileState::Missing),
        Err(_) => return Ok(GuardedFileState::Busy),
    };
    let bytes = read_bounded_open_file(&mut file, limit)?;
    let hash = sha256_hex(&bytes);
    let identity = file_identity(&file)?;
    Ok(GuardedFileState::Present(GuardedFileSnapshot {
        file,
        bytes,
        hash,
        identity,
    }))
}

#[cfg(windows)]
fn file_identity(file: &File) -> Result<Option<WindowsFileIdentity>, CommandError> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information)
            .map_err(|_| io_error("presetFileIdentity"))?;
    }
    Ok(Some(WindowsFileIdentity {
        volume_serial_number: information.dwVolumeSerialNumber,
        file_index: (u64::from(information.nFileIndexHigh) << 32)
            | u64::from(information.nFileIndexLow),
    }))
}

#[cfg(not(windows))]
fn file_identity(_file: &File) -> Result<Option<WindowsFileIdentity>, CommandError> {
    Ok(None)
}

#[cfg(windows)]
fn delete_locked_file(file: &File) -> Result<(), CommandError> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
    };

    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    unsafe {
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileDispositionInfo,
            std::ptr::from_ref(&disposition).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
        .map_err(|_| io_error("presetRollbackRemove"))
    }
}

#[cfg(not(windows))]
fn delete_locked_file(_file: &File) -> Result<(), CommandError> {
    Err(conflict_error("presetRollbackUnsupported"))
}

#[derive(Clone)]
struct RollbackPaths {
    journal: PathBuf,
    candidate: PathBuf,
    displaced: PathBuf,
    rescue: PathBuf,
    rollover_rescue: PathBuf,
}

fn rollback_paths(target: &Path) -> Result<RollbackPaths, CommandError> {
    let parent = target
        .parent()
        .ok_or_else(|| io_error("presetConfigParent"))?;
    let name = target
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| io_error("presetConfigName"))?;
    Ok(RollbackPaths {
        journal: parent.join(format!(".{name}.aisland-rollback.json")),
        candidate: parent.join(format!(".{name}.aisland-rollback-candidate")),
        displaced: parent.join(format!(".{name}.aisland-rollback-displaced")),
        rescue: parent.join(format!(".{name}.aisland-rollback-rescue")),
        rollover_rescue: parent.join(format!(".{name}.aisland-rollback-rescue-next")),
    })
}

fn prepare_and_execute_rollback(
    descriptor: &PresetDescriptor,
    action: MergeAction,
    originally_existed: bool,
    observer: &mut impl FnMut(usize, RollbackPhase),
) -> Result<(), CommandError> {
    observer(0, RollbackPhase::BeforeLock);
    let mut file = match open_exclusive_rollback_file(&descriptor.config_path) {
        Ok(Some(file)) => file,
        Ok(None) if matches!(action, MergeAction::Uninstall) => return Ok(()),
        Ok(None) => return Err(conflict_error("presetRollbackConflict")),
        Err(_) => return Err(conflict_error("presetRollbackConflict")),
    };
    let current = read_bounded_open_file(&mut file, MAX_PRESET_CONFIG_BYTES)?;
    let (desired, changed) = merge_preset_document(
        &descriptor.adapter_id,
        &descriptor.owned_hooks,
        &current,
        action.clone(),
    )?;
    let delete_intent = !originally_existed
        && matches!(action, MergeAction::Uninstall)
        && is_empty_preset_document(&descriptor.adapter_id, &descriptor.owned_hooks, &desired)?;
    let expected_target_identity = file_identity(&file)?;
    let delete_target_identity = delete_intent
        .then(|| expected_target_identity.clone())
        .flatten();
    observer(0, RollbackPhase::AfterLockBeforeCommit);
    drop(file);
    if !changed && !delete_intent {
        return Ok(());
    }

    let paths = rollback_paths(&descriptor.config_path)?;
    remove_rollback_artifacts_without_journal(&paths)?;
    let journal = RollbackJournal {
        version: ROLLBACK_JOURNAL_VERSION,
        profile_id: descriptor.profile_id.as_str().into(),
        adapter_id: descriptor.adapter_id.clone(),
        action: match action {
            MergeAction::Install => RollbackAction::Install,
            MergeAction::Uninstall => RollbackAction::Uninstall,
        },
        originally_existed,
        delete_intent,
        expected_hash: sha256_hex(&current),
        expected_target_identity,
        desired_hash: (!delete_intent).then(|| sha256_hex(&desired)),
        candidate_identity: None,
        candidate_name: file_name_string(&paths.candidate)?,
        displaced_name: file_name_string(&paths.displaced)?,
        rescue_name: file_name_string(&paths.rescue)?,
        rollover_rescue_name: file_name_string(&paths.rollover_rescue)?,
        preserved_hash: None,
        preserved_identity: None,
        rollover_preserved_hash: None,
        rollover_preserved_identity: None,
        delete_target_identity,
        cleanup_phase: None,
    };
    write_rollback_journal(&paths.journal, &journal)?;
    observer(0, RollbackPhase::AfterJournalPrepared);
    if delete_intent {
        observer(0, RollbackPhase::AfterDeleteIntentPrepared);
    }
    execute_rollback_journal(descriptor, journal, observer)
}

fn recover_rollback_journal(descriptor: &PresetDescriptor) -> Result<bool, CommandError> {
    let paths = rollback_paths(&descriptor.config_path)?;
    let Some(bytes) = read_bounded_optional_file(&paths.journal, MAX_ROLLBACK_JOURNAL_BYTES)?
    else {
        remove_rollback_artifacts_without_journal(&paths)?;
        return Ok(false);
    };
    let journal: RollbackJournal = serde_json::from_slice(&bytes)
        .map_err(|_| conflict_error("presetRollbackJournalInvalid"))?;
    validate_rollback_journal(descriptor, &paths, &journal)?;
    execute_rollback_journal(descriptor, journal, &mut |_, _| {})?;
    Ok(true)
}

fn validate_rollback_journal(
    descriptor: &PresetDescriptor,
    paths: &RollbackPaths,
    journal: &RollbackJournal,
) -> Result<(), CommandError> {
    if journal.version != ROLLBACK_JOURNAL_VERSION
        || journal.profile_id != descriptor.profile_id.as_str()
        || journal.adapter_id != descriptor.adapter_id
        || journal.candidate_name != file_name_string(&paths.candidate)?
        || journal.displaced_name != file_name_string(&paths.displaced)?
        || journal.rescue_name != file_name_string(&paths.rescue)?
        || journal.rollover_rescue_name != file_name_string(&paths.rollover_rescue)?
        || (journal.delete_intent != journal.desired_hash.is_none())
        || !is_sha256_hex(&journal.expected_hash)
        || (cfg!(windows) && journal.expected_target_identity.is_none())
        || journal
            .desired_hash
            .as_deref()
            .is_some_and(|hash| !is_sha256_hex(hash))
        || journal
            .preserved_hash
            .as_deref()
            .is_some_and(|hash| !is_sha256_hex(hash))
        || journal
            .rollover_preserved_hash
            .as_deref()
            .is_some_and(|hash| !is_sha256_hex(hash))
        || (journal.preserved_identity.is_some() && journal.preserved_hash.is_none())
        || (journal.rollover_preserved_identity.is_some()
            && journal.rollover_preserved_hash.is_none())
        || (cfg!(windows)
            && journal.preserved_hash.is_some() != journal.preserved_identity.is_some())
        || (cfg!(windows)
            && journal.rollover_preserved_hash.is_some()
                != journal.rollover_preserved_identity.is_some())
        || (journal.rollover_preserved_hash.is_some() && journal.preserved_hash.is_none())
        || (journal.delete_intent
            && (journal.originally_existed
                || journal.action != RollbackAction::Uninstall
                || journal.preserved_hash.is_some()
                || journal.preserved_identity.is_some()
                || journal.rollover_preserved_hash.is_some()
                || journal.rollover_preserved_identity.is_some()
                || (cfg!(windows) && journal.delete_target_identity.is_none())))
        || (!journal.delete_intent && journal.delete_target_identity.is_some())
        || (journal.delete_intent && journal.candidate_identity.is_some())
    {
        return Err(conflict_error("presetRollbackJournalInvalid"));
    }
    Ok(())
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn execute_rollback_journal(
    descriptor: &PresetDescriptor,
    mut journal: RollbackJournal,
    observer: &mut impl FnMut(usize, RollbackPhase),
) -> Result<(), CommandError> {
    let paths = rollback_paths(&descriptor.config_path)?;
    validate_rollback_journal(descriptor, &paths, &journal)?;
    if journal.cleanup_phase.is_some() {
        return cleanup_rollback_artifacts(descriptor, &paths, &mut journal, observer, 0);
    }
    if journal.delete_intent {
        return execute_delete_intent(descriptor, &paths, &mut journal, observer);
    }

    for attempt in 0..MAX_ROLLBACK_RETRIES {
        normalize_preserved_displaced(&paths, &mut journal)?;
        let target = read_bounded_optional_file(&descriptor.config_path, MAX_PRESET_CONFIG_BYTES)?;
        let target_hash = target.as_deref().map(sha256_hex);
        if journal.candidate_identity.is_none()
            && paths.candidate.exists()
            && target_hash.as_deref() == Some(journal.expected_hash.as_str())
        {
            ensure_candidate(
                descriptor,
                &paths,
                &mut journal,
                target.as_deref().expect("matching target has bytes"),
                observer,
                attempt,
            )?;
        }
        if target_hash.as_deref() == journal.desired_hash.as_deref() {
            if (paths.rescue.exists() && journal.preserved_hash.is_none())
                || (paths.rollover_rescue.exists() && journal.rollover_preserved_hash.is_none())
            {
                return Err(conflict_error("presetRollbackConflict"));
            }
            match guarded_file_state(&paths.displaced, MAX_PRESET_CONFIG_BYTES)? {
                GuardedFileState::Busy => continue,
                GuardedFileState::Missing => {
                    cleanup_rollback_artifacts(
                        descriptor,
                        &paths,
                        &mut journal,
                        observer,
                        attempt,
                    )?;
                    return Ok(());
                }
                GuardedFileState::Present(displaced) => {
                    if displaced.identity.as_ref() != journal.expected_target_identity.as_ref() {
                        return Err(conflict_error("presetRollbackConflict"));
                    }
                    if displaced.hash == journal.expected_hash {
                        drop(displaced);
                        cleanup_rollback_artifacts(
                            descriptor,
                            &paths,
                            &mut journal,
                            observer,
                            attempt,
                        )?;
                        return Ok(());
                    }
                    let displaced_generation = displaced.generation();
                    drop(displaced);
                    rebase_rollback_journal(
                        descriptor,
                        &paths,
                        &mut journal,
                        &displaced_generation.bytes,
                        target_hash.expect("desired target has a hash"),
                        Some(&displaced_generation),
                        observer,
                        attempt,
                    )?;
                    continue;
                }
            }
        }
        let Some(target) = target else {
            return recover_missing_target(descriptor, &paths, &mut journal, observer, attempt);
        };
        if target_hash.as_deref() != Some(journal.expected_hash.as_str()) {
            let displaced = match guarded_file_state(&paths.displaced, MAX_PRESET_CONFIG_BYTES)? {
                GuardedFileState::Busy => continue,
                GuardedFileState::Missing => None,
                GuardedFileState::Present(displaced) => {
                    if displaced.identity.as_ref() != journal.expected_target_identity.as_ref() {
                        return Err(conflict_error("presetRollbackConflict"));
                    }
                    let generation = displaced.generation();
                    drop(displaced);
                    Some(generation)
                }
            };
            rebase_rollback_journal(
                descriptor,
                &paths,
                &mut journal,
                &target,
                target_hash.expect("present target has a hash"),
                displaced.as_ref(),
                observer,
                attempt,
            )?;
            continue;
        }
        ensure_candidate(descriptor, &paths, &mut journal, &target, observer, attempt)?;
        observer(attempt, RollbackPhase::AfterCandidatePreparedBeforeReplace);
        let replace_outcome = replace_verified_with_backup(
            &paths.candidate,
            &descriptor.config_path,
            &paths.displaced,
            &journal.expected_hash,
            journal.expected_target_identity.as_ref(),
            journal
                .desired_hash
                .as_deref()
                .ok_or_else(|| conflict_error("presetRollbackJournalInvalid"))?,
            journal.candidate_identity.as_ref(),
            || observer(attempt, RollbackPhase::AfterPreflightBeforeReplace),
        )?;
        match replace_outcome {
            VerifiedReplaceOutcome::Invoked => {}
            VerifiedReplaceOutcome::TargetChanged(latest) => {
                let latest_hash = sha256_hex(&latest);
                rebase_rollback_journal(
                    descriptor,
                    &paths,
                    &mut journal,
                    &latest,
                    latest_hash,
                    None,
                    observer,
                    attempt,
                )?;
                continue;
            }
            VerifiedReplaceOutcome::CandidateChanged | VerifiedReplaceOutcome::Busy => continue,
            VerifiedReplaceOutcome::TargetMissing => {
                return recover_missing_target(descriptor, &paths, &mut journal, observer, attempt);
            }
        }
        sync_existing_path(&descriptor.config_path)?;
        sync_existing_path(&paths.candidate)?;
        sync_existing_path(&paths.displaced)?;
        observer(attempt, RollbackPhase::AfterReplaceBeforeRecovery);

        let target_after =
            read_bounded_optional_file(&descriptor.config_path, MAX_PRESET_CONFIG_BYTES)?;
        let candidate_after =
            read_bounded_optional_file(&paths.candidate, MAX_PRESET_CONFIG_BYTES)?;
        let displaced_after = match guarded_file_state(&paths.displaced, MAX_PRESET_CONFIG_BYTES)? {
            GuardedFileState::Busy => continue,
            GuardedFileState::Missing => None,
            GuardedFileState::Present(displaced) => {
                if displaced.identity.as_ref() != journal.expected_target_identity.as_ref() {
                    return Err(conflict_error("presetRollbackConflict"));
                }
                let generation = displaced.generation();
                drop(displaced);
                Some(generation)
            }
        };
        let desired_hash = journal
            .desired_hash
            .as_deref()
            .ok_or_else(|| conflict_error("presetRollbackJournalInvalid"))?;
        if target_after.as_deref().map(sha256_hex).as_deref() == Some(desired_hash) {
            if displaced_after.as_ref().map(|state| state.hash.as_str())
                == Some(journal.expected_hash.as_str())
            {
                cleanup_rollback_artifacts(descriptor, &paths, &mut journal, observer, attempt)?;
                return Ok(());
            }
            if let Some(displaced) = displaced_after {
                let current_target_hash = sha256_hex(target_after.as_deref().unwrap());
                rebase_rollback_journal(
                    descriptor,
                    &paths,
                    &mut journal,
                    &displaced.bytes,
                    current_target_hash,
                    Some(&displaced),
                    observer,
                    attempt,
                )?;
                continue;
            }
            return Err(conflict_error("presetRollbackConflict"));
        }
        if target_after.is_none()
            && candidate_after.as_deref().map(sha256_hex).as_deref() == Some(desired_hash)
            && displaced_after.as_ref().map(|state| state.hash.as_str())
                == Some(journal.expected_hash.as_str())
        {
            return recover_missing_target(descriptor, &paths, &mut journal, observer, attempt);
        }
        if target_after.as_deref().map(sha256_hex).as_deref()
            == Some(journal.expected_hash.as_str())
            && candidate_after.as_deref().map(sha256_hex).as_deref() == Some(desired_hash)
            && displaced_after.is_none()
        {
            continue;
        }
        if candidate_after.is_none() {
            if let (Some(target), Some(displaced)) =
                (target_after.as_deref(), displaced_after.as_ref())
            {
                let target_hash = sha256_hex(target);
                rebase_rollback_journal(
                    descriptor,
                    &paths,
                    &mut journal,
                    target,
                    target_hash,
                    Some(displaced),
                    observer,
                    attempt,
                )?;
                continue;
            }
        }
        return Err(conflict_error("presetRollbackConflict"));
    }
    Err(conflict_error("presetRollbackConflict"))
}

fn execute_delete_intent(
    descriptor: &PresetDescriptor,
    paths: &RollbackPaths,
    journal: &mut RollbackJournal,
    observer: &mut impl FnMut(usize, RollbackPhase),
) -> Result<(), CommandError> {
    for attempt in 0..MAX_ROLLBACK_RETRIES {
        observer(attempt, RollbackPhase::BeforeLock);
        let mut file = match open_exclusive_rollback_file(&descriptor.config_path) {
            Ok(Some(file)) => file,
            Ok(None) => {
                cleanup_rollback_artifacts(descriptor, paths, journal, observer, attempt)?;
                return Ok(());
            }
            Err(_) => continue,
        };
        let current = read_bounded_open_file(&mut file, MAX_PRESET_CONFIG_BYTES)?;
        let current_hash = sha256_hex(&current);
        let current_identity = file_identity(&file)?;
        if current_hash != journal.expected_hash
            || current_identity.as_ref() != journal.delete_target_identity.as_ref()
        {
            drop(file);
            let (desired, _) = merge_preset_document(
                &descriptor.adapter_id,
                &descriptor.owned_hooks,
                &current,
                journal.action.merge_action(),
            )?;
            if desired == current {
                journal.delete_intent = false;
                journal.delete_target_identity = None;
                journal.expected_hash = current_hash.clone();
                journal.expected_target_identity = current_identity;
                journal.desired_hash = Some(current_hash);
                journal.candidate_identity = None;
                write_rollback_journal(&paths.journal, journal)?;
                cleanup_rollback_artifacts(descriptor, paths, journal, observer, attempt)?;
                return Ok(());
            }
            journal.delete_intent = false;
            journal.delete_target_identity = None;
            journal.expected_hash = current_hash;
            journal.expected_target_identity = current_identity;
            journal.desired_hash = Some(sha256_hex(&desired));
            journal.candidate_identity = None;
            write_rollback_journal(&paths.journal, journal)?;
            return execute_rollback_journal(descriptor, journal.clone(), observer);
        }
        observer(attempt, RollbackPhase::AfterDeleteIntentPrepared);
        delete_locked_file(&file)?;
        drop(file);
        observer(attempt, RollbackPhase::AfterDelete);
        cleanup_rollback_artifacts(descriptor, paths, journal, observer, attempt)?;
        return Ok(());
    }
    Err(conflict_error("presetRollbackConflict"))
}

fn ensure_candidate(
    descriptor: &PresetDescriptor,
    paths: &RollbackPaths,
    journal: &mut RollbackJournal,
    base: &[u8],
    observer: &mut impl FnMut(usize, RollbackPhase),
    attempt: usize,
) -> Result<(), CommandError> {
    match open_replace_source_guard(&paths.candidate) {
        Ok(Some(mut file)) => {
            let bytes = read_bounded_open_file(&mut file, MAX_PRESET_CONFIG_BYTES)?;
            let identity = file_identity(&file)?;
            let hash = sha256_hex(&bytes);
            if let Some(expected_identity) = journal.candidate_identity.as_ref() {
                return if identity.as_ref() == Some(expected_identity)
                    && Some(hash.as_str()) == journal.desired_hash.as_deref()
                {
                    Ok(())
                } else {
                    Err(conflict_error("presetRollbackConflict"))
                };
            }
            if Some(hash.as_str()) != journal.desired_hash.as_deref() {
                return Err(conflict_error("presetRollbackConflict"));
            }
            let target = match guarded_file_state(&descriptor.config_path, MAX_PRESET_CONFIG_BYTES)?
            {
                GuardedFileState::Present(target)
                    if target.hash == journal.expected_hash
                        && target.identity.as_ref()
                            == journal.expected_target_identity.as_ref() =>
                {
                    target
                }
                GuardedFileState::Present(_)
                | GuardedFileState::Missing
                | GuardedFileState::Busy => {
                    return Err(conflict_error("presetRollbackConflict"));
                }
            };
            verify_candidate_adoption_sidecars(paths, journal)?;
            drop(target);
            journal.candidate_identity = identity;
            return write_rollback_journal(&paths.journal, journal);
        }
        Ok(None) => {}
        Err(_) => return Err(conflict_error("presetRollbackConflict")),
    }
    let (desired, _) = merge_preset_document(
        &descriptor.adapter_id,
        &descriptor.owned_hooks,
        base,
        journal.action.merge_action(),
    )?;
    if Some(sha256_hex(&desired).as_str()) != journal.desired_hash.as_deref() {
        return Err(conflict_error("presetRollbackJournalInvalid"));
    }
    atomic_write(&paths.candidate, &desired)?;
    sync_path(&paths.candidate)?;
    observer(
        attempt,
        RollbackPhase::AfterCandidateSyncBeforeIdentityJournal,
    );
    let Some(mut file) = open_replace_source_guard(&paths.candidate)
        .map_err(|_| conflict_error("presetRollbackConflict"))?
    else {
        return Err(conflict_error("presetRollbackConflict"));
    };
    let bytes = read_bounded_open_file(&mut file, MAX_PRESET_CONFIG_BYTES)?;
    if Some(sha256_hex(&bytes).as_str()) != journal.desired_hash.as_deref() {
        return Err(conflict_error("presetRollbackConflict"));
    }
    journal.candidate_identity = file_identity(&file)?;
    write_rollback_journal(&paths.journal, journal)
}

fn verify_candidate_adoption_sidecars(
    paths: &RollbackPaths,
    journal: &RollbackJournal,
) -> Result<(), CommandError> {
    if !matches!(
        guarded_file_state(&paths.displaced, MAX_PRESET_CONFIG_BYTES)?,
        GuardedFileState::Missing
    ) {
        return Err(conflict_error("presetRollbackConflict"));
    }
    match journal.preserved_hash.as_deref() {
        Some(hash) => verify_preserved_slot(
            &paths.rescue,
            Some(hash),
            journal.preserved_identity.as_ref(),
        )?,
        None if !matches!(
            guarded_file_state(&paths.rescue, MAX_PRESET_CONFIG_BYTES)?,
            GuardedFileState::Missing
        ) =>
        {
            return Err(conflict_error("presetRollbackConflict"));
        }
        None => {}
    }
    match journal.rollover_preserved_hash.as_deref() {
        Some(hash) => verify_preserved_slot(
            &paths.rollover_rescue,
            Some(hash),
            journal.rollover_preserved_identity.as_ref(),
        ),
        None if !matches!(
            guarded_file_state(&paths.rollover_rescue, MAX_PRESET_CONFIG_BYTES)?,
            GuardedFileState::Missing
        ) =>
        {
            Err(conflict_error("presetRollbackConflict"))
        }
        None => Ok(()),
    }
}

fn rebase_rollback_journal(
    descriptor: &PresetDescriptor,
    paths: &RollbackPaths,
    journal: &mut RollbackJournal,
    base: &[u8],
    expected_target_hash: String,
    preserved_displaced: Option<&FileGeneration>,
    observer: &mut impl FnMut(usize, RollbackPhase),
    attempt: usize,
) -> Result<(), CommandError> {
    discard_owned_candidate(paths, journal)?;
    let (desired, _) = merge_preset_document(
        &descriptor.adapter_id,
        &descriptor.owned_hooks,
        base,
        journal.action.merge_action(),
    )?;
    if let Some(preserved_displaced) = preserved_displaced {
        if journal.preserved_hash.is_none() && !paths.rescue.exists() {
            journal.preserved_hash = Some(preserved_displaced.hash.clone());
            journal.preserved_identity = preserved_displaced.identity.clone();
        } else if journal.rollover_preserved_hash.is_none() && !paths.rollover_rescue.exists() {
            journal.rollover_preserved_hash = Some(preserved_displaced.hash.clone());
            journal.rollover_preserved_identity = preserved_displaced.identity.clone();
        } else {
            return Err(conflict_error("presetRollbackConflict"));
        }
    }
    journal.expected_hash = expected_target_hash;
    journal.expected_target_identity =
        current_file_identity(&descriptor.config_path, &journal.expected_hash)?;
    journal.desired_hash = Some(sha256_hex(&desired));
    journal.candidate_identity = None;
    write_rollback_journal(&paths.journal, journal)?;
    if journal.rollover_preserved_hash.is_some() {
        observer(attempt, RollbackPhase::AfterRolloverJournalBeforeNormalize);
    }
    normalize_preserved_displaced(paths, journal)?;
    ensure_candidate(descriptor, paths, journal, base, observer, attempt)
}

fn discard_owned_candidate(
    paths: &RollbackPaths,
    journal: &RollbackJournal,
) -> Result<(), CommandError> {
    match guarded_file_state(&paths.candidate, MAX_PRESET_CONFIG_BYTES)? {
        GuardedFileState::Missing => Ok(()),
        GuardedFileState::Busy => Err(conflict_error("presetRollbackConflict")),
        GuardedFileState::Present(candidate)
            if candidate.identity.as_ref() == journal.candidate_identity.as_ref()
                && Some(candidate.hash.as_str()) == journal.desired_hash.as_deref()
                && journal.candidate_identity.is_some() =>
        {
            delete_locked_file(&candidate.file)
        }
        GuardedFileState::Present(_) => Err(conflict_error("presetRollbackConflict")),
    }
}

fn current_file_identity(
    path: &Path,
    expected_hash: &str,
) -> Result<Option<WindowsFileIdentity>, CommandError> {
    let Some(mut file) =
        open_exclusive_rollback_file(path).map_err(|_| conflict_error("presetRollbackConflict"))?
    else {
        return Err(conflict_error("presetRollbackConflict"));
    };
    let bytes = read_bounded_open_file(&mut file, MAX_PRESET_CONFIG_BYTES)?;
    if sha256_hex(&bytes) != expected_hash {
        return Err(conflict_error("presetRollbackConflict"));
    }
    file_identity(&file)
}

fn normalize_preserved_displaced(
    paths: &RollbackPaths,
    journal: &mut RollbackJournal,
) -> Result<(), CommandError> {
    if journal.rollover_preserved_hash.is_some() {
        verify_preserved_slot(
            &paths.rescue,
            journal.preserved_hash.as_deref(),
            journal.preserved_identity.as_ref(),
        )?;
        materialize_preserved_slot(
            &paths.displaced,
            &paths.rollover_rescue,
            journal.rollover_preserved_hash.as_deref(),
            journal.rollover_preserved_identity.as_ref(),
        )?;
        return Ok(());
    }
    if paths.rollover_rescue.exists() {
        return Err(conflict_error("presetRollbackConflict"));
    }
    let Some(expected) = journal.preserved_hash.as_deref() else {
        if paths.rescue.exists() {
            return Err(conflict_error("presetRollbackConflict"));
        }
        return Ok(());
    };
    materialize_preserved_slot(
        &paths.displaced,
        &paths.rescue,
        Some(expected),
        journal.preserved_identity.as_ref(),
    )
}

fn verify_preserved_slot(
    path: &Path,
    expected_hash: Option<&str>,
    expected_identity: Option<&WindowsFileIdentity>,
) -> Result<(), CommandError> {
    match guarded_file_state(path, MAX_PRESET_CONFIG_BYTES)? {
        GuardedFileState::Present(snapshot)
            if expected_hash == Some(snapshot.hash.as_str())
                && snapshot.identity.as_ref() == expected_identity =>
        {
            Ok(())
        }
        GuardedFileState::Present(_) | GuardedFileState::Missing | GuardedFileState::Busy => {
            Err(conflict_error("presetRollbackConflict"))
        }
    }
}

fn materialize_preserved_slot(
    source: &Path,
    destination: &Path,
    expected_hash: Option<&str>,
    expected_identity: Option<&WindowsFileIdentity>,
) -> Result<(), CommandError> {
    match guarded_file_state(destination, MAX_PRESET_CONFIG_BYTES)? {
        GuardedFileState::Present(snapshot)
            if expected_hash == Some(snapshot.hash.as_str())
                && snapshot.identity.as_ref() == expected_identity =>
        {
            if !matches!(
                guarded_file_state(source, MAX_PRESET_CONFIG_BYTES)?,
                GuardedFileState::Missing
            ) {
                return Err(conflict_error("presetRollbackConflict"));
            }
            return Ok(());
        }
        GuardedFileState::Present(_) | GuardedFileState::Busy => {
            return Err(conflict_error("presetRollbackConflict"));
        }
        GuardedFileState::Missing => {}
    }
    let source_snapshot = match guarded_file_state(source, MAX_PRESET_CONFIG_BYTES)? {
        GuardedFileState::Present(snapshot)
            if expected_hash == Some(snapshot.hash.as_str())
                && snapshot.identity.as_ref() == expected_identity =>
        {
            snapshot
        }
        GuardedFileState::Present(_) | GuardedFileState::Missing | GuardedFileState::Busy => {
            return Err(conflict_error("presetRollbackConflict"));
        }
    };
    drop(source_snapshot);
    let _ = move_file_if_absent(source, destination);
    sync_existing_path(source)?;
    sync_existing_path(destination)?;
    verify_preserved_slot(destination, expected_hash, expected_identity)?;
    if !matches!(
        guarded_file_state(source, MAX_PRESET_CONFIG_BYTES)?,
        GuardedFileState::Missing
    ) {
        return Err(conflict_error("presetRollbackConflict"));
    }
    Ok(())
}

fn recover_missing_target(
    descriptor: &PresetDescriptor,
    paths: &RollbackPaths,
    journal: &mut RollbackJournal,
    observer: &mut impl FnMut(usize, RollbackPhase),
    attempt: usize,
) -> Result<(), CommandError> {
    rebuild_missing_candidate_from_displaced(descriptor, paths, journal, observer, attempt)?;
    let desired_hash = journal
        .desired_hash
        .clone()
        .ok_or_else(|| conflict_error("presetRollbackJournalInvalid"))?;
    let (candidate, displaced) = guard_missing_recovery_inputs(paths, journal, &desired_hash)?;
    drop(candidate);
    drop(displaced);
    observer(attempt, RollbackPhase::BeforeMissingTargetCommit);
    let (candidate, displaced) = guard_missing_recovery_inputs(paths, journal, &desired_hash)?;
    drop(candidate);
    drop(displaced);
    observer(attempt, RollbackPhase::AfterMissingPreflightBeforeMove);
    let (candidate, displaced) = guard_missing_recovery_inputs(paths, journal, &desired_hash)?;
    drop(candidate);
    drop(displaced);
    let _ = move_file_if_absent(&paths.candidate, &descriptor.config_path);
    sync_existing_path(&descriptor.config_path)?;
    sync_existing_path(&paths.candidate)?;
    sync_existing_path(&paths.displaced)?;
    observer(attempt, RollbackPhase::AfterMissingMoveBeforePostValidation);
    let target = match guarded_file_state(&descriptor.config_path, MAX_PRESET_CONFIG_BYTES)? {
        GuardedFileState::Present(target) => target,
        GuardedFileState::Missing | GuardedFileState::Busy => {
            return Err(conflict_error("presetRollbackConflict"));
        }
    };
    if target.hash == desired_hash
        && target.identity.as_ref() == journal.candidate_identity.as_ref()
        && matches!(
            guarded_file_state(&paths.candidate, MAX_PRESET_CONFIG_BYTES)?,
            GuardedFileState::Missing
        )
    {
        drop(target);
        cleanup_rollback_artifacts(descriptor, paths, journal, observer, attempt)?;
        return Ok(());
    }
    drop(target);
    Err(conflict_error("presetRollbackConflict"))
}

fn rebuild_missing_candidate_from_displaced(
    descriptor: &PresetDescriptor,
    paths: &RollbackPaths,
    journal: &mut RollbackJournal,
    observer: &mut impl FnMut(usize, RollbackPhase),
    attempt: usize,
) -> Result<(), CommandError> {
    if !matches!(
        guarded_file_state(&descriptor.config_path, MAX_PRESET_CONFIG_BYTES)?,
        GuardedFileState::Missing
    ) {
        return Err(conflict_error("presetRollbackConflict"));
    }
    let displaced = match guarded_file_state(&paths.displaced, MAX_PRESET_CONFIG_BYTES)? {
        GuardedFileState::Present(displaced)
            if displaced.identity.as_ref() == journal.expected_target_identity.as_ref() =>
        {
            displaced
        }
        GuardedFileState::Present(_) | GuardedFileState::Missing | GuardedFileState::Busy => {
            return Err(conflict_error("presetRollbackConflict"));
        }
    };
    if displaced.hash != journal.expected_hash {
        discard_missing_rebuild_candidate(paths, journal)?;
        let (rebased_desired, _) = merge_preset_document(
            &descriptor.adapter_id,
            &descriptor.owned_hooks,
            &displaced.bytes,
            journal.action.merge_action(),
        )?;
        journal.expected_hash = displaced.hash.clone();
        journal.expected_target_identity = displaced.identity.clone();
        journal.desired_hash = Some(sha256_hex(&rebased_desired));
        journal.candidate_identity = None;
        write_rollback_journal(&paths.journal, journal)?;
    }
    let (desired, _) = merge_preset_document(
        &descriptor.adapter_id,
        &descriptor.owned_hooks,
        &displaced.bytes,
        journal.action.merge_action(),
    )?;
    let current_desired_hash = journal
        .desired_hash
        .clone()
        .ok_or_else(|| conflict_error("presetRollbackJournalInvalid"))?;
    if sha256_hex(&desired) != current_desired_hash {
        return Err(conflict_error("presetRollbackJournalInvalid"));
    }
    match guarded_file_state(&paths.candidate, MAX_PRESET_CONFIG_BYTES)? {
        GuardedFileState::Present(candidate)
            if candidate.hash == current_desired_hash
                && candidate.identity.as_ref() == journal.candidate_identity.as_ref() =>
        {
            return Ok(());
        }
        GuardedFileState::Present(candidate)
            if candidate.hash == current_desired_hash && journal.candidate_identity.is_none() =>
        {
            journal.candidate_identity = candidate.identity.clone();
            return write_rollback_journal(&paths.journal, journal);
        }
        GuardedFileState::Present(_) | GuardedFileState::Busy => {
            return Err(conflict_error("presetRollbackConflict"));
        }
        GuardedFileState::Missing => {}
    }
    if journal.candidate_identity.is_some() {
        journal.candidate_identity = None;
        write_rollback_journal(&paths.journal, journal)?;
    }
    drop(displaced);
    atomic_write(&paths.candidate, &desired)?;
    sync_path(&paths.candidate)?;
    observer(
        attempt,
        RollbackPhase::AfterCandidateSyncBeforeIdentityJournal,
    );
    let candidate = match guarded_file_state(&paths.candidate, MAX_PRESET_CONFIG_BYTES)? {
        GuardedFileState::Present(candidate) if candidate.hash == current_desired_hash => candidate,
        GuardedFileState::Present(_) | GuardedFileState::Missing | GuardedFileState::Busy => {
            return Err(conflict_error("presetRollbackConflict"));
        }
    };
    journal.candidate_identity = candidate.identity.clone();
    write_rollback_journal(&paths.journal, journal)
}

fn discard_missing_rebuild_candidate(
    paths: &RollbackPaths,
    journal: &RollbackJournal,
) -> Result<(), CommandError> {
    match guarded_file_state(&paths.candidate, MAX_PRESET_CONFIG_BYTES)? {
        GuardedFileState::Missing => Ok(()),
        GuardedFileState::Busy => Err(conflict_error("presetRollbackConflict")),
        GuardedFileState::Present(candidate)
            if Some(candidate.hash.as_str()) == journal.desired_hash.as_deref()
                && (journal.candidate_identity.is_none()
                    || candidate.identity.as_ref() == journal.candidate_identity.as_ref()) =>
        {
            delete_locked_file(&candidate.file)
        }
        GuardedFileState::Present(_) => Err(conflict_error("presetRollbackConflict")),
    }
}

fn guard_missing_recovery_inputs(
    paths: &RollbackPaths,
    journal: &RollbackJournal,
    desired_hash: &str,
) -> Result<(GuardedFileSnapshot, GuardedFileSnapshot), CommandError> {
    let candidate = match guarded_file_state(&paths.candidate, MAX_PRESET_CONFIG_BYTES)? {
        GuardedFileState::Present(candidate)
            if candidate.hash == desired_hash
                && candidate.identity.as_ref() == journal.candidate_identity.as_ref() =>
        {
            candidate
        }
        GuardedFileState::Present(_) | GuardedFileState::Missing | GuardedFileState::Busy => {
            return Err(conflict_error("presetRollbackConflict"));
        }
    };
    let displaced = match guarded_file_state(&paths.displaced, MAX_PRESET_CONFIG_BYTES)? {
        GuardedFileState::Present(displaced)
            if displaced.hash == journal.expected_hash
                && displaced.identity.as_ref() == journal.expected_target_identity.as_ref() =>
        {
            displaced
        }
        GuardedFileState::Present(_) | GuardedFileState::Missing | GuardedFileState::Busy => {
            return Err(conflict_error("presetRollbackConflict"));
        }
    };
    Ok((candidate, displaced))
}

fn write_rollback_journal(path: &Path, journal: &RollbackJournal) -> Result<(), CommandError> {
    let bytes = serde_json::to_vec(journal).map_err(|_| io_error("presetRollbackJournalWrite"))?;
    if bytes.len() as u64 > MAX_ROLLBACK_JOURNAL_BYTES {
        return Err(io_error("presetRollbackJournalWrite"));
    }
    atomic_write(path, &bytes)?;
    sync_path(path)
}

fn cleanup_rollback_artifacts(
    descriptor: &PresetDescriptor,
    paths: &RollbackPaths,
    journal: &mut RollbackJournal,
    observer: &mut impl FnMut(usize, RollbackPhase),
    attempt: usize,
) -> Result<(), CommandError> {
    let expected_final_identity =
        if journal.desired_hash.as_deref() == Some(journal.expected_hash.as_str()) {
            journal.expected_target_identity.as_ref()
        } else {
            journal.candidate_identity.as_ref()
        };
    let target_guard = match guarded_file_state(&descriptor.config_path, MAX_PRESET_CONFIG_BYTES)? {
        GuardedFileState::Busy => return Err(conflict_error("presetRollbackConflict")),
        GuardedFileState::Missing if journal.delete_intent => None,
        GuardedFileState::Present(target)
            if !journal.delete_intent
                && Some(target.hash.as_str()) == journal.desired_hash.as_deref()
                && target.identity.as_ref() == expected_final_identity =>
        {
            Some(target)
        }
        GuardedFileState::Present(_) => return Err(conflict_error("presetRollbackConflict")),
        GuardedFileState::Missing => return Err(conflict_error("presetRollbackConflict")),
    };
    let artifact_phases = [
        RollbackCleanupPhase::Candidate,
        RollbackCleanupPhase::Displaced,
        RollbackCleanupPhase::Rescue,
        RollbackCleanupPhase::RolloverRescue,
    ];
    if journal.cleanup_phase.is_none() {
        for phase in artifact_phases {
            verify_cleanup_artifact(paths, journal, phase)?;
        }
    }
    let start = journal
        .cleanup_phase
        .unwrap_or(RollbackCleanupPhase::Candidate);
    for phase in artifact_phases {
        if cleanup_phase_rank(phase) < cleanup_phase_rank(start) {
            verify_cleanup_artifact_was_consumed(paths, phase)?;
        }
    }
    for phase in artifact_phases {
        if cleanup_phase_rank(phase) < cleanup_phase_rank(start) {
            continue;
        }
        if journal.cleanup_phase != Some(phase) {
            journal.cleanup_phase = Some(phase);
            write_rollback_journal(&paths.journal, journal)?;
        }
        delete_cleanup_artifact(paths, journal, phase)?;
        observer(attempt, cleanup_observer_phase(phase));
    }
    journal.cleanup_phase = Some(RollbackCleanupPhase::Journal);
    write_rollback_journal(&paths.journal, journal)?;
    let journal_bytes =
        serde_json::to_vec(journal).map_err(|_| conflict_error("presetRollbackJournalInvalid"))?;
    let journal_hash = sha256_hex(&journal_bytes);
    let journal_file = match guarded_file_state(&paths.journal, MAX_ROLLBACK_JOURNAL_BYTES)? {
        GuardedFileState::Present(snapshot) if snapshot.hash == journal_hash => snapshot.file,
        GuardedFileState::Missing | GuardedFileState::Present(_) | GuardedFileState::Busy => {
            return Err(conflict_error("presetRollbackConflict"));
        }
    };
    delete_locked_file(&journal_file)?;
    drop(target_guard);
    Ok(())
}

fn verify_cleanup_artifact_was_consumed(
    paths: &RollbackPaths,
    phase: RollbackCleanupPhase,
) -> Result<(), CommandError> {
    let path = match phase {
        RollbackCleanupPhase::Candidate => &paths.candidate,
        RollbackCleanupPhase::Displaced => &paths.displaced,
        RollbackCleanupPhase::Rescue => &paths.rescue,
        RollbackCleanupPhase::RolloverRescue => &paths.rollover_rescue,
        RollbackCleanupPhase::Journal => unreachable!("journal is cleaned separately"),
    };
    match guarded_file_state(path, MAX_PRESET_CONFIG_BYTES)? {
        GuardedFileState::Missing => Ok(()),
        GuardedFileState::Present(_) | GuardedFileState::Busy => {
            Err(conflict_error("presetRollbackConflict"))
        }
    }
}

fn cleanup_phase_rank(phase: RollbackCleanupPhase) -> usize {
    match phase {
        RollbackCleanupPhase::Candidate => 0,
        RollbackCleanupPhase::Displaced => 1,
        RollbackCleanupPhase::Rescue => 2,
        RollbackCleanupPhase::RolloverRescue => 3,
        RollbackCleanupPhase::Journal => 4,
    }
}

fn cleanup_observer_phase(phase: RollbackCleanupPhase) -> RollbackPhase {
    match phase {
        RollbackCleanupPhase::Candidate => RollbackPhase::AfterCleanupCandidate,
        RollbackCleanupPhase::Displaced => RollbackPhase::AfterCleanupDisplaced,
        RollbackCleanupPhase::Rescue => RollbackPhase::AfterCleanupRescue,
        RollbackCleanupPhase::RolloverRescue => RollbackPhase::AfterCleanupRolloverRescue,
        RollbackCleanupPhase::Journal => unreachable!("journal cleanup has no observer phase"),
    }
}

fn cleanup_artifact_expectation<'a>(
    paths: &'a RollbackPaths,
    journal: &'a RollbackJournal,
    phase: RollbackCleanupPhase,
) -> (&'a Path, Option<&'a str>, Option<&'a WindowsFileIdentity>) {
    match phase {
        RollbackCleanupPhase::Candidate => (
            &paths.candidate,
            journal.desired_hash.as_deref(),
            journal.candidate_identity.as_ref(),
        ),
        RollbackCleanupPhase::Displaced => (
            &paths.displaced,
            Some(journal.expected_hash.as_str()),
            journal.expected_target_identity.as_ref(),
        ),
        RollbackCleanupPhase::Rescue => (
            &paths.rescue,
            journal.preserved_hash.as_deref(),
            journal.preserved_identity.as_ref(),
        ),
        RollbackCleanupPhase::RolloverRescue => (
            &paths.rollover_rescue,
            journal.rollover_preserved_hash.as_deref(),
            journal.rollover_preserved_identity.as_ref(),
        ),
        RollbackCleanupPhase::Journal => unreachable!("journal is cleaned separately"),
    }
}

fn verify_cleanup_artifact(
    paths: &RollbackPaths,
    journal: &RollbackJournal,
    phase: RollbackCleanupPhase,
) -> Result<(), CommandError> {
    let (path, expected_hash, expected_identity) =
        cleanup_artifact_expectation(paths, journal, phase);
    match guarded_file_state(path, MAX_PRESET_CONFIG_BYTES)? {
        GuardedFileState::Missing => Ok(()),
        GuardedFileState::Busy => Err(conflict_error("presetRollbackConflict")),
        GuardedFileState::Present(snapshot)
            if expected_hash.is_some_and(|expected| expected == snapshot.hash)
                && snapshot.identity.as_ref() == expected_identity =>
        {
            Ok(())
        }
        GuardedFileState::Present(_) => Err(conflict_error("presetRollbackConflict")),
    }
}

fn delete_cleanup_artifact(
    paths: &RollbackPaths,
    journal: &RollbackJournal,
    phase: RollbackCleanupPhase,
) -> Result<(), CommandError> {
    let (path, expected_hash, expected_identity) =
        cleanup_artifact_expectation(paths, journal, phase);
    match guarded_file_state(path, MAX_PRESET_CONFIG_BYTES)? {
        GuardedFileState::Missing => Ok(()),
        GuardedFileState::Busy => Err(conflict_error("presetRollbackConflict")),
        GuardedFileState::Present(snapshot)
            if expected_hash.is_some_and(|expected| expected == snapshot.hash)
                && snapshot.identity.as_ref() == expected_identity =>
        {
            delete_locked_file(&snapshot.file)
        }
        GuardedFileState::Present(_) => Err(conflict_error("presetRollbackConflict")),
    }
}

fn remove_rollback_artifacts_without_journal(paths: &RollbackPaths) -> Result<(), CommandError> {
    if paths.journal.exists()
        || paths.candidate.exists()
        || paths.displaced.exists()
        || paths.rescue.exists()
        || paths.rollover_rescue.exists()
    {
        return Err(conflict_error("presetRollbackConflict"));
    }
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<(), CommandError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(io_error("presetRollbackCleanup")),
    }
}

fn file_name_string(path: &Path) -> Result<String, CommandError> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .ok_or_else(|| io_error("presetConfigName"))
}

fn sync_existing_path(path: &Path) -> Result<(), CommandError> {
    if path.exists() {
        sync_path(path)
    } else {
        Ok(())
    }
}

fn sync_path(path: &Path) -> Result<(), CommandError> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| io_error("presetRollbackFlush"))
}

#[derive(Debug)]
enum VerifiedReplaceOutcome {
    Invoked,
    TargetChanged(Vec<u8>),
    CandidateChanged,
    TargetMissing,
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReplaceCallOutcome {
    ReportedSuccess,
    ReportedPartialFailure,
}

#[cfg(windows)]
fn replace_verified_with_backup(
    candidate: &Path,
    target: &Path,
    displaced: &Path,
    expected_hash: &str,
    expected_target_identity: Option<&WindowsFileIdentity>,
    desired_hash: &str,
    expected_candidate_identity: Option<&WindowsFileIdentity>,
    before_replace: impl FnOnce(),
) -> Result<VerifiedReplaceOutcome, CommandError> {
    replace_verified_with_backup_using(
        candidate,
        target,
        displaced,
        expected_hash,
        expected_target_identity,
        desired_hash,
        expected_candidate_identity,
        before_replace,
        replace_file_with_backup,
    )
}

#[cfg(windows)]
fn replace_verified_with_backup_using(
    candidate: &Path,
    target: &Path,
    displaced: &Path,
    expected_hash: &str,
    expected_target_identity: Option<&WindowsFileIdentity>,
    desired_hash: &str,
    expected_candidate_identity: Option<&WindowsFileIdentity>,
    before_replace: impl FnOnce(),
    replace: impl FnOnce(&Path, &Path, &Path) -> Result<ReplaceCallOutcome, CommandError>,
) -> Result<VerifiedReplaceOutcome, CommandError> {
    let Some(mut target_preflight) = open_exclusive_rollback_file(target)
        .map_err(|_| conflict_error("presetRollbackConflict"))?
    else {
        return Ok(VerifiedReplaceOutcome::TargetMissing);
    };
    let Some(mut candidate_preflight) = open_replace_source_guard(candidate)
        .map_err(|_| conflict_error("presetRollbackConflict"))?
    else {
        return Ok(VerifiedReplaceOutcome::CandidateChanged);
    };
    let target_bytes = read_bounded_open_file(&mut target_preflight, MAX_PRESET_CONFIG_BYTES)?;
    if sha256_hex(&target_bytes) != expected_hash
        || file_identity(&target_preflight)?.as_ref() != expected_target_identity
    {
        return Ok(VerifiedReplaceOutcome::TargetChanged(target_bytes));
    }
    let candidate_bytes =
        read_bounded_open_file(&mut candidate_preflight, MAX_PRESET_CONFIG_BYTES)?;
    if sha256_hex(&candidate_bytes) != desired_hash
        || file_identity(&candidate_preflight)?.as_ref() != expected_candidate_identity
    {
        return Ok(VerifiedReplaceOutcome::CandidateChanged);
    }
    let candidate_identity = file_identity(&candidate_preflight)?;
    target_preflight
        .sync_all()
        .map_err(|_| io_error("presetRollbackFlush"))?;
    if displaced.exists() {
        return Ok(VerifiedReplaceOutcome::Busy);
    }
    drop(target_preflight);
    drop(candidate_preflight);
    before_replace();
    let _reported = replace(candidate, target, displaced)?;
    match guarded_file_state(target, MAX_PRESET_CONFIG_BYTES)? {
        GuardedFileState::Busy => Ok(VerifiedReplaceOutcome::Busy),
        GuardedFileState::Present(replacement)
            if replacement.hash == desired_hash && replacement.identity == candidate_identity =>
        {
            Ok(VerifiedReplaceOutcome::Invoked)
        }
        GuardedFileState::Present(replacement)
            if replacement.hash == expected_hash
                && replacement.identity.as_ref() == expected_target_identity =>
        {
            match guarded_file_state(candidate, MAX_PRESET_CONFIG_BYTES)? {
                GuardedFileState::Present(candidate)
                    if candidate.hash == desired_hash
                        && candidate.identity.as_ref() == expected_candidate_identity =>
                {
                    Ok(VerifiedReplaceOutcome::Busy)
                }
                GuardedFileState::Present(_)
                | GuardedFileState::Missing
                | GuardedFileState::Busy => Ok(VerifiedReplaceOutcome::CandidateChanged),
            }
        }
        GuardedFileState::Present(replacement) => {
            Ok(VerifiedReplaceOutcome::TargetChanged(replacement.bytes))
        }
        GuardedFileState::Missing => {
            let candidate_matches = matches!(
                guarded_file_state(candidate, MAX_PRESET_CONFIG_BYTES)?,
                GuardedFileState::Present(candidate)
                    if candidate.hash == desired_hash
                        && candidate.identity.as_ref() == expected_candidate_identity
            );
            let displaced_matches = matches!(
                guarded_file_state(displaced, MAX_PRESET_CONFIG_BYTES)?,
                GuardedFileState::Present(displaced)
                    if displaced.hash == expected_hash
                        && displaced.identity.as_ref() == expected_target_identity
            );
            if candidate_matches && displaced_matches {
                Ok(VerifiedReplaceOutcome::Invoked)
            } else {
                Ok(VerifiedReplaceOutcome::TargetMissing)
            }
        }
    }
}

#[cfg(not(windows))]
fn replace_verified_with_backup(
    _candidate: &Path,
    _target: &Path,
    _displaced: &Path,
    _expected_hash: &str,
    _expected_target_identity: Option<&WindowsFileIdentity>,
    _desired_hash: &str,
    _expected_candidate_identity: Option<&WindowsFileIdentity>,
    _before_replace: impl FnOnce(),
) -> Result<VerifiedReplaceOutcome, CommandError> {
    Ok(VerifiedReplaceOutcome::Busy)
}

#[cfg(windows)]
fn open_replace_source_guard(path: &Path) -> std::io::Result<Option<File>> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ};

    match OpenOptions::new()
        .access_mode(GENERIC_READ.0)
        .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_DELETE.0)
        .open(path)
    {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn replace_file_with_backup(
    replacement: &Path,
    target: &Path,
    backup: &Path,
) -> Result<ReplaceCallOutcome, CommandError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{ReplaceFileW, REPLACE_FILE_FLAGS};

    #[cfg(test)]
    match SIMULATED_REPLACE_ERROR.with(|simulate| simulate.replace(0)) {
        1175 | 1176 => return Ok(ReplaceCallOutcome::ReportedPartialFailure),
        1177 => {
            move_file_if_absent(target, backup)?;
            return Ok(ReplaceCallOutcome::ReportedPartialFailure);
        }
        _ => {}
    }
    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>()
    };
    let replacement = wide(replacement);
    let target = wide(target);
    let backup = wide(backup);
    let result = unsafe {
        ReplaceFileW(
            PCWSTR(target.as_ptr()),
            PCWSTR(replacement.as_ptr()),
            PCWSTR(backup.as_ptr()),
            REPLACE_FILE_FLAGS(0),
            None,
            None,
        )
    };
    match result {
        Ok(()) => Ok(ReplaceCallOutcome::ReportedSuccess),
        Err(error) => {
            let dos_error = (error.code().0 as u32) & 0x0000_ffff;
            if matches!(dos_error, 1175..=1177) {
                Ok(ReplaceCallOutcome::ReportedPartialFailure)
            } else {
                Err(io_error("presetRollbackReplace"))
            }
        }
    }
}

#[cfg(not(windows))]
fn open_replace_source_guard(path: &Path) -> std::io::Result<Option<File>> {
    open_exclusive_rollback_file(path)
}

#[cfg(not(windows))]
fn replace_file_with_backup(
    _replacement: &Path,
    _target: &Path,
    _backup: &Path,
) -> Result<ReplaceCallOutcome, CommandError> {
    Err(conflict_error("presetRollbackUnsupported"))
}

struct PresetDescriptor {
    profile_id: AgentIntegrationId,
    adapter_id: PresetAgentAdapterId,
    config_path: PathBuf,
    owned_hooks: Vec<OwnedHookFragment>,
}

struct SpoolWatcherRuntime {
    stop: mpsc::Sender<()>,
    dirty: mpsc::SyncSender<()>,
    join: Option<thread::JoinHandle<()>>,
}

fn dirty_hint_channel() -> (mpsc::SyncSender<()>, mpsc::Receiver<()>) {
    mpsc::sync_channel(1)
}

fn mark_dirty(sender: &mpsc::SyncSender<()>) -> bool {
    sender.try_send(()).is_ok()
}

fn stop_requested(stop: Option<&mpsc::Receiver<()>>) -> bool {
    stop.is_some_and(|receiver| {
        matches!(
            receiver.try_recv(),
            Ok(()) | Err(mpsc::TryRecvError::Disconnected)
        )
    })
}

impl SpoolWatcherRuntime {
    fn mark_dirty(&self) {
        let _ = mark_dirty(&self.dirty);
    }

    fn stop(mut self) {
        let _ = self.stop.send(());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl PresetProfileBridge {
    pub fn new(
        repository: AgentProfileRepository,
        emitter: Arc<dyn EventEmitterPort>,
        windows_home: PathBuf,
        app_data_dir: PathBuf,
    ) -> Self {
        let roaming_app_data = app_data_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| app_data_dir.clone());
        Self {
            repository,
            emitter,
            windows_home,
            roaming_app_data,
            spool_dir: app_data_dir.join("agent-profile-events"),
            script_path: app_data_dir
                .join("agent-hooks")
                .join(PROFILE_EVENT_SCRIPT_NAME),
            watcher: Mutex::new(None),
            installing: Mutex::new(HashSet::new()),
        }
    }

    pub fn ensure_started(self: &Arc<Self>) -> Result<(), CommandError> {
        let mut watcher_slot = self
            .watcher
            .lock()
            .expect("profile spool watcher lock poisoned");
        if watcher_slot.is_some() {
            return Ok(());
        }
        self.recover_pending_rollbacks()?;
        fs::create_dir_all(&self.spool_dir).map_err(|_| io_error("profileSpoolDirectory"))?;
        let (dirty_tx, dirty_rx) = dirty_hint_channel();
        let watcher_dirty = dirty_tx.clone();
        let mut watcher = RecommendedWatcher::new(
            move |result: notify::Result<notify::Event>| {
                if result.is_ok() {
                    let _ = mark_dirty(&watcher_dirty);
                }
            },
            Config::default(),
        )
        .map_err(|_| io_error("profileSpoolWatcher"))?;
        watcher
            .watch(&self.spool_dir, RecursiveMode::NonRecursive)
            .map_err(|_| io_error("profileSpoolWatch"))?;
        let (stop_tx, stop_rx) = mpsc::channel();
        let bridge = self.clone();
        let join = thread::Builder::new()
            .name("agent-profile-spool".into())
            .spawn(move || {
                let _watcher = watcher;
                loop {
                    if stop_rx.try_recv().is_ok() {
                        return;
                    }
                    match dirty_rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(()) => match bridge.scan_pending_with_stop(Some(&stop_rx)) {
                            Ok((_, true)) => return,
                            Ok((_, false)) => {}
                            Err(error) => log::warn!(
                                "agent profile spool scan failed: code={:?} message_key={}",
                                error.code,
                                error.message_key
                            ),
                        },
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
            })
            .map_err(|_| io_error("profileSpoolThread"))?;
        *watcher_slot = Some(SpoolWatcherRuntime {
            stop: stop_tx,
            dirty: dirty_tx,
            join: Some(join),
        });
        watcher_slot
            .as_ref()
            .expect("profile spool watcher just installed")
            .mark_dirty();
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        self.watcher
            .lock()
            .expect("profile spool watcher lock poisoned")
            .is_some()
    }

    pub fn shutdown(&self) {
        let watcher = self
            .watcher
            .lock()
            .expect("profile spool watcher lock poisoned")
            .take();
        if let Some(watcher) = watcher {
            watcher.stop();
        }
    }

    pub fn scan_pending(&self) -> Result<usize, CommandError> {
        self.scan_pending_with_stop(None)
            .map(|(processed, _)| processed)
    }

    fn scan_pending_with_stop(
        &self,
        stop: Option<&mpsc::Receiver<()>>,
    ) -> Result<(usize, bool), CommandError> {
        fs::create_dir_all(&self.spool_dir).map_err(|_| io_error("profileSpoolDirectory"))?;
        let mut entries =
            fs::read_dir(&self.spool_dir).map_err(|_| io_error("profileSpoolRead"))?;
        let mut processed = 0;
        loop {
            if stop_requested(stop) {
                return Ok((processed, true));
            }
            let mut paths = Vec::with_capacity(MAX_STARTUP_SPOOL_FILES);
            let mut exhausted = false;
            while paths.len() < MAX_STARTUP_SPOOL_FILES {
                match entries.next() {
                    Some(Ok(entry)) => {
                        let path = entry.path();
                        if is_spool_json_path(&self.spool_dir, &path) {
                            paths.push(path);
                        }
                    }
                    Some(Err(_)) => {}
                    None => {
                        exhausted = true;
                        break;
                    }
                }
            }
            paths.sort();
            for path in paths {
                if self.consume_path_safely(&path) {
                    processed += 1;
                }
            }
            if exhausted {
                return Ok((processed, false));
            }
            thread::yield_now();
        }
    }

    fn request_scan(&self) {
        if let Some(watcher) = self
            .watcher
            .lock()
            .expect("profile spool watcher lock poisoned")
            .as_ref()
        {
            watcher.mark_dirty();
        }
    }

    fn begin_install(&self, id: &AgentIntegrationId) -> Result<(), CommandError> {
        let mut installing = self
            .installing
            .lock()
            .expect("profile install marker lock poisoned");
        if !installing.insert(id.as_str().to_string()) {
            return Err(conflict_error("presetInstallInProgress"));
        }
        Ok(())
    }

    pub(crate) fn finish_install(&self, id: &AgentIntegrationId, committed: bool) {
        self.installing
            .lock()
            .expect("profile install marker lock poisoned")
            .remove(id.as_str());
        log::debug!(
            "agent profile preset install transaction finished: profile_id={} committed={committed}",
            id.as_str()
        );
        self.request_scan();
    }

    fn is_installing(&self, id: &AgentIntegrationId) -> bool {
        self.installing
            .lock()
            .expect("profile install marker lock poisoned")
            .contains(id.as_str())
    }

    pub fn install(
        self: &Arc<Self>,
        profile: &StoredAgentIntegrationProfile,
        now: i64,
    ) -> Result<PresetInstallOutcome, CommandError> {
        self.verify_script()?;
        self.ensure_started()?;
        let descriptor = self.descriptor(profile)?;
        recover_rollback_journal(&descriptor)?;
        self.begin_install(&profile.id)?;
        let mutation = match mutate_descriptor(&descriptor, MergeAction::Install, now) {
            Ok(mutation) => mutation,
            Err(error) => {
                self.finish_install(&profile.id, false);
                return Err(error);
            }
        };
        let verification = read_bounded_file(&descriptor.config_path, MAX_PRESET_CONFIG_BYTES)
            .and_then(|written| inspect_descriptor(&descriptor, &written));
        if !matches!(verification, Ok(true)) {
            let error = verification
                .err()
                .unwrap_or_else(|| config_error("presetVerification"));
            let rollback = mutation.rollback();
            self.finish_install(&profile.id, false);
            return Err(rollback.err().unwrap_or(error));
        }
        let installation = AgentProfileInstallation {
            profile_id: profile.id.clone(),
            state: IntegrationState::Installed,
            reason_code: None,
            owned_resource: Some(descriptor.config_path.display().to_string()),
            owned_fingerprint: Some(descriptor_fingerprint(&descriptor)),
            external_hash: Some(owned_state_hash(&descriptor)),
            updated_at: now,
        };
        Ok(PresetInstallOutcome {
            installation,
            mutation,
        })
    }

    pub fn uninstall(
        &self,
        profile: &StoredAgentIntegrationProfile,
        installation: &AgentProfileInstallation,
        now: i64,
    ) -> Result<ConfigMutation, CommandError> {
        let descriptor = self.descriptor(profile)?;
        recover_rollback_journal(&descriptor)?;
        if installation.owned_resource.as_deref()
            != Some(descriptor.config_path.to_string_lossy().as_ref())
            || installation.owned_fingerprint.as_deref()
                != Some(descriptor_fingerprint(&descriptor).as_str())
        {
            return Err(conflict_error("presetReceiptMismatch"));
        }
        let current = read_bounded_file(&descriptor.config_path, MAX_PRESET_CONFIG_BYTES)?;
        if installation.external_hash.as_deref() != Some(owned_state_hash(&descriptor).as_str())
            || !inspect_descriptor(&descriptor, &current)?
        {
            return Err(conflict_error("presetConfigChanged"));
        }
        let mutation = mutate_descriptor(&descriptor, MergeAction::Uninstall, now)?;
        let written = read_bounded_file(&descriptor.config_path, MAX_PRESET_CONFIG_BYTES)?;
        if inspect_descriptor(&descriptor, &written)? {
            let error = config_error("presetRemovalVerification");
            return Err(mutation.rollback().err().unwrap_or(error));
        }
        Ok(mutation)
    }

    pub fn validate_installation(
        &self,
        profile: &StoredAgentIntegrationProfile,
        installation: &AgentProfileInstallation,
    ) -> Result<(), CommandError> {
        self.verify_script()?;
        let descriptor = self.descriptor(profile)?;
        if installation.owned_resource.as_deref()
            != Some(descriptor.config_path.to_string_lossy().as_ref())
            || installation.owned_fingerprint.as_deref()
                != Some(descriptor_fingerprint(&descriptor).as_str())
        {
            return Err(conflict_error("presetReceiptMismatch"));
        }
        let bytes = read_bounded_file(&descriptor.config_path, MAX_PRESET_CONFIG_BYTES)?;
        if installation.external_hash.as_deref() != Some(owned_state_hash(&descriptor).as_str())
            || !inspect_descriptor(&descriptor, &bytes)?
        {
            return Err(conflict_error("presetConfigChanged"));
        }
        Ok(())
    }

    fn verify_script(&self) -> Result<(), CommandError> {
        let installed = read_bounded_file(&self.script_path, 256 * 1024)?;
        if sha256_hex(&installed) != sha256_hex(PROFILE_EVENT_SCRIPT) {
            return Err(io_error("profileEventScriptHashMismatch"));
        }
        Ok(())
    }

    fn recover_pending_rollbacks(&self) -> Result<usize, CommandError> {
        let mut recovered = 0;
        for profile in self.repository.list()? {
            let Ok(descriptor) = self.descriptor(&profile) else {
                continue;
            };
            if recover_rollback_journal(&descriptor)? {
                recovered += 1;
            }
        }
        Ok(recovered)
    }

    fn descriptor(
        &self,
        profile: &StoredAgentIntegrationProfile,
    ) -> Result<PresetDescriptor, CommandError> {
        if profile.kind != AgentIntegrationKind::Preset
            || profile.environment != AgentEnvironment::Windows
        {
            return Err(unsupported_error("profileWslNotSupported"));
        }
        let AgentConfigTarget::Preset { adapter_id } = &profile.config_target else {
            return Err(config_error("presetTargetRequired"));
        };
        let Some(specs) = preset_event_specs(adapter_id) else {
            return Err(unsupported_error("traeHooksVersionOrConfigUnavailable"));
        };
        let expected_mapping = specs
            .iter()
            .map(|spec| crate::contracts::AgentEventMapping {
                native_event: spec.native_event.into(),
                normalized_status: spec.status.clone(),
            })
            .collect::<Vec<_>>();
        if profile.event_mapping != expected_mapping {
            return Err(config_error("presetMappingMismatch"));
        }
        let config_path = match adapter_id {
            PresetAgentAdapterId::Kimi => {
                let desktop_runtime = self
                    .roaming_app_data
                    .join("kimi-desktop/daimon-share/daimon/runtime/kimi-code/config.toml");
                if desktop_runtime.is_file()
                    || desktop_runtime
                        .parent()
                        .is_some_and(|parent| parent.is_dir())
                {
                    desktop_runtime
                } else {
                    self.windows_home.join(".kimi-code").join("config.toml")
                }
            }
            PresetAgentAdapterId::Qoderwork => {
                self.windows_home.join(".qoder").join("settings.json")
            }
            PresetAgentAdapterId::Cursor => self.windows_home.join(".cursor").join("hooks.json"),
            PresetAgentAdapterId::Trae => unreachable!("TRAE has no verified descriptor"),
        };
        let owned_hooks = specs
            .iter()
            .map(|spec| OwnedHookFragment {
                event: spec.native_event.into(),
                command: profile_event_command(
                    &self.script_path,
                    &profile.id,
                    spec.native_event,
                    &self.spool_dir,
                ),
            })
            .collect();
        Ok(PresetDescriptor {
            profile_id: profile.id.clone(),
            adapter_id: adapter_id.clone(),
            config_path,
            owned_hooks,
        })
    }

    fn consume_path_safely(&self, path: &Path) -> bool {
        match self.consume_path(path) {
            Ok(processed) => processed,
            Err(error) => {
                log::warn!(
                    "agent profile spool event rejected: code={:?} message_key={}",
                    error.code,
                    error.message_key
                );
                let _ = fs::remove_file(path);
                true
            }
        }
    }

    fn consume_path(&self, path: &Path) -> Result<bool, CommandError> {
        if !is_spool_json_path(&self.spool_dir, path) || !path.exists() {
            return Ok(false);
        }
        let bytes = read_bounded_file(path, MAX_SPOOL_EVENT_BYTES)?;
        let wire: PresetSpoolEventWire =
            serde_json::from_slice(&bytes).map_err(|_| config_error("spoolEventParse"))?;
        let profile_id = AgentIntegrationId::parse(wire.profile_id)
            .ok_or_else(|| config_error("spoolProfileId"))?;
        if !valid_identifier(&wire.native_event, 64)
            || !valid_identifier(&wire.task_id, 128)
            || !valid_identifier(&wire.source_event_id, 128)
            || wire.occurred_at < 0
        {
            return Err(config_error("spoolEventFields"));
        }
        let profile = self.repository.get(&profile_id)?;
        if profile.kind != AgentIntegrationKind::Preset
            || profile.environment != AgentEnvironment::Windows
        {
            return Err(config_error("spoolProfileKind"));
        }
        let installation = self.repository.get_installation(&profile_id)?;
        if installation
            .as_ref()
            .is_none_or(|receipt| receipt.state != IntegrationState::Installed || !profile.enabled)
        {
            if self.is_installing(&profile_id) {
                return Ok(false);
            }
            fs::remove_file(path).map_err(|_| io_error("profileSpoolRemove"))?;
            return Ok(true);
        }
        let installation = installation.expect("installed receipt checked above");
        let received_at = now_millis();
        if wire.occurred_at < installation.updated_at
            || wire.occurred_at < received_at.saturating_sub(MAX_DURABLE_EVENT_AGE_MILLIS)
            || wire.occurred_at > received_at.saturating_add(5 * 60 * 1000)
        {
            return Err(config_error("spoolEventTime"));
        }
        let status = profile
            .event_mapping
            .iter()
            .find(|mapping| {
                mapping
                    .native_event
                    .eq_ignore_ascii_case(&wire.native_event)
            })
            .map(|mapping| mapping.normalized_status.clone())
            .ok_or_else(|| config_error("spoolEventUnknown"))?;
        let event = ValidatedAgentProfileEvent {
            event_id: wire.source_event_id,
            profile_id,
            native_event: wire.native_event,
            task_id: wire.task_id,
            status,
            occurred_at: wire.occurred_at,
        };
        let outcome = self.repository.project_event_with_reply(
            &event,
            wire.latest_reply_preview.as_deref(),
            received_at,
        )?;
        if outcome == AgentProfileProjectionOutcome::Advanced {
            let _ = self.emitter.emit(
                AGENT_PROFILE_STATE_CHANGED,
                agent_profile_state_changed_payload(&event),
            );
        }
        fs::remove_file(path).map_err(|_| io_error("profileSpoolRemove"))?;
        Ok(true)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PresetSpoolEventWire {
    profile_id: String,
    native_event: String,
    task_id: String,
    latest_reply_preview: Option<String>,
    source_event_id: String,
    occurred_at: i64,
}

fn mutate_descriptor(
    descriptor: &PresetDescriptor,
    action: MergeAction,
    now: i64,
) -> Result<ConfigMutation, CommandError> {
    if let Some(parent) = descriptor.config_path.parent() {
        fs::create_dir_all(parent).map_err(|_| io_error("presetConfigParent"))?;
    }
    let before = read_bounded_optional_file(&descriptor.config_path, MAX_PRESET_CONFIG_BYTES)?;
    let source = before
        .as_deref()
        .unwrap_or_else(|| match descriptor.adapter_id {
            PresetAgentAdapterId::Kimi => b"",
            PresetAgentAdapterId::Qoderwork => b"{}",
            PresetAgentAdapterId::Cursor => br#"{"version":1}"#,
            PresetAgentAdapterId::Trae => unreachable!(),
        });
    let (after, changed) = merge_preset_document(
        &descriptor.adapter_id,
        &descriptor.owned_hooks,
        source,
        action.clone(),
    )?;
    let wrote = changed || before.is_none();
    if wrote {
        if let Some(bytes) = &before {
            write_backup(&descriptor.config_path, bytes, now)?;
        }
        atomic_write(&descriptor.config_path, &after)?;
    }
    Ok(ConfigMutation {
        profile_id: descriptor.profile_id.clone(),
        path: descriptor.config_path.clone(),
        adapter_id: descriptor.adapter_id.clone(),
        owned_hooks: descriptor.owned_hooks.clone(),
        rollback_action: match action {
            MergeAction::Install => MergeAction::Uninstall,
            MergeAction::Uninstall => MergeAction::Install,
        },
        originally_existed: before.is_some(),
        changed: wrote,
    })
}

fn merge_preset_document(
    adapter_id: &PresetAgentAdapterId,
    owned_hooks: &[OwnedHookFragment],
    source: &[u8],
    action: MergeAction,
) -> Result<(Vec<u8>, bool), CommandError> {
    match adapter_id {
        PresetAgentAdapterId::Kimi => merge_kimi(source, owned_hooks, action),
        PresetAgentAdapterId::Qoderwork => {
            merge_config(source, ConfigFormat::JsonHooks, owned_hooks, action)
        }
        PresetAgentAdapterId::Cursor => merge_cursor(source, owned_hooks, action),
        PresetAgentAdapterId::Trae => unreachable!("TRAE has no verified config target"),
    }
}

fn is_empty_preset_document(
    adapter_id: &PresetAgentAdapterId,
    owned_hooks: &[OwnedHookFragment],
    source: &[u8],
) -> Result<bool, CommandError> {
    let empty_source: &[u8] = match adapter_id {
        PresetAgentAdapterId::Kimi => b"",
        PresetAgentAdapterId::Qoderwork => b"{}",
        PresetAgentAdapterId::Cursor => br#"{"version":1}"#,
        PresetAgentAdapterId::Trae => return Ok(false),
    };
    let (installed, _) =
        merge_preset_document(adapter_id, owned_hooks, empty_source, MergeAction::Install)?;
    let (canonical_empty, _) =
        merge_preset_document(adapter_id, owned_hooks, &installed, MergeAction::Uninstall)?;
    Ok(source == canonical_empty)
}

fn inspect_descriptor(descriptor: &PresetDescriptor, bytes: &[u8]) -> Result<bool, CommandError> {
    match descriptor.adapter_id {
        PresetAgentAdapterId::Kimi => inspect_kimi(bytes, &descriptor.owned_hooks),
        PresetAgentAdapterId::Qoderwork => {
            inspect_config(bytes, ConfigFormat::JsonHooks, &descriptor.owned_hooks)
        }
        PresetAgentAdapterId::Cursor => inspect_cursor(bytes, &descriptor.owned_hooks),
        PresetAgentAdapterId::Trae => Ok(false),
    }
}

fn merge_cursor(
    bytes: &[u8],
    owned: &[OwnedHookFragment],
    action: MergeAction,
) -> Result<(Vec<u8>, bool), CommandError> {
    let mut root: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| config_error("cursorHooksParse"))?;
    let object = root
        .as_object_mut()
        .ok_or_else(|| config_error("cursorHooksRoot"))?;
    let mut changed = false;
    if matches!(action, MergeAction::Install) {
        match object.get("version") {
            Some(version) if version.as_u64() == Some(1) => {}
            Some(_) => return Err(config_error("cursorHooksVersion")),
            None => {
                object.insert("version".into(), serde_json::json!(1));
                changed = true;
            }
        }
    }
    let hooks = match action {
        MergeAction::Install => object
            .entry("hooks")
            .or_insert_with(|| serde_json::Value::Object(Default::default())),
        MergeAction::Uninstall => match object.get_mut("hooks") {
            Some(hooks) => hooks,
            None => return Ok((bytes.to_vec(), false)),
        },
    };
    let hooks = hooks
        .as_object_mut()
        .ok_or_else(|| config_error("cursorHooksShape"))?;
    for fragment in owned {
        let entries = match action {
            MergeAction::Install => hooks
                .entry(fragment.event.clone())
                .or_insert_with(|| serde_json::Value::Array(Vec::new())),
            MergeAction::Uninstall => match hooks.get_mut(&fragment.event) {
                Some(entries) => entries,
                None => continue,
            },
        };
        let entries = entries
            .as_array_mut()
            .ok_or_else(|| config_error("cursorHookEventShape"))?;
        if matches!(action, MergeAction::Install) {
            if entries
                .iter()
                .any(|entry| cursor_entry_is_owned(entry, fragment))
            {
                continue;
            }
            if let Some(entry) = entries.iter_mut().find(|entry| {
                entry
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|command| same_profile_script(command, &fragment.command))
            }) {
                *entry = serde_json::json!({"command": fragment.command});
            } else {
                entries.push(serde_json::json!({"command": fragment.command}));
            }
            changed = true;
        } else {
            let before = entries.len();
            entries.retain(|entry| !cursor_entry_is_owned(entry, fragment));
            changed |= entries.len() != before;
        }
    }
    Ok((
        if changed {
            serde_json::to_vec_pretty(&root).map_err(|_| config_error("cursorHooksSerialize"))?
        } else {
            bytes.to_vec()
        },
        changed,
    ))
}

fn inspect_cursor(bytes: &[u8], owned: &[OwnedHookFragment]) -> Result<bool, CommandError> {
    let root: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| config_error("cursorHooksParse"))?;
    if root.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return Ok(false);
    }
    Ok(owned.iter().all(|fragment| {
        root.get("hooks")
            .and_then(serde_json::Value::as_object)
            .and_then(|hooks| hooks.get(&fragment.event))
            .and_then(serde_json::Value::as_array)
            .is_some_and(|entries| {
                entries
                    .iter()
                    .any(|entry| cursor_entry_is_owned(entry, fragment))
            })
    }))
}

fn cursor_entry_is_owned(entry: &serde_json::Value, fragment: &OwnedHookFragment) -> bool {
    entry.as_object().is_some_and(|object| object.len() == 1)
        && entry.get("command").and_then(serde_json::Value::as_str)
            == Some(fragment.command.as_str())
}

fn same_profile_script(candidate: &str, owned: &str) -> bool {
    same_aisland_managed_script(candidate, owned)
}

fn merge_kimi(
    bytes: &[u8],
    owned: &[OwnedHookFragment],
    action: MergeAction,
) -> Result<(Vec<u8>, bool), CommandError> {
    let text = std::str::from_utf8(bytes).map_err(|_| config_error("kimiTomlUtf8"))?;
    let mut document = text
        .parse::<DocumentMut>()
        .map_err(|_| config_error("kimiTomlParse"))?;
    if matches!(action, MergeAction::Uninstall) && document.get("hooks").is_none() {
        return Ok((bytes.to_vec(), false));
    }
    if matches!(action, MergeAction::Install) && inspect_kimi_document(&document, owned) {
        return Ok((bytes.to_vec(), false));
    }
    if document.get("hooks").is_none() {
        document["hooks"] = Item::ArrayOfTables(ArrayOfTables::new());
    }
    let hooks = document["hooks"]
        .as_array_of_tables_mut()
        .ok_or_else(|| config_error("kimiHooksShape"))?;
    let before = hooks.len();
    hooks.retain(|table| {
        !owned.iter().any(|fragment| {
            let same_event =
                table.get("event").and_then(Item::as_str) == Some(fragment.event.as_str());
            let managed_command =
                table
                    .get("command")
                    .and_then(Item::as_str)
                    .is_some_and(|command| {
                        command == fragment.command
                            || (matches!(action, MergeAction::Install)
                                && same_profile_script(command, &fragment.command))
                    });
            same_event && managed_command
        })
    });
    if matches!(action, MergeAction::Install) {
        for fragment in owned {
            let mut table = Table::new();
            table["event"] = value(&fragment.event);
            table["command"] = value(&fragment.command);
            table["timeout"] = value(5);
            hooks.push(table);
        }
    }
    let changed = hooks.len() != before || matches!(action, MergeAction::Install);
    Ok((document.to_string().into_bytes(), changed))
}

fn inspect_kimi(bytes: &[u8], owned: &[OwnedHookFragment]) -> Result<bool, CommandError> {
    let text = std::str::from_utf8(bytes).map_err(|_| config_error("kimiTomlUtf8"))?;
    let document = text
        .parse::<DocumentMut>()
        .map_err(|_| config_error("kimiTomlParse"))?;
    Ok(inspect_kimi_document(&document, owned))
}

fn inspect_kimi_document(document: &DocumentMut, owned: &[OwnedHookFragment]) -> bool {
    let Some(hooks) = document.get("hooks").and_then(Item::as_array_of_tables) else {
        return false;
    };
    owned.iter().all(|fragment| {
        hooks.iter().any(|table| {
            table.get("event").and_then(Item::as_str) == Some(fragment.event.as_str())
                && table.get("command").and_then(Item::as_str) == Some(fragment.command.as_str())
                && table.get("timeout").and_then(Item::as_integer) == Some(5)
        })
    })
}

fn descriptor_fingerprint(descriptor: &PresetDescriptor) -> String {
    let mut material = vec![
        descriptor.profile_id.as_str().to_owned(),
        descriptor.config_path.display().to_string(),
        sha256_hex(PROFILE_EVENT_SCRIPT),
    ];
    material.extend(
        descriptor
            .owned_hooks
            .iter()
            .map(|hook| format!("{}\n{}", hook.event, hook.command)),
    );
    sha256_hex(material.join("\n").as_bytes())
}

fn owned_state_hash(descriptor: &PresetDescriptor) -> String {
    let material = descriptor
        .owned_hooks
        .iter()
        .map(|hook| format!("{}\n{}", hook.event, hook.command))
        .collect::<Vec<_>>()
        .join("\n");
    sha256_hex(material.as_bytes())
}

fn profile_event_command(
    script_path: &Path,
    profile_id: &AgentIntegrationId,
    native_event: &str,
    spool_dir: &Path,
) -> String {
    format!(
        "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File {} -ProfileId {} -NativeEvent {} -SpoolDirectory {}",
        windows_quote(&script_path.to_string_lossy()),
        windows_quote(profile_id.as_str()),
        windows_quote(native_event),
        windows_quote(&spool_dir.to_string_lossy()),
    )
}

fn windows_quote(value: &str) -> String {
    let mut quoted = String::from("\"");
    let mut slashes = 0usize;
    for character in value.chars() {
        if character == '\\' {
            slashes += 1;
        } else if character == '"' {
            quoted.push_str(&"\\".repeat(slashes * 2 + 1));
            quoted.push(character);
            slashes = 0;
        } else {
            quoted.push_str(&"\\".repeat(slashes));
            quoted.push(character);
            slashes = 0;
        }
    }
    quoted.push_str(&"\\".repeat(slashes * 2));
    quoted.push('"');
    quoted
}

fn write_backup(path: &Path, bytes: &[u8], now: i64) -> Result<(), CommandError> {
    let backup = PathBuf::from(format!(
        "{}.aisland-backup-{now:019}-{}",
        path.display(),
        uuid::Uuid::new_v4().simple()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(backup)
        .map_err(|_| io_error("presetBackupCreate"))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| io_error("presetBackupWrite"))?;
    prune_backups(path)
}

fn prune_backups(path: &Path) -> Result<(), CommandError> {
    let parent = path
        .parent()
        .ok_or_else(|| io_error("presetConfigParent"))?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| io_error("presetConfigName"))?;
    let prefix = format!("{file_name}.aisland-backup-");
    let mut backups = fs::read_dir(parent)
        .map_err(|_| io_error("presetBackupRead"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|candidate| {
            candidate
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .collect::<Vec<_>>();
    backups.sort();
    let excess = backups.len().saturating_sub(MAX_BACKUPS_PER_CONFIG);
    for backup in backups.into_iter().take(excess) {
        fs::remove_file(backup).map_err(|_| io_error("presetBackupPrune"))?;
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), CommandError> {
    let parent = path
        .parent()
        .ok_or_else(|| io_error("presetConfigParent"))?;
    fs::create_dir_all(parent).map_err(|_| io_error("presetConfigParent"))?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| io_error("presetConfigName"))?;
    let temporary = parent.join(format!(
        ".{name}.aisland-tmp-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|_| io_error("presetTempCreate"))?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        let _ = error;
        return Err(io_error("presetTempWrite"));
    }
    drop(file);
    if let Err(error) = replace_file(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

#[cfg(windows)]
fn replace_file(temporary: &Path, path: &Path) -> Result<(), CommandError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let temporary = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    unsafe {
        MoveFileExW(
            PCWSTR(temporary.as_ptr()),
            PCWSTR(path.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|_| io_error("presetReplace"))
    }
}

#[cfg(windows)]
fn move_file_if_absent(temporary: &Path, path: &Path) -> Result<(), CommandError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
    let temporary = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    unsafe {
        MoveFileExW(
            PCWSTR(temporary.as_ptr()),
            PCWSTR(path.as_ptr()),
            MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|_| io_error("presetRollbackMove"))
    }
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, path: &Path) -> Result<(), CommandError> {
    fs::rename(temporary, path).map_err(|_| io_error("presetReplace"))
}

#[cfg(not(windows))]
fn move_file_if_absent(_temporary: &Path, _path: &Path) -> Result<(), CommandError> {
    Err(conflict_error("presetRollbackUnsupported"))
}

fn read_bounded_file(path: &Path, limit: u64) -> Result<Vec<u8>, CommandError> {
    let file = File::open(path).map_err(|_| io_error("profileFileRead"))?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| io_error("profileFileRead"))?;
    if bytes.len() as u64 > limit {
        return Err(config_error("profileConfigSizeExceeded"));
    }
    Ok(bytes)
}

fn read_bounded_optional_file(path: &Path, limit: u64) -> Result<Option<Vec<u8>>, CommandError> {
    match File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(limit + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| io_error("profileFileRead"))?;
            if bytes.len() as u64 > limit {
                return Err(config_error("profileConfigSizeExceeded"));
            }
            Ok(Some(bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(io_error("presetConfigRead")),
    }
}

fn is_spool_json_path(root: &Path, path: &Path) -> bool {
    path.parent() == Some(root)
        && path.extension().and_then(|value| value.to_str()) == Some("json")
        && path
            .file_stem()
            .and_then(|value| value.to_str())
            .is_some_and(|value| {
                value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
}

fn valid_identifier(value: &str, max_chars: usize) -> bool {
    value.trim() == value
        && (1..=max_chars).contains(&value.chars().count())
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':' | '@')
        })
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn io_error(reason: &str) -> CommandError {
    CommandError::with_detail(
        AppErrorCode::IoFailure,
        "errors.ioFailure",
        "reasonCode",
        SafeParameterValue::String(reason.into()),
        false,
    )
}

fn config_error(reason: &str) -> CommandError {
    CommandError::with_detail(
        AppErrorCode::IntegrationConfigInvalid,
        "errors.integrationConfigInvalid",
        "reasonCode",
        SafeParameterValue::String(reason.into()),
        false,
    )
}

fn conflict_error(reason: &str) -> CommandError {
    CommandError::with_detail(
        AppErrorCode::Conflict,
        "errors.conflict",
        "reasonCode",
        SafeParameterValue::String(reason.into()),
        false,
    )
}

fn unsupported_error(reason: &str) -> CommandError {
    CommandError::with_detail(
        AppErrorCode::IntegrationUnsupported,
        "errors.integrationUnsupported",
        "reasonCode",
        SafeParameterValue::String(reason.into()),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Storage;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct RecordingEmitter {
        calls: AtomicUsize,
    }

    impl EventEmitterPort for RecordingEmitter {
        fn emit(
            &self,
            event_name: &'static str,
            _payload: serde_json::Value,
        ) -> Result<(), CommandError> {
            assert_eq!(event_name, AGENT_PROFILE_STATE_CHANGED);
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    fn repository(root: &Path) -> AgentProfileRepository {
        AgentProfileRepository::new(Arc::new(Storage::open(root).unwrap()))
    }

    fn bridge(
        root: &Path,
        repository: AgentProfileRepository,
        emitter: Arc<dyn EventEmitterPort>,
    ) -> Arc<PresetProfileBridge> {
        let bridge = Arc::new(PresetProfileBridge::new(
            repository,
            emitter,
            root.join("home"),
            root.join("data"),
        ));
        fs::create_dir_all(bridge.script_path.parent().unwrap()).unwrap();
        fs::write(&bridge.script_path, PROFILE_EVENT_SCRIPT).unwrap();
        bridge
    }

    fn descriptor(root: &Path, adapter_id: PresetAgentAdapterId) -> PresetDescriptor {
        PresetDescriptor {
            profile_id: AgentIntegrationId::parse(match adapter_id {
                PresetAgentAdapterId::Kimi => "kimi-windows",
                PresetAgentAdapterId::Qoderwork => "qoderwork-windows",
                PresetAgentAdapterId::Cursor => "cursor-windows",
                PresetAgentAdapterId::Trae => "trae-windows",
            })
            .unwrap(),
            adapter_id,
            config_path: root.join("settings.conf"),
            owned_hooks: vec![OwnedHookFragment {
                event: "Stop".into(),
                command: "powershell.exe -File C:\\AIsland\\profile-event.ps1".into(),
            }],
        }
    }

    fn assert_no_rollback_artifacts(paths: &RollbackPaths, phase: &str) {
        assert!(!paths.journal.exists(), "journal remained after {phase}");
        assert!(
            !paths.candidate.exists(),
            "candidate remained after {phase}"
        );
        assert!(
            !paths.displaced.exists(),
            "displaced remained after {phase}"
        );
        assert!(!paths.rescue.exists(), "rescue remained after {phase}");
        assert!(
            !paths.rollover_rescue.exists(),
            "rollover rescue remained after {phase}"
        );
    }

    #[test]
    fn kimi_attention_events_are_not_presented_as_idle_or_running() {
        let specs = preset_event_specs(&PresetAgentAdapterId::Kimi).unwrap();
        let status_for = |event: &str| {
            specs
                .iter()
                .find(|spec| spec.native_event == event)
                .map(|spec| spec.status.clone())
        };

        assert_eq!(status_for("PermissionRequest"), Some(AgentStatus::Waiting));
        assert_eq!(status_for("Interrupt"), Some(AgentStatus::Failed));
    }

    #[test]
    fn cursor_install_targets_the_exact_tauri_app_data_directory() {
        let root = tempfile::tempdir().unwrap();
        let repository = repository(&root.path().join("db"));
        let emitter = Arc::new(RecordingEmitter::default());
        let app_data_dir = root.path().join("data/com.aisland.app");
        let bridge = Arc::new(PresetProfileBridge::new(
            repository.clone(),
            emitter,
            root.path().join("home"),
            app_data_dir.clone(),
        ));
        fs::create_dir_all(bridge.script_path.parent().unwrap()).unwrap();
        fs::write(&bridge.script_path, PROFILE_EVENT_SCRIPT).unwrap();
        let id = AgentIntegrationId::parse("cursor-windows").unwrap();
        let profile = repository.get(&id).unwrap();

        let outcome = bridge.install(&profile, now_millis()).unwrap();
        let installed: serde_json::Value =
            serde_json::from_slice(&fs::read(root.path().join("home/.cursor/hooks.json")).unwrap())
                .unwrap();
        let command = installed["hooks"]["afterAgentResponse"][0]["command"]
            .as_str()
            .unwrap();

        assert!(
            command.contains(
                app_data_dir
                    .join("agent-hooks")
                    .join("aisland-profile-event-windows.ps1")
                    .to_string_lossy()
                    .as_ref()
            ),
            "expected exact script path in {command}"
        );
        assert!(
            command.contains(
                app_data_dir
                    .join("agent-profile-events")
                    .to_string_lossy()
                    .as_ref()
            ),
            "expected exact spool path in {command}"
        );
        assert!(!command.contains("com.aisland.app\\com.aisland"));
        outcome.mutation.rollback().unwrap();
        bridge.finish_install(&id, false);
    }

    #[test]
    fn cursor_hooks_use_the_official_flat_shape_and_remove_only_owned_entries() {
        let owned = vec![OwnedHookFragment {
            event: "afterAgentResponse".into(),
            command:
                "powershell.exe -File C:\\AIsland\\profile-event.ps1 -ProfileId cursor-windows"
                    .into(),
        }];
        let source = br#"{"version":1,"hooks":{"afterAgentResponse":[{"command":"user-command"}]},"vendor":{"keep":true}}"#;

        let (installed, changed) = merge_cursor(source, &owned, MergeAction::Install).unwrap();
        assert!(changed);
        assert!(inspect_cursor(&installed, &owned).unwrap());
        let parsed: serde_json::Value = serde_json::from_slice(&installed).unwrap();
        let entries = parsed["hooks"]["afterAgentResponse"].as_array().unwrap();
        assert_eq!(entries[0], serde_json::json!({"command": "user-command"}));
        assert_eq!(entries[1], serde_json::json!({"command": owned[0].command}));
        assert_eq!(parsed["vendor"]["keep"], true);

        let (removed, changed) = merge_cursor(&installed, &owned, MergeAction::Uninstall).unwrap();
        assert!(changed);
        let removed: serde_json::Value = serde_json::from_slice(&removed).unwrap();
        assert_eq!(
            removed["hooks"]["afterAgentResponse"],
            serde_json::json!([{"command": "user-command"}])
        );
        assert_eq!(removed["vendor"]["keep"], true);
    }

    #[test]
    fn preset_repair_migrates_the_legacy_aisland_app_data_path_in_place() {
        let current = r#"powershell.exe -File "C:\Users\Alice\AppData\Roaming\com.aisland.app\agent-hooks\aisland-profile-event-windows.ps1" -ProfileId cursor-windows"#;
        let legacy = r#"powershell.exe -File "C:\Users\Alice\AppData\Roaming\com.aisland\agent-hooks\aisland-profile-event-windows.ps1" -ProfileId cursor-windows"#;
        let owned = vec![OwnedHookFragment {
            event: "afterAgentResponse".into(),
            command: current.into(),
        }];
        let cursor_source = serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "hooks": {
                "afterAgentResponse": [
                    {"command": legacy},
                    {"command": "user-command"}
                ]
            }
        }))
        .unwrap();

        let (cursor_repaired, changed) =
            merge_cursor(&cursor_source, &owned, MergeAction::Install).unwrap();
        assert!(changed);
        let cursor: serde_json::Value = serde_json::from_slice(&cursor_repaired).unwrap();
        let commands = cursor["hooks"]["afterAgentResponse"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|entry| entry["command"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(commands, vec![current, "user-command"]);

        let kimi_source = format!(
            "[[hooks]]\nevent = \"afterAgentResponse\"\ncommand = '{}'\ntimeout = 5\n\n[[hooks]]\nevent = \"afterAgentResponse\"\ncommand = \"user-command\"\ntimeout = 8\n",
            legacy
        );
        let (kimi_repaired, changed) =
            merge_kimi(kimi_source.as_bytes(), &owned, MergeAction::Install).unwrap();
        assert!(changed);
        let kimi_text = String::from_utf8(kimi_repaired).unwrap();
        assert!(kimi_text.contains(current));
        assert!(!kimi_text.contains(legacy));
        assert!(kimi_text.contains("user-command"));
    }

    #[test]
    fn preset_config_is_bounded_before_backup_parse_or_write() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        let oversized = vec![b'x'; (MAX_PRESET_CONFIG_BYTES + 1) as usize];
        fs::write(&descriptor.config_path, &oversized).unwrap();

        let error = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap_err();

        assert_eq!(error.code, AppErrorCode::IntegrationConfigInvalid);
        assert_eq!(fs::read(&descriptor.config_path).unwrap(), oversized);
        assert_eq!(
            fs::read_dir(root.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .contains("aisland-backup"))
                .count(),
            0
        );
    }

    #[test]
    fn kimi_desktop_install_targets_its_effective_runtime_config() {
        let root = tempfile::tempdir().unwrap();
        let repository = repository(&root.path().join("db"));
        let app_data_dir = root.path().join("roaming/com.aisland.app");
        let runtime_config = root
            .path()
            .join("roaming/kimi-desktop/daimon-share/daimon/runtime/kimi-code/config.toml");
        fs::create_dir_all(runtime_config.parent().unwrap()).unwrap();
        fs::write(&runtime_config, b"").unwrap();
        let bridge = Arc::new(PresetProfileBridge::new(
            repository.clone(),
            Arc::new(RecordingEmitter::default()),
            root.path().join("home"),
            app_data_dir,
        ));
        fs::create_dir_all(bridge.script_path.parent().unwrap()).unwrap();
        fs::write(&bridge.script_path, PROFILE_EVENT_SCRIPT).unwrap();
        let profile = repository
            .get(&AgentIntegrationId::parse("kimi-windows").unwrap())
            .unwrap();

        let outcome = bridge.install(&profile, now_millis()).unwrap();

        assert_eq!(
            outcome
                .installation
                .owned_resource
                .as_deref()
                .map(Path::new),
            Some(runtime_config.as_path())
        );
        assert!(fs::read_to_string(runtime_config)
            .unwrap()
            .contains("aisland-profile-event-windows.ps1"));
    }

    #[test]
    fn qoderwork_install_targets_official_qoder_settings_directory() {
        let root = tempfile::tempdir().unwrap();
        let repository = repository(&root.path().join("db"));
        let bridge = bridge(
            root.path(),
            repository.clone(),
            Arc::new(RecordingEmitter::default()),
        );
        let profile = repository
            .get(&AgentIntegrationId::parse("qoderwork-windows").unwrap())
            .unwrap();

        let outcome = bridge.install(&profile, now_millis()).unwrap();
        let expected = root.path().join("home/.qoder/settings.json");

        assert_eq!(
            outcome
                .installation
                .owned_resource
                .as_deref()
                .map(Path::new),
            Some(expected.as_path())
        );
        assert!(expected.is_file());
        assert!(!root.path().join("home/.qoderwork/settings.json").exists());
    }

    #[test]
    fn rollback_preserves_a_vendor_replacement_that_already_removed_owned_hooks() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let vendor_replacement = b"api_key = \"rotated\"\n[unrelated]\nenabled = true\n";
        fs::write(&descriptor.config_path, vendor_replacement).unwrap();

        mutation.rollback().unwrap();

        assert_eq!(
            fs::read(&descriptor.config_path).unwrap(),
            vendor_replacement
        );
    }

    #[test]
    fn rollback_preserves_a_vendor_replacement_completed_before_the_lock() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let mut vendor_replacement = fs::read(&descriptor.config_path).unwrap();
        vendor_replacement.extend_from_slice(b"\n[vendor_edit]\nkeep = \"yes\"\n");
        let path = descriptor.config_path.clone();

        mutation
            .rollback_with_observer(move |attempt, phase| {
                if attempt == 0 && phase == RollbackPhase::BeforeLock {
                    fs::write(&path, &vendor_replacement).unwrap();
                }
            })
            .unwrap();

        let rolled_back = fs::read_to_string(&descriptor.config_path).unwrap();
        assert!(rolled_back.contains("api_key = \"secret\""));
        assert!(rolled_back.contains("[vendor_edit]"));
        assert!(rolled_back.contains("keep = \"yes\""));
        assert!(!rolled_back.contains("powershell.exe"));
    }

    #[test]
    fn rollback_exclusive_commit_window_rejects_a_vendor_write() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let mut vendor_replacement = fs::read(&descriptor.config_path).unwrap();
        vendor_replacement.extend_from_slice(b"\n[commit_window_edit]\nkeep = \"yes\"\n");
        let path = descriptor.config_path.clone();
        let write_was_blocked = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed_block = Arc::clone(&write_was_blocked);

        mutation
            .rollback_with_observer(move |attempt, phase| {
                if attempt == 0 && phase == RollbackPhase::AfterLockBeforeCommit {
                    observed_block.store(
                        fs::write(&path, &vendor_replacement).is_err(),
                        std::sync::atomic::Ordering::SeqCst,
                    );
                }
            })
            .unwrap();

        let rolled_back = fs::read_to_string(&descriptor.config_path).unwrap();
        assert!(write_was_blocked.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!rolled_back.contains("[commit_window_edit]"));
        assert!(!rolled_back.contains("powershell.exe"));
    }

    #[test]
    fn rollback_never_overwrites_vendor_v2_written_after_atomic_replace() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let path = descriptor.config_path.clone();
        let mut vendor_v1 = fs::read(&descriptor.config_path).unwrap();
        vendor_v1.extend_from_slice(b"\n[vendor]\nversion = 1\n");
        let vendor_v2 = b"api_key = \"vendor-v2\"\n[vendor]\nversion = 2\n".to_vec();
        let injected_vendor_v2 = vendor_v2.clone();

        let mut injected_v1 = false;
        let mut injected_v2 = false;
        let result = mutation.rollback_with_observer(move |_, phase| match phase {
            RollbackPhase::AfterCandidatePreparedBeforeReplace if !injected_v1 => {
                fs::write(&path, &vendor_v1).unwrap();
                injected_v1 = true;
            }
            RollbackPhase::AfterReplaceBeforeRecovery if !injected_v2 => {
                fs::write(&path, &injected_vendor_v2).unwrap();
                injected_v2 = true;
            }
            _ => {}
        });

        result.unwrap();
        assert_eq!(fs::read(&descriptor.config_path).unwrap(), vendor_v2);
    }

    #[cfg(windows)]
    #[test]
    fn rollback_rebases_v3_without_wedging_the_displaced_slot() {
        for replace_identity in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
            fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
            let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
            let mut vendor_v3 = fs::read(&descriptor.config_path).unwrap();
            vendor_v3.extend_from_slice(b"\n[vendor_v3]\nkeep = \"yes\"\n");
            let injected = vendor_v3.clone();
            let target = descriptor.config_path.clone();
            let mut wrote_v3 = false;

            mutation
                .rollback_with_observer(move |_, phase| {
                    if phase == RollbackPhase::AfterReplaceBeforeRecovery && !wrote_v3 {
                        if replace_identity {
                            atomic_write(&target, &injected).unwrap();
                        } else {
                            fs::write(&target, &injected).unwrap();
                        }
                        wrote_v3 = true;
                    }
                })
                .unwrap();

            let recovered = fs::read_to_string(&descriptor.config_path).unwrap();
            assert!(recovered.contains("[vendor_v3]"));
            assert!(recovered.contains("keep = \"yes\""));
            assert!(!recovered.contains("powershell.exe"));
            assert!(!recover_rollback_journal(&descriptor).unwrap());
            assert!(!recover_rollback_journal(&descriptor).unwrap());
            let paths = rollback_paths(&descriptor.config_path).unwrap();
            assert!(!paths.journal.exists());
            assert!(!paths.candidate.exists());
            assert!(!paths.displaced.exists());
            assert!(!paths.rescue.exists());
        }
    }

    #[cfg(windows)]
    #[test]
    fn rollback_rebases_a_second_vendor_generation_after_rescue_is_occupied() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let installed = fs::read(&descriptor.config_path).unwrap();
        let target = descriptor.config_path.clone();
        let mut generation = 0;

        let result = mutation.rollback_with_observer(move |_, phase| {
            if phase != RollbackPhase::AfterReplaceBeforeRecovery {
                return;
            }
            generation += 1;
            if generation == 1 {
                let mut vendor_v2 = installed.clone();
                vendor_v2.extend_from_slice(b"\n[vendor_v2]\nkeep = \"yes\"\n");
                atomic_write(&target, &vendor_v2).unwrap();
            } else if generation == 2 {
                let mut vendor_v3 = fs::read(&target).unwrap();
                vendor_v3.extend_from_slice(b"\n[vendor_v3]\nkeep = \"yes\"\n");
                atomic_write(&target, &vendor_v3).unwrap();
            }
        });

        result.unwrap();
        let recovered = fs::read_to_string(&descriptor.config_path).unwrap();
        assert!(recovered.contains("[vendor_v2]"));
        assert!(recovered.contains("[vendor_v3]"));
        assert!(!recovered.contains("powershell.exe"));
        assert!(!recover_rollback_journal(&descriptor).unwrap());
        assert!(!recover_rollback_journal(&descriptor).unwrap());
        let paths = rollback_paths(&descriptor.config_path).unwrap();
        assert!(!paths.journal.exists());
        assert!(!paths.candidate.exists());
        assert!(!paths.displaced.exists());
        assert!(!paths.rescue.exists());
    }

    #[cfg(windows)]
    #[test]
    fn rollover_validates_busy_next_generation_before_preserved_rescue_cleanup() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let installed = fs::read(&descriptor.config_path).unwrap();
        let target = descriptor.config_path.clone();
        let paths = rollback_paths(&target).unwrap();
        let observed_paths = paths.clone();
        let writer = Arc::new(Mutex::new(None::<File>));
        let observed_writer = Arc::clone(&writer);
        let mut generation = 0;

        let result = mutation.rollback_with_observer(move |_, phase| {
            if phase == RollbackPhase::AfterReplaceBeforeRecovery {
                generation += 1;
                if generation == 1 {
                    let mut vendor_v2 = installed.clone();
                    vendor_v2.extend_from_slice(b"\n[vendor_v2]\nkeep = \"yes\"\n");
                    atomic_write(&target, &vendor_v2).unwrap();
                } else if generation == 2 {
                    let mut vendor_v3 = fs::read(&target).unwrap();
                    vendor_v3.extend_from_slice(b"\n[vendor_v3]\nkeep = \"yes\"\n");
                    atomic_write(&target, &vendor_v3).unwrap();
                }
            }
            if phase == RollbackPhase::AfterRolloverJournalBeforeNormalize
                && observed_writer.lock().unwrap().is_none()
            {
                let mut file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0)
                    .open(&observed_paths.displaced)
                    .unwrap();
                file.seek(SeekFrom::End(0)).unwrap();
                file.write_all(b"\n[vendor_late]\nkeep = true\n").unwrap();
                file.sync_all().unwrap();
                *observed_writer.lock().unwrap() = Some(file);
            }
        });

        assert_eq!(result.unwrap_err().code, AppErrorCode::Conflict);
        assert!(paths.rescue.exists());
        assert!(paths.displaced.exists());
        assert!(paths.journal.exists());
        drop(writer.lock().unwrap().take());
        assert_eq!(
            recover_rollback_journal(&descriptor).unwrap_err().code,
            AppErrorCode::Conflict
        );
        assert!(paths.rescue.exists());
        assert!(paths.displaced.exists());
    }

    #[cfg(windows)]
    #[test]
    fn close_to_replace_rebases_same_identity_and_preserves_unknown_new_identity() {
        for replace_identity in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
            fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
            let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
            let mut vendor_v2 = fs::read(&descriptor.config_path).unwrap();
            vendor_v2.extend_from_slice(b"\n[vendor_v2]\nkeep = \"yes\"\n");
            let injected = vendor_v2.clone();
            let target = descriptor.config_path.clone();
            let mut wrote_v2 = false;

            let result = mutation.rollback_with_observer(move |_, phase| {
                if phase == RollbackPhase::AfterPreflightBeforeReplace && !wrote_v2 {
                    if replace_identity {
                        atomic_write(&target, &injected).unwrap();
                    } else {
                        fs::write(&target, &injected).unwrap();
                    }
                    wrote_v2 = true;
                }
            });

            if replace_identity {
                assert_eq!(result.unwrap_err().code, AppErrorCode::Conflict);
                let paths = rollback_paths(&descriptor.config_path).unwrap();
                assert_eq!(fs::read(&paths.displaced).unwrap(), vendor_v2);
                assert!(paths.journal.exists());
                continue;
            }
            result.unwrap();

            let recovered = fs::read_to_string(&descriptor.config_path).unwrap();
            assert!(recovered.contains("[vendor_v2]"));
            assert!(recovered.contains("keep = \"yes\""));
            assert!(!recovered.contains("powershell.exe"));
            assert!(!recover_rollback_journal(&descriptor).unwrap());
            let paths = rollback_paths(&descriptor.config_path).unwrap();
            assert!(!paths.journal.exists());
            assert!(!paths.candidate.exists());
            assert!(!paths.displaced.exists());
            assert!(!paths.rescue.exists());
        }
    }

    #[test]
    fn recovery_preserves_displaced_vendor_bytes_when_the_journal_is_missing() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        let paths = rollback_paths(&descriptor.config_path).unwrap();
        let vendor = b"api_key = \"vendor-rescue\"\n";
        fs::write(&paths.candidate, b"aisland-candidate").unwrap();
        fs::write(&paths.displaced, vendor).unwrap();

        let error = recover_rollback_journal(&descriptor).unwrap_err();

        assert_eq!(error.code, AppErrorCode::Conflict);
        assert_eq!(fs::read(&paths.displaced).unwrap(), vendor);
        assert_eq!(fs::read(&paths.candidate).unwrap(), b"aisland-candidate");
    }

    #[test]
    fn recovery_without_a_journal_preserves_a_lone_unknown_candidate() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        let paths = rollback_paths(&descriptor.config_path).unwrap();
        let unknown = b"complete external candidate";
        fs::write(&paths.candidate, unknown).unwrap();

        let error = recover_rollback_journal(&descriptor).unwrap_err();

        assert_eq!(error.code, AppErrorCode::Conflict);
        assert_eq!(fs::read(&paths.candidate).unwrap(), unknown);
    }

    #[test]
    fn corrupt_rollback_journal_is_fail_closed_without_touching_vendor_content() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        let paths = rollback_paths(&descriptor.config_path).unwrap();
        let vendor = b"api_key = \"vendor-secret\"\n";
        fs::write(&descriptor.config_path, vendor).unwrap();
        fs::write(&paths.journal, b"{not-a-valid-journal").unwrap();

        let error = recover_rollback_journal(&descriptor).unwrap_err();

        assert_eq!(error.code, AppErrorCode::Conflict);
        assert_eq!(fs::read(&descriptor.config_path).unwrap(), vendor);
        assert_eq!(fs::read(&paths.journal).unwrap(), b"{not-a-valid-journal");
    }

    #[test]
    fn unknown_rollback_journal_version_is_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            mutation
                .rollback_with_observer(|_, phase| {
                    if phase == RollbackPhase::AfterJournalPrepared {
                        panic!("leave a complete journal for version mutation");
                    }
                })
                .unwrap();
        }));
        assert!(interrupted.is_err());
        let paths = rollback_paths(&descriptor.config_path).unwrap();
        let mut journal: serde_json::Value =
            serde_json::from_slice(&fs::read(&paths.journal).unwrap()).unwrap();
        journal["version"] = serde_json::json!(ROLLBACK_JOURNAL_VERSION + 1);
        fs::write(&paths.journal, serde_json::to_vec(&journal).unwrap()).unwrap();
        let before = fs::read(&descriptor.config_path).unwrap();

        let error = recover_rollback_journal(&descriptor).unwrap_err();

        assert_eq!(error.code, AppErrorCode::Conflict);
        assert_eq!(fs::read(&descriptor.config_path).unwrap(), before);
        assert!(paths.journal.exists());
    }

    #[test]
    fn malformed_rollback_hash_is_fail_closed_before_any_config_mutation() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            mutation
                .rollback_with_observer(|_, phase| {
                    if phase == RollbackPhase::AfterJournalPrepared {
                        panic!("leave a complete journal for hash mutation");
                    }
                })
                .unwrap();
        }));
        assert!(interrupted.is_err());
        let paths = rollback_paths(&descriptor.config_path).unwrap();
        let mut journal: serde_json::Value =
            serde_json::from_slice(&fs::read(&paths.journal).unwrap()).unwrap();
        journal["expectedHash"] = serde_json::json!("not-a-sha256");
        fs::write(&paths.journal, serde_json::to_vec(&journal).unwrap()).unwrap();
        let before = fs::read(&descriptor.config_path).unwrap();

        let error = recover_rollback_journal(&descriptor).unwrap_err();

        assert_eq!(error.code, AppErrorCode::Conflict);
        assert_eq!(fs::read(&descriptor.config_path).unwrap(), before);
        assert!(paths.journal.exists());
    }

    #[test]
    fn rollback_journal_is_durable_before_the_candidate_is_materialized() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let paths = rollback_paths(&descriptor.config_path).unwrap();
        let observed_paths = paths.clone();
        let journal_preceded_candidate = Arc::new(AtomicBool::new(false));
        let observed_order = Arc::clone(&journal_preceded_candidate);
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            mutation
                .rollback_with_observer(move |_, phase| {
                    if phase == RollbackPhase::AfterJournalPrepared {
                        observed_order.store(
                            observed_paths.journal.exists() && !observed_paths.candidate.exists(),
                            Ordering::SeqCst,
                        );
                        panic!("simulate termination after the journal commit");
                    }
                })
                .unwrap();
        }));

        assert!(interrupted.is_err());
        assert!(journal_preceded_candidate.load(Ordering::SeqCst));
        let recovery = recover_rollback_journal(&descriptor);
        assert!(
            recovery.is_ok(),
            "recovery={recovery:?}, target={}, candidate={}, displaced={}, rescue={}",
            descriptor.config_path.exists(),
            paths.candidate.exists(),
            paths.displaced.exists(),
            paths.rescue.exists()
        );
        assert!(recovery.unwrap());
        assert_eq!(
            fs::read(&descriptor.config_path).unwrap(),
            b"api_key = \"secret\"\n"
        );
    }

    #[test]
    fn delete_intent_preserves_a_vendor_recreated_empty_config() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let path = descriptor.config_path.clone();
        let replaced = Arc::new(AtomicBool::new(false));
        let replacement_observed = Arc::clone(&replaced);

        mutation
            .rollback_with_observer(move |_, phase| {
                if phase == RollbackPhase::AfterDeleteIntentPrepared
                    && !replacement_observed.swap(true, Ordering::SeqCst)
                {
                    fs::remove_file(&path).unwrap();
                    fs::write(&path, b"").unwrap();
                }
            })
            .unwrap();

        assert!(replaced.load(Ordering::SeqCst));
        assert!(descriptor.config_path.exists());
        assert_eq!(fs::read(&descriptor.config_path).unwrap(), b"");
    }

    #[cfg(windows)]
    #[test]
    fn candidate_tampering_is_preserved_when_target_rebase_discards_owned_candidate() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let paths = rollback_paths(&descriptor.config_path).unwrap();
        let target = descriptor.config_path.clone();
        let mut tampered = false;

        let result = mutation.rollback_with_observer(move |_, phase| {
            if phase == RollbackPhase::AfterCandidatePreparedBeforeReplace && !tampered {
                fs::write(&paths.candidate, b"untrusted candidate bytes").unwrap();
                let mut vendor_v2 = fs::read(&target).unwrap();
                vendor_v2.extend_from_slice(b"\n[vendor_v2]\nkeep = true\n");
                fs::write(&target, vendor_v2).unwrap();
                tampered = true;
            }
        });

        assert_eq!(result.unwrap_err().code, AppErrorCode::Conflict);
        assert_eq!(
            fs::read(rollback_paths(&descriptor.config_path).unwrap().candidate).unwrap(),
            b"untrusted candidate bytes"
        );
        assert!(fs::read_to_string(&descriptor.config_path)
            .unwrap()
            .contains("vendor_v2"));
    }

    #[cfg(windows)]
    #[test]
    fn active_vendor_writer_is_not_displaced_and_later_updates_are_recovered() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let path = descriptor.config_path.clone();
        let writer = Arc::new(Mutex::new(None::<File>));
        let observed_writer = Arc::clone(&writer);

        let result = mutation.rollback_with_observer(move |_, phase| {
            if phase == RollbackPhase::AfterCandidatePreparedBeforeReplace
                && observed_writer.lock().unwrap().is_none()
            {
                let mut file = OpenOptions::new()
                    .append(true)
                    .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0)
                    .open(&path)
                    .unwrap();
                file.write_all(b"\n[vendor]\nbefore = true\n").unwrap();
                file.sync_all().unwrap();
                *observed_writer.lock().unwrap() = Some(file);
            }
        });

        assert_eq!(result.unwrap_err().code, AppErrorCode::Conflict);
        let mut file = writer.lock().unwrap().take().unwrap();
        file.write_all(b"after = true\n").unwrap();
        file.sync_all().unwrap();
        drop(file);
        let recovery = recover_rollback_journal(&descriptor);
        let paths = rollback_paths(&descriptor.config_path).unwrap();
        assert!(
            recovery.is_ok(),
            "recovery={recovery:?}, target={:?}, candidate={:?}, displaced={:?}, rescue={:?}",
            read_bounded_optional_file(&descriptor.config_path, MAX_PRESET_CONFIG_BYTES),
            read_bounded_optional_file(&paths.candidate, MAX_PRESET_CONFIG_BYTES),
            read_bounded_optional_file(&paths.displaced, MAX_PRESET_CONFIG_BYTES),
            read_bounded_optional_file(&paths.rescue, MAX_PRESET_CONFIG_BYTES),
        );
        assert!(recovery.unwrap());
        let recovered = fs::read_to_string(&descriptor.config_path).unwrap();
        assert!(recovered.contains("before = true"));
        assert!(recovered.contains("after = true"));
        assert!(!recovered.contains("powershell.exe"));
    }

    #[cfg(windows)]
    #[test]
    fn writer_that_crosses_replace_is_drained_before_sidecar_cleanup() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let target = descriptor.config_path.clone();
        let writer = Arc::new(Mutex::new(None::<File>));
        let observed_writer = Arc::clone(&writer);

        let result = mutation.rollback_with_observer(move |_, phase| {
            if phase == RollbackPhase::AfterPreflightBeforeReplace
                && observed_writer.lock().unwrap().is_none()
            {
                let mut file = OpenOptions::new()
                    .append(true)
                    .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0)
                    .open(&target)
                    .unwrap();
                file.write_all(b"\n[vendor_cross_replace]\nbefore = true\n")
                    .unwrap();
                file.sync_all().unwrap();
                *observed_writer.lock().unwrap() = Some(file);
            }
        });

        assert_eq!(result.unwrap_err().code, AppErrorCode::Conflict);
        let mut file = writer.lock().unwrap().take().unwrap();
        file.write_all(b"after = true\n").unwrap();
        file.sync_all().unwrap();
        drop(file);
        let recovery = recover_rollback_journal(&descriptor);
        let paths = rollback_paths(&descriptor.config_path).unwrap();
        assert!(
            recovery.is_ok(),
            "recovery={recovery:?}, target={:?}, candidate={:?}, displaced={:?}, rescue={:?}",
            read_bounded_optional_file(&descriptor.config_path, MAX_PRESET_CONFIG_BYTES),
            read_bounded_optional_file(&paths.candidate, MAX_PRESET_CONFIG_BYTES),
            read_bounded_optional_file(&paths.displaced, MAX_PRESET_CONFIG_BYTES),
            read_bounded_optional_file(&paths.rescue, MAX_PRESET_CONFIG_BYTES),
        );
        assert!(recovery.unwrap());
        let recovered = fs::read_to_string(&descriptor.config_path).unwrap();
        assert!(recovered.contains("[vendor_cross_replace]"));
        assert!(recovered.contains("before = true"));
        assert!(recovered.contains("after = true"));
        assert!(!recovered.contains("powershell.exe"));
    }

    #[cfg(windows)]
    #[test]
    fn reported_1177_moved_state_enters_hash_and_identity_reconciliation() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("settings.toml");
        let candidate = root.path().join("candidate.toml");
        let displaced = root.path().join("displaced.toml");
        let expected = b"vendor original";
        let desired = b"rollback candidate";
        fs::write(&target, expected).unwrap();
        fs::write(&candidate, desired).unwrap();
        let target_identity = current_file_identity(&target, &sha256_hex(expected)).unwrap();
        let candidate_identity = current_file_identity(&candidate, &sha256_hex(desired)).unwrap();

        let outcome = replace_verified_with_backup_using(
            &candidate,
            &target,
            &displaced,
            &sha256_hex(expected),
            target_identity.as_ref(),
            &sha256_hex(desired),
            candidate_identity.as_ref(),
            || {},
            |_, target, displaced| {
                move_file_if_absent(target, displaced).unwrap();
                Ok(ReplaceCallOutcome::ReportedPartialFailure)
            },
        )
        .unwrap();

        assert!(matches!(outcome, VerifiedReplaceOutcome::Invoked));
        assert!(!target.exists());
        assert_eq!(fs::read(&candidate).unwrap(), desired);
        assert_eq!(fs::read(&displaced).unwrap(), expected);
    }

    #[cfg(windows)]
    #[test]
    fn reported_1177_moved_state_is_completed_end_to_end() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        let original = b"api_key = \"secret\"\n";
        fs::write(&descriptor.config_path, original).unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        SIMULATED_REPLACE_ERROR.with(|simulate| simulate.set(1177));

        mutation.rollback().unwrap();

        assert_eq!(fs::read(&descriptor.config_path).unwrap(), original);
        assert!(!recover_rollback_journal(&descriptor).unwrap());
        let paths = rollback_paths(&descriptor.config_path).unwrap();
        assert!(!paths.journal.exists());
        assert!(!paths.candidate.exists());
        assert!(!paths.displaced.exists());
        assert!(!paths.rescue.exists());
    }

    #[cfg(windows)]
    #[test]
    fn reported_1175_and_1176_unchanged_states_retry_end_to_end() {
        for error in [1175, 1176] {
            let root = tempfile::tempdir().unwrap();
            let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
            let original = b"api_key = \"secret\"\n";
            fs::write(&descriptor.config_path, original).unwrap();
            let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
            SIMULATED_REPLACE_ERROR.with(|simulate| simulate.set(error));

            mutation.rollback().unwrap();

            assert_eq!(fs::read(&descriptor.config_path).unwrap(), original);
            assert!(!recover_rollback_journal(&descriptor).unwrap());
        }
    }

    #[cfg(windows)]
    #[test]
    fn replacefile_1177_recovery_never_overwrites_a_vendor_recreated_target() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let target = descriptor.config_path.clone();
        let vendor_v2 = b"api_key = \"vendor-v2\"\n[vendor]\nversion = 2\n".to_vec();
        let injected_vendor_v2 = vendor_v2.clone();
        let mut recreated = false;
        SIMULATED_REPLACE_ERROR.with(|simulate| simulate.set(1177));

        let result = mutation.rollback_with_observer(move |_, phase| match phase {
            RollbackPhase::BeforeMissingTargetCommit if !recreated => {
                fs::write(&target, &injected_vendor_v2).unwrap();
                recreated = true;
            }
            _ => {}
        });

        assert_eq!(result.unwrap_err().code, AppErrorCode::Conflict);
        assert_eq!(fs::read(&descriptor.config_path).unwrap(), vendor_v2);
    }

    #[cfg(windows)]
    #[test]
    fn missing_target_recovery_rejects_a_same_hash_candidate_with_a_new_identity() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let target = descriptor.config_path.clone();
        let paths = rollback_paths(&target).unwrap();
        let observed_paths = paths.clone();
        let mut moved_state = false;
        let mut replaced_candidate = false;

        let result = mutation.rollback_with_observer(move |_, phase| match phase {
            RollbackPhase::AfterReplaceBeforeRecovery if !moved_state => {
                let desired = fs::read(&target).unwrap();
                fs::write(&observed_paths.candidate, desired).unwrap();
                fs::remove_file(&target).unwrap();
                moved_state = true;
            }
            RollbackPhase::AfterMissingPreflightBeforeMove if !replaced_candidate => {
                let desired = fs::read(&observed_paths.candidate).unwrap();
                atomic_write(&observed_paths.candidate, &desired).unwrap();
                replaced_candidate = true;
            }
            _ => {}
        });

        assert_eq!(result.unwrap_err().code, AppErrorCode::Conflict);
        assert!(!descriptor.config_path.exists());
        assert!(paths.candidate.exists());
        assert!(paths.displaced.exists());
        assert!(paths.journal.exists());
    }

    #[cfg(windows)]
    #[test]
    fn missing_target_recovery_keeps_vendor_target_written_after_candidate_move() {
        for replace_identity in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
            fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
            let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
            let target = descriptor.config_path.clone();
            let paths = rollback_paths(&target).unwrap();
            let vendor_v2 = b"api_key = \"vendor-v2\"\n[vendor]\nversion = 2\n".to_vec();
            let injected = vendor_v2.clone();
            let mut wrote_vendor = false;
            SIMULATED_REPLACE_ERROR.with(|simulate| simulate.set(1177));

            let result = mutation.rollback_with_observer(move |_, phase| {
                if phase == RollbackPhase::AfterMissingMoveBeforePostValidation && !wrote_vendor {
                    if replace_identity {
                        atomic_write(&target, &injected).unwrap();
                    } else {
                        fs::write(&target, &injected).unwrap();
                    }
                    wrote_vendor = true;
                }
            });

            assert_eq!(result.unwrap_err().code, AppErrorCode::Conflict);
            assert_eq!(fs::read(&descriptor.config_path).unwrap(), vendor_v2);
            assert!(!paths.candidate.exists());
            assert!(paths.displaced.exists());
            assert!(paths.journal.exists());
        }
    }

    #[cfg(windows)]
    #[test]
    fn missing_target_recovery_rebuilds_after_the_moved_candidate_is_deleted() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        let original = b"api_key = \"secret\"\n";
        fs::write(&descriptor.config_path, original).unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let target = descriptor.config_path.clone();
        let paths = rollback_paths(&target).unwrap();
        let mut deleted = false;
        SIMULATED_REPLACE_ERROR.with(|simulate| simulate.set(1177));

        let result = mutation.rollback_with_observer(move |_, phase| {
            if phase == RollbackPhase::AfterMissingMoveBeforePostValidation && !deleted {
                fs::remove_file(&target).unwrap();
                deleted = true;
            }
        });

        assert_eq!(result.unwrap_err().code, AppErrorCode::Conflict);
        assert!(!descriptor.config_path.exists());
        assert!(!paths.candidate.exists());
        assert!(paths.displaced.exists());
        assert!(paths.journal.exists());

        assert!(recover_rollback_journal(&descriptor).unwrap());
        assert!(!recover_rollback_journal(&descriptor).unwrap());
        assert_eq!(fs::read(&descriptor.config_path).unwrap(), original);
        assert_no_rollback_artifacts(&paths, "missing-target delete recovery");
    }

    #[cfg(windows)]
    #[test]
    fn missing_target_rebases_a_same_identity_displaced_writer_update() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let paths = rollback_paths(&descriptor.config_path).unwrap();
        let observed_paths = paths.clone();
        let mut updated = false;
        SIMULATED_REPLACE_ERROR.with(|simulate| simulate.set(1177));

        let result = mutation.rollback_with_observer(move |_, phase| {
            if phase == RollbackPhase::AfterReplaceBeforeRecovery && !updated {
                let before =
                    guarded_file_state(&observed_paths.displaced, MAX_PRESET_CONFIG_BYTES).unwrap();
                let before_identity = match before {
                    GuardedFileState::Present(snapshot) => snapshot.identity,
                    GuardedFileState::Missing | GuardedFileState::Busy => {
                        panic!("displaced generation was unavailable")
                    }
                };
                let mut file = OpenOptions::new()
                    .append(true)
                    .open(&observed_paths.displaced)
                    .unwrap();
                file.write_all(b"\n[vendor_late]\nkeep = true\n").unwrap();
                file.sync_all().unwrap();
                drop(file);
                let after =
                    guarded_file_state(&observed_paths.displaced, MAX_PRESET_CONFIG_BYTES).unwrap();
                let after_identity = match after {
                    GuardedFileState::Present(snapshot) => snapshot.identity,
                    GuardedFileState::Missing | GuardedFileState::Busy => {
                        panic!("updated displaced generation was unavailable")
                    }
                };
                assert_eq!(after_identity, before_identity);
                fs::remove_file(&observed_paths.candidate).unwrap();
                updated = true;
            }
        });

        assert_eq!(result.unwrap_err().code, AppErrorCode::Conflict);
        assert!(!descriptor.config_path.exists());
        assert!(!paths.candidate.exists());
        assert!(paths.displaced.exists());
        assert!(paths.journal.exists());

        assert!(recover_rollback_journal(&descriptor).unwrap());
        assert!(!recover_rollback_journal(&descriptor).unwrap());
        let recovered = fs::read_to_string(&descriptor.config_path).unwrap();
        assert!(recovered.contains("[vendor_late]"));
        assert!(recovered.contains("keep = true"));
        assert!(!recovered.contains("powershell.exe"));
        assert_no_rollback_artifacts(&paths, "same-identity displaced rebase");
    }

    #[cfg(windows)]
    #[test]
    fn missing_target_revalidates_displaced_identity_before_committing_candidate() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let target = descriptor.config_path.clone();
        let paths = rollback_paths(&target).unwrap();
        let observed_paths = paths.clone();
        let mut replaced_displaced = false;
        SIMULATED_REPLACE_ERROR.with(|simulate| simulate.set(1177));

        let result = mutation.rollback_with_observer(move |_, phase| {
            if phase == RollbackPhase::BeforeMissingTargetCommit && !replaced_displaced {
                let bytes = fs::read(&observed_paths.displaced).unwrap();
                atomic_write(&observed_paths.displaced, &bytes).unwrap();
                replaced_displaced = true;
            }
        });

        assert_eq!(result.unwrap_err().code, AppErrorCode::Conflict);
        assert!(!descriptor.config_path.exists());
        assert!(paths.candidate.exists());
        assert!(paths.displaced.exists());
        assert!(paths.journal.exists());
    }

    #[cfg(windows)]
    #[test]
    fn post_replace_rejects_unowned_displaced_identity_even_with_expected_hash() {
        for preserve_hash in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
            fs::write(&descriptor.config_path, b"api_key = \"secret\"\n").unwrap();
            let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
            let paths = rollback_paths(&descriptor.config_path).unwrap();
            let observed_paths = paths.clone();
            let arbitrary = b"api_key = \"unowned\"\n[vendor]\nkeep = true\n".to_vec();
            let mut injected = false;

            let result = mutation.rollback_with_observer(move |_, phase| {
                if phase == RollbackPhase::AfterReplaceBeforeRecovery && !injected {
                    let replacement = if preserve_hash {
                        fs::read(&observed_paths.displaced).unwrap()
                    } else {
                        arbitrary.clone()
                    };
                    atomic_write(&observed_paths.displaced, &replacement).unwrap();
                    injected = true;
                }
            });

            assert_eq!(result.unwrap_err().code, AppErrorCode::Conflict);
            assert!(paths.displaced.exists());
            assert!(paths.journal.exists());
        }
    }

    #[test]
    fn recovery_preserves_unjournaled_displaced_identity_and_fails_closed() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        let original = b"api_key = \"secret\"\n";
        fs::write(&descriptor.config_path, original).unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let installed = fs::read(&descriptor.config_path).unwrap();
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            mutation
                .rollback_with_observer(|_, phase| {
                    if phase == RollbackPhase::AfterJournalPrepared {
                        panic!("leave the original rollback journal");
                    }
                })
                .unwrap();
        }));
        assert!(interrupted.is_err());
        let paths = rollback_paths(&descriptor.config_path).unwrap();
        let mut vendor_displaced = installed;
        vendor_displaced.extend_from_slice(b"\n[vendor_after_observation]\nkeep = true\n");
        fs::write(&descriptor.config_path, original).unwrap();
        fs::write(&paths.displaced, &vendor_displaced).unwrap();
        remove_if_exists(&paths.candidate).unwrap();

        assert_eq!(
            recover_rollback_journal(&descriptor).unwrap_err().code,
            AppErrorCode::Conflict
        );
        assert_eq!(fs::read(&paths.displaced).unwrap(), vendor_displaced);
        assert!(paths.journal.exists());
    }

    #[test]
    fn startup_recovery_is_idempotent_and_finishes_before_the_spool_watcher_starts() {
        let root = tempfile::tempdir().unwrap();
        let repository = repository(&root.path().join("db"));
        let bridge = bridge(
            root.path(),
            repository.clone(),
            Arc::new(RecordingEmitter::default()),
        );
        let profile = repository
            .get(&AgentIntegrationId::parse("kimi-windows").unwrap())
            .unwrap();
        let descriptor = bridge.descriptor(&profile).unwrap();
        fs::create_dir_all(descriptor.config_path.parent().unwrap()).unwrap();
        let original = b"# vendor comment\napi_key = \"secret\"\n";
        fs::write(&descriptor.config_path, original).unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            mutation
                .rollback_with_observer(|_, phase| {
                    if phase == RollbackPhase::AfterJournalPrepared {
                        panic!("simulated process interruption after journal flush");
                    }
                })
                .unwrap();
        }));
        assert!(interrupted.is_err());
        assert!(rollback_paths(&descriptor.config_path)
            .unwrap()
            .journal
            .exists());

        bridge.ensure_started().unwrap();

        assert_eq!(fs::read(&descriptor.config_path).unwrap(), original);
        assert!(!recover_rollback_journal(&descriptor).unwrap());
        bridge.ensure_started().unwrap();
        bridge.shutdown();
    }

    #[cfg(windows)]
    #[test]
    fn rollback_readers_observe_only_complete_old_or_new_documents() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        let mut original = b"api_key = \"secret\"\n# ".to_vec();
        original.extend(std::iter::repeat_n(b'x', 2 * 1024 * 1024));
        original.push(b'\n');
        fs::write(&descriptor.config_path, &original).unwrap();
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        let installed = fs::read(&descriptor.config_path).unwrap();
        assert_ne!(installed, original);

        let stop = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let unexpected = Arc::new(Mutex::new(Vec::<String>::new()));
        let transient_misses = Arc::new(AtomicUsize::new(0));
        let reads = Arc::new(AtomicUsize::new(0));
        let reader_path = descriptor.config_path.clone();
        let reader_stop = Arc::clone(&stop);
        let reader_started = Arc::clone(&started);
        let reader_unexpected = Arc::clone(&unexpected);
        let reader_transient_misses = Arc::clone(&transient_misses);
        let reader_reads = Arc::clone(&reads);
        let allowed_old = installed.clone();
        let allowed_new = original.clone();
        let reader = thread::spawn(move || {
            reader_started.store(true, Ordering::SeqCst);
            while !reader_stop.load(Ordering::SeqCst) {
                match fs::read(&reader_path) {
                    Ok(bytes) if bytes == allowed_old || bytes == allowed_new => {
                        reader_reads.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(bytes) => {
                        reader_unexpected
                            .lock()
                            .unwrap()
                            .push(format!("unexpected {}-byte document", bytes.len()));
                        break;
                    }
                    Err(error)
                        if error.kind() == std::io::ErrorKind::NotFound
                            || error.raw_os_error() == Some(32) =>
                    {
                        if reader_transient_misses.fetch_add(1, Ordering::Relaxed) >= 4096 {
                            reader_unexpected
                                .lock()
                                .unwrap()
                                .push("replacement namespace gap exceeded retry bound".into());
                            break;
                        }
                        thread::yield_now();
                    }
                    Err(error) => {
                        reader_unexpected
                            .lock()
                            .unwrap()
                            .push(format!("read failed: {error}"));
                        break;
                    }
                }
            }
        });
        while !started.load(Ordering::SeqCst) {
            thread::yield_now();
        }
        while reads.load(Ordering::Relaxed) < 2 {
            thread::yield_now();
        }

        let replace_state = Arc::new(Mutex::new(None));
        let observed_replace_state = Arc::clone(&replace_state);
        let target_path = descriptor.config_path.clone();
        let sidecars = rollback_paths(&descriptor.config_path).unwrap();
        mutation
            .rollback_with_observer(move |_, phase| {
                if phase == RollbackPhase::AfterReplaceBeforeRecovery {
                    *observed_replace_state.lock().unwrap() = Some((
                        target_path.exists(),
                        sidecars.candidate.exists(),
                        sidecars.displaced.exists(),
                    ));
                }
            })
            .unwrap();
        thread::sleep(Duration::from_millis(20));
        stop.store(true, Ordering::SeqCst);
        reader.join().unwrap();

        assert!(reads.load(Ordering::Relaxed) >= 2);
        assert!(transient_misses.load(Ordering::Relaxed) <= 4096);
        assert_eq!(
            *unexpected.lock().unwrap(),
            Vec::<String>::new(),
            "replace state: {:?}",
            *replace_state.lock().unwrap()
        );
        assert_eq!(fs::read(&descriptor.config_path).unwrap(), original);
    }

    #[cfg(windows)]
    #[test]
    fn rollback_crash_child_fixture() {
        let Ok(root) = std::env::var("AISLAND_ROLLBACK_CRASH_ROOT") else {
            return;
        };
        let phase = std::env::var("AISLAND_ROLLBACK_CRASH_PHASE").unwrap();
        let originally_existed =
            std::env::var("AISLAND_ROLLBACK_ORIGINALLY_EXISTED").unwrap() == "true";
        let descriptor = descriptor(Path::new(&root), PresetAgentAdapterId::Kimi);
        if originally_existed {
            fs::write(
                &descriptor.config_path,
                b"# vendor original\napi_key = \"secret\"\n",
            )
            .unwrap();
        }
        let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
        if phase == "missing-rebuild-candidate-sync" {
            let target = descriptor.config_path.clone();
            let mut deleted = false;
            SIMULATED_REPLACE_ERROR.with(|simulate| simulate.set(1177));
            let result = mutation.rollback_with_observer(move |_, current| {
                if current == RollbackPhase::AfterMissingMoveBeforePostValidation && !deleted {
                    fs::remove_file(&target).unwrap();
                    deleted = true;
                }
            });
            assert_eq!(result.unwrap_err().code, AppErrorCode::Conflict);
            let paths = rollback_paths(&descriptor.config_path).unwrap();
            let journal: RollbackJournal =
                serde_json::from_slice(&fs::read(&paths.journal).unwrap()).unwrap();
            execute_rollback_journal(&descriptor, journal, &mut |_, current| {
                if current == RollbackPhase::AfterCandidateSyncBeforeIdentityJournal {
                    std::process::abort();
                }
            })
            .unwrap();
            panic!("missing-target rebuild did not reach candidate sync");
        }
        let installed = fs::read(&descriptor.config_path).unwrap();
        let target = descriptor.config_path.clone();
        let mut candidate_sync_count = 0usize;
        let mut replace_count = 0usize;
        mutation
            .rollback_with_observer(|_, current| {
                if current == RollbackPhase::AfterReplaceBeforeRecovery
                    && (phase.starts_with("candidate-sync-") || phase.starts_with("cleanup-"))
                {
                    replace_count += 1;
                    if replace_count == 1 {
                        let mut vendor_v2 = installed.clone();
                        vendor_v2.extend_from_slice(b"\n[vendor_v2]\nkeep = true\n");
                        atomic_write(&target, &vendor_v2).unwrap();
                    } else if replace_count == 2
                        && (phase == "candidate-sync-rollover"
                            || phase == "cleanup-rollover-rescue")
                    {
                        let mut vendor_v3 = fs::read(&target).unwrap();
                        vendor_v3.extend_from_slice(b"\n[vendor_v3]\nkeep = true\n");
                        atomic_write(&target, &vendor_v3).unwrap();
                    }
                }
                if current == RollbackPhase::AfterCandidateSyncBeforeIdentityJournal {
                    candidate_sync_count += 1;
                    let should_abort = match phase.as_str() {
                        "candidate-sync" => candidate_sync_count == 1,
                        "candidate-sync-rebase" => candidate_sync_count == 2,
                        "candidate-sync-rollover" => candidate_sync_count == 3,
                        _ => false,
                    };
                    if should_abort {
                        std::process::abort();
                    }
                }
                let cleanup_phase = match current {
                    RollbackPhase::AfterCleanupCandidate => Some("cleanup-candidate"),
                    RollbackPhase::AfterCleanupDisplaced => Some("cleanup-displaced"),
                    RollbackPhase::AfterCleanupRescue => Some("cleanup-rescue"),
                    RollbackPhase::AfterCleanupRolloverRescue => Some("cleanup-rollover-rescue"),
                    _ => None,
                };
                if cleanup_phase == Some(phase.as_str()) {
                    std::process::abort();
                }
                let current = match current {
                    RollbackPhase::AfterJournalPrepared => "journal",
                    RollbackPhase::AfterCandidateSyncBeforeIdentityJournal => return,
                    RollbackPhase::AfterCandidatePreparedBeforeReplace => "candidate",
                    RollbackPhase::AfterPreflightBeforeReplace => "pre-replace",
                    RollbackPhase::AfterReplaceBeforeRecovery => "replace",
                    RollbackPhase::AfterRolloverJournalBeforeNormalize => return,
                    RollbackPhase::BeforeMissingTargetCommit => return,
                    RollbackPhase::AfterMissingPreflightBeforeMove => return,
                    RollbackPhase::AfterMissingMoveBeforePostValidation => return,
                    RollbackPhase::AfterCleanupCandidate
                    | RollbackPhase::AfterCleanupDisplaced
                    | RollbackPhase::AfterCleanupRescue
                    | RollbackPhase::AfterCleanupRolloverRescue => return,
                    RollbackPhase::AfterDeleteIntentPrepared => "delete-intent",
                    RollbackPhase::AfterDelete => "delete",
                    RollbackPhase::BeforeLock | RollbackPhase::AfterLockBeforeCommit => return,
                };
                if current == phase {
                    std::process::abort();
                }
            })
            .unwrap();
        panic!("rollback crash fixture did not reach phase {phase}");
    }

    #[cfg(windows)]
    #[test]
    fn every_durable_rollback_phase_recovers_after_process_termination() {
        for (phase, originally_existed) in [
            ("journal", true),
            ("candidate-sync", true),
            ("candidate", true),
            ("pre-replace", true),
            ("replace", true),
            ("delete-intent", false),
            ("delete", false),
        ] {
            let root = tempfile::tempdir().unwrap();
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "services::agent_profile_spool::tests::rollback_crash_child_fixture",
                    "--nocapture",
                ])
                .env("AISLAND_ROLLBACK_CRASH_ROOT", root.path())
                .env("AISLAND_ROLLBACK_CRASH_PHASE", phase)
                .env(
                    "AISLAND_ROLLBACK_ORIGINALLY_EXISTED",
                    originally_existed.to_string(),
                )
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(!status.success(), "fixture did not terminate at {phase}");

            let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
            let recovered_once = recover_rollback_journal(&descriptor)
                .unwrap_or_else(|error| panic!("{phase} recovery failed: {error:?}"));
            assert!(recovered_once, "{phase}");
            assert!(!recover_rollback_journal(&descriptor).unwrap(), "{phase}");
            if originally_existed {
                assert_eq!(
                    fs::read(&descriptor.config_path).unwrap(),
                    b"# vendor original\napi_key = \"secret\"\n",
                    "{phase}"
                );
            } else {
                assert!(!descriptor.config_path.exists(), "{phase}");
            }
            let paths = rollback_paths(&descriptor.config_path).unwrap();
            assert!(!paths.journal.exists(), "{phase}");
            assert!(!paths.candidate.exists(), "{phase}");
            assert!(!paths.displaced.exists(), "{phase}");
            assert!(!paths.rescue.exists(), "{phase}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn rebase_and_rollover_candidate_sync_crashes_are_recoverable() {
        for phase in ["candidate-sync-rebase", "candidate-sync-rollover"] {
            let root = tempfile::tempdir().unwrap();
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "services::agent_profile_spool::tests::rollback_crash_child_fixture",
                    "--nocapture",
                ])
                .env("AISLAND_ROLLBACK_CRASH_ROOT", root.path())
                .env("AISLAND_ROLLBACK_CRASH_PHASE", phase)
                .env("AISLAND_ROLLBACK_ORIGINALLY_EXISTED", "true")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(!status.success(), "fixture did not terminate at {phase}");

            let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
            let paths = rollback_paths(&descriptor.config_path).unwrap();
            let journal: RollbackJournal =
                serde_json::from_slice(&fs::read(&paths.journal).unwrap()).unwrap();
            assert!(journal.candidate_identity.is_none(), "{phase}");
            verify_candidate_adoption_sidecars(&paths, &journal)
                .unwrap_or_else(|error| panic!("{phase} sidecar adoption failed: {error:?}"));
            let recovered_once = recover_rollback_journal(&descriptor)
                .unwrap_or_else(|error| panic!("{phase} recovery failed: {error:?}"));
            assert!(recovered_once, "{phase}");
            assert!(!recover_rollback_journal(&descriptor).unwrap(), "{phase}");
            let recovered = fs::read_to_string(&descriptor.config_path).unwrap();
            assert!(recovered.contains("[vendor_v2]"), "{phase}");
            if phase == "candidate-sync-rollover" {
                assert!(recovered.contains("[vendor_v3]"), "{phase}");
            }
            assert!(!recovered.contains("powershell.exe"), "{phase}");
            assert_no_rollback_artifacts(&paths, phase);
        }
    }

    #[cfg(windows)]
    #[test]
    fn missing_target_rebuild_candidate_sync_crash_is_recoverable() {
        let root = tempfile::tempdir().unwrap();
        let phase = "missing-rebuild-candidate-sync";
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "services::agent_profile_spool::tests::rollback_crash_child_fixture",
                "--nocapture",
            ])
            .env("AISLAND_ROLLBACK_CRASH_ROOT", root.path())
            .env("AISLAND_ROLLBACK_CRASH_PHASE", phase)
            .env("AISLAND_ROLLBACK_ORIGINALLY_EXISTED", "true")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(!status.success(), "fixture did not terminate at {phase}");

        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        assert!(recover_rollback_journal(&descriptor).unwrap());
        assert!(!recover_rollback_journal(&descriptor).unwrap());
        assert_eq!(
            fs::read(&descriptor.config_path).unwrap(),
            b"# vendor original\napi_key = \"secret\"\n"
        );
        assert_no_rollback_artifacts(&rollback_paths(&descriptor.config_path).unwrap(), phase);
    }

    #[cfg(windows)]
    #[test]
    fn cleanup_resume_rejects_an_unknown_artifact_in_an_already_consumed_phase() {
        let root = tempfile::tempdir().unwrap();
        let phase = "cleanup-displaced";
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "services::agent_profile_spool::tests::rollback_crash_child_fixture",
                "--nocapture",
            ])
            .env("AISLAND_ROLLBACK_CRASH_ROOT", root.path())
            .env("AISLAND_ROLLBACK_CRASH_PHASE", phase)
            .env("AISLAND_ROLLBACK_ORIGINALLY_EXISTED", "true")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(!status.success(), "fixture did not terminate at {phase}");

        let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
        let paths = rollback_paths(&descriptor.config_path).unwrap();
        let unknown = b"unknown complete candidate generation";
        fs::write(&paths.candidate, unknown).unwrap();

        assert_eq!(
            recover_rollback_journal(&descriptor).unwrap_err().code,
            AppErrorCode::Conflict
        );
        assert_eq!(fs::read(&paths.candidate).unwrap(), unknown);
        assert!(paths.journal.exists());
    }

    #[cfg(windows)]
    #[test]
    fn every_cleanup_deletion_boundary_recovers_after_process_termination() {
        for phase in [
            "cleanup-candidate",
            "cleanup-displaced",
            "cleanup-rescue",
            "cleanup-rollover-rescue",
        ] {
            let root = tempfile::tempdir().unwrap();
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "services::agent_profile_spool::tests::rollback_crash_child_fixture",
                    "--nocapture",
                ])
                .env("AISLAND_ROLLBACK_CRASH_ROOT", root.path())
                .env("AISLAND_ROLLBACK_CRASH_PHASE", phase)
                .env("AISLAND_ROLLBACK_ORIGINALLY_EXISTED", "true")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(!status.success(), "fixture did not terminate at {phase}");

            let descriptor = descriptor(root.path(), PresetAgentAdapterId::Kimi);
            assert!(recover_rollback_journal(&descriptor).unwrap(), "{phase}");
            assert!(!recover_rollback_journal(&descriptor).unwrap(), "{phase}");
            let recovered = fs::read_to_string(&descriptor.config_path).unwrap();
            assert!(recovered.contains("[vendor_v2]"), "{phase}");
            if phase == "cleanup-rollover-rescue" {
                assert!(recovered.contains("[vendor_v3]"), "{phase}");
            }
            assert!(!recovered.contains("powershell.exe"), "{phase}");
            assert_no_rollback_artifacts(&rollback_paths(&descriptor.config_path).unwrap(), phase);
        }
    }

    #[test]
    fn failed_first_install_removes_aisland_created_kimi_and_qoder_configs() {
        for adapter in [PresetAgentAdapterId::Kimi, PresetAgentAdapterId::Qoderwork] {
            let root = tempfile::tempdir().unwrap();
            let descriptor = descriptor(root.path(), adapter);
            assert!(!descriptor.config_path.exists());

            let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
            assert!(descriptor.config_path.exists());

            mutation.rollback().unwrap();

            assert!(
                !descriptor.config_path.exists(),
                "failed first install left an orphan config for {:?}",
                descriptor.adapter_id
            );
        }
    }

    #[test]
    fn failed_first_install_preserves_vendor_content_added_to_new_configs() {
        for adapter in [PresetAgentAdapterId::Kimi, PresetAgentAdapterId::Qoderwork] {
            let root = tempfile::tempdir().unwrap();
            let descriptor = descriptor(root.path(), adapter.clone());
            let mutation = mutate_descriptor(&descriptor, MergeAction::Install, 1).unwrap();
            match adapter {
                PresetAgentAdapterId::Kimi => {
                    let mut vendor_edit = fs::read(&descriptor.config_path).unwrap();
                    vendor_edit.extend_from_slice(b"\n[vendor]\nkeep = \"yes\"\n");
                    fs::write(&descriptor.config_path, vendor_edit).unwrap();
                }
                PresetAgentAdapterId::Qoderwork => {
                    let mut vendor_edit: serde_json::Value =
                        serde_json::from_slice(&fs::read(&descriptor.config_path).unwrap())
                            .unwrap();
                    vendor_edit["vendor"] = serde_json::json!({"keep": true});
                    fs::write(
                        &descriptor.config_path,
                        serde_json::to_vec_pretty(&vendor_edit).unwrap(),
                    )
                    .unwrap();
                }
                PresetAgentAdapterId::Trae => unreachable!(),
                PresetAgentAdapterId::Cursor => unreachable!(),
            }

            mutation.rollback().unwrap();

            let rolled_back = fs::read_to_string(&descriptor.config_path).unwrap();
            assert!(rolled_back.contains("vendor"));
            assert!(rolled_back.contains("keep"));
            assert!(!rolled_back.contains("powershell.exe"));
        }
    }

    #[test]
    fn preset_backups_are_retained_with_a_fixed_bound() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        fs::write(&path, b"{}").unwrap();
        for now in 1..=6 {
            write_backup(&path, format!("backup-{now}").as_bytes(), now).unwrap();
        }
        let backups = fs::read_dir(root.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains("aisland-backup")
            })
            .count();
        assert_eq!(backups, MAX_BACKUPS_PER_CONFIG);
    }

    #[test]
    fn startup_scan_isolates_a_bad_file_and_projects_the_following_good_file() {
        let root = tempfile::tempdir().unwrap();
        let repository = repository(&root.path().join("db"));
        let emitter = Arc::new(RecordingEmitter::default());
        let bridge = bridge(root.path(), repository.clone(), emitter.clone());
        let id = AgentIntegrationId::parse("kimi-windows").unwrap();
        let profile = repository.get(&id).unwrap();
        let installed_at = now_millis() - 1_000;
        repository
            .set_installation(
                &AgentProfileInstallation {
                    profile_id: id.clone(),
                    state: IntegrationState::Installed,
                    reason_code: None,
                    owned_resource: Some("test".into()),
                    owned_fingerprint: Some("test".into()),
                    external_hash: Some("test".into()),
                    updated_at: installed_at,
                },
                profile.revision,
                true,
            )
            .unwrap();
        fs::create_dir_all(&bridge.spool_dir).unwrap();
        fs::write(
            bridge
                .spool_dir
                .join("00000000000000000000000000000000.json"),
            b"{not-json",
        )
        .unwrap();
        let good = serde_json::json!({
            "profileId": "kimi-windows",
            "nativeEvent": "UserPromptSubmit",
            "taskId": "session-1",
            "sourceEventId": "event-1",
            "occurredAt": installed_at + 1,
        });
        fs::write(
            bridge
                .spool_dir
                .join("11111111111111111111111111111111.json"),
            serde_json::to_vec(&good).unwrap(),
        )
        .unwrap();

        assert_eq!(bridge.scan_pending().unwrap(), 2);
        let observations = repository.list_observations(&id).unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].status, AgentStatus::Running);
        assert_eq!(emitter.calls.load(Ordering::Relaxed), 1);
        assert_eq!(fs::read_dir(&bridge.spool_dir).unwrap().count(), 0);
    }

    #[test]
    fn dirty_hint_channel_coalesces_an_unbounded_notification_burst() {
        let (dirty_tx, dirty_rx) = dirty_hint_channel();

        assert!(mark_dirty(&dirty_tx));
        for _ in 0..10_000 {
            assert!(!mark_dirty(&dirty_tx));
        }
        assert_eq!(dirty_rx.try_recv(), Ok(()));
        assert!(mark_dirty(&dirty_tx));
    }

    #[test]
    fn background_scan_drains_more_than_one_bounded_batch_with_bad_and_valid_files() {
        let root = tempfile::tempdir().unwrap();
        let repository = repository(&root.path().join("db"));
        let emitter = Arc::new(RecordingEmitter::default());
        let bridge = bridge(root.path(), repository.clone(), emitter.clone());
        let qoder_id = AgentIntegrationId::parse("qoderwork-windows").unwrap();
        let qoder = repository.get(&qoder_id).unwrap();
        let installed_at = now_millis() - 1_000;
        repository
            .set_installation(
                &AgentProfileInstallation {
                    profile_id: qoder_id.clone(),
                    state: IntegrationState::Installed,
                    reason_code: None,
                    owned_resource: Some("test".into()),
                    owned_fingerprint: Some("test".into()),
                    external_hash: Some("test".into()),
                    updated_at: installed_at,
                },
                qoder.revision,
                true,
            )
            .unwrap();
        fs::create_dir_all(&bridge.spool_dir).unwrap();
        for index in 0..=MAX_STARTUP_SPOOL_FILES {
            let event = serde_json::json!({
                "profileId": "kimi-windows",
                "nativeEvent": "Stop",
                "taskId": format!("inactive-{index}"),
                "sourceEventId": format!("inactive-{index}"),
                "occurredAt": installed_at + 1,
            });
            fs::write(
                bridge.spool_dir.join(format!("{index:032x}.json")),
                serde_json::to_vec(&event).unwrap(),
            )
            .unwrap();
        }
        fs::write(
            bridge
                .spool_dir
                .join("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee.json"),
            b"{not-json",
        )
        .unwrap();
        let good = serde_json::json!({
            "profileId": "qoderwork-windows",
            "nativeEvent": "UserPromptSubmit",
            "taskId": "session-final",
            "sourceEventId": "event-final",
            "occurredAt": installed_at + 2,
        });
        fs::write(
            bridge
                .spool_dir
                .join("ffffffffffffffffffffffffffffffff.json"),
            serde_json::to_vec(&good).unwrap(),
        )
        .unwrap();

        bridge.ensure_started().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while std::time::Instant::now() < deadline {
            let remaining = fs::read_dir(&bridge.spool_dir).unwrap().count();
            if remaining == 0 && !repository.list_observations(&qoder_id).unwrap().is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }

        assert_eq!(fs::read_dir(&bridge.spool_dir).unwrap().count(), 0);
        let observations = repository.list_observations(&qoder_id).unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].status, AgentStatus::Running);
        assert_eq!(emitter.calls.load(Ordering::Relaxed), 1);
        bridge.shutdown();
    }

    #[test]
    fn failed_preset_install_clears_deferred_unowned_spool_events() {
        let root = tempfile::tempdir().unwrap();
        let repository = repository(&root.path().join("db"));
        let bridge = bridge(
            root.path(),
            repository.clone(),
            Arc::new(RecordingEmitter::default()),
        );
        let id = AgentIntegrationId::parse("kimi-windows").unwrap();
        let profile = repository.get(&id).unwrap();
        let install_started_at = now_millis();
        let outcome = bridge.install(&profile, install_started_at).unwrap();
        let event_path = bridge
            .spool_dir
            .join("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.json");
        let event = serde_json::json!({
            "profileId": "kimi-windows",
            "nativeEvent": "Stop",
            "taskId": "session-orphan",
            "sourceEventId": "event-orphan",
            "occurredAt": install_started_at + 1,
        });
        fs::write(&event_path, serde_json::to_vec(&event).unwrap()).unwrap();

        assert_eq!(bridge.scan_pending().unwrap(), 0);
        assert!(event_path.exists());
        outcome.mutation.rollback().unwrap();
        bridge.finish_install(&id, false);
        bridge.scan_pending().unwrap();

        assert!(!event_path.exists());
        assert!(repository.get_installation(&id).unwrap().is_none());
        bridge.shutdown();
    }

    #[test]
    fn kimi_and_qoder_allow_unrelated_user_edits_then_remove_only_owned_hooks() {
        for adapter in [PresetAgentAdapterId::Kimi, PresetAgentAdapterId::Qoderwork] {
            let root = tempfile::tempdir().unwrap();
            let repository = repository(&root.path().join("db"));
            let bridge = bridge(
                root.path(),
                repository.clone(),
                Arc::new(RecordingEmitter::default()),
            );
            let id = AgentIntegrationId::parse(match adapter {
                PresetAgentAdapterId::Kimi => "kimi-windows",
                PresetAgentAdapterId::Qoderwork => "qoderwork-windows",
                PresetAgentAdapterId::Trae => unreachable!(),
                PresetAgentAdapterId::Cursor => unreachable!(),
            })
            .unwrap();
            let profile = repository.get(&id).unwrap();
            let descriptor = bridge.descriptor(&profile).unwrap();
            fs::create_dir_all(descriptor.config_path.parent().unwrap()).unwrap();
            let source: &[u8] = match adapter {
                PresetAgentAdapterId::Kimi => {
                    b"# keep this comment\napi_key = \"do-not-log\"\n[unrelated]\nenabled = true\n"
                }
                PresetAgentAdapterId::Qoderwork => {
                    br#"{"secret":"do-not-log","unrelated":{"enabled":true}}"#
                }
                PresetAgentAdapterId::Trae => unreachable!(),
                PresetAgentAdapterId::Cursor => unreachable!(),
            };
            fs::write(&descriptor.config_path, source).unwrap();
            let outcome = bridge.install(&profile, now_millis()).unwrap();
            let installed = fs::read(&descriptor.config_path).unwrap();
            assert!(inspect_descriptor(&descriptor, &installed).unwrap());

            let with_user_edit = match adapter {
                PresetAgentAdapterId::Kimi => {
                    let mut text = String::from_utf8(installed).unwrap();
                    text.push_str("\n[user_added]\nvalue = 42\n");
                    text.into_bytes()
                }
                PresetAgentAdapterId::Qoderwork => {
                    let mut json: serde_json::Value = serde_json::from_slice(&installed).unwrap();
                    json["userAdded"] = serde_json::json!({"value": 42});
                    serde_json::to_vec_pretty(&json).unwrap()
                }
                PresetAgentAdapterId::Trae => unreachable!(),
                PresetAgentAdapterId::Cursor => unreachable!(),
            };
            fs::write(&descriptor.config_path, &with_user_edit).unwrap();
            bridge
                .validate_installation(&profile, &outcome.installation)
                .unwrap();
            bridge
                .uninstall(&profile, &outcome.installation, now_millis())
                .unwrap();
            let removed = fs::read(&descriptor.config_path).unwrap();
            assert!(!inspect_descriptor(&descriptor, &removed).unwrap());
            let text = String::from_utf8_lossy(&removed);
            assert!(text.contains("do-not-log"));
            assert!(text.contains("userAdded") || text.contains("user_added"));
            if matches!(adapter, PresetAgentAdapterId::Kimi) {
                assert!(text.contains("# keep this comment"));
            }
            bridge.shutdown();
        }
    }

    #[cfg(windows)]
    #[test]
    fn vendor_sensitive_stdin_is_reduced_to_the_allowlisted_spool_schema() {
        let root = tempfile::tempdir().unwrap();
        let spool = root.path().join("spool");
        let mut child = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("agent-hooks")
                    .join(PROFILE_EVENT_SCRIPT_NAME),
            )
            .args([
                "-ProfileId",
                "kimi-windows",
                "-NativeEvent",
                "Stop",
                "-SpoolDirectory",
            ])
            .arg(&spool)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let sensitive = br#"{"hook_event_name":"Stop","session_id":"session-1","last_assistant_message":"  Safe profile reply  ","prompt":"top secret prompt","tool_input":{"token":"secret-token"},"unknown":"private"}"#;
        child.stdin.take().unwrap().write_all(sensitive).unwrap();
        assert!(child.wait().unwrap().success());
        let event_path = fs::read_dir(&spool)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let bytes = fs::read(event_path).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 6);
        let serialized = String::from_utf8(bytes).unwrap();
        assert!(!serialized.contains("prompt"));
        assert!(!serialized.contains("secret-token"));
        assert_eq!(value["taskId"], "session-1");
        assert_eq!(value["latestReplyPreview"], "Safe profile reply");

        let cursor_spool = root.path().join("cursor-spool");
        let mut cursor = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("agent-hooks")
                    .join(PROFILE_EVENT_SCRIPT_NAME),
            )
            .args([
                "-ProfileId",
                "cursor-windows",
                "-NativeEvent",
                "afterAgentResponse",
                "-SpoolDirectory",
            ])
            .arg(&cursor_spool)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let sensitive = br#"{"hook_event_name":"afterAgentResponse","conversation_id":"cursor-session","generation_id":"cursor-generation","text":"  Safe Cursor reply  ","prompt":"private user input","thought":"private reasoning","tool_output":"private tool output"}"#;
        cursor.stdin.take().unwrap().write_all(sensitive).unwrap();
        assert!(cursor.wait().unwrap().success());
        let cursor_event = fs::read_dir(&cursor_spool)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let cursor_bytes = fs::read(cursor_event).unwrap();
        let cursor_value: serde_json::Value = serde_json::from_slice(&cursor_bytes).unwrap();
        let serialized = String::from_utf8(cursor_bytes).unwrap();
        assert_eq!(cursor_value["taskId"], "cursor-session");
        assert_eq!(cursor_value["latestReplyPreview"], "Safe Cursor reply");
        assert!(!serialized.contains("private user input"));
        assert!(!serialized.contains("private reasoning"));
        assert!(!serialized.contains("private tool output"));
    }
}
