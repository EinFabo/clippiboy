//! Der Banner, der über dem Spiel eingeblendet wird — Medal/ShadowPlay-Stil.
//!
//! Technisch ein zweites, randloses und durchsichtiges Fenster, das immer oben
//! liegt, keinen Fokus annimmt und keine Mausklicks abfängt. Solange nichts
//! anzuzeigen ist, bleibt es versteckt.
//!
//! Grenze: über einem Spiel im *exklusiven* Vollbild kann ein WebView2-Fenster
//! nicht zeichnen — dafür bräuchte es einen Present-Hook im Spielprozess, und
//! genau das macht ClippiBoy bewusst nicht. Im randlosen Vollbild und im
//! Fenstermodus, also bei praktisch allen aktuellen Spielen, klappt es.

use std::sync::atomic::{AtomicU64, Ordering};

use tauri::{
    Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder,
};

use crate::model::{OverlayConfig, OverlayCorner};
use crate::state::AppState;

pub const LABEL: &str = "overlay";

// Etwas größer als die Karte: der Glow der Umrandung braucht Rand, sonst
// schneidet ihn der Fensterrand ab.
const WIDTH: f64 = 416.0;
const HEIGHT: f64 = 128.0;
const MARGIN: f64 = 24.0;

/// Welche Art von Meldung — entscheidet über Farbe im Banner und darüber,
/// welcher Schalter in den Einstellungen sie abschaltet.
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BannerKind {
    Clip,
    Buffer,
    /// Puffer aus. Eigene Art, damit die Umrandung rückwärts laufen und dabei
    /// ausgrauen kann — sonst sieht An und Aus identisch aus.
    BufferOff,
    Error,
    Info,
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

/// Zählt die gezeigten Banner mit, damit ein nachgereichter Verstecken-Auftrag
/// einen inzwischen neu erschienenen Banner nicht wegräumt.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Das Overlay-Fenster anlegen. Wird beim Start einmal aufgerufen.
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
        // Das Banner gehört auf den Bildschirm, nicht in den Clip. Windows
        // nimmt ein Fenster mit dieser Anzeigezugehörigkeit aus jeder
        // Bildschirmaufnahme heraus — auch aus Windows.Graphics.Capture, mit
        // dem hier aufgenommen wird. Zu sehen ist es dadurch nur noch live.
        .content_protected(true)
        .skip_taskbar(true)
        .shadow(false)
        .resizable(false)
        .focused(false)
        .visible(false)
        .build();

    match window {
        Ok(window) => {
            // Klicks gehen durch das Fenster hindurch ins Spiel.
            if let Err(err) = window.set_ignore_cursor_events(true) {
                log::warn!("Overlay bleibt klickbar: {err}");
            }
            // Einmal jetzt platzieren: beim Einblenden soll nichts mehr
            // gerechnet oder verschoben werden.
            reposition(app);
        }
        Err(err) => log::error!("Overlay-Fenster konnte nicht angelegt werden: {err}"),
    }
}

/// Banner an die eingestellte Stelle setzen. Beim Start und nach jeder
/// Änderung an den Einstellungen aufgerufen — nicht bei jedem Banner.
pub fn reposition(app: &tauri::AppHandle) {
    let config = app.state::<AppState>().config_snapshot().overlay;
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    if let Err(err) = place(&window, &config) {
        log::warn!("Overlay konnte nicht positioniert werden: {err}");
    }
}

/// Banner anzeigen, sofern er in den Einstellungen nicht abgeschaltet ist.
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
        BannerKind::Info => true,
    };
    if !wanted {
        return;
    }

    // Fehler stehen länger, sie sind sonst im Spiel schlicht zu übersehen.
    let duration_ms = if kind == BannerKind::Error {
        config.duration_ms.max(6000)
    } else {
        config.duration_ms
    };

    let Some(window) = app.get_webview_window(LABEL) else {
        log::warn!("Overlay-Fenster fehlt — Banner wird verworfen");
        return;
    };

    // Bei festem Bildschirm steht die Position schon — das spart beim
    // Einblenden ein Resize plus Move und damit sichtbares Ruckeln.
    if config.follow_active_screen {
        if let Err(err) = place(&window, &config) {
            log::warn!("Overlay konnte nicht positioniert werden: {err}");
        }
    }
    let _ = window.show();
    // Nach dem Anzeigen erneut nach oben holen: Spiele reißen die Z-Reihenfolge
    // beim Moduswechsel gern an sich.
    let _ = window.set_always_on_top(true);

    let banner = Banner {
        kind,
        title: title.into(),
        detail,
        thumb_path,
        duration_ms,
    };
    let _ = window.emit("overlay-banner", banner);

    // Verstecken, sobald der Banner ausgeblendet ist (plus Zeit für die Animation).
    let ticket = SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1;
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(duration_ms as u64 + 600));
        if SEQUENCE.load(Ordering::SeqCst) != ticket {
            return; // Inzwischen kam ein neuer Banner.
        }
        if let Some(window) = handle.get_webview_window(LABEL) {
            let _ = window.hide();
        }
    });
}

/// Fenster in die gewünschte Ecke setzen — auf dem Monitor, auf dem gerade
/// gespielt wird, sonst auf dem primären.
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
    // Physische Pixel, nicht logische: bei zwei Monitoren mit verschiedener
    // Skalierung rechnet Tauri logische Angaben mit dem Faktor des *alten*
    // Monitors um — der Banner landet dann daneben.
    // Arbeitsbereich statt Monitorgröße — sonst liegt der Banner unten hinter
    // der Taskleiste.
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
