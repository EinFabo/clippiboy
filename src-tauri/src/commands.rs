//! Tauri commands — the UI's only interface to the core.

use std::path::Path;

use tauri::State;

use crate::audio::devices;
use crate::capture;
use crate::clips::Library;
use crate::edit;
use crate::encode;
use crate::preview;
use crate::stems;
use crate::model::{
    AppConfig, AudioDevice, AudioProcess, AudioSource, CaptureTarget, Clip, ClipEdit, ClipTrack,
    EncoderInfo, EngineStatus, StorageUsage, TrackMix, TrimOriginals,
};
use crate::state::AppState;

type Result<T> = std::result::Result<T, String>;

#[tauri::command(async)]
pub fn list_audio_devices() -> Vec<AudioDevice> {
    devices::list_devices()
}

#[tauri::command(async)]
pub fn list_audio_processes() -> Vec<AudioProcess> {
    devices::list_processes()
}

#[tauri::command(async)]
pub fn list_capture_targets() -> Vec<CaptureTarget> {
    capture::list_targets()
}

/// Async so the Media Foundation enumeration runs off the main thread. It
/// looks again, so a driver installed while the app runs shows up.
#[tauri::command(async)]
pub fn list_encoders() -> Vec<EncoderInfo> {
    encode::refresh_encoders();
    encode::list_encoders()
}

/// The most a window may put into the log in one report.
const UI_ERROR_MAX_CHARS: usize = 4000;

fn clip_ui_error(message: &str) -> String {
    message.chars().take(UI_ERROR_MAX_CHARS).collect()
}

/// A window's render or promise error, for `clippiboy.log`: a blank window
/// leaves no other trace. The text is cut, so a runaway string cannot flood it.
#[tauri::command]
pub fn ui_error(window: tauri::Window, message: String) {
    log::error!("ui {}: {}", window.label(), clip_ui_error(&message));
}

/// The settings' "listen" button: the sound at the level being set, whether or
/// not it is switched on.
#[tauri::command]
pub fn preview_sound(kind: crate::model::SoundKind, volume: f32) {
    crate::sounds::play_at(kind, volume);
}

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> AppConfig {
    state.config_snapshot()
}

#[tauri::command]
pub fn set_config(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    config: AppConfig,
) -> AppConfig {
    let previous = state.config_snapshot();
    let mut config = config;
    // A recording is one stream from one source. Switching the source would
    // restart the capture and cut it in two — so the source stays until it
    // stops.
    if state.is_recording()
        && (config.recording.target_kind != previous.recording.target_kind
            || config.recording.target_id != previous.recording.target_id
            || config.recording.target_stable_id != previous.recording.target_stable_id)
    {
        config.recording.target_kind = previous.recording.target_kind;
        config.recording.target_id = previous.recording.target_id.clone();
        config.recording.target_stable_id = previous.recording.target_stable_id.clone();
        crate::notify(&app, "error", "Stop the recording before changing the source.");
    }
    // The clip folder and the token have their own commands: the folder is
    // granted to the player recursively and the token is what every Stream
    // Deck key holds, so a settings write (a stale page, the mock config) must
    // not carry either — an empty token would kill every key too. Against the
    // config under its lock, not `previous`: a `set_clip_dir` or a new token
    // in between would be undone.
    config.accent_color = crate::model::clean_accent(config.accent_color.as_deref());
    config.accent_color_2 = crate::model::clean_accent(config.accent_color_2.as_deref());
    let mut next = state.replace_config_with(config, |next, current| {
        crate::config::keep_core_owned(next, current);
        crate::config::keep_once_flags(next, current);
    });
    if previous.friends != next.friends {
        crate::friends::config_changed(&app);
    }
    // Every window paints in the accent — the banner and the console too, which
    // never read the settings page. Told once, to all of them.
    if previous.accent() != next.accent() {
        use tauri::Emitter;
        // The icons follow once the main window has drawn the logo anew
        // (`set_logo_icon`).
        let _ = app.emit("accent-changed", next.accent());
    }
    // The corner or the screen may have changed.
    crate::overlay::reposition(&app);
    // And so may the badge. Switching it off is a switch flipped while
    // watching the corner, so it has to clear right then — not only with the
    // next recording.
    if state.is_recording() {
        crate::overlay::set_recording(&app, true);
    }
    // The registry key is read only when its switch changed.
    if previous.auto_start_with_windows != next.auto_start_with_windows {
        crate::apply_autostart(&app, next.auto_start_with_windows);
    }

    // A running capture is bound to its monitor or window, and its encoder to
    // the size, rate, quality and keyframe interval it was built with. Without
    // a restart the choice in the UI would have no effect: the buffer would
    // keep recording the old source, at the old size (CFG-7). All of these are
    // picked from lists, so a restart per change is what the user asked for.
    // Asked for even while the buffer looks off: during its start it does, and
    // the change would be lost (CFG-8). The worker decides under the lifecycle
    // lock, on what really runs, and leaves a stopped buffer stopped.
    if crate::state::capture_changed(&previous.recording, &next.recording) {
        // Against what runs: the Video page writes the fitted size on opening,
        // and that is no change worth a word.
        let running = state.active_recording.lock().clone();
        if state.is_recording() && crate::state::restart_needed(running.as_ref(), &next.recording) {
            crate::notify(&app, "ok", "The new video settings apply once the recording stops.");
        }
        // Not here: this runs on the main thread, and stopping waits for a
        // save while starting brings up the whole capture — every window
        // stood still meanwhile.
        crate::restart_buffer_for_settings(&app);
    }

    // Hotkeys normally go through `set_hotkeys`; if they do come through here
    // once, they must not just sit in the JSON and apply nowhere.
    if previous.save_clip_hotkey != next.save_clip_hotkey
        || previous.toggle_buffer_hotkey != next.toggle_buffer_hotkey
        || previous.screenshot_hotkey != next.screenshot_hotkey
        || previous.record_hotkey != next.record_hotkey
        || previous.console_hotkey != next.console_hotkey
        || previous.console_enabled != next.console_enabled
    {
        if let Err(err) = crate::register_hotkeys(&app) {
            crate::notify(&app, "error", err);
        }
    }

    // Size, screen, visibility or the glow of the console changed in the settings
    // while it happens to be open: it should not wait for the next opening.
    // `relayout` re-emits `console-opened`, and the console re-reads the config
    // on that — which is how the glow gets switched without a restart.
    if previous.console_scale != next.console_scale
        || previous.console_monitor != next.console_monitor
        || previous.console_monitor_stable_id != next.console_monitor_stable_id
        || previous.console_follow_active_screen != next.console_follow_active_screen
        || previous.console_in_capture != next.console_in_capture
        || previous.console_glow != next.console_glow
        || previous.console_style != next.console_style
    {
        crate::console::relayout(&app);
    }

    // Switched off, or moved to another port: the listener has to follow, and
    // right away — otherwise the old port keeps answering until the next start.
    if previous.control.enabled != next.control.enabled
        || previous.control.port != next.control.port
    {
        crate::control::restart(&app);
        // The restart made a new token: hand that one back, not the dead one —
        // and the revision it was written under, or the window's next write
        // would be merged against a copy the core never held.
        next = state.config_snapshot();
    }
    next
}

/// Reassign the two global hotkeys.
///
/// Kept apart from `set_config` because something can go wrong here: an
/// unusable combination, or one another program already holds. If registering
/// fails, the previous assignment applies again — otherwise the settings would
/// show a hotkey that triggers nothing.
#[tauri::command]
pub fn set_hotkeys(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    save_clip: String,
    toggle_buffer: String,
    screenshot: String,
    record: String,
    console: String,
) -> Result<AppConfig> {
    let result = apply_hotkeys(&state, &app, save_clip, toggle_buffer, screenshot, record, console);
    if result.is_err() {
        // Even after rejected input the previous hotkeys have to take hold
        // again — the settings suspend them while recording a new one.
        let _ = crate::register_hotkeys(&app);
    }
    result
}

fn apply_hotkeys(
    state: &State<'_, AppState>,
    app: &tauri::AppHandle,
    save_clip: String,
    toggle_buffer: String,
    screenshot: String,
    record: String,
    console: String,
) -> Result<AppConfig> {
    let save_clip = save_clip.trim().to_string();
    let toggle_buffer = toggle_buffer.trim().to_string();
    let screenshot = screenshot.trim().to_string();
    let record = record.trim().to_string();
    let console = console.trim().to_string();
    // Empty means unbound: that action then has no hotkey, only its buttons.
    let taken = [&save_clip, &toggle_buffer, &screenshot, &record, &console];
    for one in taken.iter().filter(|one| !one.is_empty()) {
        crate::parse_hotkey(one)?;
    }
    // Every pair, not just the first two — with five assignments the clash can
    // sit anywhere among them. Two unbound ones do not clash.
    for (at, one) in taken.iter().enumerate().filter(|(_, one)| !one.is_empty()) {
        if taken[at + 1..]
            .iter()
            .any(|other| one.eq_ignore_ascii_case(other))
        {
            return Err("Two hotkeys are on the same key combination.".into());
        }
    }

    let previous = state.config_snapshot();
    let mut config = previous.clone();
    config.save_clip_hotkey = save_clip;
    config.toggle_buffer_hotkey = toggle_buffer;
    config.screenshot_hotkey = screenshot;
    config.record_hotkey = record;
    config.console_hotkey = console;
    let next = state.replace_config(config);

    match crate::register_hotkeys(app) {
        Ok(()) => Ok(next),
        Err(err) => {
            let mut rollback = state.config_snapshot();
            rollback.save_clip_hotkey = previous.save_clip_hotkey;
            rollback.toggle_buffer_hotkey = previous.toggle_buffer_hotkey;
            rollback.screenshot_hotkey = previous.screenshot_hotkey;
            rollback.record_hotkey = previous.record_hotkey;
            rollback.console_hotkey = previous.console_hotkey;
            state.replace_config(rollback);
            let _ = crate::register_hotkeys(app);
            Err(err)
        }
    }
}

/// Suspend the global hotkeys while a new combination is being recorded in the
/// settings — otherwise pressing the old assignment would save a clip or stop
/// the buffer along the way.
#[tauri::command]
pub fn suspend_hotkeys(app: tauri::AppHandle) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    if let Err(err) = app.global_shortcut().unregister_all() {
        log::warn!("could not suspend hotkeys: {err}");
    }
}

/// Counterpart to `suspend_hotkeys` — after recording was cancelled.
#[tauri::command]
pub fn resume_hotkeys(app: tauri::AppHandle) {
    if let Err(err) = crate::register_hotkeys(&app) {
        log::warn!("{err}");
    }
}

/// Change the folder new clips land in.
///
/// It is created right away, and written to once as well — a path that only
/// blows up when the first clip is saved helps nobody.
#[tauri::command(async)]
pub fn set_clip_dir(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    dir: String,
) -> Result<AppConfig> {
    // The disk guard measures the drive the recording is on and recovery looks
    // where its files lie; both would point at the wrong place after a move.
    if state.is_recording() {
        return Err("Stop the recording before moving the clip folder.".into());
    }
    let dir = dir.trim();
    if !usable_clip_dir(dir) {
        return Err("Pick a folder for the clips, not a whole drive.".into());
    }
    let path = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&path)
        .map_err(|err| format!("Folder \"{dir}\" cannot be created: {err}"))?;
    // Checked as it really is: a junction onto a drive is a whole drive as
    // well. Kept as typed, though — the clips already filed carry that
    // spelling, and `filing` compares paths as text.
    if dunce::canonicalize(&path).is_ok_and(|real| !crate::grantable_dir(&real)) {
        return Err("Pick a folder for the clips, not a whole drive.".into());
    }
    let probe = path.join(".clippiboy-schreibtest");
    std::fs::write(&probe, b"")
        .map_err(|err| format!("ClippiBoy is not allowed to write in \"{dir}\": {err}"))?;
    let _ = std::fs::remove_file(&probe);

    let mut config = state.config_snapshot();
    config.clip_dir = path.to_string_lossy().to_string();
    let next = state.replace_config(config);
    // Without the grant the player will not play anything that lands here.
    crate::allow_clip_dir(&app, &next.clip_dir);
    Ok(next)
}

/// A clip folder that may be granted: absolute, not a whole drive, and no
/// `..` in it — `D:\clips\..` is a whole drive too.
fn usable_clip_dir(dir: &str) -> bool {
    crate::grantable_dir(Path::new(dir.trim()))
}

/// Where an export goes: the `.mp4` the user picked in the core's save dialog.
///
/// No window hands in a path any more, so a share is as good as a drive — the
/// user chose it. What stays refused is what the dialog lets through but
/// nobody means: another extension (ffmpeg would write an `.mp4` under it, and
/// a typed name without one gets `.mp4`), and ClippiBoy's own data folder, where
/// an export could land on a clip's original or a track. The clip itself is
/// refused in [`shrink`], by the file's identity rather than its spelling.
fn export_target(chosen: &Path) -> Result<std::path::PathBuf> {
    let mut path = chosen.to_path_buf();
    if path.extension().is_none() {
        path.set_extension("mp4");
    }
    let mp4 = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("mp4"));
    let up = path.components().any(|part| matches!(part, std::path::Component::ParentDir));
    if !path.is_absolute() || !mp4 || up {
        return Err("Export to a .mp4 file.".into());
    }
    // Case-insensitive, as Windows treats the folder.
    let folded = |path: &Path| -> Vec<String> {
        path.components()
            .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    };
    if folded(&path).starts_with(&folded(&crate::config::data_dir())) {
        return Err("Export somewhere outside ClippiBoy's own data folder.".into());
    }
    Ok(path)
}

/// The name the save dialog offers: the window's suggestion reduced to a bare
/// file name, without folders or characters Windows refuses in one. `None`
/// when nothing usable is left.
fn export_file_name(suggested: &str) -> Option<String> {
    let last = suggested.rsplit(['/', '\\']).next().unwrap_or_default();
    let name: String = last
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .collect();
    let name = name.trim();
    if name.trim_matches('.').is_empty() {
        return None;
    }
    Some(name.to_string())
}

/// Where ffmpeg writes an export before it is renamed into `target`: beside it,
/// so the rename stays on one volume, and under a name of its own, so a cut-short
/// run never carries the final name. The process id lets a later start tell
/// its own leftovers (`*.<digits>.clippiboy-export.mp4`, see `sweep`) from a
/// run still going; the "clippiboy" keeps them apart from the user's files.
fn export_part(target: &Path) -> std::path::PathBuf {
    let stem = target
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "export".into());
    target.with_file_name(format!("{stem}.{}.clippiboy-export.mp4", std::process::id()))
}

/// The name a manual export is offered under: `"<stem> - <n> MB.mp4"`. Not
/// the console's `"<stem> (<n> MB).mp4"`, so the two never stand for each
/// other — the console's copies live in ClippiBoy's own data folder
/// ([`discord_dir`]) and go when the clip is edited or deleted.
fn manual_export_name(clip_stem: &str, megabytes: u64) -> String {
    format!("{clip_stem} - {megabytes} MB.mp4")
}

/// Where the console's Discord copies are kept: ClippiBoy's data folder, where
/// nobody but the core writes. Beside the clip they could not be told from a
/// file of the user's under the same name.
pub(crate) fn discord_root() -> std::path::PathBuf {
    crate::config::data_dir().join("discord")
}

/// The folder of one clip's Discord copies.
fn discord_dir(root: &Path, clip_id: &str) -> std::path::PathBuf {
    root.join(crate::muxer::sanitize(clip_id))
}

/// A Discord copy of the clip: `"<stem> (<n> MB).mp4"` — the name the friend
/// sees in Discord — in the clip's own folder of copies.
fn discord_copy(root: &Path, clip_id: &str, clip_stem: &str, megabytes: u64) -> std::path::PathBuf {
    discord_dir(root, clip_id).join(format!("{clip_stem} ({megabytes} MB).mp4"))
}

/// Delete a clip's Discord copies. They are copies of what the clip was: once
/// it is edited or gone they would hand out the old picture. Only the clip's
/// folder under `root` — nothing beside the clip is ever touched.
fn remove_discord_copies(root: &Path, clip_id: &str) {
    // An empty id names the root itself, with every clip's copies in it.
    if crate::muxer::sanitize(clip_id).is_empty() {
        return;
    }
    let dir = discord_dir(root, clip_id);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => log::warn!("could not remove the Discord copies in {}: {err}", dir.display()),
    }
}

/// The suggested clip folder — the starting point of the folder dialog and the
/// target of the "Reset" button.
#[tauri::command]
pub fn default_clip_dir() -> String {
    crate::config::default_clip_dir()
        .to_string_lossy()
        .to_string()
}

/// Hand the main window a config it did not write itself. It keeps its copy
/// for the next write; without this a source muted in the console stayed
/// unmuted there, and that copy is what the mixer's own rows are built from.
/// Only to `main`, and only for changes it did not start: an echo of its own
/// write would land on its newer, not yet answered edits.
pub(crate) fn tell_main(app: &tauri::AppHandle, config: &AppConfig) {
    use tauri::Emitter;
    let _ = app.emit_to("main", "config-changed", config);
}

/// The answer of a source command, and the main window told when another
/// window (the console) sent it.
fn source_written(webview: &tauri::WebviewWindow, config: AppConfig) -> AppConfig {
    use tauri::Manager;
    if webview.label() != "main" {
        tell_main(webview.app_handle(), &config);
    }
    config
}

/// The source commands take the revision of the copy the window built its
/// row on (`AppState::upsert_source`); without one the row goes onto the
/// config as it stands.
#[tauri::command]
pub fn add_audio_source(
    state: State<'_, AppState>,
    webview: tauri::WebviewWindow,
    source: AudioSource,
    revision: Option<u64>,
) -> AppConfig {
    source_written(&webview, state.upsert_source(source, revision))
}

#[tauri::command]
pub fn update_audio_source(
    state: State<'_, AppState>,
    webview: tauri::WebviewWindow,
    source: AudioSource,
    revision: Option<u64>,
) -> AppConfig {
    source_written(&webview, state.upsert_source(source, revision))
}

#[tauri::command]
pub fn remove_audio_source(
    state: State<'_, AppState>,
    webview: tauri::WebviewWindow,
    id: String,
    revision: Option<u64>,
) -> AppConfig {
    source_written(&webview, state.remove_source(&id, revision))
}

#[tauri::command(async)]
pub fn engine_status(state: State<'_, AppState>) -> EngineStatus {
    state.status_snapshot()
}

/// Start the buffer. Runs through the same path as hotkey and tray, so the
/// button in the app raises a toast and a banner too.
///
/// `async` for the same reason as `save_clip`: bringing up the capture stack
/// takes a few hundred milliseconds.
#[tauri::command(async)]
pub fn start_buffer(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<()> {
    if state.status.lock().buffer_active {
        return Ok(());
    }
    crate::toggle_buffer_and_notify(&app);
    match state.status.lock().buffer_active {
        true => Ok(()),
        // The message is already out; the error only brings the store back to
        // the real state.
        false => Err("The replay buffer could not be started.".into()),
    }
}

/// Counterpart to `start_buffer`. `async` because stopping waits, if need be,
/// for a save in progress.
#[tauri::command(async)]
pub fn stop_buffer(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<()> {
    if state.status.lock().buffer_active {
        crate::toggle_buffer_and_notify(&app);
    }
    Ok(())
}

/// Start or stop a recording, the same way the hotkey does. `async`: stopping
/// writes the whole recording out.
#[tauri::command(async)]
pub fn toggle_recording(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<bool> {
    if state.recording_or_closing() {
        match crate::stop_recording_and_notify(&app) {
            // A second click while the first stop closed: it has stopped.
            Err(err) if err != crate::state::NOT_RECORDING => return Err(err),
            _ => {}
        }
    } else {
        crate::start_recording_and_notify(&app);
    }
    Ok(state.is_recording())
}

/// Saves the buffer. `seconds` is currently only needed by the trim idea; the
/// usual route is the shared path with hotkey and tray, which additionally
/// raises `clip-saved` and the overlay banner.
///
/// `async` because sealing and muxing take several seconds: as a synchronous
/// command that would run on the main thread and the UI would stand still all
/// the while — including the window calls the overlay banner sends there.
#[tauri::command(async)]
pub fn save_clip(
    app: tauri::AppHandle,
    seconds: Option<u32>,
) -> Result<Clip> {
    crate::save_clip_and_notify(&app, seconds)
}

/// One picture from the recording source, like the hotkey and the tray entry.
///
/// `async` because reading the picture back off the GPU and deflating the PNG
/// take a few hundred milliseconds, and the window should keep painting.
#[tauri::command(async)]
pub fn take_screenshot(app: tauri::AppHandle) -> Result<Clip> {
    crate::take_screenshot_and_notify(&app)
}

/// One step of the annotation, in the order it was drawn.
///
/// Two kinds, because they work in opposite directions: a layer covers what is
/// underneath, while a blur reads it. They cannot be merged into one image, so
/// the order between them has to survive the journey — otherwise everything
/// blurred would end up under every mark, whichever was drawn first.
#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Step {
    /// An area to make unreadable, in pixels of the **original** picture.
    Blur {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        /// How far the blur reaches. The WebView sets it, so preview and file
        /// agree.
        radius: u32,
        /// Blur an oval inside the rectangle rather than the rectangle itself.
        #[serde(default)]
        ellipse: bool,
    },
    /// A PNG with alpha, the size of the **original** picture, holding
    /// everything drawn in one go — arrows, boxes, freehand, text.
    Layer { png: Vec<u8> },
}

/// What a screenshot's editor has to know when it opens.
///
/// Two grounds, because the two halves of the editor stand on different ones:
/// marks are drawn on the picture as cropped, while cropping itself has to show
/// the whole original — otherwise a crop could only ever get smaller.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotEdit {
    pub crop: Option<crate::shot::Rect>,
    pub marks: String,
    /// The original with the crop but without the marks — what is drawn on.
    /// `None` means nothing has been done yet and the clip's own file will do.
    pub base_path: Option<String>,
    /// The untouched picture — what is cropped from.
    pub original_path: Option<String>,
    /// Its edges. Every mark and every crop is reckoned in these coordinates.
    pub original_width: u32,
    pub original_height: u32,
    /// What lies under the watermark, and where, in the original's
    /// coordinates. Set means: the finished picture gets the mark anew, and the
    /// editor shows the grounds without it and the mark where it will land.
    pub under_path: Option<String>,
    pub under_rect: Option<crate::shot::Rect>,
}

#[tauri::command]
pub fn screenshot_edit(state: State<'_, AppState>, id: String) -> Result<ShotEdit> {
    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "clip not found".to_string())?;
    let edit = crate::shot::read_edit(&id);

    let base = crate::shot::base_path(&id);
    let original = crate::shot::original_path(&id);
    let text = |path: &std::path::Path| path.to_string_lossy().to_string();

    // Untouched so far: the clip's own file is both grounds at once.
    let (original_width, original_height) = if original.is_file() {
        crate::shot::size_of_png(&original).unwrap_or((clip.width, clip.height))
    } else {
        (clip.width, clip.height)
    };
    let under_rect = crate::shot::under_rect(&id, (original_width, original_height));

    Ok(ShotEdit {
        crop: edit.crop,
        marks: edit.marks,
        // With a crop that is `base`; with marks alone the original already is
        // the picture without them.
        base_path: if base.is_file() {
            Some(text(&base))
        } else if original.is_file() {
            Some(text(&original))
        } else {
            None
        },
        original_path: original.is_file().then(|| text(&original)),
        original_width,
        original_height,
        under_rect,
        under_path: under_rect.map(|_| text(&crate::shot::under_path(&id))),
    })
}

/// Write a screenshot from its original, its marks and its crop.
///
/// **Everything** is rebuilt from the untouched picture every time, and that is
/// the whole point: crop and marks stay two things that can be taken back
/// separately. Flattening them into the file would weld them together — a mark
/// that is in the pixels cannot be peeled off again.
///
/// The order matters. The marks are reckoned in the original's coordinates, so
/// they go on first and the crop cuts through them afterwards; a note at the
/// edge is then clipped, which is what anyone would expect.
///
/// `async` because decoding, drawing and deflating take a moment on a big
/// picture.
#[tauri::command(async)]
pub fn write_screenshot(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
    crop: Option<crate::shot::Rect>,
    steps: Vec<Step>,
    marks: String,
) -> Result<Clip> {
    let clip = edit_screenshot(&state, id, crop, steps, marks)?;
    announce_change(&app);
    Ok(clip)
}

fn edit_screenshot(
    state: &State<'_, AppState>,
    id: String,
    crop: Option<crate::shot::Rect>,
    steps: Vec<Step>,
    marks: String,
) -> Result<Clip> {
    // Held until the picture and its notes are written, before the row is
    // read: a delete or a move of this screenshot finishes first, or waits —
    // the swap in `write_picture` would otherwise bring a deleted file back.
    let own = edit::clip_lock(&id);
    let _clip = own.lock();
    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "clip not found".to_string())?;
    if !clip.screenshot {
        return Err("Only a screenshot can be edited this way.".into());
    }
    keep_original(&clip)?;

    // Nothing left to do to it: the untouched picture goes back and the whole
    // store goes away, so the clip stops calling itself edited.
    //
    // The marks have their own say in that. A mark can be too small to paint
    // anything — a blur dragged two pixels wide leaves no step — and without
    // asking, that one slip would throw away the untouched picture and every
    // mark noted beside it.
    if crop.is_none() && steps.is_empty() && marks.is_empty() {
        let whole = crate::shot::read_png(&crate::shot::original_path(&clip.id))?;
        let clip = write_picture(&state, &clip, &whole)?;
        crate::shot::forget(&id);
        return Ok(clip);
    }

    let mut pristine = crate::shot::read_png(&crate::shot::original_path(&clip.id))?;
    // The original carries the watermark. Laid bare again here and stamped onto
    // the finished picture at the end — wherever its corner then is.
    let under = crate::shot::read_under(&clip.id, (pristine.width, pristine.height));
    if let Some((at, piece)) = &under {
        pristine.paste(at.x, at.y, piece);
    }

    // Clamped once, here, and from here on nobody asks for anything else. What
    // gets noted down has to be what was really cut — otherwise the note and
    // base.png describe two different pictures.
    let crop = match crop {
        Some(area) => Some(pristine.clamped(area)?),
        None => None,
    };

    // Everything that can fail happens before the first byte is written.
    //
    // The order used to be the other way round: base.png went to disk with the
    // new crop, and only then were the layers blended. A layer that would not
    // decode returned here — with the new base.png already lying there and the
    // note still holding the old crop. The next open then sized the overlay
    // from one and scaled it against the other, and every mark sat askew.
    let ground = match crop {
        Some(area) => Some(pristine.crop(area.x, area.y, area.width, area.height)?),
        None => None,
    };

    let mut picture = pristine;
    for step in &steps {
        match step {
            Step::Blur {
                x,
                y,
                width,
                height,
                radius,
                ellipse,
            } => picture.blur(*x, *y, *width, *height, *radius, *ellipse),
            Step::Layer { png } if !png.is_empty() => picture.blend(png)?,
            Step::Layer { .. } => {}
        }
    }
    if let Some(area) = crop {
        picture = picture.crop(area.x, area.y, area.width, area.height)?;
    }
    if under.is_some() {
        crate::watermark::stamp_kept(&mut picture);
    }

    // The ground to draw on next time: the crop, but none of the marks. Under
    // a temporary name until the picture itself is in place — that is the
    // write most likely to fail (the file open somewhere), and the sidecars
    // committed before it described an edit the picture did not have.
    let base = crate::shot::base_path(&clip.id);
    let base_part = base.with_extension("png.part");
    if let Some(shot) = &ground {
        shot.write_png(&base_part)?;
    }
    let written = match write_picture(&state, &clip, &picture) {
        Ok(written) => written,
        Err(err) => {
            let _ = std::fs::remove_file(&base_part);
            return Err(err);
        }
    };
    match &ground {
        Some(_) => std::fs::rename(&base_part, &base)
            .map_err(|err| format!("could not keep the picture to draw on: {err}"))?,
        None => {
            let _ = std::fs::remove_file(&base);
        }
    }
    crate::shot::write_edit(&id, &crate::shot::Edit { crop, marks })?;
    Ok(written)
}

/// Put the untouched picture aside, once.
///
/// Rescue first, write second: the other way round, a program that died in
/// between would leave nothing to come back to. A second edit adds nothing —
/// the original is the *original*, not the state before the last handle moved.
fn keep_original(clip: &Clip) -> Result<()> {
    if crate::shot::has_original(&clip.id) {
        return Ok(());
    }
    let original = crate::shot::original_path(&clip.id);
    if let Some(parent) = original.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("could not create folder: {err}"))?;
    }
    // Copied, not moved: the clip folder may sit on another drive than the
    // store, and the file has to stay where it is anyway. To a `.part` first:
    // `has_original` only asks whether the file is there, and a copy cut off
    // halfway would count as the untouched picture from then on.
    let part = original.with_extension("png.part");
    std::fs::copy(&clip.path, &part)
        .and_then(|_| std::fs::rename(&part, &original))
        .map_err(|err| {
            let _ = std::fs::remove_file(&part);
            format!("Could not save the original ({err}).")
        })
}

/// Write a picture over a screenshot's file and record what changed.
///
/// Windows will not let an open file be replaced, so the new picture goes to a
/// temporary file beside it first and takes its place from there — the same
/// route `muxer::build` takes for a clip.
fn write_picture(
    state: &State<'_, AppState>,
    clip: &Clip,
    picture: &crate::shot::Shot,
) -> Result<Clip> {
    let target = std::path::PathBuf::from(&clip.path);
    let temp = target.with_extension(format!("{}.new.png", std::process::id()));
    picture.write_png(&temp)?;
    if let Err(err) = crate::muxer::replace_file(&temp, &target) {
        let _ = std::fs::remove_file(&temp);
        return Err(err);
    }

    let size_bytes = std::fs::metadata(&target).map(|meta| meta.len()).unwrap_or(0);
    // The old thumbnail shows the picture from before this edit — with what was
    // just blurred still readable. If no new one can be made, none is better.
    let stale_thumb = match crate::thumbs::make_still(&target, &clip.id) {
        Ok(_) => false,
        Err(err) => {
            log::warn!("thumbnail not refreshed, removed instead: {err}");
            crate::thumbs::remove(&clip.id);
            true
        }
    };

    with_library(state, |lib| {
        lib.set_picture(&clip.id, picture.width, picture.height, size_bytes)
            .map_err(|e| e.to_string())?;
        if stale_thumb {
            lib.set_thumb(&clip.id, None).map_err(|e| e.to_string())?;
        }
        lib.get(&clip.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "clip not found".to_string())
    })
}

/// The picture itself on the clipboard, not its file.
///
/// The file (`copy_clip_file`) only helps where files can be dropped; a chat
/// window wants the picture. Reading it back and turning it into a bitmap takes
/// a moment on a 4K screenshot, hence `async`.
#[tauri::command(async)]
pub fn copy_clip_image(state: State<'_, AppState>, id: String) -> Result<()> {
    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "clip not found".to_string())?;
    if !clip.screenshot {
        return Err("Only a screenshot can go on the clipboard as a picture.".into());
    }
    let shot = crate::shot::read_png(std::path::Path::new(&clip.path))?;
    crate::clipboard::copy_image(&shot.dib())
}

#[tauri::command(async)]
pub fn list_clips(state: State<'_, AppState>) -> Result<Vec<Clip>> {
    with_library(&state, |lib| lib.list().map_err(|e| e.to_string()))
}

#[tauri::command(async)]
pub fn delete_clip(state: State<'_, AppState>, app: tauri::AppHandle, id: String) -> Result<()> {
    use tauri::Emitter;
    let clip_dir = state.config_snapshot().clip_dir;
    // Held to the end, before the row is even read: an edit, a move or an
    // export of this clip finishes first — a save swapping its file back in
    // after the row went left a clip nobody could see.
    let own = edit::clip_lock(&id);
    let _clip = match own.try_lock() {
        Some(held) => held,
        None => {
            // A save or an export of this clip is running; it can take a
            // while, and a delete that seems to do nothing gets clicked again.
            crate::notify(&app, "ok", "Waiting for this clip to finish saving…");
            own.lock()
        }
    };
    // The clip first, and the stores by its stored id. The stores are named
    // after a sanitised id, so an id that is empty or spelled differently
    // named the whole store — every clip's tracks and originals went with it.
    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?;
    // Already gone (deleted from another window): nothing left to clear.
    let Some(clip) = clip else {
        return Ok(());
    };
    // The file first, without the library lock — a player that holds it keeps
    // the clip, record and all, and the notice says why.
    if let Err(err) = crate::clips::remove_file_patiently(Path::new(&clip.path), 10) {
        let msg = format!(
            "Could not delete the clip ({err}). Is it open somewhere right now?"
        );
        crate::notify(&app, "error", msg.clone());
        return Err(msg);
    }
    with_library(&state, |lib| lib.delete(&clip.id).map_err(|e| e.to_string()))?;
    if let Some(thumb) = &clip.thumb_path {
        let _ = std::fs::remove_file(thumb);
    }
    let folder = std::path::PathBuf::from(&clip.path).parent().map(Path::to_path_buf);
    // The individual tracks and the original belong to the clip and serve no
    // purpose without it.
    stems::remove(&clip.id);
    edit::remove(&clip.id);
    // The thumbnail too, even when none is recorded in the database.
    crate::thumbs::remove(&clip.id);

    // Its Discord copies too — they would show what is gone.
    remove_discord_copies(&discord_root(), &clip.id);
    // If that was the last clip of its game, an empty folder would otherwise be
    // left standing.
    if let Some(folder) = folder {
        crate::filing::prune(&folder, Path::new(&clip_dir));
    }
    // Every window keeps its own list. Without this, a clip deleted in the
    // console stays in the main window and opens as "File not found".
    let _ = app.emit("clip-deleted", &id);
    crate::links::clip_deleted(&app, &id);
    announce_change(&app);
    Ok(())
}

/// Set or take away a clip's heart.
///
/// The file does **not** move by itself — `file_clip` takes care of that once
/// the clip is no longer open in the player. Moving it out from under the
/// playing video would tear the playback off.
#[tauri::command]
pub fn set_clip_favorite(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
    favorite: bool,
) -> Result<Clip> {
    let clip = with_library(&state, |lib| {
        lib.set_favorite(&id, favorite).map_err(|e| e.to_string())?;
        lib.get(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "clip not found".into())
    })?;
    announce_change(&app);
    Ok(clip)
}

/// Tell every window that clips changed. Each window keeps its own list, and
/// the console renames and hearts clips too. No clip goes along on purpose:
/// a copy taken now could arrive after a newer one, so every window reads the
/// list back from the database instead.
fn announce_change(app: &tauri::AppHandle) {
    use tauri::Emitter;
    let _ = app.emit("clips-changed", ());
}

/// Move a clip's file into the folder it belongs in.
///
/// If the move fails — usually because the file is still open — it stays where
/// it is and the clip comes back unchanged. The next start catches it up
/// (`filing::tidy`); the gallery is right in the meantime regardless, because it
/// reads from the database.
#[tauri::command(async)]
pub fn file_clip(state: State<'_, AppState>, app: tauri::AppHandle, id: String) -> Result<Clip> {
    let clip_dir = state.config_snapshot().clip_dir;
    // An edit swapping the file in at its old place, or an export reading it,
    // goes first; the row is read only after.
    let own = edit::clip_lock(&id);
    let _clip = own.lock();
    let clip = with_library(&state, |lib| {
        let clip = lib
            .get(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "clip not found".to_string())?;
        match crate::filing::place(&clip, &clip_dir) {
            Ok(Some(target)) => {
                lib.set_path(&id, &target.to_string_lossy())
                    .map_err(|e| e.to_string())?;
                lib.get(&id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "clip not found".into())
            }
            Ok(None) => Ok(clip),
            Err(err) => {
                log::warn!("Clip '{id}' stayed where it was: {err}");
                Ok(clip)
            }
        }
    })?;
    announce_change(&app);
    Ok(clip)
}

/// Take a game off every clip that has it, in one transaction, and file each
/// clip the way `file_clip` does (with no game it belongs back in the clip
/// folder). One announcement for all of it — also when one clip could not be
/// moved or its new place not written down: that is logged, the others still
/// go, and the game is off all of them either way.
///
/// Each clip moves under its own lock, one at a time, taken outside the
/// library lock (see `edit::clip_lock`); its row is read again inside, since
/// an edit or a delete may have finished while this waited.
#[tauri::command(async)]
pub fn clear_game(state: State<'_, AppState>, app: tauri::AppHandle, game: String) -> Result<()> {
    let clip_dir = state.config_snapshot().clip_dir;
    let cleared = with_library(&state, |lib| lib.clear_game(&game).map_err(|e| e.to_string()));
    // The game is off every clip now; the moves below can take a while (each
    // waits for its clip), and the lists should not show the old game meanwhile.
    if cleared.is_ok() {
        announce_change(&app);
    }
    for id in cleared.iter().flatten().map(|clip| clip.id.clone()) {
        let own = edit::clip_lock(&id);
        let _clip = own.lock();
        let filed = with_library(&state, |lib| {
            // Deleted in the meantime: nothing to move.
            let Some(clip) = lib.get(&id).map_err(|e| e.to_string())? else {
                return Ok(());
            };
            match crate::filing::place(&clip, &clip_dir) {
                Ok(Some(target)) => {
                    if let Err(err) = lib.set_path(&id, &target.to_string_lossy()) {
                        log::warn!("Clip '{id}' moved, but its new place was not saved: {err}");
                    }
                }
                Ok(None) => {}
                Err(err) => log::warn!("Clip '{id}' stayed where it was: {err}"),
            }
            Ok(())
        });
        if let Err(err) = filed {
            log::warn!("Clip '{id}' stayed where it was: {err}");
        }
    }
    announce_change(&app);
    cleared.map(|_| ())
}

#[tauri::command]
pub fn reveal_clip(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;

    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "clip not found".to_string())?;
    // Opens the folder and selects the file in it.
    app.opener()
        .reveal_item_in_dir(&clip.path)
        .map_err(|e| e.to_string())
}

/// Put a clip's video file on the clipboard.
///
/// Not the path, the **file**: in Discord or WhatsApp, Ctrl+V then attaches the
/// clip; in Explorer it drops a copy.
#[tauri::command]
pub fn copy_clip_file(state: State<'_, AppState>, id: String) -> Result<()> {
    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "clip not found".to_string())?;
    let path = std::path::PathBuf::from(&clip.path);
    if !path.is_file() {
        return Err("The clip file is no longer there.".into());
    }
    crate::clipboard::copy_files(&[path])
}

/// The logo as the main window drew it — square RGBA — for the tray and the
/// task bar. See `tray::set_logo`.
#[tauri::command]
pub fn set_logo_icon(app: tauri::AppHandle, rgba: Vec<u8>, size: u32) -> Result<()> {
    if size == 0 || size > 512 || rgba.len() != (size * size * 4) as usize {
        return Err("The logo came in the wrong size.".into());
    }
    crate::tray::set_logo(&app, tauri::image::Image::new_owned(rgba, size, size));
    Ok(())
}

/// Close the console over the game. The window asks for this itself — on
/// Escape, or a click beside the dock.
#[tauri::command]
pub fn close_console(app: tauri::AppHandle) {
    crate::console::close(&app);
}

/// What travels with "open in the app": the clip, and the second the console
/// stood at. Without the second the app would start the clip over, and the spot
/// that was worth looking at properly is exactly the one you just lost.
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FocusClip {
    id: String,
    at: f64,
}

/// Show a clip in the app itself: window to the front, gallery open, player on
/// that clip, standing where the console stood. The console closes behind it —
/// the question was "let me see this properly", and that happens in the window.
#[tauri::command]
pub fn show_clip_in_app(app: tauri::AppHandle, id: String, at: f64) -> Result<()> {
    use tauri::Emitter;

    // The window first, the console second. The console still owns the
    // foreground at this point, and that is the only state in which Windows
    // lets us hand it on — see `console::hide`.
    crate::tray::show_main_window(&app);
    crate::console::hide(&app);
    // A screenshot has no position, and a video that never started reports 0 —
    // both mean "from the top", which is what a negative or unreal number has
    // to mean as well.
    let at = if at.is_finite() && at > 0.0 { at } else { 0.0 };
    app.emit("focus-clip", FocusClip { id, at })
        .map_err(|e| e.to_string())
}

/// Shrink a clip under a size limit and put the result on the clipboard.
///
/// The console's version of the export dialog: over a game there is no save
/// dialog to point anywhere, and the next step is always the same — paste it
/// into Discord. So the file lands in ClippiBoy's own folder of copies
/// ([`discord_dir`]), named after the clip with the size added, and goes
/// straight onto the clipboard.
#[tauri::command(async)]
pub fn export_for_discord(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
    megabytes: u64,
) -> Result<String> {
    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "clip not found".to_string())?;
    refuse_still(&clip)?;
    let source = std::path::PathBuf::from(&clip.path);
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "clip".into());
    let output = discord_copy(&discord_root(), &id, &stem, megabytes);
    if let Some(folder) = output.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|err| format!("Could not make a folder for the Discord copy: {err}"))?;
    }
    let target_bytes = megabytes * 1024 * 1024;

    // Already there from an earlier run, small enough and newer than the clip:
    // copying it again beats encoding the same file twice. An older one still
    // has what a trim or a muted track has since taken out.
    // Under the clip's lock, so a save of the clip in progress finishes first
    // and its newer file counts — `shrink` takes the lock again for its run.
    let ready = {
        let own = edit::clip_lock(&id);
        let _clip = own.lock();
        std::fs::metadata(&output)
            .map(|meta| meta.len() > 0 && meta.len() <= target_bytes)
            .unwrap_or(false)
            && stems::is_newer(&output, &source)
    };
    if !ready {
        // Straight to the work: the target is the core's own, in its data
        // folder — `export_target` would rightly refuse it for the dialog.
        export_to(&state, &app, &id, target_bytes, &output)?;
    }
    crate::clipboard::copy_files(&[output.clone()])?;
    Ok(output.to_string_lossy().to_string())
}

/// Open the clip in whichever player Windows has chosen for it.
#[tauri::command]
pub fn open_clip(state: State<'_, AppState>, app: tauri::AppHandle, id: String) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;

    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "clip not found".to_string())?;
    app.opener()
        .open_path(&clip.path, None::<&str>)
        .map_err(|e| e.to_string())
}

/// Put text on the clipboard — for "Copy path" and the text field menu.
#[tauri::command]
pub fn clipboard_write_text(text: String) -> Result<()> {
    crate::clipboard::copy_text(&text)
}

/// Fetch text from the clipboard. If there is no text in it, an empty one comes
/// back — then there is simply nothing to paste.
#[tauri::command]
pub fn clipboard_read_text() -> Result<String> {
    crate::clipboard::read_text()
}

/// Change a clip's name, description and game. Empty fields clear the entry in
/// question — the gallery then shows the file name again.
#[tauri::command]
pub fn update_clip(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
    title: Option<String>,
    description: Option<String>,
    game: Option<String>,
) -> Result<Clip> {
    let trimmed = |value: Option<String>| {
        value
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    };
    let (title, description, game) = (trimmed(title), trimmed(description), trimmed(game));

    let clip = with_library(&state, |lib| {
        lib.update_meta(&id, title.as_deref(), description.as_deref(), game.as_deref())
            .map_err(|e| e.to_string())?;
        lib.get(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "clip not found".to_string())
    })?;
    announce_change(&app);
    Ok(clip)
}

/// Replace a clip's tags. The library cleans them up (case, `#`, length), and
/// the clip comes back as stored — the page then shows exactly what was kept.
#[tauri::command]
pub fn set_clip_tags(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
    tags: Vec<String>,
) -> Result<Clip> {
    let clip = with_library(&state, |lib| {
        lib.set_tags(&id, &tags).map_err(|e| e.to_string())?;
        lib.get(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "clip not found".to_string())
    })?;
    announce_change(&app);
    Ok(clip)
}

/// Picture of the audio track for the timeline. Returns the path to the PNG.
///
/// `async` because ffmpeg reads the audio through once from end to end.
#[tauri::command(async)]
pub fn clip_waveform(state: State<'_, AppState>, id: String) -> Result<String> {
    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "clip not found".to_string())?;
    refuse_still(&clip)?;
    preview::waveform(&clip).map(|path| path.to_string_lossy().to_string())
}

/// A clip's audio tracks, each with a file of its own for the preview — that is
/// the only way they can be levelled individually in the player.
///
/// `async` because catching up a clip from before the changeover takes a second
/// depending on its length, and the main thread would not paint the window all
/// that while.
#[tauri::command(async)]
pub fn clip_tracks(state: State<'_, AppState>, id: String) -> Result<Vec<ClipTrack>> {
    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "clip not found".to_string())?;
    refuse_still(&clip)?;
    // From the untouched recording if the clip is trimmed — the individual
    // tracks are always in coordinates of the original.
    stems::tracks(&clip.id, &edit::source_path(&clip), edit::audio_untouched(&clip))
}

/// Write the clip exactly as it stands in the editor: mix applied, trim
/// carried out.
///
/// The trim really does land in the file — whoever sends the clip sends the
/// trimmed one. The untouched recording moves into the originals store on the
/// way and comes back at any time via [`restore_clip_original`].
///
/// `async` because a cut at the start re-encodes the picture and that takes a
/// while depending on length; the main thread would not paint the window in the
/// meantime.
#[tauri::command(async)]
pub fn apply_clip_edit(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
    start_ms: u64,
    end_ms: u64,
    tracks: Vec<TrackMix>,
) -> Result<Clip> {
    // From before the row is read until the new state is stored: a delete or
    // a move in between would leave the row describing a file that is gone or
    // elsewhere. `edit::run` takes the same lock again (it is reentrant).
    let own = edit::clip_lock(&id);
    let _clip = own.lock();
    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "clip not found".to_string())?;
    refuse_still(&clip)?;
    let (encoder, quality) = encoder_for(&state);
    let trim = edit::Trim { start_ms, end_ms };

    let applied = edit::apply(&clip, trim, &tracks, encoder, quality, progress(&app, &id));
    let stored = store(&state, &app, &clip, applied, tracks);
    if stored.is_ok() {
        crate::telemetry::used(crate::telemetry::Feature::Editor);
    }
    stored
}

/// Undo the trim: pull the whole recording back, keep the mix.
///
/// `async` for the same reason as [`apply_clip_edit`] — even though this only
/// copies, remuxing a long recording takes its seconds.
#[tauri::command(async)]
pub fn restore_clip_original(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
) -> Result<Clip> {
    // As in `apply_clip_edit`: held across the run and `store`.
    let own = edit::clip_lock(&id);
    let _clip = own.lock();
    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "clip not found".to_string())?;
    refuse_still(&clip)?;
    let (encoder, quality) = encoder_for(&state);
    let tracks = clip.edit.as_ref().map(|e| e.tracks.clone()).unwrap_or_default();

    // If it fails, the record stays on the clip: it carries the offset the
    // individual tracks sit at. Throwing it away because the recording cannot
    // be found would leave the preview out of sync forever.
    let applied = edit::restore(&clip, &tracks, encoder, quality, progress(&app, &id));
    store(&state, &app, &clip, applied, tracks)
}

/// Throw a clip's untouched recording away and keep the trimmed file.
///
/// The one way to get back what a trim costs: until this existed, the full
/// original sat in the app data directory for good, so a trimmed clip occupied
/// *more* than an untrimmed one. Undo is gone afterwards — the trim is not.
///
/// Not `async`: this deletes two files, it does not run ffmpeg.
#[tauri::command]
pub fn discard_clip_original(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
) -> Result<Clip> {
    // On the main thread, so never waiting: a save of this clip still needs
    // the original it would throw away.
    let own = edit::clip_lock(&id);
    let Some(_clip) = own.try_lock() else {
        return Err("This clip is being saved — try again in a moment.".into());
    };
    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "clip not found".to_string())?;
    edit::discard_original(&clip.id)?;
    // The record on the clip stays: it carries the offset the individual tracks
    // sit at, and a remix without it would run against the picture. What changed
    // is `original_available`, which the library reads off the disk.
    let clip = with_library(&state, |lib| {
        lib.get(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "clip not found".to_string())
    })?;
    announce_change(&app);
    Ok(clip)
}

/// The work of an export, without its notices: shrink a clip into `output`,
/// reporting 0 to 1 along the way. Returns the bytes written. A share link
/// shrinks through here too, and says what happened in its own words.
///
/// ffmpeg writes beside `output` under a name of its own ([`export_part`]) and
/// the result is renamed in only when whole: a run cut short leaves no file
/// that looks finished, and a target the identity check below missed would be
/// replaced, not truncated while it is being read.
pub(crate) fn shrink(
    state: &State<'_, AppState>,
    id: &str,
    target_bytes: u64,
    output: &std::path::Path,
    on_progress: &impl Fn(f32),
) -> Result<u64> {
    // Held for the whole ffmpeg run, which reads the clip file throughout: a
    // save, a move or a delete of this clip waits for it, and it for them.
    let own = edit::clip_lock(id);
    let _clip = own.lock();
    let clip = with_library(state, |lib| lib.get(id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "clip not found".to_string())?;
    refuse_still(&clip)?;

    let source = std::path::PathBuf::from(&clip.path);
    if !source.is_file() {
        return Err("The clip file is no longer there.".into());
    }
    let output = output.to_path_buf();
    // By the file's identity, not its spelling: another case, a hard link or a
    // junction names the clip just as well.
    if output == source || (output.exists() && crate::disk::same_file(&output, &source)) {
        return Err("An export cannot overwrite the clip it comes from.".into());
    }
    let part = export_part(&output);

    // The length off the file, not out of the database: the arithmetic hangs on
    // it, and a row that drifted from its file would put the export over the
    // limit it exists to stay under.
    let duration_ms = crate::muxer::probe_duration_ms(&source).unwrap_or(clip.duration_ms);
    let recipe = crate::export::recipe(&crate::export::Target {
        bytes: target_bytes,
        duration_ms,
        width: clip.width,
        height: clip.height,
    });
    let (encoder, _) = encoder_for(state);
    let args = crate::export::arguments(&source, &part, &recipe, clip.height, encoder);

    match edit::run_with_progress(&args, duration_ms, on_progress) {
        Ok(()) => {}
        // The same fallback as a trim: a hardware encoder refuses often enough —
        // busy sessions, driver trouble — and x264 is always there.
        Err(err) if encoder != crate::model::EncoderId::X264 => {
            log::warn!("export with {encoder:?} failed ({err}) — retrying with x264");
            let _ = std::fs::remove_file(&part);
            let args = crate::export::arguments(
                &source,
                &part,
                &recipe,
                clip.height,
                crate::model::EncoderId::X264,
            );
            edit::run_with_progress(&args, duration_ms, on_progress).inspect_err(
                |_| {
                    let _ = std::fs::remove_file(&part);
                },
            )?;
        }
        Err(err) => {
            // A half-written file looks like a finished one in the folder.
            let _ = std::fs::remove_file(&part);
            return Err(err);
        }
    }
    let written = std::fs::metadata(&part).map(|meta| meta.len()).unwrap_or(0);
    if written == 0 {
        let _ = std::fs::remove_file(&part);
        return Err("The export came out empty.".into());
    }
    if crate::muxer::replace_file(&part, &output).is_err() {
        let _ = std::fs::remove_file(&part);
        let name = output.file_name().unwrap_or_default().to_string_lossy();
        return Err(format!("Could not put the export in place — is \"{name}\" open somewhere right now?"));
    }
    Ok(written)
}

/// Write a copy of a clip that comes in under a size.
///
/// The counterpart to everything else here: recording and trimming aim at a
/// quality and let the size follow, this aims at the size. It is what "will this
/// go through Discord" needs, and no amount of quality helps a file that is
/// refused at the door.
///
/// The original clip is not touched — this writes a second file wherever the
/// user points the save dialog. The core opens that dialog itself: a window
/// only suggests a name, so no script in it can choose where a file is written,
/// and whatever the user picks — a network share too — is fine. Returns the
/// path written, `None` when the user cancelled.
#[tauri::command(async)]
pub fn export_clip(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
    target_bytes: u64,
    suggested_name: String,
) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;

    let chosen = (|| -> Result<Option<std::path::PathBuf>> {
        let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
            .ok_or_else(|| "clip not found".to_string())?;
        refuse_still(&clip)?;
        let name = export_file_name(&suggested_name).unwrap_or_else(|| {
            let stem = Path::new(&clip.path)
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_else(|| "clip".into());
            manual_export_name(&stem, target_bytes / 1024 / 1024)
        });
        // Off the main thread (an async command), which the dialog needs: it
        // is shown there, and this waits for the answer.
        let Some(picked) = app
            .dialog()
            .file()
            .set_parent(&window)
            .set_title("Save the export as")
            .set_file_name(name)
            .add_filter("Video", &["mp4"])
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let picked = picked.into_path().map_err(|err| err.to_string())?;
        let output = export_target(&picked)?;
        // The dialog asked before replacing what was picked, not the name
        // with `.mp4` added.
        if output != picked && output.exists() {
            let name = output.file_name().unwrap_or_default().to_string_lossy();
            return Err(format!("\"{name}\" already exists — pick another name."));
        }
        Ok(Some(output))
    })()
    // Said here, once: the dialog only goes back to ready and relies on this
    // notice. Cancelling is no failure and says nothing.
    .inspect_err(|err| crate::notify(&app, "error", err.clone()))?;
    let Some(output) = chosen else {
        return Ok(None);
    };
    export_to(&state, &app, &id, target_bytes, &output)?;
    Ok(Some(output.to_string_lossy().to_string()))
}

/// Shrink a clip into `output` and say how it went — the part of an export
/// that comes after the target is settled. Every failure is raised as a
/// notice here, once.
fn export_to(
    state: &State<'_, AppState>,
    app: &tauri::AppHandle,
    id: &str,
    target_bytes: u64,
    output: &Path,
) -> Result<()> {
    // The target is settled: the window can show the copy being written.
    progress(app, id)(0.0);
    let written = shrink(state, id, target_bytes, output, &progress(app, id))
        .inspect_err(|err| crate::notify(app, "error", err.clone()))?;
    progress(app, id)(1.0);
    if written > target_bytes {
        // Not an error: the file is there and usually only just over. Saying so
        // beats letting it be refused somewhere else without explanation.
        crate::notify(
            app,
            "ok",
            format!(
                "Exported at {} MB — a little over the {} MB asked for.",
                written / 1024 / 1024,
                target_bytes / 1024 / 1024
            ),
        );
    } else {
        crate::notify(
            app,
            "ok",
            format!("Exported · {} MB", written.div_ceil(1024 * 1024)),
        );
    }
    Ok(())
}

/// What ClippiBoy occupies: the clips themselves, and what grows out of sight
/// beside them — originals, individual tracks, thumbnails, the web views' cache.
///
/// The clips used to be left out, on the grounds that they lie plainly visible
/// in the user's own folder. But whoever opens "Storage" wants the whole of it,
/// and a total of 400 MB beside 11 GB of clips read as simply wrong.
///
/// The clips are counted file by file from the library rather than by walking
/// the clip folder: the folder can have changed, and older clips stay where
/// they were saved.
#[tauri::command(async)]
pub fn storage_usage(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<StorageUsage> {
    use tauri::Manager;
    let clips = with_library(&state, |lib| lib.list().map_err(|e| e.to_string())).unwrap_or_default();
    let clips_bytes = clips
        .iter()
        .filter_map(|clip| std::fs::metadata(&clip.path).ok())
        .map(|meta| meta.len())
        .sum();
    let local = app.path().app_local_data_dir().ok();
    // ffmpeg lives in the same folder as the web views' cache (`tools.rs`), but
    // unlike the cache it does not come and go — it gets a row of its own.
    let tools_bytes = local.as_ref().map(|dir| dir_bytes(&dir.join("ffmpeg"))).unwrap_or(0);
    let cache_bytes = local.as_ref().map(|dir| dir_bytes(dir)).unwrap_or(0).saturating_sub(tools_bytes);
    // One store for both: a trimmed clip leaves its `video.mp4` there, a
    // screenshot its pictures. Only the first is what the row and "Clear all
    // trims" speak of.
    let store_bytes = dir_bytes(&edit::root());
    let originals_bytes = edit::trim_originals(&Default::default()).bytes;
    Ok(StorageUsage {
        clips_bytes,
        originals_bytes,
        shot_edits_bytes: store_bytes.saturating_sub(originals_bytes),
        tracks_bytes: dir_bytes(&crate::stems::root()),
        thumbs_bytes: dir_bytes(&crate::thumbs::dir()),
        cache_bytes,
        tools_bytes,
    })
}

/// What "Clear all trims" in the storage settings would free.
#[tauri::command(async)]
pub fn trim_originals(state: State<'_, AppState>) -> Result<TrimOriginals> {
    Ok(edit::trim_originals(&originals_still_needed(&state)?))
}

/// Throw away the untouched recording of every trimmed clip. The trims stay;
/// only undoing them is gone. Returns what was freed.
#[tauri::command(async)]
pub fn discard_all_originals(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<TrimOriginals> {
    let freed = edit::discard_all_originals(&originals_still_needed(&state)?);
    announce_change(&app);
    Ok(freed)
}

/// The store folders of clips whose file is missing — a crash between moving
/// the original aside and putting the trimmed file in its place. There the
/// original is the only copy. Without the library there is no telling which
/// those are, so nothing is cleared at all.
fn originals_still_needed(state: &State<'_, AppState>) -> Result<std::collections::HashSet<String>> {
    let clips = with_library(state, |lib| lib.list().map_err(|e| e.to_string()))?;
    Ok(clips
        .iter()
        .filter(|clip| !clip.screenshot && !Path::new(&clip.path).is_file())
        .map(|clip| edit::folder_name(&clip.id))
        .collect())
}

/// Everything below a folder, in bytes. Missing folders count as nothing.
///
/// Hand-rolled rather than pulled in: the project carries no directory-walking
/// dependency. Recursive to any depth — the web views' cache runs to thousands
/// of files, which is why the storage page does not wait on this to show the
/// rest.
fn dir_bytes(root: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.metadata() {
            Ok(meta) if meta.is_dir() => dir_bytes(&entry.path()),
            Ok(meta) => meta.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Encoder and quality for a re-encode run, taken from the settings.
///
/// Quality, not bitrate: a cut at the front re-encodes from the original, and
/// aiming at the bitrate it was recorded with produced the same size at one
/// generation less quality. The same number the recording itself aims at.
fn encoder_for(state: &State<'_, AppState>) -> (crate::model::EncoderId, u32) {
    let recording = state.config_snapshot().recording;
    (encode::effective(recording.encoder), recording.quality)
}

/// Report progress to the UI. ffmpeg reports often enough that a bar moves
/// visibly.
fn progress<'a>(app: &'a tauri::AppHandle, id: &'a str) -> impl Fn(f32) + 'a {
    use tauri::Emitter;
    move |value| {
        let _ = app.emit(
            "clip-progress",
            crate::model::ClipProgress {
                clip_id: id.to_string(),
                progress: value,
            },
        );
    }
}

/// Write a run's result into the database and read the clip back.
fn store(
    state: &State<'_, AppState>,
    app: &tauri::AppHandle,
    clip: &Clip,
    applied: Result<edit::Applied>,
    tracks: Vec<TrackMix>,
) -> Result<Clip> {
    let applied = match applied {
        Ok(applied) => applied,
        Err(err) => {
            crate::notify(app, "error", err.clone());
            return Err(err);
        }
    };

    // Baked into the file, the levels start over from there.
    let tracks = if applied.mix_baked {
        tracks
            .into_iter()
            .map(|track| TrackMix {
                gain_db: 0.0,
                muted: false,
                ..track
            })
            .collect()
    } else {
        tracks
    };
    // The trim now sits in the file — what is stored here is the full range of
    // the new file. Where that sits in the original is recorded alongside.
    let edit = ClipEdit {
        start_ms: 0,
        end_ms: applied.duration_ms,
        tracks,
        // Once in the file, it stays there: zero levels must not read as
        // "as recorded" afterwards (`edit::audio_untouched`).
        baked: applied.mix_baked || clip.edit.as_ref().is_some_and(|edit| edit.baked),
    };
    let stored = with_library(state, |lib| {
        lib.set_edit(&clip.id, Some(&edit)).map_err(|e| e.to_string())?;
        lib.set_original(&clip.id, applied.original.as_ref())
            .map_err(|e| e.to_string())?;
        lib.set_file_state(&clip.id, applied.duration_ms, applied.size_bytes)
            .map_err(|e| e.to_string())?;
        lib.get(&clip.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "clip not found".to_string())
    })?;
    // The clip's Discord copies show what it was before. An undo puts back a
    // file with the original's old time, so the age check in
    // `export_for_discord` alone would let them through.
    remove_discord_copies(&discord_root(), &clip.id);
    announce_change(app);
    Ok(stored)
}

/// Show any file in Explorer.
#[tauri::command]
pub fn reveal_path(app: tauri::AppHandle, path: String) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;

    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| e.to_string())
}

/// Turn away everything that only a recording has.
///
/// Trimming, the individual tracks and the waveform all run through ffmpeg and
/// expect a stream with a duration. Without this the caller would get an ffmpeg
/// error nobody can do anything with, instead of the plain reason.
fn refuse_still(clip: &Clip) -> Result<()> {
    if clip.screenshot {
        return Err("A screenshot has no sound and no length.".into());
    }
    Ok(())
}

fn with_library<T>(
    state: &State<'_, AppState>,
    f: impl FnOnce(&Library) -> Result<T>,
) -> Result<T> {
    let guard = state.library.lock();
    match guard.as_ref() {
        Some(lib) => f(lib),
        None => Err("clip database is not available".into()),
    }
}

/// Throw the control port's token away and generate a new one.
///
/// For the moment somebody has handed the token on and wants it to stop working
/// — the port restarts, and the plugin on this machine picks the new one up out
/// of the handshake file by itself.
#[tauri::command]
pub fn regenerate_control_token(state: State<'_, AppState>, app: tauri::AppHandle) -> AppConfig {
    crate::control::restart(&app);
    state.config_snapshot()
}

/// Version from `tauri.conf.json` — the settings display it.
#[tauri::command]
pub fn app_version(app: tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Look for an update. `None` means already up to date.
/// The update found by the hourly check, if any — asked for when the window
/// starts, because an event sent while it was hidden has nobody listening.
#[tauri::command]
pub fn pending_update(app: tauri::AppHandle) -> Option<crate::updater::UpdateInfo> {
    crate::updater::pending(&app)
}

#[tauri::command]
pub async fn check_update(app: tauri::AppHandle) -> Result<Option<crate::updater::UpdateInfo>> {
    crate::updater::check(&app).await
}

/// Install the update that was found. Quits the app and starts the installer.
#[tauri::command]
pub async fn install_update(app: tauri::AppHandle) -> Result<()> {
    crate::updater::install(&app).await
}

/// Where ffmpeg stands — asked for when the window starts, the changes come
/// as `ffmpeg-status`.
#[tauri::command]
pub fn ffmpeg_status() -> crate::tools::Status {
    crate::tools::status()
}

/// Try the ffmpeg download again right away instead of after the wait.
#[tauri::command]
pub fn retry_ffmpeg() {
    crate::tools::retry()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_export_goes_to_an_mp4_outside_clippiboys_data() {
        let local = if cfg!(windows) { r"C:\Users\me\Videos\clip.mp4" } else { "/home/me/clip.mp4" };
        let target = |path: &str| export_target(Path::new(path));
        assert_eq!(target(local).unwrap(), Path::new(local));
        assert!(target("clip.mp4").is_err(), "relative");
        if cfg!(windows) {
            // Picked in the core's own dialog, so a share is the user's choice.
            assert_eq!(
                target(r"\\server\share\clip.mp4").unwrap(),
                Path::new(r"\\server\share\clip.mp4"),
                "UNC"
            );
        }
        assert!(target(&local.replace(".mp4", ".exe")).is_err(), "another extension");
        let upper = local.replace(".mp4", ".MP4");
        assert_eq!(target(&upper).unwrap(), Path::new(&upper), "the extension in any case");
        assert_eq!(
            target(&local.replace(".mp4", "")).unwrap(),
            Path::new(local),
            "no extension gets one"
        );
        let own = crate::config::data_dir().join("originals").join("x").join("video.mp4");
        assert!(target(&own.to_string_lossy()).is_err(), "ClippiBoy's own data");
        let shouted = own.to_string_lossy().to_uppercase().replace(".MP4", ".mp4");
        assert!(target(&shouted).is_err(), "ClippiBoy's own data in another case");
        let around = crate::config::data_dir()
            .join("..")
            .join(crate::config::data_dir().file_name().unwrap())
            .join("video.mp4");
        assert!(target(&around.to_string_lossy()).is_err(), "a way round through ..");
    }

    #[test]
    fn the_suggested_name_is_only_a_file_name() {
        assert_eq!(export_file_name("Valorant (10 MB).mp4").as_deref(), Some("Valorant (10 MB).mp4"));
        assert_eq!(export_file_name(r"C:\Windows\evil.mp4").as_deref(), Some("evil.mp4"));
        assert_eq!(export_file_name("../../up/there.mp4").as_deref(), Some("there.mp4"));
        assert_eq!(export_file_name(r"\\server\share\x.mp4").as_deref(), Some("x.mp4"));
        assert_eq!(export_file_name("C:x.mp4").as_deref(), Some("Cx.mp4"));
        assert_eq!(export_file_name("  "), None);
        assert_eq!(export_file_name("a/"), None);
        assert_eq!(export_file_name(".."), None);
    }

    #[test]
    fn an_export_is_written_beside_its_target_under_a_name_of_its_own() {
        let target = Path::new("dir").join("Valorant (10 MB).mp4");
        let part = export_part(&target);
        assert_eq!(part.parent(), target.parent());
        assert_eq!(
            part.file_name().unwrap().to_string_lossy(),
            format!("Valorant (10 MB).{}.clippiboy-export.mp4", std::process::id())
        );
        assert!(crate::sweep::is_scratch_name(&part.file_name().unwrap().to_string_lossy()));
    }

    #[test]
    fn a_discord_copy_lives_in_its_clips_folder_of_copies() {
        let root = Path::new("data").join("discord");
        let id = "1b4e28ba-2fa1-11d2-883f-0016d3cca427";
        let folder = root.join("1b4e28ba_2fa1_11d2_883f_0016d3cca427");
        assert_eq!(discord_dir(&root, id), folder);
        assert_eq!(
            discord_copy(&root, id, "Valorant_2026-10-22_12-00-00-123", 10),
            folder.join("Valorant_2026-10-22_12-00-00-123 (10 MB).mp4"),
            "the name the friend sees in Discord"
        );
        assert!(discord_dir(&root, "../../up").starts_with(&root));
        assert!(discord_root().starts_with(crate::config::data_dir()));
    }

    #[test]
    fn the_cleanup_takes_the_clips_copies_and_leaves_files_beside_the_clip_alone() {
        let dir = std::env::temp_dir().join(format!("clippiboy-discord-{}", uuid::Uuid::new_v4()));
        let root = dir.join("discord");
        let clips = dir.join("clips");
        std::fs::create_dir_all(&clips).unwrap();
        let (id, other) = ("clip-1", "clip-2");
        let copy = discord_copy(&root, id, "Valorant", 10);
        let kept = discord_copy(&root, other, "Valorant", 10);
        for file in [&copy, &kept] {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, b"x").unwrap();
        }
        let beside = clips.join("Valorant (10 MB).mp4");
        std::fs::write(clips.join("Valorant.mp4"), b"x").unwrap();
        std::fs::write(&beside, b"x").unwrap();

        remove_discord_copies(&root, id);
        assert!(!discord_dir(&root, id).exists(), "the clip's copies are gone");
        assert!(kept.is_file(), "another clip's copies stay");
        assert!(beside.is_file(), "a file of the user's beside the clip");
        remove_discord_copies(&root, id);
        remove_discord_copies(&root, "");
        assert!(kept.is_file(), "an empty id is not the whole folder");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_manual_export_is_never_named_like_a_discord_copy() {
        let stem = "Valorant_2026-10-22_12-00-00-123";
        let name = manual_export_name(stem, 10);
        assert_eq!(name, "Valorant_2026-10-22_12-00-00-123 - 10 MB.mp4");
        assert_ne!(
            Some(name.as_str()),
            discord_copy(Path::new("d"), "id", stem, 10).file_name().and_then(|n| n.to_str())
        );
    }

    #[test]
    fn a_whole_drive_is_no_clip_folder() {
        let (root, folder) = if cfg!(windows) { (r"C:\", r"C:\Videos\ClippiBoy") } else { ("/", "/home/me/clips") };
        assert!(usable_clip_dir(folder));
        assert!(!usable_clip_dir(root));
        assert!(!usable_clip_dir(""));
        assert!(!usable_clip_dir("clips"));
        let climbs: &[&str] = if cfg!(windows) { &[r"C:\..", r"D:\x\..\.."] } else { &["/home/..", "/home/x/../.."] };
        for dir in climbs {
            assert!(!usable_clip_dir(dir), "{dir}");
        }
    }

    #[test]
    fn a_way_up_out_of_the_clip_folder_is_no_clip_folder() {
        let dirs: &[&str] = if cfg!(windows) {
            &[r"D:\clips\..", r"D:\clips\.\..", r"D:\clips\sub\..\..", r"\\server\share\a\.."]
        } else {
            &["/home/me/clips/..", "/home/me/clips/./..", "/home/me/clips/sub/../.."]
        };
        for dir in dirs {
            assert!(!usable_clip_dir(dir), "{dir}");
        }
        assert!(usable_clip_dir(if cfg!(windows) { r"D:\clips" } else { "/home/me/clips" }));
    }

    #[test]
    fn ui_error_text_is_cut() {
        let long = "é".repeat(UI_ERROR_MAX_CHARS * 3);
        assert_eq!(clip_ui_error(&long).chars().count(), UI_ERROR_MAX_CHARS);
        assert_eq!(clip_ui_error("short"), "short");
    }
}
