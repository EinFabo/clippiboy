//! Vorschaubilder der Clips.
//!
//! Sie liegen bewusst **nicht** neben dem Video: Der Clip-Ordner gehört dem
//! Nutzer, und wer ihn öffnet, will Clips sehen und keine halbe Bilddatei je
//! Aufnahme. Deshalb sammelt ClippiBoy sie hier, benannt nach der Kennung des
//! Clips.
//!
//! Anders als der Wegwerf-Ordner in `preview.rs` bleibt diese Ablage über den
//! Neustart hinaus liegen — ein Bild neu zu rechnen heißt, ffmpeg über jeden
//! Clip der Galerie laufen zu lassen.

use std::path::{Path, PathBuf};

use crate::clips::Library;
use crate::config;
use crate::muxer::{ffmpeg, run, sanitize};

/// Ordner aller Vorschaubilder.
pub fn dir() -> PathBuf {
    config::data_dir().join("thumbs")
}

/// Das Bild eines Clips. Der Name folgt der Kennung, nicht dem Dateinamen:
/// Der Clip darf umbenannt oder verschoben werden, ohne sein Bild zu verlieren.
pub fn path(clip_id: &str) -> PathBuf {
    dir().join(format!("{}.jpg", sanitize(clip_id)))
}

/// Ein Vorschaubild aus dem Video rechnen und ablegen.
pub fn make(video: &Path, clip_id: &str) -> Result<PathBuf, String> {
    let out = path(clip_id);
    std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
    run(
        ffmpeg()
            .args(["-y", "-hide_banner", "-loglevel", "error", "-ss", "0.5"])
            .arg("-i")
            .arg(video)
            .args(["-frames:v", "1", "-vf", "scale=480:-1", "-q:v", "4"])
            .arg(&out),
        "Vorschaubild",
    )?;
    Ok(out)
}

/// Das Bild eines gelöschten Clips wegräumen.
pub fn remove(clip_id: &str) {
    let _ = std::fs::remove_file(path(clip_id));
}

/// Bilder aus älteren Fassungen einsammeln.
///
/// Bis 0.1.4 lag das Bild als `.jpg` neben dem `.mp4` im Clip-Ordner. Beim
/// Start wandert es hierher — sonst bliebe für jeden bestehenden Clip eine
/// Bilddatei im Ordner des Nutzers zurück, und zwar für immer.
pub fn migrate(library: &Library) {
    let clips = match library.list() {
        Ok(clips) => clips,
        Err(err) => {
            log::warn!("Vorschaubilder nicht umgezogen: {err}");
            return;
        }
    };
    let dir = dir();
    if let Err(err) = std::fs::create_dir_all(&dir) {
        log::warn!("Ordner für Vorschaubilder nicht angelegt: {err}");
        return;
    }

    for clip in clips {
        let Some(old) = clip.thumb_path.as_deref().map(PathBuf::from) else {
            continue;
        };
        if old.starts_with(&dir) {
            continue;
        }
        let new = path(&clip.id);
        let target = if old.is_file() {
            match move_file(&old, &new) {
                Ok(()) => Some(new),
                Err(err) => {
                    log::warn!("Vorschaubild '{}' blieb liegen: {err}", old.display());
                    continue;
                }
            }
        } else if new.is_file() {
            // Schon umgezogen, nur der Eintrag hinkt hinterher.
            Some(new)
        } else {
            // Das Bild ist weg — dann soll auch der Eintrag nicht ins Leere
            // zeigen. Die Galerie zeigt dafür ihren Farbverlauf.
            None
        };
        let value = target.map(|path| path.to_string_lossy().to_string());
        if let Err(err) = library.set_thumb(&clip.id, value.as_deref()) {
            log::warn!("Vorschaubild von '{}' nicht vermerkt: {err}", clip.id);
        }
    }
}

/// Verschieben über Laufwerksgrenzen hinweg: Der Clip-Ordner liegt gern auf
/// einer anderen Platte als `%APPDATA%`, und dort scheitert `rename`.
fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(from);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Der Ordner des Nutzers bleibt außen vor — genau darum geht es hier.
    #[test]
    fn the_pictures_live_in_the_data_folder() {
        assert!(path("abc").starts_with(config::data_dir()));
        assert_eq!(path("a/b:c").file_name().unwrap(), "a_b_c.jpg");
    }
}
