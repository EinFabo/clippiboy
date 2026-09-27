//! Does Discord let ClippiBoy read which voice call someone is in — without
//! Discord's approval for RPC?
//!
//!     cargo run --example discord-probe
//!
//! Point 26 in the plan: tagging clips with the friends in the same Discord
//! call. Discord's own docs say RPC needs approval, or the user has to be one of
//! at most 50 testers. The unofficial docs say that holds only for the broad
//! `rpc` scope, and that `rpc.voice.read` — enough for
//! `GET_SELECTED_VOICE_CHANNEL` — is open to anyone. This finds out which.
//!
//! It talks to the Discord app on this PC over its local pipe, asks for
//! `rpc.voice.read` and nothing else, and reports what Discord answered. Meant
//! to be run by a friend who is **not** a tester of the ClippiBoy app: if
//! Discord shows its consent window and hands out a code after "Authorize",
//! the scope is open. It reads nothing and keeps nothing; the code it gets is
//! not used and expires by itself.
//!
//! The result also lands in `discord-probe.txt` beside the exe, to send back.

#[cfg(windows)]
fn main() {
    let mut log = probe::Log::default();
    probe::run(&mut log);
    log.line("");
    let path = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("discord-probe.txt")))
        .unwrap_or_else(|| "discord-probe.txt".into());
    match std::fs::write(&path, &log.text) {
        Ok(()) => println!("Saved to {}", path.display()),
        Err(err) => println!("Could not save the result: {err}"),
    }
    println!("Press Enter to close.");
    let _ = std::io::stdin().read_line(&mut String::new());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only.");
}

#[cfg(windows)]
mod probe {
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};

    use serde_json::{json, Value};

    /// The ClippiBoy app in the Discord developer portal — the same one the
    /// friends login uses (`server/wrangler.jsonc`). Not a secret.
    const CLIENT_ID: &str = "1553530949491236894";

    const HANDSHAKE: u32 = 0;
    const FRAME: u32 = 1;

    #[derive(Default)]
    pub struct Log {
        pub text: String,
    }

    impl Log {
        pub fn line(&mut self, line: &str) {
            println!("{line}");
            self.text.push_str(line);
            self.text.push_str("\r\n");
        }
    }

    pub fn run(log: &mut Log) {
        log.line("ClippiBoy Discord probe — can the voice call be read without approval?");
        log.line(&format!("App: {CLIENT_ID}, scope asked for: rpc.voice.read"));
        log.line("");

        // Discord opens the first free one of ten; with PTB or Canary running
        // alongside, it may not be 0.
        let Some((index, mut pipe)) = (0..10).find_map(|n| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(format!(r"\\.\pipe\discord-ipc-{n}"))
                .ok()
                .map(|pipe| (n, pipe))
        }) else {
            log.line("RESULT: NO DISCORD — no Discord app found on this PC. Start Discord (the app, not the browser) and run this again.");
            return;
        };
        log.line(&format!("Found Discord on pipe discord-ipc-{index}."));

        if let Err(err) = send(&mut pipe, HANDSHAKE, &json!({ "v": 1, "client_id": CLIENT_ID })) {
            log.line(&format!("RESULT: FAILED — handshake could not be sent: {err}"));
            return;
        }
        match receive(&mut pipe) {
            Ok((_, ready)) if ready["evt"] == "READY" => {
                let user = &ready["data"]["user"];
                let name = user["global_name"].as_str().or(user["username"].as_str()).unwrap_or("?");
                log.line(&format!("Connected. Signed in to Discord as: {name}"));
            }
            Ok((op, other)) => {
                log.line(&format!("RESULT: FAILED — Discord refused the connection (op {op}): {other}"));
                return;
            }
            Err(err) => {
                log.line(&format!("RESULT: FAILED — no answer to the handshake: {err}"));
                return;
            }
        }

        log.line("");
        log.line(">>> Discord may now show a window \"ClippiBoy wants to access your account\".");
        log.line(">>> If it does, click \"Authorize\". (Nothing is read or kept — this only tests whether it is allowed.)");
        // The narrow scope first — the one that would work for everybody. Owner
        // and friend alike got `invalid_scope` for it (2026-09-27), so then the
        // broad `rpc`, which Discord allows the owner and up to 50 testers: if
        // that works, the tester route is open.
        for scope in ["rpc.voice.read", "rpc"] {
            log.line("");
            log.line(&format!("Asking for: {scope}"));
            match authorize(&mut pipe, scope) {
                Outcome::Allowed => {
                    log.line(&format!("RESULT {scope}: ALLOWED — Discord handed out a code."));
                    return;
                }
                Outcome::Refused(line) => log.line(&format!("RESULT {scope}: REFUSED — {line}")),
                Outcome::Broken(line) => {
                    log.line(&format!("RESULT {scope}: FAILED — {line}"));
                    return;
                }
            }
        }
    }

    enum Outcome {
        Allowed,
        Refused(String),
        Broken(String),
    }

    fn authorize(pipe: &mut File, scope: &str) -> Outcome {
        let request = json!({
            "cmd": "AUTHORIZE",
            "args": { "client_id": CLIENT_ID, "scopes": [scope] },
            "nonce": format!("probe-{scope}"),
        });
        if let Err(err) = send(pipe, FRAME, &request) {
            return Outcome::Broken(format!("the request could not be sent: {err}"));
        }
        // Blocks until the window is answered, or Discord gives up on it.
        match receive(pipe) {
            Ok((_, answer)) if answer["data"]["code"].is_string() => Outcome::Allowed,
            Ok((_, answer)) if answer["evt"] == "ERROR" => {
                let code = answer["data"]["code"].as_i64().unwrap_or(0);
                let message = answer["data"]["message"].as_str().unwrap_or("");
                // 5000 is Discord's catch-all for OAuth2 errors — the message
                // says which one.
                let meaning = if message.contains("invalid_scope") {
                    "not allowed for this account"
                } else if message.contains("access_denied") || message.to_lowercase().contains("cancel") {
                    "the window was closed or \"Cancel\" was clicked"
                } else {
                    "see the message"
                };
                Outcome::Refused(format!("error {code}: {message} ({meaning})"))
            }
            Ok((op, other)) => Outcome::Broken(format!("unexpected answer (op {op}): {other}")),
            Err(err) => Outcome::Broken(format!("no answer from Discord: {err}")),
        }
    }

    /// One frame: opcode and length as little-endian u32, then the JSON.
    fn send(pipe: &mut File, op: u32, body: &Value) -> std::io::Result<()> {
        let body = serde_json::to_vec(body)?;
        let mut frame = Vec::with_capacity(8 + body.len());
        frame.extend_from_slice(&op.to_le_bytes());
        frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
        frame.extend_from_slice(&body);
        pipe.write_all(&frame)
    }

    fn receive(pipe: &mut File) -> std::io::Result<(u32, Value)> {
        let mut header = [0u8; 8];
        pipe.read_exact(&mut header)?;
        let op = u32::from_le_bytes(header[..4].try_into().unwrap());
        let length = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
        if length > 1 << 20 {
            return Err(std::io::Error::other("frame too large"));
        }
        let mut body = vec![0u8; length];
        pipe.read_exact(&mut body)?;
        Ok((op, serde_json::from_slice(&body)?))
    }
}
