//! The camera in recordings: written beside the recording while it runs, one
//! raw H.264 file per camera session, and burned in once it is stopped.
//!
//! Kept out of `recorder.rs` on purpose: nothing here can cost the recording.
//! The tap only queues (it runs under the camera ring's lock), a writer thread
//! of its own does the disk, and `cam.json` describes the segments from the
//! first one on, so a crashed recording keeps its camera too.

use serde::{Deserialize, Serialize};

/// One camera session within a recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub file: String,
    pub session: u64,
    /// Its first frame against the recording's first frame.
    pub offset_us: i64,
    pub frames: u64,
}

/// `cam.json`: what a recording's camera wrote.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CamFile {
    pub segments: Vec<Segment>,
}

/// Which segment each header and packet belongs to, and what `cam.json`
/// says. Pure: the writer thread only does the disk.
pub struct SegmentLog {
    start_100ns: i64,
    segments: Vec<Segment>,
    /// The current session and its origin.
    current: Option<(u64, i64)>,
    /// The current segment has had its first keyframe.
    keyed: bool,
}

impl SegmentLog {
    pub fn new(start_100ns: i64) -> Self {
        Self { start_100ns, segments: Vec::new(), current: None, keyed: false }
    }

    /// A header of `session`: a session not seen yet begins a segment, whose
    /// index comes back.
    pub fn header(&mut self, session: u64, origin_100ns: i64) -> Option<usize> {
        if self.current.is_some_and(|(s, _)| s == session) {
            return None;
        }
        self.current = Some((session, origin_100ns));
        self.keyed = false;
        let index = self.segments.len();
        self.segments.push(Segment { file: format!("cam.{index}.h264"), session, offset_us: 0, frames: 0 });
        Some(index)
    }

    /// A packet of the current session: the segment it goes into, or `None`
    /// before a header or before the segment's first keyframe.
    pub fn packet(&mut self, pts_us: i64, keyframe: bool) -> Option<usize> {
        let (_, origin) = self.current?;
        if !self.keyed && !keyframe {
            return None;
        }
        let index = self.segments.len() - 1;
        let segment = &mut self.segments[index];
        if !self.keyed {
            self.keyed = true;
            segment.offset_us = (origin + pts_us * 10 - self.start_100ns) / 10;
        }
        segment.frames += 1;
        Some(index)
    }

    /// Every segment begun — what a crash leaves to recover; counts may lag.
    pub fn file(&self) -> CamFile {
        CamFile { segments: self.segments.clone() }
    }

    /// At the end: the segments that got a frame, with their counts.
    pub fn final_file(&self) -> CamFile {
        CamFile { segments: self.segments.iter().filter(|s| s.frames > 0).cloned().collect() }
    }
}

enum Write {
    Begin(usize, Vec<u8>),
    Packet(usize, std::sync::Arc<[u8]>),
    Describe(CamFile),
    Close(std::sync::mpsc::Sender<CamFile>),
}

/// A recording's camera: a tap on the camera ring that queues for a writer of
/// its own.
pub struct RecordingCam {
    log: parking_lot::Mutex<SegmentLog>,
    queue: parking_lot::Mutex<Option<std::sync::mpsc::Sender<Write>>>,
}

impl RecordingCam {
    /// Writes into the recording's work folder `dir`.
    pub fn start(dir: &std::path::Path, start_100ns: i64) -> std::sync::Arc<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<Write>();
        let dir = dir.to_path_buf();
        let spawned = std::thread::Builder::new().name("clippiboy-reccam".into()).spawn(move || write_loop(&dir, rx));
        if let Err(err) = spawned {
            log::warn!("recording camera: no writer ({err}) — the recording stays without it");
        }
        std::sync::Arc::new(Self { log: parking_lot::Mutex::new(SegmentLog::new(start_100ns)), queue: parking_lot::Mutex::new(Some(tx)) })
    }

    fn send(&self, write: Write) {
        if let Some(tx) = self.queue.lock().as_ref() {
            let _ = tx.send(write);
        }
    }

    /// Stops taking packets, lets the writer finish, and says what it wrote.
    pub fn close(&self) -> CamFile {
        let last = self.log.lock().final_file();
        let Some(tx) = self.queue.lock().take() else { return last };
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let _ = tx.send(Write::Describe(last.clone()));
        let _ = tx.send(Write::Close(done_tx));
        drop(tx);
        done_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap_or(last)
    }
}

impl crate::camtrack::PacketTap for RecordingCam {
    fn cam(&self, packet: &crate::buffer::EncodedPacket) {
        let index = self.log.lock().packet(packet.pts_us, packet.keyframe);
        if let Some(index) = index {
            self.send(Write::Packet(index, packet.data.clone()));
        }
    }

    fn header(&self, session: u64, origin_100ns: i64, header: &[u8]) {
        let (index, file) = {
            let mut log = self.log.lock();
            (log.header(session, origin_100ns), log.file())
        };
        if let Some(index) = index {
            self.send(Write::Begin(index, header.to_vec()));
            self.send(Write::Describe(file));
        }
    }
}

/// The disk side: one open segment file at a time, `cam.json` rewritten
/// whenever the segments change.
fn write_loop(dir: &std::path::Path, rx: std::sync::mpsc::Receiver<Write>) {
    use std::io::Write as _;
    let mut open: Option<(usize, std::io::BufWriter<std::fs::File>)> = None;
    let mut broken = false;
    for write in rx {
        match write {
            Write::Begin(index, header) => {
                if let Some((_, mut file)) = open.take() {
                    let _ = file.flush();
                }
                match std::fs::File::create(dir.join(format!("cam.{index}.h264"))) {
                    Ok(file) => {
                        let mut file = std::io::BufWriter::new(file);
                        broken = file.write_all(&header).is_err();
                        open = Some((index, file));
                    }
                    Err(err) => {
                        log::warn!("recording camera: cam.{index}.h264: {err}");
                        broken = true;
                    }
                }
            }
            Write::Packet(index, data) => {
                if let Some((current, file)) = open.as_mut().filter(|(current, _)| *current == index && !broken) {
                    if file.write_all(&data).is_err() {
                        log::warn!("recording camera: writing cam.{current}.h264 failed");
                        broken = true;
                    }
                }
            }
            Write::Describe(file) => describe(dir, &file),
            Write::Close(done) => {
                if let Some((_, mut file)) = open.take() {
                    let _ = file.flush();
                }
                let file = read_cam_file(dir).unwrap_or_default();
                let _ = done.send(file);
                return;
            }
        }
    }
}

fn describe(dir: &std::path::Path, file: &CamFile) {
    let Ok(text) = serde_json::to_string_pretty(file) else { return };
    let temp = dir.join("cam.json.tmp");
    if std::fs::write(&temp, text).and_then(|_| std::fs::rename(&temp, dir.join("cam.json"))).is_err() {
        log::warn!("recording camera: cam.json not written");
    }
}

pub fn read_cam_file(dir: &std::path::Path) -> Option<CamFile> {
    serde_json::from_str(&std::fs::read_to_string(dir.join("cam.json")).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: i64 = 1_000_000_000;

    #[test]
    fn a_new_session_starts_a_new_segment() {
        let mut log = SegmentLog::new(START);
        assert_eq!(log.header(1, START), Some(0));
        assert_eq!(log.packet(0, true), Some(0));
        assert_eq!(log.packet(33_333, false), Some(0));
        // The same session's header again (a resend) is no new segment.
        assert_eq!(log.header(1, START), None);
        // A lock or a sleep: the camera comes back with a new session.
        assert_eq!(log.header(2, START + 600_000_000), Some(1));
        assert_eq!(log.packet(0, true), Some(1));
        let file = log.file();
        assert_eq!(file.segments.len(), 2);
        assert_eq!((file.segments[0].file.as_str(), file.segments[0].frames), ("cam.0.h264", 2));
        assert_eq!((file.segments[1].file.as_str(), file.segments[1].frames), ("cam.1.h264", 1));
        assert_eq!(file.segments[1].offset_us, 60_000_000);
    }

    #[test]
    fn segment_offsets_are_in_recording_time() {
        let mut log = SegmentLog::new(START);
        // The session began 1 s before the recording; its backlog starts 0.5 s in.
        log.header(1, START - 10_000_000);
        log.packet(500_000, true);
        assert_eq!(log.file().segments[0].offset_us, -500_000);
    }

    #[test]
    fn nothing_is_written_before_a_header_and_a_keyframe() {
        let mut log = SegmentLog::new(START);
        assert_eq!(log.packet(0, true), None, "no header yet");
        log.header(1, START);
        assert_eq!(log.packet(0, false), None, "a segment starts on a keyframe");
        assert_eq!(log.packet(33_333, true), Some(0));
        assert_eq!(log.file().segments[0].frames, 1);
    }

    /// Written as a segment begins, before its first frame: a crash right
    /// after must still find it.
    #[test]
    fn a_segment_is_described_from_its_start() {
        let mut log = SegmentLog::new(START);
        log.header(1, START);
        assert_eq!(log.file().segments.len(), 1);
        assert!(log.final_file().segments.is_empty(), "no frame, nothing to burn in");
    }

    #[test]
    fn cam_json_survives_a_reread() {
        let mut log = SegmentLog::new(START);
        log.header(3, START);
        log.packet(0, true);
        let file = log.file();
        let text = serde_json::to_string(&file).unwrap();
        assert_eq!(serde_json::from_str::<CamFile>(&text).unwrap(), file);
    }
}
