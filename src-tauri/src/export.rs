//! Export a copy of a clip that fits a size.
//!
//! Recording aims at a quality and lets the bitrate follow (see `mft.rs`). This
//! is the one place that does the opposite, and deliberately: the question here
//! is not "how good can it look" but "will it go through" — Discord takes 10 MB,
//! 25 MB with Nitro, and a clip that misses the limit is no use however good it
//! looks.
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

    // Never upscale: the source height is the ceiling and the first try.
    let mut height = target.height;
    for step in std::iter::once(target.height).chain(HEIGHTS.into_iter().filter(|h| *h < target.height)) {
        height = step;
        let wants = crate::encode::bitrate_for(width_for(target, step), step, 30);
        if budget >= wants / STARVED {
            break;
        }
    }

    Recipe {
        // A floor, or ffmpeg is handed a bitrate it cannot encode at all.
        video_kbps: budget.max(VIDEO_FLOOR_KBPS),
        audio_kbps,
        width: width_for(target, height),
        height,
        too_long: budget < VIDEO_FLOOR_KBPS,
    }
}

/// A size in MB as a notice or a file name shows it: one decimal, none when
/// it is whole.
pub fn megabytes(bytes: u64) -> String {
    let tenths = (bytes * 10 + 512 * 1024) / (1024 * 1024);
    if tenths % 10 == 0 {
        format!("{}", tenths / 10)
    } else {
        format!("{}.{}", tenths / 10, tenths % 10)
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
/// `edit::arguments`.
pub fn arguments(
    source: &Path,
    output: &Path,
    recipe: &Recipe,
    source_height: u32,
    encoder: EncoderId,
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
    args.extend(["-map", "0:v:0", "-map", "0:a:0?"].map(String::from));

    if recipe.scales(source_height) {
        // `-2` keeps the aspect ratio and lands on an even width by itself.
        args.push("-vf".into());
        args.push(format!("scale=-2:{}", recipe.height));
    }

    args.extend(video_args(encoder, recipe.video_kbps));
    args.extend(["-c:a", "aac", "-b:a"].map(String::from));
    args.push(format!("{}k", recipe.audio_kbps));
    // Unlike a saved clip, this one exists to be sent. `+faststart` costs a
    // second pass over a file that is small by construction, and buys playback
    // that starts before the download has finished.
    args.extend(["-movflags", "+faststart"].map(String::from));
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
        }
    }

    const MB: u64 = 1024 * 1024;

    /// The common case: a short highlight at the Nitro limit has room to spare,
    /// so the picture is left alone.
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
        );
        assert!(args.iter().any(|a| a == "0:a:0?"), "{args:?}");
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

    #[test]
    fn a_1440p_clip_with_room_keeps_its_height() {
        let plan = recipe(&target(100 * MB, 30_000, 2560, 1440));
        assert_eq!(plan.height, 1440);
    }

    #[test]
    fn a_portrait_clip_on_a_thin_budget_goes_to_480() {
        let plan = recipe(&target((6.1 * MB as f64) as u64, 60_000, 1080, 1920));
        assert_eq!((plan.width, plan.height), (270, 480));
    }
}
