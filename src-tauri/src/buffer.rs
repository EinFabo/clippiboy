//! Ring buffer over *already encoded* packets.
//!
//! Buffering raw frames would be unaffordable (5 min of 1080p60 BGRA ≈ 90 GB);
//! encoded at 40 Mbit/s it is ≈ 1.5 GB. Saving a clip is therefore pure muxing
//! with no re-encode.
//!
//! Important: the buffer may only be trimmed at the front up to a video
//! keyframe, otherwise the start of the clip is not decodable.

use std::collections::VecDeque;
use std::sync::Arc;

/// Track 0 is always video, the audio tracks follow from 1.
pub const VIDEO_TRACK: u32 = 0;

/// A single step between neighbouring frame timestamps longer than this is
/// not an encoder falling short but a break in time: the PC slept, or the
/// process stood still. Below it, frames that are missing were lost, and the
/// ones there are spread over the time they cover (`muxer::timing`). The same
/// 2 s the clock uses to stop counting skipped ticks (`convert::GAP`).
pub const DISCONTINUITY_US: i64 = 2_000_000;

/// The longest step between neighbouring frames that a clip still keeps as
/// time that passed: three keyframe intervals, and at least a second. A hang
/// up to this long stays in the clip as a held frame, in sync with its sound
/// (`muxer::keeps_timestamps`); a longer one is a break the ring is meant to
/// cut (BUF-5), not something to play back. A clip caps it further at what
/// ffmpeg leaves alone in a TS (`pipeline::ts_jump_limit_us`).
pub fn jump_limit_us(keyframe_seconds: u32) -> i64 {
    (3 * keyframe_seconds as i64 * 1_000_000).max(1_000_000)
}

#[derive(Debug, Clone)]
pub struct EncodedPacket {
    pub track: u32,
    pub pts_us: i64,
    pub duration_us: i64,
    pub keyframe: bool,
    pub data: Arc<[u8]>,
}

impl EncodedPacket {
    pub fn is_video(&self) -> bool {
        self.track == VIDEO_TRACK
    }
}

pub struct ReplayBuffer {
    capacity_us: i64,
    /// Hard ceiling on what the ring may hold.
    ///
    /// The length alone stopped being enough once the encoder was allowed to
    /// pick its own bitrate for a constant quality: a quiet menu costs a
    /// fraction of a firefight, so the same two minutes are no longer the same
    /// number of bytes. Without this the ring would follow the picture straight
    /// into memory the machine does not have.
    capacity_bytes: u64,
    packets: VecDeque<EncodedPacket>,
    /// Has the encoder's silence about keyframes already been reported? Once is
    /// enough — but once is needed, or the buffer looks merely empty.
    warned_about_keyframes: bool,
    /// Sequence number of the frontmost packet — for stable keyframe positions.
    head_seq: u64,
    next_seq: u64,
    /// (seq, pts) of every video keyframe still in the buffer.
    keyframes: VecDeque<(u64, i64)>,
    bytes: u64,
    newest_pts: i64,
    /// Has a video packet come in behind the newest one since the last clear?
    /// An encoder with B-frames does that, and its stream cannot go into a
    /// TS without DTS ([`crate::ts`]).
    reorders: bool,
    /// A step forward between neighbouring packets longer than this is a
    /// break in time (BUF-5), not a stall: the PC slept with the buffer
    /// running. `i64::MAX`, the default, never sees one.
    jump_us: i64,
    /// Sequence number of the first packet after the latest break in time,
    /// while no keyframe has followed it yet. Nothing before it may be cut,
    /// and the first video keyframe at or after it drops everything older.
    cut_seq: Option<u64>,
    /// Has the stream that flags only its first keyframe been reported?
    warned_about_one_keyframe: bool,
}

impl ReplayBuffer {
    pub fn new(capacity_seconds: u32, capacity_bytes: u64) -> Self {
        Self {
            capacity_us: capacity_seconds as i64 * 1_000_000,
            capacity_bytes,
            packets: VecDeque::new(),
            warned_about_keyframes: false,
            head_seq: 0,
            next_seq: 0,
            keyframes: VecDeque::new(),
            bytes: 0,
            newest_pts: 0,
            reorders: false,
            jump_us: i64::MAX,
            cut_seq: None,
            warned_about_one_keyframe: false,
        }
    }

    /// The longest step forward that is still time passing; see
    /// [`jump_limit_us`] for the game ring's.
    pub fn set_jump_limit_us(&mut self, us: i64) {
        self.jump_us = us;
    }

    pub fn set_capacity(&mut self, seconds: u32, bytes: u64) {
        self.capacity_us = seconds as i64 * 1_000_000;
        self.capacity_bytes = bytes;
        self.trim();
    }

    pub fn push(&mut self, packet: EncodedPacket) {
        // Before `newest_pts` moves: a picture behind the newest one so far.
        if packet.is_video() && packet.pts_us < self.newest_pts && !self.reorders {
            self.reorders = true;
            // Once until the next clear: the field log says why recordings
            // take the raw path from here on.
            log::info!(
                "the encoder reorders its frames (B-frames) — recordings keep the raw stream"
            );
        }
        let seq = self.next_seq;
        let step = packet.pts_us.saturating_sub(self.newest_pts);
        if !self.packets.is_empty() && step > self.jump_us {
            // A later jump replaces an earlier one still waiting: what lies
            // between the two is from before a break in time as well.
            self.cut_seq = Some(seq);
            log::info!(
                "the stream jumped {:.1} s ahead (the PC slept?) — the buffer starts over at the next keyframe",
                step as f64 / 1_000_000.0
            );
        }
        if packet.is_video() && packet.keyframe {
            if self.cut_seq.is_some_and(|cut| seq >= cut) {
                // Every packet so far lies before this one, and every one
                // from before the jump goes.
                self.packets.clear();
                self.keyframes.clear();
                self.bytes = 0;
                self.head_seq = seq;
                self.cut_seq = None;
            }
            self.keyframes.push_back((seq, packet.pts_us));
        }
        self.newest_pts = self.newest_pts.max(packet.pts_us);
        self.bytes += packet.data.len() as u64;
        self.packets.push_back(packet);
        self.next_seq += 1;
        self.trim();
    }

    /// Drops from the front as long as the full buffer length starting at the
    /// *next* keyframe still remains afterwards — or as long as the ring is over
    /// its memory budget.
    ///
    /// Whole groups of pictures go at a time, and never the last one: a ring
    /// that starts anywhere but a keyframe is not decodable.
    fn trim(&mut self) {
        while self.keyframes.len() > 1 {
            let (seq, pts) = self.keyframes[1];
            let too_long = self.newest_pts - pts >= self.capacity_us;
            let too_big = self.bytes > self.capacity_bytes;
            if !too_long && !too_big {
                break;
            }
            let drop_count = (seq - self.head_seq) as usize;
            for _ in 0..drop_count {
                if let Some(p) = self.packets.pop_front() {
                    self.bytes -= p.data.len() as u64;
                }
            }
            self.head_seq = seq;
            self.keyframes.pop_front();
        }

        self.give_up_a_lone_keyframe();
        self.enforce_budget_without_keyframes();
    }

    /// The safety valve for a stream that flags its first keyframe and none
    /// after (BUF-4): the loop above needs a second one to cut at, the floor
    /// below needs none, so with exactly one the ring would ignore both limits
    /// until the machine gives out.
    ///
    /// Once that keyframe is older than the ring is long — and either the ring
    /// is twice over its budget or the keyframe is 10 s past the length, more
    /// than any keyframe interval — its entry goes, and the floor holds the
    /// ring until another one arrives. A young last keyframe is never given
    /// up, whatever the budget: that is the group of pictures a save needs.
    ///
    /// A jump waiting for its keyframe is the same fault seen from the other
    /// side: if none follows within the same time, what lies before the jump
    /// goes and the floor takes over.
    fn give_up_a_lone_keyframe(&mut self) {
        let long_past = self.capacity_us.saturating_add(10_000_000);
        if let Some(cut) = self.cut_seq {
            let Some(jump_pts) = cut
                .checked_sub(self.head_seq)
                .and_then(|i| self.packets.get(i as usize))
                .map(|p| p.pts_us)
            else {
                return;
            };
            if self.newest_pts - jump_pts <= long_past {
                return;
            }
            for _ in self.head_seq..cut {
                if let Some(p) = self.packets.pop_front() {
                    self.bytes -= p.data.len() as u64;
                }
            }
            self.head_seq = cut;
            self.cut_seq = None;
            // Every keyframe left lay before the jump. None since: the
            // floor's own line is the true one here.
            self.keyframes.clear();
            return;
        }
        if self.keyframes.len() != 1 {
            return;
        }
        let age = self.newest_pts - self.keyframes[0].1;
        let too_big =
            age >= self.capacity_us && self.bytes > self.capacity_bytes.saturating_mul(2);
        if !too_big && age <= long_past {
            return;
        }
        self.keyframes.clear();
        // The floor's own line ("not flagging any keyframes") would be wrong
        // here: the encoder did flag one.
        self.warned_about_keyframes = true;
        if !self.warned_about_one_keyframe {
            self.warned_about_one_keyframe = true;
            log::warn!(
                "the encoder flags only its first keyframe — the buffer is being held to its \
                 budget by force"
            );
        }
    }

    /// The safety valve for a stream that never declares a keyframe.
    ///
    /// Everything above cuts *between* keyframes, which is right — a ring that
    /// starts anywhere else is not decodable, and the last group of pictures is
    /// deliberately never given up. But it also means both limits hang off the
    /// keyframe list, and a transform that does not flag its keyframes leaves
    /// them powerless: the ring then grows past its length and past its memory
    /// budget until the machine gives out, while the UI shows a buffer that never
    /// fills, because `buffered_seconds` has nothing to measure from either.
    ///
    /// `mft::opens_a_gop` is what should keep this from ever happening. It is the
    /// second line rather than the first: memory has to win over decodability,
    /// because a buffer that cannot be saved is a fault, and a buffer that takes
    /// the app down with it is a worse one.
    fn enforce_budget_without_keyframes(&mut self) {
        if !self.keyframes.is_empty() || self.packets.is_empty() {
            return;
        }
        if !self.warned_about_keyframes {
            self.warned_about_keyframes = true;
            log::warn!(
                "the encoder is not flagging any keyframes — clips cannot be cut out of this \
                 stream, and the buffer is being held to its budget by force"
            );
        }
        while let Some(front) = self.packets.front() {
            let too_long = self.newest_pts - front.pts_us >= self.capacity_us;
            let too_big = self.bytes > self.capacity_bytes;
            if !too_long && !too_big {
                break;
            }
            let bytes = front.data.len() as u64;
            self.packets.pop_front();
            self.bytes -= bytes;
            self.head_seq += 1;
        }
    }

    pub fn clear(&mut self) {
        self.packets.clear();
        self.keyframes.clear();
        self.bytes = 0;
        self.head_seq = self.next_seq;
        self.newest_pts = 0;
        self.reorders = false;
        self.cut_seq = None;
    }

    /// Does the encoder hand out pictures out of order (B-frames)? Then a
    /// recording keeps the raw stream rather than its timestamps.
    pub fn reorders(&self) -> bool {
        self.reorders
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// The latest pts the ring has seen, in µs.
    pub fn newest_pts(&self) -> i64 {
        self.newest_pts
    }

    pub fn len(&self) -> usize {
        self.packets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }

    /// Duration actually buffered, in seconds (from the first keyframe).
    /// Nothing while a jump in time waits for its keyframe: none of it can
    /// be saved.
    pub fn buffered_seconds(&self) -> f32 {
        if self.cut_seq.is_some() {
            return 0.0;
        }
        match self.keyframes.front() {
            Some((_, pts)) => (self.newest_pts - pts) as f32 / 1_000_000.0,
            None => 0.0,
        }
    }

    /// The keyframes a cut may begin on: none from before a jump in time.
    fn usable_keyframes(&self) -> impl DoubleEndedIterator<Item = &(u64, i64)> {
        let cut = self.cut_seq.unwrap_or(0);
        self.keyframes.iter().filter(move |(seq, _)| *seq >= cut)
    }

    /// Snapshot of the last `seconds` seconds, starting at the last keyframe
    /// *before* that point in time (so the video is decodable).
    pub fn snapshot(&self, seconds: u32) -> Vec<EncodedPacket> {
        self.snapshot_range(self.newest_pts - seconds as i64 * 1_000_000, i64::MAX)
    }

    /// The packets from `from_us` to `to_us`, starting at the newest keyframe
    /// at or before `from_us` (else the front keyframe), so the cut is
    /// decodable. Absolute times, for a ring cut by another ring's window.
    pub fn snapshot_range(&self, from_us: i64, to_us: i64) -> Vec<EncodedPacket> {
        let start_pts = self
            .usable_keyframes()
            .rev()
            .map(|(_, pts)| *pts)
            .find(|pts| *pts <= from_us)
            .or_else(|| self.usable_keyframes().next().map(|(_, pts)| *pts));

        let Some(start_pts) = start_pts else {
            return Vec::new();
        };

        self.packets
            .iter()
            .filter(|p| p.pts_us >= start_pts && p.pts_us <= to_us)
            .cloned()
            .collect()
    }

    /// Is there a keyframe to begin a recording on? Not one from before a
    /// jump in time.
    pub fn has_keyframe(&self) -> bool {
        self.usable_keyframes().next().is_some()
    }

    /// Everything from the newest keyframe on — where a recording begins.
    ///
    /// Not the moment of the key press: the stream is only decodable from a
    /// keyframe, and reaching back to the one before costs at most one
    /// keyframe interval. Waiting for the next one instead would lose that
    /// much at the start.
    pub fn from_last_keyframe(&self) -> Vec<EncodedPacket> {
        let Some(&(seq, _)) = self.usable_keyframes().next_back() else {
            return Vec::new();
        };
        self.packets
            .iter()
            .skip((seq - self.head_seq) as usize)
            .filter(|p| p.is_video())
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests that predate the memory budget: only the length may cut.
    const NO_LIMIT: u64 = u64::MAX;

    fn packet(track: u32, pts_ms: i64, keyframe: bool, size: usize) -> EncodedPacket {
        EncodedPacket {
            track,
            pts_us: pts_ms * 1000,
            duration_us: 16_666,
            keyframe,
            data: vec![0u8; size].into(),
        }
    }

    /// 60 fps, a keyframe every 2 s, plus one audio track.
    fn fill(buf: &mut ReplayBuffer, seconds: i64) {
        for frame in 0..seconds * 60 {
            let pts_ms = frame * 1000 / 60;
            buf.push(packet(VIDEO_TRACK, pts_ms, frame % 120 == 0, 1000));
            if frame % 3 == 0 {
                buf.push(packet(1, pts_ms, false, 100));
            }
        }
    }

    #[test]
    fn trims_to_capacity_but_keeps_a_leading_keyframe() {
        let mut buf = ReplayBuffer::new(10, NO_LIMIT);
        fill(&mut buf, 60);

        let secs = buf.buffered_seconds();
        assert!(
            (10.0..=12.1).contains(&secs),
            "buffer should hold a good 10 s, was {secs}"
        );
        // The first packet has to be a video keyframe.
        let first = buf.packets.front().unwrap();
        assert!(first.is_video() && first.keyframe);
    }

    #[test]
    fn a_recording_starts_on_the_newest_keyframe() {
        let mut buf = ReplayBuffer::new(30, NO_LIMIT);
        assert!(buf.from_last_keyframe().is_empty());

        // Keyframes every 2 s; 5 s in, the newest is at 4 s.
        fill(&mut buf, 5);
        let start = buf.from_last_keyframe();
        assert!(start[0].keyframe);
        assert_eq!(start[0].pts_us, 4_000_000);
        assert!(start.iter().all(|p| p.is_video()), "audio packets slipped in");
        assert_eq!(start.len(), 60, "one second at 60 fps");
    }

    #[test]
    fn snapshot_starts_on_keyframe_and_covers_request() {
        let mut buf = ReplayBuffer::new(30, NO_LIMIT);
        fill(&mut buf, 30);

        let clip = buf.snapshot(10);
        assert!(!clip.is_empty());
        assert!(clip[0].is_video() && clip[0].keyframe);

        let span = (clip.last().unwrap().pts_us - clip[0].pts_us) as f32 / 1e6;
        assert!(span >= 10.0, "clip too short: {span} s");
        assert!(span <= 12.0, "clip needlessly long: {span} s");
    }

    #[test]
    fn shrinking_capacity_frees_memory() {
        let mut buf = ReplayBuffer::new(30, NO_LIMIT);
        fill(&mut buf, 30);
        let before = buf.bytes();
        buf.set_capacity(5, NO_LIMIT);
        assert!(buf.bytes() < before / 2, "buffer was not shrunk");
    }

    /// The point of the budget: with a constant quality the bitrate follows the
    /// picture, so thirty seconds of one scene are not thirty seconds of another.
    /// Memory has to win over duration, or a busy scene takes the machine down.
    #[test]
    fn the_memory_budget_cuts_the_buffer_short() {
        // `fill` writes 1000 bytes per frame plus audio — 30 s is well over this.
        let mut buf = ReplayBuffer::new(30, 400_000);
        fill(&mut buf, 30);

        assert!(
            buf.bytes() <= 400_000,
            "the ring stayed over its budget: {} bytes",
            buf.bytes()
        );
        assert!(
            buf.buffered_seconds() < 30.0,
            "the budget should have cost some length, held {} s",
            buf.buffered_seconds()
        );
        // Still usable, still starting on a keyframe.
        let first = buf.packets.front().unwrap();
        assert!(first.is_video() && first.keyframe);
    }

    /// Even an absurd budget must leave a decodable group of pictures behind.
    /// An empty ring would turn "save clip" into an error message for the rest
    /// of the session.
    #[test]
    fn the_budget_never_empties_the_ring() {
        let mut buf = ReplayBuffer::new(30, 1);
        fill(&mut buf, 30);

        assert!(!buf.is_empty(), "the last group of pictures has to stay");
        let first = buf.packets.front().unwrap();
        assert!(first.is_video() && first.keyframe);
    }

    /// Without a budget nothing about the old behaviour may change.
    #[test]
    fn an_unlimited_budget_leaves_the_length_in_charge() {
        let mut capped = ReplayBuffer::new(10, NO_LIMIT);
        let mut plain = ReplayBuffer::new(10, NO_LIMIT);
        fill(&mut capped, 60);
        fill(&mut plain, 60);
        assert_eq!(capped.bytes(), plain.bytes());
        assert_eq!(capped.len(), plain.len());
    }

    /// The case that used to have no floor at all. Both limits are enforced
    /// between keyframes, so a transform that never flags one left them powerless:
    /// the ring grew past its length and past its budget until the machine gave
    /// out, while the UI showed a buffer that never filled. `mft::opens_a_gop` is
    /// what should stop this arising; this is the floor under it.
    #[test]
    fn a_stream_without_keyframes_still_obeys_the_memory_budget() {
        let mut buf = ReplayBuffer::new(2, 100_000);
        for frame in 0..600i64 {
            buf.push(packet(VIDEO_TRACK, frame * 1000 / 60, false, 1000));
        }
        assert!(
            buf.bytes() <= 100_000,
            "the ring grew to {} bytes with no keyframe to cut at",
            buf.bytes()
        );
        assert!(!buf.is_empty(), "and it must not have thrown everything away");
    }

    #[test]
    fn a_stream_without_keyframes_still_obeys_its_length() {
        let mut buf = ReplayBuffer::new(2, NO_LIMIT);
        for frame in 0..600i64 {
            buf.push(packet(VIDEO_TRACK, frame * 1000 / 60, false, 1000));
        }
        // Two seconds at 60 fps, give or take the packet the cut lands on.
        assert!(
            buf.len() <= 130,
            "the ring held {} packets for a two-second buffer",
            buf.len()
        );
    }

    /// The floor must stay out of the way of the normal case: with keyframes
    /// present the last group of pictures is deliberately kept, however small the
    /// budget, and nothing here may cut into it.
    #[test]
    fn the_floor_does_not_touch_a_ring_that_has_keyframes() {
        let mut buf = ReplayBuffer::new(30, 1);
        fill(&mut buf, 30);

        assert!(!buf.is_empty(), "the last group of pictures has to stay");
        let first = buf.packets.front().unwrap();
        assert!(first.is_video() && first.keyframe);
        assert!(
            buf.bytes() > 1,
            "the budget was allowed to win over the last group of pictures"
        );
    }

    /// Review Focus, BUF-6: an encoder with B-frames hands out pictures
    /// behind the newest one. A recording then cannot keep its timestamps (the
    /// TS carries no DTS), so the ring says so; a clear starts afresh.
    #[test]
    fn a_ring_notices_reordered_frames() {
        let mut buf = ReplayBuffer::new(30, NO_LIMIT);
        for (pts, key) in [(0, true), (16_666, false), (33_333, false)] {
            buf.push(EncodedPacket { pts_us: pts, ..packet(VIDEO_TRACK, 0, key, 10) });
        }
        assert!(!buf.reorders());
        buf.clear();
        for (pts, key) in [(0, true), (50_000, false), (16_666, false)] {
            buf.push(EncodedPacket { pts_us: pts, ..packet(VIDEO_TRACK, 0, key, 10) });
        }
        assert!(buf.reorders());
        buf.clear();
        assert!(!buf.reorders(), "a clear starts afresh");
    }

    #[test]
    fn snapshot_of_empty_buffer_is_empty() {
        let buf = ReplayBuffer::new(30, NO_LIMIT);
        assert!(buf.snapshot(10).is_empty());
    }

    /// `count` video packets at 60 fps from `from_ms` on, keyframes where
    /// `key` says.
    fn video_from(buf: &mut ReplayBuffer, from_ms: i64, count: i64, key: impl Fn(i64) -> bool) {
        for frame in 0..count {
            buf.push(packet(VIDEO_TRACK, from_ms + frame * 1000 / 60, key(frame), 1000));
        }
    }

    /// The cam is cut by the game clip's absolute window, not by "the last N
    /// seconds" of its own ring.
    #[test]
    fn a_cut_from_a_time_starts_on_the_keyframe_before_it() {
        let mut buf = ReplayBuffer::new(30, NO_LIMIT);
        fill(&mut buf, 30);

        let cut = buf.snapshot_range(5_000_000, 9_000_000);
        assert!(cut[0].keyframe && cut[0].pts_us == 4_000_000, "started at {}", cut[0].pts_us);
        assert!(cut.iter().all(|p| (4_000_000..=9_000_000).contains(&p.pts_us)));
    }

    #[test]
    fn a_cut_before_the_ring_starts_on_its_first_keyframe() {
        let mut buf = ReplayBuffer::new(10, NO_LIMIT);
        fill(&mut buf, 30);

        let cut = buf.snapshot_range(0, i64::MAX);
        let front = buf.packets.front().unwrap();
        assert_eq!(cut[0].pts_us, front.pts_us);
        assert!(cut[0].is_video() && cut[0].keyframe);
    }

    #[test]
    fn the_old_cut_is_a_range_up_to_now() {
        let mut buf = ReplayBuffer::new(30, NO_LIMIT);
        fill(&mut buf, 30);

        let old = buf.snapshot(10);
        let range = buf.snapshot_range(buf.newest_pts() - 10_000_000, i64::MAX);
        assert_eq!(old.len(), range.len());
        assert_eq!(old[0].pts_us, range[0].pts_us);
    }

    /// Review Focus, BUF-4: an encoder that flags its first keyframe and none
    /// after. The trim needs a second keyframe, the floor needs none, so with
    /// exactly one the ring used to grow until the allocation failed.
    #[test]
    fn a_stream_with_one_keyframe_still_obeys_its_budget() {
        let mut buf = ReplayBuffer::new(2, 100_000);
        video_from(&mut buf, 0, 601, |frame| frame == 0);
        assert!(buf.bytes() <= 200_000, "the ring grew to {} bytes", buf.bytes());
        assert!(!buf.is_empty(), "and it must not have thrown everything away");
    }

    #[test]
    fn a_young_single_keyframe_is_kept_whatever_the_budget() {
        let mut buf = ReplayBuffer::new(30, 1);
        video_from(&mut buf, 0, 61, |frame| frame == 0);
        let front = buf.packets.front().unwrap();
        assert!(front.keyframe && front.pts_us == 0);
    }

    /// Review Focus, BUF-5: the PC slept with the buffer running. The pts jump
    /// by the sleep, and a clip that reaches back over it comes out silent and
    /// hours long. Everything before the jump goes at the first keyframe after.
    #[test]
    fn a_jump_in_time_drops_what_lies_before_it() {
        let mut buf = ReplayBuffer::new(30, NO_LIMIT);
        buf.set_jump_limit_us(6_000_000);
        fill(&mut buf, 10);
        video_from(&mut buf, 610_000, 180, |frame| frame % 120 == 0);

        assert!(buf.snapshot_range(0, i64::MAX)[0].pts_us >= 610_000_000);
        assert!(buf.buffered_seconds() < 4.0, "held {} s", buf.buffered_seconds());
    }

    #[test]
    fn nothing_from_before_a_jump_is_cut_until_the_next_keyframe() {
        let mut buf = ReplayBuffer::new(30, NO_LIMIT);
        buf.set_jump_limit_us(6_000_000);
        fill(&mut buf, 10);
        video_from(&mut buf, 610_000, 30, |_| false);

        assert!(buf.snapshot_range(0, i64::MAX).is_empty());
        assert_eq!(buf.buffered_seconds(), 0.0);
    }

    #[test]
    fn a_stall_under_the_limit_is_no_jump() {
        let mut buf = ReplayBuffer::new(30, NO_LIMIT);
        buf.set_jump_limit_us(6_000_000);
        fill(&mut buf, 10);
        video_from(&mut buf, 12_000, 120, |frame| frame == 0);

        assert_eq!(buf.snapshot(30)[0].pts_us, 0);
    }

    /// A recording takes the ring's backlog from its newest keyframe. Across
    /// a pending cut that keyframe lies before the jump, so there is nothing
    /// to begin on until the next one (`attach_recorder` waits for it).
    #[test]
    fn a_recording_waits_for_the_keyframe_after_a_jump() {
        let mut buf = ReplayBuffer::new(30, NO_LIMIT);
        buf.set_jump_limit_us(6_000_000);
        fill(&mut buf, 10);
        video_from(&mut buf, 610_000, 30, |_| false);
        assert!(!buf.has_keyframe());
        assert!(buf.from_last_keyframe().is_empty());

        video_from(&mut buf, 610_500, 30, |frame| frame == 0);
        assert!(buf.has_keyframe());
        let start = buf.from_last_keyframe();
        assert!(start[0].keyframe && start[0].pts_us == 610_500_000);
        assert_eq!(start.len(), 30);
    }

    #[test]
    fn a_clear_forgets_a_pending_cut() {
        let mut buf = ReplayBuffer::new(30, NO_LIMIT);
        buf.set_jump_limit_us(6_000_000);
        fill(&mut buf, 10);
        video_from(&mut buf, 610_000, 30, |_| false);
        buf.clear();

        fill(&mut buf, 5);
        assert_eq!(buf.snapshot_range(0, i64::MAX)[0].pts_us, 0);
        assert!(buf.buffered_seconds() > 4.0);
    }

    /// Both faults at once: the jump waits for a keyframe that never comes.
    /// The ring must not grow while it waits.
    #[test]
    fn a_jump_without_a_keyframe_after_it_still_obeys_the_length() {
        let mut buf = ReplayBuffer::new(2, NO_LIMIT);
        buf.set_jump_limit_us(6_000_000);
        video_from(&mut buf, 0, 60, |frame| frame == 0);
        video_from(&mut buf, 610_000, 60 * 30, |_| false);

        assert!(buf.len() <= 130, "the ring held {} packets for a two-second buffer", buf.len());
        assert!(buf.packets.front().unwrap().pts_us >= 610_000_000);
        assert!(buf.snapshot_range(0, i64::MAX).is_empty());
    }
}
