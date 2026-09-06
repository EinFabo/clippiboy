//! Trims a real clip and measures whether what comes out is what should come
//! out — and whether "undo trim" really brings it back.
//!
//! The keyframe and sync questions cannot be settled in a unit test: that needs
//! ffmpeg, a real file, and ffprobe to measure afterwards.
//!
//!     cargo run --example trim-probe -- "C:\\path\\to\\clip.mp4"
//!
//! **Careful:** the run rewrites the file it is given. It moves it into the
//! originals store and fetches it back at the end — but run it on a copy to be
//! safe.

use std::path::Path;

use clippiboy_lib::model::{Clip, ClipOriginal, EncoderId, TrackMix};
use clippiboy_lib::{edit, muxer, stems};

/// How far the measured length may be off. One frame at 60 fps is 17 ms; the
/// container rounds to whole ticks, and the last audio often reaches a few
/// milliseconds past the last frame.
const TOLERANCE_MS: i64 = 120;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: trim-probe <clip.mp4>");
        std::process::exit(2);
    };
    let resources = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
    muxer::set_tool_dir(resources);

    let id = "trim-probe";
    // Start from a clean state — otherwise the run checks half a leftover from
    // earlier.
    stems::remove(id);
    edit::remove(id);

    let full = match muxer::probe_duration_ms(Path::new(&path)) {
        Some(ms) if ms > 8_000 => ms,
        Some(ms) => {
            eprintln!("At {ms} ms the clip is too short to trim — 8 s minimum.");
            std::process::exit(2);
        }
        None => {
            eprintln!("length not readable. Is the file there?");
            std::process::exit(2);
        }
    };
    println!("Source clip: {path}\n  {full} ms");

    let mut clip = Clip {
        id: id.into(),
        path: path.clone(),
        created_at: 0,
        duration_ms: full,
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
    };

    let mix = mix_for(&clip);
    let mut failures = 0;

    // 1. Shorten at the back only. Lossless, done in a second or two — and the
    //    video has to stay identical frame for frame.
    let want = full - 3_000;
    println!("\n1. Shorten at the back to {want} ms (copy, no re-encode)");
    let plan = edit::plan(None, true, full, edit::Trim { start_ms: 0, end_ms: want });
    assert!(!plan.reencode, "shortening at the end must not encode");
    failures += step(&mut clip, edit::Trim { start_ms: 0, end_ms: want }, &mix, want);

    // 2. Cut at the front. Has to be frame-accurate — when copying, the start
    //    slid to the keyframe before it, so up to two seconds too early.
    println!("\n2. Cut 2000 ms off the front (frame-accurate, gets re-encoded)");
    let before = clip.duration_ms;
    let trim = edit::Trim { start_ms: 2_000, end_ms: before };
    failures += step(&mut clip, trim, &mix, before - 2_000);
    match clip.original {
        Some(ClipOriginal { start_ms, duration_ms, .. }) => {
            println!("  excerpt sits at {start_ms} ms of {duration_ms} ms");
            if start_ms != 2_000 {
                eprintln!("  ERROR: expected 2000 ms");
                failures += 1;
            }
        }
        None => {
            eprintln!("  ERROR: no original recorded");
            failures += 1;
        }
    }

    // 3. Undo. Has to bring the full length back, losslessly.
    println!("\n3. Undo trim");
    match edit::restore(&clip, &mix, EncoderId::X264, 40_000, |_| {}) {
        Ok(applied) => {
            report(applied.duration_ms, full);
            if (applied.duration_ms as i64 - full as i64).abs() > TOLERANCE_MS {
                failures += 1;
            }
            if applied.original.is_some() {
                eprintln!("  ERROR: the original should have been gone");
                failures += 1;
            }
            if edit::has_original(id) {
                eprintln!("  ERROR: the store is still there");
                failures += 1;
            }
        }
        Err(err) => {
            eprintln!("  ERROR: {err}");
            failures += 1;
        }
    }

    stems::remove(id);
    edit::remove(id);

    if failures > 0 {
        eprintln!("\n{failures} check(s) failed.");
        std::process::exit(1);
    }
    println!("\nTrim, re-trim and undo all add up.");
}

/// Every track at full — the mix should not distort anything here.
fn mix_for(clip: &Clip) -> Vec<TrackMix> {
    stems::tracks(&clip.id, &edit::source_path(clip))
        .unwrap_or_default()
        .iter()
        .map(|track| TrackMix {
            index: track.index,
            gain_db: 0.0,
            muted: false,
        })
        .collect()
}

/// Carry out one cut, measure afterwards and bring the clip to the new state.
fn step(clip: &mut Clip, trim: edit::Trim, mix: &[TrackMix], want_ms: u64) -> u32 {
    match edit::apply(clip, trim, mix, EncoderId::X264, 40_000, |value| {
        if value >= 1.0 {
            println!("  done");
        }
    }) {
        Ok(applied) => {
            report(applied.duration_ms, want_ms);
            let off = (applied.duration_ms as i64 - want_ms as i64).abs() > TOLERANCE_MS;
            clip.duration_ms = applied.duration_ms;
            clip.original = applied.original;
            u32::from(off)
        }
        Err(err) => {
            eprintln!("  ERROR: {err}");
            1
        }
    }
}

fn report(got: u64, want: u64) {
    let delta = got as i64 - want as i64;
    let mark = if delta.abs() <= TOLERANCE_MS { "ok" } else { "OFF" };
    println!("  measured {got} ms, expected {want} ms ({delta:+} ms) — {mark}");
}
