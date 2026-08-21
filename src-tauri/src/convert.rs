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

/// How many NV12 textures are used in rotation.
///
/// The encoder MFT only releases a submitted texture once it is done with it. At
/// 60 fps a slot is not up again for a good 130 ms after 8 frames — considerably
/// more than a hardware encoder ever holds on to.
const SLOTS: usize = 8;

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
    /// Input views keyed by the source texture's raw pointer. The frame pool has
    /// only two textures, so the table stays tiny — and calling
    /// `CreateVideoProcessorInputView` afresh for every frame would be waste.
    input_views: Vec<(isize, ID3D11VideoProcessorInputView)>,
    width: u32,
    height: u32,
}

impl Converter {
    pub fn new(gpu: &GpuDevice, width: u32, height: u32, fps: u32) -> Result<Self, String> {
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
        }

        let mut slots = Vec::with_capacity(SLOTS);
        for _ in 0..SLOTS {
            slots.push(Self::make_slot(
                &gpu.device,
                &video_device,
                &enumerator,
                width,
                height,
            )?);
        }

        Ok(Self {
            video_device,
            video_context,
            enumerator,
            processor,
            slots,
            next_slot: 0,
            input_views: Vec::new(),
            width,
            height,
        })
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
    pub fn convert(&mut self, source: &ID3D11Texture2D) -> Result<usize, String> {
        let input = self.input_view(source)?;
        let slot = self.next_slot;
        self.next_slot = (self.next_slot + 1) % self.slots.len();

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

        let result = unsafe {
            self.video_context.VideoProcessorBlt(
                &self.processor,
                &self.slots[slot].output_view,
                0,
                &[stream],
            )
        };
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

/// Receives what the clock puts out at even intervals.
pub trait FrameSink: Send {
    /// `pts_100ns` is the time on the output timeline, not the capture time —
    /// from here on the stream is clocked at a constant rate.
    ///
    /// `duplicate` means no new frame has arrived since the last tick and this is
    /// a repeat. To the encoder that makes no difference; to the status display it
    /// does.
    fn on_frame(&mut self, texture: &ID3D11Texture2D, pts_100ns: i64, duplicate: bool);
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
    running: AtomicBool,
}

impl Latest {
    pub fn new(converter: Converter) -> Arc<Self> {
        Arc::new(Self {
            converter: Mutex::new(converter),
            slot: AtomicU64::new(u64::MAX),
            slot_qpc: AtomicI64::new(0),
            generation: AtomicU64::new(0),
            duplicated: AtomicU64::new(0),
            running: AtomicBool::new(true),
        })
    }

    /// Called from the capture callback.
    pub fn submit(&self, source: &ID3D11Texture2D, qpc_100ns: i64) -> Result<(), String> {
        let slot = self.converter.lock().convert(source)?;
        self.slot_qpc.store(qpc_100ns, Ordering::Relaxed);
        self.slot.store(slot as u64, Ordering::Release);
        self.generation.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// QPC of the frame last converted.
    pub fn frame_qpc(&self) -> i64 {
        self.slot_qpc.load(Ordering::Relaxed)
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
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
        let due = start + period * index as u32;
        let now = Instant::now();
        if due > now {
            std::thread::sleep(due - now);
        }

        let generation = latest.generation.load(Ordering::Acquire);
        let slot = latest.slot.load(Ordering::Acquire);
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
        let converter = latest.converter.lock();
        let texture = SendPtr(converter.texture(slot as usize).clone());
        drop(converter);
        sink.on_frame(&texture, pts, duplicate);
    }
}
