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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

pub(crate) const SERVER: &str = "https://api.clippiboy.com";
const SOCKET: &str = "wss://api.clippiboy.com/ws";
/// The server closes a connection after 100 s without a word.
const PING_EVERY: Duration = Duration::from_secs(30);
/// No pong for this long and the connection is as good as dead — the laptop
/// slept, the network went. Better to dial again than to wait for TCP.
const PONG_DEADLINE: Duration = Duration::from_secs(75);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A dial (TCP, TLS and the upgrade) or a send that takes longer than this
/// hangs: the line is given up and dialled again, as for a missing pong.
const SOCKET_TIMEOUT: Duration = Duration::from_secs(15);

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
    /// Their own line under the name.
    #[serde(default)]
    pub status: Option<String>,
    /// Do not disturb — no clips to them right now.
    #[serde(default)]
    pub busy: bool,
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
    /// When an offline friend was last seen, ms since the epoch, by id.
    pub last_seen: HashMap<String, i64>,
    /// The last `clippiboy://add?code=…` link: the page puts the code into the
    /// Add field once per `seq`. The user still presses Add — a link from any
    /// web page must not send a request on its own.
    pub add: Option<AddLink>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AddLink {
    pub code: String,
    pub seq: u64,
}

#[derive(Default)]
pub struct Friends {
    view: Mutex<FriendsView>,
    token: Mutex<Option<String>>,
    /// Kept between opening the browser and the link coming back.
    verifier: Mutex<Option<String>>,
    /// The own game and since when, as `track_game` reports it.
    game: Mutex<Option<(String, i64)>>,
    /// The user is at that game right now — its window in front, or one of
    /// ours over it ([`set_game`]).
    playing: AtomicBool,
    /// Clip banners waiting to go up as one while playing ([`clip_alert`]).
    bundle: Mutex<Vec<ClipNotice>>,
    /// Messages for the open connection, if one stands.
    outbox: Mutex<Option<mpsc::UnboundedSender<String>>>,
    /// Bumped on sign-out: a connection loop from before gives up.
    generation: AtomicU64,
    /// When a friend request from someone was last announced, ms since the
    /// epoch, by id: withdrawing and sending again must not pop up every time.
    announced: Mutex<HashMap<String, i64>>,
    /// "Retry now": ends the connection loop's wait before the next dial.
    redial: tokio::sync::Notify,
    /// The lists have been fetched since signing in ([`lists_loaded`]). Not
    /// the same as `view.me`: a fresh sign-in brings the profile, not the lists.
    loaded: AtomicBool,
}

// --- Setup ---------------------------------------------------------------------

/// The session token, if signed in — for the share links, which talk to the
/// same server.
pub(crate) fn token(app: &AppHandle) -> Option<String> {
    app.try_state::<Friends>()?.token.lock().clone()
}

/// The signed-in Discord account's id.
pub(crate) fn my_id(app: &AppHandle) -> Option<String> {
    let friends = app.try_state::<Friends>()?;
    let view = friends.view.lock();
    view.me.as_ref().map(|me| me.user.id.clone())
}

/// Whether the friends lists have been loaded at least once this session.
pub(crate) fn lists_loaded(app: &AppHandle) -> bool {
    app.try_state::<Friends>()
        .is_some_and(|friends| friends.loaded.load(Ordering::SeqCst))
}

/// At program start: signed in last time? Then connect straight away.
pub fn start(app: &AppHandle) {
    app.manage(Friends::default());
    let Some(token) = load_token() else { return };
    let friends = app.state::<Friends>();
    *friends.token.lock() = Some(token);
    friends.view.lock().signed_in = true;
    spawn_connection(app);
}

/// `clippiboy://auth?code=…` and `clippiboy://add?code=…`, handed over by the
/// deep-link plugin.
pub fn handle_url(app: &AppHandle, url: &str) {
    // `clippiboy://webhook?data=…`: a channel to post clips to.
    if crate::webhook::handle_url(app, url) {
        return;
    }
    if let Some(code) = add_code_from_url(url) {
        // "Open ClippiBoy" after "Add <name> as friend" on clippiboy.com.
        let friends = app.state::<Friends>();
        {
            let mut view = friends.view.lock();
            let seq = view.add.as_ref().map_or(0, |add| add.seq) + 1;
            view.add = Some(AddLink { code, seq });
        }
        emit(app);
        crate::tray::show_main_window(app);
        return;
    }
    let Some(rest) = url.strip_prefix("clippiboy://auth") else {
        return;
    };
    let code = rest
        .trim_start_matches('/')
        .trim_start_matches('?')
        .split('&')
        .find_map(|pair| pair.strip_prefix("code="))
        .map(str::to_owned);
    let Some(code) = code else { return };
    // A link nobody asked for (an old tab, a pasted URL) must not wipe a sign-in or raise a notice.
    if app.state::<Friends>().verifier.lock().is_none() {
        log::info!("friends: ignored an auth link, no sign-in is waiting");
        return;
    }
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

/// Every two seconds from the status tick: the game as the core holds it,
/// and whether the user is at it right now.
pub fn set_game(app: &AppHandle, game: Option<&str>, playing: bool) {
    let Some(friends) = app.try_state::<Friends>() else {
        return;
    };
    friends.playing.store(playing, Ordering::SeqCst);
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
    // The Discord status switch lives among these settings too.
    crate::presence::nudge();
}

// --- Commands ------------------------------------------------------------------

#[tauri::command]
pub fn friends_state(friends: tauri::State<'_, Friends>) -> FriendsView {
    friends.view.lock().clone()
}

/// The verifier of the sign-in that is waiting, or a new one kept in its place.
fn pending(slot: &mut Option<String>) -> String {
    slot.get_or_insert_with(random_token).clone()
}

#[tauri::command]
pub fn friends_sign_in(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let friends = app.state::<Friends>();
    // Pressing the button again keeps the verifier, so the first browser tab still finishes.
    let verifier = pending(&mut friends.verifier.lock());
    let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
    friends.view.lock().signing_in = true;
    emit(&app);
    app.opener()
        .open_url(
            format!("{SERVER}/auth/start?challenge={challenge}"),
            None::<&str>,
        )
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

/// "Retry now" while the server cannot be reached: the next dial goes out at
/// once instead of after the backoff. A press during a dial is kept for the
/// wait after it, should that dial fail too.
#[tauri::command]
pub fn friends_reconnect(app: AppHandle) {
    let friends = app.state::<Friends>();
    let signed_in = friends.token.lock().is_some();
    if signed_in && !friends.view.lock().connected {
        friends.redial.notify_one();
    }
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
    request(
        &app,
        reqwest::Method::POST,
        &format!("/friends/{}/accept", checked_id(&id)?),
        None,
    )
    .await?;
    refresh(&app, false).await
}

/// Declines, withdraws, or unfriends.
#[tauri::command]
pub async fn friends_remove(app: AppHandle, id: String) -> Result<(), String> {
    request(
        &app,
        reqwest::Method::DELETE,
        &format!("/friends/{}", checked_id(&id)?),
        None,
    )
    .await?;
    refresh(&app, false).await
}

#[tauri::command]
pub async fn friends_block(app: AppHandle, id: String) -> Result<(), String> {
    request(
        &app,
        reqwest::Method::POST,
        &format!("/blocks/{}", checked_id(&id)?),
        None,
    )
    .await?;
    refresh(&app, false).await
}

#[tauri::command]
pub async fn friends_unblock(app: AppHandle, id: String) -> Result<(), String> {
    request(
        &app,
        reqwest::Method::DELETE,
        &format!("/blocks/{}", checked_id(&id)?),
        None,
    )
    .await?;
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
    // The server took the account's share links along.
    crate::links::forget_all(&app);
    Ok(())
}

// --- Sign-in -------------------------------------------------------------------

async fn finish_sign_in(app: &AppHandle, code: &str) -> Result<(), String> {
    let friends = app.state::<Friends>();
    let Some(verifier) = friends.verifier.lock().clone() else {
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
    let session: Session =
        serde_json::from_value(read(response).await?).map_err(|err| err.to_string())?;
    // Only a sign-in that worked uses the verifier up.
    *friends.verifier.lock() = None;
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

/// The server turned the session away — a share link heard it first.
pub(crate) fn session_rejected(app: &AppHandle) {
    sign_out_locally(app);
}

fn sign_out_locally(app: &AppHandle) {
    let friends = app.state::<Friends>();
    friends.generation.fetch_add(1, Ordering::SeqCst);
    *friends.token.lock() = None;
    // Dropping the sender ends the connection loop's wait at once.
    *friends.outbox.lock() = None;
    *friends.view.lock() = FriendsView::default();
    friends.loaded.store(false, Ordering::SeqCst);
    save_token(None);
    emit(app);
}

/// Signs out when `token` is still the session's: a request or a dial of a
/// session signed out since — the old one's last request, answered 401 just
/// after a sign-in — must not sign the new one out. True when it did.
fn sign_out_if_current(app: &AppHandle, token: &str) -> bool {
    let current = same_session(app.state::<Friends>().token.lock().as_deref(), token);
    if current {
        sign_out_locally(app);
    }
    current
}

fn same_session(current: Option<&str>, used: &str) -> bool {
    current == Some(used)
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
        view.last_seen.retain(|id, _| ids.contains(id.as_str()));
        previous
    };
    friends.loaded.store(true, Ordering::SeqCst);
    emit(app);
    // Offers that came before the lists did can be looked at now.
    crate::share::retry_held(app);
    if announce {
        let known: HashSet<&str> = previous
            .incoming
            .iter()
            .map(|p| p.user.id.as_str())
            .collect();
        let now = now_ms();
        for request in lists
            .incoming
            .iter()
            .filter(|p| !known.contains(p.user.id.as_str()))
        {
            if !first_today(&mut friends.announced.lock(), &request.user.id, now) {
                continue;
            }
            alert(
                app,
                Alert::Request,
                &request.user.id,
                &request.user.display_name,
                &format!("{} sent you a friend request", request.user.display_name),
                request.user.avatar.clone(),
            );
        }
        let asked: HashSet<&str> = previous
            .outgoing
            .iter()
            .map(|p| p.user.id.as_str())
            .collect();
        for friend in lists
            .friends
            .iter()
            .filter(|user| asked.contains(user.id.as_str()))
        {
            alert(
                app,
                Alert::Accepted,
                &friend.id,
                &friend.display_name,
                &format!("{} accepted your friend request", friend.display_name),
                friend.avatar.clone(),
            );
        }
    }
    Ok(())
}

/// Forgets what is older than a day, then notes `id`: true if it was not there.
fn first_today(seen: &mut HashMap<String, i64>, id: &str, now: i64) -> bool {
    seen.retain(|_, at| now - *at < 24 * 3600 * 1000);
    if seen.contains_key(id) {
        return false;
    }
    seen.insert(id.to_owned(), now);
    true
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
            let Some(token) = friends.token.lock().clone() else {
                return;
            };
            let started = Instant::now();
            match serve(&app, &token, generation).await {
                Ok(End::SignedOut) => return,
                Ok(End::Dropped) => {}
                Err(err) => log::info!("friends: connection: {err}"),
            }
            // Signed out meanwhile, and maybe in again: what is there now is
            // the new session's, its line and its list.
            if friends.generation.load(Ordering::SeqCst) != generation {
                return;
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
            // Signed out meanwhile: a loop from before must not sit in the
            // wait below, where it would take a "Retry now" meant for the next.
            if friends.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            let (wait, next) = next_backoff(backoff, random_unit());
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = friends.redial.notified() => {
                    // Signed out and in again during the wait: the press was
                    // for the loop of the new sign-in — pass it on.
                    if friends.generation.load(Ordering::SeqCst) != generation {
                        friends.redial.notify_one();
                        return;
                    }
                    log::info!("friends: dialling again on request");
                }
            }
            backoff = next;
        }
    });
}

/// The wait before the next dial and the step after it. The wait is somewhere
/// in the upper half of the step (`unit` in 0..1), so the apps a server restart
/// dropped all at once do not all dial back in the same instant; the step
/// doubles up to [`BACKOFF_MAX`].
fn next_backoff(step: Duration, unit: f64) -> (Duration, Duration) {
    let unit = if unit.is_finite() {
        unit.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let wait = step.mul_f64(0.5 + unit / 2.0).min(step);
    (wait, (step * 2).min(BACKOFF_MAX))
}

/// A number in 0..1 for the jitter; 1 (the full step, as before) should the
/// system's randomness fail.
fn random_unit() -> f64 {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 4];
    match ring::rand::SystemRandom::new().fill(&mut bytes) {
        Ok(()) => f64::from(u32::from_le_bytes(bytes)) / (f64::from(u32::MAX) + 1.0),
        Err(_) => 1.0,
    }
}

/// A send on the socket that does not finish in time ends the line.
async fn send_within<S>(sink: &mut S, message: Message) -> Result<(), String>
where
    S: futures_util::Sink<Message, Error = tungstenite::Error> + Unpin,
{
    match tokio::time::timeout(SOCKET_TIMEOUT, sink.send(message)).await {
        Ok(sent) => sent.map_err(|err| err.to_string()),
        Err(_) => Err("send timed out — connection presumed dead".into()),
    }
}

enum End {
    /// The server does not know the token (any more) — stop dialling.
    SignedOut,
    /// The line went; dial again.
    Dropped,
}

async fn serve(app: &AppHandle, token: &str, generation: u64) -> Result<End, String> {
    install_crypto();
    let mut request = SOCKET
        .into_client_request()
        .map_err(|err| err.to_string())?;
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|err| err.to_string())?,
    );
    // Without a limit a dial into a network that swallows packets waited on
    // the system's TCP timeouts, and nothing dialled again meanwhile.
    let dialled = tokio::time::timeout(SOCKET_TIMEOUT, tokio_tungstenite::connect_async(request))
        .await
        .map_err(|_| "dial timed out".to_string())?;
    let (socket, _) = match dialled {
        Ok(ok) => ok,
        Err(tungstenite::Error::Http(response)) if response.status() == 401 => {
            if sign_out_if_current(app, token) {
                log::info!("friends: session no longer valid — signed out");
            }
            return Ok(End::SignedOut);
        }
        Err(err) => return Err(err.to_string()),
    };
    let (mut sink, mut stream) = socket.split();
    let (sender, mut outbox) = mpsc::unbounded_channel::<String>();
    let friends = app.state::<Friends>();
    {
        // Checked under the outbox's lock: a sign-out raises the generation
        // before it takes the outbox away, so either this sees it, or the
        // sign-out ends this line right after.
        let mut current = friends.outbox.lock();
        if friends.generation.load(Ordering::SeqCst) != generation {
            // A dial of a session signed out meanwhile: the line of the one
            // signed in since is not this one to take over.
            return Ok(End::SignedOut);
        }
        *current = Some(sender);
    }
    friends.view.lock().connected = true;
    // A "Retry now" pressed during this dial has done its work: it must not
    // cut short the wait after some later drop.
    let _ = futures_util::FutureExt::now_or_never(friends.redial.notified());
    crate::telemetry::used(crate::telemetry::Feature::Friends);
    emit(app);
    // The lists may have changed while we were away.
    if let Err(err) = refresh(app, true).await {
        log::info!("friends: refresh failed: {err}");
    }
    send_within(
        &mut sink,
        Message::Text(presence_message(app, "hello").into()),
    )
    .await?;

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
                    // 4001: signed out, suspended or deleted elsewhere. Any
                    // other code is a drop to dial again — 4004 too, which an
                    // account's deletion sends before anything is deleted, so
                    // one that fails leaves the app signed in.
                    if frame.is_some_and(|frame| u16::from(frame.code) == 4001) {
                        sign_out_if_current(app, token);
                        return Ok(End::SignedOut);
                    }
                    return Ok(End::Dropped);
                }
                Some(Ok(_)) => {}
                Some(Err(err)) => return Err(err.to_string()),
                None => return Ok(End::Dropped),
            },
            outgoing = outbox.recv() => match outgoing {
                Some(text) => send_within(&mut sink, Message::Text(text.into())).await?,
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
                send_within(&mut sink, Message::Text("ping".into())).await?;
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "camelCase")]
enum ServerMessage {
    Snapshot {
        friends: HashMap<String, Option<Presence>>,
        #[serde(default, rename = "lastSeen")]
        last_seen: HashMap<String, Option<i64>>,
    },
    Presence {
        id: String,
        presence: Option<Presence>,
        #[serde(default, rename = "lastSeen")]
        last_seen: Option<i64>,
    },
    Friends,
    /// A friend's app sent this one something — the clip handshake.
    Relay {
        from: String,
        body: serde_json::Value,
    },
    /// The server could not pass one of ours on.
    RelayFailed {
        #[serde(default, rename = "ref")]
        reference: Option<serde_json::Value>,
        reason: String,
    },
}

async fn handle_message(app: &AppHandle, text: &str) {
    let Ok(message) = serde_json::from_str::<ServerMessage>(text) else {
        log::info!("friends: unknown message {text}");
        return;
    };
    let friends = app.state::<Friends>();
    match message {
        ServerMessage::Snapshot {
            friends: all,
            last_seen,
        } => {
            {
                let mut view = friends.view.lock();
                view.presence = all
                    .into_iter()
                    .filter_map(|(id, presence)| Some((id, presence?)))
                    .collect();
                view.last_seen = last_seen
                    .into_iter()
                    .filter_map(|(id, at)| Some((id, at?)))
                    .collect();
            }
            emit(app);
        }
        ServerMessage::Relay { from, body } => crate::share::handle_relay(app, &from, body),
        ServerMessage::RelayFailed { reference, reason } => {
            crate::share::relay_failed(app, reference.as_ref().and_then(|r| r.as_str()), &reason)
        }
        ServerMessage::Presence {
            id,
            presence,
            last_seen,
        } => {
            let (name, before) = {
                let mut view = friends.view.lock();
                if let Some(at) = last_seen {
                    view.last_seen.insert(id.clone(), at);
                }
                let name = view
                    .lists
                    .friends
                    .iter()
                    .find(|user| user.id == id)
                    .map(|user| (user.display_name.clone(), user.avatar.clone()));
                let before = match &presence {
                    Some(presence) => view.presence.insert(id.clone(), presence.clone()),
                    None => view.presence.remove(&id),
                };
                (name, before)
            };
            emit(app);
            if let (Some((name, avatar)), Some(now)) = (name, presence) {
                match (before, now.game) {
                    (None, Some(game)) => alert(
                        app,
                        Alert::Game,
                        &id,
                        &name,
                        &format!("{name} is playing {game}"),
                        avatar,
                    ),
                    (None, None) => alert(
                        app,
                        Alert::Online,
                        &id,
                        &name,
                        &format!("{name} is online"),
                        avatar,
                    ),
                    (Some(before), Some(game)) if before.game.as_deref() != Some(game.as_str()) => {
                        alert(
                            app,
                            Alert::Game,
                            &id,
                            &name,
                            &format!("{name} started {game}"),
                            avatar,
                        )
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
        "status": config.status,
        "busy": config.busy,
    })
    .to_string()
}

// --- For sending clips ---------------------------------------------------------

/// A friend's presence, if they are online.
pub fn presence_of(app: &AppHandle, id: &str) -> Option<Presence> {
    app.try_state::<Friends>()?
        .view
        .lock()
        .presence
        .get(id)
        .cloned()
}

/// Friends playing this game right now, by name — for the tags on a clip saved
/// in this moment. Whether it is the same match nobody can tell from here;
/// the same game at the same time is what there is.
pub fn playing(app: &AppHandle, game: &str) -> Vec<String> {
    let Some(friends) = app.try_state::<Friends>() else {
        return Vec::new();
    };
    let view = friends.view.lock();
    view.lists
        .friends
        .iter()
        .filter(|user| {
            view.presence
                .get(&user.id)
                .and_then(|presence| presence.game.as_deref())
                .is_some_and(|theirs| theirs.eq_ignore_ascii_case(game))
        })
        .map(|user| user.display_name.clone())
        .collect()
}

/// A friend's picture (a Discord URL), for the banners about them.
pub fn friend_avatar(app: &AppHandle, id: &str) -> Option<String> {
    let friends = app.try_state::<Friends>()?;
    let view = friends.view.lock();
    view.lists
        .friends
        .iter()
        .find(|user| user.id == id)
        .and_then(|user| user.avatar.clone())
}

pub fn friend_name(app: &AppHandle, id: &str) -> Option<String> {
    let friends = app.try_state::<Friends>()?;
    let view = friends.view.lock();
    view.lists
        .friends
        .iter()
        .find(|user| user.id == id)
        .map(|user| user.display_name.clone())
}

/// Hands a friend's app a message through the server. False when there is no
/// connection to send it on.
pub fn send_relay(app: &AppHandle, to: &str, body: &impl Serialize) -> bool {
    let Some(friends) = app.try_state::<Friends>() else {
        return false;
    };
    let Some(outbox) = friends.outbox.lock().clone() else {
        return false;
    };
    let text = serde_json::json!({ "t": "relay", "to": to, "body": body }).to_string();
    outbox.send(text).is_ok()
}

fn send_presence(app: &AppHandle, kind: &str) {
    let Some(friends) = app.try_state::<Friends>() else {
        return;
    };
    let Some(outbox) = friends.outbox.lock().clone() else {
        return;
    };
    let _ = outbox.send(presence_message(app, kind));
}

// --- Notices -------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Alert {
    Request,
    /// One of ours was answered — nothing left to do, so no hint to the console.
    Accepted,
    Online,
    Game,
    /// A clip between friends, either way ([`clip_alert`]).
    Clip,
}

/// Where a notice about a friend goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// Nowhere: not wanted, do not disturb, over one's own game — or a clip,
    /// which the window in front shows as a card or in the friend's row already.
    Nowhere,
    /// A toast in the main window, which is in front.
    InApp,
    /// Our own banner, in game and on the desktop alike.
    Banner,
    /// Windows' notification: banners are switched off altogether.
    Windows,
}

/// What the moment adds to the settings when a notice comes.
#[derive(Debug, Clone, Copy, Default)]
struct Moment {
    /// The main window is visible and has the focus.
    in_front: bool,
    /// One's own game runs ([`set_game`]).
    in_game: bool,
    /// The banner over the game is switched on at all.
    banners: bool,
}

impl Moment {
    fn now(app: &AppHandle, config: &crate::model::AppConfig) -> Self {
        Moment {
            in_front: app.get_webview_window("main").is_some_and(|window| {
                window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false)
            }),
            in_game: app
                .try_state::<Friends>()
                .is_some_and(|friends| friends.game.lock().is_some()),
            banners: config.overlay.enabled,
        }
    }

    /// "Friend banners while I'm playing" says "Clips only", and one is playing.
    fn hushed(self, config: &crate::model::FriendsConfig) -> bool {
        self.in_game && !config.notify_while_playing
    }
}

/// Where a notice of `kind` goes — about a friend who is a favourite or not.
fn route(
    config: &crate::model::FriendsConfig,
    kind: Alert,
    favorite: bool,
    moment: Moment,
) -> Route {
    let wanted = match kind {
        Alert::Request | Alert::Accepted => config.notify_requests,
        Alert::Online => config.notify_online,
        Alert::Game => config.notify_games && (favorite || !config.notify_games_favorites_only),
        Alert::Clip => config.notify_clips,
    };
    if !wanted || config.busy {
        return Route::Nowhere;
    }
    if moment.in_front {
        // An offer, a clip that came or went: the window shows it as a card,
        // or in the friend's row. A toast on top said it once more.
        return if kind == Alert::Clip {
            Route::Nowhere
        } else {
            Route::InApp
        };
    }
    if moment.hushed(config) {
        // "Clips only": a clip keeps its banner over the game — an offer
        // waits for an answer, and the clip banners have a switch of their
        // own. Windows' notification never: it steals the focus in some games.
        return if kind == Alert::Clip && moment.banners {
            Route::Banner
        } else {
            Route::Nowhere
        };
    }
    if moment.banners {
        Route::Banner
    } else {
        Route::Windows
    }
}

/// In the app while its window is in front; as our banner while it is not —
/// but never over one's own game unless that is wanted. `id` is the friend's,
/// `who` their name in `text`, for streamer mode to hide.
fn alert(app: &AppHandle, kind: Alert, id: &str, who: &str, text: &str, avatar: Option<String>) {
    let config = app.state::<AppState>().config_snapshot();
    let favorite = config.friends.favorites.iter().any(|fav| fav == id);
    let moment = Moment::now(app, &config);
    let route = route(&config.friends, kind, favorite, moment);
    if route == Route::Nowhere {
        return;
    }
    if kind == Alert::Online && !moment.hushed(&config.friends) {
        crate::sounds::play(app, crate::model::SoundKind::FriendOnline);
    }
    match route {
        Route::Nowhere => {}
        Route::InApp => crate::notify(app, "ok", text),
        Route::Windows => windows_notice(app, who, text),
        // Our own banner, in game and on the desktop alike — not Windows'. A
        // toast steals focus in some games, sits in the wrong corner in all of
        // them, and from a build without installer it even says "Windows
        // PowerShell".
        Route::Banner => {
            use crate::overlay::BannerKind;
            let (kind, detail) = match kind {
                Alert::Online => (BannerKind::FriendOnline, None),
                Alert::Game => (BannerKind::FriendGame, None),
                // Unbound, the console cannot be reached over the game either.
                Alert::Request => (
                    BannerKind::Friend,
                    (!config.console_hotkey_shown().is_empty())
                        .then(|| format!("{} to answer", config.console_hotkey_spoken())),
                ),
                Alert::Accepted => (BannerKind::Friend, None),
                // Clips come through `clip_alert`.
                Alert::Clip => return,
            };
            crate::overlay::show_friend(app, kind, who, text.to_owned(), detail, None, avatar);
        }
    }
}

/// Windows' own notification — the only way left to hear of something with
/// the banners switched off altogether.
fn windows_notice(app: &AppHandle, who: &str, text: &str) {
    use tauri_plugin_notification::NotificationExt;
    let text = if crate::streamer::active() {
        crate::streamer::mask_name(text, who)
    } else {
        text.to_owned()
    };
    if let Err(err) = app
        .notification()
        .builder()
        .title("ClippiBoy")
        .body(text)
        .show()
    {
        log::info!("friends: notification failed: {err}");
    }
}

/// What a notice about a clip between friends is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipEvent {
    /// A friend wants to send one.
    Offer,
    /// One from a friend is in.
    Arrived,
    /// One of ours reached them.
    Delivered,
    /// One of ours did not: a no, no answer, or a failure.
    Missed,
}

impl ClipEvent {
    /// Coming in, and so worth folding into one banner while playing
    /// ([`BUNDLE_WINDOW`]). What one sent oneself is answered one by one.
    fn bundles(self) -> bool {
        matches!(self, ClipEvent::Offer | ClipEvent::Arrived)
    }
}

/// A notice about a clip going to or coming from a friend (`share.rs`).
#[derive(Debug, Clone)]
pub struct ClipNotice {
    pub event: ClipEvent,
    /// The transfer it is about.
    pub id: String,
    /// The friend's id.
    pub friend: String,
    /// Their name as it stands in `title` and `detail`, for streamer mode.
    pub who: String,
    pub title: String,
    pub detail: Option<String>,
    /// A frame of a clip that arrived — it stands where the face would.
    pub thumb: Option<String>,
    pub avatar: Option<String>,
    /// What Windows' notification says when the banners are off altogether.
    pub toast: String,
    /// "clip", "screenshot" or "recording", for the line of a bundle.
    pub what: &'static str,
}

/// How long clip banners from friends wait while a game is in front, so that
/// several in a row become one ("3 clips arrived") instead of a banner each.
const BUNDLE_WINDOW: Duration = Duration::from_millis(2500);

/// A clip offered, arrived, delivered or missed: a banner, a Windows
/// notification with the banners off — or nothing, when the window in front
/// shows it already or the settings say so.
pub fn clip_alert(app: &AppHandle, notice: ClipNotice) {
    let config = app.state::<AppState>().config_snapshot();
    match route(
        &config.friends,
        Alert::Clip,
        false,
        Moment::now(app, &config),
    ) {
        Route::Nowhere | Route::InApp => {}
        Route::Windows => windows_notice(app, &notice.who, &notice.toast),
        Route::Banner => {
            let Some(friends) = app.try_state::<Friends>() else {
                return;
            };
            if !(notice.event.bundles() && friends.playing.load(Ordering::SeqCst)) {
                show_clip(app, notice);
                return;
            }
            let first = {
                let mut bundle = friends.bundle.lock();
                gather(&mut bundle, notice);
                bundle.len() == 1
            };
            if first {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(BUNDLE_WINDOW).await;
                    flush_bundle(&app);
                });
            }
        }
    }
}

/// An offer was answered or called off before its banner went up: it stays
/// out of the bundle.
pub fn clip_settled(app: &AppHandle, id: &str) {
    if let Some(friends) = app.try_state::<Friends>() {
        friends
            .bundle
            .lock()
            .retain(|notice| !(notice.event == ClipEvent::Offer && notice.id == id));
    }
}

/// Puts a notice into the bundle; one about the same transfer — its offer,
/// when it has arrived meanwhile — makes way for it.
fn gather(bundle: &mut Vec<ClipNotice>, notice: ClipNotice) {
    bundle.retain(|waiting| waiting.id != notice.id);
    bundle.push(notice);
}

fn show_clip(app: &AppHandle, notice: ClipNotice) {
    crate::overlay::show_friend(
        app,
        crate::overlay::BannerKind::Friend,
        &notice.who,
        notice.title,
        notice.detail,
        notice.thumb,
        notice.avatar,
    );
}

fn flush_bundle(app: &AppHandle) {
    let Some(friends) = app.try_state::<Friends>() else {
        return;
    };
    let mut gathered = std::mem::take(&mut *friends.bundle.lock());
    if gathered.len() <= 1 {
        if let Some(one) = gathered.pop() {
            show_clip(app, one);
        }
        return;
    }
    let hotkey = app
        .state::<AppState>()
        .config_snapshot()
        .console_hotkey_spoken();
    let one = bundled(&gathered, &hotkey);
    let avatar = one.friend.as_deref().and_then(|id| friend_avatar(app, id));
    crate::overlay::show_friend(
        app,
        crate::overlay::BannerKind::Friend,
        &one.who,
        one.title,
        one.detail,
        None,
        avatar,
    );
}

/// The one banner for several clip notices.
#[derive(Debug, PartialEq)]
struct Bundle {
    title: String,
    detail: Option<String>,
    /// The friend's name when they all came from one, for streamer mode.
    who: String,
    /// That friend's id, for their picture.
    friend: Option<String>,
}

/// "3 clips arrived", "2 clips waiting for you", "3 clips from Luca".
fn bundled(notices: &[ClipNotice], hotkey: &str) -> Bundle {
    let Some(first) = notices.first() else {
        return Bundle {
            title: String::new(),
            detail: None,
            who: String::new(),
            friend: None,
        };
    };
    let count = notices.len();
    let thing = if notices.iter().all(|notice| notice.what == first.what) {
        first.what
    } else {
        "clip"
    };
    let things = if count == 1 {
        thing.to_owned()
    } else {
        format!("{thing}s")
    };
    let mut friends: Vec<&str> = notices.iter().map(|n| n.friend.as_str()).collect();
    friends.sort_unstable();
    friends.dedup();
    let one_friend = friends.len() == 1;
    let from = if one_friend {
        first.who.clone()
    } else {
        format!("{} friends", friends.len())
    };
    let offers = notices
        .iter()
        .filter(|notice| notice.event == ClipEvent::Offer)
        .count();
    let (title, from_said) = if offers == 0 {
        (format!("{count} {things} arrived"), false)
    } else if offers == count {
        (format!("{count} {things} waiting for you"), false)
    } else if one_friend {
        (format!("{count} {things} from {from}"), true)
    } else {
        (format!("{count} {things} from friends"), true)
    };
    let mut detail = Vec::new();
    if !from_said {
        detail.push(format!("From {from}"));
    }
    if offers > 0 && !hotkey.is_empty() {
        detail.push(format!("{hotkey} to answer"));
    }
    Bundle {
        title,
        detail: (!detail.is_empty()).then(|| detail.join(" · ")),
        who: if one_friend {
            first.who.clone()
        } else {
            String::new()
        },
        friend: one_friend.then(|| first.friend.clone()),
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
pub(crate) fn install_crypto() {
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
        builder = builder
            .header("Content-Type", "application/json")
            .body(body.to_string());
    }
    let response = builder
        .send()
        .await
        .map_err(|_| "The friends server is not reachable right now.".to_string())?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        sign_out_if_current(app, &token);
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

/// The friend code in `clippiboy://add?code=K7QXM2PD` (the dash and case as
/// people type them are fine), or None — the server's alphabet, eight long.
fn add_code_from_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("clippiboy://add")?;
    if !(rest.is_empty() || rest.starts_with('/') || rest.starts_with('?')) {
        return None;
    }
    let raw = rest
        .trim_start_matches('/')
        .trim_start_matches('?')
        .split('&')
        .find_map(|pair| pair.strip_prefix("code="))?;
    let code: String = raw
        .chars()
        .filter(|c| *c != '-' && *c != ' ')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    const ALPHABET: &str = "ABCDEFGHJKMNPQRSTUVWXYZ23456789";
    (code.len() == 8 && code.chars().all(|c| ALPHABET.contains(c))).then_some(code)
}

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
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
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

/// The session as `friends.json` keeps it — sealed with DPAPI since 1.0.1
/// (`secret.rs`); a plain one from before is sealed on the first read.
#[derive(Serialize, Deserialize)]
struct StoredSession {
    token: String,
}

fn load_token() -> Option<String> {
    crate::secret::read::<StoredSession>(&token_path(), "friends session")
        .map(|stored| stored.token)
}

fn save_token(token: Option<&str>) {
    let path = token_path();
    let result = match token {
        Some(token) => crate::secret::write_sealed(
            &path,
            &StoredSession {
                token: token.to_owned(),
            },
            "friends session",
        ),
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

    /// App layer #4: an answer to the old session leaves the new one alone.
    #[test]
    fn only_the_current_session_is_signed_out() {
        assert!(same_session(Some("new"), "new"));
        assert!(!same_session(Some("new"), "old"));
        assert!(!same_session(None, "old"), "signed out already");
    }

    #[test]
    fn an_add_link_carries_a_friend_code() {
        let code = Some("K7QXM2PD".to_owned());
        assert_eq!(add_code_from_url("clippiboy://add?code=K7QXM2PD"), code);
        assert_eq!(add_code_from_url("clippiboy://add/?code=k7qx-m2pd"), code);
        assert_eq!(add_code_from_url("clippiboy://add?x=1&code=K7QXM2PD"), code);
        // Not a code: too short, look-alikes, junk, no code at all, another link.
        assert_eq!(add_code_from_url("clippiboy://add?code=K7QXM2P"), None);
        assert_eq!(add_code_from_url("clippiboy://add?code=K7QXM2P0"), None);
        assert_eq!(add_code_from_url("clippiboy://add?code=<script>"), None);
        assert_eq!(add_code_from_url("clippiboy://add"), None);
        assert_eq!(
            add_code_from_url("clippiboy://addfriend?code=K7QXM2PD"),
            None
        );
        assert_eq!(add_code_from_url("clippiboy://auth?code=K7QXM2PD"), None);
    }

    #[test]
    fn the_backoff_doubles_to_a_minute_with_the_wait_in_the_upper_half() {
        let second = Duration::from_secs(1);
        // The step doubles and stops at the cap, whatever the jitter.
        let mut step = second;
        for want in [2, 4, 8, 16, 32, 60, 60] {
            step = next_backoff(step, 0.3).1;
            assert_eq!(step, Duration::from_secs(want));
        }
        // The wait spans half the step to the full step, never more.
        assert_eq!(
            next_backoff(Duration::from_secs(8), 0.0).0,
            Duration::from_secs(4)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(8), 0.5).0,
            Duration::from_secs(6)
        );
        assert!(next_backoff(Duration::from_secs(8), 0.999).0 < Duration::from_secs(8));
        // Out of range clamps instead of waiting past the step or not at all.
        assert_eq!(next_backoff(BACKOFF_MAX, 7.0).0, BACKOFF_MAX);
        assert_eq!(next_backoff(BACKOFF_MAX, -1.0).0, BACKOFF_MAX / 2);
        assert_eq!(
            next_backoff(BACKOFF_MAX, f64::NAN),
            (BACKOFF_MAX, BACKOFF_MAX)
        );
    }

    #[test]
    fn the_jitter_stays_in_range() {
        for _ in 0..1000 {
            let unit = random_unit();
            assert!((0.0..=1.0).contains(&unit), "{unit}");
        }
    }

    #[test]
    fn a_request_that_comes_back_is_announced_once_a_day() {
        let (mut seen, day) = (HashMap::new(), 24 * 3600 * 1000);
        assert!(first_today(&mut seen, "u", 0));
        assert!(!first_today(&mut seen, "u", day - 1));
        assert!(first_today(&mut seen, "u", day));
    }

    #[test]
    fn a_second_sign_in_reuses_the_waiting_verifier() {
        let mut slot = None;
        let first = pending(&mut slot);
        assert_eq!(pending(&mut slot), first);
        assert_eq!(slot.as_deref(), Some(first.as_str()));
    }

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

    const DESKTOP: Moment = Moment {
        in_front: false,
        in_game: false,
        banners: true,
    };

    #[test]
    fn a_clip_has_a_banner_switch_of_its_own() {
        let mut config = crate::model::FriendsConfig::default();
        assert_eq!(route(&config, Alert::Clip, false, DESKTOP), Route::Banner);
        config.notify_clips = false;
        assert_eq!(route(&config, Alert::Clip, false, DESKTOP), Route::Nowhere);
        // Without banners at all, not even Windows' notification.
        let bare = Moment {
            banners: false,
            ..DESKTOP
        };
        assert_eq!(route(&config, Alert::Clip, false, bare), Route::Nowhere);
        config.notify_clips = true;
        assert_eq!(route(&config, Alert::Clip, false, bare), Route::Windows);
        // Do not disturb holds for clips too.
        config.busy = true;
        assert_eq!(route(&config, Alert::Clip, false, DESKTOP), Route::Nowhere);
    }

    #[test]
    fn friend_banners_while_playing_can_be_kept_to_clips() {
        let config = crate::model::FriendsConfig {
            notify_online: true,
            notify_while_playing: false,
            ..Default::default()
        };
        let in_game = Moment {
            in_game: true,
            ..DESKTOP
        };
        for kind in [Alert::Request, Alert::Accepted, Alert::Online, Alert::Game] {
            assert_eq!(
                route(&config, kind, true, in_game),
                Route::Nowhere,
                "{kind:?}"
            );
            assert_eq!(
                route(&config, kind, true, DESKTOP),
                Route::Banner,
                "{kind:?}"
            );
        }
        // An offer waits for an answer: its banner comes over the game too,
        // unless the clip banners are off.
        assert_eq!(route(&config, Alert::Clip, false, in_game), Route::Banner);
        let no_clips = crate::model::FriendsConfig {
            notify_clips: false,
            ..config.clone()
        };
        assert_eq!(
            route(&no_clips, Alert::Clip, false, in_game),
            Route::Nowhere
        );
        // Never Windows' notification over the game: it steals the focus.
        let bare = Moment {
            banners: false,
            ..in_game
        };
        assert_eq!(route(&config, Alert::Clip, false, bare), Route::Nowhere);
        let all = crate::model::FriendsConfig {
            notify_while_playing: true,
            ..config
        };
        assert_eq!(route(&all, Alert::Game, false, in_game), Route::Banner);
    }

    #[test]
    fn with_the_window_in_front_a_clip_needs_no_toast() {
        let config = crate::model::FriendsConfig::default();
        let front = Moment {
            in_front: true,
            ..DESKTOP
        };
        // Its card or row says it already.
        assert_eq!(route(&config, Alert::Clip, false, front), Route::Nowhere);
        // A request has no card on every page: the toast stays.
        assert_eq!(route(&config, Alert::Request, false, front), Route::InApp);
    }

    #[test]
    fn game_notices_can_be_kept_for_favourites() {
        let mut config = crate::model::FriendsConfig::default();
        assert_eq!(route(&config, Alert::Game, false, DESKTOP), Route::Banner);
        config.notify_games_favorites_only = true;
        assert_eq!(route(&config, Alert::Game, false, DESKTOP), Route::Nowhere);
        assert_eq!(route(&config, Alert::Game, true, DESKTOP), Route::Banner);
        // Only the game notice — coming online is its own switch.
        config.notify_online = true;
        assert_eq!(route(&config, Alert::Online, false, DESKTOP), Route::Banner);
    }

    fn notice(event: ClipEvent, id: &str, friend: &str, what: &'static str) -> ClipNotice {
        let who = friend.to_uppercase();
        ClipNotice {
            event,
            id: id.into(),
            friend: friend.into(),
            who: who.clone(),
            title: format!("{who} …"),
            detail: None,
            thumb: None,
            avatar: None,
            toast: String::new(),
            what,
        }
    }

    #[test]
    fn clips_from_friends_while_playing_go_up_as_one_banner() {
        let arrived = [
            notice(ClipEvent::Arrived, "1", "luca", "clip"),
            notice(ClipEvent::Arrived, "2", "luca", "clip"),
            notice(ClipEvent::Arrived, "3", "matti", "clip"),
        ];
        let one = bundled(&arrived, "Alt+C");
        assert_eq!(one.title, "3 clips arrived");
        assert_eq!(one.detail.as_deref(), Some("From 2 friends"));
        assert_eq!((one.who.as_str(), one.friend), ("", None));

        let offers = [
            notice(ClipEvent::Offer, "1", "luca", "screenshot"),
            notice(ClipEvent::Offer, "2", "matti", "screenshot"),
        ];
        let one = bundled(&offers, "Alt+C");
        assert_eq!(one.title, "2 screenshots waiting for you");
        assert_eq!(
            one.detail.as_deref(),
            Some("From 2 friends · Alt+C to answer")
        );
        // Unbound, the console is not named.
        assert_eq!(
            bundled(&offers, "").detail.as_deref(),
            Some("From 2 friends")
        );

        let mixed = [
            notice(ClipEvent::Arrived, "1", "luca", "clip"),
            notice(ClipEvent::Offer, "2", "luca", "recording"),
        ];
        let one = bundled(&mixed, "Alt+C");
        assert_eq!(one.title, "2 clips from LUCA");
        assert_eq!(one.detail.as_deref(), Some("Alt+C to answer"));
        // One friend: their name, for streamer mode, and their picture.
        assert_eq!(one.who, "LUCA");
        assert_eq!(one.friend.as_deref(), Some("luca"));
    }

    #[test]
    fn a_clip_that_arrives_takes_the_place_of_its_offer() {
        let mut bundle = Vec::new();
        gather(&mut bundle, notice(ClipEvent::Offer, "1", "luca", "clip"));
        gather(&mut bundle, notice(ClipEvent::Offer, "2", "matti", "clip"));
        gather(&mut bundle, notice(ClipEvent::Arrived, "1", "luca", "clip"));
        let events: Vec<_> = bundle.iter().map(|n| (n.id.as_str(), n.event)).collect();
        assert_eq!(events, [("2", ClipEvent::Offer), ("1", ClipEvent::Arrived)]);
        // Only incoming clips wait; one's own sends are answered one by one.
        assert!(ClipEvent::Offer.bundles() && ClipEvent::Arrived.bundles());
        assert!(!ClipEvent::Delivered.bundles() && !ClipEvent::Missed.bundles());
    }
}
