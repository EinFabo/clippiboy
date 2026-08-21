//! Wo eine Clipdatei im Clip-Ordner liegt.
//!
//! Je Spiel ein Ordner, Favoriten in ihrem eigenen, alles ohne Spiel bleibt
//! direkt im Clip-Ordner. Die Datenbank bleibt dabei die Wahrheit über den
//! Clip: Der Ordner ist nur die Ordnung, die man im Explorer sieht — in der
//! App findet man einen Favoriten weiterhin unter seinem Spiel.
//!
//! Verschoben wird nur, was **im eingestellten Clip-Ordner** liegt. Wer den
//! Speicherort umstellt, lässt seine bisherigen Clips bewusst liegen, wo sie
//! sind; die räumt hier niemand hinterher.

use std::path::{Path, PathBuf};

use crate::clips::Library;
use crate::model::Clip;

/// Ordner der mit dem Herz markierten Clips.
pub const FAVORITES: &str = "Favoriten";

/// Zeichen, die Windows in einem Ordnernamen nicht zulässt.
const FORBIDDEN: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Gerätenamen, die Windows für sich behält — als Ordnername unbrauchbar.
const RESERVED: [&str; 6] = ["CON", "PRN", "AUX", "NUL", "COM", "LPT"];

/// Länger wird ein Ordnername nicht. Spielnamen kommen teils aus
/// Fenstertiteln, und die können ganze Sätze sein.
const MAX_LEN: usize = 60;

/// Aus einem Spielnamen einen Ordnernamen machen, den Windows annimmt.
///
/// `None` heißt: Von dem Namen bleibt nichts Brauchbares übrig — dann kommt
/// der Clip in den Clip-Ordner selbst, wie einer ganz ohne Spiel.
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
    // Windows verträgt am Ende weder Punkt noch Leerzeichen.
    let name = name.trim_end_matches(['.', ' ']).to_string();
    if name.is_empty() {
        return None;
    }
    // `CON.mp4` lässt sich unter Windows nicht anlegen, `_CON` schon.
    let stem = name.split('.').next().unwrap_or(&name).to_ascii_uppercase();
    let reserved = RESERVED.iter().any(|word| {
        stem == *word
            || (stem.len() == word.len() + 1
                && stem.starts_with(word)
                && stem.ends_with(|c: char| c.is_ascii_digit()))
    });
    Some(if reserved { format!("_{name}") } else { name })
}

/// Der Ordner, in den ein Clip mit diesem Spiel und diesem Herz gehört.
pub fn dir_for(clip_dir: &Path, game: Option<&str>, favorite: bool) -> PathBuf {
    if favorite {
        return clip_dir.join(FAVORITES);
    }
    match game.and_then(folder_name) {
        Some(name) => clip_dir.join(name),
        None => clip_dir.to_path_buf(),
    }
}

/// Liegt die Datei im Clip-Ordner — direkt darin oder eine Ebene tiefer?
fn inside(clip_dir: &Path, file: &Path) -> bool {
    match file.parent() {
        Some(parent) => parent == clip_dir || parent.parent() == Some(clip_dir),
        None => false,
    }
}

/// Die Datei eines Clips dorthin bringen, wo sie hingehört.
///
/// Gibt den neuen Pfad zurück; `None` heißt, dass nichts zu tun war. Ein Clip,
/// dessen Datei gerade nicht auffindbar ist oder außerhalb des Clip-Ordners
/// liegt, bleibt unangetastet.
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
        .ok_or_else(|| "Der Clip hat keinen Dateinamen.".to_string())?;

    std::fs::create_dir_all(&dir).map_err(|err| {
        format!("Ordner '{}' ließ sich nicht anlegen: {err}", dir.display())
    })?;
    let target = free_name(&dir, name.to_string_lossy().as_ref());
    move_file(&file, &target)?;
    // War das der letzte Clip seines Spiels, soll der Ordner nicht leer
    // stehen bleiben.
    if let Some(parent) = file.parent() {
        prune(parent, clip_dir);
    }
    Ok(Some(target))
}

/// Alle Clips einsammeln, die nicht in ihrem Ordner liegen.
///
/// Läuft beim Start: Ein Umzug, der scheiterte, weil die Datei gerade offen
/// war, wird so nachgeholt — und Clips aus älteren Fassungen finden ohne
/// Zutun in ihren Spielordner.
pub fn tidy(library: &Library, clip_dir: &str) {
    let clips = match library.list() {
        Ok(clips) => clips,
        Err(err) => {
            log::warn!("Clips nicht einsortiert: {err}");
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
                    log::warn!("Neuer Pfad von '{}' nicht vermerkt: {err}", clip.id);
                } else {
                    moved += 1;
                }
            }
            Err(err) => log::warn!("Clip '{}' blieb liegen: {err}", clip.id),
        }
    }
    if moved > 0 {
        log::info!("{moved} Clip(s) in ihren Ordner einsortiert");
    }
}

/// Einen leer gewordenen Unterordner wegräumen. Der Clip-Ordner selbst bleibt
/// immer stehen, und ein Ordner mit Inhalt ebenfalls: `remove_dir` scheitert
/// dann von sich aus.
pub fn prune(dir: &Path, clip_dir: &Path) {
    if dir == clip_dir || !dir.starts_with(clip_dir) {
        return;
    }
    let _ = std::fs::remove_dir(dir);
}

/// Ein freier Dateiname im Zielordner. Die Namen tragen Millisekunden, ein
/// Zusammenstoß ist also die Ausnahme — überschrieben wird trotzdem nichts.
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

/// Verschieben mit ein paar Anläufen: Der Player kann die Datei noch einen
/// Wimpernschlag lang offen halten, und Windows lässt sie dann nicht los.
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
        "Die Datei ließ sich nicht nach '{}' verschieben ({}). Ist sie gerade geöffnet?",
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
        // Verbotene Zeichen werden zu Leerraum und der fällt zusammen.
        assert_eq!(folder_name("Tom: Der / Film").as_deref(), Some("Tom Der Film"));
        assert_eq!(folder_name("  Bodycam  ").as_deref(), Some("Bodycam"));
    }

    /// Windows legt weder einen Ordner mit Punkt am Ende an noch einen, der
    /// wie ein Gerät heißt.
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
        // Ohne Spiel bleibt der Clip, wo er ist.
        assert_eq!(dir_for(root, None, false), root);
    }

    /// Nur der eigene Ordner und eine Ebene darunter werden umgeräumt — sonst
    /// zöge ein Wechsel des Speicherorts die alten Clips hinterher.
    #[test]
    fn only_files_in_the_clip_folder_are_moved() {
        let root = Path::new("C:/clips");
        assert!(inside(root, Path::new("C:/clips/a.mp4")));
        assert!(inside(root, Path::new("C:/clips/Bodycam/a.mp4")));
        assert!(!inside(root, Path::new("C:/clips/Bodycam/alt/a.mp4")));
        assert!(!inside(root, Path::new("D:/woanders/a.mp4")));
    }

    /// Der Clip-Ordner selbst darf nie verschwinden, auch wenn er leer ist.
    #[test]
    fn pruning_stops_at_the_clip_folder() {
        let dir = std::env::temp_dir().join("clippiboy-prune-test");
        let _ = std::fs::create_dir_all(&dir);
        prune(&dir, &dir);
        assert!(dir.is_dir());
        let _ = std::fs::remove_dir(&dir);
    }
}
