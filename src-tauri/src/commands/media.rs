use crate::contracts::{
    AppErrorCode, CommandError, MediaCommand, MediaControlInput, MediaSnapshot, SafeParameterValue,
};
use crate::services::{media_service::MediaService, AppServices};
use std::sync::Arc;

#[tauri::command(rename = "getMediaSnapshot", rename_all = "camelCase")]
pub fn get_media_snapshot(
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<MediaSnapshot, CommandError> {
    get_media_snapshot_with(&services.media, now_millis())
}

#[tauri::command(rename = "sendMediaCommand", rename_all = "camelCase")]
pub fn send_media_command(
    command: MediaCommand,
    position_seconds: Option<f64>,
    volume_percent: Option<f64>,
    services: tauri::State<'_, Arc<AppServices>>,
) -> Result<MediaSnapshot, CommandError> {
    send_media_command_with(
        &services.media,
        command,
        position_seconds,
        volume_percent,
        now_millis(),
    )
}

pub(crate) fn get_media_snapshot_with(
    service: &MediaService,
    now: i64,
) -> Result<MediaSnapshot, CommandError> {
    service.snapshot(now)
}

pub(crate) fn send_media_command_with(
    service: &MediaService,
    command: MediaCommand,
    position_seconds: Option<f64>,
    volume_percent: Option<f64>,
    now: i64,
) -> Result<MediaSnapshot, CommandError> {
    service.control(media_input(command, position_seconds, volume_percent)?, now)
}

pub(crate) fn media_input(
    command: MediaCommand,
    position_seconds: Option<f64>,
    volume_percent: Option<f64>,
) -> Result<MediaControlInput, CommandError> {
    match (command, position_seconds, volume_percent) {
        (MediaCommand::Play, None, None) => Ok(MediaControlInput::Play),
        (MediaCommand::Pause, None, None) => Ok(MediaControlInput::Pause),
        (MediaCommand::Previous, None, None) => Ok(MediaControlInput::Previous),
        (MediaCommand::Next, None, None) => Ok(MediaControlInput::Next),
        (MediaCommand::Seek, Some(position_seconds), None)
            if position_seconds.is_finite() && position_seconds >= 0.0 =>
        {
            Ok(MediaControlInput::Seek { position_seconds })
        }
        (MediaCommand::SetVolume, None, Some(volume_percent))
            if volume_percent.is_finite() && (0.0..=100.0).contains(&volume_percent) =>
        {
            Ok(MediaControlInput::SetVolume { volume_percent })
        }
        _ => Err(invalid_input("invalidMediaCommandShape")),
    }
}

fn invalid_input(reason: &str) -> CommandError {
    CommandError::with_detail(
        AppErrorCode::InvalidInput,
        "errors.invalidInput",
        "reasonCode",
        SafeParameterValue::String(reason.into()),
        false,
    )
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_all_six_exact_media_union_shapes() {
        assert_eq!(
            media_input(MediaCommand::Play, None, None).unwrap(),
            MediaControlInput::Play
        );
        assert_eq!(
            media_input(MediaCommand::Pause, None, None).unwrap(),
            MediaControlInput::Pause
        );
        assert_eq!(
            media_input(MediaCommand::Previous, None, None).unwrap(),
            MediaControlInput::Previous
        );
        assert_eq!(
            media_input(MediaCommand::Next, None, None).unwrap(),
            MediaControlInput::Next
        );
        assert_eq!(
            media_input(MediaCommand::Seek, Some(42.5), None).unwrap(),
            MediaControlInput::Seek {
                position_seconds: 42.5
            }
        );
        assert_eq!(
            media_input(MediaCommand::SetVolume, None, Some(35.0)).unwrap(),
            MediaControlInput::SetVolume {
                volume_percent: 35.0
            }
        );
    }

    #[test]
    fn rejects_missing_extra_nonfinite_and_out_of_range_branch_data() {
        for result in [
            media_input(MediaCommand::Play, Some(1.0), None),
            media_input(MediaCommand::Seek, None, None),
            media_input(MediaCommand::Seek, Some(f64::NAN), None),
            media_input(MediaCommand::Seek, Some(1.0), Some(2.0)),
            media_input(MediaCommand::SetVolume, None, None),
            media_input(MediaCommand::SetVolume, None, Some(-1.0)),
            media_input(MediaCommand::SetVolume, None, Some(101.0)),
        ] {
            assert_eq!(result.unwrap_err().code, AppErrorCode::InvalidInput);
        }
    }
}
