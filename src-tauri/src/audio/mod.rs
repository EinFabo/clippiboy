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

/// What plays on each device a leftovers source asks about, as
/// (pid, is it rendering right now).
///
/// Only those devices: the enumeration is a COM round trip each, and this runs
/// on the two-second tick.
pub fn sessions_by_device(sources: &[AudioSource]) -> HashMap<String, Vec<(u32, bool)>> {
    let mut out = HashMap::new();
    for source in sources.iter().filter(|s| s.enabled) {
        if let SourceKind::OutputDevice {
            device_id,
            leftovers_only: true,
        } = &source.kind
        {
            out.entry(device_id.clone())
                .or_insert_with(|| devices::session_states(device_id));
        }
    }
    out
}

/// Decides which leftovers source each application belongs to.
///
/// The point is that nobody should have to sort this out by hand. Windows says
/// whether a session is *rendering* or merely open, and that is exactly the
/// difference between "this application plays on that device" and "it once
/// opened a stream there and left it lying". So an application goes where it is
/// audibly playing, and everything else is only there to break ties.
#[derive(Default)]
pub struct Leftovers {
    /// The default output device — where anything not routed on purpose ends up,
    /// and therefore the right home for an application that is playing nowhere in
    /// particular.
    pub default_device: String,
    /// Who owned which application last time, so a moment of silence does not
    /// move it to another source and cost a stream restart.
    pub previous: HashMap<u32, String>,
}

impl Leftovers {
    fn assign(
        &self,
        sources: &[AudioSource],
        playing: &HashMap<String, Vec<(u32, bool)>>,
        tree: &HashMap<u32, (u32, String)>,
        taken: &[u32],
    ) -> Vec<(u32, String)> {
        // (source id, device id) of every source asking for leftovers, in the
        // order they sit in the mixer.
        let buckets: Vec<(&str, &str)> = sources
            .iter()
            .filter(|source| source.enabled)
            .filter_map(|source| match &source.kind {
                SourceKind::OutputDevice {
                    device_id,
                    leftovers_only: true,
                } => Some((source.id.as_str(), device_id.as_str())),
                SourceKind::Leftovers => Some((source.id.as_str(), "")),
                _ => None,
            })
            .collect();

        // One entry per application rather than per session: Discord holds two,
        // and tapping both would split it for no gain. Ancestors first, so a
        // chosen tap already covers its own descendants when they come up.
        let mut roots: Vec<u32> = Vec::new();
        for sessions in playing.values() {
            for (pid, _) in sessions {
                let root = app_root(*pid, tree);
                if !roots.contains(&root) {
                    roots.push(root);
                }
            }
        }
        roots.sort_by_key(|root| ancestors(*root, tree).len());

        let mut out: Vec<(u32, String)> = Vec::new();
        let mut spoken_for: Vec<u32> = taken.to_vec();
        for root in roots {
            // A tap covers the target and its children, so anything below an
            // application another source records is already in the clip.
            let covered = spoken_for.contains(&root)
                || ancestors(root, tree)
                    .iter()
                    .any(|parent| spoken_for.contains(parent));
            if covered {
                continue;
            }

            // Does this application have a session on that bucket's device, and
            // is it rendering there?
            let state = |device: &str| {
                playing.get(device)?.iter().find_map(|(pid, active)| {
                    (app_root(*pid, tree) == root).then_some(*active)
                })
            };

            let owner = buckets
                .iter()
                // Where it is audibly playing. That is the whole rule.
                .find(|(_, device)| state(device) == Some(true))
                // It went quiet: stay put rather than move and cut the stream.
                .or_else(|| {
                    buckets.iter().find(|(id, device)| {
                        self.previous.get(&root).map(String::as_str) == Some(*id)
                            && state(device).is_some()
                    })
                })
                // Playing nowhere in particular belongs on the catch-all.
                .or_else(|| {
                    buckets
                        .iter()
                        .find(|(_, device)| *device == self.default_device && state(device).is_some())
                })
                .or_else(|| buckets.iter().find(|(_, device)| state(device).is_some()));

            if let Some((id, _)) = owner {
                spoken_for.push(root);
                out.push((root, (*id).to_string()));
            }
        }
        out
    }
}

/// Turns the stored, abstract sources into the streams the engine can open.
///
/// `playing` maps a device id to the processes holding a session on it, `tree`
/// is the process parentage both rules below need.
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
///   assembled from one tap per application on that device that no other source
///   records. Membership is decided by descent, not by an equal PID: a tap covers
///   the target *and its children*, so a parent and a child taken separately
///   would record the child twice.
///
///   Several devices may ask for the leftovers, and each application goes to
///   exactly one of them — see [`Leftovers`].
pub fn resolve(
    sources: &[AudioSource],
    game_pid: Option<u32>,
    playing: &HashMap<String, Vec<(u32, bool)>>,
    tree: &HashMap<u32, (u32, String)>,
    ledger: &Leftovers,
) -> Vec<Stream> {
    let enabled = || sources.iter().filter(|s| s.enabled);

    // Everything another source already records, plus ClippiBoy itself: playing a
    // clip back while the buffer runs must not end up in the next one. The
    // leftovers add to this as they go, which is what keeps several of them from
    // taking the same application.
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

    // Which leftovers source each application belongs to, decided once for all
    // of them so no two can claim the same one.
    let home = ledger.assign(sources, playing, tree, &taken);

    let mut out = Vec::with_capacity(sources.len());
    for source in enabled() {
        let include = |pid: u32| Stream {
            source_id: source.id.clone(),
            kind: SourceKind::Process {
                pid,
                mode: ProcessMode::Include,
            },
        };
        let leftovers = || {
            home.iter()
                .filter(|(_, owner)| owner == &source.id)
                .map(|(root, _)| include(*root))
                .collect::<Vec<_>>()
        };

        match &source.kind {
            SourceKind::Game => out.extend(game_pid.map(include)),
            SourceKind::OutputDevice {
                leftovers_only: true,
                ..
            }
            // Only ever read from an old configuration; `config::migrate_sources`
            // turns it into the option above before it gets here.
            | SourceKind::Leftovers => out.extend(leftovers()),
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
            leftovers_only: false,
        }
    }

    fn leftovers(device_id: &str) -> SourceKind {
        SourceKind::OutputDevice {
            device_id: device_id.into(),
            leftovers_only: true,
        }
    }

    /// Sessions per device as (pid, rendering right now).
    fn playing(devices: &[(&str, &[(u32, bool)])]) -> HashMap<String, Vec<(u32, bool)>> {
        devices
            .iter()
            .map(|(device, sessions)| ((*device).to_string(), sessions.to_vec()))
            .collect()
    }

    /// Sessions that merely sit open — the state most of them are in.
    fn idle(pids: &[u32]) -> Vec<(u32, bool)> {
        pids.iter().map(|pid| (*pid, false)).collect()
    }

    fn nothing() -> HashMap<String, Vec<(u32, bool)>> {
        HashMap::new()
    }

    /// No previous assignment, and the named device is the catch-all.
    fn ledger(default_device: &str) -> Leftovers {
        Leftovers {
            default_device: default_device.into(),
            previous: HashMap::new(),
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
            &nothing(),
            &loose(),
            &ledger(""),
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
        let streams = resolve(&sources, None, &nothing(), &loose(), &ledger(""));
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].source_id, "mic");
    }

    /// Windows names one process per tap, so the leftovers are not one client
    /// but many.
    #[test]
    fn the_leftovers_become_one_stream_per_application() {
        let sources = vec![of_kind("rest", leftovers("spk"))];
        let streams = resolve(
            &sources,
            None,
            &playing(&[("spk", &idle(&[10, 11, 12]))]),
            &loose(),
            &ledger("spk"),
        );
        assert_eq!(pids_of(&streams, "rest"), vec![10, 11, 12]);
    }

    /// The whole point: what another source already records must not land in
    /// the leftovers as well, or it sits in the clip twice.
    #[test]
    fn the_leftovers_leave_out_what_is_recorded_elsewhere() {
        let sources = vec![
            of_kind("game", SourceKind::Game),
            of_kind("discord", include(11)),
            of_kind("rest", leftovers("spk")),
        ];
        let streams = resolve(
            &sources,
            Some(10),
            &playing(&[("spk", &idle(&[10, 11, 12, 13]))]),
            &loose(),
            &ledger("spk"),
        );
        assert_eq!(pids_of(&streams, "rest"), vec![12, 13]);
        assert_eq!(pids_of(&streams, "game"), vec![10]);
        assert_eq!(pids_of(&streams, "discord"), vec![11]);
    }

    /// The rule that makes sorting by hand unnecessary, in the shape it really
    /// occurs: a hardware mixer routes Discord to its chat channel, and the
    /// default device still lists a session for it because everything opens one
    /// there. Only one of the two is *rendering*, and that is the one that
    /// decides — regardless of which source sits higher.
    #[test]
    fn an_application_goes_where_it_is_actually_playing() {
        let sources = vec![
            of_kind("system", leftovers("system")),
            of_kind("chat", leftovers("chat")),
        ];
        let streams = resolve(
            &sources,
            None,
            &playing(&[
                ("system", &[(5556, false), (900, true)]),
                ("chat", &[(5556, true)]),
            ]),
            &loose(),
            &ledger("system"),
        );
        assert_eq!(pids_of(&streams, "chat"), vec![5556]);
        assert_eq!(pids_of(&streams, "system"), vec![900]);
    }

    /// Steam sits open on every device without playing anywhere. It belongs on
    /// the catch-all, not on whichever specific channel happens to come first.
    #[test]
    fn something_playing_nowhere_lands_on_the_default_device() {
        let sources = vec![
            of_kind("chat", leftovers("chat")),
            of_kind("system", leftovers("system")),
        ];
        let streams = resolve(
            &sources,
            None,
            &playing(&[("chat", &idle(&[33264])), ("system", &idle(&[33264]))]),
            &loose(),
            &ledger("system"),
        );
        assert_eq!(pids_of(&streams, "system"), vec![33264]);
        assert!(pids_of(&streams, "chat").is_empty());
    }

    /// Between two sentences a voice chat falls silent for a moment. Moving it
    /// to another source and back would restart the stream and punch a hole in
    /// the track each time.
    #[test]
    fn a_moment_of_silence_does_not_move_an_application() {
        let sources = vec![
            of_kind("system", leftovers("system")),
            of_kind("chat", leftovers("chat")),
        ];
        let quiet_everywhere = playing(&[
            ("system", &idle(&[5556])),
            ("chat", &idle(&[5556])),
        ]);
        let mut ledger = ledger("system");
        ledger.previous.insert(5556, "chat".into());

        let streams = resolve(&sources, None, &quiet_everywhere, &loose(), &ledger);
        assert_eq!(pids_of(&streams, "chat"), vec![5556]);
        assert!(pids_of(&streams, "system").is_empty());
    }

    /// Discord's two sessions are one application. Tapping both would split it
    /// across the leftovers for nothing; the common parent covers them at once.
    #[test]
    fn an_application_with_two_sessions_is_tapped_once() {
        let sources = vec![of_kind("rest", leftovers("spk"))];
        let streams = resolve(
            &sources,
            None,
            &playing(&[("spk", &idle(&[2584, 30988]))]),
            &discord(),
            &ledger("spk"),
        );
        assert_eq!(pids_of(&streams, "rest"), vec![5556]);
    }

    /// A tap covers the target *and its children*. So a session whose ancestor
    /// another source already records must be left alone — comparing PIDs for
    /// equality would miss exactly this and record the child twice.
    #[test]
    fn the_leftovers_skip_a_child_of_a_recorded_application() {
        let sources = vec![
            of_kind("discord", include(5556)),
            of_kind("rest", leftovers("spk")),
        ];
        // 2584 is a child of the recorded 5556; 40944 belongs to nobody.
        let mut processes = discord();
        processes.insert(40944, (0, "VALORANT-Win64-Shipping.exe".into()));
        let streams = resolve(
            &sources,
            None,
            &playing(&[("spk", &idle(&[2584, 30988, 40944]))]),
            &processes,
            &ledger("spk"),
        );
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
        let sources = vec![of_kind("rest", leftovers("spk"))];
        // Deliberately the child first: the order of the session list must not
        // decide the outcome.
        let streams = resolve(
            &sources,
            None,
            &playing(&[("spk", &idle(&[12604, 33264]))]),
            &processes,
            &ledger("spk"),
        );
        assert_eq!(pids_of(&streams, "rest"), vec![33264]);
    }

    /// Playing a clip back while the buffer runs must not end up in the next
    /// one.
    #[test]
    fn the_leftovers_never_record_clippiboy_itself() {
        let sources = vec![of_kind("rest", leftovers("spk"))];
        let own = std::process::id();
        let streams = resolve(
            &sources,
            None,
            &playing(&[("spk", &idle(&[own, own + 1]))]),
            &loose(),
            &ledger("spk"),
        );
        assert_eq!(pids_of(&streams, "rest"), vec![own + 1]);
    }

    /// A disabled source that still held a stream would keep recording.
    #[test]
    fn disabled_sources_produce_no_streams() {
        let mut source = of_kind("mic", SourceKind::InputDevice { device_id: "m".into() });
        source.enabled = false;
        assert!(resolve(&[source], Some(1), &nothing(), &loose(), &ledger("")).is_empty());
    }

    #[test]
    fn other_sources_pass_through_unchanged() {
        let sources = vec![
            of_kind("mic", SourceKind::InputDevice { device_id: "m".into() }),
            of_kind("desktop", output("speakers")),
            of_kind("discord", include(7)),
        ];
        let streams = resolve(&sources, Some(1234), &nothing(), &loose(), &ledger(""));
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
            of_kind("rest", leftovers("spk")),
            of_kind("mic", SourceKind::InputDevice { device_id: "m".into() }),
        ];
        let layout = TrackLayout::from_sources(&sources);
        assert_eq!(layout.track_count(), 3);

        let streams = resolve(
            &sources,
            Some(10),
            &playing(&[("spk", &idle(&[10, 12]))]),
            &loose(),
            &ledger("spk"),
        );
        for id in &layout.separate {
            assert!(
                streams.iter().any(|s| &s.source_id == id),
                "track '{id}' has no stream"
            );
        }
    }
}
