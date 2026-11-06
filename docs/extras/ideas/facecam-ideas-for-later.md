# Facecam: ideas for later (extra)

You did not ask for this. It takes the spec's "Ideas for later" plus a few more ideas and gives each a rough design, so you can pick what is worth its own plan.
Each idea covers only what is left after the v1 design (`docs/superpowers/specs/2026-10-20-facecam-design.md`): what it is, how it would be built, what it costs while playing, the risks and the size.
Nothing was built or run. Line numbers are as of 401a320. Web facts carry a source (list at the end); numbers marked *estimate* are estimates.

| # | Idea | Value for players (1–3) | Effort | Depends on |
|---|---|---|---|---|
| 1 | "Cam off" hotkey, tray item and Stream Deck key | 2 | S | stage 2 |
| 2 | Per game: own position, hidden, or camera off | 2 | S | stage 2 (core), stage 3 (editor button) |
| 3 | Frame: accent border, shadow, crop and zoom | 2 | S | stage 2 (graph), stage 3 (editor) |
| 4 | Light fix: brightness, contrast, saturation | 2 | S | stage 2 (graph), stage 3 (editor) |
| 5 | Background: blur, removal, green screen | 3 | S (Windows hint, green screen) / M–L (own blur) | stage 2 (phase-2 job), stage 3 (preview) |
| 6 | Export the cam only, or game and cam as separate files | 2 | S | stage 3 |
| 7 | Layout presets, plus two more portrait layouts | 2 | S | stage 3 |
| 8 | Moves: the cam jumps, glides or hides for a moment | 2 | L | stage 3 |
| 9 | Swap: cam big, game small, for a moment | 1 | M | idea 8 |
| 10 | Cam state in the overlay (no permanent dot) | 1 | S | stage 2 |
| 11 | Sync test: measure the delay with a clap | 1 | M | stage 2 |
| 12 | Auto-hide when no face is seen | 1 | M | stage 2 (phase-2 job), stage 3 |
| 13 | Auto-frame: the crop follows the face | 1 | M | idea 12 |

The order is a suggestion: cheap and useful first.

**Already in the v1 design, not repeated here:** rectangle, rounded and circle shapes and mirror (3.1); the privacy rule (the camera runs only while something the user switched on needs it, 2.2); the master switch and the console's *Cam off* button (5.1, 5.3); "leave it out of recordings" as the *Facecam in recordings* switch (5.1, `in_recordings`); the stop behind the lock screen (2.2); the "TikTok" layout as the portrait default, cam on top at 35 % (3.1, 3.5); the per-clip delay slider (2.5, 3.3).

---

## 1. "Cam off" hotkey, tray item and Stream Deck key

**What.** One key turns the camera off or on without opening the console. The same switch sits in the tray menu and on a Stream Deck key. A short banner confirms it.

**How.**
- One core function, `set_facecam(app, Option<bool>)` (`None` = toggle), which the spec's `set_facecam_enabled` command (5.3) calls as well. It writes `facecam.enabled` through `replace_config` (`state.rs:419-457`) and opens or closes the camera on its own thread (5.1).
- Hotkey: `facecam_hotkey: String` in `AppConfig` next to the other keys (`model.rs:456-467`), `#[serde(default)]`, so it is empty and unbound by default (an empty key is skipped, `lib.rs:768-772`). It is registered in `register_hotkeys` (`lib.rs:773-864`) like the buffer toggle, with the handler on its own thread. `set_hotkeys` takes the keys one by one (`commands.rs:180-189`, called from `Hotkeys.tsx:165`): add a sixth argument, or better, pass a struct.
- Tray: a `CheckMenuItem` "Camera" between "Take screenshot" and "Start recording" (`tray.rs:55-87`). Its handle goes into `TrayHandles` (`tray.rs:17-28`), the click into the menu handler (`tray.rs:108-150`), and the check mark is set in `refresh` (`tray.rs:293-358`) from a new `EngineStatus.cam_on` (`model.rs:841`). The tooltip gets " · cam on".
- Stream Deck: `POST /v1/facecam` with `toggle | on | off`, next to `/v1/buffer` (`control.rs:356-367`). `/v1/status` gets `camOn`, so the key can show the state. A fifth action in `streamdeck/src/actions/`, copied from `toggle-buffer.ts`, plus its manifest entry.
- Banner: `overlay::show(app, BannerKind::Info, "Cam off", None)` (`overlay.rs:317`), behind the existing `on_buffer_toggle` switch (`model.rs:234`) or a new `on_cam_toggle`.
- "Off" means what the master switch already means: device closed, encoder and ring gone (2.2). The cam's last two minutes go with it, so a clip saved right after has no cam at all. That is the right answer to "something just happened on camera".

**Cost while playing.** None. Turning it on again costs the camera start (0.5–2 s, *estimate*), and the cam ring starts empty.

**Risks.**
- With the virtual camera on (stage 5), "off" also blanks Discord ("Camera is off in ClippiBoy", 4.2). That is probably right for a privacy key, but the settings should say so. If people want it, a second action "Cam out of clips" can stop only the cam track later.
- The write races the main window's config writes (review CFG-1). The revision fix from 5.1 has to land first.

**Effort.** S, 1–2 days.

---

## 2. Per game: own position, hidden, or camera off

**What.** Each game can have its own cam setting: its own position and size (the spec's example: bottom left in Valorant), *hidden* (the cam is stored but hidden in that game's clips), or *off* (the camera does not run while that game runs).

**How.**
- Data, in `FacecamConfig` (5.1):
  ```rust
  per_game: BTreeMap<String, GameCam>,   // key: game::normalize_name(name), lower case; at most 200 entries
  struct GameCam {
      mode: GameCamMode,                  // Show | Hide | Off (unknown values load as Show)
      layout: Option<CamLayout>,          // None = the default layout
      portrait: Option<PortraitLayout>,
  }
  ```
- The key is the name the clip gets: `game_for(window)` (`state.rs:642-647`, called in `save_clip` at `state.rs:904`; screenshots take `current_game`, `state.rs:966`), normalised with `game::normalize_name` (`game.rs:293`), the rule that detection and filing already share.
- Position and *hidden*: the layout is read at save anyway (5.1). A pure `layout_for(game)` returns the game's layout or the default. *Hide* sets `visible = false`, so phase 2 is skipped (2.6, step 6) and the cam can still be shown later in the editor.
- *Off*: the cam track's "wanted" rule (2.2) gets one more condition: the held game is not `Off`. `track_game` (`state.rs:657-757`) already keeps a sticky name that follows the game process, not the foreground window (`state.rs:720-731`), so alt-tab and the console do not flip the camera. The cam stops at most one 2-second tick after detection and comes back when the process ends. *Off* stops the cam track and clears its ring. The device itself closes only if nothing else needs it (settings preview, virtual camera), as the lifecycle in 2.2 already says.
- UI: the fast path is a button in the editor's Facecam section (3.3): "Use for all <game> clips". It copies the clip's layout into `per_game`. Arrange the cam once on a real clip, then click. The Facecam tab gets a short list of games with mode and reset. Names come from the clip DB (a `SELECT DISTINCT game` next to the list query at `clips.rs:135`) plus the current game.

**Cost while playing.** A map lookup on the 2-second tick. *Off* saves the cam's whole cost for that game.

**Risks.**
- Names read from window titles can change (`game.rs:1-9`, the log line at `state.rs:668-676`), and the entry then misses. Same weakness as the game folders; `games.json` fixes it.
- *Off* at game start: up to 2 s of the game are captured before the ring is cleared, and the cam from before the game (lobby) is lost with it.
- Each close and reopen at game start and end gives Discord a moment to grab the camera (fact 5). Rare, acceptable.

**Effort.** S for core and editor button; M with a polished games list.

---

## 3. Frame: accent border, shadow, crop and zoom

**What.** v1 has rectangle, rounded, circle and mirror. What is left: a border in the accent colour (or white), a soft shadow, and choosing which part of the camera picture shows (crop and zoom).

**How.**
- Layout additions (3.1). The defaults change nothing:
  ```rust
  border: Option<CamBorder>,
  struct CamBorder { width: f32 /* share of the cam height, 0.005..0.03 */, color: BorderColor }
  enum BorderColor { Accent { first: String, second: Option<String> }, Custom(String), White }
  shadow: bool,
  crop: CamCrop { cx: f32, cy: f32, zoom: f32 },  // centre 0..1 in source coordinates, zoom 1.0..3.0
  ```
- The accent is copied into the clip's layout as hex at save, from `AppConfig::accent()` (`model.rs:634-641`) and checked with `clean_accent` (`model.rs:648`). A later accent change then never repaints old clips at their next re-composite *(design call)*. Gradient accent: the ring PNG gets the gradient laid across it, the way `tint_rgba` lays one across the tray icon (`tint.rs:130`). RGB accent: the colour of that moment, frozen, as the banner and console do (`src/lib/accent.ts:118-127`).
- Graph: exactly research `ffmpeg-compositing.md` 2.b. Mask and ring PNGs are rendered in Rust, then `alphamerge`, `pad` and an inner `overlay`, so the border lives on the cam's timeline and disappears with the cam. Shadow: one more RGBA PNG (the shape, blurred once in Rust, about 40 % black), overlaid under the badge by the same rule.
- Crop and zoom: `crop=w:h:x:y` in source coordinates **before** `hflip` (research 2.d: `x_src = iw - x_ui - crop_w`), then the existing `scale`. The circle's square crop (research 2.b) becomes the `zoom = 1` case of the same rule.
- Editor: border and shadow as CSS over the clipped `<video>`, rounded to the same even pixels as the PNGs; zoom as a transform of the inner `<video>`. A "Frame" row in the Facecam section (3.3): border none / accent / white, width, zoom, and dragging inside the cam box with a modifier key pans the crop.
- Under the zoom slider, a warning when the box shows more pixels than the source has (`cam_h / zoom < box_h`): "The cam gets soft. Set the camera to 1080p" (`resolution`, 5.1).

**Cost while playing.** None. In the composite, two small overlays and a crop: microseconds per cam frame (research 2.b, table).

**Risks.** Zoom on a 720p cam track gets soft fast. The CSS preview and the PNG ring must agree to the pixel (one table of test vectors for TS and Rust).

**Effort.** S, 2–4 days, most of it the editor controls.

---

## 4. Light fix: brightness, contrast, saturation

**What.** Webcams in dark gaming rooms look grey and flat. In sharing mode ClippiBoy cannot change the camera's exposure (2.3), so the fix has to go into the picture: three sliders, applied when compositing.

**How.**
- `look: CamLook { brightness: f32 /* 0.7..1.4 */, contrast: f32 /* 0.7..1.5 */, saturation: f32 /* 0..2 */ }`. A default per camera in `FacecamConfig` (keyed by `device_id`, then the name), copied into the clip's layout at save, adjustable per clip.
- Use the CSS maths on both sides, so the editor preview (`filter: brightness(b) contrast(c) saturate(s)`) is what the composite draws:
  - brightness and contrast as one `lutrgb` with `'clip(((val/255*b)-0.5)*c+0.5,0,1)*255'` per channel (a table built once, then one lookup per pixel);
  - saturation as `colorchannelmixer` with the CSS `saturate()` matrix (rr = 0.213+0.787s, rg = 0.715−0.715s, and so on).
  - `eq` would be shorter, but its brightness adds instead of multiplying and would not match the preview.
- On the cam chain after `scale`, so it runs on the small frame: `format=gbrp,lutrgb=…,colorchannelmixer=…,format=yuva420p`. After the BT.601→BT.709 conversion (research 2.e).
- The settings preview tile (5.2) uses the same CSS.

**Cost while playing.** None. In the composite, two passes and two format conversions on a 480×270 frame at 30 fps: negligible (*estimate*).

**Risks.** Chromium may apply CSS filters in a different colour space than the LUT; check once against a test frame. People will expect "auto"; leave that out.

**Effort.** S, 2 days.

---

## 5. Background: blur, removal, green screen

**What.** Blur the room behind the player, or remove it so only the player sits over the game. There are three routes, from cheap to expensive. None of them adds work while playing.

**How.**

*What is realistically available on Windows (research):*
- **Windows Studio Effects (WSE).** It needs a supported NPU and an OEM that ships the WSE driver. The full set needs a Copilot+ PC with an NPU above 40 TOPS; weaker NPUs get only some effects [MS Support]. The effects run in the camera pipeline: "once an effect is turned on … it is on by default for any app using the camera" [MS Learn, Studio Effects]. USB cameras: only on Copilot+ PCs, first in Insider builds 26220.5790 / 26120.5790 (September 2025), Intel first, AMD and Snapdragon later, switched on per camera with "Use Windows Studio Effects" [Thurrott; Neowin; Windows Report]. I found no source saying it reached normal releases yet. Apps control it with `KSPROPERTY_CAMERACONTROL_EXTENDED_BACKGROUNDSEGMENTATION` (OFF, BLUR, MASK, SHALLOWFOCUS). MASK delivers a per-pixel mask as frame metadata, 0 = background to 255 = foreground [MS Learn, KS property; NtDoc]. For ClippiBoy's players, desktop PCs with USB webcams, this reaches few people. And in sharing mode ClippiBoy can read camera controls but not set them; the controlling app does.
- **An own segmentation model.** MediaPipe Selfie Segmentation: two models of 244–249 KB, input 144×256 (landscape) or 256×256, a person mask out, "Run in real-time via XNNPack TFLite backend on a laptop CPU", licensed under Apache 2.0 [model card]. It is TFLite; a one-time conversion to ONNX is fine under Apache 2.0 with the NOTICE kept. Runtime: ONNX Runtime (MIT) through the `ort` crate (MIT / Apache-2.0) [ort]. For the GPU, DirectML "is in sustained engineering" and new work has moved to Windows ML [ONNX Runtime, DirectML EP; MS Learn, DirectML]. Windows ML is generally available since September 2025 (Windows App SDK 1.8.1, Windows 11 24H2+; 1.8.3 added Windows 10 1809+ for CPU and GPU) and downloads the hardware execution providers itself [Windows Developer Blog; Thurrott; PureAI]. RobustVideoMatting looks better but is GPL-3.0 [RVM]; ClippiBoy is "all rights reserved" (`LICENSE`), so it is out.
- **Green screen.** ffmpeg's `chromakey` (options `color`, `similarity`, `blend`) is a built-in filter with no external library [ffmpeg filters], so the essentials build has it (research 1.3).
- **Other apps' cameras.** NVIDIA Broadcast (RTX only) creates a virtual camera with effects [NVIDIA FAQ]; vendor tools do similar things. If Media Foundation sees such a camera, ClippiBoy can simply use it as its device. Untested: DirectShow-only virtual cameras are invisible to the MF camera source (spec, Open item 13).

*Recommended order:*
1. **WSE hint (S).** If the device has the segmentation control, the Facecam tab reads it (GET only) and says: "Background blur is on in Windows. It is in your clips and in Discord." A button opens `ms-settings:camera?cameraId=<symbolic link>` [MS Learn, Studio Effects]. When ClippiBoy is the controlling client (virtual camera on, stage 5), it may set BLUR itself. Never blur twice: if the WSE blur is on, ClippiBoy's own blur stays off. Microsoft's page warns about exactly this overlap.
2. **Green screen (S).** `key: Option<ChromaKey { color, similarity, blend }>` in the layout; the colour is picked by clicking into the preview. Graph: `…scale=480:270,chromakey=0x00d000:0.12:0.06[ck]`, then multiply the key alpha with the shape mask: `[ck]alphaextract[ka];[ka][shape]blend=all_mode=multiply[a];[ck][a]alphamerge`. 
3. **Own blur and removal, at save time (M–L).** This fits approach A: nothing while playing, a few seconds more in phase 2 (2.6).
   - Phase 2, before the composite: `ffmpeg -i cam.mp4 -vf scale=256:144,format=rgb24 -f rawvideo pipe:1` → Rust runs the landscape model on each frame → a 256×144 mask, smoothed over 2–3 frames against flicker → `originals/<id>/mask.mkv` (FFV1, gray8, lossless and small, `-framerate 30`; valid because the cam track is exact CFR, 2.3). Kept, so the editor can switch between off, blur and remove without running the model again. Listed under *Free up space* with the clean picture (2.7).
   - Blur: `[cam]split[c1][c2];[c2]scale=120:68,boxblur=lr=4:lp=2,scale=480:270[bg];[m]scale=480:270,format=gray[pm];[c1][pm]alphamerge[fg];[bg][fg]overlay[cb]`, then the shape mask as in v1.
   - Remove: person mask × shape mask (`blend=all_mode=multiply`) into `alphamerge`. Only the player is drawn, no box; the border (idea 3) is off in this mode.
   - Editor preview: blur from a baked `cam_fx.mp4`; removal through a small WebGL canvas that uses the mask video's luma as alpha (the main window has the GPU).
   - Distribution: ONNX Runtime with the CPU provider first, downloaded on first use like ffmpeg (`tools.rs:29`, `118`, `208`), with a pinned hash; the 244 KB model ships in the app. DirectML or Windows ML only if the CPU numbers are too slow.
   - Live blur for Discord through the virtual camera is not part of this. It would run on every frame while playing; WSE and NVIDIA Broadcast already do it.

**Cost while playing.** None for routes 2 and 3; route 1 runs on the NPU in Windows' own pipeline. Phase 2 grows by one model run per cam frame: 900 for a 30 s clip. At 2–5 ms each on one desktop core that is 2–5 s at below-normal priority (*estimate, measure first*).

**Risks.**
- Mask quality. The model card names thin features such as fingers, several people, low light, noise, fast motion and large occluders as weak spots. Dark rooms lit by RGB strips are the normal case for gamers. A high-backed gaming chair is likely to count as "person" (*assumption*).
- A 2-minute clip has 3600 cam frames: up to 18 s more in phase 2 at the high end.
- Download size (onnxruntime.dll is roughly 10–20 MB, *estimate*) and one more native library to keep current.
- Licences: keep the model's Apache NOTICE; check the redistribution terms of whichever DirectML or Windows ML package is used.

**Effort.** Route 1: S (2 days). Route 2: S (2–3 days). Route 3: M–L (2–3 weeks, half of it measuring quality and speed in real rooms).

---

## 6. Export the cam only, or game and cam as separate files

**What.** Two more choices in the export dialog, next to landscape and portrait (3.5): *Cam only* (the reaction, with the clip's sound) and *For editing* (the clean game and the cam as two files of equal length that line up at 0 in CapCut, Premiere or Resolve). The clean picture and `cam.mp4` already exist for every facecam clip (2.7), so this is mostly argument building.

**How.**
- `ExportKind { Clip, CamOnly, Separate }` in the export request. `export::arguments` (`export.rs:116-153`) gets two siblings, pure and tested like it.
- *Cam only:* a `color=c=black:s=1280x720:r=30:d=<L>` base inside the graph, the cam overlaid with the offset rule of research 2.e (`-ss e`, or `setpts=PTS-STARTPTS+X/TB`), `eof_action=pass` and the gap `enable`, so the file has the clip's exact length even when the cam started late. Mirror and light fix (idea 4) apply; shape, border and position do not. Audio `-map <n>:a:0?` from the current clip file, as the export does today. Optional 9:16 cover crop for a reaction short.
- *Separate:* `<name>_game.mp4` is the clean original cut to the trim (stream copy without a front cut, else a re-encode, the rule in 3.4) with the mix; `<name>_cam.mp4` as above, same start, same length. Optionally the stems as `.m4a` from `tracks/<id>/` (they already exist, `stems.rs`). Everything goes into one folder named after the clip.
- The size target of the export dialog applies to the game file only.

**Cost while playing.** None.

**Risks.** People may expect the cam file to keep shape and position; call it "raw cam" in the dialog. A front trim of the clean original needs a re-encode to be frame-exact.

**Effort.** S, 2–3 days.

---

## 7. Layout presets, plus two more portrait layouts

**What.** A few named layouts to apply in one click in the editor and the export dialog, and "Save as preset". "TikTok: cam on top at 35 %" already is the v1 portrait default (3.1, 3.5), so presets only earn their keep with layouts v1 does not have: *round cam over a full-height game* and *game on top, cam below*.

**How.**
- `struct Preset { id: String, name: String, landscape: Option<CamLayout>, portrait: Option<PortraitLayout> }`. Built-in presets live in the code (never stored, so they can be improved); the user's own go into `FacecamConfig.presets` (at most 20).
- `PortraitLayout` gets `mode: Split | Bubble | SplitBelow` (unknown values load as `Split`):
  - *Bubble:* the game as a 9:16 crop (`crop=608:1080:<x>:0,scale=1080:1920,setsar=1`, research 2.c) and the cam as a badge on it: the landscape graph of research 2.a and 2.b with W×H = 1080×1920.
  - *SplitBelow:* research 2.c with the bands swapped (game at y = 0, the cam band below it).
- Built-ins: Corner small (size 0.2), Corner large (0.33), Portrait split 35 %, Portrait split 50 %, Portrait bubble.
- Per-game entries (idea 2) copy a preset's values instead of pointing at it, so deleting a preset breaks nothing.

**Cost while playing.** None.

**Risks.** A list nobody uses: keep the built-ins at five. In *Bubble* the cam covers part of a game picture that is already cropped hard, so it must stay movable there.

**Effort.** S, about 3 days (two graphs, their editor previews, one dropdown).

---

## 8. Moves: the cam jumps, glides or hides for a moment

**What.** Today one layout holds for the whole clip (decision: "No keyframes for now"). A *move* changes it from a point in the clip on: another corner when the kill feed comes up, hidden for a few seconds when someone walks in, back again. The change is a cut, a glide (position only) or a fade.

**How.**
- Data, in the clip's `facecam` column (2.7), next to the layout:
  ```rust
  moves: Vec<CamMove>,               // sorted by time, at most 16
  struct CamMove {
      at_ms: u32,                    // original coordinates, like the gaps, so a front trim does not shift it
      to: CamLayout,                 // visible = false: hidden from here on
      transition: Transition,        // Cut | Glide { ms } | Fade { ms }, ms 150..1000
  }
  ```
- Rust cuts the clip into stretches of constant layout (a pure function next to the layout builder of 3.1) and builds the graph from them:
  - Position changes at the same size and shape: one badge chain. The overlay's `x` and `y` become piecewise expressions, evaluated per frame (`eval=frame`, the default), e.g. `x='if(lt(t,10.0),1412,if(lt(t,10.4),1412-1384*(t-10.0)/0.4,28))'`. An ease (smoothstep) fits in the same expression with `st()`/`ld()`.
  - Size or shape changes: one badge chain (scale, mask, ring) per distinct size, each overlay switched with `enable='between(t,a,b)'`. A fade keeps both on for its length, with `fade=t=out:st=…:d=…:alpha=1` and `fade=t=in:…:alpha=1` on the badge streams.
  - Hidden: `enable` off for the range, the way gaps are hidden (research 2.e).
  - **Rule:** every branch of a shared `split` must be consumed frame by frame. So `enable`, never `trim` or `select`, on those branches. `overlay` waits for a trimmed branch's first frame, and every frame of the other branch queues up until then. For a stretch that starts at 60 s in a 1080p clip that is gigabytes (*reasoned from framesync, research 2.e; verify with a test*).
  - At most four distinct sizes. Each chain scales every cam frame (about 1 ms at 480×270, *estimate*), which starts to show in phase 2 beyond that.
  - No size glide: `alphamerge` fails as soon as the frame size changes ("Input frame sizes do not match", research 2.b). A size change is a cut or a fade.
- Editor: a thin "cam" lane under the scrubber (`Scrubber`, `src/components/ui/PlayerControls.tsx:102`). "Add move here" at the playhead puts a marker there; the cam box (`RectLayer`, 3.3) then edits the layout from that marker on; clicking a marker chooses the transition or deletes it. The preview computes the same piecewise function in TS on each cam frame (`requestVideoFrameCallback`). One table of test vectors for TS and Rust keeps the two equal.
- Undo trim, re-composite and *Free up space* need nothing new: moves are part of the layout.

**Cost while playing.** None.

**Risks.** The editor lane is most of the work and easily gets fiddly. Long recordings (stage 4) with many moves make long graphs, hence the cap. The TS and Rust curves must match to the pixel.

**Effort.** L, 2–3 weeks.

---

## 9. Swap: cam big, game small, for a moment

**What.** For a reaction: the cam fills the frame and the game shrinks into a corner for a few seconds. Built as one more kind of move (idea 8).

**How.**
- `CamLayout.mode: Inset | Swap` (unknown values load as `Inset`). In `Swap` the layout's box says where the **game** sits; the cam covers the frame.
- Graph, for a clip with swap stretches:
  - `[cam]…crop=<cover>,scale=W:H[cfull]`, overlaid on the game at 0,0 with `enable` for the swap windows;
  - `[0:v]split[g][gs];[gs]scale=<w>:<h>[gsmall]`, overlaid on top with the same `enable`, with the shape mask if wanted.
  - The same rule as in idea 8: `enable`, not `trim`. So the small game copy is scaled on every game frame of the clip, not only during the swap (`scale` has no timeline support): about 1–2 ms per 1080p frame (*estimate*).
- Fades in and out as in idea 8, with `fade` and `alpha=1` on the full-frame cam.
- A ready entry in the "Add move" menu: "Swap for 3 s".

**Cost while playing.** None. In the composite: one full-frame scale per cam frame and one game downscale per game frame.

**Risks.** A 720p cam blown up to 1440p looks soft; suggest 1080p for the cam. In portrait a swap would change the band sizes; leave portrait out at first.

**Effort.** M, about a week on top of idea 8.

---

## 10. Cam state in the overlay (no permanent dot)

**What.** See whether the cam is on without opening the console. A dot that stands over the game whenever the cam runs would stand almost all the time (the cam runs with the buffer), so that is the wrong shape. Put the state where ClippiBoy already speaks instead.

**How.**
- The banners that appear anyway get a small cam glyph: "Buffer on" while the cam runs, "Clip saved" when the clip got a cam. `Banner` gets `cam: bool` (`overlay.rs:112-123`); the page draws the glyph (`src/overlay/Overlay.tsx`).
- The REC badge (`Overlay.tsx:122-136`) gets the glyph while the running recording includes the cam (stage 4).
- An opt-in "Cam dot while the camera runs", off by default. It keeps the window up the way `BADGE` does for the REC badge (`overlay.rs:210-237`). Static, with no endless CSS animation: the REC dot's blink keeps Chromium drawing (`overlay.css:320-321`, review UI-7 in `docs/extras/review-beyond-facecam.md`).
- The window is `content_protected` (`overlay.rs:181`), so none of this lands in a clip or a screen share.
- Already there: the camera's own LED, the console tile (5.3), the tray tooltip (idea 1), and Windows' own "Camera on/off" notice, which is off by default [Pureinfotech].

**Cost while playing.** None for the glyphs. The opt-in dot: one shown, static, CPU-drawn window (measure together with UI-7).

**Risks.** Clutter over the game, hence off by default. Games in exclusive fullscreen show no overlay at all (`overlay.rs:7-10`).

**Effort.** S, 1–2 days.

---

## 11. Sync test: measure the delay with a clap

**What.** The default delay of −40 ms is an estimate (2.5). A button on the Facecam tab measures it: clap three times, ClippiBoy finds each clap in the mic and in the picture and suggests the delay. Is it worth it? Sound ahead of the picture is noticed from about 45 ms and is acceptable up to about 90 ms [ITU-R BT.1359]. The camera's estimated 40–90 ms lag minus the default leaves 0–50 ms, mostly fine already. It matters with a Bluetooth headset mic, whose own lag can be larger than the camera's (*estimate*).

**How.**
- A 6-second window. Mic samples from the mic source's `SampleRing::read_window` (`audio/ring.rs:183`), already on the QPC timeline, so no second capture is needed (the source has to be running, as it is with the buffer on). Cam frames from the raw-frame tap with their QPC (2.10), shrunk to 160×90 gray.
- Sound: a 1 ms RMS envelope. A clap is the envelope rising 20 dB above the median floor within 5 ms. Take the three strongest, at least 400 ms apart.
- Picture: motion energy per frame (mean absolute luma difference to the previous frame). Hands speed up, then stop dead when they meet. Contact = the first frame after the motion peak whose energy falls below half the peak, minus half a frame.
- Delay = mean of (sound onset − contact), rounded to 10 ms and clamped to −300…300. Accepted only if the three agree within 40 ms and the camera ran at 25 fps or more (low light halves the rate, 2.3, and doubles the error). Otherwise: "Couldn't measure. More light, closer to the camera."
- A filmstrip of the frames around each clap, with the chosen frame marked, lets the player correct it with one click. That is more robust than trusting the motion curve alone.
- Stored per camera: `delay_by_device: BTreeMap<String, i32>`, with `delay_ms` (5.1) as the fallback. The C920 and the eMeet will differ (research `ffmpeg-compositing.md` 5.6).

**Cost while playing.** None; it is a settings action.

**Risks.** At 30 fps one frame is 33 ms, about the size of the correction: average over the claps, or use the filmstrip. Game sound in the mic spoils the onsets.

**Effort.** M, about a week (signal code, tests on recorded samples, the filmstrip).

---

## 12. Auto-hide when no face is seen

**What.** When the player leaves the desk, the clip shows an empty chair. Auto-hide hides the cam for stretches without a face and shows it again when the face is back. Off by default.

**How.**
- Detector: `Windows.Media.FaceAnalysis.FaceTracker`, built into Windows 10 and 11 (from 10.0.10240), fed `VideoFrame`s, with `IsSupported` and a query for its pixel formats [MS Learn, FaceTracker]. No model to ship. windows-rs features `Media_FaceAnalysis` and `Graphics_Imaging`.
- When: in phase 2 (2.6), not live. `ffmpeg -i cam.mp4 -vf fps=5,scale=320:-2,format=gray -f rawvideo pipe:1` → Gray8 `SoftwareBitmap` (if the tracker lists Gray8; otherwise NV12) → `ProcessNextFrameAsync`. The samples go into `facecam.json` (2.7): `faces: [[t_ms, x, y, w, h]]`, normalised, empty where none was found.
- Hidden ranges = runs without a face of 3 s or more, shrunk by 0.5 s at both ends. A pure function, recomputed at every composite, so switching it in the editor needs no new detection run.
- Graph: the same `enable='not(between(t,…)+…)'` as hidden gaps (research 2.e), optionally a 0.3 s alpha fade. In portrait the band shows the blurred fill (3.5).
- Editor: the hidden ranges as shaded stretches on the cam lane (idea 8), and a per-clip "Always show the cam".

**Cost while playing.** None. In phase 2, about 150 detections per 30 s clip on 320×180 gray; at tens of milliseconds each, 1–3 s on a background thread (*estimate*).

**Risks.** A missed face hides the cam while the player is there, which is worse than an empty chair: a turned head, a headset mic across the mouth, low light, a face far from the camera. Hence the 3 s, the default off, and the visible ranges in the editor. `FaceTracker` is an old API and likely tuned for frontal faces (*assumption, to test with both cameras*).

**Effort.** M, about a week.

---

## 13. Auto-frame: the crop follows the face

**What.** The crop of idea 3 follows the face, so a player who sits off-centre or leans stays in frame. It reuses the face samples of idea 12. Copilot+ PCs do this live with WSE "Automatic framing" [MS Learn, Studio Effects]; this works for every camera, at save time.

**How.**
- From the face samples, a centre path smoothed in Rust: a dead zone of 5 % of the frame, then a 1-second ease to the new centre, never faster than 20 % of the frame per second. The zoom is fixed per clip (about 1.4 × the face height, clamped to 1.0–2.5), so the crop size never changes.
- Graph: `crop` evaluates `x` and `y` per frame and takes `x` and `y` as commands [ffmpeg filters]. Rust writes a `sendcmd` file with one line per cam frame (`<t> crop@cam x <px>, crop@cam y <px>;`) into the temp folder, placed in front of a named `crop@cam`. For short clips a piecewise expression over 2 Hz points works too.
- Editor: the same path moves the cam `<video>`'s CSS transform on each frame (`requestVideoFrameCallback`).
- On or off per clip; off by default.

**Cost while playing.** None.

**Risks.** Wobble makes people seasick, hence the strong smoothing and the dead zone. Zooming a 720p track gets soft (the warning from idea 3). A false detection pulls the frame away: drop samples whose face jumps more than 30 % between two samples.

**Effort.** M, about a week after idea 12.

---

## Left out, and why

- **A permanent "cam on" dot over the game as the default:** it would stand whenever the buffer runs (idea 10 has the opt-in).
- **Live blur in the cam worker or the virtual camera:** GPU work on every frame while playing, against approach A. WSE and NVIDIA Broadcast already do it live.
- **RobustVideoMatting and other GPL models:** the licence does not fit a closed app.
- **Size glides with a per-frame `scale`:** a changing frame size breaks `alphamerge` (idea 8).
- **An animated RGB border:** possible with `hue=h=…*t` on a looped ring [ffmpeg filters], but a per-frame cost for a detail nobody will miss.
- **An automatic swap "at the highlight":** the clip ends at the key press, so there is nothing left to react to.

## Sources

- Microsoft Learn, Windows Studio Effects overview (requirements, "on by default for any app", KS controls, `ms-settings:camera?cameraId=`, overlapping effects, automatic framing): https://learn.microsoft.com/en-us/windows/apps/develop/windows-integration/studio-effects
- Microsoft Support, Windows Studio Effects (NPU above 40 TOPS, effects list, third-party cameras): https://support.microsoft.com/en-us/windows/ai/ai-features/windows-studio-effects
- Microsoft Learn, `KSPROPERTY_CAMERACONTROL_EXTENDED_BACKGROUNDSEGMENTATION` (OFF, BLUR, MASK, SHALLOWFOCUS): https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/ksproperty-cameracontrol-extended-backgroundsegmentation
- NtDoc, `KSCAMERA_METADATA_BACKGROUNDSEGMENTATIONMASK` (0 background … 255 foreground): https://ntdoc.m417z.com/kscamera_metadata_backgroundsegmentationmask
- Thurrott, Studio Effects on additional cameras: https://www.thurrott.com/?p=325961
- Neowin, Studio Effects for USB webcams in Dev/Beta builds: https://www.neowin.net/news/windows-11-is-getting-new-ai-features-for-your-webcams/
- Windows Report, build 26220.5790 / 26120.5790: https://windowsreport.com/?p=1469490
- Framework community (announcement dated September 2025, no AMD timeline): https://community.frame.work/t/missing-windows-studio-effects-on-windows-11/78878/10
- Google, Model Card MediaPipe Selfie Segmentation (sizes, inputs, Apache 2.0, limitations): https://storage.googleapis.com/mediapipe-assets/Model%20Card%20MediaPipe%20Selfie%20Segmentation.pdf
- ONNX Runtime, DirectML execution provider ("in sustained engineering", DirectX 12 GPUs): https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html
- Microsoft Learn, DirectML: https://learn.microsoft.com/windows/ai/directml
- Windows Developer Blog, Windows ML generally available (September 2025): https://blogs.windows.com/windowsdeveloper/2025/09/23/windows-ml-is-generally-available-empowering-developers-to-scale-local-ai-across-windows-devices
- Thurrott, Windows ML GA with Windows App SDK 1.8.1: https://www.thurrott.com/?p=327165
- PureAI, Windows ML runtime (Windows 11 24H2+): https://pureai.com/Articles/2025/09/26/Microsoft-Launches-Windows-ML-Runtime-for-OnDevice-AI-Applications.aspx
- `ort` crate (MIT / Apache-2.0): https://github.com/pykeio/ort
- ONNX Runtime (MIT): https://github.com/microsoft/onnxruntime
- RobustVideoMatting (GPL-3.0): https://github.com/PeterL1n/RobustVideoMatting
- NVIDIA Broadcast FAQ (virtual camera): https://www.nvidia.com/broadcast-app-faq
- Microsoft Learn, `FaceTracker`: https://learn.microsoft.com/en-us/uwp/api/windows.media.faceanalysis.facetracker
- ITU-R BT.1359 (detectability about +45 / −125 ms, acceptability about +90 / −185 ms): https://itu.int/dms_pubrec/itu-r/rec/bt/R-REC-BT.1359-0-199802-S!!PDF-E.pdf
- ffmpeg filters (crop x/y per frame and as commands, chromakey, eq, fade `alpha`, hue expressions with `t`): https://ffmpeg.org/ffmpeg-filters.html
- Pureinfotech, Windows 11 camera privacy indicator (off by default): https://pureinfotech.com/enable-camera-privacy-indicator-windows-11/
