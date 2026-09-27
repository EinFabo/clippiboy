//! Sending a clip to a friend, straight from one PC to the other.
//!
//! Like Quick Share: both have to be online, the sender offers, the receiver
//! says yes, and the file goes across. Nothing is uploaded anywhere — the
//! friends server only carries the few hundred bytes of the handshake
//! (`friends::send_relay`), and the file itself travels over an iroh
//! connection: QUIC, end-to-end encrypted, direct wherever the two routers
//! let it through and via iroh's relays where they do not.
//!
//! The protocol on top is small on purpose. The receiver dials the sender,
//! opens one stream and says which offer, the secret from the offer, and from
//! which byte on; the sender answers with the length that follows and the
//! bytes. Should the line drop, the receiver dials again and asks for the rest.
//! At the end the SHA-256 from the offer decides whether the file is whole, and
//! one byte back tells the sender it arrived.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::{presets, Connection};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointAddr};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};

use crate::model::{AcceptClips, Clip, ClipEdit, ClipKind, TrackMix};
use crate::state::AppState;

const ALPN: &[u8] = b"clippiboy/share/1";
/// An offer nobody answers is gone after this — like a call that rings out.
const ANSWER_WITHIN: Duration = Duration::from_secs(60);
/// Dialling again after a dropped line; a clip that cannot get through in
/// this many tries will not in the next.
const ATTEMPTS: u32 = 6;
const CHUNK: usize = 256 * 1024;
/// Progress reaches the page at most this often.
const EMIT_EVERY: Duration = Duration::from_millis(250);

// --- What the page sees ----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    Out,
    In,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    /// Hashing the file and getting the connection ready.
    Preparing,
    /// Out: the friend has been asked. In: waiting for the user's answer.
    Asking,
    /// Bytes are moving.
    Moving,
    Done,
    Declined,
    /// Nobody answered in time.
    Expired,
    Cancelled,
    Failed,
}

impl Stage {
    fn over(self) -> bool {
        matches!(self, Stage::Done | Stage::Declined | Stage::Expired | Stage::Cancelled | Stage::Failed)
    }
}

/// One transfer as the page draws it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transfer {
    pub id: String,
    pub direction: Direction,
    pub friend_id: String,
    pub friend_name: String,
    /// What the clip is called — its title, or the file name.
    pub name: String,
    pub size: u64,
    pub moved: u64,
    pub stage: Stage,
    pub error: Option<String>,
    /// The clip in one's own library once a received one is in.
    pub clip_id: Option<String>,
    pub game: Option<String>,
    pub duration_ms: u64,
    pub screenshot: bool,
    pub recording: bool,
}

impl Transfer {
    /// What to call the thing in a sentence — a banner must not say "clip"
    /// about a screenshot or a recording.
    fn noun(&self) -> &'static str {
        noun(self.screenshot, self.recording)
    }
}

fn noun(screenshot: bool, recording: bool) -> &'static str {
    if screenshot {
        "screenshot"
    } else if recording {
        "recording"
    } else {
        "clip"
    }
}

/// "clip" → "Clip", for the start of a sentence.
fn capital(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

// --- On the wire -------------------------------------------------------------------

/// What describes the clip in an offer, so the receiver can file it like one
/// of its own.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    file_name: String,
    title: Option<String>,
    game: Option<String>,
    duration_ms: u64,
    width: u32,
    height: u32,
    #[serde(default)]
    screenshot: bool,
    #[serde(default)]
    recording: bool,
    /// The individual tracks, so the mixer works on the other side too
    /// (`stems.rs`). Cut to the clip already, so they start where it starts.
    /// Empty for a clip with a single track, and from senders before 0.7.3.
    #[serde(default)]
    tracks: Vec<TrackOffer>,
    /// The sender's levels for those tracks — the mix the clip file carries.
    #[serde(default)]
    mix: Vec<TrackMix>,
}

/// One individual track in an offer. It is fetched like the clip itself,
/// on a connection of its own, before the clip.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrackOffer {
    label: String,
    size: u64,
    sha256: String,
}

/// More would be a strange clip, or not a clip at all.
const MAX_TRACKS: usize = 16;

/// The handshake, passed through the friends server as the body of a relay.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Signal {
    #[serde(rename_all = "camelCase")]
    ShareOffer {
        id: String,
        size: u64,
        sha256: String,
        secret: String,
        addr: EndpointAddr,
        meta: Meta,
    },
    #[serde(rename_all = "camelCase")]
    ShareAnswer { id: String, accept: bool },
    #[serde(rename_all = "camelCase")]
    ShareCancel { id: String },
}

/// The first line on the stream, receiver to sender.
#[derive(Debug, Serialize, Deserialize)]
struct Ask {
    id: String,
    secret: String,
    offset: u64,
    /// Which individual track, or `None` for the clip. Left out on the wire
    /// for the clip, so a sender before 0.7.3 reads the ask it always did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    track: Option<u32>,
}

// --- State ---------------------------------------------------------------------

struct Outgoing {
    transfer: Transfer,
    path: PathBuf,
    /// The clip file's size. `transfer.size` counts the tracks as well.
    size: u64,
    secret: String,
    /// The individual tracks, cut to the clip, in the order of the offer.
    tracks: Vec<Cut>,
}

/// An individual track cut for sending — a file of its own in the temp folder.
struct Cut {
    label: String,
    path: PathBuf,
    size: u64,
    sha256: String,
}

struct Incoming {
    transfer: Transfer,
    size: u64,
    sha256: String,
    secret: String,
    addr: EndpointAddr,
    meta: Meta,
}

#[derive(Default)]
pub struct Share {
    router: tokio::sync::OnceCell<Router>,
    outgoing: Mutex<HashMap<String, Outgoing>>,
    incoming: Mutex<HashMap<String, Incoming>>,
    /// Set for a transfer the user called off, read by the loop moving it.
    cancelled: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

// --- Commands ------------------------------------------------------------------

#[tauri::command]
pub fn share_state(app: AppHandle) -> Vec<Transfer> {
    transfers(&app)
}

/// Offers a clip to a friend who is online.
#[tauri::command]
pub async fn share_send(app: AppHandle, clip_id: String, friend_id: String) -> Result<(), String> {
    let clip = {
        let state = app.state::<AppState>();
        let guard = state.library.lock();
        let library = guard.as_ref().ok_or("The clip database is not available.")?;
        library.get(&clip_id).map_err(|err| err.to_string())?.ok_or("That clip is gone.")?
    };
    let presence = crate::friends::presence_of(&app, &friend_id)
        .ok_or("They are offline — clips go across only while both of you are online.")?;
    if presence.busy {
        return Err("They are busy right now.".into());
    }
    let path = PathBuf::from(&clip.path);
    let size = std::fs::metadata(&path).map_err(|_| "The clip's file is missing.")?.len();
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "clip.mp4".into());
    let id = uuid::Uuid::new_v4().simple().to_string();
    let mut meta = Meta {
        file_name: file_name.clone(),
        title: clip.title.clone(),
        game: clip.game.clone(),
        duration_ms: clip.duration_ms,
        width: clip.width,
        height: clip.height,
        screenshot: clip.screenshot,
        recording: clip.recording,
        tracks: Vec::new(),
        mix: Vec::new(),
    };
    let transfer = Transfer {
        id: id.clone(),
        direction: Direction::Out,
        friend_name: crate::friends::friend_name(&app, &friend_id).unwrap_or_else(|| "Friend".into()),
        friend_id: friend_id.clone(),
        name: clip.title.clone().unwrap_or(file_name),
        size,
        moved: 0,
        stage: Stage::Preparing,
        error: None,
        clip_id: Some(clip.id.clone()),
        game: clip.game.clone(),
        duration_ms: clip.duration_ms,
        screenshot: clip.screenshot,
        recording: clip.recording,
    };
    let secret = uuid::Uuid::new_v4().simple().to_string();
    let share = app.state::<Share>();
    share.outgoing.lock().insert(
        id.clone(),
        Outgoing { transfer, path: path.clone(), size, secret: secret.clone(), tracks: Vec::new() },
    );
    emit(&app);

    // The individual tracks lie in coordinates of the untouched recording;
    // the clip is the stretch from `original.start_ms` on.
    let stems = if clip.screenshot { None } else { crate::stems::stored(&clip.id) };
    let start_ms = clip.original.map(|original| original.start_ms).unwrap_or(0);
    let length_ms = clip.duration_ms;
    let cut_id = id.clone();
    let prepared = async {
        let hashing = tokio::task::spawn_blocking(move || hash_file(&path, u64::MAX));
        let cutting = tokio::task::spawn_blocking(move || match stems {
            Some(stems) => cut_tracks(&cut_id, &stems, start_ms, length_ms),
            None => Vec::new(),
        });
        let router = router(&app).await?;
        let sha256 = hashing.await.map_err(|err| err.to_string())??;
        let tracks = cutting.await.unwrap_or_default();
        Ok::<_, String>((router.endpoint().addr(), sha256, tracks))
    };
    let (addr, sha256, tracks) = match prepared.await {
        Ok(ok) => ok,
        Err(err) => {
            set_out(&app, &id, |t| fail(t, &err));
            return Err(err);
        }
    };
    if out_stage(&app, &id) != Some(Stage::Preparing) {
        return Ok(()); // Called off meanwhile.
    }
    if !tracks.is_empty() {
        meta.tracks = tracks
            .iter()
            .map(|cut| TrackOffer { label: cut.label.clone(), size: cut.size, sha256: cut.sha256.clone() })
            .collect();
        meta.mix = clip.edit.as_ref().map(|edit| edit.tracks.clone()).unwrap_or_default();
        let extra: u64 = tracks.iter().map(|cut| cut.size).sum();
        if let Some(entry) = app.state::<Share>().outgoing.lock().get_mut(&id) {
            entry.transfer.size = size + extra;
            entry.tracks = tracks;
        }
        emit(&app);
    }
    let offer = Signal::ShareOffer { id: id.clone(), size, sha256, secret, addr, meta };
    if !crate::friends::send_relay(&app, &friend_id, &offer) {
        let err = "Not connected to the friends server.".to_string();
        set_out(&app, &id, |t| fail(t, &err));
        return Err(err);
    }
    set_out(&app, &id, |t| t.stage = Stage::Asking);

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(ANSWER_WITHIN).await;
        if out_stage(&app, &id) == Some(Stage::Asking) {
            set_out(&app, &id, |t| t.stage = Stage::Expired);
            crate::friends::send_relay(&app, &friend_id, &Signal::ShareCancel { id });
        }
    });
    Ok(())
}

/// Yes to an offer: dial the sender and fetch the clip.
#[tauri::command]
pub fn share_accept(app: AppHandle, id: String) -> Result<(), String> {
    let friend = {
        let share = app.state::<Share>();
        let mut incoming = share.incoming.lock();
        let entry = incoming.get_mut(&id).ok_or("That offer is gone.")?;
        if entry.transfer.stage != Stage::Asking {
            return Err("That offer is no longer open.".into());
        }
        entry.transfer.stage = Stage::Moving;
        entry.transfer.friend_id.clone()
    };
    emit(&app);
    crate::friends::send_relay(&app, &friend, &Signal::ShareAnswer { id: id.clone(), accept: true });
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match receive(&app, &id).await {
            Ok(clip) => {
                let name = in_friend_name(&app, &id);
                let what = capital(noun(clip.screenshot, clip.recording));
                set_in(&app, &id, |t| {
                    t.stage = Stage::Done;
                    t.moved = t.size;
                    t.clip_id = Some(clip.id.clone());
                });
                crate::overlay::show_friend(
                    &app,
                    crate::overlay::BannerKind::Friend,
                    format!("{what} from {name}"),
                    Some("In your clips".into()),
                    clip.thumb_path.clone(),
                    None,
                );
                let _ = app.emit("clip-saved", clip);
                crate::notify(&app, "ok", format!("{what} from {name} received"));
            }
            Err(err) if cancel_flag(&app, &id).load(Ordering::SeqCst) => {
                log::info!("share: receive of {id} called off: {err}");
            }
            Err(err) => {
                log::warn!("share: receive of {id} failed: {err}");
                set_in(&app, &id, |t| fail(t, &err));
                // Otherwise the sender stands at whatever percentage it got to.
                let friend = app.state::<Share>().incoming.lock().get(&id).map(|i| i.transfer.friend_id.clone());
                if let Some(friend) = friend {
                    crate::friends::send_relay(&app, &friend, &Signal::ShareCancel { id: id.clone() });
                }
            }
        }
    });
    Ok(())
}

#[tauri::command]
pub fn share_decline(app: AppHandle, id: String) {
    let friend = {
        let share = app.state::<Share>();
        let mut incoming = share.incoming.lock();
        let Some(entry) = incoming.get_mut(&id) else { return };
        if entry.transfer.stage != Stage::Asking {
            return;
        }
        entry.transfer.stage = Stage::Declined;
        entry.transfer.friend_id.clone()
    };
    emit(&app);
    crate::friends::send_relay(&app, &friend, &Signal::ShareAnswer { id, accept: false });
}

/// Calls a transfer off, in either direction.
#[tauri::command]
pub fn share_cancel(app: AppHandle, id: String) {
    cancel_flag(&app, &id).store(true, Ordering::SeqCst);
    let mut friend = None;
    let mut cancel = |t: &mut Transfer| {
        if !t.stage.over() {
            t.stage = Stage::Cancelled;
            friend = Some(t.friend_id.clone());
        }
    };
    set_out(&app, &id, &mut cancel);
    set_in(&app, &id, &mut cancel);
    if let Some(friend) = friend {
        crate::friends::send_relay(&app, &friend, &Signal::ShareCancel { id });
    }
}

/// Takes one finished transfer off the list — the page does it a few seconds
/// after the end, so the cards do not pile up.
#[tauri::command]
pub fn share_dismiss(app: AppHandle, id: String) {
    let share = app.state::<Share>();
    share.outgoing.lock().retain(|key, o| key != &id || !o.transfer.stage.over());
    share.incoming.lock().retain(|key, i| key != &id || !i.transfer.stage.over());
    share.cancelled.lock().remove(&id);
    emit(&app);
}

/// Clears finished transfers off the list.
#[tauri::command]
pub fn share_clear(app: AppHandle) {
    let share = app.state::<Share>();
    share.outgoing.lock().retain(|_, o| !o.transfer.stage.over());
    share.incoming.lock().retain(|_, i| !i.transfer.stage.over());
    emit(&app);
}

// --- From the friends server --------------------------------------------------------

/// A relay from a friend arrived. Only friends can send one — the server
/// checks — but what they send is still only data.
pub fn handle_relay(app: &AppHandle, from: &str, body: serde_json::Value) {
    let Ok(signal) = serde_json::from_value::<Signal>(body) else {
        log::info!("share: unknown relay from {from}");
        return;
    };
    match signal {
        Signal::ShareOffer { id, size, sha256, secret, addr, meta } => {
            offered(app, from, id, size, sha256, secret, addr, meta)
        }
        Signal::ShareAnswer { id, accept } => {
            let from_them = app
                .state::<Share>()
                .outgoing
                .lock()
                .get(&id)
                .is_some_and(|o| o.transfer.friend_id == from && o.transfer.stage == Stage::Asking);
            if !from_them {
                return;
            }
            // Accepted: the bytes start moving once they dial in.
            set_out(app, &id, |t| t.stage = if accept { Stage::Moving } else { Stage::Declined });
        }
        Signal::ShareCancel { id } => {
            let theirs = |t: &Transfer| t.friend_id == from;
            let share = app.state::<Share>();
            let ours = share.outgoing.lock().get(&id).is_some_and(|o| theirs(&o.transfer))
                || share.incoming.lock().get(&id).is_some_and(|i| theirs(&i.transfer));
            if !ours {
                return;
            }
            cancel_flag(app, &id).store(true, Ordering::SeqCst);
            let cancel = |t: &mut Transfer| {
                if !t.stage.over() {
                    t.stage = Stage::Cancelled;
                }
            };
            // Mid-transfer, their end gave up — worth saying, not hiding.
            set_out(app, &id, |t| {
                if t.stage == Stage::Moving {
                    fail(t, "It didn't get through to them.");
                } else {
                    cancel(t);
                }
            });
            set_in(app, &id, cancel);
        }
    }
}

/// The server could not pass one of our messages on.
pub fn relay_failed(app: &AppHandle, reference: Option<&str>, reason: &str) {
    let Some(id) = reference else { return };
    let message = match reason {
        "offline" => "They went offline.",
        "not a friend" => "You are no longer friends.",
        _ => "The friends server did not pass the offer on.",
    };
    set_out(app, id, |t| {
        if matches!(t.stage, Stage::Preparing | Stage::Asking) {
            fail(t, message);
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn offered(
    app: &AppHandle,
    from: &str,
    id: String,
    size: u64,
    sha256: String,
    secret: String,
    addr: EndpointAddr,
    meta: Meta,
) {
    let config = app.state::<AppState>().config_snapshot().friends;
    let welcome = match config.accept_clips {
        AcceptClips::All => true,
        AcceptClips::Favorites => config.favorites.iter().any(|fav| fav == from),
        AcceptClips::Off => false,
    };
    // Busy, or not from someone welcome: a quiet no, without anything popping up.
    if config.busy || !welcome || id.len() > 64 || sha256.len() != 64 {
        crate::friends::send_relay(app, from, &Signal::ShareAnswer { id, accept: false });
        return;
    }
    let friend_name = crate::friends::friend_name(app, from).unwrap_or_else(|| "A friend".into());
    let name = meta.title.clone().unwrap_or_else(|| meta.file_name.clone());
    let what = noun(meta.screenshot, meta.recording);
    // Tracks that do not add up are left behind; the clip still comes.
    let mut meta = meta;
    if meta.tracks.len() > MAX_TRACKS || meta.tracks.iter().any(|track| track.sha256.len() != 64) {
        meta.tracks.clear();
    }
    let total = size + meta.tracks.iter().map(|track| track.size).sum::<u64>();
    let transfer = Transfer {
        id: id.clone(),
        direction: Direction::In,
        friend_id: from.to_owned(),
        friend_name: friend_name.clone(),
        name: name.clone(),
        size: total,
        moved: 0,
        stage: Stage::Asking,
        error: None,
        clip_id: None,
        game: meta.game.clone(),
        duration_ms: meta.duration_ms,
        screenshot: meta.screenshot,
        recording: meta.recording,
    };
    app.state::<Share>().incoming.lock().insert(
        id.clone(),
        Incoming { transfer, size, sha256, secret, addr, meta },
    );
    emit(app);
    crate::friends::announce_clip(app, &format!("{friend_name} wants to send you {name} ({})", megabytes(total)));
    // Over the game too: the banner cannot be clicked, so it says where the
    // answer goes — the console has the buttons.
    let hotkey = app.state::<AppState>().config_snapshot().console_hotkey;
    crate::overlay::show_friend(
        app,
        crate::overlay::BannerKind::Friend,
        format!("{friend_name} wants to send you a {what}"),
        Some(format!("{} · {hotkey} to answer", megabytes(total))),
        None,
        crate::friends::friend_avatar(app, from),
    );

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(ANSWER_WITHIN).await;
        set_in(&app, &id, |t| {
            if t.stage == Stage::Asking {
                t.stage = Stage::Expired;
            }
        });
    });
}

// --- Receiving -----------------------------------------------------------------

async fn receive(app: &AppHandle, id: &str) -> Result<Clip, String> {
    let (size, sha256, secret, addr, meta, from) = {
        let share = app.state::<Share>();
        let incoming = share.incoming.lock();
        let entry = incoming.get(id).ok_or("That offer is gone.")?;
        (
            entry.size,
            entry.sha256.clone(),
            entry.secret.clone(),
            entry.addr.clone(),
            entry.meta.clone(),
            entry.transfer.friend_name.clone(),
        )
    };
    let config = app.state::<AppState>().config_snapshot();
    let kind = if meta.screenshot {
        ClipKind::Screenshot
    } else if meta.recording {
        ClipKind::Recording
    } else {
        ClipKind::Clip
    };
    let dir = crate::filing::dir_for(Path::new(&config.clip_dir), meta.game.as_deref(), false, kind);
    std::fs::create_dir_all(&dir).map_err(|err| format!("Could not create the folder: {err}"))?;
    let target = crate::filing::free_name(&dir, &safe_file_name(&meta.file_name));
    let part = target.with_extension(format!(
        "{}.part",
        target.extension().and_then(|ext| ext.to_str()).unwrap_or("mp4")
    ));

    let router = router(app).await?;
    let cancelled = cancel_flag(app, id);
    // The tracks first: small, and the sender counts the transfer as done
    // once the clip is in — after that it would not hand them out any more.
    let clip_id = uuid::Uuid::new_v4().to_string();
    let has_tracks = fetch_tracks(app, router.endpoint(), &addr, id, &secret, &meta.tracks, &clip_id, &cancelled).await;
    let base: u64 = meta.tracks.iter().map(|track| track.size).sum();
    let mut last_error = String::new();
    for attempt in 0..ATTEMPTS {
        if cancelled.load(Ordering::SeqCst) {
            let _ = std::fs::remove_file(&part);
            crate::stems::remove(&clip_id);
            return Err("cancelled".into());
        }
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(2u64.pow(attempt.min(4)))).await;
        }
        match fetch(app, router.endpoint(), &addr, id, &secret, None, size, base, &part, &cancelled).await {
            Ok(connection) => {
                let file = part.clone();
                let expected = sha256.clone();
                let whole = tokio::task::spawn_blocking(move || hash_file(&file, u64::MAX))
                    .await
                    .map_err(|err| err.to_string())??;
                if whole != expected {
                    let _ = std::fs::remove_file(&part);
                    crate::stems::remove(&clip_id);
                    connection.close(1u32.into(), b"bad hash");
                    return Err("The clip arrived damaged — ask them to send it again.".into());
                }
                if let Ok((mut send, _)) = connection.open_bi().await {
                    let _ = send.write_all(b"k").await;
                    let _ = send.finish();
                }
                // Closing drops whatever is still unsent — the "k" with it,
                // and the sender stood at 100 % for good. The sender hangs up
                // once it has read it; until then, wait (not forever).
                let _ = tokio::time::timeout(Duration::from_secs(5), connection.closed()).await;
                if let Err(err) = std::fs::rename(&part, &target) {
                    crate::stems::remove(&clip_id);
                    return Err(format!("Could not keep the clip: {err}"));
                }
                return file_clip(app, &clip_id, &target, &meta, size, &from, has_tracks);
            }
            Err(err) => {
                log::info!("share: attempt {} for {id}: {err}", attempt + 1);
                last_error = err;
            }
        }
    }
    let _ = std::fs::remove_file(&part);
    crate::stems::remove(&clip_id);
    Err(format!("The clip did not get through: {last_error}"))
}

/// The individual tracks into the stems store of the clip to be, each checked
/// against its hash. All or none: a mixer with a track missing would be worse
/// than the clip's own mixed audio. False when there are none to be had.
#[allow(clippy::too_many_arguments)]
async fn fetch_tracks(
    app: &AppHandle,
    endpoint: &Endpoint,
    addr: &EndpointAddr,
    id: &str,
    secret: &str,
    tracks: &[TrackOffer],
    clip_id: &str,
    cancelled: &AtomicBool,
) -> bool {
    if tracks.is_empty() {
        return false;
    }
    if let Err(err) = std::fs::create_dir_all(crate::stems::dir(clip_id)) {
        log::info!("share: no folder for the tracks: {err}");
        return false;
    }
    let mut base = 0;
    for (index, track) in tracks.iter().enumerate() {
        let target = crate::stems::track_path(clip_id, index as u32);
        let part = target.with_extension("m4a.part");
        let mut kept = false;
        for attempt in 0..ATTEMPTS {
            if cancelled.load(Ordering::SeqCst) {
                break;
            }
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(2u64.pow(attempt.min(4)))).await;
            }
            match fetch(app, endpoint, addr, id, secret, Some(index as u32), track.size, base, &part, cancelled).await {
                Ok(connection) => {
                    // Every byte is in; the sender waits for this to let go.
                    connection.close(0u32.into(), b"track");
                    let file = part.clone();
                    let whole = tokio::task::spawn_blocking(move || hash_file(&file, u64::MAX)).await;
                    kept = matches!(whole, Ok(Ok(hash)) if hash == track.sha256)
                        && std::fs::rename(&part, &target).is_ok();
                    break;
                }
                Err(err) => log::info!("share: track {index} of {id}, attempt {}: {err}", attempt + 1),
            }
        }
        if !kept {
            log::info!("share: track {index} of {id} did not arrive — the clip comes without its tracks");
            crate::stems::remove(clip_id);
            return false;
        }
        base += track.size;
    }
    let labels: Vec<String> = tracks.iter().map(|track| track.label.clone()).collect();
    if let Err(err) = crate::stems::write_index(clip_id, &labels) {
        log::info!("share: track index not written: {err}");
        crate::stems::remove(clip_id);
        return false;
    }
    true
}

/// One try: dial, ask for the rest, write it to the part file. Returns the
/// connection once every byte is in.
#[allow(clippy::too_many_arguments)]
async fn fetch(
    app: &AppHandle,
    endpoint: &Endpoint,
    addr: &EndpointAddr,
    id: &str,
    secret: &str,
    track: Option<u32>,
    size: u64,
    // Bytes of the transfer that came before this file, for the progress.
    base: u64,
    part: &Path,
    cancelled: &AtomicBool,
) -> Result<Connection, String> {
    let offset = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0).min(size);
    let connection = tokio::time::timeout(Duration::from_secs(30), endpoint.connect(addr.clone(), ALPN))
        .await
        .map_err(|_| "could not reach them".to_string())?
        .map_err(net)?;
    let (mut send, mut recv) = connection.open_bi().await.map_err(|err| err.to_string())?;
    let ask = serde_json::to_vec(&Ask { id: id.into(), secret: secret.into(), offset, track }).map_err(net)?;
    send.write_all(&(ask.len() as u32).to_be_bytes()).await.map_err(net)?;
    send.write_all(&ask).await.map_err(net)?;
    let _ = send.finish();

    let mut header = [0u8; 8];
    recv.read_exact(&mut header).await.map_err(net)?;
    let rest = u64::from_be_bytes(header);
    if offset + rest != size {
        return Err("they offered a different file".into());
    }
    let mut file = open_part(part, offset).map_err(|err| format!("Could not write the clip: {err}"))?;
    let mut moved = offset;
    let mut buf = vec![0u8; CHUNK];
    let mut last_emit = std::time::Instant::now();
    while moved < size {
        if cancelled.load(Ordering::SeqCst) {
            connection.close(2u32.into(), b"cancelled");
            return Err("cancelled".into());
        }
        let n = match recv.read(&mut buf).await.map_err(net)? {
            Some(n) => n,
            None => break,
        };
        file.write_all(&buf[..n]).map_err(|err| format!("Could not write the clip: {err}"))?;
        moved += n as u64;
        if last_emit.elapsed() >= EMIT_EVERY {
            last_emit = std::time::Instant::now();
            set_in(app, id, |t| t.moved = base + moved);
        }
    }
    file.flush().map_err(net)?;
    if moved != size {
        return Err("the line dropped".into());
    }
    Ok(connection)
}

/// Into the library, tagged with who sent it.
fn file_clip(
    app: &AppHandle,
    id: &str,
    path: &Path,
    meta: &Meta,
    size: u64,
    from: &str,
    has_tracks: bool,
) -> Result<Clip, String> {
    let id = id.to_owned();
    let thumb = if meta.screenshot {
        crate::thumbs::make_still(path, &id)
    } else {
        crate::thumbs::make(path, &id)
    };
    let clip = Clip {
        id: id.clone(),
        path: path.to_string_lossy().into_owned(),
        created_at: now_ms(),
        duration_ms: meta.duration_ms,
        game: meta.game.clone(),
        width: meta.width,
        height: meta.height,
        size_bytes: size,
        thumb_path: thumb.ok().map(|p| p.to_string_lossy().into_owned()),
        title: meta.title.clone(),
        description: None,
        favorite: false,
        // The sender's levels, so the mixer opens where the clip's own audio
        // stands. The range is the whole file, as after any saved edit.
        edit: (has_tracks && !meta.mix.is_empty()).then(|| ClipEdit {
            start_ms: 0,
            end_ms: meta.duration_ms,
            tracks: meta.mix.clone(),
        }),
        original: None,
        original_available: false,
        screenshot: meta.screenshot,
        recording: meta.recording,
        tags: vec![format!("from {from}")],
    };
    let state = app.state::<AppState>();
    let guard = state.library.lock();
    let library = guard.as_ref().ok_or("The clip database is not available.")?;
    library.insert(&clip).map_err(|err| err.to_string())?;
    library.set_tags(&clip.id, &clip.tags).map_err(|err| err.to_string())?;
    Ok(clip)
}

// --- Sending -------------------------------------------------------------------

/// The iroh side, started the first time a clip goes either way and kept for
/// the rest of the run.
async fn router(app: &AppHandle) -> Result<Router, String> {
    let share = app.state::<Share>();
    let handler = Serve { app: app.clone() };
    let router = share
        .router
        .get_or_try_init(|| async move {
            let endpoint = Endpoint::bind(presets::N0)
                .await
                .map_err(|err| format!("Could not open the connection for sending clips: {err}"))?;
            Ok::<_, String>(Router::builder(endpoint).accept(ALPN, handler).spawn())
        })
        .await?;
    // Without a relay or a public address the other side cannot find us yet.
    let _ = tokio::time::timeout(Duration::from_secs(10), router.endpoint().online()).await;
    Ok(router.clone())
}

#[derive(Clone)]
struct Serve {
    app: AppHandle,
}

impl std::fmt::Debug for Serve {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Serve")
    }
}

impl ProtocolHandler for Serve {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        serve(&self.app, &connection)
            .await
            .map_err(|err| AcceptError::from_err(std::io::Error::other(err)))
    }
}

/// A receiver dialled in: check the ask, then stream the file from its offset.
async fn serve(app: &AppHandle, connection: &Connection) -> Result<(), String> {
    let (mut send, mut recv) = connection.accept_bi().await.map_err(net)?;
    let mut length = [0u8; 4];
    recv.read_exact(&mut length).await.map_err(net)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > 4096 {
        return Err("ask too long".into());
    }
    let mut ask = vec![0u8; length];
    recv.read_exact(&mut ask).await.map_err(net)?;
    let ask: Ask = serde_json::from_slice(&ask).map_err(net)?;

    let (path, size, base) = {
        let share = app.state::<Share>();
        let mut outgoing = share.outgoing.lock();
        let entry = outgoing.get_mut(&ask.id).ok_or("unknown offer")?;
        // The secret was only ever in the offer to that one friend. Dialling
        // in is their yes — it may well beat the answer through the server.
        let open = matches!(entry.transfer.stage, Stage::Asking | Stage::Moving);
        if !same(entry.secret.as_bytes(), ask.secret.as_bytes()) || !open {
            return Err("not allowed".into());
        }
        entry.transfer.stage = Stage::Moving;
        // The tracks go first, so the clip's bytes count after all of them.
        let tracks: u64 = entry.tracks.iter().map(|cut| cut.size).sum();
        match ask.track {
            None => (entry.path.clone(), entry.size, tracks),
            Some(index) => {
                let index = index as usize;
                let cut = entry.tracks.get(index).ok_or("unknown track")?;
                let before = entry.tracks[..index].iter().map(|cut| cut.size).sum();
                (cut.path.clone(), cut.size, before)
            }
        }
    };
    emit(app);
    let offset = ask.offset.min(size);
    send.write_all(&(size - offset).to_be_bytes()).await.map_err(net)?;

    let mut file = std::fs::File::open(&path).map_err(net)?;
    file.seek(SeekFrom::Start(offset)).map_err(net)?;
    let cancelled = cancel_flag(app, &ask.id);
    let mut moved = offset;
    let mut buf = vec![0u8; CHUNK];
    let mut last_emit = std::time::Instant::now();
    loop {
        if cancelled.load(Ordering::SeqCst) {
            connection.close(2u32.into(), b"cancelled");
            return Ok(());
        }
        let n = file.read(&mut buf).map_err(net)?;
        if n == 0 {
            break;
        }
        send.write_all(&buf[..n]).await.map_err(net)?;
        moved += n as u64;
        if last_emit.elapsed() >= EMIT_EVERY {
            last_emit = std::time::Instant::now();
            set_out(app, &ask.id, |t| t.moved = base + moved);
        }
    }
    let _ = send.finish();
    set_out(app, &ask.id, |t| t.moved = base + moved);

    // A track is done once the receiver hangs up — it checks the hash on its
    // own, and a bad one simply means the clip comes without tracks.
    if ask.track.is_some() {
        let _ = tokio::time::timeout(Duration::from_secs(30), connection.closed()).await;
        return Ok(());
    }

    // The receiver checks the hash and answers on a second stream.
    let arrived = tokio::time::timeout(Duration::from_secs(120), async {
        let (_, mut recv) = connection.accept_bi().await.ok()?;
        let mut ok = [0u8; 1];
        recv.read_exact(&mut ok).await.ok()?;
        Some(ok[0] == b'k')
    })
    .await
    .ok()
    .flatten()
    .unwrap_or(false);
    if arrived {
        set_out(app, &ask.id, |t| {
            t.stage = Stage::Done;
            t.moved = t.size;
        });
        // The receiver waits for this before it lets go.
        connection.close(0u32.into(), b"done");
        let (name, friend, what) = app
            .state::<Share>()
            .outgoing
            .lock()
            .get_mut(&ask.id)
            .map(|o| {
                // The cut tracks have done their job.
                for cut in o.tracks.drain(..) {
                    let _ = std::fs::remove_file(&cut.path);
                }
                (o.transfer.friend_name.clone(), o.transfer.friend_id.clone(), o.transfer.noun())
            })
            .unwrap_or_else(|| ("Your friend".into(), String::new(), "clip"));
        let avatar = crate::friends::friend_avatar(app, &friend);
        crate::overlay::show_friend(
            app,
            crate::overlay::BannerKind::Friend,
            format!("{name} has your {what}"),
            None,
            None,
            avatar,
        );
    } else if bad_hash(connection) {
        // Every byte went out, and what arrived did not match. A line that
        // merely dropped stays open instead: they dial again for the rest.
        set_out(app, &ask.id, |t| fail(t, "It arrived damaged on their side — try sending it again."));
    }
    Ok(())
}

/// Did the receiver hang up with "bad hash" (code 1)?
fn bad_hash(connection: &Connection) -> bool {
    matches!(
        connection.close_reason(),
        Some(iroh::endpoint::ConnectionError::ApplicationClosed(close)) if close.error_code == 1u32.into()
    )
}

// --- Bits ----------------------------------------------------------------------

fn transfers(app: &AppHandle) -> Vec<Transfer> {
    let share = app.state::<Share>();
    let mut all: Vec<Transfer> = share
        .outgoing
        .lock()
        .values()
        .map(|o| o.transfer.clone())
        .chain(share.incoming.lock().values().map(|i| i.transfer.clone()))
        .collect();
    all.sort_by(|a, b| a.id.cmp(&b.id));
    all
}

fn emit(app: &AppHandle) {
    let _ = app.emit("share-state", transfers(app));
}

fn set_out(app: &AppHandle, id: &str, change: impl FnOnce(&mut Transfer)) {
    let changed = {
        let share = app.state::<Share>();
        let mut outgoing = share.outgoing.lock();
        outgoing.get_mut(id).map(|o| change(&mut o.transfer)).is_some()
    };
    if changed {
        emit(app);
    }
}

fn set_in(app: &AppHandle, id: &str, change: impl FnOnce(&mut Transfer)) {
    let changed = {
        let share = app.state::<Share>();
        let mut incoming = share.incoming.lock();
        incoming.get_mut(id).map(|i| change(&mut i.transfer)).is_some()
    };
    if changed {
        emit(app);
    }
}

fn out_stage(app: &AppHandle, id: &str) -> Option<Stage> {
    app.state::<Share>().outgoing.lock().get(id).map(|o| o.transfer.stage)
}

fn in_friend_name(app: &AppHandle, id: &str) -> String {
    app.state::<Share>()
        .incoming
        .lock()
        .get(id)
        .map(|i| i.transfer.friend_name.clone())
        .unwrap_or_else(|| "a friend".into())
}

fn cancel_flag(app: &AppHandle, id: &str) -> Arc<AtomicBool> {
    app.state::<Share>().cancelled.lock().entry(id.to_owned()).or_default().clone()
}

fn fail(transfer: &mut Transfer, error: &str) {
    if !transfer.stage.over() {
        transfer.stage = Stage::Failed;
        transfer.error = Some(error.to_owned());
    }
}

/// SHA-256 of the first `limit` bytes, as hex.
/// The individual tracks cut to the clip, into the temp folder. Best effort:
/// whatever goes wrong, the clip still goes — only without its tracks.
fn cut_tracks(id: &str, stems: &[crate::model::ClipTrack], start_ms: u64, length_ms: u64) -> Vec<Cut> {
    let dir = std::env::temp_dir().join("clippiboy-share").join(crate::muxer::sanitize(id));
    let cut = || -> Result<Vec<Cut>, String> {
        if stems.len() > MAX_TRACKS {
            return Err(format!("{} tracks", stems.len()));
        }
        std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
        let seconds = |ms: u64| format!("{:.3}", ms as f64 / 1000.0);
        let mut cuts = Vec::with_capacity(stems.len());
        let mut sorted: Vec<_> = stems.iter().collect();
        sorted.sort_by_key(|track| track.index);
        for (slot, track) in sorted.into_iter().enumerate() {
            let source = track.preview_path.as_deref().ok_or("track without a file")?;
            let path = dir.join(format!("{slot}.m4a"));
            // Copying AAC cuts on a frame, some 20 ms — nothing an ear hears.
            let mut command = crate::muxer::ffmpeg();
            command
                .args(["-y", "-hide_banner", "-loglevel", "error", "-ss", &seconds(start_ms), "-i"])
                .arg(source)
                .args(["-t", &seconds(length_ms), "-map", "0:a:0", "-c", "copy", "-movflags", "+faststart"])
                .arg(&path);
            crate::muxer::run(&mut command, "Spur zum Senden schneiden")?;
            let size = std::fs::metadata(&path).map_err(|err| err.to_string())?.len();
            let sha256 = hash_file(&path, u64::MAX)?;
            cuts.push(Cut { label: track.label.clone(), path, size, sha256 });
        }
        Ok(cuts)
    };
    match cut() {
        Ok(cuts) => cuts,
        Err(err) => {
            log::info!("share: tracks not sent along: {err}");
            let _ = std::fs::remove_dir_all(&dir);
            Vec::new()
        }
    }
}

fn hash_file(path: &Path, limit: u64) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|err| format!("Could not read the clip: {err}"))?;
    let mut reader = std::io::BufReader::with_capacity(CHUNK, file.take(limit));
    let mut hasher = Sha256::new();
    std::io::copy(&mut reader, &mut hasher).map_err(|err| format!("Could not read the clip: {err}"))?;
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The part file, cut back to `offset` and positioned there.
///
/// Not `append`: on Windows that grants only FILE_APPEND_DATA, and cutting the
/// file then fails with "access denied" — which dropped every connection
/// before its first byte.
fn open_part(part: &Path, offset: u64) -> std::io::Result<std::fs::File> {
    let mut file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(part)?;
    file.set_len(offset)?;
    file.seek(SeekFrom::Start(offset))?;
    Ok(file)
}

/// A network error with its cause — "connection lost" alone says nothing.
fn net(err: impl std::fmt::Debug) -> String {
    format!("{err:?}")
}

/// Only a plain name with a known ending — never a path from the other side.
fn safe_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_owned();
    let (stem, ext) = match cleaned.rsplit_once('.') {
        Some((stem, ext)) => (stem.to_owned(), ext.to_ascii_lowercase()),
        None => (cleaned.clone(), String::new()),
    };
    let ext = match ext.as_str() {
        "mp4" | "mkv" | "mov" | "webm" | "png" | "jpg" | "jpeg" => ext,
        _ => "mp4".into(),
    };
    let stem: String = stem.chars().take(120).collect();
    let stem = if stem.trim().is_empty() { "clip".to_owned() } else { stem };
    format!("{stem}.{ext}")
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn megabytes(size: u64) -> String {
    let mb = size as f64 / 1_048_576.0;
    if mb < 10.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{mb:.0} MB")
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_from_a_friend_stay_names() {
        assert_eq!(safe_file_name("..\\..\\Windows\\evil.exe"), "evil.mp4");
        assert_eq!(safe_file_name("Ace on Bind.MP4"), "Ace on Bind.mp4");
        assert_eq!(safe_file_name("shot.png"), "shot.png");
        assert_eq!(safe_file_name("..."), "clip.mp4");
        assert_eq!(safe_file_name("a:b?.mkv"), "a_b_.mkv");
    }

    #[test]
    fn signals_round_trip() {
        let text = r#"{"kind":"shareAnswer","id":"o1","accept":true}"#;
        match serde_json::from_str::<Signal>(text).unwrap() {
            Signal::ShareAnswer { id, accept } => assert!(id == "o1" && accept),
            _ => panic!("wrong kind"),
        }
        let cancel = serde_json::to_value(Signal::ShareCancel { id: "x".into() }).unwrap();
        assert_eq!(cancel["kind"], "shareCancel");
    }

    #[test]
    fn hashing_matches_sha256() {
        let dir = std::env::temp_dir().join(format!("cb-share-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.bin");
        std::fs::write(&file, b"abc").unwrap();
        assert_eq!(
            hash_file(&file, u64::MAX).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_part_file_resumes_where_it_was_cut() {
        let dir = std::env::temp_dir().join(format!("cb-share-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("a.part");
        std::fs::write(&part, b"abcdef").unwrap();
        // Four bytes are trusted, the rest of the last try is not.
        let mut file = open_part(&part, 4).unwrap();
        file.write_all(b"XY").unwrap();
        drop(file);
        assert_eq!(std::fs::read(&part).unwrap(), b"abcdXY");
        let mut fresh = open_part(&dir.join("b.part"), 0).unwrap();
        fresh.write_all(b"new").unwrap();
        drop(fresh);
        assert_eq!(std::fs::read(dir.join("b.part")).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stages_that_are_over() {
        assert!(Stage::Done.over() && Stage::Failed.over() && Stage::Expired.over());
        assert!(!Stage::Asking.over() && !Stage::Moving.over());
    }

    #[test]
    fn an_ask_for_the_clip_reads_as_before_0_7_3() {
        let ask = Ask { id: "a".into(), secret: "s".into(), offset: 7, track: None };
        let text = serde_json::to_string(&ask).unwrap();
        assert!(!text.contains("track"), "{text}");
        let old: Ask = serde_json::from_str(r#"{"id":"a","secret":"s","offset":0}"#).unwrap();
        assert_eq!(old.track, None);
    }

    #[test]
    fn an_offer_from_before_0_7_3_has_no_tracks() {
        let meta: Meta = serde_json::from_str(
            r#"{"fileName":"a.mp4","title":null,"game":null,"durationMs":1000,"width":1920,"height":1080}"#,
        )
        .unwrap();
        assert!(meta.tracks.is_empty() && meta.mix.is_empty());
    }
}
