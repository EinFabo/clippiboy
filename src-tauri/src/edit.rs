//! Write the clip exactly as it stands in the editor — mix **and** trim.
//!
//! The trim used to be nothing but a marker in the database: whoever sent the
//! clip sent the untrimmed one. Now it sits in the file. So nothing is lost
//! regardless, the untouched recording moves into a store beside the individual
//! tracks on the first real cut:
//!
//! ```text
//! <data>/originals/<clip-id>/video.mp4  the untouched recording
//! <data>/originals/<clip-id>/trim.json  where the excerpt sits inside it
//! ```
//!
//! The note is written **before** the video and deleted **after** it. That makes
//! it the signal "this original is alive", and [`repair`] can clean up any state
//! a crash in the middle of the swap leaves behind.
//!
//! # Three invariants
//!
//! 1. **The individual tracks stay untrimmed** and are always in coordinates of
//!    the original. They are never replaced — no handle problem on Windows, no
//!    second timeline that can run away.
//! 2. **Audio still sitting in the file** (legacy, or a recording with only one
//!    source) is in coordinates of the file that is currently input 0, and
//!    therefore inherits the *video* offset, not the stems offset.
//! 3. **Re-encoding always happens from the original**, never from the
//!    already-trimmed file. The quality loss thus stays at exactly one
//!    generation, however often you trim again.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use parking_lot::Mutex;

use crate::config;
use crate::model::{Clip, ClipOriginal, EncoderId, TrackMix};
use crate::muxer::{ffmpeg, probe_duration_ms, replace_file, sanitize};
use crate::stems;

/// From here on a boundary counts as deliberately set. Below that it is the
/// result of a handle that did not quite sit at the end stop — nobody should
/// wait on a re-encode run for that.
const EDGE_TOLERANCE_MS: u64 = 250;

/// Any shorter and there is no video left.
const MIN_LENGTH_MS: u64 = 200;

/// Only one render-and-swap at a time.
///
/// Two quick clicks on save would otherwise fight over the same intermediate
/// file — and worse: the second run would read the clip file while the first is
/// replacing it.
static WORKING: Mutex<()> = Mutex::new(());

/// Root of all originals. **Permanent**, like the individual tracks.
pub fn root() -> PathBuf {
    config::data_dir().join("originals")
}

/// A clip's folder.
pub fn dir(clip_id: &str) -> PathBuf {
    root().join(sanitize(clip_id))
}

/// The untouched recording.
pub fn video_path(clip_id: &str) -> PathBuf {
    dir(clip_id).join("video.mp4")
}

/// The note beside it.
///
/// Up to 0.1.7 this file was called `schnitt.json`. A store written back then
/// keeps that name — every path goes through this function, so reading, writing
/// and deleting all agree on it.
fn note_path(clip_id: &str) -> PathBuf {
    let legacy = dir(clip_id).join("schnitt.json");
    if legacy.is_file() {
        return legacy;
    }
    dir(clip_id).join("trim.json")
}

/// What we read from: the original, otherwise the clip file.
///
/// Everything that needs the full recording — extracting the individual tracks,
/// re-encoding, undoing the trim — goes through this path.
pub fn source_path(clip: &Clip) -> PathBuf {
    let original = video_path(&clip.id);
    if original.is_file() {
        original
    } else {
        PathBuf::from(&clip.path)
    }
}

/// Is there a complete original for this clip? Only with note *and* video.
pub fn has_original(clip_id: &str) -> bool {
    note_path(clip_id).is_file() && video_path(clip_id).is_file()
}

fn read_note(clip_id: &str) -> Option<ClipOriginal> {
    let text = std::fs::read_to_string(note_path(clip_id)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_note(clip_id: &str, original: &ClipOriginal) -> Result<(), String> {
    std::fs::create_dir_all(dir(clip_id)).map_err(|err| err.to_string())?;
    let text = serde_json::to_string_pretty(original).map_err(|err| err.to_string())?;
    std::fs::write(note_path(clip_id), text).map_err(|err| err.to_string())
}

/// Clear away everything belonging to a clip. The note first, then the video —
/// as long as the note is there the original counts as alive.
pub fn remove(clip_id: &str) {
    let _ = std::fs::remove_file(note_path(clip_id));
    let _ = std::fs::remove_file(video_path(clip_id));
    let _ = std::fs::remove_dir_all(dir(clip_id));
}

/// The excerpt the editor is showing right now — in seconds of the **current**
/// file, converted to milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trim {
    pub start_ms: u64,
    pub end_ms: u64,
}

impl Trim {
    /// The whole range, i.e. "cut nothing away".
    pub fn whole(duration_ms: u64) -> Self {
        Self {
            start_ms: 0,
            end_ms: duration_ms,
        }
    }
}

/// Where the video comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoSource {
    /// The delivered file — it is already trimmed as far as it should be and is
    /// only copied.
    Clip,
    /// The untouched recording. Re-encoding only ever happens from here.
    Original,
}

/// What the single ffmpeg run should do. Pure data, so the decision can be
/// checked without ffmpeg.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderPlan {
    pub video: VideoSource,
    /// Offset into the video source.
    pub video_start_ms: u64,
    /// Offset into the individual tracks — those are always in the original.
    pub stems_start_ms: u64,
    pub length_ms: u64,
    /// Does the video have to be recomputed? Only when cutting at the front.
    pub reencode: bool,
    /// How the clip stands in the database afterwards. `None` means the whole
    /// recording, no original needed.
    pub original: Option<ClipOriginal>,
}

/// Derive the job from the editor state.
///
/// `base` is the cut already sitting in the file; `trim` is the excerpt the user
/// marked on **this** file's timeline. The new cut in original coordinates is
/// therefore `base.start + trim.start .. base.start + trim.end` — which allows
/// trimming further inwards indefinitely. Pulling the range back open only works
/// through [`restore_plan`].
///
/// `base` comes from the database and not from disk: it carries the offset the
/// individual tracks sit at, and has to remain correct even if somebody cleared
/// the store out by hand. `has_original` says whether a file is still there as
/// well — only then can it be encoded from.
pub fn plan(
    base: Option<&ClipOriginal>,
    has_original: bool,
    clip_duration_ms: u64,
    trim: Trim,
) -> RenderPlan {
    let (offset, full) = match base {
        Some(original) => (original.start_ms, original.duration_ms),
        None => (0, clip_duration_ms),
    };

    let start = trim.start_ms.min(clip_duration_ms);
    let end = trim.end_ms.clamp(start + MIN_LENGTH_MS, clip_duration_ms.max(start + MIN_LENGTH_MS));

    let cuts_front = start > EDGE_TOLERANCE_MS;
    let cuts_back = end + EDGE_TOLERANCE_MS < clip_duration_ms;

    let absolute_start = offset + if cuts_front { start } else { 0 };
    let absolute_end = offset + if cuts_back { end } else { clip_duration_ms };
    let length_ms = absolute_end.saturating_sub(absolute_start).max(MIN_LENGTH_MS);

    // If everything at the front stays, the delivered file already is the right
    // beginning — then it is copied from and the video is not touched. That is
    // the common case: only change the mix, or only shorten at the back.
    //
    // If the front is cut, the video comes from the original: that keeps the loss
    // at one generation even if somebody trims three times over. If the store is
    // gone, the already-trimmed file has to do — then the offset into it is
    // relative, not absolute.
    let (video, video_start_ms) = match (cuts_front, has_original) {
        (false, _) => (VideoSource::Clip, 0),
        (true, true) => (VideoSource::Original, absolute_start),
        (true, false) => (VideoSource::Clip, start),
    };

    let trimmed = absolute_start > 0 || absolute_end < full;
    RenderPlan {
        video,
        video_start_ms,
        stems_start_ms: absolute_start,
        length_ms,
        reencode: cuts_front,
        original: trimmed.then_some(ClipOriginal {
            duration_ms: full,
            start_ms: absolute_start,
            end_ms: absolute_end,
        }),
    }
}

/// The job for undoing: the whole recording, copied losslessly.
pub fn restore_plan(base: &ClipOriginal) -> RenderPlan {
    RenderPlan {
        video: VideoSource::Original,
        video_start_ms: 0,
        stems_start_ms: 0,
        length_ms: base.duration_ms,
        reencode: false,
        original: None,
    }
}

/// What lands in the database after writing.
pub struct Applied {
    pub duration_ms: u64,
    pub size_bytes: u64,
    pub original: Option<ClipOriginal>,
}

/// Rewrite the clip: apply the mix, carry out the trim, save the original.
/// Returns what has to stand in the database afterwards.
pub fn apply(
    clip: &Clip,
    trim: Trim,
    mix: &[TrackMix],
    encoder: EncoderId,
    bitrate_kbps: u32,
    on_progress: impl Fn(f32),
) -> Result<Applied, String> {
    // The record on the clip is the truth about the coordinates; the store only
    // says whether there is still a file to go with it.
    let plan = plan(
        clip.original.as_ref(),
        has_original(&clip.id),
        clip.duration_ms,
        trim,
    );
    run(clip, &plan, mix, encoder, bitrate_kbps, &on_progress)
}

/// Undo the trim: pull the whole recording back, keep the mix.
pub fn restore(
    clip: &Clip,
    mix: &[TrackMix],
    encoder: EncoderId,
    bitrate_kbps: u32,
    on_progress: impl Fn(f32),
) -> Result<Applied, String> {
    let base = clip
        .original
        .filter(|_| has_original(&clip.id))
        .ok_or_else(|| "The original can no longer be found.".to_string())?;
    let plan = restore_plan(&base);
    run(clip, &plan, mix, encoder, bitrate_kbps, &on_progress)
}

fn run(
    clip: &Clip,
    plan: &RenderPlan,
    mix: &[TrackMix],
    encoder: EncoderId,
    bitrate_kbps: u32,
    on_progress: &impl Fn(f32),
) -> Result<Applied, String> {
    let _busy = WORKING.lock();

    let target = PathBuf::from(&clip.path);
    if !target.is_file() {
        return Err("The clip file is no longer there.".into());
    }

    // Fetch the individual tracks before anything else: if they are not in the
    // store yet they get pulled out of the untouched recording here — and that
    // stays reachable under `source_path` until the swap.
    let source = source_path(clip);
    let list = match stems::tracks(&clip.id, &source) {
        Ok(list) => list,
        Err(err) if recoverable(&source) => {
            log::warn!("tracks unusable ({err}) — extracting them again");
            stems::remove(&clip.id);
            stems::tracks(&clip.id, &source)?
        }
        Err(err) => return Err(err),
    };

    let video = match plan.video {
        VideoSource::Clip => target.clone(),
        VideoSource::Original => source.clone(),
    };
    if !video.is_file() {
        return Err("The untouched recording can no longer be found.".into());
    }

    // ffmpeg must not write into its own input — so beside it, and over it
    // afterwards. The process id makes the file unique per run.
    let temp = target.with_extension(format!("{}.neu.mp4", std::process::id()));
    let args = arguments(plan, &video, &list, mix, &temp, encoder, bitrate_kbps);

    match run_with_progress(&args, plan.length_ms, on_progress) {
        Ok(()) => {}
        Err(err) if plan.reencode && encoder != EncoderId::X264 => {
            // Hardware encoders like to refuse: driver trouble, or all sessions
            // occupied because the replay buffer is running next door.
            log::warn!("trim with {encoder:?} failed ({err}) — retrying with x264");
            let _ = std::fs::remove_file(&temp);
            let fallback = arguments(
                plan,
                &video,
                &list,
                mix,
                &temp,
                EncoderId::X264,
                bitrate_kbps,
            );
            if let Err(err) = run_with_progress(&fallback, plan.length_ms, on_progress) {
                let _ = std::fs::remove_file(&temp);
                return Err(err);
            }
        }
        Err(err) => {
            let _ = std::fs::remove_file(&temp);
            return Err(err);
        }
    }

    swap_in(clip, plan, &temp, &target)?;
    on_progress(1.0);

    // What gets recorded is what really came out — not what was asked for.
    // Otherwise the bookkeeping drifts away from the file with every further
    // cut.
    let duration_ms = probe_duration_ms(&target).unwrap_or(plan.length_ms);
    let original = plan.original.map(|original| ClipOriginal {
        end_ms: original.start_ms + duration_ms,
        ..original
    });
    if let Some(original) = original.as_ref() {
        write_note(&clip.id, original)?;
    }

    // Otherwise the thumbnail would show a frame that no longer appears in the
    // clip at all.
    if let Err(err) = crate::thumbs::make(&target, &clip.id) {
        log::warn!("could not refresh the thumbnail: {err}");
    }

    Ok(Applied {
        duration_ms,
        size_bytes: std::fs::metadata(&target).map(|meta| meta.len()).unwrap_or(0),
        original,
    })
}

/// Move a file, even onto another drive.
///
/// The originals store sits next to the config in `AppData`, the clips lie
/// wherever the folder in the settings points — a second disk, say. `rename`
/// refuses that outright ("Das System kann die Datei nicht auf ein anderes
/// Laufwerk verschieben", os error 17), and no amount of retrying changes it.
/// Only then is the file copied and the source removed afterwards; that costs
/// the time a copy of a few hundred megabytes takes, but it is the only way
/// across.
fn move_across(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Err(err) if err.kind() == std::io::ErrorKind::CrossesDevices => {
            std::fs::copy(from, to)?;
            // From here on the copy is the one that counts. If the source will
            // not go, it stays behind as a leftover — annoying, not fatal, and
            // above all not a reason to fail the move.
            if let Err(err) = std::fs::remove_file(from) {
                log::warn!("'{}' stayed behind after the move: {err}", from.display());
            }
            Ok(())
        }
        other => other,
    }
}

/// Move the finished file into its place — and, if this is the first real cut,
/// rescue the untouched recording along the way.
///
/// The order is the crux: every step can be interrupted on its own, and the last
/// one rolls back. That way the clip can never disappear entirely at any point.
fn swap_in(clip: &Clip, plan: &RenderPlan, temp: &Path, target: &Path) -> Result<(), String> {
    let archive = video_path(&clip.id);
    let keep_original = plan.original.is_some();
    let first_cut = keep_original && !archive.is_file();

    if first_cut {
        // Note first: if the process dies right away, `repair` finds the file
        // again by it. The other way round a video would lie there with nothing
        // to tie it to.
        write_note(&clip.id, plan.original.as_ref().expect("checked"))?;
        if let Err(err) = move_across(target, &archive) {
            let _ = std::fs::remove_file(temp);
            let _ = std::fs::remove_file(note_path(&clip.id));
            return Err(format!(
                "Could not save the original ({err}). Is the clip open somewhere right now?"
            ));
        }
    }

    if let Err(err) = replace_file(temp, target) {
        if first_cut {
            // Back to the start: better an untrimmed clip than none at all.
            let _ = move_across(&archive, target);
            let _ = std::fs::remove_file(note_path(&clip.id));
            let _ = std::fs::remove_dir_all(dir(&clip.id));
        }
        let _ = std::fs::remove_file(temp);
        return Err(err);
    }

    if !keep_original {
        remove(&clip.id);
    }
    Ok(())
}

/// The ffmpeg line. Pulled out so it can be checked without a run.
fn arguments(
    plan: &RenderPlan,
    video: &Path,
    list: &[crate::model::ClipTrack],
    mix: &[TrackMix],
    output: &Path,
    encoder: EncoderId,
    bitrate_kbps: u32,
) -> Vec<String> {
    let gains = stems::levels(list, mix);

    // Where the audio comes from: from the stored individual tracks, or — if
    // there are none — from the video file itself.
    let stem_files: Vec<PathBuf> = list
        .iter()
        .filter_map(|track| track.preview_path.as_ref().map(PathBuf::from))
        .collect();
    let from_stems = stem_files.len() == list.len() && !stem_files.is_empty();

    let seconds = |ms: u64| format!("{:.3}", ms as f64 / 1000.0);

    let mut args: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
    ];

    // `-ss` is a **per-input** option and belongs before that input's `-i`. A
    // single `-ss` before the first `-i` would only affect the video, and the
    // individual tracks would run on offset by the trim.
    //
    // When copying, `video_start_ms` is always 0 — an `-ss` with `-c:v copy`
    // would slide to the keyframe before it (up to two seconds) and, placed after
    // `-i`, would even end up with no leading keyframe at all.
    if plan.video_start_ms > 0 {
        args.push("-ss".into());
        args.push(seconds(plan.video_start_ms));
    }
    args.push("-i".into());
    args.push(video.to_string_lossy().to_string());

    if from_stems {
        for path in &stem_files {
            if plan.stems_start_ms > 0 {
                args.push("-ss".into());
                args.push(seconds(plan.stems_start_ms));
            }
            args.push("-i".into());
            args.push(path.to_string_lossy().to_string());
        }
    }

    // If the audio still sits in the video file it shares that file's timeline
    // and is therefore already cut by the video's `-ss` (invariant 2).
    let inputs: Vec<(String, f32)> = if from_stems {
        gains
            .iter()
            .enumerate()
            .map(|(slot, gain)| (format!("{}:a", slot + 1), *gain))
            .collect()
    } else {
        list.iter()
            .zip(&gains)
            .map(|(track, gain)| (format!("0:a:{}", track.index), *gain))
            .collect()
    };
    let filter = stems::mix_filter(&inputs);

    if let Some(filter) = &filter {
        args.push("-filter_complex".into());
        args.push(filter.clone());
    }

    // Map the video first so it is track 0 in the result too.
    args.push("-map".into());
    args.push("0:v:0".into());
    match &filter {
        Some(_) => {
            args.push("-map".into());
            args.push(stems::MIX_LABEL.into());
            args.extend(["-c:a", "aac", "-b:a", "192k"].map(String::from));
            args.extend(["-metadata:s:a:0", "title=Mix"].map(String::from));
            args.extend(["-metadata:s:a:0", "handler_name=Mix"].map(String::from));
        }
        // Everything muted: then the clip simply gets no audio track.
        None => args.push("-an".into()),
    }

    if plan.reencode {
        args.extend(video_args(encoder, bitrate_kbps));
    } else {
        args.extend(["-c:v", "copy"].map(String::from));
    }
    args.extend(["-avoid_negative_ts", "make_zero"].map(String::from));
    args.extend(["-movflags", "+faststart"].map(String::from));

    // `-t` applies to the whole output and therefore belongs after all inputs.
    args.push("-t".into());
    args.push(seconds(plan.length_ms));
    args.push(output.to_string_lossy().to_string());
    args
}

/// Can the individual tracks be pulled again if need be?
///
/// Only then may the store be thrown away. For clips from the new recording path
/// it is the **only** copy of the separated sources — deleting it would mean
/// nailing the mix down forever.
fn recoverable(source: &Path) -> bool {
    stems::tracks_in_file(source)
        .map(|list| list.len() > 1)
        .unwrap_or(false)
}

fn video_args(encoder: EncoderId, bitrate_kbps: u32) -> Vec<String> {
    let rate = bitrate_kbps.max(4_000);
    let bitrate = format!("{rate}k");
    let maxrate = format!("{}k", rate * 3 / 2);
    let common = |mut args: Vec<&str>| -> Vec<String> {
        args.extend(["-pix_fmt", "yuv420p"]);
        args.into_iter().map(String::from).collect()
    };
    match encoder {
        EncoderId::Nvenc => {
            let mut args = common(vec!["-c:v", "h264_nvenc", "-preset", "p5", "-rc", "vbr"]);
            args.extend(["-b:v".into(), bitrate, "-maxrate".into(), maxrate]);
            args
        }
        EncoderId::Amf => {
            let mut args = common(vec!["-c:v", "h264_amf", "-quality", "balanced"]);
            args.extend(["-b:v".into(), bitrate]);
            args
        }
        EncoderId::Qsv => {
            let mut args = common(vec!["-c:v", "h264_qsv", "-preset", "medium"]);
            args.extend(["-b:v".into(), bitrate]);
            args
        }
        EncoderId::X264 => common(vec!["-c:v", "libx264", "-preset", "veryfast", "-crf", "20"]),
    }
}

/// Start ffmpeg and read `-progress` along the way. Without it the app would
/// stand there for minutes with no sign of life on a re-encoded clip.
fn run_with_progress(
    args: &[String],
    length_ms: u64,
    on_progress: &impl Fn(f32),
) -> Result<(), String> {
    let mut child = ffmpeg()
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("could not start ffmpeg: {err}"))?;

    // Drain stderr on a thread of its own: if the pipe filled up, ffmpeg could
    // hang while writing.
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
            let Some(time) = line.strip_prefix("out_time=") else {
                continue;
            };
            if let Some(ms) = parse_timestamp_ms(time) {
                on_progress((ms as f32 / length_ms.max(1) as f32).clamp(0.0, 0.99));
            }
        }
    }

    let status = child.wait().map_err(|err| err.to_string())?;
    let stderr = collector.join().unwrap_or_default();
    if status.success() {
        return Ok(());
    }
    let tail: Vec<&str> = stderr.lines().rev().take(4).collect();
    Err(format!(
        "ffmpeg failed while writing the clip: {}",
        tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
    ))
}

/// `00:01:02.500000` in milliseconds.
fn parse_timestamp_ms(text: &str) -> Option<u64> {
    let mut parts = text.trim().split(':');
    let hours: f64 = parts.next()?.parse().ok()?;
    let minutes: f64 = parts.next()?.parse().ok()?;
    let seconds: f64 = parts.next()?.parse().ok()?;
    Some(((hours * 3600.0 + minutes * 60.0 + seconds) * 1000.0) as u64)
}

/// Clean up at startup whatever a crash in the middle of the swap left behind.
///
/// The note tells the cases apart. The rule underneath is always the same:
/// **the record on the clip must never be lost while the file is trimmed** — it
/// carries the offset the individual tracks sit at, and without it the preview
/// would run out of sync forever.
pub fn repair(library: &crate::clips::Library) {
    let clips = match library.list() {
        Ok(clips) => clips,
        Err(err) => {
            log::warn!("originals store not checkable: {err}");
            return;
        }
    };
    for clip in clips {
        let note = read_note(&clip.id);
        let video = video_path(&clip.id);
        let target = Path::new(&clip.path);

        match (target.is_file(), note, video.is_file()) {
            // Died between saving and swapping: the clip file is missing, the
            // untouched recording lies in the store. Push it back — an untrimmed
            // clip is infinitely better than none at all.
            (false, Some(_), true) => {
                if move_across(&video, target).is_ok() {
                    log::info!("clip '{}' recovered from the originals store", clip.id);
                    remove(&clip.id);
                    let _ = library.set_original(&clip.id, None);
                }
            }

            // Note without a recording. Two possibilities, and the clip's length
            // says which: if it still stands at the full duration, the crash came
            // before the save and nothing was ever trimmed. Otherwise somebody
            // cleared the store out by hand — then the recording is lost, but the
            // tracks' offset is not.
            (_, Some(note), false) => {
                remove(&clip.id);
                if clip.duration_ms == note.duration_ms {
                    log::info!("note without an original at '{}' — cleared away", clip.id);
                    let _ = library.set_original(&clip.id, None);
                } else {
                    log::warn!("original of '{}' is gone — the clip stays trimmed", clip.id);
                    let _ = library.set_original(&clip.id, Some(&note));
                }
            }

            // Swapped, but the database never got its turn.
            (true, Some(note), true) if clip.original.is_none() => {
                log::info!("original of '{}' recorded after the fact", clip.id);
                let _ = library.set_original(&clip.id, Some(&note));
            }

            // Recording without a note. If the database still knows where the
            // excerpt sits, the note is reconstructed from it — otherwise this is
            // the remainder of a completed "undo" and may go.
            (true, None, true) => match clip.original.as_ref() {
                Some(original) => {
                    log::info!("note for '{}' reconstructed", clip.id);
                    let _ = write_note(&clip.id, original);
                }
                None => {
                    log::info!("original of '{}' with nothing to tie it to — cleared away", clip.id);
                    remove(&clip.id);
                }
            },

            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ClipTrack;

    fn track(index: u32, stem: bool) -> ClipTrack {
        ClipTrack {
            index,
            label: format!("Track {index}"),
            channels: 2,
            preview_path: stem.then(|| format!("C:/data/tracks/x/{index}.m4a")),
        }
    }

    /// The most common case: only the sliders were moved. Then the video stays
    /// untouched and the file is rewritten in seconds.
    #[test]
    fn only_mixing_leaves_the_picture_alone() {
        let plan = plan(None, true, 30_000, Trim::whole(30_000));
        assert_eq!(plan.video, VideoSource::Clip);
        assert!(!plan.reencode);
        assert_eq!(plan.video_start_ms, 0);
        assert_eq!(plan.stems_start_ms, 0);
        assert_eq!(plan.length_ms, 30_000);
        assert_eq!(plan.original, None, "with no trim no original is needed");
    }

    /// Shortening at the back is lossless and fast — and that is exactly the
    /// usual trim.
    #[test]
    fn trimming_only_the_tail_stays_a_copy() {
        let plan = plan(None, true, 30_000, Trim { start_ms: 0, end_ms: 12_000 });
        assert_eq!(plan.video, VideoSource::Clip);
        assert!(!plan.reencode);
        assert_eq!(plan.length_ms, 12_000);
        assert_eq!(
            plan.original,
            Some(ClipOriginal { duration_ms: 30_000, start_ms: 0, end_ms: 12_000 })
        );
    }

    /// Cutting at the front has to be frame-accurate. When copying, the cut
    /// would slide to the keyframe before it — hence re-encoding, and from the
    /// original at that, so the loss stays at one generation.
    #[test]
    fn trimming_the_head_re_encodes_from_the_original() {
        let plan = plan(None, true, 30_000, Trim { start_ms: 4_000, end_ms: 12_000 });
        assert_eq!(plan.video, VideoSource::Original);
        assert!(plan.reencode);
        assert_eq!(plan.video_start_ms, 4_000);
        assert_eq!(plan.stems_start_ms, 4_000);
        assert_eq!(plan.length_ms, 8_000);
    }

    /// If the store was cleared out by hand the cut still has to work — then
    /// from the already-trimmed file, with a **relative** offset. The individual
    /// tracks keep their absolute one.
    #[test]
    fn a_missing_original_falls_back_to_the_clip() {
        let base = ClipOriginal { duration_ms: 60_000, start_ms: 5_000, end_ms: 20_000 };
        let plan = plan(Some(&base), false, 15_000, Trim { start_ms: 2_000, end_ms: 8_000 });
        assert_eq!(plan.video, VideoSource::Clip);
        assert!(plan.reencode, "bildgenau muss es trotzdem sein");
        assert_eq!(plan.video_start_ms, 2_000, "the clip file already starts at 5 s");
        assert_eq!(plan.stems_start_ms, 7_000, "the tracks are in the original");
        assert_eq!(plan.length_ms, 6_000);
    }

    /// A handle that did not quite sit at the end stop must not trigger an
    /// encode run.
    #[test]
    fn a_hair_off_the_edge_is_not_a_cut() {
        let plan = plan(None, true, 30_000, Trim { start_ms: 120, end_ms: 29_900 });
        assert!(!plan.reencode);
        assert_eq!(plan.video, VideoSource::Clip);
        assert_eq!(plan.original, None);
        assert_eq!(plan.length_ms, 30_000);
    }

    /// Trimming further inwards works without undoing: the handles are on the
    /// trimmed file's timeline, the arithmetic happens in the original.
    #[test]
    fn cuts_compose_in_original_coordinates() {
        let base = ClipOriginal { duration_ms: 60_000, start_ms: 5_000, end_ms: 20_000 };
        let plan = plan(Some(&base), true, 15_000, Trim { start_ms: 2_000, end_ms: 8_000 });
        assert_eq!(plan.video_start_ms, 7_000);
        assert_eq!(plan.stems_start_ms, 7_000);
        assert_eq!(plan.length_ms, 6_000);
        assert_eq!(
            plan.original,
            Some(ClipOriginal { duration_ms: 60_000, start_ms: 7_000, end_ms: 13_000 })
        );
    }

    /// Trimmed three times over and the bookkeeping still adds up.
    #[test]
    fn three_cuts_in_a_row_still_add_up() {
        let mut base = None;
        let mut duration = 60_000;
        for _ in 0..3 {
            let plan = plan(base.as_ref(), true, duration, Trim { start_ms: 1_000, end_ms: duration - 1_000 });
            duration = plan.length_ms;
            base = plan.original;
        }
        let base = base.unwrap();
        assert_eq!(base.start_ms, 3_000);
        assert_eq!(base.end_ms, 57_000);
        assert_eq!(duration, 54_000);
    }

    /// Only change the mix although the clip has long been trimmed: the excerpt
    /// must not wander, and the tracks still need their offset.
    #[test]
    fn remixing_a_trimmed_clip_keeps_its_bounds() {
        let base = ClipOriginal { duration_ms: 60_000, start_ms: 5_000, end_ms: 20_000 };
        let plan = plan(Some(&base), true, 15_000, Trim::whole(15_000));
        assert_eq!(plan.video, VideoSource::Clip);
        assert!(!plan.reencode);
        assert_eq!(plan.video_start_ms, 0, "the clip file already starts in the right place");
        assert_eq!(plan.stems_start_ms, 5_000, "the tracks are in the original");
        assert_eq!(plan.length_ms, 15_000);
        assert_eq!(plan.original, Some(base));
    }

    #[test]
    fn restoring_is_a_lossless_copy_of_everything() {
        let base = ClipOriginal { duration_ms: 60_000, start_ms: 5_000, end_ms: 20_000 };
        let plan = restore_plan(&base);
        assert_eq!(plan.video, VideoSource::Original);
        assert!(!plan.reencode);
        assert_eq!(plan.stems_start_ms, 0);
        assert_eq!(plan.length_ms, 60_000);
        assert_eq!(plan.original, None);
    }

    /// `-ss` applies per input. Placed only once at the front, the tracks run on
    /// offset by the trim — the audio would come from a different point in the
    /// game than the picture.
    #[test]
    fn every_input_gets_its_own_seek() {
        let plan = plan(None, true, 30_000, Trim { start_ms: 4_000, end_ms: 12_000 });
        let list = vec![track(0, true), track(1, true)];
        let args = arguments(
            &plan,
            Path::new("C:/data/originals/x/video.mp4"),
            &list,
            &[],
            Path::new("C:/clips/x.neu.mp4"),
            EncoderId::X264,
            40_000,
        );

        let inputs: Vec<usize> = args
            .iter()
            .enumerate()
            .filter(|(_, arg)| *arg == "-i")
            .map(|(at, _)| at)
            .collect();
        assert_eq!(inputs.len(), 3, "Video plus zwei Spuren: {args:?}");
        for at in inputs {
            assert_eq!(args[at - 2], "-ss", "jede Eingabe braucht ihr -ss: {args:?}");
            assert_eq!(args[at - 1], "4.000");
        }
    }

    /// `-t` applies to the output and has to come after all inputs — read at the
    /// front it would be an option of input 0.
    #[test]
    fn the_length_comes_after_the_last_input() {
        let plan = plan(None, true, 30_000, Trim { start_ms: 0, end_ms: 12_000 });
        let list = vec![track(0, true)];
        let args = arguments(
            &plan,
            Path::new("C:/clips/x.mp4"),
            &list,
            &[],
            Path::new("C:/clips/x.neu.mp4"),
            EncoderId::X264,
            40_000,
        );
        let last_input = args.iter().rposition(|arg| arg == "-i").unwrap();
        let length = args.iter().position(|arg| arg == "-t").unwrap();
        assert!(length > last_input, "{args:?}");
        assert_eq!(args[length + 1], "12.000");
        assert!(args.windows(2).any(|pair| pair == ["-c:v", "copy"]), "{args:?}");
    }

    /// If the audio still sits in the video file it shares that timeline — an
    /// `-ss` of its own would cut it a second time.
    #[test]
    fn audio_inside_the_file_inherits_the_picture_offset() {
        let plan = plan(None, true, 30_000, Trim { start_ms: 4_000, end_ms: 12_000 });
        let list = vec![track(0, false)];
        let args = arguments(
            &plan,
            Path::new("C:/clips/x.mp4"),
            &list,
            &[],
            Path::new("C:/clips/x.neu.mp4"),
            EncoderId::X264,
            40_000,
        );
        assert_eq!(args.iter().filter(|arg| *arg == "-i").count(), 1, "{args:?}");
        assert_eq!(args.iter().filter(|arg| *arg == "-ss").count(), 1, "{args:?}");
        let filter = args.iter().position(|arg| arg == "-filter_complex").unwrap();
        assert!(args[filter + 1].contains("0:a:0"), "{}", args[filter + 1]);
    }

    /// The store must not sit where startup cleans up — otherwise the original
    /// would be gone after the next program start.
    #[test]
    fn originals_live_outside_the_scratch_folders() {
        assert!(!root().starts_with(crate::preview::dir()));
        assert!(!root().starts_with(stems::root()));
        assert!(!root().starts_with(config::data_dir().join("temp")));
        assert!(!root().starts_with(config::data_dir().join("buffer")));
    }

    #[test]
    fn timestamps_become_milliseconds() {
        assert_eq!(parse_timestamp_ms("00:00:01.500000"), Some(1_500));
        assert_eq!(parse_timestamp_ms("01:02:03.000000"), Some(3_723_000));
        assert_eq!(parse_timestamp_ms("kaputt"), None);
    }

    /// x264 is the fallback when the hardware refuses — it always has to produce
    /// a complete command line.
    #[test]
    fn every_encoder_yields_a_complete_line() {
        for encoder in [EncoderId::Nvenc, EncoderId::Amf, EncoderId::Qsv, EncoderId::X264] {
            let args = video_args(encoder, 100);
            assert!(args.contains(&"-c:v".to_string()), "{encoder:?}: {args:?}");
            assert!(args.contains(&"-pix_fmt".to_string()), "{encoder:?}: {args:?}");
        }
    }
}
