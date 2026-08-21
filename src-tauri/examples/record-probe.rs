//! Trial recording: starts the pipeline, buffers a few seconds and writes a
//! clip. Checks in one run what unit tests cannot capture — capture, colour
//! conversion, the encoder MFT, the clock and muxing.
//!
//!     cargo run --example record-probe -- [seconds]

use std::sync::Arc;
use std::time::{Duration, Instant};

use clippiboy_lib::audio::engine::AudioEngine;
use clippiboy_lib::model::{
    AudioSource, Clip, EncoderId, RecordingConfig, SourceKind, TargetKind, TrackMix,
};
use clippiboy_lib::{edit, encode, muxer, pipeline::Pipeline, stems};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(8);

    let resources = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
    muxer::set_tool_dir(resources);
    println!("ffmpeg found: {}", muxer::available());

    println!("\nEncoders according to Media Foundation:");
    for info in encode::list_encoders() {
        println!(
            "  {:<28} available={} hardware={}",
            info.name, info.available, info.hardware
        );
    }

    let recording = RecordingConfig {
        target_kind: TargetKind::Monitor,
        target_id: None,
        width: 1920,
        height: 1080,
        fps: 60,
        bitrate_kbps: 40_000,
        encoder: encode::preferred_encoder(),
        keyframe_seconds: 2,
    };
    println!("\nRequested encoder: {:?}", recording.encoder);

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
    let mut pipeline = match Pipeline::start(&recording, 60, sources, audio) {
        Ok(pipeline) => pipeline,
        Err(err) => {
            eprintln!("pipeline will not start: {err}");
            std::process::exit(1);
        }
    };
    println!("Actual encoder: {:?}", *pipeline.shared.encoder.lock());

    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(seconds) {
        std::thread::sleep(Duration::from_secs(1));
        let shared = &pipeline.shared;
        let frames = shared.frames.load(std::sync::atomic::Ordering::Relaxed);
        let duplicated = shared.duplicated.load(std::sync::atomic::Ordering::Relaxed);
        println!(
            "  {:>2}s  frames={frames:<5} of those repeated={duplicated:<5} \
             buffered={:.1}s {:.1} MB",
            started.elapsed().as_secs(),
            shared.buffered_seconds(),
            shared.buffer_bytes() as f64 / 1_048_576.0,
        );
    }

    if let Some(err) = pipeline.shared.error.lock().take() {
        eprintln!("error from the recording: {err}");
    }

    let snapshot = match pipeline.snapshot(5) {
        Ok(snapshot) => snapshot,
        Err(err) => {
            eprintln!("no snapshot: {err}");
            std::process::exit(1);
        }
    };
    let keyframes = snapshot.packets.iter().filter(|p| p.keyframe).count();
    println!(
        "\nSnapshot: {} packets, {keyframes} keyframes, SPS/PPS {} bytes",
        snapshot.packets.len(),
        snapshot.sequence_header.len()
    );
    println!(
        "Audio tracks: {} ({:?}), {} frames from QPC {}",
        snapshot.tracks.len(),
        snapshot.tracks.iter().map(|t| t.label.clone()).collect::<Vec<_>>(),
        snapshot.audio_frames,
        snapshot.start_100ns
    );

    let temp = std::env::temp_dir().join("clippiboy-probe");
    let output = temp.join("probe.mp4");
    let _ = std::fs::create_dir_all(&temp);
    match muxer::build(muxer::ClipRequest {
        snapshot,
        clip_id: "probe".into(),
        output: output.clone(),
        temp_dir: temp.clone(),
    }) {
        Ok(result) => {
            println!(
                "\nClip: {} ({:.1} MB, {} ms)",
                result.path.display(),
                result.size_bytes as f64 / 1_048_576.0,
                result.duration_ms
            );
            probe(&result.path);
            check_save(&result.path, result.duration_ms);
        }
        Err(err) => eprintln!("clip not written: {err}"),
    }

    pipeline.stop();
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
        Err(err) => eprintln!("ffprobe: {err}"),
    }
}

/// Check the save path: reapply the mix and measure that the video track stays
/// untouched and exactly one audio track is left.
fn check_save(path: &std::path::Path, duration_ms: u64) {
    let before = video_frames(path);
    let clip = Clip {
        id: "probe".into(),
        path: path.to_string_lossy().to_string(),
        created_at: 0,
        duration_ms,
        game: None,
        width: 1920,
        height: 1080,
        size_bytes: 0,
        thumb_path: None,
        title: None,
        description: None,
        edit: None,
        original: None,
    };

    let tracks = match stems::tracks(&clip.id, &edit::source_path(&clip)) {
        Ok(tracks) => tracks,
        Err(err) => {
            eprintln!("tracks not readable: {err}");
            return;
        }
    };
    println!(
        "\nSave probe: {} track(s){}",
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

    // With no trim: the video has to stay identical frame for frame.
    let whole = edit::Trim::whole(clip.duration_ms);
    match edit::apply(&clip, whole, &mix, EncoderId::X264, 40_000, |_| {}) {
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
        Err(err) => eprintln!("  save failed: {err}"),
    }
    stems::remove("probe");
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
