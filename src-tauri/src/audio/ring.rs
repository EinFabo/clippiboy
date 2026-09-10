//! One ring buffer per audio source — with a timeline.
//!
//! This used to be nothing but a queue of samples, and whoever read simply took
//! from the front. How much had been read was worked out by the recording from
//! the wall clock (`Instant::now()`). But the WASAPI clock does not run quite as
//! fast as the wall clock — the difference accumulated, had to be thrown away
//! with `trim_to`, and that is exactly what you heard as dropouts and as audio
//! slowly running away.
//!
//! Now every written block carries the QPC timestamp WASAPI supplies anyway
//! (`pu64QPCPosition`). Reading no longer means "whatever is next" but **a
//! window in time**. That is the same clock
//! `Direct3D11CaptureFrame::SystemRelativeTime` sits on — so picture and sound
//! are measured against one another instead of each against itself.
//!
//! One timestamp on its own is not enough, though. Anchoring the timeline on the
//! first block and running it off the sample count from there assumes the device
//! hands over exactly 48000 frames per second of QPC, and no crystal does. Fifty
//! to a hundred ppm is ordinary, which is three to six milliseconds a minute:
//! after an hour of buffer a source sits a third of a second away from the
//! picture, and two devices sit that far from each other. The buffer runs as
//! long as the app does, so it had all day to add up — and only one direction
//! was ever caught. A device running slow tripped the gap tolerance every few
//! minutes and jumped back into place; a device running fast was let go, and ran
//! away without a limit until the ring was shorter than the error and the source
//! fell silent altogether.
//!
//! So the timeline is now pulled towards the timestamps the whole time, by a
//! rate-limited amount per block (`MAX_SLEW_PPM`). The cost is a single frame
//! repeated or skipped every few seconds; what it buys is a source that stays
//! where it belongs however long the buffer runs. The same pull also swallows an
//! anchor that started out crooked — the first block after `clear` sets the zero
//! point, and where the device left the timestamp empty that is the read-out
//! time, a good ten milliseconds late.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::Mutex;

/// How much audio the ring holds on to. The mixer collects every few
/// milliseconds; a second is ample slack for a brief stall and costs only a few
/// hundred kilobytes per source.
const CAPACITY_MS: usize = 1000;

/// From what deviation a block counts as a jump in the timeline.
const GAP_TOLERANCE_100NS: i64 = 50 * 10_000;

/// At most this much silence is inserted to bridge a gap. Beyond that the source
/// was simply gone — the timeline then starts over instead of filling the ring
/// up with silence.
const MAX_GAP_FILL_100NS: i64 = 1_000 * 10_000;

/// How much of a block's own duration may go into pulling the timeline, in parts
/// per million. Crystals are off by tens of ppm, so this leaves room to spare and
/// still averages the jitter of the single timestamps away.
const MAX_SLEW_PPM: i64 = 2_000;

/// The headroom that leaves over a crystal: a bad one is 100 ppm out, so twenty
/// times over. Checked here rather than in a test because it is a statement
/// about the constant, not about anything that runs.
const _: () = assert!(MAX_SLEW_PPM >= 20 * 100);

struct Inner {
    samples: VecDeque<f32>,
    /// QPC (100 ns) of the first sample in `samples`.
    start_100ns: i64,
    primed: bool,
}

pub struct SampleRing {
    inner: Mutex<Inner>,
    sample_rate: u32,
    channels: usize,
    capacity: usize,
    /// Peak level as f32 bits so the UI can read it without a lock.
    peak: AtomicU32,
}

impl SampleRing {
    pub fn new(sample_rate: u32, channels: usize) -> Self {
        let capacity = sample_rate as usize * channels * CAPACITY_MS / 1000;
        Self {
            inner: Mutex::new(Inner {
                samples: VecDeque::with_capacity(capacity),
                start_100ns: 0,
                primed: false,
            }),
            sample_rate,
            channels,
            capacity,
            peak: AtomicU32::new(0),
        }
    }

    fn frames_to_100ns(&self, frames: usize) -> i64 {
        frames as i64 * 10_000_000 / self.sample_rate as i64
    }

    fn duration_to_frames(&self, span_100ns: i64) -> i64 {
        span_100ns * self.sample_rate as i64 / 10_000_000
    }

    /// How far the timeline may be pulled by one block of `frames`.
    ///
    /// Rate-limited instead of applied in full, because a single QPC stamp
    /// carries the jitter of whatever thread read it — following that exactly
    /// would shove the read position back and forth by whole milliseconds, which
    /// is the very thing the timestamps were brought in to stop. Measured as a
    /// share of the block rather than a fixed figure, so it does not depend on
    /// how large a device's packets are: at 48 kHz and 10 ms blocks it comes to
    /// roughly one frame.
    fn max_slew_100ns(&self, frames: usize) -> i64 {
        (self.frames_to_100ns(frames) * MAX_SLEW_PPM / 1_000_000).max(1)
    }

    /// Put in one block with its QPC timestamp.
    pub fn write(&self, block: &[f32], qpc_100ns: i64) {
        if block.is_empty() {
            return;
        }

        let mut peak = 0.0f32;
        for sample in block {
            let value = sample.abs();
            if value > peak {
                peak = value;
            }
        }
        let previous = f32::from_bits(self.peak.load(Ordering::Relaxed));
        if peak > previous {
            self.peak.store(peak.to_bits(), Ordering::Relaxed);
        }

        let mut inner = self.inner.lock();
        if !inner.primed {
            inner.start_100ns = qpc_100ns;
            inner.primed = true;
        } else {
            // Where would this block sit if nothing had been missing?
            let held = inner.samples.len() / self.channels;
            let expected = inner.start_100ns + self.frames_to_100ns(held);
            let drift = qpc_100ns - expected;

            if drift.abs() > MAX_GAP_FILL_100NS {
                // The source was gone for a long time, or the device set its
                // clock anew. Either way nothing held here relates to what
                // arrives now: restart the timeline.
                inner.samples.clear();
                inner.start_100ns = qpc_100ns;
            } else if drift > GAP_TOLERANCE_100NS {
                // A real gap (the device dropped out): fill with silence so
                // everything after it stays in its right place.
                let missing = self.duration_to_frames(drift) as usize * self.channels;
                inner.samples.extend(std::iter::repeat(0.0).take(missing));
            } else {
                // What is left over is the difference between two crystals, in
                // whichever direction. Follow it — but only by a sliver of the
                // block, see `max_slew_100ns`. Letting a negative offset lie,
                // the way this used to, is what let a fast device run away.
                let slew = self.max_slew_100ns(block.len() / self.channels);
                inner.start_100ns += drift.clamp(-slew, slew);
            }
        }

        inner.samples.extend(block.iter().copied());

        if inner.samples.len() > self.capacity {
            let excess = inner.samples.len() - self.capacity;
            inner.samples.drain(..excess);
            let dropped_frames = excess / self.channels;
            inner.start_100ns += self.frames_to_100ns(dropped_frames);
        }
    }

    /// Read out a window in time: `out` is filled completely, missing stretches
    /// with silence.
    ///
    /// Consumes nothing — the ring discards by itself whatever is older than its
    /// capacity. That way the same stretch cannot be delivered once too much and
    /// once too little depending on read order, which is what shifted the sources
    /// against each other with the old "take from the front".
    pub fn read_window(&self, from_100ns: i64, out: &mut [f32]) {
        out.fill(0.0);

        let inner = self.inner.lock();
        if !inner.primed || inner.samples.is_empty() {
            return;
        }

        // Offset of the wanted start against the ring's start, in samples.
        let offset_frames = self.duration_to_frames(from_100ns - inner.start_100ns);
        let offset = offset_frames * self.channels as i64;

        // `src` runs over the ring, `dst` over the output. If the window lies
        // partly before the ring's start, `dst` begins correspondingly later.
        let (mut src, mut dst) = if offset < 0 {
            (0usize, (-offset) as usize)
        } else {
            (offset as usize, 0usize)
        };

        while dst < out.len() {
            let Some(sample) = inner.samples.get(src) else {
                break;
            };
            out[dst] = *sample;
            src += 1;
            dst += 1;
        }
    }

    /// QPC of the first sample still in the ring.
    pub fn start_100ns(&self) -> Option<i64> {
        let inner = self.inner.lock();
        inner.primed.then_some(inner.start_100ns)
    }

    /// QPC past the last sample — there is material up to here.
    pub fn end_100ns(&self) -> Option<i64> {
        let inner = self.inner.lock();
        if !inner.primed {
            return None;
        }
        let held = inner.samples.len() / self.channels;
        Some(inner.start_100ns + self.frames_to_100ns(held))
    }

    /// Peak level since the last call (0.0–1.0), reset afterwards.
    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.peak.swap(0, Ordering::Relaxed))
    }

    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.samples.clear();
        inner.primed = false;
        inner.start_100ns = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deliberately not 48 kHz: a sample there lasts 208.33 QPC ticks and cannot
    /// be expressed exactly at 100 ns resolution. For the window logic that is
    /// beside the point (real blocks are hundreds of samples long), but for a
    /// sample-exact check it is fatal — hence a rate here where one sample lasts
    /// exactly 1000 ticks.
    const RATE: u32 = 10_000;

    /// QPC (100 ns) for a sample position at `RATE`.
    fn at(frames: i64) -> i64 {
        frames * 10_000_000 / RATE as i64
    }

    #[test]
    fn peak_is_reported_once_and_then_reset() {
        let ring = SampleRing::new(RATE, 2);
        ring.write(&[0.1, -0.7, 0.3, 0.2], 0);
        assert!((ring.take_peak() - 0.7).abs() < 1e-6);
        assert_eq!(ring.take_peak(), 0.0);
    }

    #[test]
    fn a_window_is_read_at_its_timestamp() {
        let ring = SampleRing::new(RATE, 1);
        ring.write(&[1.0, 2.0, 3.0, 4.0], at(100));

        // Exactly from the second sample on.
        let mut out = vec![0.0; 2];
        ring.read_window(at(101), &mut out);
        assert_eq!(out, vec![2.0, 3.0]);
    }

    #[test]
    fn a_window_before_the_data_is_padded_with_silence() {
        let ring = SampleRing::new(RATE, 1);
        ring.write(&[1.0, 2.0], at(10));

        let mut out = vec![9.0; 4];
        ring.read_window(at(8), &mut out);
        assert_eq!(out, vec![0.0, 0.0, 1.0, 2.0]);
    }

    #[test]
    fn a_window_past_the_data_is_padded_with_silence() {
        let ring = SampleRing::new(RATE, 1);
        ring.write(&[1.0, 2.0], at(0));

        let mut out = vec![9.0; 4];
        ring.read_window(at(0), &mut out);
        assert_eq!(out, vec![1.0, 2.0, 0.0, 0.0]);
    }

    /// If the device drops out briefly, nothing later may slide forward —
    /// otherwise the audio would run ahead of the picture from there on.
    #[test]
    fn a_gap_is_filled_with_silence_so_later_audio_keeps_its_place() {
        let ring = SampleRing::new(RATE, 1);
        ring.write(&[1.0], at(0));
        // The next block only 100 ms later instead of after one sample.
        ring.write(&[2.0], at(0) + 100 * 10_000);

        let expected_position = ring.start_100ns().unwrap() + 100 * 10_000;
        let mut out = vec![0.0; 1];
        ring.read_window(expected_position, &mut out);
        assert_eq!(out, vec![2.0], "the block after the gap is in the wrong place");
    }

    /// A source that was gone for minutes must not bury the ring in silence.
    #[test]
    fn a_long_absence_restarts_the_timeline() {
        let ring = SampleRing::new(RATE, 1);
        ring.write(&[1.0], at(0));
        ring.write(&[2.0], at(0) + 10 * 10_000_000);

        assert_eq!(ring.start_100ns(), Some(at(0) + 10 * 10_000_000));
        let mut out = vec![0.0; 1];
        ring.read_window(at(0) + 10 * 10_000_000, &mut out);
        assert_eq!(out, vec![2.0]);
    }

    #[test]
    fn the_oldest_material_is_dropped_and_the_start_moves_with_it() {
        // 1000 Hz mono, 1000 ms capacity -> 1000 samples
        let ring = SampleRing::new(1000, 1);
        for frame in 0..1500i64 {
            ring.write(&[0.5], frame * 10_000_000 / 1000);
        }
        let start = ring.start_100ns().unwrap();
        let end = ring.end_100ns().unwrap();
        assert_eq!((end - start) / 10_000, 1000, "ring does not hold 1000 ms");
    }

    #[test]
    fn clear_empties_the_ring() {
        let ring = SampleRing::new(RATE, 2);
        ring.write(&[1.0, 2.0], 0);
        ring.clear();
        assert_eq!(ring.start_100ns(), None);
    }

    // The clock tests below run at the real 48 kHz on purpose. What is being
    // measured here is a difference of parts per million over minutes, and that
    // only means anything at the rate and the block size a device actually uses.

    /// 10 ms at 48 kHz — the usual WASAPI packet.
    const BLOCK: usize = 480;

    /// Runs a source whose crystal is `ppm` beside QPC for `seconds`, and gives
    /// back how far the ring's timeline ends up from the real clock, in ms.
    ///
    /// Positive means the ring believes it holds more time than has passed — the
    /// mixer then reads material that is that much too old, and the source lags
    /// the picture by exactly this figure.
    fn timeline_error_ms(ppm: f64, seconds: f64) -> f64 {
        let ring = SampleRing::new(48_000, 1);
        let device_rate = 48_000.0 * (1.0 + ppm / 1e6);
        let block = vec![0.5f32; BLOCK];

        let mut produced = 0usize;
        let mut qpc = 0i64;
        while (produced as f64 / device_rate) < seconds {
            // The stamp is the true time the device took the first frame of the
            // block — that is what WASAPI reports and what the picture sits on.
            qpc = ((produced as f64 / device_rate) * 1e7) as i64;
            ring.write(&block, qpc);
            produced += BLOCK;
        }

        let should_end = qpc + BLOCK as i64 * 10_000_000 / 48_000;
        (ring.end_100ns().unwrap() - should_end) as f64 / 10_000.0
    }

    /// The bug this file exists for. A device running faster than QPC pushes more
    /// samples in than time passes, and the old code let that stand — "a few
    /// samples twice". Over ten minutes it was 60 ms, over an hour a third of a
    /// second, and it never turned round.
    #[test]
    fn a_fast_device_does_not_run_away() {
        let error = timeline_error_ms(100.0, 600.0);
        assert!(
            error.abs() < 1.0,
            "after ten minutes the timeline is {error:.1} ms off (uncorrected: about +60)"
        );
    }

    /// The other direction used to right itself, but only by tripping the gap
    /// tolerance — 50 ms of silence dropped in every few minutes. Now it never
    /// gets that far.
    #[test]
    fn a_slow_device_does_not_run_away() {
        let error = timeline_error_ms(-100.0, 600.0);
        assert!(
            error.abs() < 1.0,
            "after ten minutes the timeline is {error:.1} ms off"
        );
    }

    /// Half an hour at the drift of a bad crystal — the length of a session, and
    /// the case where the old code gave up entirely: past a second of error the
    /// window falls out of the ring and the source goes silent.
    #[test]
    fn half_an_hour_at_a_bad_crystal_stays_put() {
        let error = timeline_error_ms(250.0, 1800.0);
        assert!(
            error.abs() < 1.0,
            "after half an hour the timeline is {error:.1} ms off (uncorrected: about +450)"
        );
    }

    /// Two sources on different crystals have to stay together, not just each
    /// stay near the picture — a microphone against game sound is where a
    /// listener hears it first.
    #[test]
    fn two_devices_stay_together() {
        let a = timeline_error_ms(60.0, 900.0);
        let b = timeline_error_ms(-60.0, 900.0);
        assert!(
            (a - b).abs() < 2.0,
            "after fifteen minutes the two sit {:.1} ms apart (uncorrected: about 58)",
            (a - b).abs()
        );
    }

    /// A clock that matches must not be pulled about by the correction.
    #[test]
    fn a_matching_clock_is_left_alone() {
        let error = timeline_error_ms(0.0, 300.0);
        assert!(error.abs() < 0.1, "an exact clock was moved by {error:.3} ms");
    }

    /// The point of rate-limiting: one stamp read late by a busy thread says
    /// nothing about the crystal, and must not shift the read position.
    ///
    /// Measured at the end of the timeline rather than the start — the start
    /// moves by a whole block on every write once the ring is full, simply
    /// because the oldest block drops off, and that would drown the figure.
    #[test]
    fn one_jittery_timestamp_barely_moves_the_timeline() {
        let ring = SampleRing::new(48_000, 1);
        let block = vec![0.5f32; BLOCK];
        for index in 0..200i64 {
            ring.write(&block, index * 100_000);
        }
        let before = ring.end_100ns().unwrap();

        // The next block claims to be 5 ms early.
        ring.write(&block, 200 * 100_000 - 50_000);
        let followed = before + 100_000 - ring.end_100ns().unwrap();

        assert!(
            followed.abs() <= 208,
            "a stamp 5 ms out pulled the timeline {followed} ticks — jitter is being followed"
        );
    }

    /// The limit is a rate, not a fixed figure: the same parts per million
    /// whatever size packets a device hands over. Otherwise a source on 20 ms
    /// buffers would be corrected half as fast as one on 10 ms, for no reason
    /// that has anything to do with its crystal.
    #[test]
    fn the_correction_is_capped_at_a_constant_rate() {
        let ring = SampleRing::new(48_000, 2);
        for frames in [48usize, 240, 480, 960, 4800] {
            let span = ring.frames_to_100ns(frames);
            let ppm = ring.max_slew_100ns(frames) * 1_000_000 / span;
            assert_eq!(ppm, MAX_SLEW_PPM, "a block of {frames} frames is capped at {ppm} ppm");
        }

        // At the usual packet that comes to a single frame — the figure the
        // module doc quotes, and the reason the correction cannot be heard.
        assert!(ring.max_slew_100ns(BLOCK) <= ring.frames_to_100ns(1));

    }

    /// The audible side of it. Whatever the correction does to the timeline, the
    /// material under a point in time that steps forward evenly may move by at
    /// most one frame per block — one sample repeated or skipped, which nobody
    /// hears. Following the jitter instead would move it by tens of frames, and
    /// that is a crackle.
    #[test]
    fn material_under_an_even_clock_moves_by_at_most_one_frame() {
        let ring = SampleRing::new(48_000, 1);
        // A ramp, so every sample says where in the stream it sat.
        let mut counter = 0f32;
        let mut block = vec![0.0f32; BLOCK];
        let mut previous: Option<f32> = None;
        let mut checked = 0;

        for index in 0..2_000i64 {
            for slot in block.iter_mut() {
                *slot = counter;
                counter += 1.0;
            }
            // A device 300 ppm fast, with 2 ms of jitter on every stamp.
            let jitter = if index % 2 == 0 { 20_000 } else { -20_000 };
            ring.write(&block, (index as f64 * 100_000.0 / 1.0003) as i64 + jitter);

            // Warm-up first: the ring has to be full and the anchor settled.
            if index < 500 {
                continue;
            }
            // A reading point on the QPC axis, one block further along each time.
            let mut out = [0.0f32; 1];
            ring.read_window(index * 100_000 - 3_000_000, &mut out);
            if let Some(before) = previous {
                // One block further in time has to be one block further into the
                // material — give or take the one frame the correction may move.
                let advance = out[0] - before;
                assert!(
                    (advance - BLOCK as f32).abs() <= 1.0,
                    "block {index}: the material jumped {advance} frames instead of {BLOCK}"
                );
                checked += 1;
            }
            previous = Some(out[0]);
        }
        assert!(checked > 1_000, "only {checked} readings were checked");
    }

    /// The anchor is whatever the first block after `clear` says, and where the
    /// device left `pu64QPCPosition` empty that is the read-out time — ten to
    /// fifteen milliseconds late. The pull towards the stamps takes that out too.
    #[test]
    fn a_crooked_anchor_is_pulled_straight() {
        let ring = SampleRing::new(48_000, 1);
        let block = vec![0.5f32; BLOCK];

        // First block stamped 15 ms late, everything after it honest.
        ring.write(&block, 150_000);
        let mut qpc = 0i64;
        for index in 1..3_000i64 {
            qpc = index * 100_000;
            ring.write(&block, qpc);
        }

        let should_end = qpc + BLOCK as i64 * 10_000_000 / 48_000;
        let error = (ring.end_100ns().unwrap() - should_end) as f64 / 10_000.0;
        assert!(
            error.abs() < 1.0,
            "the anchor error is still {error:.1} ms after thirty seconds"
        );
    }

    /// The mirror of a long absence: a device that sets its clock back further
    /// than the ring is long has nothing to do with what is stored, so the
    /// timeline starts over rather than crawling there one frame at a time.
    #[test]
    fn a_large_overlap_restarts_the_timeline() {
        let ring = SampleRing::new(48_000, 1);
        let block = vec![0.5f32; BLOCK];
        ring.write(&block, 100_000_000);
        ring.write(&block, 80_000_000);

        assert_eq!(ring.start_100ns(), Some(80_000_000));
        assert_eq!(
            ring.end_100ns(),
            Some(80_000_000 + BLOCK as i64 * 10_000_000 / 48_000),
            "the restarted timeline holds one block and nothing else"
        );
    }

    /// A real dropout still has to be bridged with silence — the correction must
    /// not quietly swallow gaps that the picture will keep.
    #[test]
    fn a_dropout_is_still_bridged_rather_than_slewed() {
        let ring = SampleRing::new(48_000, 1);
        let block = vec![0.5f32; BLOCK];
        ring.write(&block, 0);
        // 200 ms nothing, then on again.
        ring.write(&block, 2_000_000);

        let end = ring.end_100ns().unwrap();
        let should_end = 2_000_000 + BLOCK as i64 * 10_000_000 / 48_000;
        assert!(
            (end - should_end).abs() < 1_000,
            "after the gap the material ends at {end} instead of {should_end}"
        );
    }
}
