use crate::contracts::{AgentEnvironment, AgentId, AppErrorCode, CommandError, SafeParameterValue};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::Manager;

pub struct HookInvocation {
    pub agent_id: AgentId,
    pub environment: AgentEnvironment,
    pub native_event: String,
    pub output_path: PathBuf,
}

pub struct HookAssetPaths {
    pub paths: Vec<HookAssetPath>,
    pub wsl_available: bool,
    pub wsl_status_dir: Option<String>,
}

pub struct HookAssetPath {
    pub agent_id: AgentId,
    pub environment: AgentEnvironment,
    pub destination: HookAssetDestination,
}

#[derive(Debug, PartialEq, Eq)]
pub enum HookAssetDestination {
    Windows(PathBuf),
    Wsl(String),
}
impl HookAssetPaths {
    pub fn get(&self, agent_id: AgentId, environment: AgentEnvironment) -> Option<&Path> {
        self.paths
            .iter()
            .find(|entry| entry.agent_id == agent_id && entry.environment == environment)
            .and_then(|entry| match &entry.destination {
                HookAssetDestination::Windows(path) => Some(path.as_path()),
                HookAssetDestination::Wsl(_) => None,
            })
    }
}

pub fn windows_hook_command(invocation: &HookInvocation, script_path: &Path) -> String {
    let arguments = [
        script_path.to_string_lossy().into_owned(),
        agent_name(&invocation.agent_id).into(),
        environment_name(&invocation.environment).into(),
        invocation.native_event.clone(),
        invocation.output_path.to_string_lossy().into_owned(),
    ];
    format!(
        "powershell.exe -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File {} -Agent {} -Environment {} -NativeEvent {} -OutputPath {}",
        windows_quote(&arguments[0]),
        windows_quote(&arguments[1]),
        windows_quote(&arguments[2]),
        windows_quote(&arguments[3]),
        windows_quote(&arguments[4]),
    )
}

pub fn wsl_hook_command(invocation: &HookInvocation, script_path: &str) -> String {
    [
        shell_quote(script_path),
        "--agent".into(),
        shell_quote(agent_name(&invocation.agent_id)),
        "--environment".into(),
        shell_quote(environment_name(&invocation.environment)),
        "--native-event".into(),
        shell_quote(&invocation.native_event),
        "--output-path".into(),
        shell_quote(&invocation.output_path.to_string_lossy()),
    ]
    .join(" ")
}

pub fn install_hook_assets(app: &tauri::AppHandle) -> Result<HookAssetPaths, CommandError> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|_| hook_asset_error("resourceDir"))?;
    let windows_app_data = app
        .path()
        .app_data_dir()
        .map_err(|_| hook_asset_error("appDataDir"))?;
    let windows_source = resource_dir
        .join("agent-hooks")
        .join("aiceland-status-windows.ps1");
    let profile_event_source = resource_dir
        .join("agent-hooks")
        .join("aiceland-profile-event-windows.ps1");
    let wsl_source = resource_dir
        .join("agent-hooks")
        .join("aiceland-status-wsl.sh");
    let paths = install_hook_assets_with(
        &windows_app_data,
        &windows_source,
        &wsl_source,
        &SystemWslHookAssetPort,
    )?;
    install_one(
        &profile_event_source,
        &windows_app_data
            .join("agent-hooks")
            .join("aiceland-profile-event-windows.ps1"),
    )?;
    if paths.wsl_available {
        install_wsl_config_helper(
            &resource_dir
                .join("agent-hooks")
                .join("aiceland-config-wsl.sh"),
            &SystemWslHookAssetPort,
        )?;
    }
    Ok(paths)
}

// This helper is intentionally not a status destination: seven status files remain the complete
// public HookAssetPaths collection. It is a package-owned WSL file-operation endpoint only.
fn install_wsl_config_helper(
    source: &Path,
    wsl: &dyn WslHookAssetPort,
) -> Result<(), CommandError> {
    let home = wsl.home()?;
    let destination = format!(
        "{}/.local/share/aiceland/agent-hooks/aiceland-config-wsl.sh",
        home.trim_end_matches('/')
    );
    let expected_sha = sha256_hex(&fs::read(source).map_err(|_| hook_asset_error("readResource"))?);
    let source = wsl.unix_path(source)?;
    wsl.install(&source, &destination, &expected_sha)
}

fn install_one(source: &Path, destination: &Path) -> Result<PathBuf, CommandError> {
    let package_owned_bytes = fs::read(source).map_err(|_| hook_asset_error("readResource"))?;
    let parent = destination
        .parent()
        .ok_or_else(|| hook_asset_error("assetParent"))?;
    fs::create_dir_all(parent).map_err(|_| hook_asset_error("createAssetParent"))?;
    if !parent.is_dir() {
        return Err(hook_asset_error("assetParent"));
    }
    fs::write(destination, &package_owned_bytes).map_err(|_| hook_asset_error("writeAsset"))?;
    let installed_bytes = fs::read(destination).map_err(|_| hook_asset_error("verifyRead"))?;
    if sha256(&package_owned_bytes) != sha256(&installed_bytes) {
        return Err(hook_asset_error("sha256Mismatch"));
    }
    Ok(destination.to_owned())
}

trait WslHookAssetPort {
    fn home(&self) -> Result<String, CommandError>;
    fn unix_path(&self, source: &Path) -> Result<String, CommandError>;
    fn install(
        &self,
        source: &str,
        destination: &str,
        expected_sha: &str,
    ) -> Result<(), CommandError>;
}

struct SystemWslHookAssetPort;

#[cfg(windows)]
fn wsl_background_creation_flags() -> u32 {
    0x0800_0000
}

fn wsl_command() -> std::process::Command {
    let mut command = std::process::Command::new("wsl.exe");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(wsl_background_creation_flags());
    }
    command
}

impl WslHookAssetPort for SystemWslHookAssetPort {
    fn home(&self) -> Result<String, CommandError> {
        let output = wsl_command()
            .args(["--exec", "sh", "-lc", "printf %s \"$HOME\""])
            .output()
            .map_err(|_| hook_asset_error("wslUnavailable"))?;
        if !output.status.success() {
            return Err(hook_asset_error("wslUnavailable"));
        }
        let home = String::from_utf8(output.stdout).map_err(|_| hook_asset_error("wslHome"))?;
        let home = home.trim();
        if !is_unix_absolute_path(home) {
            return Err(hook_asset_error("wslHome"));
        }
        Ok(home.to_owned())
    }

    fn unix_path(&self, source: &Path) -> Result<String, CommandError> {
        unix_path_for_wsl(source)
    }

    fn install(
        &self,
        source: &str,
        destination: &str,
        expected_sha: &str,
    ) -> Result<(), CommandError> {
        let (parent, _) = destination
            .rsplit_once('/')
            .filter(|(parent, file)| is_unix_absolute_path(parent) && !file.is_empty())
            .ok_or_else(|| hook_asset_error("wslDestination"))?;
        let output = wsl_command()
            .args(["--exec", "sh", "-lc", "set -eu; umask 077; d=\"$1\"; s=\"$2\"; t=\"$3\"; mkdir -p -- \"$d\"; cp -- \"$s\" \"$t\"; chmod 700 -- \"$t\"; sha256sum -- \"$t\"", "--"])
            .arg(parent)
            .arg(source)
            .arg(destination)
            .output()
            .map_err(|_| hook_asset_error("wslCopy"))?;
        if !output.status.success() {
            return Err(hook_asset_error("wslCopy"));
        }
        let actual = String::from_utf8(output.stdout).map_err(|_| hook_asset_error("wslVerify"))?;
        if actual.split_whitespace().next() != Some(expected_sha) {
            return Err(hook_asset_error("sha256Mismatch"));
        }
        Ok(())
    }
}

fn unix_path_for_wsl(source: &Path) -> Result<String, CommandError> {
    let output = wsl_command()
        .args(["--exec", "wslpath", "-a"])
        .arg(source.as_os_str())
        .output()
        .map_err(|_| hook_asset_error("wslSource"))?;
    if !output.status.success() {
        return Err(hook_asset_error("wslSource"));
    }
    let source = String::from_utf8(output.stdout).map_err(|_| hook_asset_error("wslSource"))?;
    let source = source.trim();
    if !is_unix_absolute_path(source) {
        return Err(hook_asset_error("wslSource"));
    }
    Ok(source.to_owned())
}

fn is_unix_absolute_path(value: &str) -> bool {
    value.starts_with('/')
        && !value.contains('\\')
        && !value.contains('\n')
        && !value.contains('\r')
}

fn install_hook_assets_with(
    windows_app_data: &Path,
    windows_source: &Path,
    wsl_source: &Path,
    wsl: &dyn WslHookAssetPort,
) -> Result<HookAssetPaths, CommandError> {
    let mut paths = windows_asset_destinations(windows_app_data);
    for entry in &paths {
        let HookAssetDestination::Windows(destination) = &entry.destination else {
            unreachable!("windows asset list contains only Windows paths");
        };
        install_one(windows_source, destination)?;
    }

    let Ok(home) = wsl.home() else {
        return Ok(HookAssetPaths {
            paths,
            wsl_available: false,
            wsl_status_dir: None,
        });
    };
    let wsl_paths = wsl_asset_destinations(&home)?;
    let wsl_status_dir = wsl.unix_path(&windows_app_data.join("agent-status"))?;
    if !is_unix_absolute_path(&wsl_status_dir) {
        return Err(hook_asset_error("wslStatusDir"));
    }
    let expected_sha =
        sha256_hex(&fs::read(wsl_source).map_err(|_| hook_asset_error("readResource"))?);
    let wsl_source = wsl.unix_path(wsl_source)?;
    for entry in &wsl_paths {
        let HookAssetDestination::Wsl(destination) = &entry.destination else {
            unreachable!("WSL asset list contains only Unix paths");
        };
        wsl.install(&wsl_source, destination, &expected_sha)?;
    }
    paths.extend(wsl_paths);
    Ok(HookAssetPaths {
        paths,
        wsl_available: true,
        wsl_status_dir: Some(wsl_status_dir),
    })
}

fn hook_asset_error(reason: &str) -> CommandError {
    CommandError::with_detail(
        AppErrorCode::IoFailure,
        "errors.ioFailure",
        "reasonCode",
        SafeParameterValue::String(reason.into()),
        false,
    )
}

fn agent_name(value: &AgentId) -> &'static str {
    match value {
        AgentId::Codex => "codex",
        AgentId::Hermes => "hermes",
        AgentId::Workbuddy => "workbuddy",
        AgentId::Claude => "claude",
    }
}

fn environment_name(value: &AgentEnvironment) -> &'static str {
    match value {
        AgentEnvironment::Windows => "windows",
        AgentEnvironment::Wsl => "wsl",
    }
}

fn windows_quote(value: &str) -> String {
    let mut quoted = String::from("\"");
    let mut slashes = 0usize;
    for character in value.chars() {
        if character == '\\' {
            slashes += 1;
        } else if character == '\"' {
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
    quoted.push('\"');
    quoted
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\\"'\\\"'"))
}

fn normalize_status(agent: &str, native_event: &str, payload: &serde_json::Value) -> &'static str {
    let timeout = payload
        .pointer("/extra/choice")
        .and_then(serde_json::Value::as_str)
        == Some("timeout")
        || payload
            .get("failure_reason")
            .and_then(serde_json::Value::as_str)
            == Some("timeout")
        || payload.get("timeout").and_then(serde_json::Value::as_bool) == Some(true);
    let failed = payload.get("success").and_then(serde_json::Value::as_bool) == Some(false)
        || payload.get("failed").and_then(serde_json::Value::as_bool) == Some(true);
    match (agent, native_event) {
        (_, "PermissionRequest") | (_, "pre_approval_request") => "waiting",
        (_, "SessionStart")
        | (_, "UserPromptSubmit")
        | (_, "on_session_start")
        | (_, "pre_llm_call") => "running",
        (_, "StopFailure") => {
            if timeout {
                "timeout"
            } else {
                "failed"
            }
        }
        (_, "Stop") | (_, "post_llm_call") => "completed",
        (_, "SessionEnd") | (_, "on_session_end") => {
            if timeout {
                "timeout"
            } else if failed {
                "failed"
            } else {
                "idle"
            }
        }
        (_, "post_approval_response") => {
            if timeout {
                "timeout"
            } else {
                "running"
            }
        }
        _ => "running",
    }
}

fn fallback_event_id(
    agent: &str,
    environment: &str,
    task_id: &str,
    native_event: &str,
    sequence: Option<u64>,
    source_occurred_at: Option<i64>,
) -> String {
    let source_occurred_at = source_occurred_at
        .map(|value| value.to_string())
        .unwrap_or_else(|| "missing-occurred-at".into());
    let material = format!(
        "{agent}\n{environment}\n{task_id}\n{native_event}\n{}\n{source_occurred_at}",
        sequence.map(|value| value.to_string()).unwrap_or_default()
    );
    let mut encoded = String::with_capacity(64);
    for byte in sha256(material.as_bytes()) {
        encoded.push_str(&format!("{byte:02x}"));
    }
    format!("aiceland-{agent}-{environment}-{encoded}")
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    sha256(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn windows_asset_destinations(windows_app_data: &Path) -> Vec<HookAssetPath> {
    let mut paths = Vec::new();
    for agent in [
        AgentId::Codex,
        AgentId::Hermes,
        AgentId::Workbuddy,
        AgentId::Claude,
    ] {
        paths.push(HookAssetPath {
            agent_id: agent.clone(),
            environment: AgentEnvironment::Windows,
            destination: HookAssetDestination::Windows(
                windows_app_data
                    .join("agent-hooks")
                    .join(format!("{}-windows.ps1", agent_name(&agent))),
            ),
        });
    }
    paths
}

fn wsl_asset_destinations(wsl_home: &str) -> Result<Vec<HookAssetPath>, CommandError> {
    if !is_unix_absolute_path(wsl_home) {
        return Err(hook_asset_error("wslHome"));
    }
    let root = format!(
        "{}/.local/share/aiceland/agent-hooks",
        wsl_home.trim_end_matches('/')
    );
    let mut paths = Vec::new();
    for agent in [AgentId::Codex, AgentId::Hermes, AgentId::Claude] {
        paths.push(HookAssetPath {
            agent_id: agent.clone(),
            environment: AgentEnvironment::Wsl,
            destination: HookAssetDestination::Wsl(format!("{root}/{}-wsl.sh", agent_name(&agent))),
        });
    }
    Ok(paths)
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut message = bytes.to_vec();
    let bit_length = (message.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_be_bytes());
    let mut state = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    for chunk in message.chunks_exact(64) {
        let mut words = [0u32; 64];
        for (index, word) in words.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes(chunk[index * 4..index * 4 + 4].try_into().unwrap());
        }
        for index in 16..64 {
            words[index] = words[index - 16]
                .wrapping_add(
                    words[index - 15].rotate_right(7)
                        ^ words[index - 15].rotate_right(18)
                        ^ (words[index - 15] >> 3),
                )
                .wrapping_add(words[index - 7])
                .wrapping_add(
                    words[index - 2].rotate_right(17)
                        ^ words[index - 2].rotate_right(19)
                        ^ (words[index - 2] >> 10),
                );
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h) = (
            state[0], state[1], state[2], state[3], state[4], state[5], state[6], state[7],
        );
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ (!e & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }
    let mut output = [0u8; 32];
    for (index, word) in state.iter().enumerate() {
        output[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{AgentEnvironment, AgentId};
    use serde_json::Value;
    use std::fs;
    use std::io::Write;
    use std::path::Path;
    use std::process::{Command, Stdio};

    const CODEX_FIXTURE: &str =
        include_str!("../../tests/fixtures/agent-hooks/codex-session-start.json");
    const HERMES_FIXTURE: &str =
        include_str!("../../tests/fixtures/agent-hooks/hermes-approval-timeout.json");
    const CLAUDE_FIXTURE: &str =
        include_str!("../../tests/fixtures/agent-hooks/claude-stop-failure.json");

    // Break caught: changing command argument order or quoting causes the registered hook to
    // invoke a different program or pass a different status-file identity.
    #[test]
    fn command_builders_preserve_fixed_executable_and_individual_arguments() {
        let windows = HookInvocation {
            agent_id: AgentId::Codex,
            environment: AgentEnvironment::Windows,
            native_event: "SessionStart".into(),
            output_path: r"C:\Users\Alice Smith\.aiceland\codex-windows.json".into(),
        };
        assert_eq!(
            windows_hook_command(
                &windows,
                Path::new(r"C:\Program Files\AIceLand\aiceland-status-windows.ps1")
            ),
            r#"powershell.exe -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File "C:\Program Files\AIceLand\aiceland-status-windows.ps1" -Agent "codex" -Environment "windows" -NativeEvent "SessionStart" -OutputPath "C:\Users\Alice Smith\.aiceland\codex-windows.json""#,
        );

        let wsl = HookInvocation {
            agent_id: AgentId::Codex,
            environment: AgentEnvironment::Wsl,
            native_event: "session start; never-run".into(),
            output_path: "/home/alice smith/.aiceland/codex-wsl.json".into(),
        };
        assert_eq!(
            wsl_hook_command(&wsl, "/opt/AIceLand/aiceland-status-wsl.sh"),
            "'/opt/AIceLand/aiceland-status-wsl.sh' --agent 'codex' --environment 'wsl' --native-event 'session start; never-run' --output-path '/home/alice smith/.aiceland/codex-wsl.json'",
        );
    }

    // Break caught: a native hook mapping that treats an approval timeout as a completed response.
    #[test]
    fn normalizes_hermes_approval_timeout_without_untrusted_text() {
        let native: Value = serde_json::from_str(HERMES_FIXTURE).unwrap();
        assert_eq!(
            normalize_status("hermes", "post_approval_response", &native),
            "timeout"
        );
    }

    // Break caught: a stop failure that is not the literal native timeout reason being downgraded.
    #[test]
    fn normalizes_claude_stop_failure_as_failed_unless_native_timeout() {
        let native: Value = serde_json::from_str(CLAUDE_FIXTURE).unwrap();
        assert_eq!(normalize_status("claude", "StopFailure", &native), "failed");
        assert_eq!(
            normalize_status(
                "claude",
                "StopFailure",
                &serde_json::json!({ "failure_reason": "timeout" }),
            ),
            "timeout"
        );
    }

    // Break caught: heuristic matching silently misclassifies owned hooks when a new spelling
    // lacks one of its guessed substrings.
    #[test]
    fn normalizes_every_owned_event_from_the_fixed_descriptor_matrix() {
        let cases = [
            ("codex", "SessionStart", "running"),
            ("codex", "UserPromptSubmit", "running"),
            ("codex", "PermissionRequest", "waiting"),
            ("codex", "Stop", "completed"),
            ("codex", "SessionEnd", "idle"),
            ("hermes", "on_session_start", "running"),
            ("hermes", "pre_llm_call", "running"),
            ("hermes", "pre_approval_request", "waiting"),
            ("hermes", "post_approval_response", "running"),
            ("hermes", "post_llm_call", "completed"),
            ("hermes", "on_session_end", "idle"),
            ("workbuddy", "PermissionRequest", "waiting"),
            ("workbuddy", "StopFailure", "failed"),
            ("claude", "SessionEnd", "idle"),
        ];
        for (agent, event, expected) in cases {
            assert_eq!(
                normalize_status(agent, event, &serde_json::json!({})),
                expected,
                "{agent}:{event}"
            );
        }
        assert_eq!(
            normalize_status(
                "claude",
                "SessionEnd",
                &serde_json::json!({"success":false})
            ),
            "failed"
        );
    }

    // Break caught: omitting a native event ID makes unrelated task sessions share an event ID.
    #[test]
    fn generated_fallback_event_id_is_stable_and_binds_task_event_sequence_and_time() {
        let one = fallback_event_id(
            "codex",
            "windows",
            "task-a",
            "SessionStart",
            Some(3),
            Some(1000),
        );
        assert_eq!(
            one,
            fallback_event_id(
                "codex",
                "windows",
                "task-a",
                "SessionStart",
                Some(3),
                Some(1000),
            )
        );
        assert_ne!(
            one,
            fallback_event_id(
                "codex",
                "windows",
                "task-b",
                "SessionStart",
                Some(3),
                Some(1000),
            )
        );
        assert_ne!(
            one,
            fallback_event_id("codex", "windows", "task-a", "Stop", Some(3), Some(1000))
        );
        assert_eq!(
            fallback_event_id("codex", "windows", "task-a", "SessionStart", None, None),
            fallback_event_id("codex", "windows", "task-a", "SessionStart", None, None)
        );
    }

    // Break caught: package installation loses the per-integration identity and cannot safely
    // register the seven fixed hook commands.
    #[test]
    fn fixed_asset_destinations_use_the_exact_tauri_app_data_directory() {
        let mut paths = windows_asset_destinations(Path::new(r"C:\AppData\com.aiceland.app"));
        paths.extend(wsl_asset_destinations("/home/alice").unwrap());
        assert_eq!(paths.len(), 7);
        assert_eq!(
            paths
                .iter()
                .find(|entry| entry.agent_id == AgentId::Codex
                    && entry.environment == AgentEnvironment::Windows)
                .unwrap()
                .destination,
            HookAssetDestination::Windows(PathBuf::from(
                r"C:\AppData\com.aiceland.app\agent-hooks\codex-windows.ps1"
            ))
        );
        assert_eq!(
            paths
                .iter()
                .find(|entry| entry.agent_id == AgentId::Claude
                    && entry.environment == AgentEnvironment::Wsl)
                .unwrap()
                .destination,
            HookAssetDestination::Wsl(
                "/home/alice/.local/share/aiceland/agent-hooks/claude-wsl.sh".into()
            )
        );
        assert!(!paths
            .iter()
            .any(|entry| entry.agent_id == AgentId::Workbuddy
                && entry.environment == AgentEnvironment::Wsl));
    }

    #[cfg(windows)]
    #[test]
    fn wsl_probe_processes_are_created_without_a_console_window() {
        assert_eq!(wsl_background_creation_flags(), 0x0800_0000);
    }

    // Break caught: lossy UTF-8 decoding or non-object/multiple JSON accepts invalid hook input
    // and can overwrite a previously valid target.
    #[test]
    fn windows_writer_rejects_invalid_bytes_shapes_and_preserves_existing_target() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("codex-windows.json");
        fs::write(&output, b"{\"sentinel\":true}").unwrap();
        for input in [b"\xff".as_slice(), b"[]", b"{}{}"] {
            let result = run_windows_hook_raw(input, &output);
            assert!(!result.status.success());
            assert_eq!(fs::read(&output).unwrap(), b"{\"sentinel\":true}");
        }
    }

    // Break caught: accepting body-like native fields or partial writes leaks sensitive data or
    // leaves the watcher with malformed status JSON.
    #[test]
    fn windows_writer_emits_complete_allowlisted_schema_and_replay_stable_id() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("codex-windows.json");
        let first = run_windows_hook(CODEX_FIXTURE, &output);
        let second_input =
            CODEX_FIXTURE.replace("do not persist this prompt", "changed prompt body");
        let second = run_windows_hook(&second_input, &output);
        assert_eq!(first["event_id"], second["event_id"]);
        assert_eq!(first["schema_version"], 1);
        assert_eq!(first["agent"], "codex");
        assert_eq!(first["environment"], "windows");
        assert_eq!(first["task_id"], "session:session-42");
        assert_eq!(first["status"], "running");
        assert!(first["occurred_at"]
            .as_i64()
            .is_some_and(|millis| millis > 0));
        assert_eq!(
            first
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec![
                "agent",
                "environment",
                "event_id",
                "occurred_at",
                "path",
                "project",
                "schema_version",
                "sequence",
                "status",
                "task_id",
                "task_title"
            ],
        );
        assert!(!fs::read_to_string(&output).unwrap().contains("prompt"));
        assert!(!fs::read_to_string(&output).unwrap().contains("tool_body"));
        assert!(fs::read_dir(directory.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
    }

    #[test]
    fn windows_stop_writer_keeps_only_the_bounded_latest_assistant_reply() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("codex-windows.json");
        let input = serde_json::json!({
            "session_id": "session-reply",
            "event_id": "stop-reply",
            "last_assistant_message": "  Safe Agent reply  ",
            "message": "ambiguous native message",
            "prompt": "private user prompt",
            "tool_body": "private tool body"
        })
        .to_string();

        let written = run_windows_hook_for_event(&input, &output, "Stop");

        assert_eq!(
            written["message"],
            "aiceland-agent-reply-v1:Safe Agent reply"
        );
        let persisted = fs::read_to_string(output).unwrap();
        assert!(!persisted.contains("private user prompt"));
        assert!(!persisted.contains("private tool body"));
        assert!(!persisted.contains("ambiguous native message"));
    }

    #[test]
    fn windows_hermes_post_llm_writer_reads_only_the_explicit_assistant_response() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("hermes-windows.json");
        let input = serde_json::json!({
            "hook_event_name": "post_llm_call",
            "session_id": "hermes-session",
            "extra": {
                "task_id": "hermes-task",
                "turn_id": "hermes-turn-1",
                "assistant_response": "  Safe Hermes reply  ",
                "user_message": "private user prompt",
                "conversation_history": ["private history"],
                "tool_output": "private tool output"
            }
        })
        .to_string();

        let written = run_windows_hook_for_agent_event(&input, &output, "hermes", "post_llm_call");

        assert_eq!(written["agent"], "hermes");
        assert_eq!(written["task_id"], "hermes-task");
        assert_eq!(
            written["message"],
            "aiceland-agent-reply-v1:Safe Hermes reply"
        );
        let persisted = fs::read_to_string(output).unwrap();
        assert!(!persisted.contains("private user prompt"));
        assert!(!persisted.contains("private history"));
        assert!(!persisted.contains("private tool output"));

        let second_output = directory.path().join("hermes-windows-second.json");
        let second_input = input.replace("hermes-turn-1", "hermes-turn-2");
        let second = run_windows_hook_for_agent_event(
            &second_input,
            &second_output,
            "hermes",
            "post_llm_call",
        );
        assert_ne!(written["event_id"], second["event_id"]);
    }

    // Break caught: a missing native ID and timestamp must not put a freshly generated wall-clock
    // value into the identity material; the emitted timestamp may be fresh, but the event ID is not.
    #[test]
    fn windows_writer_missing_id_and_timestamp_replays_a_stable_task_bound_event_id() {
        let directory = tempfile::tempdir().unwrap();
        let first_output = directory.path().join("first.json");
        let second_output = directory.path().join("second.json");
        let other_task_output = directory.path().join("other-task.json");
        let task_a = r#"{"session_id":"task-a"}"#;
        let task_b = r#"{"session_id":"task-b"}"#;

        let first = run_windows_hook(task_a, &first_output);
        std::thread::sleep(std::time::Duration::from_millis(5));
        let second = run_windows_hook(task_a, &second_output);
        let other_task = run_windows_hook(task_b, &other_task_output);

        assert_eq!(first["event_id"], second["event_id"]);
        assert_ne!(first["event_id"], other_task["event_id"]);
        assert!(first["occurred_at"].as_i64().is_some());
        assert!(second["occurred_at"].as_i64().is_some());
    }

    // Break caught: a clean APPDATA root needs no pre-created parent directories; all four
    // Windows package assets must land under the fixed product namespace with verified bytes.
    #[test]
    fn install_one_creates_a_fresh_appdata_parent_and_verifies_windows_asset_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("aiceland-status-windows.ps1");
        let destination = directory
            .path()
            .join("fresh-appdata")
            .join("com.aiceland")
            .join("agent-hooks")
            .join("codex-windows.ps1");
        let package_owned = b"package-owned-windows-hook";
        fs::write(&source, package_owned).unwrap();

        install_one(&source, &destination).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), package_owned);
        assert_eq!(
            sha256_hex(&fs::read(&destination).unwrap()),
            sha256_hex(package_owned)
        );
    }

    // Break caught: a missing WSL runtime must leave the four independently installable Windows
    // hooks present and byte-verified instead of failing the whole package installation.
    #[test]
    fn unavailable_wsl_still_installs_and_verifies_all_four_windows_assets() {
        let directory = tempfile::tempdir().unwrap();
        let windows_source = directory.path().join("aiceland-status-windows.ps1");
        let wsl_source = directory.path().join("aiceland-status-wsl.sh");
        fs::write(&windows_source, b"windows package script").unwrap();
        fs::write(&wsl_source, b"wsl package script").unwrap();

        let installed = install_hook_assets_with(
            &directory.path().join("fresh-appdata"),
            &windows_source,
            &wsl_source,
            &UnavailableWsl,
        )
        .unwrap();

        assert!(!installed.wsl_available);
        assert_eq!(installed.wsl_status_dir, None);
        assert_eq!(installed.paths.len(), 4);
        for entry in installed.paths {
            let HookAssetDestination::Windows(path) = entry.destination else {
                panic!("WSL path was fabricated while WSL is unavailable");
            };
            assert!(path.ends_with(format!("{}-windows.ps1", agent_name(&entry.agent_id))));
            assert_eq!(fs::read(&path).unwrap(), b"windows package script");
            assert_eq!(
                sha256_hex(&fs::read(&path).unwrap()),
                sha256_hex(b"windows package script")
            );
        }
    }

    // Break caught: WSL paths passed to the fixed helper must stay Unix strings; PathBuf joins on
    // a Windows host silently convert the intended Linux destination into backslash-separated text.
    #[test]
    fn wsl_installer_passes_unix_destination_and_preserves_chmod_and_sha_contract() {
        let directory = tempfile::tempdir().unwrap();
        let windows_source = directory.path().join("aiceland-status-windows.ps1");
        let wsl_source = directory.path().join("aiceland-status-wsl.sh");
        fs::write(&windows_source, b"windows package script").unwrap();
        fs::write(&wsl_source, b"wsl package script").unwrap();
        let wsl = CapturingWsl::new("/home/alice");

        let installed = install_hook_assets_with(
            &directory.path().join("APPDATA/com.aiceland.app"),
            &windows_source,
            &wsl_source,
            &wsl,
        )
        .unwrap();

        assert!(installed.wsl_available);
        assert_eq!(
            installed.wsl_status_dir.as_deref(),
            Some("/mnt/c/Users/Alice/AppData/Roaming/com.aiceland.app/agent-status")
        );
        assert_eq!(wsl.installs.borrow().len(), 3);
        for (source, destination, expected_sha) in wsl.installs.borrow().iter() {
            assert_eq!(source, "/mnt/c/package/aiceland-status-wsl.sh");
            assert!(destination.starts_with("/home/alice/.local/share/aiceland/agent-hooks/"));
            assert!(!destination.contains('\\'));
            assert_eq!(expected_sha, &sha256_hex(b"wsl package script"));
        }
    }

    struct UnavailableWsl;

    impl WslHookAssetPort for UnavailableWsl {
        fn home(&self) -> Result<String, CommandError> {
            Err(hook_asset_error("wslUnavailable"))
        }

        fn unix_path(&self, _: &Path) -> Result<String, CommandError> {
            panic!("WSL source conversion called after unavailable home")
        }

        fn install(&self, _: &str, _: &str, _: &str) -> Result<(), CommandError> {
            panic!("WSL install called after unavailable home")
        }
    }

    struct CapturingWsl {
        home: String,
        installs: std::cell::RefCell<Vec<(String, String, String)>>,
    }

    impl CapturingWsl {
        fn new(home: &str) -> Self {
            Self {
                home: home.into(),
                installs: std::cell::RefCell::new(Vec::new()),
            }
        }
    }

    impl WslHookAssetPort for CapturingWsl {
        fn home(&self) -> Result<String, CommandError> {
            Ok(self.home.clone())
        }

        fn unix_path(&self, path: &Path) -> Result<String, CommandError> {
            if path.ends_with("agent-status") {
                Ok("/mnt/c/Users/Alice/AppData/Roaming/com.aiceland.app/agent-status".into())
            } else {
                Ok("/mnt/c/package/aiceland-status-wsl.sh".into())
            }
        }

        fn install(
            &self,
            source: &str,
            destination: &str,
            expected_sha: &str,
        ) -> Result<(), CommandError> {
            self.installs.borrow_mut().push((
                source.into(),
                destination.into(),
                expected_sha.into(),
            ));
            Ok(())
        }
    }

    fn run_windows_hook(input: &str, output: &Path) -> Value {
        run_windows_hook_for_event(input, output, "SessionStart")
    }

    fn run_windows_hook_for_event(input: &str, output: &Path, native_event: &str) -> Value {
        run_windows_hook_for_agent_event(input, output, "codex", native_event)
    }

    fn run_windows_hook_for_agent_event(
        input: &str,
        output: &Path,
        agent: &str,
        native_event: &str,
    ) -> Value {
        let result =
            run_windows_hook_raw_for_agent_event(input.as_bytes(), output, agent, native_event);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice(&fs::read(output).unwrap()).unwrap()
    }

    fn run_windows_hook_raw(input: &[u8], output: &Path) -> std::process::Output {
        run_windows_hook_raw_for_event(input, output, "SessionStart")
    }

    fn run_windows_hook_raw_for_event(
        input: &[u8],
        output: &Path,
        native_event: &str,
    ) -> std::process::Output {
        run_windows_hook_raw_for_agent_event(input, output, "codex", native_event)
    }

    fn run_windows_hook_raw_for_agent_event(
        input: &[u8],
        output: &Path,
        agent: &str,
        native_event: &str,
    ) -> std::process::Output {
        let script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("agent-hooks")
            .join("aiceland-status-windows.ps1");
        let mut child = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(script)
            .args([
                "-Agent",
                agent,
                "-Environment",
                "windows",
                "-NativeEvent",
                native_event,
                "-OutputPath",
            ])
            .arg(output)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        child.wait_with_output().unwrap()
    }
}
