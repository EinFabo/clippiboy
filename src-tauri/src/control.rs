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

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{json, Value};
use tauri::Manager;

use crate::state::AppState;

/// The handshake file next to `config.json`. The plugin reads port and token
/// from here.
const HANDOFF: &str = "streamdeck.json";

/// Which header carries the token. Not `Authorization`: nothing here is a
/// standard scheme, and a name of our own says plainly that this is ours.
const TOKEN_HEADER: &str = "x-clippiboy-token";

/// Limits on what a request may be. The port used to run on tiny_http, which
/// read the whole of a refused request's body before closing — any program on
/// this machine could send a `Content-Length` of terabytes without the token
/// and either hang the one serving thread for good or make the app allocate
/// that much and abort. Here a request that is too big, too slow or too many
/// is closed, not drained.
const MAX_HEAD: usize = 8 * 1024;
const MAX_BODY: usize = 4 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(2);
/// For the whole request: [`READ_TIMEOUT`] holds for one read only, and a
/// client sending a byte every 1.9 s would keep a connection for hours —
/// eight of them and the Stream Deck is locked out.
const REQUEST_DEADLINE: Duration = Duration::from_secs(5);
const MAX_CONNECTIONS: usize = 8;

struct Running {
    stop: Arc<AtomicBool>,
    /// Kept so a restart can wait for the old thread — see [`stop`].
    thread: std::thread::JoinHandle<()>,
}

/// The server currently listening, if there is one.
static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

/// Connections being served right now.
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

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
    //
    // Taken, any free one instead: the plugin reads the port from the
    // handshake file, not from the settings. WSL in mirrored networking mode
    // reserves its whole range of ephemeral ports on Windows — 44620–48715 by
    // default, with 47653 in the middle — and with the configured port alone
    // the keys were dead whenever WSL ran.
    let bound = TcpListener::bind(("127.0.0.1", port)).or_else(|err| {
        log::warn!("control port {port} will not open ({err}) — taking a free one");
        TcpListener::bind(("127.0.0.1", 0))
    });
    let listener = match bound.and_then(|listener| listener.set_nonblocking(true).map(|()| listener)) {
        Ok(listener) => listener,
        Err(err) => {
            log::warn!("control port will not open ({err}) — Stream Deck keys stay silent");
            clear_handoff();
            return;
        }
    };
    let port = listener.local_addr().map(|address| address.port()).unwrap_or(port);

    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let stop = stop.clone();
        let app = app.clone();
        let token = token.clone();
        std::thread::spawn(move || serve(app, listener, stop, token))
    };

    *RUNNING.lock() = Some(Running { stop, thread });
    write_handoff(app, port, &token);
    log::info!("control port listening on 127.0.0.1:{port}");
}

/// Stop the control port and take the handshake file with it.
///
/// Waits for the listening thread, and that is the point of it: the thread holds
/// the socket, so without the wait a `restart` onto the same port would find it
/// still taken and quietly give up.
pub fn stop() {
    if let Some(running) = RUNNING.lock().take() {
        running.stop.store(true, Ordering::SeqCst);
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

/// Accept connections until told to stop. Non-blocking with a short nap, so
/// `stop` needs no trick to wake it.
fn serve(app: tauri::AppHandle, listener: TcpListener, stop: Arc<AtomicBool>, token: String) {
    let token: Arc<str> = token.into();
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                if ACTIVE.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                    ACTIVE.fetch_sub(1, Ordering::SeqCst);
                    continue; // dropped, i.e. closed
                }
                let app = app.clone();
                let token = token.clone();
                // Each on its own thread: saving a clip takes a second or two,
                // and the answer waits for it — the key shows whether the clip
                // made it.
                std::thread::spawn(move || {
                    connection(&app, stream, &token);
                    ACTIVE.fetch_sub(1, Ordering::SeqCst);
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(err) => {
                log::debug!("control port: {err}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    log::info!("control port stopped");
}

/// One request, one answer, then the connection closes.
fn connection(app: &tauri::AppHandle, mut stream: TcpStream, token: &str) {
    let setup = stream
        .set_nonblocking(false)
        .and_then(|()| stream.set_read_timeout(Some(READ_TIMEOUT)))
        .and_then(|()| stream.set_write_timeout(Some(Duration::from_secs(5))));
    if setup.is_err() {
        return;
    }
    let (code, body) = match read_request(&mut stream, std::time::Instant::now() + REQUEST_DEADLINE) {
        Ok(request) => handle(app, &request, token),
        Err((code, error)) => (code, json!({ "ok": false, "error": error })),
    };
    if let Err(err) = write_response(&mut stream, code, &body) {
        // The plugin gave up waiting, or the Stream Deck software was closed
        // mid-request. Nothing to fix here.
        log::debug!("control port could not answer: {err}");
    }
}

/// A request as far as this port cares.
#[derive(Debug, PartialEq)]
struct Request {
    method: String,
    /// The path, without the query — nothing here uses one, and cutting it off
    /// keeps a stray `?x=1` from turning into a 404.
    route: String,
    /// Header names in lower case.
    headers: Vec<(String, String)>,
    body: String,
}

/// Read one request, within [`MAX_HEAD`] and [`MAX_BODY`] and by `deadline`.
/// The error is the status to answer with.
fn read_request(stream: &mut impl Read, deadline: std::time::Instant) -> Result<Request, (u16, &'static str)> {
    let in_time = || {
        if std::time::Instant::now() > deadline {
            Err((408, "request too slow"))
        } else {
            Ok(())
        }
    };
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let head_end = loop {
        if let Some(at) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break at;
        }
        if buffer.len() > MAX_HEAD {
            return Err((431, "request head too large"));
        }
        match stream.read(&mut chunk) {
            Ok(0) => return Err((400, "incomplete request")),
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            Err(_) => return Err((408, "request too slow")),
        }
        in_time()?;
    };
    let head = std::str::from_utf8(&buffer[..head_end]).map_err(|_| (400, "unreadable head"))?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let method = first.next().unwrap_or("").to_string();
    let target = first.next().unwrap_or("");
    let route = target.split('?').next().unwrap_or("").to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let header = |name: &str| headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str());

    if header("transfer-encoding").is_some() {
        return Err((411, "send a Content-Length"));
    }
    let length = match header("content-length") {
        Some(value) => value.parse::<usize>().map_err(|_| (400, "bad Content-Length"))?,
        None => 0,
    };
    if length > MAX_BODY {
        return Err((413, "body too large"));
    }
    let mut body = buffer[head_end + 4..].to_vec();
    body.truncate(length);
    while body.len() < length {
        let want = (length - body.len()).min(chunk.len());
        match stream.read(&mut chunk[..want]) {
            Ok(0) => return Err((400, "incomplete body")),
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(_) => return Err((408, "request too slow")),
        }
        in_time()?;
    }
    let body = String::from_utf8(body).map_err(|_| (400, "unreadable body"))?;
    Ok(Request { method, route, headers, body })
}

fn write_response(stream: &mut impl Write, code: u16, body: &Value) -> std::io::Result<()> {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    };
    let body = body.to_string();
    // In one piece: `write!` straight onto the socket sent every fragment of
    // the format as a packet of its own.
    let response = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn handle(app: &tauri::AppHandle, request: &Request, token: &str) -> (u16, Value) {
    let mut supplied: Option<&str> = None;
    // A browser sets `Origin` on every cross-site request it makes. Nothing that
    // belongs here ever sends one, so refusing them closes the door on a web
    // page firing clips through a rebound DNS name — even if it somehow knew
    // the token.
    let mut from_browser = false;
    for (name, value) in &request.headers {
        match name.as_str() {
            "origin" | "referer" | "sec-fetch-site" => from_browser = true,
            TOKEN_HEADER => supplied = Some(value),
            _ => {}
        }
    }

    let allowed = !from_browser && supplied.is_some_and(|supplied| equals(supplied, token));
    if !allowed {
        return (403, json!({ "ok": false, "error": "forbidden" }));
    }
    if request.method == "POST" {
        crate::telemetry::used(crate::telemetry::Feature::StreamDeck);
    }
    let action = || {
        serde_json::from_str::<Value>(&request.body)
            .ok()
            .and_then(|value| value.get("action")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| "toggle".into())
    };

    match (request.method.as_str(), request.route.as_str()) {
        ("GET", "/v1/status") => (200, status(app)),
        ("POST", "/v1/clip") => {
            // Optional `{"seconds": 150}`: a key set to a length of its own.
            // Without it, or unreadable, the configured clip length applies.
            let seconds = serde_json::from_str::<Value>(&request.body)
                .ok()
                .and_then(|value| value.get("seconds")?.as_u64())
                .filter(|&seconds| seconds > 0)
                .map(|seconds| seconds.min(u32::MAX as u64) as u32);
            match crate::save_clip_and_notify(app, seconds) {
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
        }
        ("POST", "/v1/screenshot") => match crate::take_screenshot_and_notify(app) {
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
        },
        ("POST", "/v1/buffer") => match action().as_str() {
            action @ ("toggle" | "start" | "stop") => {
                match action {
                    "start" => crate::start_buffer_and_notify(app),
                    "stop" => crate::stop_buffer_and_notify(app),
                    _ => crate::toggle_buffer_and_notify(app),
                }
                let active = app.state::<AppState>().status.lock().buffer_active;
                (200, json!({ "ok": true, "bufferActive": active }))
            }
            other => (400, json!({ "ok": false, "error": format!("unknown action '{other}'") })),
        },
        ("POST", "/v1/recording") => match action().as_str() {
            // Answered once the work is done — for "stop" that is when the
            // file is written, so the key can show the check mark.
            action @ ("toggle" | "start" | "stop") => {
                let running = app.state::<AppState>().is_recording();
                let outcome = match (action, running) {
                    ("start", true) | ("stop", false) => Ok(()),
                    (_, true) => crate::stop_recording_and_notify(app).map(|_| ()),
                    (_, false) => {
                        crate::start_recording_and_notify(app);
                        Ok(())
                    }
                };
                let recording = app.state::<AppState>().is_recording();
                match outcome {
                    Ok(()) => (200, json!({ "ok": true, "recording": recording })),
                    Err(error) => (409, json!({ "ok": false, "error": error, "recording": recording })),
                }
            }
            other => (400, json!({ "ok": false, "error": format!("unknown action '{other}'") })),
        },
        _ => (404, json!({ "ok": false, "error": "no such route" })),
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
        "finalizing": state.is_finalizing(),
        "recording": status.recording,
        "recordingSeconds": status.recording_seconds,
    })
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


    fn read(raw: &[u8]) -> Result<Request, (u16, &'static str)> {
        read_request(&mut std::io::Cursor::new(raw.to_vec()), std::time::Instant::now() + REQUEST_DEADLINE)
    }

    #[test]
    fn a_request_is_read_with_its_body_and_without_its_query() {
        let request = read(
            b"POST /v1/clip?x=1 HTTP/1.1\r\nHost: 127.0.0.1\r\nX-ClippiBoy-Token: abc\r\n\
              Content-Length: 15\r\n\r\n{\"seconds\": 30}",
        )
        .unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.route, "/v1/clip");
        assert!(request.headers.contains(&(TOKEN_HEADER.into(), "abc".into())));
        assert_eq!(request.body, "{\"seconds\": 30}");
    }

    /// The attack on the old server: a huge declared body, never sent. It has
    /// to be turned away from the head alone — not read, not allocated.
    #[test]
    fn a_huge_body_is_refused_before_it_is_read() {
        let refused = read(b"POST /v1/clip HTTP/1.1\r\nContent-Length: 99999999999999\r\n\r\n");
        assert_eq!(refused.unwrap_err().0, 413);
        let chunked = read(b"POST /v1/clip HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n");
        assert_eq!(chunked.unwrap_err().0, 411);
    }

    #[test]
    fn an_endless_head_is_cut_off() {
        let mut raw = b"GET /v1/status HTTP/1.1\r\n".to_vec();
        raw.extend(std::iter::repeat_n(b'a', MAX_HEAD + 10));
        assert_eq!(read(&raw).unwrap_err().0, 431);
    }

    #[test]
    fn a_body_that_stops_short_is_not_taken() {
        assert_eq!(read(b"POST /v1/buffer HTTP/1.1\r\nContent-Length: 50\r\n\r\n{}").unwrap_err().0, 400);
    }

    /// Over a real socket, as the attack came: the server side reads with the
    /// port's timeout and has to let go on its own.
    fn served(send: &'static [u8], hold: Duration) -> (Result<Request, (u16, &'static str)>, Duration) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            stream.write_all(send).unwrap();
            // Keeps the connection open without sending more.
            std::thread::sleep(hold);
        });
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(READ_TIMEOUT)).unwrap();
        let started = std::time::Instant::now();
        let outcome = read_request(&mut stream, started + REQUEST_DEADLINE);
        let took = started.elapsed();
        client.join().unwrap();
        (outcome, took)
    }

    #[test]
    fn a_huge_declared_body_is_turned_away_at_once_over_a_socket() {
        let (outcome, took) = served(
            b"POST /v1/clip HTTP/1.1\r\nContent-Length: 99999999999\r\n\r\n",
            Duration::from_secs(3),
        );
        assert_eq!(outcome.unwrap_err().0, 413);
        assert!(took < Duration::from_millis(500), "waited {took:?} for a body that never comes");
    }

    #[test]
    fn a_client_that_stalls_is_let_go_after_the_timeout() {
        let (outcome, took) = served(b"GET /v1/status HTTP/1.1\r\nX-Cl", Duration::from_secs(4));
        assert_eq!(outcome.unwrap_err().0, 408);
        assert!(took >= READ_TIMEOUT && took < READ_TIMEOUT * 2, "took {took:?}");
    }

    /// One byte per read, each well inside the read timeout.
    struct Drip(&'static [u8], usize);

    impl Read for Drip {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            std::thread::sleep(Duration::from_millis(30));
            let Some(&byte) = self.0.get(self.1) else {
                return Ok(0);
            };
            self.1 += 1;
            out[0] = byte;
            Ok(1)
        }
    }

    #[test]
    fn a_client_that_drips_its_request_is_let_go_at_the_deadline() {
        let started = std::time::Instant::now();
        let mut drip = Drip(b"GET /v1/status HTTP/1.1\r\nX-Pad: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n\r\n", 0);
        let outcome = read_request(&mut drip, started + Duration::from_millis(200));
        assert_eq!(outcome.unwrap_err().0, 408);
        let took = started.elapsed();
        assert!(took < Duration::from_millis(400), "took {took:?}");
    }

    #[test]
    fn the_token_has_to_match_whole() {
        assert!(equals("abc", "abc"));
        assert!(!equals("abc", "abd"));
        assert!(!equals("ab", "abc"));
        assert!(!equals("abcd", "abc"));
    }
}
