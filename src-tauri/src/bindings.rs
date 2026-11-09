//! Hotkeys beyond the keyboard: the mouse's middle and side buttons, Xbox
//! controller combos, and tapping versus holding — the pure part.
//!
//! A binding is still one string in the config, as before, so an old config
//! loads unchanged; each action has a second one beside it (`…_hotkey_alt`),
//! so a mouse button or a controller combo can come on top of the keyboard:
//! - `Ctrl+Shift+S` — a keyboard combination, as `tauri_plugin_global_shortcut`
//!   spells it;
//! - `Mouse3` (middle), `Mouse4`, `Mouse5`, optionally behind `Ctrl`, `Alt`,
//!   `Shift` — `Ctrl+Mouse5`;
//! - `Pad:View+A`, `Pad:LB+RB+A` — at least two buttons held together on an
//!   Xbox controller (XInput). Guide and Share are not in `XInputGetState` and
//!   not offered;
//! - any of these behind `Hold:` — the action fires once the binding has been
//!   held for a moment, so the same button can do one thing tapped and another
//!   held (`Mouse5` saves a clip, `Hold:Mouse5` takes a screenshot).
//!
//! The keys get read in `inputpoll` (keyboard through `RegisterHotKey`, mouse
//! and controller by polling); what they mean — which slot a press goes to,
//! when a tap or a hold fires — is decided here, on timestamps, so it can be
//! tested.

use std::fmt;

/// Held this long, a mouse button or key counts as held rather than tapped.
pub const HOLD_MS: u64 = 400;
/// A controller combo has to stand this long before it counts at all: buttons
/// meet by accident in the middle of play, and a clip should not.
pub const PAD_PRESS_MS: u64 = 300;
/// Held this long, a controller combo counts as held. Above
/// [`PAD_PRESS_MS`] by enough that a tap still has room between the two.
pub const PAD_HOLD_MS: u64 = 800;

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

/// Controller buttons as bits: XInput's own `wButtons` bits, plus the two
/// triggers above them (XInput reports those as 0–255).
pub mod pad {
    pub const UP: u32 = 0x0001;
    pub const DOWN: u32 = 0x0002;
    pub const LEFT: u32 = 0x0004;
    pub const RIGHT: u32 = 0x0008;
    pub const MENU: u32 = 0x0010;
    pub const VIEW: u32 = 0x0020;
    pub const LS: u32 = 0x0040;
    pub const RS: u32 = 0x0080;
    pub const LB: u32 = 0x0100;
    pub const RB: u32 = 0x0200;
    pub const A: u32 = 0x1000;
    pub const B: u32 = 0x2000;
    pub const X: u32 = 0x4000;
    pub const Y: u32 = 0x8000;
    pub const LT: u32 = 0x1_0000;
    pub const RT: u32 = 0x2_0000;

    /// The `wButtons` bits that are buttons one may bind (0x0400 and 0x0800
    /// are not documented — Guide shows up there only through a hidden call).
    pub const BUTTONS: u16 = 0xF3FF;

    /// A trigger pulled past this counts as pressed —
    /// `XINPUT_GAMEPAD_TRIGGER_THRESHOLD`.
    pub const TRIGGER_THRESHOLD: u8 = 30;

    /// Names in the order they are written: shoulders and the small buttons
    /// first, the face buttons last, as one reads a combo.
    pub const NAMES: [(u32, &str); 16] = [
        (LB, "LB"),
        (RB, "RB"),
        (LT, "LT"),
        (RT, "RT"),
        (VIEW, "View"),
        (MENU, "Menu"),
        (LS, "LS"),
        (RS, "RS"),
        (UP, "Up"),
        (DOWN, "Down"),
        (LEFT, "Left"),
        (RIGHT, "Right"),
        (A, "A"),
        (B, "B"),
        (X, "X"),
        (Y, "Y"),
    ];

    /// The old names from the Xbox 360 pad and a few obvious spellings.
    pub const ALIASES: [(u32, &str); 6] = [
        (VIEW, "Back"),
        (MENU, "Start"),
        (UP, "DpadUp"),
        (DOWN, "DpadDown"),
        (LEFT, "DpadLeft"),
        (RIGHT, "DpadRight"),
    ];

    /// One controller's state as bits.
    pub fn mask(buttons: u16, left_trigger: u8, right_trigger: u8) -> u32 {
        let mut mask = u32::from(buttons & BUTTONS);
        if left_trigger > TRIGGER_THRESHOLD {
            mask |= LT;
        }
        if right_trigger > TRIGGER_THRESHOLD {
            mask |= RT;
        }
        mask
    }
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
        if mask.count_ones() < 2 {
            return Err("A controller combination needs at least two buttons.".into());
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

/// A binding as a banner says it: "Alt+C", "Mouse 4", "Hold View+A". Text the
/// parser does not know comes back as it is.
pub fn spoken(text: &str) -> String {
    let Ok(binding) = parse(text) else {
        return text.trim().to_string();
    };
    let trigger = match &binding.trigger {
        Trigger::Key(combo) => combo.clone(),
        Trigger::Mouse(..) => binding.trigger.to_string().replace("Mouse", "Mouse "),
        Trigger::Pad(_) => binding.trigger.to_string()["Pad:".len()..].to_string(),
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

/// One thing that can be pressed, with what it does tapped and held.
///
/// - only a tap: fires on the press, or — for a controller combo — once the
///   combo has stood [`PAD_PRESS_MS`];
/// - only a hold: fires once held for the hold time; let go earlier, nothing;
/// - both: the tap fires on letting go before the hold time, the hold when
///   the hold time passes — and then no tap.
#[derive(Clone, Debug)]
pub struct Slot<A> {
    pub tap: Option<A>,
    pub hold: Option<A>,
    press_ms: u64,
    hold_ms: u64,
    down: Option<Down>,
}

#[derive(Clone, Copy, Debug)]
struct Down {
    since: u64,
    fired: bool,
}

impl<A: Copy> Slot<A> {
    pub fn new(press_ms: u64, hold_ms: u64) -> Self {
        Slot {
            tap: None,
            hold: None,
            press_ms,
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
        let instant = self.hold.is_none() && self.press_ms == 0;
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
        if held >= self.press_ms {
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
        if held >= self.press_ms {
            return self.tap;
        }
        None
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
    pub pads: [u32; 4],
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
    pads: Vec<(u32, Slot<A>)>,
    /// Which mouse slot the press of each button went to, so its release
    /// goes there too — whatever happened to the modifiers meanwhile.
    mouse_owner: [Option<usize>; 3],
    mouse_was: [bool; 3],
    /// Combos that were already held when the table came, or that a bigger
    /// combo took over: they count again only after being let go.
    pad_blocked: Vec<bool>,
    primed: bool,
}

impl<A: Copy> Default for Table<A> {
    fn default() -> Self {
        Table {
            keys: Vec::new(),
            mouse: Vec::new(),
            pads: Vec::new(),
            mouse_owner: [None; 3],
            mouse_was: [false; 3],
            pad_blocked: Vec::new(),
            primed: false,
        }
    }
}

impl<A: Copy> Table<A> {
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

    pub fn bind_pad(&mut self, mask: u32, mode: Mode, action: A) {
        let at = match self.pads.iter().position(|(known, _)| *known == mask) {
            Some(at) => at,
            None => {
                self.pads.push((mask, Slot::new(PAD_PRESS_MS, PAD_HOLD_MS)));
                self.pad_blocked.push(false);
                self.pads.len() - 1
            }
        };
        self.pads[at].1.set(mode, action);
    }

    pub fn wants_mouse(&self) -> bool {
        !self.mouse.is_empty()
    }

    pub fn wants_pads(&self) -> bool {
        !self.pads.is_empty()
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
        let satisfied: Vec<bool> = self
            .pads
            .iter()
            .map(|(mask, _)| input.pads.iter().any(|pad| pad & mask == *mask))
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
                .collect();
        }

        let mut fired = Vec::new();
        for button in MouseButton::ALL {
            let at = button.index();
            let down = input.mouse[at];
            if down && !self.mouse_was[at] {
                if let Some(slot) = self.pick_mouse(button, input.mods) {
                    self.mouse_owner[at] = Some(slot);
                    fired.extend(self.mouse[slot].2.press(now));
                }
            } else if !down && self.mouse_was[at] {
                if let Some(slot) = self.mouse_owner[at].take() {
                    fired.extend(self.mouse[slot].2.release(now));
                }
            }
        }
        self.mouse_was = input.mouse;

        for at in 0..self.pads.len() {
            let mask = self.pads[at].0;
            if !satisfied[at] {
                self.pad_blocked[at] = false;
            }
            let outdone = self.pads.iter().enumerate().any(|(other, (bigger, _))| {
                other != at && satisfied[other] && bigger & mask == mask && *bigger != mask
            });
            let active = satisfied[at] && !outdone && !self.pad_blocked[at];
            let slot = &mut self.pads[at].1;
            if active && !slot.is_down() {
                fired.extend(slot.press(now));
            } else if !active && slot.is_down() {
                if satisfied[at] {
                    slot.cancel();
                    self.pad_blocked[at] = true;
                } else {
                    fired.extend(slot.release(now));
                }
            }
        }

        let slots = self
            .keys
            .iter_mut()
            .map(|(_, _, s)| s)
            .chain(self.mouse.iter_mut().map(|(_, _, s)| s))
            .chain(self.pads.iter_mut().map(|(_, s)| s));
        for slot in slots {
            fired.extend(slot.tick(now));
        }
        fired
    }
}

/// What the settings get while recording a binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Captured {
    /// A trigger in config spelling, without `Hold:` — tap or hold stays as
    /// the settings had it.
    Binding(String),
    /// A single controller button was pressed and let go.
    TooFew,
}

/// Records the next mouse button or controller combo for the settings.
///
/// A mouse button counts on its press, with the modifiers held then. A
/// controller combo is every button pressed until all are let go again —
/// press View, then A, let go: `Pad:View+A`. What was already held when
/// recording began is left out until it has been let go.
#[derive(Clone, Debug, Default)]
pub struct Capture {
    mouse_was: [bool; 3],
    pad_ignored: [u32; 4],
    pad_combo: [u32; 4],
    primed: bool,
}

impl Capture {
    pub fn poll(&mut self, input: &Input) -> Option<Captured> {
        if !self.primed {
            self.primed = true;
            self.mouse_was = input.mouse;
            self.pad_ignored = input.pads;
            return None;
        }
        let mut got = None;
        for button in MouseButton::ALL {
            let at = button.index();
            if input.mouse[at] && !self.mouse_was[at] && got.is_none() {
                got = Some(Captured::Binding(
                    Trigger::Mouse(button, input.mods).to_string(),
                ));
            }
        }
        self.mouse_was = input.mouse;
        for slot in 0..4 {
            self.pad_ignored[slot] &= input.pads[slot];
            let live = input.pads[slot] & !self.pad_ignored[slot];
            self.pad_combo[slot] |= live;
            if live == 0 && self.pad_combo[slot] != 0 {
                let combo = std::mem::take(&mut self.pad_combo[slot]);
                if got.is_none() {
                    got = Some(if combo.count_ones() >= 2 {
                        Captured::Binding(Trigger::Pad(combo).to_string())
                    } else {
                        Captured::TooFew
                    });
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
    fn controller_combos_need_two_known_buttons() {
        assert_eq!(
            parse("Pad:View+A").unwrap().trigger,
            Trigger::Pad(pad::VIEW | pad::A)
        );
        // Written in one order, whatever order they came in.
        assert_eq!(round("pad:a+rb+LB"), "Pad:LB+RB+A");
        assert_eq!(round("Pad:Back+Start"), "Pad:View+Menu");
        assert_eq!(round("Pad:LT+RT+DpadDown"), "Pad:LT+RT+Down");
        assert!(parse("Pad:A").is_err());
        assert!(parse("Pad:A+A").is_err());
        assert!(parse("Pad:Guide+A").is_err());
        assert!(parse("Pad:").is_err());
    }

    #[test]
    fn hold_is_a_prefix_on_any_binding() {
        let binding = parse("hold: Mouse5").unwrap();
        assert_eq!(binding.mode, Mode::Hold);
        assert_eq!(binding.to_string(), "Hold:Mouse5");
        assert_eq!(round("HOLD:Pad:A+View"), "Hold:Pad:View+A");
        assert_eq!(round("Hold:Ctrl+Shift+S"), "Hold:Ctrl+Shift+S");
        assert!(parse("Hold:").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn spoken_reads_like_a_person_would_say_it() {
        assert_eq!(spoken("Alt+C"), "Alt+C");
        assert_eq!(spoken("Ctrl+Mouse4"), "Ctrl+Mouse 4");
        assert_eq!(spoken("Hold:Pad:View+A"), "Hold View+A");
    }

    #[test]
    fn the_trigger_mask_leaves_guide_out_and_counts_pulled_triggers() {
        assert_eq!(pad::mask(0x1000, 0, 0), pad::A);
        assert_eq!(pad::mask(0x0400 | 0x0020, 0, 0), pad::VIEW);
        assert_eq!(pad::mask(0, 31, 30), pad::LT);
        assert_eq!(pad::mask(0, 0, 255), pad::RT);
    }

    #[test]
    fn a_clash_is_the_same_thing_pressed_the_same_way() {
        let one = |text: &str, id: Option<u32>| (parse(text).unwrap(), id);
        assert!(!clashes(&[one("Mouse5", None), one("Hold:Mouse5", None)]));
        assert!(clashes(&[
            one("Mouse5", None),
            one("F9", Some(1)),
            one("mouse5", None)
        ]));
        assert!(clashes(&[one("Pad:A+View", None), one("Pad:View+A", None)]));
        assert!(!clashes(&[
            one("Pad:View+A", None),
            one("Pad:View+B", None)
        ]));
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
    fn a_controller_tap_has_to_stand_a_moment() {
        let mut slot = Slot::new(PAD_PRESS_MS, PAD_HOLD_MS);
        slot.set(Mode::Tap, Act::Clip);
        assert_eq!(slot.press(0), None);
        assert_eq!(slot.release(100), None);
        assert_eq!(slot.press(1000), None);
        assert_eq!(slot.tick(1299), None);
        assert_eq!(slot.tick(1300), Some(Act::Clip));
        assert_eq!(slot.release(1400), None);

        slot.set(Mode::Hold, Act::Shot);
        assert_eq!(slot.press(2000), None);
        assert_eq!(slot.release(2100), None, "too short to be a tap");
        assert_eq!(slot.press(3000), None);
        assert_eq!(slot.release(3500), Some(Act::Clip));
        assert_eq!(slot.press(4000), None);
        assert_eq!(slot.tick(4800), Some(Act::Shot));
    }

    fn mouse(middle: bool, x1: bool, x2: bool, mods: Mods) -> Input {
        Input {
            mouse: [middle, x1, x2],
            mods,
            pads: [0; 4],
        }
    }

    fn pads(first: u32, second: u32) -> Input {
        Input {
            pads: [first, second, 0, 0],
            ..Input::default()
        }
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
    fn a_controller_combo_fires_after_standing_on_one_pad() {
        let mut table = Table::default();
        table.bind_pad(pad::VIEW | pad::A, Mode::Tap, Act::Clip);
        table.poll(0, &pads(0, 0));
        assert!(table
            .poll(10, &pads(pad::VIEW | pad::A | pad::RT, 0))
            .is_empty());
        assert!(table
            .poll(200, &pads(pad::VIEW | pad::A | pad::RT, 0))
            .is_empty());
        assert_eq!(
            table.poll(310, &pads(pad::VIEW | pad::A, 0)),
            vec![Act::Clip]
        );
        assert!(table.poll(1000, &pads(pad::VIEW | pad::A, 0)).is_empty());
        assert!(table.poll(1100, &pads(0, 0)).is_empty());
        // Brushed for a moment in play: nothing.
        assert!(table.poll(2000, &pads(pad::VIEW | pad::A, 0)).is_empty());
        assert!(table.poll(2100, &pads(0, 0)).is_empty());
        // Split across two controllers is no combo.
        assert!(table.poll(3000, &pads(pad::VIEW, pad::A)).is_empty());
        assert!(table.poll(4000, &pads(pad::VIEW, pad::A)).is_empty());
    }

    #[test]
    fn a_bigger_combo_takes_over_from_the_smaller_one() {
        let mut table = Table::default();
        table.bind_pad(pad::LB | pad::A, Mode::Tap, Act::Clip);
        table.bind_pad(pad::LB | pad::RB | pad::A, Mode::Tap, Act::Shot);
        table.poll(0, &pads(0, 0));
        assert!(table.poll(10, &pads(pad::LB | pad::A, 0)).is_empty());
        assert!(table
            .poll(100, &pads(pad::LB | pad::RB | pad::A, 0))
            .is_empty());
        assert_eq!(
            table.poll(400, &pads(pad::LB | pad::RB | pad::A, 0)),
            vec![Act::Shot]
        );
        // RB let go, LB+A still held: the small combo does not fire on top.
        assert!(table.poll(500, &pads(pad::LB | pad::A, 0)).is_empty());
        assert!(table.poll(1000, &pads(pad::LB | pad::A, 0)).is_empty());
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
            Some(Captured::Binding("Ctrl+Mouse5".into()))
        );
    }

    #[test]
    fn recording_a_combo_waits_until_all_buttons_are_let_go() {
        let mut capture = Capture::default();
        assert_eq!(capture.poll(&pads(pad::RT, 0)), None);
        assert_eq!(capture.poll(&pads(pad::RT | pad::VIEW, 0)), None);
        assert_eq!(capture.poll(&pads(pad::RT | pad::VIEW | pad::A, 0)), None);
        assert_eq!(capture.poll(&pads(pad::RT | pad::A, 0)), None);
        assert_eq!(
            capture.poll(&pads(pad::RT, 0)),
            Some(Captured::Binding("Pad:View+A".into()))
        );
        assert_eq!(capture.poll(&pads(0, pad::B)), None);
        assert_eq!(capture.poll(&pads(0, 0)), Some(Captured::TooFew));
    }
}
