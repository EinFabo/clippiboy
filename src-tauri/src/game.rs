//! Spielerkennung über das Vordergrundfenster.
//!
//! Der Weg ist immer derselbe: Vordergrundfenster → PID → EXE-Name. Steht die
//! EXE in der mitgelieferten Liste, ist der Name damit sicher; sonst gilt eine
//! unbekannte Anwendung dann als Spiel, wenn ihr Fenster den ganzen Monitor
//! ausfüllt — dann wird der aufgeräumte Fenstertitel benutzt.
//!
//! Der Fenstertitel allein reicht nicht: er ändert sich im Spiel (Menü, Karte,
//! Server) und viele Spiele setzen gar keinen.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// Anwendungen, die nie als Spiel zählen — egal wie groß ihr Fenster ist.
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
];

/// Endungen am Fenstertitel, die nichts über das Spiel aussagen.
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

/// EXE-Name (klein geschrieben) → Anzeigename.
///
/// Grundlage ist die eingebaute `games.json`; eine gleichnamige Datei im
/// Datenverzeichnis wird darübergelegt, sodass sich die Liste ohne Neubau
/// erweitern oder korrigieren lässt.
fn games() -> &'static HashMap<String, String> {
    static MAP: OnceLock<HashMap<String, String>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map: HashMap<String, String> =
            serde_json::from_str(include_str!("games.json")).unwrap_or_else(|err| {
                log::error!("Eingebaute games.json ist kaputt: {err}");
                HashMap::new()
            });

        let user = crate::config::data_dir().join("games.json");
        if let Ok(text) = std::fs::read_to_string(&user) {
            match serde_json::from_str::<HashMap<String, String>>(&text) {
                Ok(extra) => {
                    log::info!("{} eigene Spielnamen aus {}", extra.len(), user.display());
                    map.extend(extra);
                }
                Err(err) => log::warn!("{} ist unlesbar: {err}", user.display()),
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

/// Fenstertitel auf den nackten Spielnamen zurechtstutzen.
pub fn clean_title(title: &str) -> String {
    let mut text = title.trim().to_string();

    // Bekannte Endungen abschneiden — mehrfach, „Spiel (64-bit) - Steam".
    loop {
        let lower = text.to_lowercase();
        let Some(suffix) = TITLE_SUFFIXES.iter().find(|s| lower.ends_with(*s)) else {
            break;
        };
        text.truncate(text.len() - suffix.len());
        text = text.trim().to_string();
    }

    // Versionsnummern am Ende: „Minecraft 1.21.4", „Spiel v2.0".
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

/// „eldenring.exe" → „Eldenring" — letzter Ausweg, wenn es keinen Titel gibt.
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

/// Aus EXE, Fenstertitel und „füllt den Monitor" einen Spielnamen ableiten.
///
/// Reine Funktion, damit die Regeln auch ohne Windows testbar sind.
pub fn resolve_name(exe: &str, title: &str, fullscreen: bool) -> Option<String> {
    if exe.is_empty() || is_ignored(exe) {
        return None;
    }
    if let Some(name) = games().get(exe.to_lowercase().as_str()) {
        return Some(name.clone());
    }
    // Unbekannte Anwendung: nur im Vollbild als Spiel durchgehen lassen.
    if !fullscreen {
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

    /// Vollständiger EXE-Pfad eines Prozesses, auf den Dateinamen reduziert.
    ///
    /// `QueryFullProcessImageNameW` kommt mit `PROCESS_QUERY_LIMITED_INFORMATION`
    /// aus und funktioniert deshalb auch bei Spielen, die als Administrator
    /// laufen — anders als der `GetModuleBaseNameW`-Weg in `audio/devices.rs`,
    /// der zusätzlich `PROCESS_VM_READ` braucht.
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

    /// Deckt das Fenster den Monitor vollständig ab? Trifft auf echtes Vollbild
    /// genauso zu wie auf randloses Fenster — beides sind Spiel-Indizien.
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

    /// Mittelpunkt des Vordergrundfensters in physischen Bildschirmkoordinaten.
    ///
    /// Damit landet der Overlay-Banner auf dem Monitor, auf dem gespielt wird,
    /// und nicht stur auf dem primären.
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

    /// EXE-Name, Fenstertitel und Vollbild-Flag des Vordergrundfensters.
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

/// Welches Spiel läuft gerade im Vordergrund? `None`, wenn keins erkennbar ist.
#[cfg(windows)]
pub fn detect() -> Option<String> {
    let (exe, title, fullscreen) = win::foreground()?;
    resolve_name(&exe, &title, fullscreen)
}

#[cfg(not(windows))]
pub fn detect() -> Option<String> {
    None
}

/// Mittelpunkt des Vordergrundfensters — für die Monitorwahl des Overlays.
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
        assert_eq!(resolve_name("unbekannt.exe", "Irgendein Spiel", false), None);
        assert_eq!(
            resolve_name("unbekannt.exe", "Irgendein Spiel", true).as_deref(),
            Some("Irgendein Spiel")
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
        assert_eq!(clean_title("Mein Spiel (64-bit) - Steam"), "Mein Spiel");
        assert_eq!(clean_title("Assetto Corsa - DirectX 11"), "Assetto Corsa");
        assert_eq!(clean_title("  Spiel v2.0  "), "Spiel");
    }

    #[test]
    fn a_number_that_belongs_to_the_name_survives() {
        // Keine Versionsnummer — kein Punkt, also Teil des Namens.
        assert_eq!(clean_title("Counter-Strike 2"), "Counter-Strike 2");
        assert_eq!(clean_title("Half-Life 2"), "Half-Life 2");
    }

    #[test]
    fn the_builtin_list_parses() {
        assert!(games().len() > 50);
        assert_eq!(games().get("eldenring.exe").map(String::as_str), Some("Elden Ring"));
    }
}
