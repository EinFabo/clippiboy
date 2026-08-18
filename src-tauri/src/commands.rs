//! Tauri-Commands — die einzige Schnittstelle der UI zum Kern.

use tauri::State;

use crate::audio::devices;
use crate::capture;
use crate::clips::Library;
use crate::encode;
use crate::model::{
    AppConfig, AudioDevice, AudioProcess, AudioSource, CaptureTarget, Clip, EncoderInfo,
    EngineStatus,
};
use crate::state::AppState;

type Result<T> = std::result::Result<T, String>;

#[tauri::command]
pub fn list_audio_devices() -> Vec<AudioDevice> {
    devices::list_devices()
}

#[tauri::command]
pub fn list_audio_processes() -> Vec<AudioProcess> {
    devices::list_processes()
}

#[tauri::command]
pub fn list_capture_targets() -> Vec<CaptureTarget> {
    capture::list_targets()
}

#[tauri::command]
pub fn list_encoders() -> Vec<EncoderInfo> {
    encode::list_encoders()
}

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> AppConfig {
    state.config_snapshot()
}

#[tauri::command]
pub fn set_config(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    config: AppConfig,
) -> AppConfig {
    let next = state.replace_config(config);
    // Der Player kommt sonst nicht an Clips außerhalb des Videos-Ordners.
    crate::allow_clip_dir(&app, &next.clip_dir);
    // Ecke oder Bildschirm können sich geändert haben.
    crate::overlay::reposition(&app);
    crate::apply_autostart(&app, next.auto_start_with_windows);
    next
}

#[tauri::command]
pub fn add_audio_source(state: State<'_, AppState>, source: AudioSource) -> AppConfig {
    state.upsert_source(source)
}

#[tauri::command]
pub fn update_audio_source(state: State<'_, AppState>, source: AudioSource) -> AppConfig {
    state.upsert_source(source)
}

#[tauri::command]
pub fn remove_audio_source(state: State<'_, AppState>, id: String) -> AppConfig {
    state.remove_source(&id)
}

#[tauri::command]
pub fn engine_status(state: State<'_, AppState>) -> EngineStatus {
    state.status_snapshot()
}

/// Puffer starten. Läuft über denselben Pfad wie Hotkey und Tray, damit auch
/// der Knopf in der App Toast und Banner auslöst.
#[tauri::command]
pub fn start_buffer(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<()> {
    if state.status.lock().buffer_active {
        return Ok(());
    }
    crate::toggle_buffer_and_notify(&app);
    match state.status.lock().buffer_active {
        true => Ok(()),
        // Die Meldung ist schon draußen; der Fehler bringt nur den Store zurück
        // auf den echten Zustand.
        false => Err("Der Replay-Puffer konnte nicht gestartet werden.".into()),
    }
}

#[tauri::command]
pub fn stop_buffer(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<()> {
    if state.status.lock().buffer_active {
        crate::toggle_buffer_and_notify(&app);
    }
    Ok(())
}

/// Speichert den Puffer. `seconds` wird derzeit nur vom Trim-Gedanken gebraucht;
/// der übliche Weg ist der gemeinsame Pfad mit Hotkey und Tray, der zusätzlich
/// `clip-saved` und den Overlay-Banner auslöst.
#[tauri::command]
pub fn save_clip(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    seconds: Option<u32>,
) -> Result<Clip> {
    match seconds {
        None => crate::save_clip_and_notify(&app),
        Some(seconds) => {
            let clip = state.save_clip(Some(seconds))?;
            with_library(&state, |lib| lib.insert(&clip).map_err(|e| e.to_string()))?;
            Ok(clip)
        }
    }
}

#[tauri::command]
pub fn list_clips(state: State<'_, AppState>) -> Result<Vec<Clip>> {
    with_library(&state, |lib| lib.list().map_err(|e| e.to_string()))
}

#[tauri::command]
pub fn delete_clip(state: State<'_, AppState>, id: String) -> Result<()> {
    with_library(&state, |lib| lib.delete(&id).map_err(|e| e.to_string()))
}

#[tauri::command]
pub fn reveal_clip(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;

    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "Clip nicht gefunden".to_string())?;
    // Öffnet den Ordner und markiert die Datei darin.
    app.opener()
        .reveal_item_in_dir(&clip.path)
        .map_err(|e| e.to_string())
}

fn with_library<T>(
    state: &State<'_, AppState>,
    f: impl FnOnce(&Library) -> Result<T>,
) -> Result<T> {
    let guard = state.library.lock();
    match guard.as_ref() {
        Some(lib) => f(lib),
        None => Err("Clip-Datenbank ist nicht verfügbar".into()),
    }
}

/// Version aus `tauri.conf.json` — die Einstellungen zeigen sie an.
#[tauri::command]
pub fn app_version(app: tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Nach einem Update sehen. `None` heißt: schon aktuell.
#[tauri::command]
pub async fn check_update(app: tauri::AppHandle) -> Result<Option<crate::updater::UpdateInfo>> {
    crate::updater::check(&app).await
}

/// Das gefundene Update installieren. Beendet die App und startet das Setup.
#[tauri::command]
pub async fn install_update(app: tauri::AppHandle) -> Result<()> {
    crate::updater::install(&app).await
}
