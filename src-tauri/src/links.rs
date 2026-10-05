//! Share links: a clip on clippiboy.com/c/<id> for five days.
//!
//! The opposite of `share.rs` on purpose — that one goes from PC to PC and keeps
//! nothing anywhere; this one leaves a copy on the friends server's R2 bucket,
//! so whoever has the link can watch it in a browser or right in Discord.
//!
//! The server holds the rules (three a week, 50 MB, the bucket's ceiling, see
//! `server/src/shares.ts`). The app asks first, so a clip is not shrunk only to
//! be turned away, then shrinks it if it has to and streams it up.
//!
//! The links themselves are remembered here, in `links.json`: the menu offers
//! "Copy link" instead of a second upload, and "Delete link".

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

/// The server takes 50 MB. A little under, because a size-targeted encode
/// lands near its target rather than exactly on it.
const MAX_BYTES: u64 = 50 * 1024 * 1024;
const TARGET_BYTES: u64 = 46 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub id: String,
    pub url: String,
    /// ms since the epoch.
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    pub used: u32,
    pub limit: u32,
    pub resets_at: Option<i64>,
    pub full: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkState {
    pub signed_in: bool,
    /// `None` when the server could not be asked — offline, say.
    pub quota: Option<Quota>,
    /// By clip id, only the ones still alive.
    pub links: HashMap<String, Link>,
}

/// One clip on its way up. `stage` is `shrinking`, `uploading`, `done` or
/// `failed`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkProgress {
    pub clip_id: String,
    pub title: String,
    pub stage: &'static str,
    pub progress: f32,
    pub error: Option<String>,
}

/// Sends `link-progress` for one clip. Owned, so the upload stream can carry
/// a copy along.
#[derive(Clone)]
struct Reporter {
    app: AppHandle,
    clip_id: String,
    title: String,
}

impl Reporter {
    fn emit(&self, stage: &'static str, progress: f32, error: Option<String>) {
        let _ = self.app.emit(
            "link-progress",
            LinkProgress {
                clip_id: self.clip_id.clone(),
                title: self.title.clone(),
                stage,
                progress,
                error,
            },
        );
    }
}

// --- The list on disk -------------------------------------------------------------

static FILE: Mutex<()> = Mutex::new(());

fn path() -> PathBuf {
    crate::config::data_dir().join("links.json")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// The live links, by clip id. Run-out ones drop off on the way.
fn load() -> HashMap<String, Link> {
    let _guard = FILE.lock();
    let mut links: HashMap<String, Link> = std::fs::read(path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let now = now_ms();
    links.retain(|_, link| link.expires_at > now);
    links
}

fn change(edit: impl FnOnce(&mut HashMap<String, Link>)) -> Result<(), String> {
    let mut links = load();
    edit(&mut links);
    let _guard = FILE.lock();
    let json = serde_json::to_vec_pretty(&links).map_err(|err| err.to_string())?;
    std::fs::write(path(), json).map_err(|err| format!("Could not remember the link: {err}"))
}

// --- Talking to the server ----------------------------------------------------------

/// Its own client: the friends one gives up after 20 s, and 50 MB on a slow
/// upload line take minutes.
fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        crate::friends::install_crypto();
        reqwest::Client::builder()
            .user_agent(concat!("ClippiBoy/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20 * 60))
            .build()
            .expect("HTTP client")
    })
}

fn token(app: &AppHandle) -> Result<String, String> {
    crate::friends::token(app).ok_or_else(|| "Sign in with Discord to share links.".to_string())
}

/// The body as JSON, or the server's own words for what went wrong.
async fn read(response: reqwest::Response) -> Result<serde_json::Value, String> {
    let status = response.status();
    let text = response.text().await.map_err(|err| err.to_string())?;
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    if status.is_success() {
        return Ok(value);
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("Sign in with Discord again to share links.".into());
    }
    if value.get("reason").and_then(|r| r.as_str()) == Some("quota") {
        let resets = value.get("resetsAt").and_then(|r| r.as_i64());
        return Err(quota_message(resets));
    }
    Err(value
        .get("error")
        .and_then(|error| error.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("The server answered {status}.")))
}

fn quota_message(resets_at: Option<i64>) -> String {
    match resets_at.map(|at| (at - now_ms()).max(0) / 3_600_000) {
        Some(hours) if hours >= 24 => format!(
            "All links for this week are used — the next one is free in {} days.",
            (hours + 23) / 24
        ),
        Some(hours) if hours >= 1 => {
            format!("All links for this week are used — the next one is free in {hours} hours.")
        }
        _ => "All links for this week are used — the next one is free shortly.".into(),
    }
}

async fn fetch_quota(token: &str) -> Result<Quota, String> {
    let response = client()
        .get(format!("{}/shares/quota", crate::friends::SERVER))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|_| "The ClippiBoy server is not reachable right now.".to_string())?;
    serde_json::from_value(read(response).await?).map_err(|err| err.to_string())
}

// --- Commands ---------------------------------------------------------------------

#[tauri::command]
pub async fn link_state(app: AppHandle) -> Result<LinkState, String> {
    let links = load();
    let Ok(token) = token(&app) else {
        return Ok(LinkState {
            signed_in: false,
            quota: None,
            links,
        });
    };
    Ok(LinkState {
        signed_in: true,
        quota: fetch_quota(&token).await.ok(),
        links,
    })
}

/// Upload a clip and put its link on the clipboard. An existing live link is
/// handed out again instead.
#[tauri::command]
pub async fn link_create(app: AppHandle, id: String) -> Result<Link, String> {
    if let Some(link) = load().remove(&id) {
        crate::clipboard::copy_text(&link.url)?;
        crate::notify(&app, "ok", "Link copied");
        return Ok(link);
    }
    let token = token(&app)?;
    let clip = {
        let state = app.state::<AppState>();
        let guard = state.library.lock();
        let library = guard.as_ref().ok_or("The clip database is not available.")?;
        library.get(&id).map_err(|err| err.to_string())?.ok_or("That clip is gone.")?
    };
    if clip.screenshot {
        return Err("Only clips can be shared as a link.".into());
    }
    let title = clip.title.clone().unwrap_or_else(|| {
        PathBuf::from(&clip.path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Clip".into())
    });

    let report = Reporter {
        app: app.clone(),
        clip_id: id.clone(),
        title: title.clone(),
    };
    let result = upload(&token, &clip, &title, &report).await;
    match &result {
        Ok(link) => {
            report.emit("done", 1.0, None);
            let _ = change(|links| {
                links.insert(id.clone(), link.clone());
            });
            let _ = app.emit("links-changed", ());
            // The upload card says so; a toast on top would say it twice.
            let _ = crate::clipboard::copy_text(&link.url);
        }
        Err(err) => report.emit("failed", 0.0, Some(err.clone())),
    }
    result
}

async fn upload(
    token: &str,
    clip: &crate::model::Clip,
    title: &str,
    report: &Reporter,
) -> Result<Link, String> {
    // Asked first: no point shrinking a clip for two minutes only to hear the
    // week is used up.
    let quota = fetch_quota(token).await?;
    if quota.used >= quota.limit {
        return Err(quota_message(quota.resets_at));
    }
    if quota.full {
        return Err("Sharing is full right now — try again later.".into());
    }

    let source = PathBuf::from(&clip.path);
    let size = std::fs::metadata(&source)
        .map_err(|_| "The clip file is no longer there.".to_string())?
        .len();
    // Small enough already: the original goes, in full quality.
    let (file, temporary) = if size <= MAX_BYTES {
        (source, None)
    } else {
        report.emit("shrinking", 0.0, None);
        let temp = crate::config::data_dir()
            .join("temp")
            .join(format!("link-{}.mp4", clip.id));
        let _ = std::fs::create_dir_all(temp.parent().unwrap());
        let shrinking = {
            let report = report.clone();
            let temp = temp.clone();
            tokio::task::spawn_blocking(move || {
                let state = report.app.state::<AppState>();
                let progress = |value: f32| report.emit("shrinking", value, None);
                crate::commands::shrink(&state, &report.clip_id, TARGET_BYTES, &temp, &progress)
            })
        };
        let written = shrinking.await.map_err(|err| err.to_string())??;
        if written > MAX_BYTES {
            let _ = std::fs::remove_file(&temp);
            return Err("This clip is too long to fit in 50 MB. Trim it first.".into());
        }
        (temp.clone(), Some(temp))
    };

    let result = send(token, clip, title, &file, report).await;
    if let Some(temp) = temporary {
        let _ = std::fs::remove_file(temp);
    }
    let link = result?;

    // The preview picture for Discord and the page. Without it the link still
    // works, so a failure here is only logged.
    if let Some(thumb) = clip.thumb_path.as_ref().and_then(|path| std::fs::read(path).ok()) {
        let sent = client()
            .put(format!("{}/shares/{}/poster", crate::friends::SERVER, link.id))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "image/jpeg")
            .body(thumb)
            .send()
            .await;
        if let Err(err) = sent {
            log::warn!("share link {}: no poster: {err}", link.id);
        }
    }
    Ok(link)
}

/// Stream the file up, reporting as the chunks leave.
async fn send(
    token: &str,
    clip: &crate::model::Clip,
    title: &str,
    file: &std::path::Path,
    report: &Reporter,
) -> Result<Link, String> {
    let bytes = std::fs::read(file).map_err(|err| format!("Could not read the clip: {err}"))?;
    let total = bytes.len();
    let mut params = vec![
        ("title", title.to_owned()),
        ("width", clip.width.to_string()),
        ("height", clip.height.to_string()),
    ];
    if let Some(game) = &clip.game {
        params.push(("game", game.clone()));
    }
    let url = reqwest::Url::parse_with_params(&format!("{}/shares", crate::friends::SERVER), &params)
        .map_err(|err| err.to_string())?;

    const CHUNK: usize = 256 * 1024;
    report.emit("uploading", 0.0, None);
    let chunks: Vec<Vec<u8>> = bytes.chunks(CHUNK).map(<[u8]>::to_vec).collect();
    let mut sent = 0usize;
    let stream = futures_util::stream::iter(chunks.into_iter().map(move |chunk| {
        sent += chunk.len();
        Ok::<_, std::io::Error>((chunk, sent))
    }));
    // Progress as reqwest pulls the next chunk — close enough to what has left.
    let stream = {
        use futures_util::StreamExt;
        let report = report.clone();
        stream.map(move |item| {
            item.map(|(chunk, sent)| {
                report.emit("uploading", sent as f32 / total.max(1) as f32, None);
                chunk
            })
        })
    };
    let response = client()
        .post(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "video/mp4")
        .header("Content-Length", total.to_string())
        .body(reqwest::Body::wrap_stream(stream))
        .send()
        .await
        .map_err(|_| "The upload broke off. Check your connection and try again.".to_string())?;
    serde_json::from_value(read(response).await?).map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn link_delete(app: AppHandle, id: String) -> Result<(), String> {
    let Some(link) = load().remove(&id) else {
        return Ok(());
    };
    let token = token(&app)?;
    let response = client()
        .delete(format!("{}/shares/{}", crate::friends::SERVER, link.id))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|_| "The ClippiBoy server is not reachable right now.".to_string())?;
    // Gone already counts as deleted.
    if response.status() != reqwest::StatusCode::NOT_FOUND {
        read(response).await?;
    }
    change(|links| {
        links.remove(&id);
    })?;
    let _ = app.emit("links-changed", ());
    crate::notify(&app, "ok", "Link deleted");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quota_message_counts_in_days_then_hours() {
        let in_days = quota_message(Some(now_ms() + 50 * 3_600_000));
        assert!(in_days.contains("in 3 days"), "{in_days}");
        let in_hours = quota_message(Some(now_ms() + 5 * 3_600_000 + 60_000));
        assert!(in_hours.contains("in 5 hours"), "{in_hours}");
        assert!(quota_message(None).contains("shortly"));
    }
}
