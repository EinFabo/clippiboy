# Facecam Stage 5: Virtual Camera Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. This plan is rough on purpose: before a task is executed, expand it into checkbox steps with superpowers:writing-plans. Task 1 and Task 10 need a person at the Windows 11 26200 machine with both webcams, Discord and a second account. Task group 9 also needs a Windows 10 machine.

**Goal:** Discord, browsers, Teams and OBS on Windows 11 can pick **ClippiBoy Camera** and get the real webcam's frames unchanged, while ClippiBoy holds the real camera for the facecam. The user switches it on once in the Facecam tab and sees one UAC prompt.

**Architecture:**
- While the virtual camera is on, ClippiBoy opens the real camera **controlling** (the exception in spec 2.2) and publishes its raw frames into a fixed-size shared-memory section: protocol v1, four seqlock slots, a heartbeat.
- On Windows 11 it registers a Session-lifetime camera with `MFCreateVirtualCamera`. Its media source is a Rust COM DLL in its own Cargo project, built with `panic = "unwind"`, and runs inside Frame Server, where it reads the section.
- A small, signed, elevated helper carries the DLL inside itself and installs it under `%ProgramFiles%` and HKLM. The NSIS uninstaller calls it.
- A Windows 10 DirectShow filter in C++ is an optional task group, gated on Open for Fabi 5.

**Tech Stack:** Rust with `windows` 0.58 (Media Foundation, COM `#[implement]`), a separate cdylib workspace under `vcam/`, C++17 on the DirectShow BaseClasses (MIT, optional group), NSIS hooks, Authenticode signing.

**Spec:** `docs/superpowers/specs/2026-10-20-facecam-design.md`, section 4 (4.1–4.7), 2.2 (lifecycle and the controlling exception), 5.1 (`virtual_camera`), 5.2 item 6. Open for Fabi 3, 4 and 5. Background: `docs/extras/research/virtual-camera.md`: §7 (design), §7.3 (layout with offsets), §8 (licences), §9 (spike checklist).

**When:** right after stage 2 if the stage-1 result shows that Discord blocks a sharing reader (Open for Fabi 3 (a), the recommendation), otherwise after stage 4. It depends only on stage 2: the camera module (`camera::Source`, `camera::open(device, OpenMode, Sink)`, `camera::Frame` as NV12 in system memory at the camera's size, `camera::Error`, `camera::windows::holder`), `CamTrack` with its raw-frame tap `latest_frame()`, `CamWants`/`cam_wanted`, `AppState::cam_plan`/`reconcile_cam`, `src/routes/settings/Facecam.tsx`, `FacecamConfig.virtual_camera`, and the config revision fix of 5.1 (`config::accept`, `commands::tell_main`).

## Global Constraints

- English UI text. Commits in English, **without** a `Co-Authored-By` line.
- **No panic crosses a COM boundary.** Every export and every COM method is wrapped in `catch_unwind` and returns an HRESULT; there is no `unwrap`/`expect` on the COM path. The DLL is its own Cargo workspace with `panic = "unwind"`: the app's release profile is `panic = "abort"` (`src-tauri/Cargo.toml:106`), and Cargo does not allow that to be overridden per crate.
- The DLL only ever lives in `%ProgramFiles%\ClippiBoy Virtual Camera\<version>\`, written by the elevated helper from bytes inside itself. Nothing is copied from `%LOCALAPPDATA%` into a path SYSTEM loads from.
- Fixed CLSIDs and the fixed friendly name "ClippiBoy Camera", never changed across versions or languages: Windows keys the camera on these parameters.
- Frames pass through unchanged: no mirror, no effects, no watermark (spec decision).
- The IPC objects can be read only by the user, SYSTEM and LocalService. Never Everyone, Interactive Users or AppContainers.
- The protocol is versioned: another major version is refused, minor versions only add. A newer app works with an older DLL.
- On Windows 11 the MF camera and the DirectShow filter are never registered together, or DirectShow apps list two cameras.
- The UAC prompt appears only from the Facecam tab, never inside the passive updater.
- Licences: the MIT sources (Windows-Camera samples, VCamSample/VCamNetSample, softcam, BaseClasses, VL.Video) and JStream (MIT/Apache-2.0) may be adapted, and their notices ship. OBS (GPLv2) and akvirtualcamera (GPLv3) are read for ideas at most, never copied.
- Outward-facing steps (buying the certificate, adding signing secrets, publishing): ask Fabi first. Open for Fabi 4 must be "yes" before Task 5, and Open for Fabi 5 must be "(b)" before task group 9.

## Review Focus

- **A panic or a NULL argument in the source** takes down Frame Server and every camera on the machine → Task 4 `every_com_method_turns_a_panic_into_an_hresult`, `null_arguments_return_e_pointer`; the reader fuzz loop in Task 2; the soak test in Task 10.
- **Torn frames, or a dead producer shown as live** → Task 2 `a_torn_slot_is_never_accepted`, `a_stale_heartbeat_shows_not_running`.
- **Privilege escalation** through a DLL path a user can write to → Task 5 `install_refuses_a_path_outside_program_files`, and a review of the helper.
- **ClippiBoy opening the real camera controlling while Discord still uses it** brings back exactly the block of fact 1 → Task 3 `the_real_camera_stays_shared_while_another_app_streams_it`.
- **Uninstall leaving ClippiBoy Camera or the HKLM key behind**, or an update prompting → Task 6.

---

### Task 1: Spike on the Windows 11 26200 machine (virtual-camera.md §9) — first

**Files:** nothing is committed. A throwaway spike in `scratch/vcam-spike/` (add `scratch/` to `.git/info/exclude`, as in stage 1): Microsoft's VirtualCamera sample or VCamSample (both MIT), built and registered as admin, plus a producer of about 100 lines that creates `Local\ClippiBoy.VCam.v1` with the §7.3 DACL and writes a test pattern. The results go into the spec, as a new subsection "Result of the virtual-camera spike" in section 4.

**Checklist** (the nine items of §9, plus three):
1. Do the section **and** the event open from Frame Server as `Session\<N>\ClippiBoy.VCam.v1` with that DACL? Record `sc qsidtype FrameServer`.
2. Do the activate attributes carry `MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK`?
3. Do Discord or Chrome load the DLL in their own process (Process Explorer)?
4. System lifetime with CurrentUser access, not elevated: does it work, and is the camera still there after a reboot?
5. Which format does Discord pick (NV12 or YUY2, 720p or 1080p)? Does new Teams show the picture on the remote side?
6. Does a 32-bit DirectShow app see the MF camera?
7. On a Windows 10 VM: is an HKCU-only DirectShow registration enumerated in Discord and Chrome?
8. Chrome on Windows 10 with the mandatory label `S:(ML;;NWNR;;;ME)` on the section. NWNR is "no write up, no read up": a process below medium integrity cannot write to the section or read from it, so sandboxed low-integrity processes of the same user cannot read the frames.
9. Windows 11's "Allow multiple apps to use camera" switch with the C920 and the eMeet: do Discord and a sharing reader then share the real camera?
10. Does `windows` 0.58 expose `IMFMediaSourceEx_Impl`, `IMFMediaStream2_Impl` and `IKsControl_Impl`? (`cargo check` of an empty `#[implement]` in the spike.)
11. What does Smart App Control do with an unsigned DLL on this machine?
12. With ClippiBoy's producer stopped, does Discord show the source's placeholder, or "device invalidated"?

**Decisions:**

| Result | Then |
|---|---|
| `Session\<N>\` opens | the section is the transport |
| it does not | the named-pipe fallback `\\.\pipe\ClippiBoy.VCam.v1.<session>` (spec 4.5); Task 2 and Task 4 read records from the pipe |
| System lifetime works without admin | noted as the later option of 4.6, not built now |
| 0.58 lacks a trait | only the DLL's workspace moves to a newer `windows`, the app stays on 0.58 (design call) |
| item 9 shares the real camera | written into the Facecam tab's hint; the virtual camera is still needed for Windows 10 and older builds |

**Risks:** Frame Server keeps a DLL loaded, so `net stop FrameServer` is needed between builds (§1.7). The sample's installer registers machine-wide, so remove it afterwards.

**Done when:** every item has an answer in the spec and the transport is chosen.

---

### Task 2: The protocol crate and its C header

**Files:**
- Create: `vcam/protocol/Cargo.toml` (no runtime dependencies), `vcam/protocol/src/lib.rs` (`VcamHeader` and `SlotHeader`, `#[repr(C)]`, exactly as §7.3; the constants `HEADER_BYTES = 4096`, `SLOT_COUNT = 4`, `SLOT_CAPACITY = 4_149_248`, `SLOT_STRIDE = 4_153_344`, `TOTAL_BYTES = 16_617_472`; fourccs; states; idle reasons)
- Create: `vcam/protocol/src/writer.rs`, `vcam/protocol/src/reader.rs`. Both work on a raw memory region, so the tests run on plain memory on any OS.
- Create: `vcam/protocol/include/clippiboy_vcam.h` (the C mirror, with `_Static_assert(offsetof(...))` for every field)
- Create: `vcam/protocol/tests/layout.rs`, `vcam/protocol/tests/seqlock.rs`, and a fuzz loop (`cargo fuzz`, or a seeded random loop without new dependencies — decide when expanding)
- Modify: `src-tauri/Cargo.toml` (path dependency on `../vcam/protocol`)

**Interfaces:**
- `pub struct Writer { .. }` with `unsafe fn new(region: *mut u8, len: usize)`, `fn init(&mut self, pid: u32, session: u32, preferred: Format)`, `fn publish(&mut self, frame: &FrameRef) -> u64` (seqlock to odd, write, to even, then `latest_frame`, heartbeat, and the caller sets the event), `fn set_state(&self, state: State, reason: IdleReason)`, `fn heartbeat(&self, now_100ns: i64)`
- `pub struct Reader { .. }` with `fn read_latest(&self, out: &mut Vec<u8>) -> ReadResult`, where `ReadResult = Frame(FrameMeta) | Idle(IdleReason) | NotRunning | Torn`
- `pub fn validate(header: &VcamHeader, len: usize) -> Result<(), ProtocolError>`
- `pub fn is_stale(heartbeat_100ns: i64, now_100ns: i64) -> bool` (older than 1 s, spec 4.6)

**Tests:**
- `the_layout_matches_the_documented_offsets` (`core::mem::offset_of!` for every field against the §7.3 table)
- `the_c_header_agrees_with_rust` (compiled with the `cc` crate in a test build script on Windows CI)
- `a_torn_slot_is_never_accepted` (a writer thread hammering, and a reader checking a per-frame checksum over 100 000 reads)
- `a_reader_falls_back_to_the_previous_frame`
- `another_major_version_is_refused`, `a_newer_minor_version_is_read`
- `a_stale_heartbeat_shows_not_running`
- `garbage_headers_never_panic_or_read_out_of_bounds` (`payload_bytes` above capacity, plane offsets past the slot, zero sizes)

**Risks:** copying memory that another process writes is outside Rust's memory model. Copy through raw pointers into a private buffer and validate with the seqlock afterwards, in one small `unsafe` module with the reasoning in its comments.

**Done when:** both sides use one definition, the C header and Rust agree byte for byte, and the fuzz loop runs clean.

---

### Task 3: The producer in ClippiBoy

**Files:**
- Create: `src-tauri/src/vcam/mod.rs` (platform-neutral: the state, and the pure decisions)
- Create: `src-tauri/src/vcam/windows.rs`
  - the section from `CreateFileMappingW` (pagefile-backed) and the `.Frame` auto-reset event, both with the §7.3 security descriptors built by `ConvertStringSecurityDescriptorToSecurityDescriptorW`;
  - `MFCreateVirtualCamera(SoftwareCameraSource, Session, CurrentUser, "ClippiBoy Camera", "{CLSID}", None)` and `Start(callback)` on a background MTA thread; SOURCE_START and SOURCE_STOP from the callback;
  - a heartbeat at least every 250 ms; `Shutdown()` on exit (`lib.rs:1410`, `RunEvent::Exit`);
  - `%LOCALAPPDATA%\gg.clippiboy.app\vcam.json` with the preferred format, for Windows 10.
- Modify: `src-tauri/Cargo.toml` (`windows` features: `Win32_Security_Authorization`; check by compiling that `MFCreateVirtualCamera` is behind `Win32_Media_MediaFoundation`)
- Modify: the camera module (stage 2)
  - `camera/mod.rs`: `OpenMode` gets `Controlling(Format)` next to stage 2's shared modes, with the format choice of spec 4.1 (720p30 or 1080p30 MJPEG);
  - `camera/windows.rs`: the controlling open, with the chosen native type, decoded to NV12 in system memory as stage 2's shared open does, so `camera::Frame` stays the same;
  - the producer takes every `camera::Frame` at the camera's own size, *before* the GPU scaler, in the cam worker's loop next to `latest_frame` and `preview.offer`; downscaled only above 1080p. Frames are published as NV12; the DLL converts to YUY2 for clients that negotiate it (Task 4).
- Modify: `src-tauri/src/camtrack.rs`: `CamWants` gets `vcam_consumer: bool`, and `cam_wanted` adds it (spec 2.2: "a virtual-camera consumer is attached"); `CamPlan` carries the open mode from `camera_mode`.
- Modify: `src-tauri/src/state.rs` (`replace_config` 419-457 and `reconcile_cam`: `virtual_camera` on and off; the master switch off sets IDLE / CAM_OFF and closes the real camera, while the section stays)
- Modify: `src-tauri/src/lib.rs` (start the producer at setup when the setting is on and the DLL is installed; the 5 s health line, `lib.rs:1091-1103` next to stage 2's `CamTrack::health_line`, gets consumers, frames published and the state)

**Interfaces:**
- `pub struct VcamProducer` with `fn start(preferred: Format) -> Result<Self, String>`, `fn publish(&self, frame: &camera::Frame)`, `fn set_idle(&self, reason: IdleReason)`, `fn consumer_active(&self) -> bool`
- `pub enum VcamStatus { NotInstalled, NeedsUpdate { installed: String, bundled: String }, Unsupported, Off, Ready, Live { consumers: u32 }, Error(String) }`
- `pub fn camera_mode(vcam_on: bool, other_app_streams_real_camera: bool, shared: OpenMode) -> OpenMode` (pure; `shared` is the mode stage 1 picked, by default `MfSensorShared`)

**Tests:**
- `the_camera_opens_controlling_only_while_the_virtual_camera_is_on`
- `the_real_camera_stays_shared_while_another_app_streams_it`: when stage 2's `camera::windows::holder` (`IMFSensorActivityMonitor`) names another process on the real camera, stay in sharing mode and show "Discord still uses your camera directly — pick ClippiBoy Camera there" (design call).
- `cam_off_publishes_idle_not_frames`
- `the_master_switch_off_closes_the_real_camera_but_keeps_the_section`
- `frames_above_1080p_are_scaled_down`
- `a_second_start_reuses_the_section` (`ERROR_ALREADY_EXISTS`: validate magic and size, bump `generation`)

**Risks:**
- Spec 2.2's "with the master switch off nothing of it exists" is about the `CamTrack`. The producer has to stay, or the consumer cannot show "Camera is off in ClippiBoy" (4.2). Keep the two apart.
- `E_ACCESSDENIED` from the create call means camera privacy is off for desktop apps (`camera::Error::Denied`, `CamState::Blocked`); show the same hint as 2.8.
- A 1080p YUY2 copy is about 4 MB per frame (120 MB/s), so frames are copied only between SOURCE_START and SOURCE_STOP.

**Done when:** the spike's sample source (or Task 4's DLL) shows ClippiBoy's live frames, and SOURCE_START and SOURCE_STOP open and close the real camera.

---

### Task 4: The Windows 11 media source DLL

**Files:**
- Create: `vcam/Cargo.toml` (a workspace of its own: `protocol`, `mf-source`, `setup`; `[profile.release] panic = "unwind"`, `lto = true`), `vcam/.cargo/config.toml` (`-C target-feature=+crt-static`; without the static CRT the DLL fails with `MOD_NOT_FOUND`)
- Create: `vcam/mf-source/Cargo.toml` (`crate-type = ["cdylib"]`; `windows` 0.58 with `implement`, `Win32_Media_MediaFoundation`, `Win32_Media_KernelStreaming`, `Win32_System_Com`, `Win32_System_Threading`, `Win32_System_RemoteDesktop`, `Win32_Security`, `Win32_System_Memory`)
- Create, in `vcam/mf-source/src/`:
  - `lib.rs`: the exports `DllGetClassObject`, `DllCanUnloadNow`, and `DllRegisterServer`/`DllUnregisterServer` (the helper writes the registry);
  - `guard.rs`: the `catch_unwind` wrapper;
  - `activate.rs`: `IMFActivate`, with `IMFAttributes` forwarded to an inner store; `DetachObject`/`ShutdownObject` return `E_NOTIMPL`;
  - `source.rs`: `IMFMediaSourceEx`, `IMFGetService` (`MF_E_UNSUPPORTED_SERVICE`), `IKsControl` (`ERROR_SET_NOT_FOUND`), event queues from `MFCreateEventQueue`, `Start` posting `MESourceStarted`, `MENewStream`, `MEStreamStarted`; `Pause` returns `MF_E_INVALID_STATE_TRANSITION`;
  - `stream.rs`: `IMFMediaStream2`; static media types NV12 and YUY2 at 1280×720, 1920×1080 and 640×480, 30 fps; `PINNAME_VIDEO_CAPTURE`, stream id 0, `FRAMESOURCE_TYPES = Color`, `FRAMESERVER_SHARED = 1`; `RequestSample` only stores the token;
  - `pacer.rs`: a high-resolution waitable timer plus the frame event; it sends the newest frame or repeats the last one; sample time = `capture_100ns`, kept monotonic; placeholders use `MFGetSystemTime()`;
  - `ipc.rs`: host detection (session 0, LocalService); the session from `WTSGetActiveConsoleSessionId`, else `WTSEnumerateSessionsW`; opens `Session\<id>\…` read-only and retries every second;
  - `convert.rs`: NV12↔YUY2, scaling, the colour matrix (pure);
  - `placeholder.rs`: pre-baked pictures for "ClippiBoy is not running", "Camera is off in ClippiBoy", "No camera", "Camera is busy";
  - `log.rs`: `OutputDebugString` plus a capped file in the host account's profile.

**Interfaces:**
- `pub const CLSID_CLIPPIBOY_VCAM: GUID`
- `fn guarded<T>(f: impl FnOnce() -> windows::core::Result<T>) -> windows::core::Result<T>`
- `fn convert(meta: &FrameMeta, src: &[u8], to: Format, out: &mut [u8])`
- `enum Picture { Live, NotRunning, CamOff, NoCamera, Busy }` and `fn picture_for(read: &ReadResult) -> Picture` (pure)

**Tests** (`cargo test` in `vcam/` on Windows):
- `every_com_method_turns_a_panic_into_an_hresult`
- `null_arguments_return_e_pointer` (the Rust source crash of Windows-Camera #41)
- `nv12_to_yuy2_round_trips_a_test_pattern`
- `scaling_letterboxes_to_the_negotiated_size`
- `the_media_types_do_not_depend_on_the_producer` (listing them without ClippiBoy gives the same list; Frame Server queries them during sensor-group generation)
- `sample_times_never_go_backwards`
- `picture_for_maps_every_reader_state`
- Manual: with the DLL registered by hand as admin, Discord, Chrome and the Camera app show the frames, and the Frame Server event log stays clean.

**Risks:**
- A crash in the source kills Frame Server and every camera (§7.6): the guards, no allocation per frame in the pacer, the fuzzed reader.
- Double `Release` (VCamSample #4): let windows-rs do the reference counting, and avoid raw `from_raw`.
- Lock order source before stream; never hold a lock while calling `GetEvent`.
- Differences between the 0.58 and 0.62 signatures (Task 1 item 10).

**Done when:** the hand-registered DLL serves ClippiBoy's frames to Discord, Chrome and the Camera app, and a test build that forces a panic returns an error instead of killing Frame Server.

---

### Task 5: The elevated helper, signing, bundling, CI

**Files:**
- Create: `vcam/setup/` (a Rust binary of about 300 lines: `install`, `uninstall`, `status`), `vcam/setup/setup.manifest` (`requireAdministrator`, embedded by a build script, e.g. `embed-resource`), `include_bytes!` of the release DLL(s), so the DLLs are built first.
- Modify: `src-tauri/tauri.conf.json` (ship `clippiboy-vcam-setup.exe` next to the app as a sidecar or resource)
- Modify: `.github/workflows/release.yml` (build `vcam/`, sign the DLLs, build and sign the helper, then the app; secrets per Open for Fabi 4)
- Modify: `.github/workflows/ci.yml` (build and test `vcam/` on `windows-latest`)
- Create: `THIRD-PARTY-NOTICES.txt` (shipped with the installer; the repo has none today, research §8)
- Modify: `src-tauri/src/vcam/windows.rs` (run the helper with `ShellExecuteExW` "runas" and wait; read `HKLM\SOFTWARE\ClippiBoy\VirtualCamera`)

**Install:**
1. Check the Windows build: 22000 or later uses the MF source; earlier uses the DirectShow filter (group 9), or refuses.
2. Write the DLL to `%ProgramFiles%\ClippiBoy Virtual Camera\<version>\`, with an explicit read-and-execute entry for LOCAL SERVICE.
3. Write `HKLM\SOFTWARE\Classes\CLSID\{…}\InprocServer32` (default value = path, `ThreadingModel=Both`).
4. Write `HKLM\SOFTWARE\ClippiBoy\VirtualCamera` (`Version`, `Kind`, `Path`, and `Users`, the SIDs that installed it).
5. Put a copy of the helper in the same folder, plus an entry under *Installed apps*, so the component can be removed even after ClippiBoy is gone (design call).
6. Delete old version folders, or mark them for deletion at reboot (`MoveFileEx`, delay until reboot).
7. Never restart Frame Server unless asked, because that drops live calls.

**Uninstall:** the reverse, but only once the last SID has left `Users` (one DLL serves every Windows user).

**Interfaces:** `clippiboy-vcam-setup.exe install | uninstall | status [--user-sid <sid>]`, with exit codes 0 = done, 2 = refused, 3 = locked (needs a reboot), 4 = unsupported build.

**Tests:**
- `install_refuses_a_path_outside_program_files`
- `install_and_uninstall_mirror_each_other` (a pure plan of registry and file steps)
- `an_older_version_folder_is_removed_or_marked_for_reboot`
- `uninstall_keeps_the_camera_while_another_user_still_has_it`
- `the_app_asks_for_uac_only_when_the_bundled_version_is_newer`

**Risks:**
- Open for Fabi 4: the certificate (RSA recommended); Smart App Control blocks unsigned DLLs.
- The helper itself sits in the per-user install folder, which the user can write to. Another process of the same user could swap it, and UAC would then show "Unknown publisher". That is acceptable for a per-user app, but the app's text before the prompt names the publisher to look for.

**Done when:** on a clean Windows 11 VM, the signed helper installs and removes the DLL with one UAC prompt, and an app update without a new DLL version asks for nothing.

---

### Task 6: NSIS uninstall hook

**Files:**
- Modify: `src-tauri/windows/hooks.nsh`. A new `!macro NSIS_HOOK_PREUNINSTALL`: if `HKLM\SOFTWARE\ClippiBoy\VirtualCamera` exists, run the helper's `uninstall --user-sid …` through `runas` and wait. If the user declines, print one line saying the camera can be removed under *Installed apps*. Plain ASCII, commented like the existing macros.

**Tests** (manual, on a VM):
- uninstall with the camera installed: UAC, then no camera, no folder, no key;
- decline: the app is gone, the component is still listed and can be removed;
- an app update through the passive updater: no prompt. **Verify** whether Tauri's update runs the old uninstaller; if it does, the hook must recognise the update case.

**Done when:** uninstalling ClippiBoy leaves no ClippiBoy Camera behind, and an update never prompts.

---

### Task 7: The Facecam tab — the switch and its states (spec 5.2 item 6)

**Files:**
- Modify: `src/routes/settings/Facecam.tsx` (stage 2): section 6 "Virtual camera" with the switch, one line on what it does, one on the administrator rights, a status line, and *Remove*
- Modify: `src/lib/types.ts`, `src/lib/ipc.ts`, `src/store.ts`
- Modify: `src-tauri/src/commands.rs` (new `vcam_status() -> VcamStatus` and `set_virtual_camera(on: bool)`, which writes `facecam.virtual_camera` through `replace_config` like stage 2's `set_facecam_enabled`); registered in `generate_handler!`, `build.rs` and `capabilities/default.json` only, not in `capabilities/console.json`

**Copy:**
- Label: "Virtual camera".
- Hint: "Lets Discord and other apps use your camera while ClippiBoy has it. Pick “ClippiBoy Camera” in their settings once. Needs administrator rights once."
- Status lines: "Not installed" / "Installing…" / "Ready — pick ClippiBoy Camera in Discord" / "In use by 1 app" / "Needs an update (asks for administrator rights)" / "Not available on this version of Windows".

**Tests:**
- `switching_on_without_the_dll_installs_it_first`
- `switching_off_removes_the_camera_but_keeps_the_dll`
- `the_console_cannot_switch_the_virtual_camera`

**Done when:** the switch works end to end on a clean machine, and every state has its line.

---

### Task 8: Updates and protocol compatibility

**Files:** `src-tauri/src/vcam/windows.rs` (compares the HKLM version with the bundled one and prompts in the app only), and `vcam/protocol` (the version rules written next to the constants).

**Tests:**
- `a_newer_app_works_with_an_older_dll_of_the_same_major`
- `a_different_major_asks_for_the_update`
- `the_updater_never_starts_the_helper`

**Done when:** an app update with an unchanged DLL keeps working without a UAC prompt.

---

### Task group 9 (optional — only if Fabi chooses (b) for Open for Fabi 5): Windows 10 DirectShow filter

**9.1 The filter**
- Create: `vcam/ds-filter/` (C++17; the DirectShow BaseClasses vendored under `vcam/ds-filter/baseclasses/` with their MIT notice; optionally softcam (MIT) as the skeleton).
  - `CSource` and `CSourceStream`; one output pin with NV12, YUY2 and I420 at 1280×720, 1920×1080 and 640×480, 30 fps, through `IAMStreamConfig::GetStreamCaps` (which libwebrtc reads); `IKsPropertySet` with `PIN_CATEGORY_CAPTURE`;
  - it reads the protocol through Task 2's C header from `Local\ClippiBoy.VCam.v1` (same session, user DACL) and writes `reader_heartbeat_100ns`;
  - placeholder at the format from `vcam.json` while ClippiBoy is not running;
  - built with the `cc` crate from the `vcam` workspace (or MSBuild in CI), `/MT`, x64 (x86 only if spike item 6 or a user asks for it).

**9.2 The producer side:** copy frames only while the reader heartbeat is fresh (under 2 s), and keep `vcam.json` up to date.

**9.3 The helper:** on builds before 22000, register the filter (the HKLM CLSID plus `IFilterMapper2::RegisterFilter` in `CLSID_VideoInputDeviceCategory` with `MERIT_DO_NOT_USE`). Never on Windows 11.

**Tests:** a small C++ test executable for format negotiation and conversion; Task 2's header test; the manual matrix of Discord, Chrome, Edge, Zoom and OBS on Windows 10 22H2.

**Risks:** GPL sources must not be copied. The filter runs inside every consumer, so a crash kills Discord (not Frame Server): guard it the same way.

**Done when:** Discord on Windows 10 shows the camera through ClippiBoy Camera, and "ClippiBoy is not running" without the app.

---

### Task 10: Compatibility matrix and soak test

**Matrix:** Discord (preview **and** remote side), Chrome, Edge, new Teams (remote side), Zoom, OBS (DirectShow route), Firefox and the Camera app; on Windows 11, plus Windows 10 if group 9 was built; with the C920 and the eMeet.

Each cell records:
- listed or not, picture, chosen format, remote side;
- ClippiBoy quits: "device invalidated", then the real camera works;
- the master switch goes off mid-call: the "Camera is off" picture;
- the real camera is unplugged;
- sleep and resume;
- ClippiBoy is killed: still listed with "not running" until the next start.

**Soak:** a four-hour Discord call with the buffer running and two clips saved every 10 minutes. Watch Frame Server (Event Viewer, svchost restarts), the memory of ClippiBoy and Frame Server, and the frame rate on the remote side.

**Files:** the results go into the spec (section 4) and the README's feature list.

**Done when:** every cell is filled in, and the soak test ends without a Frame Server restart.
