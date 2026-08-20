use crate::contracts::{AppErrorCode, CommandError, SafeMessageParameters};
use crate::domain::notes::validate_local_date;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

const MAX_RECORDING_BYTES: u64 = 100 * 1024 * 1024;
const MAX_CHUNK_BYTES: usize = 1024 * 1024;

pub struct StagedRecordingAsset {
    original: PathBuf,
    staged: PathBuf,
}

pub struct NoteRecordingAssetStore {
    root: PathBuf,
}

impl NoteRecordingAssetStore {
    pub fn new(app_data_dir: &Path) -> Result<Self, CommandError> {
        fs::create_dir_all(app_data_dir).map_err(|_| io_failure())?;
        let app_data_root = app_data_dir.canonicalize().map_err(|_| io_failure())?;
        let requested_root = app_data_dir.join("note-recordings");
        fs::create_dir_all(&requested_root).map_err(|_| io_failure())?;
        let root = requested_root.canonicalize().map_err(|_| io_failure())?;
        if root != app_data_root.join("note-recordings") {
            return Err(invalid_input());
        }
        Ok(Self { root })
    }

    pub fn create_temporary(
        &self,
        note_date: &str,
        id: Uuid,
        extension: &str,
    ) -> Result<(), CommandError> {
        let directory = self.owned_date_directory(note_date)?;
        let temporary = directory.join(temporary_name(id, extension)?);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temporary)
            .and_then(|file| file.sync_all())
            .map_err(|_| io_failure())
    }

    pub fn append_temporary(
        &self,
        note_date: &str,
        id: Uuid,
        extension: &str,
        bytes: &[u8],
    ) -> Result<(), CommandError> {
        if bytes.is_empty() || bytes.len() > MAX_CHUNK_BYTES {
            return Err(invalid_input());
        }
        let path = self.existing_owned_temporary(note_date, id, extension)?;
        let current = fs::metadata(&path).map_err(|_| io_failure())?.len();
        let added = u64::try_from(bytes.len()).map_err(|_| invalid_input())?;
        if current
            .checked_add(added)
            .filter(|total| *total <= MAX_RECORDING_BYTES)
            .is_none()
        {
            return Err(invalid_input());
        }
        let mut file = OpenOptions::new()
            .append(true)
            .open(path)
            .map_err(|_| io_failure())?;
        file.write_all(bytes).map_err(|_| io_failure())?;
        file.flush().map_err(|_| io_failure())
    }

    pub fn finalize(
        &self,
        note_date: &str,
        id: Uuid,
        extension: &str,
    ) -> Result<(String, i64), CommandError> {
        let temporary = self.existing_owned_temporary(note_date, id, extension)?;
        let directory = self.owned_date_directory(note_date)?;
        let asset_name = completed_name(id, extension)?;
        let target = directory.join(&asset_name);
        let bytes = fs::metadata(&temporary).map_err(|_| io_failure())?.len();
        if bytes == 0 || bytes > MAX_RECORDING_BYTES {
            return Err(invalid_input());
        }
        fs::rename(&temporary, target).map_err(|_| io_failure())?;
        Ok((asset_name, i64::try_from(bytes).map_err(|_| io_failure())?))
    }

    pub fn rollback_finalize(
        &self,
        note_date: &str,
        id: Uuid,
        extension: &str,
    ) -> Result<(), CommandError> {
        let directory = self.owned_date_directory(note_date)?;
        let target = directory.join(completed_name(id, extension)?);
        let temporary = directory.join(temporary_name(id, extension)?);
        if target.exists() {
            fs::rename(target, temporary).map_err(|_| io_failure())?;
        }
        Ok(())
    }

    pub fn discard_temporary(
        &self,
        note_date: &str,
        id: Uuid,
        extension: &str,
    ) -> Result<(), CommandError> {
        let directory = self.owned_date_directory(note_date)?;
        let target = directory.join(temporary_name(id, extension)?);
        match fs::remove_file(target) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(io_failure()),
        }
    }

    pub fn read_completed(
        &self,
        note_date: &str,
        asset_name: &str,
    ) -> Result<Vec<u8>, CommandError> {
        validate_local_date(note_date)?;
        validate_completed_asset_name(asset_name)?;
        let directory = self.owned_date_directory(note_date)?;
        let path = directory
            .join(asset_name)
            .canonicalize()
            .map_err(|_| io_failure())?;
        if !path.starts_with(&directory) || !path.is_file() {
            return Err(invalid_input());
        }
        let file = OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|_| io_failure())?;
        let mut bytes = Vec::new();
        file.take(MAX_RECORDING_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| io_failure())?;
        if bytes.is_empty() || bytes.len() as u64 > MAX_RECORDING_BYTES {
            return Err(io_failure());
        }
        Ok(bytes)
    }

    pub fn stage_temporary_deletion(
        &self,
        note_date: &str,
        id: Uuid,
        extension: &str,
    ) -> Result<StagedRecordingAsset, CommandError> {
        let original = self.existing_owned_temporary(note_date, id, extension)?;
        let staged = self
            .owned_date_directory(note_date)?
            .join(format!("{id}.{extension}.abort"));
        fs::rename(&original, &staged).map_err(|_| io_failure())?;
        Ok(StagedRecordingAsset { original, staged })
    }

    pub fn stage_completed_deletion(
        &self,
        note_date: &str,
        id: Uuid,
        extension: &str,
    ) -> Result<StagedRecordingAsset, CommandError> {
        let directory = self.owned_date_directory(note_date)?;
        let original = directory
            .join(completed_name(id, extension)?)
            .canonicalize()
            .map_err(|_| io_failure())?;
        if !original.starts_with(&directory) || !original.is_file() {
            return Err(invalid_input());
        }
        let staged = directory.join(format!("{id}.{extension}.delete"));
        fs::rename(&original, &staged).map_err(|_| io_failure())?;
        Ok(StagedRecordingAsset { original, staged })
    }

    pub fn commit_staged_deletion(&self, staged: StagedRecordingAsset) -> Result<(), CommandError> {
        if !staged.staged.starts_with(&self.root) || !staged.staged.is_file() {
            return Err(invalid_input());
        }
        fs::remove_file(staged.staged).map_err(|_| io_failure())
    }

    pub fn rollback_staged_deletion(
        &self,
        staged: StagedRecordingAsset,
    ) -> Result<(), CommandError> {
        if !staged.staged.starts_with(&self.root) || !staged.staged.is_file() {
            return Err(invalid_input());
        }
        fs::rename(staged.staged, staged.original).map_err(|_| io_failure())
    }

    fn owned_date_directory(&self, note_date: &str) -> Result<PathBuf, CommandError> {
        validate_local_date(note_date)?;
        let requested = self.root.join(note_date);
        fs::create_dir_all(&requested).map_err(|_| io_failure())?;
        let canonical = requested.canonicalize().map_err(|_| io_failure())?;
        if !canonical.starts_with(&self.root) || canonical.parent() != Some(self.root.as_path()) {
            return Err(invalid_input());
        }
        Ok(canonical)
    }

    fn existing_owned_temporary(
        &self,
        note_date: &str,
        id: Uuid,
        extension: &str,
    ) -> Result<PathBuf, CommandError> {
        let directory = self.owned_date_directory(note_date)?;
        let path = directory
            .join(temporary_name(id, extension)?)
            .canonicalize()
            .map_err(|_| io_failure())?;
        if !path.starts_with(&directory) || !path.is_file() {
            return Err(invalid_input());
        }
        Ok(path)
    }
}

fn validate_extension(extension: &str) -> Result<(), CommandError> {
    if matches!(extension, "webm" | "ogg" | "mp4") {
        Ok(())
    } else {
        Err(invalid_input())
    }
}

fn temporary_name(id: Uuid, extension: &str) -> Result<String, CommandError> {
    validate_extension(extension)?;
    Ok(format!("{id}.{extension}.part"))
}

fn completed_name(id: Uuid, extension: &str) -> Result<String, CommandError> {
    validate_extension(extension)?;
    Ok(format!("{id}.{extension}"))
}

fn validate_completed_asset_name(asset_name: &str) -> Result<(), CommandError> {
    let path = Path::new(asset_name);
    if path.components().count() != 1 {
        return Err(invalid_input());
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .ok_or_else(invalid_input)?;
    validate_extension(extension)?;
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(invalid_input)?;
    let id = Uuid::parse_str(stem).map_err(|_| invalid_input())?;
    if asset_name != format!("{id}.{extension}") {
        return Err(invalid_input());
    }
    Ok(())
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
    use super::*;

    fn store() -> (tempfile::TempDir, NoteRecordingAssetStore) {
        let directory = tempfile::tempdir().unwrap();
        let store = NoteRecordingAssetStore::new(directory.path()).unwrap();
        (directory, store)
    }

    #[test]
    fn enforces_chunk_and_total_size_boundaries_without_mutating_on_rejection() {
        let (_directory, store) = store();
        let id = Uuid::new_v4();
        store.create_temporary("2026-08-08", id, "webm").unwrap();
        assert_eq!(
            store
                .append_temporary("2026-08-08", id, "webm", &[])
                .unwrap_err()
                .code,
            AppErrorCode::InvalidInput
        );
        assert_eq!(
            store
                .append_temporary("2026-08-08", id, "webm", &vec![0; MAX_CHUNK_BYTES + 1])
                .unwrap_err()
                .code,
            AppErrorCode::InvalidInput
        );

        let path = store
            .root
            .join("2026-08-08")
            .join(format!("{id}.webm.part"));
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_RECORDING_BYTES - 1)
            .unwrap();
        store
            .append_temporary("2026-08-08", id, "webm", &[1])
            .unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), MAX_RECORDING_BYTES);
        assert_eq!(
            store
                .append_temporary("2026-08-08", id, "webm", &[2])
                .unwrap_err()
                .code,
            AppErrorCode::InvalidInput
        );
        assert_eq!(fs::metadata(path).unwrap().len(), MAX_RECORDING_BYTES);
    }

    #[test]
    fn finalize_rollback_and_staged_delete_preserve_exact_asset_bytes() {
        let (_directory, store) = store();
        let id = Uuid::new_v4();
        let bytes = [1_u8, 2, 3, 4];
        store.create_temporary("2026-08-08", id, "webm").unwrap();
        store
            .append_temporary("2026-08-08", id, "webm", &bytes)
            .unwrap();

        let (asset_name, byte_size) = store.finalize("2026-08-08", id, "webm").unwrap();
        assert_eq!(byte_size, bytes.len() as i64);
        assert_eq!(
            store.read_completed("2026-08-08", &asset_name).unwrap(),
            bytes
        );

        store.rollback_finalize("2026-08-08", id, "webm").unwrap();
        assert_eq!(
            store
                .read_completed("2026-08-08", &asset_name)
                .unwrap_err()
                .code,
            AppErrorCode::IoFailure
        );
        store.finalize("2026-08-08", id, "webm").unwrap();

        let staged = store
            .stage_completed_deletion("2026-08-08", id, "webm")
            .unwrap();
        store.rollback_staged_deletion(staged).unwrap();
        assert_eq!(
            store.read_completed("2026-08-08", &asset_name).unwrap(),
            bytes
        );
        let staged = store
            .stage_completed_deletion("2026-08-08", id, "webm")
            .unwrap();
        store.commit_staged_deletion(staged).unwrap();
        assert_eq!(
            store
                .read_completed("2026-08-08", &asset_name)
                .unwrap_err()
                .code,
            AppErrorCode::IoFailure
        );
    }

    #[test]
    fn failed_finalize_keeps_the_temporary_asset_available_for_compensation() {
        let (_directory, store) = store();
        let id = Uuid::new_v4();
        store.create_temporary("2026-08-08", id, "webm").unwrap();
        store
            .append_temporary("2026-08-08", id, "webm", &[1])
            .unwrap();
        fs::create_dir(store.root.join("2026-08-08").join(format!("{id}.webm"))).unwrap();

        assert_eq!(
            store.finalize("2026-08-08", id, "webm").unwrap_err().code,
            AppErrorCode::IoFailure
        );
        assert!(store
            .root
            .join("2026-08-08")
            .join(format!("{id}.webm.part"))
            .is_file());
    }
}
