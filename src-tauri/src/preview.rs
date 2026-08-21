//! Throwaway helper files for the player.
//!
//! Everything here can be regenerated in a second and is cleared out at
//! startup. What does **not** belong here are a clip's individual tracks —
//! those are the master copy of the mix and live in `stems.rs`.

use std::path::{Path, PathBuf};

use crate::config;
use crate::model::Clip;
use crate::muxer::{ffmpeg, run, sanitize};
use crate::stems::is_newer;

/// Folder for the helper files. It sits in the data directory because the clip
/// folder belongs to the user and should not have to put up with scratch files.
pub fn dir() -> PathBuf {
    config::data_dir().join("preview")
}

/// Clean up at startup: these files belong to clips that may not exist any
/// more.
pub fn clear() {
    let _ = std::fs::remove_dir_all(dir());
    // Recreate it right away: the player gets the folder whitelisted at
    // startup, and a missing folder is needless to explain.
    let _ = std::fs::create_dir_all(dir());
}

/// A picture of the audio track for the timeline. It shows where something
/// happens before you have to listen for it — otherwise finding the right spot
/// is pure guesswork.
///
/// A transparent PNG, so the timeline keeps its own colour.
pub fn waveform(clip: &Clip) -> Result<PathBuf, String> {
    let dir = dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let out = dir.join(format!("{}_wave.png", sanitize(&clip.id)));
    if is_newer(&out, Path::new(&clip.path)) {
        return Ok(out);
    }

    let mut draw = ffmpeg();
    draw.args(["-y", "-hide_banner", "-loglevel", "error"])
        .arg("-i")
        .arg(&clip.path)
        // `split_channels=0` lays left and right on top of each other: the
        // timeline is 40 pixels tall, two separate channels would no longer be
        // distinguishable in there.
        .args([
            "-filter_complex",
            "[0:a:0]aformat=channel_layouts=mono,\
             showwavespic=s=1920x64:colors=white:split_channels=0[wave]",
            "-map",
            "[wave]",
            "-frames:v",
            "1",
        ])
        .arg(&out);
    run(&mut draw, "draw waveform")?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stems;

    /// `clear()` deletes its folder along with everything in it. If the stems
    /// lived inside it, every mix would be permanently unchangeable after each
    /// app start — and without an error message at that.
    #[test]
    fn the_stems_are_not_inside_the_scratch_folder() {
        assert!(
            !stems::root().starts_with(dir()),
            "the stems must not live in the scratch folder"
        );
        assert_ne!(stems::root(), dir());
    }
}
