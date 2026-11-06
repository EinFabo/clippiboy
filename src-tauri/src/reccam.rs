//! The camera in recordings: written beside the recording while it runs, one
//! raw H.264 file per camera session, and burned in once it is stopped.
//!
//! Kept out of `recorder.rs` on purpose: nothing here can cost the recording.
//! The tap only queues (it runs under the camera ring's lock), a writer thread
//! of its own does the disk, and `cam.json` describes the segments from the
//! first one on, so a crashed recording keeps its camera too.

use serde::{Deserialize, Serialize};

/// One camera session within a recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub file: String,
    pub session: u64,
    /// Its first frame against the recording's first frame.
    pub offset_us: i64,
    pub frames: u64,
}

/// `cam.json`: what a recording's camera wrote.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CamFile {
    pub segments: Vec<Segment>,
}

/// Which segment each header and packet belongs to, and what `cam.json`
/// says. Pure: the writer thread only does the disk.
pub struct SegmentLog {
    start_100ns: i64,
    segments: Vec<Segment>,
    /// The current session and its origin.
    current: Option<(u64, i64)>,
    /// The current segment has had its first keyframe.
    keyed: bool,
}

impl SegmentLog {
    pub fn new(start_100ns: i64) -> Self {
        Self { start_100ns, segments: Vec::new(), current: None, keyed: false }
    }

    /// A header of `session`: a session not seen yet begins a segment, whose
    /// index comes back.
    pub fn header(&mut self, session: u64, origin_100ns: i64) -> Option<usize> {
        if self.current.is_some_and(|(s, _)| s == session) {
            return None;
        }
        self.current = Some((session, origin_100ns));
        self.keyed = false;
        let index = self.segments.len();
        self.segments.push(Segment { file: format!("cam.{index}.h264"), session, offset_us: 0, frames: 0 });
        Some(index)
    }

    /// A packet of the current session: the segment it goes into and whether
    /// it is the segment's first — its offset is known from then on — or
    /// `None` before a header or before the segment's first keyframe.
    pub fn packet(&mut self, pts_us: i64, keyframe: bool) -> Option<(usize, bool)> {
        let (_, origin) = self.current?;
        if !self.keyed && !keyframe {
            return None;
        }
        let index = self.segments.len() - 1;
        let segment = &mut self.segments[index];
        let first = !self.keyed;
        if first {
            self.keyed = true;
            segment.offset_us = (origin + pts_us * 10 - self.start_100ns) / 10;
        }
        segment.frames += 1;
        Some((index, first))
    }

    /// Every segment begun — what a crash leaves to recover; counts may lag.
    pub fn file(&self) -> CamFile {
        CamFile { segments: self.segments.clone() }
    }

    /// At the end: the segments that got a frame, with their counts.
    pub fn final_file(&self) -> CamFile {
        CamFile { segments: self.segments.iter().filter(|s| s.frames > 0).cloned().collect() }
    }
}

enum Write {
    Begin(usize, Vec<u8>),
    Packet(usize, std::sync::Arc<[u8]>),
    Describe(CamFile),
    Close(std::sync::mpsc::Sender<CamFile>),
}

/// A recording's camera: a tap on the camera ring that queues for a writer of
/// its own.
pub struct RecordingCam {
    log: parking_lot::Mutex<SegmentLog>,
    queue: parking_lot::Mutex<Option<std::sync::mpsc::Sender<Write>>>,
}

impl RecordingCam {
    /// Writes into the recording's work folder `dir`.
    pub fn start(dir: &std::path::Path, start_100ns: i64) -> std::sync::Arc<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<Write>();
        let dir = dir.to_path_buf();
        let spawned = std::thread::Builder::new().name("clippiboy-reccam".into()).spawn(move || write_loop(&dir, rx));
        if let Err(err) = spawned {
            log::warn!("recording camera: no writer ({err}) — the recording stays without it");
        }
        std::sync::Arc::new(Self { log: parking_lot::Mutex::new(SegmentLog::new(start_100ns)), queue: parking_lot::Mutex::new(Some(tx)) })
    }

    fn send(&self, write: Write) {
        if let Some(tx) = self.queue.lock().as_ref() {
            let _ = tx.send(write);
        }
    }

    /// Stops taking packets, lets the writer finish, and says what it wrote.
    pub fn close(&self) -> CamFile {
        let last = self.log.lock().final_file();
        let Some(tx) = self.queue.lock().take() else { return last };
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let _ = tx.send(Write::Describe(last.clone()));
        let _ = tx.send(Write::Close(done_tx));
        drop(tx);
        done_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap_or(last)
    }
}

impl crate::camtrack::PacketTap for RecordingCam {
    fn cam(&self, packet: &crate::buffer::EncodedPacket) {
        let (placed, file) = {
            let mut log = self.log.lock();
            let placed = log.packet(packet.pts_us, packet.keyframe);
            let file = placed.filter(|(_, first)| *first).map(|_| log.file());
            (placed, file)
        };
        if let Some((index, _)) = placed {
            self.send(Write::Packet(index, packet.data.clone()));
        }
        // The offset is known now: written down before a crash can need it.
        if let Some(file) = file {
            self.send(Write::Describe(file));
        }
    }

    fn header(&self, session: u64, origin_100ns: i64, header: &[u8]) {
        let (index, file) = {
            let mut log = self.log.lock();
            (log.header(session, origin_100ns), log.file())
        };
        if let Some(index) = index {
            self.send(Write::Begin(index, header.to_vec()));
            self.send(Write::Describe(file));
        }
    }
}

/// The disk side: one open segment file at a time, `cam.json` rewritten
/// whenever the segments change.
fn write_loop(dir: &std::path::Path, rx: std::sync::mpsc::Receiver<Write>) {
    use std::io::Write as _;
    let mut open: Option<(usize, std::io::BufWriter<std::fs::File>)> = None;
    let mut broken = false;
    for write in rx {
        match write {
            Write::Begin(index, header) => {
                if let Some((_, mut file)) = open.take() {
                    let _ = file.flush();
                }
                match std::fs::File::create(dir.join(format!("cam.{index}.h264"))) {
                    Ok(file) => {
                        let mut file = std::io::BufWriter::new(file);
                        broken = file.write_all(&header).is_err();
                        open = Some((index, file));
                    }
                    Err(err) => {
                        log::warn!("recording camera: cam.{index}.h264: {err}");
                        broken = true;
                    }
                }
            }
            Write::Packet(index, data) => {
                if let Some((current, file)) = open.as_mut().filter(|(current, _)| *current == index && !broken) {
                    if file.write_all(&data).is_err() {
                        log::warn!("recording camera: writing cam.{current}.h264 failed");
                        broken = true;
                    }
                }
            }
            Write::Describe(file) => describe(dir, &file),
            Write::Close(done) => {
                if let Some((_, mut file)) = open.take() {
                    let _ = file.flush();
                }
                let file = read_cam_file(dir).unwrap_or_default();
                let _ = done.send(file);
                return;
            }
        }
    }
}

fn describe(dir: &std::path::Path, file: &CamFile) {
    let Ok(text) = serde_json::to_string_pretty(file) else { return };
    let temp = dir.join("cam.json.tmp");
    if std::fs::write(&temp, text).and_then(|_| std::fs::rename(&temp, dir.join("cam.json"))).is_err() {
        log::warn!("recording camera: cam.json not written");
    }
}

pub fn read_cam_file(dir: &std::path::Path) -> Option<CamFile> {
    serde_json::from_str(&std::fs::read_to_string(dir.join("cam.json")).ok()?).ok()
}

// ------------------------------------------------------------ after the stop

/// A segment as kept in the store, its offset in the recording's own time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSegment {
    pub file: String,
    pub offset_us: i64,
    /// Its picture's size: another camera half way is another size.
    pub width: u32,
    pub height: u32,
}

/// A stopped recording's camera, kept beside it until it is burned in:
/// `originals/<id>/reccam.json`, written after its segments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredCam {
    /// The recording's frame rate, for its keyframe interval.
    pub fps: u32,
    pub segments: Vec<StoredSegment>,
    /// The clean recording's size: a file that changed since — trimmed in the
    /// editor — is not burned into.
    pub clean_bytes: u64,
    /// The composite's size, written just before it replaces the recording.
    #[serde(default)]
    pub applying_bytes: Option<u64>,
}

pub fn stored_path(clip_id: &str) -> std::path::PathBuf {
    crate::edit::dir(clip_id).join("reccam.json")
}

pub fn read_stored(clip_id: &str) -> Option<StoredCam> {
    serde_json::from_str(&std::fs::read_to_string(stored_path(clip_id)).ok()?).ok()
}

fn write_stored(clip_id: &str, stored: &StoredCam) -> std::io::Result<()> {
    let path = stored_path(clip_id);
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, serde_json::to_string_pretty(stored).map_err(std::io::Error::other)?)?;
    std::fs::rename(&temp, &path)
}

/// The store's camera files gone; the folder too when nothing else is in it.
fn drop_stored(clip_id: &str) {
    if let Some(stored) = read_stored(clip_id) {
        for segment in &stored.segments {
            let _ = std::fs::remove_file(crate::edit::dir(clip_id).join(&segment.file));
        }
    }
    let _ = std::fs::remove_file(stored_path(clip_id));
    let _ = std::fs::remove_dir(crate::edit::dir(clip_id));
}

/// The segments whose file holds something — after a crash the counts in
/// `cam.json` may still be zero, the disk is what counts.
pub fn worth_keeping(file: &CamFile, size_of: impl Fn(&str) -> u64) -> Vec<Segment> {
    file.segments.iter().filter(|s| size_of(&s.file) > 0).cloned().collect()
}

/// Where a moment `real_us` after the recording's first frame lies in the
/// file: the picture's gaps (the PC slept) are not in it.
pub fn recording_time(real_us: i64, gaps: &[crate::recorder::VideoGap]) -> i64 {
    let mut cut = 0;
    for gap in gaps {
        if real_us >= gap.at_us + gap.len_us {
            cut += gap.len_us;
        } else if real_us > gap.at_us {
            cut += real_us - gap.at_us;
        }
    }
    real_us - cut
}

/// At finalize, once the clean recording is written and before the work
/// folder goes: each segment into an MP4 in the store, then `reccam.json`.
/// A segment that cannot be muxed is left out; `None` when none is left.
pub fn secure(
    work_dir: &std::path::Path,
    clip_id: &str,
    gaps: &[crate::recorder::VideoGap],
    clean_bytes: u64,
    fps: u32,
) -> Option<StoredCam> {
    let file = read_cam_file(work_dir)?;
    let size_of = |name: &str| std::fs::metadata(work_dir.join(name)).map(|m| m.len()).unwrap_or(0);
    let keep = worth_keeping(&file, size_of);
    if keep.is_empty() {
        return None;
    }
    let store = crate::edit::dir(clip_id);
    std::fs::create_dir_all(&store).ok()?;
    let mut segments = Vec::new();
    for segment in keep {
        let name = segment.file.replace(".h264", ".mp4");
        let out = store.join(&name);
        let args = crate::facecam::cam_mp4_arguments(&work_dir.join(&segment.file), &out);
        match crate::muxer::command("ffmpeg").args(&args).output() {
            Ok(done) if done.status.success() => {}
            Ok(done) => {
                log::warn!("recording camera: {name}: {}", String::from_utf8_lossy(&done.stderr).trim());
                let _ = std::fs::remove_file(&out);
                continue;
            }
            Err(err) => {
                log::warn!("recording camera: {name}: {err}");
                continue;
            }
        }
        let Some((width, height)) = crate::muxer::probe_video_size(&out) else {
            log::warn!("recording camera: {name} has no picture");
            let _ = std::fs::remove_file(&out);
            continue;
        };
        segments.push(StoredSegment { file: name, offset_us: recording_time(segment.offset_us, gaps), width, height });
    }
    if segments.is_empty() {
        let _ = std::fs::remove_dir(&store);
        return None;
    }
    let stored = StoredCam { fps, segments, clean_bytes, applying_bytes: None };
    if let Err(err) = write_stored(clip_id, &stored) {
        log::warn!("recording camera: reccam.json: {err}");
        return None;
    }
    log::info!("recording camera: {} segment(s) kept for {clip_id}", stored.segments.len());
    Some(stored)
}

/// One segment going into the burn-in.
pub struct SegmentInput {
    pub path: std::path::PathBuf,
    pub cam: (u32, u32),
    pub timing: crate::composite::CamTiming,
}

pub struct BurnIn<'a> {
    pub clean: &'a std::path::Path,
    pub segments: &'a [SegmentInput],
    pub mask: Option<&'a std::path::Path>,
    pub output: &'a std::path::Path,
    pub frame: (u32, u32),
    pub layout: crate::model::CamLayout,
    pub length_ms: u64,
    pub encoder: crate::model::EncoderId,
    pub quality: u32,
    pub gop: u32,
    pub hwaccel: bool,
}

/// ffmpeg's arguments for a recording with its camera: one overlay per
/// segment, each letting the game through once it ends, every sound track
/// copied as it is.
pub fn recording_arguments(job: &BurnIn) -> Vec<String> {
    let mut args: Vec<String> =
        ["-y", "-hide_banner", "-loglevel", "error", "-progress", "pipe:1", "-nostats"].into_iter().map(String::from).collect();
    if job.hwaccel {
        args.extend(["-hwaccel".into(), "d3d11va".into()]);
    }
    args.extend(["-threads".into(), "4".into(), "-i".into(), job.clean.to_string_lossy().into_owned()]);
    for segment in job.segments {
        if let Some(seek) = segment.timing.seek_s {
            args.extend(["-ss".into(), format!("{seek:.6}")]);
        }
        args.extend(["-i".into(), segment.path.to_string_lossy().into_owned()]);
    }
    let count = job.segments.len();
    if let Some(mask) = job.mask {
        args.extend(["-loop".into(), "1".into(), "-i".into(), mask.to_string_lossy().into_owned()]);
    }
    let rect = crate::composite::cam_rect(job.frame, &job.layout);
    let mut parts = Vec::new();
    if job.mask.is_some() {
        let labels: String = (0..count).map(|i| format!("[mask{i}]")).collect();
        parts.push(if count > 1 {
            format!("[{}:v]format=gray,split={count}{labels}", count + 1)
        } else {
            format!("[{}:v]format=gray{labels}", count + 1)
        });
    }
    let mut base = "0:v".to_string();
    for (i, segment) in job.segments.iter().enumerate() {
        let mut cam = format!(
            "[{}:v]setpts=PTS+{:.6}/TB,trim=end={:.6}",
            i + 1,
            segment.timing.shift_s,
            segment.timing.end_s
        );
        if let Some(crop) = crate::composite::cam_crop(segment.cam, job.layout.shape) {
            cam += &format!(",crop={}:{}:{}:{}", crop.w, crop.h, crop.x, crop.y);
        }
        if job.layout.mirror {
            cam += ",hflip";
        }
        cam += &format!(",scale={}:{},format=yuva420p[cam{i}]", rect.w, rect.h);
        parts.push(cam);
        let mut overlaid = format!("cam{i}");
        if job.mask.is_some() {
            parts.push(format!("[cam{i}][mask{i}]alphamerge[camm{i}]"));
            overlaid = format!("camm{i}");
        }
        let out = if i + 1 == count { "v".to_string() } else { format!("o{i}") };
        parts.push(format!("[{base}][{overlaid}]overlay=x={}:y={}:eval=init:eof_action=pass[{out}]", rect.x, rect.y));
        base = out;
    }
    args.extend(["-filter_complex_threads".into(), "2".into(), "-filter_complex".into(), parts.join(";")]);
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

/// The row's state after a burn-in, and whether the segments go: a failed
/// one keeps the clean recording and its segments for another try.
pub fn after_burn(ok: bool) -> (crate::model::FacecamState, bool) {
    if ok {
        (crate::model::FacecamState::Baked, true)
    } else {
        (crate::model::FacecamState::Failed, false)
    }
}

/// What a job finds when it starts — also after a crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    Burn,
    /// The composite replaced the recording; only the row did not say so yet.
    Finish,
    /// The recording is gone or was changed since.
    GiveUp,
}

pub fn resume(stored: &StoredCam, file_bytes: Option<u64>) -> Resume {
    match file_bytes {
        Some(bytes) if bytes == stored.clean_bytes => Resume::Burn,
        Some(bytes) if Some(bytes) == stored.applying_bytes => Resume::Finish,
        _ => Resume::GiveUp,
    }
}

// ------------------------------------------------------------- the burn-in

fn set_row(app: &tauri::AppHandle, id: &str, facecam: Option<&crate::model::ClipFacecam>) {
    use tauri::{Emitter, Manager};
    let library = &app.state::<crate::state::AppState>().library;
    if let Some(Err(err)) = crate::clips::with_library(library, |lib| lib.set_facecam(id, facecam)) {
        log::error!("could not note the recording camera's state: {err}");
    }
    let _ = app.emit("clips-changed", ());
}

/// The composite is the recording now: the segments go, the row follows the
/// file.
fn finish(app: &tauri::AppHandle, id: &str, path: &std::path::Path, cam: &crate::model::ClipFacecam) {
    use tauri::Manager;
    let (state, drop) = after_burn(true);
    if drop {
        drop_stored(id);
    }
    if let Err(err) = crate::thumbs::make(path, id) {
        log::warn!("thumbnail for {id}: {err}");
    }
    let library = &app.state::<crate::state::AppState>().library;
    if let (Some(length), Some(size)) = (crate::muxer::probe_duration_ms(path), crate::composite::file_size(path)) {
        let _ = crate::clips::with_library(library, |lib| lib.set_file_state(id, length, size));
    }
    log::info!("camera burned into recording {id}");
    set_row(app, id, Some(&crate::model::ClipFacecam { state, ..cam.clone() }));
    crate::notify(app, "ok", "Camera added to the recording");
}

fn fail(app: &tauri::AppHandle, id: &str, cam: &crate::model::ClipFacecam, why: &str) {
    log::warn!("recording camera for {id}: {why}");
    let (state, _) = after_burn(false);
    set_row(app, id, Some(&crate::model::ClipFacecam { state, ..cam.clone() }));
    crate::notify(app, "error", "The camera couldn't be added to the recording — it was kept without it.");
}

/// Burn-ins asked to stop: their recording is being deleted.
static ABORTED: std::sync::LazyLock<parking_lot::Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(Default::default);

/// Stops the running burn-in of `id` at ffmpeg's next progress line.
pub fn abort(id: &str) {
    ABORTED.lock().insert(id.to_string());
}

fn aborted(id: &str) -> bool {
    ABORTED.lock().contains(id)
}

/// Clears a job's stop when it ends, however it ends.
struct Unmark<'a>(&'a str);

impl Drop for Unmark<'_> {
    fn drop(&mut self) {
        ABORTED.lock().remove(self.0);
    }
}

/// What the burn-in does once it has the recording's lock again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settle {
    Swap,
    /// Deleted, or no longer waiting for its camera: the composite goes.
    Leave,
    /// The recording changed meanwhile: neither the composite nor the
    /// segments fit it any more.
    Drop,
}

pub fn settle(aborted: bool, pending: bool, resume: Resume) -> Settle {
    match (aborted || !pending, resume) {
        (true, _) | (false, Resume::Finish) => Settle::Leave,
        (false, Resume::Burn) => Settle::Swap,
        (false, Resume::GiveUp) => Settle::Drop,
    }
}

/// One burn-in. `Some(at)` when the swap has to be tried again later.
pub fn job(app: &tauri::AppHandle, id: &str) -> Option<std::time::Instant> {
    use crate::composite::{file_size, temp_path};
    use crate::model::FacecamState;
    use tauri::Manager;
    let _unmark = Unmark(id);
    let state = app.state::<crate::state::AppState>();
    let clip_lock = crate::edit::clip_lock(id);
    let mut guard = Some(clip_lock.lock());
    let clip = crate::clips::fresh(&state.library, id)?;
    let cam = clip.facecam.clone()?;
    if cam.state != FacecamState::Pending {
        return None;
    }
    let path = std::path::PathBuf::from(&clip.path);
    let Some(mut stored) = read_stored(id) else {
        set_row(app, id, None);
        return None;
    };
    match resume(&stored, file_size(&path)) {
        Resume::GiveUp => {
            log::info!("recording camera for {id}: the recording changed or is gone — not burned in");
            drop_stored(id);
            set_row(app, id, None);
            return None;
        }
        Resume::Finish => {
            finish(app, id, &path, &cam);
            return None;
        }
        Resume::Burn => {}
    }
    let temp = temp_path(&path);
    // A swap tried again finds its composite still there.
    let composited = stored.applying_bytes.is_some() && file_size(&temp) == stored.applying_bytes;
    if !composited {
        let config = state.config_snapshot();
        let data = crate::config::data_dir();
        let dir = path.parent().map(std::path::Path::to_path_buf).unwrap_or_else(|| data.clone());
        let free = crate::disk::free_bytes(&dir).unwrap_or(u64::MAX);
        if !crate::composite::room_for_composite(stored.clean_bytes, free, u64::MAX, true) {
            fail(app, id, &cam, "not enough free space");
            return None;
        }
        let video_start_us = crate::muxer::probe_video_start_us(&path).unwrap_or(0);
        let length_ms = crate::muxer::probe_duration_ms(&path).unwrap_or(clip.duration_ms);
        let frame = (clip.width, clip.height);
        let rect = crate::composite::cam_rect(frame, &cam.layout);
        let mask = crate::composite::mask_gray8(cam.layout.shape, rect.w, rect.h).and_then(|pixels| {
            let dir = data.join("temp");
            let _ = std::fs::create_dir_all(&dir);
            let file = dir.join(format!("{}_recmask_{}x{}.png", crate::muxer::sanitize(id), rect.w, rect.h));
            crate::composite::write_mask_png(&file, rect.w, rect.h, &pixels).ok().map(|_| file)
        });
        let segments: Vec<SegmentInput> = stored
            .segments
            .iter()
            .map(|segment| SegmentInput {
                path: crate::edit::dir(id).join(&segment.file),
                cam: (segment.width, segment.height),
                timing: crate::composite::cam_timing(segment.offset_us, cam.delay_ms, video_start_us, 0, length_ms),
            })
            .collect();
        let encoder = crate::encode::effective(config.recording.encoder);
        // The encode runs without the recording's lock — it takes minutes, and
        // a delete stops it instead of waiting for it. Whoever else wants the
        // file waits for the queue first.
        guard.take();
        let mut done = false;
        for (encoder, hwaccel) in crate::composite::attempts(encoder) {
            let args = recording_arguments(&BurnIn {
                clean: &path,
                segments: &segments,
                mask: mask.as_deref(),
                output: &temp,
                frame,
                layout: cam.layout,
                length_ms,
                encoder,
                quality: config.recording.quality,
                gop: stored.fps * config.recording.keyframe_seconds,
                hwaccel,
            });
            let stop = || aborted(id);
            match crate::edit::run_command_watched(crate::muxer::background("ffmpeg"), &args, length_ms, &|_| {}, &stop) {
                Ok(()) => {
                    done = true;
                    break;
                }
                Err(err) if err == crate::edit::STOPPED => break,
                Err(err) => {
                    log::warn!("recording camera for {id} with {encoder:?} (hardware decoding {hwaccel}): {err}");
                    let _ = std::fs::remove_file(&temp);
                }
            }
        }
        if let Some(mask) = &mask {
            let _ = std::fs::remove_file(mask);
        }
        guard = Some(clip_lock.lock());
        let pending = crate::clips::fresh(&state.library, id)
            .and_then(|c| c.facecam)
            .is_some_and(|c| c.state == FacecamState::Pending);
        match settle(aborted(id), pending, resume(&stored, file_size(&path))) {
            Settle::Swap => {}
            Settle::Leave => {
                let _ = std::fs::remove_file(&temp);
                return None;
            }
            Settle::Drop => {
                log::info!("recording camera for {id}: the recording changed during the burn-in — not burned in");
                let _ = std::fs::remove_file(&temp);
                drop_stored(id);
                set_row(app, id, None);
                return None;
            }
        }
        if !done {
            fail(app, id, &cam, "ffmpeg failed with every encoder");
            return None;
        }
        stored.applying_bytes = file_size(&temp);
        if let Err(err) = write_stored(id, &stored) {
            log::warn!("reccam.json for {id}: {err}");
        }
    }
    if crate::muxer::replace_file(&temp, &path).is_ok() {
        finish(app, id, &path, &cam);
        return None;
    }
    // A player holds the file. Wait for it without holding the recording.
    guard.take();
    for wait in crate::composite::swap_waits() {
        std::thread::sleep(wait);
        let _guard = clip_lock.lock();
        let still = crate::clips::fresh(&state.library, id)
            .and_then(|c| c.facecam)
            .is_some_and(|c| c.state == FacecamState::Pending);
        if !still {
            let _ = std::fs::remove_file(&temp);
            return None;
        }
        if crate::muxer::replace_file(&temp, &path).is_ok() {
            finish(app, id, &path, &cam);
            return None;
        }
    }
    log::info!("recording camera for {id}: the file is still in use — tried again in five minutes");
    Some(std::time::Instant::now() + crate::composite::SWAP_RETRY_EVERY)
}

/// Whether "Try adding the camera again" applies: a recording whose burn-in
/// failed and whose segments are still kept.
pub fn retryable(recording: bool, state: Option<crate::model::FacecamState>, stored: bool) -> bool {
    recording && stored && state == Some(crate::model::FacecamState::Failed)
}

/// At start: every recording whose camera was on its way when the app last
/// stopped goes back in the queue; a composite half written is cleared away.
pub fn recover(library: &parking_lot::Mutex<Option<crate::clips::Library>>) -> usize {
    let clips = crate::clips::with_library(library, |lib| lib.list()).and_then(Result::ok).unwrap_or_default();
    let mut queued = 0;
    for clip in clips {
        let pending = clip.facecam.as_ref().is_some_and(|cam| cam.state == crate::model::FacecamState::Pending);
        if !clip.recording || !pending {
            continue;
        }
        let path = std::path::PathBuf::from(&clip.path);
        let applying = read_stored(&clip.id).and_then(|s| s.applying_bytes);
        for stale in crate::composite::stale_temps(&path) {
            // One finished and waiting for its swap is kept.
            if applying.is_none() || crate::composite::file_size(&stale) != applying {
                let _ = std::fs::remove_file(stale);
            }
        }
        crate::composite::enqueue_recording(&clip.id);
        queued += 1;
    }
    queued
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: i64 = 1_000_000_000;

    #[test]
    fn a_new_session_starts_a_new_segment() {
        let mut log = SegmentLog::new(START);
        assert_eq!(log.header(1, START), Some(0));
        assert_eq!(log.packet(0, true), Some((0, true)));
        assert_eq!(log.packet(33_333, false), Some((0, false)));
        // The same session's header again (a resend) is no new segment.
        assert_eq!(log.header(1, START), None);
        // A lock or a sleep: the camera comes back with a new session.
        assert_eq!(log.header(2, START + 600_000_000), Some(1));
        assert_eq!(log.packet(0, true), Some((1, true)));
        let file = log.file();
        assert_eq!(file.segments.len(), 2);
        assert_eq!((file.segments[0].file.as_str(), file.segments[0].frames), ("cam.0.h264", 2));
        assert_eq!((file.segments[1].file.as_str(), file.segments[1].frames), ("cam.1.h264", 1));
        assert_eq!(file.segments[1].offset_us, 60_000_000);
    }

    #[test]
    fn segment_offsets_are_in_recording_time() {
        let mut log = SegmentLog::new(START);
        // The session began 1 s before the recording; its backlog starts 0.5 s in.
        log.header(1, START - 10_000_000);
        log.packet(500_000, true);
        assert_eq!(log.file().segments[0].offset_us, -500_000);
    }

    #[test]
    fn nothing_is_written_before_a_header_and_a_keyframe() {
        let mut log = SegmentLog::new(START);
        assert_eq!(log.packet(0, true), None, "no header yet");
        log.header(1, START);
        assert_eq!(log.packet(0, false), None, "a segment starts on a keyframe");
        assert_eq!(log.packet(33_333, true), Some((0, true)));
        assert_eq!(log.file().segments[0].frames, 1);
    }

    /// `cam.json` is written as a segment begins, when its offset is not known
    /// yet, and again on its first frame: a crash later on found 0 there and
    /// put the camera up to a keyframe interval off.
    #[test]
    fn the_first_frame_settles_the_offset_on_disk() {
        let mut log = SegmentLog::new(START);
        log.header(1, START - 10_000_000);
        assert_eq!(log.packet(500_000, true), Some((0, true)), "describe again now");
        assert_eq!(log.file().segments[0].offset_us, -500_000);
        assert_eq!(log.packet(533_333, false), Some((0, false)));
    }

    /// Written as a segment begins, before its first frame: a crash right
    /// after must still find it.
    #[test]
    fn a_segment_is_described_from_its_start() {
        let mut log = SegmentLog::new(START);
        log.header(1, START);
        assert_eq!(log.file().segments.len(), 1);
        assert!(log.final_file().segments.is_empty(), "no frame, nothing to burn in");
    }

    fn segment(file: &str, offset_us: i64) -> Segment {
        Segment { file: file.into(), session: 1, offset_us, frames: 0 }
    }

    /// After a crash the counts may be zero; what is on disk decides.
    #[test]
    fn recovered_segments_come_from_the_folder() {
        let file = CamFile { segments: vec![segment("cam.0.h264", 0), segment("cam.1.h264", 60_000_000), segment("cam.2.h264", 90_000_000)] };
        let sizes = |name: &str| match name {
            "cam.0.h264" => 1_000,
            "cam.2.h264" => 500,
            _ => 0,
        };
        let kept: Vec<String> = worth_keeping(&file, sizes).into_iter().map(|s| s.file).collect();
        assert_eq!(kept, ["cam.0.h264", "cam.2.h264"]);
    }

    #[test]
    fn a_recording_without_the_camera_has_no_cam_files() {
        let work = std::env::temp_dir().join("clippiboy-test-reccam-none");
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();
        let id = "clippiboy-test-reccam-none";
        assert_eq!(secure(&work, id, &[], 1_000, 60), None);
        assert!(!crate::edit::dir(id).exists(), "no store made for nothing");
        let _ = std::fs::remove_dir_all(&work);
    }

    fn input(path: &str, offset_us: i64) -> SegmentInput {
        SegmentInput {
            path: std::path::PathBuf::from(path),
            cam: (1280, 720),
            timing: crate::composite::cam_timing(offset_us, -40, 0, 0, 120_000),
        }
    }

    fn args(segments: &[SegmentInput], mask: bool) -> Vec<String> {
        args_in(segments, mask, crate::model::CamLayout::default())
    }

    fn args_in(segments: &[SegmentInput], mask: bool, layout: crate::model::CamLayout) -> Vec<String> {
        recording_arguments(&BurnIn {
            clean: std::path::Path::new("rec.mp4"),
            segments,
            mask: mask.then(|| std::path::Path::new("mask.png")),
            output: std::path::Path::new("out.mp4"),
            frame: (1920, 1080),
            layout,
            length_ms: 120_000,
            encoder: crate::model::EncoderId::X264,
            quality: 23,
            gop: 60,
            hwaccel: false,
        })
    }

    fn graph(args: &[String]) -> String {
        args.windows(2).find(|w| w[0] == "-filter_complex").map(|w| w[1].clone()).unwrap_or_default()
    }

    #[test]
    fn every_segment_gets_its_own_overlay() {
        let segments = [input("cam.0.mp4", -500_000), input("cam.1.mp4", 60_000_000)];
        let a = args(&segments, true);
        let g = graph(&a);
        assert_eq!(g.matches("overlay=").count(), 2, "{g}");
        assert!(g.contains("[1:v]") && g.contains("[2:v]"), "{g}");
        assert!(g.contains("split=2"), "one mask for both: {g}");
        assert!(g.ends_with("[v]"), "{g}");
        // The second segment starts a minute in: shifted, not seeked.
        let joined = a.join(" ");
        assert!(joined.contains("-i cam.0.mp4") && joined.contains("-i cam.1.mp4"), "{joined}");
    }

    /// Another camera half way — 720p, then 1080p: each segment is cut at
    /// its own size.
    #[test]
    fn each_segment_is_cut_at_its_own_size() {
        let mut second = input("cam.1.mp4", 60_000_000);
        second.cam = (1920, 1080);
        let layout = crate::model::CamLayout { shape: crate::model::CamShape::Circle, ..Default::default() };
        let g = graph(&args_in(&[input("cam.0.mp4", 0), second], true, layout));
        assert!(g.contains("crop=720:720:280:0"), "{g}");
        assert!(g.contains("crop=1080:1080:420:0"), "{g}");
    }

    #[test]
    fn segments_let_the_game_through_after_their_end() {
        let g = graph(&args(&[input("cam.0.mp4", 0)], false));
        assert!(g.contains("eof_action=pass"), "{g}");
        assert!(!g.contains("eof_action=repeat"), "{g}");
    }

    #[test]
    fn all_audio_tracks_are_kept() {
        let a = args(&[input("cam.0.mp4", 0)], false);
        assert!(a.windows(2).any(|w| w == ["-map", "0:a?"]), "{a:?}");
        assert!(a.windows(2).any(|w| w == ["-c:a", "copy"]), "{a:?}");
        assert!(a.windows(2).any(|w| w == ["-t", "120.000"]), "{a:?}");
    }

    #[test]
    fn a_failed_burn_in_keeps_the_clean_recording() {
        assert_eq!(after_burn(true), (crate::model::FacecamState::Baked, true));
        assert_eq!(after_burn(false), (crate::model::FacecamState::Failed, false), "clean kept, segments kept for a retry");
    }

    fn gap(at_us: i64, len_us: i64) -> crate::recorder::VideoGap {
        crate::recorder::VideoGap { at_us, len_us }
    }

    /// The PC slept ten minutes a minute in: a segment that started after
    /// waking sits a minute in, not eleven.
    #[test]
    fn segments_after_a_sleep_are_moved_up() {
        let gaps = [gap(60_000_000, 600_000_000)];
        assert_eq!(recording_time(30_000_000, &gaps), 30_000_000);
        assert_eq!(recording_time(662_000_000, &gaps), 62_000_000);
        assert_eq!(recording_time(300_000_000, &gaps), 60_000_000, "inside the gap: where it was cut");
    }

    fn stored(clean_bytes: u64, applying_bytes: Option<u64>) -> StoredCam {
        StoredCam { fps: 60, segments: Vec::new(), clean_bytes, applying_bytes }
    }

    #[test]
    fn a_changed_recording_is_not_burned_into() {
        assert_eq!(resume(&stored(1_000, None), Some(1_000)), Resume::Burn);
        assert_eq!(resume(&stored(1_000, None), Some(400)), Resume::GiveUp, "trimmed in the editor");
        assert_eq!(resume(&stored(1_000, None), None), Resume::GiveUp, "deleted");
    }

    #[test]
    fn a_swap_done_before_a_crash_is_finished_not_burned_again() {
        assert_eq!(resume(&stored(1_000, Some(1_200)), Some(1_200)), Resume::Finish);
        assert_eq!(resume(&stored(1_000, Some(1_200)), Some(1_000)), Resume::Burn);
    }

    #[test]
    fn only_a_failed_recording_camera_is_tried_again() {
        use crate::model::FacecamState::*;
        assert!(retryable(true, Some(Failed), true));
        assert!(!retryable(true, Some(Failed), false), "segments gone");
        assert!(!retryable(true, Some(Pending), true), "on its way already");
        assert!(!retryable(true, Some(Baked), true));
        assert!(!retryable(true, None, true));
        assert!(!retryable(false, Some(Failed), true), "a clip retries from the editor");
    }

    /// The burn-in runs without the recording's lock: what it finds when it
    /// takes it again decides.
    #[test]
    fn a_recording_deleted_during_its_burn_in_is_left_alone() {
        assert_eq!(settle(false, true, Resume::Burn), Settle::Swap);
        assert_eq!(settle(true, true, Resume::Burn), Settle::Leave, "deleted: stopped");
        assert_eq!(settle(false, false, Resume::Burn), Settle::Leave, "row gone");
        assert_eq!(settle(false, true, Resume::GiveUp), Settle::Drop, "changed meanwhile");
    }

    #[test]
    fn cam_json_survives_a_reread() {
        let mut log = SegmentLog::new(START);
        log.header(3, START);
        log.packet(0, true);
        let file = log.file();
        let text = serde_json::to_string(&file).unwrap();
        assert_eq!(serde_json::from_str::<CamFile>(&text).unwrap(), file);
    }
}
