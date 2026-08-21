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
//! `Direct3D11CaptureFrame::SystemRelativeTime` sits on — picture and sound are
//! therefore in sync of their own accord, without correction.

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

            if drift > GAP_TOLERANCE_100NS {
                if drift <= MAX_GAP_FILL_100NS {
                    // A real gap (the device dropped out): fill with silence so
                    // everything after it stays in its right place.
                    let missing = self.duration_to_frames(drift) as usize * self.channels;
                    inner.samples.extend(std::iter::repeat(0.0).take(missing));
                } else {
                    // The source was gone for a long time. Restart the timeline.
                    inner.samples.clear();
                    inner.start_100ns = qpc_100ns;
                }
            }
            // A negative offset (the block sits before the expected point) means
            // overlap. Writing those few samples twice is more harmless than
            // computing them out.
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
}
