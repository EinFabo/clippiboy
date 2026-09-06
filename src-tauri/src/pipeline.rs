//! Recording pipeline: screen → NV12 → hardware encoder → packet ring.
//!
//! Recording runs through **one** continuous encoder. The finished H.264 packets
//! land in the ring buffer from `buffer.rs`, the audio alongside as PCM in rings
//! on the same timeline. Saving is nothing but muxing — no re-encoding, no
//! gluing files together.
//!
//! It used to work differently: every ten seconds a new encoder was set up,
//! written into an MPEG-TS file, and stitched together with `ffmpeg concat` on
//! save. Every changeover cost a forced keyframe plus the time between the swap
//! and the first new frame — the encoder always stamps its first frame at zero,
//! so that gap quietly fell under the table. That was the regular hitch, and it
//! was the reason for all the effort around segment boundaries, `+genpts` and
//! the `-ss` that did not work.
//!
//! Video and audio now both hang off the QPC: Windows.Graphics.Capture stamps
//! its frames with `SystemRelativeTime`, WASAPI its blocks with
//! `pu64QPCPosition` — the same clock. Sync is therefore no longer a
//! calculation; it simply follows.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use crate::audio::capture::{now_100ns, CHANNELS, SAMPLE_RATE};
use crate::audio::engine::AudioEngine;
use crate::audio::recorded;
use crate::buffer::{EncodedPacket, ReplayBuffer};
use crate::model::{AudioSource, EncoderId, RateControl, RecordingConfig};

/// How far behind the present the mixer stays.
///
/// WASAPI only delivers a block once it is full; a few milliseconds pass before
/// it is in the ring. Mixing closer to the present fetches silence that would
/// later have had to be replaced by real audio — and the slot is already taken
/// by then.
const AUDIO_LAG_100NS: i64 = 80 * 10_000;

/// The mixer's tick.
const MIX_INTERVAL: Duration = Duration::from_millis(10);

/// Ring buffer of one audio track (16-bit PCM), on the same QPC timeline as the
/// video.
pub struct TrackRing {
    pub source_id: String,
    /// Changeable: renaming a source must not cost it its track — the ring stays
    /// the same, only the label changes.
    label: Mutex<String>,
    inner: Mutex<TrackInner>,
    capacity: usize,
}

struct TrackInner {
    samples: VecDeque<i16>,
    /// QPC of the first sample in the ring.
    start_100ns: i64,
    primed: bool,
    /// Silence that has come in but is not stored — see [`TrackRing::push`].
    /// Counted in samples and never larger than the ring itself.
    silence: usize,
}

impl TrackRing {
    fn new(source_id: String, label: String, seconds: u32) -> Self {
        Self {
            source_id,
            label: Mutex::new(label),
            inner: Mutex::new(TrackInner {
                samples: VecDeque::new(),
                start_100ns: 0,
                primed: false,
                silence: 0,
            }),
            // One buffer across the full clip length, plus some slack.
            capacity: (seconds as usize + 2) * SAMPLE_RATE as usize * CHANNELS,
        }
    }

    pub fn label(&self) -> String {
        self.label.lock().clone()
    }

    fn set_label(&self, label: String) {
        *self.label.lock() = label;
    }

    /// The mixer produces gapless consecutive windows; `at_100ns` therefore only
    /// sets the timeline on the very first stored block.
    ///
    /// **Silence is counted, not stored.** A ring holding minutes of a source
    /// that says nothing would be minutes of zeros, and there are as many of
    /// those rings as there are sources: a device nobody plays on, a microphone
    /// on mute, a chat channel between two sentences. Instead the quiet blocks
    /// only raise a counter, and the moment real audio arrives it is turned into
    /// the zeros it stands for — so the new block cannot slide forward in time.
    ///
    /// Silence longer than the whole ring means everything stored is older than
    /// the buffer anyway. Then the ring is emptied and hands its memory back —
    /// a device nobody has played on for two minutes costs nothing at all.
    fn push(&self, block: &[i16], at_100ns: i64) {
        let mut inner = self.inner.lock();

        if block.iter().all(|sample| *sample == 0) {
            // Before the first real block there is nothing to count: leading
            // silence is what the read fills in by itself, and it must not set
            // the timeline.
            if !inner.primed {
                return;
            }
            inner.silence += block.len();
            // Quiet for longer than the ring is long: everything in it is now
            // older than the buffer and nobody will ever read it again. Give
            // the memory back instead of holding minutes of nothing.
            if inner.silence >= self.capacity {
                inner.samples.clear();
                inner.samples.shrink_to_fit();
                inner.primed = false;
                inner.silence = 0;
            }
            return;
        }

        // Deliberately counted in samples rather than worked out from the
        // clock: exactly as many zeros come back as were pushed in, so the
        // stream is sample for sample the one the ring would hold if it had
        // stored every quiet block. A gap worked out from timestamps would be
        // off by the rounding of every single window.
        let gap = std::mem::take(&mut inner.silence);

        if !inner.primed {
            inner.start_100ns = at_100ns;
            inner.primed = true;
        } else {
            inner.samples.extend(std::iter::repeat(0).take(gap));
        }

        inner.samples.extend(block.iter().copied());
        if inner.samples.len() > self.capacity {
            let excess = inner.samples.len() - self.capacity;
            inner.samples.drain(..excess);
            let frames = (excess / CHANNELS) as i64;
            inner.start_100ns += frames * 10_000_000 / SAMPLE_RATE as i64;
        }
    }

    /// The window starting at `from_100ns` over `frames` frames, as interleaved
    /// PCM.
    ///
    /// Missing stretches become silence — that keeps the track exactly as long as
    /// the video, even when a source only joined later.
    fn window(&self, from_100ns: i64, frames: usize) -> Vec<i16> {
        let wanted = frames * CHANNELS;
        let mut data = vec![0i16; wanted];
        {
            let inner = self.inner.lock();
            if inner.primed {
                let offset_frames =
                    (from_100ns - inner.start_100ns) * SAMPLE_RATE as i64 / 10_000_000;
                let offset = offset_frames * CHANNELS as i64;
                let (mut src, mut dst) = if offset < 0 {
                    (0usize, (-offset) as usize)
                } else {
                    (offset as usize, 0usize)
                };
                while dst < wanted {
                    let Some(sample) = inner.samples.get(src) else {
                        break;
                    };
                    data[dst] = *sample;
                    src += 1;
                    dst += 1;
                }
            }
        }

        data
    }

    /// Write this track's window as WAV.
    pub fn write_wav_window(
        &self,
        path: &Path,
        from_100ns: i64,
        frames: usize,
    ) -> std::io::Result<()> {
        write_wav(path, &self.window(from_100ns, frames))
    }
}

/// Write several tracks summed into one WAV — the main mix.
///
/// It is not recorded as one: every source keeps a ring of its own, and the sum
/// is drawn only here, out of whatever has no track of its own at the moment the
/// clip is saved. That is what makes the ⧉ switch work backwards over the whole
/// buffer instead of only from the moment it was flipped.
///
/// Summed with saturation rather than wrapping — the same reasoning as the hard
/// clipping in the mixer: a clipped peak is better than a crack.
pub fn write_mix_wav(
    tracks: &[&TrackRing],
    path: &Path,
    from_100ns: i64,
    frames: usize,
) -> std::io::Result<()> {
    let mut sum = vec![0i16; frames * CHANNELS];
    for track in tracks {
        for (slot, sample) in sum.iter_mut().zip(track.window(from_100ns, frames)) {
            *slot = slot.saturating_add(sample);
        }
    }
    write_wav(path, &sum)
}

/// PCM into a WAV file — 16 bit, [`CHANNELS`] channels, [`SAMPLE_RATE`].
fn write_wav(path: &Path, data: &[i16]) -> std::io::Result<()> {
    let byte_len = (data.len() * 2) as u32;
    let mut out = Vec::with_capacity(byte_len as usize + 44);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + byte_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&(CHANNELS as u16).to_le_bytes());
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * CHANNELS as u32 * 2).to_le_bytes());
    out.extend_from_slice(&((CHANNELS * 2) as u16).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&byte_len.to_le_bytes());
    for sample in data {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, out)
}

/// State shared by capture, encoder and mixer.
pub struct Shared {
    pub buffer_seconds: u32,
    pub fps: u32,
    /// The encoded video packets.
    pub packets: Mutex<ReplayBuffer>,
    /// One ring per enabled source — the main mix is not among them, it is
    /// summed out of these on save.
    pub tracks: Mutex<Vec<Arc<TrackRing>>>,
    pub sources: Mutex<Vec<AudioSource>>,
    pub audio: Arc<AudioEngine>,
    /// The QPC that PTS 0 of the video stream falls on. This is what lets the
    /// real capture time be determined for every packet — and the audio cut at
    /// exactly the same point.
    pub base_100ns: AtomicI64,
    pub anchored: AtomicBool,
    pub frames: AtomicU64,
    pub dropped: AtomicU64,
    /// Frames the clock had to repeat because the picture stood still.
    pub duplicated: AtomicU64,
    pub error: Mutex<Option<String>>,
    /// Has this run's first error already been reported? An encoder that keeps
    /// stumbling should not flood the UI — but *once* somebody has to hear about
    /// it, otherwise the app appears to keep buffering and only the key press
    /// brings it to light.
    pub error_seen: AtomicBool,
    /// Bumped when `sources` changes. The mixer keeps a copy and only re-reads
    /// when the number has moved.
    pub sources_generation: AtomicU64,
    /// Which encoder is really running — not the requested one.
    pub encoder: Mutex<Option<EncoderId>>,
    /// And what it agreed to do about bitrate — likewise not the wish, see
    /// [`crate::model::RateControl`].
    pub rate_control: Mutex<Option<RateControl>>,
    /// SPS/PPS that belong in front of the elementary stream when saving.
    pub sequence_header: Mutex<Vec<u8>>,
    /// What the encoder is costing, once one is running.
    #[cfg(windows)]
    pub encoder_stats: Mutex<Option<Arc<crate::mft::EncoderStats>>>,
    /// The clock, so its overrun count can be read from outside.
    #[cfg(windows)]
    pub pacer: Mutex<Option<Arc<crate::convert::Latest>>>,
    /// What [`Self::health`] saw last time. Every number here is a *rate*, and
    /// a rate needs two readings.
    health_mark: Mutex<HealthMark>,
}

#[derive(Clone, Copy)]
struct HealthMark {
    at: std::time::Instant,
    frames: u64,
    encoded: u64,
    encode_us: u64,
}

/// What the pipeline is doing, measured over the interval since the last look.
///
/// The interval is the whole point. A cumulative average is the shape of a
/// number that hides exactly the fault we are hunting: an encoder that starts
/// out fast and gets slower drags its own average up so gently that ten seconds
/// of steady decline still read as "a bit above budget". The first report we
/// got back from AMD hardware had to be differentiated by hand to see it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Health {
    /// Frames per second over the interval, measured against the wall clock.
    pub fps: f32,
    pub frames: u64,
    pub dropped: u64,
    pub duplicated: u64,
    pub overruns: u64,
    /// Mean time a frame spent inside the encoder **during this interval**.
    pub encode_us: u64,
    /// Worst single frame since the recording started.
    pub encode_worst_us: u64,
    pub in_flight: u64,
    pub submit_waited: u64,
    pub submit_wait_ms: u64,
    pub ring_seconds: f32,
    pub ring_bytes: u64,
}

impl Shared {
    pub fn buffered_seconds(&self) -> f32 {
        self.packets.lock().buffered_seconds()
    }

    pub fn buffer_bytes(&self) -> u64 {
        self.packets.lock().bytes()
    }

    /// Take on a new source list and let the mixer know.
    pub fn set_sources(&self, sources: Vec<AudioSource>) {
        *self.sources.lock() = sources;
        self.sources_generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Measure the interval since the last call. See [`Health`].
    ///
    /// The frame rate here is **measured** — frames since the last call divided
    /// by the time since the last call. `EngineStatus::fps` is something else:
    /// the configured rate multiplied by the share of frames that were not
    /// repeats, summed over the whole session. That answers "how still was the
    /// picture", not "is this machine keeping up", and until now it was the
    /// only number anybody had.
    ///
    /// Call it on a timer; it consumes the interval it measures.
    pub fn health(&self) -> Health {
        let now = std::time::Instant::now();
        let frames = self.frames.load(Ordering::Relaxed);

        let mut out = Health {
            frames,
            dropped: self.dropped.load(Ordering::Relaxed),
            duplicated: self.duplicated.load(Ordering::Relaxed),
            ring_seconds: self.buffered_seconds(),
            ring_bytes: self.buffer_bytes(),
            ..Health::default()
        };

        let mut encoded = 0u64;
        let mut encode_us = 0u64;

        #[cfg(windows)]
        {
            if let Some(pacer) = self.pacer.lock().as_ref() {
                out.overruns = pacer.overruns.load(Ordering::Relaxed);
            }
            if let Some(stats) = self.encoder_stats.lock().as_ref() {
                encoded = stats.encoded.load(Ordering::Relaxed);
                encode_us = stats.encode_us.load(Ordering::Relaxed);
                out.encode_worst_us = stats.encode_worst_us.load(Ordering::Relaxed);
                out.in_flight = stats.in_flight.load(Ordering::Relaxed);
                out.submit_waited = stats.submit_waited.load(Ordering::Relaxed);
                out.submit_wait_ms = stats.submit_wait_us.load(Ordering::Relaxed) / 1000;
            }
        }

        let before = {
            let mut mark = self.health_mark.lock();
            let previous = *mark;
            *mark = HealthMark {
                at: now,
                frames,
                encoded,
                encode_us,
            };
            previous
        };

        let seconds = now.duration_since(before.at).as_secs_f32();
        if seconds > 0.0 {
            out.fps = frames.saturating_sub(before.frames) as f32 / seconds;
        }
        // Only the frames that came out during this interval, and only the time
        // they took. This is the number that moves when an encoder starts to
        // struggle; the running average is the one that does not.
        let fresh = encoded.saturating_sub(before.encoded);
        if fresh > 0 {
            out.encode_us = encode_us.saturating_sub(before.encode_us) / fresh;
        }
        out
    }

    /// One line for the log, from [`Self::health`].
    pub fn health_line(&self) -> String {
        let health = self.health();
        format!(
            "pipeline: {:.1} fps measured (of {} configured), frames={} dropped={} \
             repeated={} clock-overruns={} | encoder {} µs/frame this second, worst {} µs, \
             in-flight={} submit-waits={} ({} ms lost) | ring {:.1}s {:.1} MB",
            health.fps,
            self.fps,
            health.frames,
            health.dropped,
            health.duplicated,
            health.overruns,
            health.encode_us,
            health.encode_worst_us,
            health.in_flight,
            health.submit_waited,
            health.submit_wait_ms,
            health.ring_seconds,
            health.ring_bytes as f64 / 1_048_576.0,
        )
    }

    /// Record a problem with the running capture.
    pub fn report(&self, message: String) {
        log::warn!("capture: {message}");
        *self.error.lock() = Some(message);
    }

    /// Collect the first error not yet reported.
    pub fn take_unseen_error(&self) -> Option<String> {
        if self.error_seen.swap(true, Ordering::SeqCst) {
            return None;
        }
        self.error.lock().clone()
    }

    /// QPC time for a packet timestamp.
    pub fn qpc_of(&self, pts_us: i64) -> i64 {
        self.base_100ns.load(Ordering::Acquire) + pts_us * 10
    }
}

/// Everything needed to write a clip — grabbed in one go so the locks are not
/// held across the whole ffmpeg run.
pub struct ClipSnapshot {
    pub packets: Vec<EncodedPacket>,
    /// One per recorded source, in mixer order.
    pub tracks: Vec<Arc<TrackRing>>,
    /// Of those, the ones that have no track of their own and are summed into
    /// the main mix. Read at this moment rather than while recording — that is
    /// what lets the assignment still be changed for the minutes already in the
    /// buffer.
    pub main_mix: Vec<String>,
    pub sequence_header: Vec<u8>,
    /// QPC of the first frame in the clip — the audio is cut at exactly this
    /// value.
    pub start_100ns: i64,
    pub audio_frames: usize,
    pub fps: u32,
}

/// A running capture.
pub struct Pipeline {
    pub shared: Arc<Shared>,
    #[cfg(windows)]
    inner: Option<win::Running>,
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

    use crate::convert::{pace, Converter, FrameSink, Latest};
    use crate::gpu::GpuDevice;
    use crate::mft::{EncoderSettings, VideoEncoder};
    use crate::wgc::{self, Capture};

    pub struct Running {
        /// Has to drop before the encoder: no frame may arrive after that.
        _capture: Capture,
        latest: Arc<Latest>,
        pacer: Option<std::thread::JoinHandle<()>>,
        mixer_stop: Arc<AtomicBool>,
        mixer: Option<std::thread::JoinHandle<()>>,
        encoder: Option<Arc<VideoEncoder>>,
    }

    /// Passes the clocked frames on to the encoder.
    struct EncoderSink {
        encoder: Arc<VideoEncoder>,
        shared: Arc<Shared>,
        latest: Arc<Latest>,
    }

    impl FrameSink for EncoderSink {
        fn on_frame(&mut self, texture: &ID3D11Texture2D, pts_100ns: i64, duplicate: bool) {
            // On the very first frame, anchor the output timeline to the real
            // capture time. Without it nobody would know at save time which
            // stretch of audio belongs to which frame.
            if !self.shared.anchored.load(Ordering::Acquire) {
                let qpc = self.latest.frame_qpc();
                self.shared
                    .base_100ns
                    .store(qpc - pts_100ns, Ordering::Release);
                self.shared.anchored.store(true, Ordering::Release);
            }
            if self.encoder.submit(texture, pts_100ns) {
                self.shared.frames.fetch_add(1, Ordering::Relaxed);
            } else {
                self.shared.dropped.fetch_add(1, Ordering::Relaxed);
            }
            if duplicate {
                self.shared.duplicated.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn start(
        shared: Arc<Shared>,
        recording: &RecordingConfig,
    ) -> Result<Running, String> {
        // Settled once, before anything is built: the same answer chooses the
        // graphics adapter below and drives the MFT further down. Resolving it
        // twice is how the encoder and the device it runs on came apart.
        let wanted = crate::encode::for_recording(recording.encoder);

        let gpu = Arc::new(GpuDevice::new(Some(wanted))?);
        let converter = Converter::new(&gpu, recording.width, recording.height, recording.fps)?;
        let latest = Latest::new(converter);

        let encoder = {
            let shared = shared.clone();
            Arc::new(VideoEncoder::start(
                gpu.clone(),
                EncoderSettings {
                    width: recording.width,
                    height: recording.height,
                    fps: recording.fps,
                    bitrate_kbps: recording.bitrate_kbps,
                    quality: recording.quality,
                    keyframe_seconds: recording.keyframe_seconds,
                    requested: wanted,
                },
                move |packet| shared.packets.lock().push(packet),
            )?)
        };
        *shared.encoder.lock() = Some(encoder.chosen);
        *shared.rate_control.lock() = Some(encoder.rate_control);
        *shared.sequence_header.lock() = encoder.sequence_header.clone();
        *shared.encoder_stats.lock() = Some(encoder.stats.clone());
        *shared.pacer.lock() = Some(latest.clone());

        let capture = {
            let latest = latest.clone();
            let closed_shared = shared.clone();
            let shared = shared.clone();
            wgc::start(
                &gpu,
                recording.target_kind,
                recording.target_id.as_deref(),
                recording.fps,
                move |frame| {
                    if let Err(err) = latest.submit(frame.texture, frame.qpc_100ns) {
                        shared.dropped.fetch_add(1, Ordering::Relaxed);
                        shared.report(err);
                    }
                },
                {
                    let shared = closed_shared.clone();
                    move || {
                        shared.report(
                            "The capture source has gone away — no more frames are arriving."
                                .into(),
                        );
                    }
                },
            )?
        };

        let pacer = {
            let latest = latest.clone();
            let sink = Box::new(EncoderSink {
                encoder: encoder.clone(),
                shared: shared.clone(),
                latest: latest.clone(),
            });
            let fps = recording.fps;
            std::thread::Builder::new()
                .name("clippiboy-pacer".into())
                .spawn(move || pace(latest, fps, sink))
                .map_err(|err| format!("Taktgeber: {err}"))?
        };

        let mixer_stop = Arc::new(AtomicBool::new(false));
        let mixer = {
            let shared = shared.clone();
            let stop = mixer_stop.clone();
            std::thread::Builder::new()
                .name("clippiboy-mixer".into())
                .spawn(move || super::mix_loop(shared, stop))
                .map_err(|err| format!("Mischer: {err}"))?
        };

        Ok(Running {
            _capture: capture,
            latest,
            pacer: Some(pacer),
            mixer_stop,
            mixer: Some(mixer),
            encoder: Some(encoder),
        })
    }

    pub fn stop(mut running: Running) {
        // Order: stop clocking new frames first, then let the encoder drain. The
        // other way round the clock would run into nothing.
        running.latest.stop();
        if let Some(pacer) = running.pacer.take() {
            let _ = pacer.join();
        }
        running.mixer_stop.store(true, Ordering::Relaxed);
        if let Some(mixer) = running.mixer.take() {
            let _ = mixer.join();
        }
        if let Some(encoder) = running.encoder.take() {
            // The sink holds the second reference; that dropped with the clock,
            // here the last one drops and the encoder drains.
            drop(encoder);
        }
    }
}

/// The mixer: produces gapless consecutive audio windows on the QPC axis.
fn mix_loop(shared: Arc<Shared>, stop: Arc<AtomicBool>) {
    let mut sources = shared.sources.lock().clone();
    let mut generation = shared.sources_generation.load(Ordering::Relaxed);

    let mut buffers: Vec<Vec<f32>> = Vec::new();
    let mut scratch: Vec<i16> = Vec::new();
    // Computed against the clock, not accumulated: that way no error can build
    // up, however imprecisely the thread wakes.
    let mut next_100ns: Option<i64> = None;

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(MIX_INTERVAL);

        let current = shared.sources_generation.load(Ordering::Relaxed);
        if current != generation {
            generation = current;
            sources = shared.sources.lock().clone();
            sync_tracks(&shared, &sources);
        }

        let target = now_100ns() - AUDIO_LAG_100NS;
        let from = *next_100ns.get_or_insert(target);
        if target <= from {
            continue;
        }
        let frames = ((target - from) * SAMPLE_RATE as i64 / 10_000_000) as usize;
        if frames == 0 {
            continue;
        }

        shared.audio.mix_window(&sources, from, frames, &mut buffers);

        // Every ring gets something in every window — whatever is audible right
        // now gets its mix, whatever is muted or soloed out gets silence. Pushing
        // nothing would be wrong: the ring only stamps its very first block, so a
        // gap would pull everything after it forward and from there on the audio
        // would run ahead of the picture.
        //
        // Matched by id rather than by position: the two lists are built from
        // the same sources and agree, but a mismatch would put one source's
        // audio onto another's track, and that is not worth risking.
        let rings = shared.tracks.lock();
        for ring in rings.iter() {
            let slot = recorded(&sources).position(|s| s.id == ring.source_id);
            scratch.clear();
            match slot.and_then(|index| buffers.get(index)) {
                Some(track) => scratch.extend(track.iter().map(|s| to_i16(*s))),
                None => scratch.resize(frames * CHANNELS, 0),
            }
            ring.push(&scratch, from);
        }
        drop(rings);

        next_100ns = Some(from + frames as i64 * 10_000_000 / SAMPLE_RATE as i64);
    }
}

/// The ring list for a source list — one per recorded source, no main mix among
/// them. What becomes a track of its own in the clip and what is summed into the
/// main mix is decided on save (`muxer::build`).
///
/// Rings that already exist are **passed through**. Creating them anew would be
/// the convenient route but costs all of that source's buffered audio — and
/// every change to the sources comes past here, including every single step of a
/// volume slider. Previously that meant: turn the microphone up during recording
/// and there is no microphone left in the clip.
///
/// Deliberately **not** filtered by `muted`/`solo` — nor by `separate_track`.
/// A muted source keeps its ring and the mixer pushes silence into it; a source
/// that runs into the main mix keeps one so it can be lifted back out of it
/// afterwards. Otherwise a ⧉ would only ever apply from the moment it was
/// flipped, and flipping it back would cost the minutes recorded so far.
fn tracks_for(
    sources: &[AudioSource],
    existing: &[Arc<TrackRing>],
    buffer_seconds: u32,
) -> Vec<Arc<TrackRing>> {
    let mut out = Vec::new();
    for source in recorded(sources) {
        match existing.iter().find(|ring| ring.source_id == source.id) {
            Some(ring) => {
                ring.set_label(source.label.clone());
                out.push(ring.clone());
            }
            None => out.push(Arc::new(TrackRing::new(
                source.id.clone(),
                source.label.clone(),
                buffer_seconds,
            ))),
        }
    }
    out
}

/// Bring the track list in line with a changed config.
fn sync_tracks(shared: &Arc<Shared>, sources: &[AudioSource]) {
    let mut rings = shared.tracks.lock();
    let next = tracks_for(sources, &rings, shared.buffer_seconds);
    *rings = next;
}

fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0) as i16
}

impl Pipeline {
    pub fn start(
        recording: &RecordingConfig,
        buffer_seconds: u32,
        buffer_bytes: u64,
        sources: Vec<AudioSource>,
        audio: Arc<AudioEngine>,
    ) -> Result<Self, String> {
        let tracks = tracks_for(&sources, &[], buffer_seconds);

        // Without this, every ring still holds the last few minutes of audio that
        // nobody collected.
        audio.reset_rings();

        let shared = Arc::new(Shared {
            buffer_seconds,
            fps: recording.fps.max(1),
            packets: Mutex::new(ReplayBuffer::new(buffer_seconds, buffer_bytes)),
            tracks: Mutex::new(tracks),
            sources: Mutex::new(sources),
            audio,
            base_100ns: AtomicI64::new(0),
            anchored: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            duplicated: AtomicU64::new(0),
            error: Mutex::new(None),
            error_seen: AtomicBool::new(false),
            sources_generation: AtomicU64::new(0),
            encoder: Mutex::new(None),
            rate_control: Mutex::new(None),
            sequence_header: Mutex::new(Vec::new()),
            #[cfg(windows)]
            encoder_stats: Mutex::new(None),
            #[cfg(windows)]
            pacer: Mutex::new(None),
            health_mark: Mutex::new(HealthMark {
                at: std::time::Instant::now(),
                frames: 0,
                encoded: 0,
                encode_us: 0,
            }),
        });

        #[cfg(windows)]
        {
            let inner = win::start(shared.clone(), recording)?;
            Ok(Self {
                shared,
                inner: Some(inner),
            })
        }

        #[cfg(not(windows))]
        {
            let _ = recording;
            let _ = shared;
            Err("recording is only available on Windows".into())
        }
    }

    pub fn stop(&mut self) {
        #[cfg(windows)]
        if let Some(inner) = self.inner.take() {
            win::stop(inner);
        }
        self.shared.packets.lock().clear();
    }

    /// Grab the last `seconds` seconds.
    ///
    /// The cut sits on the last keyframe **before** the wanted start time —
    /// otherwise the beginning would not be decodable. The audio is cut at exactly
    /// the same QPC; so no correction is needed any more for picture and sound to
    /// line up.
    pub fn snapshot(&self, seconds: u32) -> Result<ClipSnapshot, String> {
        let packets = self.shared.packets.lock().snapshot(seconds);
        if packets.is_empty() {
            return Err("The replay buffer is still empty.".into());
        }

        let first_pts = packets.first().map(|p| p.pts_us).unwrap_or(0);
        let last = packets.last().map(|p| p.pts_us).unwrap_or(first_pts);
        let frame_us = 1_000_000 / self.shared.fps as i64;
        let span_us = (last - first_pts + frame_us).max(0);

        // The assignment as it stands **now**, not as it stood while recording:
        // every source has a ring of its own, so a ⧉ flipped a minute ago still
        // reaches back over the whole buffer.
        let main_mix = self
            .shared
            .sources
            .lock()
            .iter()
            .filter(|s| s.enabled && !s.separate_track)
            .map(|s| s.id.clone())
            .collect();

        Ok(ClipSnapshot {
            tracks: self.shared.tracks.lock().clone(),
            main_mix,
            sequence_header: self.shared.sequence_header.lock().clone(),
            start_100ns: self.shared.qpc_of(first_pts),
            audio_frames: (span_us * SAMPLE_RATE as i64 / 1_000_000) as usize,
            fps: self.shared.fps,
            packets,
        })
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// QPC (100 ns) for a frame position.
    ///
    /// At 48 kHz only multiples of 6 frames are exactly representable in 100 ns
    /// (6 frames = 1250 ticks). The tests below therefore work in such steps —
    /// otherwise they would be checking the rounding rather than the window
    /// logic.
    const STEP: i64 = 6;

    fn qpc_of_frame(frame: i64) -> i64 {
        assert_eq!(frame % STEP, 0, "only test exactly representable distances");
        frame * 10_000_000 / SAMPLE_RATE as i64
    }

    #[test]
    fn a_track_window_lands_where_its_timestamp_says() {
        let ring = TrackRing::new("a".into(), "A".into(), 5);
        // Six stereo frames, ascending, so the position is recognizable.
        let block: Vec<i16> = (1..=(STEP as i16 * CHANNELS as i16)).collect();
        ring.push(&block, qpc_of_frame(0));
        ring.push(&[77, 88], qpc_of_frame(STEP));

        let dir = std::env::temp_dir().join("clippiboy-test-track");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("window.wav");
        ring.write_wav_window(&path, qpc_of_frame(STEP), 1).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            &bytes[44..],
            &77i16.to_le_bytes()[..].iter().chain(88i16.to_le_bytes().iter())
                .copied().collect::<Vec<u8>>()[..],
            "the window sits in the wrong place"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// A window before the first audio has to deliver silence, not the beginning
    /// — otherwise the audio would slide forward on save.
    #[test]
    fn a_window_before_the_audio_is_silence() {
        let ring = TrackRing::new("a".into(), "A".into(), 5);
        ring.push(&[7, 7], qpc_of_frame(STEP * 20));

        let dir = std::env::temp_dir().join("clippiboy-test-track");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("before.wav");
        ring.write_wav_window(&path, qpc_of_frame(0), 1).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[44..], &[0, 0, 0, 0]);
        let _ = std::fs::remove_file(&path);
    }

    /// The main mix is a sum drawn at save time, not a track that was recorded.
    /// This is what makes a ⧉ flipped mid-recording reach back over the whole
    /// buffer — the sources are still lying there one by one.
    #[test]
    fn the_main_mix_is_summed_from_the_tracks() {
        let dir = std::env::temp_dir().join("clippiboy-test-mix");
        let _ = std::fs::create_dir_all(&dir);

        let game = TrackRing::new("game".into(), "Game".into(), 5);
        game.push(&[100, -100], qpc_of_frame(0));
        let mic = TrackRing::new("mic".into(), "Microphone".into(), 5);
        mic.push(&[25, 25], qpc_of_frame(0));

        let path = dir.join("mix.wav");
        write_mix_wav(&[&game, &mic], &path, qpc_of_frame(0), 1).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let sample = |at: usize| i16::from_le_bytes([bytes[44 + at * 2], bytes[45 + at * 2]]);
        assert_eq!((sample(0), sample(1)), (125, -75));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Summing must not wrap around: two loud tracks together are a clipped peak
    /// at worst, never a crack from the sign flipping.
    #[test]
    fn the_sum_saturates_instead_of_wrapping() {
        let dir = std::env::temp_dir().join("clippiboy-test-mix-loud");
        let _ = std::fs::create_dir_all(&dir);

        let one = TrackRing::new("a".into(), "A".into(), 5);
        one.push(&[30000, -30000], qpc_of_frame(0));
        let two = TrackRing::new("b".into(), "B".into(), 5);
        two.push(&[30000, -30000], qpc_of_frame(0));

        let path = dir.join("loud.wav");
        write_mix_wav(&[&one, &two], &path, qpc_of_frame(0), 1).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let sample = |at: usize| i16::from_le_bytes([bytes[44 + at * 2], bytes[45 + at * 2]]);
        assert_eq!((sample(0), sample(1)), (i16::MAX, i16::MIN));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_oldest_audio_is_dropped_and_the_start_moves_with_it() {
        // 1 s capacity + 2 s slack = 3 s.
        let ring = TrackRing::new("a".into(), "A".into(), 1);
        // Not zeros: silence is not stored at all, and the ring would stay empty.
        let block = vec![7i16; SAMPLE_RATE as usize * CHANNELS];
        for second in 0..5 {
            ring.push(&block, qpc_of_frame(second * SAMPLE_RATE as i64));
        }
        // 48000 is divisible by 6 — so the one-second steps are exact.
        let inner = ring.inner.lock();
        assert_eq!(inner.samples.len(), ring.capacity);
        assert!(
            inner.start_100ns > 0,
            "the ring start has to move along, otherwise every window points beside"
        );
    }

    /// Silence is the normal state of most tracks — a device nobody plays on,
    /// a microphone on mute, a chat channel between two sentences. Storing it
    /// would be minutes of zeros per source.
    #[test]
    fn silence_costs_the_ring_nothing() {
        let ring = TrackRing::new("a".into(), "A".into(), 1);
        let quiet = vec![0i16; SAMPLE_RATE as usize * CHANNELS];
        for second in 0..3 {
            ring.push(&quiet, qpc_of_frame(second * SAMPLE_RATE as i64));
        }
        assert!(
            ring.inner.lock().samples.is_empty(),
            "silence must not take any room"
        );
    }

    /// The whole point of counting it: what comes back has to be the very same
    /// stream the ring would hold if it had stored every quiet block. One sample
    /// too few and everything after the pause runs ahead of the picture.
    #[test]
    fn counted_silence_comes_back_as_the_zeros_it_stood_for() {
        let block = STEP as usize * CHANNELS;
        let ring = TrackRing::new("a".into(), "A".into(), 5);
        // Six frames of sound, six of nothing, then sound again.
        ring.push(&vec![11i16; block], qpc_of_frame(0));
        ring.push(&vec![0i16; block], qpc_of_frame(STEP));
        ring.push(&[33, 44], qpc_of_frame(STEP * 2));

        {
            let inner = ring.inner.lock();
            assert_eq!(inner.samples.len(), block + block + 2);
            assert_eq!(inner.samples[0], 11);
            assert_eq!(inner.samples[block], 0, "the pause has to be there");
            assert_eq!(inner.samples[block * 2], 33);
            assert_eq!(inner.silence, 0);
        }

        // And it reads back at the place on the timeline it was pushed at — one
        // sample too few and everything after the pause runs ahead of the
        // picture.
        let dir = std::env::temp_dir().join("clippiboy-test-gap");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("gap.wav");
        ring.write_wav_window(&path, qpc_of_frame(STEP * 2), 1).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            &bytes[44..],
            &[33i16.to_le_bytes(), 44i16.to_le_bytes()].concat()[..]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Quiet for longer than the ring is long: everything in it is older than
    /// the buffer, so it goes and the memory with it.
    #[test]
    fn a_track_quiet_for_a_whole_buffer_gives_its_memory_back() {
        // 1 s capacity + 2 s slack = 3 s.
        let ring = TrackRing::new("a".into(), "A".into(), 1);
        ring.push(&[7, 7], qpc_of_frame(0));
        assert!(!ring.inner.lock().samples.is_empty());

        let quiet = vec![0i16; SAMPLE_RATE as usize * CHANNELS];
        for second in 1..=4 {
            ring.push(&quiet, qpc_of_frame(second * SAMPLE_RATE as i64));
        }

        let inner = ring.inner.lock();
        assert!(inner.samples.is_empty(), "stale audio was kept");
        assert!(!inner.primed, "the timeline has to start again with the sound");
        assert_eq!(inner.silence, 0);
    }

    fn source(id: &str, label: &str, separate: bool) -> AudioSource {
        AudioSource {
            id: id.into(),
            label: label.into(),
            kind: crate::model::SourceKind::InputDevice {
                device_id: "d".into(),
            },
            enabled: true,
            gain_db: 0.0,
            muted: false,
            solo: false,
            separate_track: separate,
        }
    }

    /// The actual bug behind "my microphone is silent in the clip": every slider
    /// movement during recording came past here and recreated the individual
    /// tracks — losing all of the buffered audio along with them.
    #[test]
    fn a_changed_source_keeps_its_ring_and_its_audio() {
        let mut sources = vec![source("mic", "Microphone", true)];
        let first = tracks_for(&sources, &[], 5);
        first[0].push(&[123, 456], qpc_of_frame(0));

        // Turned up and renamed — neither may cost it its track.
        sources[0].gain_db = 6.0;
        sources[0].label = "My mic".into();
        let second = tracks_for(&sources, &first, 5);

        assert!(Arc::ptr_eq(&first[0], &second[0]), "microphone track was recreated");
        assert_eq!(second[0].label(), "My mic", "name was not carried over");
        assert_eq!(second[0].inner.lock().samples.len(), 2, "Ton verloren");
    }

    /// Muting means "silence from here on", not "the track never existed".
    #[test]
    fn a_muted_source_keeps_its_track() {
        let mut sources = vec![source("mic", "Microphone", true)];
        let first = tracks_for(&sources, &[], 5);
        sources[0].muted = true;
        let second = tracks_for(&sources, &first, 5);

        assert_eq!(second.len(), 1);
        assert!(Arc::ptr_eq(&first[0], &second[0]));
    }

    /// The whole point of the rebuild: a source that runs into the main mix is
    /// recorded on a ring of its own just the same, and switching ⧉ keeps that
    /// ring. Only then does the switch reach the minutes already buffered.
    #[test]
    fn a_source_in_the_main_mix_has_a_ring_of_its_own() {
        let mut sources = vec![
            source("game", "Game", false),
            source("mic", "Microphone", true),
        ];
        let first = tracks_for(&sources, &[], 5);
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].source_id, "game");
        assert_eq!(first[1].source_id, "mic");
        first[0].push(&[123, 456], qpc_of_frame(0));

        sources[0].separate_track = true;
        let second = tracks_for(&sources, &first, 5);
        assert!(
            Arc::ptr_eq(&first[0], &second[0]),
            "the ring must survive the switch, or the buffered minutes are gone"
        );
        assert_eq!(second[0].inner.lock().samples.len(), 2);
    }

    /// A removed source disappears, a new one joins empty.
    #[test]
    fn removed_sources_drop_out_and_new_ones_start_empty() {
        let first = tracks_for(&[source("mic", "Microphone", true)], &[], 5);
        let second = tracks_for(&[source("discord", "Discord", true)], &first, 5);

        assert_eq!(second.len(), 1);
        assert_eq!(second[0].source_id, "discord");
        assert!(!second[0].inner.lock().primed);
    }

    /// A disabled source is not recorded at all — it has no streams either.
    #[test]
    fn a_disabled_source_gets_no_ring() {
        let mut sources = vec![source("mic", "Microphone", true)];
        sources[0].enabled = false;
        assert!(tracks_for(&sources, &[], 5).is_empty());
    }
}
