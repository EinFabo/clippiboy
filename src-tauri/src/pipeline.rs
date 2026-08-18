//! Aufnahme-Pipeline: Bildschirm → Hardware-Encoder → Segment-Ring → Clip.
//!
//! Aufgenommen wird durchgehend in MPEG-TS-Segmente (Standard: 10 s). TS ist
//! der einzige Container, der sich verlustfrei aneinanderhängen lässt — beim
//! Speichern eines Clips werden also nur die passenden Segmente kopiert und in
//! ein MP4 umgepackt, ohne neu zu encodieren.
//!
//! Der Ton kommt aus dem Mixer: der Hauptmix läuft direkt in die Aufnahme, die
//! Quellen mit eigener Spur werden parallel als PCM mitgeschrieben und beim
//! Speichern als zusätzliche Tonspuren ins MP4 gemuxt.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::audio::capture::{CHANNELS, SAMPLE_RATE};
use crate::audio::engine::AudioEngine;
use crate::audio::TrackLayout;
use crate::model::{AudioSource, RecordingConfig, TargetKind};

/// Länge eines Segments. Kurz genug für feines Zuschneiden, lang genug, dass
/// die Segmentwechsel selten sind.
const SEGMENT_SECONDS: u64 = 10;

/// So lang muss ein Segment mindestens sein, bevor es abgeschlossen werden
/// darf. Fällt ein erzwungenes Versiegeln direkt hinter eine planmäßige
/// Rotation, entstünde sonst ein Segment von wenigen Millisekunden.
const MIN_SEGMENT_MS: u64 = 400;

#[derive(Debug, Clone)]
pub struct Segment {
    /// Laufende Nummer in der Reihenfolge, in der die Segmente aufgenommen
    /// wurden. Die Abschluss-Threads laufen nebenläufig und werden nicht
    /// zwingend in dieser Reihenfolge fertig — der Ring wird danach sortiert.
    pub index: u64,
    pub path: PathBuf,
    /// Millisekunden seit Start der Aufnahme.
    pub start_ms: u64,
    pub end_ms: u64,
    /// Beim Abschließen einmal ermittelt — sonst müsste die Statusanzeige im
    /// Sekundentakt jede Segmentdatei neu statten.
    pub bytes: u64,
}

/// Ringpuffer für eine separat mitgeschriebene Tonspur (16-bit PCM).
pub struct TrackRing {
    pub source_id: String,
    pub label: String,
    samples: Mutex<VecDeque<i16>>,
    capacity: usize,
}

impl TrackRing {
    fn new(source_id: String, label: String, seconds: u32) -> Self {
        Self {
            source_id,
            label,
            samples: Mutex::new(VecDeque::new()),
            capacity: seconds as usize * SAMPLE_RATE as usize * CHANNELS,
        }
    }

    fn push(&self, block: &[i16]) {
        let mut samples = self.samples.lock();
        samples.extend(block.iter().copied());
        if samples.len() > self.capacity {
            let excess = samples.len() - self.capacity;
            samples.drain(..excess);
        }
    }

    /// Die letzten `seconds` Sekunden als WAV-Datei schreiben.
    pub fn write_wav(&self, path: &Path, seconds: u32) -> std::io::Result<()> {
        let wanted = seconds as usize * SAMPLE_RATE as usize * CHANNELS;
        let samples = self.samples.lock();
        let start = samples.len().saturating_sub(wanted);
        let data: Vec<i16> = samples.iter().skip(start).copied().collect();
        drop(samples);

        let byte_len = (data.len() * 2) as u32;
        let mut out = Vec::with_capacity(byte_len as usize + 44);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + byte_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&(CHANNELS as u16).to_le_bytes());
        out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        out.extend_from_slice(&(SAMPLE_RATE * CHANNELS as u32 * 2).to_le_bytes());
        out.extend_from_slice(&((CHANNELS * 2) as u16).to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&byte_len.to_le_bytes());
        for sample in data {
            out.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(path, out)
    }
}

/// Von Pipeline und Capture-Thread gemeinsam genutzter Zustand.
pub struct Shared {
    pub dir: PathBuf,
    pub buffer_seconds: u32,
    pub segments: Mutex<Vec<Segment>>,
    pub tracks: Mutex<Vec<Arc<TrackRing>>>,
    pub sources: Mutex<Vec<AudioSource>>,
    pub audio: Arc<AudioEngine>,
    pub force_rotate: AtomicBool,
    /// Index des Segments, in das gerade geschrieben wird. `seal_current_segment`
    /// merkt sich den Wert und wartet gezielt auf genau dieses Segment.
    pub current_segment: AtomicU64,
    /// Wie viele Clips gerade geschrieben werden. `Pipeline::stop` wartet
    /// darauf, sonst löscht es die Segmentdateien unter ffmpeg weg.
    pub saves_in_flight: AtomicU64,
    pub frames: AtomicU64,
    /// Bilder, die im *gerade laufenden* Segment gelandet sind. Wird bei jeder
    /// Rotation zurückgesetzt. Ein Segment ohne ein einziges Bild darf nicht
    /// abgeschlossen werden — siehe `rotate_due`.
    pub segment_frames: AtomicU64,
    pub dropped: AtomicU64,
    pub elapsed_ms: AtomicU64,
    pub error: Mutex<Option<String>>,
    /// Wird hochgezählt, wenn sich `sources` ändert. Der Capture-Thread hält
    /// eine Kopie und liest nur nach, wenn sich die Zahl bewegt hat — sonst
    /// klont er die Quellenliste 60-mal pro Sekunde.
    pub sources_generation: AtomicU64,
}

impl Shared {
    /// Gesamtdauer, die der Segment-Ring gerade abdeckt.
    pub fn buffered_seconds(&self) -> f32 {
        let segments = self.segments.lock();
        let elapsed = self.elapsed_ms.load(Ordering::Relaxed);
        match segments.first() {
            Some(first) => (elapsed.saturating_sub(first.start_ms)) as f32 / 1000.0,
            None => 0.0,
        }
    }

    pub fn buffer_bytes(&self) -> u64 {
        self.segments.lock().iter().map(|s| s.bytes).sum()
    }

    /// Fertiges Segment in den Ring legen — nach Index einsortiert.
    ///
    /// `finish()` läuft je Segment in einem eigenen Thread; ein kurzes
    /// erzwungenes Segment kann vor einem langen fertig werden. Unsortiert
    /// stünde der Clip später in der falschen Reihenfolge, `prune` würde die
    /// neueste Datei löschen und `buffered_seconds` liefe daneben.
    fn push_segment(&self, segment: Segment) {
        let mut segments = self.segments.lock();
        let at = segments
            .iter()
            .position(|existing| existing.index > segment.index)
            .unwrap_or(segments.len());
        segments.insert(at, segment);
    }

    /// Laufendes Segment abschließen lassen und warten, bis es im Ring liegt.
    ///
    /// Es genügt nicht, auf einen wachsenden Zähler zu warten: ein noch
    /// laufender Abschluss-Thread einer früheren Rotation kann ihn hochziehen,
    /// und dann fehlten dem Clip genau die letzten Sekunden.
    pub fn seal_current_segment(&self, timeout: Duration) -> bool {
        let wanted = self.current_segment.load(Ordering::Relaxed);
        if self.sealed(wanted) {
            return true;
        }
        self.force_rotate.store(true, Ordering::Relaxed);

        let started = Instant::now();
        let deadline = started + timeout;
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            if self.sealed(wanted) {
                return true;
            }
            // Steht das Bild still, liefert Windows.Graphics.Capture keine
            // Frames — das laufende Segment bleibt dann leer und lässt sich
            // nicht abschließen. Zu holen gibt es aber auch nichts: Alles
            // Aufgenommene liegt vollständig im vorherigen Segment. Nach einer
            // Sekunde ohne ein einziges Bild ist das eindeutig (bei 60 fps
            // wären es 60 gewesen).
            if started.elapsed() > Duration::from_secs(1)
                && self.segment_frames.load(Ordering::Relaxed) == 0
                && !self.segments.lock().is_empty()
            {
                self.force_rotate.store(false, Ordering::Relaxed);
                return true;
            }
        }
        self.force_rotate.store(false, Ordering::Relaxed);
        false
    }

    fn sealed(&self, index: u64) -> bool {
        self.segments.lock().iter().any(|s| s.index >= index)
    }

    /// Meldet ein laufendes Speichern an. Solange die Rückgabe lebt, räumt
    /// `Pipeline::stop` die Segmentdateien nicht weg.
    pub fn begin_save(self: &Arc<Self>) -> SaveGuard {
        self.saves_in_flight.fetch_add(1, Ordering::SeqCst);
        SaveGuard(self.clone())
    }

    /// Neue Quellenliste übernehmen und den Capture-Thread darüber informieren.
    pub fn set_sources(&self, sources: Vec<AudioSource>) {
        *self.sources.lock() = sources;
        self.sources_generation.fetch_add(1, Ordering::Relaxed);
    }

    fn prune(&self) {
        // Während ein Clip geschrieben wird, liest ffmpeg noch aus genau diesen
        // Dateien. Der `SaveGuard` hält sie deshalb auch hier fest, nicht nur
        // gegen `Pipeline::stop`. Eine ausgelassene Runde kostet nichts — der
        // Ring wird dann einmal um ein Segment länger als nötig.
        if self.saves_in_flight.load(Ordering::SeqCst) > 0 {
            return;
        }
        let mut segments = self.segments.lock();
        let elapsed = self.elapsed_ms.load(Ordering::Relaxed);
        let keep_from = elapsed.saturating_sub((self.buffer_seconds as u64 + SEGMENT_SECONDS) * 1000);
        while segments.len() > 1 && segments[0].end_ms < keep_from {
            let old = segments.remove(0);
            let _ = std::fs::remove_file(&old.path);
        }
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use crossbeam_channel::Receiver;
    use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
    use windows_capture::encoder::{
        AudioSettingsBuilder, ContainerSettingsBuilder, ContainerSettingsSubType, VideoEncoder,
        VideoSettingsBuilder, VideoSettingsSubType,
    };
    use windows_capture::frame::Frame;
    use windows_capture::graphics_capture_api::InternalCaptureControl;
    use windows_capture::monitor::Monitor;
    use windows_capture::settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    };
    use windows_capture::window::Window;

    /// Takt des Ton- und Zeitgebers.
    const PUMP_INTERVAL: Duration = Duration::from_millis(10);

    /// Wie viel Ton höchstens auf Abholung warten darf. Alles darüber ist
    /// Rückstand gegenüber dem Bild und wird verworfen.
    const MAX_AUDIO_BACKLOG_MS: usize = 40;

    pub struct Flags {
        pub slot: Arc<EncoderSlot>,
    }

    struct SegmentState {
        index: u64,
        path: PathBuf,
        start_ms: u64,
    }

    /// Encoder plus Segmentzustand, geteilt zwischen Capture-Thread und Pumpe.
    ///
    /// Warum geteilt: Windows.Graphics.Capture liefert nur bei Bildänderung ein
    /// Frame. Hinge — wie ursprünglich — Ton, Segmentwechsel und die Uhr am
    /// Frame-Callback, dann stünde bei ruhigem Bild alles still: der Ton
    /// verhungert, das Segment rotiert nicht und `elapsed_ms` friert ein.
    /// Deshalb läuft daneben eine Pumpe auf eigener Uhr.
    pub struct EncoderSlot {
        shared: Arc<Shared>,
        recording: RecordingConfig,
        start: Instant,
        encoder: Mutex<Option<VideoEncoder>>,
        segment: Mutex<SegmentState>,
        next_encoder: Mutex<Option<Receiver<Result<VideoEncoder, String>>>>,
        /// Bereits gelieferte Audio-Frames über alle Segmente hinweg. Diese
        /// Zahl ist die Autorität — dadurch bleibt die Gesamtlänge des Tons
        /// exakt an der Videozeit, auch wenn ein Segmentwechsel mitten in einen
        /// Tonblock fällt.
        audio_frames_sent: AtomicU64,
        /// Zeitpunkt des ersten Bildes, in Millisekunden seit `start`.
        ///
        /// Der Encoder stempelt Video relativ zum ersten Bild, Ton dagegen
        /// relativ zum ersten Tonblock. Bis das erste Bild eintrifft, ist der
        /// Capture-Stack aber schon einige hundert Millisekunden am Aufbauen.
        /// Ohne gemeinsamen Nullpunkt wäre der Ton genau um diese Zeit voraus.
        video_start_ms: AtomicU64,
        video_started: AtomicBool,
        running: AtomicBool,
    }

    fn build_encoder(
        recording: &RecordingConfig,
        path: &Path,
    ) -> Result<VideoEncoder, Box<dyn std::error::Error + Send + Sync>> {
        // H.264 statt des Standard-HEVC: Discord, Browser und Schnittprogramme
        // nehmen H.264 überall an, HEVC nicht.
        let video = VideoSettingsBuilder::new(recording.width, recording.height)
            .sub_type(VideoSettingsSubType::H264)
            .frame_rate(recording.fps)
            .bitrate(recording.bitrate_kbps * 1000);
        let audio = AudioSettingsBuilder::new()
            .sample_rate(SAMPLE_RATE)
            .channel_count(CHANNELS as u32)
            .bit_per_sample(16)
            .bitrate(192_000);
        let container = ContainerSettingsBuilder::new().sub_type(ContainerSettingsSubType::MPEG2);

        Ok(VideoEncoder::new(video, audio, container, path)?)
    }

    fn segment_path(dir: &Path, index: u64) -> PathBuf {
        dir.join(format!("segment_{index:06}.ts"))
    }

    /// Encoder für ein Segment nebenher aufsetzen; das Ergebnis kommt über den
    /// Kanal zurück.
    fn spawn_encoder(
        recording: RecordingConfig,
        path: PathBuf,
    ) -> Receiver<Result<VideoEncoder, String>> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let built = build_encoder(&recording, &path).map_err(|e| e.to_string());
            let _ = tx.send(built);
        });
        rx
    }

    fn to_i16(sample: f32) -> i16 {
        (sample.clamp(-1.0, 1.0) * 32767.0) as i16
    }

    impl EncoderSlot {
        pub fn new(shared: Arc<Shared>, recording: RecordingConfig) -> Result<Arc<Self>, String> {
            std::fs::create_dir_all(&shared.dir).map_err(|e| e.to_string())?;
            let path = segment_path(&shared.dir, 0);
            let encoder = build_encoder(&recording, &path).map_err(|e| e.to_string())?;
            // Den Encoder fürs nächste Segment schon jetzt bauen lassen: das
            // dauert deutlich länger als ein Bildabstand und darf später weder
            // den Capture-Thread noch die Pumpe aufhalten.
            let next = spawn_encoder(recording.clone(), segment_path(&shared.dir, 1));

            Ok(Arc::new(Self {
                shared,
                recording,
                start: Instant::now(),
                encoder: Mutex::new(Some(encoder)),
                segment: Mutex::new(SegmentState {
                    index: 0,
                    path,
                    start_ms: 0,
                }),
                next_encoder: Mutex::new(Some(next)),
                audio_frames_sent: AtomicU64::new(0),
                video_start_ms: AtomicU64::new(0),
                video_started: AtomicBool::new(false),
                running: AtomicBool::new(true),
            }))
        }

        fn now_ms(&self) -> u64 {
            self.start.elapsed().as_millis() as u64
        }

        fn send_frame(&self, frame: &Frame) {
            // Das erste Bild setzt den gemeinsamen Nullpunkt für Bild und Ton.
            if !self.video_started.load(Ordering::Acquire) {
                self.video_start_ms
                    .store(self.now_ms(), Ordering::Relaxed);
                self.video_started.store(true, Ordering::Release);
            }
            let mut guard = self.encoder.lock();
            let Some(encoder) = guard.as_mut() else {
                self.shared.dropped.fetch_add(1, Ordering::Relaxed);
                return;
            };
            if let Err(err) = encoder.send_frame(frame) {
                self.shared.dropped.fetch_add(1, Ordering::Relaxed);
                *self.shared.error.lock() = Some(format!("Encoder: {err}"));
            } else {
                self.shared.frames.fetch_add(1, Ordering::Relaxed);
                self.shared.segment_frames.fetch_add(1, Ordering::Relaxed);
            }
        }

        /// Aktuelles Segment abschließen und das nächste beginnen.
        fn rotate(&self, now_ms: u64) {
            let Some(encoder) = self.encoder.lock().take() else {
                return;
            };

            let (finished_path, finished_index, start_ms, next_index) = {
                let mut segment = self.segment.lock();
                let finished = segment.path.clone();
                let finished_index = segment.index;
                let start = segment.start_ms;
                segment.index += 1;
                segment.path = segment_path(&self.shared.dir, segment.index);
                segment.start_ms = now_ms;
                self.shared
                    .current_segment
                    .store(segment.index, Ordering::Relaxed);
                self.shared.segment_frames.store(0, Ordering::Relaxed);
                (finished, finished_index, start, segment.index)
            };

            // Der vorgebaute Encoder sollte längst fertig sein — nur falls
            // nicht, wird hier gewartet bzw. neu gebaut.
            let next = match self.next_encoder.lock().take() {
                Some(rx) => rx.recv().unwrap_or_else(|err| Err(err.to_string())),
                None => build_encoder(&self.recording, &segment_path(&self.shared.dir, next_index))
                    .map_err(|e| e.to_string()),
            };
            match next {
                Ok(fresh) => {
                    // Beide Uhren des neuen Encoders fangen bei null an.
                    *self.encoder.lock() = Some(fresh);
                }
                Err(err) => *self.shared.error.lock() = Some(format!("Encoder-Neustart: {err}")),
            }
            *self.next_encoder.lock() = Some(spawn_encoder(
                self.recording.clone(),
                segment_path(&self.shared.dir, next_index + 1),
            ));

            let shared = self.shared.clone();
            std::thread::spawn(move || {
                if let Err(err) = encoder.finish() {
                    log::warn!("Segment konnte nicht abgeschlossen werden: {err}");
                    return;
                }
                let bytes = std::fs::metadata(&finished_path)
                    .map(|m| m.len())
                    .unwrap_or(0);
                shared.push_segment(Segment {
                    index: finished_index,
                    path: finished_path,
                    start_ms,
                    end_ms: now_ms,
                    bytes,
                });
                shared.prune();
            });
        }

        fn rotate_due(&self, now_ms: u64) -> bool {
            // Ein Segment ohne ein einziges Bild darf nicht abgeschlossen
            // werden. Windows.Graphics.Capture liefert nur bei Bildänderung
            // Frames; bei stehendem Bild entstünde eine TS-Datei ganz ohne
            // Videospur. Der concat-Demuxer verlangt in allen Dateien dasselbe
            // Stream-Layout und bricht daran ab — und zwar bei *jedem* weiteren
            // Clip, solange die Datei im Ring liegt. Das Segment wächst dann
            // eben über seine zehn Sekunden hinaus; es kostet nur den Ton.
            if self.shared.segment_frames.load(Ordering::Relaxed) == 0 {
                return false;
            }
            let age = now_ms.saturating_sub(self.segment.lock().start_ms);
            if self.shared.force_rotate.load(Ordering::Relaxed) {
                // Fällt der Tastendruck direkt hinter eine planmäßige Rotation,
                // wäre das Segment sonst nur Millisekunden lang.
                if age < MIN_SEGMENT_MS {
                    return false;
                }
                self.shared.force_rotate.store(false, Ordering::Relaxed);
                return true;
            }
            age >= SEGMENT_SECONDS * 1000
        }
    }

    /// Zustand der Pumpe. Liegt im Thread, damit die Puffer wiederverwendet
    /// werden und pro Durchlauf nichts allokiert wird.
    struct Pump {
        slot: Arc<EncoderSlot>,
        sources: Vec<AudioSource>,
        layout: TrackLayout,
        generation: u64,
        mix_buffers: Vec<Vec<f32>>,
        pcm: Vec<u8>,
        track_scratch: Vec<i16>,
    }

    impl Pump {
        fn new(slot: Arc<EncoderSlot>) -> Self {
            let sources = slot.shared.sources.lock().clone();
            let layout = TrackLayout::from_sources(&sources);
            let generation = slot.shared.sources_generation.load(Ordering::Relaxed);
            Self {
                slot,
                sources,
                layout,
                generation,
                mix_buffers: Vec::new(),
                pcm: Vec::new(),
                track_scratch: Vec::new(),
            }
        }

        fn refresh_sources(&mut self) {
            let generation = self.slot.shared.sources_generation.load(Ordering::Relaxed);
            if generation == self.generation {
                return;
            }
            self.generation = generation;
            self.sources = self.slot.shared.sources.lock().clone();
            self.layout = TrackLayout::from_sources(&self.sources);
        }

        /// So viel Ton nachschieben, wie seit dem letzten Mal vergangen ist.
        fn feed_audio(&mut self, now_ms: u64) {
            // Vor dem ersten Bild gibt es keine Zeitachse, an die sich der Ton
            // hängen könnte — was jetzt käme, läge später vor dem Bild.
            if !self.slot.video_started.load(Ordering::Acquire) {
                return;
            }
            let shared = self.slot.shared.clone();
            let since_video = now_ms.saturating_sub(self.slot.video_start_ms.load(Ordering::Relaxed));
            let target = since_video * SAMPLE_RATE as u64 / 1000;
            let sent = self.slot.audio_frames_sent.load(Ordering::Relaxed);
            let missing = target.saturating_sub(sent);
            if missing == 0 {
                return;
            }
            // Nicht mehr als 200 ms auf einmal nachschieben.
            let frames = missing.min(SAMPLE_RATE as u64 / 5) as usize;

            self.refresh_sources();

            // Rückstand begrenzen: Aufnahme- und Abholrate laufen auf
            // verschiedenen Uhren, ohne Bremse sammelt sich der Unterschied.
            shared
                .audio
                .trim_backlog(MAX_AUDIO_BACKLOG_MS * SAMPLE_RATE as usize * CHANNELS / 1000);
            shared
                .audio
                .mix_into(&self.sources, &self.layout, frames, &mut self.mix_buffers);

            let has_main = !self.layout.main_mix.is_empty();
            self.pcm.clear();
            self.pcm.reserve(frames * CHANNELS * 2);
            if has_main {
                for sample in &self.mix_buffers[0] {
                    self.pcm.extend_from_slice(&to_i16(*sample).to_le_bytes());
                }
            } else {
                self.pcm.resize(frames * CHANNELS * 2, 0);
            }

            // Separate Spuren in ihre Ringe schreiben.
            let offset = usize::from(has_main);
            {
                let rings = shared.tracks.lock();
                for (index, source_id) in self.layout.separate.iter().enumerate() {
                    let Some(track) = self.mix_buffers.get(offset + index) else {
                        continue;
                    };
                    let Some(ring) = rings.iter().find(|r| &r.source_id == source_id) else {
                        continue;
                    };
                    self.track_scratch.clear();
                    self.track_scratch.extend(track.iter().map(|s| to_i16(*s)));
                    ring.push(&self.track_scratch);
                }
            }

            let mut guard = self.slot.encoder.lock();
            let Some(encoder) = guard.as_mut() else {
                return;
            };
            if let Err(err) = encoder.send_audio_buffer(&self.pcm, 0) {
                *shared.error.lock() = Some(format!("Ton: {err}"));
                return;
            }
            drop(guard);
            self.slot
                .audio_frames_sent
                .fetch_add(frames as u64, Ordering::Relaxed);
        }

        fn run(mut self) {
            while self.slot.running.load(Ordering::Relaxed) {
                std::thread::sleep(PUMP_INTERVAL);
                let now_ms = self.slot.now_ms();
                // Die Uhr des Puffers hängt an der Pumpe, nicht an den Bildern.
                self.slot.shared.elapsed_ms.store(now_ms, Ordering::Relaxed);

                if self.slot.rotate_due(now_ms) {
                    self.slot.rotate(now_ms);
                }
                self.feed_audio(now_ms);
            }
        }
    }

    pub struct Handler {
        slot: Arc<EncoderSlot>,
    }

    impl GraphicsCaptureApiHandler for Handler {
        type Flags = Flags;
        type Error = Box<dyn std::error::Error + Send + Sync>;

        fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
            Ok(Self {
                slot: ctx.flags.slot,
            })
        }

        /// Nur noch Bilder — Ton, Rotation und Uhr laufen in der Pumpe.
        fn on_frame_arrived(
            &mut self,
            frame: &mut Frame,
            _control: InternalCaptureControl,
        ) -> Result<(), Self::Error> {
            self.slot.send_frame(frame);
            Ok(())
        }

        fn on_closed(&mut self) -> Result<(), Self::Error> {
            log::info!("Aufnahmequelle wurde geschlossen");
            Ok(())
        }
    }

    pub struct Control {
        capture: windows_capture::capture::CaptureControl<
            Handler,
            Box<dyn std::error::Error + Send + Sync>,
        >,
        slot: Arc<EncoderSlot>,
        pump: Option<std::thread::JoinHandle<()>>,
    }

    pub fn start(shared: Arc<Shared>, recording: RecordingConfig) -> Result<Control, String> {
        let slot = EncoderSlot::new(shared, recording.clone())?;
        let flags = Flags { slot: slot.clone() };

        let capture = match recording.target_kind {
            TargetKind::Monitor => {
                let monitor = match &recording.target_id {
                    Some(device) => match Monitor::enumerate()
                        .map_err(|e| e.to_string())?
                        .into_iter()
                        .find(|m| m.device_name().map(|n| &n == device).unwrap_or(false))
                    {
                        Some(monitor) => monitor,
                        // Abgestöpselter oder umbenannter Bildschirm: lieber den
                        // primären aufnehmen als gar nicht puffern.
                        None => {
                            log::warn!("Monitor '{device}' nicht gefunden — nehme den primären");
                            Monitor::primary().map_err(|e| e.to_string())?
                        }
                    },
                    None => Monitor::primary().map_err(|e| e.to_string())?,
                };
                let settings = Settings::new(
                    monitor,
                    CursorCaptureSettings::WithCursor,
                    DrawBorderSettings::WithoutBorder,
                    SecondaryWindowSettings::Default,
                    MinimumUpdateIntervalSettings::Default,
                    DirtyRegionSettings::Default,
                    ColorFormat::Bgra8,
                    flags,
                );
                Handler::start_free_threaded(settings).map_err(|e| e.to_string())?
            }
            TargetKind::Window => {
                let wanted = recording
                    .target_id
                    .clone()
                    .ok_or_else(|| "Kein Fenster ausgewählt".to_string())?;
                // Zuerst über das Handle: Der Titel ändert sich im Spiel
                // ständig, und `title()` schlägt bei manchen Fenstern ganz fehl
                // — dann darf das den Handle-Vergleich nicht mitreißen.
                let window = Window::enumerate()
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .find(|w| {
                        format!("0x{:X}", w.as_raw_hwnd() as usize) == wanted
                            || w.title().map(|t| t == wanted).unwrap_or(false)
                    })
                    .ok_or_else(|| {
                        "Das gewählte Fenster ist nicht mehr offen.".to_string()
                    })?;
                let settings = Settings::new(
                    window,
                    CursorCaptureSettings::WithCursor,
                    DrawBorderSettings::WithoutBorder,
                    SecondaryWindowSettings::Default,
                    MinimumUpdateIntervalSettings::Default,
                    DirtyRegionSettings::Default,
                    ColorFormat::Bgra8,
                    flags,
                );
                Handler::start_free_threaded(settings).map_err(|e| e.to_string())?
            }
        };

        let pump = {
            let slot = slot.clone();
            std::thread::Builder::new()
                .name("clippiboy-audio-pump".into())
                .spawn(move || Pump::new(slot).run())
                .map_err(|e| e.to_string())?
        };

        Ok(Control {
            capture,
            slot,
            pump: Some(pump),
        })
    }

    pub fn stop(mut control: Control) {
        control.slot.running.store(false, Ordering::Relaxed);
        if let Some(pump) = control.pump.take() {
            let _ = pump.join();
        }
        let _ = control.capture.stop();
        // Das laufende Segment sauber schließen, sonst bleibt eine
        // unvollständige TS-Datei liegen.
        if let Some(encoder) = control.slot.encoder.lock().take() {
            let _ = encoder.finish();
        }
    }
}

/// Lebt so lange, wie ein Clip geschrieben wird. Siehe `Shared::begin_save`.
pub struct SaveGuard(Arc<Shared>);

impl Drop for SaveGuard {
    fn drop(&mut self) {
        self.0.saves_in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Laufende Aufnahme.
pub struct Pipeline {
    pub shared: Arc<Shared>,
    #[cfg(windows)]
    control: Option<win::Control>,
}

impl Pipeline {
    pub fn start(
        recording: &RecordingConfig,
        buffer_seconds: u32,
        sources: Vec<AudioSource>,
        audio: Arc<AudioEngine>,
        dir: PathBuf,
    ) -> Result<Self, String> {
        // Alte Segmente einer früheren Sitzung wegräumen.
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.path().extension().map(|e| e == "ts").unwrap_or(false) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }

        let layout = TrackLayout::from_sources(&sources);
        let tracks: Vec<Arc<TrackRing>> = layout
            .separate
            .iter()
            .map(|id| {
                let label = sources
                    .iter()
                    .find(|s| &s.id == id)
                    .map(|s| s.label.clone())
                    .unwrap_or_else(|| id.clone());
                Arc::new(TrackRing::new(id.clone(), label, buffer_seconds))
            })
            .collect();

        // Ohne das steckt in jedem Ring noch der Ton der letzten Minuten, den
        // niemand abgeholt hat — der Clip liefe von der ersten Sekunde an
        // hinter dem Bild her.
        audio.reset_rings();

        let shared = Arc::new(Shared {
            dir,
            buffer_seconds,
            segments: Mutex::new(Vec::new()),
            tracks: Mutex::new(tracks),
            sources: Mutex::new(sources),
            audio,
            force_rotate: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            segment_frames: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            elapsed_ms: AtomicU64::new(0),
            error: Mutex::new(None),
            current_segment: AtomicU64::new(0),
            saves_in_flight: AtomicU64::new(0),
            sources_generation: AtomicU64::new(0),
        });

        #[cfg(windows)]
        {
            let control = win::start(shared.clone(), recording.clone())?;
            Ok(Self {
                shared,
                control: Some(control),
            })
        }

        #[cfg(not(windows))]
        {
            let _ = recording;
            let _ = shared;
            Err("Aufnahme ist nur unter Windows verfügbar".into())
        }
    }

    pub fn stop(&mut self) {
        #[cfg(windows)]
        if let Some(control) = self.control.take() {
            win::stop(control);
        }
        // Ein Speichern, das gerade läuft, liest noch aus diesen Dateien —
        // erst zu Ende kommen lassen, sonst bricht ffmpeg mittendrin ab.
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.shared.saves_in_flight.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }

        // Segmentdateien aufräumen — auch das gerade laufende, das noch nicht
        // in der Liste steht.
        self.shared.segments.lock().clear();
        if let Ok(entries) = std::fs::read_dir(&self.shared.dir) {
            for entry in entries.flatten() {
                if entry.path().extension().map(|e| e == "ts").unwrap_or(false) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    /// Schließt das laufende Segment ab und wartet, bis es im Ring liegt.
    pub fn seal_current_segment(&self, timeout: Duration) -> bool {
        self.shared.seal_current_segment(timeout)
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.stop();
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> Arc<Shared> {
        Arc::new(Shared {
            // Existiert nicht: `prune` löscht Segmentdateien, und der Test soll
            // dabei nichts Echtes erwischen.
            dir: PathBuf::from("clippiboy-test-nirgendwo"),
            buffer_seconds: 60,
            segments: Mutex::new(Vec::new()),
            tracks: Mutex::new(Vec::new()),
            sources: Mutex::new(Vec::new()),
            audio: Arc::new(AudioEngine::new()),
            force_rotate: AtomicBool::new(false),
            current_segment: AtomicU64::new(0),
            saves_in_flight: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            segment_frames: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            elapsed_ms: AtomicU64::new(0),
            error: Mutex::new(None),
            sources_generation: AtomicU64::new(0),
        })
    }

    fn segment(index: u64, start_ms: u64, end_ms: u64) -> Segment {
        Segment {
            index,
            path: PathBuf::from(format!("clippiboy-test-nirgendwo/segment_{index}.ts")),
            start_ms,
            end_ms,
            bytes: 1000,
        }
    }

    #[test]
    fn sealing_returns_at_once_when_the_segment_is_already_in_the_ring() {
        let shared = shared();
        shared.current_segment.store(3, Ordering::Relaxed);
        shared.segments.lock().push(segment(3, 0, 10_000));

        assert!(shared.seal_current_segment(Duration::from_millis(100)));
    }

    /// Bei stehendem Bild liefert Windows.Graphics.Capture keine Frames, das
    /// laufende Segment bleibt leer und lässt sich nicht abschließen. Zu holen
    /// gibt es dann aber auch nichts — das Speichern darf nicht sechs Sekunden
    /// warten und danach scheitern.
    #[test]
    fn a_frozen_picture_does_not_block_saving() {
        let shared = shared();
        shared.current_segment.store(2, Ordering::Relaxed);
        shared.segments.lock().push(segment(1, 0, 10_000));

        let started = Instant::now();
        assert!(shared.seal_current_segment(Duration::from_secs(6)));
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "Aufgeben hat {:?} gedauert",
            started.elapsed()
        );
        assert!(
            !shared.force_rotate.load(Ordering::Relaxed),
            "Der Rotationswunsch muss zurückgenommen sein — sonst rotiert das \
             nächste ankommende Bild sofort ein Segment von Millisekunden."
        );
    }

    /// Ohne ein einziges fertiges Segment gibt es wirklich nichts zu speichern.
    #[test]
    fn without_any_segment_sealing_gives_up() {
        let shared = shared();
        assert!(!shared.seal_current_segment(Duration::from_millis(1200)));
        assert!(!shared.force_rotate.load(Ordering::Relaxed));
    }

    /// `prune` läuft im Abschluss-Thread und löscht Segmentdateien — die liest
    /// ein gerade laufendes ffmpeg aber noch.
    #[test]
    fn a_running_save_keeps_its_segments() {
        let shared = shared();
        shared.elapsed_ms.store(200_000, Ordering::Relaxed);
        {
            let mut segments = shared.segments.lock();
            segments.push(segment(0, 0, 10_000));
            segments.push(segment(1, 190_000, 200_000));
        }

        let guard = shared.begin_save();
        shared.prune();
        assert_eq!(
            shared.segments.lock().len(),
            2,
            "Während des Speicherns darf kein Segment verschwinden"
        );

        drop(guard);
        shared.prune();
        assert_eq!(shared.segments.lock().len(), 1, "Danach schon");
    }
}
