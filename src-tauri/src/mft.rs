//! Der H.264-Encoder, direkt als Media-Foundation-Transform.
//!
//! Vorher lief die Aufnahme über den WinRT-`MediaTranscoder`. Der nimmt außer
//! einer Bitrate **nichts** entgegen: keine Ratensteuerung, keinen
//! GOP-Abstand, kein Profil, kein CABAC. Deshalb sahen 40 Mbit/s aus wie
//! deutlich weniger, und deshalb tat der Encoder-Schalter in den
//! Einstellungen nichts — welcher MFT lief, entschied Windows.
//!
//! Hier wird der MFT selbst gesucht, selbst konfiguriert und selbst gefüttert.
//! Er bekommt NV12-Texturen direkt von der GPU (kein Readback) und liefert
//! fertige H.264-Pakete, die in den Ringpuffer aus `buffer.rs` wandern.

#![cfg(windows)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use windows::core::{Interface, GUID, VARIANT};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;

use crate::buffer::{EncodedPacket, VIDEO_TRACK};
use crate::gpu::{GpuDevice, SendPtr};
use crate::model::EncoderId;

const VENDOR_NVIDIA: &str = "VEN_10DE";
const VENDOR_AMD: &str = "VEN_1002";
const VENDOR_INTEL: &str = "VEN_8086";

/// Wie viele Bilder auf die Einreichung warten dürfen. Bei 60 fps sind vier
/// Plätze gut 66 ms Spielraum für einen kurzen Hänger des Encoders.
const QUEUE_DEPTH: usize = 4;

/// So lange wartet der Taktgeber, wenn die Warteschlange voll ist.
///
/// Warten statt wegwerfen: Der fertige Clip wird als roher Elementarstrom
/// gemuxt, dessen Zeitachse allein aus der Bildrate entsteht (`-r` in
/// `muxer.rs`). Fehlt darin auch nur ein Bild, ist die Datei kürzer als ihr
/// Ton — das Bild liefe dem Ton davon. Ein Tick Verzögerung kostet dagegen
/// nichts: Der Taktgeber rechnet seine Zeitstempel aus einem Zähler und holt
/// den Rückstand von selbst wieder auf.
const SUBMIT_WAIT: Duration = Duration::from_millis(500);

pub struct EncoderSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub keyframe_seconds: u32,
    pub requested: EncoderId,
}

/// Media Foundation einmal je Prozess hochfahren.
pub fn startup() -> Result<(), String> {
    static STATE: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    STATE
        .get_or_init(|| unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)
                .map_err(|err| format!("Media Foundation: {err}"))
        })
        .clone()
}

fn vendor_of(activate: &IMFActivate) -> Option<String> {
    unsafe {
        let mut len = 0u32;
        let mut buf = [0u16; 64];
        activate
            .GetString(&MFT_ENUM_HARDWARE_VENDOR_ID_Attribute, &mut buf, Some(&mut len))
            .ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]).to_uppercase())
    }
}

fn vendor_to_encoder(vendor: &str) -> Option<EncoderId> {
    if vendor.contains(VENDOR_NVIDIA) {
        Some(EncoderId::Nvenc)
    } else if vendor.contains(VENDOR_AMD) {
        Some(EncoderId::Amf)
    } else if vendor.contains(VENDOR_INTEL) {
        Some(EncoderId::Qsv)
    } else {
        None
    }
}

/// Alle H.264-Encoder-MFTs auflisten, Hardware zuerst.
fn enumerate() -> Result<Vec<(Option<EncoderId>, IMFActivate)>, String> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };

    let mut array: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input),
            Some(&output),
            &mut array,
            &mut count,
        )
        .map_err(|err| format!("Encoder suchen: {err}"))?;
    }

    let mut found = Vec::new();
    unsafe {
        for index in 0..count as usize {
            if let Some(activate) = (*array.add(index)).clone() {
                let id = vendor_of(&activate).as_deref().and_then(vendor_to_encoder);
                found.push((id, activate));
            }
        }
        // Das Feld gehört uns; die Verweise darin haben wir oben geklont.
        for index in 0..count as usize {
            let _ = (*array.add(index)).take();
        }
        CoTaskMemFree(Some(array as *const _));
    }
    Ok(found)
}

/// Welche Hardware-Encoder es auf diesem Rechner wirklich gibt.
///
/// Ersetzt die frühere Vermutung über die DXGI-Hersteller-ID: Eine NVIDIA-Karte
/// im Rechner heißt noch nicht, dass ihr Encoder-MFT auch angemeldet ist.
pub fn available_encoders() -> Vec<EncoderId> {
    let Ok(list) = enumerate() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (id, _) in list {
        if let Some(id) = id {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

/// Eine Eigenschaft setzen. `false` heißt: Der Treiber kennt sie nicht.
///
/// Nicht jeder Encoder kennt jede Eigenschaft. Was nicht angenommen wird,
/// bleibt auf dem Standard — das ist kein Grund, die Aufnahme scheitern zu
/// lassen. Gemeldet gehört es trotzdem: Wenn ein Clip schlechter aussieht als
/// erwartet, ist genau das die erste Frage.
fn set_codec_property(codec: &ICodecAPI, name: &str, api: &GUID, value: VARIANT) -> bool {
    unsafe {
        match codec.SetValue(api, &value) {
            Ok(()) => true,
            Err(err) => {
                log::debug!("Encoder nimmt '{name}' nicht an: {err}");
                false
            }
        }
    }
}

fn configure_codec(transform: &IMFTransform, settings: &EncoderSettings) {
    let Ok(codec) = transform.cast::<ICodecAPI>() else {
        log::warn!("Encoder bietet kein ICodecAPI — Qualitätseinstellungen bleiben aus");
        return;
    };

    let mean = settings.bitrate_kbps.saturating_mul(1000);
    // Spitzen dürfen über den Mittelwert hinaus: Bei einer schnellen Drehung
    // im Spiel braucht ein Bild ein Vielfaches eines ruhigen.
    let peak = mean.saturating_add(mean / 2);
    let gop = settings.fps.max(1) * settings.keyframe_seconds.max(1);

    let wanted: Vec<(&str, &GUID, VARIANT)> = vec![
        (
            "Ratensteuerung",
            &CODECAPI_AVEncCommonRateControlMode,
            VARIANT::from(eAVEncCommonRateControlMode_UnconstrainedVBR.0 as u32),
        ),
        ("Bitrate", &CODECAPI_AVEncCommonMeanBitRate, VARIANT::from(mean)),
        ("Spitzenbitrate", &CODECAPI_AVEncCommonMaxBitRate, VARIANT::from(peak)),
        ("Keyframe-Abstand", &CODECAPI_AVEncMPVGOPSize, VARIANT::from(gop)),
        // 0 = schnellstmöglich, 100 = beste Qualität. Ein Replay-Puffer läuft
        // im Hintergrund, aber nicht in Echtzeit-Not — 70 ist der Punkt, an dem
        // NVENC und AMF spürbar besser werden, ohne Bilder zu verlieren.
        ("Qualität/Tempo", &CODECAPI_AVEncCommonQualityVsSpeed, VARIANT::from(70u32)),
        // CABAC statt CAVLC: bei gleicher Bitrate rund 10 % weniger Artefakte.
        ("CABAC", &CODECAPI_AVEncH264CABACEnable, VARIANT::from(true)),
        // Low-Latency schaltet Lookahead und B-Frames ab. Für einen
        // Replay-Puffer ist Latenz gleichgültig, Qualität nicht.
        ("Low-Latency aus", &CODECAPI_AVLowLatencyMode, VARIANT::from(false)),
    ];

    let rejected: Vec<&str> = wanted
        .into_iter()
        .filter(|(name, api, value)| !set_codec_property(&codec, name, api, value.clone()))
        .map(|(name, _, _)| name)
        .collect();
    if !rejected.is_empty() {
        log::warn!(
            "Encoder kennt diese Einstellungen nicht: {} — sie bleiben auf dem Standard",
            rejected.join(", ")
        );
    }

    // Angenommen heißt nicht übernommen. Der NVIDIA-MFT etwa quittiert jeden
    // Keyframe-Abstand mit Erfolg, deckelt ihn aber auf die Bildrate — aus
    // „alle 2 s" wird stillschweigend „jede Sekunde". Genau solche stummen
    // Abweichungen waren der Grund, dass die Einstellungen vorher nichts taten,
    // ohne dass es jemandem auffiel.
    if let Some(effective) = read_u32(&codec, &CODECAPI_AVEncMPVGOPSize) {
        if effective != gop {
            log::info!(
                "Encoder hält sich nicht an den Keyframe-Abstand: gewünscht alle {} Bilder, \
                 tatsächlich alle {effective}",
                gop
            );
        }
    }
}

fn read_u32(codec: &ICodecAPI, api: &GUID) -> Option<u32> {
    unsafe { u32::try_from(&codec.GetValue(api).ok()?).ok() }
}

fn media_type_video(
    subtype: GUID,
    settings: &EncoderSettings,
) -> Result<IMFMediaType, String> {
    unsafe {
        let media = MFCreateMediaType().map_err(|err| format!("Medientyp: {err}"))?;
        media
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|err| format!("Medientyp (Major): {err}"))?;
        media
            .SetGUID(&MF_MT_SUBTYPE, &subtype)
            .map_err(|err| format!("Medientyp (Subtype): {err}"))?;
        media
            .SetUINT64(
                &MF_MT_FRAME_SIZE,
                ((settings.width as u64) << 32) | settings.height as u64,
            )
            .map_err(|err| format!("Bildgröße: {err}"))?;
        media
            .SetUINT64(&MF_MT_FRAME_RATE, ((settings.fps.max(1) as u64) << 32) | 1)
            .map_err(|err| format!("Bildrate: {err}"))?;
        media
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)
            .map_err(|err| format!("Pixelseitenverhältnis: {err}"))?;
        media
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|err| format!("Interlace: {err}"))?;

        // Farbmetadaten mitschreiben, passend zu dem, was `convert.rs`
        // tatsächlich erzeugt. Ohne diese Angaben rät der Player, und derselbe
        // Clip sieht in Discord anders aus als im Schnittprogramm.
        let _ = media.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32);
        let _ = media.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32);
        let _ = media.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32);
        let _ = media.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32);

        Ok(media)
    }
}

struct Transform {
    transform: IMFTransform,
    events: Option<IMFMediaEventGenerator>,
    provides_samples: bool,
    chosen: EncoderId,
    /// SPS/PPS aus dem Ausgabetyp. Die meisten Encoder schicken sie ohnehin vor
    /// jedem IDR mit; fehlen sie, ist der Elementarstrom ohne diesen Vorspann
    /// nicht dekodierbar. Doppelt schadet nicht, gar nicht schon.
    sequence_header: Vec<u8>,
}

fn build(gpu: &GpuDevice, settings: &EncoderSettings) -> Result<Transform, String> {
    startup()?;
    let candidates = enumerate()?;
    if candidates.is_empty() {
        return Err("Kein H.264-Encoder gefunden".into());
    }

    // Gewünschten Encoder zuerst; sonst der erste Hardware-Encoder; sonst der
    // erste überhaupt (Software-Fallback).
    let pick = candidates
        .iter()
        .position(|(id, _)| *id == Some(settings.requested))
        .or_else(|| candidates.iter().position(|(id, _)| id.is_some()))
        .unwrap_or(0);
    let (chosen_id, activate) = &candidates[pick];
    let chosen = chosen_id.unwrap_or(EncoderId::X264);

    let transform: IMFTransform = unsafe { activate.ActivateObject() }
        .map_err(|err| format!("Encoder starten: {err}"))?;

    // Hardware-Encoder sind asynchron und müssen dafür erst freigeschaltet
    // werden — sonst weist `SetOutputType` sie zurück.
    let mut is_async = false;
    if let Ok(attributes) = unsafe { transform.GetAttributes() } {
        is_async = unsafe { attributes.GetUINT32(&MF_TRANSFORM_ASYNC) }.unwrap_or(0) == 1;
        if is_async {
            unsafe { attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1) }
                .map_err(|err| format!("Encoder freischalten: {err}"))?;
        }
    }

    // Das D3D11-Gerät durchreichen — nur so nimmt der Encoder Texturen
    // entgegen, statt ein Readback über den Hauptspeicher zu verlangen.
    let mut token = 0u32;
    let mut manager: Option<IMFDXGIDeviceManager> = None;
    unsafe {
        MFCreateDXGIDeviceManager(&mut token, &mut manager)
            .map_err(|err| format!("DXGI-Gerätemanager: {err}"))?;
    }
    let manager = manager.ok_or_else(|| "DXGI-Gerätemanager fehlt".to_string())?;
    unsafe {
        manager
            .ResetDevice(&gpu.device, token)
            .map_err(|err| format!("Gerät anmelden: {err}"))?;
        transform
            .ProcessMessage(
                MFT_MESSAGE_SET_D3D_MANAGER,
                manager.as_raw() as usize,
            )
            .map_err(|err| format!("Gerät an Encoder: {err}"))?;
    }

    // Reihenfolge ist vorgeschrieben: Ausgabetyp vor Eingabetyp.
    let output = media_type_video(MFVideoFormat_H264, settings)?;
    unsafe {
        output
            .SetUINT32(&MF_MT_AVG_BITRATE, settings.bitrate_kbps.saturating_mul(1000))
            .map_err(|err| format!("Bitrate: {err}"))?;
        let _ = output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32);
        // Keyframe-Abstand gehört hierher, nicht nur an ICodecAPI: Der
        // NVIDIA-MFT nimmt `CODECAPI_AVEncMPVGOPSize` zwar widerspruchslos an,
        // richtet sich aber nach diesem Feld. Ohne das blieb es bei einem
        // Keyframe pro Sekunde, egal was eingestellt war.
        let _ = output.SetUINT32(
            &MF_MT_MAX_KEYFRAME_SPACING,
            settings.fps.max(1) * settings.keyframe_seconds.max(1),
        );
        transform
            .SetOutputType(0, &output, 0)
            .map_err(|err| format!("Ausgabetyp: {err}"))?;
    }

    let input = media_type_video(MFVideoFormat_NV12, settings)?;
    unsafe {
        transform
            .SetInputType(0, &input, 0)
            .map_err(|err| format!("Eingabetyp: {err}"))?;
    }

    configure_codec(&transform, settings);

    let provides_samples = unsafe { transform.GetOutputStreamInfo(0) }
        .map(|info| info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0)
        .unwrap_or(false);

    let events = if is_async {
        transform.cast::<IMFMediaEventGenerator>().ok()
    } else {
        None
    };

    let sequence_header = unsafe {
        transform
            .GetOutputCurrentType(0)
            .ok()
            .and_then(|media| {
                let size = media.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER).ok()?;
                let mut blob = vec![0u8; size as usize];
                media.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut blob, None).ok()?;
                Some(blob)
            })
            .unwrap_or_default()
    };

    unsafe {
        let _ = transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
        transform
            .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
            .map_err(|err| format!("Encoder starten: {err}"))?;
        let _ = transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);
    }

    Ok(Transform {
        transform,
        events,
        provides_samples,
        chosen,
        sequence_header,
    })
}

/// Ein Bild auf dem Weg zum Encoder.
struct Job {
    texture: SendPtr<ID3D11Texture2D>,
    pts_100ns: i64,
}

impl Transform {
    fn feed(&self, job: &Job, duration_100ns: i64) -> Result<(), String> {
        unsafe {
            let buffer = MFCreateDXGISurfaceBuffer(
                &ID3D11Texture2D::IID,
                &*job.texture,
                0,
                false,
            )
            .map_err(|err| format!("Texturpuffer: {err}"))?;

            // Ohne gesetzte Länge hält der Encoder den Puffer für leer.
            if let Ok(two_d) = buffer.cast::<IMF2DBuffer>() {
                if let Ok(len) = two_d.GetContiguousLength() {
                    let _ = buffer.SetCurrentLength(len);
                }
            }

            let sample = MFCreateSample().map_err(|err| format!("Sample: {err}"))?;
            sample
                .AddBuffer(&buffer)
                .map_err(|err| format!("Sample füllen: {err}"))?;
            sample
                .SetSampleTime(job.pts_100ns)
                .map_err(|err| format!("Sample-Zeit: {err}"))?;
            let _ = sample.SetSampleDuration(duration_100ns);

            self.transform
                .ProcessInput(0, &sample, 0)
                .map_err(|err| format!("Bild einreichen: {err}"))
        }
    }

    /// Ein fertiges Paket abholen. `Ok(None)` heißt: gerade nichts da.
    fn take_output(&self, duration_100ns: i64) -> Result<Option<EncodedPacket>, String> {
        unsafe {
            let mut data = MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: std::mem::ManuallyDrop::new(None),
                dwStatus: 0,
                pEvents: std::mem::ManuallyDrop::new(None),
            };

            // Manche Encoder liefern das Sample selbst, andere erwarten eines.
            if !self.provides_samples {
                let info = self
                    .transform
                    .GetOutputStreamInfo(0)
                    .map_err(|err| format!("Ausgabeinfo: {err}"))?;
                let buffer = MFCreateMemoryBuffer(info.cbSize.max(1))
                    .map_err(|err| format!("Ausgabepuffer: {err}"))?;
                let sample = MFCreateSample().map_err(|err| format!("Ausgabesample: {err}"))?;
                sample
                    .AddBuffer(&buffer)
                    .map_err(|err| format!("Ausgabesample füllen: {err}"))?;
                data.pSample = std::mem::ManuallyDrop::new(Some(sample));
            }

            let mut status = 0u32;
            let mut buffers = [data];
            let result = self.transform.ProcessOutput(0, &mut buffers, &mut status);
            let [mut data] = buffers;
            let sample = std::mem::ManuallyDrop::take(&mut data.pSample);
            let _ = std::mem::ManuallyDrop::take(&mut data.pEvents);

            match result {
                Ok(()) => {}
                Err(err) if err.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                Err(err) if err.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    // Der Encoder hat seinen Ausgabetyp geändert (kommt bei
                    // manchen Treibern einmal zu Beginn vor). Neu abholen und
                    // erneut anfragen — sonst steht der Strom.
                    if let Ok(media) = self.transform.GetOutputAvailableType(0, 0) {
                        let _ = self.transform.SetOutputType(0, &media, 0);
                    }
                    return Ok(None);
                }
                Err(err) => return Err(format!("Paket abholen: {err}")),
            }

            let Some(sample) = sample else {
                return Ok(None);
            };

            let keyframe = sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0;
            let pts_100ns = sample.GetSampleTime().unwrap_or(0);

            let buffer = sample
                .ConvertToContiguousBuffer()
                .map_err(|err| format!("Paketpuffer: {err}"))?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut len = 0u32;
            buffer
                .Lock(&mut ptr, None, Some(&mut len))
                .map_err(|err| format!("Paket lesen: {err}"))?;
            let bytes: Arc<[u8]> = std::slice::from_raw_parts(ptr, len as usize).into();
            let _ = buffer.Unlock();

            Ok(Some(EncodedPacket {
                track: VIDEO_TRACK,
                pts_us: pts_100ns / 10,
                duration_us: duration_100ns / 10,
                keyframe,
                data: bytes,
            }))
        }
    }
}

/// Griff auf den laufenden Encoder.
pub struct VideoEncoder {
    jobs: crossbeam_channel::Sender<Job>,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Welcher Encoder es tatsächlich geworden ist — die Statusanzeige zeigt
    /// damit die Wahrheit statt der Wunschvorstellung aus der Konfiguration.
    pub chosen: EncoderId,
    /// SPS/PPS, die beim Speichern vor den Strom gehören.
    pub sequence_header: Vec<u8>,
}

impl VideoEncoder {
    /// `on_packet` läuft auf dem Encoder-Faden und muss kurz sein.
    pub fn start<F>(
        gpu: Arc<GpuDevice>,
        settings: EncoderSettings,
        on_packet: F,
    ) -> Result<Self, String>
    where
        F: FnMut(EncodedPacket) + Send + 'static,
    {
        let (jobs_tx, jobs_rx) = crossbeam_channel::bounded::<Job>(QUEUE_DEPTH);
        let (ready_tx, ready_rx) =
            crossbeam_channel::bounded::<Result<(EncoderId, Vec<u8>), String>>(1);
        let running = Arc::new(AtomicBool::new(true));

        // Der MFT wird im eigenen Faden gebaut und benutzt: COM-Objekte bleiben
        // damit dort, wo sie erzeugt wurden.
        let thread = {
            let running = running.clone();
            let gpu = gpu.clone();
            std::thread::Builder::new()
                .name("clippiboy-encoder".into())
                .spawn(move || {
                    let transform = match build(&gpu, &settings) {
                        Ok(transform) => {
                            let _ = ready_tx
                                .send(Ok((transform.chosen, transform.sequence_header.clone())));
                            transform
                        }
                        Err(err) => {
                            let _ = ready_tx.send(Err(err));
                            return;
                        }
                    };
                    run(transform, settings, jobs_rx, running, on_packet);
                })
                .map_err(|err| format!("Encoder-Faden: {err}"))?
        };

        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok((chosen, sequence_header))) => Ok(Self {
                jobs: jobs_tx,
                running,
                thread: Some(thread),
                chosen,
                sequence_header,
            }),
            Ok(Err(err)) => {
                running.store(false, Ordering::Relaxed);
                let _ = thread.join();
                Err(err)
            }
            Err(_) => {
                running.store(false, Ordering::Relaxed);
                Err("Encoder antwortet nicht".into())
            }
        }
    }

    /// Ein Bild einreichen.
    ///
    /// Wartet kurz, wenn der Encoder im Rückstand ist — siehe [`SUBMIT_WAIT`].
    /// Gibt `false` zurück, wenn das Bild wirklich verloren ging; dann stimmt
    /// die Bildrate der Aufnahme nicht mehr und der Clip wäre gegenüber seinem
    /// Ton verschoben.
    pub fn submit(&self, texture: &ID3D11Texture2D, pts_100ns: i64) -> bool {
        let job = Job {
            texture: SendPtr(texture.clone()),
            pts_100ns,
        };
        match self.jobs.send_timeout(job, SUBMIT_WAIT) {
            Ok(()) => true,
            Err(_) => {
                log::warn!("Encoder kommt nicht nach — ein Bild ist verloren");
                false
            }
        }
    }

    pub fn stop(mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for VideoEncoder {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Die Schleife auf dem Encoder-Faden.
fn run<F>(
    transform: Transform,
    settings: EncoderSettings,
    jobs: crossbeam_channel::Receiver<Job>,
    running: Arc<AtomicBool>,
    mut on_packet: F,
) where
    F: FnMut(EncodedPacket) + Send + 'static,
{
    let duration = 10_000_000 / settings.fps.max(1) as i64;

    match transform.events.clone() {
        // Hardware-Encoder: Er sagt, wann er ein Bild will und wann eines
        // fertig ist.
        Some(events) => {
            // Offene „Ich will ein Bild"-Aufforderungen. Der Zähler ist nötig,
            // weil das Warten auf das nächste Bild sonst eine Aufforderung
            // verschlucken würde — der Encoder schickt sie kein zweites Mal.
            let mut pending_input = 0u32;

            while running.load(Ordering::Relaxed) {
                if pending_input > 0 {
                    match jobs.recv_timeout(Duration::from_millis(100)) {
                        Ok(job) => {
                            if let Err(err) = transform.feed(&job, duration) {
                                log::warn!("{err}");
                            }
                            pending_input -= 1;
                        }
                        // Nichts da — die Aufforderung bleibt bestehen.
                        Err(_) => continue,
                    }
                    continue;
                }

                let event = match unsafe { events.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0)) }
                {
                    Ok(event) => event,
                    Err(_) => break,
                };
                let kind = unsafe { event.GetType() }.unwrap_or(0);
                if kind == METransformNeedInput.0 as u32 {
                    pending_input += 1;
                } else if kind == METransformHaveOutput.0 as u32 {
                    match transform.take_output(duration) {
                        Ok(Some(packet)) => on_packet(packet),
                        Ok(None) => {}
                        Err(err) => log::warn!("{err}"),
                    }
                }
            }
        }
        // Software-Encoder: einreichen, dann abholen, bis nichts mehr kommt.
        None => {
            while running.load(Ordering::Relaxed) {
                let Ok(job) = jobs.recv_timeout(Duration::from_millis(100)) else {
                    continue;
                };
                if let Err(err) = transform.feed(&job, duration) {
                    log::warn!("{err}");
                    continue;
                }
                loop {
                    match transform.take_output(duration) {
                        Ok(Some(packet)) => on_packet(packet),
                        Ok(None) => break,
                        Err(err) => {
                            log::warn!("{err}");
                            break;
                        }
                    }
                }
            }
        }
    }

    // Auslaufen lassen, damit die letzten Bilder noch herauskommen.
    unsafe {
        let _ = transform
            .transform
            .ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);
    }
    while let Ok(Some(packet)) = transform.take_output(duration) {
        on_packet(packet);
    }
    unsafe {
        let _ = transform
            .transform
            .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
    }
}
