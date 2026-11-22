//! Still images of the screens for the Video page's monitor map.
//!
//! GDI, not Windows.Graphics.Capture: even a single WGC frame puts the yellow
//! capture border around the screen, and the page asks every few seconds.
//! `StretchBlt` from the screen DC with `HALFTONE` shrinks straight down to
//! the small picture, so no full-size copy is ever made; one screen after the
//! other, on the CPU. A window that keeps itself out of captures comes out
//! black or missing, as in any other screenshot.
//!
//! Nothing is taken unless ClippiBoy's main window is the one in front and
//! not minimised — a game in front with the Video page still open behind it
//! must not have its screen read every few seconds — nor while the lock
//! screen or a UAC prompt is in front, nor in streamer mode: then every
//! screen answers with no picture.

use serde::Serialize;

use crate::model::{CaptureTarget, TargetKind, TargetRect};

/// The long side of a still, in pixels. The map draws screens a few hundred
/// pixels wide at most.
pub const LONG_SIDE: u32 = 320;
/// JPEG quality: soft is fine at this size, the page fetches often.
const QUALITY: u8 = 70;
/// The most screens one call looks at. More is no setup anyone has.
const MAX_IDS: usize = 16;

/// One screen's still, or none when there is none to give.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Thumb {
    pub id: String,
    /// `data:image/jpeg;base64,…`
    pub jpeg: Option<String>,
}

/// The still's size for a screen of `width` × `height`: the aspect kept, the
/// long side [`LONG_SIDE`] — never larger than the screen itself.
pub fn thumb_size(width: u32, height: u32) -> Option<(u32, u32)> {
    if width == 0 || height == 0 {
        return None;
    }
    let long = width.max(height);
    if long <= LONG_SIDE {
        return Some((width, height));
    }
    let scale = |side: u32| ((side as u64 * LONG_SIDE as u64 + long as u64 / 2) / long as u64).max(1) as u32;
    Some((scale(width), scale(height)))
}

/// 32-bit BGRA (or BGRX) pixels as packed RGB, as the JPEG wants them.
pub fn bgra_to_rgb(bgra: &[u8]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(bgra.len() / 4 * 3);
    for px in bgra.chunks_exact(4) {
        rgb.extend_from_slice(&[px[2], px[1], px[0]]);
    }
    rgb
}

/// `rgb` (`width` × `height`) as a JPEG data URL.
pub fn jpeg_data_url(width: u32, height: u32, rgb: &[u8]) -> Option<String> {
    let (w, h) = (u16::try_from(width).ok()?, u16::try_from(height).ok()?);
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, QUALITY)
        .encode(rgb, w, h, jpeg_encoder::ColorType::Rgb)
        .ok()?;
    Some(format!("data:image/jpeg;base64,{}", crate::appicon::base64(&out)))
}

/// The answer for `ids`, in their order: a still from `grab` for each screen
/// among `monitors`, none for windows and for ids nobody knows — and none at
/// all while `blocked`.
pub fn thumbs_for(
    ids: &[String],
    monitors: &[CaptureTarget],
    blocked: bool,
    mut grab: impl FnMut(TargetRect) -> Option<String>,
) -> Vec<Thumb> {
    ids.iter()
        .take(MAX_IDS)
        .map(|id| {
            let jpeg = if blocked {
                None
            } else {
                monitors
                    .iter()
                    .find(|m| m.kind == TargetKind::Monitor && &m.id == id)
                    .and_then(|m| grab(TargetRect { x: m.x, y: m.y, width: m.width, height: m.height }))
            };
            Thumb { id: id.clone(), jpeg }
        })
        .collect()
}

/// What decides whether stills may be taken at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Moment {
    /// Lock screen, sign-in or UAC prompt in front.
    pub unrecordable: bool,
    /// Streamer mode is in effect.
    pub streamer: bool,
    /// ClippiBoy's main window, raw; `None` when it is not there.
    pub main: Option<isize>,
    /// The window in front, raw; 0 for none.
    pub foreground: isize,
    /// The main window is minimised.
    pub minimised: bool,
}

/// Stills only for someone looking at the Video page: the main window in
/// front and open, the screen recordable, streamer mode off.
pub fn may_grab(at: Moment) -> bool {
    !at.unrecordable
        && !at.streamer
        && !at.minimised
        && at.main.is_some_and(|main| main != 0 && main == at.foreground)
}

/// Stills of the screens named in `ids` (device names, `\\.\DISPLAY1`).
/// `main` is ClippiBoy's main window, raw.
pub fn capture(ids: &[String], main: Option<isize>) -> Vec<Thumb> {
    let started = std::time::Instant::now();
    let moment = Moment {
        unrecordable: crate::capture::screen_unrecordable(),
        streamer: crate::streamer::active(),
        main,
        foreground: foreground(),
        minimised: main.is_some_and(minimised),
    };
    let blocked = !may_grab(moment);
    let monitors = if blocked || ids.is_empty() { Vec::new() } else { crate::capture::list_monitors() };
    let thumbs = thumbs_for(ids, &monitors, blocked, grab);
    log::debug!(
        "stills: {} of {} screens in {} ms{}",
        thumbs.iter().filter(|t| t.jpeg.is_some()).count(),
        ids.len(),
        started.elapsed().as_millis(),
        if blocked { " (not taken)" } else { "" },
    );
    thumbs
}

#[cfg(windows)]
fn grab(rect: TargetRect) -> Option<String> {
    let started = std::time::Instant::now();
    let (width, height) = thumb_size(rect.width, rect.height)?;
    let bgra = unsafe { win::stretch(rect, width, height) };
    let url = bgra.and_then(|bgra| jpeg_data_url(width, height, &bgra_to_rgb(&bgra)));
    log::debug!(
        "still of the screen at {},{} ({}x{}): {} in {} ms",
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        if url.is_some() { "taken" } else { "failed" },
        started.elapsed().as_millis(),
    );
    url
}

#[cfg(windows)]
fn foreground() -> isize {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    unsafe { GetForegroundWindow() }.0 as isize
}

#[cfg(windows)]
fn minimised(window: isize) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::IsIconic;
    unsafe { IsIconic(HWND(window as *mut core::ffi::c_void)) }.as_bool()
}

#[cfg(not(windows))]
fn foreground() -> isize {
    0
}

#[cfg(not(windows))]
fn minimised(_window: isize) -> bool {
    false
}

#[cfg(not(windows))]
fn grab(_rect: TargetRect) -> Option<String> {
    None
}

#[cfg(windows)]
mod win {
    use windows::Win32::Foundation::{HANDLE, HWND};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, GetDC, ReleaseDC,
        SelectObject, SetBrushOrgEx, SetStretchBltMode, StretchBlt, BITMAPINFO, BITMAPINFOHEADER,
        BI_RGB, DIB_RGB_COLORS, HALFTONE, SRCCOPY,
    };

    use crate::model::TargetRect;

    /// The screen area `rect` (virtual-desktop pixels, as `GetMonitorInfoW`
    /// gives them — the process is per-monitor DPI aware, so the screen DC
    /// counts in the same pixels) shrunk to `width` × `height`: BGRX, top row
    /// first.
    pub unsafe fn stretch(rect: TargetRect, width: u32, height: u32) -> Option<Vec<u8>> {
        let screen = GetDC(HWND::default());
        if screen.is_invalid() {
            return None;
        }
        let memory = CreateCompatibleDC(screen);
        let pixels = if memory.is_invalid() { None } else { into_dib(screen, memory, rect, width, height) };
        if !memory.is_invalid() {
            let _ = DeleteDC(memory);
        }
        ReleaseDC(HWND::default(), screen);
        pixels
    }

    unsafe fn into_dib(
        screen: windows::Win32::Graphics::Gdi::HDC,
        memory: windows::Win32::Graphics::Gdi::HDC,
        rect: TargetRect,
        width: u32,
        height: u32,
    ) -> Option<Vec<u8>> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                // Negative: rows top-down.
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0).ok()?;
        if bits.is_null() {
            let _ = DeleteObject(bitmap);
            return None;
        }
        let previous = SelectObject(memory, bitmap);
        if previous.is_invalid() {
            let _ = DeleteObject(bitmap);
            return None;
        }
        // HALFTONE averages the pixels each one covers, so text and edges stay
        // readable; it wants the brush origin set after the mode.
        SetStretchBltMode(memory, HALFTONE);
        let _ = SetBrushOrgEx(memory, 0, 0, None);
        let drawn = StretchBlt(
            memory,
            0,
            0,
            width as i32,
            height as i32,
            screen,
            rect.x,
            rect.y,
            rect.width as i32,
            rect.height as i32,
            SRCCOPY,
        )
        .as_bool();
        let _ = GdiFlush();
        let pixels = drawn.then(|| {
            std::slice::from_raw_parts(bits as *const u8, width as usize * height as usize * 4).to_vec()
        });
        SelectObject(memory, previous);
        let _ = DeleteObject(bitmap);
        pixels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(id: &str, x: i32, width: u32, height: u32) -> CaptureTarget {
        CaptureTarget {
            kind: TargetKind::Monitor,
            id: id.into(),
            stable_id: None,
            title: id.into(),
            width,
            height,
            is_primary: x == 0,
            refresh_hz: None,
            x,
            y: 0,
            icon: None,
            is_game: false,
        }
    }

    #[test]
    fn the_still_keeps_the_aspect_with_the_long_side_capped() {
        assert_eq!(thumb_size(2560, 1440), Some((320, 180)));
        assert_eq!(thumb_size(1920, 1080), Some((320, 180)));
        assert_eq!(thumb_size(3440, 1440), Some((320, 134)));
        // A screen turned upright: the height is the long side.
        assert_eq!(thumb_size(1080, 1920), Some((180, 320)));
        assert_eq!(thumb_size(1280, 1024), Some((320, 256)));
    }

    #[test]
    fn a_small_or_empty_screen_is_never_blown_up() {
        assert_eq!(thumb_size(300, 200), Some((300, 200)));
        assert_eq!(thumb_size(0, 1080), None);
        assert_eq!(thumb_size(1920, 0), None);
        // Absurdly wide still keeps one row.
        assert_eq!(thumb_size(100_000, 10), Some((320, 1)));
    }

    #[test]
    fn bgra_becomes_rgb_and_the_fourth_byte_goes() {
        let bgra = [1, 2, 3, 255, 10, 20, 30, 0];
        assert_eq!(bgra_to_rgb(&bgra), vec![3, 2, 1, 30, 20, 10]);
        assert!(bgra_to_rgb(&[]).is_empty());
    }

    #[test]
    fn the_jpeg_comes_as_a_data_url() {
        let rgb = vec![128u8; 16 * 9 * 3];
        let url = jpeg_data_url(16, 9, &rgb).unwrap();
        // base64 of the JPEG start-of-image marker FF D8 FF.
        assert!(url.starts_with("data:image/jpeg;base64,/9j/"), "{url}");
        assert_eq!(jpeg_data_url(70_000, 1, &[]), None);
    }

    #[test]
    fn each_screen_gets_its_own_still_in_the_order_asked() {
        let monitors = vec![screen("\\\\.\\DISPLAY1", 0, 2560, 1440), screen("\\\\.\\DISPLAY2", -1920, 1920, 1080)];
        let ids = vec!["\\\\.\\DISPLAY2".to_string(), "0x1A2B".into(), "\\\\.\\DISPLAY1".into(), "\\\\.\\DISPLAY9".into()];
        let mut seen = Vec::new();
        let thumbs = thumbs_for(&ids, &monitors, false, |rect| {
            seen.push(rect);
            Some(format!("still at {}", rect.x))
        });
        assert_eq!(
            thumbs,
            vec![
                Thumb { id: ids[0].clone(), jpeg: Some("still at -1920".into()) },
                // A window and a screen nobody knows: nothing, and no grab.
                Thumb { id: ids[1].clone(), jpeg: None },
                Thumb { id: ids[2].clone(), jpeg: Some("still at 0".into()) },
                Thumb { id: ids[3].clone(), jpeg: None },
            ]
        );
        assert_eq!(seen, vec![
            TargetRect { x: -1920, y: 0, width: 1920, height: 1080 },
            TargetRect { x: 0, y: 0, width: 2560, height: 1440 },
        ]);
    }

    #[test]
    fn nothing_is_taken_while_blocked() {
        let monitors = vec![screen("\\\\.\\DISPLAY1", 0, 2560, 1440)];
        let ids = vec!["\\\\.\\DISPLAY1".to_string()];
        let thumbs = thumbs_for(&ids, &monitors, true, |_| panic!("no grab on the lock screen"));
        assert_eq!(thumbs, vec![Thumb { id: ids[0].clone(), jpeg: None }]);
    }

    const IN_FRONT: Moment = Moment {
        unrecordable: false,
        streamer: false,
        main: Some(0x1234),
        foreground: 0x1234,
        minimised: false,
    };

    #[test]
    fn stills_are_taken_with_the_main_window_in_front() {
        assert!(may_grab(IN_FRONT));
    }

    #[test]
    fn a_game_in_front_gets_no_stills_taken() {
        assert!(!may_grab(Moment { foreground: 0x9999, ..IN_FRONT }));
        assert!(!may_grab(Moment { foreground: 0, ..IN_FRONT }));
    }

    #[test]
    fn no_stills_minimised_without_a_window_locked_or_streaming() {
        assert!(!may_grab(Moment { minimised: true, ..IN_FRONT }));
        assert!(!may_grab(Moment { main: None, ..IN_FRONT }));
        assert!(!may_grab(Moment { main: Some(0), foreground: 0, ..IN_FRONT }));
        assert!(!may_grab(Moment { unrecordable: true, ..IN_FRONT }));
        assert!(!may_grab(Moment { streamer: true, ..IN_FRONT }));
    }

    #[test]
    fn a_flood_of_ids_is_cut() {
        let ids: Vec<String> = (0..100).map(|n| format!("id{n}")).collect();
        assert_eq!(thumbs_for(&ids, &[], false, |_| None).len(), MAX_IDS);
    }
}
