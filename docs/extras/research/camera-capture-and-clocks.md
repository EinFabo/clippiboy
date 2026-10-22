# Webcam capture with Media Foundation in Rust: clocks, sharing, encoder

> Research notes made for the facecam design on 2026-10-22: web research and read-only code reading, nothing was run. Every claim carries a source and a confidence tag. Where this dossier and the spec (`docs/superpowers/specs/2026-10-20-facecam-design.md`) disagree, the spec wins: it weighs all four dossiers and the code review together.

Research dossier for the ClippiBoy facecam (`docs/superpowers/specs/2026-10-20-facecam-design.md`, building block 1 "Camera source" and block 2 "Cam track").
Date: 2026-10-22. Target: Windows 11 build 26200, Logitech C920, eMeet C960, Rust + `windows` 0.58.

Each claim carries one of these tags:

- **[D]** documented by a primary source (URL given)
- **[S]** secondary source (forum, press, third-party code)
- **[I]** my inference from documented facts
- **[U]** unconfirmed; needs a test on the real hardware (see section 10)

---

## 0. Short answers

1. **Clock.** Every relevant timestamp is QPC in 100 ns units:
   - WGC `SystemRelativeTime` [D]
   - WASAPI `pu64QPCPosition` [D]
   - MF `MFSampleExtension_DeviceTimestamp` [D]
   - WinRT `MediaFrameReference.SystemRelativeTime` [D]
   - `MFGetSystemTime()`, which Microsoft describes as a QPC wrapper [D]

   All of these can be compared directly with `audio::capture::now_100ns()`. One thing is not settled: whether `IMFSample::GetSampleTime()` from a webcam reader is raw QPC or is rebased to the start of streaming. The documentation implies it is rebased [D/U]. So use `DeviceTimestamp` and check that each value is plausible.
2. **Sharing with Discord.** Windows 11 build 26100 and later has a documented Win32 attribute, `MF_DEVSOURCE_ATTRIBUTE_FRAMESERVER_SHARE_MODE`, that opens the camera in **sharing mode** [D]. In sharing mode ClippiBoy never blocks a "controlling" app such as Discord. The cost is that ClippiBoy cannot choose the format. On Windows 10 the only non-blocking route is WinRT `MediaCapture` with `SharedReadOnly` (available since 1607) [D].
   - The attribute's GUID is **not in `windows` 0.58 or 0.62.2**. It has to be copied from `mfidl.h` in SDK 10.0.26100 [I/U].
3. **Format.** Over USB 2.0, 1280×720 YUY2 at 30 fps does not fit. It needs 55 MB/s, and USB 2.0 isochronous transfer tops out at about 24.6 MB/s [I from S]. So 720p30 means **MJPEG**, which then has to be decoded:
   - on the CPU by `mfmjpegdec.dll` [S], at an estimated 10–25 % of one core at 720p30 [I/U]
   - or by a hardware MJPEG MFT, if the GPU vendor registers one [U]
4. **Encoder.** A second hardware H.264 session is cheap:
   - NVENC: an extra 720p30 stream costs about 1.5–6 % of one encoder engine (from NVIDIA's own fps table) [I from D]. GeForce cards allow 8 sessions per system [D] (driver 551.76 or later on Windows [S]).
   - AMD and Intel: no documented session cap [S].
   - **MJPEG passthrough** (store the camera's JPEGs, no encode at all) is a good **fallback** and low-CPU mode. It needs about 5–12 × the memory of H.264: roughly 160–540 MB per 90 s, against about 35–45 MB for H.264 [I/U]. It also only works when the camera natively delivers MJPEG at the target size.
5. **Delay.** For certified USB cameras, Microsoft's HLK requires photon-to-display latency of at most 120 ms (MIPI cameras: at most 80 ms) [D]. When `DeviceTimestamp` is used, I suggest an automatic base shift of about −40 ms for the camera and a user slider of ±300 ms [I/U]. A clap test should confirm the number.
   - Keep the delay **out of the encoded stream**: store the QPC anchor and apply the offset only on save. The editor can then re-sync each clip.
6. **VFR → CFR.** In low light, auto-exposure priority drops the frame rate to around 15 fps. This is by design: the HLK target is 15 fps at 20 lux [D]. Place each camera frame on a fixed 30 fps grid that is anchored to QPC, and repeat the last frame where a slot is empty. This keeps ClippiBoy's existing `-f h264 -r <rate>` muxing unchanged.

---

## 1. What the repo does today (read-only survey)

| Item | Where | Clock / behaviour |
|---|---|---|
| Game frame time | `src-tauri/src/wgc.rs:247` | `frame.SystemRelativeTime()?.Duration`, a QPC value in 100 ns, stored as `qpc_100ns` |
| Audio time | `src-tauri/src/audio/capture.rs:644-685` | `IAudioCaptureClient::GetBuffer(..., Some(&mut qpc_100ns))`. A sanity check, `usable_stamp()`, falls back to `now_100ns()` if a value is more than 2 s away from now, and sets `fallback_clock`. |
| "Now" | `audio/capture.rs:21-45` `now_100ns()` | QPC/QPF converted to 100 ns, computed as whole seconds plus remainder |
| CFR clock | `src-tauri/src/convert.rs:736-795` `pace()` | A thread ticks at 1/fps against an absolute start. It repeats the last frame when none arrived (`duplicated`) and skips forward when more than 250 ms behind (`skipped`). Output `pts = (index-1)/fps`. |
| Anchor | `src-tauri/src/pipeline.rs:613-626` | On the first frame, `base_100ns = frame_qpc - pts`. `qpc_of(pts_us) = base + pts_us*10`. The clip's audio is cut at `qpc_of(first.pts_us)`. |
| Muxing | `src-tauri/src/muxer.rs:316` | Raw Annex-B stream with `-f h264 -r <rate>`, so **video timestamps come from a constant frame rate**. This forces the cam track to be CFR too, or to use another container. |
| MF startup | `src-tauri/src/mft.rs:149-158` | `MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)` once per process (`OnceLock`), with no `MFShutdown` |
| Encoder | `mft.rs:680-800` | Hardware MFT only. It gets a D3D manager from `MFCreateDXGIDeviceManager` + `ResetDevice(gpu.device)`, and is fed NV12 textures through `MFCreateDXGISurfaceBuffer`. The software MFT is rejected because it answers `SET_D3D_MANAGER` with `E_NOTIMPL`. |
| D3D device | `gpu.rs:202-232` | `BGRA_SUPPORT \| VIDEO_SUPPORT`, `SetMultithreadProtected(true)` |
| COM | `audio/devices.rs:33`, `audio/capture.rs:500` | `CoInitializeEx(None, COINIT_MULTITHREADED)` on worker threads |
| `#[implement]` style (0.58) | `audio/capture.rs:211-223` | `impl IFoo_Impl for Foo_Impl`, with parameters as `Option<&IBar>` |
| windows features | `src-tauri/Cargo.toml` | Already includes `Win32_Media_MediaFoundation`, `implement`, `Win32_System_Com(_StructuredStorage)`, `Win32_Graphics_Direct3D11`, `Win32_Graphics_Dxgi(_Common)`, `Win32_System_WinRT(_Direct3D11)`, `Graphics_DirectX_Direct3D11` |

---

## 2. Q1: Enumerating and opening cameras, hotplug, device loss

### 2.1 Enumeration [D]

`MFCreateAttributes` → `SetGUID(MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)` → `MFEnumDeviceSources`. This returns a CoTaskMem array of `IMFActivate`. You release each element and then `CoTaskMemFree` the array.
https://learn.microsoft.com/en-us/windows/win32/medfound/enumerating-video-capture-devices

- **Friendly name:** `MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME`, read with `GetAllocatedString` and freed with `CoTaskMemFree`.
- **Stable id:** `MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK`. The docs say it "should be considered an opaque string" and that it can be passed back to `MFCreateDeviceSourceActivate` or used with `MFCreateDeviceSource` [D].
  https://learn.microsoft.com/en-us/windows/win32/medfound/mf-devsource-attribute-source-type-vidcap-symbolic-link
  - The link contains the PnP device-interface path. For USB devices without a serial number, that path can change when the camera moves to another port [U, general PnP behaviour, not in MF docs].
  - Store the link and the friendly name. Match on the link first; fall back to a unique friendly name.
- **Opening by link:** set `MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE` + `..._VIDCAP_SYMBOLIC_LINK` (+ the share mode, see 2.4) on an attribute store and call `MFCreateDeviceSource(attrs)`. Microsoft's own sample does exactly this [D].
  https://learn.microsoft.com/en-us/windows/win32/medfound/mf-devsource-attribute-frameserver-share-mode
- **`IMFActivate` cache gotcha:** "The IMFActivate object will internally cache a newly created IMFMediaSource from the call to ActivateObject. So if ActivateObject is called again before a call to DetachObject is made, the cached instance will be returned and any changes to the attributes will be ignored." [D, same page]. Whoever calls `ActivateObject` must call `ShutdownObject` when done [D].
  https://learn.microsoft.com/en-us/windows/win32/api/mfobjects/nf-mfobjects-imfactivate-shutdownobject
- **Privacy denial:** if the user has turned the camera off, `ActivateObject` returns **`E_ACCESSDENIED`** [D] (section 7).

### 2.2 Hotplug [D]

There is no MF-specific hotplug API. I found no `IMFDeviceSourceEx` in the docs, so it is probably not a real interface. The options:

- **`CM_Register_Notification`** with `CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE` and the class `KSCATEGORY_VIDEO_CAMERA`. Windows 8 and later; the callback runs on a system thread and needs no window.
  - Arrival and removal come as `CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL` / `...REMOVAL` with the interface's symbolic link in `EventData->u.DeviceInterface`.
  - "Handle … as quickly as possible", so hand work off to another thread.
  - It "does not provide notification of existing device interfaces": register first, then enumerate.
  - https://learn.microsoft.com/en-us/windows/win32/api/cfgmgr32/nf-cfgmgr32-cm_register_notification
  - windows-rs: `windows::Win32::Devices::DeviceAndDriverInstallation::CM_Register_Notification(pfilter, pcontext, pcallback, pnotifycontext) -> CONFIGRET`. The constant `windows::Win32::Media::KernelStreaming::KSCATEGORY_VIDEO_CAMERA` exists in the crate (seen in the 0.62.2 docs; assumed present in 0.58 [U]).
- **`RegisterDeviceNotification` + `WM_DEVICECHANGE`.** Microsoft's "Handling Video Device Loss" sample registers `KSCATEGORY_CAPTURE` on an HWND and compares `dbcc_name` with the symbolic link using `_wcsicmp` [D].
  https://learn.microsoft.com/en-us/windows/win32/medfound/handling-video-device-loss
  This is awkward in a Tauri backend because it needs a window.
- **WinRT `DeviceWatcher`** (`DeviceInformation.CreateWatcher` with the video-capture selector, or `MediaFrameSourceGroup.GetDeviceSelector`) [D].
  https://learn.microsoft.com/en-us/windows/apps/develop/camera/process-media-frames-with-mediaframereader
  This needs `Devices_Enumeration`.

**Recommendation:** use `CM_Register_Notification` on `KSCATEGORY_VIDEO_CAMERA`, compare links case-insensitively, and debounce for about 500 ms, because arrival events come before the device is fully ready [I].

### 2.3 Device loss and errors seen while streaming

| HRESULT | Value | Meaning | Source |
|---|---|---|---|
| `MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED` | 0xC00D3EA2 | "The video recording device is no longer present." | [S] https://windows-hexerror.linestarve.com/0xC00D3EA2 |
| `MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED` | 0xC00D3EA3 | "preempted by another immersive application" | [S] https://windows-hexerror.linestarve.com/0xC00D3EA3 |
| `MF_E_HW_MFT_FAILED_START_STREAMING` | 0xC00D3704 | "Hardware MFT failed to start streaming due to lack of hardware resources." Chromium: "This may indicate that the camera is in use by another application." It then asks `IMFSensorActivityMonitor` which processes hold the camera. | [S] https://windows-hexerror.linestarve.com/0xC00D3704 ; Chromium `video_capture_device_mf_win.cc` |
| `E_ACCESSDENIED` | 0x80070005 | Camera privacy switch off | [D] https://learn.microsoft.com/en-us/windows/apps/develop/camera/camera-privacy-setting |
| sharing violation | ? | Raised when a second **controlling** instance becomes active. The doc says "sharing violation" but names no HRESULT. The UWP path reports 0x80070020 [S]. | [D] share-mode page; [U] exact MF code |
| `DXGI_ERROR_DEVICE_REMOVED` | | Only on the D3D path. Chromium caps D3D restarts (`kMaxD3DRestarts`) and then falls back to the software path. | [S] Chromium |

- **In the source reader:** a failure arrives as `hrStatus` in `OnReadSample`, or as `MF_SOURCE_READERF_ERROR`. After that flag: "do not make any further calls to IMFSourceReader methods" [D].
  https://learn.microsoft.com/en-us/windows/win32/api/mfreadwrite/ne-mfreadwrite-mf_source_reader_flag
- **`MEError`:** "Signals a serious error. Any Media Foundation component can send this event at any time." Recoverable streaming errors use `MENonFatalError` instead [D].
  https://learn.microsoft.com/en-us/windows/win32/medfound/meerror

**Recovery** [I]:

1. Release the reader. By default this shuts down the media source [D, see 3.6].
2. Mark the cam track as "lost" (filler frames, see 9.6).
3. Retry with backoff (1 s, 2 s, 5 s, then every 10 s), plus immediately on a CM arrival for the same link.
4. On `E_ACCESSDENIED`, stop retrying and tell the user (section 7).
5. On "in use", query the `IMFSensorActivityMonitor` (`MFCreateSensorActivityMonitor(callback) -> Result<IMFSensorActivityMonitor>` in windows-rs [D-crate]) to name the process holding the camera, then retry in sharing mode.

### 2.4 Sharing mode in MF (important for Discord) [D]

`MF_DEVSOURCE_ATTRIBUTE_FRAMESERVER_SHARE_MODE` (UINT32). **Minimum client: Windows 11, build 26100.**
https://learn.microsoft.com/en-us/windows/win32/medfound/mf-devsource-attribute-frameserver-share-mode

- 0 or not set means **controlling** mode (the default). "Only one controlling mode instance of IMFMediaSource can be in an active state at any time."
  - A source becomes active only on `Start` or when an extended camera control is set.
  - "Attempting to render active an IMFMediaSource instance when another controlling mode instance is already active will result in a sharing violation."
- 1 means **sharing** mode. "At any time there can be an arbitrary number of active sharing mode instances … newly created sharing mode instances can be activated while a controlling mode instance is active. Sharing mode instances cannot change media types and must use the current media type in use."
  - They cannot change `KSPROPERTYSETID_ExtendedCameraControl` controls.
  - They **can** change legacy `PROPSETID_VIDCAP_CAMERACONTROL` / `PROPSETID_VIDCAP_VIDEOPROCAMP` controls.
- The attribute has to be set at creation time: on the attrs passed to `MFCreateDeviceSource`, or on the `IMFActivate` before `ActivateObject`.
- **GUID:** not found in `windows` 0.58, and the windows-docs-rs page for 0.62.2 does not exist either (404). Define it yourself, copying the `EXTERN_GUID` from `mfidl.h` in SDK ≥ 10.0.26100 [U value; I did not find it online].
- What a sharing instance gets when **no** controlling app runs (which "current media type") is not documented [U]. Test this first (E2/E3).

Related system feature: since 24H2 (build 26100), Settings has **"Allow multiple apps to use camera at the same time"** per camera ("multi-app camera"). Microsoft says it "allows multiple applications to access the camera stream simultaneously". With it on, "camera settings like brightness can only be adjusted from this specific settings page" [D/S].
- https://blogs.windows.com/windows-insider/2024/12/13/announcing-windows-11-insider-preview-build-26120-2702-dev-channel/
- https://pureinfotech.com/allow-multiple-apps-use-camera-windows-11/
- This is a good "if Discord can't see the camera" hint in the UI.

Older Microsoft camera-team statement (2020) describing the same model: one controlling app plus any number of shared apps. Shared apps get no camera controls and no media-type selection, can be notified when the controlling app releases the camera, and "the mechanism … does not require any copying of the captured frame" [S, Microsoft forum archive].
https://learn.microsoft.com/en-us/archive/msdn-technet-forums/12de4147-9fa0-4031-818f-2cec9a1d080b

---

## 3. Q2: IMFSourceReader for capture

### 3.1 Sync or async [D]

- **Sync:** `ReadSample` blocks until the next sample.
- **Async:** set `MF_SOURCE_READER_ASYNC_CALLBACK` (an `IMFSourceReaderCallback`) at creation. The mode cannot be switched later.
  - Call `ReadSample(stream, 0, NULL, NULL, NULL, NULL)`; exactly one `OnReadSample` comes back per call, and you re-issue `ReadSample` from there.
  - "The callback interface must be thread-safe, because OnReadSample and the other callback methods are called from worker threads."
  - `OnFlush` signals that `Flush` has finished.
  - https://learn.microsoft.com/en-us/windows/win32/medfound/using-the-source-reader-in-asynchronous-mode
- Flags to handle on every sample:
  - `MF_SOURCE_READERF_STREAMTICK` (0x100): "There is a gap in the stream."
  - `NATIVEMEDIATYPECHANGED` / `CURRENTMEDIATYPECHANGED`: relevant in sharing mode when Discord changes the format.
  - `ERROR`
  - [D] https://learn.microsoft.com/en-us/windows/win32/api/mfreadwrite/ne-mfreadwrite-mf_source_reader_flag
- **Buffering:** "By default, the video capture source buffers a maximum of one frame at a time" (`MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_MAX_BUFFERS`). Holding samples therefore stalls or drops frames, so release each `IMFSample` quickly [D].
  https://learn.microsoft.com/en-us/windows/win32/medfound/mf-devsource-attribute-source-type-vidcap-max-buffers
- **Recommendation:** use **async**. A sync `ReadSample` can block indefinitely when a camera stalls, and async makes stopping deterministic: stop re-issuing reads, `Flush`, wait for `OnFlush`, release. Have the callback push into a small bounded `crossbeam_channel` (already a dependency). Windows-rs COM pointers are not `Send`, so wrap them in the existing `gpu::SendPtr` [I].

### 3.2 Video processing flags [D]

- `MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING`: YUV→RGB32 and deinterlacing, done **in software**, "not optimized for playback". It must be FALSE when `MF_SOURCE_READER_D3D_MANAGER` is set. **Don't use it.**
  https://learn.microsoft.com/en-us/windows/win32/medfound/mf-source-reader-enable-video-processing
- `MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING` (Win8+): colour conversion, deinterlacing, resizing and **frame-rate conversion** through a video processor MFT (the XVP). "Some conversions can be performed in hardware using the GPU."
  https://learn.microsoft.com/en-us/windows/win32/medfound/mf-source-reader-enable-advanced-video-processing
  It is usable, but ClippiBoy already has its own `ID3D11VideoProcessor` in `convert.rs`. Keep conversion there, and don't let MF do frame-rate conversion, because that would hide the true capture times [I].
- `MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS`: "By default, the source reader … do[es] not use hardware decoders or encoders. To enable … set this attribute to TRUE." It "does not affect hardware-accelerated video decoding that uses DXVA"; for that, set `MF_SOURCE_READER_D3D_MANAGER`.
  https://learn.microsoft.com/en-us/windows/win32/medfound/mf-readwrite-enable-hardware-transforms
- `MF_SOURCE_READER_D3D_MANAGER` (`IMFDXGIDeviceManager`): "provide a Direct3D device for any video decoders loaded by the source reader … the source reader uses the Direct3D device to allocate video buffers". It is not needed when you read compressed (passthrough) video.
  https://learn.microsoft.com/en-us/windows/win32/medfound/mf-source-reader-d3d-manager

### 3.3 MJPEG → NV12

- Asking for an uncompressed subtype while the native type is compressed makes the reader "search for a decoder that can decode from the native format to the specified uncompressed format" [D].
  https://learn.microsoft.com/en-us/windows/win32/api/mfreadwrite/nf-mfreadwrite-imfsourcereader-setcurrentmediatype
- The in-box decoder is the **Media Foundation MJPEG Decoder** (`mfmjpegdec.dll`), a CPU decoder [S].
  - https://www.nirsoft.net/dll_information/windows8/mfmjpegdec_dll.html
  - Developers report frame drops with it and difficulty getting Intel's "Hardware M-JPEG Decoder MFT" to appear [S]: https://community.intel.com/t5/Media-Intel-Video-Processing/Intel-Hardware-M-JPEG-Decoder-MFT/td-p/1004407
- Chromium's MF path, which uses the capture engine, maps MJPG→I420 on the software path and MJPG→NV12 when a DXGI manager is present. Its comment: "IMFCaptureEngine inevitably performs the video frame decoding itself." [S]
  https://raw.githubusercontent.com/chromium/chromium/main/media/capture/video/win/video_capture_device_mf_win.cc
- **CPU cost** [I/U, estimate]:
  - Pixel rates: 720p30 = 27.6 MP/s, 1080p30 = 62.2 MP/s.
  - A SIMD JPEG decoder (libjpeg-turbo class) on a current desktop core manages very roughly 100–300 MP/s. That gives about **10–25 % of one core at 720p30** and **20–60 % at 1080p30**.
  - The speed of `mfmjpegdec` is unknown, so measure it (E5).
  - For log output, `IMFSourceReaderEx::GetTransformForStream` shows which decoder was inserted (hardware or software).
- **Prefer a 720p native type over 1080p plus downscaling.** It halves the decode cost and the USB load [I].

### 3.4 Getting D3D11 textures

- A USB UVC camera delivers frames into **system memory**: they arrive by USB DMA and then pass through the Frame Server. A GPU texture without an upload only exists if the decoder runs on the GPU (DXVA or hardware MJPEG MFT) [I].
- Chromium checks each sample for `IMFDXGIBuffer`. It only takes the texture path if the DXGI manager exists, the format is NV12, and getting the texture succeeds; otherwise it falls back to system memory [S, Chromium source]. Do the same:
  1. `buffer.cast::<IMFDXGIBuffer>()` → `GetResource(ID3D11Texture2D)` + `GetSubresourceIndex` → `CopySubresourceRegion` into a ClippiBoy NV12 slot. This works only if the texture lives on ClippiBoy's device, i.e. the same `gpu.device` was handed in through the manager.
  2. Otherwise `IMF2DBuffer::Lock2D` → copy into a STAGING NV12 texture (Map: Y plane, then the UV plane at `RowPitch*Height` [U: verify layout]) → `CopyResource` into a DEFAULT slot.
     - Upload cost: 1280×720×1.5 B × 30 fps = **41.5 MB/s**, negligible over PCIe [I].
- Whether the device source receives the D3D manager is not documented. `IMFMediaSourceEx::SetD3DManager` exists [D] but the docs don't say who calls it, and whether the Frame Server then hands out DXGI buffers for UVC cameras is unknown [U].
  https://learn.microsoft.com/en-us/windows/win32/api/mfidl/nf-mfidl-imfmediasourceex-setd3dmanager
- Then reuse `mft::Transform` unchanged. It already takes NV12 textures on `gpu.device`.

### 3.5 Choosing the native type and frame rate

1. Enumerate with `GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM, i)` until it errors. For each type read:
   - `MF_MT_SUBTYPE`
   - `MF_MT_FRAME_SIZE` (u64: width in the high 32 bits, height in the low 32)
   - `MF_MT_FRAME_RATE` (u64: numerator high, denominator low)
   - optionally `MF_MT_FRAME_RATE_RANGE_MIN/MAX`
2. To pick a rate, set `MF_MT_FRAME_RATE` on the chosen native type before `SetCurrentMediaType` [D].
   https://learn.microsoft.com/en-us/windows/win32/medfound/how-to-set-the-video-capture-frame-rate
3. Then either:
   - set the native type itself (**passthrough**: no decoder, MJPEG bytes come out), or
   - set a partial type (`MFMediaType_Video` + `MFVideoFormat_NV12`) so the reader inserts the decoder [D, SetCurrentMediaType remarks].
   - `MF_E_INVALIDREQUEST` means requests are still pending [D].
4. **Scoring** [I]:
   - exact 1280×720 at ≥ 29.97 fps, NV12 > YUY2 > MJPG (uncompressed saves the decode)
   - else 1280×720 MJPG at ≥ 29.97
   - else the smallest ≥ 720p at ≥ 29.97 (MJPG), scaled by the GPU
   - else the best available, with a warning
5. **USB 2.0 arithmetic** [I]:
   - High-bandwidth isochronous: 3 × 1024 B per 125 µs microframe = 24.576 MB/s max [S: https://patchwork.kernel.org/comment/2109752].
   - YUY2 720p = 1.84 MB/frame, so at most about 13 fps. This explains the reported **C920 720p YUY2 = 10 fps** [S: https://obsproject.com/forum/threads/logitech-c920-blurry.84690].
   - YUY2 1080p allows at most about 5.9 fps.
   - So on the C920 and, presumably, the eMeet C960 (spec: YUY2/MJPEG, 1080p30 [S retailer]), **720p30 means MJPEG.**
6. The newer C920 revisions may or may not still offer UVC H.264 [S/U]. Don't rely on it.

### 3.6 Lifetime

`MFCreateSourceReaderFromMediaSource`:
- "Call CoInitialize(Ex) and MFStartup before calling this function."
- "By default, when the application releases the source reader, the source reader shuts down the media source." (`MF_SOURCE_READER_DISCONNECT_MEDIASOURCE_ON_SHUTDOWN` changes this.)
- Do not call `Start/Stop/Pause` or the event-generator methods on the source yourself.
- [D] https://learn.microsoft.com/en-us/windows/win32/api/mfreadwrite/nf-mfreadwrite-mfcreatesourcereaderfrommediasource

---

## 4. Q3: Timestamps

### 4.1 Which clock

| Source | Value | Clock | Ref |
|---|---|---|---|
| WGC | `Direct3D11CaptureFrame.SystemRelativeTime` | "The QPC … time at which the compositor rendered the frame" (TimeSpan, 100 ns) | [D] https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframe.systemrelativetime |
| WASAPI | `pu64QPCPosition` | "the value of the performance counter at the time that the audio endpoint device recorded the device position of the first audio frame", converted as `10,000,000·t/f`, in 100 ns | [D] https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiocaptureclient-getbuffer |
| MF camera | `MFSampleExtension_DeviceTimestamp` (UINT64) | "set on media samples created by a media source for a capture device … in the MFTIME domain, sharing an epoch with query performance counter (QPC) time and always expressed in 100ns units." | [D] https://learn.microsoft.com/en-us/windows/win32/medfound/mfsampleextension-devicetimestamp |
| MF camera | `IMFSample::GetSampleTime` | Same page: "To get the time stamp relative to the start of streaming, call IMFSample::GetSampleTime" | [D wording; U whether really rebased] |
| MF camera | `MFSampleExtension_DeviceReferenceSystemTime` | "may or may not carry the actual value of the query performance counter (QPC), depending on the source … may be modified by other components" | [D] https://learn.microsoft.com/en-us/windows/win32/medfound/mfsampleextension-devicereferencesystemtime |
| MF platform | `MFGetSystemTime()` | Frame Server docs: "a wrapper around QueryPerformanceCounter and converts the QPC ticks to 100-nanosecond units". Also: "All sample timestamps must be based on QPC time". The sample source stamps `SetSampleTime(MFGetSystemTime())`. | [D] https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/frame-server-custom-media-source |
| MF HW source | `MFT_HW_TIMESTAMP_WITH_QPC` | "If this attribute is TRUE, the device source uses the system time, as returned by the QueryPerformanceCounter, for time stamps. Otherwise … the device's clock. The default value is FALSE." | [D] https://learn.microsoft.com/en-us/windows/win32/medfound/mft-hw-timestamp-with-qpc-attribute |
| WinRT | `MediaFrameReference.SystemRelativeTime` | `IReference<TimeSpan>`: "This value uses 100ns interval QPC time." Nullable. | [D] https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.frames.mediaframereference.systemrelativetime |

**Conclusion** [D + I]: WGC, WASAPI, `DeviceTimestamp`, `MFGetSystemTime` and WinRT `SystemRelativeTime` are **one clock**: QPC scaled to 100 ns. ClippiBoy's `now_100ns()` produces the same value (it differs only in rounding).

How Chromium handles it [S]:
- It reads `MFSampleExtension_DeviceReferenceSystemTime` and falls back to `MFGetSystemTime()`.
- It converts to `TimeTicks` through `mf_time_offset = TimeTicks::Now() - MFGetSystemTime()`. Its UMA histogram for that offset spans only ±150 ms, so Chromium expects the two clocks to agree [I].
- It forces monotonic values (+1 µs when a value does not advance).
- It forwards the sample time separately (`GetSampleTime`) as `timestamp`.

**Checking plausibility, as the audio code does** [I]:
- Take `DeviceTimestamp`. If it is missing or more than 2 s from `now_100ns()`, try `DeviceReferenceSystemTime`.
- Then try `GetSampleTime` if it is itself plausible as QPC.
- Then `GetSampleTime + learned_offset`, where the offset is learned from an earlier sample that carried both values.
- Last resort: the arrival time `now_100ns()`, with `fallback_clock = true`.
- Whether `DeviceTimestamp` survives through the MJPEG decoder MFT, i.e. whether MFTs copy sample attributes, is unknown [U]. With passthrough there is no MFT in between.

### 4.2 Gaps

- `MFSampleExtension_Discontinuity`: "this sample is the first to appear after the gap" [D]. https://learn.microsoft.com/en-us/windows/win32/medfound/mfsampleextension-discontinuity-attribute
- `MF_SOURCE_READERF_STREAMTICK`: gap in the stream [D].

### 4.3 What the timestamp represents, and latency

- **Not documented for UVC.** `usbvideo.sys` "can serve as a KS master clock and add time stamps to video samples" [D], but the docs don't say whether the stamp is taken at the first packet, the last packet, or start of exposure [U].
  https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/usb-video-class-implementation
  UVC payload headers carry PTS/SCR from the first and last payloads of a frame [D]: https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/uvc-extensions-1-5
- **HLK requirements (Microsoft):**
  - "Video latency measures the time for photons into the camera to photons out of the display. The video latency requirement is ≤ 80ms for MIPI cameras and ≤ 120ms for USB cameras"
  - glitch-free means "max inter-frame time ≤ 133ms at 20 lux, … ≤ 66ms at 80 lux, and jitter ≤ 7ms"
  - "frame rate must be ≥ 14 FPS in 20 lux … ≥ 29 FPS in 80 lux … target 15 FPS at 20 lux and 30 FPS at 80 lux"
  - "CPU usage must be ≤ 5%"
  - [D] https://learn.microsoft.com/en-us/windows-hardware/test/hlk/testref/troubleshooting-devicestreaming-testing
- **Measurements:**
  - 75–130 ms photon-to-screen for cheap UVC cameras [S: https://makehardware.com/2016/03/29/finding-a-low-latency-webcam/]
  - About 100–170 ms with OpenCV/GStreamer [S: https://forums.developer.nvidia.com/t/usb-camera-high-latency-on-tx1/55532]
- **Estimate for ClippiBoy** [I/U]: photon → `DeviceTimestamp` is roughly 40–90 ms. That is photon-to-display minus display and render (about 20–40 ms), and is roughly half an exposure plus readout, in-camera JPEG encoding and USB transfer of about one frame. The WASAPI microphone stamp is close to ADC time. **The camera therefore runs late against the voice by about one to two frame intervals.**
- **How others compensate** [S]:
  - OBS does nothing automatically. Its DirectShow source passes the driver's start time (`frame.timestamp = startTime * 100`) and leaves sync to the user: a "Render Delay" filter on the camera when the video runs ahead, or "Sync Offset" on the audio when the audio runs ahead.
  - https://raw.githubusercontent.com/obsproject/obs-studio/master/plugins/win-dshow/win-dshow.cpp
  - https://obsproject.com/forum/threads/cant-find-audio-delay-to-sync-with-camera.28562
- **Default** [I/U]: a cam offset of **−40 ms** (shift camera frames earlier) when `DeviceTimestamp` is used, and **−80 ms** when falling back to arrival time. Slider range −300 … +300 ms in 10 ms steps. Settle the real default with the clap test (E6). Since the offset is applied on save, the editor can correct it per clip.

### 4.4 Variable frame rate

- `KSPROPERTY_CAMERACONTROL_AUTO_EXPOSURE_PRIORITY`: 0 means the frame rate stays fixed; 1 means the device may lower it. The doc's example drops 30 → 25 fps for 40 ms exposure. It maps to the "Low-Light Compensation" checkbox, default 0, and needs auto exposure [D].
  https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/ksproperty-cameracontrol-auto-exposure-priority
  - Logitech's RightLight / gain settings commonly cause the C920 frame drops [S].
  - Sharing instances may change these legacy controls [D share-mode page]. **Don't**: it would also change Discord's picture. At most, offer a setting.
- **Handling:** snap frames onto a QPC-anchored 30 fps grid and repeat the last frame in empty slots (section 9.5). 15 fps then looks like every frame shown twice. That is correct timing and is what ClippiBoy already does for still game pictures.

---

## 5. Q4: WinRT MediaCapture + MediaFrameReader

- **Sharing** [D]:
  - `MediaCaptureInitializationSettings.SharingMode = SharedReadOnly`, Windows 10 1607+: "Multiple apps can simultaneously acquire frames from the same MediaFrameSource, but only a single active MediaCapture instance can acquire exclusive control … Attempting to render active a MediaCapture instance when another controlling mode instance is already active will result in a sharing violation failure."
  - In SharedReadOnly, `PreviewMediaDescription`, `RecordMediaDescription` and `VideoProfile` cannot be configured.
  - https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.mediacaptureinitializationsettings.sharingmode
  - `MediaFrameSource.FormatChanged` fires when the controlling app changes the format [D]: https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.frames.mediaframesource.formatchanged
  - `MediaFrameReaderStartStatus.ExclusiveControlNotAvailable` (1703+) [D]: https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.frames.mediaframereaderstartstatus
- **Frames:**
  - `MemoryPreference = Cpu` guarantees `SoftwareBitmap`. With `Auto`, frames may arrive as `IDirect3DSurface` [D].
  - If you keep `SoftwareBitmap`/`Direct3DSurface` references, "the flow of new frames will stop". Frames from `CreateFrameReaderAsync(source, subtype)` are copies and do not block [D].
  - `AcquisitionMode` Realtime (the default) drops to the latest frame; Buffered keeps order [D].
  - `FrameArrived` "is raised … on its own thread" [D].
  - https://learn.microsoft.com/en-us/windows/apps/develop/camera/process-media-frames-with-mediaframereader
- **Device:** to make MediaCapture use ClippiBoy's D3D device, `IAdvancedMediaCaptureInitializationSettings::SetDirectxDeviceManager(IMFDXGIDeviceManager)` exists (Win8+) [D].
  https://learn.microsoft.com/en-us/windows/win32/api/mfmediacapture/nn-mfmediacapture-iadvancedmediacaptureinitializationsettings
  Obtaining it by QI from `MediaCaptureInitializationSettings` is [U].
- **Timestamps:** `MediaFrameReference.SystemRelativeTime` is QPC in 100 ns [D]. It is the best-documented camera timestamp of all.
- **Privacy:** `InitializeAsync` fails with `E_ACCESSDENIED` (0x80070005) [D].
- **windows-rs 0.58 cost:**
  - Additional features: `Media_Capture`, `Media_Capture_Frames`, `Media_MediaProperties`, `Media_Devices`, `Devices_Enumeration`, `Foundation_Collections`, `Graphics_Imaging`, optionally `Security_Authorization_AppCapabilityAccess` [I: feature = namespace rule of windows-rs]. `Graphics_DirectX_Direct3D11` is already on.
  - Async calls go through `.get()` on `IAsyncOperation`/`IAsyncAction`. That blocks, so use a dedicated MTA thread and never the WebView/STA main thread [I].
  - Handlers use `TypedEventHandler::new(...)`.
  - It adds noticeably more code and compile time than the MF path [I].
- **Comparison:**

| | IMFSourceReader | MediaCapture / FrameReader |
|---|---|---|
| Non-blocking sharing | Win11 26100+ (`FRAMESERVER_SHARE_MODE`), GUID defined by hand | Win10 1607+ (`SharedReadOnly`) |
| Format control | Full, in controlling mode | Full only in ExclusiveControl |
| MJPEG passthrough | Yes (set native type) | Not obvious; decoded frames are the normal path [U] |
| Timestamp | `DeviceTimestamp` (documented QPC epoch) | `SystemRelativeTime` (documented QPC) |
| Fits existing code | `mft.rs` style, features already present | New WinRT surface |
| Notify when control frees | No direct API; poll / activity monitor | `CaptureDeviceExclusiveControlStatusChanged` [S] |

- **For coexisting with Discord:** on the test machine (26200) both APIs can open the camera without taking it from anyone. MF sharing mode is enough there and keeps one code path. Add MediaCapture `SharedReadOnly` **only for Windows 10**, and only if test E2 shows ClippiBoy blocking Discord on Windows 10 [I].
  - Whether Discord itself opens in controlling mode, and with which API, is unknown [U]. Ordinary MF/DirectShow apps default to controlling mode, and Windows 10+ routes both MF and DirectShow capture through the Frame Server [S: OSR thread https://community.osr.com/t/windows-10-anniversary-update-and-testcap-avstream-drivers/53081].

---

## 6. Q5: A second encoder, or MJPEG passthrough

### 6.1 NVENC / AMF / QSV

- **NVIDIA** [D]:
  - "On non-qualified GPUs, the number of concurrent encode sessions is limited to 8 per system." The limit counts across all cards in the system.
  - Throughput for H.264 1080p per NVENC engine (Ada): P1 910 fps, P3 652, P5 291, P7 211. "NVENC hardware natively supports multiple hardware encoding contexts with negligible context-switching penalty."
  - https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/nvenc-application-note/index.html
  - SDK 12.1 still said 5 [D: https://docs.nvidia.com/video-technologies/video-codec-sdk/12.1/pdf/NVENC_Application_Note.pdf]; 3 before 2023 [S: https://guru3d.com/story/nvidia-removes-encoding-limitations-on-consumer-gpus-allowing-up-to-5-simultaneous-streams].
  - Windows driver ≥ 551.76 is needed for 8 [S: https://forums.developer.nvidia.com/t/how-to-increase-number-of-nvenc-concurrent-sessions/169367]. The GTX 1630 is reported at 3 [S].
- **Load** [I]:
  - 720p30 is 0.444 × 30 = 13.3 "1080p-equivalent" frames per second, so **≈ 1.5 % (P1) to ≈ 6 % (P7) of one Ada NVENC.**
  - For comparison, the game at 1080p60 is about 7–28 % and 1440p60 about 12–50 %.
  - NVIDIA says throughput scales with clock rather than linearly with resolution, so this is only an order of magnitude.
- **Sessions in use at the same time** [I]:
  - game encoder (1)
  - cam (1)
  - ffmpeg `h264_nvenc` on save/export (1, short)
  - plus Discord, NVIDIA App or OBS if running
  - That fits in 8. On old drivers (3–5) ClippiBoy should handle a failure to open the cam encoder (section 9.8).
- **AMD AMF / Intel QSV:** no fixed session cap is documented; secondary sources say throughput is the only limit [S].
  - https://minipclab.com/tools/plex-transcode-calculator
  - https://support.medialooks.com/hc/en-us/articles/360000210192-H-264-encoding-options

### 6.2 CPU fallback

- **Microsoft H.264 MFT** (`mfh264enc.dll`) [D]:
  - Input I420/IYUV/**NV12/YUY2**/YV12 in system memory; Baseline/Main/High profiles.
  - CBR/VBR/quality modes; `CODECAPI_AVLowLatencyMode`; `AVEncCommonQualityVsSpeed`.
  - https://learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-encoder
  - The camera's decoded frames are in system memory anyway, so this path needs **no GPU at all** and sidesteps the `SET_D3D_MANAGER`/`E_NOTIMPL` issue `mft.rs` hits. Just don't send it the D3D manager.
  - Its CPU cost at 720p30 is not documented [U]. Expect on the order of 0.3–1 core with low complexity settings [I].
- **x264** (anecdote): 720p30 veryfast showed 12–19 % total CPU on a 4-core i5-6600 with game content [S: https://obsproject.com/forum/threads/cpu-usage-is-insanely-higher-than-before.62060/post-269065]. A facecam is easier content.
  - Linking libx264 (GPL) is incompatible with ClippiBoy's "All rights reserved" licence. Piping raw frames into a separate `ffmpeg` process works the same way ClippiBoy already uses ffmpeg, but needs about 41 MB/s through a pipe [I; licence question to be judged by the owner].

### 6.3 MJPEG passthrough as the cam track

- **How it works:** set the native MJPG type, with no decoder and no encoder. The ring holds `Arc<[u8]>` JPEGs plus their QPC.
  - Every frame is intra-coded, so cuts are exact and the ring can be trimmed frame by frame. No keyframe rule.
  - CFR repetition is free: repeat the `Arc`, not the bytes.
- **Size** [I/U]:
  - USB 2.0 allows at most about 820 KB per frame at 30 fps.
  - Typical webcam JPEGs at 720p are estimated at 60–200 KB (about 0.5–1.7 bit/pixel). Measure on both cameras (E4).

| per frame | Mbit/s | 90 s (2,700 frames) | 120 s (ClippiBoy default `buffer.seconds = 120`) |
|---|---|---|---|
| 60 KB | 14.4 | 162 MB | 216 MB |
| 100 KB | 24 | 270 MB | 360 MB |
| 150 KB | 36 | 405 MB | 540 MB |
| 200 KB | 48 | 540 MB | 720 MB |
| H.264 720p30 @ 3 Mbit/s | 3 | 34 MB | 45 MB |
| H.264 720p30 @ 4 Mbit/s | 4 | 45 MB | 60 MB |

- **On save:** ffmpeg decodes MJPEG fine. Webcam JPEGs often "lack the DHT segment required for decoding" and ffmpeg's decoder supplies the standard tables [D: https://ffmpeg.org/ffmpeg-bitstream-filters.html, section `mjpeg2jpeg`].
  - The clean cam copy in the app folder (`stems.rs`) should be **transcoded to H.264 on save**. Otherwise every clip leaves 160–540 MB of cam beside it [I].
- **Verdict** [I]:
  - It is the lightest option while playing: no decode, no encode, a few MB/s of memcpy.
  - It costs 5–12 × the memory, and the console preview and later the virtual camera still need a decoder.
  - It only works if the native type is MJPEG at the wanted size. In sharing mode the controlling app may have chosen YUY2.
  - Use it as the **fallback** when the second hardware session can't be opened, and as an optional "low CPU" mode. The **default stays hardware H.264**, which works with any input format and matches how ClippiBoy stores packets.

---

## 7. Q6: Camera privacy

- **Denied access:**
  - "The Windows camera capture APIs will return the error E_ACCESSDENIED when apps attempt to access the camera capture device if the user has disabled camera in the camera privacy Settings page."
  - Microsoft's Win32 example checks `hr == E_ACCESSDENIED` after `IMFActivate::ActivateObject` and opens `ms-settings:privacy-webcam` with `ShellExecute` (`Win32_UI_Shell` is already a feature).
  - Access can be disabled "for the entire device, for all unpackaged apps, or for individual packaged apps".
  - `AppCapability.Create("Webcam").CheckAccess()` is recommended **for packaged apps** only.
  - [D] https://learn.microsoft.com/en-us/windows/apps/develop/camera/camera-privacy-setting
  - For unpackaged ClippiBoy, detect the denial from the error. The `CapabilityAccessManager\ConsentStore\webcam\NonPackaged` registry values are undocumented forensic artefacts; don't rely on them [S].
- **Coming change:** Windows 11 Insider build 26340.9233 (Aug 2026) tests **per-app** camera, microphone and location permissions for desktop apps. The single "Let desktop apps access your camera" toggle disappears and each desktop app gets its own switch; some apps may show as "unsigned" if the publisher can't be verified [S].
  - https://www.bleepingcomputer.com/news/microsoft/microsoft-tests-new-privacy-controls-for-windows-11-desktop-apps/amp/
  - https://windowslatest.com/2026/08/19/windows-11-is-getting-a-major-privacy-upgrade-blocks-apps-spying-on-your-camera-and-mic-with-new-controls
  - **ClippiBoy should be code-signed** so it appears under its own name. Smartphone-style prompts are announced but not tied to a build [S/U]. Treat `E_ACCESSDENIED` as a normal state at any time, including mid-session.
- **Indicators** [S]:
  - The camera LED (where present, e.g. the C920) is on while the camera streams.
  - Windows shows "camera on/off" notifications on devices without an LED.
  - Settings › Privacy › Camera › "Recent activity" lists apps from the last 7 days.
  - https://geekrewind.com/how-to-see-which-apps-have-accessed-your-camera-in-windows-11/
  - Consequence: **while the buffer runs, the LED is on and ClippiBoy shows as using the camera.** The spec's "the cam only runs while the buffer runs" plus an instant console switch is the right answer. Say so clearly in the Facecam tab.
- **Who holds the camera:** `MFCreateSensorActivityMonitor` reports camera-using processes (as Chromium uses it). Use it to say "Discord is using your camera" instead of a raw HRESULT [D/S].
  https://learn.microsoft.com/en-us/windows/win32/api/mfidl/nf-mfidl-mfcreatesensoractivitymonitor
  Microsoft sample: https://github.com/microsoft/Windows-Camera (SensorActivityMonitorConsoleApp)

---

## 8. Q7: windows-rs specifics

### 8.1 Features

- **Already present and sufficient for the MF path:** `Win32_Media_MediaFoundation`, `implement`, `Win32_System_Com`, `Win32_System_Com_StructuredStorage` (PROPVARIANT), `Win32_System_Variant`, `Win32_Graphics_Direct3D11`, `Win32_Graphics_Dxgi(_Common)`, `Win32_UI_Shell` (ShellExecute for `ms-settings:`).
- **Add for the MF path:**
  - `Win32_Media_KernelStreaming`: `KSCATEGORY_VIDEO_CAMERA`, `IKsControl`, `KSPROPERTY_CAMERACONTROL_*`
  - `Win32_Devices_DeviceAndDriverInstallation`: `CM_Register_Notification`, `CM_NOTIFY_FILTER`
  - Optional `Win32_Media_DirectShow`, if you use `IAMCameraControl`/`IAMVideoProcAmp` on the MF source [U whether needed]
- **Add only for the WinRT path:** see section 5.
- **Present in the crate (0.62.2 docs):** `MFSampleExtension_DeviceTimestamp`, `MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED`, `MFCreateSensorActivityMonitor`, `KSCATEGORY_VIDEO_CAMERA`, `IAdvancedMediaCaptureInitializationSettings`. They are assumed present in 0.58 [U: compile check].
- **Missing:** `MF_DEVSOURCE_ATTRIBUTE_FRAMESERVER_SHARE_MODE`. Define `const` `GUID::from_u128(...)` from `mfidl.h` (SDK 26100+).

### 8.2 Gotchas

1. **`MFEnumDeviceSources`** (signature in windows-rs: `(pattributes, *mut *mut Option<IMFActivate>, *mut u32) -> Result<()>`). Take each element with `ptr::read`, which hands over ownership so the drop releases it, then `CoTaskMemFree` the array [D-crate].
   https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/Media/MediaFoundation/fn.MFEnumDeviceSources.html
2. **Strings:** `GetAllocatedString` → `CoTaskMemFree` [D, and the mediaway-device crate does the same].
3. **`IMFActivate`** caches the source. Set the share mode before `ActivateObject`; whoever activates calls `ShutdownObject`. Easier: use `MFCreateDeviceSource` with the link, then let the source reader own the source.
4. **MF start and stop:**
   - `MFStartup` before use, one `MFShutdown` per `MFStartup`, never from work-queue threads, not from static constructors [D].
     https://learn.microsoft.com/en-us/windows/win32/api/mfapi/nf-mfapi-mfstartup
   - Reuse `crate::mft::startup()` (process-wide `OnceLock`, no shutdown). The mediaway-device crate also never calls `MFShutdown` [S].
5. **COM:** call `CoInitializeEx(COINIT_MULTITHREADED)` on the camera thread, as the audio code does. MF callbacks come on MF work-queue threads (MTA).
6. **`#[implement]` in 0.58:** `impl IMFSourceReaderCallback_Impl for Callback_Impl`, with `psample: Option<&IMFSample>`. In 0.62 the parameter is `Ref<'_, IMFSample>`. The callback must be `Send + Sync`-safe; COM pointers have to be wrapped (`gpu::SendPtr`) to cross a channel.
7. **One `ReadSample` in flight per stream in async mode.** Re-issue it in `OnReadSample`, but not after a stop request. To stop: set a flag → `Flush` → wait for `OnFlush` → drop the reader (which shuts down the source). Don't drop the last reader reference **inside** its own callback [I].
8. **Packed attributes:** `MF_MT_FRAME_SIZE` and `MF_MT_FRAME_RATE` are u64 (hi/lo). The `MFGetAttributeSize` helper is an inline C function and is missing in Rust, so unpack by hand.
9. **`MF_SOURCE_READER_FIRST_VIDEO_STREAM`** is a newtype (`MF_SOURCE_READER_CONSTANTS`); pass `.0 as u32`.
10. **Release samples quickly.** The device source buffers one frame by default [D].
11. **Don't combine** `ENABLE_VIDEO_PROCESSING` with a D3D manager [D].

### 8.3 Code sketch (windows 0.58; signatures to be checked by the compiler)

```rust
// camera/windows.rs — sketch, not compiled
use windows::core::{implement, Interface, Result, GUID, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoTaskMemFree, CoInitializeEx, COINIT_MULTITHREADED};

/// Not in windows-rs 0.58/0.62. Copy from mfidl.h (SDK 10.0.26100+).
const FRAMESERVER_SHARE_MODE: GUID = GUID::from_u128(0x00000000_0000_0000_0000_000000000000 /* TODO */);
const FIRST_VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

pub struct CameraInfo { pub name: String, pub link: String }

pub fn list() -> Result<Vec<CameraInfo>> {
    unsafe {
        crate::mft::startup().map_err(|_| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.unwrap();
        attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)?;
        let mut array: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        MFEnumDeviceSources(&attrs, &mut array, &mut count)?;
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            if let Some(a) = std::ptr::read(array.add(i)) {          // takes ownership → released on drop
                out.push(CameraInfo {
                    name: alloc_string(&a, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME).unwrap_or_default(),
                    link: alloc_string(&a, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK)?,
                });
            }
        }
        CoTaskMemFree(Some(array as *const _));
        Ok(out)
    }
}

unsafe fn alloc_string(a: &IMFActivate, key: &GUID) -> Result<String> {
    let mut p = PWSTR::null();
    let mut len = 0u32;
    a.GetAllocatedString(key, &mut p, &mut len)?;
    let s = p.to_string().unwrap_or_default();
    CoTaskMemFree(Some(p.0 as *const _));
    Ok(s)
}

/// `shared`: only on build >= 26100. Controlling mode blocks Discord.
pub unsafe fn open_source(link: &str, shared: bool) -> Result<IMFMediaSource> {
    let mut attrs = None;
    MFCreateAttributes(&mut attrs, 3)?;
    let attrs = attrs.unwrap();
    attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)?;
    let wide: Vec<u16> = link.encode_utf16().chain(std::iter::once(0)).collect();
    attrs.SetString(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, PCWSTR(wide.as_ptr()))?;
    if shared {
        attrs.SetUINT32(&FRAMESERVER_SHARE_MODE, 1)?;
    }
    MFCreateDeviceSource(&attrs)   // E_ACCESSDENIED here = privacy switch
}

pub enum CamEvent {
    Sample { hr: HRESULT, flags: u32, ts: i64, sample: Option<crate::gpu::SendPtr<IMFSample>>, arrival: i64 },
    Flushed,
}

#[implement(IMFSourceReaderCallback)]
struct Callback { tx: crossbeam_channel::Sender<CamEvent> }

impl IMFSourceReaderCallback_Impl for Callback_Impl {
    fn OnReadSample(&self, hr: HRESULT, _s: u32, flags: u32, ts: i64, sample: Option<&IMFSample>) -> Result<()> {
        let arrival = crate::audio::capture::now_100ns();
        // bounded channel: if the worker is behind, drop rather than block the MF work queue
        let _ = self.tx.try_send(CamEvent::Sample {
            hr, flags, ts, arrival, sample: sample.cloned().map(crate::gpu::SendPtr),
        });
        Ok(())
    }
    fn OnFlush(&self, _s: u32) -> Result<()> { let _ = self.tx.try_send(CamEvent::Flushed); Ok(()) }
    fn OnEvent(&self, _s: u32, _e: Option<&IMFMediaEvent>) -> Result<()> { Ok(()) }
}

pub unsafe fn make_reader(source: &IMFMediaSource, cb: &IMFSourceReaderCallback,
                          d3d: Option<&IMFDXGIDeviceManager>) -> Result<IMFSourceReader> {
    let mut attrs = None;
    MFCreateAttributes(&mut attrs, 3)?;
    let attrs = attrs.unwrap();
    attrs.SetUnknown(&MF_SOURCE_READER_ASYNC_CALLBACK, cb)?;
    attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;   // allow a HW MJPEG MFT if one exists
    if let Some(m) = d3d {
        attrs.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, m)?;         // never together with ENABLE_VIDEO_PROCESSING
    }
    MFCreateSourceReaderFromMediaSource(source, &attrs)
}

pub struct Native { pub ty: IMFMediaType, pub sub: GUID, pub w: u32, pub h: u32, pub fps_num: u32, pub fps_den: u32 }

pub unsafe fn native_types(reader: &IMFSourceReader) -> Vec<Native> {
    let mut v = Vec::new();
    for i in 0.. {
        let Ok(ty) = reader.GetNativeMediaType(FIRST_VIDEO, i) else { break };
        let (Ok(sub), Ok(size), Ok(rate)) =
            (ty.GetGUID(&MF_MT_SUBTYPE), ty.GetUINT64(&MF_MT_FRAME_SIZE), ty.GetUINT64(&MF_MT_FRAME_RATE)) else { continue };
        v.push(Native { ty, sub, w: (size >> 32) as u32, h: size as u32,
                        fps_num: (rate >> 32) as u32, fps_den: (rate as u32).max(1) });
    }
    v
}

/// Passthrough = set the native type itself. Decoded = ask for NV12; the reader inserts the MJPEG decoder.
pub unsafe fn configure(reader: &IMFSourceReader, native: &IMFMediaType, decode_to_nv12: bool) -> Result<()> {
    reader.SetCurrentMediaType(FIRST_VIDEO, None, native)?;      // fixes size/fps on the device (controlling mode)
    if decode_to_nv12 {
        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        reader.SetCurrentMediaType(FIRST_VIDEO, None, &out)?;
    }
    reader.ReadSample(FIRST_VIDEO, 0, None, None, None, None)     // async: first request
}

/// QPC (100 ns) of a camera sample, with the same plausibility idea as audio::capture::usable_stamp.
pub unsafe fn sample_qpc(s: &IMFSample, ts: i64, arrival: i64, learned: &mut Option<i64>) -> (i64, bool /*precise*/) {
    const MAX: i64 = 2 * 10_000_000;
    let ok = |q: i64| q != 0 && (q - arrival).abs() < MAX && q <= arrival + 10_000; // not in the future
    if let Ok(q) = s.GetUINT64(&MFSampleExtension_DeviceTimestamp) {
        let q = q as i64;
        if ok(q) { *learned = Some(q - ts); return (q, true); }
    }
    if ok(ts) { return (ts, true); }                     // sample time already QPC
    if let Some(o) = *learned { if ok(ts + o) { return (ts + o, true); } }
    (arrival, false)                                     // coarse: arrival time; raise a "fallback clock" flag
}
```

Getting the pixels (worker thread):
- `sample.GetBufferByIndex(0)`
- if `buffer.cast::<IMFDXGIBuffer>()` succeeds, copy the subresource
- otherwise `cast::<IMF2DBuffer>()`, then `Lock2D`, copy into a staging NV12, `CopyResource` into the slot, `Unlock2D`
- For passthrough: `ConvertToContiguousBuffer()` → `Lock()` → `Arc<[u8]>` → `Unlock()`

Further reference code:
- Rust: `mediaway-device` 0.1.3, `src/windows_camera/capture.rs`. It uses MFEnumDeviceSources, a sync source reader, GetNativeMediaType, Lock2D, `Shutdown()`/`ShutdownObject()`, and synthetic timestamps (not usable for sync) [S].
  https://docs.rs/crate/mediaway-device/0.1.3/source/src/windows_camera/capture.rs
- C: Martins Mozeiko, sync MJPG capture [S]: https://gist.github.com/mmozeiko/a5adab1ad11ea6d0643ceb67bb8e3e19
- C++: Microsoft docs async sample (section 3.1); Chromium `video_capture_device_mf_win.cc` (capture engine, D3D11 path, timestamps, errors).

---

## 9. Recommended camera-source design for ClippiBoy

### 9.1 API

- **`IMFSourceReader`, async**, behind a platform trait (`camera::Source`) so V4L2 can follow.
- **Sharing first** [I]:
  - If `RtlGetVersion` build ≥ 26100: open with `FRAMESERVER_SHARE_MODE = 1`. This never blocks Discord.
  - On older builds: controlling mode, plus a UI note ("While ClippiBoy uses the camera, other apps may not be able to"), plus the hint about Windows' "multi-app camera" toggle.
  - A WinRT `MediaCapture SharedReadOnly` backend only if test E2 on Windows 10 demands it. The virtual camera (spec block 5) is the long-term answer there.
- **Getting 720p30 while sharing** (experiment E3, [U]): when the camera is idle and the current media type is worse than the target, open briefly in **controlling** mode, set 1280×720 MJPG at 30 fps, start, open the **sharing** instance, then shut down the controlling one. The open question is whether the Frame Server keeps the type while a sharing client streams. If not, stay in controlling mode until `IMFSensorActivityMonitor` sees another process, then switch to sharing. That can cost Discord one failed start.
- Device list from `MFEnumDeviceSources`. Id = symbolic link, display = friendly name. Hotplug through `CM_Register_Notification(KSCATEGORY_VIDEO_CAMERA)`.

### 9.2 Format

- Controlling mode: pick a native type with the scoring in 3.5. On the C920/C960 at 720p30 that will be **MJPG 1280×720 @ 30**. Request NV12 from the reader, which loads the MJPEG decoder. Set `MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS` and the D3D manager of `gpu.device` so a hardware MJPEG MFT and DXGI output can be used where they exist. Log the inserted transform.
- Sharing mode: take whatever comes (MJPG/YUY2/NV12, any size), react to `*MEDIATYPECHANGED`, and normalise on the GPU.
- **GPU stage:** NV12 (or YUY2) → `ID3D11VideoProcessor` (as in `convert.rs`) → **fixed 1280×720 NV12** (setting: 720p default, 1080p option). Aspect ratio is kept with letterboxing, so a format change mid-buffer never needs a new encoder.

### 9.3 Threading

- MF work-queue threads run `OnReadSample`, which only timestamps (`now_100ns()` at arrival), pushes into a bounded channel (capacity 2–3, drop-oldest), and re-issues `ReadSample`.
- The **cam worker** thread (MTA, `CoInitializeEx`) does upload, scaling, the CFR grid (9.5), the encoder submit (9.7) and the ring push.
- The encoder's output thread is the existing `mft::Transform` machinery, just a second instance.
- Stop: flag → `Flush` → `OnFlush` → drop the reader, which shuts down the source.

### 9.4 Mapping timestamps onto QPC / buffer time

1. For each sample, `cam_qpc = sample_qpc(...)` (8.3): `DeviceTimestamp` → plausible sample time → learned offset → arrival time (sets `cam.fallback_clock`, shown in the status, as with audio).
2. Enforce monotonicity: if `cam_qpc ≤ last`, use `last + 1` (as Chromium does).
3. **Store raw QPC, apply no delay while capturing.** Per clip, store `cam_origin_100ns` (the QPC of cam grid slot 0, the same idea as `Shared::base_100ns`) and `cam_offset_ms` (default from the setting, editable in the editor).
4. On save:
   - cam start offset relative to the clip = `(cam_origin + first_cam_frame_index·period + cam_offset) − ClipSnapshot.start_100ns`
   - ffmpeg gets the cam ES with `-f h264 -r 30` plus `-itsoffset <offset>` (or `setpts` / `tpad` in the overlay filter graph).
   - Picture, cam and sound then share the same QPC axis.

### 9.5 VFR → CFR: a grid driven by timestamps

The existing `pace()` takes "the latest frame at tick time". That adds up to one frame of scheduling jitter, and camera arrival jitter on top. The cam should instead be snapped by its own QPC [I]:

```
period = 10_000_000 / 30
origin = cam_qpc(first frame)                 // slot 0
slot(f) = round((cam_qpc(f) - origin) / period)
- several frames in one slot → keep the latest
- emit slot k once a frame for slot > k arrived, or once now > origin + (k+1)·period + 100 ms (stall timer)
- empty slot → repeat the previous frame (duplicate counter, like Latest::duplicated)
- cam_qpc jumps backwards or more than 2 s forward → new origin and a "discontinuity" counter (log it)
```

- 15 fps in low light becomes each frame twice; 29.97 fps becomes one repeat about every 33 s. The stream stays exactly 30 fps, so `muxer::timing`/`-r` keeps working.
- The 100 ms emission delay is irrelevant for a replay buffer.

### 9.6 Dropout handling

- **Short stall (< 2 s):** repeat the last frame (slots keep being emitted by the stall timer).
- **Longer, or the device is lost:** emit **filler frames**: the last frame darkened or plain black, which a hardware encoder compresses to a few bytes. The cam track then stays continuous and the same length as the game track.
  - Meanwhile reconnect with backoff and CM arrival.
  - Mark the lost QPC ranges in the clip metadata so the editor can hide the cam there automatically.
- **Format change (sharing):** reconfigure only the upload/scaler. The encoder keeps its fixed output size.
- **`E_ACCESSDENIED` mid-session:** stop, keep filler frames until the user turns the cam off or fixes privacy, and offer `ms-settings:privacy-webcam`.
- **Encoder failure:** the fallback chain in 9.8.

### 9.7 Encoder

- **Default:** a second hardware H.264 MFT through `mft::build` at 1280×720, 30 fps, no B-frames, GOP 1 s (more cut points for the editor), about **3 Mbit/s** VBR (2.5–4). That is about 45 MB per 120 s buffer.
  - It uses its own `ReplayBuffer` ring (`capacity_bytes` from bitrate × `buffer.seconds`) and its own sequence header.
  - On save the ring is cut from the cam keyframe ≤ the clip start. Compositing re-encodes anyway, so exact keyframe alignment with the game isn't needed.
- **GPU load:** about 1.5–6 % of one NVENC engine. Session count is fine with 8 (driver ≥ 551.76). AMD and Intel have no cap.

### 9.8 Fallback chain (decided at start, logged, shown in the Facecam tab)

1. Hardware H.264 (same vendor as the game encoder).
2. **MJPEG passthrough**, if the native type is MJPG at the target size: ring of `Arc<[u8]>` with a memory cap of e.g. 400 MB. The buffer shortens for the cam only when the cap is hit (show this). Cut per frame; on save ffmpeg decodes it and stores the clean cam as H.264.
3. Microsoft H.264 software MFT fed with system-memory NV12 (no D3D manager; YUY2 is accepted directly). Low complexity, slice mode.
4. Otherwise: no cam track, with a clear message.

### 9.9 Delay slider

- Range −300 … +300 ms in 10 ms steps.
- Default **−40 ms** with a precise clock, **−80 ms** on the arrival-time fallback [I/U].
- Applied at save and edit time, never baked into the stream.
- Confirm or adjust the default with the clap test (E6).
- "Video earlier/later" wording: OBS users know this as "Render Delay" (camera) versus "Sync Offset" (audio).

### 9.10 Privacy and UX

- `E_ACCESSDENIED` → message and a button to `ms-settings:privacy-webcam`.
- "In use" (`MF_E_HW_MFT_FAILED_START_STREAMING` or a sharing violation) → name the process through `IMFSensorActivityMonitor` and retry in sharing mode.
- Code-sign ClippiBoy for the coming per-app desktop permissions.
- Explain in the UI that the LED stays on while the buffer runs.
- Don't change camera controls (exposure priority and so on) unless the user asks: in sharing mode it would change Discord's picture too.

---

## 10. Experiments for spec step 1, "Test: share the camera" (cheap, decisive)

| # | Test | Decides |
|---|---|---|
| E1 | For each sample, log `GetSampleTime`, `DeviceTimestamp`, `DeviceReferenceSystemTime` and `now_100ns()`, for MJPG passthrough and for decoded NV12, on C920 and C960. | Whether sample time is QPC or rebased; whether `DeviceTimestamp` survives the decoder |
| E2 | Win11 26200: ClippiBoy MF **sharing** first, then Discord. Discord first, then ClippiBoy sharing. Same with ClippiBoy **controlling**. Repeat on Windows 10 with MediaCapture `SharedReadOnly`. Also with the "multi-app camera" toggle on. | Whether the virtual camera has to move forward; whether the WinRT backend is needed |
| E3 | Sharing mode with no other app: which media type arrives? The controlling → sharing handover trick from 9.1: does 720p30 survive? | How ClippiBoy gets a good format without blocking |
| E4 | Average and peak MJPG frame size at 720p30 and 1080p30 (bright and dim room). | MJPEG passthrough memory budget |
| E5 | CPU of `mfmjpegdec` at 720p30/1080p30. With `ENABLE_HARDWARE_TRANSFORMS` + D3D manager: which decoder is inserted (`IMFSourceReaderEx::GetTransformForStream`) and whether `IMFDXGIBuffer` arrives. | Decode cost; whether frames come as textures |
| E6 | Clap or LED test: microphone (WASAPI QPC) against camera (`DeviceTimestamp`). | Default delay |
| E7 | Dim room: frame rate with `AUTO_EXPOSURE_PRIORITY` 0 and 1 (read only). | Whether the grid/duplicate path is exercised as expected |
| E8 | Unplug and replug, sleep/resume, privacy toggle off mid-stream, Discord taking control mid-stream: which HRESULTs and flags arrive. | Recovery code |
| E9 | NVENC: game encoder + cam encoder + ffmpeg export at the same time, with NVIDIA App/OBS running. | Session headroom, fallback chain |

---

## 11. Still unconfirmed (summary)

- GUID value of `MF_DEVSOURCE_ATTRIBUTE_FRAMESERVER_SHARE_MODE`. It is absent from windows-rs 0.58 and 0.62.2.
- Whether `GetSampleTime` of webcam samples is QPC or start-relative; whether `DeviceTimestamp` passes through the MJPEG decoder (E1).
- Which media type a sharing instance gets without a controller; the handover trick (E3).
- Which API and mode Discord uses for the camera.
- Exact MF HRESULT for a sharing violation.
- Speed of `mfmjpegdec`; whether hardware MJPEG MFTs / DXGI output exist on NVIDIA/AMD/Intel drivers (E5).
- Real MJPEG frame sizes (E4) and real camera latency (E6).
- C960 native type list. Retailer pages only say "YUY2/MJPEG, 1080p30"; some list a 4K variant.

---

## 12. Sources

Microsoft (primary):
- Enumerating video capture devices: https://learn.microsoft.com/en-us/windows/win32/medfound/enumerating-video-capture-devices
- Symbolic link: https://learn.microsoft.com/en-us/windows/win32/medfound/mf-devsource-attribute-source-type-vidcap-symbolic-link
- VIDCAP_CATEGORY: https://learn.microsoft.com/en-us/windows/win32/medfound/mf-devsource-attribute-source-type-vidcap-category
- VIDCAP_MAX_BUFFERS: https://learn.microsoft.com/en-us/windows/win32/medfound/mf-devsource-attribute-source-type-vidcap-max-buffers
- FRAMESERVER_SHARE_MODE: https://learn.microsoft.com/en-us/windows/win32/medfound/mf-devsource-attribute-frameserver-share-mode
- MF_DEVICESTREAM_FRAMESERVER_SHARED: https://learn.microsoft.com/en-us/windows/win32/medfound/mf-devicestream-frameserver-shared
- Handling video device loss: https://learn.microsoft.com/en-us/windows/win32/medfound/handling-video-device-loss
- CM_Register_Notification: https://learn.microsoft.com/en-us/windows/win32/api/cfgmgr32/nf-cfgmgr32-cm_register_notification
- MEError: https://learn.microsoft.com/en-us/windows/win32/medfound/meerror
- Source reader async: https://learn.microsoft.com/en-us/windows/win32/medfound/using-the-source-reader-in-asynchronous-mode
- MF_SOURCE_READER_FLAG: https://learn.microsoft.com/en-us/windows/win32/api/mfreadwrite/ne-mfreadwrite-mf_source_reader_flag
- ENABLE_VIDEO_PROCESSING: https://learn.microsoft.com/en-us/windows/win32/medfound/mf-source-reader-enable-video-processing
- ENABLE_ADVANCED_VIDEO_PROCESSING: https://learn.microsoft.com/en-us/windows/win32/medfound/mf-source-reader-enable-advanced-video-processing
- D3D_MANAGER: https://learn.microsoft.com/en-us/windows/win32/medfound/mf-source-reader-d3d-manager
- ENABLE_HARDWARE_TRANSFORMS: https://learn.microsoft.com/en-us/windows/win32/medfound/mf-readwrite-enable-hardware-transforms
- SetCurrentMediaType: https://learn.microsoft.com/en-us/windows/win32/api/mfreadwrite/nf-mfreadwrite-imfsourcereader-setcurrentmediatype
- Set capture frame rate: https://learn.microsoft.com/en-us/windows/win32/medfound/how-to-set-the-video-capture-frame-rate
- MFCreateSourceReaderFromMediaSource: https://learn.microsoft.com/en-us/windows/win32/api/mfreadwrite/nf-mfreadwrite-mfcreatesourcereaderfrommediasource
- IMFActivate::ShutdownObject: https://learn.microsoft.com/en-us/windows/win32/api/mfobjects/nf-mfobjects-imfactivate-shutdownobject
- IMFMediaSourceEx::SetD3DManager: https://learn.microsoft.com/en-us/windows/win32/api/mfidl/nf-mfidl-imfmediasourceex-setd3dmanager
- MFStartup: https://learn.microsoft.com/en-us/windows/win32/api/mfapi/nf-mfapi-mfstartup
- MFGetSystemTime: https://learn.microsoft.com/en-us/windows/win32/api/mfidl/nf-mfidl-mfgetsystemtime
- DeviceTimestamp: https://learn.microsoft.com/en-us/windows/win32/medfound/mfsampleextension-devicetimestamp
- DeviceReferenceSystemTime: https://learn.microsoft.com/en-us/windows/win32/medfound/mfsampleextension-devicereferencesystemtime
- MFT_HW_TIMESTAMP_WITH_QPC: https://learn.microsoft.com/en-us/windows/win32/medfound/mft-hw-timestamp-with-qpc-attribute
- Discontinuity: https://learn.microsoft.com/en-us/windows/win32/medfound/mfsampleextension-discontinuity-attribute
- Frame Server custom media source (QPC rule): https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/frame-server-custom-media-source
- UVC implementation: https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/usb-video-class-implementation
- UVC 1.5 extensions: https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/uvc-extensions-1-5
- AUTO_EXPOSURE_PRIORITY: https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/ksproperty-cameracontrol-auto-exposure-priority
- HLK Device.Streaming (latency, fps, jitter): https://learn.microsoft.com/en-us/windows-hardware/test/hlk/testref/troubleshooting-devicestreaming-testing
- H.264 encoder MFT: https://learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-encoder
- MFCreateSensorActivityMonitor: https://learn.microsoft.com/en-us/windows/win32/api/mfidl/nf-mfidl-mfcreatesensoractivitymonitor
- Camera privacy: https://learn.microsoft.com/en-us/windows/apps/develop/camera/camera-privacy-setting
- WGC SystemRelativeTime: https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframe.systemrelativetime
- WASAPI GetBuffer: https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiocaptureclient-getbuffer
- MediaFrameReader guide: https://learn.microsoft.com/en-us/windows/apps/develop/camera/process-media-frames-with-mediaframereader
- MediaFrameReference.SystemRelativeTime: https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.frames.mediaframereference.systemrelativetime
- SharingMode: https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.mediacaptureinitializationsettings.sharingmode
- MediaCaptureSharingMode: https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.mediacapturesharingmode
- FormatChanged: https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.frames.mediaframesource.formatchanged
- MediaFrameReaderStartStatus: https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.frames.mediaframereaderstartstatus
- IAdvancedMediaCaptureInitializationSettings: https://learn.microsoft.com/en-us/windows/win32/api/mfmediacapture/nn-mfmediacapture-iadvancedmediacaptureinitializationsettings
- Windows Insider 26120.2702 (multi-app camera): https://blogs.windows.com/windows-insider/2024/12/13/announcing-windows-11-insider-preview-build-26120-2702-dev-channel/
- Windows-Camera samples: https://github.com/microsoft/Windows-Camera
- Microsoft forum archive (controlling/shared model): https://learn.microsoft.com/en-us/archive/msdn-technet-forums/12de4147-9fa0-4031-818f-2cec9a1d080b

Vendors, projects:
- NVENC application note 13.0: https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/nvenc-application-note/index.html
- NVENC note 12.1: https://docs.nvidia.com/video-technologies/video-codec-sdk/12.1/pdf/NVENC_Application_Note.pdf
- NVIDIA forum (551.76): https://forums.developer.nvidia.com/t/how-to-increase-number-of-nvenc-concurrent-sessions/169367
- Chromium MF capture: https://raw.githubusercontent.com/chromium/chromium/main/media/capture/video/win/video_capture_device_mf_win.cc
- OBS win-dshow: https://raw.githubusercontent.com/obsproject/obs-studio/master/plugins/win-dshow/win-dshow.cpp
- ffmpeg bitstream filters (mjpeg2jpeg/DHT): https://ffmpeg.org/ffmpeg-bitstream-filters.html
- windows-docs-rs (MFEnumDeviceSources, IMFSourceReaderCallback_Impl, KSCATEGORY_VIDEO_CAMERA, CM_Register_Notification, MFCreateSensorActivityMonitor): https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/Media/MediaFoundation/index.html
- mediaway-device (Rust MF camera): https://docs.rs/crate/mediaway-device/0.1.3/source/src/windows_camera/capture.rs
- mmozeiko MF webcam gist: https://gist.github.com/mmozeiko/a5adab1ad11ea6d0643ceb67bb8e3e19

Secondary (forums, press):
- windows-hexerror (HRESULT texts): https://windows-hexerror.linestarve.com/0xC00D3EA2, https://windows-hexerror.linestarve.com/0xC00D3EA3, https://windows-hexerror.linestarve.com/0xC00D3704
- Multi-app camera: https://pureinfotech.com/allow-multiple-apps-use-camera-windows-11/, https://windowslatest.com/2024/08/21/windows-11-24h2-will-finally-allow-multiple-apps-to-use-your-camera-at-the-same-time
- Per-app desktop privacy (Insider 26340): https://www.bleepingcomputer.com/news/microsoft/microsoft-tests-new-privacy-controls-for-windows-11-desktop-apps/amp/, https://windowslatest.com/2026/08/19/windows-11-is-getting-a-major-privacy-upgrade-blocks-apps-spying-on-your-camera-and-mic-with-new-controls
- Privacy indicators: https://geekrewind.com/how-to-see-which-apps-have-accessed-your-camera-in-windows-11/
- USB isoc 3072 B/µframe: https://patchwork.kernel.org/comment/2109752
- C920 720p@10 fps YUY2: https://obsproject.com/forum/threads/logitech-c920-blurry.84690
- Webcam latency: https://makehardware.com/2016/03/29/finding-a-low-latency-webcam/, https://forums.developer.nvidia.com/t/usb-camera-high-latency-on-tx1/55532
- OBS sync tools: https://obsproject.com/forum/threads/cant-find-audio-delay-to-sync-with-camera.28562
- x264 720p30 CPU anecdote: https://obsproject.com/forum/threads/cpu-usage-is-insanely-higher-than-before.62060/post-269065
- mfmjpegdec.dll: https://www.nirsoft.net/dll_information/windows8/mfmjpegdec_dll.html
- Intel HW MJPEG MFT thread: https://community.intel.com/t5/Media-Intel-Video-Processing/Intel-Hardware-M-JPEG-Decoder-MFT/td-p/1004407
- AMD/Intel no session cap: https://minipclab.com/tools/plex-transcode-calculator, https://support.medialooks.com/hc/en-us/articles/360000210192-H-264-encoding-options
- OSR (Frame Server routes MF and DirectShow): https://community.osr.com/t/windows-10-anniversary-update-and-testcap-avstream-drivers/53081
- NVENC 3→5: https://guru3d.com/story/nvidia-removes-encoding-limitations-on-consumer-gpus-allowing-up-to-5-simultaneous-streams
