//! Wegwerfbare Hilfsdateien für den Player.
//!
//! Alles hier lässt sich in einer Sekunde neu erzeugen und wird beim Start
//! weggeräumt. **Nicht** hierher gehören die Einzelspuren eines Clips — die
//! sind das Original der Mischung und liegen in `stems.rs`.

use std::path::{Path, PathBuf};

use crate::config;
use crate::model::Clip;
use crate::muxer::{ffmpeg, run, sanitize};
use crate::stems::is_newer;

/// Ordner für die Hilfsdateien. Liegt im Datenverzeichnis, weil der
/// Clip-Ordner dem Nutzer gehört und keine Hilfsdateien vertragen soll.
pub fn dir() -> PathBuf {
    config::data_dir().join("preview")
}

/// Beim Start aufräumen: Die Dateien gehören zu Clips, die es womöglich nicht
/// mehr gibt.
pub fn clear() {
    let _ = std::fs::remove_dir_all(dir());
    // Gleich wieder anlegen: der Player bekommt den Ordner beim Start
    // freigegeben, und ein fehlender Ordner ist unnötig zu erklären.
    let _ = std::fs::create_dir_all(dir());
}

/// Ein Bild der Tonspur für die Zeitleiste. Wo etwas passiert, sieht man damit
/// vor dem Hinhören — das Suchen nach der richtigen Stelle ist sonst reines
/// Blindtasten.
///
/// Ein transparentes PNG, damit die Leiste ihre eigene Farbe behält.
pub fn waveform(clip: &Clip) -> Result<PathBuf, String> {
    let dir = dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let out = dir.join(format!("{}_wave.png", sanitize(&clip.id)));
    if is_newer(&out, Path::new(&clip.path)) {
        return Ok(out);
    }

    let mut draw = ffmpeg();
    draw.args(["-y", "-hide_banner", "-loglevel", "error"])
        .arg("-i")
        .arg(&clip.path)
        // `split_channels=0` legt links und rechts übereinander: Die Leiste ist
        // 40 Pixel hoch, zwei getrennte Kanäle wären darin nicht mehr zu
        // unterscheiden.
        .args([
            "-filter_complex",
            "[0:a:0]aformat=channel_layouts=mono,\
             showwavespic=s=1920x64:colors=white:split_channels=0[wave]",
            "-map",
            "[wave]",
            "-frames:v",
            "1",
        ])
        .arg(&out);
    run(&mut draw, "Wellenform zeichnen")?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stems;

    /// `clear()` löscht seinen Ordner mitsamt Inhalt. Läge die Ablage der
    /// Einzelspuren darin, wären nach jedem App-Start sämtliche Mischungen für
    /// immer unveränderbar — und zwar ohne Fehlermeldung.
    #[test]
    fn the_stems_are_not_inside_the_scratch_folder() {
        assert!(
            !stems::root().starts_with(dir()),
            "Die Einzelspuren dürfen nicht im Wegwerf-Ordner liegen"
        );
        assert_ne!(stems::root(), dir());
    }
}
