//! Encoder selection.
//!
//! We ask the Media Foundation registry, not the GPU. That distinction
//! matters: the old code only asked DXGI for the graphics card vendor and
//! concluded from that its encoder was available. But an NVIDIA card in the
//! machine does not mean its H.264 MFT is registered — with a missing or
//! stripped-down driver the UI would then list an encoder that does not exist.
//!
//! What made it worse: recording never read that selection anyway. It does now
//! (see `mft.rs`), so the list has to be right.

use crate::model::{EncoderId, EncoderInfo};

#[cfg(windows)]
fn hardware_encoders() -> Vec<EncoderId> {
    crate::mft::available_encoders()
}

#[cfg(not(windows))]
fn hardware_encoders() -> Vec<EncoderId> {
    Vec::new()
}

pub fn list_encoders() -> Vec<EncoderInfo> {
    let hardware = hardware_encoders();
    let has = |id: EncoderId| hardware.contains(&id);

    vec![
        EncoderInfo {
            id: EncoderId::Nvenc,
            name: "NVIDIA NVENC (H.264)".into(),
            available: has(EncoderId::Nvenc),
            hardware: true,
        },
        EncoderInfo {
            id: EncoderId::Amf,
            name: "AMD AMF (H.264)".into(),
            available: has(EncoderId::Amf),
            hardware: true,
        },
        EncoderInfo {
            id: EncoderId::Qsv,
            name: "Intel QuickSync (H.264)".into(),
            available: has(EncoderId::Qsv),
            hardware: true,
        },
        EncoderInfo {
            // For recording this is the Windows software H.264 MFT, for export
            // it is x264. Both are always there.
            id: EncoderId::X264,
            name: "Software (CPU, fallback)".into(),
            available: true,
            hardware: false,
        },
    ]
}

/// Best available encoder — hardware before CPU.
pub fn preferred_encoder() -> EncoderId {
    list_encoders()
        .into_iter()
        .find(|e| e.available && e.hardware)
        .map(|e| e.id)
        .unwrap_or(EncoderId::X264)
}

/// Falls back to an available encoder if the requested one is missing (a
/// config carried over from another machine, for instance).
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
    fn software_is_always_available() {
        let encoders = list_encoders();
        let software = encoders.iter().find(|e| e.id == EncoderId::X264).unwrap();
        assert!(software.available);
    }

    #[test]
    fn unavailable_request_falls_back() {
        // With no matching encoder, one that is not there must never come back.
        let resolved = resolve(EncoderId::Nvenc);
        let encoders = list_encoders();
        assert!(encoders.iter().any(|e| e.id == resolved && e.available));
    }
}
