//! Prüft das Entpacken der Einzelspuren gegen den Fall, der sie kaputt gemacht
//! hat: zwei gleichzeitige Anfragen für denselben Clip.
//!
//! Genau das passiert im Player, weil `React.StrictMode` den Effekt doppelt
//! mountet. Vorher schrieben dann zwei ffmpeg-Läufe in dieselben Zieldateien.
//!
//!     cargo run --example spuren-probe -- "C:\\Pfad\\zum\\clip.mp4"

use std::sync::Arc;

use clippiboy_lib::model::Clip;
use clippiboy_lib::{muxer, stems};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let Some(path) = std::env::args().nth(1) else {
        eprintln!("Aufruf: spuren-probe <clip.mp4>");
        std::process::exit(2);
    };
    let resources = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
    muxer::set_tool_dir(resources);

    let clip = Arc::new(Clip {
        id: "spuren-probe".into(),
        path: path.clone(),
        created_at: 0,
        duration_ms: 0,
        game: None,
        width: 0,
        height: 0,
        size_bytes: 0,
        thumb_path: None,
        title: None,
        description: None,
        edit: None,
    });

    // Mit einem sauberen Stand anfangen, sonst prüft der Lauf nur den
    // Kurzschluss über den Index.
    stems::remove(&clip.id);

    println!("Zwei gleichzeitige Anfragen auf {path}");
    let handles: Vec<_> = (0..2)
        .map(|n| {
            let clip = clip.clone();
            std::thread::spawn(move || (n, stems::tracks(&clip)))
        })
        .collect();

    let mut listed = Vec::new();
    for handle in handles {
        match handle.join().expect("Faden abgestürzt") {
            (n, Ok(tracks)) => {
                println!("  Anfrage {n}: {} Spur(en)", tracks.len());
                listed = tracks;
            }
            (n, Err(err)) => {
                eprintln!("  Anfrage {n} fehlgeschlagen: {err}");
                std::process::exit(1);
            }
        }
    }

    println!("\nJede abgelegte Spur dekodieren:");
    let mut broken = 0;
    for track in &listed {
        let Some(file) = track.preview_path.as_deref() else {
            println!("  {:>2}  {:<40} (nur in der Clipdatei)", track.index, track.label);
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
                "  {:>2}  {:<40} {size:>9} B  KAPUTT: {}",
                track.index,
                track.label,
                complaint.lines().next().unwrap_or_default()
            );
        }
    }

    // Und jetzt der Weg, der beim Nutzer gescheitert ist: aus genau diesen
    // Spuren eine einzige Tonspur mischen und in den Clip schreiben.
    let mix: Vec<clippiboy_lib::model::TrackMix> = listed
        .iter()
        .enumerate()
        .map(|(slot, track)| clippiboy_lib::model::TrackMix {
            index: track.index,
            // Unterschiedliche Pegel, damit die Filterkette wirklich rechnet.
            gain_db: -3.0 * slot as f32,
            muted: false,
        })
        .collect();
    println!("\nSpeichern mit gemischten Pegeln:");
    match stems::apply(&clip, &mix) {
        Ok(size) => println!("  geschrieben, {:.1} MB", size as f64 / 1_048_576.0),
        Err(err) => {
            eprintln!("  fehlgeschlagen: {err}");
            stems::remove(&clip.id);
            std::process::exit(1);
        }
    }

    stems::remove(&clip.id);
    if broken > 0 {
        eprintln!("\n{broken} Spur(en) beschädigt.");
        std::process::exit(1);
    }
    println!("\nAlle Spuren sind lesbar.");
}
