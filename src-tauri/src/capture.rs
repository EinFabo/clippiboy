//! Aufnahmequellen: Monitore und sichtbare Fenster.
//!
//! Die eigentliche Bildaufnahme (Phase 1) läuft über Windows.Graphics.Capture;
//! hier werden nur die auswählbaren Ziele ermittelt. Die IDs sind so gewählt,
//! dass sie später direkt wieder in ein Capture-Handle aufgelöst werden können
//! (Monitor: Gerätename, Fenster: HWND als Hex).

use crate::model::{CaptureTarget, TargetKind};

#[cfg(windows)]
mod win {
    use super::*;
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, TRUE};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
    };

    /// MONITORINFOF_PRIMARY — im windows-Crate 0.58 nicht exportiert.
    const PRIMARY_FLAG: u32 = 0x0000_0001;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible,
    };

    unsafe extern "system" fn monitor_proc(
        monitor: HMONITOR,
        _dc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let out = &mut *(data.0 as *mut Vec<CaptureTarget>);
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;

        if GetMonitorInfoW(monitor, &mut info.monitorInfo as *mut _).as_bool() {
            let rect = info.monitorInfo.rcMonitor;
            let width = (rect.right - rect.left) as u32;
            let height = (rect.bottom - rect.top) as u32;
            let device = String::from_utf16_lossy(&info.szDevice)
                .trim_end_matches('\0')
                .to_string();
            let is_primary = info.monitorInfo.dwFlags & PRIMARY_FLAG != 0;
            let index = out.len() + 1;

            out.push(CaptureTarget {
                kind: TargetKind::Monitor,
                id: device,
                title: format!("Monitor {index} — {width}×{height}"),
                width,
                height,
                is_primary,
            });
        }
        TRUE
    }

    unsafe extern "system" fn window_proc(hwnd: HWND, data: LPARAM) -> BOOL {
        let out = &mut *(data.0 as *mut Vec<CaptureTarget>);

        if !IsWindowVisible(hwnd).as_bool() {
            return TRUE;
        }
        let len = GetWindowTextLengthW(hwnd);
        if len == 0 {
            return TRUE;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let written = GetWindowTextW(hwnd, &mut buf);
        if written == 0 {
            return TRUE;
        }
        let title = String::from_utf16_lossy(&buf[..written as usize]);

        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return TRUE;
        }
        let width = (rect.right - rect.left) as u32;
        let height = (rect.bottom - rect.top) as u32;
        // Winzige Hilfsfenster interessieren nicht.
        if width < 320 || height < 240 {
            return TRUE;
        }

        out.push(CaptureTarget {
            kind: TargetKind::Window,
            id: format!("0x{:X}", hwnd.0 as usize),
            title,
            width,
            height,
            is_primary: false,
        });
        TRUE
    }

    pub fn list_targets() -> Vec<CaptureTarget> {
        let mut monitors: Vec<CaptureTarget> = Vec::new();
        let mut windows: Vec<CaptureTarget> = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(
                None,
                None,
                Some(monitor_proc),
                LPARAM(&mut monitors as *mut _ as isize),
            );
            let _ = EnumWindows(
                Some(window_proc),
                LPARAM(&mut windows as *mut _ as isize),
            );
        }
        monitors.extend(windows);
        monitors
    }
}

#[cfg(windows)]
pub fn list_targets() -> Vec<CaptureTarget> {
    win::list_targets()
}

// Das Vordergrundfenster wird in `game.rs` ausgewertet — dort geht es nicht nur
// um den Titel, sondern um den Prozess dahinter.

#[cfg(not(windows))]
pub fn list_targets() -> Vec<CaptureTarget> {
    let _ = TargetKind::Monitor;
    Vec::new()
}

/// Native Größe des eingestellten Ziels. `None`, wenn es gerade nicht
/// auffindbar ist — dann bleibt die eingestellte Auflösung unangetastet.
pub fn target_size(kind: TargetKind, id: Option<&str>) -> Option<(u32, u32)> {
    let targets = list_targets();
    let hit = match id {
        Some(id) => targets.iter().find(|t| t.kind == kind && t.id == id),
        // Ohne Auswahl nimmt die Pipeline den primären Monitor.
        None => targets
            .iter()
            .find(|t| t.kind == TargetKind::Monitor && t.is_primary)
            .or_else(|| targets.iter().find(|t| t.kind == TargetKind::Monitor)),
    }?;
    match (hit.width, hit.height) {
        (0, _) | (_, 0) => None,
        size => Some(size),
    }
}

/// Aufnahmegröße an die Quelle angleichen.
///
/// Hochskalieren bringt kein Bild dazu, ein Detail zu zeigen, das der
/// Bildschirm nicht hat — es kostet nur Bitrate. Deshalb wird die Höhe auf die
/// der Quelle gedeckelt und die Breite immer aus deren Seitenverhältnis
/// gerechnet, statt 16:9 anzunehmen. Beide Werte bleiben gerade, sonst nimmt
/// H.264 sie nicht an.
pub fn fit_to_target(recording: &mut crate::model::RecordingConfig) {
    let Some((native_width, native_height)) =
        target_size(recording.target_kind, recording.target_id.as_deref())
    else {
        return;
    };
    let height = recording.height.clamp(2, native_height);
    let width = (height as u64 * native_width as u64 / native_height as u64) as u32;
    recording.height = height & !1;
    recording.width = width.max(2) & !1;
}
