//! Cameras only DirectShow knows — OBS Virtual Camera is one — read through
//! ffmpeg's dshow input: NV12 frames on its stdout, stamped on arrival.

use super::{CameraDevice, Error};

/// The id prefix of a camera read through ffmpeg.
pub const PREFIX: &str = "dshow:";

/// The video devices' names in `ffmpeg -list_devices true -f dshow` output.
pub fn video_devices(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .filter_map(|line| {
            let rest = line.split_once("] \"").map(|(_, rest)| rest)?;
            let (name, kinds) = rest.rsplit_once("\" (")?;
            kinds.trim_end_matches(')').split(", ").any(|k| k == "video").then(|| name.to_string())
        })
        .collect()
}

/// Our own virtual camera is never a source: it would show itself.
pub fn without_our_camera(list: Vec<CameraDevice>) -> Vec<CameraDevice> {
    list.into_iter().filter(|c| !c.name.starts_with(vcam_protocol::FRIENDLY_NAME)).collect()
}

/// The DirectShow cameras Media Foundation does not list, as devices with a
/// `dshow:` id — never ClippiBoy Camera, never a name ffmpeg would misread.
pub fn extras(mf: &[CameraDevice], names: Vec<String>) -> Vec<CameraDevice> {
    let devices = names
        .into_iter()
        .filter(|name| !mf.iter().any(|c| c.name.eq_ignore_ascii_case(name)))
        .map(|name| CameraDevice { id: format!("{PREFIX}{name}"), name })
        .filter(|c| name_of(&c.id).is_some());
    without_our_camera(devices.collect())
}

/// The device name in a `dshow:` id. `"` and `:` are refused: ffmpeg's
/// `video=…:audio=…` syntax splits on the colon.
pub fn name_of(id: &str) -> Option<&str> {
    id.strip_prefix(PREFIX).filter(|name| !name.is_empty() && !name.contains(['"', ':']))
}

/// Watches ffmpeg's log for the size of what it writes to stdout.
#[derive(Default)]
pub struct SizeWatch {
    in_output: bool,
}

impl SizeWatch {
    pub fn feed(&mut self, line: &str) -> Option<(u32, u32)> {
        if line.starts_with("Output #0") {
            self.in_output = true;
            return None;
        }
        if !self.in_output || !line.contains("Video:") {
            return None;
        }
        line.split(',').find_map(|part| {
            let word = part.split_whitespace().next()?;
            let (w, h) = word.split_once('x')?;
            let (w, h) = (w.parse::<u32>().ok()?, h.parse::<u32>().ok()?);
            let usable = (2..=4096).contains(&w) && (2..=4096).contains(&h) && w % 2 == 0 && h % 2 == 0;
            usable.then_some((w, h))
        })
    }
}

/// What ffmpeg's last words say about the camera.
pub fn error_from(stderr_tail: &str) -> Error {
    if stderr_tail.contains("Could not run graph") || stderr_tail.contains("already in use") {
        Error::Busy { by: None }
    } else if stderr_tail.contains("Could not find video device") {
        Error::Gone
    } else {
        Error::Other(stderr_tail.trim().to_string())
    }
}

/// Up to 1080p as it comes; larger (an OBS canvas at 1440p) is scaled down,
/// or ClippiBoy Camera's slots would refuse every frame of it.
pub fn arguments(name: &str) -> Vec<String> {
    let fit = format!(
        "scale=w='min(iw,{})':h='min(ih,{})':force_original_aspect_ratio=decrease:force_divisible_by=2",
        vcam_protocol::MAX_WIDTH,
        vcam_protocol::MAX_HEIGHT
    );
    [
        "-hide_banner",
        "-nostdin",
        "-nostats",
        "-f",
        "dshow",
        "-rtbufsize",
        "64M",
        "-i",
        &format!("video={name}"),
        "-an",
        "-vf",
        &fit,
        "-f",
        "rawvideo",
        "-pix_fmt",
        "nv12",
        "-",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

#[cfg(windows)]
pub use self::win::{list_names, open};

#[cfg(windows)]
mod win {
    use std::collections::VecDeque;
    use std::io::{BufRead, BufReader, Read};
    use std::process::{Child, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    use parking_lot::Mutex;

    use super::super::{ClockKind, Event, Frame, ProbeFrame, Sink, Source};
    use super::*;

    /// The names DirectShow lists; empty while ffmpeg is not there yet.
    pub fn list_names() -> Vec<String> {
        if !crate::tools::is_ready() {
            return Vec::new();
        }
        let out = crate::muxer::ffmpeg()
            .args(["-hide_banner", "-list_devices", "true", "-f", "dshow", "-i", "dummy"])
            .stdin(Stdio::null())
            .output();
        match out {
            Ok(out) => video_devices(&String::from_utf8_lossy(&out.stderr)),
            Err(err) => {
                log::warn!("cam: listing DirectShow cameras: {err}");
                Vec::new()
            }
        }
    }

    struct DshowSource {
        child: Child,
        size: (u32, u32),
        closing: Arc<AtomicBool>,
        reader: Option<std::thread::JoinHandle<()>>,
    }

    impl Source for DshowSource {
        fn close(mut self: Box<Self>) {
            self.closing.store(true, Ordering::SeqCst);
            let _ = self.child.kill();
            let _ = self.child.wait();
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        }

        fn size(&self) -> (u32, u32) {
            self.size
        }
    }

    /// ffmpeg reads the camera below normal priority; its log is drained on a
    /// thread of its own, frames are read on another.
    pub fn open(device: &CameraDevice, sink: Sink) -> Result<Box<dyn Source>, Error> {
        let name = name_of(&device.id).ok_or_else(|| Error::Unsupported(format!("the camera name {}", device.name)))?;
        let mut child = crate::muxer::background("ffmpeg")
            .args(arguments(name))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| Error::Other(format!("ffmpeg: {err}")))?;
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            return Err(Error::Other("ffmpeg without pipes".into()));
        };
        let tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
        let (size_tx, size_rx) = mpsc::channel();
        {
            let tail = tail.clone();
            std::thread::Builder::new()
                .name("clippiboy-cam-dshow-log".into())
                .spawn(move || {
                    let mut watch = SizeWatch::default();
                    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                        if let Some(size) = watch.feed(&line) {
                            let _ = size_tx.send(size);
                        }
                        let mut tail = tail.lock();
                        if tail.len() == 4 {
                            tail.pop_front();
                        }
                        tail.push_back(line);
                    }
                })
                .map_err(|err| Error::Other(err.to_string()))?;
        }
        let last_words = |tail: &Mutex<VecDeque<String>>| tail.lock().iter().cloned().collect::<Vec<_>>().join(" / ");
        let size = match size_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(size) => size,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                // The log thread ends with the process; its last lines are in.
                std::thread::sleep(Duration::from_millis(50));
                return Err(error_from(&last_words(&tail)));
            }
        };
        let closing = Arc::new(AtomicBool::new(false));
        let reader = {
            let closing = closing.clone();
            let mut sink = sink;
            let mut stdout = stdout;
            let (width, height) = size;
            let bytes = width as usize * height as usize * 3 / 2;
            std::thread::Builder::new()
                .name("clippiboy-cam-dshow".into())
                .spawn(move || loop {
                    let mut buf = vec![0u8; bytes];
                    if stdout.read_exact(&mut buf).is_err() {
                        if !closing.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(50));
                            sink(Event::Lost(error_from(&last_words(&tail))));
                        }
                        return;
                    }
                    let arrival = crate::audio::capture::now_100ns();
                    sink(Event::Frame(Frame {
                        probe: ProbeFrame {
                            qpc_100ns: arrival,
                            clock: ClockKind::Arrival,
                            sample_time_100ns: None,
                            device_ts_100ns: None,
                            device_ref_ts_100ns: None,
                            arrival_100ns: arrival,
                            subtype: *b"NV12",
                            width,
                            height,
                            bytes,
                        },
                        nv12: Arc::from(buf),
                    }));
                })
                .map_err(|err| Error::Other(err.to_string()))?
        };
        log::info!("cam: reading {} through ffmpeg at {}x{}", device.name, size.0, size.1);
        Ok(Box::new(DshowSource { child, size, closing, reader: Some(reader) }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"[in#0 @ 0000018bbb2f0fc0] "HD Pro Webcam C920" (video)
[in#0 @ 0000018bbb2f0fc0]   Alternative name "@device_pnp_\\?\usb#vid_046d&pid_08e5&mi_00#8&1ca148a&0&0000#{65e8773d-8f56-11d0-a3b9-00a0c9223196}\global"
[in#0 @ 0000018bbb2f0fc0] "OBS Virtual Camera" (video)
[in#0 @ 0000018bbb2f0fc0]   Alternative name "@device_sw_{860BB310-5D01-11D0-BD3B-00A0C911CE86}\{A3FCE0F5-3493-419F-958A-ABA1250EC20B}"
[in#0 @ 0000018bbb2f0fc0] "ClippiBoy Camera (Windows Virtual Camera)" (video)
[in#0 @ 0000018bbb2f0fc0] "Capture: Card" (video)
[in#0 @ 0000018bbb2f0fc0] "Both \"kinds\"" (audio, video)
[in#0 @ 0000018bbb2f0fc0] "Chat Mic (TC-HELICON GoXLR Mini)" (audio)
Error opening input file dummy."#;

    fn mf(name: &str) -> CameraDevice {
        CameraDevice { id: format!("\\\\?\\usb#{name}"), name: name.into() }
    }

    #[test]
    fn the_list_has_the_video_devices_only() {
        assert_eq!(
            video_devices(LIST),
            vec!["HD Pro Webcam C920", "OBS Virtual Camera", "ClippiBoy Camera (Windows Virtual Camera)", "Capture: Card", r#"Both \"kinds\""#]
        );
    }

    #[test]
    fn only_what_media_foundation_misses_is_added() {
        let extra = extras(&[mf("HD Pro Webcam C920")], video_devices(LIST));
        assert_eq!(extra, vec![CameraDevice { id: "dshow:OBS Virtual Camera".into(), name: "OBS Virtual Camera".into() }]);
    }

    #[test]
    fn a_name_ffmpeg_would_misread_is_refused() {
        assert_eq!(name_of("dshow:OBS Virtual Camera"), Some("OBS Virtual Camera"));
        assert_eq!(name_of("dshow:Capture: Card"), None);
        assert_eq!(name_of(r#"dshow:a"b"#), None);
        assert_eq!(name_of("dshow:"), None);
        assert_eq!(name_of(r"\\?\usb#vid_046d"), None);
    }

    #[test]
    fn the_size_comes_from_the_output_stream() {
        let mut watch = SizeWatch::default();
        let lines = [
            "Input #0, dshow, from 'video=OBS Virtual Camera':",
            "  Stream #0:0: Video: rawvideo (NV12 / 0x3231564E), nv12, 1280x720, 30 fps, 30 tbr, 10000k tbn",
            "Output #0, rawvideo, to 'pipe:':",
            "  Stream #0:0: Video: rawvideo (NV12 / 0x3231564E), nv12(progressive), 1920x1080 [SAR 1:1 DAR 16:9], q=2-31, 746496 kb/s, 30 fps, 30 tbn",
        ];
        let found: Vec<_> = lines.iter().filter_map(|l| watch.feed(l)).collect();
        assert_eq!(found, vec![(1920, 1080)]);
    }

    #[test]
    fn odd_or_absurd_sizes_are_not_taken() {
        let mut watch = SizeWatch::default();
        watch.feed("Output #0, rawvideo, to 'pipe:':");
        assert_eq!(watch.feed("  Stream #0:0: Video: rawvideo, nv12, 0x0, 30 fps"), None);
        assert_eq!(watch.feed("  Stream #0:0: Video: rawvideo, nv12, 99999x99999, 30 fps"), None);
    }

    #[test]
    fn an_odd_size_is_not_taken() {
        let mut watch = SizeWatch::default();
        watch.feed("Output #0, rawvideo, to 'pipe:':");
        assert_eq!(watch.feed("  Stream #0:0: Video: rawvideo, nv12, 1281x721, 30 fps"), None);
    }

    #[test]
    fn clippiboy_camera_never_reads_itself() {
        let list = vec![mf("ClippiBoy Camera (Windows Virtual Camera)"), mf("HD Pro Webcam C920")];
        assert_eq!(without_our_camera(list), vec![mf("HD Pro Webcam C920")]);
    }

    #[test]
    fn ffmpegs_complaints_become_camera_errors() {
        assert_eq!(
            error_from("[in#0 @ 01] Could not run graph (sometimes caused by a device already in use by other application)"),
            Error::Busy { by: None }
        );
        assert_eq!(error_from("[in#0 @ 01] Could not find video device with name [OBS Virtual Camera] among source devices of type video."), Error::Gone);
        assert_eq!(error_from("something else"), Error::Other("something else".into()));
    }

    #[test]
    fn a_picture_larger_than_clippiboy_cameras_slots_is_scaled_down() {
        let args = arguments("OBS Virtual Camera");
        let filter = args.windows(2).find(|w| w[0] == "-vf").map(|w| w[1].clone()).unwrap_or_default();
        assert!(filter.contains(&format!("min(iw,{})", vcam_protocol::MAX_WIDTH)), "{filter}");
        assert!(filter.contains(&format!("min(ih,{})", vcam_protocol::MAX_HEIGHT)), "{filter}");
        assert!(filter.contains("force_original_aspect_ratio=decrease"), "{filter}");
        assert!(filter.contains("force_divisible_by=2"), "{filter}");
    }

    #[test]
    fn the_device_is_one_argument_and_nothing_else_reads_input() {
        let args = arguments("OBS Virtual Camera");
        assert!(args.windows(2).any(|w| w[0] == "-i" && w[1] == "video=OBS Virtual Camera"));
        assert!(args.contains(&"-nostdin".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("-"));
    }
}
