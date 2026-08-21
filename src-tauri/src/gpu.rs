//! The D3D11 device that capture, colour conversion and the encoder all share.
//!
//! It has to satisfy all three, and that is where the earlier approach failed:
//! `windows-capture` creates its device with `BGRA_SUPPORT` only. Without
//! `VIDEO_SUPPORT` there is no `ID3D11VideoProcessor` on it (BGRA→NV12), and a
//! hardware encoder MFT will not reliably accept a device without multithread
//! protection — together those two forced the old path through the
//! `MediaTranscoder`, which accepts neither rate control nor a GOP distance.

#![cfg(windows)]

use windows::core::Interface;
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice;

/// Device plus context. The context is multithread-protected, so any thread
/// may use it.
pub struct GpuDevice {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    /// The same hardware as a WinRT device — that is what the frame pool wants.
    pub winrt: IDirect3DDevice,
}

// The multithread protection below makes exactly this promise: the COM objects
// may be used across thread boundaries.
unsafe impl Send for GpuDevice {}
unsafe impl Sync for GpuDevice {}

impl GpuDevice {
    pub fn new() -> Result<Self, String> {
        let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        let mut level = D3D_FEATURE_LEVEL::default();

        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
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
