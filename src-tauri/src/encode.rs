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

/// Bits spent per pixel and frame.
///
/// 0.15 puts 1080p60 at a good 19 Mbit/s. That is the point where H.264 stops
/// paying for more on game footage — the previous fixed 40 Mbit/s was roughly
/// twice what the picture could use, and it cost that whether the screen showed
/// a firefight or a menu.
const BITS_PER_PIXEL: f32 = 0.15;

/// Sensible bitrate for a picture of this size and speed.
///
/// The old default was a single number for every resolution, so switching to
/// 720p30 kept the 40 Mbit/s that had been meant for 1080p60 — four times the
/// pixels per second, same budget. Deriving it means the setting follows the
/// picture instead of standing next to it.
pub fn bitrate_for(width: u32, height: u32, fps: u32) -> u32 {
    let pixels_per_second = width as u64 * height as u64 * fps.max(1) as u64;
    let kbps = (pixels_per_second as f32 * BITS_PER_PIXEL / 1000.0) as u64;
    // To the nearest megabit — a number nobody has to read twice.
    let rounded = (kbps + 500) / 1000 * 1000;
    rounded.clamp(2_000, 100_000) as u32
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

    /// The steps that actually get picked in the settings. A regression here
    /// changes every new user's clip size, so the numbers are spelled out.
    #[test]
    fn the_usual_resolutions_land_where_they_should() {
        assert_eq!(bitrate_for(1280, 720, 30), 4_000);
        assert_eq!(bitrate_for(1920, 1080, 60), 19_000);
        assert_eq!(bitrate_for(2560, 1440, 60), 33_000);
        assert_eq!(bitrate_for(3840, 2160, 60), 75_000);
    }

    /// Twice the frames is twice the picture to pay for.
    #[test]
    fn more_frames_cost_more() {
        assert!(bitrate_for(1920, 1080, 60) > bitrate_for(1920, 1080, 30));
        assert!(bitrate_for(2560, 1440, 60) > bitrate_for(1920, 1080, 60));
    }

    /// Nothing may fall out of the range the encoder and the UI agree on — a
    /// tiny window must not end up at a bitrate no encoder accepts.
    #[test]
    fn the_result_stays_in_range() {
        assert_eq!(bitrate_for(2, 2, 1), 2_000);
        assert_eq!(bitrate_for(7680, 4320, 120), 100_000);
    }

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
