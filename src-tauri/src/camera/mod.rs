//! The webcam: listing and opening cameras, frames with a timestamp on the
//! one clock (QPC in 100 ns), and the errors a camera gives by name.
//! Platform-neutral here; Media Foundation in `windows.rs`.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use crate::model::CameraDevice;

pub mod grid;
#[cfg(windows)]
pub mod windows;

/// How the camera is opened. Stage 1 found that a sharing reader neither lets
/// Discord in nor gets more than YUY2 640×480 from a C920 when alone, so the
/// facecam controls the camera and picks its format (`pick_native`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    /// The camera's own, at the native type `pick_native` chooses.
    Controlling,
    /// The MF sensor group in shared mode: whatever format the camera runs.
    MfSensorShared,
}

/// Which of a frame's timestamps was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClockKind {
    Device,
    DeviceReference,
    SampleTime,
    /// None was plausible; the time the frame reached us. Later than the
    /// moment of capture by the camera's own latency.
    Arrival,
}

/// How far a stamp may lie from the arrival time and still count — the bound
/// the audio code's `usable_stamp` uses.
const PLAUSIBLE_100NS: i64 = 20_000_000;

/// The first of `device`, `device_ref`, `sample` within ±2 s of `arrival`,
/// else `arrival`. A stream-relative sample time (one that starts at zero)
/// never passes.
pub fn choose_stamp(device: Option<i64>, device_ref: Option<i64>, sample: Option<i64>, arrival: i64) -> (i64, ClockKind) {
    [(device, ClockKind::Device), (device_ref, ClockKind::DeviceReference), (sample, ClockKind::SampleTime)]
        .into_iter()
        .find_map(|(stamp, kind)| stamp.filter(|t| (t - arrival).abs() <= PLAUSIBLE_100NS).map(|t| (t, kind)))
        .unwrap_or((arrival, ClockKind::Arrival))
}

/// What a frame came with, for the grid and the diagnostics.
#[derive(Debug, Clone)]
pub struct ProbeFrame {
    /// The chosen stamp, QPC in 100 ns.
    pub qpc_100ns: i64,
    pub clock: ClockKind,
    pub sample_time_100ns: Option<i64>,
    pub device_ts_100ns: Option<i64>,
    pub device_ref_ts_100ns: Option<i64>,
    pub arrival_100ns: i64,
    /// What we receive, not the camera's native format.
    pub subtype: [u8; 4],
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
}

/// One camera frame: tightly packed NV12 in system memory — rows of
/// `probe.width` bytes, the UV plane right after `width × height`.
#[derive(Debug, Clone)]
pub struct Frame {
    pub probe: ProbeFrame,
    pub nv12: Arc<[u8]>,
}

pub enum Event {
    Frame(Frame),
    FormatChanged { width: u32, height: u32 },
    Lost(Error),
}

pub type Sink = Box<dyn FnMut(Event) + Send>;

/// An open camera. Closing it turns the LED off.
pub trait Source: Send {
    fn close(self: Box<Self>);
    fn size(&self) -> (u32, u32);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Windows' camera privacy blocks desktop apps.
    Denied,
    /// Another app holds the camera.
    Busy { by: Option<String> },
    /// Unplugged.
    Gone,
    NoCamera,
    Unsupported(String),
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Denied => write!(f, "Windows blocks the camera for desktop apps"),
            Self::Busy { by: Some(by) } => write!(f, "the camera is taken by {by}"),
            Self::Busy { by: None } => write!(f, "the camera is taken by another app"),
            Self::Gone => write!(f, "the camera is gone"),
            Self::NoCamera => write!(f, "no camera"),
            Self::Unsupported(what) => write!(f, "not supported: {what}"),
            Self::Other(what) => write!(f, "{what}"),
        }
    }
}

/// The HRESULTs a camera answers with, by name.
pub fn classify(hresult: i32) -> Error {
    match hresult as u32 {
        0x8007_0005 => Error::Denied,
        0xC00D_3704 => Error::Busy { by: None },
        0xC00D_3EA2 => Error::Gone,
        other => Error::Other(format!("{other:#010x}")),
    }
}

/// The chosen camera: by id (case-insensitive), else by a name only one camera
/// has — the same camera on another USB port gets another id — else the first
/// when nothing was chosen. A chosen camera that is gone is not replaced.
pub fn pick_device<'a>(list: &'a [CameraDevice], id: Option<&str>, name: Option<&str>) -> Option<&'a CameraDevice> {
    if id.is_none() && name.is_none() {
        return list.first();
    }
    if let Some(found) = id.and_then(|id| list.iter().find(|d| d.id.eq_ignore_ascii_case(id))) {
        return Some(found);
    }
    let name = name?;
    let mut named = list.iter().filter(|d| d.name == name);
    match (named.next(), named.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    }
}

/// A rectangle in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

fn even(v: f64) -> u32 {
    (2.0 * (v / 2.0).round()).max(0.0) as u32
}

/// The centred part of a `src` picture with the aspect of `dst`, so the cam
/// fills its box instead of getting bars. Every number even (NV12).
pub fn cover_crop(src: (u32, u32), dst: (u32, u32)) -> Rect {
    let (sw, sh) = (src.0 as u64, src.1 as u64);
    let (dw, dh) = (dst.0 as u64, dst.1 as u64);
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return Rect { x: 0, y: 0, w: src.0, h: src.1 };
    }
    if sw * dh > dw * sh {
        let w = even((sh * dw) as f64 / dh as f64).min(src.0);
        Rect { x: even((src.0 - w) as f64 / 2.0), y: 0, w, h: src.1 }
    } else {
        let h = even((sw * dh) as f64 / dw as f64).min(src.1);
        Rect { x: 0, y: even((src.1 - h) as f64 / 2.0), w: src.0, h }
    }
}

/// `(BT.709, full range)` from Media Foundation's raw values: matrix 1 is
/// BT.709, 2 BT.601; range 1 is 0–255, 2 is 16–235. Untagged is what webcams
/// almost always mean: BT.601, studio range.
pub fn colour_of(matrix: Option<u32>, nominal_range: Option<u32>) -> (bool, bool) {
    (matrix == Some(1), nominal_range == Some(1))
}

/// The waits between attempts to open a camera: 1, 2, 5, then 10 s.
#[derive(Debug, Default)]
pub struct Backoff {
    step: usize,
}

impl Backoff {
    const WAITS: [u64; 4] = [1, 2, 5, 10];

    pub fn next(&mut self) -> Duration {
        let wait = Self::WAITS[self.step.min(Self::WAITS.len() - 1)];
        self.step += 1;
        Duration::from_secs(wait)
    }

    pub fn reset(&mut self) {
        self.step = 0;
    }
}

/// One of the formats a camera can run in.
#[derive(Debug, Clone, PartialEq)]
pub struct NativeType {
    pub subtype: [u8; 4],
    pub width: u32,
    pub height: u32,
    pub fps: f32,
}

/// The format to run a controlled camera in: at 30 fps when it has one, the
/// smallest picture at least as large as the track (else its largest), MJPEG
/// before NV12 before YUY2 at the same size — over USB 2.0 raw 720p only
/// manages about 10 fps.
pub fn pick_native(types: &[NativeType], track: (u32, u32)) -> Option<usize> {
    let fastest = types.iter().map(|t| t.fps).fold(0.0f32, f32::max);
    let floor = if fastest >= 29.0 { 29.0 } else { fastest };
    let rank = |t: &NativeType| match &t.subtype {
        b"MJPG" => 0,
        b"NV12" => 1,
        b"YUY2" => 2,
        _ => 3,
    };
    let area = |t: &NativeType| t.width as u64 * t.height as u64;
    let usable = || types.iter().enumerate().filter(|(_, t)| t.fps >= floor);
    let large_enough = usable()
        .filter(|(_, t)| t.width >= track.0 && t.height >= track.1)
        .min_by_key(|(_, t)| (area(t), rank(t)));
    large_enough
        .or_else(|| usable().max_by_key(|(_, t)| (area(t), std::cmp::Reverse(rank(t)))))
        .map(|(i, _)| i)
}

/// The cameras Windows lists. Empty on other systems for now.
pub fn list() -> Vec<CameraDevice> {
    #[cfg(windows)]
    {
        self::windows::list()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Opens `device`; frames and events go to `sink` on a thread of the camera's.
pub fn open(device: &CameraDevice, mode: OpenMode, sink: Sink) -> Result<Box<dyn Source>, Error> {
    #[cfg(windows)]
    {
        self::windows::open(device, mode, sink)
    }
    #[cfg(not(windows))]
    {
        let _ = (device, mode, sink);
        Err(Error::NoCamera)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: i64 = 36_000_000_000; // a QPC of an hour

    #[test]
    fn a_device_stamp_half_a_second_old_is_used() {
        assert_eq!(choose_stamp(Some(A), None, None, A + 5_000_000), (A, ClockKind::Device));
    }

    #[test]
    fn a_stamp_three_seconds_off_falls_back_to_arrival() {
        assert_eq!(choose_stamp(Some(A), None, None, A + 30_000_000), (A + 30_000_000, ClockKind::Arrival));
    }

    #[test]
    fn a_stream_relative_sample_time_is_not_taken() {
        assert_eq!(choose_stamp(None, None, Some(1_000), A), (A, ClockKind::Arrival));
    }

    #[test]
    fn the_device_reference_is_second_choice() {
        assert_eq!(choose_stamp(Some(A - 30_000_000), Some(A - 1), None, A), (A - 1, ClockKind::DeviceReference));
    }

    #[test]
    fn the_device_stamp_wins_when_all_are_plausible() {
        assert_eq!(choose_stamp(Some(A - 3), Some(A - 2), Some(A - 1), A), (A - 3, ClockKind::Device));
    }

    #[test]
    fn the_sample_time_is_used_when_it_is_the_only_plausible_one() {
        assert_eq!(choose_stamp(None, Some(5), Some(A - 7), A), (A - 7, ClockKind::SampleTime));
    }

    #[test]
    fn two_seconds_exactly_is_still_plausible() {
        assert_eq!(choose_stamp(Some(A - 20_000_000), None, None, A), (A - 20_000_000, ClockKind::Device));
        assert_eq!(choose_stamp(Some(A - 20_000_001), None, None, A).1, ClockKind::Arrival);
        assert_eq!(choose_stamp(Some(A + 20_000_000), None, None, A), (A + 20_000_000, ClockKind::Device));
        assert_eq!(choose_stamp(Some(A + 20_000_001), None, None, A).1, ClockKind::Arrival);
    }

    #[test]
    fn the_errors_a_camera_gives_are_named() {
        assert_eq!(classify(0x8007_0005_u32 as i32), Error::Denied);
        assert_eq!(classify(0xC00D_3704_u32 as i32), Error::Busy { by: None });
        assert_eq!(classify(0xC00D_3EA2_u32 as i32), Error::Gone);
        assert_eq!(classify(0x8000_4005_u32 as i32), Error::Other("0x80004005".into()));
    }

    fn cam(id: &str, name: &str) -> CameraDevice {
        CameraDevice { id: id.into(), name: name.into() }
    }

    #[test]
    fn a_replugged_camera_is_found_by_its_name() {
        let list = [cam("usb#new-port", "HD Pro Webcam C920"), cam("usb#other", "EMEET SmartCam C960")];
        let found = pick_device(&list, Some("usb#old-port"), Some("HD Pro Webcam C920"));
        assert_eq!(found.map(|d| d.id.as_str()), Some("usb#new-port"));
        // The id wins, case-insensitively.
        let found = pick_device(&list, Some("USB#OTHER"), Some("HD Pro Webcam C920"));
        assert_eq!(found.map(|d| d.id.as_str()), Some("usb#other"));
    }

    #[test]
    fn two_cameras_of_one_name_are_not_guessed() {
        let list = [cam("usb#a", "USB Camera"), cam("usb#b", "USB Camera")];
        assert!(pick_device(&list, Some("usb#gone"), Some("USB Camera")).is_none());
    }

    #[test]
    fn no_choice_takes_the_first() {
        let list = [cam("usb#a", "First"), cam("usb#b", "Second")];
        assert_eq!(pick_device(&list, None, None).map(|d| d.id.as_str()), Some("usb#a"));
        assert!(pick_device(&[], None, None).is_none());
    }

    #[test]
    fn a_chosen_camera_that_is_gone_is_not_replaced() {
        let list = [cam("usb#a", "First")];
        assert!(pick_device(&list, Some("usb#gone"), Some("Gone")).is_none());
    }

    #[test]
    fn a_four_by_three_camera_is_cropped_to_fill() {
        assert_eq!(cover_crop((640, 480), (1280, 720)), Rect { x: 0, y: 60, w: 640, h: 360 });
        assert_eq!(cover_crop((1920, 1080), (1280, 720)), Rect { x: 0, y: 0, w: 1920, h: 1080 });
        assert_eq!(cover_crop((1080, 1920), (1280, 720)), Rect { x: 0, y: 656, w: 1080, h: 608 });
    }

    #[test]
    fn a_wide_camera_is_cropped_at_the_sides() {
        let r = cover_crop((2560, 1080), (1280, 720));
        assert_eq!((r.y, r.h), (0, 1080));
        assert_eq!(r.w, 1920);
        assert_eq!(r.x, 320);
        assert!(r.x % 2 == 0 && r.w % 2 == 0);
    }

    #[test]
    fn a_camera_without_colour_tags_is_bt601_studio_range() {
        assert_eq!(colour_of(None, None), (false, false));
        assert_eq!(colour_of(Some(1), Some(1)), (true, true));
        assert_eq!(colour_of(Some(2), Some(2)), (false, false));
    }

    #[test]
    fn the_retry_waits_grow_to_ten_seconds() {
        let mut backoff = Backoff::default();
        let waits: Vec<u64> = (0..5).map(|_| backoff.next().as_secs()).collect();
        assert_eq!(waits, [1, 2, 5, 10, 10]);
        backoff.reset();
        assert_eq!(backoff.next().as_secs(), 1);
    }

    fn native(fourcc: &[u8; 4], w: u32, h: u32, fps: f32) -> NativeType {
        NativeType { subtype: *fourcc, width: w, height: h, fps }
    }

    /// What a C920 offers over USB 2.0: YUY2 at 720p only at 10 fps.
    fn c920() -> Vec<NativeType> {
        vec![
            native(b"YUY2", 640, 480, 30.0),
            native(b"YUY2", 1280, 720, 10.0),
            native(b"MJPG", 640, 480, 30.0),
            native(b"MJPG", 1280, 720, 30.0),
            native(b"MJPG", 1920, 1080, 30.0),
        ]
    }

    #[test]
    fn a_controlled_c920_runs_mjpeg_at_the_tracks_size() {
        assert_eq!(pick_native(&c920(), (1280, 720)), Some(3));
        assert_eq!(pick_native(&c920(), (1920, 1080)), Some(4));
    }

    #[test]
    fn a_camera_without_the_tracks_size_takes_the_next_larger_one() {
        let list = vec![native(b"NV12", 1920, 1080, 30.0), native(b"NV12", 640, 360, 30.0)];
        assert_eq!(pick_native(&list, (1280, 720)), Some(0));
    }

    #[test]
    fn a_small_camera_gives_its_largest_picture() {
        let list = vec![native(b"YUY2", 320, 240, 30.0), native(b"YUY2", 640, 480, 30.0)];
        assert_eq!(pick_native(&list, (1280, 720)), Some(1));
    }

    #[test]
    fn a_camera_that_never_reaches_thirty_takes_its_fastest() {
        let list = vec![native(b"YUY2", 1280, 720, 10.0), native(b"YUY2", 640, 480, 15.0)];
        assert_eq!(pick_native(&list, (1280, 720)), Some(1));
        assert_eq!(pick_native(&[], (1280, 720)), None);
    }
}
