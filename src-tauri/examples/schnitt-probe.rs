//! Schneidet einen echten Clip und misst nach, ob dabei herauskommt, was
//! herauskommen soll — und ob „Zuschnitt aufheben" ihn wirklich zurückbringt.
//!
//! Die Keyframe- und Synchronfragen lassen sich im Unit-Test nicht belegen:
//! Dafür braucht es ffmpeg, eine echte Datei und ffprobe, das nachmisst.
//!
//!     cargo run --example schnitt-probe -- "C:\\Pfad\\zum\\clip.mp4"
//!
//! **Achtung:** Der Lauf schreibt die übergebene Datei um. Er legt sie dabei in
//! die Original-Ablage und holt sie am Ende zurück — sicherheitshalber trotzdem
//! auf einer Kopie laufen lassen.

use std::path::Path;

use clippiboy_lib::model::{Clip, ClipOriginal, EncoderId, TrackMix};
use clippiboy_lib::{edit, muxer, stems};

/// So weit darf die gemessene Länge danebenliegen. Ein Bild bei 60 fps sind
/// 17 ms; der Container rundet auf ganze Ticks, und der letzte Ton reicht oft
/// ein paar Millisekunden über das letzte Bild hinaus.
const TOLERANCE_MS: i64 = 120;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let Some(path) = std::env::args().nth(1) else {
        eprintln!("Aufruf: schnitt-probe <clip.mp4>");
        std::process::exit(2);
    };
    let resources = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
    muxer::set_tool_dir(resources);

    let id = "schnitt-probe";
    // Mit sauberem Stand anfangen — sonst prüft der Lauf einen halben Rest von
    // vorhin.
    stems::remove(id);
    edit::remove(id);

    let full = match muxer::probe_duration_ms(Path::new(&path)) {
        Some(ms) if ms > 8_000 => ms,
        Some(ms) => {
            eprintln!("Der Clip ist mit {ms} ms zu kurz zum Schneiden — mindestens 8 s.");
            std::process::exit(2);
        }
        None => {
            eprintln!("Länge nicht lesbar. Ist die Datei da?");
            std::process::exit(2);
        }
    };
    println!("Ausgangsclip: {path}\n  {full} ms");

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
        edit: None,
        original: None,
    };

    let mix = mix_for(&clip);
    let mut failures = 0;

    // 1. Nur hinten kürzen. Verlustfrei, in ein bis zwei Sekunden durch — und
    //    das Bild muss dabei Bild für Bild dasselbe bleiben.
    let want = full - 3_000;
    println!("\n1. Hinten kürzen auf {want} ms (Kopie, kein Neuencodieren)");
    let plan = edit::plan(None, true, full, edit::Trim { start_ms: 0, end_ms: want });
    assert!(!plan.reencode, "Kürzen am Ende darf nicht encodieren");
    failures += step(&mut clip, edit::Trim { start_ms: 0, end_ms: want }, &mix, want);

    // 2. Vorne schneiden. Muss bildgenau sitzen — beim Kopieren rutschte der
    //    Anfang auf das Keyframe davor, also bis zu zwei Sekunden zu früh.
    println!("\n2. Vorne 2000 ms abschneiden (bildgenau, wird neu encodiert)");
    let before = clip.duration_ms;
    let trim = edit::Trim { start_ms: 2_000, end_ms: before };
    failures += step(&mut clip, trim, &mix, before - 2_000);
    match clip.original {
        Some(ClipOriginal { start_ms, duration_ms, .. }) => {
            println!("  Ausschnitt sitzt bei {start_ms} ms von {duration_ms} ms");
            if start_ms != 2_000 {
                eprintln!("  FEHLER: erwartet 2000 ms");
                failures += 1;
            }
        }
        None => {
            eprintln!("  FEHLER: kein Original vermerkt");
            failures += 1;
        }
    }

    // 3. Aufheben. Muss die volle Länge zurückbringen, verlustfrei.
    println!("\n3. Zuschnitt aufheben");
    match edit::restore(&clip, &mix, EncoderId::X264, 40_000, |_| {}) {
        Ok(applied) => {
            report(applied.duration_ms, full);
            if (applied.duration_ms as i64 - full as i64).abs() > TOLERANCE_MS {
                failures += 1;
            }
            if applied.original.is_some() {
                eprintln!("  FEHLER: das Original hätte weg sein müssen");
                failures += 1;
            }
            if edit::has_original(id) {
                eprintln!("  FEHLER: die Ablage liegt noch da");
                failures += 1;
            }
        }
        Err(err) => {
            eprintln!("  FEHLER: {err}");
            failures += 1;
        }
    }

    stems::remove(id);
    edit::remove(id);

    if failures > 0 {
        eprintln!("\n{failures} Prüfung(en) fehlgeschlagen.");
        std::process::exit(1);
    }
    println!("\nSchnitt, Nachschnitt und Aufheben stimmen.");
}

/// Alle Spuren auf Anschlag — die Mischung soll hier nichts verfälschen.
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

/// Einen Schnitt ausführen, nachmessen und den Clip auf den neuen Stand ziehen.
fn step(clip: &mut Clip, trim: edit::Trim, mix: &[TrackMix], want_ms: u64) -> u32 {
    match edit::apply(clip, trim, mix, EncoderId::X264, 40_000, |value| {
        if value >= 1.0 {
            println!("  fertig");
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
            eprintln!("  FEHLER: {err}");
            1
        }
    }
}

fn report(got: u64, want: u64) {
    let delta = got as i64 - want as i64;
    let mark = if delta.abs() <= TOLERANCE_MS { "ok" } else { "DANEBEN" };
    println!("  gemessen {got} ms, erwartet {want} ms ({delta:+} ms) — {mark}");
}
