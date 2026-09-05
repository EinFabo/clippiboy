//! The local control port — a fourth route to the same three buttons.
//!
//! Hotkey, tray menu and the window all end up in `crate::save_clip_and_notify`,
//! `crate::take_screenshot_and_notify` and `crate::toggle_buffer_and_notify`.
//! This module adds a Stream Deck to that list, and gives it something the other
//! three do not need: a way to *ask* how things stand, so the key can show the
//! buffer level and the detected game instead of being a mute switch.
//!
//! It listens on `127.0.0.1` and nowhere else, and every request has to carry
//! the token from the config. The token is handed over through a file next to
//! the config (see [`write_handoff`]) — both programs run on this machine under
//! this user, so nobody has to copy anything by hand.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};
use tauri::Manager;
use tiny_http::{Header, Method, Request, Response, Server};

use crate::state::AppState;

/// The handshake file next to `config.json`. The plugin reads port and token
/// from here.
const HANDOFF: &str = "streamdeck.json";

/// Which header carries the token. Not `Authorization`: nothing here is a
/// standard scheme, and a name of our own says plainly that this is ours.
const TOKEN_HEADER: &str = "x-clippiboy-token";

struct Running {
    server: Arc<Server>,
    stop: Arc<AtomicBool>,
    /// Kept so a restart can wait for the old thread — see [`stop`].
    thread: std::thread::JoinHandle<()>,
}

/// The server currently listening, if there is one.
static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

/// Start the control port, unless it is switched off.
///
/// Never fatal: a port already taken costs the Stream Deck keys, not the app.
pub fn start(app: &tauri::AppHandle) {
    let config = app.state::<AppState>().config_snapshot();
    if !config.control.enabled {
        clear_handoff();
        log::info!("control port switched off");
        return;
    }

    let port = config.control.port;
    let token = ensure_token(app);
    if token.is_empty() {
        log::warn!("control port has no token — not starting");
        return;
    }

    // Explicitly the loopback address, not `0.0.0.0`: this must not be a port
    // anybody else on the network can reach.
    let server = match Server::http(("127.0.0.1", port)) {
        Ok(server) => Arc::new(server),
        Err(err) => {
            log::warn!("control port {port} will not open ({err}) — Stream Deck keys stay silent");
            clear_handoff();
            return;
        }
    };

    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let server = server.clone();
        let stop = stop.clone();
        let app = app.clone();
        let token = token.clone();
        std::thread::spawn(move || serve(app, server, stop, token))
    };

    *RUNNING.lock() = Some(Running {
        server,
        stop,
        thread,
    });
    write_handoff(app, port, &token);
    log::info!("control port listening on 127.0.0.1:{port}");
}

/// Stop the control port and take the handshake file with it.
///
/// Waits for the listening thread, and that is the point of it: the thread holds
/// the second half of the socket, so without the wait a `restart` onto the same
/// port would find it still taken and quietly give up.
pub fn stop() {
    if let Some(running) = RUNNING.lock().take() {
        running.stop.store(true, Ordering::SeqCst);
        // Wakes the thread out of `incoming_requests`, which otherwise sits
        // there until the next request that will never come.
        running.server.unblock();
        drop(running.server);
        if running.thread.join().is_err() {
            log::warn!("the control port's thread ended badly");
        }
    }
    clear_handoff();
}

/// After a change to port or on/off in the settings.
pub fn restart(app: &tauri::AppHandle) {
    stop();
    start(app);
}

/// Throw the token away and generate a new one. The old one stops working right
/// away, so a plugin still holding it has to fetch the new one from the
/// handshake file.
pub fn regenerate_token(app: &tauri::AppHandle) {
    {
        let state = app.state::<AppState>();
        let mut config = state.config.lock();
        config.control.token = String::new();
        // Only the token, and saved directly: `replace_config` would reapply
        // the audio sources and disturb a running recording.
        if let Err(err) = crate::config::save(&config) {
            log::warn!("could not save the new control token: {err}");
        }
    }
    restart(app);
}

fn serve(app: tauri::AppHandle, server: Arc<Server>, stop: Arc<AtomicBool>, token: String) {
    for request in server.incoming_requests() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        handle(&app, request, &token);
    }
    log::info!("control port stopped");
}

fn handle(app: &tauri::AppHandle, mut request: Request, token: &str) {
    let mut supplied: Option<String> = None;
    // A browser sets `Origin` on every cross-site request it makes. Nothing that
    // belongs here ever sends one, so refusing them closes the door on a web
    // page firing clips through a rebound DNS name — even if it somehow knew
    // the token.
    let mut from_browser = false;
    for header in request.headers() {
        match header.field.as_str().as_str().to_ascii_lowercase().as_str() {
            "origin" | "referer" | "sec-fetch-site" => from_browser = true,
            TOKEN_HEADER => supplied = Some(header.value.as_str().to_string()),
            _ => {}
        }
    }

    let allowed = !from_browser && supplied.is_some_and(|supplied| equals(&supplied, token));
    if !allowed {
        return respond(request, 403, json!({ "ok": false, "error": "forbidden" }));
    }

    // The query part is nothing this port uses; cutting it off keeps a stray
    // `?x=1` from turning into a 404.
    let route = request.url().split('?').next().unwrap_or("").to_string();
    match (request.method(), route.as_str()) {
        (Method::Get, "/v1/status") => {
            let body = status(app);
            respond(request, 200, body)
        }
        (Method::Post, "/v1/clip") => reply_later(app, request, |app| {
            match crate::save_clip_and_notify(app) {
                Ok(clip) => (
                    200,
                    json!({
                        "ok": true,
                        "id": clip.id,
                        "durationMs": clip.duration_ms,
                        "game": clip.game,
                    }),
                ),
                Err(error) => (409, json!({ "ok": false, "error": error })),
            }
        }),
        (Method::Post, "/v1/screenshot") => reply_later(app, request, |app| {
            match crate::take_screenshot_and_notify(app) {
                Ok(clip) => (
                    200,
                    json!({
                        "ok": true,
                        "id": clip.id,
                        "width": clip.width,
                        "height": clip.height,
                    }),
                ),
                Err(error) => (409, json!({ "ok": false, "error": error })),
            }
        }),
        (Method::Post, "/v1/buffer") => {
            // The body has to be read before the request moves onto the worker
            // thread — afterwards the reader is gone with it.
            let mut body = String::new();
            if request.as_reader().read_to_string(&mut body).is_err() {
                return respond(request, 400, json!({ "ok": false, "error": "unreadable body" }));
            }
            let action = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|value| value.get("action")?.as_str().map(str::to_owned))
                .unwrap_or_else(|| "toggle".into());
            match action.as_str() {
                "toggle" | "start" | "stop" => reply_later(app, request, move |app| {
                    match action.as_str() {
                        "start" => crate::start_buffer_and_notify(app),
                        "stop" => crate::stop_buffer_and_notify(app),
                        _ => crate::toggle_buffer_and_notify(app),
                    }
                    let active = app.state::<AppState>().status.lock().buffer_active;
                    (200, json!({ "ok": true, "bufferActive": active }))
                }),
                other => respond(
                    request,
                    400,
                    json!({ "ok": false, "error": format!("unknown action '{other}'") }),
                ),
            }
        }
        _ => respond(request, 404, json!({ "ok": false, "error": "no such route" })),
    }
}

/// Everything the Stream Deck key draws itself from.
///
/// Through `status_snapshot`, not `state.status` directly: the stored struct is
/// only the skeleton — buffer level, real frame rate and the detected game are
/// read out of the running recording there. Without it the key would say the
/// buffer runs and holds nothing, which is exactly what it looks like when
/// capture is broken.
fn status(app: &tauri::AppHandle) -> Value {
    let state = app.state::<AppState>();
    let status = state.status_snapshot();
    let config = state.config_snapshot();
    json!({
        "ok": true,
        "version": app.package_info().version.to_string(),
        "bufferActive": status.buffer_active,
        "bufferedSeconds": status.buffered_seconds,
        "bufferSeconds": config.buffer.seconds,
        "clipSeconds": crate::config::effective_clip_seconds(&config.buffer),
        "game": status.game,
        "encoder": status.encoder,
        "fps": status.fps,
        "droppedFrames": status.dropped_frames,
        "saving": state.is_saving(),
    })
}

/// Do the work on a thread of its own and answer when it is done.
///
/// Saving a clip takes a second or two — on the listening thread that would
/// block every other request, and answering before the file exists would rob
/// the key of the one thing it is for: showing whether the clip made it.
fn reply_later<F>(app: &tauri::AppHandle, request: Request, work: F)
where
    F: FnOnce(&tauri::AppHandle) -> (u16, Value) + Send + 'static,
{
    let app = app.clone();
    std::thread::spawn(move || {
        let (code, body) = work(&app);
        respond(request, code, body);
    });
}

fn respond(request: Request, code: u16, body: Value) {
    let json = Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
        .expect("a constant header");
    let response = Response::from_string(body.to_string())
        .with_status_code(code)
        .with_header(json);
    if let Err(err) = request.respond(response) {
        // The plugin gave up waiting, or the Stream Deck software was closed
        // mid-request. Nothing to fix here.
        log::debug!("control port could not answer: {err}");
    }
}

/// Compare without giving away how far the match got.
fn equals(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.is_empty() || a.len() != b.len() {
        return false;
    }
    let mut differing = 0u8;
    for (x, y) in a.iter().zip(b) {
        differing |= x ^ y;
    }
    differing == 0
}

/// The token from the config, generated on first use.
fn ensure_token(app: &tauri::AppHandle) -> String {
    let state = app.state::<AppState>();
    let existing = state.config_snapshot().control.token;
    if !existing.is_empty() {
        return existing;
    }

    let token = uuid::Uuid::new_v4().to_string();
    let mut config = state.config.lock();
    config.control.token = token.clone();
    // Deliberately not `replace_config`: that reapplies the audio sources, and
    // this runs while the buffer may already be recording.
    if let Err(err) = crate::config::save(&config) {
        log::warn!("could not save the control token: {err}");
    }
    token
}

pub fn handoff_path() -> std::path::PathBuf {
    crate::config::data_dir().join(HANDOFF)
}

/// Hand port and token to the plugin.
///
/// A file rather than a dialogue with a code to type: both sides run on this
/// machine under this user, and a setup nobody has to perform is one nobody can
/// get wrong.
fn write_handoff(app: &tauri::AppHandle, port: u16, token: &str) {
    let path = handoff_path();
    let body = json!({
        "port": port,
        "token": token,
        "version": app.package_info().version.to_string(),
    });
    let write = std::fs::create_dir_all(crate::config::data_dir())
        .and_then(|()| std::fs::write(&path, format!("{body:#}")));
    if let Err(err) = write {
        log::warn!("could not write {}: {err}", path.display());
    }
}

fn clear_handoff() {
    let _ = std::fs::remove_file(handoff_path());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty token must never let anything through — that is the state of a
    /// config written before this feature existed.
    #[test]
    fn an_empty_token_matches_nothing() {
        assert!(!equals("", ""));
        assert!(!equals("something", ""));
        assert!(!equals("", "something"));
    }

    #[test]
    fn the_token_has_to_match_whole() {
        assert!(equals("abc", "abc"));
        assert!(!equals("abc", "abd"));
        assert!(!equals("ab", "abc"));
        assert!(!equals("abcd", "abc"));
    }
}
