//! Anonymous usage stats: one ping a day with a random id, the version, where
//! the download came from and a few counts. No account, no content, no address
//! — see "Usage statistics" on clippiboy.com/privacy.html.
//!
//! Counts live in memory and go to `usage.json` at most once a minute, so a
//! crash costs a minute at most. A ping that does not get through leaves them
//! where they are for the next try. Switching it off deletes the file; on again
//! starts with a new id. Until someone has said yes (`usage_stats_asked`)
//! nothing is counted, kept or sent — the switch alone is only the default.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::Manager;

const FILE: &str = "usage.json";
/// Written by the installer (`windows/hooks.nsh`): its own file name.
const SOURCE_FILE: &str = "install-source.txt";
/// At most one ping in this long. The server files it under its own day.
const EVERY_SECS: u64 = 20 * 60 * 60;
const HEARD: [&str; 6] = ["tiktok", "youtube", "discord", "friend", "reddit", "other"];

/// Bits of `Counts::features`; the server keeps the same numbering.
#[derive(Clone, Copy, Debug)]
pub enum Feature {
    Console = 0,
    Friends = 1,
    SendFriend = 2,
    ShareLink = 3,
    Editor = 4,
    StreamDeck = 5,
}

#[derive(Clone, Copy, Debug)]
enum Event {
    Clip,
    Shot,
    Recording,
    Used(Feature),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Counts {
    clips: u32,
    shots: u32,
    recordings: u32,
    features: u8,
}

impl Counts {
    /// Take off what a ping carried; whatever came in meanwhile stays.
    fn sent(&mut self, what: &Counts) {
        self.clips = self.clips.saturating_sub(what.clips);
        self.shots = self.shots.saturating_sub(what.shots);
        self.recordings = self.recordings.saturating_sub(what.recordings);
        self.features &= !what.features;
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct UsageFile {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    heard: Option<String>,
    #[serde(default)]
    pending: Counts,
    /// Unix seconds of the last ping that got through.
    #[serde(default)]
    last_sent: u64,
}

#[derive(Default)]
struct Usage {
    file: UsageFile,
    dir: Option<PathBuf>,
    enabled: bool,
    dirty: bool,
}

impl Usage {
    fn open(&mut self, dir: &Path, enabled: bool) {
        self.dir = Some(dir.to_path_buf());
        self.enabled = enabled;
        self.file = if enabled {
            std::fs::read(dir.join(FILE))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default()
        } else {
            UsageFile::default()
        };
    }

    fn count(&mut self, event: Event) {
        if !self.enabled {
            return;
        }
        let pending = &mut self.file.pending;
        match event {
            Event::Clip => pending.clips = pending.clips.saturating_add(1),
            Event::Shot => pending.shots = pending.shots.saturating_add(1),
            Event::Recording => pending.recordings = pending.recordings.saturating_add(1),
            Event::Used(feature) => pending.features |= 1 << feature as u8,
        }
        self.dirty = true;
    }

    /// The id (made on first need) and the counts a ping would carry now.
    fn snapshot(&mut self) -> (String, Counts) {
        if self.file.id.is_none() {
            self.file.id = Some(uuid::Uuid::new_v4().to_string());
            self.dirty = true;
        }
        (self.file.id.clone().unwrap_or_default(), self.file.pending)
    }

    fn sent(&mut self, what: &Counts, now: u64) {
        self.file.pending.sent(what);
        self.file.last_sent = now;
        self.dirty = true;
    }

    fn set_heard(&mut self, heard: Option<String>) {
        if !self.enabled {
            return;
        }
        self.file.heard = heard.filter(|h| HEARD.contains(&h.as_str()));
        self.dirty = true;
    }

    fn set_enabled(&mut self, on: bool) {
        if on == self.enabled {
            return;
        }
        self.enabled = on;
        self.file = UsageFile::default();
        self.dirty = false;
        if !on {
            if let Some(dir) = &self.dir {
                let _ = std::fs::remove_file(dir.join(FILE));
            }
        }
    }

    fn flush(&mut self) {
        if !self.enabled || !self.dirty {
            return;
        }
        let Some(dir) = &self.dir else { return };
        let written = serde_json::to_vec(&self.file)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                std::fs::create_dir_all(dir)?;
                std::fs::write(dir.join(FILE), bytes)
            });
        match written {
            Ok(()) => self.dirty = false,
            Err(err) => log::warn!("could not keep the usage counts: {err}"),
        }
    }
}

/// `asked`: someone has said yes (or no) — before that, `enabled` is only the
/// default and nothing goes out.
fn due(file: &UsageFile, enabled: bool, asked: bool, now: u64) -> bool {
    enabled && asked && now.saturating_sub(file.last_sent) >= EVERY_SECS
}

/// `ClippiBoy_0.8.3_x64-setup.tiktok1.exe` → `tiktok1`. Copes with Windows'
/// ` (1)` for a second download; anything else is no source.
pub fn source_from_file_name(name: &str) -> Option<String> {
    let lower = name.trim().to_ascii_lowercase();
    let mut stem = lower.strip_suffix(".exe")?;
    if let Some(open) = stem.rfind(" (") {
        let inner = &stem[open + 2..];
        if inner.len() > 1 && inner.ends_with(')') && inner[..inner.len() - 1].bytes().all(|b| b.is_ascii_digit()) {
            stem = &stem[..open];
        }
    }
    let (_, source) = stem.rsplit_once("-setup.")?;
    let valid = (1..=32).contains(&source.len())
        && source.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    valid.then(|| source.to_string())
}

static USAGE: LazyLock<Mutex<Usage>> = LazyLock::new(|| Mutex::new(Usage::default()));

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn clip() {
    USAGE.lock().count(Event::Clip);
}

pub fn shot() {
    USAGE.lock().count(Event::Shot);
}

pub fn recording() {
    USAGE.lock().count(Event::Recording);
}

pub fn used(feature: Feature) {
    USAGE.lock().count(Event::Used(feature));
}

pub fn flush() {
    USAGE.lock().flush();
}

/// On only once someone said yes: the switch's default alone counts nothing.
fn consented(config: &crate::model::AppConfig) -> bool {
    config.usage_stats && config.usage_stats_asked
}

/// Called with every new config; switching off forgets everything at once.
pub fn on_config(config: &crate::model::AppConfig) {
    USAGE.lock().set_enabled(consented(config));
}

fn read_source(dir: &Path) -> Option<String> {
    let name = std::fs::read_to_string(dir.join(SOURCE_FILE)).ok()?;
    source_from_file_name(&name)
}

fn client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        crate::friends::install_crypto();
        reqwest::Client::builder()
            .user_agent(concat!("ClippiBoy/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(15))
            .build()
            .expect("HTTP client")
    })
}

/// One ping if one is due. Deliberately without the friends sign-in: nothing
/// ties it to an account.
fn send_if_due(app: &tauri::AppHandle) {
    let (enabled, asked) = {
        let config = app.state::<crate::state::AppState>();
        let config = config.config.lock();
        (config.usage_stats, config.usage_stats_asked)
    };
    let (id, counts, heard, dir) = {
        let mut usage = USAGE.lock();
        if !due(&usage.file, enabled, asked, now()) {
            return;
        }
        let (id, counts) = usage.snapshot();
        usage.flush();
        (id, counts, usage.file.heard.clone(), usage.dir.clone())
    };
    let body = json!({
        "id": id,
        "version": env!("CARGO_PKG_VERSION"),
        "source": dir.as_deref().and_then(read_source),
        "heard": heard,
        "clips": counts.clips.min(10_000),
        "shots": counts.shots.min(10_000),
        "recordings": counts.recordings.min(10_000),
        "features": counts.features,
    });
    let url = format!("{}/t/ping", crate::friends::SERVER);
    let outcome = tauri::async_runtime::block_on(async {
        let response = client().post(url).json(&body).send().await.map_err(|err| err.to_string())?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("usage ping answered {}", response.status()))
        }
    });
    match outcome {
        Ok(()) => {
            let mut usage = USAGE.lock();
            // Switched off while it was on its way: nothing to record.
            if usage.enabled && usage.file.id.as_deref() == Some(id.as_str()) {
                usage.sent(&counts, now());
                usage.flush();
            }
        }
        Err(err) => log::info!("usage ping did not go out: {err}"),
    }
}

/// Load the counts and keep them: flushed every minute, a ping looked at two
/// minutes after start (not to compete with it) and then every hour.
pub fn start(app: &tauri::AppHandle) {
    let enabled = consented(&app.state::<crate::state::AppState>().config.lock());
    match app.path().app_local_data_dir() {
        Ok(dir) => USAGE.lock().open(&dir, enabled),
        Err(err) => {
            log::warn!("no place for the usage counts: {err}");
            return;
        }
    }
    let app = app.clone();
    std::thread::spawn(move || {
        for minute in 1u64.. {
            std::thread::sleep(Duration::from_secs(60));
            flush();
            if minute % 60 == 2 {
                send_if_due(&app);
            }
        }
    });
}

/// For the setup: the source the installer's name carried, if any.
#[tauri::command]
pub fn usage_source(app: tauri::AppHandle) -> Option<String> {
    read_source(&app.path().app_local_data_dir().ok()?)
}

/// For the setup: the answer to "Where did you hear about ClippiBoy?".
#[tauri::command]
pub fn usage_set_heard(heard: Option<String>) {
    let mut usage = USAGE.lock();
    usage.set_heard(heard);
    usage.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("clippiboy-usage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn usage_in(dir: &Path) -> Usage {
        let mut usage = Usage::default();
        usage.open(dir, true);
        usage
    }

    #[test]
    fn the_source_comes_out_of_the_installer_name() {
        let cases = [
            ("ClippiBoy_0.8.3_x64-setup.tiktok1.exe", Some("tiktok1")),
            ("ClippiBoy_0.8.3_x64-setup.tiktok1 (1).exe", Some("tiktok1")),
            ("CLIPPIBOY_0.8.3_X64-SETUP.TikTok1.EXE", Some("tiktok1")),
            ("ClippiBoy_0.8.3_x64-setup.site.exe\r\n", Some("site")),
        ];
        for (name, want) in cases {
            assert_eq!(source_from_file_name(name).as_deref(), want, "{name}");
        }
    }

    #[test]
    fn no_source_without_a_ref() {
        let long = format!("ClippiBoy_0.8.3_x64-setup.{}.exe", "a".repeat(33));
        let cases = [
            "ClippiBoy_0.8.3_x64-setup.exe",
            "ClippiBoy_0.8.3_x64-setup (1).exe",
            "ClippiBoy_0.8.3_x64-setup.a_b.exe",
            "ClippiBoy_0.8.3_x64-setup..exe",
            "something.exe",
            "",
            long.as_str(),
        ];
        for name in cases {
            assert_eq!(source_from_file_name(name), None, "{name}");
        }
    }

    #[test]
    fn a_failed_send_keeps_the_counts() {
        let dir = temp_dir();
        let mut usage = usage_in(&dir);
        for _ in 0..3 {
            usage.count(Event::Clip);
        }
        usage.count(Event::Shot);
        usage.count(Event::Used(Feature::Console));
        usage.count(Event::Used(Feature::ShareLink));
        assert_eq!(usage.file.pending, Counts { clips: 3, shots: 1, recordings: 0, features: 0b1001 });

        // Taken for a send that then fails: nothing is lost, the two clips
        // counted meanwhile add on.
        let (_, first) = usage.snapshot();
        usage.count(Event::Clip);
        usage.count(Event::Clip);
        assert_eq!(usage.file.pending, Counts { clips: 5, shots: 1, recordings: 0, features: 0b1001 });

        // The next one gets through; what came in during it stays for later.
        let (_, second) = usage.snapshot();
        assert_ne!(first, second);
        usage.count(Event::Clip);
        usage.count(Event::Used(Feature::Friends));
        usage.sent(&second, 1_000);
        assert_eq!(usage.file.pending, Counts { clips: 1, shots: 0, recordings: 0, features: 0b0010 });
        assert_eq!(usage.file.last_sent, 1_000);

        // And all of it survives a restart.
        usage.flush();
        let again = usage_in(&dir);
        assert_eq!(again.file.pending, usage.file.pending);
        assert_eq!(again.file.last_sent, 1_000);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_id_is_made_once_and_kept() {
        let dir = temp_dir();
        let mut usage = usage_in(&dir);
        let (id, _) = usage.snapshot();
        assert!(uuid::Uuid::parse_str(&id).is_ok());
        assert_eq!(usage.snapshot().0, id);
        usage.flush();
        assert_eq!(usage_in(&dir).snapshot().0, id);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn nothing_goes_out_before_someone_said_yes() {
        let hour = 3600;
        let file = UsageFile { last_sent: 100 * hour, ..Default::default() };
        assert!(!due(&file, true, false, 200 * hour));
        assert!(due(&file, true, true, 120 * hour));
        assert!(!due(&file, true, true, 119 * hour));
        assert!(!due(&file, false, true, 200 * hour));
    }

    #[test]
    fn switching_off_forgets_everything() {
        let dir = temp_dir();
        let mut usage = usage_in(&dir);
        usage.count(Event::Clip);
        usage.snapshot();
        usage.set_heard(Some("discord".into()));
        usage.flush();
        assert!(dir.join(FILE).exists());

        usage.set_enabled(false);
        assert!(!dir.join(FILE).exists());
        usage.count(Event::Clip);
        usage.flush();
        assert!(!dir.join(FILE).exists());
        assert_eq!(usage.file.pending, Counts::default());

        // On again: a new id, nothing carried over.
        usage.set_enabled(true);
        assert_eq!(usage.file.id, None);
        assert_eq!(usage.file.heard, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_known_answers_are_kept() {
        let dir = temp_dir();
        let mut usage = usage_in(&dir);
        usage.set_heard(Some("myspace".into()));
        assert_eq!(usage.file.heard, None);
        usage.set_heard(Some("friend".into()));
        assert_eq!(usage.file.heard.as_deref(), Some("friend"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
