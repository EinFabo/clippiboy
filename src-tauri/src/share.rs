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

use crate::model::{AcceptClips, Clip, ClipKind};
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
}

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
}

// --- State ---------------------------------------------------------------------

struct Outgoing {
    transfer: Transfer,
    path: PathBuf,
    secret: String,
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
    let meta = Meta {
        file_name: file_name.clone(),
        title: clip.title.clone(),
        game: clip.game.clone(),
        duration_ms: clip.duration_ms,
        width: clip.width,
        height: clip.height,
        screenshot: clip.screenshot,
        recording: clip.recording,
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
    };
    let secret = uuid::Uuid::new_v4().simple().to_string();
    let share = app.state::<Share>();
    share.outgoing.lock().insert(
        id.clone(),
        Outgoing { transfer, path: path.clone(), secret: secret.clone() },
    );
    emit(&app);

    let prepared = async {
        let hashing = tokio::task::spawn_blocking(move || hash_file(&path, u64::MAX));
        let router = router(&app).await?;
        let sha256 = hashing.await.map_err(|err| err.to_string())??;
        Ok::<_, String>((router.endpoint().addr(), sha256))
    };
    let (addr, sha256) = match prepared.await {
        Ok(ok) => ok,
        Err(err) => {
            set_out(&app, &id, |t| fail(t, &err));
            return Err(err);
        }
    };
    if out_stage(&app, &id) != Some(Stage::Preparing) {
        return Ok(()); // Called off meanwhile.
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
                set_in(&app, &id, |t| {
                    t.stage = Stage::Done;
                    t.moved = t.size;
                    t.clip_id = Some(clip.id.clone());
                });
                let _ = app.emit("clip-saved", clip);
                crate::notify(&app, "ok", format!("Clip from {name} received"));
            }
            Err(err) if cancel_flag(&app, &id).load(Ordering::SeqCst) => {
                log::info!("share: receive of {id} called off: {err}");
            }
            Err(err) => {
                log::warn!("share: receive of {id} failed: {err}");
                set_in(&app, &id, |t| fail(t, &err));
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
            set_out(app, &id, cancel);
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
    let transfer = Transfer {
        id: id.clone(),
        direction: Direction::In,
        friend_id: from.to_owned(),
        friend_name: friend_name.clone(),
        name: name.clone(),
        size,
        moved: 0,
        stage: Stage::Asking,
        error: None,
        clip_id: None,
        game: meta.game.clone(),
        duration_ms: meta.duration_ms,
        screenshot: meta.screenshot,
    };
    app.state::<Share>().incoming.lock().insert(
        id.clone(),
        Incoming { transfer, size, sha256, secret, addr, meta },
    );
    emit(app);
    crate::friends::announce_clip(app, &format!("{friend_name} wants to send you {name} ({})", megabytes(size)));

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
    let mut last_error = String::new();
    for attempt in 0..ATTEMPTS {
        if cancelled.load(Ordering::SeqCst) {
            let _ = std::fs::remove_file(&part);
            return Err("cancelled".into());
        }
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(2u64.pow(attempt.min(4)))).await;
        }
        match fetch(app, router.endpoint(), &addr, id, &secret, size, &part, &cancelled).await {
            Ok(connection) => {
                let file = part.clone();
                let expected = sha256.clone();
                let whole = tokio::task::spawn_blocking(move || hash_file(&file, u64::MAX))
                    .await
                    .map_err(|err| err.to_string())??;
                if whole != expected {
                    let _ = std::fs::remove_file(&part);
                    connection.close(1u32.into(), b"bad hash");
                    return Err("The clip arrived damaged — ask them to send it again.".into());
                }
                if let Ok((mut send, _)) = connection.open_bi().await {
                    let _ = send.write_all(b"k").await;
                    let _ = send.finish();
                }
                std::fs::rename(&part, &target).map_err(|err| format!("Could not keep the clip: {err}"))?;
                connection.close(0u32.into(), b"thanks");
                return file_clip(app, &target, &meta, size, &from);
            }
            Err(err) => {
                log::info!("share: attempt {} for {id}: {err}", attempt + 1);
                last_error = err;
            }
        }
    }
    let _ = std::fs::remove_file(&part);
    Err(format!("The clip did not get through: {last_error}"))
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
    size: u64,
    part: &Path,
    cancelled: &AtomicBool,
) -> Result<Connection, String> {
    let offset = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0).min(size);
    let connection = tokio::time::timeout(Duration::from_secs(30), endpoint.connect(addr.clone(), ALPN))
        .await
        .map_err(|_| "could not reach them".to_string())?
        .map_err(|err| err.to_string())?;
    let (mut send, mut recv) = connection.open_bi().await.map_err(|err| err.to_string())?;
    let ask = serde_json::to_vec(&Ask { id: id.into(), secret: secret.into(), offset }).map_err(|e| e.to_string())?;
    send.write_all(&(ask.len() as u32).to_be_bytes()).await.map_err(|e| e.to_string())?;
    send.write_all(&ask).await.map_err(|e| e.to_string())?;
    let _ = send.finish();

    let mut header = [0u8; 8];
    recv.read_exact(&mut header).await.map_err(|e| e.to_string())?;
    let rest = u64::from_be_bytes(header);
    if offset + rest != size {
        return Err("they offered a different file".into());
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(part)
        .map_err(|err| format!("Could not write the clip: {err}"))?;
    file.set_len(offset).map_err(|e| e.to_string())?;
    let mut moved = offset;
    let mut buf = vec![0u8; CHUNK];
    let mut last_emit = std::time::Instant::now();
    while moved < size {
        if cancelled.load(Ordering::SeqCst) {
            connection.close(2u32.into(), b"cancelled");
            return Err("cancelled".into());
        }
        let n = match recv.read(&mut buf).await.map_err(|e| e.to_string())? {
            Some(n) => n,
            None => break,
        };
        file.write_all(&buf[..n]).map_err(|err| format!("Could not write the clip: {err}"))?;
        moved += n as u64;
        if last_emit.elapsed() >= EMIT_EVERY {
            last_emit = std::time::Instant::now();
            set_in(app, id, |t| t.moved = moved);
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    if moved != size {
        return Err("the line dropped".into());
    }
    Ok(connection)
}

/// Into the library, tagged with who sent it.
fn file_clip(app: &AppHandle, path: &Path, meta: &Meta, size: u64, from: &str) -> Result<Clip, String> {
    let id = uuid::Uuid::new_v4().to_string();
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
        edit: None,
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
    let (mut send, mut recv) = connection.accept_bi().await.map_err(|e| e.to_string())?;
    let mut length = [0u8; 4];
    recv.read_exact(&mut length).await.map_err(|e| e.to_string())?;
    let length = u32::from_be_bytes(length) as usize;
    if length > 4096 {
        return Err("ask too long".into());
    }
    let mut ask = vec![0u8; length];
    recv.read_exact(&mut ask).await.map_err(|e| e.to_string())?;
    let ask: Ask = serde_json::from_slice(&ask).map_err(|e| e.to_string())?;

    let (path, size) = {
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
        (entry.path.clone(), entry.transfer.size)
    };
    emit(app);
    let offset = ask.offset.min(size);
    send.write_all(&(size - offset).to_be_bytes()).await.map_err(|e| e.to_string())?;

    let mut file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    let cancelled = cancel_flag(app, &ask.id);
    let mut moved = offset;
    let mut buf = vec![0u8; CHUNK];
    let mut last_emit = std::time::Instant::now();
    loop {
        if cancelled.load(Ordering::SeqCst) {
            connection.close(2u32.into(), b"cancelled");
            return Ok(());
        }
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        send.write_all(&buf[..n]).await.map_err(|e| e.to_string())?;
        moved += n as u64;
        if last_emit.elapsed() >= EMIT_EVERY {
            last_emit = std::time::Instant::now();
            set_out(app, &ask.id, |t| t.moved = moved);
        }
    }
    let _ = send.finish();
    set_out(app, &ask.id, |t| t.moved = moved);

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
    }
    connection.closed().await;
    Ok(())
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
fn hash_file(path: &Path, limit: u64) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|err| format!("Could not read the clip: {err}"))?;
    let mut reader = std::io::BufReader::with_capacity(CHUNK, file.take(limit));
    let mut hasher = Sha256::new();
    std::io::copy(&mut reader, &mut hasher).map_err(|err| format!("Could not read the clip: {err}"))?;
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
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
    fn stages_that_are_over() {
        assert!(Stage::Done.over() && Stage::Failed.over() && Stage::Expired.over());
        assert!(!Stage::Asking.over() && !Stage::Moving.over());
    }
}
