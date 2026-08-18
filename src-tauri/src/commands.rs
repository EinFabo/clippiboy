//! Tauri-Commands — die einzige Schnittstelle der UI zum Kern.

use tauri::State;

use crate::audio::devices;
use crate::capture;
use crate::clips::Library;
use crate::encode;
use crate::export;
use crate::model::{
    AppConfig, AudioDevice, AudioProcess, AudioSource, CaptureTarget, Clip, ClipTrack, EncoderInfo,
    EngineStatus, ExportProgress, ExportRequest, ExportResult,
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
    let previous = state.config_snapshot();
    let next = state.replace_config(config);
    // Der Player kommt sonst nicht an Clips außerhalb des Videos-Ordners.
    crate::allow_clip_dir(&app, &next.clip_dir);
    // Ecke oder Bildschirm können sich geändert haben.
    crate::overlay::reposition(&app);
    crate::apply_autostart(&app, next.auto_start_with_windows);

    // Ein laufendes Capture hängt fest an seinem Monitor bzw. Fenster. Ohne
    // Neustart bliebe die Auswahl in der Oberfläche wirkungslos: Der Puffer
    // nähme weiter die alte Quelle auf. Nur beim echten Quellenwechsel — an
    // Auflösung oder Bitrate wird per Regler gedreht, da wäre ein Neustart je
    // Mausbewegung fatal.
    let target_changed = next.recording.target_kind != previous.recording.target_kind
        || next.recording.target_id != previous.recording.target_id;
    if target_changed && state.is_buffering() {
        log::info!("Aufnahmequelle gewechselt — Puffer wird neu gestartet");
        state.stop_pipeline();
        crate::start_buffer_and_notify(&app);
    }

    // Normalerweise gehen Hotkeys über `set_hotkeys`; kommen sie doch einmal
    // hier durch, dürfen sie nicht bloß im JSON stehen und nirgends gelten.
    if previous.save_clip_hotkey != next.save_clip_hotkey
        || previous.toggle_buffer_hotkey != next.toggle_buffer_hotkey
    {
        if let Err(err) = crate::register_hotkeys(&app) {
            crate::notify(&app, "error", err);
        }
    }
    next
}

/// Die beiden globalen Hotkeys neu belegen.
///
/// Getrennt von `set_config`, weil hier etwas schiefgehen kann: eine
/// unbrauchbare Kombination oder eine, die schon ein anderes Programm hält.
/// Schlägt das Registrieren fehl, gilt wieder die vorherige Belegung — sonst
/// stünde in den Einstellungen ein Hotkey, der nichts auslöst.
#[tauri::command]
pub fn set_hotkeys(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    save_clip: String,
    toggle_buffer: String,
) -> Result<AppConfig> {
    let result = apply_hotkeys(&state, &app, save_clip, toggle_buffer);
    if result.is_err() {
        // Auch nach einer abgelehnten Eingabe müssen die bisherigen Hotkeys
        // wieder greifen — die Einstellungen legen sie fürs Aufnehmen still.
        let _ = crate::register_hotkeys(&app);
    }
    result
}

fn apply_hotkeys(
    state: &State<'_, AppState>,
    app: &tauri::AppHandle,
    save_clip: String,
    toggle_buffer: String,
) -> Result<AppConfig> {
    let save_clip = save_clip.trim().to_string();
    let toggle_buffer = toggle_buffer.trim().to_string();
    crate::parse_hotkey(&save_clip)?;
    crate::parse_hotkey(&toggle_buffer)?;
    if save_clip.eq_ignore_ascii_case(&toggle_buffer) {
        return Err("Beide Hotkeys liegen auf derselben Tastenkombination.".into());
    }

    let previous = state.config_snapshot();
    let mut config = previous.clone();
    config.save_clip_hotkey = save_clip;
    config.toggle_buffer_hotkey = toggle_buffer;
    let next = state.replace_config(config);

    match crate::register_hotkeys(app) {
        Ok(()) => Ok(next),
        Err(err) => {
            let mut rollback = state.config_snapshot();
            rollback.save_clip_hotkey = previous.save_clip_hotkey;
            rollback.toggle_buffer_hotkey = previous.toggle_buffer_hotkey;
            state.replace_config(rollback);
            let _ = crate::register_hotkeys(app);
            Err(err)
        }
    }
}

/// Die globalen Hotkeys stilllegen, solange in den Einstellungen eine neue
/// Kombination aufgenommen wird — sonst speichert das Drücken der alten
/// Belegung nebenbei einen Clip oder stoppt den Puffer.
#[tauri::command]
pub fn suspend_hotkeys(app: tauri::AppHandle) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    if let Err(err) = app.global_shortcut().unregister_all() {
        log::warn!("Hotkeys konnten nicht stillgelegt werden: {err}");
    }
}

/// Gegenstück zu `suspend_hotkeys` — nach einem Abbruch der Aufnahme.
#[tauri::command]
pub fn resume_hotkeys(app: tauri::AppHandle) {
    if let Err(err) = crate::register_hotkeys(&app) {
        log::warn!("{err}");
    }
}

/// Den Ordner wechseln, in dem neue Clips landen.
///
/// Angelegt wird er gleich mit, und einmal hineingeschrieben wird auch — ein
/// Pfad, der erst beim Speichern des ersten Clips auffliegt, hilft niemandem.
#[tauri::command]
pub fn set_clip_dir(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    dir: String,
) -> Result<AppConfig> {
    let dir = dir.trim();
    if dir.is_empty() {
        return Err("Kein Ordner ausgewählt.".into());
    }
    let path = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&path)
        .map_err(|err| format!("Ordner „{dir}“ lässt sich nicht anlegen: {err}"))?;
    let probe = path.join(".clippiboy-schreibtest");
    std::fs::write(&probe, b"")
        .map_err(|err| format!("In „{dir}“ darf ClippiBoy nicht schreiben: {err}"))?;
    let _ = std::fs::remove_file(&probe);

    let mut config = state.config_snapshot();
    config.clip_dir = path.to_string_lossy().to_string();
    let next = state.replace_config(config);
    // Ohne die Freigabe spielt der Player nichts ab, was hier landet.
    crate::allow_clip_dir(&app, &next.clip_dir);
    Ok(next)
}

/// Der Vorschlag für den Clip-Ordner — Ausgangspunkt des Ordner-Dialogs und
/// Ziel des Knopfs „Zurücksetzen".
#[tauri::command]
pub fn default_clip_dir() -> String {
    crate::config::default_clip_dir()
        .to_string_lossy()
        .to_string()
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

/// Name, Beschreibung und Spiel eines Clips ändern. Leere Felder löschen den
/// jeweiligen Eintrag wieder — in der Galerie steht dann wieder der Dateiname.
#[tauri::command]
pub fn update_clip(
    state: State<'_, AppState>,
    id: String,
    title: Option<String>,
    description: Option<String>,
    game: Option<String>,
) -> Result<Clip> {
    let trimmed = |value: Option<String>| {
        value
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    };
    let (title, description, game) = (trimmed(title), trimmed(description), trimmed(game));

    with_library(&state, |lib| {
        lib.update_meta(&id, title.as_deref(), description.as_deref(), game.as_deref())
            .map_err(|e| e.to_string())?;
        lib.get(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Clip nicht gefunden".to_string())
    })
}

/// Die Tonspuren eines Clips — jede mit einer entpackten Datei für die
/// Vorschau, damit der Player sie einzeln aussteuern kann.
///
/// `async`, weil ffprobe und das Entpacken je nach Länge eine Sekunde brauchen
/// und der Hauptfaden solange das Fenster nicht zeichnen würde.
#[tauri::command(async)]
pub fn clip_tracks(state: State<'_, AppState>, id: String) -> Result<Vec<ClipTrack>> {
    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "Clip nicht gefunden".to_string())?;

    let mut tracks = export::tracks(&clip)?;
    for track in tracks.iter_mut().skip(1) {
        match export::extract_track(&clip, track.index) {
            Ok(path) => track.preview_path = Some(path.to_string_lossy().to_string()),
            // Ohne Vorschaudatei bleibt der Regler bedienbar, nur hörbar wird
            // die Spur erst im Export.
            Err(err) => log::warn!("Spur {} nicht entpackt: {err}", track.index),
        }
    }
    Ok(tracks)
}

/// Den Clip mit der eingestellten Mischung und dem Zuschnitt neu ausgeben.
#[tauri::command(async)]
pub fn export_clip(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    request: ExportRequest,
) -> Result<ExportResult> {
    use tauri::Emitter;

    let clip = with_library(&state, |lib| {
        lib.get(&request.clip_id).map_err(|e| e.to_string())
    })?
    .ok_or_else(|| "Clip nicht gefunden".to_string())?;

    let config = state.config_snapshot();
    let clip_id = clip.id.clone();
    let result = export::export(
        &clip,
        &request,
        crate::encode::resolve(config.recording.encoder),
        config.recording.bitrate_kbps,
        |progress| {
            let _ = app.emit(
                "export-progress",
                ExportProgress {
                    clip_id: clip_id.clone(),
                    progress,
                },
            );
        },
    );
    match &result {
        Ok(export) => crate::notify(
            &app,
            "ok",
            format!("Export fertig · {}", file_name(&export.path)),
        ),
        Err(err) => crate::notify(&app, "error", err.clone()),
    }
    result
}

/// Beliebige Datei im Explorer zeigen — für den Export, der auch außerhalb des
/// Clip-Ordners landen darf und deshalb keine Clip-Kennung hat.
#[tauri::command]
pub fn reveal_path(app: tauri::AppHandle, path: String) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;

    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| e.to_string())
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
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
