//! Self-update via GitHub Releases.
//!
//! The app asks at startup and then every hour whether a newer version has been
//! published, but does not download it by itself: installing on Windows means the process is ended
//! and the installer started — in the middle of a recording that would be
//! exactly the wrong thing. So ClippiBoy only reports the find — a notice in the
//! window — and the user decides when to install.
//!
//! The updater checks authenticity itself: every package is signed with the
//! private key, the public one sits in `tauri.conf.json`. What the signature
//! does not cover is the version `latest.json` claims, so ClippiBoy holds it
//! against the version the signature names before it installs ([`vet`]).

use std::cmp::Ordering;
use std::time::Duration;

use parking_lot::Mutex;
use tauri::Manager;
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::state::AppState;

/// The most recent find that has not been installed yet.
#[derive(Default)]
pub struct Pending(Mutex<Option<Update>>);

/// What the UI needs to know about an update.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    pub notes: Option<String>,
    pub date: Option<String>,
}

impl UpdateInfo {
    fn of(update: &Update) -> Self {
        Self {
            version: update.version.clone(),
            current_version: update.current_version.clone(),
            notes: update.body.clone(),
            date: update.date.map(|date| date.to_string()),
        }
    }
}

/// How far the download of an update has come.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgress {
    pub downloaded: u64,
    /// `None` when the server does not say how large the package is.
    pub total: Option<u64>,
}

/// How often the download reports its progress to the UI at most.
const PROGRESS_EVERY: std::time::Duration = std::time::Duration::from_millis(150);

/// How often to look again after the first check.
///
/// ClippiBoy starts with Windows and then runs for days. Looking only at startup
/// meant a release published after the boot stayed unseen until the next one —
/// in practice, never.
const INTERVAL: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// The check is a few hundred bytes of JSON; past this it hangs, and the
/// hourly round waited on it for good.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// Reaching GitHub, for the check and the download alike.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// The download takes as long as the line needs, but may not sit without a
/// byte for longer than this.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);

/// The update found last, if it has not been installed yet.
///
/// The window asks for this when it starts. The event alone was not enough:
/// the check ran at startup, while the window still sat hidden in the tray, and
/// the only listener lived in the settings page — so the find went to nobody.
pub fn pending(app: &tauri::AppHandle) -> Option<UpdateInfo> {
    app.state::<Pending>().0.lock().as_ref().map(UpdateInfo::of)
}

/// Look whether there is something newer.
pub async fn check(app: &tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    let updater = app
        .updater_builder()
        // No overall limit here: it would carry over to a download of tens
        // of megabytes on a slow line. The check gets its own below.
        .configure_client(|client| client.connect_timeout(CONNECT_TIMEOUT).read_timeout(STALL_TIMEOUT))
        // End the recording cleanly before the restart, otherwise half-finished
        // segment files are left behind.
        .on_before_exit({
            let app = app.clone();
            move || {
                // A save or an edit cut off by the restart loses its clip: none
                // starts from here on, and a running one is waited for.
                let state = app.state::<AppState>();
                // Only a flag set here is the update's to take back (`install`);
                // one from "Quit" stays.
                if !state.quitting.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    QUIT_FOR_UPDATE.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                crate::wait_for_saves(&app);
                state.abandon_recording();
                state.stop_pipeline();
                // The installer runs next: the handshake must not outlive the app.
                crate::control::stop();
            }
        })
        .build()
        .map_err(|err| format!("update check not possible: {err}"))?;

    let found = tokio::time::timeout(CHECK_TIMEOUT, updater.check())
        .await
        .map_err(|_| "update check failed: no answer in time".to_string())?
        .map_err(|err| format!("update check failed: {err}"))?;

    let pending = app.state::<Pending>();
    match found {
        Some(update) => {
            match vet(&running(app), &update.version, &update.signature) {
                Ok(Signed::AsOffered) => {}
                Ok(Signed::Unnamed) => {
                    if first_note(&format!("unnamed {}", update.version)) {
                        log::warn!(
                            "update {}: its signature names no version, taken on the version floor alone",
                            update.version
                        );
                    }
                }
                Err(why) => {
                    *pending.0.lock() = None;
                    if first_note(&format!("refused {}", update.version)) {
                        log::warn!("update {}{REFUSED}{why}", update.version);
                    }
                    return Err(format!("update {}{REFUSED}{why}", update.version));
                }
            }
            let info = UpdateInfo::of(&update);
            *pending.0.lock() = Some(update);
            Ok(Some(info))
        }
        None => {
            *pending.0.lock() = None;
            Ok(None)
        }
    }
}

/// Download and install the version that was found.
///
/// The app does not come back afterwards: on Windows the installer starts and
/// ends the running process.
pub async fn install(app: &tauri::AppHandle) -> Result<(), String> {
    let update = app.state::<Pending>().0.lock().clone();
    let Some(update) = update else {
        return Err("No verified update is ready.".into());
    };
    // The check vetted it already; this is the last word before the download.
    vet(&running(app), &update.version, &update.signature)
        .map_err(|why| format!("could not install the update: {why}"))?;

    // The package is some tens of megabytes; without word on how far along it
    // is, the button just sat on "Installing …" for a minute. Chunks come in a
    // few kilobytes at a time, so the UI hears about it only now and then.
    let mut downloaded: u64 = 0;
    let mut last_emit: Option<std::time::Instant> = None;
    update
        .download_and_install(
            |chunk, total| {
                downloaded += chunk as u64;
                let done = total.is_some_and(|total| downloaded >= total);
                if done || last_emit.map_or(true, |at| at.elapsed() >= PROGRESS_EVERY) {
                    last_emit = Some(std::time::Instant::now());
                    let _ = tauri::Emitter::emit(
                        app,
                        "update-progress",
                        UpdateProgress { downloaded, total },
                    );
                }
            },
            || {
                let _ = tauri::Emitter::emit(app, "update-downloaded", ());
            },
        )
        .await
        .map_err(|err| {
            // The installer never took over, so ClippiBoy keeps running: with
            // `quitting` left set it would refuse every save and no longer
            // restart a failed encoder. The buffer the hook stopped stays
            // stopped until it is started again (or auto start does it).
            if QUIT_FOR_UPDATE.swap(false, std::sync::atomic::Ordering::SeqCst) {
                app.state::<AppState>()
                    .quitting
                    .store(false, std::sync::atomic::Ordering::SeqCst);
                // The wait for saves told the start-up passes over the
                // library to stop. ClippiBoy goes on, so one still under way
                // goes on too instead of ending at its next clip.
                crate::clips::STOP_PASSES.store(false, std::sync::atomic::Ordering::SeqCst);
                // The hook took the control port down with the handshake file.
                crate::control::start(&app);
            }
            format!("could not install the update: {err}")
        })?;
    Ok(())
}

/// The version of the app that is running.
fn running(app: &tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Between the version and the reason in a refusal; the hourly round leaves
/// those to the one warning [`check`] gives per version.
const REFUSED: &str = " refused: ";

/// Notes already logged, so a refusal shows once per version and not hourly.
static NOTED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// True the first time `note` comes by.
fn first_note(note: &str) -> bool {
    let mut noted = NOTED.lock();
    if noted.iter().any(|seen| seen == note) {
        return false;
    }
    noted.push(note.to_owned());
    true
}

/// What the signature said about the version.
#[derive(Debug, PartialEq)]
enum Signed {
    /// It names the version offered.
    AsOffered,
    /// It names none that could be read.
    Unnamed,
}

/// Whether an update may be installed: its version must be above the running
/// one, and a version the signature's trusted comment names must be that same
/// one. The signature covers the package and that comment, not `latest.json` —
/// without this an older package, validly signed, could come back in as
/// "newer" under a made-up version. The comment is only read here; the updater
/// checks it is the key's own once the package is in, and refuses the install
/// otherwise.
///
/// A comment with no readable version is let through on the version floor
/// alone: a change in how releases are named must not leave every installed
/// copy unable to update.
fn vet(running: &str, offered: &str, signature: &str) -> Result<Signed, String> {
    let (Some(now), Some(new)) = (Version::parse(running), Version::parse(offered)) else {
        return Err(format!("{offered:?} cannot be compared with {running}"));
    };
    if new.precedence(&now) != Ordering::Greater {
        return Err(format!("{offered} is not newer than the running {running}"));
    }
    let Some(signed) = signed_version(signature) else {
        return Ok(Signed::Unnamed);
    };
    if Version::parse(&signed).is_none_or(|signed| signed.precedence(&new) != Ordering::Equal) {
        return Err(format!("its signature is for {signed}, not {offered}"));
    }
    Ok(Signed::AsOffered)
}

/// The version in the installer's name, which the bundler writes into the
/// trusted comment of the minisign signature (`timestamp:…\tfile:ClippiBoy_1.0.1_x64-setup.exe`).
/// The signature comes as `latest.json` carries it: the `.sig` file in base64.
fn signed_version(signature: &str) -> Option<String> {
    let url_safe = signature.trim_end_matches('=').replace('+', "-").replace('/', "_");
    let text = String::from_utf8(crate::webhook::from_base64url(&url_safe)?).ok()?;
    let comment = text.lines().find_map(|line| line.strip_prefix("trusted comment: "))?;
    let file = comment.split('\t').find_map(|field| field.strip_prefix("file:"))?;
    // `<product>_<version>_<arch>-setup.exe`, or `…_<arch>_<lang>.msi`; the
    // first part that is a version, so a product name with `_` in it works.
    file.split('_').skip(1).find(|part| Version::parse(part).is_some()).map(str::to_owned)
}

/// A semantic version, as far as ordering goes: build metadata is dropped.
struct Version<'a> {
    core: [u64; 3],
    pre: Vec<&'a str>,
}

impl<'a> Version<'a> {
    fn parse(text: &'a str) -> Option<Self> {
        let text = text.split('+').next()?;
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut parts = core.split('.');
        let mut numbers = [0u64; 3];
        for number in &mut numbers {
            let part = parts.next().filter(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))?;
            *number = part.parse().ok()?;
        }
        if parts.next().is_some() {
            return None;
        }
        let pre: Vec<&str> = pre.map(|pre| pre.split('.').collect()).unwrap_or_default();
        if pre.iter().any(|id| id.is_empty()) {
            return None;
        }
        Some(Self { core: numbers, pre })
    }

    /// Semver precedence: a pre-release ranks below its release, and its
    /// identifiers compare as numbers where both are numbers.
    fn precedence(&self, other: &Self) -> Ordering {
        self.core.cmp(&other.core).then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                for (a, b) in self.pre.iter().zip(&other.pre) {
                    let numeric = |id: &str| id.bytes().all(|b| b.is_ascii_digit()).then(|| id.parse::<u64>().ok()).flatten();
                    let order = match (numeric(a), numeric(b)) {
                        (Some(a), Some(b)) => a.cmp(&b),
                        (Some(_), None) => Ordering::Less,
                        (None, Some(_)) => Ordering::Greater,
                        (None, None) => a.cmp(b),
                    };
                    if order != Ordering::Equal {
                        return order;
                    }
                }
                self.pre.len().cmp(&other.pre.len())
            }
        })
    }
}

/// Set when the update's exit hook started the quit, so a failed install
/// knows to call it off again.
static QUIT_FOR_UPDATE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Look at startup and then every [`INTERVAL`], and report a find to the UI.
///
/// Every find goes out as an event — a window opened since the last one learns
/// of it through [`pending`] instead. The log mentions a version once, not once
/// an hour.
pub fn watch(app: &tauri::AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let mut announced: Option<String> = None;
        loop {
            match tauri::async_runtime::block_on(check(&app)) {
                Ok(Some(info)) => {
                    if announced.as_deref() != Some(info.version.as_str()) {
                        log::info!("update {} available", info.version);
                        announced = Some(info.version.clone());
                    }
                    let _ = tauri::Emitter::emit(&app, "update-available", info);
                }
                Ok(None) => {
                    if announced.is_none() {
                        log::info!("no update available");
                    }
                }
                // With no network or no release this is not an error the user
                // needs to see; the next round tries again.
                // A refusal was logged once by `check`.
                Err(err) if err.contains(REFUSED) => {}
                Err(err) => log::info!("{err}"),
            }
            std::thread::sleep(INTERVAL);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The signature `latest.json` carries for 1.0.0, as published.
    const SIGNED_1_0_0: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVUTzAzcmFvak1HUHZralYrY2ZhTGlYN0Nnc1pLNW8rSS9KYUtocUVRYkNWdUhoYjVrbkxGVGtZNlBSL3I1MzlLbmVUVXJ1ZXRwZ2kwdnFFaUZ5a1FOL2hDcDgvWElkdFFRPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk1MzgxMTQzCWZpbGU6Q2xpcHBpQm95XzEuMC4wX3g2NC1zZXR1cC5leGUKRkFyTkJIbDdySzgvNVNQL2E2WDBNajZJYjhKdVRJeWdXektDTDUzaDB4OXRSTktMNDZyMUVzSWQ2N0hDNU1adnUrRFBsU2k5RTZmMTVYVmZLWXRFQ1E9PQo=";

    fn signature(trusted: &str) -> String {
        crate::appicon::base64(
            format!("untrusted comment: signature from tauri secret key\nRUTO\ntrusted comment: {trusted}\nFArN\n").as_bytes(),
        )
    }

    fn newer(offered: &str, running: &str) -> Option<bool> {
        Some(Version::parse(offered)?.precedence(&Version::parse(running)?) == Ordering::Greater)
    }

    #[test]
    fn versions_order_by_semver() {
        assert_eq!(newer("1.0.1", "1.0.0"), Some(true));
        assert_eq!(newer("1.10.0", "1.9.0"), Some(true));
        assert_eq!(newer("2.0.0", "1.99.99"), Some(true));
        assert_eq!(newer("1.0.0", "1.0.0"), Some(false));
        assert_eq!(newer("0.9.6", "1.0.0"), Some(false));
        assert_eq!(newer("1.0.0+build.7", "1.0.0"), Some(false));
        // Pre-releases rank below their release, numbers as numbers.
        assert_eq!(newer("1.0.0", "1.0.0-rc.1"), Some(true));
        assert_eq!(newer("1.0.0-rc.1", "1.0.0"), Some(false));
        assert_eq!(newer("1.0.0-rc.10", "1.0.0-rc.2"), Some(true));
        assert_eq!(newer("1.0.0-beta", "1.0.0-alpha"), Some(true));
        assert_eq!(newer("1.0.0-alpha.1", "1.0.0-alpha"), Some(true));
        assert_eq!(newer("1.0.0-alpha", "1.0.0-1"), Some(true));
        // Not a version.
        for junk in ["", "1", "1.0", "1.0.0.0", "v1.0.0", "1.x.0", "1.0.0-", "1.0.0-a..b", "+1.0.0", "1.-1.0"] {
            assert_eq!(Version::parse(junk).map(|_| ()), None, "{junk}");
        }
    }

    #[test]
    fn the_signature_names_its_version() {
        assert_eq!(signed_version(SIGNED_1_0_0).as_deref(), Some("1.0.0"));
        assert_eq!(
            signed_version(&signature("timestamp:1\tfile:ClippiBoy_1.2.3-rc.1_x64_en-US.msi")).as_deref(),
            Some("1.2.3-rc.1")
        );
        assert_eq!(signed_version(&signature("timestamp:1")), None);
        assert_eq!(signed_version(&signature("timestamp:1\tfile:ClippiBoy-setup.exe")), None);
        assert_eq!(signed_version(&signature("timestamp:1\tfile:ClippiBoy_latest_x64-setup.exe")), None);
        // An underscore in the product name.
        assert_eq!(
            signed_version(&signature("timestamp:1\tfile:Clippi_Boy_1.0.2_x64-setup.exe")).as_deref(),
            Some("1.0.2")
        );
        assert_eq!(signed_version("not base64 at all!"), None);
        assert_eq!(signed_version(""), None);
    }

    #[test]
    fn only_a_newer_version_signed_as_such_is_installed() {
        assert_eq!(vet("0.9.6", "1.0.0", SIGNED_1_0_0), Ok(Signed::AsOffered));
        // The same or an older version, whatever the signature says.
        assert!(vet("1.0.0", "1.0.0", SIGNED_1_0_0).is_err());
        assert!(vet("1.0.1", "1.0.0", SIGNED_1_0_0).is_err());
        // An older package under a made-up version.
        assert!(vet("1.0.0", "1.0.1", SIGNED_1_0_0).is_err());
        assert!(vet("1.0.0", "9.9.9", SIGNED_1_0_0).is_err());
        assert!(vet("0.9.6", "1.0.2", SIGNED_1_0_0).is_err());
        assert!(vet("1.0.0", "1.0.1", &signature("timestamp:1\tfile:ClippiBoy_1.0.0_x64-setup.exe")).is_err());
        assert_eq!(
            vet("1.0.0", "1.0.2", &signature("timestamp:1\tfile:Clippi_Boy_1.0.2_x64-setup.exe")),
            Ok(Signed::AsOffered)
        );
        // A signature that names no version is taken on the floor alone.
        assert_eq!(vet("1.0.0", "1.0.1", &signature("timestamp:1")), Ok(Signed::Unnamed));
        assert_eq!(vet("1.0.0", "1.0.1", ""), Ok(Signed::Unnamed));
        assert!(vet("1.0.1", "1.0.1", &signature("timestamp:1")).is_err());
        // No versions to compare.
        assert!(vet("1.0.0", "latest", SIGNED_1_0_0).is_err());
        assert!(vet("dev", "1.0.0", SIGNED_1_0_0).is_err());
        // Build metadata does not make a version another one.
        assert_eq!(vet("0.9.6", "1.0.0+ci.4", SIGNED_1_0_0), Ok(Signed::AsOffered));
    }

    #[test]
    fn a_note_is_logged_once() {
        assert!(first_note("test refused 0.0.1"));
        assert!(!first_note("test refused 0.0.1"));
        assert!(first_note("test refused 0.0.2"));
    }
}
