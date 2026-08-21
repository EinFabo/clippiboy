//! Gemeinsamer Zustand zwischen UI-Commands und Aufnahme-Threads.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::audio::engine::AudioEngine;
use crate::clips::Library;
use crate::config;
use crate::pipeline::{Pipeline, Shared};
use crate::model::{AppConfig, AudioSource, Clip, EngineStatus, RecordingConfig};

pub struct AppState {
    pub config: Mutex<AppConfig>,
    pub library: Mutex<Option<Library>>,
    pub status: Mutex<EngineStatus>,
    pub audio: Arc<AudioEngine>,
    pub pipeline: Mutex<Option<Pipeline>>,
    /// Zweiter Griff auf den Pipeline-Zustand, nur zum Lesen von Kennzahlen.
    ///
    /// `save_clip()` hält `pipeline` über den ganzen Seal- und ffmpeg-Lauf; ein
    /// Statustick, der dort mitlesen wollte, würde eine Sekunde blockieren.
    pub shared: Mutex<Option<Arc<Shared>>>,
    /// Zuletzt erkanntes Vordergrundspiel, fortlaufend aktualisiert.
    pub current_game: Mutex<Option<String>>,
    /// Spiel, das lief, als zuletzt gepuffert wurde — der Name am Clip.
    ///
    /// Gemerkt wird er, weil beim Speichern über den Button ClippiBoy selbst im
    /// Vordergrund steht und die Momentaufnahme dann leer wäre.
    pub buffering_game: Mutex<Option<String>>,
    /// Erst wenn im Tray „Beenden" gewählt wurde, darf der Prozess aussteigen —
    /// sonst versteckt das ✕ das Fenster nur.
    pub quitting: std::sync::atomic::AtomicBool,
    /// Merker für den selbsttätigen Puffer, siehe `AutoBuffer`.
    pub auto: AutoBuffer,
    /// Die Aufnahmeeinstellungen, mit denen der laufende Puffer gestartet
    /// wurde — an die Quelle angeglichen und damit die echten Maße des Clips.
    pub active_recording: Mutex<Option<RecordingConfig>>,
    /// Läuft gerade ein Speichervorgang? Siehe `begin_save`.
    saving: std::sync::atomic::AtomicBool,
    /// Umschließt Start und Stopp der Aufnahme.
    ///
    /// Die Prüfung „läuft schon?" und das Eintragen der fertigen Pipeline
    /// liegen weit auseinander — dazwischen wird die Aufnahmehardware
    /// hochgefahren, was einen Moment dauert. Ohne diese Sperre kämen zwei
    /// gleichzeitige Starts (Hotkey und Automatik, oder Tray und Fenster)
    /// beide an der Prüfung vorbei und legten **zwei** Aufnahmen an; die erste
    /// hinge danach unerreichbar fest und hielte Encoder und Bildschirm
    /// belegt. Ein Stopp mitten in einem Start hätte ebenso ins Leere
    /// gegriffen.
    lifecycle: Mutex<()>,
}

/// Lebt so lange, wie ein Clip geschrieben wird, und gibt den Platz beim
/// Fallenlassen wieder frei — auch wenn zwischendurch ein `?` zuschlägt.
pub struct SavingGuard<'a>(&'a std::sync::atomic::AtomicBool);

impl Drop for SavingGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Zustand der Automatik „Puffer an, sobald ein Spiel läuft".
///
/// Zwei Dinge muss sie auseinanderhalten: Einen Puffer, den der Nutzer selbst
/// gestartet hat, darf sie nicht wieder ausschalten. Und einen, den der Nutzer
/// selbst ausgeschaltet hat, darf sie nicht zwei Sekunden später neu starten —
/// deshalb bleibt sie bis zum Ende des Spiels stumm.
#[derive(Default)]
pub struct AutoBuffer {
    /// Läuft der Puffer, weil die Automatik ihn gestartet hat?
    started: std::sync::atomic::AtomicBool,
    /// Vom Nutzer abgeschaltet — bis das Spiel weg ist, nicht wieder anfassen.
    suppressed: std::sync::atomic::AtomicBool,
    /// Wie viele Prüfungen in Folge kein Spiel gesehen haben.
    missing: std::sync::atomic::AtomicU32,
}

/// Was die Automatik als Nächstes tun soll.
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum AutoAction {
    Nothing,
    Start,
    Stop,
}

/// So viele Prüfungen ohne erkanntes Spiel gelten als „Spiel beendet".
/// Geprüft wird alle zwei Sekunden — kurzes Alt-Tab darf den Puffer nicht
/// abwürgen, sonst ist der Mitschnitt genau dann weg, wenn man zurückkommt.
const GRACE_POLLS: u32 = 15;

impl AutoBuffer {
    /// Eine Runde der Automatik. Wird im Takt der Spielerkennung aufgerufen und
    /// nur, wenn „automatisch starten" und „nur im Spiel" beide an sind.
    pub fn poll(&self, game_present: bool, buffer_active: bool) -> AutoAction {
        use std::sync::atomic::Ordering::SeqCst;
        if game_present {
            self.missing.store(0, SeqCst);
            if !buffer_active && !self.suppressed.load(SeqCst) {
                self.started.store(true, SeqCst);
                return AutoAction::Start;
            }
            return AutoAction::Nothing;
        }

        if self.missing.fetch_add(1, SeqCst) + 1 < GRACE_POLLS {
            return AutoAction::Nothing;
        }
        // Das Spiel ist wirklich beendet: ein neues darf den Puffer wieder
        // starten, auch wenn der Nutzer ihn zwischendurch abgeschaltet hat.
        self.suppressed.store(false, SeqCst);
        if buffer_active && self.started.swap(false, SeqCst) {
            AutoAction::Stop
        } else {
            AutoAction::Nothing
        }
    }

    /// Eine Runde für den Betrieb ohne „nur im Spiel": Der Puffer soll
    /// laufen, solange ihn niemand von Hand ausgeschaltet hat.
    ///
    /// Das im Takt zu prüfen statt nur beim Programmstart heißt, dass ein
    /// Umschalten in den Einstellungen sofort wirkt — vorher passierte bis zum
    /// nächsten Start von ClippiBoy schlicht nichts.
    pub fn poll_always(&self, buffer_active: bool) -> AutoAction {
        use std::sync::atomic::Ordering::SeqCst;
        if buffer_active || self.suppressed.load(SeqCst) {
            return AutoAction::Nothing;
        }
        self.started.store(true, SeqCst);
        AutoAction::Start
    }

    /// Der Nutzer hat den Puffer selbst gestartet.
    pub fn manual_start(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        self.started.store(false, SeqCst);
        self.suppressed.store(false, SeqCst);
    }

    /// Der Nutzer hat den Puffer selbst gestoppt.
    pub fn manual_stop(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        self.started.store(false, SeqCst);
        self.suppressed.store(true, SeqCst);
    }
}

impl AppState {
    pub fn new() -> Self {
        let config = config::load();
        let library = match Library::open() {
            Ok(lib) => Some(lib),
            Err(err) => {
                log::error!("Clip-Datenbank konnte nicht geöffnet werden: {err}");
                None
            }
        };
        let status = EngineStatus {
            buffer_active: false,
            buffered_seconds: 0.0,
            buffer_bytes: 0,
            dropped_frames: 0,
            encoder: Some(config.recording.encoder),
            fps: 0.0,
            game: None,
        };

        // Die Quellen laufen ab dem Start mit — nur so zeigt der Mixer echte
        // Pegel, auch bevor der Puffer aktiv ist.
        let audio = Arc::new(AudioEngine::new());
        audio.apply(&config.sources);

        Self {
            config: Mutex::new(config),
            library: Mutex::new(library),
            status: Mutex::new(status),
            audio,
            pipeline: Mutex::new(None),
            shared: Mutex::new(None),
            current_game: Mutex::new(None),
            buffering_game: Mutex::new(None),
            quitting: std::sync::atomic::AtomicBool::new(false),
            auto: AutoBuffer::default(),
            lifecycle: Mutex::new(()),
            active_recording: Mutex::new(None),
            saving: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn config_snapshot(&self) -> AppConfig {
        self.config.lock().clone()
    }

    /// Konfiguration ersetzen, auf Platte schreiben und abhängige Teile
    /// nachziehen (Pufferlänge und Tonquellen). Die Bildquelle einer laufenden
    /// Aufnahme wechselt `commands::set_config` — dort lässt sich der Neustart
    /// auch melden.
    pub fn replace_config(&self, mut next: AppConfig) -> AppConfig {
        next.recording.encoder = crate::encode::resolve(next.recording.encoder);
        self.audio.apply(&next.sources);
        // Läuft gerade eine Aufnahme, muss sie die geänderten Quellen auch
        // mitbekommen — sonst mischt sie bis zum Neustart die alten.
        if let Some(shared) = self.shared.lock().as_ref() {
            shared.set_sources(next.sources.clone());
        }
        {
            let mut guard = self.config.lock();
            *guard = next.clone();
        }
        if let Err(err) = config::save(&next) {
            log::error!("Konfiguration konnte nicht gespeichert werden: {err}");
        }
        next
    }

    /// Meldet einen Speichervorgang an. `None` heißt: es läuft schon einer.
    ///
    /// Bis ein Clip geschrieben ist, vergehen je nach Bitrate und Länge ein
    /// paar Sekunden. Ohne diese Sperre startet jeder weitere Tastendruck in
    /// dieser Zeit einen zweiten Lauf — und genau das war der häufigste Grund
    /// für „ffmpeg ist fehlgeschlagen": Zwei Läufe kurz hintereinander teilten
    /// sich Zielpfad und Temp-Dateien und räumten sie sich gegenseitig weg.
    pub fn begin_save(&self) -> Option<SavingGuard<'_>> {
        use std::sync::atomic::Ordering::SeqCst;
        match self.saving.compare_exchange(false, true, SeqCst, SeqCst) {
            Ok(_) => Some(SavingGuard(&self.saving)),
            Err(_) => None,
        }
    }

    /// Läuft gerade eine Aufnahme in den Puffer?
    pub fn is_buffering(&self) -> bool {
        self.pipeline.lock().is_some()
    }

    pub fn upsert_source(&self, source: AudioSource) -> AppConfig {
        let mut config = self.config_snapshot();
        match config.sources.iter_mut().find(|s| s.id == source.id) {
            Some(existing) => *existing = source,
            None => config.sources.push(source),
        }
        self.replace_config(config)
    }

    pub fn remove_source(&self, id: &str) -> AppConfig {
        let mut config = self.config_snapshot();
        config.sources.retain(|s| s.id != id);
        self.replace_config(config)
    }
}

impl AppState {
    /// Startet die Aufnahme in den Replay-Puffer.
    pub fn start_pipeline(&self) -> Result<(), String> {
        let _lifecycle = self.lifecycle.lock();
        if self.pipeline.lock().is_some() {
            return Ok(());
        }
        if !crate::muxer::available() {
            return Err(
                "ffmpeg wurde nicht gefunden. Ohne ffmpeg lassen sich keine Clips schreiben."
                    .into(),
            );
        }

        let mut config = self.config_snapshot();
        // Der Bildschirm kann seit dem Einstellen gewechselt haben — lieber
        // hier noch einmal an die Quelle angleichen als hochskaliert
        // aufnehmen.
        crate::capture::fit_to_target(&mut config.recording);
        let pipeline = Pipeline::start(
            &config.recording,
            config.buffer.seconds,
            config.sources.clone(),
            self.audio.clone(),
        )?;
        *self.shared.lock() = Some(pipeline.shared.clone());
        *self.pipeline.lock() = Some(pipeline);
        *self.active_recording.lock() = Some(config.recording.clone());
        // Erst kopieren, dann setzen: Hielte man beide Sperren gleichzeitig,
        // liefe das gegen die umgekehrte Reihenfolge in `save_clip` und die
        // beiden Hotkey-Threads könnten sich gegenseitig blockieren.
        let game = self.current_game.lock().clone();
        *self.buffering_game.lock() = game;

        let mut status = self.status.lock();
        status.buffer_active = true;
        // Der tatsächlich gewählte Encoder, nicht der gewünschte: Ist der
        // Wunsch-MFT nicht angemeldet, stünde in der Anzeige sonst dauerhaft
        // etwas anderes, als da läuft.
        status.encoder = match self.shared.lock().as_ref() {
            Some(shared) => *shared.encoder.lock(),
            None => None,
        };
        status.dropped_frames = 0;
        Ok(())
    }

    pub fn stop_pipeline(&self) {
        let _lifecycle = self.lifecycle.lock();
        self.shared.lock().take();
        self.active_recording.lock().take();
        if let Some(mut pipeline) = self.pipeline.lock().take() {
            pipeline.stop();
        }
        let mut status = self.status.lock();
        status.buffer_active = false;
        status.buffered_seconds = 0.0;
        status.buffer_bytes = 0;
        status.fps = 0.0;
    }

    /// Vom Statustick aufgerufen: Vordergrundspiel nachführen.
    ///
    /// Solange gepuffert wird, überschreibt nur eine echte Erkennung den
    /// gemerkten Namen — wechselt man aus dem Spiel auf den Desktop, bleibt der
    /// Clip trotzdem dem Spiel zugeordnet.
    pub fn track_game(&self) -> Option<String> {
        let detected = crate::game::detect();
        *self.current_game.lock() = detected.clone();
        if detected.is_some() && self.status.lock().buffer_active {
            *self.buffering_game.lock() = detected.clone();
        }
        detected
    }

    /// Aktuelle Kennzahlen der laufenden Aufnahme in den Status übernehmen.
    pub fn status_snapshot(&self) -> EngineStatus {
        let mut status = self.status.lock().clone();
        if let Some(shared) = self.shared.lock().as_ref() {
            status.buffered_seconds = shared.buffered_seconds();
            status.buffer_bytes = shared.buffer_bytes();
            status.dropped_frames = shared.dropped.load(std::sync::atomic::Ordering::Relaxed);
            // Der Encoder bekommt konstant `fps` Bilder — die Zahl allein sagt
            // also nichts. Interessant ist, wie viele davon echt waren: Der
            // Rest sind Wiederholungen, weil das Bild stillstand.
            let frames = shared.frames.load(std::sync::atomic::Ordering::Relaxed);
            let duplicated = shared.duplicated.load(std::sync::atomic::Ordering::Relaxed);
            if frames > 0 {
                let live = frames.saturating_sub(duplicated) as f32 / frames as f32;
                status.fps = shared.fps as f32 * live;
            }
        }
        status.game = self.current_game.lock().clone();
        status
    }

    /// Schreibt die letzten `seconds` Sekunden als MP4.
    pub fn save_clip(&self, seconds: Option<u32>) -> Result<Clip, String> {
        let config = self.config_snapshot();
        let seconds = seconds.unwrap_or(config.buffer.seconds).max(1);

        // Alles unter einer kurzen Sperre abgreifen und sie sofort wieder
        // freigeben: Muxen und Vorschaubild dauern Sekunden, und solange käme
        // z. B. „Beenden" aus dem Tray nicht an den Puffer heran.
        //
        // Ein Versiegeln wie früher gibt es nicht mehr: Der Encoder läuft
        // durch, jedes fertige Paket liegt bereits im Ring. Es gibt also
        // nichts abzuwarten und nichts, was dabei verloren gehen könnte.
        let snapshot = {
            let guard = self.pipeline.lock();
            let pipeline = guard
                .as_ref()
                .ok_or_else(|| "Der Replay-Puffer läuft nicht.".to_string())?;
            pipeline.snapshot(seconds)?
        };

        // Die Maße der Datei stammen von der laufenden Aufnahme, nicht aus der
        // Konfiguration: gespeichert wird, was der Encoder wirklich bekommen hat.
        let recording = self
            .active_recording
            .lock()
            .clone()
            .unwrap_or_else(|| config.recording.clone());

        // Nicht die Momentaufnahme, sondern das während des Puffers erkannte
        // Spiel — beim Speichern über den Button steht ClippiBoy im Vordergrund.
        let buffering_game = self.buffering_game.lock().clone();
        let game = buffering_game.or_else(|| self.current_game.lock().clone());
        // Millisekunden gehören dazu: Zwei Clips in derselben Sekunde bekamen
        // sonst denselben Pfad. Beide ffmpeg-Läufe schrieben dann dieselbe
        // Datei, teilten sich die Segmentliste im Temp-Ordner, und in der
        // Datenbank (`path` ist UNIQUE) blieb am Ende nur einer von beiden übrig.
        let now = jiff::Zoned::now();
        let stamp = format!(
            "{}-{:03}",
            now.strftime("%Y-%m-%d_%H-%M-%S"),
            now.subsec_nanosecond() / 1_000_000
        );
        let name = match &game {
            Some(app) => format!("{}_{stamp}.mp4", sanitize_name(app)),
            None => format!("clip_{stamp}.mp4"),
        };

        // Die Kennung schon hier: Unter ihr legt der Muxer die Einzelspuren ab,
        // und die entstehen im selben ffmpeg-Lauf wie der Clip.
        let id = uuid::Uuid::new_v4().to_string();
        let result = crate::muxer::build(crate::muxer::ClipRequest {
            snapshot,
            clip_id: id.clone(),
            output: std::path::PathBuf::from(&config.clip_dir).join(name),
            temp_dir: config::data_dir().join("temp"),
        })?;

        Ok(Clip {
            id,
            path: result.path.to_string_lossy().to_string(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
            duration_ms: result.duration_ms,
            game,
            width: recording.width,
            height: recording.height,
            size_bytes: result.size_bytes,
            thumb_path: result.thumb_path.map(|p| p.to_string_lossy().to_string()),
            title: None,
            description: None,
            edit: None,
            original: None,
        })
    }
}

fn sanitize_name(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
