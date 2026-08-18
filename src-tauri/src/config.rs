//! Persistente Konfiguration als JSON neben der App-Datenbank.

use std::path::PathBuf;

use crate::encode;
use crate::model::{AppConfig, BufferConfig, OverlayConfig, RecordingConfig, TargetKind};

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
        auto_start_with_windows: false,
        only_buffer_in_game: true,
        overlay: OverlayConfig::default(),
        tray_hint_shown: false,
    }
}

pub fn load() -> AppConfig {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<AppConfig>(&text) {
            Ok(mut config) => {
                // Encoder gegen die tatsächlich vorhandene Hardware prüfen.
                config.recording.encoder = encode::resolve(config.recording.encoder);
                config
            }
            Err(err) => {
                log::warn!("Konfiguration unlesbar ({err}) — Standardwerte werden benutzt");
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
    // Erst temporär schreiben, dann umbenennen — kein halb geschriebenes JSON.
    let tmp = config_path().with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, config_path())
}
