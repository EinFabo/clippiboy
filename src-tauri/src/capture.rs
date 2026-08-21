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
    use windows::core::PCWSTR;
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, MonitorFromWindow, DEVMODEW,
        ENUM_CURRENT_SETTINGS, HDC, HMONITOR, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
    };

    /// MONITORINFOF_PRIMARY — im windows-Crate 0.58 nicht exportiert.
    const PRIMARY_FLAG: u32 = 0x0000_0001;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible,
    };

    /// Bildwiederholrate eines Bildschirms in Hertz.
    ///
    /// `szDevice` ist der Gerätename (`\\.\DISPLAY1`), nullterminiert. Windows
    /// meldet die Rate ganzzahlig; laut Doku stehen 0 und 1 für „Standardrate
    /// des Treibers", also für: unbekannt.
    fn refresh_of_device(device: &[u16]) -> Option<u32> {
        let mut mode = DEVMODEW {
            dmSize: std::mem::size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        let ok = unsafe {
            EnumDisplaySettingsW(
                PCWSTR(device.as_ptr()),
                ENUM_CURRENT_SETTINGS,
                &mut mode as *mut _,
            )
        };
        if !ok.as_bool() {
            return None;
        }
        match mode.dmDisplayFrequency {
            0 | 1 => None,
            hz => Some(hz),
        }
    }

    /// Die Rate des Bildschirms, auf dem das Fenster überwiegend liegt. Ein
    /// Fenster hat keine eigene — aufgenommen wird trotzdem im Takt des
    /// Bildschirms darunter.
    unsafe fn refresh_of_window(hwnd: HWND) -> Option<u32> {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if monitor.0.is_null() {
            return None;
        }
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(monitor, &mut info.monitorInfo as *mut _).as_bool() {
            return None;
        }
        refresh_of_device(&info.szDevice)
    }

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
            let refresh_hz = refresh_of_device(&info.szDevice);
            let index = out.len() + 1;

            out.push(CaptureTarget {
                kind: TargetKind::Monitor,
                id: device,
                title: format!("Monitor {index} — {width}×{height}"),
                width,
                height,
                is_primary,
                refresh_hz,
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
            refresh_hz: refresh_of_window(hwnd),
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

/// Das eingestellte Ziel in der aktuellen Liste finden. Ohne Auswahl gilt der
/// primäre Monitor — genau wie in der Pipeline.
fn find_target(kind: TargetKind, id: Option<&str>) -> Option<CaptureTarget> {
    let targets = list_targets();
    let hit = match id {
        Some(id) => targets.iter().find(|t| t.kind == kind && t.id == id),
        None => targets
            .iter()
            .find(|t| t.kind == TargetKind::Monitor && t.is_primary)
            .or_else(|| targets.iter().find(|t| t.kind == TargetKind::Monitor)),
    }?;
    Some(hit.clone())
}

/// Die Bildraten, die zu einem Bildschirm mit dieser Wiederholrate passen.
///
/// Mehr Bilder aufzunehmen, als der Bildschirm ausgibt, bringt keine Bewegung
/// dazu, flüssiger zu sein — es entstehen nur doppelte Bilder. Deshalb die
/// üblichen Stufen bis zur Rate des Bildschirms und die Rate selbst obendrauf,
/// damit ein 165-Hz-Panel auch wirklich 165 anbieten kann.
pub fn fps_choices(refresh_hz: Option<u32>) -> Vec<u32> {
    const STEPS: [u32; 3] = [30, 60, 120];
    let Some(refresh) = refresh_hz.filter(|hz| *hz >= 20) else {
        return STEPS.to_vec();
    };
    let mut choices: Vec<u32> = STEPS.into_iter().filter(|fps| *fps < refresh).collect();
    choices.push(refresh);
    choices
}

/// Eine Bildrate auf eine der angebotenen Stufen bringen — nach unten, denn
/// mehr aufzunehmen als eingestellt wäre eine Überraschung.
fn snap_fps(fps: u32, choices: &[u32]) -> u32 {
    if choices.contains(&fps) {
        return fps;
    }
    choices
        .iter()
        .rev()
        .find(|step| **step <= fps)
        .copied()
        .unwrap_or_else(|| choices.first().copied().unwrap_or(fps))
}

/// Aufnahmegröße und Bildrate an die Quelle angleichen.
///
/// Hochskalieren bringt kein Bild dazu, ein Detail zu zeigen, das der
/// Bildschirm nicht hat — es kostet nur Bitrate. Deshalb wird die Höhe auf die
/// der Quelle gedeckelt und die Breite immer aus deren Seitenverhältnis
/// gerechnet, statt 16:9 anzunehmen. Beide Werte bleiben gerade, sonst nimmt
/// H.264 sie nicht an.
///
/// Dieselbe Überlegung gilt für die Bildrate: Über der Wiederholrate des
/// Bildschirms kämen nur doppelte Bilder heraus. Sie landet außerdem auf einer
/// der angebotenen Stufen — in der Oberfläche steht eine Auswahl, und ein Wert
/// daneben hätte dort keinen Eintrag.
pub fn fit_to_target(recording: &mut crate::model::RecordingConfig) {
    let Some(target) = find_target(recording.target_kind, recording.target_id.as_deref()) else {
        return;
    };
    recording.fps = snap_fps(recording.fps, &fps_choices(target.refresh_hz));
    let (native_width, native_height) = match (target.width, target.height) {
        (0, _) | (_, 0) => return,
        size => size,
    };
    let height = recording.height.clamp(2, native_height);
    let width = (height as u64 * native_width as u64 / native_height as u64) as u32;
    recording.height = height & !1;
    recording.width = width.max(2) & !1;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ohne bekannte Wiederholrate bleibt es bei den üblichen Stufen — lieber
    /// 120 anbieten, als auf einem 240-Hz-Panel bei 60 zu deckeln.
    #[test]
    fn unknown_refresh_keeps_the_usual_steps() {
        assert_eq!(fps_choices(None), vec![30, 60, 120]);
        assert_eq!(fps_choices(Some(0)), vec![30, 60, 120]);
    }

    /// Was zwischen zwei Stufen liegt, rutscht auf die darunter — nach oben
    /// zu runden hieße, mehr aufzunehmen als eingestellt. Und was gar nicht
    /// mehr passt, landet auf der kleinsten Stufe statt auf keiner.
    #[test]
    fn a_rate_between_two_steps_drops_to_the_lower_one() {
        let on_165 = fps_choices(Some(165));
        assert_eq!(snap_fps(100, &on_165), 60);
        assert_eq!(snap_fps(165, &on_165), 165);

        let on_60 = fps_choices(Some(60));
        assert_eq!(snap_fps(120, &on_60), 60);
        assert_eq!(snap_fps(24, &on_60), 30);
    }

    /// Die Rate des Bildschirms steht immer zur Auswahl, auch die krummen.
    #[test]
    fn the_screen_rate_is_always_offered() {
        assert_eq!(fps_choices(Some(60)), vec![30, 60]);
        assert_eq!(fps_choices(Some(75)), vec![30, 60, 75]);
        assert_eq!(fps_choices(Some(144)), vec![30, 60, 120, 144]);
        assert_eq!(fps_choices(Some(240)), vec![30, 60, 120, 240]);
    }
}
