//! Aus den TS-Segmenten des Ringpuffers ein fertiges MP4 bauen.
//!
//! Das Video wird nur kopiert (`-c copy`), nicht neu encodiert — deshalb ist
//! das Speichern eines Clips eine Sache von Sekundenbruchteilen und kostet
//! keine Qualität. Zusätzliche Tonspuren werden dabei mit eingemuxt.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use crate::pipeline::{Segment, TrackRing};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Ordner, in dem die mitgelieferten Programme liegen — im Installationspaket
/// der Ressourcenordner der App. Wird beim Start einmal gesetzt.
static TOOL_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Wo ffmpeg und ffprobe zu finden sind. Ohne Aufruf sucht sie nur der PATH ab.
pub fn set_tool_dir(dir: PathBuf) {
    let _ = TOOL_DIR.set(dir);
}

/// Mitgeliefertes Programm, sonst der Name für die PATH-Suche.
///
/// Mitgeliefert schlägt PATH bewusst: eine zufällig installierte ffmpeg-Version
/// kann andere Vorgaben haben, und getestet ist die beigelegte.
fn tool(name: &str) -> PathBuf {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    match TOOL_DIR.get().map(|dir| dir.join(&file)) {
        Some(path) if path.is_file() => path,
        _ => PathBuf::from(name),
    }
}

pub fn command(name: &str) -> Command {
    let mut command = Command::new(tool(name));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

pub fn ffmpeg() -> Command {
    command("ffmpeg")
}

pub fn run(command: &mut Command, what: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|err| format!("ffmpeg konnte nicht gestartet werden ({what}): {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let tail: Vec<&str> = stderr.lines().rev().take(6).collect();
    Err(format!(
        "ffmpeg ist bei '{what}' fehlgeschlagen: {}",
        tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
    ))
}

/// Ist ffmpeg auffindbar?
pub fn available() -> bool {
    ffmpeg()
        .arg("-version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

pub struct ClipRequest<'a> {
    pub segments: &'a [Segment],
    pub tracks: &'a [Arc<TrackRing>],
    /// Zeitpunkt „jetzt" in Millisekunden seit Aufnahmestart.
    pub now_ms: u64,
    pub seconds: u32,
    pub output: PathBuf,
    pub temp_dir: PathBuf,
}

pub struct ClipResult {
    pub path: PathBuf,
    pub duration_ms: u64,
    pub size_bytes: u64,
    pub thumb_path: Option<PathBuf>,
}

pub fn build(request: ClipRequest<'_>) -> Result<ClipResult, String> {
    let window_start = request.now_ms.saturating_sub(request.seconds as u64 * 1000);

    let mut selected: Vec<&Segment> = request
        .segments
        .iter()
        .filter(|segment| segment.end_ms > window_start)
        .collect();
    if selected.is_empty() {
        return Err("Der Replay-Puffer ist noch leer.".into());
    }
    selected.sort_by_key(|segment| segment.index);

    std::fs::create_dir_all(&request.temp_dir).map_err(|e| e.to_string())?;

    // Concat-Demuxer statt `concat:`-Protokoll: Jedes Segment kommt aus einem
    // frisch gestarteten Encoder, dessen Uhr wieder bei null beginnt. Das
    // Protokoll klebt die TS-Dateien nur aneinander, ffmpeg sähe an jeder
    // Segmentgrenze einen Zeitstempel-Rücksprung und würde den Rest verwerfen
    // oder mit kaputtem Timing schreiben. Der Demuxer schiebt die Zeitstempel
    // jeder Datei hinter die vorherige.
    // Aus dem Ziel-Clip abgeleitet und damit eindeutig je Speichervorgang:
    // Zwei gleichzeitige Speicherungen dürfen sich weder die Segmentliste noch
    // die Tonspuren unter den Händen wegschreiben.
    let stem = sanitize(&request.output.file_stem().unwrap_or_default().to_string_lossy());
    let list_path = request.temp_dir.join(format!("concat_{stem}.txt"));
    let list = selected
        .iter()
        .map(|segment| format!("file '{}'\n", escape_for_list(&segment.path)))
        .collect::<String>();
    std::fs::write(&list_path, list).map_err(|e| e.to_string())?;
    if let Some(parent) = request.output.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // Zusätzliche Tonspuren als WAV bereitstellen.
    let mut wavs: Vec<(PathBuf, String)> = Vec::new();
    for track in request.tracks {
        let path = request
            .temp_dir
            .join(format!("track_{stem}_{}.wav", sanitize(&track.source_id)));
        if track.write_wav(&path, request.seconds).is_ok() {
            wavs.push((path, track.label.clone()));
        }
    }

    let mut command = ffmpeg();
    command.arg("-y").arg("-hide_banner").arg("-loglevel").arg("error");
    // `+genpts` füllt die Zeitstempel auf, die beim Segmentwechsel fehlen.
    command.arg("-fflags").arg("+genpts");
    command.arg("-f").arg("concat").arg("-safe").arg("0");
    // Kein `-ss`, um vorne auf die gewünschte Länge zu kürzen. Der
    // Concat-Demuxer kann nur suchen, wenn in der Liste zu jeder Datei eine
    // `duration` steht — sonst tut `-ss` *nichts*: Nachgemessen liefern
    // `-ss 4.5` und `-ss 12` über dieselbe Liste Byte für Byte dieselbe Länge.
    // Weggeworfen wird trotzdem etwas, nämlich alle Videopakete bis zum
    // nächsten Keyframe. Der Clip begann dadurch mit bis zu einer Sekunde Ton
    // ohne Bild (gemessen: `start_time` der Videospur bei 0,98 s bei 1 s
    // Keyframe-Abstand), ohne dafür auch nur eine Sekunde kürzer zu werden.
    //
    // Ohne `-ss` fängt der Clip an einer Segmentgrenze an — also an einem
    // Keyframe, mit Bild und Ton ab dem ersten Moment. Er ist damit bis zu
    // einer Segmentlänge länger als angefordert, aber genau das war er vorher
    // auch schon. Für ein echtes Kürzen bräuchte die Liste die tatsächlichen
    // Laufzeiten der Segmentdateien; die Marken aus dem Ringpuffer taugen
    // dafür nicht, weil der Demuxer sie den Zeitstempeln vorzieht und ein paar
    // Millisekunden Abweichung je Segment die Spuren wieder verschieben würden.
    command.arg("-i").arg(&list_path);
    for (path, _) in &wavs {
        command.arg("-i").arg(path);
    }

    command.arg("-map").arg("0:v:0").arg("-map").arg("0:a:0?");
    for index in 1..=wavs.len() {
        command.arg("-map").arg(format!("{index}:a:0"));
    }
    command.arg("-c:v").arg("copy");
    if wavs.is_empty() {
        command.arg("-c:a").arg("copy");
    } else {
        // Die WAV-Spuren müssen encodiert werden, das Video bleibt unangetastet.
        command.arg("-c:a").arg("aac").arg("-b:a").arg("192k");
    }
    // Zweimal derselbe Name mit Absicht: MP4 kennt kein einheitliches Feld für
    // Spurnamen, und je nach Programm wird das eine oder das andere gelesen.
    command.arg("-metadata:s:a:0").arg("title=Mix");
    command.arg("-metadata:s:a:0").arg("handler_name=Mix");
    for (index, (_, label)) in wavs.iter().enumerate() {
        command
            .arg(format!("-metadata:s:a:{}", index + 1))
            .arg(format!("title={label}"));
        command
            .arg(format!("-metadata:s:a:{}", index + 1))
            .arg(format!("handler_name={label}"));
    }
    // Nach dem Suchen fängt der erste Zeitstempel nicht bei null an — ohne das
    // stünde im MP4 ein negativer Versatz.
    command.arg("-avoid_negative_ts").arg("make_zero");
    // Kein `+faststart`: Das schiebt das moov-Atom nach vorn und liest dafür
    // die fertige Datei noch einmal komplett durch — bei 40 Mbit/s und zwei
    // Minuten Puffer ein zweiter Durchlauf über gut 600 MB, also glatt die
    // doppelte Wartezeit nach dem Tastendruck. Zum Abspielen und Schneiden
    // braucht es das nicht; der Export setzt es weiterhin, und der ist der
    // Weg zur Datei, die man weitergibt.
    command.arg(&request.output);

    let outcome = run(&mut command, "Clip schreiben");
    let _ = std::fs::remove_file(&list_path);
    for (path, _) in &wavs {
        let _ = std::fs::remove_file(path);
    }
    outcome?;

    let size_bytes = std::fs::metadata(&request.output)
        .map(|meta| meta.len())
        .unwrap_or(0);
    if size_bytes == 0 {
        return Err("Der Clip ist leer geblieben.".into());
    }

    let duration_ms = probe_duration_ms(&request.output).unwrap_or(request.seconds as u64 * 1000);
    let thumb_path = make_thumbnail(&request.output).ok();

    Ok(ClipResult {
        path: request.output,
        duration_ms,
        size_bytes,
        thumb_path,
    })
}

/// Pfad für die Segmentliste des Concat-Demuxers aufbereiten. Der Parser liest
/// bis zum schließenden Apostroph, ein Apostroph im Pfad muss also aussteigen;
/// Rückwärtsschrägstriche behandelt er als Escape, deshalb Schrägstriche.
fn escape_for_list(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").replace('\'', "'\\''")
}

pub fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

pub fn probe_duration_ms(path: &Path) -> Option<u64> {
    let output = command("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let seconds: f64 = text.trim().parse().ok()?;
    Some((seconds * 1000.0) as u64)
}

fn make_thumbnail(video: &Path) -> Result<PathBuf, String> {
    let thumb = video.with_extension("jpg");
    run(
        ffmpeg()
            .args(["-y", "-hide_banner", "-loglevel", "error", "-ss", "0.5"])
            .arg("-i")
            .arg(video)
            .args(["-frames:v", "1", "-vf", "scale=480:-1", "-q:v", "4"])
            .arg(&thumb),
        "Vorschaubild",
    )?;
    Ok(thumb)
}
