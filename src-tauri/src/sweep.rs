//! What a save, an edit or an export cut short left behind.
//!
//! Every one of them writes beside its target first and only renames at the
//! end, and clears its scratch files itself on the normal paths. A crash, a
//! kill, an update restart or the power going leaves them where they were —
//! as big as the clip, in the user's folder, for good. At start they go:
//!
//! - in the clip folder and up to two levels below (`<clip folder>/<game>/Videos`,
//!   as deep as `filing` ever puts a clip): a save's `<clip id>.part.*` (and
//!   the `<stem>_<clip id>.part.*` of versions before), an edit's
//!   `<clip>.<pid>.neu.mp4` and an export's `<name>.<pid>.clippiboy-export.mp4`;
//!   and the empty file a screenshot claims its name with
//!   (`<game or shot>_<stamp>.png`, see `state::claim_file`) when the picture
//!   never got into it — 0 bytes is never a picture;
//! - in the originals store: a cam the save did not finish (`cam.part.*`) and
//!   a picture an edit did not finish (`<clip id>.part.png`);
//! - in the folder of Discord copies: an export's part file, as above, and
//!   copies more than a week old ([`COPY_AGE`]);
//! - in the same folders: a received clip's `.clippiboy-<offer>.part`
//!   (see `share::sweep_parts`);
//! - in the stem store: a clip's folder with neither an index nor a clip.
//!
//! Only names given out here, and only files from well before the start
//! ([`CLOCK_MARGIN`]) — a save begun meanwhile keeps its own.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tauri::Manager;

use crate::state::AppState;

/// How old a stem store without an index must be before it counts as left
/// over: a share import writes stems before its clip has a row.
const STORE_GRACE: Duration = Duration::from_secs(10 * 60);

/// How long a Discord copy is kept. It is only a hand-off to the clipboard —
/// `export_for_discord` makes a fresh one when it is needed again — and out of
/// sight in the data folder nobody would ever clear it.
const COPY_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How much earlier than the start a file must have been written to count as
/// left over. A clip folder on a network drive is stamped by the server's
/// clock, and a NAS a little behind made a save begun during the sweep look
/// older than the start. A crash's leftovers are far older than this.
const CLOCK_MARGIN: Duration = Duration::from_secs(2 * 60);

/// What counts as "before the start" for a file written at `started`.
fn cutoff(started: SystemTime) -> SystemTime {
    started.checked_sub(CLOCK_MARGIN).unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Clear away the leftovers, on a thread of its own: a clip folder on a share
/// that is offline must not hold up the start until the network gives up.
pub fn run(app: &tauri::AppHandle) {
    let started = SystemTime::now();
    let before_start = cutoff(started);
    let clip_dir = PathBuf::from(app.state::<AppState>().config_snapshot().clip_dir);
    let app = app.clone();
    std::thread::spawn(move || {
        // The data folder first: it is local, the clip folder may not be —
        // though the stores do ask it which recordings wait to be finished.
        for path in cam_partials(&crate::edit::root(), before_start) {
            remove_file(&path, before_start);
        }
        // A Discord copy is written beside where it goes, like any export.
        let discord = crate::commands::discord_root();
        for path in leftovers_in(&discord, before_start) {
            remove_file(&path, before_start);
        }
        let before = started.checked_sub(COPY_AGE).unwrap_or(SystemTime::UNIX_EPOCH);
        for path in aged_copies(&discord, before) {
            let removed = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            match removed {
                Ok(()) => log::info!("removed the old Discord copy '{}'", path.display()),
                Err(err) => log::warn!("could not remove '{}': {err}", path.display()),
            }
        }
        if let Some(mut known) = clip_ids(&app) {
            // A recording that is finished after the fact writes its stems
            // under its own id before it has a row.
            known.extend(recording_ids(&clip_dir));
            let before = started.checked_sub(STORE_GRACE).unwrap_or(SystemTime::UNIX_EPOCH);
            for dir in orphan_stores(&crate::stems::root(), &known, before) {
                match std::fs::remove_dir_all(&dir) {
                    Ok(()) => log::info!("removed the tracks '{}' of a save cut short", dir.display()),
                    Err(err) => log::warn!("could not remove '{}': {err}", dir.display()),
                }
            }
        }
        for path in leftovers_in(&clip_dir, before_start) {
            remove_file(&path, before_start);
        }
        // A receive cut short leaves `.clippiboy-<offer>.part` behind.
        let parts = crate::share::sweep_parts(&clip_dir, STORE_GRACE);
        if parts > 0 {
            log::info!("removed {parts} part file(s) of a receive cut short");
        }
    });
}

/// Removes a leftover found a moment ago — asked once more first: a save,
/// a recovery or a screenshot may have written to that name since it was
/// listed, and a newer or no longer empty file is theirs.
fn remove_file(path: &Path, before: SystemTime) {
    if !still_left_over(path, before) {
        log::info!("'{}' was written to since the start; left alone", path.display());
        return;
    }
    match std::fs::remove_file(path) {
        Ok(()) => log::info!("removed '{}' from a save cut short", path.display()),
        Err(err) => log::warn!("could not remove '{}': {err}", path.display()),
    }
}

/// Every clip's id as its stores are named. `None` when the library cannot
/// say — then no store counts as an orphan.
fn clip_ids(app: &tauri::AppHandle) -> Option<HashSet<String>> {
    let state = app.state::<AppState>();
    let ids = match state.library.lock().as_ref()?.ids() {
        Ok(ids) => ids,
        Err(err) => {
            log::warn!("stem stores not checked: {err}");
            return None;
        }
    };
    Some(ids.iter().map(|id| crate::muxer::sanitize(id)).collect())
}

/// The ids of the recordings waiting in the clip folder to be finished.
fn recording_ids(clip_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(clip_dir.join(crate::recorder::WORK_DIR)) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| crate::muxer::sanitize(&entry.file_name().to_string_lossy()))
        .collect()
}

fn still_left_over(path: &Path, before: SystemTime) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    modified_before(path, before) && (is_scratch_name(&name) || is_empty_file(path))
}

fn is_empty_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() == 0)
}

/// The name a screenshot claims (`state::destination`, `state::claim_file`):
/// `<anything>_<YYYY-MM-DD_HH-MM-SS-mmm>.png`, with ` (<n>)` before the
/// extension when that was taken. Only ever swept when the file is empty.
pub fn is_screenshot_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".png") else {
        return false;
    };
    let stem = match stem.strip_suffix(')').and_then(|rest| rest.rsplit_once(" (")) {
        Some((rest, n)) if is_pid(n) => rest,
        Some(_) => return false,
        None => stem,
    };
    let Some(at) = stem.len().checked_sub(23) else {
        return false;
    };
    let Some(stamp) = stem.get(at..) else {
        return false;
    };
    let stamp_like = stamp.char_indices().all(|(at, c)| match at {
        4 | 7 | 13 | 16 | 19 => c == '-',
        10 => c == '_',
        _ => c.is_ascii_digit(),
    });
    stamp_like && at > 0 && stem.as_bytes()[at - 1] == b'_'
}

fn modified_before(path: &Path, before: SystemTime) -> bool {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .is_ok_and(|at| at < before)
}

/// A name only a save, an edit or an export gives out.
pub fn is_scratch_name(name: &str) -> bool {
    if let Some((head, tail)) = name.split_once(".part.") {
        let kind = match tail.strip_suffix(".wav") {
            Some(track) => !track.is_empty() && track.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            None => tail == "ts" || tail == "h264" || tail == "mp4" || tail == "png",
        };
        return kind && ends_in_clip_id(head);
    }
    // `<target stem>.<pid>.<what>.mp4`, see `edit.rs`, `commands::export_part`.
    // The pid as Windows gives it out — up to ten digits, no leading zero — so
    // a "Holiday.2019.neu.mp4" of the user's has to be very unlucky.
    [".neu.mp4", ".clippiboy-export.mp4"].iter().any(|suffix| {
        name.strip_suffix(suffix)
            .and_then(|rest| rest.rsplit_once('.'))
            .is_some_and(|(_, pid)| is_pid(pid))
    })
}

fn is_pid(text: &str) -> bool {
    (1..=10).contains(&text.len())
        && !text.starts_with('0')
        && text.bytes().all(|b| b.is_ascii_digit())
}

/// `<clip id>` or `<anything>_<clip id>`, the id sanitized: 8-4-4-4-12 hex
/// digits joined by `_`.
fn ends_in_clip_id(head: &str) -> bool {
    let Some(at) = head.len().checked_sub(36) else {
        return false;
    };
    let Some(id) = head.get(at..) else {
        return false;
    };
    let id_like = id.char_indices().all(|(at, c)| match at {
        8 | 13 | 18 | 23 => c == '_',
        _ => c.is_ascii_hexdigit(),
    });
    id_like && (at == 0 || head.as_bytes()[at - 1] == b'_')
}

/// The scratch files in the clip folder and up to two levels below it,
/// written before `before`. Folders whose name starts with a dot are not
/// entered: `.recording` holds recordings that are picked up again.
pub fn leftovers_in(clip_dir: &Path, before: SystemTime) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut dirs = vec![(clip_dir.to_path_buf(), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if depth < 2 && !name.starts_with('.') {
                    dirs.push((path, depth + 1));
                }
            } else if kind.is_file()
                && (is_scratch_name(&name) || (is_screenshot_name(&name) && is_empty_file(&path)))
                && modified_before(&path, before)
            {
                found.push(path);
            }
        }
    }
    found
}

/// `cam.part.h264` and `cam.part.mp4` in a clip's folder of the originals
/// store, written before `before`. `facecam.json` is written only after
/// `cam.mp4`, so a part file there is never a finished cam. Also a picture
/// staged beside `base.png` or `under.png` (`<clip id>.part.png`,
/// `shot::write_png`) and never renamed into place.
pub fn cam_partials(originals: &Path, before: SystemTime) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(originals) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        for name in ["cam.part.h264", "cam.part.mp4"] {
            let path = entry.path().join(name);
            if path.is_file() && modified_before(&path, before) {
                found.push(path);
            }
        }
        let Ok(files) = std::fs::read_dir(entry.path()) else {
            continue;
        };
        for file in files.flatten() {
            let name = file.file_name().to_string_lossy().to_string();
            let path = file.path();
            if name.ends_with(".part.png")
                && is_scratch_name(&name)
                && file.file_type().is_ok_and(|kind| kind.is_file())
                && modified_before(&path, before)
            {
                found.push(path);
            }
        }
    }
    found
}

/// Folders of the stem store that belong to no clip in `known` (sanitized
/// ids), have neither `tracks.json` nor `spuren.json`, and were last changed
/// before `before`: the tracks of a save that never got as far as its index.
pub fn orphan_stores(root: &Path, known: &HashSet<String>, before: SystemTime) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| !known.contains(entry.file_name().to_string_lossy().as_ref()))
        .map(|entry| entry.path())
        .filter(|dir| !dir.join("tracks.json").exists() && !dir.join("spuren.json").exists())
        .filter(|dir| modified_before(dir, before))
        .collect()
}

/// The Discord copies under `root` last changed before `before`: a clip's
/// whole folder when everything in it is that old (an empty one by its own
/// time), otherwise the old copies in it one by one.
pub fn aged_copies(root: &Path, before: SystemTime) -> Vec<PathBuf> {
    let Ok(folders) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for folder in folders.flatten() {
        if !folder.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(folder.path()) else {
            continue;
        };
        let entries: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        let old: Vec<PathBuf> = entries
            .iter()
            .filter(|path| modified_before(path, before))
            .cloned()
            .collect();
        if old.len() == entries.len() && (!entries.is_empty() || modified_before(&folder.path(), before)) {
            found.push(folder.path());
        } else {
            found.extend(old.into_iter().filter(|path| path.is_file()));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "1b4e28ba_2fa1_11d2_883f_0016d3cca427";

    fn scratch_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("clippiboy-sweep-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn old_and_new_scratch_names_are_both_recognised() {
        for name in [
            format!("Valorant_2026-10-22_12-00-00-123_{ID}.part.h264"),
            format!("Valorant_2026-10-22_12-00-00-123_{ID}.part.mix.wav"),
            format!("{ID}.part.mp4"),
            format!("{ID}.part.ts"),
            format!("{ID}.part.png"),
            format!("Valorant_2026-10-22_12-00-00-123_{ID}.part.ts"),
            format!("{ID}.part.game_1.wav"),
            "Valorant_2026-10-22_12-00-00-123.4711.neu.mp4".into(),
            "Valorant_2026-10-22_12-00-00-123.4294967295.neu.mp4".into(),
            "Valorant_2026-10-22_12-00-00-123 (10 MB).4711.clippiboy-export.mp4".into(),
            crate::muxer::scratch_name("1b4e28ba-2fa1-11d2-883f-0016d3cca427", "mix.wav"),
        ] {
            assert!(is_scratch_name(&name), "{name}");
        }
        for name in [
            "Valorant_2026-10-22_12-00-00-123.mp4",
            "Valorant_2026-10-22_12-00-00-123 (10 MB).mp4",
            "holiday.part.mp4",
            "my.neu.mp4",
            "my.cam.mp4",
            "notes.part.wav",
            // A user's own files, named the way people name them.
            "Holiday.2019.export.mp4",
            "x.0001.export.mp4",
            "my.2023.cam.mp4",
            "Valorant_2026-10-22_12-00-00-123.4711.cam.mp4",
            "Valorant_2026-10-22_12-00-00-123 (10 MB).4711.export.mp4",
            "x.12345678901.neu.mp4",
            "x..clippiboy-export.mp4",
            "x.4711.clippiboy-export.mkv",
        ] {
            assert!(!is_scratch_name(name), "{name}");
        }
    }

    #[test]
    fn a_screenshot_cut_short_is_swept() {
        assert!(is_scratch_name(&crate::muxer::scratch_name(
            "0b8f2a4e-1c2d-4e5f-8a9b-0c1d2e3f4a5b",
            "png"
        )));
    }

    /// What the first sweep (in `muxer`, up to this one) turned down.
    #[test]
    fn names_that_only_look_like_ours_are_left_alone() {
        for name in [
            format!("Valorant_2026_10_20_{ID}.mp4"),
            format!("clip_{ID}.part.mkv"),
            format!("clip_{ID}.part.tsv"),
            format!("clip_{ID}.part..wav"),
            format!("clip-{ID}.part.h264"),
            "clip_not_a_uuid_at_all_but_36_chars_x.part.h264".to_owned(),
            "clip.mp4.part".to_owned(),
            "clip.part.export.mp4".to_owned(),
        ] {
            assert!(!is_scratch_name(&name), "{name}");
        }
    }

    #[test]
    fn leftovers_are_found_two_levels_down_and_only_from_before_the_start() {
        let dir = scratch_dir();
        let videos = dir.join("Valorant").join("Videos");
        std::fs::create_dir_all(videos.join("deeper")).unwrap();
        let top = dir.join(format!("{ID}.part.mp4"));
        let nested = videos.join(format!("Valorant_2026-10-22_12-00-00-123_{ID}.part.h264"));
        let too_deep = videos.join("deeper").join(format!("{ID}.part.mp4"));
        let clip = videos.join("Valorant_2026-10-22_12-00-00-123.mp4");
        for file in [&top, &nested, &too_deep, &clip] {
            std::fs::write(file, b"x").unwrap();
        }
        let mut found = leftovers_in(&dir, SystemTime::now() + Duration::from_secs(60));
        found.sort();
        let mut expected = vec![top, nested];
        expected.sort();
        assert_eq!(found, expected);
        assert!(
            leftovers_in(&dir, SystemTime::UNIX_EPOCH).is_empty(),
            "a file written since the start may be a save in progress"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Ledger rest T10: a crash between claiming a screenshot's name and
    /// writing the picture left a 0-byte "picture" in the user's folder.
    #[test]
    fn an_empty_screenshot_claim_is_swept_and_a_picture_is_not() {
        let dir = scratch_dir();
        let game = dir.join("Valorant").join("Screenshots");
        std::fs::create_dir_all(&game).unwrap();
        let claim = dir.join("shot_2026-10-24_12-00-00-123.png");
        let second = game.join("Valorant_2026-10-24_12-00-00-123 (2).png");
        let picture = dir.join("shot_2026-10-24_12-00-00-456.png");
        let users = dir.join("empty.png");
        std::fs::write(&claim, b"").unwrap();
        std::fs::write(&second, b"").unwrap();
        std::fs::write(&picture, b"\x89PNG").unwrap();
        std::fs::write(&users, b"").unwrap();
        let mut found = leftovers_in(&dir, SystemTime::now() + Duration::from_secs(60));
        found.sort();
        let mut expected = vec![claim, second];
        expected.sort();
        assert_eq!(found, expected);
        assert!(leftovers_in(&dir, SystemTime::UNIX_EPOCH).is_empty(), "a screenshot being taken right now");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn screenshot_names_are_told_apart() {
        for name in [
            "shot_2026-10-24_12-00-00-123.png",
            "Valorant_2026-10-24_12-00-00-123.png",
            "shot_2026-10-24_12-00-00-123 (2).png",
            "My_Game_2026-10-24_12-00-00-123 (1000).png",
        ] {
            assert!(is_screenshot_name(name), "{name}");
        }
        for name in [
            "2026-10-24_12-00-00-123.png",
            "shot_2026-10-24_12-00-00-123.mp4",
            "shot_2026-10-24_12-00-00-12.png",
            "shot_2026-10-24 12-00-00-123.png",
            "shot_2026-10-24_12-00-00-123 (x).png",
            "shot_2026-10-24_12-00-00-123 (02).png",
            "holiday.png",
        ] {
            assert!(!is_screenshot_name(name), "{name}");
        }
    }

    /// Final review: a NAS whose clock runs behind stamps a save begun
    /// during the sweep as older than the start.
    #[test]
    fn a_file_from_just_before_the_start_is_left_alone() {
        let dir = scratch_dir();
        let part = dir.join(format!("{ID}.part.mp4"));
        std::fs::write(&part, b"x").unwrap();
        age(&part, Duration::from_secs(60));
        assert!(leftovers_in(&dir, cutoff(SystemTime::now())).is_empty());
        age(&part, Duration::from_secs(10 * 60));
        assert_eq!(leftovers_in(&dir, cutoff(SystemTime::now())), vec![part.clone()]);

        // Written to again between the listing and the removal.
        std::fs::write(&part, b"y").unwrap();
        remove_file(&part, cutoff(SystemTime::now()));
        assert!(part.is_file(), "a file written since it was listed is kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_recording_in_progress_is_never_entered() {
        let dir = scratch_dir();
        let work = dir.join(crate::recorder::WORK_DIR).join("rec");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join(format!("{ID}.part.h264")), b"x").unwrap();
        assert!(leftovers_in(&dir, SystemTime::now() + Duration::from_secs(60)).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clip_folder_that_is_not_there_has_no_leftovers() {
        let missing = std::env::temp_dir().join("clippiboy-not-there").join("at-all");
        assert!(leftovers_in(&missing, SystemTime::now()).is_empty());
    }

    #[test]
    fn a_half_written_cam_is_found_in_the_originals_store() {
        let root = scratch_dir();
        let store = root.join(ID);
        std::fs::create_dir_all(&store).unwrap();
        let picture = format!("{ID}.part.png");
        for name in ["cam.part.h264", "cam.part.mp4", "cam.mp4", "video.mp4", "facecam.json", "base.png", &picture] {
            std::fs::write(store.join(name), b"x").unwrap();
        }
        let mut found = cam_partials(&root, SystemTime::now() + Duration::from_secs(60));
        found.sort();
        let mut expected = vec![store.join("cam.part.h264"), store.join("cam.part.mp4"), store.join(&picture)];
        expected.sort();
        assert_eq!(found, expected);
        assert!(cam_partials(&root, SystemTime::UNIX_EPOCH).is_empty(), "a save writing its cam right now");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn age(path: &Path, by: Duration) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - by).unwrap();
    }

    #[test]
    fn discord_copies_go_once_they_have_aged_out() {
        let root = scratch_dir();
        let week = Duration::from_secs(7 * 24 * 60 * 60);
        let day = Duration::from_secs(24 * 60 * 60);
        for name in ["old", "fresh", "mixed"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        let old = root.join("old").join("Valorant (10 MB).mp4");
        let fresh = root.join("fresh").join("Valorant (10 MB).mp4");
        let stale = root.join("mixed").join("Valorant (10 MB).mp4");
        let newer = root.join("mixed").join("Valorant (25 MB).mp4");
        for file in [&old, &fresh, &stale, &newer] {
            std::fs::write(file, b"x").unwrap();
        }
        age(&old, week + day);
        age(&stale, week + day);
        age(&newer, day);
        let before = SystemTime::now() - week;
        let mut found = aged_copies(&root, before);
        found.sort();
        let mut expected = vec![root.join("old"), stale];
        expected.sort();
        assert_eq!(found, expected, "the old folder whole, an old copy beside a fresh one alone");
        assert!(aged_copies(&root, SystemTime::UNIX_EPOCH).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_incomplete_stores_without_a_clip_are_orphans() {
        let root = scratch_dir();
        for name in ["known", "orphan", "indexed", "legacy"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        std::fs::write(root.join("indexed").join("tracks.json"), b"{}").unwrap();
        std::fs::write(root.join("legacy").join("spuren.json"), b"{}").unwrap();
        let known = HashSet::from(["known".to_string()]);
        let later = SystemTime::now() + Duration::from_secs(60);
        assert_eq!(orphan_stores(&root, &known, later), vec![root.join("orphan")]);
        assert!(orphan_stores(&root, &known, SystemTime::UNIX_EPOCH).is_empty(), "a store being written right now");
        let _ = std::fs::remove_dir_all(&root);
    }
}
