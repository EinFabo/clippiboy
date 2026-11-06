//! The 30 fps grid webcam frames land on, anchored on their own timestamps.

//!
//! The stream that leaves is exact constant frame rate: the newest frame in a
//! slot wins, an empty slot repeats the last frame, a slot goes out once a
//! later frame arrived or 100 ms after its time. From two seconds without a
//! frame the grid sends black fillers and records the gap. A sleep, or a jump
//! in the camera's clock, ends the session; the next frame starts a new one.

/// A slot goes out this long after its time when no later frame proved it complete.
pub const EMIT_AFTER_100NS: i64 = 1_000_000;
/// How long the last frame is repeated before fillers take over.
pub const HOLD_100NS: i64 = 20_000_000;
/// A frame this far off the expected time belongs to a new device clock.
pub const JUMP_100NS: i64 = 20_000_000;
/// QPC ran on this much longer than unbiased time since the last tick: the PC slept.
pub const SUSPEND_100NS: i64 = 10_000_000;

/// The pts of a slot, an integer per slot.
pub fn pts_of(slot: i64, fps: u32) -> i64 {
    slot * 10_000_000 / fps as i64
}

/// The slot a time after the origin falls into, rounded to the nearest.
pub fn slot_of(rel_100ns: i64, fps: u32) -> i64 {
    let scaled = rel_100ns as i128 * fps as i128;
    ((scaled + 5_000_000 * scaled.signum()) / 10_000_000) as i64
}

#[derive(Debug, Clone, PartialEq)]
pub enum Out<T> {
    /// A session begins; the ring is reset to this origin. Comes with the frame
    /// that set it, the very first one too.
    Started { origin_100ns: i64 },
    Frame { pts_100ns: i64, frame: T, duplicate: bool },
    Filler { pts_100ns: i64 },
    GapOpened { from_100ns: i64 },
    GapClosed { from_100ns: i64, to_100ns: i64 },
    /// The session ended.
    Reset { at_100ns: i64 },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GridStats {
    pub received: u64,
    pub emitted: u64,
    pub duplicates: u64,
    pub fillers: u64,
    pub superseded: u64,
    pub late: u64,
    pub resets: u64,
}

pub struct Grid<T: Clone> {
    fps: u32,
    origin: Option<i64>,
    next: i64,
    pending: Option<(i64, T)>,
    last: Option<T>,
    last_frame_qpc: i64,
    gap_open: bool,
    stats: GridStats,
}

impl<T: Clone> Grid<T> {
    pub fn new(fps: u32) -> Self {
        Self {
            fps: fps.max(1),
            origin: None,
            next: 0,
            pending: None,
            last: None,
            last_frame_qpc: 0,
            gap_open: false,
            stats: GridStats::default(),
        }
    }

    pub fn origin(&self) -> Option<i64> {
        self.origin
    }

    pub fn stats(&self) -> GridStats {
        self.stats
    }

    fn start(&mut self, qpc: i64, out: &mut Vec<Out<T>>) {
        self.origin = Some(qpc);
        self.next = 0;
        self.pending = None;
        self.last = None;
        self.gap_open = false;
        out.push(Out::Started { origin_100ns: qpc });
    }

    fn end(&mut self, at: i64, out: &mut Vec<Out<T>>) {
        self.stats.resets += 1;
        self.origin = None;
        self.pending = None;
        self.last = None;
        self.gap_open = false;
        out.push(Out::Reset { at_100ns: at });
    }

    /// A frame stamped `qpc`. The worker ticks before it pushes.
    pub fn push(&mut self, qpc: i64, frame: T, out: &mut Vec<Out<T>>) {
        self.stats.received += 1;
        let Some(origin) = self.origin else {
            self.start(qpc, out);
            self.place(qpc, frame, out);
            return;
        };
        let slot = slot_of(qpc - origin, self.fps);
        let jump_slots = slot_of(JUMP_100NS, self.fps);
        if slot - self.next > jump_slots || qpc < self.last_frame_qpc - JUMP_100NS {
            // A new device clock: what the grid holds belongs to the old one.
            self.end(qpc, out);
            self.start(qpc, out);
        }
        self.place(qpc, frame, out);
    }

    fn place(&mut self, qpc: i64, frame: T, out: &mut Vec<Out<T>>) {
        let origin = self.origin.unwrap_or(qpc);
        let mut slot = slot_of(qpc - origin, self.fps);
        if slot < self.next {
            // Its slot already went out.
            self.stats.late += 1;
            slot = self.next;
        }
        while self.next < slot {
            self.emit_slot(out);
        }
        if self.pending.is_some() {
            self.stats.superseded += 1;
        }
        self.pending = Some((slot, frame));
        if self.gap_open {
            self.gap_open = false;
            out.push(Out::GapClosed { from_100ns: self.last_frame_qpc, to_100ns: qpc });
        }
        self.last_frame_qpc = qpc;
    }

    /// Sends what is due by `now`. `suspended_100ns` is how much longer QPC ran
    /// than unbiased time since the last tick: more than a second means the PC
    /// slept, and the session ends instead of filling the sleep.
    pub fn tick(&mut self, now_100ns: i64, suspended_100ns: i64, out: &mut Vec<Out<T>>) {
        let Some(origin) = self.origin else { return };
        if suspended_100ns > SUSPEND_100NS {
            self.end(now_100ns, out);
            return;
        }
        while now_100ns > origin + pts_of(self.next, self.fps) + EMIT_AFTER_100NS {
            self.emit_slot(out);
        }
    }

    fn emit_slot(&mut self, out: &mut Vec<Out<T>>) {
        let k = self.next;
        let pts = pts_of(k, self.fps);
        let origin = self.origin.unwrap_or(0);
        self.stats.emitted += 1;
        match self.pending.take() {
            Some((slot, frame)) if slot == k => {
                self.last = Some(frame.clone());
                out.push(Out::Frame { pts_100ns: pts, frame, duplicate: false });
            }
            other => {
                self.pending = other;
                match &self.last {
                    Some(last) if origin + pts - self.last_frame_qpc < HOLD_100NS => {
                        self.stats.duplicates += 1;
                        out.push(Out::Frame { pts_100ns: pts, frame: last.clone(), duplicate: true });
                    }
                    _ => {
                        if !self.gap_open {
                            self.gap_open = true;
                            out.push(Out::GapOpened { from_100ns: self.last_frame_qpc });
                        }
                        self.stats.fillers += 1;
                        out.push(Out::Filler { pts_100ns: pts });
                    }
                }
            }
        }
        self.next = k + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: i64 = 333_333;

    fn frames(out: &[Out<u32>]) -> Vec<(i64, u32, bool)> {
        out.iter()
            .filter_map(|o| match o {
                Out::Frame { pts_100ns, frame, duplicate } => Some((*pts_100ns, *frame, *duplicate)),
                _ => None,
            })
            .collect()
    }

    /// Every slot that went out, frame or filler, by its pts.
    fn slots(out: &[Out<u32>]) -> Vec<i64> {
        out.iter()
            .filter_map(|o| match o {
                Out::Frame { pts_100ns, .. } | Out::Filler { pts_100ns } => Some(*pts_100ns),
                _ => None,
            })
            .collect()
    }

    fn consecutive_from(pts: &[i64], first_slot: i64) -> bool {
        pts.iter().enumerate().all(|(i, p)| *p == pts_of(first_slot + i as i64, 30))
    }

    #[test]
    fn every_session_starts_with_its_origin() {
        let mut grid = Grid::new(30);
        let mut out = Vec::new();
        grid.push(5_000, 0u32, &mut out);
        assert_eq!(out[0], Out::Started { origin_100ns: 5_000 });
        out.clear();
        grid.tick(600_000_000, 590_000_000, &mut out);
        out.clear();
        grid.push(600_100_000, 1, &mut out);
        assert_eq!(out[0], Out::Started { origin_100ns: 600_100_000 });
    }

    #[test]
    fn frames_on_time_go_out_one_per_slot() {
        let mut grid = Grid::new(30);
        let mut out = Vec::new();
        for k in 0..30 {
            grid.push(k * P, k as u32, &mut out);
        }
        let got = frames(&out);
        assert_eq!(got.len(), 29);
        assert_eq!(&got[..4].iter().map(|f| f.0).collect::<Vec<_>>(), &[0, 333_333, 666_666, 1_000_000]);
        assert!(got.iter().all(|f| !f.2));
        assert!(got.iter().enumerate().all(|(k, f)| f.1 == k as u32));
    }

    #[test]
    fn two_frames_in_one_slot_keep_the_newest() {
        let mut grid = Grid::new(30);
        let mut out = Vec::new();
        for (qpc, f) in [(0, 0), (400_000, 1), (450_000, 2), (700_000, 3)] {
            grid.push(qpc, f, &mut out);
        }
        assert_eq!(frames(&out), vec![(0, 0, false), (333_333, 2, false)]);
        assert_eq!(grid.stats().superseded, 1);
    }

    #[test]
    fn half_the_frames_shows_each_one_twice() {
        let mut grid = Grid::new(30);
        let mut out = Vec::new();
        for k in 0..15 {
            grid.push(k * 666_667, k as u32, &mut out);
        }
        let got = frames(&out);
        assert_eq!(got.len(), 28);
        assert!(got.iter().enumerate().all(|(i, f)| f.2 == (i % 2 == 1)));
        assert_eq!(grid.stats().duplicates, 14);
        assert!(consecutive_from(&slots(&out), 0));
    }

    #[test]
    fn a_slot_goes_out_100_ms_after_its_time() {
        let mut grid = Grid::new(30);
        let mut out = Vec::new();
        grid.push(0, 7u32, &mut out);
        out.clear();
        grid.tick(1_000_000, 0, &mut out);
        assert!(slots(&out).is_empty());
        grid.tick(1_000_001, 0, &mut out);
        assert_eq!(frames(&out), vec![(0, 7, false)]);
        out.clear();
        grid.tick(P + 1_000_001, 0, &mut out);
        assert_eq!(frames(&out), vec![(P, 7, true)]);
    }

    #[test]
    fn a_dropout_of_two_seconds_turns_into_fillers_and_a_gap() {
        let mut grid = Grid::new(30);
        let mut out = Vec::new();
        grid.push(0, 0u32, &mut out);
        let mut t = P;
        while t <= 30_000_000 {
            grid.tick(t, 0, &mut out);
            t += P;
        }
        grid.tick(30_000_000, 0, &mut out);
        grid.push(30_000_000, 1, &mut out);
        let all = frames(&out);
        // Slot 0 is the frame, slots 1–59 repeat it.
        assert_eq!(all.len(), 60);
        assert!(all[1..].iter().all(|f| f.2 && f.1 == 0));
        let fillers: Vec<i64> =
            out.iter().filter_map(|o| if let Out::Filler { pts_100ns } = o { Some(*pts_100ns) } else { None }).collect();
        assert_eq!(fillers.len(), 30);
        assert_eq!(fillers[0], pts_of(60, 30));
        assert_eq!(*fillers.last().unwrap(), pts_of(89, 30));
        let opened: Vec<_> = out.iter().filter(|o| matches!(o, Out::GapOpened { .. })).collect();
        let closed: Vec<_> = out.iter().filter(|o| matches!(o, Out::GapClosed { .. })).collect();
        assert_eq!(opened, vec![&Out::GapOpened { from_100ns: 0 }]);
        assert_eq!(closed, vec![&Out::GapClosed { from_100ns: 0, to_100ns: 30_000_000 }]);
    }

    #[test]
    fn the_stall_timer_keeps_the_pts_continuous() {
        let mut grid = Grid::new(30);
        let mut out = Vec::new();
        for k in 0..5 {
            grid.push(k * P, 0u32, &mut out);
        }
        let mut t = 4 * P;
        while t < 4 * P + 50_000_000 {
            t += 100_000;
            grid.tick(t, 0, &mut out);
        }
        grid.push(t, 1, &mut out);
        assert!(consecutive_from(&slots(&out), 0));
    }

    #[test]
    fn a_jump_past_two_seconds_starts_over() {
        let mut grid = Grid::new(30);
        let mut out = Vec::new();
        for k in 0..=10 {
            grid.push(k * P, 0u32, &mut out);
        }
        out.clear();
        let stamp = 10 * P + 6_000_000_000;
        grid.push(stamp, 1, &mut out);
        assert!(matches!(out[0], Out::Reset { .. }));
        assert_eq!(out[1], Out::Started { origin_100ns: stamp });
        grid.push(stamp + P, 2, &mut out);
        assert_eq!(frames(&out), vec![(0, 1, false)]);
        assert_eq!(grid.stats().resets, 1);
    }

    #[test]
    fn a_tick_after_a_sleep_starts_over_instead_of_catching_up() {
        let mut grid = Grid::new(30);
        let mut out = Vec::new();
        for k in 0..=10 {
            grid.push(k * P, 0u32, &mut out);
        }
        out.clear();
        grid.tick(10 * P + 6_000_000_000, 6_000_000_000, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Out::Reset { .. }));
    }

    #[test]
    fn a_late_tick_without_a_sleep_only_catches_up() {
        let mut grid = Grid::new(30);
        let mut out = Vec::new();
        for k in 0..=10 {
            grid.push(k * P, 0u32, &mut out);
        }
        out.clear();
        let now = 10 * P + 50_000_000;
        grid.tick(now, 0, &mut out);
        assert!(!out.iter().any(|o| matches!(o, Out::Reset { .. })));
        let got = slots(&out);
        assert!(consecutive_from(&got, 10));
        // Up to 100 ms before now.
        assert!(*got.last().unwrap() + 1_000_000 < now);
        assert!(pts_of(slot_of(*got.last().unwrap(), 30) + 1, 30) + 1_000_000 >= now);
        assert!(out.iter().any(|o| matches!(o, Out::Filler { .. })));
    }

    #[test]
    fn a_late_frame_counts_for_the_next_slot() {
        let mut grid = Grid::new(30);
        let mut out = Vec::new();
        for k in 0..3 {
            grid.push(k * P, k as u32, &mut out);
        }
        grid.tick(2_000_001, 0, &mut out);
        assert_eq!(*slots(&out).last().unwrap(), pts_of(3, 30));
        out.clear();
        grid.push(1_000_000, 9, &mut out);
        assert_eq!(grid.stats().late, 1);
        grid.push(5 * P, 10, &mut out);
        assert_eq!(frames(&out)[0], (pts_of(4, 30), 9, false));
    }

    #[test]
    fn jitter_of_five_ms_costs_no_duplicates() {
        let mut grid = Grid::new(30);
        let mut out = Vec::new();
        for k in 0..60 {
            let jitter = if k % 2 == 0 { 50_000 } else { -50_000 };
            grid.push(k * P + jitter, k as u32, &mut out);
        }
        assert_eq!(grid.stats().duplicates, 0);
        assert!(frames(&out).iter().all(|f| !f.2));
    }

    #[test]
    fn a_clock_that_runs_backwards_starts_over() {
        let mut grid = Grid::new(30);
        let mut out = Vec::new();
        for k in 0..=100 {
            grid.push(k * P, 0u32, &mut out);
        }
        out.clear();
        grid.push(100 * P - 30_000_000, 1, &mut out);
        assert!(matches!(out[0], Out::Reset { .. }));
    }
}
