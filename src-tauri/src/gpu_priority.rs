//! Die Konsole bekommt ihren Teil der Grafikkarte, auch wenn das Spiel sie ganz
//! haben will.
//!
//! Gezeichnet wird die Konsole nicht von ClippiBoy selbst, sondern vom
//! GPU-Prozess von WebView2 (`msedgewebview2.exe --type=gpu-process`), einem
//! Enkel dieses Prozesses. Dessen Arbeit steht beim GPU-Scheduler in derselben
//! Klasse wie die des Spiels. Läuft ein Spiel ohne FPS-Grenze, hält es die Karte
//! auf 100 %, und die Konsole bekommt nur, was zwischen zwei seiner Bilder übrig
//! ist — sie und der Player darin ruckeln, das Spiel läuft ungerührt weiter.
//! Mit einer FPS-Grenze im Spiel war alles flüssig (Fabi, RTX 3070).
//!
//! Dagegen hebt `raise` die Scheduler-Klasse aller WebView2-Prozesse unter uns
//! auf *High*. Dasselbe tut OBS mit sich selbst, aus demselben Grund. *Realtime*
//! bräuchte `SeIncreaseBasePriorityPrivilege`, also Adminrechte, und ließe die
//! Konsole das Spiel verdrängen statt nur neben ihm zu bestehen. Die Konsole
//! zeichnet wenig; was sie dem Spiel abnimmt, ist klein.
//!
//! Die Funktion steht nicht in den Headern des SDK, sondern nur im WDK — sie wird
//! deshalb zur Laufzeit aus `gdi32.dll` geholt.

#![cfg(windows)]

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use windows::core::{s, w};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
};

/// `D3DKMT_SCHEDULINGPRIORITYCLASS_HIGH` aus `d3dkmthk.h`.
const PRIORITY_HIGH: i32 = 4;

type SetPriorityClass = unsafe extern "system" fn(HANDLE, i32) -> i32;

/// Schon angehobene Prozesse. Der GPU-Prozess lebt so lange wie die App, und ein
/// neuer (nach einem Absturz von WebView2) hat eine neue Nummer — die kommt dann
/// beim nächsten Öffnen dran.
fn raised() -> &'static Mutex<HashSet<u32>> {
    static RAISED: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
    RAISED.get_or_init(Default::default)
}

fn set_priority_class() -> Option<SetPriorityClass> {
    static FUNCTION: OnceLock<Option<SetPriorityClass>> = OnceLock::new();
    *FUNCTION.get_or_init(|| unsafe {
        let module = LoadLibraryW(w!("gdi32.dll")).ok()?;
        let address = GetProcAddress(module, s!("D3DKMTSetProcessSchedulingPriorityClass"))?;
        Some(std::mem::transmute::<_, SetPriorityClass>(address))
    })
}

/// Alle `msedgewebview2.exe` unterhalb dieses Prozesses, gleich wie tief.
///
/// Nicht nur der GPU-Prozess: welcher das ist, steht nur in der Befehlszeile,
/// und die liest man einem fremden Prozess nicht ohne Weiteres ab. Die übrigen
/// (Browser, Renderer) reichen selbst keine GPU-Arbeit ein; die Klasse schadet
/// ihnen nicht und nützt ihnen nichts.
fn webview_processes() -> Vec<u32> {
    let mut parents: HashMap<u32, u32> = HashMap::new();
    let mut webview: HashSet<u32> = HashSet::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return Vec::new();
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
            let exe = String::from_utf16_lossy(&entry.szExeFile[..len]);
            if exe.eq_ignore_ascii_case("msedgewebview2.exe") {
                webview.insert(entry.th32ProcessID);
            }
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }

    let me = unsafe { GetCurrentProcessId() };
    webview
        .into_iter()
        .filter(|&pid| {
            // Den Eltern nach oben folgen, bis wir auftauchen. Die Tiefe ist
            // begrenzt: Windows vergibt Nummern neu, ein Eltern-Eintrag kann auf
            // einen längst anderen Prozess zeigen und so einen Kreis schließen.
            let mut at = pid;
            for _ in 0..8 {
                match parents.get(&at) {
                    Some(&parent) if parent == me => return true,
                    Some(&parent) if parent != 0 && parent != at => at = parent,
                    _ => return false,
                }
            }
            false
        })
        .collect()
}

/// Die WebView2-Prozesse der App beim GPU-Scheduler auf *High* heben.
///
/// Macht einen Prozess-Schnappschuss, also nicht im Haupt-Faden aufrufen.
pub fn raise() {
    let Some(set) = set_priority_class() else {
        log::warn!("gpu priority: D3DKMTSetProcessSchedulingPriorityClass not found");
        return;
    };
    for pid in webview_processes() {
        if raised().lock().unwrap().contains(&pid) {
            continue;
        }
        unsafe {
            let Ok(handle) = OpenProcess(
                PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                pid,
            ) else {
                log::warn!("gpu priority: could not open webview process {pid}");
                continue;
            };
            let status = set(handle, PRIORITY_HIGH);
            let _ = CloseHandle(handle);
            if status == 0 {
                log::info!("gpu priority: webview process {pid} raised to high");
                raised().lock().unwrap().insert(pid);
            } else {
                log::warn!("gpu priority: webview process {pid} refused, status {status:#x}");
            }
        }
    }
}
