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
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use crate::audio::capture::{now_100ns, CHANNELS, SAMPLE_RATE};
use crate::audio::engine::AudioEngine;
use crate::audio::recorded;
use crate::buffer::{EncodedPacket, ReplayBuffer};
use crate::model::{AudioSource, EncoderId, RateControl, RecordingConfig};
use crate::recorder::{self, Recorder};

/// How far behind the present the mixer stays.
///
/// WASAPI only delivers a block once it is full; a few milliseconds pass before
/// it is in the ring. Mixing closer to the present fetches silence that would
/// later have had to be replaced by real audio — and the slot is already taken
/// by then.
const AUDIO_LAG_100NS: i64 = 80 * 10_000;

/// The mixer's tick.
const MIX_INTERVAL: Duration = Duration::from_millis(10);

/// What an audio ring keeps beyond the buffer and one keyframe interval.
///
/// A save reads its audio only after the whole `.h264` is written, and the
/// rings keep moving meanwhile. Ten seconds cover a slow write — an HDD, a
/// network folder — at ~2 MB per source, where copying every window out under
/// the lock would cost a buffer's worth of memory per track and save.
pub(crate) const AUDIO_SLACK_SECONDS: u32 = 10;

/// How long an audio ring is for a buffer of `buffer_seconds`.
///
/// A clip as long as the buffer starts on the keyframe before
/// `newest − buffer_seconds`, up to one keyframe interval earlier; a ring of
/// only the buffer's length had already let that start go, and the clip began
/// mute (BUF-1). The interval comes from the config unchecked: it counts as 1
/// to 10 seconds (the settings offer 1, 2 and 4), and the sum saturates.
pub(crate) fn ring_seconds(buffer_seconds: u32, keyframe_seconds: u32) -> u32 {
    buffer_seconds
        .saturating_add(keyframe_seconds.clamp(1, 10))
        .saturating_add(AUDIO_SLACK_SECONDS)
}

/// The longest hole a clip's MPEG-TS may carry. ffmpeg's CLI takes a break in
/// a TS's timestamps above its `-dts_delta_threshold` (10 s by default) for a
/// discontinuity and closes it up: the freeze comes out shorter than its sound
/// and the picture after it runs ahead. 8 s stays clear of that. A longer hole
/// cannot reach a clip at all: the game ring cuts it like a sleep
/// ([`game_ring`]). The raw path spreads frames evenly and stretches only
/// across short stalls (`muxer::timing`), so across a hole that long its
/// picture would run ahead of its sound by the whole hole.
const TS_HOLE_LIMIT_US: i64 = 8_000_000;

/// The longest step between frames a clip still keeps its own timestamps
/// across: the ring's jump limit ([`crate::buffer::jump_limit_us`]), but never
/// more than ffmpeg leaves alone in a TS ([`TS_HOLE_LIMIT_US`]).
fn ts_jump_limit_us(keyframe_seconds: u32) -> i64 {
    crate::buffer::jump_limit_us(keyframe_seconds).min(TS_HOLE_LIMIT_US)
}

/// The game's packet ring. A step longer than a clip keeps as a held frame is
/// a break in time: the ring drops what lies before it (BUF-5). That limit is
/// the clip's own ([`ts_jump_limit_us`]), so every hole the ring keeps goes
/// out with its timestamps and the sound stays with the picture across it.
fn game_ring(buffer_seconds: u32, buffer_bytes: u64, keyframe_seconds: u32) -> ReplayBuffer {
    let mut packets = ReplayBuffer::new(buffer_seconds, buffer_bytes);
    packets.set_jump_limit_us(ts_jump_limit_us(keyframe_seconds));
    packets
}

/// How much audio a clip gets, in 48 kHz frames: as long as its picture lasts.
/// With its own timestamps that is the span they cover (`span`, falling back
/// to `timing`'s); spread evenly, it is the span `muxer::timing` settled on.
fn clip_audio_frames(timestamped: bool, span: Option<u64>, timing: &crate::muxer::Timing) -> usize {
    let span_us = if timestamped {
        span.unwrap_or(timing.span_us)
    } else {
        timing.span_us
    };
    let frames = span_us as i64 * SAMPLE_RATE as i64 / 1_000_000;
    frames.max(0) as usize
}

/// Exactly `seconds` of audio, in samples. The slack is [`ring_seconds`]'s.
fn samples_for(seconds: u32) -> usize {
    seconds as usize * SAMPLE_RATE as usize * CHANNELS
}

/// `frames` of 48 kHz audio in 100 ns ticks, rounded down once for the whole
/// count. In `i128`, so no count of frames can overflow on the way.
fn frames_to_100ns(frames: u64) -> i64 {
    (frames as i128 * 10_000_000 / SAMPLE_RATE as i128) as i64
}

/// How long a save waits at most for the mixer to reach the clip's end: the
/// 80 ms it stays behind ([`AUDIO_LAG_100NS`]), one tick and some scheduling.
pub const MIX_WAIT: Duration = Duration::from_millis(150);

/// How far the mixer has got — the QPC its next window starts at.
///
/// A save cuts its audio to end where the video ends, and the mixer runs 80 ms
/// behind the present. Read straight away, the last few dozen milliseconds of
/// every clip were the zeros of a window not mixed yet (BUF-1).
pub struct MixPosition {
    /// `i64::MIN` until the first window is mixed.
    until_100ns: AtomicI64,
}

impl Default for MixPosition {
    fn default() -> Self {
        Self {
            until_100ns: AtomicI64::new(i64::MIN),
        }
    }
}

impl MixPosition {
    /// Everything before `at_100ns` is in the rings.
    pub fn publish(&self, at_100ns: i64) {
        self.until_100ns.store(at_100ns, Ordering::Release);
    }

    /// Wait until the mixer has passed `at_100ns`, for `timeout` at most.
    /// False when it did not get there — a stalled or stopped mixer must not
    /// hold up a save; its tail stays silent instead.
    pub fn wait_until(&self, at_100ns: i64, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if self.until_100ns.load(Ordering::Acquire) >= at_100ns {
                return true;
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return false;
            }
            std::thread::sleep((deadline - now).min(Duration::from_millis(5)));
        }
    }
}

/// Ring buffer of one audio track (16-bit PCM), on the same QPC timeline as the
/// video.
pub struct TrackRing {
    pub source_id: String,
    /// Changeable: renaming a source must not cost it its track — the ring stays
    /// the same, only the label changes.
    label: Mutex<String>,
    inner: Mutex<TrackInner>,
    /// In samples. Changeable while the buffer runs: the buffer length is a
    /// slider, and a ring that kept its old length would cut the audio of a
    /// longer clip short.
    capacity: AtomicUsize,
}

struct TrackInner {
    samples: VecDeque<i16>,
    /// QPC of the first sample the ring was primed with. The start of what it
    /// holds now is counted from here in frames ([`Self::start_100ns`]), not
    /// moved along with every trim: rounding each trim to a tick would lose
    /// a third of a tick each time, and the mixer's clock no longer does.
    origin_100ns: i64,
    /// Frames trimmed off the front since the ring was primed.
    dropped_frames: u64,
    primed: bool,
    /// Silence that has come in but is not stored — see [`TrackRing::push`].
    /// Counted in samples and never larger than the ring itself.
    silence: usize,
}

impl TrackInner {
    /// QPC of the first sample in the ring.
    fn start_100ns(&self) -> i64 {
        self.origin_100ns + frames_to_100ns(self.dropped_frames)
    }
}

impl TrackRing {
    pub(crate) fn new(source_id: String, label: String, seconds: u32) -> Self {
        Self {
            source_id,
            label: Mutex::new(label),
            inner: Mutex::new(TrackInner {
                samples: VecDeque::new(),
                origin_100ns: 0,
                dropped_frames: 0,
                primed: false,
                silence: 0,
            }),
            capacity: AtomicUsize::new(samples_for(seconds)),
        }
    }

    fn capacity(&self) -> usize {
        self.capacity.load(Ordering::Relaxed)
    }

    /// A new buffer length. Longer fills up from here on; shorter is cut down
    /// with the next block.
    pub(crate) fn set_seconds(&self, seconds: u32) {
        self.capacity.store(samples_for(seconds), Ordering::Relaxed);
    }

    pub fn label(&self) -> String {
        self.label.lock().clone()
    }

    pub(crate) fn set_label(&self, label: String) {
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
    pub(crate) fn push(&self, block: &[i16], at_100ns: i64) {
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
            if inner.silence >= self.capacity() {
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
            inner.origin_100ns = at_100ns;
            inner.dropped_frames = 0;
            inner.primed = true;
        } else {
            inner.samples.extend(std::iter::repeat(0).take(gap));
        }

        inner.samples.extend(block.iter().copied());
        let capacity = self.capacity();
        if inner.samples.len() > capacity {
            let excess = inner.samples.len() - capacity;
            inner.samples.drain(..excess);
            inner.dropped_frames += (excess / CHANNELS) as u64;
        }
    }

    /// Count `samples` of silence without being handed them.
    ///
    /// For a gap the mixer will not fill in — after the PC slept, the next
    /// window would span hours, and every ring a block of zeros that long. The
    /// count is what [`Self::push`] keeps for a quiet block anyway, so the
    /// timeline comes out exactly as if the zeros had been pushed.
    pub(crate) fn skip(&self, samples: usize) {
        let mut inner = self.inner.lock();
        if !inner.primed {
            return;
        }
        inner.silence = inner.silence.saturating_add(samples);
        if inner.silence >= self.capacity() {
            inner.samples.clear();
            inner.samples.shrink_to_fit();
            inner.primed = false;
            inner.silence = 0;
        }
    }

    /// The window starting at `from_100ns` over `frames` frames, as interleaved
    /// PCM.
    ///
    /// Missing stretches become silence — that keeps the track exactly as long as
    /// the video, even when a source only joined later.
    pub(crate) fn window(&self, from_100ns: i64, frames: usize) -> Vec<i16> {
        let wanted = frames * CHANNELS;
        let mut data = vec![0i16; wanted];
        {
            let inner = self.inner.lock();
            if inner.primed {
                let offset_frames =
                    (from_100ns - inner.start_100ns()) * SAMPLE_RATE as i64 / 10_000_000;
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
    use std::io::Write;
    let byte_len = (data.len() * 2) as u32;
    // Through a buffered file rather than a copy of the whole window in memory.
    let mut out = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(path)?);
    out.write_all(b"RIFF")?;
    out.write_all(&(36 + byte_len).to_le_bytes())?;
    out.write_all(b"WAVEfmt ")?;
    out.write_all(&16u32.to_le_bytes())?;
    out.write_all(&1u16.to_le_bytes())?; // PCM
    out.write_all(&(CHANNELS as u16).to_le_bytes())?;
    out.write_all(&SAMPLE_RATE.to_le_bytes())?;
    out.write_all(&(SAMPLE_RATE * CHANNELS as u32 * 2).to_le_bytes())?;
    out.write_all(&((CHANNELS * 2) as u16).to_le_bytes())?;
    out.write_all(&16u16.to_le_bytes())?;
    out.write_all(b"data")?;
    out.write_all(&byte_len.to_le_bytes())?;
    for sample in data {
        out.write_all(&sample.to_le_bytes())?;
    }
    out.flush()
}

/// The last error of one run and whether the UI has heard of it. An encoder
/// that keeps stumbling should not flood the UI — but *once* somebody has to
/// hear about it, otherwise the app appears to keep buffering and only the key
/// press brings it to light.
pub struct ErrorSlot {
    error: Mutex<Option<String>>,
    seen: AtomicBool,
    reports: AtomicU64,
}

impl ErrorSlot {
    pub fn new() -> Self {
        Self { error: Mutex::new(None), seen: AtomicBool::new(false), reports: AtomicU64::new(0) }
    }

    /// Record a problem. A failure that repeats per frame is logged at warn
    /// on the first report and every 500th, at debug in between.
    pub fn report(&self, scope: &str, message: String) {
        let n = self.reports.fetch_add(1, Ordering::Relaxed);
        if n % 500 == 0 {
            log::warn!("{scope}: {message}");
        } else {
            log::debug!("{scope}: {message}");
        }
        *self.error.lock() = Some(message);
    }

    /// The error not yet handed out. The flag is only taken when there is an
    /// error, so a poll that finds none does not use up the run's one report.
    pub fn take_unseen(&self) -> Option<String> {
        let error = self.error.lock().clone()?;
        if self.seen.swap(true, Ordering::SeqCst) {
            return None;
        }
        Some(error)
    }
}

impl Default for ErrorSlot {
    fn default() -> Self {
        Self::new()
    }
}

/// State shared by capture, encoder and mixer.
pub struct Shared {
    /// What a newly added source's ring is sized to — see [`Shared::set_buffer_seconds`].
    buffer_seconds: AtomicU32,
    /// The encoder's keyframe interval. A clip can start this much before the
    /// buffer's length, so the audio rings keep it as well ([`ring_seconds`]).
    keyframe_seconds: u32,
    /// How far the mixer has got. A save waits for it to pass the clip's end.
    pub mixed: Arc<MixPosition>,
    pub fps: u32,
    /// Width and height the encoder runs at — the size of every clip cut
    /// from this ring.
    pub size: (u32, u32),
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
    /// The run's capture errors, handed to the UI once.
    pub errors: ErrorSlot,
    /// The screen this capture runs on, and how the settings arrived at it —
    /// `Fallback` when the chosen one was missing and the primary stands in.
    /// `None` for a window, and until capture is up.
    pub source: Mutex<Option<crate::capture::Choice>>,
    /// Windows has ended the capture: the screen went away, or the desktop was
    /// rebuilt under it. No frame arrives after this.
    pub source_closed: AtomicBool,
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
    /// A recording started by hand, if one runs. It takes every packet and
    /// every audio block alongside the buffer — see `recorder.rs`.
    ///
    /// Locked only while `packets` is held, on both sides: that is what lets a
    /// recording begin exactly at the buffer's newest keyframe with no packet
    /// missed or written twice.
    pub recorder: Mutex<Option<Arc<Recorder>>>,
    /// What the encoder is costing, once one is running.
    #[cfg(windows)]
    pub encoder_stats: Mutex<Option<Arc<crate::mft::EncoderStats>>>,
    /// The clock, so its overrun count can be read from outside.
    #[cfg(windows)]
    pub pacer: Mutex<Option<Arc<crate::convert::Latest>>>,
    /// What [`Self::health`] saw last time. Every number here is a *rate*, and
    /// a rate needs two readings.
    health_mark: Mutex<HealthMark>,
    /// The same, for the number the display shows.
    live_mark: Mutex<LiveMark>,
}

#[derive(Clone, Copy)]
struct HealthMark {
    at: std::time::Instant,
    frames: u64,
    encoded: u64,
    encode_us: u64,
}

/// What [`Shared::live_fps`] saw last time. Its own mark, not `health_mark`:
/// an interval can only be consumed once, and the log and the display ask on
/// different timers.
#[derive(Clone, Copy)]
struct LiveMark {
    at: std::time::Instant,
    frames: u64,
    duplicated: u64,
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

    /// What an audio ring is sized to now — see [`ring_seconds`].
    pub fn ring_seconds(&self) -> u32 {
        ring_seconds(self.buffer_seconds.load(Ordering::Relaxed), self.keyframe_seconds)
    }

    /// Resize every audio ring to a new buffer length, and the ones added later
    /// with them. The video ring is resized by the caller: its length also
    /// depends on whether the buffer is wanted at all (`AppState`).
    pub fn set_buffer_seconds(&self, seconds: u32) {
        self.buffer_seconds.store(seconds, Ordering::Relaxed);
        let ring_seconds = self.ring_seconds();
        for ring in self.tracks.lock().iter() {
            ring.set_seconds(ring_seconds);
        }
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

    /// New pictures per second, over the interval since the last call.
    ///
    /// Not the configured rate, and not the encoder's throughput: how often the
    /// screen actually changed. Windows.Graphics.Capture only hands a frame over
    /// when something moved, and the pipeline repeats the last one to keep the
    /// encoder on its clock — so on a still desktop most frames are repeats and
    /// this number is small, while `dropped` stays at zero. In a game it sits at
    /// the configured rate.
    ///
    /// This replaces a cumulative share of the whole session, which was the
    /// wrong shape for something read live: sit on the desktop for twenty
    /// minutes and the stillness stayed in the average long after a game had
    /// started. Call it on a timer; it consumes the interval it measures.
    pub fn live_fps(&self) -> f32 {
        let now = std::time::Instant::now();
        let frames = self.frames.load(Ordering::Relaxed);
        let duplicated = self.duplicated.load(Ordering::Relaxed);
        let before = {
            let mut mark = self.live_mark.lock();
            let previous = *mark;
            *mark = LiveMark {
                at: now,
                frames,
                duplicated,
            };
            previous
        };
        let seconds = now.duration_since(before.at).as_secs_f32();
        // A reading taken right after the previous one says nothing: two
        // callers a millisecond apart would turn one frame into a thousand.
        if seconds < 0.05 {
            return 0.0;
        }
        let fresh = frames.saturating_sub(before.frames);
        let repeats = duplicated.saturating_sub(before.duplicated);
        fresh.saturating_sub(repeats) as f32 / seconds
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
        self.errors.report("capture", message);
    }

    /// Why the encoder stopped working, if it has — see `EncoderStats::fail`.
    pub fn encoder_failure(&self) -> Option<String> {
        #[cfg(windows)]
        {
            self.encoder_stats.lock().as_ref().and_then(|stats| stats.failure())
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// Collect the first error not yet reported.
    pub fn take_unseen_error(&self) -> Option<String> {
        self.errors.take_unseen()
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
    /// For ffmpeg's `-r` — see `muxer::timing`. Used only when the snapshot
    /// is not [`Self::timestamped`].
    pub rate: String,
    /// The frames go to ffmpeg with their own timestamps
    /// (`muxer::keeps_timestamps`) instead of spread at `rate`.
    pub timestamped: bool,
    /// The longest step between neighbouring frames, which told a short
    /// encoder from a gap for `rate` (`muxer::largest_step_us`).
    pub largest_step_us: i64,
    /// How far the mixer has got — the rings are read only once it passed
    /// [`Self::end_100ns`].
    pub mixed: Arc<MixPosition>,
    /// The picture's size, from the pipeline the packets came out of.
    pub width: u32,
    pub height: u32,
}

impl ClipSnapshot {
    /// QPC where the clip's audio ends.
    pub fn end_100ns(&self) -> i64 {
        self.start_100ns + self.audio_frames as i64 * 10_000_000 / SAMPLE_RATE as i64
    }
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
    }

    impl FrameSink for EncoderSink {
        fn on_frame(
            &mut self,
            texture: &ID3D11Texture2D,
            pts_100ns: i64,
            duplicate: bool,
            qpc_100ns: i64,
        ) {
            // On the very first frame, anchor the output timeline to the real
            // capture time. Without it nobody would know at save time which
            // stretch of audio belongs to which frame.
            if !self.shared.anchored.load(Ordering::Acquire) {
                self.shared
                    .base_100ns
                    .store(qpc_100ns - pts_100ns, Ordering::Release);
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
        // Read once: the slots are sized from the same depth the encoder's job
        // channel gets.
        let queue_depth = crate::mft::queue_depth();
        let converter = Converter::new(
            &gpu,
            recording.width,
            recording.height,
            recording.fps,
            crate::convert::slots_for(queue_depth),
        )?;
        let latest = Latest::new(converter);

        let encoder = {
            let for_packets = shared.clone();
            let for_header = shared.clone();
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
                    label: "game",
                    queue_depth,
                },
                move |packet| {
                    let mut ring = for_packets.packets.lock();
                    if let Some(recorder) = for_packets.recorder.lock().as_ref() {
                        recorder.video(&packet);
                    }
                    ring.push(packet)
                },
                // The preamble arrives late — see `mft::catch_up_on_header`. The
                // one read during setup is usually empty, and a clip saved with
                // an empty one relies entirely on the encoder writing SPS/PPS
                // into the stream itself.
                move |header| *for_header.sequence_header.lock() = header,
            )?)
        };
        *shared.encoder.lock() = Some(encoder.chosen);
        *shared.rate_control.lock() = Some(encoder.rate_control);
        *shared.sequence_header.lock() = encoder.sequence_header.clone();
        *shared.encoder_stats.lock() = Some(encoder.stats.clone());
        *shared.pacer.lock() = Some(latest.clone());
        // The rotation of NV12 slots has to know how many frames the encoder is
        // holding on to, or it writes over one it is still reading — see
        // `convert::slots_for`.
        latest.watch_encoder(encoder.stats.clone(), encoder.queue_depth);

        let capture = {
            let latest = latest.clone();
            let closed_shared = shared.clone();
            let shared = shared.clone();
            wgc::start(
                &gpu,
                recording.target_kind,
                recording.target_id.as_deref(),
                recording.target_stable_id.as_deref(),
                recording.fps,
                move |frame| {
                    if let Err(err) = latest.submit(
                        frame.texture,
                        (frame.width, frame.height),
                        frame.qpc_100ns,
                    ) {
                        shared.dropped.fetch_add(1, Ordering::Relaxed);
                        shared.report(err);
                    }
                },
                {
                    let shared = closed_shared.clone();
                    move || {
                        shared.source_closed.store(true, Ordering::SeqCst);
                        shared.report(
                            "The capture source has gone away — no more frames are arriving."
                                .into(),
                        );
                    }
                },
            )?
        };
        *shared.source.lock() = capture.choice.clone();

        let pacer = {
            let latest = latest.clone();
            let sink = Box::new(EncoderSink {
                encoder: encoder.clone(),
                shared: shared.clone(),
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
        };
        let mixer = match mixer {
            Ok(mixer) => mixer,
            Err(err) => {
                // The clock is already running. Left alone it ran on detached,
                // kept the encoder and `Shared` alive and encoded the last
                // frame forever (BUF-10). Same order as `stop`.
                latest.stop();
                let _ = pacer.join();
                drop(capture);
                drop(encoder);
                return Err(format!("Could not start the audio mixer: {err}"));
            }
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

/// Longest window the mixer mixes in one go; anything longer is a gap. The
/// same 2 s the pacer and a recording's picture take for one
/// (`buffer::DISCONTINUITY_US`), so that a sleep is a gap on every side.
const MAX_WINDOW_100NS: i64 = crate::buffer::DISCONTINUITY_US * 10;

/// The mixer's position on the QPC axis, counted from where it started.
///
/// A window's end is worked out from the frames mixed in total, not added up
/// window by window: 10 ms of audio is not a whole number of 100 ns ticks, and
/// rounding every window lost a third of a tick each time — about 10 ms an
/// hour, which a recording carried into the file (AUD-6). The ring trims count
/// the same way ([`TrackInner::start_100ns`]), so the two still cancel where
/// a track is silent (AUD-V1).
struct MixClock {
    origin_100ns: i64,
    frames: u64,
}

impl MixClock {
    fn new(origin_100ns: i64) -> Self {
        Self { origin_100ns, frames: 0 }
    }

    /// Where the next window starts.
    fn at(&self) -> i64 {
        self.origin_100ns + frames_to_100ns(self.frames)
    }

    /// How many frames fit between here and `target_100ns` — none if the
    /// clock is already there.
    fn frames_until(&self, target_100ns: i64) -> usize {
        let total =
            (target_100ns as i128 - self.origin_100ns as i128) * SAMPLE_RATE as i128 / 10_000_000;
        (total - self.frames as i128).max(0) as usize
    }

    fn advance(&mut self, frames: usize) {
        self.frames += frames as u64;
    }
}

/// The mixer: produces gapless consecutive audio windows on the QPC axis.
fn mix_loop(shared: Arc<Shared>, stop: Arc<AtomicBool>) {
    let mut sources = shared.sources.lock().clone();
    let mut generation = shared.sources_generation.load(Ordering::Relaxed);

    let mut buffers: Vec<Vec<f32>> = Vec::new();
    let mut scratch: Vec<i16> = Vec::new();
    // Each window runs up to the clock, however imprecisely the thread wakes,
    // and its end is counted from the frames mixed in total, so no rounding
    // builds up either (AUD-6). Started by the first window.
    let mut clock: Option<MixClock> = None;

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(MIX_INTERVAL);

        let current = shared.sources_generation.load(Ordering::Relaxed);
        if current != generation {
            generation = current;
            sources = shared.sources.lock().clone();
            sync_tracks(&shared, &sources);
        }

        let target = now_100ns() - AUDIO_LAG_100NS;
        let clock = clock.get_or_insert_with(|| MixClock::new(target));
        let from = clock.at();
        let frames = clock.frames_until(target);
        if frames == 0 {
            continue;
        }

        // A window this long means the thread did not run — the PC slept, or
        // something stalled it. The clock kept counting, and mixing the whole
        // gap at once asked for gigabytes after a night's sleep: the
        // allocation failed and took the app and the buffer with it. Nobody
        // was playing in that gap, so it is counted as silence instead.
        if target - from > MAX_WINDOW_100NS {
            log::warn!("audio: {} s without a window — counted as silence", (target - from) / 10_000_000);
            let rings = shared.tracks.lock();
            let recording = shared.recorder.lock().clone();
            let recording = recording.filter(|recorder| recorder.begin_window(&rings, from));
            for ring in rings.iter() {
                ring.skip(frames * CHANNELS);
            }
            drop(rings);
            // A recording counts the gap rather than writing it; `finalize`
            // puts zeros back only where the picture went on.
            if let Some(recorder) = &recording {
                recorder.skip_window(frames);
            }
            // The buffers last held a window of normal length; nothing to give back.
            clock.advance(frames);
            shared.mixed.publish(clock.at());
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
        // Cloned out rather than held: `recorder` is otherwise only ever taken
        // under `packets`, and the mixer has no business with that one.
        let recording = shared.recorder.lock().clone();
        let recording = recording.filter(|recorder| recorder.begin_window(&rings, from));
        for ring in rings.iter() {
            let slot = recorded(&sources).position(|s| s.id == ring.source_id);
            scratch.clear();
            match slot.and_then(|index| buffers.get(index)) {
                Some(track) => scratch.extend(track.iter().map(|s| to_i16(*s))),
                None => scratch.resize(frames * CHANNELS, 0),
            }
            ring.push(&scratch, from);
            if let Some(recorder) = &recording {
                recorder.audio(ring, &scratch);
            }
        }
        drop(rings);
        if let Some(recorder) = &recording {
            recorder.end_window(frames);
        }

        clock.advance(frames);
        shared.mixed.publish(clock.at());
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
    ring_seconds: u32,
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
                ring_seconds,
            ))),
        }
    }
    out
}

/// Bring the track list in line with a changed config.
fn sync_tracks(shared: &Arc<Shared>, sources: &[AudioSource]) {
    let mut rings = shared.tracks.lock();
    let next = tracks_for(sources, &rings, shared.ring_seconds());
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
        let tracks = tracks_for(
            &sources,
            &[],
            ring_seconds(buffer_seconds, recording.keyframe_seconds),
        );

        // Without this, every ring still holds the last few minutes of audio that
        // nobody collected.
        audio.reset_rings();

        let packets = game_ring(buffer_seconds, buffer_bytes, recording.keyframe_seconds);

        let shared = Arc::new(Shared {
            buffer_seconds: AtomicU32::new(buffer_seconds),
            keyframe_seconds: recording.keyframe_seconds,
            mixed: Arc::new(MixPosition::default()),
            fps: recording.fps.max(1),
            size: (recording.width, recording.height),
            packets: Mutex::new(packets),
            tracks: Mutex::new(tracks),
            sources: Mutex::new(sources),
            audio,
            base_100ns: AtomicI64::new(0),
            anchored: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            duplicated: AtomicU64::new(0),
            errors: ErrorSlot::new(),
            sources_generation: AtomicU64::new(0),
            encoder: Mutex::new(None),
            rate_control: Mutex::new(None),
            sequence_header: Mutex::new(Vec::new()),
            recorder: Mutex::new(None),
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
            live_mark: Mutex::new(LiveMark {
                at: std::time::Instant::now(),
                frames: 0,
                duplicated: 0,
            }),
            source: Mutex::new(None),
            source_closed: AtomicBool::new(false),
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

    /// Begin a recording at the buffer's newest keyframe.
    ///
    /// `prepared` already has its folder, description and writer
    /// ([`Recorder::prepare`]): under the ring lock nothing touches the disk.
    /// A refused start (already recording, no keyframe) hands it back for the
    /// caller to [`recorder::Prepared::discard`] once its own locks are let go
    /// — `AppState.pipeline` is held around this call, and clearing the folder
    /// away is file work too.
    pub fn start_recording(
        &self,
        prepared: recorder::Prepared,
    ) -> Result<Arc<Recorder>, (recorder::Prepared, String)> {
        let ring = self.shared.packets.lock();
        if self.shared.recorder.lock().is_some() {
            return Err((prepared, "A recording is already running.".into()));
        }
        let backlog = ring.from_last_keyframe();
        let Some(first) = backlog.first() else {
            let why = "The encoder has not delivered a picture yet — try again in a moment.";
            return Err((prepared, why.into()));
        };
        let start_100ns = self.shared.qpc_of(first.pts_us);
        let header = self.shared.sequence_header.lock().clone();
        // An encoder that reorders its frames (B-frames) keeps the raw
        // stream: the TS carries no DTS. It shows a few frames after the
        // stream's first keyframe, and the ring keeps it until it is cleared.
        let timestamped = !ring.reorders();
        let recorder = Recorder::attach(prepared, &header, backlog, start_100ns, timestamped);
        *self.shared.recorder.lock() = Some(recorder.clone());
        drop(ring);
        Ok(recorder)
    }

    /// Unhook the running recording. Writing it out is the caller's —
    /// `Recorder::finish` and `recorder::finalize`.
    pub fn take_recording(&self) -> Option<Arc<Recorder>> {
        let _ring = self.shared.packets.lock();
        self.shared.recorder.lock().take()
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
        let pts: Vec<i64> = packets.iter().map(|p| p.pts_us).collect();
        // Highest rather than last: should frames ever be reordered, the last
        // packet is not the latest picture.
        let last_pts = pts.iter().copied().max().unwrap_or(0);
        let largest_step_us = crate::muxer::largest_step_us(&pts);
        let timestamped = crate::muxer::keeps_timestamps(
            &pts,
            largest_step_us,
            ts_jump_limit_us(self.shared.keyframe_seconds),
        );

        // The picture lasts the time its timestamps span, and the audio is cut
        // to the same length. With its own timestamps each frame sits where it
        // was captured, so the two meet everywhere, not only at the ends — a
        // stall in the middle is a held frame (BUF-6). Otherwise the picture is
        // spread evenly over that span (`muxer::timing`), which keeps the ends
        // together even when frames were lost or the clock skipped. Counted by
        // frames alone, as once, every lost frame left the audio a frame longer
        // than the picture.
        let span = crate::muxer::span_of(first_pts, last_pts, self.shared.fps);
        let timing = crate::muxer::timing(
            packets.len() as u64,
            span,
            Some(largest_step_us),
            self.shared.fps,
        );
        let audio_frames = clip_audio_frames(timestamped, span, &timing);

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
            audio_frames,
            rate: timing.rate,
            timestamped,
            largest_step_us,
            mixed: self.shared.mixed.clone(),
            width: self.shared.size.0,
            height: self.shared.size.1,
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

    #[test]
    fn an_error_after_a_quiet_first_poll_is_still_reported() {
        let slot = ErrorSlot::new();
        assert_eq!(slot.take_unseen(), None);
        slot.report("capture", "gone".into());
        assert_eq!(slot.take_unseen().as_deref(), Some("gone"));
    }

    #[test]
    fn an_error_is_reported_once_per_run() {
        let slot = ErrorSlot::new();
        slot.report("capture", "gone".into());
        assert_eq!(slot.take_unseen().as_deref(), Some("gone"));
        assert_eq!(slot.take_unseen(), None);
        slot.report("capture", "again".into());
        assert_eq!(slot.take_unseen(), None);
    }

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
    fn a_skipped_gap_reads_back_as_the_zeros_it_stands_for() {
        let pushed = TrackRing::new("a".into(), "A".into(), 5);
        let skipped = TrackRing::new("b".into(), "B".into(), 5);
        let sound: Vec<i16> = vec![3; STEP as usize * CHANNELS];
        let gap = vec![0i16; 4 * STEP as usize * CHANNELS];
        for ring in [&pushed, &skipped] {
            ring.push(&sound, qpc_of_frame(0));
        }
        pushed.push(&gap, qpc_of_frame(STEP));
        skipped.skip(gap.len());
        for ring in [&pushed, &skipped] {
            ring.push(&sound, qpc_of_frame(5 * STEP));
        }
        let whole = 6 * STEP as usize;
        assert_eq!(skipped.window(0, whole), pushed.window(0, whole));
        assert_eq!(skipped.window(0, whole)[5 * STEP as usize * CHANNELS], 3);
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
    fn a_new_buffer_length_reaches_a_running_ring() {
        // Ring seconds are exact: 1 s, then 4 s, then 1 s again.
        let ring = TrackRing::new("a".into(), "A".into(), 1);
        let block = vec![7i16; SAMPLE_RATE as usize * CHANNELS];
        let second = block.len();
        for at in 0..5 {
            ring.push(&block, qpc_of_frame(at * SAMPLE_RATE as i64));
        }
        assert_eq!(ring.inner.lock().samples.len(), second);

        ring.set_seconds(4);
        for at in 5..12 {
            ring.push(&block, qpc_of_frame(at * SAMPLE_RATE as i64));
        }
        assert_eq!(ring.inner.lock().samples.len(), 4 * second, "longer fills up");

        ring.set_seconds(1);
        ring.push(&block, qpc_of_frame(12 * SAMPLE_RATE as i64));
        assert_eq!(ring.inner.lock().samples.len(), second, "shorter is cut down");
    }

    /// BUF-1: a clip as long as the buffer starts up to one keyframe interval
    /// before `newest − seconds`, and its audio is read only after the video is
    /// written. The ring must still hold that start a second later.
    #[test]
    fn a_full_length_clip_still_finds_its_start_after_a_slow_write() {
        let ring = TrackRing::new("a".into(), "A".into(), ring_seconds(10, 4)); // 24 s
        let second = SAMPLE_RATE as i64;
        for at in 0..31 {
            ring.push(&vec![(at + 1) as i16; second as usize * CHANNELS], qpc_of_frame(at * second));
        }
        // Newest at 30 s, cut at the worst keyframe phase: 30 − 10 − 3.9 = 16.1 s,
        // read after a 1 s write (the 31st block).
        assert_eq!(ring.window(qpc_of_frame(772_800), 1)[0], 17, "the clip's start was trimmed away");
    }

    /// BUF-1: what the slack has to cover.
    #[test]
    fn a_ring_covers_the_buffer_a_keyframe_interval_and_ten_seconds() {
        assert_eq!(ring_seconds(120, 2), 132);
        assert_eq!(ring_seconds(120, 4), 134);
        assert_eq!(ring_seconds(30, 0), 41, "an interval of 0 counts as one second");
        assert_eq!(ring_seconds(120, 600), 140, "an interval from the config counts at most 10 s");
        assert_eq!(ring_seconds(u32::MAX, 4), u32::MAX, "a huge buffer saturates");
    }

    /// ffmpeg closes up a TS hole above 10 s (`-dts_delta_threshold`), so a
    /// clip keeps its own timestamps only across steps up to 8 s — with 4 s
    /// keyframes too, whose ring limit is 12 s.
    #[test]
    fn a_clip_keeps_its_timestamps_only_across_holes_ffmpeg_leaves_alone() {
        assert_eq!(ts_jump_limit_us(4), 8_000_000);
        assert_eq!(ts_jump_limit_us(2), 6_000_000, "below the cap the ring's limit holds");
        assert_eq!(ts_jump_limit_us(1), 3_000_000);
        let pts = [0, 16_666, 11_016_666, 11_033_333];
        let step = crate::muxer::largest_step_us(&pts);
        assert!(!crate::muxer::keeps_timestamps(&pts, step, ts_jump_limit_us(4)), "an 11 s freeze takes the raw path");
    }

    /// Final review I-1: with 4 s keyframes the ring kept holes up to 12 s,
    /// but a clip keeps its timestamps only up to 8 s. A 9 s freeze went out
    /// spread evenly with sound as long as the frames, and everything after
    /// it played 9 s ahead of its sound. The ring cuts such a hole instead.
    #[test]
    fn a_hole_too_long_for_a_clips_timestamps_is_cut_from_the_ring() {
        use crate::buffer::{EncodedPacket, VIDEO_TRACK};
        let mut ring = game_ring(30, u64::MAX, 4);
        let mut push = |from_ms: i64, frames: i64| {
            for frame in 0..frames {
                ring.push(EncodedPacket {
                    track: VIDEO_TRACK,
                    pts_us: (from_ms + frame * 1000 / 60) * 1000,
                    duration_us: 16_666,
                    keyframe: frame % 240 == 0,
                    data: vec![0u8; 100].into(),
                });
            }
        };
        push(0, 10 * 60);
        push(10_000 + 9_000, 10 * 60);

        let clip = ring.snapshot(30);
        let pts: Vec<i64> = clip.iter().map(|p| p.pts_us).collect();
        assert!(pts[0] >= 19_000_000, "the clip still reaches back over the hole to {} us", pts[0]);
        let step = crate::muxer::largest_step_us(&pts);
        assert!(crate::muxer::keeps_timestamps(&pts, step, ts_jump_limit_us(4)), "what is left keeps its timestamps");
    }

    /// Final review: with its own timestamps the audio is as long as the span
    /// they cover, not the span spread evenly over the frames.
    #[test]
    fn a_timestamped_clip_takes_its_audio_length_from_the_span() {
        let timing = crate::muxer::Timing { rate: "60".into(), span_us: 1_000_000 };
        assert_eq!(clip_audio_frames(true, Some(1_500_000), &timing), 72_000);
        assert_eq!(clip_audio_frames(false, Some(1_500_000), &timing), 48_000);
        assert_eq!(clip_audio_frames(true, None, &timing), 48_000, "no span: the timing's");
    }

    /// BUF-1, the tail: a save waits for the mixer to pass the clip's end, so the
    /// last 80 ms the mixer has not reached yet do not come out as zeros.
    #[test]
    fn a_save_waits_for_the_mixer_to_pass_the_clips_end() {
        let mixed = Arc::new(MixPosition::default());
        let mixer = {
            let mixed = mixed.clone();
            std::thread::spawn(move || for step in 1..=5 {
                std::thread::sleep(Duration::from_millis(10));
                mixed.publish(step * 100_000);
            })
        };
        assert!(mixed.wait_until(500_000, MIX_WAIT));
        mixer.join().unwrap();
    }

    /// BUF-1 and Review Focus: a stalled or stopped mixer must not hold up the save.
    #[test]
    fn the_wait_for_a_stalled_mixer_gives_up() {
        let began = std::time::Instant::now();
        assert!(!MixPosition::default().wait_until(1, MIX_WAIT));
        assert!(began.elapsed() >= MIX_WAIT && began.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn the_oldest_audio_is_dropped_and_the_start_moves_with_it() {
        // A capacity of exactly 1 s.
        let ring = TrackRing::new("a".into(), "A".into(), 1);
        // Not zeros: silence is not stored at all, and the ring would stay empty.
        let block = vec![7i16; SAMPLE_RATE as usize * CHANNELS];
        for second in 0..5 {
            ring.push(&block, qpc_of_frame(second * SAMPLE_RATE as i64));
        }
        // 48000 is divisible by 6 — so the one-second steps are exact.
        let inner = ring.inner.lock();
        assert_eq!(inner.samples.len(), ring.capacity());
        assert!(
            inner.start_100ns() > 0,
            "the ring start has to move along, otherwise every window points beside"
        );
    }

    /// AUD-6: rounding each window lost a third of a tick per window, about 10 ms
    /// an hour, and recordings carried it into the file.
    #[test]
    fn the_mixer_clock_does_not_drift_over_an_hour() {
        let mut clock = MixClock::new(7_000_000_000_000);
        let (mut seed, mut total) = (1u64, 0u64);
        while total < 3_600 * 48_000 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let frames = 480 + (seed >> 33) as usize % 49; // windows of 10.0–11.0 ms
            clock.advance(frames);
            total += frames as u64;
        }
        let exact = (total as i128 * 10_000_000 / 48_000) as i64;
        assert!((clock.at() - 7_000_000_000_000 - exact).abs() <= 1);
    }

    /// AUD-V1: counted silence broke the cancellation between the mixer and the
    /// ring's trim, so a track that is quiet half the time drifted late over hours.
    #[test]
    fn a_track_quiet_half_the_time_stays_on_the_clock_for_an_hour() {
        let ring = TrackRing::new("a".into(), "A".into(), 10);
        let (mut clock, mut seed, mut last) = (MixClock::new(0), 1u64, 0i64);
        while clock.at() < 36_000_000_000 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let frames = 480 + (seed >> 33) as usize % 49;
            let from = clock.at();
            let sounding = (from / 10_000_000) % 2 == 0;
            // A ramp starting at 1, so a window shows which frame of the block it hit.
            let block: Vec<i16> = (0..frames)
                .flat_map(|i| {
                    let v = if sounding { i as i16 + 1 } else { 0 };
                    [v, v]
                })
                .collect();
            ring.push(&block, from);
            if sounding {
                last = from;
            }
            clock.advance(frames);
        }
        let w = ring.window(last, 2);
        assert!(w[0] == 1 || w[0] == 2 || w[CHANNELS] == 1, "the last block reads back at {w:?}");
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
        // A capacity of exactly 1 s.
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
