use crate::contracts::{AgentId, AgentStatus, AppErrorCode, CommandError, SafeMessageParameters};
use notify::{RecursiveMode, Watcher};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Deserialize;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) const NATIVE_ACTIVITY_TASK_ID: &str = "native-session";
pub(crate) const COMPLETED_FRESHNESS_MILLIS: i64 = 5 * 60 * 1000;
const MAX_JSONL_TAIL_BYTES: u64 = 4 * 1024 * 1024;
const MAX_INCREMENTAL_JSONL_BYTES: u64 = 256 * 1024;
const MAX_INCOMPLETE_JSONL_BYTES: usize = 1024 * 1024;
const NATIVE_ACTIVITY_FALLBACK_MILLIS: i64 = 30 * 1000;
const MAX_PROJECT_DIRECTORIES: usize = 2_048;
const MAX_ID_BYTES: usize = 128;
const MAX_TITLE_BYTES: usize = 1_024;
const MAX_REPLY_BYTES: usize = 1_024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NativeAgentActivity {
    pub agent_id: AgentId,
    pub session_id: String,
    pub status: AgentStatus,
    pub title: Option<String>,
    pub latest_reply: Option<String>,
    pub occurred_at: i64,
    pub source_bytes: u64,
}

pub(crate) trait NativeAgentActivitySource: Send + Sync {
    fn latest_activity(
        &self,
        agent_id: AgentId,
        now: i64,
    ) -> Result<Option<NativeAgentActivity>, CommandError>;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct NativeActivityScanMetrics {
    bytes_read: u64,
    parser_calls: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeSessionKind {
    Codex,
    Workbuddy,
    Hermes,
    Claude,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JsonlRefreshOutcome {
    Rebuilt,
    Appended,
    Unchanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NativeFileIdentity {
    volume: u64,
    index: u64,
}

#[derive(Deserialize)]
struct CodexJsonlLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    payload: Option<CodexJsonlPayload>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum CodexJsonlPayload {
    #[serde(rename = "task_started")]
    TaskStarted,
    #[serde(rename = "task_complete")]
    TaskComplete,
    #[serde(rename = "turn_aborted")]
    TurnAborted,
    #[serde(rename = "agent_message")]
    AgentMessage {
        message: Option<String>,
        phase: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct WorkbuddyJsonlLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    role: Option<String>,
    status: Option<String>,
    #[serde(default)]
    content: Vec<WorkbuddyJsonlContent>,
}

#[derive(Deserialize)]
struct WorkbuddyJsonlContent {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
}

#[derive(Deserialize)]
struct ClaudeAuditResultLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    subtype: Option<String>,
    is_error: Option<bool>,
    result: Option<String>,
}

#[derive(Debug)]
struct CachedNativeSession {
    session_id: String,
    path: PathBuf,
    identity: NativeFileIdentity,
    offset: u64,
    last_write_marker: u64,
    incomplete_line: Vec<u8>,
    discarding_oversized_line: bool,
    kind: NativeSessionKind,
    status: AgentStatus,
    saw_lifecycle: bool,
    title: Option<String>,
    latest_reply: Option<String>,
    updated_at: i64,
}

impl CachedNativeSession {
    fn new(
        session_id: String,
        path: PathBuf,
        identity: NativeFileIdentity,
        last_write_marker: u64,
        kind: NativeSessionKind,
    ) -> Self {
        Self {
            session_id,
            path,
            identity,
            offset: 0,
            last_write_marker,
            incomplete_line: Vec::new(),
            discarding_oversized_line: false,
            kind,
            status: AgentStatus::Idle,
            saw_lifecycle: false,
            title: None,
            latest_reply: None,
            updated_at: 0,
        }
    }

    fn activity(&self, agent_id: AgentId, now: i64) -> NativeAgentActivity {
        let (status, occurred_at) = age_status(self.status.clone(), self.updated_at, now);
        NativeAgentActivity {
            agent_id,
            session_id: self.session_id.clone(),
            status,
            title: self.title.clone(),
            latest_reply: self.latest_reply.clone(),
            occurred_at,
            source_bytes: self.offset,
        }
    }
}

#[derive(Debug, Default)]
struct AgentActivityCache {
    session: Option<CachedNativeSession>,
    activity: Option<NativeAgentActivity>,
    metrics: NativeActivityScanMetrics,
    last_refresh_at: Option<i64>,
    needs_follow_up: bool,
}

#[derive(Debug, Default)]
struct NativeActivityReaderCache {
    codex: AgentActivityCache,
    workbuddy: AgentActivityCache,
    hermes: AgentActivityCache,
    claude: AgentActivityCache,
}

#[derive(Debug, Default)]
struct NativeActivityDirty {
    codex: AtomicBool,
    workbuddy: AtomicBool,
    hermes: AtomicBool,
    claude: AtomicBool,
}

impl NativeActivityDirty {
    fn mark(&self, kind: NativeSessionKind) {
        match kind {
            NativeSessionKind::Codex => self.codex.store(true, Ordering::Release),
            NativeSessionKind::Workbuddy => self.workbuddy.store(true, Ordering::Release),
            NativeSessionKind::Hermes => self.hermes.store(true, Ordering::Release),
            NativeSessionKind::Claude => self.claude.store(true, Ordering::Release),
        }
    }

    fn take(&self, kind: NativeSessionKind) -> bool {
        match kind {
            NativeSessionKind::Codex => self.codex.swap(false, Ordering::AcqRel),
            NativeSessionKind::Workbuddy => self.workbuddy.swap(false, Ordering::AcqRel),
            NativeSessionKind::Hermes => self.hermes.swap(false, Ordering::AcqRel),
            NativeSessionKind::Claude => self.claude.swap(false, Ordering::AcqRel),
        }
    }
}

pub(crate) struct NativeAgentActivityReader {
    user_profile: PathBuf,
    cache: Mutex<NativeActivityReaderCache>,
    dirty: Arc<NativeActivityDirty>,
    _watchers: Mutex<Vec<notify::RecommendedWatcher>>,
}

impl NativeAgentActivityReader {
    pub(crate) fn production() -> Option<Self> {
        #[cfg(test)]
        {
            None
        }
        #[cfg(not(test))]
        {
            std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .map(Self::new)
        }
    }

    pub(crate) fn new(user_profile: PathBuf) -> Self {
        let dirty = Arc::new(NativeActivityDirty::default());
        dirty.mark(NativeSessionKind::Codex);
        dirty.mark(NativeSessionKind::Workbuddy);
        dirty.mark(NativeSessionKind::Hermes);
        dirty.mark(NativeSessionKind::Claude);
        let watchers = [
            (user_profile.join(".codex"), NativeSessionKind::Codex),
            (
                user_profile.join(".workbuddy"),
                NativeSessionKind::Workbuddy,
            ),
            (
                user_profile.join("AppData/Local/hermes"),
                NativeSessionKind::Hermes,
            ),
            (user_profile.join(".hermes"), NativeSessionKind::Hermes),
            (
                user_profile.join("AppData/Local/Claude-3p/local-agent-mode-sessions"),
                NativeSessionKind::Claude,
            ),
        ]
        .into_iter()
        .filter_map(|(root, kind)| native_activity_watcher(&root, kind, dirty.clone()))
        .collect();
        Self {
            user_profile,
            cache: Mutex::new(NativeActivityReaderCache::default()),
            dirty,
            _watchers: Mutex::new(watchers),
        }
    }

    #[cfg(test)]
    fn scan_metrics(&self, agent_id: AgentId) -> NativeActivityScanMetrics {
        let cache = self.cache.lock().unwrap();
        match agent_id {
            AgentId::Codex => cache.codex.metrics,
            AgentId::Workbuddy => cache.workbuddy.metrics,
            AgentId::Hermes => cache.hermes.metrics,
            AgentId::Claude => cache.claude.metrics,
        }
    }

    #[cfg(test)]
    fn mark_dirty_for_test(&self, agent_id: AgentId) {
        if let Some(kind) = native_session_kind(&agent_id) {
            self.dirty.mark(kind);
        }
    }

    fn should_refresh(&self, kind: NativeSessionKind, now: i64) -> bool {
        if self.dirty.take(kind) {
            return true;
        }
        let cache = self.cache.lock().unwrap();
        let last_refresh_at = match kind {
            NativeSessionKind::Codex => cache.codex.last_refresh_at,
            NativeSessionKind::Workbuddy => cache.workbuddy.last_refresh_at,
            NativeSessionKind::Hermes => cache.hermes.last_refresh_at,
            NativeSessionKind::Claude => cache.claude.last_refresh_at,
        };
        last_refresh_at.is_none_or(|last| {
            now < last || now.saturating_sub(last) >= NATIVE_ACTIVITY_FALLBACK_MILLIS
        })
    }

    fn cached_activity(&self, kind: NativeSessionKind, now: i64) -> Option<NativeAgentActivity> {
        let cache = self.cache.lock().unwrap();
        match kind {
            NativeSessionKind::Codex => cache
                .codex
                .session
                .as_ref()
                .map(|session| session.activity(AgentId::Codex, now)),
            NativeSessionKind::Workbuddy => cache
                .workbuddy
                .session
                .as_ref()
                .map(|session| session.activity(AgentId::Workbuddy, now)),
            NativeSessionKind::Hermes => cache
                .hermes
                .activity
                .as_ref()
                .cloned()
                .map(|activity| age_direct_activity(activity, now)),
            NativeSessionKind::Claude => cache
                .claude
                .session
                .as_ref()
                .map(|session| session.activity(AgentId::Claude, now)),
        }
    }

    fn finish_refresh(&self, kind: NativeSessionKind, now: i64, found: bool) {
        let mut cache = self.cache.lock().unwrap();
        let entry = match kind {
            NativeSessionKind::Codex => &mut cache.codex,
            NativeSessionKind::Workbuddy => &mut cache.workbuddy,
            NativeSessionKind::Hermes => &mut cache.hermes,
            NativeSessionKind::Claude => &mut cache.claude,
        };
        entry.last_refresh_at = Some(now);
        if !found {
            entry.session = None;
            entry.activity = None;
            entry.needs_follow_up = false;
        }
        let needs_follow_up = entry.needs_follow_up;
        drop(cache);
        if needs_follow_up {
            self.dirty.mark(kind);
        }
    }

    fn read_codex(&self, now: i64) -> Result<Option<NativeAgentActivity>, CommandError> {
        let codex_root = self.user_profile.join(".codex");
        let database_path = codex_root.join("state_5.sqlite");
        if !database_path.is_file() {
            return Ok(None);
        }
        let connection = open_read_only(&database_path)?;
        let thread = connection
            .query_row(
                "SELECT id, rollout_path,
                        COALESCE(NULLIF(updated_at_ms, 0), updated_at * 1000), title
                   FROM threads
                  WHERE archived = 0
                  ORDER BY COALESCE(NULLIF(updated_at_ms, 0), updated_at * 1000) DESC, id DESC
                  LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(CommandError::from)?;
        let Some((session_id, rollout_path, updated_at, title)) = thread else {
            return Ok(None);
        };
        if !safe_identifier(&session_id) || updated_at < 0 {
            return Ok(None);
        }
        let rollout_path = PathBuf::from(rollout_path);
        if !is_owned_regular_file(&codex_root.join("sessions"), &rollout_path) {
            return Ok(None);
        }
        let mut cache = self.cache.lock().unwrap();
        let _ = refresh_jsonl_session(
            &mut cache.codex,
            &session_id,
            &rollout_path,
            NativeSessionKind::Codex,
        )?;
        let session = cache
            .codex
            .session
            .as_mut()
            .expect("successful refresh populates the Codex cache");
        session.title = bounded_text(&title, MAX_TITLE_BYTES);
        session.updated_at = updated_at;
        Ok(Some(session.activity(AgentId::Codex, now)))
    }

    fn read_workbuddy(&self, now: i64) -> Result<Option<NativeAgentActivity>, CommandError> {
        let workbuddy_root = self.user_profile.join(".workbuddy");
        let database_path = workbuddy_root.join("workbuddy.db");
        if !database_path.is_file() {
            return Ok(None);
        }
        let connection = open_read_only(&database_path)?;
        let session = connection
            .query_row(
                "SELECT id, status, COALESCE(last_activity_at, updated_at), COALESCE(custom_title, title, '')
                   FROM sessions
                  WHERE deleted_at IS NULL
                  ORDER BY COALESCE(last_activity_at, updated_at) DESC, id DESC
                  LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(CommandError::from)?;
        let Some((session_id, stored_status, updated_at, title)) = session else {
            return Ok(None);
        };
        if !safe_identifier(&session_id) || updated_at < 0 {
            return Ok(None);
        }
        let cached_path = self
            .cache
            .lock()
            .unwrap()
            .workbuddy
            .session
            .as_ref()
            .filter(|session| session.session_id == session_id)
            .map(|session| session.path.clone());
        let session_path = match cached_path {
            Some(path) if is_owned_regular_file(&workbuddy_root.join("projects"), &path) => path,
            _ => match find_workbuddy_session(&workbuddy_root, &session_id)? {
                Some(path) => path,
                None => return Ok(None),
            },
        };
        let status = match stored_status.to_ascii_lowercase().as_str() {
            "running" | "in_progress" | "inprogress" | "pending" => AgentStatus::Running,
            "completed" | "complete" | "success" => AgentStatus::Completed,
            "failed" | "error" | "cancelled" | "canceled" => AgentStatus::Failed,
            "waiting" => AgentStatus::Waiting,
            "timeout" | "timed_out" => AgentStatus::Timeout,
            _ => AgentStatus::Idle,
        };
        let mut cache = self.cache.lock().unwrap();
        let refresh = refresh_jsonl_session(
            &mut cache.workbuddy,
            &session_id,
            &session_path,
            NativeSessionKind::Workbuddy,
        )?;
        let session = cache
            .workbuddy
            .session
            .as_mut()
            .expect("successful refresh populates the WorkBuddy cache");
        let database_changed_without_jsonl_growth =
            refresh == JsonlRefreshOutcome::Unchanged && updated_at > session.updated_at;
        if refresh == JsonlRefreshOutcome::Rebuilt || database_changed_without_jsonl_growth {
            session.status = status;
        }
        session.title = bounded_text(&title, MAX_TITLE_BYTES);
        session.updated_at = updated_at;
        Ok(Some(session.activity(AgentId::Workbuddy, now)))
    }

    fn read_hermes(&self, now: i64) -> Result<Option<NativeAgentActivity>, CommandError> {
        let roots = [
            self.user_profile.join("AppData/Local/hermes"),
            self.user_profile.join(".hermes"),
        ];
        let Some((root, database_path)) = roots.into_iter().find_map(|root| {
            let database_path = root.join("state.db");
            database_path.is_file().then_some((root, database_path))
        }) else {
            return Ok(None);
        };
        if !is_owned_regular_file(&root, &database_path) {
            return Ok(None);
        }
        let connection = open_read_only(&database_path)?;
        let message = connection
            .query_row(
                "SELECT m.id, m.session_id, m.role, m.timestamp,
                        COALESCE(m.finish_reason, ''),
                        CASE
                          WHEN m.role = 'assistant'
                           AND (m.tool_calls IS NULL
                                OR TRIM(m.tool_calls) IN ('', '[]', 'null'))
                          THEN m.content
                          ELSE NULL
                        END,
                        CASE
                          WHEN m.tool_calls IS NULL
                            OR TRIM(m.tool_calls) IN ('', '[]', 'null') THEN 0
                          ELSE 1
                        END
                   FROM messages m
                   JOIN sessions s ON s.id = m.session_id
                  WHERE COALESCE(s.archived, 0) = 0
                  ORDER BY m.timestamp DESC, m.id DESC
                  LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, f64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, i64>(6)? != 0,
                    ))
                },
            )
            .optional()
            .map_err(CommandError::from)?;
        let Some((message_id, session_id, role, timestamp, finish_reason, content, has_tools)) =
            message
        else {
            return Ok(None);
        };
        if !safe_identifier(&session_id) || !timestamp.is_finite() || timestamp < 0.0 {
            return Ok(None);
        }
        let role = role.to_ascii_lowercase();
        let status = match role.as_str() {
            "user" | "tool" => AgentStatus::Running,
            "assistant" if has_tools => AgentStatus::Running,
            "assistant"
                if matches!(
                    finish_reason.to_ascii_lowercase().as_str(),
                    "stop" | "completed" | "complete" | "success"
                ) =>
            {
                AgentStatus::Completed
            }
            "assistant" => AgentStatus::Running,
            _ => AgentStatus::Idle,
        };
        let occurred_at = (timestamp * 1_000.0).min(i64::MAX as f64) as i64;
        let activity = NativeAgentActivity {
            agent_id: AgentId::Hermes,
            session_id,
            status,
            title: None,
            latest_reply: content.and_then(|content| bounded_text(&content, MAX_REPLY_BYTES)),
            occurred_at,
            source_bytes: database_path
                .metadata()
                .map(|metadata| metadata.len())
                .unwrap_or(0),
        };
        let mut cache = self.cache.lock().unwrap();
        cache.hermes.activity = Some(activity.clone());
        cache.hermes.metrics.parser_calls = cache.hermes.metrics.parser_calls.saturating_add(1);
        cache.hermes.metrics.bytes_read = cache
            .hermes
            .metrics
            .bytes_read
            .saturating_add(std::mem::size_of_val(&message_id) as u64);
        Ok(Some(age_direct_activity(activity, now)))
    }

    fn read_claude(&self, now: i64) -> Result<Option<NativeAgentActivity>, CommandError> {
        let root = self
            .user_profile
            .join("AppData/Local/Claude-3p/local-agent-mode-sessions");
        let Some((session_id, audit_path, updated_at)) = find_latest_claude_audit(&root)? else {
            return Ok(None);
        };
        if !safe_identifier(&session_id) || !is_owned_regular_file(&root, &audit_path) {
            return Ok(None);
        }
        let mut cache = self.cache.lock().unwrap();
        let _ = refresh_jsonl_session(
            &mut cache.claude,
            &session_id,
            &audit_path,
            NativeSessionKind::Claude,
        )?;
        let session = cache
            .claude
            .session
            .as_mut()
            .expect("successful refresh populates the Claude cache");
        session.updated_at = updated_at;
        Ok(Some(session.activity(AgentId::Claude, now)))
    }
}

impl NativeAgentActivitySource for NativeAgentActivityReader {
    fn latest_activity(
        &self,
        agent_id: AgentId,
        now: i64,
    ) -> Result<Option<NativeAgentActivity>, CommandError> {
        let Some(kind) = native_session_kind(&agent_id) else {
            return Ok(None);
        };
        if !self.should_refresh(kind, now) {
            return Ok(self.cached_activity(kind, now));
        }
        let result = match kind {
            NativeSessionKind::Codex => self.read_codex(now),
            NativeSessionKind::Workbuddy => self.read_workbuddy(now),
            NativeSessionKind::Hermes => self.read_hermes(now),
            NativeSessionKind::Claude => self.read_claude(now),
        };
        match result {
            Ok(activity) => {
                self.finish_refresh(kind, now, activity.is_some());
                Ok(activity)
            }
            Err(error) => {
                self.dirty.mark(kind);
                Err(error)
            }
        }
    }
}

fn native_session_kind(agent_id: &AgentId) -> Option<NativeSessionKind> {
    match agent_id {
        AgentId::Codex => Some(NativeSessionKind::Codex),
        AgentId::Workbuddy => Some(NativeSessionKind::Workbuddy),
        AgentId::Hermes => Some(NativeSessionKind::Hermes),
        AgentId::Claude => Some(NativeSessionKind::Claude),
    }
}

fn age_direct_activity(mut activity: NativeAgentActivity, now: i64) -> NativeAgentActivity {
    let (status, occurred_at) = age_status(activity.status, activity.occurred_at, now);
    activity.status = status;
    activity.occurred_at = occurred_at;
    activity
}

fn native_activity_watcher(
    root: &Path,
    kind: NativeSessionKind,
    dirty: Arc<NativeActivityDirty>,
) -> Option<notify::RecommendedWatcher> {
    if !root.is_dir() {
        return None;
    }
    let mut watcher = notify::recommended_watcher(move |_result: notify::Result<notify::Event>| {
        dirty.mark(kind);
    })
    .ok()?;
    watcher.watch(root, RecursiveMode::Recursive).ok()?;
    Some(watcher)
}

fn open_read_only(path: &Path) -> Result<Connection, CommandError> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(CommandError::from)
}

fn is_owned_regular_file(root: &Path, path: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    path.starts_with(root) && path.metadata().is_ok_and(|metadata| metadata.is_file())
}

fn find_workbuddy_session(
    workbuddy_root: &Path,
    session_id: &str,
) -> Result<Option<PathBuf>, CommandError> {
    let projects = workbuddy_root.join("projects");
    if !projects.is_dir() {
        return Ok(None);
    }
    let expected_name = format!("{session_id}.jsonl");
    for entry in std::fs::read_dir(&projects)
        .map_err(|_| io_failure())?
        .take(MAX_PROJECT_DIRECTORIES)
    {
        let entry = entry.map_err(|_| io_failure())?;
        if !entry.file_type().map_err(|_| io_failure())?.is_dir() {
            continue;
        }
        let candidate = entry.path().join(&expected_name);
        if is_owned_regular_file(&projects, &candidate) {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

fn find_latest_claude_audit(root: &Path) -> Result<Option<(String, PathBuf, i64)>, CommandError> {
    if !root.is_dir() {
        return Ok(None);
    }
    let mut pending = vec![(root.to_path_buf(), 0_u8)];
    let mut visited = 0_usize;
    let mut latest: Option<(String, PathBuf, i64)> = None;
    while let Some((directory, depth)) = pending.pop() {
        if visited >= MAX_PROJECT_DIRECTORIES || depth >= 4 {
            continue;
        }
        visited += 1;
        for entry in std::fs::read_dir(&directory).map_err(|_| io_failure())? {
            let entry = entry.map_err(|_| io_failure())?;
            if !entry.file_type().map_err(|_| io_failure())?.is_dir() {
                continue;
            }
            let path = entry.path();
            if depth + 1 == 3 {
                let session_id = entry.file_name().to_string_lossy().into_owned();
                let audit = path.join("audit.jsonl");
                if !session_id.starts_with("local_") || !is_owned_regular_file(root, &audit) {
                    continue;
                }
                let updated_at = file_modified_millis(&audit).unwrap_or(0);
                let replace = latest.as_ref().is_none_or(|(_, current_path, current_at)| {
                    (updated_at, &audit) > (*current_at, current_path)
                });
                if replace {
                    latest = Some((session_id, audit, updated_at));
                }
            } else {
                pending.push((path, depth + 1));
            }
        }
    }
    Ok(latest)
}

fn file_modified_millis(path: &Path) -> Option<i64> {
    path.metadata()
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
}

fn refresh_jsonl_session(
    cache: &mut AgentActivityCache,
    session_id: &str,
    path: &Path,
    kind: NativeSessionKind,
) -> Result<JsonlRefreshOutcome, CommandError> {
    let mut file = File::open(path).map_err(|_| io_failure())?;
    let metadata = file.metadata().map_err(|_| io_failure())?;
    let length = metadata.len();
    let identity = native_file_identity(&file)?;
    let last_write_marker = native_last_write_marker(&metadata);
    let rebuild = cache.session.as_ref().is_none_or(|session| {
        session.session_id != session_id
            || session.path != path
            || session.identity != identity
            || session.kind != kind
            || length < session.offset
            || (length == session.offset && session.last_write_marker != last_write_marker)
    });

    if rebuild {
        cache.needs_follow_up = false;
        cache.session = Some(CachedNativeSession::new(
            session_id.to_owned(),
            path.to_path_buf(),
            identity,
            last_write_marker,
            kind,
        ));
        let start = length.saturating_sub(MAX_JSONL_TAIL_BYTES);
        let bytes = read_file_range(&mut file, start, length)?;
        cache.metrics.bytes_read = cache.metrics.bytes_read.saturating_add(bytes.len() as u64);
        let session = cache.session.as_mut().unwrap();
        session.offset = start.saturating_add(bytes.len() as u64);
        session.last_write_marker = last_write_marker;
        consume_jsonl_bytes(session, bytes, start > 0, &mut cache.metrics);
        return Ok(JsonlRefreshOutcome::Rebuilt);
    }

    let session = cache.session.as_mut().unwrap();
    let mut outcome = JsonlRefreshOutcome::Unchanged;
    if length > session.offset {
        outcome = JsonlRefreshOutcome::Appended;
        let start = session.offset;
        let end = start
            .saturating_add(MAX_INCREMENTAL_JSONL_BYTES)
            .min(length);
        let bytes = read_file_range(&mut file, start, end)?;
        cache.metrics.bytes_read = cache.metrics.bytes_read.saturating_add(bytes.len() as u64);
        session.offset = start.saturating_add(bytes.len() as u64);
        consume_jsonl_bytes(session, bytes, false, &mut cache.metrics);
    }
    cache.needs_follow_up = session.offset < length;
    session.last_write_marker = last_write_marker;
    Ok(outcome)
}

fn read_file_range(file: &mut File, start: u64, end: u64) -> Result<Vec<u8>, CommandError> {
    file.seek(SeekFrom::Start(start))
        .map_err(|_| io_failure())?;
    let mut bytes = Vec::with_capacity((end - start) as usize);
    file.take(end - start)
        .read_to_end(&mut bytes)
        .map_err(|_| io_failure())?;
    Ok(bytes)
}

fn consume_jsonl_bytes(
    session: &mut CachedNativeSession,
    mut bytes: Vec<u8>,
    discard_leading_partial: bool,
    metrics: &mut NativeActivityScanMetrics,
) {
    if discard_leading_partial {
        if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=newline);
        } else {
            bytes.clear();
        }
    }
    if session.discarding_oversized_line {
        let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') else {
            return;
        };
        bytes.drain(..=newline);
        session.discarding_oversized_line = false;
    }
    session.incomplete_line.extend(bytes);
    let Some(complete_end) = session
        .incomplete_line
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| index + 1)
    else {
        if session.incomplete_line.len() > MAX_INCOMPLETE_JSONL_BYTES {
            session.incomplete_line.clear();
            session.discarding_oversized_line = true;
        }
        return;
    };
    let complete = session
        .incomplete_line
        .drain(..complete_end)
        .collect::<Vec<_>>();
    for line in complete
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        if line.len() <= MAX_INCOMPLETE_JSONL_BYTES {
            if session.kind == NativeSessionKind::Workbuddy
                && json_string_field_equals(line, b"type", b"message")
                && json_string_field_equals(line, b"role", b"user")
            {
                session.status = AgentStatus::Running;
            }
            if session.kind == NativeSessionKind::Claude
                && top_level_json_string_field_equals(line, b"type", b"system")
                && top_level_json_string_field_equals(line, b"subtype", b"status")
                && top_level_json_string_field_equals(line, b"status", b"requesting")
            {
                session.status = AgentStatus::Running;
                session.saw_lifecycle = true;
            }
            if native_line_candidate(session.kind, line) {
                metrics.parser_calls = metrics.parser_calls.saturating_add(1);
                parse_native_jsonl_line(session, line);
            }
        }
    }
    if session.incomplete_line.len() > MAX_INCOMPLETE_JSONL_BYTES {
        session.incomplete_line.clear();
        session.discarding_oversized_line = true;
    }
}

fn native_line_candidate(kind: NativeSessionKind, line: &[u8]) -> bool {
    match kind {
        NativeSessionKind::Codex => {
            json_string_field_equals(line, b"type", b"event_msg")
                && [
                    b"task_started".as_slice(),
                    b"task_complete".as_slice(),
                    b"turn_aborted".as_slice(),
                    b"agent_message".as_slice(),
                ]
                .into_iter()
                .any(|event| json_string_field_equals(line, b"type", event))
        }
        NativeSessionKind::Workbuddy => {
            json_string_field_equals(line, b"type", b"message")
                && json_string_field_equals(line, b"role", b"assistant")
        }
        NativeSessionKind::Hermes => false,
        NativeSessionKind::Claude => top_level_json_string_field_equals(line, b"type", b"result"),
    }
}

fn json_string_field_equals(line: &[u8], key: &[u8], expected: &[u8]) -> bool {
    let mut start = 0;
    while let Some(relative_quote) = line[start..].iter().position(|byte| *byte == b'"') {
        let key_start = start + relative_quote + 1;
        let key_end = key_start.saturating_add(key.len());
        if line.get(key_start..key_end) == Some(key) && line.get(key_end) == Some(&b'"') {
            let mut cursor = key_end + 1;
            while line.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
            if line.get(cursor) != Some(&b':') {
                start = key_start;
                continue;
            }
            cursor += 1;
            while line.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
            if line.get(cursor) == Some(&b'"') {
                let value_start = cursor + 1;
                let value_end = value_start.saturating_add(expected.len());
                if line.get(value_start..value_end) == Some(expected)
                    && line.get(value_end) == Some(&b'"')
                {
                    return true;
                }
            }
        }
        start = key_start;
    }
    false
}

fn top_level_json_string_field_equals(line: &[u8], key: &[u8], expected: &[u8]) -> bool {
    let mut index = 0;
    let mut depth = 0_u32;
    while index < line.len() {
        match line[index] {
            b'{' | b'[' => {
                depth = depth.saturating_add(1);
                index += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            b'"' => {
                let string_start = index + 1;
                index = string_start;
                let mut escaped = false;
                while index < line.len() {
                    let byte = line[index];
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        break;
                    }
                    index += 1;
                }
                if index >= line.len() {
                    return false;
                }
                let string_end = index;
                index += 1;
                if depth != 1 || line.get(string_start..string_end) != Some(key) {
                    continue;
                }
                while line.get(index).is_some_and(u8::is_ascii_whitespace) {
                    index += 1;
                }
                if line.get(index) != Some(&b':') {
                    continue;
                }
                index += 1;
                while line.get(index).is_some_and(u8::is_ascii_whitespace) {
                    index += 1;
                }
                if line.get(index) != Some(&b'"') {
                    continue;
                }
                let value_start = index + 1;
                let value_end = value_start.saturating_add(expected.len());
                return line.get(value_start..value_end) == Some(expected)
                    && line.get(value_end) == Some(&b'"');
            }
            _ => index += 1,
        }
    }
    false
}

fn parse_native_jsonl_line(session: &mut CachedNativeSession, line: &[u8]) {
    match session.kind {
        NativeSessionKind::Codex => parse_codex_line(session, line),
        NativeSessionKind::Workbuddy => parse_workbuddy_line(session, line),
        NativeSessionKind::Claude => parse_claude_audit_result(session, line),
        NativeSessionKind::Hermes => {}
    }
}

fn parse_codex_line(session: &mut CachedNativeSession, line: &[u8]) {
    let Ok(value) = serde_json::from_slice::<CodexJsonlLine>(line) else {
        return;
    };
    if value.kind.as_deref() != Some("event_msg") {
        return;
    }
    let Some(payload) = value.payload else {
        return;
    };
    match payload {
        CodexJsonlPayload::TaskStarted => {
            session.status = AgentStatus::Running;
            session.saw_lifecycle = true;
        }
        CodexJsonlPayload::TaskComplete => {
            session.status = AgentStatus::Completed;
            session.saw_lifecycle = true;
        }
        CodexJsonlPayload::TurnAborted => {
            session.status = AgentStatus::Failed;
            session.saw_lifecycle = true;
        }
        CodexJsonlPayload::AgentMessage { message, phase } => {
            if let Some(message) = message.as_deref() {
                session.latest_reply = bounded_text(message, MAX_REPLY_BYTES);
            }
            if !session.saw_lifecycle {
                session.status = if phase.as_deref() == Some("final") {
                    AgentStatus::Completed
                } else {
                    AgentStatus::Running
                };
            }
        }
        CodexJsonlPayload::Other => {}
    }
}

fn parse_workbuddy_line(session: &mut CachedNativeSession, line: &[u8]) {
    let Ok(value) = serde_json::from_slice::<WorkbuddyJsonlLine>(line) else {
        return;
    };
    if value.kind.as_deref() != Some("message") || value.role.as_deref() != Some("assistant") {
        return;
    }
    session.status = match value
        .status
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("running" | "in_progress" | "inprogress" | "streaming" | "pending") => {
            AgentStatus::Running
        }
        Some("failed" | "error" | "cancelled" | "canceled") => AgentStatus::Failed,
        Some("waiting") => AgentStatus::Waiting,
        Some("timeout" | "timed_out") => AgentStatus::Timeout,
        Some("completed" | "complete" | "success") | None => AgentStatus::Completed,
        Some(_) => AgentStatus::Completed,
    };
    for item in value.content {
        if item.kind.as_deref() != Some("text") {
            continue;
        }
        if let Some(text) = item.text.as_deref() {
            if let Some(text) = bounded_text(text, MAX_REPLY_BYTES) {
                session.latest_reply = Some(text);
            }
        }
    }
}

fn parse_claude_audit_result(session: &mut CachedNativeSession, line: &[u8]) {
    let Ok(value) = serde_json::from_slice::<ClaudeAuditResultLine>(line) else {
        return;
    };
    if value.kind.as_deref() != Some("result") {
        return;
    }
    let failed = value.is_error.unwrap_or(false) || value.subtype.as_deref() != Some("success");
    session.status = if failed {
        AgentStatus::Failed
    } else {
        AgentStatus::Completed
    };
    session.saw_lifecycle = true;
    if !failed {
        session.latest_reply = value
            .result
            .as_deref()
            .and_then(|result| bounded_text(result, MAX_REPLY_BYTES));
    }
}

#[cfg(windows)]
fn native_file_identity(file: &File) -> Result<NativeFileIdentity, CommandError> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information)
            .map_err(|_| io_failure())?;
    }
    Ok(NativeFileIdentity {
        volume: u64::from(information.dwVolumeSerialNumber),
        index: (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    })
}

#[cfg(unix)]
fn native_file_identity(file: &File) -> Result<NativeFileIdentity, CommandError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata().map_err(|_| io_failure())?;
    Ok(NativeFileIdentity {
        volume: metadata.dev(),
        index: metadata.ino(),
    })
}

#[cfg(not(any(windows, unix)))]
fn native_file_identity(file: &File) -> Result<NativeFileIdentity, CommandError> {
    let metadata = file.metadata().map_err(|_| io_failure())?;
    Ok(NativeFileIdentity {
        volume: 0,
        index: metadata.len(),
    })
}

#[cfg(windows)]
fn native_last_write_marker(metadata: &std::fs::Metadata) -> u64 {
    use std::os::windows::fs::MetadataExt;

    metadata.last_write_time()
}

#[cfg(not(windows))]
fn native_last_write_marker(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos() as u64)
}

fn age_status(status: AgentStatus, updated_at: i64, now: i64) -> (AgentStatus, i64) {
    if matches!(status, AgentStatus::Running | AgentStatus::Completed)
        && now > updated_at.saturating_add(COMPLETED_FRESHNESS_MILLIS)
    {
        return (
            AgentStatus::Idle,
            updated_at.saturating_add(COMPLETED_FRESHNESS_MILLIS),
        );
    }
    (status, updated_at)
}

fn safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn bounded_text(value: &str, max_bytes: usize) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    Some(value[..end].to_owned())
}

fn io_failure() -> CommandError {
    CommandError {
        code: AppErrorCode::IoFailure,
        message_key: "errors.ioFailure".into(),
        details: SafeMessageParameters::new(),
        retryable: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agents::{agent_reply_preview_from_message, AGENT_REPLY_MESSAGE_PREFIX};
    use rusqlite::Connection;
    use std::fs::{self, OpenOptions};
    use std::io::Write;

    fn create_codex_fixture(
        root: &Path,
        updated_at_ms: i64,
        lines: &[serde_json::Value],
    ) -> PathBuf {
        let codex = root.join(".codex");
        let sessions = codex.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let rollout = sessions.join("rollout.jsonl");
        let body = lines
            .iter()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&rollout, format!("{body}\n")).unwrap();
        let connection = Connection::open(codex.join("state_5.sqlite")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE threads(
                    id TEXT PRIMARY KEY,
                    rollout_path TEXT NOT NULL,
                    updated_at INTEGER NOT NULL,
                    updated_at_ms INTEGER,
                    title TEXT NOT NULL,
                    archived INTEGER NOT NULL DEFAULT 0
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads(id, rollout_path, updated_at, updated_at_ms, title, archived)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0)",
                rusqlite::params![
                    "codex-session",
                    rollout.to_string_lossy(),
                    updated_at_ms / 1000,
                    updated_at_ms,
                    "Fix native status"
                ],
            )
            .unwrap();
        rollout
    }

    fn create_workbuddy_fixture(
        root: &Path,
        updated_at_ms: i64,
        status: &str,
        lines: &[serde_json::Value],
    ) -> PathBuf {
        let workbuddy = root.join(".workbuddy");
        let project = workbuddy.join("projects/project-one");
        fs::create_dir_all(&project).unwrap();
        let body = lines
            .iter()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let session_path = project.join("workbuddy-session.jsonl");
        fs::write(&session_path, format!("{body}\n")).unwrap();
        let connection = Connection::open(workbuddy.join("workbuddy.db")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE sessions(
                    id TEXT PRIMARY KEY,
                    cwd TEXT NOT NULL,
                    title TEXT,
                    custom_title TEXT,
                    status TEXT NOT NULL,
                    updated_at INTEGER NOT NULL,
                    last_activity_at INTEGER,
                    deleted_at INTEGER
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO sessions(id, cwd, title, custom_title, status, updated_at, last_activity_at, deleted_at)
                 VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?5, NULL)",
                rusqlite::params![
                    "workbuddy-session",
                    r"C:\workbuddy\project-one",
                    "Review status",
                    status,
                    updated_at_ms
                ],
            )
            .unwrap();
        session_path
    }

    fn append_json_line(path: &Path, value: &serde_json::Value) -> Vec<u8> {
        let bytes = format!("{value}\n").into_bytes();
        OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        bytes
    }

    fn set_codex_updated_at(root: &Path, updated_at_ms: i64) {
        Connection::open(root.join(".codex/state_5.sqlite"))
            .unwrap()
            .execute(
                "UPDATE threads SET updated_at = ?1, updated_at_ms = ?2 WHERE id = 'codex-session'",
                rusqlite::params![updated_at_ms / 1000, updated_at_ms],
            )
            .unwrap();
    }

    fn set_workbuddy_status(root: &Path, updated_at_ms: i64, status: &str) {
        Connection::open(root.join(".workbuddy/workbuddy.db"))
            .unwrap()
            .execute(
                "UPDATE sessions SET status = ?1, updated_at = ?2, last_activity_at = ?2
                  WHERE id = 'workbuddy-session'",
                rusqlite::params![status, updated_at_ms],
            )
            .unwrap();
    }

    fn create_claude_desktop_fixture(root: &Path, lines: &[serde_json::Value]) -> PathBuf {
        let session = root.join(
            "AppData/Local/Claude-3p/local-agent-mode-sessions/tenant/00000000/local_claude-session",
        );
        fs::create_dir_all(&session).unwrap();
        let audit = session.join("audit.jsonl");
        let body = lines
            .iter()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&audit, format!("{body}\n")).unwrap();
        audit
    }

    #[test]
    fn codex_real_rollout_projects_running_and_only_the_latest_agent_reply() {
        let directory = tempfile::tempdir().unwrap();
        create_codex_fixture(
            directory.path(),
            10_000,
            &[
                serde_json::json!({"timestamp":"2026-08-15T10:00:00Z","type":"event_msg","payload":{"type":"task_started"}}),
                serde_json::json!({"timestamp":"2026-08-15T10:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"private user prompt"}]}}),
                serde_json::json!({"timestamp":"2026-08-15T10:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"private event prompt"}}),
                serde_json::json!({"timestamp":"2026-08-15T10:00:02Z","type":"event_msg","payload":{"type":"agent_message","message":"真实 Codex 回复","phase":"commentary"}}),
            ],
        );

        let reader = NativeAgentActivityReader::new(directory.path().to_path_buf());
        let activity = reader
            .latest_activity(AgentId::Codex, 10_100)
            .unwrap()
            .expect("real Codex rollout should be detected");

        assert_eq!(activity.status, AgentStatus::Running);
        assert_eq!(activity.session_id, "codex-session");
        assert_eq!(activity.title.as_deref(), Some("Fix native status"));
        assert_eq!(activity.latest_reply.as_deref(), Some("真实 Codex 回复"));
        assert!(!activity
            .latest_reply
            .as_deref()
            .unwrap()
            .contains("private user prompt"));
        assert_eq!(reader.scan_metrics(AgentId::Codex).parser_calls, 2);
    }

    #[test]
    fn workbuddy_real_session_projects_completed_and_only_assistant_text() {
        let directory = tempfile::tempdir().unwrap();
        create_workbuddy_fixture(
            directory.path(),
            20_000,
            "completed",
            &[
                serde_json::json!({"id":"u","timestamp":19_000,"type":"message","role":"user","content":[{"type":"text","text":"private user prompt"}]}),
                serde_json::json!({"id":"r","timestamp":19_500,"type":"reasoning","content":[{"type":"text","text":"private reasoning"}]}),
                serde_json::json!({"id":"a","timestamp":20_000,"type":"message","role":"assistant","status":"completed","content":[{"type":"text","text":"真实 WorkBuddy 回复"}]}),
            ],
        );

        let activity = NativeAgentActivityReader::new(directory.path().to_path_buf())
            .latest_activity(AgentId::Workbuddy, 20_100)
            .unwrap()
            .expect("real WorkBuddy session should be detected");

        assert_eq!(activity.status, AgentStatus::Completed);
        assert_eq!(activity.session_id, "workbuddy-session");
        assert_eq!(activity.title.as_deref(), Some("Review status"));
        assert_eq!(
            activity.latest_reply.as_deref(),
            Some("真实 WorkBuddy 回复")
        );
        assert!(agent_reply_preview_from_message(&format!(
            "{AGENT_REPLY_MESSAGE_PREFIX}{}",
            activity.latest_reply.unwrap()
        ))
        .is_some());
    }

    #[test]
    fn hermes_state_db_projects_user_as_running_then_only_final_assistant_text() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("AppData/Local/hermes");
        fs::create_dir_all(&root).unwrap();
        let connection = Connection::open(root.join("state.db")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE sessions(id TEXT PRIMARY KEY, archived INTEGER, ended_at REAL);
                 CREATE TABLE messages(
                    id INTEGER PRIMARY KEY, session_id TEXT NOT NULL, role TEXT NOT NULL,
                    content TEXT, tool_calls TEXT, timestamp REAL NOT NULL,
                    finish_reason TEXT, reasoning TEXT
                 );
                 INSERT INTO sessions VALUES ('hermes-session', 0, NULL);
                 INSERT INTO messages VALUES (
                    1, 'hermes-session', 'user', 'private user prompt', NULL, 10.0, NULL,
                    'private reasoning'
                 );",
            )
            .unwrap();
        let reader = NativeAgentActivityReader::new(directory.path().to_path_buf());

        let running = reader
            .latest_activity(AgentId::Hermes, 10_100)
            .unwrap()
            .expect("Hermes user turn should be detected");
        assert_eq!(running.status, AgentStatus::Running);
        assert_eq!(running.latest_reply, None);

        connection
            .execute(
                "INSERT INTO messages VALUES (
                    2, 'hermes-session', 'assistant', 'Hermes 安全回复', NULL, 11.0, 'stop',
                    'private assistant reasoning'
                 )",
                [],
            )
            .unwrap();
        reader.mark_dirty_for_test(AgentId::Hermes);

        let completed = reader
            .latest_activity(AgentId::Hermes, 11_100)
            .unwrap()
            .unwrap();
        assert_eq!(completed.status, AgentStatus::Completed);
        assert_eq!(completed.latest_reply.as_deref(), Some("Hermes 安全回复"));
        assert!(!completed.latest_reply.unwrap().contains("private"));
    }

    #[test]
    fn claude_desktop_audit_projects_requesting_then_only_the_final_result() {
        let directory = tempfile::tempdir().unwrap();
        let audit = create_claude_desktop_fixture(
            directory.path(),
            &[
                serde_json::json!({"type":"user","message":{"role":"user","content":"private user prompt"}}),
                serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[
                    {"type":"thinking","thinking":"private reasoning"},
                    {"type":"tool_use","input":{"path":"private tool input"}}
                ]}}),
                serde_json::json!({"type":"system","subtype":"status","status":"requesting"}),
            ],
        );
        let reader = NativeAgentActivityReader::new(directory.path().to_path_buf());

        let running = reader
            .latest_activity(AgentId::Claude, 40_100)
            .unwrap()
            .expect("Claude Desktop audit should be detected");
        assert_eq!(running.status, AgentStatus::Running);
        assert_eq!(running.latest_reply, None);
        assert_eq!(reader.scan_metrics(AgentId::Claude).parser_calls, 0);

        append_json_line(
            &audit,
            &serde_json::json!({
                "type":"result",
                "subtype":"success",
                "is_error":false,
                "result":"Claude 安全回复"
            }),
        );
        reader.mark_dirty_for_test(AgentId::Claude);
        let completed = reader
            .latest_activity(AgentId::Claude, 40_200)
            .unwrap()
            .unwrap();

        assert_eq!(completed.status, AgentStatus::Completed);
        assert_eq!(completed.latest_reply.as_deref(), Some("Claude 安全回复"));
        assert!(!completed.latest_reply.unwrap().contains("private"));
        let after_complete = reader.scan_metrics(AgentId::Claude);
        assert_eq!(after_complete.parser_calls, 1);

        let unchanged = reader
            .latest_activity(AgentId::Claude, 40_201)
            .unwrap()
            .unwrap();
        assert_eq!(unchanged.status, AgentStatus::Completed);
        assert_eq!(reader.scan_metrics(AgentId::Claude), after_complete);
    }

    #[test]
    fn completed_native_activity_ages_to_idle_without_losing_the_reply() {
        let directory = tempfile::tempdir().unwrap();
        create_workbuddy_fixture(
            directory.path(),
            20_000,
            "completed",
            &[
                serde_json::json!({"id":"a","timestamp":20_000,"type":"message","role":"assistant","status":"completed","content":[{"type":"text","text":"Finished"}]}),
            ],
        );

        let activity = NativeAgentActivityReader::new(directory.path().to_path_buf())
            .latest_activity(AgentId::Workbuddy, 20_000 + COMPLETED_FRESHNESS_MILLIS + 1)
            .unwrap()
            .unwrap();

        assert_eq!(activity.status, AgentStatus::Idle);
        assert_eq!(activity.latest_reply.as_deref(), Some("Finished"));
    }

    #[test]
    fn large_jsonl_initial_load_is_bounded_and_unchanged_second_scan_reads_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let rollout = create_codex_fixture(
            directory.path(),
            30_000,
            &[
                serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","message":"latest safe reply","phase":"commentary"}}),
            ],
        );
        let final_line = fs::read(&rollout).unwrap();
        let filler = format!(
            "{{\"type\":\"response_item\",\"payload\":\"{}\"}}\n",
            "x".repeat(4_000)
        );
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&rollout)
            .unwrap();
        let mut written = 0_u64;
        while written <= MAX_JSONL_TAIL_BYTES {
            file.write_all(filler.as_bytes()).unwrap();
            written += filler.len() as u64;
        }
        file.write_all(&final_line).unwrap();
        drop(file);

        let reader = NativeAgentActivityReader::new(directory.path().to_path_buf());
        let first = reader
            .latest_activity(AgentId::Codex, 30_100)
            .unwrap()
            .unwrap();
        let first_metrics = reader.scan_metrics(AgentId::Codex);
        assert_eq!(first.latest_reply.as_deref(), Some("latest safe reply"));
        assert_eq!(first_metrics.bytes_read, MAX_JSONL_TAIL_BYTES);
        assert!(first_metrics.parser_calls > 0);

        let second = reader
            .latest_activity(AgentId::Codex, 30_200)
            .unwrap()
            .unwrap();
        assert_eq!(second, first);
        assert_eq!(reader.scan_metrics(AgentId::Codex), first_metrics);
    }

    #[test]
    fn codex_growth_reads_only_the_appended_event_and_updates_status_and_reply() {
        let directory = tempfile::tempdir().unwrap();
        let rollout = create_codex_fixture(
            directory.path(),
            40_000,
            &[
                serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","message":"working","phase":"commentary"}}),
            ],
        );
        let reader = NativeAgentActivityReader::new(directory.path().to_path_buf());
        assert_eq!(
            reader
                .latest_activity(AgentId::Codex, 40_100)
                .unwrap()
                .unwrap()
                .status,
            AgentStatus::Running
        );
        let before = reader.scan_metrics(AgentId::Codex);
        let appended = append_json_line(
            &rollout,
            &serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","message":"done","phase":"final"}}),
        );
        set_codex_updated_at(directory.path(), 41_000);
        reader.mark_dirty_for_test(AgentId::Codex);

        let activity = reader
            .latest_activity(AgentId::Codex, 41_100)
            .unwrap()
            .unwrap();
        let after = reader.scan_metrics(AgentId::Codex);
        assert_eq!(activity.status, AgentStatus::Completed);
        assert_eq!(activity.latest_reply.as_deref(), Some("done"));
        assert_eq!(after.bytes_read - before.bytes_read, appended.len() as u64);
        assert_eq!(after.parser_calls - before.parser_calls, 1);
    }

    #[test]
    fn codex_partial_line_is_not_parsed_until_the_next_poll_completes_it() {
        let directory = tempfile::tempdir().unwrap();
        let rollout = create_codex_fixture(
            directory.path(),
            50_000,
            &[
                serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","message":"working","phase":"commentary"}}),
            ],
        );
        let reader = NativeAgentActivityReader::new(directory.path().to_path_buf());
        reader
            .latest_activity(AgentId::Codex, 50_100)
            .unwrap()
            .unwrap();
        let event = format!(
            "{}\n",
            serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","message":"complete after split","phase":"final"}})
        )
        .into_bytes();
        let split = event.len() / 2;
        OpenOptions::new()
            .append(true)
            .open(&rollout)
            .unwrap()
            .write_all(&event[..split])
            .unwrap();
        reader.mark_dirty_for_test(AgentId::Codex);
        let before_partial = reader.scan_metrics(AgentId::Codex);

        let partial = reader
            .latest_activity(AgentId::Codex, 50_200)
            .unwrap()
            .unwrap();
        let after_partial = reader.scan_metrics(AgentId::Codex);
        assert_eq!(partial.latest_reply.as_deref(), Some("working"));
        assert_eq!(
            after_partial.bytes_read - before_partial.bytes_read,
            split as u64
        );
        assert_eq!(after_partial.parser_calls, before_partial.parser_calls);

        OpenOptions::new()
            .append(true)
            .open(&rollout)
            .unwrap()
            .write_all(&event[split..])
            .unwrap();
        set_codex_updated_at(directory.path(), 51_000);
        reader.mark_dirty_for_test(AgentId::Codex);
        let completed = reader
            .latest_activity(AgentId::Codex, 51_100)
            .unwrap()
            .unwrap();
        let after_completed = reader.scan_metrics(AgentId::Codex);
        assert_eq!(completed.status, AgentStatus::Completed);
        assert_eq!(
            completed.latest_reply.as_deref(),
            Some("complete after split")
        );
        assert_eq!(
            after_completed.bytes_read - after_partial.bytes_read,
            (event.len() - split) as u64
        );
        assert_eq!(after_completed.parser_calls - after_partial.parser_calls, 1);
    }

    #[test]
    fn codex_truncation_rebuilds_the_cached_projection() {
        let directory = tempfile::tempdir().unwrap();
        let rollout = create_codex_fixture(
            directory.path(),
            60_000,
            &[
                serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","message":"old reply with padding padding padding","phase":"commentary"}}),
                serde_json::json!({"type":"response_item","payload":{"type":"tool_output","body":"ignored"}}),
            ],
        );
        let reader = NativeAgentActivityReader::new(directory.path().to_path_buf());
        reader
            .latest_activity(AgentId::Codex, 60_100)
            .unwrap()
            .unwrap();
        let replacement = format!(
            "{}\n",
            serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","message":"new reply","phase":"final"}})
        );
        fs::write(&rollout, replacement.as_bytes()).unwrap();
        set_codex_updated_at(directory.path(), 61_000);
        reader.mark_dirty_for_test(AgentId::Codex);
        let before = reader.scan_metrics(AgentId::Codex);

        let activity = reader
            .latest_activity(AgentId::Codex, 61_100)
            .unwrap()
            .unwrap();
        let after = reader.scan_metrics(AgentId::Codex);
        assert_eq!(activity.status, AgentStatus::Completed);
        assert_eq!(activity.latest_reply.as_deref(), Some("new reply"));
        assert_eq!(
            after.bytes_read - before.bytes_read,
            replacement.len() as u64
        );
        assert_eq!(after.parser_calls - before.parser_calls, 1);
    }

    #[test]
    fn codex_rotation_rebuilds_even_when_the_replacement_is_not_shorter() {
        let directory = tempfile::tempdir().unwrap();
        let rollout = create_codex_fixture(
            directory.path(),
            70_000,
            &[
                serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","message":"old","phase":"commentary"}}),
            ],
        );
        let reader = NativeAgentActivityReader::new(directory.path().to_path_buf());
        reader
            .latest_activity(AgentId::Codex, 70_100)
            .unwrap()
            .unwrap();
        fs::rename(&rollout, rollout.with_extension("jsonl.old")).unwrap();
        let replacement = format!(
            "{}\n",
            serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","message":"replacement reply is deliberately longer","phase":"final"}})
        );
        fs::write(&rollout, replacement.as_bytes()).unwrap();
        set_codex_updated_at(directory.path(), 71_000);
        reader.mark_dirty_for_test(AgentId::Codex);
        let before = reader.scan_metrics(AgentId::Codex);

        let activity = reader
            .latest_activity(AgentId::Codex, 71_100)
            .unwrap()
            .unwrap();
        let after = reader.scan_metrics(AgentId::Codex);
        assert_eq!(activity.status, AgentStatus::Completed);
        assert_eq!(
            activity.latest_reply.as_deref(),
            Some("replacement reply is deliberately longer")
        );
        assert_eq!(
            after.bytes_read - before.bytes_read,
            replacement.len() as u64
        );
        assert_eq!(after.parser_calls - before.parser_calls, 1);
    }

    #[test]
    fn workbuddy_unchanged_poll_is_zero_and_growth_reads_only_the_new_assistant_line() {
        let directory = tempfile::tempdir().unwrap();
        let session = create_workbuddy_fixture(
            directory.path(),
            80_000,
            "running",
            &[
                serde_json::json!({"id":"a","type":"message","role":"assistant","content":[{"type":"text","text":"working"}]} ),
            ],
        );
        let reader = NativeAgentActivityReader::new(directory.path().to_path_buf());
        reader
            .latest_activity(AgentId::Workbuddy, 80_100)
            .unwrap()
            .unwrap();
        let initial = reader.scan_metrics(AgentId::Workbuddy);
        reader
            .latest_activity(AgentId::Workbuddy, 80_200)
            .unwrap()
            .unwrap();
        assert_eq!(reader.scan_metrics(AgentId::Workbuddy), initial);

        let appended = append_json_line(
            &session,
            &serde_json::json!({"id":"b","type":"message","role":"assistant","content":[{"type":"text","text":"done"}]}),
        );
        set_workbuddy_status(directory.path(), 81_000, "completed");
        reader.mark_dirty_for_test(AgentId::Workbuddy);
        let activity = reader
            .latest_activity(AgentId::Workbuddy, 81_100)
            .unwrap()
            .unwrap();
        let after = reader.scan_metrics(AgentId::Workbuddy);
        assert_eq!(activity.status, AgentStatus::Completed);
        assert_eq!(activity.latest_reply.as_deref(), Some("done"));
        assert_eq!(after.bytes_read - initial.bytes_read, appended.len() as u64);
        assert_eq!(after.parser_calls - initial.parser_calls, 1);
    }

    #[test]
    fn workbuddy_final_assistant_reply_beats_a_stale_running_database_status() {
        let directory = tempfile::tempdir().unwrap();
        let session = create_workbuddy_fixture(
            directory.path(),
            81_500,
            "running",
            &[
                serde_json::json!({"id":"a","type":"message","role":"assistant","status":"streaming","content":[{"type":"text","text":"working"}]}),
            ],
        );
        let reader = NativeAgentActivityReader::new(directory.path().to_path_buf());
        let running = reader
            .latest_activity(AgentId::Workbuddy, 81_600)
            .unwrap()
            .unwrap();
        assert_eq!(running.status, AgentStatus::Running);

        append_json_line(
            &session,
            &serde_json::json!({"id":"b","type":"message","role":"assistant","content":[{"type":"text","text":"final safe reply"}]}),
        );
        set_workbuddy_status(directory.path(), 82_000, "running");
        reader.mark_dirty_for_test(AgentId::Workbuddy);

        let completed = reader
            .latest_activity(AgentId::Workbuddy, 82_100)
            .unwrap()
            .unwrap();
        assert_eq!(completed.status, AgentStatus::Completed);
        assert_eq!(completed.latest_reply.as_deref(), Some("final safe reply"));
    }

    #[test]
    fn workbuddy_trailing_non_assistant_metadata_does_not_reopen_a_completed_turn() {
        let directory = tempfile::tempdir().unwrap();
        let session = create_workbuddy_fixture(
            directory.path(),
            82_000,
            "running",
            &[
                serde_json::json!({"id":"a","type":"message","role":"assistant","status":"streaming","content":[{"type":"text","text":"working"}]}),
            ],
        );
        let reader = NativeAgentActivityReader::new(directory.path().to_path_buf());
        reader
            .latest_activity(AgentId::Workbuddy, 82_100)
            .unwrap()
            .unwrap();

        append_json_line(
            &session,
            &serde_json::json!({"id":"b","type":"message","role":"assistant","content":[{"type":"text","text":"final safe reply"}]}),
        );
        set_workbuddy_status(directory.path(), 83_000, "running");
        reader.mark_dirty_for_test(AgentId::Workbuddy);
        let completed = reader
            .latest_activity(AgentId::Workbuddy, 83_100)
            .unwrap()
            .unwrap();
        assert_eq!(completed.status, AgentStatus::Completed);
        let before_trailing_metadata = reader.scan_metrics(AgentId::Workbuddy);

        let appended = append_json_line(
            &session,
            &serde_json::json!({"id":"private","type":"reasoning","content":[{"type":"text","text":"must never be parsed or exposed"}]}),
        );
        set_workbuddy_status(directory.path(), 84_000, "running");
        reader.mark_dirty_for_test(AgentId::Workbuddy);
        let after_trailing_metadata = reader
            .latest_activity(AgentId::Workbuddy, 84_100)
            .unwrap()
            .unwrap();
        let metrics = reader.scan_metrics(AgentId::Workbuddy);

        assert_eq!(after_trailing_metadata.status, AgentStatus::Completed);
        assert_eq!(
            after_trailing_metadata.latest_reply.as_deref(),
            Some("final safe reply")
        );
        assert_eq!(
            metrics.bytes_read - before_trailing_metadata.bytes_read,
            appended.len() as u64
        );
        assert_eq!(
            metrics.parser_calls - before_trailing_metadata.parser_calls,
            0
        );
    }

    #[test]
    fn workbuddy_user_message_restores_running_when_the_database_still_says_completed() {
        let directory = tempfile::tempdir().unwrap();
        let session = create_workbuddy_fixture(
            directory.path(),
            82_000,
            "completed",
            &[
                serde_json::json!({"id":"a","type":"message","role":"assistant","status":"completed","content":[{"type":"text","text":"previous safe reply"}]}),
            ],
        );
        let reader = NativeAgentActivityReader::new(directory.path().to_path_buf());
        let completed = reader
            .latest_activity(AgentId::Workbuddy, 82_100)
            .unwrap()
            .unwrap();
        assert_eq!(completed.status, AgentStatus::Completed);
        let before = reader.scan_metrics(AgentId::Workbuddy);

        let appended = append_json_line(
            &session,
            &serde_json::json!({"id":"private","type":"message","role":"user","content":[{"type":"text","text":"must never be parsed or exposed"}]}),
        );
        set_workbuddy_status(directory.path(), 83_000, "completed");
        reader.mark_dirty_for_test(AgentId::Workbuddy);

        let activity = reader
            .latest_activity(AgentId::Workbuddy, 83_100)
            .unwrap()
            .unwrap();
        let after = reader.scan_metrics(AgentId::Workbuddy);
        assert_eq!(activity.status, AgentStatus::Running);
        assert_eq!(
            activity.latest_reply.as_deref(),
            Some("previous safe reply")
        );
        assert_eq!(after.bytes_read - before.bytes_read, appended.len() as u64);
        assert_eq!(after.parser_calls - before.parser_calls, 0);
    }

    #[test]
    fn large_codex_growth_is_consumed_in_bounded_incremental_chunks() {
        let directory = tempfile::tempdir().unwrap();
        let rollout = create_codex_fixture(
            directory.path(),
            90_000,
            &[
                serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","message":"working","phase":"commentary"}}),
            ],
        );
        let reader = NativeAgentActivityReader::new(directory.path().to_path_buf());
        reader
            .latest_activity(AgentId::Codex, 90_100)
            .unwrap()
            .unwrap();
        let before = reader.scan_metrics(AgentId::Codex);
        let filler = format!(
            "{{\"type\":\"response_item\",\"payload\":\"{}\"}}\n",
            "x".repeat(1_000)
        );
        let mut appended = Vec::new();
        while appended.len() <= MAX_INCREMENTAL_JSONL_BYTES as usize + 32 * 1024 {
            appended.extend_from_slice(filler.as_bytes());
        }
        appended.extend_from_slice(
            format!(
                "{}\n",
                serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","message":"done after backlog","phase":"final"}})
            )
            .as_bytes(),
        );
        OpenOptions::new()
            .append(true)
            .open(&rollout)
            .unwrap()
            .write_all(&appended)
            .unwrap();
        set_codex_updated_at(directory.path(), 91_000);
        reader.mark_dirty_for_test(AgentId::Codex);

        let first_chunk = reader
            .latest_activity(AgentId::Codex, 91_100)
            .unwrap()
            .unwrap();
        let after_first = reader.scan_metrics(AgentId::Codex);
        assert_eq!(first_chunk.latest_reply.as_deref(), Some("working"));
        assert_eq!(
            after_first.bytes_read - before.bytes_read,
            MAX_INCREMENTAL_JSONL_BYTES
        );

        let completed = reader
            .latest_activity(AgentId::Codex, 91_200)
            .unwrap()
            .unwrap();
        let after_second = reader.scan_metrics(AgentId::Codex);
        assert_eq!(completed.status, AgentStatus::Completed);
        assert_eq!(
            completed.latest_reply.as_deref(),
            Some("done after backlog")
        );
        assert_eq!(
            after_second.bytes_read - before.bytes_read,
            appended.len() as u64
        );
    }

    #[test]
    fn codex_session_switch_rebuilds_from_the_new_rollout() {
        let directory = tempfile::tempdir().unwrap();
        create_codex_fixture(
            directory.path(),
            100_000,
            &[
                serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","message":"old session","phase":"commentary"}}),
            ],
        );
        let reader = NativeAgentActivityReader::new(directory.path().to_path_buf());
        reader
            .latest_activity(AgentId::Codex, 100_100)
            .unwrap()
            .unwrap();
        let new_rollout = directory
            .path()
            .join(".codex/sessions/rollout-new-session.jsonl");
        let new_bytes = format!(
            "{}\n",
            serde_json::json!({"type":"event_msg","payload":{"type":"agent_message","message":"new session reply","phase":"final"}})
        );
        fs::write(&new_rollout, new_bytes.as_bytes()).unwrap();
        Connection::open(directory.path().join(".codex/state_5.sqlite"))
            .unwrap()
            .execute(
                "INSERT INTO threads(id, rollout_path, updated_at, updated_at_ms, title, archived)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0)",
                rusqlite::params![
                    "codex-session-2",
                    new_rollout.to_string_lossy(),
                    101,
                    101_000,
                    "New native session"
                ],
            )
            .unwrap();
        reader.mark_dirty_for_test(AgentId::Codex);
        let before = reader.scan_metrics(AgentId::Codex);

        let activity = reader
            .latest_activity(AgentId::Codex, 101_100)
            .unwrap()
            .unwrap();
        let after = reader.scan_metrics(AgentId::Codex);
        assert_eq!(activity.session_id, "codex-session-2");
        assert_eq!(activity.status, AgentStatus::Completed);
        assert_eq!(activity.title.as_deref(), Some("New native session"));
        assert_eq!(activity.latest_reply.as_deref(), Some("new session reply"));
        assert_eq!(after.bytes_read - before.bytes_read, new_bytes.len() as u64);
        assert_eq!(after.parser_calls - before.parser_calls, 1);
    }
}
