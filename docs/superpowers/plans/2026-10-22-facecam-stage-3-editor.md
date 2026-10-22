# Facecam Stage 3: Editor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. This plan is rough on purpose (Fabi: "the later stages may stay rough"): before a task is executed, expand it into checkbox steps with superpowers:writing-plans, against the code as it stands after stage 2.

**Goal:** In the player, the cam of a clip can be moved, resized, reshaped, mirrored, hidden and re-synced. Save re-composites from the clean sources when the layout or the delay changed. The export dialog writes a separate 9:16 portrait file, and the editor can preview it.

**Architecture:** The clip's `<video>` stays the clock, but its picture is hidden. Two muted, slaved elements show the clean picture and `cam.mp4`, placed with CSS over the letterbox (`pictureBox.ts`). The binding is pulled out of `useClipMix` into a `useSlavedMedia` hook. The cam box is a `RectLayer` pulled out of ShotViewer's `CropLayer`. Save sends the layout and the delay together with the trim and the mix. Stage 2 already re-composites on a front cut with the layout unchanged (3.4 row 5), on undo, and when the file does not show the stored layout (`differs`, a `failed` clip), all with the stored layout (spec 2.7: `CamPart`, `CamInputs`, `VideoSource::CleanWithCam`); stage 3 adds the UI to change that layout, the hide and copy cases (3.4 rows 1–3), and a layout or delay changed in the editor as a reason to re-composite (that half of row 4). Portrait is a separate export target with its own pure graph builder, and the size recipe is reworked to step on the output's short side.

**Tech Stack:** Rust (Tauri 2 core; `cargo test --lib` on Windows), React + TypeScript (this repo has no frontend unit tests, so UI work is checked with `tsc`, `npm run build` and the browser mock), ffmpeg 9.0.1 essentials.

**Spec:** `docs/superpowers/specs/2026-10-20-facecam-design.md`, sections 3.1–3.6 (of 3.4, stage 3's are rows 1–3, the hide and copy cases, and the *layout or delay changed* half of row 4; stage 2 already does row 5, a front cut with the layout unchanged, undo, and the `differs` case of a `failed` clip), plus 2.6 (items 3 and 5), 2.7, and Open for Fabi 2, 6, 9 and 10. Background: `docs/extras/research/ffmpeg-compositing.md` §2.a–2.f and §5.3–5.6.

**From stage 2** (Task 1 checks each one):
- `model.rs`: `CamLayout`, `CamShape`, `PortraitLayout`, `FacecamConfig`; `Clip.facecam: Option<ClipFacecam>` (`layout`, `portrait`, `delay_ms`, `state: FacecamState` = `Pending | Ready | Failed | Baked`), written with `Library::set_facecam`.
- `facecam.rs`: `FacecamFacts` (incl. `cam_offset_us`, `gaps_ms`, `video_start_us`, `applied`/`applying: Option<FacecamApplied>`), `read_facts`, `write_facts`, `cam_path`, `has_store`.
- `composite.rs`: `cam_rect`, `cam_crop`, `cam_timing` → `CamTiming`, `CamChain`, `cam_filter`, `mask_gray8`, `write_mask_png`; the worker's `enqueue`, `cancel`, `wait_settled`.
- `edit.rs`: `clip_lock(id)`, `VideoSource::CleanWithCam`, `CamPart { None, Shown { differs } }`, `plan(.., cam: CamPart)`, `restore_plan(.., cam: CamPart)`, `CamInputs`, `files_away`, `Speed`/`video_args_at`, `remove`/`remove_everything`, `bake(clip_id)`, `discard_original` → `Discarded::Baked`.
- Baking: the commands `bake_clip_cam` and `bake_all_cams`, the gallery menu's "Free up space…" for a facecam clip (`clipMenu.tsx`, `onBakeCam`), and the `BakeCams` card on the storage page.
- `StorageUsage.facecam_bytes` and the storage row "Clean pictures for moving the cam"; `src/lib/facecam.ts` (`camBox`, `cornerOf`, `layoutAt`).

## Global Constraints

- Line numbers are those of 401a320. Stage 2 moves some of them, so look each one up again.
- All user-visible text is in English. Commits are in English, **without** a `Co-Authored-By` line.
- Build: `cargo test --lib` and `npm run build` via PowerShell; `npx tsc --noEmit` works from WSL.
- Every ffmpeg number is computed in Rust, and the graphs contain only literals. Builders are pure functions with unit tests in the style of `edit.rs`/`export.rs`: sentence names, plus a doc comment naming the bug the test guards.
- **Always from the clean sources.** No composite ever reads a file that already has the cam in it: not the clip file while its facts' `applied` shows a visible cam, and never a composite filed away as "the original".
- No `vstack`, no `shortest=1`. The cam chain keeps stage 2's `trim=end=…`, and `-t L` stays the last output option (`ffmpeg-compositing.md` §2.e).
- A clip without facecam data (old clips, received clips, cam off) and a baked clip keep exactly today's paths (`CamPart::None`). Every new branch has a test for the "absent" case.
- Every rewrite of a clip runs under `edit::clip_lock(id)`, and the clip is re-read and planned *inside* the lock. Layout-only saves make review EDIT-7 (a stale snapshot behind the lock) reachable.
- Lock order as stage 2 fixes it: `edit::clip_lock(id)` → `edit::WORKING` → `AppState.library`.
- A portrait export never rewrites the clip.
- The default shape and mirror follow Open for Fabi 9 (rounded, not mirrored) until Fabi decides otherwise.

## Review Focus

- **Double cam:** a re-composite reading a composited file, or a composite filed away as the original → Task 5 `a_composite_never_reads_a_file_with_the_cam_in_it`, `a_cam_shown_after_a_hidden_save_composites_from_the_clip_file`; stage 2's `a_composited_clip_is_never_filed_as_the_original` stays green.
- **A mix-only save or an undo losing a moved cam:** stage 2 keeps the store and re-composites undo with the stored layout → Task 5 `undo_after_a_move_keeps_the_new_place`, plus stage 2's `trim_bookkeeping_never_clears_a_facecam_store` and `undo_on_a_facecam_clip_re_composites`.
- **Preview and file disagree** on where the cam starts (the sign of offset + delay, and the clean clip's `video_start_us`) → Task 1 `the_view_takes_its_lead_from_cam_timing`, Task 2 `the_player_and_ffmpeg_agree_on_where_the_cam_starts`.
- **Portrait squashed** to 608×1080, or the dialog's TS mirror disagreeing with the core → Task 7 `a_portrait_export_is_never_squashed`.
- **Free up space** racing a pending job → Task 6 `bake_all_cams_skips_a_clip_whose_job_is_pending`.

## What stage 3 still has to close

1. Stage 2's `CamPart` treats a hidden layout as `None` and counts the clip file as the clean source only while the state is `failed`; it can, because in stage 2 the layout never changes after the save. Once it can, two more cases appear: a cam switched on in a clip that was saved with it hidden (state `ready`, no `applied`: the clip file *is* the clean picture, spec 3.2), and a cam hidden in a clip whose file shows it (3.4 rows 1–2 must read the clean original, not the clip). Task 5.
2. Stage 2 bakes from the gallery's menu (`bake_clip_cam`) and from the storage page (`bake_all_cams`, the `BakeCams` card), but the editor's *Free up space* is still only shown for trimmed clips (`ClipEditor.tsx:187`), so an untrimmed facecam clip needs the button in the editor too; and `bake_all_cams` should skip a clip whose cam is still on the way instead of waiting for it. Task 6.

---

### Task 1: Stage-2 hand-off and what the player needs per clip

**Files:**
- Modify: `src-tauri/src/model.rs` (`Clip` 657-713: a derived, read-only `facecam_view: Option<FacecamView>` next to stage 2's `facecam`, like `original_available`)
- Modify: `src-tauri/src/facecam.rs` (`FacecamView`, `view_of`)
- Modify: `src-tauri/src/clips.rs` (read path; the column itself is stage 2's)
- Modify: `src/lib/types.ts` (`Clip` 350), `src/lib/mock.ts` (clips 156-285: stage 2 left one `pending` and one `ready`; add hidden at save, `failed`, `baked` and no facecam, with clean and cam URLs)

**Interfaces:**
- `pub struct FacecamView { clean_path: String, cam_path: String, cam_width: u32, cam_height: u32, lead_ms: i64, gaps_ms: Vec<(u64, u64)>, applied: Option<FacecamApplied> }`. It is derived when the clip is read and is `None` when the store is gone (`has_store` false) or the state is `Baked`. `clean_path` is `originals/<id>/video.mp4`, or the clip file while that is still clean (state `pending` or `failed`, or no `applied` because the cam was hidden at save).
- `lead_ms` comes from stage 2's `composite::cam_timing` (offset + delay, and `video_start_us`): the **one sign rule**. Nothing in stage 3 computes it again; the player only adds the number.

**Tests:**
- `the_view_takes_its_lead_from_cam_timing`: −40 ms shows the cam 40 ms earlier, and the view's number equals what `cam_timing` gives for the same facts.
- `a_clip_without_facecam_reads_as_none`, `a_received_clip_has_no_view`
- `the_view_is_none_once_the_store_is_gone`, and for a `Baked` clip
- Re-check from stage 2: `the_facecam_column_survives_a_reinsert`, `an_unknown_shape_loads_as_a_rectangle`.

**Stage-2 hand-off check** (each one is a stage-2 test or interface; anything missing becomes a step here, or stop and ask):
- the store survives an untrimmed save and the startup repair (Task 17: `trim_bookkeeping_never_clears_a_facecam_store`, `startup_repair_leaves_a_facecam_store_alone`);
- "Clear all trims" skips facecam folders (`clear_all_trims_leaves_facecam_clips_alone`);
- a composite is never filed as the original (`a_composited_clip_is_never_filed_as_the_original`);
- `clip_lock` exists and `edit::run`, `delete_clip`, `file_clip` and `shrink` take it (Task 4);
- `cam_rect`, `cam_timing`, `cam_filter` and the mask writer exist (Task 15);
- `composite::cancel` drops a *queued* job and `wait_settled` waits for a running one (Task 16);
- a front cut and undo re-composite with the stored layout (Task 18: `CamPart`, `CamInputs`, `CleanWithCam`).

**Risks:** if Fabi chose (b) for Open for Fabi 10 (layout inside `ClipEdit`), every rebuild of `ClipEdit` must carry it: `commands.rs:1413`, `edit.rs:942`, `share.rs:887`. Received clips (`share.rs`) get `facecam: None` from stage 2 and have no store, so the view must be `None` there even if a sender's clip carries a cam in its picture.

**Done when:** one `Clip` gives the player everything a facecam clip needs, `mock.ts` has the six fixtures, and every hand-off item is either green or a step of this stage.

---

### Task 2: Preview — `useSlavedMedia` and the clean + cam stage (spec 3.2)

**Files:**
- Create: `src/lib/useSlavedMedia.ts`
- Modify: `src/lib/useClipMix.ts:311-375` (the stems use the hook; their behaviour stays exactly as it is)
- Create: `src/components/CamStage.tsx` (the clean `<video>` and the cam `<video>`; shape with `border-radius`/`clip-path`, mirror with `scaleX(-1)`)
- Modify: `src/components/ClipPlayer.tsx`
  - 744-750: hide the clip element's picture (it keeps its audio when the stems are not separate, `useClipMix.ts:283`); mount `CamStage` with `usePictureBox(frame, clip.width, clip.height)` (`pictureBox.ts:89-100`);
  - `save()` 566-601 and `restoreOriginal()` 607-640: release the slaved elements as well. The first composite moves the clip file into the store, and Windows will not move an open file.
- Modify: `src/lib/dom.ts:29-49` (`freezeFrame` draws the clean frame and the cam box, so the frozen picture behind the save veil shows the cam where it is going, not the old composite)

**Interfaces:**
- `useSlavedMedia(master: RefObject<HTMLVideoElement | null>, slaves: Array<{ element: HTMLMediaElement | null; offsetSec: number }>, options: { kind: "audio" | "video"; bindKey: unknown })`
  - Audio: today's 0.12 s hard seeks.
  - Video: hard seeks only on `play`, `seeked` and a fresh element. Drift is corrected with small `playbackRate` nudges (about ±3 %), driven by `requestVideoFrameCallback` on the master.
- The cam box is stage 2's `camBox(layout, width, height)` (`src/lib/facecam.ts`, the rule of `composite::cam_rect`), offset by the `PictureBox`. No second pixel rule.

**Tests:**
- Rust: `the_player_and_ffmpeg_agree_on_where_the_cam_starts` (the `lead_ms` the view hands out equals the `-ss`/`setpts` values stage 2's `edit::arguments` writes for the same clip).
- UI (manual, there is no test runner): with `mock.ts`, the cam follows play, pause, seek, the trim loop (`ClipPlayer.tsx:368-408`) and rate changes. On a real clip in WebView2, record a stopwatch on screen and in front of the cam: after two minutes of playback the two differ by less than one frame.

**Risks:**
- Three decoders on WebView2's UI thread; `ClipPlayer.tsx:339-346` records frame drops from main-thread load.
- H.264 seeks decode from the last keyframe. The cam's 1 s GOP keeps this short; the clean picture has the game's GOP.
- The CSS box must match `object-contain` exactly, or the cam lands a few pixels off.
- The clean clip starts its video at about 0.0213 s (SAVE-7, `video_start_us`). Check where WebView2 puts that on `currentTime`, or the preview sits one frame off the file.
- While the clip is pending, failed or saved with the cam hidden, the clean element and the master both play the clip file. That is two decoders on one file, which works, but both elements must be released on save.

**Done when:** a ready clip shows exactly one face at its saved place, the cam follows seeks within a frame, and a save releases every element.

---

### Task 3: `RectLayer` and moving the cam (spec 3.3)

**Files:**
- Create: `src/components/ui/RectLayer.tsx`. It is pulled out of `ShotViewer.tsx:1338-1514` (`CropLayer`): move, four-corner resize, pointer capture, the ratio lock in `spanned()` (1381-1406), and the opposite-corner anchor (1416-1420, 1453-1463).
- Modify: `src/components/ShotViewer.tsx`. The crop becomes `RectLayer mode="crop"`. These stay crop-only: drawing a new box, the `MIN_EDGE` reset to the whole picture (63, 1445-1448), the darkening shadow (1484) and the thirds grid.
- Modify: `CamStage.tsx` (`RectLayer mode="box"`)

**Interfaces:**
- `RectLayer({ box, rect, ratio, mode, snap, min, onChange, onCommit })`
  - `rect` is in picture pixels;
  - `ratio: number | null`: 16:9, or 1:1 for the circle;
  - `mode: "crop" | "box"`;
  - `snap`: the four corners plus the margin;
  - `min`: the size at `layout.size = 0.12`.
- In `mode="box"`, a click outside the box does nothing: there is no new draw, and the click still reaches the video, so click-to-pause and double-click fullscreen keep working (`ClipPlayer.tsx:754-755`).
- `layoutFromRect(rect, frame): Pick<CamLayout, "x" | "y" | "size">` in `src/lib/facecam.ts`, the inverse of stage 2's `camBox`.

**Tests:**
- Rust twin: `layout_round_trips_through_pixels` (through `composite::cam_rect`).
- Manual: the screenshot crop behaves exactly as before (draw, move, resize with every preset including 9:16, reset below `MIN_EDGE`); the cam box snaps to all four corners; switching to the circle keeps the box anchored at its corner.

**Risks:** the crop is a shipped feature. Pull the code out first with no change in behaviour, commit that, and only then add `mode="box"`.

**Done when:** ShotViewer's crop is unchanged, and the cam box in the player can be dragged, resized and snapped.

---

### Task 4: The Facecam section and dirty detection (spec 3.3)

**Files:**
- Modify: `src/components/ClipEditor.tsx`
  - a new section between *Audio tracks* (108) and *Trim* (163): *Show cam*, shape (`Segmented`: Rectangle, Rounded, Circle, as on stage 2's Facecam tab; `CamShape::Rect` reads "Rectangle"), *Mirror*, *Reset to default*, *Cam sync* (`Slider` with `onCommit`, `Controls.tsx:91-130`, −300…300 in steps of 10), a *9:16 preview* toggle (Task 8), and the bake button (Task 6);
  - `reencodes` (92) is also true when the layout or the delay changed;
  - the status texts at 180-185 and 256-259.
- Modify: `src/components/ClipPlayer.tsx` (`current` 541-545 gets `facecam`; `savedEdit`/`sameEdit` 1074-1108; `dirty` 550-557)

**Interfaces:**
- `type FacecamDraft = { layout: CamLayout; delayMs: number; portrait: PortraitLayout }`
- `sameFacecam(a: FacecamDraft, b: FacecamDraft): boolean`: the layout is compared within 0.002 of the frame, and the delay exactly. Task 5's `layout_changed` uses the same tolerance.

**Copy:** "Show cam", "Shape", "Mirror", "Reset to default", "Cam sync" with the hint "Negative shows the cam earlier. Clap once to check." While saving: "The cam is being put where it stands now — the video is recomputed for it." A baked clip gets one line instead of the controls: "The cam is fixed in this clip."

**Tests:** manual with the mock fixtures. Moving the cam enables Save, and moving it back disables it again. *Reset to default* takes the layout from the settings. A clip without facecam data (old or received) shows no section; a baked one shows the line.

**Risks:** review EDIT-13 (a 250 ms tolerance in the core against 50 ms in the UI) already makes "dirty" confusing, so don't add a third tolerance.

**Done when:** every facecam change shows "Not saved yet", and nothing else does.

---

### Task 5: Save with a changed layout or delay, and hiding the cam (spec 3.4 rows 1–3 and the changed half of row 4, 3.6)

Stage 2 already has the composite path in `edit.rs` (`CleanWithCam`, `CamPart`, `CamInputs`, the cam after the stems, one `-filter_complex`, `-map [v]`, the x264 retry, `applying`/`applied`, `files_away`). This task feeds it the *wanted* layout instead of the stored one.

**Files:**
- Modify: `src-tauri/src/edit.rs`
  - `CamPart` gets `Hidden`: the wanted layout hides a cam the clip file shows → `VideoSource::Original` (the clean `video.mp4`), a copy without a front cut, a re-encode without an overlay with one (rows 1–2);
  - a pure `cam_part(..)` that builds `CamPart` from the clip, its facts and the wanted `FacecamEdit`; `differs` is `layout_changed` against what the file shows; the clip file counts as the clean source also when `applied` is `None` (saved with the cam hidden), so `files_away` files it away before the cam goes in;
  - `run()` (398-521): plans inside `clip_lock` with the wanted layout and writes it into `applied`.
- Modify: `src-tauri/src/commands.rs` (`apply_clip_edit` 1075-1100 takes `facecam: Option<FacecamEdit>`; `store()` 1383-1430 writes the column through `Library::set_facecam`; `restore_clip_original` 1102-1126 uses the saved layout)
- Modify: `src/lib/ipc.ts:177-182`, `src/store.ts:131-136`, `599-611`
- Create: `src-tauri/examples/cam-edit-probe.rs` (like `trim-probe.rs`: composite a real clip, move the cam, cut the front, undo, then check by eye and with ffprobe)

**Interfaces:**
- `pub struct FacecamEdit { pub layout: CamLayout, pub delay_ms: i32, pub portrait: Option<PortraitLayout> }`
- `pub fn cam_part(clip: &Clip, facts: Option<&FacecamFacts>, wanted: &FacecamEdit) -> CamPart`
- `fn layout_changed(applied: Option<&FacecamApplied>, wanted: &FacecamEdit) -> bool`. It compares against what the file **shows** (`applied`), not against the DB. A pending or failed clip, or one saved with the cam hidden, has no cam in its file, so "unchanged" still composites once the cam is visible.
- A save on a pending clip waits for its job, as stage 2 does (`composite::wait_settled`). Design call: when the save composites anyway, a job that is only *queued* may instead be dropped with `composite::cancel`.
- A save where only `portrait` changed writes the column and runs no ffmpeg.

**Tests:**
- `a_moved_cam_composites_from_the_clean_sources` and `a_changed_delay_composites` (row 4)
- `a_hidden_cam_without_a_front_cut_copies_the_clean_picture` (row 1)
- `a_hidden_cam_with_a_front_cut_reencodes_without_an_overlay` (row 2)
- `a_cam_shown_after_a_hidden_save_composites_from_the_clip_file`
- `a_move_inside_the_tolerance_is_no_change` (row 3: the clip file, `-c:v copy`)
- `a_composite_never_reads_a_file_with_the_cam_in_it`
- `undo_after_a_move_keeps_the_new_place`
- `portrait_only_changes_write_no_video`
- `a_clip_without_facecam_plans_exactly_as_before`
- Re-check from stage 2: `a_front_cut_on_a_facecam_clip_composites_from_the_clean_picture`, `a_tail_trim_on_a_facecam_clip_stays_a_copy`, `undo_on_a_facecam_clip_re_composites`, `the_cam_comes_after_the_stems`; the existing `plan`/`restore_plan` tests with `CamPart::None` unchanged.

**Risks:**
- A double cam (a composite that reads a file which already has the cam), and an undo that brings the cam back at its old place.
- "Shortening at the back is lossless" no longer holds after a layout change: correct the texts in `ClipEditor.tsx` and the README.
- With the buffer running, the hardware encoder may be refused, and the x264 retry then runs on the CPU while the user plays. Editor composites use `Speed::Edit` (`ffmpeg-compositing.md` §4), so a long clip takes a while.
- The `clip-progress` event is shared with exports, so an editor composite and an export running at once must not mix up their progress.
- A composite failing with hardware and with x264 keeps the clip, leaves `applied` unchanged, and gives one notice.

**Done when:** rows 1–3 and a changed layout or delay (row 4) work on a real clip, undo brings back the whole length at the new place, and an old clip saves exactly as before.

---

### Task 6: Free up space in the editor for untrimmed clips (spec 3.6)

Stage 2 already bakes: `edit::bake` (trimmed or not), `discard_original` → `Discarded::Baked`, the commands `bake_clip_cam` and `bake_all_cams`, the gallery menu's "Free up space…" for a facecam clip, the `BakeCams` card on the storage page, `facecam_bytes`, and the confirmation copy. Stage 3 only adds the editor's button and one guard.

**Files:**
- Modify: `src/components/ClipEditor.tsx` (the *Free up space* question at 187-224 is also shown for an untrimmed clip whose `facecam.state` is `ready`; it calls `bakeClipCam`)
- Modify: `src-tauri/src/commands.rs` (`bake_all_cams` skips a clip whose row says `pending` instead of waiting for its job)

**Interfaces:** none new. The button uses stage 2's `api.bakeClipCam` and `store.bakeClipCam`.

**Copy:** stage 2's, unchanged: an untrimmed clip asks "Bake the cam into the clip?" with "The cam can no longer be moved in this clip."; a trimmed one "The trim stays, undo goes. The cam can no longer be moved in this clip."

**Tests:**
- `bake_all_cams_skips_a_clip_whose_job_is_pending`
- `baking_keeps_the_stems_offset` (`bake_clip_cam` leaves `clip.original`, which carries the stems offset, as `discard_original` does today)
- `a_baked_clip_plans_like_a_clip_without_facecam`
- Re-check from stage 2: `freeing_a_facecam_clips_space_bakes_the_cam`, `an_untrimmed_clip_can_be_baked_too`, `clear_all_trims_leaves_facecam_clips_alone`.
- Manual: the editor's button on an untrimmed facecam clip, then the clip plays the same and the editor shows "The cam is fixed in this clip."

**Risks:**
- The folder is shared with screenshot originals (`edit.rs:154-158`) and, from stage 4, with the clean screenshot and its cam frame. Never `remove_dir_all` here, only `remove_dir`.
- Lock order: `clip_lock(id)` → `WORKING`, never the other way round (stage 2).

**Done when:** the editor offers *Free up space* on every facecam clip, a bake-all never waits on a clip whose cam is still on the way, and the editor says the cam is fixed afterwards.

---

### Task 7: Export recipe on the output's short side, and the portrait export (spec 3.5)

**Files:**
- Modify: `src-tauri/src/export.rs`
  - `recipe()` (78-102) works on the **output** frame: steps 1080 → 720 → 480 on `min(width, height)`, never above the output's own short side;
  - `width_for()` (106-112) becomes `size_for(frame, short_side)`;
  - `arguments()` (116-155) gets the video either as a scale or as a filter graph, so `-vf` and `-filter_complex` never meet.
  - Since the TS mirror changes anyway, the millisecond arithmetic from review EDIT-3 and the 2 000 kbit/s floor (EDIT-V1) go in at the same time (design call).
- Create: `portrait_filter()` in `src-tauri/src/composite.rs` next to `cam_filter`, per `ffmpeg-compositing.md` §2.c: `pad` + `overlay` (no `vstack`), the band falling back to a blurred strip of the game, and game-only on a blurred fill when there is no cam.
- Modify: `src-tauri/src/commands.rs` (`shrink` 1151-1216 and `export_clip` 1228-1275, which goes through `shrink`)
  - take `orientation` and `portrait: Option<PortraitLayout>`;
  - portrait inputs: the clean source (as `FacecamView.clean_path`) with `-ss original.start`, the cam with `cam_timing`, and the clip file for the audio (`-map 2:a:0?`);
  - a pending clip waits for its job through stage 2's `composite::wait_settled` in `shrink`;
  - a baked or received clip gets the game-only portrait, with one line in the dialog.
- Modify: `src/components/ExportDialog.tsx` (the mirror at 35-53, an orientation `Segmented` "Landscape · Portrait 9:16", the suggested name at 99 gets ", 9x16", the preview label at 191-200)

**Interfaces:**
- `pub enum Orientation { Landscape, Portrait }`
- `pub const PORTRAIT: (u32, u32) = (1080, 1920)`
- `pub fn portrait_filter(game: (u32, u32), cam: Option<&PortraitCam>, layout: &PortraitLayout, out: (u32, u32), mark: Option<&MarkInput>) -> String`, with `PortraitCam { width, height, timing: CamTiming, gaps_ms, mirror }`
- `pub struct MarkInput { png: PathBuf, rect: Rect }`: the watermark drawn again on the 9:16 frame when the watermark is on **and** the 9:16 window cuts the mark out of the game picture (Open for Fabi 6 (a), recommended); a window that keeps the bottom-left corner keeps the original mark and gets `None`. The PNG is written to `<data>/temp/` from `watermark.rs`'s embedded picture.

**Tests:**
- `a_portrait_export_is_never_squashed`: 1080×1920 at 100 MB stays 1080×1920; a starved budget steps to 720×1280, then 480×854.
- `a_landscape_export_steps_as_before`: the existing tests at `export.rs:194-341` stay green, except where the ms arithmetic is meant to change them.
- `portrait_uses_pad_and_overlay_never_vstack`
- `the_game_window_follows_game_x` (0, 0.5 and 1: left edge, centre, right edge, all even)
- `the_band_falls_back_to_a_blurred_strip_while_the_cam_is_absent`
- `portrait_without_a_cam_centres_the_game_on_a_blurred_fill`
- `portrait_audio_comes_from_the_clip_file`
- `every_size_and_position_is_even`
- `the_mark_is_drawn_again_when_the_window_cuts_it_out` (watermark on), `a_window_that_keeps_the_corner_keeps_the_original_mark`, `no_mark_is_drawn_while_the_watermark_is_off`
- `short_clips_do_not_overshoot_the_target` (1 900 ms and 15 900 ms)

**Risks:**
- The TS mirror drifted from the core once already (EDIT-V1). Keep the same test vectors in a comment next to `preview()`.
- The mark is burned into the game picture bottom left (fact 6). A game window at `game_x = 0` keeps that corner and the original mark with it, so a mark drawn again there would show twice. Draw it again only when the watermark is on and the window cuts the original mark out (Open for Fabi 6 (a)).
- `watermark::enabled()` is today's setting, not the clip's.
- Portrait filters run on the CPU (`ffmpeg-compositing.md` §3.3), possibly while a game runs.
- The console's `export_for_discord` (`commands.rs:929-962`) stays landscape-only (its reuse of an old export after an edit, review CFG-3, is a separate fix).

**Done when:** a 9:16 file with the cam on top and the game below plays on a phone, and a landscape export gives the same arguments as before for the same input (apart from the ms change, if folded in).

---

### Task 8: The 9:16 preview in the editor (spec 3.5)

**Files:**
- Modify: `src/components/CamStage.tsx`. It gets a portrait mode that reuses the same two elements: the cam band on top, cover-cropped; the clean picture below, shifted to show the game window at `game_x`; a split handle (0.25–0.5) and a game window that drags sideways.
- Modify: `src/components/ClipEditor.tsx` (the toggle), `ClipPlayer.tsx`

**Interfaces:** `portraitBoxes(stage, game: { w: number; h: number }, cam: { w: number; h: number }, layout: PortraitLayout): { frame: Box; band: Box; gameWindow: Box }`. It only draws; the Rust numbers in `portrait_filter` are the truth.

**Tests:** Rust `portrait_geometry_matches_the_filter`, which works if `portrait_filter` takes its numbers from a pure `portrait_geometry` that the test can call. Manual: dragging the split and the window updates the draft, and the export of that draft matches the preview side by side.

**Risks:** moving a `<video>` element to another parent can reload it in WebView2. Switch layouts with CSS only.

**Done when:** what the 9:16 preview shows is what the portrait export writes.

---

### Task 9: Ship check

- Full checks: `cargo test --lib` (PowerShell), `npx tsc --noEmit`, `npm run build` (PowerShell).
- Fabi on the dev build, with the buffer and a game running:
  - move the cam on a 30 s clip and on a 2 min clip, and note how long each takes;
  - hide the cam and show it again, on a clip saved with it visible and on one saved with it hidden;
  - undo, Free up space, the storage row;
  - a portrait export, played on a phone;
  - an old clip, a received clip and a pending clip.
- **Done when:** all of the above behave as the spec says, and the measured times are noted in the spec next to the estimates of `ffmpeg-compositing.md` §3.2.
