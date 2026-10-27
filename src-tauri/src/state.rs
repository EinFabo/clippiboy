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

/// The packet ring's memory budget. Worked out from the recording the
/// pipeline really runs — fitted to the source, with its bitrate — when there
/// is one: from the config's, an automatic budget changed size with every
/// length change against the one it started with (BUF-V2).
pub fn memory_budget(running: Option<&RecordingConfig>, config: &AppConfig) -> u64 {
    config::effective_memory_bytes(running.unwrap_or(&config.recording), &config.buffer)
}

/// Seconds and bytes for the packet ring. Kept for a recording alone, it
/// stays small whatever the buffer is set to.
pub fn ring_size(for_buffer: bool, seconds: u32, memory: u64) -> (u32, u64) {
    if for_buffer {
        (seconds, memory)
    } else {
        (IDLE_RING_SECONDS, u64::MAX)
    }
}

/// Does going from `a` to `b` need the capture restarted? The source and
/// everything the encoder is built with. Not the bitrate: it is derived from
/// the picture (`config::normalize_recording`), so it moves with the fields
/// that already count here.
pub fn capture_changed(a: &RecordingConfig, b: &RecordingConfig) -> bool {
    a.target_kind != b.target_kind
        || a.target_id != b.target_id
        || a.target_stable_id != b.target_stable_id
        || a.width != b.width
        || a.height != b.height
        || a.fps != b.fps
        || a.quality != b.quality
        || a.encoder != b.encoder
        || a.keyframe_seconds != b.keyframe_seconds
}

/// Restarts asked for by settings changes, run by one worker at a time.
///
/// Every request after the worker began a pass makes it run one more, so a
/// change made while the capture was starting is not left behind — clicking
/// window A, then window B, ends on B (CFG-8) — and any number of them in a
/// row cost a single extra pass, not one restart each.
#[derive(Default)]
pub struct RestartQueue {
    pending: std::sync::atomic::AtomicBool,
    running: std::sync::atomic::AtomicBool,
}

impl RestartQueue {
    /// Ask for a restart. True: no worker runs, the caller has to start one.
    pub fn request(&self) -> bool {
        use std::sync::atomic::Ordering::SeqCst;
        self.pending.store(true, SeqCst);
        !self.running.swap(true, SeqCst)
    }

    /// For the worker: true runs one more pass, false means it has ended.
    pub fn next(&self) -> bool {
        use std::sync::atomic::Ordering::SeqCst;
        loop {
            if self.pending.swap(false, SeqCst) {
                return true;
            }
            self.running.store(false, SeqCst);
            // A request between the two lines above found the worker still
            // running and started none: this one takes it on, unless a new
            // worker already has.
            if !self.pending.load(SeqCst) || self.running.swap(true, SeqCst) {
                return false;
            }
        }
    }

    /// The worker died mid-pass: the next request starts a new one. A
    /// request it left pending is kept for that one.
    pub fn abandon(&self) {
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Does the capture have to be rebuilt to match `fitted` — the configured
/// recording fitted to its source, as a start would make it? Against what
/// actually runs, so a write that only spells out the size the capture was
/// already fitted to (the Video page does that on opening) empties nothing.
pub fn restart_needed(running: Option<&RecordingConfig>, fitted: &RecordingConfig) -> bool {
    running.is_none_or(|running| capture_changed(running, fitted))
}

/// What a settings restart does.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum SettingsStep {
    Nothing,
    /// A recording holds the capture: after it (`settings_waiting`).
    Wait,
    Restart,
}

/// The rule for a settings restart. A recording comes first, buffer on or
/// off: switched on during the recording, the buffer takes over its capture
/// as it stands, and only the recording's end can bring the new settings in.
/// `needed` (see [`restart_needed`]) is asked only when it decides.
pub fn settings_step(
    pipeline_running: bool,
    recording: bool,
    buffer_wanted: bool,
    needed: impl FnOnce() -> bool,
) -> SettingsStep {
    if !pipeline_running {
        SettingsStep::Nothing
    } else if recording {
        SettingsStep::Wait
    } else if buffer_wanted && needed() {
        SettingsStep::Restart
    } else {
        SettingsStep::Nothing
    }
}

pub struct AppState {
    pub config: Mutex<AppConfig>,
    /// The last configs handed out, by revision, for merging a write built on
    /// an older one (`config::accept`). Only ever locked inside `config`,
    /// never the other way round.
    config_history: Mutex<std::collections::VecDeque<AppConfig>>,
    /// What went wrong reading the config file at start, worded for the
    /// start-up banner; `None` when it loaded fine.
    pub config_trouble: Mutex<Option<String>>,
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
    /// When the game in `buffering_game` was closed. Its name stays on clips
    /// only as long as a clip can still hold its footage — see `assigned_game`.
    game_ended: Mutex<Option<std::time::Instant>>,
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
    /// How many recordings are being written out — a stop's from the close
    /// until the muxing is done, and the recovery's. Counted, not a flag: the
    /// two can run at once, and the first to end must not lower it for the
    /// other. The control port reports it.
    finalizing: std::sync::atomic::AtomicUsize,
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
    /// Restarts for changed video settings, see `lib::restart_buffer_for_settings`.
    pub restarts: RestartQueue,
    /// Video settings changed while a recording held the buffer: it restarts
    /// with them once the recording has stopped.
    pub settings_waiting: std::sync::atomic::AtomicBool,
}

/// Keeps a flag raised for as long as it lives and lowers it when dropped —
/// even if a `?` strikes in between. Holds the save slot (`begin_save`).
pub struct FlagGuard<'a>(&'a std::sync::atomic::AtomicBool);

impl<'a> FlagGuard<'a> {
    /// Raise the flag; it comes down with the guard.
    pub fn raise(flag: &'a std::sync::atomic::AtomicBool) -> Self {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        Self(flag)
    }
}

impl Drop for FlagGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Counts one more for as long as it lives — the same as [`FlagGuard`] for
/// something more than one can hold at a time.
pub struct CountGuard<'a>(&'a std::sync::atomic::AtomicUsize);

impl<'a> CountGuard<'a> {
    pub fn raise(count: &'a std::sync::atomic::AtomicUsize) -> Self {
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self(count)
    }
}

impl Drop for CountGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// What a stop answers when there is nothing to stop. A second press while
/// the first stop writes the recording out gets it — and stays quiet.
pub const NOT_RECORDING: &str = "No recording is running.";

/// The recording folders being written out right now, process-wide (REC-12).
///
/// A stop still muxing when the bundled ffmpeg finishes downloading would
/// otherwise see its folder finished a second time by
/// [`AppState::recover_recordings`]: two MP4s, both writing the same mix and
/// stems, both deleting the folder.
pub struct Finalizing;

static FINALIZING: Mutex<std::collections::BTreeSet<std::path::PathBuf>> =
    Mutex::new(std::collections::BTreeSet::new());

impl Finalizing {
    /// Take `dir` for writing out. `None` while somebody else has it.
    pub fn claim(dir: &std::path::Path) -> Option<FinalizingGuard> {
        FINALIZING
            .lock()
            .insert(dir.to_path_buf())
            .then(|| FinalizingGuard(dir.to_path_buf()))
    }
}

/// A folder taken with [`Finalizing::claim`]; handed back when dropped.
pub struct FinalizingGuard(std::path::PathBuf);

impl Drop for FinalizingGuard {
    fn drop(&mut self) {
        FINALIZING.lock().remove(&self.0);
    }
}

/// A recording that has been closed and not written out yet — what
/// [`AppState::begin_stop`] hands to [`AppState::finish_stop`]. It holds its
/// folder and the "finalizing" count until it is written out or dropped.
pub struct StopTicket<'a> {
    recorder: Arc<Recorder>,
    meta: recorder::Meta,
    folder: FinalizingGuard,
    _finalizing: CountGuard<'a>,
    /// How applying the video settings that waited for the recording went,
    /// if any waited — for the caller to report, off the lock.
    pub restarted: Option<Result<bool, String>>,
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
    /// Automatic starts that failed in a row.
    failures: std::sync::atomic::AtomicU32,
    /// Checks still to sit out before the next automatic start.
    wait: std::sync::atomic::AtomicU32,
}

/// Automatic starts that may fail in a row before the automation gives up
/// until the game ends or the user starts the buffer by hand.
const MAX_START_FAILURES: u32 = 5;

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
            if !buffer_active && !self.suppressed.load(SeqCst) && self.may_try() {
                self.started.store(true, SeqCst);
                return AutoAction::Start;
            }
            return AutoAction::Nothing;
        }

        if self.missing.fetch_add(1, SeqCst) + 1 < GRACE_POLLS {
            return AutoAction::Nothing;
        }
        // The game really has ended: a new one may start the buffer again, even
        // if the user switched it off in between — or the automation gave up.
        self.suppressed.store(false, SeqCst);
        self.start_succeeded();
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
        if !self.for_lock.load(SeqCst) || (!buffer_active && !self.may_try()) {
            return AutoAction::Nothing;
        }
        self.for_lock.store(false, SeqCst);
        if !buffer_active {
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
        if buffer_active || self.suppressed.load(SeqCst) || !self.may_try() {
            return AutoAction::Nothing;
        }
        self.started.store(true, SeqCst);
        AutoAction::Start
    }

    /// Is the automation past its back-off? Counts one check down if not.
    fn may_try(&self) -> bool {
        use std::sync::atomic::Ordering::SeqCst;
        self.wait
            .fetch_update(SeqCst, SeqCst, |left| left.checked_sub(1))
            .is_err()
    }

    /// An automatic start failed. It used to be tried again on the very next
    /// check, every two seconds, each time with the error sound and a banner
    /// that never got to go away — for as long as the game ran. Now the gap
    /// doubles each time, and after [`MAX_START_FAILURES`] the automation
    /// stops trying. Returns whether it just gave up.
    pub fn start_failed(&self) -> bool {
        use std::sync::atomic::Ordering::SeqCst;
        let failures = self.failures.fetch_add(1, SeqCst) + 1;
        if failures >= MAX_START_FAILURES {
            self.suppressed.store(true, SeqCst);
            self.started.store(false, SeqCst);
            self.failures.store(0, SeqCst);
            self.wait.store(0, SeqCst);
            return true;
        }
        self.wait.store(1 << failures, SeqCst);
        false
    }

    /// Automatic starts that have failed in a row so far.
    pub fn start_failures(&self) -> u32 {
        self.failures.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// A start went through: the next failure counts from the beginning.
    pub fn start_succeeded(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        self.failures.store(0, SeqCst);
        self.wait.store(0, SeqCst);
    }

    /// The user started the buffer themselves.
    pub fn manual_start(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        self.started.store(false, SeqCst);
        self.suppressed.store(false, SeqCst);
        self.start_succeeded();
    }

    /// The user stopped the buffer themselves.
    pub fn manual_stop(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        self.started.store(false, SeqCst);
        self.suppressed.store(true, SeqCst);
    }
}

/// Restarts after an encoder failure allowed within [`RESTART_WINDOW`].
const RESTARTS_PER_WINDOW: usize = 3;
const RESTART_WINDOW: std::time::Duration = std::time::Duration::from_secs(600);

/// Keeps a failing encoder from being restarted in a loop: one that dies again
/// every few seconds would otherwise throw the buffer away every few seconds.
#[derive(Default)]
pub struct RestartBudget {
    /// When the restarts within the window happened, oldest first.
    recent: std::collections::VecDeque<std::time::Instant>,
}

impl RestartBudget {
    /// May a restart happen at `now`? A yes counts against the budget; a no
    /// does not.
    pub fn allow(&mut self, now: std::time::Instant) -> bool {
        while self
            .recent
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) > RESTART_WINDOW)
        {
            self.recent.pop_front();
        }
        if self.recent.len() >= RESTARTS_PER_WINDOW {
            return false;
        }
        self.recent.push_back(now);
        true
    }
}

impl AppState {
    /// The state for this run. `first` is false for a second start, which
    /// single-instance ends before `setup`: it reads no config — loading may
    /// write `config.json.bak`, or move a damaged file aside — and opens no
    /// clip database. Should it get past single-instance after all, `setup`
    /// catches up with [`Self::load_late`].
    ///
    /// The audio sources are not started here but in `setup`
    /// ([`Self::apply_audio`]): this runs on the main thread, and a device that
    /// does not answer held the window up to five seconds per stream.
    pub fn new(first: bool) -> Self {
        let (config, trouble, library) = if first {
            Self::from_disk()
        } else {
            (config::default_config(), None, None)
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
        let audio = Arc::new(AudioEngine::new());
        let mut history = std::collections::VecDeque::new();
        config::remember(&mut history, &config);

        Self {
            config: Mutex::new(config),
            config_history: Mutex::new(history),
            config_trouble: Mutex::new(trouble),
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
            game_ended: Mutex::new(None),
            quitting: std::sync::atomic::AtomicBool::new(false),
            auto: AutoBuffer::default(),
            lifecycle: Mutex::new(()),
            active_recording: Mutex::new(None),
            saving: std::sync::atomic::AtomicBool::new(false),
            finalizing: std::sync::atomic::AtomicUsize::new(0),
            recorder: Mutex::new(None),
            buffer_wanted: std::sync::atomic::AtomicBool::new(false),
            drift_seen: std::sync::atomic::AtomicU8::new(0),
            restarts: RestartQueue::default(),
            settings_waiting: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// The settings, the start-up trouble with them and the clip database, as
    /// a first start finds them.
    fn from_disk() -> (AppConfig, Option<String>, Option<Library>) {
        let (config, loaded) = config::load();
        crate::watermark::set_enabled(config.watermark);
        let library = match Library::open() {
            Ok(lib) => Some(lib),
            Err(err) => {
                log::error!("could not open the clip database: {err}");
                None
            }
        };
        (config, loaded.message(), library)
    }

    /// What [`Self::new`] left out for a second start that became the app:
    /// the other ClippiBoy quit between our check and single-instance's.
    pub fn load_late(&self) {
        let (config, trouble, library) = Self::from_disk();
        self.status.lock().encoder = Some(config.recording.encoder);
        let mut guard = self.config.lock();
        let mut history = self.config_history.lock();
        history.clear();
        config::remember(&mut history, &config);
        drop(history);
        *guard = config;
        drop(guard);
        *self.config_trouble.lock() = trouble;
        *self.library.lock() = library;
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
    /// (buffer length and audio sources). The video settings of a running
    /// buffer are changed by `commands::set_config` — the restart can be
    /// reported there too.
    pub fn replace_config(&self, next: AppConfig) -> AppConfig {
        self.replace_config_with(next, |_, _| {})
    }

    /// [`Self::replace_config`], with `settle` run on the new config against
    /// the current one under the config lock, before it is saved. What has to
    /// come from the config as it stands — the clip folder and the token,
    /// changed only by their own commands — is taken from there: from a copy
    /// read earlier, an overlapping `set_clip_dir` or "New" token (`control::restart`) would
    /// be undone.
    ///
    /// Before that, `incoming` is put onto the current config by its revision
    /// (`config::accept`): one built on an older copy — a window that has not
    /// heard of a change made elsewhere, or a snapshot taken before another
    /// writer got the lock — is merged instead of undoing that change.
    pub fn replace_config_with(
        &self,
        mut incoming: AppConfig,
        settle: impl FnOnce(&mut AppConfig, &AppConfig),
    ) -> AppConfig {
        incoming.recording.encoder = crate::encode::resolve(incoming.recording.encoder);
        let (previous, next) = {
            let mut guard = self.config.lock();
            let mut history = self.config_history.lock();
            let mut next = config::accept(&guard, &history, incoming);
            settle(&mut next, &guard);
            // On the merged result: a bitrate carried over from either side
            // would belong to a picture that is no longer configured.
            config::normalize_recording(&mut next.recording);
            // Saved under the lock, as every other writer does — two saves
            // never run at once, and the file always ends on the newest config.
            if let Err(err) = config::save(&next) {
                log::error!("could not save the config: {err}");
            }
            config::remember(&mut history, &next);
            (std::mem::replace(&mut *guard, next.clone()), next)
        };
        // Takes effect with the next frame, no restart.
        crate::watermark::set_enabled(next.watermark);
        crate::telemetry::on_config(&next);
        // Only a changed source list needs the streams brought in line: a gain
        // step is one, a clip-length or sound change is not.
        let sources_changed = previous.sources != next.sources;
        if sources_changed {
            self.apply_audio();
        }
        // A running buffer takes a new length right away — longer fills up from
        // here, shorter is cut down. Without this it kept the length it was
        // started with until the next start, and a Stream Deck key on 3:00
        // saved the old 1:30.
        // The budget comes from the bitrate the pipeline runs with on both
        // sides, so a bitrate changed in the settings alone moves nothing.
        let running = self.active_recording.lock().clone();
        let memory = memory_budget(running.as_ref(), &next);
        if next.buffer.seconds != previous.buffer.seconds
            || memory != memory_budget(running.as_ref(), &previous)
        {
            if let Some(shared) = self.shared.lock().as_ref() {
                shared.set_buffer_seconds(next.buffer.seconds);
                let (seconds, bytes) =
                    ring_size(self.buffer_wanted(), next.buffer.seconds, memory);
                shared.packets.lock().set_capacity(seconds, bytes);
            }
        }
        // If a recording is running it has to learn about the changed sources
        // too — otherwise it mixes the old ones until the next restart. It gets
        // the **unresolved** ones: it only needs id, gain, mute, solo and the
        // track flag, and that way a game source keeps its track even while no
        // game is running.
        if sources_changed {
            if let Some(shared) = self.shared.lock().as_ref() {
                shared.set_sources(next.sources.clone());
            }
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
    pub fn begin_save(&self) -> Option<FlagGuard<'_>> {
        use std::sync::atomic::Ordering::SeqCst;
        match self.saving.compare_exchange(false, true, SeqCst, SeqCst) {
            Ok(_) => Some(FlagGuard(&self.saving)),
            Err(_) => None,
        }
    }

    /// Is a clip being written right now? The control port reports it so a
    /// Stream Deck key can wait instead of hammering.
    pub fn is_saving(&self) -> bool {
        self.saving.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Is a stopped recording still being written out? Then the status says
    /// "not recording" although the file does not exist yet.
    pub fn is_finalizing(&self) -> bool {
        self.finalizing.load(std::sync::atomic::Ordering::SeqCst) > 0
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

    /// For a writer that changes the config in place under its guard: count
    /// the revision up and remember the result, as `replace_config` does, so
    /// a window's next write is merged onto this change rather than over it.
    pub fn note_direct_write(&self, config: &mut AppConfig) {
        config.revision += 1;
        config::remember(&mut self.config_history.lock(), config);
    }

    /// Put one source row in. `base` is the revision the window built the row
    /// on: the row goes onto that copy and is merged from there
    /// (`config::with_sources`), so a row from the console's older copy does
    /// not undo what the main window changed on it since.
    pub fn upsert_source(&self, source: AudioSource, base: Option<u64>) -> AppConfig {
        let incoming = self.source_write(base, |sources| config::put_source(sources, source));
        self.replace_config(incoming)
    }

    pub fn remove_source(&self, id: &str, base: Option<u64>) -> AppConfig {
        let incoming = self.source_write(base, |sources| sources.retain(|s| s.id != id));
        self.replace_config(incoming)
    }

    fn source_write(&self, base: Option<u64>, change: impl FnOnce(&mut Vec<AudioSource>)) -> AppConfig {
        let current = self.config.lock();
        let history = self.config_history.lock();
        config::with_sources(&current, &history, base, change)
    }
}

/// What a start has to do, given whether a pipeline runs and whether it
/// already serves as the buffer.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum StartStep {
    /// Nothing runs: bring the capture stack up.
    Launch,
    /// Running for a recording only: the buffer gets its length back.
    Promote,
    /// The buffer runs already — a slow start overtaken by the next tick, a
    /// second hotkey. Nothing to do and nothing to announce.
    AlreadyRunning,
}

impl StartStep {
    /// Whether this start made the buffer run, and so is announced.
    pub fn started(self) -> bool {
        self != StartStep::AlreadyRunning
    }
}

pub fn start_step(pipeline_running: bool, buffer_wanted: bool) -> StartStep {
    match (pipeline_running, buffer_wanted) {
        (false, _) => StartStep::Launch,
        (true, false) => StartStep::Promote,
        (true, true) => StartStep::AlreadyRunning,
    }
}

impl AppState {
    /// Starts recording into the replay buffer. `Ok(true)` only when this call
    /// brought the pipeline up or turned a recording-only one into a buffer;
    /// `Ok(false)` when the buffer was running already (see [`start_step`]).
    pub fn start_pipeline(&self) -> Result<bool, String> {
        let _lifecycle = self.lifecycle.lock();
        let step = start_step(
            self.pipeline.lock().is_some(),
            self.buffer_wanted.load(std::sync::atomic::Ordering::SeqCst),
        );
        match step {
            StartStep::Launch => self.launch_pipeline(true)?,
            StartStep::Promote => {
                // Running for a recording only: the buffer just gets its
                // length back, and fills from here.
                self.buffer_wanted
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                let config = self.config_snapshot();
                let running = self.active_recording.lock().clone();
                let memory = memory_budget(running.as_ref(), &config);
                if let Some(shared) = self.shared.lock().as_ref() {
                    let (seconds, bytes) = ring_size(true, config.buffer.seconds, memory);
                    shared.packets.lock().set_capacity(seconds, bytes);
                }
                let game = self.current_game.lock().clone();
                *self.buffering_game.lock() = game;
                self.status.lock().buffer_active = true;
            }
            StartStep::AlreadyRunning => {}
        }
        Ok(step.started())
    }

    /// Bring the capture stack up. `for_buffer` false means a recording is the
    /// only reason, and the ring stays small. The caller holds `lifecycle`.
    fn launch_pipeline(&self, for_buffer: bool) -> Result<(), String> {
        // While ffmpeg is still being fetched the buffer may already run —
        // it is only needed once a clip is written.
        // The cheap check first: `available` starts two processes.
        if crate::tools::is_ready() && !crate::muxer::available() {
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
            memory_budget(Some(&config.recording), &config),
            config.sources.clone(),
            self.audio.clone(),
        )?;
        self.buffer_wanted
            .store(for_buffer, std::sync::atomic::Ordering::SeqCst);
        // Before `shared`: a config change that finds the pipeline registered
        // sizes the ring from the recording it really runs.
        *self.active_recording.lock() = Some(config.recording.clone());
        let shared = pipeline.shared.clone();
        *self.shared.lock() = Some(shared.clone());
        *self.pipeline.lock() = Some(pipeline);
        // A length or a source changed during the seconds `Pipeline::start`
        // took found no pipeline to pass it to (BUF-9). Read once more now
        // that it is registered — what comes later reaches it by itself.
        let fresh = self.config_snapshot();
        shared.set_buffer_seconds(fresh.buffer.seconds);
        let (seconds, bytes) = ring_size(
            for_buffer,
            fresh.buffer.seconds,
            memory_budget(Some(&config.recording), &fresh),
        );
        shared.packets.lock().set_capacity(seconds, bytes);
        shared.set_sources(fresh.sources.clone());
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

    /// Restart a running buffer on the settings as they stand now. `Ok(false)`
    /// when there is nothing to restart: the buffer is off, the capture runs
    /// on these settings already, or a recording holds it — then the restart
    /// waits for its end (`settings_waiting`).
    ///
    /// Decided under `lifecycle`, which is what keeps a "Stop buffer" pressed
    /// right after a change from being undone (R9): either the stop ran first
    /// and nothing is wanted here, or this ran first and the stop follows it.
    pub fn restart_if_buffering(&self) -> Result<bool, String> {
        let _lifecycle = self.lifecycle.lock();
        self.restart_for_settings()
    }

    /// [`Self::restart_if_buffering`]; the caller holds `lifecycle`.
    fn restart_for_settings(&self) -> Result<bool, String> {
        // Read one at a time: no guard may be held while `needed` enumerates
        // the capture targets.
        let pipeline_running = self.pipeline.lock().is_some();
        let recording = self.is_recording();
        let needed = || {
            let mut fitted = self.config_snapshot().recording;
            crate::capture::fit_to_target(&mut fitted);
            let running = self.active_recording.lock().clone();
            restart_needed(running.as_ref(), &fitted)
        };
        match settings_step(pipeline_running, recording, self.buffer_wanted(), needed) {
            SettingsStep::Nothing => Ok(false),
            SettingsStep::Wait => {
                // Under `lifecycle`, before the stop can close the recording:
                // its end is sure to see this.
                self.settings_waiting
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(false)
            }
            SettingsStep::Restart => {
                log::info!("capture settings changed — restarting the buffer");
                // Counted as on throughout: a "Stop buffer" or the toggle
                // hotkey meanwhile reads the status, and seeing it off they
                // did nothing, or started it. Now they stop it, once this
                // has let go of `lifecycle`.
                self.take_down(true);
                if let Err(err) = self.launch_pipeline(true) {
                    self.status.lock().buffer_active = false;
                    return Err(err);
                }
                Ok(true)
            }
        }
    }

    /// Take the capture stack down. The caller holds `lifecycle`.
    fn halt_pipeline(&self) {
        self.take_down(false);
    }

    /// [`Self::halt_pipeline`]; `counted_on` leaves the buffer shown as
    /// running, for a restart that brings it straight back.
    fn take_down(&self, counted_on: bool) {
        self.buffer_wanted
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.shared.lock().take();
        self.active_recording.lock().take();
        // Taken out first, stopped after: in the `if let` the guard lived
        // through the joins and the encoder's drain, and the status tick — the
        // meters — and every save waited on it (BUF-7). `lifecycle`, held by
        // the caller, keeps a start out meanwhile.
        let taken = self.pipeline.lock().take();
        if let Some(mut pipeline) = taken {
            pipeline.stop();
        }
        let mut status = self.status.lock();
        status.buffer_active = counted_on;
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

    /// The game a capture covering the last `window` belongs to.
    fn game_for(&self, window: std::time::Duration) -> Option<String> {
        // One lock at a time, see `launch_pipeline`.
        let current = self.current_game.lock().clone();
        let buffering = self.buffering_game.lock().clone();
        let ended_ago = self.game_ended.lock().map(|at| at.elapsed());
        assigned_game(buffering, current, ended_ago, window)
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
        if held.is_some() {
            *self.game_ended.lock() = None;
            if self.status.lock().buffer_active || self.is_recording() {
                *self.buffering_game.lock() = held.clone();
            }
        } else if self.buffering_game.lock().is_some() {
            // Das Spiel ist zu. Sein Name bleibt am Puffer, solange dort noch
            // Spielbild liegen kann — danach stand er auf Clips vom Desktop.
            // Eine laufende Aufnahme hält ihn bis zum Stopp: sie enthält das
            // Spiel, egal wie lange es schon zu ist.
            let ended = *self.game_ended.lock().get_or_insert_with(std::time::Instant::now);
            let longest = self.config_snapshot().buffer.seconds;
            if !self.is_recording() && ended.elapsed() >= std::time::Duration::from_secs(longest.into()) {
                *self.buffering_game.lock() = None;
                *self.game_ended.lock() = None;
            }
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

    /// Room the running recording needs to be saved: the file it becomes, plus
    /// the scratch space the stop writes beside it — the summed main mix, and
    /// for a recording with gaps the cut copy of a track.
    pub fn recording_room_needed(&self) -> Option<u64> {
        // The lock is dropped before the config is read.
        let recorder = self.recorder.lock().clone()?;
        let fps = self.shared.lock().as_ref().map(|s| s.fps).unwrap_or(1).max(1);
        let separate = separate_tracks(&self.config_snapshot().sources);
        let seconds = recorder.frames() / fps as u64;
        Some(
            recorder.estimated_size(fps)
                + recorder::mix_scratch_bytes(recorder.tracks_in_mix(&separate), seconds)
                + recorder.cut_scratch_bytes(),
        )
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
            // Ticks the clock skipped are frames lost just the same.
            #[cfg(windows)]
            if let Some(pacer) = shared.pacer.lock().as_ref() {
                status.dropped_frames += pacer.skipped.load(std::sync::atomic::Ordering::Relaxed);
            }
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
        // A length asked for from outside (a Stream Deck key set to 2:30) can
        // outlast a buffer shortened since; the buffer is all there is.
        let seconds = seconds
            .map(|asked| asked.min(config.buffer.seconds))
            .unwrap_or_else(|| config::effective_clip_seconds(&config.buffer))
            .max(1);
        // The buffer keeps what it has; the same press works once ffmpeg is in.
        if !crate::tools::is_ready() && !crate::muxer::available() {
            return Err(crate::tools::not_ready_reason());
        }
        // The ring keeps its old packets when the encoder stops; "the last 30
        // seconds" would be the 30 seconds before it did.
        if let Some(why) = self.encoder_failure() {
            return Err(format!(
                "The video encoder stopped ({why}), so the buffer has nothing newer to save."
            ));
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

        // Not the snapshot but the game detected while buffering — when saving
        // via the button, ClippiBoy is in the foreground.
        let game = self.game_for(std::time::Duration::from_secs(seconds.into()));
        let output = destination(&config.clip_dir, game.as_deref(), ClipKind::Clip)?;

        // The id already here: the muxer files the individual tracks under it,
        // and those come out of the same ffmpeg run as the clip.
        let id = uuid::Uuid::new_v4().to_string();
        // The file's dimensions are what the encoder received, taken with the
        // packets: read afterwards, a restart in between gave the row the next
        // pipeline's size (BUF-8).
        let (width, height) = (snapshot.width, snapshot.height);
        let result = crate::muxer::build(crate::muxer::ClipRequest {
            snapshot,
            clip_id: id.clone(),
            output,
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
            width,
            height,
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
            tags: Vec::new(),
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

        let mut shot = crate::shot::grab(
            recording.target_kind,
            recording.target_id.as_deref(),
            recording.target_stable_id.as_deref(),
        )?;

        // Ein Bild ist ein Augenblick: es gehört zu dem Spiel, das jetzt läuft.
        let game = self.current_game.lock().clone();
        let path = destination(&config.clip_dir, game.as_deref(), ClipKind::Screenshot)?;
        // Into the file itself. What lay under it goes into the editor's store:
        // an edit lays it back and stamps the result anew, so the mark can be
        // neither painted over nor cropped away.
        let id = uuid::Uuid::new_v4().to_string();
        let under = crate::watermark::stamp(&mut shot);
        let size = (shot.width, shot.height);
        shot.write_png(&path)?;
        if let Some((at, piece)) = under {
            let at = crate::shot::Rect {
                x: at.x,
                y: at.y,
                width: at.width,
                height: at.height,
            };
            if let Err(err) = crate::shot::write_under(&id, at, &piece, size) {
                log::warn!("could not keep what lies under the watermark: {err}");
            }
        }

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
            tags: Vec::new(),
        })
    }
}

impl AppState {
    /// Start a recording by hand. Brings the pipeline up if the buffer is not
    /// running; it goes down again with the recording.
    ///
    /// `friends_in` names the friends playing a game right now — asked for the
    /// game the recording is filed under, so the two cannot disagree.
    pub fn start_recording(&self, friends_in: &dyn Fn(&str) -> Vec<String>) -> Result<(), String> {
        let _lifecycle = self.lifecycle.lock();
        if self.is_recording() {
            return Err("A recording is already running.".into());
        }
        if !crate::tools::is_ready() && !crate::muxer::available() {
            return Err(crate::tools::not_ready_reason());
        }
        // Read off the status thread's last look, not asked here: on a lost
        // network drive the question stalls (`disk.rs`). 0 means not known yet.
        let free = self.free_bytes.load(std::sync::atomic::Ordering::Relaxed);
        if free != 0 && free < crate::RECORDING_MIN_START_BYTES {
            return Err(format!(
                "Only {} left on the clip drive.",
                crate::format::size(free)
            ));
        }
        let started_here = self.pipeline.lock().is_none();
        if started_here {
            self.launch_pipeline(false)?;
        }

        let outcome = self.attach_recorder(friends_in);
        if outcome.is_err() && started_here {
            self.halt_pipeline();
        }
        let recorder = outcome?;
        *self.recorder.lock() = Some(recorder);
        self.status.lock().recording = true;
        Ok(())
    }

    fn attach_recorder(&self, friends_in: &dyn Fn(&str) -> Vec<String>) -> Result<Arc<Recorder>, String> {
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
        // Die Aufnahme beginnt jetzt; ein länger geschlossenes Spiel hat darin
        // nichts verloren.
        let game = self.game_for(std::time::Duration::ZERO);
        let friends = match game.as_deref() {
            Some(game) if config.friends.tag_friends => friends_in(game),
            _ => Vec::new(),
        };
        let meta = recorder::Meta {
            id: uuid::Uuid::new_v4().to_string(),
            created_at: now_ms(),
            // The running pipeline's, which `lifecycle` keeps the one below.
            fps: shared.fps,
            width: recording.width,
            height: recording.height,
            game,
            // Written now, not only at stop: after a crash the folder is all
            // there is, and without it every track ends up in the main mix.
            separate: separate_tracks(&config.sources),
            friends,
            ..recorder::Meta::default()
        };
        // The folder, its description and the writer before any lock (BUF-3):
        // on a sleeping drive or a network folder that takes seconds, and
        // under the ring lock it stalled the encoder. A start refused below
        // clears it away again.
        let prepared = recorder::Recorder::prepare(std::path::Path::new(&config.clip_dir), meta)?;
        let outcome = match self.pipeline.lock().as_ref() {
            Some(pipeline) => pipeline.start_recording(prepared),
            None => Err((prepared, "The capture is not running.".to_string())),
        };
        // Cleared away with no lock held: it joins the writer and deletes.
        outcome.map_err(|(prepared, why)| {
            prepared.discard();
            why
        })
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
        // Ein Spiel, das während der Aufnahme lief, gehört zu ihr — eins, das
        // schon vor ihrem Beginn geschlossen wurde, nicht.
        let length = std::time::Duration::from_millis(
            now_ms().saturating_sub(recorder.started_at()).max(0) as u64,
        );
        let game = self.game_for(length);
        // Whatever `finish` could persist: capture and the REC state end here
        // either way, the folder stays for `recover_recordings`.
        let meta = recorder.finish(separate, game);
        if !self.buffer_wanted() {
            self.halt_pipeline();
        }
        self.status.lock().recording = false;
        Some((recorder, meta))
    }

    /// Stop the recording and write it out. Takes as long as the audio of the
    /// whole recording takes to encode — off the hotkey thread.
    ///
    /// Video settings changed during the recording reach the buffer right
    /// after the close, before the writing out; `on_restart` hears how that
    /// went, off the lock.
    pub fn stop_recording(
        &self,
        on_progress: &dyn Fn(f32),
        on_restart: &dyn Fn(Result<bool, String>),
    ) -> Result<Clip, String> {
        let mut ticket = self.begin_stop()?;
        if let Some(outcome) = ticket.restarted.take() {
            on_restart(outcome);
        }
        self.finish_stop(ticket, on_progress)
    }

    /// The fast half of a stop: close the recording, take its folder and
    /// apply the video settings that waited for it. Fails with
    /// [`NOT_RECORDING`] when there is nothing to stop — a second press while
    /// the first stop writes out, which its caller keeps quiet (REC-8).
    pub fn begin_stop(&self) -> Result<StopTicket<'_>, String> {
        let _lifecycle = self.lifecycle.lock();
        if !self.is_recording() {
            return Err(NOT_RECORDING.into());
        }
        let (recorder, meta) = self.close_recording().ok_or_else(|| {
            "The recording could not be closed — it is finished on the next start.".to_string()
        })?;
        // Raised before the lifecycle lock is let go, so the status never
        // shows "not recording" without "finalizing" in between.
        let finalizing = CountGuard::raise(&self.finalizing);
        // Taken under `lifecycle` too, where the recovery picks its folders:
        // from the close on, this one is never on its list.
        let folder = Finalizing::claim(&recorder.dir).ok_or_else(|| {
            "The recording is already being written out.".to_string()
        })?;
        let restarted = self
            .settings_waiting
            .swap(false, std::sync::atomic::Ordering::SeqCst)
            .then(|| self.restart_for_settings());
        Ok(StopTicket {
            recorder,
            meta,
            folder,
            _finalizing: finalizing,
            restarted,
        })
    }

    /// The slow half of a stop: write the closed recording out.
    pub fn finish_stop(&self, ticket: StopTicket<'_>, on_progress: &dyn Fn(f32)) -> Result<Clip, String> {
        self.finish_recording(&ticket.folder, &ticket.recorder.dir, &ticket.meta, on_progress)
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

    /// Why the running encoder stopped working, if it has.
    pub fn encoder_failure(&self) -> Option<String> {
        self.shared.lock().as_ref().and_then(|shared| shared.encoder_failure())
    }

    /// Write a recording folder out. Only with the folder claimed: `_claim`
    /// is the proof that nobody else is finishing it at the same time.
    fn finish_recording(
        &self,
        _claim: &FinalizingGuard,
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
            // Not in the library yet — the caller stores them with the clip.
            tags: meta.friends.iter().map(|name| crate::clips::friend_tag(name)).collect(),
        })
    }

    /// Finish what a crash, or a quit mid-recording, left behind. Needs
    /// ffmpeg, so it runs once that is in place.
    pub fn recover_recordings(&self, on_progress: &dyn Fn(f32)) -> Vec<Clip> {
        let clip_dir = self.config_snapshot().clip_dir;
        let clip_path = std::path::Path::new(&clip_dir);
        // The list is made under `lifecycle`, the muxing is not. A recording
        // being made ready at this start is then either running — its folder
        // skipped — or not there yet; one being stopped holds its folder
        // (REC-12). Each folder taken here stays taken until it is written
        // out. Emptied folders go here too: a start after the lock is let go
        // may be making `.recording` anew.
        let work = {
            let _lifecycle = self.lifecycle.lock();
            let live = self.recorder.lock().as_ref().map(|r| r.dir.clone());
            let mut work = Vec::new();
            for dir in recorder::leftovers(clip_path) {
                if live.as_ref() == Some(&dir) {
                    continue;
                }
                let Some(claim) = Finalizing::claim(&dir) else {
                    log::info!("recording in '{}' is being written out already", dir.display());
                    continue;
                };
                let meta = match recorder::read_meta(&dir) {
                    Ok(meta) => meta,
                    Err(err) => {
                        log::warn!("recording in '{}' left alone: {err}", dir.display());
                        continue;
                    }
                };
                if recorder::is_empty_leftover(&dir, &meta) {
                    // Nothing to finish; retrying it at every start would never end.
                    if std::fs::remove_dir_all(&dir).is_ok() {
                        let _ = std::fs::remove_dir(clip_path.join(recorder::WORK_DIR));
                        log::info!("empty recording folder '{}' cleared", dir.display());
                    } else {
                        log::warn!("empty recording folder '{}' not cleared", dir.display());
                    }
                    continue;
                }
                work.push((claim, dir, meta));
            }
            work
        };
        let mut done = Vec::new();
        for (claim, dir, meta) in work {
            // Counted like a stop's: the status says a recording is being
            // written while this one is.
            let _finalizing = CountGuard::raise(&self.finalizing);
            match self.finish_recording(&claim, &dir, &meta, on_progress) {
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

/// The game a capture belongs to.
///
/// A running game wins. A closed one keeps the capture only if it ended inside
/// the captured `window` — the clip still shows it then. Without this, a clip
/// of the desktop half an hour later still carried the game's name, in the
/// banner, the tag and the folder.
fn assigned_game(
    buffering: Option<String>,
    current: Option<String>,
    ended_ago: Option<std::time::Duration>,
    window: std::time::Duration,
) -> Option<String> {
    if current.is_some() {
        return current;
    }
    match ended_ago {
        Some(ago) if ago >= window => None,
        _ => buffering,
    }
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

/// A game name as the front of a file name: at most [`crate::filing::MAX_LEN`]
/// characters, like its folder. Unknown games are named after their window
/// title, and a title that is a whole sentence made every save fail on a path
/// too long for Windows.
fn sanitize_name(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .chars()
        .take(crate::filing::MAX_LEN)
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

/// Poll `busy` every `step` until it says no, for at most `limit`. `true` when
/// it did in time.
///
/// What Quit and the update restart wait on: a save or an edit cut off half
/// way loses the clip the user asked for.
pub fn wait_until_idle(busy: impl Fn() -> bool, limit: std::time::Duration, step: std::time::Duration) -> bool {
    let until = std::time::Instant::now() + limit;
    while busy() {
        if std::time::Instant::now() >= until {
            return false;
        }
        std::thread::sleep(step);
    }
    true
}

impl Default for AppState {
    fn default() -> Self {
        Self::new(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REC-12: a stop still writing its folder out and the recovery that runs
    /// once ffmpeg has arrived must not both finish it.
    #[test]
    fn a_folder_being_finalized_is_not_taken_twice() {
        let dir = std::path::PathBuf::from("C:/clips/.recording/abc");
        let first = Finalizing::claim(&dir).expect("free");
        assert!(Finalizing::claim(&dir).is_none(), "a second finalize of the same folder waits its turn");
        drop(first);
        assert!(Finalizing::claim(&dir).is_some(), "free again once the first is done");
    }

    /// A stop and the recovery can write out at the same time: the first to
    /// end must not report "finalizing" over for the other.
    #[test]
    fn finalizing_stays_raised_while_any_writer_runs() {
        let count = std::sync::atomic::AtomicUsize::new(0);
        let stop = CountGuard::raise(&count);
        let recovery = CountGuard::raise(&count);
        drop(stop);
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
        drop(recovery);
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn a_start_that_found_the_buffer_running_is_not_announced() {
        assert_eq!(start_step(false, false), StartStep::Launch);
        assert_eq!(start_step(true, false), StartStep::Promote);
        assert_eq!(start_step(true, true), StartStep::AlreadyRunning);
        assert!(StartStep::Launch.started());
        assert!(StartStep::Promote.started());
        assert!(!StartStep::AlreadyRunning.started());
    }

    #[test]
    fn a_raised_flag_comes_down_with_its_guard() {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        let flag = AtomicBool::new(false);
        {
            let _guard = FlagGuard::raise(&flag);
            assert!(flag.load(SeqCst));
        }
        assert!(!flag.load(SeqCst));
    }

    #[test]
    fn the_memory_budget_follows_the_bitrate_the_pipeline_started_with() {
        let mut config = crate::config::default_config();
        config.buffer.memory_mb = 0;
        let mut running = config.recording.clone();
        running.bitrate_kbps = config.recording.bitrate_kbps * 2;
        assert_eq!(
            memory_budget(Some(&running), &config),
            crate::config::effective_memory_bytes(&running, &config.buffer)
        );
        assert_eq!(
            memory_budget(None, &config),
            crate::config::effective_memory_bytes(&config.recording, &config.buffer)
        );
    }

    #[test]
    fn a_new_resolution_needs_a_restart_and_a_bitrate_alone_does_not() {
        let a = crate::config::default_config().recording;
        let mut b = a.clone();
        b.bitrate_kbps += 1000;
        assert!(!capture_changed(&a, &b));
        b.height = 720;
        assert!(capture_changed(&a, &b));
    }

    #[test]
    fn requests_during_a_restart_make_one_more_pass() {
        let queue = RestartQueue::default();
        assert!(queue.request(), "the first request starts a worker");
        assert!(queue.next(), "and the worker runs once");
        assert!(!queue.request(), "a request while it runs starts no second worker");
        assert!(!queue.request());
        assert!(queue.next(), "both coalesce into one more pass");
        assert!(!queue.next(), "then the worker ends");
        assert!(queue.request(), "a later request starts a new one");
    }

    #[test]
    fn a_worker_that_died_does_not_block_the_next_one() {
        let queue = RestartQueue::default();
        assert!(queue.request());
        assert!(queue.next());
        queue.abandon();
        assert!(queue.request(), "the next request starts a worker again");
    }

    #[test]
    fn the_page_fitting_the_stored_size_to_the_source_restarts_nothing() {
        let running = crate::config::default_config().recording;
        // What the Video page writes on opening: the size the capture was
        // already fitted to — and a bitrate that does not count.
        let mut fitted = running.clone();
        fitted.bitrate_kbps += 1000;
        assert!(!restart_needed(Some(&running), &fitted));
        fitted.fps = 30;
        assert!(restart_needed(Some(&running), &fitted));
        assert!(restart_needed(None, &fitted));
    }

    #[test]
    fn a_settings_change_waits_for_a_recording_even_with_the_buffer_off() {
        use SettingsStep::*;
        let never = || -> bool { panic!("nothing to compare when nothing restarts") };
        assert_eq!(settings_step(true, true, false, never), Wait);
        assert_eq!(settings_step(true, true, true, never), Wait);
        assert_eq!(settings_step(false, false, true, never), Nothing);
        assert_eq!(settings_step(true, false, false, never), Nothing);
        assert_eq!(settings_step(true, false, true, || false), Nothing);
        assert_eq!(settings_step(true, false, true, || true), Restart);
    }

    #[test]
    fn a_ring_kept_only_for_a_recording_stays_small() {
        assert_eq!(ring_size(false, 300, 1 << 30), (IDLE_RING_SECONDS, u64::MAX));
        assert_eq!(ring_size(true, 300, 1 << 30), (300, 1 << 30));
    }

    fn secs(n: u64) -> std::time::Duration {
        std::time::Duration::from_secs(n)
    }

    #[test]
    fn a_running_game_names_the_clip() {
        let game = assigned_game(Some("Valorant".into()), Some("Valorant".into()), None, secs(90));
        assert_eq!(game.as_deref(), Some("Valorant"));
    }

    #[test]
    fn a_game_closed_inside_the_clip_still_names_it() {
        let game = assigned_game(Some("Valorant".into()), None, Some(secs(30)), secs(90));
        assert_eq!(game.as_deref(), Some("Valorant"));
    }

    #[test]
    fn a_game_closed_before_the_clip_does_not() {
        assert_eq!(assigned_game(Some("Valorant".into()), None, Some(secs(120)), secs(90)), None);
    }

    #[test]
    fn alt_tab_keeps_the_game() {
        // Im Hintergrund, aber der Prozess läuft: kein Ende gemerkt.
        let game = assigned_game(Some("Valorant".into()), None, None, secs(90));
        assert_eq!(game.as_deref(), Some("Valorant"));
    }

    #[test]
    fn a_new_game_wins_over_the_closed_one() {
        let game = assigned_game(Some("Valorant".into()), Some("CS2".into()), Some(secs(5)), secs(90));
        assert_eq!(game.as_deref(), Some("CS2"));
    }

    #[test]
    fn a_recording_starts_clean() {
        assert_eq!(assigned_game(Some("Valorant".into()), None, Some(secs(1)), secs(0)), None);
    }

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

    #[test]
    fn a_start_that_keeps_failing_backs_off_and_then_gives_up() {
        let auto = AutoBuffer::default();
        let mut starts = 0;
        let mut gave_up = false;
        // Two minutes of checks with a game running and a buffer that never
        // comes up.
        for _ in 0..60 {
            if auto.poll(true, false) == AutoAction::Start {
                starts += 1;
                gave_up |= auto.start_failed();
            }
        }
        assert!(gave_up, "still trying after {starts} starts");
        assert_eq!(starts, MAX_START_FAILURES, "not every check, not forever");
        // The game ends: the next one gets a fresh go.
        for _ in 0..GRACE_POLLS {
            auto.poll(false, false);
        }
        assert_eq!(auto.poll(true, false), AutoAction::Start);
    }

    #[test]
    fn a_start_that_worked_resets_the_back_off() {
        let auto = AutoBuffer::default();
        assert_eq!(auto.poll_always(false), AutoAction::Start);
        auto.start_failed();
        // Sits out two checks, then tries again.
        assert_eq!(auto.poll_always(false), AutoAction::Nothing);
        assert_eq!(auto.poll_always(false), AutoAction::Nothing);
        assert_eq!(auto.poll_always(false), AutoAction::Start);
        auto.start_succeeded();
        assert_eq!(auto.start_failures(), 0);
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

    #[test]
    fn restarts_stop_after_three_in_ten_minutes() {
        let mut budget = RestartBudget::default();
        let start = std::time::Instant::now();
        let at = |s: u64| start + std::time::Duration::from_secs(s);
        assert!(budget.allow(at(0)));
        assert!(budget.allow(at(60)));
        assert!(budget.allow(at(120)));
        assert!(!budget.allow(at(180)), "a fourth within ten minutes");
        assert!(budget.allow(at(601)), "the first one has aged out");
    }

    #[test]
    fn a_long_window_title_makes_a_short_file_name() {
        assert_eq!(sanitize_name(&"Ы".repeat(300)).chars().count(), 60);
        assert_eq!(sanitize_name("Counter-Strike 2"), "Counter_Strike_2");
        assert_eq!(sanitize_name(&format!("{}!", "a".repeat(59))), "a".repeat(59), "no underscore left after the cut");
    }

    #[test]
    fn quitting_waits_for_a_save_but_not_forever() {
        let polls = std::cell::Cell::new(0);
        let busy_twice = || {
            polls.set(polls.get() + 1);
            polls.get() < 3
        };
        let ms = std::time::Duration::from_millis;
        assert!(wait_until_idle(busy_twice, ms(1_000), ms(1)));
        assert!(!wait_until_idle(|| true, ms(20), ms(5)));
    }
}
