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
