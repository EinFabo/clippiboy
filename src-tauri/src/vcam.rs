//! ClippiBoy Camera, the producer side: the shared memory the media source
//! reads, the virtual camera object, and whether an app is watching it.


use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// What the Camera tab says about ClippiBoy Camera.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum VcamStatus {
    NotInstalled,
    NeedsUpdate { installed: String, bundled: String },
    Off,
    /// Listed for other apps, none watching.
    Ready,
    /// An app streams it.
    Live,
}

/// `a` is an older version than `b`, number by number.
pub fn older(a: &str, b: &str) -> bool {
    let parts = |v: &str| v.split('.').map(|p| p.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>();
    parts(a) < parts(b)
}

/// The elevated setup has to run before the camera can be listed.
pub fn needs_setup(installed: Option<&str>, bundled: &str) -> bool {
    installed.is_none_or(|v| older(v, bundled))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupDone {
    Done,
    /// A file was still in use; Windows finishes after a restart.
    AfterRestart,
}

/// The setup's exit codes: 0 done, 3 a file in use, anything else refused.
pub fn setup_outcome(code: u32) -> Result<SetupDone, String> {
    match code {
        0 => Ok(SetupDone::Done),
        3 => Ok(SetupDone::AfterRestart),
        code => Err(format!("The virtual camera's setup failed ({code})")),
    }
}

/// Windows' camera activity report, per camera link: the processes and
/// whether each streams. An app other than ClippiBoy streaming our link is
/// watching — the vcam events' SOURCE_START never came on Windows 11 here.
pub fn someone_streams(report: &[(String, Vec<(u32, bool)>)], link: &str, me: u32) -> bool {
    report
        .iter()
        .filter(|(l, _)| l.eq_ignore_ascii_case(link))
        .flat_map(|(_, processes)| processes)
        .any(|(pid, streaming)| *streaming && *pid != me)
}

pub fn status_for(installed: Option<&str>, bundled: &str, switched_on: bool, running: bool, watching: bool) -> VcamStatus {
    let Some(installed) = installed else { return VcamStatus::NotInstalled };
    if older(installed, bundled) {
        return VcamStatus::NeedsUpdate { installed: installed.into(), bundled: bundled.into() };
    }
    match (switched_on && running, watching) {
        (false, _) => VcamStatus::Off,
        (true, false) => VcamStatus::Ready,
        (true, true) => VcamStatus::Live,
    }
}

/// ClippiBoy Camera's producer: owned by `AppState` for the life of the app.
pub struct Vcam {
    /// The installed DLL's version, read at start and after each setup run.
    installed: Mutex<Option<String>>,
    watching: Arc<AtomicBool>,
    on_change: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(windows)]
    running: Mutex<Option<win::Running>>,
}

impl Default for Vcam {
    fn default() -> Self {
        Self::new()
    }
}

impl Vcam {
    pub fn new() -> Self {
        Self {
            installed: Mutex::new(Self::installed_version()),
            watching: Arc::new(AtomicBool::new(false)),
            on_change: Mutex::new(None),
            #[cfg(windows)]
            running: Mutex::new(None),
        }
    }

    /// Called when an app starts or stops streaming the camera.
    pub fn set_on_change(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.on_change.lock() = Some(Arc::new(f));
    }

    pub fn watching(&self) -> bool {
        self.watching.load(Ordering::SeqCst)
    }

    pub fn running(&self) -> bool {
        #[cfg(windows)]
        {
            self.running.lock().is_some()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    /// The version the setup put in, from the registry.
    pub fn installed_version() -> Option<String> {
        #[cfg(windows)]
        {
            win::installed_version()
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    pub fn installed(&self) -> Option<String> {
        self.installed.lock().clone()
    }

    pub fn refresh_installed(&self) {
        *self.installed.lock() = Self::installed_version();
    }

    pub fn status(&self, switched_on: bool) -> VcamStatus {
        status_for(
            self.installed().as_deref(),
            vcam_protocol::DLL_VERSION,
            switched_on,
            self.running(),
            self.watching(),
        )
    }

    /// Brings the camera in line with the switch: created while it is on and
    /// the DLL installed, gone when it is off. The shared memory is created
    /// once and handed to the cam worker.
    pub fn apply(&self, on: bool, shared: &crate::camtrack::CamShared) {
        #[cfg(windows)]
        {
            let usable = on && !needs_setup(self.installed().as_deref(), vcam_protocol::DLL_VERSION);
            let mut running = self.running.lock();
            match (usable, running.is_some()) {
                (true, false) => {
                    let writer = {
                        let mut slot = shared.vcam.lock();
                        if slot.is_none() {
                            *slot = win::create_section().map(Arc::new);
                        }
                        slot.clone()
                    };
                    if writer.is_none() {
                        return;
                    }
                    let watching = self.watching.clone();
                    let notify = self.on_change.lock().clone();
                    // Two sources report it (events, activity monitor); only a
                    // change is passed on.
                    match win::Running::start(move |active| {
                        if watching.swap(active, Ordering::SeqCst) != active {
                            if let Some(f) = &notify {
                                f();
                            }
                        }
                    }) {
                        Ok(camera) => {
                            *running = Some(camera);
                            shared.vcam_on.store(true, Ordering::SeqCst);
                        }
                        Err(err) => log::warn!("ClippiBoy Camera could not start: {err}"),
                    }
                }
                (false, true) => {
                    if let Some(camera) = running.take() {
                        camera.shutdown();
                    }
                    shared.vcam_on.store(false, Ordering::SeqCst);
                    self.watching.store(false, Ordering::SeqCst);
                }
                _ => {}
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (on, shared);
        }
    }

    /// A sign of life for the media source while nothing is published.
    pub fn heartbeat(&self, shared: &crate::camtrack::CamShared) {
        if let Some(writer) = shared.vcam.lock().as_ref() {
            writer.heartbeat(crate::audio::capture::now_100ns());
        }
    }

    pub fn shutdown(&self) {
        #[cfg(windows)]
        if let Some(camera) = self.running.lock().take() {
            camera.shutdown();
        }
    }
}

/// The setup helper: a resource of the installed app; a dev build takes the
/// one `vcam/build.ps1` made.
pub fn setup_exe(resource_dir: Option<std::path::PathBuf>) -> Option<std::path::PathBuf> {
    const NAME: &str = "clippiboy-vcam-setup.exe";
    let bundled = resource_dir.map(|dir| dir.join("resources").join(NAME));
    #[cfg(debug_assertions)]
    let dev = Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../vcam/target/release").join(NAME));
    #[cfg(not(debug_assertions))]
    let dev: Option<std::path::PathBuf> = None;
    [bundled, dev].into_iter().flatten().find(|path| path.is_file())
}

/// Runs the setup with "runas": Windows asks once, then the helper does its
/// work and ends. Blocks until it has.
pub fn run_setup(exe: &std::path::Path, verb: &str, owner: Option<isize>) -> Result<SetupDone, String> {
    #[cfg(windows)]
    {
        setup_outcome(win::run_elevated(exe, verb, owner)?)
    }
    #[cfg(not(windows))]
    {
        let _ = (exe, verb, owner);
        Err("Windows only".into())
    }
}

#[cfg(windows)]
mod win {
    use windows::core::{implement, Interface, Result as WinResult, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{BOOL, E_NOTIMPL, INVALID_HANDLE_VALUE};
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows::Win32::System::Memory::{CreateFileMappingW, MapViewOfFile, FILE_MAP_ALL_ACCESS, PAGE_READWRITE};
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

    use crate::gpu::SendPtr;

    /// The helper elevated; the exit code, or why it never ran.
    pub fn run_elevated(exe: &std::path::Path, verb: &str, owner: Option<isize>) -> Result<u32, String> {
        use windows::Win32::Foundation::{CloseHandle, HWND, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
        use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
        use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

        let file = HSTRING::from(exe.as_os_str());
        let params = HSTRING::from(verb);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            hwnd: HWND(owner.unwrap_or(0) as *mut _),
            lpVerb: windows::core::w!("runas"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(params.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        unsafe {
            if let Err(err) = ShellExecuteExW(&mut info) {
                // ERROR_CANCELLED: the UAC prompt was declined.
                return Err(if err.code().0 as u32 == 0x8007_04C7 {
                    "Cancelled".into()
                } else {
                    format!("The virtual camera's setup did not start: {err}")
                });
            }
            if info.hProcess.is_invalid() {
                return Err("The virtual camera's setup did not start".into());
            }
            let waited = WaitForSingleObject(info.hProcess, INFINITE);
            let mut code = 1u32;
            let read = GetExitCodeProcess(info.hProcess, &mut code);
            let _ = CloseHandle(info.hProcess);
            if waited != WAIT_OBJECT_0 || read.is_err() {
                return Err("The virtual camera's setup did not finish".into());
            }
            Ok(code)
        }
    }

    pub fn installed_version() -> Option<String> {
        let key = HSTRING::from(vcam_protocol::REGISTRY_KEY);
        let name = HSTRING::from("Version");
        let mut buf = [0u16; 64];
        let mut size = (buf.len() * 2) as u32;
        let found = unsafe {
            RegGetValueW(HKEY_LOCAL_MACHINE, &key, &name, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut size))
        };
        if found.is_err() {
            return None;
        }
        let len = (size as usize / 2).saturating_sub(1).min(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }

    /// The section the media source reads: the user and SYSTEM full rights,
    /// Frame Server's LocalService read only. Lives as long as the app.
    pub fn create_section() -> Option<vcam_protocol::Writer> {
        unsafe {
            let mut sd = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                windows::core::w!("D:(A;;GA;;;SY)(A;;GA;;;OW)(A;;GR;;;LS)"),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
            .ok()?;
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: BOOL(0),
            };
            let name = HSTRING::from(format!("Local\\{}", vcam_protocol::NAME));
            let handle = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                Some(&sa),
                PAGE_READWRITE,
                0,
                vcam_protocol::TOTAL_BYTES as u32,
                &name,
            )
            .map_err(|err| log::warn!("ClippiBoy Camera's memory: {err}"))
            .ok()?;
            let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, vcam_protocol::TOTAL_BYTES);
            // The handle stays open with the app; the view keeps the section.
            let writer = vcam_protocol::Writer::new(view.Value as *mut u8, vcam_protocol::TOTAL_BYTES)?;
            let mut session = 0u32;
            let _ = windows::Win32::System::RemoteDesktop::ProcessIdToSessionId(std::process::id(), &mut session);
            writer.init(std::process::id(), session);
            writer.set_state(vcam_protocol::State::Idle, vcam_protocol::IdleReason::CamOff, crate::audio::capture::now_100ns());
            Some(writer)
        }
    }

    type OnActive = std::sync::Arc<dyn Fn(bool) + Send + Sync>;

    #[implement(IMFAsyncCallback)]
    struct Events {
        on_active: OnActive,
    }

    impl IMFAsyncCallback_Impl for Events_Impl {
        fn GetParameters(&self, _: *mut u32, _: *mut u32) -> WinResult<()> {
            Err(E_NOTIMPL.into())
        }

        fn Invoke(&self, result: Option<&IMFAsyncResult>) -> WinResult<()> {
            // The documentation says only that the callback "may be invoked
            // with an IMFMediaEvent"; it is looked for in both places.
            let event = result.and_then(|r| {
                let state = unsafe { r.GetState() }.ok().and_then(|s| s.cast::<IMFMediaEvent>().ok());
                state.or_else(|| unsafe { r.GetObject() }.ok().and_then(|o| o.cast::<IMFMediaEvent>().ok()))
            });
            let Some(event) = event else {
                log::info!("ClippiBoy Camera: a notice without an event");
                return Ok(());
            };
            match unsafe { event.GetExtendedType() } {
                Ok(kind) if kind == MF_FRAMESERVER_VCAMEVENT_EXTENDED_SOURCE_START => {
                    log::info!("ClippiBoy Camera: an app started watching");
                    (self.on_active)(true);
                }
                Ok(kind) if kind == MF_FRAMESERVER_VCAMEVENT_EXTENDED_SOURCE_STOP => {
                    log::info!("ClippiBoy Camera: no app watching");
                    (self.on_active)(false);
                }
                Ok(kind) => log::info!("ClippiBoy Camera: event {kind:?}"),
                Err(_) => {
                    let kind = unsafe { event.GetType() }.unwrap_or(0);
                    let status = unsafe { event.GetStatus() }.map(|s| s.0).unwrap_or(0);
                    log::info!("ClippiBoy Camera: event type {kind}, status {status:#010x}");
                }
            }
            Ok(())
        }
    }

    /// Watches Windows' camera activity for our camera's link.
    #[implement(IMFSensorActivitiesReportCallback)]
    struct Activity {
        link: String,
        on_active: OnActive,
    }

    impl IMFSensorActivitiesReportCallback_Impl for Activity_Impl {
        fn OnActivitiesReport(&self, report: Option<&IMFSensorActivitiesReport>) -> WinResult<()> {
            let Some(report) = report else { return Ok(()) };
            let mut entries = Vec::new();
            unsafe {
                for i in 0..report.GetCount().unwrap_or(0) {
                    let Ok(activity) = report.GetActivityReport(i) else { continue };
                    let mut buf = [0u16; 512];
                    let mut written = 0u32;
                    if activity.GetSymbolicLink(&mut buf, &mut written).is_err() {
                        continue;
                    }
                    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
                    let processes = (0..activity.GetProcessCount().unwrap_or(0))
                        .filter_map(|j| activity.GetProcessActivity(j).ok())
                        .filter_map(|p| Some((p.GetProcessId().ok()?, p.GetStreamingState().ok()?.as_bool())))
                        .collect();
                    entries.push((String::from_utf16_lossy(&buf[..end]), processes));
                }
            }
            (self.on_active)(super::someone_streams(&entries, &self.link, std::process::id()));
            Ok(())
        }
    }

    /// The camera object, Session lifetime: it leaves Windows' camera list when
    /// it is shut down or the app ends.
    pub struct Running {
        camera: SendPtr<IMFVirtualCamera>,
        monitor: Option<SendPtr<IMFSensorActivityMonitor>>,
    }

    impl Running {
        pub fn start(on_active: impl Fn(bool) + Send + Sync + 'static) -> Result<Self, String> {
            let on_active: OnActive = std::sync::Arc::new(on_active);
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
                crate::mft::startup()?;
                let name = HSTRING::from(vcam_protocol::FRIENDLY_NAME);
                let clsid = HSTRING::from(vcam_protocol::CLSID);
                let camera = MFCreateVirtualCamera(
                    MFVirtualCameraType_SoftwareCameraSource,
                    MFVirtualCameraLifetime_Session,
                    MFVirtualCameraAccess_CurrentUser,
                    PCWSTR(name.as_ptr()),
                    PCWSTR(clsid.as_ptr()),
                    None,
                )
                .map_err(|err| format!("MFCreateVirtualCamera: {err}"))?;
                let events: IMFAsyncCallback = Events { on_active: on_active.clone() }.into();
                camera.Start(&events).map_err(|err| format!("Start: {err}"))?;
                log::info!("ClippiBoy Camera is listed for other apps");
                let monitor = Self::watch(&camera, on_active);
                Ok(Self { camera: SendPtr(camera), monitor })
            }
        }

        /// The activity monitor on the link Windows gave the camera; without
        /// it, "watching" stays unknown and frames still go out.
        unsafe fn watch(camera: &IMFVirtualCamera, on_active: OnActive) -> Option<SendPtr<IMFSensorActivityMonitor>> {
            let mut link = windows::core::PWSTR::null();
            let mut len = 0u32;
            camera.GetAllocatedString(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, &mut link, &mut len).ok()?;
            let text = link.to_string().unwrap_or_default();
            windows::Win32::System::Com::CoTaskMemFree(Some(link.0 as *const _));
            let callback: IMFSensorActivitiesReportCallback = Activity { link: text, on_active }.into();
            let monitor = MFCreateSensorActivityMonitor(&callback)
                .map_err(|err| log::warn!("ClippiBoy Camera's activity monitor: {err}"))
                .ok()?;
            monitor.Start().map_err(|err| log::warn!("ClippiBoy Camera's activity monitor: {err}")).ok()?;
            Some(SendPtr(monitor))
        }

        pub fn shutdown(self) {
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
                if let Some(monitor) = &self.monitor {
                    let _ = monitor.Stop();
                }
                let _ = self.camera.Shutdown();
            }
            log::info!("ClippiBoy Camera taken off the list");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_says_what_the_switch_can_do() {
        assert_eq!(status_for(None, "1.0.0", false, false, false), VcamStatus::NotInstalled);
        assert_eq!(status_for(None, "1.0.0", true, false, false), VcamStatus::NotInstalled);
        assert_eq!(
            status_for(Some("0.9.0"), "1.0.0", true, false, false),
            VcamStatus::NeedsUpdate { installed: "0.9.0".into(), bundled: "1.0.0".into() }
        );
        assert_eq!(status_for(Some("1.0.0"), "1.0.0", false, false, false), VcamStatus::Off);
        assert_eq!(status_for(Some("1.0.0"), "1.0.0", true, true, false), VcamStatus::Ready);
        assert_eq!(status_for(Some("1.0.0"), "1.0.0", true, true, true), VcamStatus::Live);
        // A newer DLL than this app carries is fine: the protocol keeps its major.
        assert_eq!(status_for(Some("1.2.0"), "1.0.0", true, true, false), VcamStatus::Ready);
    }

    #[test]
    fn the_setup_runs_only_when_the_dll_is_missing_or_older() {
        assert!(needs_setup(None, "1.0.0"));
        assert!(needs_setup(Some("0.9.0"), "1.0.0"));
        assert!(!needs_setup(Some("1.0.0"), "1.0.0"));
        assert!(!needs_setup(Some("1.1.0"), "1.0.0"));
    }

    #[test]
    fn the_setups_exit_code_says_what_happened() {
        assert_eq!(setup_outcome(0), Ok(SetupDone::Done));
        assert_eq!(setup_outcome(3), Ok(SetupDone::AfterRestart));
        assert!(setup_outcome(2).is_err());
        assert!(setup_outcome(1).is_err());
    }

    #[test]
    fn an_app_watches_while_it_streams_our_link() {
        let ours = r"\\?\SWD#VCAMDEVAPI#clippiboy";
        let other = r"\\?\usb#vid_046d";
        let me = 100;
        let report = |entries: Vec<(&str, Vec<(u32, bool)>)>| {
            entries.into_iter().map(|(l, p)| (l.to_string(), p)).collect::<Vec<_>>()
        };
        assert!(someone_streams(&report(vec![(ours, vec![(7, true)])]), ours, me));
        assert!(someone_streams(&report(vec![(&ours.to_uppercase(), vec![(7, true)])]), ours, me));
        assert!(!someone_streams(&report(vec![(ours, vec![(7, false)])]), ours, me), "opened, not streaming");
        assert!(!someone_streams(&report(vec![(other, vec![(7, true)])]), ours, me), "another camera");
        assert!(!someone_streams(&report(vec![(ours, vec![(me, true)])]), ours, me), "ClippiBoy itself");
        assert!(!someone_streams(&[], ours, me));
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(older("0.9.0", "1.0.0"));
        assert!(older("1.0.0", "1.0.10"));
        assert!(!older("1.10.0", "1.9.0"));
        assert!(!older("1.0.0", "1.0.0"));
    }
}
