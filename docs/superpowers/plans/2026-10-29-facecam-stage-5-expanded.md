# Facecam Stage 5, expanded: the virtual camera, and OBS Virtual Camera as a source

> Expands `2026-10-22-facecam-stage-5-virtual-camera.md` with what the spike
> (`scratch/vcam-spike`, spec "Result of the stage-1 test") proved, and with two
> decisions Fabi made on 2026-10-29: OBS Virtual Camera must work as the camera
> ClippiBoy reads (Open for Fabi 13 → (b)), and the ClippiBoy Camera gets a switch
> of its own. Windows 11 only (Open for Fabi 5 → (a), the recommendation); no
> code-signing certificate yet (Open for Fabi 4 stays open for the release).

**Goal:** Discord, browsers and OBS can pick **ClippiBoy Camera** and get the real
webcam's picture while ClippiBoy holds the camera for clips; a switch in the Camera
tab installs it with one UAC prompt and turns it off again; OBS Virtual Camera can
be the camera ClippiBoy reads.

**Architecture:** a workspace `vcam/` of its own (`panic = "unwind"`, static CRT):
`protocol` (shared-memory layout, writer, reader — pure, used by both sides),
`source` (the Frame Server media source DLL, from the spike), `setup` (the elevated
helper that carries the DLL in itself). The app gets `vcam.rs`: the section, the
`MFCreateVirtualCamera` object and its callback; the cam worker publishes every raw
frame into the section while a client streams. OBS Virtual Camera, which Media
Foundation cannot see, is read through ffmpeg's DirectShow input into the same
`camera::Event` stream.

**Spec:** sections 4.1, 4.2, 4.4–4.7, 5.2 item 6; Open for Fabi 4, 5, 13.

## Global constraints

- Commits in English, no `Co-Authored-By`. UI in English, labels without
  explanation lines (Fabi: no hint under every switch).
- No panic leaves a COM method of the DLL (guard + `catch_unwind`); no `unwrap` on
  that path.
- The DLL only lives in `%ProgramFiles%\ClippiBoy Virtual Camera\<version>\`,
  written by the elevated helper from bytes inside itself.
- Fixed CLSID and friendly name "ClippiBoy Camera" (Windows appends its suffix).
- The section's DACL: the user and SYSTEM full, LocalService read. Never Everyone.
- Frames pass through unchanged (no mirror, no watermark).
- A device name only reaches ffmpeg as one argument of `-i video=…`, never a shell,
  and names with `"` or `:` are refused.

## Tasks

### Task 1: `vcam/protocol`
- Files: `vcam/Cargo.toml` (workspace), `vcam/.cargo/config.toml` (crt-static),
  `vcam/protocol/{Cargo.toml,src/lib.rs}`.
- `Header` (repr C: magic `CBVC`, version major/minor, slot count, slot size, pid,
  session, generation, state, idle reason, producer heartbeat, latest slot),
  `Slot` (seqlock, frame number, capture QPC, width, height, bytes), constants
  (4 slots of 1920×1080 NV12). `Writer::publish(nv12, w, h, qpc, now)`,
  `Writer::set_state(state, reason)`, `Reader::read_latest(&mut Vec) -> ReadResult`
  (`Frame{w,h,qpc}`, `Idle(reason)`, `NotRunning`, `Torn`), `is_stale(beat, now)`.
- Tests: layout offsets; a torn slot is never accepted (writer thread hammering,
  checksum per frame); the reader falls back to the previous slot; another major
  version is refused; a stale heartbeat reads `NotRunning`; garbage headers never
  read out of bounds.

### Task 2: `vcam/source` — the media source DLL
- Port `scratch/vcam-spike/source` onto the protocol reader: NV12 and YUY2 at
  1280×720, 1920×1080, 640×360, 30 fps; paced `RequestSample`; the last good frame
  on a torn read; a placeholder (dark, with an accent bar) for `NotRunning` and
  `Idle`; guards on every COM method; a log under `%ProgramData%\ClippiBoy\vcam.log`
  capped at 1 MB.
- Tests: conversion and scaling (from the spike), `picture_for(ReadResult)`.

### Task 3: `vcam/setup` — the elevated helper
- `install | uninstall | status`, manifest `requireAdministrator`, the DLL by
  `include_bytes!`. Install: `%ProgramFiles%\ClippiBoy Virtual Camera\<version>\`,
  read+execute for LocalService, HKLM CLSID (`ThreadingModel=Both`),
  `HKLM\SOFTWARE\ClippiBoy\VirtualCamera` (`Version`, `Path`), the log folder with
  LocalService modify, old versions removed (or marked for reboot). Exit codes
  0 done, 2 refused, 3 needs a reboot.
- Tests: the plan of registry and file steps is pure and mirrored by uninstall;
  a path outside Program Files is refused.

### Task 4: the producer in the app
- `src-tauri/src/vcam.rs` (+ Windows half): `VcamStatus { NotInstalled, NeedsUpdate,
  Off, Ready, Live }`, the section created once, the camera created when
  `facecam.virtual_camera` is on and the DLL is installed, `Start(callback)`:
  SOURCE_START/STOP set `consumer_active`. `CamWants.vcam_consumer` joins
  `cam_wanted`; the cam worker publishes each raw frame while a consumer is active,
  and the idle state when the master switch is off.
- Tests: `cam_wanted` with a consumer; the plan carries it; status from the
  registry facts (pure).

### Task 5: the switch
- Camera tab: "Virtual camera" toggle and a status word; on → `set_virtual_camera`
  runs the helper with `runas` if the DLL is missing or older, then creates the
  camera; off → shuts it down (the DLL stays). "Remove" uninstalls.
- Commands `vcam_status`, `set_virtual_camera`, `remove_virtual_camera`
  (main window only), the generated permissions.

### Task 6: OBS Virtual Camera as a source
- `camera::list` adds DirectShow-only devices from
  `ffmpeg -list_devices true -f dshow -i dummy` (names Media Foundation does not
  list, never ClippiBoy Camera) with ids `dshow:<name>`.
- `camera::open` for such an id runs `ffmpeg -f dshow -i video=<name> -f rawvideo
  -pix_fmt nv12 -` below normal priority, reads the size off ffmpeg's stream line,
  then frames off stdout on a thread, stamped `ClockKind::Arrival`; `close` kills it.
- Tests: parsing the device list and the stream line; names with `"` or `:` refused.

### Task 7: shipping
- The helper as a Tauri resource; `release.yml` builds `vcam/` first; the NSIS
  uninstall hook runs `uninstall` through `runas` when the key exists.

### Task 8: check with Discord (Fabi)
- Switch on → one UAC prompt → Discord lists ClippiBoy Camera and shows the face
  while clips get the cam; ClippiBoy closed → the camera leaves the list; OBS
  Virtual Camera picked in the Camera tab → its picture in the clips.
