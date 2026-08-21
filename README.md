# ClippiBoy

A clip recorder for Windows: a replay buffer like Medal's, but with an audio
system that handles several sources at once — output devices, individual
applications (process loopback) and microphones, each source either in the main
mix or on a track of its own in the clip.

## Status

| Area | Status |
|---|---|
| UI shell, design system, all screens | ✅ done |
| Device, process and monitor/window enumeration | ✅ done (Windows) |
| Encoder detection via DXGI (NVENC/AMF/QSV/x264) | ✅ done |
| Configuration + clip database (SQLite) | ✅ done |
| Replay ring buffer (keyframe-safe, tested) | ✅ done |
| Video capture (Windows.Graphics.Capture, zero-copy) | ✅ done |
| Encoder (Media Foundation, H.264 + AAC) | ✅ done |
| WASAPI capture of every source type + mixer | ✅ done |
| Save clip (packet ring → MP4, no re-encode) | ✅ done |
| Global hotkeys | ✅ done |
| In-app clip player | ✅ done |
| Tray icon, close to tray | ✅ done |
| Banner over the game | ✅ done |
| Game detection (process exe) | ✅ done |
| Individual tracks beside the clip, mix changeable afterwards | ✅ done |
| Icon, installer (NSIS), auto-update | ✅ done |
| Buffer automation (at startup / in game) | ✅ done |
| Clip editing: name, description, track mix, trim | ✅ done |
| Upload | ⏳ open |

## Developing

Builds happen **on the Windows side** (WSL cannot build Windows binaries).
In PowerShell:

```powershell
cd C:\Users\fabia\projects\clippiboy
npm install
npm run ffmpeg       # fetches ffmpeg.exe/ffprobe.exe into src-tauri\resources
npm run app          # Tauri dev mode with hot reload
npm run app:build    # NSIS installer into src-tauri\target\release\bundle
```

Requirements: Node 20+, Rust (MSVC toolchain), Visual Studio 2022 Build Tools
with the C++ workload, and WebView2 (pre-installed from Windows 11 on).

ffmpeg does **not** have to be installed: `npm run ffmpeg` puts a tested build
next to the app, and that one takes precedence over any on the PATH.

Hotkeys: `Ctrl+Shift+B` buffer on/off, `Ctrl+Shift+S` save clip. Both can be
bound to any key in the settings — including one with no modifier, `F9` for
instance. A single letter key then applies everywhere, chat included.

The ✕ does not close the app but puts it in the tray — from there it can be
reopened, the buffer switched on and off and a clip saved; the tooltip shows the
buffer level and the detected game. Quitting happens via "Quit" in the tray
menu.

### UI only (works on Linux/WSL too)

```bash
npm run dev          # http://localhost:1420 with mock data from src/lib/mock.ts
```

### Checking without Windows

```bash
cd src-tauri
cargo test --lib                            # buffer, mixer and DB logic
cargo check --target x86_64-pc-windows-gnu  # type-checks the Windows code too
```

If the type check aborts with `Inconsistency detected by ld.so` inside a build
script, the target directory sits on the Windows disk — WSL cannot start every
binary from there. Redirect it once:

```bash
export CARGO_TARGET_DIR=~/.cache/clippiboy-target
```

## Shipping and updating

`npm run app:build` produces `ClippiBoy_<version>_x64-setup.exe` — one file you
can send around. It installs per user (no administrator needed), creates a Start
menu entry and brings ffmpeg along; on the other end all it needs is WebView2,
which is present on Windows 10/11. Because ffmpeg is in the package, the
installer is about 90 MB.

**Updates** come from the GitHub releases of `EinFabo/clippiboy`. Every package
is signed; the public key sits in `tauri.conf.json`, the private one under
`%USERPROFILE%\.clippiboy\updater.key` with its password beside it in
`updater.password`. Both belong in repository secrets and **not** in the repo:

| Secret | Contents |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | contents of `updater.key` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | contents of `updater.password` |

If the key is lost, no existing installation can accept an update any more —
then all that is left is sending everyone a new installer.

Publishing:

```powershell
# 1. Bump the version in src-tauri/tauri.conf.json and package.json
# 2. Tag and push — the workflow builds, signs and publishes
git tag v0.2.0
git push origin v0.2.0
```

Building and signing locally by hand (the password has to be set, otherwise the
build asks for it interactively):

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = "$env:USERPROFILE\.clippiboy\updater.key"
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = Get-Content "$env:USERPROFILE\.clippiboy\updater.password"
npm run app:build
```

The app asks once at startup whether there is something newer, but installs
nothing by itself: on Windows, installing means the app quits and the installer
starts — in the middle of a recording that would be exactly the wrong thing. The
find shows up in the settings under "Version", where the installation runs at
the press of a button (a running buffer is stopped cleanly first).

## When the buffer runs

By default not on its own — the buffer goes on via hotkey, tray or the button in
the app. Under "Behaviour" in the settings that can be changed:

* **Start the buffer automatically** — the automation takes over.
* plus **Only buffer in game** on: the buffer starts as soon as a game is
  detected in the foreground and stops 30 seconds after none is left. The half
  minute of run-on is deliberate: a brief alt-tab must not choke the recording.
* plus **Only buffer in game** off: the buffer runs from ClippiBoy's start
  onwards, whatever is in the foreground.

What the user switches themselves always wins: a buffer started by hand is never
stopped automatically, and one stopped by hand is not restarted two seconds
later — the automation holds back until the game has ended.

**Start with Windows** registers ClippiBoy for auto-start, and it then starts
hidden in the tray (`--autostart`). Together with the automation, recording runs
without you ever seeing a window.

## Layout

```
src/                     React UI
  lib/types.ts           IPC types — counterpart to src-tauri/src/model.rs
  lib/ipc.ts             typed command wrappers
  lib/mock.ts            mock data for browser mode
  store.ts               Zustand store, talks to the core
  routes/                Overview, Clips, Audio mixer, Recording, Settings
  components/ClipPlayer.tsx   player over the gallery
  components/ClipEditor.tsx   editing pane: metadata, tracks, export
  components/ui/Menu.tsx      right-click menu (one menu, global)
  components/clipMenu.tsx     its entries for a clip
  components/TextMenu.tsx     WebView menu off, own menu in text fields
  components/SourceTrouble.tsx audio sources that do not run, or run doubled
  lib/useClipMix.ts      play the extra audio tracks in sync with the video
  overlay/               a window of its own: the banner over the game
src-tauri/src/
  model.rs               shared data types
  pipeline.rs            capture → encoder → packet ring
  wgc.rs                 Windows.Graphics.Capture, frames as D3D11 textures
  gpu.rs                 D3D11 device, shared by capture and encoder
  convert.rs             BGRA→NV12 on the GPU + clock for true CFR
  mft.rs                 H.264 encoder as a Media Foundation Transform
  buffer.rs              keyframe-safe packet ring (+ unit tests)
  muxer.rs               packets + audio tracks → MP4 (ffmpeg, no re-encode)
  stems.rs               store, extract and mix individual tracks (+ tests)
  edit.rs                rewrite a clip: mix + trim (+ tests)
  preview.rs             throwaway helper files (waveform for the timeline)
  audio/capture.rs       WASAPI per source (device, loopback, process)
  audio/engine.rs        running streams, levels, mixing
  audio/ring.rs          ring buffer per source (+ unit tests)
  audio/devices.rs       WASAPI endpoints and processes with an audio session
  audio/mod.rs           track layout, gain (+ unit tests)
  capture.rs             monitors and windows as capture targets, incl. Hz
  game.rs                game detection (+ unit tests)
  games.json             exe → game name
  tray.rs                tray icon, menu, tooltip
  overlay.rs             driving the overlay window
  encode.rs              encoder detection
  clipboard.rs           Windows clipboard: file (CF_HDROP) and text
  clips.rs               SQLite clip index
  filing.rs              order in the clip folder: one folder per game (+ tests)
  thumbs.rs              thumbnails (in the data directory, not beside the clip)
  config.rs              configuration as JSON
  commands.rs            Tauri commands
  updater.rs             update check and installation
src-tauri/examples/
  record-probe.rs        run capture → encoder → muxer once by hand
  tracks-probe.rs        two parallel track requests on the same clip
  trim-probe.rs          trim, measure, undo, measure
scripts/fetch-ffmpeg.mjs fetch ffmpeg/ffprobe for the package
scripts/make-icons.py    every icon size from icons/icon.png
```

## Resolution and frame rate

Both follow the chosen source. The resolution is capped at the source's height
and the width computed from its aspect ratio, not from an assumed 16:9. The
frame rate offers the usual steps only up to the screen's refresh rate — plus
that rate itself, so a 165 Hz panel really does give 165. Capturing more frames
than the screen puts out does not make any motion smoother; it only produces
duplicate frames that cost bitrate. For a window, the screen it sits on counts.

The core straightens out a setting that no longer fits
(`capture::fit_to_target`) — switching from a 165 Hz monitor to a 60 Hz second
screen leaves you with 60 there rather than a number the panel can never show.

## Game detection

Detection goes by the **process** behind the foreground window, not by its
title: titles change during play, and many games do not set one at all. If the
exe is in `src-tauri/src/games.json`, the name is certain. An unknown
application counts as a game when its window fills the whole monitor — the
tidied-up window title is used then.

Custom names without a rebuild: put a `games.json` into
`%APPDATA%\ClippiBoy\`; it is layered on top of the built-in list.

```json
{ "mygame.exe": "My Game" }
```

The check runs every two seconds. On save, what counts is the game that was
running *while buffering* — otherwise the clip would say "ClippiBoy" whenever
you save it via the button in the window.

## The banner over the game

A second, transparent window that stays on top, takes no focus and catches no
mouse clicks. It appears on the monitor currently being played on and reports
saved clips, buffer on/off and errors — each of them switchable off individually
in the settings.

The banner sticks to a fixed screen (configurable, default: the primary one) so
it does not jump between monitors; optionally it follows the foreground window.
The fade-in is an outline that draws itself once around the card and then fades
out — pure compositor animation, so it does not stutter when the game needs the
GPU.

Over a game in **exclusive** fullscreen it cannot appear: that would need a
present hook inside the game process, and that is exactly what ClippiBoy
deliberately does not do. In borderless fullscreen and windowed mode — so in
practically every current game — it works.

## How the replay buffer works

**One** encoder runs continuously, and its finished packets land in a ring in
memory (`buffer.rs`). On save the matching packets are cut out, written as a raw
H.264 elementary stream and packed together with the audio into an MP4 in a
single ffmpeg run (`-c:v copy`) — nothing is re-encoded, a clip is ready in one
to two seconds.

The ring always cuts at a keyframe at the front: a clip starting in the middle
of a group of pictures would have blocky artefacts at the beginning. Older
packets fall away continuously, so exactly the configured buffer length is kept.

The video goes into the hardware encoder directly as a Direct3D texture — it is
never copied through the CPU. Capture and encoder share the same D3D11 device
for that (`gpu.rs`), and the graphics card's video processor does the BGRA→NV12
conversion.

The buffer used to sit on disk as 10-second **MPEG-TS segments**, stitched
together with `ffmpeg concat` on save. That had three costs, all of them now
gone:

* Every segment change needed a new encoder. Setting one up takes longer than a
  frame interval, so the next one had to be pre-built in the background — and
  even then a hole of 60 to 300 ms tore into the video at every boundary, adding
  up over a clip into a drift between picture and sound.
* A segment that no longer produced a file — because not a single frame arrived
  with a still picture, say — made **every** further save fail as long as it was
  in the ring: `Impossible to open '…/segment_001124.ts'`.
* The buffer was constantly on disk. An older version that crashed left it lying
  there; on the first start of a new one `buffer/` is therefore cleared away (on
  one test machine, 323 MB).

Audio and the buffer's clock hang off a thread of their own that runs every
10 ms — not off the frame callback. Windows.Graphics.Capture only delivers a
frame when the picture changes: if everything hung off the callback, a still
picture would starve the audio and stall the buffer. For the same reason a
thread of its own clocks the frames at `1/fps` and sends the last one again with
a still picture — the encoder therefore sees true CFR instead of a frame rate it
would have to convert itself. That was exactly the judder. The zero point for
both tracks is the first frame that arrived, so audio and video share the same
time origin.

The audio sources' ring buffers run from program start (for the level meters)
but are emptied before every recording and limited to 40 ms of lag while it
runs. Without that they would hold old audio nobody ever collected — the clip
would run behind the picture from the very first second.

## The audio system

Three source types, combinable at will:

* **Output device (loopback)** — an endpoint's complete audio
* **Application** — process loopback via `ActivateAudioInterfaceAsync`
  (Windows 10 build 20348+), e.g. Discord separate from the game
* **Input device** — microphone

Every source has gain, mute, solo and a live level. Sources without "own track"
run into the main mix, the others are written along in parallel as PCM and muxed
into the MP4 as extra audio tracks on save — so Discord, for instance, can be
muted afterwards in an editor. It works without an editor too: see "Editing
clips".

A source with its own track keeps that track as long as it is enabled — even
when it is muted or another one is soloed. The mixer pushes silence into it
then. Anything else would mean that muting throws the source out of the buffer
**retroactively** too, and that every step of a volume slider costs the minutes
of that track buffered so far.

## Editing clips

In the player, **Edit** (or `E`) opens a pane beside the picture:

* **Name, description, game** — these land in the clip database, not in the file
  name; the file keeps its own. The gallery's search finds all three. The name
  can also be changed without the player: click the name in the gallery, it
  comes up selected, Enter saves, Escape discards. Changing the game while the
  gallery filters by it still leaves you on your clip in the player: the playlist
  is frozen when it opens.
* **Audio tracks** — one slider per track from −30 to +12 dB, plus a mute
  switch.
* **Trim** — `I` and `O` set start and end to the current position, and the
  handles on the timeline can be dragged too. Playback jumps back to the start of
  the selection when it reaches the end.
* **Save** — writes both into the file: the mix **and** the trim. What is in the
  folder afterwards is the finished clip — you can send it as is.

## The right-click menu

WebView2's built-in menu — "Back", "Reload", "Print", "Inspect" — is switched
off throughout the app. It offers not a single useful action and looks like a
browser that got lost.

Two menus of our own in the UI's style take its place:

**On a clip** (tile in the gallery, picture in the player): Open · Open in
default player · Rename · Favorite · **Copy clip** · Copy path · Show in
folder · Delete. "Copy clip" puts the **file** on the clipboard, not its path —
in Discord or WhatsApp, Ctrl+V then attaches the clip; in Explorer it drops a
copy. The format for that is `CF_HDROP`, and no WebView can do it: it comes from
`src-tauri/src/clipboard.rs`.

**In text fields**: Cut · Copy · Paste · Select all. The text goes through the
core rather than `navigator.clipboard` too — reading the clipboard asks for
permission in the WebView, and that dialog does not belong in an app that is
native anyway. Pasting goes through the prototype's setter plus an `input`
event, otherwise React would not notice the change and the draft would jump back
on the next render.

The menu deliberately takes no focus (`onMouseDown` swallowed): the text field
underneath would otherwise lose its selection, and the editor would save on blur
mid-operation. So the keyboard (↑/↓/Enter/Escape) runs through the document.
Rendering happens into `document.fullscreenElement ?? document.body` — in the
player's fullscreen everything else is invisible.

## Order in the clip folder

Every game gets a folder of its own, favorites go into `Favorites`, and whatever
has no game stays directly in the clip folder:

```
Videos\ClippiBoy\
  clip_2026-08-18_11-37.mp4      ← no game
  Bodycam\
  Counter-Strike 2\
  Favorites\                     ← everything with a heart, across all games
```

If a clip's game or its heart changes, the file moves along. Two rules for that:

**Nothing is moved until the clip is no longer open.** The player holds the file
during playback; pulling it out from under it would tear the video off. The
gallery catches up as soon as the player closes — and whatever went wrong in the
process (file locked, crash) is cleaned up by `filing::tidy` on the next start.
The gallery is right in the meantime regardless: it reads from the database, not
from the file system.

**Only what lies in the configured clip folder is touched** — directly in it or
one level down. Whoever changes the storage location deliberately leaves their
existing clips where they are; nobody drags them along. Game folders that have
become empty disappear by themselves, the clip folder itself never does.

A **favorite is a category at the same time**: the file lives in `Favorites`,
while inside the app the clip stays findable under its game — both are filters
over the same database. The heart sits on the tile (visible once set) and in the
player.

Game names are made safe for a folder: forbidden characters are dropped, so are
trailing dots and spaces, device names like `CON` get an underscore in front,
and after 60 characters it stops — game names sometimes come from window titles,
and those can be whole sentences.

The **thumbnails** do not sit beside the clip but under
`%APPDATA%\ClippiBoy\thumbs\<clip-id>.jpg`. The clip folder belongs to the user
and should contain nothing but videos — whoever opens it wants to see clips, not
half an image file for each one. Pictures from older versions that still sit
next to the video are moved there by ClippiBoy at startup. After a trim the
picture is recomputed; it is deleted along with the clip.

The trim does not lose anything in the process. On the first real cut the
untouched recording moves to `%APPDATA%\ClippiBoy\originals\<clip-id>\`, and the
editing pane then offers **Undo trim** — one click and the whole clip is back.
Trimming further *inwards* works without undoing; the handles run over the
trimmed file's timeline while the arithmetic happens internally in the original.

Four things worth knowing about this:

**Shortening at the back is lossless, at the front it is not.** If the cut
starts at zero, the video is only copied (`-c:v copy`) and the file is ready in
one to two seconds. A cut at the start, by contrast, has to be frame-accurate —
when copying it slid to the keyframe before it, so up to two seconds too early.
The video is re-encoded for that, with the encoder from the settings and x264 as
a fallback if the hardware refuses (because the buffer is running next door, for
instance). A progress bar shows how far along it is. The arithmetic **always**
works from the original, never from the already-trimmed file — that keeps the
loss at one generation even if you trim three times over.

**A trimmed clip needs twice the space** as long as the original sits beside it.
It disappears as soon as the trim is undone or the clip is deleted.

**The clip has exactly one audio track.** Discord, the browser and most players
stubbornly play back only the first audio track of an MP4 — with microphone and
Discord sitting beside it as separate tracks, as they used to, they were silent
everywhere outside the editor. So the mix can still be changed at any time, the
raw individual tracks sit alongside, one folder per clip under
`%APPDATA%\ClippiBoy\tracks\<clip-id>\`. They belong to the clip and are deleted
with it.

The tracks stay **untrimmed** in the process and are always in coordinates of
the untouched recording. That is deliberate: it means they are never replaced,
there is no second timeline that can run away, and Windows cannot lock an open
file on you. The price is one number that has to be right — the offset between
track and video, and that is exactly `original.startMs`.

**The preview mixes over WebAudio.** WebView2 cannot reach `audioTracks`, so the
UI runs the individual tracks as audio elements of their own in sync with the
video. Their `volume` can only attenuate, never boost — every slider above 0 dB
would have lowered the other tracks instead of raising its own, and turning the
microphone up would make everything else get louder or quieter. So the tracks run
through a small WebAudio graph: one `GainNode` per track, then a master and hard
clipping at ±1 — the same as what happens on save. Preview and finished clip
therefore sound alike.

The tracks live under `asset.localhost` and therefore on a different origin than
the UI; without `crossOrigin = "anonymous"` a `MediaElementSource` would output
**silence**, with no error message at all. In case it happens anyway, an
`AnalyserNode` listens in and falls back to the old route over `volume` after two
seconds without a single sample.
