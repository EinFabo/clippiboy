//! Export a copy of a clip that fits a size.
//!
//! Recording aims at a quality and lets the bitrate follow (see `mft.rs`). This
//! is the one place that does the opposite, and deliberately: the question here
//! is not "how good can it look" but "will it go through" — Discord takes 20 MB,
//! 50 or 500 MB with Nitro, and a clip that misses the limit is no use however
//! good it looks.
//!
//! So the arithmetic runs backwards. From the target size and the clip's length
//! comes a bitrate; if that bitrate is too thin for the resolution, the picture
//! is made smaller rather than muddier. Below a certain number of bits per pixel
//! H.264 stops resolving detail and starts smearing it, and a sharp 720p beats a
//! blurred 1080p every time.

use std::path::Path;

use crate::model::EncoderId;

/// The usual steps down. The source's own height is always tried first, so a
/// source taller than 1080p keeps its height when the budget allows, and
/// nothing is ever scaled up.
const HEIGHTS: [u32; 3] = [1080, 720, 480];

/// What one audio track costs. 128 kbit/s AAC is transparent enough for game
/// audio and, on a short clip at a small target, the difference between the
/// picture working and not.
const AUDIO_KBPS: u32 = 128;

/// What the sound drops to when, at [`AUDIO_KBPS`], the picture would be
/// left less than [`VIDEO_FLOOR_KBPS`]: still clear speech and game sound,
/// and on a long clip the room that keeps it under its target.
const THIN_AUDIO_KBPS: u32 = 64;

/// The least the picture is given. ffmpeg cannot encode at nothing, so a
/// target that leaves less than this cannot be met.
const VIDEO_FLOOR_KBPS: u32 = 200;

/// The container's own overhead — headers, the index, per-packet framing.
/// Three percent is generous for MP4 and a good deal cheaper than landing a
/// kilobyte over the limit.
const CONTAINER_OVERHEAD: u64 = 3;

/// Below this fraction of what a resolution normally wants, the picture is
/// scaled down instead of starved. Half of the 30 fps figure: at that point the
/// smearing is visible, and a smaller sharp picture is the better trade.
const STARVED: u32 = 2;

/// What is being asked for.
pub struct Target {
    /// The size to come in under, in bytes.
    pub bytes: u64,
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    /// What the clip itself spends, in kbit/s, when it is the picture being
    /// shrunk: the picture never gets more. A target far above the clip (a
    /// short clip at Nitro's 475 MB) would otherwise be met with a bitrate the
    /// clip never had — a bigger file, no better picture.
    pub source_kbps: Option<u32>,
}

/// What that works out to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recipe {
    pub video_kbps: u32,
    pub audio_kbps: u32,
    /// What to scale to. Equal to the source when nothing is scaled.
    pub width: u32,
    pub height: u32,
    /// The clip is too long for the target: even with the thinner sound the
    /// picture got its floor, and the file comes out larger than asked.
    pub too_long: bool,
}

impl Recipe {
    /// Is the picture being made smaller? Then the filter is needed.
    pub fn scales(&self, source_height: u32) -> bool {
        self.height < source_height
    }

    /// The same for a frame of either orientation.
    pub fn scales_frame(&self, frame: (u32, u32)) -> bool {
        self.width < frame.0 || self.height < frame.1
    }
}

/// Video bitrate left over for a target, in kbit/s. The clip's real length in
/// milliseconds: kbit/s times ms is bits, so a 5.9 s clip is budgeted for 5.9 s
/// and not rounded down to 5.
fn video_budget(bytes: u64, duration_ms: u64, audio_kbps: u32) -> u32 {
    let usable_bits = bytes.saturating_mul(8) * (100 - CONTAINER_OVERHEAD) / 100;
    let audio_bits = audio_kbps as u64 * duration_ms;
    let video_bits = usable_bits.saturating_sub(audio_bits);
    (video_bits / duration_ms.max(1000)).min(u32::MAX as u64) as u32
}

/// Work out how to hit a target size.
///
/// Steps the resolution down while the bitrate would be too thin for it, and
/// stops at the smallest step rather than looping — a target small enough to
/// starve 480p is going to look poor whatever happens, and saying so with a
/// picture is better than not producing one.
///
/// When the picture would get less than its floor, the sound gives up half
/// its bitrate first. Should that still not be enough, the recipe says so
/// (`too_long`): the export goes ahead at the floor, larger than asked.
pub fn recipe(target: &Target) -> Recipe {
    let mut audio_kbps = AUDIO_KBPS;
    let mut budget = video_budget(target.bytes, target.duration_ms, audio_kbps);
    if budget < VIDEO_FLOOR_KBPS {
        audio_kbps = THIN_AUDIO_KBPS;
        budget = video_budget(target.bytes, target.duration_ms, audio_kbps);
    }

    // Steps on the short side — the height of a landscape frame, the width of
    // a 9:16 one. Never upscale: the source's short side is the ceiling and
    // the first try.
    let short = target.width.min(target.height);
    let mut size = (target.width, target.height);
    for step in std::iter::once(short).chain(HEIGHTS.into_iter().filter(|h| *h < short)) {
        size = size_for(target, step);
        let wants = crate::encode::bitrate_for(size.0, size.1, 30);
        if budget >= wants / STARVED {
            break;
        }
    }

    // A floor, or ffmpeg is handed a bitrate it cannot encode at all.
    let video_kbps = budget.max(VIDEO_FLOOR_KBPS);
    let video_kbps = match target.source_kbps {
        Some(source) => video_kbps.min(source.max(VIDEO_FLOOR_KBPS)),
        None => video_kbps,
    };
    Recipe {
        video_kbps,
        audio_kbps,
        width: size.0,
        height: size.1,
        too_long: budget < VIDEO_FLOOR_KBPS,
    }
}

/// A size in MB as a notice or a file name shows it: one decimal, none when
/// it is whole.
pub fn megabytes(bytes: u64) -> String {
    let tenths = (bytes * 10 + 512 * 1024) / (1024 * 1024);
    if tenths.is_multiple_of(10) {
        format!("{}", tenths / 10)
    } else {
        format!("{}.{}", tenths / 10, tenths % 10)
    }
}

/// The size whose short side is `short`, keeping the source's shape, the long
/// side rounded down even — H.264 will not take an odd one.
fn size_for(target: &Target, short: u32) -> (u32, u32) {
    if target.width < target.height {
        let height = (target.height as u64 * short as u64 / target.width.max(1) as u64).max(2) as u32 & !1;
        (short, height)
    } else {
        (width_for(target, short), short)
    }
}

/// The width that keeps the source's shape at a given height, rounded even —
/// H.264 will not take an odd one.
fn width_for(target: &Target, height: u32) -> u32 {
    if target.height == 0 {
        return target.width;
    }
    let width = target.width as u64 * height as u64 / target.height as u64;
    (width.max(2) as u32) & !1
}

/// The ffmpeg line. Pulled out so it can be checked without a run, exactly like
/// `edit::arguments`. `remix`: the sound mixed again from the clip's tracks
/// instead of the clip's own (a track left out, `stems::without_discord`).
pub fn arguments(
    source: &Path,
    output: &Path,
    recipe: &Recipe,
    source_height: u32,
    encoder: EncoderId,
    remix: Option<&crate::stems::Remix>,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
        "-i".into(),
        source.to_string_lossy().to_string(),
    ];

    // The picture, then whatever audio there is. The `?` matters: a clip whose
    // tracks were all muted has no audio stream, and without it ffmpeg refuses
    // the whole run over a missing map.
    let sound = match remix {
        None => {
            args.extend(["-map", "0:v:0", "-map", "0:a:0?"].map(String::from));
            true
        }
        Some(remix) => {
            args.extend(remix.inputs());
            args.extend(["-map", "0:v:0"].map(String::from));
            remix_map(&mut args, remix, 1)
        }
    };

    if recipe.scales(source_height) {
        // `-2` keeps the aspect ratio and lands on an even side by itself;
        // a 9:16 frame is scaled by its width, its short side.
        args.push("-vf".into());
        args.push(if recipe.width < recipe.height {
            format!("scale={}:-2", recipe.width)
        } else {
            format!("scale=-2:{}", recipe.height)
        });
    }

    args.extend(video_args(encoder, recipe.video_kbps));
    if sound {
        args.extend(["-c:a", "aac", "-b:a"].map(String::from));
        args.push(format!("{}k", recipe.audio_kbps));
    }
    // Unlike a saved clip, this one exists to be sent. `+faststart` costs a
    // second pass over a file that is small by construction, and buys playback
    // that starts before the download has finished.
    args.extend(["-movflags", "+faststart"].map(String::from));
    args.push(output.to_string_lossy().to_string());
    args
}

/// The remixed sound's graph and map — or `-an` when nothing audible is left.
/// Says whether there is sound to encode. The graph stands alone: the
/// picture's `-vf` is a simple filter on another stream and may sit beside it.
fn remix_map(args: &mut Vec<String>, remix: &crate::stems::Remix, first_input: usize) -> bool {
    match remix.filter(first_input) {
        Some(graph) => {
            args.extend(["-filter_complex".into(), graph, "-map".into(), crate::stems::MIX_LABEL.into()]);
            true
        }
        None => {
            args.push("-an".into());
            false
        }
    }
}

/// Which way up the export is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Orientation {
    #[default]
    Landscape,
    /// 9:16: the camera on top, the game below.
    Portrait,
}

/// The 9:16 frame a portrait export starts from.
pub const PORTRAIT: (u32, u32) = (1080, 1920);

/// The files a 9:16 export reads: the game picture (the clean one when there
/// is one), the camera with its seek, the clip file for the sound, and the
/// watermark when it is drawn again.
#[derive(Clone, Copy)]
pub struct PortraitInputs<'a> {
    pub game: &'a Path,
    pub game_seek_ms: u64,
    pub cam: Option<(&'a Path, Option<f64>)>,
    pub audio: &'a Path,
    pub mark: Option<&'a Path>,
}

/// The ffmpeg line of a 9:16 export. Input 0 is the game, 1 the camera (when
/// there is one), then the clip file for the sound, then the mark — the order
/// `composite::portrait_filter` counts in.
pub fn portrait_arguments(
    inputs: &PortraitInputs,
    graph: &str,
    output: &Path,
    recipe: &Recipe,
    length_ms: u64,
    encoder: EncoderId,
    remix: Option<&crate::stems::Remix>,
) -> Vec<String> {
    let seconds = |ms: u64| format!("{:.3}", ms as f64 / 1000.0);
    let mut args: Vec<String> = ["-y", "-hide_banner", "-loglevel", "error", "-progress", "pipe:1", "-nostats"]
        .map(String::from)
        .to_vec();
    if inputs.game_seek_ms > 0 {
        args.extend(["-ss".into(), seconds(inputs.game_seek_ms)]);
    }
    args.extend(["-i".into(), inputs.game.to_string_lossy().to_string()]);
    if let Some((cam, seek)) = inputs.cam {
        if let Some(seek) = seek {
            args.extend(["-ss".into(), format!("{seek:.6}")]);
        }
        args.extend(["-i".into(), cam.to_string_lossy().to_string()]);
    }
    let audio_input = 1 + usize::from(inputs.cam.is_some());
    args.extend(["-i".into(), inputs.audio.to_string_lossy().to_string()]);
    if let Some(mark) = inputs.mark {
        args.extend(["-loop".into(), "1".into(), "-i".into(), mark.to_string_lossy().to_string()]);
    }
    // A remix's tracks after everything the picture's graph counts on; the
    // clip file stays an input, unread, so the graph's numbers hold.
    let remixed = remix.map(|remix| {
        let first = audio_input + 1 + usize::from(inputs.mark.is_some());
        args.extend(remix.inputs());
        (remix.filter(first), remix.tracks.len())
    });
    let graph = match &remixed {
        Some((Some(audio), _)) => format!("{graph};{audio}"),
        _ => graph.to_string(),
    };
    args.extend(["-filter_complex".into(), graph, "-map".into(), "[v]".into()]);
    let sound = match &remixed {
        None => {
            args.extend(["-map".into(), format!("{audio_input}:a:0?")]);
            true
        }
        Some((Some(_), _)) => {
            args.extend(["-map".into(), crate::stems::MIX_LABEL.into()]);
            true
        }
        Some((None, _)) => {
            args.push("-an".into());
            false
        }
    };
    args.extend(video_args(encoder, recipe.video_kbps));
    if sound {
        args.extend(["-c:a", "aac", "-b:a"].map(String::from));
        args.push(format!("{}k", recipe.audio_kbps));
    }
    args.extend(["-movflags", "+faststart"].map(String::from));
    // Last before the output: the looped mark and the cam would run on.
    args.extend(["-t".into(), seconds(length_ms)]);
    args.push(output.to_string_lossy().to_string());
    args
}

/// Aim at the bitrate, with a leash on it.
///
/// The opposite of `edit::video_args`, which aims at a quality — here the size
/// is the point. `maxrate` and `bufsize` are what keep a busy scene from
/// spending the whole budget in the first ten seconds.
fn video_args(encoder: EncoderId, kbps: u32) -> Vec<String> {
    let bitrate = format!("{kbps}k");
    let maxrate = format!("{}k", kbps + kbps / 10);
    let bufsize = format!("{}k", kbps.saturating_mul(2));
    let mut args: Vec<String> = match encoder {
        EncoderId::Nvenc => ["-c:v", "h264_nvenc", "-preset", "p5", "-rc", "cbr"]
            .map(String::from)
            .to_vec(),
        EncoderId::Amf => ["-c:v", "h264_amf", "-quality", "balanced", "-rc", "cbr"]
            .map(String::from)
            .to_vec(),
        EncoderId::Qsv => ["-c:v", "h264_qsv", "-preset", "medium"]
            .map(String::from)
            .to_vec(),
        EncoderId::X264 => ["-c:v", "libx264", "-preset", "medium"]
            .map(String::from)
            .to_vec(),
    };
    args.extend([
        "-b:v".into(),
        bitrate,
        "-maxrate".into(),
        maxrate,
        "-bufsize".into(),
        bufsize,
        "-pix_fmt".into(),
        "yuv420p".into(),
    ]);
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(bytes: u64, duration_ms: u64, width: u32, height: u32) -> Target {
        Target {
            bytes,
            duration_ms,
            width,
            height,
            source_kbps: None,
        }
    }

    const MB: u64 = 1024 * 1024;

    /// The common case: a short highlight at the Nitro limit has room to spare,
    /// so the picture is left alone.
    /// A 9:16 export steps on its short side, the width: 1080×1920, then
    /// 720×1280, then 480×852 — never 608×1080 from stepping on the height.
    #[test]
    fn portrait_audio_comes_from_the_clip_file() {
        let plan = recipe(&Target { bytes: 50 * MB, duration_ms: 20_000, width: PORTRAIT.0, height: PORTRAIT.1, source_kbps: None });
        let inputs = PortraitInputs {
            game: Path::new("clean.mp4"),
            game_seek_ms: 5_000,
            cam: Some((Path::new("cam.mp4"), Some(1.25))),
            audio: Path::new("clip.mp4"),
            mark: Some(Path::new("mark.png")),
        };
        let args = portrait_arguments(&inputs, "[0:v]null[v]", Path::new("out.mp4"), &plan, 20_000, EncoderId::X264, None);
        let joined = args.join(" ");
        assert!(joined.contains("-ss 5.000 -i clean.mp4 -ss 1.250000 -i cam.mp4 -i clip.mp4 -loop 1 -i mark.png"), "{joined}");
        assert!(args.windows(2).any(|w| w == ["-map", "[v]"]), "{joined}");
        assert!(args.windows(2).any(|w| w == ["-map", "2:a:0?"]), "the clip file is input 2: {joined}");
        assert!(args.windows(2).any(|w| w == ["-t", "20.000"]), "{joined}");
        assert!(!args.iter().any(|a| a == "-vf"), "-vf and -filter_complex never meet: {joined}");
        // Without a cam the clip file is input 1.
        let bare = PortraitInputs { cam: None, mark: None, ..inputs };
        let args = portrait_arguments(&bare, "[0:v]null[v]", Path::new("out.mp4"), &plan, 20_000, EncoderId::X264, None);
        assert!(args.windows(2).any(|w| w == ["-map", "1:a:0?"]), "{args:?}");
    }

    #[test]
    fn a_portrait_export_is_never_squashed() {
        let at = |mb: u64| recipe(&Target { bytes: mb * 1024 * 1024, duration_ms: 30_000, width: 1080, height: 1920, source_kbps: None });
        let roomy = at(100);
        assert_eq!((roomy.width, roomy.height), (1080, 1920));
        let tighter = at(8);
        assert_eq!((tighter.width, tighter.height), (720, 1280));
        let starved = at(3);
        assert_eq!((starved.width, starved.height), (480, 852));
        assert!(starved.scales(1920) && starved.scales_frame((1080, 1920)));
    }

    #[test]
    fn a_short_clip_at_25_mb_stays_at_its_size() {
        let plan = recipe(&target(25 * MB, 30_000, 1920, 1080));
        assert_eq!(plan.height, 1080);
        assert!(!plan.scales(1080));
        assert!(plan.video_kbps > 6_000, "{plan:?}");
    }

    /// Two minutes into 10 MB is roughly 600 kbit/s. At 1080p that is a smear;
    /// the picture goes down instead.
    #[test]
    fn a_long_clip_at_10_mb_gives_up_resolution_instead_of_detail() {
        let plan = recipe(&target(10 * MB, 120_000, 1920, 1080));
        assert!(plan.height < 1080, "{plan:?}");
        assert!(plan.scales(1080));
        // The shape is kept and the width stays even. Not to the pixel: 480p out
        // of 16:9 is 853.3 wide, and H.264 will not take an odd number — so the
        // rounding down to even may cost up to two pixels.
        assert_eq!(plan.width % 2, 0);
        let ideal = 1920 * plan.height / 1080;
        assert!(
            ideal.abs_diff(plan.width) <= 2,
            "{}x{} is not 16:9 any more",
            plan.width,
            plan.height
        );
    }

    /// An impossible target must still produce a plan rather than spin looking
    /// for one.
    #[test]
    fn an_absurd_target_lands_on_the_smallest_step() {
        let plan = recipe(&target(MB / 100, 600_000, 1920, 1080));
        assert_eq!(plan.height, 480);
        assert!(plan.video_kbps >= 200, "ffmpeg needs something to work with");
    }

    /// Final review I-2: a long clip at a small target used to be floored at
    /// 200 kbit/s with full sound and come out at 2.4 times the size asked
    /// for, without a word. The sound gives way first; when that is not
    /// enough the recipe says so.
    #[test]
    fn a_clip_too_long_for_its_target_says_so() {
        let ten_minutes = recipe(&target(10 * MB, 600_000, 1920, 1080));
        assert!(ten_minutes.too_long, "{ten_minutes:?}");
        assert_eq!((ten_minutes.video_kbps, ten_minutes.audio_kbps), (200, 64));

        // 280 s fits 10 MB only with the thinner sound.
        let rescued = recipe(&target(10 * MB, 280_000, 1920, 1080));
        assert!(!rescued.too_long, "{rescued:?}");
        assert_eq!(rescued.audio_kbps, 64);
        assert!(rescued.video_kbps >= 200);
        let predicted = (rescued.video_kbps + rescued.audio_kbps) as u64 * 280_000 / 8;
        assert!(predicted <= 10 * MB, "predicted {predicted} bytes");

        let short = recipe(&target(10 * MB, 60_000, 1920, 1080));
        assert_eq!(short.audio_kbps, 128, "a clip that fits keeps its full sound");
        assert!(!short.too_long);
    }

    #[test]
    fn sizes_are_shown_to_one_decimal() {
        assert_eq!(megabytes(10 * MB), "10");
        assert_eq!(megabytes(10 * MB + MB / 4), "10.3");
        assert_eq!(megabytes(24 * MB + 6 * MB / 10), "24.6");
        assert_eq!(megabytes(0), "0");
    }

    /// Never upscale — a 720p clip exported "at 25 MB" stays 720p.
    #[test]
    fn a_small_source_is_never_blown_up() {
        let plan = recipe(&target(25 * MB, 30_000, 1280, 720));
        assert_eq!(plan.height, 720);
        assert!(!plan.scales(720));
    }

    /// The audio has to come out of the budget, or every export lands over the
    /// limit by exactly the size of its sound.
    #[test]
    fn the_sound_is_paid_for_out_of_the_same_budget() {
        let plan = recipe(&target(10 * MB, 60_000, 1920, 1080));
        let total_kbps = plan.video_kbps + plan.audio_kbps;
        let predicted = total_kbps as u64 * 1000 / 8 * 60;
        assert!(
            predicted <= 10 * MB,
            "predicted {predicted} bytes over a {} byte target",
            10 * MB
        );
    }

    /// `-vf` may only appear when the picture really gets smaller: a scale
    /// filter that changes nothing still costs a full decode and re-encode of
    /// every frame through swscale.
    #[test]
    fn nothing_is_scaled_that_does_not_shrink() {
        let plan = recipe(&target(25 * MB, 30_000, 1920, 1080));
        let args = arguments(
            Path::new("C:/clips/x.mp4"),
            Path::new("C:/out/x.mp4"),
            &plan,
            1080,
            EncoderId::X264,
            None,
        );
        assert!(!args.iter().any(|a| a == "-vf"), "{args:?}");
    }

    #[test]
    fn a_shrunk_export_carries_its_scale_filter() {
        let plan = recipe(&target(10 * MB, 120_000, 1920, 1080));
        let args = arguments(
            Path::new("C:/clips/x.mp4"),
            Path::new("C:/out/x.mp4"),
            &plan,
            1080,
            EncoderId::X264,
            None,
        );
        let at = args.iter().position(|a| a == "-vf").expect("scale missing");
        assert_eq!(args[at + 1], format!("scale=-2:{}", plan.height));
    }

    /// The output path goes last and the input first — ffmpeg reads per-input
    /// options before their `-i` and output options after all of them.
    #[test]
    fn the_input_comes_first_and_the_output_last() {
        let plan = recipe(&target(25 * MB, 30_000, 1920, 1080));
        let args = arguments(
            Path::new("C:/clips/in.mp4"),
            Path::new("C:/out/out.mp4"),
            &plan,
            1080,
            EncoderId::Nvenc,
            None,
        );
        let input = args.iter().position(|a| a == "-i").unwrap();
        assert_eq!(args[input + 1], "C:/clips/in.mp4");
        assert_eq!(args.last().unwrap(), "C:/out/out.mp4");
    }

    /// A clip with every track muted has no audio stream at all. Without the
    /// `?` the map would fail the run instead of quietly finding nothing.
    #[test]
    fn a_clip_without_sound_still_exports() {
        let plan = recipe(&target(25 * MB, 30_000, 1920, 1080));
        let args = arguments(
            Path::new("C:/clips/x.mp4"),
            Path::new("C:/out/x.mp4"),
            &plan,
            1080,
            EncoderId::X264,
            None,
        );
        assert!(args.iter().any(|a| a == "0:a:0?"), "{args:?}");
    }

    /// "Mute Discord voices when sharing": the sound comes from the kept
    /// tracks, cut where the clip starts, and the clip's own is not mapped.
    #[test]
    fn a_remixed_export_maps_the_kept_tracks() {
        let plan = recipe(&target(10 * MB, 120_000, 1920, 1080));
        let remix = crate::stems::Remix {
            tracks: vec![(Path::new("t/0.m4a").into(), 0.0), (Path::new("t/1.m4a").into(), -3.0)],
            start_ms: 2_000,
            length_ms: 120_000,
        };
        let args = arguments(Path::new("x.mp4"), Path::new("o.mp4"), &plan, 1080, EncoderId::X264, Some(&remix));
        let joined = args.join(" ");
        assert!(joined.contains("-i x.mp4 -ss 2.000 -t 120.000 -i t/0.m4a -ss 2.000 -t 120.000 -i t/1.m4a"), "{joined}");
        assert!(!args.iter().any(|a| a == "0:a:0?"), "{joined}");
        assert!(args.windows(2).any(|w| w == ["-map", "[aout]"]), "{joined}");
        let graph = args.iter().position(|a| a == "-filter_complex").map(|at| &args[at + 1]).unwrap();
        assert!(graph.starts_with("[1:a]volume=1[m0];[2:a]"), "{graph}");
        assert!(args.windows(2).any(|w| w[0] == "-vf"), "the scale stays a simple filter: {joined}");

        // Nothing audible left: no sound at all rather than a failed map.
        let silent = crate::stems::Remix { tracks: Vec::new(), start_ms: 0, length_ms: 120_000 };
        let args = arguments(Path::new("x.mp4"), Path::new("o.mp4"), &plan, 1080, EncoderId::X264, Some(&silent));
        assert!(args.iter().any(|a| a == "-an") && !args.iter().any(|a| a == "-c:a"), "{args:?}");
    }

    #[test]
    fn a_remixed_portrait_counts_its_tracks_after_the_mark() {
        let plan = recipe(&target(25 * MB, 20_000, 1080, 1920));
        let inputs = PortraitInputs {
            game: Path::new("clean.mp4"),
            game_seek_ms: 0,
            cam: Some((Path::new("cam.mp4"), None)),
            audio: Path::new("clip.mp4"),
            mark: Some(Path::new("mark.png")),
        };
        let remix = crate::stems::Remix { tracks: vec![(Path::new("t/0.m4a").into(), 0.0)], start_ms: 0, length_ms: 20_000 };
        let args = portrait_arguments(&inputs, "[0:v]null[v]", Path::new("out.mp4"), &plan, 20_000, EncoderId::X264, Some(&remix));
        let joined = args.join(" ");
        assert!(joined.contains("-loop 1 -i mark.png -t 20.000 -i t/0.m4a"), "{joined}");
        assert!(joined.contains("[0:v]null[v];[4:a]volume=1[m0];[m0]anull[aout]"), "{joined}");
        assert!(args.windows(2).any(|w| w == ["-map", "[aout]"]), "{joined}");
        assert!(!joined.contains("2:a:0?"), "{joined}");
    }

    /// Every encoder has to produce a line ffmpeg can actually run.
    #[test]
    fn every_encoder_aims_at_the_target() {
        for encoder in [EncoderId::Nvenc, EncoderId::Amf, EncoderId::Qsv, EncoderId::X264] {
            let args = video_args(encoder, 4_000);
            assert!(args.contains(&"-c:v".to_string()), "{encoder:?}: {args:?}");
            assert!(args.contains(&"-b:v".to_string()), "{encoder:?}: {args:?}");
            assert!(args.contains(&"-pix_fmt".to_string()), "{encoder:?}: {args:?}");
        }
    }

    #[test]
    fn a_short_clip_stays_under_its_target() {
        for ms in [1_900u64, 5_900, 15_900] {
            let plan = recipe(&target(10 * MB, ms, 1920, 1080));
            let bits = (plan.video_kbps + plan.audio_kbps) as u64 * ms;
            assert!(bits / 8 <= 10 * MB, "{ms} ms: {} bytes", bits / 8);
        }
    }

    /// A 30 s clip at Nitro's 475 MB: the budget is over 100 Mbit/s, the clip
    /// spends 12. The picture gets what the clip had, not what the size allows.
    #[test]
    fn the_picture_never_gets_more_than_the_clip_spends() {
        let roomy = Target { source_kbps: Some(12_000), ..target(475 * MB, 30_000, 1920, 1080) };
        let plan = recipe(&roomy);
        assert_eq!(plan.video_kbps, 12_000);
        assert_eq!(plan.height, 1080);
        // Tight targets are unchanged by it.
        let tight = Target { source_kbps: Some(12_000), ..target(10 * MB, 120_000, 1920, 1080) };
        assert_eq!(recipe(&tight), recipe(&target(10 * MB, 120_000, 1920, 1080)));
        // And a starved clip still gets the floor.
        let thin = Target { source_kbps: Some(50), ..target(475 * MB, 30_000, 1920, 1080) };
        assert_eq!(recipe(&thin).video_kbps, VIDEO_FLOOR_KBPS);
    }

    #[test]
    fn a_1440p_clip_with_room_keeps_its_height() {
        let plan = recipe(&target(100 * MB, 30_000, 2560, 1440));
        assert_eq!(plan.height, 1440);
    }

    #[test]
    fn a_portrait_export_is_scaled_by_its_width() {
        let plan = recipe(&target((6.1 * MB as f64) as u64, 60_000, 1080, 1920));
        let args = arguments(Path::new("in.mp4"), Path::new("out.mp4"), &plan, 1920, EncoderId::X264, None);
        assert!(args.windows(2).any(|w| w == ["-vf", "scale=480:-2"]), "{args:?}");
    }

    /// The short side steps (spec 3.5): 480 wide, not 480 high — 270×480
    /// was the squashed size this used to give.
    #[test]
    fn a_portrait_clip_on_a_thin_budget_goes_to_480_wide() {
        let plan = recipe(&target((6.1 * MB as f64) as u64, 60_000, 1080, 1920));
        assert_eq!((plan.width, plan.height), (480, 852));
    }
}
