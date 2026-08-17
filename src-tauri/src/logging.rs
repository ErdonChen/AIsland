use std::sync::OnceLock;

use tauri::{plugin::TauriPlugin, Runtime};
use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy};

const MAX_LOG_FILE_BYTES: u128 = 5 * 1024 * 1024;
const RETAINED_LOG_FILES: usize = 5;

pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    // The file target flushes each record before returning, so failures are
    // persisted without a separate timer thread or an in-memory batch.
    tauri_plugin_log::Builder::new()
        .clear_targets()
        .targets([
            Target::new(TargetKind::Stdout),
            Target::new(TargetKind::LogDir {
                file_name: Some("aisland".to_string()),
            }),
        ])
        .level(log::LevelFilter::Warn)
        .level_for("aisland", log::LevelFilter::Info)
        .max_file_size(MAX_LOG_FILE_BYTES)
        .rotation_strategy(RotationStrategy::KeepSome(RETAINED_LOG_FILES))
        .timezone_strategy(TimezoneStrategy::UseLocal)
        .build()
}

pub fn install_panic_hook() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    if INSTALLED.set(()).is_err() {
        return;
    }

    // Discard the default hook because it formats the panic payload and can
    // write prompts, tokens, or tool data to stderr. The replacement below
    // records only a fixed marker and source location; it intentionally does
    // not emit a backtrace.
    drop(std::panic::take_hook());
    std::panic::set_hook(Box::new(move |panic_info| {
        let location = panic_info
            .location()
            .map(|location| (location.file(), location.line(), location.column()));
        log::error!(target: "aisland::panic", "{}", format_panic_marker(location));
        flush();
    }));
}

fn format_panic_marker(location: Option<(&str, u32, u32)>) -> String {
    match location {
        Some((file, line, column)) => {
            format!("panic=unhandled location={file}:{line}:{column}")
        }
        None => "panic=unhandled location=unknown".into(),
    }
}

pub fn flush() {
    log::logger().flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    const PANIC_CHILD_SECRET_ENV: &str = "AISLAND_PANIC_HOOK_TEST_SECRET";

    #[test]
    fn retention_budget_is_bounded() {
        assert_eq!(MAX_LOG_FILE_BYTES, 5 * 1024 * 1024);
        assert_eq!(RETAINED_LOG_FILES, 5);
    }

    #[test]
    fn panic_marker_never_formats_the_payload() {
        let secret_payload = "api-token-should-never-enter-the-log";
        let record = format_panic_marker(Some(("safe.rs", 42, 7)));

        assert_eq!(record, "panic=unhandled location=safe.rs:42:7");
        assert!(!record.contains(secret_payload));
    }

    #[test]
    fn panic_hook_child_fixture() {
        let Ok(secret) = std::env::var(PANIC_CHILD_SECRET_ENV) else {
            return;
        };
        install_panic_hook();
        panic!("{secret}");
    }

    #[test]
    fn panic_hook_never_forwards_payload_to_process_streams() {
        let secret = "panic-payload-must-not-reach-stdout-or-stderr";
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "logging::tests::panic_hook_child_fixture",
                "--nocapture",
            ])
            .env(PANIC_CHILD_SECRET_ENV, secret)
            .output()
            .unwrap();

        assert!(!output.status.success());
        let streams = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!streams.contains(secret), "panic payload leaked: {streams}");
    }
}
