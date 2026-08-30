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

/// Sources written while "leftovers" was still a flag on an output device.
///
/// The flag is gone from the model, so serde drops it silently — and a
/// configuration where several devices all claimed the leftovers would come back
/// as several *full* endpoint loopbacks, which doubles far worse than the bug
/// this replaces. So the raw JSON is consulted once more: the first such source
/// becomes the one leftovers source, every further one is switched off with its
/// device left intact, so it can be turned back on by hand.
fn migrate_leftovers(config: &mut AppConfig, raw: &str) {
    let flagged: Vec<String> = serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .as_ref()
        .and_then(|value| value.pointer("/sources"))
        .and_then(|sources| sources.as_array())
        .map(|sources| {
            sources
                .iter()
                .filter(|source| {
                    source
                        .pointer("/kind/leftoversOnly")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false)
                })
                .filter_map(|source| source.get("id")?.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if flagged.is_empty() {
        return;
    }

    let mut kept = false;
    for source in config
        .sources
        .iter_mut()
        .filter(|source| flagged.contains(&source.id))
    {
        if !kept {
            source.kind = SourceKind::Leftovers;
            kept = true;
            log::info!("'{}' is now the leftovers source", source.label);
        } else {
            source.enabled = false;
            log::info!(
                "'{}' switched off: there can only be one leftovers source",
                source.label
            );
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
                migrate_leftovers(&mut config, &text);
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

    /// A source in the shape written while "leftovers" was a flag on the device.
    fn old_source(id: &str, leftovers: bool) -> String {
        format!(
            r#"{{
                "id": "{id}",
                "label": "{id}",
                "kind": {{
                    "type": "outputDevice",
                    "deviceId": "dev-{id}",
                    "leftoversOnly": {leftovers}
                }},
                "enabled": true,
                "gainDb": 0.0,
                "muted": false,
                "solo": false,
                "separateTrack": false
            }}"#
        )
    }

    fn config_with(sources: &[String]) -> (AppConfig, String) {
        let mut config = default_config();
        let raw = format!(r#"{{ "sources": [{}] }}"#, sources.join(","));
        config.sources = serde_json::from_str::<serde_json::Value>(&raw).unwrap()["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| serde_json::from_value(s.clone()).unwrap())
            .collect();
        (config, raw)
    }

    /// The real shape of a GoXLR setup: five virtual devices, every one of them
    /// asking for the leftovers. Left alone, all five would come back as full
    /// endpoint loopbacks and record everything five times over.
    #[test]
    fn only_one_source_survives_as_the_leftovers() {
        let (mut config, raw) = config_with(&[
            old_source("system", true),
            old_source("chat", true),
            old_source("music", true),
            old_source("game", true),
            old_source("sample", true),
        ]);
        migrate_leftovers(&mut config, &raw);

        assert_eq!(config.sources[0].kind, SourceKind::Leftovers);
        assert!(config.sources[0].enabled);
        for source in &config.sources[1..] {
            assert!(
                matches!(source.kind, SourceKind::OutputDevice { .. }),
                "the device has to survive so it can be switched back on"
            );
            assert!(!source.enabled, "a second leftovers source must not run");
        }
    }

    /// A configuration that never had the flag has to come through untouched.
    #[test]
    fn a_configuration_without_the_flag_is_left_alone() {
        let (mut config, raw) =
            config_with(&[old_source("speakers", false), old_source("headset", false)]);
        let before = config.sources.clone();
        migrate_leftovers(&mut config, &raw);

        for (before, after) in before.iter().zip(&config.sources) {
            assert_eq!(before.kind, after.kind);
            assert_eq!(before.enabled, after.enabled);
        }
    }
}
