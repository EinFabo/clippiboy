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
//!
//! Each installation keeps its iroh key (`share-key.bin`, sealed with DPAPI),
//! so its id stays the same across runs. The offer says so (`stable`) and the
//! answer names the receiver's id (`node`); `peers.rs` keeps the ids a clip
//! really went to or came from, per friend. For now a device that does not fit
//! is only logged.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::{presets, Connection};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointAddr, SecretKey};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};

use crate::model::{AcceptClips, Clip, ClipEdit, ClipKind, FriendsConfig, TrackMix};
use crate::state::AppState;

const ALPN: &[u8] = b"clippiboy/share/1";
/// An offer nobody answers is gone after this — like a call that rings out.
const ANSWER_WITHIN: Duration = Duration::from_secs(60);
/// Dialling again after a dropped line; a clip that cannot get through in
/// this many tries will not in the next.
const ATTEMPTS: u32 = 6;
/// How long a peer may take to open the stream or send the header or the ask.
const HANDSHAKE: Duration = Duration::from_secs(10);
/// How long a peer may go without sending or taking a chunk before it is given up on.
const IDLE: Duration = Duration::from_secs(30);
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
        matches!(
            self,
            Stage::Done | Stage::Declined | Stage::Expired | Stage::Cancelled | Stage::Failed
        )
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
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
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
/// Largest clip or recording an offer may announce. Recordings have no length
/// limit, and at the highest bitrate (100 Mbit/s) 64 GiB were reached after an
/// hour and a half; 2 TiB is some 44 hours of that. The limit is not what keeps
/// the disk from filling up (`receive` checks the free space), it only turns
/// sizes that cannot be right away.
const MAX_OFFER_BYTES: u64 = 2 << 40;
/// Largest single track an offer may announce.
const MAX_TRACK_BYTES: u64 = 2 << 30;
/// Left free on a drive after a download, so receiving never fills it.
const SPACE_MARGIN: u64 = 512 << 20;

/// The handshake, passed through the friends server as the body of a relay.
// The variant names are the `kind` tags on the wire, and a signal lives only
// as long as one relay message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[allow(clippy::enum_variant_names, clippy::large_enum_variant)]
enum Signal {
    #[serde(rename_all = "camelCase")]
    ShareOffer {
        id: String,
        size: u64,
        sha256: String,
        secret: String,
        addr: EndpointAddr,
        meta: Meta,
        /// `addr.id` stays the same across the sender's runs and may be kept
        /// (`peers.rs`). Missing before 1.0.1.
        #[serde(default)]
        stable: bool,
    },
    #[serde(rename_all = "camelCase")]
    ShareAnswer {
        id: String,
        accept: bool,
        /// The receiver's lasting iroh id, on a yes — the id that will dial in.
        /// A string, not an `EndpointId`: an id that does not parse must not
        /// cost the answer. Missing before 1.0.1, and for a key not kept.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<String>,
    },
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
    /// The receiver's device as their yes named it (`ShareAnswer.node`).
    answered: Option<String>,
    /// The device that took the clip, once it arrived.
    reached: Option<String>,
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
    /// `ShareOffer.stable`.
    stable: bool,
}

#[derive(Default)]
pub struct Share {
    router: tokio::sync::OnceCell<Router>,
    outgoing: Mutex<HashMap<String, Outgoing>>,
    incoming: Mutex<HashMap<String, Incoming>>,
    /// Set for a transfer the user called off, read by the loop moving it.
    cancelled: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Offers that came before the friends lists were ever loaded ([`Held`]).
    held: Mutex<Vec<Held<Offer>>>,
}

// --- Commands ------------------------------------------------------------------

#[tauri::command]
pub fn share_state(app: AppHandle) -> Vec<Transfer> {
    transfers(&app)
}

/// Offers a clip to a friend who is online.
#[tauri::command]
pub async fn share_send(app: AppHandle, clip_id: String, friend_id: String) -> Result<(), String> {
    // A cam still being added goes in first: the friend gets the clip with it.
    let waiting = clip_id.clone();
    let settled = tauri::async_runtime::spawn_blocking(move || {
        crate::composite::wait_settled(&waiting, std::time::Duration::from_secs(300))
    })
    .await
    .unwrap_or(false);
    if !settled {
        return Err("The cam is still being added to this clip — try again in a moment.".into());
    }
    let clip = {
        let state = app.state::<AppState>();
        let guard = state.library.lock();
        let library = guard
            .as_ref()
            .ok_or("The clip database is not available.")?;
        library
            .get(&clip_id)
            .map_err(|err| err.to_string())?
            .ok_or("That clip is gone.")?
    };
    let presence = crate::friends::presence_of(&app, &friend_id)
        .ok_or("They are offline — clips go across only while both of you are online.")?;
    if presence.busy {
        return Err("They are busy right now.".into());
    }
    let path = PathBuf::from(&clip.path);
    let size = std::fs::metadata(&path)
        .map_err(|_| {
            format!(
                "The {}'s file is missing.",
                noun(clip.screenshot, clip.recording)
            )
        })?
        .len();
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
        friend_name: crate::friends::friend_name(&app, &friend_id)
            .unwrap_or_else(|| "Friend".into()),
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
    {
        // Checked and entered under one lock, so two quick clicks cannot both pass.
        let share = app.state::<Share>();
        let mut outgoing = share.outgoing.lock();
        if let Some(waiting) = unanswered(outgoing.values().map(|o| &o.transfer), &friend_id) {
            return Err(format!(
                "Wait until {} has answered the last {}.",
                waiting.friend_name,
                waiting.noun()
            ));
        }
        outgoing.insert(
            id.clone(),
            Outgoing {
                transfer,
                path: path.clone(),
                size,
                secret: secret.clone(),
                tracks: Vec::new(),
                answered: None,
                reached: None,
            },
        );
    }
    emit(&app);

    // The individual tracks lie in coordinates of the untouched recording;
    // the clip is the stretch from `original.start_ms` on.
    // Tracks the user muted or turned right down stay home. A mix saved into
    // the file leaves no trace of what was muted, so a baked clip sends none.
    let baked = clip.edit.as_ref().is_some_and(|edit| edit.baked);
    let (stems, mix) = match crate::stems::stored(&clip.id) {
        Some(stems) if !clip.screenshot && !baked => {
            let levels = clip
                .edit
                .as_ref()
                .map(|edit| edit.tracks.as_slice())
                .unwrap_or(&[]);
            let (kept, mix) = audible_stems(stems, levels);
            ((!kept.is_empty()).then_some(kept), mix)
        }
        _ => (None, Vec::new()),
    };
    let start_ms = clip.original.map(|original| original.start_ms).unwrap_or(0);
    let length_ms = clip.duration_ms;
    let cut_id = id.clone();
    let copy = copy_path(&id);
    let (screenshot, what) = (clip.screenshot, noun(clip.screenshot, clip.recording));
    let prepared = async {
        let hashing =
            tokio::task::spawn_blocking(move || file_to_send(&path, &copy, screenshot, what));
        let cutting = tokio::task::spawn_blocking(move || match stems {
            Some(stems) => cut_tracks(&cut_id, &stems, start_ms, length_ms),
            None => Vec::new(),
        });
        let router = router(&app).await;
        let file = hashing
            .await
            .map_err(|err| err.to_string())
            .and_then(|file| file);
        // The cutter is always awaited, error or not: it must be done before
        // a failure clears its folder, or it would make the folder again.
        let tracks = cutting.await.unwrap_or_default();
        let router = router?;
        Ok::<_, String>((router.endpoint().addr(), file?, tracks))
    };
    let (addr, (sent, size, sha256), tracks) = match prepared.await {
        Ok(ok) => ok,
        Err(err) => {
            set_out(&app, &id, |t| fail(t, &err));
            drop_cuts(&id);
            return Err(err);
        }
    };
    if out_stage(&app, &id) != Some(Stage::Preparing) {
        drop_cuts(&id);
        return Ok(()); // Called off meanwhile.
    }
    if !tracks.is_empty() {
        meta.tracks = tracks
            .iter()
            .map(|cut| TrackOffer {
                label: cut.label.clone(),
                size: cut.size,
                sha256: cut.sha256.clone(),
            })
            .collect();
        meta.mix = mix;
    }
    // What goes may be a copy of the clip (`file_to_send`), and the tracks
    // count too.
    let extra: u64 = tracks.iter().map(|cut| cut.size).sum();
    if let Some(entry) = app.state::<Share>().outgoing.lock().get_mut(&id) {
        entry.path = sent;
        entry.size = size;
        entry.transfer.size = size + extra;
        entry.tracks = tracks;
    }
    emit(&app);
    let stable = identity(&app).stable;
    let offer = Signal::ShareOffer {
        id: id.clone(),
        size,
        sha256,
        secret,
        addr,
        meta,
        stable,
    };
    if !crate::friends::send_relay(&app, &friend_id, &offer) {
        let err = "Not connected to the friends server.".to_string();
        set_out(&app, &id, |t| fail(t, &err));
        drop_cuts(&id);
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
    crate::telemetry::used(crate::telemetry::Feature::SendFriend);
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
    let node = identity(&app).node();
    crate::friends::send_relay(
        &app,
        &friend,
        &Signal::ShareAnswer {
            id: id.clone(),
            accept: true,
            node,
        },
    );
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
                    &name,
                    format!("{what} from {name}"),
                    Some("In your clips".into()),
                    clip.thumb_path.clone(),
                    None,
                );
                let _ = app.emit("clip-saved", clip);
                crate::notify(&app, "ok", format!("{what} from {name} received"));
                crate::sounds::play(&app, crate::model::SoundKind::ClipReceived);
            }
            Err(err) if cancel_flag(&app, &id).load(Ordering::SeqCst) => {
                log::info!("share: receive of {id} called off: {err}");
            }
            Err(err) => {
                log::warn!("share: receive of {id} failed: {err}");
                set_in(&app, &id, |t| fail(t, &err));
                // Otherwise the sender stands at whatever percentage it got to.
                let friend = app
                    .state::<Share>()
                    .incoming
                    .lock()
                    .get(&id)
                    .map(|i| i.transfer.friend_id.clone());
                if let Some(friend) = friend {
                    crate::friends::send_relay(
                        &app,
                        &friend,
                        &Signal::ShareCancel { id: id.clone() },
                    );
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
        let Some(entry) = incoming.get_mut(&id) else {
            return;
        };
        if entry.transfer.stage != Stage::Asking {
            return;
        }
        entry.transfer.stage = Stage::Declined;
        entry.transfer.friend_id.clone()
    };
    emit(&app);
    crate::friends::send_relay(
        &app,
        &friend,
        &Signal::ShareAnswer {
            id,
            accept: false,
            node: None,
        },
    );
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
    share
        .outgoing
        .lock()
        .retain(|key, o| key != &id || !o.transfer.stage.over());
    share
        .incoming
        .lock()
        .retain(|key, i| key != &id || !i.transfer.stage.over());
    share.cancelled.lock().remove(&id);
    // Whatever way the transfer ended, its cut tracks have no use any more —
    // but a stray dismiss for one still running must not take them away.
    if may_drop_cuts(out_stage(&app, &id)) {
        drop_cuts(&id);
    }
    emit(&app);
}

/// Clears finished transfers off the list.
#[tauri::command]
pub fn share_clear(app: AppHandle) {
    let share = app.state::<Share>();
    let mut ended = Vec::new();
    share.outgoing.lock().retain(|key, o| {
        let over = o.transfer.stage.over();
        if over {
            ended.push(key.clone());
        }
        !over
    });
    for id in &ended {
        drop_cuts(id);
    }
    share
        .incoming
        .lock()
        .retain(|_, i| !i.transfer.stage.over());
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
        Signal::ShareOffer {
            id,
            size,
            sha256,
            secret,
            addr,
            meta,
            stable,
        } => {
            let offer = Offer {
                id,
                size,
                sha256,
                secret,
                addr,
                meta,
                stable,
            };
            offered(app, from, offer, std::time::Instant::now())
        }
        Signal::ShareAnswer { id, accept, node } => {
            let asking = {
                let share = app.state::<Share>();
                let mut outgoing = share.outgoing.lock();
                let Some(entry) = outgoing
                    .get_mut(&id)
                    .filter(|o| o.transfer.friend_id == from)
                else {
                    return;
                };
                // Kept even when their dial beat the answer here (Moving, or
                // Done already): it only names the device to keep.
                if accept && entry.answered.is_none() {
                    entry.answered = node.filter(|n| crate::peers::is_node(n));
                }
                entry.transfer.stage == Stage::Asking
            };
            settle_pin(app, &id);
            if !asking {
                return;
            }
            // Accepted: the bytes start moving once they dial in.
            set_out(app, &id, |t| {
                t.stage = if accept {
                    Stage::Moving
                } else {
                    Stage::Declined
                }
            });
        }
        Signal::ShareCancel { id } => {
            let theirs = |t: &Transfer| t.friend_id == from;
            let share = app.state::<Share>();
            let ours = share
                .outgoing
                .lock()
                .get(&id)
                .is_some_and(|o| theirs(&o.transfer))
                || share
                    .incoming
                    .lock()
                    .get(&id)
                    .is_some_and(|i| theirs(&i.transfer));
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

/// What an offer comes to with its tracks, or `None` when its sizes cannot be
/// right.
///
/// Added up with checks: the release build has no overflow checks, and a track
/// of 2^64 − 1 bytes wrapped the sum round to the clip's own size — the banner
/// said "1 MB", and the download wrote until the disk was full.
fn offer_total(size: u64, tracks: &[TrackOffer]) -> Option<u64> {
    tracks
        .iter()
        .try_fold(size, |sum, track| sum.checked_add(track.size))
        .filter(|_| {
            size <= MAX_OFFER_BYTES && tracks.iter().all(|track| track.size <= MAX_TRACK_BYTES)
        })
}

/// What to do with an offer before looking at what it holds.
#[derive(Debug, PartialEq)]
enum Verdict {
    Ask,
    Decline,
    /// No answer at all: not while invisible — an answer would show the user is
    /// there — not for a second offer from a friend whose first still waits,
    /// and not for an offer that cannot be taken at its word (see [`Known`]).
    Ignore,
    /// Not yet: the friends lists were never loaded, so whether the sender is
    /// a friend cannot be told. Held until they are ([`Held`]).
    Hold,
}

/// What is known here about an offer's sender and id, before it is looked at.
#[derive(Debug, Clone, Copy)]
struct Known {
    /// The friends lists have been loaded at least once. Until then nobody is
    /// on them — a refresh that failed after the connection stood left an
    /// offer from a friend unanswered.
    lists_loaded: bool,
    /// The sender is on our own friends list. The server checks that too, but
    /// only what it says it checked is known here.
    friend: bool,
    /// The id is in use already — by an offer still open or running, by one of
    /// our own, or by one called off. A second offer under it would overwrite
    /// the first one's entry mid-transfer.
    id_taken: bool,
    /// An offer from the same friend still waits for an answer.
    asking_already: bool,
}

fn verdict(friends: &FriendsConfig, from: &str, known: Known, sane: bool) -> Verdict {
    if friends.invisible {
        return Verdict::Ignore;
    }
    if !known.lists_loaded {
        return Verdict::Hold;
    }
    if !known.friend || known.id_taken || known.asking_already {
        return Verdict::Ignore;
    }
    let welcome = match friends.accept_clips {
        AcceptClips::All => true,
        AcceptClips::Favorites => friends.favorites.iter().any(|fav| fav == from),
        AcceptClips::Off => false,
    };
    if friends.busy || !welcome || !sane {
        Verdict::Decline
    } else {
        Verdict::Ask
    }
}

/// A `ShareOffer` as it came in.
struct Offer {
    id: String,
    size: u64,
    sha256: String,
    secret: String,
    addr: EndpointAddr,
    meta: Meta,
    stable: bool,
}

/// An offer held back until the friends lists are loaded ([`Verdict::Hold`]),
/// and when it came: it is taken up again after the next refresh that worked
/// ([`retry_held`]), and lasts no longer than it would have anyway.
struct Held<T> {
    from: String,
    id: String,
    arrived: std::time::Instant,
    offer: T,
}

/// How many offers wait for the lists at most; the oldest makes room.
const MAX_HELD: usize = 8;

/// How long an offer that came at `arrived` is still open at `now` — `None`
/// once the [`ANSWER_WITHIN`] the sender waits for an answer is up.
fn open_for(arrived: std::time::Instant, now: std::time::Instant) -> Option<Duration> {
    ANSWER_WITHIN
        .checked_sub(now.saturating_duration_since(arrived))
        .filter(|left| !left.is_zero())
}

/// Puts an offer aside. Offers that have run out go first; an id held
/// already is not held twice. False when this one was not held.
fn hold<T>(held: &mut Vec<Held<T>>, offer: Held<T>, now: std::time::Instant) -> bool {
    held.retain(|h| open_for(h.arrived, now).is_some());
    if held
        .iter()
        .any(|h| h.id == offer.id && h.from == offer.from)
    {
        return false;
    }
    if held.len() >= MAX_HELD {
        held.remove(0);
    }
    held.push(offer);
    true
}

/// The lists have been loaded: the offers held for them are looked at again,
/// in the order they came, those that have not run out meanwhile.
pub(crate) fn retry_held(app: &AppHandle) {
    let Some(share) = app.try_state::<Share>() else {
        return;
    };
    let held = std::mem::take(&mut *share.held.lock());
    let now = std::time::Instant::now();
    for entry in held {
        if open_for(entry.arrived, now).is_some() {
            log::info!(
                "share: taking up the offer {} from {} held for the friends lists",
                entry.id,
                entry.from
            );
            offered(app, &entry.from, entry.offer, entry.arrived);
        }
    }
}

/// An offer from `from`, which came at `arrived` — now, or earlier when it
/// was held.
fn offered(app: &AppHandle, from: &str, offer: Offer, arrived: std::time::Instant) {
    let Offer {
        id,
        size,
        sha256,
        secret,
        addr,
        meta,
        stable,
    } = offer;
    let config = app.state::<AppState>().config_snapshot().friends;
    let share = app.state::<Share>();
    // Relays come in one after another on the one connection to the server,
    // so nothing can slip in between this check and the insert below.
    let (in_taken, asking_already) = {
        let incoming = share.incoming.lock();
        let asking = incoming
            .values()
            .any(|entry| entry.transfer.friend_id == from && entry.transfer.stage == Stage::Asking);
        (incoming.contains_key(&id), asking)
    };
    let known = Known {
        lists_loaded: crate::friends::lists_loaded(app),
        friend: crate::friends::friend_name(app, from).is_some(),
        id_taken: in_taken
            || share.outgoing.lock().contains_key(&id)
            || share.cancelled.lock().contains_key(&id),
        asking_already,
    };
    match verdict(&config, from, known, id.len() <= 64 && sha256.len() == 64) {
        Verdict::Ask => {}
        Verdict::Ignore => {
            log::info!(
                "share: an offer from {from} left unanswered (friend: {}, id taken: {})",
                known.friend,
                known.id_taken
            );
            return;
        }
        Verdict::Hold => {
            let entry = Held {
                from: from.to_owned(),
                id: id.clone(),
                arrived,
                offer: Offer {
                    id,
                    size,
                    sha256,
                    secret,
                    addr,
                    meta,
                    stable,
                },
            };
            if hold(&mut share.held.lock(), entry, std::time::Instant::now()) {
                log::info!("share: an offer from {from} held until the friends lists are loaded");
            }
            return;
        }
        // Busy, or not from someone welcome: a quiet no, without anything popping up.
        Verdict::Decline => {
            crate::friends::send_relay(
                app,
                from,
                &Signal::ShareAnswer {
                    id,
                    accept: false,
                    node: None,
                },
            );
            return;
        }
    }
    let friend_name = crate::friends::friend_name(app, from).unwrap_or_else(|| "A friend".into());
    let name = meta.title.clone().unwrap_or_else(|| meta.file_name.clone());
    let what = noun(meta.screenshot, meta.recording);
    // Tracks that do not add up are left behind; the clip still comes.
    let mut meta = meta;
    if meta.tracks.len() > MAX_TRACKS || meta.tracks.iter().any(|track| track.sha256.len() != 64) {
        meta.tracks.clear();
    }
    let Some(total) = offer_total(size, &meta.tracks) else {
        log::warn!("share: offer {id} from {from} with sizes that cannot be right — refused");
        crate::friends::send_relay(
            app,
            from,
            &Signal::ShareAnswer {
                id,
                accept: false,
                node: None,
            },
        );
        return;
    };
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
    crate::peers::note(app, from, &addr.id.to_string(), stable, "an offer");
    app.state::<Share>().incoming.lock().insert(
        id.clone(),
        Incoming {
            transfer,
            size,
            sha256,
            secret,
            addr,
            meta,
            stable,
        },
    );
    emit(app);
    crate::friends::announce_clip(
        app,
        &friend_name,
        &format!(
            "{friend_name} wants to send you {name} ({})",
            megabytes(total)
        ),
    );
    // Over the game too: the banner cannot be clicked, so it says where the
    // answer goes — the console has the buttons.
    let hotkey = app
        .state::<AppState>()
        .config_snapshot()
        .console_hotkey_spoken();
    crate::sounds::play(app, crate::model::SoundKind::ClipReceived);
    crate::overlay::show_friend(
        app,
        crate::overlay::BannerKind::Friend,
        &friend_name,
        format!("{friend_name} wants to send you a {what}"),
        Some(if hotkey.is_empty() {
            megabytes(total)
        } else {
            format!("{} · {hotkey} to answer", megabytes(total))
        }),
        None,
        crate::friends::friend_avatar(app, from),
    );

    let left = open_for(arrived, std::time::Instant::now()).unwrap_or_default();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(left).await;
        set_in(&app, &id, |t| {
            if t.stage == Stage::Asking {
                t.stage = Stage::Expired;
            }
        });
    });
}

// --- Receiving -----------------------------------------------------------------

async fn receive(app: &AppHandle, id: &str) -> Result<Clip, String> {
    let (size, sha256, secret, addr, meta, from, (friend_id, stable)) = {
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
            (entry.transfer.friend_id.clone(), entry.stable),
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
    let what = noun(meta.screenshot, meta.recording);
    let dir = crate::filing::dir_for(
        Path::new(&config.clip_dir),
        meta.game.as_deref(),
        false,
        kind,
    );
    std::fs::create_dir_all(&dir).map_err(|err| format!("Could not create the folder: {err}"))?;
    // Room for it first, on both drives: the clip goes to the clip folder,
    // its tracks to the app data folder — usually both on C:.
    // Other offers on their way take their share of the same drives: each
    // alone fitting is not enough when they all land.
    let mut meta = meta;
    let tracks_size: u64 = meta.tracks.iter().map(|track| track.size).sum();
    let pending = app
        .state::<Share>()
        .incoming
        .lock()
        .iter()
        .filter(|(key, entry)| key.as_str() != id && entry.transfer.stage == Stage::Moving)
        .map(|(_, entry)| still_to_come(entry.size, entry.transfer.size, entry.transfer.moved))
        .fold((0u64, 0u64), |sum, left| {
            (sum.0.saturating_add(left.0), sum.1.saturating_add(left.1))
        });
    let data_dir = crate::config::data_dir();
    let tracks_anywhere = tracks_size > 0 || pending.1 > 0;
    let same_drive = tracks_anywhere && crate::disk::same_volume(&dir, &data_dir);
    let data_free = if tracks_size > 0 {
        crate::disk::free_bytes(&data_dir)
    } else {
        None
    };
    match room(
        size,
        tracks_size,
        crate::disk::free_bytes(&dir),
        data_free,
        same_drive,
        pending,
    ) {
        Room::Fits => {}
        Room::WithoutTracks => {
            // Tracks are a bonus everywhere else as well: the clip still comes.
            log::warn!("share: no room for the tracks of {id} — receiving the {what} alone");
            meta.tracks.clear();
            // The size shown counted them; without, the bar would stop short.
            set_in(app, id, |transfer| transfer.size = size);
        }
        Room::None => {
            return Err(format!(
                "Not enough free space for this {what} ({}).",
                megabytes(size)
            ))
        }
    }
    let name = safe_file_name(&meta.file_name);
    let part = part_path(&dir, id);

    let router = router(app).await?;
    let cancelled = cancel_flag(app, id);
    // The tracks first: small, and the sender counts the transfer as done
    // once the clip is in — after that it would not hand them out any more.
    let clip_id = uuid::Uuid::new_v4().to_string();
    let has_tracks = fetch_tracks(
        app,
        router.endpoint(),
        &addr,
        id,
        &secret,
        &meta.tracks,
        &clip_id,
        &cancelled,
    )
    .await;
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
        match fetch(
            app,
            router.endpoint(),
            &addr,
            id,
            &secret,
            None,
            size,
            base,
            &part,
            &cancelled,
        )
        .await
        {
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
                    return Err(format!(
                        "The {what} arrived damaged — ask them to send it again."
                    ));
                }
                // The whole clip came from the key `addr.id` names — the
                // connection proved it — so that is a device of theirs.
                if stable {
                    crate::peers::pin(app, &friend_id, &addr.id.to_string());
                }
                if let Ok((mut send, _)) = connection.open_bi().await {
                    let _ = send.write_all(b"k").await;
                    let _ = send.finish();
                }
                // Closing drops whatever is still unsent — the "k" with it,
                // and the sender stood at 100 % for good. The sender hangs up
                // once it has read it; until then, wait (not forever).
                let _ = tokio::time::timeout(Duration::from_secs(5), connection.closed()).await;
                // Chosen only now: two offers of one name no longer pick the
                // same free name while both are still on their way.
                let target = crate::filing::free_name(&dir, &name);
                // With every numbered name taken as well, `free_name` hands
                // back the name itself — and `rename` would replace that file.
                if target.exists() {
                    let _ = std::fs::remove_file(&part);
                    crate::stems::remove(&clip_id);
                    return Err(format!(
                        "There is no free name for this {what} in its folder."
                    ));
                }
                if let Err(err) = std::fs::rename(&part, &target) {
                    crate::stems::remove(&clip_id);
                    return Err(format!("Could not keep the {what}: {err}"));
                }
                // ffmpeg makes the thumbnail: seconds, not for an async worker.
                let app = app.clone();
                return tokio::task::spawn_blocking(move || {
                    file_clip(&app, &clip_id, &target, &meta, size, &from, has_tracks)
                })
                .await
                .map_err(|err| err.to_string())?;
            }
            Err(err) => {
                log::info!("share: attempt {} for {id}: {err}", attempt + 1);
                last_error = err;
            }
        }
    }
    let _ = std::fs::remove_file(&part);
    crate::stems::remove(&clip_id);
    Err(format!("The {what} did not get through: {last_error}"))
}

/// Whether an offer fits on the drives it goes to.
#[derive(Debug, PartialEq, Eq)]
enum Room {
    Fits,
    /// The clip fits, its tracks do not.
    WithoutTracks,
    None,
}

/// Room for a clip of `size` and its tracks, with [`SPACE_MARGIN`] left over
/// on each drive. A free size that cannot be read counts as room: refusing a
/// clip over a question nobody could answer helps nobody.
///
/// `pending` is what other receives still have to write, clips and tracks
/// ([`still_to_come`]): the free space is theirs already.
fn room(
    size: u64,
    tracks_size: u64,
    clip_free: Option<u64>,
    data_free: Option<u64>,
    same_drive: bool,
    pending: (u64, u64),
) -> Room {
    let on_clip_drive = pending
        .0
        .saturating_add(if same_drive { pending.1 } else { 0 });
    let clip_free = clip_free.map(|free| free.saturating_sub(on_clip_drive));
    let data_free = data_free.map(|free| free.saturating_sub(pending.1));
    let short = |free: Option<u64>, need: u64| {
        free.is_some_and(|free| free < need.saturating_add(SPACE_MARGIN))
    };
    if short(clip_free, size) {
        return Room::None;
    }
    if tracks_size == 0 {
        return Room::Fits;
    }
    let tracks_short = if same_drive {
        short(clip_free, size.saturating_add(tracks_size))
    } else {
        short(data_free, tracks_size)
    };
    if tracks_short {
        Room::WithoutTracks
    } else {
        Room::Fits
    }
}

/// What a running receive still has to write: of its clip and of its tracks.
/// `total` counts both (`Transfer::size`), and the tracks come first, so the
/// bytes moved so far count against them first.
fn still_to_come(clip: u64, total: u64, moved: u64) -> (u64, u64) {
    let tracks = total.saturating_sub(clip);
    (
        clip.saturating_sub(moved.saturating_sub(tracks)),
        tracks.saturating_sub(moved),
    )
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
            match fetch(
                app,
                endpoint,
                addr,
                id,
                secret,
                Some(index as u32),
                track.size,
                base,
                &part,
                cancelled,
            )
            .await
            {
                Ok(connection) => {
                    // Every byte is in; the sender waits for this to let go.
                    connection.close(0u32.into(), b"track");
                    let file = part.clone();
                    let whole =
                        tokio::task::spawn_blocking(move || hash_file(&file, u64::MAX)).await;
                    kept = matches!(whole, Ok(Ok(hash)) if hash == track.sha256)
                        && std::fs::rename(&part, &target).is_ok();
                    break;
                }
                Err(err) => log::info!(
                    "share: track {index} of {id}, attempt {}: {err}",
                    attempt + 1
                ),
            }
        }
        if !kept {
            log::info!(
                "share: track {index} of {id} did not arrive — the clip comes without its tracks"
            );
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
    let offset = std::fs::metadata(part)
        .map(|m| m.len())
        .unwrap_or(0)
        .min(size);
    let connection = tokio::time::timeout(
        Duration::from_secs(30),
        endpoint.connect(addr.clone(), ALPN),
    )
    .await
    .map_err(|_| "could not reach them".to_string())?
    .map_err(net)?;
    let (mut send, mut recv) =
        within(HANDSHAKE, "opening the stream", connection.open_bi()).await?;
    let ask = serde_json::to_vec(&Ask {
        id: id.into(),
        secret: secret.into(),
        offset,
        track,
    })
    .map_err(net)?;
    send.write_all(&(ask.len() as u32).to_be_bytes())
        .await
        .map_err(net)?;
    send.write_all(&ask).await.map_err(net)?;
    let _ = send.finish();

    let mut header = [0u8; 8];
    within(
        HANDSHAKE,
        "reading the header",
        recv.read_exact(&mut header),
    )
    .await?;
    let rest = u64::from_be_bytes(header);
    if offset.checked_add(rest) != Some(size) {
        return Err("they offered a different file".into());
    }
    let mut file =
        open_part(part, offset).map_err(|err| format!("Could not write the file: {err}"))?;
    let mut moved = offset;
    let mut buf = vec![0u8; CHUNK];
    let mut last_emit = std::time::Instant::now();
    while moved < size {
        if cancelled.load(Ordering::SeqCst) {
            connection.close(2u32.into(), b"cancelled");
            return Err("cancelled".into());
        }
        let n = match within(IDLE, "they stopped sending", recv.read(&mut buf)).await? {
            Some(n) => n,
            None => break,
        };
        // Never past the size that was agreed on, whatever the other side sends.
        let n = n.min((size - moved) as usize);
        file.write_all(&buf[..n])
            .map_err(|err| format!("Could not write the file: {err}"))?;
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
    // A screenshot's size as its file has it, not as the offer says.
    let (width, height) = if meta.screenshot {
        crate::shot::size_of_png(path).unwrap_or((meta.width, meta.height))
    } else {
        (meta.width, meta.height)
    };
    let clip = Clip {
        id: id.clone(),
        path: path.to_string_lossy().into_owned(),
        created_at: now_ms(),
        duration_ms: meta.duration_ms,
        game: meta.game.clone(),
        width,
        height,
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
            baked: false,
        }),
        original: None,
        original_available: false,
        facecam: None,
        screenshot: meta.screenshot,
        recording: meta.recording,
        tags: vec![crate::clips::from_tag(from)],
    };
    let state = app.state::<AppState>();
    let guard = state.library.lock();
    let library = guard
        .as_ref()
        .ok_or("The clip database is not available.")?;
    library.insert(&clip).map_err(|err| err.to_string())?;
    library
        .set_tags(&clip.id, &clip.tags)
        .map_err(|err| err.to_string())?;
    Ok(clip)
}

// --- Who this PC is ----------------------------------------------------------------

/// This installation's iroh key, and whether it lasts beyond this run.
struct Identity {
    key: SecretKey,
    stable: bool,
}

impl Identity {
    /// The id for `ShareAnswer.node`: only one that lasts is worth keeping.
    fn node(&self) -> Option<String> {
        self.stable.then(|| self.key.public().to_string())
    }
}

/// What to do with `share-key.bin` as it was found.
#[derive(Debug)]
enum KeyFile {
    Use([u8; 32]),
    /// None there (`None`), or one that is clearly no key of ours (why): make
    /// a key and keep it. Kept as it was, a broken file would leave this PC
    /// on a key for one run only for good.
    Create(Option<String>),
    /// One there that does not open for a reason that may pass (DPAPI not
    /// working right now, the file not readable): a key for this run only,
    /// as before 1.0.1. The file stays — it may well open on the next start.
    Ephemeral(String),
}

/// `open` is [`crate::secret::unprotect_coded`]: a failure carries its
/// HRESULT, which tells a broken seal from one that cannot open just now.
fn key_file(
    read: std::io::Result<Vec<u8>>,
    open: impl FnOnce(&[u8]) -> Result<Vec<u8>, (i32, String)>,
) -> KeyFile {
    let sealed = match read {
        Ok(sealed) => sealed,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return KeyFile::Create(None),
        Err(err) => return KeyFile::Ephemeral(err.to_string()),
    };
    if !crate::secret::looks_sealed(&sealed) {
        return KeyFile::Create(Some("it is no sealed key".into()));
    }
    match open(&sealed) {
        Ok(bytes) => match <[u8; 32]>::try_from(bytes.as_slice()) {
            Ok(key) => KeyFile::Use(key),
            Err(_) => KeyFile::Create(Some(format!("it holds {} bytes, not a key", bytes.len()))),
        },
        Err((code, err)) if crate::secret::seal_broken(code) => {
            KeyFile::Create(Some(format!("it does not open: {err}")))
        }
        Err((_, err)) => KeyFile::Ephemeral(err),
    }
}

/// Loaded or made once per run; the router and every offer and answer use it.
fn identity(app: &AppHandle) -> &'static Identity {
    static IDENTITY: std::sync::OnceLock<Identity> = std::sync::OnceLock::new();
    IDENTITY.get_or_init(|| {
        let ephemeral = |reason: &str| {
            log::warn!("share: no lasting key for this PC ({reason}) — one for this run");
            Identity { key: SecretKey::generate(), stable: false }
        };
        let path = match app.path().app_local_data_dir() {
            Ok(dir) => dir.join("share-key.bin"),
            Err(err) => return ephemeral(&err.to_string()),
        };
        match key_file(std::fs::read(&path), crate::secret::unprotect_coded) {
            KeyFile::Use(bytes) => Identity { key: SecretKey::from_bytes(&bytes), stable: true },
            KeyFile::Create(replaced) => {
                if let Some(why) = replaced {
                    log::warn!("share: the kept key cannot be used ({why}) \u{2014} replacing it with a new one");
                }
                let key = SecretKey::generate();
                let kept = crate::secret::protect(&key.to_bytes())
                    .and_then(|sealed| crate::secret::write_file(&path, &sealed).map_err(|err| err.to_string()));
                match kept {
                    Ok(()) => {
                        log::info!("share: made this PC's key");
                        Identity { key, stable: true }
                    }
                    Err(err) => ephemeral(&format!("could not keep it: {err}")),
                }
            }
            KeyFile::Ephemeral(reason) => ephemeral(&reason),
        }
    })
}

/// What the sender does with the receiver's device.
#[derive(Debug, PartialEq)]
enum SenderPin {
    /// Not both known yet: the clip has not arrived, or their yes (which can
    /// come after their dial) has not, or never will (an app before 1.0.1).
    Wait,
    /// The device their yes named took the clip: keep it.
    Pin(String),
    /// Their yes named one device, another took the clip.
    Mismatch { answered: String, reached: String },
}

fn sender_pin(answered: Option<&str>, reached: Option<&str>) -> SenderPin {
    match (answered, reached) {
        (Some(answered), Some(reached)) if answered == reached => {
            SenderPin::Pin(reached.to_owned())
        }
        (Some(answered), Some(reached)) => SenderPin::Mismatch {
            answered: answered.to_owned(),
            reached: reached.to_owned(),
        },
        _ => SenderPin::Wait,
    }
}

/// Once an outgoing clip has arrived and their yes is in, in whichever order:
/// keep the device, or log that it was not the one they named.
fn settle_pin(app: &AppHandle, id: &str) {
    let (friend, verdict) = {
        let share = app.state::<Share>();
        let mut outgoing = share.outgoing.lock();
        let Some(entry) = outgoing.get_mut(id) else {
            return;
        };
        let verdict = sender_pin(entry.answered.as_deref(), entry.reached.as_deref());
        if verdict == SenderPin::Wait {
            return;
        }
        // Settled once.
        entry.answered = None;
        entry.reached = None;
        (entry.transfer.friend_id.clone(), verdict)
    };
    match verdict {
        SenderPin::Wait => {}
        SenderPin::Pin(node) => {
            crate::peers::note(app, &friend, &node, true, "a clip went");
            crate::peers::pin(app, &friend, &node);
        }
        SenderPin::Mismatch { answered, reached } => log::warn!(
            "share: {id} to {friend}: their yes named {} but {} took it — noted only",
            answered.get(..10).unwrap_or(&answered),
            reached.get(..10).unwrap_or(&reached),
        ),
    }
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
            let endpoint = Endpoint::builder(presets::N0)
                .secret_key(identity(app).key.clone())
                .bind()
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
    let (mut send, mut recv) =
        within(HANDSHAKE, "waiting for the ask", connection.accept_bi()).await?;
    let mut length = [0u8; 4];
    within(HANDSHAKE, "reading the ask", recv.read_exact(&mut length)).await?;
    let length = u32::from_be_bytes(length) as usize;
    if length > 4096 {
        return Err("ask too long".into());
    }
    let mut ask = vec![0u8; length];
    within(HANDSHAKE, "reading the ask", recv.read_exact(&mut ask)).await?;
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
    send.write_all(&(size - offset).to_be_bytes())
        .await
        .map_err(net)?;

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
        within(IDLE, "they stopped receiving", send.write_all(&buf[..n])).await?;
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
                o.tracks.clear();
                o.reached = Some(connection.remote_id().to_string());
                (
                    o.transfer.friend_name.clone(),
                    o.transfer.friend_id.clone(),
                    o.transfer.noun(),
                )
            })
            .unwrap_or_else(|| ("Your friend".into(), String::new(), "clip"));
        settle_pin(app, &ask.id);
        // The cut tracks have done their job.
        drop_cuts(&ask.id);
        let avatar = crate::friends::friend_avatar(app, &friend);
        crate::overlay::show_friend(
            app,
            crate::overlay::BannerKind::Friend,
            &name,
            format!("{name} has your {what}"),
            None,
            None,
            avatar,
        );
    } else if bad_hash(connection) {
        // Every byte went out, and what arrived did not match. A line that
        // merely dropped stays open instead: they dial again for the rest.
        set_out(app, &ask.id, |t| {
            fail(
                t,
                "It arrived damaged on their side — try sending it again.",
            )
        });
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

/// Is one of the clip's sends to a friend still under way? The clean-up by
/// age leaves such a clip for its next round (`cleanup.rs`).
pub fn sending(app: &AppHandle, clip_id: &str) -> bool {
    let Some(share) = app.try_state::<Share>() else {
        return false;
    };
    let outgoing = share.outgoing.lock();
    outgoing
        .values()
        .any(|o| !o.transfer.stage.over() && o.transfer.clip_id.as_deref() == Some(clip_id))
}

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
        outgoing
            .get_mut(id)
            .map(|o| change(&mut o.transfer))
            .is_some()
    };
    if changed {
        emit(app);
    }
}

fn set_in(app: &AppHandle, id: &str, change: impl FnOnce(&mut Transfer)) {
    let changed = {
        let share = app.state::<Share>();
        let mut incoming = share.incoming.lock();
        incoming
            .get_mut(id)
            .map(|i| change(&mut i.transfer))
            .is_some()
    };
    if changed {
        emit(app);
    }
}

fn out_stage(app: &AppHandle, id: &str) -> Option<Stage> {
    app.state::<Share>()
        .outgoing
        .lock()
        .get(id)
        .map(|o| o.transfer.stage)
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
    app.state::<Share>()
        .cancelled
        .lock()
        .entry(id.to_owned())
        .or_default()
        .clone()
}

fn fail(transfer: &mut Transfer, error: &str) {
    if !transfer.stage.over() {
        transfer.stage = Stage::Failed;
        transfer.error = Some(error.to_owned());
    }
}

/// The tracks that can be heard in the clip's mix, sorted by index, with the
/// mix renumbered `0..n` in that order — the slots `cut_tracks` writes and the
/// receiver stores. A track without a mix entry counts as 0 dB and unmuted;
/// with no mix at all everything stays.
fn audible_stems(
    mut stems: Vec<crate::model::ClipTrack>,
    mix: &[TrackMix],
) -> (Vec<crate::model::ClipTrack>, Vec<TrackMix>) {
    stems.sort_by_key(|track| track.index);
    if mix.is_empty() {
        return (stems, Vec::new());
    }
    let mut kept = Vec::new();
    let mut levels = Vec::new();
    for track in stems {
        let entry = mix.iter().find(|entry| entry.index == track.index);
        let (gain_db, muted) = entry
            .map(|entry| (entry.gain_db, entry.muted))
            .unwrap_or((0.0, false));
        if muted || !crate::stems::audible(gain_db) {
            continue;
        }
        levels.push(TrackMix {
            index: levels.len() as u32,
            gain_db,
            muted: false,
        });
        kept.push(track);
    }
    (kept, levels)
}

/// The offer to `friend` that still waits for their answer, if there is one.
/// Their side ignores a second offer from us meanwhile (`verdict`, the
/// backstop against a flood), so a second one would only end in "didn't
/// answer" — and that clip would be lost without a word.
fn unanswered<'a>(
    outgoing: impl IntoIterator<Item = &'a Transfer>,
    friend: &str,
) -> Option<&'a Transfer> {
    outgoing
        .into_iter()
        .find(|t| t.friend_id == friend && matches!(t.stage, Stage::Preparing | Stage::Asking))
}

/// Whether a clip with `audio_streams` sound tracks in its file (`None`: could
/// not be read) goes as a copy. Clips from before the track store carry the
/// raw source tracks — microphone, Discord — next to the mix inside the MP4,
/// and muted tracks stay home: those go as a copy with the mix only.
fn sends_a_copy(audio_streams: Option<usize>) -> bool {
    audio_streams.is_none_or(|count| count > 1)
}

/// Where the copy of a clip to send lies — in the scratch folder startup
/// clears, should a crash leave it there.
fn copy_path(id: &str) -> PathBuf {
    crate::config::data_dir()
        .join("temp")
        .join(format!("share-{}.mp4", crate::muxer::sanitize(id)))
}

/// The file that goes to the friend, its size and its hash: the clip itself,
/// or — when it still carries its source tracks — a copy at `copy` with the
/// picture and the mix only, made the way a link's copy is. A copy that fails
/// stops the send: the file itself would take the tracks along.
fn file_to_send(
    source: &Path,
    copy: &Path,
    screenshot: bool,
    what: &str,
) -> Result<(PathBuf, u64, String), String> {
    let streams = (!screenshot).then(|| {
        crate::stems::tracks_in_file(source)
            .ok()
            .map(|tracks| tracks.len())
    });
    let path = match streams {
        Some(streams) if sends_a_copy(streams) => {
            if let Some(dir) = copy.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(err) = crate::links::faststart(source, copy, "copying a clip to send") {
                log::warn!("share: no copy of {} to send: {err}", source.display());
                let _ = std::fs::remove_file(copy);
                return Err(format!("Could not get the {what} ready to send."));
            }
            copy.to_path_buf()
        }
        _ => source.to_path_buf(),
    };
    let size = std::fs::metadata(&path)
        .map_err(|_| format!("The {what}'s file is missing."))?
        .len();
    let sha256 = hash_file(&path, u64::MAX)?;
    Ok((path, size, sha256))
}

/// Where an outgoing transfer's cut tracks lie.
fn cuts_dir(id: &str) -> PathBuf {
    std::env::temp_dir()
        .join("clippiboy-share")
        .join(crate::muxer::sanitize(id))
}

/// Whether an outgoing transfer in `stage` (`None`: not on the list) is done
/// with its cut tracks.
fn may_drop_cuts(stage: Option<Stage>) -> bool {
    stage.is_none_or(|stage| stage.over())
}

/// Removes the cut tracks of a transfer that has ended, in whatever way, and
/// the copy of the clip if one was sent.
fn drop_cuts(id: &str) {
    let _ = std::fs::remove_dir_all(cuts_dir(id));
    let _ = std::fs::remove_file(copy_path(id));
}

/// The part file of an incoming offer, in the folder the clip will go to.
/// Named after the offer, never after the clip: two offers of one name get
/// parts of their own, and a leftover cannot lend its length to another file.
fn part_path(dir: &Path, offer_id: &str) -> PathBuf {
    dir.join(format!(
        "{PART_PREFIX}{}.part",
        crate::muxer::sanitize(offer_id)
    ))
}

const PART_PREFIX: &str = ".clippiboy-";

/// Whether `name` is a part file of ours: `.clippiboy-<letters, digits, _>.part`.
fn is_part_name(name: &str) -> bool {
    name.strip_prefix(PART_PREFIX)
        .and_then(|rest| rest.strip_suffix(".part"))
        .is_some_and(|id| {
            !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// Removes our part files, last changed more than `older_than` ago, from the
/// clip folder and up to two levels below it (as deep as `filing` goes).
/// Folders starting with a dot are not entered. Returns how many went.
pub fn sweep_parts(clip_dir: &Path, older_than: Duration) -> usize {
    let before = std::time::SystemTime::now()
        .checked_sub(older_than)
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let mut removed = 0;
    let mut dirs = vec![(clip_dir.to_path_buf(), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if depth < 2 && !name.starts_with('.') {
                    dirs.push((path, depth + 1));
                }
            } else if kind.is_file()
                && is_part_name(&name)
                && std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .is_ok_and(|at| at < before)
                && std::fs::remove_file(&path).is_ok()
            {
                removed += 1;
            }
        }
    }
    removed
}

/// The individual tracks cut to the clip, into the temp folder. Best effort:
/// whatever goes wrong, the clip still goes — only without its tracks.
fn cut_tracks(
    id: &str,
    stems: &[crate::model::ClipTrack],
    start_ms: u64,
    length_ms: u64,
) -> Vec<Cut> {
    let dir = cuts_dir(id);
    let cut = || -> Result<Vec<Cut>, String> {
        if stems.len() > MAX_TRACKS {
            return Err(format!("{} tracks", stems.len()));
        }
        std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
        let mut cuts = Vec::with_capacity(stems.len());
        let mut sorted: Vec<_> = stems.iter().collect();
        sorted.sort_by_key(|track| track.index);
        for (slot, track) in sorted.into_iter().enumerate() {
            let source = track
                .preview_path
                .as_deref()
                .ok_or("track without a file")?;
            let path = dir.join(format!("{slot}.m4a"));
            // Copying AAC cuts on a frame, some 20 ms — nothing an ear hears.
            let mut command = crate::muxer::ffmpeg();
            command.args(cut_track_args(
                Path::new(source),
                &path,
                start_ms,
                length_ms,
            ));
            crate::muxer::run(
                &mut command,
                "cutting a track to send",
                crate::child::MOVING,
            )?;
            let size = std::fs::metadata(&path)
                .map_err(|err| err.to_string())?
                .len();
            let sha256 = hash_file(&path, u64::MAX)?;
            cuts.push(Cut {
                label: track.label.clone(),
                path,
                size,
                sha256,
            });
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

/// The ffmpeg line [`cut_tracks`] runs for one track. The track file is the
/// clip's own, but read as no more than one all the same.
fn cut_track_args(source: &Path, output: &Path, start_ms: u64, length_ms: u64) -> Vec<String> {
    let seconds = |ms: u64| format!("{:.3}", ms as f64 / 1000.0);
    let mut args: Vec<String> = ["-y", "-hide_banner", "-loglevel", "error"]
        .map(String::from)
        .to_vec();
    args.extend(crate::child::PROGRESS.map(String::from));
    args.extend(crate::muxer::clip_input().map(String::from));
    args.extend([
        "-ss".into(),
        seconds(start_ms),
        "-i".into(),
        source.to_string_lossy().into_owned(),
    ]);
    args.extend(["-t".into(), seconds(length_ms)]);
    args.extend(["-map", "0:a:0", "-c", "copy", "-movflags", "+faststart"].map(String::from));
    args.push(output.to_string_lossy().into_owned());
    args
}

/// SHA-256 of the first `limit` bytes, as hex.
fn hash_file(path: &Path, limit: u64) -> Result<String, String> {
    let file =
        std::fs::File::open(path).map_err(|err| format!("Could not read the file: {err}"))?;
    let mut reader = std::io::BufReader::with_capacity(CHUNK, file.take(limit));
    let mut hasher = Sha256::new();
    std::io::copy(&mut reader, &mut hasher)
        .map_err(|err| format!("Could not read the file: {err}"))?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// The part file, cut back to `offset` and positioned there.
///
/// Not `append`: on Windows that grants only FILE_APPEND_DATA, and cutting the
/// file then fails with "access denied" — which dropped every connection
/// before its first byte.
fn open_part(part: &Path, offset: u64) -> std::io::Result<std::fs::File> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(part)?;
    file.set_len(offset)?;
    file.seek(SeekFrom::Start(offset))?;
    Ok(file)
}

/// Runs one network await under a limit: a peer that goes quiet becomes an error,
/// which the retry loop treats like any lost connection (and checks Cancel first).
async fn within<T, E: std::fmt::Debug>(
    limit: Duration,
    what: &str,
    work: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, String> {
    match tokio::time::timeout(limit, work).await {
        Ok(done) => done.map_err(net),
        Err(_) => Err(format!("{what} timed out")),
    }
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
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
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
    let stem = if stem.trim().is_empty() {
        "clip".to_owned()
    } else {
        stem
    };
    crate::filing::unreserved(&format!("{stem}.{ext}"))
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
    use crate::model::ClipTrack;

    #[test]
    fn a_track_is_cut_from_a_file_read_only_as_a_clip() {
        let args = cut_track_args(Path::new("t/0.m4a"), Path::new("cuts/0.m4a"), 4_000, 12_500);
        let input = args.iter().position(|a| a == "-i").unwrap();
        assert_eq!(
            args[input - 6..input - 2],
            crate::muxer::clip_input(),
            "{args:?}"
        );
        assert_eq!(
            args[input - 2..input + 4],
            ["-ss", "4.000", "-i", "t/0.m4a", "-t", "12.500"]
        );
        assert!(
            args.windows(3).any(|w| w == crate::child::PROGRESS),
            "{args:?}"
        );
        assert_eq!(args.last().map(String::as_str), Some("cuts/0.m4a"));
    }

    #[test]
    fn a_peer_that_goes_quiet_is_given_up_on() {
        let quiet = std::future::pending::<Result<(), String>>();
        let outcome =
            tauri::async_runtime::block_on(within(Duration::from_millis(50), "reading", quiet));
        assert_eq!(outcome, Err("reading timed out".to_string()));
    }

    fn stem(index: u32, label: &str) -> ClipTrack {
        ClipTrack {
            index,
            label: label.into(),
            channels: 2,
            preview_path: Some(format!("{index}.m4a")),
        }
    }
    fn level(index: u32, gain_db: f32, muted: bool) -> TrackMix {
        TrackMix {
            index,
            gain_db,
            muted,
        }
    }

    /// A friend's first offer under a new id.
    const FRESH: Known = Known {
        lists_loaded: true,
        friend: true,
        id_taken: false,
        asking_already: false,
    };
    const NO_PENDING: (u64, u64) = (0, 0);

    #[test]
    fn an_offer_from_someone_not_on_the_list_or_under_a_used_id_gets_no_answer() {
        let config = FriendsConfig::default();
        assert_eq!(
            verdict(
                &config,
                "f",
                Known {
                    friend: false,
                    ..FRESH
                },
                true
            ),
            Verdict::Ignore
        );
        assert_eq!(
            verdict(
                &config,
                "f",
                Known {
                    id_taken: true,
                    ..FRESH
                },
                true
            ),
            Verdict::Ignore
        );
        // Not even the quiet no a busy user gives: that would answer about the
        // offer whose id it took.
        let busy = FriendsConfig {
            busy: true,
            ..Default::default()
        };
        assert_eq!(
            verdict(
                &busy,
                "f",
                Known {
                    id_taken: true,
                    ..FRESH
                },
                true
            ),
            Verdict::Ignore
        );
        assert_eq!(verdict(&config, "f", FRESH, true), Verdict::Ask);
    }

    #[test]
    fn an_invisible_user_neither_asks_nor_answers() {
        let config = FriendsConfig {
            invisible: true,
            busy: true,
            ..Default::default()
        };
        assert_eq!(verdict(&config, "f", FRESH, true), Verdict::Ignore);
    }

    /// Before the lists were ever loaded nobody is a friend yet: the offer
    /// waits for them instead of going unanswered — unless the user is
    /// invisible, which no list changes.
    #[test]
    fn an_offer_before_the_lists_were_loaded_is_held() {
        let unloaded = Known {
            lists_loaded: false,
            friend: false,
            ..FRESH
        };
        assert_eq!(
            verdict(&FriendsConfig::default(), "f", unloaded, true),
            Verdict::Hold
        );
        assert_eq!(
            verdict(
                &FriendsConfig {
                    busy: true,
                    ..Default::default()
                },
                "f",
                unloaded,
                true
            ),
            Verdict::Hold
        );
        let invisible = FriendsConfig {
            invisible: true,
            ..Default::default()
        };
        assert_eq!(verdict(&invisible, "f", unloaded, true), Verdict::Ignore);
    }

    /// A held offer lasts as long as the sender waits for an answer, counted
    /// from when it came; the same offer is held once, and the oldest makes
    /// room.
    #[test]
    fn a_held_offer_runs_out_with_the_offer() {
        let start = std::time::Instant::now();
        let at = |s: u64| start + Duration::from_secs(s);
        assert_eq!(open_for(at(0), at(0)), Some(ANSWER_WITHIN));
        assert_eq!(open_for(at(0), at(45)), Some(Duration::from_secs(15)));
        assert_eq!(open_for(at(0), at(60)), None);
        assert_eq!(open_for(at(0), at(600)), None);

        let held_at = |id: &str, s: u64| Held {
            from: "f".into(),
            id: id.into(),
            arrived: at(s),
            offer: (),
        };
        let mut held = Vec::new();
        assert!(hold(&mut held, held_at("a", 0), at(0)));
        assert!(
            !hold(&mut held, held_at("a", 1), at(1)),
            "the same offer twice"
        );
        assert!(hold(&mut held, held_at("b", 30), at(30)));
        assert!(hold(&mut held, held_at("c", 70), at(70)));
        let ids: Vec<&str> = held.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["b", "c"], "a ran out");
        for n in 0..MAX_HELD - 1 {
            hold(&mut held, held_at(&format!("n{n}"), 71), at(71));
        }
        assert_eq!(held.len(), MAX_HELD);
        assert_eq!(held[0].id, "c", "b made room first");
    }

    #[test]
    fn a_second_offer_from_the_same_friend_waits_for_the_first() {
        assert_eq!(
            verdict(
                &FriendsConfig::default(),
                "f",
                Known {
                    asking_already: true,
                    ..FRESH
                },
                true
            ),
            Verdict::Ignore
        );
    }

    fn outgoing(friend: &str, stage: Stage) -> Transfer {
        Transfer {
            id: format!("{friend}-{stage:?}"),
            direction: Direction::Out,
            friend_id: friend.into(),
            friend_name: friend.into(),
            name: "clip".into(),
            size: 1,
            moved: 0,
            stage,
            error: None,
            clip_id: None,
            game: None,
            duration_ms: 0,
            screenshot: false,
            recording: false,
        }
    }

    /// Their side ignores a second offer while the first still asks — so this
    /// side does not make one: it would only end in "didn't answer".
    #[test]
    fn a_second_clip_waits_until_the_friend_answered_the_first() {
        for stage in [Stage::Preparing, Stage::Asking] {
            assert!(
                unanswered([&outgoing("f", stage)], "f").is_some(),
                "{stage:?}"
            );
        }
        for stage in [
            Stage::Moving,
            Stage::Done,
            Stage::Declined,
            Stage::Expired,
            Stage::Cancelled,
            Stage::Failed,
        ] {
            assert!(
                unanswered([&outgoing("f", stage)], "f").is_none(),
                "{stage:?}"
            );
        }
        assert!(unanswered([&outgoing("g", Stage::Asking)], "f").is_none());
        assert!(unanswered([], "f").is_none());
    }

    /// An old clip still carries its raw source tracks (microphone, Discord)
    /// next to the mix; those must not reach a friend inside the file.
    #[test]
    fn a_clip_with_its_source_tracks_inside_goes_as_a_copy() {
        assert!(!sends_a_copy(Some(0)));
        assert!(!sends_a_copy(Some(1)));
        assert!(sends_a_copy(Some(2)));
        assert!(sends_a_copy(Some(5)));
        // Unreadable: no way to tell what is inside, so not the file itself.
        assert!(sends_a_copy(None));
    }

    /// Where startup clears whatever a crash left, one file per transfer.
    #[test]
    fn the_copy_to_send_lies_in_the_scratch_folder() {
        let copy = copy_path("abc");
        assert!(
            copy.starts_with(crate::config::data_dir().join("temp")),
            "{}",
            copy.display()
        );
        assert_ne!(copy, copy_path("abd"));
        assert!(copy_path("../x").starts_with(crate::config::data_dir().join("temp")));
    }

    #[test]
    fn busy_or_unwelcome_is_a_quiet_no() {
        let favorites = FriendsConfig {
            accept_clips: AcceptClips::Favorites,
            favorites: vec!["g".into()],
            ..Default::default()
        };
        assert_eq!(
            verdict(
                &FriendsConfig {
                    busy: true,
                    ..Default::default()
                },
                "f",
                FRESH,
                true
            ),
            Verdict::Decline
        );
        assert_eq!(
            verdict(
                &FriendsConfig {
                    accept_clips: AcceptClips::Off,
                    ..Default::default()
                },
                "f",
                FRESH,
                true
            ),
            Verdict::Decline
        );
        assert_eq!(verdict(&favorites, "f", FRESH, true), Verdict::Decline);
        assert_eq!(verdict(&favorites, "g", FRESH, true), Verdict::Ask);
        assert_eq!(
            verdict(&FriendsConfig::default(), "f", FRESH, false),
            Verdict::Decline
        );
    }

    #[test]
    fn muted_and_silent_tracks_stay_home() {
        let stems = vec![
            stem(0, "Game"),
            stem(1, "Mic"),
            stem(2, "Discord"),
            stem(3, "Music"),
        ];
        let mix = [
            level(0, 0.0, false),
            level(1, -3.0, true),
            level(2, -60.0, false),
            level(3, 4.0, false),
        ];
        let (kept, mix) = audible_stems(stems, &mix);
        assert_eq!(
            kept.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(),
            ["Game", "Music"]
        );
        assert_eq!(mix, [level(0, 0.0, false), level(1, 4.0, false)]);
    }

    #[test]
    fn without_a_mix_every_track_goes() {
        let (kept, mix) = audible_stems(vec![stem(1, "Mic"), stem(0, "Game")], &[]);
        assert_eq!(kept.iter().map(|t| t.index).collect::<Vec<_>>(), [0, 1]);
        assert!(mix.is_empty());
    }

    #[test]
    fn a_clip_with_every_track_muted_goes_without_tracks() {
        let (kept, mix) = audible_stems(vec![stem(0, "Game")], &[level(0, 0.0, true)]);
        assert!(kept.is_empty() && mix.is_empty());
    }

    #[test]
    fn room_is_checked_for_the_clip_and_its_tracks_together_on_one_drive() {
        const GB: u64 = 1 << 30;
        // Without tracks the app data drive does not matter.
        assert_eq!(
            room(3 << 20, 0, Some(10 * GB), Some(0), false, NO_PENDING),
            Room::Fits
        );
        // Only the app data drive is short: the clip comes alone.
        assert_eq!(
            room(GB, GB, Some(10 * GB), Some(GB / 2), false, NO_PENDING),
            Room::WithoutTracks
        );
        // Both on C:, 11 GB free, 10 GB clip and 6 GB tracks: each alone fits,
        // together they do not.
        assert_eq!(
            room(
                10 * GB,
                6 * GB,
                Some(11 * GB),
                Some(11 * GB),
                true,
                NO_PENDING
            ),
            Room::WithoutTracks
        );
        assert_eq!(
            room(
                10 * GB,
                6 * GB,
                Some(11 * GB),
                Some(11 * GB),
                false,
                NO_PENDING
            ),
            Room::Fits
        );
        assert_eq!(
            room(10 * GB, 0, Some(10 * GB), None, false, NO_PENDING),
            Room::None,
            "the margin counts"
        );
        assert_eq!(
            room(10 * GB, 6 * GB, None, None, true, NO_PENDING),
            Room::Fits,
            "unknown counts as room"
        );
    }

    #[test]
    fn offers_on_their_way_count_against_the_free_space() {
        const GB: u64 = 1 << 30;
        // 10 GB free, two 6 GB clips: the first fits, the second does not
        // while the first still has all of its bytes to come.
        assert_eq!(
            room(6 * GB, 0, Some(10 * GB), None, false, NO_PENDING),
            Room::Fits
        );
        assert_eq!(
            room(6 * GB, 0, Some(10 * GB), None, false, (6 * GB, 0)),
            Room::None
        );
        // What has arrived is gone from the free space already and is not
        // counted twice: 5 of the first 6 GB in, 5 GB free, a 3 GB clip fits.
        let left = still_to_come(6 * GB, 6 * GB, 5 * GB);
        assert_eq!(left, (GB, 0));
        assert_eq!(room(3 * GB, 0, Some(5 * GB), None, false, left), Room::Fits);
        assert_eq!(
            room(3 * GB, 0, Some(5 * GB), None, false, (6 * GB, 0)),
            Room::None
        );
        // Their tracks weigh on the clip drive only when it is the data drive too.
        assert_eq!(
            room(GB, GB, Some(3 * GB), Some(3 * GB), true, (0, 2 * GB)),
            Room::None
        );
        assert_eq!(
            room(GB, GB, Some(3 * GB), Some(3 * GB), false, (0, 2 * GB)),
            Room::WithoutTracks
        );
        assert_eq!(
            room(GB, GB, Some(3 * GB), Some(5 * GB), false, (0, 2 * GB)),
            Room::Fits
        );
        assert_eq!(
            room(GB, 0, None, None, true, (u64::MAX, u64::MAX)),
            Room::Fits,
            "unknown still counts as room"
        );
    }

    #[test]
    fn what_is_still_to_come_counts_the_tracks_first() {
        // 100 bytes of clip, 30 of tracks.
        assert_eq!(still_to_come(100, 130, 0), (100, 30));
        assert_eq!(still_to_come(100, 130, 20), (100, 10));
        assert_eq!(still_to_come(100, 130, 50), (80, 0));
        assert_eq!(still_to_come(100, 130, 130), (0, 0));
        // A clip received without its tracks: the size shown is the clip's.
        assert_eq!(still_to_come(100, 100, 40), (60, 0));
        // Numbers that cannot be right do not wrap round.
        assert_eq!(still_to_come(100, 50, u64::MAX), (0, 0));
    }

    #[test]
    fn a_long_recording_may_be_offered_but_no_size_that_cannot_be_right() {
        let track = |size| TrackOffer {
            label: "Game".into(),
            size,
            sha256: "0".repeat(64),
        };
        // Some two and a half hours at 100 Mbit/s, with two tracks.
        let long = 100 << 30;
        assert_eq!(
            offer_total(long, &[track(300 << 20), track(300 << 20)]),
            Some(long + (600 << 20))
        );
        assert_eq!(offer_total(u64::MAX, &[]), None);
        assert_eq!(offer_total(1 << 20, &[track(MAX_TRACK_BYTES + 1)]), None);
        assert!(offer_total(MAX_OFFER_BYTES, &vec![track(MAX_TRACK_BYTES); MAX_TRACKS]).is_some());
        // The sum wraps round: refused, not counted as the clip's own size.
        let wrapping = [track(u64::MAX - (1 << 20)), track(2 << 20)];
        assert_eq!(offer_total(1 << 20, &wrapping), None);
    }

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
            Signal::ShareAnswer { id, accept, node } => {
                assert!(id == "o1" && accept && node.is_none())
            }
            _ => panic!("wrong kind"),
        }
        let cancel = serde_json::to_value(Signal::ShareCancel { id: "x".into() }).unwrap();
        assert_eq!(cancel["kind"], "shareCancel");
    }

    /// The handshake as 1.0.0 has it, to check both directions against.
    #[derive(Debug, Deserialize, Serialize)]
    #[serde(tag = "kind", rename_all = "camelCase")]
    #[allow(clippy::large_enum_variant, clippy::enum_variant_names)]
    enum OldSignal {
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

    fn test_meta() -> Meta {
        Meta {
            file_name: "clip.mp4".into(),
            title: None,
            game: None,
            duration_ms: 1000,
            width: 1920,
            height: 1080,
            screenshot: false,
            recording: false,
            tracks: Vec::new(),
            mix: Vec::new(),
        }
    }

    #[test]
    fn a_1_0_0_app_reads_the_new_handshake() {
        let key = SecretKey::from_bytes(&[7; 32]);
        let addr = EndpointAddr::new(key.public());
        let node = key.public().to_string();
        let offer = Signal::ShareOffer {
            id: "o1".into(),
            size: 5,
            sha256: "0".repeat(64),
            secret: "s".into(),
            addr: addr.clone(),
            meta: test_meta(),
            stable: true,
        };
        let text = serde_json::to_string(&offer).unwrap();
        assert!(text.contains(r#""stable":true"#));
        match serde_json::from_str::<OldSignal>(&text).unwrap() {
            OldSignal::ShareOffer {
                id, addr: old_addr, ..
            } => assert!(id == "o1" && old_addr == addr),
            other => panic!("wrong kind: {other:?}"),
        }
        let answer = Signal::ShareAnswer {
            id: "o1".into(),
            accept: true,
            node: Some(node.clone()),
        };
        let text = serde_json::to_string(&answer).unwrap();
        assert!(text.contains(&format!(r#""node":"{node}""#)));
        match serde_json::from_str::<OldSignal>(&text).unwrap() {
            OldSignal::ShareAnswer { id, accept } => assert!(id == "o1" && accept),
            other => panic!("wrong kind: {other:?}"),
        }
        // A no names no device, and leaves the field out.
        let no = serde_json::to_string(&Signal::ShareAnswer {
            id: "o1".into(),
            accept: false,
            node: None,
        })
        .unwrap();
        assert!(!no.contains("node"));
    }

    #[test]
    fn the_new_app_reads_a_1_0_0_handshake() {
        let addr = EndpointAddr::new(SecretKey::from_bytes(&[9; 32]).public());
        let old = OldSignal::ShareOffer {
            id: "o2".into(),
            size: 5,
            sha256: "0".repeat(64),
            secret: "s".into(),
            addr: addr.clone(),
            meta: test_meta(),
        };
        match serde_json::from_str::<Signal>(&serde_json::to_string(&old).unwrap()).unwrap() {
            Signal::ShareOffer {
                id,
                addr: new_addr,
                stable,
                ..
            } => assert!(id == "o2" && new_addr == addr && !stable),
            _ => panic!("wrong kind"),
        }
        let old = serde_json::to_string(&OldSignal::ShareAnswer {
            id: "o2".into(),
            accept: true,
        })
        .unwrap();
        match serde_json::from_str::<Signal>(&old).unwrap() {
            Signal::ShareAnswer { node, accept, .. } => assert!(accept && node.is_none()),
            _ => panic!("wrong kind"),
        }
        // A node that is not an id still leaves the answer standing.
        let odd = r#"{"kind":"shareAnswer","id":"o2","accept":true,"node":"?"}"#;
        assert!(matches!(
            serde_json::from_str::<Signal>(odd),
            Ok(Signal::ShareAnswer { accept: true, .. })
        ));
        // The ids pins are kept under are what `Display` writes.
        assert!(crate::peers::is_node(&addr.id.to_string()));
    }

    #[test]
    fn the_key_file_is_used_made_or_passed_over() {
        // A file that begins like a DPAPI blob; `opens` hands back what follows.
        let blob = |key: &[u8]| {
            let mut bytes = crate::secret::protect(b"head").unwrap()[..20].to_vec();
            bytes.extend_from_slice(key);
            Ok(bytes)
        };
        let opens = |bytes: &[u8]| Ok::<_, (i32, String)>(bytes[20..].to_vec());
        assert!(matches!(key_file(blob(&[3; 32]), opens), KeyFile::Use(key) if key == [3; 32]));
        let missing = Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        assert!(matches!(key_file(missing, opens), KeyFile::Create(None)));
        // Opens, but is no key: broken, made again.
        assert!(matches!(
            key_file(blob(&[3; 31]), opens),
            KeyFile::Create(Some(_))
        ));
        // Not even the head of a seal: never ours, made again.
        assert!(matches!(
            key_file(Ok(vec![3; 200]), opens),
            KeyFile::Create(Some(_))
        ));
        // A seal that is broken, or sealed under a key this user no longer has.
        for code in [0x8009_0005_u32, 0x8007_000D, 0x8009_000B] {
            let broken = |_: &[u8]| Err::<Vec<u8>, _>((code as i32, "no".to_string()));
            assert!(
                matches!(key_file(blob(&[3; 200]), broken), KeyFile::Create(Some(_))),
                "{code:#x}"
            );
        }
        // Does not open just now: this run only, the file left alone.
        let fails = |_: &[u8]| Err::<Vec<u8>, _>((0x8007_0005_u32 as i32, "no".to_string()));
        assert!(matches!(
            key_file(blob(&[3; 200]), fails),
            KeyFile::Ephemeral(_)
        ));
        let denied = Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert!(matches!(key_file(denied, opens), KeyFile::Ephemeral(_)));
    }

    #[test]
    fn a_key_kept_sealed_is_the_same_key_again() {
        let key = SecretKey::generate();
        let sealed = crate::secret::protect(&key.to_bytes()).unwrap();
        let KeyFile::Use(bytes) = key_file(Ok(sealed), crate::secret::unprotect_coded) else {
            panic!("did not open")
        };
        assert_eq!(SecretKey::from_bytes(&bytes).public(), key.public());
    }

    #[test]
    fn the_sender_keeps_the_device_its_yes_named() {
        let (a, b) = ("a".repeat(64), "b".repeat(64));
        assert_eq!(sender_pin(None, None), SenderPin::Wait);
        assert_eq!(sender_pin(Some(&a), None), SenderPin::Wait);
        // An app before 1.0.1 names none: nothing kept, nothing said.
        assert_eq!(sender_pin(None, Some(&a)), SenderPin::Wait);
        assert_eq!(sender_pin(Some(&a), Some(&a)), SenderPin::Pin(a.clone()));
        assert_eq!(
            sender_pin(Some(&a), Some(&b)),
            SenderPin::Mismatch {
                answered: a.clone(),
                reached: b.clone()
            }
        );
    }

    #[test]
    fn cuts_stay_while_the_transfer_runs() {
        assert!(may_drop_cuts(None));
        assert!(may_drop_cuts(Some(Stage::Done)));
        assert!(!may_drop_cuts(Some(Stage::Moving)));
        assert!(!may_drop_cuts(Some(Stage::Asking)));
    }

    #[test]
    fn a_device_name_from_a_friend_is_not_a_file_name() {
        assert_eq!(safe_file_name("CON.mp4"), "_CON.mp4");
        assert_eq!(safe_file_name("com1.MKV"), "_com1.mkv");
        assert_eq!(safe_file_name("Console.mp4"), "Console.mp4");
    }

    #[test]
    fn two_offers_of_one_name_get_parts_of_their_own() {
        let dir = Path::new("C:/clips/CS2/Videos");
        assert_ne!(part_path(dir, "a1"), part_path(dir, "b2"));
        assert_eq!(
            part_path(dir, "../x").file_name().unwrap(),
            ".clippiboy-___x.part"
        );
    }

    #[test]
    fn only_our_own_old_parts_are_swept() {
        let root = std::env::temp_dir().join(format!("cb-sweep-{}", uuid::Uuid::new_v4()));
        let videos = root.join("CS2").join("Videos");
        let top = root.join("Videos");
        std::fs::create_dir_all(&videos).unwrap();
        std::fs::create_dir_all(&top).unwrap();
        let hour_ago = std::time::SystemTime::now() - Duration::from_secs(3600);
        let make = |path: &Path, old: bool| {
            let file = std::fs::File::create(path).unwrap();
            if old {
                file.set_modified(hour_ago).unwrap();
            }
        };
        let (a, b) = (
            videos.join(".clippiboy-a.part"),
            top.join(".clippiboy-b.part"),
        );
        let (c, movie) = (
            videos.join(".clippiboy-c.part"),
            videos.join("movie.mp4.part"),
        );
        let odd = videos.join(".clippiboy-.part");
        make(&a, true);
        make(&b, true);
        make(&c, false);
        make(&movie, true);
        make(&odd, true);
        assert_eq!(sweep_parts(&root, Duration::from_secs(600)), 2);
        assert!(c.exists() && movie.exists() && odd.exists() && !a.exists() && !b.exists());
        let _ = std::fs::remove_dir_all(root);
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
        let ask = Ask {
            id: "a".into(),
            secret: "s".into(),
            offset: 7,
            track: None,
        };
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
