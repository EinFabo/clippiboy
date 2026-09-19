//! A recording started and stopped by hand.
//!
//! It rides on the same pipeline as the replay buffer — one encoder, one mixer
//! — and only taps what already flows: every video packet the encoder hands
//! out, every audio block the mixer pushes into a track ring. Nothing is held
//! in memory. A writer thread streams it all into a folder of its own,
//!
//! ```text
//! <clip folder>/.recording/<id>/video.h264    the raw elementary stream
//! <clip folder>/.recording/<id>/track_<n>.pcm one per source, 16-bit, 48 kHz
//! <clip folder>/.recording/<id>/meta.json     what the files are
//! ```
//!
//! and stopping muxes that into an MP4 the same way a clip is written
//! (`muxer::encode`). The folder sits inside the clip folder on purpose: the
//! finished file is renamed out of it, and a rename across drives is a copy of
//! an hour of video.
//!
//! Should the app die mid-recording the folder is still there, whole up to
//! the last write, and the next start finds it ([`leftovers`]) and finishes it.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::audio::capture::{CHANNELS, SAMPLE_RATE};
use crate::buffer::EncodedPacket;
use crate::muxer::{self, AudioInput, ClipResult};
use crate::pipeline::TrackRing;

/// The folder inside the clip folder that unfinished recordings live in.
pub const WORK_DIR: &str = ".recording";

const VIDEO_FILE: &str = "video.h264";
const META_FILE: &str = "meta.json";
const MIX_FILE: &str = "mix.pcm";

/// Bytes per audio frame: 16 bit per sample, [`CHANNELS`] samples.
const FRAME_BYTES: u64 = 2 * CHANNELS as u64;

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
    Video(Arc<[u8]>),
    Open(usize),
    Audio(usize, Vec<i16>),
    /// Zeros, counted in frames rather than handed over — a source that joins
    /// an hour in would otherwise be an allocation of an hour of silence.
    Silence(usize, u64),
}

/// The audio side of a running recording.
struct AudioSide {
    /// Has the mixer caught up with the start yet? See [`Recorder::begin_window`].
    attached: bool,
    /// Frames on the shared timeline so far — every track is held to this.
    timeline: u64,
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
    video_bytes: AtomicU64,
    failure: Arc<Mutex<Option<String>>>,
    audio: Mutex<AudioSide>,
}

impl Recorder {
    /// Create the folder and the writer, and write the packets the recording
    /// starts with (see `ReplayBuffer::from_last_keyframe`) behind the
    /// sequence header.
    pub fn start(
        clip_dir: &Path,
        meta: Meta,
        sequence_header: &[u8],
        backlog: Vec<EncodedPacket>,
        start_100ns: i64,
    ) -> Result<Arc<Self>, String> {
        let dir = clip_dir.join(WORK_DIR).join(&meta.id);
        std::fs::create_dir_all(&dir)
            .map_err(|err| format!("could not create '{}': {err}", dir.display()))?;
        let video = File::create(dir.join(VIDEO_FILE))
            .map_err(|err| format!("could not create the recording: {err}"))?;
        write_meta(&dir, &meta)?;

        let (tx, rx) = channel::<Job>();
        let failure = Arc::new(Mutex::new(None));
        let writer = {
            let dir = dir.clone();
            let failure = failure.clone();
            std::thread::Builder::new()
                .name("clippiboy-recorder".into())
                .spawn(move || {
                    let mut video = BufWriter::with_capacity(WRITE_BUFFER, video);
                    let mut tracks: Vec<Option<BufWriter<File>>> = Vec::new();
                    let mut broken = false;
                    for job in rx {
                        // After the first failure everything else is dropped:
                        // the recording is being stopped, and a half-full disk
                        // only gets fuller.
                        if broken {
                            continue;
                        }
                        if let Err(err) = run_job(&dir, job, &mut video, &mut tracks) {
                            log::error!("recording: {err}");
                            *failure.lock() = Some(err);
                            broken = true;
                        }
                    }
                    let _ = video.flush();
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
            video_bytes: AtomicU64::new(0),
            failure,
            audio: Mutex::new(AudioSide {
                attached: false,
                timeline: 0,
                tracks: Vec::new(),
            }),
        });

        // SPS/PPS first, as a clip does: most encoders repeat them before every
        // IDR, but they must not be missing.
        recorder.send(Job::Video(sequence_header.to_vec().into()));
        for packet in &backlog {
            recorder.video(packet);
        }
        Ok(recorder)
    }

    fn send(&self, job: Job) {
        if let Some(tx) = self.jobs.lock().as_ref() {
            let _ = tx.send(job);
        }
    }

    /// One packet from the encoder. Called with the replay buffer locked, so
    /// it must not wait on anything — it only queues.
    pub fn video(&self, packet: &EncodedPacket) {
        if !packet.is_video() {
            return;
        }
        self.frames.fetch_add(1, Ordering::Relaxed);
        self.video_bytes
            .fetch_add(packet.data.len() as u64, Ordering::Relaxed);
        self.send(Job::Video(packet.data.clone()));
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
        // with the silence it missed, or its audio would run ahead.
        let timeline = audio.timeline;
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
        if let Err(err) = write_meta(&self.dir, &meta) {
            log::warn!("recording: {err}");
        }
        index
    }

    /// Stop writing and settle the metadata. The folder is then complete and
    /// [`finalize`] can make the recording out of it.
    ///
    /// `separate` and `game` are taken now rather than at the start: the track
    /// assignment may have been changed, and a game detected half way should
    /// still claim the recording.
    pub fn finish(&self, separate: Vec<String>, game: Option<String>) -> Result<Meta, String> {
        // Dropping the sender ends the writer's loop once the queue is empty.
        self.jobs.lock().take();
        if let Some(writer) = self.writer.lock().take() {
            let _ = writer.join();
        }
        let mut meta = self.meta.lock().clone();
        meta.separate = separate;
        meta.game = game.or(meta.game);
        meta.frames = Some(self.frames());
        // Labels as they stand now — a source renamed mid-recording.
        write_meta(&self.dir, &meta)?;
        Ok(meta)
    }
}

fn track_file(index: usize) -> String {
    format!("track_{index}.pcm")
}

fn run_job(
    dir: &Path,
    job: Job,
    video: &mut BufWriter<File>,
    tracks: &mut Vec<Option<BufWriter<File>>>,
) -> Result<(), String> {
    match job {
        Job::Video(data) => {
            video
                .write_all(&data)
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
/// The length comes from the picture, as it does for a clip: the video is
/// muxed from `-r fps` and is exactly as long as it has frames, so audio
/// measured any other way runs ahead the moment a frame was lost.
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
    let video = dir.join(VIDEO_FILE);
    let fps = meta.fps.max(1);
    let frames = match meta.frames {
        Some(frames) => frames,
        None => count_frames(&video)
            .ok_or_else(|| "the recording's video could not be read".to_string())?,
    };
    if frames == 0 {
        return Err("The recording is empty.".into());
    }
    let audio_frames = frames * SAMPLE_RATE as u64 / fps as u64;

    let mut into_mix: Vec<PathBuf> = Vec::new();
    let mut own: Vec<AudioInput> = Vec::new();
    for track in &meta.tracks {
        let path = dir.join(&track.file);
        // Stop rather than leave the track out. Carrying on would write a
        // recording without it and then clear the folder away — the audio
        // would be gone for good. Stopping keeps the folder, and the next
        // start tries again.
        align(&path, audio_frames).map_err(|err| {
            format!("could not prepare audio track '{}': {err}", track.label)
        })?;
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
            sum_tracks(&into_mix, &mix)
                .map_err(|err| format!("could not mix the recording's audio: {err}"))?;
            inputs.push(AudioInput {
                path: mix,
                label: "Main mix".into(),
                raw: true,
            });
        }
    }
    inputs.extend(own);
    on_progress(0.1);

    let length_ms = frames * 1000 / fps as u64;
    let result = muxer::encode(
        &video,
        fps,
        &inputs,
        &meta.id,
        output,
        Some(length_ms),
        &|share| on_progress(0.1 + share * 0.9),
    )?;
    if let Err(err) = std::fs::remove_dir_all(dir) {
        log::warn!("recording folder '{}' not cleared: {err}", dir.display());
    }
    if let Some(parent) = dir.parent() {
        // `.recording` itself only when nothing else is in it.
        let _ = std::fs::remove_dir(parent);
    }
    Ok(result)
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
        let recorder = Recorder::start(&dir, meta, &[], Vec::new(), 0).unwrap();
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

        let meta = recorder.finish(vec!["mic".into()], None).unwrap();
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
        let recorder = Recorder::start(&dir, meta, &[], Vec::new(), 2500).unwrap();
        assert!(!recorder.begin_window(&[ring_with("game")], 1250));
        let _ = recorder.finish(Vec::new(), None);
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
