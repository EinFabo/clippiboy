//! ClippiBoy Camera, the producer side: the shared memory the media source
//! reads, the virtual camera object, and whether an app is watching it.


use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;

/// What the Camera tab says about ClippiBoy Camera.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

    pub fn status(&self, switched_on: bool) -> VcamStatus {
        status_for(
            Self::installed_version().as_deref(),
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
            let usable = on && Self::installed_version().is_some_and(|v| !older(&v, vcam_protocol::DLL_VERSION));
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
                    let shared_flag = shared.vcam_watching.clone();
                    let notify = self.on_change.lock().clone();
                    match win::Running::start(move |active| {
                        watching.store(active, Ordering::SeqCst);
                        shared_flag.store(active, Ordering::SeqCst);
                        if let Some(f) = &notify {
                            f();
                        }
                    }) {
                        Ok(camera) => *running = Some(camera),
                        Err(err) => log::warn!("ClippiBoy Camera could not start: {err}"),
                    }
                }
                (false, true) => {
                    if let Some(camera) = running.take() {
                        camera.shutdown();
                    }
                    self.watching.store(false, Ordering::SeqCst);
                    shared.vcam_watching.store(false, Ordering::SeqCst);
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

    #[implement(IMFAsyncCallback)]
    struct Events {
        on_active: Box<dyn Fn(bool) + Send + Sync>,
    }

    impl IMFAsyncCallback_Impl for Events_Impl {
        fn GetParameters(&self, _: *mut u32, _: *mut u32) -> WinResult<()> {
            Err(E_NOTIMPL.into())
        }

        fn Invoke(&self, result: Option<&IMFAsyncResult>) -> WinResult<()> {
            let event = result.and_then(|r| unsafe { r.GetObject() }.ok()).and_then(|o| o.cast::<IMFMediaEvent>().ok());
            if let Some(kind) = event.and_then(|e| unsafe { e.GetExtendedType() }.ok()) {
                if kind == MF_FRAMESERVER_VCAMEVENT_EXTENDED_SOURCE_START {
                    log::info!("ClippiBoy Camera: an app started watching");
                    (self.on_active)(true);
                } else if kind == MF_FRAMESERVER_VCAMEVENT_EXTENDED_SOURCE_STOP {
                    log::info!("ClippiBoy Camera: no app watching");
                    (self.on_active)(false);
                }
            }
            Ok(())
        }
    }

    /// The camera object, Session lifetime: it leaves Windows' camera list when
    /// it is shut down or the app ends.
    pub struct Running(SendPtr<IMFVirtualCamera>);

    impl Running {
        pub fn start(on_active: impl Fn(bool) + Send + Sync + 'static) -> Result<Self, String> {
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
                let events: IMFAsyncCallback = Events { on_active: Box::new(on_active) }.into();
                camera.Start(&events).map_err(|err| format!("Start: {err}"))?;
                log::info!("ClippiBoy Camera is listed for other apps");
                Ok(Self(SendPtr(camera)))
            }
        }

        pub fn shutdown(self) {
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
                let _ = self.0.Shutdown();
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
    fn versions_compare_by_number() {
        assert!(older("0.9.0", "1.0.0"));
        assert!(older("1.0.0", "1.0.10"));
        assert!(!older("1.10.0", "1.9.0"));
        assert!(!older("1.0.0", "1.0.0"));
    }
}
