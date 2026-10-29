//! Writes the 9:16 export of a clip from the library, as the export dialog
//! would, to check it by eye and with ffprobe.
//!
//!     cargo run --example portrait-probe -- <clip id> <output.mp4>

use std::path::Path;
use std::process::Command;

use clippiboy_lib::model::EncoderId;
use clippiboy_lib::{clips::Library, commands, export};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(id), Some(output)) = (args.next(), args.next()) else {
        eprintln!("usage: portrait-probe <clip id> <output.mp4>");
        std::process::exit(2);
    };
    let library = Library::open().expect("the library");
    let clip = library.get(&id).expect("readable").expect("the clip");
    let target = export::Target { bytes: 50 * 1024 * 1024, duration_ms: clip.duration_ms, width: export::PORTRAIT.0, height: export::PORTRAIT.1 };
    let recipe = export::recipe(&target);
    let job = commands::portrait_job(&clip, clip.duration_ms, (recipe.width, recipe.height));
    let line = export::portrait_arguments(&job.inputs(), &job.graph, Path::new(&output), &recipe, clip.duration_ms, EncoderId::X264);
    println!("graph: {}", job.graph);
    let ffmpeg = format!("{}\\gg.clippiboy.app\\ffmpeg\\9.0.1\\ffmpeg.exe", std::env::var("LOCALAPPDATA").unwrap_or_default());
    let started = std::time::Instant::now();
    let status = Command::new(ffmpeg).args(&line).stdout(std::process::Stdio::null()).status().expect("ffmpeg runs");
    println!("ffmpeg {status} in {:.1} s", started.elapsed().as_secs_f32());
}
