use crate::contracts::{AppErrorCode, CommandError, SafeMessageParameters};
use std::fs;
use std::path::PathBuf;

pub trait MarkdownExportDirectoryProvider: Send + Sync {
    fn default_directory(&self) -> Result<PathBuf, CommandError>;
}

pub struct BootstrapMarkdownExportDirectoryProvider;

impl MarkdownExportDirectoryProvider for BootstrapMarkdownExportDirectoryProvider {
    fn default_directory(&self) -> Result<PathBuf, CommandError> {
        default_directory_from_profile(std::env::var_os("USERPROFILE").map(PathBuf::from))
    }
}

fn default_directory_from_profile(profile: Option<PathBuf>) -> Result<PathBuf, CommandError> {
    let profile = profile
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| CommandError {
            code: AppErrorCode::SourceUnavailable,
            message_key: "errors.sourceUnavailable".into(),
            details: SafeMessageParameters::new(),
            retryable: false,
        })?;
    let directory = profile.join("Documents").join("AIsland");
    fs::create_dir_all(&directory).map_err(|_| io_failure())?;
    directory.canonicalize().map_err(|_| io_failure())
}

fn io_failure() -> CommandError {
    CommandError {
        code: AppErrorCode::IoFailure,
        message_key: "errors.ioFailure".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::default_directory_from_profile;
    use crate::contracts::AppErrorCode;

    #[test]
    fn export_default_directory_creates_and_canonicalizes_the_bootstrap_folder() {
        let profile = tempfile::tempdir().unwrap();
        let expected = profile.path().join("Documents").join("AIsland");

        let actual = default_directory_from_profile(Some(profile.path().to_path_buf())).unwrap();

        assert!(expected.is_dir());
        assert_eq!(actual, expected.canonicalize().unwrap());
    }

    #[test]
    fn export_default_directory_requires_a_profile_without_exposing_environment_data() {
        let error = default_directory_from_profile(None).unwrap_err();

        assert_eq!(error.code, AppErrorCode::SourceUnavailable);
        assert_eq!(error.message_key, "errors.sourceUnavailable");
        assert!(error.details.is_empty());
    }
}
