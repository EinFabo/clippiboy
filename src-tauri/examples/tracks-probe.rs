//! Checks the extraction of the individual tracks against the case that used to
//! break them: two simultaneous requests for the same clip.
//!
//! That is exactly what happens in the player, because `React.StrictMode` mounts
//! the effect twice. Previously two ffmpeg runs then wrote into the same target
//! files.
//!
//!     cargo run --example tracks-probe -- "C:\\path\\to\\clip.mp4"

use std::sync::Arc;

use clippiboy_lib::model::{Clip, EncoderId};
use clippiboy_lib::{edit, muxer, stems};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: tracks-probe <clip.mp4>");
        std::process::exit(2);
    };
    let resources = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
    muxer::set_tool_dir(resources);

    let clip = Arc::new(Clip {
        id: "tracks-probe".into(),
        path: path.clone(),
        created_at: 0,
        duration_ms: muxer::probe_duration_ms(std::path::Path::new(&path)).unwrap_or(0),
        game: None,
        width: 0,
        height: 0,
        size_bytes: 0,
        thumb_path: None,
        title: None,
        description: None,
        favorite: false,
        edit: None,
        original: None,
        original_available: false,
        screenshot: false,
    });

    // Start from a clean state, otherwise the run only checks the shortcut
    // through the index.
    stems::remove(&clip.id);

    println!("Two simultaneous requests on {path}");
    let handles: Vec<_> = (0..2)
        .map(|n| {
            let clip = clip.clone();
            std::thread::spawn(move || {
                let source = edit::source_path(&clip);
                (n, stems::tracks(&clip.id, &source))
            })
        })
        .collect();

    let mut listed = Vec::new();
    for handle in handles {
        match handle.join().expect("thread crashed") {
            (n, Ok(tracks)) => {
                println!("  request {n}: {} track(s)", tracks.len());
                listed = tracks;
            }
            (n, Err(err)) => {
                eprintln!("  request {n} failed: {err}");
                std::process::exit(1);
            }
        }
    }

    println!("\nDecode every stored track:");
    let mut broken = 0;
    for track in &listed {
        let Some(file) = track.preview_path.as_deref() else {
            println!("  {:>2}  {:<40} (only in the clip file)", track.index, track.label);
            continue;
        };
        let size = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
        let output = muxer::ffmpeg()
            .args(["-v", "error", "-i", file, "-f", "null", "-"])
            .output();
        let complaint = match output {
            Ok(out) => String::from_utf8_lossy(&out.stderr).trim().to_string(),
            Err(err) => err.to_string(),
        };
        if complaint.is_empty() {
            println!("  {:>2}  {:<40} {size:>9} B  ok", track.index, track.label);
        } else {
            broken += 1;
            println!(
                "  {:>2}  {:<40} {size:>9} B  BROKEN: {}",
                track.index,
                track.label,
                complaint.lines().next().unwrap_or_default()
            );
        }
    }

    // And now the path that failed for the user: mix exactly these tracks into a
    // single audio track and write it into the clip.
    let mix: Vec<clippiboy_lib::model::TrackMix> = listed
        .iter()
        .enumerate()
        .map(|(slot, track)| clippiboy_lib::model::TrackMix {
            index: track.index,
            // Different levels, so the filter chain really has work to do.
            gain_db: -3.0 * slot as f32,
            muted: false,
        })
        .collect();
    println!("\nSaving with mixed levels:");
    let whole = edit::Trim::whole(clip.duration_ms);
    match edit::apply(&clip, whole, &mix, EncoderId::X264, 40_000, |_| {}) {
        Ok(applied) => println!(
            "  written, {:.1} MB",
            applied.size_bytes as f64 / 1_048_576.0
        ),
        Err(err) => {
            eprintln!("  failed: {err}");
            stems::remove(&clip.id);
            std::process::exit(1);
        }
    }

    stems::remove(&clip.id);
    edit::remove(&clip.id);
    if broken > 0 {
        eprintln!("\n{broken} track(s) damaged.");
        std::process::exit(1);
    }
    println!("\nAll tracks are readable.");
}
