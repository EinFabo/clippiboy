//! ClippiBoy Camera: a Frame Server media source. Frames come from ClippiBoy
//! through shared memory (`vcam-protocol`); without them, a quiet placeholder.
//! Frame Server loads this as LocalService in session 0, so a panic or a bad
//! pointer here takes every camera on the machine down: every COM method is
//! guarded, nothing on that path unwraps.

#![allow(non_snake_case)]


use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::null_mut;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::core::{implement, ComObject, Error, IUnknown, Interface, Result, GUID, HRESULT, PCWSTR, PROPVARIANT, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, BOOL, CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_INVALIDARG, E_POINTER, E_UNEXPECTED,
    ERROR_SET_NOT_FOUND, S_FALSE, S_OK,
};
use windows::Win32::Media::KernelStreaming::{IKsControl, IKsControl_Impl, KSIDENTIFIER, PINNAME_VIDEO_CAPTURE};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::Win32::System::Memory::{MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS};
use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId};
use windows::Win32::System::Threading::GetCurrentProcessId;

/// Fixed for good: Windows keys the camera on it.
pub const CLSID: GUID = GUID::from_u128(0x23b5c234_8158_430a_b8f3_681b7e370ec6);
const CLIENT_PID: GUID = GUID::from_u128(0x5f8d322e_0fe4_43e4_9e50_d83ecd9fc2b8);
const LOG: &str = r"C:\ProgramData\ClippiBoy\vcam.log";
/// The log starts over beyond this.
const LOG_LIMIT: u64 = 1024 * 1024;
const FRAME: Duration = Duration::from_nanos(33_333_333);

/// What the camera offers. The first one is the default.
const FORMATS: [(GUID, u32, u32); 6] = [
    (MFVideoFormat_NV12, 1280, 720),
    (MFVideoFormat_YUY2, 1280, 720),
    (MFVideoFormat_NV12, 1920, 1080),
    (MFVideoFormat_YUY2, 1920, 1080),
    (MFVideoFormat_NV12, 640, 360),
    (MFVideoFormat_YUY2, 640, 360),
];

// ---------------------------------------------------------------- plumbing

fn log(msg: impl AsRef<str>) {
    use std::io::Write;
    static EXE: OnceLock<String> = OnceLock::new();
    let exe = EXE.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default()
    });
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let mut session = 0u32;
    unsafe {
        let _ = ProcessIdToSessionId(GetCurrentProcessId(), &mut session);
    }
    let line = format!("{ms} pid={} s={session} {exe}: {}\r\n", std::process::id(), msg.as_ref());
    let wide: Vec<u16> = line.encode_utf16().chain(Some(0)).collect();
    unsafe { OutputDebugStringW(PCWSTR(wide.as_ptr())) };
    if std::fs::metadata(LOG).is_ok_and(|m| m.len() > LOG_LIMIT) {
        let _ = std::fs::remove_file(LOG);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(LOG) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// No panic may leave a COM method: inside Frame Server it would take every camera down.
fn guard<T>(what: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => {
            log(format!("{what} failed: {:#010x} {}", e.code().0, e.message()));
            Err(e)
        }
        Err(_) => {
            log(format!("{what} PANICKED"));
            Err(E_UNEXPECTED.into())
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn shut() -> Error {
    MF_E_SHUTDOWN.into()
}

fn fourcc(g: GUID) -> String {
    String::from_utf8_lossy(&g.data1.to_le_bytes()).into_owned()
}

unsafe fn new_attributes(n: u32) -> Result<IMFAttributes> {
    let mut a = None;
    MFCreateAttributes(&mut a, n)?;
    a.ok_or_else(|| E_UNEXPECTED.into())
}

unsafe fn video_type(sub: GUID, w: u32, h: u32) -> Result<IMFMediaType> {
    let t = MFCreateMediaType()?;
    t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    t.SetGUID(&MF_MT_SUBTYPE, &sub)?;
    t.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)?;
    t.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1)?;
    t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
    t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    t.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
    let (stride, bytes) = if sub == MFVideoFormat_YUY2 { (w * 2, w * h * 2) } else { (w, w * h * 3 / 2) };
    t.SetUINT32(&MF_MT_DEFAULT_STRIDE, stride)?;
    t.SetUINT32(&MF_MT_FIXED_SIZE_SAMPLES, 1)?;
    t.SetUINT32(&MF_MT_SAMPLE_SIZE, bytes)?;
    t.SetUINT32(&MF_MT_AVG_BITRATE, bytes * 8 * 30)?;
    Ok(t)
}

/// IMFAttributes forwarded to an inner MF attribute store.
macro_rules! forward_attributes {
    ($impl:ty) => {
        impl IMFAttributes_Impl for $impl {
            fn GetItem(&self, k: *const GUID, v: *mut PROPVARIANT) -> Result<()> {
                unsafe { (self.attrs.vtable().GetItem)(self.attrs.as_raw(), k, v.cast()).ok() }
            }
            fn GetItemType(&self, k: *const GUID) -> Result<MF_ATTRIBUTE_TYPE> {
                let mut o = MF_ATTRIBUTE_TYPE(0);
                unsafe { (self.attrs.vtable().GetItemType)(self.attrs.as_raw(), k, &mut o).ok()? };
                Ok(o)
            }
            fn CompareItem(&self, k: *const GUID, v: *const PROPVARIANT) -> Result<BOOL> {
                let mut o = BOOL(0);
                unsafe { (self.attrs.vtable().CompareItem)(self.attrs.as_raw(), k, v.cast(), &mut o).ok()? };
                Ok(o)
            }
            fn Compare(&self, theirs: Option<&IMFAttributes>, m: MF_ATTRIBUTES_MATCH_TYPE) -> Result<BOOL> {
                let mut o = BOOL(0);
                let p = theirs.map_or(null_mut(), |t| t.as_raw());
                unsafe { (self.attrs.vtable().Compare)(self.attrs.as_raw(), p, m, &mut o).ok()? };
                Ok(o)
            }
            fn GetUINT32(&self, k: *const GUID) -> Result<u32> {
                unsafe { self.attrs.GetUINT32(k) }
            }
            fn GetUINT64(&self, k: *const GUID) -> Result<u64> {
                unsafe { self.attrs.GetUINT64(k) }
            }
            fn GetDouble(&self, k: *const GUID) -> Result<f64> {
                unsafe { self.attrs.GetDouble(k) }
            }
            fn GetGUID(&self, k: *const GUID) -> Result<GUID> {
                unsafe { self.attrs.GetGUID(k) }
            }
            fn GetStringLength(&self, k: *const GUID) -> Result<u32> {
                unsafe { self.attrs.GetStringLength(k) }
            }
            fn GetString(&self, k: *const GUID, buf: PWSTR, cch: u32, len: *mut u32) -> Result<()> {
                unsafe { (self.attrs.vtable().GetString)(self.attrs.as_raw(), k, buf, cch, len).ok() }
            }
            fn GetAllocatedString(&self, k: *const GUID, pp: *mut PWSTR, len: *mut u32) -> Result<()> {
                unsafe { self.attrs.GetAllocatedString(k, pp, len) }
            }
            fn GetBlobSize(&self, k: *const GUID) -> Result<u32> {
                unsafe { self.attrs.GetBlobSize(k) }
            }
            fn GetBlob(&self, k: *const GUID, buf: *mut u8, cb: u32, len: *mut u32) -> Result<()> {
                unsafe { (self.attrs.vtable().GetBlob)(self.attrs.as_raw(), k, buf, cb, len).ok() }
            }
            fn GetAllocatedBlob(&self, k: *const GUID, pp: *mut *mut u8, cb: *mut u32) -> Result<()> {
                unsafe { self.attrs.GetAllocatedBlob(k, pp, cb) }
            }
            fn GetUnknown(&self, k: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
                unsafe { (self.attrs.vtable().GetUnknown)(self.attrs.as_raw(), k, riid, ppv).ok() }
            }
            fn SetItem(&self, k: *const GUID, v: *const PROPVARIANT) -> Result<()> {
                unsafe { (self.attrs.vtable().SetItem)(self.attrs.as_raw(), k, v.cast()).ok() }
            }
            fn DeleteItem(&self, k: *const GUID) -> Result<()> {
                unsafe { self.attrs.DeleteItem(k) }
            }
            fn DeleteAllItems(&self) -> Result<()> {
                unsafe { self.attrs.DeleteAllItems() }
            }
            fn SetUINT32(&self, k: *const GUID, v: u32) -> Result<()> {
                unsafe { self.attrs.SetUINT32(k, v) }
            }
            fn SetUINT64(&self, k: *const GUID, v: u64) -> Result<()> {
                unsafe { self.attrs.SetUINT64(k, v) }
            }
            fn SetDouble(&self, k: *const GUID, v: f64) -> Result<()> {
                unsafe { self.attrs.SetDouble(k, v) }
            }
            fn SetGUID(&self, k: *const GUID, v: *const GUID) -> Result<()> {
                unsafe { self.attrs.SetGUID(k, v) }
            }
            fn SetString(&self, k: *const GUID, v: &PCWSTR) -> Result<()> {
                unsafe { (self.attrs.vtable().SetString)(self.attrs.as_raw(), k, *v).ok() }
            }
            fn SetBlob(&self, k: *const GUID, buf: *const u8, cb: u32) -> Result<()> {
                unsafe { (self.attrs.vtable().SetBlob)(self.attrs.as_raw(), k, buf, cb).ok() }
            }
            fn SetUnknown(&self, k: *const GUID, u: Option<&IUnknown>) -> Result<()> {
                let p = u.map_or(null_mut(), |u| u.as_raw());
                unsafe { (self.attrs.vtable().SetUnknown)(self.attrs.as_raw(), k, p).ok() }
            }
            fn LockStore(&self) -> Result<()> {
                unsafe { self.attrs.LockStore() }
            }
            fn UnlockStore(&self) -> Result<()> {
                unsafe { self.attrs.UnlockStore() }
            }
            fn GetCount(&self) -> Result<u32> {
                unsafe { self.attrs.GetCount() }
            }
            fn GetItemByIndex(&self, i: u32, k: *mut GUID, v: *mut PROPVARIANT) -> Result<()> {
                unsafe { (self.attrs.vtable().GetItemByIndex)(self.attrs.as_raw(), i, k, v.cast()).ok() }
            }
            fn CopyAllItems(&self, dest: Option<&IMFAttributes>) -> Result<()> {
                let p = dest.map_or(null_mut(), |d| d.as_raw());
                unsafe { (self.attrs.vtable().CopyAllItems)(self.attrs.as_raw(), p).ok() }
            }
        }
    };
}

/// IMFMediaEventGenerator forwarded to the object's event queue. The queue is
/// taken out of the lock first: GetEvent may block.
macro_rules! forward_events {
    ($impl:ty) => {
        impl IMFMediaEventGenerator_Impl for $impl {
            fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
                let q = self.queue()?;
                unsafe { q.GetEvent(flags.0) }
            }
            fn BeginGetEvent(&self, cb: Option<&IMFAsyncCallback>, state: Option<&IUnknown>) -> Result<()> {
                let q = self.queue()?;
                unsafe { q.BeginGetEvent(cb, state) }
            }
            fn EndGetEvent(&self, r: Option<&IMFAsyncResult>) -> Result<IMFMediaEvent> {
                let q = self.queue()?;
                unsafe { q.EndGetEvent(r) }
            }
            fn QueueEvent(&self, met: u32, ext: *const GUID, hr: HRESULT, v: *const PROPVARIANT) -> Result<()> {
                let q = self.queue()?;
                unsafe { q.QueueEventParamVar(met, ext, hr, v) }
            }
        }
    };
}

macro_rules! no_ks_control {
    ($impl:ty) => {
        impl IKsControl_Impl for $impl {
            fn KsProperty(&self, _: *const KSIDENTIFIER, _: u32, _: *mut c_void, _: u32, _: *mut u32) -> Result<()> {
                Err(HRESULT::from_win32(ERROR_SET_NOT_FOUND.0).into())
            }
            fn KsMethod(&self, _: *const KSIDENTIFIER, _: u32, _: *mut c_void, _: u32, _: *mut u32) -> Result<()> {
                Err(HRESULT::from_win32(ERROR_SET_NOT_FOUND.0).into())
            }
            fn KsEvent(&self, _: *const KSIDENTIFIER, _: u32, _: *mut c_void, _: u32, _: *mut u32) -> Result<()> {
                Err(HRESULT::from_win32(ERROR_SET_NOT_FOUND.0).into())
            }
        }
    };
}

// ---------------------------------------------------------------- frames

struct Mapping {
    view: *const u8,
    reader: vcam_protocol::Reader,
}

impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.view as *mut c_void });
        }
    }
}

/// Opens ClippiBoy's section. Frame Server lives in session 0, ClippiBoy in
/// the user's, so the name carries the session.
unsafe fn open_mapping() -> Option<Mapping> {
    let mut sessions = vec![WTSGetActiveConsoleSessionId()];
    sessions.extend(1..=8);
    for s in sessions {
        if s == u32::MAX || s == 0 {
            continue;
        }
        let name: Vec<u16> = format!("Session\\{s}\\{}", vcam_protocol::NAME).encode_utf16().chain(Some(0)).collect();
        if let Ok(h) = OpenFileMappingW(FILE_MAP_READ.0, BOOL(0), PCWSTR(name.as_ptr())) {
            let view = MapViewOfFile(h, FILE_MAP_READ, 0, 0, vcam_protocol::TOTAL_BYTES);
            let _ = CloseHandle(h);
            if view.Value.is_null() {
                continue;
            }
            match vcam_protocol::Reader::new(view.Value as *const u8, vcam_protocol::TOTAL_BYTES) {
                Some(reader) => {
                    log(format!("opened Session\\{s}\\{}", vcam_protocol::NAME));
                    return Some(Mapping { view: view.Value as *const u8, reader });
                }
                None => {
                    let _ = UnmapViewOfFile(view);
                }
            }
        }
    }
    None
}

/// What the camera shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Picture {
    Live,
    /// ClippiBoy is not running, or its camera is off or busy.
    Placeholder,
}

fn picture_for(read: &vcam_protocol::ReadResult) -> Picture {
    match read {
        vcam_protocol::ReadResult::Frame { .. } => Picture::Live,
        _ => Picture::Placeholder,
    }
}

#[derive(Default)]
struct Feed {
    map: Option<Mapping>,
    last_try: Option<Instant>,
    /// The last good frame from ClippiBoy and its size.
    live: Vec<u8>,
    dims: Option<(usize, usize)>,
    scratch: Vec<u8>,
    placeholder: Vec<u8>,
    out: Vec<u8>,
    was_live: Option<bool>,
    sent: u64,
}

impl Feed {
    /// ClippiBoy's newest frame, or its previous one when a read met a write
    /// half-way. `None` while there is none to show.
    unsafe fn read_live(&mut self) -> Option<(usize, usize)> {
        if self.map.is_none() {
            if self.last_try.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
                return None;
            }
            self.last_try = Some(Instant::now());
            self.map = open_mapping();
        }
        let read = self.map.as_ref()?.reader.read_latest(&mut self.scratch, MFGetSystemTime());
        match (picture_for(&read), read) {
            (Picture::Live, vcam_protocol::ReadResult::Frame { width, height, .. }) => {
                std::mem::swap(&mut self.live, &mut self.scratch);
                self.dims = Some((width as usize, height as usize));
            }
            (_, vcam_protocol::ReadResult::Torn) => {}
            _ => self.dims = None,
        }
        self.dims
    }

    unsafe fn frame(&mut self, sub: GUID, w: usize, h: usize) -> &[u8] {
        let live = self.read_live();
        if self.was_live != Some(live.is_some()) {
            log(if live.is_some() { "feed: frames from ClippiBoy" } else { "feed: placeholder" });
            self.was_live = Some(live.is_some());
        }
        let (src, sw, sh) = match live {
            Some((sw, sh)) => (&self.live, sw, sh),
            None => {
                placeholder(&mut self.placeholder, 1280, 720);
                (&self.placeholder, 1280, 720)
            }
        };
        let yuy2 = sub == MFVideoFormat_YUY2;
        self.out.resize(if yuy2 { w * h * 2 } else { w * h * 3 / 2 }, 0);
        if yuy2 {
            nv12_to_yuy2(src, sw, sh, &mut self.out, w, h);
        } else {
            nv12_scaled(src, sw, sh, &mut self.out, w, h);
        }
        self.sent += 1;
        if self.sent == 1 || self.sent % 9000 == 0 {
            log(format!("sent {} frames ({} {w}x{h})", self.sent, fourcc(sub)));
        }
        &self.out
    }
}

/// A dark picture with ClippiBoy's violet along the bottom: the camera is
/// there, but ClippiBoy has no picture for it right now.
fn placeholder(out: &mut Vec<u8>, w: usize, h: usize) {
    if out.len() == w * h * 3 / 2 {
        return;
    }
    out.resize(w * h * 3 / 2, 0);
    let (y, uv) = out.split_at_mut(w * h);
    let bar = h - h / 24;
    for row in 0..h {
        y[row * w..(row + 1) * w].fill(if row >= bar { 70 } else { 22 });
    }
    for (i, pair) in uv.chunks_exact_mut(2).enumerate() {
        let row = i / (w / 2) * 2;
        let (u, v) = if row >= bar { (176, 160) } else { (128, 128) };
        pair[0] = u;
        pair[1] = v;
    }
}

/// NV12 to NV12, nearest-neighbour scaled.
fn nv12_scaled(src: &[u8], sw: usize, sh: usize, out: &mut [u8], w: usize, h: usize) {
    if sw == w && sh == h {
        let n = w * h * 3 / 2;
        out[..n].copy_from_slice(&src[..n]);
        return;
    }
    let (ys, uvs) = src.split_at(sw * sh);
    let (yo, uvo) = out.split_at_mut(w * h);
    for y in 0..h {
        let sy = y * sh / h;
        let srow = &ys[sy * sw..(sy + 1) * sw];
        for (x, o) in yo[y * w..(y + 1) * w].iter_mut().enumerate() {
            *o = srow[x * sw / w];
        }
    }
    for r in 0..h / 2 {
        let sr = r * sh / h;
        let srow = &uvs[sr * sw..(sr + 1) * sw];
        let orow = &mut uvo[r * w..(r + 1) * w];
        for c in 0..w / 2 {
            let sc = c * sw / w;
            orow[2 * c] = srow[2 * sc];
            orow[2 * c + 1] = srow[2 * sc + 1];
        }
    }
}

/// NV12 to YUY2, nearest-neighbour scaled. DirectShow clients such as
/// libwebrtc (Discord) do not take NV12.
fn nv12_to_yuy2(src: &[u8], sw: usize, sh: usize, out: &mut [u8], w: usize, h: usize) {
    let (ys, uvs) = src.split_at(sw * sh);
    for y in 0..h {
        let sy = y * sh / h;
        let yrow = &ys[sy * sw..(sy + 1) * sw];
        let uvrow = &uvs[(sy / 2) * sw..(sy / 2 + 1) * sw];
        let orow = &mut out[y * w * 2..(y + 1) * w * 2];
        for c in 0..w / 2 {
            let sx0 = 2 * c * sw / w;
            let sx1 = (2 * c + 1) * sw / w;
            let sc = sx0 / 2;
            orow[4 * c] = yrow[sx0];
            orow[4 * c + 1] = uvrow[2 * sc];
            orow[4 * c + 2] = yrow[sx1];
            orow[4 * c + 3] = uvrow[2 * sc + 1];
        }
    }
}

unsafe fn make_sample(data: &[u8]) -> Result<IMFSample> {
    let buf = MFCreateMemoryBuffer(data.len() as u32)?;
    let mut p = null_mut();
    buf.Lock(&mut p, None, None)?;
    std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
    buf.Unlock()?;
    buf.SetCurrentLength(data.len() as u32)?;
    let s = MFCreateSample()?;
    s.AddBuffer(&buf)?;
    Ok(s)
}

// ---------------------------------------------------------------- stream

struct StreamInner {
    queue: Option<IMFMediaEventQueue>,
    descriptor: Option<IMFStreamDescriptor>,
    source: Option<IMFMediaSource>,
    state: MF_STREAM_STATE,
    subtype: GUID,
    width: usize,
    height: usize,
    next_due: Option<Instant>,
    feed: Feed,
}

#[implement(IMFMediaStream2, IKsControl, IMFAttributes)]
struct MediaStream {
    attrs: IMFAttributes,
    inner: Mutex<StreamInner>,
}

impl MediaStream {
    unsafe fn create() -> Result<ComObject<MediaStream>> {
        let attrs = new_attributes(8)?;
        attrs.SetGUID(&MF_DEVICESTREAM_STREAM_CATEGORY, &PINNAME_VIDEO_CAPTURE)?;
        attrs.SetUINT32(&MF_DEVICESTREAM_STREAM_ID, 0)?;
        attrs.SetUINT32(&MF_DEVICESTREAM_FRAMESERVER_SHARED, 1)?;
        attrs.SetUINT32(&MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES, MFFrameSourceTypes_Color.0 as u32)?;
        let mut types = Vec::new();
        for (sub, w, h) in FORMATS {
            types.push(Some(video_type(sub, w, h)?));
        }
        let descriptor = MFCreateStreamDescriptor(0, &types)?;
        if let Some(first) = &types[0] {
            descriptor.GetMediaTypeHandler()?.SetCurrentMediaType(first)?;
        }
        Ok(ComObject::new(MediaStream {
            attrs,
            inner: Mutex::new(StreamInner {
                queue: Some(MFCreateEventQueue()?),
                descriptor: Some(descriptor),
                source: None,
                state: MF_STREAM_STATE_STOPPED,
                subtype: FORMATS[0].0,
                width: FORMATS[0].1 as usize,
                height: FORMATS[0].2 as usize,
                next_due: None,
                feed: Feed::default(),
            }),
        }))
    }

    fn queue(&self) -> Result<IMFMediaEventQueue> {
        lock(&self.inner).queue.clone().ok_or_else(shut)
    }

    fn state(&self) -> MF_STREAM_STATE {
        lock(&self.inner).state
    }

    unsafe fn start(&self, ty: Option<&IMFMediaType>) -> Result<()> {
        let mut g = lock(&self.inner);
        let q = g.queue.clone().ok_or_else(shut)?;
        if let Some(t) = ty {
            let sub = t.GetGUID(&MF_MT_SUBTYPE)?;
            let size = t.GetUINT64(&MF_MT_FRAME_SIZE)?;
            let (w, h) = ((size >> 32) as usize, (size & 0xffff_ffff) as usize);
            if w == 0 || h == 0 || w > 1920 || h > 1080 || w % 2 != 0 || h % 2 != 0 {
                return Err(MF_E_INVALIDMEDIATYPE.into());
            }
            (g.subtype, g.width, g.height) = (sub, w, h);
        }
        g.state = MF_STREAM_STATE_RUNNING;
        g.next_due = None;
        log(format!("stream start {} {}x{}", fourcc(g.subtype), g.width, g.height));
        drop(g);
        q.QueueEventParamVar(MEStreamStarted.0 as u32, &GUID::zeroed(), S_OK, std::ptr::null())
    }

    unsafe fn stop(&self) -> Result<()> {
        let mut g = lock(&self.inner);
        let q = g.queue.clone().ok_or_else(shut)?;
        g.state = MF_STREAM_STATE_STOPPED;
        log(format!("stream stop after {} frames", g.feed.sent));
        drop(g);
        q.QueueEventParamVar(MEStreamStopped.0 as u32, &GUID::zeroed(), S_OK, std::ptr::null())
    }

    unsafe fn shutdown(&self) {
        let mut g = lock(&self.inner);
        if let Some(q) = g.queue.take() {
            let _ = q.Shutdown();
        }
        g.descriptor = None;
        g.source = None;
        g.feed = Feed::default();
    }
}

forward_attributes!(MediaStream_Impl);
forward_events!(MediaStream_Impl);
no_ks_control!(MediaStream_Impl);

impl IMFMediaStream_Impl for MediaStream_Impl {
    fn GetMediaSource(&self) -> Result<IMFMediaSource> {
        lock(&self.inner).source.clone().ok_or_else(shut)
    }

    fn GetStreamDescriptor(&self) -> Result<IMFStreamDescriptor> {
        lock(&self.inner).descriptor.clone().ok_or_else(shut)
    }

    fn RequestSample(&self, token: Option<&IUnknown>) -> Result<()> {
        guard("RequestSample", || unsafe {
            // Pacing is the source's job: Frame Server asks again as soon as it gets a frame.
            let wait = lock(&self.inner).next_due.map(|d| d.saturating_duration_since(Instant::now()));
            if let Some(w) = wait.filter(|w| !w.is_zero()) {
                std::thread::sleep(w.min(Duration::from_millis(100)));
            }
            let mut g = lock(&self.inner);
            let q = g.queue.clone().ok_or_else(shut)?;
            let now = Instant::now();
            g.next_due = Some(g.next_due.unwrap_or(now).max(now) + FRAME);
            let (sub, w, h) = (g.subtype, g.width, g.height);
            let sample = make_sample(g.feed.frame(sub, w, h))?;
            drop(g);
            sample.SetSampleTime(MFGetSystemTime())?;
            sample.SetSampleDuration(333_333)?;
            if let Some(t) = token {
                sample.SetUnknown(&MFSampleExtension_Token, t)?;
            }
            q.QueueEventParamUnk(MEMediaSample.0 as u32, &GUID::zeroed(), S_OK, &sample)
        })
    }
}

impl IMFMediaStream2_Impl for MediaStream_Impl {
    fn SetStreamState(&self, value: MF_STREAM_STATE) -> Result<()> {
        guard("SetStreamState", || unsafe {
            let cur = self.state();
            if value == cur {
                return Ok(());
            }
            match value {
                MF_STREAM_STATE_PAUSED if cur == MF_STREAM_STATE_RUNNING => {
                    lock(&self.inner).state = value;
                    Ok(())
                }
                MF_STREAM_STATE_RUNNING => self.start(None),
                MF_STREAM_STATE_STOPPED => self.stop(),
                _ => Err(MF_E_INVALID_STATE_TRANSITION.into()),
            }
        })
    }

    fn GetStreamState(&self) -> Result<MF_STREAM_STATE> {
        Ok(self.state())
    }
}

// ---------------------------------------------------------------- source

struct SourceInner {
    queue: Option<IMFMediaEventQueue>,
    descriptor: Option<IMFPresentationDescriptor>,
    stream: Option<ComObject<MediaStream>>,
}

#[implement(IMFMediaSourceEx, IMFGetService, IKsControl, IMFAttributes)]
struct MediaSource {
    attrs: IMFAttributes,
    inner: Mutex<SourceInner>,
}

impl MediaSource {
    fn queue(&self) -> Result<IMFMediaEventQueue> {
        lock(&self.inner).queue.clone().ok_or_else(shut)
    }
}

unsafe fn create_source(activator: &IMFAttributes) -> Result<IMFMediaSource> {
    let attrs = new_attributes(8)?;
    activator.CopyAllItems(&attrs)?;
    let stream = MediaStream::create()?;
    let sd = lock(&stream.inner).descriptor.clone().ok_or_else(shut)?;
    let pd = MFCreatePresentationDescriptor(Some(&[Some(sd)]))?;
    let source = ComObject::new(MediaSource {
        attrs,
        inner: Mutex::new(SourceInner { queue: Some(MFCreateEventQueue()?), descriptor: Some(pd), stream: Some(stream.clone()) }),
    });
    let ex: IMFMediaSourceEx = source.to_interface();
    let iface: IMFMediaSource = ex.cast()?;
    lock(&stream.inner).source = Some(iface.clone());
    log("source created");
    Ok(iface)
}

forward_attributes!(MediaSource_Impl);
forward_events!(MediaSource_Impl);
no_ks_control!(MediaSource_Impl);

impl IMFMediaSource_Impl for MediaSource_Impl {
    fn GetCharacteristics(&self) -> Result<u32> {
        Ok(MFMEDIASOURCE_IS_LIVE.0 as u32)
    }

    fn CreatePresentationDescriptor(&self) -> Result<IMFPresentationDescriptor> {
        guard("CreatePresentationDescriptor", || unsafe {
            let d = lock(&self.inner).descriptor.clone().ok_or_else(shut)?;
            d.Clone()
        })
    }

    fn Start(&self, pd: Option<&IMFPresentationDescriptor>, format: *const GUID, _pos: *const PROPVARIANT) -> Result<()> {
        guard("Source::Start", || unsafe {
            let pd = pd.ok_or_else(|| Error::from(E_POINTER))?;
            if !format.is_null() && *format != GUID::zeroed() {
                return Err(E_INVALIDARG.into());
            }
            let g = lock(&self.inner);
            let queue = g.queue.clone().ok_or_else(shut)?;
            let ours = g.descriptor.clone().ok_or_else(shut)?;
            let stream = g.stream.clone().ok_or_else(shut)?;
            drop(g);

            let count = pd.GetStreamDescriptorCount()?;
            for i in 0..count {
                let mut selected = BOOL(0);
                let mut desc = None;
                pd.GetStreamDescriptorByIndex(i, &mut selected, &mut desc)?;
                let desc = desc.ok_or_else(|| Error::from(E_POINTER))?;
                let mut ours_selected = BOOL(0);
                let mut ours_desc = None;
                ours.GetStreamDescriptorByIndex(0, &mut ours_selected, &mut ours_desc)?;
                let running = stream.state() != MF_STREAM_STATE_STOPPED;
                if selected.as_bool() && !running {
                    ours.SelectStream(0)?;
                    let unk: IUnknown = stream.to_interface();
                    queue.QueueEventParamUnk(MENewStream.0 as u32, &GUID::zeroed(), S_OK, &unk)?;
                    let ty = desc.GetMediaTypeHandler()?.GetCurrentMediaType()?;
                    stream.start(Some(&ty))?;
                } else if !selected.as_bool() && running {
                    ours.DeselectStream(0)?;
                    stream.stop()?;
                }
            }
            let now = PROPVARIANT::from(MFGetSystemTime());
            queue.QueueEventParamVar(MESourceStarted.0 as u32, &GUID::zeroed(), S_OK, &now)
        })
    }

    fn Stop(&self) -> Result<()> {
        guard("Source::Stop", || unsafe {
            let g = lock(&self.inner);
            let queue = g.queue.clone().ok_or_else(shut)?;
            let ours = g.descriptor.clone().ok_or_else(shut)?;
            let stream = g.stream.clone().ok_or_else(shut)?;
            drop(g);
            if stream.state() != MF_STREAM_STATE_STOPPED {
                stream.stop()?;
            }
            ours.DeselectStream(0)?;
            let now = PROPVARIANT::from(MFGetSystemTime());
            queue.QueueEventParamVar(MESourceStopped.0 as u32, &GUID::zeroed(), S_OK, &now)
        })
    }

    fn Pause(&self) -> Result<()> {
        Err(MF_E_INVALID_STATE_TRANSITION.into())
    }

    fn Shutdown(&self) -> Result<()> {
        guard("Source::Shutdown", || unsafe {
            let mut g = lock(&self.inner);
            let queue = g.queue.take().ok_or_else(shut)?;
            let stream = g.stream.take();
            g.descriptor = None;
            drop(g);
            let _ = queue.Shutdown();
            if let Some(s) = stream {
                s.shutdown();
            }
            log("source shut down");
            Ok(())
        })
    }
}

impl IMFMediaSourceEx_Impl for MediaSource_Impl {
    fn GetSourceAttributes(&self) -> Result<IMFAttributes> {
        Ok(self.attrs.clone())
    }

    fn GetStreamAttributes(&self, id: u32) -> Result<IMFAttributes> {
        if id != 0 {
            return Err(E_INVALIDARG.into());
        }
        let stream = lock(&self.inner).stream.clone().ok_or_else(shut)?;
        Ok(stream.attrs.clone())
    }

    fn SetD3DManager(&self, _: Option<&IUnknown>) -> Result<()> {
        Ok(())
    }
}

impl IMFGetService_Impl for MediaSource_Impl {
    fn GetService(&self, _: *const GUID, _: *const GUID, _: *mut *mut c_void) -> Result<()> {
        Err(MF_E_UNSUPPORTED_SERVICE.into())
    }
}

// ---------------------------------------------------------------- activator, factory, exports

#[implement(IMFActivate)]
struct Activator {
    attrs: IMFAttributes,
    source: Mutex<Option<IMFMediaSource>>,
}

forward_attributes!(Activator_Impl);

impl IMFActivate_Impl for Activator_Impl {
    fn ActivateObject(&self, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
        guard("ActivateObject", || unsafe {
            if ppv.is_null() || riid.is_null() {
                return Err(E_POINTER.into());
            }
            *ppv = null_mut();
            let mut slot = lock(&self.source);
            if slot.is_none() {
                *slot = Some(create_source(&self.attrs)?);
            }
            let pid = self.attrs.GetUINT32(&CLIENT_PID).unwrap_or(0);
            log(format!("ActivateObject {:?} client pid {pid}", *riid));
            match slot.as_ref() {
                Some(s) => s.query(riid, ppv).ok(),
                None => Err(E_UNEXPECTED.into()),
            }
        })
    }

    fn ShutdownObject(&self) -> Result<()> {
        Ok(())
    }

    fn DetachObject(&self) -> Result<()> {
        *lock(&self.source) = None;
        Ok(())
    }
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Option<&IUnknown>, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
        guard("CreateInstance", || unsafe {
            if ppv.is_null() || riid.is_null() {
                return Err(E_POINTER.into());
            }
            *ppv = null_mut();
            if outer.is_some() {
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            let attrs = new_attributes(4)?;
            attrs.SetGUID(&MFT_TRANSFORM_CLSID_Attribute, &CLSID)?;
            let act: IMFActivate = Activator { attrs, source: Mutex::new(None) }.into();
            log(format!("CreateInstance {:?}", *riid));
            act.query(riid, ppv).ok()
        })
    }

    fn LockServer(&self, _: BOOL) -> Result<()> {
        Ok(())
    }
}

#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(clsid: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> HRESULT {
    catch_unwind(AssertUnwindSafe(|| {
        if ppv.is_null() || clsid.is_null() || riid.is_null() {
            return E_POINTER;
        }
        *ppv = null_mut();
        if *clsid != CLSID {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        log("DllGetClassObject");
        let f: IClassFactory = Factory.into();
        f.query(riid, ppv)
    }))
    .unwrap_or(E_UNEXPECTED)
}

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: usize, h: usize) -> Vec<u8> {
        let mut v = vec![0u8; w * h * 3 / 2];
        for (i, b) in v.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        v
    }

    #[test]
    fn the_class_id_is_the_one_the_installer_registers() {
        assert_eq!(format!("{{{:?}}}", CLSID).to_lowercase(), vcam_protocol::CLSID);
    }

    #[test]
    fn only_a_frame_shows_live() {
        use vcam_protocol::{IdleReason, ReadResult};
        assert_eq!(picture_for(&ReadResult::Frame { width: 2, height: 2, qpc_100ns: 0 }), Picture::Live);
        for read in [ReadResult::NotRunning, ReadResult::Idle(IdleReason::CamOff), ReadResult::Torn] {
            assert_eq!(picture_for(&read), Picture::Placeholder);
        }
    }

    #[test]
    fn the_placeholder_is_dark_with_a_violet_bar() {
        let mut p = Vec::new();
        placeholder(&mut p, 64, 48);
        assert_eq!(p.len(), 64 * 48 * 3 / 2);
        assert_eq!(p[0], 22);
        assert_eq!(p[64 * 47], 70);
    }

    #[test]
    fn same_size_nv12_is_copied() {
        let src = gradient(64, 32);
        let mut out = vec![0u8; src.len()];
        nv12_scaled(&src, 64, 32, &mut out, 64, 32);
        assert_eq!(src, out);
    }

    #[test]
    fn scaling_stays_in_bounds_both_ways() {
        let src = gradient(1280, 720);
        for (w, h) in [(1920, 1080), (640, 360), (1280, 720)] {
            let mut out = vec![0u8; w * h * 3 / 2];
            nv12_scaled(&src, 1280, 720, &mut out, w, h);
            let mut yuy2 = vec![0u8; w * h * 2];
            nv12_to_yuy2(&src, 1280, 720, &mut yuy2, w, h);
        }
    }

    #[test]
    fn yuy2_takes_luma_and_shared_chroma() {
        // 2x2 NV12: Y = 10 20 / 30 40, UV = 100 200
        let src = vec![10, 20, 30, 40, 100, 200];
        let mut out = vec![0u8; 8];
        nv12_to_yuy2(&src, 2, 2, &mut out, 2, 2);
        assert_eq!(out, vec![10, 100, 20, 200, 30, 100, 40, 200]);
    }
}
