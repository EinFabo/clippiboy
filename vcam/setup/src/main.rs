//! Installs ClippiBoy Camera's media source for Frame Server. Runs elevated
//! (ClippiBoy starts it with "runas"), carries the DLL inside itself, so
//! nothing is copied from the user-writable app folder into a place SYSTEM
//! loads from.
//!
//!     clippiboy-vcam-setup install | uninstall | status

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const FOLDER: &str = "ClippiBoy Virtual Camera";
const DLL_NAME: &str = "clippiboy_vcam.dll";
/// LocalService: Frame Server runs as it and loads the DLL.
const LOCAL_SERVICE: &str = "*S-1-5-19";
const USERS: &str = "*S-1-5-32-545";
const DLL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/dll.bin"));

/// One thing the installer does; pure, so install and uninstall can be checked
/// against each other without touching the machine.
#[derive(Debug, Clone, PartialEq)]
enum Step {
    WriteDll { path: PathBuf },
    Grant { path: PathBuf, account: String, rights: String },
    RegistryDefault { key: String, value: String },
    RegistryValue { key: String, name: String, value: String },
    RegistryDelete { key: String },
    RemoveFolder { path: PathBuf },
    RemoveOldVersions { root: PathBuf, keep: String },
}

fn clsid_key() -> String {
    format!(r"HKLM\SOFTWARE\Classes\CLSID\{}", vcam_protocol::CLSID)
}

fn version_key() -> String {
    format!(r"HKLM\{}", vcam_protocol::REGISTRY_KEY)
}

fn log_folder() -> PathBuf {
    PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into())).join("ClippiBoy")
}

/// Only ever below Program Files: SYSTEM loads the DLL, so it must sit where
/// only administrators can write.
fn install_plan(program_files: &Path, version: &str) -> Result<Vec<Step>, String> {
    let protected = program_files
        .file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("Program Files"));
    if !protected {
        return Err(format!("{} is not Program Files", program_files.display()));
    }
    if version.is_empty() || !version.chars().all(|c| c.is_ascii_digit() || c == '.') || version.contains("..") {
        return Err(format!("not a version: {version}"));
    }
    let root = program_files.join(FOLDER);
    let dir = root.join(version);
    let dll = dir.join(DLL_NAME);
    let server = format!(r"{}\InprocServer32", clsid_key());
    Ok(vec![
        Step::WriteDll { path: dll.clone() },
        Step::Grant { path: dir, account: LOCAL_SERVICE.into(), rights: "(OI)(CI)RX".into() },
        Step::RegistryDefault { key: server.clone(), value: dll.to_string_lossy().into_owned() },
        Step::RegistryValue { key: server, name: "ThreadingModel".into(), value: "Both".into() },
        Step::RegistryValue { key: version_key(), name: "Version".into(), value: version.into() },
        Step::RegistryValue { key: version_key(), name: "Path".into(), value: dll.to_string_lossy().into_owned() },
        Step::Grant { path: log_folder(), account: LOCAL_SERVICE.into(), rights: "(OI)(CI)M".into() },
        Step::Grant { path: log_folder(), account: USERS.into(), rights: "(OI)(CI)M".into() },
        Step::RemoveOldVersions { root, keep: version.into() },
    ])
}

fn uninstall_plan(program_files: &Path) -> Vec<Step> {
    vec![
        Step::RegistryDelete { key: clsid_key() },
        Step::RegistryDelete { key: version_key() },
        Step::RemoveFolder { path: program_files.join(FOLDER) },
    ]
}

/// What a step could not do.
enum Failure {
    Refused(String),
    /// A file still in use (Frame Server holds the DLL): gone after a reboot.
    Locked(String),
}

fn run_tool(program: &str, args: &[&str]) -> Result<(), Failure> {
    let out = Command::new(program).args(args).output().map_err(|e| Failure::Refused(format!("{program}: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    Err(Failure::Refused(format!("{program} {}: {}", args.join(" "), text.trim())))
}

fn run(step: &Step) -> Result<(), Failure> {
    match step {
        Step::WriteDll { path } => {
            if DLL.is_empty() {
                return Err(Failure::Refused("built without the camera's DLL".into()));
            }
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| Failure::Refused(format!("{}: {e}", dir.display())))?;
            }
            std::fs::write(path, DLL).map_err(|e| Failure::Locked(format!("{}: {e}", path.display())))
        }
        Step::Grant { path, account, rights } => {
            std::fs::create_dir_all(path).map_err(|e| Failure::Refused(format!("{}: {e}", path.display())))?;
            let grant = format!("{account}:{rights}");
            run_tool("icacls", &[&path.to_string_lossy(), "/grant", &grant])
        }
        Step::RegistryDefault { key, value } => run_tool("reg", &["add", key, "/ve", "/d", value, "/f"]),
        Step::RegistryValue { key, name, value } => run_tool("reg", &["add", key, "/v", name, "/d", value, "/f"]),
        Step::RegistryDelete { key } => {
            // Already gone is fine.
            let _ = run_tool("reg", &["delete", key, "/f"]);
            Ok(())
        }
        Step::RemoveFolder { path } => match std::fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Failure::Locked(format!("{}: {e}", path.display()))),
        },
        Step::RemoveOldVersions { root, keep } => {
            for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
                if entry.file_name().to_string_lossy() != *keep {
                    // One still loaded goes the next time; nothing waits for it.
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
            Ok(())
        }
    }
}

fn program_files() -> PathBuf {
    PathBuf::from(std::env::var("ProgramW6432").or_else(|_| std::env::var("ProgramFiles")).unwrap_or_else(|_| r"C:\Program Files".into()))
}

fn execute(steps: Vec<Step>) -> ExitCode {
    let mut locked = false;
    for step in &steps {
        match run(step) {
            Ok(()) => {}
            Err(Failure::Refused(why)) => {
                eprintln!("refused: {why}");
                return ExitCode::from(2);
            }
            Err(Failure::Locked(why)) => {
                eprintln!("in use: {why}");
                locked = true;
            }
        }
    }
    ExitCode::from(if locked { 3 } else { 0 })
}

fn main() -> ExitCode {
    let version = env!("CARGO_PKG_VERSION");
    match std::env::args().nth(1).as_deref() {
        Some("install") => match install_plan(&program_files(), version) {
            Ok(steps) => execute(steps),
            Err(why) => {
                eprintln!("refused: {why}");
                ExitCode::from(2)
            }
        },
        Some("uninstall") => execute(uninstall_plan(&program_files())),
        Some("status") => {
            let out = Command::new("reg").args(["query", &version_key(), "/v", "Version"]).output();
            match out {
                Ok(out) if out.status.success() => {
                    let text = String::from_utf8_lossy(&out.stdout);
                    let found = text.lines().find_map(|l| l.trim().strip_prefix("Version")).map(|r| r.trim().trim_start_matches("REG_SZ").trim().to_string());
                    println!("installed {} (this setup carries {version})", found.unwrap_or_default());
                    ExitCode::SUCCESS
                }
                _ => {
                    println!("not installed (this setup carries {version})");
                    ExitCode::from(1)
                }
            }
        }
        _ => {
            eprintln!("usage: clippiboy-vcam-setup install | uninstall | status");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn pf() -> &'static Path {
        Path::new(r"C:\Program Files")
    }

    #[test]
    fn install_and_uninstall_mirror_each_other() {
        let install = install_plan(pf(), "1.0.0").unwrap();
        let uninstall = uninstall_plan(pf());
        let clsid_key = format!(r"HKLM\SOFTWARE\Classes\CLSID\{}", vcam_protocol::CLSID);
        assert!(install.iter().any(|s| matches!(s, Step::RegistryDefault { key, .. } if key.starts_with(&clsid_key))));
        assert!(uninstall.iter().any(|s| matches!(s, Step::RegistryDelete { key } if *key == clsid_key)));
        assert!(install.iter().any(|s| matches!(s, Step::RegistryValue { key, name, .. } if key.ends_with(r"ClippiBoy\VirtualCamera") && name == "Version")));
        assert!(uninstall.iter().any(|s| matches!(s, Step::RegistryDelete { key } if key.ends_with(r"ClippiBoy\VirtualCamera"))));
        let dll = install.iter().find_map(|s| match s {
            Step::WriteDll { path } => Some(path.clone()),
            _ => None,
        });
        assert_eq!(
            dll.unwrap(),
            Path::new(r"C:\Program Files\ClippiBoy Virtual Camera\1.0.0\clippiboy_vcam.dll")
        );
        assert!(uninstall.iter().any(|s| matches!(s, Step::RemoveFolder { path } if path.ends_with("ClippiBoy Virtual Camera"))));
    }

    #[test]
    fn the_setup_carries_the_version_the_app_expects() {
        assert_eq!(env!("CARGO_PKG_VERSION"), vcam_protocol::DLL_VERSION);
    }

    #[test]
    fn install_refuses_a_path_outside_program_files() {
        assert!(install_plan(Path::new(r"C:\Users\fabia\AppData\Local"), "1.0.0").is_err());
        assert!(install_plan(pf(), "../evil").is_err());
    }

    #[test]
    fn the_dll_is_readable_by_frame_server() {
        let install = install_plan(pf(), "1.0.0").unwrap();
        assert!(install.iter().any(|s| matches!(s, Step::Grant { account, .. } if account == LOCAL_SERVICE)));
    }
}
