//! Secrets kept on disk — the friends session, the Discord webhook, the key
//! that names this PC to friends — sealed to the Windows user with DPAPI.
//!
//! Sealed like this, a file copied off the PC, out of a backup or by another
//! account on it opens nowhere else. A program running as the same user can
//! still open it, as it could read the user's browser profile: nothing at this
//! level stops that.
//!
//! Files from before (1.0.0 and older) hold the secret in plain JSON. They are
//! read once as they are and then written again sealed.

use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

/// Mixed into every seal, so another program of the same user that uses DPAPI
/// cannot open ours by mistake, nor we its.
const ENTROPY: &[u8] = b"ClippiBoy/secret/1";

/// Seals `plain` to the current Windows user.
pub fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
    dpapi(plain, true).map_err(|(_, err)| err)
}

/// Opens what [`protect`] sealed. Fails for anything sealed by another user,
/// on another PC or without our entropy, and for anything changed.
pub fn unprotect(sealed: &[u8]) -> Result<Vec<u8>, String> {
    unprotect_coded(sealed).map_err(|(_, err)| err)
}

/// [`unprotect`], with the HRESULT of a failure (0 when DPAPI gave none) —
/// to tell a seal that is no seal ([`seal_broken`]) from one DPAPI cannot
/// open just now.
pub fn unprotect_coded(sealed: &[u8]) -> Result<Vec<u8>, (i32, String)> {
    dpapi(sealed, false)
}

/// How every DPAPI blob begins: version 1, then the provider's GUID
/// (df9d8cd0-1501-11d1-8c7a-00c04fc297eb) as it lies in memory.
const BLOB_HEAD: [u8; 20] = [
    0x01, 0x00, 0x00, 0x00, 0xd0, 0x8c, 0x9d, 0xdf, 0x01, 0x15, 0xd1, 0x11, 0x8c, 0x7a, 0x00, 0xc0,
    0x4f, 0xc2, 0x97, 0xeb,
];

/// Could `bytes` be a DPAPI blob at all? A file that does not even begin
/// like one was not written by [`protect`].
pub fn looks_sealed(bytes: &[u8]) -> bool {
    bytes.starts_with(&BLOB_HEAD)
}

/// Did a seal fail to open because it is none — damaged, or sealed with a key
/// this user no longer has — rather than because DPAPI cannot work right now?
/// NTE_BAD_DATA, ERROR_INVALID_DATA (as an HRESULT) and NTE_BAD_KEY_STATE.
pub fn seal_broken(code: i32) -> bool {
    const NTE_BAD_DATA: u32 = 0x8009_0005;
    const ERROR_INVALID_DATA: u32 = 0x8007_000D;
    const NTE_BAD_KEY_STATE: u32 = 0x8009_000B;
    matches!(
        code as u32,
        NTE_BAD_DATA | ERROR_INVALID_DATA | NTE_BAD_KEY_STATE
    )
}

fn dpapi(data: &[u8], seal: bool) -> Result<Vec<u8>, (i32, String)> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(data.len()).map_err(|_| (0, "too large to seal".to_string()))?,
        pbData: data.as_ptr() as *mut u8,
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: ENTROPY.len() as u32,
        pbData: ENTROPY.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: DPAPI only reads `input` and `entropy`, which outlive the call;
    // on success `output` is a LocalAlloc'd buffer of `cbData` bytes, copied
    // out and freed below.
    let result = unsafe {
        if seal {
            CryptProtectData(
                &input,
                PCWSTR::null(),
                Some(&entropy as *const _),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                None,
                Some(&entropy as *const _),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    result.map_err(|err| (err.code().0, err.to_string()))?;
    if output.pbData.is_null() {
        return Err((0, "DPAPI handed back nothing".into()));
    }
    // SAFETY: see above; an opened secret is wiped before its buffer goes back.
    unsafe {
        let out = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        if !seal {
            std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
        }
        let _ = LocalFree(HLOCAL(output.pbData.cast()));
        Ok(out)
    }
}

// --- Files ------------------------------------------------------------------------

/// A sealed file as it is kept: the DPAPI blob, in base64url.
#[derive(Serialize, Deserialize)]
struct Envelope {
    sealed: String,
}

/// What a file with a secret in it holds.
#[derive(Debug, PartialEq)]
pub(crate) enum Held<T> {
    /// Written by [`write_sealed`]: the blob to open.
    Sealed(Vec<u8>),
    /// Written before secrets were sealed, or when DPAPI failed.
    Plain(T),
    /// Neither — broken, or a shape nobody wrote.
    Unreadable,
}

pub(crate) fn held<T: DeserializeOwned>(bytes: &[u8]) -> Held<T> {
    if let Ok(envelope) = serde_json::from_slice::<Envelope>(bytes) {
        return match crate::webhook::from_base64url(&envelope.sealed) {
            Some(blob) if !blob.is_empty() => Held::Sealed(blob),
            _ => Held::Unreadable,
        };
    }
    match serde_json::from_slice::<T>(bytes) {
        Ok(value) => Held::Plain(value),
        Err(_) => Held::Unreadable,
    }
}

/// `value` as the content of a sealed file.
fn sealed<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let plain = serde_json::to_vec(value).map_err(|err| err.to_string())?;
    let blob = protect(&plain)?;
    serde_json::to_vec(&Envelope {
        sealed: crate::webhook::base64url(&blob),
    })
    .map_err(|err| err.to_string())
}

/// Reads a file with a secret in it: opens a sealed one, and takes a plain one
/// from before as it is and seals it in place. `None` when there is none, or
/// when it cannot be opened here — a profile copied from another PC, say —
/// which then counts as never stored (signed out, not connected).
pub(crate) fn read<T: Serialize + DeserializeOwned>(path: &Path, what: &str) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    match held::<T>(&bytes) {
        Held::Sealed(blob) => match unprotect(&blob) {
            Ok(plain) => match serde_json::from_slice(&plain) {
                Ok(value) => Some(value),
                // Not serde's words: they can quote what the file holds.
                Err(_) => {
                    warn_once(
                        path,
                        &format!("{what}: the stored one does not parse — taken as none"),
                    );
                    None
                }
            },
            Err(err) => {
                warn_once(
                    path,
                    &format!("{what}: the stored one does not open here ({err}) — taken as none"),
                );
                None
            }
        },
        Held::Plain(value) => {
            match sealed(&value)
                .and_then(|bytes| write_file(path, &bytes).map_err(|err| err.to_string()))
            {
                Ok(()) => log::info!("{what}: sealed the one kept from before"),
                Err(err) => warn_once(
                    path,
                    &format!("{what}: could not seal the one kept from before: {err}"),
                ),
            }
            Some(value)
        }
        Held::Unreadable => {
            warn_once(
                path,
                &format!("{what}: the stored one cannot be read — taken as none"),
            );
            None
        }
    }
}

/// Writes `value` sealed. Should DPAPI fail, it is kept in plain as before —
/// the user stays signed in — and sealed on a later read.
pub(crate) fn write_sealed<T: Serialize>(
    path: &Path,
    value: &T,
    what: &str,
) -> std::io::Result<()> {
    let bytes = match sealed(value) {
        Ok(bytes) => bytes,
        Err(err) => {
            log::warn!("{what}: could not seal it, kept in plain: {err}");
            serde_json::to_vec(value).map_err(std::io::Error::other)?
        }
    };
    write_file(path, &bytes)
}

/// Into a file next to it first, on the disk before it goes over the old one:
/// a crash or a power cut halfway leaves the old file or the new one, never
/// half of one. The file next to it is named for this write alone, so two
/// writes at once (two threads, or a second ClippiBoy) do not share one.
pub(crate) fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    sweep_parts(path);
    let write = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut part = path.as_os_str().to_owned();
    part.push(format!(".{}-{write}.part", std::process::id()));
    let part = PathBuf::from(part);
    let written = std::fs::File::create(&part).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    written
        .and_then(|()| std::fs::rename(&part, path))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&part);
        })
}

/// How old another run's file next to the target must be before it counts as
/// left behind. A write takes milliseconds; a second ClippiBoy writing the same
/// file just now keeps its own.
const PART_STALE: std::time::Duration = std::time::Duration::from_secs(60);

/// The pid in the name of a file [`write_file`] puts next to `file`
/// (`<file>.<pid>-<write>.part`), or `None` for any other name.
fn part_pid(file: &str, name: &str) -> Option<u32> {
    let rest = name
        .strip_prefix(file)?
        .strip_prefix('.')?
        .strip_suffix(".part")?;
    let (pid, write) = rest.split_once('-')?;
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    if !digits(pid) || !digits(write) {
        return None;
    }
    pid.parse().ok()
}

/// Removes what a write cut short by a crash or a power cut left next to
/// `path`: files of other runs only, and only once they are [`PART_STALE`].
/// Without this each such file would stay for good, as its name is never
/// written again.
fn sweep_parts(path: &Path) {
    let (Some(dir), Some(file)) = (
        path.parent(),
        path.file_name().and_then(|name| name.to_str()),
    ) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let own = std::process::id();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|name| part_pid(file, name)) else {
            continue;
        };
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= PART_STALE);
        if pid != own && stale {
            match std::fs::remove_file(entry.path()) {
                Ok(()) => log::info!("removed {name:?}, left by a write that did not finish"),
                Err(err) => log::warn!("could not remove {name:?}: {err}"),
            }
        }
    }
}

/// Once per file and run: a file that does not open would otherwise be
/// reported on every look at it.
fn warn_once(path: &Path, message: &str) {
    static WARNED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    let mut warned = WARNED.lock();
    if !warned.iter().any(|seen| seen == path) {
        warned.push(path.to_owned());
        log::warn!("{message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Stored {
        token: String,
    }

    #[test]
    fn a_seal_opens_again_and_only_as_it_was() {
        let sealed = protect(b"session-token").unwrap();
        assert!(!sealed.windows(13).any(|w| w == b"session-token"));
        assert_eq!(unprotect(&sealed).unwrap(), b"session-token");
        let mut changed = sealed.clone();
        let last = changed.len() - 1;
        changed[last] ^= 1;
        assert!(unprotect(&changed).is_err());
        assert!(unprotect(b"not a seal").is_err());
    }

    #[test]
    fn a_seal_without_our_entropy_does_not_open() {
        let input = CRYPT_INTEGER_BLOB {
            cbData: 4,
            pbData: b"abcd".as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        unsafe {
            CryptProtectData(
                &input,
                PCWSTR::null(),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
            .unwrap();
            let foreign =
                std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            let _ = LocalFree(HLOCAL(output.pbData.cast()));
            assert!(unprotect(&foreign).is_err());
        }
    }

    /// What DPAPI writes begins the same way every time; a file that does not
    /// was never ours. A blob changed after sealing does not open, and says
    /// it is broken rather than unopenable for now.
    #[test]
    fn a_seal_is_known_by_its_head_and_a_broken_one_by_its_error() {
        let sealed = protect(b"key").unwrap();
        assert!(looks_sealed(&sealed));
        assert!(!looks_sealed(b"not a seal at all, but long enough"));
        assert!(!looks_sealed(&BLOB_HEAD[..19]));
        assert!(!looks_sealed(&[0u8; 32]));

        let mut changed = sealed.clone();
        let last = changed.len() - 1;
        changed[last] ^= 1;
        let (code, err) = unprotect_coded(&changed).unwrap_err();
        assert!(seal_broken(code), "{code:#x}: {err}");

        assert!(seal_broken(0x8009_0005_u32 as i32));
        assert!(seal_broken(0x8007_000D_u32 as i32));
        assert!(seal_broken(0x8009_000B_u32 as i32));
        assert!(
            !seal_broken(0x8007_0005_u32 as i32),
            "access denied may pass"
        );
        assert!(!seal_broken(0));
    }

    /// Each write has a file of its own next to the target, and none is left
    /// behind.
    #[test]
    fn writes_leave_only_the_file() {
        let dir = std::env::temp_dir().join(format!(
            "clippiboy-secret-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let path = dir.join("webhook.json");
        let threads: Vec<_> = (0..8u8)
            .map(|n| {
                let path = path.clone();
                // Windows may turn a rename away while another lands on the
                // same name; what counts is that nothing half is left.
                std::thread::spawn(move || {
                    let _ = write_file(&path, &[n; 64]);
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let content = std::fs::read(&path).unwrap();
        assert_eq!(content.len(), 64);
        assert!(
            content.iter().all(|byte| *byte == content[0]),
            "one write whole"
        );
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["webhook.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_part_is_known_by_its_name() {
        assert_eq!(
            part_pid("webhook.json", "webhook.json.4242-7.part"),
            Some(4242)
        );
        assert_eq!(part_pid("webhook.json", "webhook.json.4242-7"), None);
        assert_eq!(part_pid("webhook.json", "webhook.json.part"), None);
        assert_eq!(part_pid("webhook.json", "webhook.json.-7.part"), None);
        assert_eq!(part_pid("webhook.json", "webhook.json.+1-7.part"), None);
        assert_eq!(part_pid("webhook.json", "webhook.json.4242-.part"), None);
        assert_eq!(part_pid("webhook.json", "other.json.4242-7.part"), None);
        assert_eq!(part_pid("webhook.json", "webhook.json2.4242-7.part"), None);
    }

    /// A crash between the write and the rename leaves a file next to the
    /// target. The next write removes another run's once it is old, and keeps
    /// a fresh one (a second ClippiBoy may be writing it), this run's own, and
    /// whatever belongs to another file.
    #[test]
    fn a_write_sweeps_the_parts_a_crash_left() {
        let dir = std::env::temp_dir().join(format!(
            "clippiboy-secret-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("webhook.json");
        let other = std::process::id().wrapping_add(1);
        let own = std::process::id();
        let old = std::time::SystemTime::now() - 2 * PART_STALE;
        let place = |name: String, modified: Option<std::time::SystemTime>| {
            let file = std::fs::File::create(dir.join(&name)).unwrap();
            if let Some(at) = modified {
                file.set_modified(at).unwrap();
            }
            name
        };
        let left = place(format!("webhook.json.{other}-3.part"), Some(old));
        let fresh = place(format!("webhook.json.{other}-4.part"), None);
        let mine = place(format!("webhook.json.{own}-999999.part"), Some(old));
        let foreign = place(format!("token.json.{other}-3.part"), Some(old));

        write_file(&path, b"{}").unwrap();

        let mut names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        let mut want = vec![fresh, mine, foreign, "webhook.json".to_string()];
        want.sort();
        assert_eq!(names, want, "{left} should be gone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_from_before_and_sealed_ones_are_told_apart() {
        assert_eq!(
            held::<Stored>(br#"{"token":"abc"}"#),
            Held::Plain(Stored {
                token: "abc".into()
            })
        );
        assert_eq!(
            held::<Stored>(br#"{"sealed":"AQID"}"#),
            Held::Sealed(vec![1, 2, 3])
        );
        assert_eq!(held::<Stored>(br#"{"sealed":"*"}"#), Held::Unreadable);
        assert_eq!(held::<Stored>(br#"{"sealed":""}"#), Held::Unreadable);
        assert_eq!(held::<Stored>(b"{\"tok"), Held::Unreadable);
        assert_eq!(held::<Stored>(br#"{"other":1}"#), Held::Unreadable);
    }

    #[test]
    fn a_plain_file_from_before_is_read_and_then_kept_sealed() {
        let dir = std::env::temp_dir().join(format!(
            "clippiboy-secret-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let path = dir.join("friends.json");
        write_file(&path, br#"{"token":"abc"}"#).unwrap();
        assert_eq!(
            read::<Stored>(&path, "test"),
            Some(Stored {
                token: "abc".into()
            })
        );
        let now = std::fs::read(&path).unwrap();
        assert!(matches!(held::<Stored>(&now), Held::Sealed(_)));
        assert!(!String::from_utf8_lossy(&now).contains("abc"));
        assert_eq!(
            read::<Stored>(&path, "test"),
            Some(Stored {
                token: "abc".into()
            })
        );

        write_sealed(
            &path,
            &Stored {
                token: "def".into(),
            },
            "test",
        )
        .unwrap();
        assert_eq!(
            read::<Stored>(&path, "test"),
            Some(Stored {
                token: "def".into()
            })
        );
        // Sealed elsewhere (another PC, another user): signed out, no crash.
        let foreign = serde_json::to_vec(&Envelope {
            sealed: crate::webhook::base64url(&[7; 200]),
        })
        .unwrap();
        write_file(&path, &foreign).unwrap();
        assert_eq!(read::<Stored>(&path, "test"), None);
        assert_eq!(read::<Stored>(&dir.join("missing.json"), "test"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
