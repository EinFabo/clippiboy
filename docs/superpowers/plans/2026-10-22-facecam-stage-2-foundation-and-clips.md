# Facecam Stage 2: Foundation and Clips Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Tasks 3, 5, 9, 11 and 14–22 end with a check on the Windows machine with a webcam (C920 or eMeet C960) and Discord; everything else is unit-tested or type-checked.

**Goal:** The webcam is buffered as its own small H.264 track next to the game picture and laid over every saved clip a few seconds after the save, at the default position from a new Facecam settings tab, with a master switch and a live preview in the console — without a save ever getting slower than today.

**Architecture:** A platform-neutral `camera` module (types, the 30 fps grid) with a Windows Media Foundation source in sharing mode behind `camera::Source` feeds a `CamTrack` that `AppState` owns next to the game pipeline: grid → own D3D11 device → second `VideoEncoder` → second `ReplayBuffer`. A save writes the clean clip exactly as today, then cuts the cam by the game's absolute QPC window into `originals/<id>/cam.mp4` plus `facecam.json`; a below-normal-priority worker composites it in and swaps the result in. `edit.rs` learns the facecam store, so trims, undo and "Free up space" keep or re-composite the cam. Five review fixes come first because the cam depends on them.

**Tech Stack:** Tauri 2 / Rust (`windows` 0.58, Media Foundation, D3D11 video processor, `png` 0.17), ffmpeg 9.0.1 (gyan essentials), React + TypeScript, zustand.

**Spec:** `docs/superpowers/specs/2026-10-20-facecam-design.md` — section 1 "Order" item 2, sections 2.1–2.10, 3.1, 3.4 rows 4–5, 3.6 (state `baked`), 5.1–5.4. Background, read where a task points to it: `docs/extras/research/camera-capture-and-clocks.md` (sections 8–9), `docs/extras/research/ffmpeg-compositing.md` (sections 2, 4, 5). Before Task 9, read the spec's "Result of the stage-1 test": it names the camera API this plan builds on.

## Global Constraints

- All user-visible text in English and short, like the other tabs. Commits in English, **without** a `Co-Authored-By` line.
- `cargo test --lib` and `npm run build` run via PowerShell on the Windows side; `npx tsc --noEmit` and `cargo check` work from WSL (`cargo check` there only sees the `#[cfg(not(windows))]` side, so the stub must compile).
- Line numbers below are those of 401a320. Every task before yours moves some; look each one up again.
- Master switch `facecam.enabled` defaults to **false**. With it off nothing of the cam exists: no thread, no device, no encoder, no ring.
- The camera is opened in **sharing mode only** (spec fact 1). Default backend: MF sensor group set to shared. Camera controls (exposure, low-light compensation) are never touched.
- One clock: QPC in 100 ns. `choose_stamp` takes the first of device / device-reference / sample time within ±2 s of the arrival time, else the arrival time (`ClockKind::Arrival`).
- Grid: 30 fps, `slot = round((qpc − origin) / period)`, `pts = slot × 10 000 000 / 30` (integer per slot); several frames in one slot keep the newest; an empty slot repeats the last frame; a slot goes out once a later frame arrived or 100 ms after its time passed; under 2 s without frames: repeats; from 2 s: black filler frames and a recorded gap; a sleep (QPC moved on more than 1 s further than `QueryUnbiasedInterruptTime`) or a jump of more than 2 s in the cam's timestamps clears the cam ring, starts a new origin and a new encoder session; a tick that is merely late catches up and never resets.
- Cam track: 1280×720 NV12 (1920×1080 for `P1080`), 30 fps, GOP 1 s, **no B-frames**, peak-constrained VBR at 3 Mbit/s (6 Mbit/s at 1080p, decided here; never constant quality, which would ignore the bitrate the ring is budgeted for), cover-cropped, on its own D3D11 device on the encoder's adapter, no watermark. Packets are track 0 in their own ring and never reach `Shared.packets` or `Recorder::video`.
- Cam ring length = buffer + game keyframe interval + 1 s + 0.3 s + 10 s, rounded up to whole seconds; memory `1.5 × bitrate × length`, floor 16 MB. Its lock is never taken together with `Shared.packets` or `Shared.recorder`.
- Cut: from the last cam keyframe at or before `G0 − 300 ms` to `G0 + L + 300 ms`, **after `muxer::build` has written the clip**, waiting at most 500 ms outside every lock. `cam_offset_us = (cam_origin + first_cam_pts·10 − G0) / 10`.
- Delay `delay_ms`: −300 … 300 in steps of 10, default −40; another −40 when the cam ran on the arrival clock; applied when compositing, never baked into the ring.
- Layout normalised to the game frame; margin 2.5 % of the frame height; every pixel number even; the filter graph holds only literals that Rust computed; no `vstack`, no per-frame `geq`, no `shortest`; never a device name or any other string from the camera in an ffmpeg argument.
- Phase 2: one worker thread, one job at a time, `BELOW_NORMAL_PRIORITY_CLASS` next to `CREATE_NO_WINDOW`, `-hwaccel d3d11va`, `-threads 4` on the game input, `-filter_complex_threads 2`, fast presets (NVENC `p3`, AMF `speed`, QSV `veryfast`, x264 `superfast`), the quantiser of `edit::quantiser`, the x264 retry.
- `ClipFacecam.state` is one of `pending`, `ready`, `failed`, `baked`. The clip file is the clean picture whenever the state is `pending` or `failed` or the cam was hidden at save.
- **SAVE-7, decided here:** offsets are measured against the clean clip's real video start (`ffprobe … stream=start_time` of `v:0`, about 0.0213 s today), not by dropping `-avoid_negative_ts make_zero` first. Phase 1 must stay byte for byte today's save, the reason for `make_zero` is undocumented and dropping it changes every clip and every edit; a probed start is local to the facecam and still right (it reads 0) if SAVE-7 is fixed later.
- Lock order: `edit::clip_lock(id)` → `edit::WORKING` → `AppState.library`. The composite worker never takes `WORKING`.
  - `composite::wait_settled` is called **before** `clip_lock` is taken, never while holding it (the job it waits for needs that lock).
  - The composite worker releases the queue mutex before it takes `clip_lock`; `delete_clip` holds `clip_lock` and then calls `composite::cancel`, which takes the queue mutex.
  - `composite::recover` runs inside the setup's library lock (`lib.rs:1258-1270`), so it takes no clip lock, and it runs before `composite::start`.
  - An old cam encoder is stopped and dropped **outside** the cam ring lock: `VideoEncoder`'s drop joins its thread, whose drain calls `on_packet`, which takes the ring lock (`mft.rs:1211-1217`, `1392-1438`).
  - Opening and closing the camera, `holder()` and `VideoEncoder::start` never run on the thread that ticks the grid (Task 11).
- Exact copy fixed by the spec: "The cam couldn't be added — try again from the editor"; status lines "Shared with other apps", "Taken by Discord" (the holder's name), "Blocked by Windows privacy", "No camera"; Facecam tab "Windows blocks the camera for desktop apps"; storage row "Clean pictures for moving the cam".
- Stage 2 shows no switches for recordings, screenshots or the virtual camera (spec 5.2), but keeps the config fields.

## Review Focus

- A webcam in a dim room drops to ~15 fps, and a USB hiccup stalls it for a second or three: the clip's cam must stay in sync over the whole clip (every frame twice, a short freeze, then hidden), never drift or slide. → Task 8 tests `half_the_frames_shows_each_one_twice`, `a_dropout_of_two_seconds_turns_into_fillers_and_a_gap`, `the_stall_timer_keeps_the_pts_continuous`; Task 12 test `a_gap_is_carried_in_clip_coordinates`.
- Two highlights saved seconds apart, and a clip deleted, trimmed or exported from the console while its cam is still being added: both clips get their cam in order, nothing is orphaned, nothing gets the cam twice. → Task 4 test `one_clip_one_lock`; Task 16 tests `a_clip_is_never_queued_twice`, `a_deleted_clip_leaves_the_queue`, `jobs_run_in_the_order_they_came`.
- Quit, crash or power loss during phase 2: the clip is never lost and never ends up with two cams. → Task 16 test `a_crash_mid_composite_is_resolved_from_the_file_size`.
- The PC sleeps with the buffer running and wakes without a lock screen: a clip after the wake has the cam from the wake-up on, at the right place, and the game ring holds nothing from before the sleep. → Task 2 test `a_jump_in_time_drops_what_lies_before_it`; Task 8 tests `a_tick_after_a_sleep_starts_over_instead_of_catching_up` and, the other way round, `a_late_tick_without_a_sleep_only_catches_up`; Task 12 test `a_cam_that_started_after_the_clip_gives_a_positive_offset`.
- "Cam off" in the console, then any unrelated change in the still-open main window: the camera must stay off. → Task 6 test `cam_off_from_the_console_survives_a_stale_write`.

---

### Task 1: Capture errors reach the UI after the first second (BUF-2)

**Files:**
- Modify: `src-tauri/src/pipeline.rs:302-307` (fields), `542-554` (`report`, `take_unseen_error`), `946-947` (literal)
- Test: `src-tauri/src/pipeline.rs` (tests module at 1086)

**Interfaces:**
- Produces: `pub struct ErrorSlot` with `pub fn new() -> Self`, `pub fn report(&self, scope: &str, message: String)`, `pub fn take_unseen(&self) -> Option<String>`. `Shared` gets `pub errors: ErrorSlot` in place of `error` + `error_seen`; `Shared::report` and `Shared::take_unseen_error` keep their signatures and delegate. Task 11 gives the `CamTrack` its own slot.

- [ ] **Step 1: Write the failing tests**:
  - `an_error_after_a_quiet_first_poll_is_still_reported`: `let slot = ErrorSlot::new(); assert_eq!(slot.take_unseen(), None); slot.report("capture", "gone".into()); assert_eq!(slot.take_unseen().as_deref(), Some("gone"));`
  - `an_error_is_reported_once_per_run`: after the above, `assert_eq!(slot.take_unseen(), None)`; `slot.report("capture", "again".into()); assert_eq!(slot.take_unseen(), None)`.
- [ ] **Step 2: Run** `cargo test --lib pipeline` — Expected: FAIL to compile, `ErrorSlot` not found.
- [ ] **Step 3: Implement.** `take_unseen` reads the error first and only swaps the flag when there is one (the refined fix of BUF-2 in `docs/facecam-review.md`). `report` logs at warn on the first call and every 500th after it, at debug otherwise — a removed device reports once per captured frame today.
- [ ] **Step 4: Run** `cargo test --lib` — Expected: PASS, previous count + 2.
- [ ] **Step 5: Commit** `Capture errors after the first second reach the UI again: the status tick's first poll used to mark the error as seen before there was one, so a window target that closed or a converter that failed later froze the buffer silently; the error slot is its own type now, for the cam to use too, and a failure that repeats per frame is logged once in 500`

---

### Task 2: The replay ring cuts by a range, keeps a one-keyframe stream in its budget, and drops what lies before a jump in time (BUF-4, BUF-5)

**Files:**
- Modify: `src-tauri/src/buffer.rs:31-67` (fields `jump_us: i64`, `cut_seq: Option<u64>`), `75-111` (`push`, `trim`), `151-157` (`clear` resets `cut_seq`), `171-200` (`buffered_seconds`, `snapshot`)
- Modify: `src-tauri/src/pipeline.rs:937` (`set_jump_limit_us` on the game ring)
- Test: `src-tauri/src/buffer.rs` (tests 225-408, helpers `packet`, `fill`, `NO_LIMIT`)

**Interfaces:**
- Produces:
  - `pub fn snapshot_range(&self, from_us: i64, to_us: i64) -> Vec<EncodedPacket>` — starts on the newest keyframe at or before `from_us` (else the front keyframe), keeps packets with `start ≤ pts ≤ to_us`, never starts before a pending cut. `snapshot(seconds)` becomes `snapshot_range(newest − seconds·10⁶, i64::MAX)`.
  - `pub fn set_jump_limit_us(&mut self, us: i64)` (default `i64::MAX`, i.e. off) and `pub fn newest_pts(&self) -> i64`.
  - The game ring gets `max(1 s, 3 × keyframe_seconds)`; Task 10 gives the cam ring 2 s.

- [ ] **Step 1: Write the failing tests:**
  - `a_cut_from_a_time_starts_on_the_keyframe_before_it`: `fill(&mut buf, 30)` on `ReplayBuffer::new(30, NO_LIMIT)`; `let cut = buf.snapshot_range(5_000_000, 9_000_000);` → `cut[0].keyframe && cut[0].pts_us == 4_000_000`, every `p.pts_us` in `4_000_000..=9_000_000`.
  - `a_cut_before_the_ring_starts_on_its_first_keyframe`: capacity 10 s, `fill` 30 s → `snapshot_range(0, i64::MAX)[0]` is the front packet and a keyframe.
  - `the_old_cut_is_a_range_up_to_now`: `buf.snapshot(10)` has the same length and first pts as `buf.snapshot_range(buf.newest_pts() - 10_000_000, i64::MAX)`.
  - `a_stream_with_one_keyframe_still_obeys_its_budget`: `ReplayBuffer::new(2, 100_000)`, one keyframe at 0 then 600 non-key packets of 1000 bytes at 60 fps (10 s) → `buf.bytes() <= 200_000` and `!buf.is_empty()`.
  - `a_young_single_keyframe_is_kept_whatever_the_budget`: `ReplayBuffer::new(30, 1)`, one keyframe at 0 and 60 packets → `buf.packets.front()` is that keyframe.
  - `a_jump_in_time_drops_what_lies_before_it`: `set_jump_limit_us(6_000_000)`, `fill` 10 s, then 3 s of 60 fps packets starting at 610 s with a keyframe at 610 s and 612 s → `buf.snapshot_range(0, i64::MAX)[0].pts_us >= 610_000_000`, `buf.buffered_seconds() < 4.0`.
  - `nothing_from_before_a_jump_is_cut_until_the_next_keyframe`: `fill` 10 s, then 30 non-key packets from 610 s → `snapshot_range(0, i64::MAX).is_empty()` and `buffered_seconds() == 0.0`.
  - `a_stall_under_the_limit_is_no_jump`: limit 6 s, `fill` 10 s, a 2 s hole, 2 s more → `snapshot(30)[0].pts_us == 0`.
- [ ] **Step 2: Run** `cargo test --lib buffer` — Expected: FAIL to compile, `snapshot_range` not found.
- [ ] **Step 3: Implement.** In `push`, a packet whose pts lies more than `jump_us` past `newest_pts` sets `cut_seq = Some(next_seq)`; the first video keyframe with `seq ≥ cut_seq` drops everything before it and clears `cut_seq`. While a cut is pending, `snapshot_range` only considers keyframes with `seq ≥ cut_seq` and `buffered_seconds` answers 0. In `trim`, when exactly one keyframe is left, no jump cut is pending, and that keyframe is older than the ring is long — `newest − kf ≥ capacity_us` together with `bytes > 2 × capacity_bytes`, or `newest − kf > capacity_us + 10 s` — drop that keyframe's entry so `enforce_budget_without_keyframes` holds the ring until the next one, with its own log line once: "the encoder flags only its first keyframe — the buffer is being held to its budget by force". A young last keyframe is never dropped, whatever the budget: `the_budget_never_empties_the_ring` (333-340) and `the_floor_does_not_touch_a_ring_that_has_keyframes` (390-401) run a 1-byte budget and must stay green.
- [ ] **Step 4: Run** `cargo test --lib buffer` then `cargo test --lib` — Expected: PASS, the 11 existing ring tests unchanged.
- [ ] **Step 5: Commit** `Replay ring: a cut by an absolute range for the cam to use, a stream that flags only its first keyframe stays inside its budget instead of growing until the app dies, and a jump in time (the PC slept with the buffer running) drops everything before it at the next keyframe — a save in between finds nothing old instead of a clip that is silent and hours long`

---

### Task 3: The encoder is shut down explicitly and says which one it is (CAP-V2, CAP-8)

**Files:**
- Modify: `src-tauri/src/mft.rs:136-146` (`EncoderSettings.label: &'static str`), `544-555` (`Transform.activate: IMFActivate`), `568-580` (`profile` names the label), `716` (keep the activate), `717-839` (its error paths), `843-850` (store it in the `Transform` literal), `1109-1131` (thread name `clippiboy-encoder-{label}`, `CoInitializeEx(None, COINIT_MULTITHREADED)` at the top, `CoUninitialize` at the end), `1440-1444` (after `END_STREAMING`)
- Modify: `src-tauri/src/pipeline.rs:657-665` (`label: "game"`)

**Interfaces:**
- Produces: `EncoderSettings.label`; every encoder releases its hardware session when it stops. Task 11 starts the second one with `label: "cam"`.

- [ ] **Step 1: Implement.** After `MFT_MESSAGE_NOTIFY_END_STREAMING`: `if let Ok(shutdown) = transform.transform.cast::<IMFShutdown>() { let _ = shutdown.Shutdown(); }` then `let _ = transform.activate.ShutdownObject();`, and the same `ShutdownObject` on every `build` error path after `ActivateObject` succeeded. Log once at info: `encoder ({label}) shut down`.
- [ ] **Step 2: Verify** `cargo test --lib` (PowerShell) — Expected: PASS, nothing else changed (`opens_a_gop` tests at 1447-1510 still green).
- [ ] **Step 3: Verify on the Windows machine** (NVIDIA): note `nvidia-smi --query-gpu=encoder.stats.sessionCount --format=csv,noheader` with the buffer off; switch the buffer on and off ten times; the count with the buffer off is the same number again, and the log shows ten `encoder (game) shut down` lines.
- [ ] **Step 4: Commit** `Encoder: the transform is shut down and its activation object released when it stops, as Media Foundation asks, instead of waiting for the last reference — with a second encoder for the cam every start and stop costs two hardware sessions, and GeForce cards have eight; the thread joins the multithreaded apartment itself and carries a label in its name and in the encoder profile`

---

### Task 4: One lock per clip for every operation that rewrites, moves or deletes its file (EDIT-1)

**Files:**
- Modify: `src-tauri/src/edit.rs:48-53` (new `clip_lock` beside `WORKING`, with the lock order in its doc comment), `406` (`run` takes the clip lock first, then `WORKING`)
- Modify: `src-tauri/src/commands.rs:740-777` (`delete_clip`: hold it across the library delete and the store removal), `816-841` (`file_clip`), `1151-1216` (`shrink`, for the whole ffmpeg run)
- Test: `src-tauri/src/edit.rs` (tests at 1054)

**Interfaces:**
- Produces: `pub fn clip_lock(clip_id: &str) -> Arc<parking_lot::Mutex<()>>` — the same `Arc` for the same id while anyone holds it (a `HashMap<String, Weak<…>>` behind a static `Mutex`, dead entries pruned on every call). Used by Tasks 16–19 and by stage 3.

- [ ] **Step 1: Write the failing tests:**
  - `one_clip_one_lock`: `Arc::ptr_eq(&clip_lock("a"), &clip_lock("a"))` while one of them is held; `!Arc::ptr_eq(&clip_lock("a"), &clip_lock("b"))`.
  - `a_lock_nobody_holds_is_forgotten`: take and drop `clip_lock("gone")`, call `clip_lock("other")` once → `#[cfg(test)] fn held_locks() -> usize` does not count `"gone"`.
- [ ] **Step 2: Run** `cargo test --lib edit` — Expected: FAIL to compile, `clip_lock` not found.
- [ ] **Step 3: Implement** as described under Interfaces and wire the four callers. `delete_clip` takes the lock by the requested id before `lib.get`, so a delete from the console waits for an edit of that clip instead of pulling the file out from under it.
- [ ] **Step 4: Run** `cargo test --lib` — Expected: PASS.
- [ ] **Step 5: Commit** `One lock per clip: deleting, filing and exporting a clip now wait for a save of that same clip, so a delete from the console in the middle of an editor save no longer brings the file back with no row; edits of different clips still queue on the one global lock, which the coming cam composite never takes`

---

### Task 5: A settings write built on an old copy no longer undoes a change made elsewhere (CFG-1)

**Files:**
- Modify: `src-tauri/src/model.rs:449-549` (`#[serde(default)] pub revision: u64` as the last field)
- Modify: `src-tauri/src/config.rs:25-77` (`revision: 0`); add `merge_stale` and `accept` after `keep_once_flags` (189-192)
- Modify: `src-tauri/src/state.rs:18-106` (`config_history: Mutex<VecDeque<AppConfig>>`, taken only inside the `config` lock), `369-393` (seeded with the loaded config), `419-457` (`replace_config` runs `config::accept` under the lock and uses its result for every side effect)
- Modify: `src-tauri/src/commands.rs:335-348` (the three source commands take `webview: tauri::WebviewWindow` and call `tell_main` when `webview.label() != "main"`)
- Modify: the direct writers that save under the config lock — `src-tauri/src/lib.rs:496-543` (`repair_screens`), `1164-1180` (`hide_to_tray`), `src-tauri/src/control.rs:134-146` (`regenerate_token`), `435-450` (`ensure_token`) — call `AppState::note_direct_write(&mut config)` before their `config::save`
- Modify: `src/lib/types.ts:185-240` (`revision: number`), `src/lib/mock.ts:40-153` (`revision: 0`), `src/store.ts:305-308` (`onConfigChanged`), `371-388` (`patchConfig`)
- Test: `src-tauri/src/config.rs` (tests 194-343)

**Interfaces:**
- Produces:
  - `pub fn merge_stale(base: &AppConfig, incoming: &AppConfig, current: &AppConfig) -> AppConfig` — three-way, over the JSON form: a value the window did not change (`incoming == base`) keeps `current`; objects merge key by key; arrays whose items all carry a string `id` merge by id (removed in the window → removed, added in the window → appended, both → merged recursively, removed elsewhere → stays removed); any other value the window changed wins.
  - `pub fn accept(current: &AppConfig, history: &VecDeque<AppConfig>, incoming: AppConfig) -> AppConfig` — `incoming.revision < current.revision` merges against the history entry with that revision (an unknown one is taken as it stands, with a log line); the result always gets `current.revision + 1`. History keeps the last 16.
  - `pub(crate) fn tell_main(app: &tauri::AppHandle, config: &AppConfig)` in `commands.rs`: `app.emit_to("main", "config-changed", config)`. Task 19's `set_facecam_enabled` uses it too.
  - `AppState::note_direct_write(&self, config: &mut AppConfig)` — for a writer already holding the config guard: `revision += 1` and the result into the history (history lock nested inside the config lock, never the other way round).
  - `store.ts`: a config from `onConfigChanged` or from a `patchConfig` answer is taken only when its `revision` is higher than the store's; `onConfigChanged` no longer bumps `configWrites`, so an answer in flight is not thrown away by an echo.

- [ ] **Step 1: Write the failing tests** (sources built with the existing `source()` helper; `AppConfig` has no `PartialEq`, `model.rs:449`, so whole configs are compared as `serde_json::to_value`):
  - `a_stale_window_write_keeps_the_consoles_mute`: base = mic unmuted; current = mic muted (rev 5); incoming = base with `overlay.enabled` flipped (rev 4) → merged mic `muted == true`, `overlay.enabled` flipped.
  - `two_changes_to_one_source_merge_field_by_field`: current mic muted, incoming mic `gain_db == -6.0` → both.
  - `a_source_added_in_the_window_survives_a_stale_write` and `a_source_removed_in_the_window_stays_removed`.
  - `a_write_on_the_current_revision_replaces_and_counts_up`: `accept(&current_rev7, &history, incoming_rev7)` equals incoming with `revision == 8`.
  - `a_write_from_an_unknown_revision_is_taken_as_it_stands`: history without rev 2 → incoming, `revision == current + 1`.
- [ ] **Step 2: Run** `cargo test --lib config` — Expected: FAIL to compile, `merge_stale` not found.
- [ ] **Step 3: Implement** as specified, including the four direct writers (each keeps saving under its own guard; `repair_screens` goes on telling every window) and the two `store.ts` rules.
- [ ] **Step 4: Run** `cargo test --lib` and `npx tsc --noEmit` — Expected: PASS, no type errors.
- [ ] **Step 5: Verify on the Windows machine:** mute the mic in the console, then change a banner switch in the main window, which was open all along → the mixer page shows the mic muted and the next clip has no mic.
- [ ] **Step 6: Commit** `Settings carry a revision: a write from a window whose copy is older than what the core holds is merged field by field onto the current settings instead of replacing them, and the main window hears of changes it did not make — a source muted in the console used to come back with the next unrelated switch in the main window`

---

### Task 6: The Facecam settings, loaded leniently and clamped before they can reach ffmpeg

**Files:**
- Modify: `src-tauri/src/model.rs` (new types after `FriendsConfig` 313-360; `AppConfig` 449-549 gets `#[serde(default, deserialize_with = "lenient_facecam")] pub facecam: FacecamConfig`)
- Modify: `src-tauri/src/config.rs:25-77` (`facecam: FacecamConfig::default()`)
- Modify: `src-tauri/src/commands.rs:303-325` (`clean_facecam` beside the 8d18a92 validators)
- Modify: `src-tauri/src/state.rs:419-457` (`replace_config` calls `clean_facecam` before the swap, so every write path is clamped)
- Modify: `src/lib/types.ts:185-240`, `src/lib/mock.ts:40-153`
- Test: `src-tauri/src/config.rs`, `src-tauri/src/commands.rs` (1514-1536)

**Interfaces:**
- Produces (all `#[serde(rename_all = "camelCase")]`, structs with container `default`):
  - `FacecamConfig { enabled: bool, device_id: Option<String>, device_name: Option<String>, resolution: CamResolution, fps: u32, layout: CamLayout, portrait: PortraitLayout, delay_ms: i32, in_clips: bool, in_recordings: bool, in_screenshots: bool, console_preview: bool, virtual_camera: bool }` — defaults exactly spec 5.1: `false, None, None, P720, 30, CamLayout::default(), PortraitLayout::default(), -40, true, true, true, true, false`; derives `Debug, Clone, PartialEq, Serialize, Deserialize`.
  - `enum CamResolution { P720, P1080 }` (default `P720`, JSON `"p720"`/`"p1080"`), `fn size(self) -> (u32, u32)` → `(1280, 720)` / `(1920, 1080)`.
  - `CamLayout { visible: bool, x: f32, y: f32, size: f32, shape: CamShape, mirror: bool }`, default `true, 1.0, 1.0, 0.25, Rounded, false`; `#[derive(Copy)]`.
  - `enum CamShape { #[default] Rounded, Circle, #[serde(other)] Rect }` — `#[serde(other)]` has to sit on the last variant; an unknown value loads as `Rect` (spec 3.1), `"rect"` still loads as itself.
  - `PortraitLayout { split: f32, game_x: f32 }`, default `0.35, 0.5`.
  - `fn lenient_facecam<'de, D: Deserializer<'de>>(d: D) -> Result<FacecamConfig, D::Error>` — reads a `serde_json::Value`, falls back to `FacecamConfig::default()` with `log::warn!("facecam settings unreadable ({err}) — the cam starts over from its defaults")`, and passes a block it could read through `clean_facecam` before returning it. `config::load` (`config.rs:148-174`) parses through it, so a hand-edited or old `config.json` is clamped on load too and can never reach `cam_rect` or ffmpeg unclamped.
  - `pub fn clean_facecam(cam: &mut FacecamConfig)` — `x, y, game_x` to 0…1, `size` 0.12…0.5, `split` 0.25…0.5, NaN or infinite to the default, `delay_ms` rounded to 10 and clamped to −300…300, `fps = 30`, an empty `device_id`/`device_name` to `None`.
  - TS: `FacecamConfig`, `CamLayout`, `PortraitLayout`, `CamShape = "rect" | "rounded" | "circle"`, `CamResolution = "p720" | "p1080"`; `AppConfig.facecam`. The mock is the default with `enabled: true`.

- [ ] **Step 1: Write the failing tests:**
  - `a_configuration_written_before_the_facecam_still_loads` (config.rs, pattern of 250-265: remove `"facecam"`) → `!parsed.facecam.enabled`, `parsed.facecam.layout.size == 0.25`.
  - `a_broken_facecam_block_costs_only_the_facecam`: a valid config with one source whose `"facecam"` is `{"enabled": "yes", "layout": 5}` → `parsed.sources.len() == 1`, `parsed.facecam == FacecamConfig::default()`.
  - `a_facecam_block_missing_a_field_keeps_the_rest`: `{"enabled": true}` → `enabled`, `delay_ms == -40`, `in_clips`.
  - `an_unknown_shape_loads_as_a_rectangle`: `"hexagon"` → `Rect`; `"rect"`, `"rounded"`, `"circle"` load as themselves.
  - `a_facecam_block_out_of_range_is_clamped_on_load`: `{"layout": {"size": 3, "x": -2}}` → `layout.size == 0.5`, `layout.x == 0.0`.
  - `cam_off_from_the_console_survives_a_stale_write` (Review Focus): base `enabled == true`; current `enabled == false`; incoming = base with `layout.size == 0.3` → `merge_stale` gives `enabled == false`, `size == 0.3`.
  - `facecam_numbers_are_clamped_before_they_reach_ffmpeg` (commands.rs): size 0.9 → 0.5, 0.01 → 0.12, NaN → 0.25; x −1 → 0, y 2 → 1; delay 1234 → 300, −47 → −50, 44 → 40; fps 60 → 30; split 0.9 → 0.5; `game_x` NaN → 0.5; `device_id: Some("")` → `None`.
- [ ] **Step 2: Run** `cargo test --lib config commands` — Expected: FAIL to compile, `FacecamConfig` not found.
- [ ] **Step 3: Implement** the types, the lenient field and `clean_facecam` as specified. `clean_facecam` is called from `replace_config` (`state.rs:419-457`), not only from `set_config`, so the console's write path is clamped too (spec 5.1), and from `lenient_facecam`, so loading is clamped as well.
- [ ] **Step 4: Run** `cargo test --lib` and `npx tsc --noEmit` — Expected: PASS, no type errors.
- [ ] **Step 5: Commit** `Facecam settings in the config, off by default so nothing changes on update: an unreadable cam block falls back to the cam's defaults with a log line instead of throwing away every other setting, an unknown shape reads as a rectangle, and every number is clamped in Rust before it can reach a filter graph`

---

### Task 7: The camera module's neutral half, and the device list

**Files:**
- Create: `src-tauri/src/camera/mod.rs` (types, `choose_stamp`, `classify`, `pick_device`, `cover_crop`, `colour_of`, `Backoff`, the `Source` trait, `list`/`open` dispatch, `#[cfg(not(windows))]` stubs like `audio/devices.rs:284-345`)
- Modify: `src-tauri/src/lib.rs:1-40` (`pub mod camera;`), `1323-1405` (`commands::list_cameras`)
- Modify: `src-tauri/src/model.rs:11-18` (`CameraDevice` beside `AudioDevice`)
- Modify: `src-tauri/src/commands.rs:22-40` (`#[tauri::command(async)] pub fn list_cameras() -> Vec<CameraDevice>`)
- Modify: `src-tauri/build.rs:8-90` (`"list_cameras"`), `src-tauri/capabilities/default.json` (`"allow-list-cameras"`)
- Create (generated by the next build, commit it): `src-tauri/permissions/autogenerated/list_cameras.toml`; regenerated `src-tauri/gen/schemas/*`
- Modify: `src/lib/types.ts` (`CameraDevice`), `src/lib/ipc.ts:36-204` (`listCameras`), `src/lib/mock.ts` (`mockCameras`: "HD Pro Webcam C920", "EMEET SmartCam C960")

**Interfaces:**
- Produces (stage-1 names kept):
  - `pub enum OpenMode { MfSensorShared, MfShareAttr, WinrtShared }` (the probe's two negative controls are not carried over).
  - `#[derive(Serialize, Deserialize)] #[serde(rename_all = "camelCase")] pub enum ClockKind { Device, DeviceReference, SampleTime, Arrival }`.
  - `pub fn choose_stamp(device: Option<i64>, device_ref: Option<i64>, sample: Option<i64>, arrival: i64) -> (i64, ClockKind)` — as stage 1 (±2 s).
  - `pub struct ProbeFrame` exactly as stage 1; `pub struct Frame { pub probe: ProbeFrame, pub nv12: Arc<[u8]> }` — tightly packed NV12 in system memory (row length = `probe.width`, the UV plane right after `width × height` bytes), no texture, so stage 6 can reuse it.
  - `pub enum Event { Frame(Frame), FormatChanged { width: u32, height: u32 }, Lost(Error) }`; `pub type Sink = Box<dyn FnMut(Event) + Send>`.
  - `pub trait Source: Send { fn close(self: Box<Self>); fn size(&self) -> (u32, u32); }`; `pub fn list() -> Vec<CameraDevice>`; `pub fn open(device: &CameraDevice, mode: OpenMode, sink: Sink) -> Result<Box<dyn Source>, Error>`.
  - `pub enum Error { Denied, Busy { by: Option<String> }, Gone, NoCamera, Unsupported(String), Other(String) }`; `pub fn classify(hresult: i32) -> Error` (`0x80070005` Denied, `0xC00D3704` Busy, `0xC00D3EA2` Gone, else `Other("0x…")`).
  - `pub fn pick_device<'a>(list: &'a [CameraDevice], id: Option<&str>, name: Option<&str>) -> Option<&'a CameraDevice>` — the id (case-insensitive), else a unique name, else the first when nothing was chosen, else `None`.
  - `pub struct Rect { pub x: u32, pub y: u32, pub w: u32, pub h: u32 }`; `pub fn cover_crop(src: (u32, u32), dst: (u32, u32)) -> Rect` — centred, even numbers.
  - `pub fn colour_of(matrix: Option<u32>, nominal_range: Option<u32>) -> (bool /*709*/, bool /*full range*/)` — raw MF values: matrix 1 = BT.709, 2 = BT.601; range 1 = 0–255, 2 = 16–235; untagged = BT.601 studio.
  - `pub struct Backoff` with `next(&mut self) -> Duration` (1, 2, 5, then 10 s) and `reset(&mut self)`.
  - `model::CameraDevice { id: String /*symbolic link*/, name: String }`.

- [ ] **Step 1: Write the failing tests** (`camera/mod.rs`, these run on any OS):
  - all `choose_stamp` cases of stage 1 Task 2 Step 1 (incl. the order and the ±2 s boundary), verbatim.
  - `the_errors_a_camera_gives_are_named`: the four `classify` cases above.
  - `a_replugged_camera_is_found_by_its_name` (id changed, name unique → found); `two_cameras_of_one_name_are_not_guessed` (→ `None`); `no_choice_takes_the_first`; `a_chosen_camera_that_is_gone_is_not_replaced` (id and name both missing from the list → `None`).
  - `a_four_by_three_camera_is_cropped_to_fill`: `cover_crop((640, 480), (1280, 720)) == Rect { x: 0, y: 60, w: 640, h: 360 }`; `(1920, 1080) → (1280, 720)` is the whole picture; `(1080, 1920)` gives `Rect { x: 0, y: 656, w: 1080, h: 608 }`.
  - `a_camera_without_colour_tags_is_bt601_studio_range`: `colour_of(None, None) == (false, false)`, `colour_of(Some(1), Some(1)) == (true, true)`.
  - `the_retry_waits_grow_to_ten_seconds`: `[1, 2, 5, 10, 10]` s, after `reset` 1 s again.
- [ ] **Step 2: Run** `cargo test --lib camera` — Expected: FAIL to compile, module missing.
- [ ] **Step 3: Implement** the neutral half; on Windows `list`/`open` call into `camera::windows` (Task 9), which for now returns an empty list and `Err(Error::NoCamera)`.
- [ ] **Step 4: Run** `cargo test --lib`, `cargo check` (WSL) and `npx tsc --noEmit` — Expected: PASS. Then `cargo build` in `src-tauri` once via PowerShell — `tauri-build` writes the permission TOML and the schemas; `git status` shows `list_cameras.toml` and changes in `gen/schemas/` (five tracked files: `acl-manifests`, `capabilities`, `desktop-schema`, `linux-schema`, `windows-schema`; commit every one that changed).
- [ ] **Step 5: Commit** `Camera module, the half that is the same on every system: devices, frames with the stage-1 timestamp choice, the errors a camera gives by name, finding a re-plugged camera by its name, the cover crop, the retry waits — and a list_cameras command for the settings`

---

### Task 8: The 30 fps grid

**Files:**
- Create: `src-tauri/src/camera/grid.rs` (the spec puts the grid in the neutral half; it gets a file of its own for its tests)
- Modify: `src-tauri/src/camera/mod.rs` (`pub mod grid;`)

**Interfaces:**
- Produces:
  - `pub struct Grid<T: Clone>`, `pub fn new(fps: u32) -> Self`, `pub fn push(&mut self, qpc_100ns: i64, frame: T, out: &mut Vec<Out<T>>)`, `pub fn tick(&mut self, now_100ns: i64, suspended_100ns: i64, out: &mut Vec<Out<T>>)` — `suspended_100ns` is how much longer QPC ran than `QueryUnbiasedInterruptTime` since the last tick (the worker measures it, Task 11; unbiased time stands still while the PC sleeps), so the grid tells a sleep from a late tick without reading a clock; `pub fn origin(&self) -> Option<i64>`, `pub fn stats(&self) -> GridStats`. The worker always ticks before it pushes.
  - `pub enum Out<T> { Started { origin_100ns: i64 }, Frame { pts_100ns: i64, frame: T, duplicate: bool }, Filler { pts_100ns: i64 }, GapOpened { from_100ns: i64 }, GapClosed { from_100ns: i64, to_100ns: i64 }, Reset { at_100ns: i64 } }` — `Started` comes with the frame that sets a session's origin (the very first one too) and is what the cam ring is reset to; `Reset` only says the session ended.
  - `pub struct GridStats { received, emitted, duplicates, fillers, superseded, late, resets: u64 }`.
  - Constants `EMIT_AFTER_100NS = 1_000_000`, `HOLD_100NS = 20_000_000`, `JUMP_100NS = 20_000_000`, `SUSPEND_100NS = 10_000_000`.
  - `pub fn pts_of(slot: i64, fps: u32) -> i64 = slot * 10_000_000 / fps` and `pub fn slot_of(rel_100ns: i64, fps: u32) -> i64` (rounded to nearest).

The emit rule, which the tests below pin but do not fully determine:

```text
push(qpc, frame):
  received += 1
  if origin is None: origin = qpc, next = 0; out Started{origin: qpc}
  s = slot_of(qpc - origin)
  if s - next > JUMP slots or qpc < last_frame_qpc - JUMP:      # a new device clock (ticks keep next current)
      out Reset{at: qpc}; origin = qpc; next = 0; pending = last = None; gap closed; s = 0
      out Started{origin: qpc}
  if s < next: late += 1; s = next                              # its slot already went out
  for k in next..s: emit_slot(k)                               # a later frame proves them complete
  if pending is Some((s, _)): superseded += 1
  pending = Some((s, frame)); if gap open: out GapClosed{from: last_frame_qpc, to: qpc}
  last_frame_qpc = qpc

tick(now, suspended):
  if origin is Some and suspended > SUSPEND:                    # the PC slept
      out Reset{at: now}; origin = None; pending = last = None; gap closed; return
  while origin is Some and now > origin + pts_of(next) + EMIT_AFTER: emit_slot(next)   # a late tick catches up

emit_slot(k):                                                  # always advances next to k + 1
  if pending is (k, f): out Frame{pts_of(k), f, false}; last = f; pending = None
  elif last is Some and origin + pts_of(k) - last_frame_qpc < HOLD: out Frame{pts_of(k), last, true}
  else: if gap not open: out GapOpened{from: last_frame_qpc}; out Filler{pts_of(k)}
  next = k + 1
```

- [ ] **Step 1: Write the failing tests** (`F` = a `u32` frame number; period ≈ 333 333; every `tick` without a sleep passes `suspended = 0`):
  - `every_session_starts_with_its_origin`: the first push at 5_000 → `out[0] == Started { origin_100ns: 5_000 }`; after a sleep tick, the next push at `q` → `Started { origin_100ns: q }`.
  - `frames_on_time_go_out_one_per_slot`: 30 frames at `k * 333_333` → after the 30th, 29 `Frame`s with pts `0, 333_333, 666_666, 1_000_000, …`, none a duplicate.
  - `two_frames_in_one_slot_keep_the_newest`: frames at 0, 400_000, 450_000, then 700_000 → slot 1 carries the frame of 450_000; `superseded == 1`.
  - `half_the_frames_shows_each_one_twice`: 15 frames at `k * 666_667` → 28 slots out, every second one a duplicate, `duplicates == 14`, pts consecutive.
  - `a_slot_goes_out_100_ms_after_its_time`: a frame at 0; `tick(1_000_000)` emits nothing, `tick(1_000_001)` emits slot 0; `tick(333_333 + 1_000_001)` emits slot 1 as a duplicate.
  - `a_dropout_of_two_seconds_turns_into_fillers_and_a_gap`: a frame at 0, ticks every 333_333 up to 30_000_000, a frame at 30_000_000 → slots 1–59 duplicates, slots 60–89 fillers, exactly one `GapOpened{from: 0}` and one `GapClosed{from: 0, to: 30_000_000}`.
  - `the_stall_timer_keeps_the_pts_continuous`: a 5 s dropout with ticks → every emitted `pts` is `pts_of(k)` for consecutive `k`.
  - `a_jump_past_two_seconds_starts_over`: frames to slot 10, then a frame stamped +600 s without ticks (a new device clock) → `Reset` then `Started { origin_100ns: that stamp }`, the next emitted pts is 0, `resets == 1`.
  - `a_tick_after_a_sleep_starts_over_instead_of_catching_up`: frames to slot 10, then `tick(now + 600 s, 600 s)` → `out == [Reset { .. }]`, no fillers.
  - `a_late_tick_without_a_sleep_only_catches_up`: frames to slot 10, then `tick(now + 5 s, 0)` (the worker was busy) → no `Reset`, slots 11 up to `now + 4.9 s` go out as duplicates and then fillers, pts consecutive.
  - `a_late_frame_counts_for_the_next_slot`: slot 3 emitted by the stall timer, then a frame stamped in slot 3 → pending for slot 4, `late == 1`.
  - `jitter_of_five_ms_costs_no_duplicates`: 60 frames at `k * 333_333 ± 50_000` → no duplicates.
  - `a_clock_that_runs_backwards_starts_over`: a frame 3 s earlier than the last → `Reset`.
- [ ] **Step 2: Run** `cargo test --lib grid` — Expected: FAIL to compile.
- [ ] **Step 3: Implement** the rule above.
- [ ] **Step 4: Run** `cargo test --lib` — Expected: PASS.
- [ ] **Step 5: Commit** `Camera grid: webcam frames land on a 30 fps grid anchored on their own timestamps — the newest frame in a slot wins, an empty slot repeats the last, a slot goes out once a later frame came or 100 ms after its time, from two seconds without frames the grid sends black fillers and records the gap, a sleep or a jump in the camera's clock starts a new session while a tick that is merely late only catches up; the stream that leaves is exact constant frame rate, so a raw elementary stream with -r 30 stays right for it`

---

### Task 9: The camera on Windows, in sharing mode

**Files:**
- Create: `src-tauri/src/camera/windows.rs`
- Modify: `src-tauri/Cargo.toml:64-99` (features `Win32_Media_KernelStreaming`, `Win32_Devices_DeviceAndDriverInstallation`)
- Create: `src-tauri/examples/cam-source.rs` (a probe like `trim-probe.rs`: opens a camera through `camera::open` for 20 s and prints frames per second, the winning `ClockKind`, the size and every `Event` other than a frame)

**Interfaces:**
- Consumes: Task 7's `Source`, `Sink`, `Event`, `Frame`, `ProbeFrame`, `choose_stamp`, `classify`, `OpenMode`; `crate::mft::startup()`; `crate::audio::capture::now_100ns()`.
- Produces: `pub fn list() -> Vec<CameraDevice>`, `pub fn open(device, mode, sink) -> Result<Box<dyn Source>, Error>`, `pub fn holder(device: &CameraDevice) -> Option<String>` (the process `IMFSensorActivityMonitor` reports for it; blocks up to 500 ms, so only the cam helper thread of Task 11 calls it), `pub fn watch_arrivals(on_change: impl Fn() + Send + Sync + 'static) -> Option<ArrivalWatch>` (`CM_Register_Notification` on `KSCATEGORY_VIDEO_CAMERA`, 500 ms debounce, unregistered on drop).

- [ ] **Step 1: Implement `list`**: `CoInitializeEx(None, COINIT_MULTITHREADED)` first, as `audio/devices.rs:31-35` does — `list_cameras` runs on a tokio worker — and `mft::startup()`; then the list — port it from the stage-1 probe (`scratch/cam-probe`) if it was kept on the test machine (it is never committed); otherwise implement it from stage 1 Task 2 step 4's description: `MFEnumDeviceSources` with `MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID`, each `IMFActivate` taken with `ptr::read`, friendly name and symbolic link via `GetAllocatedString` + `CoTaskMemFree`, then `CoTaskMemFree` of the array.
- [ ] **Step 2: Implement `open` for `MfSensorShared`**: on the calling thread (the cam helper of Task 11, already MTA; it takes about a second): `MFCreateSensorGroup(link)` → `IMFSensorGroup::GetSensorDevice(0)` → `IMFSensorDevice::SetSensorDeviceMode(MFSensorDeviceMode_Shared)` → `IMFSensorGroup::CreateMediaSource` → `MFCreateSourceReaderFromMediaSource` with `MF_SOURCE_READER_ASYNC_CALLBACK` (an `#[implement(IMFSourceReaderCallback)]`) and `MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING = 1`, no D3D manager; `SetCurrentMediaType` with a partial NV12 type, so MJPEG and YUY2 both arrive as NV12 in system memory. `OnReadSample`: arrival = `now_100ns()`; the three stamps (`MFSampleExtension_DeviceTimestamp`, `MFSampleExtension_DeviceReferenceSystemTime`, `GetSampleTime`) into `choose_stamp`; `IMF2DBuffer::Lock2D` and a copy into a tight NV12 `Arc<[u8]>`; release the sample at once; re-issue `ReadSample`. `MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED` → `Event::FormatChanged`; `MF_SOURCE_READERF_ERROR` or a failed `hrStatus` → `Event::Lost(classify(hr))`. The callback re-issues `ReadSample` through an `Option<IMFSourceReader>` behind a `Mutex` that it does not own; `close`: stop flag, `Flush`, wait for `OnFlush` at most 1 s, **take the reader out of that `Option`** (the callback and the reader hold each other, and without breaking that cycle the source is never shut down and the LED stays on), drop it, which shuts the source down. Every COM pointer that crosses a thread goes through `gpu::SendPtr`.
- [ ] **Step 3: Implement `holder` and `watch_arrivals`.** `holder`: `MFCreateSensorActivityMonitor` with an `#[implement(IMFSensorActivitiesReportCallback)]` that sends the first report over a channel; `Start`, wait at most 500 ms, `Stop`; from the report for this symbolic link take the first process id that is not `std::process::id()` and turn it into a name with `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `QueryFullProcessImageNameW` (file stem, so "Discord"). `MfShareAttr` and `WinrtShared` return `Err(Error::Unsupported(..))` unless the stage-1 result picked one of them (note below).
- [ ] **Step 4: Verify** `cargo test --lib` and `cargo build --example cam-source` (PowerShell) — Expected: both pass.
- [ ] **Step 5: Verify on the Windows machine**, for the C920 and the C960 in turn: `cargo run --example cam-source -- 0` → about 30 fps in bright light at the camera's current format, `ClockKind` as stage 1 recorded it, no `Lost`. Start a Discord test video meanwhile → Discord's picture works, the probe keeps running (or behaves exactly as the stage-1 result says for this API). Unplug the camera → `Lost(Gone)` within a second. Camera privacy off for desktop apps, run again → the open fails with `Denied`.
- [ ] **Step 6: Commit** `Camera on Windows: listed through Media Foundation, opened as a shared sensor group so it never takes the camera away from Discord, frames decoded to NV12 in system memory with the QPC timestamp the stage-1 test settled on, unplugging and a privacy block reported by name, the app holding the camera named through the sensor activity monitor, and a cam-source probe for trying it on a machine`

**If stage 1 picked WinRT `SharedReadOnly`:** only `camera/windows.rs` and `Cargo.toml` change. `open(WinrtShared)` becomes stage 1's `MediaCapture` path (`SharingMode = SharedReadOnly`, `MemoryPreference = Cpu`, `InitializeWithSettingsAsync`, `CreateFrameReaderWithSubtypeAndSizeAsync(color, "NV12", size)`, `AcquisitionMode = Buffered` before `StartAsync`, `FrameArrived` → `SoftwareBitmap` → NV12 copy, `MediaCapture.Failed` → `Event::Lost(..)`), `SystemRelativeTime` goes into `choose_stamp`'s `device` slot (it can be null), `FormatChanged` maps to `Event::FormatChanged`, `CaptureDeviceExclusiveControlStatusChanged` to `Event::Lost(Busy)`; the features are stage 1's WinRT list (`Foundation`, `Foundation_Collections`, `Devices_Enumeration`, `Media_Capture`, `Media_Capture_Frames`, `Media_MediaProperties`, `Media_Devices`, `Graphics_Imaging`); a group is matched to a device through `SourceInfos[i].DeviceInformation().Id()`, case-insensitively. If it picked `MfShareAttr` (Windows 11 26100+ only): `MFCreateDeviceSource` with the GUID constant copied from `mfidl.h` as in stage 1, and `MfSensorShared` stays the fallback below 26100. If Fabi chose Open for Fabi 11 (a) and stage 1's `prime` kept 720p30, add `prime` as stage 1 Task 3 Step 6 did, only when `holder()` reports nobody. Nothing outside `windows.rs` knows which way was taken.

---

### Task 10: The cam track's rules: when it runs, its ring, its taps, its preview frames

**Files:**
- Create: `src-tauri/src/camtrack.rs` (the platform-neutral half; Task 11 adds `#[cfg(windows)] mod win`)
- Modify: `src-tauri/src/lib.rs:1-40` (`pub mod camtrack;`)
- Modify: `src-tauri/src/config.rs:94-110` (cam ring formulas beside `effective_memory_bytes`)
- Modify: `src-tauri/src/state.rs:143-321` (`AutoBuffer::screen_locked`), tests at 1320
- Test: `src-tauri/src/camtrack.rs`, `src-tauri/src/config.rs`, `src-tauri/src/state.rs`

**Interfaces:**
- Produces:
  - `pub struct CamWants { pub enabled: bool, pub in_clips: bool, pub buffer_wanted: bool, pub preview_open: bool, pub locked: bool }`; `pub fn cam_wanted(w: &CamWants) -> bool` = `enabled && !locked && ((buffer_wanted && in_clips) || preview_open)`; `pub fn stop_delay(before: &CamWants, after: &CamWants) -> Option<Duration>` — `None` while still wanted; `Some(ZERO)` when the master switch, *in clips*, the lock or the end of the preview lease took it away; `Some(3 s)` when only the buffer went away, so a pipeline restart (stop + start) does not touch the camera (spec 2.2). `AppState` keeps the last `CamWants` to compare against (Task 11).
  - `AutoBuffer::screen_locked(&self) -> bool` — the same 10 s count as `poll_lock`, true also when no buffer is running (a recording keeps the pipeline).
  - `config::cam_ring_seconds(buffer_seconds: u32, keyframe_seconds: u32) -> u32` (`b + k + 12`, i.e. 1 + 0.3 + 10 rounded up), `config::cam_bitrate_kbps(res: CamResolution) -> u32` (3000 / 6000), `config::effective_cam_memory_bytes(ring_seconds: u32, bitrate_kbps: u32) -> u64` (floor 16 MiB).
  - `pub struct Gap { pub from_100ns: i64, pub to_100ns: Option<i64> }` (open while `to` is `None`).
  - `pub struct CamRing { pub ring: ReplayBuffer, pub origin_100ns: i64, pub session: u64, pub header: Vec<u8>, pub gaps: VecDeque<Gap>, tap: Option<Arc<dyn PacketTap>> }` with `push(&mut self, session: u64, packet: EncodedPacket) -> bool` (false for an old session), `set_header(&mut self, session: u64, header: Vec<u8>)` (the cam encoder's SPS/PPS: its `sequence_header` at start, then its `on_header`, both session-checked like `push`), `reset(&mut self, origin_100ns: i64)` (on the grid's `Started`: clear ring, gaps and header, `session += 1`, the origin from `Started`), `open_gap`/`close_gap`, `attach_tap(&mut self, tap, from_100ns: i64) -> Vec<EncodedPacket>` (backlog from the cam keyframe at or before `from_100ns`), `detach_tap(&mut self)`. Gaps that end before the ring's front leave with it. The ring gets `set_jump_limit_us(2_000_000)`.
  - `pub trait PacketTap: Send + Sync { fn cam(&self, packet: &EncodedPacket); }` — the stage-4 recorder's hook, called while the ring lock is held, like `Recorder::video`.
  - `pub struct PreviewFrame { pub seq: u64, pub width: u32, pub height: u32, pub bgra: Arc<[u8]> }`; `pub struct PreviewSlot` with `offer(&self, frame: &camera::Frame, crop: Rect, now: Instant)` (at most every 1/15 s, only while someone asked in the last 2 s) and `wait_after(&self, after: u64, limit: Duration) -> Option<PreviewFrame>` (a `Mutex` + `Condvar`).
  - `pub fn preview_bgra(frame: &camera::Frame, crop: Rect, width: u32) -> (u32, u32, Vec<u8>)` — point-sampled, BT.601, 320 px wide; `pub fn encode_preview(frame: &PreviewFrame) -> Vec<u8>` — `seq` u64 LE, then width and height as u32 LE, then the pixels.

- [ ] **Step 1: Write the failing tests:**
  - `the_cam_runs_only_while_something_the_user_switched_on_needs_it`: a table over `cam_wanted` — master off → false in every row; buffer + clips → true; buffer without clips → false; preview alone → true; locked with buffer or preview → false.
  - `a_buffer_restart_does_not_blink_the_camera`: buffer true → false, rest unchanged → `Some(3 s)`; `cam_off_closes_at_once`: enabled true → false → `Some(Duration::ZERO)`; `the_lock_closes_at_once`; `a_closed_preview_lets_go_at_once`: buffer off, `preview_open` true → false → `Some(Duration::ZERO)`.
  - `the_cam_lock_follows_the_buffers_ten_seconds` (state.rs, style of 1376-1473): four `poll_lock(true, false)` → `!screen_locked()`, the fifth → `screen_locked()`, `poll_lock(false, false)` → `!screen_locked()`.
  - `the_cam_ring_costs_about_72_mib_for_the_default_buffer`: `cam_ring_seconds(120, 2) == 134`, `effective_cam_memory_bytes(134, 3000) == 75_375_000` (about 72 MiB, 75,375,000 bytes); `a_tiny_cam_ring_keeps_its_floor`: `effective_cam_memory_bytes(10, 3000) == 16 * 1024 * 1024`.
  - `a_packet_from_an_old_session_is_dropped`: `reset` then `push(old_session, p) == false` and `set_header(old_session, ..)` leaves the header empty.
  - `gaps_leave_with_the_ring`: a gap that ends before the front keyframe is gone after the next trim.
  - `a_tap_sees_its_backlog_and_every_packet_after_it`: 3 s of cam packets (1 s GOP) → `attach_tap(tap, qpc of 1.5 s)` returns packets from 1.0 s; two more `push`es reach the tap; after `detach_tap` none.
  - `a_grey_frame_previews_grey`: a 4×2 NV12 frame of Y=126, U=V=128 → every BGRA pixel has B, G and R within 2 of 128 and A = 255.
  - `the_preview_frame_starts_with_its_sequence_number`: `encode_preview` of seq 7, 2×1 → first 8 bytes `7u64.to_le_bytes()`, then `2u32`, `1u32`, then 8 bytes of pixels.
  - `nobody_asking_means_no_preview_work`: `offer` without a `wait_after` in the last 2 s stores nothing.
- [ ] **Step 2: Run** `cargo test --lib camtrack config state` — Expected: FAIL to compile.
- [ ] **Step 3: Implement** as specified.
- [ ] **Step 4: Run** `cargo test --lib` — Expected: PASS.
- [ ] **Step 5: Commit** `Cam track rules: it runs only while the buffer wants clips with the cam or the settings show its preview, never behind a lock screen held for ten seconds, and a buffer restart does not switch the camera off and on; its own ring sized for the clip plus ten seconds of margin at 1.5 × 3 Mbit/s, packets of an old session never reach it, a tap for recordings to come, and small preview frames made only while somebody looks`

---

### Task 11: The cam track running: worker, encoder, ring, diagnostics

**Files:**
- Modify: `src-tauri/src/camtrack.rs` (`pub struct CamTrack`, `#[cfg(windows)] mod win` with the worker, the helper and the scaler, and a `#[cfg(not(windows))]` `CamTrack` whose `apply` does nothing and whose `status` is `None`, so `cargo check` from WSL compiles)
- Modify: `src-tauri/src/mft.rs:136-146` (`memory: FrameMemory`, `bframes: Option<u32>`, `rate: RateMode`), `318-381` (`configure_rate_control`: `RateMode::PeakVbr` goes straight to peak-constrained VBR at `bitrate_kbps`, peak 1.5 ×; `RateMode::Quality` is today's order), `392-401` (`Some(n)` sets the B-frame count regardless of the environment), `681-758` (`FrameMemory::System` picks the vendor-less candidate and skips the D3D manager), `853-857` (`Job` carries `JobFrame::Texture | JobFrame::Bytes`), `948-978` (`feed` for bytes: `MFCreateMemoryBuffer` + copy + `SetCurrentLength`), `1163-1201` (`submit_bytes`)
- Modify: `src-tauri/src/pipeline.rs:655-665` (`memory: Gpu, bframes: None, rate: RateMode::Quality`: the game is unchanged)
- Modify: `src-tauri/Cargo.toml:64-99` (`Win32_System_WindowsProgramming` for `QueryUnbiasedInterruptTime`; the compiler confirms the module)
- Modify: `src-tauri/src/state.rs:18-106`, `334-350` (`cam: None` in the `EngineStatus` literal), `369-393` (`pub camera: CamTrack`, `cam_preview_until: AtomicI64`, `cam_wants: Mutex<Option<CamWants>>`), `419-457` (`reconcile_cam` after the swap), `512-620` (after every change of `buffer_wanted`), `762-812` (`status.cam`), new `cam_plan`/`reconcile_cam`
- Modify: `src-tauri/src/lib.rs:665-700` (`reconcile_cam` right after `poll_lock` at 675, before the early returns at 676 and 678-680), `1067-1080` (poll `state.camera.take_unseen_error()`: `notify` + `overlay::show(.., BannerKind::Error, "Facecam stopped", Some(err))`), `1091-1104` (cam health line)
- Modify: `src-tauri/src/model.rs:839-883` (`#[serde(default)] pub cam: Option<CamStatus>`), new `CamStatus`, `CamState`; `src/lib/types.ts:480` (mirrors)

**Interfaces:**
- Consumes: Tasks 1, 3, 7–10.
- Produces:
  - `pub struct CamPlan { pub run: bool, pub stop_after: Duration, pub device_id: Option<String>, pub device_name: Option<String>, pub resolution: CamResolution, pub fps: u32, pub ring_seconds: u32, pub encoder: EncoderId, pub preview: bool }`.
  - `CamTrack::new() -> Self` (no thread), `apply(&self, plan: CamPlan)` (never blocks: spawns the `clippiboy-cam` worker on the first run and hands later plans over a channel; the sender lives in a `Mutex<Option<Sender>>` that `apply` holds while it sends or spawns, and the worker ends only by taking that same lock, finding the channel empty and setting it to `None` — so a plan sent while the worker is on its way out starts a new one instead of being lost), `latest_frame(&self) -> Option<Arc<camera::Frame>>` (the raw-frame tap with its QPC, spec 2.10; `None` while the cam is stopped — stage 4's screenshots rely on it), `preview(&self) -> &PreviewSlot`, `ring(&self) -> &Mutex<CamRing>`, `status(&self) -> Option<CamStatus>`, `health_line(&self) -> Option<String>`, `take_unseen_error(&self) -> Option<String>`, `attach_tap`/`detach_tap` (forwarding to the ring).
  - `AppState::cam_plan(&self) -> CamPlan`, `AppState::reconcile_cam(&self)`.
  - `CamStatus { state: CamState, camera: Option<String>, fps_in: f32, fps_out: f32, duplicates: u64, fillers: u64, gaps: u32, clock: Option<ClockKind>, encoder: Option<EncoderId>, software: bool, ring_seconds: f32, ring_bytes: u64 }`; `#[serde(tag = "kind", rename_all = "camelCase")] enum CamState { Starting, Shared, Taken { by: Option<String> }, Blocked, NoCamera, Failed { message: String } }`.
  - `VideoEncoder::submit_bytes(&self, nv12: Arc<[u8]>, pts_100ns: i64) -> bool`; `pub enum FrameMemory { Gpu, System }`; `pub enum RateMode { Quality, PeakVbr }`.
  - `fn suspended_since(&mut self) -> i64` in the worker: `(QPC now − QPC last) − (unbiased interrupt time now − last)`, both in 100 ns, fed into `grid.tick`.

- [ ] **Step 1: Two threads** (`mod win`), both `CoInitializeEx` MTA and `mft::startup()`. The **worker** (`clippiboy-cam`) does only what must keep time: tick the grid, scale, submit, push. The **helper** (`clippiboy-cam-ctl`) does everything that blocks — `camera::open` (about 1 s), `Source::close` (up to 1 s), `holder()` (up to 500 ms), `VideoEncoder::start` (up to 10 s) and the stop and drop of an old encoder (outside the ring lock) — and posts the results back to the worker as messages (`Opened(Box<dyn Source>)`, `EncoderReady { session, encoder }`, `Holder(Option<String>)`). Frames the grid emits while no encoder of the current session is ready are dropped; the encoder's first packet then carries a pts greater than 0, which the offset in Task 12 accounts for. The worker's own device: `GpuDevice::new(Some(encoder))` on the encoder's adapter; the scaler: a staging NV12 texture at the camera's size (Map, Y plane, then UV at `RowPitch × height`), `CopyResource` into a default texture, one `VideoProcessorBlt` into a pool of `mft::QUEUE_DEPTH + mft::ENCODER_HOLD_MAX + 2` NV12 textures at the track size, source rect `cover_crop`, input colour from `colour_of`, output BT.709 studio, `VideoProcessorSetStreamAutoProcessingMode(.., false)` (CAP-V1); one black NV12 texture (Y 16, UV 128) for fillers. A re-sent frame re-submits the slot it went out in. `Event::FormatChanged` rebuilds only the staging and input textures and the processor's input side; the encoder keeps its size (spec 2.8).
- [ ] **Step 2: The loop**: `crossbeam_channel::select!` over source events, plans, helper messages and a 10 ms timeout; on every turn first `grid.tick(now_100ns(), suspended_since())`, then the event. Every `Out` goes on: `Started { origin }` → `ring.reset(origin)` and an `EncoderStart` request to the helper for the new session; `Frame` → scaled slot → `submit`; `Filler` → black → `submit`; `Reset` → the current encoder to the helper to be stopped; gap events to the ring; each raw frame to `latest_frame` and `preview.offer`. The encoder: `VideoEncoder::start(gpu, EncoderSettings { width, height, fps: 30, bitrate_kbps: cam_bitrate_kbps(res), quality: 70, keyframe_seconds: 1, requested: encoder, label: "cam", memory: Gpu, bframes: Some(0), rate: RateMode::PeakVbr }, …)` — peak-constrained VBR, because constant quality (the order `configure_rate_control` tries first, `mft.rs:318-381`) would ignore the bitrate the ring's memory is budgeted for; its `on_packet` locks only the cam ring and pushes with its session, its header goes through `set_header`. If the hardware session is refused, the Microsoft software MFT with `FrameMemory::System` (slot read back through a staging texture, `submit_bytes`); if that fails too, `CamState::Failed` with the error in the slot and no ring. `CLIPPIBOY_CAM_SOFTWARE=1` forces the software encoder (the `env_bool` pattern of `mft.rs:83-90`).
- [ ] **Step 3: The camera's life** (all of it on the helper): open via `camera::open(pick_device(..), OpenMode::MfSensorShared, sink)`, the sink forwarding into the worker's channel; `Lost(Gone)` or a failed open → fillers continue, `Backoff`, immediate retry on `watch_arrivals`; `Busy` → `Taken { by: holder() }`, retried; `Denied` → `Blocked`, retried every 30 s (decided here: the user fixes it in Windows Settings and expects the cam back); no frame for 5 s while open → `Taken { by: holder() }`, fillers, the source kept open. Stop: close the camera first (LED off) and set `latest_frame` to `None`, then the encoder, then the ring.
- [ ] **Step 4: AppState wiring**: `cam_plan` from `config_snapshot().facecam`, `buffer_wanted`, `cam_preview_until > now`, `auto.screen_locked()`, `encode::for_recording(config.recording.encoder)`, `cam_ring_seconds(config.buffer.seconds, config.recording.keyframe_seconds)` and `stop_delay` against the `CamWants` kept in `cam_wants` (which it then replaces); `reconcile_cam` is called at the places listed under Files. `AppState::new` puts `cam: None` into the status literal. The health line every 5 s: `cam: 29.8 fps in, 30.0 out, dup=412 filler=0 gaps=0 clock=device encoder=Nvenc ring 133.2 s 48.1 MB`.
- [ ] **Step 5: Verify** `cargo test --lib` and `npx tsc --noEmit` — Expected: PASS.
- [ ] **Step 6: Verify on the Windows machine** (buffer on, master switch on via the config file, `inClips` on):
  - the LED is on; every 5 s a `cam:` line with about 30 fps in bright light, about 15 in and 30 out with `dup` climbing in a dim room;
  - master switch off → the LED goes off within a second, no `cam:` line, no `clippiboy-cam` thread (Process Explorer);
  - change the capture source → the buffer restarts, the LED stays on;
  - unplug → after 2 s `filler` climbs, `gaps=1`; plug back → frames within a few seconds;
  - lock the screen for 15 s → the LED goes off after about 10 s and comes back after the sign-in;
  - sleep the PC for a minute and wake it without a lock screen → one `Reset` in the log, the ring starts over, the `cam:` line comes back; a busy moment without sleep (Task Manager open on a full CPU) → no `Reset`;
  - the encoder profile log for the cam names peak-constrained VBR at 3000 kbit/s and 0 B-frames;
  - `CLIPPIBOY_CAM_SOFTWARE=1` → the encoder profile log names the Microsoft software MFT, the `cam:` line still shows 30 out;
  - Discord's test video keeps running through all of it (or behaves as the stage-1 result says).
- [ ] **Step 7: Commit** `Cam track: the webcam is buffered as its own 720p30 H.264 track — its own graphics device so it never queues behind the game, a grid so dim light and USB hiccups cannot shift it, a second encoder with one-second groups and no B-frames that falls back to the Windows software encoder, a ring of its own, and a line in the five-second health log; it lives beside the game pipeline, so a restart of the buffer does not blink the camera, and it stops behind a lock screen`

---

### Task 12: Cutting the cam to the clip window, and the facts that go with it

**Files:**
- Create: `src-tauri/src/facecam.rs` (store paths, facts, the cut)
- Modify: `src-tauri/src/lib.rs:1-40` (`pub mod facecam;`)
- Test: `src-tauri/src/facecam.rs`

**Interfaces:**
- Consumes: `CamRing` (its `origin_100ns` is the grid's `Started` origin), `Gap` (Task 10), `ReplayBuffer::snapshot_range` (Task 2), `ClockKind`, `CamLayout`.
- Produces:
  - `pub struct CamWindow { pub g0_100ns: i64, pub length_us: u64 }`; `pub fn cut_window(w: &CamWindow) -> (i64, i64)` = `(g0 − 3_000_000, g0 + length·10 + 3_000_000)`.
  - `pub struct CamCut { pub packets: Vec<EncodedPacket>, pub offset_us: i64, pub frames: u32, pub gaps_ms: Vec<(u64, u64)> }`; `pub fn cam_cut(ring: &CamRing, window: &CamWindow) -> Option<CamCut>` — `None` when no cam frame lies in `[G0, G0 + L)` or gaps cover all of it; a cut never spans a hole in the pts (it then starts on the first keyframe after the last hole).
  - `pub fn saved_delay_ms(config_delay: i32, clock: ClockKind) -> i32` — another −40 on `Arrival`, clamped to −300…300.
  - `#[serde(rename_all = "camelCase")] pub struct FacecamFacts { version: u32 /*1*/, width: u32, height: u32, fps: u32, cam_offset_us: i64, frames: u32, clip_length_us: u64, gaps_ms: Vec<(u64, u64)>, clock: ClockKind, camera: String, encoder: Option<EncoderId>, game_gop: u32, #[serde(default)] video_start_us: Option<i64>, #[serde(default)] applied: Option<FacecamApplied>, #[serde(default)] applying: Option<FacecamApplied>, #[serde(default)] swap_failing_since_ms: Option<i64> }`; `pub struct FacecamApplied { layout: CamLayout, delay_ms: i32, start_ms: u64, length_ms: u64, size_bytes: u64 }` — what the clip file shows; `applying` is the intent written just before a swap, `game_gop` the game's keyframe interval in frames at the save (Task 14), `swap_failing_since_ms` the Unix time of the first swap that failed (all Task 16).
  - `pub fn cam_path(id) -> PathBuf` (`edit::dir(id)/cam.mp4`), `pub fn facts_path(id) -> PathBuf` (`…/facecam.json`), `pub fn has_store(id: &str) -> bool` (facts file present), `pub fn read_facts(id) -> Option<FacecamFacts>` (`None` without `cam.mp4`), `pub fn write_facts(id, &FacecamFacts) -> std::io::Result<()>` (`.part` + rename, like `stems::write_index`, 85-104).
  - Stage 4 extends these for recordings' segment stores (`cam.<n>.mp4` in place of `cam.mp4`), together with the worker's step 1 and `recover` (Task 16).

- [ ] **Step 1: Write the failing tests** (a `CamRing` filled at 30 fps with a keyframe every 30 frames, origin 0 unless said):
  - `the_cut_starts_on_the_keyframe_before_the_delay_range`: G0 = 10.5 s, L = 5 s → first packet a keyframe at 10.0 s, last pts ≤ 15.8 s, `offset_us == -500_000`.
  - `the_offset_is_measured_against_the_games_first_frame`: origin 5_000_000_000, G0 = origin + 10.5 s → `offset_us == -500_000`.
  - `a_cam_that_started_after_the_clip_gives_a_positive_offset`: ring from 12 s, G0 = 10.5 s → `offset_us == 1_500_000`.
  - `no_cam_frame_in_the_window_means_no_facecam`: ring ends at 9 s, G0 = 10 s → `None`; ring starts after `G0 + L` → `None`.
  - `a_window_covered_by_a_gap_has_no_facecam`: one gap over all of `[G0, G0 + L)` → `None`.
  - `a_gap_is_carried_in_clip_coordinates`: gap [12 s, 13.5 s) with G0 = 10.5 s → `(1500, 3000)`; one starting before G0 → from 0; an open one → to L.
  - `a_cut_never_spans_a_hole_in_the_pts`: G0 = 4 s, L = 4 s, the packet of slot 150 (5 s) missing → the cut starts on the keyframe at 6 s, `offset_us == 2_000_000`.
  - `steps_of_33333_and_33334_us_are_no_hole`.
  - `the_arrival_clock_shifts_the_cam_another_40_ms_earlier`: `(−40, Device) → −40`, `(−40, Arrival) → −80`, `(−300, Arrival) → −300`.
  - `facts_survive_the_roundtrip` and `a_store_without_cam_mp4_does_not_count` (real data dir under a test id, cleaned up after, as the `stems.rs` tests do).
- [ ] **Step 2: Run** `cargo test --lib facecam` — Expected: FAIL to compile.
- [ ] **Step 3: Implement** as specified.
- [ ] **Step 4: Run** `cargo test --lib` — Expected: PASS.
- [ ] **Step 5: Commit** `Facecam cut: the cam is cut by the game's absolute window plus 300 ms either side, from the cam keyframe before it, its offset to the game's first frame measured on the one clock, gaps carried in clip coordinates, never across a hole in its timestamps — and facecam.json with the facts that go with it`

---

### Task 13: The `facecam` column in the clip library

**Files:**
- Modify: `src-tauri/src/model.rs:655-713` (`#[serde(default)] pub facecam: Option<ClipFacecam>` on `Clip`), new `ClipFacecam`, `FacecamState`
- Modify: `src-tauri/src/clips.rs:56-71` (`add_column("facecam", "TEXT")`), `89-131` (`facecam` as the 17th column of `INSERT OR REPLACE`), `133-164` (read it), new `set_facecam` beside `set_original` (306-312), `encode_facecam`/`decode_facecam` beside 370-405
- Modify: every `Clip { … }` literal — `state.rs:916-938`, `998-1019`, `1180-1199`; `share.rs:872-897`; `filing.rs:329-349`, `clips.rs:420-441`, `edit.rs:1070-1092` (tests); `examples/trim-probe.rs:52`, `examples/record-probe.rs:1024`, `examples/tracks-probe.rs:25` — with `facecam: None`
- Modify: `src/lib/types.ts:350-400` (`ClipFacecam`, `FacecamState`, `Clip.facecam`), `src/lib/mock.ts:156-285` (one clip `pending`, one `ready`)

**Interfaces:**
- Produces: `ClipFacecam { #[serde(default)] layout: CamLayout, #[serde(default)] portrait: PortraitLayout, delay_ms: i32, #[serde(default)] state: FacecamState }`; `enum FacecamState { #[default] Pending, Ready, Failed, Baked }` (`"pending" | "ready" | "failed" | "baked"`); `Library::set_facecam(&self, id: &str, facecam: Option<&ClipFacecam>) -> rusqlite::Result<()>`.

- [ ] **Step 1: Write the failing tests** (clips.rs, `Library::in_memory`):
  - `the_facecam_survives_the_roundtrip`: insert, `set_facecam(Some(..))`, `get` → equal; `set_facecam(None)` → `None`.
  - `the_facecam_column_survives_a_reinsert`: insert a clip carrying a facecam, `get`, insert what came back → still equal (the `INSERT OR REPLACE` list).
  - `a_broken_facecam_entry_is_dropped_not_fatal`: `decode_facecam(Some("{broken".into())).is_none()`.
- [ ] **Step 2: Run** `cargo test --lib clips` — Expected: FAIL to compile.
- [ ] **Step 3: Implement**; the column is its own, not a `ClipEdit` field (spec 2.7, Open for Fabi 10).
- [ ] **Step 4: Run** `cargo test --lib` and `npx tsc --noEmit` — Expected: PASS.
- [ ] **Step 5: Commit** `Clip library: a facecam column of its own with the clip's layout, delay and state — not a field of the edit record, which three places rebuild from scratch, and in insert's column list so a re-inserted clip keeps it`

---

### Task 14: Saving, phase 1 — the clip as today, then `cam.mp4` and `facecam.json`

**Files:**
- Modify: `src-tauri/src/pipeline.rs:564-580`, `1068-1076` (`ClipSnapshot.span_us: u64` = `timing.span_us`)
- Modify: `src-tauri/src/camtrack.rs` (`CamTrack::cut`)
- Modify: `src-tauri/src/facecam.rs` (`cam_mp4_arguments`, `write_phase_one`)
- Modify: `src-tauri/src/state.rs:863-939` (`save_clip` also returns the `CamWindow`)
- Modify: `src-tauri/src/lib.rs:105-149` (`save_clip_and_notify`: the cam work after the banner), `src-tauri/src/commands.rs:404-416` (the same for `save_clip` with seconds)

**Interfaces:**
- Consumes: Tasks 10–13.
- Produces:
  - `CamTrack::cut(&self, window: &CamWindow, wait: Duration) -> Option<CamCutOut>` — polls `newest` against the window's end every 20 ms up to `wait` with no lock held between polls, then cuts under the cam ring lock; `CamCutOut { cut: CamCut, header: Vec<u8> /*the ring's `header`, Task 10*/, width, height, fps, clock: ClockKind, camera: String, encoder: Option<EncoderId> }`.
  - `pub fn cam_mp4_arguments(h264: &Path, output: &Path) -> Vec<String>` = `-y -hide_banner -loglevel error -f h264 -r 30 -i <h264> -c:v copy -an -movflags +faststart <output>`.
  - `pub fn write_phase_one(clip_id: &str, out: &CamCutOut, clip_length_us: u64, game_gop: u32) -> Result<FacecamFacts, String>` — `cam.part.h264` (header + packets) → ffmpeg → `cam.part.mp4` → probe → rename to `cam.mp4` → `facecam.json` last; any failure removes the partial files and returns `Err`.

- [ ] **Step 1: Write the failing test** `the_cam_file_is_copied_at_exactly_thirty_frames`: `["-f", "h264", "-r", "30"]` stand right before the first `-i`, `["-c:v", "copy"]` is present, there is no `-filter_complex` and no `-ss`.
- [ ] **Step 2: Run** `cargo test --lib facecam` — Expected: FAIL to compile.
- [ ] **Step 3: Implement.** `save_clip` (863-939) returns the clip together with `Option<(CamWindow, u32)>` — `start_100ns` and `span_us` copied out of the snapshot before it moves into the request, and the game's GOP `recording.fps × recording.keyframe_seconds` of the fitted settings it already reads from `active_recording` for the clip's size (894-900); `Some` only when `facecam.enabled && facecam.in_clips`. `write_phase_one` stores that GOP in the facts as `game_gop`. The cam work happens **after** today's row insert, banner, sound and `clip-saved` in `save_clip_and_notify` (`lib.rs:105-149`), on the same thread and still inside `begin_save` (taken at `lib.rs:110`), so the "Clip saved" banner comes exactly as fast as today — and in `commands::save_clip`'s seconds path (`commands.rs:411-415`), which has no guard, no banner and no `clip-saved` (review SAVE-9), right after its `lib.insert` (413): `self.camera.cut(&window, 500 ms)` (the cam ring's 10 s margin covers the banner's few milliseconds), then `write_phase_one`; on `Ok(facts)` `Library::set_facecam(id, Some(&ClipFacecam { layout, portrait, delay_ms: saved_delay_ms(config delay, facts.clock), state: if layout.visible { Pending } else { Ready } }))` and `clips-changed`; on `Err` a log line and no facecam. Task 16 enqueues right after `set_facecam`.
- [ ] **Step 4: Run** `cargo test --lib` — Expected: PASS.
- [ ] **Step 5: Verify on the Windows machine:** save a 30 s clip with the cam on → the clip file has no cam (phase 2 does not exist yet), plays as before, and the "Clip saved" banner comes as fast as before (the log's save line, then the cam lines after it); `originals/<id>/cam.mp4` plays in a player and lasts 30.6–31.6 s; `facecam.json` has `camOffsetUs` between −1 300 000 and −300 000, `clock` as the health line said; the row's `facecam.state` is `pending`. A save with the master switch off writes nothing into `originals/`.
- [ ] **Step 6: Commit** `Saving, first half: the clip is written exactly as before, and only then is the cam cut by the game's window — so the wait for the cam never delays reading the audio rings, which are trimmed while a save runs; a second ffmpeg run copies it into cam.mp4 in the originals store, facecam.json goes last as the sign it is complete, and a broken cam stream costs the cam, never the clip`

---

### Task 15: The composite, as pure argument builders

**Files:**
- Create: `src-tauri/src/composite.rs` (geometry, timing, mask, filter, arguments — the same kind of pure builder as `stems::mix_filter`, `stems.rs:342-372`)
- Modify: `src-tauri/src/lib.rs:1-40` (`pub mod composite;`)
- Modify: `src-tauri/src/edit.rs:768-798` (`pub(crate) enum Speed { Edit, Save }`, `pub(crate) fn video_args_at(encoder, quality, speed) -> Vec<String>`; `video_args` becomes `video_args_at(.., Speed::Edit)`)
- Modify: `src-tauri/src/muxer.rs:529-545` (`probe_video_start_us`, `parse_start_us` beside `probe_duration_ms`; the probe below needs them)
- Create: `src-tauri/examples/composite-probe.rs` (`-- <clip.mp4> <cam.mp4> [--offset-us N] [--encoder nvenc|amf|qsv|x264]`: builds `composite_arguments`, runs ffmpeg, prints the line and the `speed=` ffmpeg reported)

**Interfaces:**
- Consumes: `CamLayout`, `CamShape`, `FacecamFacts`, `camera::Rect`.
- Produces:
  - `pub fn cam_rect(frame: (u32, u32), layout: &CamLayout) -> Rect` — `h = even(size·H)`, `w = even(h·16/9)` (circle: `w = h`), `m = even(0.025·H)`; when `w > W − 2m` (a portrait screen), `w = W − 2m` and `h = even(w·9/16)` (circle: `h = w`); `x = m + even(layout.x·(W − w − 2m))`, `y` likewise; `even(v) = 2·round(v/2)`. The name stage 3 expects.
  - `muxer::probe_video_start_us(path: &Path) -> Option<i64>` (`ffprobe -v error -select_streams v:0 -show_entries stream=start_time -of default=noprint_wrappers=1:nokey=1`) and `muxer::parse_start_us(text: &str) -> Option<i64>`.
  - `pub fn cam_crop(cam: (u32, u32), shape: CamShape) -> Option<Rect>` — the centred square for `Circle`, else `None`.
  - `pub struct CamTiming { pub seek_s: Option<f64>, pub shift_s: f64, pub end_s: f64 }`; `pub fn cam_timing(offset_us: i64, delay_ms: i32, video_start_us: i64, start_ms: u64, length_ms: u64) -> CamTiming` — `d = offset + delay`, `e = start − d`; `e > 0` → `seek = e`, `shift = video_start`; else no seek, `shift = video_start − e`; `end = video_start + length` (`docs/extras/research/ffmpeg-compositing.md` 2.e plus the SAVE-7 start).
  - `pub fn mask_gray8(shape: CamShape, w: u32, h: u32) -> Option<Vec<u8>>` (`None` for `Rect`; anti-aliased, rounded radius 12 % of the height) and `pub fn write_mask_png(path: &Path, w: u32, h: u32, pixels: &[u8]) -> std::io::Result<()>` with the `png` crate.
  - `pub struct CamChain<'a> { pub cam_input: usize, pub mask_input: Option<usize>, pub main: &'a str, pub frame: (u32, u32), pub cam: (u32, u32), pub layout: CamLayout, pub timing: CamTiming, pub gaps_ms: &'a [(u64, u64)], pub video_start_us: i64 }`; `pub fn cam_filter(chain: &CamChain) -> String` — the video graph ending in `[v]`: `[n:v]setpts=PTS+<shift>/TB,trim=end=<end>[,crop=..][,hflip],scale=<w>:<h>,format=yuva420p` (`PTS`, not `PTS-STARTPTS`: with `-ss e` ffmpeg has already rebased the cam input, and the first frame it keeps lies 0–33 ms after `e`; pulling it to 0 would show the cam up to a frame early) (`[m:v]format=gray` + `alphamerge` for a mask), `[<main>][cam]overlay=x=<x>:y=<y>:eval=init:eof_action=repeat[:enable='not(between(t,a,b)+…)']`; gap times shifted by the video start. Stage 3's edit path reuses it with stems in between.
  - `pub struct CompositeJob<'a> { clip, cam, mask: Option<&Path>, output, frame, facts: &FacecamFacts, layout, delay_ms, video_start_us, start_ms, length_ms, gaps_ms, encoder, quality, gop: u32, hwaccel: bool }`; `pub fn composite_arguments(job: &CompositeJob) -> Vec<String>`: `-y -hide_banner -loglevel error -progress pipe:1 -nostats [-hwaccel d3d11va] -threads 4 -i <clip> [-ss <e>] -i <cam> [-i <mask>] -filter_complex_threads 2 -filter_complex <cam_filter> -map [v] -map 0:a? -c:a copy <video_args_at(Save)> -g <gop> -avoid_negative_ts make_zero -t <L> <output>`.
  - `pub fn attempts(encoder: EncoderId) -> Vec<(EncoderId, bool /*hwaccel*/)>` — `[(enc, true), (X264, true), (X264, false)]`, deduplicated.

- [ ] **Step 1: Write the failing tests** (style of `edit.rs:1274-1447`, `args.windows(2)`, `position`/`rposition`):
  - `the_default_cam_sits_bottom_right_inside_the_margin`: `cam_rect((1920, 1080), &CamLayout::default()) == Rect { x: 1412, y: 782, w: 480, h: 270 }`; top left is `(28, 28)`.
  - `a_circle_is_cropped_square`: circle → `w == h == 270`, `x == 1622`; `cam_crop((1280, 720), Circle) == Some(Rect { x: 280, y: 0, w: 720, h: 720 })`.
  - `every_number_is_even_and_inside_the_frame`: for 1366×768, 2560×1080, 3440×1440, 1080×1920 and the four corners at sizes 0.12 and 0.5 → all even, `x + w + m <= W`, `y + h + m <= H`.
  - `a_portrait_screen_narrows_the_cam_instead_of_running_off_it`: `cam_rect((1080, 1920), size 0.5, bottom right) == Rect { x: 48, y: 1318, w: 984, h: 554 }`.
  - `the_cam_gets_a_seek_exactly_when_it_starts_before_the_clip`: `cam_timing(-412_000, 0, 21_333, 0, 30_000)` → `seek_s == Some(0.412)`, `shift_s == 0.021333`, and the graph says `setpts=PTS+0.021333/TB` with no `STARTPTS`; `cam_timing(1_500_000, 0, 21_333, 0, 30_000)` → `seek_s == None`, `shift_s == 1.521333`.
  - `the_video_start_is_read_off_ffprobe`: `parse_start_us("0.021333\n") == Some(21_333)`, `"N/A"` → `None`.
  - `the_delay_moves_the_cam`: offset −412_000, delay −40 → `seek_s == Some(0.452)`.
  - `a_trimmed_clip_starts_the_cam_at_its_place_in_the_original`: start 4000 → `seek_s == Some(4.412)`.
  - `the_cam_is_bound_to_the_clip_length`: the graph has `trim=end=30.021333`; `-t 30.000` comes after the last `-i`.
  - `a_gap_hides_the_cam`: gap (1500, 3000) → `enable='not(between(t,1.521333,3.021333))'`; no gaps → no `enable=`.
  - `a_mirrored_circle_is_cropped_flipped_and_masked`: `crop=720:720:280:0,hflip,scale=270:270` and `alphamerge`, three `-i`.
  - `the_audio_is_copied_and_nothing_cuts_the_game_short`: `["-map", "0:a?"]`, `["-c:a", "copy"]`; no `vstack`, no `shortest`, no `-itsoffset`, no `geq`.
  - `hardware_decoding_only_when_asked`: `hwaccel: true` puts `-hwaccel d3d11va` before the first `-i`.
  - `a_save_time_composite_uses_the_fast_presets`: `video_args_at(Nvenc, 70, Save)` has `p3`, Amf `speed`, Qsv `veryfast`, X264 `superfast`, all with the quantiser of quality 70 (20).
  - `the_retries_fall_back_to_software_step_by_step`: `attempts(Nvenc) == [(Nvenc, true), (X264, true), (X264, false)]`, `attempts(X264) == [(X264, true), (X264, false)]`.
  - `a_circle_mask_is_clear_in_the_corners`, `a_rounded_mask_is_opaque_along_the_middle_of_each_edge`, `a_rectangle_needs_no_mask`.
- [ ] **Step 2: Run** `cargo test --lib composite edit muxer` — Expected: FAIL to compile.
- [ ] **Step 3: Implement** as specified; numbers are formatted `{:.6}` for seconds and `{:.3}` for `-t`, like `edit.rs:651`.
- [ ] **Step 4: Run** `cargo test --lib` — Expected: PASS; the existing `video_args` tests (1487-1520) unchanged.
- [ ] **Step 5: Verify on the Windows machine:** `cargo run --example composite-probe -- <a phase-1 clip> <its cam.mp4> --offset-us <camOffsetUs>` → the output plays with the cam bottom right with rounded corners, the clap of a hand in view lands on its sound within a frame or two, the length equals the clip's.
- [ ] **Step 6: Commit** `Composite builders: Rust turns the layout into even pixel numbers and the cam's offset, delay and the clean clip's real video start into a seek and a shift, the graph holds nothing but those literals — bounded to the clip's length, gaps hidden with enable, masks for rounded corners and circles rendered as PNGs — and the save-time encoder presets; a composite probe runs it on a real clip`

---

### Task 16: Saving, phase 2 — the background composite

**Files:**
- Modify: `src-tauri/src/composite.rs` (queue, worker, job, crash recovery)
- Modify: `src-tauri/src/muxer.rs:52-64` (`pub fn background(name: &str) -> Command` with `CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS` (`0x0000_4000`))
- Modify: `src-tauri/src/edit.rs:537` (`move_across` → `pub(crate)`), `804-851` (`run_with_progress` keeps its signature over a new `pub(crate) fn run_command_with_progress(command: Command, args, length_ms, on_progress)`)
- Modify: `src-tauri/src/lib.rs:105-149` (`composite::enqueue` right after Task 14's `set_facecam` when the state is `pending`), `1258-1270` (`composite::recover(library)` right after `edit::repair`, inside that library block; `composite::start(handle)` after the block)
- Modify: `src-tauri/src/commands.rs:404-416` (`save_clip` with seconds enqueues too)
- Modify: `src/routes/Clips.tsx:509-538` (a pill "Adding cam" while `clip.facecam?.state === "pending"`)

**Interfaces:**
- Consumes: Tasks 4, 12, 13, 15.
- Produces:
  - `pub fn start(app: &tauri::AppHandle)`, `pub fn enqueue(clip_id: &str)`, `pub fn cancel(clip_id: &str) -> bool` (drops a *queued* job; a running one is waited for through the clip lock), `pub fn wait_settled(clip_id: &str, limit: Duration) -> bool`, `pub fn recover(library: &Library)`.
  - `struct Queue { waiting: VecDeque<String>, later: Vec<(String, Instant)>, running: Option<String> }` with `push(&mut self, id) -> bool`, `push_later(&mut self, id, at: Instant) -> bool` (a swap retry, step 7), `next(&mut self, now: Instant) -> Option<String>` (the waiting first, then a `later` entry whose time has come), `done(&mut self, id)`, `busy(&self, id) -> bool`, `drop_waiting(&mut self, id) -> bool` (both lists; `cancel` also removes the temp a `later` job keeps). The worker's `Condvar` wait is bounded by the earliest `later` time.
  - `pub fn room_for_composite(clip_bytes: u64, free_clip_drive: u64, free_data_drive: u64, same_drive: bool) -> bool` — the clip drive needs 1.2 × the clip + 64 MB, the data drive (when another one) the clip + 64 MB.
  - `pub enum Recovery { Nothing, PutCleanBack, FinishSwap, Requeue }`; `pub fn after_crash(facts: &FacecamFacts, clip_bytes: Option<u64>, clean_in_store: bool, state: FacecamState) -> Recovery`.
  - The swap retry of spec 2.8: `pub fn swap_waits() -> [Duration; 5]` = 2, 5, 10, 20, 30 s (about a minute of back-off), then `pub const SWAP_RETRY_EVERY: Duration` = 5 min while the app runs, and again at the next start; `pub fn swap_given_up(failing_since_ms: Option<i64>, now_ms: i64) -> bool` — true once a day has passed since `FacecamFacts.swap_failing_since_ms` (Task 12).

- [ ] **Step 1: Write the failing tests:**
  - `a_clip_is_never_queued_twice`, `a_running_clip_counts_as_busy`, `jobs_run_in_the_order_they_came`, `a_deleted_clip_leaves_the_queue` (`drop_waiting` true for a waiting id, false for the running one).
  - `a_composite_needs_room_for_the_clip_once_more`: `room_for_composite(1_000_000_000, 1_300_000_000, 0, true)` true, `(…, 1_200_000_000, 0, true)` false; other drive: clip drive fine but data drive 900 MB → false.
  - `a_crash_mid_composite_is_resolved_from_the_file_size` (Review Focus): clip missing + clean in store → `PutCleanBack`; `applying` with `size_bytes: 100` and the clip at 100 bytes → `FinishSwap`; at 90 → `Requeue`; no `applying`, state `Pending` → `Requeue`; `applied` set, state `Ready` → `Nothing`; no `applied`, state `Ready` (hidden at save) → `Nothing`.
  - `a_clip_open_in_a_player_is_waited_for_not_failed`: `swap_waits()` sums to about a minute; `swap_given_up(Some(t), t + 23 h)` false, `swap_given_up(Some(t), t + 25 h)` true, `swap_given_up(None, _)` false; `after_crash` with `swap_failing_since_ms` set, state `Pending` → `Requeue`.
  - `a_swap_retry_waits_its_turn`: `push_later("a", now + 5 min)`, `push("b")` → `next(now) == Some("b")`, then `next(now) == None`, and `next(now + 5 min) == Some("a")`.
- [ ] **Step 2: Run** `cargo test --lib composite` — Expected: FAIL to compile.
- [ ] **Step 3: Implement the worker** — one `clippiboy-composite` thread blocked on a `Condvar`; it takes the next id from the queue, **releases the queue mutex**, then runs the job under `edit::clip_lock(id)`:
  1. Re-read the row; gone, or state not `Pending` → done. No facts → `set_facecam(None)` and done.
  2. `room_for_composite` with `disk::free_bytes` for the clip's folder and `config::data_dir()`; no room → state `Failed`, notice "Not enough free space to add the cam — the clip was kept without it.".
  3. `video_start_us` = `probe_video_start_us(clip)` or 0; the mask PNG into `<data>/temp/<id>_mask_<w>x<h>.png` when the shape needs one; `gop = facts.game_gop` — the fitted fps of the running recording (`active_recording`, read at the save, Task 14) times its keyframe seconds, not the config's `recording.fps`, because `capture::fit_to_target` (`capture.rs:633-657`) can change the fps.
  4. For each of `attempts(encoder)`: `composite_arguments` into `<clip>.<pid>.cam.mp4`, `run_command_with_progress(muxer::background("ffmpeg"), …)`; break on success, otherwise remove the temp and go on.
  5. All failed → state `Failed`, `notify(app, "error", "The cam couldn't be added — try again from the editor")`, temp and mask removed; the clip is untouched.
  6. Success → probe the length; write facts with `applying` (layout, delay, start 0, length, the temp's size); if `edit::video_path(id)` does not exist, `edit::move_across(clip → video.mp4)`; `muxer::replace_file(temp → clip)`; facts with `applied = applying.take()` and `video_start_us`; `thumbs::make`; `set_file_state`; `set_facecam` state `Ready`; `clips-changed`.
  7. A clip open in a player makes the move or the replace fail (spec 2.8). Then the clean clip goes back to its path if it was moved, `swap_failing_since_ms` is set if it is not set yet, and the clip lock is released while the worker waits `swap_waits()` one by one before trying the two steps again with the same composite. After the last wait the job goes back with `push_later(id, now + SWAP_RETRY_EVERY)` and keeps its temp, so other clips go first and the swap is tried again every five minutes while the app runs; when its turn comes, a temp whose size equals `applying.size_bytes` sends the job straight to step 6's swap, without compositing again. No signal from the player is needed. The state stays `Pending` throughout. It becomes `Failed` with the notice of step 5 only once `swap_given_up` says a day has passed, or when the composite itself fails (step 5). A swap that succeeds clears `swap_failing_since_ms`.
  `recover` at start: per row with a facecam and a store, `after_crash` decides; `PutCleanBack` moves `video.mp4` back to the clip path and requeues; a `<clip>.<pid>.cam.mp4` left by an earlier run is removed, and a clip whose swap was failing is requeued, so it composites afresh and tries the swap again at this start.
- [ ] **Step 4: Run** `cargo test --lib` and `npx tsc --noEmit` — Expected: PASS.
- [ ] **Step 5: Verify on the Windows machine:**
  - save a 30 s 1080p60 clip with a game running: "Clip saved" as fast as before; the gallery shows "Adding cam"; within about 10 s the pill goes, the clip plays with the cam bottom right, `originals/<id>/video.mp4` is the clean clip, `facecam.json` has `applied`;
  - two saves 5 s apart → both get their cam, one after the other;
  - open the clip in the player while "Adding cam" shows, close it 20 s later → the cam lands within the next wait, no "couldn't be added" notice; keep it open for ten minutes → still no notice, and the cam lands within five minutes of closing it;
  - quit from the tray while "Adding cam" shows, start again → the clip is whole and gets its cam once;
  - set the encoder to NVENC and run three other NVENC sessions → the log shows the x264 retry, the clip still gets its cam;
  - Task Manager shows the ffmpeg process at "Below normal".
- [ ] **Step 6: Commit** `Saving, second half: a worker below normal priority lays the cam over each new clip a few seconds after the save, files the clean clip into the originals store and swaps the composite in — hardware first, x264 after, never two at once, never on a disk too full for it, and a quit in the middle resolved at the next start from the composite's exact size; the gallery marks a clip while its cam is on the way`

---

### Task 17: `edit.rs` keeps the facecam store, and "Free up space" bakes the cam

**Files:**
- Modify: `src-tauri/src/edit.rs:128-138` (`remove` keeps a facecam store; new `remove_everything`), `159-170` (`discard_original` → `Discarded`), `172-196` (`trim_originals_on_disk` skips folders with a `facecam.json`), `571-629` (`files_away` replaces `archives_the_clip`; the note only with a trim), tests 1356-1390
- Modify: `src-tauri/src/commands.rs:740-777` (`delete_clip`: `composite::cancel`, then `edit::remove_everything`), `1127-1146` (`discard_clip_original` becomes `#[tauri::command(async)]`: `wait_settled` first, then under `clip_lock`; state `Baked` or column `None`), `1276-1304` (`storage_usage`: `facecam_bytes`, `shot_edits_bytes` without it), new `bake_clip_cam` and `bake_all_cams` beside `discard_all_originals` (1314-1322)
- Modify: `src-tauri/src/lib.rs:1323-1405`, `src-tauri/build.rs:8-90` (`"bake_clip_cam"`, `"bake_all_cams"`), `src-tauri/capabilities/default.json` (`allow-bake-clip-cam`, `allow-bake-all-cams`); generated `permissions/autogenerated/{bake_clip_cam,bake_all_cams}.toml` and `gen/schemas/*`
- Modify: `src-tauri/src/model.rs:805-828` (`StorageUsage.facecam_bytes: u64`), `src/lib/types.ts:461-472`, `src/lib/ipc.ts` (`bakeClipCam`, `bakeAllCams`), `src/store.ts` (`bakeClipCam` replaces the clip with the answer, like `discardClipOriginal`)
- Modify: `src/routes/settings/Storage.tsx:64-96` (row "Clean pictures for moving the cam", hint "The clean picture and the cam of every clip with a facecam, so the cam can still be moved."), a `BakeCams` card after it built like `ClearTrims` (135) / `ClearTrimsCard` (153)
- Modify: `src/components/clipMenu.tsx:219-233` ("Free up space…" also for a clip whose `facecam.state` is `ready`, through a new `onBakeCam` option), `src/routes/Clips.tsx:440-453`, `586-597` (wire it, with its own confirmation)

**Interfaces:**
- Produces:
  - `pub fn remove(clip_id)` — with a facecam store only the trim note goes; `pub fn remove_everything(clip_id)` — today's `remove` plus `cam.mp4` and `facecam.json` (only `delete_clip` calls it).
  - `pub enum Discarded { Original, Baked }`; `discard_original(clip_id) -> Result<Discarded, String>` — on a facecam store it calls `bake`.
  - `pub fn bake(clip_id: &str) -> Result<u64, String>` — removes `video.mp4`, `cam.mp4`, `facecam.json` and the trim note (undo goes with it, the same trade as today's "Free up space"), returns the bytes freed; for an untrimmed clip as well as a trimmed one.
  - `#[tauri::command(async)] pub fn bake_clip_cam(state, app, id: String) -> Result<Clip, String>` — `wait_settled`, then under `clip_lock`: `edit::bake`, state `Baked` (composited) or column `None`, `clips-changed`.
  - `#[tauri::command(async)] pub fn bake_all_cams(state, app) -> Result<TrimOriginals, String>` — the same for every clip with a store, one clip lock at a time; returns count and bytes, like `discard_all_originals`.
  - `fn files_away(already_trimmed: bool, keeps_original: bool, archive_exists: bool, source: VideoSource, composited: bool) -> bool` — `archive_exists` → false; `composited` → false; else today's rule. Task 18 adds the branch "`source == CleanWithCam` → true" before the `composited` one. `composited` is `clip.facecam` with state `Ready` and a visible layout.

- [ ] **Step 1: Write the failing tests:**
  - `trim_bookkeeping_never_clears_a_facecam_store` (real data dir under a test id): `video.mp4`, `cam.mp4`, `facecam.json`, `trim.json` → `remove(id)` leaves the first three; `remove_everything(id)` leaves nothing.
  - `clear_all_trims_leaves_facecam_clips_alone`: a test folder with `video.mp4` + `facecam.json` is not in `trim_originals_on_disk`, one with `video.mp4` + `trim.json` is.
  - `freeing_a_facecam_clips_space_bakes_the_cam`: `discard_original` → `Ok(Discarded::Baked)`, the folder is gone.
  - `an_untrimmed_clip_can_be_baked_too`: a store with `video.mp4`, `cam.mp4`, `facecam.json` and no note → `bake` returns the three sizes summed, the folder is gone.
  - `startup_repair_leaves_a_facecam_store_alone`: `Library::in_memory`, a clip file in a temp dir, row state `Ready`, store with `video.mp4`, `cam.mp4`, `facecam.json`, no note → after `repair`, all three files are there.
  - `a_composited_clip_is_never_filed_as_the_original`: `files_away(false, true, false, VideoSource::Clip, true) == false`; the four existing `archives_the_clip` tests carried over with `composited: false`.
- [ ] **Step 2: Run** `cargo test --lib edit` — Expected: FAIL.
- [ ] **Step 3: Implement.** `repair`'s arms need no change of their own once `remove` keeps the store; `discard_all_originals` (217-230) inherits the skip. `discard_clip_original` sets the state to `Baked` when the clip was composited (as defined above), and the column to `None` otherwise (the file never had the cam). Both confirmations of "Free up space" — `src/components/ClipEditor.tsx:203-207` and `src/routes/Clips.tsx:587-591` — read `confirmTitle="The trim stays, undo goes. The cam can no longer be moved in this clip."` for a clip whose `facecam.state` is `ready`; the confirmation of an untrimmed clip's bake reads `question="Bake the cam into the clip?"`, `confirmTitle="The cam can no longer be moved in this clip."`. `BakeCams` shows the count and size, asks "Bake the cam into every clip?" with "The cams can no longer be moved.", and counts the freed size down as `ClearTrims` does. The two new commands go into `generate_handler!`, `build.rs` and `default.json`; one `cargo build` via PowerShell regenerates the TOMLs and schemas.
- [ ] **Step 4: Run** `cargo test --lib` and `npx tsc --noEmit` — Expected: PASS.
- [ ] **Step 5: Verify on the Windows machine:** "Free up space…" on an untrimmed clip with its cam in → the confirmation, then the clip plays with the cam where it was and the storage row shrinks by its clean picture and cam; "Bake all" on the storage page → the row reads 0 B; "Clear all trims" before that left the facecam clips' rows alone.
- [ ] **Step 6: Commit** `Editor bookkeeping learns the facecam store: a save that trims nothing, an undo or the startup repair no longer delete a clip's clean picture and cam along with the trim note, Clear all trims leaves facecam clips alone, a composited clip is never filed away as the original, Free up space on a facecam clip — trimmed or not — bakes the cam, and the storage page counts the clean pictures as their own row with a way to bake them all`

---

### Task 18: A front cut and undo re-composite the cam (spec 3.4 rows 4–5)

**Files:**
- Modify: `src-tauri/src/edit.rs:250-258` (`VideoSource::CleanWithCam`), `262-275` (`RenderPlan.cam_input: bool`), `289-337` (`plan`), `339-349` (`restore_plan`), `351-359` (`Applied.composited: bool`, read by `store()`), `363-396` (`apply`, `restore` compute `CamPart`), `398-521` (`run`: probe the clean source's video start, mask, facts `applying`/`applied`), `632-737` (`arguments` gets `cam: Option<&CamInputs>`), tests (incl. `a_written_clip_keeps_at_most_one_audio_track`, 1416-1447, which must count `[v]` as the video map)
- Modify: `src-tauri/src/commands.rs:1074-1118` (`apply_clip_edit`, `restore_clip_original`: `composite::wait_settled(id, 5 min)` first, else "The cam is still being added to this clip — try again in a moment."), `1383-1430` (`store`: state `Ready` after a composite), `928-962` and `1151-1160` (`export_for_discord`, `shrink`: `wait_settled` first)
- Modify: `src-tauri/src/share.rs:248-260` (`share_send`: `wait_settled` through `tauri::async_runtime::spawn_blocking`)
- Modify: `src/components/ClipPlayer.tsx:550-557` (`dirty` also when `clip.facecam?.state === "failed"`, so Save is the "try again from the editor" the notice promises), `src/components/ClipEditor.tsx:250-265` (status line for that case: "The cam is not in the file yet — Save adds it.")

**Interfaces:**
- Consumes: `cam_filter`, `cam_timing`, `cam_rect`, `cam_crop`, `mask_gray8`, `write_mask_png`, `FacecamFacts`, `clip_lock`.
- Produces:
  - `pub enum CamPart { None, Shown { differs: bool } }` — `Shown` when the clip has a facecam store, a visible layout and a clean source (`video.mp4`, or the clip file itself while the state is `failed`); `differs` when the file does not show the stored layout and delay (state `failed`, or `applied` not equal). `Baked`, hidden, `pending` (waited for) → `None`.
  - `pub fn plan(base: Option<&ClipOriginal>, has_original: bool, clip_duration_ms: u64, trim: Trim, cam: CamPart) -> RenderPlan` — with `Shown`: front cut or `differs` → `CleanWithCam` at the absolute start, `reencode`; else today's copy from the clip. `apply` passes `has_original` true for a facecam store's clean `video.mp4` even without a trim note. Every `plan`/`restore_plan` call in the tests (1128-1455, incl. the argument tests at 1276, 1305, 1327, 1419-1427 and 1455) gets `CamPart::None` and must not change otherwise.
  - `pub fn restore_plan(base: &ClipOriginal, cam: CamPart) -> RenderPlan` — `Shown` → `CleanWithCam` from 0, full length, `reencode`.
  - `pub struct CamInputs { pub cam: PathBuf, pub mask: Option<PathBuf>, pub facts: FacecamFacts, pub layout: CamLayout, pub delay_ms: i32, pub video_start_us: i64 }`. `arguments` puts the cam after the stems (input `n + 1`, the mask `n + 2`), joins the audio and video graphs into one `-filter_complex` with `;`, maps `[v]`, and uses `video_args_at(.., Speed::Edit)`.

- [ ] **Step 1: Write the failing tests:**
  - `a_front_cut_on_a_facecam_clip_composites_from_the_clean_picture`: `plan(None, true, 30_000, Trim { start_ms: 4_000, end_ms: 12_000 }, CamPart::Shown { differs: false })` → `CleanWithCam`, `reencode`, `video_start_ms == 4_000`, `length_ms == 8_000`.
  - `a_tail_trim_on_a_facecam_clip_stays_a_copy`: `Trim { 0, 12_000 }`, `Shown { differs: false }` → `Clip`, `!reencode`.
  - `a_clip_whose_cam_failed_gets_it_on_the_next_save`: `Trim::whole(30_000)`, `Shown { differs: true }` → `CleanWithCam`, `reencode`.
  - `undo_on_a_facecam_clip_re_composites`: `restore_plan(&base, Shown { differs: false })` → `CleanWithCam`, `reencode`, `length_ms == base.duration_ms`; with `CamPart::None` it stays `restoring_is_a_lossless_copy_of_everything` (1261).
  - `the_cam_comes_after_the_stems`: two stems + `CamInputs` → input order clean (`-ss 4.000`), stem 0 and stem 1 (each `-ss 4.000`), cam (`-ss 4.452000` for offset −412 000 and delay −40), mask; one `-filter_complex` holding both `[aout]` and `[v]`; `[3:v]` is the cam with `setpts=PTS+0.021333/TB` (video start 21 333 µs), `[4:v]` the mask; `-map [v]`, no `-map 0:v:0`.
  - `a_failed_clip_files_its_clean_picture_before_the_cam_goes_in`: `files_away(false, false, false, VideoSource::CleanWithCam, false) == true`.
- [ ] **Step 2: Run** `cargo test --lib edit` — Expected: FAIL.
- [ ] **Step 3: Implement.** In `run`: for `CleanWithCam`, `video_start_us = muxer::probe_video_start_us(&video)` before `arguments`, the facts' gaps shifted by `−video_start_ms` and clipped to `[0, length_ms)`, the mask into `<data>/temp/`, facts `applying` before `swap_in` and `applied` after it (the protocol of Task 16); `swap_in` files the clean clip away when `files_away` says so and writes a note only when `plan.original` is `Some`; `Applied.composited` is true for a `CleanWithCam` run, and `store()` then sets the state `Ready`. The x264 retry (461-479) covers the composite as it covers every re-encode. `apply_clip_edit` and `restore_clip_original` call `wait_settled` before `edit::apply`/`restore`, which take the clip lock inside `run`.
- [ ] **Step 4: Run** `cargo test --lib` — Expected: PASS.
- [ ] **Step 5: Verify on the Windows machine** with a clip whose cam is in: cut 5 s off the front in the editor → the cam is where it was, in sync, the picture one generation from the recording; undo the trim → the full clip, cam in place; a back trim only → finished in a second (a copy); a clip left `failed` (save it with the clip drive nearly full, so the job's room check refuses, then free the space) opens with Save enabled and "The cam is not in the file yet — Save adds it.", and Save puts the cam in; delete the clip from the console while the editor saves → the delete waits, no file is left behind.
- [ ] **Step 6: Commit** `Editor and the cam: trimming exists already, so the moment clips have a baked cam a cut at the front and an undo re-composite from the clean picture and cam.mp4 with the clip's stored layout, in one ffmpeg run with the stems, instead of cutting the cam off or baking a second one in; a back trim stays a copy, and edits, exports and shares of a clip whose cam is still on the way wait for it`

---

### Task 19: The preview and Cam-off commands

**Files:**
- Modify: `src-tauri/src/commands.rs` (`cam_preview_frame`, `set_facecam_enabled`, `open_camera_privacy`, `preview_allowed`)
- Modify: `src-tauri/src/console.rs:44-52` (`pub static CONSOLE_SHOWN: AtomicBool`), `90-104` (cleared on `Focused(false)` and on `WindowEvent::Destroyed`), `185` (set after `show`), `217`, `230` (cleared)
- Modify: `src-tauri/src/lib.rs:1323-1405`, `src-tauri/build.rs:8-90`, `src-tauri/capabilities/console.json:13-34` (`allow-cam-preview-frame`, `allow-set-facecam-enabled`), `src-tauri/capabilities/default.json` (those two plus `allow-open-camera-privacy`); `overlay.json` gets none
- Create (generated): `src-tauri/permissions/autogenerated/{cam_preview_frame,set_facecam_enabled,open_camera_privacy}.toml`, regenerated `gen/schemas/*`
- Modify: `src/lib/ipc.ts` (`camPreviewFrame(after): Promise<ArrayBuffer>`, `setFacecamEnabled(enabled): Promise<AppConfig>`, `openCameraPrivacy()`)

**Interfaces:**
- Consumes: `PreviewSlot`, `encode_preview` (Task 10), `tell_main` (Task 5), `cam_preview_until` (Task 11).
- Produces:
  - `#[tauri::command] pub async fn cam_preview_frame(app: AppHandle, webview: tauri::WebviewWindow, after: u64) -> Result<tauri::ipc::Response, String>` — refused by `preview_allowed`; from `main` it renews `cam_preview_until` to now + 3 s (and `reconcile_cam` when that starts the lease); waits up to 500 ms on `spawn_blocking` for a frame newer than `after`; an empty response means "none yet".
  - `pub fn preview_allowed(label: &str, console_shown: bool, cam_enabled: bool, console_preview: bool, in_capture: bool) -> Result<(), &'static str>` — `"main"` needs `cam_enabled`; `"console"` needs all of `console_shown`, `cam_enabled`, `console_preview` and `!in_capture`; anything else is refused.
  - `#[tauri::command(async)] pub fn set_facecam_enabled(state, app, webview: tauri::WebviewWindow, enabled: bool) -> AppConfig` — `async` because it writes the config, and a plain sync command runs on the main thread; `replace_config` with only that field changed, `tell_main` when the caller is not `main`, no `console::relayout` (review CON-6).
  - `#[tauri::command] pub fn open_camera_privacy(app: AppHandle) -> Result<(), String>` — `app.opener().open_url("ms-settings:privacy-webcam", None::<&str>)` from Rust, so no URL scope has to be widened.

- [ ] **Step 1: Write the failing test** `the_console_sees_the_cam_only_while_it_is_shown_and_out_of_recordings`: the table over `preview_allowed` — `("console", true, true, true, false)` Ok; each of hidden, cam off, preview off, in capture → Err; `("main", false, true, false, true)` Ok; `("main", _, false, ..)` Err; `("overlay", ..)` Err.
- [ ] **Step 2: Run** `cargo test --lib commands` — Expected: FAIL to compile.
- [ ] **Step 3: Implement** and register in all four places (`generate_handler!`, `build.rs`, the two capabilities); one `cargo build` via PowerShell regenerates the TOMLs and schemas. A grant without its `build.rs` entry fails that build; a command missing from `generate_handler!` only fails at run time, so check the console devtools in Step 5.
- [ ] **Step 4: Run** `cargo test --lib` and `npx tsc --noEmit` — Expected: PASS.
- [ ] **Step 5: Verify on the Windows machine:** in the console's devtools, `__TAURI_INTERNALS__.invoke("cam_preview_frame", { after: 0 })` resolves while the console is open and is refused after Escape; the same from the banner window is refused.
- [ ] **Step 6: Commit** `Cam preview over the bridge: a narrow command hands the newest small frame as raw bytes, only to the main window and to the console while it is on screen and kept out of recordings — the console is never throttled while hidden, so the core refuses as well as the page stopping; Cam off gets a command of its own for the console, which has no set_config, and tells the main window`

---

### Task 20: The Facecam settings tab

**Files:**
- Create: `src/routes/settings/Facecam.tsx` (exports `FacecamTab`, built like `Banner.tsx:7-19`)
- Create: `src/lib/facecam.ts` (`camBox`, `cornerOf`, `layoutAt`, `camStateLine`, `decodePreview`, `startCamPreview`)
- Modify: `src/routes/settings/index.tsx:17-26` (`{ key: "facecam", label: "Facecam", page: FacecamTab }` after Console), `src/store.ts:23-32` (`"facecam"`), store state `cameras: CameraDevice[]`, `refreshCameras()` (not in `init`, 264-273), `setFacecamEnabled(on)` (optimistic, newest answer wins with the `configWrites` ticket like `patchConfig`, 371-388, and the revision rule of Task 5), `camStatus: CamStatus | null` filled from `s.cam` in the `onStatus` mapping (276-288)
- Modify: `src/routes/settings/shared.tsx:79-121` (`CornerPad` gets `mark?: "banner" | "cam"`; the cam mark is 16:9)
- Modify: `src/routes/Recording.tsx:464-495` (under the memory budget: "Facecam: {n} MB on top" when the cam is on for clips, from `camMemoryMb`, the mirror of `effective_cam_memory_bytes`)

**Interfaces:**
- Consumes: `api.listCameras`, `api.setFacecamEnabled`, `api.camPreviewFrame`, `api.openCameraPrivacy`, `EngineStatus.cam` (through `camStatus` in the store), `FacecamConfig`.
- Produces (`src/lib/facecam.ts`, used by Task 21 too):
  - `camBox(layout: CamLayout, width: number, height: number): { left: number; top: number; width: number; height: number }` — the rule of `composite::cam_rect`, kept in step by hand.
  - `cornerOf(layout): OverlayCorner | null`, `layoutAt(corner): { x: number; y: number }`.
  - `camStateLine(state: CamState): string` — `shared` "Shared with other apps", `taken` "Taken by {by}" or "Taken by another app", `blocked` "Blocked by Windows privacy", `noCamera` "No camera", `starting` "Starting…", `failed` its message.
  - `decodePreview(buffer: ArrayBuffer): { seq: number; width: number; height: number; pixels: Uint8ClampedArray } | null`.
  - `startCamPreview(canvas: HTMLCanvasElement, opts: { onlyWhileVisible: boolean }): () => void` — asks for the next frame after drawing one (`putImageData`, through refs, no React state per frame), waits 200 ms after an empty answer, stops on a refusal; with `onlyWhileVisible` it pauses while `document.visibilityState !== "visible"`, so a main window hidden in the tray does not keep the camera on.

- [ ] **Step 1: Build the tab**, sections per spec 5.2 (no virtual camera, no recordings or screenshots):
  1. "Facecam": `Row` "Facecam", hint "Your webcam in your clips. Off, the camera is not opened at all.", `Toggle` → `setFacecamEnabled`; under it the preview (16:9 canvas, `border-radius`/circle and `scaleX(-1)` from the layout) and `camStateLine(camStatus.state)` ("Off" while `camStatus` is `null`); on `blocked` the line "Windows blocks the camera for desktop apps" and a button "Open camera privacy"; on `taken` the hint "Windows 11: Settings › Bluetooth & devices › Cameras › your camera › Allow multiple apps to use camera at the same time".
  2. "Camera": `Row` "Camera" with a `Select` — "First camera", the list, and the saved one as "{name} (not connected)" when it is not there, the `screenChoice` idea (`shared.tsx:19-48`); the list refreshes when the tab opens; hint "OBS Virtual Camera and other cameras only older programs can see are not supported yet." (Open for Fabi 13 (a)). `Row` "Resolution", hint "Applies the next time the camera starts.", `Segmented` 720p / 1080p.
  3. "Where it appears": `Row` "Clips", hint "Saved clips get the cam a few seconds after saving."; `Row` "Console preview", hint "Shows where the cam will sit while the console is open."
  4. "Default position": `Row` "Corner" with `CornerPad mark="cam"`; `Row` "Size" with a `Slider` 12–50 % (draft + `onCommit`, like `Recording.tsx:395-417`); `Row` "Shape" with `Segmented` "Rectangle" / "Rounded" / "Circle" (`CamShape::Rect` reads "Rectangle"); `Row` "Mirror", hint "Off, others see what the camera sees."
  5. "Sync": `Row` "Cam delay" with a `Slider` −300…300 step 10 and `onCommit`, the value as "−40 ms", hint "Clap once in view and save a clip. If you hear the clap before you see it, move this to the left."
  Every dependent row is `disabled={!config.facecam.enabled}`; patches go through `patchConfig({ facecam: { ...config.facecam, ...p } })`, the master switch through `setFacecamEnabled`.
- [ ] **Step 2: Verify** `npx tsc --noEmit` (WSL) and `npm run build` (PowerShell) — no errors. In the browser with `mock.ts`: the tab shows both mock cameras, every control moves its value.
- [ ] **Step 3: Verify on the Windows machine:** switch on → the LED comes on and the preview moves within two seconds even with the buffer off; leave the tab → the LED goes off within about five seconds; hide the window to the tray on the tab → the LED goes off; privacy off → "Blocked by Windows privacy" and the button opens the Windows page; unplug → "No camera", then back.
- [ ] **Step 4: Commit** `Facecam tab: the master switch with a live preview and what the camera is doing, the camera and its size, the cam in clips and in the console, the default corner, size, shape and mirror, and the sync slider with the clap test in one line — sliders write once when let go, the camera list loads when the tab opens, and a window hidden in the tray stops asking for frames`

---

### Task 21: The console preview

**Files:**
- Modify: `src/console/Console.tsx:213-215` (`previewStop` ref), `322-332` (`close()` stops it), `363-394` (`console-closed` stops it), `410-443` (`begin()` starts it from the `getConfig()` answer when `facecam.enabled && facecam.consolePreview && !consoleInCapture`), `444-446` (cleanup stops it); a `CamTile` component positioned with `camBox` over the whole screen, and a "Cam off" button beside it calling `api.setFacecamEnabled(false)`
- Modify: `src/console/console.css` (the tile and its button; no `backdrop-filter`, the console runs without GPU)

**Interfaces:**
- Consumes: `startCamPreview` with `onlyWhileVisible: false` (the console's own visibility is the core gate), `camBox`, `camStateLine`.

- [ ] **Step 1: Implement.** The tile sits at the default position and size, shaped and mirrored like the clip; with `consoleInCapture` it shows "The cam preview is hidden while the console shows in recordings." instead of a picture; "Cam off" stops the pump and hides the tile without a relayout.
- [ ] **Step 2: Verify** `npx tsc --noEmit` (WSL) and `npm run build` (PowerShell) — no errors.
- [ ] **Step 3: Verify on the Windows machine** (buffer and cam on, a game in borderless fullscreen): open the console → the face appears in the bottom-right corner within half a second, where the cam lands in a clip saved right after; Escape → `cam_preview_frame` calls stop (log at debug), the LED stays on (the buffer still wants it); "Cam off" → the tile goes, the LED goes off, and the main window's Facecam switch shows off without a reload; with "visible in recordings" on → the hint instead of the face.
- [ ] **Step 4: Commit** `Console preview: while the console is open the face is drawn exactly where the cam will sit in the clip, pulled frame by frame so it stops with the page, hidden when the console shows in recordings, with a Cam off button that switches the camera off on the spot`

---

### Task 22: Measure on the Windows machine and write the numbers into the spec

**Files:**
- Modify: `docs/superpowers/specs/2026-10-20-facecam-design.md` (2.3 bitrate and memory, 2.5 delay default, 2.9 cost)

- [ ] **Step 1: Composite speed** (`docs/extras/research/ffmpeg-compositing.md` 5.6), with `cargo run --example composite-probe`: a 30 s and a 120 s clip at 1080p60 and 1440p60; NVENC p5/p3/p1, AMF balanced/speed, QSV medium/veryfast, x264 superfast; once with a game running and once with `-f null -` in place of the output; note `speed=`. Decides whether `p3` stays the save preset.
- [ ] **Step 2: Clap test** (`docs/extras/research/camera-capture-and-clocks.md` section 10, E6) for the delay default, per camera (C920, C960) and per clock kind: `delayMs: 0` in the config, five claps in view, one clip; per clap the frame where the hands meet against the audio peak; the median becomes the default, the spread goes into 2.5.
- [ ] **Step 3: NVENC sessions** (`docs/extras/research/camera-capture-and-clocks.md` section 10, E9): game + cam + a composite + Discord Go Live or OBS → `nvidia-smi --query-gpu=encoder.stats.sessionCount --format=csv,noheader` at each step; ten buffer restarts afterwards leave the count where it was.
- [ ] **Step 4: Console preview cost**: CPU of the console's `msedgewebview2.exe` over 30 s with the preview open, against 30 s of clip playback in the console (Process Explorer); 320 px BGRA as built, and whether JPEG is worth it.
- [ ] **Step 5: Cam ring memory**: the `cam:` health line's ring MB with a 120 s buffer at 720p and 1080p, bright and dim, against the about 72 MiB (75,375,000 bytes) the formula gives.
- [ ] **Step 6: Commit** `Facecam: what stage 2 measured — composite speed per encoder and preset with a game running, the delay the clap test found for each camera, NVENC sessions with the cam in use, the console preview's cost and the cam ring's real size`
