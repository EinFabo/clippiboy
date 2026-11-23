//! Which devices each friend has sent from or received on — trust on first
//! use, the first step against a friends server that would put a device of
//! its own in a friend's place (`share.rs` gets each side's iroh id through
//! that server).
//!
//! Since 1.0.1 an installation keeps its iroh id across runs and says so
//! (`ShareOffer.stable`, `ShareAnswer.node`). The ids a clip really went to or
//! came from are kept here per friend, several per friend: one account can
//! run on two PCs. For now that is all — a device that does not fit is only
//! logged, nothing is refused because of it.
//!
//! `peers.json` in the local app data folder. Not secret, so not sealed.

use std::collections::BTreeMap;
use std::path::PathBuf;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

/// Devices kept per friend; the one seen longest ago makes room.
const MAX_DEVICES: usize = 8;

/// One device of a friend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pin {
    /// Its iroh id, as lowercase hex.
    pub node: String,
    pub first_seen: i64,
    pub last_seen: i64,
}

/// Friend id → their devices.
pub type Pins = BTreeMap<String, Vec<Pin>>;

/// How a device fits what is known of a friend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinVerdict {
    /// The friend has no device kept yet.
    New,
    /// One of the friend's devices.
    Known,
    /// The friend has devices kept, and this is none of them: a new PC of
    /// theirs, or someone in their place.
    Changed,
    /// An id that does not last (an app before 1.0.1, or one whose key could
    /// not be kept), or no id at all: nothing to compare or keep.
    Unpinnable,
}

pub fn pin_verdict(pins: &Pins, friend: &str, node: &str, stable: bool) -> PinVerdict {
    if !stable || !is_node(node) {
        return PinVerdict::Unpinnable;
    }
    match pins.get(friend) {
        Some(devices) if devices.iter().any(|pin| pin.node == node) => PinVerdict::Known,
        Some(devices) if !devices.is_empty() => PinVerdict::Changed,
        _ => PinVerdict::New,
    }
}

/// An iroh id as its `Display` writes it: 32 bytes in lowercase hex.
pub fn is_node(node: &str) -> bool {
    node.len() == 64
        && node
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Keeps `node` as one of `friend`'s devices, or marks it seen again.
fn add_pin(pins: &mut Pins, friend: &str, node: &str, now: i64) {
    if !is_node(node) {
        return;
    }
    let devices = pins.entry(friend.to_owned()).or_default();
    if let Some(pin) = devices.iter_mut().find(|pin| pin.node == node) {
        pin.last_seen = pin.last_seen.max(now);
        return;
    }
    while devices.len() >= MAX_DEVICES {
        let oldest = (0..devices.len())
            .min_by_key(|&i| devices[i].last_seen)
            .unwrap_or(0);
        devices.remove(oldest);
    }
    devices.push(Pin {
        node: node.to_owned(),
        first_seen: now,
        last_seen: now,
    });
}

// --- The file ---------------------------------------------------------------------

/// `peers.json` is read and written under this.
static FILE: Mutex<()> = Mutex::new(());

fn path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_local_data_dir()
        .ok()
        .map(|dir| dir.join("peers.json"))
}

fn read(path: &std::path::Path) -> Pins {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|err| {
            log::warn!(
                "peers: {} cannot be read ({err}) — starting over",
                path.display()
            );
            Pins::new()
        }),
        Err(_) => Pins::new(),
    }
}

/// Looks how `node` fits `friend`'s devices and logs it when it does not.
/// `what` says what happened, for the log ("an offer", "a clip went").
pub fn note(app: &AppHandle, friend: &str, node: &str, stable: bool, what: &str) -> PinVerdict {
    let Some(path) = path(app) else {
        return PinVerdict::Unpinnable;
    };
    let verdict = {
        let _guard = FILE.lock();
        pin_verdict(&read(&path), friend, node, stable)
    };
    let short = node.get(..10).unwrap_or(node);
    match verdict {
        PinVerdict::Changed => {
            log::warn!("peers: {what} with {friend} on a device not seen with them before ({short}…) — noted only")
        }
        PinVerdict::New => {
            log::info!("peers: {what} with {friend}, no device known for them yet ({short}…)")
        }
        PinVerdict::Known | PinVerdict::Unpinnable => {}
    }
    verdict
}

/// Keeps `node` as one of `friend`'s devices: a clip went across with it.
pub fn pin(app: &AppHandle, friend: &str, node: &str) {
    let Some(path) = path(app) else { return };
    let _guard = FILE.lock();
    let mut pins = read(&path);
    add_pin(&mut pins, friend, node, now_ms());
    let written = serde_json::to_vec_pretty(&pins)
        .map_err(std::io::Error::other)
        .and_then(|bytes| crate::secret::write_file(&path, &bytes));
    if let Err(err) = written {
        log::warn!("peers: could not keep a device: {err}");
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(n: u8) -> String {
        format!("{n:02x}").repeat(32)
    }

    #[test]
    fn a_device_is_new_then_known_and_a_second_one_is_noticed() {
        let mut pins = Pins::new();
        assert_eq!(pin_verdict(&pins, "f1", &node(1), true), PinVerdict::New);
        add_pin(&mut pins, "f1", &node(1), 100);
        assert_eq!(pin_verdict(&pins, "f1", &node(1), true), PinVerdict::Known);
        assert_eq!(
            pin_verdict(&pins, "f1", &node(2), true),
            PinVerdict::Changed
        );
        // Another friend's device is no reason for this one.
        assert_eq!(pin_verdict(&pins, "f2", &node(1), true), PinVerdict::New);
        // A second PC of the same account is kept beside the first.
        add_pin(&mut pins, "f1", &node(2), 200);
        assert_eq!(pin_verdict(&pins, "f1", &node(1), true), PinVerdict::Known);
        assert_eq!(pin_verdict(&pins, "f1", &node(2), true), PinVerdict::Known);
        assert_eq!(pins["f1"].len(), 2);
    }

    #[test]
    fn ids_that_do_not_last_or_are_no_ids_are_not_compared() {
        let mut pins = Pins::new();
        add_pin(&mut pins, "f1", &node(1), 100);
        assert_eq!(
            pin_verdict(&pins, "f1", &node(2), false),
            PinVerdict::Unpinnable
        );
        assert_eq!(pin_verdict(&pins, "f1", "zz", true), PinVerdict::Unpinnable);
        add_pin(&mut pins, "f1", &node(0xab), 150);
        assert_eq!(
            pin_verdict(&pins, "f1", &node(0xab).to_uppercase(), true),
            PinVerdict::Unpinnable
        );
        add_pin(&mut pins, "f1", "not an id", 200);
        assert_eq!(pins["f1"].len(), 2);
        // A friend whose list is there but empty counts as new.
        pins.insert("f2".into(), Vec::new());
        assert_eq!(pin_verdict(&pins, "f2", &node(1), true), PinVerdict::New);
    }

    #[test]
    fn seeing_a_device_again_moves_only_its_last_seen() {
        let mut pins = Pins::new();
        add_pin(&mut pins, "f1", &node(1), 100);
        add_pin(&mut pins, "f1", &node(1), 300);
        add_pin(&mut pins, "f1", &node(1), 200);
        assert_eq!(
            pins["f1"],
            vec![Pin {
                node: node(1),
                first_seen: 100,
                last_seen: 300
            }]
        );
    }

    #[test]
    fn the_device_seen_longest_ago_makes_room() {
        let mut pins = Pins::new();
        for n in 0..MAX_DEVICES as u8 {
            add_pin(&mut pins, "f1", &node(n), 1000 + n as i64);
        }
        add_pin(&mut pins, "f1", &node(0), 5000);
        add_pin(&mut pins, "f1", &node(99), 6000);
        let kept: Vec<&str> = pins["f1"].iter().map(|pin| pin.node.as_str()).collect();
        assert_eq!(kept.len(), MAX_DEVICES);
        assert!(kept.contains(&node(0).as_str()) && kept.contains(&node(99).as_str()));
        assert!(!kept.contains(&node(1).as_str()));
    }

    #[test]
    fn the_file_reads_as_it_is_written() {
        let mut pins = Pins::new();
        add_pin(&mut pins, "f1", &node(1), 100);
        let text = serde_json::to_string(&pins).unwrap();
        assert!(text.contains(r#""firstSeen":100"#));
        assert_eq!(serde_json::from_str::<Pins>(&text).unwrap(), pins);
    }
}
