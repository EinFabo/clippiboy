//! How much room is left on the drive the clips go to.
//!
//! One question, one syscall — but it deserves a file of its own because of
//! *when* it must not be asked. On a disconnected network share
//! `GetDiskFreeSpaceExW` blocks until Windows gives up on the connection, which
//! is seconds. Anything that calls it therefore has to be a thread that is
//! allowed to stall; the status snapshot, which answers a command from the
//! webview, is not. So the status tick reads this every ten seconds into an
//! atomic and everybody else reads the atomic.

use std::path::Path;

/// The first ancestor of `dir` that actually exists — `dir` itself if it does.
///
/// The clip folder is created on the first save, so for a fresh installation it
/// is not there yet. Its drive is, though, and that is the number being asked
/// for.
fn existing(dir: &Path) -> Option<&Path> {
    let mut at = Some(dir);
    while let Some(path) = at {
        if path.exists() {
            return Some(path);
        }
        at = path.parent();
    }
    None
}

#[cfg(windows)]
pub fn free_bytes(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let path = existing(dir)?;
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let mut free: u64 = 0;
    // The first of the three numbers, not the third: what is free **to this
    // user**. On a drive with a quota the two differ, and the one that decides
    // whether the next clip fits is this one.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), None, None) }.ok()?;
    Some(free)
}

#[cfg(not(windows))]
pub fn free_bytes(_dir: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_that_is_not_there_yet_answers_for_its_drive() {
        let root = std::env::temp_dir();
        let missing = root.join("clippiboy-nicht-da").join("auch-nicht");
        assert_eq!(existing(&missing), Some(root.as_path()));
    }

    #[test]
    fn an_existing_folder_answers_for_itself() {
        let root = std::env::temp_dir();
        assert_eq!(existing(&root), Some(root.as_path()));
    }
}
