//! Windows.Graphics.Capture in eigener Regie.
//!
//! Vorher lief das über `windows-capture`. Dessen Frame-Pool ist auf **einen**
//! Puffer festgelegt, und sein Encoder reicht nur den COM-Zeiger auf die
//! Textur weiter und liest sie später — bis dahin hat WGC dieselbe Textur
//! längst mit dem nächsten Bild überschrieben. Genau daher kamen die
//! doppelten und zerrissenen Bilder.
//!
//! Hier gilt deshalb beides: Der Pool hat zwei Puffer, und der Rückruf
//! bekommt die Textur **synchron**, solange sie gültig ist. Wer sie behalten
//! will, kopiert sie dort.

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

/// Ein aufgenommenes Bild, wie es der Rückruf sieht.
pub struct CapturedFrame<'a> {
    pub texture: &'a ID3D11Texture2D,
    pub width: u32,
    pub height: u32,
    /// QPC in 100-ns-Einheiten. **Dieselbe Uhr**, die WASAPI über
    /// `pu64QPCPosition` liefert — darauf beruht die Bild-Ton-Synchronität.
    pub qpc_100ns: i64,
}

/// Läuft, solange die Aufnahme läuft. Beim Fallenlassen wird abgemeldet.
pub struct Capture {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    token: windows::Foundation::EventRegistrationToken,
    /// Muss am Leben bleiben, sonst meldet niemand mehr, dass die Quelle weg
    /// ist.
    item: GraphicsCaptureItem,
    closed_token: windows::Foundation::EventRegistrationToken,
}

// Alle beteiligten WinRT-Objekte sind agil, und das D3D11-Gerät darunter ist
// multithread-geschützt (siehe `gpu.rs`).
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

/// HMONITOR zum Gerätenamen (`\\.\DISPLAY1`), sonst der primäre Bildschirm.
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
        // MONITORINFOF_PRIMARY — im windows-Crate 0.58 nicht exportiert.
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
    // Abgestöpselter oder umbenannter Bildschirm: lieber den primären
    // aufnehmen als gar nicht puffern — wie bisher auch.
    search
        .hit
        .or(search.primary)
        .ok_or_else(|| "Kein Bildschirm gefunden".to_string())
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
            let id = id.ok_or_else(|| "Kein Fenster ausgewählt".to_string())?;
            let raw = id
                .trim_start_matches("0x")
                .trim_start_matches("0X");
            let handle = usize::from_str_radix(raw, 16)
                .map_err(|_| format!("Unbrauchbare Fenster-Kennung: {id}"))?;
            if handle == 0 {
                return Err("Das gewählte Fenster ist nicht mehr offen.".into());
            }
            unsafe { interop.CreateForWindow(HWND(handle as *mut _)) }
                .map_err(|_| "Das gewählte Fenster ist nicht mehr offen.".to_string())
        }
    }
}

/// Aufnahme starten.
///
/// `on_frame` läuft auf einem Threadpool-Faden und muss kurz sein — die Textur
/// ist nur währenddessen gültig. `on_closed` meldet, dass die Quelle
/// verschwunden ist (Fenster zu, Bildschirm abgestöpselt); danach kommt kein
/// Bild mehr.
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
    let size = item.Size().map_err(|err| format!("Quellgröße: {err}"))?;

    // Zwei Puffer statt einem: WGC darf das nächste Bild schon schreiben,
    // während der Rückruf noch am vorherigen arbeitet.
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
    // Beide erst ab Windows 11 vorhanden. Fehlen sie, bleibt es beim
    // Standardverhalten — kein Grund, die Aufnahme scheitern zu lassen.
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

            // Größe geändert (Fenster skaliert, Auflösung gewechselt): Der Pool
            // muss neu, und dieses Bild hängt noch am alten. Erst freigeben,
            // dann neu anlegen — sonst bleibt die alte Textur am Leben.
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

    // Ohne das bleibt eine tote Quelle unbemerkt: Der Taktgeber wiederholt
    // dann bis in alle Ewigkeit das letzte Bild, und aufgenommen wird ein
    // Standbild, ohne dass irgendwo etwas davon steht.
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
        .map_err(|err| format!("Aufnahme starten: {err}"))?;

    Ok(Capture {
        pool,
        session,
        token,
        item,
        closed_token,
    })
}
