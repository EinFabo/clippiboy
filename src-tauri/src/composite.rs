//! Laying the cam over a clip: Rust turns the layout and the timing into
//! literals, and the ffmpeg line holds nothing else.


use std::path::Path;

use crate::camera::Rect;
use crate::facecam::FacecamFacts;
use crate::model::{CamLayout, CamShape, EncoderId};

fn even(v: f64) -> u32 {
    (2.0 * (v / 2.0).round()).max(0.0) as u32
}

/// The space kept free around the cam: 2.5 % of the frame's height.
pub fn margin(frame_height: u32) -> u32 {
    even(0.025 * frame_height as f64)
}

/// Where the cam lands in a frame: its height a share of the frame's, 16:9
/// (a circle square), inside the margin, every number even. On a screen too
/// narrow for it (portrait) the cam narrows instead of running off.
pub fn cam_rect(frame: (u32, u32), layout: &CamLayout) -> Rect {
    let (fw, fh) = (frame.0 as f64, frame.1 as f64);
    let circle = layout.shape == CamShape::Circle;
    let m = margin(frame.1);
    let mut h = even(layout.size as f64 * fh).max(2);
    let mut w = if circle { h } else { even(h as f64 * 16.0 / 9.0) };
    let room = frame.0.saturating_sub(2 * m);
    if w > room {
        w = room & !1;
        h = if circle { w } else { even(w as f64 * 9.0 / 16.0) };
    }
    let free_x = (fw - w as f64 - 2.0 * m as f64).max(0.0);
    let free_y = (fh - h as f64 - 2.0 * m as f64).max(0.0);
    Rect {
        x: m + even(layout.x.clamp(0.0, 1.0) as f64 * free_x).min(free_x as u32 & !1),
        y: m + even(layout.y.clamp(0.0, 1.0) as f64 * free_y).min(free_y as u32 & !1),
        w,
        h,
    }
}

/// The centred square of the cam picture for a circle.
pub fn cam_crop(cam: (u32, u32), shape: CamShape) -> Option<Rect> {
    if shape != CamShape::Circle {
        return None;
    }
    let side = cam.0.min(cam.1) & !1;
    Some(Rect { x: ((cam.0 - side) / 2) & !1, y: ((cam.1 - side) / 2) & !1, w: side, h: side })
}

/// Where the cam starts against the clip, in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CamTiming {
    /// Seek into `cam.mp4` when it starts before the clip.
    pub seek_s: Option<f64>,
    /// Added to the cam's timestamps.
    pub shift_s: f64,
    /// The cam ends with the clip.
    pub end_s: f64,
}

/// `d` is where the cam starts against the game (its offset plus the delay),
/// `e` how far into the cam the clip's `start` lies. The clean clip's video
/// starts at `video_start_us`, not 0 (SAVE-7), and the cam follows it.
pub fn cam_timing(offset_us: i64, delay_ms: i32, video_start_us: i64, start_ms: u64, length_ms: u64) -> CamTiming {
    let d = offset_us + delay_ms as i64 * 1000;
    let e = start_ms as i64 * 1000 - d;
    let start_s = video_start_us as f64 / 1e6;
    let (seek_s, shift_s) = if e > 0 { (Some(e as f64 / 1e6), start_s) } else { (None, start_s - e as f64 / 1e6) };
    CamTiming { seek_s, shift_s, end_s: start_s + length_ms as f64 / 1000.0 }
}

/// An 8-bit alpha mask for the shape, anti-aliased; `None` for a rectangle.
/// Rounded corners have a radius of 12 % of the height.
pub fn mask_gray8(shape: CamShape, w: u32, h: u32) -> Option<Vec<u8>> {
    let (wf, hf) = (w as f64, h as f64);
    let alpha = |inside: f64| (inside.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut out = Vec::with_capacity((w * h) as usize);
    match shape {
        CamShape::Rect => return None,
        CamShape::Circle => {
            let r = wf.min(hf) / 2.0;
            for y in 0..h {
                for x in 0..w {
                    let (dx, dy) = (x as f64 + 0.5 - wf / 2.0, y as f64 + 0.5 - hf / 2.0);
                    out.push(alpha(r - (dx * dx + dy * dy).sqrt() + 0.5));
                }
            }
        }
        CamShape::Rounded => {
            let r = 0.12 * hf;
            for y in 0..h {
                for x in 0..w {
                    let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                    let dx = (r - px).max(px - (wf - r)).max(0.0);
                    let dy = (r - py).max(py - (hf - r)).max(0.0);
                    out.push(alpha(r - (dx * dx + dy * dy).sqrt() + 0.5));
                }
            }
        }
    }
    Some(out)
}

pub fn write_mask_png(path: &Path, w: u32, h: u32, pixels: &[u8]) -> std::io::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, w, h);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
    writer.write_image_data(pixels).map_err(std::io::Error::other)
}

/// What the cam's part of a filter graph is made from.
pub struct CamChain<'a> {
    pub cam_input: usize,
    pub mask_input: Option<usize>,
    /// The picture the cam goes over, as a pad label ("0:v").
    pub main: &'a str,
    pub frame: (u32, u32),
    pub cam: (u32, u32),
    pub layout: CamLayout,
    pub timing: CamTiming,
    pub gaps_ms: &'a [(u64, u64)],
    pub video_start_us: i64,
}

/// The video graph, ending in `[v]`. Only numbers Rust worked out go in.
pub fn cam_filter(chain: &CamChain) -> String {
    let rect = cam_rect(chain.frame, &chain.layout);
    let mut cam = format!(
        "[{}:v]setpts=PTS+{:.6}/TB,trim=end={:.6}",
        chain.cam_input, chain.timing.shift_s, chain.timing.end_s
    );
    if let Some(crop) = cam_crop(chain.cam, chain.layout.shape) {
        cam += &format!(",crop={}:{}:{}:{}", crop.w, crop.h, crop.x, crop.y);
    }
    if chain.layout.mirror {
        cam += ",hflip";
    }
    cam += &format!(",scale={}:{},format=yuva420p[cam]", rect.w, rect.h);
    let mut graph = cam;
    let mut overlaid = "cam";
    if let Some(mask) = chain.mask_input {
        graph += &format!(";[{mask}:v]format=gray[mask];[cam][mask]alphamerge[camm]");
        overlaid = "camm";
    }
    let start = chain.video_start_us as f64 / 1e6;
    let enable = if chain.gaps_ms.is_empty() {
        String::new()
    } else {
        let hidden: Vec<String> = chain
            .gaps_ms
            .iter()
            .map(|(a, b)| format!("between(t,{:.6},{:.6})", start + *a as f64 / 1000.0, start + *b as f64 / 1000.0))
            .collect();
        format!(":enable='not({})'", hidden.join("+"))
    };
    graph += &format!(
        ";[{}][{overlaid}]overlay=x={}:y={}:eval=init:eof_action=repeat{enable}[v]",
        chain.main, rect.x, rect.y
    );
    graph
}

/// Everything one background composite needs.
pub struct CompositeJob<'a> {
    pub clip: &'a Path,
    pub cam: &'a Path,
    pub mask: Option<&'a Path>,
    pub output: &'a Path,
    pub frame: (u32, u32),
    pub facts: &'a FacecamFacts,
    pub layout: CamLayout,
    pub delay_ms: i32,
    pub video_start_us: i64,
    pub start_ms: u64,
    pub length_ms: u64,
    pub gaps_ms: &'a [(u64, u64)],
    pub encoder: EncoderId,
    pub quality: u32,
    pub gop: u32,
    pub hwaccel: bool,
}

pub fn composite_arguments(job: &CompositeJob) -> Vec<String> {
    let timing = cam_timing(job.facts.cam_offset_us, job.delay_ms, job.video_start_us, job.start_ms, job.length_ms);
    let mut args: Vec<String> =
        ["-y", "-hide_banner", "-loglevel", "error", "-progress", "pipe:1", "-nostats"].into_iter().map(String::from).collect();
    if job.hwaccel {
        args.extend(["-hwaccel".into(), "d3d11va".into()]);
    }
    args.extend(["-threads".into(), "4".into(), "-i".into(), job.clip.to_string_lossy().into_owned()]);
    if let Some(seek) = timing.seek_s {
        args.extend(["-ss".into(), format!("{seek:.6}")]);
    }
    args.extend(["-i".into(), job.cam.to_string_lossy().into_owned()]);
    if let Some(mask) = job.mask {
        // One picture, looped: alphamerge needs it for every cam frame.
        args.extend(["-loop".into(), "1".into(), "-i".into(), mask.to_string_lossy().into_owned()]);
    }
    let graph = cam_filter(&CamChain {
        cam_input: 1,
        mask_input: job.mask.map(|_| 2),
        main: "0:v",
        frame: job.frame,
        cam: (job.facts.width, job.facts.height),
        layout: job.layout,
        timing,
        gaps_ms: job.gaps_ms,
        video_start_us: job.video_start_us,
    });
    args.extend(["-filter_complex_threads".into(), "2".into(), "-filter_complex".into(), graph]);
    args.extend(["-map", "[v]", "-map", "0:a?", "-c:a", "copy"].into_iter().map(String::from));
    args.extend(crate::edit::video_args_at(job.encoder, job.quality, crate::edit::Speed::Save));
    args.extend([
        "-g".into(),
        job.gop.max(1).to_string(),
        "-avoid_negative_ts".into(),
        "make_zero".into(),
        "-t".into(),
        format!("{:.3}", job.length_ms as f64 / 1000.0),
        job.output.to_string_lossy().into_owned(),
    ]);
    args
}

/// Hardware first, then x264 with hardware decoding, then x264 alone.
pub fn attempts(encoder: EncoderId) -> Vec<(EncoderId, bool)> {
    let mut out = Vec::new();
    for attempt in [(encoder, true), (EncoderId::X264, true), (EncoderId::X264, false)] {
        if !out.contains(&attempt) {
            out.push(attempt);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Rect;
    use crate::model::{CamLayout, CamShape, EncoderId};
    use std::path::Path;

    fn layout(x: f32, y: f32, size: f32, shape: CamShape, mirror: bool) -> CamLayout {
        CamLayout { visible: true, x, y, size, shape, mirror }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn the_default_cam_sits_bottom_right_inside_the_margin() {
        assert_eq!(cam_rect((1920, 1080), &CamLayout::default()), Rect { x: 1412, y: 782, w: 480, h: 270 });
        let top_left = cam_rect((1920, 1080), &layout(0.0, 0.0, 0.25, CamShape::Rounded, false));
        assert_eq!((top_left.x, top_left.y), (28, 28));
    }

    #[test]
    fn a_circle_is_cropped_square() {
        let r = cam_rect((1920, 1080), &layout(1.0, 1.0, 0.25, CamShape::Circle, false));
        assert_eq!((r.w, r.h, r.x), (270, 270, 1622));
        assert_eq!(cam_crop((1280, 720), CamShape::Circle), Some(Rect { x: 280, y: 0, w: 720, h: 720 }));
        assert_eq!(cam_crop((1280, 720), CamShape::Rounded), None);
    }

    #[test]
    fn every_number_is_even_and_inside_the_frame() {
        for frame in [(1366, 768), (2560, 1080), (3440, 1440), (1080, 1920), (1920, 1080)] {
            for size in [0.12, 0.5] {
                for (x, y) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                    for shape in [CamShape::Rect, CamShape::Rounded, CamShape::Circle] {
                        let r = cam_rect(frame, &layout(x, y, size, shape, false));
                        let m = margin(frame.1);
                        assert!([r.x, r.y, r.w, r.h].iter().all(|v| v % 2 == 0), "{frame:?} {r:?}");
                        assert!(r.x + r.w + m <= frame.0 && r.y + r.h + m <= frame.1, "{frame:?} {size} {r:?}");
                        assert!(r.x >= m && r.y >= m);
                    }
                }
            }
        }
    }

    #[test]
    fn a_portrait_screen_narrows_the_cam_instead_of_running_off_it() {
        assert_eq!(
            cam_rect((1080, 1920), &layout(1.0, 1.0, 0.5, CamShape::Rounded, false)),
            Rect { x: 48, y: 1318, w: 984, h: 554 }
        );
    }

    #[test]
    fn the_cam_gets_a_seek_exactly_when_it_starts_before_the_clip() {
        let t = cam_timing(-412_000, 0, 21_333, 0, 30_000);
        assert!(close(t.seek_s.unwrap(), 0.412));
        assert!(close(t.shift_s, 0.021333));
        let t = cam_timing(1_500_000, 0, 21_333, 0, 30_000);
        assert_eq!(t.seek_s, None);
        assert!(close(t.shift_s, 1.521333));
    }

    #[test]
    fn the_video_start_is_read_off_ffprobe() {
        assert_eq!(crate::muxer::parse_start_us("0.021333\n"), Some(21_333));
        assert_eq!(crate::muxer::parse_start_us("N/A"), None);
    }

    #[test]
    fn the_delay_moves_the_cam() {
        assert!(close(cam_timing(-412_000, -40, 21_333, 0, 30_000).seek_s.unwrap(), 0.452));
    }

    #[test]
    fn a_trimmed_clip_starts_the_cam_at_its_place_in_the_original() {
        assert!(close(cam_timing(-412_000, 0, 21_333, 4000, 8000).seek_s.unwrap(), 4.412));
    }

    fn facts() -> crate::facecam::FacecamFacts {
        crate::facecam::FacecamFacts {
            version: 1,
            width: 1280,
            height: 720,
            fps: 30,
            cam_offset_us: -412_000,
            frames: 912,
            clip_length_us: 30_000_000,
            gaps_ms: vec![],
            clock: crate::camera::ClockKind::Device,
            camera: "C920".into(),
            encoder: Some(EncoderId::Nvenc),
            game_gop: 120,
            video_start_us: None,
            applied: None,
            applying: None,
            swap_failing_since_ms: None,
        }
    }

    fn args_for(cam: CamLayout, mask: Option<&Path>, gaps: &[(u64, u64)], hwaccel: bool) -> Vec<String> {
        let facts = facts();
        composite_arguments(&CompositeJob {
            clip: Path::new("clip.mp4"),
            cam: Path::new("cam.mp4"),
            mask,
            output: Path::new("out.mp4"),
            frame: (1920, 1080),
            facts: &facts,
            layout: cam,
            delay_ms: 0,
            video_start_us: 21_333,
            start_ms: 0,
            length_ms: 30_000,
            gaps_ms: gaps,
            encoder: EncoderId::Nvenc,
            quality: 70,
            gop: 120,
            hwaccel,
        })
    }

    fn graph(args: &[String]) -> String {
        let i = args.iter().position(|a| a == "-filter_complex").unwrap();
        args[i + 1].clone()
    }

    #[test]
    fn the_cam_is_bound_to_the_clip_length() {
        let args = args_for(CamLayout::default(), None, &[], false);
        assert!(graph(&args).contains("trim=end=30.021333"));
        assert!(graph(&args).contains("setpts=PTS+0.021333/TB"));
        assert!(!graph(&args).contains("STARTPTS"));
        let t = args.iter().position(|a| a == "-t").unwrap();
        assert_eq!(args[t + 1], "30.000");
        assert!(t > args.iter().rposition(|a| a == "-i").unwrap());
    }

    #[test]
    fn a_gap_hides_the_cam() {
        let args = args_for(CamLayout::default(), None, &[(1500, 3000)], false);
        assert!(graph(&args).contains("enable='not(between(t,1.521333,3.021333))'"));
        assert!(!graph(&args_for(CamLayout::default(), None, &[], false)).contains("enable="));
    }

    #[test]
    fn a_mirrored_circle_is_cropped_flipped_and_masked() {
        let args = args_for(layout(1.0, 1.0, 0.25, CamShape::Circle, true), Some(Path::new("mask.png")), &[], false);
        let g = graph(&args);
        assert!(g.contains("crop=720:720:280:0,hflip,scale=270:270"), "{g}");
        assert!(g.contains("alphamerge"));
        assert_eq!(args.iter().filter(|a| *a == "-i").count(), 3);
    }

    #[test]
    fn the_audio_is_copied_and_nothing_cuts_the_game_short() {
        let args = args_for(CamLayout::default(), None, &[], false);
        assert!(args.windows(2).any(|w| w == ["-map", "0:a?"]));
        assert!(args.windows(2).any(|w| w == ["-c:a", "copy"]));
        assert!(args.windows(2).any(|w| w == ["-map", "[v]"]));
        let all = args.join(" ");
        for banned in ["vstack", "shortest", "-itsoffset", "geq"] {
            assert!(!all.contains(banned), "{banned}");
        }
        assert!(graph(&args).contains("overlay=x=1412:y=782:eval=init:eof_action=repeat"));
    }

    #[test]
    fn hardware_decoding_only_when_asked() {
        let args = args_for(CamLayout::default(), None, &[], true);
        let hw = args.iter().position(|a| a == "-hwaccel").unwrap();
        assert_eq!(args[hw + 1], "d3d11va");
        assert!(hw < args.iter().position(|a| a == "-i").unwrap());
        assert!(!args_for(CamLayout::default(), None, &[], false).iter().any(|a| a == "-hwaccel"));
    }

    #[test]
    fn a_save_time_composite_uses_the_fast_presets() {
        use crate::edit::{video_args_at, Speed};
        let has = |enc, words: &[&str]| {
            let args = video_args_at(enc, 70, Speed::Save);
            words.iter().all(|w| args.iter().any(|a| a == w))
        };
        assert!(has(EncoderId::Nvenc, &["p3", "20"]));
        assert!(has(EncoderId::Amf, &["speed", "20"]));
        assert!(has(EncoderId::Qsv, &["veryfast", "20"]));
        assert!(has(EncoderId::X264, &["superfast", "20"]));
        assert!(video_args_at(EncoderId::Nvenc, 70, Speed::Edit).iter().any(|a| a == "p5"));
    }

    #[test]
    fn the_retries_fall_back_to_software_step_by_step() {
        assert_eq!(
            attempts(EncoderId::Nvenc),
            vec![(EncoderId::Nvenc, true), (EncoderId::X264, true), (EncoderId::X264, false)]
        );
        assert_eq!(attempts(EncoderId::X264), vec![(EncoderId::X264, true), (EncoderId::X264, false)]);
    }

    #[test]
    fn a_circle_mask_is_clear_in_the_corners() {
        let mask = mask_gray8(CamShape::Circle, 100, 100).unwrap();
        assert_eq!(mask[0], 0);
        assert_eq!(mask[99], 0);
        assert_eq!(mask[99 * 100], 0);
        assert_eq!(mask[50 * 100 + 50], 255);
    }

    #[test]
    fn a_rounded_mask_is_opaque_along_the_middle_of_each_edge() {
        let (w, h) = (160usize, 90usize);
        let mask = mask_gray8(CamShape::Rounded, w as u32, h as u32).unwrap();
        assert_eq!(mask[w / 2], 255);
        assert_eq!(mask[(h - 1) * w + w / 2], 255);
        assert_eq!(mask[(h / 2) * w], 255);
        assert_eq!(mask[(h / 2) * w + w - 1], 255);
        assert_eq!(mask[0], 0);
    }

    #[test]
    fn a_rectangle_needs_no_mask() {
        assert!(mask_gray8(CamShape::Rect, 160, 90).is_none());
    }
}
