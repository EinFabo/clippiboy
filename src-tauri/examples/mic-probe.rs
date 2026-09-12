//! Measures how fast an input device's clock runs against QPC, and whether the
//! ring keeps up with it.
//!
//! Written for a microphone whose level meter moved while its track in the
//! clips stayed silent. Before the ring pulled its timeline towards the stamps,
//! a device running fast ran away from the picture without limit, and once the
//! error outgrew the one-second ring the source read as silence. This counts the
//! frames the device really hands over against the time that really passes and
//! reports the difference in parts per million — and how long a ring of one
//! second would have lasted at that rate.
//!
//! Beside that the same device runs through the real capture path into a real
//! ring, and the ring's end is held against the clock. Uncorrected, that figure
//! grows by exactly what the device is ahead; corrected, it stays where it
//! started.
//!
//!     cargo run --example mic-probe -- "{0.0.1.00000000}.{…}" [seconds]

#[cfg(windows)]
fn main() {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use clippiboy_lib::audio::capture::{self, now_100ns, CHANNELS, SAMPLE_RATE};
    use clippiboy_lib::audio::ring::SampleRing;
    use clippiboy_lib::model::SourceKind;
    use windows::core::PCWSTR;
    use windows::Win32::Media::Audio::{
        IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
        AUDCLNT_SHAREMODE_SHARED,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    let Some(device_id) = std::env::args().nth(1) else {
        eprintln!("usage: mic-probe <input device id> [seconds]");
        std::process::exit(2);
    };
    let seconds: u64 = std::env::args()
        .nth(2)
        .and_then(|text| text.parse().ok())
        .unwrap_or(180);

    let ring = Arc::new(SampleRing::new(SAMPLE_RATE, CHANNELS));
    let stream = capture::start(
        &SourceKind::InputDevice {
            device_id: device_id.clone(),
        },
        ring.clone(),
    )
    .unwrap();
    // Where the ring's end sits against the clock, first and latest.
    let ring_offset_ms = || {
        ring.end_100ns()
            .map(|end| (end - now_100ns()) as f64 / 10_000.0)
    };
    let mut ring_base: Option<f64> = None;

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
        let wide: Vec<u16> = device_id.encode_utf16().chain(Some(0)).collect();
        let device = enumerator.GetDevice(PCWSTR(wide.as_ptr())).unwrap();
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None).unwrap();
        let mix = client.GetMixFormat().unwrap();
        let rate = (*mix).nSamplesPerSec as f64;
        client
            .Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 2_000_000, 0, mix, None)
            .unwrap();
        CoTaskMemFree(Some(mix as *const _));
        let capture: IAudioCaptureClient = client.GetService().unwrap();
        client.Start().unwrap();
        println!("{rate} Hz, measuring for {seconds} s");

        // (frames before this block, device stamp, stamp read here)
        let mut first: Option<(u64, i64, i64)> = None;
        let mut frames_total = 0u64;
        let began = Instant::now();
        let mut next_report = Duration::from_secs(10);

        while began.elapsed() < Duration::from_secs(seconds) {
            std::thread::sleep(Duration::from_millis(5));
            while capture.GetNextPacketSize().unwrap() > 0 {
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                let mut qpc = 0u64;
                capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, Some(&mut qpc))
                    .unwrap();
                let now = now_100ns();
                let stamp = if qpc != 0 { qpc as i64 } else { now };
                match first {
                    None => first = Some((frames_total, stamp, now)),
                    Some((base_frames, base_stamp, _)) => {
                        let elapsed_s = (stamp - base_stamp) as f64 / 1e7;
                        if began.elapsed() >= next_report && elapsed_s > 1.0 {
                            let counted_s = (frames_total - base_frames) as f64 / rate;
                            let ppm = (counted_s / elapsed_s - 1.0) * 1e6;
                            let hours = if ppm.abs() > 0.1 { 1e6 / ppm.abs() / 3600.0 } else { f64::INFINITY };
                            // Averaged over a few reads: a single one lands
                            // anywhere within the last block.
                            let ring_now = (0..20)
                                .filter_map(|_| {
                                    std::thread::sleep(Duration::from_millis(1));
                                    ring_offset_ms()
                                })
                                .sum::<f64>()
                                / 20.0;
                            let base = *ring_base.get_or_insert(ring_now);
                            println!(
                                "{elapsed_s:>6.0}s  device {:+8.2} ms ahead  = {ppm:+7.1} ppm  \
                                 -> 1 s ring gone after {hours:.1} h  |  ring moved {:+6.2} ms",
                                (counted_s - elapsed_s) * 1000.0,
                                ring_now - base,
                            );
                            next_report += Duration::from_secs(10);
                        }
                    }
                }
                frames_total += frames as u64;
                let _ = capture.ReleaseBuffer(frames);
            }
        }
        let _ = client.Stop();
    }
    stream.stop();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("mic-probe measures a WASAPI device and only runs on Windows");
}
