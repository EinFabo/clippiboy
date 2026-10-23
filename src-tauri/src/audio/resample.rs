//! One linear resampler to 48 kHz stereo for every packet of a device, sounding
//! or silent. It carries its phase and the last frame across packets, so no
//! fraction of a frame is lost at a packet edge (AUD-7) and the zeros of a
//! SILENT packet take exactly the time their frames last (AUD-V2).

use super::capture::{CHANNELS, SAMPLE_RATE};

pub struct Resampler {
    /// Input frames per output frame: device rate / 48 000.
    step: f64,
    /// Where the next output frame sits, in input frames counted from the first
    /// frame of the next block; −1 is `prev`.
    pos: f64,
    /// The last frame of the block before.
    prev: [f32; CHANNELS],
    passthrough: bool,
    zeros: Vec<f32>,
}

impl Resampler {
    pub fn new(device_rate: u32) -> Self {
        Self {
            step: device_rate.max(1) as f64 / SAMPLE_RATE as f64,
            pos: 0.0,
            prev: [0.0; CHANNELS],
            passthrough: device_rate == SAMPLE_RATE,
            zeros: Vec::new(),
        }
    }

    /// Append `stereo` (interleaved, at the device rate) to `out` at 48 kHz.
    pub fn process(&mut self, stereo: &[f32], out: &mut Vec<f32>) {
        if self.passthrough {
            out.extend_from_slice(stereo);
            return;
        }
        let n = stereo.len() / CHANNELS;
        if n == 0 {
            return;
        }
        let prev = self.prev;
        let at = |i: isize| -> [f32; CHANNELS] {
            if i < 0 {
                prev
            } else {
                [stereo[CHANNELS * i as usize], stereo[CHANNELS * i as usize + 1]]
            }
        };
        while self.pos < n as f64 - 1.0 {
            let i = self.pos.floor() as isize;
            let frac = (self.pos - i as f64) as f32;
            let (a, b) = (at(i), at(i + 1));
            out.extend([a[0] + (b[0] - a[0]) * frac, a[1] + (b[1] - a[1]) * frac]);
            self.pos += self.step;
        }
        self.pos -= n as f64;
        self.prev = at(n as isize - 1);
    }

    /// The same for `frames` frames of silence at the device rate.
    pub fn silence(&mut self, frames: usize, out: &mut Vec<f32>) {
        let mut zeros = std::mem::take(&mut self.zeros);
        zeros.clear();
        zeros.resize(frames * CHANNELS, 0.0);
        self.process(&zeros, out);
        self.zeros = zeros;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::ring::SampleRing;

    /// AUD-7: 448-frame packets at 44.1 kHz each lost 0.62 frames (1 270 ppm).
    #[test]
    fn odd_packets_at_44k_lose_no_fraction_of_a_frame() {
        let mut resampler = Resampler::new(44_100);
        let (packet, mut out) = (vec![0.25f32; 448 * CHANNELS], Vec::new());
        for _ in 0..1_000 {
            resampler.process(&packet, &mut out);
        }
        // 448 000 × 48 000 / 44 100 = 487 619.05; the last fraction waits for the next packet.
        assert!((out.len() / CHANNELS).abs_diff(487_619) <= 2, "{} frames", out.len() / CHANNELS);
    }

    /// AUD-V2: SILENT packets from a 96 kHz device added 20 ms of timeline per
    /// 10 ms packet, and the source then lagged by up to a second for minutes.
    #[test]
    fn silent_packets_at_96k_take_only_their_real_time() {
        let (mut resampler, mut out) = (Resampler::new(96_000), Vec::new());
        for _ in 0..100 {
            resampler.silence(960, &mut out);
        }
        assert!((out.len() / CHANNELS).abs_diff(48_000) <= 1);
    }

    /// AUD-7: the cut between packets must not be audible or measurable.
    #[test]
    fn a_tone_resampled_in_pieces_matches_it_resampled_whole() {
        let tone: Vec<f32> = (0..44_100)
            .flat_map(|i| {
                let v = (2.0 * std::f32::consts::PI * 1_000.0 * i as f32 / 44_100.0).sin();
                [v, v]
            })
            .collect();
        let mut whole = Vec::new();
        Resampler::new(44_100).process(&tone, &mut whole);

        let (mut resampler, mut pieces) = (Resampler::new(44_100), Vec::new());
        let (mut at, mut k) = (0, 0);
        let sizes = [441, 448, 100, 7, 1_000];
        while at < 44_100 {
            let len = sizes[k % sizes.len()].min(44_100 - at);
            resampler.process(&tone[at * CHANNELS..(at + len) * CHANNELS], &mut pieces);
            at += len;
            k += 1;
        }
        assert!((whole.len() / CHANNELS).abs_diff(pieces.len() / CHANNELS) <= 1);
        let common = whole.len().min(pieces.len());
        for i in 0..common {
            assert!((whole[i] - pieces[i]).abs() <= 1e-5, "sample {i}: {} vs {}", whole[i], pieces[i]);
        }
    }

    /// AUD-V2: alternating silent and sounding packets keep the ring on its stamps.
    #[test]
    fn silent_and_sounding_packets_land_where_their_stamps_say() {
        for rate in [44_100u32, 96_000] {
            let mut resampler = Resampler::new(rate);
            let ring = SampleRing::new(48_000, 2);
            let frames = (rate / 100) as usize;
            let loud = vec![0.5f32; frames * CHANNELS];
            for k in 0..1_000i64 {
                let mut out = Vec::new();
                if k % 2 == 0 {
                    resampler.silence(frames, &mut out);
                } else {
                    resampler.process(&loud, &mut out);
                }
                ring.write(&out, k * 100_000);
            }
            let end = ring.end_100ns().unwrap();
            assert!((end - 100_000_000).abs() < 10_000, "{rate} Hz: ring ends at {end}");
        }
    }

    #[test]
    fn a_48k_device_passes_through_untouched() {
        let mut resampler = Resampler::new(48_000);
        let x: Vec<f32> = (0..20).map(|i| i as f32 * 0.01).collect();
        let mut out = Vec::new();
        resampler.process(&x, &mut out);
        assert_eq!(out, x);
    }
}
