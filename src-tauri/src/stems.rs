//! A clip's individual tracks ("stems").
//!
//! The clip file itself has exactly **one** audio track with everything in it —
//! game audio, microphone, Discord. That is the only way you really hear
//! everything in Discord, in a browser and in any player: most of them play the
//! first audio track of an MP4 and nothing else. Previously the sources sat as
//! extra tracks in the same file and were therefore silent everywhere except in
//! the editor.
//!
//! So the mix can still be changed at any time, the raw individual tracks live
//! alongside here — one folder per clip in the data directory:
//!
//! ```text
//! <data>/tracks/<clip-id>/0.m4a
//!                        /1.m4a
//!                        /tracks.json
//! ```
//!
//! The clip folder belongs to the user and stays free of helper files.

use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::config;
use crate::model::{ClipTrack, TrackMix};
use crate::muxer::{command, ffmpeg, run, sanitize};

/// Output label of the mix chain from [`mix_filter`].
pub const MIX_LABEL: &str = "[aout]";

/// From here on a track counts as silent — below this it cannot be heard
/// anyway, and ffmpeg need not drag it along at all.
const SILENT_DB: f32 = -59.0;

/// Root of all stems. **Permanent** — unlike the preview folder, which is
/// cleared at startup.
pub fn root() -> PathBuf {
    config::data_dir().join("tracks")
}

/// A clip's folder.
pub fn dir(clip_id: &str) -> PathBuf {
    root().join(sanitize(clip_id))
}

/// File of a single track.
pub fn track_path(clip_id: &str, index: u32) -> PathBuf {
    dir(clip_id).join(format!("{index}.m4a"))
}

fn index_path(clip_id: &str) -> PathBuf {
    dir(clip_id).join("tracks.json")
}

/// The index name used up to 0.1.7. Read-only: a store written back then stays
/// usable, and the next `write_index` puts the current name next to it.
fn legacy_index_path(clip_id: &str) -> PathBuf {
    dir(clip_id).join("spuren.json")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    index: u32,
    label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Index {
    tracks: Vec<Entry>,
}

/// Only one extraction at a time.
///
/// The player asks for the tracks when it opens, and in development mode it does
/// so twice because of `React.StrictMode`. Without this lock two ffmpeg runs
/// worked on the same target files at once — what came out were files that
/// reported the right length but could not be decoded ("channel element 0.0
/// duplicate"). Saving then had ffmpeg abort halfway through.
static EXTRACTING: Mutex<()> = Mutex::new(());

/// Store the track names. The file alone only yields the number, and "1.m4a"
/// tells nobody that the microphone is in it.
pub fn write_index(clip_id: &str, labels: &[String]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir(clip_id))?;
    let index = Index {
        tracks: labels
            .iter()
            .enumerate()
            .map(|(index, label)| Entry {
                index: index as u32,
                label: label.clone(),
            })
            .collect(),
    };
    let text = serde_json::to_string_pretty(&index)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    // The index is the signal "everything here is complete" — which is why it is
    // written last and in one go.
    let temp = index_path(clip_id).with_extension("part");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, index_path(clip_id))
}

/// The stored tracks as a track list, provided they are complete.
fn from_index(clip_id: &str) -> Option<Vec<ClipTrack>> {
    Some(
        read_index(clip_id)?
            .into_iter()
            .map(|entry| ClipTrack {
                index: entry.index,
                label: entry.label,
                channels: 2,
                preview_path: Some(
                    track_path(clip_id, entry.index).to_string_lossy().to_string(),
                ),
            })
            .collect(),
    )
}

/// Stored tracks, provided they are complete.
fn read_index(clip_id: &str) -> Option<Vec<Entry>> {
    let text = std::fs::read_to_string(index_path(clip_id))
        .or_else(|_| std::fs::read_to_string(legacy_index_path(clip_id)))
        .ok()?;
    let index: Index = serde_json::from_str(&text).ok()?;
    // If even one file is missing the store is unusable — better to pull fresh
    // from the clip file than to offer the editor a dead track.
    index
        .tracks
        .iter()
        .all(|entry| track_path(clip_id, entry.index).is_file())
        .then_some(index.tracks)
}

/// Clear away everything belonging to a clip.
pub fn remove(clip_id: &str) {
    let _ = std::fs::remove_dir_all(dir(clip_id));
}

/// A clip's tracks — from the store, otherwise pulled out of `source`.
///
/// For clips from before the changeover the sources still sit as extra audio
/// tracks in the MP4. Those are extracted once here; from then on the store is
/// the source.
///
/// `source` is deliberately not `clip.path`: as soon as a clip is trimmed, the
/// untouched recording lies in the originals store, and only that may be
/// extracted here — otherwise the store itself would be trimmed and the preview
/// would run offset by the trim. [`crate::edit::source_path`] supplies the right
/// path.
pub fn tracks(clip_id: &str, source: &Path) -> Result<Vec<ClipTrack>, String> {
    if let Some(tracks) = from_index(clip_id) {
        return Ok(tracks);
    }

    // From here on we write, so one at a time. Whoever waited finds the store
    // finished afterwards and has nothing left to do.
    let _busy = EXTRACTING.lock();
    if let Some(tracks) = from_index(clip_id) {
        return Ok(tracks);
    }

    let in_file = tracks_in_file(source)?;
    if in_file.len() <= 1 {
        // One track: the clip file's audio track *is* the original, a copy
        // beside it would only be double the space. Without `previewPath` the
        // player plays it straight out of the video.
        return Ok(in_file);
    }

    // Legacy: pull the tracks out of the MP4 into the store.
    let mut extracted = Vec::with_capacity(in_file.len());
    for track in &in_file {
        let path = extract(clip_id, source, track.index)?;
        extracted.push(ClipTrack {
            preview_path: Some(path.to_string_lossy().to_string()),
            ..track.clone()
        });
    }
    let labels: Vec<String> = extracted.iter().map(|t| t.label.clone()).collect();
    write_index(clip_id, &labels).map_err(|err| err.to_string())?;
    Ok(extracted)
}

/// A clip file's audio tracks, in the file's own order.
pub fn tracks_in_file(source: &Path) -> Result<Vec<ClipTrack>, String> {
    let output = command("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a",
            "-show_entries",
            "stream=index,channels:stream_tags",
            "-of",
            "json",
        ])
        .arg(source)
        .output()
        .map_err(|err| format!("could not start ffprobe: {err}"))?;
    if !output.status.success() {
        return Err("Could not read the clip's audio tracks.".into());
    }

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|err| err.to_string())?;
    let streams = json
        .get("streams")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();

    Ok(streams
        .iter()
        .enumerate()
        .map(|(index, stream)| ClipTrack {
            index: index as u32,
            label: label_of(stream, index),
            channels: stream.get("channels").and_then(|c| c.as_u64()).unwrap_or(2) as u32,
            preview_path: None,
        })
        .collect())
}

/// Meaningless defaults that ffmpeg and other tools write as the handler into
/// every MP4 file.
const GENERIC_LABELS: [&str; 4] = [
    "soundhandler",
    "videohandler",
    "gpac iso audio handler",
    "mainconcept mp4 sound media handler",
];

/// Track name from the metadata. Where a title ends up in an MP4 file depends on
/// the writing tool: ffmpeg stores it as `name` but also reads it from `title` or
/// `handler_name`.
fn label_of(stream: &serde_json::Value, index: usize) -> String {
    ["title", "name", "handler_name"]
        .iter()
        .filter_map(|key| {
            stream
                .pointer(&format!("/tags/{key}"))
                .and_then(|value| value.as_str())
                .map(str::trim)
        })
        .find(|value| {
            !value.is_empty() && !GENERIC_LABELS.contains(&value.to_ascii_lowercase().as_str())
        })
        .map(String::from)
        .unwrap_or_else(|| match index {
            0 => "Main mix".to_string(),
            other => format!("Track {}", other + 1),
        })
}

/// Pull one audio track out of `source` into the store.
fn extract(clip_id: &str, source: &Path, index: u32) -> Result<PathBuf, String> {
    let out = track_path(clip_id, index);
    std::fs::create_dir_all(dir(clip_id)).map_err(|e| e.to_string())?;
    // Write beside the target first, then move: a half file must never lie under
    // the target name. One like that reports the right length and only shows up
    // when somebody tries to decode it.
    let temp = dir(clip_id).join(format!("{index}.{}.teil", std::process::id()));

    let mut copy = ffmpeg();
    copy.args(["-y", "-hide_banner", "-loglevel", "error"])
        .arg("-i")
        .arg(source)
        .args(["-map", &format!("0:a:{index}"), "-c:a", "copy"])
        .args(["-movflags", "+faststart"])
        .arg("-f")
        .arg("mp4")
        .arg(&temp);
    let mut outcome = run(&mut copy, "Tonspur entpacken");

    if outcome.is_err() {
        // If the track is in a format an MP4 will not take (PCM, say), only
        // re-encoding helps.
        let mut encode = ffmpeg();
        encode
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .arg("-i")
            .arg(source)
            .args(["-map", &format!("0:a:{index}"), "-c:a", "aac", "-b:a", "192k"])
            .args(["-movflags", "+faststart"])
            .arg("-f")
            .arg("mp4")
            .arg(&temp);
        outcome = run(&mut encode, "Tonspur entpacken");
    }

    if let Err(err) = outcome {
        let _ = std::fs::remove_file(&temp);
        return Err(err);
    }
    std::fs::rename(&temp, &out).map_err(|err| {
        let _ = std::fs::remove_file(&temp);
        format!("could not store the audio track: {err}")
    })?;
    Ok(out)
}

/// dB to a linear factor, rounded to three places for the ffmpeg line.
pub fn factor(db: f32) -> f32 {
    (crate::audio::gain_factor(db) * 1000.0).round() / 1000.0
}

/// Filter chain that sums the given inputs at their levels into **one** track
/// under [`MIX_LABEL`].
///
/// `inputs` are ffmpeg stream specifiers like `"1:a"` together with a level in
/// dB. `None` means nothing audible is left over and the clip gets no sound.
///
/// Two sides need this chain — recording, when it merges the freshly mixed
/// tracks, and saving, when it changes the mix later. Which is why it lives here
/// and not twice over there.
pub fn mix_filter(inputs: &[(String, f32)]) -> Option<String> {
    let audible: Vec<&(String, f32)> = inputs
        .iter()
        .filter(|(_, gain_db)| *gain_db > SILENT_DB)
        .collect();
    if audible.is_empty() {
        return None;
    }

    let mut chains: Vec<String> = audible
        .iter()
        .enumerate()
        .map(|(slot, (stream, gain_db))| {
            format!("[{stream}]volume={}[m{slot}]", factor(*gain_db))
        })
        .collect();
    let marks: String = (0..audible.len()).map(|slot| format!("[m{slot}]")).collect();

    if audible.len() > 1 {
        // `normalize=0`, otherwise amix divides the levels by the number of
        // tracks and the finished audio would suddenly be far quieter than
        // before.
        chains.push(format!(
            "{marks}amix=inputs={}:normalize=0:dropout_transition=0{MIX_LABEL}",
            audible.len()
        ));
    } else {
        chains.push(format!("{marks}anull{MIX_LABEL}"));
    }
    Some(chains.join(";"))
}

/// The levels from a stored mix, mapped onto the track list. Tracks with no
/// entry stay untouched.
pub fn levels(tracks: &[ClipTrack], mix: &[TrackMix]) -> Vec<f32> {
    tracks
        .iter()
        .map(|track| {
            match mix.iter().find(|entry| entry.index == track.index) {
                Some(entry) if entry.muted => f32::NEG_INFINITY,
                Some(entry) => entry.gain_db,
                None => 0.0,
            }
        })
        .collect()
}

/// Is `file` newer than `source` — and therefore still valid?
pub fn is_newer(file: &Path, source: &Path) -> bool {
    let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    match (modified(file), modified(source)) {
        (Some(a), Some(b)) => a >= b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The index is the signal "everything here is complete". If it counted
    /// without its files too, the editor would offer dead tracks and saving would
    /// abort halfway through.
    #[test]
    fn an_index_without_its_files_does_not_count() {
        let id = "clippiboy-test-unvollstaendig";
        remove(id);

        write_index(id, &["Mix".into(), "Microphone".into()]).unwrap();
        assert!(from_index(id).is_none(), "an index without tracks must not count");

        for index in 0..2 {
            std::fs::write(track_path(id, index), b"not empty").unwrap();
        }
        let tracks = from_index(id).expect("with files present the index must count");
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[1].label, "Microphone");
        assert!(tracks[0].preview_path.is_some());

        remove(id);
        assert!(from_index(id).is_none(), "remove must clear everything away");
    }

    #[test]
    fn track_labels_come_from_any_of_the_tags() {
        let named = serde_json::json!({ "tags": { "name": "Microphone" } });
        assert_eq!(label_of(&named, 1), "Microphone");

        // ffmpeg writes "SoundHandler" into every MP4 file — that is not a name.
        let generic = serde_json::json!({ "tags": { "handler_name": "SoundHandler" } });
        assert_eq!(label_of(&generic, 0), "Main mix");
        assert_eq!(label_of(&generic, 2), "Track 3");
    }

    #[test]
    fn gain_becomes_a_linear_factor() {
        assert_eq!(factor(0.0), 1.0);
        assert!((factor(-6.0) - 0.501).abs() < 0.01);
    }

    #[test]
    fn a_single_input_needs_no_amix() {
        let filter = mix_filter(&[("1:a".into(), 0.0)]).unwrap();
        assert!(filter.contains("anull"), "{filter}");
        assert!(filter.ends_with(MIX_LABEL));
    }

    #[test]
    fn several_inputs_are_summed_without_normalising() {
        let filter =
            mix_filter(&[("1:a".into(), 0.0), ("2:a".into(), -6.0)]).unwrap();
        assert!(filter.contains("amix=inputs=2"), "{filter}");
        // Without `normalize=0` amix halves the levels and the clip would be
        // quieter than what was heard while playing.
        assert!(filter.contains("normalize=0"), "{filter}");
    }

    /// A silent track must not reach ffmpeg at all — otherwise `amix` mixes it in
    /// at level 0 and only lengthens the chain.
    #[test]
    fn muted_inputs_drop_out() {
        let filter = mix_filter(&[
            ("1:a".into(), 0.0),
            ("2:a".into(), f32::NEG_INFINITY),
        ])
        .unwrap();
        assert!(filter.contains("anull"), "{filter}");
        assert!(!filter.contains("2:a"), "{filter}");
    }

    #[test]
    fn nothing_audible_means_no_track() {
        assert!(mix_filter(&[("1:a".into(), f32::NEG_INFINITY)]).is_none());
        assert!(mix_filter(&[]).is_none());
    }

    #[test]
    fn a_muted_entry_becomes_silence() {
        let tracks = vec![
            ClipTrack { index: 0, label: "Mix".into(), channels: 2, preview_path: None },
            ClipTrack { index: 1, label: "Mic".into(), channels: 2, preview_path: None },
        ];
        let mix = vec![
            TrackMix { index: 0, gain_db: -3.0, muted: false },
            TrackMix { index: 1, gain_db: 0.0, muted: true },
        ];
        assert_eq!(levels(&tracks, &mix), vec![-3.0, f32::NEG_INFINITY]);
    }

    /// Tracks with nothing stored for them stay at full — otherwise a newly
    /// added track would disappear silently.
    #[test]
    fn unknown_tracks_stay_at_unity() {
        let tracks = vec![ClipTrack {
            index: 7,
            label: "Neu".into(),
            channels: 2,
            preview_path: None,
        }];
        assert_eq!(levels(&tracks, &[]), vec![0.0]);
    }
}
