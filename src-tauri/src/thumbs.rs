//! Clip thumbnails.
//!
//! They deliberately do **not** sit next to the video: the clip folder belongs
//! to the user, and whoever opens it wants to see clips, not half an image file
//! per recording. So ClippiBoy collects them here, named after the clip's id.
//!
//! Unlike the scratch folder in `preview.rs`, this store survives a restart —
//! regenerating a picture means running ffmpeg over every clip in the gallery.

use std::path::{Path, PathBuf};

use crate::clips::Library;
use crate::config;
use crate::muxer::{ffmpeg, run, sanitize};

/// Folder holding all thumbnails.
pub fn dir() -> PathBuf {
    config::data_dir().join("thumbs")
}

/// A clip's picture. The name follows the id, not the file name: the clip may
/// be renamed or moved without losing its picture.
pub fn path(clip_id: &str) -> PathBuf {
    dir().join(format!("{}.jpg", sanitize(clip_id)))
}

/// Compute a thumbnail from the video and store it.
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
        "thumbnail",
    )?;
    Ok(out)
}

/// Clear away the picture of a deleted clip.
pub fn remove(clip_id: &str) {
    let _ = std::fs::remove_file(path(clip_id));
}

/// Collect pictures from older versions.
///
/// Up to 0.1.4 the picture sat as a `.jpg` next to the `.mp4` in the clip
/// folder. At startup it moves here — otherwise every existing clip would leave
/// an image file behind in the user's folder, forever.
pub fn migrate(library: &Library) {
    let clips = match library.list() {
        Ok(clips) => clips,
        Err(err) => {
            log::warn!("thumbnails not migrated: {err}");
            return;
        }
    };
    let dir = dir();
    if let Err(err) = std::fs::create_dir_all(&dir) {
        log::warn!("thumbnail folder not created: {err}");
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
                    log::warn!("thumbnail '{}' left in place: {err}", old.display());
                    continue;
                }
            }
        } else if new.is_file() {
            // Already migrated, only the record is lagging behind.
            Some(new)
        } else {
            // The picture is gone — then the record should not point into the
            // void either. The gallery shows its gradient instead.
            None
        };
        let value = target.map(|path| path.to_string_lossy().to_string());
        if let Err(err) = library.set_thumb(&clip.id, value.as_deref()) {
            log::warn!("thumbnail of '{}' not recorded: {err}", clip.id);
        }
    }
}

/// Moving across drive boundaries: the clip folder likes to sit on a different
/// disk than `%APPDATA%`, and `rename` fails there.
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

    /// The user's folder stays out of it — that is the whole point here.
    #[test]
    fn the_pictures_live_in_the_data_folder() {
        assert!(path("abc").starts_with(config::data_dir()));
        assert_eq!(path("a/b:c").file_name().unwrap(), "a_b_c.jpg");
    }
}
