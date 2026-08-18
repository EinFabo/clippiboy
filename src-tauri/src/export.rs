//! Nachbearbeitung fertiger Clips: Tonspuren auflisten, sie für die Vorschau
//! einzeln entpacken und den Clip mit eigener Mischung neu ausgeben.
//!
//! Beim Aufnehmen bekommt jede Quelle, die auf eine eigene Spur soll, auch eine
//! eigene Spur im MP4 — der Hauptmix enthält sie also *nicht*. Wer die Datei
//! irgendwo hochlädt, hört deshalb nur den Hauptmix. Genau das räumt der Export
//! auf: Er rechnet die gewählten Spuren mit ihren Reglern zu einer einzigen
//! Tonspur zusammen.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::config;
use crate::model::{Clip, ClipTrack, EncoderId, ExportRequest, ExportResult};
use crate::muxer::{command, ffmpeg, probe_duration_ms, run, sanitize};

/// Ordner für die entpackten Vorschauspuren. Liegt im Datenverzeichnis, weil
/// der Clip-Ordner dem Nutzer gehört und keine Hilfsdateien vertragen soll.
pub fn preview_dir() -> PathBuf {
    config::data_dir().join("preview")
}

/// Beim Start aufräumen: Die Dateien gehören zu Clips, die es womöglich nicht
/// mehr gibt, und lassen sich jederzeit in einer Sekunde neu erzeugen.
pub fn clear_previews() {
    let _ = std::fs::remove_dir_all(preview_dir());
    // Gleich wieder anlegen: der Player bekommt den Ordner beim Start
    // freigegeben, und ein fehlender Ordner ist unnötig zu erklären.
    let _ = std::fs::create_dir_all(preview_dir());
}

/// Die Tonspuren einer Clipdatei, in der Reihenfolge der Datei.
pub fn tracks(clip: &Clip) -> Result<Vec<ClipTrack>, String> {
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
        .arg(&clip.path)
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
        .map(|(index, stream)| {
            ClipTrack {
                index: index as u32,
                label: label_of(stream, index),
                channels: stream
                    .get("channels")
                    .and_then(|c| c.as_u64())
                    .unwrap_or(2) as u32,
                preview_path: None,
            }
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
            !value.is_empty()
                && !GENERIC_LABELS.contains(&value.to_ascii_lowercase().as_str())
        })
        .map(String::from)
        .unwrap_or_else(|| match index {
            0 => "Hauptmix".to_string(),
            other => format!("Spur {}", other + 1),
        })
}

/// Eine einzelne Tonspur als eigene Datei ablegen, damit die Vorschau im
/// Player sie abspielen kann — WebView2 gibt von einem MP4 immer nur die erste
/// Tonspur wieder, an die übrigen kommt man sonst nicht heran.
pub fn extract_track(clip: &Clip, index: u32) -> Result<PathBuf, String> {
    let dir = preview_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let out = dir.join(format!("{}_{index}.m4a", sanitize(&clip.id)));
    if is_newer(&out, Path::new(&clip.path)) {
        return Ok(out);
    }

    let mut copy = ffmpeg();
    copy.args(["-y", "-hide_banner", "-loglevel", "error"])
        .arg("-i")
        .arg(&clip.path)
        .args(["-map", &format!("0:a:{index}"), "-c:a", "copy"])
        .args(["-movflags", "+faststart"])
        .arg(&out);
    if run(&mut copy, "Tonspur entpacken").is_ok() {
        return Ok(out);
    }

    // Liegt die Spur in einem Format, das ein MP4 nicht aufnimmt (etwa PCM),
    // hilft nur neu encodieren.
    let mut encode = ffmpeg();
    encode
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .arg("-i")
        .arg(&clip.path)
        .args(["-map", &format!("0:a:{index}"), "-c:a", "aac", "-b:a", "192k"])
        .args(["-movflags", "+faststart"])
        .arg(&out);
    run(&mut encode, "Tonspur entpacken")?;
    Ok(out)
}

/// Ist `file` jünger als `source` — und damit noch gültig?
fn is_newer(file: &Path, source: &Path) -> bool {
    let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    match (modified(file), modified(source)) {
        (Some(a), Some(b)) => a >= b,
        _ => false,
    }
}

/// dB in linearen Faktor, gerundet auf drei Stellen für die ffmpeg-Zeile.
fn factor(db: f32) -> f32 {
    (crate::audio::gain_factor(db) * 1000.0).round() / 1000.0
}

/// Den Clip mit der gewählten Mischung und dem gewählten Ausschnitt neu
/// schreiben. `on_progress` bekommt Werte zwischen 0 und 1.
pub fn export(
    clip: &Clip,
    request: &ExportRequest,
    encoder: EncoderId,
    bitrate_kbps: u32,
    on_progress: impl Fn(f32),
) -> Result<ExportResult, String> {
    if !Path::new(&clip.path).is_file() {
        return Err("Die Clipdatei ist nicht mehr da.".into());
    }

    let start_ms = request.start_ms;
    let end_ms = request.end_ms.max(start_ms + 200).min(clip.duration_ms.max(1));
    let length_ms = end_ms.saturating_sub(start_ms).max(200);
    let output = target_path(clip, request);
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Nicht über die eigene Vorlage bügeln — das wäre unwiederbringlich.
    if output == Path::new(&clip.path) {
        return Err("Der Export darf den Ausgangsclip nicht überschreiben.".into());
    }

    // Nur schneiden, wenn wirklich geschnitten wird: ohne Zuschnitt bleibt das
    // Bild unangetastet und der Export dauert Sekunden statt Minuten.
    let trims_start = start_ms > 250;
    let trims_end = end_ms + 250 < clip.duration_ms;

    let audible: Vec<_> = request
        .tracks
        .iter()
        .filter(|track| !track.muted && track.gain_db > -59.0)
        .collect();

    let mut args: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
    ];
    if trims_start {
        args.push("-ss".into());
        args.push(format!("{:.3}", start_ms as f64 / 1000.0));
    }
    args.push("-i".into());
    args.push(clip.path.clone());
    if trims_end {
        args.push("-t".into());
        args.push(format!("{:.3}", length_ms as f64 / 1000.0));
    }

    // Bild zuerst abbilden, damit es auch im Ergebnis Spur 0 ist.
    args.push("-map".into());
    args.push("0:v:0".into());

    if audible.is_empty() {
        args.push("-an".into());
    } else {
        let mut chains: Vec<String> = audible
            .iter()
            .map(|track| {
                format!(
                    "[0:a:{}]volume={}[a{}]",
                    track.index,
                    factor(track.gain_db),
                    track.index
                )
            })
            .collect();
        let inputs: String = audible
            .iter()
            .map(|track| format!("[a{}]", track.index))
            .collect();
        if audible.len() > 1 {
            // `normalize=0`, sonst teilt amix die Pegel durch die Anzahl der
            // Spuren und der fertige Ton wäre plötzlich viel leiser als vorher.
            chains.push(format!(
                "{inputs}amix=inputs={}:normalize=0:dropout_transition=0[aout]",
                audible.len()
            ));
        } else {
            chains.push(format!("{inputs}anull[aout]"));
        }
        args.push("-filter_complex".into());
        args.push(chains.join(";"));
        args.push("-map".into());
        args.push("[aout]".into());
        args.push("-c:a".into());
        args.push("aac".into());
        args.push("-b:a".into());
        args.push("256k".into());
        args.push("-ac".into());
        args.push("2".into());
        args.push("-metadata:s:a:0".into());
        args.push("title=Mix".into());
    }

    args.push("-avoid_negative_ts".into());
    args.push("make_zero".into());
    args.push("-movflags".into());
    args.push("+faststart".into());

    let with_video = |video: Vec<String>| {
        let mut all = args.clone();
        all.extend(video);
        all.push(output.to_string_lossy().to_string());
        all
    };

    let attempt = if trims_start {
        // Ein Schnitt mitten im Bild geht nur beim Neuencodieren bildgenau —
        // beim Kopieren müsste er auf das nächste Keyframe rutschen.
        with_video(video_args(encoder, bitrate_kbps))
    } else {
        with_video(vec!["-c:v".into(), "copy".into()])
    };

    match run_with_progress(&attempt, length_ms, &on_progress) {
        Ok(()) => {}
        Err(err) if trims_start && encoder != EncoderId::X264 => {
            // Hardware-Encoder streiken gern mal (Treiber, belegte Sitzung).
            log::warn!("Export mit {encoder:?} fehlgeschlagen ({err}) — jetzt mit x264");
            let fallback = with_video(video_args(EncoderId::X264, bitrate_kbps));
            run_with_progress(&fallback, length_ms, &on_progress)?;
        }
        Err(err) => return Err(err),
    }

    let size_bytes = std::fs::metadata(&output)
        .map(|meta| meta.len())
        .unwrap_or(0);
    if size_bytes == 0 {
        return Err("Der Export ist leer geblieben.".into());
    }
    on_progress(1.0);

    Ok(ExportResult {
        duration_ms: probe_duration_ms(&output).unwrap_or(length_ms),
        path: output.to_string_lossy().to_string(),
        size_bytes,
    })
}

/// Zielpfad: entweder der gewählte, sonst `<name>_export.mp4` neben dem Clip.
fn target_path(clip: &Clip, request: &ExportRequest) -> PathBuf {
    if let Some(output) = request.output.as_ref().filter(|o| !o.trim().is_empty()) {
        return PathBuf::from(output);
    }
    let source = Path::new(&clip.path);
    let stem = source.file_stem().unwrap_or_default().to_string_lossy();
    source.with_file_name(format!("{stem}_export.mp4"))
}

fn video_args(encoder: EncoderId, bitrate_kbps: u32) -> Vec<String> {
    let rate = bitrate_kbps.max(4_000);
    let bitrate = format!("{rate}k");
    let maxrate = format!("{}k", rate * 3 / 2);
    let common = |mut args: Vec<&str>| -> Vec<String> {
        args.extend(["-pix_fmt", "yuv420p"]);
        args.into_iter().map(String::from).collect()
    };
    match encoder {
        EncoderId::Nvenc => {
            let mut args = common(vec!["-c:v", "h264_nvenc", "-preset", "p5", "-rc", "vbr"]);
            args.extend(["-b:v".into(), bitrate, "-maxrate".into(), maxrate]);
            args
        }
        EncoderId::Amf => {
            let mut args = common(vec!["-c:v", "h264_amf", "-quality", "balanced"]);
            args.extend(["-b:v".into(), bitrate]);
            args
        }
        EncoderId::Qsv => {
            let mut args = common(vec!["-c:v", "h264_qsv", "-preset", "medium"]);
            args.extend(["-b:v".into(), bitrate]);
            args
        }
        EncoderId::X264 => common(vec!["-c:v", "libx264", "-preset", "veryfast", "-crf", "20"]),
    }
}

/// ffmpeg starten und dabei `-progress` mitlesen. Ohne das stünde die App bei
/// einem neu encodierten Clip minutenlang ohne Lebenszeichen da.
fn run_with_progress(
    args: &[String],
    length_ms: u64,
    on_progress: &impl Fn(f32),
) -> Result<(), String> {
    let mut child = ffmpeg()
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("ffmpeg konnte nicht gestartet werden: {err}"))?;

    // stderr in einem eigenen Faden leeren: bliebe die Röhre stehen, könnte
    // ffmpeg beim Schreiben hängen bleiben.
    let stderr = child.stderr.take();
    let collector = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(stderr) = stderr {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                text.push_str(&line);
                text.push('\n');
            }
        }
        text
    });

    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let Some(time) = line.strip_prefix("out_time=") else {
                continue;
            };
            if let Some(ms) = parse_timestamp_ms(time) {
                on_progress((ms as f32 / length_ms.max(1) as f32).clamp(0.0, 0.99));
            }
        }
    }

    let status = child.wait().map_err(|err| err.to_string())?;
    let stderr = collector.join().unwrap_or_default();
    if status.success() {
        return Ok(());
    }
    let tail: Vec<&str> = stderr.lines().rev().take(4).collect();
    Err(format!(
        "ffmpeg ist beim Export fehlgeschlagen: {}",
        tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
    ))
}

/// `00:01:02.500000` in Millisekunden.
fn parse_timestamp_ms(text: &str) -> Option<u64> {
    let mut parts = text.trim().split(':');
    let hours: f64 = parts.next()?.parse().ok()?;
    let minutes: f64 = parts.next()?.parse().ok()?;
    let seconds: f64 = parts.next()?.parse().ok()?;
    Some(((hours * 3600.0 + minutes * 60.0 + seconds) * 1000.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TrackMix;

    fn clip() -> Clip {
        Clip {
            id: "abc".into(),
            path: "C:/clips/run.mp4".into(),
            created_at: 0,
            duration_ms: 30_000,
            game: None,
            width: 1920,
            height: 1080,
            size_bytes: 1,
            thumb_path: None,
            title: None,
            description: None,
        }
    }

    fn request() -> ExportRequest {
        ExportRequest {
            clip_id: "abc".into(),
            start_ms: 0,
            end_ms: 30_000,
            tracks: vec![TrackMix {
                index: 0,
                gain_db: 0.0,
                muted: false,
            }],
            output: None,
        }
    }

    #[test]
    fn export_lands_next_to_the_clip() {
        let path = target_path(&clip(), &request());
        assert_eq!(path.file_name().unwrap(), "run_export.mp4");
    }

    #[test]
    fn chosen_target_wins() {
        let mut request = request();
        request.output = Some("D:/upload/ace.mp4".into());
        assert_eq!(target_path(&clip(), &request), PathBuf::from("D:/upload/ace.mp4"));
    }

    #[test]
    fn timestamps_become_milliseconds() {
        assert_eq!(parse_timestamp_ms("00:00:02.500000"), Some(2500));
        assert_eq!(parse_timestamp_ms("00:01:00.000000"), Some(60_000));
        assert_eq!(parse_timestamp_ms("N/A"), None);
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
}
