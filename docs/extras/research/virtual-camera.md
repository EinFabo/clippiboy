# Virtual camera for ClippiBoy (research dossier)

> Research notes made for the facecam design on 2026-10-22: web research and read-only code reading, nothing was run. Every claim carries a source and a confidence tag. Where this dossier and the spec (`docs/superpowers/specs/2026-10-20-facecam-design.md`) disagree, the spec wins: it weighs all four dossiers and the code review together.

Status: research, 2026-10-22. Feeds open section 4 of `docs/superpowers/specs/2026-10-20-facecam-design.md` ("DLL packaging and registration, shared-memory format, behaviour when ClippiBoy is closed, Windows 10 path").

How to read this: every claim carries a source URL. **[UNCONFIRMED]** marks something only one third-party source reports, something I inferred, or something that has to be tested on the Win11 26200 test machine. Third-party projects are cited as evidence of behaviour, not as authorities. Web pages were read as data only.

---

## 0. Summary of findings

1. **Windows 11 (`MFCreateVirtualCamera`, build 22000+)**. ClippiBoy ships a COM in-proc DLL holding a custom media source. **Windows Camera Frame Server** (`svchost -k Camera`, **LocalService**, **session 0**) loads it. **Frame Server Monitor** (**LocalSystem**) also loads it while the camera is being created. Clients get frames through Frame Server. The DLL **must be registered in HKLM** (HKCU fails), so installing it needs admin. It must also sit in a folder the service accounts can read, which **rules out ClippiBoy's per-user `%LOCALAPPDATA%` install folder**. Creating the camera at runtime (Session lifetime, CurrentUser access, default categories) does **not** need admin.
2. **IPC crosses a session boundary**: ClippiBoy runs in session N and the source in session 0. A non-admin process **cannot create `Global\` file mappings** (needs SeCreateGlobalPrivilege). That leaves two workable patterns. (a) ClippiBoy creates the section in its own session namespace with an explicit DACL for LOCAL SERVICE, and the source opens it as `Session\<id>\…`. This has worked in practice, but Microsoft says the prefix is "reserved for system use". (b) A named pipe, which is fully documented but costs one extra copy. Recommendation: (a), keeping (b) as a fallback.
3. **Visibility**: an MF virtual camera shows up in MF/WinRT apps (Camera app, browsers, Settings). It also shows up in **DirectShow apps** (OBS, GraphStudioNext, Discord in practice), because it registers under KSCATEGORY_VIDEO and KSCATEGORY_CAPTURE by default. **Offer YUY2 next to NV12**: Microsoft recommends it for DirectShow compatibility, and libwebrtc's DirectShow capturer does not know NV12.
4. **Session lifetime**: the camera disappears when ClippiBoy releases it or exits, and apps using it get `MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED`. **System lifetime** keeps it, so the source can show a "ClippiBoy is closed" placeholder. Microsoft's sample README says a System camera survives only until reboot, while the API reference says it survives reboots; this needs a test.
5. **Rust is feasible** for the MF source. windows-rs `#[implement]` has every needed `_Impl` trait. One Rust media source runs in Frame Server on Win11 (JStream). Another Rust source is in a Microsoft issue (Windows-Camera #41). Pitfalls: panics must never cross COM (needs `panic = "unwind"` plus `catch_unwind`, which the current `panic = "abort"` profile does not allow), static CRT, NULL arguments, and pacing `RequestSample` yourself.
6. **Windows 10**: no `MFCreateVirtualCamera`. The only driverless route is a **DirectShow source filter**. **Write it in C++ on the DirectShow BaseClasses (MIT)**, optionally starting from **softcam (MIT)**. **OBS's module is GPLv2 and akvirtualcamera is GPLv3**: neither may be copied. DirectShow cameras are seen by Discord, Chrome, Edge (which add DirectShow-only devices to their MF list), Zoom, Teams and OBS. MF-only apps (Camera app, Settings, UWP) do not see them. Windows 10 is still about 23.5% of Steam (June 2026).
7. **OBS 32.x still ships only the DirectShow virtual camera.** A maintainer called a move to MF "low priority" in June 2026 (obs-studio #13439).

---

## 1. `MFCreateVirtualCamera` in detail

### 1.1 API surface

- **Minimum build**: "Windows Build 22000". Header `mfvirtualcamera.h`, lib/DLL `mfsensorgroup.lib` / `mfsensorgroup.dll` ([MFCreateVirtualCamera](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nf-mfvirtualcamera-mfcreatevirtualcamera)). On Windows 10, `mfsensorgroup.dll` does not export the function ([VCamSample #11](https://github.com/smourier/VCamSample/issues/11)). Microsoft's maintainer confirms the framework "was introduced with Windows 11 (build 22000)" ([Windows-Camera #26](https://github.com/microsoft/Windows-Camera/issues/26)).
- **Signature** ([docs](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nf-mfvirtualcamera-mfcreatevirtualcamera)):
  ```cpp
  HRESULT MFCreateVirtualCamera(MFVirtualCameraType type, MFVirtualCameraLifetime lifetime,
      MFVirtualCameraAccess access, LPCWSTR friendlyName, LPCWSTR sourceId,
      const GUID *categories, ULONG categoryCount, IMFVirtualCamera **virtualCamera);
  ```
  - `type`: only `MFVirtualCameraType_SoftwareCameraSource` is supported.
  - `lifetime`: `MFVirtualCameraLifetime_Session` means that once the `IMFVirtualCamera` is disposed or `Shutdown()` is called, the camera "will no longer be enumerable or activatable". `_System` "persists across sessions and reboots" ([enum](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/ne-mfvirtualcamera-mfvirtualcameralifetime)).
  - `access`: `_CurrentUser` creates the camera only for the calling user, and "each user account gets a unique virtual camera". `_AllUsers` **requires administrator** ([docs](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nf-mfvirtualcamera-mfcreatevirtualcamera), [enum](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/ne-mfvirtualcamera-mfvirtualcameraaccess)).
  - `friendlyName`: must not be NULL. The pipeline appends "Windows Virtual Camera".
  - `sourceId`: the CLSID of the custom media source as a `"{CLSID}"` string.
  - `categories`: NULL means KSCATEGORY_VIDEO_CAMERA, KSCATEGORY_VIDEO and KSCATEGORY_CAPTURE. A non-admin caller may only pass a subset of VIDEO_CAMERA, VIDEO, CAPTURE and SENSOR_CAMERA.
  - Errors: `E_ACCESSDENIED` if camera privacy denies access, or if a non-admin passes admin-only parameters.
  - The camera is **keyed off the parameters**: "By keeping the same parameters, applications can re-open the same virtual camera" ([docs](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nf-mfvirtualcamera-mfcreatevirtualcamera)). So ClippiBoy must keep the friendly name and CLSID constant across versions and languages, or cameras will pile up.
  - It is subject to the webcam privacy control. Packaged apps must not call it on the UI thread (consent deadlock). ClippiBoy is unpackaged but should still call it from a background MTA thread.
- **`IMFVirtualCamera`** inherits `IMFAttributes` and has `AddDeviceSourceInfo`, `AddProperty`, `AddRegistryEntry`, `CreateSyncEvent`, `CreateSyncSemaphore`, `GetMediaSource`, `Remove`, `SendCameraProperty`, `Shutdown`, `Start` and `Stop` ([interface](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nn-mfvirtualcamera-imfvirtualcamera)).
  - **`Start(IMFAsyncCallback*)`** registers the camera. After it succeeds, the attributes hold `MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK`. The callback receives `MF_FRAMESERVER_VCAMEVENT_EXTENDED_SOURCE_INITIALIZE / _SOURCE_START / _SOURCE_STOP / _SOURCE_UNINITIALIZE / _PIPELINE_SHUTDOWN / _CUSTOM_EVENT` ([Start](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nf-mfvirtualcamera-imfvirtualcamera-start)). **This tells ClippiBoy when a consumer starts or stops streaming**, so it only has to copy frames while someone watches.
  - **`Shutdown()`**: for a Session camera this removes it, and "Any application using the virtual camera will receive ... MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED" ([Shutdown](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nf-mfvirtualcamera-imfvirtualcamera-shutdown)).
  - **`Remove()`** is the "equivalent to physically removing an external camera and uninstalling the driver". It returns `E_ACCESSDENIED` for a system-wide camera without permission, and "Applications ... should ensure that their virtual cameras are removed when the application is uninstalled" ([Remove](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nf-mfvirtualcamera-imfvirtualcamera-remove)).
  - **`AddProperty` / `AddRegistryEntry` require administrator** ([AddProperty](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nf-mfvirtualcamera-imfvirtualcamera-addproperty), [AddRegistryEntry](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nf-mfvirtualcamera-imfvirtualcamera-addregistryentry)), so ClippiBoy cannot use them at runtime to pass configuration.
  - **`SendCameraProperty`** wraps `IKsControl::KsProperty` toward the source, and **`CreateSyncEvent`** wraps `KsEvent` ([SendCameraProperty](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nf-mfvirtualcamera-imfvirtualcamera-sendcameraproperty), [CreateSyncEvent](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nf-mfvirtualcamera-imfvirtualcamera-createsyncevent)). Both are a documented control channel from app to source, with no admin note. Microsoft's contributor says frame injection "is not something we highlight", that "you could send frames to the vcam (i.e. via a custom DDI)" in batches, and that you "would probably want to tie the vcam lifetime to the lifetime of the frame-providing app" ([Windows-Camera #53](https://github.com/microsoft/Windows-Camera/issues/53)).
  - **`AddDeviceSourceInfo`** marks a physical camera as exclusively used by the vcam ([docs](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nf-mfvirtualcamera-imfvirtualcamera-adddevicesourceinfo)). This only matters for the "wrapper" design (§1.7).

### 1.2 What the app must provide: the custom media source COM DLL

From Microsoft's [Frame Server Custom Media Source](https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/frame-server-custom-media-source) (updated 2025-05):

- The source is "a standard inproc COM server" implementing **IMFMediaEventGenerator, IMFMediaSource, IMFMediaSourceEx, IKsControl and IMFGetService**. Each stream implements **IMFMediaEventGenerator, IMFMediaStream and IMFMediaStream2**.
- **IMFGetService is mandatory**. It may return `MF_E_UNSUPPORTED_SERVICE` for everything.
- **IKsControl**: unsupported controls return `HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND)`.
- **Event queues**: "must use the MFCreateEventQueue API" and route the generator methods to it. A queue shared between source and stream serializes them.
- **No Seek or Pause**: `Pause` returns `MF_E_INVALID_STATE_TRANSITION`. Never send `MEEndOfStream` or `MEEndOfPresentation` for video.
- `Start` posts `MESourceStarted`, then `MENewStream` (or `MEUpdatedStream`) per selected stream, then `MEStreamStarted`.
- **Stream attributes**:
  - `MF_DEVICESTREAM_STREAM_CATEGORY = PINNAME_VIDEO_CAPTURE` (PREVIEW is not supported).
  - `MF_DEVICESTREAM_STREAM_ID` is mandatory and 0-based.
  - Recommended: `MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES = MFFrameSourceTypes_Color`, and `MF_DEVICESTREAM_FRAMESERVER_SHARED = 1` so shared-mode apps can open it.
- **Samples**: use `MFCreateSample`, and give every sample a time and duration "based on QPC time", with `MFGetSystemTime` recommended. Buffers come only from `MFCreateMemoryBuffer`, `MFCreateAlignedMemoryBuffer`, `MFCreate2DMediaBuffer` (recommended for NV12/YUY2) or `MFCreateDXGISurfaceBuffer`. Never use your own `IMFMediaBuffer` or malloc. Deliver through `MEMediaSample` on the stream's queue and attach `MFSampleExtension_Token` when the request carries one.
- **Optional `IMFActivate`** (Win10 1809+): `ActivateObject` receives the attributes, and `DetachObject` and `ShutdownObject` "must return E_NOTIMPL".
- **Performance**: the source is also instantiated during sensor-group generation, which "won't attempt to start" it but queries streams and media types. So creation must be cheap, and **the media types must be static**, with no dependency on ClippiBoy running.
- **Legacy compatibility**: "it's recommended that YUY2 media type is made available ... if legacy app compat is desired."
- `IMFSampleAllocatorControl` (Win10 2004+) is optional. Without it, "when running from inside a container, the system will have to copy all samples" ([docs](https://learn.microsoft.com/en-us/windows/win32/api/mfidl/nn-mfidl-imfsampleallocatorcontrol)). VCamSample implements it ([MediaStream.cpp](https://raw.githubusercontent.com/smourier/VCamSample/master/VCamSampleSource/MediaStream.cpp)). It is fine to skip it in v1.
- `MF_VIRTUALCAMERA_CONFIGURATION_APP_PACKAGE_FAMILY_NAME` adds a launch point in Camera Settings for a **packaged** configuration app ([docs](https://learn.microsoft.com/en-us/windows/win32/medfound/mf-virtualcamera-configuration-app-package-family-name)). It is irrelevant for unpackaged ClippiBoy.
- What working samples additionally do:
  - VCamSample registers an **`IMFActivate`** and sets `MF_VIRTUALCAMERA_PROVIDE_ASSOCIATED_CAMERA_SOURCES` ([Activator.cpp](https://raw.githubusercontent.com/smourier/VCamSample/master/VCamSampleSource/Activator.cpp)).
  - The source exposes `MF_DEVICEMFT_SENSORPROFILE_COLLECTION` (Legacy plus HighFrameRate profiles), `MFMEDIASOURCE_IS_LIVE`, and NV12 plus RGB32 types ([MediaSource.cpp](https://raw.githubusercontent.com/smourier/VCamSample/master/VCamSampleSource/MediaSource.cpp)).
  - In `ActivateObject` it reads the undocumented `MF_FRAMESERVER_CLIENTCONTEXT_CLIENTPID`.

### 1.3 Registration, admin rights, signing

- **HKLM, not HKCU**: "Registration goes in HKLM, not HKCU, because multiple processes load the media source", and registering needs admin ([VCamSample README](https://raw.githubusercontent.com/smourier/VCamSample/HEAD/README.md), [VCamNetSample README](https://raw.githubusercontent.com/smourier/VCamNetSample/master/README.md)). The JStream project tested this on Win11 26200: HKCU registration makes `MFCreateVirtualCamera` succeed, but "`IMFVirtualCamera::Start` fails with `0x80070003`, and Frame Server never loads the DLL" ([JStream ARCHITECTURE](https://raw.githubusercontent.com/jeffmhopkins/JStream/main/docs/ARCHITECTURE.md), [PR #174](https://github.com/jeffmhopkins/JStream/pull/174)).
- Microsoft's sample MSI writes `HKLM\…\CLSID\{…}\InprocServer32` (default value = path, `ThreadingModel` = `Both`) ([VirtualCamera README](https://github.com/microsoft/Windows-Camera/tree/master/Samples/VirtualCamera)). The driver-sample INF uses the same `ThreadingModel=Both` ([custom media source](https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/frame-server-custom-media-source)).
- **File location and ACL**: "Frame Server runs as Local Service, and Frame Server Monitor runs as Local System. Both must be able to access the DLL… If the DLL is in a directory under your user profile … it won't work" ([VCamSample #1](https://github.com/smourier/VCamSample/issues/1), [VCamNetSample README](https://raw.githubusercontent.com/smourier/VCamNetSample/master/README.md)). Microsoft confirms that FrameServer "requires access to the folder where the virtual camera is installed" ([Windows-Camera #47](https://github.com/microsoft/Windows-Camera/issues/47)), and a maintainer recommends granting LOCAL SERVICE read and execute ([#51](https://github.com/microsoft/Windows-Camera/issues/51)). After registering, restart the FrameServer service ([#51](https://github.com/microsoft/Windows-Camera/issues/51)).
- **Security consequence (my analysis)**: FrameServerMonitor runs as **LocalSystem** ([batcmd FrameServerMonitor](https://batcmd.com/windows/11/services/frameservermonitor/)) and loads the DLL while the camera is created ([MS README](https://raw.githubusercontent.com/microsoft/Windows-Camera/master/Samples/VirtualCamera/README.md)). An HKLM CLSID pointing at a **user-writable path** (for example `%LOCALAPPDATA%\ClippiBoy\…`) therefore lets any process running as that user get code into SYSTEM, which is local privilege escalation. **The DLL must live in an admin-only folder (`%ProgramFiles%`).** JStream registers from the app's own folder, and its own review flagged related ACL problems ([PR #25](https://github.com/jeffmhopkins/JStream/pull/25)). Do not copy that.
- **Signing**: no source requires the DLL to be signed. VCamSample's unsigned binaries work, and the WHQL requirement only applies to the driver-package route ([custom media source](https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/frame-server-custom-media-source)). Smart App Control, however, checks signatures "on all code modules, including DLLs" ([Eric Lawrence, Authenticode in 2024](https://textslashplain.com/2024/05/22/authenticode-in-2024/)), and RSA certificates are recommended over ECC ([textslashplain SAC](https://textslashplain.com/2026/04/28/smart-app-control/)). ClippiBoy currently signs only updater artifacts (minisign in `release.yml`), not Authenticode.
- Missing dependencies show up as `MOD_NOT_FOUND` from CoCreateInstance, and Microsoft's sample stopped linking the VC runtime dynamically ([VirtualCamera README](https://github.com/microsoft/Windows-Camera/tree/master/Samples/VirtualCamera)). So link the CRT statically.

### 1.4 Which processes load the DLL

| Process | Account / session | When | Source |
|---|---|---|---|
| FrameServerMonitor (`svchost -k CameraMonitor`) | LocalSystem, session 0 | camera creation / `Start` | [MS README](https://raw.githubusercontent.com/microsoft/Windows-Camera/master/Samples/VirtualCamera/README.md), [batcmd](https://batcmd.com/windows/11/services/frameservermonitor/) |
| FrameServer (`svchost -k Camera`, `FrameServer.dll`) | **NT AUTHORITY\LocalService**, **session 0**, no desktop, write access to files and registry "isn't" permitted | when an app streams | [custom media source](https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/frame-server-custom-media-source), [batcmd](https://batcmd.com/windows/11/services/frameserver/), confirmed for a Rust DLL by [JStream PR #195](https://github.com/jeffmhopkins/JStream/pull/195) |
| The creating app and client apps (Camera app, browsers, OBS, Teams) | user | **[UNCONFIRMED]**, reported by VCamNetSample's author | [VCamNetSample README](https://raw.githubusercontent.com/smourier/VCamNetSample/master/README.md) |

Design consequence: the DLL must do nothing in `DllMain` or at creation except build static descriptors. It touches IPC only in `IMFMediaSource::Start`, and only when its host is the FrameServer service. JStream detects its host by checking session 0, `svchost.exe` and the LOCAL SERVICE SID ([PR #25](https://github.com/jeffmhopkins/JStream/pull/25)). A virtual camera "running in a Windows service has no app identity", so WinRT `ApplicationModel` APIs fail inside it ([Windows-Camera #41](https://github.com/microsoft/Windows-Camera/issues/41), [#78](https://github.com/microsoft/Windows-Camera/issues/78)).

### 1.5 IPC consequences (ClippiBoy in session N, source in session 0)

- **`Global\` cannot be created by ClippiBoy**: "Creating a file mapping object in the global namespace from a session other than session zero requires the SeCreateGlobalPrivilege", and the "privilege check is limited to the creation … and does not apply to opening existing ones" ([CreateFileMapping](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createfilemappinga), [Kernel object namespaces](https://learn.microsoft.com/en-us/windows/win32/termserv/kernel-object-namespaces)). By default only Administrators, Local Service, Network Service and Service hold "Create global objects" ([policy doc](https://learn.microsoft.com/en-us/previous-versions/windows/it-pro/windows-10/security/threat-protection/security-policy-settings/create-global-objects)). JStream observed "access denied" for a non-elevated producer ([ARCHITECTURE](https://raw.githubusercontent.com/jeffmhopkins/JStream/main/docs/ARCHITECTURE.md)).
- **Pattern A, the service creates `Global\…`** (VL.Video.VirtualCamera): "The Frame Server (session 0) creates the section … Its DACL grants write access to the vvvv node and read access to everyone, including sandboxed consumers (AppContainer and LPAC)" ([VL.Video.VirtualCamera](https://github.com/bj-rn/VL.Video.VirtualCamera)).
  - Downsides: the section exists only while a consumer streams, so ClippiBoy has to poll for it.
  - The source does not know which user's SID to grant, which leads to Everyone/IU grants: **any process on the machine could read the user's camera**.
  - Rejected.
- **Pattern B, ClippiBoy creates the section in its session namespace with an explicit DACL, and the source opens `Session\<id>\Name`** (JStream). "the hand-off uses the engine's session namespace, and the DLL opens it as `Session\<id>\…`". This is backed by a passing hardware run: Frame Server loaded the Rust DLL "in session 0, under svchost as LOCAL SERVICE" and served "11,124 … shared frames" ([ARCHITECTURE](https://raw.githubusercontent.com/jeffmhopkins/JStream/main/docs/ARCHITECTURE.md), [PR #195](https://github.com/jeffmhopkins/JStream/pull/195)).
  - Caveat: Microsoft says "The 'Session\' prefix is reserved for system use and you should not use it in names of kernel objects" ([namespaces](https://learn.microsoft.com/en-us/windows/win32/termserv/kernel-object-namespaces)). We *open* through it and never *create* with it, but this is still undocumented usage. **[UNCONFIRMED for events; re-test on 26200]**
  - The DACL must name LOCAL SERVICE (`LS`, S-1-5-19) explicitly. The default DACL comes from the creator's token ([CreateFileMapping](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createfilemappinga)) and does not include it.
  - **[UNCONFIRMED]** FrameServer's service SID type (`sc qsidtype FrameServer` on the test machine). If it is "restricted", reads also need a grant for a restricting SID. If it is "write-restricted", only writes are affected, so keep the source read-only.
- **Pattern C, a named pipe** (`\\.\pipe\…` is not session-scoped). ClippiBoy is the pipe server, and its DACL grants the user plus LS. The source connects and reads frames. It is fully documented, and pipe breakage reports producer death for free. It costs an extra copy: 1080p NV12 is about 3.1 MB × 30 fps ≈ 93 MB/s, which is negligible. Name squatting by another logged-on user is possible, so the source must check the pipe's owner SID. **[design option, not tested by anyone cited]**
- **Pattern D, the control path through `SendCameraProperty`** (custom KS property set): documented and suitable for small control messages. Microsoft suggests batching if you use it for frames ([#53](https://github.com/microsoft/Windows-Camera/issues/53)). **[UNCONFIRMED]** whether the call activates the source when no client streams.
- **AppContainer**: the global namespace "is not available for Windows Store apps" ([namespaces](https://learn.microsoft.com/en-us/windows/win32/termserv/kernel-object-namespaces)). With the MF path, AppContainer consumers never touch our shared memory, because Frame Server proxies the frames. So **do not** grant ALL APPLICATION PACKAGES on the IPC objects. The DLL *file* in Program Files is already readable by AppContainers.
- **Timing inside the service**: a third-party report says the default ~15.6 ms timer resolution starved a standby image at 21.7 fps, and a high-resolution waitable timer fixed it ([search snippet of the now-deleted spiegel #1](https://github.com/juniormartinxo/spiegel/issues/1), **[UNCONFIRMED]**). Use `CreateWaitableTimerExW(..., CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, ...)`.
- **Pacing is the source's job**: Microsoft's maintainer says to stall "for the amount of time correlating with your desired framerate", because timestamps alone do not throttle ([Windows-Camera #74](https://github.com/microsoft/Windows-Camera/issues/74)). JStream saw about 4 pulls per producer frame before pacing ([PR #195](https://github.com/jeffmhopkins/JStream/pull/195)).

### 1.6 Lifetime: what happens when ClippiBoy exits

- **Session**: removed when ClippiBoy releases or shuts down the object, and consumers get `MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED` ([Shutdown](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nf-mfvirtualcamera-imfvirtualcamera-shutdown)). Microsoft's sample: "this virtual camera would then be removed upon closing the app" ([README](https://raw.githubusercontent.com/microsoft/Windows-Camera/master/Samples/VirtualCamera/README.md)). After a producer **crash**, a third party reports that "the camera may stay listed (showing black) until you log off or start and stop the node" ([VL.Video](https://github.com/bj-rn/VL.Video.VirtualCamera)). Because cameras are keyed by parameters, ClippiBoy re-opens the same camera on next start.
- **System**: the camera stays without ClippiBoy, and Frame Server activates our source when an app opens it, so **the source can show a placeholder** ("ClippiBoy is closed"). Microsoft's sample README says it "will remain functional until the system reboots", and the sample re-creates System cameras on next launch. The API reference says it "persists across sessions and reboots". **[CONFLICTING: test]**. Users can also remove virtual cameras in Settings > Devices > Camera ([README](https://raw.githubusercontent.com/microsoft/Windows-Camera/master/Samples/VirtualCamera/README.md)). **[UNCONFIRMED]** whether System lifetime with CurrentUser access needs admin. The docs only state the admin requirement for AllUsers.
- Microsoft's guidance: tie the vcam lifetime to the frame-providing app ([#53](https://github.com/microsoft/Windows-Camera/issues/53)).

### 1.7 Known pitfalls (Windows-Camera, VCamSample and related issues)

- **E_ACCESSDENIED on `Start`**: the DLL is in a user-profile folder ([#47](https://github.com/microsoft/Windows-Camera/issues/47), [VCamSample #1](https://github.com/smourier/VCamSample/issues/1)). It can also be Mark-of-the-Web on downloaded binaries ([VCamSample README](https://raw.githubusercontent.com/smourier/VCamSample/HEAD/README.md)).
- **`0x80070003` on `Start`**: HKCU registration ([JStream](https://raw.githubusercontent.com/jeffmhopkins/JStream/main/docs/ARCHITECTURE.md)), or the FrameServer service not running (third-party snippet, [MyCamPortable](https://github.com/jguadalix-uoc/MyCamPortable), **[UNCONFIRMED]**).
- **`RPC_S_CALL_FAILED 0x800706BE` on `Start`** means Frame Server crashed inside the source. In that case it was **a Rust media source dereferencing a NULL passed to `IMFAttributes::SetUnknown`** ([Windows-Camera #41](https://github.com/microsoft/Windows-Camera/issues/41)).
- A COM **double `Release`** caused random Frame Server crashes (c0000374/c0000005). It hit Discord users of the vcam ([VCamSample #4](https://github.com/smourier/VCamSample/issues/4)), which also shows that **Discord uses MF virtual cameras**.
- **Exclusive vs shared**: "there can only be 1 app/client using a camera source with 'Exclusive control'". Browsers and OBS probably request exclusive. By contrast, an OBS DirectShow vcam can be used by several apps at once ([Windows-Camera #72](https://github.com/microsoft/Windows-Camera/issues/72)). The new Windows 11 "Allow multiple apps to use camera at the same time" toggle under Advanced camera options shipped in builds 26100.8524 and 26200.8524 ([#72](https://github.com/microsoft/Windows-Camera/issues/72), [Pureinfotech](https://pureinfotech.com/allow-multiple-apps-use-camera-windows-11/)). This bears on spec step 1, the "share the camera" test: on recent Win11 the physical camera may be shareable without any vcam.
- **Teams** shows the vcam in preview "but isn't rendered to the other party", cause unknown ([VCamSample README](https://raw.githubusercontent.com/smourier/VCamSample/HEAD/README.md)). **Chrome**: one user only got it working with Chrome's "MediaFoundation Video Capture" flag disabled. The author could not reproduce this ([VCamSample #13](https://github.com/smourier/VCamSample/issues/13)).
- **Mirroring**: apps decide whether to mirror from the camera's ACPI PLD panel. A virtual camera has none unless one is set at creation ([Windows-Camera #59](https://github.com/microsoft/Windows-Camera/issues/59)). Setting it would need `AddProperty`, which requires admin.
- **A virtual camera cannot wrap another virtual camera.** Only one vcam can wrap a given physical camera. Opening the physical camera in shared mode from inside a vcam "is not recommended" ([VirtualCamera README](https://github.com/microsoft/Windows-Camera/tree/master/Samples/VirtualCamera)).
- **"Wrapper" alternative**: the vcam source opens the physical camera itself, using `AddDeviceSourceInfo` and `MF_VIRTUALCAMERA_PROVIDE_ASSOCIATED_CAMERA_SOURCES` on 22621+ ([attribute](https://learn.microsoft.com/en-us/windows/win32/medfound/mf-virtualcamera-provide-associated-camera-sources), [Windows-Camera #36](https://github.com/microsoft/Windows-Camera/issues/36)). **Not recommended for ClippiBoy.** The vcam then becomes the only exclusive client of the physical camera. Both ClippiBoy and Discord would have to share *the vcam*, and the vcam behaves like a real camera with a single exclusive client ([#72](https://github.com/microsoft/Windows-Camera/issues/72)). That brings back the very sharing problem the vcam is meant to solve, and it is Win11-only.
- Frame Server keeps the DLL loaded after unregistering, so `net stop FrameServer` is needed before swapping it, and a busy DLL breaks rebuilds ([JStream PR #25](https://github.com/jeffmhopkins/JStream/pull/25)).

---

## 2. How the virtual camera appears to apps

| Consumer | Sees MF vcam (Win11)? | Sees DirectShow filter? | Evidence |
|---|---|---|---|
| Windows Camera app, Settings > Cameras, WinRT `MediaCapture`, UWP | Yes | **No** | MF vcam: [alax.info 2245](https://alax.info/blog/2245), [VCamSample](https://raw.githubusercontent.com/smourier/VCamSample/HEAD/README.md). DirectShow: [MS Q&A](https://learn.microsoft.com/en-us/answers/questions/872883/which-api-to-use-for-creating-virtual-webcam) ("If the application consumes video using Media Foundation, the virtual DirectShow camera won't be seen"), [OBS #13439](https://github.com/obsproject/obs-studio/issues/13439) |
| DirectShow apps (OBS, GraphStudioNext, legacy) | **Yes**, through KSCATEGORY_VIDEO and KSCATEGORY_CAPTURE, which `MFCreateVirtualCamera` registers by default | Yes (same bitness only) | "To allow enumeration by legacy DirectShow applications … also register under KSCATEGORY_VIDEO and KSCATEGORY_CAPTURE" ([custom media source](https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/frame-server-custom-media-source)). GraphStudioNext using the MF vcam: [alax.info 2245](https://alax.info/blog/2245). OBS: [VL.Video](https://github.com/bj-rn/VL.Video.VirtualCamera), [VCamNetSample](https://raw.githubusercontent.com/smourier/VCamNetSample/master/README.md). Bitness: [MS Q&A](https://learn.microsoft.com/en-us/answers/questions/872883/which-api-to-use-for-creating-virtual-webcam) |
| Chrome / Edge / WebView2 (incl. new Teams) | Yes (MF enumerates KSCATEGORY_VIDEO_CAMERA and SENSOR_CAMERA) | **Yes**: `AugmentDevicesListWithDirectShowOnlyDevices()` adds "descriptors that are only reported by the DirectShow API", because "DirectShow virtual cameras are not supported by MediaFoundation" | [Chromium video_capture_device_factory_win.cc](https://chromium.googlesource.com/chromium/src/+/main/media/capture/video/win/video_capture_device_factory_win.cc) |
| Discord | Yes (used with VCamSample) | Yes (OBS Virtual Camera is commonly used) | [VCamSample #4](https://github.com/smourier/VCamSample/issues/4). DirectShow: libwebrtc's Windows capture is DirectShow-only ([webrtc tree](https://webrtc.googlesource.com/src/+/refs/heads/main/modules/video_capture/windows/)). **[inference]** that Discord's engine uses it. Discord has been 64-bit only since 15 June 2024 ([Discord support](https://support.discord.com/hc/en-us/articles/17997797368471)) |
| Zoom, Teams (classic), Skype | Probably (DirectShow route) | "DShow sources are still supported in some form by Teams, Skype, and Zoom" | [Windows-Camera #72](https://github.com/microsoft/Windows-Camera/issues/72) |

**Formats for DirectShow consumers**: libwebrtc's `device_info_ds.cc` maps I420, YUY2, RGB24, RGB565, MJPG and UYVY. **NV12 does not appear**, and unknown subtypes are skipped. It builds capabilities from `IAMStreamConfig::GetStreamCaps` and ignores "(VFW)" and "Google Camera Adapter" names ([device_info_ds.cc](https://webrtc.googlesource.com/src/+/refs/heads/main/modules/video_capture/windows/device_info_ds.cc)). OBS's filter offers NV12, I420 and YUY2 ([virtualcam-filter.cpp](https://raw.githubusercontent.com/obsproject/obs-studio/master/plugins/win-dshow/virtualcam-module/virtualcam-filter.cpp)). → **Expose YUY2 (and I420 on DirectShow) in addition to NV12.**

On Windows 11, **do not register both** the MF vcam and a DirectShow filter. DirectShow apps would list two "ClippiBoy" cameras (my inference from the table above).

---

## 3. Rust feasibility

- **Bindings exist**: `MFCreateVirtualCamera(type, lifetime, access, friendlyname, sourceid, categories: Option<&[GUID]>) -> Result<IMFVirtualCamera>` ([windows-docs-rs](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/Media/MediaFoundation/fn.MFCreateVirtualCamera.html)). The implementable traits include `IMFMediaSourceEx_Impl` (supertrait `IMFMediaSource_Impl`: `GetSourceAttributes`, `GetStreamAttributes`, `SetD3DManager`) ([doc](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/Media/MediaFoundation/trait.IMFMediaSourceEx_Impl.html)), `IMFMediaStream2_Impl` ([doc](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/Media/MediaFoundation/trait.IMFMediaStream2_Impl.html)) and `IKsControl_Impl` in `Win32::Media::KernelStreaming` ([doc](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/Media/KernelStreaming/trait.IKsControl_Impl.html)). These docs are for windows 0.62.2. **[UNCONFIRMED]** that 0.58 exposes the same items under feature `Win32_Media_KernelStreaming`; check by compiling. Parameter types differ between 0.58 and 0.62 (for example `Option<&IUnknown>` versus `Ref<IUnknown>`).
- **Prior art in Rust**:
  - JStream ships a windows-rs `#[implement]` media source with this chain: an `IMFActivate` that creates an `IMFMediaSourceEx`, which also implements IMFGetService, IKsControl and IMFAttributes, plus one `IMFMediaStream2`. It has the four exports, `ThreadingModel=Both`, `catch_unwind` guards on every COM method, and lock ordering source then stream ([PR #25](https://github.com/jeffmhopkins/JStream/pull/25), [PR #174](https://github.com/jeffmhopkins/JStream/pull/174)). A hardware run on Win11 passed ([PR #195](https://github.com/jeffmhopkins/JStream/pull/195)). JStream is dual MIT/Apache-2.0 licensed ([repo](https://github.com/jeffmhopkins/JStream)).
  - A second Rust source is in [Windows-Camera #41](https://github.com/microsoft/Windows-Camera/issues/41).
  - No Rust crate offers this ready-made.
- **COM server plumbing**: `#[no_mangle] extern "system" fn DllGetClassObject/DllCanUnloadNow/DllRegisterServer/DllUnregisterServer`, a `#[implement(IClassFactory)]` factory, and a global object/lock counter for `DllCanUnloadNow`. ClippiBoy already uses `#[implement]` once (`src-tauri/src/audio/capture.rs:211`).
- **Threading**: register `ThreadingModel=Both` ([MS INF](https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/frame-server-custom-media-source)). All objects must be free-threaded and internally locked. Do not hold the lock while calling `IMFMediaEventQueue::GetEvent`, as Microsoft's sample shows ([same doc](https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/frame-server-custom-media-source)). Use one event queue per source and one per stream, from `MFCreateEventQueue`.
- **Event and async model**: `RequestSample(token)` must not block a Frame Server work-queue thread for long. Store the token and let a pacing thread (high-resolution waitable timer) send `MEMediaSample` at the frame interval ([#74](https://github.com/microsoft/Windows-Camera/issues/74) says pacing is the source's job).
- **Samples**: `MFCreateSample` + `MFCreate2DMediaBuffer` (NV12/YUY2), with sample time from the producer's QPC capture time or `MFGetSystemTime()` and the duration set. `SetD3DManager` may be ignored, so system-memory samples are used (JStream does this). `IMFSampleAllocatorControl` can be added later.
- **Formats**: one stream; NV12 first (the default), then YUY2; 1280×720, 1920×1080 and 640×480 at 30 fps. Optionally add 60 fps variants and a sensor-profile collection like VCamSample's.
- **Attributes boilerplate**: `IMFActivate` inherits `IMFAttributes` (about 30 methods). Forward them to an inner `MFCreateAttributes` store with a macro. `GetStreamAttributes` can return a separate store, so the stream need not implement IMFAttributes. Handle every nullable pointer, as #41 shows.
- **Panics**: since Rust 1.81, "the non-unwind ABIs (e.g., `"C"`) will now abort on uncaught unwinds" ([Rust 1.81 notes](https://blog.rust-lang.org/2024/09/05/Rust-1.81.0/)). A panic that leaves an `extern "system"` COM method therefore aborts the process, which here is Frame Server's svchost and takes every camera down with it. So every method needs `catch_unwind`, which only works with `panic = "unwind"`. ClippiBoy's release profile sets `panic = "abort"` (`src-tauri/Cargo.toml:106`), and "Overrides cannot specify the panic, lto, or rpath settings" ([Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html)). **The vcam DLL therefore needs its own Cargo project and profile** (for example `vcam/mf-source/` with its own `[workspace]`, as JStream's spike did). Build it with `-C target-feature=+crt-static`.
- **Logging**: no `tracing` subscriber exists inside Frame Server. JStream logs to `OutputDebugString` plus a capped file in the host account's profile ([ARCHITECTURE](https://raw.githubusercontent.com/jeffmhopkins/JStream/main/docs/ARCHITECTURE.md)). VCamSample uses ETW ([README](https://raw.githubusercontent.com/smourier/VCamSample/HEAD/README.md)). The debugging scripts in Microsoft's sample attach to FrameServer and FrameServerMonitor ([README](https://github.com/microsoft/Windows-Camera/tree/master/Samples/VirtualCamera)).
- **Verdict**: Rust is a good fit for the MF source. MF's helper APIs (queues, samples, buffers) do the heavy lifting, and the producer's protocol code can be shared.

---

## 4. Windows 10 path: DirectShow source filter

- **The only driverless option.** The alternative is a custom media source plus a stub driver package that "requires WHQL certification" ([custom media source](https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/frame-server-custom-media-source), [Windows-Camera #26](https://github.com/microsoft/Windows-Camera/issues/26)). That is not realistic for ClippiBoy.
- **How OBS does it** (GPLv2: read for ideas only, copy nothing):
  - It is a DirectShow filter in `CLSID_VideoInputDeviceCategory` registered through `IFilterMapper2` with `MERIT_DO_NOT_USE`. The output pin is NV12. The COM keys go to HKCR (which lands in HKLM when elevated) with `ThreadingModel=Both` ([virtualcam-module.cpp](https://raw.githubusercontent.com/obsproject/obs-studio/master/plugins/win-dshow/virtualcam-module/virtualcam-module.cpp)).
  - The installer's `regsvr32 /i /s` registers **both** `obs-virtualcam-module32.dll` and `obs-virtualcam-module64.dll` after an admin check (`net session`) ([virtualcam-install.bat.in](https://raw.githubusercontent.com/obsproject/obs-studio/master/plugins/win-dshow/virtualcam-module/virtualcam-install.bat.in)).
  - The filter thread paces with `sleepto_100ns` at OBS's interval and offers NV12, I420 and YUY2. When OBS is not running it shows `placeholder.png`, loaded with GDI+ from next to the DLL and converted to 4:2:0, or mid-grey. Outside OBS it keeps its advertised format, read from `obs-virtualcam.txt` in AppData, and scales OBS frames to it ([virtualcam-filter.cpp](https://raw.githubusercontent.com/obsproject/obs-studio/master/plugins/win-dshow/virtualcam-module/virtualcam-filter.cpp), [placeholder.cpp](https://raw.githubusercontent.com/obsproject/obs-studio/master/plugins/win-dshow/virtualcam-module/placeholder.cpp)).
  - The shared-memory queue is §5.
- **softcam (MIT)**: a DirectShow filter DLL plus a sender API (`scCreateCamera`, `scSendFrame`). It bundles the BaseClasses (MIT), needs admin to install, and ships 32-bit and 64-bit builds ([README](https://raw.githubusercontent.com/tshino/softcam/main/README.md), [repo](https://github.com/tshino/softcam)). It uses `CSource`/`CSourceStream`, is RGB24 only, and has a single `IAMStreamConfig` capability fixed to the sender's size. Without a sender it shows black or a darkened last frame. It has a null-deref bug if no sender was ever active ([DShowSoftcam.cpp](https://raw.githubusercontent.com/tshino/softcam/main/src/softcamcore/DShowSoftcam.cpp)). It is a good skeleton, but it needs YUY2/NV12/I420 and several capabilities.
- **akvirtualcamera** (webcamoid): "virtual camera implemented as a DirectShow filter in Windows", **GPL-3.0** ([repo](https://github.com/webcamoid/akvirtualcamera)), so it cannot be used.
- **DirectShow BaseClasses**: `Samples/Win7Samples/multimedia/directshow/baseclasses` in Windows-classic-samples ([tree](https://github.com/microsoft/Windows-classic-samples/tree/main/Samples/Win7Samples/multimedia/directshow/baseclasses)), **MIT** ([LICENSE](https://raw.githubusercontent.com/microsoft/Windows-classic-samples/main/LICENSE)).
- **Rust or C++?** I found no Rust DirectShow source-filter crate or example. A web search turned up only C++ softcam and its forks **[UNCONFIRMED: absence of evidence]**. A filter needs IBaseFilter, IMediaFilter, IPin, IEnumPins, IEnumMediaTypes, IAMStreamConfig (which libwebrtc reads, [device_info_ds.cc](https://webrtc.googlesource.com/src/+/refs/heads/main/modules/video_capture/windows/device_info_ds.cc)), IKsPropertySet (pin category), allocator negotiation with the downstream IMemInputPin, a streaming thread and clock and state handling. `CSource`/`CSourceStream` already give you all of this. **Recommendation: C++ on BaseClasses, built from Cargo with the `cc` crate (or MSBuild in CI), `/MT`, 64-bit first.** A 32-bit build is optional: Discord ([support](https://support.discord.com/hc/en-us/articles/17997797368471)), Chrome and OBS are 64-bit, and DirectShow cameras are "only visible to same bitness applications" ([MS Q&A](https://learn.microsoft.com/en-us/answers/questions/872883/which-api-to-use-for-creating-virtual-webcam)).
- **Registration**: HKLM via `DllRegisterServer`/`IFilterMapper2`, which needs admin. **[UNCONFIRMED]** whether per-user registration under `HKCU\Software\Classes\CLSID\{860BB310-5D01-11d0-BD3B-00A0C911CE86}\Instance` is enumerated. No source confirms it. Processes above medium integrity ignore per-user COM ([MS dsnotes](https://learn.microsoft.com/en-us/archive/blogs/dsnotes/points-to-note-while-registering-and-unregistering-com-components)).
- **Who sees it**: see the table in §2. Windows 10's share of Steam was about 23.5% in June 2026, with Windows 11 at about 70–71% in June to August 2026, per press coverage of Valve's survey ([tech-insider](https://tech-insider.org/steam-hardware-survey-august-2026/), [Windows Central](https://www.windowscentral.com/gaming/pc-gaming/windows-11-new-milestone-steam-over-70-percent-share)). These are secondary sources: check them against Valve's page. Windows 10 support ended 14 October 2025 ([MS ESU](https://learn.microsoft.com/en-us/windows/whats-new/extended-security-updates)).
- **Bonus**: on Windows 11 a DirectShow filter also works for Discord, Zoom, OBS and Chrome. It is a fallback if the MF path fails on a given machine. Never register both.

---

## 5. Shared-memory format: practice and prior art

| | OBS (`shared-memory-queue.c`) | softcam (`FrameBuffer.cpp`) | VL.Video.VirtualCamera |
|---|---|---|---|
| Name / namespace | `OBSVirtualCamVideo`, no prefix (session-local), NULL DACL | `DirectShow Softcam/SharedMemory` plus named mutex | `Global\vvvv-vcam0-v2-{w}x{h}`, created by Frame Server |
| Slots | 3 (triple buffer), `counter % 3` | 1 | ring with per-slot seqlock |
| Sync | none (`volatile`, no barriers, torn reads possible) | named mutex on every access, 1 ms polling | seqlock, no cross-process locks |
| Liveness | `state` READY/STOPPING; the reader gives up after 10 repeated reads | sender and receiver heartbeat counters (watchdog), `is_active` flag | writer heartbeat in a 4 KB config file, then black frames |
| Format | NV12, `cx`, `cy`, `interval` in header | RGB24, w/h multiple of 4, `framerate` | size encoded in the section name (a new name on change) |
| Source | [OBS](https://raw.githubusercontent.com/obsproject/obs-studio/master/shared/obs-shared-memory-queue/shared-memory-queue.c) | [softcam](https://raw.githubusercontent.com/tshino/softcam/main/src/softcamcore/FrameBuffer.cpp) | [VL.Video](https://github.com/bj-rn/VL.Video.VirtualCamera) |

**Lessons**:
- Use a seqlock per slot, not a mutex. A mutex cannot be shared across session 0 without global objects, and a crashed holder blocks readers.
- Use explicit memory ordering, which OBS lacks.
- Use QPC timestamps so readers can detect staleness and so MF sample times are correct ([custom media source](https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/frame-server-custom-media-source)). QPC is machine-wide, so session 0 and session N share the timebase.
- Let readers send a heartbeat back where they can (softcam's receiver watchdog), so the producer skips copying when nobody reads. On Win11 the `Start` callback (`SOURCE_START`/`STOP`) does the same job officially.
- Keep the format per frame, so a camera change does not require a remap.
- Tighten security: the camera image is private, so never grant Everyone, IU or AppContainers.

---

## 6. Packaging and installation facts

- **ClippiBoy installs per user**: `bundle.windows.nsis.installMode = "currentUser"` (`src-tauri/tauri.conf.json`), so the installer needs no admin and "the app is installed in the `%LOCALAPPDATA%` folder" ([Tauri NSIS docs](https://v2.tauri.app/distribute/windows-installer/)). The updater uses `installMode: "passive"`. The hooks live in `src-tauri/windows/hooks.nsh` (PREINSTALL waits for the exe, POSTINSTALL writes `install-source.txt`). Tauri also offers `NSIS_HOOK_PREUNINSTALL` and `POSTUNINSTALL` ([docs](https://v2.tauri.app/distribute/windows-installer/)).
- **Admin needs**:
  - Win11 needs admin once, to place the DLL in Program Files and register it in HKLM. Runtime `MFCreateVirtualCamera` (Session, CurrentUser, default categories) does not ([docs](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nf-mfvirtualcamera-mfcreatevirtualcamera); [JStream](https://raw.githubusercontent.com/jeffmhopkins/JStream/main/docs/ARCHITECTURE.md) ran a non-elevated producer).
  - Win10 needs admin once for HKLM DirectShow registration. OBS and softcam both require it ([OBS bat](https://raw.githubusercontent.com/obsproject/obs-studio/master/plugins/win-dshow/virtualcam-module/virtualcam-install.bat.in), [softcam](https://raw.githubusercontent.com/tshino/softcam/main/README.md)).
- **Uninstall**: Session cameras vanish with the app. System cameras need `Remove()` ([docs](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nf-mfvirtualcamera-imfvirtualcamera-remove)). Microsoft's MSI removes "all virtual cameras created with this … dll" on uninstall ([README](https://raw.githubusercontent.com/microsoft/Windows-Camera/master/Samples/VirtualCamera/README.md)). A leftover DirectShow registration keeps "ClippiBoy Camera" listed forever.
- **Locked DLL on update**: Frame Server keeps it loaded ([JStream PR #25](https://github.com/jeffmhopkins/JStream/pull/25)). DirectShow consumers (Discord) load the filter in-proc. Microsoft's own INF uses `COPYFLG_IN_USE_RENAME` for the source DLL ([custom media source](https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/frame-server-custom-media-source)), which renames it and replaces it at reboot. Our approach is in §7.5.
- **SmartScreen, Defender and SAC**: I found no reports of Defender flagging virtual-camera DLLs, only VirusTotal noise around third-party OBS plugins ([OBS forum](https://obsproject.com/forum/threads/obs-virtualcam.121448/page-12)) **[low evidence]**. SAC checks DLL signatures ([textslashplain](https://textslashplain.com/2024/05/22/authenticode-in-2024/)). An unsigned elevated helper shows "Unknown publisher" in UAC (standard Windows behaviour).

---

## 7. Recommended design for ClippiBoy

### 7.1 Overview

```
ClippiBoy.exe (user, session N, medium IL)
  camera source (MF, real cam) ──► facecam ring/encoder (existing plan)
                               └─► vcam producer ──► Local\ClippiBoy.VCam.v1      (section, DACL: user, SY, LS:read)
                                                 └─► Local\ClippiBoy.VCam.v1.Frame (auto-reset event, LS:SYNCHRONIZE)
  Win11: holds IMFVirtualCamera ("ClippiBoy Camera", Session, CurrentUser); Start(callback) → SOURCE_START/STOP

Win11: FrameServer svchost (LocalService, session 0)
  ClippiBoyVCam.dll (Rust MF source) ── opens Session\<N>\ClippiBoy.VCam.v1 read-only
      ──► Discord, Chrome/Edge, Teams, Zoom, OBS (DirectShow apps via KS bridge), Camera app

Win10: Discord.exe / chrome.exe / Zoom.exe (user, session N)
  ClippiBoyVCamDS.dll (C++ DirectShow filter, in-proc) ── opens Local\ClippiBoy.VCam.v1 (r/w for reader heartbeat)
```

Both DLLs speak **one protocol** (§7.3). The producer code in ClippiBoy differs only in its DACL and in how it learns that a consumer exists.

### 7.2 DLL layout and registration

- **Files** (installed by the elevated helper, never by the per-user NSIS):
  - `%ProgramFiles%\ClippiBoy Virtual Camera\<vcam-version>\ClippiBoyVCam.dll`: Win11 MF source, Rust, x64 (add ARM64 later if needed). Separate Cargo project with `panic = "unwind"`, `crt-static` and LTO.
  - `%ProgramFiles%\ClippiBoy Virtual Camera\<vcam-version>\ClippiBoyVCamDS.dll`: Win10 DirectShow filter, C++ BaseClasses, x64 (x86 optional), `/MT`.
  - The placeholder images are embedded as resources (PNG decoded with WIC, or pre-baked NV12 and YUY2). Nothing is loaded from user-writable paths.
- **Two fixed CLSIDs** (never change them): one for the MF source, which the CLSID string passes to `MFCreateVirtualCamera`, and one for the DirectShow filter. Use one constant friendly name, `ClippiBoy Camera`. Windows appends its virtual-camera suffix on Win11.
- **Elevated helper** `clippiboy-vcam-setup.exe`: about 300 lines of Rust with a `requireAdministrator` manifest, Authenticode-signed. It **embeds the DLL bytes** (`include_bytes!`), so it never copies a DLL out of the user-writable `%LOCALAPPDATA%\ClippiBoy`. That avoids a time-of-check/time-of-use swap from user-writable files into a SYSTEM-loaded path.
  - `install`:
    1. Check the Windows build (≥ 22000 → MF, else DirectShow).
    2. Write the DLL to a versioned folder.
    3. Set `HKLM\SOFTWARE\Classes\CLSID\{CLSID}\InprocServer32` (default = path, `ThreadingModel=Both`). For DirectShow, also call `IFilterMapper2::RegisterFilter` in `CLSID_VideoInputDeviceCategory` with `MERIT_DO_NOT_USE`, as OBS does.
    4. Write `HKLM\SOFTWARE\ClippiBoy\VirtualCamera` with `Version`, `Kind` and `Path`.
    5. Delete older version folders, or mark them `MoveFileEx(…DELAY_UNTIL_REBOOT)` if they are locked.
  - `uninstall`: the reverse.
  - Restart Frame Server only on explicit user request, because it would interrupt live calls.
- **When the UAC prompt appears**:
  - When the user turns on "Virtual camera" in the Facecam tab.
  - When an app update ships a newer vcam DLL version. Show the prompt in the app with an explanation, never inside the passive updater.
  - In `NSIS_HOOK_PREUNINSTALL`, if `HKLM\SOFTWARE\ClippiBoy\VirtualCamera` exists, run the helper's `uninstall` via ShellExecute `runas` and wait. If the user declines, tell them it can be removed later by running the helper.
- **No change to `installMode`**: the app stays per-user. Only the vcam component is machine-wide.

### 7.3 Shared-memory protocol v1 (concrete)

Names:
- Producer side: `Local\ClippiBoy.VCam.v1` (section) and `Local\ClippiBoy.VCam.v1.Frame` (event).
- Seen from session 0: `Session\<N>\ClippiBoy.VCam.v1` and `Session\<N>\ClippiBoy.VCam.v1.Frame`.

The section is pagefile-backed (`INVALID_HANDLE_VALUE`), `PAGE_READWRITE`, and has a **fixed size**:
- `HEADER_BYTES = 4096`, `SLOT_COUNT = 4`, `SLOT_HEADER_BYTES = 4096`
- `SLOT_CAPACITY = 4_149_248` (the YUY2 1920×1080 size of 4,147,200 rounded up to 4 KiB, so NV12 or YUY2 up to 1080p fits)
- `SLOT_STRIDE = 4_153_344`, `TOTAL = 4096 + 4 × 4_153_344 = 16_617_472` bytes (≈15.8 MiB)
- Larger cameras are downscaled by the producer before publishing.

```rust
// All little-endian, #[repr(C)], offsets fixed; a C header mirrors this for the DirectShow filter.
#[repr(C)]
pub struct VcamHeader {                        // offset
    pub magic: u32,                            // 0x00  0x4356_4243 = b"CBVC"
    pub version_major: u16,                    // 0x04  1 — readers reject other majors
    pub version_minor: u16,                    // 0x06  additive changes only
    pub header_bytes: u32,                     // 0x08  4096
    pub slot_count: u32,                       // 0x0C  4
    pub slot_stride: u32,                      // 0x10  4_153_344
    pub slot_capacity: u32,                    // 0x14  4_149_248
    pub total_bytes: u64,                      // 0x18  16_617_472
    pub producer_pid: u32,                     // 0x20
    pub producer_session: u32,                 // 0x24
    pub generation: AtomicU32,                 // 0x28  +1 each time a producer (re)initialises the block
    pub state: AtomicU32,                      // 0x2C  0 INIT, 1 IDLE (no frames), 2 LIVE, 3 STOPPING
    pub idle_reason: AtomicU32,                // 0x30  1 CAM_OFF (user/master switch), 2 NO_CAMERA, 3 CAMERA_BUSY, 4 CLOSING
    pub _pad0: u32,                            // 0x34
    pub heartbeat_100ns: AtomicI64,            // 0x38  QPC in 100 ns (MFGetSystemTime domain); ≥ every 250 ms
    pub latest_frame: AtomicU64,               // 0x40  frame_no of newest complete frame; 0 = none; slot = (n-1) % slot_count
    pub reader_heartbeat_100ns: AtomicI64,     // 0x48  written by readers that can (DirectShow path); 0 = none
    pub preferred_width: u32,                  // 0x50  format consumers should default to (also cached in vcam.json for Win10)
    pub preferred_height: u32,                 // 0x54
    pub preferred_fps_num: u32,                // 0x58
    pub preferred_fps_den: u32,                // 0x5C
    pub vcam_symlink_sha256: [u8; 32],         // 0x60  SHA-256 of lowercase UTF-16 symbolic link from IMFVirtualCamera after Start (Win11); zero on Win10
    pub reserved: [u8; 4096 - 0x80],           // 0x80  zero
}

#[repr(C)]
pub struct SlotHeader {                        // at 4096 + i*slot_stride
    pub seq: AtomicU64,                        // 0x00  seqlock: odd = writing, even = stable
    pub frame_no: u64,                         // 0x08  1,2,3… (monotonic per generation)
    pub capture_100ns: i64,                    // 0x10  QPC capture time of the real camera frame
    pub duration_100ns: i64,                   // 0x18  e.g. 333_333
    pub fourcc: u32,                           // 0x20  NV12 = 0x3231_564E, YUY2 = 0x3259_5559
    pub width: u32,                            // 0x24
    pub height: u32,                           // 0x28
    pub plane_count: u32,                      // 0x2C  NV12: 2, YUY2: 1
    pub plane_offset: [u32; 4],                // 0x30  relative to slot data (= slot start + 4096), 64-byte aligned
    pub plane_stride: [u32; 4],                // 0x40  bytes per row
    pub payload_bytes: u32,                    // 0x50  ≤ slot_capacity
    pub flags: u32,                            // 0x54  bit0 MIRRORED (producer applied), bit1 FULL_RANGE
    pub color_matrix: u32,                     // 0x58  MFVideoTransferMatrix (BT601/BT709)
    pub generation: u32,                       // 0x5C  header.generation at write time
    pub reserved: [u8; 4096 - 0x60],
}
```

**Writer (ClippiBoy), per frame**:
1. `n = latest + 1`, `slot = (n-1) % 4`.
2. `s = seq.load(Relaxed)`, then `seq.store(s+1, Relaxed)`, then `fence(Release)`.
3. Write the metadata and planes.
4. `seq.store(s+2, Release)`, then `latest_frame.store(n, Release)`, then `heartbeat_100ns.store(now)`, then `SetEvent(frame)`.

**Reader**:
1. `n = latest_frame.load(Acquire)` and compute the slot.
2. `s1 = seq.load(Acquire)`. If it is odd, use the previous slot.
3. Copy the metadata and payload out.
4. `fence(Acquire)`, `s2 = seq.load(Relaxed)`.
5. Accept the copy if `s1 == s2 && frame_no == n`. Otherwise retry once, then fall back to the previous frame.

With 4 slots, the writer needs about 3 frame times (≈100 ms at 30 fps) to come back to a slot. Note that copying racing memory is outside Rust's memory model. Copy through raw pointers into a private buffer and validate with the seqlock; this is the usual shared-memory practice.

**Liveness and placeholders** (in the reader):
- No section, `state == STOPPING`, or `now − heartbeat_100ns > 1 s`: show "ClippiBoy is not running". Close the handle so a new producer's block is picked up, and retry opening every 1 s.
- `state == IDLE`: show the image for `idle_reason` (camera off / no camera / camera busy).
- Format changes need no remap. Each slot carries its own format, and the reader converts and scales to the negotiated output (NV12 or YUY2, CPU, BT.601/709 per `color_matrix`).
- If a new producer finds the name still existing (`ERROR_ALREADY_EXISTS`, because a reader still holds the crashed producer's block), it validates `magic`/`total_bytes`, bumps `generation` and re-initialises in place.

**Security descriptors** (built with `ConvertStringSecurityDescriptorToSecurityDescriptorW`; `<U>` is the user's SID from the process token):
- Win11 section: `D:P(A;;GA;;;<U>)(A;;GA;;;SY)(A;;GR;;;LS)`. GR on a section is query plus map-read, so the source maps read-only.
- Win11 event: `D:P(A;;GA;;;<U>)(A;;GA;;;SY)(A;;0x00100000;;;LS)`, which is SYNCHRONIZE only.
- Win10 section and event: `D:P(A;;GA;;;<U>)(A;;GA;;;SY)`. The DirectShow readers run as the same user.
- Consider `S:(ML;;NWNR;;;ME)` so low-integrity (sandboxed) processes of the same user cannot read the camera. **Test it with Chrome's capture process first [UNCONFIRMED]**.
- Who can read the frames: the user's own medium-IL processes (who could open the real camera anyway), SYSTEM, and LocalService services. Other users cannot. AppContainers cannot.

**Session discovery in the Win11 source** (on `IMFMediaSource::Start`, host = FrameServer):
1. Try `WTSGetActiveConsoleSessionId()`.
2. Otherwise go through `WTSEnumerateSessionsW` over the active sessions.
3. Open `Session\<id>\ClippiBoy.VCam.v1` with `FILE_MAP_READ` and validate `magic`/`version`/`total_bytes`.
4. If the activation attributes carry the vcam symbolic link, prefer the producer whose `vcam_symlink_sha256` matches. That handles two Windows users who each run ClippiBoy. **[UNCONFIRMED: whether Frame Server puts the symlink on the activate]**
5. Keep the undocumented `MF_FRAMESERVER_CLIENTCONTEXT_CLIENTPID` ([VCamSample Activator](https://raw.githubusercontent.com/smourier/VCamSample/master/VCamSampleSource/Activator.cpp)) only as a debug aid.

**Delivery in the Win11 source**:
- `RequestSample` stores the token.
- A pacing thread waits on the frame event with a timeout of the frame interval, using a high-resolution waitable timer as the clock.
- It sends the newest frame (repeating the last one if there is nothing new), or a placeholder.
- `sample time = capture_100ns`, clamped to be monotonic. Placeholders use `MFGetSystemTime()`.

**Fallback transport (Pattern C)**: if `Session\<id>\` opening fails on some build, ClippiBoy runs a named-pipe server `\\.\pipe\ClippiBoy.VCam.v1.<sessionId>` (DACL: user, SY, LS) that streams the same `SlotHeader`+payload records. The source checks that the pipe's owner SID belongs to the session's user.

### 7.4 Lifetime behaviour

**Windows 11**
- When the "Virtual camera" setting is on, ClippiBoy creates the camera at start-up (ClippiBoy autostarts with Windows) on a background MTA thread: `MFCreateVirtualCamera(SoftwareCameraSource, Session, CurrentUser, "ClippiBoy Camera", "{CLSID}", NULL, 0)`, then `Start(callback)`. It keeps the object for the whole process life and calls `Shutdown()` on exit.
- `SOURCE_START`: if the facecam master switch is on, open the real camera if needed and publish frames. Otherwise set `IDLE/CAM_OFF`, so the consumer sees "Camera is off in ClippiBoy".
- `SOURCE_STOP`: stop copying frames, and release the real camera if nothing else in ClippiBoy needs it.
- `E_ACCESSDENIED` from create means camera privacy is off for desktop apps. Show a hint pointing at Settings > Privacy > Camera.
- When ClippiBoy is closed, the camera is gone. Discord then shows the device as unavailable and the user picks the real camera. That is acceptable, since the real camera is free when ClippiBoy is closed.
- If a crash leaves the camera listed, the source shows "ClippiBoy is not running" until the next ClippiBoy start re-opens it (same parameters).
- Later option: System lifetime, which keeps the device listed with a "ClippiBoy is closed" placeholder like OBS, after testing whether it needs admin and whether it survives a reboot (§1.6).

**Windows 10**
- The filter is always registered, so "ClippiBoy Camera" is always listed.
- Without ClippiBoy it shows the "ClippiBoy is not running" placeholder at the advertised format.
- Like OBS's `obs-virtualcam.txt`, ClippiBoy writes `%LOCALAPPDATA%\gg.clippiboy.app\vcam.json` with the preferred size and fps, so the filter's capabilities are stable even before ClippiBoy starts.
- The filter writes `reader_heartbeat_100ns`. ClippiBoy copies frames only while that heartbeat is fresh (under 2 s).
- Several apps can use it at once, since each loads its own filter instance.

### 7.5 Updates, uninstall, multi-user

- **Version the DLL folder.** An update registers the new path. The old DLL stays loaded in FrameServer, FrameServerMonitor and Discord until they let go, then it is deleted on the next helper run or at reboot.
- Keep the vcam DLL **stable and rarely changed**. Protocol `version_major`/`minor` and the reserved bytes let a newer ClippiBoy work with an older DLL, so most app updates need no UAC.
- Uninstall is described in §7.2.
- One machine-wide DLL serves every Windows user. Each user's ClippiBoy creates that user's own CurrentUser camera.

### 7.6 Risks

1. **A crash in our source kills Frame Server**, and with it every camera on the machine (VCamSample #4, Windows-Camera #41). Mitigations: `catch_unwind` everywhere, no `unwrap` on the COM path, fuzz the protocol reader, run a soak test with Discord for several hours.
2. **Undocumented pieces**: the `Session\<id>\` prefix (reserved) and `MF_FRAMESERVER_CLIENTCONTEXT_CLIENTPID`. Mitigation: the named-pipe fallback.
3. **The UAC requirement** (HKLM plus Program Files) collides with the per-user, admin-free install promise. Mitigation: opt-in from settings, a clear explanation, a signed helper.
4. **Privilege escalation if the DLL path is user-writable.** It must be Program Files, and the helper embeds the DLLs.
5. **App compatibility**:
   - Teams remote side black (VCamSample report).
   - Chrome's MF flag (VCamSample #13).
   - libwebrtc NV12 gap (offer YUY2/I420).
   - Exclusive-mode conflicts on the MF vcam (#72).
   - Duplicate listing if both paths are registered.
6. **Unconfirmed behaviour** to test on 26200: System lifetime without admin and across reboot; FrameServer SID type; symlink in activate attributes; whether client apps load the DLL in-proc; 32-bit DirectShow apps seeing the MF vcam.
7. **Unsigned binaries**: Smart App Control blocks unsigned DLLs. ClippiBoy has no Authenticode signing today.
8. **The need may shrink**: Win11's "Allow multiple apps to use camera" toggle (26100.8524 / 26200.8524) may already let Discord and ClippiBoy share the real camera on recent builds. Fold this into spec step 1.
9. **Pacing and timer resolution** in session 0. Mitigation: a high-resolution waitable timer plus the frame event.

### 7.7 Rough effort (one developer)

| Work | Estimate |
|---|---|
| Protocol crate (Rust + C header) with layout and seqlock tests, producer in ClippiBoy (DACLs, heartbeat, NV12/YUY2 publish, `vcam.json`) | 3–4 days |
| Spike on the 26200 test machine (HKLM, `Session\<id>\`, Discord/Chrome/OBS see the camera, the §7.6 item 6 questions) | 2 days |
| Win11 MF source in Rust (activate + attributes forwarding, source, stream, IKsControl/IMFGetService, pacing thread, NV12↔YUY2 conversion and scaling, placeholders, panic guards, exports, logging) | 2–3 weeks |
| Elevated helper, NSIS hooks, settings UI, update and uninstall flow | 4–5 days |
| Win10 DirectShow filter (C++ BaseClasses, NV12/YUY2/I420, IAMStreamConfig with several caps, IKsPropertySet, placeholder, reader heartbeat, CI build) | 1.5–2 weeks |
| Compatibility matrix (Discord, Chrome, Edge, new Teams, Zoom, OBS, Firefox, Camera app × Win11/Win10) and soak tests | 1 week |
| **Total** | **≈ 6–8 weeks** |

---

## 8. Licensing constraints (ClippiBoy is closed source, "free to use, not open source")

| Component | License | Use in ClippiBoy |
|---|---|---|
| OBS Studio, incl. `virtualcam-module`, `shared-memory-queue`, `libdshowcapture` | GPLv2+ ([repo](https://github.com/obsproject/obs-studio)) | **Do not copy or link.** Reading for ideas only, and write our own implementation from the docs. |
| akvirtualcamera (webcamoid) | **GPL-3.0** ([repo](https://github.com/webcamoid/akvirtualcamera)) | **Do not use.** |
| softcam (tshino) | MIT ([repo](https://github.com/tshino/softcam)) | May copy or adapt. Keep the copyright and permission notice in a THIRD-PARTY-NOTICES file shipped with the installer. |
| DirectShow BaseClasses (Windows-classic-samples) | MIT ([LICENSE](https://raw.githubusercontent.com/microsoft/Windows-classic-samples/main/LICENSE)) | May compile in, with notice. |
| Windows-Camera samples (VirtualCamera, SimpleMediaSource) | MIT ([LICENSE](https://raw.githubusercontent.com/microsoft/Windows-Camera/master/LICENSE)) | May adapt, with notice. |
| VCamSample / VCamNetSample (smourier) | MIT ([VCamSample](https://github.com/smourier/VCamSample), [VCamNetSample](https://github.com/smourier/VCamNetSample)) | May adapt, with notice. |
| VL.Video.VirtualCamera | MIT ([repo](https://github.com/bj-rn/VL.Video.VirtualCamera)) | May adapt, with notice. |
| JStream vcam crates | MIT OR Apache-2.0 ([repo](https://github.com/jeffmhopkins/JStream)) | May adapt, with notice (Apache needs its NOTICE handling if that option is chosen). |
| windows-rs | MIT/Apache-2.0 | Already a dependency. |
| Chromium, libwebrtc | BSD-3 | Read only, for compatibility facts. |

Note: ClippiBoy's LICENSE says third-party components "stay under their own licenses". MIT notices must actually ship. Today the repo has no third-party notices file.

---

## 9. Test checklist for the spike (things this research could not settle)

1. On 26200, check whether `Session\<N>\ClippiBoy.VCam.v1` (section **and** event) opens from FrameServer with the §7.3 DACL. Record `sc qsidtype FrameServer`.
2. Check whether the activate attributes contain `MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK`.
3. Use Process Explorer to see whether Discord or Chrome load `ClippiBoyVCam.dll` in-proc (VCamNetSample's claim).
4. Test System lifetime with CurrentUser access, non-elevated, and whether it is present after a reboot.
5. Check which format Discord picks from the MF vcam (NV12 or YUY2, 720p or 1080p), and whether new Teams shows the image to the remote side.
6. Check whether a 32-bit DirectShow app sees the MF vcam.
7. Check HKCU-only DirectShow registration (Win10 VM): does it enumerate in Discord and Chrome?
8. Check Chrome on Win10 with the `NWNR` mandatory label on the section.
9. Check the Windows 11 multi-app camera toggle with C920 and eMeet: do Discord and ClippiBoy then share the real camera?
