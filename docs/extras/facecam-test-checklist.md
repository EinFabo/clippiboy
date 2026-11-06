# Facecam: manual test checklist

An extra for the developer, not part of the spec. It turns the design
(`docs/superpowers/specs/2026-10-20-facecam-design.md`) into hands-on checks for
stages 2–5. Stage 1 (the camera sharing test) has its own plan
(`docs/superpowers/plans/2026-10-22-facecam-stage-1-camera-sharing-test.md`) and is
not repeated here.

How to use it:

- Every item is `- [ ] action → expected result`. If the result differs, write down
  what you saw, the row of the matrix you were on, and the log
  (`%APPDATA%\ClippiBoy\logs\clippiboy.log`).
- `<data>` is `%APPDATA%\ClippiBoy`. A clip's untouched sources live in
  `<data>\originals\<id>\` (the newest folder there belongs to the newest clip).
- Items marked *(stage-1)* depend on the stage-1 result, which is still pending.
  Items marked *(Open n)* follow the recommendation for "Open for Fabi" item n; if
  Fabi decides otherwise, change the expected result.
- "Bright" and "dim" are two fixed lamp settings, the same for every run. "Dim"
  means the cam drops below 30 fps (the diagnostics line shows it).
- "Discord works" always means its own preview **and** the remote side of a real
  call (second account or a phone).

---

## 1. Matrix to cover over time

Not every combination. Run each stage's checklist on the first row, then spread
the rest over the rows below as hardware becomes available.

| Dimension | Values | Why it matters |
|---|---|---|
| GPU | NVIDIA, driver **before 551.76** (lower GeForce session limit) | game + cam + composite + OBS/NVIDIA App can run out of NVENC sessions |
| | NVIDIA, driver 551.76 or newer (8 sessions) | the normal case |
| | AMD | no GPU overlay in the pinned ffmpeg: CPU overlay + AMF, the heavier composite |
| | Intel iGPU only | QSV, CPU overlay path |
| | Hybrid laptop (iGPU + dGPU) | the cam's own D3D11 device must sit on the encoder's adapter; on battery and on mains |
| Camera | Logitech C920 **046d:082d** | original, with onboard H.264 (unused) |
| | Logitech C920 **046d:08e5** | 2021 variant; MJPEG quirk at 1080p30 reported on Linux |
| | eMeet C960 | 1080p30, weak in low light → variable frame rate |
| | Laptop built-in MIPI camera | different driver stack; may have Windows Studio Effects |
| | A 4:3 camera | cover-crop into the 16:9 box |
| | A 1080p60 camera | 60 → 30 fps grid, more MJPEG decode CPU |
| Windows | 10 22H2 | no virtual-camera API; no share-mode attribute |
| | 11 23H2 | no multi-app switch |
| | 11 24H2 / 25H2 (26100 / 26200), multi-app switch **off** | the main target |
| | 11 24H2 / 25H2, multi-app switch **on** | one forced format, controls only in Settings |
| Consumer | Discord desktop | probably DirectShow — the key unknown |
| | Discord in the browser | MF controller, like Chrome |
| | Teams | unknown API; reports of a black remote side with virtual cameras |
| | OBS (Video Capture Device) | DirectShow, controlling |
| | Chrome (`getUserMedia` test page) | MF controller |
| Capture target | Monitor | the normal case |
| | Window | odd sizes, resizing, moving between screens |
| Refresh rate | 60 / 144 / 240 Hz | composite time grows with frame count; sync at high fps |
| Resolution | 1080p, 1440p, 4K, ultrawide (3440×1440, 5120×1440) | layout is relative to frame height; composite cost |

Suggested rows, in this order:

| # | Windows | GPU | Camera | Consumer | Target / screen |
|---|---|---|---|---|---|
| R1 | 11 25H2, switch off | NVIDIA new driver | C920 08e5 | Discord desktop | monitor 1440p144 |
| R2 | 11 25H2, switch off | NVIDIA new driver | eMeet C960 | Discord desktop | monitor 1080p60 |
| R3 | 11 25H2, switch on | NVIDIA new driver | C920 08e5 | Discord desktop, OBS | monitor 1440p144 |
| R4 | 11 23H2 | AMD | C920 082d | Discord in browser, Teams | monitor 4K60 |
| R5 | 11 24H2 | Hybrid laptop | built-in MIPI | Discord desktop, Chrome | window, 1080p, on battery |
| R6 | 10 22H2 | Intel iGPU | 4:3 camera | Discord desktop | monitor 1080p60 |
| R7 | 11 25H2 | NVIDIA old driver | 1080p60 camera | OBS + Discord | ultrawide 3440×1440 240 Hz |

Write down for every session: `winver` build, multi-app switch state, GPU and
driver version, camera model + USB hardware id + port (direct or hub), ClippiBoy
version, Discord/OBS/Chrome versions, vendor camera software running or not,
bright/dim.

---

## 2. Stage 2: foundation and clips

### 2.1 Settings tab and master switch

- [ ] Update from the previous release, keeping its config → a "Facecam" tab exists; the master switch is off; the camera LED stays off; every other setting is unchanged.
- [ ] Fresh install, first start → master switch off; no LED, no camera prompt.
- [ ] Open the Facecam tab with the master switch off → no preview; LED stays off.
- [ ] Turn the master switch on, buffer off, tab open → preview appears within a few seconds; LED on; status line reads "Shared with other apps".
- [ ] Leave the tab (go to Clips) with the buffer off → LED goes off within a few seconds.
- [ ] Open the device picker → every camera Windows lists is there; the window does not freeze while the list loads.
- [ ] Pick a second camera → preview switches; the old camera's LED goes off, the new one's on; the choice survives a restart.
- [ ] Quit, move the selected camera to another USB port, start → the same camera is selected (found by name), not "first camera".
- [ ] No camera connected, master on → status "No camera"; buffer runs; clips save as today; the log does not repeat an error every second.
- [ ] "Where it appears" in stage 2 → only *Clips* and *Console preview*; no switches for recordings or screenshots yet.
- [ ] Corner pad: save one clip per corner → cam in that corner, margin 2.5 % of frame height, rounded to an even number (28 px at 1080p, where the default cam sits at (1412, 782), 480×270; 36 px at 1440p).
- [ ] Default size 0.25 → cam height 25 % of frame height (270 px at 1080p, 360 px at 1440p).
- [ ] Drag the size slider and let go → value sticks after a restart; the preview does not stutter while dragging.
- [ ] Delay slider → range −300 … +300 ms in 10 ms steps; default −40 ms.
- [ ] Resolution 1080p, then toggle the buffer → `cam.mp4` of the next clip is 1920×1080 (ffprobe); 720p → 1280×720.
- [ ] Master switch off while the buffer runs → LED off within 1–2 s; the cam line on the Recording page disappears; the next clip has no cam and no "adding cam" mark.
- [ ] Master switch on while the buffer runs, save 5 s later (clip length 30 s) → cam appears only in the last ~5 s; the first part shows no cam (no frozen first frame, no black box).
- [ ] *Facecam in clips* off, buffer on → LED off (unless the Facecam tab is open; only the main window's preview keeps the camera on, the console preview never does); clips without cam.
- [ ] Quit; in `config.json` set facecam `layout.size` to 3, `delay_ms` to 5000, `layout.x` to −2; start → all other settings intact; next clip composites without error; values end up inside their ranges (note whether on load or only after the next settings change).
- [ ] Quit; set `layout.shape` to `"Hexagon"`; start → shape loads as Rect; all other settings intact.
- [ ] Quit; set `resolution` to `"P4K"`; start → **the rest of the config must not fall back to defaults** (today an unreadable config resets everything; check the log for "config unreadable").
- [ ] Press "Cam off" in the console, then change any other setting in the main window → the cam stays off (config revision fix, CFG-1).
- [ ] Change one setting five times quickly in the main window → the last value sticks; nothing jumps back.

### 2.2 Camera lifecycle and privacy (LED)

- [ ] Buffer off, Facecam tab closed, 10 min, opening and closing the console a few times → LED never lights (the console preview only shows frames while the cam already runs for the buffer).
- [ ] Buffer on → LED on; buffer off → LED off within a few seconds.
- [ ] *Start automatically* + *Only buffer in game*: start a game → buffer and LED on; quit the game → both off ~30 s later.
- [ ] Switch the capture target monitor → window while the buffer runs → LED does not blink; a Discord call keeps its video; the next clip has the cam.
- [ ] Change display resolution or refresh rate, or unplug a second monitor, while the buffer runs → the game pipeline restarts; LED does not blink; the cam in the next clip has no hole at the restart.
- [ ] Where Windows shows camera "recent activity" (Privacy & security › Camera) → ClippiBoy appears only for the times something you switched on needed the cam.

### 2.3 Saving a clip (two phases)

- [ ] Save with the cam visible → banner and sound as fast as today (1–2 s); the file is in the folder and plays (without cam yet); the tile shows "adding cam".
- [ ] Wait → the mark goes away; the thumbnail shows the cam; size updates; the clip file now has the cam; its duration equals the clean one (±1 frame).
- [ ] After phase 2 → `<data>\originals\<id>\` holds `cam.mp4`, `facecam.json`, `video.mp4`; no `*.cam.mp4` or `.part` files left in the clip folder or `<data>\temp\`.
- [ ] Play `originals\<id>\video.mp4` → no cam; watermark present if it was on.
- [ ] Compare the audio streams of `video.mp4` and the composited clip (ffprobe) → same count, codec, channels, titles, duration (`-c:a copy`).
- [ ] Open the composited clip in the editor → separate tracks still mixable as before.
- [ ] Stream Deck *Save Clip* → checkmark at the end of phase 1, as today.
- [ ] During phase 2, look at `ffmpeg.exe` in Task Manager › Details → priority "Below normal"; only one composite runs at a time.
- [ ] Longest case: buffer 120 s at 1440p144, save → composite finishes (estimate ~45–55 s at NVENC p3); game stays playable; record the time.
- [ ] Default layout with *Show cam* off → no "adding cam", no composite; the clip is today's clip; note whether `cam.mp4` is kept for showing the cam later.
- [ ] Switch the master on and save within half a second (no cam frame yet) → clip saved exactly as today; no facecam data; no Facecam section later.

### 2.4 Sync and frame rate

- [ ] Clap test (section 7), default delay −40 ms, bright → offset within ±33 ms (one cam frame); record per camera.
- [ ] Clap at the start and at the end of a 120 s clip → the same offset at both ends (no drift).
- [ ] Clap test after the buffer has run 3 h or more → the same offset as on a fresh start.
- [ ] Clap test at 60, 144 and 240 Hz capture → the same offset.
- [ ] Clap test on a run whose diagnostics line says `clock=arrival` (`ClockKind::Arrival`) → default −80 ms is used; offset within ±2 cam frames.
- [ ] Delay −300, save; +300, save → the cam visibly shifts ~0.3 s each way; earlier clips keep their own value.
- [ ] Dim the room until the cam runs ~15 fps → cam motion in the clip is even at half rate (each frame twice); no speed-up, no bursts of stutter; sync still within ±2 cam frames.
- [ ] Switch bright/dim every 10 s during a 60 s clip → no freeze longer than one frame at the current rate; no drift.
- [ ] Cover the lens → dark but continuous cam; not hidden, no gap recorded.
- [ ] 1080p60 camera → cam in the clip is a smooth 30 fps, no alternating judder.

### 2.5 Dropouts, unplug, replug

- [ ] Save a clip 3 s after unplugging the cam → clip saves at normal speed; the cam disappears at the unplug moment (a frozen frame of at most 2 s before it is acceptable; note how long) and does not freeze for the rest of the clip; the Facecam tab says the camera is gone.
- [ ] Replug into the same port → LED on within a few seconds without touching the app; a clip spanning unplug + replug shows cam → nothing → cam, in sync after the gap (clap after replug).
- [ ] Replug into another port or a hub → reopened by name; the picker still shows that camera.
- [ ] Unplug for 1 s and replug → no crash; cam back.
- [ ] Unplug/replug 10 times in 2 min → app stable; cam ends up live; memory flat; log not flooded.
- [ ] Power-cycle a USB hub that carries cam and microphone → both recover; the next clip has both.
- [ ] Plug in a second camera while the first is in use → the selection does not change.
- [ ] Unplug the camera with the master switch off → nothing happens, no notice.

### 2.6 Other apps: Discord before and after *(stage-1)*

- [ ] ClippiBoy cam running, then Discord *Test Video*, then a real call → Discord works; ClippiBoy's preview keeps running; the clip has the cam, in sync.
- [ ] Discord video already on, then start the buffer → either the cam is in the clip, or the status reads "Taken by Discord" and the clip is saved without the cam; Discord's video never freezes or drops (check the remote side).
- [ ] Discord toggles video every 2 min for 30 min while the buffer runs → ClippiBoy's cam never stalls for good; clips across a toggle show the cam continuous or hidden for the gap; never stretched after a format change.
- [ ] Discord changes camera quality mid-buffer → the cam track keeps its size; not stretched or squashed; note an LED blink.
- [ ] For Discord in browser, Teams, OBS, Chrome test page, Camera app: each one first then ClippiBoy, and ClippiBoy first then each → record works / fails / black / frozen per pair; when ClippiBoy started first it must never make the other app fail.
- [ ] OBS holds the camera, ClippiBoy reads → after ~5 s without frames the status names OBS and suggests the multi-app switch (or the virtual camera once it exists); clips save without cam.
- [ ] Windows 11 24H2+/25H2 with *Allow multiple apps to use camera at the same time* on → Discord and ClippiBoy both get a picture; the Facecam tab's hint about the switch shows only where the switch exists (not on 23H2 or 10).
- [ ] Logi Tune / G HUB / eMeet Studio running → record whether it holds the camera; if it does, the status names it.
- [ ] Camera that Media Foundation cannot see (OBS Virtual Camera) → the picker explains it is not supported yet *(Open 13)*.

### 2.7 Camera privacy setting off

- [ ] Turn off *Let desktop apps access your camera* while the buffer runs → cam stops; Facecam tab: "Windows blocks the camera for desktop apps" with a button; the button opens the Windows camera privacy page.
- [ ] Turn it back on → the cam returns (note: by itself within the retry back-off, or only after toggling the master switch); later clips have the cam.
- [ ] Start ClippiBoy with the setting already off → the same message; no crash; no error every second in the log.
- [ ] Turn off *Camera access* for the whole device → the same message.

### 2.8 Sleep, lock screen, UAC

- [ ] Buffer with cam, sleep 1 min, wake, save at once → clip saves; the cam appears from the wake-up on, in sync (clap after wake); nothing before the sleep is shifted.
- [ ] Save 2 min after wake → full cam, in sync.
- [ ] Hibernate instead of sleep → the same.
- [ ] Sleep while a composite runs → after wake the job finishes or restarts; the clip ends up with the cam.
- [ ] A camera that re-enumerates after wake → reopened; status OK within a few seconds.
- [ ] Lock (Win+L) for 5 s, unlock → buffer and cam keep running; LED never goes off.
- [ ] Lock for 30 s → after ~10 s buffer stops and LED goes off; unlock → both come back by themselves.
- [ ] UAC prompt answered within 3 s → cam and buffer keep running; a clip across the prompt has a continuous cam.
- [ ] UAC prompt left open 20 s → like the lock screen: both stop after ~10 s and return after answering.

### 2.9 Delete, export, share, move while "adding cam"

- [ ] Delete a clip while "adding cam" → after confirming, it is gone; no file reappears in the clip folder; `originals\<id>\` removed; no stray `*.cam.mp4`.
- [ ] *Export a copy* while "adding cam" → the export waits for the job, then contains the cam.
- [ ] *Send to a friend* while "adding cam" → the transfer starts after the job; the receiver gets the clip with the cam; the transfer completes (checksum OK).
- [ ] *Share as link* while "adding cam" → the shared file has the cam.
- [ ] Favorite (heart) or change the game while "adding cam" → the file ends up in the new folder with the cam; no copy left in the old folder.
- [ ] Rename while "adding cam" → name kept; the job is unaffected.
- [ ] Tail-trim and save in the editor while "adding cam" → the save waits; the final file has the trim and the cam.
- [ ] *Copy clip* or drag the file out while "adding cam" → the copy has no cam (accepted); the clip itself still gets the cam.
- [ ] Play the clip in the player while phase 2 swaps it in → playback is not torn; after reopening, the cam is there.
- [ ] Open the clip in the default (external) player during phase 2 → the swap retries or fails cleanly; the clip is never lost; note the state shown.
- [ ] Two saves 3 s apart → both saved at today's speed, neither refused; both get the cam, one after the other.
- [ ] Five saves in 20 s → all five saved; composites queue; all end with the cam; the game stays smooth.
- [ ] Open another clip and save a trim while a long composite runs → that save is not blocked by the composite.
- [ ] Quit from the tray while "adding cam" → the app quits within a few seconds; at the next start the job resumes; the clip ends with the cam; no leftovers.
- [ ] Kill `ClippiBoy.exe` in Task Manager during phase 2 → no `ffmpeg.exe` keeps running for long; at the next start the job is queued again; the clip is never truncated.
- [ ] Hard power-off (VM reset) during phase 2 → after boot the clip plays (clean or with cam), never a broken file.
- [ ] Install an update from *Version* with composites queued → the buffer stops cleanly; after the update the jobs resume.

### 2.10 Failure paths

- [ ] Clip drive with room for the clip but not for clean copy + composite → phase 2 skipped; clip stays clean; a notice says why; no half-written files.
- [ ] Data drive (`%APPDATA%`, usually C:) full, clip drive free → phase 2 skipped with a notice; clip kept.
- [ ] Disk fills up during phase 2 → the composite fails; clip kept clean; state "failed" with the notice "The cam couldn't be added — try again from the editor"; partial files removed.
- [ ] Free space, then retry from the editor → the composite succeeds.
- [ ] Save a long clip, then a short one; while the first composites, overwrite the second's `originals\<id>\cam.mp4` with a few bytes → the second clip fails cleanly (clean clip kept, notice); no crash; retry fails cleanly again.
- [ ] Use up NVENC sessions (OBS recording with NVENC + NVIDIA App Instant Replay + ClippiBoy) and save → the composite falls back to x264 and succeeds (slower); the game still playable.
- [ ] Old NVIDIA driver, sessions used up, then start the buffer → the cam encoder falls back to software (the diagnostics line names the encoder); the game buffer keeps NVENC and is not affected; if software also fails → cam off with a notice, clips as today.
- [ ] Start and stop the buffer 20 times with the cam on (old NVIDIA driver) → both encoders start every time; no session errors (encoder shutdown fix, CAP-V2).

### 2.11 Watermark, shapes, mirror, odd sources

- [ ] Watermark on, default layout, at 1080p / 1440p / 4K / ultrawide → mark bottom left, cam bottom right, neither covers the other.
- [ ] Default position bottom left → the cam covers the mark *(Open 6)*; the mark is not drawn on top of the cam.
- [ ] Watermark on → `originals\<id>\video.mp4` carries the mark too; `cam.mp4` has none.
- [ ] Shapes Rect / Rounded / Circle → clean edges, no grey or black fringe; the circle is round (not an ellipse) on 16:9 and ultrawide; all four rounded corners alike.
- [ ] Mirror on → hold up printed text: it reads backwards in the clip; mirror off (default) → it reads correctly.
- [ ] 4:3 camera → the cam fills its 16:9 box (cropped), face not stretched, no bars.
- [ ] Built-in MIPI camera → listed and working, or a clear status; record whether Studio Effects (blur, eye contact) show up in ClippiBoy's cam.
- [ ] Window capture at an odd size (e.g. 1365×767) → composite succeeds; cam at the right spot.
- [ ] Resize the captured window mid-buffer → the game behaves as today; the cam is not interrupted.
- [ ] Ultrawide 3440×1440 → cam 360 px tall, bottom right, 36 px margin.
- [ ] 4K60, 120 s → composite completes; Task Manager shows the GPU's Video Decode engine busy (GPU decode used); record the time.

### 2.12 Console preview

- [ ] Open the console with the cam running → a live tile at the cam's default position and size, at most ~15 fps.
- [ ] Close the console each way it can close → preview stops; ClippiBoy CPU back to baseline within 1–2 s.
- [ ] Console closed for 10 min while gaming → no CPU from the preview (same as never opened).
- [ ] *Visible in screen recordings* on → the tile shows a hint, not the picture; a clip saved with the console open has no preview tile in it; a Discord screen share shows no preview.
- [ ] *Visible in screen recordings* off (default), save with the console open → no console in the clip and only one cam (the composited one).
- [ ] "Cam off" in the console → LED off within 1–2 s; the Facecam tab shows the master off; the console does not jump or reset its layout (CON-6).
- [ ] *Console preview* setting off → no tile; the cam still lands in clips.
- [ ] Master switch off → no tile and no preview requests (CPU as without the feature).
- [ ] Exclusive fullscreen game → console behaves as today (says it cannot open); no preview requests.
- [ ] Cost: game frame time with the console open with preview vs open without preview → difference within noise; record ClippiBoy and WebView2 CPU for both.

### 2.13 Diagnostics and memory

- [ ] Log with the cam on → the 5-second line has cam fps received and emitted, duplicates, gaps, clock kind, encoder kind, ring seconds.
- [ ] Recording page → its own cam memory line, ~70 MB at 120 s buffer *(Open 7)*; at 300 s → roughly 2.4× that.
- [ ] Ring seconds in the diagnostics → buffer length + keyframe interval + ~11.3 s, rounded up (134 s for a 120 s buffer with 2 s keyframes).
- [ ] ClippiBoy private memory after 10 min, cam on minus cam off → about the cam line plus a few tens of MB; flat over 1 h.

### 2.14 Performance while gaming

Same game, same scene, same settings; each condition three times (section 7).

- [ ] Game frame time, cam off vs cam on (buffer on in both) → average and 1 % low within noise; write down your threshold (e.g. ≤ 2 % on the average).
- [ ] Buffer dropped/repeated frames (log) → cam on equals cam off.
- [ ] ClippiBoy CPU, cam on minus off → record per camera at 720p and 1080p (estimate 10–25 % of one core at 720p30 for MJPEG decode).
- [ ] GPU Video Encode engine → the cam adds only a small share (estimate 1.5–6 % of one NVENC engine).
- [ ] During a phase-2 composite → record the worst 1 % frame time; buffer drops stay at zero; ffmpeg at below-normal priority.
- [ ] AMD (CPU overlay + AMF) and Intel (QSV) → record composite time and game impact.
- [ ] Hybrid laptop → the diagnostics show the cam on the encoder's adapter; game on the dGPU not affected; repeat on battery.

### 2.15 Trims, undo and storage with a facecam clip

Trimming exists before the editor's cam controls, so stage 2 keeps or re-composites the cam with the clip's stored layout (spec 2.7).

- [ ] Front cut (e.g. 3.4 s) on a clip with its cam in, save → composite from the clean picture; the cam where it was, one cam, not two; clap after the cut still in sync.
- [ ] Tail trim only → fast path (1–2 s); cam still there.
- [ ] Mix (track gain) change only → fast path; cam still there.
- [ ] *Undo trim* → whole length back, re-composited with the clip's layout.
- [ ] Untrimmed save of a facecam clip → `originals\<id>\` is kept (unlike a plain clip).
- [ ] Restart the app → facecam originals survive start-up repair (folder with `facecam.json` but no `trim.json`).
- [ ] Delete a facecam clip → `originals\<id>\` (cam, clean video, json) and its tracks are gone.
- [ ] *Free up space* on a facecam clip → says "The cam can no longer be moved in this clip." (on a trimmed one also that undo goes); afterwards the file keeps the cam, *Undo trim* is gone.
- [ ] Storage page → a row "Clean pictures for moving the cam" with its size; baking them all asks first, then the row reads 0 B and every clip keeps its cam.
- [ ] *Clear all trims* → plain clips' trims cleared as before; facecam clips untouched (undo still there, the clean-pictures row unchanged).

---

## 3. Stage 3: editor and portrait export

### 3.1 Opening and playback

- [ ] Open a facecam clip → a Facecam section between Audio and Trim: *Show cam*, shape, mirror, *Reset to default*, *Cam sync*; the player shows one face, not two.
- [ ] Open a clip from before the feature, saved with the cam off, or baked with *Free up space* → no Facecam section (or a disabled one); the editor works as before.
- [ ] Open a clip while "adding cam" → still one face (clip file + overlay).
- [ ] Play a 2-min clip → the overlay stays in sync with game and voice to the end (clap at the end).
- [ ] Seek 20 times, scrub, pause/play → the cam catches up within ~0.5 s every time; no lasting offset; never frozen.
- [ ] Play with a trim set (loops back at the end of the selection) → the cam follows the jump.
- [ ] Close and reopen the clip (resume position) → the cam is at the right moment.
- [ ] A clip with a cam gap → the overlay is hidden for the gap, as in the file.

### 3.2 Move, resize, hide, undo

- [ ] Drag the cam → follows the pointer; snaps to corners and margins; cannot leave the picture.
- [ ] Resize from each corner → ratio stays 16:9 (1:1 for the circle); stops at 12 % and 50 % of frame height.
- [ ] Drag out of the window and release → no stuck drag.
- [ ] Hide (*Show cam* off) → cam gone in the preview; "unsaved changes" shows.
- [ ] Change shape and mirror, save → a frame of the saved file matches the preview at the same moment (position within a few px, same shape and mirror).
- [ ] *Reset to default* → layout from the settings; counts as unsaved.
- [ ] Close the player with an unsaved layout → the same unsaved-changes behaviour as for a trim.
- [ ] `I`, `O`, Space after touching the cam box → work as before.

### 3.3 What Save does

- [ ] Move the cam and save → the file has the cam at the new spot; editor saves of other clips are not blocked meanwhile.
- [ ] Hide, no front cut, save → done in 1–2 s (copy from the clean original); no cam; picture identical to the clean one.
- [ ] Hide + front cut, save → re-encode, no cam.
- [ ] Move and save five times → game picture quality the same as after the first save (one generation); file size stable.
- [ ] Hide, save, show again, save → the cam is back at its layout.
- [ ] *Cam sync* −300 … +300 on one clip → that clip shifts; others do not; the value is kept on reopening.
- [ ] Trim a clip, move the cam and save, then *Undo trim* → whole length back, re-composited with the **current** layout.
- [ ] Change name, description, game, heart after moving the cam → the layout is kept.
- [ ] *Send to a friend* / *Share as link* after moving the cam → the receiver gets the moved version; the layout is still there afterwards.

### 3.4 Portrait export 9:16

- [ ] Export dialog → orientation *Landscape* (default, as today) / *Portrait 9:16*.
- [ ] Portrait at full size → 1080×1920; cam band on top (35 %), game below; the clip file's modified time unchanged.
- [ ] Portrait at each size target (10 MB, 25 MB, …) → short side steps 1080 → 720 → 480 (1080×1920, 720×1280, 480 wide); never 608×1080; file within the target.
- [ ] *9:16* toggle in the editor → preview in portrait; drag the game window sideways and the split between 25 % and 50 %; the export matches the preview.
- [ ] Reopen the clip → split and game position kept.
- [ ] Portrait where the cam is absent (before its start, in a gap) → the band shows a blurred strip of the game, not black, not frozen.
- [ ] Portrait of a clip without facecam → game centred with a blurred fill.
- [ ] Portrait of a trimmed and re-mixed clip → audio is the current clip's (trim and mix applied); same length.
- [ ] Portrait from 4K and ultrawide sources → game not squashed; 4:3 camera fills the band.
- [ ] Clap test on a portrait export → same offset as landscape.
- [ ] Watermark on → the mark is drawn again on the 9:16 frame, readable *(Open 6)*; watermark off → no mark.
- [ ] Game window at the far left → note whether the burned-in mark from the game picture shows next to the redrawn one (two marks).
- [ ] Portrait export of a clip still "adding cam" → waits for the job.
- [ ] Cancel a portrait export midway → no partial file left.

---

## 4. Stage 4: recordings and screenshots

### 4.1 Recordings

- [ ] Facecam tab → *Recordings* and *Screenshots* switches now appear.
- [ ] Record 10 min with the cam → stop finishes as fast as today (clean file); then "adding cam" and a background composite (minutes); the final file has the cam; clap at start and end within ±1 cam frame.
- [ ] Recording longer than the buffer (15 min with a 2 min buffer) → cam from the first to the last minute.
- [ ] Start a recording while the cam already runs → cam from the first second.
- [ ] *Clips* off, *Recordings* on, buffer off → LED on at recording start, off at stop.
- [ ] *Recordings* off → recording clean; no `cam.h264` in `.recording\<id>\`.
- [ ] Toggle *Recordings* during a recording → takes effect at the next one.
- [ ] Save a clip during a recording → both get the cam; composites one after another; the recording continues.
- [ ] Kill `ClippiBoy.exe` mid-recording → the next start recovers the recording as today, with the cam; the composite runs.
- [ ] Unplug and replug the cam mid-recording → cam hidden for the gap; back and in sync after.
- [ ] Lock the screen > 10 s during a recording → recording continues (as today); LED off; no cam for that span; cam back after unlocking *(Open 8)*.
- [ ] Sleep during a recording → the recording as today; cam from the wake-up on.
- [ ] 30 min at 1440p → the Storage page shows no clean copy and no doubled size (the cam is burned in at stop, spec 2.10); not enough space → phase 2 skipped, notice, recording kept clean.
- [ ] 30-min composite while gaming → game playable; buffer drops zero; quit midway → resumes at the next start.
- [ ] Stream Deck *Recording* checkmark → at the clean finish, as today.
- [ ] Editor of a recording → cam movable or not, as decided *(Open 12)*.

### 4.2 Screenshots

- [ ] Screenshot hotkey with the cam running → PNG has the cam at the default layout; as fast as before (record).
- [ ] Clap and press the Stream Deck *Screenshot* key at once → the cam frame shows the moment (within ~2 cam frames).
- [ ] `originals\<id>\` holds the clean PNG; the screenshot viewer can move and hide the cam; crop, marks and blur still work and stay re-editable.
- [ ] Crop through the cam → cam cut off cleanly.
- [ ] Blur over the cam → the blur lies on top of the cam.
- [ ] Watermark bottom left, not covered at the default layout; mirror and shape as in clips.
- [ ] Cam unplugged → screenshot without cam; no error.
- [ ] Cam frozen (no frames for > 2 s) → no stale cam frame in the shot (note what happens).
- [ ] Cam not running (buffer off, no preview) with *Screenshots* on → screenshot without cam; the camera is not opened just for the shot (LED stays off) — confirm this is intended.
- [ ] *Screenshots* off → no cam in the PNG.
- [ ] Ten screenshots in 5 s → all have the cam; no slowdown.
- [ ] Delete a screenshot → its clean PNG is gone too.

---

## 5. Stage 5: virtual camera

### 5.1 Switching on

- [ ] Turn on *Virtual camera* → the app explains the admin prompt first; exactly one UAC prompt; it names a verified publisher.
- [ ] Decline the UAC prompt → switch back off, a short message; nothing registered.
- [ ] Accept → `%ProgramFiles%\ClippiBoy Virtual Camera\<version>\` exists; a normal user cannot create a file in it.
- [ ] *ClippiBoy Camera* (plus Windows' suffix) is listed exactly once in Discord, Discord in browser, Teams, OBS, Chrome, Camera app.
- [ ] Smart App Control on (where available) → the DLL loads; record.

### 5.2 Using it

- [ ] Discord on *ClippiBoy Camera* → Discord works; clips have the cam at the same time; LED on.
- [ ] Facecam tab → tells you to pick *ClippiBoy Camera* in Discord once.
- [ ] Mirror on, watermark on, circle shape in ClippiBoy → the virtual camera shows the plain picture: not mirrored, no mark, no mask.
- [ ] Discord, Chrome, OBS at 720p and 1080p where selectable → correct colours (no green or pink tint), correct aspect.
- [ ] Teams call → the remote side sees the picture, not black.
- [ ] OBS and Discord on *ClippiBoy Camera* at the same time → record whether both show the picture.
- [ ] Camera app → picture.
- [ ] A 32-bit DirectShow app, if one is at hand → record whether it lists the camera.
- [ ] Phone stopwatch in front of the camera; photograph the screen with Discord on the real camera vs on *ClippiBoy Camera* → the virtual camera adds at most 1–2 frames.
- [ ] With the virtual camera on, clips in bright light → cam ≥ 25 fps at 720p (ClippiBoy now controls the real camera's format).
- [ ] Buffer off, Discord uses the virtual camera → LED on; Discord stops its video → real camera closed within a few seconds (LED off).
- [ ] Master switch off while Discord uses it → Discord shows the "Camera is off in ClippiBoy" picture (not frozen, not black); LED off; on again → live.
- [ ] Unplug the real camera while in use → placeholder; replug → live without reselecting in Discord.
- [ ] Another app opens the real camera directly → placeholder "camera busy" or a clear state; record.
- [ ] Discord changes resolution → no freeze in Discord or in clips.
- [ ] Toggle Discord video 20 times in 2 min → no hang; other cameras still work.
- [ ] Sleep/resume while Discord uses it → picture returns (record whether a reselect is needed).
- [ ] Lock the screen > 10 s → record what Discord shows and the LED; live again after unlocking.
- [ ] Soak: a Discord call of 3 h or more on the virtual camera with the buffer on → no freeze; ClippiBoy memory flat; Frame Server's `svchost.exe` memory flat; clips during the soak fine.
- [ ] Second Windows user signed in on the same PC → cannot see the first user's frames.

### 5.3 ClippiBoy closed or crashed

- [ ] Quit ClippiBoy while Discord uses the virtual camera (Windows 11) → Discord loses the camera without crashing; picking the real camera in Discord works at once.
- [ ] Start ClippiBoy again → *ClippiBoy Camera* is listed again; record whether Discord reconnects by itself.
- [ ] Kill `ClippiBoy.exe` while in use → Discord shows "ClippiBoy is not running" or loses the camera; the Camera app still works with the real camera (Frame Server not down); live again after the next start.
- [ ] Windows 10 filter, if built *(Open 5)* → listed without ClippiBoy, shows "ClippiBoy is not running"; start ClippiBoy → live within ~1 s; the Camera app does not list it (expected).
- [ ] Windows 11 with both paths registered → only one listing.

### 5.4 Updates and uninstall

- [ ] App update with the same DLL version → no UAC prompt; the virtual camera still works.
- [ ] App update with a new DLL version → explained in the app, one UAC prompt, never inside the silent updater; the old version folder is gone (or gone after a reboot).
- [ ] Decline the DLL update → the newer app still drives the older DLL, or says clearly why not.
- [ ] Update while Discord uses the virtual camera → nothing crashes; Discord recovers (reselect if needed).
- [ ] Switch *Virtual camera* off → record: listing gone or placeholder, UAC prompt or not; the real camera goes back to sharing mode.
- [ ] Uninstall ClippiBoy → one UAC prompt for the helper; afterwards no *ClippiBoy Camera* in any app; the Program Files folder, `HKLM\SOFTWARE\ClippiBoy\VirtualCamera` and the CLSID key are gone; other cameras work.
- [ ] Uninstall while Discord uses it → no crash; leftovers go at the next reboot; after it nothing remains.
- [ ] Uninstall without ever enabling the virtual camera → no UAC prompt, as today.
- [ ] Reinstall afterwards → switching on works again.

---

## 6. Regression guard: cam off must stay exactly as before

Run on the previous release and on the new build, same machine, same settings,
master switch off (and once with the feature never touched since the update).

- [ ] Save speed: ten saves each → median time from key press to banner equal to the previous release (within ~100 ms).
- [ ] Clip file: ffprobe both (section 7) → same streams in the same order, same codec, profile, level, time base, frame rate, pixel format, audio track count, titles and tags; only duration, size, bitrate and dates differ.
- [ ] MP4 box order (any MP4 box viewer) → the same as before.
- [ ] ffmpeg command line of a save (Process Monitor, section 7) → identical to the previous release except for paths and the clip id.
- [ ] No `originals\<id>\cam.mp4` or `facecam.json` for clips saved with the cam off.
- [ ] Memory: ClippiBoy private bytes after 10 min with a full buffer → same as before within noise; no cam line on the Recording page; thread count about the same.
- [ ] LED never lights; no camera "recent activity" entry for ClippiBoy.
- [ ] Settings migration: start the new build with saved `config.json` files from the previous release and one older release → every setting kept (hotkeys, audio sources, clip folder, buffer and clip length, memory, encoder, watermark, console, Stream Deck port and token, friends sign-in); facecam present and off; no "config unreadable" in the log.
- [ ] Downgrade: run the previous release on the new config → it starts with all settings (no "config unreadable"); note that it may drop the facecam block when it next writes.
- [ ] Clip database: the new build opens an old database → every clip listed, playable, editable; trims undo; the previous release still opens the database after the new build has added its column.
- [ ] Trims of plain clips: front cut, tail cut, *Undo trim*, *Free up space*, *Clear all trims* → same results and times as before.
- [ ] Two editor saves on two different clips at once → both work (per-clip lock change).
- [ ] Landscape export at each size target → the same dimensions and about the same size as before (the portrait rework must not move them).
- [ ] Screenshot and recording speed and files → as before.
- [ ] Buffer behaviour: lock screen, 10 s UAC rule, sleep, monitor change, automation run-on → as before.
- [ ] Capture errors still reach the UI (error channel fix, BUF-2).
- [ ] Encoder diagnostics line → same fields as before (cam fields only when the cam is on).
- [ ] Console without the cam → opens as fast as before; no preview tile; CPU as before.
- [ ] Settings writes: change audio settings in the console, then a setting in the main window → both stick (revision fix must not lose writes).
- [ ] Stream Deck keys, hotkeys, tray, banner, friends sharing, share links → as before.
- [ ] Game frame time with the buffer on → the same as the previous release (section 7).
- [ ] Uninstall → as before, no UAC prompt.

---

## 7. How to measure

**Game frame time.** PresentMon (Intel, free) or CapFrameX (which builds on
PresentMon) record each presented frame of the game. Use one fixed scene or a
built-in benchmark, 60 s per run, three runs per condition, and alternate the
order of conditions. Compare the average frame time, the 1 % low and the worst
spikes. Cap or uncap the game the same way every time.

**Buffer dropped frames.** ClippiBoy's log writes a line every 5 s with the
measured frame rate, dropped and repeated frames and time in the encoder; with the
cam on it also carries the cam's numbers. Compare a few minutes of lines per
condition, not a single one.

**CPU and GPU per process.** Task Manager › Details: add the *CPU*, *GPU* and *GPU
engine* columns; the Performance tab shows the GPU's engines (3D, Video Encode,
Video Decode). Process Explorer shows CPU, private bytes, threads and GPU per
process. Performance Monitor has *GPU Engine* counters per process and engine type
for logging over time. Count every process involved: `ClippiBoy.exe`, its
`msedgewebview2.exe` children (windows, console), `ffmpeg.exe` (composite) and, in
stage 5, Frame Server's `svchost.exe`.

**NVENC sessions.** `nvidia-smi --query-gpu=encoder.stats.sessionCount
--format=csv,noheader` (`nvidia-smi` ships with the driver; the stage-2 plan uses
this query) prints how many encoder sessions are open on the GPU. Use it as the
indicator, with its caveat: it counts every process on the GPU, not ClippiBoy
alone, and it is a snapshot, so a short composite can open and close between two
queries. Note it before starting ClippiBoy, then at each step: buffer (1) + cam (1)
+ composite (1) + every other app that may encode on the GPU (OBS, NVIDIA App
Instant Replay, possibly a Discord screen share). If your driver answers `[N/A]`,
count by hand. To test the limit, add NVENC users until ClippiBoy's log reports a
refused session, then check that the fallback happened.

**Sync by clap.**
1. Wired microphone in the main mix or its own track (a Bluetooth headset can add
   its own delay — avoid it). Cam in view, bright light.
2. Save a clip with five sharp claps about 2 s apart, hands fully visible.
3. Open the composited clip in a video editor that shows the waveform and steps
   single frames (e.g. Shotcut, DaVinci Resolve).
4. Per clap: the time of the audio onset and the time of the first frame where the
   hands touch. Offset = picture − sound. Positive = the cam is late → the delay
   must go more negative.
5. Take the median of the five. One cam frame is 33 ms at 30 fps, so ±33 ms is the
   resolution of this test.
6. Repeat in dim light, at the end of a long clip, after a front trim, in a
   portrait export and after sleep.

**Save speed.** Film the keyboard and the screen with a phone at 60 fps and count
frames from key press to the banner, ten saves, median. Use log timestamps instead
if the save is logged.

**File layout.** `ffprobe -v error -show_format -show_streams -of json clip.mp4`
(ClippiBoy's own copy or any recent one) for the old and the new clip, then diff;
ignore duration, size, bit rate and dates.

**ffmpeg command line.** Process Monitor (Sysinternals), filter on process name
`ffmpeg.exe` and operation *Process Start*; the event's properties show the
command line.

**Memory.** Process Explorer's *Private Bytes* (or Task Manager's private working
set) for `ClippiBoy.exe` after 10 min with a full buffer; for leaks, note it every
15 min over an hour or more.
