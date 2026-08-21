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

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use crate::pipeline::ClipSnapshot;
use crate::stems;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Folder holding the bundled programs — in the installed package that is the
/// app's resource folder. Set once at startup.
static TOOL_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Where to find ffmpeg and ffprobe. Without a call only PATH is searched.
pub fn set_tool_dir(dir: PathBuf) {
    let _ = TOOL_DIR.set(dir);
}

/// The bundled program, otherwise the name for the PATH search.
///
/// Bundled deliberately beats PATH: some ffmpeg version that happens to be
/// installed may have different defaults, and the one shipped alongside is the
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
pub fn available() -> bool {
    ffmpeg()
        .arg("-version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

pub struct ClipRequest {
    pub snapshot: ClipSnapshot,
    /// The id the clip will land under in the database — the individual tracks
    /// are filed by it.
    pub clip_id: String,
    pub output: PathBuf,
    pub temp_dir: PathBuf,
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

    std::fs::create_dir_all(&request.temp_dir).map_err(|e| e.to_string())?;
    if let Some(parent) = request.output.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // Derived from the target clip and therefore unique per save: two concurrent
    // saves must not write each other's intermediate files out from under them.
    let stem = sanitize(&request.output.file_stem().unwrap_or_default().to_string_lossy());

    // Video packets as a raw H.264 elementary stream.
    let video_path = request.temp_dir.join(format!("clip_{stem}.h264"));
    let mut stream: Vec<u8> = Vec::with_capacity(
        snapshot.packets.iter().map(|p| p.data.len()).sum::<usize>()
            + snapshot.sequence_header.len(),
    );
    // Prepend SPS/PPS. Most encoders send them before every IDR anyway — any
    // decoder skips a duplicate, but they must not be missing entirely.
    stream.extend_from_slice(&snapshot.sequence_header);
    for packet in &snapshot.packets {
        stream.extend_from_slice(&packet.data);
    }
    std::fs::write(&video_path, &stream).map_err(|e| e.to_string())?;

    // Audio tracks as WAV, cut to the same QPC as the first frame.
    let mut wavs: Vec<(PathBuf, String)> = Vec::new();
    for track in &snapshot.tracks {
        let path = request
            .temp_dir
            .join(format!("track_{stem}_{}.wav", sanitize(&track.source_id)));
        // Silently dropping a track would be the worst outcome: the clip would
        // simply be mute with nothing anywhere saying why.
        match track.write_wav_window(&path, snapshot.start_100ns, snapshot.audio_frames) {
            Ok(()) => wavs.push((path, track.label())),
            Err(err) => log::warn!(
                "could not write audio track '{}': {err}",
                track.label()
            ),
        }
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
    // The elementary stream carries no timestamps — the frame rate supplies them.
    // It is exact because the clock generates true CFR.
    command.arg("-f").arg("h264").arg("-r").arg(snapshot.fps.to_string());
    command.arg("-i").arg(&video_path);
    for (path, _) in &wavs {
        command.arg("-i").arg(path);
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
    command.arg(&request.output);

    // Further outputs: the individual tracks, so the mix can still be changed
    // later. With only one track that would be a copy of the clip's audio track —
    // and that one will do.
    let keep_stems = wavs.len() > 1;
    let stems_dir = stems::dir(&request.clip_id);
    if keep_stems {
        std::fs::create_dir_all(&stems_dir).map_err(|e| e.to_string())?;
        for index in 1..=wavs.len() {
            command.arg("-map").arg(format!("{index}:a"));
            command.arg("-c:a").arg("aac").arg("-b:a").arg("192k");
            command.arg("-movflags").arg("+faststart");
            command.arg(stems::track_path(&request.clip_id, index as u32 - 1));
        }
    }

    let outcome = run(&mut command, "Clip schreiben");
    let _ = std::fs::remove_file(&video_path);
    for (path, _) in &wavs {
        let _ = std::fs::remove_file(path);
    }
    if let Err(err) = outcome {
        // Half-written individual tracks would be worse than none at all: the
        // editor would take them for complete and mix from them.
        if keep_stems {
            stems::remove(&request.clip_id);
        }
        return Err(err);
    }

    if keep_stems {
        let labels: Vec<String> = wavs.iter().map(|(_, label)| label.clone()).collect();
        if let Err(err) = stems::write_index(&request.clip_id, &labels) {
            log::warn!("track index not written: {err}");
            stems::remove(&request.clip_id);
        }
    }

    let size_bytes = std::fs::metadata(&request.output)
        .map(|meta| meta.len())
        .unwrap_or(0);
    if size_bytes == 0 {
        return Err("The clip came out empty.".into());
    }

    let duration_ms = probe_duration_ms(&request.output)
        .unwrap_or(snapshot.audio_frames as u64 * 1000 / 48_000);
    let thumb_path = crate::thumbs::make(&request.output, &request.clip_id).ok();

    Ok(ClipResult {
        path: request.output,
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

pub fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

pub fn probe_duration_ms(path: &Path) -> Option<u64> {
    let output = command("ffprobe")
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
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let seconds: f64 = text.trim().parse().ok()?;
    Some((seconds * 1000.0) as u64)
}
