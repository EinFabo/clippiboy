//! A window may only call the commands its capability grants. The check is
//! made by reading the front end: which commands each window's imports can
//! reach, against the `allow-…` permissions of that window's capability file.
//! A missing grant otherwise fails silently at the call (the console's
//! "Delete with its link?" question never appeared).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
struct Wrapper {
    object: String,
    member: String,
    command: String,
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

fn skip_space_back(bytes: &[u8], mut end: usize) -> usize {
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    end
}

fn skip_space(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() && bytes[at].is_ascii_whitespace() {
        at += 1;
    }
    at
}

/// Every `invoke` followed (after an optional `<…>`) by `("name"`:
/// the position of `invoke` and the command name.
fn invoke_hits(source: &str) -> Vec<(usize, String)> {
    let bytes = source.as_bytes();
    let mut hits = Vec::new();
    let mut from = 0;
    while let Some(found) = source[from..].find("invoke") {
        let start = from + found;
        from = start + "invoke".len();
        if start > 0 && is_ident(bytes[start - 1]) {
            continue;
        }
        let mut at = skip_space(bytes, from);
        if bytes.get(at) == Some(&b'<') {
            let mut depth = 0usize;
            while at < bytes.len() {
                match bytes[at] {
                    b'<' => depth += 1,
                    b'>' => {
                        depth -= 1;
                        if depth == 0 {
                            at += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                at += 1;
            }
            at = skip_space(bytes, at);
        }
        if bytes.get(at) != Some(&b'(') {
            continue;
        }
        at = skip_space(bytes, at + 1);
        if bytes.get(at) != Some(&b'"') {
            continue;
        }
        let name_start = at + 1;
        let Some(len) = source[name_start..].find('"') else { continue };
        hits.push((start, source[name_start..name_start + len].to_string()));
    }
    hits
}

/// The wrapper an `invoke` at `start` belongs to: `member: (…) => invoke…`
/// inside the nearest `const object`.
fn wrapper_at(source: &str, start: usize, command: &str) -> Option<Wrapper> {
    let bytes = source.as_bytes();
    let mut at = skip_space_back(bytes, start);
    if at < 2 || &bytes[at - 2..at] != b"=>" {
        return None;
    }
    at = skip_space_back(bytes, at - 2);
    if at == 0 || bytes[at - 1] != b')' {
        return None;
    }
    let mut depth = 0usize;
    loop {
        if at == 0 {
            return None;
        }
        at -= 1;
        match bytes[at] {
            b')' => depth += 1,
            b'(' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
    at = skip_space_back(bytes, at);
    if at == 0 || bytes[at - 1] != b':' {
        return None;
    }
    at = skip_space_back(bytes, at - 1);
    let end = at;
    while at > 0 && is_ident(bytes[at - 1]) {
        at -= 1;
    }
    if at == end {
        return None;
    }
    let member = source[at..end].to_string();
    let decl = source[..at].rfind("const ")? + "const ".len();
    let object: String = source[decl..].chars().take_while(|c| is_ident(*c as u8)).collect();
    Some(Wrapper { object, member, command: command.to_string() })
}

fn wrappers(source: &str) -> Vec<Wrapper> {
    invoke_hits(source)
        .into_iter()
        .filter_map(|(start, command)| wrapper_at(source, start, &command))
        .collect()
}

/// `invoke("name"` hits outside any wrapper: they run in every window that
/// imports the module.
fn bare_invokes(source: &str) -> Vec<String> {
    invoke_hits(source)
        .into_iter()
        .filter(|(start, command)| wrapper_at(source, *start, command).is_none())
        .map(|(_, command)| command)
        .collect()
}

fn calls(source: &str, wrappers: &[Wrapper]) -> BTreeSet<String> {
    // Whitespace around a dot goes, so a chain split over lines reads as one.
    let mut tight = String::with_capacity(source.len());
    let chars: Vec<char> = source.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if c.is_whitespace() {
            let next = chars[i..].iter().find(|c| !c.is_whitespace());
            let prev = tight.chars().next_back();
            if next == Some(&'.') || prev == Some('.') {
                continue;
            }
        }
        tight.push(*c);
    }
    let bytes = tight.as_bytes();
    let mut found = BTreeSet::new();
    for w in wrappers {
        let needle = format!("{}.{}(", w.object, w.member);
        let mut from = 0;
        while let Some(at) = tight[from..].find(&needle) {
            let pos = from + at;
            from = pos + needle.len();
            if pos == 0 || !is_ident(bytes[pos - 1]) {
                found.insert(w.command.clone());
            }
        }
    }
    found
}

fn src_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The file and every file it imports through `@/…` or a relative path.
fn reachable(entry: &Path) -> Vec<PathBuf> {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut todo = vec![entry.to_path_buf()];
    while let Some(file) = todo.pop() {
        if seen.contains(&file) {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&file) else { continue };
        seen.push(file.clone());
        for spec in specifiers(&source) {
            let base = if let Some(rest) = spec.strip_prefix("@/") {
                src_root().join("src").join(rest)
            } else if spec.starts_with('.') {
                file.parent().unwrap().join(&spec)
            } else {
                continue;
            };
            let name = base.to_string_lossy().into_owned();
            let candidates = [
                format!("{name}.ts"),
                format!("{name}.tsx"),
                format!("{name}/index.ts"),
                format!("{name}/index.tsx"),
            ];
            if let Some(hit) = candidates.iter().map(PathBuf::from).find(|p| p.is_file()) {
                todo.push(hit);
            }
        }
    }
    seen
}

/// The strings after `from` and after a bare `import`.
fn specifiers(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for keyword in ["from", "import"] {
        let mut from = 0;
        while let Some(at) = source[from..].find(keyword) {
            let pos = from + at;
            from = pos + keyword.len();
            let bytes = source.as_bytes();
            if pos > 0 && is_ident(bytes[pos - 1]) {
                continue;
            }
            let quote = skip_space(bytes, from);
            if from == quote || bytes.get(quote) != Some(&b'"') {
                continue;
            }
            // `import type …` is erased by the build: it reaches nothing at run time.
            let head = &source[..pos];
            let statement = ["import ", "export "].iter().filter_map(|k| head.rfind(k)).max();
            if let Some(start) = statement {
                if keyword == "from" && head[start + 7..].trim_start().starts_with("type ") {
                    continue;
                }
            }
            if let Some(len) = source[quote + 1..].find('"') {
                out.push(source[quote + 1..quote + 1 + len].to_string());
            }
        }
    }
    out
}

/// Every permission a capability file lists, as it is written there.
fn permissions(capability: &str) -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(capability);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    json["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap_or_else(|| panic!("{capability}: {p} is no plain permission")).to_string())
        .collect()
}

/// `allow-a-b` → `a_b`; every other permission is ignored.
fn granted(capability: &str) -> BTreeSet<String> {
    permissions(capability)
        .iter()
        .filter_map(|p| p.strip_prefix("allow-"))
        .map(|name| name.replace('-', "_"))
        .collect()
}

fn needed(entry: &str) -> BTreeSet<String> {
    let files = reachable(&src_root().join(entry));
    assert!(!files.is_empty(), "{entry} could not be read");
    let sources: Vec<String> = files.iter().filter_map(|f| std::fs::read_to_string(f).ok()).collect();
    let all: Vec<Wrapper> = sources.iter().flat_map(|s| wrappers(s)).collect();
    let mut found = BTreeSet::new();
    for source in &sources {
        found.extend(calls(source, &all));
        found.extend(bare_invokes(source));
    }
    found
}

const WINDOWS: [(&str, &str, &str); 3] = [
    ("main", "src/main.tsx", "capabilities/default.json"),
    ("console", "src/console/main.tsx", "capabilities/console.json"),
    ("overlay", "src/overlay/main.tsx", "capabilities/overlay.json"),
];
/// Reachable by import but never called there: `installAccent` draws the
/// tray icon only with `turning: true`, which only the main window passes
/// (`src/lib/accent.ts:128-142`).
const NOT_CALLED: [(&str, &str); 2] = [("console", "set_logo_icon"), ("overlay", "set_logo_icon")];

#[test]
fn a_call_split_over_lines_is_still_seen() {
    let ipc = "export const api = {\n  linkExists: (id: string) => invoke<boolean>(\"link_exists\", { id }),\n  setHotkeys: (\n    a: string,\n  ) =>\n    invoke<AppConfig>(\"set_hotkeys\", { a }),\n};\n";
    let mut found = wrappers(ipc);
    assert!(found.iter().any(|w| w.object == "api" && w.member == "linkExists" && w.command == "link_exists"));
    assert!(found.iter().any(|w| w.member == "setHotkeys" && w.command == "set_hotkeys"));
    found.push(Wrapper { object: "api".into(), member: "send".into(), command: "not_this_one".into() });
    let page = "void api\n  .linkExists(clip.id)\n  .then(() => {});\nshareApi.send(a, b);";
    assert_eq!(calls(page, &found), BTreeSet::from(["link_exists".to_string()]));
}

#[test]
fn every_command_a_window_calls_is_granted_to_it() {
    for (window, entry, capability) in WINDOWS {
        let missing: Vec<String> = needed(entry)
            .difference(&granted(capability))
            .filter(|command| !NOT_CALLED.contains(&(window, command.as_str())))
            .cloned()
            .collect();
        assert!(missing.is_empty(), "{window} calls commands it is not granted: {missing:?}");
    }
}

/// CON-3 / CFG-6: neither window moves, hides or shows itself, and neither
/// reads pictures or makes tray icons — `core:default` let them. They listen
/// to the core's events and call the commands above, nothing more.
#[test]
fn the_console_and_the_banner_may_only_listen() {
    for capability in ["capabilities/console.json", "capabilities/overlay.json"] {
        let core: Vec<String> = permissions(capability).into_iter().filter(|p| p.starts_with("core:")).collect();
        assert_eq!(core, ["core:event:allow-listen", "core:event:allow-unlisten"], "{capability}");
    }
}
