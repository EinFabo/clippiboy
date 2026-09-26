//! Friends: the account, the lists, and the live connection to the friends
//! server (`server/` in the repo, a Cloudflare worker).
//!
//! The core owns all of it rather than the page, for one reason: the main
//! window spends a game hidden in the tray, and that is exactly when friends
//! should see what you play. The game comes straight from `track_game`, the
//! connection lives on the async runtime, and the page only ever gets a
//! finished [`FriendsView`] to draw.
//!
//! Signing in goes through the browser: the server starts Discord's login, and
//! Discord's answer comes back as `clippiboy://auth?code=…`. That code alone is
//! worth nothing — only together with the verifier that stays in here does it
//! turn into a session, so another program that grabs the link gets nowhere.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, http::HeaderValue, Message};

use crate::state::AppState;

const SERVER: &str = "https://clippiboy-friends.fabian081964.workers.dev";
const SOCKET: &str = "wss://clippiboy-friends.fabian081964.workers.dev/ws";
/// The server closes a connection after 100 s without a word.
const PING_EVERY: Duration = Duration::from_secs(30);
/// No pong for this long and the connection is as good as dead — the laptop
/// slept, the network went. Better to dial again than to wait for TCP.
const PONG_DEADLINE: Duration = Duration::from_secs(75);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub avatar: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    #[serde(flatten)]
    pub user: User,
    pub friend_code: String,
    pub allow_requests: bool,
}

/// A request in either direction, with when it was made.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    #[serde(flatten)]
    pub user: User,
    pub since: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lists {
    pub friends: Vec<User>,
    pub incoming: Vec<Pending>,
    pub outgoing: Vec<Pending>,
    pub blocked: Vec<User>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Presence {
    pub game: Option<String>,
    /// When the game started, ms since the epoch.
    pub since: Option<i64>,
}

/// Everything the Friends page draws, sent whole on every change.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FriendsView {
    pub signed_in: bool,
    /// The browser is open and the app waits for Discord's answer.
    pub signing_in: bool,
    /// The live connection stands — without it, presence is stale.
    pub connected: bool,
    pub me: Option<Me>,
    pub lists: Lists,
    /// Friends who are online, by id. Missing means offline.
    pub presence: HashMap<String, Presence>,
}

#[derive(Default)]
pub struct Friends {
    view: Mutex<FriendsView>,
    token: Mutex<Option<String>>,
    /// Kept between opening the browser and the link coming back.
    verifier: Mutex<Option<String>>,
    /// The own game and since when, as `track_game` reports it.
    game: Mutex<Option<(String, i64)>>,
    /// Messages for the open connection, if one stands.
    outbox: Mutex<Option<mpsc::UnboundedSender<String>>>,
    /// Bumped on sign-out: a connection loop from before gives up.
    generation: AtomicU64,
}

// --- Setup ---------------------------------------------------------------------

/// At program start: signed in last time? Then connect straight away.
pub fn start(app: &AppHandle) {
    app.manage(Friends::default());
    let Some(token) = load_token() else { return };
    let friends = app.state::<Friends>();
    *friends.token.lock() = Some(token);
    friends.view.lock().signed_in = true;
    spawn_connection(app);
}

/// `clippiboy://auth?code=…`, handed over by the deep-link plugin.
pub fn handle_url(app: &AppHandle, url: &str) {
    let Some(rest) = url.strip_prefix("clippiboy://auth") else { return };
    let code = rest
        .trim_start_matches('/')
        .trim_start_matches('?')
        .split('&')
        .find_map(|pair| pair.strip_prefix("code="))
        .map(str::to_owned);
    let Some(code) = code else { return };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = finish_sign_in(&app, &code).await {
            log::warn!("friends: sign-in failed: {err}");
            app.state::<Friends>().view.lock().signing_in = false;
            emit(&app);
            crate::notify(&app, "error", err);
        }
    });
}

/// Every two seconds from the status tick: the game as the core holds it.
pub fn set_game(app: &AppHandle, game: Option<&str>) {
    let Some(friends) = app.try_state::<Friends>() else { return };
    {
        let mut current = friends.game.lock();
        if current.as_ref().map(|(name, _)| name.as_str()) == game {
            return;
        }
        *current = game.map(|name| (name.to_owned(), now_ms()));
    }
    send_presence(app, "presence");
}

/// The privacy settings changed — friends see the difference at once.
pub fn config_changed(app: &AppHandle) {
    send_presence(app, "presence");
}

// --- Commands ------------------------------------------------------------------

#[tauri::command]
pub fn friends_state(friends: tauri::State<'_, Friends>) -> FriendsView {
    friends.view.lock().clone()
}

#[tauri::command]
pub fn friends_sign_in(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let verifier = random_token();
    let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
    let friends = app.state::<Friends>();
    *friends.verifier.lock() = Some(verifier);
    friends.view.lock().signing_in = true;
    emit(&app);
    app.opener()
        .open_url(format!("{SERVER}/auth/start?challenge={challenge}"), None::<&str>)
        .map_err(|err| format!("Could not open the browser: {err}"))
}

#[tauri::command]
pub fn friends_cancel_sign_in(app: AppHandle) {
    let friends = app.state::<Friends>();
    *friends.verifier.lock() = None;
    friends.view.lock().signing_in = false;
    emit(&app);
}

#[tauri::command]
pub async fn friends_sign_out(app: AppHandle) -> Result<(), String> {
    // The server forgets the session; should it be unreachable, the token is
    // gone from here anyway and dies of age on the server.
    let _ = request(&app, reqwest::Method::POST, "/auth/logout", None).await;
    sign_out_locally(&app);
    Ok(())
}

#[tauri::command]
pub async fn friends_refresh(app: AppHandle) -> Result<(), String> {
    refresh(&app, false).await
}

/// `sent` or `accepted` — a request to someone who had already asked is a yes.
#[tauri::command]
pub async fn friends_request(app: AppHandle, query: String) -> Result<String, String> {
    let body = serde_json::json!({ "query": query });
    let answer = request(&app, reqwest::Method::POST, "/friends/request", Some(body)).await?;
    refresh(&app, false).await?;
    Ok(answer
        .get("status")
        .and_then(|status| status.as_str())
        .unwrap_or("sent")
        .to_owned())
}

#[tauri::command]
pub async fn friends_accept(app: AppHandle, id: String) -> Result<(), String> {
    request(&app, reqwest::Method::POST, &format!("/friends/{}/accept", checked_id(&id)?), None).await?;
    refresh(&app, false).await
}

/// Declines, withdraws, or unfriends.
#[tauri::command]
pub async fn friends_remove(app: AppHandle, id: String) -> Result<(), String> {
    request(&app, reqwest::Method::DELETE, &format!("/friends/{}", checked_id(&id)?), None).await?;
    refresh(&app, false).await
}

#[tauri::command]
pub async fn friends_block(app: AppHandle, id: String) -> Result<(), String> {
    request(&app, reqwest::Method::POST, &format!("/blocks/{}", checked_id(&id)?), None).await?;
    refresh(&app, false).await
}

#[tauri::command]
pub async fn friends_unblock(app: AppHandle, id: String) -> Result<(), String> {
    request(&app, reqwest::Method::DELETE, &format!("/blocks/{}", checked_id(&id)?), None).await?;
    refresh(&app, false).await
}

#[tauri::command]
pub async fn friends_set_allow_requests(app: AppHandle, allow: bool) -> Result<(), String> {
    let body = serde_json::json!({ "allowRequests": allow });
    let me = request(&app, reqwest::Method::PATCH, "/me", Some(body)).await?;
    let me: Me = serde_json::from_value(me).map_err(|err| err.to_string())?;
    app.state::<Friends>().view.lock().me = Some(me);
    emit(&app);
    Ok(())
}

#[tauri::command]
pub async fn friends_delete_account(app: AppHandle) -> Result<(), String> {
    request(&app, reqwest::Method::DELETE, "/me", None).await?;
    sign_out_locally(&app);
    Ok(())
}

// --- Sign-in -------------------------------------------------------------------

async fn finish_sign_in(app: &AppHandle, code: &str) -> Result<(), String> {
    let friends = app.state::<Friends>();
    let Some(verifier) = friends.verifier.lock().take() else {
        return Err("That sign-in was not started here — start it again from Friends.".into());
    };
    #[derive(Deserialize)]
    struct Session {
        token: String,
        me: Me,
    }
    let body = serde_json::json!({ "code": code, "verifier": verifier });
    let response = client()
        .post(format!("{SERVER}/auth/exchange"))
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|err| format!("The friends server is not reachable: {err}"))?;
    let session: Session = serde_json::from_value(read(response).await?).map_err(|err| err.to_string())?;
    save_token(Some(&session.token));
    *friends.token.lock() = Some(session.token);
    {
        let mut view = friends.view.lock();
        view.signed_in = true;
        view.signing_in = false;
        view.me = Some(session.me);
    }
    emit(app);
    crate::notify(app, "ok", "Signed in with Discord.");
    spawn_connection(app);
    Ok(())
}

fn sign_out_locally(app: &AppHandle) {
    let friends = app.state::<Friends>();
    friends.generation.fetch_add(1, Ordering::SeqCst);
    *friends.token.lock() = None;
    // Dropping the sender ends the connection loop's wait at once.
    *friends.outbox.lock() = None;
    *friends.view.lock() = FriendsView::default();
    save_token(None);
    emit(app);
}

// --- Lists ---------------------------------------------------------------------

/// Fetches profile and lists. With `announce`, whatever is new since the last
/// fetch — a request, an acceptance — is worth a notice; after one's own action
/// it is not.
async fn refresh(app: &AppHandle, announce: bool) -> Result<(), String> {
    let me = request(app, reqwest::Method::GET, "/me", None).await?;
    let lists = request(app, reqwest::Method::GET, "/friends", None).await?;
    let me: Me = serde_json::from_value(me).map_err(|err| err.to_string())?;
    let lists: Lists = serde_json::from_value(lists).map_err(|err| err.to_string())?;
    let friends = app.state::<Friends>();
    let previous = {
        let mut view = friends.view.lock();
        let previous = std::mem::replace(&mut view.lists, lists.clone());
        view.me = Some(me);
        // Presence of someone who is no longer a friend must not linger.
        let ids: HashSet<&str> = lists.friends.iter().map(|user| user.id.as_str()).collect();
        view.presence.retain(|id, _| ids.contains(id.as_str()));
        previous
    };
    emit(app);
    if announce {
        let known: HashSet<&str> = previous.incoming.iter().map(|p| p.user.id.as_str()).collect();
        for request in lists.incoming.iter().filter(|p| !known.contains(p.user.id.as_str())) {
            alert(app, Alert::Request, &format!("{} sent you a friend request", request.user.display_name));
        }
        let asked: HashSet<&str> = previous.outgoing.iter().map(|p| p.user.id.as_str()).collect();
        for friend in lists.friends.iter().filter(|user| asked.contains(user.id.as_str())) {
            alert(app, Alert::Request, &format!("{} accepted your friend request", friend.display_name));
        }
    }
    Ok(())
}

// --- Connection ----------------------------------------------------------------

fn spawn_connection(app: &AppHandle) {
    let generation = app.state::<Friends>().generation.load(Ordering::SeqCst);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut backoff = Duration::from_secs(1);
        loop {
            let friends = app.state::<Friends>();
            if friends.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            let Some(token) = friends.token.lock().clone() else { return };
            let started = Instant::now();
            match serve(&app, &token).await {
                Ok(End::SignedOut) => return,
                Ok(End::Dropped) => {}
                Err(err) => log::info!("friends: connection: {err}"),
            }
            {
                let mut view = friends.view.lock();
                view.connected = false;
                // Offline friends and online ones look the same while we cannot
                // tell — better than a list that pretends to still know.
                view.presence.clear();
            }
            *friends.outbox.lock() = None;
            emit(&app);
            // A connection that held for a while earns a quick retry.
            if started.elapsed() > Duration::from_secs(60) {
                backoff = Duration::from_secs(1);
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    });
}

enum End {
    /// The server does not know the token (any more) — stop dialling.
    SignedOut,
    /// The line went; dial again.
    Dropped,
}

async fn serve(app: &AppHandle, token: &str) -> Result<End, String> {
    install_crypto();
    let mut request = SOCKET.into_client_request().map_err(|err| err.to_string())?;
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|err| err.to_string())?,
    );
    let (socket, _) = match tokio_tungstenite::connect_async(request).await {
        Ok(ok) => ok,
        Err(tungstenite::Error::Http(response)) if response.status() == 401 => {
            log::info!("friends: session no longer valid — signed out");
            sign_out_locally(app);
            return Ok(End::SignedOut);
        }
        Err(err) => return Err(err.to_string()),
    };
    let (mut sink, mut stream) = socket.split();
    let (sender, mut outbox) = mpsc::unbounded_channel::<String>();
    let friends = app.state::<Friends>();
    *friends.outbox.lock() = Some(sender);
    friends.view.lock().connected = true;
    emit(app);
    // The lists may have changed while we were away.
    if let Err(err) = refresh(app, true).await {
        log::info!("friends: refresh failed: {err}");
    }
    sink.send(Message::Text(presence_message(app, "hello").into()))
        .await
        .map_err(|err| err.to_string())?;

    let mut ping = tokio::time::interval(PING_EVERY);
    let mut last_pong = Instant::now();
    loop {
        tokio::select! {
            incoming = stream.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    if text.as_str() == "pong" {
                        last_pong = Instant::now();
                    } else {
                        handle_message(app, text.as_str()).await;
                    }
                }
                Some(Ok(Message::Close(frame))) => {
                    // 4001: the account was deleted elsewhere.
                    if frame.is_some_and(|frame| u16::from(frame.code) == 4001) {
                        sign_out_locally(app);
                        return Ok(End::SignedOut);
                    }
                    return Ok(End::Dropped);
                }
                Some(Ok(_)) => {}
                Some(Err(err)) => return Err(err.to_string()),
                None => return Ok(End::Dropped),
            },
            outgoing = outbox.recv() => match outgoing {
                Some(text) => sink.send(Message::Text(text.into())).await.map_err(|err| err.to_string())?,
                // Signed out: the sender was dropped.
                None => {
                    let _ = sink.close().await;
                    return Ok(End::SignedOut);
                }
            },
            _ = ping.tick() => {
                if last_pong.elapsed() > PONG_DEADLINE {
                    return Err("no pong — connection presumed dead".into());
                }
                sink.send(Message::Text("ping".into())).await.map_err(|err| err.to_string())?;
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "camelCase")]
enum ServerMessage {
    Snapshot { friends: HashMap<String, Option<Presence>> },
    Presence { id: String, presence: Option<Presence> },
    Friends,
}

async fn handle_message(app: &AppHandle, text: &str) {
    let Ok(message) = serde_json::from_str::<ServerMessage>(text) else {
        log::info!("friends: unknown message {text}");
        return;
    };
    let friends = app.state::<Friends>();
    match message {
        ServerMessage::Snapshot { friends: all } => {
            friends.view.lock().presence =
                all.into_iter().filter_map(|(id, presence)| Some((id, presence?))).collect();
            emit(app);
        }
        ServerMessage::Presence { id, presence } => {
            let (name, before) = {
                let mut view = friends.view.lock();
                let name = view
                    .lists
                    .friends
                    .iter()
                    .find(|user| user.id == id)
                    .map(|user| user.display_name.clone());
                let before = match &presence {
                    Some(presence) => view.presence.insert(id, presence.clone()),
                    None => view.presence.remove(&id),
                };
                (name, before)
            };
            emit(app);
            if let (Some(name), Some(now)) = (name, presence) {
                match (before, now.game) {
                    (None, Some(game)) => alert(app, Alert::Game, &format!("{name} is playing {game}")),
                    (None, None) => alert(app, Alert::Online, &format!("{name} is online")),
                    (Some(before), Some(game)) if before.game.as_deref() != Some(game.as_str()) => {
                        alert(app, Alert::Game, &format!("{name} started {game}"))
                    }
                    _ => {}
                }
            }
        }
        ServerMessage::Friends => {
            if let Err(err) = refresh(app, true).await {
                log::info!("friends: refresh failed: {err}");
            }
        }
    }
}

fn presence_message(app: &AppHandle, kind: &str) -> String {
    let config = app.state::<AppState>().config_snapshot().friends;
    let game = if config.share_game {
        app.state::<Friends>().game.lock().clone()
    } else {
        None
    };
    serde_json::json!({
        "t": kind,
        "invisible": config.invisible,
        "game": game.as_ref().map(|(name, _)| name),
        "since": game.as_ref().map(|(_, since)| since),
    })
    .to_string()
}

fn send_presence(app: &AppHandle, kind: &str) {
    let Some(friends) = app.try_state::<Friends>() else { return };
    let Some(outbox) = friends.outbox.lock().clone() else { return };
    let _ = outbox.send(presence_message(app, kind));
}

// --- Notices -------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Alert {
    Request,
    Online,
    Game,
}

/// In the app while its window is in front; as a Windows notification while it
/// is not — but never over one's own game unless that is wanted.
fn alert(app: &AppHandle, kind: Alert, text: &str) {
    let config = app.state::<AppState>().config_snapshot().friends;
    let wanted = match kind {
        Alert::Request => config.notify_requests,
        Alert::Online => config.notify_online,
        Alert::Game => config.notify_games,
    };
    if !wanted {
        return;
    }
    let in_front = app
        .get_webview_window("main")
        .is_some_and(|window| window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false));
    if in_front {
        crate::notify(app, "ok", text);
        return;
    }
    if !config.notify_while_playing && app.state::<Friends>().game.lock().is_some() {
        return;
    }
    use tauri_plugin_notification::NotificationExt;
    if let Err(err) = app.notification().builder().title("ClippiBoy").body(text).show() {
        log::info!("friends: notification failed: {err}");
    }
}

// --- HTTP ----------------------------------------------------------------------

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        install_crypto();
        reqwest::Client::builder()
            .user_agent(concat!("ClippiBoy/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()
            .expect("HTTP client")
    })
}

/// reqwest and the socket run on the rustls the updater brings along, without
/// a crypto provider of their own — the same move `tools.rs` makes.
fn install_crypto() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

async fn request(
    app: &AppHandle,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let Some(token) = app.state::<Friends>().token.lock().clone() else {
        return Err("Not signed in.".into());
    };
    let mut builder = client()
        .request(method, format!("{SERVER}{path}"))
        .header("Authorization", format!("Bearer {token}"));
    if let Some(body) = body {
        builder = builder.header("Content-Type", "application/json").body(body.to_string());
    }
    let response = builder
        .send()
        .await
        .map_err(|_| "The friends server is not reachable right now.".to_string())?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        sign_out_locally(app);
    }
    read(response).await
}

/// The body as JSON, or the server's own words for what went wrong.
async fn read(response: reqwest::Response) -> Result<serde_json::Value, String> {
    let status = response.status();
    let text = response.text().await.map_err(|err| err.to_string())?;
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    if status.is_success() {
        return Ok(value);
    }
    Err(value
        .get("error")
        .and_then(|error| error.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("The friends server answered {status}.")))
}

/// Ids go into URL paths — only the shape the server hands out.
fn checked_id(id: &str) -> Result<&str, String> {
    if id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        Ok(id)
    } else {
        Err("Unknown user.".into())
    }
}

// --- Bits ----------------------------------------------------------------------

fn emit(app: &AppHandle) {
    let view = app.state::<Friends>().view.lock().clone();
    let _ = app.emit("friends-state", view);
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// 244 random bits, from the same source `uuid` draws on.
fn random_token() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

fn token_path() -> PathBuf {
    crate::config::data_dir().join("friends.json")
}

fn load_token() -> Option<String> {
    #[derive(Deserialize)]
    struct Stored {
        token: String,
    }
    let text = std::fs::read_to_string(token_path()).ok()?;
    serde_json::from_str::<Stored>(&text).ok().map(|stored| stored.token)
}

fn save_token(token: Option<&str>) {
    let path = token_path();
    let result = match token {
        Some(token) => std::fs::create_dir_all(crate::config::data_dir())
            .and_then(|_| std::fs::write(&path, serde_json::json!({ "token": token }).to_string())),
        None => std::fs::remove_file(&path).or_else(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(err)
            }
        }),
    };
    if let Err(err) = result {
        log::warn!("friends: could not store the session: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_matches_the_server() {
        // What `base64url(sha256("abc"))` gives in the worker.
        assert_eq!(
            base64url(&Sha256::digest(b"abc")),
            "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0"
        );
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
    }

    #[test]
    fn ids_are_checked() {
        assert!(checked_id("00000000-0000-4000-8000-00000000000a").is_ok());
        assert!(checked_id("../me").is_err());
    }
}
