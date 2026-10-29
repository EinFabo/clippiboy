//! Runs the cam track on its own for a few seconds — worker, helper, scaler,
//! encoder, ring — prints its health line every second, stops it, and writes
//! what the ring held to an H.264 file for ffprobe.
//!
//!     cargo run --example cam-track -- [camera name] [seconds] [out.h264]

use std::io::Write;
use std::time::Duration;

use clippiboy_lib::camtrack::{CamPlan, CamTrack};
use clippiboy_lib::model::{CamResolution, EncoderId};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let name = args.get(1).cloned();
    let seconds: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(12);
    let out = args.get(3).cloned().unwrap_or_else(|| "cam-track.h264".into());
    let plan = |run: bool| CamPlan {
        run,
        stop_after: Duration::ZERO,
        device_id: None,
        device_name: name.clone(),
        resolution: CamResolution::P720,
        fps: 30,
        ring_seconds: 60,
        encoder: clippiboy_lib::encode::for_recording(EncoderId::Nvenc),
        preview: false,
    };
    let track = CamTrack::new();
    track.apply(plan(true));
    for _ in 0..seconds {
        std::thread::sleep(Duration::from_secs(1));
        match track.status() {
            Some(status) => println!("{:?} | {}", status.state, clippiboy_lib::camtrack::health_text(&status)),
            None => println!("no status yet"),
        }
    }
    let (header, packets, session, gaps) = {
        let ring = track.ring().lock();
        (ring.header.clone(), ring.ring.snapshot_range(0, i64::MAX), ring.session, ring.gaps.len())
    };
    println!(
        "ring: session {session}, {} packets, header {} bytes, {} keyframes, gaps {gaps}, first pts {:?} last pts {:?}",
        packets.len(),
        header.len(),
        packets.iter().filter(|p| p.keyframe).count(),
        packets.first().map(|p| p.pts_us),
        packets.last().map(|p| p.pts_us)
    );
    let mut file = std::fs::File::create(&out).expect("output file");
    file.write_all(&header).unwrap();
    for packet in &packets {
        file.write_all(&packet.data).unwrap();
    }
    println!("wrote {out}");
    track.apply(plan(false));
    std::thread::sleep(Duration::from_secs(4));
    println!("after stop: status {:?}", track.status().map(|s| s.state));
}
