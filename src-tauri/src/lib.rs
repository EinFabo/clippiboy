pub mod audio;
pub mod buffer;
pub mod capture;
pub mod clips;
pub mod commands;
pub mod config;
pub mod encode;
pub mod game;
pub mod model;
pub mod muxer;
pub mod overlay;
pub mod pipeline;
pub mod state;
pub mod tray;
pub mod updater;

use std::time::Duration;

use tauri::{Emitter, Manager};

use overlay::BannerKind;
use state::AppState;

/// Meldung an die UI (Toast).
#[derive(Clone, serde::Serialize)]
struct Notice {
    kind: &'static str,
    message: String,
}

fn notify(app: &tauri::AppHandle, kind: &'static str, message: impl Into<String>) {
    let _ = app.emit(
        "notice",
        Notice {
            kind,
            message: message.into(),
        },
    );
}

/// Clip speichern und das Ergebnis melden — von Hotkey, Tray und Button
/// gemeinsam genutzt, damit alle drei Wege identisch reagieren.
pub fn save_clip_and_notify(app: &tauri::AppHandle) -> Result<model::Clip, String> {
    let state = app.state::<AppState>();
    match state.save_clip(None) {
        Ok(clip) => {
            let seconds = clip.duration_ms / 1000;
            if let Some(library) = state.library.lock().as_ref() {
                if let Err(err) = library.insert(&clip) {
                    log::error!("Clip konnte nicht indexiert werden: {err}");
                }
            }
            let _ = app.emit("clip-saved", clip.clone());
            notify(app, "ok", format!("Clip gespeichert · {seconds} s"));
            overlay::show_with_thumb(
                app,
                BannerKind::Clip,
                clip.game.clone().unwrap_or_else(|| "Clip gespeichert".into()),
                Some(format!("Clip gespeichert · {seconds} s")),
                clip.thumb_path.clone(),
            );
            Ok(clip)
        }
        Err(err) => {
            notify(app, "error", err.clone());
            overlay::show(app, BannerKind::Error, "Clip fehlgeschlagen", Some(err.clone()));
            Err(err)
        }
    }
}

/// Replay-Puffer an- oder ausschalten und das Ergebnis melden.
///
/// Der Weg für Hotkey, Tray und Knopf — also immer eine Entscheidung des
/// Nutzers. Die Automatik meldet sich deshalb hier ab bzw. wieder an.
pub fn toggle_buffer_and_notify(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    if state.status.lock().buffer_active {
        state.auto.manual_stop();
        stop_buffer_and_notify(app);
    } else {
        state.auto.manual_start();
        start_buffer_and_notify(app);
    }
}

/// Puffer starten und das Ergebnis melden.
pub fn start_buffer_and_notify(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    match state.start_pipeline() {
        Ok(()) => {
            notify(app, "ok", "Replay-Puffer läuft");
            let seconds = state.config_snapshot().buffer.seconds;
            overlay::show(
                app,
                BannerKind::Buffer,
                "Replay-Puffer läuft",
                Some(match state.current_game.lock().as_ref() {
                    Some(game) => format!("{game} · letzte {seconds} s"),
                    None => format!("Die letzten {seconds} s werden vorgehalten"),
                }),
            );
        }
        Err(err) => {
            notify(app, "error", err.clone());
            overlay::show(app, BannerKind::Error, "Puffer startet nicht", Some(err));
        }
    }
}

/// Puffer stoppen und das Ergebnis melden.
pub fn stop_buffer_and_notify(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    state.stop_pipeline();
    notify(app, "ok", "Replay-Puffer gestoppt");
    overlay::show(app, BannerKind::BufferOff, "Replay-Puffer aus", None);
}

/// Puffer beim Programmstart einschalten, wenn er dauerhaft laufen soll.
///
/// „Nur im Spiel" wird hier bewusst ausgelassen — darum kümmert sich
/// `apply_auto_buffer`, sobald ein Spiel im Vordergrund auftaucht.
fn start_buffer_if_configured(app: &tauri::AppHandle) {
    let config = app.state::<AppState>().config_snapshot();
    if !config.buffer.auto_start || config.only_buffer_in_game {
        return;
    }
    let app = app.clone();
    // Nicht im Setup-Thread: das Hochfahren der Aufnahme dauert einen Moment
    // und das Fenster soll währenddessen schon da sein.
    std::thread::spawn(move || start_buffer_and_notify(&app));
}

/// Automatik „Puffer an, sobald ein Spiel läuft". Läuft im Takt der
/// Spielerkennung, also alle zwei Sekunden.
fn apply_auto_buffer(app: &tauri::AppHandle, game: Option<&String>) {
    let state = app.state::<AppState>();
    let config = state.config_snapshot();
    if !config.buffer.auto_start || !config.only_buffer_in_game {
        return;
    }
    let active = state.status.lock().buffer_active;
    let action = state.auto.poll(game.is_some(), active);
    if action == state::AutoAction::Nothing {
        return;
    }
    // Nicht im Statustakt erledigen: Das Hochfahren der Aufnahme dauert einen
    // Moment, und das Stoppen wartet notfalls auf ein laufendes Speichern.
    let app = app.clone();
    std::thread::spawn(move || match action {
        state::AutoAction::Start => start_buffer_and_notify(&app),
        state::AutoAction::Stop => stop_buffer_and_notify(&app),
        state::AutoAction::Nothing => {}
    });
}

/// Argument, mit dem Windows ClippiBoy beim Anmelden startet.
pub const AUTOSTART_ARG: &str = "--autostart";

fn started_by_autostart() -> bool {
    std::env::args().any(|arg| arg == AUTOSTART_ARG)
}

/// Den Windows-Autostart an die Einstellung angleichen.
pub fn apply_autostart(app: &tauri::AppHandle, wanted: bool) {
    use tauri_plugin_autostart::ManagerExt;

    let manager = app.autolaunch();
    if manager.is_enabled().unwrap_or(false) == wanted {
        return;
    }
    let result = if wanted {
        manager.enable()
    } else {
        manager.disable()
    };
    if let Err(err) = result {
        log::warn!("Autostart konnte nicht gesetzt werden: {err}");
    }
}

/// Globale Hotkeys registrieren (Speichern und Puffer an/aus).
fn register_hotkeys(app: &tauri::AppHandle) {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    let config = app.state::<AppState>().config_snapshot();
    let shortcuts = app.global_shortcut();

    let save = config.save_clip_hotkey.clone();
    if let Err(err) = shortcuts.on_shortcut(save.as_str(), move |app, _shortcut, event| {
        if event.state() == ShortcutState::Pressed {
            let app = app.clone();
            // Das Muxen dauert einen Moment — nicht im Hotkey-Thread erledigen.
            std::thread::spawn(move || {
                let _ = save_clip_and_notify(&app);
            });
        }
    }) {
        log::warn!("Hotkey '{save}' konnte nicht registriert werden: {err}");
    }

    let toggle = config.toggle_buffer_hotkey.clone();
    if let Err(err) = shortcuts.on_shortcut(toggle.as_str(), move |app, _shortcut, event| {
        if event.state() == ShortcutState::Pressed {
            toggle_buffer_and_notify(app);
        }
    }) {
        log::warn!("Hotkey '{toggle}' konnte nicht registriert werden: {err}");
    }
}

/// Schickt Pegel (20 Hz) und Statusdaten (1 Hz) an die UI.
fn spawn_ui_updates(app: &tauri::AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        let mut tick: u32 = 0;
        loop {
            std::thread::sleep(Duration::from_millis(50));
            let state = handle.state::<AppState>();
            let sources = state.config_snapshot().sources;
            let levels = state.audio.levels(&sources);
            let _ = handle.emit("audio-levels", &levels);

            // Diagnose: mit CLIPPIBOY_LOG_LEVELS=1 werden die Pegel mitgeloggt.
            if tick % 20 == 0 && std::env::var("CLIPPIBOY_LOG_LEVELS").is_ok() {
                let summary: Vec<String> = levels
                    .iter()
                    .map(|(id, level)| format!("{id}={level:.3}"))
                    .collect();
                log::info!("Pegel: {}", summary.join(" "));
            }

            tick += 1;
            // Alle 2 s nachsehen, welches Spiel im Vordergrund läuft.
            if tick % 40 == 0 {
                let game = state.track_game();
                apply_auto_buffer(&handle, game.as_ref());
            }
            if tick % 20 == 0 {
                let status = state.status_snapshot();
                tray::refresh(&handle, &status);
                let _ = handle.emit("engine-status", status);
                let _ = handle.emit("audio-errors", state.audio.errors());
            }
        }
    });
}

/// Den eingestellten Clip-Ordner für das `asset:`-Protokoll freigeben.
///
/// Ohne das spielt der Player nichts ab, sobald `clipDir` außerhalb des
/// `$VIDEO`-Bereichs aus `tauri.conf.json` liegt.
pub fn allow_clip_dir(app: &tauri::AppHandle, dir: &str) {
    if let Err(err) = app.asset_protocol_scope().allow_directory(dir, true) {
        log::warn!("Clip-Ordner '{dir}' ist für den Player nicht freigegeben: {err}");
    }
}

/// Das Fenster verstecken statt schließen — die App lebt im Tray weiter.
fn hide_to_tray(window: &tauri::Window) {
    let _ = window.hide();

    let app = window.app_handle();
    let state = app.state::<AppState>();
    {
        // Nur den Merker setzen und sichern — `replace_config` würde die
        // Audioquellen neu anwenden und mitten in einer Aufnahme stören.
        let mut config = state.config.lock();
        if config.tray_hint_shown {
            return;
        }
        config.tray_hint_shown = true;
        if let Err(err) = config::save(&config) {
            log::warn!("Tray-Hinweis konnte nicht gemerkt werden: {err}");
        }
    }
    overlay::show(
        app,
        BannerKind::Info,
        "ClippiBoy läuft weiter",
        Some("Über das Tray-Symbol wieder öffnen".into()),
    );
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // Das Argument landet in der Autostart-Verknüpfung: nur dann startet
        // ClippiBoy direkt ins Tray, ohne das Fenster aufzuziehen.
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![AUTOSTART_ARG]),
        ))
        .manage(updater::Pending::default())
        .manage(AppState::new())
        .setup(|app| {
            let handle = app.handle();
            let config = app.state::<AppState>().config_snapshot();
            allow_clip_dir(handle, &config.clip_dir);
            // Mitgeliefertes ffmpeg/ffprobe bekannt machen, bevor irgendetwas
            // einen Clip schreiben will.
            if let Ok(dir) = handle.path().resource_dir() {
                muxer::set_tool_dir(dir.join("resources"));
            }
            apply_autostart(handle, config.auto_start_with_windows);
            if started_by_autostart() {
                if let Some(window) = handle.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
            overlay::create(handle);
            if let Err(err) = tray::build(handle) {
                log::error!("Tray-Symbol konnte nicht angelegt werden: {err}");
            }
            spawn_ui_updates(handle);
            register_hotkeys(handle);
            start_buffer_if_configured(handle);
            updater::check_on_startup(handle);
            Ok(())
        })
        .on_window_event(|window, event| {
            // Gilt für Alt+F4 und die Fensterliste der Taskleiste; das ✕ in der
            // eigenen Titelleiste versteckt das Fenster direkt.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    hide_to_tray(window);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_audio_devices,
            commands::list_audio_processes,
            commands::list_capture_targets,
            commands::list_encoders,
            commands::get_config,
            commands::set_config,
            commands::add_audio_source,
            commands::update_audio_source,
            commands::remove_audio_source,
            commands::engine_status,
            commands::start_buffer,
            commands::stop_buffer,
            commands::save_clip,
            commands::list_clips,
            commands::delete_clip,
            commands::reveal_clip,
            commands::app_version,
            commands::check_update,
            commands::install_update,
        ])
        .build(tauri::generate_context!())
        .expect("ClippiBoy konnte nicht gestartet werden");

    app.run(|handle, event| {
        // Ein verstecktes Fenster darf den Prozess nicht mitnehmen — beendet
        // wird nur über „Beenden" im Tray.
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            if !handle
                .state::<AppState>()
                .quitting
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                api.prevent_exit();
            }
        }
    });
}
