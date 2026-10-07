//! The notification sounds: a short tone when a clip is saved, a recording
//! starts, a friend comes online.
//!
//! Chosen in the sound lab and rendered once to WAV (`scripts/render-sounds.cjs`);
//! they ship inside the binary. Played from here rather than from a web view so
//! they sound with the window in the tray, and through `PlaySound`: one call,
//! nothing to set up, and a new sound simply takes over from one still ringing —
//! two tones on top of each other would only blur.
//!
//! The levels are applied to the samples before playing: `PlaySound` has no
//! volume of its own.

use tauri::Manager;

use crate::model::{SoundConfig, SoundKind};
use crate::state::AppState;

const CLIP_SAVED: &[u8] = include_bytes!("../assets/sounds/clip-saved.wav");
const SCREENSHOT: &[u8] = include_bytes!("../assets/sounds/screenshot.wav");
const RECORDING_STARTED: &[u8] = include_bytes!("../assets/sounds/recording-started.wav");
const RECORDING_SAVED: &[u8] = include_bytes!("../assets/sounds/recording-saved.wav");
const BUFFER_ON: &[u8] = include_bytes!("../assets/sounds/buffer-on.wav");
const BUFFER_OFF: &[u8] = include_bytes!("../assets/sounds/buffer-off.wav");
const FRIEND_ONLINE: &[u8] = include_bytes!("../assets/sounds/friend-online.wav");
const CLIP_RECEIVED: &[u8] = include_bytes!("../assets/sounds/clip-received.wav");
const ERROR: &[u8] = include_bytes!("../assets/sounds/error.wav");

/// The rendered files all carry the plain 44-byte header: 16-bit PCM, stereo.
const HEADER: usize = 44;

fn wav(kind: SoundKind) -> &'static [u8] {
    match kind {
        SoundKind::ClipSaved => CLIP_SAVED,
        SoundKind::Screenshot => SCREENSHOT,
        SoundKind::RecordingStarted => RECORDING_STARTED,
        SoundKind::RecordingSaved => RECORDING_SAVED,
        SoundKind::BufferOn => BUFFER_ON,
        SoundKind::BufferOff => BUFFER_OFF,
        SoundKind::FriendOnline => FRIEND_ONLINE,
        SoundKind::ClipReceived => CLIP_RECEIVED,
        SoundKind::Error => ERROR,
    }
}

/// How loud `kind` plays under these settings; 0 when it does not play.
pub fn level(config: &SoundConfig, kind: SoundKind) -> f32 {
    let setting = config.setting(kind);
    if !config.enabled || !setting.on {
        return 0.0;
    }
    (config.volume * setting.volume).clamp(0.0, 1.0)
}

/// Play the sound for `kind`, as the settings say.
pub fn play(app: &tauri::AppHandle, kind: SoundKind) {
    let config = app.state::<AppState>().config_snapshot().sounds;
    play_at(kind, level(&config, kind));
}

/// The WAV for `kind` with every sample scaled by `gain`.
fn scaled(kind: SoundKind, gain: f32) -> Vec<u8> {
    let source = wav(kind);
    let mut out = source.to_vec();
    for sample in out[HEADER..].chunks_exact_mut(2) {
        let value = i16::from_le_bytes([sample[0], sample[1]]) as f32 * gain;
        sample.copy_from_slice(&(value.round() as i16).to_le_bytes());
    }
    out
}

/// Play `kind` at `gain` (0 to 1), whatever the settings say — the settings'
/// own "listen" button comes through here with the level being set.
pub fn play_at(kind: SoundKind, gain: f32) {
    if gain <= 0.0 {
        return;
    }
    let data = scaled(kind, gain.min(1.0));
    imp::play(data);
}

#[cfg(windows)]
mod imp {
    use parking_lot::Mutex;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

    /// The sound playing now. With `SND_ASYNC` Windows reads from this memory
    /// while it plays, so it has to outlive the call — until the next sound
    /// has taken over, which stops this one first.
    static PLAYING: Mutex<Option<Vec<u8>>> = Mutex::new(None);

    pub fn play(data: Vec<u8>) {
        let mut playing = PLAYING.lock();
        // SAFETY: `data` is a complete WAV image and stays alive in `PLAYING`
        // until a later call has started a new sound in its place.
        let ok = unsafe {
            PlaySoundW(
                PCWSTR(data.as_ptr() as *const u16),
                HMODULE::default(),
                SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
            )
        };
        if !ok.as_bool() {
            log::warn!("notification sound did not play");
        }
        *playing = Some(data);
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn play(_data: Vec<u8>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [SoundKind; 9] = [
        SoundKind::ClipSaved,
        SoundKind::Screenshot,
        SoundKind::RecordingStarted,
        SoundKind::RecordingSaved,
        SoundKind::BufferOn,
        SoundKind::BufferOff,
        SoundKind::FriendOnline,
        SoundKind::ClipReceived,
        SoundKind::Error,
    ];

    #[test]
    fn every_sound_is_plain_16_bit_stereo_pcm() {
        for kind in ALL {
            let data = wav(kind);
            assert_eq!(&data[0..4], b"RIFF", "{kind:?}");
            assert_eq!(&data[36..40], b"data", "{kind:?}: header is not the plain 44 bytes");
            assert_eq!(u16::from_le_bytes([data[20], data[21]]), 1, "{kind:?}: not PCM");
            assert_eq!(u16::from_le_bytes([data[22], data[23]]), 2, "{kind:?}: not stereo");
            assert_eq!(u16::from_le_bytes([data[34], data[35]]), 16, "{kind:?}: not 16 bit");
            let length = u32::from_le_bytes([data[40], data[41], data[42], data[43]]) as usize;
            assert_eq!(length, data.len() - HEADER, "{kind:?}");
        }
    }

    #[test]
    fn half_the_level_halves_the_samples() {
        let full = scaled(SoundKind::ClipSaved, 1.0);
        let half = scaled(SoundKind::ClipSaved, 0.5);
        assert_eq!(full, CLIP_SAVED);
        let peak = |data: &[u8]| {
            data[HEADER..]
                .chunks_exact(2)
                .map(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs())
                .max()
                .unwrap()
        };
        assert!((peak(&half) as i32 - peak(&full) as i32 / 2).abs() <= 1);
    }

    #[test]
    fn switched_off_means_silent() {
        let mut config = SoundConfig::default();
        assert!(level(&config, SoundKind::Error) > 0.0);
        config.error.on = false;
        assert_eq!(level(&config, SoundKind::Error), 0.0);
        config.error.on = true;
        config.enabled = false;
        assert_eq!(level(&config, SoundKind::Error), 0.0);
    }
}
