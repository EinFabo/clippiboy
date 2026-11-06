# Sync and Timestamp Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. The "Manual check" steps need a person at a Windows machine; everything else an agent can do.

**Goal:** Fix the timing bugs the review confirmed, so that picture and sound stay together through full-length saves, encoder stalls, sleep, 44.1/96 kHz devices, stalled audio threads and long sessions.

**Architecture:** There are three fronts. (1) *Audio rings and mixer.* The clip rings cover the buffer plus a keyframe interval plus 10 s, and a save waits at most 150 ms, outside every lock, for the mixer to pass the clip's end (BUF-1). The mixer and the rings count time from running totals instead of rounding each window (AUD-6/V1). (2) *Device timestamps.* A missing or flagged stamp is estimated from what is still queued (AUD-2). One resampler that keeps its phase serves sounding and silent packets alike (AUD-7/V2). (3) *Video timestamps.* One 2 s rule tells a stall from a discontinuity (SAVE-4). Recordings cut a discontinuity out of the picture and the sound together (REC-1). The encoded frames reach ffmpeg in a minimal MPEG-TS written in Rust, which carries each frame's own time, instead of a raw stream spread at one `-r` (BUF-6, for clips and then for recordings). The raw path stays for reordered streams, for snapshots across a sleep, and for old recording folders. Every ffmpeg output follows one zero-point policy (SAVE-7). Last, the anchor is read under its lock and the tick count no longer wraps (CAP-4/10).

Why MPEG-TS: `setts` needs one expression term per hole. A sustained shortfall makes about four holes a second (one per 250 ms `CATCH_UP`), so a 2-minute clip needs about 500 terms and a recording needs thousands, which is past the 32 767-character Windows command line. Matroska and MP4 need AVCC plus an `avcC` built from SPS/PPS, and the header can be empty (`pipeline.rs:673-677`). NUT needs varints, frame codes and checksums. TS carries Annex B as it is and is append-only, so a crash leaves a readable file. ffmpeg's TS demuxer is also what the old segment pipeline used.

**Tech Stack:** Rust (Tauri 2 core, `src-tauri/src`), WASAPI, Windows.Graphics.Capture, Media Foundation H.264 MFTs, ffmpeg/ffprobe 9.0.1 as pinned in `tools.rs:26`, MPEG-TS (ISO/IEC 13818-1) written by hand.

**Spec:** `docs/facecam-review.md` is the source for every ID, severity, scenario and fix proposal used here. Background: `docs/extras/how-time-works.md`, sections 4–6 (the clock, the cut, `muxer::timing`).

Not in this plan: BUF-2, BUF-4, BUF-5, CAP-V2, CFG-1 and EDIT-1. The facecam stage-2 plan fixes them as its prerequisites.

## Global Constraints

- Line numbers are those of 401a320. Re-read each spot before editing it. `docs/extras/plans/2026-10-22-fix-data-safety-and-silent-failures.md` changes the same files (`recorder.rs` `finish`, writer loop and `finalize`; `muxer.rs` scratch names and `encode`). Whichever plan runs second rebases onto the other. If that plan ran first, `Recorder::finish` returns `Meta` and the tests here drop their `.unwrap()`.
- Commits are in English, **without** a `Co-Authored-By` line, in the house style "Area: what changes for the user; …".
- Unit tests run on the Windows side, via PowerShell in `src-tauri`, as `cargo test --lib -- <filter>`. `convert.rs` and `mft.rs` are `#![cfg(windows)]`. `cargo check` also runs from WSL.
- Real files: the probes in `src-tauri/examples`, via PowerShell: `cargo run --example record-probe -- --encoder <nvenc|amf|qsv|x264> 10`, `cargo run --example trim-probe -- "C:\path\to\copy.mp4"` and `tracks-probe` (same form). They take ffmpeg and ffprobe from `src-tauri/resources`; if those are missing, copy both from `%LOCALAPPDATA%\gg.clippiboy.app\ffmpeg\9.0.1\`. In the manual checks, `ffmpeg` and `ffprobe` mean those two `.exe` files.
- No new crates. The TS writer and its CRC are written by hand.
- Units stay in the names: `_100ns` for QPC, `_us` for pts, `_frames` for 48 kHz audio frames, `_90k` for the TS clock.
- Nothing waits or does file I/O while it holds `AppState.pipeline` or `Shared.packets`.
- Every new `recorder::Meta` field gets `#[serde(default)]`. A `.recording` folder written by 401a320 must finalize exactly as before.
- There are two thresholds for "this pts jump is not a stall", and both are written down:
  - `buffer::DISCONTINUITY_US = 2_000_000` decides the raw path's rate (SAVE-4) and which jumps a recording cuts out (REC-1). It is the same 2 s the spec uses for the cam (section 2.3, "Sleep").
  - `buffer::jump_limit_us(keyframe_seconds)` = `max(1 s, 3 × keyframe_seconds)` is what the stage-2 plan's Task 2 gives the game ring for its BUF-5 cut (`set_jump_limit_us`). Task 7 uses it to decide whether a clip can keep its timestamps. Whichever plan lands first adds the function, and the other uses it.
- Do not raise `CATCH_UP` (`convert.rs:734`). The comment at `convert.rs:754-761` explains why it is 250 ms: under a sustained shortfall, replayed ticks fall further and further behind the clock. Task 7 makes raising it unnecessary, and it also meets the review's "stage 2 must fix BUF-6 before compositing".
- Tests have sentence-style names and a doc comment naming the finding they guard. Use pure functions and simulated clocks, as `audio/ring.rs:352-375` does.

## Review Focus

- **A clip saved soon after waking from sleep.** The snapshot spans a jump of minutes. Expected: no worse than today, and never minutes of frozen picture with a 100 MB WAV per track. The raw path stays until BUF-5 (stage 2) cuts such snapshots. → Task 7 `a_snapshot_across_a_discontinuity_keeps_the_raw_path`.
- **An encoder that reorders frames** (`CLIPPIBOY_BFRAMES`, or a driver that defaults to B-frames). Expected: a clip and a recording exactly as today, never a TS with backward timestamps. → Task 7 `reordered_frames_keep_the_raw_path`, Task 8 `a_ring_notices_reordered_frames`.
- **A `.recording` folder from the version before** (a crash, then the update). Expected: it finalizes exactly as before. → Task 2 `a_folder_without_a_largest_step_keeps_the_old_rule`, Task 8 `a_folder_from_before_the_update_still_finalizes_raw`.
- **A save while the mixer is stalled or the buffer is stopping.** Expected: the save finishes at most 150 ms later with a silent tail, and does not hang. → Task 1 `the_wait_for_a_stalled_mixer_gives_up`.
- **Sleep where the mixer's gap and the picture's gap do not line up.** They differ by the mixer lag and the pacer's skip. Expected: a few ms of zeros at the seam, and no lasting shift. → Task 3 `gaps_that_do_not_line_up_leave_only_zeros_at_the_seam`.

---

### Task 1: Audio rings that outlast the keyframe cut, and a bounded wait for the mixer (BUF-1, AUD-1)

**Files:**
- Modify: `src-tauri/src/pipeline.rs:36-50` (constants, `samples_for`), `:282-343` and `:934-970` (`Shared`), `:398-407` (`set_buffer_seconds`), `:564-580` (`ClipSnapshot`), `:781-869` (`mix_loop` publishes its position), `:887-914` (`tracks_for`, `sync_tracks`), `:928` and `:1068-1076`; tests `:1207-1317`
- Modify: `src-tauri/src/muxer.rs:155-178` (wait before the first WAV)
- Modify: `src-tauri/examples/record-probe.rs:855` (drop `temp_dir: temp.clone(),`; `ClipRequest` lost that field in b781591, so the probe does not build today)

**Interfaces:**
- Produces (`pipeline.rs`):
  - `pub(crate) const AUDIO_SLACK_SECONDS: u32 = 10;`
  - `pub(crate) fn ring_seconds(buffer_seconds: u32, keyframe_seconds: u32) -> u32`, which returns `buffer + keyframe.max(1) + AUDIO_SLACK_SECONDS`.
  - `fn samples_for(seconds: u32) -> usize` becomes exact: `seconds × 48 000 × 2`, with no hidden 2 s. `TrackRing::new(.., seconds)`, `TrackRing::set_seconds(seconds)` and `tracks_for(sources, existing, ring_seconds)` all take ring seconds.
  - `pub struct MixPosition` holds an `AtomicI64`. `Default` is implemented by hand and starts it at `i64::MIN`, meaning nothing has been mixed yet. `pub fn publish(&self, at_100ns: i64)`. `pub fn wait_until(&self, at_100ns: i64, timeout: Duration) -> bool` polls every 5 ms and returns true once the published position is at least `at_100ns`.
  - `pub const MIX_WAIT: Duration = Duration::from_millis(150);` covers the 80 ms mixer lag, one 10 ms tick and scheduling.
  - `Shared` gets `keyframe_seconds: u32` and `pub mixed: Arc<MixPosition>`, plus `Shared::ring_seconds(&self) -> u32`.
  - `ClipSnapshot` gets `pub mixed: Arc<MixPosition>` and `ClipSnapshot::end_100ns(&self) -> i64`, which is `start_100ns + audio_frames × 10⁷ / 48 000`.

- [ ] **Step 1: Write the failing tests** in `pipeline.rs`:

```rust
/// BUF-1: a clip as long as the buffer starts up to one keyframe interval
/// before `newest − seconds`, and its audio is read only after the video is
/// written. The ring must still hold that start a second later.
#[test]
fn a_full_length_clip_still_finds_its_start_after_a_slow_write() {
    let ring = TrackRing::new("a".into(), "A".into(), ring_seconds(10, 4)); // 24 s
    let second = SAMPLE_RATE as i64;
    for at in 0..31 {
        ring.push(&vec![(at + 1) as i16; second as usize * CHANNELS], qpc_of_frame(at * second));
    }
    // Newest at 30 s, cut at the worst keyframe phase: 30 − 10 − 3.9 = 16.1 s,
    // read after a 1 s write (the 31st block).
    assert_eq!(ring.window(qpc_of_frame(772_800), 1)[0], 17, "the clip's start was trimmed away");
}

/// BUF-1: what the slack has to cover.
#[test]
fn a_ring_covers_the_buffer_a_keyframe_interval_and_ten_seconds() {
    assert_eq!(ring_seconds(120, 2), 132);
    assert_eq!(ring_seconds(120, 4), 134);
    assert_eq!(ring_seconds(30, 0), 41, "an interval of 0 counts as one second");
}

/// BUF-1, the tail: a save waits for the mixer to pass the clip's end, so the
/// last 80 ms the mixer has not reached yet do not come out as zeros.
#[test]
fn a_save_waits_for_the_mixer_to_pass_the_clips_end() {
    let mixed = Arc::new(MixPosition::default());
    let mixer = {
        let mixed = mixed.clone();
        std::thread::spawn(move || for step in 1..=5 {
            std::thread::sleep(Duration::from_millis(10));
            mixed.publish(step * 100_000);
        })
    };
    assert!(mixed.wait_until(500_000, MIX_WAIT));
    mixer.join().unwrap();
}

/// BUF-1 and Review Focus: a stalled or stopped mixer must not hold up the save.
#[test]
fn the_wait_for_a_stalled_mixer_gives_up() {
    let began = std::time::Instant::now();
    assert!(!MixPosition::default().wait_until(1, MIX_WAIT));
    assert!(began.elapsed() >= MIX_WAIT && began.elapsed() < Duration::from_secs(1));
}
```
  Update `a_new_buffer_length_reaches_a_running_ring`, since capacities are now exact: `new(.., 1)` holds 1 s, `set_seconds(4)` holds 4 s, and `set_seconds(1)` holds 1 s. Also update the "1 s + 2 s slack" comments at `:1209`, `:1231` and `:1303`.

- [ ] **Step 2: Run** `cargo test --lib -- pipeline::tests`. Expected: FAIL to compile, because `ring_seconds`, `MixPosition` and `MIX_WAIT` are not found.

- [ ] **Step 3: Implement.**
  - `Pipeline::start` stores `recording.keyframe_seconds` and builds the rings with `ring_seconds`.
  - `set_buffer_seconds` stores the buffer length as before and resizes every ring to `ring_seconds(seconds, keyframe_seconds)`. `sync_tracks` passes `shared.ring_seconds()`.
  - `mix_loop` calls `shared.mixed.publish(next)` after both branches (`:831`, `:868`).
  - `snapshot` puts a clone of `shared.mixed` into the snapshot.
  - In `muxer::build`, right before the first WAV (`:178`), call `snapshot.mixed.wait_until(snapshot.end_100ns(), pipeline::MIX_WAIT)`. If it returns false, log at info that the tail is silent.
  - The video is still written first. The wait usually returns at once, because writing the `.h264` took longer than 80 ms.
  - The WAVs are still written straight from the rings. The 10 s slack covers the write, so no 23 MB copy per track is needed.
  - Do not move the WAVs in front of the video on their own: the review says every clip would then end in 50–70 ms of zeros.

- [ ] **Step 4: Run** `cargo test --lib -- pipeline::tests`. Expected: PASS, with 4 more tests than before. `cargo run --example record-probe -- --encoder <yours> 10` builds and prints `clip: …`.

- [ ] **Step 5: Manual check on Windows.**
  1. Set the keyframe interval to 4 s, the buffer to 30 s and the clip length to the whole buffer.
  2. Play music without pauses.
  3. Save 10 clips, each at least 40 s after the buffer started.
  4. For each clip, run `ffmpeg -hide_banner -t 1 -i clip.mp4 -af volumedetect -f null - 2>&1 | Select-String mean_volume`, and the same with `-sseof -0.1` in place of `-t 1`.

  Expected: no clip shows `-91.0 dB`. Before the fix, about half of the clips started mute.

- [ ] **Step 6: Commit** `Clips: a clip as long as the buffer no longer starts silent — the audio rings keep the buffer plus a keyframe interval plus 10 s, and a save waits up to 150 ms for the mixer to reach the clip's end, so clips no longer end in a few dozen milliseconds of silence; record-probe builds again`

---

### Task 2: One rule for gaps: a shortfall is not a sleep (SAVE-4)

**Files:**
- Modify: `src-tauri/src/buffer.rs:13-14` (the constant)
- Modify: `src-tauri/src/muxer.rs:228-271` (`timing`, `largest_step_us`); tests `:551-579`
- Modify: `src-tauri/src/pipeline.rs:564-580` and `:1039-1054`
- Modify: `src-tauri/src/recorder.rs:53-85` (`Meta`), `:128-131`, `:223-233` (`video`), `:349-376` (`finish`), `:566`

**Interfaces:**
- Produces:
  - `buffer::DISCONTINUITY_US: i64 = 2_000_000`
  - `muxer::largest_step_us(pts_us: &[i64]) -> i64` is the largest difference between neighbours after sorting a copy, or 0 for fewer than two frames.
  - `muxer::timing(frames: u64, span_us: Option<u64>, largest_step_us: Option<i64>, fps: u32) -> Timing`.
    - With `Some(step)`, it stretches iff `frames > 0 && span ≥ nominal && step ≤ DISCONTINUITY_US`; otherwise it takes the nominal rate.
    - With `None` (a folder from before this change), it keeps today's `span ≤ nominal·3/2` rule.
  - `ClipSnapshot.largest_step_us: i64`
  - `recorder::Meta.largest_step_us: Option<i64>` (`#[serde(default)]`), set by `finish`; `Recorder.largest_step_us: AtomicI64`.

- [ ] **Step 1: Write the failing tests** in `muxer.rs`:

```rust
/// SAVE-4: 36 of 60 fps for 30 s was muxed at 60 fps. The clip came out 18 s
/// long, played 1.67× fast, and its end was 12 s away from its sound.
#[test]
fn a_sustained_shortfall_is_stretched_not_taken_for_sleep() {
    let timing = timing(1080, Some(30_000_000), Some(266_666), 60);
    assert_eq!(timing.rate, "1080000000/30000000");
    assert_eq!(timing.span_us, 30_000_000);
}

/// Review Focus: a recording folder from before this change has no largest
/// step, and keeps the old rule.
#[test]
fn a_folder_without_a_largest_step_keeps_the_old_rule() {
    assert_eq!(timing(1080, Some(30_000_000), None, 60).rate, "60");
    assert_eq!(timing(1500, Some(30_000_000), None, 60).rate, "1500000000/30000000");
}

/// SAVE-4: steps are measured between neighbours in time, not in arrival order.
#[test]
fn the_largest_step_is_measured_between_neighbours_in_time() {
    assert_eq!(largest_step_us(&[0, 16_666, 1_516_666, 33_333]), 1_483_333);
    assert_eq!(largest_step_us(&[5]), 0);
}
```
  `a_gap_the_frames_cannot_explain_keeps_the_nominal_rate` becomes `timing(1800, Some(3_630_000_000), Some(3_600_016_666), 60)`, and still expects `"60"` and 30 000 000. The two other existing tests pass `Some(16_667)` and `Some(266_666)`.

  In `recorder.rs`, add a helper `fn frame(pts_us: i64, keyframe: bool) -> EncodedPacket`: track 0, `duration_us: 16_666`, two bytes of data. Then:

```rust
/// SAVE-4: a recording hands its largest step to `finalize`.
#[test]
fn a_recording_remembers_its_largest_step() {
    let dir = scratch("step");
    let meta = Meta { id: "s".into(), fps: 60, ..Meta::default() };
    let backlog = vec![frame(0, true), frame(16_666, false), frame(516_666, false)];
    let recorder = Recorder::start(&dir, meta, &[], backlog, 0).unwrap();
    assert_eq!(recorder.finish(Vec::new(), None).unwrap().largest_step_us, Some(500_000));
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: Run** `cargo test --lib -- muxer:: recorder::`. Expected: FAIL to compile, because `timing` takes 3 arguments and `largest_step_us` is not found.

- [ ] **Step 3: Implement.**
  - `Pipeline::snapshot` collects the video pts once, stores the largest step in the snapshot, and passes `Some(step)` to `timing`.
  - `Recorder::video` takes `previous = last_pts_us.fetch_max(pts)`. When `pts > previous`, it records `pts − previous` with `largest_step_us.fetch_max`.
  - `finish` writes the value to `Meta`, and `finalize` passes `meta.largest_step_us` to `timing`.
  - Update the doc comment at `muxer.rs:248-250`.

- [ ] **Step 4: Run** `cargo test --lib -- muxer:: recorder:: pipeline::`. Expected: PASS.

- [ ] **Step 5: Manual check on Windows** (optional; it needs a GPU that falls short). Set 2160p60 on a laptop iGPU where the health line in the log shows under 40 fps measured. Save a 30 s clip while playing an A/V sync test video, one with a flash and a beep every second. Expected: the clip lasts about 30 s and the beeps sit on the flashes. Before the fix, it lasted about 18 s, played fast, and lost its sound at the end.

- [ ] **Step 6: Commit** `Clips and recordings: an encoder that falls more than a third short is no longer taken for sleep — only a single jump of more than 2 s counts as one — so such clips keep their length and their sound instead of playing fast with the end cut off`

---

### Task 3: Sleep during a recording (REC-1)

**Files:**
- Modify: `src-tauri/src/recorder.rs:53-117` (`Meta`, `Job`, `AudioSide`), `:223-233` (`video`), `:292-336` (`audio`, `end_window`, the new `skip_window`, `open_track`), `:349-376` (`finish`), `:383-421` (`run_job`), `:449-472` (next to `align`), `:550-633` (`finalize`); tests
- Modify: `src-tauri/src/pipeline.rs:825-829` (the gap branch calls `skip_window`)

**Interfaces:**
- Consumes: `buffer::DISCONTINUITY_US`, `Recorder.largest_step_us` (Task 2).
- Produces (`recorder.rs`):
  - `pub struct VideoGap { pub at_us: i64, pub len_us: i64 }`. `at_us` is the time, in µs from the recording's first frame, where the last frame before the jump ends. `len_us` is the length of the hole. Derive `Debug, Clone, Copy, PartialEq, Default`, with serde camelCase.
  - `pub struct AudioGap { pub at_frame: u64, pub frames: u64 }`, on the recording's audio timeline.
  - `Meta.video_gaps: Vec<VideoGap>` and `Meta.audio_gaps: Vec<AudioGap>`, both `#[serde(default)]`. `Meta.span_us` now excludes the video gaps; update its doc at `:73-78`.
  - `pub fn skip_window(&self, frames: usize)` replaces `end_window` in the mixer's gap branch.
  - `Recorder.closed_us: AtomicI64` holds the sum of `len_us` so far. Task 8 subtracts it from the written times.
  - `Job::Meta(Meta)`. After `start`, every meta write goes through the writer thread: the gap here, and `open_track` (`:332`). This way the encoder and mixer threads never write `meta.json.tmp`, and two writers never race on it.
  - `#[derive(Debug, PartialEq)] enum Edit { Copy(u64), Skip(u64), Zeros(u64) }`, in frames.
  - `fn audio_edits(timeline_frames: u64, video_gaps: &[Range<u64>], audio_gaps: &[Range<u64>]) -> Vec<Edit>` walks the timeline.
    - Inside a video gap, it `Skip`s whatever the PCM holds there; the PCM holds nothing inside an audio gap.
    - Outside a video gap, it emits `Zeros` inside an audio gap and `Copy` everywhere else.
    - Neighbours of the same kind are merged.
  - `fn apply_edits(path: &Path, edits: &[Edit]) -> std::io::Result<()>` rewrites the file through `<file>.cut`, then renames over it.
  - `fn prepare_tracks(dir: &Path, meta: &Meta, audio_frames: u64) -> Result<Vec<AudioInput>, String>` is the track half of `finalize` (`:569-612`): it applies the edits when `meta` has gaps, then `align`, then builds the main mix.

- [ ] **Step 1: Write the failing tests** in `recorder.rs`:

```rust
/// REC-1: ten minutes asleep in the middle. The picture's jump is written down
/// and taken out of the span, instead of stretching the whole recording.
#[test]
fn a_sleep_in_the_picture_is_a_gap_not_part_of_the_span() {
    let dir = scratch("sleep");
    let meta = Meta { id: "z".into(), fps: 60, ..Meta::default() };
    let before = (0..60).map(|k| frame(k * 16_666, k == 0));
    let after = (0..60).map(|k| frame(600_000_000 + k * 16_666, k == 0));
    let recorder = Recorder::start(&dir, meta, &[], before.chain(after).collect(), 0).unwrap();
    let meta = recorder.finish(Vec::new(), None).unwrap();
    assert_eq!(meta.video_gaps, vec![VideoGap { at_us: 999_960, len_us: 599_000_040 }]);
    assert_eq!(meta.span_us, Some(1_999_920));
    assert_eq!(meta.largest_step_us, Some(16_666), "the gap is not a step");
}

/// REC-1: the mixer's gap is counted, not written. Eight hours asleep used to
/// be 5.5 GB of zeros per track.
#[test]
fn a_sleep_writes_no_zeros() {
    let dir = scratch("nozeros");
    let meta = Meta { id: "n".into(), fps: 60, ..Meta::default() };
    let recorder = Recorder::start(&dir, meta, &[], Vec::new(), 0).unwrap();
    let game = ring_with("game");
    game.push(&[5i16; 12], 0);
    assert!(recorder.begin_window(&[game.clone()], 1250));
    recorder.audio(&game, &[7i16; 12]);
    recorder.end_window(6);
    recorder.skip_window(28_800_000);
    assert!(recorder.begin_window(&[game.clone()], 6_000_001_250));
    recorder.audio(&game, &[8i16; 12]);
    recorder.end_window(6);
    let meta = recorder.finish(Vec::new(), None).unwrap();
    let track = samples(&dir.join(WORK_DIR).join("n").join(&meta.tracks[0].file));
    assert_eq!(track.len(), 36, "zeros were written for the sleep");
    assert_eq!(meta.audio_gaps, vec![AudioGap { at_frame: 12, frames: 28_800_000 }]);
}

/// REC-1, Review Focus: the mixer's gap and the picture's gap never line up to
/// the frame (mixer lag, pacer skip). The seam costs a few zeros and never a shift.
#[test]
fn gaps_that_do_not_line_up_leave_only_zeros_at_the_seam() {
    assert_eq!(
        audio_edits(100, &[40..90], &[44..92]),
        vec![Edit::Copy(40), Edit::Skip(4), Edit::Zeros(2), Edit::Copy(8)]
    );
}

/// REC-1: the other cases, in frames on the recording's timeline.
#[test]
fn audio_edits_follow_both_gaps() {
    assert_eq!(audio_edits(100, &[], &[]), vec![Edit::Copy(100)]);
    // Sleep: picture and sound stopped together.
    assert_eq!(audio_edits(100, &[40..90], &[40..90]), vec![Edit::Copy(50)]);
    // Only the mixer stalled: its zeros come back, and nothing shifts.
    assert_eq!(audio_edits(100, &[], &[30..60]), vec![Edit::Copy(30), Edit::Zeros(30), Edit::Copy(40)]);
    // Only the encoder hung: that sound goes together with the picture it belongs to.
    assert_eq!(audio_edits(100, &[30..60], &[]), vec![Edit::Copy(30), Edit::Skip(30), Edit::Copy(40)]);
}
```
  More tests, written the same way:
  - `a_stall_shorter_than_two_seconds_is_not_a_gap` (REC-1): frames at 0, 16 666 and 1 516 666 give `video_gaps` empty and `span_us == Some(1_533_332)`.
  - `a_source_joining_after_a_sleep_gets_only_the_silence_it_missed` (REC-1): the same set-up as `a_sleep_writes_no_zeros`, then `mic` joins after the skip. Its file has 36 samples, the first 24 of them 0.
  - `a_cut_track_is_exactly_as_long_as_the_picture` (REC-1): a 52-frame ramp goes through the seam edits above and comes out 50 frames long. Frames 0–39 are ramp 0–39, frames 40–41 are 0, and frames 42–49 are ramp 44–51.
  - `a_slept_recording_keeps_its_sound_after_the_wake` (REC-1): the folder has fps 60 and two main-mix tracks. Each track is 48 000 frames of 1, then 48 000 frames of 2. `video_gaps` is `[{ at_us: 1_000_000, len_us: 600_000_000 }]` and `audio_gaps` is `[{ at_frame: 48_000, frames: 28_800_000 }]`. `prepare_tracks(&dir, &meta, 96_000)` gives a mix of `96_000 * CHANNELS` samples, with `mix[0] == 2` and `mix[50_000 * CHANNELS] == 4`.

- [ ] **Step 2: Run** `cargo test --lib -- recorder::`. Expected: FAIL to compile, because `VideoGap`, `skip_window` and `audio_edits` are not found.

- [ ] **Step 3: Implement.**
  - `video()`: a step above `DISCONTINUITY_US` becomes a gap, `{ at_us: previous + period − first, len_us: pts − (previous + period) }`.
    - `period` is `1_000_000 / fps`, and `first` is the start keyframe's pts.
    - Add the gap's length to `closed_us`, push the gap into `meta` and `send(Job::Meta(meta.clone()))`.
    - Such a step does not count for `largest_step_us`.
  - `AudioSide` gets `virtual_frames: u64`.
    - `skip_window(frames)`, once the recorder is attached, pushes `AudioGap { at_frame: timeline, frames }` and persists it through `Job::Meta`. It then adds `frames` to both `timeline` and `virtual_frames`.
    - `audio()` pads a track to `timeline − virtual_frames` instead of `timeline`.
  - `finish`: `span_us = span_of(..) − Σ len_us`, saturating.
  - `finalize` computes `audio_frames` as now, then calls `prepare_tracks`. `prepare_tracks` works as follows:
    - It converts the gaps with `frame = us × 48_000 / 1_000_000`.
    - It sets `timeline_frames = audio_frames + Σ` video gap frames.
    - It rewrites a track only when its edits are more than one `Copy`.
    - It then `align`s every track as today.
  - `pipeline.rs:827-829` calls `recorder.skip_window(frames)`.
  - Keep advancing the mixer's timeline in the gap branch (that is what `skip_window` does). The review's second check (REC-1) warns: if the advance stopped, a stall of the mixer alone would shift the sound for the rest of the file.

- [ ] **Step 4: Run** `cargo test --lib -- recorder:: pipeline::`. Expected: PASS.

- [ ] **Step 5: Manual check on Windows.**
  1. Start a recording and play an A/V sync test video full screen for 2 min.
  2. Start → Sleep, and wait 3 min.
  3. Wake: no lock screen, or unlock within 10 s.
  4. Play 2 more minutes and stop.

  Expected:
  - The file lasts about 4 min, not 7.
  - The beeps sit on their flashes before and after the wake. Step through in the editor, or cut with `ffmpeg -ss <t> -t 3`.
  - Right after the wake, every `.recording\<id>\track_*.pcm` is about 23 MB (2 min), not about 58 MB.

  Repeat, but kill ClippiBoy in Task Manager right after the wake and restart it. The recovered recording leaves the sleep out too, because the gaps were already in `meta.json`.

- [ ] **Step 6: Commit** `Recordings: a sleep in the middle no longer ruins the recording — the picture's jump is written down as a gap and left out together with the sound of that stretch, nothing is written while the PC sleeps, and the file plays at its real speed and stays in sync after the wake`

---

### Task 4: One resampler for sounding and silent packets (AUD-V2, AUD-7)

**Files:**
- Create: `src-tauri/src/audio/resample.rs`
- Modify: `src-tauri/src/audio/mod.rs:4-7` (`pub mod resample;`)
- Modify: `src-tauri/src/audio/capture.rs:280-342` (`convert` takes a `&mut Resampler`; the per-packet linear block at `:325-341` goes), `:600-686` (one `Resampler` per attempt, built from `format.sample_rate`; the SILENT path at `:676-678`)

**Interfaces:**
- Produces (`audio::resample`):
  - `pub struct Resampler`, with `pub fn new(device_rate: u32) -> Self`.
  - `pub fn process(&mut self, stereo: &[f32], out: &mut Vec<f32>)` appends 48 kHz stereo to `out`.
  - `pub fn silence(&mut self, frames: usize, out: &mut Vec<f32>)` does the same for zeros.
  - At 48 kHz, input passes through untouched.

The resampler carries its phase and its last frame across packets. The signature does not determine that, so here is the algorithm:

```rust
// `pos`: where the next output frame sits, in input frames, counted from the
// first frame of `stereo`; −1 is `prev`, the last frame of the block before.
// `step` = device_rate / 48 000. `pos` starts at 0.0, `prev` at silence.
let n = stereo.len() / CHANNELS;            // return early when 0
let at = |i: isize| if i < 0 { self.prev } else { [stereo[2 * i as usize], stereo[2 * i as usize + 1]] };
while self.pos < n as f64 - 1.0 {
    let i = self.pos.floor() as isize;
    let frac = (self.pos - i as f64) as f32;
    let (a, b) = (at(i), at(i + 1));
    out.extend([a[0] + (b[0] - a[0]) * frac, a[1] + (b[1] - a[1]) * frac]);
    self.pos += self.step;
}
self.pos -= n as f64;
self.prev = at(n as isize - 1);
```
`silence(frames, out)` runs the same loop over zeros (reuse a zero buffer).

- [ ] **Step 1: Write the failing tests** in `resample.rs`:

```rust
/// AUD-7: 448-frame packets at 44.1 kHz each lost 0.62 frames (1 270 ppm).
#[test]
fn odd_packets_at_44k_lose_no_fraction_of_a_frame() {
    let mut resampler = Resampler::new(44_100);
    let (packet, mut out) = (vec![0.25f32; 448 * CHANNELS], Vec::new());
    for _ in 0..1_000 { resampler.process(&packet, &mut out); }
    // 448 000 × 48 000 / 44 100 = 487 619.05; the last fraction waits for the next packet.
    assert!((out.len() / CHANNELS).abs_diff(487_619) <= 2, "{} frames", out.len() / CHANNELS);
}

/// AUD-V2: SILENT packets from a 96 kHz device added 20 ms of timeline per
/// 10 ms packet, and the source then lagged by up to a second for minutes.
#[test]
fn silent_packets_at_96k_take_only_their_real_time() {
    let (mut resampler, mut out) = (Resampler::new(96_000), Vec::new());
    for _ in 0..100 { resampler.silence(960, &mut out); }
    assert!((out.len() / CHANNELS).abs_diff(48_000) <= 1);
}
```
  More tests:
  - `a_tone_resampled_in_pieces_matches_it_resampled_whole` (AUD-7): 1 s of a 1 kHz sine at 44 100 Hz, once whole and once in pieces of 441, 448, 100, 7 and 1 000 frames, cycling. The lengths differ by at most one frame, and over their common length no sample differs by more than `1e-5`.
  - `silent_and_sounding_packets_land_where_their_stamps_say` (AUD-V2): for 44 100 and 96 000 Hz, take 1 000 packets of `rate / 100` frames that alternate between `silence` and `process(0.5)`. Write each into `SampleRing::new(48_000, 2)` with the honest stamp `k × 100_000`. Assert `(ring.end_100ns().unwrap() - 100_000_000).abs() < 10_000`.
  - `a_48k_device_passes_through_untouched`: `process(&x)` appends exactly `x`.

- [ ] **Step 2: Run** `cargo test --lib -- audio::resample`. Expected: FAIL to compile (module missing).

- [ ] **Step 3: Implement.**
  - Write `resample.rs` as above.
  - `convert(raw, frames, format, resampler, out)` folds to stereo as now, then calls `resampler.process`.
  - The SILENT branch becomes `converted.clear(); resampler.silence(frames as usize, &mut converted);`.

- [ ] **Step 4: Run** `cargo test --lib -- audio::`. Expected: PASS.

- [ ] **Step 5: Manual check on Windows.**
  1. Sound settings → the microphone → Advanced → "2 channel, 16 bit, 44100 Hz". Save a clip of the sync video while counting aloud: the voice is in sync with the picture.
  2. Set an output device to 96 000 Hz and record it as "Windows default" output. Leave it silent for 2 min, then play the sync video, and save after 1 min.

  Expected: the beeps sit on the flashes. Before the fix, the lag was up to about 1 s. Also check that the log has no new `audio:` warnings.

- [ ] **Step 6: Commit** `Audio: devices at 44.1 or 96 kHz stay in time — a resampler that carries its phase across packets keeps every fraction of a frame, and silent packets from such devices are converted to 48 kHz like sounding ones instead of counting their own frames, which left a 96 kHz device up to a second late after a pause`

---

### Task 5: Place a stampless packet by its queue, not by its read-out time (AUD-2)

**Files:**
- Modify: `src-tauri/src/audio/capture.rs:47-78` (flags, `capture_time` next to `usable_stamp`), `:655-671` (call site); tests `:700-762`

**Interfaces:**
- Produces:
  - `pub const BUFFER_FLAG_SILENT: u32 = 0x2;` and `pub const BUFFER_FLAG_TIMESTAMP_ERROR: u32 = 0x4;` The SILENT check at `:676` uses the first.
  - `pub fn capture_time(stamped: i64, flags: u32, now: i64, queued_frames: u32, device_rate: u32, trust: &mut bool) -> i64`.
    - It returns `stamped` when the device is trusted, the stamp is non-zero and the stamp is not flagged.
    - In every other case it returns the estimate `now − queued_frames × 10⁷ / device_rate`.
    - The 2 s distrust rule of `usable_stamp`, which stays with its tests, runs only on unflagged non-zero stamps.

The explainer suggests "no stamp, append by count" for 0 stamps. This plan estimates them instead: a device that never fills the stamp would otherwise run on count alone, and the ring's pull towards the clock (`ring.rs:156-163`) needs stamps to correct its crystal.

- [ ] **Step 1: Write the failing tests** in `capture.rs`:

```rust
/// AUD-2: an empty stamp used to be the read-out time, 10 ms or a whole stall late.
#[test]
fn a_missing_stamp_is_estimated_from_what_is_still_queued() {
    let mut trust = true;
    assert_eq!(capture_time(0, 0, NOW, 4_800, 48_000, &mut trust), NOW - 1_000_000);
    assert!(trust);
}

/// AUD-2: a stamp the device flags as wrong is not used, and costs no trust.
#[test]
fn a_stamp_flagged_as_wrong_is_not_used() {
    let mut trust = true;
    let flagged = capture_time(NOW + 3_000_000, BUFFER_FLAG_TIMESTAMP_ERROR, NOW, 480, 48_000, &mut trust);
    assert_eq!(flagged, NOW - 100_000);
    assert!(trust);
}

/// AUD-2: on the fallback clock, a stalled reader stamped the first queued
/// packet "now": 150 ms of fake dropout, then the source sat late for over a minute.
#[test]
fn a_stalled_reader_on_the_fallback_clock_does_not_drag_the_source_late() {
    let ring = SampleRing::new(48_000, 2);
    let block = vec![0.5f32; 480 * CHANNELS];
    let mut trust = false; // a device on the fallback clock
    let mut k = 0i64; // packet k covers [k·10 ms, (k+1)·10 ms)
    while k < 600 {
        // Read 2 ms after a packet completes. At 5.2 s the reader stalls and
        // then drains the 16 packets that piled up.
        let burst: i64 = if k == 520 { 16 } else { 1 };
        let now = (k + burst) * 100_000 + 20_000;
        for queued in (1..=burst).rev() {
            ring.write(&block, capture_time(0, 0, now, queued as u32 * 480, 48_000, &mut trust));
            k += 1;
        }
    }
    let end = ring.end_100ns().unwrap();
    assert!((end - 60_020_000).abs() < 10_000, "the source ends {} ms off", (end - 60_020_000) / 10_000);
    let mut out = vec![0.0f32; 480 * CHANNELS];
    ring.read_window(52_500_000, &mut out);
    assert!(out.iter().all(|s| *s == 0.5), "a fake dropout was filled in after the stall");
}
```
  More tests:
  - `a_good_stamp_is_still_taken_as_it_is`: `capture_time(NOW - 50_000, 0, NOW, 480, 48_000, &mut true)` returns `NOW - 50_000`.
  - `a_distrusted_device_is_placed_by_its_queue`: with `trust = false`, `capture_time(NOW - 50_000, 0, NOW, 441, 44_100, ..)` returns `NOW - 100_000`.

- [ ] **Step 2: Run** `cargo test --lib -- audio::capture`. Expected: FAIL to compile (`capture_time` not found).

- [ ] **Step 3: Implement.**
  - After `GetBuffer` (`:647-655`), set `let queued = unsafe { client.GetCurrentPadding() }.unwrap_or(frames).max(frames);`. MS Learn describes the padding as "the amount of valid, unread data that the endpoint buffer currently contains", so it includes this packet.
  - Replace `usable_stamp(..)` at `:663` with `capture_time(stamped, flags, now, queued, format.sample_rate, &mut trust_qpc)`. The fallback-clock warning stays as it is.

- [ ] **Step 4: Run** `cargo test --lib -- audio::`. Expected: PASS.

- [ ] **Step 5: Manual check on Windows.**
  - First, confirm that the padding counts the whole backlog. Add a temporary `log::debug!` for when `queued > frames`, and suspend ClippiBoy for about 300 ms (Process Explorer → Suspend, Resume). The log should show `queued` of about 20 × 480. Do not commit the line.
  - If a device in the setup shows the fallback-clock warning (`engine.rs:287-308`), record 5 min with a game running and the sync video, then compare the sync at the start and at the end. Without such a device, the simulation test is the evidence; say so in the commit body.

- [ ] **Step 6: Commit** `Audio: a source without usable timestamps is placed by what is still waiting in its buffer instead of by the moment it was read, so a stalled audio thread no longer inserts a fake dropout and leaves the source up to 190 ms late for over a minute; stamps the device flags as wrong are not used`

---

### Task 6: One zero point for every output (SAVE-7)

**Files:**
- Modify: `src-tauri/src/muxer.rs:279-396` (`encode`; pull out `arguments`), `:356`; tests `:547`
- Modify: `src-tauri/src/recorder.rs:616-624` (the call to `encode`)
- Modify: `src-tauri/src/edit.rs:729`; tests next to `:1416`

**Interfaces:**
- Produces (`muxer.rs`):
  - `pub enum VideoInput { Raw { path: PathBuf, rate: String } }`. Task 7 adds `Timestamped(PathBuf)`.
  - `pub(crate) fn arguments(video: &VideoInput, wavs: &[AudioInput], pending: &Path, stem_outputs: &[PathBuf]) -> Vec<String>`. `stem_outputs` is empty when no stems are kept. `encode` fills it with `stems::track_path(clip_id, i)` when `wavs.len() > 1`.
  - `encode(video: &VideoInput, wavs, clip_id, output, length_ms, on_progress)`. Its callers are `muxer.rs:212` and `recorder.rs:616`.
- The review asks for this `arguments()` before stage 2 ("No argument builder in `muxer::encode`"). The stage-2 plan does not wait for SAVE-7: it probes the clean clip's video start (its "SAVE-7, decided here"). Its offsets therefore stay right whichever policy this task picks; the probe then reads 0.

- [ ] **Step 1: Measure first** (Windows, no code change).
  1. Give the microphone its own track (⧉) so that a save has two or more audio inputs, and save a clip.
  2. Run `ffprobe -v error -show_entries stream=codec_type,start_time -of compact <clip>.mp4`, and the same on `%APPDATA%\ClippiBoy\tracks\<id with _ for ->\0.m4a`.
  3. In the editor, change only a level and save. Do it twice, then probe the clip again.

  If SAVE-7 is real, the probes show: clip `video|start_time=0.021333` and `audio|start_time=0.000000`; stem `0.000000`; after the two saves, `video|start_time=0.064000`. Write the numbers down for the commit message.

- [ ] **Step 2: Write the failing tests.**
  - `the_clip_command_is_the_one_encode_built_before` guards the refactor. Take `Raw { "C:/c/x.part.h264", "60" }`, a WAV and a raw PCM input, and two stem outputs. Assert `assert_eq!` against the literal list `muxer.rs:311-393` builds today, element by element. Write that list by hand from the code, not from the new function.
  - In `muxer.rs`, add the test helpers `raw(path, rate) -> VideoInput`, `two_wavs() -> Vec<AudioInput>` (a WAV at `C:/c/mix.wav` and a raw PCM at `C:/c/mic.pcm`) and `two_stems() -> Vec<PathBuf>` (`C:/d/0.m4a`, `C:/d/1.m4a`). Then:

```rust
/// SAVE-7: `make_zero` on the clip output alone shifted its picture by the
/// AAC priming, while the stems kept theirs. Every output follows one policy.
#[test]
fn every_output_of_a_save_shares_one_zero_point() {
    let args = arguments(&raw("C:/c/x.part.h264", "60"), &two_wavs(), Path::new("C:/c/x.part.mp4"), &two_stems());
    assert_eq!(args.iter().filter(|a| *a == "-avoid_negative_ts").count(), 0); // policy (a)
}
```
  - In `edit.rs`:

```rust
/// SAVE-7: a sound-only edit no longer moves the picture another 21.33 ms.
#[test]
fn an_edit_leaves_the_zero_point_where_it_is() {
    let plan = plan(None, true, 30_000, Trim::whole(30_000));
    let list = vec![track(0, true), track(1, true)];
    let args = arguments(&plan, Path::new("C:/clips/x.mp4"), &list, &[], Path::new("C:/clips/x.neu.mp4"), EncoderId::X264, 70);
    assert!(!args.iter().any(|a| a == "-avoid_negative_ts"));
}
```
  If Step 1 does not confirm SAVE-7, keep `make_zero`. The two policy tests then pin "the clip's own output and the edit's output have it once each, the stem outputs do not". The commit message records the measured numbers.

- [ ] **Step 3: Run** `cargo test --lib -- muxer:: edit::`. Expected: FAIL to compile (`arguments`, `VideoInput` not found).

- [ ] **Step 4: Implement.** Pull the list out into `arguments` unchanged, until the golden test passes. Then apply the policy:
  - **(a), preferred:** drop the `make_zero` pair at `muxer.rs:356` and `edit.rs:729`. Edit lists then carry the priming in every file.
  - **(b), only if (a) fails Step 6:** add `-avoid_negative_ts make_zero` to every stem output. Change the policy test to `== 1 + stems.len()`, and the edit test to `== 1`.

- [ ] **Step 5: Run** `cargo test --lib`. Expected: PASS.

- [ ] **Step 6: Manual check on Windows** (only with (a)).
  1. Save a sync-video clip with two sources.
  2. Play it in ClippiBoy's player and the editor preview (WebView2), in Explorer's preview or Films & TV, and in Discord after an upload, on the desktop client and on a phone.
  3. In every player the beeps must sit on the flashes, with no 21 ms click of priming at the start.
  4. Repeat Step 1's probes. Expected: clip video `0.000000` and audio `0.000000`; after two level-only saves, still `0.000000`.

  If any player plays the priming or chokes on the edit list, switch to (b) and repeat. Clips saved before this change keep a constant 21 ms lead after their next edit. That lead no longer grows, and it is below the roughly 45 ms anyone notices.

- [ ] **Step 7: Commit**, depending on Step 1:
  - If SAVE-7 was confirmed: `Saving: every output of a save and an edit shares one zero point — the clip no longer shifts its picture by the AAC encoder's 21 ms priming while its tracks do not, so sound-only edits no longer move the sound 21 ms ahead each time (measured before: video 0.021333, stem 0, after two edits 0.064)`
  - If not: `Saving: the clip's ffmpeg command is built by a function of its own and pinned in tests; the 21 ms zero-point question from the review measured as not real (…)`

---

### Task 7: Clips keep their real timestamps (BUF-6, AUD-5)

**Files:**
- Create: `src-tauri/src/ts.rs`
- Modify: `src-tauri/src/lib.rs:1-40` (`pub mod ts;`)
- Modify: `src-tauri/src/buffer.rs:13-14` (`jump_limit_us`, unless the stage-2 plan already added it)
- Modify: `src-tauri/src/muxer.rs` (`VideoInput::Timestamped`, `keeps_timestamps`, `build` writes `.part.ts`; the docs at `:1-12`, `:237-250` and `:314-315` no longer say "the rate supplies the timestamps")
- Modify: `src-tauri/src/pipeline.rs:564-580` (`ClipSnapshot.timestamped`), `:1039-1054` (span; the doc at `:1044-1048`)
- Modify: `src-tauri/examples/record-probe.rs` (a `--hole` option)

**Interfaces:**
- Consumes: `arguments`, `VideoInput` (Task 6); `ClipSnapshot.largest_step_us` (Task 2); `Shared.keyframe_seconds` (Task 1).
- Produces (`ts.rs`):
  - `pub const VIDEO_PID: u16 = 0x100;`, `const PMT_PID: u16 = 0x1000;`, `pub const PTS_OFFSET_90K: u64 = 63_000;`. 0.7 s is the gap between PCR and PTS that ffmpeg's own TS muxer leaves.
  - `pub struct TsWriter<W: Write>`, with `pub fn new(out: W) -> Self`.
  - `pub fn write_frame(&mut self, pts_us: i64, keyframe: bool, data: &[u8]) -> std::io::Result<()>`. `pts_us` is at least 0, relative to the first frame, and strictly increasing.
  - `pub fn finish(self) -> std::io::Result<W>`.
  - `pub fn read_pts(bytes: &[u8]) -> Vec<u64>` returns the PES PTS on `VIDEO_PID`, in order. The tests here use it, and Task 8's `span_us` does too.
  - `fn crc32(data: &[u8]) -> u32` is CRC-32/MPEG-2: polynomial 0x04C11DB7, initial value 0xFFFF_FFFF, no reflection, no final XOR.
- Produces (`muxer.rs`):
  - `VideoInput::Timestamped(PathBuf)` gives the arguments `-f mpegts -i <path>`, with no `-r`.
  - `pub fn keeps_timestamps(video_pts_us: &[i64], largest_step_us: i64, jump_limit_us: i64) -> bool` is true iff the pts are strictly increasing and `largest_step_us ≤ jump_limit_us`.
    - A 3 s hang is a jump the ring keeps, so it is kept as a held frame, in sync.
    - Only a jump the ring's BUF-5 cut would drop (a sleep) falls back to the raw path, and after stage 2 such a jump never reaches a snapshot.
- Produces (`buffer.rs`), unless stage 2 already added it: `pub fn jump_limit_us(keyframe_seconds: u32) -> i64`, which returns `(3 × keyframe_seconds as i64 × 1_000_000).max(1_000_000)`.
- Produces (`pipeline.rs`): `ClipSnapshot.timestamped: bool`. When it is true, `audio_frames` comes from `span_of(first, last, fps)` itself, not through `timing`.

The TS layout, which the signatures do not fix:
- Packets are 188 bytes, each starting with the sync byte 0x47.
- The PAT (PID 0, program 1 → `PMT_PID`) and the PMT (PCR PID = `VIDEO_PID`, one stream of type 0x1B on `VIDEO_PID`, no descriptors) are single-packet sections with their CRC, padded with 0xFF. They go before the first frame and before every keyframe.
- Each frame is one PES:
  - `00 00 01 E0`, then `PES_packet_length` 0 (allowed for video), then `0x80 0x80` (PTS only), then a header length of 5.
  - PTS = `(pts_us × 9 + 50) / 100 + PTS_OFFSET_90K`, in the 5-byte form with marker bits (`0010` prefix).
  - The payload is the Annex-B access unit exactly as it came. The first frame gets `sequence_header` in front, as the raw path does. No AUD is added: ffmpeg's parser splits access units without one, and the probe in Step 5 checks that frames come out one to one.
- The first TS packet of a keyframe PES has an adaptation field with `random_access_indicator` and PCR = PTS − `PTS_OFFSET_90K` (extension 0).
- The last TS packet of a PES is filled to 188 bytes with adaptation-field stuffing, never with payload padding. Each PID has its own continuity counter (mod 16), counted on packets with payload only.
- No DTS: `keeps_timestamps` guarantees increasing pts. Everything else goes through `Raw` as today: reordered streams, and, until BUF-5 lands, snapshots that span a discontinuity.

- [ ] **Step 1: Write the failing tests** in `ts.rs`:

```rust
/// BUF-6: each frame's own time reaches ffmpeg, at 90 kHz and 0.7 s in.
#[test]
fn a_frame_carries_its_time_at_90_khz() {
    let mut ts = TsWriter::new(Vec::new());
    for (pts, key) in [(0, true), (16_666, false), (1_516_666, false)] {
        ts.write_frame(pts, key, &[0, 0, 0, 1, 0x65, 0x88]).unwrap();
    }
    assert_eq!(read_pts(&ts.finish().unwrap()), vec![63_000, 64_500, 199_500]);
}

/// BUF-6: a 1 s hole stays a 1 s hole, instead of being spread over the clip.
#[test]
fn a_hole_stays_a_hole() {
    let mut ts = TsWriter::new(Vec::new());
    for tick in (0..60).chain(120..180) {
        ts.write_frame(tick * 1_000_000 / 60, tick % 120 == 0, &[0, 0, 0, 1, 0x41]).unwrap();
    }
    let pts = read_pts(&ts.finish().unwrap());
    let steps: Vec<u64> = pts.windows(2).map(|w| w[1] - w[0]).collect();
    assert_eq!(steps.iter().filter(|s| (1_499..=1_501).contains(*s)).count(), 118);
    assert!((91_490..=91_510).contains(&steps[59]), "the hole is {} ticks", steps[59]);
}

/// The check value of CRC-32/MPEG-2.
#[test]
fn the_tables_carry_the_mpeg2_crc() {
    assert_eq!(crc32(b"123456789"), 0x0376_E6E7);
}
```
  More tests:
  - `every_packet_is_188_bytes_with_its_sync_byte`: frames of 10, 500 and 5 000 bytes. The output length is a multiple of 188, and every packet starts with 0x47.
  - `continuity_counters_count_per_pid`: a 5 000-byte frame. The video packets count 0, 1, 2, … mod 16, and the PAT and PMT counters are their own.
  - `tables_come_before_every_keyframe`: frames key, non-key, key. The PIDs run 0, 0x1000, 0x100…, 0x100…, 0, 0x1000, 0x100….

  In `muxer.rs`:

```rust
/// BUF-6: a snapshot that can carry its timestamps does, including a 3 s hang,
/// which used to be spread over the whole clip.
#[test]
fn an_ordinary_snapshot_keeps_its_timestamps() {
    let limit = jump_limit_us(2); // 6 s
    assert!(keeps_timestamps(&[0, 16_666, 33_333, 1_033_333], 1_000_000, limit));
    assert!(keeps_timestamps(&[0, 16_666, 3_016_666], 3_000_000, limit));
}

/// Review Focus: B-frames arrive out of order and keep the raw path.
#[test]
fn reordered_frames_keep_the_raw_path() {
    assert!(!keeps_timestamps(&[0, 50_000, 16_666, 33_333], 33_334, jump_limit_us(2)));
}

/// Review Focus: a sleep inside a snapshot keeps today's path, until BUF-5
/// (stage 2) cuts such snapshots.
#[test]
fn a_snapshot_across_a_discontinuity_keeps_the_raw_path() {
    assert!(!keeps_timestamps(&[0, 16_666, 600_000_000], 599_983_334, jump_limit_us(2)));
    assert_eq!((jump_limit_us(0), jump_limit_us(4)), (1_000_000, 12_000_000));
}

/// BUF-6: a timestamped input is read as MPEG-TS, with no rate.
#[test]
fn a_timestamped_input_is_read_without_a_rate() {
    let input = VideoInput::Timestamped("C:/c/x.part.ts".into());
    let args = arguments(&input, &[], Path::new("C:/c/x.part.mp4"), &[]);
    assert!(args.windows(4).any(|w| w == ["-f", "mpegts", "-i", "C:/c/x.part.ts"]));
    assert!(!args.iter().any(|a| a == "-r"));
}
```

- [ ] **Step 2: Run** `cargo test --lib -- ts:: muxer::`. Expected: FAIL to compile (module `ts` missing).

- [ ] **Step 3: Implement.**
  - Write `ts.rs` to the layout above.
  - In `Pipeline::snapshot`, compute `timestamped = muxer::keeps_timestamps(&video_pts, largest_step, buffer::jump_limit_us(shared.keyframe_seconds))`.
  - If the stage-2 plan has landed, its test that "cam off saves with today's command" now expects the TS input. That change is intended.
  - In `muxer::build`:
    - When the snapshot is timestamped, write `scratch("ts")` through `TsWriter` over a 1 MiB `BufWriter`, with each pts relative to `packets[0].pts_us`, and call `encode` with `VideoInput::Timestamped`.
    - Otherwise, write the `.h264` exactly as today.
    - Delete either scratch file at the end, as now.

- [ ] **Step 4: Run** `cargo test --lib`. Expected: PASS.

- [ ] **Step 5: Real file.**
  1. Give `record-probe` a `--hole` option. After `snapshot`, it drains packets 30..90, which are all non-key at a 2 s GOP. Span and start stay as they are; that is the point.
  2. Run `cargo run --example record-probe -- --encoder <yours> --hole 10`.
  3. Run `ffprobe -v error -select_streams v:0 -show_entries packet=pts_time -of csv=p=0` on the clip.

  Expected:
  - Exactly one step of about 1.017 s (61 periods), and every other step about 16.7 ms.
  - The packet count equals the snapshot's minus 60.
  - The format duration is about the span.
  - Before the fix, every step was the same.
  - The picture breaks after the hole; this clip is only for timestamps.

  Then run `trim-probe` on a copy of a normal clip saved by the app: trim and undo stay within its tolerance.

- [ ] **Step 6: Manual check on Windows.**
  1. Save three clips of the sync video during a game. Play them in ClippiBoy, in Explorer and in Discord.
  2. Expected: in sync, and `ffprobe -show_entries stream=avg_frame_rate` says 60/1.
  3. Trim, export and share one of them: all still work.

  There is no reliable way to force a stall in the middle of a clip by hand. The `--hole` probe is the real-file evidence.

- [ ] **Step 7: Commit** `Clips: the picture keeps its real timestamps — the encoded frames go to ffmpeg in a small MPEG-TS written by ClippiBoy instead of a raw stream spread at one rate, so a stall in the middle of a clip shows as a held frame and no longer pulls picture and sound up to half its length apart; reordered streams and snapshots across a sleep keep the old path`

---

### Task 8: Recordings keep their real timestamps too (BUF-6 in `recorder.rs:566`, REC-6)

**Files:**
- Modify: `src-tauri/src/recorder.rs:1-20` (doc), `:40` (`VIDEO_TS`), `:96-103` (`Job`), `:141-213` (`start`, the writer), `:223-233` (`video`), `:383-421` (`run_job`), `:550-633` (`finalize`); tests
- Modify: `src-tauri/src/buffer.rs:75-84` (`push` notices reordering), `:151-157` (`clear`)
- Modify: `src-tauri/src/pipeline.rs:998-1018` (`start_recording`)
- Modify: `src-tauri/src/ts.rs` (`span_us`)

**Interfaces:**
- Consumes: `TsWriter`, `read_pts` and `VideoInput::Timestamped` (Task 7); `Recorder.closed_us` and the gaps (Task 3); `prepare_tracks` (Task 3).
- Produces:
  - `Recorder::start(clip_dir, meta, sequence_header, backlog, start_100ns, timestamped: bool)`. When `timestamped` is false, the recorder writes `video.h264` as today.
  - `const VIDEO_TS: &str = "video.ts";`
  - `Job::Video { data: Arc<[u8]>, pts_us: i64, keyframe: bool }` and `Job::Header(Arc<[u8]>)`.
    - The writer holds `enum VideoOut { Ts(TsWriter<BufWriter<File>>), Raw(BufWriter<File>) }`.
    - With TS, a header is kept and put in front of the next frame.
  - `Recorder::video` sends `pts_us − first − closed_us`, so the written times run gap-free from 0.
  - `ReplayBuffer::reorders(&self) -> bool` turns true at the first video packet whose pts lies below `newest_pts` (checked before `newest_pts` moves). `clear` resets it. `start_recording` passes `!ring.reorders()`.
  - `pub fn span_us(path: &Path, fps: u32) -> Option<u64>` in `ts.rs`. It reads the first 64 KiB and the last 1 MiB, aligned to 188 from the start of the file, and ignores a cut-off last packet. It returns `(last − first) × 100 / 9 + 1_000_000 / fps`.
  - `fn recorded_video(dir: &Path, meta: &Meta) -> Result<(muxer::VideoInput, u64), String>` returns the input and its span in µs.
    - If `video.ts` is there: `Timestamped`, with `meta.span_us` or else `ts::span_us`.
    - Otherwise: `Raw`, with `timing(frames, ..)` as today; `frames` comes from `meta` or `count_frames`.
- Stage 4 (recordings with the cam) assumes `video.h264` (its Task 1 "Done when"). After this task it must read `video.ts`. Whether its cam segments use `TsWriter` too is a stage-4 decision.

- [ ] **Step 1: Write the failing tests.** In `recorder.rs`:

```rust
/// BUF-6: a recording keeps the time of every frame.
#[test]
fn a_recording_keeps_the_times_of_its_frames() {
    let dir = scratch("ts");
    let meta = Meta { id: "t".into(), fps: 60, ..Meta::default() };
    let backlog = vec![frame(0, true), frame(16_666, false), frame(516_666, false)];
    let recorder = Recorder::start(&dir, meta, &[], backlog, 0, true).unwrap();
    let _ = recorder.finish(Vec::new(), None);
    let written = std::fs::read(dir.join(WORK_DIR).join("t").join(VIDEO_TS)).unwrap();
    assert_eq!(ts::read_pts(&written), vec![63_000, 64_500, 109_500]);
}

/// REC-1 together with BUF-6: a gap is taken out of the written times.
#[test]
fn a_closed_gap_is_taken_out_of_the_written_times() {
    let dir = scratch("tsgap");
    let meta = Meta { id: "g".into(), fps: 60, ..Meta::default() };
    let backlog = [0, 16_666, 600_000_000, 600_016_666].map(|pts| frame(pts, pts % 600_000_000 == 0)).to_vec();
    let recorder = Recorder::start(&dir, meta, &[], backlog, 0, true).unwrap();
    let _ = recorder.finish(Vec::new(), None);
    let written = std::fs::read(dir.join(WORK_DIR).join("g").join(VIDEO_TS)).unwrap();
    assert_eq!(ts::read_pts(&written), vec![63_000, 64_500, 66_000, 67_500]);
}

/// Review Focus: a folder from the version before finalizes as it always did.
#[test]
fn a_folder_from_before_the_update_still_finalizes_raw() {
    let dir = scratch("old");
    std::fs::write(dir.join("video.h264"), [0u8; 4]).unwrap();
    let meta = Meta { fps: 60, frames: Some(1800), span_us: Some(30_000_000), ..Meta::default() };
    let (video, span) = recorded_video(&dir, &meta).unwrap();
    assert!(matches!(&video, muxer::VideoInput::Raw { rate, .. } if rate == "1800000000/30000000"));
    assert_eq!(span, 30_000_000);
}
```
  More tests:
  - `a_reordering_encoder_records_raw`: with `timestamped = false`, the folder has `video.h264` and no `video.ts`.
  - In `buffer.rs`, `a_ring_notices_reordered_frames` (Review Focus): video packets at 0, 50 000 and 16 666 turn `reorders()` true. With 0, 16 666 and 33 333 it stays false.
  - In `ts.rs`, `the_span_of_a_ts_file_is_read_from_its_ends` (REC-6): write 120 frames at `k × 16_666` to a file and append 100 stray bytes, as a crash would leave them. `span_us(&path, 60)` lies in `1_999_900..=1_999_940`.
  - The existing `Recorder::start(...)` calls in the tests gain `, true`.

- [ ] **Step 2: Run** `cargo test --lib -- recorder:: buffer:: ts::`. Expected: FAIL to compile (`Recorder::start` takes 5 arguments; `recorded_video`, `span_us` and `reorders` not found).

- [ ] **Step 3: Implement** as described in Interfaces.
  - `finalize` becomes:
    1. `let (video, span_us) = recorded_video(dir, meta)?;`
    2. `audio_frames = span_us × 48 000 / 1e6`
    3. `prepare_tracks`
    4. `encode(&video, ..)`
  - Update the module doc's folder listing to `video.ts`, keeping `video.h264` as the raw and old form.

- [ ] **Step 4: Run** `cargo test --lib`. Expected: PASS.

- [ ] **Step 5: Manual check on Windows.**
  1. Record 3 min of the sync video. It is in sync at the start and at the end, and `ffprobe` shows `avg_frame_rate` 60/1.
  2. Start a recording and play the sync video for 2 min. Kill ClippiBoy in Task Manager and restart it. The recovered recording is in sync at its end, and its length matches the time recorded within 1 s. Before, at the nominal rate, it drifted by its frame loss (36 s an hour at 1 %).
  3. Repeat Task 3's sleep check: same result.
  4. Start ClippiBoy from PowerShell with `$env:CLIPPIBOY_BFRAMES = 2`. If the encoder profile in the log shows the B-frames were taken, a recording leaves `video.h264` in its work folder, and the result plays.

- [ ] **Step 6: Commit** `Recordings: the picture keeps its real timestamps too — the recorder writes a small MPEG-TS instead of a raw stream, so stalls no longer stretch the whole file, and a recording finished after a crash plays at its real speed instead of the nominal rate; folders from before this version and encoders with B-frames keep the raw path`

---

### Task 9: Mixer and rings count from running totals (AUD-6, AUD-V1, REC-5)

**Files:**
- Modify: `src-tauri/src/pipeline.rs:66-74` (`TrackInner`), `:122-220` (`push`, `skip`, `window`), `:781-869` (`mix_loop`); tests, including `:1239-1244`, where `inner.start_100ns` becomes `inner.start_100ns()`

**Interfaces:**
- Consumes: `Shared.mixed.publish` (Task 1).
- Produces:
  - `struct MixClock { origin_100ns: i64, frames: u64 }`, with:
    - `fn new(origin_100ns: i64) -> Self`
    - `fn at(&self) -> i64`, which returns `origin + frames × 10⁷ / 48 000` (computed in `i128`)
    - `fn frames_until(&self, target_100ns: i64) -> usize`
    - `fn advance(&mut self, frames: usize)`

    `mix_loop` uses it in both branches, in place of `next_100ns` (`:802-806`, `:831`, `:868`).
  - `TrackInner` keeps `origin_100ns: i64` and `dropped_frames: u64` in place of `start_100ns`. `fn start_100ns(&self) -> i64` returns `origin + dropped × 10⁷ / 48 000`. Priming sets the origin and zeroes `dropped`, and a trim adds to `dropped`.
  - Change both sides together. The review warns that making only the trim at `:165` exact would make buffer clips drift instead. With only the mixer exact, the trim's own rounding builds up the same way.

- [ ] **Step 1: Write the failing tests** in `pipeline.rs`:

```rust
/// AUD-6: rounding each window lost a third of a tick per window, about 10 ms
/// an hour, and recordings carried it into the file.
#[test]
fn the_mixer_clock_does_not_drift_over_an_hour() {
    let mut clock = MixClock::new(7_000_000_000_000);
    let (mut seed, mut total) = (1u64, 0u64);
    while total < 3_600 * 48_000 {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let frames = 480 + (seed >> 33) as usize % 49; // windows of 10.0–11.0 ms
        clock.advance(frames);
        total += frames as u64;
    }
    let exact = (total as i128 * 10_000_000 / 48_000) as i64;
    assert!((clock.at() - 7_000_000_000_000 - exact).abs() <= 1);
}

/// AUD-V1: counted silence broke the cancellation between the mixer and the
/// ring's trim, so a track that is quiet half the time drifted late over hours.
#[test]
fn a_track_quiet_half_the_time_stays_on_the_clock_for_an_hour() {
    let ring = TrackRing::new("a".into(), "A".into(), 10);
    let (mut clock, mut seed, mut last) = (MixClock::new(0), 1u64, 0i64);
    while clock.at() < 36_000_000_000 {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let frames = 480 + (seed >> 33) as usize % 49;
        let from = clock.at();
        let sounding = (from / 10_000_000) % 2 == 0;
        // A ramp starting at 1, so a window shows which frame of the block it hit.
        let block: Vec<i16> = (0..frames)
            .flat_map(|i| { let v = if sounding { i as i16 + 1 } else { 0 }; [v, v] })
            .collect();
        ring.push(&block, from);
        if sounding { last = from; }
        clock.advance(frames);
    }
    let w = ring.window(last, 2);
    assert!(w[0] == 1 || w[0] == 2 || w[CHANNELS] == 1, "the last block reads back at {w:?}");
}
```
  Both simulations push about 340 000 windows and run for a few seconds in a debug build.

- [ ] **Step 2: Run** `cargo test --lib -- pipeline::tests`. Expected: FAIL to compile (`MixClock` not found).

- [ ] **Step 3: Implement** as described in Interfaces. Update the comment at `:787-788`: the clock was never accumulated without error; now it is.

- [ ] **Step 4: Run** `cargo test --lib -- pipeline:: recorder::`. Expected: PASS. The recorder's tests are unchanged, because it counts samples.

- [ ] **Step 5: Manual check on Windows** (optional; it takes two hours). Record a looping sync-test video for 2 h. Check that the beep at 2:00:00 sits on its flash within one frame (16 ms), for example in Audacity next to frame stepping in the player. Before the fix it was about 20–25 ms late.

- [ ] **Step 6: Commit** `Audio: the mixer and the clip's audio rings count time from running totals instead of rounding every 10 ms window, so recordings no longer drift late by about 10 ms an hour and tracks that are often silent (a gated mic, Discord) no longer drift in long buffers`

---

### Task 10: The anchor under its lock, and a tick count that does not wrap (CAP-4, AUD-8, CAP-10)

**Files:**
- Modify: `src-tauri/src/convert.rs:590-599` (`FrameSink`), `:723-726` (`frame_qpc` goes), `:737-794` (`pace`); a new `#[cfg(test)] mod tests`
- Modify: `src-tauri/src/pipeline.rs:616-627` (`EncoderSink::on_frame`)

**Interfaces:**
- Produces:
  - `fn on_frame(&mut self, texture: &ID3D11Texture2D, pts_100ns: i64, duplicate: bool, qpc_100ns: i64)`. `pace` reads `latest.slot_qpc` under the converter lock, next to `slot` (`:772-775`), and passes it on.
  - `Latest::frame_qpc` is removed. Its only caller was `pipeline.rs:622`.
  - `fn tick_offset(period: Duration, index: u64) -> Duration` computes `Duration::from_nanos(period_ns.saturating_mul(index))`. It replaces `period * index as u32` at `:748`.

- [ ] **Step 1: Write the failing test** in `convert.rs`:

```rust
/// CAP-10: `period * index as u32` wrapped after 2³² ticks (828 days at 60 fps).
#[test]
fn the_tick_schedule_does_not_wrap_after_two_to_the_thirty_two_ticks() {
    let period = Duration::from_nanos(16_666_666);
    assert_eq!(tick_offset(period, 3), period * 3);
    assert_eq!(tick_offset(period, 1 << 32), Duration::from_nanos(16_666_666 << 32));
}
```
  CAP-4 gets no unit test, because that needs a D3D device. The type is the guard: once `frame_qpc` is gone, the capture time can only arrive together with the frame the clock picked, read under the same lock.

- [ ] **Step 2: Run** `cargo test --lib -- convert::`. Expected: FAIL to compile (`tick_offset` not found).

- [ ] **Step 3: Implement** as described in Interfaces. `EncoderSink` anchors with the passed `qpc_100ns` instead of `latest.frame_qpc()`.

- [ ] **Step 4: Run** `cargo test --lib`. Expected: PASS. `cargo run --example record-probe -- --encoder <yours> 10` writes a clip, and its health lines look as before.

- [ ] **Step 5: Commit** `Capture: the clock hands the encoder sink the capture time of the frame it picked, read under the same lock, instead of reading it afterwards when a newer frame may already be there; and the tick schedule no longer wraps after 2³² ticks`

---

## Coverage

| Finding (review) | Task |
|---|---|
| BUF-1, also AUD-1 | 1 |
| SAVE-4 | 2 (and 7 for clips) |
| REC-1 (without its rejected "at least" fallback) | 3 (and 8 for the written times) |
| AUD-7, AUD-V2 | 4 |
| AUD-2 | 5 |
| SAVE-7 | 6 |
| BUF-6, also AUD-5 | 7 (clips), 8 (recordings) |
| REC-6: a recovered recording muxed at the nominal rate | 8 (TS folders; `WM_ENDSESSION` is not part of this plan) |
| AUD-6, also REC-5; AUD-V1 | 9 |
| CAP-4, also AUD-8; CAP-10 | 10 |

Dependencies: 1 before 7 and 9; 2 before 3 and 7; 6 before 7; 3 and 7 before 8. Tasks 4, 5 and 10 stand alone. Tasks 4 and 5 edit neighbouring lines of `capture.rs`, so run them one after the other.
