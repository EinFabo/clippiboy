//! Den Clip so schreiben, wie er im Editor steht — Mischung **und** Zuschnitt.
//!
//! Der Zuschnitt war früher nur eine Markierung in der Datenbank: Wer den Clip
//! verschickte, verschickte den ungeschnittenen. Jetzt steckt er in der Datei.
//! Damit trotzdem nichts verlorengeht, wandert die unversehrte Aufnahme beim
//! ersten echten Schnitt in eine Ablage neben den Einzelspuren:
//!
//! ```text
//! <data>/originals/<clip-id>/video.mp4     die unversehrte Aufnahme
//! <data>/originals/<clip-id>/schnitt.json  wo der Ausschnitt in ihr sitzt
//! ```
//!
//! Der Beipackzettel wird **vor** dem Video geschrieben und **nach** ihm
//! gelöscht. Er ist damit das Signal „dieses Original lebt", und [`repair`]
//! kann jeden Zustand aufräumen, den ein Absturz mitten im Tausch hinterlässt.
//!
//! # Drei Invarianten
//!
//! 1. **Die Einzelspuren bleiben ungeschnitten** und stehen immer in
//!    Koordinaten des Originals. Sie werden nie ersetzt — kein Handle-Problem
//!    unter Windows, kein zweiter Zeitstrahl, der davonlaufen kann.
//! 2. **Ton, der noch in der Datei steckt** (Altbestand, oder eine Aufnahme mit
//!    nur einer Quelle) steht in Koordinaten der Datei, die gerade Eingabe 0
//!    ist, und erbt deshalb den *Bild*versatz, nicht den Spurenversatz.
//! 3. **Neu encodiert wird immer aus dem Original**, nie aus der schon
//!    geschnittenen Datei. Der Qualitätsverlust bleibt damit bei genau einer
//!    Generation, egal wie oft man nachschneidet.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use parking_lot::Mutex;

use crate::config;
use crate::model::{Clip, ClipOriginal, EncoderId, TrackMix};
use crate::muxer::{ffmpeg, make_thumbnail, probe_duration_ms, replace_file, sanitize};
use crate::stems;

/// Ab hier gilt eine Grenze als bewusst gesetzt. Darunter ist sie das Ergebnis
/// eines Griffs, der nicht ganz am Anschlag saß — dafür soll niemand auf einen
/// Neuencodierlauf warten.
const EDGE_TOLERANCE_MS: u64 = 250;

/// Kürzer ergibt kein Video mehr.
const MIN_LENGTH_MS: u64 = 200;

/// Nur ein Rendern und Tauschen auf einmal.
///
/// Zwei schnelle Klicks auf Speichern kämpften sonst um dieselbe Zwischendatei
/// — und schlimmer: der zweite Lauf läse die Clipdatei, während der erste sie
/// gerade ersetzt.
static WORKING: Mutex<()> = Mutex::new(());

/// Wurzel aller Originale. **Dauerhaft**, wie die Einzelspuren.
pub fn root() -> PathBuf {
    config::data_dir().join("originals")
}

/// Ordner eines Clips.
pub fn dir(clip_id: &str) -> PathBuf {
    root().join(sanitize(clip_id))
}

/// Die unversehrte Aufnahme.
pub fn video_path(clip_id: &str) -> PathBuf {
    dir(clip_id).join("video.mp4")
}

/// Der Beipackzettel daneben.
fn note_path(clip_id: &str) -> PathBuf {
    dir(clip_id).join("schnitt.json")
}

/// Woraus gelesen wird: das Original, sonst die Clipdatei.
///
/// Alles, was die volle Aufnahme braucht — Einzelspuren entpacken, neu
/// encodieren, den Zuschnitt aufheben — geht über diesen Pfad.
pub fn source_path(clip: &Clip) -> PathBuf {
    let original = video_path(&clip.id);
    if original.is_file() {
        original
    } else {
        PathBuf::from(&clip.path)
    }
}

/// Liegt zu diesem Clip ein vollständiges Original? Nur mit Zettel *und* Video.
pub fn has_original(clip_id: &str) -> bool {
    note_path(clip_id).is_file() && video_path(clip_id).is_file()
}

fn read_note(clip_id: &str) -> Option<ClipOriginal> {
    let text = std::fs::read_to_string(note_path(clip_id)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_note(clip_id: &str, original: &ClipOriginal) -> Result<(), String> {
    std::fs::create_dir_all(dir(clip_id)).map_err(|err| err.to_string())?;
    let text = serde_json::to_string_pretty(original).map_err(|err| err.to_string())?;
    std::fs::write(note_path(clip_id), text).map_err(|err| err.to_string())
}

/// Alles zu einem Clip wegräumen. Erst der Zettel, dann das Video — solange der
/// Zettel liegt, gilt das Original als lebendig.
pub fn remove(clip_id: &str) {
    let _ = std::fs::remove_file(note_path(clip_id));
    let _ = std::fs::remove_file(video_path(clip_id));
    let _ = std::fs::remove_dir_all(dir(clip_id));
}

/// Der Ausschnitt, den der Editor gerade zeigt — in Sekunden der **aktuellen**
/// Datei, umgerechnet in Millisekunden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trim {
    pub start_ms: u64,
    pub end_ms: u64,
}

impl Trim {
    /// Der ganze Bereich, also „nichts wegschneiden".
    pub fn whole(duration_ms: u64) -> Self {
        Self {
            start_ms: 0,
            end_ms: duration_ms,
        }
    }
}

/// Woher das Bild kommt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoSource {
    /// Die ausgelieferte Datei — sie ist schon so weit geschnitten, wie sie
    /// sein soll, und wird nur kopiert.
    Clip,
    /// Die unversehrte Aufnahme. Nur von hier wird neu encodiert.
    Original,
}

/// Was der eine ffmpeg-Lauf tun soll. Reine Daten, damit sich die Entscheidung
/// ohne ffmpeg prüfen lässt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderPlan {
    pub video: VideoSource,
    /// Versatz auf der Videoquelle.
    pub video_start_ms: u64,
    /// Versatz auf den Einzelspuren — die stehen immer im Original.
    pub stems_start_ms: u64,
    pub length_ms: u64,
    /// Muss das Bild neu gerechnet werden? Nur wenn vorne geschnitten wird.
    pub reencode: bool,
    /// Wie der Clip danach in der Datenbank steht. `None` heißt: ganze
    /// Aufnahme, kein Original nötig.
    pub original: Option<ClipOriginal>,
}

/// Den Auftrag aus dem Editor-Stand ableiten.
///
/// `base` ist der Schnitt, der schon in der Datei steckt; `trim` der Ausschnitt,
/// den der Nutzer auf der Zeitachse **dieser** Datei markiert hat. Der neue
/// Schnitt in Originalkoordinaten ist also `base.start + trim.start ..
/// base.start + trim.end` — dadurch lässt sich beliebig weiter hineinschneiden.
/// Aufziehen geht nur über [`restore_plan`].
///
/// `base` kommt aus der Datenbank und nicht von der Platte: Es trägt den
/// Versatz, in dem die Einzelspuren stehen, und muss auch dann noch stimmen,
/// wenn jemand die Ablage von Hand ausgeräumt hat. `has_original` sagt, ob dazu
/// auch noch eine Datei liegt — nur dann kann aus ihr encodiert werden.
pub fn plan(
    base: Option<&ClipOriginal>,
    has_original: bool,
    clip_duration_ms: u64,
    trim: Trim,
) -> RenderPlan {
    let (offset, full) = match base {
        Some(original) => (original.start_ms, original.duration_ms),
        None => (0, clip_duration_ms),
    };

    let start = trim.start_ms.min(clip_duration_ms);
    let end = trim.end_ms.clamp(start + MIN_LENGTH_MS, clip_duration_ms.max(start + MIN_LENGTH_MS));

    let cuts_front = start > EDGE_TOLERANCE_MS;
    let cuts_back = end + EDGE_TOLERANCE_MS < clip_duration_ms;

    let absolute_start = offset + if cuts_front { start } else { 0 };
    let absolute_end = offset + if cuts_back { end } else { clip_duration_ms };
    let length_ms = absolute_end.saturating_sub(absolute_start).max(MIN_LENGTH_MS);

    // Bleibt vorne alles stehen, ist die ausgelieferte Datei schon der richtige
    // Anfang — dann wird aus ihr kopiert und das Bild nicht angefasst. Das ist
    // der häufige Fall: nur die Mischung ändern, oder nur hinten kürzen.
    //
    // Wird vorne geschnitten, kommt das Bild aus dem Original: So bleibt der
    // Verlust bei einer Generation, auch wenn jemand dreimal nachschneidet.
    // Ist die Ablage weg, muss die geschnittene Datei herhalten — dann ist der
    // Versatz auf ihr relativ, nicht absolut.
    let (video, video_start_ms) = match (cuts_front, has_original) {
        (false, _) => (VideoSource::Clip, 0),
        (true, true) => (VideoSource::Original, absolute_start),
        (true, false) => (VideoSource::Clip, start),
    };

    let trimmed = absolute_start > 0 || absolute_end < full;
    RenderPlan {
        video,
        video_start_ms,
        stems_start_ms: absolute_start,
        length_ms,
        reencode: cuts_front,
        original: trimmed.then_some(ClipOriginal {
            duration_ms: full,
            start_ms: absolute_start,
            end_ms: absolute_end,
        }),
    }
}

/// Der Auftrag fürs Aufheben: die ganze Aufnahme, verlustfrei kopiert.
pub fn restore_plan(base: &ClipOriginal) -> RenderPlan {
    RenderPlan {
        video: VideoSource::Original,
        video_start_ms: 0,
        stems_start_ms: 0,
        length_ms: base.duration_ms,
        reencode: false,
        original: None,
    }
}

/// Was nach dem Schreiben in der Datenbank landet.
pub struct Applied {
    pub duration_ms: u64,
    pub size_bytes: u64,
    pub original: Option<ClipOriginal>,
}

/// Den Clip neu schreiben: Mischung einrechnen, Zuschnitt ausführen, Original
/// sichern. Gibt zurück, was danach in der Datenbank stehen muss.
pub fn apply(
    clip: &Clip,
    trim: Trim,
    mix: &[TrackMix],
    encoder: EncoderId,
    bitrate_kbps: u32,
    on_progress: impl Fn(f32),
) -> Result<Applied, String> {
    // Der Eintrag am Clip ist die Wahrheit über die Koordinaten; die Ablage
    // sagt nur, ob es dazu noch eine Datei gibt.
    let plan = plan(
        clip.original.as_ref(),
        has_original(&clip.id),
        clip.duration_ms,
        trim,
    );
    run(clip, &plan, mix, encoder, bitrate_kbps, &on_progress)
}

/// Den Zuschnitt aufheben: die ganze Aufnahme zurückholen, Mischung behalten.
pub fn restore(
    clip: &Clip,
    mix: &[TrackMix],
    encoder: EncoderId,
    bitrate_kbps: u32,
    on_progress: impl Fn(f32),
) -> Result<Applied, String> {
    let base = clip
        .original
        .filter(|_| has_original(&clip.id))
        .ok_or_else(|| "Das Original ist nicht mehr auffindbar.".to_string())?;
    let plan = restore_plan(&base);
    run(clip, &plan, mix, encoder, bitrate_kbps, &on_progress)
}

fn run(
    clip: &Clip,
    plan: &RenderPlan,
    mix: &[TrackMix],
    encoder: EncoderId,
    bitrate_kbps: u32,
    on_progress: &impl Fn(f32),
) -> Result<Applied, String> {
    let _busy = WORKING.lock();

    let target = PathBuf::from(&clip.path);
    if !target.is_file() {
        return Err("Die Clipdatei ist nicht mehr da.".into());
    }

    // Die Einzelspuren vor allem anderen holen: Liegen sie noch nicht in der
    // Ablage, werden sie hier aus der unversehrten Aufnahme gezogen — und die
    // ist bis zum Tausch weiter unter `source_path` erreichbar.
    let source = source_path(clip);
    let list = match stems::tracks(&clip.id, &source) {
        Ok(list) => list,
        Err(err) if recoverable(&source) => {
            log::warn!("Spuren unbrauchbar ({err}) — sie werden neu entpackt");
            stems::remove(&clip.id);
            stems::tracks(&clip.id, &source)?
        }
        Err(err) => return Err(err),
    };

    let video = match plan.video {
        VideoSource::Clip => target.clone(),
        VideoSource::Original => source.clone(),
    };
    if !video.is_file() {
        return Err("Die unversehrte Aufnahme ist nicht mehr auffindbar.".into());
    }

    // ffmpeg darf nicht in seine eigene Eingabe schreiben — also daneben und
    // danach darüber. Der Prozessname macht die Datei je Lauf eindeutig.
    let temp = target.with_extension(format!("{}.neu.mp4", std::process::id()));
    let args = arguments(plan, &video, &list, mix, &temp, encoder, bitrate_kbps);

    match run_with_progress(&args, plan.length_ms, on_progress) {
        Ok(()) => {}
        Err(err) if plan.reencode && encoder != EncoderId::X264 => {
            // Hardware-Encoder streiken gern mal: Treiber, oder alle Sitzungen
            // belegt, weil nebenan der Replay-Puffer läuft.
            log::warn!("Schnitt mit {encoder:?} fehlgeschlagen ({err}) — jetzt mit x264");
            let _ = std::fs::remove_file(&temp);
            let fallback = arguments(
                plan,
                &video,
                &list,
                mix,
                &temp,
                EncoderId::X264,
                bitrate_kbps,
            );
            if let Err(err) = run_with_progress(&fallback, plan.length_ms, on_progress) {
                let _ = std::fs::remove_file(&temp);
                return Err(err);
            }
        }
        Err(err) => {
            let _ = std::fs::remove_file(&temp);
            return Err(err);
        }
    }

    swap_in(clip, plan, &temp, &target)?;
    on_progress(1.0);

    // Gebucht wird, was wirklich herauskam — nicht, was gewünscht war. Sonst
    // driftet die Buchführung mit jedem weiteren Schnitt von der Datei weg.
    let duration_ms = probe_duration_ms(&target).unwrap_or(plan.length_ms);
    let original = plan.original.map(|original| ClipOriginal {
        end_ms: original.start_ms + duration_ms,
        ..original
    });
    if let Some(original) = original.as_ref() {
        write_note(&clip.id, original)?;
    }

    // Das Vorschaubild zeigte sonst ein Bild, das im Clip gar nicht mehr
    // vorkommt.
    if let Err(err) = make_thumbnail(&target) {
        log::warn!("Vorschaubild ließ sich nicht erneuern: {err}");
    }

    Ok(Applied {
        duration_ms,
        size_bytes: std::fs::metadata(&target).map(|meta| meta.len()).unwrap_or(0),
        original,
    })
}

/// Die fertige Datei an ihren Platz bringen — und dabei, wenn es der erste
/// echte Schnitt ist, die unversehrte Aufnahme retten.
///
/// Die Reihenfolge ist der springende Punkt: Jeder Schritt ist für sich
/// abbrechbar, und der letzte rollt zurück. So kann der Clip an keiner Stelle
/// ganz verschwinden.
fn swap_in(clip: &Clip, plan: &RenderPlan, temp: &Path, target: &Path) -> Result<(), String> {
    let archive = video_path(&clip.id);
    let keep_original = plan.original.is_some();
    let first_cut = keep_original && !archive.is_file();

    if first_cut {
        // Zettel zuerst: Stirbt der Prozess gleich, findet `repair` die Datei
        // daran wieder. Umgekehrt läge ein Video ohne jede Zuordnung da.
        write_note(&clip.id, plan.original.as_ref().expect("geprüft"))?;
        if let Err(err) = std::fs::rename(target, &archive) {
            let _ = std::fs::remove_file(temp);
            let _ = std::fs::remove_file(note_path(&clip.id));
            return Err(format!(
                "Das Original ließ sich nicht sichern ({err}). Ist der Clip gerade woanders geöffnet?"
            ));
        }
    }

    if let Err(err) = replace_file(temp, target) {
        if first_cut {
            // Zurück auf Anfang: Lieber ein ungeschnittener Clip als gar keiner.
            let _ = std::fs::rename(&archive, target);
            let _ = std::fs::remove_file(note_path(&clip.id));
            let _ = std::fs::remove_dir_all(dir(&clip.id));
        }
        let _ = std::fs::remove_file(temp);
        return Err(err);
    }

    if !keep_original {
        remove(&clip.id);
    }
    Ok(())
}

/// Die ffmpeg-Zeile. Ausgelagert, damit sie sich ohne Lauf prüfen lässt.
fn arguments(
    plan: &RenderPlan,
    video: &Path,
    list: &[crate::model::ClipTrack],
    mix: &[TrackMix],
    output: &Path,
    encoder: EncoderId,
    bitrate_kbps: u32,
) -> Vec<String> {
    let gains = stems::levels(list, mix);

    // Woher der Ton kommt: aus den abgelegten Einzelspuren, oder — wenn es
    // keine gibt — aus der Videodatei selbst.
    let stem_files: Vec<PathBuf> = list
        .iter()
        .filter_map(|track| track.preview_path.as_ref().map(PathBuf::from))
        .collect();
    let from_stems = stem_files.len() == list.len() && !stem_files.is_empty();

    let seconds = |ms: u64| format!("{:.3}", ms as f64 / 1000.0);

    let mut args: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
    ];

    // `-ss` ist eine Option **je Eingabe** und gehört vor ihr `-i`. Ein
    // einzelnes `-ss` vor dem ersten `-i` wirkte nur auf das Video, und die
    // Einzelspuren liefen um den Zuschnitt versetzt weiter.
    //
    // Beim Kopieren ist `video_start_ms` immer 0 — ein `-ss` mit `-c:v copy`
    // rutschte auf das Keyframe davor (bis zu zwei Sekunden) und stünde hinter
    // `-i` sogar ganz ohne führendes Keyframe da.
    if plan.video_start_ms > 0 {
        args.push("-ss".into());
        args.push(seconds(plan.video_start_ms));
    }
    args.push("-i".into());
    args.push(video.to_string_lossy().to_string());

    if from_stems {
        for path in &stem_files {
            if plan.stems_start_ms > 0 {
                args.push("-ss".into());
                args.push(seconds(plan.stems_start_ms));
            }
            args.push("-i".into());
            args.push(path.to_string_lossy().to_string());
        }
    }

    // Steckt der Ton noch in der Videodatei, teilt er sich deren Zeitachse und
    // ist damit schon durch das `-ss` des Bildes geschnitten (Invariante 2).
    let inputs: Vec<(String, f32)> = if from_stems {
        gains
            .iter()
            .enumerate()
            .map(|(slot, gain)| (format!("{}:a", slot + 1), *gain))
            .collect()
    } else {
        list.iter()
            .zip(&gains)
            .map(|(track, gain)| (format!("0:a:{}", track.index), *gain))
            .collect()
    };
    let filter = stems::mix_filter(&inputs);

    if let Some(filter) = &filter {
        args.push("-filter_complex".into());
        args.push(filter.clone());
    }

    // Bild zuerst abbilden, damit es auch im Ergebnis Spur 0 ist.
    args.push("-map".into());
    args.push("0:v:0".into());
    match &filter {
        Some(_) => {
            args.push("-map".into());
            args.push(stems::MIX_LABEL.into());
            args.extend(["-c:a", "aac", "-b:a", "192k"].map(String::from));
            args.extend(["-metadata:s:a:0", "title=Mix"].map(String::from));
            args.extend(["-metadata:s:a:0", "handler_name=Mix"].map(String::from));
        }
        // Alles stumm geschaltet: Dann bekommt der Clip eben keine Tonspur.
        None => args.push("-an".into()),
    }

    if plan.reencode {
        args.extend(video_args(encoder, bitrate_kbps));
    } else {
        args.extend(["-c:v", "copy"].map(String::from));
    }
    args.extend(["-avoid_negative_ts", "make_zero"].map(String::from));
    args.extend(["-movflags", "+faststart"].map(String::from));

    // `-t` gilt für die ganze Ausgabe und gehört deshalb hinter alle Eingaben.
    args.push("-t".into());
    args.push(seconds(plan.length_ms));
    args.push(output.to_string_lossy().to_string());
    args
}

/// Lassen sich die Einzelspuren notfalls neu ziehen?
///
/// Nur dann darf die Ablage weggeworfen werden. Bei Clips aus der neuen
/// Aufnahme ist sie die **einzige** Fassung der getrennten Quellen — sie zu
/// löschen hieße, die Mischung für immer festzunageln.
fn recoverable(source: &Path) -> bool {
    stems::tracks_in_file(source)
        .map(|list| list.len() > 1)
        .unwrap_or(false)
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
        "ffmpeg ist beim Schreiben des Clips fehlgeschlagen: {}",
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

/// Was ein Absturz mitten im Tausch hinterlassen hat, beim Start aufräumen.
///
/// Der Beipackzettel unterscheidet die Fälle. Die Regel darunter ist immer
/// dieselbe: **Der Eintrag am Clip darf nie verlorengehen, solange die Datei
/// geschnitten ist** — er trägt den Versatz, in dem die Einzelspuren stehen,
/// und ohne ihn liefe die Vorschau für immer versetzt.
pub fn repair(library: &crate::clips::Library) {
    let clips = match library.list() {
        Ok(clips) => clips,
        Err(err) => {
            log::warn!("Original-Ablage nicht prüfbar: {err}");
            return;
        }
    };
    for clip in clips {
        let note = read_note(&clip.id);
        let video = video_path(&clip.id);
        let target = Path::new(&clip.path);

        match (target.is_file(), note, video.is_file()) {
            // Zwischen Sichern und Tauschen gestorben: Die Clipdatei fehlt, die
            // unversehrte Aufnahme liegt in der Ablage. Zurückschieben — ein
            // ungeschnittener Clip ist unendlich viel besser als gar keiner.
            (false, Some(_), true) => {
                if std::fs::rename(&video, target).is_ok() {
                    log::info!("Clip '{}' aus der Original-Ablage zurückgeholt", clip.id);
                    remove(&clip.id);
                    let _ = library.set_original(&clip.id, None);
                }
            }

            // Zettel ohne Aufnahme. Zwei Möglichkeiten, und die Länge des Clips
            // sagt welche: Steht sie noch auf der vollen Dauer, kam der Absturz
            // vor dem Sichern und es wurde nie geschnitten. Sonst hat jemand die
            // Ablage von Hand ausgeräumt — dann ist die Aufnahme verloren, der
            // Versatz der Spuren aber nicht.
            (_, Some(note), false) => {
                remove(&clip.id);
                if clip.duration_ms == note.duration_ms {
                    log::info!("Zettel ohne Original bei '{}' — weggeräumt", clip.id);
                    let _ = library.set_original(&clip.id, None);
                } else {
                    log::warn!("Original von '{}' ist weg — der Clip bleibt geschnitten", clip.id);
                    let _ = library.set_original(&clip.id, Some(&note));
                }
            }

            // Getauscht, aber die Datenbank kam nicht mehr dran.
            (true, Some(note), true) if clip.original.is_none() => {
                log::info!("Original von '{}' nachgetragen", clip.id);
                let _ = library.set_original(&clip.id, Some(&note));
            }

            // Aufnahme ohne Zettel. Weiß die Datenbank noch, wo der Ausschnitt
            // sitzt, wird der Zettel daraus nachgezogen — sonst ist es der Rest
            // eines fertigen „Aufheben" und darf weg.
            (true, None, true) => match clip.original.as_ref() {
                Some(original) => {
                    log::info!("Zettel für '{}' nachgezogen", clip.id);
                    let _ = write_note(&clip.id, original);
                }
                None => {
                    log::info!("Original von '{}' ohne Zuordnung — weggeräumt", clip.id);
                    remove(&clip.id);
                }
            },

            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ClipTrack;

    fn track(index: u32, stem: bool) -> ClipTrack {
        ClipTrack {
            index,
            label: format!("Spur {index}"),
            channels: 2,
            preview_path: stem.then(|| format!("C:/data/tracks/x/{index}.m4a")),
        }
    }

    /// Der häufigste Fall: nur an den Reglern gedreht. Dann bleibt das Bild
    /// unangetastet und die Datei ist in Sekunden neu geschrieben.
    #[test]
    fn only_mixing_leaves_the_picture_alone() {
        let plan = plan(None, true, 30_000, Trim::whole(30_000));
        assert_eq!(plan.video, VideoSource::Clip);
        assert!(!plan.reencode);
        assert_eq!(plan.video_start_ms, 0);
        assert_eq!(plan.stems_start_ms, 0);
        assert_eq!(plan.length_ms, 30_000);
        assert_eq!(plan.original, None, "ohne Zuschnitt braucht es kein Original");
    }

    /// Hinten kürzen ist verlustfrei und schnell — und genau das ist der
    /// übliche Zuschnitt.
    #[test]
    fn trimming_only_the_tail_stays_a_copy() {
        let plan = plan(None, true, 30_000, Trim { start_ms: 0, end_ms: 12_000 });
        assert_eq!(plan.video, VideoSource::Clip);
        assert!(!plan.reencode);
        assert_eq!(plan.length_ms, 12_000);
        assert_eq!(
            plan.original,
            Some(ClipOriginal { duration_ms: 30_000, start_ms: 0, end_ms: 12_000 })
        );
    }

    /// Vorne schneiden muss bildgenau sitzen. Beim Kopieren rutschte der Schnitt
    /// auf das Keyframe davor — deshalb neu encodieren, und zwar aus dem
    /// Original, damit der Verlust bei einer Generation bleibt.
    #[test]
    fn trimming_the_head_re_encodes_from_the_original() {
        let plan = plan(None, true, 30_000, Trim { start_ms: 4_000, end_ms: 12_000 });
        assert_eq!(plan.video, VideoSource::Original);
        assert!(plan.reencode);
        assert_eq!(plan.video_start_ms, 4_000);
        assert_eq!(plan.stems_start_ms, 4_000);
        assert_eq!(plan.length_ms, 8_000);
    }

    /// Ist die Ablage von Hand ausgeräumt worden, muss der Schnitt trotzdem
    /// gehen — dann eben aus der schon geschnittenen Datei, mit **relativem**
    /// Versatz. Die Einzelspuren behalten ihren absoluten.
    #[test]
    fn a_missing_original_falls_back_to_the_clip() {
        let base = ClipOriginal { duration_ms: 60_000, start_ms: 5_000, end_ms: 20_000 };
        let plan = plan(Some(&base), false, 15_000, Trim { start_ms: 2_000, end_ms: 8_000 });
        assert_eq!(plan.video, VideoSource::Clip);
        assert!(plan.reencode, "bildgenau muss es trotzdem sein");
        assert_eq!(plan.video_start_ms, 2_000, "die Clipdatei fängt schon bei 5s an");
        assert_eq!(plan.stems_start_ms, 7_000, "die Spuren stehen im Original");
        assert_eq!(plan.length_ms, 6_000);
    }

    /// Ein Griff, der nicht ganz am Anschlag saß, darf keinen Encodierlauf
    /// auslösen.
    #[test]
    fn a_hair_off_the_edge_is_not_a_cut() {
        let plan = plan(None, true, 30_000, Trim { start_ms: 120, end_ms: 29_900 });
        assert!(!plan.reencode);
        assert_eq!(plan.video, VideoSource::Clip);
        assert_eq!(plan.original, None);
        assert_eq!(plan.length_ms, 30_000);
    }

    /// Weiter hineinschneiden geht ohne Aufheben: Die Griffe stehen auf der
    /// Zeitachse der geschnittenen Datei, gerechnet wird im Original.
    #[test]
    fn cuts_compose_in_original_coordinates() {
        let base = ClipOriginal { duration_ms: 60_000, start_ms: 5_000, end_ms: 20_000 };
        let plan = plan(Some(&base), true, 15_000, Trim { start_ms: 2_000, end_ms: 8_000 });
        assert_eq!(plan.video_start_ms, 7_000);
        assert_eq!(plan.stems_start_ms, 7_000);
        assert_eq!(plan.length_ms, 6_000);
        assert_eq!(
            plan.original,
            Some(ClipOriginal { duration_ms: 60_000, start_ms: 7_000, end_ms: 13_000 })
        );
    }

    /// Dreimal nachgeschnitten und die Buchführung stimmt immer noch.
    #[test]
    fn three_cuts_in_a_row_still_add_up() {
        let mut base = None;
        let mut duration = 60_000;
        for _ in 0..3 {
            let plan = plan(base.as_ref(), true, duration, Trim { start_ms: 1_000, end_ms: duration - 1_000 });
            duration = plan.length_ms;
            base = plan.original;
        }
        let base = base.unwrap();
        assert_eq!(base.start_ms, 3_000);
        assert_eq!(base.end_ms, 57_000);
        assert_eq!(duration, 54_000);
    }

    /// Nur die Mischung ändern, obwohl der Clip längst geschnitten ist: Der
    /// Ausschnitt darf dabei nicht wandern, und die Spuren brauchen weiterhin
    /// ihren Versatz.
    #[test]
    fn remixing_a_trimmed_clip_keeps_its_bounds() {
        let base = ClipOriginal { duration_ms: 60_000, start_ms: 5_000, end_ms: 20_000 };
        let plan = plan(Some(&base), true, 15_000, Trim::whole(15_000));
        assert_eq!(plan.video, VideoSource::Clip);
        assert!(!plan.reencode);
        assert_eq!(plan.video_start_ms, 0, "die Clipdatei fängt schon richtig an");
        assert_eq!(plan.stems_start_ms, 5_000, "die Spuren stehen im Original");
        assert_eq!(plan.length_ms, 15_000);
        assert_eq!(plan.original, Some(base));
    }

    #[test]
    fn restoring_is_a_lossless_copy_of_everything() {
        let base = ClipOriginal { duration_ms: 60_000, start_ms: 5_000, end_ms: 20_000 };
        let plan = restore_plan(&base);
        assert_eq!(plan.video, VideoSource::Original);
        assert!(!plan.reencode);
        assert_eq!(plan.stems_start_ms, 0);
        assert_eq!(plan.length_ms, 60_000);
        assert_eq!(plan.original, None);
    }

    /// `-ss` gilt je Eingabe. Steht es nur einmal vorn, laufen die Spuren um
    /// den Zuschnitt versetzt weiter — der Ton käme aus einer anderen Stelle
    /// des Spiels als das Bild.
    #[test]
    fn every_input_gets_its_own_seek() {
        let plan = plan(None, true, 30_000, Trim { start_ms: 4_000, end_ms: 12_000 });
        let list = vec![track(0, true), track(1, true)];
        let args = arguments(
            &plan,
            Path::new("C:/data/originals/x/video.mp4"),
            &list,
            &[],
            Path::new("C:/clips/x.neu.mp4"),
            EncoderId::X264,
            40_000,
        );

        let inputs: Vec<usize> = args
            .iter()
            .enumerate()
            .filter(|(_, arg)| *arg == "-i")
            .map(|(at, _)| at)
            .collect();
        assert_eq!(inputs.len(), 3, "Video plus zwei Spuren: {args:?}");
        for at in inputs {
            assert_eq!(args[at - 2], "-ss", "jede Eingabe braucht ihr -ss: {args:?}");
            assert_eq!(args[at - 1], "4.000");
        }
    }

    /// `-t` gilt für die Ausgabe und muss hinter alle Eingaben — vorn gelesen
    /// wäre es eine Option von Eingabe 0.
    #[test]
    fn the_length_comes_after_the_last_input() {
        let plan = plan(None, true, 30_000, Trim { start_ms: 0, end_ms: 12_000 });
        let list = vec![track(0, true)];
        let args = arguments(
            &plan,
            Path::new("C:/clips/x.mp4"),
            &list,
            &[],
            Path::new("C:/clips/x.neu.mp4"),
            EncoderId::X264,
            40_000,
        );
        let last_input = args.iter().rposition(|arg| arg == "-i").unwrap();
        let length = args.iter().position(|arg| arg == "-t").unwrap();
        assert!(length > last_input, "{args:?}");
        assert_eq!(args[length + 1], "12.000");
        assert!(args.windows(2).any(|pair| pair == ["-c:v", "copy"]), "{args:?}");
    }

    /// Steckt der Ton noch in der Videodatei, teilt er deren Zeitachse — ein
    /// eigenes `-ss` für ihn schnitte ihn ein zweites Mal.
    #[test]
    fn audio_inside_the_file_inherits_the_picture_offset() {
        let plan = plan(None, true, 30_000, Trim { start_ms: 4_000, end_ms: 12_000 });
        let list = vec![track(0, false)];
        let args = arguments(
            &plan,
            Path::new("C:/clips/x.mp4"),
            &list,
            &[],
            Path::new("C:/clips/x.neu.mp4"),
            EncoderId::X264,
            40_000,
        );
        assert_eq!(args.iter().filter(|arg| *arg == "-i").count(), 1, "{args:?}");
        assert_eq!(args.iter().filter(|arg| *arg == "-ss").count(), 1, "{args:?}");
        let filter = args.iter().position(|arg| arg == "-filter_complex").unwrap();
        assert!(args[filter + 1].contains("0:a:0"), "{}", args[filter + 1]);
    }

    /// Die Ablage darf nicht dort liegen, wo beim Start aufgeräumt wird —
    /// sonst wäre das Original nach dem nächsten Programmstart weg.
    #[test]
    fn originals_live_outside_the_scratch_folders() {
        assert!(!root().starts_with(crate::preview::dir()));
        assert!(!root().starts_with(stems::root()));
        assert!(!root().starts_with(config::data_dir().join("temp")));
        assert!(!root().starts_with(config::data_dir().join("buffer")));
    }

    #[test]
    fn timestamps_become_milliseconds() {
        assert_eq!(parse_timestamp_ms("00:00:01.500000"), Some(1_500));
        assert_eq!(parse_timestamp_ms("01:02:03.000000"), Some(3_723_000));
        assert_eq!(parse_timestamp_ms("kaputt"), None);
    }

    /// x264 ist der Rückfall, wenn die Hardware streikt — er muss immer eine
    /// vollständige Zeile liefern.
    #[test]
    fn every_encoder_yields_a_complete_line() {
        for encoder in [EncoderId::Nvenc, EncoderId::Amf, EncoderId::Qsv, EncoderId::X264] {
            let args = video_args(encoder, 100);
            assert!(args.contains(&"-c:v".to_string()), "{encoder:?}: {args:?}");
            assert!(args.contains(&"-pix_fmt".to_string()), "{encoder:?}: {args:?}");
        }
    }
}
