//! A single picture instead of a recording.
//!
//! The buffer's path does not fit here: it wants a running encoder, a ring and a
//! muxer, and what it hands out is H.264. A screenshot is one frame, and it has
//! to work when nothing is running at all — hence a capture session of its own
//! that lives exactly as long as it takes one frame to arrive.
//!
//! This is the only place in ClippiBoy where a picture travels from the GPU into
//! main memory. Everywhere else the frames stay where they are: the encoder is
//! fed NV12 textures straight from the GPU and never sees a byte of them.

use std::path::Path;
use std::time::Duration;

use crate::model::TargetKind;

/// A picture in main memory: RGB, tightly packed, ready for the PNG encoder.
///
/// No alpha channel. The desktop is opaque, and a screenshot with a transparent
/// sky helps nobody — leaving it out saves a quarter of the bytes.
pub struct Shot {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// How long to wait for the first frame.
///
/// Windows.Graphics.Capture delivers when something is redrawn. A game delivers
/// with its next frame; a desktop that is standing still can take a moment.
const TIMEOUT: Duration = Duration::from_secs(3);

impl Shot {
    /// Cut a rectangle out of the picture.
    ///
    /// The rectangle is clamped to what is really there rather than rejected: it
    /// comes from a mouse dragged across a scaled preview, and a rounding error
    /// at the edge is not a reason to refuse the crop.
    pub fn crop(&self, x: u32, y: u32, width: u32, height: u32) -> Result<Shot, String> {
        let left = x.min(self.width.saturating_sub(1)) as usize;
        let top = y.min(self.height.saturating_sub(1)) as usize;
        let cut_width = (width as usize).min(self.width as usize - left);
        let cut_height = (height as usize).min(self.height as usize - top);
        if cut_width == 0 || cut_height == 0 {
            return Err("The selection is empty.".into());
        }

        let stride = self.width as usize * 3;
        let mut rgb = Vec::with_capacity(cut_width * cut_height * 3);
        for row in top..top + cut_height {
            let from = row * stride + left * 3;
            rgb.extend_from_slice(&self.rgb[from..from + cut_width * 3]);
        }
        Ok(Shot {
            width: cut_width as u32,
            height: cut_height as u32,
            rgb,
        })
    }

    /// Lay a transparent layer over the picture.
    ///
    /// The annotations are drawn in the WebView — arrows, boxes, and above all
    /// text, which would mean a font renderer in here — and arrive as a PNG with
    /// alpha. Only the layer travels, not the finished picture: it is empty
    /// almost everywhere, so it deflates to a few kilobytes, while the picture
    /// underneath is megabytes.
    pub fn blend(&mut self, layer: &[u8]) -> Result<(), String> {
        let top = read_rgba(layer)?;
        if top.0 != self.width || top.1 != self.height {
            return Err("The layer does not fit the picture.".into());
        }
        for (under, over) in self.rgb.chunks_exact_mut(3).zip(top.2.chunks_exact(4)) {
            let alpha = over[3] as u32;
            if alpha == 0 {
                continue;
            }
            for channel in 0..3 {
                under[channel] = ((over[channel] as u32 * alpha
                    + under[channel] as u32 * (255 - alpha))
                    / 255) as u8;
            }
        }
        Ok(())
    }

    /// Blur an area until nothing can be read in it any more.
    ///
    /// One pass of a box blur across, one down. That is separable, so the cost
    /// does not grow with the square of the radius — and with the sliding sum in
    /// [`blur_rows`] it does not grow with the radius at all.
    ///
    /// The copy reaches `radius` beyond the rectangle on every side. Without
    /// that margin only pixels from inside the rectangle are averaged, and a
    /// redaction barely wider than the word it covers keeps the shape of that
    /// word — while the preview, which blurs the whole picture and clips
    /// afterwards, shows it mixed into its surroundings. The margin is what
    /// makes the two agree.
    ///
    /// `ellipse` blurs the same rectangle but only writes back what lies inside
    /// the oval — a face wants a round patch, and a rectangle around it points
    /// at what it is hiding.
    pub fn blur(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        radius: u32,
        ellipse: bool,
    ) {
        let left = x.min(self.width.saturating_sub(1)) as usize;
        let top = y.min(self.height.saturating_sub(1)) as usize;
        let w = (width as usize).min(self.width as usize - left);
        let h = (height as usize).min(self.height as usize - top);
        let radius = radius.max(1) as usize;
        if w == 0 || h == 0 {
            return;
        }

        let stride = self.width as usize * 3;

        // The area actually read: the rectangle plus a margin of `radius`,
        // as far as the picture reaches.
        let from_x = left.saturating_sub(radius);
        let from_y = top.saturating_sub(radius);
        let to_x = (left + w + radius).min(self.width as usize);
        let to_y = (top + h + radius).min(self.height as usize);
        let (pw, ph) = (to_x - from_x, to_y - from_y);
        // Where the rectangle sits inside that copy.
        let (inset_x, inset_y) = (left - from_x, top - from_y);

        // A copy of the region, so a blurred pixel is never read as input for
        // the next one — that would smear the picture in one direction.
        let mut patch = vec![0u8; pw * ph * 3];
        for row in 0..ph {
            let from = (from_y + row) * stride + from_x * 3;
            patch[row * pw * 3..(row + 1) * pw * 3]
                .copy_from_slice(&self.rgb[from..from + pw * 3]);
        }

        let mut across = vec![0u8; patch.len()];
        blur_rows(&patch, &mut across, pw, ph, radius);
        // The same routine down the columns: transpose, run, transpose back.
        let mut turned = vec![0u8; patch.len()];
        transpose(&across, &mut turned, pw, ph);
        let mut done = vec![0u8; patch.len()];
        blur_rows(&turned, &mut done, ph, pw, radius);
        transpose(&done, &mut across, ph, pw);

        // From the rectangle that was asked for, not from what is left of it
        // after clamping. A blur pulled over the edge of the picture would
        // otherwise become a squashed oval here while the preview, which knows
        // nothing of the clamping, showed a round one.
        let (centre_x, centre_y) = (width as f32 / 2.0, height as f32 / 2.0);
        for row in 0..h {
            let line = (top + row) * stride + left * 3;
            let read = ((inset_y + row) * pw + inset_x) * 3;
            if !ellipse {
                self.rgb[line..line + w * 3].copy_from_slice(&across[read..read + w * 3]);
                continue;
            }
            for column in 0..w {
                let dx = (column as f32 + 0.5 - centre_x) / centre_x;
                let dy = (row as f32 + 0.5 - centre_y) / centre_y;
                if dx * dx + dy * dy > 1.0 {
                    continue;
                }
                let to = line + column * 3;
                let from = read + column * 3;
                self.rgb[to..to + 3].copy_from_slice(&across[from..from + 3]);
            }
        }
    }

    /// The picture as a device-independent bitmap, ready for the clipboard.
    ///
    /// Three shapes have to be turned around at once, and all three are
    /// historical quirks of the format: the rows run bottom to top, the channels
    /// are BGR rather than RGB, and every row is padded to a multiple of four
    /// bytes. 24 bit rather than 32 on purpose — a fourth channel a screenshot
    /// does not have is exactly where old programs paste a black picture.
    pub fn dib(&self) -> Vec<u8> {
        const HEADER: usize = 40;
        let width = self.width as usize;
        let height = self.height as usize;
        let stride = (width * 3 + 3) & !3;

        let mut out = vec![0u8; HEADER + stride * height];
        let put = |out: &mut Vec<u8>, at: usize, value: u32| {
            out[at..at + 4].copy_from_slice(&value.to_le_bytes());
        };
        put(&mut out, 0, HEADER as u32);
        put(&mut out, 4, self.width);
        put(&mut out, 8, self.height);
        out[12..14].copy_from_slice(&1u16.to_le_bytes()); // planes
        out[14..16].copy_from_slice(&24u16.to_le_bytes()); // bits per pixel
        put(&mut out, 16, 0); // BI_RGB, uncompressed
        put(&mut out, 20, (stride * height) as u32);

        for y in 0..height {
            let from = &self.rgb[y * width * 3..(y + 1) * width * 3];
            let row = HEADER + (height - 1 - y) * stride;
            for (x, pixel) in from.chunks_exact(3).enumerate() {
                let at = row + x * 3;
                out[at] = pixel[2];
                out[at + 1] = pixel[1];
                out[at + 2] = pixel[0];
            }
        }
        out
    }

    /// Write the picture as a PNG.
    pub fn write_png(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                format!("could not create folder '{}': {err}", parent.display())
            })?;
        }
        let file = std::fs::File::create(path)
            .map_err(|err| format!("could not create '{}': {err}", path.display()))?;

        let mut encoder =
            png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        // A 4K picture is eight million pixels. Squeezing the last few percent
        // out of them would cost about a second — far too long between the key
        // press and the banner, for a file that is a fifth smaller.
        encoder.set_compression(png::Compression::Fast);

        let mut writer = encoder
            .write_header()
            .map_err(|err| format!("could not write the picture: {err}"))?;
        writer
            .write_image_data(&self.rgb)
            .map_err(|err| format!("could not write the picture: {err}"))?;
        writer
            .finish()
            .map_err(|err| format!("could not finish the picture: {err}"))?;
        Ok(())
    }
}

/// The untouched picture of a screenshot that has been cropped.
///
/// Deliberately in the same folder `edit.rs` keeps a clip's original recording
/// in: `delete_clip` clears that folder out wholesale, so a deleted screenshot
/// leaves nothing behind without a line of code of its own.
pub fn original_path(clip_id: &str) -> std::path::PathBuf {
    crate::edit::dir(clip_id).join("image.png")
}

/// Is there still an untouched picture beside this one?
pub fn has_original(clip_id: &str) -> bool {
    original_path(clip_id).is_file()
}

/// The picture as it is drawn **on**: the original with the crop applied, but
/// without the marks.
///
/// Without it the marks would be seen twice once they are saved — once baked
/// into the file and once as the layer over it — and there would be no way to
/// pick one up again.
pub fn base_path(clip_id: &str) -> std::path::PathBuf {
    crate::edit::dir(clip_id).join("base.png")
}

/// What has been done to a screenshot so far.
///
/// The point of keeping it: crop and marks can then be taken back one without
/// the other. A flattened picture cannot do that — once a mark is in the pixels
/// there is no peeling it off again.
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edit {
    #[serde(default)]
    pub crop: Option<Rect>,
    /// The marks exactly as the WebView keeps them. Opaque in here — it is
    /// handed back unread, so they stay editable.
    #[serde(default)]
    pub marks: String,
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

fn edit_path(clip_id: &str) -> std::path::PathBuf {
    crate::edit::dir(clip_id).join("shot.json")
}

pub fn read_edit(clip_id: &str) -> Edit {
    std::fs::read_to_string(edit_path(clip_id))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn write_edit(clip_id: &str, edit: &Edit) -> Result<(), String> {
    let path = edit_path(clip_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("could not create folder: {err}"))?;
    }
    let text = serde_json::to_string(edit).map_err(|err| err.to_string())?;
    std::fs::write(&path, text).map_err(|err| format!("could not note the edit: {err}"))
}

/// Clear the whole store away — the picture is its untouched self again.
pub fn forget(clip_id: &str) {
    let _ = std::fs::remove_file(edit_path(clip_id));
    let _ = std::fs::remove_file(base_path(clip_id));
    let _ = std::fs::remove_file(original_path(clip_id));
    let _ = std::fs::remove_dir(crate::edit::dir(clip_id));
}

/// Edges of a PNG, without decoding the pixels.
pub fn size_of_png(path: &Path) -> Result<(u32, u32), String> {
    let file = std::fs::File::open(path)
        .map_err(|err| format!("could not open '{}': {err}", path.display()))?;
    let reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .map_err(|err| format!("could not read the picture: {err}"))?;
    let info = reader.info();
    Ok((info.width, info.height))
}

/// Read a picture back off the disk.
///
/// Used for the clipboard: what goes there is what is really in the file, not
/// what was captured — the two part company as soon as the picture is edited.
pub fn read_png(path: &Path) -> Result<Shot, String> {
    let file = std::fs::File::open(path)
        .map_err(|err| format!("could not open '{}': {err}", path.display()))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    // Palettes and 16-bit channels turn into plain 8-bit ones, so only a
    // handful of shapes are left below.
    decoder.set_transformations(
        png::Transformations::EXPAND | png::Transformations::STRIP_16,
    );
    let mut reader = decoder
        .read_info()
        .map_err(|err| format!("could not read the picture: {err}"))?;

    let mut buffer = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|err| format!("could not read the picture: {err}"))?;
    buffer.truncate(info.buffer_size());

    let pixels = (info.width as usize) * (info.height as usize);
    let rgb = match info.color_type {
        png::ColorType::Rgb => buffer,
        png::ColorType::Rgba => {
            let mut out = Vec::with_capacity(pixels * 3);
            for pixel in buffer.chunks_exact(4) {
                out.extend_from_slice(&pixel[..3]);
            }
            out
        }
        png::ColorType::Grayscale => {
            let mut out = Vec::with_capacity(pixels * 3);
            for grey in &buffer {
                out.extend_from_slice(&[*grey, *grey, *grey]);
            }
            out
        }
        png::ColorType::GrayscaleAlpha => {
            let mut out = Vec::with_capacity(pixels * 3);
            for pixel in buffer.chunks_exact(2) {
                out.extend_from_slice(&[pixel[0], pixel[0], pixel[0]]);
            }
            out
        }
        other => return Err(format!("unexpected picture format: {other:?}")),
    };

    Ok(Shot {
        width: info.width,
        height: info.height,
        rgb,
    })
}

/// Grab one frame from the given source.
///
/// Runs whether or not the replay buffer is going. Two capture sessions on the
/// same monitor are allowed — WGC keeps them apart.
#[cfg(windows)]
pub fn grab(kind: TargetKind, id: Option<&str>) -> Result<Shot, String> {
    use std::sync::Arc;

    let gpu = Arc::new(crate::gpu::GpuDevice::new()?);
    // Room for exactly one: whichever frame arrives first is the screenshot,
    // every one after it finds the channel full and is dropped.
    let (sender, receiver) = crossbeam_channel::bounded::<Result<Shot, String>>(1);

    let capture = crate::wgc::start(
        &gpu,
        kind,
        id,
        // No frame rate limit — we want the next frame, not a paced one.
        0,
        {
            let gpu = gpu.clone();
            let sender = sender.clone();
            move |frame| {
                let _ = sender.try_send(read_back(&gpu, frame.texture));
            }
        },
        {
            let sender = sender.clone();
            move || {
                let _ = sender.try_send(Err("The source is no longer there.".to_string()));
            }
        },
    )?;

    let shot = receiver.recv_timeout(TIMEOUT).map_err(|_| {
        "No picture arrived. Is anything being drawn on the recording source?".to_string()
    });
    // Explicitly, and before the picture is handed on: the session holds a frame
    // pool on the GPU, and there is no reason to keep it a moment longer.
    drop(capture);
    shot?
}

#[cfg(not(windows))]
pub fn grab(_kind: TargetKind, _id: Option<&str>) -> Result<Shot, String> {
    Err("Screenshots only work on Windows.".into())
}

/// Copy a GPU texture into main memory.
///
/// The way there is a staging texture: the captured one lives in video memory
/// with no CPU access, so it is copied into one that has it and read from there.
#[cfg(windows)]
fn read_back(
    gpu: &crate::gpu::GpuDevice,
    texture: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
) -> Result<Shot, String> {
    use windows::Win32::Graphics::Direct3D11::{
        ID3D11Texture2D, D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
        D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    };

    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);

        let staging_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            // A staging texture is bound to nothing; it only gets read.
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
            ..desc
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        gpu.device
            .CreateTexture2D(&staging_desc, None, Some(&mut staging))
            .map_err(|err| format!("could not prepare the picture: {err}"))?;
        let staging = staging.ok_or_else(|| "could not prepare the picture".to_string())?;

        gpu.context.CopyResource(&staging, texture);

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        gpu.context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|err| format!("could not read the picture: {err}"))?;

        let width = desc.Width as usize;
        let height = desc.Height as usize;
        let pitch = mapped.RowPitch as usize;
        let mut rgb = vec![0u8; width * height * 3];

        for y in 0..height {
            // `RowPitch` is not `width * 4`: the driver pads every row to its own
            // alignment. Copying the block in one go would shear the picture.
            let row = std::slice::from_raw_parts(
                (mapped.pData as *const u8).add(y * pitch),
                width * 4,
            );
            let out = &mut rgb[y * width * 3..(y + 1) * width * 3];
            for (from, to) in row.chunks_exact(4).zip(out.chunks_exact_mut(3)) {
                // WGC delivers BGRA, PNG wants RGB.
                to[0] = from[2];
                to[1] = from[1];
                to[2] = from[0];
            }
        }

        gpu.context.Unmap(&staging, 0);

        Ok(Shot {
            width: desc.Width,
            height: desc.Height,
            rgb,
        })
    }
}

/// One box blur pass along the rows.
///
/// The window is carried from one pixel to the next — what enters on the right
/// is added, what leaves on the left is subtracted — so a pass costs the same
/// whatever the radius. Summing the window afresh per pixel made a wide blur on
/// a 4K still take seconds, and the radius may go up to 96.
fn blur_rows(from: &[u8], to: &mut [u8], width: usize, height: usize, radius: usize) {
    if width == 0 {
        return;
    }
    for row in 0..height {
        let line = row * width * 3;
        for channel in 0..3 {
            let (mut first, mut last) = (0usize, radius.min(width - 1));
            let mut sum: u32 = (first..=last)
                .map(|at| from[line + at * 3 + channel] as u32)
                .sum();
            for column in 0..width {
                to[line + column * 3 + channel] = (sum / (last - first + 1) as u32) as u8;
                let next = column + 1;
                if next == width {
                    break;
                }
                if next > radius {
                    sum -= from[line + first * 3 + channel] as u32;
                    first += 1;
                }
                if last + 1 < width && next + radius > last {
                    last += 1;
                    sum += from[line + last * 3 + channel] as u32;
                }
            }
        }
    }
}

/// Turn a picture on its side, so the same row routine blurs the columns.
fn transpose(from: &[u8], to: &mut [u8], width: usize, height: usize) {
    for row in 0..height {
        for column in 0..width {
            let source = (row * width + column) * 3;
            let target = (column * height + row) * 3;
            to[target..target + 3].copy_from_slice(&from[source..source + 3]);
        }
    }
}

/// Decode a PNG keeping its alpha — the counterpart to `read_png`, which throws
/// it away because a screenshot has none to keep.
fn read_rgba(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(
        png::Transformations::EXPAND | png::Transformations::STRIP_16,
    );
    let mut reader = decoder
        .read_info()
        .map_err(|err| format!("could not read the layer: {err}"))?;

    let mut buffer = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|err| format!("could not read the layer: {err}"))?;
    buffer.truncate(info.buffer_size());

    let pixels = (info.width as usize) * (info.height as usize);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buffer,
        // A layer with no transparency at all covers everything — unusual, but
        // not a reason to give up.
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity(pixels * 4);
            for pixel in buffer.chunks_exact(3) {
                out.extend_from_slice(pixel);
                out.push(255);
            }
            out
        }
        other => return Err(format!("unexpected layer format: {other:?}")),
    };
    Ok((info.width, info.height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How `blur_rows` worked before the window was carried along: every pixel
    /// summed its whole window afresh. Correct, just slow — so it is the yardstick.
    fn naive_rows(from: &[u8], to: &mut [u8], width: usize, height: usize, radius: usize) {
        for row in 0..height {
            let line = row * width * 3;
            for column in 0..width {
                let first = column.saturating_sub(radius);
                let last = (column + radius).min(width - 1);
                let count = (last - first + 1) as u32;
                for channel in 0..3 {
                    let mut sum = 0u32;
                    for at in first..=last {
                        sum += from[line + at * 3 + channel] as u32;
                    }
                    to[line + column * 3 + channel] = (sum / count) as u8;
                }
            }
        }
    }

    fn noise(len: usize) -> Vec<u8> {
        // A fixed pattern beats a random one: a failure is reproducible.
        (0..len).map(|i| ((i * 37 + i / 5 * 11) % 251) as u8).collect()
    }

    #[test]
    fn the_carried_window_sums_what_the_slow_loop_summed() {
        for &(width, height) in &[(1, 1), (2, 3), (7, 4), (33, 5), (64, 2)] {
            for radius in [1, 2, 3, 8, 40] {
                let from = noise(width * height * 3);
                let mut fast = vec![0u8; from.len()];
                let mut slow = vec![0u8; from.len()];
                blur_rows(&from, &mut fast, width, height, radius);
                naive_rows(&from, &mut slow, width, height, radius);
                assert_eq!(fast, slow, "{width}x{height}, radius {radius}");
            }
        }
    }

    fn split_picture() -> Shot {
        // 100 x 20, black on the left half, white on the right.
        let (width, height) = (100usize, 20usize);
        let mut rgb = vec![0u8; width * height * 3];
        for row in 0..height {
            for column in 50..width {
                let at = (row * width + column) * 3;
                rgb[at..at + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        Shot { width: width as u32, height: height as u32, rgb }
    }

    #[test]
    fn the_blur_reaches_past_its_own_edge_for_input() {
        let mut shot = split_picture();
        // A white rectangle whose left edge sits on the border to the black half.
        shot.blur(50, 0, 20, 20, 10, false);
        let at = (10 * 100 + 50) * 3;
        assert!(
            shot.rgb[at] < 200,
            "the first white column stayed at {} — nothing from the black half \
             was averaged in",
            shot.rgb[at],
        );
    }

    #[test]
    fn nothing_outside_the_rectangle_is_written() {
        let before = split_picture();
        let mut after = split_picture();
        after.blur(50, 0, 20, 20, 10, false);
        for row in 0..20usize {
            for column in 0..100usize {
                if (50..70).contains(&column) {
                    continue;
                }
                let at = (row * 100 + column) * 3;
                assert_eq!(
                    before.rgb[at..at + 3],
                    after.rgb[at..at + 3],
                    "pixel {column},{row} was touched although it lies outside the rectangle",
                );
            }
        }
    }

    #[test]
    fn a_rectangle_over_the_edge_stays_inside_the_picture() {
        let mut shot = split_picture();
        // Wider and taller than what is left of the picture from that corner.
        shot.blur(90, 15, 40, 40, 6, false);
        assert_eq!(shot.rgb.len(), 100 * 20 * 3);
    }
}
