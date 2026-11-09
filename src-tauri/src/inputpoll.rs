//! Reads the hotkeys that are not keyboard combinations: the mouse's middle
//! and side buttons and Xbox controller combos — and times the holds of all
//! of them, keyboard included. What a press means is in `bindings`.
//!
//! No hook: a thread of its own asks `GetAsyncKeyState` for the three mouse
//! buttons about every [`TICK`], the way OBS reads its mouse hotkeys
//! (`libobs/obs-windows.c`), and `XInputGetState` (`xinput1_4`) for the
//! controllers in the same round — empty controller slots only every
//! [`PROBE_MS`]. Nothing is injected, nothing swallowed, no `SendInput`: the
//! game sees every press as before, and no anti-cheat has anything to object
//! to. A hook would add a timeout Windows enforces silently and lag on every
//! mouse move; Raw Input would fight tao's registration.
//!
//! The thread sleeps while nothing needs it: no mouse or controller binding,
//! no keyboard hold being timed, no binding being recorded in the settings.
//!
//! The limit is the same as for every hook: a game running as administrator
//! keeps its input from a program that is not. Then [`warn_elevated`] says so
//! once.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use tauri::{AppHandle, Emitter};

use crate::bindings::{Capture, Captured, Input, Table};

/// What a hotkey does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    SaveClip,
    Screenshot,
    Record,
    ToggleBuffer,
    Console,
}

/// How often the buttons are read. A physical click lasts longer than this.
const TICK: Duration = Duration::from_millis(10);
/// How often an empty controller slot is asked whether a pad came —
/// Microsoft advises against asking every frame.
const PROBE_MS: u64 = 2_500;
/// How often the foreground process is checked for running as administrator.
const ELEVATION_MS: u64 = 2_000;

static TABLE: Mutex<Option<Table<Action>>> = Mutex::new(None);
/// The settings are recording a new binding: nothing fires.
static SUSPENDED: AtomicBool = AtomicBool::new(false);
/// The settings want the next mouse button or controller combo.
static CAPTURING: AtomicBool = AtomicBool::new(false);
/// The administrator notice came already this session.
static WARNED: AtomicBool = AtomicBool::new(false);
static INBOX: OnceLock<Sender<()>> = OnceLock::new();
static EPOCH: OnceLock<Instant> = OnceLock::new();

/// What the settings get on `input-captured`.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CapturedEvent {
    binding: Option<String>,
    error: Option<String>,
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

/// The settings start or stop recording a binding.
pub fn capture(on: bool) {
    CAPTURING.store(on, Ordering::SeqCst);
    nudge();
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
        crate::run_hotkey(app, action);
    }
}

fn run(app: &AppHandle, inbox: &Receiver<()>) {
    let mut capture: Option<Capture> = None;
    let mut pads = Pads::default();
    let mut elevation = Elevation::default();
    loop {
        let capturing = CAPTURING.load(Ordering::SeqCst);
        let suspended = SUSPENDED.load(Ordering::SeqCst);
        let (mouse, controllers, keys) = match table().as_ref() {
            Some(table) if !suspended => {
                (table.wants_mouse(), table.wants_pads(), table.keys_held())
            }
            _ => (false, false, false),
        };
        if !capturing {
            capture = None;
        }
        if !(mouse || controllers || keys || capturing) {
            // Nothing to read: wait for a change instead of spinning.
            if let Err(RecvTimeoutError::Disconnected) = inbox.recv_timeout(Duration::from_secs(60))
            {
                return;
            }
            continue;
        }

        let now = now_ms();
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
            pads: if controllers || capturing {
                pads.read(now)
            } else {
                [0; 4]
            },
        };
        if capturing {
            if let Some(got) = capture.get_or_insert_with(Capture::default).poll(&input) {
                let event = match got {
                    Captured::Binding(binding) => CapturedEvent {
                        binding: Some(binding),
                        error: None,
                    },
                    Captured::TooFew => CapturedEvent {
                        binding: None,
                        error: Some("A controller combination needs at least two buttons.".into()),
                    },
                };
                let _ = app.emit("input-captured", event);
            }
        } else if !suspended {
            let fired = table()
                .as_mut()
                .map(|table| table.poll(now, &input))
                .unwrap_or_default();
            for action in fired {
                crate::run_hotkey(app, action);
            }
        }
        if mouse && !capturing {
            elevation.check(app, now);
        }

        if let Err(RecvTimeoutError::Disconnected) = inbox.recv_timeout(TICK) {
            return;
        }
    }
}

/// Tell once per session that a game running as administrator keeps its
/// mouse buttons from ClippiBoy — otherwise the binding just seems broken.
/// XInput reads the controller itself, not the window's input, so
/// controllers are left out of it.
fn warn_elevated(app: &AppHandle) {
    if WARNED.swap(true, Ordering::SeqCst) {
        return;
    }
    log::info!("input poll: foreground process runs as administrator; mouse hotkeys cannot see it");
    crate::overlay::show(
        app,
        crate::overlay::BannerKind::Info,
        "Mouse hotkeys can't reach this game",
        Some("It runs as administrator. Use a keyboard hotkey here.".into()),
    );
    crate::notify(
        app,
        "error",
        "Mouse hotkeys can't reach games running as administrator.",
    );
}

/// Watches the foreground process for running as administrator.
#[derive(Default)]
struct Elevation {
    next: u64,
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
        let pid = platform::foreground_pid();
        if pid == 0 || pid == self.last_pid {
            return;
        }
        self.last_pid = pid;
        if platform::elevated(pid) == Some(true) {
            warn_elevated(app);
        }
    }
}

/// The controller slots and which of them hold a pad.
#[derive(Default)]
struct Pads {
    connected: [bool; 4],
    next_probe: u64,
}

impl Pads {
    fn read(&mut self, now: u64) -> [u32; 4] {
        let probe = now >= self.next_probe;
        if probe {
            self.next_probe = now + PROBE_MS;
        }
        let mut masks = [0; 4];
        for slot in 0..4 {
            if !self.connected[slot] && !probe {
                continue;
            }
            match platform::pad(slot as u32) {
                Some(mask) => {
                    self.connected[slot] = true;
                    masks[slot] = mask;
                }
                None => self.connected[slot] = false,
            }
        }
        masks
    }
}

fn read_mouse() -> [bool; 3] {
    platform::mouse()
}

fn read_mods() -> crate::bindings::Mods {
    platform::mods()
}

#[cfg(windows)]
mod platform {
    use crate::bindings::{pad, Mods};
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
    use windows::Win32::UI::Input::XboxController::{XInputGetState, XINPUT_STATE};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

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

    /// One controller's buttons, or `None` for an empty slot.
    pub fn pad(slot: u32) -> Option<u32> {
        let mut state = XINPUT_STATE::default();
        let result = unsafe { XInputGetState(slot, &mut state) };
        (result == 0).then(|| {
            let gamepad = state.Gamepad;
            pad::mask(
                gamepad.wButtons.0,
                gamepad.bLeftTrigger,
                gamepad.bRightTrigger,
            )
        })
    }

    pub fn foreground_pid() -> u32 {
        let mut pid = 0u32;
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return 0;
            }
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        pid
    }

    /// Does `pid` run as administrator? `None` when that cannot be told.
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
    pub fn pad(_slot: u32) -> Option<u32> {
        None
    }
    pub fn foreground_pid() -> u32 {
        0
    }
    pub fn elevated(_pid: u32) -> Option<bool> {
        None
    }
}
