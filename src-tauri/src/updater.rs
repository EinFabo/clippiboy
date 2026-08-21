//! Self-update via GitHub Releases.
//!
//! The app asks at startup whether a newer version has been published, but does
//! not download it by itself: installing on Windows means the process is ended
//! and the installer started — in the middle of a recording that would be
//! exactly the wrong thing. So ClippiBoy only reports the find, and the user
//! decides in the settings when to install.
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
            let info = UpdateInfo {
                version: update.version.clone(),
                current_version: update.current_version.clone(),
                notes: update.body.clone(),
                date: update.date.map(|date| date.to_string()),
            };
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

    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
        .map_err(|err| format!("could not install the update: {err}"))?;
    Ok(())
}

/// Look once at startup and report a find to the UI.
pub fn check_on_startup(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match check(&app).await {
            Ok(Some(info)) => {
                log::info!("update {} available", info.version);
                let _ = tauri::Emitter::emit(&app, "update-available", info);
            }
            Ok(None) => log::info!("no update available"),
            // With no network or no release this is not an error the user needs
            // to see.
            Err(err) => log::info!("{err}"),
        }
    });
}
