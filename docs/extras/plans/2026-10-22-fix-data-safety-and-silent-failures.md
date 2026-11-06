# Data Safety and Silent Failures Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. The manual checks need a person at a Windows machine; everything else an agent can do.

**Goal:** Stop ClippiBoy from losing settings, clips and recordings, and from failing without a word, in the cases `docs/facecam-review.md` confirmed.

**Architecture:** Each fix sits where its bug is and adds no Tauri command, window or crate. What decides something goes into a small pure function with a sentence-named test (the pattern of `commands.rs:1514-1536`); the wiring around it is checked by hand on Windows. One test-only module (`grants.rs`) reads the frontend's imports and the capability files, so a missing grant fails `cargo test` instead of a click.

**Tech Stack:** Tauri 2 / Rust (`cargo test --lib`, `windows` 0.58, `parking_lot`), React + TypeScript (`tsc`).

**Spec:** `docs/facecam-review.md` (IDs, scenarios and fixes; line numbers as of 401a320, which the source files still match). Where an entry names a refined fix ("V-" verifications folded in), the plan follows the refined one.

## Global Constraints

- Commits in English, **without** a `Co-Authored-By` line. One commit per task, in the form "Area: what the user gets — why".
- `cargo test --lib` and `npm run build` via PowerShell on the Windows side (most of the core only compiles there); `npx tsc --noEmit` works from WSL.
- Decisions live in pure functions with sentence-named tests, like `commands.rs:1514-1536`. File tests use `std::env::temp_dir()` plus a `uuid` name, as `telemetry.rs:327-330` does. No new crates (no `regex`, no `tempfile`); `File::set_modified` and `std::fs::hard_link` are std.
- No new Tauri command. A changed capability file regenerates `src-tauri/gen/schemas/capabilities.json` at build; commit it with the change.
- No new lock is taken before `AppState.config`. Config saves happen *under* that lock, as the direct writers already do (a separate save mutex taken first would invert their order and can deadlock).
- Nothing new blocks the main thread: walks, retries and waits run on their own thread or in `async` commands.
- User-visible text in English, short, saying what happened and what to do. New code comments in English.
- Not in this plan: BUF-2, BUF-4, BUF-5, CAP-V2, CFG-1 and EDIT-1 are prerequisites in the facecam stage-2 plan; BUF-1, BUF-6, REC-1, SAVE-4, SAVE-7, AUD-2, AUD-6, AUD-V2 and CAP-4 are in the extra plan `2026-10-22-fix-sync-and-timestamps.md`; UI-n items stay with `docs/extras/review-beyond-facecam.md`.
- Added beyond the brief, because they lose data or fail silently and sit in code touched anyway: SAVE-8 (Task 7), SAVE-9 (Task 10), EDIT-15 (Task 9), REC-7 (Task 5). CAP-9 is recovered by Task 1's watchdog, not fixed at its root.

## Review Focus

- A clip folder on a NAS or USB stick that is asleep or gone: the start-up sweep, delete retries and `set_clip_dir` must not hang the window or the start → Task 7 test `a_clip_folder_that_is_not_there_has_no_leftovers`; all three run off the main thread.
- A file held for a moment by antivirus, an indexer or cloud sync (`config.json` at autostart, a clip during delete, `meta.json` at stop): keep it and say so, never replace it with defaults or drop its row → Task 8 `a_read_error_is_not_a_first_start`, Task 9 `a_file_that_will_not_go_is_reported`, Task 4 `a_description_that_cannot_be_written_still_closes_the_recording`.
- An encoder that fails again right after every restart: a bounded number of restarts, then one clear message, and no old footage saved as new → Task 1 `restarts_stop_after_three_in_ten_minutes`.
- Files and stores left by older versions (scratch names with the game in front, `spuren.json` stores): recognised exactly as before → Task 7 `old_and_new_scratch_names_are_both_recognised`, `only_incomplete_stores_without_a_clip_are_orphans`.
- Long or non-Latin game names from window titles: file names stay at 60 characters, counted as characters, and saves keep working → Task 7 `a_long_window_title_makes_a_short_file_name`.

---

### Task 1: Encoder failures restart the buffer instead of freezing it (CAP-1, CAP-2)

**Files:**
- Modify: `src-tauri/src/mft.rs:99-113` (`EncoderStats`), `:1165-1201` (`submit`), `:1220-1390` (`run`)
- Modify: `src-tauri/src/pipeline.rs:542-555` (`Shared::encoder_failure` beside `report`)
- Modify: `src-tauri/src/state.rs` (`encoder_failure`, `RestartBudget`, refusal in `save_clip` after `:872-874`)
- Modify: `src-tauri/src/lib.rs:594-651` (new `watch_encoder` beside `watch_screen`), `:1065` (call it)
- Test: `mft.rs` tests (`:1448`), `state.rs` tests (`:1321`)

**Interfaces:**
- Produces:
  - `EncoderStats` gains `failure: parking_lot::Mutex<Option<String>>` and `submit_timeouts: AtomicU32`; `fn fail(&self, why: String)` (first one wins, one `log::error!`), `fn failure(&self) -> Option<String>`.
  - `mft::ErrorStreak` (`Default`) with `fn failed(&mut self) -> bool` (true exactly once, on the `FAIL_AFTER`th error in a row) and `fn succeeded(&mut self)`; `const FAIL_AFTER: u32 = 30`.
  - `mft::age_out(sent: &mut VecDeque<Instant>, now: Instant, limit: Duration) -> usize` (entries removed); `const IN_FLIGHT_LIMIT: Duration = 2 s`; `const SUBMIT_TIMEOUTS_FAIL: u32 = 10`.
  - `pipeline::Shared::encoder_failure(&self) -> Option<String>` (`None` off Windows); `AppState::encoder_failure(&self) -> Option<String>`.
  - `state::RestartBudget` (`Default`) with `fn allow(&mut self, now: Instant) -> bool`: at most 3 restarts in any 600 s; refused attempts are not counted.

- [ ] **Step 1: Write the failing tests**

```rust
// mft.rs tests
#[test]
fn a_run_of_encoder_errors_is_reported_once() {
    let mut streak = ErrorStreak::default();
    for _ in 1..FAIL_AFTER {
        assert!(!streak.failed());
    }
    assert!(streak.failed(), "the 30th error in a row is the one");
    assert!(!streak.failed(), "and it is reported once");
    streak.succeeded();
    for _ in 1..FAIL_AFTER {
        assert!(!streak.failed(), "a packet in between starts the count over");
    }
}

#[test]
fn frames_the_encoder_never_hands_back_age_out() {
    let now = Instant::now();
    let mut sent = VecDeque::from([
        now - Duration::from_secs(3),
        now - Duration::from_millis(2_500),
        now - Duration::from_millis(100),
    ]);
    assert_eq!(age_out(&mut sent, now, IN_FLIGHT_LIMIT), 2);
    assert_eq!(sent.len(), 1);
}

#[test]
fn the_first_encoder_failure_is_the_one_kept() {
    let stats = EncoderStats::default();
    assert_eq!(stats.failure(), None);
    stats.fail("the encoder stopped on its own".into());
    stats.fail("something later".into());
    assert_eq!(stats.failure().as_deref(), Some("the encoder stopped on its own"));
}

// state.rs tests
#[test]
fn restarts_stop_after_three_in_ten_minutes() {
    let mut budget = RestartBudget::default();
    let start = std::time::Instant::now();
    let at = |s: u64| start + std::time::Duration::from_secs(s);
    assert!(budget.allow(at(0)));
    assert!(budget.allow(at(60)));
    assert!(budget.allow(at(120)));
    assert!(!budget.allow(at(180)), "a fourth within ten minutes");
    assert!(budget.allow(at(601)), "the first one has aged out");
}
```

- [ ] **Step 2: Run** `cargo test --lib -- mft::tests state::tests` (PowerShell, `src-tauri`). Expected: FAIL to compile, `ErrorStreak`, `age_out`, `RestartBudget` not found.

- [ ] **Step 3: Implement the stats, `ErrorStreak` and `age_out` in `mft.rs`** with the interfaces above.

- [ ] **Step 4: Wire them into the encoder.**
  - `run`, both paths: every `feed`/`take_output` `Err` keeps its warn line and calls `streak.failed()`; when that returns true, `stats.fail(format!("the encoder keeps failing: {err}"))`. `Ok(Some(packet))` and a successful `feed` call `succeeded()`.
  - Hardware path, once per loop turn before `if pending_input > 0`: `age_out(&mut sent, Instant::now(), IN_FLIGHT_LIMIT)`; when it removed any, one warn line and `stats.in_flight.store(sent.len())`. That ends the permanent starve of CAP-2 within 2 s. (`IMFTrackedSample` is the exact answer and stays a facecam design item.)
  - After the `match transform.events.clone()` block, before the drain at `:1391`: `if running.load(Relaxed) { stats.fail("the encoder stopped on its own".into()) }`.
  - `submit`: `Disconnected` → `if self.running.load(Relaxed) { self.stats.fail("the encoder thread has ended".into()) }`, and its warn line only while `failure()` was still `None`. Slow-path `Err` → `submit_timeouts.fetch_add(1) + 1 >= SUBMIT_TIMEOUTS_FAIL` fails with `"the encoder has taken no frame for 5 s"` (10 × `SUBMIT_WAIT`, which also catches CAP-9's lost `NeedInput`); slow-path `Ok` stores 0; the fast path resets only `if submit_timeouts.load(Relaxed) != 0`, so a frame that goes straight through still pays for no write.

- [ ] **Step 5: Use it.**
  - `Shared::encoder_failure` reads `encoder_stats`; `AppState::encoder_failure` asks the registered `shared`.
  - `AppState::save_clip`, right after the tools gate: `if let Some(why) = self.encoder_failure() { return Err(format!("The video encoder stopped ({why}), so the buffer has nothing newer to save.")) }` — the worst effect of CAP-1 was a "save the last 30 s" that wrote footage from before the failure.
  - `lib.rs` `fn watch_encoder(app: &tauri::AppHandle)`, called right after `watch_screen(&handle)`, shaped like `watch_screen`: skip on the secure desktop; own `static RESTARTING: AtomicBool` and `static BUDGET: Mutex<RestartBudget>`. If the budget refuses: once per session, error sound plus banner `"Video encoder keeps failing"` / `"Pick another encoder under Settings → Recording, or restart ClippiBoy."` and a notice; no further restarts (saves keep refusing with the message above). Otherwise on a thread: a running recording is stopped first with `stop_recording_and_notify` (what was written is kept); then, if `is_buffering()`, `stop_pipeline()` + `start_pipeline()` with banner `"Recording restarted"` / `"The video encoder stopped ({why}) — the buffer starts over"`, and the `Err` arm of `watch_screen` on failure.
  - Not through `shared.report`: until BUF-2 lands nothing reported after the first second is shown, and afterwards it would be a second banner for the same event.

- [ ] **Step 6: Run** `cargo test --lib` (PowerShell). Expected: all green, four tests more.

- [ ] **Step 7: Manual check (Windows, dev build).** Buffer running; Device Manager → display adapter → Disable, then Enable (or `pnputil /restart-device "<instance id>"`). Expected within ~10 s: "Recording restarted" (a "Recording disrupted" banner may come first if the converter failed too), and a clip saved a minute later shows footage from after the restart. The log has one `error` line naming the failure, not a warn line per frame.

- [ ] **Step 8: Commit** `Encoder: a failing or stalled encoder is noticed — 30 errors in a row, no frame taken for 5 s, or its thread ending — and the buffer restarts, at most three times in ten minutes; saving refuses the old footage meanwhile, and frames the encoder never hands back age out after 2 s instead of starving the capture for good`

---

### Task 2: A slow audio device no longer stays open behind the app's back (AUD-4)

**Files:**
- Modify: `src-tauri/src/audio/capture.rs:105-140` (`start`), `:418` (activation wait), `:602-604` (`ready.send`)
- Test: `audio/capture.rs` tests (`:701`)

**Interfaces:**
- Produces: `fn await_ready(waiting: &std::sync::mpsc::Receiver<Result<(), String>>, timeout: Duration, stop: &AtomicBool) -> Result<(), String>` — sets `stop` on a timeout and on a reported error; `const START_WAIT: Duration = 5 s`; `const ACTIVATION_WAIT_MS: u32 = 4_000`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_start_that_times_out_tells_its_thread_to_stop() {
    let (_ready, waiting) = std::sync::mpsc::channel::<Result<(), String>>();
    let stop = AtomicBool::new(false);
    assert!(await_ready(&waiting, std::time::Duration::from_millis(20), &stop).is_err());
    assert!(stop.load(Ordering::Relaxed), "left unset, the thread pumps on with the device open");
}

#[test]
fn a_start_that_answers_in_time_keeps_its_thread() {
    let (ready, waiting) = std::sync::mpsc::channel::<Result<(), String>>();
    ready.send(Ok(())).unwrap();
    let stop = AtomicBool::new(false);
    assert!(await_ready(&waiting, std::time::Duration::from_millis(20), &stop).is_ok());
    assert!(!stop.load(Ordering::Relaxed));
}
```

- [ ] **Step 2: Run** `cargo test --lib audio::capture` (PowerShell). Expected: FAIL to compile, `await_ready` not found.

- [ ] **Step 3: Implement.** `start` waits through `await_ready(&ready_rx, START_WAIT, &stop)` (enough on its own: `attempt` checks `stop` right after `Start`, `:613`). In `attempt`, `if ready.send(Ok(())).is_err() { return Attempt::Stopped; }` — the caller has given up. The process-tap activation waits `ACTIVATION_WAIT_MS`, inside the caller's 5 s.

- [ ] **Step 4: Run** `cargo test --lib` (PowerShell). Expected: green.

- [ ] **Step 5: Manual check (Windows).** Add a Bluetooth headset mic that is slow to switch to the hands-free profile (or any device that hits "timed out starting the audio source" in the log), then remove the source. Expected: the microphone indicator in the taskbar goes out within a second; Process Explorer shows no second `clippiboy-audio` thread left for it after the 10 s retry.

- [ ] **Step 6: Commit** `Audio: a source that takes longer than 5 s to open is told to stop instead of pumping on unseen with the device open — the mic indicator went out only when the app did; the process-tap activation now waits 4 s, inside the start's 5`

---

### Task 3: Exports are fresh, never touch the clip, and work on a share (CFG-3, CFG-4, CFG-10)

**Files:**
- Modify: `src-tauri/src/disk.rs` (`same_file`, module doc one line)
- Modify: `src-tauri/src/commands.rs:314-325` (`export_target`), `:928-962` (`export_for_discord`), `:1151-1217` (`shrink`), `:1227-1264` (`export_clip` → `export_to`), `:741-779` (`delete_clip`), `:1383-1430` (`store`)
- Test: `disk.rs` tests, `commands.rs` tests (`:1514`)

**Interfaces:**
- Produces:
  - `disk::same_file(a: &Path, b: &Path) -> bool` — Windows: volume serial plus file index from `GetFileInformationByHandle` (`Win32_Storage_FileSystem` is already enabled); Unix: `dev` plus `ino`; `false` when either cannot be opened.
  - `fn export_to(state: &State<'_, AppState>, app: &tauri::AppHandle, id: &str, target_bytes: u64, output: &Path) -> Result<()>` — the tail of today's `export_clip` (shrink, `progress(1.0)`, notice).
  - `fn is_discord_export(clip_stem: &str, file_name: &str) -> bool` (`"{stem} ({digits} MB).mp4"` exactly) and `fn remove_discord_exports(clip_path: &Path)`.
  - `shrink` writes `<target stem>.<pid>.export.mp4` beside the target and renames it in; Task 7's sweep matches `*.<digits>.export.mp4`.

- [ ] **Step 1: Write the failing tests**

```rust
// disk.rs tests
#[test]
fn the_same_file_is_recognised_under_another_spelling() {
    let dir = std::env::temp_dir().join(format!("clippiboy-same-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let clip = dir.join("Clip.mp4");
    let other = dir.join("Other.mp4");
    std::fs::write(&clip, b"x").unwrap();
    std::fs::write(&other, b"x").unwrap();
    let linked = dir.join("linked.mp4");
    std::fs::hard_link(&clip, &linked).unwrap();
    assert!(same_file(&clip, &dir.join(".").join("Clip.mp4")));
    assert!(same_file(&clip, &linked), "a hard link is the same file");
    if cfg!(windows) {
        assert!(same_file(&clip, &dir.join("clip.MP4")), "another case");
    }
    assert!(!same_file(&clip, &other));
    assert!(!same_file(&clip, &dir.join("missing.mp4")));
    let _ = std::fs::remove_dir_all(&dir);
}

// commands.rs tests
#[test]
fn a_discord_export_is_known_by_its_name() {
    let stem = "Valorant_2026-10-22_12-00-00-123";
    assert!(is_discord_export(stem, "Valorant_2026-10-22_12-00-00-123 (10 MB).mp4"));
    assert!(is_discord_export(stem, "Valorant_2026-10-22_12-00-00-123 (500 MB).mp4"));
    assert!(!is_discord_export(stem, "Valorant_2026-10-22_12-00-00-123.mp4"));
    assert!(!is_discord_export(stem, "Valorant_2026-10-22_12-00-00-123 (ten MB).mp4"));
    assert!(!is_discord_export(stem, "Valorant_2026-10-22_12-00-00-124 (10 MB).mp4"));
    assert!(!is_discord_export(stem, "Valorant_2026-10-22_12-00-00-123 (10 MB).4711.export.mp4"));
}
```
  Add to `an_export_goes_only_to_a_local_mp4`:
```rust
let own = crate::config::data_dir().join("originals").join("x").join("video.mp4");
assert!(export_target(&own.to_string_lossy()).is_err(), "ClippiBoy's own data");
```

- [ ] **Step 2: Run** `cargo test --lib -- disk::tests commands::tests` (PowerShell). Expected: FAIL to compile, `same_file` and `is_discord_export` not found.

- [ ] **Step 3: Implement.**
  - `same_file` as above.
  - `shrink`: refuse when `output.exists() && same_file(&output, &source)` (same message as today). ffmpeg writes the `.export.mp4` beside the target; all three cleanups remove that file, never `output`; on success `muxer::replace_file(&part, &output)`, removing `part` on failure. A miss of the identity check now ends in "replaced", not "destroyed", and a cut-short export never carries the final name.
  - `export_clip` = `export_target` + `export_to`; `export_for_discord` calls `export_to` directly (CFG-4: its sibling of a UNC clip no longer meets the window-path check). It reuses an existing file only when `len > 0 && len <= target && stems::is_newer(&output, &source)`.
  - `export_target` also refuses a path inside `config::data_dir()`, compared case-insensitively: `"Export somewhere outside ClippiBoy's own data folder."`.
  - `remove_discord_exports(Path::new(&clip.path))` in `delete_clip` before `filing::prune` (or the game folder cannot go), and in `store()` after the library writes succeeded (covers edits and undo, whose copy keeps the original's old mtime).

- [ ] **Step 4: Run** `cargo test --lib` (PowerShell). Expected: green.

- [ ] **Step 5: Manual check (Windows).**
  1. Clip folder on `\\<this pc>\<share>`: "Export for Discord" from the console puts a file on the clipboard.
  2. Export a clip for Discord at 10 MB, trim 5 s off the front, export again: the pasted file is 5 s shorter.
  3. Export dialog: type the clip's own name in another case and confirm "Replace": "An export cannot overwrite the clip it comes from." and the clip still plays.

- [ ] **Step 6: Commit** `Export: "Export for Discord" makes a fresh file once the clip was edited, and works again with the clip folder on a network share; an export is written beside its target and renamed in when whole, refuses the clip itself under any spelling, hard link or junction, and stays out of ClippiBoy's data folder; deleting or editing a clip takes its Discord copies along`

---

### Task 4: A stop that cannot write its description still ends the recording (BUF-V1, REC-2)

**Files:**
- Modify: `src-tauri/src/recorder.rs:342-376` (`finish`), tests `:724-777` (drop the `.unwrap()`s)
- Modify: `src-tauri/src/state.rs:1115-1141` (`close_recording`)
- Test: `recorder.rs` tests (`:648`)

**Interfaces:**
- Produces: `Recorder::finish(&self, separate: Vec<String>, game: Option<String>) -> Meta` (was `Result<Meta, String>`); a failed `write_meta` is logged at error and the in-memory `Meta` returned, so `finalize` can still use it and the folder stays for `recover_recordings`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_description_that_cannot_be_written_still_closes_the_recording() {
    let dir = scratch("meta-fails");
    let meta = Meta { id: "m".into(), fps: 60, ..Meta::default() };
    let recorder = Recorder::start(&dir, meta, &[], Vec::new(), 0).unwrap();
    // A folder where the temp file goes: the write fails like a full or vanished drive.
    std::fs::create_dir_all(recorder.dir.join("meta.json.tmp")).unwrap();
    let meta = recorder.finish(Vec::new(), Some("Valorant".into()));
    assert_eq!(meta.frames, Some(0));
    assert_eq!(meta.game.as_deref(), Some("Valorant"));
    assert!(recorder.dir.join(VIDEO_FILE).is_file(), "the folder stays for the next start");
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: Run** `cargo test --lib recorder` (PowerShell). Expected: FAIL to compile (`finish` returns a `Result`, so `meta.frames` is not a field).

- [ ] **Step 3: Implement** the new `finish`; in `close_recording` drop the early `return None` at `:1131-1134`: once the recorder is taken, `halt_pipeline` (when the buffer is not wanted) and `status.recording = false` always run.

- [ ] **Step 4: Run** `cargo test --lib` (PowerShell). Expected: green.

- [ ] **Step 5: Manual check (Windows).** Clip folder on a USB stick; start a recording, pull the stick after 30 s, press Stop. Expected: the REC badge and the tray state go off, an error says the recording could not be written, and the capture stops (GPU encode load in Task Manager drops). Put the stick back and restart: the recording is finished from its folder.

- [ ] **Step 6: Commit** `Recording: a stop whose description cannot be written still ends the recording — capture, encoder and the REC state ran on forever when the clip drive vanished or something held meta.json; the folder stays for the next start`

---

### Task 5: The recording's disk guard counts what the stop writes; a slow drive stops it in time (REC-3, REC-V2, REC-7)

**Files:**
- Modify: `src-tauri/src/recorder.rs:155-178` (writer loop), `:215-219` (`send`), `:240-251` (next to `estimated_size`), `:551-628` (`finalize`), `:632-646` (next to `leftovers`)
- Modify: `src-tauri/src/state.rs` (`recording_room_needed` next to `status_snapshot` `:762`; `recover_recordings` `:1205-1228`)
- Modify: `src-tauri/src/lib.rs:920-966` (`guard_recording_room`)
- Test: `recorder.rs` tests

**Interfaces:**
- Produces:
  - `pub const MIX_BYTES_PER_SECOND: u64 = SAMPLE_RATE as u64 * FRAME_BYTES` (192 000); `pub fn mix_scratch_bytes(tracks_in_mix: usize, seconds: u64) -> u64` (0 below two tracks).
  - `Recorder::tracks_in_mix(&self, separate: &[String]) -> usize` — opened tracks whose source is not in `separate`.
  - `AppState::recording_room_needed(&self) -> Option<u64>` = `estimated_size(fps) + mix_scratch_bytes(tracks_in_mix(&separate_tracks(&sources)), frames / fps)`; clone the `Arc<Recorder>` and drop the `recorder` lock before reading the config.
  - `pub fn is_empty_leftover(dir: &Path, meta: &Meta) -> bool` — `meta.frames == Some(0)`, or `video.h264` missing or 0 bytes. Never decided by ffprobe, which may simply be absent (SAVE-V1).
  - `const WRITER_BACKLOG_LIMIT: u64 = 512 MiB`; `fn backlog_failure(queued_bytes: u64) -> Option<String>`; `Job::bytes(&self) -> u64` (video data, audio samples × 2, else 0); `Recorder.queued: Arc<AtomicU64>`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn stopping_needs_room_for_the_summed_mix_too() {
    assert_eq!(mix_scratch_bytes(0, 3600), 0);
    assert_eq!(mix_scratch_bytes(1, 3600), 0, "one track is used as it is");
    assert_eq!(mix_scratch_bytes(2, 3600), 691_200_000);
}

#[test]
fn the_tracks_that_feed_the_main_mix_are_counted() {
    let dir = scratch("in-mix");
    let meta = Meta { id: "m".into(), fps: 60, ..Meta::default() };
    let recorder = Recorder::start(&dir, meta, &[], Vec::new(), 0).unwrap();
    let (game, mic) = (ring_with("game"), ring_with("mic"));
    game.push(&[5i16; 12], 0);
    assert!(recorder.begin_window(&[game.clone(), mic.clone()], 1250));
    recorder.audio(&game, &[7i16; 12]);
    recorder.audio(&mic, &[9i16; 12]);
    recorder.end_window(6);
    assert_eq!(recorder.tracks_in_mix(&[]), 2);
    assert_eq!(recorder.tracks_in_mix(&["mic".to_string()]), 1);
    let _ = recorder.finish(Vec::new(), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_empty_leftover_is_recognised() {
    let dir = scratch("empty");
    std::fs::write(dir.join(VIDEO_FILE), b"").unwrap();
    assert!(is_empty_leftover(&dir, &Meta::default()));
    std::fs::write(dir.join(VIDEO_FILE), [0u8, 0, 0, 1, 0x65]).unwrap();
    assert!(!is_empty_leftover(&dir, &Meta::default()));
    assert!(is_empty_leftover(&dir, &Meta { frames: Some(0), ..Meta::default() }));
    std::fs::remove_file(dir.join(VIDEO_FILE)).unwrap();
    assert!(is_empty_leftover(&dir, &Meta::default()), "no video at all");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_writer_half_a_gigabyte_behind_stops_the_recording() {
    assert_eq!(backlog_failure(WRITER_BACKLOG_LIMIT), None);
    let why = backlog_failure(WRITER_BACKLOG_LIMIT + 1).expect("over the limit");
    assert!(why.contains("cannot keep up"), "{why}");
}
```

- [ ] **Step 2: Run** `cargo test --lib recorder` (PowerShell). Expected: FAIL to compile, the four functions not found.

- [ ] **Step 3: Implement.**
  - The functions above. `guard_recording_room`: `let Some(needed) = state.recording_room_needed() else { return };` replaces `status_snapshot().recording_bytes` (`:934`), so both thresholds include the mix. `estimated_size` stays what the UI shows.
  - `finalize`: when `sum_tracks` or `muxer::encode` fails, `remove_file(dir.join(MIX_FILE))` before returning the error.
  - `recover_recordings`: after `read_meta`, an `is_empty_leftover` folder is removed (`remove_dir_all`, then `remove_dir` of `.recording`) with an info line instead of being finalized.
  - REC-7: `send` adds `job.bytes()` to `queued`; when the new total gives `backlog_failure(..)` and `failure` is still `None`, set it — the 1 s tick then auto-stops the recording through the existing path (`lib.rs:1081-1089`). The writer subtracts after each job. `video()` still never waits.

- [ ] **Step 4: Run** `cargo test --lib` (PowerShell). Expected: green.

- [ ] **Step 5: Manual check (Windows).** Quit ClippiBoy; in the clip folder create `.recording\x\` with a copy of any `meta.json` and an empty `video.h264`; start. Expected: the folder is gone and the log says it was cleared. (The 47-minute case is pinned by the arithmetic test; staging it needs a nearly full drive.)

- [ ] **Step 6: Commit** `Recording: the disk guard counts the summed main mix the stop writes beside the recording (192 kB/s with two or more sources in it), so the auto-stop past ~47 min fits again, and a failed stop removes mix.pcm; a writer half a gigabyte behind a slow drive stops the recording instead of growing until the app aborts; an empty work folder from a crash in the first half second is cleared at start instead of retried forever`

---

### Task 6: The console asks about share links again and survives Alt+F4 (CON-1, CON-2)

**Files:**
- Create: `src-tauri/src/grants.rs` (tests only; `#[cfg(test)] mod grants;` in `lib.rs:1-40`)
- Modify: `src-tauri/capabilities/console.json:13-34` (`"allow-link-exists"`)
- Modify: `src-tauri/src/lib.rs:1313-1322` (`CloseRequested`)
- Modify: `src-tauri/src/console.rs:174-177` (`open` re-creates a missing window)
- Commit also: `src-tauri/gen/schemas/capabilities.json` (regenerated by the build)

**Interfaces:**
- Produces (test-only, in `grants.rs`):
  - `struct Wrapper { object: String, member: String, command: String }`
  - `fn wrappers(source: &str) -> Vec<Wrapper>` — for each `invoke` followed (after an optional `<…>`) by `("name"`, walk back over whitespace: `=>`, a parenthesised parameter list (match the parentheses), `:`, the member name; the object is the name after the nearest preceding `const `.
  - `fn bare_invokes(source: &str) -> Vec<String>` — `invoke("name"` hits that are not wrapper members (module-level calls like `share.ts:41`, `friends.ts:43`, which run in every window that imports the module).
  - `fn calls(source: &str, wrappers: &[Wrapper]) -> BTreeSet<String>` — first drop whitespace around every `.`, then a wrapper counts where `object.member(` occurs with no identifier character before `object`.
  - `fn reachable(entry: &Path) -> Vec<PathBuf>` — follow `from "…"` and `import "…"` specifiers that start with `@/` (→ `src/`) or `.`; try `.ts`, `.tsx`, `/index.ts`, `/index.tsx`; skip what does not resolve (packages, CSS).
  - `fn granted(capability: &str) -> BTreeSet<String>` — the `permissions` array, `allow-a-b` → `a_b`, everything else ignored.
  - `fn needed(entry: &str) -> BTreeSet<String>` — over `reachable(entry)`: `calls(file, wrappers of all reachable files)` plus `bare_invokes(file)`.
  - Paths: `src/` at `CARGO_MANIFEST_DIR/..`, capabilities at `CARGO_MANIFEST_DIR`; CI already runs `cargo test --lib` from `src-tauri` on a full checkout.

- [ ] **Step 1: Write the tests**

```rust
const WINDOWS: [(&str, &str, &str); 3] = [
    ("main", "src/main.tsx", "capabilities/default.json"),
    ("console", "src/console/main.tsx", "capabilities/console.json"),
    ("overlay", "src/overlay/main.tsx", "capabilities/overlay.json"),
];
/// Reachable by import but never called there: `installAccent` draws the
/// tray icon only with `turning: true`, which only the main window passes
/// (`src/lib/accent.ts:128-142`).
const NOT_CALLED: [(&str, &str); 2] = [("console", "set_logo_icon"), ("overlay", "set_logo_icon")];

#[test]
fn a_call_split_over_lines_is_still_seen() {
    let ipc = "export const api = {\n  linkExists: (id: string) => invoke<boolean>(\"link_exists\", { id }),\n  setHotkeys: (\n    a: string,\n  ) =>\n    invoke<AppConfig>(\"set_hotkeys\", { a }),\n};\n";
    let mut found = wrappers(ipc);
    assert!(found.iter().any(|w| w.object == "api" && w.member == "linkExists" && w.command == "link_exists"));
    assert!(found.iter().any(|w| w.member == "setHotkeys" && w.command == "set_hotkeys"));
    found.push(Wrapper { object: "api".into(), member: "send".into(), command: "not_this_one".into() });
    let page = "void api\n  .linkExists(clip.id)\n  .then(() => {});\nshareApi.send(a, b);";
    assert_eq!(calls(page, &found), BTreeSet::from(["link_exists".to_string()]));
}

#[test]
fn every_command_a_window_calls_is_granted_to_it() {
    for (window, entry, capability) in WINDOWS {
        let missing: Vec<String> = needed(entry)
            .difference(&granted(capability))
            .filter(|command| !NOT_CALLED.contains(&(window, command.as_str())))
            .cloned()
            .collect();
        assert!(missing.is_empty(), "{window} calls commands it is not granted: {missing:?}");
    }
}
```

- [ ] **Step 2: Run** `cargo test --lib grants` (PowerShell). Expected: FAIL with `console calls commands it is not granted: ["link_exists"]` and nothing for `main` or `overlay`. If another window fails, check whether the call really runs there before adding a `NOT_CALLED` entry, each with its reason.

- [ ] **Step 3: Implement.**
  - `"allow-link-exists"` in `console.json` (read-only).
  - `on_window_event` in `lib.rs`: `CloseRequested` for `console::LABEL` → `api.prevent_close()` and `console::close(window.app_handle())`; for `overlay::LABEL` → `api.prevent_close()`; `"main"` unchanged. Tray Quit goes through `app.exit(0)`, which raises no `CloseRequested`.
  - `console::open`: a missing window is created once more (`create(app)`, then look it up again) instead of only logging. Safe there: `open` runs from the hotkey inside `run_on_main_thread`, never from a sync command or a WebView2 event, where building a window can deadlock.

- [ ] **Step 4: Run** `cargo test --lib` and `npm run build` (PowerShell). Expected: green; `git status` shows `src-tauri/gen/schemas/capabilities.json` changed.

- [ ] **Step 5: Manual check (Windows).**
  1. Create a share link for a clip, then delete that clip from the console: "Delete with its link?" appears.
  2. Open the console over a borderless game and press Alt+F4: it fades out like Escape; the hotkey opens it again.

- [ ] **Step 6: Commit** `Console: may ask whether a clip has a share link, so "Delete with its link?" appears there again (fb1a795 missed the grant); Alt+F4 closes it like Escape instead of destroying it until the next start; a test reads each window's imports and fails when it calls a command its capability does not grant`

---

### Task 7: Leftovers are cleared at start, and Quit waits for a save (SAVE-3, EDIT-16, SAVE-11, SAVE-8)

**Files:**
- Create: `src-tauri/src/sweep.rs` (`pub mod sweep;` in `lib.rs`)
- Modify: `src-tauri/src/muxer.rs:126-134` (scratch names), `:376` (pending name), `:191` and `:203-206` (failed WAVs)
- Modify: `src-tauri/src/state.rs:1306-1312` (`sanitize_name`), plus `wait_until_idle`
- Modify: `src-tauri/src/edit.rs:53` (`is_busy`), `src-tauri/src/clips.rs` (`Library::ids`)
- Modify: `src-tauri/src/lib.rs:105-113` (`save_clip_and_notify`), setup after `:1258-1270` (`sweep::run`), new `quit`
- Modify: `src-tauri/src/tray.rs:139-149`, `src-tauri/src/updater.rs:77-84`
- Test: `sweep.rs`, `muxer.rs` (`:548`), `state.rs` (`:1321`)

**Interfaces:**
- Consumes: Task 3's export temp name `<stem>.<pid>.export.mp4`; the facecam stage-2 plan's composite temp `<clip>.<pid>.cam.mp4` in the clip folder (its Task 16) and phase 1's `cam.part.h264` / `cam.part.mp4` in `originals/<id>/` (its Task 14). The sweep knows both names whichever plan lands first; before stage 2 they simply never match.
- Produces:
  - `muxer::scratch_name(clip_id: &str, kind: &str) -> String` → `"<sanitize(clip_id)>.part.<kind>"`; the pending clip is `scratch_name(id, "mp4")`, the WAVs `scratch_name(id, "<sanitized source>.wav")`.
  - `sweep::is_scratch_name(name: &str) -> bool` — `split_once(".part.")` with a tail of `h264`, `mp4` or `….wav`, and a head that is a 36-character `[0-9a-f_]` id, alone or after `_`; or a name ending `.<digits>.neu.mp4`, `.<digits>.export.mp4` or `.<digits>.cam.mp4`.
  - `sweep::leftovers_in(clip_dir: &Path, before: SystemTime) -> Vec<PathBuf>` — files in the clip folder and up to two folders below it (the depth `filing::inside` allows), skipping folders that start with `.`, matching `is_scratch_name` and modified before `before`.
  - `sweep::cam_partials(originals: &Path, before: SystemTime) -> Vec<PathBuf>` — `cam.part.h264` and `cam.part.mp4` directly inside a folder under `originals` (`edit::root()` in `run`), modified before `before` (`facecam.json` is written only after `cam.mp4`, so a part file there is never a finished cam).
  - `sweep::orphan_stores(root: &Path, known: &HashSet<String>, before: SystemTime) -> Vec<PathBuf>` — folders under `stems::root()` whose name is not in `known` (sanitised ids), with neither `tracks.json` nor `spuren.json`, modified before `before`.
  - `sweep::run(app: &tauri::AppHandle)` — on its own thread: `started` is taken before spawning; scratch files and cam partials from before `started` are removed; stores older than `started − 10 min` (share import writes stems too) whose id has no row in `Library::ids` are removed. One info line per removal.
  - `edit::is_busy() -> bool` (`WORKING.try_lock().is_none()`); `clips::Library::ids(&self) -> rusqlite::Result<Vec<String>>` (`SELECT id FROM clips`).
  - `state::wait_until_idle(busy: impl Fn() -> bool, limit: Duration, step: Duration) -> bool` (true when idle in time); `lib::quit(app: &tauri::AppHandle)`; `const QUIT_WAIT: Duration = 30 s`.

- [ ] **Step 1: Write the failing tests**

```rust
// sweep.rs tests
const ID: &str = "1b4e28ba_2fa1_11d2_883f_0016d3cca427";

#[test]
fn old_and_new_scratch_names_are_both_recognised() {
    for name in [
        format!("Valorant_2026-10-22_12-00-00-123_{ID}.part.h264"),
        format!("Valorant_2026-10-22_12-00-00-123_{ID}.part.mix.wav"),
        format!("{ID}.part.mp4"),
        format!("{ID}.part.game_1.wav"),
        "Valorant_2026-10-22_12-00-00-123.4711.neu.mp4".into(),
        "Valorant_2026-10-22_12-00-00-123 (10 MB).4711.export.mp4".into(),
        "Valorant_2026-10-22_12-00-00-123.4711.cam.mp4".into(),
        crate::muxer::scratch_name("1b4e28ba-2fa1-11d2-883f-0016d3cca427", "mix.wav"),
    ] {
        assert!(is_scratch_name(&name), "{name}");
    }
    for name in ["Valorant_2026-10-22_12-00-00-123.mp4", "Valorant_2026-10-22_12-00-00-123 (10 MB).mp4", "holiday.part.mp4", "my.neu.mp4", "my.cam.mp4", "notes.part.wav"] {
        assert!(!is_scratch_name(name), "{name}");
    }
}

#[test]
fn leftovers_are_found_two_levels_down_and_only_from_before_the_start() {
    let dir = scratch_dir();
    let videos = dir.join("Valorant").join("Videos");
    std::fs::create_dir_all(videos.join("deeper")).unwrap();
    let top = dir.join(format!("{ID}.part.mp4"));
    let nested = videos.join(format!("Valorant_2026-10-22_12-00-00-123_{ID}.part.h264"));
    let too_deep = videos.join("deeper").join(format!("{ID}.part.mp4"));
    let clip = videos.join("Valorant_2026-10-22_12-00-00-123.mp4");
    for file in [&top, &nested, &too_deep, &clip] {
        std::fs::write(file, b"x").unwrap();
    }
    let mut found = leftovers_in(&dir, SystemTime::now() + Duration::from_secs(60));
    found.sort();
    let mut expected = vec![top, nested];
    expected.sort();
    assert_eq!(found, expected);
    assert!(leftovers_in(&dir, SystemTime::UNIX_EPOCH).is_empty(), "a file written since the start may be a save in progress");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_clip_folder_that_is_not_there_has_no_leftovers() {
    let missing = std::env::temp_dir().join("clippiboy-not-there").join("at-all");
    assert!(leftovers_in(&missing, SystemTime::now()).is_empty());
}

#[test]
fn a_half_written_cam_is_found_in_the_originals_store() {
    let root = scratch_dir();
    let store = root.join(ID);
    std::fs::create_dir_all(&store).unwrap();
    for name in ["cam.part.h264", "cam.part.mp4", "cam.mp4", "video.mp4", "facecam.json"] {
        std::fs::write(store.join(name), b"x").unwrap();
    }
    let mut found = cam_partials(&root, SystemTime::now() + Duration::from_secs(60));
    found.sort();
    assert_eq!(found, vec![store.join("cam.part.h264"), store.join("cam.part.mp4")]);
    assert!(cam_partials(&root, SystemTime::UNIX_EPOCH).is_empty(), "a save writing its cam right now");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn only_incomplete_stores_without_a_clip_are_orphans() {
    let root = scratch_dir();
    for name in ["known", "orphan", "indexed", "legacy"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    std::fs::write(root.join("indexed").join("tracks.json"), b"{}").unwrap();
    std::fs::write(root.join("legacy").join("spuren.json"), b"{}").unwrap();
    let known = HashSet::from(["known".to_string()]);
    let later = SystemTime::now() + Duration::from_secs(60);
    assert_eq!(orphan_stores(&root, &known, later), vec![root.join("orphan")]);
    assert!(orphan_stores(&root, &known, SystemTime::UNIX_EPOCH).is_empty(), "a store being written right now");
    let _ = std::fs::remove_dir_all(&root);
}

// muxer.rs tests
#[test]
fn scratch_names_carry_only_the_clip_id() {
    assert_eq!(
        scratch_name("1b4e28ba-2fa1-11d2-883f-0016d3cca427", "h264"),
        "1b4e28ba_2fa1_11d2_883f_0016d3cca427.part.h264"
    );
}

// state.rs tests
#[test]
fn a_long_window_title_makes_a_short_file_name() {
    assert_eq!(sanitize_name(&"Ы".repeat(300)).chars().count(), 60);
    assert_eq!(sanitize_name("Counter-Strike 2"), "Counter_Strike_2");
    assert_eq!(sanitize_name(&format!("{}!", "a".repeat(59))), "a".repeat(59), "no underscore left after the cut");
}

#[test]
fn quitting_waits_for_a_save_but_not_forever() {
    let polls = std::cell::Cell::new(0);
    let busy_twice = || { polls.set(polls.get() + 1); polls.get() < 3 };
    let ms = std::time::Duration::from_millis;
    assert!(wait_until_idle(busy_twice, ms(1_000), ms(1)));
    assert!(!wait_until_idle(|| true, ms(20), ms(5)));
}
```
  `scratch_dir()` in `sweep.rs` tests: a fresh `temp_dir()/clippiboy-sweep-<uuid>`.

- [ ] **Step 2: Run** `cargo test --lib -- sweep muxer::tests state::tests` (PowerShell). Expected: FAIL to compile, module `sweep` and `scratch_name` missing.

- [ ] **Step 3: Implement the names.** `scratch_name` as above; `build`'s `scratch` closure and `encode`'s `pending` use it and lose the file stem (SAVE-11: `<id>.part.<kind>` is unique on its own). `sanitize_name` cuts to 60 characters (`filing::MAX_LEN`) by `chars()`, then trims `_` again. SAVE-8: both `Err` arms of the WAV writes in `build` also `remove_file(&path)`, and log at error.

- [ ] **Step 4: Implement `sweep.rs`** with the interfaces above, and call `sweep::run(handle)` in `setup` right after the library block that ends with `filing::tidy`.

- [ ] **Step 5: Implement the waiting Quit.**
  - `lib::quit`: set `quitting`, `console::close(app)` (still on the main thread), then on a thread: if `is_saving() || edit::is_busy()`, banner `"Finishing the clip…"` / `"ClippiBoy quits once it is saved"`; `wait_until_idle(.., QUIT_WAIT, 100 ms)` (a warn line when it gave up); then `abandon_recording`, `stop_pipeline`, `app.exit(0)`.
  - `tray.rs` `"quit"` calls `crate::quit(app)`. `updater.rs` `on_before_exit` waits the same way before `abandon_recording`.
  - `save_clip_and_notify` returns `Err("ClippiBoy is quitting.")` while `quitting` is set, so no save starts during the wait.

- [ ] **Step 6: Run** `cargo test --lib` (PowerShell). Expected: green.

- [ ] **Step 7: Manual check (Windows).**
  1. A 2-minute 4K buffer; save a clip and click Quit in the tray while "Saving clip…" shows: the app quits after the clip is written, and the clip is in the gallery at the next start.
  2. Save a clip and end `clippiboy.exe` in Task Manager at once: at the next start the `.part.*` files are gone from the game folder, and the log says which were cleared.

- [ ] **Step 8: Commit** `Leftovers: half-written clip, edit, export and cam files from a crash or a kill are cleared from the clip folder at start, and stem stores with neither a clip nor an index from the data folder; Quit and the update restart wait up to 30 s for a running save or edit; scratch files are named by the clip id alone and game names cut to 60 characters, so a long window title no longer breaks every save; a failed WAV is removed instead of keeping the space that ran out`

---

### Task 8: Settings are written safely and never replaced by defaults (CFG-2, CFG-V1)

**Files:**
- Modify: `src-tauri/src/config.rs:148-184` (`load`, `save`) and tests
- Modify: `src-tauri/src/state.rs:325` (`load` returns a pair), struct field, `:421-424` and `:453-455` (save inside the guard)
- Modify: `src-tauri/src/lib.rs:1280` (start-up banner after `overlay::create`)

**Interfaces:**
- Produces:
  - `#[derive(Debug, Clone, PartialEq)] pub enum Loaded { Fine, Recovered { aside: Option<PathBuf>, from_backup: bool, reason: String } }` with `fn message(&self) -> Option<String>`.
  - `pub fn load() -> (AppConfig, Loaded)`; `fn read_config(dir: &Path, stamp_ms: i64) -> (Option<AppConfig>, Loaded)` (the raw parse, before the encoder, bitrate and source fix-ups, which `load` applies to whatever came out — backup included).
  - `fn save_in(dir: &Path, config: &AppConfig) -> io::Result<()>`; `fn write_synced(path: &Path, text: &str) -> io::Result<()>` (a temp name of its own, `<name>.<pid>-<counter>.tmp`, then `File::create` → `write_all` → `sync_all` → `rename`; the temp is removed on error).
  - `static HOLD_SAVES: AtomicBool`; `AppState.config_trouble: Mutex<Option<String>>`.

- [ ] **Step 1: Write the failing tests** (`config.rs` tests; `config_dir()` is a fresh `temp_dir()/clippiboy-config-<uuid>`)

```rust
#[test]
fn a_missing_config_is_a_first_start() {
    let dir = config_dir();
    let (config, loaded) = read_config(&dir, 1);
    assert!(config.is_none());
    assert_eq!(loaded, Loaded::Fine);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "nothing written");
}

#[test]
fn a_good_config_is_kept_as_the_backup() {
    let dir = config_dir();
    let text = serde_json::to_string_pretty(&default_config()).unwrap();
    std::fs::write(dir.join("config.json"), &text).unwrap();
    let (config, loaded) = read_config(&dir, 1);
    assert!(config.is_some());
    assert_eq!(loaded, Loaded::Fine);
    assert_eq!(std::fs::read_to_string(dir.join("config.json.bak")).unwrap(), text);
}

#[test]
fn a_damaged_config_is_set_aside_and_the_backup_restored() {
    for damage in [vec![0u8; 3], vec![0xff, 0xfe, 0x00]] {
        let dir = config_dir();
        let mut good = default_config();
        good.clip_dir = r"D:\Clips".into();
        std::fs::write(dir.join("config.json.bak"), serde_json::to_string(&good).unwrap()).unwrap();
        std::fs::write(dir.join("config.json"), &damage).unwrap();
        let (config, loaded) = read_config(&dir, 1234);
        assert_eq!(config.expect("the backup").clip_dir, r"D:\Clips");
        let aside = dir.join("config.json.broken-1234");
        assert!(matches!(&loaded, Loaded::Recovered { aside: Some(path), from_backup: true, .. } if *path == aside));
        assert_eq!(std::fs::read(&aside).unwrap(), damage, "the damaged file is kept as it was");
        assert!(!dir.join("config.json").exists());
    }
}

#[test]
fn without_a_backup_the_damaged_file_is_still_kept() {
    let dir = config_dir();
    std::fs::write(dir.join("config.json"), b"{\"recording\": ").unwrap();
    let (config, loaded) = read_config(&dir, 7);
    assert!(config.is_none());
    assert!(matches!(loaded, Loaded::Recovered { aside: Some(_), from_backup: false, .. }));
    assert!(dir.join("config.json.broken-7").is_file());
}

#[test]
fn a_read_error_is_not_a_first_start() {
    let dir = config_dir();
    // A folder where the file should be: reading fails, and not with NotFound.
    std::fs::create_dir_all(dir.join("config.json")).unwrap();
    let (_, loaded) = read_config(&dir, 9);
    assert!(matches!(loaded, Loaded::Recovered { .. }), "{loaded:?}");
}

#[test]
fn concurrent_saves_never_leave_torn_json() {
    let dir = config_dir();
    let threads: Vec<_> = (0..8)
        .map(|n| {
            let dir = dir.clone();
            std::thread::spawn(move || {
                let mut config = default_config();
                config.clip_dir = format!(r"D:\Clips{n}");
                (0..25).all(|_| save_in(&dir, &config).is_ok())
            })
        })
        .collect();
    for thread in threads {
        assert!(thread.join().unwrap(), "a save failed");
    }
    let text = std::fs::read_to_string(dir.join("config.json")).unwrap();
    assert!(serde_json::from_str::<AppConfig>(&text).is_ok());
    let temps = std::fs::read_dir(&dir).unwrap().flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
        .count();
    assert_eq!(temps, 0);
}
```

- [ ] **Step 2: Run** `cargo test --lib config` (PowerShell). Expected: FAIL to compile, `read_config`, `Loaded`, `save_in` not found.

- [ ] **Step 3: Implement `config.rs`.**
  - `read_config`: read `config.json`, retrying three times 100 ms apart on errors other than `NotFound` and `InvalidData`. `NotFound` → `(None, Fine)`, the backup untouched (a user who deleted the file to reset gets defaults). Parsed → `write_synced` of the same text to `config.json.bak` (a warn line on failure) → `(Some, Fine)`. Any other outcome → rename the file to `config.json.broken-<stamp_ms>` (`aside: None` when that fails too), then try `config.json.bak`.
  - `load`: fix-ups as today on the result, `default_config()` without one; a warn line for `Recovered`; `HOLD_SAVES` set when `aside` is `None`.
  - `save`: while `HOLD_SAVES` is set → `Err(io::Error::other("the settings file could not be read at start — it is not overwritten"))`; otherwise `save_in(&data_dir(), config)`.
  - `Loaded::message`, exact copy:
    - aside and backup: `"The settings file was damaged. ClippiBoy restored the copy from the last good start; the damaged file is kept as {name}."`
    - aside, no backup: `"The settings file was damaged and no good copy was left. ClippiBoy starts with default settings; the damaged file is kept as {name}."`
    - no aside: `"The settings file could not be opened ({reason}). ClippiBoy runs on {the last good copy | default settings} for now and leaves the file alone until the next start."`

- [ ] **Step 4: Wire it up.**
  - `AppState::new`: `let (config, loaded) = config::load();`, `config_trouble: Mutex::new(loaded.message())`.
  - `replace_config`: the `config::save(&next)` call moves into the guard block at `:421-424`, so every writer saves under the config lock and two saves never race. `sync_all` costs a few ms per write; CFG-11's debounce (not in this plan) is what keeps slider drags cheap.
  - `setup`, after `overlay::create(handle)`: if `config_trouble` holds a message, a thread waits 5 s (the banner page has loaded by then), then `overlay::show(.., BannerKind::Error, "Settings could not be read", Some(text))` and `notify(.., "error", text)`.

- [ ] **Step 5: Run** `cargo test --lib` (PowerShell). Expected: green.

- [ ] **Step 6: Manual check (Windows).**
  1. Quit; overwrite `%APPDATA%\ClippiBoy\config.json` with three NUL bytes; start. Expected: banner "Settings could not be read", the settings as at the last start, and `config.json.broken-<ms>` beside it.
  2. Quit; in PowerShell `$f = [IO.File]::Open("$env:APPDATA\ClippiBoy\config.json", 'Open', 'Read', 'None')`; note `Get-FileHash`; start ClippiBoy, change a setting, quit; `$f.Close()`. Expected: the banner said the file was left alone, and the hash is unchanged.

- [ ] **Step 7: Commit** `Settings: the config file is written to a temp name of its own, flushed to disk and renamed in, by every writer under the config lock; a file that cannot be read is moved aside as config.json.broken-<time> and the copy from the last good start restored — before, defaults were saved over it at the same start; a file another program holds is left alone for the session, and a banner says what happened`

---

### Task 9: Deleting a clip that is open keeps it, and says so (SAVE-6, EDIT-15)

**Files:**
- Modify: `src-tauri/src/clips.rs:314-331` (`Library::delete`), plus `remove_file_patiently`
- Modify: `src-tauri/src/commands.rs:741-779` (`delete_clip`)
- Modify: `src/store.ts:499-502` (`deleteClip`)
- Test: `clips.rs` tests (`:408`)

**Interfaces:**
- Produces: `clips::remove_file_patiently(path: &Path, attempts: u32) -> std::io::Result<()>` — `NotFound` is success; other errors retried with the back-off of `muxer::replace_file` (`50 ms × attempt`). `Library::delete(&self, id: &str) -> rusqlite::Result<()>` now removes the row and its tags only; the files are the caller's (this also drops the second `get` the review found under SAVE-6).

- [ ] **Step 1: Write the failing tests**

```rust
fn delete_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("clippiboy-delete-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_clip_file_goes_and_a_missing_one_counts_as_gone() {
    let dir = delete_dir();
    let file = dir.join("clip.mp4");
    std::fs::write(&file, b"x").unwrap();
    assert!(remove_file_patiently(&file, 1).is_ok());
    assert!(!file.exists());
    assert!(remove_file_patiently(&file, 1).is_ok(), "already gone is fine");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_that_will_not_go_is_reported() {
    let dir = delete_dir();
    // A folder where the clip should be stands in for a file a player holds.
    let blocked = dir.join("clip.mp4");
    std::fs::create_dir_all(&blocked).unwrap();
    assert!(remove_file_patiently(&blocked, 2).is_err());
    assert!(blocked.exists());
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: Run** `cargo test --lib clips` (PowerShell). Expected: FAIL to compile, `remove_file_patiently` not found.

- [ ] **Step 3: Implement.**
  - `delete_clip`: read the clip under the library lock and release it; `remove_file_patiently(&clip.path, 10)` without the lock; on `Err` → `notify(&app, "error", msg)` and `Err(msg)` with `msg = "Could not delete the clip ({err}). Is it open somewhere right now?"`, row untouched. Then `lib.delete(&clip.id)` under the lock, the old `thumb_path` file and everything after as today (stems, original, thumbnail, Task 3's Discord copies, prune, events).
  - `store.ts` `deleteClip`: keep the optimistic removal; `try { await api.deleteClip(id) } catch (err) { re-read with api.listClips(), set lastError }`, so a refused delete puts the clip back and nothing is left unhandled.

- [ ] **Step 4: Run** `cargo test --lib` (PowerShell) and `npx tsc --noEmit` (WSL). Expected: green, `delete_removes_the_entry` still passes.

- [ ] **Step 5: Manual check (Windows).** Open a clip in VLC, delete it in ClippiBoy. Expected: the notice "Could not delete the clip (…). Is it open somewhere right now?", and the clip stays in (or comes back to) the gallery. Close VLC and delete again: gone from the gallery and from disk.

- [ ] **Step 6: Commit** `Delete: the clip's file goes first, with retries, and the row only after it — a clip open in a player vanished from the gallery while its file stayed on disk for good; now it stays and the notice says why`

---

### Task 10: A missing ffprobe no longer deletes every clip (SAVE-V1, SAVE-9)

**Files:**
- Modify: `src-tauri/src/muxer.rs:81-88` (`available`), `:210-218` (`build` passes the length), `:424-426` (`encode`), `:529-545` (probe)
- Modify: `src-tauri/src/commands.rs:404-417` (`save_clip`)
- Test: `muxer.rs` tests

**Interfaces:**
- Produces: `pub fn probe_duration(path: &Path) -> Result<Option<u64>, String>` (`Err` = ffprobe could not be started); `fn probe_duration_by(program: Command, path: &Path) -> Result<Option<u64>, String>`; `probe_duration_ms` keeps its signature as `probe_duration(path).ok().flatten()`, so `edit.rs` and `shrink` are unchanged.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn an_ffprobe_that_cannot_start_is_not_an_unreadable_clip() {
    let missing = Command::new("clippiboy-no-such-ffprobe");
    assert!(probe_duration_by(missing, Path::new("clip.mp4")).is_err());
}
```

- [ ] **Step 2: Run** `cargo test --lib muxer` (PowerShell). Expected: FAIL to compile, `probe_duration_by` not found.

- [ ] **Step 3: Implement.**
  - `available()` is true only when both `ffmpeg -version` and `ffprobe -version` answer, so the PATH fallback at `state.rs:872` and `:1035` gives `tools::not_ready_reason()` instead of saving.
  - `encode`: `Ok(Some(ms))` as today, `Ok(None)` → `give_up` as today, `Err(err)` → keep the clip with `length_ms.unwrap_or(0)` and a warn line (ffprobe vanished between gate and probe). `build` passes `Some(snapshot.audio_frames as u64 * 1000 / SAMPLE_RATE as u64)`; its progress callback stays a no-op.
  - `save_clip` command: both arms go through `crate::save_clip_and_notify(&app, seconds)`, so a length from a page gets the save guard, the event and the friend tags (SAVE-9).

- [ ] **Step 4: Run** `cargo test --lib` (PowerShell). Expected: green.

- [ ] **Step 5: Manual check (Windows).** Rename `%LOCALAPPDATA%\gg.clippiboy.app\ffmpeg` away, block the download (offline), put only `ffmpeg.exe` in a PATH folder, start and save a clip. Expected: the not-ready message, nothing written or deleted. Undo the changes afterwards.

- [ ] **Step 6: Commit** `Saving: the PATH fallback needs ffprobe as well as ffmpeg — with ffmpeg alone every clip was written and then deleted as unreadable; an ffprobe that cannot start keeps the clip; save_clip with a length goes through the guarded path like every other save`

---

### Task 11: Settings writes cannot move the clip folder or change the token (CFG-5, CFG-9, REC-V1)

**Files:**
- Modify: `src-tauri/src/config.rs:186-192` (`keep_core_owned` beside `keep_once_flags`) and tests
- Modify: `src-tauri/src/commands.rs:54-90` (`set_config`), `:277-307` (`set_clip_dir`, `usable_clip_dir`), tests `:1528-1535`
- Modify: `src/store.ts:167`, `:264-273`, `:371-388`

**Interfaces:**
- Produces: `config::keep_core_owned(next: &mut AppConfig, previous: &AppConfig)` — `clip_dir` and `control.token` always come from `previous` (only `set_clip_dir` and `regenerate_token` change them); `fn names_a_whole_volume(canonical: &Path) -> bool` in `commands.rs` (`canonical.parent().is_none()`).

- [ ] **Step 1: Write the failing tests**

```rust
// config.rs tests
#[test]
fn a_settings_page_cannot_move_the_clip_folder_or_change_the_token() {
    let mut previous = default_config();
    previous.clip_dir = r"D:\Clips".into();
    previous.control.token = "real".into();
    let mut next = default_config();
    next.clip_dir = r"C:\..".into();
    next.control.token = "mock-token".into();
    keep_core_owned(&mut next, &previous);
    assert_eq!(next.clip_dir, r"D:\Clips");
    assert_eq!(next.control.token, "real");
}

// commands.rs: a_whole_drive_is_no_clip_folder gets
let climbs: &[&str] = if cfg!(windows) { &[r"C:\..", r"D:\x\..\.."] } else { &["/home/..", "/home/x/../.."] };
for dir in climbs {
    assert!(!usable_clip_dir(dir), "{dir}");
}

// commands.rs
#[test]
fn a_canonical_drive_or_share_root_is_a_whole_volume() {
    if cfg!(windows) {
        assert!(names_a_whole_volume(Path::new(r"\\?\C:\")));
        assert!(names_a_whole_volume(Path::new(r"\\?\UNC\nas\videos")));
        assert!(!names_a_whole_volume(Path::new(r"\\?\C:\Clips")));
    }
}
```

- [ ] **Step 2: Run** `cargo test --lib -- config::tests commands::tests` (PowerShell). Expected: FAIL to compile (`keep_core_owned`, `names_a_whole_volume` not found).

- [ ] **Step 3: Implement.**
  - `usable_clip_dir` also refuses any `Component::ParentDir`.
  - `set_config`: the empty-token branch (`:66-68`) and the `usable_clip_dir` branch (`:84-86`) become one `crate::config::keep_core_owned(&mut config, &previous);` next to `keep_once_flags`.
  - `set_clip_dir`: first `if state.is_recording() { return Err("Stop the recording before moving the clip folder.".into()) }` (REC-V1: the disk guard then always measures the drive the recording is on, and recovery looks where it lies). After `create_dir_all`, `std::fs::canonicalize` and refuse `names_a_whole_volume` with the existing "Pick a folder for the clips, not a whole drive."; store the path as typed, not the `\\?\` form.
  - `store.ts`: a module-level `let configLoaded = false`, set when `getConfig` in `init` succeeds. `patchConfig` in Tauri with `!configLoaded` fetches `api.getConfig()` first and applies the patch on top; when that fails too it sets `lastError` to `"Apply setting: the settings could not be read, so nothing was saved"` and returns. The mock is never sent.

- [ ] **Step 4: Run** `cargo test --lib` and `npm run build` (PowerShell), `npx tsc --noEmit` (WSL). Expected: green.

- [ ] **Step 5: Manual check (Windows).** During a recording, Settings → Storage → change the folder: the error "Stop the recording before moving the clip folder." under the field. A junction to a drive root (`mklink /J C:\Users\me\AllOfD D:\`) as the folder: "Pick a folder for the clips, not a whole drive." The Stream Deck keys still work after a few settings changes.

- [ ] **Step 6: Commit** `Settings: the clip folder and the Stream Deck token change only through their own commands — a settings write carrying "C:\.." granted the whole drive, and a window that fell back to the mock config wrote "mock-token" as the control token; the folder cannot move while a recording runs, a junction to a drive root is refused like the root, and the main window writes no settings before it has read the real ones`

---

## Coverage and links between tasks

- **Coverage:** CFG-2, CFG-V1 → Task 8; CFG-3, CFG-4, CFG-10 → Task 3; CFG-5, CFG-9, REC-V1 → Task 11; SAVE-3, EDIT-16, SAVE-11 (+SAVE-8) → Task 7; SAVE-V1 (+SAVE-9) → Task 10; SAVE-6 (+EDIT-15) → Task 9; CAP-1, CAP-2 → Task 1; AUD-4 → Task 2; REC-2/BUF-V1 → Task 4; REC-3, REC-V2 (+REC-7) → Task 5; CON-1 with the grants test, CON-2 → Task 6. Left open on purpose: an orphaned work folder after a failed finalize *followed by* a folder change (REC-V1's rarest path; its files stay visible in `.recording`), the root fix of CAP-9, REC-9 and REC-12.
- **Names across tasks:** `remove_discord_exports` (Task 3) is called in `delete_clip`, which Task 9 restructures — keep the call after the row is gone. The `.export.mp4` temp (Task 3) is matched by `is_scratch_name` (Task 7). `Recorder::finish -> Meta` (Task 4) is what Task 5's test calls. `muxer::scratch_name` (Task 7) and `probe_duration` (Task 10) both touch `muxer::build`/`encode`; Task 10 comes after Task 7.
- **Order:** by user impact in the review (CAP-1, AUD-4 and CFG-3 are in its top ten), then medium, then low. Tasks are independent except for the name links above.
