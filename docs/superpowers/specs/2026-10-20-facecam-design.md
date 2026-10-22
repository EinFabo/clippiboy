# Facecam and virtual camera

Status: draft, 2026-10-20. Decisions and section 1 (building blocks and order)
are settled; sections 2–5 below are still open and need to be designed before a
plan is written.

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

## Open sections (not designed yet)

2. **Data flow when saving:** how the cam ring is cut to the clip window, A/V
   sync with the game picture and audio, the delay slider, what happens when
   the cam drops out mid-buffer, where clean picture and cam track are stored.
3. **Editor:** UI for moving/resizing/hiding the cam, preview while editing,
   portrait export layout.
4. **Virtual camera in detail:** DLL packaging and registration, shared-memory
   format, behaviour when ClippiBoy is closed, Windows 10 path.
5. **Settings and console:** exact settings, defaults, console preview and its
   cost while gaming.

## Ideas for later

- Cam jumps or glides to another position during the clip.
- Frame and shape: round, rounded, in the accent colour, crop and zoom; mirror.
- Background blur or removal (Windows Studio Effects where available).
- Position per game (e.g. bottom left in Valorant).
- Privacy: the cam only runs while the buffer runs; a console switch turns it
  off instantly; optionally leave it out of recordings.
