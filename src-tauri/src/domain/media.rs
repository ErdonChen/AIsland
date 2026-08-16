use crate::contracts::{MediaPlaybackState, MediaSnapshot};

const WINDOWS_TICKS_PER_SECOND: i64 = 10_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativePlaybackState {
    Playing,
    Paused,
    Closed,
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeMediaSnapshot {
    pub session_id: Option<String>,
    pub title: String,
    pub artist: String,
    pub playback_state: NativePlaybackState,
    pub position_ticks: i64,
    pub duration_ticks: i64,
    pub volume_scalar: Option<f64>,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_previous: bool,
    pub can_next: bool,
    pub can_seek: bool,
    pub can_set_volume: bool,
}

pub fn unavailable_snapshot(
    now: i64,
    volume_percent: Option<i64>,
    can_set_volume: bool,
) -> MediaSnapshot {
    MediaSnapshot {
        session_id: None,
        title: String::new(),
        artist: String::new(),
        playback_state: MediaPlaybackState::Unavailable,
        position_seconds: 0,
        duration_seconds: None,
        volume_percent,
        can_play: false,
        can_pause: false,
        can_previous: false,
        can_next: false,
        can_seek: false,
        can_set_volume,
        updated_at: now,
    }
}

pub fn map_native_snapshot(input: NativeMediaSnapshot, now: i64) -> MediaSnapshot {
    let volume_percent = input
        .volume_scalar
        .filter(|scalar| scalar.is_finite())
        .map(|scalar| (scalar.clamp(0.0, 1.0) * 100.0).round() as i64);
    if input.session_id.is_none() {
        return unavailable_snapshot(
            now,
            volume_percent,
            input.can_set_volume && volume_percent.is_some(),
        );
    }
    let duration_seconds =
        (input.duration_ticks > 0).then_some(input.duration_ticks / WINDOWS_TICKS_PER_SECOND);
    let raw_position = (input.position_ticks.max(0)) / WINDOWS_TICKS_PER_SECOND;
    let position_seconds = duration_seconds
        .map(|duration| raw_position.clamp(0, duration))
        .unwrap_or(raw_position);
    let playback_state = match input.playback_state {
        NativePlaybackState::Playing => MediaPlaybackState::Playing,
        NativePlaybackState::Paused => MediaPlaybackState::Paused,
        NativePlaybackState::Closed | NativePlaybackState::Other => MediaPlaybackState::Stopped,
    };
    MediaSnapshot {
        session_id: input.session_id,
        title: input.title,
        artist: input.artist,
        playback_state,
        position_seconds,
        duration_seconds,
        volume_percent,
        can_play: input.can_play,
        can_pause: input.can_pause,
        can_previous: input.can_previous,
        can_next: input.can_next,
        can_seek: input.can_seek,
        can_set_volume: input.can_set_volume && volume_percent.is_some(),
        updated_at: now,
    }
}

pub fn seconds_to_windows_ticks(seconds: f64) -> Option<i64> {
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    let ticks = (seconds * WINDOWS_TICKS_PER_SECOND as f64).round();
    if ticks > i64::MAX as f64 {
        return None;
    }
    Some(ticks as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn native() -> NativeMediaSnapshot {
        NativeMediaSnapshot {
            session_id: Some("app.session".into()),
            title: "Track".into(),
            artist: "Artist".into(),
            playback_state: NativePlaybackState::Playing,
            position_ticks: 125_000_000,
            duration_ticks: 100_000_000,
            volume_scalar: Some(0.35),
            can_play: true,
            can_pause: true,
            can_previous: true,
            can_next: true,
            can_seek: true,
            can_set_volume: true,
        }
    }

    #[test]
    fn maps_native_playback_timeline_capabilities_and_volume() {
        let mapped = map_native_snapshot(native(), 42);
        assert_eq!(mapped.session_id.as_deref(), Some("app.session"));
        assert_eq!(mapped.title, "Track");
        assert_eq!(mapped.artist, "Artist");
        assert_eq!(mapped.playback_state, MediaPlaybackState::Playing);
        assert_eq!(mapped.position_seconds, 10);
        assert_eq!(mapped.duration_seconds, Some(10));
        assert_eq!(mapped.volume_percent, Some(35));
        assert!(mapped.can_play && mapped.can_pause && mapped.can_seek && mapped.can_set_volume);
        assert_eq!(mapped.updated_at, 42);
    }

    #[test]
    fn maps_paused_closed_zero_duration_and_tick_rounding() {
        let mut paused = native();
        paused.playback_state = NativePlaybackState::Paused;
        paused.position_ticks = -1;
        paused.duration_ticks = 0;
        let paused = map_native_snapshot(paused, 43);
        assert_eq!(paused.playback_state, MediaPlaybackState::Paused);
        assert_eq!(paused.position_seconds, 0);
        assert_eq!(paused.duration_seconds, None);

        assert_eq!(seconds_to_windows_ticks(42.5), Some(425_000_000));
        assert_eq!(seconds_to_windows_ticks(-1.0), None);
        assert_eq!(seconds_to_windows_ticks(f64::NAN), None);
    }

    #[test]
    fn unavailable_media_keeps_independent_core_audio_state() {
        assert_eq!(
            unavailable_snapshot(44, Some(61), true),
            MediaSnapshot {
                session_id: None,
                title: String::new(),
                artist: String::new(),
                playback_state: MediaPlaybackState::Unavailable,
                position_seconds: 0,
                duration_seconds: None,
                volume_percent: Some(61),
                can_play: false,
                can_pause: false,
                can_previous: false,
                can_next: false,
                can_seek: false,
                can_set_volume: true,
                updated_at: 44,
            }
        );
    }
}
