//! Shared state between UI commands and the recording threads.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::audio::engine::AudioEngine;
use crate::clips::Library;
use crate::config;
use crate::pipeline::{Pipeline, Shared};
use crate::model::{AppConfig, AudioSource, Clip, ClipKind, EngineStatus, RecordingConfig};
use crate::recorder::{self, Recorder};

/// How much the ring keeps while only a recording holds the pipeline. Enough
/// for a keyframe to begin on — the ring always keeps its last one anyway.
const IDLE_RING_SECONDS: u32 = 2;

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
    /// The name that goes with the bound process, kept while the game is not
    /// in the foreground.
    ///
    /// Detection only ever looks at the foreground window, so the name fell
    /// away the moment the console over the game opened — and that is precisely
    /// when somebody wants to read it.
    bound_game: Mutex<Option<String>>,
    /// When the running game was first bound — the start of the session the
    /// console shows under its name.
    ///
    /// Hangs off the binding below and not off `current_game`: whoever alt-tabs
    /// to the browser is still playing, and a clock that restarted every time
    /// they came back would be worthless.
    game_since: Mutex<Option<std::time::Instant>>,
    /// What is free on the clip drive, in bytes; `0` means not known yet.
    ///
    /// An atomic and not a `Mutex`, because the status tick reads it and one
    /// slow thread writes it — see `free_bytes` in `clips.rs` for why it is not
    /// simply read where it is used.
    pub free_bytes: std::sync::atomic::AtomicU64,
    /// The process the game audio source is bound to, with its exe name.
    ///
    /// Deliberately stickier than `current_game`: detection looks at the
    /// foreground window, so alt-tabbing into a browser must not tear the game's
    /// audio stream down. The exe is kept for the liveness check — Windows
    /// reuses PIDs.
    game_pid: Mutex<Option<(u32, String)>>,
    /// The game name the log last mentioned. Detection runs every two seconds;
    /// without this the same line would go into the log thirty times a minute.
    logged_game: Mutex<Option<String>>,
    /// What is wrong with the sources as configured, recomputed on the
    /// two-second tick and read out on the one-second one.
    ///
    /// Kept here rather than worked out per emit because it needs the list of
    /// endpoints, and enumerating those is a COM round trip — not something to
    /// do once a second for a line that changes about once an hour.
    pub source_notes: Mutex<std::collections::HashMap<String, String>>,
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
    /// The recording started by hand, if one runs.
    recorder: Mutex<Option<Arc<Recorder>>>,
    /// Does anybody want the replay buffer? The pipeline can run without it:
    /// a recording keeps it alive when the buffer is switched off, by hand or
    /// by the automation, and then only this goes false.
    buffer_wanted: std::sync::atomic::AtomicBool,
    /// How many screen checks in a row found the capture on the wrong screen —
    /// see `screen_drift`.
    drift_seen: std::sync::atomic::AtomicU8,
}

/// Lives for as long as a clip is being written and releases the slot when
/// dropped — even if a `?` strikes in between.
pub struct SavingGuard<'a>(&'a std::sync::atomic::AtomicBool);

impl Drop for SavingGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// What one turn of the game tick found.
///
/// The two names in here are deliberately not one. `held` hangs on the game's
/// *process* and is what the app shows and files clips under — a look into the
/// browser must not erase the name or reset the session clock. `playing` is
/// about where the user is *looking*, and only that may decide whether "only
/// buffer in game" keeps the buffer alive; hanging it on `held` too would let
/// the buffer and its encoder run on until the game is closed.
pub struct GameTick {
    /// The name to show and to file under, held for the process's life.
    pub held: Option<String>,
    /// Is the user at the game right now — its window in front, or one of ours
    /// over it?
    pub playing: bool,
    /// The process the game audio is bound to has changed; the caller has to
    /// rebuild the streams.
    pub rebound: bool,
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
    /// How many checks in a row have seen the secure desktop.
    locked: std::sync::atomic::AtomicU32,
    /// Was the buffer stopped because the screen locked? Then it comes back on
    /// unlock — whoever had started it.
    for_lock: std::sync::atomic::AtomicBool,
}

/// What the automation should do next.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum AutoAction {
    Nothing,
    Start,
    Stop,
}

/// This many checks without a detected game counts as "game ended". A check
/// runs every two seconds — a brief alt-tab must not choke the buffer, or the
/// recording is gone exactly when you come back.
const GRACE_POLLS: u32 = 15;

/// This many checks on the secure desktop before the buffer is stopped for it —
/// ten seconds at the two-second cadence.
///
/// Not zero, because a UAC prompt puts the same secure desktop in front as the
/// lock screen does and `capture::secure_desktop` cannot tell them apart. A
/// prompt is answered in seconds; stopping for it would throw away a minute and
/// a half of history over a dialog. A real lock lasts longer than this by a wide
/// margin, and the few frozen frames a prompt leaves behind cost nothing.
const LOCK_POLLS: u32 = 5;

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

    /// What the locked screen means for the buffer.
    ///
    /// Runs before the other two and regardless of every setting, because it is
    /// not about when recording is wanted but about whether recording is even
    /// possible. On the secure desktop Windows.Graphics.Capture delivers nothing
    /// and reports nothing; the clock goes on repeating the last frame, and the
    /// ring fills with a still of the lock screen. Leaving it running does not
    /// preserve anything — it overwrites what was there with a frozen picture.
    ///
    /// Coming back is deliberately not gated on `suppressed`: the user did not
    /// switch this off, the lock screen did.
    pub fn poll_lock(&self, secure_desktop: bool, buffer_active: bool) -> AutoAction {
        use std::sync::atomic::Ordering::SeqCst;
        if secure_desktop {
            if self.locked.fetch_add(1, SeqCst) + 1 < LOCK_POLLS {
                return AutoAction::Nothing;
            }
            if buffer_active {
                self.for_lock.store(true, SeqCst);
                return AutoAction::Stop;
            }
            return AutoAction::Nothing;
        }

        self.locked.store(0, SeqCst);
        if self.for_lock.swap(false, SeqCst) && !buffer_active {
            AutoAction::Start
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
            // Not known until an encoder has really been built.
            rate_control: None,
            fps: 0.0,
            game: None,
            game_seconds: None,
            free_bytes: None,
            recording: false,
            recording_seconds: 0.0,
            recording_bytes: 0,
            screen_fallback: None,
        };

        // The sources run from startup — that is the only way the mixer shows
        // real levels even before the buffer is active.
        let audio = Arc::new(AudioEngine::new());
        // Devices and processes are deliberately **not** enumerated here: this
        // runs on the main thread, and the COM initialisation that comes with it
        // would leave that thread in the multithreaded apartment, where Tauri's
        // window layer refuses to start (`RPC_E_CHANGED_MODE`). So only what
        // needs no enumeration starts right away — the first status tick, two
        // seconds later, brings up the game and the leftovers.
        audio.apply(&crate::audio::resolve(
            &config.sources,
            None,
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
            &crate::audio::Leftovers::default(),
        ));

        Self {
            config: Mutex::new(config),
            library: Mutex::new(library),
            status: Mutex::new(status),
            audio,
            pipeline: Mutex::new(None),
            shared: Mutex::new(None),
            current_game: Mutex::new(None),
            bound_game: Mutex::new(None),
            game_since: Mutex::new(None),
            free_bytes: std::sync::atomic::AtomicU64::new(0),
            game_pid: Mutex::new(None),
            logged_game: Mutex::new(None),
            source_notes: Mutex::new(std::collections::HashMap::new()),
            buffering_game: Mutex::new(None),
            quitting: std::sync::atomic::AtomicBool::new(false),
            auto: AutoBuffer::default(),
            lifecycle: Mutex::new(()),
            active_recording: Mutex::new(None),
            saving: std::sync::atomic::AtomicBool::new(false),
            recorder: Mutex::new(None),
            buffer_wanted: std::sync::atomic::AtomicBool::new(false),
            drift_seen: std::sync::atomic::AtomicU8::new(0),
        }
    }

    pub fn config_snapshot(&self) -> AppConfig {
        self.config.lock().clone()
    }

    /// The process the game audio currently follows.
    pub fn game_pid(&self) -> Option<u32> {
        self.game_pid.lock().as_ref().map(|(pid, _)| *pid)
    }

    /// Bring the running streams in line with config, detected game and what is
    /// playing right now.
    ///
    /// Always off-thread: the work behind it is COM, and it must not land on the
    /// main thread (see [`AudioEngine::refresh_async`]).
    pub fn apply_audio(&self) {
        self.audio
            .refresh_async(self.config.lock().sources.clone(), self.game_pid());
    }

    /// Replace the config, write it to disk and pull the dependent parts along
    /// (buffer length and audio sources). The video source of a running
    /// recording is changed by `commands::set_config` — the restart can be
    /// reported there too.
    pub fn replace_config(&self, mut next: AppConfig) -> AppConfig {
        next.recording.encoder = crate::encode::resolve(next.recording.encoder);
        {
            let mut guard = self.config.lock();
            *guard = next.clone();
        }
        self.apply_audio();
        // If a recording is running it has to learn about the changed sources
        // too — otherwise it mixes the old ones until the next restart. It gets
        // the **unresolved** ones: it only needs id, gain, mute, solo and the
        // track flag, and that way a game source keeps its track even while no
        // game is running.
        if let Some(shared) = self.shared.lock().as_ref() {
            shared.set_sources(next.sources.clone());
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

    /// Is a clip being written right now? The control port reports it so a
    /// Stream Deck key can wait instead of hammering.
    pub fn is_saving(&self) -> bool {
        self.saving.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Is a recording into the buffer running right now?
    pub fn is_buffering(&self) -> bool {
        self.pipeline.lock().is_some() && self.buffer_wanted()
    }

    fn buffer_wanted(&self) -> bool {
        self.buffer_wanted.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Is a recording started by hand running right now?
    pub fn is_recording(&self) -> bool {
        self.recorder.lock().is_some()
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
            // Running for a recording only: the buffer just gets its length
            // back, and fills from here.
            if !self.buffer_wanted.swap(true, std::sync::atomic::Ordering::SeqCst) {
                let config = self.config_snapshot();
                if let Some(shared) = self.shared.lock().as_ref() {
                    shared.packets.lock().set_capacity(
                        config.buffer.seconds,
                        config::effective_memory_bytes(&config.recording, &config.buffer),
                    );
                }
                let game = self.current_game.lock().clone();
                *self.buffering_game.lock() = game;
                self.status.lock().buffer_active = true;
            }
            return Ok(());
        }
        self.launch_pipeline(true)
    }

    /// Bring the capture stack up. `for_buffer` false means a recording is the
    /// only reason, and the ring stays small. The caller holds `lifecycle`.
    fn launch_pipeline(&self, for_buffer: bool) -> Result<(), String> {
        // While ffmpeg is still being fetched the buffer may already run —
        // it is only needed once a clip is written.
        if !crate::muxer::available() && crate::tools::is_ready() {
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
            config::effective_memory_bytes(&config.recording, &config.buffer),
            config.sources.clone(),
            self.audio.clone(),
        )?;
        if !for_buffer {
            pipeline.shared.packets.lock().set_capacity(IDLE_RING_SECONDS, u64::MAX);
        }
        self.buffer_wanted
            .store(for_buffer, std::sync::atomic::Ordering::SeqCst);
        *self.shared.lock() = Some(pipeline.shared.clone());
        *self.pipeline.lock() = Some(pipeline);
        *self.active_recording.lock() = Some(config.recording.clone());
        // Copy first, then set: holding both locks at once would run against
        // the reverse order in `save_clip`, and the two hotkey threads could
        // deadlock each other.
        let game = self.current_game.lock().clone();
        *self.buffering_game.lock() = game;

        let mut status = self.status.lock();
        status.buffer_active = for_buffer;
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

    /// Switch the replay buffer off. A running recording keeps the pipeline:
    /// only the ring shrinks, and the buffer counts as off.
    pub fn stop_pipeline(&self) {
        let _lifecycle = self.lifecycle.lock();
        if self.is_recording() {
            self.buffer_wanted
                .store(false, std::sync::atomic::Ordering::SeqCst);
            if let Some(shared) = self.shared.lock().as_ref() {
                shared
                    .packets
                    .lock()
                    .set_capacity(IDLE_RING_SECONDS, u64::MAX);
            }
            let mut status = self.status.lock();
            status.buffer_active = false;
            status.buffered_seconds = 0.0;
            status.buffer_bytes = 0;
            return;
        }
        self.halt_pipeline();
    }

    /// Take the capture stack down. The caller holds `lifecycle`.
    fn halt_pipeline(&self) {
        self.buffer_wanted
            .store(false, std::sync::atomic::Ordering::SeqCst);
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

    /// Measure how many real pictures a second the screen is delivering, and
    /// remember it for the next [`Self::status_snapshot`].
    ///
    /// Only the status tick calls this. The measurement consumes the interval
    /// since the last one, so a second caller — the `engine_status` command, the
    /// control port — would take the interval away from it and read nonsense.
    /// They get the sampled value instead, which is at most a second old.
    pub fn sample_fps(&self) {
        let measured = self
            .shared
            .lock()
            .as_ref()
            .filter(|_| self.buffer_wanted())
            .map(|shared| shared.live_fps());
        if let Some(fps) = measured {
            self.status.lock().fps = fps;
        }
    }

    /// Called by the status tick: keep the foreground game up to date.
    ///
    /// While buffering, only a real detection overwrites the remembered name —
    /// switching from the game to the desktop still leaves the clip assigned to
    /// the game.
    ///
    /// See `GameTick` for why the two answers it gives are not the same one.
    pub fn track_game(&self) -> GameTick {
        let detected = crate::game::detect_detailed();
        let name = detected.as_ref().map(|game| game.name.clone());
        {
            // Say once, when it changes, which game was recognised and how. A
            // name off the window title is the fragile kind — this is the line
            // that names the exe to put into `games.json` so the title stops
            // mattering for it.
            let mut previous = self.logged_game.lock();
            if *previous != name {
                if let Some(game) = detected.as_ref() {
                    if crate::game::is_known_exe(&game.exe) {
                        log::info!("game '{}' from the list ({})", game.name, game.exe);
                    } else {
                        log::info!(
                            "game '{}' from the window title of {} — put it in games.json \
                             to pin the name",
                            game.name,
                            game.exe
                        );
                    }
                }
                *previous = name.clone();
            }
        }
        // The binding outlives the detection: leaving the game for the browser
        // must not silence the game track. Only the end of the process releases
        // it — and the exe has to still match, because Windows reuses PIDs.
        let mut bound = self.game_pid.lock();
        let before = bound.as_ref().map(|(pid, _)| *pid);
        match detected {
            Some(game) => *bound = Some((game.pid, game.exe)),
            None => {
                let alive = bound
                    .as_ref()
                    .is_some_and(|(pid, exe)| crate::game::still_running(*pid, exe));
                if !alive {
                    *bound = None;
                }
            }
        }
        let now = bound.as_ref().map(|(pid, _)| *pid);
        if now != before {
            log::info!("game audio now follows {now:?} (was {before:?})");
            // Die Sitzungsuhr hängt an genau dieser Bindung: sie beginnt, wenn
            // ein Spiel gebunden wird, und endet, wenn dessen Prozess weg ist.
            // Ein Wechsel von einem Spiel zum nächsten ist ebenfalls ein
            // Wechsel der Pid und setzt die Uhr also zurück — richtig so, das
            // ist eine neue Sitzung.
            *self.game_since.lock() = now.map(|_| std::time::Instant::now());
        }
        // `now` ist kopiert, die Bindung wird ab hier nicht mehr gebraucht. Sie
        // fährt bewusst herunter, bevor unten `status` genommen wird: zwei
        // Schlösser in wechselnder Reihenfolge sind die Art von Fehler, die
        // erst nach Monaten einmal zuschlägt.
        drop(bound);
        // Der Name folgt derselben Bindung wie der Ton, und aus demselben
        // Grund: die Erkennung sieht immer nur das Fenster im Vordergrund.
        //
        // Sobald die Konsole aufgeht, ist das die Konsole — der Spielname fiel
        // also genau in dem Moment weg, in dem man ihn ablesen wollte, und die
        // Sitzungsuhr mit ihm. Dasselbe beim Blick in den Browser. Solange der
        // Prozess läuft, wird gespielt; die Erkennung ergänzt den Namen, sie
        // entzieht ihn nicht.
        {
            let mut held = self.bound_game.lock();
            match (&name, now) {
                (Some(_), _) => *held = name.clone(),
                (None, None) => *held = None,
                // Gebunden, aber gerade nicht im Vordergrund: den Namen behalten.
                (None, Some(_)) => {}
            }
        }
        let held = self.bound_game.lock().clone();
        *self.current_game.lock() = held.clone();
        if held.is_some() && (self.status.lock().buffer_active || self.is_recording()) {
            *self.buffering_game.lock() = held.clone();
        }
        // Am Spiel sitzt, wer das Spiel im Vordergrund hat — oder eins unserer
        // eigenen Fenster, während der Prozess noch läuft. Die Konsole *ist* der
        // Blick ins Spiel, und wer ein Panel eine halbe Minute offen lässt, hat
        // deshalb nicht aufgehört zu spielen.
        let playing = name.is_some() || (held.is_some() && crate::game::foreground_is_ours());
        GameTick {
            held,
            playing,
            rebound: now != before,
        }
    }

    /// Carry the running recording's current metrics into the status.
    pub fn status_snapshot(&self) -> EngineStatus {
        let mut status = self.status.lock().clone();
        if let Some(recorder) = self.recorder.lock().as_ref() {
            status.recording = true;
            let fps = self.shared.lock().as_ref().map(|s| s.fps).unwrap_or(1).max(1);
            status.recording_bytes = recorder.estimated_size(fps);
            status.recording_seconds = recorder.frames() as f32 / fps as f32;
        }
        if let Some(shared) = self.shared.lock().as_ref().filter(|_| self.buffer_wanted()) {
            status.buffered_seconds = shared.buffered_seconds();
            status.buffer_bytes = shared.buffer_bytes();
            status.dropped_frames = shared.dropped.load(std::sync::atomic::Ordering::Relaxed);
            status.rate_control = *shared.rate_control.lock();
            // `status.fps` is not worked out here: it is a rate, and a rate has
            // to be measured over an interval that only one caller may consume.
            // `sample_fps` does that, once a second, from the status tick.
        }
        // Whether it is the buffer or a recording that holds the capture, the
        // question is the same: is it where it was told to be?
        status.screen_fallback = self.shared.lock().as_ref().and_then(|shared| {
            shared
                .source
                .lock()
                .as_ref()
                .filter(|choice| choice.how == crate::capture::Match::Fallback)
                .map(|choice| choice.target.title.clone())
        });
        status.game = self.current_game.lock().clone();
        // Die Spielzeit steht nur da, wenn auch ein Spiel dasteht. Die Bindung
        // hält den Prozess länger als die Erkennung den Namen (wer zum Browser
        // tabbt, hat keinen erkannten Namen mehr) — eine Zeit ohne Namen
        // darüber wäre in der Konsole eine Zeile ohne Bezug.
        status.game_seconds = match (&status.game, *self.game_since.lock()) {
            (Some(_), Some(since)) => Some(since.elapsed().as_secs() as u32),
            _ => None,
        };
        // Nur abgelesen, nicht gemessen: den Wert holt der Statusfaden alle
        // zehn Sekunden (`clips::free_bytes`). Hier ist auch der `engine_status`
        // Befehl unterwegs, und der käme sonst auf einem getrennten Netzlaufwerk
        // zum Stehen — mitsamt dem Fenster, das ihn gerufen hat.
        status.free_bytes = match self.free_bytes.load(std::sync::atomic::Ordering::Relaxed) {
            0 => None,
            bytes => Some(bytes),
        };
        status
    }

    /// Does the running buffer still capture the screen the settings mean?
    ///
    /// Asked every two seconds by the screen watcher. Says why a restart is due
    /// — the screen went away, came back, or changed its resolution — and only
    /// once the answer has held for two checks in a row: after a sign-in
    /// Windows puts the screens back one at a time, and a restart on the first
    /// glimpse would land on whatever happened to be there already.
    ///
    /// Never while a recording runs. A restart would cut the file in two, the
    /// same reason the source cannot be switched then; the check picks it up
    /// again once the recording has stopped.
    pub fn screen_drift(&self) -> Option<crate::capture::Drift> {
        use std::sync::atomic::Ordering;
        use crate::model::TargetKind;

        let config = self.config_snapshot().recording;
        let observed = if !self.is_buffering()
            || self.is_recording()
            || config.target_kind != TargetKind::Monitor
        {
            None
        } else {
            let running = self.shared.lock().as_ref().and_then(|shared| {
                let closed = shared.source_closed.load(Ordering::SeqCst);
                shared.source.lock().clone().map(|choice| (choice, closed))
            });
            running.and_then(|(running, closed)| {
                let now = crate::capture::pick(
                    &crate::capture::list_monitors(),
                    TargetKind::Monitor,
                    config.target_id.as_deref(),
                    config.target_stable_id.as_deref(),
                );
                crate::capture::drift(&running, now.as_ref(), closed)
            })
        };
        let Some(drift) = observed else {
            self.drift_seen.store(0, Ordering::SeqCst);
            return None;
        };
        if self.drift_seen.fetch_add(1, Ordering::SeqCst) + 1 < 2 {
            return None;
        }
        self.drift_seen.store(0, Ordering::SeqCst);
        Some(drift)
    }

    /// Writes the last `seconds` seconds as an MP4. `None` takes the clip length
    /// from the settings, which is what the hotkey, the tray and the button do.
    pub fn save_clip(&self, seconds: Option<u32>) -> Result<Clip, String> {
        let config = self.config_snapshot();
        let seconds = seconds
            .unwrap_or_else(|| config::effective_clip_seconds(&config.buffer))
            .max(1);
        // The buffer keeps what it has; the same press works once ffmpeg is in.
        if !crate::tools::is_ready() && !crate::muxer::available() {
            return Err(crate::tools::not_ready_reason());
        }

        // Grab everything under a short lock and release it again right away:
        // muxing and the thumbnail take seconds, and "Quit" from the tray, say,
        // would not reach the buffer all that while.
        //
        // There is no sealing step any more: the encoder runs through, every
        // finished packet is already in the ring. So there is nothing to wait
        // for and nothing that could be lost along the way.
        let snapshot = {
            let guard = self.pipeline.lock();
            // A pipeline kept alive by a recording alone holds a ring of two
            // seconds — not a buffer anybody asked for.
            let pipeline = guard
                .as_ref()
                .filter(|_| self.buffer_wanted())
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
        let output = destination(&config.clip_dir, game.as_deref(), ClipKind::Clip)?;

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
            original_available: false,
            screenshot: false,
            recording: false,
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

        let shot = crate::shot::grab(
            recording.target_kind,
            recording.target_id.as_deref(),
            recording.target_stable_id.as_deref(),
        )?;

        let buffering_game = self.buffering_game.lock().clone();
        let game = buffering_game.or_else(|| self.current_game.lock().clone());
        let path = destination(&config.clip_dir, game.as_deref(), ClipKind::Screenshot)?;
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
            original_available: false,
            screenshot: true,
            recording: false,
        })
    }
}

impl AppState {
    /// Start a recording by hand. Brings the pipeline up if the buffer is not
    /// running; it goes down again with the recording.
    pub fn start_recording(&self) -> Result<(), String> {
        let _lifecycle = self.lifecycle.lock();
        if self.is_recording() {
            return Err("A recording is already running.".into());
        }
        if !crate::tools::is_ready() && !crate::muxer::available() {
            return Err(crate::tools::not_ready_reason());
        }
        let started_here = self.pipeline.lock().is_none();
        if started_here {
            self.launch_pipeline(false)?;
        }

        let outcome = self.attach_recorder();
        if outcome.is_err() && started_here {
            self.halt_pipeline();
        }
        let recorder = outcome?;
        *self.recorder.lock() = Some(recorder);
        self.status.lock().recording = true;
        Ok(())
    }

    fn attach_recorder(&self) -> Result<Arc<Recorder>, String> {
        let config = self.config_snapshot();
        let shared = self
            .shared
            .lock()
            .clone()
            .ok_or_else(|| "The capture is not running.".to_string())?;
        // A pipeline that has just come up has no keyframe yet. The first one
        // follows within a moment; there is nothing to begin on before it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !shared.packets.lock().has_keyframe() {
            if std::time::Instant::now() > deadline {
                return Err("The encoder has not delivered a picture — nothing to record.".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        let recording = self
            .active_recording
            .lock()
            .clone()
            .unwrap_or_else(|| config.recording.clone());
        let game = self
            .buffering_game
            .lock()
            .clone()
            .or_else(|| self.current_game.lock().clone());
        let meta = recorder::Meta {
            id: uuid::Uuid::new_v4().to_string(),
            created_at: now_ms(),
            width: recording.width,
            height: recording.height,
            game,
            // Written now, not only at stop: after a crash the folder is all
            // there is, and without it every track ends up in the main mix.
            separate: separate_tracks(&config.sources),
            ..recorder::Meta::default()
        };
        let guard = self.pipeline.lock();
        let pipeline = guard
            .as_ref()
            .ok_or_else(|| "The capture is not running.".to_string())?;
        pipeline.start_recording(std::path::Path::new(&config.clip_dir), meta)
    }

    /// Stop writing and settle the folder, without muxing it. Fast — what
    /// quitting and the updater want: the next start finishes the recording
    /// (see [`Self::recover_recordings`]).
    ///
    /// Returns the folder and its description, for a caller that does mux.
    fn close_recording(&self) -> Option<(Arc<Recorder>, recorder::Meta)> {
        let recorder = self.recorder.lock().take()?;
        if let Some(pipeline) = self.pipeline.lock().as_ref() {
            pipeline.take_recording();
        }
        // The assignment as it stands now, like a clip's: every source was
        // written on its own, so a ⧉ flipped half way applies to the whole.
        let separate = separate_tracks(&self.config_snapshot().sources);
        let game = self
            .buffering_game
            .lock()
            .clone()
            .or_else(|| self.current_game.lock().clone());
        let meta = match recorder.finish(separate, game) {
            Ok(meta) => meta,
            Err(err) => {
                log::error!("recording not closed cleanly: {err}");
                return None;
            }
        };
        if !self.buffer_wanted() {
            self.halt_pipeline();
        }
        self.status.lock().recording = false;
        Some((recorder, meta))
    }

    /// Stop the recording and write it out. Takes as long as the audio of the
    /// whole recording takes to encode — off the hotkey thread.
    pub fn stop_recording(&self, on_progress: &dyn Fn(f32)) -> Result<Clip, String> {
        let (recorder, meta) = {
            let _lifecycle = self.lifecycle.lock();
            if !self.is_recording() {
                return Err("No recording is running.".into());
            }
            self.close_recording()
                .ok_or_else(|| "The recording could not be closed — it is finished on the next start.".to_string())?
        };
        self.finish_recording(&recorder.dir, &meta, on_progress)
    }

    /// Quitting or updating: close the files, leave the muxing to the next
    /// start. An hour of audio to encode is not something to hold the exit on.
    pub fn abandon_recording(&self) {
        let _lifecycle = self.lifecycle.lock();
        if self.close_recording().is_some() {
            log::info!("recording closed on exit — it is finished on the next start");
        }
    }

    /// The first write error of the running recording, if there was one.
    pub fn recording_failure(&self) -> Option<String> {
        self.recorder.lock().as_ref().and_then(|r| r.failure())
    }

    fn finish_recording(
        &self,
        dir: &std::path::Path,
        meta: &recorder::Meta,
        on_progress: &dyn Fn(f32),
    ) -> Result<Clip, String> {
        let config = self.config_snapshot();
        let output = destination(&config.clip_dir, meta.game.as_deref(), ClipKind::Recording)?;
        let result = recorder::finalize(dir, meta, output, on_progress)?;
        Ok(Clip {
            id: meta.id.clone(),
            path: result.path.to_string_lossy().to_string(),
            created_at: meta.created_at,
            duration_ms: result.duration_ms,
            game: meta.game.clone(),
            width: meta.width,
            height: meta.height,
            size_bytes: result.size_bytes,
            thumb_path: result.thumb_path.map(|p| p.to_string_lossy().to_string()),
            title: None,
            description: None,
            favorite: false,
            edit: None,
            original: None,
            original_available: false,
            screenshot: false,
            recording: true,
        })
    }

    /// Finish what a crash, or a quit mid-recording, left behind. Needs
    /// ffmpeg, so it runs once that is in place.
    pub fn recover_recordings(&self, on_progress: &dyn Fn(f32)) -> Vec<Clip> {
        let clip_dir = self.config_snapshot().clip_dir;
        let live = self.recorder.lock().as_ref().map(|r| r.dir.clone());
        let mut done = Vec::new();
        for dir in recorder::leftovers(std::path::Path::new(&clip_dir)) {
            if live.as_ref() == Some(&dir) {
                continue;
            }
            let meta = match recorder::read_meta(&dir) {
                Ok(meta) => meta,
                Err(err) => {
                    log::warn!("recording in '{}' left alone: {err}", dir.display());
                    continue;
                }
            };
            match self.finish_recording(&dir, &meta, on_progress) {
                Ok(clip) => {
                    log::info!("recording '{}' finished after the fact", meta.id);
                    done.push(clip);
                }
                Err(err) => log::warn!("recording in '{}' not finished: {err}", dir.display()),
            }
        }
        done
    }
}

/// The sources that get a track of their own rather than the main mix.
fn separate_tracks(sources: &[AudioSource]) -> Vec<String> {
    sources
        .iter()
        .filter(|s| s.enabled && s.separate_track)
        .map(|s| s.id.clone())
        .collect()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
    kind: ClipKind,
) -> Result<std::path::PathBuf, String> {
    let now = jiff::Zoned::now();
    let stamp = format!(
        "{}-{:03}",
        now.strftime("%Y-%m-%d_%H-%M-%S"),
        now.subsec_nanosecond() / 1_000_000
    );
    let extension = if kind == ClipKind::Screenshot { "png" } else { "mp4" };
    let name = match (game, kind) {
        (Some(app), ClipKind::Recording) => {
            format!("{}_rec_{stamp}.{extension}", sanitize_name(app))
        }
        (Some(app), _) => format!("{}_{stamp}.{extension}", sanitize_name(app)),
        (None, ClipKind::Screenshot) => format!("shot_{stamp}.{extension}"),
        (None, ClipKind::Recording) => format!("rec_{stamp}.{extension}"),
        (None, ClipKind::Clip) => format!("clip_{stamp}.{extension}"),
    };

    let dir = crate::filing::dir_for(std::path::Path::new(clip_dir), game, false, kind);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Hold the secure desktop up for a whole lock's worth of checks.
    fn hold_locked(auto: &AutoBuffer, active: bool) -> AutoAction {
        let mut last = AutoAction::Nothing;
        for _ in 0..LOCK_POLLS {
            last = auto.poll_lock(true, active);
        }
        last
    }

    /// The bug this exists for: after a reboot the buffer started while the
    /// machine was still on the lock screen. WGC never delivers a frame there
    /// and never says so, the clock keeps repeating the one it had, and every
    /// clip afterwards was ninety seconds of lock screen.
    #[test]
    fn a_locked_screen_stops_the_buffer() {
        let auto = AutoBuffer::default();
        assert_eq!(hold_locked(&auto, true), AutoAction::Stop);
    }

    /// A UAC prompt puts up the same secure desktop, and `secure_desktop` cannot
    /// tell the two apart. It is answered in seconds — stopping for it would
    /// cost the whole buffer over a dialog.
    #[test]
    fn a_brief_secure_desktop_leaves_the_buffer_alone() {
        let auto = AutoBuffer::default();
        for _ in 0..LOCK_POLLS - 1 {
            assert_eq!(auto.poll_lock(true, true), AutoAction::Nothing);
        }
        // Prompt answered: the count starts over, so the next one gets the full
        // grace again rather than tipping over on its first check.
        assert_eq!(auto.poll_lock(false, true), AutoAction::Nothing);
        for _ in 0..LOCK_POLLS - 1 {
            assert_eq!(auto.poll_lock(true, true), AutoAction::Nothing);
        }
    }

    /// Signing in brings the buffer back — whoever had started it. The user did
    /// not switch this off, so `suppressed` has no say.
    #[test]
    fn the_buffer_comes_back_after_the_sign_in() {
        let auto = AutoBuffer::default();
        auto.manual_stop(); // even a buffer the user had switched off before
        auto.manual_start(); // …and started again by hand
        assert_eq!(hold_locked(&auto, true), AutoAction::Stop);
        assert_eq!(auto.poll_lock(false, false), AutoAction::Start);
    }

    /// Only what the lock stopped comes back. A buffer that was off before the
    /// screen locked must not switch itself on at the sign-in.
    #[test]
    fn an_unlock_starts_nothing_it_did_not_stop() {
        let auto = AutoBuffer::default();
        assert_eq!(hold_locked(&auto, false), AutoAction::Nothing);
        assert_eq!(auto.poll_lock(false, false), AutoAction::Nothing);
    }

    /// And it comes back exactly once — a second check must not restart a buffer
    /// the user has switched off in the meantime.
    #[test]
    fn the_buffer_comes_back_only_once() {
        let auto = AutoBuffer::default();
        assert_eq!(hold_locked(&auto, true), AutoAction::Stop);
        assert_eq!(auto.poll_lock(false, false), AutoAction::Start);
        assert_eq!(auto.poll_lock(false, false), AutoAction::Nothing);
    }

    /// Locking twice in a row asks twice — the flag from the first lock must not
    /// linger and turn the second sign-in into a start that was never stopped.
    #[test]
    fn two_locks_in_a_row_each_stand_on_their_own() {
        let auto = AutoBuffer::default();
        assert_eq!(hold_locked(&auto, true), AutoAction::Stop);
        assert_eq!(auto.poll_lock(false, false), AutoAction::Start);
        assert_eq!(hold_locked(&auto, true), AutoAction::Stop);
        assert_eq!(auto.poll_lock(false, false), AutoAction::Start);
    }
}
