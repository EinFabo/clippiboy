//! The logo in the accent colour, for the icons Windows draws itself.
//!
//! Inside the windows the logo is SVG and follows the accent through CSS tokens.
//! The tray and the task bar take pixels, so those are recoloured here: every
//! coloured pixel's hue is turned by the distance from the violet to the accent,
//! its saturation scaled by how saturated the accent is compared with the violet,
//! and its brightness left alone — the dark disc stays dark, the bright C stays
//! bright. Turned, not replaced: the icon is drawn in several violets a few
//! degrees apart, and one hue for all of them flattened it (and did not even
//! give the violet back when the violet was picked). Grey pixels stay grey.
//!
//! A gradient or the RGB mode is laid across the icon from the top left to the
//! bottom right: two colours meeting in the middle, or the whole wheel once. It
//! stands still — a tray icon swapped many times a second has Explorer redraw
//! the whole tray and flickers.
//!
//! What Windows reads out of the executable itself — the file, the Start menu,
//! the installer, notifications — cannot be changed from here and stays violet.

use tauri::image::Image;

use crate::model::{Accent, AccentMode};

/// The violet the icons were drawn in.
const VIOLET: [u8; 3] = [0x8b, 0x5c, 0xf6];

/// Below this saturation a pixel counts as grey and is left as it is.
const GREY: f32 = 0.12;

/// `#rrggbb` to red, green and blue — `None` for anything else.
fn parse(hex: &str) -> Option<[u8; 3]> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

/// Hue in degrees, saturation and value from 0 to 1.
fn to_hsv([r, g, b]: [u8; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let span = max - min;
    let hue = if span == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / span).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / span + 2.0)
    } else {
        60.0 * ((r - g) / span + 4.0)
    };
    let saturation = if max == 0.0 { 0.0 } else { span / max };
    (hue, saturation, max)
}

fn from_hsv(hue: f32, saturation: f32, value: f32) -> [u8; 3] {
    let chroma = value * saturation;
    let x = chroma * (1.0 - ((hue / 60.0).rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match (hue / 60.0) as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = value - chroma;
    let byte = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [byte(r), byte(g), byte(b)]
}

/// How a pixel's colour moves: its hue turned by `turn` degrees, its
/// saturation multiplied by `scale`.
#[derive(Clone, Copy)]
struct Shift {
    turn: f32,
    scale: f32,
}

/// The shift that takes the violet to this colour.
fn shift_to(color: [u8; 3]) -> Shift {
    let (violet_hue, violet_saturation, _) = to_hsv(VIOLET);
    let (hue, saturation, _) = to_hsv(color);
    Shift {
        turn: hue - violet_hue,
        scale: saturation / violet_saturation,
    }
}

/// Between two shifts, the short way round the colour wheel.
fn between(a: Shift, b: Shift, t: f32) -> Shift {
    let mut delta = (b.turn - a.turn).rem_euclid(360.0);
    if delta > 180.0 {
        delta -= 360.0;
    }
    Shift {
        turn: a.turn + delta * t,
        scale: a.scale + (b.scale - a.scale) * t,
    }
}

/// Recolour RGBA pixels in place. `shift` is asked once per pixel with where
/// the pixel sits on the diagonal, 0 at the top left and 1 at the bottom right.
fn recolor(rgba: &mut [u8], width: u32, height: u32, shift: impl Fn(f32) -> Shift) {
    let width = width.max(1) as usize;
    let height = height.max(1) as f32;
    for (at, pixel) in rgba.chunks_exact_mut(4).enumerate() {
        if pixel[3] == 0 {
            continue;
        }
        let (hue, saturation, value) = to_hsv([pixel[0], pixel[1], pixel[2]]);
        if saturation < GREY {
            continue;
        }
        let x = (at % width) as f32 / width as f32;
        let y = (at / width) as f32 / height;
        let Shift { turn, scale } = shift((x + y) / 2.0);
        let [r, g, b] = from_hsv((hue + turn).rem_euclid(360.0), (saturation * scale).min(1.0), value);
        pixel[0] = r;
        pixel[1] = g;
        pixel[2] = b;
    }
}

/// Recolour RGBA pixels in place for this accent. Solid without a colour — or
/// with anything that is not `#rrggbb` — leaves them untouched: the violet.
pub fn tint_rgba(rgba: &mut [u8], width: u32, height: u32, accent: &Accent) {
    let first = accent.color.as_deref().and_then(parse);
    match accent.mode {
        AccentMode::Solid => {
            let Some(color) = first else { return };
            let shift = shift_to(color);
            recolor(rgba, width, height, |_| shift);
        }
        AccentMode::Gradient => {
            // A missing colour is the violet, like everywhere else.
            let a = shift_to(first.unwrap_or(VIOLET));
            let b = shift_to(accent.color_2.as_deref().and_then(parse).unwrap_or(VIOLET));
            recolor(rgba, width, height, |t| between(a, b, t));
        }
        AccentMode::Rgb => {
            // The whole wheel once, at full strength.
            let (violet_hue, _, _) = to_hsv(VIOLET);
            recolor(rgba, width, height, |t| Shift {
                turn: t * 360.0 - violet_hue,
                scale: 1.0,
            });
        }
    }
}

/// An icon in the accent colour, as a copy that owns its pixels.
pub fn tint(icon: &Image<'_>, accent: &Accent) -> Image<'static> {
    let mut rgba = icon.rgba().to_vec();
    tint_rgba(&mut rgba, icon.width(), icon.height(), accent);
    Image::new_owned(rgba, icon.width(), icon.height())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RgbSpeed;

    fn solid(color: Option<&str>) -> Accent {
        Accent {
            mode: AccentMode::Solid,
            color: color.map(str::to_string),
            color_2: None,
            speed: RgbSpeed::Slow,
        }
    }

    /// The violet itself must come out as the violet, or picking it back would
    /// leave the icons a shade off.
    #[test]
    fn the_violet_stays_the_violet() {
        let mut pixel = [0x8b, 0x5c, 0xf6, 255, 0x7c, 0x3a, 0xed, 255];
        let before = pixel;
        tint_rgba(&mut pixel, 2, 1, &solid(Some("#8b5cf6")));
        for (a, b) in pixel.iter().zip(before) {
            assert!(a.abs_diff(b) <= 1, "{pixel:?} drifted from {before:?}");
        }
    }

    #[test]
    fn colour_turns_grey_and_clear_stay() {
        let mut rgba = [0x8b, 0x5c, 0xf6, 255, 40, 40, 44, 255, 0x8b, 0x5c, 0xf6, 0];
        tint_rgba(&mut rgba, 3, 1, &solid(Some("#22c55e")));
        let (hue, _, _) = to_hsv([rgba[0], rgba[1], rgba[2]]);
        assert!((hue - to_hsv([0x22, 0xc5, 0x5e]).0).abs() < 2.0, "hue {hue}");
        assert_eq!(&rgba[4..8], &[40, 40, 44, 255]);
        assert_eq!(&rgba[8..12], &[0x8b, 0x5c, 0xf6, 0]);
    }

    #[test]
    fn no_accent_changes_nothing() {
        let mut rgba = [0x8b, 0x5c, 0xf6, 255];
        tint_rgba(&mut rgba, 1, 1, &solid(None));
        tint_rgba(&mut rgba, 1, 1, &solid(Some("violet")));
        assert_eq!(rgba, [0x8b, 0x5c, 0xf6, 255]);
    }

    /// Top left takes the first colour, bottom right the second.
    #[test]
    fn a_gradient_runs_corner_to_corner() {
        let violet = [0x8b, 0x5c, 0xf6, 255];
        let mut rgba = [violet, violet].concat();
        let accent = Accent {
            mode: AccentMode::Gradient,
            color: Some("#ef4444".into()),
            color_2: Some("#3b82f6".into()),
            speed: RgbSpeed::Slow,
        };
        // Two pixels in a column: the first at t = 0, the second at t = 0.25.
        tint_rgba(&mut rgba, 1, 2, &accent);
        let (top, _, _) = to_hsv([rgba[0], rgba[1], rgba[2]]);
        assert!(top < 5.0 || top > 355.0, "top {top} should be red");
        let (lower, _, _) = to_hsv([rgba[4], rgba[5], rgba[6]]);
        assert!(lower > 300.0 || lower < 5.0, "lower {lower} should be on the way to blue");
    }
}
