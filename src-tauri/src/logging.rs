//! Where the log goes.
//!
//! `env_logger` writes to stderr, and the release build is a GUI application
//! without a console (`main.rs`, `windows_subsystem = "windows"`). Everything
//! the encoder had to say about itself therefore went nowhere at all on exactly
//! the machines where somebody needed to read it back: the ones we cannot sit
//! in front of. Asking a tester to reproduce a fault is only worth anything if
//! there is a file to send back afterwards.
//!
//! The split happens at the writer rather than at the logger, so both copies
//! come out byte for byte identical — same formatting, same timestamps, one
//! implementation.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Where the logs live. Next to the configuration, so "the ClippiBoy folder"
/// stays one place a person has to find.
pub fn log_dir() -> PathBuf {
    crate::config::data_dir().join("logs")
}

pub fn log_path() -> PathBuf {
    log_dir().join("clippiboy.log")
}

/// The file half of [`Fork`]. Empty until a file is attached — and in a second
/// start that hands over and ends, it stays empty.
static FILE: parking_lot::Mutex<Option<File>> = parking_lot::Mutex::new(None);

/// Stderr and the file, from one stream of bytes.
struct Fork;

impl Write for Fork {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Neither copy may take the other down: a full disk must not silence
        // the console, and a closed console must not stop the file.
        let _ = std::io::stderr().write_all(buf);
        if let Some(file) = FILE.lock().as_mut() {
            let _ = file.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stderr().flush();
        if let Some(file) = FILE.lock().as_mut() {
            let _ = file.flush();
        }
        Ok(())
    }
}

/// Start the previous run's log over, keeping one generation.
///
/// A run is the unit that matters here — a fault is reported against one
/// session, and hunting for its beginning in a file that has been growing for
/// weeks is its own small chore. One generation back is kept because the answer
/// to "it went wrong, I restarted it, then I wrote to you" would otherwise
/// already be gone.
fn rotate(path: &Path) {
    if path.exists() {
        let _ = std::fs::rename(path, path.with_extension("log.1"));
    }
}

/// The log file for this run, or `None` when this is not the first ClippiBoy.
///
/// A second start (a shortcut, a `clippiboy://` link, autostart twice) lives
/// for a moment before single-instance ends it. Rotating there moved the
/// running session's log over `.log.1` — the running one kept writing into the
/// renamed file, and the next start threw it away: the log of exactly the
/// session that had the problem. So a second start touches neither file.
fn open_for(path: &Path, first: bool) -> Option<File> {
    if !first {
        return None;
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    rotate(path);
    File::create(path).ok()
}

/// Rotate and open the log file, once. Called again from `setup`: a start that
/// found another ClippiBoy running and still got past single-instance — the
/// other one quit in between — is the app now and needs its file. A no-op once
/// a file is attached.
pub fn attach_file() {
    if FILE.lock().is_some() {
        return;
    }
    let path = log_path();
    match open_for(&path, true) {
        Some(file) => {
            *FILE.lock() = Some(file);
            log::info!("log file: {}", path.display());
        }
        None => log::warn!("no log file at {} — stderr only", path.display()),
    }
}

/// Set the logger up. Called once, before anything else logs. `first` says
/// whether this is the only ClippiBoy running; a second start logs to stderr
/// only.
pub fn init(first: bool) {
    // iroh (sending clips, `share.rs`) logs every UDP packet at info — a
    // transfer would bury everything else. Its warnings are what matters.
    const FILTER: &str = "info,iroh=warn,iroh_relay=warn,noq=warn,noq_proto=warn,noq_udp=warn,netwatch=warn,portmapper=warn,tracing::span=warn";
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(FILTER))
        // Both copies come off one writer, so colour would land in the file as
        // escape sequences. A log somebody has to read in Notepad is worth more
        // than a coloured level on a console the release build does not have.
        .write_style(env_logger::WriteStyle::Never)
        .target(env_logger::Target::Pipe(Box::new(Fork)))
        .init();

    if first {
        attach_file();
    } else {
        log::info!("another ClippiBoy is running — this start logs to stderr only");
    }

    // The release build aborts on a panic, and the message went to the stderr
    // it does not have: a tester saw the window vanish and the log simply
    // stopped. Now the reason is the log's last line. Location survives
    // `strip`; the message says what went wrong.
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        log::error!("panic on thread '{}': {info}", thread.name().unwrap_or("unnamed"));
        log::logger().flush();
        default(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log() -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("cb-log-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clippiboy.log");
        (dir, path)
    }

    #[test]
    fn a_second_start_leaves_both_logs_alone() {
        let (dir, path) = temp_log();
        std::fs::write(&path, "live").unwrap();
        std::fs::write(path.with_extension("log.1"), "previous").unwrap();
        assert!(open_for(&path, false).is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "live");
        assert_eq!(std::fs::read_to_string(path.with_extension("log.1")).unwrap(), "previous");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_first_start_keeps_one_generation() {
        let (dir, path) = temp_log();
        std::fs::write(&path, "old").unwrap();
        drop(open_for(&path, true).unwrap());
        assert_eq!(std::fs::read_to_string(path.with_extension("log.1")).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_first_start_makes_the_folder() {
        let (dir, _) = temp_log();
        let path = dir.join("logs").join("clippiboy.log");
        drop(open_for(&path, true).unwrap());
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
