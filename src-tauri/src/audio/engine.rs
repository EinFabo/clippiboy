//! Hält die laufenden Quellen-Streams, liefert Pegel für die UI und mischt die
//! Spuren für den Encoder.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::audio::capture::{self, StreamHandle, CHANNELS, SAMPLE_RATE};
use crate::audio::ring::SampleRing;
use crate::audio::{gain_factor, TrackLayout};
use crate::model::{AudioSource, SourceKind};

struct Running {
    handle: Option<StreamHandle>,
    ring: Arc<SampleRing>,
    /// Erkennt, ob sich die Quelle inhaltlich geändert hat (anderes Gerät/PID).
    fingerprint: String,
}

#[derive(Default)]
pub struct AudioEngine {
    running: Mutex<HashMap<String, Running>>,
    errors: Mutex<HashMap<String, String>>,
}

fn fingerprint(kind: &SourceKind) -> String {
    match kind {
        SourceKind::InputDevice { device_id } => format!("in:{device_id}"),
        SourceKind::OutputDevice { device_id } => format!("out:{device_id}"),
        SourceKind::Process { pid, mode } => format!("proc:{pid}:{mode:?}"),
    }
}

impl AudioEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bringt die laufenden Streams mit der Konfiguration in Deckung: neue
    /// Quellen starten, entfernte oder geänderte stoppen. Gain, Mute und Solo
    /// ändern nichts an den Streams — die wirken erst beim Mischen.
    pub fn apply(&self, sources: &[AudioSource]) {
        let mut running = self.running.lock();
        let mut errors = self.errors.lock();

        let wanted: HashMap<&str, &AudioSource> = sources
            .iter()
            .filter(|s| s.enabled)
            .map(|s| (s.id.as_str(), s))
            .collect();

        // Entfernte oder geänderte Streams stoppen.
        let stale: Vec<String> = running
            .iter()
            .filter(|(id, run)| match wanted.get(id.as_str()) {
                Some(source) => fingerprint(&source.kind) != run.fingerprint,
                None => true,
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            if let Some(mut run) = running.remove(&id) {
                if let Some(handle) = run.handle.take() {
                    handle.stop();
                }
            }
            errors.remove(&id);
        }

        // Fehlende starten.
        for (id, source) in wanted {
            if running.contains_key(id) {
                continue;
            }
            let ring = Arc::new(SampleRing::new(SAMPLE_RATE, CHANNELS));
            match capture::start(&source.kind, ring.clone()) {
                Ok(handle) => {
                    errors.remove(id);
                    running.insert(
                        id.to_string(),
                        Running {
                            handle: Some(handle),
                            ring,
                            fingerprint: fingerprint(&source.kind),
                        },
                    );
                }
                Err(err) => {
                    log::warn!("Quelle '{}' konnte nicht gestartet werden: {err}", source.label);
                    errors.insert(id.to_string(), err);
                }
            }
        }
    }

    pub fn stop_all(&self) {
        let mut running = self.running.lock();
        for (_, mut run) in running.drain() {
            if let Some(handle) = run.handle.take() {
                handle.stop();
            }
        }
    }

    /// Spitzenpegel je Quelle (0.0–1.0), Gain und Mute berücksichtigt.
    pub fn levels(&self, sources: &[AudioSource]) -> HashMap<String, f32> {
        let running = self.running.lock();
        let mut out = HashMap::with_capacity(sources.len());
        for source in sources {
            let level = match running.get(&source.id) {
                Some(run) if !source.muted => {
                    (run.ring.take_peak() * gain_factor(source.gain_db)).min(1.0)
                }
                _ => 0.0,
            };
            out.insert(source.id.clone(), level);
        }
        out
    }

    pub fn errors(&self) -> HashMap<String, String> {
        self.errors.lock().clone()
    }

    /// Alle Ringe leeren. Muss vor jedem Aufnahmestart passieren: solange nicht
    /// aufgenommen wird, schreiben die WASAPI-Threads weiter, aber niemand holt
    /// ab — die Ringe stehen dann am Anschlag und der Ton wäre von der ersten
    /// Sekunde an um die volle Ringlänge hinter dem Bild.
    pub fn reset_rings(&self) {
        for run in self.running.lock().values() {
            run.ring.clear();
        }
    }

    /// Bis wohin auf der QPC-Zeitachse alle laufenden Quellen Material haben.
    ///
    /// Der Mischer darf nur bis hierhin arbeiten: Was er einmal erzeugt hat,
    /// ist geschrieben — käme der Ton einer Quelle danach noch an, wäre sein
    /// Platz schon vergeben.
    pub fn ready_until_100ns(&self) -> Option<i64> {
        let running = self.running.lock();
        running
            .values()
            .filter_map(|run| run.ring.end_100ns())
            .min()
    }

    /// Mischt das Zeitfenster ab `from_100ns` über `frames` Frames nach `out`:
    /// Index 0 ist der Hauptmix, danach die Quellen mit eigener Spur
    /// (Reihenfolge wie im Layout).
    ///
    /// Es wird ausdrücklich **nach Zeit** gelesen, nicht „das Nächste". Vorher
    /// nahm jede Quelle vorne von ihrem Ring weg, und wie viel, ergab sich aus
    /// der Wanduhr — Quellen mit leicht unterschiedlichem Gerätetakt drifteten
    /// dadurch gegeneinander und gegen das Bild.
    ///
    /// Schreibt in mitgebrachte Puffer statt neue anzulegen: Das hier läuft im
    /// Millisekundentakt.
    pub fn mix_window(
        &self,
        sources: &[AudioSource],
        layout: &TrackLayout,
        from_100ns: i64,
        frames: usize,
        out: &mut Vec<Vec<f32>>,
    ) {
        let sample_count = frames * CHANNELS;
        let track_count = layout.track_count();

        out.resize_with(track_count, Vec::new);
        for track in out.iter_mut() {
            track.clear();
            track.resize(sample_count, 0.0);
        }

        let running = self.running.lock();
        let find = |id: &str| sources.iter().find(|s| s.id == id);
        // Nur nötig, wenn mehrere Quellen in denselben Mix summiert werden.
        let mut scratch: Vec<f32> = Vec::new();
        let mut index = 0;

        if !layout.main_mix.is_empty() {
            let mix = &mut out[0];
            let mut first = true;
            for id in &layout.main_mix {
                let (Some(run), Some(source)) = (running.get(id), find(id)) else {
                    continue;
                };
                let gain = gain_factor(source.gain_db);
                if first {
                    // Die erste Quelle darf direkt in den Zielpuffer schreiben.
                    run.ring.read_window(from_100ns, mix);
                    for sample in mix.iter_mut() {
                        *sample *= gain;
                    }
                    first = false;
                    continue;
                }
                if scratch.len() != sample_count {
                    scratch.resize(sample_count, 0.0);
                }
                run.ring.read_window(from_100ns, &mut scratch);
                for (target, sample) in mix.iter_mut().zip(scratch.iter()) {
                    *target += sample * gain;
                }
            }
            // Summieren kann übersteuern — hart begrenzen ist hier besser als
            // ein Knacken durch Wrap-around beim Encoder.
            for sample in mix.iter_mut() {
                *sample = sample.clamp(-1.0, 1.0);
            }
            index = 1;
        }

        for id in &layout.separate {
            let track = &mut out[index];
            index += 1;
            let (Some(run), Some(source)) = (running.get(id), find(id)) else {
                continue;
            };
            let gain = gain_factor(source.gain_db);
            run.ring.read_window(from_100ns, track);
            for sample in track.iter_mut() {
                *sample = (*sample * gain).clamp(-1.0, 1.0);
            }
        }
    }
}
