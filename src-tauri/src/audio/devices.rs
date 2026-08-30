//! Enumeration of audio endpoints (inputs and outputs) and of processes that
//! currently hold an audio session.

use crate::model::{AudioDevice, AudioProcess, DeviceKind};

#[cfg(windows)]
mod win {
    use super::*;
    use windows::core::Interface;
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Media::Audio::{
        eCapture, eMultimedia, eRender, EDataFlow, IAudioSessionControl2, IAudioSessionEnumerator,
        IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
        DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
    };
    use windows::Win32::System::ProcessStatus::GetModuleBaseNameW;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
    };

    /// Initialize COM per thread; an already-initialized thread is fine.
    pub fn ensure_com() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
    }

    fn friendly_name(device: &IMMDevice) -> String {
        unsafe {
            device
                .OpenPropertyStore(STGM_READ)
                .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
                .map(|value| value.to_string())
                .unwrap_or_else(|_| "Unknown device".to_string())
        }
    }

    fn device_id(device: &IMMDevice) -> String {
        unsafe {
            device
                .GetId()
                .map(|id| id.to_string().unwrap_or_default())
                .unwrap_or_default()
        }
    }

    fn collect(flow: EDataFlow, kind: DeviceKind) -> windows::core::Result<Vec<AudioDevice>> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let default_id = enumerator
                .GetDefaultAudioEndpoint(flow, eMultimedia)
                .map(|d| device_id(&d))
                .unwrap_or_default();

            let collection = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
            let count = collection.GetCount()?;
            let mut out = Vec::with_capacity(count as usize);
            for index in 0..count {
                let device = collection.Item(index)?;
                let id = device_id(&device);
                out.push(AudioDevice {
                    is_default: !id.is_empty() && id == default_id,
                    name: friendly_name(&device),
                    id,
                    kind: kind.clone(),
                });
            }
            Ok(out)
        }
    }

    pub fn list_devices() -> Vec<AudioDevice> {
        ensure_com();
        let mut devices = collect(eRender, DeviceKind::Output).unwrap_or_default();
        devices.extend(collect(eCapture, DeviceKind::Input).unwrap_or_default());
        devices
    }

    fn process_name(pid: u32) -> Option<String> {
        unsafe {
            let handle = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ,
                false,
                pid,
            )
            .ok()?;
            let mut buf = [0u16; 260];
            let len = GetModuleBaseNameW(handle, None, &mut buf);
            let _ = windows::Win32::Foundation::CloseHandle(handle);
            if len == 0 {
                return None;
            }
            Some(String::from_utf16_lossy(&buf[..len as usize]))
        }
    }

    /// The sessions on `device_id` — empty string for the default output device.
    fn sessions_of(device_id: &str) -> Option<IAudioSessionEnumerator> {
        ensure_com();
        unsafe {
            let enumerator =
                CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .ok()?;
            let device = if device_id.is_empty() {
                enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)
            } else {
                let wide: Vec<u16> = device_id.encode_utf16().chain(std::iter::once(0)).collect();
                enumerator.GetDevice(windows::core::PCWSTR(wide.as_ptr()))
            };
            device
                .ok()?
                .Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)
                .ok()?
                .GetSessionEnumerator()
                .ok()
        }
    }

    /// The PIDs holding an audio session on `device_id`.
    ///
    /// Deliberately without process names: `GetProcessId` needs no handle on the
    /// process at all, so unlike [`list_processes`] this also sees games running
    /// as administrator. That matters here — this list decides what the
    /// leftovers track is built from, and a missing entry means missing audio.
    pub fn session_pids(device_id: &str) -> Vec<u32> {
        let Some(sessions) = sessions_of(device_id) else {
            return Vec::new();
        };
        unsafe {
            let count = sessions.GetCount().unwrap_or(0);
            let mut out: Vec<u32> = Vec::new();
            for index in 0..count {
                let Ok(control) = sessions.GetSession(index) else {
                    continue;
                };
                let Ok(control2) = control.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                let Ok(pid) = control2.GetProcessId() else {
                    continue;
                };
                if pid != 0 && !out.contains(&pid) {
                    out.push(pid);
                }
            }
            out
        }
    }

    /// Every process holding an audio session on the default output device.
    pub fn list_processes() -> Vec<AudioProcess> {
        let Some(sessions) = sessions_of("") else {
            return Vec::new();
        };
        unsafe {
            let count = sessions.GetCount().unwrap_or(0);
            let mut seen: Vec<u32> = Vec::new();
            let mut out = Vec::new();
            for index in 0..count {
                let Ok(control) = sessions.GetSession(index) else {
                    continue;
                };
                let Ok(control2) = control.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                let Ok(pid) = control2.GetProcessId() else {
                    continue;
                };
                if pid == 0 || seen.contains(&pid) {
                    continue;
                }
                seen.push(pid);

                let exe = process_name(pid).unwrap_or_else(|| format!("PID {pid}"));
                let display = control2
                    .GetDisplayName()
                    .ok()
                    .and_then(|s| s.to_string().ok())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| exe.trim_end_matches(".exe").to_string());

                out.push(AudioProcess {
                    pid,
                    name: display,
                    exe,
                });
            }
            out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            out
        }
    }
}

#[cfg(windows)]
pub fn list_devices() -> Vec<AudioDevice> {
    win::list_devices()
}

/// The PIDs playing on this output device right now — `""` for the default one.
/// What the leftovers track is assembled from.
#[cfg(windows)]
pub fn session_pids(device_id: &str) -> Vec<u32> {
    win::session_pids(device_id)
}

#[cfg(not(windows))]
pub fn session_pids(device_id: &str) -> Vec<u32> {
    let _ = device_id;
    Vec::new()
}

#[cfg(windows)]
pub fn list_processes() -> Vec<AudioProcess> {
    win::list_processes()
}

// Non-Windows: empty lists so `cargo test` runs everywhere.
#[cfg(not(windows))]
pub fn list_devices() -> Vec<AudioDevice> {
    let _ = DeviceKind::Output;
    Vec::new()
}

#[cfg(not(windows))]
pub fn list_processes() -> Vec<AudioProcess> {
    Vec::new()
}
