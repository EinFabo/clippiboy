# Facecam Stage 6: Linux Camera Backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. This plan is rough on purpose: before a task is executed, expand it into checkbox steps with superpowers:writing-plans, against the camera module as stage 2 left it. Tasks 4 and 6 need a Linux machine with the C920 and the eMeet.

**Goal:** The camera module gets a V4L2 backend behind `camera::Source`, and its platform-neutral parts (clock, grid, dropouts, format choice) are tested on Linux, so that a later Linux port of ClippiBoy finds a camera source that works. **This stage does not make ClippiBoy run on Linux**, and it says so.

**Architecture:**
- `src-tauri/src/camera/linux.rs` opens `/dev/video*` through V4L2 (`VIDIOC_QUERYCAP`, `VIDIOC_ENUM_FMT`, `VIDIOC_S_FMT`, mmap streaming). It stamps each frame with the buffer's `CLOCK_MONOTONIC` timestamp, converted to the module's 100 ns timeline and passed through stage 2's `camera::choose_stamp`, takes YUYV or decodes MJPEG on the CPU, and delivers the same `camera::Frame` the Windows backend delivers (tightly packed NV12 in system memory) to the `Sink` as `Event::Frame`.
- Hotplug comes through udev, or a short poll of `/dev/v4l/by-id`, behind a `watch_arrivals` of the same shape as stage 2's Windows one.
- The platform clock `now_100ns()` gets a Linux branch. Today it returns 0 everywhere except Windows (`audio/capture.rs:23-44`).

**Tech Stack:** Rust; V4L2 through a crate (`v4l`) or raw `libc` ioctls (decided in Task 3); a pure-Rust JPEG decoder (`zune-jpeg` or `jpeg-decoder`); optionally the `udev` crate. All of these go only under `[target.'cfg(target_os = "linux")'.dependencies]`.

**Spec:** `docs/superpowers/specs/2026-10-20-facecam-design.md`: building block 1 ("Later Linux: V4L2"), Order item 6, 2.2 (module layout, the stub, "later `linux.rs`"), 2.3 (grid and dropouts), 2.5 (clock). Background: `docs/extras/research/camera-capture-and-clocks.md` §9 (the `camera::Source` trait "so V4L2 can follow") and `docs/extras/research/camera-sharing.md:154-155` (the C920's formats on Linux, and its MJPEG-without-EOI quirk).

**From stage 2:** in `camera/mod.rs` the `Source` trait (`close`, `size`), the `list()`/`open(device, mode, sink)` dispatch, `Frame { probe, nv12 }` (tightly packed NV12: row length = `probe.width`, the UV plane right after `width × height` bytes; no stride, no texture, so it is reused as is), `Event` and `Sink`, `Error { Denied, Busy { by }, Gone, NoCamera, Unsupported, Other }`, `ClockKind` and `choose_stamp`, `pick_device`, `cover_crop`, `Backoff`, and the `#[cfg(not(windows))]` stub; `model::CameraDevice { id, name }`; the 30 fps grid in `camera/grid.rs` (`Grid::push`, `Grid::tick(now_100ns, suspended_100ns, out)`, `Out` incl. `Started` and `Reset`, `JUMP_100NS` for a new device clock in `push`, `SUSPEND_100NS` for a sleep in `tick`) with its filler and gap handling, and the cam worker's `suspended_since()` that feeds `tick` on Windows (QPC against `QueryUnbiasedInterruptTime`).

## What "Linux" can honestly mean here

ClippiBoy's core is built around Windows APIs:
- Four modules are Windows-only as a whole (`#![cfg(windows)]`): `gpu.rs` (D3D11), `mft.rs` (Media Foundation encoders), `wgc.rs` (Windows.Graphics.Capture) and `convert.rs` (the GPU converter and the watermark stamp).
- Fourteen more files carry 29 `#[cfg(not(windows))]` stubs that return "only available on Windows" or nothing: audio capture and devices, capture, clipboard, console, disk, encode, game, main, overlay, pipeline, shot, sounds and tray.
- `now_100ns()` is 0 off Windows. `tools.rs:36-40` fetches `ffmpeg.exe`. `muxer.rs:52-60` sets Windows process flags.

So on Linux nothing captures, encodes, records sound, takes screenshots or shows the overlay. Stage 2 relies on `cargo check` from WSL (its global constraints: the stub must compile), so the library type-checks on Linux. Whether `cargo test --lib` links and runs there is unclear: `README.md:729-735` says it runs without Windows, while `.github/workflows/ci.yml:24` says "Most of the core only compiles on Windows", and CI builds the app only on `windows-latest`. Task 1 settles which is true.

Stage 6 therefore delivers the camera module's Linux backend with its tests, ready for a port. The port itself is listed at the end and is not planned here.

## Global Constraints

- Commits in English, **without** a `Co-Authored-By` line.
- Nothing changes on Windows: the Windows backend, the cam track and CI stay as they are. `cargo test --lib` (PowerShell) stays green.
- `camera::Source` and the `list`/`open` dispatch are the only seam. No `cfg(target_os = "linux")` in the grid, the dropout handling or the cam track. Stage 2's `Frame` is already NV12 in system memory, so the payload needs no change.
- The backend never changes camera controls (exposure, focus, low-light compensation). This is the same rule as stages 1 and 2: on a shared camera it would change another app's picture.
- A Linux CI job for `cargo test --lib` is added only if Task 1 shows the library builds there, and only after Fabi agrees, because CI is shared.

## Review Focus

- **One clock:** V4L2 stamps buffers with `CLOCK_MONOTONIC`. `now_100ns()` on Linux must be that same clock, or every offset in 2.4 is wrong → Task 2 `now_and_a_v4l2_stamp_share_one_clock`, Task 4 on hardware.
- **Suspend:** stage 2's grid sees a sleep in `tick`, as `suspended_100ns > SUSPEND_100NS`, where `suspended` is how much further QPC ran than `QueryUnbiasedInterruptTime` (`JUMP_100NS` is only `push`'s test for a new device clock). On Linux `CLOCK_MONOTONIC` stops during suspend, so the stamps show no jump at all, and `suspended` has to come from `CLOCK_BOOTTIME` (which counts suspend) against `CLOCK_MONOTONIC` → Task 2 `a_suspend_shows_up_as_suspended_time`.
- **Exclusive device:** with uvcvideo, a second process that tries to stream usually gets `EBUSY` (unverified per camera). That brings the stage-1 question back on Linux → Task 6.
- **MJPEG frames without an end marker** (the C920 08e5 quirk) must drop one frame, not stop the stream → Task 4 `a_jpeg_without_eoi_is_dropped_not_fatal`.

---

### Task 1: What builds on Linux today

**Files:** possibly `README.md:729-735` or `.github/workflows/ci.yml:24` (whichever turns out to be wrong).

**Steps:** run `cargo check --lib` and `cargo test --lib` for `x86_64-unknown-linux-gnu` at the head of stage 2, with Tauri's Linux build dependencies installed (WebKitGTK 4.1 and friends; Tauri is a dependency even for `--lib`). List every error with its file. `cargo check` should already pass (stage 2 runs it from WSL); the question is the test build and run.

**Risks:** Tauri's system packages are needed before anything links. Errors may be only in the facecam modules stage 2 added (`camera`, `camtrack`, `facecam`, `composite`), so check them first.

**Done when:** it is known whether the library builds and its tests run on Linux, and the README and the CI comment say the same thing.

---

### Task 2: The platform clock, and the suspended time the grid needs

**Files:**
- Create: `src-tauri/src/clock.rs`, with `audio::capture::now_100ns` re-exported from it, so the camera does not depend on the audio module (design call; stage 2's camera code calls `crate::audio::capture::now_100ns()`, which keeps working).
- Modify: `src-tauri/src/audio/capture.rs:23-44` (Windows branch unchanged; Linux: `clock_gettime(CLOCK_MONOTONIC)` in 100 ns)
- In `clock.rs`: the Linux twin of the worker's `suspended_since()`: ΔCLOCK_BOOTTIME − ΔCLOCK_MONOTONIC since the last call, in 100 ns, for the `suspended_100ns` argument of `Grid::tick`. The grid itself stays as stage 2 left it.

**Interfaces:**
- `pub fn now_100ns() -> i64`
- `pub fn from_timeval(sec: i64, usec: i64) -> i64`
- `pub struct Suspended` with `fn since_last(&mut self) -> i64` (both clocks read once per call)

**Tests:**
- `now_is_monotonic_and_moves`
- `a_timeval_converts_to_100ns`
- `now_and_a_v4l2_stamp_share_one_clock` (a `CLOCK_MONOTONIC` timeval converted, compared with `now_100ns()`)
- `a_suspend_shows_up_as_suspended_time` (pure: two clock readings with BOOTTIME 5 s further than MONOTONIC → 50 000 000, and a grid tick with it gives `Reset`)
- `no_suspend_is_no_suspended_time` (both clocks moved alike → 0)
- The grid tests from stage 2 run unchanged on Linux.

**Risks:** a later Linux capture (PipeWire stamps with `CLOCK_MONOTONIC`) and audio must use the same clock. That is written down here for the port. `CLOCK_BOOTTIME` is `CLOCK_MONOTONIC` plus the time spent suspended, so their difference is the suspended time and nothing else.

**Done when:** on Linux the module's clock is the clock V4L2 stamps with, and a suspend reaches `Grid::tick` as suspended time, so the grid resets as it does on Windows.

---

### Task 3: V4L2 — list devices and open one

**Files:**
- Create: `src-tauri/src/camera/linux.rs` (`#[cfg(target_os = "linux")]`)
- Modify: `src-tauri/src/camera/mod.rs` (the `list`/`open` dispatch: Windows, Linux, or the stub)
- Modify: `src-tauri/Cargo.toml` (the Linux-only dependencies)

**Interfaces:**
- `pub fn list() -> Vec<CameraDevice>` and `pub fn open(device: &CameraDevice, mode: OpenMode, sink: Sink) -> Result<Box<dyn Source>, Error>` in `camera/linux.rs`, the same shape as stage 2's `camera/windows.rs`. V4L2 has no sharing mode, so `mode` is ignored there.
- `pub struct V4l2Source`, implementing stage 2's `camera::Source` (`close`, `size`); frames go to the `Sink` as `Event::Frame`, a size change as `Event::FormatChanged`, a loss as `Event::Lost(Error)`.
- `fn pick_format(formats: &[FormatDesc], want: (u32, u32, u32)) -> Option<FormatDesc>` (pure): MJPEG 1280×720 @ 30 first; then YUYV at the largest size that still gives at least 25 fps; then NV12 or anything else.
- `CameraDevice.id` is the `/dev/v4l/by-id/…` path, which survives a replug; `/dev/videoN` is the fallback. The card name is `CameraDevice.name`, so stage 2's `pick_device` finds it again through `FacecamConfig.device_name` (5.1).

**Tests:**
- `pick_format_prefers_mjpeg_720p30`
- `pick_format_takes_yuyv_when_mjpeg_is_missing`
- `metadata_nodes_are_not_listed` (uvcvideo creates a second node per camera, with `V4L2_CAP_META_CAPTURE` only)
- `a_by_id_path_maps_back_to_its_node`
- Manual: both cameras are listed with their formats.

**Risks:** device permissions (the user must be in the `video` group); the by-id names of cheap cameras without a serial number.

**Done when:** the backend lists both cameras with their formats and opens one at 720p30 MJPEG.

---

### Task 4: Frames — mmap streaming, timestamps, decoding, scaling

**Files:**
- Modify: `src-tauri/src/camera/linux.rs`. A capture thread:
  - `VIDIOC_REQBUFS` with 4 mmap buffers, then `QBUF`/`DQBUF` driven by `poll`;
  - the stamp from `v4l2_buffer.timestamp` when `V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC` is set, passed as the `device` stamp into stage 2's `camera::choose_stamp`, which checks it against the arrival time (±2 s) and otherwise gives the arrival time with `ClockKind::Arrival`;
  - MJPEG decoded to NV12 on the CPU, YUYV converted to NV12, both tightly packed as in stage 2's `Frame.nv12` (row length = width, no stride);
  - each buffer copied out and requeued at once.
- Create: a CPU cover-scaler in `camera/` (stage 2's GPU scaler is Windows-only; this one is pure and testable everywhere), taking its source rectangle from stage 2's `camera::cover_crop`

**Interfaces:**
- `fn v4l2_stamp(buffer_ts: i64, flags: u32) -> Option<i64>` (the `device` argument of `choose_stamp`; `None` without the monotonic flag)
- `fn decode_mjpeg(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), DecodeError>` (NV12)
- `fn yuyv_to_nv12(src: &[u8], width: u32, height: u32, out: &mut Vec<u8>)`
- `pub fn scale_nv12(frame: &Frame, crop: Rect, out: (u32, u32)) -> Vec<u8>`

**Tests:**
- `a_stamp_without_the_monotonic_flag_is_not_used` (the ±2 s fallback itself is stage 2's `choose_stamp` test)
- `a_jpeg_without_eoi_is_dropped_not_fatal`
- `yuyv_to_nv12_matches_a_known_pattern`
- `cover_scaling_a_4_3_frame_fills_16_9` (spec 2.2, cover-crop)
- On hardware: stage 2's `cargo run --example cam-source -- 0`, built for Linux: 20 s at 720p30 give about 600 frames, the median of `now − stamp` lies between 0 and 150 ms, and three PNG dumps (added to the example for this) show the room.

**Risks:** decoding MJPEG on the CPU at 720p30 costs about what MF's decoder costs on Windows (spec 2.9 estimates 10–25 % of a core); the probe measures it. The EOI quirk can hit the C920 08e5 at 1080p30.

**Done when:** the probe's numbers hold on both cameras in bright and in dim light.

---

### Task 5: Hotplug, errors, permissions

**Files:** `src-tauri/src/camera/linux.rs`
- `ENODEV` on `DQBUF` (unplugged), `EBUSY` on `S_FMT`, `REQBUFS` or `STREAMON` (taken), `EACCES` (not in the `video` group);
- arrival through a udev monitor or a 2 s poll of `/dev/v4l/by-id`, as `watch_arrivals(on_change) -> Option<ArrivalWatch>` like stage 2's Windows one;
- the backend only reports `Event::Lost(..)`; reopening with stage 2's `Backoff` and the filler frames and gaps stay in the cam track, as on Windows.

**Interfaces:** the errors map onto stage 2's `camera::Error`: `Gone`, `Busy { by: None }` (there is no activity monitor to name the holder), `Denied`, `Unsupported(..)`. The hint "Add yourself to the video group" belongs in the Facecam tab's line for a blocked camera on Linux, not in the error.

**Tests:**
- `enodev_maps_to_gone`
- `ebusy_maps_to_busy`
- `eacces_maps_to_denied`
- Manual: unplug and replug during a two-minute run; the cam comes back by its by-id path.

**Done when:** unplugging, replugging and a busy camera behave like the Windows states in spec 2.8.

---

### Task 6: The sharing question on Linux (stage 1 again, rough)

**Files:** nothing committed except the result, which goes into the spec as a "Linux" note under section 1.

**Matrix:** while the backend streams, do Discord's Linux client, Firefox and Chrome get the camera, and the other way round?

The expectation is unverified. With V4L2 and uvcvideo, only one process can stream and a second gets `EBUSY`. The shared route on current desktops is PipeWire's camera node (the xdg-desktop-portal Camera interface, with libcamera or V4L2 underneath), which Firefox uses and Chrome can use. The usual virtual camera is v4l2loopback, a kernel module that needs root.

**Decisions:**

| Result | Then |
|---|---|
| `EBUSY` for the second app | a later port needs a PipeWire backend (`camera/pipewire.rs`, behind the same trait) or a v4l2loopback virtual camera |
| both stream | V4L2 stays the backend |

**Done when:** the result and the recommendation for the port are in the spec.

---

## Beyond stage 6: what a Linux port of ClippiBoy would need

Not planned here. It is listed so nobody mistakes stage 6 for it. Each item names what it replaces.

- **Screen capture** (`wgc.rs`, `capture.rs`): xdg-desktop-portal ScreenCast plus PipeWire on Wayland (the portal asks the user; restore tokens avoid asking every session), or XComposite/XShm on X11. The window and monitor pickers change with it.
- **GPU and converter** (`gpu.rs`, `convert.rs`, including the watermark stamp at `convert.rs:323-343` and the pacer): Vulkan or EGL with dmabuf import.
- **Encoders** (`mft.rs`, `encode.rs`): VAAPI for Intel and AMD, NVENC through the NVIDIA SDK, or ffmpeg/GStreamer libraries, with their own session limits and fallback chain. The cam track's second encoder comes with it.
- **Audio** (`audio/capture.rs`, `audio/devices.rs`: WASAPI loopback and per-process capture): PipeWire or PulseAudio monitor sources, with per-app capture through PipeWire node targets, on `CLOCK_MONOTONIC`.
- **Game detection** (`game.rs`): `/proc` plus the window title from X11 (Wayland mostly hides other windows).
- **Overlay and console** (`overlay.rs`, `console.rs`): always-on-top click-through windows over a fullscreen game. On Wayland that needs layer-shell, which not every compositor offers; on X11, override-redirect windows.
- **Hotkeys, tray, clipboard, sounds, disk, lock screen:** the GlobalShortcuts portal on Wayland; libappindicator; files on the clipboard (`clipboard.rs`); `sounds.rs`; `statvfs` (`disk.rs`); logind/screensaver D-Bus instead of the secure-desktop check `capture::secure_desktop()` (`capture.rs:317-345`, its stub at 347-350; the policy that acts on it is `state.rs:228-251`).
- **Installer, updater, autostart:** AppImage, deb/rpm or Flatpak instead of NSIS (`src-tauri/windows/hooks.nsh`); updater artifacts per format; `tauri-plugin-autostart` through a `.desktop` file.
- **ffmpeg** (`tools.rs:36-40` pins Windows builds and their hashes): a pinned Linux build or the system ffmpeg; `-hwaccel d3d11va` becomes `vaapi`/`cuda` in every composite builder; `muxer::command` (`muxer.rs:52-60`) sets only `CREATE_NO_WINDOW` today, which simply falls away, and stage 2's `muxer::background`, which adds `BELOW_NORMAL_PRIORITY_CLASS`, becomes `nice`.
- **Paths and validators:** the export target check wants a drive letter on Windows (`commands.rs:314-325`, already behind `cfg!(windows)`); the data directory through `dirs` works as it is.
- **Virtual camera** (stage 5): v4l2loopback or a PipeWire virtual source instead of Media Foundation and DirectShow.
- **Camera:** stage 6.
