//! The banner shown over the game — Medal/ShadowPlay style.
//!
//! Technically a second window: borderless, transparent, always on top, taking
//! no focus and catching no mouse clicks. As long as there is nothing to show,
//! it stays hidden.
//!
//! The limit: over a game in *exclusive* fullscreen a WebView2 window cannot
//! draw — that would need a present hook inside the game process, and that is
//! exactly what ClippiBoy deliberately does not do. In borderless fullscreen and
//! in windowed mode, so in practically every current game, it works.

use std::sync::atomic::{AtomicU64, Ordering};

use tauri::{
    Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder,
};

use crate::model::{OverlayConfig, OverlayCorner};
use crate::state::AppState;

pub const LABEL: &str = "overlay";

// A little larger than the card: the outline's glow needs margin, otherwise the
// window edge clips it.
const WIDTH: f64 = 416.0;
const HEIGHT: f64 = 128.0;
const MARGIN: f64 = 24.0;

/// Which kind of message — decides the banner's colour and which switch in the
/// settings turns it off.
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BannerKind {
    Clip,
    Buffer,
    /// Buffer off. A kind of its own so the outline can run backwards and grey
    /// out along the way — otherwise on and off look identical.
    BufferOff,
    Error,
    Info,
    /// A picture, not a recording — its own kind so the banner can say so
    /// without reading the text.
    Screenshot,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Banner {
    pub kind: BannerKind,
    pub title: String,
    pub detail: Option<String>,
    pub thumb_path: Option<String>,
    pub duration_ms: u32,
}

/// Counts the banners shown so a late hide order does not clear away a banner
/// that has appeared in the meantime.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Create the overlay window. Called once at startup.
pub fn create(app: &tauri::AppHandle) {
    if app.get_webview_window(LABEL).is_some() {
        return;
    }
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("overlay.html".into()))
        .title("ClippiBoy Overlay")
        .inner_size(WIDTH, HEIGHT)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        // The banner belongs on the screen, not in the clip. Windows excludes a
        // window with this display affinity from every screen capture — including
        // Windows.Graphics.Capture, which is what records here. As a result it is
        // only visible live.
        .content_protected(true)
        .skip_taskbar(true)
        .shadow(false)
        .resizable(false)
        .focused(false)
        .visible(false)
        .build();

    match window {
        Ok(window) => {
            // Clicks pass through the window into the game.
            if let Err(err) = window.set_ignore_cursor_events(true) {
                log::warn!("Overlay bleibt klickbar: {err}");
            }
            // Place it once now: nothing should be computed or moved when it
            // fades in.
            reposition(app);
        }
        Err(err) => log::error!("could not create the overlay window: {err}"),
    }
}

/// Put the banner at the configured spot. Called at startup and after every
/// change to the settings — not for every banner.
pub fn reposition(app: &tauri::AppHandle) {
    let config = app.state::<AppState>().config_snapshot().overlay;
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    if let Err(err) = place(&window, &config) {
        log::warn!("could not position the overlay: {err}");
    }
}

/// Show the banner, provided it is not switched off in the settings.
pub fn show(app: &tauri::AppHandle, kind: BannerKind, title: impl Into<String>, detail: Option<String>) {
    show_with_thumb(app, kind, title, detail, None);
}

pub fn show_with_thumb(
    app: &tauri::AppHandle,
    kind: BannerKind,
    title: impl Into<String>,
    detail: Option<String>,
    thumb_path: Option<String>,
) {
    let config = app.state::<AppState>().config_snapshot().overlay;
    if !config.enabled {
        return;
    }
    let wanted = match kind {
        BannerKind::Clip => config.on_clip_saved,
        BannerKind::Buffer | BannerKind::BufferOff => config.on_buffer_toggle,
        BannerKind::Error => config.on_error,
        BannerKind::Screenshot => config.on_screenshot,
        BannerKind::Info => true,
    };
    if !wanted {
        return;
    }

    // Errors stay longer, otherwise they are simply missed while playing.
    let duration_ms = if kind == BannerKind::Error {
        config.duration_ms.max(6000)
    } else {
        config.duration_ms
    };

    let Some(window) = app.get_webview_window(LABEL) else {
        log::warn!("overlay window missing — dropping the banner");
        return;
    };

    // With a fixed screen the position already stands — that saves a resize plus
    // a move on fade-in, and with it visible stutter.
    if config.follow_active_screen {
        if let Err(err) = place(&window, &config) {
            log::warn!("could not position the overlay: {err}");
        }
    }
    let _ = window.show();
    // Bring it to the top again after showing: games like to grab the z-order
    // when they switch modes.
    let _ = window.set_always_on_top(true);

    let banner = Banner {
        kind,
        title: title.into(),
        detail,
        thumb_path,
        duration_ms,
    };
    let _ = window.emit("overlay-banner", banner);

    // Hide once the banner has faded out (plus time for the animation).
    let ticket = SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1;
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(duration_ms as u64 + 600));
        if SEQUENCE.load(Ordering::SeqCst) != ticket {
            return; // A new banner has arrived in the meantime.
        }
        if let Some(window) = handle.get_webview_window(LABEL) {
            let _ = window.hide();
        }
    });
}

/// Put the window in the desired corner — on the monitor currently being played
/// on, otherwise on the primary one.
fn place(window: &tauri::WebviewWindow, config: &OverlayConfig) -> tauri::Result<()> {
    let chosen = if config.follow_active_screen {
        match crate::game::foreground_center() {
            Some((x, y)) => window.monitor_from_point(x, y)?,
            None => None,
        }
    } else if let Some(wanted) = &config.monitor {
        window
            .available_monitors()?
            .into_iter()
            .find(|m| m.name().map(|n| n == wanted).unwrap_or(false))
    } else {
        None
    };
    let Some(monitor) = chosen.or(window.primary_monitor()?) else {
        return Ok(());
    };
    let corner = config.corner;
    // Physical pixels, not logical: with two monitors at different scaling,
    // Tauri converts logical values using the *old* monitor's factor — the banner
    // then lands beside where it should.
    // Work area instead of monitor size — otherwise the banner sits behind the
    // taskbar at the bottom.
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let origin = area.position;
    let size = area.size;
    let width = (WIDTH * scale).round() as i32;
    let height = (HEIGHT * scale).round() as i32;
    let margin = (MARGIN * scale).round() as i32;
    let (right, bottom) = (
        size.width as i32 - width - margin,
        size.height as i32 - height - margin,
    );

    let (x, y) = match corner {
        OverlayCorner::TopLeft => (margin, margin),
        OverlayCorner::TopRight => (right, margin),
        OverlayCorner::BottomLeft => (margin, bottom),
        OverlayCorner::BottomRight => (right, bottom),
    };
    window.set_size(PhysicalSize::new(width, height))?;
    window.set_position(PhysicalPosition::new(origin.x + x, origin.y + y))
}
