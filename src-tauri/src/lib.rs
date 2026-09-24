pub mod audio;
pub mod buffer;
pub mod capture;
pub mod clipboard;
pub mod clips;
pub mod commands;
pub mod config;
pub mod console;
pub mod control;
pub mod convert;
pub mod edit;
pub mod encode;
pub mod export;
pub mod filing;
pub mod game;
pub mod gpu;
pub mod logging;
pub mod mft;
pub mod model;
pub mod muxer;
pub mod overlay;
pub mod pipeline;
pub mod recorder;
pub mod preview;
pub mod shot;
pub mod state;
pub mod stems;
pub mod thumbs;
pub mod tools;
pub mod tray;
pub mod updater;
pub mod wgc;

use std::time::Duration;

use tauri::{Emitter, Manager};

use overlay::BannerKind;
use state::AppState;

/// Message to the UI (toast).
#[derive(Clone, serde::Serialize)]
struct Notice {
    kind: &'static str,
    message: String,
}

pub fn notify(app: &tauri::AppHandle, kind: &'static str, message: impl Into<String>) {
    let _ = app.emit(
        "notice",
        Notice {
            kind,
            message: message.into(),
        },
    );
}

/// Save a clip and report the result — shared by hotkey, tray and button so all
/// three routes behave identically.
pub fn save_clip_and_notify(app: &tauri::AppHandle) -> Result<model::Clip, String> {
    let state = app.state::<AppState>();

    // Debounce before anything happens: if a save is already running, the key
    // press is the user's impatience and not a second clip.
    let Some(_saving) = state.begin_save() else {
        log::info!("a clip is already being written — key press ignored");
        return Err("A clip is already being saved.".into());
    };

    // Give feedback right away. Sealing and muxing take several seconds
    // depending on bitrate and buffer length; without a sign in the UI you press
    // a second and a third time in that window.
    overlay::show(app, BannerKind::Clip, "Saving clip…", None);

    match state.save_clip(None) {
        Ok(clip) => {
            let seconds = clip.duration_ms / 1000;
            if let Some(library) = state.library.lock().as_ref() {
                if let Err(err) = library.insert(&clip) {
                    log::error!("could not index the clip: {err}");
                }
            }
            let _ = app.emit("clip-saved", clip.clone());
            notify(app, "ok", format!("Clip saved · {seconds} s"));
            overlay::show_with_thumb(
                app,
                BannerKind::Clip,
                clip.game.clone().unwrap_or_else(|| "Clip saved".into()),
                Some(format!("Clip saved · {seconds} s")),
                clip.thumb_path.clone(),
            );
            Ok(clip)
        }
        Err(err) => {
            notify(app, "error", err.clone());
            overlay::show(app, BannerKind::Error, "Clip failed", Some(err.clone()));
            Err(err)
        }
    }
}

/// Take a screenshot and report the result.
///
/// The same shape as `save_clip_and_notify`, and deliberately over the same
/// `clip-saved` event: for the gallery a screenshot is a clip like any other,
/// only one without a duration. A second channel would buy nothing.
///
pub fn take_screenshot_and_notify(app: &tauri::AppHandle) -> Result<model::Clip, String> {
    let state = app.state::<AppState>();

    // Say so right away. Usually the picture is there before you have read this
    // — but Windows.Graphics.Capture only delivers when something is redrawn,
    // and in front of a still screen the wait runs into seconds. Then this is
    // the difference between "it is working" and "nothing happened".
    //
    // It does not end up in the picture: the overlay window is content
    // protected, so the capture never sees it (see `overlay::create`).
    overlay::show(app, BannerKind::Screenshot, "Taking screenshot…", None);

    match state.take_screenshot() {
        Ok(clip) => {
            if let Some(library) = state.library.lock().as_ref() {
                if let Err(err) = library.insert(&clip) {
                    log::error!("could not index the screenshot: {err}");
                }
            }
            let _ = app.emit("clip-saved", clip.clone());
            notify(app, "ok", "Screenshot saved".to_string());
            overlay::show_with_thumb(
                app,
                BannerKind::Screenshot,
                clip.game.clone().unwrap_or_else(|| "Screenshot".into()),
                Some(format!("Screenshot · {}×{}", clip.width, clip.height)),
                clip.thumb_path.clone(),
            );
            Ok(clip)
        }
        Err(err) => {
            notify(app, "error", err.clone());
            overlay::show(app, BannerKind::Error, "Screenshot failed", Some(err.clone()));
            Err(err)
        }
    }
}

/// Start or stop a recording and report the result — hotkey, tray, button and
/// Stream Deck all come through here.
pub fn toggle_recording_and_notify(app: &tauri::AppHandle) {
    if app.state::<AppState>().is_recording() {
        let _ = stop_recording_and_notify(app);
    } else {
        start_recording_and_notify(app);
    }
}

pub fn start_recording_and_notify(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    match state.start_recording() {
        Ok(()) => {
            overlay::set_recording(app, true);
            notify(app, "ok", "Recording started");
            overlay::show(
                app,
                BannerKind::Recording,
                "Recording",
                state.current_game.lock().clone(),
            );
            report_audio_trouble(app);
        }
        Err(err) => {
            notify(app, "error", err.clone());
            overlay::show(app, BannerKind::Error, "Recording will not start", Some(err));
        }
    }
}

/// Stop the recording and write it out. Encoding an hour of audio takes a
/// while — call it off the hotkey thread.
pub fn stop_recording_and_notify(app: &tauri::AppHandle) -> Result<model::Clip, String> {
    let state = app.state::<AppState>();
    overlay::set_recording(app, false);
    overlay::show(app, BannerKind::Recording, "Saving recording…", None);
    let outcome = state.stop_recording(&|share| recording_progress(app, share));
    // Always the closing 1, success or not — the window takes it as "done"
    // and drops the bar.
    recording_progress(app, 1.0);
    match outcome {
        Ok(clip) => {
            if let Some(library) = state.library.lock().as_ref() {
                if let Err(err) = library.insert(&clip) {
                    log::error!("could not index the recording: {err}");
                }
            }
            let _ = app.emit("clip-saved", clip.clone());
            let length = format::duration(clip.duration_ms);
            notify(app, "ok", format!("Recording saved · {length}"));
            overlay::show_with_thumb(
                app,
                BannerKind::Recording,
                clip.game.clone().unwrap_or_else(|| "Recording saved".into()),
                Some(format!("Recording saved · {length}")),
                clip.thumb_path.clone(),
            );
            Ok(clip)
        }
        Err(err) => {
            notify(app, "error", err.clone());
            overlay::show(app, BannerKind::Error, "Recording failed", Some(err.clone()));
            Err(err)
        }
    }
}

/// Finish recordings a crash or a quit left behind. Needs ffmpeg — called once
/// that is in place.
pub fn recover_recordings(app: &tauri::AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let clips = state.recover_recordings(&|share| recording_progress(&app, share));
        if !clips.is_empty() {
            recording_progress(&app, 1.0);
        }
        for clip in clips {
            if let Some(library) = state.library.lock().as_ref() {
                if let Err(err) = library.insert(&clip) {
                    log::error!("could not index the recovered recording: {err}");
                }
            }
            let _ = app.emit("clip-saved", clip.clone());
            notify(&app, "ok", "A recording from last time was finished");
        }
    });
}

/// How far writing a recording out has got, 0 to 1 — a stop by hotkey shows
/// in the window just like one by button.
fn recording_progress(app: &tauri::AppHandle, share: f32) {
    let _ = app.emit("recording-progress", share);
}

mod format {
    /// `1:02:03` or `2:03`.
    pub fn duration(ms: u64) -> String {
        let total = ms / 1000;
        let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
        if hours > 0 {
            format!("{hours}:{minutes:02}:{seconds:02}")
        } else {
            format!("{minutes}:{seconds:02}")
        }
    }
}

/// Switch the replay buffer on or off and report the result.
///
/// The route for hotkey, tray and button — so always a decision by the user.
/// The automation therefore signs off, or back on, here.
pub fn toggle_buffer_and_notify(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    if state.status.lock().buffer_active {
        state.auto.manual_stop();
        stop_buffer_and_notify(app);
    } else {
        state.auto.manual_start();
        start_buffer_and_notify(app);
    }
}

/// Start the buffer and report the result.
pub fn start_buffer_and_notify(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    match state.start_pipeline() {
        Ok(()) => {
            notify(app, "ok", "Replay buffer running");
            let seconds = state.config_snapshot().buffer.seconds;
            overlay::show(
                app,
                BannerKind::Buffer,
                "Replay buffer running",
                Some(match state.current_game.lock().as_ref() {
                    Some(game) => format!("{game} · last {seconds} s"),
                    None => format!("Keeping the last {seconds} s"),
                }),
            );
            report_audio_trouble(app);
        }
        Err(err) => {
            notify(app, "error", err.clone());
            overlay::show(app, BannerKind::Error, "Buffer will not start", Some(err));
        }
    }
}

/// Say what is wrong with the sound, at the moment recording starts.
///
/// The inline banner in the window covers somebody who is looking at the
/// window. Whoever starts the buffer by hotkey over a game never sees it — and
/// that is precisely the person who finds out hours later that a track was
/// silent. So the same finding goes out as a toast and over the banner, which
/// is the one surface visible during play.
///
/// Only at the start, and only in one line: the source list is already in the
/// mixer, and a banner per source over a game would be worse than the problem.
fn report_audio_trouble(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let sources = state.config_snapshot().sources;
    let label = |id: &String| {
        sources
            .iter()
            .find(|s| &s.id == id)
            .map(|s| s.label.clone())
            .unwrap_or_else(|| id.clone())
    };

    let errors = state.audio.errors();
    // Everything but "no game yet": the buffer has just started, and more often
    // than not the game is launched a minute later. That one is for the banner
    // in the window, not for a toast at every start.
    //
    // Worked out fresh rather than read from the two-second cache: after a boot
    // the buffer starts before that tick has run even once, and a boot is
    // exactly when a device has gone missing.
    let notes: std::collections::HashMap<String, String> = audio::configuration_warnings(
        &sources,
        &audio::devices::list_devices(),
        state.current_game.lock().is_some(),
        true,
    )
    .into_iter()
    .filter(|(id, _)| {
        !sources
            .iter()
            .any(|s| &s.id == id && matches!(s.kind, model::SourceKind::Game))
    })
    .collect();
    // A source that cannot start at all outranks one that merely runs
    // differently than it says.
    let (names, headline) = if !errors.is_empty() {
        (
            errors.keys().map(label).collect::<Vec<_>>(),
            "Audio source not recording",
        )
    } else if !notes.is_empty() {
        (
            notes.keys().map(label).collect::<Vec<_>>(),
            "Check the audio sources",
        )
    } else {
        return;
    };

    let mut names = names;
    names.sort();
    let detail = format!("{} — see the mixer", names.join(", "));
    notify(app, "error", format!("{headline}: {detail}"));
    overlay::show(app, BannerKind::Error, headline, Some(detail));
}

/// Stop the buffer and report the result.
pub fn stop_buffer_and_notify(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    state.stop_pipeline();
    notify(app, "ok", "Replay buffer stopped");
    overlay::show(app, BannerKind::BufferOff, "Replay buffer off", None);
}

/// Switch the buffer on at program start when it is meant to run permanently.
///
/// "Only in game" is deliberately left out here — `apply_auto_buffer` takes care
/// of that as soon as a game shows up in the foreground.
fn start_buffer_if_configured(app: &tauri::AppHandle) {
    let config = app.state::<AppState>().config_snapshot();
    if !config.buffer.auto_start || config.only_buffer_in_game {
        return;
    }
    // After a reboot ClippiBoy is up before anybody has signed in, and a capture
    // started against the lock screen never recovers on its own — see
    // `capture::secure_desktop`. `apply_auto_buffer` starts it on the next tick
    // after the sign-in.
    if capture::secure_desktop() {
        log::info!("the screen is still locked — the buffer waits for the sign-in");
        return;
    }
    let app = app.clone();
    // Not on the setup thread: bringing up the recording takes a moment and the
    // window should already be there while it happens.
    std::thread::spawn(move || start_buffer_and_notify(&app));
}

/// Bring every saved screen choice in line with the screens that are here —
/// recording source, banner, console — and save if anything moved.
///
/// At startup, and again whenever the screen watcher has seen them change.
fn repair_screens(state: &AppState) {
    let mut config = state.config.lock();
    let fixed_source = capture::repair(&mut config.recording);
    let fixed_banner = capture::repair_overlay(&mut config.overlay);
    // The console's own screen, the same way. It keeps the pair in `AppConfig`
    // rather than in a block of its own, so it is repaired by hand instead of
    // through a wrapper.
    let fixed_console = {
        let mut id = config.console_monitor.take();
        let mut stable = config.console_monitor_stable_id.take();
        let changed = capture::repair_monitor_choice(&mut id, &mut stable);
        config.console_monitor = id;
        config.console_monitor_stable_id = stable;
        changed
    };
    if fixed_source {
        log::info!(
            "the chosen screen is {:?} now \u{2014} corrected",
            config.recording.target_id
        );
    }
    if fixed_banner {
        log::info!(
            "the banner's screen is {:?} now \u{2014} corrected",
            config.overlay.monitor
        );
    }
    if fixed_console {
        log::info!(
            "the console's screen is {:?} now \u{2014} corrected",
            config.console_monitor
        );
    }
    if (fixed_source || fixed_banner || fixed_console) && config::save(&config).is_err() {
        log::warn!("the corrected screen was not saved");
    }
}

/// Put the banner and the console back on their screens when the screens change.
///
/// Both are positioned when they are set up, not for every showing — with a
/// screen picked by hand the spot already stands. Unplugging that screen makes
/// Windows push the windows onto one that is left, and plugging it back in
/// brought nothing back: the banner stayed on the primary screen until the next
/// start. So the set of screens is remembered, and any change to it — one gone,
/// one back, another resolution — lays both out again.
fn watch_monitors(app: &tauri::AppHandle) {
    type Landscape = Vec<(String, Option<String>, u32, u32, bool)>;
    static LAST: std::sync::Mutex<Option<Landscape>> = std::sync::Mutex::new(None);

    // Behind the lock screen the list is whatever Winlogon's desktop shows;
    // the change is caught against the one from before, once signed in.
    if capture::secure_desktop() {
        return;
    }
    let mut now: Landscape = capture::list_monitors()
        .into_iter()
        .map(|m| (m.id, m.stable_id, m.width, m.height, m.is_primary))
        .collect();
    now.sort();
    let changed = {
        let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let changed = last.as_ref().is_some_and(|last| *last != now);
        *last = Some(now);
        changed
    };
    if !changed {
        return;
    }
    log::info!("the screens have changed \u{2014} laying banner and console out again");
    repair_screens(&app.state::<AppState>());
    overlay::reposition(app);
    console::relayout(app);
}

/// Keep the buffer on the screen it is meant to be on.
///
/// The screen choice used to be settled once, when capture started. Unplugging
/// a monitor, getting it back, signing in again or switching the resolution
/// changed nothing after that: the buffer went on recording the primary screen,
/// a frozen picture or a scaled one until somebody restarted the app. Here,
/// every two seconds, the running capture is held against what the settings
/// mean now — see `AppState::screen_drift` for when that calls for a restart.
///
/// The restart is the same one a source change in the settings makes, and it
/// costs the same: what the buffer held so far is gone. Better than keeping
/// minutes of the wrong screen.
fn watch_screen(app: &tauri::AppHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static RESTARTING: AtomicBool = AtomicBool::new(false);

    // Behind the lock screen nothing can be captured at all, and the buffer
    // automation deals with that on its own.
    if capture::secure_desktop() {
        return;
    }
    let state = app.state::<AppState>();
    let Some(drift) = state.screen_drift() else {
        return;
    };
    if RESTARTING.swap(true, Ordering::SeqCst) {
        return;
    }
    log::info!(
        "the capture is no longer on the screen it should be ({drift:?}) \u{2014} restarting it"
    );
    let app = app.clone();
    // Stopping waits for a save in progress, and starting brings up the whole
    // capture stack: neither belongs on the status tick.
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        // The automation may have switched the buffer off in the meantime —
        // then there is nothing to move.
        if state.is_buffering() && !state.is_recording() {
            repair_screens(&state);
            state.stop_pipeline();
            match state.start_pipeline() {
                Ok(()) => {
                    let screen = state.status_snapshot().screen_fallback;
                    let title = match drift {
                        capture::Drift::Resized => "Recording follows the new resolution",
                        capture::Drift::Closed | capture::Drift::Moved => {
                            "Recording follows the screen"
                        }
                    };
                    let detail = match (screen, drift) {
                        (Some(stand_in), _) => format!(
                            "The chosen screen is not connected \u{2014} on {stand_in} for now"
                        ),
                        (None, capture::Drift::Resized) => {
                            "The buffer starts over at the new size".to_string()
                        }
                        (None, _) => "The buffer starts over on the chosen screen".to_string(),
                    };
                    overlay::show(&app, BannerKind::Info, title, Some(detail));
                }
                Err(err) => {
                    notify(&app, "error", err.clone());
                    overlay::show(&app, BannerKind::Error, "Buffer will not start", Some(err));
                }
            }
        }
        RESTARTING.store(false, Ordering::SeqCst);
    });
}

/// The buffer automation, on the cadence of game detection (every two seconds).
///
/// Covers both modes: "only in game" follows the foreground window, otherwise
/// the buffer should simply run. Both belong on the same tick — that way a
/// changed setting takes hold immediately and not only at the next program
/// start.
fn apply_auto_buffer(app: &tauri::AppHandle, game: Option<&String>) {
    let state = app.state::<AppState>();
    let config = state.config_snapshot();
    let active = state.status.lock().buffer_active;

    // The lock screen comes first, and it applies whatever the settings say: a
    // buffer that cannot see anything is not worth keeping alive, and one the
    // user started by hand is no exception. Only when the screen is ours does
    // the question of *when* recording is wanted arise at all.
    let locked = capture::secure_desktop();
    let action = match state.auto.poll_lock(locked, active) {
        state::AutoAction::Nothing if locked => return,
        state::AutoAction::Nothing => {
            if !config.buffer.auto_start {
                return;
            }
            if config.only_buffer_in_game {
                state.auto.poll(game.is_some(), active)
            } else {
                state.auto.poll_always(active)
            }
        }
        lock_action => lock_action,
    };
    if action == state::AutoAction::Nothing {
        return;
    }
    // Do not do it on the status tick: bringing up the recording takes a moment,
    // and stopping waits, if need be, for a save in progress.
    let app = app.clone();
    std::thread::spawn(move || match action {
        state::AutoAction::Start => start_buffer_and_notify(&app),
        state::AutoAction::Stop => stop_buffer_and_notify(&app),
        state::AutoAction::Nothing => {}
    });
}

/// Clear away leftovers nobody needs any more.
///
/// `buffer/` dates from the time the buffer lay on disk as MPEG-TS segments. The
/// buffer has long since lived in memory, but anyone coming from an older
/// version drags those files along forever otherwise — on one test machine that
/// was 323 MB, and with a long buffer and a high bitrate quickly a multiple of
/// that.
///
/// `temp/` belongs to the muxer, which clears its intermediate files itself.
/// What is still lying here is a crashed run from earlier.
fn discard_leftovers() {
    for stale in ["buffer", "temp"] {
        let dir = config::data_dir().join(stale);
        if !dir.is_dir() {
            continue;
        }
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => log::info!("cleared leftovers from '{stale}'"),
            Err(err) => log::warn!("could not clear '{stale}': {err}"),
        }
    }
}

/// Argument Windows starts ClippiBoy with on sign-in.
pub const AUTOSTART_ARG: &str = "--autostart";

fn started_by_autostart() -> bool {
    std::env::args().any(|arg| arg == AUTOSTART_ARG)
}

/// Bring the Windows auto-start in line with the setting.
pub fn apply_autostart(app: &tauri::AppHandle, wanted: bool) {
    use tauri_plugin_autostart::ManagerExt;

    let manager = app.autolaunch();
    if manager.is_enabled().unwrap_or(false) == wanted {
        return;
    }
    let result = if wanted {
        manager.enable()
    } else {
        manager.disable()
    };
    if let Err(err) = result {
        log::warn!("could not set auto-start: {err}");
    }
}

/// Check a key combination before it lands in the config.
///
/// Accepts the shortcut parser's spelling (`Ctrl+Shift+S`, `Alt+F9`,
/// `Ctrl+Numpad1`) — and equally a single key like `F9` or `PrintScreen`. A key
/// without a modifier applies globally: bind `S` and you also save a clip while
/// typing a message. That is the user's decision, the UI warns about it — it is
/// no longer rejected here.
pub fn parse_hotkey(text: &str) -> Result<tauri_plugin_global_shortcut::Shortcut, String> {
    use std::str::FromStr;
    use tauri_plugin_global_shortcut::Shortcut;

    let text = text.trim();
    if text.is_empty() {
        return Err("No key combination is set.".into());
    }
    Shortcut::from_str(text).map_err(|_| format!("\"{text}\" is not a valid key combination."))
}

/// Register the global hotkeys (save, screenshot, recording, buffer on/off).
///
/// Called at startup and after every change. Hence unregistering everything
/// first: otherwise the old assignment would stay active as well.
pub fn register_hotkeys(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    let config = app.state::<AppState>().config_snapshot();
    let shortcuts = app.global_shortcut();
    if let Err(err) = shortcuts.unregister_all() {
        log::warn!("could not unregister the old hotkeys: {err}");
    }

    let mut failed: Vec<String> = Vec::new();

    let save = config.save_clip_hotkey.clone();
    if let Err(err) = shortcuts.on_shortcut(save.as_str(), move |app, _shortcut, event| {
        if event.state() == ShortcutState::Pressed {
            let app = app.clone();
            // Muxing takes a moment — do not do it on the hotkey thread.
            std::thread::spawn(move || {
                let _ = save_clip_and_notify(&app);
            });
        }
    }) {
        log::warn!("could not register hotkey '{save}': {err}");
        failed.push(save);
    }

    let shot = config.screenshot_hotkey.clone();
    if let Err(err) = shortcuts.on_shortcut(shot.as_str(), move |app, _shortcut, event| {
        if event.state() == ShortcutState::Pressed {
            let app = app.clone();
            // Reading the picture back off the GPU and writing the PNG takes a
            // few hundred milliseconds — not on the hotkey thread.
            std::thread::spawn(move || {
                let _ = take_screenshot_and_notify(&app);
            });
        }
    }) {
        log::warn!("could not register hotkey '{shot}': {err}");
        failed.push(shot);
    }

    let console = config.console_hotkey.clone();
    if config.console_enabled {
        if let Err(err) = shortcuts.on_shortcut(console.as_str(), move |app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                // Showing a window belongs on the main thread; the hotkey
                // arrives on one of its own.
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || console::toggle(&handle));
            }
        }) {
            log::warn!("could not register hotkey '{console}': {err}");
            failed.push(console);
        }
    }

    let record = config.record_hotkey.clone();
    if let Err(err) = shortcuts.on_shortcut(record.as_str(), move |app, _shortcut, event| {
        if event.state() == ShortcutState::Pressed {
            let app = app.clone();
            // Stopping writes the whole recording out — not on the hotkey thread.
            std::thread::spawn(move || toggle_recording_and_notify(&app));
        }
    }) {
        log::warn!("could not register hotkey '{record}': {err}");
        failed.push(record);
    }

    let toggle = config.toggle_buffer_hotkey.clone();
    if let Err(err) = shortcuts.on_shortcut(toggle.as_str(), move |app, _shortcut, event| {
        if event.state() == ShortcutState::Pressed {
            toggle_buffer_and_notify(app);
        }
    }) {
        log::warn!("could not register hotkey '{toggle}': {err}");
        failed.push(toggle);
    }

    match failed.len() {
        0 => Ok(()),
        // Windows only reports "already taken" — practically always another
        // program holding the same combination.
        _ => Err(format!(
            "{} is already taken by another program.",
            failed.join(" and ")
        )),
    }
}

/// Is anybody looking at this window?
///
/// While a game runs the main window is normally closed to the tray, and a
/// hidden window has no use for twenty level readings a second. It costs more
/// than it sounds: each one crosses into the webview, is parsed, and puts the
/// mixer's bars through another round — on the same machine that is meant to be
/// encoding.
///
/// The console over the game is asked the same question, for the same reason
/// and with one difference that matters: it is never destroyed, only hidden
/// (`console::close`). Its mere existence therefore says nothing about whether
/// anybody can see it — only `is_visible` does.
fn window_awake(app: &tauri::AppHandle, label: &str) -> bool {
    let Some(window) = app.get_webview_window(label) else {
        return false;
    };
    window.is_visible().unwrap_or(true) && !window.is_minimized().unwrap_or(false)
}

/// Sends levels (20 Hz) and status data (1 Hz) to the UI.
fn spawn_ui_updates(app: &tauri::AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        let mut tick: u32 = 0;
        loop {
            std::thread::sleep(Duration::from_millis(50));
            let state = handle.state::<AppState>();
            let sources = state.config_snapshot().sources;
            // Only worked out when somebody can see them. Reading the levels is
            // cheap; the trip into the webview twenty times a second is not, and
            // the banner never wanted them at all — hence `emit_to` rather than
            // the broadcast this used to be.
            let watched = window_awake(&handle, "main");
            // Das Ton-Panel der Konsole zeigt dieselben Pegel. Es ist der eine
            // Ort, an dem man sie mitten im Spiel braucht — und der einzige
            // Grund, warum der Strom überhaupt ein zweites Ziel bekommt. Er
            // hängt an der Sichtbarkeit: steht die Konsole nur versteckt
            // herum, hört ihr niemand zu, und zwanzig Pakete die Sekunde in
            // ein unsichtbares Fenster sind genau der Aufwand, den `emit_to`
            // hier vermeiden soll.
            let console_watched = window_awake(&handle, console::LABEL);
            let logging = std::env::var("CLIPPIBOY_LOG_LEVELS").is_ok();
            if watched || console_watched || logging {
                let levels = state.audio.levels(&sources);
                if watched {
                    let _ = handle.emit_to("main", "audio-levels", &levels);
                }
                if console_watched {
                    let _ = handle.emit_to(console::LABEL, "audio-levels", &levels);
                }
                // Diagnostics: with CLIPPIBOY_LOG_LEVELS=1 the levels get logged.
                if tick % 20 == 0 && logging {
                    let summary: Vec<String> = levels
                        .iter()
                        .map(|(id, level)| format!("{id}={level:.3}"))
                        .collect();
                    log::info!("Pegel: {}", summary.join(" "));
                }
            }

            tick += 1;
            // Every 10 s: retry sources that failed to start. A headset that was
            // not plugged in at program start would otherwise not run along until
            // the next restart.
            if tick % 200 == 0 {
                state
                    .audio
                    .retry_failed(state.config_snapshot().sources, state.game_pid());
            }
            // Every 2 s, look which game is running in the foreground.
            if tick % 40 == 0 {
                let (game, _) = state.track_game();
                // Whatever moved — the game to a new process, an application
                // that started playing and now belongs in the leftovers — the
                // streams follow. The enumerating and rebuilding happens on a
                // thread of its own; the levels above run on this one.
                state
                    .audio
                    .refresh_async(state.config_snapshot().sources, state.game_pid());
                // What is wrong with the sources as configured — a device that
                // is gone, or one whose label promises a channel it does not
                // actually record. Worked out here rather than on the emit,
                // because it has to enumerate the endpoints.
                {
                    let sources = state.config_snapshot().sources;
                    let notes = audio::configuration_warnings(
                        &sources,
                        &audio::devices::list_devices(),
                        game.is_some(),
                        state.is_buffering(),
                    );
                    *state.source_notes.lock() = notes;
                }
                apply_auto_buffer(&handle, game.as_ref());
                // The windows first: the buffer's restart announces itself with
                // a banner, and that should already stand on the right screen.
                watch_monitors(&handle);
                watch_screen(&handle);
            }
            // If the recording reports a problem, somebody has to hear about it.
            // Without this the app appears to keep buffering, and only pressing
            // "Save clip" brings to light that nothing has arrived for minutes.
            if tick % 20 == 0 {
                let trouble = state
                    .shared
                    .lock()
                    .as_ref()
                    .and_then(|shared| shared.take_unseen_error());
                if let Some(err) = trouble {
                    notify(&handle, "error", err.clone());
                    overlay::show(&handle, BannerKind::Error, "Recording disrupted", Some(err));
                }
                // The recording cannot write any more — a full disk, most
                // likely. Stop it while what is there is still whole.
                if let Some(err) = state.recording_failure() {
                    notify(&handle, "error", format!("Recording stopped: {err}"));
                    let handle = handle.clone();
                    std::thread::spawn(move || {
                        let _ = stop_recording_and_notify(&handle);
                    });
                }
            }
            // Every 5 s while something is recording: what the pipeline is
            // really doing. One line, at info, so a log sent back from a machine
            // we cannot reach tells us whether the encoder kept up — instead of
            // us having to take "it lags" as the whole report.
            if tick % 100 == 0 {
                let line = state
                    .shared
                    .lock()
                    .as_ref()
                    .map(|shared| shared.health_line());
                if let Some(line) = line {
                    log::info!("{line}");
                }
            }
            if tick % 20 == 0 {
                // Erst messen, dann lesen: die Bildrate ist eine Rate, und das
                // Intervall dafür darf nur dieser eine Aufrufer verbrauchen.
                state.sample_fps();
                let status = state.status_snapshot();
                tray::refresh(&handle, &status);
                let _ = handle.emit("engine-status", status);
                let _ = handle.emit("audio-errors", state.audio.errors());
                // The engine's own notices and the ones about how the sources
                // are set up, in one map — `SourceTrouble` reads a single
                // channel and should not have to learn about a second.
                let mut warnings = state.audio.warnings(&sources);
                warnings.extend(
                    state
                        .source_notes
                        .lock()
                        .iter()
                        .map(|(id, note)| (id.clone(), note.clone())),
                );
                let _ = handle.emit("audio-warnings", warnings);
                let _ = handle.emit("audio-taps", state.audio.taps());
            }
        }
    });
}

/// Grant the configured clip folder to the `asset:` protocol.
///
/// Without it the player plays nothing as soon as `clipDir` lies outside the
/// `$VIDEO` scope from `tauri.conf.json`.
pub fn allow_clip_dir(app: &tauri::AppHandle, dir: &str) {
    if let Err(err) = app.asset_protocol_scope().allow_directory(dir, true) {
        log::warn!("clip folder '{dir}' is not granted to the player: {err}");
    }
}

/// Grant the folders older clips live in as well.
///
/// Whoever changes the storage location leaves their existing clips elsewhere —
/// without this, half the gallery would be a black frame on the next start.
fn allow_existing_clip_dirs(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let clips = match state.library.lock().as_ref() {
        Some(library) => library.list().unwrap_or_default(),
        None => return,
    };
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for clip in clips {
        let Some(dir) = std::path::Path::new(&clip.path).parent() else {
            continue;
        };
        let dir = dir.to_string_lossy().to_string();
        if seen.insert(dir.clone()) {
            allow_clip_dir(app, &dir);
        }
    }
}

/// Hide the window instead of closing it — the app lives on in the tray.
fn hide_to_tray(window: &tauri::Window) {
    let _ = window.hide();

    let app = window.app_handle();
    let state = app.state::<AppState>();
    {
        // Only set and persist the flag — `replace_config` would reapply the
        // audio sources and disturb a recording in progress.
        let mut config = state.config.lock();
        if config.tray_hint_shown {
            return;
        }
        config.tray_hint_shown = true;
        if let Err(err) = config::save(&config) {
            log::warn!("could not remember the tray hint: {err}");
        }
    }
    overlay::show(
        app,
        BannerKind::Info,
        "ClippiBoy keeps running",
        Some("Reopen it from the tray icon".into()),
    );
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    logging::init();

    let app = tauri::Builder::default()
        // First of all the plugins on purpose: a second start must find out
        // that it is one before it has set anything up. It hands its arguments
        // over here and then ends itself — the window that is already there
        // comes up instead of a second one that would fight the first over
        // hotkeys, tray icon and the replay buffer.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // Except when the second start is the auto-start one: that one is
            // supposed to stay in the tray, not tear a window open.
            if args.iter().any(|arg| arg == AUTOSTART_ARG) {
                return;
            }
            tray::show_main_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // The argument lands in the auto-start shortcut: only then does
        // ClippiBoy start straight into the tray without opening the window.
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![AUTOSTART_ARG]),
        ))
        .manage(updater::Pending::default())
        .manage(AppState::new())
        .setup(|app| {
            let handle = app.handle();
            // Before anything reads the recording settings: put the saved screen
            // back together. `\\.\DISPLAY2` is handed out by enumeration order
            // at boot, so after a restart it can name a different panel than it
            // did yesterday — the identity saved beside it says which one was
            // meant, and the device name is corrected from it here.
            repair_screens(&app.state::<AppState>());
            let config = app.state::<AppState>().config_snapshot();
            allow_clip_dir(handle, &config.clip_dir);
            allow_existing_clip_dirs(handle);
            discard_leftovers();
            // Throwaway files from an old session: clear first, then grant.
            preview::clear();
            allow_clip_dir(handle, &preview::dir().to_string_lossy());
            // The individual tracks, by contrast, are the master copy of the mix
            // and stay put — the player plays them straight from there.
            let _ = std::fs::create_dir_all(stems::root());
            allow_clip_dir(handle, &stems::root().to_string_lossy());
            // The thumbnails live in the data directory rather than the user's
            // folder — the gallery loads them from there.
            let _ = std::fs::create_dir_all(thumbs::dir());
            allow_clip_dir(handle, &thumbs::dir().to_string_lossy());
            // The originals store. A trimmed clip's untouched recording is
            // never played back from here — but a screenshot's is: while it is
            // being annotated, that is the very picture on the stage, and
            // without this the WebView refuses to load it.
            let _ = std::fs::create_dir_all(edit::root());
            allow_clip_dir(handle, &edit::root().to_string_lossy());
            if let Some(library) = app.state::<AppState>().library.lock().as_ref() {
                edit::repair(library);
                // Pictures from older versions still sit next to the videos.
                thumbs::migrate(library);
                // Game names first, folders second: a name that differs only in
                // characters nobody can see has to become the one name before
                // `tidy` works out where its clips belong — otherwise it would
                // dutifully file them into the four folders they came from.
                filing::normalize_games(library);
                // And whatever has not found its way into its game folder since
                // last time moves there now.
                filing::tidy(library, &config.clip_dir);
            }
            // ffmpeg/ffprobe: at once when they are there, otherwise fetched
            // in the background — the buffer does not wait for them.
            tools::setup(handle);
            apply_autostart(handle, config.auto_start_with_windows);
            if started_by_autostart() {
                if let Some(window) = handle.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
            overlay::create(handle);
            console::create(handle);
            if let Err(err) = tray::build(handle) {
                log::error!("could not create the tray icon: {err}");
            }
            // The Stream Deck's way in. Deliberately after the tray: if the port
            // is taken, the app is already usable by then.
            control::start(handle);
            spawn_ui_updates(handle);
            if let Err(err) = register_hotkeys(handle) {
                log::warn!("{err}");
            }
            start_buffer_if_configured(handle);
            updater::watch(handle);
            Ok(())
        })
        .on_window_event(|window, event| {
            // Applies to Alt+F4 and the taskbar's window list; the ✕ in our own
            // title bar hides the window directly.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    hide_to_tray(window);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_audio_devices,
            commands::list_audio_processes,
            commands::list_capture_targets,
            commands::list_encoders,
            commands::get_config,
            commands::set_config,
            commands::set_clip_favorite,
            commands::file_clip,
            commands::copy_clip_file,
            commands::open_clip,
            commands::clipboard_write_text,
            commands::clipboard_read_text,
            commands::set_hotkeys,
            commands::suspend_hotkeys,
            commands::resume_hotkeys,
            commands::set_clip_dir,
            commands::default_clip_dir,
            commands::add_audio_source,
            commands::update_audio_source,
            commands::remove_audio_source,
            commands::engine_status,
            commands::start_buffer,
            commands::stop_buffer,
            commands::save_clip,
            commands::toggle_recording,
            commands::close_console,
            commands::show_clip_in_app,
            commands::export_for_discord,
            commands::take_screenshot,
            commands::copy_clip_image,
            commands::write_screenshot,
            commands::screenshot_edit,
            commands::list_clips,
            commands::delete_clip,
            commands::reveal_clip,
            commands::update_clip,
            commands::clip_tracks,
            commands::clip_waveform,
            commands::apply_clip_edit,
            commands::restore_clip_original,
            commands::discard_clip_original,
            commands::storage_usage,
            commands::export_clip,
            commands::reveal_path,
            commands::app_version,
            commands::check_update,
            commands::pending_update,
            commands::install_update,
            commands::ffmpeg_status,
            commands::retry_ffmpeg,
            commands::regenerate_control_token,
        ])
        .build(tauri::generate_context!())
        .expect("could not start ClippiBoy");

    app.run(|handle, event| {
        // A hidden window must not take the process with it — quitting only
        // happens via "Quit" in the tray.
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            if !handle
                .state::<AppState>()
                .quitting
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                api.prevent_exit();
            }
        }
    });
}
