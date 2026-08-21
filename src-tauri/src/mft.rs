//! The H.264 encoder, directly as a Media Foundation Transform.
//!
//! Recording used to run through the WinRT `MediaTranscoder`. That accepts
//! **nothing** beyond a bitrate: no rate control, no GOP distance, no profile,
//! no CABAC. Which is why 40 Mbit/s looked like considerably less, and why the
//! encoder switch in the settings did nothing — Windows decided which MFT ran.
//!
//! Here the MFT is found, configured and fed by hand. It gets NV12 textures
//! straight from the GPU (no readback) and delivers finished H.264 packets that
//! travel into the ring buffer from `buffer.rs`.

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

/// How many frames may wait to be submitted. At 60 fps four slots are a good
/// 66 ms of headroom for a brief stall of the encoder.
const QUEUE_DEPTH: usize = 4;

/// How long the clock waits when the queue is full.
///
/// Waiting rather than dropping: the finished clip is muxed as a raw elementary
/// stream whose timeline comes from the frame rate alone (`-r` in `muxer.rs`).
/// If even one frame is missing from it, the file is shorter than its audio —
/// the picture would run away from the sound. A tick of delay, by contrast,
/// costs nothing: the clock computes its timestamps from a counter and catches
/// up by itself.
const SUBMIT_WAIT: Duration = Duration::from_millis(500);

pub struct EncoderSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub keyframe_seconds: u32,
    pub requested: EncoderId,
}

/// Bring Media Foundation up once per process.
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

/// List all H.264 encoder MFTs, hardware first.
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
        // The array is ours; we cloned the references in it above.
        for index in 0..count as usize {
            let _ = (*array.add(index)).take();
        }
        CoTaskMemFree(Some(array as *const _));
    }
    Ok(found)
}

/// Which hardware encoders really exist on this machine.
///
/// Replaces the earlier guess based on the DXGI vendor id: an NVIDIA card in the
/// machine does not yet mean its encoder MFT is registered.
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

/// Set one property. `false` means the driver does not know it.
///
/// Not every encoder knows every property. Whatever is not accepted stays at its
/// default — that is no reason to let the recording fail. It should still be
/// reported: if a clip looks worse than expected, that is exactly the first
/// question.
fn set_codec_property(codec: &ICodecAPI, name: &str, api: &GUID, value: VARIANT) -> bool {
    unsafe {
        match codec.SetValue(api, &value) {
            Ok(()) => true,
            Err(err) => {
                log::debug!("encoder does not accept '{name}': {err}");
                false
            }
        }
    }
}

fn configure_codec(transform: &IMFTransform, settings: &EncoderSettings) {
    let Ok(codec) = transform.cast::<ICodecAPI>() else {
        log::warn!("encoder offers no ICodecAPI — quality settings stay off");
        return;
    };

    let mean = settings.bitrate_kbps.saturating_mul(1000);
    // Peaks may exceed the mean: on a fast turn in the game one frame needs a
    // multiple of a still one.
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
        // 0 = as fast as possible, 100 = best quality. A replay buffer runs in
        // the background but is not in real-time distress — 70 is the point where
        // NVENC and AMF get noticeably better without losing frames.
        ("quality/speed", &CODECAPI_AVEncCommonQualityVsSpeed, VARIANT::from(70u32)),
        // CABAC instead of CAVLC: around 10 % fewer artefacts at the same bitrate.
        ("CABAC", &CODECAPI_AVEncH264CABACEnable, VARIANT::from(true)),
        // Low latency switches off lookahead and B-frames. For a replay buffer
        // latency is beside the point; quality is not.
        ("low latency off", &CODECAPI_AVLowLatencyMode, VARIANT::from(false)),
    ];

    let rejected: Vec<&str> = wanted
        .into_iter()
        .filter(|(name, api, value)| !set_codec_property(&codec, name, api, value.clone()))
        .map(|(name, _, _)| name)
        .collect();
    if !rejected.is_empty() {
        log::warn!(
            "encoder does not know these settings: {} — they stay at their defaults",
            rejected.join(", ")
        );
    }

    // Accepted does not mean applied. The NVIDIA MFT, for one, acknowledges any
    // keyframe distance with success but caps it at the frame rate — "every 2 s"
    // silently becomes "every second". Exactly those silent deviations were the
    // reason the settings previously did nothing without anyone noticing.
    if let Some(effective) = read_u32(&codec, &CODECAPI_AVEncMPVGOPSize) {
        if effective != gop {
            log::info!(
                "encoder does not honour the keyframe distance: asked for every {} frames, \
                 actually every {effective}",
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
            .map_err(|err| format!("frame size: {err}"))?;
        media
            .SetUINT64(&MF_MT_FRAME_RATE, ((settings.fps.max(1) as u64) << 32) | 1)
            .map_err(|err| format!("Bildrate: {err}"))?;
        media
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)
            .map_err(|err| format!("pixel aspect ratio: {err}"))?;
        media
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|err| format!("Interlace: {err}"))?;

        // Write the colour metadata along, matching what `convert.rs` actually
        // produces. Without them the player guesses, and the same clip looks
        // different in Discord than in an editor.
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
    /// SPS/PPS from the output type. Most encoders send them before every IDR
    /// anyway; if they are missing, the elementary stream is not decodable
    /// without this preamble. A duplicate does no harm; an absence does.
    sequence_header: Vec<u8>,
}

fn build(gpu: &GpuDevice, settings: &EncoderSettings) -> Result<Transform, String> {
    startup()?;
    let candidates = enumerate()?;
    if candidates.is_empty() {
        return Err("no H.264 encoder found".into());
    }

    // The requested encoder first; otherwise the first hardware encoder;
    // otherwise the first one at all (software fallback).
    let pick = candidates
        .iter()
        .position(|(id, _)| *id == Some(settings.requested))
        .or_else(|| candidates.iter().position(|(id, _)| id.is_some()))
        .unwrap_or(0);
    let (chosen_id, activate) = &candidates[pick];
    let chosen = chosen_id.unwrap_or(EncoderId::X264);

    let transform: IMFTransform = unsafe { activate.ActivateObject() }
        .map_err(|err| format!("Encoder starten: {err}"))?;

    // Hardware encoders are asynchronous and have to be unlocked for that first —
    // otherwise `SetOutputType` rejects them.
    let mut is_async = false;
    if let Ok(attributes) = unsafe { transform.GetAttributes() } {
        is_async = unsafe { attributes.GetUINT32(&MF_TRANSFORM_ASYNC) }.unwrap_or(0) == 1;
        if is_async {
            unsafe { attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1) }
                .map_err(|err| format!("Encoder freischalten: {err}"))?;
        }
    }

    // Hand the D3D11 device through — that is the only way the encoder accepts
    // textures instead of demanding a readback through main memory.
    let mut token = 0u32;
    let mut manager: Option<IMFDXGIDeviceManager> = None;
    unsafe {
        MFCreateDXGIDeviceManager(&mut token, &mut manager)
            .map_err(|err| format!("DXGI device manager: {err}"))?;
    }
    let manager = manager.ok_or_else(|| "DXGI device manager missing".to_string())?;
    unsafe {
        manager
            .ResetDevice(&gpu.device, token)
            .map_err(|err| format!("register device: {err}"))?;
        transform
            .ProcessMessage(
                MFT_MESSAGE_SET_D3D_MANAGER,
                manager.as_raw() as usize,
            )
            .map_err(|err| format!("device to encoder: {err}"))?;
    }

    // The order is prescribed: output type before input type.
    let output = media_type_video(MFVideoFormat_H264, settings)?;
    unsafe {
        output
            .SetUINT32(&MF_MT_AVG_BITRATE, settings.bitrate_kbps.saturating_mul(1000))
            .map_err(|err| format!("Bitrate: {err}"))?;
        let _ = output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32);
        // The keyframe distance belongs here, not only on ICodecAPI: the NVIDIA
        // MFT accepts `CODECAPI_AVEncMPVGOPSize` without complaint but goes by
        // this field. Without it there was one keyframe per second no matter what
        // was configured.
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

/// One frame on its way to the encoder.
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

            // Without a length set the encoder takes the buffer for empty.
            if let Ok(two_d) = buffer.cast::<IMF2DBuffer>() {
                if let Ok(len) = two_d.GetContiguousLength() {
                    let _ = buffer.SetCurrentLength(len);
                }
            }

            let sample = MFCreateSample().map_err(|err| format!("Sample: {err}"))?;
            sample
                .AddBuffer(&buffer)
                .map_err(|err| format!("fill sample: {err}"))?;
            sample
                .SetSampleTime(job.pts_100ns)
                .map_err(|err| format!("sample time: {err}"))?;
            let _ = sample.SetSampleDuration(duration_100ns);

            self.transform
                .ProcessInput(0, &sample, 0)
                .map_err(|err| format!("submit frame: {err}"))
        }
    }

    /// Collect a finished packet. `Ok(None)` means there is nothing right now.
    fn take_output(&self, duration_100ns: i64) -> Result<Option<EncodedPacket>, String> {
        unsafe {
            let mut data = MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: std::mem::ManuallyDrop::new(None),
                dwStatus: 0,
                pEvents: std::mem::ManuallyDrop::new(None),
            };

            // Some encoders supply the sample themselves, others expect one.
            if !self.provides_samples {
                let info = self
                    .transform
                    .GetOutputStreamInfo(0)
                    .map_err(|err| format!("Ausgabeinfo: {err}"))?;
                let buffer = MFCreateMemoryBuffer(info.cbSize.max(1))
                    .map_err(|err| format!("Ausgabepuffer: {err}"))?;
                let sample = MFCreateSample().map_err(|err| format!("output sample: {err}"))?;
                sample
                    .AddBuffer(&buffer)
                    .map_err(|err| format!("fill output sample: {err}"))?;
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
                    // The encoder changed its output type (some drivers do this
                    // once at the beginning). Fetch it again and ask once more —
                    // otherwise the stream stalls.
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

/// Handle on the running encoder.
pub struct VideoEncoder {
    jobs: crossbeam_channel::Sender<Job>,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Which encoder it actually turned out to be — so the status display shows
    /// the truth rather than the wish from the config.
    pub chosen: EncoderId,
    /// SPS/PPS that belong in front of the stream when saving.
    pub sequence_header: Vec<u8>,
}

impl VideoEncoder {
    /// `on_packet` runs on the encoder thread and has to be short.
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

        // The MFT is built and used on its own thread: that keeps the COM objects
        // where they were created.
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
                Err("encoder is not responding".into())
            }
        }
    }

    /// Submit one frame.
    ///
    /// Waits briefly if the encoder has fallen behind — see [`SUBMIT_WAIT`].
    /// Returns `false` when the frame really was lost; the recording's frame rate
    /// is then no longer right and the clip would be shifted against its audio.
    pub fn submit(&self, texture: &ID3D11Texture2D, pts_100ns: i64) -> bool {
        let job = Job {
            texture: SendPtr(texture.clone()),
            pts_100ns,
        };
        match self.jobs.send_timeout(job, SUBMIT_WAIT) {
            Ok(()) => true,
            Err(_) => {
                log::warn!("encoder cannot keep up — one frame is lost");
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

/// The loop on the encoder thread.
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
        // Hardware encoder: it says when it wants a frame and when one is
        // finished.
        Some(events) => {
            // Outstanding "I want a frame" requests. The counter is needed
            // because waiting for the next frame would otherwise swallow a request
            // — the encoder does not send it a second time.
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
                        // Nothing there — the request stands.
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
        // Software encoder: submit, then collect until nothing more comes.
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

    // Let it drain so the last frames still come out.
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
