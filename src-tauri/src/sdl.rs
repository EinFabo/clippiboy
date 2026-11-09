//! Controllers through SDL3 — its gamepad part only, no window, no sound.
//!
//! XInput alone sees Xbox pads, and not even their Guide button. SDL reads
//! every pad people actually own: Xbox pads through XInput with the hidden
//! `XInputGetStateEx` (Guide included), PlayStation, Switch Pro, Joy-Con and
//! 8BitDo pads through its own HID drivers — including the handshake a Switch
//! Pro Controller wants over USB before it sends anything. It reads in the
//! background: with no SDL window there is no focus to lose, and
//! `SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS` says so once more.
//!
//! `SDL3.dll` (the official release, pinned in `scripts/fetch-sdl.mjs`) sits
//! beside the exe and is **loaded at run time**, not linked: without it the app
//! still starts and keyboard and mouse hotkeys work — only controllers are
//! missing — and `cargo test` needs neither the DLL nor a C build. zlib
//! licence.
//!
//! SDL writes to some pads by default: rumble and lights on PlayStation pads
//! after switching them to "enhanced reports" (which, once on, breaks
//! DirectInput for other programs until the pad is switched off), player
//! LEDs. The hints in [`Sdl::init`] keep all of that off; what remains is what
//! SDL needs to read a pad at all.
//!
//! Everything here is called from the poll thread (`inputpoll`) only.

/// What kind of pad — for the names on its buttons.
pub use crate::bindings::Family;

/// SDL's `SDL_GamepadType` as a [`Family`].
pub fn family_of_type(kind: i32) -> Family {
    match kind {
        // PS3, PS4, PS5
        4..=6 => Family::PlayStation,
        // Switch Pro, Joy-Con left/right/pair, GameCube
        7..=11 => Family::Nintendo,
        // Xbox 360, Xbox One, standard, Steam, unknown: Xbox's names.
        _ => Family::Xbox,
    }
}

#[cfg(windows)]
pub use imp::{Gamepad, Sdl};

#[cfg(windows)]
mod imp {
    use std::ffi::{c_char, c_int, c_void, CStr, CString};
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::ptr::NonNull;
    use std::sync::OnceLock;

    use windows::core::{PCSTR, PCWSTR};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::LibraryLoader::{
        GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        LOAD_LIBRARY_SEARCH_SYSTEM32,
    };

    use crate::bindings::{pad, Family};

    const DLL: &str = "SDL3.dll";
    /// `SDL_INIT_GAMEPAD` — brings the joystick and event parts along.
    const INIT_GAMEPAD: u32 = 0x0000_2000;
    /// `SDL_HINT_OVERRIDE`: above whatever an environment variable says.
    const HINT_OVERRIDE: c_int = 2;
    /// SDL 3.2.0 — every hint below exists from there on.
    const MIN_VERSION: c_int = 3_002_000;
    const AXIS_LEFTX: c_int = 0;
    const AXIS_LEFTY: c_int = 1;
    const AXIS_LEFT_TRIGGER: c_int = 4;
    const AXIS_RIGHT_TRIGGER: c_int = 5;

    /// Set before `SDL_Init`, each over any environment variable of the same
    /// name.
    const HINTS: [(&str, &str); 14] = [
        // No window, no focus: read the pads whatever is in front.
        ("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1"),
        // Never switch a PlayStation pad into enhanced reports (rumble, LEDs,
        // gyro): it stays that way until power-off and breaks DirectInput for
        // every other program. Switch pads stay in their simple report over
        // Bluetooth too; over USB SDL needs the full one.
        ("SDL_JOYSTICK_ENHANCED_REPORTS", "0"),
        // Player LEDs stay as they are. The Home LEDs SDL leaves alone unless
        // asked, so they are not named here.
        ("SDL_JOYSTICK_HIDAPI_PS5_PLAYER_LED", "0"),
        ("SDL_JOYSTICK_HIDAPI_SWITCH_PLAYER_LED", "0"),
        ("SDL_JOYSTICK_HIDAPI_XBOX_360_PLAYER_LED", "0"),
        ("SDL_JOYSTICK_HIDAPI_WII_PLAYER_LED", "0"),
        // No pads hidden or let through by a list from outside (Steam sets
        // these for the games it starts).
        ("SDL_GAMECONTROLLER_IGNORE_DEVICES", ""),
        ("SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT", ""),
        // Raw input and Windows.Gaming.Input only work for a focused window;
        // both are off by default already — and stay off.
        ("SDL_JOYSTICK_RAWINPUT", "0"),
        ("SDL_JOYSTICK_WGI", "0"),
        // GameInput delivers background input only with a system-installed
        // redistributable; XInput and HID already cover the same pads.
        ("SDL_JOYSTICK_GAMEINPUT", "0"),
        // SDL would raise the system timer to 1 ms for the whole machine —
        // a cost to every game's power use for nothing we need.
        ("SDL_TIMER_RESOLUTION", "0"),
        // Ctrl+C and the like stay ClippiBoy's.
        ("SDL_NO_SIGNAL_HANDLERS", "1"),
        // SDL's own thread watches for pads coming and going, so nothing has
        // to pump window messages here.
        ("SDL_JOYSTICK_THREAD", "1"),
    ];

    type GetVersion = unsafe extern "C" fn() -> c_int;
    type SetHintWithPriority = unsafe extern "C" fn(*const c_char, *const c_char, c_int) -> bool;
    type Init = unsafe extern "C" fn(u32) -> bool;
    type QuitSubSystem = unsafe extern "C" fn(u32);
    type GetError = unsafe extern "C" fn() -> *const c_char;
    type SetEventsEnabled = unsafe extern "C" fn(bool);
    type FlushEvents = unsafe extern "C" fn(u32, u32);
    type Update = unsafe extern "C" fn();
    type GetGamepads = unsafe extern "C" fn(*mut c_int) -> *mut u32;
    type Free = unsafe extern "C" fn(*mut c_void);
    type OpenGamepad = unsafe extern "C" fn(u32) -> *mut c_void;
    type OnGamepad = unsafe extern "C" fn(*mut c_void);
    type Connected = unsafe extern "C" fn(*mut c_void) -> bool;
    type GetButton = unsafe extern "C" fn(*mut c_void, c_int) -> bool;
    type GetAxis = unsafe extern "C" fn(*mut c_void, c_int) -> i16;
    type GetName = unsafe extern "C" fn(*mut c_void) -> *const c_char;
    type GetType = unsafe extern "C" fn(*mut c_void) -> c_int;

    /// The SDL functions used, out of the loaded DLL. Never unloaded.
    pub struct Sdl {
        set_hint: SetHintWithPriority,
        init: Init,
        quit_sub_system: QuitSubSystem,
        get_error: GetError,
        set_joystick_events: SetEventsEnabled,
        set_gamepad_events: SetEventsEnabled,
        flush_events: FlushEvents,
        update: Update,
        get_gamepads: GetGamepads,
        free: Free,
        open: OpenGamepad,
        close: OnGamepad,
        connected: Connected,
        button: GetButton,
        axis: GetAxis,
        name: GetName,
        kind: GetType,
    }

    /// An open pad. Lives on the poll thread only — the raw pointer keeps it
    /// from going anywhere else.
    pub struct Gamepad(NonNull<c_void>);

    static LOADED: OnceLock<Result<Sdl, String>> = OnceLock::new();

    /// Where `SDL3.dll` may be: beside the exe (the installer puts it there),
    /// and in a dev build also where `npm run sdl` leaves it.
    fn candidates() -> Vec<PathBuf> {
        let mut found = Vec::new();
        if let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
        {
            found.push(dir.join(DLL));
        }
        if cfg!(debug_assertions) {
            found.push(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("..")
                    .join("dist-sdl")
                    .join(DLL),
            );
        }
        found
    }

    impl Sdl {
        /// The DLL, loaded once. An error says why there are no controllers.
        pub fn get() -> Result<&'static Sdl, String> {
            LOADED.get_or_init(load).as_ref().map_err(Clone::clone)
        }

        /// Hints, then `SDL_Init` for the gamepad part. Events are switched
        /// off: the state is read directly, and nothing piles up in a queue
        /// no one empties.
        pub fn init(&self) -> Result<(), String> {
            for (name, value) in HINTS {
                let (name, value) = (cstring(name), cstring(value));
                unsafe { (self.set_hint)(name.as_ptr(), value.as_ptr(), HINT_OVERRIDE) };
            }
            if !unsafe { (self.init)(INIT_GAMEPAD) } {
                return Err(format!("SDL_Init failed: {}", self.error()));
            }
            unsafe {
                (self.set_joystick_events)(false);
                (self.set_gamepad_events)(false);
            }
            self.flush();
            Ok(())
        }

        /// The counterpart to [`Sdl::init`]. Close every [`Gamepad`] first.
        pub fn quit(&self) {
            unsafe { (self.quit_sub_system)(INIT_GAMEPAD) };
        }

        fn error(&self) -> String {
            let text = unsafe { (self.get_error)() };
            if text.is_null() {
                return String::new();
            }
            unsafe { CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned()
        }

        /// Drop whatever events arrived anyway (sensor, battery …).
        pub fn flush(&self) {
            unsafe { (self.flush_events)(0, u32::MAX) };
        }

        /// Read what the pads sent since the last call, and notice pads that
        /// came or went.
        pub fn update(&self) {
            unsafe { (self.update)() };
        }

        /// The ids of the pads connected now.
        pub fn gamepad_ids(&self) -> Vec<u32> {
            let mut count: c_int = 0;
            let list = unsafe { (self.get_gamepads)(&mut count) };
            if list.is_null() {
                return Vec::new();
            }
            let ids = unsafe { std::slice::from_raw_parts(list, count.max(0) as usize) }.to_vec();
            unsafe { (self.free)(list.cast()) };
            ids
        }

        pub fn open(&self, id: u32) -> Option<Gamepad> {
            NonNull::new(unsafe { (self.open)(id) }).map(Gamepad)
        }

        pub fn close(&self, pad: Gamepad) {
            unsafe { (self.close)(pad.0.as_ptr()) };
        }

        pub fn connected(&self, pad: &Gamepad) -> bool {
            unsafe { (self.connected)(pad.0.as_ptr()) }
        }

        /// The buttons held, as [`pad`] bits.
        pub fn buttons(&self, gamepad: &Gamepad) -> u32 {
            let ptr = gamepad.0.as_ptr();
            let mut mask = 0;
            for button in 0..pad::BUTTON_COUNT {
                if unsafe { (self.button)(ptr, button as c_int) } {
                    mask |= 1 << button;
                }
            }
            if pad::trigger(unsafe { (self.axis)(ptr, AXIS_LEFT_TRIGGER) }) {
                mask |= pad::LT;
            }
            if pad::trigger(unsafe { (self.axis)(ptr, AXIS_RIGHT_TRIGGER) }) {
                mask |= pad::RT;
            }
            mask
        }

        /// The left stick, -32768..32767 each way, down positive.
        pub fn left_stick(&self, pad: &Gamepad) -> (i16, i16) {
            let ptr = pad.0.as_ptr();
            unsafe { ((self.axis)(ptr, AXIS_LEFTX), (self.axis)(ptr, AXIS_LEFTY)) }
        }

        pub fn name(&self, pad: &Gamepad) -> String {
            let text = unsafe { (self.name)(pad.0.as_ptr()) };
            if text.is_null() {
                return "Controller".into();
            }
            unsafe { CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned()
        }

        pub fn family(&self, pad: &Gamepad) -> Family {
            super::family_of_type(unsafe { (self.kind)(pad.0.as_ptr()) })
        }
    }

    fn cstring(text: &str) -> CString {
        CString::new(text).unwrap_or_default()
    }

    fn load() -> Result<Sdl, String> {
        let mut tried = Vec::new();
        for path in candidates() {
            if !path.is_file() {
                tried.push(path.display().to_string());
                continue;
            }
            return load_from(&path).map_err(|err| format!("{}: {err}", path.display()));
        }
        Err(format!("{DLL} not found ({})", tried.join(", ")))
    }

    fn load_from(path: &Path) -> Result<Sdl, String> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // By full path, and its own imports only from its folder and
        // System32 — never from the current directory or the PATH.
        let module = unsafe {
            LoadLibraryExW(
                PCWSTR(wide.as_ptr()),
                HANDLE::default(),
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        }
        .map_err(|err| format!("could not load: {err}"))?;

        macro_rules! symbol {
            ($name:literal) => {{
                let proc = unsafe { GetProcAddress(module, PCSTR(concat!($name, "\0").as_ptr())) }
                    .ok_or_else(|| format!("{} missing", $name))?;
                // SAFETY: the type is the function's C signature in SDL 3.2+.
                unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, _>(proc) }
            }};
        }

        let version: GetVersion = symbol!("SDL_GetVersion");
        let found = unsafe { version() };
        if found < MIN_VERSION {
            return Err(format!("SDL {found} is older than 3.2"));
        }
        Ok(Sdl {
            set_hint: symbol!("SDL_SetHintWithPriority"),
            init: symbol!("SDL_Init"),
            quit_sub_system: symbol!("SDL_QuitSubSystem"),
            get_error: symbol!("SDL_GetError"),
            set_joystick_events: symbol!("SDL_SetJoystickEventsEnabled"),
            set_gamepad_events: symbol!("SDL_SetGamepadEventsEnabled"),
            flush_events: symbol!("SDL_FlushEvents"),
            update: symbol!("SDL_UpdateGamepads"),
            get_gamepads: symbol!("SDL_GetGamepads"),
            free: symbol!("SDL_free"),
            open: symbol!("SDL_OpenGamepad"),
            close: symbol!("SDL_CloseGamepad"),
            connected: symbol!("SDL_GamepadConnected"),
            button: symbol!("SDL_GetGamepadButton"),
            axis: symbol!("SDL_GetGamepadAxis"),
            name: symbol!("SDL_GetGamepadName"),
            kind: symbol!("SDL_GetGamepadType"),
        })
    }
}

/// Elsewhere there is no SDL: no controllers, everything else as usual.
#[cfg(not(windows))]
pub use stub::{Gamepad, Sdl};

#[cfg(not(windows))]
mod stub {
    use crate::bindings::Family;

    pub struct Sdl;
    pub struct Gamepad;

    impl Sdl {
        pub fn get() -> Result<&'static Sdl, String> {
            Err("controllers are read on Windows only".into())
        }
        pub fn init(&self) -> Result<(), String> {
            Ok(())
        }
        pub fn quit(&self) {}
        pub fn flush(&self) {}
        pub fn update(&self) {}
        pub fn gamepad_ids(&self) -> Vec<u32> {
            Vec::new()
        }
        pub fn open(&self, _id: u32) -> Option<Gamepad> {
            None
        }
        pub fn close(&self, _pad: Gamepad) {}
        pub fn connected(&self, _pad: &Gamepad) -> bool {
            false
        }
        pub fn buttons(&self, _pad: &Gamepad) -> u32 {
            0
        }
        pub fn left_stick(&self, _pad: &Gamepad) -> (i16, i16) {
            (0, 0)
        }
        pub fn name(&self, _pad: &Gamepad) -> String {
            String::new()
        }
        pub fn family(&self, _pad: &Gamepad) -> Family {
            Family::Xbox
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdl_gamepad_types_map_to_the_names_printed_on_the_pad() {
        assert_eq!(family_of_type(0), Family::Xbox); // unknown
        assert_eq!(family_of_type(3), Family::Xbox); // Xbox One
        assert_eq!(family_of_type(6), Family::PlayStation); // PS5
        assert_eq!(family_of_type(7), Family::Nintendo); // Switch Pro
        assert_eq!(family_of_type(10), Family::Nintendo); // Joy-Con pair
        assert_eq!(family_of_type(12), Family::Xbox); // Steam
    }

    /// Against the real `SDL3.dll` (`npm run sdl`): loads, starts, lists the
    /// pads plugged in and what they hold for two seconds.
    /// `cargo test --lib sdl:: -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn the_real_dll_loads_and_lists_the_pads() {
        let sdl = Sdl::get().expect("SDL3.dll");
        sdl.init().expect("SDL_Init");
        let started = std::time::Instant::now();
        let mut open = Vec::new();
        while started.elapsed() < std::time::Duration::from_secs(2) {
            sdl.update();
            for id in sdl.gamepad_ids() {
                if !open.iter().any(|(known, _)| *known == id) {
                    if let Some(pad) = sdl.open(id) {
                        println!("pad {id}: {} ({:?})", sdl.name(&pad), sdl.family(&pad));
                        open.push((id, pad));
                    }
                }
            }
            for (id, pad) in &open {
                let held = sdl.buttons(pad);
                if held != 0 {
                    println!("pad {id}: {held:#x}");
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        println!("{} pad(s)", open.len());
        for (_, pad) in open {
            sdl.close(pad);
        }
        sdl.flush();
        sdl.quit();
    }
}
