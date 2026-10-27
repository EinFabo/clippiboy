//! BGRA → NV12 on the GPU, and the clock that turns it into a constant frame
//! rate stream.
//!
//! The two belong together because both solve the same problem:
//! Windows.Graphics.Capture delivers frames **only on change** and at most at
//! screen refresh rate. Previously every one of those frames went unfiltered to
//! an encoder that was at the same time told a constant 60 fps — it had to
//! convert the frame rate itself, and dropped and duplicated unevenly doing so.
//! That was the judder.
//!
//! Here a thread of its own ticks at `1/fps` instead. If a new frame is there it
//! goes out; if none is (a still picture), the last one goes out again. The
//! encoder therefore sees exactly `fps` frames per second at even intervals.
//! That is precisely what Medal and ShadowPlay do.

#![cfg(windows)]

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use windows::core::Interface;
use windows::Win32::Foundation::TRUE;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoDevice,
    ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView,
    ID3D11VideoProcessorOutputView, D3D11_BIND_RENDER_TARGET, D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
    D3D11_VIDEO_PROCESSOR_COLOR_SPACE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_PLAYBACK_NORMAL, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_RATIONAL};

use crate::gpu::{GpuDevice, SendPtr};

use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT, D2D_RECT_F,
    D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap1, ID2D1DeviceContext, ID2D1Factory1,
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET,
    D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_MULTI_THREADED,
    D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11DeviceContext, ID3D11Multithread, D3D11_BIND_SHADER_RESOURCE,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISurface};

/// How many NV12 textures are used in rotation.
///
/// The encoder reads the texture where it lies — `mft::Transform::feed` wraps it
/// with `MFCreateDXGISurfaceBuffer` and copies nothing — so a slot may not be
/// written again until every sample referring to it has been let go.
///
/// This used to be a flat 8, on the reasoning that eight frames are 130 ms at
/// 60 fps and "considerably more than a hardware encoder ever holds on to". That
/// was wrong, and provably so out of this codebase's own measurements:
/// [`mft::ENCODER_HOLD_MAX`] records sixteen frames held at once on an RX 7600
/// XT, and [`mft::QUEUE_DEPTH`] frames wait in the job channel on top. Twenty
/// references over eight textures means the rotation laps the encoder and
/// overwrites frames it is still reading — torn or wrong pictures in the clip,
/// with every counter in the app reporting a clean run.
///
/// So the figure is derived rather than guessed. `low_latency_default` keeps the
/// real queue near zero on AMD and Intel today, and NVENC's is shallow, so the
/// headroom mostly goes unused — but it is bought for what was actually measured,
/// not for what the queue is assumed to be. At 1080p an NV12 slot is 3.1 MB.
///
/// `depth` is the encoder's real queue (`CLIPPIBOY_QUEUE_DEPTH` may move it), so
/// the slots grow with it instead of the guard starving every frame.
pub fn slots_for(depth: usize) -> usize {
    depth + crate::mft::ENCODER_HOLD_MAX + 2
}

/// The rotation only holds if there are more textures than there can be frames
/// referring to them. Stated here so that lowering either figure it is built from
/// has to be a deliberate act rather than a quiet one.
const _: () = assert!(
    crate::mft::QUEUE_DEPTH + crate::mft::ENCODER_HOLD_MAX + 2
        > crate::mft::QUEUE_DEPTH + crate::mft::ENCODER_HOLD_MAX
);

/// Colour space bitfield from `d3d11.h`:
/// bit 0 `Usage`, bit 1 `RGB_Range`, bit 2 `YCbCr_Matrix`, bit 3 `YCbCr_Xvycc`,
/// bits 4–5 `Nominal_Range`.
const fn color_space(ycbcr_709: bool, nominal_range: u32) -> D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
    let bits = ((ycbcr_709 as u32) << 2) | ((nominal_range & 0x3) << 4);
    D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: bits }
}

/// `D3D11_VIDEO_PROCESSOR_NOMINAL_RANGE_16_235` — the range H.264 and every
/// player assume by default.
const RANGE_STUDIO: u32 = 1;
/// `D3D11_VIDEO_PROCESSOR_NOMINAL_RANGE_0_255` — that is how the desktop arrives.
const RANGE_FULL: u32 = 2;

struct Slot {
    texture: ID3D11Texture2D,
    output_view: ID3D11VideoProcessorOutputView,
}

/// Converts the capture's BGRA frames to NV12, scaling to the target resolution
/// along the way.
pub struct Converter {
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    slots: Vec<Slot>,
    next_slot: usize,
    /// The slot written last, for [`Self::convert`]'s `reuse`.
    last_slot: Option<usize>,
    /// Input views keyed by the source texture's raw pointer. The frame pool has
    /// only two textures, so the table stays tiny — and calling
    /// `CreateVideoProcessorInputView` afresh for every frame would be waste.
    input_views: Vec<(isize, ID3D11VideoProcessorInputView)>,
    /// The size the cached views were made for. `wgc` recreates its frame pool
    /// whenever the source changes size, and the old textures are then dead —
    /// each one still pinned by the view that refers to it. Keyed on a raw
    /// pointer, the table has no way of noticing that by itself.
    source_size: (u32, u32),
    /// Draws the watermark. `None` once it failed — the frames then go out
    /// without it rather than not at all.
    stamp: Option<Stamp>,
    width: u32,
    height: u32,
}

impl Converter {
    pub fn new(gpu: &GpuDevice, width: u32, height: u32, fps: u32,
        slots: usize,
    ) -> Result<Self, String> {
        let video_device: ID3D11VideoDevice = gpu
            .device
            .cast()
            .map_err(|err| format!("ID3D11VideoDevice: {err}"))?;
        let video_context: ID3D11VideoContext = gpu
            .context
            .cast()
            .map_err(|err| format!("ID3D11VideoContext: {err}"))?;

        let rate = DXGI_RATIONAL {
            Numerator: fps.max(1),
            Denominator: 1,
        };
        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: width,
            InputHeight: height,
            OutputFrameRate: rate,
            OutputWidth: width,
            OutputHeight: height,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };

        let enumerator = unsafe { video_device.CreateVideoProcessorEnumerator(&desc) }
            .map_err(|err| format!("VideoProcessor-Enumerator: {err}"))?;
        let processor = unsafe { video_device.CreateVideoProcessor(&enumerator, 0) }
            .map_err(|err| format!("VideoProcessor: {err}"))?;

        // Set the colour space explicitly. Without it the driver guesses, and
        // depending on the GPU washed-out or crushed colours come out — a good
        // part of the "looks worse than Medal".
        unsafe {
            video_context.VideoProcessorSetStreamColorSpace(
                &processor,
                0,
                &color_space(true, RANGE_FULL),
            );
            video_context
                .VideoProcessorSetOutputColorSpace(&processor, &color_space(true, RANGE_STUDIO));
            // Off: by default the driver may apply edge enhancement, noise
            // reduction or skin tone inside the Blt, and clips would differ per machine.
            video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
        }

        let mut slot_list = Vec::with_capacity(slots);
        for _ in 0..slots {
            slot_list.push(Self::make_slot(
                &gpu.device,
                &video_device,
                &enumerator,
                width,
                height,
            )?);
        }

        let stamp = match Stamp::new(gpu) {
            Ok(stamp) => Some(stamp),
            Err(err) => {
                log::warn!("watermark unavailable, clips go without it: {err}");
                None
            }
        };

        Ok(Self {
            video_device,
            video_context,
            enumerator,
            processor,
            slots: slot_list,
            next_slot: 0,
            last_slot: None,
            input_views: Vec::new(),
            source_size: (0, 0),
            stamp,
            width,
            height,
        })
    }

    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    fn make_slot(
        device: &ID3D11Device,
        video_device: &ID3D11VideoDevice,
        enumerator: &ID3D11VideoProcessorEnumerator,
        width: u32,
        height: u32,
    ) -> Result<Slot, String> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture = None;
        unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }
            .map_err(|err| format!("NV12-Textur: {err}"))?;
        let texture = texture.ok_or_else(|| "NV12-Textur fehlt".to_string())?;

        let view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut output_view = None;
        unsafe {
            video_device.CreateVideoProcessorOutputView(
                &texture,
                enumerator,
                &view_desc,
                Some(&mut output_view),
            )
        }
        .map_err(|err| format!("VideoProcessor-Ausgabeansicht: {err}"))?;
        let output_view = output_view.ok_or_else(|| "Ausgabeansicht fehlt".to_string())?;

        Ok(Slot {
            texture,
            output_view,
        })
    }

    fn input_view(
        &mut self,
        source: &ID3D11Texture2D,
    ) -> Result<ID3D11VideoProcessorInputView, String> {
        let key = source.as_raw() as isize;
        if let Some((_, view)) = self.input_views.iter().find(|(k, _)| *k == key) {
            return Ok(view.clone());
        }

        let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: 0,
                },
            },
        };
        let mut view = None;
        unsafe {
            self.video_device.CreateVideoProcessorInputView(
                source,
                &self.enumerator,
                &desc,
                Some(&mut view),
            )
        }
        .map_err(|err| format!("VideoProcessor-Eingabeansicht: {err}"))?;
        let view = view.ok_or_else(|| "Eingabeansicht fehlt".to_string())?;

        // If the frame pool grows again after a `Recreate`, views onto long-freed
        // textures would otherwise pile up.
        if self.input_views.len() >= 4 {
            self.input_views.clear();
        }
        self.input_views.push((key, view.clone()));
        Ok(view)
    }

    /// Convert one captured frame. Returns the slot that was used.
    ///
    /// Runs **synchronously in the capture callback**, while the source texture is
    /// still valid — that is the difference from the old route, which only passed
    /// the pointer along and read from it later.
    ///
    /// `reuse`: write into the slot written last instead of the next one —
    /// see [`Latest::submit`].
    pub fn convert(
        &mut self,
        source: &ID3D11Texture2D,
        source_size: (u32, u32),
        reuse: bool,
    ) -> Result<usize, String> {
        // A new size means `wgc` has recreated the frame pool and every texture
        // the table refers to is gone. The views keep those textures alive, which
        // is what stops a fresh one from ever landing on a cached address — but
        // it also means a window resize leaves the old pool pinned in memory
        // until four unrelated pointers have wandered past. Cheaper and clearer
        // to let go at the moment it happens.
        if self.source_size != source_size {
            self.source_size = source_size;
            self.input_views.clear();
        }

        let stamped = match self.stamp.as_mut() {
            Some(stamp) if crate::watermark::enabled() => match stamp.draw(source) {
                Ok(canvas) => canvas,
                Err(err) => {
                    log::warn!("watermark failed, going on without it: {err}");
                    self.stamp = None;
                    None
                }
            },
            Some(stamp) => {
                // Switched off: the canvas is a whole frame of memory doing
                // nothing. Gone until the switch comes back on — its cached
                // input view holds the texture too, so the cache goes with it.
                if stamp.release() {
                    self.input_views.clear();
                }
                None
            }
            None => None,
        };
        let input = self.input_view(stamped.as_ref().unwrap_or(source))?;
        let slot = match self.last_slot.filter(|_| reuse) {
            Some(slot) => slot,
            None => {
                let slot = self.next_slot;
                self.next_slot = (self.next_slot + 1) % self.slots.len();
                slot
            }
        };
        self.last_slot = Some(slot);

        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: TRUE,
            OutputIndex: 0,
            InputFrameOrField: 0,
            PastFrames: 0,
            FutureFrames: 0,
            ppPastSurfaces: std::ptr::null_mut(),
            pInputSurface: std::mem::ManuallyDrop::new(Some(input)),
            ppFutureSurfaces: std::ptr::null_mut(),
            ppPastSurfacesRight: std::ptr::null_mut(),
            pInputSurfaceRight: std::mem::ManuallyDrop::new(None),
            ppFutureSurfacesRight: std::ptr::null_mut(),
        };

        let streams = [stream];
        let result = unsafe {
            self.video_context.VideoProcessorBlt(
                &self.processor,
                &self.slots[slot].output_view,
                0,
                &streams,
            )
        };
        // `ManuallyDrop` keeps the view's reference from being released with
        // the struct — so every frame added one to a cached view, and the
        // views (with their textures and device) could never be freed. Handed
        // back here, on the error path too.
        let [stream] = streams;
        drop(std::mem::ManuallyDrop::into_inner(stream.pInputSurface));
        result.map_err(|err| format!("VideoProcessorBlt: {err}"))?;
        Ok(slot)
    }

    pub fn texture(&self, slot: usize) -> &ID3D11Texture2D {
        &self.slots[slot].texture
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// Lays the watermark over a copy of the captured frame.
///
/// Not over the frame itself: that texture belongs to the capture's frame pool
/// and goes back to it, and whatever is drawn on it would show up in the next
/// frame that happens to land in the same texture. So the frame is copied into
/// a canvas of our own first — one full-frame copy per frame, a fraction of a
/// millisecond of memory bandwidth on any card that can encode.
///
/// Direct2D rather than a shader of our own: it brings alpha blending and a
/// good downscale of the picture, and needs no shader compiler. The device was
/// created with BGRA support, which is all it asks for.
struct Stamp {
    context: ID2D1DeviceContext,
    d3d: ID3D11DeviceContext,
    device: ID3D11Device,
    /// Holds the device's lock across Direct2D's several calls, so the encoder
    /// thread cannot change the pipeline state in between.
    lock: ID3D11Multithread,
    logo: ID2D1Bitmap1,
    logo_size: (u32, u32),
    canvas: Option<Canvas>,
}

struct Canvas {
    texture: ID3D11Texture2D,
    /// Set as the context's target once and never read again — held so it
    /// lives exactly as long as the texture it draws into.
    _target: ID2D1Bitmap1,
    /// Width, height and format of the frames it was made for.
    shape: (u32, u32, i32),
}

impl Stamp {
    fn new(gpu: &GpuDevice) -> Result<Self, String> {
        let (width, height, _) = crate::watermark::picture();
        if *width == 0 {
            return Err("no picture".into());
        }
        let factory: ID2D1Factory1 =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_MULTI_THREADED, None) }
                .map_err(|err| format!("Direct2D factory: {err}"))?;
        let dxgi: IDXGIDevice = gpu
            .device
            .cast()
            .map_err(|err| format!("IDXGIDevice: {err}"))?;
        let device = unsafe { factory.CreateDevice(&dxgi) }
            .map_err(|err| format!("Direct2D device: {err}"))?;
        let context = unsafe { device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE) }
            .map_err(|err| format!("Direct2D context: {err}"))?;

        let pixels = crate::watermark::premultiplied_bgra();
        let properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        let logo = unsafe {
            context.CreateBitmap(
                D2D_SIZE_U {
                    width: *width,
                    height: *height,
                },
                Some(pixels.as_ptr().cast()),
                width * 4,
                &properties,
            )
        }
        .map_err(|err| format!("watermark bitmap: {err}"))?;
        let lock: ID3D11Multithread = gpu
            .context
            .cast()
            .map_err(|err| format!("ID3D11Multithread: {err}"))?;

        Ok(Self {
            context,
            d3d: gpu.context.clone(),
            device: gpu.device.clone(),
            lock,
            logo,
            logo_size: (*width, *height),
            canvas: None,
        })
    }

    /// Drops the canvas. `true` if there was one.
    fn release(&mut self) -> bool {
        if self.canvas.is_none() {
            return false;
        }
        unsafe { self.context.SetTarget(None) };
        self.canvas = None;
        true
    }

    /// A canvas the size and format of the frame, made anew when either changes.
    fn canvas(&mut self, desc: &D3D11_TEXTURE2D_DESC) -> Result<&Canvas, String> {
        let shape = (desc.Width, desc.Height, desc.Format.0);
        let fits = self.canvas.as_ref().is_some_and(|canvas| canvas.shape == shape);
        if !fits {
            // Drop the old one first: the target bitmap points into the context.
            unsafe { self.context.SetTarget(None) };
            self.canvas = None;
            let canvas_desc = D3D11_TEXTURE2D_DESC {
                Width: desc.Width,
                Height: desc.Height,
                MipLevels: 1,
                ArraySize: 1,
                Format: desc.Format,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut texture = None;
            unsafe { self.device.CreateTexture2D(&canvas_desc, None, Some(&mut texture)) }
                .map_err(|err| format!("watermark canvas: {err}"))?;
            let texture = texture.ok_or_else(|| "watermark canvas missing".to_string())?;
            let surface: IDXGISurface = texture
                .cast()
                .map_err(|err| format!("IDXGISurface: {err}"))?;
            let properties = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: desc.Format,
                    // The desktop's alpha is meaningless; the picture is opaque.
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            let target = unsafe { self.context.CreateBitmapFromDxgiSurface(&surface, Some(&properties)) }
                .map_err(|err| format!("watermark target: {err}"))?;
            unsafe { self.context.SetTarget(&target) };
            self.canvas = Some(Canvas {
                texture,
                _target: target,
                shape,
            });
        }
        Ok(self.canvas.as_ref().unwrap())
    }

    /// The frame with the mark on it, or `None` when it is too small to carry
    /// one — then the caller simply uses the frame as it is.
    fn draw(&mut self, source: &ID3D11Texture2D) -> Result<Option<ID3D11Texture2D>, String> {
        // The texture, not the content size: the video processor scales the
        // whole texture, so that is the picture the corner is measured in.
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { source.GetDesc(&mut desc) };
        let (logo_width, logo_height) = self.logo_size;
        let Some(at) = crate::watermark::rect(desc.Width, desc.Height, logo_width, logo_height)
        else {
            return Ok(None);
        };
        unsafe { self.lock.Enter() };
        let result = (|| {
            let texture = self.canvas(&desc)?.texture.clone();
            unsafe {
                self.d3d.CopyResource(&texture, source);
                self.context.BeginDraw();
                self.context.DrawBitmap(
                    &self.logo,
                    Some(&D2D_RECT_F {
                        left: at.x as f32,
                        top: at.y as f32,
                        right: (at.x + at.width) as f32,
                        bottom: (at.y + at.height) as f32,
                    }),
                    crate::watermark::OPACITY,
                    D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                    None,
                    None,
                );
                self.context
                    .EndDraw(None, None)
                    .map_err(|err| format!("EndDraw: {err}"))?;
            }
            Ok(Some(texture))
        })();
        unsafe { self.lock.Leave() };
        result
    }
}

/// Receives what the clock puts out at even intervals.
pub trait FrameSink: Send {
    /// `pts_100ns` is the time on the output timeline, not the capture time —
    /// from here on the stream is clocked at a constant rate.
    ///
    /// `duplicate` means no new frame has arrived since the last tick and this is
    /// a repeat. To the encoder that makes no difference; to the status display it
    /// does.
    ///
    /// `qpc_100ns` is the capture time of exactly this frame, read under the
    /// same lock that picked it.
    fn on_frame(
        &mut self,
        texture: &ID3D11Texture2D,
        pts_100ns: i64,
        duplicate: bool,
        qpc_100ns: i64,
    );
}

/// What the capture callback and the clock share.
pub struct Latest {
    converter: Mutex<Converter>,
    /// Slot last converted, `u64::MAX` while there is none yet.
    slot: AtomicU64,
    /// QPC of the frame in that slot. The zero point of the output timeline later
    /// hangs on this — and with it audio sync.
    slot_qpc: AtomicI64,
    /// Counts every converted frame — that is how the clock tells whether
    /// anything new arrived since the last tick.
    generation: AtomicU64,
    /// How often the clock had to repeat a frame because WGC delivered nothing
    /// new. A high value means a still picture, not overload.
    pub duplicated: AtomicU64,
    /// Captured frames let go because no NV12 slot was free — see
    /// [`Self::submit`]. Anything above zero means the encoder is holding more
    /// frames than [`slots_for`] sized the slots for, and the picture is being thinned to
    /// keep it from being corrupted.
    pub starved: AtomicU64,
    /// The encoder's counters and the depth of its job channel, once it runs.
    /// Both are needed to tell how many frames are still holding a slot.
    encoder: Mutex<Option<(Arc<crate::mft::EncoderStats>, usize)>>,
    /// How many NV12 slots the converter rotates through; read here so the
    /// guard needs no converter lock.
    slot_count: usize,
    /// How often a tick was already late when it came round.
    ///
    /// This is the other half of the picture next to [`Self::duplicated`]: a
    /// repeated frame says the screen stood still, an overrun says *we* did.
    /// The clock catches the time back up on its own — it counts against an
    /// absolute start — but it catches up in a burst, and a burst is what a
    /// stutter looks like from the outside. Nothing counted this before, so a
    /// clock that never kept its beat looked exactly like one that did.
    pub overruns: AtomicU64,
    /// The generation the clock picked up last.
    taken: AtomicU64,
    /// Ticks the clock skipped because it was too far behind to catch up — not
    /// those of a gap longer than [`GAP`], which is sleep rather than loss.
    /// Each one is a frame the clip does not have.
    pub skipped: AtomicU64,
    running: AtomicBool,
}

impl Latest {
    pub fn new(converter: Converter) -> Arc<Self> {
        Arc::new(Self {
            slot_count: converter.slot_count(),
            converter: Mutex::new(converter),
            slot: AtomicU64::new(u64::MAX),
            slot_qpc: AtomicI64::new(0),
            generation: AtomicU64::new(0),
            duplicated: AtomicU64::new(0),
            starved: AtomicU64::new(0),
            encoder: Mutex::new(None),
            overruns: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
            taken: AtomicU64::new(u64::MAX),
            running: AtomicBool::new(true),
        })
    }

    /// Hand the encoder's counters over, once there is an encoder to ask.
    ///
    /// Separate from `new` because the converter is built before the encoder is —
    /// the encoder needs the graphics device the converter sets up.
    pub fn watch_encoder(&self, stats: Arc<crate::mft::EncoderStats>, queue_depth: usize) {
        *self.encoder.lock() = Some((stats, queue_depth));
    }

    /// Is the slot the rotation is about to take still being read?
    ///
    /// Conservative on purpose: it counts frames, and a still picture has the
    /// clock sending the same slot over and over, so several of the frames
    /// counted here may be the same texture. Overstating costs a repeated
    /// picture; understating costs a torn one.
    fn slot_would_be_snatched(&self) -> bool {
        match self.encoder.lock().as_ref() {
            Some((stats, depth)) => {
                *depth as u64 + stats.in_flight.load(Ordering::Relaxed) >= self.slot_count as u64
            }
            None => false,
        }
    }

    /// Called from the capture callback.
    ///
    /// A frame is let go rather than converted when no slot is free. That is the
    /// whole point of counting: the alternative is not a better picture, it is
    /// the same picture written over one the encoder has not finished reading.
    /// Nothing is lost from the stream — the clock repeats the previous frame, as
    /// it does for a still picture, so the clip keeps its frame count and its
    /// length.
    pub fn submit(
        &self,
        source: &ID3D11Texture2D,
        source_size: (u32, u32),
        qpc_100ns: i64,
    ) -> Result<(), String> {
        if self.slot_would_be_snatched() {
            if self.starved.fetch_add(1, Ordering::Relaxed) == 0 {
                log::warn!(
                    "the encoder is holding more frames than there are texture slots — \
                     captured frames are being let go to keep it from reading one twice over"
                );
            }
            return Ok(());
        }

        // One slot per tick, not one per captured frame. A monitor at 144 Hz
        // or more converts several times between two ticks, and rotating on
        // each of them lapped the ring and wrote over textures still queued
        // for the encoder — the guard above counts jobs, and only holds if
        // the rotation keeps pace with them. As long as the clock has not
        // picked up the frame converted last, no job points at its slot, so
        // the next frame goes into the same one. Decided under the converter
        // lock, which the clock takes too when it picks a frame up.
        let mut converter = self.converter.lock();
        let reuse = self.slot.load(Ordering::Acquire) != u64::MAX
            && self.taken.load(Ordering::Acquire) != self.generation.load(Ordering::Acquire);
        let slot = converter.convert(source, source_size, reuse)?;
        self.slot_qpc.store(qpc_100ns, Ordering::Relaxed);
        self.slot.store(slot as u64, Ordering::Release);
        self.generation.fetch_add(1, Ordering::Release);
        drop(converter);
        Ok(())
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

/// How far the clock may fall behind before it skips rather than catches up.
const CATCH_UP: Duration = Duration::from_millis(250);

/// From this far behind the clock was not slow, the PC was asleep (or the
/// process stopped): skipped, but not counted as frames lost. Eight hours of
/// standby were 1.7 million "dropped frames" in the console and on the
/// Stream Deck otherwise.
///
/// The same 2 s ([`DISCONTINUITY_US`](crate::buffer::DISCONTINUITY_US)) that
/// decides whether a clip's frames are stretched over their time
/// (`muxer::timing`): a jump the counter calls sleep is one the clip treats as
/// sleep, and a shortfall it counts is one the clip stretches.
pub const GAP: Duration = Duration::from_micros(crate::buffer::DISCONTINUITY_US as u64);

/// When tick `index` is due, counted from the start. Nanoseconds in a `u64`
/// rather than `Duration * u32`, which wrapped after 2³² ticks.
fn tick_offset(period: Duration, index: u64) -> Duration {
    Duration::from_nanos((period.as_nanos() as u64).saturating_mul(index))
}

/// The clock. Runs until `Latest::stop`.
pub fn pace(latest: Arc<Latest>, fps: u32, mut sink: Box<dyn FrameSink>) {
    let fps = fps.max(1) as u64;
    let period = Duration::from_nanos(1_000_000_000 / fps);
    let start = Instant::now();
    let mut index: u64 = 0;
    let mut last_generation = u64::MAX;

    while latest.running.load(Ordering::Relaxed) {
        // Sleep against the start time rather than one period at a time:
        // otherwise the wake-up delays add up and the frame rate slowly drifts
        // downwards.
        let due = start + tick_offset(period, index);
        let now = Instant::now();
        if due > now {
            std::thread::sleep(due - now);
        } else if index > 0 {
            latest.overruns.fetch_add(1, Ordering::Relaxed);
            // Far behind: the encoder cannot keep up, or the PC slept. Every
            // missed tick replayed meant a burst of the same old picture —
            // after a night's sleep minutes of it at full encoder load — and
            // timestamps that fell further behind the clock with every second
            // the encoder stayed short, until the audio cut for a clip lay
            // outside the ring and the clip was silent. Jump to the present
            // instead; the clip is muxed at the rate its timestamps really
            // span (`muxer::timing`). A short stall is still caught up.
            let behind = now - due;
            if behind > CATCH_UP {
                let skip = (behind.as_nanos() / period.as_nanos()) as u64;
                index += skip;
                if behind <= GAP {
                    latest.skipped.fetch_add(skip, Ordering::Relaxed);
                }
            }
        }

        // Under the converter lock, so `Latest::submit` sees the frame as
        // taken before it could write into its slot again.
        let converter = latest.converter.lock();
        let generation = latest.generation.load(Ordering::Acquire);
        let slot = latest.slot.load(Ordering::Acquire);
        let slot_qpc = latest.slot_qpc.load(Ordering::Relaxed);
        latest.taken.store(generation, Ordering::Release);
        index += 1;

        if slot == u64::MAX {
            // Before the first frame there is nothing to repeat.
            continue;
        }
        let duplicate = generation == last_generation;
        if duplicate {
            latest.duplicated.fetch_add(1, Ordering::Relaxed);
        }
        last_generation = generation;

        // With a still picture WGC delivers nothing — then the last slot goes out
        // again so the stream does not stall.
        let pts = (index as i64 - 1) * 10_000_000 / fps as i64;
        let texture = SendPtr(converter.texture(slot as usize).clone());
        drop(converter);
        sink.on_frame(&texture, pts, duplicate, slot_qpc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CAP-6: the slots follow the queue the encoder really has.
    #[test]
    fn a_deeper_queue_gets_the_slots_to_match() {
        assert_eq!(slots_for(crate::mft::QUEUE_DEPTH), 22, "the default is what was measured");
        let depth = 30;
        assert!(
            slots_for(depth) > depth + crate::mft::ENCODER_HOLD_MAX,
            "the guard can never starve from the first frame"
        );
    }

    /// CAP-10: `period * index as u32` wrapped after 2³² ticks (828 days at 60 fps).
    #[test]
    fn the_tick_schedule_does_not_wrap_after_two_to_the_thirty_two_ticks() {
        let period = Duration::from_nanos(16_666_666);
        assert_eq!(tick_offset(period, 3), period * 3);
        assert_eq!(
            tick_offset(period, 1 << 32),
            Duration::from_nanos(16_666_666 << 32)
        );
    }
}
