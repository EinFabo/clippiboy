//! The console driven with a controller — the pure part: what a round of the
//! pads (`bindings::PadFrame`) means for the console page, and when a
//! direction held down repeats.
//!
//! The button scheme is Xbox Game Bar's: D-pad or left stick move, the
//! bottom face button activates, the right one backs out, the bumpers switch
//! panels, Start closes. On a Nintendo pad the confirm and back buttons swap
//! places, the way that pad's own menus have them — A sits on the right there.
//!
//! The game still gets every press: ClippiBoy reads the pads beside it and
//! cannot take them away. `inputpoll` reads, the console page moves the ring
//! (`src/console/useNav.ts`).

use crate::bindings::{pad, PadFrame};

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
}

impl Navigator {
    /// One round at `now` (ms). `nintendo`: confirm and back swap sides.
    pub fn step(&mut self, frame: &PadFrame, now: u64, nintendo: bool) -> Vec<Nav> {
        let mut out = Vec::new();

        self.stick = stick_dir(frame.stick, self.stick);
        let dir = dpad_dir(frame, self.dir).or(self.stick);
        if dir != self.dir {
            self.dir = dir;
            if let Some(dir) = dir {
                out.push(dir);
                self.next_at = now + FIRST_REPEAT_MS;
            }
        } else if let Some(dir) = dir {
            if now >= self.next_at {
                out.push(dir);
                // From now, not from when it was due: a round that came late
                // must not fire twice in a row to catch up.
                self.next_at = now + REPEAT_MS;
            }
        }

        let (confirm, back) = if nintendo {
            (pad::EAST, pad::SOUTH)
        } else {
            (pad::SOUTH, pad::EAST)
        };
        for (bit, nav) in [
            (confirm, Nav::Activate),
            (back, Nav::Back),
            (pad::LB, Nav::Prev),
            (pad::RB, Nav::Next),
            (pad::START, Nav::Close),
        ] {
            if frame.pressed & bit != 0 {
                out.push(nav);
            }
        }
        out
    }
}

const DPAD: [(u32, Nav); 4] = [
    (pad::UP, Nav::Up),
    (pad::DOWN, Nav::Down),
    (pad::LEFT, Nav::Left),
    (pad::RIGHT, Nav::Right),
];

/// The D-pad's direction: the one already going while it is still held — a
/// second one pressed beside it (a diagonal) does not take over — else one
/// pressed just now, else any held.
fn dpad_dir(frame: &PadFrame, going: Option<Nav>) -> Option<Nav> {
    let held = |nav: Nav| {
        DPAD.iter()
            .find(|(_, n)| *n == nav)
            .is_some_and(|(bit, _)| frame.held & bit != 0)
    };
    if let Some(going) = going.filter(|nav| held(*nav)) {
        return Some(going);
    }
    DPAD.iter()
        .find(|(bit, _)| frame.pressed & bit != 0)
        .or_else(|| DPAD.iter().find(|(bit, _)| frame.held & bit != 0))
        .map(|(_, nav)| *nav)
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
    }

    impl Pads {
        /// Starting with `held` already down — the console opened while
        /// buttons were held, which must not count as pressed.
        fn new(held: u32) -> Pads {
            Pads {
                nav: Navigator::default(),
                held,
            }
        }

        fn round(&mut self, now: u64, held: u32, stick: (i16, i16)) -> Vec<Nav> {
            let frame = PadFrame::after(self.held, &[held], stick);
            self.held = frame.held;
            self.nav.step(&frame, now, false)
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
        let mut nav = Navigator::default();
        let frame = PadFrame::after(0, &[pad::EAST], (0, 0));
        assert_eq!(nav.step(&frame, 0, true), vec![Nav::Activate]);
        let frame = PadFrame::after(0, &[pad::SOUTH], (0, 0));
        assert_eq!(nav.step(&frame, 10, true), vec![Nav::Back]);
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
