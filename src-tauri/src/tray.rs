//! Tray-Icon: der einzige Weg zurück, wenn das Fenster versteckt ist.
//!
//! Menü und Hotkeys rufen dieselben Funktionen auf (`crate::save_clip_and_notify`,
//! `crate::toggle_buffer_and_notify`), damit beide Wege identisch reagieren.

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::Manager;

use crate::model::EngineStatus;
use crate::state::AppState;

pub const ID: &str = "main";

/// Menüeinträge und Icons, die im laufenden Betrieb umgeschaltet werden.
pub struct TrayHandles {
    toggle: CheckMenuItem<tauri::Wry>,
    save: MenuItem<tauri::Wry>,
    active_icon: Image<'static>,
    idle_icon: Image<'static>,
    /// Zuletzt gesetzter Tooltip — spart das Neusetzen im Sekundentakt.
    last_tooltip: parking_lot::Mutex<String>,
    last_active: std::sync::atomic::AtomicBool,
}

/// Bildpuffer vom App-Handle lösen, damit er in `TrayHandles` liegen kann.
fn owned(icon: &Image<'_>) -> Image<'static> {
    Image::new_owned(icon.rgba().to_vec(), icon.width(), icon.height())
}

/// Eigenes Tray-Icon: nur die Bildmarke, rund freigestellt. Das volle Logo
/// enthält den Schriftzug, und der ist bei 16 px nur noch ein Fleck.
const TRAY_PNG: &[u8] = include_bytes!("../icons/tray.png");

/// Graustufen-Variante des App-Icons für „Puffer aus".
///
/// Arbeitet direkt auf den RGBA-Daten des schon dekodierten Icons — deshalb
/// braucht es dafür kein zusätzliches Asset.
fn dimmed(icon: &Image<'_>) -> Image<'static> {
    let mut rgba = icon.rgba().to_vec();
    for pixel in rgba.chunks_exact_mut(4) {
        let grey = (pixel[0] as u32 * 30 + pixel[1] as u32 * 59 + pixel[2] as u32 * 11) / 100;
        pixel[0] = grey as u8;
        pixel[1] = grey as u8;
        pixel[2] = grey as u8;
        pixel[3] = (pixel[3] as u32 * 65 / 100) as u8;
    }
    Image::new_owned(rgba, icon.width(), icon.height())
}

pub fn build(app: &tauri::AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "ClippiBoy öffnen", true, None::<&str>)?;
    let toggle = CheckMenuItem::with_id(
        app,
        "toggle",
        "Replay-Puffer läuft",
        true,
        false,
        None::<&str>,
    )?;
    let save = MenuItem::with_id(app, "save", "Clip speichern", false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Beenden", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &toggle,
            &save,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let active_icon = Image::from_bytes(TRAY_PNG).map(|icon| owned(&icon)).unwrap_or_else(|err| {
        log::warn!("Tray-Icon konnte nicht gelesen werden: {err}");
        app.default_window_icon()
            .map(owned)
            .unwrap_or_else(|| Image::new_owned(vec![0; 4], 1, 1))
    });
    let idle_icon = dimmed(&active_icon);

    TrayIconBuilder::with_id(ID)
        .icon(idle_icon.clone())
        .tooltip("ClippiBoy · Puffer aus")
        .menu(&menu)
        // Ohne das öffnet ein Linksklick unter Windows das Menü statt das Fenster.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main_window(app),
            "toggle" => {
                let app = app.clone();
                std::thread::spawn(move || crate::toggle_buffer_and_notify(&app));
            }
            "save" => {
                let app = app.clone();
                // Das Muxen dauert einen Moment — nicht im Menü-Thread erledigen.
                std::thread::spawn(move || crate::save_clip_and_notify(&app));
            }
            "quit" => {
                let state = app.state::<AppState>();
                state
                    .quitting
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                state.stop_pipeline();
                app.exit(0);
            }
            other => log::warn!("Unbekannter Tray-Eintrag: {other}"),
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    app.manage(TrayHandles {
        toggle,
        save,
        active_icon,
        idle_icon,
        last_tooltip: parking_lot::Mutex::new(String::new()),
        last_active: std::sync::atomic::AtomicBool::new(false),
    });
    Ok(())
}

pub fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Tooltip, Icon und Menüzustand an den Status angleichen. Läuft im Sekundentakt.
pub fn refresh(app: &tauri::AppHandle, status: &EngineStatus) {
    let Some(handles) = app.try_state::<TrayHandles>() else {
        return;
    };
    let Some(tray) = app.tray_by_id(ID) else {
        return;
    };

    let tooltip = if status.buffer_active {
        let mut text = format!(
            "ClippiBoy · Puffer läuft — {} s",
            status.buffered_seconds.round() as u32
        );
        if let Some(game) = &status.game {
            text.push_str(" · ");
            text.push_str(game);
        }
        text
    } else {
        match &status.game {
            Some(game) => format!("ClippiBoy · Puffer aus · {game}"),
            None => "ClippiBoy · Puffer aus".to_string(),
        }
    };

    {
        let mut last = handles.last_tooltip.lock();
        if *last != tooltip {
            let _ = tray.set_tooltip(Some(&tooltip));
            *last = tooltip;
        }
    }

    let was_active = handles
        .last_active
        .swap(status.buffer_active, std::sync::atomic::Ordering::SeqCst);
    if was_active != status.buffer_active {
        let icon = if status.buffer_active {
            handles.active_icon.clone()
        } else {
            handles.idle_icon.clone()
        };
        let _ = tray.set_icon(Some(icon));
        let _ = handles.toggle.set_checked(status.buffer_active);
        let _ = handles.save.set_enabled(status.buffer_active);
    }
}
