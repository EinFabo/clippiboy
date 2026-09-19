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

/// Is anything being recorded application by application right now?
///
/// This is the whole switch. As long as nothing is, an output device is simply
/// that device's audio. The moment the game — or a single application — gets a
/// source of its own, recording the device whole would put that sound in the
/// clip twice, so every output device turns into its leftovers instead. Derived
/// rather than stored: there is nothing here a user could get wrong, and nothing
/// that can drift out of step with the sources.
pub fn records_single_applications(sources: &[AudioSource]) -> bool {
    sources.iter().filter(|s| s.enabled).any(|source| {
        matches!(
            source.kind,
            SourceKind::Game
                | SourceKind::Process {
                    mode: ProcessMode::Include,
                    ..
                }
        )
    })
}

/// What plays on each output device, as (pid, is it rendering right now).
///
/// Empty while nothing is recorded application by application — the enumeration
/// is a COM round trip per device and runs on the two-second tick.
pub fn sessions_by_device(sources: &[AudioSource]) -> HashMap<String, Vec<(u32, bool)>> {
    let mut out = HashMap::new();
    if !records_single_applications(sources) {
        return out;
    }
    for source in sources.iter().filter(|s| s.enabled) {
        if let SourceKind::OutputDevice { device_id } = &source.kind {
            out.entry(device_id.clone())
                .or_insert_with(|| devices::session_states(device_id));
        }
    }
    out
}

/// The live pid for a source that was pinned to a process.
///
/// A pid dies with the application, and the one in the configuration then
/// points at nothing — or, after Windows has recycled it, at a different
/// program altogether. So the stored pid counts only while a process with that
/// pid is still running *under the same exe name*; otherwise the exe name is
/// looked up afresh.
///
/// The same idea `game::still_running` applies to the game source, which has
/// had it all along — this is it for the applications picked by hand, which
/// until now stayed dead until somebody deleted and re-added them.
///
/// Where several processes share the name, the lowest pid wins: it is the
/// oldest, and with `INCLUDE_TARGET_PROCESS_TREE` a tap on it covers the
/// children anyway.
fn live_pid(pid: u32, exe: Option<&str>, tree: &HashMap<u32, (u32, String)>) -> Option<u32> {
    let matches_exe = |name: &str| {
        exe.is_some_and(|want| name.eq_ignore_ascii_case(want))
    };
    match tree.get(&pid) {
        // Still there and still itself.
        Some((_, name)) if exe.is_none() || matches_exe(name) => return Some(pid),
        // The pid is taken by something else, or gone. Fall through.
        _ => {}
    }
    let Some(exe) = exe else {
        // Nothing to search by — a source from before the exe was stored. It
        // keeps the old behaviour: the pid as it stands, and an error from the
        // stream if that pid is dead.
        return Some(pid);
    };
    let found = tree
        .iter()
        .filter(|(_, (_, name))| name.eq_ignore_ascii_case(exe))
        .map(|(pid, _)| *pid)
        .min();
    if let Some(now) = found {
        log::info!("'{exe}' is {now} now (was {pid}) — the source follows it");
    }
    found
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
                SourceKind::OutputDevice { device_id } => {
                    Some((source.id.as_str(), device_id.as_str()))
                }
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
            // The live pid, not the stored one — otherwise a rebound source
            // would be tapped here and recorded by the leftovers as well.
            SourceKind::Process {
                pid,
                exe,
                mode: ProcessMode::Include,
            } => taken.extend(
                live_pid(*pid, exe.as_deref(), tree).map(|pid| app_root(pid, tree)),
            ),
            _ => {}
        }
    }

    // Which output device each application belongs to, decided once for all of
    // them so no two can claim the same one. Empty unless something is recorded
    // application by application — then a device is simply itself.
    let leftovers_mode = records_single_applications(sources);
    let home = ledger.assign(sources, playing, tree, &taken);

    let mut out = Vec::with_capacity(sources.len());
    for source in enabled() {
        let include = |pid: u32| Stream {
            source_id: source.id.clone(),
            kind: SourceKind::Process {
                pid,
                // Worked out fresh on every pass, so there is nothing to
                // remember it by.
                exe: None,
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
            SourceKind::OutputDevice { .. } if leftovers_mode => out.extend(leftovers()),
            // Only ever read from an old configuration; `config::migrate_sources`
            // turns it into a plain output device before it gets here.
            SourceKind::Leftovers => out.extend(leftovers()),
            // An application picked by hand: the stored pid is only a starting
            // point. Restarting Discord used to leave this source dead for good.
            SourceKind::Process { pid, exe, mode } => {
                if let Some(now) = live_pid(*pid, exe.as_deref(), tree) {
                    out.push(Stream {
                        source_id: source.id.clone(),
                        kind: SourceKind::Process {
                            pid: now,
                            exe: exe.clone(),
                            mode: *mode,
                        },
                    });
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

/// What is wrong with the sources as *configured* — before any stream is built.
///
/// The engine already reports a source whose stream refuses to start. This is
/// the other half, and the half that had no voice at all: a source that comes up
/// perfectly and records something other than what its label promises. Nothing
/// here fails, so nothing here was ever noticed — which is the whole complaint.
///
/// A pure function over lists, so every rule is testable without a sound card.
pub fn configuration_warnings(
    sources: &[AudioSource],
    devices: &[crate::model::AudioDevice],
    game_detected: bool,
    buffering: bool,
) -> HashMap<String, String> {
    use crate::model::DeviceKind;

    let mut out = HashMap::new();
    for source in sources.iter().filter(|s| s.enabled) {
        let (device_id, kind) = match &source.kind {
            SourceKind::OutputDevice { device_id } => (device_id.as_str(), DeviceKind::Output),
            SourceKind::InputDevice { device_id } => (device_id.as_str(), DeviceKind::Input),
            SourceKind::Game if !game_detected && buffering => {
                // Not an error: no game running is an ordinary state. But the
                // track is written regardless and it will be silence, and that
                // is worth knowing *before* the clip is saved rather than after.
                // Only while buffering — outside of that, sitting on the desktop
                // is simply what the machine is doing, and a banner saying so
                // around the clock would teach everybody to ignore the banner.
                out.insert(
                    source.id.clone(),
                    "no game detected right now — this track stays silent until one is"
                        .to_string(),
                );
                continue;
            }
            _ => continue,
        };

        let here = devices.iter().filter(|d| d.kind == kind);
        if device_id.is_empty() {
            // An empty id means the Windows default device. Sources migrated
            // from the old "leftovers" kind were left this way while keeping a
            // label naming one particular channel — so the label promised the
            // GoXLR's System output and the source recorded whatever Windows
            // had made default. Nothing failed, nothing was said.
            let default_name = here.clone().find(|d| d.is_default).map(|d| d.name.as_str());
            match default_name {
                Some(name) if !source.label.contains(name) => {
                    out.insert(
                        source.id.clone(),
                        format!(
                            "follows the Windows default device, which is '{name}' right \
                             now — not what this source is called. Pick the device in the \
                             mixer to pin it."
                        ),
                    );
                }
                _ => {}
            }
            continue;
        }

        if !here.clone().any(|d| d.id == device_id) {
            out.insert(
                source.id.clone(),
                "this device is not connected — the track stays silent until it is back"
                    .to_string(),
            );
        }
    }
    out
}

/// Every source the buffer records — one track each, in the order they sit in
/// the mixer.
///
/// Deliberately **regardless of `separate_track`**: which of these tracks end up
/// as their own in the clip and which are summed into the main mix is decided on
/// save (`muxer::build`), not here. That is what lets the assignment still be
/// changed while the buffer is running — it then applies to the whole buffer
/// rather than only from that moment on. Recorded as one sum, the past could
/// never be taken apart again.
pub fn recorded(sources: &[AudioSource]) -> impl Iterator<Item = &AudioSource> {
    sources.iter().filter(|s| s.enabled)
}

/// Is the source heard right now?
///
/// Solo beats mute: as soon as solo is set anywhere, only the soloed sources
/// count. An inaudible source keeps its track and is given silence — anything
/// else would make muting throw it out of the buffer retroactively.
pub fn audible(sources: &[AudioSource], source: &AudioSource) -> bool {
    let any_solo = sources.iter().any(|s| s.solo);
    source.enabled && !source.muted && (!any_solo || source.solo)
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

    fn app_source(id: &str, pid: u32, exe: Option<&str>) -> AudioSource {
        of_kind(
            id,
            SourceKind::Process {
                pid,
                exe: exe.map(str::to_string),
                mode: ProcessMode::Include,
            },
        )
    }

    fn pid_of(streams: &[Stream], source_id: &str) -> Option<u32> {
        streams.iter().find_map(|s| match (&s.source_id, &s.kind) {
            (id, SourceKind::Process { pid, .. }) if id == source_id => Some(*pid),
            _ => None,
        })
    }

    /// The pid is alive and still the same program: nothing to do.
    #[test]
    fn a_living_process_keeps_its_pid() {
        let sources = vec![app_source("chat", 4321, Some("Discord.exe"))];
        let tree = tree(&[(4321, 1, "Discord.exe")]);
        let streams = resolve(&sources, None, &nothing(), &tree, &ledger(""));
        assert_eq!(pid_of(&streams, "chat"), Some(4321));
    }

    /// The complaint itself: Discord was restarted, so the stored pid is dead.
    /// The source used to stay dead with it until somebody deleted and re-added
    /// it; now the exe name finds it again.
    #[test]
    fn a_restarted_application_is_found_again_by_its_exe() {
        let sources = vec![app_source("chat", 4321, Some("Discord.exe"))];
        let tree = tree(&[(9999, 1, "Discord.exe")]);
        let streams = resolve(&sources, None, &nothing(), &tree, &ledger(""));
        assert_eq!(pid_of(&streams, "chat"), Some(9999));
    }

    /// Windows reuses pids. The old number now belongs to something else, and
    /// taking it would record the wrong program entirely.
    #[test]
    fn a_recycled_pid_is_not_mistaken_for_the_old_process() {
        let sources = vec![app_source("chat", 4321, Some("Discord.exe"))];
        let tree = tree(&[(4321, 1, "notepad.exe"), (7777, 1, "Discord.exe")]);
        let streams = resolve(&sources, None, &nothing(), &tree, &ledger(""));
        assert_eq!(pid_of(&streams, "chat"), Some(7777));
    }

    /// Nothing of that name is running: the source drops out rather than
    /// tapping a pid that belongs to somebody else.
    #[test]
    fn an_application_that_is_gone_taps_nothing() {
        let sources = vec![app_source("chat", 4321, Some("Discord.exe"))];
        let tree = tree(&[(4321, 1, "notepad.exe")]);
        let streams = resolve(&sources, None, &nothing(), &tree, &ledger(""));
        assert_eq!(pid_of(&streams, "chat"), None);
    }

    /// A source written before the exe was stored behaves exactly as it did.
    #[test]
    fn a_source_without_an_exe_keeps_the_old_behaviour() {
        let sources = vec![app_source("chat", 4321, None)];
        let streams = resolve(&sources, None, &nothing(), &loose(), &ledger(""));
        assert_eq!(pid_of(&streams, "chat"), Some(4321));
    }

    /// Several of the same name: the oldest wins, and a tap on it covers the
    /// children anyway.
    #[test]
    fn the_oldest_of_several_namesakes_wins() {
        let sources = vec![app_source("chat", 1, Some("Discord.exe"))];
        let tree = tree(&[(700, 1, "Discord.exe"), (300, 1, "Discord.exe")]);
        let streams = resolve(&sources, None, &nothing(), &tree, &ledger(""));
        assert_eq!(pid_of(&streams, "chat"), Some(300));
    }

    /// A rebound source must still be excluded from the leftovers, or it lands
    /// in the clip twice — once on its own track and once inside the device.
    #[test]
    fn a_rebound_application_is_not_recorded_twice() {
        let sources = vec![
            app_source("chat", 4321, Some("Discord.exe")),
            of_kind("system", output("dev-1")),
        ];
        let tree = tree(&[(9999, 1, "Discord.exe"), (5000, 1, "game.exe")]);
        let playing: HashMap<String, Vec<(u32, bool)>> =
            [("dev-1".to_string(), vec![(9999, true), (5000, true)])]
                .into_iter()
                .collect();
        let streams = resolve(&sources, None, &playing, &tree, &ledger("dev-1"));
        let leftovers: Vec<u32> = streams
            .iter()
            .filter_map(|s| match (&s.source_id, &s.kind) {
                (id, SourceKind::Process { pid, .. }) if id == "system" => Some(*pid),
                _ => None,
            })
            .collect();
        assert!(
            !leftovers.contains(&9999),
            "the rebound Discord is in the leftovers as well: {leftovers:?}"
        );
        assert!(leftovers.contains(&5000), "the game should be in there");
    }

    // ---- configuration_warnings ----

    fn device(id: &str, name: &str, default: bool) -> crate::model::AudioDevice {
        crate::model::AudioDevice {
            id: id.into(),
            name: name.into(),
            kind: crate::model::DeviceKind::Output,
            is_default: default,
        }
    }

    fn labelled(id: &str, label: &str, kind: SourceKind) -> AudioSource {
        AudioSource {
            label: label.into(),
            ..of_kind(id, kind)
        }
    }

    /// The case found in the real configuration: an empty device id means "the
    /// Windows default", while the label still promises one fixed channel.
    #[test]
    fn a_label_that_does_not_match_the_default_device_is_flagged() {
        let sources = vec![labelled(
            "sys",
            "System (TC-HELICON GoXLR Mini)",
            output(""),
        )];
        let devices = vec![device("dev-1", "Speakers (Realtek)", true)];
        let notes = configuration_warnings(&sources, &devices, false, true);
        assert!(notes.contains_key("sys"), "the mismatch has to be said");
        assert!(notes["sys"].contains("Speakers (Realtek)"));
    }

    /// Following the default device is fine when that is plainly what it is.
    #[test]
    fn a_label_matching_the_default_device_stays_quiet() {
        let sources = vec![labelled("sys", "Speakers (Realtek)", output(""))];
        let devices = vec![device("dev-1", "Speakers (Realtek)", true)];
        assert!(configuration_warnings(&sources, &devices, false, true).is_empty());
    }

    #[test]
    fn a_device_that_is_gone_is_flagged() {
        let sources = vec![labelled("music", "Music (GoXLR)", output("dev-gone"))];
        let devices = vec![device("dev-1", "Speakers", true)];
        let notes = configuration_warnings(&sources, &devices, false, true);
        assert!(notes["music"].contains("not connected"));
    }

    #[test]
    fn a_device_that_is_there_stays_quiet() {
        let sources = vec![labelled("music", "Music (GoXLR)", output("dev-1"))];
        let devices = vec![device("dev-1", "Music (GoXLR)", false)];
        assert!(configuration_warnings(&sources, &devices, false, true).is_empty());
    }

    /// The game track is written whether a game is detected or not, and without
    /// one it is silence. Not an error — but not silent about it either.
    #[test]
    fn the_game_source_says_when_there_is_no_game() {
        let sources = vec![of_kind("game", SourceKind::Game)];
        let notes = configuration_warnings(&sources, &[], false, true);
        assert!(notes["game"].contains("no game detected"));
        assert!(configuration_warnings(&sources, &[], true, true).is_empty());
        // Not buffering: nothing is being recorded, so nothing is silent.
        assert!(configuration_warnings(&sources, &[], false, false).is_empty());
    }

    /// A source switched off is not a problem worth a banner.
    #[test]
    fn a_disabled_source_is_never_flagged() {
        let mut source = labelled("music", "Music (GoXLR)", output("dev-gone"));
        source.enabled = false;
        assert!(configuration_warnings(&[source], &[], false, true).is_empty());
    }

    #[test]
    fn solo_silences_the_others() {
        let sources = vec![
            source("a", false, false, false),
            source("b", true, false, false),
        ];
        assert!(!audible(&sources, &sources[0]));
        assert!(audible(&sources, &sources[1]));
    }

    /// A muted source stays in the buffer and is given silence. Dropping it
    /// would take the minutes already recorded with it.
    #[test]
    fn a_muted_source_is_still_recorded() {
        let sources = vec![source("mic", false, true, true)];
        assert_eq!(recorded(&sources).count(), 1);
        assert!(!audible(&sources, &sources[0]));
    }

    /// Whether a source runs into the main mix says nothing about whether it is
    /// recorded — every one of them gets a track of its own in the buffer.
    #[test]
    fn the_main_mix_is_not_recorded_as_one() {
        let sources = vec![
            source("game", false, false, false),
            source("discord", false, false, true),
            source("mic", false, false, true),
        ];
        assert_eq!(recorded(&sources).count(), 3);
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

    /// The same thing — an output device only records its leftovers while
    /// something else records applications, so the tests that expect that pair
    /// it with a game source.
    fn leftovers(device_id: &str) -> SourceKind {
        output(device_id)
    }

    /// A game source, so the leftovers rule is in force. Detected as pid 1 in the
    /// tests that use it, which no other fixture claims.
    fn recording_by_app() -> AudioSource {
        of_kind("the-game", SourceKind::Game)
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
            exe: None,
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
        let sources = vec![recording_by_app(), of_kind("rest", leftovers("spk"))];
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
            recording_by_app(),
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
            recording_by_app(),
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
            recording_by_app(),
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
        let sources = vec![recording_by_app(), of_kind("rest", leftovers("spk"))];
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
        let sources = vec![recording_by_app(), of_kind("rest", leftovers("spk"))];
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
        let sources = vec![recording_by_app(), of_kind("rest", leftovers("spk"))];
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

    /// While nothing is recorded application by application there is nothing to
    /// leave out, so a device is simply that device.
    #[test]
    fn devices_pass_through_while_nothing_is_recorded_per_application() {
        let sources = vec![
            of_kind("mic", SourceKind::InputDevice { device_id: "m".into() }),
            of_kind("desktop", output("speakers")),
        ];
        let streams = resolve(&sources, Some(1234), &nothing(), &loose(), &ledger(""));
        assert_eq!(streams.len(), 2);
        for (before, after) in sources.iter().zip(&streams) {
            assert_eq!(before.id, after.source_id);
            assert_eq!(before.kind, after.kind);
        }
    }

    /// And the moment one is, the device turns into its leftovers by itself —
    /// nothing to switch, because recording it whole would put that application
    /// in the clip a second time.
    #[test]
    fn one_application_source_turns_every_device_into_its_leftovers() {
        let sources = vec![
            of_kind("discord", include(7)),
            of_kind("desktop", output("speakers")),
        ];
        let streams = resolve(
            &sources,
            None,
            &playing(&[("speakers", &idle(&[7, 8]))]),
            &loose(),
            &ledger("speakers"),
        );
        assert_eq!(pids_of(&streams, "discord"), vec![7]);
        // 7 is already recorded, so only 8 is left over — and the device itself
        // is not opened at all.
        assert_eq!(pids_of(&streams, "desktop"), vec![8]);
        assert!(!streams.iter().any(|s| matches!(s.kind, SourceKind::OutputDevice { .. })));
    }

    /// The track list must not shift underneath the recording just because a
    /// game started — otherwise the clip would suddenly have a track more or
    /// less. It is built from the stored sources, so resolving may not touch it.
    #[test]
    fn every_recorded_track_has_its_streams() {
        let sources = vec![
            of_kind("game", SourceKind::Game),
            of_kind("rest", leftovers("spk")),
            of_kind("mic", SourceKind::InputDevice { device_id: "m".into() }),
        ];
        assert_eq!(recorded(&sources).count(), 3);

        let streams = resolve(
            &sources,
            Some(10),
            &playing(&[("spk", &idle(&[10, 12]))]),
            &loose(),
            &ledger("spk"),
        );
        for source in recorded(&sources) {
            assert!(
                streams.iter().any(|s| s.source_id == source.id),
                "track '{}' has no stream",
                source.id
            );
        }
    }
}
