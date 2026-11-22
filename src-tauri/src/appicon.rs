//! Application icons for the window list on the Video page.
//!
//! A window is drawn with the icon of the program behind it: its exe path from
//! the process, the shell's own icon for that file (`IShellItemImageFactory`),
//! as a small PNG data URL the page can put straight into an `<img>`. Each exe
//! is looked at once per run — the answer, a missing one included, is kept.
//! A game behind an anti-cheat may refuse to be opened; it gets no icon and
//! the page draws its generic one.

use std::collections::HashMap;
use std::sync::OnceLock;

use parking_lot::Mutex;

use crate::model::{CaptureTarget, TargetKind};

/// Edge length the shell is asked for. The list shows the icon at 22 px and
/// the window card at 64 px, so this is sharp on both at 200 % scaling too.
#[cfg_attr(not(windows), allow(dead_code))]
const ICON_SIZE: i32 = 64;

/// More entries than this and the cache starts over — no machine runs that
/// many different programs with windows, it is only a ceiling.
const CACHE_LIMIT: usize = 512;

fn cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The cache's key for an exe: Windows paths do not care about case, and one
/// program reached as `C:\Games\…` and `c:\games\…` is still one program.
pub fn cache_key(path: &str) -> String {
    path.to_lowercase()
}

/// Adds the icon and the game mark to every window. `game_pid` is the process
/// the game detection is bound to.
#[cfg(windows)]
pub fn dress_windows(targets: &mut [CaptureTarget], game_pid: Option<u32>) {
    // The exe of every window first: cheap, and it shows which ones are new.
    let paths: Vec<Option<(u32, String)>> = targets
        .iter()
        .map(|t| match t.kind {
            TargetKind::Window => win::process_of(&t.id),
            TargetKind::Monitor => None,
        })
        .collect();

    let missing: Vec<String> = {
        let known = cache().lock();
        // One lookup per program, however many windows it has open.
        let mut missing: HashMap<String, String> = HashMap::new();
        for (_, path) in paths.iter().flatten() {
            let key = cache_key(path);
            if !known.contains_key(&key) {
                missing.entry(key).or_insert_with(|| path.clone());
            }
        }
        missing.into_values().collect()
    };
    if !missing.is_empty() {
        let found = win::icons(missing);
        let mut known = cache().lock();
        if known.len() + found.len() > CACHE_LIMIT {
            known.clear();
        }
        for (path, icon) in found {
            known.insert(cache_key(&path), icon);
        }
    }

    let known = cache().lock();
    for (target, process) in targets.iter_mut().zip(paths) {
        let Some((pid, path)) = process else { continue };
        target.icon = known.get(&cache_key(&path)).cloned().flatten();
        target.is_game = game_pid == Some(pid);
    }
}

#[cfg(not(windows))]
pub fn dress_windows(_targets: &mut [CaptureTarget], _game_pid: Option<u32>) {}

/// Says once which DPI awareness the target list is read under. Under
/// per-monitor v2 (what tao sets for the process) `rcMonitor` and the sizes are
/// physical pixels, the same system the capture works in; under anything else
/// Windows would scale them and the map would be off on mixed-DPI desktops.
#[cfg(windows)]
pub fn log_dpi_awareness() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        use windows::Win32::UI::HiDpi::{
            GetAwarenessFromDpiAwarenessContext, GetThreadDpiAwarenessContext,
        };
        // 0 unaware, 1 system aware, 2 per monitor (v1 or v2).
        let awareness =
            unsafe { GetAwarenessFromDpiAwarenessContext(GetThreadDpiAwarenessContext()) };
        log::info!("capture targets read with DPI awareness {}", awareness.0);
    });
}

#[cfg(not(windows))]
pub fn log_dpi_awareness() {}

/// The bitmap the shell hands out, BGRA with premultiplied alpha, as straight
/// RGBA for the PNG. A bitmap whose alpha is zero throughout has none at all
/// (an old-style icon) and is opaque, not invisible.
pub fn bgra_premultiplied_to_rgba(pixels: &mut [u8]) {
    let has_alpha = pixels.chunks_exact(4).any(|p| p[3] != 0);
    for p in pixels.chunks_exact_mut(4) {
        p.swap(0, 2);
        if !has_alpha {
            p[3] = 255;
            continue;
        }
        let a = p[3] as u32;
        if a == 0 || a == 255 {
            continue;
        }
        for c in &mut p[..3] {
            *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
        }
    }
}

/// RGBA pixels as a PNG data URL.
pub fn png_data_url(width: u32, height: u32, rgba: &[u8]) -> Option<String> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(rgba).ok()?;
    }
    Some(format!("data:image/png;base64,{}", base64(&bytes)))
}

/// Standard base64 with padding, as a data URL wants it.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(windows)]
mod win {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HWND, MAX_PATH, SIZE};
    use windows::Win32::Graphics::Gdi::{
        DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
    };
    use windows::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, IBindCtx, COINIT_APARTMENTTHREADED,
        COINIT_DISABLE_OLE1DDE,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK,
        SIIGBF_ICONONLY,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    /// The process behind a window id (`0x1A2B`, see `capture::window_proc`)
    /// and the full path of its exe.
    pub fn process_of(id: &str) -> Option<(u32, String)> {
        let raw = usize::from_str_radix(id.trim_start_matches("0x"), 16).ok()?;
        let hwnd = HWND(raw as *mut _);
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == 0 {
            return None;
        }
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; MAX_PATH as usize * 2];
            let mut len = buf.len() as u32;
            let result = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            );
            let _ = CloseHandle(handle);
            result.ok()?;
            Some((pid, String::from_utf16_lossy(&buf[..len as usize])))
        }
    }

    /// The icon of each exe. On a thread of its own: the shell wants a
    /// single-threaded apartment, and the command runs on a pool thread whose
    /// apartment is not ours to choose.
    pub fn icons(paths: Vec<String>) -> Vec<(String, Option<String>)> {
        std::thread::spawn(move || {
            let com = unsafe {
                CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE)
            }
            .is_ok();
            let found = paths
                .into_iter()
                .map(|path| {
                    let icon = icon_of(&path);
                    if icon.is_none() {
                        log::debug!("no icon for {path}");
                    }
                    (path, icon)
                })
                .collect();
            if com {
                unsafe { CoUninitialize() };
            }
            found
        })
        .join()
        .unwrap_or_default()
    }

    fn icon_of(path: &str) -> Option<String> {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let factory: IShellItemImageFactory =
                SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None::<&IBindCtx>).ok()?;
            let bitmap = factory
                .GetImage(
                    SIZE { cx: super::ICON_SIZE, cy: super::ICON_SIZE },
                    SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK,
                )
                .ok()?;
            let pixels = read_bitmap(bitmap);
            let _ = DeleteObject(bitmap);
            let (width, height, mut pixels) = pixels?;
            super::bgra_premultiplied_to_rgba(&mut pixels);
            super::png_data_url(width, height, &pixels)
        }
    }

    /// The bitmap's pixels, 32-bit BGRA, top row first.
    unsafe fn read_bitmap(bitmap: HBITMAP) -> Option<(u32, u32, Vec<u8>)> {
        let mut info = BITMAP::default();
        let size = std::mem::size_of::<BITMAP>() as i32;
        if GetObjectW(bitmap, size, Some(&mut info as *mut _ as *mut _)) == 0 {
            return None;
        }
        let (width, height) = (info.bmWidth, info.bmHeight.abs());
        if width <= 0 || height <= 0 || width > 512 || height > 512 {
            return None;
        }
        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative: rows top-down, as the PNG wants them.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        let dc = GetDC(HWND::default());
        let lines = GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut header,
            DIB_RGB_COLORS,
        );
        ReleaseDC(HWND::default(), dc);
        (lines == height).then_some((width as u32, height as u32, pixels))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_alphabet_with_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xfb, 0xff]), "+/8=");
    }

    #[test]
    fn premultiplied_pixels_come_out_straight_and_in_rgba_order() {
        // B G R A: half-transparent pure red, premultiplied to 128.
        let mut px = vec![0, 0, 128, 128, 10, 20, 30, 255, 0, 0, 0, 0];
        bgra_premultiplied_to_rgba(&mut px);
        assert_eq!(&px[0..4], &[255, 0, 0, 128]);
        assert_eq!(&px[4..8], &[30, 20, 10, 255]);
        assert_eq!(&px[8..12], &[0, 0, 0, 0]);
    }

    #[test]
    fn a_bitmap_without_any_alpha_is_opaque() {
        let mut px = vec![1, 2, 3, 0, 4, 5, 6, 0];
        bgra_premultiplied_to_rgba(&mut px);
        assert_eq!(px, vec![3, 2, 1, 255, 6, 5, 4, 255]);
    }

    #[test]
    fn a_colour_brighter_than_its_alpha_allows_is_clamped() {
        let mut px = vec![200, 0, 0, 100];
        bgra_premultiplied_to_rgba(&mut px);
        assert_eq!(&px[..], &[0, 0, 255, 100]);
    }

    #[test]
    fn one_exe_in_two_spellings_is_one_cache_entry() {
        assert_eq!(
            cache_key(r"C:\Games\CS2\cs2.exe"),
            cache_key(r"c:\games\cs2\CS2.EXE")
        );
    }

    #[test]
    fn the_png_is_a_data_url() {
        let url = png_data_url(1, 1, &[255, 0, 0, 255]).unwrap();
        assert!(url.starts_with("data:image/png;base64,iVBORw0KGgo"));
    }
}
