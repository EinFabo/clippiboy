//! Enumeration von Audio-Endpunkten (Ein- und Ausgänge) und von Prozessen,
//! die gerade eine Audio-Session halten.

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

    /// COM je Thread initialisieren; ein bereits initialisierter Thread ist ok.
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
                .unwrap_or_else(|_| "Unbekanntes Gerät".to_string())
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

    /// Alle Prozesse, die am Standard-Ausgabegerät eine Audio-Session halten.
    pub fn list_processes() -> Vec<AudioProcess> {
        ensure_com();
        unsafe {
            let Ok(enumerator) =
                CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
            else {
                return Vec::new();
            };
            let Ok(device) = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) else {
                return Vec::new();
            };
            let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                return Vec::new();
            };
            let Ok(sessions): windows::core::Result<IAudioSessionEnumerator> =
                manager.GetSessionEnumerator()
            else {
                return Vec::new();
            };

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

#[cfg(windows)]
pub fn list_processes() -> Vec<AudioProcess> {
    win::list_processes()
}

// Nicht-Windows: leere Listen, damit `cargo test` überall läuft.
#[cfg(not(windows))]
pub fn list_devices() -> Vec<AudioDevice> {
    let _ = DeviceKind::Output;
    Vec::new()
}

#[cfg(not(windows))]
pub fn list_processes() -> Vec<AudioProcess> {
    Vec::new()
}
