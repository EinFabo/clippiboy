# Facecam Stage 3, expanded: the editor

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> Expands `2026-10-22-facecam-stage-3-editor.md` against branch `facecam` after
> stages 2 and 5 (HEAD 601eb6e). The rough plan's goals, copy decisions and risks
> stand unless a line here says otherwise; its line numbers do not — anchors here
> are function names.

**Goal:** In the player, a clip's camera can be moved, resized, reshaped, mirrored,
hidden and re-synced; Save re-composites from the clean sources; the export dialog
writes a separate 9:16 file, which the editor can preview.

**Architecture:** The clip's `<video>` stays the clock and the sound; its picture is
hidden while the clip has a `facecam_view`. Two muted elements, slaved to it, show
the clean picture and `cam.mp4`. All timing and pixel numbers come from Rust
(`facecam_view`, `composite::cam_rect`, `portrait_geometry`); the UI only adds them.
Save hands the wanted `FacecamEdit` to `edit::apply`, which plans on a copy of the
row carrying it, so stage 2's single `cam_part` rule decides; the column is written
only after the file is.

**Tech Stack:** Rust (Tauri 2), React + TypeScript, ffmpeg 9.0.1. Rust tests:
`.superpowers/sdd/2026-10-22-facecam-stage-2-foundation-and-clips/wcargo test --lib`.
TS: `npx tsc --noEmit -p .` and `npm test` (node's runner over `src/**/*.test.ts` —
pure TS functions get real unit tests; the rough plan predates them).

**Spec:** `docs/superpowers/specs/2026-10-20-facecam-design.md` §3.1–3.6, 2.6 (3, 5),
2.7; Open for Fabi 2, 6, 9, 10 (recommendations stand: originals store, own
`facecam` column, rounded + not mirrored, watermark (a)).

## Global Constraints

- UI copy in English, labels without explanation lines (Fabi 2026-10-29); the
  feature is "Camera" in the UI. This overrides the rough plan's hint lines.
- Commits in English, no `Co-Authored-By`.
- Every ffmpeg number is computed in Rust; graphs hold only literals; builders are
  pure with sentence-named tests.
- **Always from the clean sources:** no composite reads a file whose facts'
  `applied` shows a visible cam, and none is filed away as the original.
- No `vstack`, no `shortest=1`; `trim=end=…` in the cam chain; `-t L` last.
- Clips without facecam data, received clips and baked clips keep today's paths
  (`CamPart::None`); every new branch has a test for the absent case.
- Every rewrite runs under `edit::clip_lock(id)` → `WORKING` → library, planning
  on the row read inside the lock.
- A portrait export never rewrites the clip.
- One layout tolerance everywhere: 0.002 of the frame for `x`, `y`, `size`;
  `visible`, `shape`, `mirror` and the delay compare exactly.

## Review Focus

1. A save that hides the cam on a clip whose file shows it reads the clip file (cam
   burned in twice or kept) → Task 4 `a_hidden_cam_reads_the_clean_picture`.
2. The DB layout (now the *wanted* one) deciding whether the file is composited,
   so a composite gets filed away as the original → Task 4
   `the_file_not_the_row_says_whether_the_cam_is_in_it`.
3. Preview and file disagree on where the cam starts → Task 1
   `the_view_takes_its_leads_from_cam_timing`, Task 2 `slave_time_adds_the_lead`.
4. A portrait export squashed, or the TS preview disagreeing with the filter →
   Task 6 `a_portrait_export_is_never_squashed`, Task 7
   `portrait_geometry_matches_the_filter`.
5. A cam shown again after a save that hid it, with no clean copy filed away yet
   → Task 4 `a_cam_shown_after_a_hidden_save_composites_from_the_clip_file`.

---

### Task 1: What the player needs per clip — `FacecamView`

**Files:** Modify `src-tauri/src/facecam.rs`, `src-tauri/src/model.rs` (`Clip`),
`src-tauri/src/clips.rs` (where `original_available` is derived), `src/lib/types.ts`,
`src/lib/mock.ts`.

**Interfaces — Produces:**
- `pub struct FacecamView { clean_path: String, cam_path: String, cam_width: u32,
  cam_height: u32, clean_lead_ms: i64, cam_lead_ms: i64, gaps_ms: Vec<(u64, u64)>,
  shows_cam: bool }` (serde camelCase), on `Clip` as `#[serde(default)]
  facecam_view: Option<FacecamView>`, derived on read like `original_available`,
  never stored.
- `pub fn view_of(clip: &Clip) -> Option<FacecamView>` in `facecam.rs`: `None` without
  `clip.facecam`, without a store (`has_store`), or when the state is `Baked`.
  `clean_path` = `edit::video_path(id)` when that file exists, else the clip file
  while the file shows no cam (`shows_cam == false`), else `None` for the view.
  `shows_cam` = facts' `applied` is `Some` with `layout.visible`.
- Leads, element time = clip time + lead: `clean_lead_ms` = `original.start_ms`
  when the clean source is the stored original, else 0;
  `cam_lead_ms` = `(e − video_start_us) / 1000` with
  `e = start_ms·1000 − (cam_offset_us + delay_ms·1000)` — the inverse of
  `composite::cam_timing` (cam frame `c` shows at clip time `c + shift − seek`).
- `pub fn file_shows_cam(clip_id: &str) -> bool` (facts' `applied` visible) —
  Task 4 uses it.
- TS: `FacecamView` type; `Clip.facecamView?: FacecamView | null`.

- [ ] **Step 1: Failing tests** in `facecam.rs`:
  `the_view_takes_its_leads_from_cam_timing` (offset 120 000 µs, delay −40,
  video_start 21 333 µs, start 5 000 ms: for clip times 0 and 3.0 s,
  `t + cam_lead/1000` equals the `c` with `c − seek + shift == t` from
  `cam_timing(…)`, within 1 ms), `a_clip_without_facecam_reads_as_none`,
  `the_view_is_none_once_the_store_is_gone`, `a_baked_clip_has_no_view`,
  `a_clip_saved_with_the_cam_hidden_plays_its_own_file_as_the_clean_picture`.
- [ ] **Step 2:** Run `wcargo test --lib facecam` → FAIL (no `view_of`).
- [ ] **Step 3:** Implement `view_of`, `file_shows_cam`, the field and its
  derivation; the TS type; `mock.ts` fixtures: ready, pending, failed, hidden at
  save, baked, none (clean and cam URLs pointing at the existing mock video).
- [ ] **Step 4:** `wcargo test --lib` → all pass; `npx tsc --noEmit -p .` clean.
- [ ] **Step 5:** Verify the asset scope in `tauri.conf.json` covers
  `originals/<id>/` (the stems' previews already play from the data dir); if not,
  add it. Commit.

### Task 2: The preview stage — slaved clean picture and cam

**Files:** Create `src/lib/slaved.ts` (+ `slaved.test.ts`),
`src/lib/useSlavedMedia.ts`, `src/components/CamStage.tsx`. Modify
`src/lib/useClipMix.ts` (its stems use the hook, behaviour unchanged),
`src/components/ClipPlayer.tsx`, `src/lib/dom.ts` (`freezeFrame`).

**Interfaces:**
- Consumes `Clip.facecamView` (Task 1), stage 2's `camBox(layout, w, h)`,
  `usePictureBox`.
- Produces `slaveTime(masterSec: number, leadMs: number): number`,
  `driftAction(drift: number, kind: "audio" | "video"): { seek: boolean; rate: number }`
  (audio: seek past 0.12 s as today; video: seek past 0.25 s, else a rate nudge
  of at most ±3 %, 1.0 inside one frame), and
  `useSlavedMedia(master, slaves: { element, leadMs }[], { kind, bindKey })`.
- `CamStage({ clip, layout, master, box })` renders the clean `<video>` and the cam
  `<video>` (muted, `preload="auto"`), the cam placed at `camBox` inside the
  picture box, shape by `border-radius`/`clip-path`, mirror by `scaleX(-1)`.

- [ ] **Step 1: Failing tests** `slaved.test.ts`: `slave_time_adds_the_lead`
  (master 3.0 s, lead −40 → 2.96), `a_small_video_drift_nudges_the_rate`
  (0.05 s ahead → rate < 1, no seek), `a_large_drift_seeks`, `audio_seeks_past_0_12_s`.
- [ ] **Step 2:** `npm test` → FAIL.
- [ ] **Step 3:** Implement `slaved.ts`, `useSlavedMedia` (hard seeks on play,
  seeked and a fresh element; nudges driven by `requestVideoFrameCallback` on the
  master), move the stems in `useClipMix` onto it, build `CamStage`, mount it in
  `ClipPlayer` while `clip.facecamView` is set and hide the master's picture
  (opacity 0, it keeps its sound). `save()` and `restoreOriginal()` release the
  slaved elements as they release the master (the first composite moves the clip
  file). `freezeFrame` draws the clean frame plus the cam box.
- [ ] **Step 4:** `npm test`, `npx tsc --noEmit -p .` pass; in the dev build
  (CDP) a ready clip shows one face at its place, follows play/pause/seek/the
  trim loop; measure drift after 2 min < 1 frame (log both `currentTime`s).
- [ ] **Step 5:** Commit.

### Task 3: `RectLayer` — moving the cam

**Files:** Create `src/components/ui/RectLayer.tsx`. Modify
`src/components/ShotViewer.tsx` (`CropLayer` → `RectLayer mode="crop"`),
`CamStage.tsx`, `src/lib/facecam.ts` (+ `facecam.test.ts`).

**Interfaces — Produces:** `RectLayer({ box, rect, ratio, mode, snap, min, onChange,
onCommit })` as the rough plan; `layoutFromRect(rect, frame): Pick<CamLayout, "x" |
"y" | "size">`, the inverse of `camBox`.

- [ ] **Step 1:** Pull `CropLayer`'s move/resize/ratio/anchor logic out into
  `RectLayer` with **no behaviour change**; ShotViewer keeps draw-new, the
  `MIN_EDGE` reset, the shadow and the thirds grid. `npx tsc` clean; in the dev
  build the screenshot crop draws, moves, resizes (incl. 9:16) as before. Commit
  this alone.
- [ ] **Step 2: Failing test** `facecam.test.ts`:
  `layout_round_trips_through_pixels` (for corners and centre at sizes 0.12, 0.25,
  0.5 on 1920×1080: `layoutFromRect(camBox(l))` within 0.002 of `l`).
- [ ] **Step 3:** `npm test` → FAIL; implement `layoutFromRect`; add
  `mode="box"` (no new draw; a click outside reaches the video), snapping to the
  four corners plus the margin, circle → ratio 1:1 anchored at its corner.
- [ ] **Step 4:** `npm test`, `tsc` pass; dev build: drag, resize, snap. Commit.

### Task 4: Saving a changed layout, delay, or hidden cam

**Files:** Modify `src-tauri/src/edit.rs`, `src-tauri/src/facecam.rs`,
`src-tauri/src/commands.rs` (`apply_clip_edit`, `restore_clip_original`, `store`),
`src/lib/ipc.ts`, `src/store.ts`.

**Interfaces:**
- Consumes `file_shows_cam` (Task 1).
- Produces `pub struct FacecamEdit { layout: CamLayout, delay_ms: i32 }` (serde
  camelCase); `edit::apply(.., facecam: Option<&FacecamEdit>, ..)` — `make_plan`
  runs `cam_part` on a copy of the row whose `facecam.layout`/`delay_ms` are the
  wanted ones; `store()` writes the column (state `Ready`) only after the file.
- `CamPart` gains `Hidden`: wanted layout not visible while `file_shows_cam`.
  `plan`: `Hidden` → `VideoSource::Original` (the clean picture),
  `video_start_ms = absolute_start`, `reencode = absolute_start > 0`.
- `cam_part`: the clean source exists when `video_path(id)` is a file **or** the
  file shows no cam; `differs` uses `same_layout(a, b)` (the 0.002 tolerance) and
  the exact delay against `applied`.
- `archives_the_clip`: "composited" = `file_shows_cam(id)`, not the row's layout.
- After a `Hidden` save, facts' `applied` becomes `None`.
- Copy: while saving a facecam change the status reads "Placing the camera…".

- [ ] **Step 1: Failing tests** (`edit.rs`): `a_moved_cam_composites_from_the_clean_sources`,
  `a_changed_delay_composites`, `a_hidden_cam_reads_the_clean_picture`,
  `a_hidden_cam_without_a_front_cut_copies_the_clean_picture` (untrimmed:
  `Original`, start 0, no re-encode), `a_hidden_cam_on_a_trimmed_clip_reencodes`,
  `a_cam_shown_after_a_hidden_save_composites_from_the_clip_file`,
  `a_move_inside_the_tolerance_is_no_change`,
  `the_file_not_the_row_says_whether_the_cam_is_in_it`,
  `undo_after_a_move_keeps_the_new_place`, `a_clip_without_facecam_plans_exactly_as_before`.
  (Plan-level tests build `CamPart` and facts directly; filesystem cases use the
  existing temp-dir helpers of stage 2's edit tests.)
- [ ] **Step 2:** `wcargo test --lib edit` → FAIL.
- [ ] **Step 3:** Implement; `apply_clip_edit` takes `facecam: Option<FacecamEdit>`;
  `restore_clip_original` keeps the stored layout; `ipc.ts`/`store.ts` pass it.
- [ ] **Step 4:** `wcargo test --lib` all green (stage 2's
  `a_composited_clip_is_never_filed_as_the_original`,
  `undo_on_a_facecam_clip_re_composites`, `the_cam_comes_after_the_stems` included).
- [ ] **Step 5:** Commit.

### Task 5: The Camera section in the editor

**Files:** Modify `src/components/ClipEditor.tsx`, `src/components/ClipPlayer.tsx`,
`src/lib/facecam.ts` (+ test).

**Interfaces:** `type FacecamDraft = { layout: CamLayout; delayMs: number }`;
`sameFacecam(a, b): boolean` (the shared tolerance); the player's `current` carries
`facecam`, `dirty` includes `!sameFacecam`, `save()` passes `FacecamEdit`.
Section rows: "Camera" (show/hide), "Shape" (Rectangle · Rounded · Circle),
"Mirror", "Sync" (slider −300…300, step 10, value in ms), a "Reset" button (layout
from the settings). A baked clip shows one line: "The camera is fixed in this clip."
Clips without a view show no section. Free up space (stage 2's
`bakeClipCam`) is offered for every `ready` facecam clip, trimmed or not.

- [ ] **Step 1: Failing test** `facecam.test.ts`:
  `same_facecam_ignores_moves_inside_the_tolerance` (0.001 → same, 0.003 → not,
  delay 10 ms → not).
- [ ] **Step 2:** `npm test` → FAIL; implement; wire the section and dirty state.
- [ ] **Step 3:** `bake_all_cams` skips a clip whose row says `pending`
  (failing test first: `bake_all_cams_skips_a_clip_whose_job_is_pending` on the
  pure selection it uses).
- [ ] **Step 4:** `npm test`, `tsc`, `wcargo test --lib` pass; dev build on a real
  clip: move → Save → reopen shows the new place; hide → one clean picture; show
  again; undo. Commit.

### Task 6: Export on the output's short side, and the portrait export

**Files:** Modify `src-tauri/src/export.rs`, `src-tauri/src/composite.rs`,
`src-tauri/src/commands.rs` (`shrink`, `export_clip`), `src/components/ExportDialog.tsx`.

**Interfaces — Produces:** `pub enum Orientation { Landscape, Portrait }`;
`pub const PORTRAIT: (u32, u32) = (1080, 1920)`;
`pub struct PortraitGeometry { band: Rect, game_window: Rect, game_scaled: (u32, u32), out: (u32, u32) }`
and `pub fn portrait_geometry(game, cam: Option<(u32, u32)>, layout: &PortraitLayout, out) -> PortraitGeometry`;
`pub fn portrait_filter(geometry: &PortraitGeometry, cam: Option<&PortraitCam>, mark: Option<&MarkInput>) -> String`
with `PortraitCam { width, height, timing: CamTiming, gaps_ms, mirror }`,
`MarkInput { png: PathBuf, rect: Rect }` as the rough plan (watermark drawn again
only when on and the window cuts the original mark out).
`recipe()` steps 1080 → 720 → 480 on the output's short side; `size_for(frame, short)`.

- [ ] **Step 1: Failing tests** (`export.rs`, `composite.rs`):
  `a_portrait_export_is_never_squashed` (1080×1920 at 100 MB stays; starved →
  720×1280 → 480×854), `a_landscape_export_steps_as_before` (existing tests green),
  `portrait_uses_pad_and_overlay_never_vstack`, `the_game_window_follows_game_x`
  (0, 0.5, 1 → left, centre, right, all even), `the_band_falls_back_to_a_blurred_strip_while_the_cam_is_absent`,
  `portrait_without_a_cam_centres_the_game_on_a_blurred_fill`,
  `portrait_audio_comes_from_the_clip_file`, `every_size_and_position_is_even`,
  `the_mark_is_drawn_again_when_the_window_cuts_it_out`,
  `a_window_that_keeps_the_corner_keeps_the_original_mark`,
  `no_mark_is_drawn_while_the_watermark_is_off`.
- [ ] **Step 2:** `wcargo test --lib` → FAIL.
- [ ] **Step 3:** Implement; `shrink`/`export_clip` take `orientation`;
  portrait inputs: clean source (`facecam_view.clean_path`, seek by
  `clean_lead_ms`), the cam with `cam_timing`, the clip file for audio
  (`-map 2:a:0?`); a pending clip waits via `composite::wait_settled`; a baked or
  received clip gets the game-only portrait. `ExportDialog`: "Landscape · Portrait
  9:16", suggested name gets ", 9x16"; its TS recipe mirror keeps the Rust test
  vectors in a comment.
- [ ] **Step 4:** Tests green; a real portrait export probed with ffprobe
  (1080×1920, audio present) and looked at frame by frame. Commit.

### Task 7: The 9:16 preview in the editor

**Files:** Modify `CamStage.tsx`, `ClipEditor.tsx`, `ClipPlayer.tsx`,
`src/lib/facecam.ts` (+ test); `src-tauri/src/composite.rs` (test only).

**Interfaces:** `portraitBoxes(stage, game, cam, layout)` mirrors
`portrait_geometry` (same rounding); a "9:16" toggle in the Camera section; split
handle 0.25–0.5 and a sideways-dragged game window edit `PortraitLayout`, saved
through a new command `set_clip_portrait(id, portrait)` that writes the column
and runs no ffmpeg (main window only, permission added).

- [ ] **Step 1: Failing tests:** Rust `portrait_geometry_matches_the_filter` (the
  filter's crop/pad/overlay literals come from the geometry), TS
  `portrait_boxes_match_the_core` (the same three vectors as the Rust test, numbers
  copied into a comment on both sides).
- [ ] **Step 2:** Run → FAIL; implement (layouts switch by CSS only — moving a
  `<video>` to another parent reloads it).
- [ ] **Step 3:** Tests green; dev build: the preview of a draft and its export
  side by side match. Commit.

### Task 8: Ship check

- [ ] `wcargo test --lib`, `npm test`, `npx tsc --noEmit -p .`, `npm run build`
  (PowerShell), WSL `cargo check --lib`, `wcargo build --examples`.
- [ ] Dev build, buffer running: move the cam on a 30 s and a 2 min clip (note the
  times next to `ffmpeg-compositing.md` §3.2's estimates in the spec); hide/show
  on clips saved with the cam visible and hidden; undo; Free up space; a portrait
  export; an old, a received and a pending clip.
- [ ] Final whole-branch review of this stage; ledger rulings; hand Fabi the
  manual list.
