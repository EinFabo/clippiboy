//! Ringpuffer je Audioquelle — mit Zeitachse.
//!
//! Vorher lag hier nur eine Schlange von Samples, und wer las, nahm einfach
//! vorne weg. Wie viel gelesen wurde, rechnete die Aufnahme aus der Wanduhr
//! aus (`Instant::now()`). Der WASAPI-Clock läuft aber nicht ganz so schnell
//! wie die Wanduhr — der Unterschied sammelte sich, musste mit `trim_to`
//! weggeworfen werden, und genau das hörte man als Aussetzer und als
//! langsam davonlaufenden Ton.
//!
//! Jetzt trägt jeder geschriebene Block den QPC-Zeitstempel, den WASAPI
//! ohnehin mitliefert (`pu64QPCPosition`). Gelesen wird nicht mehr „das
//! Nächste", sondern **ein Zeitfenster**. Das ist dieselbe Uhr, auf der auch
//! `Direct3D11CaptureFrame::SystemRelativeTime` liegt — Bild und Ton sind
//! damit von sich aus synchron, ohne Korrektur.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::Mutex;

/// Wie viel Ton der Ring vorhält. Der Mischer holt alle paar Millisekunden ab;
/// eine Sekunde ist reichlich Luft für einen kurzen Hänger und kostet je
/// Quelle nur ein paar hundert Kilobyte.
const CAPACITY_MS: usize = 1000;

/// Ab welcher Abweichung ein Block als Sprung in der Zeitachse gilt.
const GAP_TOLERANCE_100NS: i64 = 50 * 10_000;

/// So viel Stille wird höchstens eingefügt, um eine Lücke zu überbrücken.
/// Darüber hinaus war die Quelle schlicht weg — dann fängt die Zeitachse neu
/// an, statt den Ring mit Stille vollzuschreiben.
const MAX_GAP_FILL_100NS: i64 = 1_000 * 10_000;

struct Inner {
    samples: VecDeque<f32>,
    /// QPC (100 ns) des ersten Samples in `samples`.
    start_100ns: i64,
    primed: bool,
}

pub struct SampleRing {
    inner: Mutex<Inner>,
    sample_rate: u32,
    channels: usize,
    capacity: usize,
    /// Spitzenpegel als f32-Bits, damit die UI ihn ohne Lock lesen kann.
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

    /// Einen Block mit seinem QPC-Zeitstempel einlegen.
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
            // Wo würde dieser Block liegen, wenn nichts gefehlt hätte?
            let held = inner.samples.len() / self.channels;
            let expected = inner.start_100ns + self.frames_to_100ns(held);
            let drift = qpc_100ns - expected;

            if drift > GAP_TOLERANCE_100NS {
                if drift <= MAX_GAP_FILL_100NS {
                    // Echte Lücke (Gerät hat ausgesetzt): mit Stille auffüllen,
                    // damit alles danach an seiner richtigen Stelle bleibt.
                    let missing = self.duration_to_frames(drift) as usize * self.channels;
                    inner.samples.extend(std::iter::repeat(0.0).take(missing));
                } else {
                    // Die Quelle war lange weg. Die Zeitachse neu ansetzen.
                    inner.samples.clear();
                    inner.start_100ns = qpc_100ns;
                }
            }
            // Ein negativer Versatz (Block liegt vor dem Erwarteten) heißt
            // Überlappung. Die paar Samples doppelt zu schreiben ist harmloser,
            // als sie herauszurechnen.
        }

        inner.samples.extend(block.iter().copied());

        if inner.samples.len() > self.capacity {
            let excess = inner.samples.len() - self.capacity;
            inner.samples.drain(..excess);
            let dropped_frames = excess / self.channels;
            inner.start_100ns += self.frames_to_100ns(dropped_frames);
        }
    }

    /// Ein Zeitfenster auslesen: `out` wird vollständig gefüllt, fehlende
    /// Stellen mit Stille.
    ///
    /// Verbraucht nichts — der Ring wirft von selbst weg, was älter ist als
    /// seine Kapazität. Dadurch kann derselbe Abschnitt nicht je nach
    /// Lesereihenfolge einmal zu viel und einmal zu wenig geliefert werden,
    /// was beim alten „vorne wegnehmen" die Quellen gegeneinander verschob.
    pub fn read_window(&self, from_100ns: i64, out: &mut [f32]) {
        out.fill(0.0);

        let inner = self.inner.lock();
        if !inner.primed || inner.samples.is_empty() {
            return;
        }

        // Versatz des gewünschten Beginns gegenüber dem Ringanfang, in Samples.
        let offset_frames = self.duration_to_frames(from_100ns - inner.start_100ns);
        let offset = offset_frames * self.channels as i64;

        // `src` läuft über den Ring, `dst` über die Ausgabe. Liegt das Fenster
        // teilweise vor dem Ringanfang, beginnt `dst` entsprechend später.
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

    /// QPC des ersten Samples, das noch im Ring liegt.
    pub fn start_100ns(&self) -> Option<i64> {
        let inner = self.inner.lock();
        inner.primed.then_some(inner.start_100ns)
    }

    /// QPC hinter dem letzten Sample — bis hierhin gibt es Material.
    pub fn end_100ns(&self) -> Option<i64> {
        let inner = self.inner.lock();
        if !inner.primed {
            return None;
        }
        let held = inner.samples.len() / self.channels;
        Some(inner.start_100ns + self.frames_to_100ns(held))
    }

    /// Spitzenpegel seit dem letzten Aufruf (0.0–1.0), danach zurückgesetzt.
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

    /// Absichtlich nicht 48 kHz: Ein Sample dauert dort 208,33 QPC-Ticks und
    /// lässt sich in 100-ns-Auflösung nicht exakt ausdrücken. Für die
    /// Fensterlogik ist das gleichgültig (echte Blöcke sind hunderte Samples
    /// lang), für eine Prüfung auf einzelnes Sample genau aber tödlich —
    /// deshalb hier eine Rate, bei der ein Sample genau 1000 Ticks dauert.
    const RATE: u32 = 10_000;

    /// QPC (100 ns) für eine Sample-Position bei `RATE`.
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

        // Genau ab dem zweiten Sample.
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

    /// Setzt das Gerät kurz aus, darf alles Spätere nicht nach vorne rutschen —
    /// sonst liefe der Ton ab da vor dem Bild her.
    #[test]
    fn a_gap_is_filled_with_silence_so_later_audio_keeps_its_place() {
        let ring = SampleRing::new(RATE, 1);
        ring.write(&[1.0], at(0));
        // Nächster Block erst 100 ms später statt nach einem Sample.
        ring.write(&[2.0], at(0) + 100 * 10_000);

        let expected_position = ring.start_100ns().unwrap() + 100 * 10_000;
        let mut out = vec![0.0; 1];
        ring.read_window(expected_position, &mut out);
        assert_eq!(out, vec![2.0], "Der Block nach der Lücke steht falsch");
    }

    /// Eine Quelle, die minutenlang weg war, darf den Ring nicht mit Stille
    /// zuschütten.
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
        // 1000 Hz mono, 1000 ms Kapazität -> 1000 Samples
        let ring = SampleRing::new(1000, 1);
        for frame in 0..1500i64 {
            ring.write(&[0.5], frame * 10_000_000 / 1000);
        }
        let start = ring.start_100ns().unwrap();
        let end = ring.end_100ns().unwrap();
        assert_eq!((end - start) / 10_000, 1000, "Ring hält nicht 1000 ms");
    }

    #[test]
    fn clear_empties_the_ring() {
        let ring = SampleRing::new(RATE, 2);
        ring.write(&[1.0, 2.0], 0);
        ring.clear();
        assert_eq!(ring.start_100ns(), None);
    }
}
