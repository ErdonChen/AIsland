#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{
        AgentEnvironment, AgentId, AgentStatus, AgentTriggerStatus, ReminderSound,
        SaveReminderRuleInput, ServiceHealthState,
    };
    use crate::repositories::{
        agents::AgentRepository, diagnostics::DiagnosticsRepository, reminders::ReminderRepository,
        service_health::ServiceHealthRepository,
    };
    use crate::services::reminder_scheduler::{ReminderService, SystemReminderClock};
    use crate::services::EventEmitterPort;
    use crate::storage::Storage;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    struct FakeLegacyAgentPresenceSource {
        running: Mutex<Vec<AgentId>>,
        checks: std::sync::atomic::AtomicUsize,
    }

    impl LegacyAgentPresenceSource for FakeLegacyAgentPresenceSource {
        fn running_agents(&self) -> Result<Vec<AgentId>, crate::contracts::CommandError> {
            self.checks
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self.running.lock().unwrap().clone())
        }
    }

    struct FakeNativeAgentActivitySource {
        activities: Mutex<Vec<NativeAgentActivity>>,
    }

    impl NativeAgentActivitySource for FakeNativeAgentActivitySource {
        fn latest_activity(
            &self,
            agent_id: AgentId,
            _now: i64,
        ) -> Result<Option<NativeAgentActivity>, crate::contracts::CommandError> {
            Ok(self
                .activities
                .lock()
                .unwrap()
                .iter()
                .find(|activity| activity.agent_id == agent_id)
                .cloned())
        }
    }

    #[derive(Default)]
    struct CapturingEmitter(Mutex<Vec<(&'static str, serde_json::Value)>>);

    impl EventEmitterPort for CapturingEmitter {
        fn emit(
            &self,
            event_name: &'static str,
            payload: serde_json::Value,
        ) -> Result<(), crate::contracts::CommandError> {
            self.0.lock().unwrap().push((event_name, payload));
            Ok(())
        }
    }

    #[derive(Default)]
    struct ClassificationCheckingEmitter {
        watcher: Mutex<Option<std::sync::Weak<AgentStatusWatcher>>>,
        payloads: Mutex<Vec<serde_json::Value>>,
    }

    impl EventEmitterPort for ClassificationCheckingEmitter {
        fn emit(
            &self,
            event_name: &'static str,
            payload: serde_json::Value,
        ) -> Result<(), crate::contracts::CommandError> {
            if event_name == AGENT_STATE_CHANGED {
                let watcher = self
                    .watcher
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(std::sync::Weak::upgrade)
                    .expect("watcher remains alive while initial_scan emits");
                assert_eq!(
                    watcher.source_states.lock().unwrap().len(),
                    LOCKED_STATUS_FILES.len(),
                    "reload hints must wait until all locked sources are classified"
                );
                self.payloads.lock().unwrap().push(payload);
            }
            Ok(())
        }
    }

    fn fixture(
        name: &str,
        agent: &str,
        environment: &str,
        status: &str,
        event_id: &str,
    ) -> Vec<u8> {
        serde_json::json!({
            "schema_version": 1, "event_id": event_id, "agent": agent,
            "environment": environment, "task_id": "task-1", "status": status,
            "occurred_at": 1_000, "sequence": 1, "task_title": name
        })
        .to_string()
        .into_bytes()
    }

    fn fixture_with_sequence(
        name: &str,
        agent: &str,
        environment: &str,
        status: &str,
        event_id: &str,
        sequence: u64,
        occurred_at: i64,
    ) -> Vec<u8> {
        serde_json::json!({
            "schema_version": 1, "event_id": event_id, "agent": agent,
            "environment": environment, "task_id": "task-1", "status": status,
            "occurred_at": occurred_at, "sequence": sequence, "task_title": name
        })
        .to_string()
        .into_bytes()
    }

    fn fixture_for_task(
        name: &str,
        task_id: &str,
        status: &str,
        event_id: &str,
        sequence: u64,
        occurred_at: i64,
    ) -> Vec<u8> {
        serde_json::json!({
            "schema_version": 1, "event_id": event_id, "agent": "codex",
            "environment": "windows", "task_id": task_id, "status": status,
            "occurred_at": occurred_at, "sequence": sequence, "task_title": name
        })
        .to_string()
        .into_bytes()
    }

    fn watcher_for_storage(
        storage: Arc<Storage>,
        status_dir: PathBuf,
        emitter: Arc<CapturingEmitter>,
    ) -> AgentStatusWatcher {
        let reminders = ReminderService::new(
            ReminderRepository::new(storage.clone()),
            Arc::new(SystemReminderClock),
            emitter.clone(),
        )
        .0;
        AgentStatusWatcher::new(
            AgentRepository::new(storage.clone()),
            reminders,
            ServiceHealthRepository::new(storage.clone()),
            DiagnosticsRepository::new(storage),
            emitter,
            status_dir,
        )
    }

    fn watcher() -> (
        tempfile::TempDir,
        AgentStatusWatcher,
        Arc<CapturingEmitter>,
        AgentRepository,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let emitter = Arc::new(CapturingEmitter::default());
        let reminders = ReminderService::new(
            ReminderRepository::new(storage.clone()),
            Arc::new(SystemReminderClock),
            emitter.clone(),
        )
        .0;
        let agents = AgentRepository::new(storage.clone());
        let status_dir = directory.path().join("agent-status");
        let watcher = AgentStatusWatcher::new(
            agents.clone(),
            reminders,
            ServiceHealthRepository::new(storage.clone()),
            DiagnosticsRepository::new(storage),
            emitter.clone(),
            status_dir,
        );
        (directory, watcher, emitter, agents)
    }

    fn watcher_with_presence(
        running: Vec<AgentId>,
    ) -> (
        tempfile::TempDir,
        AgentStatusWatcher,
        Arc<FakeLegacyAgentPresenceSource>,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let emitter = Arc::new(CapturingEmitter::default());
        let reminders = ReminderService::new(
            ReminderRepository::new(storage.clone()),
            Arc::new(SystemReminderClock),
            emitter.clone(),
        )
        .0;
        let presence = Arc::new(FakeLegacyAgentPresenceSource {
            running: Mutex::new(running),
            checks: std::sync::atomic::AtomicUsize::new(0),
        });
        let watcher = AgentStatusWatcher::new_with_presence_source(
            AgentRepository::new(storage.clone()),
            reminders,
            ServiceHealthRepository::new(storage.clone()),
            DiagnosticsRepository::new(storage),
            emitter,
            directory.path().join("agent-status"),
            presence.clone(),
        );
        (directory, watcher, presence)
    }

    #[tokio::test(start_paused = true)]
    async fn process_presence_checks_run_every_three_seconds() {
        let (_directory, watcher, presence) = watcher_with_presence(Vec::new());
        let watcher = Arc::new(watcher);
        let mut source = FakeSource {
            replacement_between_registration_and_scan: None,
            registered: None,
            registrations: 0,
        };
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let run = tokio::spawn({
            let watcher = watcher.clone();
            async move { watcher.run_with_source(&mut source, shutdown_rx).await }
        });
        tokio::task::yield_now().await;
        assert_eq!(presence.checks.load(std::sync::atomic::Ordering::SeqCst), 1);

        tokio::time::advance(std::time::Duration::from_secs(2)).await;
        tokio::task::yield_now().await;
        assert_eq!(presence.checks.load(std::sync::atomic::Ordering::SeqCst), 1);
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(presence.checks.load(std::sync::atomic::Ordering::SeqCst), 2);

        presence.running.lock().unwrap().push(AgentId::Codex);
        tokio::time::advance(std::time::Duration::from_secs(2)).await;
        tokio::task::yield_now().await;
        assert_eq!(presence.checks.load(std::sync::atomic::Ordering::SeqCst), 2);
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(presence.checks.load(std::sync::atomic::Ordering::SeqCst), 3);
        assert_eq!(
            watcher.snapshot(now_millis()).unwrap().agents[0].aggregate_status,
            AgentStatus::Idle
        );

        shutdown_tx.send(true).unwrap();
        run.await.unwrap();
    }

    fn watcher_with_native_activity(
        activities: Vec<NativeAgentActivity>,
    ) -> (
        tempfile::TempDir,
        AgentStatusWatcher,
        Arc<FakeNativeAgentActivitySource>,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let emitter = Arc::new(CapturingEmitter::default());
        let reminders = ReminderService::new(
            ReminderRepository::new(storage.clone()),
            Arc::new(SystemReminderClock),
            emitter.clone(),
        )
        .0;
        let mut running = vec![AgentId::Codex, AgentId::Workbuddy];
        for activity in &activities {
            if !running.contains(&activity.agent_id) {
                running.push(activity.agent_id.clone());
            }
        }
        let presence = Arc::new(FakeLegacyAgentPresenceSource {
            running: Mutex::new(running),
            checks: std::sync::atomic::AtomicUsize::new(0),
        });
        let activity = Arc::new(FakeNativeAgentActivitySource {
            activities: Mutex::new(activities),
        });
        let watcher = AgentStatusWatcher::new_with_sources(
            AgentRepository::new(storage.clone()),
            reminders,
            ServiceHealthRepository::new(storage.clone()),
            DiagnosticsRepository::new(storage),
            emitter,
            directory.path().join("agent-status"),
            presence,
            activity.clone(),
        );
        (directory, watcher, activity)
    }

    #[test]
    fn real_native_activity_replaces_idle_presence_and_exposes_latest_reply() {
        let (_directory, watcher, _activity) = watcher_with_native_activity(vec![
            NativeAgentActivity {
                agent_id: AgentId::Codex,
                session_id: "codex-session".into(),
                status: AgentStatus::Running,
                title: Some("Implement status tracking".into()),
                latest_reply: Some("Codex 正在处理真实任务".into()),
                occurred_at: 1_000,
                source_bytes: 100,
            },
            NativeAgentActivity {
                agent_id: AgentId::Workbuddy,
                session_id: "workbuddy-session".into(),
                status: AgentStatus::Completed,
                title: Some("Review result".into()),
                latest_reply: Some("WorkBuddy 已完成真实回复".into()),
                occurred_at: 1_001,
                source_bytes: 200,
            },
        ]);

        watcher.reconcile_process_presence(1_100).unwrap();
        let snapshot = watcher.snapshot(1_100).unwrap();
        let codex = snapshot
            .agents
            .iter()
            .find(|agent| agent.agent_id == AgentId::Codex)
            .unwrap();
        let workbuddy = snapshot
            .agents
            .iter()
            .find(|agent| agent.agent_id == AgentId::Workbuddy)
            .unwrap();

        assert_eq!(codex.aggregate_status, AgentStatus::Running);
        assert_eq!(codex.environments.len(), 1);
        assert_eq!(
            codex.environments[0].latest_reply_preview.as_deref(),
            Some("Codex 正在处理真实任务")
        );
        assert_eq!(workbuddy.aggregate_status, AgentStatus::Completed);
        assert_eq!(workbuddy.environments.len(), 1);
        assert_eq!(
            workbuddy.environments[0].latest_reply_preview.as_deref(),
            Some("WorkBuddy 已完成真实回复")
        );
    }

    #[test]
    fn hermes_native_activity_replaces_process_idle_and_exposes_latest_reply() {
        let (_directory, watcher, _activity) =
            watcher_with_native_activity(vec![NativeAgentActivity {
                agent_id: AgentId::Hermes,
                session_id: "hermes-session".into(),
                status: AgentStatus::Completed,
                title: None,
                latest_reply: Some("Hermes 原生回复".into()),
                occurred_at: 1_000,
                source_bytes: 0,
            }]);

        watcher.reconcile_process_presence(1_100).unwrap();
        let hermes = watcher
            .snapshot(1_100)
            .unwrap()
            .agents
            .into_iter()
            .find(|agent| agent.agent_id == AgentId::Hermes)
            .unwrap();

        assert_eq!(hermes.aggregate_status, AgentStatus::Completed);
        assert_eq!(
            hermes.environments[0].latest_reply_preview.as_deref(),
            Some("Hermes 原生回复")
        );
    }

    #[test]
    fn claude_native_activity_replaces_process_idle_and_exposes_latest_reply() {
        let (_directory, watcher, _activity) =
            watcher_with_native_activity(vec![NativeAgentActivity {
                agent_id: AgentId::Claude,
                session_id: "local_claude-session".into(),
                status: AgentStatus::Completed,
                title: None,
                latest_reply: Some("Claude 原生回复".into()),
                occurred_at: 1_000,
                source_bytes: 100,
            }]);

        watcher.reconcile_process_presence(1_100).unwrap();
        let snapshot = watcher.snapshot(1_100).unwrap();
        let claude = snapshot
            .agents
            .into_iter()
            .find(|agent| agent.agent_id == AgentId::Claude)
            .expect("Claude should remain visible while its process is detected");

        assert_eq!(claude.aggregate_status, AgentStatus::Completed);
        assert_eq!(
            claude.environments[0].latest_reply_preview.as_deref(),
            Some("Claude 原生回复")
        );
    }

    #[test]
    fn missing_native_refresh_falls_back_to_idle_without_losing_the_latest_reply() {
        let (_directory, watcher, activity) =
            watcher_with_native_activity(vec![NativeAgentActivity {
                agent_id: AgentId::Codex,
                session_id: "codex-session".into(),
                status: AgentStatus::Running,
                title: Some("Live task".into()),
                latest_reply: Some("Latest safe reply".into()),
                occurred_at: 1_000,
                source_bytes: 100,
            }]);
        watcher.reconcile_process_presence(1_100).unwrap();

        activity.activities.lock().unwrap().clear();
        watcher.reconcile_process_presence(1_200).unwrap();
        let snapshot = watcher.snapshot(1_200).unwrap();
        let codex = snapshot
            .agents
            .iter()
            .find(|agent| agent.agent_id == AgentId::Codex)
            .unwrap();

        assert_eq!(codex.aggregate_status, AgentStatus::Idle);
        assert_eq!(
            codex.environments[0].latest_reply_preview.as_deref(),
            Some("Latest safe reply")
        );
    }

    #[test]
    fn unrelated_native_source_growth_does_not_append_duplicate_agent_events() {
        let (_directory, watcher, activity) =
            watcher_with_native_activity(vec![NativeAgentActivity {
                agent_id: AgentId::Codex,
                session_id: "codex-session".into(),
                status: AgentStatus::Running,
                title: Some("Live task".into()),
                latest_reply: Some("Unchanged safe reply".into()),
                occurred_at: 1_000,
                source_bytes: 100,
            }]);
        watcher.reconcile_process_presence(1_100).unwrap();
        let tasks_before = watcher.repository.list_tasks().unwrap();

        activity.activities.lock().unwrap()[0].source_bytes = 200;

        assert_eq!(watcher.reconcile_process_presence(1_200).unwrap(), 0);
        assert_eq!(watcher.repository.list_tasks().unwrap(), tasks_before);
    }

    #[test]
    fn native_session_can_return_to_a_previously_observed_status() {
        let (_directory, watcher, activity) =
            watcher_with_native_activity(vec![NativeAgentActivity {
                agent_id: AgentId::Workbuddy,
                session_id: "workbuddy-session".into(),
                status: AgentStatus::Idle,
                title: Some("Same session".into()),
                latest_reply: Some("Same safe reply".into()),
                occurred_at: 1_000,
                source_bytes: 100,
            }]);
        watcher.reconcile_process_presence(1_100).unwrap();

        let mut activities = activity.activities.lock().unwrap();
        activities[0].status = AgentStatus::Running;
        activities[0].occurred_at = 2_000;
        drop(activities);
        watcher.reconcile_process_presence(2_100).unwrap();

        let mut activities = activity.activities.lock().unwrap();
        activities[0].status = AgentStatus::Idle;
        activities[0].occurred_at = 3_000;
        drop(activities);
        watcher.reconcile_process_presence(3_100).unwrap();

        let workbuddy = watcher
            .snapshot(3_100)
            .unwrap()
            .agents
            .into_iter()
            .find(|agent| agent.agent_id == AgentId::Workbuddy)
            .unwrap();
        assert_eq!(workbuddy.aggregate_status, AgentStatus::Idle);
    }

    #[test]
    fn opened_desktop_or_terminal_agents_are_idle_until_a_hook_reports_work_then_turn_offline() {
        let (_directory, watcher, presence) =
            watcher_with_presence(vec![AgentId::Codex, AgentId::Workbuddy]);

        watcher.reconcile_process_presence(1_000).unwrap();
        let running = watcher.snapshot(1_000).unwrap();
        for agent_id in [AgentId::Codex, AgentId::Workbuddy] {
            let agent = running
                .agents
                .iter()
                .find(|agent| agent.agent_id == agent_id)
                .unwrap();
            assert_eq!(agent.aggregate_status, AgentStatus::Idle);
            assert_eq!(agent.environments.len(), 1);
            assert_eq!(agent.environments[0].environment, AgentEnvironment::Windows);
        }

        presence.running.lock().unwrap().clear();
        watcher.reconcile_process_presence(2_000).unwrap();
        let stopped = watcher.snapshot(2_000).unwrap();
        for agent_id in [AgentId::Codex, AgentId::Workbuddy] {
            let agent = stopped
                .agents
                .iter()
                .find(|agent| agent.agent_id == agent_id)
                .unwrap();
            assert_eq!(agent.aggregate_status, AgentStatus::Offline);
            assert_eq!(agent.environments.len(), 1);
        }
    }

    #[test]
    fn windows_process_names_map_case_insensitively_to_the_locked_legacy_agents() {
        assert_eq!(agent_for_process_name("CODEX.EXE"), Some(AgentId::Codex));
        assert_eq!(agent_for_process_name("claude.exe"), Some(AgentId::Claude));
        assert_eq!(agent_for_process_name("Hermes.exe"), Some(AgentId::Hermes));
        assert_eq!(
            agent_for_process_name("WorkBuddy.exe"),
            Some(AgentId::Workbuddy)
        );
        assert_eq!(agent_for_process_name("ChatGPT.exe"), None);
        assert_eq!(agent_for_process_name("unrelated.exe"), None);
    }

    #[test]
    fn hook_completion_wins_over_the_process_presence_fallback() {
        let (directory, watcher, _presence) = watcher_with_presence(vec![AgentId::Codex]);
        watcher.reconcile_process_presence(1_000).unwrap();
        let status_dir = directory.path().join("agent-status");
        std::fs::create_dir(&status_dir).unwrap();
        let path = status_dir.join("codex-windows.json");
        std::fs::write(
            &path,
            fixture("Finished", "codex", "windows", "completed", "hook-event"),
        )
        .unwrap();
        watcher.process_path(&path, 1_000).unwrap();

        let snapshot = watcher.snapshot(1_000).unwrap();
        let codex = snapshot
            .agents
            .iter()
            .find(|agent| agent.agent_id == AgentId::Codex)
            .unwrap();
        assert_eq!(codex.aggregate_status, AgentStatus::Completed);
        assert_eq!(codex.environments.len(), 1);
        assert_eq!(codex.environments[0].source_event_id, "hook-event");
    }

    #[test]
    fn snapshot_shows_completion_for_two_seconds_then_restores_running_until_all_tasks_finish() {
        let (directory, watcher, _emitter, _repository) = watcher();
        let status_dir = directory.path().join("agent-status");
        std::fs::create_dir(&status_dir).unwrap();
        let path = status_dir.join("codex-windows.json");

        std::fs::write(
            &path,
            fixture_for_task(
                "Long task",
                "task-running",
                "running",
                "running-1",
                1,
                1_000,
            ),
        )
        .unwrap();
        watcher.process_path(&path, 1_000).unwrap();
        std::fs::write(
            &path,
            fixture_for_task(
                "Short task",
                "task-completed",
                "completed",
                "completed-1",
                1,
                1_001,
            ),
        )
        .unwrap();
        watcher.process_path(&path, 1_001).unwrap();

        let while_running = watcher.snapshot(1_001).unwrap();
        let codex = while_running
            .agents
            .iter()
            .find(|agent| agent.agent_id == AgentId::Codex)
            .unwrap();
        assert_eq!(codex.aggregate_status, AgentStatus::Completed);
        assert_eq!(codex.environments.len(), 2);

        let after_completion_flash = watcher.snapshot(3_001).unwrap();
        let codex = after_completion_flash
            .agents
            .iter()
            .find(|agent| agent.agent_id == AgentId::Codex)
            .unwrap();
        assert_eq!(codex.aggregate_status, AgentStatus::Running);

        std::fs::write(
            &path,
            fixture_for_task(
                "Long task",
                "task-running",
                "completed",
                "running-2",
                2,
                3_002,
            ),
        )
        .unwrap();
        watcher.process_path(&path, 3_002).unwrap();

        let all_completed = watcher.snapshot(5_002).unwrap();
        let codex = all_completed
            .agents
            .iter()
            .find(|agent| agent.agent_id == AgentId::Codex)
            .unwrap();
        assert_eq!(codex.aggregate_status, AgentStatus::Completed);
        assert!(codex
            .environments
            .iter()
            .all(|observation| observation.status == AgentStatus::Completed));
    }

    #[test]
    fn a_successful_process_snapshot_recovers_watcher_health_after_a_transient_failure() {
        let (_directory, watcher, _presence) = watcher_with_presence(Vec::new());
        watcher.presence_degraded.store(true, Ordering::Release);
        watcher.record_degraded("processSnapshot", 1_000);

        watcher.reconcile_process_presence(2_000).unwrap();

        let health = watcher.health.list().unwrap();
        assert_eq!(health.len(), 1);
        assert_eq!(health[0].service_id, AGENT_WATCHER_SERVICE_ID);
        assert_eq!(health[0].state, ServiceHealthState::Healthy);
        assert_eq!(health[0].checked_at, 2_000);
    }

    // Break caught: a partial/replaced status file would be emitted before its SQLite projection commits.
    #[test]
    fn replacement_projects_once_then_emits_the_exact_reload_hint() {
        let (directory, watcher, emitter, repository) = watcher();
        let status_dir = directory.path().join("agent-status");
        std::fs::create_dir(&status_dir).unwrap();
        let path = status_dir.join("codex-windows.json");
        std::fs::write(
            &path,
            fixture("Release", "codex", "windows", "completed", "event-1"),
        )
        .unwrap();

        assert!(matches!(
            watcher.process_path(&path, 1_000).unwrap(),
            crate::repositories::agents::ProjectionOutcome::Advanced { .. }
        ));
        assert!(matches!(
            watcher.process_path(&path, 1_000).unwrap(),
            crate::repositories::agents::ProjectionOutcome::Duplicate
        ));
        assert_eq!(repository.list_tasks().unwrap().len(), 1);
        let emitted = emitter.0.lock().unwrap();
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].0, AGENT_STATE_CHANGED);
        assert_eq!(
            emitted[0].1,
            serde_json::json!({
                "agentId": "codex",
                "environment": "windows",
                "sourceEventId": "event-1",
                "occurredAt": 1_000
            })
        );
    }

    // Break caught: bad input could stop the watcher or store a path/body in diagnostics.
    #[test]
    fn invalid_file_isolated_from_next_approved_file_and_diagnostic_is_bounded() {
        let (directory, watcher, _, repository) = watcher();
        let status_dir = directory.path().join("agent-status");
        std::fs::create_dir(&status_dir).unwrap();
        let bad = status_dir.join("codex-windows.json");
        let good = status_dir.join("hermes-windows.json");
        std::fs::write(&bad, b"{ not json").unwrap();
        std::fs::write(
            &good,
            fixture("ok", "hermes", "windows", "running", "event-2"),
        )
        .unwrap();

        assert!(watcher.process_path(&bad, 1_000).is_ok());
        assert!(matches!(
            watcher.process_path(&good, 1_000).unwrap(),
            crate::repositories::agents::ProjectionOutcome::Advanced { .. }
        ));
        assert_eq!(
            repository.list_tasks().unwrap()[0].agent_id,
            AgentId::Hermes
        );
        let diagnostics = watcher.diagnostics_for_test().list(10).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "watcher.fileInvalid");
        assert_eq!(
            diagnostics[0].parameters["agentName"],
            crate::contracts::SafeParameterValue::String("Codex".into())
        );
        assert!(diagnostics[0].parameters.get("fileNameHash").is_some());
        assert!(diagnostics[0].parameters.get("receivedAt").is_some());
    }

    // Break caught: deleting a source after it was observed would leave a stale running card.
    #[test]
    fn deleted_observed_source_projects_deterministic_offline_but_never_observed_stays_event_free()
    {
        let (directory, watcher, _, repository) = watcher();
        let status_dir = directory.path().join("agent-status");
        std::fs::create_dir(&status_dir).unwrap();
        let path = status_dir.join("codex-windows.json");
        std::fs::write(
            &path,
            fixture("Run", "codex", "windows", "running", "event-3"),
        )
        .unwrap();
        watcher.process_path(&path, 1_000).unwrap();
        std::fs::remove_file(&path).unwrap();
        watcher.process_path(&path, 1_001).unwrap();

        let mut tasks = repository.list_tasks().unwrap();
        let task = tasks.remove(0);
        assert_eq!(task.status, AgentStatus::Offline);
        assert_eq!(task.source_event_id, "offline:codex:windows:event-3");
        let snapshot = watcher.snapshot(2_000).unwrap();
        assert_eq!(snapshot.agents.len(), 4);
        assert_eq!(snapshot.agents[1].aggregate_status, AgentStatus::Offline);
        assert!(snapshot.agents[1].environments.is_empty());
    }

    // Break caught: an audited lower sequence must not replace the authoritative source used by deletion.
    #[test]
    fn out_of_order_file_cannot_steal_the_offline_projection_baseline() {
        let (directory, watcher, _, repository) = watcher();
        let status_dir = directory.path().join("agent-status");
        std::fs::create_dir(&status_dir).unwrap();
        let path = status_dir.join("codex-windows.json");
        std::fs::write(
            &path,
            fixture_with_sequence(
                "Current", "codex", "windows", "running", "event-10", 10, 1_000,
            ),
        )
        .unwrap();
        assert!(matches!(
            watcher.process_path(&path, 1_000).unwrap(),
            ProjectionOutcome::Advanced { .. }
        ));
        std::fs::write(
            &path,
            fixture_with_sequence(
                "Older",
                "codex",
                "windows",
                "completed",
                "event-9",
                9,
                1_001,
            ),
        )
        .unwrap();
        assert_eq!(
            watcher.process_path(&path, 1_001).unwrap(),
            ProjectionOutcome::IgnoredOutOfOrder
        );

        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            watcher.process_path(&path, 1_002).unwrap(),
            ProjectionOutcome::Advanced { .. }
        ));
        let task = repository.list_tasks().unwrap().remove(0);
        assert_eq!(task.status, AgentStatus::Offline);
        assert_eq!(task.source_event_id, "offline:codex:windows:event-10");
        assert_eq!(
            watcher.snapshot(1_002).unwrap().agents[0].aggregate_status,
            AgentStatus::Offline
        );
    }

    // Break caught: a duplicate replay with weaker fields must not replace the persisted event used by deletion.
    #[test]
    fn duplicate_replay_uses_the_persisted_authoritative_event_for_offline_projection() {
        let (directory, watcher, _, repository) = watcher();
        let status_dir = directory.path().join("agent-status");
        std::fs::create_dir(&status_dir).unwrap();
        let path = status_dir.join("codex-windows.json");
        std::fs::write(
            &path,
            fixture_with_sequence(
                "Authoritative",
                "codex",
                "windows",
                "running",
                "event-duplicate",
                10,
                1_000,
            ),
        )
        .unwrap();
        assert!(matches!(
            watcher.process_path(&path, 1_000).unwrap(),
            ProjectionOutcome::Advanced { .. }
        ));

        std::fs::write(
            &path,
            fixture_with_sequence(
                "Weaker replay",
                "codex",
                "windows",
                "completed",
                "event-duplicate",
                1,
                1_001,
            ),
        )
        .unwrap();
        assert_eq!(
            watcher.process_path(&path, 1_001).unwrap(),
            ProjectionOutcome::Duplicate
        );

        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            watcher.process_path(&path, 1_002).unwrap(),
            ProjectionOutcome::Advanced { .. }
        ));
        let task = repository.list_tasks().unwrap().remove(0);
        assert_eq!(task.status, AgentStatus::Offline);
        assert_eq!(
            task.source_event_id,
            "offline:codex:windows:event-duplicate"
        );
    }

    // Break caught: after restart a missing locked source must not leak its old SQLite task into a live card.
    #[test]
    fn restart_snapshot_filters_stale_rows_for_a_missing_locked_source_without_new_event() {
        let directory = tempfile::tempdir().unwrap();
        let status_dir = directory.path().join("agent-status");
        std::fs::create_dir(&status_dir).unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let emitter = Arc::new(CapturingEmitter::default());
        let path = status_dir.join("hermes-windows.json");
        std::fs::write(
            &path,
            fixture_with_sequence(
                "Persisted",
                "hermes",
                "windows",
                "running",
                "event-before-restart",
                3,
                2_000,
            ),
        )
        .unwrap();
        let first = watcher_for_storage(storage.clone(), status_dir.clone(), emitter.clone());
        first.process_path(&path, 2_000).unwrap();
        assert_eq!(
            AgentRepository::new(storage.clone())
                .list_tasks()
                .unwrap()
                .len(),
            1
        );
        drop(first);
        std::fs::remove_file(&path).unwrap();

        let restarted = watcher_for_storage(storage.clone(), status_dir, emitter);
        assert_eq!(restarted.initial_scan(2_001).unwrap(), 0);
        let snapshot = restarted.snapshot(2_001).unwrap();
        assert_eq!(snapshot.agents[1].aggregate_status, AgentStatus::Offline);
        assert!(snapshot.agents[1].environments.is_empty());
        assert_eq!(AgentRepository::new(storage).list_tasks().unwrap().len(), 1);
    }

    // Break caught: startup must not emit a partial snapshot before all seven locked sources are classified.
    #[test]
    fn initial_scan_emits_exact_reload_hints_for_each_advanced_source() {
        let directory = tempfile::tempdir().unwrap();
        let status_dir = directory.path().join("agent-status");
        std::fs::create_dir(&status_dir).unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let emitter = Arc::new(CapturingEmitter::default());
        let claude = status_dir.join("claude-windows.json");
        std::fs::write(
            &claude,
            fixture_with_sequence(
                "Old Claude",
                "claude",
                "windows",
                "running",
                "claude-before-restart",
                4,
                3_000,
            ),
        )
        .unwrap();
        let first = watcher_for_storage(storage.clone(), status_dir.clone(), emitter.clone());
        first.process_path(&claude, 3_000).unwrap();
        emitter.0.lock().unwrap().clear();
        drop(first);
        std::fs::remove_file(&claude).unwrap();

        let codex = status_dir.join("codex-windows.json");
        std::fs::write(
            &codex,
            fixture_with_sequence(
                "New Codex",
                "codex",
                "windows",
                "running",
                "codex-after-restart",
                1,
                3_001,
            ),
        )
        .unwrap();
        let restarted = watcher_for_storage(storage, status_dir, emitter.clone());
        assert_eq!(restarted.initial_scan(3_001).unwrap(), 1);

        let emitted = emitter.0.lock().unwrap();
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].0, AGENT_STATE_CHANGED);
        assert_eq!(
            emitted[0].1,
            serde_json::json!({
                "agentId": "codex",
                "environment": "windows",
                "sourceEventId": "codex-after-restart",
                "occurredAt": 3_001
            })
        );
    }

    #[test]
    fn initial_scan_classifies_all_seven_sources_before_emitting_any_reload_hint() {
        let directory = tempfile::tempdir().unwrap();
        let status_dir = directory.path().join("agent-status");
        std::fs::create_dir(&status_dir).unwrap();
        std::fs::write(
            status_dir.join("codex-windows.json"),
            fixture_with_sequence(
                "Ready after scan",
                "codex",
                "windows",
                "running",
                "classified-before-hint",
                1,
                3_100,
            ),
        )
        .unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let emitter = Arc::new(ClassificationCheckingEmitter::default());
        let reminders = ReminderService::new(
            ReminderRepository::new(storage.clone()),
            Arc::new(SystemReminderClock),
            emitter.clone(),
        )
        .0;
        let watcher = Arc::new(AgentStatusWatcher::new(
            AgentRepository::new(storage.clone()),
            reminders,
            ServiceHealthRepository::new(storage.clone()),
            DiagnosticsRepository::new(storage),
            emitter.clone(),
            status_dir,
        ));
        *emitter.watcher.lock().unwrap() = Some(Arc::downgrade(&watcher));

        assert_eq!(watcher.initial_scan(3_100).unwrap(), 1);
        assert_eq!(emitter.payloads.lock().unwrap().len(), 1);
    }

    struct FakeSource {
        replacement_between_registration_and_scan: Option<(PathBuf, Vec<u8>)>,
        registered: Option<tokio::sync::oneshot::Sender<()>>,
        registrations: usize,
    }

    impl NotifySource for FakeSource {
        fn register(
            &mut self,
            _parent: &Path,
            _sink: Arc<dyn Fn(PathBuf) + Send + Sync>,
        ) -> Result<(), String> {
            self.registrations += 1;
            if let Some((path, bytes)) = self.replacement_between_registration_and_scan.take() {
                std::fs::write(&path, bytes).unwrap();
            }
            if let Some(registered) = self.registered.take() {
                registered.send(()).ok();
            }
            Ok(())
        }
    }

    struct FailingSource {
        registrations: usize,
    }

    impl NotifySource for FailingSource {
        fn register(
            &mut self,
            _parent: &Path,
            _sink: Arc<dyn Fn(PathBuf) + Send + Sync>,
        ) -> Result<(), String> {
            self.registrations += 1;
            Err("registrationDenied".into())
        }
    }

    struct OverflowSource {
        status_dir: PathBuf,
        occurred_at: i64,
    }

    impl NotifySource for OverflowSource {
        fn register(
            &mut self,
            _parent: &Path,
            sink: Arc<dyn Fn(PathBuf) + Send + Sync>,
        ) -> Result<(), String> {
            let status_dir = self.status_dir.clone();
            let occurred_at = self.occurred_at;
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(30));
                let path = status_dir.join("codex-windows.json");
                std::fs::write(
                    &path,
                    fixture_with_sequence(
                        "Overflow final",
                        "codex",
                        "windows",
                        "running",
                        "overflow-final",
                        1,
                        occurred_at,
                    ),
                )
                .unwrap();
                for _ in 0..10_000 {
                    sink(path.clone());
                }
            });
            Ok(())
        }
    }

    struct TempNoiseSource {
        status_dir: PathBuf,
    }

    impl NotifySource for TempNoiseSource {
        fn register(
            &mut self,
            _parent: &Path,
            sink: Arc<dyn Fn(PathBuf) + Send + Sync>,
        ) -> Result<(), String> {
            for index in 0..10_000 {
                sink(self.status_dir.join(format!("ignored-{index}.tmp")));
            }
            Ok(())
        }
    }

    struct StaggeredSource {
        status_dir: PathBuf,
        occurred_at: i64,
    }

    impl NotifySource for StaggeredSource {
        fn register(
            &mut self,
            _parent: &Path,
            sink: Arc<dyn Fn(PathBuf) + Send + Sync>,
        ) -> Result<(), String> {
            let status_dir = self.status_dir.clone();
            let occurred_at = self.occurred_at;
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(20));
                let codex = status_dir.join("codex-windows.json");
                std::fs::write(
                    &codex,
                    fixture_with_sequence(
                        "First",
                        "codex",
                        "windows",
                        "running",
                        "codex-first",
                        1,
                        occurred_at,
                    ),
                )
                .unwrap();
                sink(codex);
                std::thread::sleep(std::time::Duration::from_millis(99));
                let hermes = status_dir.join("hermes-windows.json");
                std::fs::write(&hermes, b"{ incomplete").unwrap();
                sink(hermes.clone());
                std::thread::sleep(std::time::Duration::from_millis(50));
                std::fs::write(
                    hermes,
                    fixture_with_sequence(
                        "Second",
                        "hermes",
                        "windows",
                        "running",
                        "hermes-second",
                        1,
                        occurred_at,
                    ),
                )
                .unwrap();
            });
            Ok(())
        }
    }

    // Break caught: an event arriving while startup scans could be lost if registration came second.
    #[tokio::test]
    async fn listener_registers_before_initial_scan_and_keeps_intervening_replace_event() {
        let (directory, watcher, _, repository) = watcher();
        let watcher = Arc::new(watcher);
        let status_dir = directory.path().join("agent-status");
        std::fs::create_dir(&status_dir).unwrap();
        let path = status_dir.join("claude-windows.json");
        let mut payload: serde_json::Value = serde_json::from_slice(&fixture(
            "Done",
            "claude",
            "windows",
            "completed",
            "event-4",
        ))
        .unwrap();
        payload["occurred_at"] = serde_json::json!(crate::services::now_millis());
        let (registered_tx, registered_rx) = tokio::sync::oneshot::channel();
        let mut source = FakeSource {
            replacement_between_registration_and_scan: Some((
                path,
                payload.to_string().into_bytes(),
            )),
            registered: Some(registered_tx),
            registrations: 0,
        };
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let run = tokio::spawn({
            let watcher = watcher.clone();
            async move {
                watcher.run_with_source(&mut source, shutdown_rx).await;
                source.registrations
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), registered_rx)
            .await
            .expect("listener registration callback must run")
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(130)).await;
        shutdown_tx.send(true).unwrap();
        assert_eq!(run.await.unwrap(), 1);
        assert_eq!(
            repository.list_tasks().unwrap()[0].agent_id,
            AgentId::Claude
        );
    }

    // Break caught: a later file must retain its own full quiet window instead of sharing the first file's deadline.
    #[tokio::test]
    async fn each_canonical_file_gets_one_hundred_ms_from_its_first_hint() {
        let (directory, watcher, _, repository) = watcher();
        let watcher = Arc::new(watcher);
        let status_dir = directory.path().join("agent-status");
        let occurred_at = crate::services::now_millis();
        let mut source = StaggeredSource {
            status_dir,
            occurred_at,
        };
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let run = tokio::spawn({
            let watcher = watcher.clone();
            async move { watcher.run_with_source(&mut source, shutdown_rx).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if repository.list_tasks().unwrap().len() == 2 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("both independently coalesced files must project");
        shutdown_tx.send(true).unwrap();
        run.await.unwrap();

        let tasks = repository.list_tasks().unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].agent_id, AgentId::Codex);
        assert_eq!(tasks[1].agent_id, AgentId::Hermes);
        assert_eq!(tasks[1].source_event_id, "hermes-second");
    }

    // Break caught: a failed native registration must not reconnect or fall back to directory polling.
    #[tokio::test]
    async fn registration_failure_persists_degraded_health_and_exits_without_scan_or_retry() {
        let directory = tempfile::tempdir().unwrap();
        let status_dir = directory.path().join("agent-status");
        std::fs::create_dir(&status_dir).unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let emitter = Arc::new(CapturingEmitter::default());
        let path = status_dir.join("codex-windows.json");
        let occurred_at = crate::services::now_millis();
        std::fs::write(
            &path,
            fixture_with_sequence(
                "Must not poll",
                "codex",
                "windows",
                "running",
                "not-polled",
                1,
                occurred_at,
            ),
        )
        .unwrap();
        let watcher = Arc::new(watcher_for_storage(storage.clone(), status_dir, emitter));
        let mut source = FailingSource { registrations: 0 };
        let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            watcher.run_with_source(&mut source, shutdown_rx),
        )
        .await
        .expect("failed registration worker must exit");

        assert_eq!(source.registrations, 1);
        assert!(AgentRepository::new(storage.clone())
            .list_tasks()
            .unwrap()
            .is_empty());
        let health = ServiceHealthRepository::new(storage.clone())
            .list()
            .unwrap();
        let watcher_health = health
            .iter()
            .find(|row| row.service_id == AGENT_WATCHER_SERVICE_ID)
            .unwrap();
        assert_eq!(watcher_health.state, ServiceHealthState::Degraded);
        assert_eq!(
            watcher_health.parameters["reasonCode"],
            SafeParameterValue::String("registrationDenied".into())
        );
        let diagnostics = DiagnosticsRepository::new(storage).list(10).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "watcher.registrationFailed");
    }

    // Break caught: a full hint channel must recover the final file once, without introducing periodic polling.
    #[tokio::test]
    async fn overflow_runs_one_full_scan_and_does_not_poll_after_recovery() {
        let directory = tempfile::tempdir().unwrap();
        let status_dir = directory.path().join("agent-status");
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let emitter = Arc::new(CapturingEmitter::default());
        let repository = AgentRepository::new(storage.clone());
        let watcher = Arc::new(watcher_for_storage(
            storage.clone(),
            status_dir.clone(),
            emitter,
        ));
        let occurred_at = crate::services::now_millis();
        let mut source = OverflowSource {
            status_dir: status_dir.clone(),
            occurred_at,
        };
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let run = tokio::spawn({
            let watcher = watcher.clone();
            async move { watcher.run_with_source(&mut source, shutdown_rx).await }
        });

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if repository
                    .list_tasks()
                    .unwrap()
                    .iter()
                    .any(|task| task.source_event_id == "overflow-final")
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("overflow full scan must recover the final locked file");
        let path = status_dir.join("codex-windows.json");
        std::fs::write(
            &path,
            fixture_with_sequence(
                "No poll",
                "codex",
                "windows",
                "completed",
                "without-hint",
                2,
                occurred_at + 1,
            ),
        )
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(180)).await;
        assert_eq!(
            repository.list_tasks().unwrap()[0].source_event_id,
            "overflow-final"
        );
        let health = ServiceHealthRepository::new(storage).list().unwrap();
        assert_eq!(
            health
                .iter()
                .find(|row| row.service_id == AGENT_WATCHER_SERVICE_ID)
                .unwrap()
                .parameters["reasonCode"],
            SafeParameterValue::String("hintOverflow".into())
        );
        shutdown_tx.send(true).unwrap();
        run.await.unwrap();
    }

    // Break caught: unapproved callback paths must be rejected before they can consume queue capacity.
    #[tokio::test]
    async fn ten_thousand_temp_hints_do_not_trigger_a_scan_or_degrade_health() {
        let directory = tempfile::tempdir().unwrap();
        let status_dir = directory.path().join("agent-status");
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let emitter = Arc::new(CapturingEmitter::default());
        let watcher = Arc::new(watcher_for_storage(
            storage.clone(),
            status_dir.clone(),
            emitter.clone(),
        ));
        let mut source = TempNoiseSource { status_dir };
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let run = tokio::spawn({
            let watcher = watcher.clone();
            async move { watcher.run_with_source(&mut source, shutdown_rx).await }
        });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(emitter.0.lock().unwrap().len(), 0);
        let health = ServiceHealthRepository::new(storage).list().unwrap();
        let watcher_health = health
            .iter()
            .find(|row| row.service_id == AGENT_WATCHER_SERVICE_ID)
            .unwrap();
        assert_eq!(watcher_health.state, ServiceHealthState::Healthy);
        assert!(watcher_health.parameters.get("reasonCode").is_none());

        shutdown_tx.send(true).unwrap();
        run.await.unwrap();
    }

    // Break caught: the four-card snapshot must never fabricate an unsupported WorkBuddy WSL integration.
    #[test]
    fn workbuddy_snapshot_contains_only_its_windows_integration() {
        let (_, watcher, _, _) = watcher();
        let snapshot = watcher.snapshot(4_000).unwrap();
        let workbuddy = &snapshot.agents[2];
        assert_eq!(workbuddy.agent_id, AgentId::Workbuddy);
        assert_eq!(workbuddy.integrations.len(), 1);
        assert_eq!(
            workbuddy.integrations[0].environment,
            AgentEnvironment::Windows
        );
    }

    // Break caught: a closed reminder wake receiver must not undo the agent commit or suppress its snapshot event.
    #[test]
    fn reminder_enqueue_failure_records_safe_diagnostic_without_rolling_back_agent() {
        let directory = tempfile::tempdir().unwrap();
        let status_dir = directory.path().join("agent-status");
        std::fs::create_dir(&status_dir).unwrap();
        let storage = Arc::new(Storage::open(directory.path()).unwrap());
        let emitter = Arc::new(CapturingEmitter::default());
        let reminders = ReminderRepository::new(storage.clone());
        reminders
            .save_rule(
                SaveReminderRuleInput {
                    id: None,
                    agent_ids: vec![AgentId::Codex],
                    trigger_statuses: vec![AgentTriggerStatus::Completed],
                    enabled: true,
                    delay_seconds: 0,
                    sound: ReminderSound::None,
                    toast_enabled: true,
                    window_enabled: false,
                    expected_revision: None,
                },
                1,
            )
            .unwrap();
        let (reminder_service, reminder_worker) =
            ReminderService::new(reminders, Arc::new(SystemReminderClock), emitter.clone());
        drop(reminder_worker);
        let repository = AgentRepository::new(storage.clone());
        let watcher = AgentStatusWatcher::new(
            repository.clone(),
            reminder_service,
            ServiceHealthRepository::new(storage.clone()),
            DiagnosticsRepository::new(storage.clone()),
            emitter.clone(),
            status_dir.clone(),
        );
        let path = status_dir.join("codex-windows.json");
        std::fs::write(
            &path,
            fixture_with_sequence(
                "Complete",
                "codex",
                "windows",
                "completed",
                "completed-agent",
                1,
                5_000,
            ),
        )
        .unwrap();

        assert!(matches!(
            watcher.process_path(&path, 5_000).unwrap(),
            ProjectionOutcome::Advanced { .. }
        ));
        assert_eq!(
            repository.list_tasks().unwrap()[0].source_event_id,
            "completed-agent"
        );
        assert_eq!(emitter.0.lock().unwrap().len(), 1);
        let diagnostics = DiagnosticsRepository::new(storage).list(10).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "reminder.enqueueFailed");
        assert_eq!(
            diagnostics[0].parameters["agentName"],
            SafeParameterValue::String("Codex".into())
        );
        assert!(diagnostics[0].parameters.get("fileNameHash").is_some());
        assert!(diagnostics[0].parameters.get("receivedAt").is_some());
    }
}
use crate::contracts::{
    AgentEnvironment, AgentId, AgentIntegrationRecord, AgentStatus, AgentsSnapshot, AppErrorCode,
    CommandError, DiagnosticEvent, DiagnosticLevel, IntegrationState, SafeParameterValue,
    ServiceHealthSnapshot, ServiceHealthState,
};
use crate::domain::agents::{aggregate_agent_at, ValidatedAgentEvent, AGENT_REPLY_MESSAGE_PREFIX};
use crate::events::{agent_state_changed_payload, AGENT_STATE_CHANGED};
use crate::repositories::{
    agents::{AgentRepository, ProjectionOutcome},
    diagnostics::DiagnosticsRepository,
    service_health::ServiceHealthRepository,
};
use crate::services::{
    native_agent_activity::{
        NativeAgentActivity, NativeAgentActivityReader, NativeAgentActivitySource,
        NATIVE_ACTIVITY_TASK_ID,
    },
    now_millis,
    reminder_scheduler::ReminderService,
    EventEmitterPort,
};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

trait LegacyAgentPresenceSource: Send + Sync {
    fn running_agents(&self) -> Result<Vec<AgentId>, CommandError>;
}

struct NativeLegacyAgentPresenceSource;

impl LegacyAgentPresenceSource for NativeLegacyAgentPresenceSource {
    fn running_agents(&self) -> Result<Vec<AgentId>, CommandError> {
        #[cfg(test)]
        {
            Ok(Vec::new())
        }
        #[cfg(not(test))]
        {
            running_legacy_agents()
        }
    }
}

fn agent_for_process_name(base_name: &str) -> Option<AgentId> {
    Some(match base_name.to_ascii_lowercase().as_str() {
        "codex.exe" => AgentId::Codex,
        "claude.exe" => AgentId::Claude,
        "hermes.exe" => AgentId::Hermes,
        "workbuddy.exe" => AgentId::Workbuddy,
        _ => return None,
    })
}

fn running_legacy_agents() -> Result<Vec<AgentId>, CommandError> {
    let mut agents = Vec::new();
    for base_name in running_process_base_names()? {
        if let Some(agent_id) = agent_for_process_name(&base_name) {
            if !agents.contains(&agent_id) {
                agents.push(agent_id);
            }
        }
    }
    Ok(agents)
}

#[cfg(all(windows, not(test)))]
pub(crate) fn running_process_base_names() -> Result<Vec<String>, CommandError> {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map_err(|_| process_snapshot_error())?;
    let _guard = ProcessSnapshotGuard(snapshot);
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    if unsafe { Process32FirstW(snapshot, &mut entry) }.is_err() {
        return Err(process_snapshot_error());
    }

    let mut base_names = Vec::new();
    loop {
        let end = entry
            .szExeFile
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(entry.szExeFile.len());
        let base_name = String::from_utf16_lossy(&entry.szExeFile[..end]);
        if !base_names
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&base_name))
        {
            base_names.push(base_name);
        }
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if unsafe { Process32NextW(snapshot, &mut entry) }.is_err() {
            break;
        }
    }
    Ok(base_names)
}

#[cfg(any(not(windows), test))]
pub(crate) fn running_process_base_names() -> Result<Vec<String>, CommandError> {
    Ok(Vec::new())
}

fn process_snapshot_error() -> CommandError {
    CommandError::with_detail(
        AppErrorCode::SourceUnavailable,
        "errors.sourceUnavailable",
        "reasonCode",
        SafeParameterValue::String("processSnapshotFailed".into()),
        true,
    )
}

#[cfg(windows)]
struct ProcessSnapshotGuard(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for ProcessSnapshotGuard {
    fn drop(&mut self) {
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.0) };
    }
}

pub const AGENT_WATCHER_SERVICE_ID: &str = "agentWatcher";
const COALESCE_WINDOW: std::time::Duration = std::time::Duration::from_millis(100);
const PROCESS_PRESENCE_INTERVAL_SECS: u64 = 3;
const PROCESS_PRESENCE_TASK_ID: &str = "process-presence";
const LOCKED_STATUS_FILES: [&str; 7] = [
    "codex-windows.json",
    "codex-wsl.json",
    "hermes-windows.json",
    "hermes-wsl.json",
    "workbuddy-windows.json",
    "claude-windows.json",
    "claude-wsl.json",
];

pub trait NotifySource: Send {
    fn register(
        &mut self,
        parent: &Path,
        sink: Arc<dyn Fn(PathBuf) + Send + Sync>,
    ) -> Result<(), String>;
}

#[derive(Default)]
struct NativeNotifySource {
    watcher: Option<RecommendedWatcher>,
}

impl NotifySource for NativeNotifySource {
    fn register(
        &mut self,
        parent: &Path,
        sink: Arc<dyn Fn(PathBuf) + Send + Sync>,
    ) -> Result<(), String> {
        let mut watcher = RecommendedWatcher::new(
            move |result: notify::Result<notify::Event>| {
                if let Ok(event) = result {
                    for path in event.paths {
                        sink(path);
                    }
                }
            },
            Config::default(),
        )
        .map_err(|_| "recommendedWatcher".to_string())?;
        watcher
            .watch(parent, RecursiveMode::NonRecursive)
            .map_err(|_| "watchParent".to_string())?;
        self.watcher = Some(watcher);
        Ok(())
    }
}

pub struct AgentStatusWatcher {
    repository: AgentRepository,
    reminders: Arc<ReminderService>,
    health: ServiceHealthRepository,
    diagnostics: DiagnosticsRepository,
    emitter: Arc<dyn EventEmitterPort>,
    status_dir: PathBuf,
    observed_sources: Mutex<BTreeMap<String, ValidatedAgentEvent>>,
    source_states: Mutex<BTreeMap<String, LockedSourceState>>,
    presence_source: Arc<dyn LegacyAgentPresenceSource>,
    activity_source: Option<Arc<dyn NativeAgentActivitySource>>,
    presence_degraded: AtomicBool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LockedSourceState {
    Missing,
    Valid,
    Invalid,
}

impl AgentStatusWatcher {
    pub fn new(
        repository: AgentRepository,
        reminders: Arc<ReminderService>,
        health: ServiceHealthRepository,
        diagnostics: DiagnosticsRepository,
        emitter: Arc<dyn EventEmitterPort>,
        status_dir: PathBuf,
    ) -> Self {
        Self::new_with_presence_source_inner(
            repository,
            reminders,
            health,
            diagnostics,
            emitter,
            status_dir,
            Arc::new(NativeLegacyAgentPresenceSource),
            NativeAgentActivityReader::production()
                .map(|source| Arc::new(source) as Arc<dyn NativeAgentActivitySource>),
        )
    }

    fn new_with_presence_source_inner(
        repository: AgentRepository,
        reminders: Arc<ReminderService>,
        health: ServiceHealthRepository,
        diagnostics: DiagnosticsRepository,
        emitter: Arc<dyn EventEmitterPort>,
        status_dir: PathBuf,
        presence_source: Arc<dyn LegacyAgentPresenceSource>,
        activity_source: Option<Arc<dyn NativeAgentActivitySource>>,
    ) -> Self {
        Self {
            repository,
            reminders,
            health,
            diagnostics,
            emitter,
            status_dir,
            observed_sources: Mutex::new(BTreeMap::new()),
            source_states: Mutex::new(BTreeMap::new()),
            presence_source,
            activity_source,
            presence_degraded: AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    fn new_with_presence_source(
        repository: AgentRepository,
        reminders: Arc<ReminderService>,
        health: ServiceHealthRepository,
        diagnostics: DiagnosticsRepository,
        emitter: Arc<dyn EventEmitterPort>,
        status_dir: PathBuf,
        presence_source: Arc<dyn LegacyAgentPresenceSource>,
    ) -> Self {
        Self::new_with_presence_source_inner(
            repository,
            reminders,
            health,
            diagnostics,
            emitter,
            status_dir,
            presence_source,
            None,
        )
    }

    #[cfg(test)]
    fn new_with_sources(
        repository: AgentRepository,
        reminders: Arc<ReminderService>,
        health: ServiceHealthRepository,
        diagnostics: DiagnosticsRepository,
        emitter: Arc<dyn EventEmitterPort>,
        status_dir: PathBuf,
        presence_source: Arc<dyn LegacyAgentPresenceSource>,
        activity_source: Arc<dyn NativeAgentActivitySource>,
    ) -> Self {
        Self::new_with_presence_source_inner(
            repository,
            reminders,
            health,
            diagnostics,
            emitter,
            status_dir,
            presence_source,
            Some(activity_source),
        )
    }

    fn reconcile_process_presence(&self, received_at: i64) -> Result<usize, CommandError> {
        match self.reconcile_process_presence_inner(received_at) {
            Ok(changed) => {
                if self.presence_degraded.swap(false, Ordering::AcqRel) {
                    self.record_healthy(received_at);
                }
                Ok(changed)
            }
            Err(error) => {
                self.presence_degraded.store(true, Ordering::Release);
                self.record_degraded("processSnapshot", received_at);
                Err(error)
            }
        }
    }

    fn reconcile_process_presence_inner(&self, received_at: i64) -> Result<usize, CommandError> {
        let running = self.presence_source.running_agents()?;
        let current = self.repository.list_tasks()?;
        let mut changed = 0;

        for agent_id in [
            AgentId::Codex,
            AgentId::Hermes,
            AgentId::Workbuddy,
            AgentId::Claude,
        ] {
            let existing = current.iter().find(|observation| {
                observation.agent_id == agent_id
                    && observation.environment == AgentEnvironment::Windows
                    && observation.task_id == PROCESS_PRESENCE_TASK_ID
            });
            let desired = if running.contains(&agent_id) {
                Some(AgentStatus::Idle)
            } else if existing.is_some() {
                Some(AgentStatus::Offline)
            } else {
                None
            };
            let Some(desired) = desired else {
                continue;
            };
            if existing.is_some_and(|observation| observation.status == desired) {
                continue;
            }
            let event = ValidatedAgentEvent {
                event_id: format!(
                    "process-presence:{}:{}:{received_at}",
                    agent_name(&agent_id),
                    status_name(&desired)
                ),
                agent_id,
                environment: AgentEnvironment::Windows,
                task_id: PROCESS_PRESENCE_TASK_ID.into(),
                status: desired,
                sequence: None,
                task_title: None,
                project: None,
                message: None,
                path: None,
                occurred_at: received_at,
            };
            if let ProjectionOutcome::Advanced { event } = self
                .repository
                .insert_event_and_project(&event, received_at)?
            {
                self.after_advanced(&event, received_at, true)?;
                changed += 1;
            }
        }
        changed += self.reconcile_native_activity(&running, &current, received_at)?;
        Ok(changed)
    }

    fn reconcile_native_activity(
        &self,
        running: &[AgentId],
        current: &[crate::contracts::AgentObservation],
        received_at: i64,
    ) -> Result<usize, CommandError> {
        let Some(source) = &self.activity_source else {
            return Ok(0);
        };
        let mut changed = 0;
        for agent_id in [
            AgentId::Codex,
            AgentId::Workbuddy,
            AgentId::Hermes,
            AgentId::Claude,
        ] {
            let existing = current.iter().find(|observation| {
                observation.agent_id == agent_id
                    && observation.environment == AgentEnvironment::Windows
                    && observation.task_id == NATIVE_ACTIVITY_TASK_ID
            });
            let activity = if running.contains(&agent_id) {
                source
                    .latest_activity(agent_id.clone(), received_at)
                    .ok()
                    .flatten()
            } else {
                None
            };
            let event = match activity {
                Some(activity) => native_activity_event(activity),
                None if existing.is_some() => ValidatedAgentEvent {
                    event_id: format!(
                        "native:{}:{}:{received_at}",
                        agent_name(&agent_id),
                        if running.contains(&agent_id) {
                            "idle"
                        } else {
                            "offline"
                        }
                    ),
                    agent_id: agent_id.clone(),
                    environment: AgentEnvironment::Windows,
                    task_id: NATIVE_ACTIVITY_TASK_ID.into(),
                    status: if running.contains(&agent_id) {
                        AgentStatus::Idle
                    } else {
                        AgentStatus::Offline
                    },
                    sequence: None,
                    task_title: None,
                    project: None,
                    message: None,
                    path: None,
                    occurred_at: received_at,
                },
                None => continue,
            };
            if existing.is_some_and(|observation| {
                observation.source_event_id == event.event_id && observation.status == event.status
            }) {
                continue;
            }
            if let ProjectionOutcome::Advanced { event } = self
                .repository
                .insert_event_and_project(&event, received_at)?
            {
                self.after_advanced(&event, received_at, true)?;
                changed += 1;
            }
        }
        Ok(changed)
    }

    pub fn initial_scan(&self, received_at: i64) -> Result<usize, CommandError> {
        let mut processed = 0;
        let mut first_error = None;
        let mut changes = Vec::new();
        for file_name in LOCKED_STATUS_FILES {
            let path = self.status_dir.join(file_name);
            let exists = path.exists();
            match self.process_path_with_snapshot(&path, received_at, false) {
                Ok(ProjectionOutcome::Advanced { event }) => changes.push(event),
                Ok(ProjectionOutcome::Duplicate | ProjectionOutcome::IgnoredOutOfOrder) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
            if exists {
                processed += 1;
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        for event in &changes {
            self.emit_change(event)?;
        }
        Ok(processed)
    }

    pub async fn run(self: Arc<Self>, shutdown: tokio::sync::watch::Receiver<bool>) {
        let mut source = NativeNotifySource::default();
        self.run_with_source(&mut source, shutdown).await;
        drop(source);
    }

    pub async fn run_with_source(
        self: Arc<Self>,
        source: &mut dyn NotifySource,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        if std::fs::create_dir_all(&self.status_dir).is_err() {
            self.record_registration_failure("directoryCreate", now_millis());
            return;
        }
        let (hint_tx, mut hint_rx) = tokio::sync::mpsc::channel::<PathBuf>(256);
        let overflow = Arc::new(AtomicBool::new(false));
        let sink = Arc::new({
            let overflow = overflow.clone();
            let status_dir = self.status_dir.clone();
            move |path: PathBuf| {
                let Some(name) = canonical_file_name(&path) else {
                    return;
                };
                if !LOCKED_STATUS_FILES.contains(&name.as_str()) {
                    return;
                }
                let path = status_dir.join(name);
                match hint_tx.try_send(path) {
                    Ok(()) => {}
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        overflow.store(true, Ordering::Release);
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {}
                }
            }
        });
        let _sink_lifetime = sink.clone();
        if let Err(reason) = source.register(&self.status_dir, sink) {
            self.record_registration_failure(&reason, now_millis());
            return;
        }
        if self.initial_scan(now_millis()).is_err() {
            self.record_degraded("initialScan", now_millis());
        } else {
            self.record_healthy(now_millis());
        }
        let _ = self.reconcile_process_presence(now_millis());

        let mut pending = BTreeMap::<PathBuf, tokio::time::Instant>::new();
        let mut presence_tick = tokio::time::interval_at(
            tokio::time::Instant::now()
                + std::time::Duration::from_secs(PROCESS_PRESENCE_INTERVAL_SECS),
            std::time::Duration::from_secs(PROCESS_PRESENCE_INTERVAL_SECS),
        );
        presence_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if overflow.swap(false, Ordering::AcqRel) {
                self.record_degraded("hintOverflow", now_millis());
                pending.clear();
                while hint_rx.try_recv().is_ok() {}
                let _ = self.initial_scan(now_millis());
            }
            if *shutdown.borrow() {
                return;
            }
            if pending.is_empty() {
                let hint = tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() { return; }
                        continue;
                    }
                    hint = hint_rx.recv() => match hint { Some(path) => path, None => return },
                    _ = presence_tick.tick() => {
                        let _ = self.reconcile_process_presence(now_millis());
                        continue;
                    }
                };
                if let Some(path) = self.locked_path(&hint) {
                    pending.insert(path, tokio::time::Instant::now() + COALESCE_WINDOW);
                }
                continue;
            }
            let earliest = *pending.values().min().expect("non-empty pending deadlines");
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { return; }
                }
                hint = hint_rx.recv() => match hint {
                    Some(path) => if let Some(path) = self.locked_path(&path) {
                        pending.entry(path).or_insert_with(|| tokio::time::Instant::now() + COALESCE_WINDOW);
                    },
                    None => return,
                },
                _ = presence_tick.tick() => {
                    let _ = self.reconcile_process_presence(now_millis());
                }
                _ = tokio::time::sleep_until(earliest) => {
                    let now = tokio::time::Instant::now();
                    let due = pending.iter().filter_map(|(path, deadline)| (*deadline <= now).then_some(path.clone())).collect::<Vec<_>>();
                    for path in due {
                        pending.remove(&path);
                        if self.process_path(&path, now_millis()).is_err() {
                            self.record_degraded("processPath", now_millis());
                        }
                    }
                }
            }
        }
    }

    pub fn process_path(
        &self,
        path: &Path,
        received_at: i64,
    ) -> Result<ProjectionOutcome, CommandError> {
        self.process_path_with_snapshot(path, received_at, true)
    }

    fn process_path_with_snapshot(
        &self,
        path: &Path,
        received_at: i64,
        emit_change: bool,
    ) -> Result<ProjectionOutcome, CommandError> {
        let Some(path) = self.locked_path(path) else {
            return Ok(ProjectionOutcome::Duplicate);
        };
        let key = canonical_file_name(&path).expect("locked path has a file name");
        if !path.exists() {
            self.source_states
                .lock()
                .expect("watcher source state lock poisoned")
                .insert(key.clone(), LockedSourceState::Missing);
            return self.project_offline(&key, received_at, emit_change);
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => {
                self.source_states
                    .lock()
                    .expect("watcher source state lock poisoned")
                    .insert(key.clone(), LockedSourceState::Invalid);
                self.record_file_fault(&key, None, None, "readFailed", received_at);
                return Ok(ProjectionOutcome::Duplicate);
            }
        };
        let event = match crate::domain::agents::parse_status_file_at(&key, &bytes, received_at) {
            Ok(event) => event,
            Err(fault) => {
                self.source_states
                    .lock()
                    .expect("watcher source state lock poisoned")
                    .insert(key.clone(), LockedSourceState::Invalid);
                self.record_file_fault(
                    &key,
                    fault.agent_id,
                    fault.environment,
                    fault.code,
                    received_at,
                );
                return Ok(ProjectionOutcome::Duplicate);
            }
        };
        let outcome = self
            .repository
            .insert_event_and_project(&event, received_at)?;
        self.source_states
            .lock()
            .expect("watcher source state lock poisoned")
            .insert(key.clone(), LockedSourceState::Valid);
        let authoritative = match &outcome {
            ProjectionOutcome::Advanced { event } => Some(event.clone()),
            ProjectionOutcome::Duplicate
                if self.repository.list_tasks()?.iter().any(|task| {
                    task.agent_id == event.agent_id
                        && task.environment == event.environment
                        && task.task_id == event.task_id
                        && task.source_event_id == event.event_id
                }) =>
            {
                self.repository.get_event_by_id(&event.event_id)?
            }
            ProjectionOutcome::Duplicate | ProjectionOutcome::IgnoredOutOfOrder => None,
        };
        if let Some(authoritative) = authoritative {
            self.observed_sources
                .lock()
                .expect("watcher source state lock poisoned")
                .insert(key, authoritative);
        }
        if let ProjectionOutcome::Advanced { event } = &outcome {
            self.after_advanced(event, received_at, emit_change)?;
        }
        Ok(outcome)
    }

    pub fn snapshot(&self, generated_at: i64) -> Result<AgentsSnapshot, CommandError> {
        let mut observations = self.repository.list_tasks()?;
        let source_states = self
            .source_states
            .lock()
            .expect("watcher source state lock poisoned")
            .clone();
        observations.retain(|observation| {
            if matches!(
                observation.task_id.as_str(),
                PROCESS_PRESENCE_TASK_ID | NATIVE_ACTIVITY_TASK_ID
            ) {
                return true;
            }
            let key = format!(
                "{}-{}.json",
                agent_name(&observation.agent_id),
                environment_name(&observation.environment)
            );
            source_states.get(&key) != Some(&LockedSourceState::Missing)
        });
        for agent_id in [
            AgentId::Codex,
            AgentId::Hermes,
            AgentId::Workbuddy,
            AgentId::Claude,
        ] {
            if observations.iter().any(|observation| {
                observation.agent_id == agent_id
                    && observation.task_id != PROCESS_PRESENCE_TASK_ID
                    && observation.status != AgentStatus::Offline
            }) {
                observations.retain(|observation| {
                    observation.agent_id != agent_id
                        || observation.task_id != PROCESS_PRESENCE_TASK_ID
                });
            }
        }
        let agents = [
            AgentId::Codex,
            AgentId::Hermes,
            AgentId::Workbuddy,
            AgentId::Claude,
        ]
        .into_iter()
        .map(|agent_id| {
            let integrations = integrations_for(&self.repository, &agent_id)?;
            Ok(aggregate_agent_at(
                agent_id,
                &observations,
                &integrations,
                generated_at,
            ))
        })
        .collect::<Result<Vec<_>, CommandError>>()?;
        Ok(AgentsSnapshot {
            agents,
            generated_at,
        })
    }

    fn locked_path(&self, path: &Path) -> Option<PathBuf> {
        let name = canonical_file_name(path)?;
        LOCKED_STATUS_FILES
            .contains(&name.as_str())
            .then(|| self.status_dir.join(name))
    }

    fn project_offline(
        &self,
        key: &str,
        received_at: i64,
        emit_change: bool,
    ) -> Result<ProjectionOutcome, CommandError> {
        let Some(source) = self
            .observed_sources
            .lock()
            .expect("watcher source state lock poisoned")
            .remove(key)
        else {
            return Ok(ProjectionOutcome::Duplicate);
        };
        let offline = ValidatedAgentEvent {
            event_id: format!(
                "offline:{}:{}:{}",
                agent_name(&source.agent_id),
                environment_name(&source.environment),
                source.event_id
            ),
            agent_id: source.agent_id,
            environment: source.environment,
            task_id: source.task_id,
            status: AgentStatus::Offline,
            sequence: source.sequence.map(|sequence| sequence.saturating_add(1)),
            task_title: None,
            project: None,
            message: None,
            path: None,
            occurred_at: received_at,
        };
        let outcome = self
            .repository
            .insert_event_and_project(&offline, received_at)?;
        if let ProjectionOutcome::Advanced { event } = &outcome {
            self.after_advanced(event, received_at, emit_change)?;
        }
        Ok(outcome)
    }

    fn after_advanced(
        &self,
        event: &ValidatedAgentEvent,
        received_at: i64,
        emit_change: bool,
    ) -> Result<(), CommandError> {
        if matches!(
            event.status,
            AgentStatus::Completed
                | AgentStatus::Failed
                | AgentStatus::Waiting
                | AgentStatus::Timeout
        ) && self
            .reminders
            .enqueue_agent_event(event, received_at)
            .is_err()
        {
            self.record_file_fault(
                "reminder",
                Some(event.agent_id.clone()),
                Some(event.environment.clone()),
                "enqueueFailed",
                received_at,
            );
        }
        if emit_change {
            self.emit_change(event)?;
        }
        Ok(())
    }

    fn emit_change(&self, event: &ValidatedAgentEvent) -> Result<(), CommandError> {
        self.emitter
            .emit(AGENT_STATE_CHANGED, agent_state_changed_payload(event))
    }

    fn record_healthy(&self, checked_at: i64) {
        let _ = self.health.upsert(&ServiceHealthSnapshot {
            service_id: AGENT_WATCHER_SERVICE_ID.into(),
            state: ServiceHealthState::Healthy,
            message_key: "services.healthy".into(),
            parameters: BTreeMap::from([(
                "serviceId".into(),
                SafeParameterValue::String(AGENT_WATCHER_SERVICE_ID.into()),
            )]),
            checked_at,
        });
    }

    fn record_degraded(&self, reason: &str, checked_at: i64) {
        let _ = self.health.upsert(&ServiceHealthSnapshot {
            service_id: AGENT_WATCHER_SERVICE_ID.into(),
            state: ServiceHealthState::Degraded,
            message_key: "services.degraded".into(),
            parameters: BTreeMap::from([
                (
                    "serviceId".into(),
                    SafeParameterValue::String(AGENT_WATCHER_SERVICE_ID.into()),
                ),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String(reason.into()),
                ),
            ]),
            checked_at,
        });
    }

    fn record_registration_failure(&self, reason: &str, received_at: i64) {
        self.record_degraded(reason, received_at);
        let _ = self.diagnostics.record(&DiagnosticEvent {
            id: format!("watcher-registration-{received_at}"),
            service_id: AGENT_WATCHER_SERVICE_ID.into(),
            level: DiagnosticLevel::Failure,
            code: "watcher.registrationFailed".into(),
            parameters: BTreeMap::from([
                (
                    "serviceId".into(),
                    SafeParameterValue::String(AGENT_WATCHER_SERVICE_ID.into()),
                ),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String(reason.into()),
                ),
            ]),
            created_at: received_at,
        });
    }

    fn record_file_fault(
        &self,
        file_name: &str,
        agent: Option<AgentId>,
        environment: Option<AgentEnvironment>,
        reason: &str,
        received_at: i64,
    ) {
        let (agent, environment) = match match (agent, environment) {
            (Some(agent), Some(environment)) => Some((agent, environment)),
            _ => identity_for_file(file_name),
        } {
            Some(value) => value,
            None => return,
        };
        let code = if file_name == "reminder" {
            "reminder.enqueueFailed"
        } else {
            "watcher.fileInvalid"
        };
        let _ = self.diagnostics.record(&DiagnosticEvent {
            id: format!(
                "{}-{}-{}",
                if code == "watcher.fileInvalid" {
                    "watcher-file"
                } else {
                    "reminder-enqueue"
                },
                received_at,
                file_name_hash(file_name)
            ),
            service_id: AGENT_WATCHER_SERVICE_ID.into(),
            level: DiagnosticLevel::Warning,
            code: code.into(),
            parameters: BTreeMap::from([
                (
                    "agentName".into(),
                    SafeParameterValue::String(agent.display_name().into()),
                ),
                (
                    "environment".into(),
                    SafeParameterValue::String(environment_name(&environment).into()),
                ),
                (
                    "fileNameHash".into(),
                    SafeParameterValue::String(file_name_hash(file_name)),
                ),
                (
                    "reasonCode".into(),
                    SafeParameterValue::String(reason.into()),
                ),
                (
                    "receivedAt".into(),
                    SafeParameterValue::Number(received_at.into()),
                ),
            ]),
            created_at: received_at,
        });
    }

    #[cfg(test)]
    fn diagnostics_for_test(&self) -> &DiagnosticsRepository {
        &self.diagnostics
    }
}

fn integrations_for(
    repository: &AgentRepository,
    agent: &AgentId,
) -> Result<Vec<AgentIntegrationRecord>, CommandError> {
    let environments: &[AgentEnvironment] = match agent {
        AgentId::Workbuddy => &[AgentEnvironment::Windows],
        _ => &[AgentEnvironment::Windows, AgentEnvironment::Wsl],
    };
    environments
        .iter()
        .cloned()
        .map(|environment| {
            match repository.get_integration(agent.clone(), environment.clone())? {
                Some(record) => Ok(AgentRepository::boundary_integration(&record)),
                None => Ok(AgentIntegrationRecord {
                    environment: environment.clone(),
                    supported: true,
                    required: false,
                    state: IntegrationState::NotInstalled,
                    reason_code: None,
                }),
            }
        })
        .collect()
}

fn canonical_file_name(path: &Path) -> Option<String> {
    path.file_name()?.to_str().map(str::to_ascii_lowercase)
}
fn agent_name(agent: &AgentId) -> &'static str {
    match agent {
        AgentId::Codex => "codex",
        AgentId::Hermes => "hermes",
        AgentId::Workbuddy => "workbuddy",
        AgentId::Claude => "claude",
    }
}
fn status_name(status: &AgentStatus) -> &'static str {
    match status {
        AgentStatus::Running => "running",
        AgentStatus::Waiting => "waiting",
        AgentStatus::Completed => "completed",
        AgentStatus::Failed => "failed",
        AgentStatus::Timeout => "timeout",
        AgentStatus::Idle => "idle",
        AgentStatus::Offline => "offline",
    }
}
fn native_activity_event(activity: NativeAgentActivity) -> ValidatedAgentEvent {
    let status = status_name(&activity.status);
    let mut hasher = Sha256::new();
    for part in [
        agent_name(&activity.agent_id),
        activity.session_id.as_str(),
        status,
        activity.title.as_deref().unwrap_or(""),
        activity.latest_reply.as_deref().unwrap_or(""),
    ] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    hasher.update(activity.occurred_at.to_le_bytes());
    let fingerprint = hasher
        .finalize()
        .iter()
        .take(12)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    ValidatedAgentEvent {
        event_id: format!(
            "native:{}:{}:{status}:{fingerprint}",
            agent_name(&activity.agent_id),
            activity.session_id
        ),
        agent_id: activity.agent_id,
        environment: AgentEnvironment::Windows,
        task_id: NATIVE_ACTIVITY_TASK_ID.into(),
        status: activity.status,
        sequence: None,
        task_title: activity.title,
        project: None,
        message: activity
            .latest_reply
            .map(|reply| format!("{AGENT_REPLY_MESSAGE_PREFIX}{reply}")),
        path: None,
        occurred_at: activity.occurred_at,
    }
}
fn environment_name(environment: &AgentEnvironment) -> &'static str {
    match environment {
        AgentEnvironment::Windows => "windows",
        AgentEnvironment::Wsl => "wsl",
    }
}
fn identity_for_file(file_name: &str) -> Option<(AgentId, AgentEnvironment)> {
    Some(match file_name {
        "codex-windows.json" => (AgentId::Codex, AgentEnvironment::Windows),
        "codex-wsl.json" => (AgentId::Codex, AgentEnvironment::Wsl),
        "hermes-windows.json" => (AgentId::Hermes, AgentEnvironment::Windows),
        "hermes-wsl.json" => (AgentId::Hermes, AgentEnvironment::Wsl),
        "workbuddy-windows.json" => (AgentId::Workbuddy, AgentEnvironment::Windows),
        "claude-windows.json" => (AgentId::Claude, AgentEnvironment::Windows),
        "claude-wsl.json" => (AgentId::Claude, AgentEnvironment::Wsl),
        _ => return None,
    })
}
fn file_name_hash(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}
