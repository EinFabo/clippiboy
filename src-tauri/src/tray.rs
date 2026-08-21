//! Tray icon: the only way back when the window is hidden.
//!
//! Menu and hotkeys call the same functions (`crate::save_clip_and_notify`,
//! `crate::toggle_buffer_and_notify`) so both routes behave identically.

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::Manager;

use crate::model::EngineStatus;
use crate::state::AppState;

pub const ID: &str = "main";

/// Menu entries and icons that get toggled while running.
pub struct TrayHandles {
    toggle: CheckMenuItem<tauri::Wry>,
    save: MenuItem<tauri::Wry>,
    active_icon: Image<'static>,
    idle_icon: Image<'static>,
    /// Tooltip last set — saves resetting it once a second.
    last_tooltip: parking_lot::Mutex<String>,
    last_active: std::sync::atomic::AtomicBool,
}

/// Detach the image buffer from the app handle so it can live in `TrayHandles`.
fn owned(icon: &Image<'_>) -> Image<'static> {
    Image::new_owned(icon.rgba().to_vec(), icon.width(), icon.height())
}

/// A tray icon of its own: just the mark, cut out round. The full logo carries
/// the wordmark, and at 16 px that is nothing but a smudge.
const TRAY_PNG: &[u8] = include_bytes!("../icons/tray.png");

/// Greyscale variant of the app icon for "buffer off".
///
/// Works directly on the RGBA data of the already decoded icon — which is why
/// it needs no extra asset.
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
    let open = MenuItem::with_id(app, "open", "Open ClippiBoy", true, None::<&str>)?;
    let toggle = CheckMenuItem::with_id(
        app,
        "toggle",
        "Replay buffer running",
        true,
        false,
        None::<&str>,
    )?;
    let save = MenuItem::with_id(app, "save", "Save clip", false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
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
        log::warn!("could not read the tray icon: {err}");
        app.default_window_icon()
            .map(owned)
            .unwrap_or_else(|| Image::new_owned(vec![0; 4], 1, 1))
    });
    let idle_icon = dimmed(&active_icon);

    TrayIconBuilder::with_id(ID)
        .icon(idle_icon.clone())
        .tooltip("ClippiBoy · buffer off")
        .menu(&menu)
        // Without this a left click on Windows opens the menu, not the window.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main_window(app),
            "toggle" => {
                let app = app.clone();
                std::thread::spawn(move || crate::toggle_buffer_and_notify(&app));
            }
            "save" => {
                let app = app.clone();
                // Muxing takes a moment — do not do it on the menu thread.
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

/// Bring tooltip, icon and menu state in line with the status. Runs once a second.
pub fn refresh(app: &tauri::AppHandle, status: &EngineStatus) {
    let Some(handles) = app.try_state::<TrayHandles>() else {
        return;
    };
    let Some(tray) = app.tray_by_id(ID) else {
        return;
    };

    let tooltip = if status.buffer_active {
        let mut text = format!(
            "ClippiBoy · buffer running — {} s",
            status.buffered_seconds.round() as u32
        );
        if let Some(game) = &status.game {
            text.push_str(" · ");
            text.push_str(game);
        }
        text
    } else {
        match &status.game {
            Some(game) => format!("ClippiBoy · buffer off · {game}"),
            None => "ClippiBoy · buffer off".to_string(),
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
