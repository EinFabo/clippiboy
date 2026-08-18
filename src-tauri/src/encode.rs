//! Encoder-Auswahl. Welcher Hardware-Encoder überhaupt in Frage kommt, hängt
//! am GPU-Hersteller — der wird über DXGI ermittelt. x264 ist immer verfügbar.

use crate::model::{EncoderId, EncoderInfo};

const VENDOR_NVIDIA: u32 = 0x10DE;
const VENDOR_AMD: u32 = 0x1002;
const VENDOR_INTEL: u32 = 0x8086;

#[cfg(windows)]
fn gpu_vendors() -> Vec<u32> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};

    let mut vendors = Vec::new();
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return vendors;
        };
        let mut index = 0;
        while let Ok(adapter) = factory.EnumAdapters1(index) {
            if let Ok(desc) = adapter.GetDesc1() {
                if !vendors.contains(&desc.VendorId) {
                    vendors.push(desc.VendorId);
                }
            }
            index += 1;
        }
    }
    vendors
}

#[cfg(not(windows))]
fn gpu_vendors() -> Vec<u32> {
    Vec::new()
}

pub fn list_encoders() -> Vec<EncoderInfo> {
    let vendors = gpu_vendors();
    let has = |vendor: u32| vendors.contains(&vendor);

    vec![
        EncoderInfo {
            id: EncoderId::Nvenc,
            name: "NVIDIA NVENC (H.264)".into(),
            available: has(VENDOR_NVIDIA),
            hardware: true,
        },
        EncoderInfo {
            id: EncoderId::Amf,
            name: "AMD AMF (H.264)".into(),
            available: has(VENDOR_AMD),
            hardware: true,
        },
        EncoderInfo {
            id: EncoderId::Qsv,
            name: "Intel QuickSync (H.264)".into(),
            available: has(VENDOR_INTEL),
            hardware: true,
        },
        EncoderInfo {
            id: EncoderId::X264,
            name: "x264 (CPU, Fallback)".into(),
            available: true,
            hardware: false,
        },
    ]
}

/// Bester verfügbarer Encoder — Hardware vor CPU.
pub fn preferred_encoder() -> EncoderId {
    list_encoders()
        .into_iter()
        .find(|e| e.available && e.hardware)
        .map(|e| e.id)
        .unwrap_or(EncoderId::X264)
}

/// Fällt auf einen verfügbaren Encoder zurück, falls der gewünschte fehlt
/// (z.B. Konfiguration von einem anderen Rechner übernommen).
pub fn resolve(requested: EncoderId) -> EncoderId {
    let available = list_encoders();
    if available.iter().any(|e| e.id == requested && e.available) {
        requested
    } else {
        preferred_encoder()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x264_is_always_available() {
        let encoders = list_encoders();
        let x264 = encoders.iter().find(|e| e.id == EncoderId::X264).unwrap();
        assert!(x264.available);
    }

    #[test]
    fn unavailable_request_falls_back() {
        // Ohne passende GPU darf nie ein nicht vorhandener Encoder zurückkommen.
        let resolved = resolve(EncoderId::Nvenc);
        let encoders = list_encoders();
        assert!(encoders.iter().any(|e| e.id == resolved && e.available));
    }
}
