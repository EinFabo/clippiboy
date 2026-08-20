//! WASAPI-Aufnahme je Quelle.
//!
//! Drei Quellentypen:
//!   * Eingabegerät  — normale Aufnahme (Mikrofon)
//!   * Ausgabegerät  — Loopback des kompletten Endpunkts
//!   * Anwendung     — Prozess-Loopback über `ActivateAudioInterfaceAsync`
//!     (Windows 10 Build 20348+); damit lässt sich z.B. Discord getrennt vom
//!     Spiel aufnehmen, ohne virtuelle Kabel.
//!
//! Jeder Stream läuft in einem eigenen Thread, schreibt 48 kHz/Stereo/f32 in
//! einen Ringpuffer und aktualisiert seinen Spitzenpegel für die Anzeige.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::audio::ring::SampleRing;
use crate::model::SourceKind;

/// Einheitliches Format aller Quellen nach der Konvertierung.
/// Aktuelle QPC-Zeit in 100-ns-Einheiten — dieselbe Zeitachse, auf der
/// Windows.Graphics.Capture seine Bilder stempelt.
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
        // Erst teilen, dann multiplizieren wäre ungenau; erst multiplizieren
        // liefe bei 64 Bit über. Deshalb getrennt nach ganzen Sekunden und Rest.
        let seconds = counter / frequency;
        let rest = counter % frequency;
        seconds * 10_000_000 + rest * 10_000_000 / frequency
    }
    #[cfg(not(windows))]
    0
}

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;

pub struct StreamHandle {
    pub stop: Arc<AtomicBool>,
    pub ring: Arc<SampleRing>,
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

/// Startet die Aufnahme einer Quelle. Fehler bedeuten: Gerät weg, Prozess
/// beendet oder keine Berechtigung — der Aufrufer meldet das der UI.
pub fn start(kind: &SourceKind, ring: Arc<SampleRing>) -> Result<StreamHandle, String> {
    let stop = Arc::new(AtomicBool::new(false));

    #[cfg(windows)]
    {
        let kind = kind.clone();
        let stop_flag = stop.clone();
        let ring_for_thread = ring.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        let thread = std::thread::Builder::new()
            .name("clippiboy-audio".into())
            .spawn(move || {
                win::run(&kind, stop_flag, ring_for_thread, ready_tx);
            })
            .map_err(|e| e.to_string())?;

        // Auf das Ergebnis der Initialisierung warten, damit Fehler sofort in
        // der UI landen statt still im Thread zu verschwinden.
        match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(StreamHandle {
                stop,
                ring,
                thread: Some(thread),
            }),
            Ok(Err(err)) => Err(err),
            Err(_) => Err("Zeitüberschreitung beim Starten der Audioquelle".into()),
        }
    }

    #[cfg(not(windows))]
    {
        let _ = (kind, &ring, &stop);
        Err("Audioaufnahme ist nur unter Windows verfügbar".into())
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::sync::mpsc::Sender;
    use windows::core::{implement, IUnknown, Interface, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, S_OK, WAIT_OBJECT_0};
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
    /// WAVE_FORMAT_IEEE_FLOAT — im windows-Crate nicht exportiert.
    const FORMAT_FLOAT: u16 = 0x0003;
    /// WAVE_FORMAT_EXTENSIBLE
    const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

    /// Completion-Handler für `ActivateAudioInterfaceAsync` — signalisiert nur
    /// ein Event, die Auswertung passiert im aufrufenden Thread.
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

    /// PROPVARIANT mit VT_BLOB — im windows-Crate nicht direkt konstruierbar,
    /// deshalb layout-kompatibel selbst gebaut (x64: 24 Byte).
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
        /// true = f32-Samples, false = 16-bit PCM
        float: bool,
        bits: u16,
    }

    unsafe fn read_format(ptr: *const WAVEFORMATEX) -> Format {
        let wf = &*ptr;
        // WAVE_FORMAT_EXTENSIBLE (0xFFFE) mit 32 Bit ist praktisch immer float.
        let float = wf.wFormatTag == FORMAT_FLOAT
            || (wf.wFormatTag == FORMAT_EXTENSIBLE && wf.wBitsPerSample == 32);
        Format {
            channels: wf.nChannels as usize,
            sample_rate: wf.nSamplesPerSec,
            float,
            bits: wf.wBitsPerSample,
        }
    }

    /// Rohpuffer in 48 kHz/Stereo/f32 umrechnen.
    fn convert(raw: &[u8], frames: usize, format: &Format, out: &mut Vec<f32>) {
        out.clear();
        let src_channels = format.channels.max(1);

        // Erst auf Stereo bringen.
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

        // Lineare Resampling-Stufe; Geräte laufen fast immer schon auf 48 kHz,
        // das hier ist der Notnagel für 44,1 kHz und Ähnliches.
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

            // Für das virtuelle Gerät gibt es kein Mix-Format — es muss gesetzt
            // werden. Event-Callback ist hier Pflicht.
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
    ) {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }

        let started = match kind {
            SourceKind::InputDevice { device_id } => device_client(device_id, false),
            SourceKind::OutputDevice { device_id } => device_client(device_id, true),
            SourceKind::Process { pid, mode } => process_client(
                *pid,
                matches!(mode, crate::model::ProcessMode::Include),
            ),
        };

        let (client, format) = match started {
            Ok(pair) => pair,
            Err(err) => {
                let _ = ready.send(Err(format!("Audioquelle nicht verfügbar: {err}")));
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
                // Der QPC-Zeitstempel lag hier immer schon bereit und wurde
                // weggeworfen. Er ist dieselbe Uhr wie
                // `Direct3D11CaptureFrame::SystemRelativeTime` — damit fällt
                // die ganze frühere Drift-Korrektur weg.
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
                // Nicht jeder Treiber füllt ihn. Dann selbst ablesen — die Uhr
                // ist dieselbe, nur der Ablesezeitpunkt etwas später.
                let qpc_100ns = if qpc_100ns == 0 {
                    now_100ns()
                } else {
                    qpc_100ns as i64
                };

                if frames > 0 {
                    // AUDCLNT_BUFFERFLAGS_SILENT = 0x2 — Puffer ignorieren und
                    // stattdessen Stille einspeisen, sonst driftet die Spur.
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
