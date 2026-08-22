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
    EncoderInfo, EngineStatus, TrackMix,
};
use crate::state::AppState;

type Result<T> = std::result::Result<T, String>;

#[tauri::command]
pub fn list_audio_devices() -> Vec<AudioDevice> {
    devices::list_devices()
}

#[tauri::command]
pub fn list_audio_processes() -> Vec<AudioProcess> {
    devices::list_processes()
}

#[tauri::command]
pub fn list_capture_targets() -> Vec<CaptureTarget> {
    capture::list_targets()
}

#[tauri::command]
pub fn list_encoders() -> Vec<EncoderInfo> {
    encode::list_encoders()
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
    let next = state.replace_config(config);
    // Otherwise the player cannot reach clips outside the Videos folder.
    crate::allow_clip_dir(&app, &next.clip_dir);
    // The corner or the screen may have changed.
    crate::overlay::reposition(&app);
    crate::apply_autostart(&app, next.auto_start_with_windows);

    // A running capture is bound to its monitor or window. Without a restart
    // the selection in the UI would have no effect: the buffer would keep
    // recording the old source. Only on a real source change — resolution and
    // bitrate are dragged on a slider, and a restart per mouse move would be
    // fatal there.
    let target_changed = next.recording.target_kind != previous.recording.target_kind
        || next.recording.target_id != previous.recording.target_id;
    if target_changed && state.is_buffering() {
        log::info!("capture source changed — restarting the buffer");
        state.stop_pipeline();
        crate::start_buffer_and_notify(&app);
    }

    // Hotkeys normally go through `set_hotkeys`; if they do come through here
    // once, they must not just sit in the JSON and apply nowhere.
    if previous.save_clip_hotkey != next.save_clip_hotkey
        || previous.toggle_buffer_hotkey != next.toggle_buffer_hotkey
        || previous.screenshot_hotkey != next.screenshot_hotkey
    {
        if let Err(err) = crate::register_hotkeys(&app) {
            crate::notify(&app, "error", err);
        }
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
) -> Result<AppConfig> {
    let result = apply_hotkeys(&state, &app, save_clip, toggle_buffer, screenshot);
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
) -> Result<AppConfig> {
    let save_clip = save_clip.trim().to_string();
    let toggle_buffer = toggle_buffer.trim().to_string();
    let screenshot = screenshot.trim().to_string();
    crate::parse_hotkey(&save_clip)?;
    crate::parse_hotkey(&toggle_buffer)?;
    crate::parse_hotkey(&screenshot)?;
    // Every pair, not just the first two — with three assignments the clash can
    // sit anywhere among them.
    let taken = [&save_clip, &toggle_buffer, &screenshot];
    for (at, one) in taken.iter().enumerate() {
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
    let next = state.replace_config(config);

    match crate::register_hotkeys(app) {
        Ok(()) => Ok(next),
        Err(err) => {
            let mut rollback = state.config_snapshot();
            rollback.save_clip_hotkey = previous.save_clip_hotkey;
            rollback.toggle_buffer_hotkey = previous.toggle_buffer_hotkey;
            rollback.screenshot_hotkey = previous.screenshot_hotkey;
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
#[tauri::command]
pub fn set_clip_dir(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    dir: String,
) -> Result<AppConfig> {
    let dir = dir.trim();
    if dir.is_empty() {
        return Err("No folder selected.".into());
    }
    let path = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&path)
        .map_err(|err| format!("Folder \"{dir}\" cannot be created: {err}"))?;
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

/// The suggested clip folder — the starting point of the folder dialog and the
/// target of the "Reset" button.
#[tauri::command]
pub fn default_clip_dir() -> String {
    crate::config::default_clip_dir()
        .to_string_lossy()
        .to_string()
}

#[tauri::command]
pub fn add_audio_source(state: State<'_, AppState>, source: AudioSource) -> AppConfig {
    state.upsert_source(source)
}

#[tauri::command]
pub fn update_audio_source(state: State<'_, AppState>, source: AudioSource) -> AppConfig {
    state.upsert_source(source)
}

#[tauri::command]
pub fn remove_audio_source(state: State<'_, AppState>, id: String) -> AppConfig {
    state.remove_source(&id)
}

#[tauri::command]
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

/// Saves the buffer. `seconds` is currently only needed by the trim idea; the
/// usual route is the shared path with hotkey and tray, which additionally
/// raises `clip-saved` and the overlay banner.
///
/// `async` because sealing and muxing take several seconds: as a synchronous
/// command that would run on the main thread and the UI would stand still all
/// the while — including the window calls the overlay banner sends there.
#[tauri::command(async)]
pub fn save_clip(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    seconds: Option<u32>,
) -> Result<Clip> {
    match seconds {
        None => crate::save_clip_and_notify(&app),
        Some(seconds) => {
            let clip = state.save_clip(Some(seconds))?;
            with_library(&state, |lib| lib.insert(&clip).map_err(|e| e.to_string()))?;
            Ok(clip)
        }
    }
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
    id: String,
    crop: Option<crate::shot::Rect>,
    steps: Vec<Step>,
    marks: String,
) -> Result<Clip> {
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

    let pristine = crate::shot::read_png(&crate::shot::original_path(&clip.id))?;

    // The ground to draw on next time: the crop, but none of the marks.
    if let Some(area) = crop {
        pristine
            .crop(area.x, area.y, area.width, area.height)?
            .write_png(&crate::shot::base_path(&clip.id))?;
    } else {
        let _ = std::fs::remove_file(crate::shot::base_path(&clip.id));
    }

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

    crate::shot::write_edit(&id, &crate::shot::Edit { crop, marks })?;
    write_picture(&state, &clip, &picture)
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
    // store, and the file has to stay where it is anyway.
    std::fs::copy(&clip.path, &original)
        .map_err(|err| format!("Could not save the original ({err}).").into())
        .map(|_| ())
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
    if let Err(err) = crate::thumbs::make_still(&target, &clip.id) {
        log::warn!("thumbnail not refreshed: {err}");
    }

    with_library(state, |lib| {
        lib.set_picture(&clip.id, picture.width, picture.height, size_bytes)
            .map_err(|e| e.to_string())?;
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

#[tauri::command]
pub fn list_clips(state: State<'_, AppState>) -> Result<Vec<Clip>> {
    with_library(&state, |lib| lib.list().map_err(|e| e.to_string()))
}

#[tauri::command]
pub fn delete_clip(state: State<'_, AppState>, id: String) -> Result<()> {
    // The individual tracks and the original belong to the clip and serve no
    // purpose without it.
    stems::remove(&id);
    edit::remove(&id);
    // The thumbnail too, even when none is recorded in the database.
    crate::thumbs::remove(&id);

    let clip_dir = state.config_snapshot().clip_dir;
    let folder = with_library(&state, |lib| {
        let folder = lib
            .get(&id)
            .map_err(|e| e.to_string())?
            .and_then(|clip| std::path::PathBuf::from(clip.path).parent().map(Path::to_path_buf));
        lib.delete(&id).map_err(|e| e.to_string())?;
        Ok(folder)
    })?;
    // If that was the last clip of its game, an empty folder would otherwise be
    // left standing.
    if let Some(folder) = folder {
        crate::filing::prune(&folder, Path::new(&clip_dir));
    }
    Ok(())
}

/// Set or take away a clip's heart.
///
/// The file does **not** move by itself — `file_clip` takes care of that once
/// the clip is no longer open in the player. Moving it out from under the
/// playing video would tear the playback off.
#[tauri::command]
pub fn set_clip_favorite(state: State<'_, AppState>, id: String, favorite: bool) -> Result<Clip> {
    with_library(&state, |lib| {
        lib.set_favorite(&id, favorite).map_err(|e| e.to_string())?;
        lib.get(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "clip not found".into())
    })
}

/// Move a clip's file into the folder it belongs in.
///
/// If the move fails — usually because the file is still open — it stays where
/// it is and the clip comes back unchanged. The next start catches it up
/// (`filing::tidy`); the gallery is right in the meantime regardless, because it
/// reads from the database.
#[tauri::command]
pub fn file_clip(state: State<'_, AppState>, id: String) -> Result<Clip> {
    let clip_dir = state.config_snapshot().clip_dir;
    with_library(&state, |lib| {
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
                log::warn!("Clip '{id}' blieb liegen: {err}");
                Ok(clip)
            }
        }
    })
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

    with_library(&state, |lib| {
        lib.update_meta(&id, title.as_deref(), description.as_deref(), game.as_deref())
            .map_err(|e| e.to_string())?;
        lib.get(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "clip not found".to_string())
    })
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
    stems::tracks(&clip.id, &edit::source_path(&clip))
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
    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "clip not found".to_string())?;
    refuse_still(&clip)?;
    let (encoder, bitrate) = encoder_for(&state);
    let trim = edit::Trim { start_ms, end_ms };

    let applied = edit::apply(&clip, trim, &tracks, encoder, bitrate, progress(&app, &id));
    store(&state, &app, &clip, applied, tracks)
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
    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "clip not found".to_string())?;
    refuse_still(&clip)?;
    let (encoder, bitrate) = encoder_for(&state);
    let tracks = clip.edit.as_ref().map(|e| e.tracks.clone()).unwrap_or_default();

    // If it fails, the record stays on the clip: it carries the offset the
    // individual tracks sit at. Throwing it away because the recording cannot
    // be found would leave the preview out of sync forever.
    let applied = edit::restore(&clip, &tracks, encoder, bitrate, progress(&app, &id));
    store(&state, &app, &clip, applied, tracks)
}

/// Encoder and bitrate for a re-encode run, taken from the settings.
fn encoder_for(state: &State<'_, AppState>) -> (crate::model::EncoderId, u32) {
    let recording = state.config_snapshot().recording;
    (encode::resolve(recording.encoder), recording.bitrate_kbps)
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

    // The trim now sits in the file — what is stored here is the full range of
    // the new file. Where that sits in the original is recorded alongside.
    let edit = ClipEdit {
        start_ms: 0,
        end_ms: applied.duration_ms,
        tracks,
    };
    with_library(state, |lib| {
        lib.set_edit(&clip.id, Some(&edit)).map_err(|e| e.to_string())?;
        lib.set_original(&clip.id, applied.original.as_ref())
            .map_err(|e| e.to_string())?;
        lib.set_file_state(&clip.id, applied.duration_ms, applied.size_bytes)
            .map_err(|e| e.to_string())?;
        lib.get(&clip.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "clip not found".to_string())
    })
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

/// Version from `tauri.conf.json` — the settings display it.
#[tauri::command]
pub fn app_version(app: tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Look for an update. `None` means already up to date.
#[tauri::command]
pub async fn check_update(app: tauri::AppHandle) -> Result<Option<crate::updater::UpdateInfo>> {
    crate::updater::check(&app).await
}

/// Install the update that was found. Quits the app and starts the installer.
#[tauri::command]
pub async fn install_update(app: tauri::AppHandle) -> Result<()> {
    crate::updater::install(&app).await
}
