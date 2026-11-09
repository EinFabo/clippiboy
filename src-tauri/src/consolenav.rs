//! The console driven with a controller — the pure part: what a round of the
//! pads (`bindings::PadFrame`) means for the console page, and when a
//! direction held down repeats.
//!
//! The button scheme is Xbox Game Bar's: D-pad or left stick move, the
//! bottom face button activates, the right one backs out, the bumpers switch
//! panels, Start closes. On a Nintendo pad the confirm and back buttons swap
//! places, the way that pad's own menus have them — A sits on the right there
//! ([`swap_confirm`], per pad).
//!
//! The game still gets every press: ClippiBoy reads the pads beside it and
//! cannot take them away. `inputpoll` reads, the console page moves the ring
//! (`src/console/useNav.ts`).

use crate::bindings::{pad, PadFrame, CHORD_MS};

/// A held direction repeats after this long …
pub const FIRST_REPEAT_MS: u64 = 400;
/// … and then this often.
pub const REPEAT_MS: u64 = 120;
/// The stick counts as pushed past this (of 32767) …
const STICK_ON: i32 = 16_000;
/// … and as let go only below this, so a stick resting near the edge does
/// not stutter on and off.
const STICK_OFF: i32 = 11_000;

/// What the console page is told, as the `console-nav` event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    /// Press what the ring is on.
    Activate,
    /// One step out, the way Escape goes — the last step closes the console.
    Back,
    /// The panel before / after the open one.
    Prev,
    Next,
    /// Close the console at once.
    Close,
}

/// Where the ring is pushed, and since when — one per console opening.
#[derive(Clone, Debug, Default)]
pub struct Navigator {
    /// The direction held, from the D-pad or the stick.
    dir: Option<Nav>,
    /// When it fires again.
    next_at: u64,
    /// The stick's own direction, kept between rounds for the hysteresis.
    stick: Option<Nav>,
    /// Buttons held since before the console opened: they are no steps until
    /// let go — a D-pad direction held through the opening would otherwise
    /// walk the ring off and keep repeating.
    ignore: u32,
    /// The stick was pushed as the console opened: it counts again once it
    /// has come back under [`STICK_OFF`].
    stick_blocked: bool,
    /// Steps of buttons that belong to the console's own binding, waiting
    /// [`CHORD_MS`] for the rest of it (see [`Navigator::step`]).
    waiting: Vec<(Nav, u64)>,
}

impl Navigator {
    /// The console opened with `held` down and the stick at `stick`.
    pub fn new(held: u32, (x, y): (i16, i16)) -> Navigator {
        Navigator {
            ignore: held,
            stick_blocked: i32::from(x).abs().max(i32::from(y).abs()) >= STICK_ON,
            ..Navigator::default()
        }
    }

    /// One round at `now` (ms). `chord`: the buttons of the console's own
    /// binding when that is a combo — Back+South, say. Pressing one of them
    /// may be the start of closing the console, so its step waits
    /// [`CHORD_MS`] (the time a combo's buttons get to come together), and
    /// the binding firing meanwhile takes it back ([`Navigator::drop_waiting`]).
    /// Otherwise South would click whatever the ring stands on as the
    /// console closes.
    ///
    /// The frame is in confirm-at-the-bottom positions: a Nintendo pad's
    /// mask goes through [`swap_confirm`] first.
    pub fn step(&mut self, frame: &PadFrame, now: u64, chord: u32) -> Vec<Nav> {
        let mut fresh = Vec::new();
        self.ignore &= frame.held;
        let held = frame.held & !self.ignore;
        let pressed = frame.pressed & !self.ignore;

        if self.stick_blocked {
            let (x, y) = (i32::from(frame.stick.0), i32::from(frame.stick.1));
            self.stick_blocked = x.abs().max(y.abs()) >= STICK_OFF;
        }
        self.stick = if self.stick_blocked {
            None
        } else {
            stick_dir(frame.stick, self.stick)
        };
        let dpad = dpad_dir(held, pressed, self.dir);
        let dir = dpad.map(|(nav, _)| nav).or(self.stick);
        if dir != self.dir {
            self.dir = dir;
            if let Some(dir) = dir {
                let bit = dpad.map_or(0, |(_, bit)| bit);
                fresh.push((dir, bit));
                self.next_at = now + FIRST_REPEAT_MS;
            }
        } else if let Some(dir) = dir {
            if now >= self.next_at {
                fresh.push((dir, 0));
                // From now, not from when it was due: a round that came late
                // must not fire twice in a row to catch up.
                self.next_at = now + REPEAT_MS;
            }
        }

        for (bit, nav) in [
            (pad::SOUTH, Nav::Activate),
            (pad::EAST, Nav::Back),
            (pad::LB, Nav::Prev),
            (pad::RB, Nav::Next),
            (pad::START, Nav::Close),
        ] {
            if pressed & bit != 0 {
                fresh.push((nav, bit));
            }
        }

        // What waited long enough first — it was pressed earlier.
        let mut out = Vec::new();
        self.waiting.retain(|(nav, due)| {
            if *due <= now {
                out.push(*nav);
                false
            } else {
                true
            }
        });
        for (nav, bit) in fresh {
            if bit & chord != 0 {
                self.waiting.push((nav, now + CHORD_MS));
            } else {
                out.push(nav);
            }
        }
        out
    }

    /// The console's binding fired: the steps of its buttons were part of it.
    pub fn drop_waiting(&mut self) {
        self.waiting.clear();
    }
}

/// A Nintendo pad confirms with A, which sits on the right, and backs out
/// with B at the bottom — the other way round from Xbox and PlayStation.
/// Its mask, with those two swapped, reads like theirs.
pub fn swap_confirm(mask: u32) -> u32 {
    let both = pad::SOUTH | pad::EAST;
    let rest = mask & !both;
    let south = if mask & pad::SOUTH != 0 { pad::EAST } else { 0 };
    let east = if mask & pad::EAST != 0 { pad::SOUTH } else { 0 };
    rest | south | east
}

const DPAD: [(u32, Nav); 4] = [
    (pad::UP, Nav::Up),
    (pad::DOWN, Nav::Down),
    (pad::LEFT, Nav::Left),
    (pad::RIGHT, Nav::Right),
];

/// The D-pad's direction and its button: the one already going while it is
/// still held — a second one pressed beside it (a diagonal) does not take
/// over — else one pressed just now, else any held.
fn dpad_dir(held: u32, pressed: u32, going: Option<Nav>) -> Option<(Nav, u32)> {
    if let Some(entry) = DPAD
        .iter()
        .find(|(bit, nav)| Some(*nav) == going && held & bit != 0)
    {
        return Some((entry.1, entry.0));
    }
    DPAD.iter()
        .find(|(bit, _)| pressed & bit != 0)
        .or_else(|| DPAD.iter().find(|(bit, _)| held & bit != 0))
        .map(|(bit, nav)| (*nav, *bit))
}

/// The stick's direction: the axis pushed further, once past [`STICK_ON`];
/// the one it had stays until it falls under [`STICK_OFF`].
fn stick_dir((x, y): (i16, i16), had: Option<Nav>) -> Option<Nav> {
    let (x, y) = (i32::from(x), i32::from(y));
    let along = |nav: Nav| match nav {
        Nav::Up => -y,
        Nav::Down => y,
        Nav::Left => -x,
        Nav::Right => x,
        _ => 0,
    };
    if let Some(had) = had {
        if along(had) >= STICK_OFF {
            return Some(had);
        }
    }
    if x.abs().max(y.abs()) < STICK_ON {
        return None;
    }
    Some(if x.abs() > y.abs() {
        if x > 0 {
            Nav::Right
        } else {
            Nav::Left
        }
    } else if y > 0 {
        Nav::Down
    } else {
        Nav::Up
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rounds of the poll: `held` is what all pads hold in that round.
    struct Pads {
        nav: Navigator,
        held: u32,
        chord: u32,
    }

    impl Pads {
        /// Starting with `held` already down — the console opened while
        /// buttons were held, which must not count as pressed.
        fn new(held: u32) -> Pads {
            Pads::opened(held, (0, 0), 0)
        }

        /// … with the stick at `stick`, and `chord` the console binding's
        /// buttons.
        fn opened(held: u32, stick: (i16, i16), chord: u32) -> Pads {
            Pads {
                nav: Navigator::new(held, stick),
                held,
                chord,
            }
        }

        fn round(&mut self, now: u64, held: u32, stick: (i16, i16)) -> Vec<Nav> {
            let frame = PadFrame::after(self.held, &[held], stick);
            self.held = frame.held;
            self.nav.step(&frame, now, self.chord)
        }
    }

    #[test]
    fn the_face_buttons_and_bumpers_fire_once_on_the_press() {
        let mut pads = Pads::new(0);
        assert_eq!(pads.round(0, pad::SOUTH, (0, 0)), vec![Nav::Activate]);
        assert!(pads.round(10, pad::SOUTH, (0, 0)).is_empty());
        assert!(pads.round(2000, pad::SOUTH, (0, 0)).is_empty());
        assert!(pads.round(2010, 0, (0, 0)).is_empty());
        assert_eq!(pads.round(2020, pad::EAST, (0, 0)), vec![Nav::Back]);
        assert_eq!(pads.round(2030, pad::LB, (0, 0)), vec![Nav::Prev]);
        assert_eq!(pads.round(2040, pad::RB, (0, 0)), vec![Nav::Next]);
        assert_eq!(pads.round(2050, pad::START, (0, 0)), vec![Nav::Close]);
        // West, North, Back, Guide mean nothing here.
        assert!(pads
            .round(2060, pad::WEST | pad::NORTH | pad::BACK | pad::GUIDE, (0, 0))
            .is_empty());
    }

    #[test]
    fn what_was_held_as_the_console_opened_does_not_count() {
        let mut pads = Pads::new(pad::SOUTH | pad::BACK);
        assert!(pads.round(0, pad::SOUTH | pad::BACK, (0, 0)).is_empty());
        assert!(pads.round(10, 0, (0, 0)).is_empty());
        assert_eq!(pads.round(20, pad::SOUTH, (0, 0)), vec![Nav::Activate]);
    }

    #[test]
    fn a_nintendo_pad_confirms_on_the_right_and_backs_out_at_the_bottom() {
        assert_eq!(swap_confirm(pad::EAST), pad::SOUTH);
        assert_eq!(swap_confirm(pad::SOUTH | pad::UP), pad::EAST | pad::UP);
        assert_eq!(swap_confirm(pad::SOUTH | pad::EAST), pad::SOUTH | pad::EAST);
        assert_eq!(swap_confirm(pad::LB), pad::LB);
        let mut pads = Pads::new(0);
        assert_eq!(pads.round(0, swap_confirm(pad::EAST), (0, 0)), vec![Nav::Activate]);
        assert_eq!(pads.round(10, swap_confirm(pad::SOUTH), (0, 0)), vec![Nav::Back]);
    }

    #[test]
    fn a_direction_held_through_the_opening_is_no_step_until_pressed_again() {
        let mut pads = Pads::new(pad::DOWN);
        assert!(pads.round(0, pad::DOWN, (0, 0)).is_empty());
        assert!(pads.round(500, pad::DOWN, (0, 0)).is_empty());
        assert!(pads.round(1000, pad::DOWN, (0, 0)).is_empty());
        // Another direction beside it still goes.
        assert_eq!(pads.round(1010, pad::DOWN | pad::LEFT, (0, 0)), vec![Nav::Left]);
        assert!(pads.round(1020, 0, (0, 0)).is_empty());
        assert_eq!(pads.round(1030, pad::DOWN, (0, 0)), vec![Nav::Down]);
    }

    #[test]
    fn a_stick_pushed_through_the_opening_counts_once_it_came_back() {
        let mut pads = Pads::opened(0, (30_000, 0), 0);
        assert!(pads.round(0, 0, (30_000, 0)).is_empty());
        assert!(pads.round(600, 0, (30_000, 0)).is_empty());
        // Sagging, but not back yet.
        assert!(pads.round(610, 0, (12_000, 0)).is_empty());
        assert!(pads.round(620, 0, (30_000, 0)).is_empty());
        assert!(pads.round(630, 0, (2_000, 0)).is_empty());
        assert_eq!(pads.round(640, 0, (30_000, 0)), vec![Nav::Right]);
    }

    #[test]
    fn a_button_of_the_consoles_combo_waits_and_goes_when_the_combo_fires() {
        let mut pads = Pads::opened(0, (0, 0), pad::BACK | pad::SOUTH);
        // South alone: a step, only later.
        assert!(pads.round(0, pad::SOUTH, (0, 0)).is_empty());
        assert!(pads.round(40, pad::SOUTH, (0, 0)).is_empty());
        assert_eq!(pads.round(CHORD_MS, pad::SOUTH, (0, 0)), vec![Nav::Activate]);
        // South, and Back right after: the console's binding fires, the
        // step goes.
        assert!(pads.round(100, 0, (0, 0)).is_empty());
        assert!(pads.round(110, pad::SOUTH, (0, 0)).is_empty());
        assert!(pads.round(120, pad::SOUTH | pad::BACK, (0, 0)).is_empty());
        pads.nav.drop_waiting();
        assert!(pads.round(300, pad::SOUTH | pad::BACK, (0, 0)).is_empty());
        // East is not part of it: at once.
        assert_eq!(pads.round(310, pad::EAST, (0, 0)), vec![Nav::Back]);
    }

    #[test]
    fn a_held_direction_repeats_after_a_pause_then_quickly() {
        let mut pads = Pads::new(0);
        assert_eq!(pads.round(0, pad::DOWN, (0, 0)), vec![Nav::Down]);
        assert!(pads.round(10, pad::DOWN, (0, 0)).is_empty());
        assert!(pads.round(399, pad::DOWN, (0, 0)).is_empty());
        assert_eq!(pads.round(400, pad::DOWN, (0, 0)), vec![Nav::Down]);
        assert!(pads.round(519, pad::DOWN, (0, 0)).is_empty());
        assert_eq!(pads.round(520, pad::DOWN, (0, 0)), vec![Nav::Down]);
        // A round late: one step, and the next one a full interval later.
        assert_eq!(pads.round(700, pad::DOWN, (0, 0)), vec![Nav::Down]);
        assert!(pads.round(810, pad::DOWN, (0, 0)).is_empty());
        assert_eq!(pads.round(820, pad::DOWN, (0, 0)), vec![Nav::Down]);
        // Let go and pressed again: at once, and the long pause again.
        assert!(pads.round(830, 0, (0, 0)).is_empty());
        assert_eq!(pads.round(840, pad::DOWN, (0, 0)), vec![Nav::Down]);
        assert!(pads.round(1000, pad::DOWN, (0, 0)).is_empty());
        assert_eq!(pads.round(1240, pad::DOWN, (0, 0)), vec![Nav::Down]);
    }

    #[test]
    fn a_second_direction_beside_the_first_does_not_take_over_until_the_first_goes() {
        let mut pads = Pads::new(0);
        assert_eq!(pads.round(0, pad::RIGHT, (0, 0)), vec![Nav::Right]);
        assert!(pads.round(100, pad::RIGHT | pad::DOWN, (0, 0)).is_empty());
        assert_eq!(pads.round(200, pad::DOWN, (0, 0)), vec![Nav::Down]);
    }

    #[test]
    fn the_stick_moves_past_a_threshold_and_holds_on_until_well_back() {
        let mut pads = Pads::new(0);
        assert!(pads.round(0, 0, (8_000, 0)).is_empty());
        assert_eq!(pads.round(10, 0, (20_000, 3_000)), vec![Nav::Right]);
        // Sagging back a little does not let go — nor fire again.
        assert!(pads.round(20, 0, (13_000, 0)).is_empty());
        assert_eq!(pads.round(410, 0, (13_000, 0)), vec![Nav::Right]);
        // Well back: let go; pushed again: at once.
        assert!(pads.round(420, 0, (5_000, 0)).is_empty());
        assert_eq!(pads.round(430, 0, (0, -30_000)), vec![Nav::Up]);
        assert_eq!(pads.round(440, 0, (0, 30_000)), vec![Nav::Down]);
        assert_eq!(pads.round(450, 0, (-30_000, 1_000)), vec![Nav::Left]);
    }

    #[test]
    fn the_dpad_wins_over_the_stick() {
        let mut pads = Pads::new(0);
        assert_eq!(pads.round(0, 0, (30_000, 0)), vec![Nav::Right]);
        assert_eq!(pads.round(10, pad::UP, (30_000, 0)), vec![Nav::Up]);
        // Up let go, the stick still right: right again, at once.
        assert_eq!(pads.round(20, 0, (30_000, 0)), vec![Nav::Right]);
    }

    #[test]
    fn a_direction_and_a_button_in_one_round_both_come_direction_first() {
        let mut pads = Pads::new(0);
        assert_eq!(
            pads.round(0, pad::LEFT | pad::SOUTH, (0, 0)),
            vec![Nav::Left, Nav::Activate]
        );
    }

    #[test]
    fn the_event_names_are_what_the_page_listens_for() {
        let names: Vec<String> = [
            Nav::Up,
            Nav::Down,
            Nav::Left,
            Nav::Right,
            Nav::Activate,
            Nav::Back,
            Nav::Prev,
            Nav::Next,
            Nav::Close,
        ]
        .iter()
        .map(|nav| serde_json::to_string(nav).unwrap())
        .collect();
        assert_eq!(
            names,
            [
                "\"up\"",
                "\"down\"",
                "\"left\"",
                "\"right\"",
                "\"activate\"",
                "\"back\"",
                "\"prev\"",
                "\"next\"",
                "\"close\""
            ]
        );
    }
}
