//! Which icon does Windows actually hold for ClippiBoy's windows?
//!
//!     cargo run --example icon-probe
//!
//! Only reads: it walks the top-level windows of every running `clippiboy.exe`
//! and prints, per window, whether it can have a task bar button, the icons it
//! answers `WM_GETICON` with and those of its window class — each with size and
//! average colour, so a violet icon and a recoloured one tell apart — and the
//! AppUserModelID the shell sees on it. Written for the case "the colour shows
//! in the tray but not in the task bar, only in the installed build".

#[cfg(windows)]
fn main() {
    probe::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only.");
}

#[cfg(windows)]
mod probe {
    use std::collections::HashSet;

    use windows::core::{GUID, PWSTR};
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
    };
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::UI::Shell::PropertiesSystem::{
        IPropertyStore, SHGetPropertyStoreForWindow, PROPERTYKEY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassLongPtrW, GetClassNameW, GetIconInfo, GetWindow,
        GetWindowLongPtrW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
        SendMessageTimeoutW, GCLP_HICON, GCLP_HICONSM, GWL_EXSTYLE, GW_OWNER, HICON,
        ICONINFO, ICON_BIG, ICON_SMALL, ICON_SMALL2, SMTO_ABORTIFHUNG, WM_GETICON,
        WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    };

    /// `PKEY_AppUserModel_ID` — written out, the constant sits behind a feature
    /// the app itself does not need.
    const AUMID: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3),
        pid: 5,
    };

    pub fn run() {
        let pids = clippiboy_pids();
        if pids.is_empty() {
            println!("No clippiboy.exe is running.");
            return;
        }
        println!("clippiboy.exe processes: {pids:?}\n");
        let mut windows: Vec<HWND> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM(&mut windows as *mut _ as isize));
        }
        for hwnd in windows {
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            if pids.contains(&pid) {
                describe(hwnd, pid);
            }
        }
    }

    unsafe extern "system" fn collect(hwnd: HWND, list: LPARAM) -> BOOL {
        let list = &mut *(list.0 as *mut Vec<HWND>);
        list.push(hwnd);
        BOOL(1)
    }

    fn clippiboy_pids() -> HashSet<u32> {
        let mut found = HashSet::new();
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                return found;
            };
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            let mut ok = Process32FirstW(snap, &mut entry).is_ok();
            while ok {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
                let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
                if name.eq_ignore_ascii_case("clippiboy.exe") {
                    found.insert(entry.th32ProcessID);
                }
                ok = Process32NextW(snap, &mut entry).is_ok();
            }
        }
        found
    }

    fn describe(hwnd: HWND, pid: u32) {
        let mut title = [0u16; 256];
        let mut class = [0u16; 256];
        let (title, class, visible, ex, owner) = unsafe {
            let t = GetWindowTextW(hwnd, &mut title) as usize;
            let c = GetClassNameW(hwnd, &mut class) as usize;
            (
                String::from_utf16_lossy(&title[..t]),
                String::from_utf16_lossy(&class[..c]),
                IsWindowVisible(hwnd).as_bool(),
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32,
                GetWindow(hwnd, GW_OWNER).map(|h| !h.0.is_null()).unwrap_or(false),
            )
        };
        let tool = ex & WS_EX_TOOLWINDOW.0 != 0;
        let app = ex & WS_EX_APPWINDOW.0 != 0;
        // The shell's own rule for a task bar button.
        let taskbar = visible && !tool && (!owner || app);
        println!(
            "window {:?}  pid {pid}  class \"{class}\"  title \"{title}\"",
            hwnd.0
        );
        println!(
            "  visible {visible}  toolwindow {tool}  appwindow {app}  owned {owner}  -> task bar button: {}",
            if taskbar { "YES" } else { "no" }
        );
        if !visible {
            println!();
            return;
        }
        for (name, kind) in [("ICON_BIG", ICON_BIG), ("ICON_SMALL", ICON_SMALL), ("ICON_SMALL2", ICON_SMALL2)] {
            let mut result = 0usize;
            unsafe {
                SendMessageTimeoutW(
                    hwnd,
                    WM_GETICON,
                    WPARAM(kind as usize),
                    LPARAM(0),
                    SMTO_ABORTIFHUNG,
                    1000,
                    Some(&mut result),
                );
            }
            println!("  {name:<12} {}", icon_text(result as isize));
        }
        for (name, index) in [("class HICON", GCLP_HICON), ("class HICONSM", GCLP_HICONSM)] {
            let handle = unsafe { GetClassLongPtrW(hwnd, index) } as isize;
            println!("  {name:<12} {}", icon_text(handle));
        }
        println!("  AUMID        {}", aumid(hwnd));
        println!();
    }

    /// "32×32, average #7c4fd8" — or "none".
    fn icon_text(handle: isize) -> String {
        if handle == 0 {
            return "none".into();
        }
        let icon = HICON(handle as *mut std::ffi::c_void);
        let mut info = ICONINFO::default();
        if unsafe { GetIconInfo(icon, &mut info) }.is_err() {
            return format!("{handle:#x} (unreadable)");
        }
        let text = match average(info.hbmColor) {
            Some((w, h, rgb)) => format!("{handle:#x}  {w}×{h}  average #{rgb:06x}"),
            None => format!("{handle:#x}  (no colour bitmap)"),
        };
        unsafe {
            let _ = DeleteObject(info.hbmColor);
            let _ = DeleteObject(info.hbmMask);
        }
        text
    }

    /// Size and mean colour of the pixels that are not transparent.
    fn average(bitmap: HBITMAP) -> Option<(i32, i32, u32)> {
        if bitmap.0.is_null() {
            return None;
        }
        let mut bm = BITMAP::default();
        let got = unsafe {
            GetObjectW(
                bitmap,
                std::mem::size_of::<BITMAP>() as i32,
                Some(&mut bm as *mut _ as *mut std::ffi::c_void),
            )
        };
        if got == 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth, bm.bmHeight);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        let rows = unsafe {
            let dc = GetDC(None);
            let rows = GetDIBits(
                dc,
                bitmap,
                0,
                h as u32,
                Some(pixels.as_mut_ptr() as *mut std::ffi::c_void),
                &mut info,
                DIB_RGB_COLORS,
            );
            ReleaseDC(None, dc);
            rows
        };
        if rows == 0 {
            return None;
        }
        let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
        for px in pixels.chunks_exact(4) {
            if px[3] < 128 {
                continue;
            }
            b += px[0] as u64;
            g += px[1] as u64;
            r += px[2] as u64;
            n += 1;
        }
        let n = n.max(1);
        Some((w, h, ((r / n) << 16 | (g / n) << 8 | (b / n)) as u32))
    }

    fn aumid(hwnd: HWND) -> String {
        unsafe {
            let store: windows::core::Result<IPropertyStore> = SHGetPropertyStoreForWindow(hwnd);
            let Ok(store) = store else {
                return "(no property store)".into();
            };
            let Ok(value) = store.GetValue(&AUMID) else {
                return "(unreadable)".into();
            };
            match PropVariantToStringAlloc(&value) {
                Ok(text) => {
                    let s: PWSTR = text;
                    let out = s.to_string().unwrap_or_default();
                    if out.is_empty() {
                        "none (Windows derives one from the exe path)".into()
                    } else {
                        out
                    }
                }
                Err(_) => "none (Windows derives one from the exe path)".into(),
            }
        }
    }
}
