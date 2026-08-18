//! Ring-Puffer über *bereits encodete* Pakete.
//!
//! Rohframes zu puffern wäre nicht bezahlbar (5 min 1080p60 BGRA ≈ 90 GB),
//! encodet sind es bei 40 Mbit/s ≈ 1,5 GB. Das Speichern eines Clips ist damit
//! reines Muxen ohne Re-Encode.
//!
//! Wichtig: Der Puffer darf vorne nur bis zu einem Video-Keyframe beschnitten
//! werden, sonst ist der Anfang des Clips nicht dekodierbar.

use std::collections::VecDeque;
use std::sync::Arc;

/// Spur 0 ist immer Video, ab 1 folgen die Audiospuren.
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
    packets: VecDeque<EncodedPacket>,
    /// Laufende Nummer des vordersten Pakets — für stabile Keyframe-Positionen.
    head_seq: u64,
    next_seq: u64,
    /// (seq, pts) jedes Video-Keyframes, der noch im Puffer liegt.
    keyframes: VecDeque<(u64, i64)>,
    bytes: u64,
    newest_pts: i64,
}

impl ReplayBuffer {
    pub fn new(capacity_seconds: u32) -> Self {
        Self {
            capacity_us: capacity_seconds as i64 * 1_000_000,
            packets: VecDeque::new(),
            head_seq: 0,
            next_seq: 0,
            keyframes: VecDeque::new(),
            bytes: 0,
            newest_pts: 0,
        }
    }

    pub fn set_capacity(&mut self, seconds: u32) {
        self.capacity_us = seconds as i64 * 1_000_000;
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

    /// Verwirft von vorne, solange auch nach dem Verwerfen noch die volle
    /// Pufferlänge ab dem *nächsten* Keyframe vorhanden bleibt.
    fn trim(&mut self) {
        while self.keyframes.len() > 1 {
            let (seq, pts) = self.keyframes[1];
            if self.newest_pts - pts < self.capacity_us {
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

    /// Tatsächlich gepufferte Dauer in Sekunden (ab erstem Keyframe).
    pub fn buffered_seconds(&self) -> f32 {
        match self.keyframes.front() {
            Some((_, pts)) => (self.newest_pts - pts) as f32 / 1_000_000.0,
            None => 0.0,
        }
    }

    /// Schnappschuss der letzten `seconds` Sekunden, beginnend beim letzten
    /// Keyframe *vor* dem Startzeitpunkt (damit das Video dekodierbar ist).
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

    fn packet(track: u32, pts_ms: i64, keyframe: bool, size: usize) -> EncodedPacket {
        EncodedPacket {
            track,
            pts_us: pts_ms * 1000,
            duration_us: 16_666,
            keyframe,
            data: vec![0u8; size].into(),
        }
    }

    /// 60 fps, Keyframe alle 2 s, dazu eine Audiospur.
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
        let mut buf = ReplayBuffer::new(10);
        fill(&mut buf, 60);

        let secs = buf.buffered_seconds();
        assert!(
            (10.0..=12.1).contains(&secs),
            "Puffer sollte gut 10 s halten, war {secs}"
        );
        // Erstes Paket muss ein Video-Keyframe sein.
        let first = buf.packets.front().unwrap();
        assert!(first.is_video() && first.keyframe);
    }

    #[test]
    fn snapshot_starts_on_keyframe_and_covers_request() {
        let mut buf = ReplayBuffer::new(30);
        fill(&mut buf, 30);

        let clip = buf.snapshot(10);
        assert!(!clip.is_empty());
        assert!(clip[0].is_video() && clip[0].keyframe);

        let span = (clip.last().unwrap().pts_us - clip[0].pts_us) as f32 / 1e6;
        assert!(span >= 10.0, "Clip zu kurz: {span} s");
        assert!(span <= 12.0, "Clip unnötig lang: {span} s");
    }

    #[test]
    fn shrinking_capacity_frees_memory() {
        let mut buf = ReplayBuffer::new(30);
        fill(&mut buf, 30);
        let before = buf.bytes();
        buf.set_capacity(5);
        assert!(buf.bytes() < before / 2, "Puffer wurde nicht verkleinert");
    }

    #[test]
    fn snapshot_of_empty_buffer_is_empty() {
        let buf = ReplayBuffer::new(30);
        assert!(buf.snapshot(10).is_empty());
    }
}
