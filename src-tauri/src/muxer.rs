//! Aus dem Paket-Ringpuffer ein fertiges MP4 bauen.
//!
//! Das Video wird nur kopiert (`-c copy`), nicht neu encodiert — Speichern
//! kostet damit weder Zeit noch Qualität.
//!
//! Früher lagen hier MPEG-TS-Segmentdateien, die der `concat`-Demuxer
//! zusammensetzen musste. Weil jeder Segment-Encoder seine Zeitrechnung wieder
//! bei null begann, brauchte es `+genpts`, und `-ss` war wirkungslos — der
//! Clip fing immer an einer Segmentgrenze an und war bis zu zehn Sekunden zu
//! lang. Jetzt kommt das Video als ein durchgehender Elementarstrom aus einem
//! einzigen Encoder: Der Schnitt sitzt auf dem Keyframe davor, und die
//! Zeitstempel ergeben sich aus der konstanten Bildrate.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use crate::pipeline::ClipSnapshot;
use crate::stems;

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

pub struct ClipRequest {
    pub snapshot: ClipSnapshot,
    /// Die Kennung, unter der der Clip gleich in der Datenbank landet — die
    /// Einzelspuren werden danach abgelegt.
    pub clip_id: String,
    pub output: PathBuf,
    pub temp_dir: PathBuf,
}

pub struct ClipResult {
    pub path: PathBuf,
    pub duration_ms: u64,
    pub size_bytes: u64,
    pub thumb_path: Option<PathBuf>,
}

pub fn build(request: ClipRequest) -> Result<ClipResult, String> {
    let snapshot = request.snapshot;
    if snapshot.packets.is_empty() {
        return Err("Der Replay-Puffer ist noch leer.".into());
    }

    std::fs::create_dir_all(&request.temp_dir).map_err(|e| e.to_string())?;
    if let Some(parent) = request.output.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // Aus dem Ziel-Clip abgeleitet und damit eindeutig je Speichervorgang: Zwei
    // gleichzeitige Speicherungen dürfen sich die Zwischendateien nicht unter
    // den Händen wegschreiben.
    let stem = sanitize(&request.output.file_stem().unwrap_or_default().to_string_lossy());

    // Videopakete als roher H.264-Elementarstrom.
    let video_path = request.temp_dir.join(format!("clip_{stem}.h264"));
    let mut stream: Vec<u8> = Vec::with_capacity(
        snapshot.packets.iter().map(|p| p.data.len()).sum::<usize>()
            + snapshot.sequence_header.len(),
    );
    // SPS/PPS voranstellen. Die meisten Encoder schicken sie ohnehin vor jedem
    // IDR mit — doppelt überliest jeder Decoder, ganz fehlen dürfen sie nicht.
    stream.extend_from_slice(&snapshot.sequence_header);
    for packet in &snapshot.packets {
        stream.extend_from_slice(&packet.data);
    }
    std::fs::write(&video_path, &stream).map_err(|e| e.to_string())?;

    // Tonspuren als WAV, geschnitten auf denselben QPC wie das erste Bild.
    let mut wavs: Vec<(PathBuf, String)> = Vec::new();
    for track in &snapshot.tracks {
        let path = request
            .temp_dir
            .join(format!("track_{stem}_{}.wav", sanitize(&track.source_id)));
        // Eine Spur stillschweigend wegzulassen wäre das Schlimmste: Der Clip
        // wäre dann einfach stumm, ohne dass irgendwo stünde warum.
        match track.write_wav_window(&path, snapshot.start_100ns, snapshot.audio_frames) {
            Ok(()) => wavs.push((path, track.label())),
            Err(err) => log::warn!(
                "Tonspur '{}' konnte nicht geschrieben werden: {err}",
                track.label()
            ),
        }
    }

    // Der Clip bekommt **eine** Tonspur, in der alles steckt. Discord, Browser
    // und die meisten Player spielen von einem MP4 stur die erste Tonspur ab —
    // lagen die Quellen wie früher auf eigenen Spuren daneben, waren sie
    // überall außerhalb des Editors stumm.
    //
    // Die Pegel aus dem Aufnahme-Mixer stecken bereits in den WAVs
    // (`AudioEngine::mix_window`), hier wird also bei 0 dB summiert.
    let inputs: Vec<(String, f32)> = (1..=wavs.len())
        .map(|index| (format!("{index}:a"), 0.0))
        .collect();
    let filter = stems::mix_filter(&inputs);

    let mut command = ffmpeg();
    command.arg("-y").arg("-hide_banner").arg("-loglevel").arg("error");
    // Der Elementarstrom trägt keine Zeitstempel — die Bildrate liefert sie.
    // Sie stimmt exakt, weil der Taktgeber echtes CFR erzeugt.
    command.arg("-f").arg("h264").arg("-r").arg(snapshot.fps.to_string());
    command.arg("-i").arg(&video_path);
    for (path, _) in &wavs {
        command.arg("-i").arg(path);
    }
    if let Some(filter) = &filter {
        command.arg("-filter_complex").arg(filter);
    }

    // Erste Ausgabe: der Clip selbst.
    command.arg("-map").arg("0:v:0");
    match &filter {
        Some(_) => {
            command.arg("-map").arg(stems::MIX_LABEL);
            command.arg("-c:a").arg("aac").arg("-b:a").arg("192k");
            // Zweimal derselbe Name mit Absicht: MP4 kennt kein einheitliches
            // Feld für Spurnamen, und je nach Programm wird das eine oder das
            // andere gelesen.
            command.arg("-metadata:s:a:0").arg("title=Mix");
            command.arg("-metadata:s:a:0").arg("handler_name=Mix");
        }
        None => {
            command.arg("-an");
        }
    }
    command.arg("-c:v").arg("copy");
    // Kein `-shortest`: Der Videostrom wird nur kopiert und ist deshalb in
    // Sekundenbruchteilen durch. ffmpeg hält den Output dann für fertig und
    // beendet ihn, bevor der AAC-Encoder sein erstes Paket geliefert hat — die
    // Datei kam nachweislich ganz ohne Tonspur heraus. Gebraucht wird es auch
    // nicht: `write_wav_window` schneidet den Ton bereits auf die Länge des
    // Bildes zu.
    command.arg("-avoid_negative_ts").arg("make_zero");
    // Kein `+faststart`: Das schiebt das moov-Atom nach vorn und liest dafür
    // die fertige Datei noch einmal komplett durch — bei 40 Mbit/s und zwei
    // Minuten Puffer glatt die doppelte Wartezeit nach dem Tastendruck. Zum
    // Abspielen und Schneiden braucht es das nicht.
    command.arg(&request.output);

    // Weitere Ausgaben: die Einzelspuren, damit sich die Mischung später noch
    // ändern lässt. Bei nur einer Spur wäre das eine Kopie der Tonspur des
    // Clips — die tut es dann auch.
    let keep_stems = wavs.len() > 1;
    let stems_dir = stems::dir(&request.clip_id);
    if keep_stems {
        std::fs::create_dir_all(&stems_dir).map_err(|e| e.to_string())?;
        for index in 1..=wavs.len() {
            command.arg("-map").arg(format!("{index}:a"));
            command.arg("-c:a").arg("aac").arg("-b:a").arg("192k");
            command.arg("-movflags").arg("+faststart");
            command.arg(stems::track_path(&request.clip_id, index as u32 - 1));
        }
    }

    let outcome = run(&mut command, "Clip schreiben");
    let _ = std::fs::remove_file(&video_path);
    for (path, _) in &wavs {
        let _ = std::fs::remove_file(path);
    }
    if let Err(err) = outcome {
        // Halb geschriebene Einzelspuren wären schlimmer als gar keine: Der
        // Editor hielte sie für vollständig und mischte daraus.
        if keep_stems {
            stems::remove(&request.clip_id);
        }
        return Err(err);
    }

    if keep_stems {
        let labels: Vec<String> = wavs.iter().map(|(_, label)| label.clone()).collect();
        if let Err(err) = stems::write_index(&request.clip_id, &labels) {
            log::warn!("Spurenverzeichnis nicht geschrieben: {err}");
            stems::remove(&request.clip_id);
        }
    }

    let size_bytes = std::fs::metadata(&request.output)
        .map(|meta| meta.len())
        .unwrap_or(0);
    if size_bytes == 0 {
        return Err("Der Clip ist leer geblieben.".into());
    }

    let duration_ms = probe_duration_ms(&request.output)
        .unwrap_or(snapshot.audio_frames as u64 * 1000 / 48_000);
    let thumb_path = crate::thumbs::make(&request.output, &request.clip_id).ok();

    Ok(ClipResult {
        path: request.output,
        duration_ms,
        size_bytes,
        thumb_path,
    })
}

/// Eine frisch geschriebene Datei an die Stelle einer bestehenden schieben.
///
/// Windows lässt eine geöffnete Datei nicht ersetzen; der Player gibt sie
/// vorher frei, aber das Handle verschwindet nicht immer im selben Augenblick
/// — deshalb ein paar Anläufe, bevor aufgegeben wird.
pub fn replace_file(temp: &Path, target: &Path) -> Result<(), String> {
    let mut last = None;
    for attempt in 0..10 {
        match std::fs::rename(temp, target) {
            Ok(()) => return Ok(()),
            Err(err) => {
                last = Some(err);
                std::thread::sleep(std::time::Duration::from_millis(50 * (attempt + 1)));
            }
        }
    }
    Err(format!(
        "Der Clip ließ sich nicht ersetzen ({}). Ist er gerade woanders geöffnet?",
        last.map(|err| err.to_string()).unwrap_or_default()
    ))
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
