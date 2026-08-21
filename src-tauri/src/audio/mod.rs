//! Audio subsystem: device/process enumeration and (from phase 3) the per-source
//! WASAPI clients plus the mixer.

pub mod capture;
pub mod devices;
pub mod engine;
pub mod ring;

use crate::model::AudioSource;

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
}
