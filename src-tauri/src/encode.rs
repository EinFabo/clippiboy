//! Encoder-Auswahl.
//!
//! Gefragt wird die Media-Foundation-Registrierung, nicht die GPU. Das ist ein
//! Unterschied: Früher wurde über DXGI nur der Hersteller der Grafikkarte
//! ermittelt und daraus geschlossen, dass ihr Encoder verfügbar sei. Eine
//! NVIDIA-Karte im Rechner heißt aber nicht, dass ihr H.264-MFT angemeldet ist
//! — bei fehlendem oder abgespecktem Treiber steht dann in der Oberfläche ein
//! Encoder, den es nicht gibt.
//!
//! Erschwerend kam hinzu, dass die Aufnahme diese Auswahl ohnehin nie gelesen
//! hat. Das tut sie jetzt (siehe `mft.rs`), also muss die Liste stimmen.

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
            // Bei der Aufnahme ist das der Software-H.264-MFT von Windows,
            // beim Export x264. Beide sind immer da.
            id: EncoderId::X264,
            name: "Software (CPU, Fallback)".into(),
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
    fn software_is_always_available() {
        let encoders = list_encoders();
        let software = encoders.iter().find(|e| e.id == EncoderId::X264).unwrap();
        assert!(software.available);
    }

    #[test]
    fn unavailable_request_falls_back() {
        // Ohne passenden Encoder darf nie ein nicht vorhandener zurückkommen.
        let resolved = resolve(EncoderId::Nvenc);
        let encoders = list_encoders();
        assert!(encoders.iter().any(|e| e.id == resolved && e.available));
    }
}
