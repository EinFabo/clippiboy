//! Recently deleted: a deleted clip waits here for [`KEEP_DAYS`] before it is
//! gone for good.
//!
//! The row moves out of `clips` into a table of its own (`Library::trash`), so
//! nothing that looks clips up by id can stumble over a deleted one. The file
//! moves into a hidden `.deleted` folder; the clip's tracks, original,
//! thumbnail and cam stay in their stores, so a restore brings back the whole
//! clip and not only its video. Share links and Discord copies go at once —
//! they would show what was deleted.

use std::path::{Component, Path, PathBuf};

/// How long a deleted clip can still be restored.
pub const KEEP_DAYS: i64 = 30;
pub const KEEP_MS: i64 = KEEP_DAYS * 24 * 60 * 60 * 1000;

/// The folder deleted files wait in. Starts with a dot, so the sweep over the
/// clip folder and the filing never look inside (`sweep::leftovers_in`).
pub const DIR: &str = ".deleted";

/// How often the expired ones are looked for while the app runs.
pub const CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Where a deleted clip's file waits: `.deleted` in the clip folder, unless the
/// clip lies on another drive — then beside it, so deleting stays a rename and
/// never copies gigabytes from one drive to the other.
pub fn folder_for(clip_path: &Path, clip_dir: &Path) -> PathBuf {
    let same_drive = drive(clip_path) == drive(clip_dir);
    match clip_path.parent() {
        Some(parent) if !same_drive => parent.join(DIR),
        _ => clip_dir.join(DIR),
    }
}

/// The drive a path starts with (`C:`, `\\server\share`), compared without case.
fn drive(path: &Path) -> Option<String> {
    match path.components().next() {
        Some(Component::Prefix(prefix)) => Some(prefix.as_os_str().to_string_lossy().to_lowercase()),
        _ => None,
    }
}

/// Is a clip deleted at `deleted_at` (Unix ms) past keeping at `now`?
pub fn expired(deleted_at: i64, now: i64) -> bool {
    now.saturating_sub(deleted_at) >= KEEP_MS
}

/// The moment a clip deleted at `deleted_at` goes for good.
pub fn expires_at(deleted_at: i64) -> i64 {
    deleted_at.saturating_add(KEEP_MS)
}

/// Creates the folder and hides it in Explorer — a folder of deleted clips in
/// the middle of the user's clips would look like they never went.
pub fn ensure_folder(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    hide(dir);
    Ok(())
}

#[cfg(windows)]
fn hide(dir: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{GetFileAttributesW, SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, FILE_FLAGS_AND_ATTRIBUTES, INVALID_FILE_ATTRIBUTES};
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        let now = GetFileAttributesW(PCWSTR(wide.as_ptr()));
        if now == INVALID_FILE_ATTRIBUTES || now & FILE_ATTRIBUTE_HIDDEN.0 != 0 {
            return;
        }
        let _ = SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(now | FILE_ATTRIBUTE_HIDDEN.0));
    }
}

#[cfg(not(windows))]
fn hide(_dir: &Path) {}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clip_is_kept_for_thirty_days() {
        let day = 24 * 60 * 60 * 1000;
        assert!(!expired(0, 29 * day));
        assert!(!expired(0, 30 * day - 1));
        assert!(expired(0, 30 * day));
        assert_eq!(expires_at(5), 5 + 30 * day);
        // A clock set back does not throw anything away.
        assert!(!expired(10 * day, 0));
    }

    #[cfg(windows)]
    #[test]
    fn the_folder_is_on_the_clips_own_drive() {
        let clip_dir = Path::new(r"C:\Users\x\Videos\ClippiBoy");
        assert_eq!(
            folder_for(Path::new(r"C:\Users\x\Videos\ClippiBoy\Valorant\clip.mp4"), clip_dir),
            clip_dir.join(DIR)
        );
        assert_eq!(
            folder_for(Path::new(r"c:\Elsewhere\clip.mp4"), clip_dir),
            clip_dir.join(DIR),
            "the drive letter's case does not matter"
        );
        assert_eq!(
            folder_for(Path::new(r"D:\Clips\Valorant\clip.mp4"), clip_dir),
            Path::new(r"D:\Clips\Valorant").join(DIR)
        );
    }
}
