//! Windows.Graphics.Capture, driven by hand.
//!
//! This used to run through `windows-capture`. Its frame pool is fixed at
//! **one** buffer, and its encoder only passes the COM pointer to the texture
//! along and reads it later — by which time WGC has long overwritten that same
//! texture with the next frame. That is exactly where the duplicated and torn
//! frames came from.
//!
//! So both things hold here: the pool has two buffers, and the callback gets the
//! texture **synchronously**, while it is valid. Whoever wants to keep it copies
//! it there.

#![cfg(windows)]

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use windows::core::Interface;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

use crate::gpu::{GpuDevice, SendPtr};
use crate::model::TargetKind;

/// One captured frame as the callback sees it.
pub struct CapturedFrame<'a> {
    pub texture: &'a ID3D11Texture2D,
    pub width: u32,
    pub height: u32,
    /// QPC in 100 ns units. **The same clock** WASAPI supplies through
    /// `pu64QPCPosition` — audio/video sync rests on that.
    pub qpc_100ns: i64,
}

/// Lives as long as the capture runs. Unregisters when dropped.
pub struct Capture {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    token: windows::Foundation::EventRegistrationToken,
    /// Has to stay alive, otherwise nobody reports that the source is gone.
    item: GraphicsCaptureItem,
    closed_token: windows::Foundation::EventRegistrationToken,
}

// All the WinRT objects involved are agile, and the D3D11 device underneath is
// multithread-protected (see `gpu.rs`).
unsafe impl Send for Capture {}
unsafe impl Sync for Capture {}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.item.RemoveClosed(self.closed_token);
        let _ = self.pool.RemoveFrameArrived(self.token);
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

/// HMONITOR for the device name (`\\.\DISPLAY1`), otherwise the primary screen.
fn find_monitor(device_name: Option<&str>) -> Result<HMONITOR, String> {
    struct Search {
        wanted: Option<String>,
        hit: Option<HMONITOR>,
        primary: Option<HMONITOR>,
    }

    unsafe extern "system" fn proc(
        monitor: HMONITOR,
        _dc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let search = &mut *(data.0 as *mut Search);
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(monitor, &mut info.monitorInfo as *mut _).as_bool() {
            return TRUE;
        }
        // MONITORINFOF_PRIMARY — not exported by the windows crate 0.58.
        if info.monitorInfo.dwFlags & 0x0000_0001 != 0 && search.primary.is_none() {
            search.primary = Some(monitor);
        }
        if let Some(wanted) = search.wanted.as_deref() {
            let name = String::from_utf16_lossy(&info.szDevice)
                .trim_end_matches('\0')
                .to_string();
            if name == wanted {
                search.hit = Some(monitor);
            }
        }
        TRUE
    }

    let mut search = Search {
        wanted: device_name.map(str::to_string),
        hit: None,
        primary: None,
    };
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(proc),
            LPARAM(&mut search as *mut _ as isize),
        );
    }
    // Unplugged or renamed screen: better to capture the primary one than not to
    // buffer at all — as before.
    search
        .hit
        .or(search.primary)
        .ok_or_else(|| "no screen found".to_string())
}

fn capture_item(kind: TargetKind, id: Option<&str>) -> Result<GraphicsCaptureItem, String> {
    let interop: IGraphicsCaptureItemInterop = windows::core::factory::<
        GraphicsCaptureItem,
        IGraphicsCaptureItemInterop,
    >()
    .map_err(|err| format!("Capture-Interop: {err}"))?;

    match kind {
        TargetKind::Monitor => {
            let monitor = find_monitor(id)?;
            unsafe { interop.CreateForMonitor(monitor) }
                .map_err(|err| format!("Bildschirm aufnehmen: {err}"))
        }
        TargetKind::Window => {
            let id = id.ok_or_else(|| "no window selected".to_string())?;
            let raw = id
                .trim_start_matches("0x")
                .trim_start_matches("0X");
            let handle = usize::from_str_radix(raw, 16)
                .map_err(|_| format!("unusable window id: {id}"))?;
            if handle == 0 {
                return Err("The selected window is no longer open.".into());
            }
            unsafe { interop.CreateForWindow(HWND(handle as *mut _)) }
                .map_err(|_| "The selected window is no longer open.".to_string())
        }
    }
}

/// Start capturing.
///
/// `on_frame` runs on a thread pool thread and has to be short — the texture is
/// only valid while it does. `on_closed` reports that the source has gone away
/// (window closed, screen unplugged); no frame arrives after that.
pub fn start<F, C>(
    gpu: &GpuDevice,
    kind: TargetKind,
    id: Option<&str>,
    fps: u32,
    on_frame: F,
    on_closed: C,
) -> Result<Capture, String>
where
    F: FnMut(CapturedFrame<'_>) + Send + 'static,
    C: Fn() + Send + 'static,
{
    let item = capture_item(kind, id)?;
    let size = item.Size().map_err(|err| format!("source size: {err}"))?;

    // Two buffers instead of one: WGC may already write the next frame while the
    // callback is still working on the previous one.
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &gpu.winrt,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        2,
        size,
    )
    .map_err(|err| format!("Frame-Pool: {err}"))?;

    let session = pool
        .CreateCaptureSession(&item)
        .map_err(|err| format!("Capture-Sitzung: {err}"))?;

    let _ = session.SetIsCursorCaptureEnabled(true);
    // Both only exist from Windows 11 on. If they are missing, the default
    // behaviour stands — no reason to let the capture fail.
    let _ = session.SetIsBorderRequired(false);
    if fps > 0 {
        let interval: windows::Foundation::TimeSpan =
            Duration::from_nanos(1_000_000_000 / fps as u64).into();
        let _ = session.SetMinUpdateInterval(interval);
    }

    let width = AtomicI32::new(size.Width);
    let height = AtomicI32::new(size.Height);
    let state = Arc::new((Mutex::new(on_frame), width, height));

    let handler = TypedEventHandler::<Direct3D11CaptureFramePool, windows::core::IInspectable>::new({
        let state = state.clone();
        let winrt = SendPtr(gpu.winrt.clone());
        move |pool, _| {
            let Some(pool) = pool.as_ref() else {
                return Ok(());
            };
            let frame = pool.TryGetNextFrame()?;
            let content = frame.ContentSize()?;

            // Size changed (window resized, resolution switched): the pool has to
            // be recreated, and this frame still hangs off the old one. Release
            // first, then recreate — otherwise the old texture stays alive.
            let (last_w, last_h) = (&state.1, &state.2);
            if content.Width != last_w.load(Ordering::Relaxed)
                || content.Height != last_h.load(Ordering::Relaxed)
            {
                drop(frame);
                pool.Recreate(
                    &*winrt,
                    DirectXPixelFormat::B8G8R8A8UIntNormalized,
                    2,
                    content,
                )?;
                last_w.store(content.Width, Ordering::Relaxed);
                last_h.store(content.Height, Ordering::Relaxed);
                return Ok(());
            }

            let surface = frame.Surface()?;
            let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
            let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
            let qpc_100ns = frame.SystemRelativeTime()?.Duration;

            (state.0.lock())(CapturedFrame {
                texture: &texture,
                width: content.Width.max(0) as u32,
                height: content.Height.max(0) as u32,
                qpc_100ns,
            });
            Ok(())
        }
    });

    let token = pool
        .FrameArrived(&handler)
        .map_err(|err| format!("FrameArrived: {err}"))?;

    // Without this a dead source goes unnoticed: the clock then repeats the last
    // frame for all eternity, and what gets recorded is a still image with nothing
    // anywhere saying so.
    let closed_token = item
        .Closed(&TypedEventHandler::<GraphicsCaptureItem, windows::core::IInspectable>::new(
            move |_, _| {
                on_closed();
                Ok(())
            },
        ))
        .map_err(|err| format!("Closed: {err}"))?;

    session
        .StartCapture()
        .map_err(|err| format!("start capture: {err}"))?;

    Ok(Capture {
        pool,
        session,
        token,
        item,
        closed_token,
    })
}
