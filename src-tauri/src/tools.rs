//! ffmpeg and ffprobe — fetched once, not shipped with every update.
//!
//! They used to sit in the installer. The two programs are some 200 MB, the
//! app itself a few; and since the updater always downloads and runs the whole
//! installer, every release shipped and unpacked the same unchanged ffmpeg once
//! more. Now they come from a release of their own (`ffmpeg-<version>`) and are
//! kept in the app's local data folder under their version number. An update
//! of the app leaves that folder alone; only a new [`VERSION`] fetches again.
//!
//! What comes down is not trusted by itself: both programs have to match the
//! hashes below before they are ever started. The hash is taken once, when a
//! program arrives — not at every start, which would read 200 MB each time.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use tauri::{Emitter, Manager};

/// The ffmpeg build ClippiBoy is tested with.
const VERSION: &str = "9.0.1";

/// The archive with both programs, from the repository's own release.
const URL: &str =
    "https://github.com/EinFabo/clippiboy/releases/download/ffmpeg-9.0.1/ffmpeg-9.0.1.zip";

/// Each program with the SHA-256 it has to have — from gyan.dev's
/// `ffmpeg-9.0.1-essentials_build`.
const PROGRAMS: [(&str, &str); 2] = [
    (
        "ffmpeg.exe",
        "72a489eccd008c2ec2c0a5856c5c75bc3d8bbfa90166c4566865c246445e6aa3",
    ),
    (
        "ffprobe.exe",
        "19202b23c0043f15ad1b7bce2344f406fd52bd6efd8f995ce02e7392a1cec52f",
    ),
];

/// How long to wait after a failed attempt before the next — longer each time,
/// the last value repeats. "Retry" in the window cuts the wait short.
const BACKOFF: [Duration; 4] = [
    Duration::from_secs(30),
    Duration::from_secs(2 * 60),
    Duration::from_secs(10 * 60),
    Duration::from_secs(30 * 60),
];

/// How often the download reports its progress to the UI at most.
const PROGRESS_EVERY: Duration = Duration::from_millis(150);

/// Where the programs stand — sent to the UI as `ffmpeg-status`.
#[derive(Clone, serde::Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Status {
    /// Looking whether they are already there.
    Checking,
    #[serde(rename_all = "camelCase")]
    Downloading { downloaded: u64, total: Option<u64> },
    /// Unpacking and checking the hashes.
    Unpacking,
    Ready,
    /// The last attempt failed; the next one follows by itself.
    #[serde(rename_all = "camelCase")]
    Failed { error: String, retry_in_secs: u64 },
}

static STATUS: Mutex<Status> = Mutex::new(Status::Checking);
static WAKE: OnceLock<(Sender<()>, Receiver<()>)> = OnceLock::new();

fn wake() -> &'static (Sender<()>, Receiver<()>) {
    WAKE.get_or_init(|| crossbeam_channel::bounded(1))
}

pub fn status() -> Status {
    STATUS.lock().clone()
}

pub fn is_ready() -> bool {
    matches!(*STATUS.lock(), Status::Ready)
}

/// Words for someone who wanted a clip while the programs are not there yet.
pub fn not_ready_reason() -> String {
    match status() {
        Status::Downloading {
            downloaded,
            total: Some(total),
        } if total > 0 => format!(
            "ffmpeg is still being downloaded ({} %) — the buffer keeps running, try again in a moment.",
            downloaded * 100 / total
        ),
        Status::Failed { error, .. } => {
            format!("ffmpeg could not be downloaded yet ({error}). No clips can be written without it.")
        }
        _ => "ffmpeg is still being set up — the buffer keeps running, try again in a moment."
            .into(),
    }
}

/// Cut a waiting retry short.
pub fn retry() {
    let _ = wake().0.try_send(());
}

fn set(app: &tauri::AppHandle, status: Status) {
    *STATUS.lock() = status.clone();
    let _ = app.emit("ffmpeg-status", status);
}

/// Make the programs available: at once if they are there, otherwise in the
/// background. Until then [`crate::muxer`] falls back to the PATH.
pub fn setup(app: &tauri::AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let root = match app.path().app_local_data_dir() {
            Ok(dir) => dir.join("ffmpeg"),
            Err(err) => {
                log::error!("no data folder for ffmpeg: {err}");
                set(
                    &app,
                    Status::Failed {
                        error: "no data folder".into(),
                        retry_in_secs: 0,
                    },
                );
                return;
            }
        };
        let target = root.join(VERSION);
        let mut attempt = 0;
        loop {
            match provide(&app, &root, &target) {
                Ok(()) => break,
                Err(err) => {
                    let wait = BACKOFF[attempt.min(BACKOFF.len() - 1)];
                    attempt += 1;
                    log::warn!("could not provide ffmpeg: {err}");
                    set(
                        &app,
                        Status::Failed {
                            error: err,
                            retry_in_secs: wait.as_secs(),
                        },
                    );
                    // A timeout and a wake-up both mean: try again.
                    let _ = wake().1.recv_timeout(wait);
                }
            }
        }
        crate::muxer::set_tool_dir(target.clone());
        set(&app, Status::Ready);
        tidy(&root, &target);
    });
}

fn provide(app: &tauri::AppHandle, root: &Path, target: &Path) -> Result<(), String> {
    set(app, Status::Checking);
    // Only ever filled by a rename after the check, so presence is enough.
    if PROGRAMS.iter().all(|(name, _)| target.join(name).is_file()) {
        return Ok(());
    }
    std::fs::create_dir_all(root).map_err(|err| format!("could not create {}: {err}", root.display()))?;
    if adopt_bundled(app, root, target) {
        log::info!("ffmpeg taken over from the old installation");
        return Ok(());
    }
    log::info!("downloading ffmpeg {VERSION}");
    tauri::async_runtime::block_on(download(app, root, target))
}

/// Installations up to 0.4.7 brought the programs along in their resource
/// folder, and an update does not remove them. When they are the right ones,
/// move them over instead of fetching them again.
fn adopt_bundled(app: &tauri::AppHandle, root: &Path, target: &Path) -> bool {
    let Ok(resources) = app.path().resource_dir().map(|dir| dir.join("resources")) else {
        return false;
    };
    let matches = PROGRAMS.iter().all(|(name, hash)| {
        sha256_of(&resources.join(name)).is_ok_and(|found| found == *hash)
    });
    if !matches {
        return false;
    }
    let staging = root.join(format!("{VERSION}.adopt"));
    let _ = std::fs::remove_dir_all(&staging);
    if std::fs::create_dir_all(&staging).is_err() {
        return false;
    }
    for (name, _) in PROGRAMS {
        let (from, to) = (resources.join(name), staging.join(name));
        // Both sit under the local app data, so a rename is instant; a copy
        // is the way out should they ever live on different drives.
        if std::fs::rename(&from, &to).is_err() && std::fs::copy(&from, &to).is_err() {
            let _ = std::fs::remove_dir_all(&staging);
            return false;
        }
    }
    std::fs::rename(&staging, target).is_ok()
}

async fn download(app: &tauri::AppHandle, root: &Path, target: &Path) -> Result<(), String> {
    // reqwest runs on the rustls the updater brings along, without a crypto
    // provider of its own — the same move the updater makes before it asks.
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("ClippiBoy/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30))
        // Only against a connection that stops sending altogether.
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|err| format!("no HTTP client: {err}"))?;
    let mut response = client
        .get(URL)
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|err| format!("download failed: {err}"))?;

    let total = response.content_length();
    let archive = root.join(format!("{VERSION}.zip.part"));
    let mut file =
        File::create(&archive).map_err(|err| format!("could not write {}: {err}", archive.display()))?;
    let mut downloaded = 0u64;
    let mut last_emit: Option<Instant> = None;
    set(app, Status::Downloading { downloaded, total });
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| format!("download broke off: {err}"))?
    {
        file.write_all(&chunk)
            .map_err(|err| format!("could not write the download: {err}"))?;
        downloaded += chunk.len() as u64;
        if last_emit.map_or(true, |at| at.elapsed() >= PROGRESS_EVERY) {
            last_emit = Some(Instant::now());
            set(app, Status::Downloading { downloaded, total });
        }
    }
    drop(file);
    if total.is_some_and(|total| downloaded != total) {
        let _ = std::fs::remove_file(&archive);
        return Err(format!("download incomplete ({downloaded} bytes)"));
    }

    set(app, Status::Unpacking);
    let result = unpack(&archive, root, target);
    let _ = std::fs::remove_file(&archive);
    result
}

/// Take both programs out of the archive, check them, and only then give them
/// their final place — a folder under [`VERSION`] is always complete.
fn unpack(archive: &Path, root: &Path, target: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|err| format!("could not open the download: {err}"))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|err| format!("the download is not a ZIP archive: {err}"))?;
    let staging = root.join(format!("{VERSION}.unpack"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|err| format!("could not unpack: {err}"))?;

    let checked = (|| {
        for (name, hash) in PROGRAMS {
            let mut entry = zip
                .by_name(name)
                .map_err(|_| format!("{name} is missing from the download"))?;
            let path = staging.join(name);
            let mut out =
                File::create(&path).map_err(|err| format!("could not write {name}: {err}"))?;
            let found = copy_hashing(&mut entry, &mut out)
                .map_err(|err| format!("could not unpack {name}: {err}"))?;
            if found != hash {
                return Err(format!("{name} does not match its checksum — download rejected"));
            }
        }
        Ok(())
    })();
    if let Err(err) = checked {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(err);
    }
    std::fs::rename(&staging, target).map_err(|err| format!("could not put ffmpeg in place: {err}"))
}

/// Remove what an older version or a broken-off attempt left behind.
fn tidy(root: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == keep {
            continue;
        }
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if removed.is_ok() {
            log::info!("removed {}", path.display());
        }
    }
}

fn sha256_of(path: &Path) -> std::io::Result<String> {
    copy_hashing(&mut File::open(path)?, &mut std::io::sink())
}

/// Copy `from` to `to` and return the SHA-256 of what went through.
fn copy_hashing(from: &mut impl Read, to: &mut impl Write) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let read = from.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
        to.write_all(&buf[..read])?;
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
