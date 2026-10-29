//! Lays a cam over a clip the way the background composite does, and prints
//! the ffmpeg line and how fast it ran.
//!
//!     cargo run --example composite-probe -- <clip.mp4> <cam.mp4> <out.mp4>
//!         [--offset-us N] [--encoder nvenc|amf|qsv|x264] [--shape rect|rounded|circle] [--mirror]

use std::path::Path;
use std::time::Instant;

use clippiboy_lib::composite::{self, CompositeJob};
use clippiboy_lib::facecam::FacecamFacts;
use clippiboy_lib::model::{CamLayout, CamShape, EncoderId};
use clippiboy_lib::{camera::ClockKind, muxer};

fn arg(args: &[String], key: &str) -> Option<String> {
    args.iter().position(|a| a == key).and_then(|i| args.get(i + 1)).cloned()
}

fn size_of(path: &Path) -> (u32, u32) {
    let out = muxer::command("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .expect("ffprobe");
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parts = text.trim().split(',').map(|v| v.parse().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (clip, cam, output) = (Path::new(&args[1]), Path::new(&args[2]), Path::new(&args[3]));
    let offset_us: i64 = arg(&args, "--offset-us").and_then(|v| v.parse().ok()).unwrap_or(-412_000);
    let encoder = match arg(&args, "--encoder").as_deref() {
        Some("amf") => EncoderId::Amf,
        Some("qsv") => EncoderId::Qsv,
        Some("x264") => EncoderId::X264,
        _ => EncoderId::Nvenc,
    };
    let shape = match arg(&args, "--shape").as_deref() {
        Some("rect") => CamShape::Rect,
        Some("circle") => CamShape::Circle,
        _ => CamShape::Rounded,
    };
    let layout = CamLayout { shape, mirror: args.iter().any(|a| a == "--mirror"), ..CamLayout::default() };
    let frame = size_of(clip);
    let cam_size = size_of(cam);
    let length_ms = muxer::probe_duration_ms(clip).expect("clip length");
    let video_start_us = muxer::probe_video_start_us(clip).unwrap_or(0);
    let facts = FacecamFacts {
        version: 1,
        width: cam_size.0,
        height: cam_size.1,
        fps: 30,
        cam_offset_us: offset_us,
        frames: 0,
        clip_length_us: length_ms * 1000,
        gaps_ms: vec![],
        clock: ClockKind::Device,
        camera: "probe".into(),
        encoder: Some(encoder),
        game_gop: 120,
        video_start_us: Some(video_start_us),
        applied: None,
        applying: None,
        swap_failing_since_ms: None,
    };
    let rect = composite::cam_rect(frame, &layout);
    let mask_path = output.with_extension("mask.png");
    let mask = composite::mask_gray8(shape, rect.w, rect.h).map(|pixels| {
        composite::write_mask_png(&mask_path, rect.w, rect.h, &pixels).expect("mask");
        mask_path.as_path()
    });
    let job = CompositeJob {
        clip,
        cam,
        mask,
        output,
        frame,
        facts: &facts,
        layout,
        delay_ms: -40,
        video_start_us,
        start_ms: 0,
        length_ms,
        gaps_ms: &[],
        encoder,
        quality: 70,
        gop: 120,
        hwaccel: true,
    };
    let line = composite::composite_arguments(&job);
    println!("ffmpeg {}", line.join(" "));
    let start = Instant::now();
    let run = muxer::command("ffmpeg").args(&line).output().expect("ffmpeg");
    let took = start.elapsed().as_secs_f64();
    println!(
        "exit {:?} in {took:.1} s ({:.1}x realtime) {}",
        run.status.code(),
        length_ms as f64 / 1000.0 / took,
        String::from_utf8_lossy(&run.stderr).trim()
    );
    println!("output length {:?} ms, clip {length_ms} ms", muxer::probe_duration_ms(output));
}
