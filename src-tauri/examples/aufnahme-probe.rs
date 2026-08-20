//! Probeaufnahme: startet die Pipeline, puffert ein paar Sekunden und schreibt
//! einen Clip. Prüft damit in einem Lauf, was sich nicht in Unit-Tests fassen
//! lässt — Capture, Farbumwandlung, Encoder-MFT, Taktgeber und Muxen.
//!
//!     cargo run --example aufnahme-probe -- [sekunden]

use std::sync::Arc;
use std::time::{Duration, Instant};

use clippiboy_lib::audio::engine::AudioEngine;
use clippiboy_lib::model::{
    AudioSource, Clip, RecordingConfig, SourceKind, TargetKind, TrackMix,
};
use clippiboy_lib::{encode, muxer, pipeline::Pipeline, stems};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(8);

    let resources = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
    muxer::set_tool_dir(resources);
    println!("ffmpeg gefunden: {}", muxer::available());

    println!("\nEncoder laut Media Foundation:");
    for info in encode::list_encoders() {
        println!(
            "  {:<28} verfügbar={} hardware={}",
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
    println!("\nGewünschter Encoder: {:?}", recording.encoder);

    // Zwei Quellen mit eigener Spur. Sie liefern nichts (die WASAPI-Ströme
    // laufen hier gar nicht), aber sie erzeugen die zusätzlichen Spuren — und
    // genau deren Weg durch Muxer, Ablage und Speichern soll geprüft werden.
    let sources: Vec<AudioSource> = ["Mikrofon", "Discord"]
        .iter()
        .enumerate()
        .map(|(index, label)| AudioSource {
            id: format!("probe-{index}"),
            label: (*label).into(),
            kind: SourceKind::InputDevice {
                device_id: format!("nicht-vorhanden-{index}"),
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
            eprintln!("Pipeline startet nicht: {err}");
            std::process::exit(1);
        }
    };
    println!("Tatsächlicher Encoder: {:?}", *pipeline.shared.encoder.lock());

    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(seconds) {
        std::thread::sleep(Duration::from_secs(1));
        let shared = &pipeline.shared;
        let frames = shared.frames.load(std::sync::atomic::Ordering::Relaxed);
        let duplicated = shared.duplicated.load(std::sync::atomic::Ordering::Relaxed);
        println!(
            "  {:>2}s  Bilder={frames:<5} davon wiederholt={duplicated:<5} \
             gepuffert={:.1}s {:.1} MB",
            started.elapsed().as_secs(),
            shared.buffered_seconds(),
            shared.buffer_bytes() as f64 / 1_048_576.0,
        );
    }

    if let Some(err) = pipeline.shared.error.lock().take() {
        eprintln!("Fehler aus der Aufnahme: {err}");
    }

    let snapshot = match pipeline.snapshot(5) {
        Ok(snapshot) => snapshot,
        Err(err) => {
            eprintln!("Kein Schnappschuss: {err}");
            std::process::exit(1);
        }
    };
    let keyframes = snapshot.packets.iter().filter(|p| p.keyframe).count();
    println!(
        "\nSchnappschuss: {} Pakete, {keyframes} Keyframes, SPS/PPS {} Bytes",
        snapshot.packets.len(),
        snapshot.sequence_header.len()
    );
    println!(
        "Tonspuren: {} ({:?}), {} Frames ab QPC {}",
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
        Err(err) => eprintln!("Clip nicht geschrieben: {err}"),
    }

    pipeline.stop();
}

/// Was ffprobe über die fertige Datei sagt — Bildrate, Codec, Spuren.
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

/// Den Speichern-Weg prüfen: Mischung neu einrechnen und dabei nachmessen,
/// dass die Bildspur unangetastet bleibt und genau eine Tonspur übrig ist.
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
    };

    let tracks = match stems::tracks(&clip) {
        Ok(tracks) => tracks,
        Err(err) => {
            eprintln!("Spuren nicht lesbar: {err}");
            return;
        }
    };
    println!(
        "\nSpeichern-Probe: {} Spur(en){}",
        tracks.len(),
        if tracks.iter().all(|t| t.preview_path.is_some()) {
            ", einzeln abgelegt"
        } else {
            ", nur in der Datei"
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

    match stems::apply(&clip, &mix) {
        Ok(size) => {
            let after = video_frames(path);
            println!("  neu geschrieben: {:.1} MB", size as f64 / 1_048_576.0);
            println!("  Bilder vorher {before:?}, nachher {after:?}");
            assert_eq!(before, after, "Die Bildspur wurde angefasst!");
            probe(path);
        }
        Err(err) => eprintln!("  Speichern fehlgeschlagen: {err}"),
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
