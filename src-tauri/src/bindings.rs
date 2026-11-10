//! Hotkeys beyond the keyboard: the mouse's middle and side buttons,
//! controller buttons and combos, and tapping versus holding — the pure part.
//!
//! A binding is still one string in the config, as before. Each action has
//! two for keys and mouse buttons (`…_hotkey`, `…_hotkey_alt`), and one per
//! controller profile (`pad_profiles`: one for every pad, and optionally one
//! per pad type that stands in for it on those pads — [`Profile`]):
//! - `Ctrl+Shift+S` — a keyboard combination, as `tauri_plugin_global_shortcut`
//!   spells it;
//! - `Mouse3` (middle), `Mouse4`, `Mouse5`, optionally behind `Ctrl`, `Alt`,
//!   `Shift` — `Ctrl+Mouse5`;
//! - `Pad:Guide`, `Pad:Back+South`, `Pad:LB+RB+South` — one controller
//!   button, or several held together on one controller, by position (see
//!   [`pad`]); any pad SDL reads, Guide included. Bindings written before,
//!   in Xbox names (`Pad:View+A`), read the same;
//! - any of these behind `Hold:` — the action fires once the binding has been
//!   held for a moment, so the same button can do one thing tapped and another
//!   held (`Mouse5` saves a clip, `Hold:Mouse5` takes a screenshot).
//!
//! The keys get read in `inputpoll` (keyboard through `RegisterHotKey`, mouse
//! and controller by polling); what they mean — which slot a press goes to,
//! when a tap or a hold fires — is decided here, on timestamps, so it can be
//! tested.

use std::fmt;

/// Held this long, a key, mouse button or controller binding counts as held
/// rather than tapped. Short enough for Guide too — held for seconds, that
/// one switches an Xbox pad off.
pub const HOLD_MS: u64 = 400;
/// A tap fires on the press — except on a controller binding that is part of
/// a bigger bound combo (`South` beside `Back+South`): that one waits this
/// long for the rest of the combo, as two buttons pressed "together" land a
/// few milliseconds apart.
pub const CHORD_MS: u64 = 50;
/// The same action fired from a controller again within this is the same
/// press: a remapper (DS4Windows, DualSenseX, reWASD) repeats every press of a
/// real pad on a virtual Xbox one, and with a type profile the two pads go by
/// different profiles — both would fire, and a toggle would undo itself.
pub const PAD_ECHO_MS: u64 = CHORD_MS;
/// Controllers read at once; more are left out.
pub const MAX_PADS: usize = 8;

/// Tapped or held.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Tap,
    Hold,
}

/// The mouse buttons that can be bound. The left and right ones cannot: they
/// are what one plays with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    /// The wheel button, `VK_MBUTTON`.
    Middle,
    /// The rear side button ("back"), `VK_XBUTTON1`.
    X1,
    /// The front side button ("forward"), `VK_XBUTTON2`.
    X2,
}

impl MouseButton {
    pub const ALL: [MouseButton; 3] = [MouseButton::Middle, MouseButton::X1, MouseButton::X2];

    pub fn index(self) -> usize {
        match self {
            MouseButton::Middle => 0,
            MouseButton::X1 => 1,
            MouseButton::X2 => 2,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            MouseButton::Middle => "Mouse3",
            MouseButton::X1 => "Mouse4",
            MouseButton::X2 => "Mouse5",
        }
    }
}

/// Ctrl, Alt and Shift held with a mouse button.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods(u8);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const CTRL: Mods = Mods(1);
    pub const ALT: Mods = Mods(2);
    pub const SHIFT: Mods = Mods(4);

    /// In the order the keyboard recorder writes them.
    const NAMES: [(Mods, &'static str); 3] = [
        (Mods::CTRL, "Ctrl"),
        (Mods::ALT, "Alt"),
        (Mods::SHIFT, "Shift"),
    ];

    pub fn with(self, other: Mods) -> Mods {
        Mods(self.0 | other.0)
    }

    /// Are all of `other` among these?
    pub fn contains(self, other: Mods) -> bool {
        self.0 & other.0 == other.0
    }

    fn count(self) -> u32 {
        self.0.count_ones()
    }
}

/// Controller buttons as bits: SDL's standard gamepad buttons by position
/// (`SDL_GamepadButton`: bit n is button n), plus the two triggers above them
/// (SDL reports those as an axis).
///
/// Bindings are stored by position, not by what is printed on the pad: the
/// bottom face button is `South` whether it reads A (Xbox), Cross
/// (PlayStation) or B (Nintendo). [`Family::label`] gives the printed name.
pub mod pad {
    pub const SOUTH: u32 = 1 << 0;
    pub const EAST: u32 = 1 << 1;
    pub const WEST: u32 = 1 << 2;
    pub const NORTH: u32 = 1 << 3;
    pub const BACK: u32 = 1 << 4;
    /// Xbox, PS, Home.
    pub const GUIDE: u32 = 1 << 5;
    pub const START: u32 = 1 << 6;
    pub const LS: u32 = 1 << 7;
    pub const RS: u32 = 1 << 8;
    pub const LB: u32 = 1 << 9;
    pub const RB: u32 = 1 << 10;
    pub const UP: u32 = 1 << 11;
    pub const DOWN: u32 = 1 << 12;
    pub const LEFT: u32 = 1 << 13;
    pub const RIGHT: u32 = 1 << 14;
    /// Xbox Series Share, DualSense mic, Switch Capture.
    pub const MISC: u32 = 1 << 15;
    /// The paddles, named as on the Xbox Elite pad: upper right, upper left,
    /// lower right, lower left (SDL's right/left paddle 1/2).
    pub const P1: u32 = 1 << 16;
    pub const P3: u32 = 1 << 17;
    pub const P2: u32 = 1 << 18;
    pub const P4: u32 = 1 << 19;
    /// PlayStation touchpad click.
    pub const TOUCHPAD: u32 = 1 << 20;
    pub const LT: u32 = 1 << 26;
    pub const RT: u32 = 1 << 27;

    /// SDL's buttons that are read: `SOUTH` up to `TOUCHPAD`.
    pub const BUTTON_COUNT: u32 = 21;

    /// A trigger pulled past this counts as pressed — XInput's
    /// `XINPUT_GAMEPAD_TRIGGER_THRESHOLD` (30 of 255) on SDL's 0–32767.
    pub const TRIGGER_THRESHOLD: i16 = 3855;

    pub fn trigger(axis: i16) -> bool {
        axis > TRIGGER_THRESHOLD
    }

    /// The buttons a game plays with. Bound alone, the game gets the same
    /// press — the settings say so once.
    pub const GAME_BUTTONS: u32 = SOUTH
        | EAST
        | WEST
        | NORTH
        | LB
        | RB
        | LT
        | RT
        | LS
        | RS
        | UP
        | DOWN
        | LEFT
        | RIGHT;

    /// Names in the order they are written: shoulders and the small buttons
    /// first, the face buttons last, as one reads a combo.
    pub const NAMES: [(u32, &str); 23] = [
        (LB, "LB"),
        (RB, "RB"),
        (LT, "LT"),
        (RT, "RT"),
        (BACK, "Back"),
        (START, "Start"),
        (GUIDE, "Guide"),
        (MISC, "Misc"),
        (TOUCHPAD, "Touchpad"),
        (P1, "P1"),
        (P2, "P2"),
        (P3, "P3"),
        (P4, "P4"),
        (LS, "LS"),
        (RS, "RS"),
        (UP, "Up"),
        (DOWN, "Down"),
        (LEFT, "Left"),
        (RIGHT, "Right"),
        (SOUTH, "South"),
        (EAST, "East"),
        (WEST, "West"),
        (NORTH, "North"),
    ];

    /// Xbox's names, which bindings were written in before SDL (`Pad:View+A`),
    /// the Xbox 360 pad's, and a few obvious spellings.
    pub const ALIASES: [(u32, &str); 18] = [
        (SOUTH, "A"),
        (EAST, "B"),
        (WEST, "X"),
        (NORTH, "Y"),
        (BACK, "View"),
        (START, "Menu"),
        (UP, "DpadUp"),
        (DOWN, "DpadDown"),
        (LEFT, "DpadLeft"),
        (RIGHT, "DpadRight"),
        (GUIDE, "Xbox"),
        (GUIDE, "Home"),
        (GUIDE, "PS"),
        (MISC, "Share"),
        (MISC, "Capture"),
        (MISC, "Mic"),
        (BACK, "Select"),
        (START, "Options"),
    ];
}

/// What kind of controller — for the names printed on its buttons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Family {
    /// Xbox pads, and every pad that is neither of the others.
    #[default]
    Xbox,
    PlayStation,
    Nintendo,
    /// Face buttons A (bottom), X (right), B (left), Y (top).
    GameCube,
}

impl Family {
    pub fn key(self) -> &'static str {
        match self {
            Family::Xbox => "xbox",
            Family::PlayStation => "playstation",
            Family::Nintendo => "nintendo",
            Family::GameCube => "gamecube",
        }
    }

    /// What is printed on the button with this [`pad`] bit.
    pub fn label(self, bit: u32) -> &'static str {
        use pad::*;
        let xbox = match bit {
            SOUTH => "A",
            EAST => "B",
            WEST => "X",
            NORTH => "Y",
            BACK => "View",
            START => "Menu",
            GUIDE => "Xbox",
            MISC => "Share",
            _ => pad::NAMES
                .iter()
                .find(|(known, _)| *known == bit)
                .map_or("?", |(_, name)| *name),
        };
        match self {
            Family::Xbox => xbox,
            Family::PlayStation => match bit {
                SOUTH => "Cross",
                EAST => "Circle",
                WEST => "Square",
                NORTH => "Triangle",
                BACK => "Share",
                START => "Options",
                GUIDE => "PS",
                MISC => "Mic",
                LB => "L1",
                RB => "R1",
                LT => "L2",
                RT => "R2",
                LS => "L3",
                RS => "R3",
                _ => xbox,
            },
            Family::Nintendo => match bit {
                SOUTH => "B",
                EAST => "A",
                WEST => "Y",
                NORTH => "X",
                BACK => "Minus",
                START => "Plus",
                GUIDE => "Home",
                MISC => "Capture",
                LB => "L",
                RB => "R",
                LT => "ZL",
                RT => "ZR",
                _ => xbox,
            },
            Family::GameCube => match bit {
                SOUTH => "A",
                EAST => "X",
                WEST => "B",
                NORTH => "Y",
                START => "Start",
                LT => "L",
                RT => "R",
                _ => xbox,
            },
        }
    }
}

/// What a controller is, for the profile it goes by (`PadProfiles` in the
/// config): one of the three kinds that can have a profile of their own, or
/// any other pad — which always goes by the profile for all controllers.
/// A GameCube pad counts as Nintendo's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PadType {
    Xbox,
    PlayStation,
    Nintendo,
    #[default]
    Other,
}

impl PadType {
    /// As the config and the settings write it.
    pub fn key(self) -> &'static str {
        match self {
            PadType::Xbox => "xbox",
            PadType::PlayStation => "playstation",
            PadType::Nintendo => "nintendo",
            PadType::Other => "other",
        }
    }

    pub fn from_key(key: &str) -> Option<PadType> {
        [PadType::Xbox, PadType::PlayStation, PadType::Nintendo, PadType::Other]
            .into_iter()
            .find(|ty| ty.key() == key)
    }

    /// The profile of its own a pad of this type could have; `All` for
    /// another pad.
    pub fn profile(self) -> Profile {
        match self {
            PadType::Xbox => Profile::Xbox,
            PadType::PlayStation => Profile::PlayStation,
            PadType::Nintendo => Profile::Nintendo,
            PadType::Other => Profile::All,
        }
    }

    /// Whose names its buttons carry when nothing better is known — a
    /// GameCube pad is told apart only while it is connected.
    pub fn family(self) -> Family {
        match self {
            PadType::PlayStation => Family::PlayStation,
            PadType::Nintendo => Family::Nintendo,
            PadType::Xbox | PadType::Other => Family::Xbox,
        }
    }
}

/// A set of controller bindings: the one for every controller, or one for
/// the pads of one type. A type's profile, once it exists, stands in for
/// `All` on those pads entirely — it does not add to it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Profile {
    #[default]
    All,
    Xbox,
    PlayStation,
    Nintendo,
}

impl Profile {
    pub fn key(self) -> &'static str {
        match self {
            Profile::All => "all",
            Profile::Xbox => "xbox",
            Profile::PlayStation => "playstation",
            Profile::Nintendo => "nintendo",
        }
    }
}

/// The profile a pad of type `ty` goes by: its type's own when that exists
/// (`typed` lists the type profiles there are), else the one for all.
pub fn governing(ty: PadType, typed: &[Profile]) -> Profile {
    let own = ty.profile();
    if own != Profile::All && typed.contains(&own) {
        own
    } else {
        Profile::All
    }
}

/// Whose names buttons are shown with when several pads are connected: the
/// pad a button was pressed on last (by SDL's id), or — before any was, or
/// when that one is gone — the first; Xbox's with none.
pub fn family_in_use(pads: &[(u32, Family)], pressed_last: Option<u32>) -> Family {
    pressed_last
        .and_then(|last| pads.iter().find(|(id, _)| *id == last))
        .or(pads.first())
        .map_or(Family::Xbox, |(_, family)| *family)
}

/// The Windows virtual-key code of a key, by the name the shortcut parser
/// gives it (`KeyS`, `Digit4`, `F9`, `Numpad1`, `PrintScreen` …) — the same
/// table the `global-hotkey` crate registers with. `None` for a key that is
/// not on it.
pub fn vk_of(code: &str) -> Option<u16> {
    let single = |rest: &str| -> Option<u8> {
        let mut bytes = rest.bytes();
        let first = bytes.next()?;
        bytes.next().is_none().then_some(first)
    };
    if let Some(letter) = code.strip_prefix("Key").and_then(single) {
        return letter.is_ascii_uppercase().then_some(u16::from(letter));
    }
    if let Some(digit) = code.strip_prefix("Digit").and_then(single) {
        return digit.is_ascii_digit().then_some(u16::from(digit));
    }
    if let Some(digit) = code.strip_prefix("Numpad").and_then(single) {
        if digit.is_ascii_digit() {
            return Some(0x60 + u16::from(digit - b'0'));
        }
    }
    if let Some(number) = code.strip_prefix('F').and_then(|n| n.parse::<u16>().ok()) {
        return (1..=24).contains(&number).then_some(0x6F + number);
    }
    Some(match code {
        "Equal" => 0xBB,
        "Comma" => 0xBC,
        "Minus" => 0xBD,
        "Period" => 0xBE,
        "Semicolon" => 0xBA,
        "Slash" => 0xBF,
        "Backquote" => 0xC0,
        "BracketLeft" => 0xDB,
        "Backslash" => 0xDC,
        "BracketRight" => 0xDD,
        "Quote" => 0xDE,
        "Backspace" => 0x08,
        "Tab" => 0x09,
        "Space" => 0x20,
        "Enter" | "NumpadEnter" => 0x0D,
        "CapsLock" => 0x14,
        "Escape" => 0x1B,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "End" => 0x23,
        "Home" => 0x24,
        "ArrowLeft" => 0x25,
        "ArrowUp" => 0x26,
        "ArrowRight" => 0x27,
        "ArrowDown" => 0x28,
        "PrintScreen" => 0x2C,
        "Insert" => 0x2D,
        "Delete" => 0x2E,
        "NumLock" => 0x90,
        "ScrollLock" => 0x91,
        "Pause" => 0x13,
        "NumpadMultiply" => 0x6A,
        "NumpadAdd" => 0x6B,
        "NumpadSubtract" => 0x6D,
        "NumpadDecimal" => 0x6E,
        "NumpadDivide" => 0x6F,
        "AudioVolumeMute" => 0xAD,
        "AudioVolumeDown" => 0xAE,
        "AudioVolumeUp" => 0xAF,
        "MediaTrackNext" => 0xB0,
        "MediaTrackPrevious" => 0xB1,
        "MediaStop" => 0xB2,
        "MediaPlayPause" => 0xB3,
        _ => return None,
    })
}

/// What is pressed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// A keyboard combination in the shortcut parser's spelling — checked
    /// there, not here.
    Key(String),
    Mouse(MouseButton, Mods),
    /// Controller buttons held together, as [`pad`] bits.
    Pad(u32),
}

/// One binding: what is pressed, and whether tapped or held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub mode: Mode,
    pub trigger: Trigger,
}

/// Strip `prefix` off `text` without regard to case.
fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

fn mouse_button(name: &str) -> Option<MouseButton> {
    MouseButton::ALL
        .into_iter()
        .find(|button| button.name().eq_ignore_ascii_case(name))
}

fn pad_button(name: &str) -> Option<u32> {
    pad::NAMES
        .iter()
        .chain(pad::ALIASES.iter())
        .find(|(_, known)| known.eq_ignore_ascii_case(name))
        .map(|(bit, _)| *bit)
}

/// Read a binding from the config. An empty string is no binding at all and
/// an error here, like a key combination the core does not know.
pub fn parse(text: &str) -> Result<Binding, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("No key combination is set.".into());
    }
    let (mode, rest) = match strip_prefix_ci(text, "Hold:") {
        Some(rest) => (Mode::Hold, rest.trim()),
        None => (Mode::Tap, text),
    };
    if rest.is_empty() {
        return Err("No key combination is set.".into());
    }

    if let Some(buttons) = strip_prefix_ci(rest, "Pad:") {
        let mut mask = 0;
        for name in buttons.split('+').map(str::trim) {
            mask |= pad_button(name)
                .ok_or_else(|| format!("\"{text}\" is not a valid controller combination."))?;
        }
        if mask == 0 {
            return Err(format!("\"{text}\" is not a valid controller combination."));
        }
        return Ok(Binding {
            mode,
            trigger: Trigger::Pad(mask),
        });
    }

    let parts: Vec<&str> = rest.split('+').map(str::trim).collect();
    let last = parts.len() - 1;
    if parts
        .iter()
        .any(|part| part.eq_ignore_ascii_case("Mouse1") || part.eq_ignore_ascii_case("Mouse2"))
    {
        return Err("The left and right mouse buttons cannot be hotkeys.".into());
    }
    if let Some(button) = mouse_button(parts[last]) {
        let mut mods = Mods::NONE;
        for part in &parts[..last] {
            mods = mods.with(match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => Mods::CTRL,
                "alt" => Mods::ALT,
                "shift" => Mods::SHIFT,
                _ => return Err("A mouse button goes with Ctrl, Alt and Shift only.".into()),
            });
        }
        return Ok(Binding {
            mode,
            trigger: Trigger::Mouse(button, mods),
        });
    }
    if parts[..last]
        .iter()
        .any(|part| mouse_button(part).is_some())
    {
        return Err(format!("\"{text}\" is not a valid key combination."));
    }
    Ok(Binding {
        mode,
        trigger: Trigger::Key(rest.to_string()),
    })
}

impl fmt::Display for Trigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Trigger::Key(combo) => f.write_str(combo),
            Trigger::Mouse(button, mods) => {
                for (bit, name) in Mods::NAMES {
                    if mods.contains(bit) {
                        write!(f, "{name}+")?;
                    }
                }
                f.write_str(button.name())
            }
            Trigger::Pad(mask) => {
                f.write_str("Pad:")?;
                let names: Vec<&str> = pad::NAMES
                    .iter()
                    .filter(|(bit, _)| mask & bit != 0)
                    .map(|(_, name)| *name)
                    .collect();
                f.write_str(&names.join("+"))
            }
        }
    }
}

impl fmt::Display for Binding {
    /// The spelling that goes into the config.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mode == Mode::Hold {
            f.write_str("Hold:")?;
        }
        write!(f, "{}", self.trigger)
    }
}

/// A binding as a banner says it: "Alt+C", "Mouse 4", "Hold View+A" — a
/// controller's buttons as printed on a pad of `family` (the pad last used,
/// `AppConfig::console_hotkey_spoken`). Text the parser does not know comes
/// back as it is.
pub fn spoken_for(text: &str, family: Family) -> String {
    let Ok(binding) = parse(text) else {
        return text.trim().to_string();
    };
    let trigger = match &binding.trigger {
        Trigger::Key(combo) => combo.clone(),
        Trigger::Mouse(..) => binding.trigger.to_string().replace("Mouse", "Mouse "),
        Trigger::Pad(mask) => pad::NAMES
            .iter()
            .filter(|(bit, _)| mask & bit != 0)
            .map(|(bit, _)| family.label(*bit))
            .collect::<Vec<_>>()
            .join("+"),
    };
    match binding.mode {
        Mode::Tap => trigger,
        Mode::Hold => format!("Hold {trigger}"),
    }
}

/// Do two of these sit on the same thing, tapped or held alike? Keyboard
/// combinations come with the id the shortcut parser gives them, so
/// `ctrl+s` and `Ctrl+S` are the same; the same key tapped and held is not a
/// clash — that is what holding is for.
pub fn clashes(bindings: &[(Binding, Option<u32>)]) -> bool {
    bindings.iter().enumerate().any(|(at, (one, one_id))| {
        bindings[at + 1..].iter().any(|(two, two_id)| {
            one.mode == two.mode
                && match (one_id, two_id) {
                    (Some(a), Some(b)) => a == b,
                    _ => one.trigger == two.trigger,
                }
        })
    })
}

impl Binding {
    /// A single controller button a game plays with — the game gets the
    /// press too.
    pub fn is_lone_game_button(&self) -> bool {
        matches!(self.trigger, Trigger::Pad(mask)
            if mask.count_ones() == 1 && mask & pad::GAME_BUTTONS != 0)
    }

    /// Guide is part of it — Xbox Game Bar may open on the same press.
    pub fn has_guide(&self) -> bool {
        matches!(self.trigger, Trigger::Pad(mask) if mask & pad::GUIDE != 0)
    }
}

/// One thing that can be pressed, with what it does tapped and held.
///
/// - only a tap: fires on the press — or, for a controller binding inside a
///   bigger bound combo, after [`CHORD_MS`] or on letting go, whichever is
///   first;
/// - only a hold: fires once held for the hold time; let go earlier, nothing;
/// - both: the tap fires on letting go before the hold time, the hold when
///   the hold time passes — and then no tap.
#[derive(Clone, Debug)]
pub struct Slot<A> {
    pub tap: Option<A>,
    pub hold: Option<A>,
    /// How long a lone tap waits for a bigger combo; 0 fires on the press.
    wait_ms: u64,
    hold_ms: u64,
    down: Option<Down>,
}

#[derive(Clone, Copy, Debug)]
struct Down {
    since: u64,
    fired: bool,
}

impl<A: Copy> Slot<A> {
    pub fn new(wait_ms: u64, hold_ms: u64) -> Self {
        Slot {
            tap: None,
            hold: None,
            wait_ms,
            hold_ms,
            down: None,
        }
    }

    pub fn set(&mut self, mode: Mode, action: A) {
        match mode {
            Mode::Tap => self.tap = Some(action),
            Mode::Hold => self.hold = Some(action),
        }
    }

    pub fn is_down(&self) -> bool {
        self.down.is_some()
    }

    /// Pressed at `now` (ms). A second press without a release in between is
    /// the same press.
    pub fn press(&mut self, now: u64) -> Option<A> {
        if self.down.is_some() {
            return None;
        }
        let instant = self.hold.is_none() && self.wait_ms == 0;
        self.down = Some(Down {
            since: now,
            fired: instant,
        });
        if instant {
            self.tap
        } else {
            None
        }
    }

    /// Time passing while held.
    pub fn tick(&mut self, now: u64) -> Option<A> {
        let down = self.down.as_mut()?;
        if down.fired {
            return None;
        }
        let held = now.saturating_sub(down.since);
        if self.hold.is_some() {
            if held >= self.hold_ms {
                down.fired = true;
                return self.hold;
            }
            return None;
        }
        if held >= self.wait_ms {
            down.fired = true;
            return self.tap;
        }
        None
    }

    /// Let go at `now`. What the time owed and no tick delivered yet still
    /// fires.
    pub fn release(&mut self, now: u64) -> Option<A> {
        let down = self.down.take()?;
        if down.fired {
            return None;
        }
        let held = now.saturating_sub(down.since);
        if self.hold.is_some() && held >= self.hold_ms {
            return self.hold;
        }
        self.tap
    }

    /// Gone without counting — a bigger combo took over.
    pub fn cancel(&mut self) {
        self.down = None;
    }
}

/// What the poll thread read in one round.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Input {
    /// Middle, X1, X2 — by [`MouseButton::index`].
    pub mouse: [bool; 3],
    pub mods: Mods,
    /// One mask per controller slot, 0 for an empty one.
    pub pads: [u32; MAX_PADS],
    /// The type of the pad in each slot — which profile its buttons go to.
    pub pad_types: [PadType; MAX_PADS],
}

/// What the controllers did in one round of the poll, all pads together —
/// what the console's navigation works from (`inputpoll::console_nav`): the
/// buttons that went down and up since the last round, and the left stick
/// pushed furthest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PadFrame {
    pub held: u32,
    pub pressed: u32,
    pub released: u32,
    /// The left stick, -32768..32767 each way, down positive.
    pub stick: (i16, i16),
}

impl PadFrame {
    /// The round after one where `before` was held.
    pub fn after(before: u32, pads: &[u32], stick: (i16, i16)) -> PadFrame {
        let held = pads.iter().fold(0, |all, mask| all | mask);
        PadFrame {
            held,
            pressed: held & !before,
            released: before & !held,
            stick,
        }
    }
}

/// What kind of input fired an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Key,
    Mouse,
    Pad,
}

/// Every binding, by what is pressed, and where each one stands.
#[derive(Clone, Debug)]
pub struct Table<A> {
    /// By the shortcut's id, with its key's virtual-key code when known: then
    /// the poll reads the release itself instead of waiting for the shortcut
    /// plugin's, which comes 50–60 ms late and would turn a quick tap near the
    /// hold time into a hold.
    keys: Vec<(u32, Option<u16>, Slot<A>)>,
    mouse: Vec<(MouseButton, Mods, Slot<A>)>,
    /// By profile and buttons: the same combo in two profiles is two slots,
    /// each fed only by the pads that go by its profile.
    pads: Vec<(Profile, u32, Slot<A>)>,
    /// The type profiles there are — each stands in for `All` on its pads,
    /// even with nothing bound in it.
    typed: Vec<Profile>,
    /// Which mouse slot the press of each button went to, so its release
    /// goes there too — whatever happened to the modifiers meanwhile.
    mouse_owner: [Option<usize>; 3],
    mouse_was: [bool; 3],
    /// Combos that were already held when the table came, or that a bigger
    /// combo took over: they count again only after being let go.
    pad_blocked: Vec<bool>,
    /// When each action last fired from a controller, against echoes
    /// ([`PAD_ECHO_MS`]).
    pad_fired_at: Vec<(A, u64)>,
    primed: bool,
}

impl<A: Copy> Default for Table<A> {
    fn default() -> Self {
        Table {
            keys: Vec::new(),
            mouse: Vec::new(),
            pads: Vec::new(),
            typed: Vec::new(),
            mouse_owner: [None; 3],
            mouse_was: [false; 3],
            pad_blocked: Vec::new(),
            pad_fired_at: Vec::new(),
            primed: false,
        }
    }
}

impl<A: Copy + PartialEq> Table<A> {
    /// A keyboard combination, by the id the shortcut parser gives it, and
    /// the virtual-key code of its key ([`vk_of`]) if there is one.
    pub fn bind_key(&mut self, id: u32, vk: Option<u16>, mode: Mode, action: A) {
        let at = match self.keys.iter().position(|(known, _, _)| *known == id) {
            Some(at) => at,
            None => {
                self.keys.push((id, vk, Slot::new(0, HOLD_MS)));
                self.keys.len() - 1
            }
        };
        self.keys[at].2.set(mode, action);
    }

    pub fn bind_mouse(&mut self, button: MouseButton, mods: Mods, mode: Mode, action: A) {
        let at = match self
            .mouse
            .iter()
            .position(|(b, m, _)| *b == button && *m == mods)
        {
            Some(at) => at,
            None => {
                self.mouse.push((button, mods, Slot::new(0, HOLD_MS)));
                self.mouse.len() - 1
            }
        };
        self.mouse[at].2.set(mode, action);
    }

    /// A type profile exists — from now on it stands in for `All` on the
    /// pads of its type, bound or not. `All` itself always exists.
    pub fn add_profile(&mut self, profile: Profile) {
        if profile != Profile::All && !self.typed.contains(&profile) {
            self.typed.push(profile);
        }
    }

    /// A controller binding in `profile` (which then exists).
    pub fn bind_pad(&mut self, profile: Profile, mask: u32, mode: Mode, action: A) {
        self.add_profile(profile);
        let at = match self
            .pads
            .iter()
            .position(|(known_profile, known, _)| *known_profile == profile && *known == mask)
        {
            Some(at) => at,
            None => {
                self.pads.push((profile, mask, Slot::new(0, HOLD_MS)));
                self.pad_blocked.push(false);
                self.pads.len() - 1
            }
        };
        self.pads[at].2.set(mode, action);
        // Part of a bigger combo now bound in the same profile? Then its tap
        // waits a moment for the rest of it.
        let masks: Vec<(Profile, u32)> = self.pads.iter().map(|(p, mask, _)| (*p, *mask)).collect();
        for (profile, mask, slot) in &mut self.pads {
            let inside = masks.iter().any(|(other, bigger)| {
                other == profile && bigger & *mask == *mask && *bigger != *mask
            });
            slot.wait_ms = if inside { CHORD_MS } else { 0 };
        }
    }

    /// The profile a pad of type `ty` goes by in this table.
    pub fn governing(&self, ty: PadType) -> Profile {
        governing(ty, &self.typed)
    }

    pub fn wants_mouse(&self) -> bool {
        !self.mouse.is_empty()
    }

    pub fn wants_pads(&self) -> bool {
        !self.pads.is_empty()
    }

    /// The buttons of every controller combo (two buttons or more) bound,
    /// tapped or held, to an action `of` names. One-button bindings are left
    /// out: they fire on the press and need no waiting for.
    pub fn pad_combo_buttons(&self, of: impl Fn(&A) -> bool) -> u32 {
        self.pads
            .iter()
            .filter(|(_, mask, slot)| {
                mask.count_ones() > 1
                    && (slot.tap.as_ref().is_some_and(&of) || slot.hold.as_ref().is_some_and(&of))
            })
            .fold(0, |all, (_, mask, _)| all | mask)
    }

    /// [`Table::pad_combo_buttons`] for a pad of type `ty`: only the combos
    /// of the profile it goes by.
    pub fn pad_combo_buttons_for(&self, ty: PadType, of: impl Fn(&A) -> bool) -> u32 {
        let profile = self.governing(ty);
        self.pads
            .iter()
            .filter(|(by, mask, slot)| {
                *by == profile
                    && mask.count_ones() > 1
                    && (slot.tap.as_ref().is_some_and(&of) || slot.hold.as_ref().is_some_and(&of))
            })
            .fold(0, |all, (_, mask, _)| all | mask)
    }

    /// Is a key held whose hold still has to be timed? Only then does the
    /// poll thread have to tick for the keyboard.
    pub fn keys_held(&self) -> bool {
        self.keys.iter().any(|(_, _, slot)| slot.is_down())
    }

    /// Is any keyboard combination bound to be held? A game running elevated
    /// hides the key's release from ClippiBoy, and such a hold never comes.
    pub fn key_holds(&self) -> bool {
        self.keys.iter().any(|(_, _, slot)| slot.hold.is_some())
    }

    /// A keyboard combination went down or up. A release is taken only for a
    /// key without a known code — the others' releases the poll reads
    /// ([`Table::held_vks`], [`Table::vk_up`]).
    pub fn key(&mut self, id: u32, pressed: bool, now: u64) -> Option<A> {
        let (_, vk, slot) = self.keys.iter_mut().find(|(known, _, _)| *known == id)?;
        if pressed {
            slot.press(now)
        } else if vk.is_none() {
            slot.release(now)
        } else {
            None
        }
    }

    /// The keys held right now whose release the poll has to look for.
    pub fn held_vks(&self) -> Vec<u16> {
        let mut vks: Vec<u16> = self
            .keys
            .iter()
            .filter(|(_, _, slot)| slot.is_down())
            .filter_map(|(_, vk, _)| *vk)
            .collect();
        vks.dedup();
        vks
    }

    /// The key `vk` was seen let go at `now`.
    pub fn vk_up(&mut self, vk: u16, now: u64) -> Vec<A> {
        self.keys
            .iter_mut()
            .filter(|(_, known, _)| *known == Some(vk))
            .filter_map(|(_, _, slot)| slot.release(now))
            .collect()
    }

    /// The slot a press of `button` with `mods` held goes to: the one asking
    /// for the most of the modifiers held. A bare `Mouse5` fires with Shift
    /// held — one sprints while clipping — unless `Shift+Mouse5` is bound too.
    fn pick_mouse(&self, button: MouseButton, mods: Mods) -> Option<usize> {
        self.mouse
            .iter()
            .enumerate()
            .filter(|(_, (b, m, _))| *b == button && mods.contains(*m))
            .max_by_key(|(_, (_, m, _))| m.count())
            .map(|(at, _)| at)
    }

    /// One round of the poll thread at `now` (ms): edges of the mouse buttons,
    /// combos standing on the controllers, and time for every slot held.
    pub fn poll(&mut self, now: u64, input: &Input) -> Vec<A> {
        self.poll_holding_back(now, input, |_| false)
            .into_iter()
            .map(|(action, _)| action)
            .collect()
    }

    /// [`Table::poll`], with the controller bindings whose action
    /// `held_back` names left out — while the console is open the pads
    /// steer it, and only the binding that closes it may still fire. A
    /// binding held back and still held when that ends counts again only
    /// after being let go; a hold on a binding kept (`Hold:` beside the
    /// console's tap) is dropped while held back. Says which kind of input
    /// fired each action.
    pub fn poll_holding_back(
        &mut self,
        now: u64,
        input: &Input,
        held_back: impl Fn(&A) -> bool,
    ) -> Vec<(A, Source)> {
        // Each pad feeds the slots of the profile it goes by, and no others.
        let profiles: Vec<Profile> = input.pad_types.iter().map(|ty| self.governing(*ty)).collect();
        let satisfied: Vec<bool> = self
            .pads
            .iter()
            .map(|(profile, mask, _)| {
                input
                    .pads
                    .iter()
                    .zip(&profiles)
                    .any(|(pad, by)| by == profile && pad & mask == *mask)
            })
            .collect();
        // A slot all of whose actions are held back is out for now.
        let out: Vec<bool> = self
            .pads
            .iter()
            .map(|(_, _, slot)| {
                let any = slot.tap.is_some() || slot.hold.is_some();
                any && slot.tap.as_ref().is_none_or(&held_back)
                    && slot.hold.as_ref().is_none_or(&held_back)
            })
            .collect();
        if !self.primed {
            // Whatever is held as the bindings arrive was pressed before them —
            // the button just recorded in the settings, say. It counts after
            // it has been let go.
            self.primed = true;
            self.mouse_was = input.mouse;
            self.pad_blocked = satisfied;
            return self
                .keys
                .iter_mut()
                .filter_map(|(_, _, s)| s.tick(now))
                .map(|action| (action, Source::Key))
                .collect();
        }

        let mut fired = Vec::new();
        for button in MouseButton::ALL {
            let at = button.index();
            let down = input.mouse[at];
            if down && !self.mouse_was[at] {
                if let Some(slot) = self.pick_mouse(button, input.mods) {
                    self.mouse_owner[at] = Some(slot);
                    fired.extend(self.mouse[slot].2.press(now).map(|a| (a, Source::Mouse)));
                }
            } else if !down && self.mouse_was[at] {
                if let Some(slot) = self.mouse_owner[at].take() {
                    fired.extend(self.mouse[slot].2.release(now).map(|a| (a, Source::Mouse)));
                }
            }
        }
        self.mouse_was = input.mouse;

        // What a held-back combo would outdo, it outdoes no more: it is not
        // there for the bindings.
        let live: Vec<bool> = satisfied.iter().zip(&out).map(|(s, o)| *s && !*o).collect();
        let mut pad_fired = Vec::new();
        for at in 0..self.pads.len() {
            let (profile, mask) = (self.pads[at].0, self.pads[at].1);
            if !satisfied[at] {
                self.pad_blocked[at] = false;
            }
            if out[at] {
                // Held back: gone without counting, and blocked until let go.
                self.pads[at].2.cancel();
                if satisfied[at] {
                    self.pad_blocked[at] = true;
                }
                continue;
            }
            let outdone = self.pads.iter().enumerate().any(|(other, (by, bigger, _))| {
                other != at
                    && live[other]
                    && *by == profile
                    && bigger & mask == mask
                    && *bigger != mask
            });
            let active = satisfied[at] && !outdone && !self.pad_blocked[at];
            let slot = &mut self.pads[at].2;
            if active && !slot.is_down() {
                pad_fired.extend(slot.press(now));
            } else if !active && slot.is_down() {
                if satisfied[at] {
                    slot.cancel();
                    self.pad_blocked[at] = true;
                } else {
                    pad_fired.extend(slot.release(now));
                }
            }
        }

        for (_, _, slot) in &mut self.keys {
            fired.extend(slot.tick(now).map(|a| (a, Source::Key)));
        }
        for (_, _, slot) in &mut self.mouse {
            fired.extend(slot.tick(now).map(|a| (a, Source::Mouse)));
        }
        for (_, _, slot) in &mut self.pads {
            pad_fired.extend(slot.tick(now));
        }
        // One press, one action: the same action from two profiles in this
        // round, or again within [`PAD_ECHO_MS`], is a remapper's echo.
        self.pad_fired_at
            .retain(|(_, at)| now.saturating_sub(*at) < PAD_ECHO_MS);
        for action in pad_fired {
            if held_back(&action) || self.pad_fired_at.iter().any(|(known, _)| *known == action) {
                continue;
            }
            self.pad_fired_at.push((action, now));
            fired.push((action, Source::Pad));
        }
        fired
    }
}

/// A value worth keeping in the config only once it has stood for a while —
/// the pad last used, which flips on every press with a remapper's virtual
/// pad beside the real one. [`Settled::note`] says what it is now (cheap, on
/// every change); [`Settled::due`] says when to write and what.
#[derive(Clone, Debug)]
pub struct Settled<T> {
    now: Option<T>,
    since: u64,
    written: Option<T>,
    settle_ms: u64,
}

impl<T: Copy + PartialEq> Settled<T> {
    /// `written`: what the config holds already.
    pub fn new(written: Option<T>, settle_ms: u64) -> Self {
        Settled {
            now: written,
            since: 0,
            written,
            settle_ms,
        }
    }

    /// The value at `now_ms`. The same again changes nothing.
    pub fn note(&mut self, value: T, now_ms: u64) {
        if self.now != Some(value) {
            self.now = Some(value);
            self.since = now_ms;
        }
    }

    /// What is to be written at `now_ms` — the value, once it has stood for
    /// the settle time and differs from what was written — and otherwise how
    /// long until it might be (`None` when nothing waits).
    pub fn due(&mut self, now_ms: u64) -> (Option<T>, Option<u64>) {
        if self.now == self.written {
            return (None, None);
        }
        let ready = self.since + self.settle_ms;
        if now_ms >= ready {
            self.written = self.now;
            (self.now, None)
        } else {
            (None, Some(ready - now_ms))
        }
    }
}

/// Records the next mouse button or controller button or combo for the
/// settings, as a trigger in config spelling, without `Hold:` — tap or hold
/// stays as the settings had it.
///
/// A mouse button counts on its press, with the modifiers held then. On a
/// controller it is every button pressed until all are let go again — press
/// Back, then South, let go: `Pad:Back+South`; Guide alone: `Pad:Guide`. What
/// was already held when recording began is left out until it has been let
/// go.
///
/// What it listens to depends on where the settings record: the keyboard's
/// slots take a mouse button, the controller profiles a pad — any pad for
/// `All`, only one of its type for a type's profile.
#[derive(Clone, Debug, Default)]
pub struct Capture {
    filter: CaptureFilter,
    mouse_was: [bool; 3],
    pad_ignored: [u32; MAX_PADS],
    pad_combo: [u32; MAX_PADS],
    primed: bool,
}

/// Which input a recording in the settings takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaptureFilter {
    /// Mouse buttons and every pad — what a page that does not say gets.
    #[default]
    Any,
    /// Mouse buttons only (the keyboard itself the page reads).
    Mouse,
    /// A pad going by this profile's type: any pad for `All`.
    Pad(Profile),
}

impl CaptureFilter {
    /// From the settings: `keys`, `all`, `xbox`, `playstation`, `nintendo`.
    pub fn from_key(key: Option<&str>) -> CaptureFilter {
        match key {
            Some("keys") => CaptureFilter::Mouse,
            Some("all") => CaptureFilter::Pad(Profile::All),
            Some("xbox") => CaptureFilter::Pad(Profile::Xbox),
            Some("playstation") => CaptureFilter::Pad(Profile::PlayStation),
            Some("nintendo") => CaptureFilter::Pad(Profile::Nintendo),
            _ => CaptureFilter::Any,
        }
    }

    fn mouse(self) -> bool {
        matches!(self, CaptureFilter::Any | CaptureFilter::Mouse)
    }

    fn pad(self, ty: PadType) -> bool {
        match self {
            CaptureFilter::Any | CaptureFilter::Pad(Profile::All) => true,
            CaptureFilter::Mouse => false,
            CaptureFilter::Pad(profile) => ty.profile() == profile,
        }
    }
}

impl Capture {
    pub fn new(filter: CaptureFilter) -> Capture {
        Capture {
            filter,
            ..Capture::default()
        }
    }

    pub fn poll(&mut self, input: &Input) -> Option<String> {
        if !self.primed {
            self.primed = true;
            self.mouse_was = input.mouse;
            self.pad_ignored = input.pads;
            return None;
        }
        let mut got = None;
        for button in MouseButton::ALL {
            let at = button.index();
            if self.filter.mouse() && input.mouse[at] && !self.mouse_was[at] && got.is_none() {
                got = Some(Trigger::Mouse(button, input.mods).to_string());
            }
        }
        self.mouse_was = input.mouse;
        for slot in 0..MAX_PADS {
            // A pad of another type is not listened to at all.
            if !self.filter.pad(input.pad_types[slot]) {
                self.pad_combo[slot] = 0;
                continue;
            }
            self.pad_ignored[slot] &= input.pads[slot];
            let live = input.pads[slot] & !self.pad_ignored[slot];
            self.pad_combo[slot] |= live;
            if live == 0 && self.pad_combo[slot] != 0 {
                let combo = std::mem::take(&mut self.pad_combo[slot]);
                if got.is_none() {
                    got = Some(Trigger::Pad(combo).to_string());
                }
            }
        }
        got
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(text: &str) -> String {
        parse(text).unwrap().to_string()
    }

    #[test]
    fn an_old_keyboard_binding_stays_as_it_is() {
        let binding = parse("Ctrl+Shift+S").unwrap();
        assert_eq!(binding.mode, Mode::Tap);
        assert_eq!(binding.trigger, Trigger::Key("Ctrl+Shift+S".into()));
        assert_eq!(binding.to_string(), "Ctrl+Shift+S");
        assert_eq!(round(" F9 "), "F9");
    }

    #[test]
    fn mouse_buttons_parse_with_modifiers_in_a_fixed_order() {
        assert_eq!(
            parse("Mouse4").unwrap().trigger,
            Trigger::Mouse(MouseButton::X1, Mods::NONE)
        );
        assert_eq!(round("mouse5"), "Mouse5");
        assert_eq!(round("Shift+ctrl+Mouse3"), "Ctrl+Shift+Mouse3");
        assert_eq!(round("Control+Alt+Mouse5"), "Ctrl+Alt+Mouse5");
        assert!(parse("Super+Mouse4").is_err());
        assert!(parse("Mouse1").is_err());
        assert!(parse("Ctrl+Mouse2").is_err());
        assert!(parse("Mouse4+A").is_err());
    }

    #[test]
    fn controller_bindings_are_stored_by_position() {
        assert_eq!(
            parse("Pad:Back+South").unwrap().trigger,
            Trigger::Pad(pad::BACK | pad::SOUTH)
        );
        // Written in one order, whatever order they came in.
        assert_eq!(round("pad:south+rb+LB"), "Pad:LB+RB+South");
        assert_eq!(round("Pad:LT+RT+DpadDown"), "Pad:LT+RT+Down");
        assert!(parse("Pad:").is_err());
        assert!(parse("Pad:South+Z").is_err());
    }

    #[test]
    fn bindings_in_the_old_xbox_names_still_read() {
        // As written by 0.9.x, before SDL.
        assert_eq!(
            parse("Pad:View+A").unwrap().trigger,
            Trigger::Pad(pad::BACK | pad::SOUTH)
        );
        assert_eq!(round("Pad:View+A"), "Pad:Back+South");
        assert_eq!(round("Hold:Pad:LB+RB+A"), "Hold:Pad:LB+RB+South");
        assert_eq!(round("Pad:Menu+Y"), "Pad:Start+North");
        assert_eq!(round("Pad:X+B"), "Pad:East+West");
    }

    #[test]
    fn a_single_controller_button_is_a_binding() {
        assert_eq!(parse("Pad:South").unwrap().trigger, Trigger::Pad(pad::SOUTH));
        assert_eq!(round("Pad:A+A"), "Pad:South");
        assert_eq!(round("Hold:Pad:Misc"), "Hold:Pad:Misc");
        assert_eq!(round("Pad:Capture"), "Pad:Misc");
    }

    #[test]
    fn guide_is_bindable_under_any_of_its_names() {
        assert_eq!(parse("Pad:Guide").unwrap().trigger, Trigger::Pad(pad::GUIDE));
        assert_eq!(round("Pad:Xbox"), "Pad:Guide");
        assert_eq!(round("Pad:Home"), "Pad:Guide");
        assert_eq!(round("Pad:PS+A"), "Pad:Guide+South");
        assert!(parse("Pad:Guide").unwrap().has_guide());
        assert!(parse("Hold:Pad:Back+Guide").unwrap().has_guide());
        assert!(!parse("Pad:Back+South").unwrap().has_guide());
        assert!(!parse("Mouse5").unwrap().has_guide());
    }

    #[test]
    fn a_lone_button_the_game_uses_is_told_apart() {
        let lone = |text: &str| parse(text).unwrap().is_lone_game_button();
        assert!(lone("Pad:South"));
        assert!(lone("Hold:Pad:LB"));
        assert!(lone("Pad:RT"));
        assert!(lone("Pad:Up"));
        assert!(lone("Pad:LS"));
        assert!(!lone("Pad:Guide"));
        assert!(!lone("Pad:Misc"));
        assert!(!lone("Pad:Back"));
        assert!(!lone("Pad:Back+South"));
        assert!(!lone("Mouse5"));
    }

    #[test]
    fn hold_is_a_prefix_on_any_binding() {
        let binding = parse("hold: Mouse5").unwrap();
        assert_eq!(binding.mode, Mode::Hold);
        assert_eq!(binding.to_string(), "Hold:Mouse5");
        assert_eq!(round("HOLD:Pad:A+View"), "Hold:Pad:Back+South");
        assert_eq!(round("Hold:Ctrl+Shift+S"), "Hold:Ctrl+Shift+S");
        assert!(parse("Hold:").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn spoken_reads_like_a_person_would_say_it() {
        assert_eq!(spoken_for("Alt+C", Family::Xbox), "Alt+C");
        assert_eq!(spoken_for("Ctrl+Mouse4", Family::Xbox), "Ctrl+Mouse 4");
        assert_eq!(spoken_for("Hold:Pad:View+A", Family::Xbox), "Hold View+A");
        assert_eq!(spoken_for("Pad:Guide", Family::Xbox), "Xbox");
    }

    #[test]
    fn spoken_names_the_buttons_as_printed_on_the_pad() {
        assert_eq!(
            spoken_for("Pad:Back+South", Family::PlayStation),
            "Share+Cross"
        );
        assert_eq!(spoken_for("Pad:Guide", Family::PlayStation), "PS");
        assert_eq!(spoken_for("Pad:LB+RT", Family::PlayStation), "L1+R2");
        // The bottom button is B on a Nintendo pad, the right one A.
        assert_eq!(spoken_for("Pad:South", Family::Nintendo), "B");
        assert_eq!(spoken_for("Pad:East", Family::Nintendo), "A");
        assert_eq!(spoken_for("Hold:Pad:Misc", Family::Nintendo), "Hold Capture");
        assert_eq!(spoken_for("Pad:Guide", Family::Nintendo), "Home");
        assert_eq!(spoken_for("Pad:East+West", Family::GameCube), "X+B");
    }

    #[test]
    fn buttons_are_named_after_the_pad_pressed_last() {
        let pads = [(7, Family::Xbox), (9, Family::Nintendo)];
        assert_eq!(family_in_use(&pads, None), Family::Xbox);
        assert_eq!(family_in_use(&pads, Some(9)), Family::Nintendo);
        assert_eq!(family_in_use(&pads, Some(7)), Family::Xbox);
        // The pad pressed last is gone: the first one again.
        assert_eq!(family_in_use(&pads[1..], Some(7)), Family::Nintendo);
        assert_eq!(family_in_use(&[], Some(7)), Family::Xbox);
    }

    #[test]
    fn bits_follow_sdls_button_order() {
        // SDL_GAMEPAD_BUTTON_SOUTH = 0, GUIDE = 5, DPAD_RIGHT = 14,
        // MISC1 = 15, TOUCHPAD = 20.
        assert_eq!(pad::SOUTH, 1 << 0);
        assert_eq!(pad::GUIDE, 1 << 5);
        assert_eq!(pad::RIGHT, 1 << 14);
        assert_eq!(pad::MISC, 1 << 15);
        assert_eq!(pad::TOUCHPAD, 1 << (pad::BUTTON_COUNT - 1));
        // Every bit has one name to be written in.
        for (bit, name) in pad::NAMES {
            assert_eq!(bit.count_ones(), 1, "{name}");
            assert_eq!(pad_button(name), Some(bit));
        }
    }

    #[test]
    fn a_trigger_counts_past_xinputs_threshold() {
        assert!(!pad::trigger(0));
        assert!(!pad::trigger(3855));
        assert!(pad::trigger(3856));
        assert!(pad::trigger(i16::MAX));
    }

    #[test]
    fn a_clash_is_the_same_thing_pressed_the_same_way() {
        let one = |text: &str, id: Option<u32>| (parse(text).unwrap(), id);
        assert!(!clashes(&[one("Mouse5", None), one("Hold:Mouse5", None)]));
        // Two actions on one pad button, tapped and held, in either spelling;
        // a third on it is a clash.
        assert!(!clashes(&[one("Hold:Pad:View+A", None), one("Pad:Back+South", None)]));
        assert!(clashes(&[
            one("Mouse5", None),
            one("Hold:Mouse5", None),
            one("mouse5", None)
        ]));
        assert!(clashes(&[
            one("Mouse5", None),
            one("F9", Some(1)),
            one("mouse5", None)
        ]));
        assert!(clashes(&[one("Pad:A+View", None), one("Pad:Back+South", None)]));
        assert!(!clashes(&[
            one("Pad:View+A", None),
            one("Pad:View+B", None)
        ]));
        assert!(!clashes(&[one("Pad:Guide", None), one("Hold:Pad:Xbox", None)]));
        assert!(!clashes(&[one("Ctrl+Mouse5", None), one("Mouse5", None)]));
        // Keyboard combinations by the parser's id, whatever their spelling.
        assert!(clashes(&[one("ctrl+s", Some(4)), one("Ctrl+S", Some(4))]));
        assert!(!clashes(&[
            one("Ctrl+S", Some(4)),
            one("Hold:Ctrl+S", Some(4))
        ]));
        assert!(!clashes(&[one("Ctrl+S", Some(4)), one("Ctrl+D", Some(5))]));
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Act {
        Clip,
        Shot,
    }

    #[test]
    fn a_tap_alone_fires_on_the_press() {
        let mut slot = Slot::new(0, HOLD_MS);
        slot.set(Mode::Tap, Act::Clip);
        assert_eq!(slot.press(1000), Some(Act::Clip));
        assert_eq!(slot.tick(2000), None);
        assert_eq!(slot.release(3000), None);
        assert_eq!(slot.press(3100), Some(Act::Clip));
    }

    #[test]
    fn with_tap_and_hold_the_tap_waits_for_the_release() {
        let mut slot = Slot::new(0, HOLD_MS);
        slot.set(Mode::Tap, Act::Clip);
        slot.set(Mode::Hold, Act::Shot);
        assert_eq!(slot.press(1000), None);
        assert_eq!(slot.tick(1200), None);
        assert_eq!(slot.release(1250), Some(Act::Clip));

        assert_eq!(slot.press(2000), None);
        assert_eq!(slot.tick(2399), None);
        assert_eq!(slot.tick(2400), Some(Act::Shot));
        assert_eq!(slot.tick(2600), None);
        // Held past the hold: no tap after it.
        assert_eq!(slot.release(2700), None);
    }

    #[test]
    fn a_hold_alone_needs_the_full_time() {
        let mut slot = Slot::new(0, HOLD_MS);
        slot.set(Mode::Hold, Act::Shot);
        assert_eq!(slot.press(0), None);
        assert_eq!(slot.release(300), None);
        assert_eq!(slot.press(1000), None);
        // A release seen late, with no tick in between, still owes the hold.
        assert_eq!(slot.release(1500), Some(Act::Shot));
    }

    #[test]
    fn a_tap_waiting_for_a_combo_fires_after_the_chord_time_or_on_release() {
        let mut slot = Slot::new(CHORD_MS, HOLD_MS);
        slot.set(Mode::Tap, Act::Clip);
        assert_eq!(slot.press(0), None);
        assert_eq!(slot.tick(49), None);
        assert_eq!(slot.tick(50), Some(Act::Clip));
        assert_eq!(slot.release(400), None);
        // A very quick tap is still a tap.
        assert_eq!(slot.press(1000), None);
        assert_eq!(slot.release(1020), Some(Act::Clip));
    }

    fn mouse(middle: bool, x1: bool, x2: bool, mods: Mods) -> Input {
        Input {
            mouse: [middle, x1, x2],
            mods,
            pads: [0; MAX_PADS],
            pad_types: [PadType::Other; MAX_PADS],
        }
    }

    fn pads(first: u32, second: u32) -> Input {
        let mut input = Input::default();
        input.pads[0] = first;
        input.pads[1] = second;
        input
    }

    #[test]
    fn mouse_tap_and_hold_on_the_same_button() {
        let mut table = Table::default();
        table.bind_mouse(MouseButton::X2, Mods::NONE, Mode::Tap, Act::Clip);
        table.bind_mouse(MouseButton::X2, Mods::NONE, Mode::Hold, Act::Shot);
        let up = mouse(false, false, false, Mods::NONE);
        let x2 = mouse(false, false, true, Mods::NONE);
        assert!(table.poll(0, &up).is_empty());
        assert!(table.poll(10, &x2).is_empty());
        assert_eq!(table.poll(200, &up), vec![Act::Clip]);
        assert!(table.poll(300, &x2).is_empty());
        assert_eq!(table.poll(700, &x2), vec![Act::Shot]);
        assert!(table.poll(900, &up).is_empty());
    }

    #[test]
    fn a_button_held_as_the_bindings_arrive_counts_after_its_release() {
        let mut table = Table::default();
        table.bind_mouse(MouseButton::X1, Mods::NONE, Mode::Tap, Act::Clip);
        let x1 = mouse(false, true, false, Mods::NONE);
        let up = mouse(false, false, false, Mods::NONE);
        assert!(table.poll(0, &x1).is_empty());
        assert!(table.poll(10, &x1).is_empty());
        assert!(table.poll(20, &up).is_empty());
        assert_eq!(table.poll(30, &x1), vec![Act::Clip]);
    }

    #[test]
    fn the_most_specific_mouse_binding_wins() {
        let mut table = Table::default();
        table.bind_mouse(MouseButton::X2, Mods::NONE, Mode::Tap, Act::Clip);
        table.bind_mouse(MouseButton::X2, Mods::CTRL, Mode::Tap, Act::Shot);
        let up = mouse(false, false, false, Mods::NONE);
        table.poll(0, &up);
        // Shift held for sprinting: the bare binding still fires.
        assert_eq!(
            table.poll(10, &mouse(false, false, true, Mods::SHIFT)),
            vec![Act::Clip]
        );
        table.poll(20, &up);
        assert_eq!(
            table.poll(30, &mouse(false, false, true, Mods::CTRL.with(Mods::SHIFT))),
            vec![Act::Shot]
        );
        table.poll(40, &up);

        let mut only_ctrl = Table::default();
        only_ctrl.bind_mouse(MouseButton::Middle, Mods::CTRL, Mode::Tap, Act::Clip);
        only_ctrl.poll(0, &up);
        assert!(only_ctrl
            .poll(10, &mouse(true, false, false, Mods::NONE))
            .is_empty());
    }

    #[test]
    fn a_controller_tap_fires_on_the_press() {
        let mut table = Table::default();
        table.bind_pad(Profile::All, pad::BACK | pad::SOUTH, Mode::Tap, Act::Clip);
        table.bind_pad(Profile::All, pad::GUIDE, Mode::Tap, Act::Shot);
        table.poll(0, &pads(0, 0));
        assert_eq!(
            table.poll(10, &pads(pad::BACK | pad::SOUTH | pad::RT, 0)),
            vec![Act::Clip]
        );
        assert!(table.poll(1000, &pads(pad::BACK | pad::SOUTH, 0)).is_empty());
        assert!(table.poll(1100, &pads(0, 0)).is_empty());
        // A single button too — Guide, on the second pad.
        assert_eq!(table.poll(1200, &pads(0, pad::GUIDE)), vec![Act::Shot]);
        assert!(table.poll(1210, &pads(0, 0)).is_empty());
        // Split across two controllers is no combo.
        assert!(table.poll(3000, &pads(pad::BACK, pad::SOUTH)).is_empty());
        assert!(table.poll(4000, &pads(pad::BACK, pad::SOUTH)).is_empty());
    }

    #[test]
    fn a_controller_binding_waits_only_when_it_has_a_hold_too() {
        let mut table = Table::default();
        table.bind_pad(Profile::All, pad::MISC, Mode::Tap, Act::Clip);
        table.bind_pad(Profile::All, pad::MISC, Mode::Hold, Act::Shot);
        table.poll(0, &pads(0, 0));
        assert!(table.poll(10, &pads(pad::MISC, 0)).is_empty());
        assert_eq!(table.poll(150, &pads(0, 0)), vec![Act::Clip]);
        assert!(table.poll(200, &pads(pad::MISC, 0)).is_empty());
        assert!(table.poll(590, &pads(pad::MISC, 0)).is_empty());
        assert_eq!(table.poll(600, &pads(pad::MISC, 0)), vec![Act::Shot]);
        assert!(table.poll(700, &pads(0, 0)).is_empty());
    }

    #[test]
    fn a_single_button_inside_a_bound_combo_gives_way_to_it() {
        let mut table = Table::default();
        table.bind_pad(Profile::All, pad::SOUTH, Mode::Tap, Act::Clip);
        table.bind_pad(Profile::All, pad::BACK | pad::SOUTH, Mode::Tap, Act::Shot);
        table.poll(0, &pads(0, 0));
        // South a few ms before Back: the combo, not South on its own.
        assert!(table.poll(10, &pads(pad::SOUTH, 0)).is_empty());
        assert_eq!(
            table.poll(30, &pads(pad::BACK | pad::SOUTH, 0)),
            vec![Act::Shot]
        );
        assert!(table.poll(200, &pads(0, 0)).is_empty());
        // South alone: after the chord time.
        assert!(table.poll(1000, &pads(pad::SOUTH, 0)).is_empty());
        assert_eq!(table.poll(1050, &pads(pad::SOUTH, 0)), vec![Act::Clip]);
        assert!(table.poll(1100, &pads(0, 0)).is_empty());
        // Back is not in the way of anything: Back alone does nothing here.
        assert!(table.poll(2000, &pads(pad::BACK, 0)).is_empty());
    }

    #[test]
    fn a_bigger_combo_takes_over_from_the_smaller_one() {
        let mut table = Table::default();
        table.bind_pad(Profile::All, pad::LB | pad::SOUTH, Mode::Tap, Act::Clip);
        table.bind_pad(Profile::All, pad::LB | pad::RB | pad::SOUTH, Mode::Tap, Act::Shot);
        table.poll(0, &pads(0, 0));
        assert!(table.poll(10, &pads(pad::LB | pad::SOUTH, 0)).is_empty());
        assert_eq!(
            table.poll(30, &pads(pad::LB | pad::RB | pad::SOUTH, 0)),
            vec![Act::Shot]
        );
        // RB let go, LB+South still held: the small combo does not fire on top.
        assert!(table.poll(500, &pads(pad::LB | pad::SOUTH, 0)).is_empty());
        assert!(table.poll(1000, &pads(pad::LB | pad::SOUTH, 0)).is_empty());
    }

    #[test]
    fn a_pad_frame_has_the_edges_of_all_pads_together() {
        let frame = PadFrame::after(pad::SOUTH, &[pad::EAST, pad::UP, 0], (0, 100));
        assert_eq!(frame.held, pad::EAST | pad::UP);
        assert_eq!(frame.pressed, pad::EAST | pad::UP);
        assert_eq!(frame.released, pad::SOUTH);
        assert_eq!(frame.stick, (0, 100));
        let still = PadFrame::after(frame.held, &[pad::EAST | pad::UP], (0, 0));
        assert_eq!(still.pressed, 0);
        assert_eq!(still.released, 0);
    }

    #[test]
    fn while_held_back_only_the_kept_pad_binding_fires() {
        // Clip on South, Shot (the console, say) on Back.
        let mut table = Table::default();
        table.bind_pad(Profile::All, pad::SOUTH, Mode::Tap, Act::Clip);
        table.bind_pad(Profile::All, pad::BACK, Mode::Tap, Act::Shot);
        table.bind_mouse(MouseButton::X2, Mods::NONE, Mode::Tap, Act::Clip);
        let back = |a: &Act| *a == Act::Clip;
        assert!(table.poll_holding_back(0, &pads(0, 0), back).is_empty());
        assert!(table.poll_holding_back(10, &pads(pad::SOUTH, 0), back).is_empty());
        assert_eq!(
            table.poll_holding_back(20, &pads(pad::SOUTH | pad::BACK, 0), back),
            vec![(Act::Shot, Source::Pad)]
        );
        // The mouse is not the console's business.
        let mut x2 = pads(pad::SOUTH | pad::BACK, 0);
        x2.mouse[MouseButton::X2.index()] = true;
        assert_eq!(
            table.poll_holding_back(30, &x2, back),
            vec![(Act::Clip, Source::Mouse)]
        );
        // Held back no more, South still held: nothing until let go.
        assert!(table.poll(40, &pads(pad::SOUTH, 0)).is_empty());
        assert!(table.poll(50, &pads(0, 0)).is_empty());
        assert_eq!(table.poll(60, &pads(pad::SOUTH, 0)), vec![Act::Clip]);
    }

    #[test]
    fn the_buttons_of_an_actions_combos() {
        let mut table = Table::default();
        table.bind_pad(Profile::All, pad::BACK | pad::SOUTH, Mode::Tap, Act::Shot);
        table.bind_pad(Profile::All, pad::GUIDE, Mode::Tap, Act::Shot);
        table.bind_pad(Profile::All, pad::LB | pad::RB, Mode::Hold, Act::Shot);
        table.bind_pad(Profile::All, pad::START | pad::NORTH, Mode::Tap, Act::Clip);
        assert_eq!(
            table.pad_combo_buttons(|a| *a == Act::Shot),
            pad::BACK | pad::SOUTH | pad::LB | pad::RB
        );
        assert_eq!(table.pad_combo_buttons(|_| false), 0);
    }

    #[test]
    fn a_held_back_combo_outdoes_no_smaller_one() {
        let mut table = Table::default();
        table.bind_pad(Profile::All, pad::BACK, Mode::Tap, Act::Shot);
        table.bind_pad(Profile::All, pad::BACK | pad::SOUTH, Mode::Tap, Act::Clip);
        let back = |a: &Act| *a == Act::Clip;
        assert!(table.poll_holding_back(0, &pads(0, 0), back).is_empty());
        assert!(table.poll_holding_back(10, &pads(pad::SOUTH, 0), back).is_empty());
        // Back alone waits a moment for the bigger combo — which is out.
        let both = pads(pad::SOUTH | pad::BACK, 0);
        assert!(table.poll_holding_back(20, &both, back).is_empty());
        assert_eq!(
            table.poll_holding_back(20 + CHORD_MS, &both, back),
            vec![(Act::Shot, Source::Pad)]
        );
    }

    #[test]
    fn a_hold_held_back_beside_a_kept_tap_is_dropped() {
        let mut table = Table::default();
        table.bind_pad(Profile::All, pad::BACK, Mode::Tap, Act::Shot);
        table.bind_pad(Profile::All, pad::BACK, Mode::Hold, Act::Clip);
        let back = |a: &Act| *a == Act::Clip;
        assert!(table.poll_holding_back(0, &pads(0, 0), back).is_empty());
        assert!(table.poll_holding_back(10, &pads(pad::BACK, 0), back).is_empty());
        assert!(table.poll_holding_back(500, &pads(pad::BACK, 0), back).is_empty());
        assert!(table.poll_holding_back(510, &pads(0, 0), back).is_empty());
        // The tap still comes through.
        assert!(table.poll_holding_back(520, &pads(pad::BACK, 0), back).is_empty());
        assert_eq!(
            table.poll_holding_back(600, &pads(0, 0), back),
            vec![(Act::Shot, Source::Pad)]
        );
    }

    #[test]
    fn keyboard_holds_are_timed_by_the_poll() {
        let mut table: Table<Act> = Table::default();
        table.bind_key(7, None, Mode::Tap, Act::Clip);
        table.bind_key(7, None, Mode::Hold, Act::Shot);
        assert!(table.key_holds());
        assert_eq!(table.key(7, true, 0), None);
        assert!(table.keys_held());
        assert_eq!(table.poll(200, &Input::default()), vec![]);
        assert_eq!(table.poll(400, &Input::default()), vec![Act::Shot]);
        assert_eq!(table.key(7, false, 500), None);
        assert!(!table.keys_held());
        assert_eq!(table.key(7, true, 1000), None);
        assert_eq!(table.key(7, false, 1100), Some(Act::Clip));
        assert_eq!(table.key(8, true, 1200), None);
    }

    #[test]
    fn a_known_key_is_let_go_when_the_poll_sees_it() {
        let mut table: Table<Act> = Table::default();
        table.bind_key(7, Some(0x53), Mode::Tap, Act::Clip);
        table.bind_key(7, Some(0x53), Mode::Hold, Act::Shot);
        table.bind_key(9, Some(0x78), Mode::Tap, Act::Clip);
        assert!(table.held_vks().is_empty());
        assert_eq!(table.key(7, true, 1000), None);
        assert_eq!(table.held_vks(), vec![0x53]);
        // Let go at 1390, just under the hold: the plugin's release at 1450
        // is ignored, the poll's counts — a tap.
        assert_eq!(table.vk_up(0x53, 1390), vec![Act::Clip]);
        assert_eq!(table.key(7, false, 1450), None);
        assert!(!table.keys_held());
        // A tap-only key fires on the press and is let go by the poll too.
        assert_eq!(table.key(9, true, 2000), Some(Act::Clip));
        assert_eq!(table.held_vks(), vec![0x78]);
        assert!(table.vk_up(0x78, 2100).is_empty());
        assert!(!table.keys_held());
    }

    #[test]
    fn virtual_key_codes_follow_the_windows_table() {
        assert_eq!(vk_of("KeyS"), Some(0x53));
        assert_eq!(vk_of("Digit0"), Some(0x30));
        assert_eq!(vk_of("Numpad7"), Some(0x67));
        assert_eq!(vk_of("F1"), Some(0x70));
        assert_eq!(vk_of("F24"), Some(0x87));
        assert_eq!(vk_of("PrintScreen"), Some(0x2C));
        assert_eq!(vk_of("NumpadEnter"), Some(0x0D));
        assert_eq!(vk_of("F25"), None);
        assert_eq!(vk_of("Keya"), None);
        assert_eq!(vk_of("ContextMenu"), None);
    }

    #[test]
    fn recording_takes_the_next_mouse_button_with_its_modifiers() {
        let mut capture = Capture::default();
        // Held as recording began: not taken.
        assert_eq!(capture.poll(&mouse(false, true, false, Mods::NONE)), None);
        assert_eq!(capture.poll(&mouse(false, true, false, Mods::NONE)), None);
        assert_eq!(
            capture.poll(&mouse(false, true, true, Mods::CTRL)),
            Some("Ctrl+Mouse5".into())
        );
    }

    #[test]
    fn recording_a_combo_waits_until_all_buttons_are_let_go() {
        let mut capture = Capture::default();
        assert_eq!(capture.poll(&pads(pad::RT, 0)), None);
        assert_eq!(capture.poll(&pads(pad::RT | pad::BACK, 0)), None);
        assert_eq!(
            capture.poll(&pads(pad::RT | pad::BACK | pad::SOUTH, 0)),
            None
        );
        assert_eq!(capture.poll(&pads(pad::RT | pad::SOUTH, 0)), None);
        assert_eq!(
            capture.poll(&pads(pad::RT, 0)),
            Some("Pad:Back+South".into())
        );
        // One button alone is a binding too — Guide, say.
        assert_eq!(capture.poll(&pads(0, pad::GUIDE)), None);
        assert_eq!(capture.poll(&pads(0, 0)), Some("Pad:Guide".into()));
    }

    fn typed(slots: &[(u32, PadType)]) -> Input {
        let mut input = Input::default();
        for (at, (mask, ty)) in slots.iter().enumerate() {
            input.pads[at] = *mask;
            input.pad_types[at] = *ty;
        }
        input
    }

    #[test]
    fn a_pad_goes_by_its_types_profile_and_else_by_all() {
        assert_eq!(governing(PadType::Xbox, &[]), Profile::All);
        assert_eq!(governing(PadType::Xbox, &[Profile::Xbox]), Profile::Xbox);
        assert_eq!(governing(PadType::PlayStation, &[Profile::Xbox]), Profile::All);
        // Another pad has no profile of its own.
        assert_eq!(governing(PadType::Other, &[Profile::Xbox]), Profile::All);
        assert_eq!(PadType::from_key("nintendo"), Some(PadType::Nintendo));
        assert_eq!(PadType::from_key("gamecube"), None);
    }

    #[test]
    fn a_type_profile_stands_in_for_all_on_its_pads_only() {
        let mut table: Table<Act> = Table::default();
        // All: Misc saves a clip. Xbox: Guide takes a screenshot, and Misc
        // does nothing there — the profile replaces All, it does not add.
        table.bind_pad(Profile::All, pad::MISC, Mode::Tap, Act::Clip);
        table.bind_pad(Profile::Xbox, pad::GUIDE, Mode::Tap, Act::Shot);
        let idle = typed(&[(0, PadType::Xbox), (0, PadType::Nintendo), (0, PadType::Other)]);
        assert!(table.poll(0, &idle).is_empty());

        let xbox_misc = typed(&[(pad::MISC, PadType::Xbox), (0, PadType::Nintendo), (0, PadType::Other)]);
        assert!(table.poll(10, &xbox_misc).is_empty());
        assert!(table.poll(20, &idle).is_empty());
        let xbox_guide = typed(&[(pad::GUIDE, PadType::Xbox), (0, PadType::Nintendo), (0, PadType::Other)]);
        assert_eq!(table.poll(30, &xbox_guide), vec![Act::Shot]);
        assert!(table.poll(40, &idle).is_empty());

        // The Switch pad has no profile of its own: All's Misc (Capture).
        let switch_misc = typed(&[(0, PadType::Xbox), (pad::MISC, PadType::Nintendo), (0, PadType::Other)]);
        assert_eq!(table.poll(50, &switch_misc), vec![Act::Clip]);
        assert!(table.poll(60, &idle).is_empty());
        let switch_guide = typed(&[(0, PadType::Xbox), (pad::GUIDE, PadType::Nintendo), (0, PadType::Other)]);
        assert!(table.poll(70, &switch_guide).is_empty());
        assert!(table.poll(80, &idle).is_empty());
        // Neither has another pad (pressed later than an echo would come).
        let other_misc = typed(&[(0, PadType::Xbox), (0, PadType::Nintendo), (pad::MISC, PadType::Other)]);
        assert_eq!(table.poll(200, &other_misc), vec![Act::Clip]);
    }

    #[test]
    fn an_empty_type_profile_still_replaces_all() {
        let mut table: Table<Act> = Table::default();
        table.bind_pad(Profile::All, pad::SOUTH, Mode::Tap, Act::Clip);
        table.add_profile(Profile::PlayStation);
        let idle = typed(&[(0, PadType::PlayStation)]);
        assert!(table.poll(0, &idle).is_empty());
        assert!(table.poll(10, &typed(&[(pad::SOUTH, PadType::PlayStation)])).is_empty());
        assert_eq!(table.governing(PadType::PlayStation), Profile::PlayStation);
        assert_eq!(table.governing(PadType::Xbox), Profile::All);
    }

    #[test]
    fn a_combo_waits_only_for_bigger_combos_in_its_own_profile() {
        let mut table: Table<Act> = Table::default();
        // Back+South in All does not make Xbox's lone South wait.
        table.bind_pad(Profile::All, pad::BACK | pad::SOUTH, Mode::Tap, Act::Clip);
        table.bind_pad(Profile::Xbox, pad::SOUTH, Mode::Tap, Act::Shot);
        assert!(table.poll(0, &typed(&[(0, PadType::Xbox)])).is_empty());
        assert_eq!(
            table.poll(10, &typed(&[(pad::SOUTH, PadType::Xbox)])),
            vec![Act::Shot]
        );
    }

    #[test]
    fn recording_for_a_type_takes_only_a_pad_of_that_type() {
        let mut capture = Capture::new(CaptureFilter::from_key(Some("playstation")));
        let xbox_then_ps = |xbox: u32, ps: u32| typed(&[(xbox, PadType::Xbox), (ps, PadType::PlayStation)]);
        assert_eq!(capture.poll(&xbox_then_ps(0, 0)), None);
        // The Xbox pad is not listened to.
        assert_eq!(capture.poll(&xbox_then_ps(pad::GUIDE, 0)), None);
        assert_eq!(capture.poll(&xbox_then_ps(0, 0)), None);
        assert_eq!(capture.poll(&xbox_then_ps(0, pad::GUIDE)), None);
        assert_eq!(capture.poll(&xbox_then_ps(0, 0)), Some("Pad:Guide".into()));
        // Nor is the mouse.
        let mut capture = Capture::new(CaptureFilter::Pad(Profile::Nintendo));
        assert_eq!(capture.poll(&mouse(false, false, false, Mods::NONE)), None);
        assert_eq!(capture.poll(&mouse(false, true, false, Mods::NONE)), None);
    }

    #[test]
    fn recording_for_all_takes_any_pad_and_the_keyboard_only_the_mouse() {
        let mut all = Capture::new(CaptureFilter::from_key(Some("all")));
        assert_eq!(all.poll(&typed(&[(0, PadType::Other)])), None);
        assert_eq!(all.poll(&typed(&[(pad::NORTH, PadType::Other)])), None);
        assert_eq!(all.poll(&typed(&[(0, PadType::Other)])), Some("Pad:North".into()));
        assert_eq!(all.poll(&mouse(false, true, false, Mods::NONE)), None);
        assert_eq!(all.poll(&mouse(false, false, false, Mods::NONE)), None);

        let mut keys = Capture::new(CaptureFilter::from_key(Some("keys")));
        assert_eq!(keys.poll(&typed(&[(0, PadType::Xbox)])), None);
        assert_eq!(keys.poll(&typed(&[(pad::SOUTH, PadType::Xbox)])), None);
        assert_eq!(keys.poll(&typed(&[(0, PadType::Xbox)])), None);
        assert_eq!(
            keys.poll(&mouse(false, false, true, Mods::NONE)),
            Some("Mouse5".into())
        );
    }

    #[test]
    fn a_remappers_echo_fires_once() {
        // DS4Windows: the DualSense goes by its own profile, the virtual Xbox
        // 360 pad it repeats every press on by All — both bind Guide.
        let mut table: Table<Act> = Table::default();
        table.bind_pad(Profile::All, pad::GUIDE, Mode::Tap, Act::Clip);
        table.bind_pad(Profile::PlayStation, pad::GUIDE, Mode::Tap, Act::Clip);
        let both = |real: u32, echo: u32| typed(&[(real, PadType::PlayStation), (echo, PadType::Other)]);
        assert!(table.poll(0, &both(0, 0)).is_empty());
        // In the same round.
        assert_eq!(table.poll(10, &both(pad::GUIDE, pad::GUIDE)), vec![Act::Clip]);
        assert!(table.poll(20, &both(0, 0)).is_empty());
        // A round later.
        assert_eq!(table.poll(200, &both(pad::GUIDE, 0)), vec![Act::Clip]);
        assert!(table.poll(210, &both(pad::GUIDE, pad::GUIDE)).is_empty());
        assert!(table.poll(220, &both(0, 0)).is_empty());
        // A real second press, later, counts.
        assert_eq!(table.poll(400, &both(pad::GUIDE, 0)), vec![Act::Clip]);
    }

    #[test]
    fn the_console_waits_only_for_combos_of_the_pads_own_profile() {
        let mut table: Table<Act> = Table::default();
        table.bind_pad(Profile::All, pad::BACK | pad::SOUTH, Mode::Tap, Act::Clip);
        table.bind_pad(Profile::Xbox, pad::START | pad::NORTH, Mode::Tap, Act::Clip);
        let clip = |action: &Act| *action == Act::Clip;
        assert_eq!(table.pad_combo_buttons_for(PadType::Xbox, clip), pad::START | pad::NORTH);
        assert_eq!(table.pad_combo_buttons_for(PadType::PlayStation, clip), pad::BACK | pad::SOUTH);
        assert_eq!(table.pad_combo_buttons_for(PadType::Other, clip), pad::BACK | pad::SOUTH);
    }

    #[test]
    fn the_pad_in_use_is_written_once_it_has_settled() {
        let mut used = Settled::new(Some(PadType::Xbox), 3_000);
        // As it is: nothing to write.
        assert_eq!(used.due(0), (None, None));
        // A remapper flips it on every press: nothing written meanwhile.
        used.note(PadType::PlayStation, 1_000);
        assert_eq!(used.due(1_500), (None, Some(2_500)));
        used.note(PadType::Other, 2_000);
        used.note(PadType::PlayStation, 3_000);
        used.note(PadType::PlayStation, 3_500);
        assert_eq!(used.due(5_900), (None, Some(100)));
        // Stood for the settle time: once.
        assert_eq!(used.due(6_000), (Some(PadType::PlayStation), None));
        assert_eq!(used.due(9_000), (None, None));
        // Back to what is written before it settled: nothing.
        used.note(PadType::Xbox, 10_000);
        used.note(PadType::PlayStation, 10_500);
        assert_eq!(used.due(20_000), (None, None));
    }
}
