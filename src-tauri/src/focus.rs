//! Bringing a window of ours to the front over a game — asked for, never
//! forced.
//!
//! tao's `set_focus` falls back to faking an Alt press with `SendInput` when
//! Windows refuses `SetForegroundWindow`. Over a game that is synthesized
//! input, which ClippiBoy never sends (anti-cheat, and the owner's rule). So
//! wherever a game may be in front, the foreground is asked for once, the
//! documented way, and a no is the answer: the window stays behind, and the
//! task bar may blink for it — a blink is not input.
//!
//! Windows grants the foreground to the process that got the user's latest
//! input: a hotkey registered with `RegisterHotKey`, a click into one of our
//! windows. A controller button read off the pad, or a mouse button the game
//! got, is not such input.

/// Ask for the foreground for `window`, once. Says whether Windows gave it.
#[cfg(windows)]
pub fn request(window: &tauri::WebviewWindow, what: &str) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
    let Ok(handle) = window.hwnd() else {
        return false;
    };
    let granted = unsafe { SetForegroundWindow(HWND(handle.0)) }.as_bool();
    if !granted {
        log::info!("focus: Windows refused the foreground for the {what}");
    }
    granted
}

/// Blink `window`'s task bar button until it is brought up — for a window
/// that was refused the foreground.
#[cfg(windows)]
pub fn flash(window: &tauri::WebviewWindow) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        FlashWindowEx, FLASHWINFO, FLASHW_TIMERNOFG, FLASHW_TRAY,
    };
    let Ok(handle) = window.hwnd() else {
        return;
    };
    let info = FLASHWINFO {
        cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
        hwnd: HWND(handle.0),
        dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
        uCount: 0,
        dwTimeout: 0,
    };
    unsafe {
        let _ = FlashWindowEx(&info);
    }
}

#[cfg(not(windows))]
pub fn request(_window: &tauri::WebviewWindow, _what: &str) -> bool {
    false
}

#[cfg(not(windows))]
pub fn flash(_window: &tauri::WebviewWindow) {}
