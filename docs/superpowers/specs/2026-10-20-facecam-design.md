# Facecam and virtual camera

Status: draft, 2026-10-20. Decisions and section 1 (building blocks and order)
are settled. Sections 2–5 were designed on 2026-10-22 as a proposal for Fabi to
review: every decision that is his to make is marked **Open for Fabi** and
collected at the end, every other choice made while designing is marked *(design
call)*. The result of the stage-1 test is still missing (see "Result of the
stage-1 test").

## Goal

The player's webcam can appear in clips, recordings and screenshots. Where it
sits is set beforehand as a default and can be changed per clip afterwards in
the editor. The camera can be switched off completely. Like OBS, ClippiBoy
offers a virtual camera, so Discord and other apps can keep using the webcam
while ClippiBoy has it open.

Success: save a clip, the cam sits where it should. In the editor it can be
moved, resized or hidden, and Discord sees the player normally the whole time.

Test hardware: Windows 11 (build 26200), eMeet C960 and Logitech C920.

## Decisions

- **Target:** landscape (YouTube/Twitch, 16:9 with the cam in a corner) and
  portrait (TikTok/Shorts, 9:16 with the cam on top, game below) are equally
  important.
- **Virtual camera:** passes the real camera through unchanged. Effects (crop,
  mirror, background blur) can come later.
- **Position afterwards:** one position per clip, changed in the editor (move,
  resize, hide). No keyframes for now.
- **Scope:** clips from the buffer, recordings, screenshots and a live preview
  in the console. Each one can be switched on or off on its own, on a new
  "Facecam" settings tab, plus a master switch "Cam off".
- **Approach A — composite on save:** the cam is buffered as its own small
  stream next to the game picture and laid over it only when saving, editing
  or exporting. Almost no extra load while playing; saving takes a few seconds
  longer. Rejected: B (live overlay plus a clean copy: double encoder load and
  memory) and C (burn in live: position can't change afterwards).

## Section 1: building blocks and order (settled)

### Building blocks

1. **Camera source** (own module, platform-separated): list and open cameras,
   deliver frames with timestamps on the same clock as screen and sound.
   Windows: Media Foundation. Later Linux: V4L2.
2. **Cam track in the buffer:** a second, small H.264 encoder (default 720p30)
   with its own ring next to the game picture (`buffer.rs`, `pipeline.rs`).
   When the cam is off it does not exist at all.
3. **Compositing** on save, export and edit: ffmpeg lays the cam over the
   picture with the existing hardware encoder. The clean game picture and the
   cam stay in the app folder next to the clip, like the audio tracks do today
   (`stems.rs`, `edit.rs`).
4. **Layout per clip** in the stored edit data: position, size, shape, visible.
5. **Virtual camera:** a small DLL of its own that passes the real camera
   frames on through shared memory. Windows 11: the built-in API
   (`MFCreateVirtualCamera`, no driver of our own). Windows 10: a DirectShow
   filter.
6. **Settings tab "Facecam"** (`src/routes/settings/`): device, resolution,
   default position, size, shape, mirror, delay, one switch each for clips,
   recordings, screenshots and console preview, plus the master switch.

### The risk that comes first

Whether Discord can still use the camera while ClippiBoy has it open depends
on Windows and the camera driver. Windows shares many USB cameras by itself by
now, but not all, and not always in every format. If ClippiBoy blocks the cam,
the feature is worthless until the virtual camera exists. So step 1 is a test.

### Order

1. **Test "share the camera":** ClippiBoy opens the C920 and the eMeet — does
   Discord still see them? On Windows 11 and, via a tester or a VM, Windows 10.
   The result decides how early the virtual camera has to come.
2. **Foundation and clips:** camera source, cam track, compositing on save,
   settings tab, master switch, console preview.
3. **Editor:** change the position per clip, portrait export 9:16.
4. **Recordings and screenshots.**
5. **Virtual camera:** Windows 11 first, then Windows 10. Moves forward if the
   test shows that Windows does not share the cam.
6. **Later:** Linux.

Each stage is usable on its own and gets its own plan.

## What the research changed (notes on section 1)

Section 1 stays as it is. Designing sections 2–5 turned up six facts that shape
them; where one of them touches a settled decision it is an **Open for Fabi**
item at the end. Line numbers are those of commit 401a320. The sourced research
behind this is in `docs/extras/research/`.

1. **Opening the camera the plain way blocks Discord.** `MFCreateDeviceSource` +
   source reader, and WinRT `MediaCapture` with `ExclusiveControl`, open the
   camera as the *controlling* client; Discord, browsers, OBS and the Camera app
   then fail with "camera in use" (`0xC00D3704`). Windows' Frame Server shares a
   camera only with a client that asks for **sharing mode** — and such a client
   can never choose the format, it gets whatever the controlling app set. Three
   ways to ask exist: an MF sensor group set to shared (Windows 10 1607+),
   `MF_DEVSOURCE_ATTRIBUTE_FRAMESERVER_SHARE_MODE = 1` on a device source
   (Windows 11 26100+), and WinRT `MediaCapture` with `SharedReadOnly` (1607+).
   Building block 1 stays "Media Foundation", but in sharing mode; the stage-1
   test picks the API.
2. **Discord very probably captures through DirectShow** (its engine is built on
   libwebrtc, whose Windows capture is DirectShow only). Microsoft's camera team
   says a DirectShow client "may block" sharing readers, and nothing can be done
   about it. If the stage-1 test shows that, the virtual camera moves forward, as
   section 1 already foresees.
3. **Windows 11 24H2/25H2 has a user switch for this**: *Allow multiple apps to
   use camera at the same time* (from KB5089573, 26100.8524 / 26200.8524, rolled
   out gradually, off by default, one forced format). Worth a hint in the UI, not
   something to build on.
4. **Every clock is the same clock.** The game picture (WGC `SystemRelativeTime`,
   `pipeline.rs:16-19`, used at `pipeline.rs:704`), the sound (WASAPI QPC
   position, read at `audio/capture.rs:640-663`, on the timeline of `now_100ns()`,
   `audio/capture.rs:21-44`) and the camera (MF
   `MFSampleExtension_DeviceTimestamp`, WinRT `MediaFrameReference.SystemRelativeTime`)
   are all QPC in 100 ns. The cam needs no clock of its own — only the same
   anchoring the game picture has (`pipeline.rs:621-627`).
5. **The camera source must not live inside the game pipeline.** The virtual
   camera needs the webcam while the buffer is off, and the game pipeline is torn
   down and rebuilt on screen changes (`lib.rs:594-650`) and target changes
   (`commands.rs:114-132`), and stopped behind the lock screen
   (`state.rs:217-251`, `lib.rs:665-700`) — the mechanics are
   `state.rs:536-620`. Each time, a cam inside it would switch the webcam LED off
   and on and give Discord a chance to grab the camera *(design call: the cam has
   its own lifecycle in `AppState`, section 2.2)*.
6. **The watermark is burned into the game picture at capture time**, bottom
   left (`watermark.rs:1-31`, drawn per frame in `convert.rs:323-343`). A cam placed there
   covers it, and a 9:16 crop usually cuts it off. The default position is
   therefore bottom right *(design call)*, and the portrait case is an
   **Open for Fabi** item.

## Section 2: data flow when saving

Questions from the draft: how the cam ring is cut to the clip window, A/V sync
with the game picture and audio, the delay slider, what happens when the cam
drops out mid-buffer, where clean picture and cam track are stored.

### 2.1 Overview

```
camera (shared mode) ──frames + QPC──► cam worker ──► 30 fps grid ──► cam encoder (H.264 720p30, GOP 1 s)
                                          │                               │
                                          ├─► console / settings preview   └─► cam ring (ReplayBuffer #2)
                                          └─► virtual camera (stage 5)                │
                                                                                      │ cut by the game's QPC window
hotkey ─► game snapshot (as today) ─► cam cut ─► phase 1: clean clip as today (1–2 s) + cam.mp4 + facecam.json
                                                 phase 2: composite in the background ─► clip file with the cam
```

### 2.2 The camera source and its lifecycle

- **Module** `src-tauri/src/camera/` with `mod.rs` (platform-neutral types: device
  list, frame, errors, the grid), `windows.rs` (MF), and a `#[cfg(not(windows))]`
  stub like `audio/devices.rs:284-345`. Later `linux.rs` (V4L2).
- **API:** sharing mode only (fact 1; whether ClippiBoy may take control for a
  moment to set a good format is Open for Fabi, item 11). The stage-1 probe
  decides between the three ways; the plan defaults to the MF sensor group,
  which Microsoft documents for Windows 10 1607+ and 11 and which stays inside the
  `Win32_Media_MediaFoundation` feature that is already enabled
  (`src-tauri/Cargo.toml`). One research source doubts that it shares on Windows
  10 the way WinRT `SharedReadOnly` does; stage 1 tests exactly that, and the
  camera source sits behind a trait so the WinRT way can replace it without
  touching the rest. The one exception is the virtual camera (section 4):
  while it is on, ClippiBoy is the only client of the real camera and may open it
  controlling, with full format choice.
  **After stage 1:** sharing mode does not share with Discord desktop (see
  "Result of the stage-1 test"), so stage 2 opens the camera controlling, with
  the format closest to the cam track's size at 30 fps (MJPEG first), and says
  so on the Facecam tab; the virtual camera follows right after stage 2.
- **Frames:** whatever format arrives (MJPEG decoded by MF to NV12, YUY2, NV12, any
  size) is uploaded into a texture and scaled on the GPU into the cam track's
  fixed size, 1280×720 NV12 by default, **cover-cropped** so a 4:3 camera fills
  the box instead of getting bars *(design call)*. A format change in the middle
  (Discord starting its call at another size) only changes the upload and the
  scaler; the encoder keeps its size, so the ring is never broken.
- **Its own D3D11 device on the encoder's adapter**, not the game's device: the
  game's immediate context is multithread-protected and shared by capture and
  encoder (`gpu.rs:229-232`), and the cam must not queue behind it. The existing
  `Converter` is not reused: it converts BGRA desktop frames
  (`convert.rs:93-102`) and stamps the watermark on every frame while the
  watermark is on (`convert.rs:183-189`, `323-343`).
- **Timestamps:** `DeviceTimestamp` (MF) or `SystemRelativeTime` (WinRT), checked
  for plausibility against `now_100ns()` the way the audio code checks its
  positions (`audio/capture.rs:67`); the fallback is the arrival time, which is
  flagged (`ClockKind::Arrival`) and shown in the diagnostics line.
- **Lifecycle** *(design call)*: a `CamTrack` held by `AppState` next to the
  pipeline, not inside `win::Running` (`pipeline.rs:599-607`). It runs while any
  of these wants it, and the webcam is closed the moment none does:
  - the buffer is wanted (`buffer_wanted`, `state.rs:102`) and *Facecam in clips* is on;
  - a recording runs and *Facecam in recordings* is on (stage 4);
  - the Facecam settings tab shows its preview;
  - a virtual-camera consumer is attached (stage 5).
  
  A restart of the game pipeline does not touch it: when only the buffer goes
  away, the cam waits 3 s before it closes, which covers a restart (screen or
  target change) without closing and reopening the webcam. The master switch and
  the lock-screen rule close it at once. With the master switch off nothing of it
  exists: no thread, no device, no encoder, no ring.
- **Lock screen:** the cam follows the buffer's own lock rule — it stops once
  `AutoBuffer::poll_lock` has seen the secure desktop for its 10 s
  (`state.rs:179-187`, `217-251`; a UAC prompt raises the same desktop and must
  not cost the cam), **also while a recording keeps the game pipeline alive**
  (`state.rs:588-602`), and comes back with the buffer *(design call, privacy)*.
- **Privacy rule** (from "Ideas for later", taken up here): the webcam runs only
  while something the user switched on needs it — the buffer, a recording, the
  open settings preview, a virtual-camera consumer — and the LED says so.

### 2.3 The cam track: grid, encoder, ring

- **30 fps grid anchored on the cam's own timestamps** (not `convert::pace`,
  which takes "the newest frame at tick time" and adds a frame of jitter).
  `slot = round((qpc − origin) / period)`; several frames in a slot keep the
  newest; an empty slot repeats the last frame; a slot is emitted once a later
  frame arrived or 100 ms after its time passed. Webcams drop to ~15 fps in low
  light by design (auto-exposure priority; a sharing client may not switch it
  off), which then simply shows every frame twice. The stream that reaches the
  encoder is exact CFR, so the existing raw-ES muxing with `-r` stays valid for
  the cam (`muxer.rs:251-264`, `316-317`).
- **pts like the game picture:** `pts = slot × period`, and the cam's
  `origin_100ns` plays the role of `Shared::base_100ns` (`pipeline.rs:621-627`,
  `557-559`).
- **Dropouts:** a gap under 2 s repeats the last frame. A longer one (unplugged,
  USB hiccup, Discord's DirectShow capture starving the shared reader, privacy
  switched off) emits **filler frames** (black, a few bytes each once encoded), so
  the cam stream stays continuous; the lost QPC range is recorded in a small list
  next to the ring, trimmed together with it. The camera is re-opened with
  back-off and on device arrival. Because the grid's stall timer keeps emitting,
  the cam's pts never jump while the process runs — except where the encoder
  itself drops a frame (its 500 ms submit wait, `mft.rs:134`), which leaves a
  one-frame hole a `-r 30` stream cannot carry. Such holes are recorded, and a cut
  never spans one: it starts after the newest hole inside the window (rare; the
  cam then starts a little later in that clip).
- **Sleep** is the one case where they would: QPC moves on while nothing runs. It
  is recognised by comparing QPC with `QueryUnbiasedInterruptTime`, which stops
  during sleep — not by a gap alone, which a camera open or an encoder start on a
  busy machine can also cause. A sleep **clears the cam ring** and starts a new
  origin and a new encoder session *(design call)*. The blocking calls (opening and
  closing the camera, starting an encoder) run on a helper thread, never on the
  thread that clocks the grid. A
  `-r 30` stream cannot carry a hole (ffmpeg would close it up and shift everything
  after it), and the game ring drops what lies before a sleep anyway (review
  BUF-5's fix). A clip that reaches back across the sleep simply gets the cam from
  the wake-up on (positive offset, 2.4).
- **Encoder:** a second `VideoEncoder` (`mft.rs:1090-1156`) with 1280×720, 30 fps,
  GOP 1 s (more cut points, better seeking in the editor), no B-frames, 3 Mbit/s
  peak-constrained VBR (6 Mbit/s at 1080p) — not the game's constant-quality mode
  (`mft.rs:318-340`), which would let a dim, noisy webcam run far over the budget
  its ring is sized for. It costs about 1.5–6 % of one NVENC engine; GeForce allows 8 sessions
  from driver 551.76 (game + cam + an ffmpeg composite = 3). If the hardware
  session cannot be opened, the Microsoft software H.264 MFT is used with
  system-memory NV12; if that fails too, the cam track is off with a message.
  Its packets come out as track 0 (`mft.rs:1051-1057`), which is fine in its own
  ring and must never reach the game ring or `Recorder::video`
  (`recorder.rs:223-233`). The encoder MFT is never explicitly shut down today
  (review CAP-V2); with a second session per start/stop cycle that has to be fixed
  first, or restarts may run into the session limit.
- **Ring:** a second `ReplayBuffer` (`buffer.rs:31-52`) with its own lock, never
  taken together with `Shared.packets`. Length = buffer length + game keyframe
  interval + 1 s cam GOP + 0.3 s delay range + 10 s save margin (the cam is cut
  only after the clip itself is written, 2.4; 10 s of cam are under 4 MB). The game ring's
  memory formula (`config.rs:100-110`, floor 64 MB) is not reused: the cam gets
  `1.5 × 3 Mbit/s × length`, floor 16 MB — about 70 MB for the default 120 s
  buffer. A user-set `memory_mb` is the game's; the cam is shown as its own line
  on the Recording page *(design call; Open for Fabi whether the cam should come
  out of `memory_mb` instead)*.
- The keyframe-count and sleep fixes from the review (BUF-4, BUF-5) apply to this
  ring as well.

### 2.4 Cutting the cam to the clip window

The game is snapshotted as today (`state.rs:883-892`). The cam is cut **after
`muxer::build` has written the clip** — by the game's absolute QPC window, so it
does not matter that this happens a second or two later. Cutting it earlier would
mean waiting for the cam before the audio is read, and the audio rings are
trimmed while the save runs (review BUF-1).

- Game window, as today: `G0 = qpc_of(first_pts)` (`pipeline.rs:557-559`,
  `1033-1077`), length `L = span`.
- Cam: all packets from the last cam keyframe at or before `G0 − 300 ms` up to
  `G0 + L + 300 ms` (cut *by absolute time*, not "the last N seconds", which would
  be relative to the cam ring's own newest packet, `buffer.rs:181-200`). The 300 ms
  on both sides are the delay range, so the delay can still be changed per clip
  later in either direction. This needs `ReplayBuffer::snapshot_range(from_us,
  to_us)`, the body of `snapshot` with both ends passed in.
- By the time the clip is written (1–2 s) the cam ring has long passed
  `G0 + L + 300 ms`, so there is no wait. If the clip was very quick to write, wait
  for the cam ring to get there, at most 500 ms, outside every lock; whatever is
  still missing is covered by holding the last cam frame (`eof_action=repeat`).
  The cam ring's 10 s save margin (2.3) keeps `G0 − 300 ms` in it meanwhile; on a
  save slower than that the cam simply starts a little later (positive offset).
- `cam_offset_us = (cam_origin + first_cam_pts·10 − G0) / 10` — normally negative
  by up to one cam GOP plus 300 ms.
- Gaps (from 2.3) that intersect `[G0, G0+L)` are stored in clip coordinates.
- No cam frame in the window at all (cam started after the clip, or it was off):
  the clip is saved exactly as today and gets no facecam data.

### 2.5 Sync and the delay slider

- Picture, sound and cam share QPC, so sync is the offset above and nothing else.
- What is left is the camera's own latency: the timestamp is taken around the time
  the frame reaches the host (for UVC cameras Microsoft does not document exactly
  when), not when the light hit the sensor. Microsoft certifies USB
  cameras for at most 120 ms photon-to-display; the estimate from that for
  photon-to-timestamp is 40–90 ms, so the face runs one or two frames behind the
  voice.
- **Delay** `delay_ms`, −300 … +300 ms in 10 ms steps, negative = the cam is shown
  earlier. The default is **−40 ms** as an estimate, to be replaced by the clap
  test in stage 2 (clap in front of the cam with the microphone recording;
  compare the frame with the audio peak). −80 ms when the cam runs on the
  arrival-time fallback.
- The delay is **never baked into the ring**: it is applied when compositing, so
  every clip keeps the value it was saved with and can be corrected later.

### 2.6 Saving: two phases

Compositing re-encodes the whole clip. NVIDIA's own numbers (H.264, preset P5)
give about 6–7 s for a 30 s 1080p60 clip and 40–48 s for the default 120 s at
1440p60; the x264 fallback is two to three times slower. Done inside the save,
`begin_save` (`state.rs:466-472`, `lib.rs:110-113`) would reject every further
press for that long, and a second highlight a few seconds after the first would
be lost. So *(design call)*:

1. **Phase 1, inside the save, same speed as today:** the game snapshot is muxed
   with `-c:v copy` exactly as today (`muxer.rs:114-226`, `285-448`) into the clip
   path — the clip exists, plays, can be shared. The row, the banner and the sound
   come exactly as today. Only then is the cam cut (2.4) and a second, separate
   ffmpeg run writes `cam.mp4` from the cut cam packets
   (`-f h264 -r 30 -i cam.h264 -c:v copy`), so a broken cam stream can never cost
   the clip and never delays the banner. `facecam.json` (2.7) is written last as
   the "complete" signal, the way `stems::write_index` is (`stems.rs:85-104`), and
   the clip row gets `facecam.state = pending`.
2. **Phase 2, a background job:** one worker thread, one job at a time, below
   normal priority (`BELOW_NORMAL_PRIORITY_CLASS` next to `CREATE_NO_WINDOW`,
   `muxer.rs:52-60`), GPU decode (`-hwaccel d3d11va`), the encoder from the
   settings at a faster preset (NVENC `p3`, AMF `speed`, QSV `veryfast`, x264
   `superfast`) and the same quality mapping as `edit.rs` (`edit.rs:757-798`), with
   the x264 retry of `edit.rs:461-479`. It composites clip + `cam.mp4` + mask into
   `<clip>.<pid>.cam.mp4` (audio `-c:a copy`), files the clean clip away as the
   original (2.7), and swaps the composite in with `replace_file`
   (`muxer.rs:455-470`), the way `edit::swap_in` does (`edit.rs:580-629`). Then
   thumbnail, file size, `facecam.state = ready`, `clips-changed`.
3. **Failure:** the clean clip stays where it is, `facecam.state = failed`, one
   notice ("The cam couldn't be added — try again from the editor"). The clip is
   never lost.
4. **Crash or quit during phase 2:** at start, `repair` finds `facecam.json`
   without a finished composite and puts the job back in the queue.
5. **The job, edits, deletes, exports and shares of the same clip are
   serialised per clip.** `edit::WORKING` (`edit.rs:53`, `406`) is one lock for
   all clips; a composite holding it for up to two minutes would block every
   editor save and undo. So stage 2 adds a lock per clip id
   (`edit::clip_lock(id) -> Arc<Mutex<()>>` from a small map) taken by the job,
   `edit::run`, `delete_clip` (review EDIT-1), `file_clip` and `shrink`; the
   global `WORKING` stays for the all-clips operations ("Clear all trims",
   `edit.rs:217-218`). Export and share of a pending clip wait for its job.
6. **Cam hidden by default:** no phase 2; the clip is today's clip, but `cam.mp4`
   and the facts are still written and the state is `ready`, so the cam can be
   switched on later in the editor. **No cam frame in the window:** no facecam
   data at all (2.4).

During phase 2 the gallery shows the clip with a small "adding cam" mark. Whoever
drags the file out in those seconds gets it without the cam; that is the price of
never making a save slower than today.

### 2.7 Where the files are stored

The untouched recording already has a home: `originals/<id>/` (`edit.rs:8-27`),
"the original" that trims work from. With a facecam the untouched recording is
the clean picture plus the cam, so it goes there *(design call; Open for Fabi —
the alternative is a separate `facecam/<id>/` store)*:

| File | Written | What |
|---|---|---|
| `<clip folder>/…/<name>.mp4` | phase 1 (clean), phase 2 (with cam) | the clip, as today |
| `<data>/originals/<id>/cam.mp4` | phase 1 | the cam, H.264 in MP4, from a keyframe ≤ `G0 − 300 ms` |
| `<data>/originals/<id>/facecam.json` | phase 1, updated by every composite | facts: cam size and fps, `cam_offset_us`, gaps, clock kind, camera name, what the clip file currently shows (`applied`) |
| `<data>/originals/<id>/video.mp4` | phase 2 (the clean clip moved here) | the clean picture with its audio, untrimmed |
| `<data>/tracks/<id>/…` | as today | the stems, untouched |
| `<data>/temp/…mask_<w>x<h>.png` | each composite | the shape mask, regenerated from the layout, never kept |

- The clip DB gets one new column, `facecam` (JSON), added with `add_column`
  (`clips.rs:77-87`): the user's layout, the per-clip delay and the state.
  Building block 4 says "in the stored edit data"; this is the same table and the
  same kind of JSON, but deliberately not a field inside `ClipEdit`
  (`model.rs:742-750`), because `store()`, `settle()` and the share path rebuild
  `ClipEdit` from scratch (`commands.rs:1413-1417`, `edit.rs:939-948`,
  `share.rs:887`) and would silently drop it (**Open for Fabi**, item 10). The
  column must also go into `insert`'s `INSERT OR REPLACE` list (`clips.rs:89-131`),
  or every re-insert of a clip wipes it.
- The clean copy doubles the size of every clip with a visible cam, on the data
  drive. It is listed under *Storage location* like the trim originals
  (`commands.rs:1276-1304`), and **Free up space** on a facecam clip means "bake
  the cam": the clip file stays, the cam can no longer be moved, and
  `facecam.state` becomes `baked` (the states are `pending`, `ready`, `failed`,
  `baked`). On a trimmed clip it also gives up *Undo trim* — the same trade as
  today's *Free up space* for trims, and the confirmation says so.
- `edit.rs` must learn about it, or it goes wrong silently: an untrimmed save
  removes the originals folder (`edit.rs:625-627`); undo-trim copies the archived
  file back (`edit.rs:340-349`), which with a cam has to re-composite instead;
  "Clear all trims" (`edit.rs:159-230`) must **leave facecam clips alone** — it
  counts every `video.mp4` (`trim_originals_on_disk`, `edit.rs:180`), so it has
  to skip folders with a `facecam.json`; the clean pictures get their own
  "free up space" on the storage page (3.6); `repair` treats `trim.json` as the
  "alive" signal (`edit.rs:13-15`, `958-1052`) and must count `facecam.json` too;
  and `swap_in` must never archive a composited clip as the original
  (`edit.rs:571-601`).
- **These `edit.rs` changes belong to stage 2, not stage 3.** Trimming exists
  today: the moment stage 2 ships, a user can front-cut or undo-trim a clip with a
  baked cam. So stage 2 already re-composites on a front cut and on undo with the
  clip's stored layout (rows 4 and 5 of the table in 3.4); stage 3 adds the UI to
  change that layout.

### 2.8 What can go wrong, and what the user sees

| Situation | Behaviour |
|---|---|
| Camera privacy off for desktop apps (`E_ACCESSDENIED`) | cam track off; Facecam tab: "Windows blocks the camera for desktop apps" + button to `ms-settings:privacy-webcam` |
| Camera taken by an app that does not share (`0xC00D3704`) | retry with back-off; Facecam tab names the app (`IMFSensorActivityMonitor`) and suggests the Windows 11 multi-app switch or, once it exists, the virtual camera |
| Shared reader gets no frames for 5 s (a DirectShow app holds the camera) | filler frames, gap recorded; same hint as above |
| Unplugged (`0xC00D3EA2`) | filler frames, gap; reopened on arrival (by id, then by name) |
| Format changed by another app | scaler reconfigured, nothing else |
| Hardware encoder session refused | software MFT; if that fails too, cam off with a notice |
| Composite fails with hardware and x264 | clean clip kept, `facecam.state = failed`, retry in the editor |
| The clip is open in the player when the composite is ready (Windows cannot replace an open file) | the swap retries with back-off for about a minute, then every few minutes and at the next start (no signal from the player is needed); `failed` only after a day of that or when the composite itself fails |
| Disk too full for clean copy + composite | phase 2 skipped (`disk.rs`), clip kept clean, notice |
| Sleep / resume | cam ring cleared, new origin and encoder session (2.3); a clip that reaches back across the sleep gets the cam from the wake-up on |

Errors reach the UI through the same channel as capture errors, which first
needs the review's BUF-2 fix (`pipeline.rs:549-554`).

### 2.9 Cost while playing

- Capture and MJPEG decode: MF's decoder runs on the CPU, estimated 10–25 % of one
  core at 720p30 (twice that at 1080p). Measured in stage 1.
- Upload + scale: 41 MB/s over PCIe and one small video-processor blit per frame.
- Encode: 1.5–6 % of one NVENC engine; AMF/QSV comparable.
- Memory: ~70 MB for a 120 s buffer.
- Saving: phase 1 unchanged; phase 2 is a below-normal-priority ffmpeg with
  limited threads (`-threads 4` for the game decoder, `-filter_complex_threads 2`).
  The stage-2 plan measures it with a game running.

### 2.10 Recordings and screenshots (stage 4, outline)

Not one of the open sections, but stage 2 has to leave the right hooks, so the
shape is fixed here *(design calls; how much may be burned in is Open for Fabi,
item 12)*:

- **Recordings** tap the game encoder's packets into `.recording/<id>/` while they
  run (`recorder.rs:223-233`); the cam ring only holds the buffer length. So the
  recorder gets a second writer: cam packets from the cam keyframe at or before the
  recording's start into `.recording/<id>/cam.<n>.h264`, one **segment** per cam
  session (a recording can outlive the cam: the lock screen stops it, sleep resets
  it, the master switch can turn it off and on — each of those starts a new
  session; an unplug does not, it only leaves a gap of filler frames, 2.3), each
  with its offset, plus the gaps, and the cam facts
  in the recording's `meta.json` from the start (crash recovery reads it). At stop
  the recording is finished as today (clean, `-c:v copy`), and the composite is
  the same background job as for clips — minutes for a long recording, at
  below-normal priority, one `overlay` per segment, each with `eof_action=pass`.
- A recording "keeps no untouched copy when trimmed" — it can run for an hour
  (`model.rs:703-706`). Keeping a clean copy for a movable cam would break that
  rule, so for recordings the cam is **burned in at stop** with the default layout
  and the clean file is dropped once the composite is in place (*recommended*;
  Open for Fabi, item 12).
- **Screenshots** take the newest raw cam frame at the shot's QPC (the grab
  currently drops the frame's time, `shot.rs:541-543`; the cam source keeps its last
  frame with its time for this). The cam is composited into the PNG on the CPU at
  the default layout, and the clean PNG goes where screenshot originals already go
  (`originals/<id>/`, `edit.rs:154-158`), so the screenshot viewer can move or hide
  the cam later the way it already re-edits marks and crops. A screenshot gets the
  cam only while the cam already runs (buffer or recording): opening a webcam takes
  about a second, far too long for a screenshot. The Facecam tab says so next to
  the switch.
- Stage 2 therefore provides: a raw-frame tap with the frame's QPC (for the console
  preview it is needed anyway), and a packet tap on the cam encoder that a second
  consumer can attach to, like the game encoder's recorder tap.

## Section 3: editor

Questions from the draft: UI for moving/resizing/hiding the cam, preview while
editing, portrait export layout.

### 3.1 The layout

```rust
struct CamLayout {      // per clip in the DB column `facecam`, default from the settings
    visible: bool,
    x: f32, y: f32,     // 0..1 of the free space; (1, 1) = bottom right
    size: f32,          // cam height as a share of the frame height, 0.12..0.5, default 0.25
    shape: CamShape,    // Rect | Rounded | Circle (unknown values load as Rect)
    mirror: bool,
}
struct PortraitLayout { // used by the 9:16 export
    split: f32,         // height share of the cam band, 0.25..0.5, default 0.35
    game_x: f32,        // where the 9:16 window sits on the game picture, 0..1, default 0.5
}
```

- Normalised to the game frame, so it fits every resolution and survives an
  export at another size. The margin is fixed at 2.5 % of the frame height.
- Rust turns the layout into pixels (even numbers for yuv420) and the filter
  graph contains only literals, as `edit::arguments` and `export::arguments` do;
  the builder is a pure, tested function next to `stems::mix_filter`
  (`stems.rs:342-372`).
- Shapes: a gray8 PNG mask rendered in Rust (the `png` crate is already a
  dependency) and merged with `alphamerge`. Per-pixel `geq` would cost about a
  CPU-second per clip second and is not used. Building blocks 4 and 6 list shape
  and mirror, so v1 has rectangle, rounded and circle, and mirror; the accent
  border, crop and zoom from "Ideas for later" stay later.

### 3.2 Preview while editing

The clip file already has the cam baked in at its saved position, so a
draggable cam over it would show two faces. While the clip has a facecam, the
player shows **the clean picture with the cam on top**:

- The clip's `<video>` stays the clock — trim, marks and resume all work in its
  coordinates (`ClipPlayer.tsx:368-408`, `458-467`, resume at `569-575`,
  `780-786`) — but its picture is hidden.
- Two muted `<video>` elements, the clean original (or the clip file itself
  whenever that is still clean: state `pending` or `failed`, or the cam was hidden
  when the clip was saved, 2.6 steps 3 and 6) and `cam.mp4`, follow it with the offsets
  `original.start_ms` and `cam_offset + delay`. The binding is the one the stems
  already use (`useClipMix.ts:311-375`), pulled out into a shared
  `useSlavedMedia` hook. For video, seeks are done on `play`/`seeked` only and the
  drift is corrected with small `playbackRate` nudges, because a hard seek in
  H.264 decodes from the last keyframe (the cam's 1 s GOP keeps that short).
- The cam element is positioned with CSS over the letterboxed picture
  (`pictureBox.ts:43-57`, `89-129`); shape with `border-radius`/`clip-path`,
  mirror with `scaleX(-1)` — what you see is what the composite draws.

### 3.3 Moving, resizing, hiding

- The crop box of the screenshot viewer (`CropLayer`, `ShotViewer.tsx:1338-1520`:
  move, four-corner resize with a locked ratio, pointer capture) is pulled out
  into `src/components/ui/RectLayer.tsx` and used for the cam box, ratio locked to
  the cam's (16:9, or 1:1 for the circle). It snaps to the four corners and the
  margins.
- A **Facecam** section in the edit pane (`ClipEditor.tsx`, between Audio and
  Trim, `106-236`): *Show cam*, shape, mirror, *Reset to default*, and a small
  *Cam sync* slider for the per-clip delay.
- The layout counts for "unsaved changes" (`ClipPlayer.tsx:550-557`, `1074-1108`),
  and **Save** re-composites.

### 3.4 What Save does

Always from the clean sources, so the picture loses one generation however often
the cam is moved — the rule `edit.rs` already follows for trims (`edit.rs:25-27`):

| Cam visible | Front cut | Layout or delay changed | Video |
|---|---|---|---|
| no | no | — | clean original, `-c:v copy` |
| no | yes | — | clean original, re-encode, no overlay |
| yes | no | no (tail trim / mix only) | the clip file, `-c:v copy` (today's fast path) |
| yes | any | yes | clean original + cam, composite |
| yes | yes | no | clean original + cam, composite |

`plan()` gets `reencode = cuts_front || layout_changed` (today `edit.rs:330`) and a
third video source "clean + cam"; `arguments()` gets the cam as the last input,
so the stem numbering (`edit.rs:694`) stays as it is, and one `-filter_complex`
for video and audio together.

### 3.5 Portrait export 9:16

- An **orientation choice** in the export dialog (`ExportDialog.tsx`):
  *Landscape* (today) or *Portrait 9:16*. Portrait is always a separate file and
  never rewrites the clip.
- Layout: the cam band on top (`split`, default 35 %), cover-cropped from the cam;
  the game below, a window of the band's aspect cut out of the game picture at
  `game_x` and scaled to 1080 wide. While the cam is absent (before its start, in
  a gap) the band shows a blurred strip of the game. Built with `pad` + `overlay`;
  `vstack` is not used, it drops game frames until the cam's first frame.
- In the editor a *9:16* toggle switches the preview into this layout, with the
  game window draggable sideways and the split draggable up and down. The values
  are stored as `PortraitLayout` in the clip's `facecam` column.
- The size-target recipe (`export.rs:78-102`, mirrored in `ExportDialog.tsx:35-53`)
  works on the source height and never upscales, which would squash a portrait
  export to 608×1080. It is reworked to step on the output's short side
  (1080 → 720 → 480).
- Without a facecam the portrait export still works: the game centred in 9:16 with
  a blurred fill behind it.
- Audio comes from the current clip file (already trimmed and mixed).

### 3.6 Undo, free up space, storage

- *Undo trim* restores the whole length and re-composites with the current layout.
- *Free up space* on a facecam clip bakes the cam (2.7) and says so: "The cam
  can no longer be moved in this clip."
- The storage page gets a row "Clean pictures for moving the cam" with its own
  *Free up space*, which bakes the cam in every clip that has one. "Clear all
  trims" stays what it is and does not touch them.

## Section 4: virtual camera in detail

Questions from the draft: DLL packaging and registration, shared-memory format,
behaviour when ClippiBoy is closed, Windows 10 path.

### 4.1 Shape

```
ClippiBoy.exe (user, session N)
  camera source ── the real camera, controlling (ClippiBoy is its only client now)
    ├─► cam track, previews (as before)
    └─► vcam producer ─► shared memory "ClippiBoy.VCam.v1" (+ a frame event)

Windows 11: Frame Server (svchost, LocalService, session 0)
  ClippiBoyVCam.dll (Rust, MF media source) ─► Discord, browsers, Teams, OBS, Camera app
Windows 10: inside every consumer process
  ClippiBoyVCamDS.dll (C++, DirectShow source filter) ─► Discord, Chrome, Edge, Zoom, OBS
```

- The camera is called **ClippiBoy Camera** (Windows 11 appends its own
  virtual-camera suffix). It passes the real frames through unchanged, as decided:
  no mirror, no effects, no watermark.
- With the virtual camera on, ClippiBoy opens the real camera **controlling** and
  chooses its format (720p30 or 1080p30 MJPEG) — sharing mode is no longer needed,
  and the formats the cam track gets are the best the camera offers. The user
  points Discord at *ClippiBoy Camera* once; the Facecam tab says so.
- Both paths offer NV12 **and** YUY2 at 1280×720 and 1920×1080, 30 fps: libwebrtc's
  DirectShow capture (and so, probably, Discord) does not take NV12.

### 4.2 Windows 11: `MFCreateVirtualCamera`

- ClippiBoy creates the camera at start (it autostarts with Windows) on a
  background MTA thread: `MFCreateVirtualCamera(SoftwareCameraSource, Session,
  CurrentUser, "ClippiBoy Camera", "{fixed CLSID}", …)`, then `Start(callback)`.
  This needs no admin. The object is kept for the life of the process.
- The media source is a COM in-proc DLL in Rust (windows-rs `#[implement]` has
  every interface needed; other projects run Rust media sources in Frame Server).
  It is its own Cargo project: it must be built with `panic = "unwind"` and catch
  every panic at the COM boundary, because a crash in it takes Frame Server down —
  and with it every camera on the machine. ClippiBoy's release profile is
  `panic = "abort"` (`src-tauri/Cargo.toml`).
- **Frame Server loads it as LocalService in session 0, and Frame Server Monitor as
  LocalSystem.** So the DLL must be registered under HKLM (HKCU makes `Start` fail)
  and must sit in a folder those services can read and the user cannot write:
  `%ProgramFiles%\ClippiBoy Virtual Camera\<version>\`. ClippiBoy itself installs
  per user into `%LOCALAPPDATA%`, which rules that folder out.
- `SOURCE_START` from the callback (an app opened the camera): open the real
  camera if the master switch is on, else publish "Camera is off in ClippiBoy".
  `SOURCE_STOP`: stop publishing, close the real camera if nothing else needs it.

### 4.3 Windows 10: a DirectShow source filter

- No virtual-camera API exists; the only driver-free way is a DirectShow source
  filter, registered in `CLSID_VideoInputDeviceCategory`. Discord, Chrome, Edge,
  Zoom, Teams and OBS see it; MF-only apps (Camera app, Settings) do not.
- Written in **C++ on Microsoft's DirectShow BaseClasses (MIT)**, optionally
  starting from softcam (MIT). OBS's virtual camera (GPLv2) and akvirtualcamera
  (GPLv3) must not be copied — ClippiBoy is not open source.
- The filter is always listed. Without ClippiBoy it shows a "ClippiBoy is not
  running" picture at the format stored in `%LOCALAPPDATA%\gg.clippiboy.app\vcam.json`.
- Windows 10 is still about 23.5 % of Steam (June 2026), but out of mainstream
  support; whether to build it at all is **Open for Fabi**.

### 4.4 Installing: one elevated, signed helper

- `clippiboy-vcam-setup.exe` (~300 lines of Rust, `requireAdministrator`,
  Authenticode-signed) **carries the DLLs inside itself** (`include_bytes!`), so
  nothing is ever copied from the user-writable app folder into a path SYSTEM
  loads from. `install`: write the DLL into a versioned folder, register the CLSID
  under `HKLM\SOFTWARE\Classes\CLSID\{…}\InprocServer32` (`ThreadingModel=Both`; on
  Windows 10 also `IFilterMapper2::RegisterFilter`), note the version under
  `HKLM\SOFTWARE\ClippiBoy\VirtualCamera`, remove old versions (or mark them for
  deletion at reboot while a process still has them loaded). `uninstall`: the
  reverse.
- The one UAC prompt appears when the user switches *Virtual camera* on, and again
  only when an update brings a new DLL version — explained in the app, never
  inside the silent updater. The DLLs are kept small and rarely changed; the
  protocol is versioned so a newer app works with an older DLL.
- The NSIS uninstaller (`src-tauri/windows/`) runs `uninstall` through `runas` if
  the registry key exists.
- Unsigned DLLs are blocked by Smart App Control. ClippiBoy has no code signing
  today, so this feature needs it (**Open for Fabi**).

### 4.5 Shared-memory protocol v1

- One fixed-size, pagefile-backed section of ~15.8 MiB: a 4 KiB header and four
  slots of up to 1920×1080 YUY2. Header: magic `CBVC`, version, sizes, producer
  pid and session, `generation`, `state` (init / idle / live / stopping),
  `idle_reason` (cam off / no camera / camera busy / closing), a producer
  heartbeat (QPC), `latest_frame`, a reader heartbeat (Windows 10 filter), the
  preferred format. Each slot: a seqlock counter, frame number, capture QPC,
  duration, fourcc, size, plane offsets and strides, colour matrix, flags.
- Writer: bump the slot's seqlock to odd, write, bump to even, publish
  `latest_frame`, heartbeat, set the event. Reader: read `latest_frame`, copy the
  slot, accept it only if the seqlock was even and unchanged, else take the
  previous slot. Each slot carries its own format, so a format change needs no
  re-mapping; the reader converts to what its client negotiated.
- **Names and rights.** A normal user cannot create `Global\` objects. ClippiBoy
  creates `Local\ClippiBoy.VCam.v1` (+ `.Frame` event) in its own session with a
  DACL for the user, SYSTEM and read-only for LocalService; the Windows 11 source
  opens it as `Session\<id>\ClippiBoy.VCam.v1`. That prefix works in practice but
  Microsoft calls it reserved, so a named pipe `\\.\pipe\ClippiBoy.VCam.v1.<session>`
  is the specified fallback. Only the user's own processes, SYSTEM and services
  can read the frames — no other user, no AppContainer.
- The full layout with offsets is in `docs/extras/research/virtual-camera.md`,
  section 7.3; the plan turns it into a small crate with a C header for the
  Windows 10 filter and seqlock tests.

### 4.6 When ClippiBoy is closed

- **Windows 11, Session lifetime:** the camera disappears with ClippiBoy, and an app
  using it gets "device invalidated". That is acceptable: the real camera is free
  then and Discord can use it. If a crash leaves it listed, the source shows
  "ClippiBoy is not running" until the next start.
- *System* lifetime would keep it listed with a placeholder, like OBS on Windows
  10; whether that needs admin and survives a reboot has to be tested first —
  later option.
- **Windows 10:** always listed, placeholder picture without ClippiBoy.
- Reader side: no section, `state = stopping`, or a heartbeat older than 1 s → the
  "ClippiBoy is not running" picture, retry every second.

### 4.7 Risks and size

- A crash in the source takes all cameras down → panic guards everywhere, no
  `unwrap` on the COM path, a fuzzed protocol reader, a soak test of several hours
  with Discord.
- App quirks reported by others: Teams' remote side black, Chrome's MF flag, the
  NV12 gap of libwebrtc, two listings if both paths are registered.
- To test on the 26200 machine before building: does `Session\<id>\` open from
  Frame Server with that DACL, which format Discord picks, does a 32-bit
  DirectShow app see the MF camera, System lifetime without admin.
- Effort, one developer: about 6–8 weeks (Windows 11 source 2–3 weeks, Windows 10
  filter 1.5–2 weeks, protocol + producer, helper + installer, a compatibility
  matrix).

## Section 5: settings and console

Questions from the draft: exact settings, defaults, console preview and its cost
while gaming.

### 5.1 Settings

`AppConfig.facecam: FacecamConfig`, a container default like `FriendsConfig`
(`model.rs:313-360`), so old configs load and the camera stays off after an
update:

| Field | Type | Default | Range / note | Takes effect |
|---|---|---|---|---|
| `enabled` | bool | **false** | the master switch ("Cam off" when false) | live: opens or closes the camera |
| `device_id` | Option<String> | None = first camera | symbolic link | live (reopen) |
| `device_name` | Option<String> | None | fallback when the id changed after a replug, like the screen picker (`shared.tsx:19-48`) | — |
| `resolution` | enum `P720` / `P1080` | P720 | the cam track's size (sharing mode cannot choose the camera's format; this is what it is scaled to) | at the next start of the cam track |
| `fps` | u32 | 30 | 30 only for now (the field exists for a later 60) | — |
| `layout` | `CamLayout` | visible, bottom right (1, 1), 0.25, `Rounded`, no mirror | section 3.1 | read at save |
| `portrait` | `PortraitLayout` | 0.35, 0.5 | section 3.1 | read at export |
| `delay_ms` | i32 | −40 | −300 … 300, step 10 | read at save |
| `in_clips` | bool | true | | live |
| `in_recordings` | bool | true | stage 4 | at the next recording |
| `in_screenshots` | bool | true | stage 4 | at the next screenshot |
| `console_preview` | bool | true | | at the next console opening |
| `virtual_camera` | bool | false | stage 5; switching on asks for the one UAC prompt | live |

- **The block loads leniently.** Today any value the parser cannot read throws
  the whole config away and the defaults are saved over it (`config.rs:167-170`,
  review CFG-2). The `facecam` field therefore gets its own
  `deserialize_with` that turns an unreadable block into `FacecamConfig::default()`
  with a log line, so a bad cam value can never cost the user all other settings;
  `CamShape` has a `#[serde(other)]` fallback. Tested with a broken block in an
  otherwise valid config.
- All numbers are clamped in Rust before they can reach ffmpeg, in a pure
  `clean_facecam` next to the 8d18a92 validators (`commands.rs:303-325`), called
  in `replace_config` so every write path is covered, not only `set_config`.
  Nothing numeric is clamped there today. It never enumerates devices (that is
  slow and belongs to the camera's own thread).
- A change is applied in `replace_config` (`state.rs:419-457`), not only in
  `set_config`, because the console's switch writes through another command. The
  camera is opened and closed on its own thread, never the main thread
  (`state.rs:355-360`).
- The console's audio changes are undone by the main window's next settings write
  today (review CFG-1); the same would happen to a "Cam off" from the console.
  Emitting `config-changed` alone does not fix it: a `set_config` already in
  flight still carries the old value, and the main window's own writes would echo
  back and reset newer optimistic edits. The fix that is part of stage 2: a config
  **revision number** that every `set_config` sends back and the core checks (an
  older revision is merged field by field onto the current config instead of
  replacing it), plus `config-changed` sent to the main window only from the
  write paths it did not start.

### 5.2 The Facecam tab

Built like `Banner.tsx` (`Card`, `Row`, `Toggle`, `Segmented`, `CornerPad`), with
`Slider` (`src/components/ui/Controls.tsx:91-130`) using `onCommit` like
`Recording.tsx:406`, so a slider writes once when let go; registered in
`settings/index.tsx:17-26`:

1. **Facecam** — master switch; under it a live preview (the same pull command as
   the console, 5.3) with a status line: *Shared with other apps* / *Taken by
   Discord* / *Blocked by Windows privacy* / *No camera*.
2. **Camera** — device picker (refresh on opening the tab, never in the store's
   init, enumeration can be slow) with one line that OBS Virtual Camera and other
   DirectShow-only cameras are not supported yet (Open for Fabi 13), resolution.
3. **Where it appears** — clips, console preview (stage 2); recordings and
   screenshots appear with stage 4, not before *(design call: no switches that do
   nothing)*.
4. **Default position** — the four-corner pad (`CornerPad`, `shared.tsx:79-121`,
   with a cam-shaped mark), size slider, shape, mirror.
5. **Sync** — the delay slider with one line on the clap test.
6. **Virtual camera** (stage 5) — the switch, one line on what it does and that it
   asks for administrator rights once.

Copy is short and in English, like the other tabs.

### 5.3 Console preview

- **Transport:** a narrow async command `cam_preview_frame(after: u64)` that
  returns the newest small frame newer than `after` as binary
  (`tauri::ipc::Response`, the sequence number in the first 8 bytes). The page asks
  for the next frame after drawing one, so it stops by itself when the page
  stops; there is no backlog. Not `getUserMedia` (it would open the camera a
  second time and bring a permission prompt into an always-on-top window), not a
  custom protocol (it bypasses the per-window permissions), not a pushed channel
  (no back-pressure; with `panic = "abort"` a stuck page could grow memory until
  the app dies).
- **Size and rate:** 320 px wide, at most 15 fps, made from the raw camera frames
  (not from the H.264 ring); BGRA or JPEG, decided by measuring.
- **Stops twice:** the page stops asking when the console closes (`close()`,
  `console-closed`, effect cleanup, `Console.tsx:322`, `363`, `444`), and the core
  refuses when an atomic `CONSOLE_SHOWN` (set in `console::open`, cleared in every
  hide path, `console.rs:90-104`, `205-234`) is false. The second matters because
  the console webview is never throttled while hidden (`overlay.rs:53-58`).
- **Where:** drawn where the cam will sit, at its default position and size — the
  console covers the whole screen (`console.rs:344-346`), so the preview shows
  exactly where the cam lands in the clip.
- **Not while the console is visible in screen recordings**
  (`console_in_capture`): it would land in the clip next to the composited cam
  and in every Discord screen share. The tile then shows a hint instead.
- **Cam off button** next to it, through a narrow `set_facecam_enabled` command
  granted to the console and the main window only (the console has no
  `set_config`, `capabilities/console.json`). It must not trigger the console's
  relayout, which resets it (review CON-6).
- **Cost:** only while the console is open, normally seconds: a 320 px downscale
  and copy per frame and a canvas draw on the console's CPU renderer (the console
  runs with `--disable-gpu`, `overlay.rs:53-58`). Compared in stage 2 against the
  console's own clip playback, which is already decoded on the CPU.

### 5.4 Diagnostics

The 5-second health line (`lib.rs:1091-1103`) and the status payload get the cam:
frames per second received and emitted, duplicates, gaps, clock kind, encoder
kind, ring seconds. Without it, a cam stall on someone else's machine cannot be
diagnosed.

## Result of the stage-1 test

Run on 2026-10-29 on Windows 11 25H2 (26200.9711), camera privacy on, the
multi-app switch off (its default), with a Logitech C920 (046d:08e5, the camera
Discord uses) and an eMeet C960 (328f:006d), both registered under
`KSCATEGORY_VIDEO_CAMERA`, and Discord desktop app-1.0.9265. A throwaway probe
(`scratch/`, never committed) opened the camera three ways: the MF sensor group
in shared mode, the 26100+ share-mode attribute, and the plain, controlling way.

| Case | Result |
|---|---|
| Probe alone, C920 | all three ways work; the camera then runs YUY2 640×480 at 30 fps, which is what a sharing reader gets when nobody controls it |
| Probe alone, C960 | all three ways work; NV12 1920×1080, about 15 fps in room light |
| Discord's video on first, then the probe | every way fails on its first read with `0xC00D3704`; ffmpeg's DirectShow capture fails too |
| Probe (shared sensor group) first, then Discord | Discord: "Camera couldn't be started, error 2011"; the probe keeps 30 fps for 90 s |
| Discord's capture API | `Discord.exe` loads `ksproxy.ax` and `mfksproxy.dll`: DirectShow |
| Clock | `MFSampleExtension_DeviceTimestamp` on both cameras, equal to the reader's sample time on the QPC clock: `ClockKind::Device` |

The row "A2 fails" of the stage-1 plan's Task 6 applies: a sharing reader does
not share with Discord desktop in either order, and it blocks Discord when it
comes first. The multi-app switch and Windows 10 were not tested: Fabi chose to
go straight to the virtual camera.

**Virtual-camera spike, the same day** (stage 5, Task 1, `scratch/vcam-spike`):
a Rust media source DLL in Program Files with an HKLM CLSID, and a host that
creates a Session/CurrentUser camera with `MFCreateVirtualCamera` (no admin)
and hands it frames through a section in its own session, which Frame Server
opens as `Session\1\…` with LocalService read access.
- Windows lists it as "ClippiBoy Camera (Windows virtuelle Kamera)", with a
  localized suffix of its own, in Media Foundation and in DirectShow;
  DirectShow sees YUY2 and NV12 at 1280×720, 1920×1080 and 640×360, 30 fps.
- **Discord desktop shows the C920 through it while the host holds the C920
  controlling**; Discord negotiated NV12 1280×720.
- Only Frame Server and the host load the DLL. A host killed hard: the camera
  leaves both lists at once.
- The Start callback reports SOURCE_INITIALIZE/UNINITIALIZE when apps enumerate
  and SOURCE_START/STOP while a client streams. FrameServer's service SID type is
  unrestricted.

**Consequences:** Open for Fabi 3 is decided, (a). Stage 2 opens the camera
controlling, with a format it chooses (2.2): sharing mode neither lets Discord
in nor gets a usable picture from the C920 alone. Until the virtual camera
exists, the Facecam tab says that other apps cannot use the camera while the
facecam has it.

## Open for Fabi

Most important first. Each has a recommendation; the design above follows the
recommendation until Fabi decides otherwise.

1. **When the cam is added to a clip.** (a) Two phases: the clip is saved as today
   and the cam is composited in the background a few seconds later
   (*recommended*: a save is never slower than today, a second highlight is never
   lost; the price is a few seconds in which the file has no cam). (b) Composite
   inside the save: the file has the cam from the first moment, but a 2-minute
   1440p clip takes ~45 s during which no other clip can be saved.
2. **Where the clean picture lives, and what it costs.** It doubles every clip with
   a visible cam. (a) In `originals/<id>/` on the data drive, with the trims
   (*recommended*: one place for "the untouched recording", `Free up space` already
   exists). (b) A separate `facecam/<id>/` store (fewer changes in `edit.rs`'s trim
   logic, two concepts side by side). (c) Next to the clip on the clip drive, like
   the recordings' `.recording/` folder. Also: keep it forever (*recommended*, the
   user frees space) or drop it automatically after N days.
3. **If the stage-1 test shows Discord blocks the shared reader:** (a) virtual
   camera right after stage 2, Windows 11 only (*recommended*); (b) ship stage 2
   as planned with a clear "taken by Discord" state (and, if a sharing reader
   even blocks Discord, a warning on the master switch), virtual camera at stage 5
   as planned.
   **Decided (Fabi, 2026-10-29): (a).** Stage 1 found the block in both orders;
   the virtual-camera spike works with Discord.
4. **Virtual camera needs administrator rights once and code signing.** The
   per-user, admin-free install stays for the app; only the virtual camera is
   machine-wide. Acceptable? (*recommended: yes, opt-in, with a code-signing
   certificate bought for it — Smart App Control blocks the DLL otherwise.*)
5. **Windows 10 virtual camera at all?** (a) Windows 11 only, Windows 10 users use
   the real camera with whatever sharing works (*recommended* for the first
   release); (b) the C++ DirectShow filter as well, about two more weeks.
6. **The watermark and the cam.** The mark sits bottom left in the game picture.
   (a) Default cam bottom right; a cam the user moves over the mark simply covers
   it (*recommended*). (b) Keep the cam out of that corner. For portrait exports:
   (a) when the watermark is on and the 9:16 window cuts the mark out of the game
   picture, draw it again on the 9:16 frame (*recommended*; a window that keeps the
   bottom-left corner keeps the original mark), (b) leave it out of portrait.
7. **The cam's memory and the user's memory limit.** (a) The cam has its own
   budget and its own line on the Recording page (*recommended*). (b) It comes out
   of `memory_mb`, so the limit stays the total.
8. **Should the cam stop behind the lock screen during a recording?**
   (*recommended: yes*, the game pipeline keeps running for the recording, the cam
   does not.)
9. **Default shape and mirror.** *Recommended:* rounded corners, not mirrored (what
   others see is what the camera sees).
10. **The layout's place in the database.** Building block 4 says "in the stored
    edit data". (a) Its own JSON column `facecam` next to `edit` (*recommended*:
    the three places that rebuild `ClipEdit` cannot drop it by accident). (b) A
    field in `ClipEdit`, carried through every rebuild by hand.
11. **May ClippiBoy take control of the camera for a moment to get a good format?**
    In sharing mode it gets whatever format the camera is in, and nobody knows yet
    what that is when ClippiBoy is alone with it (over USB 2.0, YUY2 gives 720p at
    only ~10 fps). (a) When no other app streams the camera, open it controlling
    for a moment, set 720p30 MJPEG, then switch to sharing — the stage-1 probe's
    `prime` test shows whether the format survives; the price is that Discord can
    fail once if it starts in exactly that second (*recommended if `prime`
    works*). (b) Never control it; take what comes and scale.
    **Overtaken by stage 1:** sharing lets nobody in either, so stage 2 controls
    the camera for as long as it runs and picks 720p30 or 1080p30 itself (2.2).
    A ruling made while building stage 2; Fabi can still overrule it.
12. **Recordings and screenshots: movable or burned in?** Screenshots: keep the
    clean PNG so the cam can be moved later (*recommended*, it costs little).
    Recordings: (a) burn the cam in at stop with the default layout and keep no
    clean copy (*recommended*: a recording "keeps no untouched copy when trimmed",
    `model.rs:703-706`, because it can run for an hour); (b) keep the clean
    recording like a clip's, doubling multi-GB files.
13. **Cameras that Media Foundation cannot see.** Sharing mode only works for
    cameras Windows registers as cameras (`KSCATEGORY_VIDEO_CAMERA`); DirectShow-only
    virtual cameras such as OBS Virtual Camera are invisible to it. Streamers use
    them. (a) v1 supports what MF sees; the picker explains that OBS Virtual Camera
    and similar are not supported yet (*recommended*). (b) A DirectShow capture
    backend as well — each app gets its own filter instance there, so it would not
    block anyone; about a week more.

## Ideas for later

- Cam jumps or glides to another position during the clip.
- Frame and shape: round, rounded, in the accent colour, crop and zoom; mirror.
- Background blur or removal (Windows Studio Effects where available).
- Position per game (e.g. bottom left in Valorant).
- Privacy: the cam only runs while the buffer runs; a console switch turns it
  off instantly; optionally leave it out of recordings.
