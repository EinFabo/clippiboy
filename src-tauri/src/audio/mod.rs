//! Audio subsystem: device/process enumeration and (from phase 3) the per-source
//! WASAPI clients plus the mixer.

pub mod capture;
pub mod devices;
pub mod engine;
pub mod ring;

use crate::model::{AudioSource, ProcessMode, SourceKind};

/// Turns the stored, abstract sources into the concrete ones the engine can
/// open. `id` and every mixer setting survive untouched — only `kind` is
/// replaced — so levels, [`TrackLayout`] and the stems all keep working off the
/// unresolved configuration.
///
/// Without a detected game the game source **drops out** rather than staying on
/// disabled: only that way does `AudioEngine::apply` stop its stream instead of
/// reporting a source that cannot start. In the configuration it stays enabled,
/// keeps its `TrackRing` and therefore its track — the mixer pushes silence into
/// it until a game turns up.
pub fn resolve(sources: &[AudioSource], game_pid: Option<u32>) -> Vec<AudioSource> {
    let mut out = Vec::with_capacity(sources.len());
    for source in sources {
        let kind = match (&source.kind, game_pid) {
            (SourceKind::Game, Some(pid)) => SourceKind::Process {
                pid,
                mode: ProcessMode::Include,
            },
            (SourceKind::Game, None) => continue,
            (
                SourceKind::OutputDevice {
                    exclude_game: true, ..
                },
                Some(pid),
            ) => SourceKind::Process {
                pid,
                mode: ProcessMode::Exclude,
            },
            // No game, nothing to leave out: the endpoint carries no game audio
            // anyway, and this way the device choice applies again.
            (other, _) => other.clone(),
        };
        out.push(AudioSource {
            kind,
            ..source.clone()
        });
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

    fn output(device_id: &str, exclude_game: bool) -> SourceKind {
        SourceKind::OutputDevice {
            device_id: device_id.into(),
            exclude_game,
        }
    }

    #[test]
    fn the_game_source_takes_the_detected_pid() {
        let sources = vec![of_kind("game", SourceKind::Game)];
        let resolved = resolve(&sources, Some(4711));
        assert_eq!(
            resolved[0].kind,
            SourceKind::Process {
                pid: 4711,
                mode: ProcessMode::Include
            }
        );
    }

    /// Everything the mixer, the levels and the stems key off has to survive —
    /// only `kind` may change.
    #[test]
    fn resolving_touches_nothing_but_the_kind() {
        let mut original = of_kind("game", SourceKind::Game);
        original.label = "Apex Legends".into();
        original.gain_db = -4.5;
        original.solo = true;
        let resolved = resolve(std::slice::from_ref(&original), Some(1));

        assert_eq!(resolved[0].id, original.id);
        assert_eq!(resolved[0].label, original.label);
        assert_eq!(resolved[0].gain_db, original.gain_db);
        assert!(resolved[0].solo);
        assert!(resolved[0].separate_track);
        assert!(resolved[0].enabled);
    }

    /// Not "disabled but present": only dropping it makes `apply` stop the
    /// stream instead of endlessly reporting a source that cannot start.
    #[test]
    fn without_a_game_the_game_source_drops_out() {
        let sources = vec![
            of_kind("game", SourceKind::Game),
            of_kind("mic", SourceKind::InputDevice { device_id: "m".into() }),
        ];
        let resolved = resolve(&sources, None);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].id, "mic");
    }

    #[test]
    fn an_excluding_output_becomes_process_loopback() {
        let sources = vec![of_kind("desktop", output("headphones", true))];
        let resolved = resolve(&sources, Some(99));
        assert_eq!(
            resolved[0].kind,
            SourceKind::Process {
                pid: 99,
                mode: ProcessMode::Exclude
            }
        );
    }

    #[test]
    fn without_a_game_the_output_keeps_its_device() {
        let sources = vec![of_kind("desktop", output("headphones", true))];
        let resolved = resolve(&sources, None);
        assert_eq!(resolved[0].kind, output("headphones", true));
    }

    #[test]
    fn other_sources_pass_through_unchanged() {
        let sources = vec![
            of_kind("mic", SourceKind::InputDevice { device_id: "m".into() }),
            of_kind("desktop", output("speakers", false)),
            of_kind(
                "discord",
                SourceKind::Process {
                    pid: 7,
                    mode: ProcessMode::Include,
                },
            ),
        ];
        let resolved = resolve(&sources, Some(1234));
        for (before, after) in sources.iter().zip(&resolved) {
            assert_eq!(before.kind, after.kind);
        }
    }

    /// The layout must not shift underneath the recording just because a game
    /// started — otherwise the clip would suddenly have a track more or less.
    #[test]
    fn resolving_does_not_move_the_tracks() {
        let sources = vec![
            of_kind("game", SourceKind::Game),
            of_kind("desktop", output("headphones", true)),
            of_kind("mic", SourceKind::InputDevice { device_id: "m".into() }),
        ];
        let plain = TrackLayout::from_sources(&sources);
        let running = TrackLayout::from_sources(&resolve(&sources, Some(5)));
        assert_eq!(plain.separate, running.separate);
        assert_eq!(plain.track_count(), running.track_count());
    }
}
