# Facecam Stage 4: Recordings and Screenshots Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. This plan is rough on purpose: before a task is executed, expand it into checkbox steps with superpowers:writing-plans, against the code as it stands after stages 2 and 3.

**Goal:** Recordings and screenshots get the cam. A recording is finished clean at stop, as today, and the cam is burned in by the background job at the default layout; once the composite is in place the clean file is gone, so a recording's cam cannot be moved (spec 2.10, Open for Fabi 12 (a)). A screenshot taken while the cam runs gets the cam at the default layout, and the screenshot viewer can move, reshape or hide it later.

**Architecture:**
- **Recordings:** the recorder gets a second writer on stage 2's cam packet tap (`PacketTap`, `CamTrack::attach_tap`). Cam packets from the cam keyframe at or before the recording's start go into `.recording/<id>/cam.<n>.h264`, one **segment** per cam session, and the cam facts are in `meta.json` from the start (crash recovery reads it). At stop the recording is muxed as today (clean, `-c:v copy`). Each segment becomes `cam.<n>.mp4` in `originals/<id>/` next to `facecam.json`, and stage 2's composite worker burns them in, one `overlay` per segment with `eof_action=pass`, clips going first. After the swap the segments and the facts are removed and the state is `baked`.
- **Screenshots:** a screenshot keeps the frame's QPC and takes stage 2's raw-frame tap (`CamTrack::latest_frame()`). The cam is composited on the CPU before the watermark, and the picture without the cam plus the cam frame stay in `originals/<id>/`, so `edit_screenshot` can lay the cam again with a new layout. A screenshot never opens the webcam: no cam running, no cam in the shot.

**Tech Stack:** Rust (Tauri 2 core; `cargo test --lib` on Windows), React + TypeScript (ShotViewer, settings), ffmpeg 9.0.1, the `png` crate.

**Spec:** `docs/superpowers/specs/2026-10-20-facecam-design.md`, sections 2.10, 2.2 (lifecycle and lock screen), 2.3 (sessions, holes, sleep), 2.6, 2.7, 5.1 (`in_recordings`, `in_screenshots`) and 5.2 item 3. Open for Fabi 6, 8 and 12. Background: `docs/extras/research/ffmpeg-compositing.md` §2.e (segments) and §3.2 (times).

**From stage 2:** `CamTrack` with `attach_tap`/`detach_tap` (`camtrack::PacketTap`, the backlog from the cam keyframe at or before a time) and `latest_frame() -> Option<Arc<camera::Frame>>` (NV12 in system memory at the camera's size, with its QPC); `CamRing` (`session`, `header`, `set_header`, `reset` on the grid's `Out::Started`, `Gap`); `CamWants`, `cam_wanted`, `stop_delay`, `AutoBuffer::screen_locked`; `facecam::cam_mp4_arguments`, `FacecamFacts`, `write_facts`, `saved_delay_ms`; `ClipFacecam`/`FacecamState`, `Library::set_facecam`; `edit::bake`, `bake_clip_cam`, the gallery menu's "Free up space…" (`clipMenu.tsx`, `onBakeCam`) and the `BakeCams` card; the composite worker (`composite::enqueue`, `cancel`, `recover`, `after_crash`, `room_for_composite`, `Queue`); `composite::cam_timing`, `CamChain`, `cam_filter`, `cam_rect`, `cam_crop`, `mask_gray8`; `camera::cover_crop`, `camtrack::preview_bgra`; `edit::clip_lock(id)`; `FacecamConfig.in_recordings`/`in_screenshots`. **From stage 3:** `RectLayer` and the editor line "The cam is fixed in this clip.", which a burned-in recording shows.

## Global Constraints

- Line numbers are those of 401a320. Stages 2 and 3 move some of them, so look each one up again.
- English UI text. Commits in English, **without** a `Co-Authored-By` line.
- Build: `cargo test --lib` and `npm run build` via PowerShell; `npx tsc --noEmit` from WSL.
- The encoder callbacks never wait: the cam tap only queues (the rule of `recorder.rs:221-233`). `PacketTap::cam` is called under the cam ring's lock; the game ring (`Shared.packets`) and the cam ring are never locked together (spec 2.3).
- The cam never costs a recording. A cam failure leaves the clean recording. `finalize` deletes the work folder only after the cam is secured (`recorder.rs:625-631`; stop rather than lose data, the rule at 577-579). The clean recording is dropped only once the composite is in place.
- The watermark stays on top of the cam in screenshots (`state.rs:972`, `commands.rs:619-621`).
- Fix REC-1 and AUD-6 (`docs/facecam-review.md`; `docs/extras/plans/2026-10-22-fix-sync-and-timestamps.md` Tasks 3 and 9) before shipping cam recordings, or long recordings carry their drift into the cam too.
- **Open for Fabi 12** is followed as recommended: recordings burned in at stop with no clean copy, screenshots keep the clean PNG. If Fabi picks (b) for recordings, Task 4 says what changes. **Open for Fabi 8** (the cam stops behind the lock screen during a recording) follows the recommendation: yes; stage 2's `AutoBuffer::screen_locked` already counts while only a recording runs.

## Review Focus

- **A recording that spans a lock or a sleep:** the cam stops (2.2) or gets a new origin and encoder (2.3), so one `cam.h264` with a hole would shift every later frame → Task 1 `a_cam_restart_starts_a_new_segment`, Task 3 `every_segment_gets_its_own_overlay`.
- **A crash mid-recording, or mid-composite:** the folder alone must give the recording and its cam, and the cam goes in once → Task 2 `a_crashed_recording_still_gets_its_cam`, Task 3 `a_quit_mid_composite_requeues_the_recording`.
- **Disk:** the composite needs about the recording's size again on the clip drive until the swap → Task 2 `the_disk_guard_counts_the_cam_and_the_composite`, Task 4 `a_recording_without_room_for_the_composite_stays_clean`.
- **The clean recording dropped too early, or kept** → Task 4 `the_clean_recording_stays_until_the_composite_is_in_place`, `a_finished_recording_keeps_no_clean_copy`.
- **An hour-long composite blocking clip composites** → Task 3 `clip_jobs_go_before_recording_jobs`.
- **Screenshot re-edit:** a moved cam leaves no trace of the old one, and the mark is neither doubled nor covered → Task 6 tests.

---

### Task 1: The recorder's cam writer, in segments

**Files:**
- Modify: `src-tauri/src/recorder.rs`
  - a cam writer that implements stage 2's `PacketTap` (it only queues);
  - `Job` (96-) gets `Cam(u32, Arc<[u8]>)` and `CamSegment(u32)`;
  - the writer thread (155-185) and `run_job` (383-421) write `cam.<n>.h264`;
  - `Meta` (55-85) gets `#[serde(default)] cam: Option<CamMeta>`, like `separate` and `friends`; it is written at the start and at every new segment;
  - counters as in `video()` (223-233);
  - `finish` (349-376) fills in frames and span per segment;
  - `estimated_size` (246-250) adds the cam bytes.
- Modify: `src-tauri/src/state.rs`
  - `attach_recorder` (1062-1110): once the game recorder is attached under `packets` (which fixes `start_100ns`), and outside that lock, `self.camera.attach_tap(tap, start_100ns)`; its backlog starts the first segment;
  - `close_recording` (1115-1143) calls `detach_tap` first.
- Modify: `src-tauri/src/camtrack.rs`: `PacketTap` gets `fn header(&self, session: u64, origin_100ns: i64, header: &[u8])` with an empty default body, called by `CamRing::set_header` (a new session's SPS/PPS, after `reset` on the grid's `Started`) and by `attach_tap`, so the writer knows when a segment ends and starts the next file with that header, as phase 1's `cam.part.h264` does; the tap survives a stop and start of the cam track while the recording runs (lock screen).

**Interfaces:**
- `pub struct CamMeta { pub width: u32, pub height: u32, pub fps: u32, pub camera: String, pub clock: ClockKind, pub segments: Vec<CamSegmentMeta> }`
- `pub struct CamSegmentMeta { pub file: String, pub session: u64, pub origin_100ns: i64, pub first_pts_us: i64, pub frames: Option<u64>, pub span_us: Option<u64>, pub gaps: Vec<Gap> }` (stage 2's `camtrack::Gap`, in 100 ns)
- `impl PacketTap for RecorderCam { fn cam(&self, packet: &EncodedPacket); fn header(&self, session: u64, origin_100ns: i64, header: &[u8]); }`

**Tests:**
- `a_recorder_with_a_cam_backlog_writes_both_streams`
- `cam_meta_survives_a_reread`, and `an_old_meta_without_cam_still_reads`
- `a_cam_restart_starts_a_new_segment`
- `the_cam_backlog_starts_on_a_cam_keyframe_before_the_start`
- `cam_packets_never_reach_the_video_file` (the `is_video` filter, `recorder.rs:224`)

**Risks:**
- The lock contract of `pipeline.rs:325-331` and `1004-1016` ("no packet missed or written twice") has to hold for the cam too: stage 2's `attach_tap` returns the backlog under the cam ring's own lock, never inside `packets`.
- Segment boundaries follow stage 2: a new segment when the ring resets on the grid's `Started` (after a sleep or a new device clock) or the cam track restarts (lock screen). An unplug stays inside one segment as fillers and a gap. A one-frame hole from the encoder (spec 2.3) cannot be carried by a `-r 30` segment either, so the writer starts a new segment after one (design call).
- Review BUF-3 (file I/O under the ring lock): `Recorder::start` runs with `shared.packets` held (`pipeline.rs:1004-1016`, the call at 1014), so "before any lock" inside `start` does not exist. Do BUF-3's split first and create the first cam file and the cam part of `meta.json` in its `prepare` half, outside the lock; `attach` only hands over the backlogs.
- Review REC-7 (unbounded writer queue): the cam adds about 0.4 MB/s to it. `docs/facecam-review.md` asks this stage to fix BUF-3 and REC-7 before `cam.h264` is added.
- With the buffer off, the cam track starts together with the recording, so the cam begins a moment after the picture. That is the positive offset of 2.4, which is fine.

**Done when:** a 10-minute recording with a lock in the middle leaves `video.h264`, two cam segments, and a `meta.json` that describes both.

---

### Task 2: Finishing — cam MP4s and facts, at stop and after a crash

**Files:**
- Modify: `src-tauri/src/recorder.rs` `finalize` (550-633). After `muxer::encode` and before `remove_dir_all` (625), each segment becomes `originals/<id>/cam.<n>.mp4` with stage 2's `facecam::cam_mp4_arguments` (`-f h264 -r 30 -i cam.<n>.h264 -c:v copy`, spec 2.6 item 1), and `facecam.json` is written last with `write_facts`. A failed segment is logged and left out, and the recording stays clean. `count_frames` (521-540) runs per segment for crash recovery.
- Modify: `src-tauri/src/state.rs` (`finish_recording` 1171-1201 only builds the `Clip`; the row is inserted by its callers): with facts written and the default layout visible, the `Clip` literal (1180-1200) gets `facecam: Some(ClipFacecam { layout, portrait, delay_ms: saved_delay_ms(..), state: Pending })`. With the default layout hidden the segments are dropped and the literal keeps `facecam: None`: without a clean copy nothing could show the cam later.
- Modify: `src-tauri/src/lib.rs`: `composite::enqueue` after `library.insert` in `stop_recording_and_notify` (242-246) and in `recover_recordings` (282-286), when the clip's state is `pending`; `guard_recording_room` 920-965 and `refresh_free_bytes` 898 count the cam bytes and the room the composite needs on the clip drive.
- Modify: `src-tauri/src/facecam.rs` and `src-tauri/src/composite.rs` (stage 2 Tasks 12 and 16), so they accept a segment store for a row with `recording: true`. Stage 2's `read_facts` returns `None` without `cam.mp4`, the worker's step 1 then sets the column to `None`, and `recover` runs per row on the same facts. So `read_facts` also accepts facts whose `segments` are all on disk as `cam.<n>.mp4`, and the worker's step 1 and `recover` take such a store only for a row with `recording: true`, and a `cam.mp4` store only for one without.
- Modify: `src-tauri/src/composite.rs`, `src/components/ClipPlayer.tsx`, `src/components/ClipEditor.tsx` (stage 2 Tasks 16 and 18). Recordings are left out of stage 2's "failed → Save adds the cam" rule: there is no clean copy for the editor to composite from. A failed recording keeps its clean file; "Try adding the cam again" from the clip menu (Task 4) re-runs the burn-in from the segments. So `ClipPlayer`'s `dirty` on `failed` and the editor line "The cam is not in the file yet — Save adds it." (stage 2 Task 18) skip a clip with `recording: true`, and the worker's failure notice for a recording reads "The cam couldn't be added to the recording — try again from its menu." in place of "… try again from the editor" (design call).

**Interfaces:**
- `FacecamFacts` gets `#[serde(default)] segments: Vec<CamSegmentFacts>`, `CamSegmentFacts { file: String, offset_us: i64, frames: u32, gaps_ms: Vec<(u64, u64)> }`. Empty for a clip, whose one cam stays in stage 2's `cam.mp4`, `cam_offset_us` and `gaps_ms`. A recording's `game_gop` comes from the fitted settings it was recorded with, as a clip's does.

**Tests:**
- `a_crashed_recording_still_gets_its_cam` (meta with cam and raw files, nothing counted yet)
- `a_broken_cam_segment_never_costs_the_recording`
- `the_work_folder_stays_until_the_cam_is_secured`
- `the_disk_guard_counts_the_cam_and_the_composite`
- `segment_offsets_are_in_recording_coordinates` (the offset of segment n = `(origin_n + first_pts_n·10 − start_100ns) / 10`, stage 2's formula with the recording's start as G0)
- `a_recording_saved_with_the_cam_hidden_keeps_no_cam_files`
- `a_recording_store_of_segments_reads_as_complete` (and a clip row with only segments does not)
- `a_failed_recording_is_not_offered_save_to_add_the_cam`

**Risks:**
- A recording's store has `cam.<n>.mp4` in place of `cam.mp4`. This task teaches `read_facts`, the worker and `recover` to accept it; the edit paths must still see such a store as "nothing to move" (Task 4).
- Review REC-1: a sleep already ruins a recording's own A/V sync. The cam offsets are measured against the video's start, so after a sleep they sit on a timeline that is itself wrong. That is why REC-1 is fixed first (Global Constraints).
- REC-3 (disk numbers past 47 minutes), REC-12 (a recovery racing a stop: the same guard must cover the cam files), and REC-V1 (the clip folder changed mid-recording).

**Done when:** a stopped and a crashed recording both end as a clean recording in the clip folder plus the cam segments and the facts in `originals/<id>/`, with the job queued.

---

### Task 3: Compositing recordings in the background

**Files:**
- Modify: `src-tauri/src/composite.rs` (stage 2's worker)
  - `Queue` gets a second lane, clips first; a running recording job is not interrupted by a clip (design call);
  - progress for long jobs through `run_command_with_progress` (the gallery's "Adding cam" pill gets a percentage);
  - deleting a recording whose composite runs stops that ffmpeg (stage 2's `cancel` only drops a queued job and waits for a running one, which here can take many minutes) (design call).
- Modify: `cam_filter`/`composite_arguments` (stage 2): a list of segments, one `CamChain` per segment chained on the game's timeline, each `overlay` with `eof_action=pass` instead of `repeat`, one mask for all (`ffmpeg-compositing.md` §2.e).

**Interfaces:**
- `pub enum JobKind { Clip, Recording }`
- `pub struct CamSegmentInput { pub path: PathBuf, pub timing: CamTiming, pub gaps_ms: Vec<(u64, u64)> }` (`timing` from stage 2's `cam_timing` with the segment's offset); `CamChain` gets an output label and the `eof_action`, so stage 2's single-cam graph stays as it is.

**Tests:**
- `clip_jobs_go_before_recording_jobs`
- `every_segment_gets_its_own_overlay`, `segments_let_the_game_through_after_their_end` (`eof_action=pass`)
- `a_deleted_recording_stops_its_composite`
- `a_quit_mid_composite_requeues_the_recording` (stage 2's `recover`/`after_crash`)

**Risks:**
- An hour at 1440p60 takes roughly 10–25 minutes at NVENC `p3` (`ffmpeg-compositing.md` §3.2, scaled), and longer on the x264 fallback, while the user plays.
- Three NVENC sessions at once (buffer, cam, composite).
- The file in the gallery has no cam for minutes; that is the price of 2.6, more visible here. An editor save of the recording meanwhile gets stage 2's "The cam is still being added to this clip — try again in a moment."

**Done when:** a 30-minute recording gets its cam at below-normal priority, and a clip saved in the meantime is composited first.

---

### Task 4: Burned in at stop (Open for Fabi 12)

**Files:**
- Modify: `src-tauri/src/composite.rs` (the recording branch of the job): no `edit::move_across` of the clean file into the store; `muxer::replace_file` swaps the composite over the clean recording in the clip folder, then the segments and `facecam.json` go and the state becomes `Baked`. On failure the clean recording stays, the state is `Failed`, and the segments stay so a retry can requeue the job (`composite::enqueue`, not `edit::run`, so `edit.rs` never needs segments).
- Modify: `src-tauri/src/edit.rs`:
  - a store with segments counts as "no cam to move" for `CamPart` and stage 3's `cam_part`;
  - stage 2's `bake` removes only `video.mp4`, `cam.mp4`, `facecam.json` and the trim note, so it must also remove the `cam.<n>.mp4` segments, or *Free up space* on a failed recording leaves them behind. `bake_clip_cam` then clears the column, because the file never had the cam.
- Modify: the UI entries: stage 2 offers "Free up space…" for a facecam clip only while its state is `ready` (`clipMenu.tsx`, `onBakeCam`; the editor button of stage 3 likewise). A failed recording needs an entry of its own in the gallery menu and the editor ("Try adding the cam again", which requeues, and "Free up space…", which drops the segments), and `BakeCams` counts the segments.

**If Fabi picks (b):** the clean recording is filed into `originals/<id>/video.mp4` like a clip's (stage 2's worker step 6 unchanged), the state is `ready`, and stage 3's `edit.rs` composite learns segments. A 10 GB recording on D: is then copied to `%APPDATA%` (Roaming, usually C:) under `clip_lock`, which takes minutes and can fill the system drive, and the storage row shows recordings on their own line.

**Tests:**
- `a_finished_recording_keeps_no_clean_copy`
- `the_clean_recording_stays_until_the_composite_is_in_place`
- `a_recording_without_room_for_the_composite_stays_clean` (stage 2's `room_for_composite`)
- `a_failed_recording_can_be_retried`
- `baking_a_failed_recording_removes_its_segments`
- `a_recording_store_never_reaches_the_edit_composite`

**Risks:** the crash rules of stage 2 (`applying` with the temp's size, `FinishSwap`/`Requeue`) must hold for the recording branch too; `PutCleanBack` never applies, because the clean file never leaves the clip folder.

**Done when:** a recording ends as one file with the cam in it, nothing of it is left in `originals/<id>/`, and a failed composite leaves the clean recording.

---

### Task 5: Screenshot — the shot's time and the cam frame

**Files:**
- Modify: `src-tauri/src/shot.rs` `grab` (523-560): return the frame's QPC, which is dropped today (541-543). Optionally the cheap fix for review REC-10 (an `AtomicBool` before `read_back`) on the same lines.
- Modify: `src-tauri/src/state.rs` `take_screenshot`: read `self.camera.latest_frame()` once, after the grab, never inside a WGC callback. A frame too far from the shot's QPC (the camera stalled) counts as none. `latest_frame` is `None` while the cam track is stopped (stage 2 Task 11 clears it on stop; `no_cam_while_the_cam_track_is_stopped` checks it).

**Interfaces:**
- `pub fn grab(...) -> Result<(Shot, i64), String>`, or `Shot.qpc_100ns: Option<i64>`
- `pub fn cam_still(frame: &camera::Frame, shot_qpc_100ns: i64, limit_100ns: i64) -> Option<CamStill>`, with `CamStill { width, height, rgba: Vec<u8> }`: cover-cropped to 16:9 with `camera::cover_crop`, converted like stage 2's `camtrack::preview_bgra` (at full size; a filtered version only if point sampling shows).

**Tests:**
- `the_shot_keeps_its_time`
- `a_cam_frame_far_from_the_shot_is_not_used`
- `no_cam_while_the_cam_track_is_stopped`
- `the_still_is_cropped_to_fill_16_9`

**Risks:** WGC callbacks are serialised by a mutex (`wgc.rs:249`), so the cam is never looked up inside one. HDR desktops come in as BGRA8, so colours can differ next to the cam; how much is not verified yet.

**Done when:** a screenshot knows its QPC and gets the cam frame of that moment while the buffer or a recording runs, and none otherwise.

---

### Task 6: Screenshot — composite, keep the clean picture, re-edit

**Files:**
- Modify: `src-tauri/src/state.rs` `take_screenshot` (947-1028), when there is a cam still and `in_screenshots` is on:
  - keep the shot without the cam as `originals/<id>/image.png` right away (today `keep_original` writes it at the first edit, `commands.rs:655-680`), plus `cam.png` and the layout in the shot's note;
  - lay the cam at the default layout before `watermark::stamp` (972), so the mark stays on top. `under.png` keeps describing the picture without the cam.
- Modify: `src-tauri/src/shot.rs`
  - `Shot::overlay(piece, at, mask)` next to `paste`/`blend` (83-121);
  - `impl From<camera::Rect> for shot::Rect` (`w` → `width`, `h` → `height`);
  - `Edit` (323) gets `#[serde(default)] cam: Option<CamLayout>` (`visible` is inside the layout);
  - `forget` (443-448) keeps `cam.png` and `image.png` while a cam exists.
- Modify: `src-tauri/src/commands.rs` `edit_screenshot` (545-653)
  - after `under` is pasted back (576-579), lay the cam with the edit's layout, then the steps, the crop, and `stamp_kept` (619-621);
  - the "nothing left to do" branch (565-571) lays the cam instead of writing `image.png` back;
  - `ShotEdit` (462-478) and `screenshot_edit` (481-530) get the cam's path and layout.
- Modify: `src/components/ShotViewer.tsx` (the cam box as `RectLayer mode="box"` from stage 3; *Show cam*, shape, mirror; the CSS preview over the stage, with the cam rectangle sent by the core in `ShotEdit`, the way `underRect`/`underPath` are (`ShotViewer.tsx:533`), and not computed again in TS the way the mark is with `markRect` (543, `src/lib/watermark.ts:27-57`))

**Interfaces:**
- `impl Shot { pub fn overlay(&mut self, piece: &Shot, at: shot::Rect, mask: Option<&[u8]>) }`: `piece` is the cam still scaled to `at.width × at.height`, `at` is `shot::Rect { x, y, width, height }` (`shot.rs:334-339`), the shot's own type like `paste` takes.
- The place is stage 2's `composite::cam_rect`, which returns `camera::Rect { x, y, w, h }`; it is converted with the `From` above before it reaches `overlay` or `ShotEdit`. The circle crop is `cam_crop`, the mask `composite::mask_gray8` at `at`'s size.
- Design call: the cam is laid on the uncropped picture, like the marks, so a crop can cut it.

**Tests:**
- `the_mark_stays_on_top_of_the_cam`
- `moving_the_cam_on_a_screenshot_leaves_no_old_cam`
- `hiding_the_cam_gives_the_shot_without_it`
- `a_screenshot_without_a_cam_is_written_as_before`
- `forget_keeps_the_cam_files`
- `under_still_describes_the_picture_without_the_cam`

**Risks:**
- 4K RGB copies of 25 MB each on the hotkey thread.
- Review REC-9: two shots in one millisecond now also share `originals/<id>/` names. The ids differ, so only the PNG path collides, as today.
- The store folder is shared with the video stores; never call `edit::remove` or `edit::remove_everything` on a screenshot's folder while its cam files are needed (stage 2's `remove` keeps only folders with a `facecam.json`).

**Done when:** a screenshot taken while the buffer runs shows the cam bottom right, under the mark, and the viewer can move, reshape and hide it without a trace of the old cam.

---

### Task 7: Settings, lifecycle, gallery

**Files:**
- Modify: `src/routes/settings/Facecam.tsx` (stage 2): *Where it appears* gets *Recordings*, hint "The cam is burned in when the recording stops.", and *Screenshots*, hint "Screenshots get the cam while the buffer or a recording runs." (5.2 item 3).
- Modify: `src-tauri/src/camtrack.rs`: `CamWants` gets `recording: bool` and `in_recordings: bool`, and `cam_wanted` adds `recording && in_recordings` (spec 2.2); `stop_delay` closes at once when a recording ends and nothing else wants the cam. `AppState::cam_plan` fills them in and `reconcile_cam` runs on recording start and stop. The lock-screen rule holds during a recording too (Open for Fabi 8): the cam stops, and the segment ends.
- Modify: the gallery and recordings list (`src/routes/Clips.tsx`): "Adding cam" with progress for recordings.

**Tests:**
- `a_recording_with_in_recordings_off_has_no_cam`
- `the_cam_stops_behind_the_lock_screen_during_a_recording` (over `cam_wanted` with `recording` and `locked`, in the style of stage 2's `the_cam_runs_only_while_something_the_user_switched_on_needs_it`)
- `a_screenshot_with_in_screenshots_off_is_written_as_before`

**Done when:** both switches do what they say, and nothing new happens while they are off.

---

### Task 8: Ship check (manual, on the 26200 machine)

- a 5-minute recording, in bright and in dim light;
- a recording with a lock and an unlock in the middle;
- a recording across a sleep (REC-1 is fixed by then: picture, sound and cam stay in sync after the wake);
- the app killed mid-recording, then the recovery with the cam;
- the app quit while a recording's cam is being added, then the restart;
- the drive nearly full;
- screenshots with the buffer on and off;
- a screenshot re-edited with crop, blur and a moved cam;
- the time a 30-minute recording's composite takes with a game running.

**Done when:** the results are noted in the spec next to section 2.10.
