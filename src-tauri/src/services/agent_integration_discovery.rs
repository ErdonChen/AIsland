use crate::contracts::{
    AgentEnvironment, AgentIntegrationDiscoveryCandidate, AgentIntegrationDiscoveryEvidence,
    AgentIntegrationDiscoveryKind, AgentIntegrationDiscoveryResult, AgentIntegrationDiscoveryState,
    CommandError, PresetAgentAdapterId,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub trait AgentIntegrationDiscoveryProbe: Send + Sync {
    fn running_process_names(&self) -> Result<Vec<String>, CommandError>;
    fn path_exists(&self, path: &Path) -> bool;
}

pub struct AgentIntegrationDiscoveryService {
    windows_home: PathBuf,
    roaming_app_data: PathBuf,
    local_app_data: PathBuf,
    probe: Arc<dyn AgentIntegrationDiscoveryProbe>,
}

pub struct SystemAgentIntegrationDiscoveryProbe;

impl AgentIntegrationDiscoveryProbe for SystemAgentIntegrationDiscoveryProbe {
    fn running_process_names(&self) -> Result<Vec<String>, CommandError> {
        crate::services::agent_status_watcher::running_process_base_names()
    }

    fn path_exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

struct CandidateDefinition {
    id: &'static str,
    display_name: &'static str,
    process_names: &'static [&'static str],
    configuration_paths: Vec<PathBuf>,
    application_paths: Vec<PathBuf>,
    integration_kind: AgentIntegrationDiscoveryKind,
    state: AgentIntegrationDiscoveryState,
    preset_id: Option<PresetAgentAdapterId>,
    reason_code: Option<&'static str>,
}

impl AgentIntegrationDiscoveryService {
    pub fn new(
        windows_home: PathBuf,
        roaming_app_data: PathBuf,
        local_app_data: PathBuf,
        probe: Arc<dyn AgentIntegrationDiscoveryProbe>,
    ) -> Self {
        Self {
            windows_home,
            roaming_app_data,
            local_app_data,
            probe,
        }
    }

    pub fn discover(
        &self,
        scanned_at: i64,
    ) -> Result<AgentIntegrationDiscoveryResult, CommandError> {
        let running = self
            .probe
            .running_process_names()?
            .into_iter()
            .map(|name| name.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        let candidates = self
            .definitions()
            .into_iter()
            .filter_map(|definition| {
                let mut evidence = Vec::new();
                if definition
                    .process_names
                    .iter()
                    .any(|name| running.contains(&name.to_ascii_lowercase()))
                {
                    evidence.push(AgentIntegrationDiscoveryEvidence::RunningProcess);
                }
                if definition
                    .configuration_paths
                    .iter()
                    .any(|path| self.probe.path_exists(path))
                {
                    evidence.push(AgentIntegrationDiscoveryEvidence::Configuration);
                }
                if definition
                    .application_paths
                    .iter()
                    .any(|path| self.probe.path_exists(path))
                {
                    evidence.push(AgentIntegrationDiscoveryEvidence::InstalledApplication);
                }
                (!evidence.is_empty()).then(|| AgentIntegrationDiscoveryCandidate {
                    id: definition.id.into(),
                    display_name: definition.display_name.into(),
                    environment: AgentEnvironment::Windows,
                    integration_kind: definition.integration_kind,
                    state: definition.state,
                    preset_id: definition.preset_id,
                    evidence,
                    reason_code: definition.reason_code.map(str::to_owned),
                })
            })
            .collect();
        Ok(AgentIntegrationDiscoveryResult {
            candidates,
            scanned_at,
        })
    }

    fn definitions(&self) -> Vec<CandidateDefinition> {
        vec![
            self.built_in("codex", "Codex", &["codex.exe"], &[".codex"], &[]),
            self.built_in(
                "claude",
                "Claude",
                &["claude.exe"],
                &[".claude"],
                &["Claude", "Claude-3p"],
            ),
            self.built_in(
                "hermes",
                "Hermes",
                &["hermes.exe"],
                &[".hermes"],
                &["hermes"],
            ),
            self.built_in(
                "workbuddy",
                "WorkBuddy",
                &["workbuddy.exe"],
                &[".workbuddy", ".workbuddy-ai"],
                &["WorkBuddy", "@genieworkbuddy-desktop-updater"],
            ),
            CandidateDefinition {
                id: "kimi",
                display_name: "Kimi Code",
                process_names: &[
                    "kimi.exe",
                    "kimi-code.exe",
                    "kimicode.exe",
                    "kimiwork.exe",
                    "kimi work.exe",
                ],
                configuration_paths: vec![
                    self.windows_home.join(".kimi-code"),
                    self.roaming_app_data
                        .join("kimi-desktop/daimon-share/daimon/runtime/kimi-code"),
                ],
                application_paths: vec![
                    self.local_app_data.join("kimi-desktop-updater"),
                    self.roaming_app_data.join("kimi-desktop"),
                    self.local_app_data.join("Programs/Kimi Code/Kimi Code.exe"),
                    self.local_app_data.join("Programs/Kimi/Kimi.exe"),
                ],
                integration_kind: AgentIntegrationDiscoveryKind::Preset,
                state: AgentIntegrationDiscoveryState::ReadyToInstall,
                preset_id: Some(PresetAgentAdapterId::Kimi),
                reason_code: None,
            },
            CandidateDefinition {
                id: "trae",
                display_name: "TRAE",
                process_names: &[
                    "trae.exe",
                    "trae cn.exe",
                    "trae solo.exe",
                    "trae solo cn.exe",
                    "traework.exe",
                    "traework cn.exe",
                    "trae work.exe",
                    "trae work cn.exe",
                ],
                configuration_paths: vec![
                    self.windows_home.join(".trae"),
                    self.roaming_app_data.join("Trae"),
                    self.roaming_app_data.join("TRAE SOLO CN"),
                ],
                application_paths: vec![self.local_app_data.join("Programs/Trae/Trae.exe")],
                integration_kind: AgentIntegrationDiscoveryKind::Preset,
                state: AgentIntegrationDiscoveryState::DetectionPending,
                preset_id: Some(PresetAgentAdapterId::Trae),
                reason_code: Some("traeHooksVersionOrConfigUnavailable"),
            },
            CandidateDefinition {
                id: "qoderwork",
                display_name: "QoderWork",
                process_names: &["qoder.exe", "qoderwork.exe", "qwenworkcn.exe"],
                configuration_paths: vec![
                    self.windows_home.join(".qoder"),
                    self.roaming_app_data.join("QwenWorkCN"),
                ],
                application_paths: vec![
                    self.local_app_data.join("Programs/Qoder/Qoder.exe"),
                    self.local_app_data.join("Programs/QoderWork/QoderWork.exe"),
                    self.local_app_data.join("Programs/QwenWorkCN"),
                ],
                integration_kind: AgentIntegrationDiscoveryKind::Preset,
                state: AgentIntegrationDiscoveryState::ReadyToInstall,
                preset_id: Some(PresetAgentAdapterId::Qoderwork),
                reason_code: None,
            },
            CandidateDefinition {
                id: "cursor",
                display_name: "Cursor",
                process_names: &["cursor.exe"],
                configuration_paths: vec![self.windows_home.join(".cursor")],
                application_paths: vec![
                    self.local_app_data.join("Programs/cursor/Cursor.exe"),
                    self.roaming_app_data.join("Cursor"),
                ],
                integration_kind: AgentIntegrationDiscoveryKind::Preset,
                state: AgentIntegrationDiscoveryState::ReadyToInstall,
                preset_id: Some(PresetAgentAdapterId::Cursor),
                reason_code: None,
            },
        ]
    }

    fn built_in(
        &self,
        id: &'static str,
        display_name: &'static str,
        process_names: &'static [&'static str],
        configuration_names: &[&str],
        local_application_names: &[&str],
    ) -> CandidateDefinition {
        CandidateDefinition {
            id,
            display_name,
            process_names,
            configuration_paths: configuration_names
                .iter()
                .map(|name| self.windows_home.join(name))
                .collect(),
            application_paths: local_application_names
                .iter()
                .map(|name| self.local_app_data.join(name))
                .collect(),
            integration_kind: AgentIntegrationDiscoveryKind::BuiltIn,
            state: AgentIntegrationDiscoveryState::Automatic,
            preset_id: None,
            reason_code: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{
        AgentEnvironment, AgentIntegrationDiscoveryEvidence, AgentIntegrationDiscoveryKind,
        AgentIntegrationDiscoveryState, PresetAgentAdapterId,
    };
    use std::fs;

    struct FixtureProbe {
        processes: Vec<String>,
    }

    impl AgentIntegrationDiscoveryProbe for FixtureProbe {
        fn running_process_names(&self) -> Result<Vec<String>, CommandError> {
            Ok(self.processes.clone())
        }

        fn path_exists(&self, path: &Path) -> bool {
            path.exists()
        }
    }

    #[test]
    fn discovers_only_exact_known_candidates_without_mutating_their_configuration() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let roaming = root.path().join("roaming");
        let local = root.path().join("local");
        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::create_dir_all(home.join(".kimi-code")).unwrap();
        fs::create_dir_all(local.join("Programs/cursor")).unwrap();
        let sentinel = home.join(".kimi-code/config.toml");
        fs::write(&sentinel, b"vendor = 'unchanged'\n").unwrap();
        fs::write(local.join("Programs/cursor/Cursor.exe"), b"fixture").unwrap();

        let service = AgentIntegrationDiscoveryService::new(
            home,
            roaming,
            local,
            Arc::new(FixtureProbe {
                processes: vec![
                    "QODERWORK.EXE".into(),
                    "trae.exe".into(),
                    "cursor-helper.exe".into(),
                ],
            }),
        );

        let result = service.discover(1_234).unwrap();

        assert_eq!(result.scanned_at, 1_234);
        assert_eq!(
            result
                .candidates
                .iter()
                .map(|candidate| candidate.id.as_str())
                .collect::<Vec<_>>(),
            ["codex", "kimi", "trae", "qoderwork", "cursor"]
        );
        let codex = &result.candidates[0];
        assert_eq!(codex.environment, AgentEnvironment::Windows);
        assert_eq!(
            codex.integration_kind,
            AgentIntegrationDiscoveryKind::BuiltIn
        );
        assert_eq!(codex.state, AgentIntegrationDiscoveryState::Automatic);
        assert_eq!(
            codex.evidence,
            [AgentIntegrationDiscoveryEvidence::Configuration]
        );
        let kimi = &result.candidates[1];
        assert_eq!(kimi.preset_id, Some(PresetAgentAdapterId::Kimi));
        assert_eq!(kimi.state, AgentIntegrationDiscoveryState::ReadyToInstall);
        let trae = &result.candidates[2];
        assert_eq!(
            trae.reason_code.as_deref(),
            Some("traeHooksVersionOrConfigUnavailable")
        );
        assert_eq!(
            result.candidates[3].preset_id,
            Some(PresetAgentAdapterId::Qoderwork)
        );
        assert_eq!(
            result.candidates[4].state,
            AgentIntegrationDiscoveryState::ReadyToInstall
        );
        assert_eq!(
            result.candidates[4].preset_id,
            Some(PresetAgentAdapterId::Cursor)
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"vendor = 'unchanged'\n");
    }

    #[test]
    fn recognizes_the_bounded_windows_vendor_locations_used_by_the_desktop_apps() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let roaming = root.path().join("roaming");
        let local = root.path().join("local");
        for path in [
            local.join("Claude"),
            local.join("hermes"),
            local.join("WorkBuddy"),
            local.join("kimi-desktop-updater"),
            roaming.join("TRAE SOLO CN"),
            roaming.join("Cursor"),
        ] {
            fs::create_dir_all(path).unwrap();
        }
        let service = AgentIntegrationDiscoveryService::new(
            home,
            roaming,
            local,
            Arc::new(FixtureProbe {
                processes: Vec::new(),
            }),
        );

        assert_eq!(
            service
                .discover(5)
                .unwrap()
                .candidates
                .into_iter()
                .map(|candidate| candidate.id)
                .collect::<Vec<_>>(),
            ["claude", "hermes", "workbuddy", "kimi", "trae", "cursor"]
        );
    }

    #[test]
    fn recognizes_every_supported_running_preset_process_alias() {
        let root = tempfile::tempdir().unwrap();
        let service = AgentIntegrationDiscoveryService::new(
            root.path().join("home"),
            root.path().join("roaming"),
            root.path().join("local"),
            Arc::new(FixtureProbe {
                processes: vec![
                    "KIMI-CODE.EXE".into(),
                    "TRAE SOLO CN.EXE".into(),
                    "QODERWORK.EXE".into(),
                ],
            }),
        );

        assert_eq!(
            service
                .discover(6)
                .unwrap()
                .candidates
                .into_iter()
                .map(|candidate| candidate.id)
                .collect::<Vec<_>>(),
            ["kimi", "trae", "qoderwork"]
        );
    }

    #[test]
    fn recognizes_traework_cn_as_part_of_the_trae_family() {
        let root = tempfile::tempdir().unwrap();
        let service = AgentIntegrationDiscoveryService::new(
            root.path().join("home"),
            root.path().join("roaming"),
            root.path().join("local"),
            Arc::new(FixtureProbe {
                processes: vec!["TRAEWORK CN.EXE".into()],
            }),
        );

        assert_eq!(
            service
                .discover(7)
                .unwrap()
                .candidates
                .into_iter()
                .map(|candidate| candidate.id)
                .collect::<Vec<_>>(),
            ["trae"]
        );
    }
}
