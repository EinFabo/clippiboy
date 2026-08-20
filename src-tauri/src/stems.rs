//! Die Einzelspuren („Stems") eines Clips.
//!
//! Die Clipdatei selbst hat genau **eine** Tonspur, in der alles steckt —
//! Spielton, Mikrofon, Discord. Nur so hört man in Discord, im Browser und in
//! jedem Player wirklich alles: Die meisten spielen von einem MP4 stur die
//! erste Tonspur ab. Vorher lagen die Quellen als zusätzliche Spuren in
//! derselben Datei und waren damit überall stumm außer im Editor.
//!
//! Damit sich die Mischung trotzdem jederzeit ändern lässt, liegen die rohen
//! Einzelspuren hier daneben — je Clip ein Ordner im Datenverzeichnis:
//!
//! ```text
//! <data>/tracks/<clip-id>/0.m4a
//!                        /1.m4a
//!                        /spuren.json
//! ```
//!
//! Der Clipordner gehört dem Nutzer und bleibt frei von Hilfsdateien.

use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::config;
use crate::model::{ClipTrack, TrackMix};
use crate::muxer::{command, ffmpeg, run, sanitize};

/// Ausgabemarke der Mischkette aus [`mix_filter`].
pub const MIX_LABEL: &str = "[aout]";

/// Ab hier gilt eine Spur als stumm — unterhalb ist sie ohnehin nicht mehr zu
/// hören, und ffmpeg muss sie dann gar nicht erst mitschleppen.
const SILENT_DB: f32 = -59.0;

/// Wurzel aller Stems. **Dauerhaft** — anders als der Vorschauordner, der beim
/// Start weggeräumt wird.
pub fn root() -> PathBuf {
    config::data_dir().join("tracks")
}

/// Ordner eines Clips.
pub fn dir(clip_id: &str) -> PathBuf {
    root().join(sanitize(clip_id))
}

/// Datei einer einzelnen Spur.
pub fn track_path(clip_id: &str, index: u32) -> PathBuf {
    dir(clip_id).join(format!("{index}.m4a"))
}

fn index_path(clip_id: &str) -> PathBuf {
    dir(clip_id).join("spuren.json")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    index: u32,
    label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Index {
    tracks: Vec<Entry>,
}

/// Nur ein Entpacken auf einmal.
///
/// Der Player fragt die Spuren beim Öffnen an, und im Entwicklungsmodus tut er
/// das wegen `React.StrictMode` gleich zweimal. Ohne diese Sperre liefen zwei
/// ffmpeg-Läufe gleichzeitig auf dieselben Zieldateien — herausgekommen sind
/// Dateien, die zwar die richtige Länge meldeten, sich aber nicht dekodieren
/// ließen („channel element 0.0 duplicate"). Beim Speichern brach ffmpeg dann
/// mittendrin ab.
static EXTRACTING: Mutex<()> = Mutex::new(());

/// Die Namen der Spuren ablegen. Aus der Datei allein ginge nur die Nummer
/// hervor, und „1.m4a" sagt niemandem, dass darin das Mikrofon liegt.
pub fn write_index(clip_id: &str, labels: &[String]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir(clip_id))?;
    let index = Index {
        tracks: labels
            .iter()
            .enumerate()
            .map(|(index, label)| Entry {
                index: index as u32,
                label: label.clone(),
            })
            .collect(),
    };
    let text = serde_json::to_string_pretty(&index)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    // Der Index ist das Signal „hier ist alles vollständig" — er wird deshalb
    // zuletzt und in einem Zug geschrieben.
    let temp = index_path(clip_id).with_extension("teil");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, index_path(clip_id))
}

/// Die abgelegten Spuren als Spurenliste, sofern vollständig vorhanden.
fn from_index(clip_id: &str) -> Option<Vec<ClipTrack>> {
    Some(
        read_index(clip_id)?
            .into_iter()
            .map(|entry| ClipTrack {
                index: entry.index,
                label: entry.label,
                channels: 2,
                preview_path: Some(
                    track_path(clip_id, entry.index).to_string_lossy().to_string(),
                ),
            })
            .collect(),
    )
}

/// Abgelegte Spuren, sofern vollständig vorhanden.
fn read_index(clip_id: &str) -> Option<Vec<Entry>> {
    let text = std::fs::read_to_string(index_path(clip_id)).ok()?;
    let index: Index = serde_json::from_str(&text).ok()?;
    // Fehlt auch nur eine Datei, ist die Ablage unbrauchbar — dann lieber neu
    // aus der Clipdatei ziehen, als dem Editor eine tote Spur anzubieten.
    index
        .tracks
        .iter()
        .all(|entry| track_path(clip_id, entry.index).is_file())
        .then_some(index.tracks)
}

/// Alles zu einem Clip wegräumen.
pub fn remove(clip_id: &str) {
    let _ = std::fs::remove_dir_all(dir(clip_id));
}

/// Die Spuren eines Clips — aus der Ablage, sonst aus `source` geholt.
///
/// Bei Clips aus der Zeit vor der Umstellung liegen die Quellen noch als
/// zusätzliche Tonspuren im MP4. Die werden hier einmalig herausgezogen; ab
/// dann ist die Ablage die Quelle.
///
/// `source` ist bewusst nicht `clip.path`: Sobald ein Clip geschnitten ist,
/// liegt die unversehrte Aufnahme in der Original-Ablage, und nur die darf
/// hier entpackt werden — sonst wäre die Ablage selbst geschnitten und die
/// Vorschau liefe um den Zuschnitt versetzt. Den richtigen Pfad liefert
/// [`crate::edit::source_path`].
pub fn tracks(clip_id: &str, source: &Path) -> Result<Vec<ClipTrack>, String> {
    if let Some(tracks) = from_index(clip_id) {
        return Ok(tracks);
    }

    // Ab hier wird geschrieben, also einer nach dem anderen. Wer gewartet hat,
    // findet die Ablage danach fertig vor und muss nichts mehr tun.
    let _busy = EXTRACTING.lock();
    if let Some(tracks) = from_index(clip_id) {
        return Ok(tracks);
    }

    let in_file = tracks_in_file(source)?;
    if in_file.len() <= 1 {
        // Eine Spur: Die Tonspur der Clipdatei *ist* das Original, eine Kopie
        // daneben wäre nur doppelter Platz. Ohne `previewPath` spielt der
        // Player sie direkt aus dem Video ab.
        return Ok(in_file);
    }

    // Altbestand: die Spuren aus dem MP4 in die Ablage holen.
    let mut extracted = Vec::with_capacity(in_file.len());
    for track in &in_file {
        let path = extract(clip_id, source, track.index)?;
        extracted.push(ClipTrack {
            preview_path: Some(path.to_string_lossy().to_string()),
            ..track.clone()
        });
    }
    let labels: Vec<String> = extracted.iter().map(|t| t.label.clone()).collect();
    write_index(clip_id, &labels).map_err(|err| err.to_string())?;
    Ok(extracted)
}

/// Die Tonspuren einer Clipdatei, in der Reihenfolge der Datei.
pub fn tracks_in_file(source: &Path) -> Result<Vec<ClipTrack>, String> {
    let output = command("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a",
            "-show_entries",
            "stream=index,channels:stream_tags",
            "-of",
            "json",
        ])
        .arg(source)
        .output()
        .map_err(|err| format!("ffprobe konnte nicht gestartet werden: {err}"))?;
    if !output.status.success() {
        return Err("Die Tonspuren des Clips ließen sich nicht lesen.".into());
    }

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|err| err.to_string())?;
    let streams = json
        .get("streams")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();

    Ok(streams
        .iter()
        .enumerate()
        .map(|(index, stream)| ClipTrack {
            index: index as u32,
            label: label_of(stream, index),
            channels: stream.get("channels").and_then(|c| c.as_u64()).unwrap_or(2) as u32,
            preview_path: None,
        })
        .collect())
}

/// Nichtssagende Vorgabewerte, die ffmpeg und andere Werkzeuge als Handler in
/// jede MP4-Datei schreiben.
const GENERIC_LABELS: [&str; 4] = [
    "soundhandler",
    "videohandler",
    "gpac iso audio handler",
    "mainconcept mp4 sound media handler",
];

/// Spurname aus den Metadaten. Wohin ein Titel in einer MP4-Datei wandert,
/// hängt am schreibenden Werkzeug: ffmpeg legt ihn als `name` ab, liest ihn
/// aber auch aus `title` oder `handler_name`.
fn label_of(stream: &serde_json::Value, index: usize) -> String {
    ["title", "name", "handler_name"]
        .iter()
        .filter_map(|key| {
            stream
                .pointer(&format!("/tags/{key}"))
                .and_then(|value| value.as_str())
                .map(str::trim)
        })
        .find(|value| {
            !value.is_empty() && !GENERIC_LABELS.contains(&value.to_ascii_lowercase().as_str())
        })
        .map(String::from)
        .unwrap_or_else(|| match index {
            0 => "Hauptmix".to_string(),
            other => format!("Spur {}", other + 1),
        })
}

/// Eine Tonspur aus `source` in die Ablage holen.
fn extract(clip_id: &str, source: &Path, index: u32) -> Result<PathBuf, String> {
    let out = track_path(clip_id, index);
    std::fs::create_dir_all(dir(clip_id)).map_err(|e| e.to_string())?;
    // Erst neben das Ziel schreiben und dann verschieben: Unter dem Zielnamen
    // darf nie eine halbe Datei liegen. Eine solche meldet die richtige Länge
    // und fällt erst auf, wenn sie jemand dekodieren will.
    let temp = dir(clip_id).join(format!("{index}.{}.teil", std::process::id()));

    let mut copy = ffmpeg();
    copy.args(["-y", "-hide_banner", "-loglevel", "error"])
        .arg("-i")
        .arg(source)
        .args(["-map", &format!("0:a:{index}"), "-c:a", "copy"])
        .args(["-movflags", "+faststart"])
        .arg("-f")
        .arg("mp4")
        .arg(&temp);
    let mut outcome = run(&mut copy, "Tonspur entpacken");

    if outcome.is_err() {
        // Liegt die Spur in einem Format, das ein MP4 nicht aufnimmt (etwa
        // PCM), hilft nur neu encodieren.
        let mut encode = ffmpeg();
        encode
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .arg("-i")
            .arg(source)
            .args(["-map", &format!("0:a:{index}"), "-c:a", "aac", "-b:a", "192k"])
            .args(["-movflags", "+faststart"])
            .arg("-f")
            .arg("mp4")
            .arg(&temp);
        outcome = run(&mut encode, "Tonspur entpacken");
    }

    if let Err(err) = outcome {
        let _ = std::fs::remove_file(&temp);
        return Err(err);
    }
    std::fs::rename(&temp, &out).map_err(|err| {
        let _ = std::fs::remove_file(&temp);
        format!("Tonspur konnte nicht abgelegt werden: {err}")
    })?;
    Ok(out)
}

/// dB in linearen Faktor, gerundet auf drei Stellen für die ffmpeg-Zeile.
pub fn factor(db: f32) -> f32 {
    (crate::audio::gain_factor(db) * 1000.0).round() / 1000.0
}

/// Filterkette, die die angegebenen Eingänge mit ihren Pegeln zu **einer**
/// Spur unter [`MIX_LABEL`] summiert.
///
/// `inputs` sind ffmpeg-Streamangaben wie `"1:a"` samt Pegel in dB. `None`
/// heißt: Es bleibt nichts Hörbares übrig, der Clip bekommt keinen Ton.
///
/// Diese Kette brauchen zwei Seiten — die Aufnahme, wenn sie die frisch
/// gemischten Spuren zusammenlegt, und das Speichern, wenn es die Mischung
/// später ändert. Deshalb steht sie hier und nicht zweimal dort.
pub fn mix_filter(inputs: &[(String, f32)]) -> Option<String> {
    let audible: Vec<&(String, f32)> = inputs
        .iter()
        .filter(|(_, gain_db)| *gain_db > SILENT_DB)
        .collect();
    if audible.is_empty() {
        return None;
    }

    let mut chains: Vec<String> = audible
        .iter()
        .enumerate()
        .map(|(slot, (stream, gain_db))| {
            format!("[{stream}]volume={}[m{slot}]", factor(*gain_db))
        })
        .collect();
    let marks: String = (0..audible.len()).map(|slot| format!("[m{slot}]")).collect();

    if audible.len() > 1 {
        // `normalize=0`, sonst teilt amix die Pegel durch die Anzahl der
        // Spuren und der fertige Ton wäre plötzlich viel leiser als vorher.
        chains.push(format!(
            "{marks}amix=inputs={}:normalize=0:dropout_transition=0{MIX_LABEL}",
            audible.len()
        ));
    } else {
        chains.push(format!("{marks}anull{MIX_LABEL}"));
    }
    Some(chains.join(";"))
}

/// Die Pegel aus einer gespeicherten Mischung, auf die Spurenliste bezogen.
/// Spuren ohne Eintrag bleiben unangetastet.
pub fn levels(tracks: &[ClipTrack], mix: &[TrackMix]) -> Vec<f32> {
    tracks
        .iter()
        .map(|track| {
            match mix.iter().find(|entry| entry.index == track.index) {
                Some(entry) if entry.muted => f32::NEG_INFINITY,
                Some(entry) => entry.gain_db,
                None => 0.0,
            }
        })
        .collect()
}

/// Ist `file` jünger als `source` — und damit noch gültig?
pub fn is_newer(file: &Path, source: &Path) -> bool {
    let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    match (modified(file), modified(source)) {
        (Some(a), Some(b)) => a >= b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Der Index ist das Signal „hier ist alles vollständig". Zählte er auch
    /// ohne seine Dateien, böte der Editor tote Spuren an und das Speichern
    /// bräche mittendrin ab.
    #[test]
    fn an_index_without_its_files_does_not_count() {
        let id = "clippiboy-test-unvollstaendig";
        remove(id);

        write_index(id, &["Mix".into(), "Mikrofon".into()]).unwrap();
        assert!(from_index(id).is_none(), "Index ohne Spuren darf nicht zählen");

        for index in 0..2 {
            std::fs::write(track_path(id, index), b"nicht leer").unwrap();
        }
        let tracks = from_index(id).expect("mit Dateien muss der Index zählen");
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[1].label, "Mikrofon");
        assert!(tracks[0].preview_path.is_some());

        remove(id);
        assert!(from_index(id).is_none(), "remove muss alles wegräumen");
    }

    #[test]
    fn track_labels_come_from_any_of_the_tags() {
        let named = serde_json::json!({ "tags": { "name": "Mikrofon" } });
        assert_eq!(label_of(&named, 1), "Mikrofon");

        // ffmpeg schreibt in jede MP4-Datei „SoundHandler" — das ist kein Name.
        let generic = serde_json::json!({ "tags": { "handler_name": "SoundHandler" } });
        assert_eq!(label_of(&generic, 0), "Hauptmix");
        assert_eq!(label_of(&generic, 2), "Spur 3");
    }

    #[test]
    fn gain_becomes_a_linear_factor() {
        assert_eq!(factor(0.0), 1.0);
        assert!((factor(-6.0) - 0.501).abs() < 0.01);
    }

    #[test]
    fn a_single_input_needs_no_amix() {
        let filter = mix_filter(&[("1:a".into(), 0.0)]).unwrap();
        assert!(filter.contains("anull"), "{filter}");
        assert!(filter.ends_with(MIX_LABEL));
    }

    #[test]
    fn several_inputs_are_summed_without_normalising() {
        let filter =
            mix_filter(&[("1:a".into(), 0.0), ("2:a".into(), -6.0)]).unwrap();
        assert!(filter.contains("amix=inputs=2"), "{filter}");
        // Ohne `normalize=0` halbiert amix die Pegel und der Clip wäre leiser
        // als das, was man beim Spielen gehört hat.
        assert!(filter.contains("normalize=0"), "{filter}");
    }

    /// Eine stumme Spur darf ffmpeg gar nicht erst erreichen — sonst mischt
    /// `amix` sie mit Pegel 0 mit und verlängert nur die Kette.
    #[test]
    fn muted_inputs_drop_out() {
        let filter = mix_filter(&[
            ("1:a".into(), 0.0),
            ("2:a".into(), f32::NEG_INFINITY),
        ])
        .unwrap();
        assert!(filter.contains("anull"), "{filter}");
        assert!(!filter.contains("2:a"), "{filter}");
    }

    #[test]
    fn nothing_audible_means_no_track() {
        assert!(mix_filter(&[("1:a".into(), f32::NEG_INFINITY)]).is_none());
        assert!(mix_filter(&[]).is_none());
    }

    #[test]
    fn a_muted_entry_becomes_silence() {
        let tracks = vec![
            ClipTrack { index: 0, label: "Mix".into(), channels: 2, preview_path: None },
            ClipTrack { index: 1, label: "Mic".into(), channels: 2, preview_path: None },
        ];
        let mix = vec![
            TrackMix { index: 0, gain_db: -3.0, muted: false },
            TrackMix { index: 1, gain_db: 0.0, muted: true },
        ];
        assert_eq!(levels(&tracks, &mix), vec![-3.0, f32::NEG_INFINITY]);
    }

    /// Spuren, zu denen nichts gespeichert ist, bleiben auf Anschlag — sonst
    /// verschwände eine neu dazugekommene Spur stillschweigend.
    #[test]
    fn unknown_tracks_stay_at_unity() {
        let tracks = vec![ClipTrack {
            index: 7,
            label: "Neu".into(),
            channels: 2,
            preview_path: None,
        }];
        assert_eq!(levels(&tracks, &[]), vec![0.0]);
    }
}
