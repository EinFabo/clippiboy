//! Aufnahme-Pipeline: Bildschirm → NV12 → Hardware-Encoder → Paket-Ring.
//!
//! Aufgenommen wird mit **einem** durchlaufenden Encoder. Die fertigen
//! H.264-Pakete landen im Ringpuffer aus `buffer.rs`, der Ton parallel als PCM
//! in Ringen mit derselben Zeitachse. Beim Speichern wird nur noch gemuxt —
//! kein Neuencodieren, kein Zusammenkleben von Dateien.
//!
//! Vorher lief das anders: alle zehn Sekunden wurde ein neuer Encoder
//! aufgesetzt, in eine MPEG-TS-Datei geschrieben und beim Speichern per
//! `ffmpeg concat` zusammengesetzt. Jeder Wechsel kostete einen erzwungenen
//! Keyframe und die Zeit zwischen Tausch und erstem neuen Bild — der Encoder
//! stempelt sein erstes Bild immer auf null, diese Lücke fiel also still unter
//! den Tisch. Das war der regelmäßige Hitch, und es war der Grund für den
//! ganzen Aufwand um Segmentgrenzen, `+genpts` und das nicht funktionierende
//! `-ss`.
//!
//! Bild und Ton hängen jetzt beide am QPC: Windows.Graphics.Capture stempelt
//! seine Bilder mit `SystemRelativeTime`, WASAPI seine Blöcke mit
//! `pu64QPCPosition` — dieselbe Uhr. Damit ist Synchronität keine Rechnung
//! mehr, sondern ergibt sich.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use crate::audio::capture::{now_100ns, CHANNELS, SAMPLE_RATE};
use crate::audio::engine::AudioEngine;
use crate::audio::TrackLayout;
use crate::buffer::{EncodedPacket, ReplayBuffer};
use crate::model::{AudioSource, EncoderId, RecordingConfig};

/// Wie weit der Mischer hinter der Gegenwart bleibt.
///
/// WASAPI liefert einen Block erst, wenn er voll ist; bis er im Ring liegt,
/// vergehen ein paar Millisekunden. Wer näher an der Gegenwart mischt, holt
/// sich Stille, die später durch echten Ton hätte ersetzt werden müssen —
/// und der Platz ist dann schon vergeben.
const AUDIO_LAG_100NS: i64 = 80 * 10_000;

/// Takt des Mischers.
const MIX_INTERVAL: Duration = Duration::from_millis(10);

/// Quellenkennung der Hauptmix-Spur. Sie gehört keiner Quelle und kollidiert
/// deshalb mit keiner Kennung aus der Konfiguration.
const MAIN_TRACK_ID: &str = "__mix";

/// Ringpuffer einer Tonspur (16-bit PCM), auf derselben QPC-Zeitachse wie das
/// Bild.
pub struct TrackRing {
    pub source_id: String,
    /// Änderbar: Wer eine Quelle umbenennt, darf dadurch nicht ihre Spur
    /// verlieren — der Ring bleibt derselbe, nur die Aufschrift wechselt.
    label: Mutex<String>,
    inner: Mutex<TrackInner>,
    capacity: usize,
}

struct TrackInner {
    samples: VecDeque<i16>,
    /// QPC des ersten Samples im Ring.
    start_100ns: i64,
    primed: bool,
}

impl TrackRing {
    fn new(source_id: String, label: String, seconds: u32) -> Self {
        Self {
            source_id,
            label: Mutex::new(label),
            inner: Mutex::new(TrackInner {
                samples: VecDeque::new(),
                start_100ns: 0,
                primed: false,
            }),
            // Ein Puffer über die volle Cliplänge, plus etwas Luft.
            capacity: (seconds as usize + 2) * SAMPLE_RATE as usize * CHANNELS,
        }
    }

    pub fn label(&self) -> String {
        self.label.lock().clone()
    }

    fn set_label(&self, label: String) {
        *self.label.lock() = label;
    }

    /// Der Mischer erzeugt lückenlos fortlaufende Fenster; `at_100ns` setzt
    /// deshalb nur beim allerersten Block die Zeitachse.
    fn push(&self, block: &[i16], at_100ns: i64) {
        let mut inner = self.inner.lock();
        if !inner.primed {
            inner.start_100ns = at_100ns;
            inner.primed = true;
        }
        inner.samples.extend(block.iter().copied());
        if inner.samples.len() > self.capacity {
            let excess = inner.samples.len() - self.capacity;
            inner.samples.drain(..excess);
            let frames = (excess / CHANNELS) as i64;
            inner.start_100ns += frames * 10_000_000 / SAMPLE_RATE as i64;
        }
    }

    /// Das Zeitfenster ab `from_100ns` über `frames` Frames als WAV schreiben.
    ///
    /// Fehlende Stellen werden zu Stille — das hält die Spur genau so lang wie
    /// das Bild, auch wenn eine Quelle erst später dazukam.
    pub fn write_wav_window(
        &self,
        path: &Path,
        from_100ns: i64,
        frames: usize,
    ) -> std::io::Result<()> {
        let wanted = frames * CHANNELS;
        let mut data = vec![0i16; wanted];
        {
            let inner = self.inner.lock();
            if inner.primed {
                let offset_frames =
                    (from_100ns - inner.start_100ns) * SAMPLE_RATE as i64 / 10_000_000;
                let offset = offset_frames * CHANNELS as i64;
                let (mut src, mut dst) = if offset < 0 {
                    (0usize, (-offset) as usize)
                } else {
                    (offset as usize, 0usize)
                };
                while dst < wanted {
                    let Some(sample) = inner.samples.get(src) else {
                        break;
                    };
                    data[dst] = *sample;
                    src += 1;
                    dst += 1;
                }
            }
        }

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

/// Von Aufnahme, Encoder und Mischer gemeinsam genutzter Zustand.
pub struct Shared {
    pub buffer_seconds: u32,
    pub fps: u32,
    /// Die encodeten Videopakete.
    pub packets: Mutex<ReplayBuffer>,
    /// Hauptmix (Index 0) und die Quellen mit eigener Spur.
    pub tracks: Mutex<Vec<Arc<TrackRing>>>,
    pub sources: Mutex<Vec<AudioSource>>,
    pub audio: Arc<AudioEngine>,
    /// QPC, auf den PTS 0 des Videostroms fällt. Damit lässt sich zu jedem
    /// Paket der echte Aufnahmezeitpunkt bestimmen — und der Ton an genau
    /// derselben Stelle schneiden.
    pub base_100ns: AtomicI64,
    pub anchored: AtomicBool,
    pub frames: AtomicU64,
    pub dropped: AtomicU64,
    /// Bilder, die der Taktgeber wiederholen musste, weil das Bild stillstand.
    pub duplicated: AtomicU64,
    pub error: Mutex<Option<String>>,
    /// Wurde der erste Fehler dieses Laufs schon gemeldet? Ein Encoder, der
    /// dauernd stolpert, soll die Oberfläche nicht zumüllen — aber *einmal*
    /// muss es jemand erfahren, sonst puffert die App scheinbar weiter und
    /// erst der Tastendruck bringt es ans Licht.
    pub error_seen: AtomicBool,
    /// Wird hochgezählt, wenn sich `sources` ändert. Der Mischer hält eine
    /// Kopie und liest nur nach, wenn sich die Zahl bewegt hat.
    pub sources_generation: AtomicU64,
    /// Welcher Encoder wirklich läuft — nicht der gewünschte.
    pub encoder: Mutex<Option<EncoderId>>,
    /// SPS/PPS, die beim Speichern vor den Elementarstrom gehören.
    pub sequence_header: Mutex<Vec<u8>>,
}

impl Shared {
    pub fn buffered_seconds(&self) -> f32 {
        self.packets.lock().buffered_seconds()
    }

    pub fn buffer_bytes(&self) -> u64 {
        self.packets.lock().bytes()
    }

    /// Neue Quellenliste übernehmen und den Mischer darüber informieren.
    pub fn set_sources(&self, sources: Vec<AudioSource>) {
        *self.sources.lock() = sources;
        self.sources_generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Ein Problem der laufenden Aufnahme festhalten.
    pub fn report(&self, message: String) {
        log::warn!("Aufnahme: {message}");
        *self.error.lock() = Some(message);
    }

    /// Den ersten noch ungemeldeten Fehler abholen.
    pub fn take_unseen_error(&self) -> Option<String> {
        if self.error_seen.swap(true, Ordering::SeqCst) {
            return None;
        }
        self.error.lock().clone()
    }

    /// QPC-Zeitpunkt zu einem Paket-Zeitstempel.
    pub fn qpc_of(&self, pts_us: i64) -> i64 {
        self.base_100ns.load(Ordering::Acquire) + pts_us * 10
    }
}

/// Alles, was zum Schreiben eines Clips gebraucht wird — in einem Rutsch
/// abgegriffen, damit die Sperren nicht über den ganzen ffmpeg-Lauf gehalten
/// werden.
pub struct ClipSnapshot {
    pub packets: Vec<EncodedPacket>,
    pub tracks: Vec<Arc<TrackRing>>,
    pub sequence_header: Vec<u8>,
    /// QPC des ersten Bildes im Clip — der Ton wird auf genau diesen Wert
    /// geschnitten.
    pub start_100ns: i64,
    pub audio_frames: usize,
    pub fps: u32,
}

/// Laufende Aufnahme.
pub struct Pipeline {
    pub shared: Arc<Shared>,
    #[cfg(windows)]
    inner: Option<win::Running>,
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

    use crate::convert::{pace, Converter, FrameSink, Latest};
    use crate::gpu::GpuDevice;
    use crate::mft::{EncoderSettings, VideoEncoder};
    use crate::wgc::{self, Capture};

    pub struct Running {
        /// Muss vor dem Encoder fallen: Erst darf kein Bild mehr kommen.
        _capture: Capture,
        latest: Arc<Latest>,
        pacer: Option<std::thread::JoinHandle<()>>,
        mixer_stop: Arc<AtomicBool>,
        mixer: Option<std::thread::JoinHandle<()>>,
        encoder: Option<Arc<VideoEncoder>>,
    }

    /// Reicht die getakteten Bilder an den Encoder weiter.
    struct EncoderSink {
        encoder: Arc<VideoEncoder>,
        shared: Arc<Shared>,
        latest: Arc<Latest>,
    }

    impl FrameSink for EncoderSink {
        fn on_frame(&mut self, texture: &ID3D11Texture2D, pts_100ns: i64, duplicate: bool) {
            // Beim allerersten Bild die Ausgabezeitachse an der echten
            // Aufnahmezeit verankern. Ohne das wüsste beim Speichern niemand,
            // welcher Tonabschnitt zu welchem Bild gehört.
            if !self.shared.anchored.load(Ordering::Acquire) {
                let qpc = self.latest.frame_qpc();
                self.shared
                    .base_100ns
                    .store(qpc - pts_100ns, Ordering::Release);
                self.shared.anchored.store(true, Ordering::Release);
            }
            if self.encoder.submit(texture, pts_100ns) {
                self.shared.frames.fetch_add(1, Ordering::Relaxed);
            } else {
                self.shared.dropped.fetch_add(1, Ordering::Relaxed);
            }
            if duplicate {
                self.shared.duplicated.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn start(
        shared: Arc<Shared>,
        recording: &RecordingConfig,
    ) -> Result<Running, String> {
        let gpu = Arc::new(GpuDevice::new()?);
        let converter = Converter::new(&gpu, recording.width, recording.height, recording.fps)?;
        let latest = Latest::new(converter);

        let encoder = {
            let shared = shared.clone();
            Arc::new(VideoEncoder::start(
                gpu.clone(),
                EncoderSettings {
                    width: recording.width,
                    height: recording.height,
                    fps: recording.fps,
                    bitrate_kbps: recording.bitrate_kbps,
                    keyframe_seconds: recording.keyframe_seconds,
                    requested: recording.encoder,
                },
                move |packet| shared.packets.lock().push(packet),
            )?)
        };
        *shared.encoder.lock() = Some(encoder.chosen);
        *shared.sequence_header.lock() = encoder.sequence_header.clone();

        let capture = {
            let latest = latest.clone();
            let closed_shared = shared.clone();
            let shared = shared.clone();
            wgc::start(
                &gpu,
                recording.target_kind,
                recording.target_id.as_deref(),
                recording.fps,
                move |frame| {
                    if let Err(err) = latest.submit(frame.texture, frame.qpc_100ns) {
                        shared.dropped.fetch_add(1, Ordering::Relaxed);
                        shared.report(err);
                    }
                },
                {
                    let shared = closed_shared.clone();
                    move || {
                        shared.report(
                            "Die Aufnahmequelle ist verschwunden — es kommt kein Bild mehr."
                                .into(),
                        );
                    }
                },
            )?
        };

        let pacer = {
            let latest = latest.clone();
            let sink = Box::new(EncoderSink {
                encoder: encoder.clone(),
                shared: shared.clone(),
                latest: latest.clone(),
            });
            let fps = recording.fps;
            std::thread::Builder::new()
                .name("clippiboy-pacer".into())
                .spawn(move || pace(latest, fps, sink))
                .map_err(|err| format!("Taktgeber: {err}"))?
        };

        let mixer_stop = Arc::new(AtomicBool::new(false));
        let mixer = {
            let shared = shared.clone();
            let stop = mixer_stop.clone();
            std::thread::Builder::new()
                .name("clippiboy-mixer".into())
                .spawn(move || super::mix_loop(shared, stop))
                .map_err(|err| format!("Mischer: {err}"))?
        };

        Ok(Running {
            _capture: capture,
            latest,
            pacer: Some(pacer),
            mixer_stop,
            mixer: Some(mixer),
            encoder: Some(encoder),
        })
    }

    pub fn stop(mut running: Running) {
        // Reihenfolge: erst keine neuen Bilder mehr takten, dann den Encoder
        // auslaufen lassen. Umgekehrt liefe der Taktgeber ins Leere.
        running.latest.stop();
        if let Some(pacer) = running.pacer.take() {
            let _ = pacer.join();
        }
        running.mixer_stop.store(true, Ordering::Relaxed);
        if let Some(mixer) = running.mixer.take() {
            let _ = mixer.join();
        }
        if let Some(encoder) = running.encoder.take() {
            // Der Sink hält den zweiten Verweis; der ist mit dem Taktgeber
            // gefallen, hier fällt der letzte und der Encoder läuft aus.
            drop(encoder);
        }
    }
}

/// Der Mischer: erzeugt lückenlos fortlaufende Tonfenster auf der QPC-Achse.
fn mix_loop(shared: Arc<Shared>, stop: Arc<AtomicBool>) {
    let mut sources = shared.sources.lock().clone();
    let mut layout = TrackLayout::from_sources(&sources);
    let mut generation = shared.sources_generation.load(Ordering::Relaxed);

    let mut buffers: Vec<Vec<f32>> = Vec::new();
    let mut scratch: Vec<i16> = Vec::new();
    // Gegen die Uhr gerechnet, nicht aufsummiert: So kann sich kein Fehler
    // ansammeln, egal wie ungenau der Faden aufwacht.
    let mut next_100ns: Option<i64> = None;

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(MIX_INTERVAL);

        let current = shared.sources_generation.load(Ordering::Relaxed);
        if current != generation {
            generation = current;
            sources = shared.sources.lock().clone();
            layout = TrackLayout::from_sources(&sources);
            sync_tracks(&shared, &sources);
        }

        let target = now_100ns() - AUDIO_LAG_100NS;
        let from = *next_100ns.get_or_insert(target);
        if target <= from {
            continue;
        }
        let frames = ((target - from) * SAMPLE_RATE as i64 / 10_000_000) as usize;
        if frames == 0 {
            continue;
        }

        shared
            .audio
            .mix_window(&sources, &layout, from, frames, &mut buffers);

        let rings = shared.tracks.lock();
        let has_main = !layout.main_mix.is_empty();
        // Spur 0 ist immer der Hauptmix — auch wenn gerade keine Quelle darauf
        // liegt. Sonst hätte der Clip mal eine Tonspur mehr, mal eine weniger,
        // je nachdem was beim Speichern eingeschaltet war.
        if let Some(main) = rings.first() {
            scratch.clear();
            match (has_main, buffers.first()) {
                (true, Some(mix)) => scratch.extend(mix.iter().map(|s| to_i16(*s))),
                _ => scratch.resize(frames * CHANNELS, 0),
            }
            main.push(&scratch, from);
        }

        // Jeder Ring bekommt in jedem Fenster etwas — wer gerade hörbar ist
        // seinen Mix, wer stumm oder ausgesolot ist Stille. Nichts zu schieben
        // wäre falsch: Der Ring stempelt nur seinen allerersten Block, eine
        // Lücke zöge deshalb alles Spätere nach vorn und der Ton liefe ab da
        // vor dem Bild her.
        let offset = usize::from(has_main);
        for ring in rings.iter().skip(1) {
            let slot = layout
                .separate
                .iter()
                .position(|id| id == &ring.source_id);
            scratch.clear();
            match slot.and_then(|index| buffers.get(offset + index)) {
                Some(track) => scratch.extend(track.iter().map(|s| to_i16(*s))),
                None => scratch.resize(frames * CHANNELS, 0),
            }
            ring.push(&scratch, from);
        }
        drop(rings);

        next_100ns = Some(from + frames as i64 * 10_000_000 / SAMPLE_RATE as i64);
    }
}

/// Die Spurenliste zu einer Quellenliste — Spur 0 (Hauptmix) und dahinter je
/// eine Spur pro Quelle mit eigener Tonspur.
///
/// Ringe, die es schon gibt, werden **weitergereicht**. Sie neu anzulegen wäre
/// der bequeme Weg, kostet aber den gesamten gepufferten Ton dieser Quelle —
/// und hier kommt jede Änderung an den Quellen vorbei, auch jeder einzelne
/// Schritt eines Lautstärkereglers. Vorher hieß das: Wer während der Aufnahme
/// am Mikrofon dreht, hat im Clip keins mehr.
///
/// Bewusst **nicht** nach `muted`/`solo` gefiltert. Eine stumme Quelle behält
/// ihre Spur, der Mischer schiebt so lange Stille hinein; sonst verschwände sie
/// mit dem Stummschalten auch rückwirkend aus dem Puffer.
fn tracks_for(
    sources: &[AudioSource],
    existing: &[Arc<TrackRing>],
    buffer_seconds: u32,
) -> Vec<Arc<TrackRing>> {
    let main = existing.first().cloned().unwrap_or_else(|| {
        Arc::new(TrackRing::new(
            MAIN_TRACK_ID.into(),
            "Mix".into(),
            buffer_seconds,
        ))
    });

    let mut out = vec![main];
    for source in sources.iter().filter(|s| s.enabled && s.separate_track) {
        match existing.iter().find(|ring| ring.source_id == source.id) {
            Some(ring) => {
                ring.set_label(source.label.clone());
                out.push(ring.clone());
            }
            None => out.push(Arc::new(TrackRing::new(
                source.id.clone(),
                source.label.clone(),
                buffer_seconds,
            ))),
        }
    }
    out
}

/// Die Spurenliste an eine geänderte Konfiguration angleichen.
fn sync_tracks(shared: &Arc<Shared>, sources: &[AudioSource]) {
    let mut rings = shared.tracks.lock();
    let next = tracks_for(sources, &rings, shared.buffer_seconds);
    *rings = next;
}

fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0) as i16
}

impl Pipeline {
    pub fn start(
        recording: &RecordingConfig,
        buffer_seconds: u32,
        sources: Vec<AudioSource>,
        audio: Arc<AudioEngine>,
    ) -> Result<Self, String> {
        // Spur 0 ist der Hauptmix und immer vorhanden.
        let tracks = tracks_for(&sources, &[], buffer_seconds);

        // Ohne das steckt in jedem Ring noch der Ton der letzten Minuten, den
        // niemand abgeholt hat.
        audio.reset_rings();

        let shared = Arc::new(Shared {
            buffer_seconds,
            fps: recording.fps.max(1),
            packets: Mutex::new(ReplayBuffer::new(buffer_seconds)),
            tracks: Mutex::new(tracks),
            sources: Mutex::new(sources),
            audio,
            base_100ns: AtomicI64::new(0),
            anchored: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            duplicated: AtomicU64::new(0),
            error: Mutex::new(None),
            error_seen: AtomicBool::new(false),
            sources_generation: AtomicU64::new(0),
            encoder: Mutex::new(None),
            sequence_header: Mutex::new(Vec::new()),
        });

        #[cfg(windows)]
        {
            let inner = win::start(shared.clone(), recording)?;
            Ok(Self {
                shared,
                inner: Some(inner),
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
        if let Some(inner) = self.inner.take() {
            win::stop(inner);
        }
        self.shared.packets.lock().clear();
    }

    /// Die letzten `seconds` Sekunden abgreifen.
    ///
    /// Der Schnitt sitzt auf dem letzten Keyframe **vor** dem gewünschten
    /// Startzeitpunkt — sonst wäre der Anfang nicht dekodierbar. Der Ton wird
    /// auf genau denselben QPC geschnitten; daher braucht es keine Korrektur
    /// mehr, damit Bild und Ton zusammenpassen.
    pub fn snapshot(&self, seconds: u32) -> Result<ClipSnapshot, String> {
        let packets = self.shared.packets.lock().snapshot(seconds);
        if packets.is_empty() {
            return Err("Der Replay-Puffer ist noch leer.".into());
        }

        let first_pts = packets.first().map(|p| p.pts_us).unwrap_or(0);
        let last = packets.last().map(|p| p.pts_us).unwrap_or(first_pts);
        let frame_us = 1_000_000 / self.shared.fps as i64;
        let span_us = (last - first_pts + frame_us).max(0);

        Ok(ClipSnapshot {
            tracks: self.shared.tracks.lock().clone(),
            sequence_header: self.shared.sequence_header.lock().clone(),
            start_100ns: self.shared.qpc_of(first_pts),
            audio_frames: (span_us * SAMPLE_RATE as i64 / 1_000_000) as usize,
            fps: self.shared.fps,
            packets,
        })
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

    /// QPC (100 ns) für eine Frame-Position.
    ///
    /// Nur Vielfache von 6 Frames sind bei 48 kHz exakt in 100 ns darstellbar
    /// (6 Frames = 1250 Ticks). Die Tests unten rechnen deshalb in solchen
    /// Schritten — sonst prüften sie die Rundung statt der Fensterlogik.
    const STEP: i64 = 6;

    fn qpc_of_frame(frame: i64) -> i64 {
        assert_eq!(frame % STEP, 0, "nur exakt darstellbare Abstände testen");
        frame * 10_000_000 / SAMPLE_RATE as i64
    }

    #[test]
    fn a_track_window_lands_where_its_timestamp_says() {
        let ring = TrackRing::new("a".into(), "A".into(), 5);
        // Sechs Frames stereo, aufsteigend, damit die Stelle erkennbar ist.
        let block: Vec<i16> = (1..=(STEP as i16 * CHANNELS as i16)).collect();
        ring.push(&block, qpc_of_frame(0));
        ring.push(&[77, 88], qpc_of_frame(STEP));

        let dir = std::env::temp_dir().join("clippiboy-test-track");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("window.wav");
        ring.write_wav_window(&path, qpc_of_frame(STEP), 1).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            &bytes[44..],
            &77i16.to_le_bytes()[..].iter().chain(88i16.to_le_bytes().iter())
                .copied().collect::<Vec<u8>>()[..],
            "Fenster sitzt an der falschen Stelle"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// Ein Fenster vor dem ersten Ton muss Stille liefern, nicht den Anfang —
    /// sonst rutschte der Ton beim Speichern nach vorn.
    #[test]
    fn a_window_before_the_audio_is_silence() {
        let ring = TrackRing::new("a".into(), "A".into(), 5);
        ring.push(&[7, 7], qpc_of_frame(STEP * 20));

        let dir = std::env::temp_dir().join("clippiboy-test-track");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("before.wav");
        ring.write_wav_window(&path, qpc_of_frame(0), 1).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[44..], &[0, 0, 0, 0]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_oldest_audio_is_dropped_and_the_start_moves_with_it() {
        // 1 s Kapazität + 2 s Luft = 3 s.
        let ring = TrackRing::new("a".into(), "A".into(), 1);
        let block = vec![0i16; SAMPLE_RATE as usize * CHANNELS];
        for second in 0..5 {
            ring.push(&block, qpc_of_frame(second * SAMPLE_RATE as i64));
        }
        // 48000 ist durch 6 teilbar — die Sekundenschritte sind also exakt.
        let inner = ring.inner.lock();
        assert_eq!(inner.samples.len(), ring.capacity);
        assert!(
            inner.start_100ns > 0,
            "Der Ringanfang muss mitwandern, sonst zeigt jedes Fenster daneben"
        );
    }

    fn source(id: &str, label: &str, separate: bool) -> AudioSource {
        AudioSource {
            id: id.into(),
            label: label.into(),
            kind: crate::model::SourceKind::InputDevice {
                device_id: "d".into(),
            },
            enabled: true,
            gain_db: 0.0,
            muted: false,
            solo: false,
            separate_track: separate,
        }
    }

    /// Der eigentliche Fehler hinter „mein Mikrofon ist im Clip stumm": Jede
    /// Reglerbewegung während der Aufnahme lief hier vorbei und legte die
    /// Einzelspuren neu an — mitsamt Verlust des gesamten gepufferten Tons.
    #[test]
    fn a_changed_source_keeps_its_ring_and_its_audio() {
        let mut sources = vec![source("mic", "Mikrofon", true)];
        let first = tracks_for(&sources, &[], 5);
        first[1].push(&[123, 456], qpc_of_frame(0));

        // Lauter gedreht und umbenannt — beides darf die Spur nicht kosten.
        sources[0].gain_db = 6.0;
        sources[0].label = "Mein Mikro".into();
        let second = tracks_for(&sources, &first, 5);

        assert!(Arc::ptr_eq(&first[0], &second[0]), "Hauptmix neu angelegt");
        assert!(Arc::ptr_eq(&first[1], &second[1]), "Mikrofonspur neu angelegt");
        assert_eq!(second[1].label(), "Mein Mikro", "Name nicht übernommen");
        assert_eq!(second[1].inner.lock().samples.len(), 2, "Ton verloren");
    }

    /// Stummschalten heißt „ab hier Stille", nicht „die Spur gab es nie".
    #[test]
    fn a_muted_source_keeps_its_track() {
        let mut sources = vec![source("mic", "Mikrofon", true)];
        let first = tracks_for(&sources, &[], 5);
        sources[0].muted = true;
        let second = tracks_for(&sources, &first, 5);

        assert_eq!(second.len(), 2);
        assert!(Arc::ptr_eq(&first[1], &second[1]));
    }

    /// Quellen ohne eigene Spur laufen in den Hauptmix und bekommen keinen Ring.
    #[test]
    fn only_separate_sources_get_a_track() {
        let sources = vec![
            source("spiel", "Spiel", false),
            source("mic", "Mikrofon", true),
        ];
        let tracks = tracks_for(&sources, &[], 5);
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].source_id, MAIN_TRACK_ID);
        assert_eq!(tracks[1].source_id, "mic");
    }

    /// Eine entfernte Quelle verschwindet, eine neue kommt leer dazu.
    #[test]
    fn removed_sources_drop_out_and_new_ones_start_empty() {
        let first = tracks_for(&[source("mic", "Mikrofon", true)], &[], 5);
        let second = tracks_for(&[source("discord", "Discord", true)], &first, 5);

        assert_eq!(second.len(), 2);
        assert_eq!(second[1].source_id, "discord");
        assert!(!second[1].inner.lock().primed);
    }
}
