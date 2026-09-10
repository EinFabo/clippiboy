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

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::{Interface, GUID, VARIANT};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;

use crate::buffer::{EncodedPacket, VIDEO_TRACK};
use crate::gpu::{GpuDevice, SendPtr};
use crate::model::{EncoderId, RateControl};

/// Media Foundation states a vendor as text — `"VEN_10DE"` — where DXGI states
/// the same number as `0x10DE`. The numbers live in `gpu.rs`, because that is
/// where the adapter is chosen from them; here they are only spelled out.
fn vendor_tag(id: u32) -> String {
    format!("VEN_{id:04X}")
}

/// How many frames may wait to be submitted. At 60 fps four slots are a good
/// 66 ms of headroom for a brief stall of the encoder.
pub const QUEUE_DEPTH: usize = 4;

/// The deepest queue a transform has been caught holding, in frames.
///
/// Measured on a Radeon RX 7600 XT at the default quality preset — see
/// [`low_latency_default`], which exists because of it. It is quoted here for a
/// second reason: every one of those frames is still holding one of
/// [`crate::convert::SLOTS`] NV12 textures, and the encoder reads them where they
/// lie. The rotation must not come round again before the encoder has let go.
pub const ENCODER_HOLD_MAX: usize = 16;

/// A knob the environment may turn.
///
/// These exist for one reason: the machines where the encoder misbehaves are
/// not the machines we can build on. Someone else can set a variable and run
/// the probe again; we cannot ship them a compiler. Every default is what
/// ClippiBoy does without them.
fn env_u32(name: &str, default: u32) -> u32 {
    let Ok(raw) = std::env::var(name) else {
        return default;
    };
    match raw.trim().parse::<u32>() {
        Ok(value) => {
            log::info!("{name}={value} instead of the usual {default}");
            value
        }
        Err(_) => {
            log::warn!("{name}=\"{raw}\" is not a number — staying at {default}");
            default
        }
    }
}

/// A knob with no default of our own — absent means the property is not touched
/// at all, and the driver keeps whatever it would have done.
fn env_opt_u32(name: &str) -> Option<u32> {
    let raw = std::env::var(name).ok()?;
    match raw.trim().parse::<u32>() {
        Ok(value) => Some(value),
        Err(_) => {
            log::warn!("{name}=\"{raw}\" is not a number — left alone");
            None
        }
    }
}

fn env_bool(name: &str, default: bool) -> bool {
    let Ok(raw) = std::env::var(name) else {
        return default;
    };
    let value = matches!(raw.trim(), "1" | "true" | "yes" | "on");
    log::info!("{name}={value} instead of the usual {default}");
    value
}

/// What the encoder costs, measured rather than assumed.
///
/// Until these existed the only signal was one warning line, printed after a
/// frame had already been lost — which says that something went wrong but not
/// how close the encoder had been running to the edge all along. On hardware we
/// cannot see, that difference is the whole diagnosis.
#[derive(Default)]
pub struct EncoderStats {
    /// Frames that could not be handed over straight away.
    pub submit_waited: AtomicU64,
    /// What those waits cost, in microseconds. This is time the pacer thread
    /// stood still — which is exactly what a stutter is made of.
    pub submit_wait_us: AtomicU64,
    /// Packets that came back out.
    pub encoded: AtomicU64,
    /// How long the encoder kept a frame, summed, in microseconds.
    pub encode_us: AtomicU64,
    /// The single worst frame so far.
    pub encode_worst_us: AtomicU64,
    /// Frames handed over and not yet returned.
    pub in_flight: AtomicU64,
}

impl EncoderStats {
    /// Average time a frame spent inside the encoder, in microseconds.
    pub fn encode_mean_us(&self) -> u64 {
        let count = self.encoded.load(Ordering::Relaxed);
        match count {
            0 => 0,
            count => self.encode_us.load(Ordering::Relaxed) / count,
        }
    }
}

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
    /// 1 to 100, the encoder's own default being 70. Only used when the encoder
    /// accepts constant quality; otherwise the bitrate governs.
    pub quality: u32,
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
    [
        (crate::gpu::VENDOR_NVIDIA, EncoderId::Nvenc),
        (crate::gpu::VENDOR_AMD, EncoderId::Amf),
        (crate::gpu::VENDOR_INTEL, EncoderId::Qsv),
    ]
    .into_iter()
    .find(|(id, _)| vendor.contains(&vendor_tag(*id)))
    .map(|(_, encoder)| encoder)
}

/// What the driver calls this transform, for the log.
///
/// Worth having verbatim: "AMD H.264 Hardware MFT Encoder" and the Microsoft
/// software encoder are told apart at a glance, and a report from a machine we
/// cannot see is otherwise guesswork.
fn name_of(activate: &IMFActivate) -> String {
    unsafe {
        let mut len = 0u32;
        let mut buf = [0u16; 256];
        match activate.GetString(&MFT_FRIENDLY_NAME_Attribute, &mut buf, Some(&mut len)) {
            Ok(()) => String::from_utf16_lossy(&buf[..len as usize]),
            Err(_) => "(unnamed)".into(),
        }
    }
}

/// List all H.264 encoder MFTs, hardware first.
/// One H.264 encoder MFT as the registry offers it.
struct Candidate {
    /// `None` for anything without a vendor of its own — the software encoder.
    id: Option<EncoderId>,
    name: String,
    activate: IMFActivate,
}

fn enumerate() -> Result<Vec<Candidate>, String> {
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
                let name = name_of(&activate);
                found.push(Candidate { id, name, activate });
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
    for candidate in list {
        if let Some(id) = candidate.id {
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

/// Set the rate control mode and check that it took.
///
/// `SetValue` answering S_OK is not the same as the encoder doing it — the same
/// trap as the keyframe distance below. So the mode is read back; only a
/// matching answer counts as accepted. An encoder that will not report the mode
/// at all gets the benefit of the doubt, because falling through to the default
/// on a missing getter would cost quality for nothing.
fn set_rate_control(codec: &ICodecAPI, mode: eAVEncCommonRateControlMode) -> bool {
    let wanted = mode.0 as u32;
    if !set_codec_property(
        codec,
        "rate control",
        &CODECAPI_AVEncCommonRateControlMode,
        VARIANT::from(wanted),
    ) {
        return false;
    }
    match read_u32(codec, &CODECAPI_AVEncCommonRateControlMode) {
        Some(effective) => effective == wanted,
        None => true,
    }
}

/// Everything MSDN wants set **before** `SetOutputType`.
///
/// This split is not cosmetic. The documentation on `AVEncCommonQuality` says
/// plainly: "set the property before calling IMFTransform::SetOutputType" — set
/// afterwards it is accepted and then ignored. Every codec property used to be
/// applied after both media types, which is why quality mode could never have
/// worked here. (`AVEncMPVDefaultBPictureCount` carries the same requirement,
/// should B-frames ever follow.)
///
/// Returns what the encoder actually agreed to.
fn configure_rate_control(codec: &ICodecAPI, settings: &EncoderSettings) -> RateControl {
    let mean = settings.bitrate_kbps.saturating_mul(1000);
    // Peaks may exceed the mean: on a fast turn in the game one frame needs a
    // multiple of a still one.
    let peak = mean.saturating_add(mean / 2);
    let quality = settings.quality.clamp(1, 100);

    // Constant quality first. This is the whole point: a bitrate target spends
    // its budget whether the picture needs it or not, so a menu screen cost as
    // much as a firefight. Certified hardware encoders are required to support
    // it, but a driver is still a driver.
    if set_rate_control(codec, eAVEncCommonRateControlMode_Quality) {
        if set_codec_property(
            codec,
            "quality",
            &CODECAPI_AVEncCommonQuality,
            VARIANT::from(quality),
        ) {
            log::info!("encoder runs at constant quality {quality}");
            return RateControl::Quality;
        }
        log::warn!("encoder takes quality mode but not a quality level — falling back");
    }

    // Second best: a bitrate, but with a ceiling that means something.
    // `AVEncCommonMaxBitRate` only applies in this mode — in the unconstrained
    // one below it is decoration, which is exactly what it used to be here.
    if set_rate_control(codec, eAVEncCommonRateControlMode_PeakConstrainedVBR) {
        let mean_ok = set_codec_property(
            codec,
            "bitrate",
            &CODECAPI_AVEncCommonMeanBitRate,
            VARIANT::from(mean),
        );
        let peak_ok = set_codec_property(
            codec,
            "peak bitrate",
            &CODECAPI_AVEncCommonMaxBitRate,
            VARIANT::from(peak),
        );
        if mean_ok && peak_ok {
            log::info!(
                "encoder does not do constant quality — {} kbit/s with a ceiling instead",
                settings.bitrate_kbps
            );
            return RateControl::PeakConstrainedVbr;
        }
    }

    // What every version until now did, whether it wanted to or not.
    let _ = set_rate_control(codec, eAVEncCommonRateControlMode_UnconstrainedVBR);
    let _ = set_codec_property(
        codec,
        "bitrate",
        &CODECAPI_AVEncCommonMeanBitRate,
        VARIANT::from(mean),
    );
    log::warn!(
        "encoder only offers unconstrained VBR — clip size follows the {} kbit/s \
         and nothing else",
        settings.bitrate_kbps
    );
    RateControl::UnconstrainedVbr
}

/// B-frames, but only when the environment asks.
///
/// ClippiBoy has never set this, so whatever the driver defaults to is what
/// runs — and on AMD that is worth being able to change from outside, because
/// B-frames are RDNA2-and-later, cost encoding time, and nobody here owns the
/// hardware to find out how much.
///
/// Set **before** `SetOutputType`: MSDN puts it in the same class as
/// `AVEncCommonQuality`, which is accepted and then ignored afterwards.
fn configure_bframes(codec: &ICodecAPI) -> Option<(u32, bool)> {
    let count = env_opt_u32("CLIPPIBOY_BFRAMES")?;
    let taken = set_codec_property(
        codec,
        "B-frames",
        &CODECAPI_AVEncMPVDefaultBPictureCount,
        VARIANT::from(count),
    );
    Some((count, taken))
}

/// Whether to ask for low latency — and why that is not the same answer for
/// every make of card.
///
/// This used to be off everywhere, and the reasoning was sound as far as it
/// went: a replay buffer does not care about latency, the clip is cut out of a
/// ring minutes later, and switching low latency on costs the lookahead. What
/// the reasoning missed is who pays for that lookahead.
///
/// Measured on a Radeon RX 7600 XT: at the default quality preset the AMD
/// transform holds **sixteen** frames at once and blocks the capture clock for
/// sixteen of every thirty seconds. Not one frame is dropped and the rate stays
/// at 60 — it is a queue, not slow encoding. But AMD run the lookahead on the
/// shaders rather than on the dedicated video block, which is to say on the very
/// part of the card the game is being drawn with. On an empty desktop that costs
/// nothing anyone can see. In a game it comes straight out of the frame budget,
/// and that is what "it lags on AMD" turned out to be. With this switch on, the
/// queue is zero. Intel's transform behaves the same way and gains three times
/// the speed from it.
///
/// NVENC does not need it: its queue stays shallow whatever it is told, so it
/// keeps its lookahead and the compression that buys. Turning it on there could
/// only cost.
///
/// What it costs where it is on is not a worse picture. The encoder is aiming at
/// a **quality**, so without the lookahead it simply spends more bits to reach
/// the same one: clips get a little bigger, and the ring holds a little less of
/// its configured length inside the same memory budget. Against a game that
/// stutters, that is a bargain.
///
/// `CLIPPIBOY_LOW_LATENCY` overrides this either way.
fn low_latency_default(chosen: EncoderId) -> bool {
    match chosen {
        EncoderId::Amf | EncoderId::Qsv => true,
        EncoderId::Nvenc | EncoderId::X264 => false,
    }
}

/// What the encoder made of the settings after the media types stood.
struct CodecReport {
    rejected: Vec<&'static str>,
    gop_wanted: u32,
    /// `None` when the encoder will not say. Anything other than `gop_wanted`
    /// is a silent deviation, and those are the expensive kind.
    gop_effective: Option<u32>,
    quality_vs_speed: u32,
    low_latency: bool,
}

/// The rest, which may be set once the media types stand.
fn configure_codec(
    codec: &ICodecAPI,
    settings: &EncoderSettings,
    chosen: EncoderId,
) -> CodecReport {
    let gop = settings.fps.max(1) * settings.keyframe_seconds.max(1);

    // 0 = as fast as possible, 100 = best quality. A replay buffer runs in the
    // background but is not in real-time distress — 70 is the point where NVENC
    // and AMF get noticeably better without losing frames.
    let quality_vs_speed = env_u32("CLIPPIBOY_QUALITY_VS_SPEED", 70).min(100);
    let low_latency = env_bool("CLIPPIBOY_LOW_LATENCY", low_latency_default(chosen));

    let wanted: Vec<(&'static str, &GUID, VARIANT)> = vec![
        ("Keyframe-Abstand", &CODECAPI_AVEncMPVGOPSize, VARIANT::from(gop)),
        (
            "quality/speed",
            &CODECAPI_AVEncCommonQualityVsSpeed,
            VARIANT::from(quality_vs_speed),
        ),
        // CABAC instead of CAVLC: around 10 % fewer artefacts at the same bitrate.
        ("CABAC", &CODECAPI_AVEncH264CABACEnable, VARIANT::from(true)),
        (
            "low latency",
            &CODECAPI_AVLowLatencyMode,
            VARIANT::from(low_latency),
        ),
    ];

    let rejected: Vec<&'static str> = wanted
        .into_iter()
        .filter(|(name, api, value)| !set_codec_property(codec, name, api, value.clone()))
        .map(|(name, _, _)| name)
        .collect();

    // Accepted does not mean applied. The NVIDIA MFT, for one, acknowledges any
    // keyframe distance with success but caps it at the frame rate — "every 2 s"
    // silently becomes "every second". Exactly those silent deviations were the
    // reason the settings previously did nothing without anyone noticing.
    CodecReport {
        rejected,
        gop_wanted: gop,
        gop_effective: read_u32(codec, &CODECAPI_AVEncMPVGOPSize),
        quality_vs_speed,
        low_latency,
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
    /// What the encoder agreed to do about bitrate — see [`RateControl`].
    rate_control: RateControl,
    /// SPS/PPS from the output type. Most encoders send them before every IDR
    /// anyway; if they are missing, the elementary stream is not decodable
    /// without this preamble. A duplicate does no harm; an absence does.
    sequence_header: Vec<u8>,
}

/// Everything about this encoder, as one record in the log.
///
/// It used to be six `debug!` and `warn!` lines scattered through the setup, and
/// in a release build none of them went anywhere at all — `main.rs` builds a GUI
/// without a console. On a machine we can reach, that was merely inconvenient.
/// On someone else's it meant the only answers we ever got were "it lags".
///
/// So it is one block, at `info`, listing what we would otherwise have to ask
/// for one question at a time: what was wanted, what was found, what actually
/// runs, on which card, and what the driver quietly refused.
#[allow(clippy::too_many_arguments)]
fn profile(
    gpu: &GpuDevice,
    settings: &EncoderSettings,
    candidates: &[Candidate],
    pick: usize,
    chosen: EncoderId,
    is_async: bool,
    provides_samples: bool,
    rate_control: RateControl,
    report: Option<&CodecReport>,
    bframes: Option<(u32, bool)>,
) {
    let mut lines = vec!["encoder profile".to_string()];

    lines.push(format!("  requested        {:?}", settings.requested));
    lines.push(format!(
        "  running          {chosen:?}{}",
        if chosen == settings.requested {
            ""
        } else {
            "   <-- NOT what was asked for"
        }
    ));
    lines.push(format!(
        "  transform        {} ({})",
        candidates[pick].name,
        match candidates[pick].id {
            Some(id) => format!("{id:?}"),
            None => "no vendor, so software".into(),
        }
    ));
    lines.push(format!(
        "  adapter          {}",
        match &gpu.adapter {
            Some(info) => info.to_string(),
            None => "DXGI default (nothing was requested)".into(),
        }
    ));
    lines.push(format!(
        "  picture          {}x{} @ {} fps, {} kbit/s, quality {}",
        settings.width, settings.height, settings.fps, settings.bitrate_kbps, settings.quality
    ));
    lines.push(format!(
        "  rate control     {rate_control:?}{}",
        match rate_control {
            RateControl::Quality => "",
            _ => "   <-- not constant quality",
        }
    ));
    lines.push(format!(
        "  model            {}, samples {}",
        if is_async {
            "asynchronous (hardware)"
        } else {
            "synchronous (software)"
        },
        if provides_samples {
            "from the encoder"
        } else {
            "ours to provide"
        }
    ));

    match report {
        Some(report) => {
            lines.push(format!(
                "  keyframes        every {} frames, encoder says {}",
                report.gop_wanted,
                match report.gop_effective {
                    Some(value) => value.to_string(),
                    None => "(will not say)".into(),
                }
            ));
            lines.push(format!(
                "  quality/speed    {}, low latency {}, B-frames {}",
                report.quality_vs_speed,
                report.low_latency,
                match bframes {
                    Some((count, true)) => format!("{count}"),
                    Some((count, false)) => {
                        format!("{count} asked for and REFUSED, so the driver's own")
                    }
                    None => "not asked, so the driver's own".into(),
                }
            ));
            lines.push(format!(
                "  refused          {}",
                if report.rejected.is_empty() {
                    "nothing".to_string()
                } else {
                    report.rejected.join(", ")
                }
            ));
        }
        None => lines.push("  refused          no ICodecAPI at all — nothing could be set".into()),
    }

    lines.push(format!("  candidates       {}", candidates.len()));
    for (index, candidate) in candidates.iter().enumerate() {
        lines.push(format!(
            "    [{index}]{} {} — {}",
            if index == pick { " *" } else { "  " },
            candidate.name,
            match candidate.id {
                Some(id) => format!("{id:?}"),
                None => "software".into(),
            }
        ));
    }

    log::info!("{}", lines.join("\n"));
}

fn build(gpu: &GpuDevice, settings: &EncoderSettings) -> Result<Transform, String> {
    startup()?;
    let candidates = enumerate()?;
    if candidates.is_empty() {
        return Err("no H.264 encoder found".into());
    }

    // The requested encoder first; otherwise the first hardware encoder;
    // otherwise the first one at all (software fallback).
    // The software encoder is the one candidate no vendor id can name — it has
    // none, and matching on `Some(requested)` therefore never found it. Asking
    // for software got you the first hardware encoder instead, without a word.
    //
    // Selecting it properly turns out not to help: it answers
    // `MFT_MESSAGE_SET_D3D_MANAGER` with E_NOTIMPL, and it would not know what
    // to do with a texture either. Recording on the CPU needs the picture read
    // back out of the GPU first, and there is no such path — the whole point of
    // this pipeline is that the frame never travels through main memory.
    //
    // So it is still passed over. The difference is that it now says so: a
    // setting that silently does something else is how the encoder switch came
    // to do nothing at all for so long.
    // `encode::for_recording` has already turned a request for software into
    // whatever hardware is there, so `X264` only survives this far on a machine
    // that has none at all. It then lands on the software transform below and
    // fails at the D3D manager, which is the truth: this pipeline cannot record
    // without a hardware encoder.
    let pick = match settings.requested {
        EncoderId::X264 => None,
        wanted => candidates.iter().position(|c| c.id == Some(wanted)),
    }
    .or_else(|| candidates.iter().position(|c| c.id.is_some()))
    .unwrap_or(0);
    let chosen = candidates[pick].id.unwrap_or(EncoderId::X264);

    let transform: IMFTransform = unsafe { candidates[pick].activate.ActivateObject() }
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
            // The software transform answers this with E_NOTIMPL, and a raw
            // HRESULT is no way to tell somebody their machine cannot do the
            // thing at all.
            .map_err(|err| match chosen {
                EncoderId::X264 => "no hardware H.264 encoder on this machine — ClippiBoy \
                    encodes straight off the graphics card, and the Windows software \
                    encoder cannot take its pictures"
                    .to_string(),
                _ => format!("device to encoder: {err}"),
            })?;
    }

    // Rate control has to be settled **before** the output type: MSDN says of
    // `AVEncCommonQuality` that it must be set before `SetOutputType`, and an
    // encoder simply ignores it afterwards. Everything else waits until the
    // media types stand.
    let codec = transform.cast::<ICodecAPI>().ok();
    let rate_control = match &codec {
        Some(codec) => configure_rate_control(codec, settings),
        None => {
            log::warn!("encoder offers no ICodecAPI — quality settings stay off");
            RateControl::UnconstrainedVbr
        }
    };
    let bframes = codec.as_ref().and_then(configure_bframes);

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

    let report = codec
        .as_ref()
        .map(|codec| configure_codec(codec, settings, chosen));

    let provides_samples = unsafe { transform.GetOutputStreamInfo(0) }
        .map(|info| info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0)
        .unwrap_or(false);

    let events = if is_async {
        transform.cast::<IMFMediaEventGenerator>().ok()
    } else {
        None
    };

    profile(
        gpu,
        settings,
        &candidates,
        pick,
        chosen,
        is_async,
        provides_samples,
        rate_control,
        report.as_ref(),
        bframes,
    );

    // Asked for here and asked for again later. At this point the transform has
    // been configured but never told to stream, and most hardware H.264 MFTs do
    // not fill `MF_MT_MPEG_SEQUENCE_HEADER` in until they have run — so this
    // usually comes back empty, and used to stay that way for the life of the
    // recording. See `catch_up_on_header`.
    let sequence_header = read_sequence_header(&transform);

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
        rate_control,
        sequence_header,
    })
}

/// One frame on its way to the encoder.
struct Job {
    texture: SendPtr<ID3D11Texture2D>,
    pts_100ns: i64,
}

/// SPS/PPS out of the encoder's current output type, empty when it will not say.
fn read_sequence_header(transform: &IMFTransform) -> Vec<u8> {
    unsafe {
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
    }
}

/// How many packets to keep asking over before giving the header up as absent.
/// Two seconds at 60 fps — an encoder that has not produced it by then never
/// will.
const HEADER_TRIES: u32 = 120;

/// Ask again for the sequence header, now that the encoder has really run.
///
/// The preamble is read once during setup, between `SetInputType` and
/// `MFT_MESSAGE_NOTIFY_BEGIN_STREAMING`. That is too early for most hardware
/// transforms: the blob is not filled in until the encoder has been started, so
/// what setup got was an empty vector — and `muxer::build` then prepended
/// nothing to the elementary stream. Where the encoder writes SPS/PPS in front of
/// every IDR itself, which most do, that goes unnoticed. Where it does not, the
/// clip is a file full of slices no decoder can start on: black, all the way
/// through, with nothing anywhere saying why.
fn catch_up_on_header<H: FnMut(Vec<u8>)>(
    transform: &Transform,
    pending: &mut bool,
    tries: &mut u32,
    on_header: &mut H,
) {
    if !*pending {
        return;
    }
    *tries += 1;
    let header = read_sequence_header(&transform.transform);
    if !header.is_empty() {
        *pending = false;
        log::debug!("encoder handed over {} bytes of SPS/PPS", header.len());
        on_header(header);
    } else if *tries >= HEADER_TRIES {
        *pending = false;
        log::warn!(
            "the encoder will not hand over its SPS/PPS — saved clips depend on the \
             stream carrying them in front of every keyframe itself"
        );
    }
}

/// Does this packet open a group of pictures?
///
/// `MFSampleExtension_CleanPoint` is the encoder's own word for it, and where it
/// is set that is the answer. Not every transform sets it, though, and the
/// `unwrap_or(0)` that reads it turns a missing attribute into "not a keyframe"
/// — for every packet there will ever be. The consequences are out of all
/// proportion to the omission: [`crate::buffer::ReplayBuffer::trim`] cuts only
/// between keyframes and so stops cutting at all, the ring grows past its budget,
/// `buffered_seconds` reports 0.0 for a buffer that is filling, and every save is
/// refused for want of a starting point. All of it silent.
///
/// So where the attribute says nothing, the stream is asked. In Annex-B a NAL
/// begins after `00 00 01`, and the low five bits of the byte after that are its
/// type: 5 is an IDR slice, 7 an SPS — which only ever stands in front of one.
fn opens_a_gop(data: &[u8]) -> bool {
    let mut zeros = 0usize;
    for (index, byte) in data.iter().enumerate() {
        match byte {
            0 => zeros += 1,
            1 if zeros >= 2 => {
                if let Some(header) = data.get(index + 1) {
                    if matches!(header & 0x1f, 5 | 7) {
                        return true;
                    }
                }
                zeros = 0;
            }
            _ => zeros = 0,
        }
    }
    false
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

            let flagged = sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0;
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

            // The attribute first, the bitstream where it is missing — see
            // `opens_a_gop`. Asking the stream costs a scan of a packet that has
            // just been copied anyway, and it is the difference between a buffer
            // that can be cut and one that cannot.
            let keyframe = flagged || opens_a_gop(&bytes);

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
    /// What the encoder really does about bitrate — the wish from the config is
    /// not the answer, see [`configure_rate_control`].
    pub rate_control: RateControl,
    /// SPS/PPS that belong in front of the stream when saving.
    pub sequence_header: Vec<u8>,
    /// How deep the job channel really is — `CLIPPIBOY_QUEUE_DEPTH` may have
    /// moved it. Frames sitting in there hold NV12 slots just as the ones inside
    /// the transform do, so the rotation has to count them.
    pub queue_depth: usize,
    /// What it is costing while it runs — see [`EncoderStats`].
    pub stats: Arc<EncoderStats>,
}

impl VideoEncoder {
    /// `on_packet` runs on the encoder thread and has to be short.
    ///
    /// `on_header` runs there too, at most once, as soon as the encoder will part
    /// with its SPS/PPS — which is generally not before it has produced its first
    /// packet. Whatever [`Self::sequence_header`] held from setup is superseded
    /// by it; see `catch_up_on_header`.
    pub fn start<F, H>(
        gpu: Arc<GpuDevice>,
        settings: EncoderSettings,
        on_packet: F,
        on_header: H,
    ) -> Result<Self, String>
    where
        F: FnMut(EncodedPacket) + Send + 'static,
        H: FnMut(Vec<u8>) + Send + 'static,
    {
        let depth = env_u32("CLIPPIBOY_QUEUE_DEPTH", QUEUE_DEPTH as u32).max(1) as usize;
        let (jobs_tx, jobs_rx) = crossbeam_channel::bounded::<Job>(depth);
        let stats = Arc::new(EncoderStats::default());
        let (ready_tx, ready_rx) =
            crossbeam_channel::bounded::<Result<(EncoderId, RateControl, Vec<u8>), String>>(1);
        let running = Arc::new(AtomicBool::new(true));

        // The MFT is built and used on its own thread: that keeps the COM objects
        // where they were created.
        let thread = {
            let running = running.clone();
            let gpu = gpu.clone();
            let stats = stats.clone();
            std::thread::Builder::new()
                .name("clippiboy-encoder".into())
                .spawn(move || {
                    let transform = match build(&gpu, &settings) {
                        Ok(transform) => {
                            let _ = ready_tx.send(Ok((
                                transform.chosen,
                                transform.rate_control,
                                transform.sequence_header.clone(),
                            )));
                            transform
                        }
                        Err(err) => {
                            let _ = ready_tx.send(Err(err));
                            return;
                        }
                    };
                    run(transform, settings, jobs_rx, running, stats, on_packet, on_header);
                })
                .map_err(|err| format!("Encoder-Faden: {err}"))?
        };

        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok((chosen, rate_control, sequence_header))) => Ok(Self {
                jobs: jobs_tx,
                running,
                thread: Some(thread),
                chosen,
                rate_control,
                sequence_header,
                queue_depth: depth,
                stats,
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

        // The normal case first, so the frame that goes straight through pays
        // for neither a clock reading nor a counter.
        let job = match self.jobs.try_send(job) {
            Ok(()) => return true,
            Err(crossbeam_channel::TrySendError::Full(job)) => job,
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                log::warn!("encoder has stopped — one frame is lost");
                return false;
            }
        };

        // From here the pacer thread is standing still, and that is precisely
        // the thing worth counting: a lost frame gets a warning line, but time
        // spent waiting shows up as a stutter long before anything is lost.
        let since = Instant::now();
        let sent = self.jobs.send_timeout(job, SUBMIT_WAIT);
        let waited = since.elapsed();
        self.stats.submit_waited.fetch_add(1, Ordering::Relaxed);
        self.stats
            .submit_wait_us
            .fetch_add(waited.as_micros() as u64, Ordering::Relaxed);

        match sent {
            Ok(()) => true,
            Err(_) => {
                log::warn!(
                    "encoder cannot keep up — one frame is lost after waiting {} ms",
                    waited.as_millis()
                );
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
fn run<F, H>(
    transform: Transform,
    settings: EncoderSettings,
    jobs: crossbeam_channel::Receiver<Job>,
    running: Arc<AtomicBool>,
    stats: Arc<EncoderStats>,
    mut on_packet: F,
    mut on_header: H,
) where
    F: FnMut(EncodedPacket) + Send + 'static,
    H: FnMut(Vec<u8>) + Send + 'static,
{
    let duration = 10_000_000 / settings.fps.max(1) as i64;

    // Empty from setup means the encoder had not started yet and there is still
    // one to fetch. Non-empty means this encoder is one of the few that answers
    // straight away, and there is nothing to chase.
    let mut header_pending = transform.sequence_header.is_empty();
    let mut header_tries = 0u32;

    /// Note down how long the encoder kept one frame.
    fn took(stats: &EncoderStats, since: Instant, in_flight: usize) {
        let micros = since.elapsed().as_micros() as u64;
        stats.encoded.fetch_add(1, Ordering::Relaxed);
        stats.encode_us.fetch_add(micros, Ordering::Relaxed);
        stats.encode_worst_us.fetch_max(micros, Ordering::Relaxed);
        stats.in_flight.store(in_flight as u64, Ordering::Relaxed);
    }

    match transform.events.clone() {
        // Hardware encoder: it says when it wants a frame and when one is
        // finished.
        Some(events) => {
            // Outstanding "I want a frame" requests. The counter is needed
            // because waiting for the next frame would otherwise swallow a request
            // — the encoder does not send it a second time.
            let mut pending_input = 0u32;
            // When each frame went in, so the wait for it to come back out can
            // be measured. Paired first in, first out: with B-frames the
            // encoder may return pictures in a different order than it took
            // them, so a single pairing can be a frame or two out. The counts
            // match regardless, which is what keeps the average honest.
            let mut sent: VecDeque<Instant> = VecDeque::new();
            let mut finished = false;

            while running.load(Ordering::Relaxed) && !finished {
                // Did the encoder say anything at all this time round?
                let mut handled = false;

                // Everything the encoder has to say, right now, without waiting.
                //
                // This is what used to be skipped. The old loop read
                // "if pending_input > 0 { wait for a frame; continue; }" and so
                // never reached `GetEvent` while a request was outstanding —
                // finished packets sat in the event queue untouched. An encoder
                // whose own output queue fills stops asking for input; the job
                // channel then backs up and `submit` stalls the pacer half a
                // second at a time. NVENC hides it behind queues deep enough to
                // absorb the ping-pong. Not every encoder has them.
                loop {
                    let event = match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                        Ok(event) => event,
                        Err(err) if err.code() == MF_E_NO_EVENTS_AVAILABLE => break,
                        Err(err) => {
                            if err.code() != MF_E_SHUTDOWN {
                                log::warn!("encoder event queue: {err}");
                            }
                            finished = true;
                            break;
                        }
                    };
                    handled = true;

                    let kind = unsafe { event.GetType() }.unwrap_or(0);
                    if kind == METransformNeedInput.0 as u32 {
                        pending_input += 1;
                    } else if kind == METransformHaveOutput.0 as u32 {
                        match transform.take_output(duration) {
                            Ok(Some(packet)) => {
                                if let Some(since) = sent.pop_front() {
                                    took(&stats, since, sent.len());
                                }
                                on_packet(packet);
                                catch_up_on_header(
                                    &transform,
                                    &mut header_pending,
                                    &mut header_tries,
                                    &mut on_header,
                                );
                            }
                            Ok(None) => {}
                            Err(err) => log::warn!("{err}"),
                        }
                    } else if kind == METransformDrainComplete.0 as u32 {
                        // Previously ignored, so the loop kept turning over an
                        // encoder that had already said it was done. This is the
                        // one an MFT sends; `MEEndOfStream` belongs to media
                        // sources and never arrives here.
                        finished = true;
                        break;
                    }
                }
                if finished {
                    break;
                }

                if pending_input > 0 {
                    // A real wait, on the one thing that is missing. Kept short
                    // so a packet announced meanwhile is not left lying about.
                    match jobs.recv_timeout(Duration::from_millis(2)) {
                        Ok(job) => {
                            match transform.feed(&job, duration) {
                                Ok(()) => {
                                    sent.push_back(Instant::now());
                                    // Written on the way in as well as on the way
                                    // out. Only `took` used to touch it, so while
                                    // the queue was growing — the one time the
                                    // number matters — it stood still at whatever
                                    // the last packet left behind.
                                    stats
                                        .in_flight
                                        .store(sent.len() as u64, Ordering::Relaxed);
                                }
                                Err(err) => log::warn!("{err}"),
                            }
                            pending_input -= 1;
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    }
                } else if !handled {
                    // The encoder is chewing and wants nothing. A moment's pause
                    // beats spinning on an empty event queue.
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
        // Software encoder: submit, then collect until nothing more comes.
        None => {
            while running.load(Ordering::Relaxed) {
                let Ok(job) = jobs.recv_timeout(Duration::from_millis(100)) else {
                    continue;
                };
                let since = Instant::now();
                if let Err(err) = transform.feed(&job, duration) {
                    log::warn!("{err}");
                    continue;
                }
                loop {
                    match transform.take_output(duration) {
                        Ok(Some(packet)) => {
                            took(&stats, since, 0);
                            on_packet(packet);
                            catch_up_on_header(
                                &transform,
                                &mut header_pending,
                                &mut header_tries,
                                &mut on_header,
                            );
                        }
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
    match &transform.events {
        // An asynchronous transform announces the packets it still holds the
        // same way it announced all the others, and says when it has finished.
        // Asking it for output unprompted — which is what used to happen here —
        // gets "need more input" on the first try, and the tail of the recording
        // goes with it.
        Some(events) => {
            let until = Instant::now() + Duration::from_secs(2);
            while Instant::now() < until {
                let event = match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                    Ok(event) => event,
                    // Nothing yet — it is still working on what it has.
                    Err(err) if err.code() == MF_E_NO_EVENTS_AVAILABLE => {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    // Anything else means there will be no more events, and
                    // waiting out the two seconds would only delay the stop.
                    Err(_) => break,
                };
                let kind = unsafe { event.GetType() }.unwrap_or(0);
                if kind == METransformHaveOutput.0 as u32 {
                    match transform.take_output(duration) {
                        Ok(Some(packet)) => on_packet(packet),
                        Ok(None) => {}
                        Err(err) => {
                            log::warn!("{err}");
                            break;
                        }
                    }
                } else if kind == METransformDrainComplete.0 as u32 {
                    break;
                }
            }
        }
        // Synchronous: it hands everything back on the spot.
        None => {
            while let Ok(Some(packet)) = transform.take_output(duration) {
                on_packet(packet);
            }
        }
    }
    unsafe {
        let _ = transform
            .transform
            .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One NAL with an Annex-B start code in front of it.
    fn nal(kind: u8, long_start_code: bool) -> Vec<u8> {
        let mut out = if long_start_code {
            vec![0, 0, 0, 1]
        } else {
            vec![0, 0, 1]
        };
        // The two high bits are `forbidden_zero` and `nal_ref_idc`; only the low
        // five are the type, so a realistic byte has them set.
        out.push(0x60 | kind);
        out.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        out
    }

    #[test]
    fn an_idr_opens_a_gop() {
        assert!(opens_a_gop(&nal(5, true)));
        assert!(opens_a_gop(&nal(5, false)), "three-byte start codes count too");
    }

    /// An SPS never stands anywhere but in front of a keyframe, and encoders that
    /// write one inline put it before the IDR — so finding it is enough.
    #[test]
    fn a_parameter_set_opens_a_gop() {
        assert!(opens_a_gop(&nal(7, true)));
    }

    #[test]
    fn an_ordinary_slice_does_not() {
        // Type 1: a non-IDR slice, which is every frame between keyframes.
        assert!(!opens_a_gop(&nal(1, true)));
    }

    /// The real shape of a keyframe packet: parameter sets, then the picture.
    #[test]
    fn a_keyframe_packet_is_found_past_its_leading_nals() {
        let mut packet = nal(9, true); // access unit delimiter
        packet.extend(nal(8, true)); // PPS
        packet.extend(nal(5, true)); // the IDR itself
        assert!(opens_a_gop(&packet));
    }

    /// A run of zeros before the start code is legal padding and must not throw
    /// the scan off.
    #[test]
    fn leading_padding_does_not_hide_the_start_code() {
        let mut packet = vec![0u8; 8];
        packet.extend(nal(5, false));
        assert!(opens_a_gop(&packet));
    }

    #[test]
    fn nothing_to_read_is_not_a_keyframe() {
        assert!(!opens_a_gop(&[]));
        // A start code with the packet ending right after it: no type byte to
        // look at, and no panic for looking.
        assert!(!opens_a_gop(&[0, 0, 0, 1]));
        assert!(!opens_a_gop(&[0, 0, 1]));
    }
}
