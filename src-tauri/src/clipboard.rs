//! The Windows clipboard.
//!
//! Three things live here: putting a **file** on the clipboard so a clip lands
//! in Discord or Explorer with Ctrl+V, putting a **picture** there so a
//! screenshot can be pasted straight into a chat, and reading and writing
//! **text** for the context menu in text fields.
//!
//! Why not from inside the WebView? Reading the clipboard asks for permission
//! there, and that dialog does not belong in an app that is native anyway. And
//! the WebView cannot put files (`CF_HDROP`) on the clipboard at all.

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

    /// File list as Explorer and Discord expect it on paste. Not exported as a
    /// constant by the windows crate 0.58.
    const CF_HDROP: u32 = 15;
    /// Text in UTF-16.
    const CF_UNICODETEXT: u32 = 13;
    /// A bitmap without its file header — that is what the clipboard wants.
    const CF_DIB: u32 = 8;

    /// Open the clipboard — with a few attempts.
    ///
    /// If another program is holding it right now (every copy does, for the
    /// blink of an eye), that is not an error but a matter of milliseconds. The
    /// returned value closes it again on the way out.
    struct Clipboard;

    impl Clipboard {
        fn open() -> Result<Self, String> {
            for attempt in 0..8 {
                if unsafe { OpenClipboard(HWND::default()) }.is_ok() {
                    return Ok(Self);
                }
                std::thread::sleep(std::time::Duration::from_millis(15 * (attempt + 1)));
            }
            Err("Another program is holding the clipboard right now.".into())
        }
    }

    impl Drop for Clipboard {
        fn drop(&mut self) {
            let _ = unsafe { CloseClipboard() };
        }
    }

    /// Allocate a block of memory and fill it. Frees it again if handing it
    /// over fails — from `SetClipboardData` on it belongs to the system.
    fn put(format: u32, bytes: &[u8]) -> Result<(), String> {
        let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }
            .map_err(|err| format!("out of memory for the clipboard: {err}"))?;

        let target = unsafe { GlobalLock(handle) };
        if target.is_null() {
            let _ = unsafe { GlobalFree(handle) };
            return Err("Could not lock the clipboard memory.".into());
        }
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), target as *mut u8, bytes.len());
            let _ = GlobalUnlock(handle);
        }

        // From here the block belongs to the system — freeing it would take
        // the clipboard apart.
        match unsafe { SetClipboardData(format, HANDLE(handle.0)) } {
            Ok(_) => Ok(()),
            Err(err) => {
                let _ = unsafe { GlobalFree(handle) };
                Err(format!("The clipboard did not accept the data: {err}"))
            }
        }
    }

    /// UTF-16 with a trailing null, the way Windows expects it everywhere.
    fn wide(text: &str) -> Vec<u16> {
        std::ffi::OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub fn copy_files(paths: &[&Path]) -> Result<(), String> {
        if paths.is_empty() {
            return Err("There is no file to copy.".into());
        }
        // Header, then all paths back to back, and a second null at the end.
        let mut names: Vec<u16> = Vec::new();
        for path in paths {
            names.extend(wide(&path.to_string_lossy()));
        }
        names.push(0);

        let header = DROPFILES {
            // Where the names start behind the header.
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
        unsafe { EmptyClipboard() }.map_err(|err| format!("empty clipboard: {err}"))?;
        put(CF_HDROP, &bytes)
    }

    /// Put a picture on the clipboard as a device-independent bitmap.
    ///
    /// `CF_DIB` is what every program understands — the file on the clipboard
    /// (`copy_files`) only helps where files can be dropped, and a chat window
    /// wants the picture itself.
    pub fn copy_image(dib: &[u8]) -> Result<(), String> {
        let _clipboard = Clipboard::open()?;
        unsafe { EmptyClipboard() }.map_err(|err| format!("empty clipboard: {err}"))?;
        put(CF_DIB, dib)
    }

    pub fn copy_text(text: &str) -> Result<(), String> {
        let units = wide(text);
        let mut bytes = Vec::with_capacity(units.len() * 2);
        for unit in &units {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }

        let _clipboard = Clipboard::open()?;
        unsafe { EmptyClipboard() }.map_err(|err| format!("empty clipboard: {err}"))?;
        put(CF_UNICODETEXT, &bytes)
    }

    pub fn read_text() -> Result<String, String> {
        let _clipboard = Clipboard::open()?;
        if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) }.is_err() {
            // No text in there is not an error — there is simply nothing to
            // paste.
            return Ok(String::new());
        }
        let handle = unsafe { GetClipboardData(CF_UNICODETEXT) }
            .map_err(|err| format!("read clipboard: {err}"))?;
        let block = HGLOBAL(handle.0);
        let source = unsafe { GlobalLock(block) } as *const u16;
        if source.is_null() {
            return Err("Could not read the clipboard contents.".into());
        }
        // Up to the trailing null. With an upper bound so a broken block is
        // not read forever.
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
pub use win::{copy_image, copy_text, read_text};

#[cfg(not(windows))]
pub fn copy_image(_dib: &[u8]) -> Result<(), String> {
    Err("The clipboard is only available on Windows.".into())
}

#[cfg(windows)]
pub fn copy_files(paths: &[std::path::PathBuf]) -> Result<(), String> {
    let refs: Vec<&std::path::Path> = paths.iter().map(|p| p.as_path()).collect();
    win::copy_files(&refs)
}

#[cfg(not(windows))]
pub fn copy_files(_paths: &[std::path::PathBuf]) -> Result<(), String> {
    Err("The clipboard is only available on Windows.".into())
}

#[cfg(not(windows))]
pub fn copy_text(_text: &str) -> Result<(), String> {
    Err("The clipboard is only available on Windows.".into())
}

#[cfg(not(windows))]
pub fn read_text() -> Result<String, String> {
    Ok(String::new())
}
