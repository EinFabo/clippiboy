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
//!
//! `PlaySound` only knows Windows' default device. A device of their own
//! (`SoundConfig::device`) keeps the sounds out of a buffer recording the
//! default one, so that one goes through WASAPI: a short shared-mode stream on
//! a thread of its own. A device that is gone falls back to the default.

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

/// May a sound play? In streamer mode only on a device of its own: the default
/// one is what the stream records — and a chosen device that is at the same
/// time Windows' default output (`defaults`) is no device of its own.
pub fn audible(streamer: bool, device: Option<&str>, defaults: &[String]) -> bool {
    !streamer || device.is_some_and(|id| !id.is_empty() && !defaults.iter().any(|d| d == id))
}

/// [`audible`] as things stand now. Asks Windows for its default outputs only
/// in streamer mode.
pub fn audible_now(device: Option<&str>) -> bool {
    let streamer = crate::streamer::active();
    let defaults = if streamer && device.is_some() { imp::default_outputs() } else { Vec::new() };
    audible(streamer, device, &defaults)
}

/// Play the sound for `kind`, as the settings say.
pub fn play(app: &tauri::AppHandle, kind: SoundKind) {
    let config = app.state::<AppState>().config_snapshot().sounds;
    if !audible_now(config.device.as_deref()) {
        return;
    }
    play_at(kind, level(&config, kind), config.device);
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

/// Play `kind` at `gain` (0 to 1) on `device` (`None`: Windows' default),
/// whatever the settings say — the settings' own "listen" button comes
/// through here with the level being set.
pub fn play_at(kind: SoundKind, gain: f32, device: Option<String>) {
    if gain <= 0.0 {
        return;
    }
    let data = scaled(kind, gain.min(1.0));
    imp::play(data, device);
}

/// The format in a plain 44-byte WAV header: (channels, samples a second,
/// bits a sample).
fn header_format(wav: &[u8]) -> (u16, u32, u16) {
    let u16_at = |at: usize| u16::from_le_bytes([wav[at], wav[at + 1]]);
    let u32_at = |at: usize| u32::from_le_bytes([wav[at], wav[at + 1], wav[at + 2], wav[at + 3]]);
    (u16_at(22), u32_at(24), u16_at(34))
}

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use parking_lot::Mutex;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Media::Audio::{
        IAudioClient, IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator, PlaySoundW,
        AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
        AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, SND_ASYNC, SND_FLAGS, SND_MEMORY, SND_NODEFAULT,
        WAVEFORMATEX, WAVE_FORMAT_PCM,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    use super::{header_format, HEADER};

    /// The sound playing now. With `SND_ASYNC` Windows reads from this memory
    /// while it plays, so it has to outlive the call — until the next sound
    /// has taken over, which stops this one first.
    static PLAYING: Mutex<Option<Vec<u8>>> = Mutex::new(None);

    /// Counts the sounds started. A stream on its own device stops as soon as a
    /// newer sound has started, the same way `PlaySound` cuts the last one off.
    static LATEST: AtomicU64 = AtomicU64::new(0);

    pub fn play(data: Vec<u8>, device: Option<String>) {
        let this = LATEST.fetch_add(1, Ordering::SeqCst) + 1;
        match device {
            None => play_default(data),
            Some(id) => {
                // A default-device sound still ringing gives way, too.
                stop_default();
                let spawned = std::thread::Builder::new()
                    .name("sound".into())
                    .spawn(move || {
                        if let Err(err) = play_on(&id, &data, this) {
                            log::warn!("sound on its own device failed ({err}), playing on the default one");
                            // Not into a stream, though (`audible`).
                            if LATEST.load(Ordering::SeqCst) == this && !crate::streamer::active() {
                                play_default(data);
                            }
                        }
                    });
                if let Err(err) = spawned {
                    log::warn!("no thread for a sound: {err}");
                }
            }
        }
    }

    /// The endpoint ids of Windows' default output, for games/system sounds
    /// (`eConsole`) and for media (`eMultimedia`) — usually the same one.
    pub fn default_outputs() -> Vec<String> {
        use windows::Win32::Media::Audio::{eConsole, eMultimedia, eRender};
        let mut out = Vec::new();
        // SAFETY: plain COM calls; the id string is freed after copying it.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let Ok(enumerator) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
            else {
                return out;
            };
            for role in [eConsole, eMultimedia] {
                let Ok(device) = enumerator.GetDefaultAudioEndpoint(eRender, role) else { continue };
                let Ok(id) = device.GetId() else { continue };
                if let Ok(text) = id.to_string() {
                    if !out.contains(&text) {
                        out.push(text);
                    }
                }
                CoTaskMemFree(Some(id.0 as *const _));
            }
        }
        out
    }

    fn stop_default() {
        let mut playing = PLAYING.lock();
        // SAFETY: a null sound only stops what is playing.
        unsafe {
            let _ = PlaySoundW(PCWSTR::null(), HMODULE::default(), SND_FLAGS(0));
        }
        *playing = None;
    }

    /// One shared-mode stream on `id`, filled as it drains, stopped early when a
    /// newer sound starts. WASAPI converts the WAV's rate and channels to the
    /// device's own mix format (`AUTOCONVERTPCM`).
    fn play_on(id: &str, data: &[u8], this: u64) -> windows::core::Result<()> {
        let (channels, rate, bits) = header_format(data);
        let block = channels * bits / 8;
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: channels,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * block as u32,
            nBlockAlign: block,
            wBitsPerSample: bits,
            cbSize: 0,
        };
        let pcm = &data[HEADER..];
        let total = (pcm.len() / block as usize) as u32;
        let superseded = || LATEST.load(Ordering::SeqCst) != this;

        // SAFETY: plain COM calls on objects owned by this thread; every buffer
        // WASAPI hands out is written within its length before being released.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
            let device = enumerator.GetDevice(PCWSTR(wide.as_ptr()))?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                // 200 ms in 100-ns units.
                2_000_000,
                0,
                &format,
                None,
            )?;
            let size = client.GetBufferSize()?;
            let render: IAudioRenderClient = client.GetService()?;

            let mut written = 0u32;
            let mut started = false;
            while written < total {
                if superseded() {
                    let _ = client.Stop();
                    return Ok(());
                }
                let free = size - client.GetCurrentPadding()?;
                let frames = free.min(total - written);
                if frames > 0 {
                    let out = render.GetBuffer(frames)?;
                    let from = written as usize * block as usize;
                    let bytes = frames as usize * block as usize;
                    std::ptr::copy_nonoverlapping(pcm[from..].as_ptr(), out, bytes);
                    render.ReleaseBuffer(frames, 0)?;
                    written += frames;
                }
                if !started {
                    client.Start()?;
                    started = true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            // Let the last of it play out before the stream goes.
            while client.GetCurrentPadding()? > 0 && !superseded() {
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = client.Stop();
        }
        Ok(())
    }

    fn play_default(data: Vec<u8>) {
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
    pub fn play(_data: Vec<u8>, _device: Option<String>) {}

    pub fn default_outputs() -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamer_mode_keeps_sounds_off_the_default_device() {
        assert!(audible(false, None, &[]));
        assert!(audible(false, Some("{dev}"), &[]));
        assert!(audible(true, Some("{dev}"), &[]));
        assert!(!audible(true, None, &[]));
        assert!(!audible(true, Some(""), &[]));
    }

    #[test]
    fn a_chosen_device_that_is_the_windows_default_is_not_one_of_its_own() {
        let defaults = vec!["{default}".to_string()];
        assert!(!audible(true, Some("{default}"), &defaults));
        assert!(audible(true, Some("{other}"), &defaults));
        // Outside streamer mode the default is as good as any.
        assert!(audible(false, Some("{default}"), &defaults));
    }

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
            let (channels, rate, bits) = header_format(data);
            assert_eq!((channels, bits), (2, 16), "{kind:?}");
            assert!(rate >= 8_000, "{kind:?}: rate {rate}");
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
