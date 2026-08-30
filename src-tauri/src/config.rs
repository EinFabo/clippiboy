//! Persistent configuration, stored as JSON next to the app database.

use std::path::PathBuf;

use crate::encode;
use crate::model::{
    AppConfig, BufferConfig, OverlayConfig, RecordingConfig, SourceKind, TargetKind,
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
            bitrate_kbps: 40_000,
            encoder: encode::preferred_encoder(),
            keyframe_seconds: 2,
        },
        buffer: BufferConfig {
            auto_start: false,
            seconds: 120,
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
    }
}

/// Undo the detour where the leftovers were briefly a source kind of their own,
/// limited to one, with the other endpoint sources switched off.
///
/// They are an option on an output device again, and several devices may have
/// it. The device that source once pointed at is not recoverable — the kind
/// carried no id — so it falls back to the default output device, which is what
/// an empty id means everywhere else (see `capture::device_client`). The
/// endpoints that detour switched off are turned back on with the option set,
/// because that is what they were before it.
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
                    leftovers_only: true,
                };
                log::info!(
                    "'{}' records the leftovers of the default output device again",
                    source.label
                );
            }
            SourceKind::OutputDevice { device_id, .. } if !source.enabled => {
                source.kind = SourceKind::OutputDevice {
                    device_id: device_id.clone(),
                    leftovers_only: true,
                };
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

    fn endpoint(device_id: &str, leftovers_only: bool) -> SourceKind {
        SourceKind::OutputDevice {
            device_id: device_id.into(),
            leftovers_only,
        }
    }

    fn with(sources: Vec<AudioSource>) -> AppConfig {
        AppConfig {
            sources,
            ..default_config()
        }
    }

    /// What the intermediate version left behind: one source turned into a kind
    /// of its own, the other endpoints switched off.
    #[test]
    fn the_one_leftovers_kind_becomes_an_option_again() {
        let mut config = with(vec![
            source("system", SourceKind::Leftovers, true),
            source("chat", endpoint("dev-chat", false), false),
            source("music", endpoint("dev-music", false), false),
        ]);
        migrate_sources(&mut config);

        // The kind carried no device, so the default output device it is.
        assert_eq!(config.sources[0].kind, endpoint("", true));
        for source in &config.sources[1..] {
            assert!(source.enabled, "switched off by the detour, not by the user");
            assert_eq!(
                source.kind,
                endpoint(&format!("dev-{}", source.id), true),
                "they asked for the leftovers before the detour"
            );
        }
    }

    /// Nothing to undo — a disabled endpoint stays disabled.
    #[test]
    fn a_configuration_without_the_detour_is_left_alone() {
        let mut config = with(vec![
            source("speakers", endpoint("dev-1", false), true),
            source("headset", endpoint("dev-2", false), false),
        ]);
        let before = config.sources.clone();
        migrate_sources(&mut config);

        for (before, after) in before.iter().zip(&config.sources) {
            assert_eq!(before.kind, after.kind);
            assert_eq!(before.enabled, after.enabled);
        }
    }

    /// The flag on the device survived the detour and must load as it is.
    #[test]
    fn the_option_still_deserializes() {
        let raw = r#"{"type":"outputDevice","deviceId":"d","leftoversOnly":true}"#;
        let kind: SourceKind = serde_json::from_str(raw).unwrap();
        assert_eq!(kind, endpoint("d", true));
    }
}
