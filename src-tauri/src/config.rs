//! Persistent configuration, stored as JSON next to the app database.

use std::path::PathBuf;

use crate::encode;
use crate::model::{
    AppConfig, BufferConfig, ControlConfig, OverlayConfig, RecordingConfig, SourceKind, TargetKind,
};

pub fn data_dir() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("ClippiBoy")
}

pub fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

pub fn default_clip_dir() -> PathBuf {
    dirs::video_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("ClippiBoy")
}

pub fn default_config() -> AppConfig {
    AppConfig {
        recording: RecordingConfig {
            target_kind: TargetKind::Monitor,
            target_id: None,
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_kbps: encode::bitrate_for(1920, 1080, 60),
            quality: crate::model::default_quality(),
            encoder: encode::preferred_encoder(),
            keyframe_seconds: 2,
        },
        buffer: BufferConfig {
            auto_start: false,
            seconds: 120,
            clip_seconds: 0,
            memory_mb: 0,
        },
        sources: Vec::new(),
        clip_dir: default_clip_dir().to_string_lossy().to_string(),
        save_clip_hotkey: "Ctrl+Shift+S".into(),
        toggle_buffer_hotkey: "Ctrl+Shift+B".into(),
        screenshot_hotkey: crate::model::default_screenshot_hotkey(),
        auto_start_with_windows: false,
        only_buffer_in_game: true,
        overlay: OverlayConfig::default(),
        tray_hint_shown: false,
        control: ControlConfig::default(),
    }
}

/// How many seconds a save actually writes.
///
/// `0` means "the whole buffer" — that was the only behaviour until this setting
/// existed, so a config written before it keeps producing the same clips.
///
/// Never longer than the buffer either: asking for a two-minute clip out of a
/// thirty-second buffer cannot give more than thirty seconds, and pretending
/// otherwise would only put a number in the UI that the file then contradicts.
pub fn effective_clip_seconds(buffer: &BufferConfig) -> u32 {
    if buffer.clip_seconds == 0 {
        return buffer.seconds;
    }
    buffer.clip_seconds.min(buffer.seconds)
}

/// How much memory the packet ring may take, in bytes.
///
/// `0` derives it from bitrate and buffer length, with half again on top for the
/// peaks a variable bitrate produces. That is roughly what the ring cost before
/// there was a budget at all, so a config written without one keeps behaving the
/// way it did.
pub fn effective_memory_bytes(recording: &RecordingConfig, buffer: &BufferConfig) -> u64 {
    if buffer.memory_mb > 0 {
        return buffer.memory_mb as u64 * 1024 * 1024;
    }
    let bytes_per_second = recording.bitrate_kbps as u64 * 1000 / 8;
    let plain = bytes_per_second * buffer.seconds.max(1) as u64;
    // A floor, so a tiny buffer at a low bitrate still has room to hold the
    // group of pictures it must never drop.
    (plain * 3 / 2).max(64 * 1024 * 1024)
}

/// Undo the detour where the leftovers were briefly a source kind of their own,
/// limited to one, with the other endpoint sources switched off.
///
/// There is nothing to switch on any more — an output device records its
/// leftovers by itself as soon as something is recorded application by
/// application. The device that source once pointed at is not recoverable, the
/// kind carried no id, so it falls back to the default output device, which is
/// what an empty id means everywhere else (see `capture::device_client`).
fn migrate_sources(config: &mut AppConfig) {
    if !config
        .sources
        .iter()
        .any(|source| matches!(source.kind, SourceKind::Leftovers))
    {
        return;
    }

    for source in &mut config.sources {
        match &source.kind {
            SourceKind::Leftovers => {
                source.kind = SourceKind::OutputDevice {
                    device_id: String::new(),
                };
                log::info!(
                    "'{}' is the default output device again",
                    source.label
                );
            }
            SourceKind::OutputDevice { .. } if !source.enabled => {
                source.enabled = true;
                log::info!("'{}' switched back on", source.label);
            }
            _ => {}
        }
    }
}

pub fn load() -> AppConfig {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<AppConfig>(&text) {
            Ok(mut config) => {
                // Check the encoder against the hardware actually present.
                config.recording.encoder = encode::resolve(config.recording.encoder);
                // The bitrate is derived from the picture, not chosen by hand —
                // a config from when it was a slider carries a number that means
                // nothing now (40 Mbit/s at any resolution, as it happens). It
                // would otherwise still size the memory budget.
                config.recording.bitrate_kbps = encode::bitrate_for(
                    config.recording.width,
                    config.recording.height,
                    config.recording.fps,
                );
                migrate_sources(&mut config);
                config
            }
            Err(err) => {
                log::warn!("config unreadable ({err}) — falling back to defaults");
                default_config()
            }
        },
        Err(_) => default_config(),
    }
}

pub fn save(config: &AppConfig) -> std::io::Result<()> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    let text = serde_json::to_string_pretty(config)?;
    // Write to a temp file first, then rename — never half-written JSON.
    let tmp = config_path().with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, config_path())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AudioSource;

    fn source(id: &str, kind: SourceKind, enabled: bool) -> AudioSource {
        AudioSource {
            id: id.into(),
            label: id.into(),
            kind,
            enabled,
            gain_db: 0.0,
            muted: false,
            solo: false,
            separate_track: false,
        }
    }

    fn endpoint(device_id: &str) -> SourceKind {
        SourceKind::OutputDevice {
            device_id: device_id.into(),
        }
    }

    fn with(sources: Vec<AudioSource>) -> AppConfig {
        AppConfig {
            sources,
            ..default_config()
        }
    }

    /// A config written before the Stream Deck port existed has no `control`
    /// block. Without `#[serde(default)]` on that field the parse fails, and
    /// `load()` answers a failed parse by throwing the **whole** configuration
    /// away — audio sources, hotkeys and clip folder included. That is the most
    /// expensive bug this file can have, so it gets a test rather than a comment.
    #[test]
    fn a_configuration_written_before_the_stream_deck_port_still_loads() {
        let mut written =
            serde_json::to_value(with(vec![source("mic", endpoint("dev-1"), true)])).unwrap();
        written
            .as_object_mut()
            .expect("a config is an object")
            .remove("control")
            .expect("the field is called 'control' in JSON too");

        let parsed: AppConfig =
            serde_json::from_value(written).expect("a config without 'control' no longer loads");
        assert_eq!(parsed.sources.len(), 1);
        assert_eq!(parsed.control.port, crate::model::DEFAULT_CONTROL_PORT);
        // Nothing generated yet — `control::start` fills it in on first use.
        assert!(parsed.control.token.is_empty());
    }

    /// What the intermediate version left behind: one source turned into a kind
    /// of its own, the other endpoints switched off.
    #[test]
    fn the_leftovers_kind_becomes_a_plain_device_again() {
        let mut config = with(vec![
            source("system", SourceKind::Leftovers, true),
            source("chat", endpoint("dev-chat"), false),
            source("music", endpoint("dev-music"), false),
        ]);
        migrate_sources(&mut config);

        // The kind carried no device, so the default output device it is.
        assert_eq!(config.sources[0].kind, endpoint(""));
        for source in &config.sources[1..] {
            assert!(source.enabled, "switched off by the detour, not by the user");
            assert_eq!(
                source.kind,
                endpoint(&format!("dev-{}", source.id)),
                "the device has to survive"
            );
        }
    }

    /// Nothing to undo — a disabled endpoint stays disabled.
    #[test]
    fn a_configuration_without_the_detour_is_left_alone() {
        let mut config = with(vec![
            source("speakers", endpoint("dev-1"), true),
            source("headset", endpoint("dev-2"), false),
        ]);
        let before = config.sources.clone();
        migrate_sources(&mut config);

        for (before, after) in before.iter().zip(&config.sources) {
            assert_eq!(before.kind, after.kind);
            assert_eq!(before.enabled, after.enabled);
        }
    }

    fn buffer(seconds: u32, clip_seconds: u32) -> BufferConfig {
        BufferConfig {
            auto_start: false,
            seconds,
            clip_seconds,
            memory_mb: 0,
        }
    }

    /// The value every config written before this setting existed carries. It has
    /// to mean "the whole buffer", or an update would silently shorten everyone's
    /// clips.
    #[test]
    fn zero_means_the_whole_buffer() {
        assert_eq!(effective_clip_seconds(&buffer(120, 0)), 120);
    }

    #[test]
    fn a_shorter_clip_is_taken_as_it_stands() {
        assert_eq!(effective_clip_seconds(&buffer(120, 30)), 30);
    }

    /// The buffer is the hard limit — there is nothing older to write.
    #[test]
    fn a_clip_longer_than_the_buffer_is_capped() {
        assert_eq!(effective_clip_seconds(&buffer(30, 120)), 30);
    }

    /// The flag one version wrote onto the device is gone from the model. Serde
    /// has to shrug it off rather than fail — a configuration it cannot read is
    /// thrown away whole.
    #[test]
    fn the_old_flag_on_a_device_is_ignored() {
        let raw = r#"{"type":"outputDevice","deviceId":"d","leftoversOnly":true}"#;
        let kind: SourceKind = serde_json::from_str(raw).unwrap();
        assert_eq!(kind, endpoint("d"));
    }
}
