//! Audio subsystem: device/process enumeration and (from phase 3) the per-source
//! WASAPI clients plus the mixer.

pub mod capture;
pub mod devices;
pub mod engine;
pub mod ring;

use std::collections::HashMap;

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

/// A process's ancestors, closest first. `pid` itself is not included.
///
/// The depth cap is pure defence: a corrupt snapshot with a cycle would
/// otherwise hang the audio thread.
fn ancestors(pid: u32, tree: &HashMap<u32, (u32, String)>) -> Vec<u32> {
    let mut out = Vec::new();
    let mut current = pid;
    for _ in 0..32 {
        let Some((parent, _)) = tree.get(&current) else {
            break;
        };
        if *parent == 0 || out.contains(parent) {
            break;
        }
        out.push(*parent);
        current = *parent;
    }
    out
}

/// The top process of the application `pid` belongs to.
///
/// Upwards while the parent runs the same executable — Discord's audio sessions
/// live in utility children of one `Discord.exe`, and only their common parent
/// covers all of them in a single tap. Stopping at a differently named parent is
/// what keeps the walk from climbing out into `explorer.exe`.
pub fn app_root(pid: u32, tree: &HashMap<u32, (u32, String)>) -> u32 {
    let mut current = pid;
    for _ in 0..32 {
        let Some((parent, exe)) = tree.get(&current) else {
            break;
        };
        match tree.get(parent) {
            Some((_, parent_exe)) if parent_exe.eq_ignore_ascii_case(exe) => current = *parent,
            _ => break,
        }
    }
    current
}

/// Turns the stored, abstract sources into the streams the engine can open.
///
/// `playing` are the processes holding a render session anywhere right now,
/// `tree` the process parentage both rules below need.
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
///   `AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS` names exactly one process. So they are
///   assembled from one tap per application that no other source records.
///   Membership is decided by descent, not by an equal PID: a tap covers the
///   target *and its children*, so a parent and a child taken separately would
///   record the child twice.
pub fn resolve(
    sources: &[AudioSource],
    game_pid: Option<u32>,
    playing: &[u32],
    tree: &HashMap<u32, (u32, String)>,
) -> Vec<Stream> {
    let enabled = || sources.iter().filter(|s| s.enabled);

    // Everything another source already records, plus ClippiBoy itself: playing a
    // clip back while the buffer runs must not end up in the next one.
    let mut taken: Vec<u32> = vec![std::process::id()];
    for source in enabled() {
        match &source.kind {
            SourceKind::Game => taken.extend(game_pid),
            SourceKind::Process {
                pid,
                mode: ProcessMode::Include,
            } => taken.push(app_root(*pid, tree)),
            _ => {}
        }
    }

    // One entry per application rather than per session: Discord holds two, and
    // tapping both would split it across sources for no gain.
    let mut roots: Vec<u32> = Vec::new();
    for pid in playing {
        let root = app_root(*pid, tree);
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    // Ancestors first, so a chosen tap already covers its own descendants when
    // they come up and they are skipped instead of taken a second time.
    roots.sort_by_key(|root| ancestors(*root, tree).len());

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
            SourceKind::Leftovers => {
                for root in &roots {
                    let covered = taken.contains(root)
                        || ancestors(*root, tree)
                            .iter()
                            .any(|parent| taken.contains(parent));
                    if covered {
                        continue;
                    }
                    taken.push(*root);
                    out.push(include(*root));
                }
            }
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

    fn output(device_id: &str) -> SourceKind {
        SourceKind::OutputDevice {
            device_id: device_id.into(),
        }
    }

    /// A process tree as (pid, parent, exe).
    fn tree(processes: &[(u32, u32, &str)]) -> HashMap<u32, (u32, String)> {
        processes
            .iter()
            .map(|(pid, parent, exe)| (*pid, (*parent, (*exe).to_string())))
            .collect()
    }

    /// Unrelated processes — the simple case, where descent decides nothing.
    fn loose() -> HashMap<u32, (u32, String)> {
        HashMap::new()
    }

    /// Discord as it really looks: two sessions in utility children of one
    /// Discord.exe, whose own parent is something else entirely.
    fn discord() -> HashMap<u32, (u32, String)> {
        tree(&[
            (2584, 5556, "Discord.exe"),
            (30988, 5556, "Discord.exe"),
            (5556, 12800, "Discord.exe"),
            (12800, 0, "explorer.exe"),
        ])
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
        let streams = resolve(
            &[of_kind("game", SourceKind::Game)],
            Some(4711),
            &[],
            &loose(),
        );
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
        let streams = resolve(&sources, None, &[], &loose());
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].source_id, "mic");
    }

    /// Windows names one process per tap, so the leftovers are not one client
    /// but many.
    #[test]
    fn the_leftovers_become_one_stream_per_application() {
        let sources = vec![of_kind("rest", SourceKind::Leftovers)];
        let streams = resolve(&sources, None, &[10, 11, 12], &loose());
        assert_eq!(pids_of(&streams, "rest"), vec![10, 11, 12]);
    }

    /// The whole point: what another source already records must not land in
    /// the leftovers as well, or it sits in the clip twice.
    #[test]
    fn the_leftovers_leave_out_what_is_recorded_elsewhere() {
        let sources = vec![
            of_kind("game", SourceKind::Game),
            of_kind("discord", include(11)),
            of_kind("rest", SourceKind::Leftovers),
        ];
        let streams = resolve(&sources, Some(10), &[10, 11, 12, 13], &loose());
        assert_eq!(pids_of(&streams, "rest"), vec![12, 13]);
        assert_eq!(pids_of(&streams, "game"), vec![10]);
        assert_eq!(pids_of(&streams, "discord"), vec![11]);
    }

    /// Discord's two sessions are one application. Tapping both would split it
    /// across the leftovers for nothing; the common parent covers them at once.
    #[test]
    fn an_application_with_two_sessions_is_tapped_once() {
        let sources = vec![of_kind("rest", SourceKind::Leftovers)];
        let streams = resolve(&sources, None, &[2584, 30988], &discord());
        assert_eq!(pids_of(&streams, "rest"), vec![5556]);
    }

    /// A tap covers the target *and its children*. So a session whose ancestor
    /// another source already records must be left alone — comparing PIDs for
    /// equality would miss exactly this and record the child twice.
    #[test]
    fn the_leftovers_skip_a_child_of_a_recorded_application() {
        let sources = vec![
            of_kind("discord", include(5556)),
            of_kind("rest", SourceKind::Leftovers),
        ];
        // 2584 is a child of the recorded 5556; 40944 belongs to nobody.
        let mut processes = discord();
        processes.insert(40944, (0, "VALORANT-Win64-Shipping.exe".into()));
        let streams = resolve(&sources, None, &[2584, 30988, 40944], &processes);
        assert_eq!(pids_of(&streams, "rest"), vec![40944]);
    }

    /// Two taps where one is the other's ancestor would record the descendant
    /// twice. Steam holds sessions on every device and spawns steamwebhelper.
    #[test]
    fn a_tap_is_never_nested_inside_another() {
        let processes = tree(&[
            (33264, 0, "steam.exe"),
            (12604, 33264, "steamwebhelper.exe"),
        ]);
        let sources = vec![of_kind("rest", SourceKind::Leftovers)];
        // Deliberately the child first: the order of the session list must not
        // decide the outcome.
        let streams = resolve(&sources, None, &[12604, 33264], &processes);
        assert_eq!(pids_of(&streams, "rest"), vec![33264]);
    }

    /// Playing a clip back while the buffer runs must not end up in the next
    /// one.
    #[test]
    fn the_leftovers_never_record_clippiboy_itself() {
        let sources = vec![of_kind("rest", SourceKind::Leftovers)];
        let own = std::process::id();
        let streams = resolve(&sources, None, &[own, own + 1], &loose());
        assert_eq!(pids_of(&streams, "rest"), vec![own + 1]);
    }

    /// A disabled source that still held a stream would keep recording.
    #[test]
    fn disabled_sources_produce_no_streams() {
        let mut source = of_kind("mic", SourceKind::InputDevice { device_id: "m".into() });
        source.enabled = false;
        assert!(resolve(&[source], Some(1), &[2], &loose()).is_empty());
    }

    #[test]
    fn other_sources_pass_through_unchanged() {
        let sources = vec![
            of_kind("mic", SourceKind::InputDevice { device_id: "m".into() }),
            of_kind("desktop", output("speakers")),
            of_kind("discord", include(7)),
        ];
        let streams = resolve(&sources, Some(1234), &[7, 8], &loose());
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
            of_kind("rest", SourceKind::Leftovers),
            of_kind("mic", SourceKind::InputDevice { device_id: "m".into() }),
        ];
        let layout = TrackLayout::from_sources(&sources);
        assert_eq!(layout.track_count(), 3);

        let streams = resolve(&sources, Some(10), &[10, 12], &loose());
        for id in &layout.separate {
            assert!(
                streams.iter().any(|s| &s.source_id == id),
                "track '{id}' has no stream"
            );
        }
    }
}
