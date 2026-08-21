//! Tauri-Commands — die einzige Schnittstelle der UI zum Kern.

use std::path::Path;

use tauri::State;

use crate::audio::devices;
use crate::capture;
use crate::clips::Library;
use crate::edit;
use crate::encode;
use crate::preview;
use crate::stems;
use crate::model::{
    AppConfig, AudioDevice, AudioProcess, AudioSource, CaptureTarget, Clip, ClipEdit, ClipTrack,
    EncoderInfo, EngineStatus, TrackMix,
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
///
/// `async` aus demselben Grund wie `save_clip`: Das Hochfahren des Capture-
/// Stacks dauert einige hundert Millisekunden.
#[tauri::command(async)]
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

/// Gegenstück zu `start_buffer`. `async`, weil das Stoppen notfalls auf ein
/// laufendes Speichern wartet.
#[tauri::command(async)]
pub fn stop_buffer(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<()> {
    if state.status.lock().buffer_active {
        crate::toggle_buffer_and_notify(&app);
    }
    Ok(())
}

/// Speichert den Puffer. `seconds` wird derzeit nur vom Trim-Gedanken gebraucht;
/// der übliche Weg ist der gemeinsame Pfad mit Hotkey und Tray, der zusätzlich
/// `clip-saved` und den Overlay-Banner auslöst.
///
/// `async`, weil Versiegeln und Muxen mehrere Sekunden dauern: als synchroner
/// Command liefe das auf dem Hauptthread und die Oberfläche stünde solange —
/// samt der Fensteraufrufe, die der Overlay-Banner dorthin schickt.
#[tauri::command(async)]
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
    // Einzelspuren und Original gehören zum Clip und haben ohne ihn keinen
    // Zweck mehr.
    stems::remove(&id);
    edit::remove(&id);
    // Das Vorschaubild auch dann, wenn in der Datenbank keines vermerkt ist.
    crate::thumbs::remove(&id);

    let clip_dir = state.config_snapshot().clip_dir;
    let folder = with_library(&state, |lib| {
        let folder = lib
            .get(&id)
            .map_err(|e| e.to_string())?
            .and_then(|clip| std::path::PathBuf::from(clip.path).parent().map(Path::to_path_buf));
        lib.delete(&id).map_err(|e| e.to_string())?;
        Ok(folder)
    })?;
    // War das der letzte Clip seines Spiels, bleibt sonst ein leerer Ordner
    // stehen.
    if let Some(folder) = folder {
        crate::filing::prune(&folder, Path::new(&clip_dir));
    }
    Ok(())
}

/// Das Herz an einem Clip setzen oder wegnehmen.
///
/// Die Datei zieht dabei **nicht** von selbst um — das erledigt `file_clip`,
/// sobald der Clip nicht mehr im Player offen ist. Ein Umzug unter dem
/// laufenden Video heraus würde die Wiedergabe abreißen lassen.
#[tauri::command]
pub fn set_clip_favorite(state: State<'_, AppState>, id: String, favorite: bool) -> Result<Clip> {
    with_library(&state, |lib| {
        lib.set_favorite(&id, favorite).map_err(|e| e.to_string())?;
        lib.get(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Clip nicht gefunden".into())
    })
}

/// Die Datei eines Clips in den Ordner bringen, in den sie gehört.
///
/// Scheitert der Umzug — meist, weil die Datei noch offen ist —, bleibt sie
/// liegen und der Clip wird unverändert zurückgegeben. Der nächste Start holt
/// es nach (`filing::tidy`); die Galerie stimmt in der Zwischenzeit trotzdem,
/// denn sie liest aus der Datenbank.
#[tauri::command]
pub fn file_clip(state: State<'_, AppState>, id: String) -> Result<Clip> {
    let clip_dir = state.config_snapshot().clip_dir;
    with_library(&state, |lib| {
        let clip = lib
            .get(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Clip nicht gefunden".to_string())?;
        match crate::filing::place(&clip, &clip_dir) {
            Ok(Some(target)) => {
                lib.set_path(&id, &target.to_string_lossy())
                    .map_err(|e| e.to_string())?;
                lib.get(&id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "Clip nicht gefunden".into())
            }
            Ok(None) => Ok(clip),
            Err(err) => {
                log::warn!("Clip '{id}' blieb liegen: {err}");
                Ok(clip)
            }
        }
    })
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

/// Die Videodatei eines Clips in die Zwischenablage legen.
///
/// Nicht den Pfad, die **Datei**: In Discord oder WhatsApp hängt Strg+V den
/// Clip danach als Anhang an, im Explorer legt es eine Kopie ab.
#[tauri::command]
pub fn copy_clip_file(state: State<'_, AppState>, id: String) -> Result<()> {
    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "Clip nicht gefunden".to_string())?;
    let path = std::path::PathBuf::from(&clip.path);
    if !path.is_file() {
        return Err("Die Clipdatei ist nicht mehr da.".into());
    }
    crate::clipboard::copy_files(&[path])
}

/// Den Clip in dem Player öffnen, den Windows dafür vorgesehen hat.
#[tauri::command]
pub fn open_clip(state: State<'_, AppState>, app: tauri::AppHandle, id: String) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;

    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "Clip nicht gefunden".to_string())?;
    app.opener()
        .open_path(&clip.path, None::<&str>)
        .map_err(|e| e.to_string())
}

/// Text in die Zwischenablage legen — für „Pfad kopieren" und das Menü in den
/// Textfeldern.
#[tauri::command]
pub fn clipboard_write_text(text: String) -> Result<()> {
    crate::clipboard::copy_text(&text)
}

/// Text aus der Zwischenablage holen. Steckt kein Text darin, kommt ein leerer
/// zurück — dann gibt es eben nichts einzufügen.
#[tauri::command]
pub fn clipboard_read_text() -> Result<String> {
    crate::clipboard::read_text()
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

/// Bild der Tonspur für die Zeitleiste. Liefert den Pfad zum PNG.
///
/// `async`, weil ffmpeg dafür den Ton einmal komplett durchliest.
#[tauri::command(async)]
pub fn clip_waveform(state: State<'_, AppState>, id: String) -> Result<String> {
    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "Clip nicht gefunden".to_string())?;
    preview::waveform(&clip).map(|path| path.to_string_lossy().to_string())
}

/// Die Tonspuren eines Clips, jede mit einer eigenen Datei für die Vorschau —
/// nur so lassen sie sich im Player einzeln aussteuern.
///
/// `async`, weil das Nachziehen bei einem Clip aus der Zeit vor der Umstellung
/// je nach Länge eine Sekunde braucht und der Hauptfaden solange das Fenster
/// nicht zeichnen würde.
#[tauri::command(async)]
pub fn clip_tracks(state: State<'_, AppState>, id: String) -> Result<Vec<ClipTrack>> {
    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "Clip nicht gefunden".to_string())?;
    // Aus der unversehrten Aufnahme, falls der Clip geschnitten ist — die
    // Einzelspuren stehen immer in Koordinaten des Originals.
    stems::tracks(&clip.id, &edit::source_path(&clip))
}

/// Den Clip so schreiben, wie er im Editor steht: Mischung eingerechnet,
/// Zuschnitt ausgeführt.
///
/// Der Zuschnitt landet wirklich in der Datei — wer den Clip verschickt,
/// verschickt den geschnittenen. Die unversehrte Aufnahme wandert dabei in die
/// Original-Ablage und kommt über [`restore_clip_original`] jederzeit zurück.
///
/// `async`, weil ein Schnitt am Anfang das Bild neu encodiert und das je nach
/// Länge dauert; solange dürfte der Hauptfaden das Fenster nicht zeichnen.
#[tauri::command(async)]
pub fn apply_clip_edit(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
    start_ms: u64,
    end_ms: u64,
    tracks: Vec<TrackMix>,
) -> Result<Clip> {
    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "Clip nicht gefunden".to_string())?;
    let (encoder, bitrate) = encoder_for(&state);
    let trim = edit::Trim { start_ms, end_ms };

    let applied = edit::apply(&clip, trim, &tracks, encoder, bitrate, progress(&app, &id));
    store(&state, &app, &clip, applied, tracks)
}

/// Den Zuschnitt aufheben: die ganze Aufnahme zurückholen, Mischung behalten.
///
/// `async` aus demselben Grund wie [`apply_clip_edit`] — auch wenn hier nur
/// kopiert wird, dauert das Ummuxen einer langen Aufnahme seine Sekunden.
#[tauri::command(async)]
pub fn restore_clip_original(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
) -> Result<Clip> {
    let clip = with_library(&state, |lib| lib.get(&id).map_err(|e| e.to_string()))?
        .ok_or_else(|| "Clip nicht gefunden".to_string())?;
    let (encoder, bitrate) = encoder_for(&state);
    let tracks = clip.edit.as_ref().map(|e| e.tracks.clone()).unwrap_or_default();

    // Scheitert es, bleibt der Eintrag am Clip stehen: Er trägt den Versatz, in
    // dem die Einzelspuren liegen. Ihn wegzuwerfen, weil die Aufnahme nicht
    // auffindbar ist, ließe die Vorschau für immer versetzt laufen.
    let applied = edit::restore(&clip, &tracks, encoder, bitrate, progress(&app, &id));
    store(&state, &app, &clip, applied, tracks)
}

/// Encoder und Bitrate für einen Neuencodierlauf, aus den Einstellungen.
fn encoder_for(state: &State<'_, AppState>) -> (crate::model::EncoderId, u32) {
    let recording = state.config_snapshot().recording;
    (encode::resolve(recording.encoder), recording.bitrate_kbps)
}

/// Fortschritt an die Oberfläche melden. ffmpeg meldet oft genug, dass ein
/// Balken sich sichtbar bewegt.
fn progress<'a>(app: &'a tauri::AppHandle, id: &'a str) -> impl Fn(f32) + 'a {
    use tauri::Emitter;
    move |value| {
        let _ = app.emit(
            "clip-progress",
            crate::model::ClipProgress {
                clip_id: id.to_string(),
                progress: value,
            },
        );
    }
}

/// Das Ergebnis eines Laufs in die Datenbank schreiben und den Clip neu lesen.
fn store(
    state: &State<'_, AppState>,
    app: &tauri::AppHandle,
    clip: &Clip,
    applied: Result<edit::Applied>,
    tracks: Vec<TrackMix>,
) -> Result<Clip> {
    let applied = match applied {
        Ok(applied) => applied,
        Err(err) => {
            crate::notify(app, "error", err.clone());
            return Err(err);
        }
    };

    // Der Zuschnitt steckt jetzt in der Datei — was hier abgelegt wird, ist der
    // volle Bereich der neuen Datei. Wo der im Original sitzt, steht daneben.
    let edit = ClipEdit {
        start_ms: 0,
        end_ms: applied.duration_ms,
        tracks,
    };
    with_library(state, |lib| {
        lib.set_edit(&clip.id, Some(&edit)).map_err(|e| e.to_string())?;
        lib.set_original(&clip.id, applied.original.as_ref())
            .map_err(|e| e.to_string())?;
        lib.set_file_state(&clip.id, applied.duration_ms, applied.size_bytes)
            .map_err(|e| e.to_string())?;
        lib.get(&clip.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Clip nicht gefunden".to_string())
    })
}

/// Beliebige Datei im Explorer zeigen.
#[tauri::command]
pub fn reveal_path(app: tauri::AppHandle, path: String) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;

    app.opener()
        .reveal_item_in_dir(&path)
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
