# Facecam Stage 4, expanded: the camera in recordings and screenshots

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> Expands `2026-10-22-facecam-stage-4-recordings-and-screenshots.md` with Fabi's
> decisions of 2026-10-29: recordings **and screenshots** get the camera burned
> in (Open for Fabi 12 (a), and (a) for screenshots too — no clean copy, no
> re-edit), each switchable ("In recordings", "In screenshots"); the camera
> stops behind the lock screen during a recording (Open for Fabi 8, the
> recommendation). The prerequisites the rough plan names are on main already:
> AUD-6 (5b12244), REC-1's `skip_window`, BUF-3's split (71f8bfd).

**Goal:** A recording with the camera on ends as one file with the camera
burned in at the default layout; a screenshot taken while the camera runs has
it burned in. Neither ever costs the recording or the shot.

**Architecture:** the recording's camera lives in a module of its own,
`reccam.rs`, so the recorder (shipped, ~2000 lines of crash-safe writing) is
touched in two places only. A `RecordingCam` is a stage-2 `PacketTap` with its
own writer thread: it writes `cam.<n>.h264` into the recording's work folder,
one segment per camera session, and `cam.json` describing them, so a crashed
recording keeps its camera. At finalize, after the clean recording is muxed and
before the work folder goes, the segments become `originals/<id>/cam.<n>.mp4`
with their facts; a worker of its own (recordings never hold up clip
composites) burns them in at below-normal priority and swaps the result over
the clean recording; on failure the clean recording stays and can be retried.

**Tech Stack:** Rust (Tauri 2), ffmpeg 9.0.1, React + TypeScript.

**Spec:** `docs/superpowers/specs/2026-10-20-facecam-design.md` §2.10, 2.2, 2.3,
2.6, 5.1; Open for Fabi 8, 12.

## Global Constraints

- The camera never costs a recording: every camera failure leaves the clean
  recording; the work folder is removed only after the segments are secured.
- The encoder callbacks never wait: the tap only queues (`PacketTap::cam` runs
  under the camera ring's lock).
- The game ring and the camera ring are never locked together.
- Commits in English, no `Co-Authored-By`; UI labels without hint lines.
- One Rust build at a time; stop the dev app before editing Rust.

## Review Focus

1. A lock or sleep mid-recording restarts the camera session → a new segment;
   one file with a hole would shift every later frame →
   `a_new_session_starts_a_new_segment`, `every_segment_gets_its_own_overlay`.
2. A crash mid-recording → `recovered_segments_come_from_the_folder`.
3. The clean recording replaced only by a finished composite, never lost →
   `a_failed_burn_in_keeps_the_clean_recording` (state Failed, segments kept).
4. Offsets in recording time → `segment_offsets_are_in_recording_time`.
5. A recording with the camera off or "In recordings" off writes nothing new →
   `a_recording_without_the_camera_has_no_cam_files`.

---

### Task 1: Screenshots (done in 37ff6a1)

`composite::lay_cam_still` (cover crop, shape mask, mirror; tests
`the_cam_lands_in_a_screenshot_where_the_layout_puts_it`,
`a_rounded_cam_leaves_its_corners_to_the_picture`,
`a_mirrored_cam_is_flipped_in_a_screenshot`,
`a_stale_cam_frame_stays_out_of_a_screenshot`), wired in `take_screenshot`
before the watermark; "In screenshots" in the Camera tab.

### Task 2: `RecordingCam` — segments while recording

**Files:** Create `src-tauri/src/reccam.rs`; modify `src-tauri/src/camtrack.rs`
(`PacketTap::header(&self, session: u64, origin_100ns: i64, header: &[u8])`
with an empty default, called by `CamRing::set_header` for the current session
and by `attach_tap` before the backlog), `src-tauri/src/state.rs`
(`attach_recorder`: after the recorder is attached and outside `packets`,
`camera.attach_tap(cam, start_100ns)` when the camera is on and
`in_recordings`; `close_recording`: `detach_tap` first, then the cam's
`close()`).

**Interfaces:** `pub struct RecordingCam` with `pub fn start(dir: &Path,
start_100ns: i64) -> Arc<Self>`, `impl PacketTap`, `pub fn close(&self) ->
CamFile` (waits for its writer);
`#[derive(Serialize, Deserialize)] pub struct CamFile { pub segments:
Vec<Segment> }`, `pub struct Segment { pub file: String, pub session: u64,
pub offset_us: i64, pub frames: u64 }` — `offset_us = (origin_100ns +
first_pts_us·10 − start_100ns) / 10`; `cam.json` rewritten on every new segment
and at close.

- [ ] Failing tests: `a_new_session_starts_a_new_segment`,
  `segment_offsets_are_in_recording_time`, `the_backlog_starts_the_first_segment`,
  `cam_json_survives_a_reread` (pure: a `SegmentLog` that takes `header` and
  `cam` calls and yields `CamFile`; the writer thread only does I/O).
- [ ] Implement; wire; `wcargo test --lib` green.

### Task 3: Finishing — segments to the store

**Files:** `reccam.rs` (`pub fn secure(work_dir: &Path, clip_id: &str) ->
Option<FacecamFacts>`: each segment → `originals/<id>/cam.<n>.mp4` via
`facecam::cam_mp4_arguments`, facts with `segments`, written last; a segment
that fails is left out), `facecam.rs` (`FacecamFacts.segments:
Vec<CamSegmentFacts>` `#[serde(default)]`), `recorder.rs` `finalize` (calls
`secure` before the work folder goes), `state.rs` `finish_recording` (the row
gets `facecam: Some(.., state: Pending)` when facts exist), `lib.rs`
(`stop_recording_and_notify` and `recover_recordings` enqueue the burn-in).

- [ ] Failing tests: `recovered_segments_come_from_the_folder`,
  `a_recording_without_the_camera_has_no_cam_files`.
- [ ] Implement; green.

### Task 4: Burning in — a worker of its own

**Files:** `reccam.rs` (queue + thread; `pub fn enqueue(id)`, `recover()` on
start for rows `recording && Pending`), the graph:
`pub fn recording_arguments(clean: &Path, segments: &[SegmentInput], mask:
Option<&Path>, layout, frame, out: &Path, encoder, quality) -> Vec<String>` —
one overlay per segment chained, `eof_action=pass`, every audio stream copied
(`-map 0:a? -c:a copy`), video re-encoded at the recording's quality, `-t` of
the clean recording.

- [ ] Failing tests: `every_segment_gets_its_own_overlay`,
  `segments_let_the_game_through_after_their_end`,
  `all_audio_tracks_are_kept`.
- [ ] On success: replace the clean recording (retrying while it is open), drop
  the segments and facts, state `Baked`. On failure: state `Failed`, clean kept,
  segments kept; the gallery menu's "Try adding the camera again" requeues.
  Test `a_failed_burn_in_keeps_the_clean_recording` on the decision function.
- [ ] Green; commit.

### Task 5: Settings and lifecycle

"In recordings" in the Camera tab; `CamWants` gets `recording` and
`in_recordings` (`cam_wanted` adds `recording && in_recordings`); reconcile on
recording start and stop. Test `the_cam_runs_for_a_recording_when_asked`.

### Task 6: Ship check

Full suites; a real 1-minute recording with the camera (frames checked); one
with a lock in the middle; the app killed mid-recording, then recovery.
