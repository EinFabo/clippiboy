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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SourceKind {
    /// The audio of whichever game is detected right now. Deliberately without
    /// a PID: a stored one is dead after the first restart of the game.
    /// [`crate::audio::resolve`] puts the current one in.
    Game,
    /// Only ever **read**, never written: one version stored the leftovers as a
    /// kind of their own. `config::migrate_sources` turns it back into a plain
    /// output device. Removing it here would make those configurations
    /// unreadable — and an unreadable one is thrown away whole.
    Leftovers,
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

/// How the encoder decides how many bits a frame is worth.
///
/// Which one is really in force is not a matter of asking: a Media Foundation
/// transform accepts a mode it then quietly ignores. `mft::configure` therefore
/// reads it back and falls down this list until one sticks — and the answer
/// belongs in the status display, because it decides how big clips get.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RateControl {
    /// Constant quality: the bitrate follows the picture. A menu screen costs a
    /// fraction of a firefight. What ClippiBoy asks for.
    Quality,
    /// A bitrate target with a peak the encoder actually honours.
    PeakConstrainedVbr,
    /// A bitrate target and no ceiling at all — the transform's own default, and
    /// the last resort.
    UnconstrainedVbr,
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
    /// Only a fallback now: with constant quality the encoder ignores it (MSDN is
    /// explicit that `AVEncCommonMeanBitRate` does not apply in quality mode).
    /// It still decides the memory budget and still governs if the encoder
    /// refuses quality mode.
    pub bitrate_kbps: u32,
    /// What the picture is worth, 1 (smallest) to 100 (best). The encoder's own
    /// default is 70, which it maps to a quantiser of 24.
    #[serde(default = "default_quality")]
    pub quality: u32,
    pub encoder: EncoderId,
    pub keyframe_seconds: u32,
}

/// The encoder's own default. A config written before this setting existed gets
/// it, so the first run after an update is the encoder at its normal quality
/// rather than at some number nobody chose.
pub fn default_quality() -> u32 {
    70
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BufferConfig {
    /// Switch the buffer on by itself — at ClippiBoy's start, or only once a
    /// game is in the foreground (see `only_buffer_in_game`).
    #[serde(default)]
    pub auto_start: bool,
    pub seconds: u32,
    /// How much of the buffer a save actually writes. `0` means "all of it",
    /// which is what every version up to here did — so an existing config keeps
    /// behaving exactly as before without a migration.
    ///
    /// The two are deliberately separate: a long buffer is insurance against
    /// pressing the key late, and that is a different question from how long the
    /// finished clip should be. Tying them together meant a 2-minute buffer could
    /// only ever produce 2-minute clips.
    #[serde(default)]
    pub clip_seconds: u32,
    /// Ceiling on what the packet ring may occupy, in megabytes. `0` derives it
    /// from bitrate and buffer length.
    ///
    /// It exists because the encoder now aims at a quality rather than a
    /// bitrate: how many bytes a minute costs depends on what is on screen, so
    /// the length alone no longer bounds the memory. The UI has always shown an
    /// "at most X MB" line here — this is what finally makes it true.
    #[serde(default)]
    pub memory_mb: u32,
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
    #[serde(default = "yes")]
    pub on_screenshot: bool,
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
            on_screenshot: true,
            corner: OverlayCorner::BottomRight,
            duration_ms: 3500,
            monitor: None,
            follow_active_screen: false,
        }
    }
}

/// The local control port a Stream Deck — or anything else on this machine —
/// presses the same buttons through that hotkey and tray already press.
///
/// It listens on `127.0.0.1` only and every request has to carry the token, so
/// nothing reaches it from outside this machine. The token is generated once on
/// first start; `port` is only in here because 47653 might already belong to
/// something else.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlConfig {
    pub enabled: bool,
    pub port: u16,
    /// Empty means "not generated yet" — `control::start` fills it in.
    #[serde(default)]
    pub token: String,
}

/// Nothing well-known, and above the range Windows hands out for outgoing
/// connections by itself.
pub const DEFAULT_CONTROL_PORT: u16 = 47653;

impl Default for ControlConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: DEFAULT_CONTROL_PORT,
            token: String::new(),
        }
    }
}

/// S saves, B is the buffer, P is the picture.
pub fn default_screenshot_hotkey() -> String {
    "Ctrl+Shift+P".into()
}

/// Serde needs a function even for a plain `true`.
fn yes() -> bool {
    true
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
    /// Came later than the other two, so it needs its own default — see the
    /// note below on why a missing default costs the whole config.
    #[serde(default = "default_screenshot_hotkey")]
    pub screenshot_hotkey: String,
    pub auto_start_with_windows: bool,
    pub only_buffer_in_game: bool,
    // Newly added fields need `default` — otherwise `config::load()` throws away
    // the entire saved config on update (audio sources included).
    #[serde(default)]
    pub overlay: OverlayConfig,
    /// Has it already been explained that closing leaves the app in the tray?
    #[serde(default)]
    pub tray_hint_shown: bool,
    /// The local port for the Stream Deck plugin — see [`ControlConfig`].
    #[serde(default)]
    pub control: ControlConfig,
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
    /// Where the delivered excerpt sits inside the untouched recording.
    ///
    /// This record **outlives the file**. The individual tracks are stored in
    /// coordinates of the original, so the offset here is what keeps a later
    /// remix in sync — `edit::repair` deliberately keeps it when the recording
    /// itself has gone, and so does discarding one by hand. Whether undo is
    /// still possible is therefore a different question: [`Self::original_available`].
    #[serde(default)]
    pub original: Option<ClipOriginal>,
    /// Is the untouched recording still on disk, so the trim can be undone?
    ///
    /// Derived when the clip is read, not stored — the file can go without the
    /// database hearing about it. Until this existed the editor offered "undo
    /// trim" whenever [`Self::original`] stood, and then failed on a file that
    /// was no longer there.
    #[serde(default)]
    pub original_available: bool,
    /// A still instead of a recording. Everything to do with time — trimming,
    /// the tracks, the waveform — does not apply to it.
    #[serde(default)]
    pub screenshot: bool,
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

/// An audio track as it sits in the finished MP4 file: the main mix first, then
/// the sources that were given their own tracks. A main mix with nothing in it —
/// every source on its own track — is left out on save and is missing here too.
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

/// What ClippiBoy keeps out of sight in the app data directory.
///
/// The clips themselves are not in here — those lie in the user's own folder,
/// where they can be seen. These three grow quietly: an original per trimmed
/// clip, the individual tracks per clip with more than one source, a thumbnail
/// each.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageUsage {
    pub originals_bytes: u64,
    pub tracks_bytes: u64,
    pub thumbs_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub buffer_active: bool,
    pub buffered_seconds: f32,
    pub buffer_bytes: u64,
    pub dropped_frames: u64,
    pub encoder: Option<EncoderId>,
    /// Which rate control the encoder really accepted — see [`RateControl`].
    #[serde(default)]
    pub rate_control: Option<RateControl>,
    pub fps: f32,
    /// Game last detected in the foreground, `None` when none is running.
    pub game: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `config::load` throws the **whole** configuration away when it cannot be
    /// parsed — sources, hotkeys, clip directory. A file written before
    /// `excludeGame` existed therefore has to keep loading, or the feature costs
    /// everyone their setup on first start.
    #[test]
    fn a_source_written_before_this_change_still_loads() {
        let old = r#"{
            "id": "src-1",
            "label": "Headphones",
            "kind": { "type": "outputDevice", "deviceId": "dev-1" },
            "enabled": true,
            "gainDb": 0.0,
            "muted": false,
            "solo": false,
            "separateTrack": false
        }"#;
        let parsed: AudioSource = serde_json::from_str(old).expect("old source no longer loads");
        assert_eq!(
            parsed.kind,
            SourceKind::OutputDevice {
                device_id: "dev-1".into(),
            }
        );
    }

    /// The wire format the mixer in `src/lib/types.ts` is written against.
    #[test]
    fn the_source_kinds_are_tagged_the_way_the_ui_expects() {
        let json = |kind: &SourceKind| serde_json::to_string(kind).unwrap();
        assert_eq!(json(&SourceKind::Game), r#"{"type":"game"}"#);
        assert_eq!(
            json(&SourceKind::OutputDevice {
                device_id: "d".into(),
            }),
            r#"{"type":"outputDevice","deviceId":"d"}"#
        );

        // Written by one intermediate version, still has to load — see
        // `config::migrate_sources`.
        let back: SourceKind = serde_json::from_str(r#"{"type":"leftovers"}"#).unwrap();
        assert_eq!(back, SourceKind::Leftovers);
    }
}
