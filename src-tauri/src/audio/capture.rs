//! WASAPI capture, one per source.
//!
//! Three source types:
//!   * input device   — normal capture (microphone)
//!   * output device  — loopback of the whole endpoint
//!   * application    — process loopback via `ActivateAudioInterfaceAsync`
//!     (Windows 11, build 20348+ — the API does not exist on 10); this is what
//!     lets Discord, say, be recorded separately from the game without virtual
//!     cables.
//!
//! Every stream runs on a thread of its own, writes 48 kHz/stereo/f32 into a
//! ring buffer and updates its peak level for the display.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::audio::ring::SampleRing;
use crate::model::SourceKind;

/// The common format all sources share after conversion.
/// Current QPC time in 100 ns units — the same timeline
/// Windows.Graphics.Capture stamps its frames on.
pub fn now_100ns() -> i64 {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Performance::{
            QueryPerformanceCounter, QueryPerformanceFrequency,
        };
        let mut frequency = 0i64;
        let mut counter = 0i64;
        if QueryPerformanceFrequency(&mut frequency).is_err() || frequency == 0 {
            return 0;
        }
        let _ = QueryPerformanceCounter(&mut counter);
        // Dividing first and then multiplying would be imprecise; multiplying
        // first would overflow at 64 bits. So whole seconds and remainder are
        // handled separately.
        let seconds = counter / frequency;
        let rest = counter % frequency;
        seconds * 10_000_000 + rest * 10_000_000 / frequency
    }
    #[cfg(not(windows))]
    0
}

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;

/// From what distance to the system clock a timestamp can no longer come from
/// that clock. Two seconds is generous — real QPC stamps are off by
/// milliseconds.
const IMPLAUSIBLE_100NS: i64 = 2 * 10_000_000;

/// Which timestamp applies to this block?
///
/// `stamped` is what the device reported, `now` the time read here. Not every
/// driver really reports QPC in `pu64QPCPosition`; some write a position there
/// that starts at zero. The ring would then sit on an entirely different timeline
/// than the picture, and
/// [`crate::audio::ring::SampleRing::read_window`] would find nothing in any
/// window — the meter would keep bouncing while the track in the clip stayed
/// silent.
///
/// `trust` remembers the verdict on the device. Once `false`, it stays that way:
/// jumping back and forth between two timelines would be worse than using the
/// coarser of the two throughout.
pub fn usable_stamp(stamped: i64, now: i64, trust: &mut bool) -> i64 {
    if *trust && stamped != 0 && (stamped - now).abs() > IMPLAUSIBLE_100NS {
        *trust = false;
    }
    // A 0 only means "not filled in" — that happens often and is harmless, the
    // time read here comes from the same clock.
    if *trust && stamped != 0 {
        stamped
    } else {
        now
    }
}

/// `AUDCLNT_BUFFERFLAGS_SILENT`: the packet holds no sound, only its length.
pub const BUFFER_FLAG_SILENT: u32 = 0x2;
/// `AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR`: the packet's stamp is not reliable.
pub const BUFFER_FLAG_TIMESTAMP_ERROR: u32 = 0x4;

/// When was this packet captured?
///
/// A trusted, non-zero, unflagged stamp is used as it is. In every other case
/// the packet is placed by its queue: it was captured `queued_frames` of the
/// device before `now`, because the reader drains the queue oldest packet first.
/// Using `now` itself put the first packet of a burst after a stall a whole
/// stall late, which left a fake dropout and then a source that sat late until
/// the ring pulled it back. The distrust rule of [`usable_stamp`] runs only on
/// stamps that are unflagged and non-zero. The queue comes from
/// [`burst_times`].
pub fn capture_time(
    stamped: i64,
    flags: u32,
    now: i64,
    queued_frames: u32,
    device_rate: u32,
    trust: &mut bool,
) -> i64 {
    let usable = flags & BUFFER_FLAG_TIMESTAMP_ERROR == 0 && stamped != 0;
    if usable {
        let chosen = usable_stamp(stamped, now, trust);
        if *trust {
            return chosen;
        }
    }
    now - queued_frames as i64 * 10_000_000 / device_rate.max(1) as i64
}

/// One packet of a burst the reader drained in one go — what [`burst_times`]
/// needs of it.
#[derive(Debug, Clone, Copy)]
pub struct Drained {
    /// The device's stamp, `pu64QPCPosition`.
    pub stamped: i64,
    pub flags: u32,
    /// In the device's own frames.
    pub frames: u32,
}

/// When was each packet of a drained burst captured? Into `times`, one per
/// packet.
///
/// A packet with a usable stamp keeps it ([`capture_time`]). Every other one is
/// placed by what came after it: the reader drains the queue oldest packet
/// first until nothing is left, so packet k was captured the frames of k and
/// of every later packet in the burst before `now`, read when the last one
/// came out. `GetCurrentPadding` cannot say this — on a capture stream it
/// counts only the next packet, which left a burst after a stall stamped near
/// `now`.
///
/// Returns the stamp that turned the device's trust off, if one did.
pub fn burst_times(
    burst: &[Drained],
    now: i64,
    device_rate: u32,
    trust: &mut bool,
    times: &mut Vec<i64>,
) -> Option<i64> {
    times.clear();
    let mut distrusted = None;
    let mut to_end: u64 = burst.iter().map(|packet| packet.frames as u64).sum();
    for packet in burst {
        let trusted = *trust;
        let queued = u32::try_from(to_end).unwrap_or(u32::MAX);
        times.push(capture_time(packet.stamped, packet.flags, now, queued, device_rate, trust));
        if trusted && !*trust {
            distrusted = Some(packet.stamped);
        }
        to_end -= packet.frames as u64;
    }
    distrusted
}

pub struct StreamHandle {
    pub stop: Arc<AtomicBool>,
    pub ring: Arc<SampleRing>,
    /// Set as soon as the device has reported unusable timestamps and this stream
    /// has fallen back to the system clock.
    pub fallback_clock: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl StreamHandle {
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// How long `start` waits for the device to open. The process-tap activation
/// waits `ACTIVATION_WAIT_MS`, so it ends inside this.
const START_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
#[cfg(windows)]
const ACTIVATION_WAIT_MS: u32 = 4_000;

/// Waits for the capture thread's first answer. On a timeout or a reported
/// error the thread is told to stop, so it cannot pump on with the device open
/// after the caller has given up.
fn await_ready(
    waiting: &std::sync::mpsc::Receiver<Result<(), String>>,
    timeout: std::time::Duration,
    stop: &AtomicBool,
) -> Result<(), String> {
    let answer = match waiting.recv_timeout(timeout) {
        Ok(answer) => answer,
        Err(_) => Err("timed out starting the audio source".into()),
    };
    if answer.is_err() {
        stop.store(true, Ordering::Relaxed);
    }
    answer
}

/// Starts capturing one source. An error means the device is gone, the process
/// has ended or there is no permission — the caller reports that to the UI.
pub fn start(kind: &SourceKind, ring: Arc<SampleRing>) -> Result<StreamHandle, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let fallback_clock = Arc::new(AtomicBool::new(false));

    #[cfg(windows)]
    {
        let kind = kind.clone();
        let stop_flag = stop.clone();
        let ring_for_thread = ring.clone();
        let fallback_for_thread = fallback_clock.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        let thread = std::thread::Builder::new()
            .name("clippiboy-audio".into())
            .spawn(move || {
                win::run(&kind, stop_flag, ring_for_thread, ready_tx, fallback_for_thread);
            })
            .map_err(|e| e.to_string())?;

        // Wait for the result of initialization so errors land in the UI right
        // away instead of vanishing quietly on the thread.
        await_ready(&ready_rx, START_WAIT, &stop)?;
        Ok(StreamHandle {
            stop,
            ring,
            fallback_clock,
            thread: Some(thread),
        })
    }

    #[cfg(not(windows))]
    {
        let _ = (kind, &ring, &stop, &fallback_clock);
        Err("audio capture is only available on Windows".into())
    }
}

/// How much of one source channel goes to the left and to the right.
///
/// Keeping only the first two channels, as before, was right for stereo and
/// dropped everything else — on a 5.1 or 7.1 device (virtual surround
/// headsets, home cinema) that is the centre channel, where games put their
/// dialogue. The layouts Windows uses for those devices all start
/// front left, front right, centre, LFE, and then their surround pairs
/// (`KSAUDIO_SPEAKER_5POINT1`, `…_5POINT1_SURROUND`, `…_7POINT1_SURROUND`);
/// centre and surrounds go in at -3 dB as in the usual ITU stereo downmix, the
/// LFE stays out. Quad has no centre: its back pair follows the fronts. Other
/// odd counts keep the old rule, their layouts do not agree on where the
/// centre sits.
///
/// Scaled so that front and centre together never go past full scale on
/// their side (-4.6 dB on 5.1 and 7.1, nothing on quad). Scaling for the
/// surrounds as well, as ffmpeg does, made plain stereo content on a virtual
/// 7.1 headset 10 dB quieter than every other track; what the surrounds add on
/// top is bent back by [`soft_clip`] instead. Stereo and mono stay as they are.
#[cfg_attr(not(windows), allow(dead_code))]
fn stereo_shares(channels: usize) -> Vec<(f32, f32)> {
    let shares: Vec<(f32, f32)> = (0..channels).map(|channel| stereo_share(channels, channel)).collect();
    let front = |channel: usize| channel < 2 || (channels >= 6 && channel == 2);
    let side = |pick: fn(&(f32, f32)) -> f32| {
        shares.iter().enumerate().filter(|(channel, _)| front(*channel)).map(|(_, share)| pick(share)).sum::<f32>().max(1.0)
    };
    let (left, right) = (side(|share| share.0), side(|share| share.1));
    shares.into_iter().map(|(l, r)| (l / left, r / right)).collect()
}

/// Peaks above the knee are bent towards full scale instead of being cut off
/// hard; below it nothing changes. Only for folded-down surround, where the
/// surrounds can add up past full scale.
#[cfg_attr(not(windows), allow(dead_code))]
fn soft_clip(sample: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let level = sample.abs();
    if level <= KNEE {
        return sample;
    }
    (KNEE + (1.0 - KNEE) * ((level - KNEE) / (1.0 - KNEE)).tanh()).copysign(sample)
}

/// One channel's share, before [`stereo_shares`] scales them.
#[cfg_attr(not(windows), allow(dead_code))]
fn stereo_share(channels: usize, channel: usize) -> (f32, f32) {
    const MINUS_3_DB: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let surround = |index: usize| {
        if index % 2 == 0 {
            (MINUS_3_DB, 0.0)
        } else {
            (0.0, MINUS_3_DB)
        }
    };
    match (channels, channel) {
        (1, _) => (1.0, 1.0),
        (_, 0) => (1.0, 0.0),
        (_, 1) => (0.0, 1.0),
        (4, extra) => surround(extra),
        (6.., 2) => (MINUS_3_DB, MINUS_3_DB),
        (6.., 3) => (0.0, 0.0),
        (6.., extra) => surround(extra),
        _ => (0.0, 0.0),
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use crate::audio::resample::Resampler;
    use std::sync::mpsc::Sender;
    use windows::core::{implement, IUnknown, Interface, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, E_INVALIDARG, HANDLE, S_OK, WAIT_OBJECT_0};
    use windows::Win32::Media::Audio::{
        eCapture, eMultimedia, eRender, ActivateAudioInterfaceAsync,
        IActivateAudioInterfaceAsyncOperation,
        IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
        IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
        AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
        AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
        AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
        PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
        PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        WAVEFORMATEX,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};

    const REFTIMES_PER_MS: i64 = 10_000;
    /// WAVE_FORMAT_IEEE_FLOAT — not exported by the windows crate.
    const FORMAT_FLOAT: u16 = 0x0003;
    /// WAVE_FORMAT_EXTENSIBLE
    const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

    /// Completion handler for `ActivateAudioInterfaceAsync` — only signals an
    /// event; the evaluation happens on the calling thread.
    #[implement(IActivateAudioInterfaceCompletionHandler)]
    struct ActivationHandler {
        event: HANDLE,
    }

    impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationHandler_Impl {
        fn ActivateCompleted(
            &self,
            _operation: Option<&IActivateAudioInterfaceAsyncOperation>,
        ) -> windows::core::Result<()> {
            unsafe {
                let _ = SetEvent(self.event);
            }
            Ok(())
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// PROPVARIANT with VT_BLOB — not directly constructible in the windows
    /// crate, so built by hand layout-compatibly (x64: 24 bytes).
    #[repr(C)]
    struct BlobPropVariant {
        vt: u16,
        _r1: u16,
        _r2: u16,
        _r3: u16,
        cb_size: u32,
        _pad: u32,
        blob_data: *mut u8,
    }
    const VT_BLOB: u16 = 65;

    fn float_format() -> WAVEFORMATEX {
        WAVEFORMATEX {
            wFormatTag: FORMAT_FLOAT,
            nChannels: CHANNELS as u16,
            nSamplesPerSec: SAMPLE_RATE,
            nAvgBytesPerSec: SAMPLE_RATE * (CHANNELS as u32) * 4,
            nBlockAlign: (CHANNELS * 4) as u16,
            wBitsPerSample: 32,
            cbSize: 0,
        }
    }

    struct Format {
        channels: usize,
        sample_rate: u32,
        /// true = f32 samples, false = 16-bit PCM
        float: bool,
        bits: u16,
        /// Each channel's share of left and right, worked out once per
        /// stream rather than for every packet.
        shares: Vec<(f32, f32)>,
    }

    unsafe fn read_format(ptr: *const WAVEFORMATEX) -> Format {
        let wf = &*ptr;
        // WAVE_FORMAT_EXTENSIBLE (0xFFFE) at 32 bits is practically always float.
        let float = wf.wFormatTag == FORMAT_FLOAT
            || (wf.wFormatTag == FORMAT_EXTENSIBLE && wf.wBitsPerSample == 32);
        Format {
            channels: wf.nChannels as usize,
            sample_rate: wf.nSamplesPerSec,
            float,
            bits: wf.wBitsPerSample,
            shares: super::stereo_shares((wf.nChannels as usize).max(1)),
        }
    }

    /// Convert a raw buffer to 48 kHz/stereo/f32.
    fn convert(
        raw: &[u8],
        frames: usize,
        format: &Format,
        resampler: &mut Resampler,
        out: &mut Vec<f32>,
    ) {
        out.clear();
        let src_channels = format.channels.max(1);

        // Bring it to stereo first.
        let mut stereo: Vec<f32> = Vec::with_capacity(frames * CHANNELS);
        for frame in 0..frames {
            let mut left = 0.0f32;
            let mut right = 0.0f32;
            for channel in 0..src_channels {
                let index = frame * src_channels + channel;
                let sample = if format.float {
                    let offset = index * 4;
                    if offset + 4 > raw.len() {
                        0.0
                    } else {
                        f32::from_le_bytes([
                            raw[offset],
                            raw[offset + 1],
                            raw[offset + 2],
                            raw[offset + 3],
                        ])
                    }
                } else if format.bits == 16 {
                    let offset = index * 2;
                    if offset + 2 > raw.len() {
                        0.0
                    } else {
                        i16::from_le_bytes([raw[offset], raw[offset + 1]]) as f32 / 32768.0
                    }
                } else {
                    0.0
                };

                let (to_left, to_right) = format.shares[channel];
                left += sample * to_left;
                right += sample * to_right;
            }
            if src_channels > 2 {
                left = super::soft_clip(left);
                right = super::soft_clip(right);
            }
            stereo.push(left);
            stereo.push(right);
        }

        resampler.process(&stereo, out);
    }

    fn device_client(device_id: &str, loopback: bool) -> windows::core::Result<(IAudioClient, Format)> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let id_wide = wide(device_id);
            let device = if device_id.is_empty() {
                // The default device of the right *direction*. Taken as eRender
                // regardless, a microphone with no id opened the default
                // speakers and recorded nothing — not reachable from the UI
                // today, but a trap laid for whoever adds a way.
                let flow = if loopback { eRender } else { eCapture };
                enumerator.GetDefaultAudioEndpoint(flow, eMultimedia)?
            } else {
                enumerator.GetDevice(PCWSTR(id_wide.as_ptr()))?
            };
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let mix = client.GetMixFormat()?;
            let format = read_format(mix);

            let flags = if loopback {
                AUDCLNT_STREAMFLAGS_LOOPBACK
            } else {
                0
            };
            let result = client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                flags,
                200 * REFTIMES_PER_MS,
                0,
                mix,
                None,
            );
            CoTaskMemFree(Some(mix as *const _));
            result?;
            Ok((client, format))
        }
    }

    fn process_client(pid: u32, include: bool) -> windows::core::Result<(IAudioClient, Format)> {
        unsafe {
            let event = CreateEventW(None, false, false, None)?;
            let handler: IActivateAudioInterfaceCompletionHandler =
                ActivationHandler { event }.into();

            let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
                ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
                Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                    ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                        TargetProcessId: pid,
                        ProcessLoopbackMode: if include {
                            PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE
                        } else {
                            PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE
                        },
                    },
                },
            };
            let prop = BlobPropVariant {
                vt: VT_BLOB,
                _r1: 0,
                _r2: 0,
                _r3: 0,
                cb_size: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                _pad: 0,
                blob_data: &mut params as *mut _ as *mut u8,
            };

            let operation: IActivateAudioInterfaceAsyncOperation = ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(&prop as *const _ as *const _),
                &handler,
            )?;

            if WaitForSingleObject(event, ACTIVATION_WAIT_MS) != WAIT_OBJECT_0 {
                let _ = CloseHandle(event);
                return Err(windows::core::Error::from_win32());
            }
            let _ = CloseHandle(event);

            let mut activate_result = S_OK;
            let mut interface: Option<IUnknown> = None;
            operation.GetActivateResult(&mut activate_result, &mut interface)?;
            activate_result.ok()?;

            let client: IAudioClient = interface
                .ok_or_else(windows::core::Error::from_win32)?
                .cast()?;

            // There is no mix format for the virtual device — it has to be set.
            // An event callback is mandatory here.
            let format = float_format();
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                200 * REFTIMES_PER_MS,
                0,
                &format,
                None,
            )?;

            Ok((
                client,
                Format {
                    channels: CHANNELS,
                    sample_rate: SAMPLE_RATE,
                    float: true,
                    bits: 32,
                    shares: super::stereo_shares(CHANNELS),
                },
            ))
        }
    }

    /// How a go at a source ended.
    enum Attempt {
        /// The engine took the source down. Nothing to do.
        Stopped,
        /// It never came up.
        Unavailable(String),
        /// It ran and then stopped delivering.
        ///
        /// This is the one that used to be invisible. WASAPI does not announce a
        /// device that has gone; it simply starts failing, and every failure
        /// looked from here like "nothing right now".
        Lost(String),
    }

    /// How long to wait before building a lost source up again, and how long at
    /// most. A box that has just been unplugged or a driver that is resetting is
    /// not back a millisecond later, and a source that fails the moment it is
    /// opened must not spin a core.
    const FIRST_RETRY: std::time::Duration = std::time::Duration::from_millis(300);
    const LAST_RETRY: std::time::Duration = std::time::Duration::from_secs(5);

    /// Run a source for as long as the engine wants it.
    ///
    /// A stream that dies is built up again. Until this existed, the thread went
    /// on turning its rounds against a dead device and wrote nothing into the
    /// ring; the pipeline dutifully filled the gap with counted silence, and what
    /// came out was a clip with a full-length audio track of pure zeros. No error
    /// anywhere, because both failure paths in the pump below read as "no packet
    /// waiting". It took a restart of the whole app to get sound back — and the
    /// recordings made in between are not recoverable, there was never anything
    /// in them.
    ///
    /// A source that never came up in the first place is a different matter: that
    /// one is reported through `ready` and left to the engine, exactly as before.
    /// Rebuilding it here would only hide it.
    pub fn run(
        kind: &SourceKind,
        stop: Arc<AtomicBool>,
        ring: Arc<SampleRing>,
        ready: Sender<Result<(), String>>,
        fallback_clock: Arc<AtomicBool>,
    ) {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }

        let mut ready = Some(ready);
        let mut wait = FIRST_RETRY;

        while !stop.load(Ordering::Relaxed) {
            let first = ready.is_some();
            match attempt(kind, &stop, &ring, &fallback_clock, ready.take()) {
                Attempt::Stopped => return,
                Attempt::Unavailable(err) => {
                    if first {
                        return;
                    }
                    log::warn!(
                        "audio source {kind:?} cannot be opened again ({err}) — next try in {wait:?}"
                    );
                    wait = (wait * 2).min(LAST_RETRY);
                }
                Attempt::Lost(err) => {
                    log::warn!(
                        "audio source {kind:?} stopped delivering ({err}) — building it up again"
                    );
                    // It ran once, so the next go is worth making promptly.
                    wait = FIRST_RETRY;
                }
            }
            std::thread::sleep(wait);
        }
    }

    /// One go: open the source, pump it until it stops or breaks.
    fn attempt(
        kind: &SourceKind,
        stop: &Arc<AtomicBool>,
        ring: &Arc<SampleRing>,
        fallback_clock: &Arc<AtomicBool>,
        ready: Option<Sender<Result<(), String>>>,
    ) -> Attempt {
        let started = match kind {
            SourceKind::InputDevice { device_id } => device_client(device_id, false),
            SourceKind::OutputDevice { device_id, .. } => device_client(device_id, true),
            SourceKind::Process { pid, mode, .. } => process_client(
                *pid,
                matches!(mode, crate::model::ProcessMode::Include),
            ),
            // `resolve` turns these into `Process` sources long before the engine
            // sees them, and drops them while there is nothing to bind to.
            // Reaching here means an `apply` path forgot to resolve.
            SourceKind::Game | SourceKind::Leftovers => {
                Err(windows::core::Error::from(E_INVALIDARG))
            }
        };

        let (client, format) = match started {
            Ok(pair) => pair,
            Err(err) => {
                if let Some(ready) = ready {
                    let _ = ready.send(Err(format!("audio source not available: {err}")));
                }
                return Attempt::Unavailable(err.to_string());
            }
        };

        let event = if matches!(kind, SourceKind::Process { .. }) {
            match unsafe { CreateEventW(None, false, false, None) } {
                Ok(handle) => {
                    if let Err(err) = unsafe { client.SetEventHandle(handle) } {
                        if let Some(ready) = ready {
                            let _ = ready.send(Err(format!("SetEventHandle: {err}")));
                        }
                        return Attempt::Unavailable(err.to_string());
                    }
                    Some(handle)
                }
                Err(err) => {
                    if let Some(ready) = ready {
                        let _ = ready.send(Err(format!("CreateEvent: {err}")));
                    }
                    return Attempt::Unavailable(err.to_string());
                }
            }
        } else {
            None
        };

        let capture: IAudioCaptureClient = match unsafe { client.GetService() } {
            Ok(service) => service,
            Err(err) => {
                if let Some(ready) = ready {
                    let _ = ready.send(Err(format!("GetService: {err}")));
                }
                return Attempt::Unavailable(err.to_string());
            }
        };

        if let Err(err) = unsafe { client.Start() } {
            if let Some(ready) = ready {
                let _ = ready.send(Err(format!("Start: {err}")));
            }
            return Attempt::Unavailable(err.to_string());
        }
        if let Some(ready) = ready {
            if ready.send(Ok(())).is_err() {
                // `start` has given up (timed out): close the device again.
                let _ = unsafe { client.Stop() };
                return Attempt::Stopped;
            }
        }

        let mut converted: Vec<f32> = Vec::with_capacity(4096);
        // One per attempt: its phase belongs to this stream and format.
        let mut resampler = Resampler::new(format.sample_rate);
        let frame_bytes = format.channels * (format.bits as usize / 8);

        // See `usable_stamp` — not every device reports usable times.
        let mut trust_qpc = true;

        // One burst: every packet that is waiting when the reader wakes, drained
        // before any of it is placed (`burst_times`). Converted in order, so the
        // resampler's phase runs on as before.
        let mut burst: Vec<Drained> = Vec::new();
        let mut burst_samples: Vec<f32> = Vec::with_capacity(4096);
        let mut burst_ends: Vec<usize> = Vec::new();
        let mut times: Vec<i64> = Vec::new();

        let mut outcome = Attempt::Stopped;
        while !stop.load(Ordering::Relaxed) {
            match event {
                Some(handle) => unsafe {
                    WaitForSingleObject(handle, 200);
                },
                None => std::thread::sleep(std::time::Duration::from_millis(5)),
            }

            burst.clear();
            burst_samples.clear();
            burst_ends.clear();
            let mut now = 0i64;
            let mut lost = None;
            loop {
                // Not "nothing waiting" — this call only fails when the stream
                // itself is in trouble, and a device that has gone fails it every
                // time from here on. Reading it as an empty poll is what let a
                // dead source record ninety seconds of silence.
                let available = match unsafe { capture.GetNextPacketSize() } {
                    Ok(frames) => frames,
                    Err(err) => {
                        lost = Some(err.to_string());
                        break;
                    }
                };
                if available == 0 {
                    break;
                }

                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                // The QPC timestamp was always available here and used to be
                // thrown away. It is the same clock as
                // `Direct3D11CaptureFrame::SystemRelativeTime` — which is what
                // makes the whole earlier drift correction unnecessary.
                let mut qpc_100ns = 0u64;
                // Same again: an empty buffer comes back as a success code, so
                // a failure here is the device and not the moment.
                if let Err(err) = unsafe {
                    capture.GetBuffer(
                        &mut data,
                        &mut frames,
                        &mut flags,
                        None,
                        Some(&mut qpc_100ns),
                    )
                } {
                    lost = Some(err.to_string());
                    break;
                }
                // The clock is the same, only read a little later. The last
                // packet's reading is the burst's end.
                now = now_100ns();

                if frames > 0 {
                    // BUFFER_FLAG_SILENT — ignore the buffer and
                    // feed silence instead, otherwise the track drifts.
                    if flags & BUFFER_FLAG_SILENT != 0 {
                        converted.clear();
                        resampler.silence(frames as usize, &mut converted);
                    } else {
                        let raw = unsafe {
                            std::slice::from_raw_parts(data, frames as usize * frame_bytes)
                        };
                        convert(raw, frames as usize, &format, &mut resampler, &mut converted);
                    }
                    burst_samples.extend_from_slice(&converted);
                }
                burst_ends.push(burst_samples.len());
                burst.push(Drained { stamped: qpc_100ns as i64, flags, frames });

                let _ = unsafe { capture.ReleaseBuffer(frames) };
            }

            // What was drained before a failure is still written.
            if !burst.is_empty() {
                let distrusted =
                    burst_times(&burst, now, format.sample_rate, &mut trust_qpc, &mut times);
                if let Some(stamped) = distrusted {
                    fallback_clock.store(true, Ordering::Relaxed);
                    log::warn!(
                        "audio source {kind:?} reports unusable timestamps \
                         ({stamped} instead of roughly {now}) — falling back to \
                         the system clock."
                    );
                }
                let mut from = 0;
                for ((packet, time), end) in burst.iter().zip(&times).zip(&burst_ends) {
                    if packet.frames > 0 {
                        ring.write(&burst_samples[from..*end], *time);
                    }
                    from = *end;
                }
            }
            if let Some(err) = lost {
                outcome = Attempt::Lost(err);
                break;
            }
        }

        let _ = unsafe { client.Stop() };
        if let Some(handle) = event {
            let _ = unsafe { CloseHandle(handle) };
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_that_times_out_tells_its_thread_to_stop() {
        let (_ready, waiting) = std::sync::mpsc::channel::<Result<(), String>>();
        let stop = AtomicBool::new(false);
        assert!(await_ready(&waiting, std::time::Duration::from_millis(20), &stop).is_err());
        assert!(stop.load(Ordering::Relaxed), "left unset, the thread pumps on with the device open");
    }

    #[test]
    fn a_start_that_answers_in_time_keeps_its_thread() {
        let (ready, waiting) = std::sync::mpsc::channel::<Result<(), String>>();
        ready.send(Ok(())).unwrap();
        let stop = AtomicBool::new(false);
        assert!(await_ready(&waiting, std::time::Duration::from_millis(20), &stop).is_ok());
        assert!(!stop.load(Ordering::Relaxed));
    }

    #[test]
    fn surround_folds_into_stereo_with_its_centre() {
        // Stereo and mono stay exactly as they were.
        assert_eq!(stereo_share(2, 0), (1.0, 0.0));
        assert_eq!(stereo_share(2, 1), (0.0, 1.0));
        assert_eq!(stereo_share(1, 0), (1.0, 1.0));
        // 5.1 and 7.1: the dialogue in the centre reaches both sides.
        for channels in [6, 8] {
            let (l, r) = stereo_share(channels, 2);
            assert!(l > 0.7 && r > 0.7, "{channels} channels: centre {l}/{r}");
            assert_eq!(stereo_share(channels, 3), (0.0, 0.0), "LFE stays out");
        }
        // Surround left to the left, surround right to the right.
        assert!(stereo_share(8, 6).0 > 0.0 && stereo_share(8, 6).1 == 0.0);
        assert!(stereo_share(8, 7).1 > 0.0 && stereo_share(8, 7).0 == 0.0);
        assert!(stereo_share(4, 2).0 > 0.0 && stereo_share(4, 3).1 > 0.0);
    }

    #[test]
    fn a_surround_downmix_keeps_front_and_centre_at_full_scale() {
        // Stereo and mono untouched.
        assert_eq!(stereo_shares(2), vec![(1.0, 0.0), (0.0, 1.0)]);
        assert_eq!(stereo_shares(1), vec![(1.0, 1.0)]);
        // Quad has no centre: its front stays at full level.
        assert_eq!(stereo_shares(4)[0], (1.0, 0.0));
        for channels in [6, 8] {
            let shares = stereo_shares(channels);
            // Front and centre add up to exactly full scale on each side …
            assert!((shares[0].0 + shares[2].0 - 1.0).abs() < 1e-6, "{channels}: {shares:?}");
            assert!((shares[1].1 + shares[2].1 - 1.0).abs() < 1e-6, "{channels}: {shares:?}");
            // … so plain stereo content loses only 4.6 dB, not 10.
            let db = 20.0 * shares[0].0.log10();
            assert!((-4.7..-4.5).contains(&db), "{channels} channels: {db} dB");
            // The balance stays: centre at -3 dB against the front.
            assert!((shares[2].0 / shares[0].0 - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        }
    }

    #[test]
    fn soft_clip_leaves_ordinary_levels_alone_and_never_reaches_past_full_scale() {
        for sample in [0.0, 0.3, -0.5, 0.8, -0.8] {
            assert_eq!(soft_clip(sample), sample);
        }
        let mut last = 0.8;
        for step in 1..400 {
            let out = soft_clip(0.8 + step as f32 * 0.01);
            assert!(out <= 1.0 && out >= last, "{out} after {last}");
            last = out;
        }
        assert_eq!(soft_clip(-3.0), -soft_clip(3.0));
        // No kink at the knee: just above it the curve still rises at about 1.
        assert!((soft_clip(0.801) - 0.801).abs() < 1e-4);
    }

    /// Roughly a week of uptime — that is how large real QPC values are.
    const NOW: i64 = 7 * 24 * 3600 * 10_000_000;

    #[test]
    fn a_sane_timestamp_is_taken_as_it_is() {
        let mut trust = true;
        // 5 ms off is the normal case.
        assert_eq!(usable_stamp(NOW - 50_000, NOW, &mut trust), NOW - 50_000);
        assert!(trust);
    }

    #[test]
    fn an_unfilled_timestamp_falls_back_without_losing_trust() {
        let mut trust = true;
        assert_eq!(usable_stamp(0, NOW, &mut trust), NOW);
        assert!(trust, "a 0 only means \"not filled in\"");
        // After that a real value counts again.
        assert_eq!(usable_stamp(NOW, NOW, &mut trust), NOW);
    }

    /// The case that really happens: a driver reports a position that starts at
    /// zero. Without this check the ring would sit on a timeline of its own and
    /// the track would arrive in the clip as silence.
    #[test]
    fn a_stream_relative_timestamp_switches_to_the_system_clock() {
        let mut trust = true;
        assert_eq!(usable_stamp(10_000, NOW, &mut trust), NOW);
        assert!(!trust, "the device has to count as unreliable");
    }

    /// Once distrusted it stays that way — otherwise the blocks would jump back
    /// and forth between two timelines.
    #[test]
    fn distrust_is_permanent() {
        let mut trust = true;
        usable_stamp(10_000, NOW, &mut trust);
        let later = NOW + 10_000_000;
        assert_eq!(usable_stamp(later, later, &mut trust), later);
        assert!(!trust);
    }

    /// AUD-2: an empty stamp used to be the read-out time, 10 ms or a whole stall late.
    #[test]
    fn a_missing_stamp_is_estimated_from_what_is_still_queued() {
        let mut trust = true;
        assert_eq!(capture_time(0, 0, NOW, 4_800, 48_000, &mut trust), NOW - 1_000_000);
        assert!(trust);
    }

    /// AUD-2: a stamp the device flags as wrong is not used, and costs no trust.
    #[test]
    fn a_stamp_flagged_as_wrong_is_not_used() {
        let mut trust = true;
        let flagged =
            capture_time(NOW + 3_000_000, BUFFER_FLAG_TIMESTAMP_ERROR, NOW, 480, 48_000, &mut trust);
        assert_eq!(flagged, NOW - 100_000);
        assert!(trust);
    }

    /// AUD-2: a good stamp is still taken as it is.
    #[test]
    fn a_good_stamp_is_still_taken_as_it_is() {
        let mut trust = true;
        assert_eq!(capture_time(NOW - 50_000, 0, NOW, 480, 48_000, &mut trust), NOW - 50_000);
    }

    /// AUD-2: on the fallback clock the stamp is ignored and the queue decides.
    #[test]
    fn a_distrusted_device_is_placed_by_its_queue() {
        let mut trust = false;
        assert_eq!(capture_time(NOW - 50_000, 0, NOW, 441, 44_100, &mut trust), NOW - 100_000);
    }

    /// AUD-2: on the fallback clock, a stalled reader stamped the first queued
    /// packet "now": 150 ms of fake dropout, then the source sat late for over a
    /// minute. The reader drains each burst whole and places it by
    /// [`burst_times`].
    #[test]
    fn a_stalled_reader_on_the_fallback_clock_does_not_drag_the_source_late() {
        let ring = SampleRing::new(48_000, 2);
        let block = vec![0.5f32; 480 * CHANNELS];
        let mut trust = false; // a device on the fallback clock
        let mut times = Vec::new();
        let mut k = 0i64; // packet k covers [k*10 ms, (k+1)*10 ms)
        while k < 600 {
            // Read 2 ms after a packet completes. At 5.2 s the reader stalls and
            // then drains the 16 packets that piled up.
            let burst: i64 = if k == 520 { 16 } else { 1 };
            let now = (k + burst) * 100_000 + 20_000;
            let drained = vec![Drained { stamped: 0, flags: 0, frames: 480 }; burst as usize];
            burst_times(&drained, now, 48_000, &mut trust, &mut times);
            for time in &times {
                ring.write(&block, *time);
                k += 1;
            }
        }
        let end = ring.end_100ns().unwrap();
        assert!((end - 60_020_000).abs() < 10_000, "the source ends {} ms off", (end - 60_020_000) / 10_000);
        let mut out = vec![0.0f32; 480 * CHANNELS];
        ring.read_window(52_500_000, &mut out);
        assert!(out.iter().all(|s| *s == 0.5), "a fake dropout was filled in after the stall");
    }

    /// AUD-2, final review: a drained burst is placed backwards from `now` by the
    /// frames from each packet to the burst's end, whatever their sizes; packets
    /// with a usable stamp in the same burst keep it.
    #[test]
    fn a_drained_burst_is_placed_back_from_its_end() {
        let mut trust = true;
        let mut times = Vec::new();
        let burst = [
            Drained { stamped: 0, flags: 0, frames: 441 },
            Drained { stamped: NOW - 150_000, flags: 0, frames: 441 },
            Drained { stamped: NOW + 3_000_000, flags: BUFFER_FLAG_TIMESTAMP_ERROR, frames: 882 },
            Drained { stamped: 0, flags: BUFFER_FLAG_SILENT, frames: 441 },
        ];
        assert_eq!(burst_times(&burst, NOW, 44_100, &mut trust, &mut times), None);
        assert_eq!(times, vec![NOW - 500_000, NOW - 150_000, NOW - 300_000, NOW - 100_000]);
        assert!(trust, "a flagged or empty stamp says nothing about the device");
    }

    /// A stamp from another timeline turns the trust off for the rest of the
    /// burst, and the caller learns which one it was.
    #[test]
    fn a_burst_reports_the_stamp_that_cost_the_trust() {
        let mut trust = true;
        let mut times = Vec::new();
        let burst = [
            Drained { stamped: NOW - 200_000, flags: 0, frames: 480 },
            Drained { stamped: 5_000, flags: 0, frames: 480 },
            Drained { stamped: NOW - 100_000, flags: 0, frames: 480 },
        ];
        assert_eq!(burst_times(&burst, NOW, 48_000, &mut trust, &mut times), Some(5_000));
        assert!(!trust);
        assert_eq!(times, vec![NOW - 200_000, NOW - 200_000, NOW - 100_000]);
    }
}
