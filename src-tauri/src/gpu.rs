//! The D3D11 device that capture, colour conversion and the encoder all share.
//!
//! It has to satisfy all three, and that is where the earlier approach failed:
//! `windows-capture` creates its device with `BGRA_SUPPORT` only. Without
//! `VIDEO_SUPPORT` there is no `ID3D11VideoProcessor` on it (BGRA→NV12), and a
//! hardware encoder MFT will not reliably accept a device without multithread
//! protection — together those two forced the old path through the
//! `MediaTranscoder`, which accepts neither rate control nor a GOP distance.
//!
//! **The adapter is chosen, not taken.** `D3D11CreateDevice` with no adapter
//! takes DXGI's first one, and on a machine with two graphics cards that is
//! simply whichever Windows enumerates first — it has nothing to do with the
//! encoder that was picked. On this developer machine adapter 0 is the NVIDIA
//! card and adapter 1 the Intel iGPU, so choosing Quick Sync handed the *Intel*
//! MFT an *NVIDIA* device and its textures. That is not a slow path, it is a
//! wrong one. Every hardware encoder therefore gets the adapter of its own
//! vendor; the software encoder has no vendor and keeps the default.

#![cfg(windows)]

use windows::core::Interface;
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0,
    D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIDevice, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE,
};
use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice;

use crate::model::EncoderId;

/// PCI vendor ids, the way DXGI reports them in `DXGI_ADAPTER_DESC1::VendorId`.
///
/// Media Foundation states the same three as text (`"VEN_10DE"`); `mft.rs`
/// formats them from here rather than spelling them out a second time.
pub const VENDOR_NVIDIA: u32 = 0x10DE;
pub const VENDOR_AMD: u32 = 0x1002;
pub const VENDOR_INTEL: u32 = 0x8086;

/// Which vendor has to supply the adapter for this encoder. The software
/// encoder has none — it runs wherever it is put.
pub fn vendor_for(encoder: EncoderId) -> Option<u32> {
    match encoder {
        EncoderId::Nvenc => Some(VENDOR_NVIDIA),
        EncoderId::Amf => Some(VENDOR_AMD),
        EncoderId::Qsv => Some(VENDOR_INTEL),
        EncoderId::X264 => None,
    }
}

/// One graphics adapter, as far as we care about it.
#[derive(Debug, Clone)]
pub struct AdapterInfo {
    pub index: u32,
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    /// Memory on the card itself. Nought means the GPU borrows main memory,
    /// which is how an integrated one looks.
    pub dedicated_vram: u64,
}

impl std::fmt::Display for AdapterInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "#{} {} ({:04x}:{:04x}, {} MB dedicated)",
            self.index,
            self.name,
            self.vendor_id,
            self.device_id,
            self.dedicated_vram / (1024 * 1024)
        )
    }
}

/// Every hardware adapter DXGI knows, in its own order.
///
/// The Basic Render Driver is left out: it is a software adapter that cannot
/// create a device at all here, and it would only ever be a wrong answer.
pub fn adapters() -> Vec<(AdapterInfo, IDXGIAdapter1)> {
    let factory: IDXGIFactory1 = match unsafe { CreateDXGIFactory1() } {
        Ok(factory) => factory,
        Err(err) => {
            log::warn!("DXGI factory: {err} — falling back to the default adapter");
            return Vec::new();
        }
    };

    let mut found = Vec::new();
    for index in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        // A bit, not a value: an adapter may be flagged remote as well, and
        // comparing the whole field for equality would then let the software
        // one through.
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let end = desc
            .Description
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(desc.Description.len());
        found.push((
            AdapterInfo {
                index,
                name: String::from_utf16_lossy(&desc.Description[..end]),
                vendor_id: desc.VendorId,
                device_id: desc.DeviceId,
                dedicated_vram: desc.DedicatedVideoMemory as u64,
            },
            adapter,
        ));
    }
    found
}

/// The adapter this encoder has to run on, or `None` for "whatever DXGI hands
/// out first".
///
/// Two cases have to come out right, which is why it is not simply the first
/// matching vendor:
///
/// * Two makes in one machine (NVIDIA plus an Intel iGPU, say) — the encoder
///   decides, otherwise it gets a device from the wrong company.
/// * Two cards of the **same** make, which is every Ryzen laptop: a Radeon iGPU
///   and a Radeon dGPU both answer `VEN_1002`. Then the one with real memory of
///   its own wins, because that is the card the game is drawn on and the only
///   one that spares the picture a trip across the bus.
fn pick(encoder: EncoderId, list: &[(AdapterInfo, IDXGIAdapter1)]) -> Option<usize> {
    let vendor = vendor_for(encoder)?;
    list.iter()
        .enumerate()
        .filter(|(_, (info, _))| info.vendor_id == vendor)
        .max_by_key(|(_, (info, _))| info.dedicated_vram)
        .map(|(index, _)| index)
}

/// Device plus context. The context is multithread-protected, so any thread
/// may use it.
pub struct GpuDevice {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    /// The same hardware as a WinRT device — that is what the frame pool wants.
    pub winrt: IDirect3DDevice,
    /// Which card this actually landed on. `None` means DXGI chose, because
    /// nothing was asked for or nothing matched. It belongs in the log next to
    /// the encoder: "which encoder" and "on which card" are one question.
    pub adapter: Option<AdapterInfo>,
}

// The multithread protection below makes exactly this promise: the COM objects
// may be used across thread boundaries.
unsafe impl Send for GpuDevice {}
unsafe impl Sync for GpuDevice {}

impl GpuDevice {
    /// `prefer` names the encoder that will run on this device, so its vendor
    /// can be given the adapter. `None` leaves the choice to DXGI — that is
    /// right for the screenshot path, which encodes nothing.
    pub fn new(prefer: Option<EncoderId>) -> Result<Self, String> {
        let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        let mut level = D3D_FEATURE_LEVEL::default();

        let list = match prefer {
            Some(_) => adapters(),
            // Nothing to match against, so nothing to enumerate for.
            None => Vec::new(),
        };
        let chosen = prefer.and_then(|encoder| pick(encoder, &list));
        if let (Some(encoder), None) = (prefer, chosen) {
            if vendor_for(encoder).is_some() && !list.is_empty() {
                log::warn!(
                    "no {encoder:?} adapter among [{}] — taking the default one, which is \
                     very likely the wrong card",
                    list.iter()
                        .map(|(info, _)| info.to_string())
                        .collect::<Vec<_>>()
                        .join("; ")
                );
            }
        }
        let adapter = chosen.map(|index| list[index].0.clone());
        if let Some(info) = &adapter {
            log::info!("graphics adapter for {:?}: {info}", prefer.unwrap());
        }

        unsafe {
            D3D11CreateDevice(
                chosen.map(|index| &list[index].1).map(|a| a.into()),
                // A named adapter and a driver type are mutually exclusive:
                // D3D11 rejects anything but UNKNOWN once an adapter is given.
                match chosen {
                    Some(_) => D3D_DRIVER_TYPE_UNKNOWN,
                    None => D3D_DRIVER_TYPE_HARDWARE,
                },
                None,
                // BGRA because that is how Windows.Graphics.Capture delivers;
                // VIDEO because the VideoProcessor turns that into NV12.
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                Some(&mut level),
                Some(&mut context),
            )
            .map_err(|err| format!("D3D11 device: {err}"))?;
        }

        let device = device.ok_or_else(|| "D3D11 device missing".to_string())?;
        let context = context.ok_or_else(|| "D3D11 context missing".to_string())?;

        // Mandatory as soon as an MFT works on the same device: encoder,
        // VideoProcessor and the capture callback live on different threads.
        // Without this there are sporadic crashes deep inside the driver.
        let multithread: ID3D11Multithread = context
            .cast()
            .map_err(|err| format!("ID3D11Multithread: {err}"))?;
        unsafe { let _ = multithread.SetMultithreadProtected(true); }

        let dxgi: IDXGIDevice = device
            .cast()
            .map_err(|err| format!("IDXGIDevice: {err}"))?;
        let winrt = unsafe {
            CreateDirect3D11DeviceFromDXGIDevice(&dxgi)
                .map_err(|err| format!("WinRT device: {err}"))?
        };
        let winrt: IDirect3DDevice = winrt
            .cast()
            .map_err(|err| format!("IDirect3DDevice: {err}"))?;

        Ok(Self {
            device,
            context,
            winrt,
            adapter,
        })
    }
}

/// Wrapper that carries a COM object across a thread boundary.
///
/// The WinRT objects here are agile and the device underneath is
/// multithread-protected; Rust's type system sees only the raw pointer and
/// takes it for unsendable.
pub struct SendPtr<T>(pub T);

unsafe impl<T> Send for SendPtr<T> {}
unsafe impl<T> Sync for SendPtr<T> {}

impl<T> std::ops::Deref for SendPtr<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
