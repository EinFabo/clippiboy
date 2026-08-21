//! Game detection via the foreground window.
//!
//! The route is always the same: foreground window → PID → exe name. If the exe
//! is in the bundled list, the name is certain; otherwise an unknown application
//! counts as a game when its window fills the whole monitor — the tidied-up
//! window title is then used.
//!
//! The window title alone is not enough: it changes during play (menu, map,
//! server) and many games do not set one at all.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// Applications that never count as a game — no matter how large their window.
const IGNORED: &[&str] = &[
    "clippiboy.exe",
    "explorer.exe",
    "searchhost.exe",
    "shellexperiencehost.exe",
    "applicationframehost.exe",
    "startmenuexperiencehost.exe",
    "lockapp.exe",
    "taskmgr.exe",
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "opera.exe",
    "operagx.exe",
    "brave.exe",
    "vivaldi.exe",
    "discord.exe",
    "discordptb.exe",
    "discordcanary.exe",
    "steam.exe",
    "steamwebhelper.exe",
    "epicgameslauncher.exe",
    "battle.net.exe",
    "riotclientservices.exe",
    "riotclientux.exe",
    "upc.exe",
    "eadesktop.exe",
    "goggalaxy.exe",
    "code.exe",
    "devenv.exe",
    "rider64.exe",
    "idea64.exe",
    "windowsterminal.exe",
    "powershell.exe",
    "cmd.exe",
    "wt.exe",
    "spotify.exe",
    "vlc.exe",
    "obs64.exe",
    "obs32.exe",
    "medal.exe",
    "nvidia app.exe",
    "nvidia share.exe",
    "teams.exe",
    "slack.exe",
    "notepad.exe",
    "notepad++.exe",
    "explorerframe.exe",
    // Tools whose windows cover the whole monitor and would therefore pass as a
    // "game" without these lines — the Snipping Tool veil is the classic.
    "snippingtool.exe",
    "screenclippinghost.exe",
    "screensketch.exe",
    "sharex.exe",
    "greenshot.exe",
    "snagit32.exe",
    "snagiteditor.exe",
    "systemsettings.exe",
    "dwm.exe",
    "sihost.exe",
    "textinputhost.exe",
    "rundll32.exe",
    "logonui.exe",
    // Video playback: a film in fullscreen is not a game.
    "mpv.exe",
    "mpc-hc.exe",
    "mpc-hc64.exe",
    "potplayermini.exe",
    "potplayermini64.exe",
    "wmplayer.exe",
    "video.ui.exe",
    "photos.exe",
    // Further browsers and Chromium shells.
    "opera_gx.exe",
    "chromium.exe",
    "librewolf.exe",
    "waterfox.exe",
    "zen.exe",
    "arc.exe",
    "msedgewebview2.exe",
    "chrome_proxy.exe",
    // Communication and office.
    "whatsapp.exe",
    "telegram.exe",
    "signal.exe",
    "zoom.exe",
    "ms-teams.exe",
    "outlook.exe",
    "thunderbird.exe",
    "winword.exe",
    "excel.exe",
    "powerpnt.exe",
    "acrord32.exe",
    "acrobat.exe",
    // Recording and overlay tools.
    "obs.exe",
    "streamlabs obs.exe",
    "xsplit.core.exe",
    "overwolf.exe",
    "nvidia overlay.exe",
    "cursor.exe",
    "sublime_text.exe",
];

/// Title endings that give away a foreign application. If one of them matches,
/// the window counts as not a game — even when it fills the monitor.
///
/// A second safeguard behind `IGNORED`: that list can never be complete —
/// portable, renamed or still unknown applications are not in it. The title
/// gives them away regardless.
const FOREIGN_TITLES: &[&str] = &[
    " - youtube",
    " – youtube",
    " - google chrome",
    " – google chrome",
    " - mozilla firefox",
    " – mozilla firefox",
    " - microsoft edge",
    " – microsoft edge",
    " - opera",
    " – opera",
    " - brave",
    " – brave",
    " - vivaldi",
    " – vivaldi",
    " - discord",
    " – discord",
    " - visual studio code",
    " – visual studio code",
    // German-locale Windows says "Überlagerung" where English says "overlay".
    " überlagerung",
    " overlay",
];

/// File extensions in the title: a window showing a file is not playing anything.
const FILE_TITLES: &[&str] = &[
    ".mp4", ".mkv", ".webm", ".avi", ".mov", ".m4v", ".mp3", ".flac", ".wav", ".png", ".jpg",
    ".jpeg", ".gif", ".webp", ".pdf", ".txt", ".log", ".json", ".docx", ".xlsx",
];

/// Suffixes on the window title that say nothing about the game.
const TITLE_SUFFIXES: &[&str] = &[
    " - steam",
    " - epic games",
    " - directx 12",
    " - directx 11",
    " - direct3d 12",
    " - direct3d 11",
    " - dx12",
    " - dx11",
    " - vulkan",
    " - opengl",
    " (64-bit)",
    " (32-bit)",
    " (x64)",
    " - 64 bit",
    " [steam]",
];

fn ignored() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| IGNORED.iter().copied().collect())
}

/// Exe name (lower-cased) → display name.
///
/// The built-in `games.json` is the base; a file of the same name in the data
/// directory is layered on top, so the list can be extended or corrected without
/// a rebuild.
fn games() -> &'static HashMap<String, String> {
    static MAP: OnceLock<HashMap<String, String>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map: HashMap<String, String> =
            serde_json::from_str(include_str!("games.json")).unwrap_or_else(|err| {
                log::error!("the built-in games.json is broken: {err}");
                HashMap::new()
            });

        let user = crate::config::data_dir().join("games.json");
        if let Ok(text) = std::fs::read_to_string(&user) {
            match serde_json::from_str::<HashMap<String, String>>(&text) {
                Ok(extra) => {
                    log::info!("{} custom game names from {}", extra.len(), user.display());
                    map.extend(extra);
                }
                Err(err) => log::warn!("{} is unreadable: {err}", user.display()),
            }
        }

        map.into_iter()
            .map(|(exe, name)| (exe.to_lowercase(), name))
            .collect()
    })
}

pub fn is_ignored(exe: &str) -> bool {
    ignored().contains(exe.to_lowercase().as_str())
}

/// Does the window title give away that no game is running here at all?
pub fn is_foreign_title(title: &str) -> bool {
    let lower = title.trim().to_lowercase();
    if lower.is_empty() {
        return false;
    }
    // A path is never a game name — "C:\\…\\clip.mp4" made it into the gallery
    // that way.
    if lower.contains(":\\") || lower.starts_with("\\\\") {
        return true;
    }
    FILE_TITLES.iter().any(|ext| lower.ends_with(ext))
        || FOREIGN_TITLES.iter().any(|end| lower.ends_with(end))
}

/// Trim a window title down to the bare game name.
pub fn clean_title(title: &str) -> String {
    let mut text = title.trim().to_string();

    // Unread counter at the front: "(102) …" says nothing about the game.
    if let Some(rest) = text.strip_prefix('(') {
        if let Some((count, tail)) = rest.split_once(')') {
            if !count.is_empty() && count.chars().all(|c| c.is_ascii_digit()) {
                text = tail.trim().to_string();
            }
        }
    }

    // Cut off known suffixes — repeatedly, "Game (64-bit) - Steam".
    loop {
        let lower = text.to_lowercase();
        let Some(suffix) = TITLE_SUFFIXES.iter().find(|s| lower.ends_with(*s)) else {
            break;
        };
        text.truncate(text.len() - suffix.len());
        text = text.trim().to_string();
    }

    // Version numbers at the end: "Minecraft 1.21.4", "Game v2.0".
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() > 1 && looks_like_version(words[words.len() - 1]) {
        text = words[..words.len() - 1].join(" ");
    }

    text.trim().trim_end_matches(['-', '–', '|', ':']).trim().to_string()
}

fn looks_like_version(word: &str) -> bool {
    let core = word.trim_start_matches(['v', 'V']);
    !core.is_empty()
        && core.starts_with(|c: char| c.is_ascii_digit())
        && core.chars().all(|c| c.is_ascii_digit() || c == '.')
        && core.contains('.')
}

/// "eldenring.exe" → "Eldenring" — the last resort when there is no title.
fn from_exe(exe: &str) -> String {
    let stem = exe.trim_end_matches(".exe").replace(['_', '-'], " ");
    stem.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Derive a game name from exe, window title and "fills the monitor".
///
/// A pure function, so the rules are testable without Windows too.
pub fn resolve_name(exe: &str, title: &str, fullscreen: bool) -> Option<String> {
    if exe.is_empty() || is_ignored(exe) {
        return None;
    }
    if let Some(name) = games().get(exe.to_lowercase().as_str()) {
        return Some(name.clone());
    }
    // Unknown application: only let it pass as a game in fullscreen.
    if !fullscreen {
        return None;
    }
    // And not even then if the title gives away a foreign application. Better
    // "Unknown" than a browser tab that stands as a gallery filter forever.
    if is_foreign_title(title) {
        return None;
    }
    let cleaned = clean_title(title);
    Some(if cleaned.is_empty() {
        from_exe(exe)
    } else {
        cleaned
    })
}

#[cfg(windows)]
mod win {
    use windows::Win32::Foundation::{CloseHandle, HWND, MAX_PATH, RECT};
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
        GetWindowThreadProcessId,
    };

    /// A process's full exe path, reduced to the file name.
    ///
    /// `QueryFullProcessImageNameW` gets by with
    /// `PROCESS_QUERY_LIMITED_INFORMATION` and therefore works for games running
    /// as administrator too — unlike the `GetModuleBaseNameW` route in
    /// `audio/devices.rs`, which additionally needs `PROCESS_VM_READ`.
    fn exe_name(pid: u32) -> Option<String> {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; MAX_PATH as usize];
            let mut len = buf.len() as u32;
            let result = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                windows::core::PWSTR(buf.as_mut_ptr()),
                &mut len,
            );
            let _ = CloseHandle(handle);
            result.ok()?;

            let path = String::from_utf16_lossy(&buf[..len as usize]);
            path.rsplit(['\\', '/']).next().map(|s| s.to_string())
        }
    }

    fn window_title(hwnd: HWND) -> String {
        unsafe {
            let len = GetWindowTextLengthW(hwnd);
            if len == 0 {
                return String::new();
            }
            let mut buf = vec![0u16; len as usize + 1];
            let written = GetWindowTextW(hwnd, &mut buf);
            String::from_utf16_lossy(&buf[..written.max(0) as usize])
        }
    }

    /// Does the window cover the monitor completely? True for real fullscreen as
    /// well as for a borderless window — both are signs of a game.
    fn covers_monitor(hwnd: HWND) -> bool {
        unsafe {
            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_err() {
                return false;
            }
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                return false;
            }
            let screen = info.rcMonitor;
            rect.left <= screen.left
                && rect.top <= screen.top
                && rect.right >= screen.right
                && rect.bottom >= screen.bottom
        }
    }

    /// Centre of the foreground window in physical screen coordinates.
    ///
    /// This is what puts the overlay banner on the monitor being played on rather
    /// than stubbornly on the primary one.
    pub fn foreground_center() -> Option<(f64, f64)> {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return None;
            }
            let mut rect = RECT::default();
            GetWindowRect(hwnd, &mut rect).ok()?;
            Some((
                (rect.left + rect.right) as f64 / 2.0,
                (rect.top + rect.bottom) as f64 / 2.0,
            ))
        }
    }

    /// Exe name, window title and fullscreen flag of the foreground window.
    pub fn foreground() -> Option<(String, String, bool)> {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 {
                return None;
            }
            let exe = exe_name(pid)?;
            Some((exe, window_title(hwnd), covers_monitor(hwnd)))
        }
    }
}

/// Which game is in the foreground right now? `None` if none is recognizable.
#[cfg(windows)]
pub fn detect() -> Option<String> {
    let (exe, title, fullscreen) = win::foreground()?;
    resolve_name(&exe, &title, fullscreen)
}

#[cfg(not(windows))]
pub fn detect() -> Option<String> {
    None
}

/// Centre of the foreground window — for the overlay's monitor choice.
#[cfg(windows)]
pub fn foreground_center() -> Option<(f64, f64)> {
    win::foreground_center()
}

#[cfg(not(windows))]
pub fn foreground_center() -> Option<(f64, f64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_exe_wins_over_the_window_title() {
        assert_eq!(
            resolve_name("cs2.exe", "Counter-Strike 2 - Direct3D 11", false).as_deref(),
            Some("Counter-Strike 2")
        );
    }

    #[test]
    fn exe_lookup_ignores_case() {
        assert_eq!(
            resolve_name("CS2.EXE", "", false).as_deref(),
            Some("Counter-Strike 2")
        );
    }

    #[test]
    fn ignored_apps_are_never_a_game() {
        assert_eq!(resolve_name("discord.exe", "Discord", true), None);
        assert_eq!(resolve_name("clippiboy.exe", "ClippiBoy", true), None);
        assert_eq!(resolve_name("CHROME.EXE", "YouTube", true), None);
    }

    #[test]
    fn unknown_app_counts_only_in_fullscreen() {
        assert_eq!(resolve_name("unknown.exe", "Some Game", false), None);
        assert_eq!(
            resolve_name("unknown.exe", "Some Game", true).as_deref(),
            Some("Some Game")
        );
    }

    #[test]
    fn unknown_app_without_title_falls_back_to_the_exe() {
        assert_eq!(
            resolve_name("no_hesi.exe", "", true).as_deref(),
            Some("No Hesi")
        );
    }

    #[test]
    fn suffixes_and_versions_are_trimmed() {
        assert_eq!(clean_title("Minecraft 1.21.4"), "Minecraft");
        assert_eq!(clean_title("My Game (64-bit) - Steam"), "My Game");
        assert_eq!(clean_title("Assetto Corsa - DirectX 11"), "Assetto Corsa");
        assert_eq!(clean_title("  Game v2.0  "), "Game");
    }

    #[test]
    fn a_number_that_belongs_to_the_name_survives() {
        // Not a version number — no dot, so part of the name.
        assert_eq!(clean_title("Counter-Strike 2"), "Counter-Strike 2");
        assert_eq!(clean_title("Half-Life 2"), "Half-Life 2");
    }

    #[test]
    fn foreign_windows_do_not_become_a_game() {
        // Exactly the entries that used to stand as filters in the gallery.
        assert_eq!(
            resolve_name(
                "unknown.exe",
                // A German-locale window title on purpose: the heuristic has to
                // hold up in any Windows language.
                "(102) WIR MÜSSEN PAYEN - YouTube – Opera",
                true
            ),
            None
        );
        assert_eq!(
            resolve_name(
                "unknown.exe",
                "C:\\Users\\fabia\\projects\\clippiboy\\synctest.mp4",
                true
            ),
            None
        );
        assert_eq!(
            resolve_name("unknown.exe", "Snipping Tool Überlagerung", true),
            None
        );
        assert!(is_ignored("SnippingTool.exe"));
    }

    #[test]
    fn a_known_game_survives_a_foreign_looking_title() {
        // The list weighs more than the title — otherwise a game would lose its
        // name just because it happens to be playing a video.
        assert_eq!(
            resolve_name("cs2.exe", "whatever.mp4", true).as_deref(),
            Some("Counter-Strike 2")
        );
    }

    #[test]
    fn unread_counters_are_trimmed() {
        assert_eq!(clean_title("(3) My Game"), "My Game");
        // Not a bracketed number but part of the name.
        assert_eq!(clean_title("(Beta) My Game"), "(Beta) My Game");
    }

    #[test]
    fn the_builtin_list_parses() {
        assert!(games().len() > 50);
        assert_eq!(games().get("eldenring.exe").map(String::as_str), Some("Elden Ring"));
    }
}
