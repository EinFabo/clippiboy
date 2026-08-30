//! Holds the running source streams, supplies levels for the UI and mixes the
//! tracks for the encoder.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::audio::capture::{self, StreamHandle, CHANNELS, SAMPLE_RATE};
use crate::audio::ring::SampleRing;
use crate::audio::{gain_factor, TrackLayout};
use crate::model::{AudioSource, SourceKind};

struct Running {
    handle: Option<StreamHandle>,
    ring: Arc<SampleRing>,
    /// Detects whether the source itself changed (different device/PID).
    fingerprint: String,
}

#[derive(Default)]
pub struct AudioEngine {
    running: Mutex<HashMap<String, Running>>,
    errors: Mutex<HashMap<String, String>>,
    /// Is an off-thread `apply` running right now? On failure `capture::start`
    /// waits up to five seconds — that must not stack up.
    applying: std::sync::atomic::AtomicBool,
}

fn fingerprint(kind: &SourceKind) -> String {
    match kind {
        SourceKind::InputDevice { device_id } => format!("in:{device_id}"),
        SourceKind::OutputDevice { device_id, exclude_game } => {
            // Without the flag the same string as before, so an existing
            // endpoint source is not restarted by this change alone.
            match exclude_game {
                true => format!("out:{device_id}:without-game"),
                false => format!("out:{device_id}"),
            }
        }
        SourceKind::Process { pid, mode } => format!("proc:{pid}:{mode:?}"),
        // Never reached after `resolve` — see `capture::start`.
        SourceKind::Game => "game:unresolved".into(),
    }
}

impl AudioEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Brings the running streams in line with the config: start new sources,
    /// stop removed or changed ones. Gain, mute and solo change nothing about the
    /// streams — those only take effect while mixing.
    ///
    /// Starting happens **without** the lock on `running`. `capture::start` waits
    /// up to five seconds on a device that does not answer — and all that while
    /// every level meter would stand still, because `levels` needs the same lock.
    /// On a one-off change that would barely show; on the regular retry (see
    /// [`Self::retry_failed`]) it very much would.
    pub fn apply(&self, sources: &[AudioSource]) {
        let wanted: Vec<&AudioSource> = sources.iter().filter(|s| s.enabled).collect();

        // Collect removed or changed streams …
        let stale: Vec<(String, Running)> = {
            let mut running = self.running.lock();
            let ids: Vec<String> = running
                .iter()
                .filter(|(id, run)| {
                    match wanted.iter().find(|source| &source.id == *id) {
                        Some(source) => fingerprint(&source.kind) != run.fingerprint,
                        None => true,
                    }
                })
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| running.remove(&id).map(|run| (id, run)))
                .collect()
        };
        // … and let them drain outside the lock: `stop` waits on the source's
        // thread.
        for (id, mut run) in stale {
            if let Some(handle) = run.handle.take() {
                handle.stop();
            }
            self.errors.lock().remove(&id);
        }

        let missing: Vec<&AudioSource> = {
            let running = self.running.lock();
            wanted
                .into_iter()
                .filter(|source| !running.contains_key(&source.id))
                .collect()
        };
        for source in missing {
            let ring = Arc::new(SampleRing::new(SAMPLE_RATE, CHANNELS));
            match capture::start(&source.kind, ring.clone()) {
                Ok(handle) => {
                    let mut running = self.running.lock();
                    // In the meantime a second call may have started the same
                    // source. Then theirs stands and this one is cleared away
                    // again — two streams on the same device would otherwise both
                    // write into the same ring.
                    if running.contains_key(&source.id) {
                        drop(running);
                        handle.stop();
                        continue;
                    }
                    running.insert(
                        source.id.clone(),
                        Running {
                            handle: Some(handle),
                            ring,
                            fingerprint: fingerprint(&source.kind),
                        },
                    );
                    drop(running);
                    self.errors.lock().remove(&source.id);
                }
                Err(err) => {
                    log::warn!("could not start source '{}': {err}", source.label);
                    self.errors.lock().insert(source.id.clone(), err);
                }
            }
        }
    }

    /// [`Self::apply`] on a thread of its own.
    ///
    /// Everything called from the status tick has to go through here. Stopping a
    /// source joins its thread (up to 200 ms), and starting one waits on a
    /// device that does not answer (up to five seconds) — on the tick thread all
    /// level meters would stand still for that whole time, because they need the
    /// same lock.
    ///
    /// A second call while one is running is dropped: the caller repeats every
    /// two seconds anyway, and two `apply`s in parallel would fight over the
    /// same sources.
    pub fn apply_async(self: &Arc<Self>, sources: Vec<AudioSource>) {
        if self.applying.swap(true, Ordering::SeqCst) {
            return;
        }
        let engine = self.clone();
        std::thread::spawn(move || {
            engine.apply(&sources);
            engine.applying.store(false, Ordering::SeqCst);
        });
    }

    /// Try sources that failed to start last time once more.
    ///
    /// A device that was busy or has just been plugged in is often there a few
    /// seconds later. Without this the source would stay dead until the next
    /// program start — and the track in the clip silent, without anyone noticing.
    pub fn retry_failed(self: &Arc<Self>, sources: Vec<AudioSource>) {
        if self.errors.lock().is_empty() {
            return;
        }
        self.apply_async(sources);
    }

    pub fn stop_all(&self) {
        let mut running = self.running.lock();
        for (_, mut run) in running.drain() {
            if let Some(handle) = run.handle.take() {
                handle.stop();
            }
        }
    }

    /// Peak level per source (0.0–1.0), with gain and mute taken into account.
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

    /// Notices that are not errors: the source runs, but not the way the user
    /// expects.
    pub fn warnings(&self, sources: &[AudioSource]) -> HashMap<String, String> {
        let running = self.running.lock();
        let mut out = HashMap::new();
        for source in sources {
            let uses_fallback = running
                .get(&source.id)
                .and_then(|run| run.handle.as_ref())
                .is_some_and(|handle| handle.fallback_clock.load(Ordering::Relaxed));
            if uses_fallback {
                out.insert(
                    source.id.clone(),
                    "This device reports unusable timestamps — ClippiBoy carries \
                     on with the system clock."
                        .to_string(),
                );
            }
        }
        out
    }

    /// Empty all rings. Has to happen before every recording start: while nothing
    /// is being recorded the WASAPI threads keep writing but nobody collects — the
    /// rings then stand at their limit and the audio would be a full ring length
    /// behind the picture from the very first second.
    pub fn reset_rings(&self) {
        for run in self.running.lock().values() {
            run.ring.clear();
        }
    }

    /// Mixes the window starting at `from_100ns` over `frames` frames into `out`:
    /// index 0 is the main mix, then the sources with their own track (in layout
    /// order).
    ///
    /// Reading is explicitly **by time**, not "whatever is next". Previously each
    /// source took from the front of its ring, and how much was worked out from
    /// the wall clock — sources with slightly different device clocks therefore
    /// drifted against each other and against the picture.
    ///
    /// Writes into buffers handed in rather than allocating new ones: this runs on
    /// a millisecond tick.
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
        // Only needed when several sources are summed into the same mix.
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
                    // The first source may write straight into the target buffer.
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
            // Summing can overshoot — hard clipping here is better than a crack
            // from wrap-around at the encoder.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProcessMode;

    /// The whole "follow the game" mechanism rests on this: `apply` restarts a
    /// source exactly when its fingerprint changes. A new PID therefore has to
    /// look different — and so does a flipped loopback mode.
    #[test]
    fn a_different_process_gives_a_different_fingerprint() {
        let include = |pid| {
            fingerprint(&SourceKind::Process {
                pid,
                mode: ProcessMode::Include,
            })
        };
        assert_ne!(include(100), include(200));
        assert_ne!(
            include(100),
            fingerprint(&SourceKind::Process {
                pid: 100,
                mode: ProcessMode::Exclude,
            })
        );
    }

    /// Existing endpoint sources must not be restarted just because the field
    /// was added.
    #[test]
    fn an_untouched_endpoint_keeps_its_fingerprint() {
        let plain = fingerprint(&SourceKind::OutputDevice {
            device_id: "spk".into(),
            exclude_game: false,
        });
        assert_eq!(plain, "out:spk");
        assert_ne!(
            plain,
            fingerprint(&SourceKind::OutputDevice {
                device_id: "spk".into(),
                exclude_game: true,
            })
        );
    }
}
