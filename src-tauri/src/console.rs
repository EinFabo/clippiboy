//! The console over the game — the whole screen, opened by hotkey.
//!
//! A second overlay window beside the banner (`overlay.rs`), and the opposite of
//! it in every way that matters: it takes the focus and it catches clicks. The
//! banner reports, the console is operated.
//!
//! It covers the screen it opens on and lays a dim over it, the way every other
//! game overlay does — one window of one size, which is also why nothing
//! flickers: the window never resizes while it is open, a panel only appears
//! inside it.
//!
//! The task bar keeps its strip: the dock is placed above it, and the page is
//! told how much room that takes.
//!
//! Two things it shares with the banner. It carries the same browser arguments,
//! or WebView2 stops drawing a window that spends its life hidden. And it is
//! content protected, so it never turns up inside a clip saved while it is open.
//!
//! The limit is the banner's as well: over a game in **exclusive** fullscreen no
//! WebView2 window draws. Worse, asking for the focus there drops the game out
//! of fullscreen. So that case is recognised before anything is shown and only
//! says so.

use std::sync::atomic::{AtomicIsize, Ordering};

use tauri::{Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder};

use crate::overlay::{self, BannerKind};
use crate::state::AppState;

pub const LABEL: &str = "console";

/// What the size setting may be turned to. Below the lower end nothing is
/// readable at arm's length; above the upper one the console is the screen.
pub const MIN_SCALE: f64 = 0.8;
pub const MAX_SCALE: f64 = 1.6;

/// The window that had the focus when the console opened — the game, normally.
/// Stored as a raw handle; zero means there is nothing to go back to.
static PREVIOUS: AtomicIsize = AtomicIsize::new(0);

/// Create the console window, hidden. Called once at startup.
pub fn create(app: &tauri::AppHandle) {
    if app.get_webview_window(LABEL).is_some() {
        return;
    }
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("console.html".into()))
        .title("ClippiBoy")
        .inner_size(1280.0, 720.0)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .additional_browser_args(overlay::BROWSER_ARGS)
        // Never inside a clip — the same reasoning as the banner's.
        .content_protected(true)
        .skip_taskbar(true)
        .shadow(false)
        .resizable(false)
        .focused(false)
        .visible(false)
        .build();
    match window {
        Ok(window) => {
            // Alt+Tab, a click on the game, a crashing full screen: whatever
            // takes the focus away, the console has no business staying over
            // the game. It only hides — the focus already went somewhere the
            // user chose, so there is nothing to hand back.
            let handle = window.clone();
            window.on_window_event(move |event| {
                if matches!(event, tauri::WindowEvent::Focused(false)) {
                    let _ = handle.hide();
                    // The page back to its first screen: whoever comes back
                    // wants the console as it opens, not the panel they left
                    // standing an hour ago.
                    let _ = handle.emit("console-closed", ());
                }
            });
        }
        Err(err) => log::error!("could not create the console window: {err}"),
    }
}

pub fn is_open(app: &tauri::AppHandle) -> bool {
    app.get_webview_window(LABEL)
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false)
}

pub fn toggle(app: &tauri::AppHandle) {
    if is_open(app) {
        close(app);
    } else {
        open(app);
    }
}

/// Bring the console up on the screen being played on.
pub fn open(app: &tauri::AppHandle) {
    if !app.state::<AppState>().config_snapshot().console_enabled {
        return;
    }
    if exclusive_fullscreen() {
        log::info!("console not opened — the game is in exclusive fullscreen");
        overlay::show(
            app,
            BannerKind::Info,
            "The console cannot open here",
            Some("The game runs in exclusive fullscreen. Borderless works.".into()),
        );
        return;
    }
    let Some(window) = app.get_webview_window(LABEL) else {
        log::warn!("console window missing");
        return;
    };

    remember_foreground();
    let mut layout = place(&window).unwrap_or_default();
    let _ = window.show();
    let _ = window.set_always_on_top(true);
    let _ = window.set_focus();
    // Once more now that it is up: a hidden window is not always on the monitor
    // it will land on, and its size is then worked out against the wrong one's
    // scaling.
    if let Ok(second) = place(&window) {
        layout = second;
    }
    // The window keeps living between openings, so whatever it showed last time
    // is still on screen. This is where it starts over — and where the page
    // learns how much room the task bar wants.
    let _ = window.emit("console-opened", layout);
}

/// Hide it again and hand the focus back to whatever had it.
pub fn close(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    let _ = window.hide();
    restore_foreground();
}

/// Take on a changed size setting while the console is open.
pub fn rescale(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    if !window.is_visible().unwrap_or(false) {
        return;
    }
    match place(&window) {
        Ok(layout) => {
            let _ = window.emit("console-opened", layout);
        }
        Err(err) => log::warn!("could not resize the console: {err}"),
    }
}

/// What the page has to know about the screen it was just laid over.
#[derive(Clone, Copy, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    /// How many of the page's own pixels the task bar takes at the bottom, so
    /// the dock can sit above it rather than behind it.
    pub bottom_inset: f64,
    /// What the console is drawn at — the page zooms by it.
    pub scale: f64,
}

/// Lay the window over the whole screen being played on.
fn place(window: &tauri::WebviewWindow) -> tauri::Result<Layout> {
    let monitor = match crate::game::foreground_center() {
        Some((x, y)) => window.monitor_from_point(x, y)?,
        None => None,
    }
    .or(window.primary_monitor()?);
    let Some(monitor) = monitor else {
        return Ok(Layout::default());
    };
    // Physical pixels: with two monitors at different scaling, logical values are
    // converted with the wrong factor — the same trap as in `overlay::place`.
    // Physical pixels throughout: with two monitors at different scaling, Tauri
    // converts logical values with the wrong factor — the same trap as in
    // `overlay::place`.
    let config = window.app_handle().state::<AppState>().config_snapshot();
    let wish = config.console_scale.clamp(MIN_SCALE, MAX_SCALE);
    let screen = *monitor.size();
    let origin = *monitor.position();
    window.set_position(PhysicalPosition::new(origin.x, origin.y))?;
    window.set_size(PhysicalSize::new(screen.width, screen.height))?;

    // Whatever the work area leaves out at the bottom is the task bar. Turned
    // into the page's own pixels: those are physical pixels divided by the
    // monitor's scaling, and then by the zoom the page runs at.
    let area = monitor.work_area();
    let used = (area.position.y - origin.y) + area.size.height as i32;
    let inset = (screen.height as i32 - used).max(0) as f64;
    Ok(Layout {
        bottom_inset: inset / (monitor.scale_factor() * wish),
        scale: wish,
    })
}

#[cfg(windows)]
fn remember_foreground() {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    let hwnd = unsafe { GetForegroundWindow() };
    PREVIOUS.store(hwnd.0 as isize, Ordering::SeqCst);
}

#[cfg(windows)]
fn restore_foreground() {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
    let raw = PREVIOUS.swap(0, Ordering::SeqCst);
    if raw == 0 {
        return;
    }
    // Without this the focus lands on the desktop and the game keeps running in
    // the background, deaf to the keyboard.
    unsafe {
        let _ = SetForegroundWindow(HWND(raw as *mut std::ffi::c_void));
    }
}

/// Is a game running in exclusive fullscreen right now?
///
/// Windows answers this for anyone who asks: the notification state it reports
/// while a Direct3D application owns the screen exclusively is exactly the case
/// where no window of ours can be drawn over it.
#[cfg(windows)]
pub fn exclusive_fullscreen() -> bool {
    use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_RUNNING_D3D_FULL_SCREEN};
    unsafe {
        SHQueryUserNotificationState()
            .map(|state| state == QUNS_RUNNING_D3D_FULL_SCREEN)
            .unwrap_or(false)
    }
}

#[cfg(not(windows))]
fn remember_foreground() {}

#[cfg(not(windows))]
fn restore_foreground() {
    PREVIOUS.store(0, Ordering::SeqCst);
}

#[cfg(not(windows))]
pub fn exclusive_fullscreen() -> bool {
    false
}
