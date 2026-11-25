//! Installs ClippiBoy Camera's media source for Frame Server. Runs elevated
//! (ClippiBoy starts it with "runas"), carries the DLL inside itself, so
//! nothing is copied from the user-writable app folder into a place SYSTEM
//! loads from.
//!
//!     clippiboy-vcam-setup install | uninstall | status | unlist
//!
//! `unlist` runs as the user, not elevated: it takes the user's ClippiBoy
//! Camera off Windows' camera list. The app lists it for good (System
//! lifetime), so the uninstaller does this before the DLL goes.
//!
//! Elevated, it trusts nothing the user can change: it sits in the app's
//! folder, which the user may write, so DLLs come from System32 only (Media
//! Foundation's are loaded by hand, for `unlist`); folders come from Windows'
//! known folders, not the environment; `reg` and `icacls` from System32 by
//! their full path; Frame Server is restarted through the service manager,
//! not a PowerShell that would load modules from the user's paths; and an
//! install folder anyone but SYSTEM, Administrators and TrustedInstaller
//! could change is refused.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{LocalFree, ERROR_SUCCESS, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSidToSidW, GetNamedSecurityInfoW, SetNamedSecurityInfoW, SE_FILE_OBJECT,
};
use windows::Win32::Security::{GetAce, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID};

const FOLDER: &str = "ClippiBoy Virtual Camera";
const DLL_NAME: &str = "clippiboy_vcam.dll";
/// LocalService: Frame Server runs as it and loads the DLL.
const LOCAL_SERVICE: &str = "*S-1-5-19";
const SYSTEM: &str = "S-1-5-18";
const ADMINISTRATORS: &str = "S-1-5-32-544";
const TRUSTED_INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
/// A stand-in on inheritable entries for whoever makes a file below — only
/// those who may change the folder can.
const CREATOR_OWNER: &str = "S-1-3-0";
/// The only ones who may change where the DLL lies.
const MAY_CHANGE: [&str; 3] = [SYSTEM, ADMINISTRATORS, TRUSTED_INSTALLER];
/// Rights that change a folder or what is in it: write and append data (add
/// file, add folder), write extended attributes, delete a child, write
/// attributes, delete, write the access list, take ownership, generic all,
/// generic write.
const CHANGES: u32 = 0x2 | 0x4 | 0x10 | 0x40 | 0x100 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x1000_0000 | 0x4000_0000;
const DLL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/dll.bin"));

/// One thing the installer does; pure, so install and uninstall can be checked
/// against each other without touching the machine.
#[derive(Debug, Clone, PartialEq)]
enum Step {
    /// A folder the DLL goes into, made when missing and given to
    /// Administrators when someone else owns it; refused unless only
    /// [`MAY_CHANGE`] may change it ([`protected`]).
    Protected { path: PathBuf },
    WriteDll { path: PathBuf },
    Grant { path: PathBuf, account: String, rights: String },
    RegistryDefault { key: String, value: String },
    RegistryValue { key: String, name: String, value: String },
    RegistryDelete { key: String },
    RemoveFolder { path: PathBuf },
    RemoveOldVersions { root: PathBuf, keep: String },
    /// A log folder; one that cannot go now is no reason to fail.
    RemoveLogs { path: PathBuf },
    /// Frame Server loads the DLL once and keeps it while any camera is held;
    /// restarted, it loads the one just registered. Apps using a camera lose
    /// its picture for a moment.
    RestartFrameServer,
}

fn clsid_key() -> String {
    format!(r"HKLM\SOFTWARE\Classes\CLSID\{}", vcam_protocol::CLSID)
}

fn version_key() -> String {
    format!(r"HKLM\{}", vcam_protocol::REGISTRY_KEY)
}

/// A Windows folder as the system itself says it (the API, never the
/// environment, which the user sets).
fn system_folder(get: unsafe fn(Option<&mut [u16]>) -> u32) -> Option<PathBuf> {
    let mut buf = [0u16; 260];
    let len = unsafe { get(Some(&mut buf)) } as usize;
    (len != 0 && len < buf.len()).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

fn windows_folder() -> Option<PathBuf> {
    system_folder(windows::Win32::System::SystemInformation::GetSystemWindowsDirectoryW)
}

/// System32, for the tools the setup runs.
fn system32() -> Option<PathBuf> {
    system_folder(windows::Win32::System::SystemInformation::GetSystemDirectoryW)
}

/// A known folder (`FOLDERID_ProgramFiles`, …): from the machine's own
/// settings, not from variables the user could point elsewhere.
fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let out = path.to_string().ok().map(PathBuf::from);
        windows::Win32::System::Com::CoTaskMemFree(Some(path.0 as *const _));
        out
    }
}

/// Where the media source logs now, in Frame Server's own profile.
fn log_folder() -> Option<PathBuf> {
    Some(windows_folder()?.join(vcam_protocol::LOG_DIR))
}

/// Where the media source logged before DLL 1.0.5, open to every user for
/// that — and so to a link of theirs that would have Frame Server write or
/// delete a file elsewhere. It logs into its own profile now; the folder goes.
fn old_log_folder() -> Option<PathBuf> {
    Some(known_folder(&windows::Win32::UI::Shell::FOLDERID_ProgramData)?.join("ClippiBoy"))
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
    let mut steps = vec![
        Step::Protected { path: root.clone() },
        Step::Protected { path: dir.clone() },
        Step::WriteDll { path: dll.clone() },
        Step::Grant { path: dir, account: LOCAL_SERVICE.into(), rights: "(OI)(CI)RX".into() },
        Step::RegistryDefault { key: server.clone(), value: dll.to_string_lossy().into_owned() },
        Step::RegistryValue { key: server, name: "ThreadingModel".into(), value: "Both".into() },
        Step::RegistryValue { key: version_key(), name: "Version".into(), value: version.into() },
        Step::RegistryValue { key: version_key(), name: "Path".into(), value: dll.to_string_lossy().into_owned() },
        Step::RestartFrameServer,
        Step::RemoveOldVersions { root, keep: version.into() },
    ];
    steps.extend(old_log_folder().map(|path| Step::RemoveLogs { path }));
    Ok(steps)
}

fn uninstall_plan(program_files: &Path) -> Vec<Step> {
    let mut steps = vec![
        Step::RegistryDelete { key: clsid_key() },
        Step::RegistryDelete { key: version_key() },
        Step::RemoveFolder { path: program_files.join(FOLDER) },
    ];
    steps.extend([old_log_folder(), log_folder()].into_iter().flatten().map(|path| Step::RemoveLogs { path }));
    steps
}

/// One entry of an access list: its type, the rights, and for an allowing
/// or denying entry of the plain kind its SID.
#[derive(Debug, Clone, PartialEq)]
struct Ace {
    kind: u8,
    mask: u32,
    sid: Option<String>,
}

const ACCESS_ALLOWED: u8 = 0;
/// Denying entries of every kind: they only ever take rights away.
const DENYING: [u8; 4] = [1, 6, 10, 12];

/// Why someone other than [`MAY_CHANGE`] could change a folder owned by
/// `owner` with these entries (`None`: no list at all, open to everyone).
fn protected(owner: &str, aces: Option<&[Ace]>) -> Result<(), String> {
    if !MAY_CHANGE.contains(&owner) {
        return Err(format!("owned by {owner}"));
    }
    let Some(aces) = aces else {
        return Err("it has no access list, so everyone may change it".into());
    };
    for ace in aces {
        if DENYING.contains(&ace.kind) || ace.mask & CHANGES == 0 {
            continue;
        }
        match (&ace.sid, ace.kind) {
            (Some(sid), ACCESS_ALLOWED) if MAY_CHANGE.contains(&sid.as_str()) || sid == CREATOR_OWNER => {}
            (Some(sid), _) => return Err(format!("{sid} may change it")),
            (None, kind) => return Err(format!("an access entry of kind {kind} may change it")),
        }
    }
    Ok(())
}

fn sid_text(sid: PSID) -> Option<String> {
    unsafe {
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(sid, &mut text).ok()?;
        let out = text.to_string().ok();
        let _ = LocalFree(HLOCAL(text.0.cast()));
        out
    }
}

/// A folder's owner and access list, as SID strings.
fn read_acl(path: &Path) -> Result<(String, Option<Vec<Ace>>), String> {
    let wide = HSTRING::from(path.as_os_str());
    unsafe {
        let (mut owner, mut dacl, mut sd) = (PSID::default(), std::ptr::null_mut::<ACL>(), PSECURITY_DESCRIPTOR::default());
        let read = GetNamedSecurityInfoW(
            &wide,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut dacl),
            None,
            &mut sd,
        );
        if read != ERROR_SUCCESS {
            return Err(format!("its access list cannot be read ({})", read.0));
        }
        let owner = sid_text(owner).unwrap_or_default();
        let aces = (!dacl.is_null()).then(|| {
            (0..(*dacl).AceCount as u32)
                .filter_map(|i| {
                    let mut ace = std::ptr::null_mut();
                    GetAce(dacl, i, &mut ace).ok()?;
                    let header = &*(ace as *const ACE_HEADER);
                    // Every ACCESS_*_ACE has its mask right after the header;
                    // the plain allowing and denying ones the SID after that.
                    let mask = *(ace as *const u8).add(4).cast::<u32>();
                    let plain = header.AceType == ACCESS_ALLOWED || header.AceType == 1;
                    let sid = plain.then(|| sid_text(PSID((ace as *mut u8).add(8).cast()))).flatten();
                    Some(Ace { kind: header.AceType, mask, sid })
                })
                .collect()
        });
        let _ = LocalFree(HLOCAL(sd.0));
        Ok((owner, aces))
    }
}

/// Gives a folder to Administrators: whoever owns one may rewrite its list.
fn give_to_administrators(path: &Path) -> Result<(), String> {
    let wide = HSTRING::from(path.as_os_str());
    unsafe {
        let mut sid = PSID::default();
        ConvertStringSidToSidW(&HSTRING::from(ADMINISTRATORS), &mut sid).map_err(|e| e.to_string())?;
        let set = SetNamedSecurityInfoW(&wide, SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION, sid, PSID::default(), None, None);
        let _ = LocalFree(HLOCAL(sid.0));
        if set != ERROR_SUCCESS {
            return Err(format!("it cannot be given to Administrators ({})", set.0));
        }
    }
    Ok(())
}

fn make_protected(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
    let (owner, _) = read_acl(path)?;
    if !MAY_CHANGE.contains(&owner.as_str()) {
        give_to_administrators(path)?;
    }
    let (owner, aces) = read_acl(path)?;
    protected(&owner, aces.as_deref())
}

/// What a step could not do.
enum Failure {
    Refused(String),
    /// A file still in use (Frame Server holds the DLL): gone after a reboot.
    Locked(String),
}

/// A tool from System32, by its full path: never one of the same name in
/// the setup's own folder or the current one.
fn tool(name: &str) -> Result<Command, Failure> {
    let dir = system32().ok_or_else(|| Failure::Refused("System32 not found".into()))?;
    Ok(Command::new(dir.join(name)))
}

fn run_tool(program: &str, args: &[&str]) -> Result<(), Failure> {
    let out = tool(program)?.args(args).output().map_err(|e| Failure::Refused(format!("{program}: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    Err(Failure::Refused(format!("{program} {}: {}", args.join(" "), text.trim())))
}

fn run(step: &Step) -> Result<(), Failure> {
    match step {
        Step::Protected { path } => make_protected(path).map_err(|why| Failure::Refused(format!("{}: {why}", path.display()))),
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
            run_tool("icacls.exe", &[&path.to_string_lossy(), "/grant", &grant])
        }
        Step::RegistryDefault { key, value } => run_tool("reg.exe", &["add", key, "/ve", "/d", value, "/f"]),
        Step::RegistryValue { key, name, value } => run_tool("reg.exe", &["add", key, "/v", name, "/d", value, "/f"]),
        Step::RegistryDelete { key } => {
            // Already gone is fine.
            let _ = run_tool("reg.exe", &["delete", key, "/f"]);
            Ok(())
        }
        Step::RemoveFolder { path } => match std::fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Failure::Locked(format!("{}: {e}", path.display()))),
        },
        Step::RestartFrameServer => {
            // Only a running one; it starts again on demand. A failure leaves
            // the old DLL until the next reboot, which is no reason to fail.
            if let Err(why) = restart_frame_server() {
                eprintln!("Frame Server not restarted: {why}");
            }
            Ok(())
        }
        Step::RemoveLogs { path } => {
            // Links inside, or the folder itself one, go as links: nothing
            // they point at is touched.
            match std::fs::remove_dir_all(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => eprintln!("{} not removed: {e}", path.display()),
                _ => {}
            }
            Ok(())
        }
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

/// Stops a running Frame Server and starts it again, as `Restart-Service`
/// would, through the service manager.
fn restart_frame_server() -> Result<(), String> {
    use windows::Win32::System::Services::*;
    unsafe {
        let scm = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT).map_err(|e| format!("service manager: {e}"))?;
        let service = OpenServiceW(scm, windows::core::w!("FrameServer"), SERVICE_QUERY_STATUS | SERVICE_STOP | SERVICE_START);
        let done = match service {
            Ok(service) => {
                let done = (|| {
                    let mut status = SERVICE_STATUS::default();
                    QueryServiceStatus(service, &mut status).map_err(|e| e.to_string())?;
                    if status.dwCurrentState != SERVICE_RUNNING {
                        return Ok(());
                    }
                    ControlService(service, SERVICE_CONTROL_STOP, &mut status).map_err(|e| format!("stop: {e}"))?;
                    let until = Instant::now() + Duration::from_secs(30);
                    while status.dwCurrentState != SERVICE_STOPPED {
                        if Instant::now() > until {
                            return Err("it did not stop".to_string());
                        }
                        std::thread::sleep(Duration::from_millis(200));
                        QueryServiceStatus(service, &mut status).map_err(|e| e.to_string())?;
                    }
                    StartServiceW(service, None).map_err(|e| format!("start: {e}"))
                })();
                let _ = CloseServiceHandle(service);
                done
            }
            Err(e) => Err(format!("FrameServer: {e}")),
        };
        let _ = CloseServiceHandle(scm);
        done
    }
}

fn program_files() -> Option<PathBuf> {
    known_folder(&windows::Win32::UI::Shell::FOLDERID_ProgramFiles)
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

/// A function of a DLL from System32. Media Foundation's are loaded by hand:
/// imported, they would be looked for in the setup's own folder first, also
/// when it runs elevated.
unsafe fn system_function(dll: &str, name: windows::core::PCSTR) -> Result<unsafe extern "system" fn() -> isize, String> {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
    let module = LoadLibraryExW(&HSTRING::from(dll), windows::Win32::Foundation::HANDLE::default(), LOAD_LIBRARY_SEARCH_SYSTEM32)
        .map_err(|err| format!("{dll}: {err}"))?;
    GetProcAddress(module, name).ok_or_else(|| format!("{dll} has no {}", name.to_string().unwrap_or_default()))
}

/// The same parameters the app lists the camera with open that listing; then
/// it is removed.
fn unlist() -> Result<(), String> {
    use std::ffi::c_void;
    use windows::core::{Interface, GUID, HRESULT};
    use windows::Win32::Media::MediaFoundation::*;
    type Startup = unsafe extern "system" fn(u32, u32) -> HRESULT;
    type Create = unsafe extern "system" fn(
        MFVirtualCameraType,
        MFVirtualCameraLifetime,
        MFVirtualCameraAccess,
        PCWSTR,
        PCWSTR,
        *const GUID,
        u32,
        *mut *mut c_void,
    ) -> HRESULT;
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        let startup: Startup = std::mem::transmute(system_function("mfplat.dll", windows::core::s!("MFStartup"))?);
        startup(MF_VERSION, MFSTARTUP_FULL).ok().map_err(|err| format!("MFStartup: {err}"))?;
        let create: Create = std::mem::transmute(system_function("mfsensorgroup.dll", windows::core::s!("MFCreateVirtualCamera"))?);
        let name = HSTRING::from(vcam_protocol::FRIENDLY_NAME);
        let clsid = HSTRING::from(vcam_protocol::CLSID);
        let mut raw = std::ptr::null_mut();
        create(
            MFVirtualCameraType_SoftwareCameraSource,
            MFVirtualCameraLifetime_System,
            MFVirtualCameraAccess_CurrentUser,
            PCWSTR(name.as_ptr()),
            PCWSTR(clsid.as_ptr()),
            std::ptr::null(),
            0,
            &mut raw,
        )
        .ok()
        .map_err(|err| format!("MFCreateVirtualCamera: {err}"))?;
        if raw.is_null() {
            return Err("MFCreateVirtualCamera gave no camera".into());
        }
        let camera = IMFVirtualCamera::from_raw(raw);
        let removed = camera.Remove().map_err(|err| format!("Remove: {err}"));
        let _ = camera.Shutdown();
        removed
    }
}

fn main() -> ExitCode {
    // Before anything loads a DLL: from System32 only, never the setup's
    // own folder, which the user can write.
    unsafe {
        use windows::Win32::System::LibraryLoader::{SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_SYSTEM32};
        if SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32).is_err() {
            eprintln!("refused: the DLL search cannot be limited to System32");
            return ExitCode::from(2);
        }
    }
    let version = env!("CARGO_PKG_VERSION");
    let Some(program_files) = program_files() else {
        eprintln!("refused: Program Files not found");
        return ExitCode::from(2);
    };
    match std::env::args().nth(1).as_deref() {
        Some("install") => match install_plan(&program_files, version) {
            Ok(steps) => execute(steps),
            Err(why) => {
                eprintln!("refused: {why}");
                ExitCode::from(2)
            }
        },
        Some("uninstall") => execute(uninstall_plan(&program_files)),
        Some("unlist") => match unlist() {
            Ok(()) => ExitCode::SUCCESS,
            Err(why) => {
                eprintln!("not listed: {why}");
                ExitCode::from(1)
            }
        },
        Some("status") => {
            let out = tool("reg.exe").ok().map(|mut reg| reg.args(["query", &version_key(), "/v", "Version"]).output());
            match out {
                Some(Ok(out)) if out.status.success() => {
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
            eprintln!("usage: clippiboy-vcam-setup install | uninstall | status | unlist");
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

    /// Frame Server keeps the DLL it loaded for as long as anything holds a
    /// camera — ClippiBoy itself does while its virtual camera is on — so an
    /// update showed the old placeholder until a reboot. It is restarted once
    /// the new DLL is registered, and the old versions go after that, unlocked.
    #[test]
    fn an_update_restarts_frame_server_before_the_old_versions_go() {
        let install = install_plan(pf(), "1.0.2").unwrap();
        let at = |want: fn(&Step) -> bool| install.iter().position(want).unwrap();
        let registered = at(|s| matches!(s, Step::RegistryDefault { .. }));
        let restart = at(|s| matches!(s, Step::RestartFrameServer));
        let old = at(|s| matches!(s, Step::RemoveOldVersions { .. }));
        assert!(registered < restart && restart < old, "{install:?}");
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

    /// The old log folder was open to every user; nothing is granted to them
    /// any more, and the folder goes on install and uninstall alike.
    #[test]
    fn no_folder_is_left_open_to_every_user() {
        let install = install_plan(pf(), "1.0.5").unwrap();
        assert!(install.iter().all(|s| !matches!(s, Step::Grant { account, .. } if account != LOCAL_SERVICE)), "{install:?}");
        assert!(install.iter().all(|s| !matches!(s, Step::Grant { rights, .. } if rights.contains('M') || rights.contains('F') || rights.contains('W'))));
        for plan in [install, uninstall_plan(pf())] {
            assert!(plan.iter().any(|s| matches!(s, Step::RemoveLogs { path } if Some(path) == old_log_folder().as_ref())), "{plan:?}");
        }
    }

    /// The DLL goes only into folders checked first.
    #[test]
    fn the_folders_are_checked_before_the_dll_goes_in() {
        let install = install_plan(pf(), "1.0.5").unwrap();
        let root = Path::new(r"C:\Program Files\ClippiBoy Virtual Camera");
        assert_eq!(install[0], Step::Protected { path: root.to_path_buf() });
        assert_eq!(install[1], Step::Protected { path: root.join("1.0.5") });
        assert!(matches!(install[2], Step::WriteDll { .. }), "{install:?}");
    }

    fn ace(kind: u8, mask: u32, sid: &str) -> Ace {
        Ace { kind, mask, sid: Some(sid.into()) }
    }

    /// The camera's folder as an elevated setup made it below Program Files.
    fn as_installed() -> Vec<Ace> {
        vec![
            ace(0, 0x1200a9, "S-1-5-19"),
            ace(0, 0x1f01ff, TRUSTED_INSTALLER),
            ace(0, 0x1000_0000, TRUSTED_INSTALLER),
            ace(0, 0x1f01ff, SYSTEM),
            ace(0, 0x1f01ff, ADMINISTRATORS),
            ace(0, 0x1200a9, "S-1-5-32-545"),
            ace(0, 0xa000_0000, "S-1-5-32-545"),
            ace(0, 0x1000_0000, CREATOR_OWNER),
            ace(0, 0x1200a9, "S-1-15-2-1"),
        ]
    }

    #[test]
    fn a_folder_only_the_system_may_change_passes() {
        assert_eq!(protected(ADMINISTRATORS, Some(&as_installed())), Ok(()));
        assert_eq!(protected(TRUSTED_INSTALLER, Some(&as_installed())), Ok(()));
        let mut denied = as_installed();
        denied.push(ace(1, 0x1f01ff, "S-1-1-0"));
        assert_eq!(protected(ADMINISTRATORS, Some(&denied)), Ok(()), "a denying entry only takes away");
    }

    #[test]
    fn a_folder_anyone_else_may_change_is_refused() {
        let user = "S-1-5-21-1-2-3-1001";
        assert!(protected(user, Some(&as_installed())).is_err(), "an owner may rewrite the list");
        assert!(protected(ADMINISTRATORS, None).is_err(), "no list: open to everyone");
        for (mask, what) in [(0x1301bf, "modify"), (0x2, "add a file"), (0x4_0000, "write the list"), (0x4000_0000, "generic write")] {
            let mut open = as_installed();
            open.push(ace(0, mask, "S-1-5-32-545"));
            assert!(protected(ADMINISTRATORS, Some(&open)).is_err(), "Users may {what}");
        }
        let mut odd = as_installed();
        odd.push(Ace { kind: 9, mask: 0x1301bf, sid: None });
        assert!(protected(ADMINISTRATORS, Some(&odd)).is_err(), "an allowing entry of another kind");
    }

    /// Read off the disk: Program Files passes, a folder of the user's does not.
    #[test]
    fn the_access_lists_are_read_as_windows_has_them() {
        let pf = program_files().expect("Program Files");
        assert!(pf.is_absolute() && pf.file_name().is_some_and(|n| n.eq_ignore_ascii_case("Program Files")), "{}", pf.display());
        let (owner, aces) = read_acl(&pf).unwrap();
        assert_eq!(protected(&owner, aces.as_deref()), Ok(()), "{owner} {aces:?}");
        // The camera's folders, where installed here, pass too.
        let camera = pf.join(FOLDER);
        let installed = std::fs::read_dir(&camera).into_iter().flatten().flatten().map(|e| e.path());
        for dir in installed.chain([camera.clone()]).filter(|dir| dir.is_dir()) {
            let (owner, aces) = read_acl(&dir).unwrap();
            assert_eq!(protected(&owner, aces.as_deref()), Ok(()), "{}", dir.display());
        }
        let mine = std::env::temp_dir().join(format!("clippiboy-vcam-setup-test-{}", std::process::id()));
        std::fs::create_dir_all(&mine).unwrap();
        let (owner, aces) = read_acl(&mine).unwrap();
        let _ = std::fs::remove_dir(&mine);
        assert!(protected(&owner, aces.as_deref()).is_err(), "{owner} {aces:?}");
    }

    #[test]
    fn the_tools_come_from_system32_by_their_full_path() {
        let Ok(reg) = tool("reg.exe") else { panic!("no System32") };
        let path = Path::new(reg.get_program());
        assert!(path.is_absolute(), "{}", path.display());
        assert!(path.ends_with(r"System32\reg.exe") || path.ends_with(r"system32\reg.exe"), "{}", path.display());
        assert!(path.is_file());
    }

    #[test]
    fn the_dll_is_readable_by_frame_server() {
        let install = install_plan(pf(), "1.0.0").unwrap();
        assert!(install.iter().any(|s| matches!(s, Step::Grant { account, .. } if account == LOCAL_SERVICE)));
    }
}
