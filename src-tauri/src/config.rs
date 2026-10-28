//! Persistent configuration, stored as JSON next to the app database.

use std::collections::VecDeque;
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Map, Value};

use crate::encode;
use crate::model::{
    AppConfig, AudioSource, BufferConfig, ControlConfig, OverlayConfig, RecordingConfig, SourceKind,
    TargetKind,
};

pub fn data_dir() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("ClippiBoy")
}

pub fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

pub fn default_clip_dir() -> PathBuf {
    dirs::video_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("ClippiBoy")
}

pub fn default_config() -> AppConfig {
    AppConfig {
        recording: RecordingConfig {
            target_kind: TargetKind::Monitor,
            target_id: None,
            target_stable_id: None,
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_kbps: encode::bitrate_for(1920, 1080, 60),
            quality: crate::model::default_quality(),
            encoder: encode::preferred_encoder(),
            keyframe_seconds: 2,
        },
        buffer: BufferConfig {
            auto_start: false,
            seconds: 120,
            clip_seconds: 0,
            memory_mb: 0,
        },
        sources: Vec::new(),
        clip_dir: default_clip_dir().to_string_lossy().to_string(),
        save_clip_hotkey: "Ctrl+Shift+S".into(),
        toggle_buffer_hotkey: "Ctrl+Shift+B".into(),
        screenshot_hotkey: crate::model::default_screenshot_hotkey(),
        record_hotkey: crate::model::default_record_hotkey(),
        console_hotkey: crate::model::default_console_hotkey(),
        console_enabled: true,
        console_scale: crate::model::default_console_scale(),
        console_monitor: None,
        console_monitor_stable_id: None,
        console_follow_active_screen: true,
        console_in_capture: false,
        console_glow: true,
        console_style: crate::model::ConsoleStyle::default(),
        auto_start_with_windows: false,
        only_buffer_in_game: true,
        overlay: OverlayConfig::default(),
        tray_hint_shown: false,
        watermark: true,
        setup_done: false,
        usage_stats: true,
        usage_stats_noticed: false,
        control: ControlConfig::default(),
        accent_color: None,
        colors_unlocked: false,
        accent_mode: crate::model::AccentMode::default(),
        accent_color_2: None,
        rgb_speed: crate::model::RgbSpeed::default(),
        friends: crate::model::FriendsConfig::default(),
        sounds: crate::model::SoundConfig::default(),
        revision: 0,
    }
}

/// How many seconds a save actually writes.
///
/// `0` means "the whole buffer" — that was the only behaviour until this setting
/// existed, so a config written before it keeps producing the same clips.
///
/// Never longer than the buffer either: asking for a two-minute clip out of a
/// thirty-second buffer cannot give more than thirty seconds, and pretending
/// otherwise would only put a number in the UI that the file then contradicts.
pub fn effective_clip_seconds(buffer: &BufferConfig) -> u32 {
    if buffer.clip_seconds == 0 {
        return buffer.seconds;
    }
    buffer.clip_seconds.min(buffer.seconds)
}

/// How much memory the packet ring may take, in bytes.
///
/// `0` derives it from bitrate and buffer length, with half again on top for the
/// peaks a variable bitrate produces. That is roughly what the ring cost before
/// there was a budget at all, so a config written without one keeps behaving the
/// way it did.
pub fn effective_memory_bytes(recording: &RecordingConfig, buffer: &BufferConfig) -> u64 {
    if buffer.memory_mb > 0 {
        return buffer.memory_mb as u64 * 1024 * 1024;
    }
    let bytes_per_second = recording.bitrate_kbps as u64 * 1000 / 8;
    let plain = bytes_per_second * buffer.seconds.max(1) as u64;
    // A floor, so a tiny buffer at a low bitrate still has room to hold the
    // group of pictures it must never drop.
    (plain * 3 / 2).max(64 * 1024 * 1024)
}

/// The derived and bounded parts of the recording settings, put right on every
/// write and at load.
///
/// The bitrate follows the picture: from a write that changed only the
/// resolution or the frame rate it kept the old number until the next start,
/// and the "MB per clip" and memory estimates with it (CFG-7). The keyframe
/// interval stays within what the ring and the encoder are built for.
pub fn normalize_recording(recording: &mut RecordingConfig) {
    recording.bitrate_kbps =
        encode::bitrate_for(recording.width, recording.height, recording.fps);
    recording.keyframe_seconds = recording.keyframe_seconds.clamp(1, 10);
}

/// Undo the detour where the leftovers were briefly a source kind of their own,
/// limited to one, with the other endpoint sources switched off.
///
/// There is nothing to switch on any more — an output device records its
/// leftovers by itself as soon as something is recorded application by
/// application. The device that source once pointed at is not recoverable, the
/// kind carried no id, so it falls back to the default output device, which is
/// what an empty id means everywhere else (see `capture::device_client`).
fn migrate_sources(config: &mut AppConfig) {
    if !config
        .sources
        .iter()
        .any(|source| matches!(source.kind, SourceKind::Leftovers))
    {
        return;
    }

    for source in &mut config.sources {
        match &source.kind {
            SourceKind::Leftovers => {
                source.kind = SourceKind::OutputDevice {
                    device_id: String::new(),
                };
                log::info!(
                    "'{}' is the default output device again",
                    source.label
                );
            }
            SourceKind::OutputDevice { .. } if !source.enabled => {
                source.enabled = true;
                log::info!("'{}' switched back on", source.label);
            }
            _ => {}
        }
    }
}

/// How the config file came in at start.
#[derive(Debug, Clone, PartialEq)]
pub enum Loaded {
    /// Read and parsed — or not there at all, which is a first start.
    Fine,
    /// The file was there but could not be used. `aside` is where a damaged
    /// file was moved (`None` when it could not be read at all, or not be
    /// moved — then it is left alone and nothing is saved over it this
    /// session); `from_backup` says whether the copy from the last good start
    /// stands in for it.
    Recovered {
        aside: Option<PathBuf>,
        from_backup: bool,
        reason: String,
    },
}

impl Loaded {
    /// What the start-up banner says, if anything went wrong.
    pub fn message(&self) -> Option<String> {
        let Loaded::Recovered {
            aside,
            from_backup,
            reason,
        } = self
        else {
            return None;
        };
        Some(match aside {
            Some(path) => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                if *from_backup {
                    format!(
                        "The settings file was damaged. ClippiBoy restored the copy from the last good start; the damaged file is kept as {name}."
                    )
                } else {
                    format!(
                        "The settings file was damaged and no good copy was left. ClippiBoy starts with default settings; the damaged file is kept as {name}."
                    )
                }
            }
            None => {
                let running = if *from_backup {
                    "the last good copy"
                } else {
                    "default settings"
                };
                format!(
                    "The settings file could not be opened ({reason}). ClippiBoy runs on {running} for now and leaves the file alone until the next start."
                )
            }
        })
    }
}

/// Set at start when the config file could not be read — another program
/// holding it, say — or was damaged and could not be moved aside. Saving would put this
/// session's settings over a file that may be perfectly fine, so nothing is
/// written until the next start.
static HOLD_SAVES: AtomicBool = AtomicBool::new(false);

/// Read the config file in `dir` as it stands, before any fix-ups.
///
/// Only a file that is not there means a first start. A good file is copied to
/// `config.json.bak` — the copy from the last good start. Bytes that are not
/// a config (not UTF-8, not JSON, JSON that is not a config) move the file
/// aside as `config.json.broken-<stamp_ms>` and bring that copy back, so the
/// defaults never get saved over the user's settings. A file that cannot be
/// read at all is left where it is (`aside: None`): the copy stands in for
/// it, and nothing is saved over it until the next start.
fn read_config(dir: &Path, stamp_ms: i64) -> (Option<AppConfig>, Loaded) {
    let path = dir.join("config.json");
    let mut attempt = 0;
    let read = loop {
        match std::fs::read_to_string(&path) {
            // A sharing violation or an antivirus scan at autostart usually
            // passes within a moment.
            Err(err)
                if attempt < 3
                    && !matches!(err.kind(), ErrorKind::NotFound | ErrorKind::InvalidData) =>
            {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(100));
            }
            other => break other,
        }
    };
    // Only bytes that were read and are no config make a damaged file. One
    // that could not be read at all may be perfectly fine — held by another
    // program, say — and moving it aside would lose the last session's
    // changes: it stays, and nothing is saved over it this session.
    let (reason, damaged) = match read {
        Err(err) if err.kind() == ErrorKind::NotFound => return (None, Loaded::Fine),
        // Not UTF-8: read, and no config.
        Err(err) if err.kind() == ErrorKind::InvalidData => (err.to_string(), true),
        Err(err) => (err.to_string(), false),
        Ok(text) => match serde_json::from_str::<AppConfig>(&text) {
            Ok(config) => {
                if let Err(err) = write_synced(&dir.join("config.json.bak"), &text) {
                    log::warn!("could not keep a copy of the settings: {err}");
                }
                return (Some(config), Loaded::Fine);
            }
            Err(err) => (err.to_string(), true),
        },
    };

    let aside = if damaged {
        let target = dir.join(format!("config.json.broken-{stamp_ms}"));
        match std::fs::rename(&path, &target) {
            Ok(()) => Some(target),
            Err(err) => {
                log::warn!("could not move the damaged settings file aside: {err}");
                None
            }
        }
    } else {
        None
    };
    let backup = std::fs::read_to_string(dir.join("config.json.bak"))
        .ok()
        .and_then(|text| serde_json::from_str::<AppConfig>(&text).ok());
    let from_backup = backup.is_some();
    (
        backup,
        Loaded::Recovered {
            aside,
            from_backup,
            reason,
        },
    )
}

pub fn load() -> (AppConfig, Loaded) {
    let dir = data_dir();
    let stamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0);
    let (read, loaded) = read_config(&dir, stamp_ms);
    let mut config = match read {
        Some(mut config) => {
            // Check the encoder against the hardware actually present.
            config.recording.encoder = encode::resolve(config.recording.encoder);
            // The bitrate is derived from the picture, not chosen by hand —
            // a config from when it was a slider carries a number that means
            // nothing now (40 Mbit/s at any resolution, as it happens). It
            // would otherwise still size the memory budget.
            normalize_recording(&mut config.recording);
            migrate_sources(&mut config);
            config
        }
        None => default_config(),
    };
    // Revision 0 stays the mark of a window that has not read the settings
    // yet ([`unread`]) — a config from 0.8.3, before revisions, or a first
    // start included.
    config.revision = config.revision.max(FIRST_REVISION);
    if let Loaded::Recovered {
        aside,
        from_backup,
        reason,
    } = &loaded
    {
        log::warn!(
            "config unreadable ({reason}) — moved aside: {aside:?}, from the backup: {from_backup}"
        );
        match aside {
            None => HOLD_SAVES.store(true, Ordering::SeqCst),
            // The damaged file is out of the way: write what stands in for it
            // now, or a session that changes nothing leaves no config.json, and
            // the next start would be a first start.
            Some(_) => {
                if let Err(err) = save_in(&dir, &config) {
                    log::warn!("could not write the recovered settings: {err}");
                }
            }
        }
    }
    (config, loaded)
}

/// Write the config file. Every caller holds `AppState.config` while it does,
/// so two saves never race.
pub fn save(config: &AppConfig) -> io::Result<()> {
    if HOLD_SAVES.load(Ordering::SeqCst) {
        return Err(io::Error::other(
            "the settings file could not be read at start — it is not overwritten",
        ));
    }
    save_in(&data_dir(), config)
}

fn save_in(dir: &Path, config: &AppConfig) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let text = serde_json::to_string_pretty(config)?;
    write_synced(&dir.join("config.json"), &text)
}

/// Write `text` to `path` so that the file is either the old one or the whole
/// new one, even after a crash or a power cut: a temp name no other writer
/// shares, flushed to disk, then renamed over it.
pub(crate) fn write_synced(path: &Path, text: &str) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(
        "{name}.{}-{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Flags the app sets on its own ("shown once") only ever go from false to
/// true. A settings page still holding the config it loaded at start sends the
/// old false with its next change, and would bring the notice back.
pub fn keep_once_flags(next: &mut AppConfig, previous: &AppConfig) {
    next.usage_stats_noticed |= previous.usage_stats_noticed;
    next.tray_hint_shown |= previous.tray_hint_shown;
}

/// The clip folder and the control token change only through their own
/// commands (`set_clip_dir`, "New" token (`control::restart`)). A settings write — from a
/// page holding stale or mock values — carries whatever it likes in them: a
/// folder that grants a whole drive, "mock-token" for the Stream Deck's key.
pub fn keep_core_owned(next: &mut AppConfig, previous: &AppConfig) {
    next.clip_dir = previous.clip_dir.clone();
    next.control.token = previous.control.token.clone();
}

/// How many earlier configs the core keeps to merge a stale write against.
/// A window more than this many changes behind is taken as it stands.
pub const HISTORY_LEN: usize = 16;

/// Keep `config` as one a window may build its next write on, the oldest
/// falling out past [`HISTORY_LEN`].
pub fn remember(history: &mut VecDeque<AppConfig>, config: &AppConfig) {
    history.push_back(config.clone());
    while history.len() > HISTORY_LEN {
        history.pop_front();
    }
}

/// The revision a loaded config starts at, at least. The core never gives out
/// 0 once it holds one, so 0 means a window still on its stand-in values.
pub const FIRST_REVISION: u64 = 1;

/// Is `revision` that of a window that never read the settings? The main
/// window shows stand-in values at revision 0 until the real ones arrive
/// (`src/lib/mock.ts`); a write built on them carries made-up sources,
/// hotkeys and settings and must not replace the user's.
pub fn unread(revision: u64, current: &AppConfig) -> bool {
    revision == 0 && current.revision >= FIRST_REVISION
}

/// A write from a window, put onto the config the core holds.
///
/// The contract with the windows: every write carries the revision of the
/// copy it was built on — the last one the core answered with or announced.
///
/// A window that holds the current revision replaces the config, as it always
/// did. One that holds an older copy — the main window while the console muted
/// a source — would undo every change made since with values it never touched:
/// it is merged against the copy it was given (`merge_stale`). A revision no
/// longer in the history is taken as it stands. One built on no read at all
/// (revision 0, [`unread`]) is not taken: the config stays as it is. The
/// result always counts up, so the window hears the real config back.
pub fn accept(current: &AppConfig, history: &VecDeque<AppConfig>, incoming: AppConfig) -> AppConfig {
    if unread(incoming.revision, current) {
        log::warn!(
            "a settings write built on no settings read (revision 0, now {}) — refused",
            current.revision
        );
        let mut next = current.clone();
        next.revision = current.revision + 1;
        return next;
    }
    let mut next = if incoming.revision >= current.revision {
        incoming
    } else {
        match history.iter().rev().find(|old| old.revision == incoming.revision) {
            Some(base) => merge_stale(base, &incoming, current),
            None => {
                log::warn!(
                    "a settings write from revision {} (now {}) is too old to merge — taken as it stands",
                    incoming.revision,
                    current.revision
                );
                incoming
            }
        }
    };
    next.revision = current.revision + 1;
    next
}

/// A source command's write, as a whole config for [`accept`]: `change` (one
/// row put in or taken out) done to the copy the window built that row on.
///
/// The console sends whole rows from its own copy. Put onto the config as it
/// stands, a row read before the main window turned that source down would
/// undo the level with the mute; put onto its own revision, `accept` merges it
/// field by field. Without a revision, or with one the history no longer
/// holds, the row goes onto the current config as before. A row from a window
/// that never read the settings ([`unread`]) changes nothing.
pub fn with_sources(
    current: &AppConfig,
    history: &VecDeque<AppConfig>,
    base: Option<u64>,
    change: impl FnOnce(&mut Vec<AudioSource>),
) -> AppConfig {
    if base.is_some_and(|revision| unread(revision, current)) {
        log::warn!("a source write built on no settings read (revision 0) — refused");
        return current.clone();
    }
    let mut incoming = base
        .and_then(|revision| history.iter().rev().find(|old| old.revision == revision))
        .unwrap_or(current)
        .clone();
    change(&mut incoming.sources);
    incoming
}

/// Replace the row with this source's id, or add it at the end.
pub fn put_source(sources: &mut Vec<AudioSource>, source: AudioSource) {
    match sources.iter_mut().find(|s| s.id == source.id) {
        Some(existing) => *existing = source,
        None => sources.push(source),
    }
}

/// Three-way merge of a window's write: `base` is the copy the window was
/// given, `incoming` what it sent back, `current` what the core holds now.
///
/// Over the JSON form: what the window left as it was keeps `current`; objects
/// merge key by key; lists of items with a string `id` (the audio sources)
/// merge item by item; any other value the window changed is the window's.
pub fn merge_stale(base: &AppConfig, incoming: &AppConfig, current: &AppConfig) -> AppConfig {
    let (Ok(base_json), Ok(incoming_json), Ok(current_json)) = (
        serde_json::to_value(base),
        serde_json::to_value(incoming),
        serde_json::to_value(current),
    ) else {
        return incoming.clone();
    };
    let merged = merge_value(&base_json, &incoming_json, &current_json);
    serde_json::from_value(merged).unwrap_or_else(|err| {
        log::warn!("a merged settings write is no config ({err}) — the window's taken as it stands");
        incoming.clone()
    })
}

fn merge_value(base: &Value, incoming: &Value, current: &Value) -> Value {
    if incoming == base {
        return current.clone();
    }
    match (base, incoming, current) {
        (Value::Object(base), Value::Object(incoming), Value::Object(current)) => {
            // A tagged enum (a source's kind) whose variant changed is one value:
            // its fields mean nothing on another variant.
            let tag = |object: &Map<String, Value>| object.get("type").cloned();
            if tag(base) != tag(incoming) {
                return Value::Object(incoming.clone());
            }
            if tag(base) != tag(current) {
                return Value::Object(current.clone());
            }
            Value::Object(merge_objects(base, incoming, current))
        }
        (Value::Array(base), Value::Array(incoming), Value::Array(current))
            if [base, incoming, current].iter().all(|list| list.iter().all(|item| item_id(item).is_some())) =>
        {
            Value::Array(merge_by_id(base, incoming, current))
        }
        _ => incoming.clone(),
    }
}

fn merge_objects(
    base: &Map<String, Value>,
    incoming: &Map<String, Value>,
    current: &Map<String, Value>,
) -> Map<String, Value> {
    let mut merged = Map::new();
    for (key, value) in incoming {
        match (base.get(key), current.get(key)) {
            (Some(was), Some(now)) => {
                merged.insert(key.clone(), merge_value(was, value, now));
            }
            // Gone elsewhere and untouched here: stays gone.
            (Some(was), None) if was == value => {}
            _ => {
                merged.insert(key.clone(), value.clone());
            }
        }
    }
    for (key, value) in current {
        // Removed in the window when the base had it; added elsewhere when not.
        if !incoming.contains_key(key) && !base.contains_key(key) {
            merged.insert(key.clone(), value.clone());
        }
    }
    merged
}

fn item_id(item: &Value) -> Option<&str> {
    item.get("id").and_then(Value::as_str)
}

/// The sources, item by item: removed in the window → removed, added in the
/// window → appended, in all three → merged, removed elsewhere → stays
/// removed. The order is the current one, unless the window reordered.
fn merge_by_id(base: &[Value], incoming: &[Value], current: &[Value]) -> Vec<Value> {
    let find = |list: &[Value], id: &str| list.iter().find(|item| item_id(item) == Some(id)).cloned();
    let ids = |list: &[Value]| -> Vec<String> {
        list.iter().filter_map(item_id).map(str::to_owned).collect()
    };
    let (base_ids, incoming_ids) = (ids(base), ids(incoming));
    let kept_order = |list: &[String], other: &[String]| -> Vec<String> {
        list.iter().filter(|id| other.contains(id)).cloned().collect()
    };
    let reordered = kept_order(&base_ids, &incoming_ids) != kept_order(&incoming_ids, &base_ids);
    let (first, then) = if reordered {
        (incoming_ids, ids(current))
    } else {
        (ids(current), incoming_ids)
    };
    let extra: Vec<String> = then.into_iter().filter(|id| !first.contains(id)).collect();
    let order = first.into_iter().chain(extra);

    order
        .filter_map(|id| match (find(base, &id), find(incoming, &id), find(current, &id)) {
            (Some(was), Some(value), Some(now)) => Some(merge_value(&was, &value, &now)),
            // Removed in the window, or elsewhere.
            (Some(_), _, _) => None,
            // Added in the window (here and there both: the window's).
            (None, Some(value), _) => Some(value),
            // Added elsewhere.
            (None, None, now) => now,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_settings_page_cannot_move_the_clip_folder_or_change_the_token() {
        let mut previous = default_config();
        previous.clip_dir = r"D:\Clips".into();
        previous.control.token = "real".into();
        let mut next = default_config();
        next.clip_dir = r"C:\..".into();
        next.control.token = "mock-token".into();
        keep_core_owned(&mut next, &previous);
        assert_eq!(next.clip_dir, r"D:\Clips");
        assert_eq!(next.control.token, "real");
    }

    #[test]
    fn a_stale_settings_page_cannot_take_back_a_shown_notice() {
        // The app set both flags itself; the page still holds the config it
        // loaded at start and sends all of it with the next change.
        let mut previous = default_config();
        previous.usage_stats_noticed = true;
        previous.tray_hint_shown = true;
        let mut next = default_config();
        keep_once_flags(&mut next, &previous);
        assert!(next.usage_stats_noticed);
        assert!(next.tray_hint_shown);

        // And a page that knows better still gets its way going forward.
        let mut next = default_config();
        next.usage_stats_noticed = true;
        keep_once_flags(&mut next, &default_config());
        assert!(next.usage_stats_noticed);
    }

    fn source(id: &str, kind: SourceKind, enabled: bool) -> AudioSource {
        AudioSource {
            id: id.into(),
            label: id.into(),
            kind,
            enabled,
            gain_db: 0.0,
            muted: false,
            solo: false,
            separate_track: false,
        }
    }

    fn endpoint(device_id: &str) -> SourceKind {
        SourceKind::OutputDevice {
            device_id: device_id.into(),
        }
    }

    fn with(sources: Vec<AudioSource>) -> AppConfig {
        AppConfig {
            sources,
            ..default_config()
        }
    }

    /// A config written before the Stream Deck port existed has no `control`
    /// block. Without `#[serde(default)]` on that field the parse fails, and
    /// `load()` answers a failed parse by throwing the **whole** configuration
    /// away — audio sources, hotkeys and clip folder included. That is the most
    /// expensive bug this file can have, so it gets a test rather than a comment.
    #[test]
    fn a_configuration_written_before_the_stream_deck_port_still_loads() {
        let mut written =
            serde_json::to_value(with(vec![source("mic", endpoint("dev-1"), true)])).unwrap();
        written
            .as_object_mut()
            .expect("a config is an object")
            .remove("control")
            .expect("the field is called 'control' in JSON too");

        let parsed: AppConfig =
            serde_json::from_value(written).expect("a config without 'control' no longer loads");
        assert_eq!(parsed.sources.len(), 1);
        assert_eq!(parsed.control.port, crate::model::DEFAULT_CONTROL_PORT);
        // Nothing saved yet — `control::start` replaces it at every start.
        assert!(parsed.control.token.is_empty());
    }

    /// What the intermediate version left behind: one source turned into a kind
    /// of its own, the other endpoints switched off.
    #[test]
    fn the_leftovers_kind_becomes_a_plain_device_again() {
        let mut config = with(vec![
            source("system", SourceKind::Leftovers, true),
            source("chat", endpoint("dev-chat"), false),
            source("music", endpoint("dev-music"), false),
        ]);
        migrate_sources(&mut config);

        // The kind carried no device, so the default output device it is.
        assert_eq!(config.sources[0].kind, endpoint(""));
        for source in &config.sources[1..] {
            assert!(source.enabled, "switched off by the detour, not by the user");
            assert_eq!(
                source.kind,
                endpoint(&format!("dev-{}", source.id)),
                "the device has to survive"
            );
        }
    }

    /// Nothing to undo — a disabled endpoint stays disabled.
    #[test]
    fn a_configuration_without_the_detour_is_left_alone() {
        let mut config = with(vec![
            source("speakers", endpoint("dev-1"), true),
            source("headset", endpoint("dev-2"), false),
        ]);
        let before = config.sources.clone();
        migrate_sources(&mut config);

        for (before, after) in before.iter().zip(&config.sources) {
            assert_eq!(before.kind, after.kind);
            assert_eq!(before.enabled, after.enabled);
        }
    }

    fn buffer(seconds: u32, clip_seconds: u32) -> BufferConfig {
        BufferConfig {
            auto_start: false,
            seconds,
            clip_seconds,
            memory_mb: 0,
        }
    }

    /// The value every config written before this setting existed carries. It has
    /// to mean "the whole buffer", or an update would silently shorten everyone's
    /// clips.
    #[test]
    fn zero_means_the_whole_buffer() {
        assert_eq!(effective_clip_seconds(&buffer(120, 0)), 120);
    }

    #[test]
    fn a_shorter_clip_is_taken_as_it_stands() {
        assert_eq!(effective_clip_seconds(&buffer(120, 30)), 30);
    }

    /// The buffer is the hard limit — there is nothing older to write.
    #[test]
    fn a_clip_longer_than_the_buffer_is_capped() {
        assert_eq!(effective_clip_seconds(&buffer(30, 120)), 30);
    }

    /// The flag one version wrote onto the device is gone from the model. Serde
    /// has to shrug it off rather than fail — a configuration it cannot read is
    /// thrown away whole.
    #[test]
    fn the_old_flag_on_a_device_is_ignored() {
        let raw = r#"{"type":"outputDevice","deviceId":"d","leftoversOnly":true}"#;
        let kind: SourceKind = serde_json::from_str(raw).unwrap();
        assert_eq!(kind, endpoint("d"));
    }

    /// A fresh, empty folder to play the data folder.
    fn config_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("clippiboy-config-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_missing_config_is_a_first_start() {
        let dir = config_dir();
        let (config, loaded) = read_config(&dir, 1);
        assert!(config.is_none());
        assert_eq!(loaded, Loaded::Fine);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "nothing written");
    }

    #[test]
    fn a_good_config_is_kept_as_the_backup() {
        let dir = config_dir();
        let text = serde_json::to_string_pretty(&default_config()).unwrap();
        std::fs::write(dir.join("config.json"), &text).unwrap();
        let (config, loaded) = read_config(&dir, 1);
        assert!(config.is_some());
        assert_eq!(loaded, Loaded::Fine);
        assert_eq!(std::fs::read_to_string(dir.join("config.json.bak")).unwrap(), text);
    }

    #[test]
    fn a_damaged_config_is_set_aside_and_the_backup_restored() {
        for damage in [vec![0u8; 3], vec![0xff, 0xfe, 0x00]] {
            let dir = config_dir();
            let mut good = default_config();
            good.clip_dir = r"D:\Clips".into();
            std::fs::write(dir.join("config.json.bak"), serde_json::to_string(&good).unwrap()).unwrap();
            std::fs::write(dir.join("config.json"), &damage).unwrap();
            let (config, loaded) = read_config(&dir, 1234);
            assert_eq!(config.expect("the backup").clip_dir, r"D:\Clips");
            let aside = dir.join("config.json.broken-1234");
            assert!(matches!(&loaded, Loaded::Recovered { aside: Some(path), from_backup: true, .. } if *path == aside));
            assert_eq!(std::fs::read(&aside).unwrap(), damage, "the damaged file is kept as it was");
            assert!(!dir.join("config.json").exists());
        }
    }

    #[test]
    fn without_a_backup_the_damaged_file_is_still_kept() {
        let dir = config_dir();
        std::fs::write(dir.join("config.json"), b"{\"recording\": ").unwrap();
        let (config, loaded) = read_config(&dir, 7);
        assert!(config.is_none());
        assert!(matches!(loaded, Loaded::Recovered { aside: Some(_), from_backup: false, .. }));
        assert!(dir.join("config.json.broken-7").is_file());
    }

    #[test]
    fn a_read_error_is_not_a_first_start() {
        let dir = config_dir();
        // A folder where the file should be: reading fails, and not with NotFound.
        std::fs::create_dir_all(dir.join("config.json")).unwrap();
        let mut good = default_config();
        good.clip_dir = r"D:\Clips".into();
        std::fs::write(dir.join("config.json.bak"), serde_json::to_string(&good).unwrap()).unwrap();
        let (config, loaded) = read_config(&dir, 9);
        assert!(matches!(loaded, Loaded::Recovered { aside: None, from_backup: true, .. }), "{loaded:?}");
        assert_eq!(config.expect("the backup").clip_dir, r"D:\Clips");
        assert!(dir.join("config.json").is_dir(), "a file that could not be read is never moved");
        assert!(!dir.join("config.json.broken-9").exists());
        assert!(
            loaded.message().unwrap().starts_with("The settings file could not be opened ("),
            "{:?}",
            loaded.message()
        );
        assert!(loaded.message().unwrap().ends_with("leaves the file alone until the next start."));
    }

    #[test]
    fn concurrent_saves_never_leave_torn_json() {
        let dir = config_dir();
        let threads: Vec<_> = (0..8)
            .map(|n| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    let mut config = default_config();
                    config.clip_dir = format!(r"D:\Clips{n}");
                    (0..25).all(|_| save_in(&dir, &config).is_ok())
                })
            })
            .collect();
        for thread in threads {
            assert!(thread.join().unwrap(), "a save failed");
        }
        let text = std::fs::read_to_string(dir.join("config.json")).unwrap();
        assert!(serde_json::from_str::<AppConfig>(&text).is_ok());
        let temps = std::fs::read_dir(&dir).unwrap().flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(temps, 0);
    }

    fn revision(mut config: AppConfig, revision: u64) -> AppConfig {
        config.revision = revision;
        config
    }

    fn muted(mut source: AudioSource) -> AudioSource {
        source.muted = true;
        source
    }

    fn json(config: &AppConfig) -> serde_json::Value {
        serde_json::to_value(config).unwrap()
    }

    fn ids(config: &AppConfig) -> Vec<&str> {
        config.sources.iter().map(|source| source.id.as_str()).collect()
    }

    fn mic() -> AudioSource {
        source("mic", SourceKind::InputDevice { device_id: "dev-mic".into() }, true)
    }

    fn game() -> AudioSource {
        source("game", SourceKind::Game, true)
    }

    /// CFG-1: the console mutes the mic; the main window, still holding the
    /// copy from before, flips a banner switch and sends all of it back.
    #[test]
    fn a_stale_window_write_keeps_the_consoles_mute() {
        let base = revision(with(vec![mic()]), 4);
        let current = revision(with(vec![muted(mic())]), 5);
        let mut incoming = base.clone();
        incoming.overlay.enabled = !base.overlay.enabled;

        let merged = merge_stale(&base, &incoming, &current);
        assert!(merged.sources[0].muted, "the console's mute came undone");
        assert_eq!(merged.overlay.enabled, !base.overlay.enabled);

        let history = VecDeque::from([base.clone(), current.clone()]);
        let accepted = accept(&current, &history, incoming);
        assert!(accepted.sources[0].muted);
        assert_eq!(accepted.overlay.enabled, !base.overlay.enabled);
        assert_eq!(accepted.revision, 6);
    }

    /// The same, end to end the way the core runs it: the console's write
    /// (`upsert_source` on a snapshot) and the main window's `set_config`
    /// both go through `accept` and then `set_config`'s settle, and every
    /// result is remembered.
    #[test]
    fn the_consoles_mute_survives_the_main_windows_next_switch() {
        let mut history = VecDeque::new();
        let mut core = revision(with(vec![mic(), game()]), 4);
        core.control.token = "real".into();
        remember(&mut history, &core);
        // What the main window holds: the config as it was read at start.
        let window = core.clone();

        // The console mutes the mic.
        let mut snapshot = core.clone();
        snapshot.sources[0] = muted(mic());
        core = accept(&core, &history, snapshot);
        remember(&mut history, &core);

        // The main window flips a banner switch on its old copy.
        let mut incoming = window.clone();
        incoming.overlay.on_clip_saved = !window.overlay.on_clip_saved;
        let mut next = accept(&core, &history, incoming);
        keep_core_owned(&mut next, &core);
        keep_once_flags(&mut next, &core);
        remember(&mut history, &next);

        assert!(next.sources[0].muted, "the mic is recorded again");
        assert!(!next.sources[1].muted);
        assert_eq!(next.overlay.on_clip_saved, !window.overlay.on_clip_saved);
        assert_eq!(next.control.token, "real");
        assert_eq!(next.revision, 6);
    }

    #[test]
    fn two_changes_to_one_source_merge_field_by_field() {
        let base = revision(with(vec![mic()]), 4);
        let current = revision(with(vec![muted(mic())]), 5);
        let mut incoming = base.clone();
        incoming.sources[0].gain_db = -6.0;

        let merged = merge_stale(&base, &incoming, &current);
        assert!(merged.sources[0].muted);
        assert_eq!(merged.sources[0].gain_db, -6.0);
    }

    #[test]
    fn a_source_added_in_the_window_survives_a_stale_write() {
        let base = revision(with(vec![mic()]), 4);
        let current = revision(with(vec![muted(mic())]), 5);
        let incoming = revision(with(vec![mic(), game()]), 4);

        let merged = merge_stale(&base, &incoming, &current);
        assert_eq!(ids(&merged), ["mic", "game"]);
        assert!(merged.sources[0].muted);
    }

    #[test]
    fn a_source_added_elsewhere_survives_a_stale_write() {
        let base = revision(with(vec![mic()]), 4);
        let current = revision(with(vec![mic(), game()]), 5);
        let mut incoming = base.clone();
        incoming.watermark = !base.watermark;

        let merged = merge_stale(&base, &incoming, &current);
        assert_eq!(ids(&merged), ["mic", "game"]);
        assert_eq!(merged.watermark, !base.watermark);
    }

    #[test]
    fn a_source_removed_in_the_window_stays_removed() {
        let base = revision(with(vec![mic(), game()]), 4);
        // Changed elsewhere meanwhile — removing it still wins.
        let current = revision(with(vec![mic(), muted(game())]), 5);
        let incoming = revision(with(vec![mic()]), 4);

        let merged = merge_stale(&base, &incoming, &current);
        assert_eq!(ids(&merged), ["mic"]);
    }

    #[test]
    fn a_source_removed_elsewhere_is_not_brought_back() {
        let base = revision(with(vec![mic(), game()]), 4);
        let current = revision(with(vec![mic()]), 5);
        // The window touched the removed source on its old copy.
        let mut incoming = base.clone();
        incoming.sources[1].gain_db = 3.0;

        let merged = merge_stale(&base, &incoming, &current);
        assert_eq!(ids(&merged), ["mic"]);
    }

    /// The mixer reorders by sending the sources in their new order — that
    /// order is the change, and a mute from elsewhere rides along.
    #[test]
    fn a_reorder_in_the_window_keeps_its_order() {
        let base = revision(with(vec![mic(), game()]), 4);
        let current = revision(with(vec![muted(mic()), game()]), 5);
        let incoming = revision(with(vec![game(), mic()]), 4);

        let merged = merge_stale(&base, &incoming, &current);
        assert_eq!(ids(&merged), ["game", "mic"]);
        assert!(merged.sources[1].muted);
    }

    /// A value both changed: the window's is what the user just did.
    #[test]
    fn a_value_both_changed_takes_the_windows() {
        let base = revision(with(vec![mic()]), 4);
        let mut current = revision(with(vec![mic()]), 5);
        current.sources[0].gain_db = 3.0;
        let mut incoming = base.clone();
        incoming.sources[0].gain_db = -6.0;

        let merged = merge_stale(&base, &incoming, &current);
        assert_eq!(merged.sources[0].gain_db, -6.0);
    }

    #[test]
    fn a_write_on_the_current_revision_replaces_and_counts_up() {
        let current = revision(with(vec![muted(mic())]), 7);
        let history = VecDeque::from([current.clone()]);
        let mut incoming = revision(with(vec![mic()]), 7);
        incoming.watermark = !current.watermark;

        let accepted = accept(&current, &history, incoming.clone());
        incoming.revision = 8;
        assert_eq!(json(&accepted), json(&incoming));
    }

    #[test]
    fn a_write_from_an_unknown_revision_is_taken_as_it_stands() {
        let current = revision(with(vec![muted(mic())]), 9);
        let history = VecDeque::from([revision(with(vec![mic()]), 8), current.clone()]);
        let incoming = revision(with(vec![mic(), game()]), 2);

        let accepted = accept(&current, &history, incoming.clone());
        assert_eq!(accepted.revision, 10);
        let mut expected = incoming;
        expected.revision = 10;
        assert_eq!(json(&accepted), json(&expected));
    }

    /// A window writes one change at a time, each on the revision of the
    /// answer to the one before: a switch flipped on and straight back off
    /// ends off — B is built on A's answer, so its "off" is a change.
    #[test]
    fn a_switch_flipped_back_on_the_answers_revision_stays_back() {
        let mut history = VecDeque::new();
        let mut core = revision(with(vec![mic()]), 4);
        core.watermark = false;
        remember(&mut history, &core);

        let mut a = core.clone();
        a.watermark = true;
        core = accept(&core, &history, a);
        remember(&mut history, &core);
        assert_eq!(core.revision, 5);

        let mut b = core.clone();
        b.watermark = false;
        core = accept(&core, &history, b);
        assert!(!core.watermark, "the switch came back on");
        assert_eq!(core.revision, 6);
    }

    /// CFG-1 for the console's own rows: the main window turns the mic down,
    /// the console — which built its row before that — mutes it. The console
    /// sends its row with the revision it was built on, and the core puts it
    /// onto that copy before merging: the level stays, the mute lands.
    #[test]
    fn the_main_windows_level_survives_the_consoles_stale_mute() {
        let mut history = VecDeque::new();
        let mut core = revision(with(vec![mic(), game()]), 4);
        remember(&mut history, &core);
        // The console's copy of the mic row, read when it opened.
        let console_row = core.sources[0].clone();

        // The main window turns the mic down.
        let mut main = core.clone();
        main.sources[0].gain_db = -6.0;
        core = accept(&core, &history, main);
        remember(&mut history, &core);

        // The console mutes the mic on its row from revision 4.
        let incoming = with_sources(&core, &history, Some(4), |sources| {
            put_source(sources, muted(console_row))
        });
        core = accept(&core, &history, incoming);

        assert!(core.sources[0].muted, "the console's mute was lost");
        assert_eq!(core.sources[0].gain_db, -6.0, "the main window's level came undone");
        assert_eq!(ids(&core), ["mic", "game"]);
        assert_eq!(core.revision, 6);
    }

    /// No revision, or one the history no longer holds: the row goes onto the
    /// config as it stands now, the way the source commands always worked.
    #[test]
    fn a_source_row_without_a_known_revision_goes_onto_the_current_config() {
        let mut history = VecDeque::new();
        let mut core = revision(with(vec![mic(), game()]), 9);
        core.sources[1].gain_db = 3.0;
        remember(&mut history, &core);
        for base in [None, Some(2)] {
            let incoming = with_sources(&core, &history, base, |sources| put_source(sources, muted(mic())));
            let accepted = accept(&core, &history, incoming);
            assert!(accepted.sources[0].muted);
            assert_eq!(accepted.sources[1].gain_db, 3.0);
        }
    }

    /// A source removed in the console on an old copy goes, and what changed
    /// elsewhere on the others stays.
    #[test]
    fn a_source_removed_on_an_old_revision_keeps_the_others_changes() {
        let mut history = VecDeque::new();
        let mut core = revision(with(vec![mic(), game()]), 4);
        remember(&mut history, &core);
        let mut main = core.clone();
        main.sources[1] = muted(game());
        core = accept(&core, &history, main);
        remember(&mut history, &core);

        let incoming = with_sources(&core, &history, Some(4), |sources| sources.retain(|s| s.id != "mic"));
        core = accept(&core, &history, incoming);
        assert_eq!(ids(&core), ["game"]);
        assert!(core.sources[0].muted);
    }

    #[test]
    fn the_history_keeps_the_last_sixteen() {
        let mut history = VecDeque::new();
        for n in 0..20 {
            remember(&mut history, &revision(default_config(), n));
        }
        assert_eq!(history.len(), 16);
        assert_eq!(history.front().unwrap().revision, 4);
        assert_eq!(history.back().unwrap().revision, 19);
    }

    /// Final review (state M4, frontend I1): the main window's stand-in
    /// config is revision 0. Written before the real one arrived, it replaced
    /// the user's sources, hotkeys and settings whole.
    #[test]
    fn a_write_built_on_no_settings_read_changes_nothing() {
        let current = revision(with(vec![muted(mic())]), FIRST_REVISION);
        let history = VecDeque::from([current.clone()]);
        let mut stand_in = revision(with(vec![mic(), game()]), 0);
        stand_in.watermark = !current.watermark;

        let accepted = accept(&current, &history, stand_in);
        assert_eq!(accepted.revision, FIRST_REVISION + 1, "still counts up, so the window hears back");
        let mut expected = current.clone();
        expected.revision = FIRST_REVISION + 1;
        assert_eq!(json(&accepted), json(&expected));

        let row = with_sources(&current, &history, Some(0), |sources| sources.push(game()));
        assert_eq!(json(&row), json(&current), "a source row from it is no change either");
        assert!(!unread(0, &revision(with(vec![]), 0)), "a core that never loaded is at 0 itself");
    }

    /// A config written before the revision existed loads at revision 0.
    #[test]
    fn a_configuration_without_a_revision_loads_at_zero() {
        let mut written = json(&default_config());
        written.as_object_mut().unwrap().remove("revision").expect("called 'revision'");
        let parsed: AppConfig = serde_json::from_value(written).unwrap();
        assert_eq!(parsed.revision, 0);
    }

    /// What the banner says, one line per case.
    #[test]
    fn each_recovery_says_what_happened() {
        assert_eq!(Loaded::Fine.message(), None);
        let aside = Some(PathBuf::from(r"C:\x\config.json.broken-5"));
        let restored = Loaded::Recovered { aside: aside.clone(), from_backup: true, reason: "x".into() };
        assert_eq!(
            restored.message().unwrap(),
            "The settings file was damaged. ClippiBoy restored the copy from the last good start; the damaged file is kept as config.json.broken-5."
        );
        let lost = Loaded::Recovered { aside, from_backup: false, reason: "x".into() };
        assert_eq!(
            lost.message().unwrap(),
            "The settings file was damaged and no good copy was left. ClippiBoy starts with default settings; the damaged file is kept as config.json.broken-5."
        );
        let held = Loaded::Recovered { aside: None, from_backup: true, reason: "in use".into() };
        assert_eq!(
            held.message().unwrap(),
            "The settings file could not be opened (in use). ClippiBoy runs on the last good copy for now and leaves the file alone until the next start."
        );
        let held = Loaded::Recovered { aside: None, from_backup: false, reason: "in use".into() };
        assert_eq!(
            held.message().unwrap(),
            "The settings file could not be opened (in use). ClippiBoy runs on default settings for now and leaves the file alone until the next start."
        );
    }

    #[test]
    fn every_write_recomputes_the_bitrate_and_bounds_the_keyframe_interval() {
        let mut rec = default_config().recording;
        rec.width = 1280;
        rec.height = 720;
        rec.fps = 30;
        rec.bitrate_kbps = 99_000;
        rec.keyframe_seconds = 60;
        normalize_recording(&mut rec);
        assert_eq!(rec.bitrate_kbps, encode::bitrate_for(1280, 720, 30));
        assert_eq!(rec.keyframe_seconds, 10);
        rec.keyframe_seconds = 0;
        normalize_recording(&mut rec);
        assert_eq!(rec.keyframe_seconds, 1);
    }
}
