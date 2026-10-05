//! The "ClippiBoy" mark in the bottom-left corner of every clip and screenshot.
//!
//! Video and pictures take two different roads to it. The video never passes
//! through main memory, so the mark is drawn onto each frame on the GPU before
//! the encoder sees it (`convert::Stamp`) — that way the buffer, recordings and
//! exports all carry it without a single extra encode. A screenshot is already
//! in main memory, and gets it here in [`stamp`] before it is written.
//!
//! Both read the same picture and the same [`rect`], so the corner looks alike
//! in a clip and a screenshot of the same moment.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use crate::shot::Shot;

/// Logo plus lettering, white with a soft shadow, fully opaque — made by
/// `scripts/make-watermark.py`. The opacity is applied when drawing.
const PICTURE: &[u8] = include_bytes!("../assets/watermark.png");

/// How strongly the mark lies over the picture.
pub const OPACITY: f32 = 0.65;
/// The picture's height as a share of the frame's. The PNG carries some room
/// for its shadow, so the logo itself ends up near 6 %. (3 % was too small to
/// read on a phone — Fabi, 2026-10-05.)
const HEIGHT_SHARE: f32 = 0.07;
/// Distance to the left and bottom edge, as a share of the frame's height —
/// of the height for both, so the corner looks the same on an ultrawide.
const MARGIN_SHARE: f32 = 0.015;
/// Below this the lettering is mush; better none than a smudge.
const MIN_HEIGHT: u32 = 14;

/// Read on every frame — a switch in the settings takes effect with the next.
static ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Where the mark goes in a frame of this size. `None` when the frame is too
/// small to hold it legibly.
pub fn rect(frame_width: u32, frame_height: u32, picture_width: u32, picture_height: u32) -> Option<Rect> {
    if picture_width == 0 || picture_height == 0 {
        return None;
    }
    let height = (frame_height as f32 * HEIGHT_SHARE).round() as u32;
    if height < MIN_HEIGHT {
        return None;
    }
    let width = (height as u64 * picture_width as u64 / picture_height as u64) as u32;
    let margin = (frame_height as f32 * MARGIN_SHARE).round() as u32;
    // A narrow window (a portrait phone mirror, say) would have the lettering
    // run across half of it.
    if width + 2 * margin > frame_width / 2 {
        return None;
    }
    Some(Rect {
        x: margin,
        y: frame_height - margin - height,
        width,
        height,
    })
}

/// The decoded picture: width, height, straight RGBA.
pub fn picture() -> &'static (u32, u32, Vec<u8>) {
    static PICTURE_RGBA: OnceLock<(u32, u32, Vec<u8>)> = OnceLock::new();
    PICTURE_RGBA.get_or_init(|| {
        crate::shot::read_rgba(PICTURE).unwrap_or_else(|err| {
            log::error!("watermark picture unreadable: {err}");
            (0, 0, Vec::new())
        })
    })
}

/// Like [`rect`], but for a picture that has to keep its mark whatever was done
/// to it — an edited screenshot. A crop down to a small piece or a narrow strip
/// would otherwise lose it, and cropping the mark away is exactly what it must
/// not allow. Below the legible size it stays at the smallest one, and a
/// picture too small even for that gets it shrunk to fit. `None` only for a
/// picture of a few pixels.
pub fn rect_kept(frame_width: u32, frame_height: u32, picture_width: u32, picture_height: u32) -> Option<Rect> {
    if let Some(at) = rect(frame_width, frame_height, picture_width, picture_height) {
        return Some(at);
    }
    if picture_width == 0 || picture_height == 0 {
        return None;
    }
    let margin = (frame_height as f32 * MARGIN_SHARE).round() as u32;
    let mut height = ((frame_height as f32 * HEIGHT_SHARE).round() as u32).max(MIN_HEIGHT);
    let mut width = (height as u64 * picture_width as u64 / picture_height as u64) as u32;
    let room_width = frame_width.saturating_sub(2 * margin);
    let room_height = frame_height.saturating_sub(2 * margin);
    if width > room_width {
        width = room_width;
        height = (width as u64 * picture_height as u64 / picture_width as u64) as u32;
    }
    if height > room_height {
        height = room_height;
        width = (height as u64 * picture_width as u64 / picture_height as u64) as u32;
    }
    if width == 0 || height == 0 {
        return None;
    }
    Some(Rect {
        x: margin,
        y: frame_height - margin - height,
        width,
        height,
    })
}

/// Lay the mark into a fresh screenshot, if it is switched on.
///
/// Hands back what lay under it, and where: the editor puts that back before it
/// works on the picture and stamps the result anew (`stamp_kept`). Otherwise
/// the mark could be painted over or cropped away.
pub fn stamp(shot: &mut Shot) -> Option<(Rect, Shot)> {
    if !enabled() {
        return None;
    }
    let (picture_width, picture_height, _) = picture();
    let at = rect(shot.width, shot.height, *picture_width, *picture_height)?;
    let under = shot.crop(at.x, at.y, at.width, at.height).ok()?;
    stamp_at(shot, at);
    Some((at, under))
}

/// Lay the mark into an edited screenshot — always, see [`rect_kept`].
pub fn stamp_kept(shot: &mut Shot) {
    let (picture_width, picture_height, _) = picture();
    if let Some(at) = rect_kept(shot.width, shot.height, *picture_width, *picture_height) {
        stamp_at(shot, at);
    }
}

fn stamp_at(shot: &mut Shot, at: Rect) {
    let (picture_width, picture_height, rgba) = picture();
    let scaled = scale(rgba, *picture_width, *picture_height, at.width, at.height);
    let stride = shot.width as usize * 3;
    for row in 0..at.height as usize {
        for column in 0..at.width as usize {
            // Premultiplied, so a half-covered edge pixel brings half its colour.
            let [red, green, blue, alpha] = scaled[row * at.width as usize + column];
            let alpha = alpha * OPACITY;
            if alpha <= 0.0 {
                continue;
            }
            let at_pixel = (at.y as usize + row) * stride + (at.x as usize + column) * 3;
            for (channel, value) in [red, green, blue].into_iter().enumerate() {
                let under = shot.rgb[at_pixel + channel] as f32;
                let out = value * OPACITY + under * (1.0 - alpha);
                shot.rgb[at_pixel + channel] = out.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// Shrink a straight-RGBA picture by averaging the area each target pixel
/// covers. Returns premultiplied colour (0–255) and alpha (0–1).
///
/// Bilinear would sample four pixels out of the dozen a target pixel covers at
/// 1080p, and the lettering would flicker between thin and bold. The area
/// average is what a GPU's mipmap would have given.
fn scale(rgba: &[u8], width: u32, height: u32, to_width: u32, to_height: u32) -> Vec<[f32; 4]> {
    let step_x = width as f32 / to_width as f32;
    let step_y = height as f32 / to_height as f32;
    let mut out = Vec::with_capacity((to_width * to_height) as usize);
    for target_y in 0..to_height {
        let top = (target_y as f32 * step_y) as u32;
        let bottom = (((target_y + 1) as f32 * step_y).ceil() as u32).clamp(top + 1, height);
        for target_x in 0..to_width {
            let left = (target_x as f32 * step_x) as u32;
            let right = (((target_x + 1) as f32 * step_x).ceil() as u32).clamp(left + 1, width);
            let mut sum = [0.0f32; 4];
            for y in top..bottom {
                for x in left..right {
                    let pixel = &rgba[((y * width + x) * 4) as usize..][..4];
                    let alpha = pixel[3] as f32 / 255.0;
                    sum[0] += pixel[0] as f32 * alpha;
                    sum[1] += pixel[1] as f32 * alpha;
                    sum[2] += pixel[2] as f32 * alpha;
                    sum[3] += alpha;
                }
            }
            let count = ((bottom - top) * (right - left)) as f32;
            out.push([sum[0] / count, sum[1] / count, sum[2] / count, sum[3] / count]);
        }
    }
    out
}

/// The picture as premultiplied BGRA — what Direct2D wants for its bitmap.
pub fn premultiplied_bgra() -> Vec<u8> {
    let (_, _, rgba) = picture();
    rgba.chunks_exact(4)
        .flat_map(|pixel| {
            let alpha = pixel[3] as u32;
            let mul = |value: u8| ((value as u32 * alpha + 127) / 255) as u8;
            [mul(pixel[2]), mul(pixel[1]), mul(pixel[0]), pixel[3]]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_decodes() {
        let (width, height, rgba) = picture();
        assert!(*width > *height && *height > 0);
        assert_eq!(rgba.len(), (*width * *height * 4) as usize);
    }

    #[test]
    fn full_hd_sits_in_the_bottom_left() {
        let at = rect(1920, 1080, 535, 152).unwrap();
        assert_eq!(at.height, 76);
        assert_eq!(at.x, 16);
        assert_eq!(at.y + at.height + at.x, 1080);
        assert_eq!(at.width, 76 * 535 / 152);
    }

    #[test]
    fn it_grows_with_the_frame() {
        let hd = rect(1920, 1080, 535, 152).unwrap();
        let qhd = rect(2560, 1440, 535, 152).unwrap();
        let uhd = rect(3840, 2160, 535, 152).unwrap();
        assert!(hd.height < qhd.height && qhd.height < uhd.height);
        assert_eq!(uhd.height, 151);
    }

    #[test]
    fn an_ultrawide_gets_the_same_corner_as_a_16_9() {
        assert_eq!(rect(3440, 1440, 535, 152), rect(2560, 1440, 535, 152));
    }

    #[test]
    fn a_tiny_window_goes_without() {
        assert_eq!(rect(160, 120, 535, 152), None);
        // Tall but narrow: the lettering would cover half of it.
        assert_eq!(rect(300, 1200, 535, 152), None);
    }

    #[test]
    fn a_screenshot_gets_it_and_stays_untouched_elsewhere() {
        let mut shot = Shot {
            width: 1920,
            height: 1080,
            rgb: vec![10; 1920 * 1080 * 3],
        };
        let (at, under) = stamp(&mut shot).unwrap();
        assert_eq!(at, rect(1920, 1080, picture().0, picture().1).unwrap());
        // What lay under it comes back untouched.
        assert_eq!((under.width, under.height), (at.width, at.height));
        assert!(under.rgb.iter().all(|&value| value == 10));
        // Top-right corner: nothing.
        assert_eq!(shot.rgb[(1919) * 3], 10);
        // Somewhere in the mark the picture got lighter.
        let changed = (at.y..at.y + at.height).any(|y| {
            (at.x..at.x + at.width).any(|x| shot.rgb[((y * 1920 + x) * 3) as usize] > 40)
        });
        assert!(changed);
    }

    #[test]
    fn an_edit_keeps_the_mark_however_small_the_crop() {
        // Too small for the ordinary rule…
        assert_eq!(rect(400, 150, 535, 152), None);
        // …but an edited picture keeps it, at the smallest legible size.
        let at = rect_kept(400, 150, 535, 152).unwrap();
        assert_eq!(at.height, MIN_HEIGHT);
        assert_eq!(at.y + at.height + at.x, 150);
        // A narrow strip shrinks it until it fits.
        let at = rect_kept(40, 300, 535, 152).unwrap();
        assert!(at.x + at.width <= 40);
        // Where the ordinary rule fits, both agree.
        assert_eq!(rect_kept(1920, 1080, 535, 152), rect(1920, 1080, 535, 152));
    }
}
