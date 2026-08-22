//! Shared state between UI commands and the recording threads.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::audio::engine::AudioEngine;
use crate::clips::Library;
use crate::config;
use crate::pipeline::{Pipeline, Shared};
use crate::model::{AppConfig, AudioSource, Clip, EngineStatus, RecordingConfig};

pub struct AppState {
    pub config: Mutex<AppConfig>,
    pub library: Mutex<Option<Library>>,
    pub status: Mutex<EngineStatus>,
    pub audio: Arc<AudioEngine>,
    pub pipeline: Mutex<Option<Pipeline>>,
    /// A second handle on the pipeline state, for reading metrics only.
    ///
    /// `save_clip()` holds `pipeline` across the whole seal and ffmpeg run; a
    /// status tick that wanted to read along there would block for a second.
    pub shared: Mutex<Option<Arc<Shared>>>,
    /// Foreground game last detected, updated continuously.
    pub current_game: Mutex<Option<String>>,
    /// The game that was running when the buffer last ran — the name on the
    /// clip.
    ///
    /// It is remembered because when saving via the button ClippiBoy itself is
    /// in the foreground, and the snapshot would be empty then.
    pub buffering_game: Mutex<Option<String>>,
    /// Only once "Quit" was chosen in the tray may the process exit — otherwise
    /// the ✕ just hides the window.
    pub quitting: std::sync::atomic::AtomicBool,
    /// Bookkeeping for the automatic buffer, see `AutoBuffer`.
    pub auto: AutoBuffer,
    /// The recording settings the running buffer was started with — fitted to
    /// the source, and therefore the clip's real dimensions.
    pub active_recording: Mutex<Option<RecordingConfig>>,
    /// Is a save in progress right now? See `begin_save`.
    saving: std::sync::atomic::AtomicBool,
    /// Wraps starting and stopping the recording.
    ///
    /// The "already running?" check and registering the finished pipeline are
    /// far apart — the capture hardware is brought up in between, which takes a
    /// moment. Without this lock two simultaneous starts (hotkey and automation,
    /// or tray and window) would both get past the check and create **two**
    /// recordings; the first would then hang unreachable, holding encoder and
    /// screen occupied. A stop in the middle of a start would likewise have
    /// grabbed at nothing.
    lifecycle: Mutex<()>,
}

/// Lives for as long as a clip is being written and releases the slot when
/// dropped — even if a `?` strikes in between.
pub struct SavingGuard<'a>(&'a std::sync::atomic::AtomicBool);

impl Drop for SavingGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// State of the "buffer on as soon as a game runs" automation.
///
/// It has to keep two things apart: a buffer the user started themselves it
/// must not switch off again. And one the user switched off themselves it must
/// not restart two seconds later — which is why it stays quiet until the game
/// ends.
#[derive(Default)]
pub struct AutoBuffer {
    /// Is the buffer running because the automation started it?
    started: std::sync::atomic::AtomicBool,
    /// Switched off by the user — do not touch again until the game is gone.
    suppressed: std::sync::atomic::AtomicBool,
    /// How many checks in a row have seen no game.
    missing: std::sync::atomic::AtomicU32,
}

/// What the automation should do next.
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum AutoAction {
    Nothing,
    Start,
    Stop,
}

/// This many checks without a detected game counts as "game ended". A check
/// runs every two seconds — a brief alt-tab must not choke the buffer, or the
/// recording is gone exactly when you come back.
const GRACE_POLLS: u32 = 15;

impl AutoBuffer {
    /// One round of the automation. Called at the cadence of game detection,
    /// and only when both "auto start" and "only in game" are on.
    pub fn poll(&self, game_present: bool, buffer_active: bool) -> AutoAction {
        use std::sync::atomic::Ordering::SeqCst;
        if game_present {
            self.missing.store(0, SeqCst);
            if !buffer_active && !self.suppressed.load(SeqCst) {
                self.started.store(true, SeqCst);
                return AutoAction::Start;
            }
            return AutoAction::Nothing;
        }

        if self.missing.fetch_add(1, SeqCst) + 1 < GRACE_POLLS {
            return AutoAction::Nothing;
        }
        // The game really has ended: a new one may start the buffer again, even
        // if the user switched it off in between.
        self.suppressed.store(false, SeqCst);
        if buffer_active && self.started.swap(false, SeqCst) {
            AutoAction::Stop
        } else {
            AutoAction::Nothing
        }
    }

    /// One round for running without "only in game": the buffer should run as
    /// long as nobody has switched it off by hand.
    ///
    /// Checking that on a tick rather than only at program start means a toggle
    /// in the settings takes effect immediately — before, nothing at all
    /// happened until the next start of ClippiBoy.
    pub fn poll_always(&self, buffer_active: bool) -> AutoAction {
        use std::sync::atomic::Ordering::SeqCst;
        if buffer_active || self.suppressed.load(SeqCst) {
            return AutoAction::Nothing;
        }
        self.started.store(true, SeqCst);
        AutoAction::Start
    }

    /// The user started the buffer themselves.
    pub fn manual_start(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        self.started.store(false, SeqCst);
        self.suppressed.store(false, SeqCst);
    }

    /// The user stopped the buffer themselves.
    pub fn manual_stop(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        self.started.store(false, SeqCst);
        self.suppressed.store(true, SeqCst);
    }
}

impl AppState {
    pub fn new() -> Self {
        let config = config::load();
        let library = match Library::open() {
            Ok(lib) => Some(lib),
            Err(err) => {
                log::error!("could not open the clip database: {err}");
                None
            }
        };
        let status = EngineStatus {
            buffer_active: false,
            buffered_seconds: 0.0,
            buffer_bytes: 0,
            dropped_frames: 0,
            encoder: Some(config.recording.encoder),
            fps: 0.0,
            game: None,
        };

        // The sources run from startup — that is the only way the mixer shows
        // real levels even before the buffer is active.
        let audio = Arc::new(AudioEngine::new());
        audio.apply(&config.sources);

        Self {
            config: Mutex::new(config),
            library: Mutex::new(library),
            status: Mutex::new(status),
            audio,
            pipeline: Mutex::new(None),
            shared: Mutex::new(None),
            current_game: Mutex::new(None),
            buffering_game: Mutex::new(None),
            quitting: std::sync::atomic::AtomicBool::new(false),
            auto: AutoBuffer::default(),
            lifecycle: Mutex::new(()),
            active_recording: Mutex::new(None),
            saving: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn config_snapshot(&self) -> AppConfig {
        self.config.lock().clone()
    }

    /// Replace the config, write it to disk and pull the dependent parts along
    /// (buffer length and audio sources). The video source of a running
    /// recording is changed by `commands::set_config` — the restart can be
    /// reported there too.
    pub fn replace_config(&self, mut next: AppConfig) -> AppConfig {
        next.recording.encoder = crate::encode::resolve(next.recording.encoder);
        self.audio.apply(&next.sources);
        // If a recording is running it has to learn about the changed sources
        // too — otherwise it mixes the old ones until the next restart.
        if let Some(shared) = self.shared.lock().as_ref() {
            shared.set_sources(next.sources.clone());
        }
        {
            let mut guard = self.config.lock();
            *guard = next.clone();
        }
        if let Err(err) = config::save(&next) {
            log::error!("could not save the config: {err}");
        }
        next
    }

    /// Registers a save. `None` means one is already running.
    ///
    /// Writing a clip takes a few seconds depending on bitrate and length.
    /// Without this lock every further key press in that window starts a second
    /// run — and that was exactly the most common cause of "ffmpeg failed": two
    /// runs in quick succession shared target path and temp files and cleared
    /// them out from under each other.
    pub fn begin_save(&self) -> Option<SavingGuard<'_>> {
        use std::sync::atomic::Ordering::SeqCst;
        match self.saving.compare_exchange(false, true, SeqCst, SeqCst) {
            Ok(_) => Some(SavingGuard(&self.saving)),
            Err(_) => None,
        }
    }

    /// Is a recording into the buffer running right now?
    pub fn is_buffering(&self) -> bool {
        self.pipeline.lock().is_some()
    }

    pub fn upsert_source(&self, source: AudioSource) -> AppConfig {
        let mut config = self.config_snapshot();
        match config.sources.iter_mut().find(|s| s.id == source.id) {
            Some(existing) => *existing = source,
            None => config.sources.push(source),
        }
        self.replace_config(config)
    }

    pub fn remove_source(&self, id: &str) -> AppConfig {
        let mut config = self.config_snapshot();
        config.sources.retain(|s| s.id != id);
        self.replace_config(config)
    }
}

impl AppState {
    /// Starts recording into the replay buffer.
    pub fn start_pipeline(&self) -> Result<(), String> {
        let _lifecycle = self.lifecycle.lock();
        if self.pipeline.lock().is_some() {
            return Ok(());
        }
        if !crate::muxer::available() {
            return Err(
                "ffmpeg was not found. Without ffmpeg no clips can be written."
                    .into(),
            );
        }

        let mut config = self.config_snapshot();
        // The screen may have changed since it was configured — better to fit
        // to the source once more here than to record upscaled.
        crate::capture::fit_to_target(&mut config.recording);
        let pipeline = Pipeline::start(
            &config.recording,
            config.buffer.seconds,
            config.sources.clone(),
            self.audio.clone(),
        )?;
        *self.shared.lock() = Some(pipeline.shared.clone());
        *self.pipeline.lock() = Some(pipeline);
        *self.active_recording.lock() = Some(config.recording.clone());
        // Copy first, then set: holding both locks at once would run against
        // the reverse order in `save_clip`, and the two hotkey threads could
        // deadlock each other.
        let game = self.current_game.lock().clone();
        *self.buffering_game.lock() = game;

        let mut status = self.status.lock();
        status.buffer_active = true;
        // The encoder actually chosen, not the requested one: if the wanted MFT
        // is not registered, the display would otherwise permanently show
        // something other than what is running.
        status.encoder = match self.shared.lock().as_ref() {
            Some(shared) => *shared.encoder.lock(),
            None => None,
        };
        status.dropped_frames = 0;
        Ok(())
    }

    pub fn stop_pipeline(&self) {
        let _lifecycle = self.lifecycle.lock();
        self.shared.lock().take();
        self.active_recording.lock().take();
        if let Some(mut pipeline) = self.pipeline.lock().take() {
            pipeline.stop();
        }
        let mut status = self.status.lock();
        status.buffer_active = false;
        status.buffered_seconds = 0.0;
        status.buffer_bytes = 0;
        status.fps = 0.0;
    }

    /// Called by the status tick: keep the foreground game up to date.
    ///
    /// While buffering, only a real detection overwrites the remembered name —
    /// switching from the game to the desktop still leaves the clip assigned to
    /// the game.
    pub fn track_game(&self) -> Option<String> {
        let detected = crate::game::detect();
        *self.current_game.lock() = detected.clone();
        if detected.is_some() && self.status.lock().buffer_active {
            *self.buffering_game.lock() = detected.clone();
        }
        detected
    }

    /// Carry the running recording's current metrics into the status.
    pub fn status_snapshot(&self) -> EngineStatus {
        let mut status = self.status.lock().clone();
        if let Some(shared) = self.shared.lock().as_ref() {
            status.buffered_seconds = shared.buffered_seconds();
            status.buffer_bytes = shared.buffer_bytes();
            status.dropped_frames = shared.dropped.load(std::sync::atomic::Ordering::Relaxed);
            // The encoder gets a constant `fps` frames — so the number alone
            // says nothing. What is interesting is how many of them were real:
            // the rest are repeats because the picture stood still.
            let frames = shared.frames.load(std::sync::atomic::Ordering::Relaxed);
            let duplicated = shared.duplicated.load(std::sync::atomic::Ordering::Relaxed);
            if frames > 0 {
                let live = frames.saturating_sub(duplicated) as f32 / frames as f32;
                status.fps = shared.fps as f32 * live;
            }
        }
        status.game = self.current_game.lock().clone();
        status
    }

    /// Writes the last `seconds` seconds as an MP4.
    pub fn save_clip(&self, seconds: Option<u32>) -> Result<Clip, String> {
        let config = self.config_snapshot();
        let seconds = seconds.unwrap_or(config.buffer.seconds).max(1);

        // Grab everything under a short lock and release it again right away:
        // muxing and the thumbnail take seconds, and "Quit" from the tray, say,
        // would not reach the buffer all that while.
        //
        // There is no sealing step any more: the encoder runs through, every
        // finished packet is already in the ring. So there is nothing to wait
        // for and nothing that could be lost along the way.
        let snapshot = {
            let guard = self.pipeline.lock();
            let pipeline = guard
                .as_ref()
                .ok_or_else(|| "The replay buffer is not running.".to_string())?;
            pipeline.snapshot(seconds)?
        };

        // The file's dimensions come from the running recording, not from the
        // config: what gets saved is what the encoder really received.
        let recording = self
            .active_recording
            .lock()
            .clone()
            .unwrap_or_else(|| config.recording.clone());

        // Not the snapshot but the game detected while buffering — when saving
        // via the button, ClippiBoy is in the foreground.
        let buffering_game = self.buffering_game.lock().clone();
        let game = buffering_game.or_else(|| self.current_game.lock().clone());
        let output = destination(&config.clip_dir, game.as_deref(), false)?;

        // The id already here: the muxer files the individual tracks under it,
        // and those come out of the same ffmpeg run as the clip.
        let id = uuid::Uuid::new_v4().to_string();
        let result = crate::muxer::build(crate::muxer::ClipRequest {
            snapshot,
            clip_id: id.clone(),
            output,
            temp_dir: config::data_dir().join("temp"),
        })?;

        Ok(Clip {
            id,
            path: result.path.to_string_lossy().to_string(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
            duration_ms: result.duration_ms,
            game,
            width: recording.width,
            height: recording.height,
            size_bytes: result.size_bytes,
            thumb_path: result.thumb_path.map(|p| p.to_string_lossy().to_string()),
            title: None,
            description: None,
            favorite: false,
            edit: None,
            original: None,
            screenshot: false,
        })
    }

    /// One picture from the recording source.
    ///
    /// Unlike `save_clip` this needs no running buffer — it opens a capture
    /// session of its own and closes it again as soon as a frame has arrived.
    /// It deliberately skips `begin_save` too: a screenshot is done in a blink,
    /// and whoever presses twice wants two pictures.
    pub fn take_screenshot(&self) -> Result<Clip, String> {
        let config = self.config_snapshot();
        // Whatever the buffer is really capturing, otherwise what is configured
        // — the source, not the size: a picture comes at the source's native
        // resolution. The scaling in the recording settings is there to save
        // bitrate, and a screenshot has none to save.
        let recording = self
            .active_recording
            .lock()
            .clone()
            .unwrap_or_else(|| config.recording.clone());

        let shot = crate::shot::grab(recording.target_kind, recording.target_id.as_deref())?;

        let buffering_game = self.buffering_game.lock().clone();
        let game = buffering_game.or_else(|| self.current_game.lock().clone());
        let path = destination(&config.clip_dir, game.as_deref(), true)?;
        shot.write_png(&path)?;

        let id = uuid::Uuid::new_v4().to_string();
        // A picture of its own, not the screenshot itself: `thumbs::migrate`
        // carries every thumbnail lying outside its folder into it, and would
        // take the original out of the user's clip folder along the way.
        let thumb_path = match crate::thumbs::make_still(&path, &id) {
            Ok(thumb) => Some(thumb.to_string_lossy().to_string()),
            Err(err) => {
                log::warn!("no thumbnail for the screenshot: {err}");
                None
            }
        };

        Ok(Clip {
            id,
            path: path.to_string_lossy().to_string(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
            duration_ms: 0,
            game,
            width: shot.width,
            height: shot.height,
            size_bytes: std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0),
            thumb_path,
            title: None,
            description: None,
            favorite: false,
            edit: None,
            original: None,
            screenshot: true,
        })
    }
}

/// Where a new file goes and what it is called.
///
/// Milliseconds are part of the name: two clips in the same second would
/// otherwise get the same path. Both ffmpeg runs would then write the same file,
/// share the segment list in the temp folder, and in the database (`path` is
/// UNIQUE) only one of the two would remain.
///
/// The folder follows from game and kind. Nothing new is a favorite yet, so that
/// question does not arise here.
fn destination(
    clip_dir: &str,
    game: Option<&str>,
    screenshot: bool,
) -> Result<std::path::PathBuf, String> {
    let now = jiff::Zoned::now();
    let stamp = format!(
        "{}-{:03}",
        now.strftime("%Y-%m-%d_%H-%M-%S"),
        now.subsec_nanosecond() / 1_000_000
    );
    let extension = if screenshot { "png" } else { "mp4" };
    let name = match game {
        Some(app) => format!("{}_{stamp}.{extension}", sanitize_name(app)),
        None if screenshot => format!("shot_{stamp}.{extension}"),
        None => format!("clip_{stamp}.{extension}"),
    };

    let dir = crate::filing::dir_for(
        std::path::Path::new(clip_dir),
        game,
        false,
        screenshot,
    );
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("could not create folder '{}': {err}", dir.display()))?;
    Ok(dir.join(name))
}

fn sanitize_name(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
