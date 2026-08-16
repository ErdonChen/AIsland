use crate::contracts::{AppErrorCode, CommandError, SafeMessageParameters};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub struct ClipboardAssetStore {
    root: PathBuf,
}

impl ClipboardAssetStore {
    pub fn new(app_data_dir: &Path) -> Result<Self, CommandError> {
        fs::create_dir_all(app_data_dir).map_err(|_| io_failure())?;
        let app_data_root = app_data_dir.canonicalize().map_err(|_| io_failure())?;
        let expected_root = app_data_root.join("clipboard-assets");
        let requested_root = app_data_dir.join("clipboard-assets");
        fs::create_dir_all(&requested_root).map_err(|_| io_failure())?;
        let root = requested_root.canonicalize().map_err(|_| io_failure())?;
        if root != expected_root {
            return Err(invalid_input());
        }
        Ok(Self { root })
    }

    pub fn write_png_atomic(&self, asset_id: Uuid, bytes: &[u8]) -> Result<String, CommandError> {
        if bytes.is_empty() {
            return Err(invalid_input());
        }
        let asset_name = format!("{asset_id}.png");
        let temporary = self.root.join(format!("{asset_id}.tmp"));
        let target = self.root.join(&asset_name);
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|_| io_failure())?;
            file.write_all(bytes).map_err(|_| io_failure())?;
            file.flush().map_err(|_| io_failure())?;
            file.sync_all().map_err(|_| io_failure())?;
            drop(file);
            fs::rename(&temporary, &target).map_err(|_| io_failure())?;
            Ok(asset_name)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    pub fn read_owned(&self, asset_name: &str) -> Result<Vec<u8>, CommandError> {
        fs::read(self.existing_owned_path(asset_name)?).map_err(|_| io_failure())
    }

    pub fn read_owned_bounded(
        &self,
        asset_name: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, CommandError> {
        if max_bytes == 0 {
            return Err(invalid_input());
        }
        let path = self.existing_owned_path(asset_name)?;
        let file = OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|_| io_failure())?;
        let max_read = u64::try_from(max_bytes)
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or_else(io_failure)?;
        let mut bytes = Vec::new();
        file.take(max_read)
            .read_to_end(&mut bytes)
            .map_err(|_| io_failure())?;
        if bytes.is_empty() || bytes.len() > max_bytes {
            return Err(io_failure());
        }
        Ok(bytes)
    }

    pub fn delete_owned(&self, asset_name: &str) -> Result<(), CommandError> {
        validate_asset_name(asset_name)?;
        let target = self.root.join(asset_name);
        match fs::symlink_metadata(&target) {
            Ok(_) => {
                let canonical = target.canonicalize().map_err(|_| io_failure())?;
                if !canonical.starts_with(&self.root) || !canonical.is_file() {
                    return Err(invalid_input());
                }
                fs::remove_file(canonical).map_err(|_| io_failure())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(io_failure()),
        }
    }

    pub fn remove_orphans(&self, referenced_names: &BTreeSet<String>) -> Result<u64, CommandError> {
        let mut removed = 0_u64;
        for entry in fs::read_dir(&self.root).map_err(|_| io_failure())? {
            let entry = entry.map_err(|_| io_failure())?;
            if !entry.file_type().map_err(|_| io_failure())?.is_file() {
                continue;
            }
            let Some(asset_name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if validate_asset_name(&asset_name).is_err() || referenced_names.contains(&asset_name) {
                continue;
            }
            self.delete_owned(&asset_name)?;
            removed = removed.checked_add(1).ok_or_else(io_failure)?;
        }
        Ok(removed)
    }

    fn existing_owned_path(&self, asset_name: &str) -> Result<PathBuf, CommandError> {
        validate_asset_name(asset_name)?;
        let canonical = self
            .root
            .join(asset_name)
            .canonicalize()
            .map_err(|_| io_failure())?;
        if !canonical.starts_with(&self.root) || !canonical.is_file() {
            return Err(invalid_input());
        }
        Ok(canonical)
    }
}

fn validate_asset_name(asset_name: &str) -> Result<Uuid, CommandError> {
    let path = Path::new(asset_name);
    if path.components().count() != 1
        || path.extension().and_then(|value| value.to_str()) != Some("png")
    {
        return Err(invalid_input());
    }
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(invalid_input)?;
    let id = Uuid::parse_str(stem).map_err(|_| invalid_input())?;
    if asset_name != format!("{id}.png") {
        return Err(invalid_input());
    }
    Ok(id)
}

fn invalid_input() -> CommandError {
    CommandError {
        code: AppErrorCode::InvalidInput,
        message_key: "errors.invalidInput".into(),
        details: SafeMessageParameters::new(),
        retryable: false,
    }
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
    use super::ClipboardAssetStore;
    use crate::contracts::AppErrorCode;
    use std::collections::BTreeSet;
    use std::fs;
    use std::process::Command;
    use uuid::Uuid;

    #[test]
    fn writes_reads_and_deletes_only_lowercase_uuid_png_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = ClipboardAssetStore::new(dir.path()).unwrap();
        let id = Uuid::new_v4();
        let name = store.write_png_atomic(id, b"png-bytes").unwrap();
        assert_eq!(name, format!("{id}.png"));
        assert_eq!(store.read_owned(&name).unwrap(), b"png-bytes");
        assert_eq!(
            store.read_owned_bounded(&name, 8).unwrap_err().code,
            AppErrorCode::IoFailure
        );
        assert_eq!(store.read_owned_bounded(&name, 9).unwrap(), b"png-bytes");
        store.delete_owned(&name).unwrap();
        assert_eq!(
            store.read_owned(&name).unwrap_err().code,
            AppErrorCode::IoFailure
        );
    }

    #[test]
    fn traversal_is_rejected_without_touching_the_named_user_file() {
        let dir = tempfile::tempdir().unwrap();
        let user_file = dir.path().join("user.png");
        fs::write(&user_file, b"user-owned").unwrap();
        let store = ClipboardAssetStore::new(&dir.path().join("app-data")).unwrap();
        let error = store.delete_owned("..\\user.png").unwrap_err();
        assert_eq!(error.code, AppErrorCode::InvalidInput);
        let uppercase_name = format!("{}.png", Uuid::new_v4().to_string().to_uppercase());
        assert_eq!(
            store.delete_owned(&uppercase_name).unwrap_err().code,
            AppErrorCode::InvalidInput
        );
        assert_eq!(fs::read(user_file).unwrap(), b"user-owned");
    }

    #[test]
    fn orphan_cleanup_removes_only_unreferenced_owned_uuid_png_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = ClipboardAssetStore::new(dir.path()).unwrap();
        let kept_id = Uuid::new_v4();
        let removed_id = Uuid::new_v4();
        let kept = store.write_png_atomic(kept_id, b"kept").unwrap();
        let removed = store.write_png_atomic(removed_id, b"removed").unwrap();
        let root_note = dir.path().join("clipboard-assets").join("note.txt");
        fs::write(&root_note, b"not an owned png").unwrap();

        assert_eq!(
            store
                .remove_orphans(&BTreeSet::from([kept.clone()]))
                .unwrap(),
            1
        );
        assert_eq!(store.read_owned(&kept).unwrap(), b"kept");
        assert_eq!(
            store.read_owned(&removed).unwrap_err().code,
            AppErrorCode::IoFailure
        );
        assert_eq!(fs::read(root_note).unwrap(), b"not an owned png");
    }

    #[cfg(windows)]
    #[test]
    fn preexisting_asset_root_junction_outside_app_data_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let app_data = dir.path().join("app-data");
        let outside = dir.path().join("outside");
        fs::create_dir_all(&app_data).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let junction = app_data.join("clipboard-assets");
        let status = Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .status()
            .unwrap();
        assert!(status.success());

        let error = match ClipboardAssetStore::new(&app_data) {
            Ok(_) => panic!("an asset-root junction outside app data was accepted"),
            Err(error) => error,
        };
        assert_eq!(error.code, AppErrorCode::InvalidInput);
    }
}
