//! BGRA → NV12 auf der GPU, und der Taktgeber, der daraus einen Strom mit
//! konstanter Bildrate macht.
//!
//! Beides gehört zusammen, weil beides dasselbe Problem löst:
//! Windows.Graphics.Capture liefert Bilder **nur bei Änderung** und höchstens
//! in Bildschirm-Wiederholrate. Vorher ging jedes dieser Bilder ungefiltert an
//! einen Encoder, dem gleichzeitig konstante 60 fps gemeldet wurden — der
//! musste die Bildrate selbst umrechnen und hat dabei ungleichmäßig verworfen
//! und verdoppelt. Das war der Judder.
//!
//! Hier taktet stattdessen ein eigener Faden auf `1/fps`. Liegt ein neues Bild
//! vor, geht es raus; liegt keines vor (ruhiges Bild), geht das letzte noch
//! einmal raus. Der Encoder sieht damit exakt `fps` Bilder pro Sekunde in
//! gleichen Abständen. Genau so machen es Medal und ShadowPlay.

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

/// Wie viele NV12-Texturen reihum benutzt werden.
///
/// Der Encoder-MFT gibt eine eingereichte Textur erst frei, wenn er mit ihr
/// fertig ist. Bei 60 fps ist ein Platz nach 8 Bildern gut 130 ms nicht mehr
/// dran — deutlich mehr, als ein Hardware-Encoder je vorhält.
const SLOTS: usize = 8;

/// Farbraum-Bitfeld aus `d3d11.h`:
/// Bit 0 `Usage`, Bit 1 `RGB_Range`, Bit 2 `YCbCr_Matrix`, Bit 3 `YCbCr_Xvycc`,
/// Bits 4–5 `Nominal_Range`.
const fn color_space(ycbcr_709: bool, nominal_range: u32) -> D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
    let bits = ((ycbcr_709 as u32) << 2) | ((nominal_range & 0x3) << 4);
    D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: bits }
}

/// `D3D11_VIDEO_PROCESSOR_NOMINAL_RANGE_16_235` — der Bereich, den H.264 und
/// jeder Player als Standard annimmt.
const RANGE_STUDIO: u32 = 1;
/// `D3D11_VIDEO_PROCESSOR_NOMINAL_RANGE_0_255` — so kommt der Desktop an.
const RANGE_FULL: u32 = 2;

struct Slot {
    texture: ID3D11Texture2D,
    output_view: ID3D11VideoProcessorOutputView,
}

/// Wandelt die BGRA-Bilder der Aufnahme in NV12 um und skaliert dabei auf die
/// Zielauflösung.
pub struct Converter {
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    slots: Vec<Slot>,
    next_slot: usize,
    /// Eingangsansichten nach Rohzeiger der Quelltextur. Der Frame-Pool hat
    /// nur zwei Texturen, die Tabelle bleibt also winzig — und `CreateVideo­
    /// ProcessorInputView` bei jedem Bild neu aufzurufen wäre Verschwendung.
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

        // Farbraum ausdrücklich festlegen. Ohne das rät der Treiber, und je
        // nach GPU kommen ausgewaschene oder abgesoffene Farben heraus — ein
        // guter Teil des „sieht schlechter aus als Medal".
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

        // Wächst der Frame-Pool nach einem `Recreate` neu, sammeln sich sonst
        // Ansichten auf längst freigegebene Texturen an.
        if self.input_views.len() >= 4 {
            self.input_views.clear();
        }
        self.input_views.push((key, view.clone()));
        Ok(view)
    }

    /// Ein aufgenommenes Bild umwandeln. Gibt den benutzten Platz zurück.
    ///
    /// Läuft **synchron im Capture-Rückruf**, solange die Quelltextur gültig
    /// ist — das ist der Unterschied zum alten Weg, der nur den Zeiger
    /// weiterreichte und später las.
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

/// Nimmt entgegen, was der Taktgeber im gleichmäßigen Abstand ausspuckt.
pub trait FrameSink: Send {
    /// `pts_100ns` ist die Zeit auf der Ausgabe-Zeitachse, nicht die
    /// Aufnahmezeit — der Strom ist ab hier konstant getaktet.
    ///
    /// `duplicate` heißt: Seit dem letzten Takt kam kein neues Bild, das hier
    /// ist eine Wiederholung. Für den Encoder macht das keinen Unterschied,
    /// für die Statusanzeige schon.
    fn on_frame(&mut self, texture: &ID3D11Texture2D, pts_100ns: i64, duplicate: bool);
}

/// Was der Capture-Rückruf und der Taktgeber sich teilen.
pub struct Latest {
    converter: Mutex<Converter>,
    /// Zuletzt umgewandelter Platz, `u64::MAX` solange noch keiner da ist.
    slot: AtomicU64,
    /// QPC des Bildes, das in diesem Platz liegt. Daran hängt später der
    /// Nullpunkt der Ausgabezeitachse — und damit die Tonsynchronität.
    slot_qpc: AtomicI64,
    /// Zählt jedes umgewandelte Bild — daran erkennt der Taktgeber, ob seit
    /// dem letzten Tick etwas Neues kam.
    generation: AtomicU64,
    /// Wie oft der Taktgeber ein Bild wiederholen musste, weil WGC nichts
    /// Neues geliefert hat. Hoher Wert heißt: ruhiges Bild, nicht Überlastung.
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

    /// Vom Capture-Rückruf aufgerufen.
    pub fn submit(&self, source: &ID3D11Texture2D, qpc_100ns: i64) -> Result<(), String> {
        let slot = self.converter.lock().convert(source)?;
        self.slot_qpc.store(qpc_100ns, Ordering::Relaxed);
        self.slot.store(slot as u64, Ordering::Release);
        self.generation.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// QPC des zuletzt umgewandelten Bildes.
    pub fn frame_qpc(&self) -> i64 {
        self.slot_qpc.load(Ordering::Relaxed)
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

/// Der Taktgeber. Läuft bis `Latest::stop`.
pub fn pace(latest: Arc<Latest>, fps: u32, mut sink: Box<dyn FrameSink>) {
    let fps = fps.max(1) as u64;
    let period = Duration::from_nanos(1_000_000_000 / fps);
    let start = Instant::now();
    let mut index: u64 = 0;
    let mut last_generation = u64::MAX;

    while latest.running.load(Ordering::Relaxed) {
        // Gegen den Startzeitpunkt schlafen statt jeweils eine Periode: Sonst
        // summieren sich die Aufwachverzögerungen und die Bildrate driftet
        // langsam nach unten.
        let due = start + period * index as u32;
        let now = Instant::now();
        if due > now {
            std::thread::sleep(due - now);
        }

        let generation = latest.generation.load(Ordering::Acquire);
        let slot = latest.slot.load(Ordering::Acquire);
        index += 1;

        if slot == u64::MAX {
            // Vor dem ersten Bild gibt es nichts zu wiederholen.
            continue;
        }
        let duplicate = generation == last_generation;
        if duplicate {
            latest.duplicated.fetch_add(1, Ordering::Relaxed);
        }
        last_generation = generation;

        // Bei ruhigem Bild liefert WGC nichts — dann geht der letzte Platz
        // noch einmal raus, damit der Strom nicht stehen bleibt.
        let pts = (index as i64 - 1) * 10_000_000 / fps as i64;
        let converter = latest.converter.lock();
        let texture = SendPtr(converter.texture(slot as usize).clone());
        drop(converter);
        sink.on_frame(&texture, pts, duplicate);
    }
}
