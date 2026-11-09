//! "Post to <server>": clips into a Discord channel through an incoming webhook.
//!
//! Connecting goes through the browser, like signing in: the app sends the
//! friends server a key of its own (`/webhook/begin`, with the session) and
//! opens the start page it gets back. Discord asks there which server and
//! channel, and its answer — a webhook for that channel — comes back sealed
//! with the key in `clippiboy://webhook?data=…` (`server/src/webhook.ts`). The
//! server never keeps the webhook, and a program that grabs the link gets
//! ciphertext. The key lives only here, in memory, until the link comes.
//!
//! The webhook is a credential: whoever has its URL can post into the channel.
//! It lives in `webhook.json` in ClippiBoy's data folder, never goes to a window
//! and never into the log — the windows see the server's name only.
//!
//! Posting takes the same Discord copy as "Copy for Discord", sized for a free
//! account: a webhook's limit is the server's, not the user's Nitro. When even
//! that copy does not fit, a share link goes instead, if the user can make one.
//! One post at a time, queued, and Discord's rate limits are waited out.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

use crate::state::AppState;

const DISCORD_WEBHOOKS: &str = "https://discord.com/api/webhooks";
/// What a webhook takes per file on a server without boosts.
const FILE_LIMIT: u64 = 20 * 1024 * 1024;
/// The copy aims a little under it, as "Copy for Discord" on a free account.
const TARGET_MB: u64 = 19;
/// As long as the server keeps the start (`WEBHOOK_TTL_MS`).
const KEY_TTL: Duration = Duration::from_secs(10 * 60);
/// Binds a sealed webhook to this purpose — the server's `SEAL_LABEL`.
const SEAL_LABEL: &[u8] = b"clippiboy-webhook:v1";
/// How often a post asked to wait by Discord is tried again.
const RATE_TRIES: u32 = 5;
/// A wait longer than this is not sat out: the post fails and says so.
const LONGEST_WAIT: Duration = Duration::from_secs(120);
/// Discord's limit for a message's text.
const CONTENT_MAX: usize = 2000;
/// Automatic posts wait until no game has run for this long: the Discord copy
/// is a second encode beside the buffer's, and a game is no place for it.
const AFTER_GAME: Duration = Duration::from_secs(30);
/// How often the queue looks whether the game has ended.
const GAME_CHECK: Duration = Duration::from_secs(5);

/// The webhook as it is kept, and as the server seals it.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Hook {
    id: String,
    token: String,
    guild_id: String,
    channel_id: String,
    /// The server's name, for "Post to <server>".
    guild: String,
}

impl Hook {
    fn url(&self) -> String {
        format!("{DISCORD_WEBHOOKS}/{}/{}", self.id, self.token)
    }

    /// Only the shapes Discord hands out: id and token go into a URL path.
    fn valid(&self) -> bool {
        let snowflake = |s: &str| !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit());
        snowflake(&self.id)
            && snowflake(&self.guild_id)
            && snowflake(&self.channel_id)
            && !self.token.is_empty()
            && self.token.len() <= 200
            && self.token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }
}

// Not Debug on purpose: the token must not end up in a log line by accident.

/// What the windows see.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookView {
    pub connected: bool,
    /// The server's name.
    pub guild: Option<String>,
    /// The browser is open and the app waits for Discord's answer.
    pub connecting: bool,
    /// Clips waiting to be posted or going up now, in order.
    pub posting: Vec<String>,
}

/// The connection waiting for its link: the key, since when, and the server's
/// state for it (to call it off there too).
static PENDING: Mutex<Option<([u8; 32], Instant, String)>> = Mutex::new(None);

/// A clip waiting to be posted, or going up now.
#[derive(Debug, Clone, PartialEq)]
struct Job {
    id: String,
    /// Saved and posted by the setting, not asked for: waits for the game to end.
    auto: bool,
}

/// Clips waiting or going up, in order — one entry per clip.
static WAITING: Mutex<VecDeque<Job>> = Mutex::new(VecDeque::new());
/// Wakes the queue when a job comes in.
fn wake() -> &'static Notify {
    static WAKE: OnceLock<Notify> = OnceLock::new();
    WAKE.get_or_init(Notify::new)
}
/// The queue runs once started.
static WORKER: OnceLock<()> = OnceLock::new();
/// "Will post after your game" is said once a session.
static SAID_AFTER_GAME: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// `webhook.json` is read and written under this.
static FILE: Mutex<()> = Mutex::new(());

// --- Stored ---------------------------------------------------------------------

fn path() -> PathBuf {
    crate::config::data_dir().join("webhook.json")
}

fn load() -> Option<Hook> {
    let _guard = FILE.lock();
    let text = std::fs::read(path()).ok()?;
    serde_json::from_slice::<Hook>(&text).ok().filter(Hook::valid)
}

fn store(hook: Option<&Hook>) -> Result<(), String> {
    let _guard = FILE.lock();
    match hook {
        Some(hook) => {
            std::fs::create_dir_all(crate::config::data_dir()).map_err(|err| err.to_string())?;
            let json = serde_json::to_vec(hook).map_err(|err| err.to_string())?;
            std::fs::write(path(), json).map_err(|err| format!("Could not keep the channel: {err}"))
        }
        None => match std::fs::remove_file(path()) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.to_string()),
            _ => Ok(()),
        },
    }
}

/// Forget the stored webhook only if it is still `hook`: an answer about an
/// old one, after a disconnect and a new connect, must not take the new one.
fn forget_if(hook: &Hook) -> bool {
    let _guard = FILE.lock();
    let stored = std::fs::read(path())
        .ok()
        .and_then(|text| serde_json::from_slice::<Hook>(&text).ok());
    if stored.as_ref() != Some(hook) {
        return false;
    }
    std::fs::remove_file(path()).is_ok()
}

fn view() -> WebhookView {
    let hook = load();
    let connecting = PENDING.lock().as_ref().is_some_and(|(_, at, _)| at.elapsed() < KEY_TTL);
    WebhookView {
        connected: hook.is_some(),
        guild: hook.map(|hook| hook.guild),
        connecting,
        posting: WAITING.lock().iter().map(|job| job.id.clone()).collect(),
    }
}

fn emit(app: &AppHandle) {
    let _ = app.emit("webhook-changed", view());
}

// --- Commands -------------------------------------------------------------------

#[tauri::command]
pub fn webhook_state() -> WebhookView {
    view()
}

/// "Connect channel": a key for the server to seal the webhook with, then the
/// browser on Discord's server-and-channel dialog.
#[tauri::command]
pub async fn webhook_connect(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let Some(token) = crate::friends::token(&app) else {
        return Err("Sign in on the Friends page first.".into());
    };
    let key = random_key()?;
    let response = client()
        .post(format!("{}/webhook/begin", crate::friends::SERVER))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .body(serde_json::json!({ "key": base64url(&key) }).to_string())
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|_| "The ClippiBoy server is not reachable right now.".to_string())?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        crate::friends::session_rejected(&app);
        return Err("Sign in with Discord again first.".into());
    }
    let value: serde_json::Value = serde_json::from_str(&response.text().await.unwrap_or_default()).unwrap_or_default();
    if !status.is_success() {
        return Err(value
            .get("error")
            .and_then(|e| e.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("The ClippiBoy server answered {status}.")));
    }
    let prefix = format!("{}/", crate::friends::SERVER);
    let Some(start) = value.get("url").and_then(|u| u.as_str()).filter(|u| u.starts_with(&prefix)) else {
        return Err("The ClippiBoy server gave no start page.".into());
    };
    let state = value.get("state").and_then(|s| s.as_str()).unwrap_or_default().to_owned();
    *PENDING.lock() = Some((key, Instant::now(), state));
    emit(&app);
    app.opener()
        .open_url(start, None::<&str>)
        .map_err(|err| format!("Could not open the browser: {err}"))
}

/// Called off here and on the server, whose start then refuses the browser tab.
#[tauri::command]
pub async fn webhook_cancel(app: AppHandle) {
    let pending = PENDING.lock().take();
    emit(&app);
    let (Some((_, _, state)), Some(token)) = (pending, crate::friends::token(&app)) else { return };
    if state.is_empty() {
        return;
    }
    let _ = client()
        .post(format!("{}/webhook/cancel", crate::friends::SERVER))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .body(serde_json::json!({ "state": state }).to_string())
        .timeout(Duration::from_secs(20))
        .send()
        .await;
}

/// Forget the channel here, and delete the webhook in Discord as well — with
/// its own token, no account needed. Should Discord be out of reach, it is
/// forgotten anyway; whoever manages the Discord server can remove it under
/// Integrations.
#[tauri::command]
pub async fn webhook_disconnect(app: AppHandle) -> Result<(), String> {
    let hook = load();
    store(None)?;
    WAITING.lock().clear();
    emit(&app);
    if let Some(hook) = hook {
        match client().delete(hook.url()).timeout(Duration::from_secs(20)).send().await {
            Ok(response) => log::info!("webhook: disconnected, Discord answered {}", response.status()),
            Err(_) => log::info!("webhook: disconnected, Discord not reachable to delete it"),
        }
    }
    Ok(())
}

/// "Post to <server>" on a clip or screenshot: queued behind any post going up.
#[tauri::command]
pub fn webhook_post(app: AppHandle, id: String) -> Result<(), String> {
    let Some(hook) = load() else {
        return Err("No Discord channel is connected.".into());
    };
    if !enqueue(&app, &id, false) {
        return Err(format!("This one is already on its way to {}.", hook.guild));
    }
    Ok(())
}

// --- From the rest of the core ----------------------------------------------------

/// `clippiboy://webhook?data=…`. True when the link was one of these, used or not.
pub fn handle_url(app: &AppHandle, url: &str) -> bool {
    let Some(rest) = url.strip_prefix("clippiboy://webhook") else { return false };
    if !(rest.is_empty() || rest.starts_with('/') || rest.starts_with('?')) {
        return false;
    }
    let data = rest
        .trim_start_matches('/')
        .trim_start_matches('?')
        .split('&')
        .find_map(|pair| pair.strip_prefix("data="))
        .unwrap_or_default();
    // A link nobody asked for — an old tab, a pasted URL — changes nothing.
    let key = match *PENDING.lock() {
        Some((key, at, _)) if at.elapsed() < KEY_TTL => key,
        _ => {
            log::info!("webhook: ignored a channel link, no connection is waiting");
            return true;
        }
    };
    // One that does not open leaves the waiting key: the right link may still come.
    let Some(hook) = unseal(&key, data).filter(Hook::valid) else {
        log::warn!("webhook: a channel link did not open with the waiting key");
        crate::notify(app, "error", "Connecting the channel failed. Try again in Settings.");
        return true;
    };
    *PENDING.lock() = None;
    match store(Some(&hook)) {
        Ok(()) => {
            log::info!("webhook: connected a channel");
            crate::notify(app, "ok", format!("Clips can go to {} now", hook.guild));
        }
        Err(err) => crate::notify(app, "error", err),
    }
    emit(app);
    crate::tray::show_main_window(app);
    true
}

/// A clip or screenshot was just saved: posted on its own when the setting says
/// so. Not recordings — an hour does not go into a chat channel.
pub fn clip_saved(app: &AppHandle, clip: &crate::model::Clip) {
    if clip.recording || !app.state::<AppState>().config_snapshot().webhook_auto_post {
        return;
    }
    let Some(hook) = load() else { return };
    if enqueue(app, &clip.id, true)
        && game_running(app)
        && !SAID_AFTER_GAME.swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        crate::notify(app, "ok", format!("Will post to {} after your game", hook.guild));
    }
}

/// A clip went to Recently deleted: it is not posted any more.
pub fn clip_deleted(app: &AppHandle, id: &str) {
    let removed = {
        let mut waiting = WAITING.lock();
        let before = waiting.len();
        waiting.retain(|job| job.id != id);
        waiting.len() != before
    };
    if removed {
        emit(app);
    }
}

/// Is the clip waiting to be posted, or going up? The clean-up by age leaves it.
pub fn posting(id: &str) -> bool {
    WAITING.lock().iter().any(|job| job.id == id)
}

fn game_running(app: &AppHandle) -> bool {
    app.state::<AppState>().current_game.lock().is_some()
}

/// False when the clip is in the queue already. Asked for by hand, a clip
/// waiting as an automatic post goes now instead.
fn enqueue(app: &AppHandle, id: &str, auto: bool) -> bool {
    {
        let mut waiting = WAITING.lock();
        match waiting.iter_mut().find(|job| job.id == id) {
            Some(job) if job.auto && !auto => job.auto = false,
            Some(_) => return false,
            None => waiting.push_back(Job { id: id.to_owned(), auto }),
        }
    }
    WORKER.get_or_init(|| {
        tauri::async_runtime::spawn(work(app.clone()));
    });
    wake().notify_one();
    emit(app);
    true
}

/// The next clip to post: one asked for by hand first, an automatic one only
/// while no game runs.
fn next_job(waiting: &VecDeque<Job>, game_quiet: bool) -> Option<String> {
    waiting
        .iter()
        .find(|job| !job.auto)
        .or_else(|| waiting.iter().find(|_| game_quiet))
        .map(|job| job.id.clone())
}

/// Has no game run for [`AFTER_GAME`]? `last_game` is when one was last seen.
fn game_quiet(last_game: Option<Instant>, now: Instant) -> bool {
    last_game.is_none_or(|at| now.saturating_duration_since(at) >= AFTER_GAME)
}

/// One post at a time.
async fn work(app: AppHandle) {
    let mut last_game: Option<Instant> = None;
    loop {
        if game_running(&app) {
            last_game = Some(Instant::now());
        }
        let next = next_job(&WAITING.lock(), game_quiet(last_game, Instant::now()));
        let Some(id) = next else {
            // A new job wakes it; otherwise it looks again whether the game has ended.
            let _ = tokio::time::timeout(GAME_CHECK, wake().notified()).await;
            continue;
        };
        let outcome = match load() {
            Some(hook) => Some((post(&app, &hook, &id).await, hook)),
            None => None,
        };
        WAITING.lock().retain(|job| job.id != id);
        emit(&app);
        match outcome {
            Some((Ok(Posted::File), hook)) => crate::notify(&app, "ok", format!("Posted to {}", hook.guild)),
            Some((Ok(Posted::Link), hook)) => {
                crate::notify(&app, "ok", format!("Too large for a file — posted a share link to {}", hook.guild))
            }
            Some((Err(Failed::Quiet(why)), _)) => log::info!("webhook: clip {id} not posted: {why}"),
            Some((Err(Failed::Said(why)), hook)) => {
                log::warn!("webhook: clip {id} not posted: {why}");
                crate::notify(&app, "error", format!("Could not post to {}: {why}", hook.guild));
            }
            // Only if it is still the one connected: after a disconnect and a
            // new connect, this answer is about the old one.
            Some((Err(Failed::Gone), hook)) if forget_if(&hook) => {
                log::warn!("webhook: Discord no longer knows the webhook — forgotten");
                WAITING.lock().clear();
                emit(&app);
                crate::notify(
                    &app,
                    "error",
                    "The channel's webhook was removed in Discord. Connect the channel again in Settings.",
                );
            }
            Some((Err(Failed::Gone), _)) => log::info!("webhook: an old webhook was gone; the connected one stays"),
            None => {}
        }
    }
}

enum Posted {
    File,
    Link,
}

enum Failed {
    /// Nothing to tell: the clip is gone.
    Quiet(String),
    /// A notice says it.
    Said(String),
    /// Discord no longer knows the webhook.
    Gone,
}

/// The clip as the library has it now — none once it went to Recently deleted.
fn clip_now(app: &AppHandle, id: &str) -> Option<crate::model::Clip> {
    let state = app.state::<AppState>();
    let guard = state.library.lock();
    guard.as_ref().and_then(|library| library.get(id).ok().flatten())
}

async fn post(app: &AppHandle, hook: &Hook, id: &str) -> Result<Posted, Failed> {
    let gone = || Failed::Quiet("the clip is gone".into());
    let clip = clip_now(app, id).ok_or_else(gone)?;
    let content = message(clip.title.as_deref(), clip.game.as_deref());

    // The Discord copy, as "Copy for Discord" makes it — encoding is blocking work.
    let copy = {
        let (app, id, still) = (app.clone(), id.to_owned(), clip.screenshot);
        tauri::async_runtime::spawn_blocking(move || {
            let state = app.state::<AppState>();
            if still {
                crate::commands::shot_discord_copy_of(&state, &id, TARGET_MB)
            } else {
                crate::commands::discord_copy_of(&state, &app, &id, TARGET_MB, false)
            }
        })
        .await
        .unwrap_or_else(|err| Err(err.to_string()))
    };
    // Deleted while the copy was made: not posted.
    if !posting(id) || clip_now(app, id).is_none() {
        return Err(gone());
    }
    // Only a copy that is too large falls back to a link; any other failure
    // is said as it is.
    let path = copy.map_err(Failed::Said)?;
    let len = std::fs::metadata(&path)
        .map_err(|err| Failed::Said(format!("Could not read the copy: {err}")))?
        .len();
    let why_not_a_file = if len <= FILE_LIMIT {
        let name = upload_name(Path::new(&clip.path), &path);
        match send_file(hook, &content, &name, &path).await {
            Ok(()) => return Ok(Posted::File),
            Err(Sent::TooLarge) => "Discord turned the file away as too large".to_string(),
            Err(Sent::Gone) => return Err(Failed::Gone),
            Err(Sent::Failed(why)) => return Err(Failed::Said(why)),
        }
    } else {
        "it does not fit in 20 MB".to_string()
    };

    // Too large: a share link, where one can be made.
    if clip.screenshot {
        return Err(Failed::Said(why_not_a_file));
    }
    if crate::friends::token(app).is_none() {
        return Err(Failed::Said(format!(
            "{} — sign in on the Friends page to post a share link instead.",
            capitalized(&why_not_a_file)
        )));
    }
    log::info!("webhook: clip {id} goes as a share link: {why_not_a_file}");
    let link = crate::links::link_for(app, id).await.map_err(Failed::Said)?;
    if !posting(id) || clip_now(app, id).is_none() {
        return Err(gone());
    }
    let text = if content.is_empty() { link.url.clone() } else { format!("{content}\n{}", link.url) };
    match send(hook, "application/json", payload_json(&text).into_bytes()).await {
        Ok(()) => Ok(Posted::Link),
        Err(Sent::Gone) => Err(Failed::Gone),
        Err(Sent::TooLarge) => Err(Failed::Said("Discord turned the message away.".into())),
        Err(Sent::Failed(why)) => Err(Failed::Said(why)),
    }
}

enum Sent {
    TooLarge,
    Gone,
    Failed(String),
}

async fn send_file(hook: &Hook, content: &str, name: &str, path: &Path) -> Result<(), Sent> {
    let file = tokio::fs::read(path)
        .await
        .map_err(|err| Sent::Failed(format!("Could not read the copy: {err}")))?;
    let boundary = format!("clippiboy-{}", uuid::Uuid::new_v4().simple());
    let body = multipart(&boundary, &payload_json(content), name, content_type(path), &file);
    send(hook, &format!("multipart/form-data; boundary={boundary}"), body).await
}

/// One message to the webhook, Discord's rate limits waited out.
async fn send(hook: &Hook, content_type: &str, body: Vec<u8>) -> Result<(), Sent> {
    let url = format!("{}?wait=true", hook.url());
    for _ in 0..RATE_TRIES {
        let response = client()
            .post(&url)
            .header("Content-Type", content_type)
            .body(body.clone())
            .send()
            .await
            .map_err(|_| Sent::Failed("Discord is not reachable right now.".into()))?;
        let status = response.status().as_u16();
        let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let (retry_header, remaining, reset_after) =
            (header("retry-after"), header("x-ratelimit-remaining"), header("x-ratelimit-reset-after"));
        let text = response.text().await.unwrap_or_default();
        match answer(status, retry_header.as_deref(), &text) {
            Answer::Done => {
                // The bucket is empty: the next post waits here rather than be refused.
                if let Some(pause) = pause_after(remaining.as_deref(), reset_after.as_deref()) {
                    tokio::time::sleep(pause).await;
                }
                return Ok(());
            }
            Answer::Wait(wait) if wait <= LONGEST_WAIT => tokio::time::sleep(wait).await,
            Answer::Wait(_) => return Err(Sent::Failed("Discord asks to wait — try again in a few minutes.".into())),
            Answer::TooLarge => return Err(Sent::TooLarge),
            Answer::Gone => return Err(Sent::Gone),
            Answer::Failed(why) => return Err(Sent::Failed(why)),
        }
    }
    Err(Sent::Failed("Discord kept asking to wait — try again in a few minutes.".into()))
}

/// Its own client: the file can take minutes on a slow upload line.
fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        crate::friends::install_crypto();
        reqwest::Client::builder()
            .user_agent(concat!("ClippiBoy/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(15 * 60))
            .build()
            .expect("HTTP client")
    })
}

fn random_key() -> Result<[u8; 32], String> {
    use ring::rand::SecureRandom;
    let mut key = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut key)
        .map_err(|_| "Could not make a key.".to_string())?;
    Ok(key)
}

// --- Pure parts -------------------------------------------------------------------

/// What Discord's answer to a post means.
#[derive(Debug, PartialEq)]
enum Answer {
    Done,
    /// 429: try again after this.
    Wait(Duration),
    TooLarge,
    /// The webhook is deleted (or its token no longer valid).
    Gone,
    Failed(String),
}

fn answer(status: u16, retry_after: Option<&str>, body: &str) -> Answer {
    let json: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    // Discord's JSON error codes: 10015 unknown webhook, 40005 request too large.
    let code = json.get("code").and_then(|c| c.as_u64());
    match status {
        200..=299 => Answer::Done,
        429 => {
            // The body's seconds are precise; the header is whole seconds.
            let seconds = json
                .get("retry_after")
                .and_then(|r| r.as_f64())
                .or_else(|| retry_after.and_then(|r| r.trim().parse::<f64>().ok()))
                .filter(|s| s.is_finite() && *s >= 0.0)
                .unwrap_or(1.0);
            // A hair over, so the bucket is surely open again.
            Answer::Wait(Duration::from_secs_f64(seconds.min(3600.0)) + Duration::from_millis(100))
        }
        413 => Answer::TooLarge,
        400 if code == Some(40005) => Answer::TooLarge,
        401 | 404 => Answer::Gone,
        _ if code == Some(10015) => Answer::Gone,
        _ => Answer::Failed(match json.get("message").and_then(|m| m.as_str()) {
            Some(message) => format!("Discord said: {message}"),
            None => format!("Discord answered {status}."),
        }),
    }
}

/// After a post that left the bucket empty: how long until the next may go.
fn pause_after(remaining: Option<&str>, reset_after: Option<&str>) -> Option<Duration> {
    if remaining?.trim() != "0" {
        return None;
    }
    let seconds = reset_after?.trim().parse::<f64>().ok().filter(|s| s.is_finite() && *s > 0.0)?;
    Some(Duration::from_secs_f64(seconds.min(LONGEST_WAIT.as_secs_f64())))
}

/// The text above the file: the clip's title in bold and the game, both as
/// plain text — Discord's markdown in them is escaped, and no mention pings
/// (`payload_json`).
fn message(title: Option<&str>, game: Option<&str>) -> String {
    let clean = |text: Option<&str>| text.map(|t| escape_markdown(t.trim())).filter(|t| !t.is_empty());
    let text = match (clean(title), clean(game)) {
        (Some(title), Some(game)) => format!("**{title}** · {game}"),
        (Some(title), None) => format!("**{title}**"),
        (None, Some(game)) => game,
        (None, None) => String::new(),
    };
    text.chars().take(CONTENT_MAX - 200).collect()
}

fn escape_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            out.push(' ');
            continue;
        }
        if matches!(c, '\\' | '*' | '_' | '~' | '`' | '|' | '>' | '#' | '[' | ']' | '<' | '@') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The message's JSON: the text, and nobody pinged, whatever it says.
fn payload_json(content: &str) -> String {
    serde_json::json!({ "content": content, "allowed_mentions": { "parse": [] } }).to_string()
}

/// A `multipart/form-data` body: `payload_json`, then the file as `files[0]`.
fn multipart(boundary: &str, payload: &str, file_name: &str, content_type: &str, file: &[u8]) -> Vec<u8> {
    let name: String = file_name
        .chars()
        .map(|c| if c == '"' || c == '\\' || c.is_control() { '_' } else { c })
        .collect();
    let mut body = Vec::with_capacity(file.len() + payload.len() + 512);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"payload_json\"\r\nContent-Type: application/json\r\n\r\n{payload}\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"files[0]\"; filename=\"{name}\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        _ => "video/mp4",
    }
}

/// The name the file goes under in the channel: the clip's own, not the copy's
/// `"<stem> (19 MB)"`, with the copy's extension (a screenshot may be a JPEG).
fn upload_name(clip: &Path, copy: &Path) -> String {
    let stem = clip.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "clip".into());
    match copy.extension() {
        Some(ext) => format!("{stem}.{}", ext.to_string_lossy()),
        None => stem,
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The webhook out of the link's data, if it opens with `key`: base64url of a
/// 12-byte IV and AES-256-GCM ciphertext with its tag (`seal` on the server).
fn unseal(key: &[u8; 32], data: &str) -> Option<Hook> {
    let bytes = from_base64url(data)?;
    if bytes.len() < NONCE_LEN + 16 {
        return None;
    }
    let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).ok()?);
    let nonce = Nonce::try_assume_unique_for_key(&bytes[..NONCE_LEN]).ok()?;
    let mut sealed = bytes[NONCE_LEN..].to_vec();
    let plain = key.open_in_place(nonce, Aad::from(SEAL_LABEL), &mut sealed).ok()?;
    serde_json::from_slice(plain).ok()
}

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// Unpadded base64url, as the server writes it; None for anything else.
fn from_base64url(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        } as u32)
    };
    if text.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    for chunk in text.as_bytes().chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= value(c)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sealed by the server's `seal` (Node's WebCrypto) with the key 0, 1, … 31
    /// and the IV 100, 101, … 111.
    const SERVER_KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";
    const SERVER_SEALED: &str = "ZGVmZ2hpamtsbW5vMzm3AlvTdK0KVW7Z7lJfzXr6Nju5XMtElPOAao_Mzi36y3riJ1nfYNCsAB3RYK26cjf8rkRoewrIyJzv3beSszq-6Tjlfgw3vl_PbzPpWYcKFnpY7qGv_P3CAvFNIXhnuEigTF90gEyHyuznji_fnUtH5D37W-1c11nAwK_VbOVPV-YicIAPOJR5yXWrgt61yw";

    fn key() -> [u8; 32] {
        from_base64url(SERVER_KEY).unwrap().try_into().unwrap()
    }

    #[test]
    fn a_webhook_sealed_by_the_server_opens_with_the_key() {
        let hook = unseal(&key(), SERVER_SEALED).expect("opens");
        assert_eq!(hook.id, "347114750880120863");
        assert_eq!(hook.token, "kKDd-_x");
        assert_eq!(hook.guild_id, "290926792226357250");
        assert_eq!(hook.channel_id, "345626669224982402");
        assert_eq!(hook.guild, "The Squad");
        assert!(hook.valid());
        assert_eq!(hook.url(), "https://discord.com/api/webhooks/347114750880120863/kKDd-_x");
    }

    #[test]
    fn another_key_or_a_changed_link_opens_nothing() {
        let mut other = key();
        other[0] ^= 1;
        assert!(unseal(&other, SERVER_SEALED).is_none());
        let mut changed = SERVER_SEALED.to_string();
        changed.replace_range(30..31, if &SERVER_SEALED[30..31] == "A" { "B" } else { "A" });
        assert!(unseal(&key(), &changed).is_none());
        assert!(unseal(&key(), "").is_none());
        assert!(unseal(&key(), "not base64!").is_none());
    }

    #[test]
    fn base64url_goes_both_ways() {
        for len in 0..40 {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(from_base64url(&base64url(&bytes)).as_deref(), Some(&bytes[..]));
        }
        assert_eq!(base64url(&key()), SERVER_KEY);
        assert!(from_base64url("abc=").is_none());
        assert!(from_base64url("a").is_none());
    }

    #[test]
    fn only_the_shapes_discord_hands_out_are_kept() {
        let good = unseal(&key(), SERVER_SEALED).unwrap();
        assert!(!Hook { id: "../x".into(), ..good.clone() }.valid());
        assert!(!Hook { token: "a/b".into(), ..good.clone() }.valid());
        assert!(!Hook { token: String::new(), ..good.clone() }.valid());
        assert!(!Hook { guild_id: "1".repeat(21), ..good.clone() }.valid());
        assert!(!Hook { channel_id: "x".into(), ..good }.valid());
    }

    #[test]
    fn the_multipart_body_carries_the_json_then_the_file() {
        let body = multipart("XYZ", "{\"content\":\"hi\"}", "My \"clip\".mp4", "video/mp4", b"\x00\x01data");
        let expected = b"--XYZ\r\nContent-Disposition: form-data; name=\"payload_json\"\r\nContent-Type: application/json\r\n\r\n{\"content\":\"hi\"}\r\n--XYZ\r\nContent-Disposition: form-data; name=\"files[0]\"; filename=\"My _clip_.mp4\"\r\nContent-Type: video/mp4\r\n\r\n\x00\x01data\r\n--XYZ--\r\n";
        assert_eq!(body, expected.to_vec());
        // No line break can sneak a header in through the name.
        let body = multipart("B", "{}", "a\r\nX-Evil: 1", "video/mp4", b"");
        assert!(!String::from_utf8_lossy(&body).contains("\r\nX-Evil"));
    }

    #[test]
    fn the_payload_pings_nobody() {
        let json: serde_json::Value = serde_json::from_str(&payload_json("@everyone look")).unwrap();
        assert_eq!(json["content"], "@everyone look");
        assert_eq!(json["allowed_mentions"]["parse"], serde_json::json!([]));
    }

    #[test]
    fn the_message_is_title_and_game_as_plain_text() {
        assert_eq!(message(Some("Ace"), Some("Valorant")), "**Ace** · Valorant");
        assert_eq!(message(Some("  Ace "), None), "**Ace**");
        assert_eq!(message(None, Some("CS2")), "CS2");
        assert_eq!(message(Some(" "), None), "");
        assert_eq!(message(None, None), "");
        assert_eq!(message(Some("*bold* @here"), None), "**\\*bold\\* \\@here**");
        assert_eq!(message(Some("a\nb"), None), "**a b**");
        assert!(message(Some(&"x".repeat(5000)), None).chars().count() <= CONTENT_MAX);
    }

    #[test]
    fn a_429_waits_as_long_as_discord_says() {
        let wait = |d: Duration| Answer::Wait(d + Duration::from_millis(100));
        assert_eq!(answer(429, Some("3"), r#"{"retry_after": 1.5, "global": false}"#), wait(Duration::from_millis(1500)));
        assert_eq!(answer(429, Some("3"), "not json"), wait(Duration::from_secs(3)));
        assert_eq!(answer(429, None, ""), wait(Duration::from_secs(1)));
        assert_eq!(answer(429, Some("-5"), ""), wait(Duration::from_secs(1)));
        assert_eq!(answer(429, None, r#"{"retry_after": 99999}"#), wait(Duration::from_secs(3600)));
    }

    #[test]
    fn discords_answers_are_told_apart() {
        assert_eq!(answer(200, None, "{}"), Answer::Done);
        assert_eq!(answer(204, None, ""), Answer::Done);
        assert_eq!(answer(413, None, ""), Answer::TooLarge);
        assert_eq!(answer(400, None, r#"{"code": 40005, "message": "Request entity too large"}"#), Answer::TooLarge);
        assert_eq!(answer(404, None, r#"{"code": 10015, "message": "Unknown Webhook"}"#), Answer::Gone);
        assert_eq!(answer(401, None, ""), Answer::Gone);
        assert_eq!(answer(403, None, r#"{"code": 10015}"#), Answer::Gone);
        assert_eq!(
            answer(403, None, r#"{"code": 50013, "message": "Missing Permissions"}"#),
            Answer::Failed("Discord said: Missing Permissions".into())
        );
        assert_eq!(answer(500, None, "oops"), Answer::Failed("Discord answered 500.".into()));
    }

    #[test]
    fn an_empty_bucket_holds_the_next_post() {
        assert_eq!(pause_after(Some("0"), Some("1.25")), Some(Duration::from_millis(1250)));
        assert_eq!(pause_after(Some("3"), Some("1.25")), None);
        assert_eq!(pause_after(None, Some("1.25")), None);
        assert_eq!(pause_after(Some("0"), None), None);
        assert_eq!(pause_after(Some("0"), Some("nan")), None);
        assert_eq!(pause_after(Some("0"), Some("100000")), Some(LONGEST_WAIT));
    }

    fn job(id: &str, auto: bool) -> Job {
        Job { id: id.into(), auto }
    }

    #[test]
    fn automatic_posts_wait_for_the_game_and_asked_ones_do_not() {
        let waiting: VecDeque<Job> = [job("a", true), job("b", false), job("c", true)].into();
        assert_eq!(next_job(&waiting, false).as_deref(), Some("b"));
        assert_eq!(next_job(&waiting, true).as_deref(), Some("b"));
        let autos: VecDeque<Job> = [job("a", true), job("c", true)].into();
        assert_eq!(next_job(&autos, false), None);
        assert_eq!(next_job(&autos, true).as_deref(), Some("a"));
        assert_eq!(next_job(&VecDeque::new(), true), None);
    }

    #[test]
    fn the_game_counts_as_over_half_a_minute_after_it_was_last_seen() {
        let now = Instant::now();
        assert!(game_quiet(None, now));
        assert!(!game_quiet(Some(now), now));
        assert!(!game_quiet(Some(now), now + AFTER_GAME - Duration::from_secs(1)));
        assert!(game_quiet(Some(now), now + AFTER_GAME));
    }

    #[test]
    fn the_file_goes_under_the_clips_own_name() {
        assert_eq!(
            upload_name(Path::new(r"C:\Clips\Valorant 2026-11-09.mp4"), Path::new(r"C:\data\discord\x\Valorant 2026-11-09 (19 MB).mp4")),
            "Valorant 2026-11-09.mp4"
        );
        assert_eq!(upload_name(Path::new("shot.png"), Path::new("shot (19 MB).jpg")), "shot.jpg");
        assert_eq!(content_type(Path::new("a.JPG")), "image/jpeg");
        assert_eq!(content_type(Path::new("a.png")), "image/png");
        assert_eq!(content_type(Path::new("a.mp4")), "video/mp4");
    }
}
