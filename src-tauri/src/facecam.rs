//! The facecam's store and facts: the cam cut to a clip's window, written as
//! `originals/<id>/cam.mp4` with `facecam.json` beside it.


use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::buffer::EncodedPacket;
use crate::camera::ClockKind;
use crate::camtrack::CamRing;
use crate::model::{CamLayout, EncoderId};

/// What is cut on either side of the clip, for the delay slider (±300 ms).
const MARGIN_100NS: i64 = 3_000_000;
/// A step between two cam packets longer than one and a half frames is a hole.
const HOLE_US: i64 = 50_000;

/// The game's window of a clip on the one clock: its first frame and length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CamWindow {
    pub g0_100ns: i64,
    pub length_us: u64,
}

/// The stretch the cam is cut from, QPC in 100 ns.
pub fn cut_window(w: &CamWindow) -> (i64, i64) {
    (w.g0_100ns - MARGIN_100NS, w.g0_100ns + w.length_us as i64 * 10 + MARGIN_100NS)
}

/// The cam packets for one clip, and where they sit against it.
#[derive(Debug, Clone)]
pub struct CamCut {
    pub packets: Vec<EncodedPacket>,
    /// The first cam packet's time minus the game's first frame.
    pub offset_us: i64,
    pub frames: u32,
    /// Stretches without a camera, in milliseconds of the clip.
    pub gaps_ms: Vec<(u64, u64)>,
}

/// The cam for `window`: from the cam keyframe at or before the window's start
/// to its end. `None` when no cam frame lies in the clip, or a gap covers all
/// of it. A cut never spans a hole in the timestamps (a new encoder that took
/// a moment to start): it then begins on the first keyframe after the last one.
pub fn cam_cut(ring: &CamRing, window: &CamWindow) -> Option<CamCut> {
    let (from, to) = cut_window(window);
    let origin = ring.origin_100ns;
    let g0 = window.g0_100ns;
    let end = g0 + window.length_us as i64 * 10;
    let mut packets = ring.ring.snapshot_range((from - origin) / 10, (to - origin) / 10);
    if let Some(after_hole) = packets
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[1].pts_us - pair[0].pts_us > HOLE_US)
        .map(|(i, _)| i + 1)
        .last()
    {
        let restart = packets[after_hole..].iter().position(|p| p.keyframe)?;
        packets.drain(..after_hole + restart);
    }
    let at = |p: &EncodedPacket| origin + p.pts_us * 10;
    if !packets.iter().any(|p| (g0..end).contains(&at(p))) {
        return None;
    }
    if ring.gaps.iter().any(|g| g.from_100ns <= g0 && g.to_100ns.map_or(true, |t| t >= end)) {
        return None;
    }
    let length_ms = window.length_us / 1000;
    let gaps_ms = ring
        .gaps
        .iter()
        .filter(|g| g.from_100ns < end && g.to_100ns.map_or(true, |t| t > g0))
        .map(|g| {
            let from = ((g.from_100ns - g0).max(0) / 10_000) as u64;
            let to = g.to_100ns.map_or(length_ms, |t| (((t - g0) / 10_000).max(0) as u64).min(length_ms));
            (from.min(length_ms), to)
        })
        .collect();
    let offset_us = (at(&packets[0]) - g0) / 10;
    Some(CamCut { frames: packets.len() as u32, packets, offset_us, gaps_ms })
}

/// The delay a clip keeps: the setting, and another 40 ms earlier when the
/// cam ran on the arrival clock, which is late by the camera's own latency.
pub fn saved_delay_ms(config_delay: i32, clock: ClockKind) -> i32 {
    let extra = if clock == ClockKind::Arrival { -40 } else { 0 };
    (config_delay + extra).clamp(-300, 300)
}

/// What the clip file shows of the cam.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FacecamApplied {
    pub layout: CamLayout,
    pub delay_ms: i32,
    pub start_ms: u64,
    pub length_ms: u64,
    pub size_bytes: u64,
}

/// `facecam.json`: the facts that go with `cam.mp4`. Written last, as the sign
/// the store is complete.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FacecamFacts {
    pub version: u32,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub cam_offset_us: i64,
    pub frames: u32,
    pub clip_length_us: u64,
    pub gaps_ms: Vec<(u64, u64)>,
    pub clock: ClockKind,
    pub camera: String,
    pub encoder: Option<EncoderId>,
    /// The game's keyframe interval in frames at the save.
    pub game_gop: u32,
    /// The clean clip's real video start, once it was probed.
    #[serde(default)]
    pub video_start_us: Option<i64>,
    #[serde(default)]
    pub applied: Option<FacecamApplied>,
    /// The intent written just before a swap; `applied` once it is done.
    #[serde(default)]
    pub applying: Option<FacecamApplied>,
    /// Unix milliseconds of the first swap that failed (a player held the file).
    #[serde(default)]
    pub swap_failing_since_ms: Option<i64>,
}

pub fn cam_path(clip_id: &str) -> PathBuf {
    crate::edit::dir(clip_id).join("cam.mp4")
}

pub fn facts_path(clip_id: &str) -> PathBuf {
    crate::edit::dir(clip_id).join("facecam.json")
}

/// Has the clip a facecam store? The facts file says so.
pub fn has_store(clip_id: &str) -> bool {
    facts_path(clip_id).exists()
}

/// The facts, provided `cam.mp4` is there too.
pub fn read_facts(clip_id: &str) -> Option<FacecamFacts> {
    if !cam_path(clip_id).exists() {
        return None;
    }
    let text = std::fs::read_to_string(facts_path(clip_id)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_facts(clip_id: &str, facts: &FacecamFacts) -> std::io::Result<()> {
    std::fs::create_dir_all(crate::edit::dir(clip_id))?;
    let text = serde_json::to_string_pretty(facts)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    let temp = facts_path(clip_id).with_extension("part");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, facts_path(clip_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::EncodedPacket;
    use crate::camera::grid::pts_of;
    use crate::camtrack::CamRing;

    const S: i64 = 10_000_000; // a second in 100 ns

    /// A ring at 30 fps with a keyframe every 30 frames, slots `from..to`.
    fn ring(origin: i64, from: i64, to: i64, skip: Option<i64>) -> CamRing {
        let mut ring = CamRing::new(600, u64::MAX);
        ring.reset(origin);
        let session = ring.session;
        for k in from..to {
            if Some(k) == skip {
                continue;
            }
            ring.push(
                session,
                EncodedPacket {
                    track: 0,
                    pts_us: pts_of(k, 30) / 10,
                    duration_us: 33_333,
                    keyframe: k % 30 == 0,
                    data: vec![k as u8; 10].into(),
                },
            );
        }
        ring
    }

    fn window(g0: i64, length_s: f64) -> CamWindow {
        CamWindow { g0_100ns: g0, length_us: (length_s * 1_000_000.0) as u64 }
    }

    #[test]
    fn the_cut_starts_on_the_keyframe_before_the_delay_range() {
        let ring = ring(0, 0, 30 * 30, None);
        let cut = cam_cut(&ring, &window(105_000_000, 5.0)).unwrap();
        assert!(cut.packets[0].keyframe);
        assert_eq!(cut.packets[0].pts_us, 10_000_000);
        assert!(cut.packets.last().unwrap().pts_us <= 15_800_000);
        assert_eq!(cut.offset_us, -500_000);
        assert_eq!(cut.frames as usize, cut.packets.len());
    }

    #[test]
    fn the_offset_is_measured_against_the_games_first_frame() {
        let origin = 5_000_000_000;
        let ring = ring(origin, 0, 30 * 30, None);
        let cut = cam_cut(&ring, &window(origin + 105_000_000, 5.0)).unwrap();
        assert_eq!(cut.offset_us, -500_000);
    }

    #[test]
    fn a_cam_that_started_after_the_clip_gives_a_positive_offset() {
        let ring = ring(12 * S, 0, 30 * 30, None);
        let cut = cam_cut(&ring, &window(105_000_000, 5.0)).unwrap();
        assert_eq!(cut.offset_us, 1_500_000);
    }

    #[test]
    fn no_cam_frame_in_the_window_means_no_facecam() {
        let ended = ring(0, 0, 9 * 30, None);
        assert!(cam_cut(&ended, &window(10 * S, 5.0)).is_none());
        let late = ring(16 * S, 0, 300, None);
        assert!(cam_cut(&late, &window(10 * S, 5.0)).is_none());
    }

    #[test]
    fn a_window_covered_by_a_gap_has_no_facecam() {
        let mut ring = ring(0, 0, 30 * 30, None);
        ring.open_gap(9 * S);
        ring.close_gap(16 * S);
        assert!(cam_cut(&ring, &window(10 * S, 5.0)).is_none());
    }

    #[test]
    fn a_gap_is_carried_in_clip_coordinates() {
        let mut ring = ring(0, 0, 30 * 30, None);
        ring.open_gap(12 * S);
        ring.close_gap(135_000_000);
        let cut = cam_cut(&ring, &window(105_000_000, 5.0)).unwrap();
        assert_eq!(cut.gaps_ms, vec![(1500, 3000)]);

        let mut ring = ring_with_gap_from(9 * S);
        ring.close_gap(11 * S);
        let cut = cam_cut(&ring, &window(105_000_000, 5.0)).unwrap();
        assert_eq!(cut.gaps_ms, vec![(0, 500)]);

        let ring = ring_with_gap_from(14 * S);
        let cut = cam_cut(&ring, &window(105_000_000, 5.0)).unwrap();
        assert_eq!(cut.gaps_ms, vec![(3500, 5000)]);
    }

    fn ring_with_gap_from(from: i64) -> CamRing {
        let mut r = ring(0, 0, 30 * 30, None);
        r.open_gap(from);
        r
    }

    #[test]
    fn a_cut_never_spans_a_hole_in_the_pts() {
        let ring = ring(0, 0, 30 * 30, Some(150));
        let cut = cam_cut(&ring, &window(4 * S, 4.0)).unwrap();
        assert_eq!(cut.packets[0].pts_us, 6_000_000);
        assert_eq!(cut.offset_us, 2_000_000);
    }

    #[test]
    fn steps_of_33333_and_33334_us_are_no_hole() {
        let ring = ring(0, 0, 30 * 30, None);
        let cut = cam_cut(&ring, &window(4 * S, 4.0)).unwrap();
        assert_eq!(cut.packets[0].pts_us, 3_000_000);
    }

    #[test]
    fn the_arrival_clock_shifts_the_cam_another_40_ms_earlier() {
        use crate::camera::ClockKind;
        assert_eq!(saved_delay_ms(-40, ClockKind::Device), -40);
        assert_eq!(saved_delay_ms(-40, ClockKind::Arrival), -80);
        assert_eq!(saved_delay_ms(-300, ClockKind::Arrival), -300);
    }

    fn facts() -> FacecamFacts {
        FacecamFacts {
            version: 1,
            width: 1280,
            height: 720,
            fps: 30,
            cam_offset_us: -412_000,
            frames: 912,
            clip_length_us: 30_000_000,
            gaps_ms: vec![(1500, 3000)],
            clock: crate::camera::ClockKind::Device,
            camera: "HD Pro Webcam C920".into(),
            encoder: Some(crate::model::EncoderId::Nvenc),
            game_gop: 120,
            video_start_us: None,
            applied: None,
            applying: None,
            swap_failing_since_ms: None,
        }
    }

    #[test]
    fn facts_survive_the_roundtrip() {
        let id = "clippiboy-test-facecam-roundtrip";
        let _ = std::fs::remove_dir_all(crate::edit::dir(id));
        std::fs::create_dir_all(crate::edit::dir(id)).unwrap();
        std::fs::write(cam_path(id), b"cam").unwrap();
        write_facts(id, &facts()).unwrap();
        assert!(has_store(id));
        assert_eq!(read_facts(id), Some(facts()));
        let _ = std::fs::remove_dir_all(crate::edit::dir(id));
    }

    #[test]
    fn a_store_without_cam_mp4_does_not_count() {
        let id = "clippiboy-test-facecam-no-cam";
        let _ = std::fs::remove_dir_all(crate::edit::dir(id));
        write_facts(id, &facts()).unwrap();
        assert!(has_store(id));
        assert_eq!(read_facts(id), None);
        let _ = std::fs::remove_dir_all(crate::edit::dir(id));
    }
}
