//! Capture sources: monitors and visible windows.
//!
//! The actual frame capture (phase 1) runs through Windows.Graphics.Capture;
//! this module only determines the selectable targets. The ids are chosen so
//! they can later be resolved straight back into a capture handle (monitor:
//! device name, window: HWND as hex).

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

    /// MONITORINFOF_PRIMARY — not exported by the windows crate 0.58.
    const PRIMARY_FLAG: u32 = 0x0000_0001;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible,
    };

    /// A screen's refresh rate in hertz.
    ///
    /// `szDevice` is the device name (`\\.\DISPLAY1`), null-terminated. Windows
    /// reports the rate as a whole number; per the docs 0 and 1 mean "the
    /// driver's default rate", i.e. unknown.
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

    /// The rate of the screen the window mostly sits on. A window has no rate
    /// of its own — but capture still runs at the cadence of the screen
    /// underneath it.
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
        // Tiny helper windows are of no interest.
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

/// Is the secure desktop in front — lock screen, sign-in, UAC prompt?
///
/// There is no plain "is the session locked" call for a process without a window
/// and a message pump. `OpenInputDesktop` hands back the desktop that currently
/// receives input, and on the secure desktop that is Winlogon's, which a normal
/// user process may not open. The failure is the signal.
///
/// It matters because Windows.Graphics.Capture goes quiet there rather than
/// reporting anything: no frame arrives, the clock keeps re-sending the last one
/// it had (see `convert::pace`), and the ring quietly fills with a still. After
/// a reboot that still was the lock screen, and every clip saved afterwards was
/// ninety seconds of it.
#[cfg(windows)]
pub fn secure_desktop() -> bool {
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, OpenInputDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS,
    };
    unsafe {
        match OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) {
            Ok(desktop) => {
                let _ = CloseDesktop(desktop);
                false
            }
            // Anything but success counts as "not ours": whatever is in front,
            // it is not a desktop this process can record.
            Err(_) => true,
        }
    }
}

#[cfg(not(windows))]
pub fn secure_desktop() -> bool {
    false
}

// The foreground window is evaluated in `game.rs` — what matters there is not
// just the title but the process behind it.

#[cfg(not(windows))]
pub fn list_targets() -> Vec<CaptureTarget> {
    let _ = TargetKind::Monitor;
    Vec::new()
}

/// Find the configured target in the current list. With no selection the
/// primary monitor applies — exactly as in the pipeline.
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

/// The frame rates that suit a screen with this refresh rate.
///
/// Capturing more frames than the screen puts out does not make any motion
/// smoother — it only produces duplicate frames. Hence the usual steps up to
/// the screen's rate, plus that rate itself, so a 165 Hz panel really can offer
/// 165.
pub fn fps_choices(refresh_hz: Option<u32>) -> Vec<u32> {
    const STEPS: [u32; 3] = [30, 60, 120];
    let Some(refresh) = refresh_hz.filter(|hz| *hz >= 20) else {
        return STEPS.to_vec();
    };
    let mut choices: Vec<u32> = STEPS.into_iter().filter(|fps| *fps < refresh).collect();
    choices.push(refresh);
    choices
}

/// Snap a frame rate onto one of the offered steps — downwards, because
/// capturing more than was configured would be a surprise.
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

/// Fit capture size and frame rate to the source.
///
/// Upscaling does not make a frame show a detail the screen does not have — it
/// only costs bitrate. So the height is capped at the source's, and the width
/// is always computed from the source's aspect ratio instead of assuming 16:9.
/// Both values stay even, otherwise H.264 will not accept them.
///
/// The same reasoning applies to the frame rate: above the screen's refresh
/// rate only duplicate frames would come out. It also lands on one of the
/// offered steps — the UI shows a fixed set of choices, and a value beside them
/// would have no entry there.
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
    // The bitrate follows the picture rather than standing next to it. It is no
    // longer a setting anyone makes by hand — the encoder aims at a quality —
    // but it still sizes the memory budget and still governs on an encoder that
    // refuses constant quality. A fixed number would mean 720p30 kept a budget
    // meant for four times the pixels per second.
    recording.bitrate_kbps =
        crate::encode::bitrate_for(recording.width, recording.height, recording.fps);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With no known refresh rate the usual steps stand — better to offer 120
    /// than to cap at 60 on a 240 Hz panel.
    #[test]
    fn unknown_refresh_keeps_the_usual_steps() {
        assert_eq!(fps_choices(None), vec![30, 60, 120]);
        assert_eq!(fps_choices(Some(0)), vec![30, 60, 120]);
    }

    /// Anything between two steps slides down to the lower one — rounding up
    /// would mean capturing more than was configured. And whatever fits nowhere
    /// lands on the smallest step rather than on none.
    #[test]
    fn a_rate_between_two_steps_drops_to_the_lower_one() {
        let on_165 = fps_choices(Some(165));
        assert_eq!(snap_fps(100, &on_165), 60);
        assert_eq!(snap_fps(165, &on_165), 165);

        let on_60 = fps_choices(Some(60));
        assert_eq!(snap_fps(120, &on_60), 60);
        assert_eq!(snap_fps(24, &on_60), 30);
    }

    /// The screen's own rate is always on offer, odd numbers included.
    #[test]
    fn the_screen_rate_is_always_offered() {
        assert_eq!(fps_choices(Some(60)), vec![30, 60]);
        assert_eq!(fps_choices(Some(75)), vec![30, 60, 75]);
        assert_eq!(fps_choices(Some(144)), vec![30, 60, 120, 144]);
        assert_eq!(fps_choices(Some(240)), vec![30, 60, 120, 240]);
    }
}
