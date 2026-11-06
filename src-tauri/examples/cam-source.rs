//! Opens a camera through `camera::open` for 20 s and prints, once a second,
//! the frames that came, their size and the clock they were stamped on, plus
//! every event other than a frame. For trying the camera on a machine.
//!
//!     cargo run --example cam-source -- [index] [shared]
//!     cargo run --example cam-source -- [index] holder   (who streams it now)

use std::time::{Duration, Instant};

use clippiboy_lib::camera::{self, Event, OpenMode};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let index = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(0usize);
    let mode = if args.iter().any(|a| a == "shared") {
        OpenMode::MfSensorShared
    } else {
        OpenMode::Controlling { width: 1280, height: 720 }
    };
    let list = camera::list();
    for (i, device) in list.iter().enumerate() {
        println!("[{i}] {}", device.name);
    }
    let Some(device) = list.get(index) else {
        eprintln!("no camera {index}");
        std::process::exit(2);
    };
    if args.iter().any(|a| a == "holder") {
        println!("held by {:?}", camera::windows::holder(device));
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let source = match camera::open(
        device,
        mode,
        Box::new(move |event| {
            let _ = tx.send(event);
        }),
        &|| false,
    ) {
        Ok(source) => source,
        Err(err) => {
            eprintln!("open failed: {err} ({err:?})");
            std::process::exit(2);
        }
    };
    println!("opened {} ({mode:?}) at {:?}", device.name, source.size());
    let start = Instant::now();
    let (mut second, mut frames, mut clock, mut size) = (1u64, 0u32, None, (0, 0));
    while start.elapsed() < Duration::from_secs(20) {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Event::Frame(frame)) => {
                frames += 1;
                clock = Some(frame.probe.clock);
                size = (frame.probe.width, frame.probe.height);
            }
            Ok(Event::FormatChanged { width, height }) => println!("format changed: {width}x{height}"),
            Ok(Event::Lost(err)) => println!("lost: {err} ({err:?})"),
            Err(_) => {}
        }
        if start.elapsed() >= Duration::from_secs(second) {
            println!("t={second:>2}s {frames:>2} fps {}x{} clock {clock:?}", size.0, size.1);
            frames = 0;
            second += 1;
        }
    }
    let closing = Instant::now();
    source.close();
    println!("closed in {} ms", closing.elapsed().as_millis());
}
