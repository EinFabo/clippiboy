//! Where a clip file sits inside the clip folder.
//!
//! One folder per game, favorites in their own, anything without a game stays
//! directly in the clip folder. The database remains the truth about a clip:
//! the folder is only the order you see in Explorer — inside the app a favorite
//! is still found under its game.
//!
//! Only what lies **in the configured clip folder** is moved. Whoever changes
//! the storage location deliberately leaves their existing clips where they
//! are; nobody tidies those up afterwards.

use std::path::{Path, PathBuf};

use crate::clips::Library;
use crate::model::Clip;

/// Folder for the clips marked with a heart.
pub const FAVORITES: &str = "Favorites";

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
    let cleaned: String = game
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

/// The folder a clip with this game and this heart belongs in.
pub fn dir_for(clip_dir: &Path, game: Option<&str>, favorite: bool) -> PathBuf {
    if favorite {
        return clip_dir.join(FAVORITES);
    }
    match game.and_then(folder_name) {
        Some(name) => clip_dir.join(name),
        None => clip_dir.to_path_buf(),
    }
}

/// Is the file in the clip folder — directly in it or one level down?
fn inside(clip_dir: &Path, file: &Path) -> bool {
    match file.parent() {
        Some(parent) => parent == clip_dir || parent.parent() == Some(clip_dir),
        None => false,
    }
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
    let dir = dir_for(clip_dir, clip.game.as_deref(), clip.favorite);
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

/// Clear away a subfolder that has become empty. The clip folder itself always
/// stays, and so does a folder with content: `remove_dir` fails of its own
/// accord then.
pub fn prune(dir: &Path, clip_dir: &Path) {
    if dir == clip_dir || !dir.starts_with(clip_dir) {
        return;
    }
    let _ = std::fs::remove_dir(dir);
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
        assert_eq!(dir_for(root, Some("Bodycam"), false), root.join("Bodycam"));
        assert_eq!(dir_for(root, Some("Bodycam"), true), root.join(FAVORITES));
        // With no game the clip stays where it is.
        assert_eq!(dir_for(root, None, false), root);
    }

    /// Only our own folder and one level below get rearranged — otherwise
    /// changing the storage location would drag the old clips along.
    #[test]
    fn only_files_in_the_clip_folder_are_moved() {
        let root = Path::new("C:/clips");
        assert!(inside(root, Path::new("C:/clips/a.mp4")));
        assert!(inside(root, Path::new("C:/clips/Bodycam/a.mp4")));
        assert!(!inside(root, Path::new("C:/clips/Bodycam/alt/a.mp4")));
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
