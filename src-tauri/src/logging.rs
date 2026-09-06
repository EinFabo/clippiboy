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

use std::io::Write;
use std::path::PathBuf;

/// Where the logs live. Next to the configuration, so "the ClippiBoy folder"
/// stays one place a person has to find.
pub fn log_dir() -> PathBuf {
    crate::config::data_dir().join("logs")
}

pub fn log_path() -> PathBuf {
    log_dir().join("clippiboy.log")
}

/// Stderr and the file, from one stream of bytes.
struct Fork {
    file: Option<std::fs::File>,
}

impl Write for Fork {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Neither copy may take the other down: a full disk must not silence
        // the console, and a closed console must not stop the file.
        let _ = std::io::stderr().write_all(buf);
        if let Some(file) = &mut self.file {
            let _ = file.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stderr().flush();
        if let Some(file) = &mut self.file {
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
fn rotate(path: &std::path::Path) {
    if path.exists() {
        let _ = std::fs::rename(path, path.with_extension("log.1"));
    }
}

/// Set the logger up. Called once, before anything else logs.
pub fn init() {
    let path = log_path();
    let file = match std::fs::create_dir_all(log_dir()) {
        Ok(()) => {
            rotate(&path);
            std::fs::File::create(&path).ok()
        }
        Err(_) => None,
    };
    let opened = file.is_some();

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        // Both copies come off one writer, so colour would land in the file as
        // escape sequences. A log somebody has to read in Notepad is worth more
        // than a coloured level on a console the release build does not have.
        .write_style(env_logger::WriteStyle::Never)
        .target(env_logger::Target::Pipe(Box::new(Fork { file })))
        .init();

    if opened {
        log::info!("log file: {}", path.display());
    } else {
        log::warn!("no log file at {} — stderr only", path.display());
    }
}
