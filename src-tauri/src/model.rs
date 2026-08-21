//! Data types of the IPC interface. Must match src/lib/types.ts.
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
    /// Its own audio track in the MP4 instead of the main mix.
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
    /// The screen's refresh rate in hertz — for a window, that of the screen
    /// it sits on. `None` if Windows does not report it.
    pub refresh_hz: Option<u32>,
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
    /// Switch the buffer on by itself — at ClippiBoy's start, or only once a
    /// game is in the foreground (see `only_buffer_in_game`).
    #[serde(default)]
    pub auto_start: bool,
    pub seconds: u32,
}

/// Which screen corner the overlay banner appears in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OverlayCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// The banner shown over the game (Medal/ShadowPlay style). Every kind of
/// message can be switched off on its own — in doubt it gets in the way while
/// playing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayConfig {
    pub enabled: bool,
    pub on_clip_saved: bool,
    pub on_buffer_toggle: bool,
    pub on_error: bool,
    pub corner: OverlayCorner,
    pub duration_ms: u32,
    /// The screen the banner sticks to (device name like `\\.\DISPLAY1`).
    /// `None` means the primary screen.
    #[serde(default)]
    pub monitor: Option<String>,
    /// Follow the foreground window instead of a fixed screen. Handy when
    /// playing across changing monitors, but the banner jumps around then.
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
    // Newly added fields need `default` — otherwise `config::load()` throws away
    // the entire saved config on update (audio sources included).
    #[serde(default)]
    pub overlay: OverlayConfig,
    /// Has it already been explained that closing leaves the app in the tray?
    #[serde(default)]
    pub tray_hint_shown: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    pub id: String,
    pub path: String,
    /// Unix milliseconds.
    pub created_at: i64,
    pub duration_ms: u64,
    pub game: Option<String>,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub thumb_path: Option<String>,
    /// A name given by hand. Without it the gallery shows the file name.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Marked with the heart. Doubles as its own category: the file then lives
    /// in the `Favorites` folder, while inside the app the clip stays findable
    /// under its game.
    #[serde(default)]
    pub favorite: bool,
    /// Trim and track mix as they last stood in the editor. `None` means
    /// untouched, i.e. the whole clip with all tracks.
    #[serde(default)]
    pub edit: Option<ClipEdit>,
    /// Is the untouched recording still in the originals store? Then this clip
    /// has been trimmed and can be pulled open again at any time.
    #[serde(default)]
    pub original: Option<ClipOriginal>,
}

/// What was set in the editor.
///
/// After saving, the trim sits **in the file** — so `start_ms` and `end_ms`
/// only describe the full range of the current file, i.e. `0 .. duration`.
/// Where that range sat in the original is recorded in [`ClipOriginal`]. The
/// levels, by contrast, are the truth here: they can be changed at any time
/// because the individual tracks stay put alongside.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipEdit {
    /// Trim in milliseconds from the start of the clip.
    pub start_ms: u64,
    pub end_ms: u64,
    #[serde(default)]
    pub tracks: Vec<TrackMix>,
}

/// The recording that was trimmed away and lives on under
/// `<data>/originals/<clip-id>/` — and where inside it the delivered excerpt
/// sits.
///
/// These numbers are the zero point for everything else: the individual tracks
/// are always in coordinates of the **original**, while the handles in the
/// player are in coordinates of the **current** file. `start_ms` is the offset
/// between the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipOriginal {
    /// Full length of the untouched recording.
    pub duration_ms: u64,
    /// Where the delivered excerpt starts and ends inside the original.
    pub start_ms: u64,
    pub end_ms: u64,
}

/// How far along rewriting a clip is. Travels to the UI as the `clip-progress`
/// event — a cut at the start re-encodes the picture, and that takes too long
/// for a button with no sign of life.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipProgress {
    pub clip_id: String,
    /// 0 to 1.
    pub progress: f32,
}

/// An audio track as it sits in the finished MP4 file. Track 0 is the main mix,
/// followed by the sources that were given their own tracks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipTrack {
    /// Index among the file's audio tracks, not the ffmpeg stream index.
    pub index: u32,
    pub label: String,
    pub channels: u32,
    /// A file extracted for preview. Track 0 needs none — the video element
    /// plays that one anyway.
    pub preview_path: Option<String>,
}

/// How a track is weighted on export.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackMix {
    pub index: u32,
    pub gain_db: f32,
    pub muted: bool,
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
    /// Game last detected in the foreground, `None` when none is running.
    pub game: Option<String>,
}
