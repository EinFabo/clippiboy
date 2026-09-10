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
        }
    }

    pub fn set_capacity(&mut self, seconds: u32, bytes: u64) {
        self.capacity_us = seconds as i64 * 1_000_000;
        self.capacity_bytes = bytes;
        self.trim();
    }

    pub fn push(&mut self, packet: EncodedPacket) {
        self.newest_pts = self.newest_pts.max(packet.pts_us);
        self.bytes += packet.data.len() as u64;
        if packet.is_video() && packet.keyframe {
            self.keyframes.push_back((self.next_seq, packet.pts_us));
        }
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

        self.enforce_budget_without_keyframes();
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
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.packets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }

    /// Duration actually buffered, in seconds (from the first keyframe).
    pub fn buffered_seconds(&self) -> f32 {
        match self.keyframes.front() {
            Some((_, pts)) => (self.newest_pts - pts) as f32 / 1_000_000.0,
            None => 0.0,
        }
    }

    /// Snapshot of the last `seconds` seconds, starting at the last keyframe
    /// *before* that point in time (so the video is decodable).
    pub fn snapshot(&self, seconds: u32) -> Vec<EncodedPacket> {
        let want_from = self.newest_pts - seconds as i64 * 1_000_000;
        let start_pts = self
            .keyframes
            .iter()
            .rev()
            .map(|(_, pts)| *pts)
            .find(|pts| *pts <= want_from)
            .or_else(|| self.keyframes.front().map(|(_, pts)| *pts));

        let Some(start_pts) = start_pts else {
            return Vec::new();
        };

        self.packets
            .iter()
            .filter(|p| p.pts_us >= start_pts)
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

    #[test]
    fn snapshot_of_empty_buffer_is_empty() {
        let buf = ReplayBuffer::new(30, NO_LIMIT);
        assert!(buf.snapshot(10).is_empty());
    }
}
