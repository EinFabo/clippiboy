//! Build a finished MP4 out of the packet ring buffer.
//!
//! The video is only copied (`-c copy`), not re-encoded — so saving costs
//! neither time nor quality.
//!
//! This used to hold MPEG-TS segment files that the `concat` demuxer had to
//! stitch together. Because every segment encoder started its timekeeping over
//! at zero, `+genpts` was needed and `-ss` had no effect — the clip always began
//! at a segment boundary and was up to ten seconds too long. Now the video comes
//! out of a single encoder as one continuous elementary stream: the cut sits on
//! the keyframe before it, and the timestamps follow from the constant frame
//! rate.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use crate::audio::capture::{CHANNELS, SAMPLE_RATE};
use crate::pipeline::{self, ClipSnapshot};
use crate::stems;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Folder holding the programs ClippiBoy fetched for itself (see `tools.rs`).
/// Set once they are in place.
static TOOL_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Where to find ffmpeg and ffprobe. Without a call only PATH is searched.
pub fn set_tool_dir(dir: PathBuf) {
    let _ = TOOL_DIR.set(dir);
}

/// ClippiBoy's own program, otherwise the name for the PATH search.
///
/// Its own deliberately beats PATH: some ffmpeg version that happens to be
/// installed may have different defaults, and the one pinned in `tools.rs` is the
/// one that was tested.
fn tool(name: &str) -> PathBuf {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    match TOOL_DIR.get().map(|dir| dir.join(&file)) {
        Some(path) if path.is_file() => path,
        _ => PathBuf::from(name),
    }
}

pub fn command(name: &str) -> Command {
    let mut command = Command::new(tool(name));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

pub fn ffmpeg() -> Command {
    command("ffmpeg")
}

pub fn run(command: &mut Command, what: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|err| format!("could not start ffmpeg ({what}): {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let tail: Vec<&str> = stderr.lines().rev().take(6).collect();
    Err(format!(
        "ffmpeg failed at '{what}': {}",
        tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
    ))
}

/// Can ffmpeg be found?
///
/// Both programs: a save without ffprobe would write the clip and then be unable
/// to check it.
pub fn available() -> bool {
    let answers = |mut program: Command| {
        program
            .arg("-version")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    };
    answers(ffmpeg()) && answers(command("ffprobe"))
}

pub struct ClipRequest {
    pub snapshot: ClipSnapshot,
    /// The id the clip will land under in the database — the individual tracks
    /// are filed by it.
    pub clip_id: String,
    pub output: PathBuf,
}

/// One audio input for [`encode`].
pub struct AudioInput {
    pub path: PathBuf,
    pub label: String,
    /// Headerless 16-bit PCM instead of WAV — what a recording writes, since
    /// its length is not known up front and WAV stops at 4 GB.
    pub raw: bool,
}

pub struct ClipResult {
    pub path: PathBuf,
    pub duration_ms: u64,
    pub size_bytes: u64,
    pub thumb_path: Option<PathBuf>,
}

pub fn build(request: ClipRequest) -> Result<ClipResult, String> {
    let snapshot = request.snapshot;
    if snapshot.packets.is_empty() {
        return Err("The replay buffer is still empty.".into());
    }

    if let Some(parent) = request.output.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // Named off the clip id and therefore unique per save: two concurrent
    // saves must not write each other's intermediate files out from under them.
    // Beside the clip, like the `.part.mp4` it is written as: on the drive the
    // free space was checked for. In the app data folder they went to C:
    // whatever drive the clips are on — a buffer's worth per save.
    let scratch = |kind: &str| request.output.with_file_name(scratch_name(&request.clip_id, kind));

    // Video packets as a raw H.264 elementary stream, straight into the file:
    // gathered in one buffer first, a long buffer at a high bitrate cost its
    // own size in memory a second time, for the whole save.
    let video_path = scratch("h264");
    let write_video = || -> std::io::Result<()> {
        let mut out = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(&video_path)?);
        // Prepend SPS/PPS. Most encoders send them before every IDR anyway —
        // any decoder skips a duplicate, but they must not be missing entirely.
        out.write_all(&snapshot.sequence_header)?;
        for packet in &snapshot.packets {
            out.write_all(&packet.data)?;
        }
        out.flush()
    };
    if let Err(err) = write_video() {
        let _ = std::fs::remove_file(&video_path);
        return Err(err.to_string());
    }

    // The mixer runs 80 ms behind the present: read at once, the clip's last
    // few dozen milliseconds would be windows it has not mixed yet, and so
    // zeros. Writing the video usually took longer than that and this returns
    // at once. Bounded, and no lock is held here: a stalled mixer costs the
    // tail, never the save.
    if !snapshot.mixed.wait_until(snapshot.end_100ns(), pipeline::MIX_WAIT) {
        log::info!("audio: the mixer has not reached the clip's end — its tail is silent");
    }

    // Audio tracks as WAV, cut to the same QPC as the first frame.
    //
    // The buffer holds one ring per source; which of them belong together is
    // decided here and now. Everything without a track of its own is summed into
    // the main mix, and that sum is drawn at this moment rather than while
    // recording — so a ⧉ flipped during the recording applies to the whole clip
    // and not just to the seconds after it.
    let mut wavs: Vec<AudioInput> = Vec::new();
    let wav_path = |name: &str| scratch(&format!("{}.wav", sanitize(name)));

    let mut into_mix: Vec<&pipeline::TrackRing> = Vec::new();
    let mut own: Vec<&Arc<pipeline::TrackRing>> = Vec::new();
    for track in &snapshot.tracks {
        if snapshot.main_mix.contains(&track.source_id) {
            into_mix.push(track);
        } else {
            own.push(track);
        }
    }

    // No main mix when nothing runs into it — every source has a track of its
    // own, and a track nobody feeds would only be a slider in the editor that
    // moves nothing.
    if !into_mix.is_empty() {
        let path = wav_path("mix");
        match pipeline::write_mix_wav(
            &into_mix,
            &path,
            snapshot.start_100ns,
            snapshot.audio_frames,
        ) {
            Ok(()) => wavs.push(AudioInput {
                path,
                label: "Main mix".into(),
                raw: false,
            }),
            Err(err) => {
                // Half written, it is no input, and it would stay beside the clip.
                let _ = std::fs::remove_file(&path);
                log::error!("could not write the main mix: {err}");
            }
        }
    }
    for track in own {
        let path = wav_path(&track.source_id);
        // Silently dropping a track would be the worst outcome: the clip would
        // simply be mute with nothing anywhere saying why.
        match track.write_wav_window(&path, snapshot.start_100ns, snapshot.audio_frames) {
            Ok(()) => wavs.push(AudioInput {
                path,
                label: track.label(),
                raw: false,
            }),
            Err(err) => {
                let _ = std::fs::remove_file(&path);
                log::error!("could not write audio track '{}': {err}", track.label());
            }
        }
    }

    // A clip is written in a second or two — nobody watches a bar for that.
    let outcome = encode(
        &video_path,
        &snapshot.rate,
        &wavs,
        &request.clip_id,
        request.output,
        Some(snapshot.audio_frames as u64 * 1000 / SAMPLE_RATE as u64),
        &|_| {},
    );
    let _ = std::fs::remove_file(&video_path);
    for input in &wavs {
        let _ = std::fs::remove_file(&input.path);
    }
    outcome
}

/// How a raw H.264 stream is laid out in time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timing {
    /// For ffmpeg's `-r`: pictures per second, possibly as a fraction.
    pub rate: String,
    /// How long the picture lasts, in microseconds — the audio is cut to it.
    pub span_us: u64,
}

/// The rate a raw stream of `frames` pictures plays at so it lasts as long as
/// it took to record.
///
/// The elementary stream carries no timestamps; ffmpeg spreads it at a fixed
/// rate. At the configured rate that is right only while every tick became a
/// frame. When the encoder falls short, the clock skips ticks
/// (`convert::pace`) and a frame lost in `mft::submit` leaves a hole; at the
/// nominal rate the picture then came out shorter than the time it covers and
/// ran ahead of the audio cut beside it. Spread over `first..=last` instead,
/// both last the same and meet at either end.
///
/// A single step between neighbouring frames longer than
/// [`DISCONTINUITY_US`](crate::buffer::DISCONTINUITY_US) (`largest_step_us`)
/// is not a short encoder but a gap — the PC slept with a recording running.
/// Stretched over that, the whole recording would play in slow motion, so the
/// nominal rate stays. Short of that, however far the encoder fell behind, the
/// frames are stretched: told apart by the ratio of span to frames, as before,
/// an encoder at 36 of 60 fps passed for sleep, and its clip played 1.67× fast
/// with the end of its sound cut off.
///
/// Without a largest step (a recording folder written before it was kept) the
/// old ratio decides: a span up to half again the nominal one is stretched.
pub fn timing(
    frames: u64,
    span_us: Option<u64>,
    largest_step_us: Option<i64>,
    fps: u32,
) -> Timing {
    let fps = fps.max(1) as u64;
    let nominal = frames * 1_000_000 / fps;
    let stretch = |span: u64| {
        frames > 0
            && span >= nominal
            && match largest_step_us {
                Some(step) => step <= crate::buffer::DISCONTINUITY_US,
                None => span <= nominal * 3 / 2,
            }
    };
    match span_us {
        Some(span) if stretch(span) => Timing {
            rate: format!("{}/{}", frames * 1_000_000, span),
            span_us: span,
        },
        _ => Timing {
            rate: fps.to_string(),
            span_us: nominal,
        },
    }
}

/// The longest step between two frames that follow each other in time, or 0
/// for fewer than two. Sorted first: should frames ever arrive out of order,
/// neighbours in the list are not neighbours in time.
pub fn largest_step_us(pts_us: &[i64]) -> i64 {
    let mut sorted = pts_us.to_vec();
    sorted.sort_unstable();
    sorted
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .max()
        .unwrap_or(0)
}

/// From the first and last timestamp of a run of frames to the span it covers:
/// the last frame lasts a period too.
pub fn span_of(first_pts_us: i64, last_pts_us: i64, fps: u32) -> Option<u64> {
    let period = 1_000_000 / fps.max(1) as i64;
    u64::try_from(last_pts_us - first_pts_us + period).ok()
}

/// ffmpeg's quicker AAC rate control. Stopping a recording encodes every
/// track at once, and with the default (`twoloop`) half an hour took about
/// 50 s; this takes well under half of that (ffmpeg 9: 10 min of audio in
/// 3.7 s instead of 9.8 s) at 192 kbit/s and no audible difference.
const AAC_CODER: &str = "fast";

/// Mux a raw H.264 stream and its audio into the finished clip: one mixed
/// audio track in the file, the inputs themselves kept as individual tracks
/// when there is more than one. Written beside the target and only renamed
/// onto it once ffprobe can read it.
///
/// The inputs are left where they are — clearing them away is the caller's.
pub fn encode(
    video_path: &Path,
    rate: &str,
    wavs: &[AudioInput],
    clip_id: &str,
    output: PathBuf,
    length_ms: Option<u64>,
    on_progress: &dyn Fn(f32),
) -> Result<ClipResult, String> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // The clip gets **one** audio track with everything in it. Discord, browsers
    // and most players stubbornly play only the first audio track of an MP4 — with
    // the sources on separate tracks alongside, as they used to be, they were
    // silent everywhere outside the editor.
    //
    // The levels from the recording mixer are already baked into the WAVs
    // (`AudioEngine::mix_window`), so this sums at 0 dB.
    let inputs: Vec<(String, f32)> = (1..=wavs.len())
        .map(|index| (format!("{index}:a"), 0.0))
        .collect();
    let filter = stems::mix_filter(&inputs);

    let mut command = ffmpeg();
    command.arg("-y").arg("-hide_banner").arg("-loglevel").arg("error");
    command.arg("-progress").arg("pipe:1").arg("-nostats");
    // The elementary stream carries no timestamps — the rate supplies them
    // ([`timing`]).
    command.arg("-f").arg("h264").arg("-r").arg(rate);
    command.arg("-i").arg(video_path);
    for input in wavs {
        if input.raw {
            command
                .arg("-f")
                .arg("s16le")
                .arg("-ar")
                .arg(SAMPLE_RATE.to_string())
                .arg("-ac")
                .arg(CHANNELS.to_string());
        }
        command.arg("-i").arg(&input.path);
    }
    if let Some(filter) = &filter {
        command.arg("-filter_complex").arg(filter);
    }

    // First output: the clip itself.
    command.arg("-map").arg("0:v:0");
    match &filter {
        Some(_) => {
            command.arg("-map").arg(stems::MIX_LABEL);
            command.arg("-c:a").arg("aac").arg("-b:a").arg("192k");
            command.arg("-aac_coder").arg(AAC_CODER);
            // The same name twice on purpose: MP4 has no single agreed field for
            // track names, and depending on the program one or the other is read.
            command.arg("-metadata:s:a:0").arg("title=Mix");
            command.arg("-metadata:s:a:0").arg("handler_name=Mix");
        }
        None => {
            command.arg("-an");
        }
    }
    command.arg("-c:v").arg("copy");
    // No `-shortest`: the video stream is only copied and is therefore done in
    // fractions of a second. ffmpeg then considers the output finished and closes
    // it before the AAC encoder has delivered its first packet — the file
    // demonstrably came out with no audio track at all. It is not needed either:
    // `write_wav_window` already cuts the audio to the length of the video.
    command.arg("-avoid_negative_ts").arg("make_zero");
    // No `+faststart`: that moves the moov atom to the front and reads the
    // finished file through once more to do it — at 40 Mbit/s and a two-minute
    // buffer, flatly twice the wait after the key press. Playback and trimming do
    // not need it.
    //
    // Written beside the target rather than onto it. ffmpeg fills an MP4 from the
    // front and writes the `moov` atom last, so a run that dies half way leaves
    // something that is the right size, carries the right name and opens as a
    // black nothing — and since the database row is only written on success,
    // nobody ever comes back to clear it away. The clip only takes its real name
    // once it is whole.
    //
    // Beside it, like the intermediate files: the clip folder may sit on
    // another drive, and a rename across volumes is not a rename.
    //
    // Named off the clip id, not off the file name: `with_extension` cuts at the
    // last dot, so two clips named `a.b.mp4` and `a.c.mp4` would both want to be
    // `a.part.mp4` — and two saves at once would write through each other, which
    // is the very thing the temp names above are careful to avoid.
    let pending = output.with_file_name(scratch_name(clip_id, "mp4"));
    let _ = std::fs::remove_file(&pending);
    command.arg(&pending);

    // Further outputs: the individual tracks, so the mix can still be changed
    // later. With only one track that would be a copy of the clip's audio track —
    // and that one will do.
    let keep_stems = wavs.len() > 1;
    let stems_dir = stems::dir(clip_id);
    if keep_stems {
        std::fs::create_dir_all(&stems_dir).map_err(|e| e.to_string())?;
        for index in 1..=wavs.len() {
            command.arg("-map").arg(format!("{index}:a"));
            command.arg("-c:a").arg("aac").arg("-b:a").arg("192k");
            command.arg("-aac_coder").arg(AAC_CODER);
            command.arg("-movflags").arg("+faststart");
            command.arg(stems::track_path(clip_id, index as u32 - 1));
        }
    }

    let outcome = run_reporting(&mut command, "writing the clip", length_ms, on_progress);

    // Everything from here that says "no" has to leave the half-written file
    // behind it gone, not lying in the clip folder.
    let give_up = |err: String| -> Result<ClipResult, String> {
        let _ = std::fs::remove_file(&pending);
        // Half-written individual tracks would be worse than none at all: the
        // editor would take them for complete and mix from them.
        if keep_stems {
            stems::remove(clip_id);
        }
        Err(err)
    };

    if let Err(err) = outcome {
        return give_up(err);
    }

    let size_bytes = std::fs::metadata(&pending).map(|meta| meta.len()).unwrap_or(0);
    if size_bytes == 0 {
        return give_up("The clip came out empty.".into());
    }

    // ffprobe as the actual test of whether the file is whole. Its length was
    // only ever a nicety before, with the failure swallowed by `unwrap_or` — but
    // a truncated MP4 is hundreds of megabytes of the right shape and the only
    // thing that notices is the one call whose answer was being thrown away. A
    // clip that cannot be read here cannot be played later either.
    let duration_ms = match probe_duration(&pending) {
        Ok(Some(ms)) => ms,
        Ok(None) => {
            return give_up(
                "The clip was written but cannot be read back — nothing was kept.".into(),
            )
        }
        // ffprobe itself is gone (it was there at the gate): that says nothing
        // about the file, so the clip stays.
        Err(err) => {
            log::warn!("{err} — keeping the clip unchecked");
            length_ms.unwrap_or(0)
        }
    };

    if let Err(err) = replace_file(&pending, &output) {
        return give_up(err);
    }

    if keep_stems {
        let labels: Vec<String> = wavs.iter().map(|input| input.label.clone()).collect();
        if let Err(err) = stems::write_index(clip_id, &labels) {
            log::warn!("track index not written: {err}");
            stems::remove(clip_id);
        }
    }

    let thumb_path = crate::thumbs::make(&output, clip_id).ok();

    Ok(ClipResult {
        path: output,
        duration_ms,
        size_bytes,
        thumb_path,
    })
}

/// Move a freshly written file into the place of an existing one.
///
/// Windows will not let an open file be replaced; the player releases it
/// beforehand, but the handle does not always vanish in the same instant — hence
/// a few attempts before giving up.
pub fn replace_file(temp: &Path, target: &Path) -> Result<(), String> {
    let mut last = None;
    for attempt in 0..10 {
        match std::fs::rename(temp, target) {
            Ok(()) => return Ok(()),
            Err(err) => {
                last = Some(err);
                std::thread::sleep(std::time::Duration::from_millis(50 * (attempt + 1)));
            }
        }
    }
    Err(format!(
        "Could not replace the clip ({}). Is it open somewhere right now?",
        last.map(|err| err.to_string()).unwrap_or_default()
    ))
}

/// [`run`], reading `-progress` along the way when the length is known.
///
/// Stdout carries the progress, stderr the error; both are drained on their
/// own, or a full pipe would stall ffmpeg.
fn run_reporting(
    command: &mut Command,
    what: &str,
    length_ms: Option<u64>,
    on_progress: &dyn Fn(f32),
) -> Result<(), String> {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("could not start ffmpeg ({what}): {err}"))?;
    let stderr = child.stderr.take();
    let collector = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(stderr) = stderr {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                text.push_str(&line);
                text.push('\n');
            }
        }
        text
    });
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let (Some(length), Some(us)) = (length_ms, line.strip_prefix("out_time_us=")) else {
                continue;
            };
            if let Ok(us) = us.trim().parse::<u64>() {
                on_progress((us as f32 / 1000.0 / length.max(1) as f32).clamp(0.0, 0.99));
            }
        }
    }
    let status = child.wait().map_err(|err| err.to_string())?;
    let stderr = collector.join().unwrap_or_default();
    if status.success() {
        return Ok(());
    }
    let tail: Vec<&str> = stderr.lines().rev().take(6).collect();
    Err(format!(
        "ffmpeg failed at '{what}': {}",
        tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
    ))
}

pub fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// The name of a save's intermediate file of `kind` (`h264`, `mp4`,
/// `<track>.wav`): `<clip id>.part.<kind>`.
///
/// The clip id alone keeps it unique, and short: with the file name in front, a
/// game named after a long window title pushed it past what Windows allows.
/// [`crate::sweep`] clears the ones a save did not live to clear.
pub fn scratch_name(clip_id: &str, kind: &str) -> String {
    format!("{}.part.{kind}", sanitize(clip_id))
}

/// The length of a media file in milliseconds. `Err` means ffprobe could not
/// be started at all — that says nothing about the file; `Ok(None)` means it ran
/// and could not read the file.
pub fn probe_duration(path: &Path) -> Result<Option<u64>, String> {
    probe_duration_by(command("ffprobe"), path)
}

fn probe_duration_by(mut program: Command, path: &Path) -> Result<Option<u64>, String> {
    let output = program
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .map_err(|err| format!("could not start ffprobe: {err}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text
        .trim()
        .parse::<f64>()
        .ok()
        .map(|seconds| (seconds * 1000.0) as u64))
}

pub fn probe_duration_ms(path: &Path) -> Option<u64> {
    probe_duration(path).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ffprobe_that_cannot_start_is_not_an_unreadable_clip() {
        let missing = Command::new("clippiboy-no-such-ffprobe");
        assert!(probe_duration_by(missing, Path::new("clip.mp4")).is_err());
    }

    #[test]
    fn scratch_names_carry_only_the_clip_id() {
        assert_eq!(
            scratch_name("1b4e28ba-2fa1-11d2-883f-0016d3cca427", "h264"),
            "1b4e28ba_2fa1_11d2_883f_0016d3cca427.part.h264"
        );
    }

    #[test]
    fn a_stream_without_losses_plays_at_the_configured_rate() {
        // 1800 frames at 60 fps, timestamps 0 … 29.983 s.
        let timing = timing(1800, span_of(0, 1799 * 1_000_000 / 60, 60), Some(16_667), 60);
        assert!(timing.span_us.abs_diff(30_000_000) < 100, "{}", timing.span_us);
        let rate = match timing.rate.split_once('/') {
            Some((a, b)) => a.parse::<f64>().unwrap() / b.parse::<f64>().unwrap(),
            None => timing.rate.parse::<f64>().unwrap(),
        };
        assert!((rate - 60.0).abs() < 0.01, "{}", timing.rate);
    }

    #[test]
    fn a_short_encoder_spreads_its_frames_over_the_real_time() {
        // 30 s of timestamps but only 1500 frames — the clock skipped ticks.
        let timing = timing(1500, Some(30_000_000), Some(266_666), 60);
        assert_eq!(timing.span_us, 30_000_000);
        assert_eq!(timing.rate, "1500000000/30000000");
    }

    #[test]
    fn a_gap_the_frames_cannot_explain_keeps_the_nominal_rate() {
        // A recording across an hour of sleep: stretching would be slow motion.
        let slept = timing(1800, Some(3_630_000_000), Some(3_600_016_666), 60);
        assert_eq!(slept.rate, "60");
        assert_eq!(slept.span_us, 30_000_000);
        // No span known (a recording recovered after a crash): the same.
        assert_eq!(timing(1800, None, None, 60).rate, "60");
    }

    /// SAVE-4: 36 of 60 fps for 30 s was muxed at 60 fps. The clip came out 18 s
    /// long, played 1.67× fast, and its end was 12 s away from its sound.
    #[test]
    fn a_sustained_shortfall_is_stretched_not_taken_for_sleep() {
        let timing = timing(1080, Some(30_000_000), Some(266_666), 60);
        assert_eq!(timing.rate, "1080000000/30000000");
        assert_eq!(timing.span_us, 30_000_000);
    }

    /// Review Focus: a recording folder from before this change has no largest
    /// step, and keeps the old rule.
    #[test]
    fn a_folder_without_a_largest_step_keeps_the_old_rule() {
        assert_eq!(timing(1080, Some(30_000_000), None, 60).rate, "60");
        assert_eq!(timing(1500, Some(30_000_000), None, 60).rate, "1500000000/30000000");
    }

    /// SAVE-4: steps are measured between neighbours in time, not in arrival order.
    #[test]
    fn the_largest_step_is_measured_between_neighbours_in_time() {
        assert_eq!(largest_step_us(&[0, 16_666, 1_516_666, 33_333]), 1_483_333);
        assert_eq!(largest_step_us(&[5]), 0);
    }
}
