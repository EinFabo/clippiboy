//! ffmpeg and ffprobe as child processes: tied to the app's life and given a
//! limit.
//!
//! A hung ffmpeg used to hold its caller for good. A save holds the one
//! `saving` flag for its whole run (`state.rs`), so every later save was
//! refused until a restart; and an ffmpeg left behind by a crash ran on as an
//! orphan. Now every child goes into one job object that ends it with the app,
//! and every run has a [`Limit`]: a fixed time for what is quick anyway
//! (probes, thumbnails), and for work that reports `-progress` no time at all
//! while it moves on, only a cap on how long it may stand still.

use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// How long a run may take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// The whole run, from start to exit.
    Total(Duration),
    /// No end while `-progress` moves on ([`PROGRESS`] among the arguments);
    /// ended after this long without a step forward (see [`Watchdog`]).
    Stall(Duration),
}

/// ffprobe and `-version`: they read a header and answer. Generous for a
/// first start, when an antivirus scans the 100 MB program before it runs.
pub const PROBE: Limit = Limit::Total(Duration::from_secs(30));

/// One picture out of a clip: a thumbnail.
pub const STILL: Limit = Limit::Total(Duration::from_secs(60));

/// The longest a run reporting `-progress` may stand still.
pub const STALL: Duration = Duration::from_secs(120);

/// Work that reports `-progress`.
pub const MOVING: Limit = Limit::Stall(STALL);

/// The arguments [`MOVING`] needs: progress on stdout, no stats on stderr.
pub const PROGRESS: [&str; 3] = ["-progress", "pipe:1", "-nostats"];

/// What stdout keeps for the caller. ffprobe's answers are a few hundred
/// bytes; an hour of `-progress` is megabytes nobody reads.
const KEEP_STDOUT: usize = 1 << 20;

/// How often a run looks at its clock and at `stop` while waiting.
const TICK: Duration = Duration::from_millis(100);

/// A fixed limit for work that reports nothing on the way and takes about as
/// long as the media it reads: the waveform of a whole clip, say. Five times
/// its length, and never under two minutes.
pub fn deadline_for(length_ms: u64) -> Limit {
    Limit::Total(Duration::from_millis(length_ms.saturating_mul(5)).max(Duration::from_secs(120)))
}

/// Decides when a run has taken too long. Times count from its start.
#[derive(Debug)]
pub struct Watchdog {
    limit: Limit,
    /// When the output last moved on.
    moved_at: Duration,
    /// How far the output has got, in µs; `None` before the first report.
    written_us: Option<i64>,
}

impl Watchdog {
    pub fn new(limit: Limit) -> Self {
        Self { limit, moved_at: Duration::ZERO, written_us: None }
    }

    /// ffmpeg says its output has reached `out_us`. Only a step forward
    /// counts: a stuck encoder goes on printing the same time. True when it
    /// was one.
    pub fn progress(&mut self, at: Duration, out_us: i64) -> bool {
        if self.written_us.is_some_and(|written| out_us <= written) {
            return false;
        }
        self.written_us = Some(out_us);
        self.moved_at = at;
        true
    }

    /// Has the run outstayed its limit at `at`?
    pub fn expired(&self, at: Duration) -> bool {
        match self.limit {
            Limit::Total(limit) => at >= limit,
            Limit::Stall(limit) => at.saturating_sub(self.moved_at) >= self.allowance(limit),
        }
    }

    /// How long the run may stand still now. After the last frame ffmpeg
    /// writes the file's index and, with `+faststart`, reads the whole file
    /// through once more to put it in front — minutes for an hour's recording
    /// on a slow drive, and no progress on the way. So the more is written,
    /// the longer the wait: half the output's length, never under `stall`.
    fn allowance(&self, stall: Duration) -> Duration {
        let written = self.written_us.unwrap_or(0).max(0) as u64;
        stall.max(Duration::from_micros(written / 2))
    }
}

/// The output time of one `-progress` line, in µs.
pub fn progress_of(line: &[u8]) -> Option<i64> {
    let text = std::str::from_utf8(line).ok()?;
    text.trim().strip_prefix("out_time_us=")?.parse().ok()
}

/// May `program` be started? Only by a full path, unless a search of the
/// PATH is allowed: a release build runs its own pinned, checked ffmpeg or
/// none at all, never whichever one some other program put on the PATH.
pub fn may_start(program: &Path, search_allowed: bool) -> bool {
    search_allowed || program.is_absolute()
}

/// Start `command` and tie it to the app's life. For a child that lives as
/// long as its caller wants (a camera); everything else goes through [`run`].
pub fn spawn(command: &mut Command) -> io::Result<Child> {
    if !may_start(Path::new(command.get_program()), cfg!(debug_assertions)) {
        return Err(io::Error::new(io::ErrorKind::NotFound, "ffmpeg is not ready yet"));
    }
    let child = command.spawn()?;
    #[cfg(windows)]
    job::tie(&child);
    Ok(child)
}

/// [`run`] for a caller that only wants the result.
pub fn output(command: &mut Command, limit: Limit) -> io::Result<Output> {
    run(command, limit, &|_| {}, &|| false)
}

/// Run `command` to its end within `limit`, with no input, its stdout and
/// stderr collected. `on_progress` hears every step forward of a
/// `-progress` report (the output time in µs); `stop` is asked all along and
/// ends the run when it says so.
///
/// A run that outstays its limit is killed and comes back as `TimedOut`, one
/// that was stopped as `Interrupted` ([`is_stopped`]).
pub fn run(
    command: &mut Command,
    limit: Limit,
    on_progress: &dyn Fn(i64),
    stop: &dyn Fn() -> bool,
) -> io::Result<Output> {
    let name = Path::new(command.get_program())
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let started = Instant::now();
    let mut child = spawn(command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()))?;

    // Both pipes drained on threads of their own: a full one stalls ffmpeg.
    let stderr = child.stderr.take();
    let errors = std::thread::spawn(move || {
        let mut text = Vec::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_end(&mut text);
        }
        text
    });
    let stdout = child.stdout.take();
    let (tx, rx) = mpsc::channel::<i64>();
    let reader = std::thread::spawn(move || {
        let mut kept = Vec::new();
        let Some(stdout) = stdout else {
            return kept;
        };
        let mut stdout = BufReader::new(stdout);
        let mut line = Vec::new();
        loop {
            line.clear();
            match stdout.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if let Some(us) = progress_of(&line) {
                let _ = tx.send(us);
            }
            if kept.len() < KEEP_STDOUT {
                kept.extend_from_slice(&line);
            }
        }
        kept
    });

    let mut watchdog = Watchdog::new(limit);
    let mut reporting = true;
    let ended = loop {
        if reporting {
            match rx.recv_timeout(TICK) {
                Ok(us) => {
                    if watchdog.progress(started.elapsed(), us) {
                        on_progress(us);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                // Stdout closed: the program is ending.
                Err(RecvTimeoutError::Disconnected) => reporting = false,
            }
        } else {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(err) => break Err(err),
            }
        }
        if stop() {
            break Err(io::Error::new(io::ErrorKind::Interrupted, STOPPED));
        }
        if watchdog.expired(started.elapsed()) {
            let words = match limit {
                Limit::Total(limit) => format!("did not finish within {} s", limit.as_secs()),
                Limit::Stall(_) => format!("stood still for {} s", started.elapsed().saturating_sub(watchdog.moved_at).as_secs()),
            };
            break Err(io::Error::new(io::ErrorKind::TimedOut, format!("{name} {words} and was ended")));
        }
    };
    if ended.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    // The pipes are closed now that the program is gone.
    let stdout = reader.join().unwrap_or_default();
    let stderr = errors.join().unwrap_or_default();
    Ok(Output { status: ended?, stdout, stderr })
}

/// What [`run`] says when `stop` ended it.
const STOPPED: &str = "stopped";

/// Was this run ended by its `stop`?
pub fn is_stopped(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::Interrupted && err.to_string() == STOPPED
}

/// One job object for the whole app, set to end everything in it when its
/// last handle closes — which the app's own exit or crash does.
#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;

    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// The job's handle, kept as a number (a raw handle is not `Sync`).
    /// Never closed: it closes with the process.
    static JOB: OnceLock<Option<usize>> = OnceLock::new();

    fn job() -> Option<HANDLE> {
        let handle = JOB.get_or_init(|| match create() {
            Ok(job) => Some(job.0 as usize),
            Err(err) => {
                log::warn!("no job object for ffmpeg, it may outlive the app: {err}");
                None
            }
        });
        handle.map(|handle| HANDLE(handle as *mut _))
    }

    pub(super) fn create() -> windows::core::Result<HANDLE> {
        // SAFETY: plain Win32 calls on a handle created here; the limits
        // struct lives across the call that reads it.
        unsafe {
            let job = CreateJobObjectW(None, windows::core::PCWSTR::null())?;
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if let Err(err) = set {
                let _ = CloseHandle(job);
                return Err(err);
            }
            Ok(job)
        }
    }

    pub(super) fn assign(job: HANDLE, child: &Child) -> windows::core::Result<()> {
        // SAFETY: both handles are open: the job as long as the caller keeps
        // it, the process while `child` is borrowed.
        unsafe { AssignProcessToJobObject(job, HANDLE(child.as_raw_handle())) }
    }

    /// Put `child` into the job. Said once in the log when it cannot be.
    pub fn tie(child: &Child) {
        static WARNED: AtomicBool = AtomicBool::new(false);
        let Some(job) = job() else {
            return;
        };
        if let Err(err) = assign(job, child) {
            if !WARNED.swap(true, Ordering::Relaxed) {
                log::warn!("ffmpeg not tied to the app, it may outlive it: {err}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: fn(u64) -> Duration = Duration::from_secs;

    #[test]
    fn a_fixed_limit_ends_the_run_at_its_time_whatever_it_reports() {
        let mut watchdog = Watchdog::new(Limit::Total(S(30)));
        assert!(!watchdog.expired(S(29)));
        assert!(watchdog.progress(S(29), 5_000_000));
        assert!(watchdog.expired(S(30)));
    }

    #[test]
    fn a_run_that_moves_on_has_no_end() {
        let mut watchdog = Watchdog::new(Limit::Stall(S(120)));
        for minute in 1..=600u64 {
            assert!(watchdog.progress(S(minute * 60), minute as i64 * 60_000_000));
            assert!(!watchdog.expired(S(minute * 60 + 59)), "minute {minute}");
        }
    }

    #[test]
    fn a_run_that_never_reports_is_ended_after_the_stall() {
        let watchdog = Watchdog::new(Limit::Stall(S(120)));
        assert!(!watchdog.expired(S(119)));
        assert!(watchdog.expired(S(120)));
    }

    #[test]
    fn the_same_time_again_is_no_progress() {
        let mut watchdog = Watchdog::new(Limit::Stall(S(120)));
        assert!(watchdog.progress(S(5), 0), "the first report is a sign of life");
        assert!(watchdog.progress(S(10), 3_000_000));
        // A stuck encoder repeats its last time twice a second.
        for second in 11..200 {
            assert!(!watchdog.progress(S(second), 3_000_000));
        }
        assert!(!watchdog.expired(S(129)));
        assert!(watchdog.expired(S(130)));
    }

    /// The index at the end, and `+faststart` reading the file through once
    /// more, report nothing: an hour written may wait half an hour.
    #[test]
    fn the_more_is_written_the_longer_the_end_may_take() {
        let mut watchdog = Watchdog::new(Limit::Stall(S(120)));
        assert!(watchdog.progress(S(600), 3_600_000_000));
        assert!(!watchdog.expired(S(600 + 1_799)));
        assert!(watchdog.expired(S(600 + 1_800)));
        // A short clip keeps the plain stall.
        let mut watchdog = Watchdog::new(Limit::Stall(S(120)));
        assert!(watchdog.progress(S(5), 30_000_000));
        assert!(watchdog.expired(S(125)));
    }

    #[test]
    fn a_negative_start_counts_as_a_sign_of_life() {
        // The AAC priming puts the first output time a hair before zero.
        let mut watchdog = Watchdog::new(Limit::Stall(S(120)));
        assert!(watchdog.progress(S(1), -21_333));
        assert!(!watchdog.expired(S(120)));
        assert!(watchdog.expired(S(121)));
    }

    #[test]
    fn work_without_reports_gets_five_times_its_length() {
        assert_eq!(deadline_for(0), Limit::Total(S(120)));
        assert_eq!(deadline_for(10_000), Limit::Total(S(120)));
        assert_eq!(deadline_for(60_000), Limit::Total(S(300)));
        assert_eq!(deadline_for(3_600_000), Limit::Total(S(18_000)));
        assert_eq!(deadline_for(u64::MAX), Limit::Total(Duration::from_millis(u64::MAX)));
    }

    #[test]
    fn progress_is_read_off_out_time_us() {
        assert_eq!(progress_of(b"out_time_us=1500000\n"), Some(1_500_000));
        assert_eq!(progress_of(b"out_time_us=1500000\r\n"), Some(1_500_000));
        assert_eq!(progress_of(b"out_time_us=-21333\n"), Some(-21_333));
        assert_eq!(progress_of(b"out_time_us=N/A\n"), None);
        assert_eq!(progress_of(b"out_time_ms=1500000\n"), None);
        assert_eq!(progress_of(b"out_time=00:00:01.500000\n"), None);
        assert_eq!(progress_of(&[0xff, 0xfe]), None);
    }

    #[test]
    fn a_release_starts_tools_only_by_their_full_path() {
        let full = if cfg!(windows) { "C:\\data\\ffmpeg\\9.0.1\\ffmpeg.exe" } else { "/data/ffmpeg/ffmpeg" };
        assert!(may_start(Path::new(full), false));
        assert!(!may_start(Path::new("ffmpeg"), false));
        assert!(!may_start(Path::new("ffmpeg.exe"), false));
        assert!(may_start(Path::new("ffmpeg"), true));
    }

    #[test]
    fn the_progress_arguments_report_on_stdout() {
        assert_eq!(PROGRESS, ["-progress", "pipe:1", "-nostats"]);
        assert_eq!(MOVING, Limit::Stall(STALL));
    }

    #[cfg(windows)]
    fn ping(seconds: u32) -> Command {
        let mut ping = Command::new("ping");
        ping.args(["-n", &seconds.to_string(), "127.0.0.1"]);
        ping
    }

    /// A run past its limit is ended and says so; one within it is waited for.
    #[cfg(windows)]
    #[test]
    fn a_run_past_its_limit_is_ended() {
        let started = Instant::now();
        let err = output(&mut ping(30), Limit::Total(Duration::from_millis(500))).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
        assert!(started.elapsed() < S(10), "{:?}", started.elapsed());

        let done = output(&mut ping(1), Limit::Total(S(20))).unwrap();
        assert!(done.status.success());
        assert!(String::from_utf8_lossy(&done.stdout).contains("127.0.0.1"));
    }

    /// Ping prints no `-progress`, so it stands still from the start.
    #[cfg(windows)]
    #[test]
    fn a_run_without_progress_is_ended_after_its_stall() {
        let started = Instant::now();
        let err = output(&mut ping(30), Limit::Stall(Duration::from_millis(500))).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
        assert!(started.elapsed() < S(10), "{:?}", started.elapsed());
    }

    /// The job's one promise: when its last handle closes — the app ends or
    /// crashes — what is in it ends too. Shown on a job of the test's own.
    #[cfg(windows)]
    #[test]
    fn closing_the_job_ends_its_children() {
        let job = job::create().unwrap();
        let mut child = ping(30).stdout(Stdio::null()).spawn().unwrap();
        job::assign(job, &child).unwrap();
        // SAFETY: the job was created above and is not used after this.
        unsafe { windows::Win32::Foundation::CloseHandle(job).unwrap() };
        let started = Instant::now();
        while child.try_wait().unwrap().is_none() {
            assert!(started.elapsed() < S(10), "the child outlived its job");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Against the ffmpeg ClippiBoy fetched, in a scratch folder: a run that
    /// moves on reports and finishes, one whose output stands still is ended,
    /// and a probe of a clip answers through the whitelist.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn the_real_ffmpeg_moves_on_or_is_ended() {
        let Some(local) = std::env::var_os("LOCALAPPDATA") else { return };
        let tools = std::path::PathBuf::from(local).join("gg.clippiboy.app").join("ffmpeg").join("9.0.1");
        assert!(tools.join("ffmpeg.exe").is_file(), "no ffmpeg in {}", tools.display());
        crate::muxer::set_tool_dir(tools);
        let dir = std::env::temp_dir().join(format!("clippiboy-child-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("clip.mp4");

        let reports = std::cell::Cell::new(0u32);
        let last = std::cell::Cell::new(0i64);
        let mut encode = crate::muxer::ffmpeg();
        encode
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .args(PROGRESS)
            .args(["-f", "lavfi", "-i", "testsrc=size=640x360:rate=30", "-f", "lavfi", "-i", "sine"])
            .args(["-t", "10", "-c:v", "libx264", "-preset", "ultrafast", "-c:a", "aac", "-movflags", "+faststart"])
            .arg(&clip);
        let done = run(&mut encode, MOVING, &|us| {
            reports.set(reports.get() + 1);
            last.set(us);
        }, &|| false)
        .unwrap();
        assert!(done.status.success(), "{}", String::from_utf8_lossy(&done.stderr));
        assert!(reports.get() > 0 && last.get() >= 9_000_000, "{} reports, last {}", reports.get(), last.get());
        assert_eq!(crate::muxer::probe_duration(&clip).unwrap().map(|ms| ms / 100), Some(100));

        // Endless input, nothing let through to the output: it stands still.
        let started = Instant::now();
        let mut stuck = crate::muxer::ffmpeg();
        stuck
            .args(["-hide_banner", "-loglevel", "error"])
            .args(PROGRESS)
            .args(["-re", "-f", "lavfi", "-i", "testsrc=rate=30", "-vf", "select=0", "-f", "null", "-"]);
        let err = output(&mut stuck, Limit::Stall(S(3))).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
        assert!(started.elapsed() < S(8), "{:?}", started.elapsed());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn a_stopped_run_says_so() {
        let err = run(&mut ping(30), Limit::Total(S(60)), &|_| {}, &|| true).unwrap_err();
        assert!(is_stopped(&err), "{err}");
    }
}
