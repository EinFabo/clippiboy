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
    dpapi(plain, true)
}

/// Opens what [`protect`] sealed. Fails for anything sealed by another user,
/// on another PC or without our entropy, and for anything changed.
pub fn unprotect(sealed: &[u8]) -> Result<Vec<u8>, String> {
    dpapi(sealed, false)
}

fn dpapi(data: &[u8], seal: bool) -> Result<Vec<u8>, String> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(data.len()).map_err(|_| "too large to seal".to_string())?,
        pbData: data.as_ptr() as *mut u8,
    };
    let entropy = CRYPT_INTEGER_BLOB { cbData: ENTROPY.len() as u32, pbData: ENTROPY.as_ptr() as *mut u8 };
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
    result.map_err(|err| err.to_string())?;
    if output.pbData.is_null() {
        return Err("DPAPI handed back nothing".into());
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
    serde_json::to_vec(&Envelope { sealed: crate::webhook::base64url(&blob) }).map_err(|err| err.to_string())
}

/// Reads a file with a secret in it: opens a sealed one, and takes a plain one
/// from before as it is and seals it in place. `None` when there is none, or
/// when it cannot be opened here — a profile copied from another PC, say —
/// which then counts as never stored (signed out, not connected).
pub(crate) fn read<T: Serialize + DeserializeOwned>(path: &Path, what: &str) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    match held::<T>(&bytes) {
        Held::Sealed(blob) => {
            let opened = unprotect(&blob).and_then(|plain| serde_json::from_slice(&plain).map_err(|err| err.to_string()));
            match opened {
                Ok(value) => Some(value),
                Err(err) => {
                    warn_once(path, &format!("{what}: the stored one does not open here ({err}) — taken as none"));
                    None
                }
            }
        }
        Held::Plain(value) => {
            match sealed(&value).and_then(|bytes| write_file(path, &bytes).map_err(|err| err.to_string())) {
                Ok(()) => log::info!("{what}: sealed the one kept from before"),
                Err(err) => warn_once(path, &format!("{what}: could not seal the one kept from before: {err}")),
            }
            Some(value)
        }
        Held::Unreadable => {
            warn_once(path, &format!("{what}: the stored one cannot be read — taken as none"));
            None
        }
    }
}

/// Writes `value` sealed. Should DPAPI fail, it is kept in plain as before —
/// the user stays signed in — and sealed on a later read.
pub(crate) fn write_sealed<T: Serialize>(path: &Path, value: &T, what: &str) -> std::io::Result<()> {
    let bytes = match sealed(value) {
        Ok(bytes) => bytes,
        Err(err) => {
            log::warn!("{what}: could not seal it, kept in plain: {err}");
            serde_json::to_vec(value).map_err(std::io::Error::other)?
        }
    };
    write_file(path, &bytes)
}

/// Into a file next to it first, then over it: a crash halfway leaves the old
/// file or the new one, never half of one.
pub(crate) fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    std::fs::write(&part, bytes)?;
    std::fs::rename(&part, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&part);
    })
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
        let input = CRYPT_INTEGER_BLOB { cbData: 4, pbData: b"abcd".as_ptr() as *mut u8 };
        let mut output = CRYPT_INTEGER_BLOB::default();
        unsafe {
            CryptProtectData(&input, PCWSTR::null(), None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output).unwrap();
            let foreign = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            let _ = LocalFree(HLOCAL(output.pbData.cast()));
            assert!(unprotect(&foreign).is_err());
        }
    }

    #[test]
    fn files_from_before_and_sealed_ones_are_told_apart() {
        assert_eq!(held::<Stored>(br#"{"token":"abc"}"#), Held::Plain(Stored { token: "abc".into() }));
        assert_eq!(held::<Stored>(br#"{"sealed":"AQID"}"#), Held::Sealed(vec![1, 2, 3]));
        assert_eq!(held::<Stored>(br#"{"sealed":"*"}"#), Held::Unreadable);
        assert_eq!(held::<Stored>(br#"{"sealed":""}"#), Held::Unreadable);
        assert_eq!(held::<Stored>(b"{\"tok"), Held::Unreadable);
        assert_eq!(held::<Stored>(br#"{"other":1}"#), Held::Unreadable);
    }

    #[test]
    fn a_plain_file_from_before_is_read_and_then_kept_sealed() {
        let dir = std::env::temp_dir().join(format!("clippiboy-secret-{}", uuid::Uuid::new_v4().simple()));
        let path = dir.join("friends.json");
        write_file(&path, br#"{"token":"abc"}"#).unwrap();
        assert_eq!(read::<Stored>(&path, "test"), Some(Stored { token: "abc".into() }));
        let now = std::fs::read(&path).unwrap();
        assert!(matches!(held::<Stored>(&now), Held::Sealed(_)));
        assert!(!String::from_utf8_lossy(&now).contains("abc"));
        assert_eq!(read::<Stored>(&path, "test"), Some(Stored { token: "abc".into() }));

        write_sealed(&path, &Stored { token: "def".into() }, "test").unwrap();
        assert_eq!(read::<Stored>(&path, "test"), Some(Stored { token: "def".into() }));
        // Sealed elsewhere (another PC, another user): signed out, no crash.
        let foreign = serde_json::to_vec(&Envelope { sealed: crate::webhook::base64url(&[7; 200]) }).unwrap();
        write_file(&path, &foreign).unwrap();
        assert_eq!(read::<Stored>(&path, "test"), None);
        assert_eq!(read::<Stored>(&dir.join("missing.json"), "test"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
