//! A recording started and stopped by hand.
//!
//! It rides on the same pipeline as the replay buffer — one encoder, one mixer
//! — and only taps what already flows: every video packet the encoder hands
//! out, every audio block the mixer pushes into a track ring. Nothing is held
//! in memory. A writer thread streams it all into a folder of its own,
//!
//! ```text
//! <clip folder>/.recording/<id>/video.ts      the picture, every frame with its time
//! <clip folder>/.recording/<id>/track_<n>.pcm one per source, 16-bit, 48 kHz
//! <clip folder>/.recording/<id>/meta.json     what the files are
//! ```
//!
//! The picture keeps its timestamps in a small MPEG-TS ([`crate::ts`]), so a
//! stall is a held frame where it happened rather than spread over the whole
//! file (BUF-6), and a folder finished after a crash plays at its real speed
//! (REC-6). An encoder with B-frames, whose pictures come out of order, and
//! folders written before this version have `video.h264` instead: the raw
//! elementary stream, spread at one rate (`muxer::timing`).
//!
//! and stopping muxes that into an MP4 the same way a clip is written
//! (`muxer::encode`). The folder sits inside the clip folder on purpose: the
//! finished file is renamed out of it, and a rename across drives is a copy of
//! an hour of video.
//!
//! Should the app die mid-recording the folder is still there, whole up to
//! the last write, and the next start finds it ([`leftovers`]) and finishes it.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::audio::capture::{CHANNELS, SAMPLE_RATE};
use crate::buffer::{EncodedPacket, DISCONTINUITY_US};
use crate::muxer::{self, AudioInput, ClipResult};
use crate::pipeline::TrackRing;
use crate::ts::{self, TsWriter};

/// The folder inside the clip folder that unfinished recordings live in.
pub const WORK_DIR: &str = ".recording";

/// The picture as a raw elementary stream: from an encoder that reorders its
/// frames, or from a version before [`VIDEO_TS`].
const VIDEO_FILE: &str = "video.h264";
/// The picture with its timestamps.
const VIDEO_TS: &str = "video.ts";
const META_FILE: &str = "meta.json";
const MIX_FILE: &str = "mix.pcm";

/// Bytes per audio frame: 16 bit per sample, [`CHANNELS`] samples.
const FRAME_BYTES: u64 = 2 * CHANNELS as u64;

/// What the summed main mix takes per second of recording: 48 kHz, 16 bit,
/// stereo.
pub const MIX_BYTES_PER_SECOND: u64 = SAMPLE_RATE as u64 * FRAME_BYTES;

/// Scratch space the stop needs for the main mix beside the recording. Below
/// two tracks nothing is summed: the one track is used as it is.
pub fn mix_scratch_bytes(tracks_in_mix: usize, seconds: u64) -> u64 {
    if tracks_in_mix < 2 {
        0
    } else {
        seconds * MIX_BYTES_PER_SECOND
    }
}

/// How much the writer may lag behind before the recording is stopped: a
/// drive that cannot keep up would otherwise let the queue grow until the app
/// runs out of memory.
const WRITER_BACKLOG_LIMIT: u64 = 512 << 20;

fn backlog_failure(queued_bytes: u64) -> Option<String> {
    (queued_bytes > WRITER_BACKLOG_LIMIT)
        .then(|| "the clip drive cannot keep up with the recording".to_string())
}

/// Large writes rather than many small ones — the disk is shared with a game
/// that is loading its own things.
const WRITE_BUFFER: usize = 1 << 20;

/// What the folder holds, so that a recording can be finished from it alone —
/// by the same run, or by the next start after a crash.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub id: String,
    /// Unix milliseconds.
    pub created_at: i64,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub game: Option<String>,
    pub tracks: Vec<MetaTrack>,
    /// Sources that get a track of their own. Everything else is summed into
    /// the main mix — the same rule a clip follows. Written at the start so a
    /// crashed recording keeps its tracks apart, and settled again at stop.
    #[serde(default)]
    pub separate: Vec<String>,
    /// Video frames written. Only known once the recording stopped cleanly;
    /// after a crash it is counted off the stream.
    #[serde(default)]
    pub frames: Option<u64>,
    /// Microseconds from the first frame to the end of the last, by their
    /// timestamps, less the [`Self::video_gaps`] — what the picture is spread
    /// over (`muxer::timing`). Known only after a clean stop, like `frames`;
    /// without it the configured rate stands.
    #[serde(default)]
    pub span_us: Option<u64>,
    /// The longest step between neighbouring frame timestamps, which tells an
    /// encoder that fell short from a gap such as sleep (`muxer::timing`).
    /// Known only after a clean stop; a folder written before it was kept
    /// falls back to the old rule.
    #[serde(default)]
    pub largest_step_us: Option<i64>,
    /// Friends who played the same game when the recording started. Kept here
    /// rather than beside the running recording, so that they stay with this
    /// one: a stop that is still encoding while the next recording starts,
    /// or a folder finished after a crash, still knows who was there.
    #[serde(default)]
    pub friends: Vec<String>,
    /// Jumps in the picture longer than [`DISCONTINUITY_US`] — the PC slept.
    /// They are left out of the span, and the sound of that stretch goes with
    /// them. Written down the moment they happen, so a recording finished
    /// after a crash leaves them out too.
    #[serde(default)]
    pub video_gaps: Vec<VideoGap>,
    /// Stretches the mixer did not run through, counted rather than written
    /// as zeros: nothing of them is in the track files.
    #[serde(default)]
    pub audio_gaps: Vec<AudioGap>,
}

/// A hole in the picture, in microseconds from the recording's first frame.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoGap {
    /// Where the last frame before the jump ends.
    pub at_us: i64,
    /// How long nothing came.
    pub len_us: i64,
}

/// A hole in the sound, on the recording's audio timeline (48 kHz frames).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioGap {
    pub at_frame: u64,
    pub frames: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaTrack {
    pub source_id: String,
    pub label: String,
    pub file: String,
}

/// What the writer thread is told to do.
enum Job {
    /// One frame. `pts_us` counts from the recording's first frame, its gaps
    /// taken out; only the TS writes it.
    Video {
        data: Arc<[u8]>,
        pts_us: i64,
        keyframe: bool,
    },
    /// SPS/PPS. Raw, it goes into the stream as it is; in a TS it is no frame
    /// of its own and rides in front of the next one.
    Header(Arc<[u8]>),
    Open(usize),
    Audio(usize, Vec<i16>),
    /// Zeros, counted in frames rather than handed over — a source that joins
    /// an hour in would otherwise be an allocation of an hour of silence.
    Silence(usize, u64),
    /// The description, written by the writer rather than by the thread that
    /// changed it: the encoder and the mixer never wait on the disk, and two
    /// of them never race on `meta.json.tmp`. Boxed: every job in the queue
    /// is as big as the biggest kind.
    Meta(Box<Meta>),
}

impl Job {
    /// What the job holds in memory while it waits in the queue.
    fn bytes(&self) -> u64 {
        match self {
            Job::Video { data, .. } | Job::Header(data) => data.len() as u64,
            Job::Audio(_, samples) => samples.len() as u64 * 2,
            Job::Open(_) | Job::Silence(..) | Job::Meta(_) => 0,
        }
    }
}

/// Where the writer puts the picture.
enum VideoOut {
    Ts(TsWriter<BufWriter<File>>),
    Raw(BufWriter<File>),
}

/// The picture's file, and in a TS the header still waiting for a frame.
struct VideoSink {
    out: VideoOut,
    header: Vec<u8>,
}

impl VideoSink {
    fn header(&mut self, data: &[u8]) -> std::io::Result<()> {
        match &mut self.out {
            VideoOut::Ts(_) => {
                self.header.extend_from_slice(data);
                Ok(())
            }
            VideoOut::Raw(out) => out.write_all(data),
        }
    }

    fn frame(&mut self, data: &[u8], pts_us: i64, keyframe: bool) -> std::io::Result<()> {
        match &mut self.out {
            VideoOut::Ts(ts) if self.header.is_empty() => ts.write_frame(pts_us, keyframe, data),
            VideoOut::Ts(ts) => {
                let mut unit = std::mem::take(&mut self.header);
                unit.extend_from_slice(data);
                ts.write_frame(pts_us, keyframe, &unit)
            }
            VideoOut::Raw(out) => out.write_all(data),
        }
    }

    fn close(self) -> std::io::Result<()> {
        match self.out {
            VideoOut::Ts(ts) => ts.finish().map(drop),
            VideoOut::Raw(mut out) => out.flush(),
        }
    }
}

/// The audio side of a running recording.
struct AudioSide {
    /// Has the mixer caught up with the start yet? See [`Recorder::begin_window`].
    attached: bool,
    /// Frames on the shared timeline so far — every track is held to this.
    timeline: u64,
    /// The part of `timeline` that is a gap ([`Recorder::skip_window`]) and
    /// not in the files.
    virtual_frames: u64,
    tracks: Vec<TrackState>,
}

struct TrackState {
    source_id: String,
    frames: u64,
}

/// A running recording.
pub struct Recorder {
    pub dir: PathBuf,
    /// QPC of the first frame — the keyframe the recording began on.
    pub start_100ns: i64,
    pub started: std::time::Instant,
    meta: Mutex<Meta>,
    jobs: Mutex<Option<Sender<Job>>>,
    writer: Mutex<Option<std::thread::JoinHandle<()>>>,
    frames: AtomicU64,
    /// Earliest and latest frame timestamp so far, for `Meta::span_us`.
    first_pts_us: AtomicI64,
    last_pts_us: AtomicI64,
    /// The longest step forward between a frame and the latest one before it,
    /// for `Meta::largest_step_us`.
    largest_step_us: AtomicI64,
    /// The picture's gaps so far, summed: how much the timestamps after the
    /// latest one run ahead of the recording.
    closed_us: AtomicI64,
    video_bytes: AtomicU64,
    failure: Arc<Mutex<Option<String>>>,
    /// Bytes handed to the writer and not yet written.
    queued: Arc<AtomicU64>,
    audio: Mutex<AudioSide>,
}

impl Recorder {
    /// Create the folder and the writer, and write the packets the recording
    /// starts with (see `ReplayBuffer::from_last_keyframe`) behind the
    /// sequence header.
    ///
    /// `timestamped`: the frames come in presentation order and go into
    /// [`VIDEO_TS`] with their times. Otherwise (B-frames) the raw stream
    /// goes into [`VIDEO_FILE`], as before.
    pub fn start(
        clip_dir: &Path,
        meta: Meta,
        sequence_header: &[u8],
        backlog: Vec<EncodedPacket>,
        start_100ns: i64,
        timestamped: bool,
    ) -> Result<Arc<Self>, String> {
        let dir = clip_dir.join(WORK_DIR).join(&meta.id);
        std::fs::create_dir_all(&dir)
            .map_err(|err| format!("could not create '{}': {err}", dir.display()))?;
        let name = if timestamped { VIDEO_TS } else { VIDEO_FILE };
        let video = File::create(dir.join(name))
            .map_err(|err| format!("could not create the recording: {err}"))?;
        write_meta(&dir, &meta)?;

        let (tx, rx) = channel::<Job>();
        let failure = Arc::new(Mutex::new(None));
        let queued = Arc::new(AtomicU64::new(0));
        let writer = {
            let dir = dir.clone();
            let failure = failure.clone();
            let queued = queued.clone();
            std::thread::Builder::new()
                .name("clippiboy-recorder".into())
                .spawn(move || {
                    let video = BufWriter::with_capacity(WRITE_BUFFER, video);
                    let mut video = VideoSink {
                        out: if timestamped {
                            VideoOut::Ts(TsWriter::new(video))
                        } else {
                            VideoOut::Raw(video)
                        },
                        header: Vec::new(),
                    };
                    let mut tracks: Vec<Option<BufWriter<File>>> = Vec::new();
                    let mut broken = false;
                    for job in rx {
                        let bytes = job.bytes();
                        // After the first failure everything else is dropped:
                        // the recording is being stopped, and a half-full disk
                        // only gets fuller.
                        if broken {
                            queued.fetch_sub(bytes, Ordering::Relaxed);
                            continue;
                        }
                        if let Err(err) = run_job(&dir, job, &mut video, &mut tracks) {
                            log::error!("recording: {err}");
                            failure.lock().get_or_insert(err);
                            broken = true;
                        }
                        queued.fetch_sub(bytes, Ordering::Relaxed);
                    }
                    let _ = video.close();
                    for track in tracks.iter_mut().flatten() {
                        let _ = track.flush();
                    }
                })
                .map_err(|err| format!("recorder: {err}"))?
        };

        let recorder = Arc::new(Self {
            dir,
            start_100ns,
            started: std::time::Instant::now(),
            meta: Mutex::new(meta),
            jobs: Mutex::new(Some(tx)),
            writer: Mutex::new(Some(writer)),
            frames: AtomicU64::new(0),
            first_pts_us: AtomicI64::new(i64::MAX),
            last_pts_us: AtomicI64::new(i64::MIN),
            largest_step_us: AtomicI64::new(0),
            closed_us: AtomicI64::new(0),
            video_bytes: AtomicU64::new(0),
            failure,
            queued,
            audio: Mutex::new(AudioSide {
                attached: false,
                timeline: 0,
                virtual_frames: 0,
                tracks: Vec::new(),
            }),
        });

        // SPS/PPS first, as a clip does: most encoders repeat them before every
        // IDR, but they must not be missing.
        recorder.send(Job::Header(sequence_header.to_vec().into()));
        for packet in &backlog {
            recorder.video(packet);
        }
        Ok(recorder)
    }

    fn send(&self, job: Job) {
        let bytes = job.bytes();
        // Raised before the job is handed over, so the writer's subtraction
        // can never run ahead of it and wrap the counter.
        let queued = self.queued.fetch_add(bytes, Ordering::Relaxed) + bytes;
        let sent = match self.jobs.lock().as_ref() {
            Some(tx) => tx.send(job).is_ok(),
            None => false,
        };
        if !sent {
            self.queued.fetch_sub(bytes, Ordering::Relaxed);
            return;
        }
        if let Some(why) = backlog_failure(queued) {
            // The 1 s tick sees it and stops the recording. Nothing waits here.
            self.failure.lock().get_or_insert(why);
        }
    }

    /// One packet from the encoder. Called with the replay buffer locked, so
    /// it must not wait on anything — it only queues.
    pub fn video(&self, packet: &EncodedPacket) {
        if !packet.is_video() {
            return;
        }
        self.frames.fetch_add(1, Ordering::Relaxed);
        self.first_pts_us.fetch_min(packet.pts_us, Ordering::Relaxed);
        let previous = self.last_pts_us.fetch_max(packet.pts_us, Ordering::Relaxed);
        // The first frame has nothing before it; one that arrives behind the
        // latest leaves the step to the next one forward.
        if previous != i64::MIN && packet.pts_us > previous {
            let step = packet.pts_us - previous;
            if step > DISCONTINUITY_US {
                self.close_gap(previous, packet.pts_us);
            } else {
                self.largest_step_us.fetch_max(step, Ordering::Relaxed);
            }
        }
        self.video_bytes
            .fetch_add(packet.data.len() as u64, Ordering::Relaxed);
        // From the first frame, less the gaps so far — including one this
        // frame just closed: the written times run on without a hole.
        let pts_us = packet.pts_us
            - self.first_pts_us.load(Ordering::Relaxed)
            - self.closed_us.load(Ordering::Relaxed);
        self.send(Job::Video {
            data: packet.data.clone(),
            pts_us,
            keyframe: packet.keyframe,
        });
    }

    /// A jump in the picture too long for a stall: the PC slept, or the
    /// encoder hung. It is written down rather than spread over, and the step
    /// does not count as one.
    fn close_gap(&self, previous_us: i64, pts_us: i64) {
        let mut meta = self.meta.lock();
        let period = 1_000_000 / meta.fps.max(1) as i64;
        let first = self.first_pts_us.load(Ordering::Relaxed);
        let end = previous_us + period;
        let gap = VideoGap {
            at_us: end - first,
            len_us: pts_us - end,
        };
        self.closed_us.fetch_add(gap.len_us, Ordering::Relaxed);
        meta.video_gaps.push(gap);
        // Sent under the lock, so that descriptions reach the writer in the
        // order they were made and an older one never lands last.
        self.send(Job::Meta(Box::new(meta.clone())));
    }

    /// How long the recording runs so far, by its frames.
    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    /// Roughly how big the finished file will be: the video as it is, plus
    /// the one AAC track at 192 kbit/s it gets. The individual tracks live
    /// elsewhere and are not counted.
    ///
    /// Not what lies on disk: that includes uncompressed PCM for every source,
    /// and with six of them showed a 211 MB recording as over 2 GB.
    pub fn estimated_size(&self, fps: u32) -> u64 {
        const AAC_BYTES_PER_SECOND: u64 = 192_000 / 8;
        let seconds = self.frames() / fps.max(1) as u64;
        self.video_bytes.load(Ordering::Relaxed) + seconds * AAC_BYTES_PER_SECOND
    }

    /// How many of the opened tracks are summed into the main mix, that is
    /// whose source is not in `separate`.
    pub fn tracks_in_mix(&self, separate: &[String]) -> usize {
        self.audio
            .lock()
            .tracks
            .iter()
            .filter(|t| !separate.contains(&t.source_id))
            .count()
    }

    /// The first thing that went wrong while writing — the disk filling up,
    /// most likely. The recording has to be stopped then.
    pub fn failure(&self) -> Option<String> {
        self.failure.lock().clone()
    }

    /// Called by the mixer before it pushes the window starting at `from`.
    ///
    /// Returns whether the blocks of this window belong to the recording.
    ///
    /// The recording starts on a keyframe that lies in the past, so the first
    /// window the mixer sees is already later than the start. The stretch in
    /// between is still in the track rings, and is read out of them once; from
    /// then on every block is taken as the mixer produces it. Reading every
    /// window back out of the rings would be simpler, but a window read by
    /// timestamp is rounded, and a frame rounded twice at every boundary is a
    /// click every ten milliseconds.
    pub fn begin_window(&self, rings: &[Arc<TrackRing>], from_100ns: i64) -> bool {
        let mut audio = self.audio.lock();
        if !audio.attached {
            // The keyframe can be younger than the mixer's lag; the start is
            // then still ahead and this window is skipped.
            if from_100ns < self.start_100ns {
                return false;
            }
            let frames =
                ((from_100ns - self.start_100ns) * SAMPLE_RATE as i64 / 10_000_000) as u64;
            for ring in rings {
                let index = self.open_track(&mut audio, ring);
                let samples = ring.window(self.start_100ns, frames as usize);
                audio.tracks[index].frames = frames;
                self.send(Job::Audio(index, samples));
            }
            audio.timeline = frames;
            audio.attached = true;
        }
        true
    }

    /// One block of one ring, after [`Self::begin_window`] said yes.
    pub fn audio(&self, ring: &TrackRing, block: &[i16]) {
        let mut audio = self.audio.lock();
        let index = match audio.tracks.iter().position(|t| t.source_id == ring.source_id) {
            Some(index) => index,
            None => self.open_track(&mut audio, ring),
        };
        // A source that joined late — or dropped out and came back — starts
        // with the silence it missed, or its audio would run ahead. Missed on
        // file: a gap is in no track, so none is padded for it.
        let timeline = audio.timeline - audio.virtual_frames;
        let track = &mut audio.tracks[index];
        if track.frames < timeline {
            self.send(Job::Silence(index, timeline - track.frames));
            track.frames = timeline;
        }
        track.frames += (block.len() / CHANNELS) as u64;
        self.send(Job::Audio(index, block.to_vec()));
    }

    /// Close the window the mixer just pushed.
    pub fn end_window(&self, frames: usize) {
        let mut audio = self.audio.lock();
        if audio.attached {
            audio.timeline += frames as u64;
        }
    }

    /// A window the mixer did not mix — the PC slept, or the mixer stalled.
    ///
    /// Counted on the timeline rather than written as zeros: eight hours
    /// asleep were 5.5 GB per track. `finalize` puts back the zeros only where
    /// the picture went on ([`audio_edits`]). The timeline still moves on, or
    /// a stall of the mixer alone would shift the sound for the rest of the
    /// file.
    pub fn skip_window(&self, frames: usize) {
        let mut audio = self.audio.lock();
        if !audio.attached {
            return;
        }
        let gap = AudioGap {
            at_frame: audio.timeline,
            frames: frames as u64,
        };
        {
            let mut meta = self.meta.lock();
            meta.audio_gaps.push(gap);
            self.send(Job::Meta(Box::new(meta.clone())));
        }
        audio.timeline += gap.frames;
        audio.virtual_frames += gap.frames;
    }

    /// A new file for a source the recording has not seen yet.
    fn open_track(&self, audio: &mut AudioSide, ring: &TrackRing) -> usize {
        let index = audio.tracks.len();
        audio.tracks.push(TrackState {
            source_id: ring.source_id.clone(),
            frames: 0,
        });
        self.send(Job::Open(index));
        let mut meta = self.meta.lock();
        meta.tracks.push(MetaTrack {
            source_id: ring.source_id.clone(),
            label: ring.label(),
            file: track_file(index),
        });
        self.send(Job::Meta(Box::new(meta.clone())));
        index
    }

    /// When the recording began, in ms since the epoch.
    pub fn started_at(&self) -> i64 {
        self.meta.lock().created_at
    }

    /// Stop writing and settle the metadata. The folder is then complete and
    /// [`finalize`] can make the recording out of it.
    ///
    /// `separate` and `game` are taken now rather than at the start: the track
    /// assignment may have been changed, and a game detected half way should
    /// still claim the recording.
    pub fn finish(&self, separate: Vec<String>, game: Option<String>) -> Meta {
        // Dropping the sender ends the writer's loop once the queue is empty.
        self.jobs.lock().take();
        if let Some(writer) = self.writer.lock().take() {
            let _ = writer.join();
        }
        let mut meta = self.meta.lock().clone();
        meta.separate = separate;
        if let Some(game) = game {
            // The friends were looked up for the game at the start. Another
            // game claiming the recording at the end is not one they were in.
            if meta.game.as_deref().is_some_and(|start| !start.eq_ignore_ascii_case(&game)) {
                meta.friends.clear();
            }
            meta.game = Some(game);
        }
        meta.frames = Some(self.frames());
        let (first, last) = (
            self.first_pts_us.load(Ordering::Relaxed),
            self.last_pts_us.load(Ordering::Relaxed),
        );
        // The gaps are not played: the picture is spread over what is left.
        let closed = self.closed_us.load(Ordering::Relaxed).max(0) as u64;
        meta.span_us = (first <= last)
            .then(|| crate::muxer::span_of(first, last, meta.fps))
            .flatten()
            .map(|span| span.saturating_sub(closed));
        meta.largest_step_us = Some(self.largest_step_us.load(Ordering::Relaxed));
        // Labels as they stand now — a source renamed mid-recording.
        // A description that cannot be written (drive gone, file held) must not
        // keep the recording open: the caller still gets it, and the folder
        // stays for the next start.
        if let Err(err) = write_meta(&self.dir, &meta) {
            log::error!("recording description not written: {err}");
        }
        meta
    }
}

fn track_file(index: usize) -> String {
    format!("track_{index}.pcm")
}

fn run_job(
    dir: &Path,
    job: Job,
    video: &mut VideoSink,
    tracks: &mut Vec<Option<BufWriter<File>>>,
) -> Result<(), String> {
    match job {
        Job::Video { data, pts_us, keyframe } => {
            video
                .frame(&data, pts_us, keyframe)
                .map_err(|err| format!("could not write the video: {err}"))?;
        }
        Job::Header(data) => {
            video
                .header(&data)
                .map_err(|err| format!("could not write the video: {err}"))?;
        }
        Job::Open(index) => {
            let file = File::create(dir.join(track_file(index)))
                .map_err(|err| format!("could not create an audio track: {err}"))?;
            if tracks.len() <= index {
                tracks.resize_with(index + 1, || None);
            }
            tracks[index] = Some(BufWriter::with_capacity(WRITE_BUFFER / 4, file));
        }
        Job::Audio(index, samples) => {
            let Some(Some(track)) = tracks.get_mut(index) else {
                return Ok(());
            };
            let raw: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
            track
                .write_all(&raw)
                .map_err(|err| format!("could not write the audio: {err}"))?;
        }
        Job::Silence(index, frames) => {
            let Some(Some(track)) = tracks.get_mut(index) else {
                return Ok(());
            };
            write_zeros(track, frames * FRAME_BYTES)
                .map_err(|err| format!("could not write the audio: {err}"))?;
        }
        // Not a reason to stop the recording, as before: `finish` writes it
        // again, and a held file is usually free by then.
        Job::Meta(meta) => {
            if let Err(err) = write_meta(dir, &meta) {
                log::warn!("recording: {err}");
            }
        }
    }
    Ok(())
}

fn write_zeros(out: &mut impl Write, mut bytes: u64) -> std::io::Result<()> {
    let chunk = [0u8; 64 * 1024];
    while bytes > 0 {
        let now = bytes.min(chunk.len() as u64) as usize;
        out.write_all(&chunk[..now])?;
        bytes -= now as u64;
    }
    Ok(())
}

fn write_meta(dir: &Path, meta: &Meta) -> Result<(), String> {
    let text = serde_json::to_string_pretty(meta).map_err(|err| err.to_string())?;
    // Beside it first, then over it: a crash half way through writing must
    // not leave a folder whose description is unreadable.
    let temp = dir.join("meta.json.tmp");
    std::fs::write(&temp, text)
        .and_then(|()| std::fs::rename(&temp, dir.join(META_FILE)))
        .map_err(|err| format!("could not write the recording's description: {err}"))
}

pub fn read_meta(dir: &Path) -> Result<Meta, String> {
    let text = std::fs::read_to_string(dir.join(META_FILE))
        .map_err(|err| format!("no description in '{}': {err}", dir.display()))?;
    serde_json::from_str(&text).map_err(|err| format!("unreadable description: {err}"))
}

/// Bring every track to exactly `frames` frames: cut what runs over, pad what
/// falls short with silence.
///
/// The length comes from the picture, as it does for a clip: the video lasts
/// what `muxer::timing` spreads it over, so audio measured any other way runs
/// ahead the moment a frame was lost.
///
/// Opened for writing, not for appending: on Windows an append handle carries
/// no right to change the length, and cutting failed with "access denied" —
/// which is how a recovered recording once came out with no sound at all.
fn align(path: &Path, frames: u64) -> std::io::Result<()> {
    let want = frames * FRAME_BYTES;
    let mut file = OpenOptions::new().create(true).write(true).truncate(false).open(path)?;
    let have = file.metadata()?.len();
    if have > want {
        file.set_len(want)?;
    } else if have < want {
        file.seek(SeekFrom::End(0))?;
        let mut out = BufWriter::with_capacity(WRITE_BUFFER, &file);
        write_zeros(&mut out, want - have)?;
        out.flush()?;
    }
    Ok(())
}

/// One step of cutting a track to its picture, in frames.
#[derive(Debug, PartialEq)]
enum Edit {
    /// Keep what the file holds.
    Copy(u64),
    /// Drop what the file holds: sound from a stretch whose picture is gone.
    Skip(u64),
    /// Silence the file does not hold: the mixer stalled while the picture
    /// went on.
    Zeros(u64),
}

/// What to do with a track, walking the recording's audio timeline of
/// `timeline_frames`. The track file holds every stretch outside the
/// `audio_gaps` and none inside them; the `video_gaps` are what the finished
/// recording leaves out. The two never line up to the frame — the mixer lags,
/// the pacer skips — so the seam may cost a few zeros, but never a shift.
fn audio_edits(
    timeline_frames: u64,
    video_gaps: &[Range<u64>],
    audio_gaps: &[Range<u64>],
) -> Vec<Edit> {
    let mut cuts: Vec<u64> = vec![0, timeline_frames];
    for gap in video_gaps.iter().chain(audio_gaps) {
        cuts.extend([gap.start.min(timeline_frames), gap.end.min(timeline_frames)]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut edits: Vec<Edit> = Vec::new();
    for piece in cuts.windows(2) {
        let (at, len) = (piece[0], piece[1] - piece[0]);
        let inside = |gaps: &[Range<u64>]| gaps.iter().any(|gap| gap.contains(&at));
        let edit = match (inside(video_gaps), inside(audio_gaps)) {
            (false, false) => Edit::Copy(len),
            (false, true) => Edit::Zeros(len),
            (true, false) => Edit::Skip(len),
            // Neither in the file nor in the recording.
            (true, true) => continue,
        };
        match (edits.last_mut(), edit) {
            (Some(Edit::Copy(n)), Edit::Copy(m))
            | (Some(Edit::Skip(n)), Edit::Skip(m))
            | (Some(Edit::Zeros(n)), Edit::Zeros(m)) => *n += m,
            (_, edit) => edits.push(edit),
        }
    }
    edits
}

/// Where a track lies once it is cut: a name of its own rather than the old
/// one, so that a second try at `finalize` knows it is done and does not cut
/// what is already cut.
fn cut_path(path: &Path) -> PathBuf {
    path.with_extension("cut.pcm")
}

/// Cut a track file by its `edits`, through `<file>.cut`. The result is
/// renamed to [`cut_path`] and only then the uncut file removed; returns the
/// cut one.
///
/// A file that runs out early stops being read: whatever follows is silence,
/// which [`align`] pads in at the end.
fn apply_edits(path: &Path, edits: &[Edit]) -> std::io::Result<PathBuf> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".cut");
    let temp = PathBuf::from(temp);
    let written = (|| {
        let mut input = BufReader::with_capacity(WRITE_BUFFER, File::open(path)?);
        let mut out = BufWriter::with_capacity(WRITE_BUFFER, File::create(&temp)?);
        for edit in edits {
            match *edit {
                Edit::Copy(frames) => {
                    std::io::copy(&mut (&mut input).take(frames * FRAME_BYTES), &mut out)?;
                }
                Edit::Skip(frames) => {
                    input.seek_relative((frames * FRAME_BYTES) as i64)?;
                }
                Edit::Zeros(frames) => write_zeros(&mut out, frames * FRAME_BYTES)?,
            }
        }
        out.flush()
    })();
    if let Err(err) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(err);
    }
    let cut = cut_path(path);
    std::fs::rename(&temp, &cut)?;
    std::fs::remove_file(path)?;
    Ok(cut)
}

/// The tracks of a finished folder, ready for `muxer::encode`: cut to the
/// picture where the recording has gaps, every one `audio_frames` long, and
/// the main mix summed — first in the list, as in a clip.
fn prepare_tracks(dir: &Path, meta: &Meta, audio_frames: u64) -> Result<Vec<AudioInput>, String> {
    let to_frames = |us: i64| us.max(0) as u64 * SAMPLE_RATE as u64 / 1_000_000;
    let video_gaps: Vec<Range<u64>> = meta
        .video_gaps
        .iter()
        .map(|gap| {
            let at = to_frames(gap.at_us);
            at..at + to_frames(gap.len_us)
        })
        .collect();
    let audio_gaps: Vec<Range<u64>> = meta
        .audio_gaps
        .iter()
        .map(|gap| gap.at_frame..gap.at_frame + gap.frames)
        .collect();
    // The timeline still holds the gaps the picture is spread without.
    let timeline_frames =
        audio_frames + video_gaps.iter().map(|gap| gap.end - gap.start).sum::<u64>();
    let edits = if video_gaps.is_empty() && audio_gaps.is_empty() {
        Vec::new()
    } else {
        audio_edits(timeline_frames, &video_gaps, &audio_gaps)
    };
    let rewrite =
        edits.len() > 1 || edits.first().is_some_and(|edit| !matches!(edit, Edit::Copy(_)));

    let mut into_mix: Vec<PathBuf> = Vec::new();
    let mut own: Vec<AudioInput> = Vec::new();
    for track in &meta.tracks {
        let mut path = dir.join(&track.file);
        // Stop rather than leave the track out. Carrying on would write a
        // recording without it and then clear the folder away — the audio
        // would be gone for good. Stopping keeps the folder, and the next
        // start tries again.
        let failed = |err: std::io::Error| {
            format!("could not prepare audio track '{}': {err}", track.label)
        };
        if rewrite {
            let cut = cut_path(&path);
            // Without the uncut file, an earlier try got as far as cutting it.
            // Without either, the track never got a file: `align` makes it
            // silence, as before.
            if path.exists() {
                path = apply_edits(&path, &edits).map_err(failed)?;
            } else if cut.exists() {
                path = cut;
            }
        }
        align(&path, audio_frames).map_err(failed)?;
        if meta.separate.contains(&track.source_id) {
            own.push(AudioInput {
                path,
                label: track.label.clone(),
                raw: true,
            });
        } else {
            into_mix.push(path);
        }
    }

    // The main mix first, as in a clip — the editor lists the tracks in this
    // order.
    let mut inputs: Vec<AudioInput> = Vec::new();
    match into_mix.len() {
        0 => {}
        1 => inputs.push(AudioInput {
            path: into_mix.remove(0),
            label: "Main mix".into(),
            raw: true,
        }),
        _ => {
            let mix = dir.join(MIX_FILE);
            if let Err(err) = sum_tracks(&into_mix, &mix) {
                // A half-written mix of an hour is gigabytes the next try
                // would have to find room beside.
                let _ = std::fs::remove_file(&mix);
                return Err(format!("could not mix the recording's audio: {err}"));
            }
            inputs.push(AudioInput {
                path: mix,
                label: "Main mix".into(),
                raw: true,
            });
        }
    }
    inputs.extend(own);
    Ok(inputs)
}

/// Sum several tracks of the same length into one, block by block — an hour
/// of audio per source does not go through memory in one piece.
///
/// Saturating, like `pipeline::write_mix_wav`: a clipped peak rather than a
/// crack from the sign flipping.
fn sum_tracks(inputs: &[PathBuf], output: &Path) -> std::io::Result<()> {
    const BLOCK: usize = 64 * 1024;
    let mut files = inputs
        .iter()
        .map(File::open)
        .collect::<std::io::Result<Vec<_>>>()?;
    let mut out = BufWriter::with_capacity(WRITE_BUFFER, File::create(output)?);
    let mut sum = vec![0i16; BLOCK / 2];
    let mut raw = vec![0u8; BLOCK];
    loop {
        sum.iter_mut().for_each(|s| *s = 0);
        let mut longest = 0;
        for file in files.iter_mut() {
            let read = read_full(file, &mut raw)?;
            longest = longest.max(read);
            for (slot, pair) in sum.iter_mut().zip(raw[..read].chunks_exact(2)) {
                *slot = slot.saturating_add(i16::from_le_bytes([pair[0], pair[1]]));
            }
        }
        if longest == 0 {
            break;
        }
        for sample in &sum[..longest / 2] {
            out.write_all(&sample.to_le_bytes())?;
        }
    }
    out.flush()
}

fn read_full(file: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// Count the frames of a raw H.264 stream. Only needed after a crash, when the
/// recorder never got to say how many it wrote.
fn count_frames(video: &Path) -> Option<u64> {
    let output = muxer::command("ffprobe")
        .args([
            "-v",
            "error",
            "-f",
            "h264",
            "-count_packets",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=nb_read_packets",
            "-of",
            "csv=p=0",
        ])
        .arg(video)
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// The picture of a finished folder for `muxer::encode`, and how long it
/// lasts in microseconds — what the sound is cut to.
///
/// A [`VIDEO_TS`] brings its own times. Its span is the one a clean stop wrote
/// down, or after a crash the one between its first and last timestamp
/// (REC-6) — no longer the nominal rate, which ran the picture ahead of its
/// sound by every frame lost. A [`VIDEO_FILE`] is spread as it always was.
fn recorded_video(dir: &Path, meta: &Meta) -> Result<(muxer::VideoInput, u64), String> {
    let fps = meta.fps.max(1);
    if meta.frames == Some(0) {
        return Err("The recording is empty.".into());
    }
    let unreadable = || "the recording's video could not be read".to_string();
    let stamped = dir.join(VIDEO_TS);
    if stamped.is_file() {
        let span_us = match meta.span_us {
            Some(span) => span,
            None => ts::span_us(&stamped, fps).ok_or_else(unreadable)?,
        };
        return Ok((muxer::VideoInput::Timestamped(stamped), span_us));
    }
    let video = dir.join(VIDEO_FILE);
    let frames = match meta.frames {
        Some(frames) => frames,
        None => count_frames(&video).ok_or_else(unreadable)?,
    };
    if frames == 0 {
        return Err("The recording is empty.".into());
    }
    let timing = muxer::timing(frames, meta.span_us, meta.largest_step_us, fps);
    let video = muxer::VideoInput::Raw {
        path: video,
        rate: timing.rate,
    };
    Ok((video, timing.span_us))
}

/// Turn a finished folder into the recording at `output`, and clear the folder
/// away once that worked.
///
/// The same path for a clean stop and for the leftovers of a crash: all it
/// needs is what lies in the folder.
///
/// `on_progress` goes from 0 to 1. Summing the main mix takes the first
/// tenth, ffmpeg the rest — it is where the time goes.
pub fn finalize(
    dir: &Path,
    meta: &Meta,
    output: PathBuf,
    on_progress: &dyn Fn(f32),
) -> Result<ClipResult, String> {
    let (video, span_us) = recorded_video(dir, meta)?;
    let audio_frames = span_us * SAMPLE_RATE as u64 / 1_000_000;
    let inputs = prepare_tracks(dir, meta, audio_frames)?;
    on_progress(0.1);

    let length_ms = span_us / 1000;
    let result = muxer::encode(
        &video,
        &inputs,
        &meta.id,
        output,
        Some(length_ms),
        &|share| on_progress(0.1 + share * 0.9),
    );
    let result = match result {
        Ok(result) => result,
        Err(err) => {
            // The folder stays for the next try, but not the summed mix: it
            // is made again from the tracks.
            let _ = std::fs::remove_file(dir.join(MIX_FILE));
            return Err(err);
        }
    };
    if let Err(err) = std::fs::remove_dir_all(dir) {
        log::warn!("recording folder '{}' not cleared: {err}", dir.display());
    }
    if let Some(parent) = dir.parent() {
        // `.recording` itself only when nothing else is in it.
        let _ = std::fs::remove_dir(parent);
    }
    Ok(result)
}

/// A work folder with nothing to finish: no frame was written (a crash in the
/// first half second), or the video file is missing or empty — for a TS, not
/// even one whole packet, or its tables without a frame after them (a crash in
/// the first frames). Decided by the files alone, never by ffprobe, which may
/// simply be absent.
pub fn is_empty_leftover(dir: &Path, meta: &Meta) -> bool {
    let size = |name: &str| std::fs::metadata(dir.join(name)).ok().map(|m| m.len());
    let empty = match size(VIDEO_TS) {
        Some(len) => len < ts::PACKET as u64 || has_no_frame(&dir.join(VIDEO_TS), meta.fps),
        None => size(VIDEO_FILE).is_none_or(|len| len == 0),
    };
    meta.frames == Some(0) || empty
}

/// A TS in which [`ts::span_us`] finds no frame. Only a file that can be read:
/// one that cannot — held open by a scanner, say — may be a whole recording,
/// and an empty leftover is deleted.
fn has_no_frame(path: &Path, fps: u32) -> bool {
    let readable = File::open(path)
        .and_then(|mut file| file.read(&mut [0u8; ts::PACKET]))
        .is_ok();
    readable && ts::span_us(path, fps).is_none()
}

/// Folders left behind by a recording that never stopped — the app crashed, or
/// Windows went down with it.
pub fn leftovers(clip_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(clip_dir.join(WORK_DIR)) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join(META_FILE).is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("clippiboy-rec-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn samples(path: &Path) -> Vec<i16> {
        std::fs::read(path)
            .unwrap()
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect()
    }

    fn frame(pts_us: i64, keyframe: bool) -> EncodedPacket {
        EncodedPacket {
            track: crate::buffer::VIDEO_TRACK,
            pts_us,
            duration_us: 16_666,
            keyframe,
            data: vec![0u8, 1].into(),
        }
    }

    /// SAVE-4: a recording hands its largest step to `finalize`.
    #[test]
    fn a_recording_remembers_its_largest_step() {
        let dir = scratch("step");
        let meta = Meta { id: "s".into(), fps: 60, ..Meta::default() };
        let backlog = vec![frame(0, true), frame(16_666, false), frame(516_666, false)];
        let recorder = Recorder::start(&dir, meta, &[], backlog, 0, true).unwrap();
        assert_eq!(recorder.finish(Vec::new(), None).largest_step_us, Some(500_000));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stopping_needs_room_for_the_summed_mix_too() {
        assert_eq!(mix_scratch_bytes(0, 3600), 0);
        assert_eq!(mix_scratch_bytes(1, 3600), 0, "one track is used as it is");
        assert_eq!(mix_scratch_bytes(2, 3600), 691_200_000);
    }

    #[test]
    fn the_tracks_that_feed_the_main_mix_are_counted() {
        let dir = scratch("in-mix");
        let meta = Meta { id: "m".into(), fps: 60, ..Meta::default() };
        let recorder = Recorder::start(&dir, meta, &[], Vec::new(), 0, true).unwrap();
        let (game, mic) = (ring_with("game"), ring_with("mic"));
        game.push(&[5i16; 12], 0);
        assert!(recorder.begin_window(&[game.clone(), mic.clone()], 1250));
        recorder.audio(&game, &[7i16; 12]);
        recorder.audio(&mic, &[9i16; 12]);
        recorder.end_window(6);
        assert_eq!(recorder.tracks_in_mix(&[]), 2);
        assert_eq!(recorder.tracks_in_mix(&["mic".to_string()]), 1);
        let _ = recorder.finish(Vec::new(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_leftover_is_recognised() {
        let dir = scratch("empty");
        std::fs::write(dir.join(VIDEO_FILE), b"").unwrap();
        assert!(is_empty_leftover(&dir, &Meta::default()));
        std::fs::write(dir.join(VIDEO_FILE), [0u8, 0, 0, 1, 0x65]).unwrap();
        assert!(!is_empty_leftover(&dir, &Meta::default()));
        assert!(is_empty_leftover(&dir, &Meta { frames: Some(0), ..Meta::default() }));
        std::fs::remove_file(dir.join(VIDEO_FILE)).unwrap();
        assert!(is_empty_leftover(&dir, &Meta::default()), "no video at all");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// REC-6: a leftover that wrote its picture as MPEG-TS is told apart
    /// from an empty one by that file, with no `video.h264` beside it.
    #[test]
    fn an_empty_timestamped_leftover_is_recognised() {
        let dir = scratch("empty-ts");
        std::fs::write(dir.join(VIDEO_TS), b"").unwrap();
        assert!(is_empty_leftover(&dir, &Meta::default()));
        // Less than one packet: nothing ffmpeg could read.
        std::fs::write(dir.join(VIDEO_TS), [0x47u8; 100]).unwrap();
        assert!(is_empty_leftover(&dir, &Meta::default()));
        let mut ts = ts::TsWriter::new(Vec::new());
        ts.write_frame(0, true, &[0, 0, 0, 1, 0x65]).unwrap();
        let whole = ts.finish().unwrap();
        // A crash in the first frames: the tables (PAT, PMT) but no frame.
        std::fs::write(dir.join(VIDEO_TS), &whole[..2 * ts::PACKET]).unwrap();
        assert!(is_empty_leftover(&dir, &Meta::default()), "tables alone are nothing to finish");
        std::fs::write(dir.join(VIDEO_TS), &whole).unwrap();
        assert!(!is_empty_leftover(&dir, &Meta::default()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// BUF-6: a recording keeps the time of every frame.
    #[test]
    fn a_recording_keeps_the_times_of_its_frames() {
        let dir = scratch("ts");
        let meta = Meta { id: "t".into(), fps: 60, ..Meta::default() };
        let backlog = vec![frame(0, true), frame(16_666, false), frame(516_666, false)];
        let recorder = Recorder::start(&dir, meta, &[], backlog, 0, true).unwrap();
        let _ = recorder.finish(Vec::new(), None);
        let written = std::fs::read(dir.join(WORK_DIR).join("t").join(VIDEO_TS)).unwrap();
        assert_eq!(ts::read_pts(&written), vec![63_000, 64_500, 109_500]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// REC-1 together with BUF-6: a gap is taken out of the written times.
    #[test]
    fn a_closed_gap_is_taken_out_of_the_written_times() {
        let dir = scratch("tsgap");
        let meta = Meta { id: "g".into(), fps: 60, ..Meta::default() };
        let backlog = [0, 16_666, 600_000_000, 600_016_666]
            .map(|pts| frame(pts, pts % 600_000_000 == 0))
            .to_vec();
        let recorder = Recorder::start(&dir, meta, &[], backlog, 0, true).unwrap();
        let _ = recorder.finish(Vec::new(), None);
        let written = std::fs::read(dir.join(WORK_DIR).join("g").join(VIDEO_TS)).unwrap();
        assert_eq!(ts::read_pts(&written), vec![63_000, 64_500, 66_000, 67_500]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// BUF-6: SPS/PPS are no frame of their own. They ride in front of the
    /// first one, so the times stay one per picture.
    #[test]
    fn the_sequence_header_rides_in_front_of_the_first_frame() {
        let dir = scratch("tshead");
        let meta = Meta { id: "h".into(), fps: 60, ..Meta::default() };
        let header = [0u8, 0, 0, 1, 0x67, 0x42];
        let recorder = Recorder::start(&dir, meta, &header, vec![frame(0, true)], 0, true).unwrap();
        let _ = recorder.finish(Vec::new(), None);
        let written = std::fs::read(dir.join(WORK_DIR).join("h").join(VIDEO_TS)).unwrap();
        assert_eq!(ts::read_pts(&written), vec![63_000]);
        let together = [&header[..], &[0u8, 1]].concat();
        assert!(written.windows(together.len()).any(|w| w == together), "header and frame apart");
        assert_eq!(recorder.queued.load(Ordering::Relaxed), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// BUF-6: an encoder with B-frames sends its pictures out of order, and
    /// the TS carries no DTS. Such a recording stays a raw stream.
    #[test]
    fn a_reordering_encoder_records_raw() {
        let dir = scratch("raw");
        let meta = Meta { id: "r".into(), fps: 60, ..Meta::default() };
        let backlog = vec![frame(0, true), frame(50_000, false), frame(16_666, false)];
        let header = [0u8, 0, 0, 1, 0x67];
        let recorder = Recorder::start(&dir, meta, &header, backlog, 0, false).unwrap();
        let _ = recorder.finish(Vec::new(), None);
        let folder = dir.join(WORK_DIR).join("r");
        assert!(!folder.join(VIDEO_TS).exists());
        let raw = std::fs::read(folder.join(VIDEO_FILE)).unwrap();
        assert_eq!(raw, [&header[..], &[0, 1], &[0, 1], &[0, 1]].concat());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review Focus: a folder from the version before finalizes as it always did.
    #[test]
    fn a_folder_from_before_the_update_still_finalizes_raw() {
        let dir = scratch("old");
        std::fs::write(dir.join("video.h264"), [0u8; 4]).unwrap();
        let meta = Meta { fps: 60, frames: Some(1800), span_us: Some(30_000_000), ..Meta::default() };
        let (video, span) = recorded_video(&dir, &meta).unwrap();
        assert!(matches!(&video, muxer::VideoInput::Raw { rate, .. } if rate == "1800000000/30000000"));
        assert_eq!(span, 30_000_000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// REC-6: a recording that never stopped cleanly has no span in its
    /// description. It is read off the file's timestamps instead of assumed
    /// from the nominal rate.
    #[test]
    fn a_crashed_recording_takes_its_span_from_the_file() {
        let dir = scratch("crashed");
        let mut ts = ts::TsWriter::new(Vec::new());
        // 60 frames, of which 20 were lost: the nominal rate would make 0.67 s of it.
        for k in (0..60).filter(|k| k % 3 != 1) {
            ts.write_frame(k * 16_666, k == 0, &[0, 0, 0, 1, 0x41]).unwrap();
        }
        std::fs::write(dir.join(VIDEO_TS), ts.finish().unwrap()).unwrap();
        let meta = Meta { fps: 60, ..Meta::default() };
        let (video, span) = recorded_video(&dir, &meta).unwrap();
        assert!(matches!(&video, muxer::VideoInput::Timestamped(path) if path == &dir.join(VIDEO_TS)));
        assert!((999_900..=1_000_000).contains(&span), "{span}");
        // A clean stop's own span is taken as it is.
        let meta = Meta { fps: 60, span_us: Some(1_000_000), ..Meta::default() };
        assert_eq!(recorded_video(&dir, &meta).unwrap().1, 1_000_000);
        // No frame at all is an empty recording, not one to mux.
        let meta = Meta { fps: 60, frames: Some(0), ..Meta::default() };
        assert!(recorded_video(&dir, &meta).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_writer_half_a_gigabyte_behind_stops_the_recording() {
        assert_eq!(backlog_failure(WRITER_BACKLOG_LIMIT), None);
        let why = backlog_failure(WRITER_BACKLOG_LIMIT + 1).expect("over the limit");
        assert!(why.contains("cannot keep up"), "{why}");
    }

    #[test]
    fn the_backlog_counter_is_back_at_zero_once_everything_is_written() {
        let dir = scratch("queued");
        let meta = Meta { id: "q".into(), fps: 60, ..Meta::default() };
        let recorder = Recorder::start(&dir, meta, &[0, 0, 0, 1, 0x67], Vec::new(), 0, true).unwrap();
        let ring = ring_with("game");
        assert!(recorder.begin_window(&[ring.clone()], 1250));
        for _ in 0..50 {
            recorder.audio(&ring, &[3i16; 960]);
        }
        recorder.end_window(480);
        let _ = recorder.finish(Vec::new(), None);
        assert_eq!(recorder.queued.load(Ordering::Relaxed), 0);
        assert!(recorder.failure().is_none());
        // After the stop nothing is counted for a job that goes nowhere.
        recorder.audio(&ring, &[1i16; 8]);
        assert_eq!(recorder.queued.load(Ordering::Relaxed), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_description_that_cannot_be_written_still_closes_the_recording() {
        let dir = scratch("meta-fails");
        let meta = Meta { id: "m".into(), fps: 60, ..Meta::default() };
        let recorder = Recorder::start(&dir, meta, &[], Vec::new(), 0, true).unwrap();
        // A folder where the temp file goes: the write fails like a full or vanished drive.
        std::fs::create_dir_all(recorder.dir.join("meta.json.tmp")).unwrap();
        let meta = recorder.finish(Vec::new(), Some("Valorant".into()));
        assert_eq!(meta.frames, Some(0));
        assert_eq!(meta.game.as_deref(), Some("Valorant"));
        assert!(recorder.dir.join(VIDEO_TS).is_file(), "the folder stays for the next start");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_short_track_is_padded_and_a_long_one_cut() {
        let dir = scratch("align");
        let short = dir.join("short.pcm");
        std::fs::write(&short, [1u8, 0, 2, 0]).unwrap();
        align(&short, 3).unwrap();
        assert_eq!(samples(&short), vec![1, 2, 0, 0, 0, 0]);

        let long = dir.join("long.pcm");
        std::fs::write(&long, [1u8, 0, 2, 0, 3, 0, 4, 0]).unwrap();
        align(&long, 1).unwrap();
        assert_eq!(samples(&long), vec![1, 2]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_mix_saturates_like_a_clip() {
        let dir = scratch("sum");
        let a = dir.join("a.pcm");
        let b = dir.join("b.pcm");
        let raw = |values: &[i16]| values.iter().flat_map(|s| s.to_le_bytes()).collect::<Vec<u8>>();
        std::fs::write(&a, raw(&[100, 30000, -30000])).unwrap();
        std::fs::write(&b, raw(&[25, 30000, -30000])).unwrap();
        let out = dir.join("mix.pcm");
        sum_tracks(&[a, b], &out).unwrap();
        assert_eq!(samples(&out), vec![125, i16::MAX, i16::MIN]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn ring_with(id: &str) -> Arc<TrackRing> {
        Arc::new(TrackRing::new(id.into(), id.into(), 5))
    }

    /// The part before the mixer caught up comes out of the ring, the rest
    /// block by block, and a source that joins later gets the silence it
    /// missed — every track ends up on the same timeline.
    #[test]
    fn every_track_lands_on_the_same_timeline() {
        let dir = scratch("timeline");
        let meta = Meta {
            id: "t".into(),
            fps: 60,
            ..Meta::default()
        };
        // 1250 ticks are exactly 6 frames at 48 kHz.
        let recorder = Recorder::start(&dir, meta, &[], Vec::new(), 0, true).unwrap();
        let game = ring_with("game");
        game.push(&[5i16; 12], 0);

        assert!(recorder.begin_window(&[game.clone()], 1250));
        recorder.audio(&game, &[7i16; 12]);
        recorder.end_window(6);

        let mic = ring_with("mic");
        assert!(recorder.begin_window(&[game.clone(), mic.clone()], 2500));
        recorder.audio(&game, &[8i16; 12]);
        recorder.audio(&mic, &[9i16; 12]);
        recorder.end_window(6);

        let meta = recorder.finish(vec!["mic".into()], None);
        let rec = dir.join(WORK_DIR).join("t");
        let game_track = samples(&rec.join(&meta.tracks[0].file));
        let mic_track = samples(&rec.join(&meta.tracks[1].file));
        assert_eq!(game_track.len(), 36);
        assert_eq!(&game_track[..12], &[5i16; 12], "the backfill is missing");
        assert_eq!(&game_track[12..24], &[7i16; 12]);
        assert_eq!(mic_track.len(), 36, "the late source is not on the timeline");
        assert_eq!(&mic_track[..24], &[0i16; 24]);
        assert_eq!(&mic_track[24..], &[9i16; 12]);
        assert_eq!(meta.separate, vec!["mic".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A keyframe younger than the mixer's lag: the first window is skipped
    /// rather than written in front of the picture.
    #[test]
    fn a_window_before_the_start_is_skipped() {
        let dir = scratch("early");
        let meta = Meta {
            id: "e".into(),
            fps: 60,
            ..Meta::default()
        };
        let recorder = Recorder::start(&dir, meta, &[], Vec::new(), 2500, true).unwrap();
        assert!(!recorder.begin_window(&[ring_with("game")], 1250));
        let _ = recorder.finish(Vec::new(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The friends from the start survive a stop in the same game and are
    /// written to the folder, so a crash does not lose them either. Another
    /// game at the stop drops them — they were not in that one.
    #[test]
    fn friends_from_the_start_stay_with_their_game() {
        for (stop_game, kept) in [(None, true), (Some("VALORANT"), true), (Some("Minecraft"), false)] {
            let dir = scratch("friends");
            let meta = Meta {
                id: "f".into(),
                fps: 60,
                game: Some("Valorant".into()),
                friends: vec!["Luca".into()],
                ..Meta::default()
            };
            let recorder = Recorder::start(&dir, meta, &[], Vec::new(), 0, true).unwrap();
            let on_disk = read_meta(&recorder.dir).unwrap();
            assert_eq!(on_disk.friends, vec!["Luca".to_string()]);
            let meta = recorder.finish(Vec::new(), stop_game.map(String::from));
            assert_eq!(!meta.friends.is_empty(), kept, "stop in {stop_game:?}");
            assert_eq!(read_meta(&recorder.dir).unwrap().friends, meta.friends);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    fn write_samples(path: &Path, values: &[i16]) {
        let raw: Vec<u8> = values.iter().flat_map(|s| s.to_le_bytes()).collect();
        std::fs::write(path, raw).unwrap();
    }

    /// REC-1: ten minutes asleep in the middle. The picture's jump is written down
    /// and taken out of the span, instead of stretching the whole recording.
    #[test]
    fn a_sleep_in_the_picture_is_a_gap_not_part_of_the_span() {
        let dir = scratch("sleep");
        let meta = Meta { id: "z".into(), fps: 60, ..Meta::default() };
        let before = (0..60).map(|k| frame(k * 16_666, k == 0));
        let after = (0..60).map(|k| frame(600_000_000 + k * 16_666, k == 0));
        let recorder = Recorder::start(&dir, meta, &[], before.chain(after).collect(), 0, true).unwrap();
        let meta = recorder.finish(Vec::new(), None);
        assert_eq!(meta.video_gaps, vec![VideoGap { at_us: 999_960, len_us: 599_000_040 }]);
        assert_eq!(meta.span_us, Some(1_999_920));
        assert_eq!(meta.largest_step_us, Some(16_666), "the gap is not a step");
        assert_eq!(recorder.closed_us.load(Ordering::Relaxed), 599_000_040);
        assert_eq!(read_meta(&recorder.dir).unwrap().video_gaps, meta.video_gaps);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// REC-1: a stall the encoder or the pacer recovers from is no sleep. It
    /// stays in the span, as `muxer::timing` expects.
    #[test]
    fn a_stall_shorter_than_two_seconds_is_not_a_gap() {
        let dir = scratch("stall");
        let meta = Meta { id: "st".into(), fps: 60, ..Meta::default() };
        let backlog = vec![frame(0, true), frame(16_666, false), frame(1_516_666, false)];
        let recorder = Recorder::start(&dir, meta, &[], backlog, 0, true).unwrap();
        let meta = recorder.finish(Vec::new(), None);
        assert!(meta.video_gaps.is_empty());
        assert_eq!(meta.span_us, Some(1_533_332));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// REC-1: the mixer's gap is counted, not written. Eight hours asleep used to
    /// be 5.5 GB of zeros per track.
    #[test]
    fn a_sleep_writes_no_zeros() {
        let dir = scratch("nozeros");
        let meta = Meta { id: "n".into(), fps: 60, ..Meta::default() };
        let recorder = Recorder::start(&dir, meta, &[], Vec::new(), 0, true).unwrap();
        let game = ring_with("game");
        game.push(&[5i16; 12], 0);
        assert!(recorder.begin_window(&[game.clone()], 1250));
        recorder.audio(&game, &[7i16; 12]);
        recorder.end_window(6);
        recorder.skip_window(28_800_000);
        assert!(recorder.begin_window(&[game.clone()], 6_000_001_250));
        recorder.audio(&game, &[8i16; 12]);
        recorder.end_window(6);
        let meta = recorder.finish(Vec::new(), None);
        let track = samples(&dir.join(WORK_DIR).join("n").join(&meta.tracks[0].file));
        assert_eq!(track.len(), 36, "zeros were written for the sleep");
        assert_eq!(meta.audio_gaps, vec![AudioGap { at_frame: 12, frames: 28_800_000 }]);
        assert_eq!(recorder.queued.load(Ordering::Relaxed), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// REC-1: a source that turns up after the wake is held to the timeline
    /// on file, which the sleep is not part of.
    #[test]
    fn a_source_joining_after_a_sleep_gets_only_the_silence_it_missed() {
        let dir = scratch("join-after-sleep");
        let meta = Meta { id: "j".into(), fps: 60, ..Meta::default() };
        let recorder = Recorder::start(&dir, meta, &[], Vec::new(), 0, true).unwrap();
        let game = ring_with("game");
        game.push(&[5i16; 12], 0);
        assert!(recorder.begin_window(&[game.clone()], 1250));
        recorder.audio(&game, &[7i16; 12]);
        recorder.end_window(6);
        recorder.skip_window(28_800_000);
        let mic = ring_with("mic");
        assert!(recorder.begin_window(&[game.clone(), mic.clone()], 6_000_001_250));
        recorder.audio(&game, &[8i16; 12]);
        recorder.audio(&mic, &[9i16; 12]);
        recorder.end_window(6);
        let meta = recorder.finish(Vec::new(), None);
        let rec = dir.join(WORK_DIR).join("j");
        let mic_track = samples(&rec.join(&meta.tracks[1].file));
        assert_eq!(mic_track.len(), 36, "the sleep was padded in");
        assert_eq!(&mic_track[..24], &[0i16; 24]);
        assert_eq!(&mic_track[24..], &[9i16; 12]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// REC-1, Review Focus: the mixer's gap and the picture's gap never line up to
    /// the frame (mixer lag, pacer skip). The seam costs a few zeros and never a shift.
    #[test]
    fn gaps_that_do_not_line_up_leave_only_zeros_at_the_seam() {
        assert_eq!(
            audio_edits(100, &[40..90], &[44..92]),
            vec![Edit::Copy(40), Edit::Skip(4), Edit::Zeros(2), Edit::Copy(8)]
        );
    }

    /// REC-1: the other cases, in frames on the recording's timeline.
    #[test]
    fn audio_edits_follow_both_gaps() {
        assert_eq!(audio_edits(100, &[], &[]), vec![Edit::Copy(100)]);
        // Sleep: picture and sound stopped together.
        assert_eq!(audio_edits(100, &[40..90], &[40..90]), vec![Edit::Copy(50)]);
        // Only the mixer stalled: its zeros come back, and nothing shifts.
        assert_eq!(audio_edits(100, &[], &[30..60]), vec![Edit::Copy(30), Edit::Zeros(30), Edit::Copy(40)]);
        // Only the encoder hung: that sound goes together with the picture it belongs to.
        assert_eq!(audio_edits(100, &[30..60], &[]), vec![Edit::Copy(30), Edit::Skip(30), Edit::Copy(40)]);
    }

    /// REC-1: the seam edits above, done to a file. What comes out is as long
    /// as the picture, and the sound after the seam is the sound after the wake.
    #[test]
    fn a_cut_track_is_exactly_as_long_as_the_picture() {
        let dir = scratch("cut");
        let path = dir.join("track_0.pcm");
        let ramp: Vec<i16> = (0..52i16).flat_map(|k| [k; CHANNELS]).collect();
        write_samples(&path, &ramp);
        let cut = apply_edits(&path, &audio_edits(100, &[40..90], &[44..92])).unwrap();
        let out = samples(&cut);
        assert_eq!(out.len(), 50 * CHANNELS);
        let at = |frame: usize| out[frame * CHANNELS];
        assert!((0..40).all(|k| at(k) == k as i16));
        assert_eq!((at(40), at(41)), (0, 0));
        assert!((42..50).all(|k| at(k) == k as i16 + 2));
        assert!(!path.exists(), "the uncut track is left lying beside the cut one");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn slept_folder(name: &str) -> (PathBuf, Meta) {
        let dir = scratch(name);
        let track: Vec<i16> = std::iter::repeat_n(1i16, 48_000 * CHANNELS)
            .chain(std::iter::repeat_n(2i16, 48_000 * CHANNELS))
            .collect();
        let mut tracks = Vec::new();
        for (index, id) in ["game", "mic"].into_iter().enumerate() {
            write_samples(&dir.join(track_file(index)), &track);
            tracks.push(MetaTrack {
                source_id: id.into(),
                label: id.into(),
                file: track_file(index),
            });
        }
        let meta = Meta {
            id: "w".into(),
            fps: 60,
            tracks,
            video_gaps: vec![VideoGap { at_us: 1_000_000, len_us: 600_000_000 }],
            audio_gaps: vec![AudioGap { at_frame: 48_000, frames: 28_800_000 }],
            ..Meta::default()
        };
        (dir, meta)
    }

    /// REC-1: a recording that slept between its two seconds keeps both of
    /// them, the second where it belongs — the sound after the wake is not
    /// cut off as running past the picture.
    #[test]
    fn a_slept_recording_keeps_its_sound_after_the_wake() {
        let (dir, meta) = slept_folder("slept");
        let inputs = prepare_tracks(&dir, &meta, 96_000).unwrap();
        let mix = samples(&inputs[0].path);
        assert_eq!(mix.len(), 96_000 * CHANNELS);
        assert_eq!(mix[0], 2);
        assert_eq!(mix[50_000 * CHANNELS], 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// REC-1: a stop whose ffmpeg failed keeps the folder, and the next try
    /// prepares the tracks again. A track that is cut already is not cut a
    /// second time — that would throw away sound after the seam.
    #[test]
    fn a_second_try_does_not_cut_a_track_twice() {
        let dir = scratch("cut-twice");
        let ramp: Vec<i16> = (0..52i16).flat_map(|k| [k; CHANNELS]).collect();
        write_samples(&dir.join(track_file(0)), &ramp);
        let meta = Meta {
            id: "c".into(),
            fps: 60,
            tracks: vec![MetaTrack {
                source_id: "game".into(),
                label: "Game".into(),
                file: track_file(0),
            }],
            // Frames 40..90 of the picture, in µs (a frame is 20.83 µs), and
            // 44..92 of the sound: the seam above.
            video_gaps: vec![VideoGap { at_us: 834, len_us: 1_042 }],
            audio_gaps: vec![AudioGap { at_frame: 44, frames: 48 }],
            ..Meta::default()
        };
        let first = samples(&prepare_tracks(&dir, &meta, 50).unwrap()[0].path);
        let second = samples(&prepare_tracks(&dir, &meta, 50).unwrap()[0].path);
        assert_eq!(first.len(), 50 * CHANNELS);
        assert_eq!(first[42 * CHANNELS], 44, "the cut is not the seam's");
        assert_eq!(first, second);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn leftovers_are_found_by_their_description() {
        let dir = scratch("leftovers");
        let work = dir.join(WORK_DIR);
        std::fs::create_dir_all(work.join("a")).unwrap();
        std::fs::create_dir_all(work.join("b")).unwrap();
        write_meta(&work.join("a"), &Meta::default()).unwrap();
        assert_eq!(leftovers(&dir), vec![work.join("a")]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
