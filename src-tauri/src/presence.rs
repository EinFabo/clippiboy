//! Discord Rich Presence: "Using ClippiBoy" with the logo and a "Get ClippiBoy"
//! button in one's Discord status, so friends see it and can get it too.
//!
//! Spoken over the Discord app's local pipe (`\\.\pipe\discord-ipc-N`), the
//! same frames `examples/discord-probe.rs` uses — a handful of lines, so no
//! crate for it. Needs no approval from Discord, only the ClippiBoy app in the
//! developer portal and an art asset named [`LOGO_ASSET`] there.
//!
//! Everything runs on a thread of its own: a pipe read blocks, and Discord may
//! not be running at all. Without Discord the thread tries again now and then
//! ([`RETRY_FIRST`] up to [`RETRY_MAX`]) and says nothing about it; Discord
//! started later is found that way. The status goes when the setting is
//! switched off, when [`suppress`] is set, and when ClippiBoy quits.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::state::AppState;

/// The ClippiBoy app in the Discord developer portal — the one the friends
/// login uses (`server/wrangler.jsonc`). Not a secret.
pub const CLIENT_ID: &str = "1553530949491236894";

/// The key of the large picture: an art asset of this name has to be uploaded
/// under Rich Presence → Art Assets in the developer portal. Without it Discord
/// shows the status without a picture.
pub const LOGO_ASSET: &str = "logo";

const BUTTON_LABEL: &str = "Get ClippiBoy";
const BUTTON_URL: &str = "https://clippiboy.com";
const DETAILS: &str = "Using ClippiBoy";

/// Only while no game runs? Unknown yet whether Discord still shows "Playing
/// <game>" while ours is set. If it turns out ours pushes the game aside, flip
/// this to `true`: the status then goes as soon as a game is detected
/// (`AppState::current_game`) and comes back after it.
pub const HIDE_WHILE_PLAYING: bool = false;

/// How often the setting and the game are looked at.
const TICK: Duration = Duration::from_secs(5);
/// How often a shown status is sent again — the only way to notice that
/// Discord quit (the write fails) and to put it back once it is there again.
const REFRESH: Duration = Duration::from_secs(60);
const RETRY_FIRST: Duration = Duration::from_secs(10);
const RETRY_MAX: Duration = Duration::from_secs(60);
/// How long quitting waits for the status to be cleared.
const QUIT_WAIT: Duration = Duration::from_millis(500);
/// How long Discord may take to answer. A pipe that takes the frame and never
/// answers counts as broken after this — a plain read would wait forever, and
/// the thread with it, deaf to the setting.
const ANSWER_WAIT: Duration = Duration::from_secs(3);

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;

enum Msg {
    /// Something changed — look now rather than at the next tick.
    Nudge,
    /// Clear the status and end; answered on the sender.
    Quit(Sender<()>),
}

static INBOX: OnceLock<Sender<Msg>> = OnceLock::new();

/// Set by whatever must keep ClippiBoy out of the Discord status regardless of
/// the setting — the streamer mode (point 161) is meant to call [`suppress`].
static SUPPRESSED: AtomicBool = AtomicBool::new(false);

/// Starts the thread. Connects only once the status is wanted.
pub fn start(app: &AppHandle) {
    let (tx, rx) = crossbeam_channel::unbounded();
    if INBOX.set(tx).is_err() {
        return;
    }
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("clippiboy-presence".into())
        .spawn(move || run(&app, &rx));
    if let Err(err) = spawned {
        log::warn!("Discord status: thread not started: {err}");
    }
}

/// The setting may have changed (`friends::config_changed`).
pub fn nudge() {
    if let Some(inbox) = INBOX.get() {
        let _ = inbox.send(Msg::Nudge);
    }
}

/// Keeps ClippiBoy out of the Discord status while `on`, whatever the setting
/// says — and puts it back after. The hook for the streamer mode.
pub fn suppress(on: bool) {
    if SUPPRESSED.swap(on, Ordering::SeqCst) != on {
        nudge();
    }
}

/// On quit: clears the status, waiting a moment at most. The pipe closing with
/// the process would clear it too; this is the polite way.
pub fn shutdown() {
    let Some(inbox) = INBOX.get() else { return };
    let (done, wait) = crossbeam_channel::bounded(1);
    if inbox.send(Msg::Quit(done)).is_ok() {
        let _ = wait.recv_timeout(QUIT_WAIT);
    }
}

/// Should the status show? The setting, the suppression, and — when
/// [`HIDE_WHILE_PLAYING`] is on — no game running.
pub fn wanted(enabled: bool, suppressed: bool, playing: bool, hide_while_playing: bool) -> bool {
    enabled && !suppressed && !(hide_while_playing && playing)
}

/// The wait before the next attempt to reach Discord: doubles, up to the cap.
pub fn next_retry(previous: Option<Duration>) -> Duration {
    match previous {
        None => RETRY_FIRST,
        Some(wait) => (wait * 2).min(RETRY_MAX),
    }
}

/// The activity as Discord's `SET_ACTIVITY` takes it — no game, no counter.
pub fn activity() -> Value {
    json!({
        "details": DETAILS,
        "assets": { "large_image": LOGO_ASSET, "large_text": "ClippiBoy" },
        "buttons": [{ "label": BUTTON_LABEL, "url": BUTTON_URL }],
    })
}

/// A `SET_ACTIVITY` command; `None` clears the status.
fn set_activity(pid: u32, activity: Option<Value>, nonce: &str) -> Value {
    let mut args = json!({ "pid": pid });
    if let Some(activity) = activity {
        args["activity"] = activity;
    }
    json!({ "cmd": "SET_ACTIVITY", "args": args, "nonce": nonce })
}

fn run(app: &AppHandle, inbox: &Receiver<Msg>) {
    let mut link: Option<Link> = None;
    let mut shown = false;
    let mut last_sent = Instant::now();
    let mut retry_at: Option<Instant> = None;
    let mut backoff: Option<Duration> = None;
    loop {
        match inbox.recv_timeout(TICK) {
            Ok(Msg::Quit(done)) => {
                if let Some(link) = link.as_mut() {
                    let _ = link.set(None);
                }
                let _ = done.send(());
                return;
            }
            // Asked by hand: no reason to sit out the wait.
            Ok(Msg::Nudge) => retry_at = None,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let Some(state) = app.try_state::<AppState>() else { continue };
        let enabled = state.config.lock().friends.discord_status;
        let playing = state.current_game.lock().is_some();
        let want = wanted(enabled, SUPPRESSED.load(Ordering::SeqCst), playing, HIDE_WHILE_PLAYING);

        if !want {
            if let Some(mut gone) = link.take() {
                let _ = gone.set(None);
                if shown {
                    log::info!("Discord status cleared");
                }
            }
            shown = false;
            continue;
        }
        if shown && last_sent.elapsed() < REFRESH {
            continue;
        }
        if link.is_none() {
            if retry_at.is_some_and(|at| Instant::now() < at) {
                continue;
            }
            match Link::open() {
                Ok(opened) => link = Some(opened),
                Err(err) => {
                    // Once per streak; Discord simply not running is normal.
                    if backoff.is_none() {
                        log::debug!("Discord status: Discord not reachable ({err})");
                    }
                    let wait = next_retry(backoff);
                    backoff = Some(wait);
                    retry_at = Some(Instant::now() + wait);
                    continue;
                }
            }
        }
        let Some(open) = link.as_mut() else { continue };
        match open.set(Some(activity())) {
            Ok(()) => {
                if !shown {
                    log::info!("Discord status shown");
                }
                shown = true;
                last_sent = Instant::now();
                backoff = None;
                retry_at = None;
            }
            Err(err) => {
                if shown {
                    log::info!("Discord status: connection lost ({err})");
                } else if backoff.is_none() {
                    log::debug!("Discord status: not set ({err})");
                }
                link = None;
                shown = false;
                let wait = next_retry(backoff);
                backoff = Some(wait);
                retry_at = Some(Instant::now() + wait);
            }
        }
    }
}

/// An open, greeted connection to the Discord app.
struct Link {
    pipe: File,
    nonce: u64,
}

impl Link {
    fn open() -> std::io::Result<Self> {
        // Discord takes the first free of ten; with PTB or Canary beside it,
        // it may not be 0 — and a pipe that opens but will not greet us is
        // passed over for the next.
        let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, "no Discord pipe");
        for n in 0..10 {
            let Ok(pipe) = OpenOptions::new().read(true).write(true).open(format!(r"\\.\pipe\discord-ipc-{n}")) else {
                continue;
            };
            let mut link = Self { pipe, nonce: 0 };
            match link.greet() {
                Ok(()) => return Ok(link),
                Err(err) => last = err,
            }
        }
        Err(last)
    }

    fn greet(&mut self) -> std::io::Result<()> {
        self.pipe.write_all(&frame(OP_HANDSHAKE, &json!({ "v": 1, "client_id": CLIENT_ID }))?)?;
        let (op, ready) = self.receive()?;
        if op == OP_FRAME && ready["evt"] == "READY" {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("handshake refused (op {op}): {}", ready["message"])))
        }
    }

    /// One frame from Discord, within [`ANSWER_WAIT`].
    fn receive(&mut self) -> std::io::Result<(u32, Value)> {
        let deadline = Instant::now() + ANSWER_WAIT;
        let pipe = &self.pipe;
        read_frame(&mut &self.pipe, &mut |bytes| wait_for(pipe, bytes, deadline))
    }

    /// Sends the activity (or clears it with `None`) and waits for Discord's answer.
    fn set(&mut self, activity: Option<Value>) -> std::io::Result<()> {
        self.nonce += 1;
        let nonce = self.nonce.to_string();
        let command = set_activity(std::process::id(), activity, &nonce);
        self.pipe.write_all(&frame(OP_FRAME, &command)?)?;
        // The answer carries the nonce; a ping in between is answered.
        for _ in 0..8 {
            let (op, body) = self.receive()?;
            match op {
                OP_PING => self.pipe.write_all(&frame(OP_PONG, &body)?)?,
                OP_CLOSE => return Err(std::io::Error::other(format!("closed by Discord: {}", body["message"]))),
                OP_FRAME if body["nonce"] == nonce.as_str() => {
                    return if body["evt"] == "ERROR" {
                        Err(std::io::Error::other(format!("refused: {}", body["data"]["message"])))
                    } else {
                        Ok(())
                    };
                }
                _ => {}
            }
        }
        Err(std::io::Error::other("no answer to SET_ACTIVITY"))
    }
}

/// One frame: opcode and length as little-endian u32, then the JSON.
fn frame(op: u32, body: &Value) -> std::io::Result<Vec<u8>> {
    let body = serde_json::to_vec(body)?;
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Reads one frame. `ready(n)` is asked before each read and returns once `n`
/// bytes can be read without blocking — or fails, and the read with it.
fn read_frame(
    pipe: &mut impl Read,
    ready: &mut dyn FnMut(usize) -> std::io::Result<()>,
) -> std::io::Result<(u32, Value)> {
    let mut header = [0u8; 8];
    ready(header.len())?;
    pipe.read_exact(&mut header)?;
    let op = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    if length > 1 << 20 {
        return Err(std::io::Error::other("frame too large"));
    }
    let mut body = vec![0u8; length];
    if length > 0 {
        ready(length)?;
    }
    pipe.read_exact(&mut body)?;
    Ok((op, serde_json::from_slice(&body)?))
}

/// Returns once `bytes` wait in the pipe, fails once `deadline` passed or the
/// pipe broke. Looks with `PeekNamedPipe`, which never blocks.
#[cfg(windows)]
fn wait_for(pipe: &File, bytes: usize, deadline: Instant) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Pipes::PeekNamedPipe;
    let handle = HANDLE(pipe.as_raw_handle());
    loop {
        let mut available = 0u32;
        unsafe { PeekNamedPipe(handle, None, 0, None, Some(&mut available as *mut u32), None) }
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::BrokenPipe, err.to_string()))?;
        if available as usize >= bytes {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "Discord did not answer"));
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

#[cfg(not(windows))]
fn wait_for(_pipe: &File, _bytes: usize, _deadline: Instant) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_follows_the_setting_and_the_suppression() {
        assert!(wanted(true, false, false, false));
        assert!(!wanted(false, false, false, false), "switched off");
        assert!(!wanted(true, true, false, false), "suppressed, e.g. streamer mode");
        assert!(wanted(true, false, true, false), "a game running changes nothing by default");
        assert!(!wanted(true, false, true, true), "unless it is to hide while playing");
        assert!(wanted(true, false, false, true));
    }

    #[test]
    fn retries_slow_down_to_a_minute() {
        assert_eq!(next_retry(None), RETRY_FIRST);
        assert_eq!(next_retry(Some(RETRY_FIRST)), RETRY_FIRST * 2);
        let mut wait = None;
        for _ in 0..20 {
            wait = Some(next_retry(wait));
        }
        assert_eq!(wait, Some(RETRY_MAX));
    }

    #[test]
    fn the_activity_has_the_logo_and_one_button() {
        let command = set_activity(42, Some(activity()), "7");
        assert_eq!(command["cmd"], "SET_ACTIVITY");
        assert_eq!(command["nonce"], "7");
        assert_eq!(command["args"]["pid"], 42);
        let activity = &command["args"]["activity"];
        assert_eq!(activity["details"], "Using ClippiBoy");
        assert_eq!(activity["assets"]["large_image"], LOGO_ASSET);
        assert_eq!(activity["buttons"][0]["label"], "Get ClippiBoy");
        assert_eq!(activity["buttons"][0]["url"], "https://clippiboy.com");
        assert!(activity.get("timestamps").is_none(), "no counter");
        // Clearing leaves the activity out altogether.
        assert!(set_activity(42, None, "8")["args"].get("activity").is_none());
    }

    #[test]
    fn a_frame_reads_back_as_written() {
        let body = json!({ "cmd": "SET_ACTIVITY", "nonce": "1" });
        let bytes = frame(OP_FRAME, &body).unwrap();
        assert_eq!(&bytes[..4], &1u32.to_le_bytes());
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize, bytes.len() - 8);
        let mut asked = Vec::new();
        let (op, back) = read_frame(&mut bytes.as_slice(), &mut |n| {
            asked.push(n);
            Ok(())
        })
        .unwrap();
        assert_eq!(asked, vec![8, bytes.len() - 8], "each read waits for its bytes first");
        assert_eq!(op, OP_FRAME);
        assert_eq!(back, body);
    }

    #[test]
    fn an_oversized_frame_is_refused() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&OP_FRAME.to_le_bytes());
        bytes.extend_from_slice(&(2u32 << 20).to_le_bytes());
        assert!(read_frame(&mut bytes.as_slice(), &mut |_| Ok(())).is_err());
    }

    #[test]
    fn a_silent_pipe_breaks_the_read_instead_of_blocking() {
        let bytes = frame(OP_FRAME, &json!({})).unwrap();
        let timed_out = || std::io::Error::new(std::io::ErrorKind::TimedOut, "Discord did not answer");
        let err = read_frame(&mut bytes.as_slice(), &mut |_| Err(timed_out())).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    }
}
