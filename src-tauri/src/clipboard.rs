//! Die Windows-Zwischenablage.
//!
//! Zwei Dinge liegen hier: eine **Datei** ablegen, damit ein Clip in Discord
//! oder im Explorer mit Strg+V landet, und **Text** lesen und schreiben für das
//! Menü in den Textfeldern.
//!
//! Warum nicht aus dem WebView heraus? Lesen aus der Zwischenablage fragt dort
//! um Erlaubnis, und dieser Dialog gehört nicht in eine App, die ohnehin schon
//! nativ ist. Die Datei-Ablage (`CF_HDROP`) kann das WebView überhaupt nicht.

#[cfg(windows)]
mod win {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable,
        OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::UI::Shell::DROPFILES;

    /// Dateiliste, wie sie Explorer und Discord beim Einfügen erwarten. Im
    /// windows-Crate 0.58 nicht als Konstante exportiert.
    const CF_HDROP: u32 = 15;
    /// Text in UTF-16.
    const CF_UNICODETEXT: u32 = 13;

    /// Die Zwischenablage öffnen — mit ein paar Anläufen.
    ///
    /// Hält sie gerade ein anderes Programm (jedes Kopieren tut das für einen
    /// Wimpernschlag), ist das kein Fehler, sondern eine Sache von
    /// Millisekunden. Der Rückgabewert schließt sie beim Verlassen wieder.
    struct Clipboard;

    impl Clipboard {
        fn open() -> Result<Self, String> {
            for attempt in 0..8 {
                if unsafe { OpenClipboard(HWND::default()) }.is_ok() {
                    return Ok(Self);
                }
                std::thread::sleep(std::time::Duration::from_millis(15 * (attempt + 1)));
            }
            Err("Die Zwischenablage ist gerade von einem anderen Programm belegt.".into())
        }
    }

    impl Drop for Clipboard {
        fn drop(&mut self) {
            let _ = unsafe { CloseClipboard() };
        }
    }

    /// Einen Speicherblock anlegen und füllen. Gibt ihn wieder frei, wenn das
    /// Ablegen scheitert — sonst gehört er ab `SetClipboardData` dem System.
    fn put(format: u32, bytes: &[u8]) -> Result<(), String> {
        let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }
            .map_err(|err| format!("Kein Speicher für die Zwischenablage: {err}"))?;

        let target = unsafe { GlobalLock(handle) };
        if target.is_null() {
            let _ = unsafe { GlobalFree(handle) };
            return Err("Der Speicher für die Zwischenablage ließ sich nicht sperren.".into());
        }
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), target as *mut u8, bytes.len());
            let _ = GlobalUnlock(handle);
        }

        // Ab hier gehört der Block dem System — freigeben würde die
        // Zwischenablage zerlegen.
        match unsafe { SetClipboardData(format, HANDLE(handle.0)) } {
            Ok(_) => Ok(()),
            Err(err) => {
                let _ = unsafe { GlobalFree(handle) };
                Err(format!("Die Zwischenablage nahm die Daten nicht an: {err}"))
            }
        }
    }

    /// UTF-16 mit abschließender Null, wie Windows es überall erwartet.
    fn wide(text: &str) -> Vec<u16> {
        std::ffi::OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub fn copy_files(paths: &[&Path]) -> Result<(), String> {
        if paths.is_empty() {
            return Err("Es ist keine Datei zum Kopieren da.".into());
        }
        // Kopf, dann alle Pfade hintereinander, am Ende eine zweite Null.
        let mut names: Vec<u16> = Vec::new();
        for path in paths {
            names.extend(wide(&path.to_string_lossy()));
        }
        names.push(0);

        let header = DROPFILES {
            // Wo hinter dem Kopf die Namen anfangen.
            pFiles: std::mem::size_of::<DROPFILES>() as u32,
            fWide: true.into(),
            ..Default::default()
        };
        let mut bytes = Vec::with_capacity(std::mem::size_of::<DROPFILES>() + names.len() * 2);
        bytes.extend_from_slice(unsafe {
            std::slice::from_raw_parts(
                &header as *const DROPFILES as *const u8,
                std::mem::size_of::<DROPFILES>(),
            )
        });
        for unit in &names {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }

        let _clipboard = Clipboard::open()?;
        unsafe { EmptyClipboard() }.map_err(|err| format!("Zwischenablage leeren: {err}"))?;
        put(CF_HDROP, &bytes)
    }

    pub fn copy_text(text: &str) -> Result<(), String> {
        let units = wide(text);
        let mut bytes = Vec::with_capacity(units.len() * 2);
        for unit in &units {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }

        let _clipboard = Clipboard::open()?;
        unsafe { EmptyClipboard() }.map_err(|err| format!("Zwischenablage leeren: {err}"))?;
        put(CF_UNICODETEXT, &bytes)
    }

    pub fn read_text() -> Result<String, String> {
        let _clipboard = Clipboard::open()?;
        if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) }.is_err() {
            // Kein Text darin ist kein Fehler — dann gibt es eben nichts
            // einzufügen.
            return Ok(String::new());
        }
        let handle = unsafe { GetClipboardData(CF_UNICODETEXT) }
            .map_err(|err| format!("Zwischenablage lesen: {err}"))?;
        let block = HGLOBAL(handle.0);
        let source = unsafe { GlobalLock(block) } as *const u16;
        if source.is_null() {
            return Err("Der Inhalt der Zwischenablage ließ sich nicht lesen.".into());
        }
        // Bis zur abschließenden Null. Eine Obergrenze, damit ein kaputter
        // Block nicht endlos gelesen wird.
        let mut units: Vec<u16> = Vec::new();
        unsafe {
            for offset in 0..4_000_000isize {
                let unit = *source.offset(offset);
                if unit == 0 {
                    break;
                }
                units.push(unit);
            }
            let _ = GlobalUnlock(block);
        }
        Ok(String::from_utf16_lossy(&units))
    }
}

#[cfg(windows)]
pub use win::{copy_text, read_text};

#[cfg(windows)]
pub fn copy_files(paths: &[std::path::PathBuf]) -> Result<(), String> {
    let refs: Vec<&std::path::Path> = paths.iter().map(|p| p.as_path()).collect();
    win::copy_files(&refs)
}

#[cfg(not(windows))]
pub fn copy_files(_paths: &[std::path::PathBuf]) -> Result<(), String> {
    Err("Die Zwischenablage gibt es nur unter Windows.".into())
}

#[cfg(not(windows))]
pub fn copy_text(_text: &str) -> Result<(), String> {
    Err("Die Zwischenablage gibt es nur unter Windows.".into())
}

#[cfg(not(windows))]
pub fn read_text() -> Result<String, String> {
    Ok(String::new())
}
