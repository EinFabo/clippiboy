//! The camera through Media Foundation: listed with `MFEnumDeviceSources`,
//! read through an asynchronous source reader that decodes to NV12 in system
//! memory, so whatever the camera runs (MJPEG, YUY2, NV12) arrives the same.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use ::windows::core::{implement, Result as WinResult, GUID, HRESULT, PCWSTR, PWSTR};
use ::windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Register_Notification, CM_Unregister_Notification, CM_NOTIFY_ACTION, CM_NOTIFY_EVENT_DATA,
    CM_NOTIFY_FILTER, CM_NOTIFY_FILTER_0, CM_NOTIFY_FILTER_0_2, CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE, CR_SUCCESS,
    HCMNOTIFICATION,
};
use ::windows::Win32::Foundation::{CloseHandle, BOOL, E_FAIL};
use ::windows::Win32::Media::KernelStreaming::KSCATEGORY_VIDEO_CAMERA;
use ::windows::Win32::Media::MediaFoundation::*;
use ::windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};
use ::windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use parking_lot::Mutex;

use super::{choose_stamp, classify, pick_native, CameraDevice, Error, Event, Frame, NativeType, OpenMode, ProbeFrame, Sink, Source};
use crate::audio::capture::now_100ns;
use crate::gpu::SendPtr;

const FIRST_VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
/// Our own virtual camera is never a source for the facecam.
const OWN_CAMERA: &str = "ClippiBoy Camera";

/// Every entry point may run on a fresh tokio or helper thread.
fn ensure_mf() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    if let Err(err) = crate::mft::startup() {
        log::warn!("camera: {err}");
    }
}

unsafe fn attributes(n: u32) -> WinResult<IMFAttributes> {
    let mut a = None;
    MFCreateAttributes(&mut a, n)?;
    a.ok_or_else(|| E_FAIL.into())
}

unsafe fn alloc_string(a: &IMFAttributes, key: &GUID) -> Option<String> {
    let mut p = PWSTR::null();
    let mut n = 0u32;
    a.GetAllocatedString(key, &mut p, &mut n).ok()?;
    let text = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as _));
    text
}

pub fn list() -> Vec<CameraDevice> {
    ensure_mf();
    unsafe {
        enumerate().unwrap_or_else(|err| {
            log::warn!("camera list: {err}");
            Vec::new()
        })
    }
}

unsafe fn enumerate() -> WinResult<Vec<CameraDevice>> {
    let attrs = attributes(1)?;
    attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)?;
    let mut array: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    MFEnumDeviceSources(&attrs, &mut array, &mut count)?;
    let mut out = Vec::new();
    for i in 0..count as usize {
        if let Some(activate) = std::ptr::read(array.add(i)) {
            let name = alloc_string(&activate, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME).unwrap_or_default();
            let id = alloc_string(&activate, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK).unwrap_or_default();
            if !id.is_empty() && !name.starts_with(OWN_CAMERA) {
                out.push(CameraDevice { id, name });
            }
        }
    }
    CoTaskMemFree(Some(array as _));
    Ok(out)
}

fn fourcc(sub: GUID) -> [u8; 4] {
    sub.data1.to_le_bytes()
}

unsafe fn native_of(t: &IMFMediaType) -> NativeType {
    let size = t.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
    let rate = t.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(0);
    NativeType {
        subtype: fourcc(t.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default()),
        width: (size >> 32) as u32,
        height: size as u32,
        fps: (rate >> 32) as f32 / (rate as u32).max(1) as f32,
    }
}

unsafe fn media_source(link: &str, mode: OpenMode) -> WinResult<IMFMediaSource> {
    let wide: Vec<u16> = link.encode_utf16().chain(Some(0)).collect();
    match mode {
        OpenMode::MfSensorShared => {
            let group = MFCreateSensorGroup(PCWSTR(wide.as_ptr()))?;
            let device = group.GetSensorDevice(0)?;
            device.SetSensorDeviceMode(MFSensorDeviceMode_Shared)?;
            group.CreateMediaSource()
        }
        OpenMode::Controlling { .. } => {
            let attrs = attributes(2)?;
            attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)?;
            attrs.SetString(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, PCWSTR(wide.as_ptr()))?;
            MFCreateDeviceSource(&attrs)
        }
    }
}

/// What the reader's callback and the handle share. The callback holds it and
/// the reader holds the callback: taking the reader out of `reader` on close is
/// what breaks that cycle, without which the source is never shut down and the
/// LED stays on.
struct Shared {
    reader: Mutex<Option<SendPtr<IMFSourceReader>>>,
    sink: Mutex<Sink>,
    stop: AtomicBool,
    flushed: Mutex<Option<mpsc::Sender<()>>>,
    size: Mutex<(u32, u32)>,
    stride: Mutex<u32>,
    lost: AtomicBool,
}

impl Shared {
    fn send(&self, event: Event) {
        (self.sink.lock())(event);
    }

    /// Reported once; the reader is not asked again after it.
    fn lose(&self, error: Error) {
        if !self.lost.swap(true, Ordering::SeqCst) {
            self.send(Event::Lost(error));
        }
    }

    fn read_next(&self) {
        if let Some(reader) = self.reader.lock().as_ref() {
            if let Err(err) = unsafe { reader.ReadSample(FIRST_VIDEO, 0, None, None, None, None) } {
                self.lose(classify(err.code().0));
            }
        }
    }

    /// The size and row pitch of what the reader hands out now.
    fn read_type(&self) {
        let guard = self.reader.lock();
        let Some(reader) = guard.as_ref() else { return };
        unsafe {
            if let Ok(t) = reader.GetCurrentMediaType(FIRST_VIDEO) {
                let size = t.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
                let (w, h) = ((size >> 32) as u32, size as u32);
                *self.size.lock() = (w, h);
                *self.stride.lock() = t.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| (s as i32).unsigned_abs()).unwrap_or(w);
            }
        }
    }

    /// A tight NV12 copy with the stamps of the stage-1 rule.
    unsafe fn frame(&self, sample: &IMFSample, sample_time: i64) -> Option<Frame> {
        let arrival = now_100ns();
        let device = sample.GetUINT64(&MFSampleExtension_DeviceTimestamp).ok().map(|v| v as i64);
        let device_ref = sample.GetUINT64(&MFSampleExtension_DeviceReferenceSystemTime).ok().map(|v| v as i64);
        let (qpc, clock) = choose_stamp(device, device_ref, Some(sample_time), arrival);
        let (w, h) = *self.size.lock();
        let (w, h) = (w as usize, h as usize);
        let stride = (*self.stride.lock() as usize).max(w);
        let buffer = sample.ConvertToContiguousBuffer().ok()?;
        let mut p = std::ptr::null_mut();
        let mut len = 0u32;
        buffer.Lock(&mut p, None, Some(&mut len)).ok()?;
        let need = stride * h + stride * (h / 2);
        let copy = (w > 0 && h > 0 && len as usize >= need).then(|| {
            let src = std::slice::from_raw_parts(p, len as usize);
            let mut out = Vec::with_capacity(w * h * 3 / 2);
            for row in 0..h {
                out.extend_from_slice(&src[row * stride..row * stride + w]);
            }
            let uv = stride * h;
            for row in 0..h / 2 {
                out.extend_from_slice(&src[uv + row * stride..uv + row * stride + w]);
            }
            out
        });
        let _ = buffer.Unlock();
        let nv12: Arc<[u8]> = copy?.into();
        Some(Frame {
            probe: ProbeFrame {
                qpc_100ns: qpc,
                clock,
                sample_time_100ns: Some(sample_time),
                device_ts_100ns: device,
                device_ref_ts_100ns: device_ref,
                arrival_100ns: arrival,
                subtype: *b"NV12",
                width: w as u32,
                height: h as u32,
                bytes: len as usize,
            },
            nv12,
        })
    }
}

#[implement(IMFSourceReaderCallback)]
struct Callback {
    shared: Arc<Shared>,
}

impl IMFSourceReaderCallback_Impl for Callback_Impl {
    fn OnReadSample(&self, hr: HRESULT, _stream: u32, flags: u32, timestamp: i64, sample: Option<&IMFSample>) -> WinResult<()> {
        let shared = &self.shared;
        if shared.stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        if hr.is_err() {
            // `0xC00D3704` here, not on open, when another app holds the camera.
            shared.lose(classify(hr.0));
            return Ok(());
        }
        if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
            shared.lose(Error::Other("the camera's stream failed".into()));
            return Ok(());
        }
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            shared.lose(Error::Gone);
            return Ok(());
        }
        if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
            shared.read_type();
            let (width, height) = *shared.size.lock();
            shared.send(Event::FormatChanged { width, height });
        }
        if let Some(sample) = sample {
            if let Some(frame) = unsafe { shared.frame(sample, timestamp) } {
                shared.send(Event::Frame(frame));
            }
        }
        shared.read_next();
        Ok(())
    }

    fn OnFlush(&self, _stream: u32) -> WinResult<()> {
        if let Some(done) = self.shared.flushed.lock().take() {
            let _ = done.send(());
        }
        Ok(())
    }

    fn OnEvent(&self, _stream: u32, _event: Option<&IMFMediaEvent>) -> WinResult<()> {
        Ok(())
    }
}

struct Handle {
    shared: Arc<Shared>,
    source: SendPtr<IMFMediaSource>,
}

impl Source for Handle {
    fn close(self: Box<Self>) {
        drop(self);
    }

    fn size(&self) -> (u32, u32) {
        *self.shared.size.lock()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        let (done, flushed) = mpsc::channel();
        *self.shared.flushed.lock() = Some(done);
        let reader = self.shared.reader.lock().take();
        if let Some(reader) = &reader {
            if unsafe { reader.Flush(FIRST_VIDEO) }.is_ok() {
                let _ = flushed.recv_timeout(Duration::from_secs(1));
            }
        }
        // The camera itself takes about two seconds to stop (C920); the cam
        // helper thread waits it out, never the worker.
        unsafe {
            let _ = self.source.Shutdown();
        }
        drop(reader);
    }
}

pub fn open(device: &CameraDevice, mode: OpenMode, sink: Sink) -> Result<Box<dyn Source>, Error> {
    ensure_mf();
    unsafe { open_reader(device, mode, sink) }
}

unsafe fn open_reader(device: &CameraDevice, mode: OpenMode, sink: Sink) -> Result<Box<dyn Source>, Error> {
    let source = media_source(&device.id, mode).map_err(|err| classify(err.code().0))?;
    let shared = Arc::new(Shared {
        reader: Mutex::new(None),
        sink: Mutex::new(sink),
        stop: AtomicBool::new(false),
        flushed: Mutex::new(None),
        size: Mutex::new((0, 0)),
        stride: Mutex::new(0),
        lost: AtomicBool::new(false),
    });
    let callback: IMFSourceReaderCallback = Callback { shared: shared.clone() }.into();
    let build = || -> WinResult<IMFSourceReader> {
        let attrs = attributes(2)?;
        attrs.SetUnknown(&MF_SOURCE_READER_ASYNC_CALLBACK, &callback)?;
        // The reader inserts decoders by itself, colour converters only with
        // this; a YUY2 camera would not reach NV12 without it.
        attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
        let reader = MFCreateSourceReaderFromMediaSource(&source, &attrs)?;
        if let OpenMode::Controlling { width, height } = mode {
            let mut types = Vec::new();
            while let Ok(t) = reader.GetNativeMediaType(FIRST_VIDEO, types.len() as u32) {
                types.push(t);
            }
            let natives: Vec<NativeType> = types.iter().map(|t| native_of(t)).collect();
            if let Some(best) = pick_native(&natives, (width, height)) {
                reader.SetCurrentMediaType(FIRST_VIDEO, None, &types[best])?;
                let n = &natives[best];
                log::info!(
                    "camera: {} runs {} {}x{} at {:.0} fps",
                    device.name,
                    String::from_utf8_lossy(&n.subtype),
                    n.width,
                    n.height,
                    n.fps
                );
            }
        }
        // NV12 at the size the camera runs: a type without a size lets the
        // reader pick another native type to match it (YUY2 640×480 on a C920).
        let current = reader.GetCurrentMediaType(FIRST_VIDEO)?;
        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        if let Ok(size) = current.GetUINT64(&MF_MT_FRAME_SIZE) {
            out.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
        }
        reader.SetCurrentMediaType(FIRST_VIDEO, None, &out)?;
        Ok(reader)
    };
    let reader = match build() {
        Ok(reader) => reader,
        Err(err) => {
            let _ = source.Shutdown();
            return Err(classify(err.code().0));
        }
    };
    *shared.reader.lock() = Some(SendPtr(reader.clone()));
    shared.read_type();
    let handle = Box::new(Handle { shared: shared.clone(), source: SendPtr(source) });
    if let Err(err) = reader.ReadSample(FIRST_VIDEO, 0, None, None, None, None) {
        return Err(classify(err.code().0));
    }
    Ok(handle)
}

#[implement(IMFSensorActivitiesReportCallback)]
struct Reports {
    link: String,
    found: Mutex<Option<mpsc::Sender<Vec<u32>>>>,
}

impl IMFSensorActivitiesReportCallback_Impl for Reports_Impl {
    fn OnActivitiesReport(&self, report: Option<&IMFSensorActivitiesReport>) -> WinResult<()> {
        let Some(done) = self.found.lock().clone() else { return Ok(()) };
        let mut pids = Vec::new();
        if let Some(report) = report {
            unsafe {
                for i in 0..report.GetCount().unwrap_or(0) {
                    let Ok(activity) = report.GetActivityReport(i) else { continue };
                    let mut buf = [0u16; 512];
                    let mut written = 0u32;
                    if activity.GetSymbolicLink(&mut buf, &mut written).is_err() {
                        continue;
                    }
                    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
                    if !String::from_utf16_lossy(&buf[..end]).eq_ignore_ascii_case(&self.link) {
                        continue;
                    }
                    for j in 0..activity.GetProcessCount().unwrap_or(0) {
                        if let Ok(pid) = activity.GetProcessActivity(j).and_then(|p| p.GetProcessId()) {
                            pids.push(pid);
                        }
                    }
                }
            }
        }
        let _ = done.send(pids);
        Ok(())
    }
}

/// The app that streams `device`, by its file name ("Discord"), from Windows'
/// sensor activity monitor. Blocks up to 500 ms: only the cam helper calls it.
pub fn holder(device: &CameraDevice) -> Option<String> {
    ensure_mf();
    let (done, found) = mpsc::channel();
    unsafe {
        let callback: IMFSensorActivitiesReportCallback =
            Reports { link: device.id.clone(), found: Mutex::new(Some(done)) }.into();
        let monitor = MFCreateSensorActivityMonitor(&callback).ok()?;
        monitor.Start().ok()?;
        // The first report comes at once and is often empty; the one with the
        // streams follows right after it.
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        let mut pids = Vec::new();
        while pids.is_empty() {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match found.recv_timeout(left) {
                Ok(report) => pids = report,
                Err(_) => break,
            }
        }
        let _ = monitor.Stop();
        let me = std::process::id();
        pids.into_iter().find(|pid| *pid != me).and_then(|pid| process_name(pid))
    }
}

unsafe fn process_name(pid: u32) -> Option<String> {
    let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, BOOL(0), pid).ok()?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let named = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
    let _ = CloseHandle(process);
    named.ok()?;
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned())
}

/// Cameras plugged in or out. Unregistered on drop.
pub struct ArrivalWatch {
    handle: SendPtr<HCMNOTIFICATION>,
    _context: Box<crossbeam_channel::Sender<()>>,
}

impl Drop for ArrivalWatch {
    fn drop(&mut self) {
        // Waits for a callback in flight, so the context outlives every call.
        unsafe {
            let _ = CM_Unregister_Notification(self.handle.0);
        }
    }
}

unsafe extern "system" fn on_notify(
    _handle: HCMNOTIFICATION,
    context: *const c_void,
    _action: CM_NOTIFY_ACTION,
    _data: *const CM_NOTIFY_EVENT_DATA,
    _size: u32,
) -> u32 {
    if let Some(tx) = (context as *const crossbeam_channel::Sender<()>).as_ref() {
        let _ = tx.send(());
    }
    0
}

/// Calls `on_change` 500 ms after the last camera came or went.
pub fn watch_arrivals(on_change: impl Fn() + Send + Sync + 'static) -> Option<ArrivalWatch> {
    let (tx, rx) = crossbeam_channel::unbounded::<()>();
    std::thread::Builder::new()
        .name("clippiboy-cam-arrivals".into())
        .spawn(move || {
            while rx.recv().is_ok() {
                loop {
                    match rx.recv_timeout(Duration::from_millis(500)) {
                        Ok(()) => continue,
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => break,
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
                    }
                }
                on_change();
            }
        })
        .ok()?;
    let context = Box::new(tx);
    let filter = CM_NOTIFY_FILTER {
        cbSize: std::mem::size_of::<CM_NOTIFY_FILTER>() as u32,
        FilterType: CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
        u: CM_NOTIFY_FILTER_0 { DeviceInterface: CM_NOTIFY_FILTER_0_2 { ClassGuid: KSCATEGORY_VIDEO_CAMERA } },
        ..Default::default()
    };
    let mut handle = HCMNOTIFICATION::default();
    let result = unsafe {
        CM_Register_Notification(&filter, Some(&*context as *const _ as *const c_void), Some(on_notify), &mut handle)
    };
    (result == CR_SUCCESS).then(|| ArrivalWatch { handle: SendPtr(handle), _context: context })
}
