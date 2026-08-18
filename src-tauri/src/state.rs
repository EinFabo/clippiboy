//! Gemeinsamer Zustand zwischen UI-Commands und Aufnahme-Threads.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::audio::engine::AudioEngine;
use crate::buffer::ReplayBuffer;
use crate::clips::Library;
use crate::config;
use crate::pipeline::{Pipeline, Shared};
use crate::model::{AppConfig, AudioSource, Clip, EngineStatus};

pub struct AppState {
    pub config: Mutex<AppConfig>,
    pub buffer: Mutex<ReplayBuffer>,
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
        let buffer = ReplayBuffer::new(config.buffer.seconds);
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
            buffer: Mutex::new(buffer),
            library: Mutex::new(library),
            status: Mutex::new(status),
            audio,
            pipeline: Mutex::new(None),
            shared: Mutex::new(None),
            current_game: Mutex::new(None),
            buffering_game: Mutex::new(None),
            quitting: std::sync::atomic::AtomicBool::new(false),
            auto: AutoBuffer::default(),
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
        self.buffer.lock().set_capacity(next.buffer.seconds);
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
        if self.pipeline.lock().is_some() {
            return Ok(());
        }
        if !crate::muxer::available() {
            return Err(
                "ffmpeg wurde nicht gefunden. Ohne ffmpeg lassen sich keine Clips schreiben."
                    .into(),
            );
        }

        let config = self.config_snapshot();
        let pipeline = Pipeline::start(
            &config.recording,
            config.buffer.seconds,
            config.sources.clone(),
            self.audio.clone(),
            config::data_dir().join("buffer"),
        )?;
        *self.shared.lock() = Some(pipeline.shared.clone());
        *self.pipeline.lock() = Some(pipeline);
        // Erst kopieren, dann setzen: Hielte man beide Sperren gleichzeitig,
        // liefe das gegen die umgekehrte Reihenfolge in `save_clip` und die
        // beiden Hotkey-Threads könnten sich gegenseitig blockieren.
        let game = self.current_game.lock().clone();
        *self.buffering_game.lock() = game;

        let mut status = self.status.lock();
        status.buffer_active = true;
        status.encoder = Some(config.recording.encoder);
        status.dropped_frames = 0;
        Ok(())
    }

    pub fn stop_pipeline(&self) {
        self.shared.lock().take();
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
            let elapsed = shared.elapsed_ms.load(std::sync::atomic::Ordering::Relaxed);
            let frames = shared.frames.load(std::sync::atomic::Ordering::Relaxed);
            if elapsed > 0 {
                status.fps = frames as f32 * 1000.0 / elapsed as f32;
            }
        }
        status.game = self.current_game.lock().clone();
        status
    }

    /// Schreibt die letzten `seconds` Sekunden als MP4.
    pub fn save_clip(&self, seconds: Option<u32>) -> Result<Clip, String> {
        let config = self.config_snapshot();
        let seconds = seconds.unwrap_or(config.buffer.seconds).max(1);

        // Nur den gemeinsamen Zustand herausholen und die Sperre sofort wieder
        // freigeben: Versiegeln, Muxen und Vorschaubild dauern Sekunden, und
        // solange käme z. B. „Beenden" aus dem Tray nicht an den Puffer heran.
        // `_save` hält die Segmentdateien am Leben, falls parallel gestoppt
        // wird; angemeldet wird es noch unter der Sperre, damit ein Stopp
        // entweder davor greift oder darauf wartet.
        let (shared, _save) = {
            let guard = self.pipeline.lock();
            let pipeline = guard
                .as_ref()
                .ok_or_else(|| "Der Replay-Puffer läuft nicht.".to_string())?;
            (pipeline.shared.clone(), pipeline.shared.begin_save())
        };

        // Das laufende Segment muss abgeschlossen sein, sonst fehlen die
        // letzten Sekunden — also genau der Moment, den man speichern will.
        if !shared.seal_current_segment(std::time::Duration::from_secs(6)) {
            return Err("Die Aufnahme liefert gerade keine Bilder.".into());
        }

        if let Some(err) = shared.error.lock().clone() {
            log::warn!("Encoder meldete: {err}");
        }

        let segments = shared.segments.lock().clone();
        let tracks = shared.tracks.lock().clone();
        let now_ms = shared
            .elapsed_ms
            .load(std::sync::atomic::Ordering::Relaxed);

        // Nicht die Momentaufnahme, sondern das während des Puffers erkannte
        // Spiel — beim Speichern über den Button steht ClippiBoy im Vordergrund.
        let buffering_game = self.buffering_game.lock().clone();
        let game = buffering_game.or_else(|| self.current_game.lock().clone());
        let stamp = jiff::Zoned::now()
            .strftime("%Y-%m-%d_%H-%M-%S")
            .to_string();
        let name = match &game {
            Some(app) => format!("{}_{stamp}.mp4", sanitize_name(app)),
            None => format!("clip_{stamp}.mp4"),
        };

        let result = crate::muxer::build(crate::muxer::ClipRequest {
            segments: &segments,
            tracks: &tracks,
            now_ms,
            seconds,
            output: std::path::PathBuf::from(&config.clip_dir).join(name),
            temp_dir: config::data_dir().join("temp"),
        })?;

        Ok(Clip {
            id: uuid::Uuid::new_v4().to_string(),
            path: result.path.to_string_lossy().to_string(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
            duration_ms: result.duration_ms,
            game,
            width: config.recording.width,
            height: config.recording.height,
            size_bytes: result.size_bytes,
            thumb_path: result.thumb_path.map(|p| p.to_string_lossy().to_string()),
            title: None,
            description: None,
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
