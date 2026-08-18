//! Ringpuffer je Audioquelle: entkoppelt den WASAPI-Thread vom Mixer und hält
//! nebenbei den Spitzenpegel für die Anzeige.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::Mutex;

/// Kapazität des Rings. Solange nicht aufgenommen wird, holt niemand die
/// Samples ab — der Ring steht dann dauerhaft am Anschlag. Deshalb ist er
/// bewusst klein: er kostet nichts und begrenzt, wie alt der Ton höchstens
/// sein kann, wenn eine Aufnahme startet.
const CAPACITY_MS: usize = 400;

pub struct SampleRing {
    samples: Mutex<VecDeque<f32>>,
    capacity: usize,
    /// Spitzenpegel als f32-Bits, damit die UI ihn ohne Lock lesen kann.
    peak: AtomicU32,
    dropped: AtomicU32,
}

impl SampleRing {
    pub fn new(sample_rate: u32, channels: usize) -> Self {
        let capacity = sample_rate as usize * channels * CAPACITY_MS / 1000;
        Self {
            samples: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
            peak: AtomicU32::new(0),
            dropped: AtomicU32::new(0),
        }
    }

    pub fn write(&self, block: &[f32]) {
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
        // Höchsten Wert seit dem letzten Auslesen behalten.
        let previous = f32::from_bits(self.peak.load(Ordering::Relaxed));
        if peak > previous {
            self.peak.store(peak.to_bits(), Ordering::Relaxed);
        }

        let mut samples = self.samples.lock();
        samples.extend(block.iter().copied());
        if samples.len() > self.capacity {
            let excess = samples.len() - self.capacity;
            samples.drain(..excess);
            self.dropped.fetch_add(excess as u32, Ordering::Relaxed);
        }
    }

    /// Genau `count` Samples abholen; fehlende werden mit Stille aufgefüllt,
    /// damit die Spur nicht kürzer wird als das Video.
    pub fn read_exact(&self, count: usize) -> Vec<f32> {
        let mut out = vec![0.0; count];
        self.read_into(&mut out);
        out
    }

    /// Wie `read_exact`, aber ohne Allokation — der Aufrufer bringt den Puffer
    /// mit. Läuft pro Videobild, deshalb zählt hier jede Allokation.
    pub fn read_into(&self, out: &mut [f32]) {
        let mut samples = self.samples.lock();
        let take = out.len().min(samples.len());
        for (slot, sample) in out[..take].iter_mut().zip(samples.drain(..take)) {
            *slot = sample;
        }
        out[take..].fill(0.0);
    }

    /// Rückstand über `max` Samples verwerfen.
    ///
    /// Aufnahme- und Abholrate sind beide nominell 48 kHz, laufen aber auf
    /// verschiedenen Uhren — der Füllstand wandert deshalb langsam. Ohne diese
    /// Bremse sammelt sich der Unterschied bis zum Anschlag und der Ton bleibt
    /// dauerhaft hinter dem Bild zurück.
    pub fn trim_to(&self, max: usize) {
        let mut samples = self.samples.lock();
        if samples.len() > max {
            let excess = samples.len() - max;
            samples.drain(..excess);
            self.dropped.fetch_add(excess as u32, Ordering::Relaxed);
        }
    }

    pub fn available(&self) -> usize {
        self.samples.lock().len()
    }

    /// Spitzenpegel seit dem letzten Aufruf (0.0–1.0), danach zurückgesetzt.
    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.peak.swap(0, Ordering::Relaxed))
    }

    pub fn clear(&self) {
        self.samples.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_is_reported_once_and_then_reset() {
        let ring = SampleRing::new(48_000, 2);
        ring.write(&[0.1, -0.7, 0.3, 0.2]);
        assert!((ring.take_peak() - 0.7).abs() < 1e-6);
        assert_eq!(ring.take_peak(), 0.0);
    }

    #[test]
    fn read_exact_pads_with_silence() {
        let ring = SampleRing::new(48_000, 2);
        ring.write(&[1.0, 1.0]);
        let block = ring.read_exact(6);
        assert_eq!(block.len(), 6);
        assert_eq!(&block[..2], &[1.0, 1.0]);
        assert!(block[2..].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn oldest_samples_are_dropped_when_the_mixer_stalls() {
        // 1000 Hz mono, 400 ms Kapazität -> 400 Samples
        let ring = SampleRing::new(1000, 1);
        for _ in 0..600 {
            ring.write(&[0.5]);
        }
        assert_eq!(ring.available(), 400);
    }

    #[test]
    fn read_into_reuses_the_buffer_and_pads() {
        let ring = SampleRing::new(48_000, 2);
        ring.write(&[1.0, 1.0]);
        let mut buf = vec![9.0; 4];
        ring.read_into(&mut buf);
        assert_eq!(buf, vec![1.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn trim_to_drops_the_oldest_backlog() {
        let ring = SampleRing::new(48_000, 2);
        ring.write(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        ring.trim_to(2);
        assert_eq!(ring.available(), 2);
        assert_eq!(ring.read_exact(2), vec![4.0, 5.0]);
    }

    #[test]
    fn clear_empties_the_ring() {
        let ring = SampleRing::new(48_000, 2);
        ring.write(&[1.0, 2.0, 3.0]);
        ring.clear();
        assert_eq!(ring.available(), 0);
    }
}
