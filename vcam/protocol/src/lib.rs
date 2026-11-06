//! The shared memory between ClippiBoy and its virtual camera.

//!
//! ClippiBoy (the producer, in the user's session) creates the section
//! `Local\ClippiBoy.VCam.v1`; the media source in Frame Server (session 0)
//! opens it read-only as `Session\<n>\ClippiBoy.VCam.v1`. A 4 KiB header, then
//! four slots of up to 1920×1080 NV12. Each slot has a seqlock: the writer makes
//! its sequence odd, writes, makes it even; the reader copies and keeps the copy
//! only if the sequence was even and did not move. A frame half-written is never
//! shown — the previous slot is.
//!
//! Versioned: another major version is refused, a newer minor one is read.

use std::sync::atomic::{fence, AtomicI64, AtomicU32, AtomicU64, Ordering};

pub const NAME: &str = "ClippiBoy.VCam.v1";
/// The media source's COM class, fixed for good: Windows keys the camera on it.
pub const CLSID: &str = "{23b5c234-8158-430a-b8f3-681b7e370ec6}";
/// The camera's name; Windows appends its own virtual-camera suffix.
pub const FRIENDLY_NAME: &str = "ClippiBoy Camera";
/// Where the installer notes what it installed.
/// The DLL version the setup carries; the app compares it with the installed one.
pub const DLL_VERSION: &str = "1.0.2";
pub const REGISTRY_KEY: &str = r"SOFTWARE\ClippiBoy\VirtualCamera";
pub const MAGIC: u32 = u32::from_le_bytes(*b"CBVC");
pub const MAJOR: u32 = 1;
pub const MINOR: u32 = 0;
pub const HEADER_BYTES: usize = 4096;
pub const SLOT_COUNT: usize = 4;
pub const SLOT_HEADER_BYTES: usize = 64;
pub const MAX_WIDTH: u32 = 1920;
pub const MAX_HEIGHT: u32 = 1080;
pub const SLOT_CAPACITY: usize = (MAX_WIDTH * MAX_HEIGHT * 3 / 2) as usize;
pub const SLOT_STRIDE: usize = (SLOT_HEADER_BYTES + SLOT_CAPACITY).div_ceil(4096) * 4096;
pub const TOTAL_BYTES: usize = HEADER_BYTES + SLOT_COUNT * SLOT_STRIDE;
/// A producer silent this long counts as gone.
pub const STALE_100NS: i64 = 10_000_000;

#[repr(C)]
pub struct Header {
    pub magic: AtomicU32,
    pub major: AtomicU32,
    pub minor: AtomicU32,
    pub slot_count: AtomicU32,
    pub slot_stride: AtomicU32,
    pub pid: AtomicU32,
    pub session: AtomicU32,
    pub state: AtomicU32,
    pub idle_reason: AtomicU32,
    _pad: u32,
    /// Counts the producer's starts.
    pub generation: AtomicU64,
    /// The producer's last sign of life, QPC in 100 ns.
    pub heartbeat: AtomicI64,
    /// The slot written last.
    pub latest: AtomicU64,
}

#[repr(C)]
pub struct SlotHeader {
    pub seq: AtomicU64,
    pub frame: AtomicU64,
    /// When the camera took it, QPC in 100 ns.
    pub qpc: AtomicI64,
    pub width: AtomicU32,
    pub height: AtomicU32,
    pub bytes: AtomicU32,
    _pad: [u32; 7],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum State {
    Init = 0,
    Idle = 1,
    Live = 2,
    Stopping = 3,
}

/// Why the producer has no picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum IdleReason {
    None = 0,
    CamOff = 1,
    NoCamera = 2,
    Busy = 3,
    Closing = 4,
}

impl IdleReason {
    fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::CamOff,
            2 => Self::NoCamera,
            3 => Self::Busy,
            4 => Self::Closing,
            _ => Self::None,
        }
    }
}

pub fn is_stale(heartbeat_100ns: i64, now_100ns: i64) -> bool {
    now_100ns - heartbeat_100ns > STALE_100NS
}

/// The region behind a mapped view; both sides share these accessors.
struct Region(*mut u8);

impl Region {
    fn header(&self) -> &Header {
        unsafe { &*(self.0 as *const Header) }
    }

    unsafe fn slot(&self, index: usize) -> &SlotHeader {
        &*(self.0.add(HEADER_BYTES + index * SLOT_STRIDE) as *const SlotHeader)
    }

    unsafe fn data(&self, index: usize) -> *mut u8 {
        self.0.add(HEADER_BYTES + index * SLOT_STRIDE + SLOT_HEADER_BYTES)
    }
}

/// The producer's side.
pub struct Writer(Region);

// The view is shared memory meant for exactly this; every field is atomic.
unsafe impl Send for Writer {}
unsafe impl Sync for Writer {}

impl Writer {
    /// # Safety
    /// `base` points at a writable, 8-byte aligned view of `len` bytes that
    /// outlives the writer.
    pub unsafe fn new(base: *mut u8, len: usize) -> Option<Self> {
        (!base.is_null() && len >= TOTAL_BYTES && base as usize % 8 == 0).then(|| Self(Region(base)))
    }

    pub fn init(&self, pid: u32, session: u32) {
        let h = self.0.header();
        h.state.store(State::Init as u32, Ordering::SeqCst);
        h.major.store(MAJOR, Ordering::SeqCst);
        h.minor.store(MINOR, Ordering::SeqCst);
        h.slot_count.store(SLOT_COUNT as u32, Ordering::SeqCst);
        h.slot_stride.store(SLOT_STRIDE as u32, Ordering::SeqCst);
        h.pid.store(pid, Ordering::SeqCst);
        h.session.store(session, Ordering::SeqCst);
        h.generation.fetch_add(1, Ordering::SeqCst);
        h.magic.store(MAGIC, Ordering::SeqCst);
    }

    /// One NV12 frame into the next slot.
    pub fn publish(&self, nv12: &[u8], width: u32, height: u32, qpc_100ns: i64, now_100ns: i64) {
        let bytes = (width as usize * height as usize * 3 / 2).min(nv12.len());
        if width > MAX_WIDTH || height > MAX_HEIGHT || bytes > SLOT_CAPACITY {
            return;
        }
        let h = self.0.header();
        let previous = h.latest.load(Ordering::Acquire) as usize % SLOT_COUNT;
        let next = (previous + 1) % SLOT_COUNT;
        unsafe {
            let slot = self.0.slot(next);
            let s = slot.seq.load(Ordering::Relaxed) & !1;
            slot.seq.store(s + 1, Ordering::Relaxed);
            fence(Ordering::Release);
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), self.0.data(next), bytes);
            slot.width.store(width, Ordering::Relaxed);
            slot.height.store(height, Ordering::Relaxed);
            slot.bytes.store(bytes as u32, Ordering::Relaxed);
            slot.qpc.store(qpc_100ns, Ordering::Relaxed);
            slot.frame.store(self.0.slot(previous).frame.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
            fence(Ordering::Release);
            slot.seq.store(s + 2, Ordering::Release);
        }
        h.latest.store(next as u64, Ordering::Release);
        h.heartbeat.store(now_100ns, Ordering::Release);
        h.state.store(State::Live as u32, Ordering::Release);
    }

    pub fn set_state(&self, state: State, reason: IdleReason, now_100ns: i64) {
        let h = self.0.header();
        h.idle_reason.store(reason as u32, Ordering::Release);
        h.heartbeat.store(now_100ns, Ordering::Release);
        h.state.store(state as u32, Ordering::Release);
    }

    pub fn heartbeat(&self, now_100ns: i64) {
        self.0.header().heartbeat.store(now_100ns, Ordering::Release);
    }

    #[doc(hidden)]
    pub fn latest_slot(&self) -> usize {
        self.0.header().latest.load(Ordering::Acquire) as usize % SLOT_COUNT
    }

    #[doc(hidden)]
    /// # Safety
    /// `index` < `SLOT_COUNT`.
    pub unsafe fn slot_header(&self, index: usize) -> &SlotHeader {
        self.0.slot(index)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadResult {
    Frame { width: u32, height: u32, qpc_100ns: i64 },
    Idle(IdleReason),
    NotRunning,
    Torn,
}

/// The media source's side; never writes.
pub struct Reader(Region);

unsafe impl Send for Reader {}
unsafe impl Sync for Reader {}

impl Reader {
    /// # Safety
    /// `base` points at a readable, 8-byte aligned view of `len` bytes that
    /// outlives the reader.
    pub unsafe fn new(base: *const u8, len: usize) -> Option<Self> {
        (!base.is_null() && len >= TOTAL_BYTES && base as usize % 8 == 0).then(|| Self(Region(base as *mut u8)))
    }

    /// The newest frame's stamp, without copying it — to tell whether a new
    /// one came. `None` before the writer published anything.
    pub fn latest_stamp(&self) -> Option<i64> {
        let h = self.0.header();
        if h.magic.load(Ordering::Acquire) != MAGIC || h.major.load(Ordering::Acquire) != MAJOR {
            return None;
        }
        let latest = h.latest.load(Ordering::Acquire) as usize;
        if latest >= SLOT_COUNT {
            return None;
        }
        let slot = unsafe { self.0.slot(latest) };
        let seq = slot.seq.load(Ordering::Acquire);
        (seq != 0 && seq % 2 == 0).then(|| slot.qpc.load(Ordering::Relaxed))
    }

    /// The newest whole frame into `out`, or why there is none.
    pub fn read_latest(&self, out: &mut Vec<u8>, now_100ns: i64) -> ReadResult {
        let h = self.0.header();
        if h.magic.load(Ordering::Acquire) != MAGIC || h.major.load(Ordering::Acquire) != MAJOR {
            return ReadResult::NotRunning;
        }
        let state = h.state.load(Ordering::Acquire);
        if state == State::Stopping as u32 || state == State::Init as u32 {
            return ReadResult::NotRunning;
        }
        if is_stale(h.heartbeat.load(Ordering::Acquire), now_100ns) {
            return ReadResult::NotRunning;
        }
        if state != State::Live as u32 {
            return ReadResult::Idle(IdleReason::from_u32(h.idle_reason.load(Ordering::Acquire)));
        }
        let latest = h.latest.load(Ordering::Acquire) as usize;
        if latest >= SLOT_COUNT {
            return ReadResult::Torn;
        }
        for index in [latest, (latest + SLOT_COUNT - 1) % SLOT_COUNT] {
            if let Some(read) = self.try_slot(index, out) {
                return read;
            }
        }
        ReadResult::Torn
    }

    fn try_slot(&self, index: usize, out: &mut Vec<u8>) -> Option<ReadResult> {
        let slot = unsafe { self.0.slot(index) };
        for _ in 0..3 {
            let s1 = slot.seq.load(Ordering::Acquire);
            if s1 & 1 == 1 {
                std::thread::yield_now();
                continue;
            }
            let (w, h, bytes) = (
                slot.width.load(Ordering::Relaxed),
                slot.height.load(Ordering::Relaxed),
                slot.bytes.load(Ordering::Relaxed) as usize,
            );
            let qpc = slot.qpc.load(Ordering::Relaxed);
            let valid = w > 0
                && h > 0
                && w % 2 == 0
                && h % 2 == 0
                && w <= MAX_WIDTH
                && h <= MAX_HEIGHT
                && bytes == (w * h * 3 / 2) as usize
                && bytes <= SLOT_CAPACITY;
            if !valid {
                return None;
            }
            out.resize(bytes, 0);
            unsafe { std::ptr::copy_nonoverlapping(self.0.data(index), out.as_mut_ptr(), bytes) };
            fence(Ordering::Acquire);
            if slot.seq.load(Ordering::Relaxed) == s1 {
                return Some(ReadResult::Frame { width: w, height: h, qpc_100ns: qpc });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// A region aligned like a mapped view.
    fn region() -> Vec<u64> {
        vec![0u64; TOTAL_BYTES / 8 + 1]
    }

    fn frame(w: u32, h: u32, fill: u8) -> Vec<u8> {
        vec![fill; (w * h * 3 / 2) as usize]
    }

    #[test]
    fn the_layout_matches_the_documented_offsets() {
        assert_eq!(std::mem::size_of::<Header>(), 64);
        assert_eq!(std::mem::size_of::<SlotHeader>(), 64);
        assert!(HEADER_BYTES >= std::mem::size_of::<Header>());
        assert_eq!(SLOT_STRIDE % 4096, 0);
        assert!(SLOT_STRIDE >= SLOT_HEADER_BYTES + SLOT_CAPACITY);
        assert_eq!(TOTAL_BYTES, HEADER_BYTES + SLOT_COUNT * SLOT_STRIDE);
        assert_eq!(SLOT_CAPACITY, 1920 * 1080 * 3 / 2);
    }

    /// The source asks this on every poll; copying 3 MB to learn whether a
    /// frame is new would cost a gigabyte a second.
    #[test]
    fn the_newest_stamp_is_known_without_copying_the_frame() {
        let mut mem = region();
        let base = mem.as_mut_ptr() as *mut u8;
        let writer = unsafe { Writer::new(base, TOTAL_BYTES) }.unwrap();
        let reader = unsafe { Reader::new(base, TOTAL_BYTES) }.unwrap();
        assert_eq!(reader.latest_stamp(), None, "before init");
        writer.init(7, 1);
        assert_eq!(reader.latest_stamp(), None, "nothing published yet");
        writer.publish(&frame(64, 36, 9), 64, 36, 1_000, 1_000);
        assert_eq!(reader.latest_stamp(), Some(1_000));
        writer.publish(&frame(64, 36, 3), 64, 36, 1_333, 1_333);
        assert_eq!(reader.latest_stamp(), Some(1_333));
    }

    #[test]
    fn a_frame_written_is_read_back() {
        let mut mem = region();
        let base = mem.as_mut_ptr() as *mut u8;
        let writer = unsafe { Writer::new(base, TOTAL_BYTES) }.unwrap();
        writer.init(7, 1);
        writer.publish(&frame(64, 36, 9), 64, 36, 1_000, 1_000);
        let reader = unsafe { Reader::new(base, TOTAL_BYTES) }.unwrap();
        let mut out = Vec::new();
        assert_eq!(reader.read_latest(&mut out, 1_000), ReadResult::Frame { width: 64, height: 36, qpc_100ns: 1_000 });
        assert_eq!(out, frame(64, 36, 9));
    }

    #[test]
    fn a_torn_slot_is_never_accepted() {
        let mut mem = region();
        let base = mem.as_mut_ptr() as usize;
        let writer = unsafe { Writer::new(base as *mut u8, TOTAL_BYTES) }.unwrap();
        writer.init(1, 1);
        let stop = Arc::new(AtomicBool::new(false));
        let hammer = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let writer = unsafe { Writer::new(base as *mut u8, TOTAL_BYTES) }.unwrap();
                let mut n = 0u8;
                while !stop.load(Ordering::Relaxed) {
                    n = n.wrapping_add(1);
                    writer.publish(&frame(320, 180, n), 320, 180, 5, 5);
                }
            })
        };
        let reader = unsafe { Reader::new(base as *mut u8, TOTAL_BYTES) }.unwrap();
        let mut out = Vec::new();
        // The writer's thread first has to start: on a busy machine all reads
        // were over before its first frame, and the test saw none.
        let started = std::time::Instant::now();
        while !matches!(reader.read_latest(&mut out, 5), ReadResult::Frame { .. }) {
            assert!(started.elapsed() < std::time::Duration::from_secs(5), "the writer never published");
            std::thread::yield_now();
        }
        let mut frames = 0;
        for _ in 0..20_000 {
            if let ReadResult::Frame { .. } = reader.read_latest(&mut out, 5) {
                let first = out[0];
                assert!(out.iter().all(|b| *b == first), "a frame mixed from two writes was accepted");
                frames += 1;
            }
        }
        stop.store(true, Ordering::Relaxed);
        hammer.join().unwrap();
        assert!(frames > 0);
    }

    #[test]
    fn a_reader_falls_back_to_the_previous_frame() {
        let mut mem = region();
        let base = mem.as_mut_ptr() as *mut u8;
        let writer = unsafe { Writer::new(base, TOTAL_BYTES) }.unwrap();
        writer.init(1, 1);
        writer.publish(&frame(64, 36, 1), 64, 36, 10, 10);
        writer.publish(&frame(64, 36, 2), 64, 36, 20, 20);
        // A write left half-way: the newest slot's sequence is odd.
        unsafe { writer.slot_header(writer.latest_slot()) }.seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let reader = unsafe { Reader::new(base, TOTAL_BYTES) }.unwrap();
        let mut out = Vec::new();
        assert_eq!(reader.read_latest(&mut out, 20), ReadResult::Frame { width: 64, height: 36, qpc_100ns: 10 });
        assert_eq!(out[0], 1);
    }

    #[test]
    fn another_major_version_is_refused() {
        let mut mem = region();
        let base = mem.as_mut_ptr() as *mut u8;
        let writer = unsafe { Writer::new(base, TOTAL_BYTES) }.unwrap();
        writer.init(1, 1);
        unsafe { &*(base as *const Header) }.major.store(MAJOR + 1, std::sync::atomic::Ordering::SeqCst);
        let reader = unsafe { Reader::new(base, TOTAL_BYTES) }.unwrap();
        assert_eq!(reader.read_latest(&mut Vec::new(), 0), ReadResult::NotRunning);
        // A newer minor version is read.
        let mut mem = region();
        let base = mem.as_mut_ptr() as *mut u8;
        let writer = unsafe { Writer::new(base, TOTAL_BYTES) }.unwrap();
        writer.init(1, 1);
        writer.publish(&frame(64, 36, 3), 64, 36, 1, 1);
        unsafe { &*(base as *const Header) }.minor.store(MINOR + 1, std::sync::atomic::Ordering::SeqCst);
        let reader = unsafe { Reader::new(base, TOTAL_BYTES) }.unwrap();
        assert!(matches!(reader.read_latest(&mut Vec::new(), 1), ReadResult::Frame { .. }));
    }

    #[test]
    fn a_stale_heartbeat_shows_not_running() {
        assert!(!is_stale(0, 9_999_999));
        assert!(is_stale(0, 10_000_001));
        let mut mem = region();
        let base = mem.as_mut_ptr() as *mut u8;
        let writer = unsafe { Writer::new(base, TOTAL_BYTES) }.unwrap();
        writer.init(1, 1);
        writer.publish(&frame(64, 36, 1), 64, 36, 0, 0);
        let reader = unsafe { Reader::new(base, TOTAL_BYTES) }.unwrap();
        assert_eq!(reader.read_latest(&mut Vec::new(), 20_000_000), ReadResult::NotRunning);
    }

    #[test]
    fn an_idle_producer_says_why() {
        let mut mem = region();
        let base = mem.as_mut_ptr() as *mut u8;
        let writer = unsafe { Writer::new(base, TOTAL_BYTES) }.unwrap();
        writer.init(1, 1);
        writer.set_state(State::Idle, IdleReason::CamOff, 100);
        let reader = unsafe { Reader::new(base, TOTAL_BYTES) }.unwrap();
        assert_eq!(reader.read_latest(&mut Vec::new(), 100), ReadResult::Idle(IdleReason::CamOff));
    }

    #[test]
    fn garbage_headers_never_read_out_of_bounds() {
        let mut mem = region();
        let base = mem.as_mut_ptr() as *mut u8;
        let writer = unsafe { Writer::new(base, TOTAL_BYTES) }.unwrap();
        writer.init(1, 1);
        writer.publish(&frame(64, 36, 1), 64, 36, 1, 1);
        let slot = unsafe { writer.slot_header(writer.latest_slot()) };
        for (w, h) in [(u32::MAX, 2), (4000, 4000), (0, 0), (63, 36)] {
            slot.width.store(w, std::sync::atomic::Ordering::SeqCst);
            slot.height.store(h, std::sync::atomic::Ordering::SeqCst);
            let reader = unsafe { Reader::new(base, TOTAL_BYTES) }.unwrap();
            let got = reader.read_latest(&mut Vec::new(), 1);
            assert!(!matches!(got, ReadResult::Frame { width, .. } if width == w), "{w}x{h}");
        }
        unsafe { &*(base as *const Header) }.latest.store(99, std::sync::atomic::Ordering::SeqCst);
        let reader = unsafe { Reader::new(base, TOTAL_BYTES) }.unwrap();
        let _ = reader.read_latest(&mut Vec::new(), 1);
        assert!(unsafe { Reader::new(base, 100) }.is_none());
    }
}
