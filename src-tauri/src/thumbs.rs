//! Clip thumbnails.
//!
//! They deliberately do **not** sit next to the video: the clip folder belongs
//! to the user, and whoever opens it wants to see clips, not half an image file
//! per recording. So ClippiBoy collects them here, named after the clip's id.
//!
//! Unlike the scratch folder in `preview.rs`, this store survives a restart —
//! regenerating a picture means running ffmpeg over every clip in the gallery.

use std::path::{Path, PathBuf};

use crate::clips::{fresh, passes_stopped, with_library, Library};
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

/// Compute a thumbnail from a still and store it.
///
/// The same thing, minus the `-ss 0.5`: a single image has no half second to
/// seek to, and ffmpeg gives up rather than delivering the one frame it has.
///
/// A screenshot must not be its own thumbnail, however tempting that is —
/// `migrate` below moves every picture that lies outside this folder into it,
/// and would carry the original out of the user's clip folder.
pub fn make_still(image: &Path, clip_id: &str) -> Result<PathBuf, String> {
    let out = path(clip_id);
    std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
    run(
        ffmpeg()
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .arg("-i")
            .arg(image)
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
/// Returns how many records were corrected.
pub fn migrate(library: &parking_lot::Mutex<Option<Library>>) -> usize {
    let clips = match with_library(library, |lib| lib.list()).unwrap_or(Ok(Vec::new())) {
        Ok(clips) => clips,
        Err(err) => {
            log::warn!("thumbnails not migrated: {err}");
            return 0;
        }
    };
    let dir = dir();
    if let Err(err) = std::fs::create_dir_all(&dir) {
        log::warn!("thumbnail folder not created: {err}");
        return 0;
    }

    let mut changed = 0usize;
    for listed in clips {
        if passes_stopped() {
            break;
        }
        if listed.thumb_path.is_none() {
            continue;
        }
        let lock = crate::edit::clip_lock(&listed.id);
        let _own = lock.lock();
        let Some(clip) = fresh(library, &listed.id) else {
            continue;
        };
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
        match with_library(library, |lib| lib.set_thumb(&clip.id, value.as_deref())) {
            Some(Ok(())) => changed += 1,
            Some(Err(err)) => log::warn!("thumbnail of '{}' not recorded: {err}", clip.id),
            None => {}
        }
    }
    changed
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
