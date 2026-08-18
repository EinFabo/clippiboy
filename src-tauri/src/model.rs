//! Datentypen der IPC-Schnittstelle. Muss zu src/lib/types.ts passen.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Output,
    Input,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioProcess {
    pub pid: u32,
    pub name: String,
    pub exe: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProcessMode {
    Include,
    Exclude,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SourceKind {
    #[serde(rename_all = "camelCase")]
    OutputDevice { device_id: String },
    #[serde(rename_all = "camelCase")]
    InputDevice { device_id: String },
    #[serde(rename_all = "camelCase")]
    Process { pid: u32, mode: ProcessMode },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSource {
    pub id: String,
    pub label: String,
    pub kind: SourceKind,
    pub enabled: bool,
    pub gain_db: f32,
    pub muted: bool,
    pub solo: bool,
    /// Eigene Tonspur im MP4 statt Hauptmix.
    pub separate_track: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EncoderId {
    Nvenc,
    Amf,
    Qsv,
    X264,
}

impl EncoderId {
    pub fn ffmpeg_name(self) -> &'static str {
        match self {
            EncoderId::Nvenc => "h264_nvenc",
            EncoderId::Amf => "h264_amf",
            EncoderId::Qsv => "h264_qsv",
            EncoderId::X264 => "libx264",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncoderInfo {
    pub id: EncoderId,
    pub name: String,
    pub available: bool,
    pub hardware: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetKind {
    Monitor,
    Window,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureTarget {
    pub kind: TargetKind,
    pub id: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingConfig {
    pub target_kind: TargetKind,
    pub target_id: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub encoder: EncoderId,
    pub keyframe_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BufferConfig {
    /// Puffer selbsttätig einschalten — beim Start von ClippiBoy, oder erst
    /// sobald ein Spiel im Vordergrund ist (siehe `only_buffer_in_game`).
    #[serde(default)]
    pub auto_start: bool,
    pub seconds: u32,
}

/// In welcher Bildschirmecke der Overlay-Banner erscheint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OverlayCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Der Banner, der über dem Spiel eingeblendet wird (Medal/ShadowPlay-Stil).
/// Jede Meldungsart ist einzeln abschaltbar — im Zweifel stört sie beim Spielen.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayConfig {
    pub enabled: bool,
    pub on_clip_saved: bool,
    pub on_buffer_toggle: bool,
    pub on_error: bool,
    pub corner: OverlayCorner,
    pub duration_ms: u32,
    /// Bildschirm, auf dem der Banner klebt (Gerätename wie `\\.\DISPLAY1`).
    /// `None` heißt: der primäre Bildschirm.
    #[serde(default)]
    pub monitor: Option<String>,
    /// Statt eines festen Bildschirms dem Fenster im Vordergrund folgen.
    /// Praktisch beim Zocken auf wechselnden Monitoren, aber der Banner
    /// springt dann.
    #[serde(default)]
    pub follow_active_screen: bool,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            on_clip_saved: true,
            on_buffer_toggle: true,
            on_error: true,
            corner: OverlayCorner::BottomRight,
            duration_ms: 3500,
            monitor: None,
            follow_active_screen: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub recording: RecordingConfig,
    pub buffer: BufferConfig,
    pub sources: Vec<AudioSource>,
    pub clip_dir: String,
    pub save_clip_hotkey: String,
    pub toggle_buffer_hotkey: String,
    pub auto_start_with_windows: bool,
    pub only_buffer_in_game: bool,
    // Neu hinzugekommene Felder brauchen `default` — sonst wirft `config::load()`
    // beim Update die komplette gespeicherte Konfiguration weg (samt Audioquellen).
    #[serde(default)]
    pub overlay: OverlayConfig,
    /// Wurde schon einmal erklärt, dass die App beim Schließen im Tray weiterläuft?
    #[serde(default)]
    pub tray_hint_shown: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    pub id: String,
    pub path: String,
    /// Unix-Millisekunden.
    pub created_at: i64,
    pub duration_ms: u64,
    pub game: Option<String>,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub thumb_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub buffer_active: bool,
    pub buffered_seconds: f32,
    pub buffer_bytes: u64,
    pub dropped_frames: u64,
    pub encoder: Option<EncoderId>,
    pub fps: f32,
    /// Zuletzt im Vordergrund erkanntes Spiel, `None` wenn keins läuft.
    pub game: Option<String>,
}
