//! Where a clip file sits inside the clip folder.
//!
//! One folder per game, favorites in their own, anything without a game stays
//! directly in the clip folder — and in each of those, the kind splits the
//! files once more: `Videos`, `Screenshots` and `Recordings`. The database remains the truth
//! about a clip: the folder is only the order you see in Explorer — inside the
//! app a favorite is still found under its game.
//!
//! Only what lies **in the configured clip folder** is moved. Whoever changes
//! the storage location deliberately leaves their existing clips where they
//! are; nobody tidies those up afterwards.

use std::path::{Path, PathBuf};

use crate::clips::Library;
use crate::model::{Clip, ClipKind};

/// Folder for the clips marked with a heart.
pub const FAVORITES: &str = "Favorites";

/// The bottom level is always the kind, so that a game folder does not mix
/// clips, stills and hour-long recordings.
pub const VIDEOS: &str = "Videos";
pub const SCREENSHOTS: &str = "Screenshots";
pub const RECORDINGS: &str = "Recordings";

/// Characters Windows does not allow in a folder name.
const FORBIDDEN: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Device names Windows keeps for itself — unusable as a folder name.
const RESERVED: [&str; 6] = ["CON", "PRN", "AUX", "NUL", "COM", "LPT"];

/// A folder name gets no longer than this. Game names sometimes come from
/// window titles, and those can be whole sentences.
const MAX_LEN: usize = 60;

/// Turn a game name into a folder name Windows will accept.
///
/// `None` means nothing usable is left of the name — the clip then goes into
/// the clip folder itself, like one with no game at all.
pub fn folder_name(game: &str) -> Option<String> {
    // Invisible characters come out entirely rather than becoming a space: a
    // zero width space sits *inside* a word ("Raid<ZWSP>ers"), and turning it
    // into a space would split the word instead of closing it up. Names reach
    // this point from the window title but also straight from the game field,
    // typed by hand — so the guard belongs here as well.
    let cleaned: String = crate::game::strip_invisibles(game)
        .chars()
        .map(|c| if FORBIDDEN.contains(&c) || (c as u32) < 0x20 { ' ' } else { c })
        .collect();
    let mut name: String = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    name.truncate(
        name.char_indices()
            .nth(MAX_LEN)
            .map(|(at, _)| at)
            .unwrap_or(name.len()),
    );
    // Windows tolerates neither a dot nor a space at the end.
    let name = name.trim_end_matches(['.', ' ']).to_string();
    if name.is_empty() {
        return None;
    }
    // `CON.mp4` cannot be created on Windows, `_CON` can.
    let stem = name.split('.').next().unwrap_or(&name).to_ascii_uppercase();
    let reserved = RESERVED.iter().any(|word| {
        stem == *word
            || (stem.len() == word.len() + 1
                && stem.starts_with(word)
                && stem.ends_with(|c: char| c.is_ascii_digit()))
    });
    Some(if reserved { format!("_{name}") } else { name })
}

/// The folder a clip with this game, this heart and this kind belongs in.
pub fn dir_for(clip_dir: &Path, game: Option<&str>, favorite: bool, kind: ClipKind) -> PathBuf {
    let base = if favorite {
        clip_dir.join(FAVORITES)
    } else {
        match game.and_then(folder_name) {
            Some(name) => clip_dir.join(name),
            None => clip_dir.to_path_buf(),
        }
    };
    base.join(match kind {
        ClipKind::Clip => VIDEOS,
        ClipKind::Screenshot => SCREENSHOTS,
        ClipKind::Recording => RECORDINGS,
    })
}

/// Is the file in the clip folder — in it, or up to two levels down?
///
/// Two, because the deepest a file of ours ever goes is
/// `<clip folder>/<game>/Videos`. Anything below that belongs to somebody else
/// and stays where it is; the same goes for clips from before the storage
/// location was moved.
fn inside(clip_dir: &Path, file: &Path) -> bool {
    let mut dir = file.parent();
    for _ in 0..3 {
        match dir {
            Some(path) if path == clip_dir => return true,
            Some(path) => dir = path.parent(),
            None => return false,
        }
    }
    false
}

/// Move a clip's file to where it belongs.
///
/// Returns the new path; `None` means there was nothing to do. A clip whose
/// file cannot be found right now, or that lies outside the clip folder, is
/// left untouched.
pub fn place(clip: &Clip, clip_dir: &str) -> Result<Option<PathBuf>, String> {
    let file = PathBuf::from(&clip.path);
    let clip_dir = Path::new(clip_dir);
    if !file.is_file() || !inside(clip_dir, &file) {
        return Ok(None);
    }
    let dir = dir_for(clip_dir, clip.game.as_deref(), clip.favorite, clip.kind());
    if file.parent() == Some(dir.as_path()) {
        return Ok(None);
    }
    let name = file
        .file_name()
        .ok_or_else(|| "The clip has no file name.".to_string())?;

    std::fs::create_dir_all(&dir).map_err(|err| {
        format!("could not create folder '{}': {err}", dir.display())
    })?;
    let target = free_name(&dir, name.to_string_lossy().as_ref());
    move_file(&file, &target)?;
    // If that was the last clip of its game, the folder should not be left
    // standing empty.
    if let Some(parent) = file.parent() {
        prune(parent, clip_dir);
    }
    Ok(Some(target))
}

/// Bring every stored game name to the one spelling.
///
/// Runs at startup, right before [`tidy`], and only ever changes names that are
/// not already normal. A game whose window title carried invisible characters
/// built a category per sprinkling — four folders called "ARC Raiders", each
/// holding a few clips, none of them findable under the others. Putting the
/// names right is all this does: `tidy` then sees clips whose game no longer
/// matches their folder and moves the files together, and `place` prunes the
/// folders that empties.
///
/// Returns how many clips were rewritten.
pub fn normalize_games(library: &Library) -> usize {
    let clips = match library.list() {
        Ok(clips) => clips,
        Err(err) => {
            log::warn!("game names not normalised: {err}");
            return 0;
        }
    };

    let mut before: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut after: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut changed = 0usize;

    for clip in clips {
        let Some(game) = clip.game.as_deref() else {
            continue;
        };
        let clean = crate::game::normalize_name(game);
        before.insert(game.to_string());
        // Nothing left but invisibles: the clip has no game rather than one
        // with an empty name.
        let target = (!clean.is_empty()).then_some(clean);
        after.extend(target.clone());
        if target.as_deref() == Some(game) {
            continue;
        }
        match library.set_game(&clip.id, target.as_deref()) {
            Ok(()) => changed += 1,
            Err(err) => log::warn!("game of '{}' not corrected: {err}", clip.id),
        }
    }

    if changed > 0 {
        log::info!(
            "normalised the game of {changed} clip(s): {} name(s) became {}",
            before.len(),
            after.len()
        );
    }
    changed
}

/// Collect every clip that is not in its folder.
///
/// Runs at startup: a move that failed because the file was open at the time is
/// caught up here — and clips from older versions find their way into their
/// game folder without anyone doing anything.
pub fn tidy(library: &Library, clip_dir: &str) {
    let clips = match library.list() {
        Ok(clips) => clips,
        Err(err) => {
            log::warn!("clips not filed: {err}");
            return;
        }
    };
    let mut moved = 0usize;
    for clip in clips {
        match place(&clip, clip_dir) {
            Ok(None) => {}
            Ok(Some(target)) => {
                let path = target.to_string_lossy().to_string();
                if let Err(err) = library.set_path(&clip.id, &path) {
                    log::warn!("new path of '{}' not recorded: {err}", clip.id);
                } else {
                    moved += 1;
                }
            }
            Err(err) => log::warn!("clip '{}' left in place: {err}", clip.id),
        }
    }
    if moved > 0 {
        log::info!("filed {moved} clip(s) into their folder");
    }
}

/// Clear away a subfolder that has become empty, and the one above it if that
/// leaves it empty too — an emptied `Counter-Strike 2/Videos` would otherwise
/// leave `Counter-Strike 2` standing around forever.
///
/// The clip folder itself always stays, and so does a folder with content:
/// `remove_dir` fails of its own accord then, and that ends the walk upwards.
pub fn prune(dir: &Path, clip_dir: &Path) {
    if dir == clip_dir || !dir.starts_with(clip_dir) {
        return;
    }
    if std::fs::remove_dir(dir).is_err() {
        return;
    }
    if let Some(parent) = dir.parent() {
        prune(parent, clip_dir);
    }
}

/// A free file name in the target folder. The names carry milliseconds, so a
/// collision is the exception — but nothing gets overwritten regardless.
fn free_name(dir: &Path, name: &str) -> PathBuf {
    let target = dir.join(name);
    if !target.exists() {
        return target;
    }
    let path = Path::new(name);
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let ext = path.extension().map(|e| e.to_string_lossy().to_string());
    for n in 2..1000 {
        let candidate = match &ext {
            Some(ext) => dir.join(format!("{stem} ({n}).{ext}")),
            None => dir.join(format!("{stem} ({n})")),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    target
}

/// Moving with a few attempts: the player can hold the file open for another
/// blink of an eye, and Windows will not let go of it then.
fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    let mut last = None;
    for attempt in 0..5 {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(err) => {
                last = Some(err);
                std::thread::sleep(std::time::Duration::from_millis(40 * (attempt + 1)));
            }
        }
    }
    Err(format!(
        "Could not move the file to '{}' ({}). Is it open right now?",
        to.display(),
        last.map(|err| err.to_string()).unwrap_or_default()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_game_becomes_a_readable_folder() {
        assert_eq!(folder_name("Counter-Strike 2").as_deref(), Some("Counter-Strike 2"));
        // Forbidden characters turn into whitespace, and that collapses.
        assert_eq!(folder_name("Tom: The / Movie").as_deref(), Some("Tom The Movie"));
        assert_eq!(folder_name("  Bodycam  ").as_deref(), Some("Bodycam"));
    }

    /// The real strings ARC Raiders wrote into the clip database. Identical to
    /// the eye, four different folders on disk.
    const ARC_RAIDERS: [&str; 4] = [
        "A\u{200b}\u{200b}\u{200b}\u{200b}R\u{feff}C\u{200b}\u{200b}\u{2005}\u{feff}\u{200b}\u{200b}\u{feff}\u{200b}\u{200b}\u{feff}\u{feff}Raid\u{feff}\u{200b}\u{200b}e\u{200b}\u{200b}rs",
        "A\u{200b}\u{200b}\u{feff}R\u{200b}\u{200b}C\u{2005}\u{200b}\u{200b}\u{200b}R\u{200b}\u{200b}ai\u{200b}\u{200b}\u{200b}\u{200b}\u{200b}\u{200b}\u{200b}\u{200b}de\u{200b}\u{200b}rs",
        "A\u{feff}RC\u{2005}\u{200b}\u{200b}\u{200b}\u{200b}\u{feff}Ra\u{feff}\u{feff}\u{feff}\u{200b}\u{200b}i\u{200b}d\u{200b}\u{200b}e\u{200b}r\u{200b}\u{feff}\u{200b}\u{200b}s",
        "\u{200b}A\u{200b}\u{200b}\u{feff}\u{feff}\u{200b}RC\u{200b}\u{200b} \u{200b}R\u{200b}a\u{200b}\u{200b}i\u{200b}d\u{feff}er\u{200b}\u{200b}\u{200b}\u{200b}\u{feff}s",
    ];

    #[test]
    fn invisible_characters_do_not_make_a_second_folder() {
        for game in ARC_RAIDERS {
            assert_eq!(folder_name(game).as_deref(), Some("ARC Raiders"), "from {game:?}");
        }
    }

    /// A zero width space sits inside a word. Replacing it with a space the way
    /// a forbidden character is replaced would give "Raid ers".
    #[test]
    fn an_invisible_closes_up_instead_of_splitting_the_word() {
        assert_eq!(folder_name("Raid\u{200b}ers").as_deref(), Some("Raiders"));
        // A forbidden character still becomes a space, as before.
        assert_eq!(folder_name("Raid/ers").as_deref(), Some("Raid ers"));
    }

    #[test]
    fn a_name_of_nothing_but_invisibles_is_no_name() {
        assert_eq!(folder_name("\u{200b}\u{feff}\u{200b}"), None);
    }

    fn clip_with_game(id: &str, game: Option<&str>) -> Clip {
        Clip {
            id: id.into(),
            path: format!("C:/clips/{id}.mp4"),
            created_at: 1,
            duration_ms: 30_000,
            game: game.map(str::to_string),
            width: 1920,
            height: 1080,
            size_bytes: 1,
            thumb_path: None,
            title: None,
            description: None,
            edit: None,
            original: None,
            original_available: false,
            favorite: false,
            screenshot: false,
            recording: false,
        }
    }

    /// The whole point of the migration: twenty-four clips spread over four
    /// categories that were always the same game end up under one name.
    #[test]
    fn the_four_categories_become_one() {
        let lib = Library::in_memory().unwrap();
        for (n, game) in ARC_RAIDERS.iter().enumerate() {
            lib.insert(&clip_with_game(&format!("c{n}"), Some(game))).unwrap();
        }
        // One that was never broken must not be touched.
        lib.insert(&clip_with_game("plain", Some("Bodycam"))).unwrap();

        assert_eq!(normalize_games(&lib), 4);

        let names: std::collections::HashSet<Option<String>> =
            lib.list().unwrap().into_iter().map(|c| c.game).collect();
        assert_eq!(
            names,
            ["ARC Raiders", "Bodycam"]
                .iter()
                .map(|s| Some(s.to_string()))
                .collect()
        );
    }

    #[test]
    fn a_second_run_changes_nothing() {
        let lib = Library::in_memory().unwrap();
        for (n, game) in ARC_RAIDERS.iter().enumerate() {
            lib.insert(&clip_with_game(&format!("c{n}"), Some(game))).unwrap();
        }
        assert_eq!(normalize_games(&lib), 4);
        assert_eq!(normalize_games(&lib), 0, "the pass is not idempotent");
    }

    #[test]
    fn a_game_of_nothing_but_invisibles_becomes_no_game() {
        let lib = Library::in_memory().unwrap();
        lib.insert(&clip_with_game("ghost", Some("\u{200b}\u{feff}"))).unwrap();
        assert_eq!(normalize_games(&lib), 1);
        assert_eq!(lib.list().unwrap()[0].game, None);
    }

    #[test]
    fn a_clip_without_a_game_is_left_alone() {
        let lib = Library::in_memory().unwrap();
        lib.insert(&clip_with_game("none", None)).unwrap();
        assert_eq!(normalize_games(&lib), 0);
        assert_eq!(lib.list().unwrap()[0].game, None);
    }

    /// Windows creates neither a folder ending in a dot nor one named like a
    /// device.
    #[test]
    fn windows_quirks_are_taken_care_of() {
        assert_eq!(folder_name("Portal 2.").as_deref(), Some("Portal 2"));
        assert_eq!(folder_name("CON").as_deref(), Some("_CON"));
        assert_eq!(folder_name("com1").as_deref(), Some("_com1"));
        assert_eq!(folder_name("   "), None);
        assert_eq!(folder_name("???"), None);
    }

    #[test]
    fn the_favourite_folder_wins_over_the_game() {
        let root = Path::new("C:/clips");
        assert_eq!(
            dir_for(root, Some("Bodycam"), false, ClipKind::Clip),
            root.join("Bodycam").join(VIDEOS),
        );
        assert_eq!(
            dir_for(root, Some("Bodycam"), true, ClipKind::Clip),
            root.join(FAVORITES).join(VIDEOS),
        );
        // With no game only the kind is left.
        assert_eq!(dir_for(root, None, false, ClipKind::Clip), root.join(VIDEOS));
    }

    /// A clip, a still and a recording of the same game are never in the
    /// same folder.
    #[test]
    fn the_kind_splits_the_game_folder() {
        let root = Path::new("C:/clips");
        assert_eq!(
            dir_for(root, Some("Bodycam"), false, ClipKind::Screenshot),
            root.join("Bodycam").join(SCREENSHOTS),
        );
        assert_eq!(
            dir_for(root, None, true, ClipKind::Screenshot),
            root.join(FAVORITES).join(SCREENSHOTS),
        );
        assert_eq!(
            dir_for(root, Some("Bodycam"), false, ClipKind::Recording),
            root.join("Bodycam").join(RECORDINGS),
        );
        assert_eq!(
            dir_for(root, Some("Bodycam"), true, ClipKind::Recording),
            root.join(FAVORITES).join(RECORDINGS),
        );
    }

    /// The flag on the clip is what picks the folder.
    #[test]
    fn a_recording_is_filed_as_one() {
        let mut clip = clip_with_game("r", Some("Bodycam"));
        assert_eq!(clip.kind(), ClipKind::Clip);
        clip.recording = true;
        assert_eq!(clip.kind(), ClipKind::Recording);
    }

    /// Only our own folder and one level below get rearranged — otherwise
    /// changing the storage location would drag the old clips along.
    #[test]
    fn only_files_in_the_clip_folder_are_moved() {
        let root = Path::new("C:/clips");
        assert!(inside(root, Path::new("C:/clips/a.mp4")));
        assert!(inside(root, Path::new("C:/clips/Videos/a.mp4")));
        assert!(inside(root, Path::new("C:/clips/Bodycam/Videos/a.mp4")));
        assert!(!inside(root, Path::new("C:/clips/Bodycam/Videos/old/a.mp4")));
        assert!(!inside(root, Path::new("D:/elsewhere/a.mp4")));
    }

    /// The clip folder itself must never disappear, even when empty.
    #[test]
    fn pruning_stops_at_the_clip_folder() {
        let dir = std::env::temp_dir().join("clippiboy-prune-test");
        let _ = std::fs::create_dir_all(&dir);
        prune(&dir, &dir);
        assert!(dir.is_dir());
        let _ = std::fs::remove_dir(&dir);
    }
}
