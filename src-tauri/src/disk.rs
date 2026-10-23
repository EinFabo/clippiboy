//! How much room is left on the drive the clips go to.
//!
//! One question, one syscall — but it deserves a file of its own because of
//! *when* it must not be asked. On a disconnected network share
//! `GetDiskFreeSpaceExW` blocks until Windows gives up on the connection, which
//! is seconds. Anything that calls it therefore has to be a thread that is
//! allowed to stall; the status snapshot, which answers a command from the
//! webview, is not. So the status tick reads this every ten seconds into an
//! atomic and everybody else reads the atomic.
//!
//! It also answers whether two paths name one file, which a string compare
//! cannot: case, `.`, hard links and junctions all spell the same file anew.

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

/// The root of the volume `dir` lies on (`C:\`, a mount point, a share) —
/// two folders with the same one share their free space. Blocks like
/// [`free_bytes`] on a share that is gone.
#[cfg(windows)]
pub fn volume_of(dir: &Path) -> Option<std::path::PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetVolumePathNameW;

    let path = existing(dir)?;
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let mut root = vec![0u16; wide.len().max(4) + 1];
    unsafe { GetVolumePathNameW(PCWSTR(wide.as_ptr()), &mut root) }.ok()?;
    let len = root.iter().position(|&c| c == 0)?;
    Some(std::ffi::OsString::from_wide(&root[..len]).into())
}

#[cfg(not(windows))]
pub fn volume_of(_dir: &Path) -> Option<std::path::PathBuf> {
    None
}

/// Do `a` and `b` name the same file — under any spelling, through a hard link
/// or a junction? Asks the file system for the file's identity: the volume's
/// serial number and the file's index on Windows, device and inode elsewhere.
/// `false` when either cannot be opened.
#[cfg(windows)]
pub fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_READ_ATTRIBUTES,
    };

    let identity = |path: &Path| -> Option<(u32, u32, u32)> {
        // The attributes are all it takes, and asking for no more lets this
        // open a clip another program holds open for writing.
        let file = std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES.0)
            .open(path)
            .ok()?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }.ok()?;
        Some((info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow))
    };
    match (identity(a), identity(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(not(windows))]
pub fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
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

    #[cfg(windows)]
    #[test]
    fn two_folders_on_one_drive_share_a_volume() {
        let root = std::env::temp_dir();
        let volume = volume_of(&root).expect("the temp folder has a volume");
        assert_eq!(volume_of(&root.join("not-there-yet").join("deeper")), Some(volume.clone()));
        let drive = &root.to_string_lossy()[..2];
        assert!(volume.to_string_lossy().eq_ignore_ascii_case(&format!("{drive}\\")), "{}", volume.display());
    }

    #[test]
    fn the_same_file_is_recognised_under_another_spelling() {
        let dir = std::env::temp_dir().join(format!("clippiboy-same-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("Clip.mp4");
        let other = dir.join("Other.mp4");
        std::fs::write(&clip, b"x").unwrap();
        std::fs::write(&other, b"x").unwrap();
        let linked = dir.join("linked.mp4");
        std::fs::hard_link(&clip, &linked).unwrap();
        assert!(same_file(&clip, &dir.join(".").join("Clip.mp4")));
        assert!(same_file(&clip, &linked), "a hard link is the same file");
        if cfg!(windows) {
            assert!(same_file(&clip, &dir.join("clip.MP4")), "another case");
        }
        assert!(!same_file(&clip, &other));
        assert!(!same_file(&clip, &dir.join("missing.mp4")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_existing_folder_answers_for_itself() {
        let root = std::env::temp_dir();
        assert_eq!(existing(&root), Some(root.as_path()));
    }
}
