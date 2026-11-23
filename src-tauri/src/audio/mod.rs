//! Audio subsystem: device/process enumeration and (from phase 3) the per-source
//! WASAPI clients plus the mixer.

pub mod capture;
pub mod devices;
pub mod engine;
pub mod resample;
pub mod ring;
pub mod watch;

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

/// Cuts every parent link the snapshot cannot vouch for. A process keeps the
/// pid of its parent after the parent ended, and Windows hands that pid to the
/// next process — a program started later would pass for the parent, and an
/// application for a child of a game's launcher. A real parent was created
/// before its child; one created after it is a stranger, and the link becomes
/// 0 (no parent, as for the System process). `created` holds the creation
/// times that could be read; a link with a time missing on either side stays.
pub fn cut_reused_parents(tree: &mut HashMap<u32, (u32, String)>, created: &HashMap<u32, u64>) {
    for (pid, (parent, _)) in tree.iter_mut() {
        if let (Some(child_at), Some(parent_at)) = (created.get(pid), created.get(parent)) {
            if parent_at > child_at {
                *parent = 0;
            }
        }
    }
}

/// Creation times kept between snapshots, by pid, parent pid and exe: a pid
/// Windows has handed to another process comes with another parent or name
/// and is looked up anew.
pub type CreatedCache = HashMap<(u32, u32, String), u64>;

/// The creation times [`cut_reused_parents`] needs for `tree` — of every
/// process linked to a parent that is there, and of that parent. `lookup`
/// (which opens the process) is asked only for what `cache` does not know,
/// and once per pid in a call even when it refuses: a protected launcher with
/// forty children used to be opened forty times, every two seconds. `cache`
/// is left holding the processes of `tree` alone.
pub fn creation_times(
    tree: &HashMap<u32, (u32, String)>,
    cache: &mut CreatedCache,
    mut lookup: impl FnMut(u32) -> Option<u64>,
) -> HashMap<u32, u64> {
    // Only links to a parent that is there can mislead.
    let linked: Vec<u32> = tree
        .iter()
        .filter(|(_, (parent, _))| *parent != 0 && tree.contains_key(parent))
        .flat_map(|(pid, (parent, _))| [*pid, *parent])
        .collect();
    let mut asked: HashMap<u32, Option<u64>> = HashMap::new();
    for pid in linked {
        if asked.contains_key(&pid) {
            continue;
        }
        let Some((parent, exe)) = tree.get(&pid) else {
            continue;
        };
        let key = (pid, *parent, exe.clone());
        let at = match cache.get(&key) {
            Some(at) => Some(*at),
            None => {
                let at = lookup(pid);
                // A refusal is not kept: it is asked again next time.
                if let Some(at) = at {
                    cache.insert(key, at);
                }
                at
            }
        };
        asked.insert(pid, at);
    }
    cache.retain(|(pid, parent, exe), _| {
        tree.get(pid).is_some_and(|(now_parent, now_exe)| now_parent == parent && now_exe == exe)
    });
    asked.into_iter().filter_map(|(pid, at)| Some((pid, at?))).collect()
}

/// A process's ancestors, closest first. `pid` itself is not included.
/// Relies on the tree's links being real, see [`cut_reused_parents`].
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

/// The application the user means by `pid`: [`app_root`], and where that root
/// is a known audio helper, on up to whoever started it. The web view of an
/// application plays the sound, the application is what the picker lists and a
/// source follows. Only for choosing and finding; the leftovers keep
/// [`app_root`], where a launcher's helper is deliberately its own entry.
pub fn app_of(pid: u32, tree: &HashMap<u32, (u32, String)>) -> u32 {
    let mut current = app_root(pid, tree);
    for _ in 0..8 {
        let Some((parent, exe)) = tree.get(&current) else {
            break;
        };
        if *parent == 0 {
            break;
        }
        match tree.get(parent) {
            // Never up into the shell or a system host: the helper stays.
            Some((_, parent_exe)) if is_audio_helper(exe) && !is_system_host(parent_exe) =>
            {
                current = app_root(*parent, tree)
            }
            _ => break,
        }
    }
    current
}

/// Executables that only ever play sound on behalf of another application —
/// embedded browsers. Picking one by itself would record whichever program
/// happens to host a web view.
///
/// Not the launchers' own web views (`steamwebhelper.exe`, `epicwebhelper.exe`):
/// climbing from those lands on the launcher, and a tap on the launcher records
/// its whole tree — every game it started, a second time. They stay entries of
/// their own, as before; see [`LAUNCHER_HELPERS`].
const AUDIO_HELPERS: &[&str] = &[
    "msedgewebview2.exe",
    "cefsharp.browsersubprocess.exe",
    "cefsubprocess.exe",
    "crashpad_handler.exe",
];

/// The launchers' web views — overlay, friends' voice chat, the store — and the
/// name the picker gives them, since the exe says little.
const LAUNCHER_HELPERS: &[(&str, &str)] = &[
    ("steamwebhelper.exe", "Steam (overlay & voice)"),
    ("epicwebhelper.exe", "Epic Games (web)"),
];

fn launcher_helper_name(exe: &str) -> Option<&'static str> {
    LAUNCHER_HELPERS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(exe))
        .map(|(_, shown)| *shown)
}

/// Processes that start things for the whole session and own no application.
const SYSTEM_HOSTS: &[&str] = &[
    "explorer.exe",
    "svchost.exe",
    "sihost.exe",
    "runtimebroker.exe",
    "services.exe",
    "winlogon.exe",
    "dwm.exe",
    "applicationframehost.exe",
];

fn is_system_host(exe: &str) -> bool {
    SYSTEM_HOSTS.iter().any(|name| name.eq_ignore_ascii_case(exe))
}

pub fn is_audio_helper(exe: &str) -> bool {
    AUDIO_HELPERS.iter().any(|name| name.eq_ignore_ascii_case(exe))
}

/// Does `pid` sit in the process tree of `root` (or is it `root`)?
pub fn in_tree_of(pid: u32, root: u32, tree: &HashMap<u32, (u32, String)>) -> bool {
    pid == root || ancestors(pid, tree).contains(&root)
}

/// One process holding an audio session, as the enumeration saw it.
#[derive(Debug, Clone)]
pub struct SeenSession {
    pub pid: u32,
    pub active: bool,
    /// What the session calls itself; often empty.
    pub display: Option<String>,
    /// The exe read from the process itself, where Windows allowed it.
    pub exe: Option<String>,
}

/// The sessions as the picker shows them: one entry per application.
///
/// Sessions are grouped by [`app_root`], so Discord's two utility children are
/// one row, and the row carries the root's pid and exe — what a source has to
/// store to find the application again. ClippiBoy's own process tree (its web
/// views play the player's audio) is never listed.
pub fn group_apps(
    sessions: &[SeenSession],
    tree: &HashMap<u32, (u32, String)>,
    own: u32,
) -> Vec<crate::model::AudioProcess> {
    struct Group {
        root: u32,
        exe: String,
        pids: Vec<u32>,
        active: bool,
        display: Option<String>,
    }
    let mut groups: Vec<Group> = Vec::new();
    for session in sessions {
        let root = app_of(session.pid, tree);
        if in_tree_of(session.pid, own, tree) || in_tree_of(root, own, tree) {
            continue;
        }
        let exe = tree
            .get(&root)
            .map(|(_, name)| name.clone())
            .or_else(|| {
                // The root is not in the snapshot: only the session itself
                // can say what it is.
                (root == session.pid).then(|| session.exe.clone()).flatten()
            })
            .unwrap_or_else(|| format!("PID {root}"));
        // The host is gone: a bare web view says nothing about who it plays for.
        if is_audio_helper(&exe) {
            continue;
        }
        let own_session = root == session.pid
            || session
                .exe
                .as_deref()
                .or_else(|| tree.get(&session.pid).map(|(_, name)| name.as_str()))
                .is_some_and(|name| name.eq_ignore_ascii_case(&exe));
        // Names a helper session gives itself describe the helper, not the app.
        let display = session
            .display
            .clone()
            .filter(|name| own_session && !name.is_empty() && !name.starts_with('@'));
        let index = match groups.iter().position(|g| g.root == root) {
            Some(index) => index,
            None => {
                groups.push(Group {
                    root,
                    exe,
                    pids: Vec::new(),
                    active: false,
                    display: None,
                });
                groups.len() - 1
            }
        };
        let group = &mut groups[index];
        if !group.pids.contains(&session.pid) {
            group.pids.push(session.pid);
        }
        group.active |= session.active;
        if group.display.is_none() || session.pid == root {
            group.display = display.or(group.display.take());
        }
    }
    let mut out: Vec<crate::model::AudioProcess> = groups
        .into_iter()
        .map(|group| crate::model::AudioProcess {
            pid: group.root,
            name: launcher_helper_name(&group.exe)
                .map(str::to_string)
                .or(group.display)
                .unwrap_or_else(|| group.exe.trim_end_matches(".exe").to_string()),
            exe: group.exe,
            count: group.pids.len() as u32,
            active: group.active,
            icon: None,
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// A tapped process, named for the UI.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tap {
    pub pid: u32,
    pub name: String,
}

/// The taps with the name of each tapped process, so the UI never has to show
/// a bare pid. The name is the exe without its extension.
pub fn name_taps(
    taps: &HashMap<String, Vec<u32>>,
    tree: &HashMap<u32, (u32, String)>,
) -> HashMap<String, Vec<Tap>> {
    taps.iter()
        .map(|(id, pids)| {
            let named = pids
                .iter()
                .map(|pid| Tap {
                    pid: *pid,
                    name: tree
                        .get(pid)
                        .map(|(_, exe)| exe.trim_end_matches(".exe").to_string())
                        .unwrap_or_else(|| format!("PID {pid}")),
                })
                .collect();
            (id.clone(), named)
        })
        .collect()
}

/// A note on one source, written to follow its name ("Discord is not
/// running — …"), the way every note the core gives reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub text: String,
    /// Only for the source's row in the mixer: a state the user chose (an
    /// application closed for now), nothing to raise over a game or on the
    /// Video page.
    pub mixer_only: bool,
}

impl Note {
    fn everywhere(text: String) -> Self {
        Note { text, mixer_only: false }
    }

    fn mixer(text: String) -> Self {
        Note { text, mixer_only: true }
    }
}

/// The applications another source records: the detected game — when a Game
/// source or an output device (which takes the leftovers, game included) records
/// it; with neither, a launcher picked by hand is the only way it is heard — and
/// every application picked by hand that runs. A tap on an ancestor of one of
/// them would record it a second time.
fn recorded_roots(
    sources: &[AudioSource],
    game_pid: Option<u32>,
    tree: &HashMap<u32, (u32, String)>,
    own: u32,
) -> Vec<u32> {
    let game_recorded = sources
        .iter()
        .filter(|s| s.enabled)
        .any(|s| matches!(s.kind, SourceKind::Game | SourceKind::OutputDevice { .. }));
    let mut out: Vec<u32> = game_pid
        .filter(|_| game_recorded)
        .map(|pid| app_root(pid, tree))
        .into_iter()
        .collect();
    for source in sources.iter().filter(|s| s.enabled) {
        if let SourceKind::Process {
            pid,
            exe,
            mode: ProcessMode::Include,
        } = &source.kind
        {
            out.extend(live_pid(own, *pid, exe.as_deref(), tree).map(|pid| app_root(pid, tree)));
        }
    }
    out
}

/// The application of `others` that a tap on `root` would take in with its
/// tree — a launcher picked as a source, with the game it started below it.
fn swallowed(root: u32, others: &[u32], tree: &HashMap<u32, (u32, String)>) -> Option<u32> {
    others
        .iter()
        .copied()
        .find(|other| *other != root && ancestors(*other, tree).contains(&root))
}

/// [`swallowed`] said no to a tap: once per pair in the log, not every 2 s.
fn log_left_out(label: &str, root: u32, other: u32) {
    static SEEN: std::sync::Mutex<Vec<(u32, u32)>> = std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(|poison| poison.into_inner());
    if !seen.contains(&(root, other)) {
        seen.push((root, other));
        log::info!(
            "'{label}' (process {root}) started process {other}, which is recorded already — not tapped, it would be in the clip twice"
        );
    }
}

/// The notes about application sources: one nothing matches while the buffer
/// runs (that track is written, and it is silence — mixer only), one stored
/// under a bare helper name that cannot be found again, one left out because
/// its tap would take in another recorded application (mixer only), and the
/// second of two sources that end up on the same application.
pub fn missing_app_notes(
    sources: &[AudioSource],
    game_pid: Option<u32>,
    tree: &HashMap<u32, (u32, String)>,
    own: u32,
    buffering: bool,
) -> HashMap<String, Note> {
    let mut out = HashMap::new();
    let others = recorded_roots(sources, game_pid, tree, own);
    // Who has which application, in the order the engine claims them.
    let mut owner: HashMap<u32, &str> = HashMap::new();
    for source in sources.iter().filter(|s| s.enabled) {
        let root = match &source.kind {
            SourceKind::Game => game_pid.map(|pid| app_root(pid, tree)),
            SourceKind::Process {
                pid,
                exe,
                mode: ProcessMode::Include,
            } => {
                let live = live_pid(own, *pid, exe.as_deref(), tree);
                if live.is_none() && buffering {
                    let helper = exe.as_deref().is_some_and(is_audio_helper);
                    out.insert(
                        source.id.clone(),
                        if helper {
                            Note::everywhere("has to be added again from the list".to_string())
                        } else {
                            Note::mixer("is not running — this track stays silent".to_string())
                        },
                    );
                }
                let root = live.map(|pid| app_root(pid, tree));
                if let Some(other) = root.and_then(|root| swallowed(root, &others, tree)) {
                    let name = tree
                        .get(&other)
                        .map(|(_, exe)| exe.trim_end_matches(".exe").to_string())
                        .unwrap_or_else(|| format!("PID {other}"));
                    out.insert(
                        source.id.clone(),
                        Note::mixer(format!("is left out — it would record {name} a second time")),
                    );
                    continue;
                }
                root
            }
            _ => None,
        };
        let Some(root) = root else { continue };
        match owner.get(&root) {
            Some(winner) => {
                out.insert(
                    source.id.clone(),
                    Note::everywhere(format!("is already recorded by {winner}")),
                );
            }
            None => {
                owner.insert(root, source.label.as_str());
            }
        }
    }
    out
}

/// Sources stored under a helper's exe (`msedgewebview2.exe`) by an older
/// version, whose helper still runs: the host application's exe to store
/// instead, as (source id, new pid, new exe, new label when the label was the
/// helper's name). Empty once migrated, so applying it again changes nothing,
/// and empty where an application cannot be tapped at all (Windows 10): there
/// the source records nothing either way, and is left as the user stored it.
pub fn helper_migrations(
    sources: &[AudioSource],
    tree: &HashMap<u32, (u32, String)>,
    own: u32,
    supported: bool,
) -> Vec<(String, u32, String, Option<String>)> {
    let mut out = Vec::new();
    if !supported {
        return out;
    }
    for source in sources.iter().filter(|s| s.enabled) {
        let SourceKind::Process { pid, exe: Some(exe), .. } = &source.kind else {
            continue;
        };
        if !is_audio_helper(exe) {
            continue;
        }
        let Some((_, running)) = tree.get(pid) else { continue };
        if !running.eq_ignore_ascii_case(exe) {
            continue;
        }
        let host = app_of(*pid, tree);
        let Some((_, host_exe)) = tree.get(&host) else { continue };
        if host == *pid || is_audio_helper(host_exe) || in_tree_of(host, own, tree) {
            continue;
        }
        let stem = exe.trim_end_matches(".exe");
        let label = (source.label.eq_ignore_ascii_case(stem)
            || source.label.eq_ignore_ascii_case(exe))
        .then(|| host_exe.trim_end_matches(".exe").to_string());
        out.push((source.id.clone(), host, host_exe.clone(), label));
    }
    out
}

/// One of [`helper_migrations`] applied to the source as it is stored *now*.
///
/// `before` is the source the migration was worked out from; `current` the
/// same id read again right before the write. Anything changed in between — a
/// rename, another pick, the source deleted — and nothing is written: the
/// migration would put back a stale copy. The next tick works it out afresh.
pub fn migrated_source(
    before: &AudioSource,
    current: Option<&AudioSource>,
    pid: u32,
    exe: String,
    label: Option<String>,
) -> Option<AudioSource> {
    let current = current.filter(|now| *now == before)?;
    let SourceKind::Process { exe: Some(old), mode, .. } = &current.kind else {
        return None;
    };
    if !is_audio_helper(old) {
        return None;
    }
    let mut source = current.clone();
    source.kind = SourceKind::Process {
        pid,
        exe: Some(exe),
        mode: *mode,
    };
    if let Some(label) = label {
        source.label = label;
    }
    Some(source)
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
    records_single_applications_on(sources, app_capture_ok())
}

/// The first Windows build with process loopback — Windows 11. Before it a
/// per-application tap cannot be opened at all.
const APP_CAPTURE_MIN_BUILD: u32 = 20348;

/// Can a single application be recorded on this Windows build?
pub fn app_capture_supported(build: u32) -> bool {
    build >= APP_CAPTURE_MIN_BUILD
}

/// [`app_capture_supported`] for the machine we run on, read once. Where the
/// build cannot be read, assume it works: that is what the app did before.
pub fn app_capture_ok() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| os_build().map_or(true, app_capture_supported))
}

#[cfg(windows)]
fn os_build() -> Option<u32> {
    use windows::Wdk::System::SystemServices::RtlGetVersion;
    use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is a valid, correctly sized OSVERSIONINFOW.
    let status = unsafe { RtlGetVersion(&mut info) };
    status.is_ok().then_some(info.dwBuildNumber)
}

#[cfg(not(windows))]
fn os_build() -> Option<u32> {
    None
}

/// The note on every Game or Process source where per-app capture does not
/// exist. Empty where it does.
pub fn app_capture_notes(sources: &[AudioSource], supported: bool) -> HashMap<String, String> {
    if supported {
        return HashMap::new();
    }
    sources
        .iter()
        .filter(|s| s.enabled && is_app_source(&s.kind))
        .map(|s| {
            (
                s.id.clone(),
                "records nothing — per-app audio needs Windows 11".to_string(),
            )
        })
        .collect()
}

fn is_app_source(kind: &SourceKind) -> bool {
    matches!(kind, SourceKind::Game | SourceKind::Process { .. })
}

fn records_single_applications_on(sources: &[AudioSource], supported: bool) -> bool {
    supported && sources.iter().filter(|s| s.enabled).any(|source| {
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

/// The sources with every "Windows default" device (an empty id) replaced by
/// the real default device, so the engine sees which device a stream is on and
/// restarts it when the default changes. Only for the engine: the result is
/// never written to the configuration, where `""` must stay "follow Windows".
/// An empty `output` or `input` leaves that kind alone.
pub fn pin_defaults(sources: &[AudioSource], output: &str, input: &str) -> Vec<AudioSource> {
    sources
        .iter()
        .map(|source| {
            let mut source = source.clone();
            match &mut source.kind {
                SourceKind::OutputDevice { device_id } if device_id.is_empty() && !output.is_empty() => {
                    *device_id = output.to_string();
                }
                SourceKind::InputDevice { device_id } if device_id.is_empty() && !input.is_empty() => {
                    *device_id = input.to_string();
                }
                _ => {}
            }
            source
        })
        .collect()
}

/// Remembers the last non-empty default per half: a device list that fails for
/// a moment (or a default that is briefly gone) must not read as "no default"
/// and drop every stream that follows it.
fn keep_good(last: &mut (String, String), now: (String, String)) -> (String, String) {
    if !now.0.is_empty() {
        last.0 = now.0;
    }
    if !now.1.is_empty() {
        last.1 = now.1;
    }
    last.clone()
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
/// Always the application's root, never one of its children: Discord plays
/// from utility children of one `Discord.exe`, and only a tap on their common
/// parent (with `INCLUDE_TARGET_PROCESS_TREE`) has all of its sessions. The
/// picker stores the pid of a session, which is such a child, so even a living
/// pid goes up to its root.
///
/// Where several processes share the name, only the roots among them count.
/// Pids say nothing about age on Windows, and taking the lowest one often
/// landed on a child without the voice session — a silent Discord track after
/// every restart. The lowest pid only breaks the tie between several roots.
fn live_pid(
    own: u32,
    pid: u32,
    exe: Option<&str>,
    tree: &HashMap<u32, (u32, String)>,
) -> Option<u32> {
    // "PID 1234" is what the picker stored where it could not read the name —
    // a game behind anti-cheat. No process is called that, so searching by it
    // dropped the source even while the application ran.
    let exe = exe.filter(|name| !is_pid_placeholder(name));
    let matches_exe = |name: &str| {
        exe.is_some_and(|want| name.eq_ignore_ascii_case(want))
    };
    // ClippiBoy's own web views are never what a source means.
    let outside = |pid: u32| !in_tree_of(pid, own, tree);
    match tree.get(&pid) {
        // Nothing to compare the name with: the pid as it stands.
        Some(_) if exe.is_none() => return Some(pid),
        // Still there and still itself.
        Some((_, name)) if matches_exe(name) && outside(pid) => {
            let root = app_of(pid, tree);
            return outside(root).then_some(root);
        }
        // The pid is taken by something else, or gone. Fall through.
        _ => {}
    }
    let Some(exe) = exe else {
        // Nothing to search by — a source from before the exe was stored. It
        // keeps the old behaviour: the pid as it stands, and an error from the
        // stream if that pid is dead.
        return Some(pid);
    };
    // A bare helper name says nothing about whose helper it was.
    if is_audio_helper(exe) {
        return None;
    }
    let found = tree
        .iter()
        .filter(|(_, (_, name))| name.eq_ignore_ascii_case(exe))
        .map(|(pid, _)| app_of(*pid, tree))
        .filter(|root| outside(*root))
        .min();
    if let Some(now) = found {
        log::info!("'{exe}' is {now} now (was {pid}) — the source follows it");
    }
    found
}

/// The stand-in `devices::list_processes` gives an application whose name it
/// cannot read — what older versions stored for every game behind anti-cheat.
fn is_pid_placeholder(exe: &str) -> bool {
    exe.strip_prefix("PID ")
        .is_some_and(|rest| rest.parse::<u32>().is_ok())
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
        // The other way round too: a tap on an application *above* one another
        // source records takes that one in as well. Launchers start the game as
        // their child — Steam, Battle.net, the Riot Client — and they hold
        // sessions of their own, so the launcher came up as a leftover, was
        // tapped with its whole tree, and the game landed in the clip twice:
        // muting the game track left it playing in the mix. Such a root is
        // skipped; its other children are roots of their own and get picked up
        // one by one. Only `taken` needs this — leftovers go shallowest first,
        // so nothing chosen below can sit under a later one.
        let above_taken: Vec<u32> = taken.iter().flat_map(|pid| ancestors(*pid, tree)).collect();
        for root in roots {
            // A tap covers the target and its children, so anything below an
            // application another source records is already in the clip.
            let covered = spoken_for.contains(&root)
                || above_taken.contains(&root)
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
    resolve_for(
        std::process::id(),
        app_capture_ok(),
        sources,
        game_pid,
        playing,
        tree,
        ledger,
    )
}

/// [`resolve`] with ClippiBoy's own process given, so a test's made-up pids
/// can never be taken for it.
fn resolve_for(
    own: u32,
    supported: bool,
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
    //
    // An application picked by hand whose tap would take in the game or another
    // recorded application with its tree — Steam, with the game it started —
    // is left out altogether, here and below: tapped, the game would be in the
    // clip twice, and as taken, the leftovers would skip what sits below it.
    let others = recorded_roots(sources, game_pid, tree, own);
    let mut taken: Vec<u32> = vec![own];
    for source in enabled() {
        match &source.kind {
            // The application, not the one process: a game that runs as
            // several processes of the same exe is all of them.
            SourceKind::Game => taken.extend(game_pid.map(|pid| app_root(pid, tree))),
            // The live pid, not the stored one — otherwise a rebound source
            // would be tapped here and recorded by the leftovers as well.
            SourceKind::Process {
                pid,
                exe,
                mode: ProcessMode::Include,
            } => taken.extend(
                live_pid(own, *pid, exe.as_deref(), tree)
                    .map(|pid| app_root(pid, tree))
                    .filter(|root| swallowed(*root, &others, tree).is_none()),
            ),
            _ => {}
        }
    }

    // Which output device each application belongs to, decided once for all of
    // them so no two can claim the same one. Empty unless something is recorded
    // application by application — then a device is simply itself.
    let leftovers_mode = records_single_applications_on(sources, supported);
    let home = if supported {
        ledger.assign(sources, playing, tree, &taken)
    } else {
        Vec::new()
    };

    let mut out = Vec::with_capacity(sources.len());
    // The applications a source of its own taps, so two sources that end up on
    // the same one (two entries for Discord, or a game that is also picked by
    // hand) are not both given a tap: that records it twice, about 6 dB louder.
    let mut tapped: Vec<u32> = Vec::new();
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
            // Without process loopback an application cannot be tapped; the
            // source stays (with its note) and records nothing.
            kind if !supported && is_app_source(kind) => {}
            SourceKind::Game => {
                if let Some(root) = game_pid.map(|pid| app_root(pid, tree)) {
                    if claim(&mut tapped, root, &source.label) {
                        out.push(include(root));
                    }
                }
            }
            SourceKind::OutputDevice { .. } if leftovers_mode => out.extend(leftovers()),
            // Only ever read from an old configuration; `config::migrate_sources`
            // turns it into a plain output device before it gets here.
            SourceKind::Leftovers => out.extend(leftovers()),
            // An application picked by hand: the stored pid is only a starting
            // point. Restarting Discord used to leave this source dead for good.
            SourceKind::Process { pid, exe, mode } => {
                if let Some(now) = live_pid(own, *pid, exe.as_deref(), tree) {
                    if *mode == ProcessMode::Include {
                        let root = app_root(now, tree);
                        if let Some(other) = swallowed(root, &others, tree) {
                            log_left_out(&source.label, root, other);
                            continue;
                        }
                        // Only a source that taps claims; "everything except" does not.
                        if !claim(&mut tapped, root, &source.label) {
                            continue;
                        }
                    }
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

/// Marks `root` as tapped; false (and a log line) when another source has it.
fn claim(tapped: &mut Vec<u32>, root: u32, label: &str) -> bool {
    if tapped.contains(&root) {
        // Every 2 s while it lasts: debug, the UI says it in the row's note.
        log::debug!("'{label}' is already recorded by another source (process {root}) — tapped once");
        return false;
    }
    tapped.push(root);
    true
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

    /// Several of the same name: the root wins, not the lowest pid — pids say
    /// nothing about age on Windows.
    #[test]
    fn of_several_namesakes_the_root_wins() {
        let sources = vec![app_source("chat", 1, Some("Discord.exe"))];
        let tree = tree(&[
            (700, 12800, "Discord.exe"),
            (300, 700, "Discord.exe"),
            (12800, 0, "explorer.exe"),
        ]);
        let streams = resolve(&sources, None, &nothing(), &tree, &ledger(""));
        assert_eq!(pid_of(&streams, "chat"), Some(700));
    }

    /// Two separate roots of the same name: the lowest pid breaks the tie.
    #[test]
    fn between_several_roots_the_lowest_pid_wins() {
        let sources = vec![app_source("chat", 1, Some("Discord.exe"))];
        let tree = tree(&[(700, 1, "Discord.exe"), (300, 1, "Discord.exe")]);
        let streams = resolve(&sources, None, &nothing(), &tree, &ledger(""));
        assert_eq!(pid_of(&streams, "chat"), Some(300));
    }

    /// The real Discord after a restart: the lowest pid is a utility child, and
    /// tapping it recorded a child without the voice session — a silent track.
    #[test]
    fn a_restarted_discord_is_tapped_at_its_root() {
        let sources = vec![app_source("chat", 4321, Some("Discord.exe"))];
        let streams = resolve(&sources, None, &nothing(), &discord(), &ledger(""));
        assert_eq!(pid_of(&streams, "chat"), Some(5556));
    }

    /// The picker stores the pid of a session, which for Discord is a utility
    /// child. Alive as it is, the tap still goes on the root that has both
    /// sessions.
    #[test]
    fn a_living_child_goes_up_to_its_root() {
        let sources = vec![app_source("chat", 30988, Some("Discord.exe"))];
        let streams = resolve(&sources, None, &nothing(), &discord(), &ledger(""));
        assert_eq!(pid_of(&streams, "chat"), Some(5556));
    }

    /// The Discord source and an output device's leftovers side by side: the
    /// sessions sit in the children, and neither may land in the device too.
    #[test]
    fn a_discord_source_keeps_its_children_out_of_the_leftovers() {
        let sources = vec![
            app_source("chat", 4321, Some("Discord.exe")),
            of_kind("system", output("dev-1")),
        ];
        let playing: HashMap<String, Vec<(u32, bool)>> =
            [("dev-1".to_string(), vec![(2584, true), (30988, true)])]
                .into_iter()
                .collect();
        let streams = resolve(&sources, None, &playing, &discord(), &ledger("dev-1"));
        assert_eq!(pid_of(&streams, "chat"), Some(5556));
        assert_eq!(pids_of(&streams, "system"), Vec::<u32>::new());
    }

    /// A game behind anti-cheat could not be named, and the picker stored
    /// "PID 1234" as its exe. No process is called that: searching by it took
    /// the source out while the game was running.
    #[test]
    fn a_placeholder_exe_keeps_the_living_pid() {
        let sources = vec![app_source("game", 4321, Some("PID 4321"))];
        let tree = tree(&[(4321, 1, "VALORANT-Win64-Shipping.exe")]);
        let streams = resolve(&sources, None, &nothing(), &tree, &ledger(""));
        assert_eq!(pid_of(&streams, "game"), Some(4321));
        assert!(is_pid_placeholder("PID 4321"));
        assert!(!is_pid_placeholder("PID.exe"));
        assert!(!is_pid_placeholder("Discord.exe"));
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
    /// What the tests take for ClippiBoy's own process: not a multiple of 4,
    /// so Windows never hands it out and no made-up pid can be it.
    const NOT_US: u32 = u32::MAX - 2;

    fn resolve(
        sources: &[AudioSource],
        game_pid: Option<u32>,
        playing: &HashMap<String, Vec<(u32, bool)>>,
        tree: &HashMap<u32, (u32, String)>,
        ledger: &Leftovers,
    ) -> Vec<Stream> {
        resolve_for(NOT_US, true, sources, game_pid, playing, tree, ledger)
    }

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

    fn output_source(id: &str, device_id: &str) -> AudioSource {
        of_kind(id, output(device_id))
    }

    fn input_source(id: &str, device_id: &str) -> AudioSource {
        of_kind(id, SourceKind::InputDevice { device_id: device_id.into() })
    }

    #[test]
    fn a_windows_default_source_names_the_real_device() {
        let sources = vec![output_source("sys", ""), input_source("mic", ""), output_source("chat", "chat-dev")];
        let pinned = pin_defaults(&sources, "spk-2", "mic-1");
        assert!(matches!(&pinned[0].kind, SourceKind::OutputDevice { device_id } if device_id == "spk-2"));
        assert!(matches!(&pinned[1].kind, SourceKind::InputDevice { device_id } if device_id == "mic-1"));
        assert!(matches!(&pinned[2].kind, SourceKind::OutputDevice { device_id } if device_id == "chat-dev"));
        // No known default: the source stays "follow Windows".
        let unpinned = pin_defaults(&sources, "", "");
        assert_eq!(unpinned[0].kind, sources[0].kind);
    }

    #[test]
    fn an_empty_default_keeps_the_last_good_one() {
        let mut last = ("spk-1".to_string(), "mic-1".to_string());
        assert_eq!(
            keep_good(&mut last, ("".into(), "mic-2".into())),
            ("spk-1".to_string(), "mic-2".to_string())
        );
    }

    #[test]
    fn something_playing_nowhere_lands_on_a_windows_default_bucket() {
        // System = Windows default, plus a "Chat" device; Steam idles on both.
        let sources = pin_defaults(
            &[
                recording_by_app(),
                output_source("system", ""),
                output_source("chat", "chat-dev"),
            ],
            "spk",
            "",
        );
        let streams = resolve(
            &sources,
            None,
            &playing(&[("spk", &idle(&[42])), ("chat-dev", &idle(&[42]))]),
            &tree(&[(42, 1, "steam.exe")]),
            &ledger("spk"),
        );
        assert_eq!(pids_of(&streams, "system"), vec![42]);
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

    /// Steam started the game, and Steam holds sessions of its own. Tapped
    /// with its tree, it would bring the game into the leftovers a second time.
    #[test]
    fn a_launcher_above_the_game_is_not_tapped_with_it() {
        let processes = tree(&[
            (33264, 0, "steam.exe"),
            (12604, 33264, "steamwebhelper.exe"),
            (5000, 33264, "cs2.exe"),
        ]);
        let sources = vec![of_kind("game", SourceKind::Game), of_kind("rest", leftovers("spk"))];
        let streams = resolve(
            &sources,
            Some(5000),
            &playing(&[("spk", &idle(&[33264, 12604, 5000]))]),
            &processes,
            &ledger("spk"),
        );
        assert_eq!(pids_of(&streams, "game"), vec![5000]);
        // Steam's helper is still recorded, on its own; Steam itself is not.
        assert_eq!(pids_of(&streams, "rest"), vec![12604]);
    }

    #[test]
    fn app_capture_needs_windows_11_build() {
        assert!(!app_capture_supported(19045));
        assert!(!app_capture_supported(20347));
        assert!(app_capture_supported(20348));
        assert!(app_capture_supported(22631));
    }

    /// Windows 10: devices are recorded whole and the application sources
    /// record nothing, so desktop and game sound stay in the clip.
    #[test]
    fn without_process_loopback_devices_stay_whole() {
        let sources = vec![
            recording_by_app(),
            of_kind("proc", SourceKind::Process {
                pid: 77,
                exe: None,
                mode: ProcessMode::Include,
            }),
            of_kind("spk", SourceKind::OutputDevice { device_id: "spk".into() }),
        ];
        let procs = tree(&[(5000, 4, "game.exe"), (77, 4, "discord.exe")]);
        let streams = resolve_for(
            NOT_US,
            false,
            &sources,
            Some(5000),
            &playing(&[("spk", &idle(&[5000, 77, 900]))]),
            &procs,
            &ledger("spk"),
        );
        assert!(streams.iter().all(|s| s.source_id == "spk"));
        assert_eq!(streams.len(), 1);
        assert!(matches!(
            streams[0].kind,
            SourceKind::OutputDevice { ref device_id } if device_id == "spk"
        ));
        assert!(!records_single_applications_on(&sources, false));
        assert!(records_single_applications_on(&sources, true));
    }

    #[test]
    fn unsupported_application_sources_get_a_note() {
        let sources = vec![
            of_kind("game", SourceKind::Game),
            of_kind("spk", SourceKind::OutputDevice { device_id: "spk".into() }),
        ];
        let notes = app_capture_notes(&sources, false);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes["game"], "records nothing — per-app audio needs Windows 11");
        assert!(app_capture_notes(&sources, true).is_empty());
    }

    /// The tests' own process is never one of their made-up applications: on a
    /// fresh CI machine the test binary ran as pid 5000 — the game of
    /// `a_rebound_application_is_not_recorded_twice` — and was left out as
    /// ClippiBoy itself.
    #[test]
    fn only_the_process_named_as_ours_is_left_out() {
        let sources = vec![recording_by_app(), of_kind("rest", leftovers("spk"))];
        let streams = resolve_for(
            5000,
            true,
            &sources,
            None,
            &playing(&[("spk", &idle(&[5000, 5004]))]),
            &loose(),
            &ledger("spk"),
        );
        assert_eq!(pids_of(&streams, "rest"), vec![5004]);
        assert_eq!(pids_of(&resolve(&sources, None, &playing(&[("spk", &idle(&[5000, 5004]))]), &loose(), &ledger("spk")), "rest"), vec![5000, 5004]);
    }

    /// Playing a clip back while the buffer runs must not end up in the next
    /// one.
    #[test]
    fn the_leftovers_never_record_clippiboy_itself() {
        let sources = vec![recording_by_app(), of_kind("rest", leftovers("spk"))];
        let own = std::process::id();
        let streams = super::resolve(
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

    // ---- the picker's grouping and helper processes ----

    fn seen(pid: u32, active: bool, display: Option<&str>) -> SeenSession {
        SeenSession {
            pid,
            active,
            display: display.map(str::to_string),
            exe: None,
        }
    }

    /// Discord's two sessions are one application, listed under its root.
    #[test]
    fn two_sessions_of_one_application_are_one_row() {
        let sessions = [seen(2584, false, None), seen(30988, true, None)];
        let rows = group_apps(&sessions, &discord(), 9999);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, 5556);
        assert_eq!(rows[0].exe, "Discord.exe");
        assert_eq!(rows[0].count, 2);
        assert!(rows[0].active);
        assert_eq!(rows[0].name, "Discord");
    }

    /// An embedded web view plays for the application that hosts it.
    #[test]
    fn a_helper_is_listed_as_the_application_that_started_it() {
        let tree = tree(&[
            (10, 1, "Spotify.exe"),
            (11, 10, "Spotify.exe"),
            (12, 11, "msedgewebview2.exe"),
            (13, 12, "msedgewebview2.exe"),
        ]);
        let rows = group_apps(&[seen(13, true, Some("Edge WebView"))], &tree, 9999);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, 10);
        assert_eq!(rows[0].exe, "Spotify.exe");
        assert_eq!(rows[0].name, "Spotify", "a helper's own name is not the app's");
        assert_eq!(app_of(13, &tree), 10);
        // The leftovers' notion of a root is unchanged: the helper's own.
        assert_eq!(app_root(13, &tree), 12);
    }

    /// The helper walk stops at the shell.
    #[test]
    fn a_helper_never_climbs_into_the_shell() {
        let tree = tree(&[(5, 1, "explorer.exe"), (6, 5, "msedgewebview2.exe")]);
        assert_eq!(app_of(6, &tree), 6);
    }

    /// ClippiBoy's own web views are never listed or picked.
    #[test]
    fn clippiboys_own_tree_is_left_out() {
        let tree = tree(&[
            (100, 1, "clippiboy.exe"),
            (101, 100, "msedgewebview2.exe"),
            (102, 101, "msedgewebview2.exe"),
            (200, 1, "Spotify.exe"),
        ]);
        let rows = group_apps(&[seen(102, true, None), seen(200, true, None)], &tree, 100);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].exe, "Spotify.exe");
    }

    /// A source stored with a webview exe is not rebound to some app's web view,
    /// and a namesake inside ClippiBoy's tree is not taken either.
    #[test]
    fn a_source_never_finds_a_process_of_clippiboys_own_tree() {
        let tree = tree(&[
            (100, 1, "clippiboy.exe"),
            (101, 100, "msedgewebview2.exe"),
            (300, 1, "msedgewebview2.exe"),
        ]);
        assert_eq!(live_pid(100, 7, Some("msedgewebview2.exe"), &tree), None);
        // Stored pid is alive but inside our tree: it is not followed.
        let tree2 = self::tree(&[(100, 1, "clippiboy.exe"), (101, 100, "chat.exe"), (400, 1, "chat.exe")]);
        assert_eq!(live_pid(100, 101, Some("chat.exe"), &tree2), Some(400));
    }

    /// Display names like "@%SystemRoot%\..." are resource references.
    #[test]
    fn resource_display_names_fall_back_to_the_exe() {
        let tree = tree(&[(10, 1, "Game.exe")]);
        let rows = group_apps(&[seen(10, false, Some("@%SystemRoot%\\x.dll,-1"))], &tree, 9999);
        assert_eq!(rows[0].name, "Game");
    }

    /// Two sources on the same application tap it once.
    #[test]
    fn a_pid_tapped_by_two_sources_is_tapped_once() {
        let sources = vec![
            app_source("a", 30988, Some("Discord.exe")),
            app_source("b", 2584, Some("Discord.exe")),
        ];
        let streams = resolve_for(9999, true, &sources, None, &nothing(), &discord(), &ledger(""));
        assert_eq!(pid_of(&streams, "a"), Some(5556));
        assert_eq!(pid_of(&streams, "b"), None);
    }

    #[test]
    fn a_game_and_a_picked_process_tap_the_application_once() {
        let sources = vec![
            of_kind("game", SourceKind::Game),
            app_source("chat", 5556, Some("Discord.exe")),
        ];
        let streams =
            resolve_for(9999, true, &sources, Some(30988), &nothing(), &discord(), &ledger(""));
        assert_eq!(pid_of(&streams, "game"), Some(5556));
        assert_eq!(pid_of(&streams, "chat"), None);
    }

    #[test]
    fn a_missing_application_gets_a_note_only_while_buffering() {
        let mut gone = app_source("chat", 4321, Some("Discord.exe"));
        gone.label = "Discord".into();
        let sources = vec![gone];
        let tree = tree(&[(4321, 1, "notepad.exe")]);
        let notes = missing_app_notes(&sources, None, &tree, 9999, true);
        // Follows the name ("Discord is not running …"), and only in the mixer.
        assert_eq!(
            notes.get("chat"),
            Some(&Note::mixer("is not running — this track stays silent".into()))
        );
        assert!(missing_app_notes(&sources, None, &tree, 9999, false).is_empty());
        assert!(missing_app_notes(&sources, None, &discord(), 9999, true).is_empty());
    }

    #[test]
    fn taps_are_named_by_their_exe() {
        let taps: HashMap<String, Vec<u32>> = [("chat".to_string(), vec![5556, 999])]
            .into_iter()
            .collect();
        let named = name_taps(&taps, &discord());
        assert_eq!(named["chat"][0], Tap { pid: 5556, name: "Discord".into() });
        assert_eq!(named["chat"][1].name, "PID 999");
    }

    // ---- fix round 1 ----

    #[test]
    fn a_helper_source_that_cannot_be_found_says_to_add_it_again() {
        let mut src = app_source("web", 77, Some("msedgewebview2.exe"));
        src.label = "Spotify".into();
        let notes = missing_app_notes(&[src], None, &tree(&[(5, 1, "x.exe")]), 9999, true);
        assert_eq!(notes["web"], Note::everywhere("has to be added again from the list".into()));
    }

    #[test]
    fn a_living_helper_source_is_moved_to_its_host() {
        let tree = tree(&[(10, 1, "Spotify.exe"), (12, 10, "msedgewebview2.exe")]);
        let mut src = app_source("web", 12, Some("msedgewebview2.exe"));
        src.label = "msedgewebview2".into();
        let moves = helper_migrations(&[src.clone()], &tree, 9999, true);
        assert_eq!(moves, vec![("web".into(), 10, "Spotify.exe".into(), Some("Spotify".to_string()))]);
        // Windows 10: nothing can be tapped, nothing is rewritten.
        assert!(helper_migrations(&[src.clone()], &tree, 9999, false).is_empty());
        // A name the user chose stays.
        src.label = "Music".into();
        assert_eq!(helper_migrations(&[src], &tree, 9999, true)[0].3, None);
        // Migrated already: nothing left to do.
        let done = app_source("web", 10, Some("Spotify.exe"));
        assert!(helper_migrations(&[done], &tree, 9999, true).is_empty());
        // Dead helper: nothing to resolve.
        let gone = app_source("web", 99, Some("msedgewebview2.exe"));
        assert!(helper_migrations(&[gone], &tree, 9999, true).is_empty());
    }

    #[test]
    fn an_exclude_source_never_claims_an_application() {
        let mut ex = app_source("rest", 5556, Some("Discord.exe"));
        ex.kind = SourceKind::Process {
            pid: 5556,
            exe: Some("Discord.exe".into()),
            mode: ProcessMode::Exclude,
        };
        let sources = vec![ex, app_source("chat", 5556, Some("Discord.exe"))];
        let streams = resolve_for(9999, true, &sources, None, &nothing(), &discord(), &ledger(""));
        assert_eq!(pid_of(&streams, "chat"), Some(5556));
    }

    #[test]
    fn the_helper_stays_put_below_a_system_host() {
        for host in ["svchost.exe", "RuntimeBroker.exe", "sihost.exe", "dwm.exe"] {
            let tree = tree(&[(5, 1, host), (6, 5, "msedgewebview2.exe")]);
            assert_eq!(app_of(6, &tree), 6, "{host}");
        }
    }

    #[test]
    fn the_losing_duplicate_says_who_has_the_application() {
        let mut a = app_source("a", 30988, Some("Discord.exe"));
        a.label = "Discord".into();
        let b = app_source("b", 2584, Some("Discord.exe"));
        let notes = missing_app_notes(&[a, b], None, &discord(), 9999, false);
        assert_eq!(notes.get("b"), Some(&Note::everywhere("is already recorded by Discord".into())));
        assert!(!notes.contains_key("a"));
    }

    #[test]
    fn a_picker_row_whose_host_is_gone_is_left_out() {
        let tree = tree(&[(5, 1, "explorer.exe"), (6, 5, "msedgewebview2.exe")]);
        assert!(group_apps(&[seen(6, true, None)], &tree, 9999).is_empty());
    }

    // ---- final fix wave: launchers and their helpers ----

    /// Steam as it runs: its web view in two processes, the game a child of
    /// Steam itself.
    fn steam_with_game() -> HashMap<u32, (u32, String)> {
        tree(&[
            (1, 0, "explorer.exe"),
            (10, 1, "steam.exe"),
            (11, 10, "steamwebhelper.exe"),
            (12, 11, "steamwebhelper.exe"),
            (20, 10, "cs2.exe"),
        ])
    }

    /// Steam's web view is an entry of its own: climbing to steam.exe would tap
    /// the game Steam started a second time.
    #[test]
    fn a_launchers_helper_never_climbs_to_the_launcher() {
        let tree = steam_with_game();
        assert_eq!(app_of(12, &tree), 11);
        let rows = group_apps(&[seen(12, true, None)], &tree, 9999);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, 11);
        assert_eq!(rows[0].exe, "steamwebhelper.exe");
        assert_eq!(rows[0].name, "Steam (overlay & voice)");
        // A source stored under it stays where it is and is found again.
        let src = app_source("steam", 11, Some("steamwebhelper.exe"));
        assert!(helper_migrations(std::slice::from_ref(&src), &tree, 9999, true).is_empty());
        assert_eq!(live_pid(9999, 4242, Some("steamwebhelper.exe"), &tree), Some(11));
        let streams = resolve_for(9999, true, &[src], Some(20), &nothing(), &tree, &ledger(""));
        assert_eq!(pid_of(&streams, "steam"), Some(11));
    }

    #[test]
    fn epics_helper_stays_its_own_entry() {
        let tree = tree(&[
            (1, 0, "explorer.exe"),
            (30, 1, "EpicGamesLauncher.exe"),
            (31, 30, "EpicWebHelper.exe"),
            (40, 30, "FortniteClient-Win64-Shipping.exe"),
        ]);
        assert_eq!(app_of(31, &tree), 31);
        assert_eq!(group_apps(&[seen(31, true, None)], &tree, 9999)[0].name, "Epic Games (web)");
        // The launcher itself, picked by hand, would take in the game.
        let sources = vec![
            of_kind("game", SourceKind::Game),
            app_source("epic", 30, Some("EpicGamesLauncher.exe")),
        ];
        let streams = resolve_for(9999, true, &sources, Some(40), &nothing(), &tree, &ledger(""));
        assert_eq!(pid_of(&streams, "game"), Some(40));
        assert_eq!(pid_of(&streams, "epic"), None);
    }

    /// Steam picked as a source while it runs the game: not tapped — the game
    /// would be in the clip twice — and the mixer row says why.
    #[test]
    fn a_launcher_picked_by_hand_never_takes_in_the_game() {
        let tree = steam_with_game();
        let mut steam = app_source("steam", 10, Some("steam.exe"));
        steam.label = "Steam".into();
        let sources = vec![
            of_kind("game", SourceKind::Game),
            steam,
            of_kind("rest", leftovers("spk")),
        ];
        let playing = playing(&[("spk", &idle(&[10, 12, 20]))]);
        let streams = resolve_for(9999, true, &sources, Some(20), &playing, &tree, &ledger("spk"));
        assert_eq!(pids_of(&streams, "game"), vec![20]);
        assert!(pids_of(&streams, "steam").is_empty());
        // The leftovers still get Steam's web view, and the game only once.
        assert_eq!(pids_of(&streams, "rest"), vec![11]);
        let notes = missing_app_notes(&sources, Some(20), &tree, 9999, true);
        assert_eq!(
            notes.get("steam"),
            Some(&Note::mixer("is left out — it would record cs2 a second time".into()))
        );
        // Nothing else records the game: Steam is tapped and carries it, once.
        let streams = resolve_for(9999, true, &sources[1..2], Some(20), &nothing(), &tree, &ledger(""));
        assert_eq!(pids_of(&streams, "steam"), vec![10]);
        assert!(missing_app_notes(&sources[1..2], Some(20), &tree, 9999, true).is_empty());
        // With only a Game source added, Steam is left out again.
        let streams = resolve_for(9999, true, &sources[..2], Some(20), &nothing(), &tree, &ledger(""));
        assert!(pids_of(&streams, "steam").is_empty());
        // No game running: Steam is just an application.
        let streams = resolve_for(9999, true, &sources[1..2], None, &nothing(), &tree, &ledger(""));
        assert_eq!(pids_of(&streams, "steam"), vec![10]);
        assert!(missing_app_notes(&sources[1..2], None, &tree, 9999, true).is_empty());
    }

    /// The same for an application picked by hand below another one.
    #[test]
    fn a_tap_never_takes_in_another_picked_application() {
        let tree = steam_with_game();
        let sources = vec![
            app_source("steam", 10, Some("steam.exe")),
            app_source("cs2", 20, Some("cs2.exe")),
        ];
        let streams = resolve_for(9999, true, &sources, None, &nothing(), &tree, &ledger(""));
        assert_eq!(pid_of(&streams, "cs2"), Some(20));
        assert_eq!(pid_of(&streams, "steam"), None);
    }

    /// Discord was started by a process that has since ended, and Steam,
    /// started later, got that pid: Steam is no parent of Discord, and Discord
    /// is not in Steam's tree. A web view whose host's pid went the same way
    /// stays itself instead of climbing to the System process.
    #[test]
    fn a_parent_created_after_its_child_is_no_parent() {
        let mut processes = tree(&[
            (0, 0, "[System Process]"),
            (1, 0, "explorer.exe"),
            (500, 1, "steam.exe"),
            (40, 500, "Discord.exe"),
            (600, 500, "steamwebhelper.exe"),
            (70, 800, "msedgewebview2.exe"),
            (800, 1, "Spotify.exe"),
        ]);
        let created: HashMap<u32, u64> =
            [(1, 10), (40, 100), (70, 120), (500, 200), (600, 210), (800, 300)]
                .into_iter()
                .collect();
        assert!(in_tree_of(40, 500, &processes));
        cut_reused_parents(&mut processes, &created);
        assert!(!in_tree_of(40, 500, &processes));
        assert_eq!(processes[&40].0, 0);
        assert_eq!(app_of(70, &processes), 70);
        // Real links stay, and so does one whose time could not be read.
        assert!(in_tree_of(600, 500, &processes));
        assert_eq!(processes[&500].0, 1);
        let mut unread = tree(&[(500, 1, "steam.exe"), (40, 500, "Discord.exe")]);
        cut_reused_parents(&mut unread, &[(40, 100)].into_iter().collect());
        assert!(in_tree_of(40, 500, &unread));
    }

    /// A creation time is read once per process and kept while the process
    /// is there; a process that refuses is opened once per snapshot, not once
    /// per child; a pid that now belongs to another process is read anew.
    #[test]
    fn creation_times_are_read_once_and_forgotten_with_the_process() {
        let processes = tree(&[
            (1, 0, "explorer.exe"),
            (500, 1, "launcher.exe"),
            (40, 500, "game.exe"),
            (41, 500, "helper.exe"),
            (42, 500, "helper.exe"),
        ]);
        let mut cache = CreatedCache::new();
        let asked = std::cell::RefCell::new(Vec::new());
        // The launcher (500) will not be opened.
        let lookup = |pid: u32| {
            asked.borrow_mut().push(pid);
            (pid != 500).then_some(u64::from(pid) * 10)
        };
        let created = creation_times(&processes, &mut cache, lookup);
        let mut first = asked.borrow().clone();
        first.sort_unstable();
        assert_eq!(first, [1, 40, 41, 42, 500], "each once, the refusing launcher too");
        assert_eq!(created.len(), 4);
        assert!(!created.contains_key(&500));

        asked.borrow_mut().clear();
        let again = creation_times(&processes, &mut cache, lookup);
        assert_eq!(*asked.borrow(), [500], "only the refusal is asked again");
        assert_eq!(again, created);

        // 42 ended and its pid went to another program; 41 ended.
        let later = tree(&[
            (1, 0, "explorer.exe"),
            (500, 1, "launcher.exe"),
            (40, 500, "game.exe"),
            (42, 1, "other.exe"),
        ]);
        asked.borrow_mut().clear();
        creation_times(&later, &mut cache, lookup);
        let mut third = asked.borrow().clone();
        third.sort_unstable();
        assert_eq!(third, [42, 500]);
        assert!(cache.keys().all(|(pid, _, _)| *pid != 41), "41 is forgotten");
        assert!(cache.contains_key(&(42, 1, "other.exe".to_string())));
        assert!(!cache.contains_key(&(42, 500, "helper.exe".to_string())));
    }

    /// A web view still plays for its host, and the host with nothing recorded
    /// below it is tapped as ever.
    #[test]
    fn a_webview_under_spotify_still_climbs() {
        let tree = tree(&[
            (1, 0, "explorer.exe"),
            (10, 1, "Spotify.exe"),
            (12, 10, "msedgewebview2.exe"),
            (20, 1, "cs2.exe"),
        ]);
        assert_eq!(app_of(12, &tree), 10);
        let sources = vec![
            of_kind("game", SourceKind::Game),
            app_source("music", 12, Some("msedgewebview2.exe")),
        ];
        let streams = resolve_for(9999, true, &sources, Some(20), &nothing(), &tree, &ledger(""));
        assert_eq!(pid_of(&streams, "music"), Some(10));
        assert_eq!(pid_of(&streams, "game"), Some(20));
    }

    /// The migration writes only onto the source as it still is.
    #[test]
    fn a_migration_never_writes_a_stale_copy() {
        let mut before = app_source("web", 12, Some("msedgewebview2.exe"));
        before.label = "msedgewebview2".into();
        let moved =
            migrated_source(&before, Some(&before), 10, "Spotify.exe".into(), Some("Spotify".into()))
                .expect("unchanged source is migrated");
        assert_eq!(moved.label, "Spotify");
        assert_eq!(
            moved.kind,
            SourceKind::Process {
                pid: 10,
                exe: Some("Spotify.exe".into()),
                mode: ProcessMode::Include
            }
        );
        // Renamed in the meantime.
        let mut renamed = before.clone();
        renamed.label = "Music".into();
        assert!(migrated_source(&before, Some(&renamed), 10, "Spotify.exe".into(), None).is_none());
        // Deleted in the meantime.
        assert!(migrated_source(&before, None, 10, "Spotify.exe".into(), None).is_none());
        // No longer a helper (picked again).
        let repicked = app_source("web", 10, Some("Spotify.exe"));
        assert!(migrated_source(&repicked, Some(&repicked), 10, "Spotify.exe".into(), None).is_none());
    }
}
