//! Audio subsystem: device/process enumeration and (from phase 3) the per-source
//! WASAPI clients plus the mixer.

pub mod capture;
pub mod devices;
pub mod engine;
pub mod ring;

use crate::model::{AudioSource, ProcessMode, SourceKind};

/// One WASAPI client the engine has to run, and the mixer source it feeds.
///
/// Several streams can carry the same `source_id`: the leftovers track is one
/// process loopback per application, because Windows can only ever name a single
/// process per client.
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    pub source_id: String,
    pub kind: SourceKind,
}

/// Which processes the leftovers source has to be assembled from.
///
/// Empty when no enabled source asks for them — the session enumeration is a COM
/// round trip, and this runs on the two-second tick.
pub fn playing_now(sources: &[AudioSource]) -> Vec<u32> {
    sources
        .iter()
        .find_map(|source| match &source.kind {
            SourceKind::OutputDevice {
                device_id,
                leftovers_only: true,
            } if source.enabled => Some(devices::session_pids(device_id)),
            _ => None,
        })
        .unwrap_or_default()
}

/// Turns the stored, abstract sources into the streams the engine can open.
///
/// `playing` are the processes currently holding a session on the leftovers
/// source's output device.
///
/// Two things cannot be expressed in the configuration and are only decided
/// here:
///
/// * **The game.** Its PID would be dead after the first restart of the game, so
///   it is filled in from the live detection. Without a detected game the source
///   **drops out** rather than staying on disabled — only that way does
///   `AudioEngine::apply` stop its stream instead of reporting a source that
///   cannot start. In the configuration it stays enabled, keeps its `TrackRing`
///   and therefore its track; the mixer pushes silence into it until a game
///   turns up.
///
/// * **The leftovers.** Windows has no "everything except these three" tap:
///   `AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS` names exactly one process. So the
///   leftovers are not taken as one endpoint loopback but assembled from one tap
///   per application that no other source already records. Otherwise Discord
///   would sit in the clip twice — once on its own track and once inside the
///   main mix.
pub fn resolve(sources: &[AudioSource], game_pid: Option<u32>, playing: &[u32]) -> Vec<Stream> {
    let enabled = || sources.iter().filter(|s| s.enabled);

    // Everything another source already records, plus ClippiBoy itself: playing
    // a clip back while the buffer runs must not end up in the next one.
    let mut claimed: Vec<u32> = vec![std::process::id()];
    for source in enabled() {
        match &source.kind {
            SourceKind::Game => claimed.extend(game_pid),
            SourceKind::Process {
                pid,
                mode: ProcessMode::Include,
            } => claimed.push(*pid),
            _ => {}
        }
    }

    let mut out = Vec::with_capacity(sources.len());
    for source in enabled() {
        let include = |pid: u32| Stream {
            source_id: source.id.clone(),
            kind: SourceKind::Process {
                pid,
                mode: ProcessMode::Include,
            },
        };
        match &source.kind {
            SourceKind::Game => out.extend(game_pid.map(include)),
            SourceKind::OutputDevice {
                leftovers_only: true,
                ..
            } => out.extend(
                playing
                    .iter()
                    .filter(|pid| !claimed.contains(pid))
                    .map(|pid| include(*pid)),
            ),
            other => out.push(Stream {
                source_id: source.id.clone(),
                kind: other.clone(),
            }),
        }
    }
    out
}

/// Summarizes which sources go into the main mix and which onto a track of their
/// own. Track 0 is video, track 1 the main mix.
pub struct TrackLayout {
    pub main_mix: Vec<String>,
    pub separate: Vec<String>,
}

impl TrackLayout {
    pub fn from_sources(sources: &[AudioSource]) -> Self {
        let any_solo = sources.iter().any(|s| s.solo);
        let audible = |s: &AudioSource| s.enabled && !s.muted && (!any_solo || s.solo);

        let mut main_mix = Vec::new();
        let mut separate = Vec::new();
        for source in sources.iter().filter(|s| audible(s)) {
            if source.separate_track {
                separate.push(source.id.clone());
            } else {
                main_mix.push(source.id.clone());
            }
        }
        Self { main_mix, separate }
    }

    /// Number of audio tracks in the resulting MP4.
    pub fn track_count(&self) -> usize {
        usize::from(!self.main_mix.is_empty()) + self.separate.len()
    }
}

/// dB to a linear factor.
pub fn gain_factor(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SourceKind;

    fn source(id: &str, solo: bool, muted: bool, separate: bool) -> AudioSource {
        AudioSource {
            id: id.into(),
            label: id.into(),
            kind: SourceKind::InputDevice {
                device_id: "d".into(),
            },
            enabled: true,
            gain_db: 0.0,
            muted,
            solo,
            separate_track: separate,
        }
    }

    #[test]
    fn solo_silences_the_others() {
        let sources = vec![
            source("a", false, false, false),
            source("b", true, false, false),
        ];
        let layout = TrackLayout::from_sources(&sources);
        assert_eq!(layout.main_mix, vec!["b".to_string()]);
    }

    #[test]
    fn separate_tracks_are_counted_extra() {
        let sources = vec![
            source("game", false, false, false),
            source("discord", false, false, true),
            source("mic", false, false, true),
        ];
        let layout = TrackLayout::from_sources(&sources);
        assert_eq!(layout.track_count(), 3);
    }

    #[test]
    fn gain_conversion() {
        assert!((gain_factor(0.0) - 1.0).abs() < 1e-6);
        assert!((gain_factor(-6.0) - 0.501).abs() < 0.01);
    }

    fn of_kind(id: &str, kind: SourceKind) -> AudioSource {
        AudioSource {
            kind,
            ..source(id, false, false, true)
        }
    }

    fn output(device_id: &str, leftovers_only: bool) -> SourceKind {
        SourceKind::OutputDevice {
            device_id: device_id.into(),
            leftovers_only,
        }
    }

    fn include(pid: u32) -> SourceKind {
        SourceKind::Process {
            pid,
            mode: ProcessMode::Include,
        }
    }

    /// The PIDs of one source, so a test does not depend on the order the
    /// session enumeration happens to return.
    fn pids_of(streams: &[Stream], source_id: &str) -> Vec<u32> {
        let mut out: Vec<u32> = streams
            .iter()
            .filter(|s| s.source_id == source_id)
            .filter_map(|s| match s.kind {
                SourceKind::Process { pid, .. } => Some(pid),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn the_game_source_takes_the_detected_pid() {
        let streams = resolve(&[of_kind("game", SourceKind::Game)], Some(4711), &[]);
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].source_id, "game");
        assert_eq!(streams[0].kind, include(4711));
    }

    /// Not "disabled but present": only dropping it makes `apply` stop the
    /// stream instead of endlessly reporting a source that cannot start.
    #[test]
    fn without_a_game_the_game_source_drops_out() {
        let sources = vec![
            of_kind("game", SourceKind::Game),
            of_kind("mic", SourceKind::InputDevice { device_id: "m".into() }),
        ];
        let streams = resolve(&sources, None, &[]);
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].source_id, "mic");
    }

    /// Windows names one process per client, so the leftovers are not one tap
    /// but many.
    #[test]
    fn the_leftovers_become_one_stream_per_application() {
        let sources = vec![of_kind("rest", output("headphones", true))];
        let streams = resolve(&sources, None, &[10, 11, 12]);
        assert_eq!(pids_of(&streams, "rest"), vec![10, 11, 12]);
    }

    /// The whole point: what another source already records must not land in
    /// the leftovers as well, or it sits in the clip twice.
    #[test]
    fn the_leftovers_leave_out_what_is_recorded_elsewhere() {
        let sources = vec![
            of_kind("game", SourceKind::Game),
            of_kind("discord", include(11)),
            of_kind("rest", output("headphones", true)),
        ];
        let streams = resolve(&sources, Some(10), &[10, 11, 12, 13]);
        assert_eq!(pids_of(&streams, "rest"), vec![12, 13]);
        assert_eq!(pids_of(&streams, "game"), vec![10]);
        assert_eq!(pids_of(&streams, "discord"), vec![11]);
    }

    /// Playing a clip back while the buffer runs must not end up in the next
    /// one.
    #[test]
    fn the_leftovers_never_record_clippiboy_itself() {
        let sources = vec![of_kind("rest", output("headphones", true))];
        let own = std::process::id();
        let streams = resolve(&sources, None, &[own, own + 1]);
        assert_eq!(pids_of(&streams, "rest"), vec![own + 1]);
    }

    /// A disabled source that still held a stream would keep recording.
    #[test]
    fn disabled_sources_produce_no_streams() {
        let mut source = of_kind("mic", SourceKind::InputDevice { device_id: "m".into() });
        source.enabled = false;
        assert!(resolve(&[source], Some(1), &[2]).is_empty());
    }

    #[test]
    fn other_sources_pass_through_unchanged() {
        let sources = vec![
            of_kind("mic", SourceKind::InputDevice { device_id: "m".into() }),
            of_kind("desktop", output("speakers", false)),
            of_kind("discord", include(7)),
        ];
        let streams = resolve(&sources, Some(1234), &[7, 8]);
        assert_eq!(streams.len(), 3);
        for (before, after) in sources.iter().zip(&streams) {
            assert_eq!(before.id, after.source_id);
            assert_eq!(before.kind, after.kind);
        }
    }

    /// The layout must not shift underneath the recording just because a game
    /// started — otherwise the clip would suddenly have a track more or less.
    /// It is built from the stored sources, so resolving may not touch it.
    #[test]
    fn every_track_of_the_layout_has_its_streams() {
        let sources = vec![
            of_kind("game", SourceKind::Game),
            of_kind("rest", output("headphones", true)),
            of_kind("mic", SourceKind::InputDevice { device_id: "m".into() }),
        ];
        let layout = TrackLayout::from_sources(&sources);
        assert_eq!(layout.track_count(), 3);

        let streams = resolve(&sources, Some(10), &[10, 12]);
        for id in &layout.separate {
            assert!(
                streams.iter().any(|s| &s.source_id == id),
                "track '{id}' has no stream"
            );
        }
    }
}
