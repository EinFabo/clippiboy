//! Trial recording: starts the pipeline, buffers a few seconds and writes a
//! clip. Checks in one run what unit tests cannot capture — capture, colour
//! conversion, the encoder MFT, the clock and muxing.
//!
//!     cargo run --example record-probe -- [seconds]
//!     cargo run --example record-probe -- --encoder qsv 10
//!
//! With no `--encoder` it works through **every** encoder Media Foundation
//! admits to and writes a `report.txt` next to itself. That is the shape it is
//! meant to be handed over in: the machines where the encoder misbehaves are
//! not the machines we can build on, and "it lags" is not a measurement. What
//! comes back instead is which transform really ran, on which card, how long it
//! kept each frame, and how often it made the clock stand and wait.
//!
//! Each encoder is measured in a child process of its own. Media Foundation
//! keeps state per process, and an encoder that wedges must not take the rest
//! of the report down with it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clippiboy_lib::audio::engine::AudioEngine;
use clippiboy_lib::model::{
    AudioSource, Clip, EncoderId, RecordingConfig, SourceKind, TargetKind, TrackMix,
};
use clippiboy_lib::{config, edit, encode, muxer, pipeline::Pipeline, stems};

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const FPS: u32 = 60;

fn main() {
    // Everything on stdout. The sweep below collects a child's output as the
    // report, and a log line that had gone to stderr instead would simply be
    // missing from it — which is the half that carries the encoder profile.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .target(env_logger::Target::Stdout)
        .init();

    muxer::set_tool_dir(resources());

    let options = Options::parse();
    match &options.encoder {
        Some(name) => one(name, &options.variant, options.seconds, options.full),
        None => sweep(options.seconds, options.full),
    }
}

struct Options {
    encoder: Option<String>,
    variant: String,
    seconds: u64,
    full: bool,
}

impl Options {
    fn parse() -> Self {
        let mut options = Self {
            encoder: None,
            variant: VARIANTS[0].name.to_string(),
            seconds: 30,
            full: false,
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--encoder" => options.encoder = args.next(),
                "--variant" => {
                    if let Some(name) = args.next() {
                        options.variant = name;
                    }
                }
                "--full" => options.full = true,
                "--help" | "-h" => {
                    println!(
                        "usage: record-probe [seconds] [--encoder nvenc|amf|qsv] \
                         [--variant {}] [--full]",
                        VARIANTS
                            .iter()
                            .map(|v| v.name)
                            .collect::<Vec<_>>()
                            .join("|")
                    );
                    std::process::exit(0);
                }
                other => match other.parse::<u64>() {
                    Ok(seconds) => options.seconds = seconds.clamp(4, 300),
                    Err(_) => eprintln!("ignoring the argument {other:?}"),
                },
            }
        }
        options
    }
}

/// One set of encoder knobs to measure.
///
/// The first report from AMD hardware said the plumbing is fine — right adapter,
/// right transform, nothing refused, nothing dropped — and that the encoder
/// nonetheless grew steadily slower as the picture got busier, from 8 ms a frame
/// to 46 ms against a budget of 16.7. So the question is no longer *whether* it
/// is the encoder but *which* of its settings costs that.
///
/// AMD's own documentation points at one mechanism in particular: plain encoding
/// runs on the dedicated video block (VCN), but **pre-analysis runs on the
/// shaders** — the same part of the card the game is drawn with. That fits both
/// halves of what we have: a cost that grows with how much is moving, and a
/// complaint that only shows up during play, on a machine where an idle-desktop
/// measurement looks healthy.
///
/// Each variant is one process, so nothing carries over between them.
struct Variant {
    name: &'static str,
    why: &'static str,
    env: &'static [(&'static str, &'static str)],
}

const VARIANTS: &[Variant] = &[
    Variant {
        name: "as-is",
        why: "what ClippiBoy does now — low latency on for AMD and Intel, off for NVENC",
        env: &[],
    },
    // The old default, kept so a report shows the before and the after side by
    // side rather than asking anyone to take the change on trust.
    Variant {
        name: "lookahead",
        why: "what ClippiBoy did before: the lookahead left on. On AMD this is the fault",
        env: &[("CLIPPIBOY_LOW_LATENCY", "0")],
    },
    Variant {
        name: "fast-preset",
        why: "the other lever — quality/speed at the far end, low latency untouched",
        env: &[("CLIPPIBOY_QUALITY_VS_SPEED", "0")],
    },
    // The baseline again, last. Nothing is being tested here except the
    // machine: if these two disagree over the same amount of picture, something
    // else had the graphics card while we measured, and every row between them
    // is worthless. Finding that out afterwards, from the numbers themselves,
    // beats trusting that whoever ran this had closed everything — the first
    // time this table was tried, OBS was quietly holding the encoder and made a
    // healthy card look broken.
    Variant {
        name: "as-is-again",
        why: "the baseline a second time — the check on whether the machine was quiet",
        env: &[],
    },
];

/// The two runs that must agree, if anything else in the table is to mean
/// something.
const BASELINE: &str = "as-is";
const BASELINE_REPEAT: &str = "as-is-again";

/// What one run turned out to be, in numbers the parent can line up.
///
/// Printed by the child on one line and parsed back by the parent. A summary
/// table is the whole point of the exercise — five runs of prose would leave the
/// comparison to whoever reads it, and the last report needed differentiating by
/// hand before it said anything.
#[derive(Default, Clone)]
struct Outcome {
    encoder: String,
    variant: String,
    running: String,
    fps: f32,
    /// Mean µs per frame early in the run, and again at the end. Two numbers,
    /// because the fault we are chasing is a slope, not a level.
    early_us: u64,
    late_us: u64,
    worst_us: u64,
    waits: u64,
    /// Milliseconds the clock actually stood still waiting to hand a frame over.
    /// The count above says how often the queue was briefly full, which is
    /// ordinary backpressure; this says what it cost, which is the stutter.
    stall_ms: u64,
    dropped: u64,
    repeats: u64,
    /// Every frame the clock put out, repeats included.
    total_frames: u64,
    /// The most frames the encoder held at once.
    ///
    /// This is the column that explains the rest. A hardware encoder working on
    /// one picture at a time answers in single-digit milliseconds; one running a
    /// lookahead holds a dozen or more, and the "time per frame" measured from
    /// hand-over to hand-back is then that queue, not the work.
    queued: u64,
    ring_mb: f32,
}

const RESULT_TAG: &str = "#RESULT";

impl Outcome {
    /// The share of frames that carried a new picture rather than repeating the
    /// last one. This is the encoder's actual workload: Windows.Graphics.Capture
    /// only delivers a frame when something changes, and the clock repeats the
    /// last one to keep the stream at a constant rate. A run at 15 % is encoding
    /// a slideshow; a game is nearer 100 %.
    fn moving(&self) -> f32 {
        match self.total_frames {
            0 => 0.0,
            total => (total - self.repeats.min(total)) as f32 / total as f32,
        }
    }
}

impl Outcome {
    fn print(&self) {
        println!(
            "{RESULT_TAG} encoder={} variant={} running={} fps={:.1} early_us={} late_us={} \
             worst_us={} waits={} stall_ms={} dropped={} repeats={} frames={} queued={} \
             ring_mb={:.1}",
            self.encoder,
            self.variant,
            self.running,
            self.fps,
            self.early_us,
            self.late_us,
            self.worst_us,
            self.waits,
            self.stall_ms,
            self.dropped,
            self.repeats,
            self.total_frames,
            self.queued,
            self.ring_mb
        );
    }

    fn parse(line: &str) -> Option<Self> {
        let rest = line.trim().strip_prefix(RESULT_TAG)?;
        let mut out = Self::default();
        for pair in rest.split_whitespace() {
            let (key, value) = pair.split_once('=')?;
            match key {
                "encoder" => out.encoder = value.into(),
                "variant" => out.variant = value.into(),
                "running" => out.running = value.into(),
                "fps" => out.fps = value.parse().ok()?,
                "early_us" => out.early_us = value.parse().ok()?,
                "late_us" => out.late_us = value.parse().ok()?,
                "worst_us" => out.worst_us = value.parse().ok()?,
                "waits" => out.waits = value.parse().ok()?,
                "stall_ms" => out.stall_ms = value.parse().ok()?,
                "dropped" => out.dropped = value.parse().ok()?,
                "repeats" => out.repeats = value.parse().ok()?,
                "frames" => out.total_frames = value.parse().ok()?,
                "queued" => out.queued = value.parse().ok()?,
                "ring_mb" => out.ring_mb = value.parse().ok()?,
                _ => {}
            }
        }
        Some(out)
    }
}

/// Where ffmpeg and ffprobe are.
///
/// Beside the executable first, because that is how the probe is handed to
/// somebody: a folder with the exe and the two tools in it, no checkout and no
/// toolchain. Only then the place `cargo run` would find them.
fn resources() -> PathBuf {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    if let Some(dir) = beside {
        if dir.join("ffmpeg.exe").exists() {
            return dir;
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("resources")
}

fn tag(id: EncoderId) -> &'static str {
    match id {
        EncoderId::Nvenc => "nvenc",
        EncoderId::Amf => "amf",
        EncoderId::Qsv => "qsv",
        EncoderId::X264 => "x264",
    }
}

/// Programs that would be sharing the encoder with us.
///
/// By name, and a short list on purpose. There is no portable way to ask a
/// graphics card "who else is encoding right now" across NVIDIA, AMD and Intel,
/// and a confident wrong answer would be worse than none — so this only names
/// the ones that actually turn up, and says it is a suspicion rather than a
/// finding.
///
/// It exists because the first time this table was run, OBS was quietly holding
/// the encoder at 86 %, and a perfectly healthy card produced 647 ms frames.
/// Nothing in the numbers said why. Running the baseline twice catches load
/// that comes and goes; it cannot catch load that was there the whole time.
/// This can.
const RIVALS: &[(&str, &str)] = &[
    ("obs64.exe", "OBS Studio"),
    ("obs32.exe", "OBS Studio"),
    ("clippiboy.exe", "ClippiBoy itself"),
    ("streamlabs obs.exe", "Streamlabs"),
    ("xsplit.core.exe", "XSplit"),
    ("medal.exe", "Medal"),
    ("overwolf.exe", "Overwolf"),
    ("bdcam.exe", "Bandicam"),
    ("action.exe", "Action!"),
    ("nvidia app.exe", "the NVIDIA app (instant replay)"),
];

#[cfg(windows)]
fn rivals() -> Vec<String> {
    let running = clippiboy_lib::audio::devices::process_tree();
    let mut found: Vec<String> = Vec::new();
    for (_, (_, exe)) in running {
        let exe = exe.to_ascii_lowercase();
        if let Some((_, label)) = RIVALS.iter().find(|(name, _)| *name == exe) {
            if !found.iter().any(|seen| seen == label) {
                found.push((*label).to_string());
            }
        }
    }
    if let Some(game) = clippiboy_lib::game::detect() {
        found.push(format!("a game in the foreground ({game})"));
    }
    found
}

#[cfg(not(windows))]
fn rivals() -> Vec<String> {
    Vec::new()
}

/// The warning, or nothing at all when the machine is clear.
fn rival_warning(found: &[String]) -> String {
    if found.is_empty() {
        return String::new();
    }
    format!(
        "\n!! These are running and may be using the video encoder too:\n\
         !!   {}\n\
         !! Close them and run this again. Sharing the encoder makes a healthy\n\
         !! card look broken, and there is no way to subtract it afterwards.\n",
        found.join(", ")
    )
}

/// Stand in the way until the machine is quiet, or until told to go ahead.
///
/// The note in the folder asks for this in its first paragraph, and the note was
/// not enough — the first report back from AMD hardware had OBS running through
/// the whole sweep, which cost four minutes of somebody else's evening and told
/// us nothing. A warning in a file is a warning read afterwards, if at all. This
/// one is in front of the person while they can still do something about it.
///
/// Returns whether it ended up quiet, so the report can say which it was.
/// Started without a console — piped, or from a script — this reads end-of-file
/// straight away and goes ahead rather than hanging forever.
fn insist_on_quiet() -> bool {
    for attempt in 0..3 {
        let found = rivals();
        if found.is_empty() {
            return true;
        }

        println!("{}", rival_warning(&found));
        if attempt == 2 {
            println!("Still running. Going ahead anyway — the report will say so.");
            return false;
        }
        println!("Please close those now, then press Enter to start the test.");
        println!("(If you cannot close them, type  anyway  and press Enter.)");

        let mut answer = String::new();
        match std::io::stdin().read_line(&mut answer) {
            // No console to answer with: piped input or a script. Get on with it.
            Ok(0) => return false,
            Ok(_) => {
                if answer.trim().eq_ignore_ascii_case("anyway") {
                    println!("Right — going ahead. The report will say the machine was busy.");
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
    false
}

/// What the machine is, before any encoder has run on it.
fn header() -> String {
    let mut lines = vec![
        format!("ClippiBoy probe {}", env!("CARGO_PKG_VERSION")),
        format!("ffmpeg found: {}", muxer::available()),
        String::new(),
        "graphics adapters".into(),
    ];

    #[cfg(windows)]
    {
        let adapters = clippiboy_lib::gpu::adapters();
        if adapters.is_empty() {
            lines.push("  (DXGI listed none — that alone is the fault)".into());
        }
        for (info, _) in &adapters {
            lines.push(format!("  {info}"));
        }
    }
    #[cfg(not(windows))]
    lines.push("  (not Windows — there is nothing to record here)".into());

    lines.push(String::new());
    lines.push("encoders according to Media Foundation".into());
    for info in encode::list_encoders() {
        lines.push(format!(
            "  {:<28} available={} hardware={}",
            info.name, info.available, info.hardware
        ));
    }

    let warning = rival_warning(&rivals());
    if !warning.is_empty() {
        lines.push(warning);
    }
    lines.join("\n")
}

/// Every hardware encoder on the machine, each put through every variant.
fn sweep(seconds: u64, full: bool) {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            eprintln!("cannot find my own path: {err}");
            std::process::exit(1);
        }
    };
    let report_path = exe
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("report.txt");

    // Before anything is measured, and before the four minutes are spent.
    let quiet = insist_on_quiet();

    let mut report = header();
    if !quiet {
        report.push_str(
            "\n!! This ran with other software holding the encoder, after being asked to \
             close it.\n!! Every number below is worth less than it looks.\n",
        );
    }
    println!("{report}");

    // Hardware only. Asking for the software encoder used to look like a
    // baseline and was not one: this pipeline hands the encoder Direct3D
    // textures, the Windows software MFT cannot take them, and the request
    // quietly ran on hardware again. So it would just be a sixth measurement of
    // the same thing.
    let encoders: Vec<EncoderId> = encode::list_encoders()
        .into_iter()
        .filter(|info| info.available && info.hardware)
        .map(|info| info.id)
        .collect();

    if encoders.is_empty() {
        let note = "\n\nNo hardware H.264 encoder on this machine. ClippiBoy encodes straight \
                    off the graphics card and cannot record without one — that alone is the \
                    answer, and there is nothing further to measure.\n";
        print!("{note}");
        report.push_str(note);
    }

    let mut outcomes: Vec<Outcome> = Vec::new();

    for id in &encoders {
        for variant in VARIANTS {
            let banner = format!(
                "\n\n======== {} · {} ========\n{}\n",
                tag(*id),
                variant.name,
                variant.why
            );
            print!("{banner}");
            report.push_str(&banner);

            let mut command = std::process::Command::new(&exe);
            command
                .env("CLIPPIBOY_ENCODER", tag(*id))
                .arg("--encoder")
                .arg(tag(*id))
                .arg("--variant")
                .arg(variant.name)
                .arg(seconds.to_string());
            for (key, value) in variant.env {
                command.env(key, value);
            }
            if full {
                command.arg("--full");
            }

            match command.output() {
                Ok(output) => {
                    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
                    text.push_str(&String::from_utf8_lossy(&output.stderr));
                    if !output.status.success() {
                        text.push_str(&format!("\nthe run ended with {}\n", output.status));
                    }
                    outcomes.extend(text.lines().filter_map(Outcome::parse));
                    print!("{text}");
                    report.push_str(&text);
                }
                Err(err) => {
                    let text = format!("could not start the run: {err}\n");
                    print!("{text}");
                    report.push_str(&text);
                }
            }
        }
    }

    for id in &encoders {
        let table = summary(tag(*id), &outcomes, seconds);
        print!("{table}");
        report.push_str(&table);
    }

    // Again at the end: something may have been started while this ran, and a
    // warning only at the top of a long file is a warning nobody reads twice.
    let warning = rival_warning(&rivals());
    if !warning.is_empty() {
        print!("{warning}");
        report.push_str(&warning);
    }

    match std::fs::write(&report_path, &report) {
        Ok(()) => println!("\n\nreport written to {}", report_path.display()),
        Err(err) => println!("\n\nreport not written: {err}"),
    }

    // Double-clicked from Explorer, the window would otherwise shut the instant
    // this returns — and a run that failed in the first second would leave the
    // person who did us the favour with nothing to look at and nothing to send.
    println!("\nDone. Send back the file report.txt. Press Enter to close.");
    let _ = std::io::stdin().read_line(&mut String::new());
}

/// The table the whole exercise exists for: which setting changes what.
fn summary(encoder: &str, outcomes: &[Outcome], seconds: u64) -> String {
    let mine: Vec<&Outcome> = outcomes.iter().filter(|o| o.encoder == encoder).collect();
    if mine.is_empty() {
        return format!("\n\n======== summary: {encoder} ========\n  no run produced a result\n");
    }

    let ms = |us: u64| format!("{:.1} ms", us as f64 / 1000.0);
    let mut out = format!(
        "\n\n======== summary: {encoder} ========\n\
         one frame is {} µs at {FPS} fps; each run is {seconds} s\n\
         \"early\" is seconds 2-4, \"late\" the last three — a rising pair is the fault\n\
         \"stalled\" is how long the clock stood waiting for the encoder, in total\n\n\
         \"moving\" is how much of the picture was new rather than a repeat — an encoder\n\
         given a still screen is barely working, and two runs at different values\n\
         are not comparing the same thing\n\n\
         \"queued\" is the most frames the encoder held at once — a lookahead, and the\n\
         reason a \"time per frame\" can read in the hundreds of milliseconds while\n\
         nothing at all is being dropped\n\n\
         {:<14} {:>6} {:>7} {:>7}  {:>9} {:>9} {:>9}  {:>8} {:>8}\n",
        budget_us(),
        "variant",
        "fps",
        "moving",
        "queued",
        "early",
        "late",
        "worst",
        "stalled",
        "dropped"
    );

    for outcome in &mine {
        out.push_str(&format!(
            "{:<14} {:>6.1} {:>6.0}% {:>7}  {:>9} {:>9} {:>9}  {:>8} {:>8}\n",
            outcome.variant,
            outcome.fps,
            outcome.moving() * 100.0,
            outcome.queued,
            ms(outcome.early_us),
            ms(outcome.late_us),
            ms(outcome.worst_us),
            format!("{} ms", outcome.stall_ms),
            outcome.dropped,
        ));
    }

    // A still screen is the cheapest input an encoder ever gets, and a table
    // measured on one says nothing about a game. This was not obvious until a
    // report came back perfectly flat at 9 ms a frame with 85 % of the picture
    // standing still.
    let quietest = mine.iter().map(|o| o.moving()).fold(1.0f32, f32::min);
    let busiest = mine.iter().map(|o| o.moving()).fold(0.0f32, f32::max);
    if busiest < 0.25 {
        out.push_str(&format!(
            "\nthe screen barely moved ({:.0} % of frames were new at most). An encoder \
             given\na still picture is hardly working — this says little about a game. \
             Leave a\nvideo playing next time.\n",
            busiest * 100.0
        ));
    } else if busiest > quietest * 2.0 {
        out.push_str(&format!(
            "\nthe runs did not see the same amount of movement ({:.0} % to {:.0} % new \
             frames),\nso the rows below are not strictly comparable.\n",
            quietest * 100.0,
            busiest * 100.0
        ));
    }

    let baseline = mine.iter().find(|o| o.variant == BASELINE);
    let repeat = mine.iter().find(|o| o.variant == BASELINE_REPEAT);

    // Before anything is read off this table, the table has to be worth
    // reading. The same settings measured twice, minutes apart, should land in
    // the same place; if they do not, something else was using the graphics
    // card and no row here means anything.
    if let (Some(first), Some(second)) = (baseline, repeat) {
        let (low, high) = if first.late_us <= second.late_us {
            (first.late_us, second.late_us)
        } else {
            (second.late_us, first.late_us)
        };
        let agree = low > 0 && high * 2 <= low * 3;

        // Two baselines that disagree mean one of two things, and telling them
        // apart matters: either something else was using the card, or the two
        // runs simply had different amounts of picture to encode. Blaming the
        // machine for what was really a quieter screen throws away a good
        // measurement — which is exactly what happened the first time this
        // check ran on real data.
        let (quiet_work, busy_work) = {
            let a = first.moving();
            let b = second.moving();
            if a <= b { (a, b) } else { (b, a) }
        };
        let same_work = quiet_work > 0.0 && busy_work <= quiet_work * 1.5;

        out.push_str(&format!(
            "\nthe same settings twice: {} at {:.0} % moving, {} at {:.0} % moving\n",
            ms(first.late_us),
            first.moving() * 100.0,
            ms(second.late_us),
            second.moving() * 100.0
        ));

        if agree {
            out.push_str("  they agree — the table can be read straight off\n");
        } else if same_work {
            out.push_str(
                "  they disagree over the same amount of picture, so something else was \
                 using\n  the graphics card. Close every game, OBS and any other recorder \
                 and run\n  this again — nothing below is trustworthy as it stands.\n",
            );
            return out;
        } else {
            out.push_str(
                "  they disagree, but they also saw different amounts of picture, so this \
                 is\n  not evidence of a busy machine. Read the table for which settings \
                 differ,\n  not for the exact numbers.\n",
            );
        }
    }

    // Say the answer out loud, so nobody has to divide two columns in their head.
    if let Some(baseline) = baseline {
        out.push_str("\nread against the baseline:\n");
        // The queue is the mechanism, and it is worth naming rather than
        // leaving as a column somebody has to interpret.
        if baseline.queued >= 4 {
            let quickest = mine
                .iter()
                .filter(|o| o.variant != BASELINE && o.variant != BASELINE_REPEAT)
                .min_by_key(|o| o.queued);
            out.push_str(&format!(
                "  as-is holds up to {} frames inside the encoder at once. That is a \
                 lookahead,\n  not slow encoding — nothing is dropped — but on AMD it runs \
                 on the shaders,\n  the same part of the card the game is drawn with.\n",
                baseline.queued
            ));
            if let Some(quickest) = quickest {
                if quickest.queued * 2 < baseline.queued {
                    out.push_str(&format!(
                        "  {} brings that down to {}.\n",
                        quickest.variant, quickest.queued
                    ));
                }
            }
        }
        if baseline.late_us > baseline.early_us * 3 / 2 {
            out.push_str(&format!(
                "  as-is gets slower as it runs: {} early, {} late. That is the complaint.\n",
                ms(baseline.early_us),
                ms(baseline.late_us)
            ));
        } else if baseline.late_us > budget_us() {
            out.push_str(&format!(
                "  as-is is steady but over budget: {} a frame against {}.\n",
                ms(baseline.late_us),
                ms(budget_us())
            ));
        } else {
            out.push_str(
                "  as-is held steady and inside its budget — whatever is wrong did not \
                 show up in this run.\n",
            );
        }
        for outcome in mine
            .iter()
            .filter(|o| o.variant != BASELINE && o.variant != BASELINE_REPEAT)
        {
            let verdict = if outcome.late_us * 2 < baseline.late_us {
                "fixes it"
            } else if outcome.late_us * 4 < baseline.late_us * 5 {
                "no real difference"
            } else {
                "makes it worse"
            };
            out.push_str(&format!(
                "  {:<14} {} late against {} — {verdict}\n",
                outcome.variant,
                ms(outcome.late_us),
                ms(baseline.late_us)
            ));
        }
    }
    out
}

/// One encoder in one variant, measured.
fn one(name: &str, variant: &str, seconds: u64, full: bool) {
    // The sweep sets these for the child; set them here too, so calling the
    // probe by hand behaves the same. Forcing the encoder goes past the
    // availability check on purpose — an encoder that is not offered is the
    // interesting one.
    std::env::set_var("CLIPPIBOY_ENCODER", name);
    if let Some(chosen) = VARIANTS.iter().find(|v| v.name == variant) {
        for (key, value) in chosen.env {
            std::env::set_var(key, value);
        }
    }

    let recording = RecordingConfig {
        target_kind: TargetKind::Monitor,
        target_id: None,
        width: WIDTH,
        height: HEIGHT,
        fps: FPS,
        bitrate_kbps: encode::bitrate_for(WIDTH, HEIGHT, FPS),
        quality: clippiboy_lib::model::default_quality(),
        encoder: encode::resolve(EncoderId::X264),
        keyframe_seconds: 2,
    };

    // The same sum the app itself budgets with, rather than a number invented
    // here — the ring behaving differently under the probe than in the app is
    // exactly the kind of difference that wastes an afternoon.
    let mut buffer = config::default_config().buffer;
    buffer.seconds = 60;
    let buffer_bytes = config::effective_memory_bytes(&recording, &buffer);

    println!("requested encoder: {name}, variant: {variant}");

    // Two sources with a track of their own. They deliver nothing (the WASAPI
    // streams do not run here at all), but they create the extra tracks — and it
    // is exactly their path through muxer, store and save that is being checked.
    let sources: Vec<AudioSource> = ["Microphone", "Discord"]
        .iter()
        .enumerate()
        .map(|(index, label)| AudioSource {
            id: format!("probe-{index}"),
            label: (*label).into(),
            kind: SourceKind::InputDevice {
                device_id: format!("not-present-{index}"),
            },
            enabled: true,
            gain_db: 0.0,
            muted: false,
            solo: false,
            separate_track: true,
        })
        .collect();

    let audio = Arc::new(AudioEngine::new());
    let mut pipeline = match Pipeline::start(&recording, buffer.seconds, buffer_bytes, sources, audio)
    {
        Ok(pipeline) => pipeline,
        Err(err) => {
            println!("pipeline will not start: {err}");
            std::process::exit(1);
        }
    };
    let running = *pipeline.shared.encoder.lock();
    println!("running encoder:   {running:?}");

    // One sample a second. Each one covers only the second before it, so a
    // slope shows up as a slope instead of being flattened into an average.
    let mut samples: Vec<clippiboy_lib::pipeline::Health> = Vec::new();
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(seconds) {
        std::thread::sleep(Duration::from_secs(1));
        let health = pipeline.shared.health();
        println!(
            "  {:>3}s  {:.1} fps, encoder {} µs/frame, worst {} µs, in-flight {}, \
             dropped {}, repeats {}, ring {:.1} MB",
            started.elapsed().as_secs(),
            health.fps,
            health.encode_us,
            health.encode_worst_us,
            health.in_flight,
            health.dropped,
            health.duplicated,
            health.ring_bytes as f64 / 1_048_576.0
        );
        samples.push(health);
    }

    if let Some(err) = pipeline.shared.error.lock().take() {
        println!("error from the recording: {err}");
    }

    let outcome = weigh(name, variant, running, &samples);
    verdict(&outcome, budget_us());
    outcome.print();

    let snapshot = match pipeline.snapshot(5) {
        Ok(snapshot) => snapshot,
        Err(err) => {
            println!("no snapshot: {err}");
            pipeline.stop();
            std::process::exit(1);
        }
    };
    let keyframes = snapshot.packets.iter().filter(|p| p.keyframe).count();
    println!(
        "\nsnapshot: {} packets, {keyframes} keyframes, SPS/PPS {} bytes",
        snapshot.packets.len(),
        snapshot.sequence_header.len()
    );

    if muxer::available() {
        let temp = std::env::temp_dir().join("clippiboy-probe");
        let output = temp.join(format!("probe-{name}-{variant}.mp4"));
        let _ = std::fs::create_dir_all(&temp);
        match muxer::build(muxer::ClipRequest {
            snapshot,
            clip_id: format!("probe-{name}"),
            output: output.clone(),
            temp_dir: temp.clone(),
        }) {
            Ok(result) => {
                println!(
                    "\nclip: {} ({:.1} MB, {} ms)",
                    result.path.display(),
                    result.size_bytes as f64 / 1_048_576.0,
                    result.duration_ms
                );
                probe(&result.path);
                if full {
                    check_save(&result.path, result.duration_ms, name);
                }
            }
            Err(err) => println!("clip not written: {err}"),
        }
    } else {
        println!("\n(no ffmpeg beside the probe, so no test clip — the measurements above \
                  do not need one)");
    }

    pipeline.stop();
    let _ = std::io::stdout().flush();
}

fn budget_us() -> u64 {
    1_000_000 / FPS as u64
}

/// Turn a run of per-second samples into the numbers that get compared.
fn weigh(
    name: &str,
    variant: &str,
    running: Option<EncoderId>,
    samples: &[clippiboy_lib::pipeline::Health],
) -> Outcome {
    let mut outcome = Outcome {
        encoder: name.into(),
        variant: variant.into(),
        running: running.map(tag).unwrap_or("none").into(),
        ..Outcome::default()
    };
    let Some(last) = samples.last() else {
        return outcome;
    };

    outcome.worst_us = last.encode_worst_us;
    outcome.waits = last.submit_waited;
    outcome.stall_ms = last.submit_wait_ms;
    outcome.dropped = last.dropped;
    outcome.repeats = last.duplicated;
    outcome.total_frames = last.frames;
    outcome.queued = samples.iter().map(|h| h.in_flight).max().unwrap_or(0);
    outcome.ring_mb = last.ring_bytes as f32 / 1_048_576.0;

    // The first second is startup — capture and encoder are still being built,
    // and counting it as recording turns every healthy run into a failing one.
    let steady: Vec<&clippiboy_lib::pipeline::Health> = samples.iter().skip(1).collect();
    if steady.is_empty() {
        return outcome;
    }
    outcome.fps = steady.iter().map(|h| h.fps).sum::<f32>() / steady.len() as f32;

    let mean = |window: &[&clippiboy_lib::pipeline::Health]| -> u64 {
        let counted: Vec<u64> = window.iter().map(|h| h.encode_us).filter(|us| *us > 0).collect();
        match counted.len() {
            0 => 0,
            len => counted.iter().sum::<u64>() / len as u64,
        }
    };
    let width = 3.min(steady.len());
    outcome.early_us = mean(&steady[..width]);
    outcome.late_us = mean(&steady[steady.len() - width..]);
    outcome
}

/// The part we would otherwise have to work out by reading numbers.
///
/// Each line names one thing that is wrong, in the words we would use about it.
/// A report saying "nothing stands out" is as much of an answer as one that
/// does not: it moves the fault out of the encoder and into capture or the
/// clock, and there was no way to tell those apart at all before.
fn verdict(outcome: &Outcome, budget_us: u64) {
    let mut found: Vec<String> = Vec::new();
    let ms = |us: u64| format!("{:.1} ms", us as f64 / 1000.0);

    if outcome.running == "none" {
        found.push("no encoder is running at all".into());
    } else if outcome.running != outcome.encoder {
        found.push(format!(
            "{} was asked for and {} ran — the log line above this run says why",
            outcome.encoder, outcome.running
        ));
    }

    if outcome.fps < FPS as f32 * 0.95 {
        found.push(format!(
            "{:.1} fps against {FPS} configured — the pipeline is not keeping up",
            outcome.fps
        ));
    }
    if outcome.dropped > 0 {
        found.push(format!("{} frames were lost outright", outcome.dropped));
    }
    if outcome.waits > 0 {
        found.push(format!(
            "the clock had to wait on the encoder {} times",
            outcome.waits
        ));
    }

    // The one that matters, and the one a running average hides: not "is it
    // slow" but "is it getting slower". Half again over a run is well past
    // anything a busier picture explains on its own.
    if outcome.late_us > outcome.early_us * 3 / 2 && outcome.early_us > 0 {
        found.push(format!(
            "the encoder slowed down while it ran: {} a frame early, {} a frame late",
            ms(outcome.early_us),
            ms(outcome.late_us)
        ));
    }
    if outcome.late_us > budget_us {
        found.push(format!(
            "{} a frame at the end, against a budget of {} — it cannot hold this rate",
            ms(outcome.late_us),
            ms(budget_us)
        ));
    }

    println!("\nverdict");
    if found.is_empty() {
        println!("  nothing stands out — the encoder kept up");
    }
    for line in found {
        println!("  {line}");
    }
    println!(
        "  · {:.1} fps, {} a frame early and {} late, worst {}, {} repeats, ring {:.1} MB",
        outcome.fps,
        ms(outcome.early_us),
        ms(outcome.late_us),
        ms(outcome.worst_us),
        outcome.repeats,
        outcome.ring_mb
    );
}

/// What ffprobe says about the finished file — frame rate, codec, tracks.
fn probe(path: &std::path::Path) {
    let output = muxer::command("ffprobe")
        .args([
            "-v", "error",
            "-show_entries",
            "stream=index,codec_name,width,height,r_frame_rate,avg_frame_rate,nb_frames,channels",
            "-of", "default=noprint_wrappers=1",
        ])
        .arg(path)
        .output();
    match output {
        Ok(out) => println!("\nffprobe:\n{}", String::from_utf8_lossy(&out.stdout)),
        Err(err) => println!("ffprobe: {err}"),
    }
}

/// Check the save path: reapply the mix and measure that the video track stays
/// untouched and exactly one audio track is left.
fn check_save(path: &std::path::Path, duration_ms: u64, name: &str) {
    let id = format!("probe-{name}");
    let before = video_frames(path);
    let clip = Clip {
        id: id.clone(),
        path: path.to_string_lossy().to_string(),
        created_at: 0,
        duration_ms,
        game: None,
        width: WIDTH,
        height: HEIGHT,
        size_bytes: 0,
        thumb_path: None,
        title: None,
        description: None,
        favorite: false,
        edit: None,
        original: None,
        original_available: false,
        screenshot: false,
    };

    let tracks = match stems::tracks(&clip.id, &edit::source_path(&clip)) {
        Ok(tracks) => tracks,
        Err(err) => {
            println!("tracks not readable: {err}");
            return;
        }
    };
    println!(
        "\nsave probe: {} track(s){}",
        tracks.len(),
        if tracks.iter().all(|t| t.preview_path.is_some()) {
            ", stored individually"
        } else {
            ", only in the file"
        }
    );

    let mix: Vec<TrackMix> = tracks
        .iter()
        .map(|track| TrackMix {
            index: track.index,
            gain_db: -6.0,
            muted: false,
        })
        .collect();

    // With no trim: the video has to stay identical frame for frame. The last
    // argument is a quality, not a bitrate — it used to be handed 40_000, which
    // the encoder clamped to its top end and nobody noticed.
    let whole = edit::Trim::whole(clip.duration_ms);
    match edit::apply(
        &clip,
        whole,
        &mix,
        EncoderId::X264,
        clippiboy_lib::model::default_quality(),
        |_| {},
    ) {
        Ok(applied) => {
            let after = video_frames(path);
            println!(
                "  rewritten: {:.1} MB",
                applied.size_bytes as f64 / 1_048_576.0
            );
            println!("  frames before {before:?}, after {after:?}");
            assert_eq!(before, after, "the video track was touched!");
            probe(path);
        }
        Err(err) => println!("  save failed: {err}"),
    }
    stems::remove(&id);
}

fn video_frames(path: &std::path::Path) -> Option<String> {
    let out = muxer::command("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "v",
            "-count_frames",
            "-show_entries", "stream=nb_read_frames",
            "-of", "csv=p=0",
        ])
        .arg(path)
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
