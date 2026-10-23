//! Persistent configuration, stored as JSON next to the app database.

use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use crate::encode;
use crate::model::{
    AppConfig, BufferConfig, ControlConfig, OverlayConfig, RecordingConfig, SourceKind, TargetKind,
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
    let config = match read {
        Some(mut config) => {
            // Check the encoder against the hardware actually present.
            config.recording.encoder = encode::resolve(config.recording.encoder);
            // The bitrate is derived from the picture, not chosen by hand —
            // a config from when it was a slider carries a number that means
            // nothing now (40 Mbit/s at any resolution, as it happens). It
            // would otherwise still size the memory budget.
            config.recording.bitrate_kbps = encode::bitrate_for(
                config.recording.width,
                config.recording.height,
                config.recording.fps,
            );
            migrate_sources(&mut config);
            config
        }
        None => default_config(),
    };
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
fn write_synced(path: &Path, text: &str) -> io::Result<()> {
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
/// commands (`set_clip_dir`, `regenerate_token`). A settings write — from a
/// page holding stale or mock values — carries whatever it likes in them: a
/// folder that grants a whole drive, "mock-token" for the Stream Deck's key.
pub fn keep_core_owned(next: &mut AppConfig, previous: &AppConfig) {
    next.clip_dir = previous.clip_dir.clone();
    next.control.token = previous.control.token.clone();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AudioSource;

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
        // Nothing generated yet — `control::start` fills it in on first use.
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
}
