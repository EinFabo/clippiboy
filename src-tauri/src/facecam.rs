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

/// ffmpeg's arguments for `cam.mp4`: the elementary stream at exactly 30
/// fps — the grid made it constant — copied into an MP4.
pub fn cam_mp4_arguments(h264: &std::path::Path, output: &std::path::Path) -> Vec<String> {
    [
        "-y", "-hide_banner", "-loglevel", "error", "-f", "h264", "-r", "30", "-i",
    ]
    .into_iter()
    .map(String::from)
    .chain([h264.to_string_lossy().into_owned()])
    .chain(["-c:v", "copy", "-an", "-movflags", "+faststart"].into_iter().map(String::from))
    .chain([output.to_string_lossy().into_owned()])
    .collect()
}

/// The cam cut for one clip, with what the facts need to say about it.
pub struct CamCutOut {
    pub cut: CamCut,
    /// The cam encoder's SPS/PPS.
    pub header: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub clock: ClockKind,
    pub camera: String,
    pub encoder: Option<EncoderId>,
}

/// Writes `cam.mp4` and then `facecam.json` into the clip's store. Any
/// failure removes what was started and costs the cam, never the clip.
pub fn write_phase_one(clip_id: &str, out: &CamCutOut, clip_length_us: u64, game_gop: u32) -> Result<FacecamFacts, String> {
    let dir = crate::edit::dir(clip_id);
    let h264 = dir.join("cam.part.h264");
    let part = dir.join("cam.part.mp4");
    let result = (|| {
        std::fs::create_dir_all(&dir).map_err(|err| format!("cam store: {err}"))?;
        let mut stream = out.header.clone();
        for packet in &out.cut.packets {
            stream.extend_from_slice(&packet.data);
        }
        std::fs::write(&h264, &stream).map_err(|err| format!("cam stream: {err}"))?;
        let run = crate::muxer::command("ffmpeg")
            .args(cam_mp4_arguments(&h264, &part))
            .output()
            .map_err(|err| format!("ffmpeg: {err}"))?;
        if !run.status.success() {
            return Err(format!("ffmpeg: {}", String::from_utf8_lossy(&run.stderr).trim()));
        }
        crate::muxer::probe_duration_ms(&part).ok_or("cam.mp4 cannot be read back")?;
        std::fs::rename(&part, cam_path(clip_id)).map_err(|err| format!("cam.mp4: {err}"))?;
        let facts = FacecamFacts {
            version: 1,
            width: out.width,
            height: out.height,
            fps: out.fps,
            cam_offset_us: out.cut.offset_us,
            frames: out.cut.frames,
            clip_length_us,
            gaps_ms: out.cut.gaps_ms.clone(),
            clock: out.clock,
            camera: out.camera.clone(),
            encoder: out.encoder,
            game_gop,
            video_start_us: None,
            applied: None,
            applying: None,
            swap_failing_since_ms: None,
        };
        write_facts(clip_id, &facts).map_err(|err| format!("facecam.json: {err}"))?;
        Ok(facts)
    })();
    let _ = std::fs::remove_file(&h264);
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
        let _ = std::fs::remove_file(cam_path(clip_id));
        let _ = std::fs::remove_file(facts_path(clip_id));
    }
    result
}

/// What the player needs to show a clip's camera movable: the clean picture,
/// the cam, and where each sits against the clip file's own time
/// (element time = clip time + lead). Asked for on demand when a clip opens —
/// it reads the store, which a list of every clip must not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FacecamView {
    pub clean_path: String,
    pub cam_path: String,
    pub cam_width: u32,
    pub cam_height: u32,
    pub clean_lead_ms: i64,
    pub cam_lead_ms: i64,
    pub gaps_ms: Vec<(u64, u64)>,
    /// The clip file has the cam in it.
    pub shows_cam: bool,
}

/// Does the clip file show the cam? Its facts say what went in; the row says
/// only what is wanted.
pub fn file_shows_cam(clip_id: &str) -> bool {
    read_facts(clip_id).is_some_and(|facts| shows(&facts))
}

fn shows(facts: &FacecamFacts) -> bool {
    facts.applied.as_ref().is_some_and(|a| a.layout.visible)
}

/// The view of `clip`, from its facts and whether a clean copy is stored.
/// `None` without a cam, without a store, once baked, or when the file has
/// the cam in it and no clean copy is left to show.
pub fn view_from(clip: &crate::model::Clip, facts: Option<&FacecamFacts>, clean_stored: bool) -> Option<FacecamView> {
    let cam = clip.facecam.as_ref()?;
    if cam.state == crate::model::FacecamState::Baked {
        return None;
    }
    let facts = facts?;
    let shows_cam = shows(facts);
    let start_ms = clip.original.map(|o| o.start_ms).unwrap_or(0);
    let (clean_path, clean_lead_ms) = if clean_stored {
        (crate::edit::video_path(&clip.id).to_string_lossy().into_owned(), start_ms as i64)
    } else if !shows_cam {
        (clip.path.clone(), 0)
    } else {
        return None;
    };
    // The inverse of `composite::cam_timing`: the composite shows cam time c
    // at clip time c - seek + shift, so c = t + e - video_start.
    let d = facts.cam_offset_us + cam.delay_ms as i64 * 1000;
    let e = start_ms as i64 * 1000 - d;
    let cam_lead_ms = ((e - facts.video_start_us.unwrap_or(0)) as f64 / 1000.0).round() as i64;
    Some(FacecamView {
        clean_path,
        cam_path: cam_path(&clip.id).to_string_lossy().into_owned(),
        cam_width: facts.width,
        cam_height: facts.height,
        clean_lead_ms,
        cam_lead_ms,
        gaps_ms: facts.gaps_ms.clone(),
        shows_cam,
    })
}

/// [`view_from`] off the disk.
pub fn view_of(clip: &crate::model::Clip) -> Option<FacecamView> {
    clip.facecam.as_ref()?;
    let facts = read_facts(&clip.id)?;
    view_from(clip, Some(&facts), crate::edit::video_path(&clip.id).is_file())
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

    /// The grid's stream is exact constant frame rate, so a raw elementary
    /// stream read at 30 fps is right; copied, never re-encoded or cut.
    #[test]
    fn the_cam_file_is_copied_at_exactly_thirty_frames() {
        let args = cam_mp4_arguments(std::path::Path::new("cam.part.h264"), std::path::Path::new("cam.part.mp4"));
        let input = args.iter().position(|a| a == "-i").unwrap();
        assert_eq!(&args[input - 4..input], ["-f", "h264", "-r", "30"]);
        assert!(args.windows(2).any(|w| w == ["-c:v", "copy"]));
        assert!(!args.iter().any(|a| a == "-filter_complex" || a == "-ss"));
        assert_eq!(args.last().map(String::as_str), Some("cam.part.mp4"));
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

    fn facecam_clip(state: crate::model::FacecamState, start_ms: Option<u64>) -> crate::model::Clip {
        let mut clip = crate::edit::tests::plain_clip(None, None);
        clip.facecam = Some(crate::model::ClipFacecam {
            layout: crate::model::CamLayout::default(),
            portrait: Default::default(),
            delay_ms: -40,
            state,
        });
        clip.original = start_ms.map(|start| crate::model::ClipOriginal { duration_ms: 60_000, start_ms: start, end_ms: start + 30_000 });
        clip
    }

    fn shown(facts: FacecamFacts) -> FacecamFacts {
        let layout = crate::model::CamLayout::default();
        FacecamFacts {
            applied: Some(FacecamApplied { layout, delay_ms: -40, start_ms: 0, length_ms: 30_000, size_bytes: 1 }),
            ..facts
        }
    }

    #[test]
    fn the_view_takes_its_leads_from_cam_timing() {
        let facts = FacecamFacts { cam_offset_us: 120_000, video_start_us: Some(21_333), ..shown(facts()) };
        let clip = facecam_clip(crate::model::FacecamState::Ready, Some(5_000));
        let view = view_from(&clip, Some(&facts), true).expect("a view");
        let timing = crate::composite::cam_timing(120_000, -40, 21_333, 5_000, 30_000);
        for t in [0.0, 3.0] {
            // The composite shows cam time c at clip time c - seek + shift.
            let c = t + timing.seek_s.unwrap_or(0.0) - timing.shift_s;
            let ours = t + view.cam_lead_ms as f64 / 1000.0;
            assert!((ours - c).abs() < 0.001, "t={t}: {ours} vs {c}");
        }
        assert_eq!(view.clean_lead_ms, 5_000, "the stored clean picture is the whole recording");
        assert!(view.shows_cam);
    }

    #[test]
    fn a_clip_without_facecam_reads_as_none() {
        let mut clip = facecam_clip(crate::model::FacecamState::Ready, None);
        clip.facecam = None;
        assert_eq!(view_from(&clip, Some(&shown(facts())), true), None);
    }

    #[test]
    fn the_view_is_none_once_the_store_is_gone() {
        let clip = facecam_clip(crate::model::FacecamState::Ready, None);
        assert_eq!(view_from(&clip, None, true), None);
    }

    #[test]
    fn a_baked_clip_has_no_view() {
        let clip = facecam_clip(crate::model::FacecamState::Baked, None);
        assert_eq!(view_from(&clip, Some(&shown(facts())), true), None);
    }

    #[test]
    fn a_clip_saved_with_the_cam_hidden_plays_its_own_file_as_the_clean_picture() {
        let clip = facecam_clip(crate::model::FacecamState::Ready, None);
        let view = view_from(&clip, Some(&facts()), false).expect("a view");
        assert_eq!(view.clean_path, clip.path);
        assert_eq!(view.clean_lead_ms, 0);
        assert!(!view.shows_cam);
        // A file with the cam in it and no clean copy: nothing clean to show.
        assert_eq!(view_from(&clip, Some(&shown(facts())), false), None);
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
