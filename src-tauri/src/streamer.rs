//! Streamer mode: Off, On, or Automatic — on while OBS or Streamlabs runs.
//!
//! While it is in effect:
//! - every ClippiBoy window is excluded from screen capture
//!   (`WDA_EXCLUDEFROMCAPTURE`, which is what Tauri's content protection sets),
//!   the console whatever `console_in_capture` says — so a display capture in
//!   OBS shows the game, not the app. ClippiBoy's own clips lose the windows
//!   too, which is what one wants there anyway;
//! - friend banners say "a friend" instead of the name and drop the picture
//!   ([`mask_name`], used by `overlay::show_friend`), and the windows hide
//!   names, friend codes and links (they read [`active`] from `engine-status`
//!   and the `streamer-mode` event);
//! - ClippiBoy's sounds only play on a sound device of their own
//!   (`sounds::audible`), never on the default one the stream records;
//! - the Discord status is suppressed (`presence::suppress`).
//!
//! Automatic looks at the process list every [`TICK`] — a ToolHelp snapshot,
//! the one `audio::devices::process_tree` already takes — and only while the
//! setting asks for it. No OBS WebSocket: that needs a password and a plugin
//! setting, and a process name is enough to know OBS is open.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use tauri::{AppHandle, Emitter, Manager};

use crate::model::StreamerMode;
use crate::state::AppState;

/// The streaming programs Automatic waits for, compared without case.
/// OBS Studio is `obs64.exe` (32-bit builds up to 27 were `obs32.exe`);
/// Streamlabs Desktop kept the old `Streamlabs OBS.exe` for years, newer
/// installs may carry the new name.
pub const STREAMING_APPS: &[&str] = &[
    "obs64.exe",
    "obs32.exe",
    "Streamlabs OBS.exe",
    "Streamlabs Desktop.exe",
];

/// How often Automatic looks for OBS. Going live takes longer than that.
const TICK: Duration = Duration::from_secs(5);

/// In effect right now.
static ACTIVE: AtomicBool = AtomicBool::new(false);

static INBOX: OnceLock<Sender<()>> = OnceLock::new();

/// Is streamer mode in effect right now?
pub fn active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

/// Is `exe` one of [`STREAMING_APPS`]?
pub fn is_streaming_app(exe: &str) -> bool {
    STREAMING_APPS.iter().any(|app| app.eq_ignore_ascii_case(exe))
}

/// Whether streamer mode is in effect under `mode`, with a streaming program
/// running or not.
pub fn effective(mode: StreamerMode, streaming_app_running: bool) -> bool {
    match mode {
        StreamerMode::Off => false,
        StreamerMode::On => true,
        StreamerMode::Automatic => streaming_app_running,
    }
}

/// What a window is to screen capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Affinity {
    /// `WDA_NONE`: captured like any window.
    Normal,
    /// `WDA_EXCLUDEFROMCAPTURE`: shown on the screen, missing from every
    /// capture of it.
    Excluded,
}

/// The affinity of the window `label`: the banner is never captured, the
/// console only when `console_in_capture` asks for it, everything else always
/// — and in streamer mode none of them.
pub fn affinity(label: &str, streamer: bool, console_in_capture: bool) -> Affinity {
    let excluded = streamer
        || label == crate::overlay::LABEL
        || (label == crate::console::LABEL && !console_in_capture);
    if excluded {
        Affinity::Excluded
    } else {
        Affinity::Normal
    }
}

/// Gives `window` the affinity it should have now.
pub fn apply(app: &AppHandle, window: &tauri::WebviewWindow) {
    let console_in_capture = app.state::<AppState>().config_snapshot().console_in_capture;
    let excluded = affinity(window.label(), active(), console_in_capture) == Affinity::Excluded;
    if let Err(err) = window.set_content_protected(excluded) {
        log::warn!("could not set the capture protection of {}: {err}", window.label());
    }
}

/// [`apply`] to every window there is.
fn apply_all(app: &AppHandle) {
    for window in app.webview_windows().values() {
        apply(app, window);
    }
}

/// What a friend banner says in streamer mode: `name` becomes "A friend" at
/// the start of `text`, "a friend" anywhere else. Only whole words count, so a
/// name that is part of another word leaves that word alone.
pub fn mask_name(text: &str, name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        return text.to_owned();
    }
    let is_word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut at = 0;
    while let Some(found) = rest.find(name) {
        let before = text[..at + found].chars().next_back();
        let after = rest[found + name.len()..].chars().next();
        out.push_str(&rest[..found]);
        if is_word(before) || is_word(after) {
            out.push_str(name);
        } else if at + found == 0 {
            out.push_str("A friend");
        } else {
            out.push_str("a friend");
        }
        let used = found + name.len();
        at += used;
        rest = &rest[used..];
    }
    out.push_str(rest);
    out
}

/// Starts the thread that follows the setting and, for Automatic, the
/// process list.
pub fn start(app: &AppHandle) {
    let (tx, rx) = crossbeam_channel::unbounded();
    if INBOX.set(tx).is_err() {
        return;
    }
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("clippiboy-streamer".into())
        .spawn(move || run(&app, &rx));
    if let Err(err) = spawned {
        log::warn!("streamer mode: thread not started: {err}");
    }
}

/// The setting may have changed — look now rather than at the next tick.
pub fn nudge() {
    if let Some(inbox) = INBOX.get() {
        let _ = inbox.send(());
    }
}

fn run(app: &AppHandle, inbox: &Receiver<()>) {
    loop {
        if let Some(state) = app.try_state::<AppState>() {
            let mode = state.config.lock().streamer_mode;
            let running = mode == StreamerMode::Automatic && streaming_app_running();
            let now = effective(mode, running);
            if ACTIVE.swap(now, Ordering::SeqCst) != now {
                changed(app, now);
            }
        }
        match inbox.recv_timeout(TICK) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn streaming_app_running() -> bool {
    crate::audio::devices::process_tree()
        .values()
        .any(|(_, exe)| is_streaming_app(exe))
}

fn changed(app: &AppHandle, on: bool) {
    log::info!("streamer mode {}", if on { "on" } else { "off" });
    apply_all(app);
    crate::presence::suppress(on);
    let _ = app.emit("streamer-mode", on);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_and_on_ignore_obs_and_automatic_follows_it() {
        assert!(!effective(StreamerMode::Off, true));
        assert!(effective(StreamerMode::On, false));
        assert!(effective(StreamerMode::Automatic, true));
        assert!(!effective(StreamerMode::Automatic, false));
    }

    #[test]
    fn obs_and_streamlabs_count_in_any_case_and_nothing_else_does() {
        assert!(is_streaming_app("obs64.exe"));
        assert!(is_streaming_app("OBS64.EXE"));
        assert!(is_streaming_app("streamlabs obs.exe"));
        assert!(is_streaming_app("Streamlabs Desktop.exe"));
        assert!(!is_streaming_app("obs-browser-page.exe"));
        assert!(!is_streaming_app("Discord.exe"));
        assert!(!is_streaming_app(""));
    }

    #[test]
    fn the_banner_is_never_captured_and_the_console_only_when_asked() {
        use crate::{console, overlay};
        assert_eq!(affinity(overlay::LABEL, false, true), Affinity::Excluded);
        assert_eq!(affinity(console::LABEL, false, false), Affinity::Excluded);
        assert_eq!(affinity(console::LABEL, false, true), Affinity::Normal);
        assert_eq!(affinity("main", false, true), Affinity::Normal);
        assert_eq!(affinity("main", false, false), Affinity::Normal);
    }

    #[test]
    fn streamer_mode_takes_every_window_out_of_capture() {
        use crate::{console, overlay};
        for label in ["main", console::LABEL, overlay::LABEL, "anything-new"] {
            assert_eq!(affinity(label, true, true), Affinity::Excluded, "{label}");
        }
    }

    #[test]
    fn a_name_becomes_a_friend_where_it_stands() {
        assert_eq!(mask_name("Luca is online", "Luca"), "A friend is online");
        assert_eq!(mask_name("Clip from Luca", "Luca"), "Clip from a friend");
        assert_eq!(
            mask_name("Luca wants to send you a clip", "Luca"),
            "A friend wants to send you a clip"
        );
        assert_eq!(mask_name("Luca started Valorant", "Luca"), "A friend started Valorant");
    }

    #[test]
    fn only_whole_names_are_masked() {
        assert_eq!(mask_name("Al is playing Valorant", "Al"), "A friend is playing Valorant");
        assert_eq!(mask_name("Alan is online", "Al"), "Alan is online");
        assert_eq!(mask_name("Al, Al and Al", "Al"), "A friend, a friend and a friend");
    }

    #[test]
    fn names_with_odd_characters_and_no_name_at_all() {
        assert_eq!(mask_name("✨Mia✨ has your clip", "✨Mia✨"), "A friend has your clip");
        assert_eq!(mask_name("Jörg is online", "Jörg"), "A friend is online");
        assert_eq!(mask_name("Someone is online", ""), "Someone is online");
        assert_eq!(mask_name("Someone is online", "Luca"), "Someone is online");
    }
}
