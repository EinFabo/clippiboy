// No console window in the Windows release build.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    own_task_bar_identity();
    clippiboy_lib::run()
}

/// Our windows under an AppUserModelID of their own, set before the first one
/// exists.
///
/// Without one Windows derives the ID from the exe's path, finds the Start menu
/// shortcut the installer made for that exe, and draws the task bar button with
/// the shortcut's icon — the violet one — whatever the window sets. The window
/// held the recoloured icon all along (`examples/icon-probe.rs`); only the dev
/// build, which has no shortcut, ever showed it.
///
/// Notifications are not affected: they name `gg.clippiboy.app`, the shortcut's
/// ID, on every toast themselves.
#[cfg(windows)]
fn own_task_bar_identity() {
    use windows::core::w;
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(w!("gg.clippiboy.app.window"));
    }
}

#[cfg(not(windows))]
fn own_task_bar_identity() {}
