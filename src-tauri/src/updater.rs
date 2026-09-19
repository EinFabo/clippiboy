//! Self-update via GitHub Releases.
//!
//! The app asks at startup and then every hour whether a newer version has been
//! published, but does not download it by itself: installing on Windows means the process is ended
//! and the installer started — in the middle of a recording that would be
//! exactly the wrong thing. So ClippiBoy only reports the find — a notice in the
//! window — and the user decides when to install.
//!
//! The updater checks authenticity itself: every package is signed with the
//! private key, the public one sits in `tauri.conf.json`.

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
        // End the recording cleanly before the restart, otherwise half-finished
        // segment files are left behind.
        .on_before_exit({
            let app = app.clone();
            move || app.state::<AppState>().stop_pipeline()
        })
        .build()
        .map_err(|err| format!("update check not possible: {err}"))?;

    let found = updater
        .check()
        .await
        .map_err(|err| format!("update check failed: {err}"))?;

    let pending = app.state::<Pending>();
    match found {
        Some(update) => {
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
        .map_err(|err| format!("could not install the update: {err}"))?;
    Ok(())
}

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
                Err(err) => log::info!("{err}"),
            }
            std::thread::sleep(INTERVAL);
        }
    });
}
