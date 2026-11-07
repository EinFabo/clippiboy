//! A screenshot made small enough for Discord: "Copy for Discord".
//!
//! Discord takes 10 MB without Nitro, and a 4K PNG of a busy game is often
//! more. The PNG goes as it is when it fits — nothing is lost. Otherwise a JPEG
//! at falling quality, and only when even the lowest one is too big, a smaller
//! picture: a soft picture beats a blocky one at the same size.

use crate::shot::Shot;

/// Tried in this order, each at every scale before the picture shrinks.
const QUALITIES: [u8; 4] = [92, 82, 72, 60];
/// The picture's own size first, then three quarters, then half.
const SCALES: [(u32, u32); 3] = [(1, 1), (3, 4), (1, 2)];

/// What goes to Discord.
#[derive(Debug)]
pub enum Fitted {
    /// The PNG on disk is small enough as it is.
    Png,
    /// A JPEG, with the size it came out at.
    Jpeg { bytes: Vec<u8>, width: u32, height: u32 },
}

/// Fit `shot` (whose PNG is `png_len` bytes) under `limit` bytes.
pub fn fit(shot: &Shot, png_len: u64, limit: u64) -> Result<Fitted, String> {
    if png_len <= limit {
        return Ok(Fitted::Png);
    }
    for (num, den) in SCALES {
        let scaled;
        let picture = if num == den {
            shot
        } else {
            scaled = shrink(shot, num, den);
            &scaled
        };
        for quality in QUALITIES {
            let bytes = jpeg(picture, quality)?;
            if bytes.len() as u64 <= limit {
                return Ok(Fitted::Jpeg { bytes, width: picture.width, height: picture.height });
            }
        }
    }
    Err(format!(
        "The picture does not fit under {} MB, not even at half its size.",
        limit / (1024 * 1024)
    ))
}

fn jpeg(shot: &Shot, quality: u8) -> Result<Vec<u8>, String> {
    let width = u16::try_from(shot.width).map_err(|_| "The picture is too wide for a JPEG.".to_string())?;
    let height = u16::try_from(shot.height).map_err(|_| "The picture is too tall for a JPEG.".to_string())?;
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, quality)
        .encode(&shot.rgb, width, height, jpeg_encoder::ColorType::Rgb)
        .map_err(|err| format!("could not write the JPEG: {err}"))?;
    Ok(out)
}

/// `shot` at `num / den` of its size, every new pixel the mean of the old ones
/// it covers — a box filter, which keeps text readable where skipping pixels
/// would break it up.
fn shrink(shot: &Shot, num: u32, den: u32) -> Shot {
    let width = (shot.width * num / den).max(1);
    let height = (shot.height * num / den).max(1);
    let (sw, sh) = (shot.width as usize, shot.height as usize);
    let mut rgb = vec![0u8; width as usize * height as usize * 3];
    for y in 0..height as usize {
        let y0 = y * sh / height as usize;
        let y1 = ((y + 1) * sh / height as usize).max(y0 + 1);
        for x in 0..width as usize {
            let x0 = x * sw / width as usize;
            let x1 = ((x + 1) * sw / width as usize).max(x0 + 1);
            let mut sum = [0u32; 3];
            for row in y0..y1 {
                let line = &shot.rgb[(row * sw + x0) * 3..(row * sw + x1) * 3];
                for pixel in line.chunks_exact(3) {
                    sum[0] += pixel[0] as u32;
                    sum[1] += pixel[1] as u32;
                    sum[2] += pixel[2] as u32;
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u32;
            let at = (y * width as usize + x) * 3;
            for channel in 0..3 {
                rgb[at + channel] = ((sum[channel] + count / 2) / count) as u8;
            }
        }
    }
    Shot { width, height, rgb }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: u64 = 10 * 1024 * 1024;

    /// Pure noise: the worst case for every encoder, far past any real game.
    fn noise(width: u32, height: u32) -> Shot {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let rgb = (0..width as usize * height as usize * 3)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 56) as u8
            })
            .collect();
        Shot { width, height, rgb }
    }

    #[test]
    fn a_png_that_fits_goes_as_it_is() {
        let shot = noise(64, 64);
        assert!(matches!(fit(&shot, LIMIT, LIMIT).unwrap(), Fitted::Png));
    }

    #[test]
    fn a_noisy_4k_picture_comes_in_under_ten_megabytes() {
        let shot = noise(3840, 2160);
        match fit(&shot, 30 * 1024 * 1024, LIMIT).unwrap() {
            Fitted::Jpeg { bytes, width, height } => {
                assert!(bytes.len() as u64 <= LIMIT, "{} bytes", bytes.len());
                assert_eq!(&bytes[..2], &[0xff, 0xd8], "a JPEG");
                assert!(width >= 1920 && height >= 1080, "{width}x{height}");
            }
            Fitted::Png => panic!("a 30 MB PNG does not fit"),
        }
    }

    /// Slow without optimisation: `cargo test --release --lib -- --ignored a_noisy_8k`.
    #[test]
    #[ignore]
    fn a_noisy_8k_picture_comes_in_under_ten_megabytes() {
        let shot = noise(7680, 4320);
        match fit(&shot, 100 * 1024 * 1024, LIMIT).unwrap() {
            Fitted::Jpeg { bytes, .. } => assert!(bytes.len() as u64 <= LIMIT),
            Fitted::Png => panic!(),
        }
    }

    #[test]
    fn a_limit_nothing_reaches_says_so() {
        let shot = noise(256, 256);
        assert!(fit(&shot, u64::MAX, 100).is_err());
    }

    #[test]
    fn shrinking_averages_the_pixels_it_covers() {
        let shot = Shot { width: 2, height: 2, rgb: vec![0, 0, 0, 200, 200, 200, 100, 100, 100, 100, 100, 100] };
        let half = shrink(&shot, 1, 2);
        assert_eq!((half.width, half.height), (1, 1));
        assert_eq!(half.rgb, vec![100, 100, 100]);
        let three_quarters = shrink(&noise(1920, 1080), 3, 4);
        assert_eq!((three_quarters.width, three_quarters.height), (1440, 810));
    }
}
