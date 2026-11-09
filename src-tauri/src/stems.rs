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
use crate::model::{AudioSource, Clip, ClipTrack, ProcessMode, SourceKind, TrackMix};
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

/// The stored tracks, provided they are complete — without pulling anything
/// out of the clip file. `None` for a clip with a single track, whose audio
/// lives in the clip itself.
pub fn stored(clip_id: &str) -> Option<Vec<ClipTrack>> {
    from_index(clip_id)
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
    // An empty id names the store itself, with every clip's tracks in it.
    if sanitize(clip_id).is_empty() {
        return;
    }
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
///
/// `keep_single`: a clip with one audio track gets that track stored too, as
/// long as `source` still holds it as recorded ([`crate::edit::audio_untouched`]).
/// Mixed from the clip file instead, every save put its gain on top of the last
/// one and muting deleted the sound for good — the store is what the next save
/// starts from again.
pub fn tracks(clip_id: &str, source: &Path, keep_single: bool) -> Result<Vec<ClipTrack>, String> {
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
    if in_file.is_empty() || (in_file.len() == 1 && !keep_single) {
        // No sound, or one track whose recorded level is gone from the file:
        // without `previewPath` the player plays it straight out of the video,
        // and saving mixes from there.
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
    let mut outcome = run(&mut copy, "extracting an audio track");

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
        outcome = run(&mut encode, "extracting an audio track");
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

/// Can a track at this level still be heard in the mix?
pub fn audible(gain_db: f32) -> bool {
    gain_db > SILENT_DB
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
        .filter(|(_, gain_db)| audible(*gain_db))
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

/// Is this track the voices from Discord?
///
/// A track only knows its name. So: a name that says "Discord", or the name of
/// a source in the settings that records Discord's own process — whatever it
/// was renamed to. The main mix never counts: the game is in it too.
pub fn is_discord(label: &str, sources: &[AudioSource]) -> bool {
    let label = label.trim();
    if label.eq_ignore_ascii_case("Main mix") {
        return false;
    }
    if label.to_ascii_lowercase().contains("discord") {
        return true;
    }
    sources.iter().any(|source| {
        source.label.trim().eq_ignore_ascii_case(label)
            && matches!(&source.kind, SourceKind::Process { exe: Some(exe), mode: ProcessMode::Include, .. }
                if exe.to_ascii_lowercase().starts_with("discord"))
    })
}

/// A clip's sound mixed again from its own tracks, with some left out — for a
/// copy that leaves the PC without them. The clip itself is not touched.
#[derive(Debug, Clone, PartialEq)]
pub struct Remix {
    /// The tracks that stay, each with its level in dB. Empty when nothing
    /// audible is left: the copy then goes without sound.
    pub tracks: Vec<(PathBuf, f32)>,
    /// Where the clip starts inside the tracks — they keep the untouched
    /// recording's time, like everything in the store.
    pub start_ms: u64,
    pub length_ms: u64,
}

impl Remix {
    /// `-ss`, `-t` and `-i` for every track, in order. Per input: an `-ss`
    /// before the first `-i` would cut the picture, not the tracks.
    pub fn inputs(&self) -> Vec<String> {
        let seconds = |ms: u64| format!("{:.3}", ms as f64 / 1000.0);
        let mut args = Vec::new();
        for (path, _) in &self.tracks {
            if self.start_ms > 0 {
                args.extend(["-ss".into(), seconds(self.start_ms)]);
            }
            args.extend(["-t".into(), seconds(self.length_ms)]);
            args.extend(["-i".into(), path.to_string_lossy().to_string()]);
        }
        args
    }

    /// The mix of those inputs under [`MIX_LABEL`], when ffmpeg counts the
    /// first of them as `first_input`. `None`: nothing audible is left.
    pub fn filter(&self, first_input: usize) -> Option<String> {
        let inputs: Vec<(String, f32)> = self
            .tracks
            .iter()
            .enumerate()
            .map(|(slot, (_, gain))| (format!("{}:a", first_input + slot), *gain))
            .collect();
        mix_filter(&inputs)
    }
}

/// `tracks` at the levels of `mix`, without each track `drop` names. `None`
/// when nothing is left out — the clip's own sound will do — or when a track
/// has no file of its own to mix from.
fn kept_tracks(
    tracks: &[ClipTrack],
    mix: &[TrackMix],
    drop: impl Fn(&ClipTrack) -> bool,
) -> Option<Vec<(PathBuf, f32)>> {
    if !tracks.iter().any(&drop) {
        return None;
    }
    let gains = levels(tracks, mix);
    let mut kept = Vec::new();
    for (track, gain) in tracks.iter().zip(gains) {
        let path = PathBuf::from(track.preview_path.as_ref()?);
        if !drop(track) && audible(gain) {
            kept.push((path, gain));
        }
    }
    Some(kept)
}

/// What "Mute Discord voices when sharing" makes of a clip's sound.
#[derive(Debug, Clone, PartialEq)]
pub enum WithoutDiscord {
    /// Nothing to leave out: no separate Discord track — Discord in the main
    /// mix cannot be taken out of it — or a single track with the mix saved
    /// into the file.
    Unchanged,
    /// The sound mixed again without it.
    Remix(Remix),
    /// The clip had tracks of its own, but they are no longer there to mix
    /// from: its sound goes as it is, Discord perhaps with it. Worth saying.
    Impossible,
}

/// The clip's sound without its Discord track (setting "Mute Discord voices
/// when sharing").
///
/// The store first. Without one, a clip from before the store still carries
/// its tracks inside the MP4, and they are pulled out as the editor would.
/// A store folder without a complete index lost its tracks: nothing can be
/// told about it, and that is said rather than passed over.
pub fn without_discord(clip: &Clip, sources: &[AudioSource]) -> WithoutDiscord {
    let edit = clip.edit.as_ref();
    if clip.screenshot || edit.is_some_and(|edit| edit.baked) {
        return WithoutDiscord::Unchanged;
    }
    let tracks = match stored(&clip.id) {
        Some(tracks) => tracks,
        None => {
            let source = crate::edit::source_path(clip);
            let in_file = tracks_in_file(&source).ok().map(|list| list.len());
            match unstored(in_file, dir(&clip.id).exists()) {
                Unstored::Extract => match tracks(&clip.id, &source, false) {
                    Ok(list) if list.len() > 1 && list.iter().all(|t| t.preview_path.is_some()) => list,
                    _ => return WithoutDiscord::Impossible,
                },
                Unstored::Single => return WithoutDiscord::Unchanged,
                Unstored::Lost => return WithoutDiscord::Impossible,
            }
        }
    };
    let mix = edit.map(|edit| edit.tracks.as_slice()).unwrap_or(&[]);
    match kept_tracks(&tracks, mix, |track| is_discord(&track.label, sources)) {
        Some(tracks) => WithoutDiscord::Remix(Remix {
            tracks,
            start_ms: clip.original.map(|original| original.start_ms).unwrap_or(0),
            length_ms: clip.duration_ms,
        }),
        None => WithoutDiscord::Unchanged,
    }
}

/// A clip with no complete store, by what its file holds.
#[derive(Debug, PartialEq)]
enum Unstored {
    /// Its tracks sit inside the MP4 (from before the store): pull them out.
    Extract,
    /// One track: nothing separate was recorded.
    Single,
    /// A store that lost its files, or a file that cannot be read.
    Lost,
}

fn unstored(tracks_in_file: Option<usize>, store_dir: bool) -> Unstored {
    match tracks_in_file {
        Some(n) if n > 1 => Unstored::Extract,
        Some(_) if !store_dir => Unstored::Single,
        _ => Unstored::Lost,
    }
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

    fn process(label: &str, exe: Option<&str>) -> AudioSource {
        AudioSource {
            id: label.to_lowercase(),
            label: label.into(),
            kind: SourceKind::Process { pid: 1, exe: exe.map(String::from), mode: crate::model::ProcessMode::Include },
            enabled: true,
            gain_db: 0.0,
            muted: false,
            solo: false,
            separate_track: true,
        }
    }

    fn track(index: u32, label: &str) -> ClipTrack {
        ClipTrack { index, label: label.into(), channels: 2, preview_path: Some(format!("t/{index}.m4a")) }
    }

    #[test]
    fn discord_is_found_by_its_name_or_its_process() {
        assert!(is_discord("Discord", &[]));
        assert!(is_discord(" discord voice ", &[]));
        assert!(!is_discord("Microphone", &[]));
        assert!(!is_discord("Main mix", &[process("Main mix", Some("Discord.exe"))]));
        // Renamed in the settings: the process still says what it is.
        let sources = [process("Squad", Some("Discord.exe")), process("Music", Some("Spotify.exe"))];
        assert!(is_discord("Squad", &sources));
        assert!(is_discord("squad", &[process("Squad", Some("DiscordPTB.exe"))]));
        assert!(!is_discord("Music", &sources));
        assert!(!is_discord("Squad", &[process("Squad", None)]));
        // Everything but Discord: the track is the rest of the PC, not Discord.
        let mut all_but = process("Everything else", Some("Discord.exe"));
        if let SourceKind::Process { mode, .. } = &mut all_but.kind {
            *mode = crate::model::ProcessMode::Exclude;
        }
        assert!(!is_discord("Everything else", &[all_but]));
    }

    #[test]
    fn a_clip_without_a_store_is_told_apart_by_its_file() {
        assert_eq!(unstored(Some(3), false), Unstored::Extract, "tracks inside the MP4, from before the store");
        assert_eq!(unstored(Some(1), false), Unstored::Single);
        assert_eq!(unstored(Some(0), false), Unstored::Single);
        assert_eq!(unstored(Some(1), true), Unstored::Lost, "a store folder whose tracks are gone");
        assert_eq!(unstored(None, false), Unstored::Lost);
    }

    #[test]
    fn the_discord_track_stays_home_and_the_rest_keeps_its_levels() {
        let tracks = vec![track(0, "Main mix"), track(1, "Microphone"), track(2, "Discord")];
        let mix = vec![
            TrackMix { index: 0, gain_db: -3.0, muted: false },
            TrackMix { index: 1, gain_db: 0.0, muted: true },
        ];
        let kept = kept_tracks(&tracks, &mix, |t| is_discord(&t.label, &[])).unwrap();
        assert_eq!(kept, vec![(PathBuf::from("t/0.m4a"), -3.0)], "muted and Discord tracks both stay out");
    }

    #[test]
    fn without_a_discord_track_nothing_changes() {
        let tracks = vec![track(0, "Main mix"), track(1, "Microphone")];
        assert!(kept_tracks(&tracks, &[], |t| is_discord(&t.label, &[])).is_none());
        // A track only in the clip file cannot be mixed from.
        let mut in_file = vec![track(0, "Main mix"), track(1, "Discord")];
        in_file[0].preview_path = None;
        assert!(kept_tracks(&in_file, &[], |t| is_discord(&t.label, &[])).is_none());
    }

    #[test]
    fn a_remix_cuts_every_track_where_the_clip_starts() {
        let remix = Remix {
            tracks: vec![(PathBuf::from("a.m4a"), 0.0), (PathBuf::from("b.m4a"), -6.0)],
            start_ms: 4_000,
            length_ms: 12_500,
        };
        assert_eq!(
            remix.inputs(),
            ["-ss", "4.000", "-t", "12.500", "-i", "a.m4a", "-ss", "4.000", "-t", "12.500", "-i", "b.m4a"]
                .map(String::from)
                .to_vec()
        );
        let filter = remix.filter(1).unwrap();
        assert!(filter.starts_with("[1:a]volume=1[m0];[2:a]volume="), "{filter}");
        assert!(filter.ends_with(MIX_LABEL));
        let silent = Remix { tracks: Vec::new(), start_ms: 0, length_ms: 1_000 };
        assert!(silent.filter(1).is_none());
        assert!(silent.inputs().is_empty());
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
