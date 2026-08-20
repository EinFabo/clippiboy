//! Das D3D11-Gerät, auf dem Aufnahme, Farbumwandlung und Encoder gemeinsam
//! laufen.
//!
//! Es muss allen dreien genügen, und daran ist der frühere Weg gescheitert:
//! `windows-capture` legt sein Gerät nur mit `BGRA_SUPPORT` an. Ohne
//! `VIDEO_SUPPORT` gibt es darauf keinen `ID3D11VideoProcessor` (BGRA→NV12),
//! und ein Hardware-Encoder-MFT nimmt ein Gerät ohne Multithread-Schutz nicht
//! zuverlässig an — beides zusammen zwang den alten Pfad über den
//! `MediaTranscoder`, der weder Ratensteuerung noch GOP-Abstand annimmt.

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

/// Gerät samt Kontext. Der Kontext ist multithread-geschützt, es darf ihn
/// deshalb jeder Faden benutzen.
pub struct GpuDevice {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    /// Dieselbe Hardware als WinRT-Gerät — das verlangt der Frame-Pool.
    pub winrt: IDirect3DDevice,
}

// Der Multithread-Schutz unten macht genau diese Zusage: Die COM-Objekte
// dürfen über Fadengrenzen benutzt werden.
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
                // BGRA, weil Windows.Graphics.Capture so liefert; VIDEO, weil
                // der VideoProcessor daraus NV12 macht.
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                Some(&mut level),
                Some(&mut context),
            )
            .map_err(|err| format!("D3D11-Gerät: {err}"))?;
        }

        let device = device.ok_or_else(|| "D3D11-Gerät fehlt".to_string())?;
        let context = context.ok_or_else(|| "D3D11-Kontext fehlt".to_string())?;

        // Pflicht, sobald ein MFT auf demselben Gerät arbeitet: Encoder,
        // VideoProcessor und der Capture-Rückruf liegen auf verschiedenen
        // Fäden. Ohne das gibt es sporadische Abstürze tief im Treiber.
        let multithread: ID3D11Multithread = context
            .cast()
            .map_err(|err| format!("ID3D11Multithread: {err}"))?;
        unsafe { let _ = multithread.SetMultithreadProtected(true); }

        let dxgi: IDXGIDevice = device
            .cast()
            .map_err(|err| format!("IDXGIDevice: {err}"))?;
        let winrt = unsafe {
            CreateDirect3D11DeviceFromDXGIDevice(&dxgi)
                .map_err(|err| format!("WinRT-Gerät: {err}"))?
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

/// Hülle, die ein COM-Objekt über eine Fadengrenze trägt.
///
/// Die WinRT-Objekte hier sind agil und das Gerät darunter ist
/// multithread-geschützt; das Rust-Typsystem sieht davon nur den rohen Zeiger
/// und hält ihn für unversendbar.
pub struct SendPtr<T>(pub T);

unsafe impl<T> Send for SendPtr<T> {}
unsafe impl<T> Sync for SendPtr<T> {}

impl<T> std::ops::Deref for SendPtr<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
