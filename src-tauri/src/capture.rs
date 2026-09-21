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
    use windows::Win32::Devices::Display::{
        DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
        DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
        DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
        DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
    };
    use windows::Win32::Foundation::{ERROR_SUCCESS, WIN32_ERROR};
    use std::collections::HashMap;

    /// What Windows knows about the panel behind a `\\.\DISPLAYn`.
    pub struct Identity {
        /// The name on the box — "E2212F" rather than "Generic PnP Monitor".
        pub friendly: String,
        /// The device interface path, tied to the panel and the connector it
        /// hangs on. This is what survives a reboot.
        pub path: String,
    }

    /// Fixed-length wide field → String, stopping at the first NUL.
    fn wide_to_string(field: &[u16]) -> String {
        let len = field.iter().position(|c| *c == 0).unwrap_or(field.len());
        String::from_utf16_lossy(&field[..len])
    }

    /// Everything Windows will say about the monitors, keyed by GDI device name.
    ///
    /// `QueryDisplayConfig` is the one call that hands back both halves at once:
    /// the source side carries the `\\.\DISPLAYn` that `MONITORINFOEXW` also
    /// reports, and the target side carries the panel's own name and its device
    /// path. That pairing is the whole point — without it there is no way to say
    /// which of three identical 1920×1080 screens the saved one was.
    ///
    /// Anything unexpected gives an empty map and every caller carries on with
    /// the device name alone, exactly as before. A screen that Windows will not
    /// describe must not become a screen that cannot be recorded.
    pub fn identities() -> HashMap<String, Identity> {
        let mut out = HashMap::new();
        unsafe {
            let mut path_count = 0u32;
            let mut mode_count = 0u32;
            if GetDisplayConfigBufferSizes(
                QDC_ONLY_ACTIVE_PATHS,
                &mut path_count,
                &mut mode_count,
            ) != ERROR_SUCCESS
            {
                return out;
            }
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
            let result: WIN32_ERROR = QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                None,
            );
            if result != ERROR_SUCCESS {
                return out;
            }
            // The counts may come back smaller than the buffers asked for.
            paths.truncate(path_count as usize);

            for path in &paths {
                let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                    header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                        size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                        adapterId: path.sourceInfo.adapterId,
                        id: path.sourceInfo.id,
                    },
                    ..Default::default()
                };
                if DisplayConfigGetDeviceInfo(&mut source.header) != ERROR_SUCCESS.0 as i32 {
                    continue;
                }
                let device = wide_to_string(&source.viewGdiDeviceName);
                if device.is_empty() {
                    continue;
                }

                let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
                    header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                        size: std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                        adapterId: path.targetInfo.adapterId,
                        id: path.targetInfo.id,
                    },
                    ..Default::default()
                };
                if DisplayConfigGetDeviceInfo(&mut target.header) != ERROR_SUCCESS.0 as i32 {
                    continue;
                }
                let path_id = wide_to_string(&target.monitorDevicePath);
                if path_id.is_empty() {
                    continue;
                }
                out.insert(
                    device,
                    Identity {
                        friendly: wide_to_string(&target.monitorFriendlyDeviceName),
                        path: path_id,
                    },
                );
            }
        }
        out
    }

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

    /// What `monitor_proc` writes into and reads from.
    struct Enumeration<'a> {
        out: &'a mut Vec<CaptureTarget>,
        /// Looked up once for the whole run rather than per monitor —
        /// `QueryDisplayConfig` describes every screen in one go.
        identities: &'a HashMap<String, Identity>,
    }

    unsafe extern "system" fn monitor_proc(
        monitor: HMONITOR,
        _dc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let state = &mut *(data.0 as *mut Enumeration);
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
            let index = state.out.len() + 1;
            let identity = state.identities.get(&device);

            // The panel's own name if Windows gives one. Three screens of the
            // same size are "Monitor 1/2/3" otherwise, and that number comes
            // from enumeration order — it can mean a different screen tomorrow.
            let label = identity
                .map(|id| id.friendly.trim())
                .filter(|name| !name.is_empty())
                .map(|name| name.to_string())
                .unwrap_or_else(|| format!("Monitor {index}"));

            state.out.push(CaptureTarget {
                kind: TargetKind::Monitor,
                id: device,
                stable_id: identity.map(|id| id.path.clone()),
                title: format!("{label} \u{2014} {width}\u{d7}{height}"),
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
            // A window has no panel behind it, and its handle is new with every
            // run of the program anyway.
            stable_id: None,
            title,
            width,
            height,
            is_primary: false,
            refresh_hz: refresh_of_window(hwnd),
        });
        TRUE
    }

    pub fn list_monitors() -> Vec<CaptureTarget> {
        let mut monitors: Vec<CaptureTarget> = Vec::new();
        let identities = identities();
        let mut state = Enumeration {
            out: &mut monitors,
            identities: &identities,
        };
        unsafe {
            let _ = EnumDisplayMonitors(
                None,
                None,
                Some(monitor_proc),
                LPARAM(&mut state as *mut _ as isize),
            );
        }
        monitors
    }

    pub fn list_targets() -> Vec<CaptureTarget> {
        let mut targets = list_monitors();
        let mut windows: Vec<CaptureTarget> = Vec::new();
        unsafe {
            let _ = EnumWindows(
                Some(window_proc),
                LPARAM(&mut windows as *mut _ as isize),
            );
        }
        targets.extend(windows);
        targets
    }
}

#[cfg(windows)]
pub fn list_targets() -> Vec<CaptureTarget> {
    win::list_targets()
}

/// The screens alone. What the screen watcher asks every two seconds — walking
/// every open window for that as well would be work thrown away.
#[cfg(windows)]
pub fn list_monitors() -> Vec<CaptureTarget> {
    win::list_monitors()
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

#[cfg(not(windows))]
pub fn list_monitors() -> Vec<CaptureTarget> {
    Vec::new()
}

/// How a saved selection was matched against the screens actually present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Match {
    /// Found by the panel's own identity — right even if the device name moved.
    Stable,
    /// Found by the device name.
    Device,
    /// Nothing was ever chosen; the primary screen is simply the default.
    Default,
    /// Something *was* chosen and it is not here. The primary screen stands in.
    Fallback,
}

/// The chosen target and how it was arrived at.
#[derive(Debug, Clone)]
pub struct Choice {
    pub target: CaptureTarget,
    pub how: Match,
}

/// Work out which of these targets the saved selection means.
///
/// The order is the whole fix for "after a reboot it records the wrong screen":
/// the panel's identity outranks the device name, because `\\.\DISPLAY2` is
/// handed out by enumeration order at boot and can land on a different screen
/// tomorrow, while the device path stays with the panel.
///
/// A pure function over a list, so the rules are testable without Windows —
/// the same reason `game::resolve_name` is one.
pub fn pick(
    targets: &[CaptureTarget],
    kind: TargetKind,
    id: Option<&str>,
    stable: Option<&str>,
) -> Option<Choice> {
    let primary = || {
        targets
            .iter()
            .find(|t| t.kind == TargetKind::Monitor && t.is_primary)
            .or_else(|| targets.iter().find(|t| t.kind == TargetKind::Monitor))
    };

    // A window is only ever its handle, and a handle is new with every run of
    // the program that owns it. There is nothing stable to fall back on, and
    // standing in with a whole screen would be a surprise, not a rescue.
    if kind == TargetKind::Window {
        return targets
            .iter()
            .find(|t| t.kind == TargetKind::Window && Some(t.id.as_str()) == id)
            .map(|target| Choice { target: target.clone(), how: Match::Device });
    }

    if let Some(stable) = stable.filter(|s| !s.is_empty()) {
        if let Some(target) = targets
            .iter()
            .find(|t| t.kind == TargetKind::Monitor && t.stable_id.as_deref() == Some(stable))
        {
            return Some(Choice { target: target.clone(), how: Match::Stable });
        }
    }
    if let Some(id) = id.filter(|s| !s.is_empty()) {
        if let Some(target) = targets
            .iter()
            .find(|t| t.kind == TargetKind::Monitor && t.id == id)
        {
            return Some(Choice { target: target.clone(), how: Match::Device });
        }
    }

    let chosen = primary()?.clone();
    let how = if id.is_some() || stable.is_some() {
        Match::Fallback
    } else {
        Match::Default
    };
    Some(Choice { target: chosen, how })
}

/// Resolve the saved selection against the screens that are here right now.
///
/// Says out loud when the saved screen could not be found. That line went
/// missing when capture moved to `wgc.rs`, and without it the app quietly
/// recorded the primary screen for as long as it took somebody to notice.
pub fn resolve(
    kind: TargetKind,
    id: Option<&str>,
    stable: Option<&str>,
) -> Option<Choice> {
    let choice = pick(&list_targets(), kind, id, stable)?;
    match choice.how {
        Match::Fallback => log::warn!(
            "the chosen {} is not here (device {:?}, panel {:?}) \u{2014} recording '{}' instead",
            match kind {
                TargetKind::Monitor => "screen",
                TargetKind::Window => "window",
            },
            id.unwrap_or("-"),
            stable.unwrap_or("-"),
            choice.target.title
        ),
        Match::Stable if Some(choice.target.id.as_str()) != id => log::info!(
            "'{}' is {} now, was {:?} \u{2014} found by its panel",
            choice.target.title,
            choice.target.id,
            id.unwrap_or("-")
        ),
        _ => {}
    }
    Some(choice)
}

/// Why a running screen capture no longer matches what the settings mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drift {
    /// Windows ended the capture — the screen was unplugged, or the desktop was
    /// rebuilt under it (sign-in, driver reset). No frame comes any more.
    Closed,
    /// The settings resolve to another screen now: the chosen one came back
    /// after the capture had to stand in with the primary, or it went away.
    Moved,
    /// The same screen, at another resolution. The encoder's size was worked
    /// out from the old one, and the picture would only be scaled to fit it.
    Resized,
}

/// Compare the screen a capture was started on with the one the settings mean
/// now, and say whether — and why — the capture has to start over.
///
/// `now` is `None` when no screen is there at all: a restart would find
/// nothing either, so it waits until one is back.
///
/// Pure, like [`pick`], so the rules are testable without Windows.
pub fn drift(running: &Choice, now: Option<&Choice>, closed: bool) -> Option<Drift> {
    let now = now?;
    if closed {
        return Some(Drift::Closed);
    }
    if !same_screen(&running.target, &now.target) {
        return Some(Drift::Moved);
    }
    if (running.target.width, running.target.height) != (now.target.width, now.target.height) {
        return Some(Drift::Resized);
    }
    None
}

/// The panel's identity when both know it; the device name otherwise. A
/// renumbered `\\.\DISPLAYn` is still the same screen — see [`pick`].
fn same_screen(a: &CaptureTarget, b: &CaptureTarget) -> bool {
    match (&a.stable_id, &b.stable_id) {
        (Some(a), Some(b)) => a == b,
        _ => a.id == b.id,
    }
}

/// Bring a saved selection back in line with reality.
///
/// Two repairs, both quiet and both one-way: a monitor found by its panel under
/// a new device name has that name written back, and one found by device name
/// gains the panel identity it did not have yet. The second is how a
/// configuration written before any of this existed acquires one — on the first
/// start where the screen is present.
///
/// Returns whether anything changed, so the caller knows to save.
pub fn repair_monitor_choice(
    id: &mut Option<String>,
    stable: &mut Option<String>,
) -> bool {
    // Nothing chosen at all is a valid state: the primary screen, by default.
    if id.is_none() && stable.is_none() {
        return false;
    }
    let Some(choice) = resolve(TargetKind::Monitor, id.as_deref(), stable.as_deref()) else {
        return false;
    };
    // A screen that is merely absent keeps its saved identity — it will be back
    // when it is plugged in again, and overwriting it now would lose the choice.
    if choice.how == Match::Fallback {
        return false;
    }

    let mut changed = false;
    if id.as_deref() != Some(choice.target.id.as_str()) {
        *id = Some(choice.target.id.clone());
        changed = true;
    }
    if choice.target.stable_id.is_some() && *stable != choice.target.stable_id {
        *stable = choice.target.stable_id.clone();
        changed = true;
    }
    changed
}

/// The recording source, repaired. A window keeps its handle — see [`pick`].
pub fn repair(recording: &mut crate::model::RecordingConfig) -> bool {
    if recording.target_kind != TargetKind::Monitor {
        return false;
    }
    let mut id = recording.target_id.take();
    let mut stable = recording.target_stable_id.take();
    let changed = repair_monitor_choice(&mut id, &mut stable);
    recording.target_id = id;
    recording.target_stable_id = stable;
    changed
}

/// The screen the banner is pinned to, repaired the same way.
pub fn repair_overlay(overlay: &mut crate::model::OverlayConfig) -> bool {
    let mut id = overlay.monitor.take();
    let mut stable = overlay.monitor_stable_id.take();
    let changed = repair_monitor_choice(&mut id, &mut stable);
    overlay.monitor = id;
    overlay.monitor_stable_id = stable;
    changed
}

/// Find the configured target in the current list. With no selection the
/// primary monitor applies — exactly as in the pipeline.
fn find_target(
    kind: TargetKind,
    id: Option<&str>,
    stable: Option<&str>,
) -> Option<CaptureTarget> {
    pick(&list_targets(), kind, id, stable).map(|choice| choice.target)
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
    let Some(target) = find_target(
        recording.target_kind,
        recording.target_id.as_deref(),
        recording.target_stable_id.as_deref(),
    ) else {
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

    /// Three screens of identical size, as on the machine this was found on:
    /// telling them apart by anything but their identity is impossible.
    fn three_screens() -> Vec<CaptureTarget> {
        let screen = |device: &str, panel: &str, primary: bool| CaptureTarget {
            kind: TargetKind::Monitor,
            id: device.into(),
            stable_id: Some(format!("\\\\?\\DISPLAY#{panel}#5&1ba8a7fa&0#{{guid}}")),
            title: format!("{panel} — 1920×1080"),
            width: 1920,
            height: 1080,
            is_primary: primary,
            refresh_hz: Some(60),
        };
        vec![
            screen("\\\\.\\DISPLAY1", "E2212F", false),
            screen("\\\\.\\DISPLAY2", "CR270E", true),
            screen("\\\\.\\DISPLAY3", "CR270H", false),
        ]
    }

    fn panel(name: &str) -> String {
        format!("\\\\?\\DISPLAY#{name}#5&1ba8a7fa&0#{{guid}}")
    }

    #[test]
    fn the_saved_screen_is_found_by_its_device_name() {
        let choice = pick(
            &three_screens(),
            TargetKind::Monitor,
            Some("\\\\.\\DISPLAY3"),
            None,
        )
        .unwrap();
        assert_eq!(choice.how, Match::Device);
        assert_eq!(choice.target.title, "CR270H — 1920×1080");
    }

    /// The reason the whole thing exists: after a reboot Windows handed the
    /// panel a different `\\.\DISPLAYn`. Going by the device name alone lands
    /// on the wrong screen — silently, because all three are 1920×1080.
    #[test]
    fn a_renumbered_screen_is_still_the_same_screen() {
        let choice = pick(
            &three_screens(),
            TargetKind::Monitor,
            // Saved yesterday, when this panel was DISPLAY1.
            Some("\\\\.\\DISPLAY1"),
            Some(&panel("CR270H")),
        )
        .unwrap();
        assert_eq!(choice.how, Match::Stable);
        assert_eq!(choice.target.id, "\\\\.\\DISPLAY3");
        assert_eq!(choice.target.title, "CR270H — 1920×1080");
    }

    #[test]
    fn the_panel_outranks_the_device_name() {
        // Both match something, and they disagree. The panel wins.
        let choice = pick(
            &three_screens(),
            TargetKind::Monitor,
            Some("\\\\.\\DISPLAY1"),
            Some(&panel("CR270E")),
        )
        .unwrap();
        assert_eq!(choice.how, Match::Stable);
        assert_eq!(choice.target.id, "\\\\.\\DISPLAY2");
    }

    #[test]
    fn an_unplugged_screen_falls_back_to_the_primary_and_says_so() {
        let choice = pick(
            &three_screens(),
            TargetKind::Monitor,
            Some("\\\\.\\DISPLAY9"),
            Some(&panel("GONE123")),
        )
        .unwrap();
        assert_eq!(choice.how, Match::Fallback, "the fallback has to be visible");
        assert!(choice.target.is_primary);
    }

    /// Never having chosen is not the same as having chosen something that is
    /// gone — only the second one is worth a warning.
    #[test]
    fn no_choice_at_all_is_not_a_fallback() {
        let choice = pick(&three_screens(), TargetKind::Monitor, None, None).unwrap();
        assert_eq!(choice.how, Match::Default);
        assert!(choice.target.is_primary);
    }

    /// A screen Windows will not describe still has to be selectable.
    #[test]
    fn a_screen_without_an_identity_still_works() {
        let mut screens = three_screens();
        for screen in &mut screens {
            screen.stable_id = None;
        }
        let choice = pick(&screens, TargetKind::Monitor, Some("\\\\.\\DISPLAY2"), None).unwrap();
        assert_eq!(choice.how, Match::Device);
        assert_eq!(choice.target.id, "\\\\.\\DISPLAY2");
    }

    /// A window is its handle and nothing else — no screen stands in for one
    /// that has been closed.
    #[test]
    fn a_closed_window_does_not_become_a_screen() {
        let mut targets = three_screens();
        targets.push(CaptureTarget {
            kind: TargetKind::Window,
            id: "0xABC".into(),
            stable_id: None,
            title: "A Game".into(),
            width: 1280,
            height: 720,
            is_primary: false,
            refresh_hz: None,
        });
        assert!(pick(&targets, TargetKind::Window, Some("0xDEAD"), None).is_none());
        let open = pick(&targets, TargetKind::Window, Some("0xABC"), None).unwrap();
        assert_eq!(open.target.title, "A Game");
    }

    fn chosen(targets: &[CaptureTarget], device: &str, stable: Option<&str>) -> Choice {
        pick(targets, TargetKind::Monitor, Some(device), stable).unwrap()
    }

    #[test]
    fn a_capture_on_the_right_screen_is_left_alone() {
        let screens = three_screens();
        let running = chosen(&screens, "\\\\.\\DISPLAY3", Some(&panel("CR270H")));
        let now = chosen(&screens, "\\\\.\\DISPLAY3", Some(&panel("CR270H")));
        assert_eq!(drift(&running, Some(&now), false), None);
    }

    /// Unlocking renumbers the screens. The panel is the same, so is the
    /// capture — restarting it would only throw the buffer away.
    #[test]
    fn a_renumbered_screen_is_no_reason_to_restart() {
        let screens = three_screens();
        let running = chosen(&screens, "\\\\.\\DISPLAY3", Some(&panel("CR270H")));
        let mut later = three_screens();
        later[0].id = "\\\\.\\DISPLAY3".into();
        later[2].id = "\\\\.\\DISPLAY1".into();
        let now = chosen(&later, "\\\\.\\DISPLAY3", Some(&panel("CR270H")));
        assert_eq!(drift(&running, Some(&now), false), None);
    }

    /// The core of it: started while the screen was away, the capture stood in
    /// with the primary. Once the screen is back, it has to move over.
    #[test]
    fn a_screen_that_comes_back_takes_the_capture_back() {
        let without: Vec<_> = three_screens()
            .into_iter()
            .filter(|t| !t.title.starts_with("CR270H"))
            .collect();
        let running = chosen(&without, "\\\\.\\DISPLAY3", Some(&panel("CR270H")));
        assert_eq!(running.how, Match::Fallback);
        let now = chosen(&three_screens(), "\\\\.\\DISPLAY3", Some(&panel("CR270H")));
        assert_eq!(drift(&running, Some(&now), false), Some(Drift::Moved));
    }

    #[test]
    fn an_unplugged_screen_moves_the_capture_to_the_primary() {
        let screens = three_screens();
        let running = chosen(&screens, "\\\\.\\DISPLAY3", Some(&panel("CR270H")));
        let without: Vec<_> = screens
            .into_iter()
            .filter(|t| !t.title.starts_with("CR270H"))
            .collect();
        let now = chosen(&without, "\\\\.\\DISPLAY3", Some(&panel("CR270H")));
        assert_eq!(now.how, Match::Fallback);
        assert_eq!(drift(&running, Some(&now), true), Some(Drift::Closed));
        assert_eq!(drift(&running, Some(&now), false), Some(Drift::Moved));
    }

    #[test]
    fn a_new_resolution_restarts_the_capture() {
        let screens = three_screens();
        let running = chosen(&screens, "\\\\.\\DISPLAY3", Some(&panel("CR270H")));
        let mut later = three_screens();
        later[2].width = 2560;
        later[2].height = 1440;
        let now = chosen(&later, "\\\\.\\DISPLAY3", Some(&panel("CR270H")));
        assert_eq!(drift(&running, Some(&now), false), Some(Drift::Resized));
    }

    /// No screen at all — the moment during sign-in when Windows has not put
    /// them back yet. A restart now would find nothing and fail.
    #[test]
    fn with_no_screen_at_all_the_watcher_waits() {
        let screens = three_screens();
        let running = chosen(&screens, "\\\\.\\DISPLAY3", Some(&panel("CR270H")));
        assert_eq!(drift(&running, None, true), None);
    }

    #[test]
    fn repair_writes_back_the_new_device_name() {
        // `repair` goes through the live screen list, so only the pure decision
        // underneath it is exercised here — that the two do agree is what
        // `a_renumbered_screen_is_still_the_same_screen` covers.
        let choice = pick(
            &three_screens(),
            TargetKind::Monitor,
            Some("\\\\.\\DISPLAY1"),
            Some(&panel("CR270H")),
        )
        .unwrap();
        assert_ne!(choice.target.id, "\\\\.\\DISPLAY1");
        assert_eq!(choice.target.stable_id.as_deref(), Some(panel("CR270H").as_str()));
    }

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
