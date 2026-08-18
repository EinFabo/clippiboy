//! Selbstaktualisierung über GitHub Releases.
//!
//! Die App fragt beim Start nach, ob eine neuere Fassung veröffentlicht ist,
//! lädt sie aber nicht von selbst: Installieren heißt unter Windows, dass der
//! Prozess beendet und das Setup gestartet wird — mitten in einer Aufnahme wäre
//! das genau das Falsche. Deshalb meldet ClippiBoy den Fund nur, und der Nutzer
//! entscheidet in den Einstellungen, wann installiert wird.
//!
//! Die Echtheit prüft der Updater selbst: Jedes Paket ist mit dem privaten
//! Schlüssel signiert, der öffentliche steckt in `tauri.conf.json`.

use parking_lot::Mutex;
use tauri::Manager;
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::state::AppState;

/// Der zuletzt gefundene, noch nicht installierte Fund.
#[derive(Default)]
pub struct Pending(Mutex<Option<Update>>);

/// Was die UI über ein Update wissen muss.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    pub notes: Option<String>,
    pub date: Option<String>,
}

/// Nachsehen, ob es etwas Neueres gibt.
pub async fn check(app: &tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    let updater = app
        .updater_builder()
        // Vor dem Neustart die Aufnahme sauber beenden, sonst bleiben
        // halbfertige Segmentdateien liegen.
        .on_before_exit({
            let app = app.clone();
            move || app.state::<AppState>().stop_pipeline()
        })
        .build()
        .map_err(|err| format!("Update-Prüfung nicht möglich: {err}"))?;

    let found = updater
        .check()
        .await
        .map_err(|err| format!("Update-Prüfung fehlgeschlagen: {err}"))?;

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

/// Den gefundenen Stand herunterladen und installieren.
///
/// Danach kommt die App nicht zurück: Unter Windows startet das Setup und
/// beendet den laufenden Prozess.
pub async fn install(app: &tauri::AppHandle) -> Result<(), String> {
    let update = app.state::<Pending>().0.lock().clone();
    let Some(update) = update else {
        return Err("Es liegt kein geprüftes Update bereit.".into());
    };

    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
        .map_err(|err| format!("Update konnte nicht installiert werden: {err}"))?;
    Ok(())
}

/// Beim Start einmal nachsehen und einen Fund an die UI melden.
pub fn check_on_startup(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match check(&app).await {
            Ok(Some(info)) => {
                log::info!("Update {} verfügbar", info.version);
                let _ = tauri::Emitter::emit(&app, "update-available", info);
            }
            Ok(None) => log::info!("Kein Update verfügbar"),
            // Ohne Netz oder ohne Release ist das kein Fehler, den der Nutzer
            // sehen müsste.
            Err(err) => log::info!("{err}"),
        }
    });
}
