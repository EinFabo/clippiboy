//! Collects everything needed to diagnose a ClippiBoy problem into one file.
//!
//! Made for the case where the person seeing the fault is not the person who
//! can read a log: they run this once, and send back the one file it writes.
//!
//! It only reads. Nothing is installed, nothing is changed, nothing leaves the
//! machine — the file lands on the desktop and the person decides what happens
//! to it. The Stream Deck token is the one thing taken out on the way, because
//! a control port token in a file passed around a chat is a key handed over.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let mut out = String::with_capacity(256 * 1024);
    let started = std::time::Instant::now();

    println!("Creating the ClippiBoy report, one moment ...");

    header(&mut out);
    system(&mut out);
    installation(&mut out);
    autostart(&mut out);
    configuration(&mut out);
    ffmpeg(&mut out);
    monitors(&mut out);
    event_log(&mut out);
    let logs = read_logs();
    frozen_capture(&mut out, &logs);
    raw_logs(&mut out, &logs);

    let _ = writeln!(
        out,
        "\n--- End of report (created in {:.1} s) ---",
        started.elapsed().as_secs_f32()
    );

    let path = destination();
    match std::fs::write(&path, out.as_bytes()) {
        Ok(()) => {
            println!("\nDone. The file is here:\n\n    {}\n", path.display());
            println!("Please send back this one file.");
        }
        Err(err) => {
            eprintln!("\nThe file could not be written: {err}");
            eprintln!("Path was: {}", path.display());
        }
    }

    // Double-clicked, the window would otherwise close before anything could be
    // read off it.
    println!("\n[Press Enter to close]");
    let mut wait = String::new();
    let _ = std::io::stdin().read_line(&mut wait);
}

/// Where the report goes: the desktop, or beside the program if there is none.
///
/// Windows is asked rather than `%USERPROFILE%\Desktop` being assumed. With
/// OneDrive the desktop is redirected — `C:\Users\x\OneDrive\Desktop` —
/// and the old folder stays behind, empty and never opened. A report written
/// there is a report nobody finds; that happened on the first machine this ran
/// on.
fn destination() -> PathBuf {
    let name = "clippiboy-report.txt";
    let asked = powershell("[Environment]::GetFolderPath('Desktop')");
    let asked = PathBuf::from(asked.trim());
    if asked.is_dir() {
        return asked.join(name);
    }
    if let Some(desktop) = std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("Desktop"))
    {
        if desktop.is_dir() {
            return desktop.join(name);
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

/// ClippiBoy's own folder under AppData\Roaming.
fn data_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("ClippiBoy"))
}

fn section(out: &mut String, title: &str) {
    let _ = write!(out, "\n\n=== {title} ===\n\n");
}

/// Run a PowerShell one-liner and give back what it printed.
///
/// Every question Windows answers rather than the file system goes through
/// here. `-NoProfile` because a profile that prints a banner would land in the
/// middle of the report.
fn powershell(script: &str) -> String {
    match Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
    {
        Ok(done) => {
            let text = String::from_utf8_lossy(&done.stdout).trim_end().to_string();
            if text.is_empty() {
                let err = String::from_utf8_lossy(&done.stderr).trim_end().to_string();
                if err.is_empty() {
                    "(no output)".into()
                } else {
                    format!("(error) {err}")
                }
            } else {
                text
            }
        }
        Err(err) => format!("(PowerShell not available: {err})"),
    }
}

fn header(out: &mut String) {
    let _ = write!(
        out,
        "ClippiBoy — diagnostic report\n\
         Created:  {}\n\
         Computer: {}\n\
         User:     {}\n",
        powershell("Get-Date -Format 'yyyy-MM-dd HH:mm:ss zzz'"),
        std::env::var("COMPUTERNAME").unwrap_or_else(|_| "?".into()),
        std::env::var("USERNAME").unwrap_or_else(|_| "?".into()),
    );
}

fn system(out: &mut String) {
    section(out, "System");
    let _ = writeln!(
        out,
        "{}",
        powershell(
            "$os = Get-CimInstance Win32_OperatingSystem; \
             $cs = Get-CimInstance Win32_ComputerSystem; \
             'Windows:   ' + $os.Caption + ' (Build ' + $os.BuildNumber + ')'; \
             'Memory:    ' + [math]::Round($cs.TotalPhysicalMemory/1GB,1) + ' GB'; \
             'CPU:       ' + (Get-CimInstance Win32_Processor | Select-Object -First 1).Name; \
             Get-CimInstance Win32_VideoController | ForEach-Object { \
                'GPU:       ' + $_.Name + ' (driver ' + $_.DriverVersion + ', ' + $_.DriverDate + ')' }"
        )
    );
}

fn installation(out: &mut String) {
    section(out, "Installation");
    let _ = writeln!(
        out,
        "{}",
        powershell(
            "$exe = Join-Path $env:LOCALAPPDATA 'ClippiBoy\\clippiboy.exe'; \
             if (Test-Path $exe) { \
                $i = Get-Item $exe; \
                'Program:   ' + $exe; \
                'Version:   ' + $i.VersionInfo.FileVersion; \
                'Modified:  ' + $i.LastWriteTime; \
                'Size:      ' + [math]::Round($i.Length/1MB,1) + ' MB' \
             } else { 'Program not found at ' + $exe }; \
             $p = Get-Process clippiboy -ErrorAction SilentlyContinue; \
             if ($p) { \
                foreach ($q in $p) { \
                  'Running:   PID ' + $q.Id + ', since ' + $q.StartTime + ', responding: ' + $q.Responding } \
             } else { 'Running:   no' }"
        )
    );
}

fn autostart(out: &mut String) {
    section(out, "Autostart");
    let _ = writeln!(
        out,
        "{}",
        powershell(
            "$run = 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Run'; \
             $v = (Get-ItemProperty -Path $run -ErrorAction SilentlyContinue).PSObject.Properties | \
                  Where-Object { $_.Name -match 'clippi' }; \
             if ($v) { foreach ($e in $v) { 'Registry:  ' + $e.Name + ' = ' + $e.Value } } \
             else { 'Registry:  no entry' }; \
             $sf = Join-Path $env:APPDATA 'Microsoft\\Windows\\Start Menu\\Programs\\Startup'; \
             $f = Get-ChildItem $sf -ErrorAction SilentlyContinue | Where-Object { $_.Name -match 'clippi' }; \
             if ($f) { foreach ($e in $f) { 'Startup folder: ' + $e.Name } } \
             else { 'Startup folder: no entry' }"
        )
    );
}

/// The settings, with the control port token taken out.
fn configuration(out: &mut String) {
    section(out, "Settings (config.json)");
    let Some(path) = data_dir().map(|d| d.join("config.json")) else {
        let _ = writeln!(out, "APPDATA not found.");
        return;
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let _ = writeln!(out, "{}\n", redact_token(&text));
        }
        Err(err) => {
            let _ = writeln!(out, "{} could not be read: {err}", path.display());
        }
    }
}

/// Replace the value of `"token": "..."` with a placeholder.
///
/// Deliberately not a JSON parse: the file must go into the report exactly as
/// it is, comments, order and all, so that what is read here is what is on the
/// disk. Only the one value is cut out.
fn redact_token(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("\"token\"") {
        let (before, after) = rest.split_at(at);
        out.push_str(before);
        // Everything up to the opening quote of the value stays as it is.
        let Some(colon) = after.find(':') else {
            out.push_str(after);
            return out;
        };
        let (key, value) = after.split_at(colon + 1);
        out.push_str(key);
        let Some(open) = value.find('"') else {
            out.push_str(value);
            return out;
        };
        out.push_str(&value[..open]);
        let tail = &value[open + 1..];
        let Some(close) = tail.find('"') else {
            out.push_str(tail);
            return out;
        };
        let len = tail[..close].len();
        let _ = write!(out, "\"(removed, {len} characters)\"");
        rest = &tail[close + 1..];
    }
    out.push_str(rest);
    out
}

fn ffmpeg(out: &mut String) {
    section(out, "ffmpeg");
    let Some(root) = std::env::var_os("LOCALAPPDATA")
        .map(|p| PathBuf::from(p).join("gg.clippiboy.app").join("ffmpeg"))
    else {
        let _ = writeln!(out, "LOCALAPPDATA not found.");
        return;
    };
    if !root.is_dir() {
        let _ = writeln!(
            out,
            "Folder {} does not exist — ffmpeg was never downloaded.",
            root.display()
        );
        return;
    }
    let _ = writeln!(out, "Folder: {}", root.display());
    list_tree(out, &root, 0);
}

fn list_tree(out: &mut String, dir: &Path, depth: usize) {
    if depth > 2 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let pad = "  ".repeat(depth + 1);
        match entry.metadata() {
            Ok(meta) if meta.is_dir() => {
                let _ = writeln!(out, "{pad}{name}/");
                list_tree(out, &entry.path(), depth + 1);
            }
            Ok(meta) => {
                let _ = writeln!(out, "{pad}{name}  ({} Bytes)", meta.len());
            }
            Err(_) => {
                let _ = writeln!(out, "{pad}{name}  (not readable)");
            }
        }
    }
}

fn monitors(out: &mut String) {
    section(out, "Displays");
    let _ = writeln!(
        out,
        "{}",
        powershell(
            "Add-Type -AssemblyName System.Windows.Forms; \
             [System.Windows.Forms.Screen]::AllScreens | ForEach-Object { \
                $_.DeviceName + '  ' + $_.Bounds.Width + 'x' + $_.Bounds.Height + \
                ' at ' + $_.Bounds.X + ',' + $_.Bounds.Y + \
                (&{ if ($_.Primary) { '  (primary)' } else { '' } }) }"
        )
    );
}

fn event_log(out: &mut String) {
    section(out, "Windows event log (crashes and hangs)");
    let _ = writeln!(
        out,
        "{}",
        powershell(
            "$ErrorActionPreference = 'SilentlyContinue'; \
             $e = Get-WinEvent -FilterHashtable @{LogName='Application'; StartTime=(Get-Date).AddDays(-30)}; \
             $hits = $e | Where-Object { $_.Message -match 'clippiboy' }; \
             if (-not $hits) { 'Nothing in the last 30 days.' } else { \
               $hits | ForEach-Object { \
                 $name = 'Event ' + $_.Id; \
                 if ($_.Message -match '(?:Ereignisname|Event Name):\\s*(\\w+)') { $name = $Matches[1] } \
                 elseif ($_.Message -match 'Faulting application name') { $name = 'AppCrash' }; \
                 $ver = ''; if ($_.Message -match 'P2:\\s*([\\d\\.]+)') { $ver = ' ver=' + $Matches[1] }; \
                 $mod = ''; if ($_.Message -match 'Faulting module name:\\s*(\\S+)') { $mod = ' module=' + $Matches[1] }; \
                 $_.TimeCreated.ToString('yyyy-MM-dd HH:mm:ss') + '  ' + $name + $ver + $mod } }"
        )
    );
}

/// One log file, newest last.
struct LogFile {
    name: String,
    text: String,
}

fn read_logs() -> Vec<LogFile> {
    let Some(dir) = data_dir().map(|d| d.join("logs")) else {
        return Vec::new();
    };
    // `.log.1` is the older half; reading it first puts the report in the order
    // things happened.
    ["clippiboy.log.1", "clippiboy.log"]
        .iter()
        .filter_map(|name| {
            std::fs::read_to_string(dir.join(name))
                .ok()
                .map(|text| LogFile {
                    name: (*name).to_string(),
                    text,
                })
        })
        .collect()
}

/// One `pipeline:` line, reduced to the three numbers that matter.
struct Tick {
    stamp: String,
    frames: u64,
    repeated: u64,
    dropped: u64,
}

fn parse_ticks(text: &str) -> Vec<Tick> {
    text.lines()
        .filter(|line| line.contains("pipeline:"))
        .filter_map(|line| {
            Some(Tick {
                stamp: line
                    .trim_start_matches('[')
                    .split(' ')
                    .next()
                    .unwrap_or("?")
                    .to_string(),
                frames: field(line, "frames=")?,
                repeated: field(line, "repeated=")?,
                dropped: field(line, "dropped=")?,
            })
        })
        .collect()
}

/// The number right after `key` in a pipeline line.
fn field(line: &str, key: &str) -> Option<u64> {
    let at = line.find(key)? + key.len();
    let rest = &line[at..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// The question this whole program exists for: did the picture stand still?
///
/// Windows.Graphics.Capture only hands over a frame when something on the
/// screen changed, so the pipeline repeating its last picture is normal for a
/// desktop nobody is touching — on a machine where the recorded screen is not
/// the one being worked on, stretches of minutes are ordinary and mean nothing.
///
/// What tells the two apart is not the repeating but the *not recovering*. A
/// capture bound to a screen that no longer exists — the lock screen being the
/// case this was written for — never delivers anything again; it runs to the
/// end of the log. So the long stretches are listed longest first, and the one
/// the log ends inside is called out, because that is the only one that
/// answers the question.
fn frozen_capture(out: &mut String, logs: &[LogFile]) {
    section(out, "Frozen picture? (the most important question)");

    let mut any = false;
    for log in logs {
        let ticks = parse_ticks(&log.text);
        if ticks.len() < 2 {
            continue;
        }
        let _ = writeln!(out, "-- {} ({} samples)", log.name, ticks.len());

        // Stretches of consecutive intervals in which nearly every frame was a
        // repeat, collected first and judged afterwards.
        let mut run: Option<Run> = None;
        let mut runs: Vec<Run> = Vec::new();
        let last_stamp = ticks.last().map(|t| t.stamp.clone()).unwrap_or_default();
        for pair in ticks.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            // The counters start over when the pipeline is restarted. That
            // ends whatever stretch was open — the new capture is a new
            // question.
            if b.frames < a.frames || b.repeated < a.repeated {
                runs.extend(run.take());
                continue;
            }
            let frames = b.frames - a.frames;
            let repeated = b.repeated - a.repeated;
            let frozen = frames > 0 && repeated * 100 >= frames * 95;
            match (frozen, run.as_mut()) {
                (true, Some(open)) => {
                    open.to = b.stamp.clone();
                    open.frames += frames;
                    open.repeated += repeated;
                }
                (true, None) => {
                    run = Some(Run {
                        from: a.stamp.clone(),
                        to: b.stamp.clone(),
                        frames,
                        repeated,
                    });
                }
                (false, Some(_)) => runs.extend(run.take()),
                (false, None) => {}
            }
        }
        runs.extend(run.take());

        // Short ones are the desktop sitting still and say nothing; they would
        // only bury the one line that matters.
        runs.retain(|r| r.seconds() >= 60.0);
        runs.sort_by(|a, b| b.seconds().total_cmp(&a.seconds()));

        let reaches_end = runs.iter().any(|r| r.to == last_stamp);
        if runs.is_empty() {
            let _ = writeln!(out, "   No stretch longer than a minute.");
        } else {
            let _ = writeln!(
                out,
                "   Longest stretches with a frozen picture (over a minute), longest first:"
            );
            for r in runs.iter().take(10) {
                report_run(out, r, r.to == last_stamp);
            }
            if runs.len() > 10 {
                let _ = writeln!(out, "   ... and {} more.", runs.len() - 10);
            }
        }
        if reaches_end {
            let _ = writeln!(
                out,
                "   >>> The log ends IN THE MIDDLE of such a stretch. The capture never\n\
                 \x20      recovered — this is the bug we are looking for."
            );
        } else {
            let _ = writeln!(
                out,
                "   The capture recovered every time. Frozen stretches are then just a\n\
                 \x20  display on which nothing happens — not a bug."
            );
        }
        any = true;

        let last = ticks.last().unwrap();
        let _ = writeln!(
            out,
            "   Total up to the end: {} frames, {} of them repeated, {} dropped.",
            last.frames, last.repeated, last.dropped
        );
    }

    if !any {
        let _ = writeln!(out, "No pipeline lines found in the logs.");
    }
}

/// One stretch in which the picture stood still.
struct Run {
    from: String,
    to: String,
    frames: u64,
    repeated: u64,
}

impl Run {
    /// How long it lasted, from the two stamps.
    ///
    /// The stamps are the log's own `2026-09-24T16:42:33Z`, so the clock part
    /// is at a fixed place and needs no date library. Across midnight a day is
    /// added back on.
    fn seconds(&self) -> f64 {
        let (Some(a), Some(b)) = (clock(&self.from), clock(&self.to)) else {
            return 0.0;
        };
        let d = b - a;
        if d < 0.0 {
            d + 86400.0
        } else {
            d
        }
    }
}

/// Seconds since midnight out of `...THH:MM:SSZ`.
fn clock(stamp: &str) -> Option<f64> {
    let time = stamp.split('T').nth(1)?.trim_end_matches('Z');
    let mut parts = time.split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let s: f64 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + s)
}

fn report_run(out: &mut String, run: &Run, reaches_end: bool) {
    let share = if run.frames > 0 {
        run.repeated as f64 * 100.0 / run.frames as f64
    } else {
        0.0
    };
    let secs = run.seconds();
    let mark = if reaches_end { "  <== until the end" } else { "" };
    let _ = writeln!(
        out,
        "     {:>6.0} s   {} to {}   {} frames, {:.0} % repeated{}",
        secs, run.from, run.to, run.frames, share, mark
    );
}

fn raw_logs(out: &mut String, logs: &[LogFile]) {
    for log in logs {
        section(out, &format!("Log: {} — events", log.name));
        let events: Vec<&str> = log
            .text
            .lines()
            .filter(|line| !line.contains("pipeline:") && !line.starts_with("      "))
            .collect();
        if events.is_empty() {
            let _ = writeln!(out, "(nothing but pipeline lines)");
        } else {
            let _ = writeln!(out, "{}", events.join("\n"));
        }

        // The pipeline lines are the bulk of the file and say the same thing
        // over and over. The tail is enough to see the state it ended in.
        section(out, &format!("Log: {} — last pipeline lines", log.name));
        let ticks: Vec<&str> = log
            .text
            .lines()
            .filter(|line| line.contains("pipeline:"))
            .collect();
        let from = ticks.len().saturating_sub(40);
        let _ = writeln!(out, "{}", ticks[from..].join("\n"));
    }
}
