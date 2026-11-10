//! Reads the hotkeys that are not keyboard combinations: the mouse's middle
//! and side buttons and controller buttons — and times the holds of all of
//! them, keyboard included. What a press means is in `bindings`.
//!
//! No hook: a thread of its own asks `GetAsyncKeyState` for the three mouse
//! buttons about every [`TICK`], the way OBS reads its mouse hotkeys
//! (`libobs/obs-windows.c`), and SDL3's gamepad part (`sdl`) for the
//! controllers in the same round — Xbox pads through XInput, PlayStation,
//! Switch and 8BitDo pads through HID. Nothing is injected, nothing
//! swallowed, no `SendInput`: the game sees every press as before, and no
//! anti-cheat has anything to object to. A hook would add a timeout Windows
//! enforces silently and lag on every mouse move; Raw Input would fight tao's
//! registration.
//!
//! The thread sleeps while nothing needs it: no mouse or controller binding,
//! no keyboard hold being timed, no binding being recorded in the settings,
//! no settings page showing the controllers. SDL is started on the first
//! need of a controller and shut down again [`PAD_IDLE_MS`] after the last.
//!
//! The limit is the same as for every hook: a game running with elevated
//! rights keeps its input from a program that runs without. Then
//! [`warn_elevated`] says so once.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use tauri::{AppHandle, Emitter};

use crate::bindings::{
    Capture, CaptureFilter, Family, Input, PadFrame, PadType, Source, Table, MAX_PADS,
};
use crate::consolenav::{swap_confirm, Navigator};
use crate::sdl::{Gamepad, Sdl};

/// What a hotkey does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    SaveClip,
    Screenshot,
    Record,
    ToggleBuffer,
    Console,
}

impl Action {
    pub const ALL: [Action; 5] = [
        Action::SaveClip,
        Action::ToggleBuffer,
        Action::Screenshot,
        Action::Record,
        Action::Console,
    ];
}

/// How often the buttons are read. A physical click lasts longer than this.
const TICK: Duration = Duration::from_millis(10);
/// How often the list of controllers is looked at for pads that came or went.
/// SDL notices them itself; this only picks up what it found.
const SCAN_MS: u64 = 500;
/// SDL stays up this long after the last need of a controller. While a
/// controller binding exists or the console is open it is needed all along;
/// this is for the settings' list and recording a binding — every start
/// opens the pads anew, and opening writes to some (see `sdl`), so it should
/// not come and go.
const PAD_IDLE_MS: u64 = 5 * 60_000;
/// After SDL failed to start, try again no sooner than this.
const PAD_RETRY_MS: u64 = 30_000;
/// A settings page showing the controllers asks again within this; after it,
/// the list is no longer needed.
const WATCH_MS: u64 = 6_000;
/// The console is steered with a controller while it is open, so the pads
/// are read then even without a controller binding — but only where one may
/// be: with a controller binding, SDL up anyway, or a pad seen this session.
/// A keyboard-only player opening the console does not start SDL (and have
/// it write to pads, see `sdl`) for nothing.
const CONSOLE_NAV_READS_PADS: bool = true;
/// How often the foreground game is checked for running elevated.
const ELEVATION_MS: u64 = 2_000;
/// Recording a binding ends by itself after this long — a page that went
/// away mid-recording must not leave the hotkeys suspended.
const CAPTURE_MS: u64 = 30_000;

static TABLE: Mutex<Option<Table<Action>>> = Mutex::new(None);
/// The settings are recording a new binding: nothing fires.
static SUSPENDED: AtomicBool = AtomicBool::new(false);
/// The settings want the next mouse button or controller combo.
static CAPTURING: AtomicBool = AtomicBool::new(false);
/// The elevated-game notice came already this session.
static WARNED: AtomicBool = AtomicBool::new(false);
/// When recording began, on [`now_ms`].
static CAPTURE_SINCE: AtomicU64 = AtomicU64::new(0);
/// What the recording listens to.
static CAPTURE_FILTER: Mutex<CaptureFilter> = Mutex::new(CaptureFilter::Any);
/// Until when a settings page wants the controller list, on [`now_ms`].
static WATCH_UNTIL: AtomicU64 = AtomicU64::new(0);
/// The controllers as last seen.
static CONTROLLERS: Mutex<Controllers> = Mutex::new(Controllers::EMPTY);
/// The [`Family`] of the first controller, for naming its buttons.
static FAMILY: AtomicU8 = AtomicU8::new(0);
/// [`FAMILY`] is that of a pad a button was pressed on this session.
static FAMILY_PRESSED: AtomicBool = AtomicBool::new(false);
static INBOX: OnceLock<Sender<()>> = OnceLock::new();
static EPOCH: OnceLock<Instant> = OnceLock::new();

/// What the settings get on `input-captured`. No binding: recording was
/// ended from here (timed out, or the window lost the focus).
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CapturedEvent {
    binding: Option<String>,
}

/// The controllers for the settings — `watch_controllers` and the
/// `controllers-changed` event.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Controllers {
    /// SDL is up and has looked (or cannot be started) — before that the
    /// list says nothing.
    pub ready: bool,
    /// Why controllers cannot be read at all (`SDL3.dll` missing, SDL failing
    /// to start) — `None` when they can.
    pub unavailable: Option<String>,
    pub pads: Vec<Controller>,
    /// Whose names buttons are shown with: those of the pad pressed last, or
    /// of the first one before any was pressed.
    pub family: &'static str,
    /// The type of that same pad (`PadType::key`) — `None` with no pad.
    pub in_use: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Controller {
    pub name: String,
    /// `xbox`, `playstation`, `nintendo` or `gamecube` — whose names its
    /// buttons carry.
    pub family: &'static str,
    /// `xbox`, `playstation`, `nintendo` or `other` — which profile it goes
    /// by ([`PadType`]).
    pub pad_type: &'static str,
}

impl Controllers {
    const EMPTY: Controllers = Controllers {
        ready: false,
        unavailable: None,
        pads: Vec::new(),
        family: "xbox",
        in_use: None,
    };
}

/// Milliseconds on one clock for the keyboard events and the poll.
fn now_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

fn nudge() {
    if let Some(inbox) = INBOX.get() {
        let _ = inbox.send(());
    }
}

fn table() -> std::sync::MutexGuard<'static, Option<Table<Action>>> {
    TABLE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Start the thread. Once, at startup.
pub fn start(app: &AppHandle) {
    let (tx, rx) = crossbeam_channel::unbounded();
    if INBOX.set(tx).is_err() {
        return;
    }
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("clippiboy-inputpoll".into())
        .spawn(move || run(&app, &rx));
    if let Err(err) = spawned {
        log::warn!("input poll: thread not started: {err}");
    }
}

/// The bindings as they now stand — from `register_hotkeys`. Ends a
/// suspension: whatever is held right now counts only after its release.
pub fn install(next: Table<Action>) {
    *table() = Some(next);
    SUSPENDED.store(false, Ordering::SeqCst);
    nudge();
}

/// Nothing fires until the next [`install`] — while the settings record. Every
/// way out of recording (a new binding, Escape, a rejected one) registers the
/// hotkeys again, and that installs a fresh table.
pub fn suspend() {
    SUSPENDED.store(true, Ordering::SeqCst);
}

/// The settings start or stop recording a binding — listening to what
/// `filter` lets through.
pub fn capture(on: bool, filter: CaptureFilter) {
    if on {
        CAPTURE_SINCE.store(now_ms(), Ordering::SeqCst);
        *CAPTURE_FILTER
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = filter;
    }
    CAPTURING.store(on, Ordering::SeqCst);
    nudge();
}

/// The settings page shows the controllers: read them for the next
/// [`WATCH_MS`], and here is the list as it stands. Changes come as
/// `controllers-changed`.
pub fn watch_controllers() -> Controllers {
    WATCH_UNTIL.store(now_ms() + WATCH_MS, Ordering::SeqCst);
    nudge();
    controllers().clone()
}

fn controllers() -> std::sync::MutexGuard<'static, Controllers> {
    CONTROLLERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Whose names the controller's buttons carry — the pad pressed last, else the
/// first one connected, or Xbox's when none is.
pub fn family() -> Family {
    match FAMILY.load(Ordering::Relaxed) {
        1 => Family::PlayStation,
        2 => Family::Nintendo,
        3 => Family::GameCube,
        _ => Family::Xbox,
    }
}

/// [`family`], once a button was pressed on a pad this session — the names a
/// binding of the pad last used is said in.
pub fn family_pressed() -> Option<Family> {
    FAMILY_PRESSED.load(Ordering::Relaxed).then(family)
}

/// The console opened or closed (`console::CONSOLE_SHOWN`): wake the thread,
/// whose reading may change with it.
pub fn console_changed() {
    nudge();
}

/// End a recording the page did not end itself: tell it, and put the hotkeys
/// back — they were suspended for it. Nothing happens when none runs.
pub fn end_capture(app: &AppHandle) {
    if !CAPTURING.swap(false, Ordering::SeqCst) {
        return;
    }
    let _ = app.emit("input-captured", CapturedEvent { binding: None });
    if let Err(err) = crate::register_hotkeys(app) {
        log::warn!("{err}");
    }
}

/// A keyboard combination went down or up (from `RegisterHotKey`, by the id
/// of its shortcut).
pub fn key(app: &AppHandle, id: u32, pressed: bool) {
    if SUSPENDED.load(Ordering::SeqCst) {
        return;
    }
    let fired = table()
        .as_mut()
        .and_then(|table| table.key(id, pressed, now_ms()));
    // A hold has to be timed from here on.
    nudge();
    if let Some(action) = fired {
        // Only the press itself comes as `WM_HOTKEY`, which lets ClippiBoy
        // take the foreground; a release reported by the shortcut plugin
        // does not.
        let opener = if pressed {
            crate::console::Opener::Hotkey
        } else {
            crate::console::Opener::Polled
        };
        crate::run_hotkey(app, action, opener);
    }
}

fn run(app: &AppHandle, inbox: &Receiver<()>) {
    let mut capture: Option<Capture> = None;
    let mut pads = Pads::default();
    let mut elevation = Elevation::default();
    // The console's navigation since it opened, and what all pads held in
    // the last round.
    let mut nav: Option<(Navigator, u32)> = None;
    // The buttons of the console binding's combos, as of the last round.
    let mut console_chord = 0u32;
    loop {
        let now = now_ms();
        let capturing = CAPTURING.load(Ordering::SeqCst);
        let suspended = SUSPENDED.load(Ordering::SeqCst);
        let (mouse, controllers, keys, key_holds) = match table().as_ref() {
            Some(table) if !suspended => (
                table.wants_mouse(),
                table.wants_pads(),
                table.keys_held(),
                table.key_holds(),
            ),
            _ => (false, false, false, false),
        };
        if !capturing {
            capture = None;
        }
        let watching = now < WATCH_UNTIL.load(Ordering::SeqCst);
        let console = crate::console::CONSOLE_SHOWN.load(Ordering::SeqCst);
        // Bound controllers want reading only where they can be read; the
        // settings (recording, the list) want to hear why not, too.
        let console_pads = console
            && CONSOLE_NAV_READS_PADS
            && (controllers || pads.up || pads.seen)
            && pads.possible(now);
        let want_pads = (controllers && pads.possible(now)) || capturing || watching || console_pads;
        // A console standing without the focus watches where the foreground
        // goes, pads read or not.
        let passive = console && crate::console::is_passive();
        // A key bound to be held needs no reading until it is pressed — but
        // the look at the foreground game, every couple of seconds.
        let watch = (mouse || key_holds) && !capturing;
        if !(mouse || want_pads || keys || capturing) {
            if watch {
                elevation.check(app, now);
            }
            if passive {
                crate::console::watch_passive(app, now);
            }
            // Nothing to read: wait for a change instead of spinning — or
            // until SDL is due to be shut down.
            let mut wait = if watch {
                Duration::from_millis(ELEVATION_MS)
            } else {
                Duration::from_secs(60)
            };
            if let Some(left) = pads.idle(app, now) {
                wait = wait.min(Duration::from_millis(left));
            }
            if passive {
                wait = wait.min(Duration::from_millis(crate::console::PASSIVE_WATCH_MS));
            }
            nav = None;
            if let Err(RecvTimeoutError::Disconnected) = inbox.recv_timeout(wait) {
                return;
            }
            continue;
        }

        if capturing && now.saturating_sub(CAPTURE_SINCE.load(Ordering::SeqCst)) > CAPTURE_MS {
            log::info!("input poll: recording a binding timed out");
            end_capture(app);
            continue;
        }
        let read = if want_pads {
            pads.read(app, now, console)
        } else {
            let _ = pads.idle(app, now);
            PadRead::default()
        };
        let input = Input {
            mouse: if mouse || capturing {
                read_mouse()
            } else {
                [false; 3]
            },
            mods: if mouse || capturing {
                read_mods()
            } else {
                Default::default()
            },
            pads: read.masks,
            pad_types: read.types,
        };
        let mut console_fired = false;
        if capturing {
            let filter = *CAPTURE_FILTER
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(binding) = capture.get_or_insert_with(|| Capture::new(filter)).poll(&input) {
                let _ = app.emit(
                    "input-captured",
                    CapturedEvent {
                        binding: Some(binding),
                    },
                );
            }
        } else if !suspended {
            let fired = table()
                .as_mut()
                .map(|table| {
                    // The keys held, let go as the poll sees them rather than
                    // as the shortcut plugin reports it, 50–60 ms later.
                    let mut fired = Vec::new();
                    for vk in table.held_vks() {
                        if !platform::key_down(vk) {
                            let up = table.vk_up(vk, now);
                            fired.extend(up.into_iter().map(|a| (a, Source::Key)));
                        }
                    }
                    // While the console is open the pads steer it: of their
                    // bindings only the console's own still fires, so the
                    // same button closes it again.
                    fired.extend(table.poll_holding_back(now, &input, |action| {
                        console && *action != Action::Console
                    }));
                    console_chord = table.pad_combo_buttons(|action| *action == Action::Console);
                    fired
                })
                .unwrap_or_default();
            for (action, source) in fired {
                console_fired |= action == Action::Console;
                let opener = if source == Source::Pad {
                    crate::console::Opener::Pad
                } else {
                    crate::console::Opener::Polled
                };
                crate::run_hotkey(app, action, opener);
            }
        }
        if console && want_pads {
            // A Nintendo pad confirms on the right: its masks read like the
            // others' once A and B are swapped — and so do the console
            // binding's buttons on it.
            let mut masks = read.masks;
            let mut chord = console_chord;
            for (mask, nintendo) in masks.iter_mut().zip(read.nintendo) {
                if nintendo {
                    *mask = swap_confirm(*mask);
                    chord |= swap_confirm(console_chord);
                }
            }
            let (navigator, held) = nav.get_or_insert_with(|| {
                // What is held as the console opens — the binding that opened
                // it, a direction, the stick — was pressed before it: it
                // counts after a let-go.
                let held = masks.iter().fold(0, |all, mask| all | mask);
                (Navigator::new(held, read.stick), held)
            });
            let frame = PadFrame::after(*held, &masks, read.stick);
            *held = frame.held;
            let steps = navigator.step(&frame, now, chord);
            // The round the console's binding fired, its buttons are not
            // steps: the console is on its way out (or just came).
            if console_fired {
                navigator.drop_waiting();
            } else {
                console_nav(app, &steps);
            }
        } else {
            nav = None;
        }
        if passive {
            crate::console::watch_passive(app, now);
        }
        if watch {
            elevation.check(app, now);
        }

        if let Err(RecvTimeoutError::Disconnected) = inbox.recv_timeout(TICK) {
            return;
        }
    }
}

/// Hand the console page its steps (`consolenav`), as `console-nav` events
/// to that window alone.
fn console_nav(app: &AppHandle, steps: &[crate::consolenav::Nav]) {
    for step in steps {
        let _ = app.emit_to(crate::console::LABEL, "console-nav", step);
    }
}

/// Tell once per session that a game running elevated keeps the mouse
/// buttons and key releases from ClippiBoy — otherwise the binding just seems
/// broken. Controllers are read from the device (SDL), not from the window's
/// input, so they are left out of it.
fn warn_elevated(app: &AppHandle, game: &str) {
    if WARNED.swap(true, Ordering::SeqCst) {
        return;
    }
    log::info!("input poll: {game} runs elevated; mouse and hold hotkeys cannot see it");
    crate::overlay::show(
        app,
        crate::overlay::BannerKind::Info,
        "Mouse and hold hotkeys can't reach this game",
        Some("It runs with elevated rights.".into()),
    );
    crate::notify(
        app,
        "error",
        format!("Mouse and hold hotkeys can't reach {game}: it runs with elevated rights."),
    );
}

/// Watches the foreground game for running elevated. Only a game: Task
/// Manager or an elevated terminal in front says nothing about where the
/// hotkeys are wanted.
#[derive(Default)]
struct Elevation {
    next: u64,
    /// The last game looked at, so each is asked once.
    last_pid: u32,
    /// ClippiBoy itself runs elevated: then it sees everything anyway.
    own: Option<bool>,
}

impl Elevation {
    fn check(&mut self, app: &AppHandle, now: u64) {
        if now < self.next || WARNED.load(Ordering::SeqCst) {
            return;
        }
        self.next = now + ELEVATION_MS;
        let own = *self
            .own
            .get_or_insert_with(|| platform::elevated(std::process::id()) == Some(true));
        if own {
            return;
        }
        let Some(game) = crate::game::detect_detailed() else {
            return;
        };
        if game.pid == self.last_pid {
            return;
        }
        self.last_pid = game.pid;
        if platform::elevated(game.pid) == Some(true) {
            warn_elevated(app, &game.name);
        }
    }
}

/// What the controllers gave in one round.
#[derive(Clone, Copy, Default)]
struct PadRead {
    /// One mask per slot, 0 for an empty one.
    masks: [u32; MAX_PADS],
    /// Which slots hold a Nintendo pad — confirm and back sit the other way
    /// round there.
    nintendo: [bool; MAX_PADS],
    /// The type of the pad in each slot, for the profile it goes by.
    types: [PadType; MAX_PADS],
    /// The left stick pushed furthest, read only while the console is open.
    stick: (i16, i16),
}

/// One controller in use: SDL's id for it, and the slot it reads into.
struct Open {
    id: u32,
    pad: Gamepad,
    slot: usize,
    name: String,
    family: Family,
    pad_type: PadType,
    /// What it held in the last round — a button newly down makes it the pad
    /// in use.
    held: u32,
}

/// SDL and the controllers it has open. Lives on the poll thread.
#[derive(Default)]
struct Pads {
    /// SDL is started.
    up: bool,
    open: Vec<Open>,
    next_scan: u64,
    /// When the pads were last needed — SDL goes down [`PAD_IDLE_MS`] later.
    last_used: u64,
    /// SDL failed to start; not again before this.
    retry_at: u64,
    /// A controller was connected at some point this session.
    seen: bool,
    /// SDL's id of the pad a button was last pressed on.
    pressed_last: Option<u32>,
}

impl Pads {
    /// Can the controllers be read — or tried again — right now?
    fn possible(&self, now: u64) -> bool {
        self.up || (Sdl::get().is_ok() && now >= self.retry_at)
    }

    /// SDL, started if it is not yet. `None` when it cannot be — and the
    /// settings are told why.
    fn sdl(&mut self, app: &AppHandle, now: u64) -> Option<&'static Sdl> {
        let sdl = match Sdl::get() {
            Ok(sdl) => sdl,
            Err(err) => {
                if publish(app, |list| {
                    list.ready = true;
                    list.unavailable = Some(err.clone());
                    list.pads.clear();
                }) {
                    log::warn!("controllers: {err}");
                }
                return None;
            }
        };
        if self.up {
            return Some(sdl);
        }
        if now < self.retry_at {
            return None;
        }
        let started = Instant::now();
        if let Err(err) = sdl.init() {
            log::warn!("controllers: {err}");
            self.retry_at = now + PAD_RETRY_MS;
            publish(app, |list| {
                list.ready = true;
                list.unavailable = Some(err);
                list.pads.clear();
            });
            return None;
        }
        log::info!(
            "controllers: SDL started in {} ms",
            started.elapsed().as_millis()
        );
        self.up = true;
        self.next_scan = 0;
        Some(sdl)
    }

    fn read(&mut self, app: &AppHandle, now: u64, console: bool) -> PadRead {
        self.last_used = now;
        let mut read = PadRead::default();
        let Some(sdl) = self.sdl(app, now) else {
            return read;
        };
        sdl.update();
        if now >= self.next_scan {
            self.next_scan = now + SCAN_MS;
            self.scan(app, sdl);
        }
        let mut furthest = 0i32;
        let mut pressed = None;
        for open in &mut self.open {
            let held = sdl.buttons(&open.pad);
            if held & !open.held != 0 {
                pressed = Some(open.id);
            }
            open.held = held;
            read.masks[open.slot] = held;
            read.nintendo[open.slot] = open.family == Family::Nintendo;
            read.types[open.slot] = open.pad_type;
            if console {
                let (x, y) = sdl.left_stick(&open.pad);
                let reach = i32::from(x).abs().max(i32::from(y).abs());
                if reach > furthest {
                    furthest = reach;
                    read.stick = (x, y);
                }
            }
        }
        if pressed.is_some() && pressed != self.pressed_last {
            self.pressed_last = pressed;
            self.show_family(app);
            if let Some(open) = self.open.iter().find(|open| Some(open.id) == pressed) {
                remember_pad(app, None, Some(open.pad_type));
            }
        }
        read
    }

    /// Name the buttons after the pad in use, here and in the settings.
    fn show_family(&self, app: &AppHandle) {
        let pads: Vec<(u32, Family)> = self.open.iter().map(|o| (o.id, o.family)).collect();
        let family = crate::bindings::family_in_use(&pads, self.pressed_last);
        let in_use = self
            .pressed_last
            .and_then(|last| self.open.iter().find(|open| open.id == last))
            .or(self.open.first())
            .map(|open| open.pad_type.key());
        FAMILY.store(
            match family {
                Family::Xbox => 0,
                Family::PlayStation => 1,
                Family::Nintendo => 2,
                Family::GameCube => 3,
            },
            Ordering::Relaxed,
        );
        FAMILY_PRESSED.store(self.pressed_last.is_some(), Ordering::Relaxed);
        publish(app, |list| {
            list.family = family.key();
            list.in_use = in_use;
        });
    }

    /// Open the pads that came, close the ones that went, and tell the
    /// settings when the list changed.
    fn scan(&mut self, app: &AppHandle, sdl: &Sdl) {
        let ids = sdl.gamepad_ids();
        let before = self.open.len();
        let mut index = 0;
        while index < self.open.len() {
            let open = &self.open[index];
            if ids.contains(&open.id) && sdl.connected(&open.pad) {
                index += 1;
            } else {
                let gone = self.open.remove(index);
                log::info!("controllers: {} gone", gone.name);
                sdl.close(gone.pad);
            }
        }
        for id in ids {
            if self.open.iter().any(|open| open.id == id) {
                continue;
            }
            let Some(slot) = (0..MAX_PADS).find(|slot| self.open.iter().all(|o| o.slot != *slot))
            else {
                break;
            };
            let Some(pad) = sdl.open(id) else {
                continue;
            };
            let name = sdl.name(&pad);
            let family = sdl.family(&pad);
            let pad_type = sdl.pad_type(&pad);
            log::info!("controllers: {name} ({}, {})", family.key(), pad_type.key());
            self.open.push(Open {
                id,
                pad,
                slot,
                name,
                family,
                pad_type,
                held: 0,
            });
        }
        self.open.sort_by_key(|open| open.slot);
        self.seen |= !self.open.is_empty();
        if self.open.len() > before {
            let types: Vec<PadType> = self.open.iter().map(|open| open.pad_type).collect();
            remember_pad(app, Some(&types), None);
        }
        sdl.flush();
        self.show_family(app);
        let pads: Vec<Controller> = self
            .open
            .iter()
            .map(|open| Controller {
                name: open.name.clone(),
                family: open.family.key(),
                pad_type: open.pad_type.key(),
            })
            .collect();
        publish(app, |list| {
            list.ready = true;
            list.unavailable = None;
            list.pads = pads;
        });
    }

    /// Not needed this round. Shuts SDL down once it has not been needed for
    /// [`PAD_IDLE_MS`]; otherwise says how long until then.
    fn idle(&mut self, app: &AppHandle, now: u64) -> Option<u64> {
        if !self.up {
            return None;
        }
        let due = self.last_used + PAD_IDLE_MS;
        if now < due {
            return Some(due - now);
        }
        if let Ok(sdl) = Sdl::get() {
            for open in self.open.drain(..) {
                sdl.close(open.pad);
            }
            sdl.quit();
        }
        self.up = false;
        log::info!("controllers: SDL shut down, not needed");
        publish(app, |list| *list = Controllers::EMPTY);
        None
    }
}

/// Keep in the config what the settings and the names of bindings go by: the
/// pad types connected (`seen`, added to the ones ever seen) and the type of
/// the pad pressed last (`used`). Written only when that changes it — rarely,
/// a new type or another pad picked up — and off this thread, which reads the
/// pads on.
fn remember_pad(app: &AppHandle, seen: Option<&[PadType]>, used: Option<PadType>) {
    use tauri::Manager;
    let config = app.state::<crate::state::AppState>().config_snapshot();
    let Some(next) = crate::model::remember_pads(&config, seen.unwrap_or(&[]), used) else {
        return;
    };
    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.state::<crate::state::AppState>();
        // Built on the copy read above: what changed meanwhile is merged in
        // (`config::accept`), and a type seen meanwhile is kept.
        let written = state.replace_config_with(next, |next, current| {
            for one in &current.pad_families_seen {
                if !next.pad_families_seen.contains(one) {
                    next.pad_families_seen.push(one.clone());
                }
            }
        });
        crate::commands::tell_main(&app, &written);
    });
}

/// Change the controller list; tell the settings when that changed it.
fn publish(app: &AppHandle, change: impl FnOnce(&mut Controllers)) -> bool {
    let next = {
        let mut list = controllers();
        let before = list.clone();
        change(&mut list);
        if *list == before {
            return false;
        }
        list.clone()
    };
    let _ = app.emit("controllers-changed", next);
    true
}

fn read_mouse() -> [bool; 3] {
    platform::mouse()
}

fn read_mods() -> crate::bindings::Mods {
    platform::mods()
}

#[cfg(windows)]
mod platform {
    use crate::bindings::Mods;
    use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, HANDLE, MAX_PATH};
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VIRTUAL_KEY, VK_CONTROL, VK_MBUTTON, VK_MENU, VK_SHIFT, VK_XBUTTON1,
        VK_XBUTTON2,
    };

    fn down(key: VIRTUAL_KEY) -> bool {
        // The high bit: held right now. Only reads the state — the press
        // still goes to the game.
        unsafe { GetAsyncKeyState(i32::from(key.0)) < 0 }
    }

    pub fn mouse() -> [bool; 3] {
        [down(VK_MBUTTON), down(VK_XBUTTON1), down(VK_XBUTTON2)]
    }

    pub fn mods() -> Mods {
        let mut mods = Mods::NONE;
        if down(VK_CONTROL) {
            mods = mods.with(Mods::CTRL);
        }
        if down(VK_MENU) {
            mods = mods.with(Mods::ALT);
        }
        if down(VK_SHIFT) {
            mods = mods.with(Mods::SHIFT);
        }
        mods
    }

    /// Is the key with this virtual-key code held right now?
    pub fn key_down(vk: u16) -> bool {
        down(VIRTUAL_KEY(vk))
    }

    /// Does `pid` run elevated? `None` when that cannot be told.
    ///
    /// A process running elevated lets a normal one open it for
    /// `PROCESS_QUERY_LIMITED_INFORMATION`, but not its token. So: the
    /// process opens and gives its name (the handle really has the right —
    /// an anti-cheat may strip rights off handles to its game), and then
    /// either the token says it, or the token is refused — elevated.
    pub fn elevated(pid: u32) -> Option<bool> {
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; MAX_PATH as usize];
            let mut len = buf.len() as u32;
            let named = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                windows::core::PWSTR(buf.as_mut_ptr()),
                &mut len,
            );
            if named.is_err() {
                let _ = CloseHandle(process);
                return None;
            }
            let mut token = HANDLE::default();
            let opened = OpenProcessToken(process, TOKEN_QUERY, &mut token);
            let _ = CloseHandle(process);
            if let Err(err) = opened {
                return (err.code() == ERROR_ACCESS_DENIED.to_hresult()).then_some(true);
            }
            let mut elevation = TOKEN_ELEVATION::default();
            let mut returned = 0u32;
            let read = GetTokenInformation(
                token,
                TokenElevation,
                Some(&mut elevation as *mut TOKEN_ELEVATION as *mut core::ffi::c_void),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned,
            );
            let _ = CloseHandle(token);
            read.ok()?;
            Some(elevation.TokenIsElevated != 0)
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use crate::bindings::Mods;

    pub fn mouse() -> [bool; 3] {
        [false; 3]
    }
    pub fn mods() -> Mods {
        Mods::NONE
    }
    pub fn key_down(_vk: u16) -> bool {
        false
    }
    pub fn elevated(_pid: u32) -> Option<bool> {
        None
    }
}
