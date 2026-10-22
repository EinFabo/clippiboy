# Webcam sharing on Windows (facecam stage 1)

> Research notes made for the facecam design on 2026-10-22: web research and read-only code reading, nothing was run. Every claim carries a source and a confidence tag. Where this dossier and the spec (`docs/superpowers/specs/2026-10-20-facecam-design.md`) disagree, the spec wins: it weighs all four dossiers and the code review together.

Research dossier for the ClippiBoy facecam plan (`docs/superpowers/specs/2026-10-20-facecam-design.md`).
Date: 2026-10-22. Method: web research only; nothing was run on any machine.

**Evidence tags used below**

- **[P]**: primary source confirms it (learn.microsoft.com, support.microsoft.com, the Microsoft Windows-Camera repo or a Microsoft engineer in it, Chromium/WebRTC source, Windows Insider/Release notes).
- **[S]**: secondary source (press, forum moderators, expert blogs, community reports).
- **[I]**: my inference from the sources; not stated anywhere.
- **[U]**: unconfirmed; the stage-1 test has to settle it.

---

## TL;DR

1. **Windows does not share a camera by default.** The Frame Server (Windows 10 1607+) *can* share, but only with a client that **opts in to shared mode**. Two normal clients (each wanting control: Camera app, Chrome/Edge, OBS, almost certainly Discord) still lock each other out. A Microsoft camera engineer says browsers ("web clients") and OBS ask for exclusive control [P].
2. **If ClippiBoy opens the camera the plain way (`MFCreateDeviceSource` + `IMFSourceReader`, or MediaCapture `ExclusiveControl`), Discord will fail.** Chromium's MF code treats the result, `MF_E_HW_MFT_FAILED_START_STREAMING` (0xC00D3704), as "Camera is in use by another process" [P].
3. **ClippiBoy should open the camera read-only shared**: WinRT `MediaCapture` with `SharingMode = SharedReadOnly` plus `MediaFrameReader`, or the Win32 version (`MFCreateSensorGroup`, then `SetSensorDeviceMode(MFSensorDeviceMode_Shared)`, then a source reader). The docs say a shared client keeps getting frames while another app holds control. It can't change format or controls, so it gets whatever format the controlling app picked [P].
4. **The big unknown is DirectShow.** A Microsoft engineer: apps that use the camera through DirectShow "may block you from reading from them even in shared mode", and "nothing can be done" about it [P]. OBS's camera source is DirectShow [P]. Discord's native engine is built on the WebRTC native library [P], whose Windows camera module is DirectShow-only [P]. So **Discord probably captures through DirectShow [I]**, and that is exactly the case where sharing may break. The stage-1 test decides this.
5. **Windows 11 "Multi-App Camera"** ("Allow multiple apps to use camera at the same time", internally *AutoShare*) shipped for 24H2/25H2 in **KB5089573, OS builds 26100.8524 / 26200.8524 (26 May 2026)**, as a **gradual rollout** and **off by default** [P][S]. It treats every app as if it used sharing mode. It **forces a single media type** (chosen in Settings) and controls only work from the Settings page [P]. It doesn't exist on Windows 10 or older Windows 11 releases [P]. Treat it as a useful user-side switch, not as the solution.
6. **Both test cameras need MJPEG for 720p30 or 1080p30 over USB 2.0.** Uncompressed YUY2 is capped at about 10 fps at 720p and 5 fps at 1080p [P arithmetic + S]. MJPEG is also where shared mode has broken on some hardware [S]. So test 720p MJPEG explicitly.
7. **Low light makes the frame rate variable** (auto-exposure priority / Logitech "RightLight") [P][S], and a *shared* client **cannot** switch that off [P]. ClippiBoy has to timestamp every frame (QPC) and must not assume 30 fps.
8. **Decision rule.** Look at whether Discord and a shared-mode reader run side by side on Windows 11, in both open orders, with the multi-app toggle *off*. Then move the virtual camera forward or leave it where it is (table in section 8.3). The cheapest first look needs no code at all: Windows' own **Settings camera preview is a shared-mode client** [P]. Run it next to Discord, OBS and Chrome.

---

## 1. Frame Server: since when, which APIs go through it, who is exclusive

### Since when

- The **Windows Camera Frame Server** service arrived with **Windows 10 version 1607 (Anniversary Update, build 14393)** [P: [Camera Profile V2 overview](https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/camera-profile-v2-architectural-overview)].
- Microsoft's stated reason was concurrent camera access. Mike M. (Windows Camera team) wrote in 2016 that with the Anniversary Update "it is now possible for multiple applications to access the camera" and that "it was important for us to enable concurrent camera access" [S, quoted at [OSR thread](https://community.osr.com/t/windows-10-anniversary-update-and-testcap-avstream-drivers/53081); press: [PCWorld](https://www.pcworld.com/article/416088/the-windows-10-anniversary-update-is-breaking-webcams.html), [SiliconANGLE](https://siliconangle.com/2016/08/19/windows-10-update-blamed-for-breaking-webcams/)]. The same change broke MJPEG/H.264 for apps at first (the C920 was a well-known victim). Microsoft promised an MJPEG fix "in September" 2016 and said H.264 would take longer [S]. The workaround at the time, registry `EnableFrameServerMode=0` under `HKLM\SOFTWARE\(WOW6432Node\)Microsoft\Windows Media Foundation\Platform`, is community lore, not documented [S: [mgraves.org](https://www.mgraves.org/2016/08/can-you-see-me-now-microsoft-suffering-new-windows-10-webcam-strategy/amp/)].
- The **opt-in sharing APIs are just as old (1607)**:
  - WinRT `MediaCaptureSharingMode` {ExclusiveControl, SharedReadOnly}: introduced in 10.0.14393 [P: [docs](https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.mediacapturesharingmode)].
  - Win32 `MFCreateSensorGroup`: Windows 10 1607 [P: [docs](https://learn.microsoft.com/en-us/windows/win32/api/mfidl/nf-mfidl-mfcreatesensorgroup)]. `IMFSensorDevice::SetSensorDeviceMode`: Windows 10 1607, desktop apps [P: [docs](https://learn.microsoft.com/en-us/windows/win32/api/mfidl/nf-mfidl-imfsensordevice-setsensordevicemode)]. `MFSensorDeviceMode_Controller` (0, "settings can be modified") vs `MFSensorDeviceMode_Shared` ("settings can't be modified") [P: [docs](https://learn.microsoft.com/en-us/windows/win32/api/mfidl/ne-mfidl-mfsensordevicemode)].
  - `MediaCapture.CaptureDeviceExclusiveControlStatusChanged`: 1703 (15063) [P: [docs](https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.mediacapture.capturedeviceexclusivecontrolstatuschanged)].
- **In practice two ordinary apps still couldn't share.** A 2020 Microsoft Q&A user on 20H2 found that Zoom and Chrome could not use the laptop camera at once, and Microsoft staff found no official article about the feature [S: [Q&A 171402](https://learn.microsoft.com/en-ca/answers/questions/171402/sharing-webcam-within-multiple-applications)]. Windows Latest (2024): "When an app like the default Windows camera is using your camera, no other app can use it at the same time", blocked via "CameraReservedByAnotherApp" [S: [Windows Latest](https://www.windowslatest.com/2024/08/21/windows-11-24h2-will-finally-allow-multiple-apps-to-use-your-camera-at-the-same-time/)].

### Which client is what

| API / client | Through Frame Server? | Mode it gets | Source |
|---|---|---|---|
| WinRT `MediaCapture` / `MediaFrameReader` | yes | whatever it asks for: `ExclusiveControl` or `SharedReadOnly` | [P] docs above |
| Win32 MF via **sensor group** + `SetSensorDeviceMode(Shared)` → `CreateMediaSource` → Source Reader / Capture Engine | yes | shared | [P] [Windows-Camera wiki](https://github.com/microsoft/Windows-Camera/wiki): "the API to enabled shared mode is only available through sensor groups" |
| Win32 MF plain `MFCreateDeviceSource` / `IMFActivate::ActivateObject` → Source Reader / Capture Engine | yes | **controller** (default value 0 of `MFSensorDeviceMode`; there is no sharing flag on this path) | [I], strongly supported: Chromium does exactly this and is "exclusive" per Microsoft (below) |
| DirectShow (`ksproxy`, OBS "Video Capture Device", WebRTC native `video_capture_ds`) | yes, routed through the service (Roman Ryltsov 2016: both DirectShow and MF sessions go through it; MJPEG→NV12 decoding only for MF clients). On Windows 11 even MF virtual cameras show up in DirectShow apps, which fits a DirectShow→Frame Server bridge | **cannot request shared mode**; behaves like an exclusive client and "may block" shared readers | [S] [OSR](https://community.osr.com/t/windows-10-anniversary-update-and-testcap-avstream-drivers/53081); [S] [alax.info 2022](https://alax.info/blog/2245); [P] [wiki](https://github.com/microsoft/Windows-Camera/wiki): sharing "could also be used in Capture Engine (Win32) apps, but not in DirectShow apps"; [P] [Windows-Camera #31](https://github.com/microsoft/Windows-Camera/issues/31) |

Microsoft engineer Louis-Philippe Bourret (Windows-Camera repo), Nov 2024: "web clients and OBS will also attempt to acquire the camera in 'Exclusive control'", and the Camera app also runs in that mode, "it prevents others from doing so" [P: [Windows-Camera #72](https://github.com/microsoft/Windows-Camera/issues/72)].

**Does a DirectShow client conflict with an MF client?** Yes. Any two controller/exclusive clients conflict, whatever their API. And a DirectShow controller can even starve an MF/WinRT *shared* reader: "may block you from reading from them even in shared mode if the other app is using them via DShow … There is however nothing that can be done in that situation to read frames concurrently" [P: [#31](https://github.com/microsoft/Windows-Camera/issues/31)]. What happens the other way round (a shared MF reader starts first, a DirectShow app starts second) is **[U]**.

**Which cameras can share at all:** sensor groups, and with them shared mode, only work for cameras registered under `KSCATEGORY_VIDEO_CAMERA` or `KSCATEGORY_SENSOR_CAMERA`, "NOT … legacy cameras that only register under KSCATEGORY_VIDEO|KSCATEGORY_CAPTURE". For a typical USB camera, its single record pin is what sharing apps get [P: [wiki](https://github.com/microsoft/Windows-Camera/wiki)]. The inbox UVC driver puts the C920 and C960 in the modern category [I]. Check in Device Manager during the test.

---

## 2. WinRT `MediaCapture` SharingMode semantics

**Documented behaviour [P]:**

- `ExclusiveControl`: "should have exclusive control of one or more of the frame sources". `SharedReadOnly`: "can acquire frames … but can't adjust the configuration of the frame sources" [[enum](https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.mediacapturesharingmode)].
- "If you set this to **SharedReadOnly**, you can receive frames from the frame sources even if they are in use by another app, but you can't change the settings for the devices." With ExclusiveControl, "if another app already has exclusive control, your app will fail when it tries to initialize" [[Process media frames with MediaFrameReader](https://learn.microsoft.com/en-us/windows/apps/develop/camera/process-media-frames-with-mediaframereader)].
- An app without exclusive control "can still capture media, but it can't change the capture settings". The event `CaptureDeviceExclusiveControlStatusChanged` reports `ExclusiveControlAvailable` (you may now re-init as exclusive) and `SharedReadOnlyAvailable` ("another app has acquired exclusive control … another app may adjust the capture device settings") [[event docs](https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.mediacapture.capturedeviceexclusivecontrolstatuschanged)].
- A shared client cannot "change the MediaType nor set controls" [P: Bourret, [#31](https://github.com/microsoft/Windows-Camera/issues/31)].
- `MediaFrameSource.FormatChanged` fires when the source's current format changes; read `CurrentFormat` [[docs](https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.frames.mediaframesource.formatchanged)].
- MediaCapture works from unpackaged desktop apps, not only UWP [P: [wiki](https://github.com/microsoft/Windows-Camera/wiki)].

**Answers to the specific questions:**

- *Can a SharedReadOnly client receive frames while another app controls the camera?* Yes, documented. It works when the controller uses MF or WinRT. It may not when the controller uses DirectShow [P, #31]. Driver-dependent failures exist too: on one laptop, Teams chose MJPEG and the shared reader got only null frames "no matter who starts to open camera first". The vendor blamed older notebook GPU decoder/camera firmware that is "not suitable for reading MJPG streams in shared mode" [S: [Q&A 5608812](https://learn.microsoft.com/en-us/answers/questions/5608812/how-to-avoid-ms-teams-to-select-mjpg-as-camera-dat)]. In 2021 another developer reported that SharedReadOnly "can almost meet this request" while Skype, Camera or Zoom held the webcam [S: [Q&A 218589](https://learn.microsoft.com/en-us/answers/questions/218589/how-to-get-the-frame-from-webcam-that-other-applic)].
- *What format does the second client get?* Whatever the controller set. A shared client can't change it [P]. It can ask its own `MediaFrameReader` for a fixed subtype and size: `CreateFrameReaderAsync(source, subtype, BitmapSize)` returns *copies* of the frames [P: [docs](https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.mediacapture.createframereaderasync)]. So ClippiBoy can always get e.g. NV12 1280×720 and pay for the conversion. Which conversions really happen in-process versus in the Frame Server is **[U]**. Microsoft notes the Frame Server exposes a decoded **NV12** version of MJPEG media types [P: [VirtualCamera README](https://github.com/microsoft/Windows-Camera/blob/master/Samples/VirtualCamera/README.md)].
- *Can a shared client be first?* Yes. The Windows **Settings camera page "is a shared-mode client for a camera"** [P: Bourret, Feb 2026, [#72](https://github.com/microsoft/Windows-Camera/issues/72)], and it shows a preview with no other app open. What format the camera streams then is **[U]** (probably the source's current/default media type). Whether a controller that arrives *later* can change the format under a running shared client is not spelled out. The docs only say exclusive init fails "if another app already has exclusive control", which a shared client doesn't hold, so it should succeed [I]. Test it.

**What Discord sees, by how ClippiBoy opened the camera first** (Discord's own API is [U], see §4):

| ClippiBoy opened first with … | Discord via MF/WinRT controller | Discord via DirectShow (likely) |
|---|---|---|
| plain `MFCreateDeviceSource` + `IMFSourceReader` (controller) | **fails**: busy. Chromium gets `MF_E_HW_MFT_FAILED_START_STREAMING` and reports "Camera is in use by another process" [P] | **fails** [I, from "exclusive" behaviour] |
| `MediaCapture` `ExclusiveControl` | **fails** (exclusive init fails if another app holds exclusive) [P] | **fails** [I] |
| `MediaCapture` `SharedReadOnly` / MF sensor group `Shared` | **should work**: Discord becomes controller and sets its format; ClippiBoy gets `FormatChanged` and continues [I from P semantics] | **[U]**: decisive test case |
| *Discord first*, then ClippiBoy shared | ClippiBoy gets frames in Discord's format [P] | ClippiBoy **may get no frames** [P, #31] |
| Multi-App Camera toggle **on** | all clients treated as shared; one forced media type [P] | **[U]**: Microsoft's description says "all apps", but it is untested whether DirectShow clients are covered |

---

## 3. Windows 11 "Multi-App Camera"

- **What it is:** a per-camera toggle under Settings > Bluetooth & devices > Cameras > (camera) > *Advanced camera options* > Edit: "Allow multiple apps to use camera at the same time". The Insider post says it "allows multiple applications to access the camera stream simultaneously". It was built with the Hard-of-Hearing community (a sign-language interpreter and the audience get the same feed). A sibling toggle, **Basic camera**, is for debugging, "as a last resort" [P: [Insider blog, build 26120.2702, 13 Dec 2024](http://blogs.windows.com/windows-insider/2024/12/13/announcing-windows-11-insider-preview-build-26120-2702-dev-channel/); first seen in Canary 27758, 4 Dec 2024 [P: [Insider blog](http://blogs.windows.com/windows-insider/2024/12/04/announcing-windows-11-insider-preview-build-27758-canary-channel/)]].
- **How it works:** Microsoft's engineer describes it as provisioning the camera "as if all apps are using a 'sharing mode'". It "comes with some drawbacks however such as forcing a single MediaType", and basic controls must be set from the OS Settings page [P: [#72](https://github.com/microsoft/Windows-Camera/issues/72)]. The Policy CSP name is **AutoShare**: "Allow camera access by multiple applications simultaneously". *SafeMode* (Basic camera): "configured to auto-share and operate in a non-accelerated mode, bypassing OEM/IHV-provided plugins". Policy `./Device/Vendor/MSFT/Policy/Config/Camera/ConfigureCameraOptions` (0 = disabled, default; 1 = AutoShare; 2 = SafeMode), **"Windows 11, version 24H2 [10.0.26100] and later"**, Pro/Enterprise/Education/IoT [P: [Policy CSP – Camera](https://learn.microsoft.com/en-us/windows/client-management/mdm/policy-csp-camera)]. The Group Policy path is Computer Configuration > Administrative Templates > Windows Components > Camera > Configure Camera Options [P: release notes below].
- **Media type:** a third option, "selecting a media type" (resolution and frame rate, default "Let Windows Choose"), was announced for a later build [P: Insider blog]. A guide says it can only be changed with multi-app or basic camera on [S: [ElevenForum, via search](https://www.elevenforum.com/t/choose-media-type-for-preferred-camera-resolution-in-windows-11.31205)]. Whether it shipped to retail is **[U]**. **In multi-app mode Windows/the user picks the one format, not the apps [I from P "forcing a single MediaType"].** So Discord and ClippiBoy both get that format, possibly converted.
- **Builds / retail:**
  - Release Preview, 14–19 May 2026, builds **26100.8521/26200.8521 (KB5089573)**, under *Gradual rollout*: "[Camera] New! Windows 11's Multi-App Camera feature allows multiple applications to access your camera stream at the same time…" plus the Group Policy [P: [Release notes](https://learn.microsoft.com/en-us/windows-insider/release-notes/release-preview-24h2-25h2/build-26100-8514-26200-8514)].
  - Retail optional update **KB5089573, 26 May 2026, OS builds 26200.8524 / 26100.8524**, same item [P: [support.microsoft.com/help/5089573](https://support.microsoft.com/help/5089573)]. Coverage expected it in a later mandatory Patch Tuesday [S: TechRepublic via search].
  - So: **yes, it is in 26100/26200 retail from .8524 on, but gradually rolled out and off by default.** Your 26200 machine shows it only if it is at ≥ .8524 *and* the rollout has reached it. Pro editions can force it with the Group Policy above. Some outlets dated it to 26100.3321 (March 2025) [S: Pureinfotech], which conflicts with Microsoft's 2026 notes; I trust the Microsoft notes.
  - Not on Windows 10 and not on Windows 11 before 24H2 [P: CSP applicability].
- **Scope and limits:**
  - "All apps" per Microsoft's description [P]. Whether DirectShow apps (OBS, probably Discord) take part is **[U]**.
  - With it on, controls (brightness etc.) only work from the Settings page [P/S: [#72](https://github.com/microsoft/Windows-Camera/issues/72), [Windows Latest](https://www.windowslatest.com/2024/08/21/windows-11-24h2-will-finally-allow-multiple-apps-to-use-your-camera-at-the-same-time/)].
  - One tester ran the Camera app and Teams at once on the Insider build [S: [Rob Quickenden](https://robquickenden.blog/2024/12/windows11-camera-in-multiapps/)].
  - An app cannot detect or set the toggle. No API for that was found **[U]**.

---

## 4. How the apps capture

### Discord desktop: **[U], probably DirectShow**

- Discord: "Our desktop, iOS, and Android applications make use of a single C++ media engine built on top of the WebRTC native library" [P: [Discord engineering blog](https://discord.com/blog/how-discord-handles-two-and-half-million-concurrent-voice-users-using-webrtc)].
- WebRTC native's Windows camera module (`modules/video_capture/windows/`) is DirectShow only: `device_info_ds.cc`, `sink_filter_ds.cc`, `video_capture_ds.cc` [P: [webrtc source](https://webrtc.googlesource.com/src/+/main/modules/video_capture/windows/)]. Whether Discord kept that module or replaced it is not public **[U]**.
- Signs pointing to DirectShow:
  - DirectShow virtual cameras (OBS VirtualCam style filters) work in Discord on Windows 10 [S: [mycam PR #26](https://github.com/whoissaaif/mycam/pull/26)].
  - In July 2026, a camera failing in the MF-based Camera app and in Chromium browsers still worked in Zoom and Discord, and forcing Brave off MF onto DirectShow made it work there too [S: [Q&A 5949016](https://learn.microsoft.com/en-sg/answers/questions/5949016/windows-camera-not-working-in-camera-app-browsers)].
- Counter-claim: one third-party README lists Discord among "Media Foundation apps" without evidence [S: [CamBridge](https://github.com/incomparableus-lang/CamBridge)].
- **Behaviour when the camera is busy:** no Discord documentation found **[U]**. Generic guides say to close other apps [S]. Record the exact UI text in the test.
- **How to settle it cheaply:** with Process Explorer, compare the DLLs loaded by the Discord process before and after you start video. DirectShow capture loads `ksproxy.ax` / `qcap.dll` / `devenum.dll`; MF capture loads `mfreadwrite.dll` / `mfsensorgroup.dll` / MF capture-engine modules. The module names are a heuristic [I]. Also see §8 A7.

### Chromium (Chrome, Edge, Discord-in-browser, WebView2 apps)

- MF video capture: field-trial at 100% of stable since Oct 2018, default-on in code since 11 Jan 2019 (commit position 621848, ≈M73) [P: [commit](https://chromium.googlesource.com/chromium/src.git/+/c4f58b3898a92a0f7001f45415acf215b6481c65)]. Before that there was DirectShow.
- DirectShow is still used for devices MF can't see. DirectShow virtual cameras get appended ("DirectShow virtual cameras are not supported by MediaFoundation … we append missing DirectShow device descriptor") [P: [factory source](https://chromium.googlesource.com/chromium/src/+/main/media/capture/video/win/video_capture_device_factory_win.cc)]. There is also a 2018 fallback when MF reports a device without formats [P: [commit](https://chromium.googlesource.com/chromium/src.git/+/3772661191b045df5ed2df7aa6fde0a787967fae)].
- Chromium uses `IMFCaptureEngine` with no sharing flag [P: [device source](https://raw.githubusercontent.com/chromium/chromium/main/media/capture/video/win/video_capture_device_mf_win.cc)], so it is a controller. Microsoft confirms "web clients" ask for exclusive control [P].
- **Busy handling** [P, same file]:
  - "If the capture device is already in use by a different application, MediaFoundation will still emit MF_CAPTURE_ENGINE_PREVIEW_STARTED, and only after that raise an error event."
  - `if (hr == MF_E_HW_MFT_FAILED_START_STREAMING) { // This may indicate that the camera is in use by another application.` It then asks `IMFSensorActivityMonitor` and reports `kWinMediaFoundationCameraBusy`, "Camera is in use by another process".
  - The factory marks such devices `kUnavailableExclusivelyUsedByOtherApplication` [P: factory source].
- Users can force DirectShow with `chrome://flags/#enable-media-foundation-video-capture` [S: Q&A 5949016].

### OBS Studio

- The camera source ("Video Capture Device", `win-dshow`) is DirectShow: "OBS is only currently compatible with DirectShow" (moderator, 2021) [S: [OBS forum](https://obsproject.com/forum/threads/support-for-microsoft-translator-and-camera-app.136563)].
- A Media Foundation capture source PR (#10471, "Video Capture Device 2", Windows 11 APIs) is still **open** (milestone 32.2, change requests as of Sep 2026) [P: [PR #10471](https://github.com/obsproject/obs-studio/pull/10471)].
- OBS asks for exclusive control [P: #72]. Forum consensus: "Only a single app can access a video capture device at once". OBS grabs the camera at start-up unless "Deactivate when not showing" is set [S: [OBS forum](https://obsproject.com/forum/threads/cant-use-separate-camera-apps-at-the-same-time-as-obs.139242/)]. Busy log: "device already in use" / 0x8007001F [S].
- OBS Virtual Camera is a DirectShow filter [S: [issue #8057](https://github.com/obsproject/obs-studio/issues/8057)].

### Teams

- New Teams is built on Edge WebView2 [S: [office365itpros](https://office365itpros.com/tag/electron/)]. Its calling media stack and capture API are **[U]**. Microsoft's engineer notes Teams, Skype and Zoom "still support DShow sources in some capacity" [P: #72].
- A 2025 report shows Teams as the controller choosing MJPEG, which a SharedReadOnly reader could not consume on that laptop [S: Q&A 5608812].

### Windows Camera app / Settings preview

- Camera app: exclusive client [P: #72]. When another app holds the camera it shows **0xA00F4243 `<CameraReservedByAnotherApp>` (0xC00D3704)** [S: [Q&A 3805711](https://learn.microsoft.com/en-us/answers/questions/3805711/camera-not-working-and-getting-error-code)].
- Settings camera preview: **shared** client [P: #72]. A free shared-mode test client.

---

## 5. Logitech C920 and eMeet C960

### C920 hardware variants (check the USB ID in Device Manager > Details > Hardware Ids)

- **046d:082d**: original C920, *with* a hardware H.264 encoder. **046d:08e5**: 2021 version, "different electronics without the H264 hardware encoder" (an "Orbicam" 046d:0892 variant also lacks it) [S: [BATC wiki](https://wiki.batc.org.uk/Portsdown_Video_Sources)].
- A newer C920's Linux format list shows YUYV and MJPG only, no H264 [S: [classmethod](https://dev.classmethod.jp/articles/opencv-webcam-setting/)].
- The 08e5 variant intermittently sends MJPEG frames without the EOI marker at 1080p30 (Linux quirk proposal, June 2026). It "reportedly does not occur on Windows" [S: [LKML patch](https://lkml.iu.edu/2606.3/09447.html)].
- H.264 from the camera is irrelevant for ClippiBoy (it encodes itself) and was historically a sharing problem anyway [S: 2016 press].

### Formats over USB 2.0 (both cameras are USB 2.0 UVC)

- A high-speed isochronous endpoint moves at most 3 × 1024 = **3,072 bytes per 125 µs microframe = 24,576 bytes/ms ≈ 24.6 MB/s** [P: [MS USB isochronous docs](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/transfer-data-to-isochronous-endpoints)]. YUY2 uses 2 bytes per pixel:

| Mode | YUY2 bandwidth | fits in 24.6 MB/s? |
|---|---|---|
| 640×480 @30 | 18.4 MB/s | yes |
| 1280×720 @10 | 18.4 MB/s | yes |
| 1280×720 @30 | 55.3 MB/s | **no** |
| 1920×1080 @5 | 20.7 MB/s | yes |
| 1920×1080 @30 | 124 MB/s | **no** |

- This matches published C920 mode lists: YUYV 1280×720 at 5/7.5/10 fps, 1920×1080 at 5 fps; MJPG 1080p and 720p up to 30 fps [S: classmethod and others via search; [NVIDIA forum](https://forums.developer.nvidia.com/t/jetpack-4-2-2-usb-webcam-very-low-framerate/157440)].
- **eMeet C960:** max 1080p30, codecs YUY2/MJPEG, USB 2.0, fixed focus, "not so good in poor light" [S: [PC-WELT test](https://www.pcwelt.de/article/1940083/emeet-smartcam-c960-test-webcam.html)]. A "C960 2K" variant is sold too [S: retailer listings]. Record the exact model and USB ID.
- **Consequence:** ClippiBoy's default 720p30 needs **MJPEG** on both cameras, so somebody decodes MJPEG: Frame Server, MF decoder MFT or the app. That is the path where shared mode has failed on some hardware [S: Q&A 5608812]. → Test 720p MJPEG with Discord also at MJPEG.

### Concurrency on UVC

- One UVC streaming interface delivers one format at a time. A typical USB camera exposes a single record pin to sharing apps [P: wiki]. So all clients share one sensor format. Per-client conversion is possible in the Frame Server/frame reader [S/U].
- Vendor tools (Logi Tune/G HUB, eMeet Studio) may open the camera themselves [I]. Keep them closed during tests, or test them as one more client.

### Low light, variable frame rate, timestamps

- `KSPROPERTY_CAMERACONTROL_AUTO_EXPOSURE_PRIORITY`: 1 = "Frame rate can be dynamically varied by the device", e.g. 30 → 25 fps for 40 ms exposure. It maps to the UVC property page's **"Low-Light Compensation"** checkbox. Documented default 0, and it requires auto-exposure [P: [docs](https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/ksproperty-cameracontrol-auto-exposure-priority)]. WinRT `ExposurePriorityVideoControl`: "Apps that require a constant frame rate should disable this control" [P: [docs](https://learn.microsoft.com/en-us/uwp/api/windows.media.devices.exposurepriorityvideocontrol)].
- C920 users see frame-rate drops from "Low Light Compensation"/RightLight (on by default in Logitech software) [S: [OpenCV forum](https://forum.opencv.org/t/opencv-camera-low-fps/567), [Reallusion forum](https://forum.reallusion.com/343827/Group1.aspx)].
- **A shared client cannot set controls** [P], and in multi-app mode controls live only in Settings [P]. So ClippiBoy **cannot** force constant fps while sharing. The pipeline must take each frame's own timestamp.
- Timestamps: `MediaFrameReference.SystemRelativeTime` is "100ns interval QPC time", "correlatable across multiple media sources" [P: [docs](https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.frames.mediaframereference.systemrelativetime)]. That is the same clock family as Windows Graphics Capture's `SystemRelativeTime` and WASAPI QPC positions [I]. MF path: `MFSampleExtension_DeviceTimestamp` is in the QPC epoch [P: [docs](https://learn.microsoft.com/en-us/windows/win32/medfound/mfsampleextension-devicetimestamp)].
- `MediaFrameReader.AcquisitionMode = Buffered` (1709+) keeps every frame in order. The default `Realtime` drops frames to deliver the newest [P: MediaFrameReader article]. Use Buffered for a recorder.

---

## 6. Windows 10 vs 11, regressions, error codes, privacy

| | Windows 10 (1607–22H2) | Windows 11 21H2–23H2 | Windows 11 24H2/25H2 (26100/26200) |
|---|---|---|---|
| Frame Server, SharedReadOnly, sensor-group shared | yes (1607+) [P] | yes | yes |
| Buffered frame reader | 1709+ [P] | yes | yes |
| `MFCreateVirtualCamera` | **no** (Win11 build 22000+) [P: [docs](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nf-mfvirtualcamera-mfcreatevirtualcamera)] | yes | yes |
| Multi-App Camera / AutoShare | no | no | from .8524 (May 2026), gradual, off by default [P] |
| Camera control monitor (`MFCreateCameraControlMonitor`) | no | 22H2+ (per Chromium comment) [P/S] | yes |

- **Windows 10 support:** mainstream support ended 14 Oct 2025. Consumer ESU was reportedly extended to Oct 2027 [S: [Help Net Security](https://www.helpnetsecurity.com/2026/06/26/microsoft-windows-10-free-security-updates-esu-program/), not checked on a Microsoft page]. Windows 10 has no virtual-camera API and no multi-app toggle, so a Windows 10 user who needs to share needs a DirectShow virtual camera.
- **Privacy:** Settings > Privacy & security > Camera, "Let/Allow desktop apps access your camera", a single switch for all unpackaged apps. "Desktop apps might not appear in the list" and "might still be able to access the camera … even when these settings are turned off" [P: [support article](https://support.microsoft.com/en-us/windows/manage-app-permissions-for-a-camera-in-windows-87ebc757-1f87-7bbf-84b5-0686afb6ca6b)]. When blocked, capture APIs return **E_ACCESSDENIED (0x80070005)** from `IMFActivate::ActivateObject` or `MediaCapture.InitializeAsync`. Send the user to `ms-settings:privacy-webcam` [P: [docs](https://learn.microsoft.com/en-us/windows/apps/develop/camera/camera-privacy-setting)]. `MFCreateVirtualCamera` returns E_ACCESSDENIED under the same privacy block [P].
- **Error codes:**
  - `MF_E_HW_MFT_FAILED_START_STREAMING` = **0xC00D3704**: "device cannot enter the run state" [P: [HCK doc](https://learn.microsoft.com/en-us/previous-versions/windows/hardware/hck/dn452261(v=vs.85))]. This is the "camera busy" error in practice: Chromium [P], Camera app `0xA00F4243<CameraReservedByAnotherApp> (0xC00D3704)` [S].
  - `MF_E_VIDEO_RECORDING_DEVICE_INVALIDATED` = 0xC00D3EA2, "no longer present" (unplugged) [S: [hexerror](https://windows-hexerror.linestarve.com/0xC00D3EA2)].
  - `MF_E_VIDEO_RECORDING_DEVICE_PREEMPTED` = 0xC00D3EA3, "preempted by another immersive application" [S: [hexerror](https://windows-hexerror.linestarve.com/0xC00D3EA3)].
  - **`MF_E_VIDEO_RECORDING_DEVICE_LOCKED`: I found no evidence this constant exists [U].** The plan should use the codes above.
  - Camera app generic failure `0xA00F4271 <MediaCaptureFailedEvent>` [S].
  - OBS/DirectShow busy: 0x8007001F [S].
- **Regressions:** the 24H2 safeguard hold 53340062 (integrated cameras with face/object detection; lifted Sept 2025) does not concern USB UVC cameras [S: [Windows Report](https://windowsreport.com/microsoft-fixes-camera-issue-linked-to-object-face-detection-and-resumes-windows-11-update-on-affected-pcs/)]. Windows Studio Effects lives in the OEM camera pipeline on NPU devices. "Basic camera"/SafeMode bypasses "OEM/IHV-provided plugins" [P: CSP]. Not expected to matter for C920/C960 on a desktop [I]. No USB-webcam sharing regression for 26100/26200 was found [U].
- Multi-app mode forces one media type and moves controls to Settings [P]. Teams/Meet worked in early reports and "older or specialized software may still fail" [S: windowsforum via search].

---

## 7. What this means for ClippiBoy

1. **Open the camera in shared mode, never as controller, while the buffer runs.**
   - **Recommended:** WinRT `MediaCapture` with `MediaCaptureInitializationSettings { SourceGroup = <group of the chosen camera>, SharingMode = SharedReadOnly, StreamingCaptureMode = Video, MemoryPreference = Cpu (or Auto for D3D surfaces) }`. Then `CreateFrameReaderAsync(colorSource, "NV12", 1280×720)` and `AcquisitionMode = Buffered`. Use `SystemRelativeTime` (QPC) as the frame clock.
   - **Why:**
     - It is the documented and supported sharing path; Microsoft calls it the "cleaner" API, usable from desktop apps [P: wiki].
     - It has the events ClippiBoy needs: `FormatChanged`, `CaptureDeviceExclusiveControlStatusChanged`.
     - It gives a fixed output size no matter which format the controller chose.
     - QPC timestamps line up with screen and sound.
   - In windows-rs 0.58 this needs WinRT features such as `Media_Capture`, `Media_Capture_Frames`, `Media_MediaProperties`, `Media_Devices`, `Devices_Enumeration`, `Graphics_Imaging`, `Foundation`, `Foundation_Collections`. The crate currently enables only `Graphics_Capture` among WinRT namespaces (`src-tauri/Cargo.toml`). Feature names: verify with `cargo doc` [I].
   - **Alternative to evaluate in the probe:** Win32 MF `MFCreateSensorGroup(symlink)` → `GetSensorDevice(0)` → `SetSensorDeviceMode(MFSensorDeviceMode_Shared)` → `CreateMediaSource` → `MFCreateSourceReaderFromMediaSource` [P: wiki]. It stays inside the already-enabled `Win32_Media_MediaFoundation` feature. Choose it only if it behaves exactly like the WinRT path with Discord.
   - **Do not use** plain `MFCreateDeviceSource` / `ActivateObject` + Source Reader, or `ExclusiveControl`. Either one blocks Discord, browsers, Teams, OBS and the Camera app [P/I].
2. **Accept that someone else owns the format.** A shared client gets the controller's media type and may see it change mid-buffer (Discord starting video at its resolution). Handle `FormatChanged` without dropping the ring, or start a new segment. Always scale to the cam track's fixed size.
   - When ClippiBoy is the only client, the camera streams some default format **[U]**. A trick to evaluate: briefly open `ExclusiveControl` to set 720p30 MJPEG, start the shared reader, then dispose the exclusive instance. Only do it when `IMFSensorActivityMonitor` says no other process is streaming. Whether the format persists is **[U]**.
3. **Variable frame rate is normal.** ClippiBoy can't turn off low-light compensation from a shared client. Timestamp per frame. Encode VFR or resample to CFR on the clip timeline. Show "cam fps" in diagnostics.
4. **Busy and blocked states in the UI:**
   - E_ACCESSDENIED → privacy hint plus `ms-settings:privacy-webcam`.
   - 0xC00D3704 → "camera is used by another app". Use `IMFSensorActivityMonitor` to name the process [P: [docs](https://learn.microsoft.com/en-us/windows/win32/api/mfidl/nf-mfidl-mfcreatesensoractivitymonitor); Chromium does the same].
   - 0xC00D3EA2 → unplugged.
   - "Shared but no frames for N seconds" → probably a DirectShow controller (Discord/OBS) is starving ClippiBoy. Say so, and offer the virtual camera or the Windows multi-app toggle once it exists.
5. **Virtual camera design hints**, for whenever it comes:
   - ClippiBoy then becomes the *only* client of the physical camera and may hold it exclusively, with full format and control rights.
   - Microsoft warns against opening an existing camera "in sharing mode from within your virtual camera" ("undefined behavior", "random and undesired MediaType changes"). Only one virtual camera can wrap a given physical camera [P: [VirtualCamera README](https://github.com/microsoft/Windows-Camera/blob/master/Samples/VirtualCamera/README.md)]. ClippiBoy feeding frames through shared memory matches that advice.
   - Win11 MF virtual cameras are visible to DirectShow apps too [S: [alax.info](https://alax.info/blog/2245)], so Discord should see one whatever API it uses [I].
   - The name gets "Windows Virtual Camera" appended. `AllUsers` access needs admin [P: MFCreateVirtualCamera docs].
6. **Multi-App Camera** is a documentation and help item ("If your Windows 11 shows *Allow multiple apps to use camera at the same time*, turning it on also works"). It is not a design dependency: off by default, gradual rollout, Windows 11 24H2+ only, one forced format [P].

---

## 8. Stage-1 test protocol

### 8.0 Record before every session

- `winver` full build (e.g. 26200.xxxx), edition, and whether *Advanced camera options* shows the multi-app toggle and its state.
- Camera: model as printed, USB **Hardware Ids** (C920 082d / 08e5 / other), driver (Device Manager > Driver details: inbox `usbvideo.sys` or vendor), port (USB 2/3, direct or hub), firmware if Logi Tune shows it.
- App versions: Discord (Help > About), Chrome/Edge, OBS. Vendor camera software closed (or listed).
- Privacy: camera access on, desktop apps on.
- Light: a fixed lamp setting. Note "bright" or "dim" per run.

### 8.1 Phase A: zero-code stand-ins (about 1 hour per camera, Windows 11, toggle OFF)

Known client types:

| Code | Client | Mode |
|---|---|---|
| **S** | Settings > Bluetooth & devices > Cameras > (camera) preview | shared [P] |
| **C** | Windows Camera app | exclusive, WinRT [P] |
| **B** | Chrome or Edge on a webcam test page (`getUserMedia`) | MF controller [P] |
| **O** | OBS Video Capture Device, "Deactivate when not showing" off | DirectShow, exclusive [P] |
| **D** | Discord desktop, Settings > Voice & Video > Test Video, then a real video call with a second account or phone | unknown |
| **Dw** | Discord in the browser | MF controller, like B |

Keep both windows visible (not minimized) and wait 10 s per step.

| # | Order | Expected if theory holds | What it tells you |
|---|---|---|---|
| A1 | D alone | works | baseline |
| A2 | **S then D** | D works, S keeps running | **ClippiBoy-first case** with a shared reader |
| A3 | **D then S** | S shows video (if D is MF) / S black or frozen (if D is DirectShow) | **Discord-first case**, the DirectShow question |
| A4 | C then D, D then C | second one fails | Discord is a controller (expected) |
| A5 | O then S, S then O | O→S: S black (DirectShow blocks shared) | calibrates DirectShow behaviour on this PC |
| A6 | B then S, S then B | both work | calibrates MF-controller and shared behaviour |
| A7 | Process Explorer on Discord while D starts video | DirectShow DLLs (`ksproxy.ax`, `qcap.dll`) or MF DLLs appear | Discord's API |
| A8 | A2, A3, A5 again with the **multi-app toggle ON** (if present) | all work; one fixed format | value of the toggle, DirectShow coverage |

**Observe and record per step:**

- works / fails / black / frozen
- exact error text (Discord, Camera app code)
- whether the *first* app keeps running, freezes or changes resolution
- resolution/fps each app shows: Discord's video debug overlay if available, OBS source properties, Chrome `chrome://media-internals`
- camera LED behaviour (a restart blink means a renegotiation)

Repeat A2/A3 with Discord at its default and at its highest camera quality.

### 8.2 Phase B: throwaway probe (half a day; Rust or C#)

A console tool, not part of the app (separate crate or a .NET 8 console with `net8.0-windows10.0.19041.0`; C# is the fastest for WinRT). Rust with windows-rs 0.58 also proves the bindings ClippiBoy will use.

Commands:

- `list`: MediaFrameSourceGroups + MF devices. Symbolic link, KS categories, and all formats (subtype, size, fps) per source.
- `read --mode {winrt-shared | mf-shared | mf-plain | winrt-exclusive} --cam N --out NV12 --size 1280x720 --secs 120`. Log once per second:
  - frames received; min/avg/max inter-frame interval from `SystemRelativeTime` / sample time
  - `CurrentFormat`
  - every `FormatChanged`, every exclusive-status change
  - every error as hex HRESULT
  - QPC-now minus frame time (latency)
  - dump every 300th frame as PNG to check the picture isn't garbage. The Q&A 5608812 case got "colorful noise".
- `monitor`: `MFCreateSensorActivityMonitor` and print, on every change, which PIDs/processes stream which camera. Run it in a second window during all tests. It also shows whether a DirectShow Discord appears as a Frame Server client.
- `prime`: winrt-exclusive → set 1280×720 MJPEG 30 → start winrt-shared reader → dispose exclusive. Does the format survive? Does a later D still start?

**Matrix** (cut it down if time is short; first rows first):

1. Win11 26200, toggle off × {C920, C960} × {winrt-shared, mf-shared} × {probe→D, D→probe} × {bright, dim}.
2. Robustness: probe running 30 min while D toggles video every 2 min. Also D running and probe stop/start. Unplug/replug (expect 0xC00D3EA2). Privacy off (expect E_ACCESSDENIED).
3. Negative controls: mf-plain and winrt-exclusive → D must fail. This confirms the test setup sees the conflict.
4. Toggle ON (if available): repeat row 1 for one camera.
5. **Windows 10 22H2:** row 1 + row 3 for one camera. A real machine or tester is better than a VM. VM USB passthrough of an isochronous UVC camera is unreliable [I]. Chromium even blocklists "VMware Virtual Webcams" because they hang [P]. If a VM is the only option, pass the *physical* USB camera through, not the hypervisor's virtual webcam.

**Pass criterion per cell:**

- Discord's own preview **and** the remote side see the user.
- The probe receives ≥ 25 fps in bright light at 720p with no 2-second gaps, and frames are visually correct.
- Nothing freezes after a `FormatChanged`.

### 8.3 Interpretation → decision

| Result (Windows 11, toggle off, both cameras) | Meaning | Decision |
|---|---|---|
| A2 and A3 both pass; probe winrt-shared works in both orders, also in dim light and at 720p MJPEG | Discord is an MF/WinRT controller or the DirectShow block doesn't apply here | **Camera source = shared mode. The virtual camera stays at stage 5.** Windows 10 result only sets its stage-5 priority |
| A2 passes, A3 fails (probe gets no frames while Discord holds the camera) | Discord holds the camera via DirectShow (A7 should agree); Microsoft says nothing helps | ClippiBoy loses the cam in every Discord video call, the main use case. **Move the virtual camera forward** (into or right after stage 2), Windows 11 first. Ship stage 2 with the shared reader plus a clear "cam taken by Discord" state |
| A2 fails (Discord can't start while S/probe is shared) | A shared client blocks a later DirectShow controller | Same as above, and worse: ClippiBoy would break Discord. **Virtual camera first, or cam off by default until it exists** |
| Only one camera passes | driver/firmware dependent (e.g. MJPEG decoder path) | Shared mode as default, virtual camera forward for the failing class. Test more cameras (ask users) |
| Everything passes only with the toggle on | Windows' AutoShare covers it | Document the toggle as the Windows 11 24H2+ workaround. Virtual camera still needed for Windows 10, older Windows 11 builds and users without the rollout: moderate priority, not first |
| Probe works but frames garbage/null with MJPEG controller (as in Q&A 5608812) | MJPEG shared path broken on that hardware | Ask the probe for YUY2/BGRA8 instead of NV12 (worked there [S]), or virtual camera |

Also file whatever you learn about Discord's API (A7) in the spec. It decides whether a Windows 10 DirectShow filter is enough for Discord, and whether a Windows 11 MF virtual camera is enough for every app.

---

## 9. Open questions (unconfirmed, need the test or more digging)

- Discord desktop's capture API (DirectShow via WebRTC's `video_capture_ds`, or its own MF path) and its behaviour and error text when the camera is busy.
- Whether a DirectShow controller arriving *after* a shared MF/WinRT reader can start, and whether the reader survives.
- What format a *first* shared client gets, and whether a format set by a short-lived exclusive client persists.
- Whether multi-app camera (AutoShare) covers DirectShow clients, and how the single media type is chosen ("Let Windows choose").
- Whether `MF_E_VIDEO_RECORDING_DEVICE_LOCKED` exists at all (not found).
- Teams' calling stack capture API.
- Exact windows-rs 0.58 feature names for the WinRT capture namespaces (check locally).

---

## Sources

Primary (Microsoft docs and notes, Microsoft repos and engineers, Chromium/WebRTC/Discord):

- MediaCaptureSharingMode: https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.mediacapturesharingmode
- Process media frames with MediaFrameReader: https://learn.microsoft.com/en-us/windows/apps/develop/camera/process-media-frames-with-mediaframereader
- CaptureDeviceExclusiveControlStatusChanged: https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.mediacapture.capturedeviceexclusivecontrolstatuschanged
- MediaFrameSource.FormatChanged: https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.frames.mediaframesource.formatchanged
- CreateFrameReaderAsync: https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.mediacapture.createframereaderasync
- MediaFrameReference.SystemRelativeTime: https://learn.microsoft.com/en-us/uwp/api/windows.media.capture.frames.mediaframereference.systemrelativetime
- ExposurePriorityVideoControl: https://learn.microsoft.com/en-us/uwp/api/windows.media.devices.exposurepriorityvideocontrol
- MFSensorDeviceMode: https://learn.microsoft.com/en-us/windows/win32/api/mfidl/ne-mfidl-mfsensordevicemode
- IMFSensorDevice::SetSensorDeviceMode: https://learn.microsoft.com/en-us/windows/win32/api/mfidl/nf-mfidl-imfsensordevice-setsensordevicemode
- MFCreateSensorGroup: https://learn.microsoft.com/en-us/windows/win32/api/mfidl/nf-mfidl-mfcreatesensorgroup
- MFCreateSensorActivityMonitor: https://learn.microsoft.com/en-us/windows/win32/api/mfidl/nf-mfidl-mfcreatesensoractivitymonitor
- MFCreateVirtualCamera: https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nf-mfvirtualcamera-mfcreatevirtualcamera
- MFSampleExtension_DeviceTimestamp: https://learn.microsoft.com/en-us/windows/win32/medfound/mfsampleextension-devicetimestamp
- KSPROPERTY_CAMERACONTROL_AUTO_EXPOSURE_PRIORITY: https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/ksproperty-cameracontrol-auto-exposure-priority
- USB isochronous transfers: https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/transfer-data-to-isochronous-endpoints
- Camera Profile V2 overview (Frame Server since 1607): https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/camera-profile-v2-architectural-overview
- HCK doc on 0xC00D3704: https://learn.microsoft.com/en-us/previous-versions/windows/hardware/hck/dn452261(v=vs.85)
- Camera privacy setting: https://learn.microsoft.com/en-us/windows/apps/develop/camera/camera-privacy-setting
- Manage app permissions for a camera: https://support.microsoft.com/en-us/windows/manage-app-permissions-for-a-camera-in-windows-87ebc757-1f87-7bbf-84b5-0686afb6ca6b
- Policy CSP – Camera (ConfigureCameraOptions): https://learn.microsoft.com/en-us/windows/client-management/mdm/policy-csp-camera
- Release Preview 26100.8521/26200.8521 notes: https://learn.microsoft.com/en-us/windows-insider/release-notes/release-preview-24h2-25h2/build-26100-8514-26200-8514
- KB5089573 (26200.8524/26100.8524): https://support.microsoft.com/help/5089573
- Insider build 26120.2702: http://blogs.windows.com/windows-insider/2024/12/13/announcing-windows-11-insider-preview-build-26120-2702-dev-channel/
- Insider build 27758: http://blogs.windows.com/windows-insider/2024/12/04/announcing-windows-11-insider-preview-build-27758-canary-channel/
- Windows-Camera wiki: https://github.com/microsoft/Windows-Camera/wiki
- Windows-Camera VirtualCamera README: https://github.com/microsoft/Windows-Camera/blob/master/Samples/VirtualCamera/README.md
- Windows-Camera issue #31: https://github.com/microsoft/Windows-Camera/issues/31
- Windows-Camera issue #72: https://github.com/microsoft/Windows-Camera/issues/72
- Chromium video_capture_device_factory_win.cc: https://chromium.googlesource.com/chromium/src/+/main/media/capture/video/win/video_capture_device_factory_win.cc
- Chromium video_capture_device_mf_win.cc: https://raw.githubusercontent.com/chromium/chromium/main/media/capture/video/win/video_capture_device_mf_win.cc
- Chromium "Enable MF capture by default": https://chromium.googlesource.com/chromium/src.git/+/c4f58b3898a92a0f7001f45415acf215b6481c65
- Chromium DirectShow fallback: https://chromium.googlesource.com/chromium/src.git/+/3772661191b045df5ed2df7aa6fde0a787967fae
- WebRTC Windows video capture module: https://webrtc.googlesource.com/src/+/main/modules/video_capture/windows/
- Discord engineering blog: https://discord.com/blog/how-discord-handles-two-and-half-million-concurrent-voice-users-using-webrtc
- OBS PR #10471: https://github.com/obsproject/obs-studio/pull/10471

Secondary:

- OSR thread (2016, Roman Ryltsov, quotes of Mike M.): https://community.osr.com/t/windows-10-anniversary-update-and-testcap-avstream-drivers/53081
- PCWorld 2016: https://www.pcworld.com/article/416088/the-windows-10-anniversary-update-is-breaking-webcams.html
- SiliconANGLE 2016: https://siliconangle.com/2016/08/19/windows-10-update-blamed-for-breaking-webcams/
- mgraves.org 2016: https://www.mgraves.org/2016/08/can-you-see-me-now-microsoft-suffering-new-windows-10-webcam-strategy/amp/
- alax.info, Windows 11 Virtual Video Camera: https://alax.info/blog/2245
- Microsoft Q&A 171402: https://learn.microsoft.com/en-ca/answers/questions/171402/sharing-webcam-within-multiple-applications
- Microsoft Q&A 218589: https://learn.microsoft.com/en-us/answers/questions/218589/how-to-get-the-frame-from-webcam-that-other-applic
- Microsoft Q&A 5608812 (Teams MJPG vs SharedReadOnly): https://learn.microsoft.com/en-us/answers/questions/5608812/how-to-avoid-ms-teams-to-select-mjpg-as-camera-dat
- Microsoft Q&A 5949016 (Discord/Zoom work, MF apps fail): https://learn.microsoft.com/en-sg/answers/questions/5949016/windows-camera-not-working-in-camera-app-browsers
- Microsoft Q&A 3805711 (0xA00F4243 / 0xC00D3704): https://learn.microsoft.com/en-us/answers/questions/3805711/camera-not-working-and-getting-error-code
- Windows Latest 2024: https://www.windowslatest.com/2024/08/21/windows-11-24h2-will-finally-allow-multiple-apps-to-use-your-camera-at-the-same-time/
- Windows Latest, media type: https://www.windowslatest.com/2024/12/23/windows-11-now-lets-you-easily-change-camera-fps-frame-rate-quality-and-more/
- Thurrott on KB5089573: https://www.thurrott.com/?p=336114
- Rob Quickenden test: https://robquickenden.blog/2024/12/windows11-camera-in-multiapps/
- OBS forum (DirectShow only): https://obsproject.com/forum/threads/support-for-microsoft-translator-and-camera-app.136563
- OBS forum (one app at a time): https://obsproject.com/forum/threads/cant-use-separate-camera-apps-at-the-same-time-as-obs.139242/
- OBS issue #8057: https://github.com/obsproject/obs-studio/issues/8057
- mycam PR #26 (DirectShow vcam in Discord, Windows 10): https://github.com/whoissaaif/mycam/pull/26
- CamBridge README: https://github.com/incomparableus-lang/CamBridge
- smourier VCamSample: https://github.com/smourier/VCamSample
- BATC wiki (C920 variants): https://wiki.batc.org.uk/Portsdown_Video_Sources
- LKML C920 08e5 quirk: https://lkml.iu.edu/2606.3/09447.html
- classmethod (C920 formats): https://dev.classmethod.jp/articles/opencv-webcam-setting/
- NVIDIA forum (C920 640×480 YUYV 30): https://forums.developer.nvidia.com/t/jetpack-4-2-2-usb-webcam-very-low-framerate/157440
- PC-WELT eMeet C960 test: https://www.pcwelt.de/article/1940083/emeet-smartcam-c960-test-webcam.html
- OpenCV forum (C920 low-light fps): https://forum.opencv.org/t/opencv-camera-low-fps/567
- Reallusion forum (Low Light Compensation): https://forum.reallusion.com/343827/Group1.aspx
- New Teams on WebView2: https://office365itpros.com/tag/electron/
- Windows Report (24H2 camera safeguard): https://windowsreport.com/microsoft-fixes-camera-issue-linked-to-object-face-detection-and-resumes-windows-11-update-on-affected-pcs/
- Help Net Security (Windows 10 ESU extension): https://www.helpnetsecurity.com/2026/06/26/microsoft-windows-10-free-security-updates-esu-program/
- linestarve hexerror 0xC00D3EA2 / 0xC00D3EA3: https://windows-hexerror.linestarve.com/0xC00D3EA2 , https://windows-hexerror.linestarve.com/0xC00D3EA3
