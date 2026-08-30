//! Diagnostic: which processes hold an audio session on which output device,
//! and which process does game detection point at?
//!
//! `cargo run --example sessions`

use clippiboy_lib::audio::devices;

fn main() {
    let names: std::collections::HashMap<u32, String> = devices::list_processes()
        .into_iter()
        .map(|p| (p.pid, p.name))
        .collect();
    let name_of = |pid: u32| names.get(&pid).cloned().unwrap_or_else(|| "?".into());

    println!("--- sessions per output device ---");
    for device in devices::list_devices() {
        if !matches!(device.kind, clippiboy_lib::model::DeviceKind::Output) {
            continue;
        }
        let pids = devices::session_pids(&device.id);
        let listed: Vec<String> = pids
            .iter()
            .map(|pid| format!("{pid} ({})", name_of(*pid)))
            .collect();
        println!(
            "  {}{}\n      {}",
            device.name,
            if device.is_default { " [default]" } else { "" },
            if listed.is_empty() {
                "nothing playing".into()
            } else {
                listed.join(", ")
            }
        );
    }

    println!("--- game detection (foreground window) ---");
    match clippiboy_lib::game::detect_detailed() {
        Some(game) => println!("  {} -> pid {} ({})", game.name, game.pid, game.exe),
        None => println!("  no game in the foreground"),
    }
}
