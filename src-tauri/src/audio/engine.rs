//! Holds the running source streams, supplies levels for the UI and mixes the
//! tracks for the encoder.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::audio::capture::{self, StreamHandle, CHANNELS, SAMPLE_RATE};
use crate::audio::ring::SampleRing;
use crate::audio::{audible, gain_factor, recorded, Stream};
use crate::model::{AudioSource, SourceKind};

struct Running {
    handle: Option<StreamHandle>,
    ring: Arc<SampleRing>,
    /// Which mixer source this stream feeds. Several streams can share one —
    /// the leftovers track is one WASAPI client per application.
    source_id: String,
    /// The process being tapped, for [`AudioEngine::taps`]. `None` for device
    /// streams.
    pid: Option<u32>,
}

#[derive(Default)]
pub struct AudioEngine {
    /// Keyed by [`stream_key`], not by source: a source can own several streams.
    running: Mutex<HashMap<String, Running>>,
    errors: Mutex<HashMap<String, String>>,
    /// The stream set of the last `apply`, so the two-second tick can tell an
    /// unchanged situation from a real change without touching the streams.
    wanted: Mutex<Vec<String>>,
    /// Is an off-thread `apply` running right now? On failure `capture::start`
    /// waits up to five seconds — that must not stack up.
    applying: std::sync::atomic::AtomicBool,
}

fn fingerprint(kind: &SourceKind) -> String {
    match kind {
        SourceKind::InputDevice { device_id } => format!("in:{device_id}"),
        SourceKind::OutputDevice { device_id, .. } => format!("out:{device_id}"),
        SourceKind::Process { pid, mode } => format!("proc:{pid}:{mode:?}"),
        // Never reached after `resolve` — see `capture::start`.
        SourceKind::Game | SourceKind::Leftovers => "unresolved".into(),
    }
}

/// Identifies one running stream. Because the kind is part of it, a source that
/// changed — a new PID, another device — automatically counts as a different
/// stream, and `apply` stops the old one and starts the new one.
fn stream_key(stream: &Stream) -> String {
    format!("{}#{}", stream.source_id, fingerprint(&stream.kind))
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
    pub fn apply(&self, streams: &[Stream]) {
        let wanted: Vec<(String, &Stream)> = streams
            .iter()
            .map(|stream| (stream_key(stream), stream))
            .collect();

        // Collect streams that are no longer wanted …
        let stale: Vec<Running> = {
            let mut running = self.running.lock();
            let keys: Vec<String> = running
                .keys()
                .filter(|key| !wanted.iter().any(|(want, _)| want == *key))
                .cloned()
                .collect();
            keys.into_iter()
                .filter_map(|key| running.remove(&key))
                .collect()
        };
        // … and let them drain outside the lock: `stop` waits on the source's
        // thread.
        for mut run in stale {
            if let Some(handle) = run.handle.take() {
                handle.stop();
            }
        }

        let missing: Vec<(String, &Stream)> = {
            let running = self.running.lock();
            wanted
                .iter()
                .filter(|(key, _)| !running.contains_key(key))
                .map(|(key, stream)| (key.clone(), *stream))
                .collect()
        };
        // Rebuilt from scratch: a source whose stream now runs must lose its old
        // error, and one that still fails gets it back in the same pass.
        let mut failures: HashMap<String, String> = HashMap::new();
        for (key, stream) in missing {
            let ring = Arc::new(SampleRing::new(SAMPLE_RATE, CHANNELS));
            match capture::start(&stream.kind, ring.clone()) {
                Ok(handle) => {
                    let mut running = self.running.lock();
                    // In the meantime a second call may have started the same
                    // stream. Then theirs stands and this one is cleared away
                    // again — two clients on the same device would otherwise both
                    // write into the same ring.
                    if running.contains_key(&key) {
                        drop(running);
                        handle.stop();
                        continue;
                    }
                    running.insert(
                        key,
                        Running {
                            handle: Some(handle),
                            ring,
                            source_id: stream.source_id.clone(),
                            pid: match stream.kind {
                                SourceKind::Process { pid, .. } => Some(pid),
                                _ => None,
                            },
                        },
                    );
                }
                Err(err) => {
                    log::warn!("could not start a stream of '{}': {err}", stream.source_id);
                    failures.insert(stream.source_id.clone(), err);
                }
            }
        }
        *self.errors.lock() = failures;
        // Sorted, because session enumeration does not promise an order and the
        // comparison in `refresh` would otherwise see a change in every
        // reshuffle.
        let mut keys: Vec<String> = wanted.into_iter().map(|(key, _)| key).collect();
        keys.sort();
        // Worth a line in the log every time it changes: "what is actually in
        // the leftovers" is the question this whole part keeps raising, and
        // without a device to point at there is nothing else to look at.
        log::info!("audio streams: {}", keys.join(" "));
        *self.wanted.lock() = keys;
    }

    /// Ask Windows who is playing, work out the streams and bring them up —
    /// all on a thread of its own, and only when something actually changed.
    ///
    /// The enumeration belongs here rather than at the call site for two
    /// reasons. It is COM work: doing it on the main thread puts that thread into
    /// the multithreaded apartment, and Tauri's window layer then fails to start
    /// with `RPC_E_CHANGED_MODE`. And it is a round trip per output device — on a
    /// machine with a hardware mixer that is nine of them, every two seconds, on
    /// the same thread that ships the level meters.
    ///
    /// The comparison against the last stream set is what keeps the two-second
    /// tick cheap: without it a source that cannot start would be retried every
    /// two seconds instead of every ten (see [`Self::retry_failed`]), each
    /// attempt blocking for the full five-second timeout.
    pub fn refresh_async(self: &Arc<Self>, sources: Vec<AudioSource>, game_pid: Option<u32>) {
        self.refresh(sources, game_pid, false);
    }

    /// Try sources that failed to start last time once more.
    ///
    /// A device that was busy or has just been plugged in is often there a few
    /// seconds later. Without this the source would stay dead until the next
    /// program start — and the track in the clip silent, without anyone noticing.
    /// This is the one caller that works past the unchanged-set shortcut, because
    /// a failed stream looks exactly like an unchanged one.
    pub fn retry_failed(self: &Arc<Self>, sources: Vec<AudioSource>, game_pid: Option<u32>) {
        if self.errors.lock().is_empty() {
            return;
        }
        self.refresh(sources, game_pid, true);
    }

    fn refresh(self: &Arc<Self>, sources: Vec<AudioSource>, game_pid: Option<u32>, force: bool) {
        // A second call while one is running is dropped: every caller repeats
        // within two seconds, and two `apply`s in parallel would fight over the
        // same streams.
        if self.applying.swap(true, Ordering::SeqCst) {
            return;
        }
        let engine = self.clone();
        std::thread::spawn(move || {
            let ledger = crate::audio::Leftovers {
                default_device: crate::audio::devices::default_output_id(),
                previous: engine.assignments(),
            };
            let streams = crate::audio::resolve(
                &sources,
                game_pid,
                &crate::audio::sessions_by_device(&sources),
                &crate::audio::devices::process_tree(),
                &ledger,
            );
            let mut keys: Vec<String> = streams.iter().map(stream_key).collect();
            keys.sort();
            if force || *engine.wanted.lock() != keys {
                engine.apply(&streams);
            }
            engine.applying.store(false, Ordering::SeqCst);
        });
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
    ///
    /// The loudest of a source's streams wins — the meter shows a level, not a
    /// sum. Every ring is read even for a muted source, because `take_peak`
    /// resets: skipping it would make the meter jump on unmute.
    pub fn levels(&self, sources: &[AudioSource]) -> HashMap<String, f32> {
        let running = self.running.lock();
        let mut peaks: HashMap<&str, f32> = HashMap::new();
        for run in running.values() {
            let peak = run.ring.take_peak();
            let slot = peaks.entry(run.source_id.as_str()).or_insert(0.0);
            *slot = slot.max(peak);
        }

        let mut out = HashMap::with_capacity(sources.len());
        for source in sources {
            let level = match peaks.get(source.id.as_str()) {
                Some(peak) if !source.muted => (peak * gain_factor(source.gain_db)).min(1.0),
                _ => 0.0,
            };
            out.insert(source.id.clone(), level);
        }
        out
    }

    pub fn errors(&self) -> HashMap<String, String> {
        self.errors.lock().clone()
    }

    /// Which source is tapping which process, the other way round from
    /// [`Self::taps`]. Feeds `Leftovers::previous` so an application that falls
    /// silent for a moment is not moved to another source and back.
    pub fn assignments(&self) -> HashMap<u32, String> {
        let running = self.running.lock();
        running
            .values()
            .filter_map(|run| Some((run.pid?, run.source_id.clone())))
            .collect()
    }

    /// Which processes each source is tapping right now.
    ///
    /// For the leftovers there is no device to look at and no list to compare
    /// against — this is the only way to see whether something is missing from
    /// the track or sitting in it twice. No process names here: turning a PID
    /// into a name is COM work, and the UI has that list already.
    pub fn taps(&self) -> HashMap<String, Vec<u32>> {
        let running = self.running.lock();
        let mut out: HashMap<String, Vec<u32>> = HashMap::new();
        for run in running.values() {
            if let Some(pid) = run.pid {
                out.entry(run.source_id.clone()).or_default().push(pid);
            }
        }
        for pids in out.values_mut() {
            pids.sort();
        }
        out
    }

    /// Notices that are not errors: the source runs, but not the way the user
    /// expects.
    pub fn warnings(&self, sources: &[AudioSource]) -> HashMap<String, String> {
        let running = self.running.lock();
        let mut out = HashMap::new();
        for source in sources {
            let uses_fallback = running.values().any(|run| {
                run.source_id == source.id
                    && run
                        .handle
                        .as_ref()
                        .is_some_and(|handle| handle.fallback_clock.load(Ordering::Relaxed))
            });
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

    /// Mixes the window starting at `from_100ns` over `frames` frames into `out`
    /// — **one buffer per recorded source**, in the order they sit in the mixer
    /// (see [`recorded`]). A source that cannot be heard right now gets silence
    /// rather than being left out: it keeps its place, and its track keeps the
    /// minutes already in it.
    ///
    /// No main mix is drawn here. Which of these buffers are summed into one is
    /// decided when a clip is saved, so the assignment can still be changed
    /// while the buffer runs.
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
        from_100ns: i64,
        frames: usize,
        out: &mut Vec<Vec<f32>>,
    ) {
        let sample_count = frames * CHANNELS;

        out.resize_with(recorded(sources).count(), Vec::new);
        for track in out.iter_mut() {
            track.clear();
            track.resize(sample_count, 0.0);
        }

        let running = self.running.lock();
        // A source can own several streams, so every read is a sum — the buffers
        // above start at zero.
        let mut scratch: Vec<f32> = Vec::new();

        for (track, source) in out.iter_mut().zip(recorded(sources)) {
            if !audible(sources, source) {
                continue;
            }
            add_streams(
                &running,
                &source.id,
                from_100ns,
                gain_factor(source.gain_db),
                track,
                &mut scratch,
            );
            // Several streams can overshoot together — hard clipping here is
            // better than a crack from wrap-around at the encoder.
            for sample in track.iter_mut() {
                *sample = sample.clamp(-1.0, 1.0);
            }
        }
    }
}

/// Adds every stream belonging to `source_id` into `target`, scaled by `gain`.
fn add_streams(
    running: &HashMap<String, Running>,
    source_id: &str,
    from_100ns: i64,
    gain: f32,
    target: &mut [f32],
    scratch: &mut Vec<f32>,
) {
    if scratch.len() != target.len() {
        scratch.clear();
        scratch.resize(target.len(), 0.0);
    }
    for run in running.values().filter(|run| run.source_id == source_id) {
        run.ring.read_window(from_100ns, scratch);
        for (out, sample) in target.iter_mut().zip(scratch.iter()) {
            *out += sample * gain;
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

    /// An endpoint stream is identified by its device.
    #[test]
    fn an_endpoint_is_identified_by_its_device() {
        let endpoint = |device_id: &str| {
            fingerprint(&SourceKind::OutputDevice {
                device_id: device_id.into(),
            })
        };
        assert_eq!(endpoint("spk"), "out:spk");
        assert_ne!(endpoint("spk"), endpoint("headset"));
    }

    /// Two sources of the same kind stay apart, and one source can hold several
    /// streams — that is what carries the leftovers track.
    #[test]
    fn a_stream_is_keyed_by_source_and_kind() {
        let stream = |source_id: &str, pid| Stream {
            source_id: source_id.into(),
            kind: SourceKind::Process {
                pid,
                mode: ProcessMode::Include,
            },
        };
        assert_ne!(stream_key(&stream("rest", 1)), stream_key(&stream("rest", 2)));
        assert_ne!(stream_key(&stream("rest", 1)), stream_key(&stream("game", 1)));
        assert_eq!(stream_key(&stream("rest", 1)), stream_key(&stream("rest", 1)));
    }
}
