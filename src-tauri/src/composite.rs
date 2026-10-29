//! Laying the cam over a clip: Rust turns the layout and the timing into
//! literals, and the ffmpeg line holds nothing else.


use std::path::Path;

use crate::camera::Rect;
use crate::facecam::FacecamFacts;
use crate::model::{CamLayout, CamShape, EncoderId};

fn even(v: f64) -> u32 {
    (2.0 * (v / 2.0).round()).max(0.0) as u32
}

/// The space kept free around the cam: 2.5 % of the frame's height.
pub fn margin(frame_height: u32) -> u32 {
    even(0.025 * frame_height as f64)
}

/// Where the cam lands in a frame: its height a share of the frame's, 16:9
/// (a circle square), inside the margin, every number even. On a screen too
/// narrow for it (portrait) the cam narrows instead of running off.
pub fn cam_rect(frame: (u32, u32), layout: &CamLayout) -> Rect {
    let (fw, fh) = (frame.0 as f64, frame.1 as f64);
    let circle = layout.shape == CamShape::Circle;
    let m = margin(frame.1);
    let mut h = even(layout.size as f64 * fh).max(2);
    let mut w = if circle { h } else { even(h as f64 * 16.0 / 9.0) };
    let room = frame.0.saturating_sub(2 * m);
    if w > room {
        w = room & !1;
        h = if circle { w } else { even(w as f64 * 9.0 / 16.0) };
    }
    let free_x = (fw - w as f64 - 2.0 * m as f64).max(0.0);
    let free_y = (fh - h as f64 - 2.0 * m as f64).max(0.0);
    Rect {
        x: m + even(layout.x.clamp(0.0, 1.0) as f64 * free_x).min(free_x as u32 & !1),
        y: m + even(layout.y.clamp(0.0, 1.0) as f64 * free_y).min(free_y as u32 & !1),
        w,
        h,
    }
}

/// The centred square of the cam picture for a circle.
pub fn cam_crop(cam: (u32, u32), shape: CamShape) -> Option<Rect> {
    if shape != CamShape::Circle {
        return None;
    }
    let side = cam.0.min(cam.1) & !1;
    Some(Rect { x: ((cam.0 - side) / 2) & !1, y: ((cam.1 - side) / 2) & !1, w: side, h: side })
}

/// Where the cam starts against the clip, in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CamTiming {
    /// Seek into `cam.mp4` when it starts before the clip.
    pub seek_s: Option<f64>,
    /// Added to the cam's timestamps.
    pub shift_s: f64,
    /// The cam ends with the clip.
    pub end_s: f64,
}

/// `d` is where the cam starts against the game (its offset plus the delay),
/// `e` how far into the cam the clip's `start` lies. The clean clip's video
/// starts at `video_start_us`, not 0 (SAVE-7), and the cam follows it.
pub fn cam_timing(offset_us: i64, delay_ms: i32, video_start_us: i64, start_ms: u64, length_ms: u64) -> CamTiming {
    let d = offset_us + delay_ms as i64 * 1000;
    let e = start_ms as i64 * 1000 - d;
    let start_s = video_start_us as f64 / 1e6;
    let (seek_s, shift_s) = if e > 0 { (Some(e as f64 / 1e6), start_s) } else { (None, start_s - e as f64 / 1e6) };
    CamTiming { seek_s, shift_s, end_s: start_s + length_ms as f64 / 1000.0 }
}

/// An 8-bit alpha mask for the shape, anti-aliased; `None` for a rectangle.
/// Rounded corners have a radius of 12 % of the height.
pub fn mask_gray8(shape: CamShape, w: u32, h: u32) -> Option<Vec<u8>> {
    let (wf, hf) = (w as f64, h as f64);
    let alpha = |inside: f64| (inside.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut out = Vec::with_capacity((w * h) as usize);
    match shape {
        CamShape::Rect => return None,
        CamShape::Circle => {
            let r = wf.min(hf) / 2.0;
            for y in 0..h {
                for x in 0..w {
                    let (dx, dy) = (x as f64 + 0.5 - wf / 2.0, y as f64 + 0.5 - hf / 2.0);
                    out.push(alpha(r - (dx * dx + dy * dy).sqrt() + 0.5));
                }
            }
        }
        CamShape::Rounded => {
            let r = 0.12 * hf;
            for y in 0..h {
                for x in 0..w {
                    let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                    let dx = (r - px).max(px - (wf - r)).max(0.0);
                    let dy = (r - py).max(py - (hf - r)).max(0.0);
                    out.push(alpha(r - (dx * dx + dy * dy).sqrt() + 0.5));
                }
            }
        }
    }
    Some(out)
}

pub fn write_mask_png(path: &Path, w: u32, h: u32, pixels: &[u8]) -> std::io::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, w, h);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
    writer.write_image_data(pixels).map_err(std::io::Error::other)
}

/// What the cam's part of a filter graph is made from.
pub struct CamChain<'a> {
    pub cam_input: usize,
    pub mask_input: Option<usize>,
    /// The picture the cam goes over, as a pad label ("0:v").
    pub main: &'a str,
    pub frame: (u32, u32),
    pub cam: (u32, u32),
    pub layout: CamLayout,
    pub timing: CamTiming,
    pub gaps_ms: &'a [(u64, u64)],
    pub video_start_us: i64,
}

/// The video graph, ending in `[v]`. Only numbers Rust worked out go in.
pub fn cam_filter(chain: &CamChain) -> String {
    let rect = cam_rect(chain.frame, &chain.layout);
    let mut cam = format!(
        "[{}:v]setpts=PTS+{:.6}/TB,trim=end={:.6}",
        chain.cam_input, chain.timing.shift_s, chain.timing.end_s
    );
    if let Some(crop) = cam_crop(chain.cam, chain.layout.shape) {
        cam += &format!(",crop={}:{}:{}:{}", crop.w, crop.h, crop.x, crop.y);
    }
    if chain.layout.mirror {
        cam += ",hflip";
    }
    cam += &format!(",scale={}:{},format=yuva420p[cam]", rect.w, rect.h);
    let mut graph = cam;
    let mut overlaid = "cam";
    if let Some(mask) = chain.mask_input {
        graph += &format!(";[{mask}:v]format=gray[mask];[cam][mask]alphamerge[camm]");
        overlaid = "camm";
    }
    let enable = gap_enable(chain.gaps_ms, chain.video_start_us);
    graph += &format!(
        ";[{}][{overlaid}]overlay=x={}:y={}:eval=init:eof_action=repeat{enable}[v]",
        chain.main, rect.x, rect.y
    );
    graph
}

/// Where the parts of a 9:16 frame go, in its pixels. With a cam, the cam
/// fills a band on top and the game, scaled to the rest's height, is cut to a
/// window that `game_x` slides; without one, the game fits the width and sits
/// in the middle. A blurred copy of the game fills everything behind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PortraitGeometry {
    pub out: (u32, u32),
    pub band: Option<Rect>,
    /// The game scaled to this before it is cut.
    pub game_scaled: (u32, u32),
    /// The cut of the scaled game.
    pub game_crop: Rect,
    /// Where the cut lands in the frame.
    pub game_at: (u32, u32),
}

pub fn portrait_geometry(
    game: (u32, u32),
    cam: Option<(u32, u32)>,
    layout: &crate::model::PortraitLayout,
    out: (u32, u32),
) -> PortraitGeometry {
    let (gw, gh) = (game.0.max(2) as f64, game.1.max(2) as f64);
    let (ow, oh) = out;
    let Some(_) = cam else {
        let h = even(gh * ow as f64 / gw).min(oh);
        return PortraitGeometry {
            out,
            band: None,
            game_scaled: (ow, h),
            game_crop: Rect { x: 0, y: 0, w: ow, h },
            game_at: (0, even((oh - h) as f64 / 2.0).min(oh - h)),
        };
    };
    let band_h = even(layout.split.clamp(0.25, 0.5) as f64 * oh as f64);
    let area_h = oh - band_h;
    // Cover the area: the larger scale wins, the window cuts the rest.
    let scale = (ow as f64 / gw).max(area_h as f64 / gh);
    let scaled = (even(gw * scale).max(ow), even(gh * scale).max(area_h));
    let free_x = scaled.0 - ow;
    let x = even(layout.game_x.clamp(0.0, 1.0) as f64 * free_x as f64).min(free_x);
    let y = even((scaled.1 - area_h) as f64 / 2.0).min(scaled.1 - area_h);
    PortraitGeometry {
        out,
        band: Some(Rect { x: 0, y: 0, w: ow, h: band_h }),
        game_scaled: scaled,
        game_crop: Rect { x, y, w: ow, h: area_h },
        game_at: (0, band_h),
    }
}

/// The cam's side of a 9:16 graph.
pub struct PortraitCam {
    pub input: usize,
    pub size: (u32, u32),
    pub timing: CamTiming,
    /// Stretches without a camera, in this cut's milliseconds.
    pub gaps_ms: Vec<(u64, u64)>,
    pub video_start_us: i64,
    pub mirror: bool,
}

/// The watermark drawn again on the 9:16 frame.
pub struct MarkInput {
    pub input: usize,
    pub rect: Rect,
}

/// The game's own mark sits in its bottom-left corner: a window that starts
/// at the left edge keeps it, any other cuts it — then, with the watermark
/// on, it is drawn again on the 9:16 frame (Open for Fabi 6 (a)).
pub fn mark_redrawn(watermark_on: bool, geometry: &PortraitGeometry) -> bool {
    watermark_on && geometry.game_crop.x > 0
}

/// `enable` that hides the cam where it was gone while recording.
fn gap_enable(gaps_ms: &[(u64, u64)], video_start_us: i64) -> String {
    if gaps_ms.is_empty() {
        return String::new();
    }
    let start = video_start_us as f64 / 1e6;
    let hidden: Vec<String> = gaps_ms
        .iter()
        .map(|(a, b)| format!("between(t,{:.6},{:.6})", start + *a as f64 / 1000.0, start + *b as f64 / 1000.0))
        .collect();
    format!(":enable='not({})'", hidden.join("+"))
}

/// The 9:16 video graph from input 0 (the game), ending in `[v]`. Only
/// numbers Rust worked out go in; overlays on a blurred fill, never vstack.
pub fn portrait_filter(geometry: &PortraitGeometry, cam: Option<&PortraitCam>, mark: Option<&MarkInput>) -> String {
    let (ow, oh) = geometry.out;
    let (sw, sh) = geometry.game_scaled;
    let c = geometry.game_crop;
    let mut graph = format!(
        "[0:v]split=2[g0][g1];\
         [g0]scale={ow}:{oh}:force_original_aspect_ratio=increase,crop={ow}:{oh},boxblur=24:2[fill];\
         [g1]scale={sw}:{sh},crop={}:{}:{}:{}[game];\
         [fill][game]overlay=x={}:y={}[base]",
        c.w, c.h, c.x, c.y, geometry.game_at.0, geometry.game_at.1
    );
    let mut last = "base".to_string();
    if let (Some(cam), Some(band)) = (cam, geometry.band) {
        // Cut the cam to the band's shape around its middle, then fill it.
        let (cw, ch) = (cam.size.0.max(2) as f64, cam.size.1.max(2) as f64);
        let band_aspect = band.w as f64 / band.h as f64;
        let (kw, kh) = if cw / ch > band_aspect { (even(ch * band_aspect), cam.size.1 & !1) } else { (cam.size.0 & !1, even(cw / band_aspect)) };
        let (kx, ky) = (even((cw - kw as f64) / 2.0), even((ch - kh as f64) / 2.0));
        graph += &format!(
            ";[{}:v]setpts=PTS+{:.6}/TB,trim=end={:.6},crop={kw}:{kh}:{kx}:{ky}{},scale={}:{}[cam];[{last}][cam]overlay=x={}:y={}:eval=init:eof_action=pass{}[withcam]",
            cam.input,
            cam.timing.shift_s,
            cam.timing.end_s,
            if cam.mirror { ",hflip" } else { "" },
            band.w,
            band.h,
            band.x,
            band.y,
            gap_enable(&cam.gaps_ms, cam.video_start_us)
        );
        last = "withcam".into();
    }
    if let Some(mark) = mark {
        graph += &format!(
            ";[{}:v]format=rgba,colorchannelmixer=aa={},scale={}:{}[mark];[{last}][mark]overlay=x={}:y={}[marked]",
            mark.input,
            crate::watermark::OPACITY,
            mark.rect.w,
            mark.rect.h,
            mark.rect.x,
            mark.rect.y
        );
        last = "marked".into();
    }
    graph + &format!(";[{last}]null[v]")
}

/// Everything one background composite needs.
pub struct CompositeJob<'a> {
    pub clip: &'a Path,
    pub cam: &'a Path,
    pub mask: Option<&'a Path>,
    pub output: &'a Path,
    pub frame: (u32, u32),
    pub facts: &'a FacecamFacts,
    pub layout: CamLayout,
    pub delay_ms: i32,
    pub video_start_us: i64,
    pub start_ms: u64,
    pub length_ms: u64,
    pub gaps_ms: &'a [(u64, u64)],
    pub encoder: EncoderId,
    pub quality: u32,
    pub gop: u32,
    pub hwaccel: bool,
}

pub fn composite_arguments(job: &CompositeJob) -> Vec<String> {
    let timing = cam_timing(job.facts.cam_offset_us, job.delay_ms, job.video_start_us, job.start_ms, job.length_ms);
    let mut args: Vec<String> =
        ["-y", "-hide_banner", "-loglevel", "error", "-progress", "pipe:1", "-nostats"].into_iter().map(String::from).collect();
    if job.hwaccel {
        args.extend(["-hwaccel".into(), "d3d11va".into()]);
    }
    args.extend(["-threads".into(), "4".into(), "-i".into(), job.clip.to_string_lossy().into_owned()]);
    if let Some(seek) = timing.seek_s {
        args.extend(["-ss".into(), format!("{seek:.6}")]);
    }
    args.extend(["-i".into(), job.cam.to_string_lossy().into_owned()]);
    if let Some(mask) = job.mask {
        // One picture, looped: alphamerge needs it for every cam frame.
        args.extend(["-loop".into(), "1".into(), "-i".into(), mask.to_string_lossy().into_owned()]);
    }
    let graph = cam_filter(&CamChain {
        cam_input: 1,
        mask_input: job.mask.map(|_| 2),
        main: "0:v",
        frame: job.frame,
        cam: (job.facts.width, job.facts.height),
        layout: job.layout,
        timing,
        gaps_ms: job.gaps_ms,
        video_start_us: job.video_start_us,
    });
    args.extend(["-filter_complex_threads".into(), "2".into(), "-filter_complex".into(), graph]);
    args.extend(["-map", "[v]", "-map", "0:a?", "-c:a", "copy"].into_iter().map(String::from));
    args.extend(crate::edit::video_args_at(job.encoder, job.quality, crate::edit::Speed::Save));
    args.extend([
        "-g".into(),
        job.gop.max(1).to_string(),
        "-avoid_negative_ts".into(),
        "make_zero".into(),
        "-t".into(),
        format!("{:.3}", job.length_ms as f64 / 1000.0),
        job.output.to_string_lossy().into_owned(),
    ]);
    args
}

/// Hardware first, then x264 with hardware decoding, then x264 alone.
pub fn attempts(encoder: EncoderId) -> Vec<(EncoderId, bool)> {
    let mut out = Vec::new();
    for attempt in [(encoder, true), (EncoderId::X264, true), (EncoderId::X264, false)] {
        if !out.contains(&attempt) {
            out.push(attempt);
        }
    }
    out
}

// ---------------------------------------------------------------- phase two

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use tauri::{Emitter, Manager};

use crate::facecam::FacecamApplied;
use crate::model::{ClipFacecam, FacecamState};

/// The background composites: one at a time, in the order they came; a swap
/// waiting for a player to let go of the file comes back after the others.
#[derive(Debug, Default)]
pub struct Queue {
    waiting: VecDeque<String>,
    later: Vec<(String, Instant)>,
    running: Option<String>,
}

impl Queue {
    /// `false` when the clip is queued or running already.
    pub fn push(&mut self, id: &str) -> bool {
        if self.busy(id) {
            return false;
        }
        self.waiting.push_back(id.to_string());
        true
    }

    /// A swap tried again at `at`.
    pub fn push_later(&mut self, id: &str, at: Instant) -> bool {
        if self.waiting.iter().any(|w| w == id) || self.later.iter().any(|(l, _)| l == id) {
            return false;
        }
        self.later.push((id.to_string(), at));
        true
    }

    /// The waiting first, then a later one whose time has come.
    pub fn next(&mut self, now: Instant) -> Option<String> {
        let id = self.waiting.pop_front().or_else(|| {
            let due = self.later.iter().position(|(_, at)| *at <= now)?;
            Some(self.later.remove(due).0)
        })?;
        self.running = Some(id.clone());
        Some(id)
    }

    pub fn done(&mut self, id: &str) {
        if self.running.as_deref() == Some(id) {
            self.running = None;
        }
    }

    pub fn busy(&self, id: &str) -> bool {
        self.running.as_deref() == Some(id)
            || self.waiting.iter().any(|w| w == id)
            || self.later.iter().any(|(l, _)| l == id)
    }

    /// Takes a waiting job out; a running one is waited for through the clip lock.
    pub fn drop_waiting(&mut self, id: &str) -> bool {
        let before = self.waiting.len() + self.later.len();
        self.waiting.retain(|w| w != id);
        self.later.retain(|(l, _)| l != id);
        before != self.waiting.len() + self.later.len()
    }

    fn earliest_later(&self) -> Option<Instant> {
        self.later.iter().map(|(_, at)| *at).min()
    }
}

/// Room for one composite: the clip's drive holds the composite next to the
/// clip, the data drive (when it is another) the clean clip filed away.
pub fn room_for_composite(clip_bytes: u64, free_clip_drive: u64, free_data_drive: u64, same_drive: bool) -> bool {
    const SLACK: u64 = 64 * 1024 * 1024;
    let composite = clip_bytes + clip_bytes / 5 + SLACK;
    if same_drive {
        free_clip_drive >= composite
    } else {
        free_clip_drive >= composite && free_data_drive >= clip_bytes + SLACK
    }
}

/// What a start finds after a quit or a crash in the middle of phase two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    Nothing,
    /// The clean clip was filed away and the composite never arrived.
    PutCleanBack,
    /// The composite is in place; only the facts did not say so yet.
    FinishSwap,
    Requeue,
}

pub fn after_crash(facts: &FacecamFacts, clip_bytes: Option<u64>, clean_in_store: bool, state: FacecamState) -> Recovery {
    if clip_bytes.is_none() {
        return if clean_in_store { Recovery::PutCleanBack } else { Recovery::Nothing };
    }
    if facts.applying.as_ref().is_some_and(|a| Some(a.size_bytes) == clip_bytes) {
        return Recovery::FinishSwap;
    }
    if state == FacecamState::Pending {
        return Recovery::Requeue;
    }
    Recovery::Nothing
}

/// How long a swap waits for a player to let go of the file, about a minute.
pub fn swap_waits() -> [Duration; 5] {
    [2, 5, 10, 20, 30].map(Duration::from_secs)
}

/// After those, the swap is tried again this often while the app runs.
pub const SWAP_RETRY_EVERY: Duration = Duration::from_secs(5 * 60);

/// A swap failing for a day is given up.
pub fn swap_given_up(failing_since_ms: Option<i64>, now_ms: i64) -> bool {
    failing_since_ms.is_some_and(|since| now_ms - since >= 24 * 3_600_000)
}

static QUEUE: LazyLock<(Mutex<Queue>, Condvar)> = LazyLock::new(|| (Mutex::new(Queue::default()), Condvar::new()));

pub fn enqueue(clip_id: &str) {
    let (queue, changed) = &*QUEUE;
    if queue.lock().push(clip_id) {
        changed.notify_all();
    }
}

/// Drops a queued job; `false` when none was waiting.
pub fn cancel(clip_id: &str) -> bool {
    let (queue, changed) = &*QUEUE;
    let dropped = queue.lock().drop_waiting(clip_id);
    changed.notify_all();
    dropped
}

/// Waits until the clip has no job queued or running, up to `limit`. Called
/// before taking the clip's lock, never while holding it.
pub fn wait_settled(clip_id: &str, limit: Duration) -> bool {
    let (queue, changed) = &*QUEUE;
    let deadline = Instant::now() + limit;
    let mut q = queue.lock();
    while q.busy(clip_id) {
        if changed.wait_until(&mut q, deadline).timed_out() {
            return !q.busy(clip_id);
        }
    }
    true
}

/// The one composite worker, below normal priority through its ffmpeg.
pub fn start(app: &tauri::AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new().name("clippiboy-composite".into()).spawn(move || loop {
        let id = {
            let (queue, changed) = &*QUEUE;
            let mut q = queue.lock();
            loop {
                if let Some(id) = q.next(Instant::now()) {
                    break id;
                }
                match q.earliest_later() {
                    Some(at) => {
                        changed.wait_until(&mut q, at);
                    }
                    None => changed.wait(&mut q),
                }
            }
        };
        // The queue's lock is released before the clip's is taken.
        let later = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&app, &id))).unwrap_or_else(|_| {
            log::error!("cam composite of {id} panicked");
            None
        });
        let (queue, changed) = &*QUEUE;
        let mut q = queue.lock();
        q.done(&id);
        if let Some(at) = later {
            q.push_later(&id, at);
        }
        changed.notify_all();
    });
    if let Err(err) = spawned {
        log::error!("composite thread: {err}");
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn file_size(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

/// `<clip>.<pid>.cam.mp4`, next to the clip.
fn temp_path(clip: &Path) -> PathBuf {
    let mut name = clip.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.cam.mp4", std::process::id()));
    clip.with_file_name(name)
}

/// Composites left by an earlier run of the app.
fn stale_temps(clip: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(name)) = (clip.parent(), clip.file_name()) else { return Vec::new() };
    let prefix = format!("{}.", name.to_string_lossy());
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    let n = p.file_name().unwrap_or_default().to_string_lossy();
                    n.starts_with(&prefix) && n.ends_with(".cam.mp4")
                })
                .collect()
        })
        .unwrap_or_default()
}

fn set_state(app: &tauri::AppHandle, id: &str, cam: &ClipFacecam, state: FacecamState) {
    let library = &app.state::<crate::state::AppState>().library;
    let next = ClipFacecam { state, ..cam.clone() };
    if let Some(Err(err)) = crate::clips::with_library(library, |lib| lib.set_facecam(id, Some(&next))) {
        log::error!("could not note the cam's state: {err}");
    }
    let _ = app.emit("clips-changed", ());
}

fn fail(app: &tauri::AppHandle, id: &str, cam: &ClipFacecam, message: &str) {
    log::warn!("cam for clip {id}: {message}");
    set_state(app, id, cam, FacecamState::Failed);
    crate::notify(app, "error", message);
}

/// Files the clean clip away and puts the composite in its place. A clip open
/// in a player refuses both; the clean clip then goes back where it was.
fn swap(id: &str, clip: &Path, temp: &Path) -> Result<(), String> {
    let clean = crate::edit::video_path(id);
    let moved = if clean.exists() {
        false
    } else {
        crate::edit::move_across(clip, &clean).map_err(|err| format!("filing the clean clip: {err}"))?;
        true
    };
    if let Err(err) = crate::muxer::replace_file(temp, clip) {
        if moved {
            let _ = crate::edit::move_across(&clean, clip);
        }
        return Err(err);
    }
    Ok(())
}

/// After a swap: the facts say what the file shows, the thumbnail and the row
/// follow the new file.
fn finish(app: &tauri::AppHandle, id: &str, clip: &Path, cam: &ClipFacecam, mut facts: FacecamFacts) {
    facts.applied = facts.applying.take();
    facts.swap_failing_since_ms = None;
    if let Err(err) = crate::facecam::write_facts(id, &facts) {
        log::warn!("facecam.json for {id}: {err}");
    }
    if let Err(err) = crate::thumbs::make(clip, id) {
        log::warn!("thumbnail for {id}: {err}");
    }
    let length = crate::muxer::probe_duration_ms(clip);
    let size = file_size(clip);
    let library = &app.state::<crate::state::AppState>().library;
    if let (Some(length), Some(size)) = (length, size) {
        let _ = crate::clips::with_library(library, |lib| lib.set_file_state(id, length, size));
    }
    log::info!("cam added to clip {id}");
    set_state(app, id, cam, FacecamState::Ready);
}

/// One composite. `Some(at)` when the swap has to be tried again later.
fn job(app: &tauri::AppHandle, id: &str) -> Option<Instant> {
    let state = app.state::<crate::state::AppState>();
    let clip_lock = crate::edit::clip_lock(id);
    let guard = clip_lock.lock();
    let clip = crate::clips::fresh(&state.library, id)?;
    let cam = clip.facecam.clone()?;
    if cam.state != FacecamState::Pending {
        return None;
    }
    let Some(mut facts) = crate::facecam::read_facts(id) else {
        let _ = crate::clips::with_library(&state.library, |lib| lib.set_facecam(id, None));
        let _ = app.emit("clips-changed", ());
        return None;
    };
    let clip_path = PathBuf::from(&clip.path);
    let temp = temp_path(&clip_path);
    // A swap tried again finds its composite still there.
    let composited = facts.applying.as_ref().is_some_and(|a| file_size(&temp) == Some(a.size_bytes));
    if !composited {
        let config = state.config_snapshot();
        let data = crate::config::data_dir();
        let clip_dir = clip_path.parent().map(Path::to_path_buf).unwrap_or_else(|| data.clone());
        let clip_bytes = file_size(&clip_path).unwrap_or(clip.size_bytes);
        let room = room_for_composite(
            clip_bytes,
            crate::disk::free_bytes(&clip_dir).unwrap_or(u64::MAX),
            crate::disk::free_bytes(&data).unwrap_or(u64::MAX),
            crate::disk::same_volume(&clip_dir, &data),
        );
        if !room {
            fail(app, id, &cam, "Not enough free space to add the cam — the clip was kept without it.");
            return None;
        }
        let video_start_us = crate::muxer::probe_video_start_us(&clip_path).unwrap_or(0);
        let frame = (clip.width, clip.height);
        let rect = cam_rect(frame, &cam.layout);
        let mask = mask_gray8(cam.layout.shape, rect.w, rect.h).and_then(|pixels| {
            let dir = data.join("temp");
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join(format!("{}_mask_{}x{}.png", id.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "_"), rect.w, rect.h));
            write_mask_png(&path, rect.w, rect.h, &pixels).ok().map(|_| path)
        });
        let length_ms = clip.duration_ms;
        let encoder = crate::encode::effective(config.recording.encoder);
        let mut done = false;
        for (encoder, hwaccel) in attempts(encoder) {
            let args = composite_arguments(&CompositeJob {
                clip: &clip_path,
                cam: &crate::facecam::cam_path(id),
                mask: mask.as_deref(),
                output: &temp,
                frame,
                facts: &facts,
                layout: cam.layout,
                delay_ms: cam.delay_ms,
                video_start_us,
                start_ms: 0,
                length_ms,
                gaps_ms: &facts.gaps_ms,
                encoder,
                quality: config.recording.quality,
                gop: facts.game_gop,
                hwaccel,
            });
            match crate::edit::run_command_with_progress(crate::muxer::background("ffmpeg"), &args, length_ms, &|_| {}) {
                Ok(()) => {
                    done = true;
                    break;
                }
                Err(err) => {
                    log::warn!("cam composite of {id} with {encoder:?} (hardware decoding {hwaccel}): {err}");
                    let _ = std::fs::remove_file(&temp);
                }
            }
        }
        if let Some(mask) = &mask {
            let _ = std::fs::remove_file(mask);
        }
        if !done {
            fail(app, id, &cam, "The cam couldn't be added — try again from the editor");
            return None;
        }
        let size = file_size(&temp)?;
        facts.video_start_us = Some(video_start_us);
        facts.applying = Some(FacecamApplied {
            layout: cam.layout,
            delay_ms: cam.delay_ms,
            start_ms: 0,
            length_ms: crate::muxer::probe_duration_ms(&temp).unwrap_or(length_ms),
            size_bytes: size,
        });
        if let Err(err) = crate::facecam::write_facts(id, &facts) {
            log::warn!("facecam.json for {id}: {err}");
        }
    }
    if swap(id, &clip_path, &temp).is_ok() {
        finish(app, id, &clip_path, &cam, facts);
        return None;
    }
    // A player holds the file. Wait for it without holding the clip.
    if facts.swap_failing_since_ms.is_none() {
        facts.swap_failing_since_ms = Some(now_ms());
        let _ = crate::facecam::write_facts(id, &facts);
    }
    if swap_given_up(facts.swap_failing_since_ms, now_ms()) {
        let _ = std::fs::remove_file(&temp);
        fail(app, id, &cam, "The cam couldn't be added — try again from the editor");
        return None;
    }
    drop(guard);
    for wait in swap_waits() {
        std::thread::sleep(wait);
        let _guard = clip_lock.lock();
        let still = crate::clips::fresh(&state.library, id)
            .and_then(|c| c.facecam)
            .is_some_and(|c| c.state == FacecamState::Pending);
        if !still {
            let _ = std::fs::remove_file(&temp);
            return None;
        }
        if swap(id, &clip_path, &temp).is_ok() {
            finish(app, id, &clip_path, &cam, facts);
            return None;
        }
    }
    log::info!("cam for clip {id}: the file is still in use — tried again in five minutes");
    Some(Instant::now() + SWAP_RETRY_EVERY)
}

/// At start: every clip whose cam was on its way when the app last stopped.
/// Takes each clip's lock, like the other start-up passes.
pub fn recover(library: &parking_lot::Mutex<Option<crate::clips::Library>>) -> usize {
    let clips = crate::clips::with_library(library, |lib| lib.list()).and_then(Result::ok).unwrap_or_default();
    let mut changed = 0;
    for clip in clips {
        let Some(cam) = clip.facecam.clone() else { continue };
        if !crate::facecam::has_store(&clip.id) {
            continue;
        }
        let lock = crate::edit::clip_lock(&clip.id);
        let _guard = lock.lock();
        let Some(mut facts) = crate::facecam::read_facts(&clip.id) else { continue };
        let clip_path = PathBuf::from(&clip.path);
        for stale in stale_temps(&clip_path) {
            let _ = std::fs::remove_file(stale);
        }
        let clean = crate::edit::video_path(&clip.id);
        match after_crash(&facts, file_size(&clip_path), clean.exists(), cam.state) {
            Recovery::Nothing => continue,
            Recovery::PutCleanBack => {
                if let Err(err) = crate::edit::move_across(&clean, &clip_path) {
                    log::warn!("could not put the clean clip {} back: {err}", clip.id);
                    continue;
                }
                facts.applying = None;
                let _ = crate::facecam::write_facts(&clip.id, &facts);
                enqueue(&clip.id);
            }
            Recovery::FinishSwap => {
                facts.applied = facts.applying.take();
                facts.swap_failing_since_ms = None;
                let _ = crate::facecam::write_facts(&clip.id, &facts);
                let next = ClipFacecam { state: FacecamState::Ready, ..cam };
                let _ = crate::clips::with_library(library, |lib| lib.set_facecam(&clip.id, Some(&next)));
            }
            Recovery::Requeue => {
                facts.applying = None;
                let _ = crate::facecam::write_facts(&clip.id, &facts);
                enqueue(&clip.id);
            }
        }
        changed += 1;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Rect;
    use crate::model::{CamLayout, CamShape, EncoderId};
    use std::path::Path;
    use std::time::Instant;

    fn layout(x: f32, y: f32, size: f32, shape: CamShape, mirror: bool) -> CamLayout {
        CamLayout { visible: true, x, y, size, shape, mirror }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn portrait(split: f32, game_x: f32) -> crate::model::PortraitLayout {
        crate::model::PortraitLayout { split, game_x }
    }

    fn even_all(g: &PortraitGeometry) -> bool {
        let rects = [g.game_crop].into_iter().chain(g.band);
        rects.into_iter().all(|r| [r.x, r.y, r.w, r.h].iter().all(|v| v % 2 == 0))
            && [g.game_scaled.0, g.game_scaled.1, g.game_at.0, g.game_at.1, g.out.0, g.out.1].iter().all(|v| v % 2 == 0)
    }

    #[test]
    fn the_game_window_follows_game_x() {
        let at = |x| portrait_geometry((1920, 1080), Some((1280, 720)), &portrait(0.35, x), (1080, 1920));
        let left = at(0.0);
        assert_eq!(left.band, Some(Rect { x: 0, y: 0, w: 1080, h: 672 }));
        assert_eq!(left.game_scaled, (2218, 1248));
        assert_eq!((left.game_crop.x, left.game_crop.w, left.game_crop.h), (0, 1080, 1248));
        assert_eq!(left.game_at, (0, 672));
        assert_eq!(at(1.0).game_crop.x, 2218 - 1080);
        let mid = at(0.5).game_crop.x;
        assert!(mid.abs_diff((2218 - 1080) / 2) <= 2 && mid % 2 == 0, "{mid}");
    }

    #[test]
    fn every_size_and_position_is_even() {
        for game in [(1920, 1080), (2560, 1080), (1280, 720), (1366, 768)] {
            for split in [0.25, 0.35, 0.5] {
                for x in [0.0, 0.33, 1.0] {
                    for cam in [Some((1280, 720)), None] {
                        let g = portrait_geometry(game, cam, &portrait(split, x), (1080, 1920));
                        assert!(even_all(&g), "{game:?} {split} {x} {cam:?}: {g:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn portrait_without_a_cam_centres_the_game_on_a_blurred_fill() {
        let g = portrait_geometry((1920, 1080), None, &portrait(0.35, 0.5), (1080, 1920));
        assert_eq!(g.band, None);
        assert_eq!(g.game_scaled, (1080, 608));
        assert_eq!(g.game_at, (0, 656));
        let graph = portrait_filter(&g, None, None);
        assert!(graph.contains("boxblur"), "{graph}");
        assert!(graph.contains("overlay=x=0:y=656"), "{graph}");
        assert!(!graph.contains("[1:v]"), "no cam input: {graph}");
        assert!(graph.ends_with("[v]"), "{graph}");
    }

    fn portrait_cam(gaps: Vec<(u64, u64)>) -> PortraitCam {
        PortraitCam {
            input: 1,
            size: (1280, 720),
            timing: cam_timing(-412_000, -40, 21_333, 0, 30_000),
            gaps_ms: gaps,
            video_start_us: 21_333,
            mirror: true,
        }
    }

    #[test]
    fn the_band_falls_back_to_a_blurred_strip_while_the_cam_is_absent() {
        let g = portrait_geometry((1920, 1080), Some((1280, 720)), &portrait(0.35, 0.5), (1080, 1920));
        let graph = portrait_filter(&g, Some(&portrait_cam(vec![(1500, 3000)])), None);
        // The blurred fill covers the whole frame, band included; the cam
        // leaves it showing in a gap and after its end.
        assert!(graph.contains("boxblur"), "{graph}");
        assert!(graph.contains("enable='not(between(t,1.521333,3.021333))'"), "{graph}");
        assert!(graph.contains("eof_action=pass"), "{graph}");
        assert!(graph.contains("hflip"), "mirrored: {graph}");
        assert!(graph.contains("scale=1080:672"), "the cam fills the band: {graph}");
    }

    #[test]
    fn portrait_never_uses_vstack_or_shortest() {
        let g = portrait_geometry((1920, 1080), Some((1280, 720)), &portrait(0.35, 0.5), (1080, 1920));
        let graph = portrait_filter(&g, Some(&portrait_cam(vec![])), None);
        assert!(!graph.contains("vstack") && !graph.contains("shortest"), "{graph}");
    }

    #[test]
    fn the_mark_is_drawn_again_when_the_window_cuts_it_out() {
        let at = |x| portrait_geometry((1920, 1080), Some((1280, 720)), &portrait(0.35, x), (1080, 1920));
        assert!(mark_redrawn(true, &at(0.5)));
        assert!(!mark_redrawn(true, &at(0.0)), "the window keeps the bottom-left corner and its mark");
        assert!(!mark_redrawn(false, &at(0.5)), "no mark while the watermark is off");
        let mark = MarkInput { input: 3, rect: Rect { x: 40, y: 1800, w: 220, h: 60 } };
        let graph = portrait_filter(&at(0.5), Some(&portrait_cam(vec![])), Some(&mark));
        assert!(graph.contains("[3:v]"), "{graph}");
        assert!(graph.contains("overlay=x=40:y=1800"), "{graph}");
    }

    #[test]
    fn a_clip_is_never_queued_twice() {
        let mut q = Queue::default();
        assert!(q.push("a"));
        assert!(!q.push("a"));
        assert_eq!(q.next(Instant::now()).as_deref(), Some("a"));
        assert!(!q.push("a"), "running counts too");
    }

    #[test]
    fn a_running_clip_counts_as_busy() {
        let mut q = Queue::default();
        q.push("a");
        assert!(q.busy("a"));
        q.next(Instant::now());
        assert!(q.busy("a"));
        q.done("a");
        assert!(!q.busy("a"));
    }

    #[test]
    fn jobs_run_in_the_order_they_came() {
        let mut q = Queue::default();
        q.push("a");
        q.push("b");
        let now = Instant::now();
        assert_eq!(q.next(now).as_deref(), Some("a"));
        q.done("a");
        assert_eq!(q.next(now).as_deref(), Some("b"));
    }

    #[test]
    fn a_deleted_clip_leaves_the_queue() {
        let mut q = Queue::default();
        q.push("a");
        q.push("b");
        q.next(Instant::now());
        assert!(!q.drop_waiting("a"), "the running one is waited for, not dropped");
        assert!(q.drop_waiting("b"));
        assert!(!q.busy("b"));
    }

    #[test]
    fn a_swap_retry_waits_its_turn() {
        let mut q = Queue::default();
        let now = Instant::now();
        assert!(q.push_later("a", now + SWAP_RETRY_EVERY));
        q.push("b");
        assert_eq!(q.next(now).as_deref(), Some("b"));
        q.done("b");
        assert_eq!(q.next(now), None);
        assert_eq!(q.next(now + SWAP_RETRY_EVERY).as_deref(), Some("a"));
    }

    #[test]
    fn a_composite_needs_room_for_the_clip_once_more() {
        assert!(room_for_composite(1_000_000_000, 1_300_000_000, 0, true));
        assert!(!room_for_composite(1_000_000_000, 1_200_000_000, 0, true));
        assert!(room_for_composite(1_000_000_000, 1_300_000_000, 1_100_000_000, false));
        assert!(!room_for_composite(1_000_000_000, 1_300_000_000, 900_000_000, false));
    }

    fn crash_facts(applying: Option<u64>, applied: bool, failing: Option<i64>) -> crate::facecam::FacecamFacts {
        let mark = |size| crate::facecam::FacecamApplied {
            layout: CamLayout::default(),
            delay_ms: -40,
            start_ms: 0,
            length_ms: 30_000,
            size_bytes: size,
        };
        crate::facecam::FacecamFacts {
            applying: applying.map(mark),
            applied: applied.then(|| mark(100)),
            swap_failing_since_ms: failing,
            ..facts()
        }
    }

    #[test]
    fn a_crash_mid_composite_is_resolved_from_the_file_size() {
        use crate::model::FacecamState::{Pending, Ready};
        assert_eq!(after_crash(&crash_facts(Some(100), false, None), None, true, Pending), Recovery::PutCleanBack);
        assert_eq!(after_crash(&crash_facts(Some(100), false, None), Some(100), true, Pending), Recovery::FinishSwap);
        assert_eq!(after_crash(&crash_facts(Some(100), false, None), Some(90), true, Pending), Recovery::Requeue);
        assert_eq!(after_crash(&crash_facts(None, false, None), Some(90), false, Pending), Recovery::Requeue);
        assert_eq!(after_crash(&crash_facts(None, true, None), Some(100), true, Ready), Recovery::Nothing);
        assert_eq!(after_crash(&crash_facts(None, false, None), Some(100), false, Ready), Recovery::Nothing);
    }

    #[test]
    fn a_clip_open_in_a_player_is_waited_for_not_failed() {
        let total: u64 = swap_waits().iter().map(|d| d.as_secs()).sum();
        assert!((60..=75).contains(&total), "{total}");
        let t = 1_800_000_000_000;
        let hour = 3_600_000;
        assert!(!swap_given_up(Some(t), t + 23 * hour));
        assert!(swap_given_up(Some(t), t + 25 * hour));
        assert!(!swap_given_up(None, t));
        assert_eq!(
            after_crash(&crash_facts(None, false, Some(t)), Some(90), false, crate::model::FacecamState::Pending),
            Recovery::Requeue
        );
    }

    #[test]
    fn the_default_cam_sits_bottom_right_inside_the_margin() {
        assert_eq!(cam_rect((1920, 1080), &CamLayout::default()), Rect { x: 1412, y: 782, w: 480, h: 270 });
        let top_left = cam_rect((1920, 1080), &layout(0.0, 0.0, 0.25, CamShape::Rounded, false));
        assert_eq!((top_left.x, top_left.y), (28, 28));
    }

    #[test]
    fn a_circle_is_cropped_square() {
        let r = cam_rect((1920, 1080), &layout(1.0, 1.0, 0.25, CamShape::Circle, false));
        assert_eq!((r.w, r.h, r.x), (270, 270, 1622));
        assert_eq!(cam_crop((1280, 720), CamShape::Circle), Some(Rect { x: 280, y: 0, w: 720, h: 720 }));
        assert_eq!(cam_crop((1280, 720), CamShape::Rounded), None);
    }

    #[test]
    fn every_number_is_even_and_inside_the_frame() {
        for frame in [(1366, 768), (2560, 1080), (3440, 1440), (1080, 1920), (1920, 1080)] {
            for size in [0.12, 0.5] {
                for (x, y) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                    for shape in [CamShape::Rect, CamShape::Rounded, CamShape::Circle] {
                        let r = cam_rect(frame, &layout(x, y, size, shape, false));
                        let m = margin(frame.1);
                        assert!([r.x, r.y, r.w, r.h].iter().all(|v| v % 2 == 0), "{frame:?} {r:?}");
                        assert!(r.x + r.w + m <= frame.0 && r.y + r.h + m <= frame.1, "{frame:?} {size} {r:?}");
                        assert!(r.x >= m && r.y >= m);
                    }
                }
            }
        }
    }

    #[test]
    fn a_portrait_screen_narrows_the_cam_instead_of_running_off_it() {
        assert_eq!(
            cam_rect((1080, 1920), &layout(1.0, 1.0, 0.5, CamShape::Rounded, false)),
            Rect { x: 48, y: 1318, w: 984, h: 554 }
        );
    }

    #[test]
    fn the_cam_gets_a_seek_exactly_when_it_starts_before_the_clip() {
        let t = cam_timing(-412_000, 0, 21_333, 0, 30_000);
        assert!(close(t.seek_s.unwrap(), 0.412));
        assert!(close(t.shift_s, 0.021333));
        let t = cam_timing(1_500_000, 0, 21_333, 0, 30_000);
        assert_eq!(t.seek_s, None);
        assert!(close(t.shift_s, 1.521333));
    }

    #[test]
    fn the_video_start_is_read_off_ffprobe() {
        assert_eq!(crate::muxer::parse_start_us("0.021333\n"), Some(21_333));
        assert_eq!(crate::muxer::parse_start_us("N/A"), None);
    }

    #[test]
    fn the_delay_moves_the_cam() {
        assert!(close(cam_timing(-412_000, -40, 21_333, 0, 30_000).seek_s.unwrap(), 0.452));
    }

    #[test]
    fn a_trimmed_clip_starts_the_cam_at_its_place_in_the_original() {
        assert!(close(cam_timing(-412_000, 0, 21_333, 4000, 8000).seek_s.unwrap(), 4.412));
    }

    fn facts() -> crate::facecam::FacecamFacts {
        crate::facecam::FacecamFacts {
            version: 1,
            width: 1280,
            height: 720,
            fps: 30,
            cam_offset_us: -412_000,
            frames: 912,
            clip_length_us: 30_000_000,
            gaps_ms: vec![],
            clock: crate::camera::ClockKind::Device,
            camera: "C920".into(),
            encoder: Some(EncoderId::Nvenc),
            game_gop: 120,
            video_start_us: None,
            applied: None,
            applying: None,
            swap_failing_since_ms: None,
        }
    }

    fn args_for(cam: CamLayout, mask: Option<&Path>, gaps: &[(u64, u64)], hwaccel: bool) -> Vec<String> {
        let facts = facts();
        composite_arguments(&CompositeJob {
            clip: Path::new("clip.mp4"),
            cam: Path::new("cam.mp4"),
            mask,
            output: Path::new("out.mp4"),
            frame: (1920, 1080),
            facts: &facts,
            layout: cam,
            delay_ms: 0,
            video_start_us: 21_333,
            start_ms: 0,
            length_ms: 30_000,
            gaps_ms: gaps,
            encoder: EncoderId::Nvenc,
            quality: 70,
            gop: 120,
            hwaccel,
        })
    }

    fn graph(args: &[String]) -> String {
        let i = args.iter().position(|a| a == "-filter_complex").unwrap();
        args[i + 1].clone()
    }

    #[test]
    fn the_cam_is_bound_to_the_clip_length() {
        let args = args_for(CamLayout::default(), None, &[], false);
        assert!(graph(&args).contains("trim=end=30.021333"));
        assert!(graph(&args).contains("setpts=PTS+0.021333/TB"));
        assert!(!graph(&args).contains("STARTPTS"));
        let t = args.iter().position(|a| a == "-t").unwrap();
        assert_eq!(args[t + 1], "30.000");
        assert!(t > args.iter().rposition(|a| a == "-i").unwrap());
    }

    #[test]
    fn a_gap_hides_the_cam() {
        let args = args_for(CamLayout::default(), None, &[(1500, 3000)], false);
        assert!(graph(&args).contains("enable='not(between(t,1.521333,3.021333))'"));
        assert!(!graph(&args_for(CamLayout::default(), None, &[], false)).contains("enable="));
    }

    #[test]
    fn a_mirrored_circle_is_cropped_flipped_and_masked() {
        let args = args_for(layout(1.0, 1.0, 0.25, CamShape::Circle, true), Some(Path::new("mask.png")), &[], false);
        let g = graph(&args);
        assert!(g.contains("crop=720:720:280:0,hflip,scale=270:270"), "{g}");
        assert!(g.contains("alphamerge"));
        assert_eq!(args.iter().filter(|a| *a == "-i").count(), 3);
    }

    #[test]
    fn the_audio_is_copied_and_nothing_cuts_the_game_short() {
        let args = args_for(CamLayout::default(), None, &[], false);
        assert!(args.windows(2).any(|w| w == ["-map", "0:a?"]));
        assert!(args.windows(2).any(|w| w == ["-c:a", "copy"]));
        assert!(args.windows(2).any(|w| w == ["-map", "[v]"]));
        let all = args.join(" ");
        for banned in ["vstack", "shortest", "-itsoffset", "geq"] {
            assert!(!all.contains(banned), "{banned}");
        }
        assert!(graph(&args).contains("overlay=x=1412:y=782:eval=init:eof_action=repeat"));
    }

    #[test]
    fn hardware_decoding_only_when_asked() {
        let args = args_for(CamLayout::default(), None, &[], true);
        let hw = args.iter().position(|a| a == "-hwaccel").unwrap();
        assert_eq!(args[hw + 1], "d3d11va");
        assert!(hw < args.iter().position(|a| a == "-i").unwrap());
        assert!(!args_for(CamLayout::default(), None, &[], false).iter().any(|a| a == "-hwaccel"));
    }

    #[test]
    fn a_save_time_composite_uses_the_fast_presets() {
        use crate::edit::{video_args_at, Speed};
        let has = |enc, words: &[&str]| {
            let args = video_args_at(enc, 70, Speed::Save);
            words.iter().all(|w| args.iter().any(|a| a == w))
        };
        assert!(has(EncoderId::Nvenc, &["p3", "20"]));
        assert!(has(EncoderId::Amf, &["speed", "20"]));
        assert!(has(EncoderId::Qsv, &["veryfast", "20"]));
        assert!(has(EncoderId::X264, &["superfast", "20"]));
        assert!(video_args_at(EncoderId::Nvenc, 70, Speed::Edit).iter().any(|a| a == "p5"));
    }

    #[test]
    fn the_retries_fall_back_to_software_step_by_step() {
        assert_eq!(
            attempts(EncoderId::Nvenc),
            vec![(EncoderId::Nvenc, true), (EncoderId::X264, true), (EncoderId::X264, false)]
        );
        assert_eq!(attempts(EncoderId::X264), vec![(EncoderId::X264, true), (EncoderId::X264, false)]);
    }

    #[test]
    fn a_circle_mask_is_clear_in_the_corners() {
        let mask = mask_gray8(CamShape::Circle, 100, 100).unwrap();
        assert_eq!(mask[0], 0);
        assert_eq!(mask[99], 0);
        assert_eq!(mask[99 * 100], 0);
        assert_eq!(mask[50 * 100 + 50], 255);
    }

    #[test]
    fn a_rounded_mask_is_opaque_along_the_middle_of_each_edge() {
        let (w, h) = (160usize, 90usize);
        let mask = mask_gray8(CamShape::Rounded, w as u32, h as u32).unwrap();
        assert_eq!(mask[w / 2], 255);
        assert_eq!(mask[(h - 1) * w + w / 2], 255);
        assert_eq!(mask[(h / 2) * w], 255);
        assert_eq!(mask[(h / 2) * w + w - 1], 255);
        assert_eq!(mask[0], 0);
    }

    #[test]
    fn a_rectangle_needs_no_mask() {
        assert!(mask_gray8(CamShape::Rect, 160, 90).is_none());
    }
}
