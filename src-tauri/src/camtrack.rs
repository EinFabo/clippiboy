//! The cam track: the webcam buffered as its own small H.264 stream next to the
//! game picture. This half holds its rules — when it runs, its ring, the tap
//! for recordings, the preview frames — and is the same on every system.


use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

use crate::buffer::{EncodedPacket, ReplayBuffer};
use crate::camera::{Frame, Rect};

/// What decides whether the webcam runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CamWants {
    /// The master switch.
    pub enabled: bool,
    /// *Facecam in clips*.
    pub in_clips: bool,
    pub buffer_wanted: bool,
    /// The settings tab shows its preview (a lease the page renews).
    pub preview_open: bool,
    /// The secure desktop has been up for the buffer's ten seconds.
    pub locked: bool,
}

/// The webcam runs only while something the user switched on needs it.
pub fn cam_wanted(w: &CamWants) -> bool {
    w.enabled && !w.locked && ((w.buffer_wanted && w.in_clips) || w.preview_open)
}

/// How long to wait before closing the camera after a change. `None` while it
/// is still wanted. Only the buffer going away waits — three seconds cover a
/// restart of the game pipeline (another screen, another window) without the
/// LED blinking; the switch, the lock and a closed preview close at once.
pub fn stop_delay(before: &CamWants, after: &CamWants) -> Option<Duration> {
    if cam_wanted(after) {
        return None;
    }
    let only_the_buffer = before.buffer_wanted && !after.buffer_wanted && CamWants { buffer_wanted: false, ..*before } == *after;
    Some(if only_the_buffer { Duration::from_secs(3) } else { Duration::ZERO })
}

/// `CamPlan::stop_after` of a plan that only repeats the last one.
pub const NO_NEWS: Duration = Duration::MAX;

/// What the cam track is told to do.
#[derive(Debug, Clone, PartialEq)]
pub struct CamPlan {
    pub run: bool,
    /// How long to keep the camera open once `run` went false.
    pub stop_after: Duration,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub resolution: crate::model::CamResolution,
    pub fps: u32,
    pub ring_seconds: u32,
    pub encoder: crate::model::EncoderId,
    /// Somebody looks at the preview.
    pub preview: bool,
}

/// What the plan is made from besides the cam's own settings.
#[derive(Debug, Clone)]
pub struct PlanInputs {
    pub buffer_seconds: u32,
    pub keyframe_seconds: u32,
    pub encoder: crate::model::EncoderId,
    pub buffer_wanted: bool,
    pub preview_open: bool,
    pub locked: bool,
}

/// The plan for the cam, and the wants it was made from, to compare the next
/// one against (`stop_delay`).
pub fn plan_for(cam: &crate::model::FacecamConfig, inputs: &PlanInputs, before: Option<&CamWants>) -> (CamPlan, CamWants) {
    let wants = CamWants {
        enabled: cam.enabled,
        in_clips: cam.in_clips,
        buffer_wanted: inputs.buffer_wanted,
        preview_open: inputs.preview_open,
        locked: inputs.locked,
    };
    let stop_after = match before {
        // Nothing changed since the last plan: a stop already on its way keeps
        // its time (the status tick sends a plan every second).
        Some(b) if *b == wants => NO_NEWS,
        Some(b) => stop_delay(b, &wants).unwrap_or(Duration::ZERO),
        None => Duration::ZERO,
    };
    let plan = CamPlan {
        run: cam_wanted(&wants),
        stop_after,
        device_id: cam.device_id.clone(),
        device_name: cam.device_name.clone(),
        resolution: cam.resolution,
        fps: cam.fps,
        ring_seconds: crate::config::cam_ring_seconds(inputs.buffer_seconds, inputs.keyframe_seconds),
        encoder: inputs.encoder,
        preview: inputs.preview_open && cam.enabled,
    };
    (plan, wants)
}

/// The cam's line in the five-second health log.
pub fn health_text(status: &crate::model::CamStatus) -> String {
    let clock = match status.clock {
        Some(crate::camera::ClockKind::Device) => "device",
        Some(crate::camera::ClockKind::DeviceReference) => "device-reference",
        Some(crate::camera::ClockKind::SampleTime) => "sample-time",
        Some(crate::camera::ClockKind::Arrival) => "arrival",
        None => "none",
    };
    let encoder = status.encoder.map(|e| format!("{e:?}")).unwrap_or_else(|| "none".into());
    format!(
        "cam: {:.1} fps in, {:.1} out, dup={} filler={} gaps={} clock={clock} encoder={encoder} ring {:.1} s {:.1} MB",
        status.fps_in,
        status.fps_out,
        status.duplicates,
        status.fillers,
        status.gaps,
        status.ring_seconds,
        status.ring_bytes as f64 / 1_048_576.0,
    )
}

/// What the cam's threads and the rest of the app share.
pub struct CamShared {
    pub ring: Mutex<CamRing>,
    pub preview: PreviewSlot,
    /// The newest raw frame with its QPC — screenshots (stage 4) take it.
    pub latest: Mutex<Option<Arc<Frame>>>,
    pub status: Mutex<Option<crate::model::CamStatus>>,
    pub errors: crate::pipeline::ErrorSlot,
    /// The cam track's size while it runs.
    pub track: Mutex<(u32, u32)>,
}

type PlanSender = Arc<Mutex<Option<crossbeam_channel::Sender<CamPlan>>>>;

/// The cam track, held by `AppState` next to the game pipeline. No thread
/// until a plan says run; the worker ends once the camera is closed again.
pub struct CamTrack {
    plans: PlanSender,
    shared: Arc<CamShared>,
}

impl Default for CamTrack {
    fn default() -> Self {
        Self::new()
    }
}

impl CamTrack {
    pub fn new() -> Self {
        Self {
            plans: Arc::new(Mutex::new(None)),
            shared: Arc::new(CamShared {
                ring: Mutex::new(CamRing::new(1, 16 * 1024 * 1024)),
                preview: PreviewSlot::new(),
                latest: Mutex::new(None),
                status: Mutex::new(None),
                errors: crate::pipeline::ErrorSlot::new(),
                track: Mutex::new((1280, 720)),
            }),
        }
    }

    /// Hands the worker a plan; never blocks. A plan that arrives while the
    /// worker is on its way out starts a new one — the worker only ends by
    /// taking the same lock and finding nothing waiting.
    pub fn apply(&self, plan: CamPlan) {
        #[cfg(windows)]
        {
            let mut plans = self.plans.lock();
            if let Some(tx) = plans.as_ref() {
                if tx.send(plan.clone()).is_ok() {
                    return;
                }
            }
            if !plan.run {
                return;
            }
            let (tx, rx) = crossbeam_channel::unbounded();
            let _ = tx.send(plan);
            *plans = Some(tx);
            win::spawn(self.shared.clone(), rx, self.plans.clone());
        }
        #[cfg(not(windows))]
        {
            let _ = (&self.plans, plan);
        }
    }

    pub fn latest_frame(&self) -> Option<Arc<Frame>> {
        self.shared.latest.lock().clone()
    }

    pub fn preview(&self) -> &PreviewSlot {
        &self.shared.preview
    }

    pub fn ring(&self) -> &Mutex<CamRing> {
        &self.shared.ring
    }

    pub fn status(&self) -> Option<crate::model::CamStatus> {
        self.shared.status.lock().clone()
    }

    pub fn health_line(&self) -> Option<String> {
        self.status().map(|status| health_text(&status))
    }

    pub fn take_unseen_error(&self) -> Option<String> {
        self.shared.errors.take_unseen()
    }

    /// The cam for a clip just saved. Waits up to `wait` — no lock held in
    /// between — for the ring to reach past the window's end, then cuts.
    pub fn cut(&self, window: &crate::facecam::CamWindow, wait: Duration) -> Option<crate::facecam::CamCutOut> {
        let (_, end) = crate::facecam::cut_window(window);
        let deadline = Instant::now() + wait;
        loop {
            let reached = {
                let ring = self.shared.ring.lock();
                ring.origin_100ns + ring.ring.newest_pts() * 10 >= end && !ring.ring.is_empty()
            };
            if reached || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let status = self.status()?;
        let (width, height) = *self.shared.track.lock();
        let ring = self.shared.ring.lock();
        let cut = crate::facecam::cam_cut(&ring, window)?;
        Some(crate::facecam::CamCutOut {
            cut,
            header: ring.header.clone(),
            width,
            height,
            fps: 30,
            clock: status.clock.unwrap_or(crate::camera::ClockKind::Arrival),
            camera: status.camera.unwrap_or_default(),
            encoder: status.encoder,
        })
    }

    pub fn attach_tap(&self, tap: Arc<dyn PacketTap>, from_100ns: i64) -> Vec<EncodedPacket> {
        self.shared.ring.lock().attach_tap(tap, from_100ns)
    }

    pub fn detach_tap(&self) {
        self.shared.ring.lock().detach_tap();
    }
}

#[cfg(windows)]
#[path = "camtrack_win.rs"]
mod win;

/// A stretch without cam frames, QPC in 100 ns; open while `to_100ns` is `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gap {
    pub from_100ns: i64,
    pub to_100ns: Option<i64>,
}

/// Where a recording takes the cam's packets as they come (stage 4). Called
/// while the ring's lock is held, like `Recorder::video`.
pub trait PacketTap: Send + Sync {
    fn cam(&self, packet: &EncodedPacket);
}

/// The cam's own ring. Packet times count from `origin_100ns`, the grid's
/// session start; a new session clears everything.
pub struct CamRing {
    pub ring: ReplayBuffer,
    pub origin_100ns: i64,
    pub session: u64,
    /// The cam encoder's SPS/PPS, for writing a cut out.
    pub header: Vec<u8>,
    pub gaps: VecDeque<Gap>,
    tap: Option<Arc<dyn PacketTap>>,
}

/// A jump in the cam's packet times past this is a new clock, not a stall.
const CAM_JUMP_US: i64 = 2_000_000;

impl CamRing {
    pub fn new(seconds: u32, bytes: u64) -> Self {
        let mut ring = ReplayBuffer::new(seconds, bytes);
        ring.set_jump_limit_us(CAM_JUMP_US);
        Self { ring, origin_100ns: 0, session: 0, header: Vec::new(), gaps: VecDeque::new(), tap: None }
    }

    /// A packet of `session`; one of an older session is dropped (`false`).
    pub fn push(&mut self, session: u64, packet: EncodedPacket) -> bool {
        if session != self.session {
            return false;
        }
        if let Some(tap) = &self.tap {
            tap.cam(&packet);
        }
        self.ring.push(packet);
        self.prune_gaps();
        true
    }

    pub fn set_header(&mut self, session: u64, header: Vec<u8>) {
        if session == self.session {
            self.header = header;
        }
    }

    /// The grid started a session at `origin_100ns`.
    pub fn reset(&mut self, origin_100ns: i64) {
        self.ring.clear();
        self.gaps.clear();
        self.header.clear();
        self.session += 1;
        self.origin_100ns = origin_100ns;
    }

    pub fn open_gap(&mut self, from_100ns: i64) {
        if self.gaps.back().is_some_and(|g| g.to_100ns.is_none()) {
            return;
        }
        self.gaps.push_back(Gap { from_100ns, to_100ns: None });
    }

    pub fn close_gap(&mut self, to_100ns: i64) {
        if let Some(gap) = self.gaps.back_mut().filter(|g| g.to_100ns.is_none()) {
            gap.to_100ns = Some(to_100ns);
        }
    }

    /// Hands every packet from now on to `tap`; returns the backlog from the
    /// cam keyframe at or before `from_100ns`.
    pub fn attach_tap(&mut self, tap: Arc<dyn PacketTap>, from_100ns: i64) -> Vec<EncodedPacket> {
        self.tap = Some(tap);
        self.ring.snapshot_range((from_100ns - self.origin_100ns) / 10, i64::MAX)
    }

    pub fn detach_tap(&mut self) {
        self.tap = None;
    }

    /// Gaps that ended before the ring's front left with it.
    fn prune_gaps(&mut self) {
        let Some(front_us) = self.ring.oldest_pts() else { return };
        let front = self.origin_100ns + front_us * 10;
        while self.gaps.front().is_some_and(|g| g.to_100ns.is_some_and(|to| to < front)) {
            self.gaps.pop_front();
        }
    }
}

/// A small picture of the cam for the settings and the console.
#[derive(Debug, Clone)]
pub struct PreviewFrame {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub bgra: Arc<[u8]>,
}

/// The preview's width in pixels.
pub const PREVIEW_WIDTH: u32 = 320;
const PREVIEW_EVERY: Duration = Duration::from_millis(1000 / 15);
/// A frame is made only while someone asked in this long.
const PREVIEW_INTEREST: Duration = Duration::from_secs(2);

#[derive(Default)]
struct PreviewInner {
    frame: Option<PreviewFrame>,
    seq: u64,
    made: Option<Instant>,
    asked: Option<Instant>,
}

/// The newest preview frame. Made at most 15 times a second, and only while
/// somebody looks.
pub struct PreviewSlot {
    inner: Mutex<PreviewInner>,
    fresh: Condvar,
}

impl Default for PreviewSlot {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewSlot {
    pub fn new() -> Self {
        Self { inner: Mutex::new(PreviewInner::default()), fresh: Condvar::new() }
    }

    pub fn offer(&self, frame: &Frame, crop: Rect, now: Instant) {
        {
            let inner = self.inner.lock();
            let wanted = inner.asked.is_some_and(|t| now.saturating_duration_since(t) < PREVIEW_INTEREST);
            let due = inner.made.map_or(true, |t| now.saturating_duration_since(t) >= PREVIEW_EVERY);
            if !wanted || !due {
                return;
            }
        }
        // Converted outside the lock: a waiting page is not held up by it.
        let (width, height, bgra) = preview_bgra(frame, crop, PREVIEW_WIDTH);
        let mut inner = self.inner.lock();
        inner.seq += 1;
        let seq = inner.seq;
        inner.frame = Some(PreviewFrame { seq, width, height, bgra: bgra.into() });
        inner.made = Some(now);
        drop(inner);
        self.fresh.notify_all();
    }

    /// A frame newer than `after`, waiting up to `limit` for one. Asking is
    /// what keeps frames coming.
    pub fn wait_after(&self, after: u64, limit: Duration) -> Option<PreviewFrame> {
        let deadline = Instant::now() + limit;
        let mut inner = self.inner.lock();
        inner.asked = Some(Instant::now());
        loop {
            if let Some(frame) = inner.frame.as_ref().filter(|f| f.seq > after) {
                return Some(frame.clone());
            }
            if self.fresh.wait_until(&mut inner, deadline).timed_out() {
                return inner.frame.as_ref().filter(|f| f.seq > after).cloned();
            }
        }
    }

    /// Forgets the last frame, so a stopped cam shows nothing old.
    pub fn clear(&self) {
        self.inner.lock().frame = None;
    }
}

/// The `crop` of an NV12 frame as BGRA, `width` pixels wide, point-sampled,
/// BT.601 studio range. Only for looking at: the clip gets the real scaler.
pub fn preview_bgra(frame: &Frame, crop: Rect, width: u32) -> (u32, u32, Vec<u8>) {
    let (fw, fh) = (frame.probe.width as usize, frame.probe.height as usize);
    let crop = if crop.w == 0 || crop.h == 0 { Rect { x: 0, y: 0, w: fw as u32, h: fh as u32 } } else { crop };
    let ow = width.max(2) as usize;
    let oh = ((ow as u64 * crop.h as u64 / crop.w.max(1) as u64) as usize).max(2) & !1;
    let mut out = vec![0u8; ow * oh * 4];
    let (luma, chroma) = frame.nv12.split_at((fw * fh).min(frame.nv12.len()));
    for oy in 0..oh {
        let sy = (crop.y as usize + oy * crop.h as usize / oh).min(fh.saturating_sub(1));
        for ox in 0..ow {
            let sx = (crop.x as usize + ox * crop.w as usize / ow).min(fw.saturating_sub(1));
            let y = *luma.get(sy * fw + sx).unwrap_or(&16) as f32;
            let c = (sy / 2) * fw + (sx / 2) * 2;
            let u = *chroma.get(c).unwrap_or(&128) as f32 - 128.0;
            let v = *chroma.get(c + 1).unwrap_or(&128) as f32 - 128.0;
            let l = 1.164 * (y - 16.0);
            let px = &mut out[(oy * ow + ox) * 4..][..4];
            px[0] = (l + 2.017 * u).round().clamp(0.0, 255.0) as u8;
            px[1] = (l - 0.392 * u - 0.813 * v).round().clamp(0.0, 255.0) as u8;
            px[2] = (l + 1.596 * v).round().clamp(0.0, 255.0) as u8;
            px[3] = 255;
        }
    }
    (ow as u32, oh as u32, out)
}

/// The bytes the bridge hands to a page: `seq` (u64), width and height (u32),
/// all little-endian, then the BGRA pixels.
pub fn encode_preview(frame: &PreviewFrame) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + frame.bgra.len());
    out.extend_from_slice(&frame.seq.to_le_bytes());
    out.extend_from_slice(&frame.width.to_le_bytes());
    out.extend_from_slice(&frame.height.to_le_bytes());
    out.extend_from_slice(&frame.bgra);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::EncodedPacket;
    use crate::camera::{ClockKind, Frame, ProbeFrame, Rect};
    use std::sync::Mutex as StdMutex;
    use std::time::{Duration, Instant};

    fn wants(enabled: bool, in_clips: bool, buffer: bool, preview: bool, locked: bool) -> CamWants {
        CamWants { enabled, in_clips, buffer_wanted: buffer, preview_open: preview, locked }
    }

    fn inputs(buffer_wanted: bool, preview_open: bool) -> PlanInputs {
        PlanInputs {
            buffer_seconds: 120,
            keyframe_seconds: 2,
            encoder: crate::model::EncoderId::Nvenc,
            buffer_wanted,
            preview_open,
            locked: false,
        }
    }

    #[test]
    fn the_plan_follows_the_switches() {
        let mut cam = crate::model::FacecamConfig::default();
        let (plan, off) = plan_for(&cam, &inputs(true, false), None);
        assert!(!plan.run);
        assert_eq!(plan.stop_after, Duration::ZERO);
        cam.enabled = true;
        let (plan, on) = plan_for(&cam, &inputs(true, false), Some(&off));
        assert!(plan.run);
        assert_eq!(plan.ring_seconds, 134);
        assert_eq!(plan.resolution, crate::model::CamResolution::P720);
        let (plan, _) = plan_for(&cam, &inputs(false, false), Some(&on));
        assert!(!plan.run);
        assert_eq!(plan.stop_after, Duration::from_secs(3));
        let (plan, _) = plan_for(&cam, &inputs(false, true), Some(&on));
        assert!(plan.run && plan.preview);
    }

    /// The status tick sends a plan every second. One that only repeats the
    /// last wants must not cut the 3 s a pipeline restart gets down to nothing.
    #[test]
    fn an_unchanged_plan_does_not_cut_the_grace_short() {
        let mut cam = crate::model::FacecamConfig::default();
        cam.enabled = true;
        let (_, on) = plan_for(&cam, &inputs(true, false), None);
        let (plan, off) = plan_for(&cam, &inputs(false, false), Some(&on));
        assert_eq!(plan.stop_after, Duration::from_secs(3));
        let (again, _) = plan_for(&cam, &inputs(false, false), Some(&off));
        assert!(!again.run);
        assert_eq!(again.stop_after, NO_NEWS);
    }

    #[test]
    fn the_health_line_says_what_the_cam_does() {
        let status = crate::model::CamStatus {
            state: crate::model::CamState::Live,
            camera: Some("HD Pro Webcam C920".into()),
            fps_in: 29.84,
            fps_out: 30.0,
            duplicates: 412,
            fillers: 0,
            gaps: 0,
            clock: Some(crate::camera::ClockKind::Device),
            encoder: Some(crate::model::EncoderId::Nvenc),
            ring_seconds: 133.2,
            ring_bytes: 50_436_506,
        };
        assert_eq!(
            health_text(&status),
            "cam: 29.8 fps in, 30.0 out, dup=412 filler=0 gaps=0 clock=device encoder=Nvenc ring 133.2 s 48.1 MB"
        );
    }

    #[test]
    fn the_cam_runs_only_while_something_the_user_switched_on_needs_it() {
        for buffer in [false, true] {
            for clips in [false, true] {
                for preview in [false, true] {
                    assert!(!cam_wanted(&wants(false, clips, buffer, preview, false)), "master off");
                }
            }
        }
        assert!(cam_wanted(&wants(true, true, true, false, false)));
        assert!(!cam_wanted(&wants(true, false, true, false, false)));
        assert!(cam_wanted(&wants(true, false, false, true, false)));
        assert!(!cam_wanted(&wants(true, true, true, false, true)));
        assert!(!cam_wanted(&wants(true, true, false, true, true)));
    }

    #[test]
    fn a_buffer_restart_does_not_blink_the_camera() {
        let before = wants(true, true, true, false, false);
        let after = wants(true, true, false, false, false);
        assert_eq!(stop_delay(&before, &after), Some(Duration::from_secs(3)));
        assert_eq!(stop_delay(&before, &before), None);
    }

    #[test]
    fn cam_off_closes_at_once() {
        let before = wants(true, true, true, false, false);
        let after = wants(false, true, true, false, false);
        assert_eq!(stop_delay(&before, &after), Some(Duration::ZERO));
    }

    #[test]
    fn the_lock_closes_at_once() {
        let before = wants(true, true, true, false, false);
        let after = wants(true, true, true, false, true);
        assert_eq!(stop_delay(&before, &after), Some(Duration::ZERO));
    }

    #[test]
    fn a_closed_preview_lets_go_at_once() {
        let before = wants(true, true, false, true, false);
        let after = wants(true, true, false, false, false);
        assert_eq!(stop_delay(&before, &after), Some(Duration::ZERO));
    }

    /// A cam packet at `ms` after the session's origin; a keyframe every second.
    fn packet(ms: i64) -> EncodedPacket {
        EncodedPacket {
            track: 0,
            pts_us: ms * 1000,
            duration_us: 33_333,
            keyframe: ms % 1000 == 0,
            data: vec![0u8; 100].into(),
        }
    }

    fn filled(ring: &mut CamRing, until_ms: i64) {
        let session = ring.session;
        let mut ms = 0;
        while ms <= until_ms {
            assert!(ring.push(session, packet(ms)));
            ms += 1000 / 30;
            if ms % 1000 > 966 {
                ms = (ms / 1000 + 1) * 1000;
            }
        }
    }

    #[test]
    fn a_packet_from_an_old_session_is_dropped() {
        let mut ring = CamRing::new(30, u64::MAX);
        let old = ring.session;
        ring.reset(5_000);
        assert_eq!(ring.origin_100ns, 5_000);
        assert!(!ring.push(old, packet(0)));
        ring.set_header(old, vec![1, 2, 3]);
        assert!(ring.header.is_empty());
        let now = ring.session;
        ring.set_header(now, vec![1, 2, 3]);
        assert_eq!(ring.header, vec![1, 2, 3]);
    }

    #[test]
    fn gaps_leave_with_the_ring() {
        let mut ring = CamRing::new(3, u64::MAX);
        ring.reset(0);
        ring.open_gap(10_000_000);
        ring.close_gap(15_000_000);
        ring.open_gap(80_000_000);
        filled(&mut ring, 10_000);
        assert_eq!(ring.gaps.len(), 1);
        assert_eq!(ring.gaps[0].from_100ns, 80_000_000);
        assert_eq!(ring.gaps[0].to_100ns, None);
    }

    struct Seen(StdMutex<Vec<i64>>);

    impl PacketTap for Seen {
        fn cam(&self, packet: &EncodedPacket) {
            self.0.lock().unwrap().push(packet.pts_us);
        }
    }

    #[test]
    fn a_tap_sees_its_backlog_and_every_packet_after_it() {
        let mut ring = CamRing::new(30, u64::MAX);
        ring.reset(0);
        filled(&mut ring, 2_966);
        let seen = Arc::new(Seen(StdMutex::new(Vec::new())));
        let backlog = ring.attach_tap(seen.clone(), 15_000_000);
        assert_eq!(backlog[0].pts_us, 1_000_000);
        assert!(backlog[0].keyframe);
        let session = ring.session;
        ring.push(session, packet(3_000));
        ring.push(session, packet(3_033));
        assert_eq!(*seen.0.lock().unwrap(), vec![3_000_000, 3_033_000]);
        ring.detach_tap();
        ring.push(session, packet(3_066));
        assert_eq!(seen.0.lock().unwrap().len(), 2);
    }

    fn grey(width: u32, height: u32) -> Frame {
        let (w, h) = (width as usize, height as usize);
        let mut nv12 = vec![126u8; w * h];
        nv12.extend(std::iter::repeat(128u8).take(w * h / 2));
        Frame {
            probe: ProbeFrame {
                qpc_100ns: 0,
                clock: ClockKind::Device,
                sample_time_100ns: None,
                device_ts_100ns: None,
                device_ref_ts_100ns: None,
                arrival_100ns: 0,
                subtype: *b"NV12",
                width,
                height,
                bytes: nv12.len(),
            },
            nv12: nv12.into(),
        }
    }

    #[test]
    fn a_grey_frame_previews_grey() {
        let (w, h, bgra) = preview_bgra(&grey(4, 2), Rect { x: 0, y: 0, w: 4, h: 2 }, 4);
        assert_eq!((w, h), (4, 2));
        assert_eq!(bgra.len(), 4 * 2 * 4);
        for px in bgra.chunks_exact(4) {
            for c in &px[..3] {
                assert!((*c as i32 - 128).abs() <= 2, "{px:?}");
            }
            assert_eq!(px[3], 255);
        }
    }

    #[test]
    fn the_preview_frame_starts_with_its_sequence_number() {
        let frame = PreviewFrame { seq: 7, width: 2, height: 1, bgra: vec![1, 2, 3, 4, 5, 6, 7, 8].into() };
        let bytes = encode_preview(&frame);
        assert_eq!(&bytes[..8], &7u64.to_le_bytes());
        assert_eq!(&bytes[8..12], &2u32.to_le_bytes());
        assert_eq!(&bytes[12..16], &1u32.to_le_bytes());
        assert_eq!(&bytes[16..], &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn nobody_asking_means_no_preview_work() {
        let slot = PreviewSlot::new();
        let now = Instant::now();
        slot.offer(&grey(64, 36), Rect { x: 0, y: 0, w: 64, h: 36 }, now);
        assert!(slot.wait_after(0, Duration::ZERO).is_none());
        // Asked for now: the next offer is made.
        slot.offer(&grey(64, 36), Rect { x: 0, y: 0, w: 64, h: 36 }, Instant::now());
        let got = slot.wait_after(0, Duration::ZERO).expect("a frame once asked for");
        assert_eq!(got.seq, 1);
        assert_eq!(got.width, 320);
    }
}
