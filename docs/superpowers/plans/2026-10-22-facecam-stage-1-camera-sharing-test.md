# Facecam Stage 1: Camera Sharing Test Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Tasks 1, 4, 5 and 6 need a person in front of a Windows machine with a webcam and Discord; an agent can do Tasks 2 and 3 and fill in Task 7 from the results.

**Goal:** Find out on real hardware whether Discord keeps the webcam while another program reads it, which camera API ClippiBoy must use, which clock and format the frames come with, and what decoding costs — so that the facecam's Open-for-Fabi items 3, 5 and 11 can be decided on evidence.

**Architecture:** Phase A uses programs that are already on the machine as stand-ins: Windows' own Settings camera preview is a sharing-mode client; the Camera app, browsers and OBS are controlling clients; Discord is the unknown. Phase B is a throwaway console probe in `scratch/cam-probe/` (never committed) that opens the camera every way the spec considers and logs, once per second, frames, timestamps, the camera's real format, decoder and errors. The results go into the spec's section "Result of the stage-1 test".

**Tech Stack:** Windows 11 26200 (+ one Windows 10 22H2 machine), Discord desktop and Discord in the browser, OBS, Chrome or Edge, Process Explorer; Rust with `windows` 0.58 and `windows-core` 0.58 (the versions in `src-tauri/Cargo.toml`) for the probe.

**Spec:** `docs/superpowers/specs/2026-10-20-facecam-design.md` (section 1 "The risk that comes first", "What the research changed" items 1–3, 2.2, 2.5, Open for Fabi 3, 5, 11, 13). Background: `docs/extras/research/camera-sharing.md` (sections 7–8) and `docs/extras/research/camera-capture-and-clocks.md` (sections 2.4, 3, 4, 5, 8, 10).

**Time:** about a day and a half for everything. If time is short, the minimum set is: Task 4 row 0 (negative controls), row 1 for the C920 in bright light, both open orders, with the best sharing mode; Task 1 step 3 (Discord's API); Task 4 row 4 (`prime`). That already decides Open for Fabi 3.

## Global Constraints

- The probe lives in `scratch/cam-probe/` and is **never committed**. Put `scratch/` into `.git/info/exclude` before creating it (not `.gitignore`, which is in the repo).
- Only results are committed: the spec section "Result of the stage-1 test", the evidence lines under the affected Open-for-Fabi items, and section 2.2 "API". The decisions themselves stay Fabi's. Commits in English, without a `Co-Authored-By` line.
- The probe never changes camera controls (exposure, low-light compensation, focus): in sharing mode that would change Discord's picture too.
- Vendor camera software (Logi Tune, G HUB, eMeet Studio) is closed during every run, or listed as one more client.
- **Clean state before every cell:** `cam-probe monitor` shows no stream on any camera; Discord is quit from the tray (not just closed), the browser tab with the camera is closed, OBS is closed; the camera Discord has selected is the one under test; the other camera is unplugged.
- **Light:** *bright* = room light plus a desk lamp on the face (about 80 lux or more, Microsoft's figure for 30 fps); *dim* = only the monitor lit (about 20 lux, Microsoft's figure for ≥ 14 fps). Same lamp position for every run.
- The C920 needs MJPEG for 720p30 over USB 2.0 (YUY2 720p tops out around 10 fps); every run logs the camera's **native** format, not only what the probe receives.
- Discord "works" means: its own preview **and** the remote side of a real call show the picture (a second account or a phone in the call). Two things are judged separately in every cell: *does it share* (both get frames) and *is the format good* (720p or more at 25 fps or more in bright light).
- Windows 10 is tested on real hardware or with the *physical* camera passed through to a VM, never a hypervisor's virtual webcam. The probe for a tester is built with `-C target-feature=+crt-static` so it runs without the VC++ runtime.
- `--log-raw` is on for every Task 4 row-1 run.

## Review Focus

- Discord's preview looks fine while the remote side is black or frozen → every Discord cell is checked from the remote side too (Task 1 step 2, Task 4).
- A format change by Discord in the middle (it starts its call at another size) freezes or kills the sharing reader → the probe logs every format change and the frame count after it, and dumps the first frame after it as PNG (Task 3 steps 1 and 3; Task 4 row 2 "Discord toggles video").
- Dim light halves the frame rate and the timestamps get irregular → Task 4 row 1 runs every cell bright and dim.
- MJPEG decoded in sharing mode gives null or garbage frames on some hardware → PNGs every ~10 s and after each format change, checked by eye (Task 3 step 3); `--out yuy2|bgra8` as the way around it (Task 2 step 5).
- The probe opened "the plain way" must make Discord fail — otherwise the setup does not see conflicts at all → Task 4 row 0, run **first**.

---

### Task 1: Phase A — zero-code stand-ins on Windows 11

**Files:** none (results go into `scratch/stage1-results.md`, from the template in Task 3 step 9, and into the spec in Task 7)

Clients:

| Code | Client | Mode |
|---|---|---|
| S | Settings › Bluetooth & devices › Cameras › (camera): the preview | sharing |
| C | Windows Camera app | controlling (WinRT) |
| B | Chrome or Edge on a webcam test page (`getUserMedia`) | controlling (MF) |
| O | OBS, Video Capture Device, "Deactivate when not showing" off | controlling (DirectShow) |
| D | Discord desktop: Settings › Voice & Video › Test Video, then a real call | unknown |
| Dw | Discord in Chrome or Edge, same call | controlling (MF, like B) |

- [ ] **Step 1: Record the setup** in `scratch/stage1-results.md`: `winver` full build (e.g. 26200.xxxx) and edition; whether *Advanced camera options* shows "Allow multiple apps to use camera at the same time" and its state; per camera the printed model, the USB hardware id (Device Manager › Details › Hardware Ids; the C920 is 046d:082d or 046d:08e5), driver (inbox `usbvideo.sys` or vendor), USB port; Discord, Chrome/Edge and OBS versions; camera privacy on, desktop apps on.
- [ ] **Step 2: Run the sequence per camera** (C920, then eMeet C960), multi-app switch **off**, both windows visible, 10 s per step, clean state between rows (Global Constraints):

| # | Order | Expected if the theory holds | Tells you |
|---|---|---|---|
| A1 | D alone | works | baseline |
| A2 | **S, then D** | D works, S keeps running | the "ClippiBoy first" case with a sharing reader |
| A3 | **D, then S** | S shows video if D is MF; S black/frozen if D is DirectShow | the "Discord first" case |
| A3w | Dw, then S | S shows video | separates "DirectShow" from "Discord"; Discord in the browser is the user's workaround |
| A4 | C then D; D then C | the second one fails | Discord is a controlling client |
| A5 | O then S; S then O | O→S: S black | how DirectShow treats sharing readers here |
| A6 | B then S; S then B | both work | MF controller next to a sharing reader |

Per step write: works / fails / black / frozen, the exact error text, whether the first app keeps running or changes resolution, the LED (a blink means the camera was renegotiated).
- [ ] **Step 3: Discord's capture API (A7).** In Process Explorer (View › Lower Pane › DLLs), find the Discord process that loads `discord_voice*.node`. Save its DLL list (File › Save) with Discord idle, start a video test, save it again, and diff. Only a new `ksproxy.ax` (DirectShow) or a new `mfsensorgroup.dll` / `mfcore.dll` capture path in **that** process counts as evidence — Electron loads Media Foundation for playback anyway. While the video runs, look at that process's threads (Properties › Threads › Stack) for `qcap`/`ksproxy` (DirectShow) or `mfreadwrite`/`mfsensorgroup` (MF) frames. Write down both lists.
- [ ] **Step 4: Repeat A2 and A3 in dim light.**
- [ ] **Step 5: If the multi-app switch exists**, turn it on and repeat A2, A3 and A5 (A8). Note which format Settings forces.
- [ ] **Step 6: Commit nothing.** The table stays in `scratch/` until Task 7.

---

### Task 2: The probe — device list and the ways to open a camera

**Files:**
- Create: `scratch/cam-probe/Cargo.toml`
- Create: `scratch/cam-probe/src/main.rs` (argument parsing, the per-second log loop, the JSON report)
- Create: `scratch/cam-probe/src/open.rs` (the open modes; `#[cfg(windows)]`)
- Create: `scratch/cam-probe/src/stamp.rs` (timestamp choice and statistics, pure, no `windows` imports)

`Cargo.toml`: `serde`, `serde_json`, `png = "0.17"` as normal dependencies; under `[target.'cfg(windows)'.dependencies]` `windows-core = "0.58"` and `windows = "0.58"` with the features `implement`, `Win32_Foundation`, `Win32_System_Com`, `Win32_System_Com_StructuredStorage`, `Win32_System_Variant`, `Win32_System_Threading`, `Win32_System_Performance`, `Win32_System_SystemInformation`, `Wdk_System_SystemServices`, `Win32_System_WinRT`, `Win32_Media_MediaFoundation`, `Win32_Media_KernelStreaming`, `Foundation`, `Foundation_Collections`, `Devices_Enumeration`, `Media_Capture`, `Media_Capture_Frames`, `Media_MediaProperties`, `Media_Devices`, `Graphics_Imaging`. `#[implement]` expands to `::windows_core`, hence the second crate. Feature names follow windows-rs's namespace rule; `cargo check` settles any that differ. With `windows` only under the Windows target and `open.rs` behind `#[cfg(windows)]`, `cargo test` runs the `stamp.rs` tests on any OS.

**Interfaces:**
- Produces (stage 2's `camera` module reuses these names):
  - `enum OpenMode { MfSensorShared, MfShareAttr, WinrtShared, MfPlain, WinrtExclusive }`
  - `struct ProbeFrame { qpc_100ns: i64, clock: ClockKind, sample_time_100ns: Option<i64>, device_ts_100ns: Option<i64>, device_ref_ts_100ns: Option<i64>, arrival_100ns: i64, subtype: [u8; 4], width: u32, height: u32, bytes: usize }` — `subtype`/`width`/`height` are what the probe *receives*; the camera's native format is logged separately (Task 3 step 1).
  - `enum ClockKind { Device, DeviceReference, SampleTime, Arrival }`
  - `fn choose_stamp(device: Option<i64>, device_ref: Option<i64>, sample: Option<i64>, arrival: i64) -> (i64, ClockKind)` — the first of `device`, `device_ref`, `sample` (in that order) with `|value − arrival| ≤ 20_000_000` (±2 s inclusive, the bound the audio code's `usable_stamp` uses); else `(arrival, Arrival)`. WinRT's `SystemRelativeTime` goes into the `device` slot (it can be null).
  - `fn gap_stats(gaps_ms: &[f64]) -> Option<(f64, f64, f64)>` — (min, avg, max); the gaps are between consecutive **chosen** stamps, including the gap from the previous second's last frame.
  - `struct Second { frames: u32, min_gap_ms: f64, avg_gap_ms: f64, max_gap_ms: f64, latency_median_ms: f64, latency_max_ms: f64, native: String, received: String, events: Vec<String> }` — latency is per frame `arrival − stamp`.

- [ ] **Step 1: Write the failing tests** in `stamp.rs` (`#[cfg(test)]`; `a = 36_000_000_000`, a QPC of an hour):
  - `a_device_stamp_half_a_second_old_is_used`: `choose_stamp(Some(a), None, None, a + 5_000_000) == (a, Device)`
  - `a_stamp_three_seconds_off_falls_back_to_arrival`: `choose_stamp(Some(a), None, None, a + 30_000_000) == (a + 30_000_000, Arrival)`
  - `a_stream_relative_sample_time_is_not_taken`: `choose_stamp(None, None, Some(1_000), a) == (a, Arrival)`
  - `the_device_reference_is_second_choice`: `choose_stamp(Some(a - 30_000_000), Some(a - 1), None, a) == (a - 1, DeviceReference)`
  - `the_device_stamp_wins_when_all_are_plausible`: `choose_stamp(Some(a - 3), Some(a - 2), Some(a - 1), a) == (a - 3, Device)`
  - `the_sample_time_is_used_when_it_is_the_only_plausible_one`: `choose_stamp(None, Some(5), Some(a - 7), a) == (a - 7, SampleTime)`
  - `two_seconds_exactly_is_still_plausible`: `choose_stamp(Some(a - 20_000_000), None, None, a) == (a - 20_000_000, Device)`; `choose_stamp(Some(a - 20_000_001), None, None, a).1 == Arrival`; the same two cases 2 s in the future.
  - `gap_statistics_over_three_gaps`: `gap_stats(&[33.3, 33.4, 66.7])` → min 33.3, max 66.7, avg within 0.001 of 44.4667; `gap_stats(&[])` → `None`.
- [ ] **Step 2: Run** `cargo test` in `scratch/cam-probe` — Expected: FAIL to compile, functions missing.
- [ ] **Step 3: Implement `stamp.rs`**, then `cargo test` — Expected: PASS.
- [ ] **Step 4: Implement `list`** (one MTA thread, `CoInitializeEx(COINIT_MULTITHREADED)`, `MFStartup` once):
  - Media Foundation: `MFEnumDeviceSources` with `MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID` (take each `IMFActivate` with `ptr::read`, then `CoTaskMemFree` the array), friendly name, symbolic link; log the interface GUID inside the link: `{e5323777-f976-4f5b-9b55-b94699c46e44}` is `KSCATEGORY_VIDEO_CAMERA` (sensor groups and sharing work), `{65e8773d-8f56-11d0-a3b9-00a0c9223196}` is the legacy capture category (they do not). All native media types per camera (subtype, size, frame rate from the packed `MF_MT_FRAME_SIZE`/`MF_MT_FRAME_RATE` u64s).
  - WinRT: `MediaFrameSourceGroup::FindAllAsync()`; match a group to an MF camera through `SourceInfos[i].DeviceInformation().Id()`, case-insensitively, not through the group's own id.
  - Print a table and write `list.json`.
- [ ] **Step 5: Implement the open modes** in `open.rs`, all on that MTA thread. Every mode takes `--out native|nv12|yuy2|bgra8` (default `nv12`) and, for `MfPlain` only, `--native MJPG:1280x720@30` (or any type from `list`) to put the camera into that format:
  - `MfSensorShared`: `MFCreateSensorGroup(link)` → `IMFSensorGroup::GetSensorDevice(0)` → `IMFSensorDevice::SetSensorDeviceMode(MFSensorDeviceMode_Shared)` → `IMFSensorGroup::CreateMediaSource` → `MFCreateSourceReaderFromMediaSource` with `MF_SOURCE_READER_ASYNC_CALLBACK` (an `#[implement(IMFSourceReaderCallback)]`) and `MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING = 1` (the reader inserts decoders on its own but colour converters only with this, so a YUY2 camera would fail without it), then `SetCurrentMediaType` with a partial type of the `--out` subtype.
  - `MfShareAttr`: `MFCreateDeviceSource` with `MF_DEVSOURCE_ATTRIBUTE_FRAMESERVER_SHARE_MODE = 1` on the attributes. The GUID is not in windows-rs; copy it from the SDK header on the test machine — `Select-String -Path "C:\Program Files (x86)\Windows Kits\10\Include\10.0.26100.0\um\mfidl.h" -Pattern FRAMESERVER_SHARE_MODE -Context 0,3` — into a `const GUID`. Windows 11 26100+ only: read the build with `RtlGetVersion` (`GetVersionEx` lies without a manifest) and refuse below 26100.
  - `WinrtShared`: `MediaCaptureInitializationSettings` with `SourceGroup`, `SharingMode = SharedReadOnly`, `StreamingCaptureMode = Video`, `MemoryPreference = Cpu` → `MediaCapture::InitializeWithSettingsAsync` → `CreateFrameReaderWithSubtypeAndSizeAsync(colorSource, <--out subtype>, 1280×720)` (a copy at a fixed size), set `AcquisitionMode = Buffered` **before** `StartAsync`, log `StartAsync`'s `MediaFrameReaderStartStatus`; subscribe to `FrameArrived`, `MediaFrameSource.FormatChanged`, `MediaCapture.CaptureDeviceExclusiveControlStatusChanged` and `MediaCapture.Failed` (the only place WinRT reports an unplug or a privacy block mid-stream).
  - `MfPlain`: `MFCreateDeviceSource` without the attribute (controlling) — negative control and the way to force a native format.
  - `WinrtExclusive`: like `WinrtShared` with `ExclusiveControl` — negative control; it can set a format with `MediaFrameSource::SetFormatAsync`.
  
  Every callback only copies what `ProbeFrame` needs (`MFSampleExtension_DeviceTimestamp`, `MFSampleExtension_DeviceReferenceSystemTime`, `GetSampleTime`; WinRT `SystemRelativeTime`) and releases the sample at once — the device source buffers one frame by default. For the MF modes log the decoder the reader inserted (`IMFSourceReaderEx::GetTransformForStream`, its friendly name), once at the start.
- [ ] **Step 6: Verify** `cargo build --release` (PowerShell) — Expected: builds; `cam-probe list` prints both cameras with their formats and the category GUID.

---

### Task 3: The probe — `read`, `monitor`, `prime` and the results template

**Files:**
- Modify: `scratch/cam-probe/src/main.rs`, `scratch/cam-probe/src/open.rs`

**Interfaces:**
- Consumes: `OpenMode`, `ProbeFrame`, `choose_stamp`, `gap_stats`, `Second` from Task 2.

- [ ] **Step 1: `read --mode <mode> --cam <n> --secs <s> --tag <cell-id> [--out …] [--native …] [--log-raw]`**: open, then once per second print and append one `Second` to `run-<tag>-<mode>-<cam>.json`: frames received, min/avg/max gap from the chosen stamps, per-frame latency (`arrival − stamp`) median and max, the camera's **native** format (MF: `GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_SOURCE_READER_CURRENT_TYPE_INDEX)` after every `CURRENTMEDIATYPECHANGED`; WinRT: `MediaFrameSource.CurrentFormat` after every `FormatChanged`), what the probe receives, and every event since the last line (format change, exclusive-control change, `MF_SOURCE_READERF_STREAMTICK`, `MediaCapture.Failed`, errors as hex HRESULT or `MediaFrameReaderStartStatus`). Once at the start: the winning `ClockKind`, the first frame's three raw stamps next to `arrival`, and the inserted decoder.
- [ ] **Step 2: Keep the camera open until Enter**, so Discord can be started and stopped next to it.
- [ ] **Step 3: PNGs:** every 300th frame (about every 10 s at 30 fps) and the first frame after each format change, converted on the CPU and written with `png`, to check by eye that the picture is not garbage. For MJPEG native types (`--out native`) log the compressed frame size, average and peak per second.
- [ ] **Step 4: `--log-raw`**: every frame's four timestamps into a CSV, for the clock question (does `GetSampleTime` start at zero? does `DeviceTimestamp` survive MF's MJPEG decoder? compare `--out native` with `--out nv12`).
- [ ] **Step 5: `monitor`**: `MFCreateSensorActivityMonitor` with an `#[implement(IMFSensorActivitiesReportCallback)]`; on every report print which process (PID and image name) streams which camera, and its `GetStreamingMode` (controlling or sharing). It runs in a second window during every Task 4–5 run and is the "clean state" check; it also shows whether Discord appears as a Frame Server client at all.
- [ ] **Step 6: `prime --cam <n> [--via winrt|mf]`**: WinRT: `WinrtExclusive`, `SetFormatAsync` to 1280×720 MJPEG 30 fps, start; open `WinrtShared`; then `StopAsync` and `MediaCapture.Close()` on the exclusive instance (dropping it is not enough — event handlers keep it alive); keep reading for 60 s. MF: the same with `MfPlain --native MJPG:1280x720@30`, then `MfSensorShared`, then release the plain reader. Logs whether the 720p30 native format survives the handover.
- [ ] **Step 7: CPU:** at the end of `read`, print the probe's process CPU time per second of wall time (`GetProcessTimes`). In sharing mode the Frame Server may decode MJPEG in its own `svchost`; note that process's CPU from Process Explorer (the `svchost` hosting the FrameServer service) during the same run.
- [ ] **Step 8: Verify** on one camera: `cam-probe read --mode winrt-shared --cam 0 --secs 20 --tag smoke` prints 20 lines with about 30 frames each in bright light, the native format, and writes two PNGs that show the room.
- [ ] **Step 9: Results template:** `scratch/stage1-results.md` with one table row per cell ID (`A1…A8`, `R0-…`, `R1-<cam>-<mode>-<order>-<light>`, `R2-…`, `R4-…`, `W10-…`) and columns: shares (Discord / probe), format good, native format seen, clock kind, error codes, notes. Every `read` run uses the cell ID as `--tag`.

---

### Task 4: Phase B — the matrix on Windows 11

**Files:** none (results into `scratch/stage1-results.md`)

`cam-probe monitor` runs in a second window throughout; clean state before every cell.

- [ ] **Step 1: Row 0 — negative controls, first.** For one camera: `mf-plain` first, then Discord → Discord must fail; `winrt-exclusive` first, then Discord → Discord must fail; Discord first, then `mf-plain` and then `winrt-exclusive` → the probe must fail. Record the exact HRESULT / `MediaFrameReaderStartStatus` of every failing open (stage 2's `classify` knows three codes: `0x80070005`, `0xC00D3704`, `0xC00D3EA2`; record any other code a failing open returns). If Discord does not fail in the first two, the conflict is invisible in this setup; stop and find out why before trusting anything else.
- [ ] **Step 2: Row 0b — alone.** Per camera × {`mf-sensor-shared`, `mf-share-attr`, `winrt-shared`}: the probe alone for 30 s — which native format does a sharing client get when nobody controls the camera? Then start Discord, wait 30 s, quit Discord while the probe runs: does the stream go on, and does the native format change back?
- [ ] **Step 3: Row 1 — sharing modes against Discord.** For C920 and C960 × {`mf-sensor-shared`, `mf-share-attr`, `winrt-shared`} × {probe first then Discord, Discord first then probe} × {bright, dim}: `read --secs 120 --log-raw`; Discord test video, then a real call checked from the remote side.
- [ ] **Step 4: Row 2 — robustness** (one camera, the best sharing mode from row 1):
  - probe running 30 min while Discord toggles video every 2 min;
  - Discord running while the probe stops and starts five times;
  - unplug and replug the camera during `read` (expect `0xC00D3EA2` or `MediaCapture.Failed`, then nothing until the probe reopens);
  - camera privacy for desktop apps switched off during `read`, then a new `read` (expect `E_ACCESSDENIED` on the open);
  - sleep and resume during `read` (log what arrives after wake, and the stamps across it).
- [ ] **Step 5: Row 3 — MJPEG decode cost:** `mf-plain --native MJPG:1280x720@30 --out nv12` and `--native MJPG:1920x1080@30 --out nv12` per camera, 60 s each, bright: probe CPU and decoder name; then `--out native` for the frame sizes.
- [ ] **Step 6: Row 4 — `prime`** on both cameras, `--via winrt` and `--via mf`, then Discord second.
- [ ] **Step 7: Row 5 — multi-app switch on** (if present): row 1 for one camera, bright only.
- [ ] **Step 8: Clock (from `--log-raw`)**: per camera and mode, which `ClockKind` won, whether `GetSampleTime` is QPC or starts at zero, whether `DeviceTimestamp` is still there behind the decoder, and the median and max latency.

Judge per cell: *shares* — Discord's preview and the remote side show the user, the probe keeps getting frames without a 2-second hole, the PNGs are correct, nothing freezes after a format change; *format good* — 720p or more at 25 fps or more in bright light. In "Discord first" cells the format is Discord's choice; there only *shares* counts.

---

### Task 5: Phase B — Windows 10

**Files:** none

- [ ] **Step 1:** On a Windows 10 22H2 machine (a tester with the `+crt-static` build, or the physical camera passed into a VM): Task 1 rows A1–A3, A5 and A7 with S replaced by `cam-probe read --mode winrt-shared` (Windows 10 has no sharing preview in Settings); Task 4 rows 0, 0b and 1 (bright only) for one camera with `mf-sensor-shared` and `winrt-shared` (`mf-share-attr` does not exist before 26100); Task 4 row 4 (`prime`).
- [ ] **Step 2:** Record the same fields as Task 1 step 1.

---

### Task 6: Read the results

**Files:** none

- [ ] **Step 1: Read the results against this table** (Windows 11, switch off, both cameras). The right-hand column is the evidence for an Open-for-Fabi item, not the decision itself:

| Result | Meaning | Evidence for |
|---|---|---|
| A2/A3 pass, and a probe sharing mode shares in both orders, dim light and 720p MJPEG included | Discord is MF/WinRT, or DirectShow does not block here | Open for Fabi 3 is moot: the camera source uses that mode; the virtual camera stays at stage 5 |
| A2 passes, A3 fails, probe "Discord first" gets no frames, A7 shows DirectShow | Discord holds the camera through DirectShow; Microsoft says nothing helps | Open for Fabi 3 is live: ClippiBoy loses the cam in every Discord call → recommendation (a), virtual camera right after stage 2, Windows 11 first |
| A2 fails (Discord cannot start while a sharing reader runs) | a sharing reader blocks a later DirectShow client | Open for Fabi 3 is live and worse: ClippiBoy would break Discord → recommendation (a), and until the virtual camera exists the master switch warns that Discord cannot use the camera while it is on |
| A3w passes where A3 fails | it is Discord desktop's DirectShow capture, not the camera | supports the above; "Discord in the browser" is the workaround to tell users |
| Settings (S) passes but no probe mode does | a probe bug | no decision; fix the probe first |
| The probe dies on a format change, but reopening works | — | stage 2 reopens on `FormatChanged` instead of reconfiguring |
| Only one camera passes | driver/firmware dependent | sharing mode as default; Open for Fabi 3 (a) for the failing kind: the virtual camera right after stage 2; ask more users with the probe |
| Passes only with the multi-app switch on | Windows' own sharing covers it | document the switch for Windows 11 24H2+; the virtual camera stays needed for Windows 10 and older builds |
| Windows 11 passes, Windows 10 fails | — | Open for Fabi 5 (Windows 10 virtual camera) |
| Probe alone (row 0b) already gets 720p or more at 25 fps or more | — | Open for Fabi 11 is moot |
| `prime` keeps 720p30 and Discord still starts | — | Open for Fabi 11 (a) works as described |
| Frames null/garbage while Discord controls with MJPEG | the MJPEG sharing path is broken on that machine | try `--out yuy2` / `--out bgra8`; if that fails, the virtual camera |

- [ ] **Step 2: The API for stage 2:** one path that passes on both Windows 11 and Windows 10 — the MF sensor group if it does (it keeps the code in the MF style of `mft.rs` and needs no WinRT features), else WinRT `SharedReadOnly` if that is the one that passes on both. Two backends behind the trait only if no single one passes on both systems.
- [ ] **Step 3: The clock rule:** a mode whose frames fall back to `Arrival` loses to one that keeps `Device`, other things equal.

---

### Task 7: Write the result into the spec

**Files:**
- Modify: `docs/superpowers/specs/2026-10-20-facecam-design.md` (section "Result of the stage-1 test"; under Open for Fabi 3, 5 and 11 one evidence line each; section 2.2 "API" with the chosen mode; 2.5 and 2.9 if the clock, latency or decode cost changed)

- [ ] **Step 1:** Replace *Pending* with: the setup (Task 1 step 1), the results table by cell ID, Discord's capture API (Task 1 step 3), the clock findings (Task 4 step 8), MJPEG decode CPU and frame sizes (Task 4 row 3), the format a sharing client gets alone (row 0b), Windows 10, and which rows of Task 6's table applied.
- [ ] **Step 2:** Add the evidence lines under Open for Fabi 3, 5 and 11, and update section 2.2's API. "Order" changes only after Fabi decided.
- [ ] **Step 3: Commit** `Facecam: result of the camera sharing test — <one line: e.g. "Discord shares with a sharing-mode reader on Windows 11; the virtual camera can stay at stage 5">`
