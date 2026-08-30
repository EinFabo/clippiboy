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
        match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(StreamHandle {
                stop,
                ring,
                fallback_clock,
                thread: Some(thread),
            }),
            Ok(Err(err)) => Err(err),
            Err(_) => Err("timed out starting the audio source".into()),
        }
    }

    #[cfg(not(windows))]
    {
        let _ = (kind, &ring, &stop, &fallback_clock);
        Err("audio capture is only available on Windows".into())
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::sync::mpsc::Sender;
    use windows::core::{implement, IUnknown, Interface, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, E_INVALIDARG, HANDLE, S_OK, WAIT_OBJECT_0};
    use windows::Win32::Media::Audio::{
        eMultimedia, eRender, ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
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
        }
    }

    /// Convert a raw buffer to 48 kHz/stereo/f32.
    fn convert(raw: &[u8], frames: usize, format: &Format, out: &mut Vec<f32>) {
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

                if src_channels == 1 {
                    left = sample;
                    right = sample;
                } else if channel == 0 {
                    left = sample;
                } else if channel == 1 {
                    right = sample;
                }
            }
            stereo.push(left);
            stereo.push(right);
        }

        if format.sample_rate == SAMPLE_RATE {
            out.extend_from_slice(&stereo);
            return;
        }

        // A linear resampling step; devices practically always run at 48 kHz
        // already, this is the stopgap for 44.1 kHz and the like.
        let ratio = SAMPLE_RATE as f64 / format.sample_rate as f64;
        let target_frames = (frames as f64 * ratio) as usize;
        for frame in 0..target_frames {
            let src_pos = frame as f64 / ratio;
            let index = src_pos.floor() as usize;
            let frac = (src_pos - index as f64) as f32;
            let next = (index + 1).min(frames.saturating_sub(1));
            for channel in 0..CHANNELS {
                let a = stereo.get(index * CHANNELS + channel).copied().unwrap_or(0.0);
                let b = stereo.get(next * CHANNELS + channel).copied().unwrap_or(0.0);
                out.push(a + (b - a) * frac);
            }
        }
    }

    fn device_client(device_id: &str, loopback: bool) -> windows::core::Result<(IAudioClient, Format)> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let id_wide = wide(device_id);
            let device = if device_id.is_empty() {
                enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?
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

            if WaitForSingleObject(event, 5_000) != WAIT_OBJECT_0 {
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
                },
            ))
        }
    }

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

        let started = match kind {
            SourceKind::InputDevice { device_id } => device_client(device_id, false),
            SourceKind::OutputDevice { device_id, .. } => device_client(device_id, true),
            SourceKind::Process { pid, mode } => process_client(
                *pid,
                matches!(mode, crate::model::ProcessMode::Include),
            ),
            // `resolve` turns this into a `Process` source long before the
            // engine sees it, and drops it while no game is detected. Reaching
            // here means an `apply` path forgot to resolve.
            SourceKind::Game => Err(windows::core::Error::from(E_INVALIDARG)),
        };

        let (client, format) = match started {
            Ok(pair) => pair,
            Err(err) => {
                let _ = ready.send(Err(format!("audio source not available: {err}")));
                return;
            }
        };

        let event = if matches!(kind, SourceKind::Process { .. }) {
            match unsafe { CreateEventW(None, false, false, None) } {
                Ok(handle) => {
                    if let Err(err) = unsafe { client.SetEventHandle(handle) } {
                        let _ = ready.send(Err(format!("SetEventHandle: {err}")));
                        return;
                    }
                    Some(handle)
                }
                Err(err) => {
                    let _ = ready.send(Err(format!("CreateEvent: {err}")));
                    return;
                }
            }
        } else {
            None
        };

        let capture: IAudioCaptureClient = match unsafe { client.GetService() } {
            Ok(service) => service,
            Err(err) => {
                let _ = ready.send(Err(format!("GetService: {err}")));
                return;
            }
        };

        if let Err(err) = unsafe { client.Start() } {
            let _ = ready.send(Err(format!("Start: {err}")));
            return;
        }
        let _ = ready.send(Ok(()));

        let mut converted: Vec<f32> = Vec::with_capacity(4096);
        let frame_bytes = format.channels * (format.bits as usize / 8);

        // See `usable_stamp` — not every device reports usable times.
        let mut trust_qpc = true;

        while !stop.load(Ordering::Relaxed) {
            match event {
                Some(handle) => unsafe {
                    WaitForSingleObject(handle, 200);
                },
                None => std::thread::sleep(std::time::Duration::from_millis(5)),
            }

            loop {
                let available = match unsafe { capture.GetNextPacketSize() } {
                    Ok(frames) => frames,
                    Err(_) => break,
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
                if unsafe {
                    capture.GetBuffer(
                        &mut data,
                        &mut frames,
                        &mut flags,
                        None,
                        Some(&mut qpc_100ns),
                    )
                }
                .is_err()
                {
                    break;
                }
                // The clock is the same, only read a little later.
                let now = now_100ns();
                let stamped = qpc_100ns as i64;
                let trusted = trust_qpc;
                let qpc_100ns = usable_stamp(stamped, now, &mut trust_qpc);
                if trusted && !trust_qpc {
                    fallback_clock.store(true, Ordering::Relaxed);
                    log::warn!(
                        "audio source {kind:?} reports unusable timestamps \
                         ({stamped} instead of roughly {now}) — falling back to \
                         the system clock."
                    );
                }

                if frames > 0 {
                    // AUDCLNT_BUFFERFLAGS_SILENT = 0x2 — ignore the buffer and
                    // feed silence instead, otherwise the track drifts.
                    if flags & 0x2 != 0 {
                        converted.clear();
                        converted.resize(frames as usize * CHANNELS, 0.0);
                    } else {
                        let raw = unsafe {
                            std::slice::from_raw_parts(data, frames as usize * frame_bytes)
                        };
                        convert(raw, frames as usize, &format, &mut converted);
                    }
                    ring.write(&converted, qpc_100ns);
                }

                let _ = unsafe { capture.ReleaseBuffer(frames) };
            }
        }

        let _ = unsafe { client.Stop() };
        if let Some(handle) = event {
            let _ = unsafe { CloseHandle(handle) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
