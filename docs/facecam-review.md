# Code review: the parts the facecam touches

Scope: the files from step 2.3 of the brief: capture and encoding (`wgc.rs`, `capture.rs`, `convert.rs`, `encode.rs`, `mft.rs`, `gpu.rs`), buffer and flow (`pipeline.rs`, `buffer.rs`, `state.rs`), saving and editor (`muxer.rs`, `stems.rs`, `edit.rs`, `export.rs`, `clips.rs`), recordings and screenshots (`recorder.rs`, `shot.rs`), console (`preview.rs`, `console.rs`, `src/console/`), sound (`audio/`), settings and permissions (`config.rs`, `commands.rs`, `src/routes/settings/`, `capabilities/`), and the callers they lead into. Rust paths are relative to `src-tauri/src/`; UI files are named without their folder.
Method: eight read-only area reviews. A second reviewer then tried to disprove every finding and corrected severities, lines, scenarios and fixes; no finding was rejected, one proposed fix (CAP-3) and one scenario (CFG-10) were. IDs with a `V` are bugs the second reviewer found. Nothing was built or run. Line numbers are those of 401a320. A bug found by two reviews appears once; the other IDs are listed under "also".
How to read it: severity is user impact, with likelihood where it changes the picture (high, medium–high, medium, low–medium, low, very low). **Confirmed** = traced in the code. **Plausible** = a real mechanism that depends on runtime behaviour; the entry says on what. Within each area the most severe come first.

## The ten that matter most

Ranked by user impact and likelihood; between equals, what the facecam builds on comes first.

| # | ID | Severity | What | Where |
|---|---|---|---|---|
| 1 | CFG-1 | high | A source muted in the console is un-muted by the next settings write in the main window; the spec's "Cam off" would inherit it | `commands.rs:54-90`, `store.ts:371-388` |
| 2 | REC-1 | medium–high | Sleep during a recording: the file plays in slow motion or goes silent after the wake, and cannot be repaired | `pipeline.rs:816-833`, `muxer.rs:251-264` |
| 3 | BUF-1 | medium | Full-length clips (the default) start silent; at a 4 s keyframe interval about half of all saves | `pipeline.rs:47-50`, `muxer.rs:139-209` |
| 4 | BUF-6 | medium | One stall mid-clip shifts picture against sound by up to half the stall; a cam encoder makes stalls likelier | `muxer.rs:251-264`, `convert.rs:762-767` |
| 5 | CAP-1 | medium | Encoder failures are only logged: the buffer freezes and saves return old footage | `mft.rs:1312`, `1344`, `1174-1177` |
| 6 | BUF-2 | medium | Capture errors after the first second never reach the UI; the spec routes cam errors through this channel | `pipeline.rs:549-554` |
| 7 | AUD-4 | medium | A slow device open leaks a capture thread that keeps the mic open; copied to the camera it keeps the LED on | `audio/capture.rs:127-136` |
| 8 | AUD-V2 | medium (can be high) | Silent packets from 44.1/96 kHz devices are not resampled: up to ~1 s lag for minutes | `audio/capture.rs:676-678` |
| 9 | CFG-3 | medium | "Export for Discord" re-sends an old export that still has the trimmed or muted part | `commands.rs:946-950` |
| 10 | EDIT-1 | medium | Deleting or filing a clip from the console during a save leaves an invisible orphan; phase 2 of the spec relies on this lock | `commands.rs:740-777`, `edit.rs:398-521` |

## Findings

### Capture and encoding

#### CAP-1 — Encoder failures are only logged; the buffer freezes and saves return old footage
**Severity:** medium · **Verified:** Confirmed · **Where:** `mft.rs:1284-1290`, `1312`, `1344`, `1174-1177`; `pipeline.rs:627-632`
**When** `ProcessInput`/`ProcessOutput` start failing while capture keeps working, or the encoder's event queue ends, **then** every error is one `log::warn!`, nothing reaches `shared.report`, the ring stops growing but keeps its old packets, and "save the last 30 s" ten minutes later writes the 30 s before the failure. If the thread ended, only the dropped-frames number in the console climbs: no banner, no restart.
**Why:** `Err(err) => log::warn!("{err}")` at `mft.rs:1312`/`1344`; a submit on the dead channel just returns `false` (`1174-1177`); the ring trims only on push and `snapshot` counts back from `newest_pts` (`buffer.rs:75-84`, `181-200`). After a TDR, `VideoProcessorBlt` usually fails too, and that one does reach the banner (`pipeline.rs:705-707`).
**Fix:** add `failed` and `last_error` to `EncoderStats` (or an `on_error` callback in `VideoEncoder::start`). Set them after ~30 consecutive errors and when the loop ends while `running` is still true; report a `Disconnected` submit once the same way. Route it to `shared.report` (needs BUF-2) and let the drift watcher restart the pipeline as for `Drift::Closed`.

#### CAP-2 — `in_flight` assumes one output per input; a lost output can freeze the picture for good
**Severity:** medium impact, low likelihood · **Verified:** Plausible (needs an MFT or driver that consumes an input without a counted output) · **Where:** `mft.rs:1297-1313`, `1334-1342`; `convert.rs:671-678`, `694-702`
**When** an output gets lost (`ProcessOutput` succeeds but `ConvertToContiguousBuffer`/`Lock` fails, an internal rate-control drop, a stream change with no later `HaveOutput`), **then** its entry in `sent` stays. After ~18 such events `QUEUE_DEPTH + in_flight >= SLOTS` holds for good, every capture is "starved", the pacer re-sends the last slot and the buffer records a still image. One log line, no UI signal.
**Why:** `sent.pop_front()` runs only on `Ok(Some(packet))` (`mft.rs:1299-1302`); nothing else shrinks `sent`; `starved` is read nowhere outside `convert.rs`.
**Fix:** pop when `ProcessOutput` returned `Ok`, whatever fails afterwards, and age entries out: `while sent.front().is_some_and(|t| t.elapsed() > Duration::from_secs(2)) { sent.pop_front(); }`. `IMFTrackedSample` is the exact answer; treat it as a facecam design item, not a hotfix.

#### CAP-5 — Every captured frame is converted and watermarked, even when the next one overwrites it before the tick
**Severity:** low–medium · **Verified:** Confirmed · **Where:** `convert.rs:712-715`, `323-343`, `560-586`; `wgc.rs:202-209`
**When** the monitor runs faster than the clip rate (165 Hz at 60 fps) on Windows 10 or Windows 11 before 24H2, **then** every extra frame runs a scaling `VideoProcessorBlt` plus the watermark's full-frame `CopyResource` and D2D draw under the D3D lock, and is thrown away. That is ~1.5 GB/s of copies at 1440p/165 Hz and ~8 GB/s at 4K/240 Hz, on the GPU the game renders on.
**Why:** with `reuse` the same slot is picked but `convert` still runs in full (`convert.rs:715`). `MinUpdateInterval` exists only from Windows 11 24H2 (build 26100); the comment at `wgc.rs:202-203` says "Windows 11".
**Fix:** draw the logo as a second video-processor input stream with alpha in the same Blt (check `MaxInputStreams >= 2` and the alpha-stream cap at start, keep D2D as fallback); that removes the copy on every frame. Or hold the newest capture frame and convert once per tick (needs a frame pool of 3 or more). Skipping young frames also works but adds up to half a period of judder.

#### CAP-3 — A stream change from the encoder resets its output type and loses bitrate, profile and keyframe spacing
**Severity:** low · **Verified:** Plausible (encoders are not supposed to send it; some drivers do) · **Where:** `mft.rs:1015-1023`, `1238`
**When** the MFT returns `MF_E_TRANSFORM_STREAM_CHANGE`, **then** `GetOutputAvailableType(0, 0)` → `SetOutputType` replaces the type from `build()` and drops `MF_MT_AVG_BITRATE`, High profile and `MF_MT_MAX_KEYFRAME_SPACING`, the field NVIDIA honours (`mft.rs:781-788`). The result is ignored and nothing is logged. If a header was captured at build time, a changed SPS/PPS is not fetched again.
**Why:** `mft.rs:1019-1020`; the comment above says "fetch it again", the code returns `Ok(None)`.
**Fix:** keep returning `Ok(None)` on the async path. (The original proposal, calling `ProcessOutput` again at once, is rejected: without a `HaveOutput` it breaks the async contract and returns `E_UNEXPECTED`.) Copy the three attributes onto the new type before `SetOutputType`, log the change at warn, and return a flag so `run` sets `header_pending = true; header_tries = 0`. Fix the comment.

#### CAP-6 — `CLIPPIBOY_QUEUE_DEPTH` changes the guard but not the slot count
**Severity:** low · **Verified:** Confirmed · **Where:** `mft.rs:1100`, `1143`; `pipeline.rs:688`; `convert.rs:75`, `674`, `778-781`
**When** the variable is set to 22 or more, **then** every frame is starved from the first, the pacer never emits, nothing is anchored and the console says "The encoder has not delivered a picture yet". Values of 18–21 starve as soon as the encoder holds a few frames. A diagnostic knob produces a misleading symptom of its own.
**Why:** `depth` is only `.max(1)` and goes straight into `depth + in_flight >= SLOTS`, with `SLOTS = 22` fixed.
**Fix:** read the depth before `Converter::new` and size the slots as `depth + ENCODER_HOLD_MAX + 2`, or clamp `depth` to `SLOTS - ENCODER_HOLD_MAX - 2` and warn.

#### CAP-7 — The encoder MFT is picked by vendor, not by the adapter the device lives on
**Severity:** low · **Verified:** Plausible (depends on whether `MFTEnumEx` lists one MFT per adapter) · **Where:** `gpu.rs:141-148`; `mft.rs:708-714`
**When** a machine has two GPUs of one vendor (Radeon iGPU + Radeon dGPU, Intel iGPU + Arc), **then** the device is made on the adapter with the most VRAM, but the first MFT with that vendor id may belong to the other one; the encoder then fails or takes a cross-adapter path.
**Why:** `mft.rs:708-714` matches `VEN_xxxx` only.
**Fix:** add the adapter LUID to `AdapterInfo` and enumerate with `MFTEnum2` plus `MFT_ENUM_ADAPTER_LUID` (documented only as an input to `MFTEnum2`; reading it off returned activates is undocumented), falling back to `MFTEnumEx`. Log the LUID in the profile line.

#### CAP-8 — The encoder thread never initialises COM
**Severity:** low · **Verified:** Plausible (latent: today the audio and status threads keep an MTA alive) · **Where:** `mft.rs:1113-1131`
**When** the encoder starts before any thread has called `CoInitializeEx(MTA)`, **then** `ActivateObject` fails with `CO_E_NOTINITIALIZED` and the buffer does not start.
**Why:** the thread calls `build` (`MFTEnumEx` → `ActivateObject`) directly; it works only through the implicit MTA that `audio::devices::ensure_com` (`audio/devices.rs:31-35`) and the capture threads leave behind.
**Fix:** `CoInitializeEx(None, COINIT_MULTITHREADED)` at the top of the encoder thread and `CoUninitialize` on exit. Mandatory for the new camera thread.

#### CAP-9 — A `feed` error before `ProcessInput` loses a `NeedInput`, and the encoder can stall for good
**Severity:** low · **Verified:** Plausible (depends on the MFT sending one `NeedInput` at a time, which FFmpeg's client also assumes) · **Where:** `mft.rs:1330-1346`
**When** `MFCreateDXGISurfaceBuffer`, `MFCreateSample` or `AddBuffer` fails (out of memory, removed device), **then** `pending_input` is still decremented, the MFT keeps waiting for that input and asks for no more; the loop only sleeps, the job channel fills, and every tick waits 500 ms and drops its frame until a restart.
**Why:** the decrement at `mft.rs:1346` runs whether or not `ProcessInput` was reached (`950-971`). Microsoft: "For each METransformNeedInput event, the client calls ProcessInput."
**Fix:** split `feed` into `prepare(job) -> IMFSample` and `ProcessInput`; decrement only after `ProcessInput` was called; on a prepare error keep the request and take the next job.

#### CAP-V1 — Video-processor auto-processing is left at the driver default
**Severity:** low · **Verified:** Plausible (depends on the driver and the user's "video enhancement" settings) · **Where:** `convert.rs:151-170`
**When** the vendor control panel has edge enhancement, noise reduction, skin tone or dynamic contrast switched on, **then** the driver may apply it inside the Blt of a `PLAYBACK_NORMAL` processor, so clips differ between machines and from what the screen showed. A camera stream is even more likely to get it.
**Why:** `VideoProcessorSetStreamAutoProcessingMode` is never called; Microsoft documents that the driver "might perform certain processing tasks automatically" by default.
**Fix:** after `CreateVideoProcessor`, `video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false)`. Do the same in the cam's converter.

#### CAP-V2 — The async encoder MFT is never shut down
**Severity:** low (higher with two encoders under an NVENC session cap) · **Verified:** Plausible (the contract deviation is certain; whether sessions survive is untested) · **Where:** `mft.rs:716`, `1440-1444`
**When** the pipeline restarts (screen change, target change, lock screen), **then** the encoder ends with only `END_STREAMING`; if a vendor MFT's worker threads hold a reference, its hardware session outlives the restart, and repeated restarts can hit the session limit.
**Why:** no `IMFShutdown::Shutdown` and no `IMFActivate::ShutdownObject` anywhere; Microsoft documents both as required after `ActivateObject` and for async MFTs.
**Fix:** keep the `IMFActivate` in `Transform`; after `END_STREAMING` call `transform.cast::<IMFShutdown>()?.Shutdown()` and then `activate.ShutdownObject()`. Verify with 10 restarts while watching the session count. Spec section 2.3 requires this before stage 2.

#### CAP-4 — The A/V anchor reads the capture time outside the lock that picked the frame
**Severity:** very low · **Verified:** Confirmed · **Where:** `convert.rs:772-793`, `716`; `pipeline.rs:621-626`
**When** a capture callback converts a newer frame between `drop(converter)` (`convert.rs:792`) and `latest.frame_qpc()` (`pipeline.rs:622`) on the very first tick, **then** `base_100ns` is one capture interval (7–17 ms) late for the whole session, and the sound leads the picture by that much.
**Why:** `slot` is read under the converter lock, `slot_qpc` after it. The window is a few instructions wide, so it rarely fires.
**Fix:** read `slot_qpc` under the lock next to `slot` and pass it on: `sink.on_frame(&texture, pts, duplicate, qpc)`. Do it before the pattern is copied for the cam.
(also: AUD-8)

#### CAP-10 — The pacer schedule truncates the tick index to u32
**Severity:** very low · **Verified:** Confirmed (theoretical) · **Where:** `convert.rs:748`
**When** the buffer runs more than 2³² ticks without a restart (~828 days at 60 fps), **then** `due` jumps back to `start`, every tick overruns and skips ~2³² more, and the loop spins against the 500 ms submit wait. No panic.
**Why:** `let due = start + period * index as u32;`
**Fix:** `let due = start + Duration::from_nanos(period_ns.saturating_mul(index));` with `period_ns: u64`.

### Buffer, pipeline and state

#### BUF-1 — Full-length clips can start silent; shorter clips end with tens of ms of silence
**Severity:** medium (high at a 4 s keyframe interval; the buffer review's verifier rated the default 2 s case low–medium) · **Verified:** Confirmed · **Where:** `pipeline.rs:47-50`, `1069`, `194-220`; `buffer.rs:92-108`, `181-200`; `muxer.rs:139-153`, then `178-209`
**When** the user saves a clip as long as the buffer (the default, `clip_seconds = 0`), **then** the video starts at the front keyframe, up to one keyframe interval older than `newest − seconds`, while the audio rings hold only `seconds + 2 s` and are read only after the whole `.h264` is written. Missing lead-in ≈ `max(0, w + L + x − (2 s + ~85 ms))` (w = write time, x = keyframe phase). At a 4 s interval about half of all full saves lose up to ~1.9 s even with an instant write; at 2 s about 5–12 % lose up to ~0.25 s, more on an HDD or network folder. Shorter clips end with ~40–80 ms of zeros because the mixer runs 80 ms behind. Picture and sound stay aligned; the start is mute.
**Why:** `snapshot` copies the packets but only clones the live ring `Arc`s (`pipeline.rs:1069`); `TrackRing::window` pads with zeros.
**Fix:** (1) Mandatory: size the rings `samples_for(seconds + keyframe_seconds + 5…10)` (~2–3 MB per source) and keep `keyframe_seconds` in `Shared`. (2) Publish the mixer position (`mixed_until_100ns`), take the `Arc`s under the pipeline lock, then outside it wait up to ~150 ms until `mixed_until ≥ start + span` and copy the windows. Do not only move the WAV writes before the video: alone, that makes every clip end with ~50–70 ms of zeros.
(also: AUD-1)

#### BUF-2 — Capture errors after the first second never reach the UI
**Severity:** medium · **Verified:** Confirmed · **Where:** `pipeline.rs:549-554`; polled from `lib.rs:1070-1080`
**When** the capture source goes away after the start (a captured window closes, the converter fails), **then** nobody is told: the first poll ran while there was no error yet, latched `error_seen`, and every later poll returns `None`. For a window target the pacer re-sends the last frame forever; `screen_drift` only handles monitors (`state.rs:830-848`).
**Why:** `if self.error_seen.swap(true, SeqCst) { return None; } self.error.lock().clone()`.
**Fix:** read first, swap only when there is an error:
```rust
let error = self.error.lock().clone()?;
if self.error_seen.swap(true, Ordering::SeqCst) { return None; }
Some(error)
```
Also rate-limit the warn line in `Shared::report` (`pipeline.rs:543-546`), which logs once per captured frame on a persistent failure. Test: a report after an empty first poll is returned once. Spec section 2.8 depends on this fix.

#### BUF-5 — After a wake without a restart, clips are silent or stretched for the next buffer length
**Severity:** medium · **Verified:** Plausible (needs QPC to advance across sleep, as the code's own comments at `pipeline.rs:811-815` say it does; the capture session and encoder to survive resume; and no lock screen held for 10 s) · **Where:** `convert.rs:762-767`; `buffer.rs:93-99`, `172-177`, `181-189`; `muxer.rs:255-262`; `pipeline.rs:816-833`
**When** the PC sleeps 10 min and wakes straight to the desktop (or Windows Hello clears the lock screen within ~10 s), **then** the last pre-sleep keyframe stays in the ring for another 120 s. A save cuts from it, the span covers the sleep, `timing` falls back to the nominal rate, and the audio window lies entirely before the wake: the whole clip is silent. After a short sleep (30 s) the frames are spread instead, and the post-wake picture runs up to ~29 s ahead of its sound. The status shows the sleep as buffered time.
**Why:** the pacer skips the sleep as ticks; the ring drops a GOP only when the next keyframe is `capacity` old. A monitor that disconnects across sleep does trigger a restart via `screen_drift`, which hides the bug on many desktops.
**Fix:** in `ReplayBuffer::push`, treat a forward pts jump above `max(1 s, 3 × GOP)` as a discontinuity and, at the next keyframe, drop everything before it. `snapshot` must never start before the last discontinuity. Optionally force a keyframe after a large skip in `pace`.

#### BUF-6 — One stall in the middle of a clip shifts picture against sound by up to half its length
**Severity:** medium (the buffer review's verifier: low–medium, because only stalls over 250 ms or dropped submits leave a hole) · **Verified:** Confirmed · **Where:** `muxer.rs:251-264`, `316`; `convert.rs:734`, `762-767`; `mft.rs:1184-1199`; `pipeline.rs:628-632`
**When** the encoder stalls ~1 s at second 60 of a 120 s clip (GPU contention in the game; a cam encoder and background composites make it likelier), **then** the pacer skips ~60 ticks, ffmpeg spreads all frames evenly over the span, and around the hole the picture is ~0.5 s off its sound (error ≈ `p·T·L/(T−L)`). The ends meet, the middle does not. Recordings share the limitation (`recorder.rs:566`); the sleep case there is REC-1.
**Why:** the raw Annex-B stream carries no timestamps; one `-r` is given for the whole stream.
**Fix:** keep real timestamps: write the packets into a timestamped container (MPEG-TS, NUT or Matroska) and mux with `-c:v copy`, or split at pts gaps; pair it with BUF-5's discontinuity cut for real gaps. Do not raise `CATCH_UP` (`convert.rs:734`) instead: the comment at `convert.rs:754-761` explains that replaying missed ticks under a sustained shortfall makes the timestamps fall further and further behind the clock, which is the bug the jump was made against. `docs/extras/plans/2026-10-22-fix-sync-and-timestamps.md` Task 7 plans the timestamped container. One change can cover BUF-5, BUF-6 and SAVE-4.
(also: AUD-5)

#### BUF-V1 — A failed `Recorder::finish` leaves capture running and "recording" on
**Severity:** medium · **Verified:** Confirmed · **Where:** `state.rs:1129-1139`, `762-769`; `commands.rs:386-394`; `recorder.rs:374`
**When** the clip drive disappears mid-recording (USB unplugged, network share lost) or AV or cloud sync holds `meta.json` during the rename, **then** the auto-stop's `finish` fails in `write_meta` and `close_recording` returns early: `halt_pipeline` and `status.recording = false` never run. Capture, encoder and mixer keep running with nobody consuming them; UI, tray and Stream Deck show a recording forever. "Stop" answers "No recording is running", or, once the drive is back with 2 GiB free, starts a new recording.
**Why:** `Err(err) => { log::error!(…); return None; }` at `state.rs:1131-1134`, before `1136-1139`; `status_snapshot` only ever sets `recording` to true.
**Fix:** once the recorder has been taken, unhook it, halt the pipeline when the buffer is not wanted, and clear `status.recording`, whatever `finish` returns. `finish` hands back the in-memory `Meta` even when persisting it fails (log the error); the folder stays for `recover_recordings`.
(also: REC-2)

#### BUF-3 — Starting a recording does file I/O while holding the packet ring lock
**Severity:** low–medium (low on an SSD, medium on a sleeping HDD or a network folder) · **Verified:** Confirmed · **Where:** `pipeline.rs:1004-1016` → `recorder.rs:148-185`; `state.rs:1103-1107`; `recorder.rs:326-334`, `pipeline.rs:846-863`
**When** the clip folder is on a spun-down HDD or a share and a recording starts, **then** `create_dir_all`, `File::create`, `write_meta` and a thread spawn run under `shared.packets` (and `AppState.pipeline`, `lifecycle`). The encoder thread blocks in `on_packet` (`pipeline.rs:667`), the job channel fills, `submit` drops frames after 500 ms and the pacer skips ticks: a hole at the start of the recording and in the buffer (which then drifts, BUF-6). The status tick and saves block too. On the mixer, `open_track` writes meta under `shared.tracks`; a stall over 1 s turns that audio into zeros.
**Why:** `Recorder::start` is called inside the locked section that publishes the recorder.
**Fix:** split it into `Recorder::prepare(dir, meta)` (folder, files, meta, writer thread) outside the lock and a cheap `attach(prepared, header, backlog, start_100ns)` under it. Later meta writes go through the writer thread (`Job::Meta`).
(also: REC-4)

#### BUF-4 — A stream that flags exactly one keyframe grows without bound
**Severity:** low (critical impact if it fires) · **Verified:** Plausible (needs an encoder that sends one IDR and then none) · **Where:** `buffer.rs:93`, `128`
**When** an encoder emits an IDR only at the start (infinite GOP, later intra pictures without CleanPoint or SPS), **then** `keyframes.len() == 1` forever: the GOP trim needs more than one, the no-keyframe floor needs none, so the ring ignores length and memory budget until allocation fails and the app aborts.
**Why:** `while self.keyframes.len() > 1` (`buffer.rs:93`) and the floor returns when `!keyframes.is_empty()` (`128`).
**Fix:** ask the encoder for an IDR (`CODECAPI_AVEncVideoForceKeyFrame`) when the newest keyframe is older than ~2× the configured GOP. As a last resort, when one keyframe holds the ring past `2 × capacity_bytes` or `capacity + max GOP`, drop the front including it and log a dedicated message (not the floor's "not flagging any keyframes").

#### BUF-7 — `halt_pipeline` holds `AppState.pipeline` across the whole teardown
**Severity:** low · **Verified:** Confirmed · **Where:** `state.rs:612-614`
**When** the buffer stops (lock screen, restart, setting change), **then** the guard in the `if let` scrutinee lives through `pipeline.stop()`: thread joins, a possible 500 ms `send_timeout`, the encoder drain of up to 2 s. The status tick (level meters freeze, `lib.rs:620`, `1057`) and any save (`state.rs:884`) wait. No deadlock.
**Why:** `if let Some(mut pipeline) = self.pipeline.lock().take() { pipeline.stop(); }` keeps the temporary guard alive to the end of the block.
**Fix:** `let taken = self.pipeline.lock().take(); if let Some(mut p) = taken { p.stop(); }`. `lifecycle` stays held by every caller, so no start can slip in.

#### BUF-8 — The clip's width and height are read outside the snapshot
**Severity:** low · **Verified:** Confirmed · **Where:** `state.rs:883-900`, `611`
**When** a restart (screen drift, source change) lands between `pipeline.snapshot()` and the `active_recording` read, **then** the clip row gets the new pipeline's size or the unfitted config size while the file has the old one.
**Why:** `halt_pipeline` clears `active_recording` at `state.rs:611`, before it blocks on the pipeline lock, so even a save in the middle of `snapshot()` is exposed.
**Fix:** put `width`/`height` (and later the cam's size, fps and offset) into `ClipSnapshot`.

#### BUF-9 — A buffer-length or source change made while the pipeline starts is lost
**Severity:** low · **Verified:** Confirmed · **Where:** `state.rs:546`, `550-556`, `562`, `437`, `450`
**When** automation starts the buffer and the user moves the length slider or adds a mic during the seconds `Pipeline::start` takes, **then** `replace_config` skips the live update because `shared` is not registered yet. The buffer keeps the old length until the next length change; the new mic gets a capture stream but no ring until any config change.
**Why:** `replace_config` takes no `lifecycle` and only updates a registered `shared`.
**Fix:** after registering `shared`, re-read the config and re-apply `set_buffer_seconds`, `set_capacity` and `set_sources`; keep `IDLE_RING_SECONDS` when `for_buffer` is false.

#### BUF-V2 — The automatic memory budget is computed from two different bitrates
**Severity:** low · **Verified:** Confirmed · **Where:** `state.rs:549-553` against `433-441` and `520-523`; `capture.rs:655-656`
**When** `memory_mb = 0` and the user changes the buffer length live, or a recording-only pipeline becomes a buffer, **then** the cap is recomputed from the config's `bitrate_kbps` instead of the fitted one used at launch; for 3440×1440 with a 1080p config it drops ~25 %.
**Why:** only `launch_pipeline` uses the fitted recording; the config bitrate is recomputed only at load (CFG-7).
**Fix:** compute the budget from `active_recording` on all three paths, or keep the bitrate the pipeline started with in `Shared`.

#### BUF-10 — A failed mixer spawn leaves the pacer and encoder running
**Severity:** very low (thread-spawn failure only) · **Verified:** Confirmed · **Where:** `pipeline.rs:724-746`
**When** spawning the mixer thread fails, **then** `?` returns after the pacer was spawned; its handle is dropped, `latest.running` stays true, and the detached pacer keeps the encoder and `Shared` alive, encoding the last frame forever.
**Why:** no cleanup on the error path.
**Fix:** on error set both stop flags (`latest.stop()`, `mixer_stop`) and join whatever was spawned. Spawning the mixer first only moves the leak.

### Saving, tracks, clip database, export

#### SAVE-2 — `Library::get` scans the whole table and stats files per clip; startup repair calls it per clip
**Severity:** medium · **Verified:** Confirmed · **Where:** `clips.rs:186-188`, `145`, `171-176`; `edit.rs:75-82`, `113-115`, `1046-1048`; `lib.rs:1258-1259`
**When** the library holds ~1000 clips, **then** `setup` runs `repair`, which calls `get` once per clip, and each `get` is `list()` + `find` with 2–3 `is_file` stats per row and a full tags scan: ~2 million stats (each opens a handle on Windows), an estimated 6–20 s of frozen start, ~1–3 s already at 300 clips. Every sync command that calls `get` (`set_clip_favorite`, `reveal_clip`, `open_clip`, `update_clip`, …) pays O(N) stats on the main thread; `delete_clip` pays it twice.
**Why:** `get` is `list().into_iter().find(…)`; `list` runs `has_original` per row.
**Fix:** a real `get` (`SELECT … WHERE id = ?1` plus one tag query); in `repair`, read the row once and re-read only after an arm wrote; drop the second `get` in `Library::delete`. A future facecam flag must not add another per-row stat to `list()`.

#### SAVE-3 — Nothing sweeps scratch or partial files, and Quit does not wait for a running save
**Severity:** medium · **Verified:** Confirmed (that ffmpeg keeps writing after the app dies is an inference; no job object exists) · **Where:** `muxer.rs:130-134`, `376`, `221-224`, `400-408`; `edit.rs:456`; `lib.rs:712-723`; `tray.rs:139-149`; `updater.rs:77-84`
**When** the user quits, an update restarts the app, Windows shuts down or any thread panics (`panic = "abort"`) during a save or an edit, **then** `<name>_<id>.part.h264` (as big as the clip's video), `.part.*.wav`, a possibly complete `.part.mp4` or `<clip>.<pid>.neu.mp4` stay in the clip folder forever, and `tracks/<id>/` keeps m4a files without `tracks.json`. The clip the user asked for is lost. Phase 2 of the spec (section 2.6) widens this window from ~2 s to tens of seconds.
**Why:** cleanup happens only on the normal paths; `discard_leftovers` clears only `buffer/` and `temp/`; Quit never looks at `is_saving()`.
**Fix:** at start, sweep the clip folder (≤ 2 levels, like `filing::inside`) for `*_[0-9a-f_]{36}.part.{h264,mp4}`, `*_[0-9a-f_]{36}.part.*.wav` (the id is sanitised, hyphens become `_`) and `*.<pid>.neu.mp4`; remove `tracks/<id>` folders with no row, no index and older than a few minutes (share import writes stems too). Before `app.exit`, wait on a worker thread for `is_saving()` to drop, with a timeout and a banner.
(also: EDIT-16)

#### SAVE-4 — A sustained encoder shortfall of more than a third is taken for sleep; the clip plays fast and the sound falls behind
**Severity:** medium impact, low likelihood · **Verified:** Confirmed · **Where:** `muxer.rs:251-264`; `convert.rs:754-767`; `pipeline.rs:1054`
**When** the encoder delivers only 36 of 60 fps for a 30 s clip, **then** frames = 1080, nominal = 18 s, span = 30 s > 27 s, so `timing` picks the nominal 60 fps: the clip lasts 18 s, plays 1.67× fast, the audio covers only the first 18 s, and the end is 12 s apart. At 70 % it stretches correctly; the cliff sits at 2/3. A cluster of mid-clip stalls totalling more than a third trips it too.
**Why:** the stretch branch needs `span ≤ nominal × 3/2`; anything longer is assumed to be sleep (`muxer.rs:248-250`).
**Fix:** tell sleep from shortfall by the largest single pts gap, not the ratio: compute `max_gap` in `Pipeline::snapshot` (and next to `span_of` in the recorder, `recorder.rs:371`) and stretch unless one gap exceeds ~2 s. Best done with BUF-5 and BUF-6.

#### SAVE-7 — Stems may sit one AAC frame (21.3 ms) off the picture, and each audio-only edit adds another
**Severity:** medium if confirmed · **Verified:** Plausible, likely (every step checks out against FFmpeg's source; nothing was run) · **Where:** `muxer.rs:356`, `387-393`; `edit.rs:724-729`
**When** a clip saved with more than one audio input (or a recording) is edited without a front cut and mixed from its stems, **then** the clip's video already starts at +21.33 ms (`make_zero` shifted everything by the AAC priming), the stems read back from 0 through their edit list, and the edit's own `make_zero` shifts again: the sound leads the picture by 21.33 ms after one edit, ~43 ms after two, noticeable after three. Stems extracted with `-c:a copy` and in-file audio are not affected.
**Why:** `-avoid_negative_ts make_zero` is set on the clip output only (`muxer.rs:356`); the stem outputs keep negative timestamps and an edit list.
**Fix:** check first: `ffprobe -v error -show_entries stream=codec_type,start_time -of compact` on such a clip, on `tracks/<id>/0.m4a`, and after two mix-only saves (expected if real: video 0.021333 / audio 0, stem 0, then video 0.064). Then one zero-point policy for every output: preferred, drop `make_zero` in `muxer.rs:356` and `edit.rs:729` and let edit lists carry the priming (check WebView2 and Discord playback); otherwise add it to every stem output. Add an argument-list test that all outputs share the policy.

#### SAVE-6 — `delete_clip` drops the row even when the file could not be deleted
**Severity:** low–medium · **Verified:** Confirmed (the sharing-violation behaviour is a Windows inference) · **Where:** `clips.rs:315-330`; `commands.rs:741-775`
**When** the clip is open in VLC, the Explorer preview, a Discord upload or ClippiBoy's own export (`shrink` holds it open), **then** `let _ = remove_file` fails silently and the row, tags and stores go anyway. The file stays in the clip folder, invisible to the app (`tidy` and `repair` only walk rows), and its space is never freed. The race with a running edit is EDIT-1.
**Why:** the result of `remove_file` is ignored before the `DELETE`.
**Fix:** delete the file first, with retries like `replace_file`; on failure return "close it first" and keep the row. Share the per-clip busy guard from EDIT-1 with `shrink` and `file_clip`.

#### SAVE-8 — A half-written WAV is left behind when its write fails
**Severity:** low · **Verified:** Confirmed · **Where:** `muxer.rs:178-209`, `221-224`; `pipeline.rs:258-279`
**When** the disk fills during a save, **then** the partial `<stem>_<id>.part.mix.wav` (~23 MB per 2-minute track) stays in the clip folder, taking exactly the space that ran out, and the clip is saved without that source while the user sees "Clip saved".
**Why:** the `Err` arms only log; the cleanup iterates over the WAVs that succeeded.
**Fix:** `let _ = std::fs::remove_file(&path);` in both `Err` arms (or in `pipeline::write_wav`); optionally a warning notice.

#### SAVE-9 — The `save_clip` command with `seconds` bypasses the save guard
**Severity:** low (no UI passes `seconds` today) · **Verified:** Confirmed · **Where:** `commands.rs:411-414`
**When** a page calls `save_clip({seconds: 30})` twice, **then** both run in parallel with no `begin_save`, no `clip-saved` event, no friend tags and `is_saving()` false. Two runs in the same millisecond get the same destination; the second rename overwrites the first file and `INSERT OR REPLACE` on the unique `path` silently drops the first row.
**Why:** `Some(seconds)` calls `state.save_clip` directly.
**Fix:** route `Some(seconds)` through `save_clip_and_notify(&app, Some(seconds))`.

#### SAVE-10 — Export always scales sources taller than 1080p down, even with room to spare
**Severity:** low · **Verified:** Confirmed (comment against code; the intent is unclear) · **Where:** `export.rs:83-93` against the comment at `19-20`
**When** a 30 s 1440p clip is exported with the 100 MB preset (~27 Mbit/s, 1.6× what 1440p30 wants), **then** it is still scaled to 1080p.
**Why:** the first step at or below the source height always replaces it.
**Fix:** try the source height first (`once(target.height).chain(HEIGHTS.filter(|h| h < target.height))`) and mirror it in `ExportDialog.tsx:41-48`.

#### SAVE-11 — A long game name pushes the scratch names past 255 characters
**Severity:** low · **Verified:** Confirmed (mechanism) · **Where:** `state.rs:1306-1312`; `muxer.rs:126-134`, `163`, `376`
**When** an unknown fullscreen app has a title that sanitises to G characters, **then** above G = 184 every save fails with a path error; for 181 < G ≤ 184 the save succeeds but the mix WAV fails and that track is silently dropped (SAVE-8). The final name alone would fit up to G = 227.
**Why:** `sanitize_name` has no length cap; scratch names add `_<id>.part.<kind>`.
**Fix:** cap `sanitize_name` at 60 characters like `filing::MAX_LEN`; scratch names need no file stem (`<id>.part.<kind>` is unique).

#### SAVE-V1 — A PATH ffmpeg without ffprobe makes every save write the clip and then delete it
**Severity:** low · **Verified:** Confirmed (code path; the environment is rare) · **Where:** `state.rs:872-874`; `muxer.rs:40-50`, `82-88`, `424-426`, `529-541`
**When** the pinned tools are not ready yet (first-run download) and PATH has `ffmpeg.exe` but no `ffprobe.exe`, **then** each save runs ffmpeg successfully, `probe_duration_ms` cannot spawn ffprobe and returns `None`, and `give_up("…cannot be read back — nothing was kept")` deletes the clip and its stems. Every clip is lost, with an error that blames the file.
**Why:** the save gate `muxer::available()` runs only `ffmpeg -version`.
**Fix:** gate on both programs, or tell "ffprobe could not start" from "ffprobe could not read the file" and keep the clip in the first case.

### Editor

#### EDIT-1 — Deleting or filing a clip from the console while it is being saved leaves an invisible orphan
**Severity:** medium · **Verified:** Confirmed (the Windows file-lock details are an inference) · **Where:** `commands.rs:740-777`, `816-841`, `1418-1427`; `edit.rs:398-521`, `501-511`, `603`; `Console.tsx:696-702`, `746-752`
**When** a trimmed clip is being re-cut in the main window (ffmpeg reads the archive, not the clip) and the user deletes it from the console, **then** `lib.delete` removes row and file, `swap_in` → `replace_file` recreates the file, the thumbnail is rebuilt, and `store()` fails at `lib.get → None` without a notice: a full clip file with no row, plus an orphan archive and stems, never reconciled. With a heart in the console, `file_clip` moves the file instead; `swap_in` writes the new file at the old path and the row points at pre-edit content.
**Why:** only `edit.rs` takes `WORKING` (`edit.rs:53`, `218`, `406`); the main window's own guards (`ClipPlayer.tsx:116-124`) do not cover the console.
**Fix:** run the file work of `delete_clip` and `file_clip` under `WORKING` (a small `edit::exclusive(f)`, or `try_lock` refusing with "This clip is being saved — try again in a moment"). The lock order `WORKING` → library is deadlock-free. Re-reading the row inside the lock does not close this window; keep a cleanup in `store()` only as a backstop. `discard_clip_original` (`edit.rs:159-170`) should take it too; today only the UI prevents the overlap.
(also: SAVE-6, its race half)

#### EDIT-3 — The export overshoots the size target because the duration is floored to whole seconds
**Severity:** medium · **Verified:** Confirmed · **Where:** `export.rs:79`, `65-70`; `ExportDialog.tsx:36`
**When** a 5.9 s clip is exported at 10 MB, **then** the budget is computed for 5 s and the file lands ~14 % over (1.9 s: ~84 % over; 15.9 s: ~3 % over, eating the whole container margin). NVENC and AMF run `-rc cbr`, so they spend it; Discord refuses the file. The dialog rounds instead of flooring and shows another figure. Share links shrink the same way, so a 1–2 s clip can be refused as too big for 50 MB.
**Why:** `let seconds = (target.duration_ms / 1000).max(1);`; the only budget test uses exactly 60 000 ms.
**Fix:** work in ms: `audio_bits = AUDIO_KBPS * duration_ms`, `video_kbps = video_bits / duration_ms.max(1000)` (kbit/s × ms = bit). Same in `ExportDialog.preview()`, rounding only for display. Tests at 1 900, 5 900 and 15 900 ms asserting predicted ≤ target.
(also: SAVE-1)

#### EDIT-5 — A single-track clip with its audio only in the file gets its level applied again on every save
**Severity:** medium (irreversible; small population) · **Verified:** Confirmed (reachable only for clips saved with a level or trimmed before e9741de, or with `tracks/<id>` removed by hand) · **Where:** `edit.rs:438`, `697-700`; `commands.rs:1398-1410`; `useClipMix.ts:143-171`; `ClipPlayer.tsx:550-557`
**When** the user sets −6 dB on such a clip and saves, **then** the core bakes −6 dB into the file and stores 0 dB, but the slider stays at −6; the pane says "Not saved yet", and Save stacks another −6 dB. Opening such a clip also shows the old stored level although it is already baked in; setting 0 to "undo" it changes nothing and reports "Saved".
**Why:** the mix is seeded only when the clip id changes; the UI was never adapted to "the saved level is the new zero".
**Fix:** after a successful save, re-seed the mix from the returned `clip.edit.tracks` (`store.applyClipEdit` returns the clip, `useClipMix` exposes `load(tracks)`). When `clipTracks` returns tracks without `previewPath`, seed the sliders at 0 dB, or have the core normalise the stored levels to 0 the first time it sees in-file audio.

#### EDIT-6 — Trimming a recording files the whole recording away as an original, against the documented behaviour
**Severity:** medium (if `model.rs` states the intent; the README promises Undo for every first cut) · **Verified:** Confirmed (doc against code) · **Where:** `model.rs:703-706`; `edit.rs:537-555`, `571-573`, `580-601`
**When** the last minute of a one-hour recording is trimmed and the clip folder is on another drive than `%APPDATA%`, **then** `move_across` copies many GB to C: while holding `WORKING`, the UI sits at 99 %, and C: keeps a full second copy until undo or "Free up space". On the same volume it is an instant rename, but the disk still holds both.
**Why:** `edit.rs` never looks at `clip.recording`; b9ce5fe added the doc line without touching `edit.rs`.
**Fix:** pass `!clip.recording` into `archives_the_clip` and keep `plan.original` as the record (the stems offset must stay), so Undo is not offered. Or correct the doc if keeping the copy is intended.

#### EDIT-7 — Edits queued behind `WORKING` plan on a stale clip snapshot
**Severity:** low today (latent; becomes reachable with any second trigger) · **Verified:** Confirmed · **Where:** `commands.rs:1083-1089`, `1107-1116`; `edit.rs:373-378`, `406-414`
**When** two apply requests each trim 0–10 s off the same 60 s clip, **then** run 2 still sees `original: None` and 60 000 ms, cuts the already trimmed file at a relative 10 s, and bakes a 10 s offset between picture (original 20 s) and stems (original 10 s). Today the UI blocks a second request; the facecam's phase-2 job, a layout-only save or a console action would make it reachable.
**Why:** the clip is read and the plan built before the lock; only "the original vanished" is re-checked under it.
**Fix:** take the lock first, then re-read the clip and plan inside it (e.g. `edit::apply(id, reload: impl Fn() -> Option<Clip>, …)`).

#### EDIT-8 — A failure after the swap leaves a stale record, and the next save cuts at the wrong place
**Severity:** low · **Verified:** Confirmed (needs an I/O or DB error) · **Where:** `edit.rs:503`; `commands.rs:1418-1427`
**When** `write_note` fails after a first front cut (disk full, AV lock) or a DB write in `store()` fails, **then** `apply` returns an error while the file is already trimmed and the DB still says untrimmed. The stems preview is off by the cut until a restart; a second save in that session cuts at the wrong place (original 5 s instead of 15 s). It heals at the next start (`repair`, `settle`).
**Why:** `write_note(...)?` runs after `swap_in` succeeded; the three DB writes have no compensation.
**Fix:** make the post-swap steps non-fatal (log the note error, always return `Applied`); when the `store()` writes fail, notify and run `settle()` for that clip at once.

#### EDIT-9 — The waveform is not refreshed after save or undo
**Severity:** low · **Verified:** Confirmed (the WebView cache across reopens is an inference) · **Where:** `ClipPlayer.tsx:327-337`; `preview.rs:37-40`
**When** 10 s are cut off the front of a 30 s clip, **then** the 30 s waveform stays stretched over the new 20 s timeline, up to 10 s off, just when it is needed for the next trim.
**Why:** the effect depends on `[clipId]` only, and the URL has no cache-buster.
**Fix:** depend on `[clipId, clip.sizeBytes, reload]` and append `?v=${clip.sizeBytes}`, as the thumbnails do.

#### EDIT-10 — A failed save throws away the selection and moves the playhead
**Severity:** low · **Verified:** Confirmed · **Where:** `ClipPlayer.tsx:572-575`, `586-600`, `775-786`
**When** `replace_file` gives up (the clip is open in another player), **then** the fresh element loads the unchanged file, the trim snaps back to the stored edit and the playhead jumps back by `trim.start`.
**Why:** `resumeAt` is computed before the outcome is known, and `restoreTrim` always overwrites the trim on load.
**Fix:** keep the current trim (a `keepTrim` ref set in `catch` that `onLoadedMetadata` respects) and apply the `- trim.start` shift only on success.

#### EDIT-11 — A custom export size with some decimals fails silently
**Severity:** low · **Verified:** Confirmed · **Where:** `ExportDialog.tsx:109`, `111-113`, `177-182`; `commands.rs:1232`
**When** the user enters 9.3 MB, **then** `9.3 × 1 048 576` is not whole, serde refuses the float for `u64`, the command never runs and no notice appears (9.5 and 9.25 work).
**Why:** the dialog assumes the core "already raised the failure as a notice"; it never ran.
**Fix:** `Math.floor(targetMb * MB)`, and show an error in the `catch`.

#### EDIT-12 — For single-track clips, level and mute do nothing in the preview but apply on save
**Severity:** low · **Verified:** Confirmed · **Where:** `useClipMix.ts:130`, `179`, `283`, `300-303`; `ClipEditor.tsx:116-124`
**When** a one-source user mutes the track in the editor, **then** the preview still plays sound, and Save writes a silent clip (`-an`). The README promises that preview and clip sound alike.
**Why:** `separate` needs more than one track; otherwise no graph exists and the fallback loop runs over an empty map.
**Fix:** for one track, set `video.volume = muted ? 0 : master * min(1, gainFactor(db))`, or route the single stem through the graph.

#### EDIT-13 — Cuts of 250 ms or less are shown and saved as trims, but the core drops them
**Severity:** low · **Verified:** Confirmed · **Where:** `edit.rs:303-304`; `ClipEditor.tsx:88`; `ClipPlayer.tsx:1094`
**When** the user presses I at 0.2 s to drop a glitch frame, **then** the pane shows a trim and enables Save, the core remuxes without the cut, and after the reload the handle snaps back to 0 with no message (and the resume point shifts by the uncut start).
**Why:** the core's tolerance is 250 ms; `trimmed` and `dirty` use 50 ms.
**Fix:** snap in the UI (start under 0.25 s → 0, end within 0.25 s of the duration → duration) and use 0.25 s for `trimmed` and `dirty`, as `reencodes` already does.

#### EDIT-14 — The progress listener can leak in ClipPlayer
**Severity:** low · **Verified:** Confirmed · **Where:** `ClipPlayer.tsx:314-325`
**When** the component unmounts before `listen()` resolves (StrictMode in development, very fast paging), **then** `unlisten` is never set and the listener stays registered.
**Why:** `unlisten` is assigned in `.then`.
**Fix:** the `cancelled` pattern from `ExportDialog.tsx:80-96`.

#### EDIT-15 — `deleteClip` removes optimistically, never rolls back, and leaves the rejection unhandled
**Severity:** low · **Verified:** Confirmed · **Where:** `store.ts:499-502`; callers `Clips.tsx:341`, `582`, `Dashboard.tsx:319`
**When** `delete_clip` fails ("clip database is not available"), **then** the clip has already vanished from the list, nothing re-reads it, and the rejection is unhandled.
**Why:** callers use `void deleteClip(id)`; there is no `try`.
**Fix:** `try/catch`; on error re-read with `api.listClips()` and set `lastError`.

#### EDIT-V1 — The export dialog's preview leaves out `bitrate_for`'s 2 000 kbit/s floor
**Severity:** low · **Verified:** Confirmed · **Where:** `ExportDialog.tsx:47-49` against `encode.rs:78-84`
**When** a portrait or square source is exported (1080×1920 at the 720 step: 1 000 in the UI, 2 000 in the core) with a budget between 500 and 1 000 kbit/s, **then** the dialog shows 404×720 while the core encodes 270×480. The 9:16 export (spec section 3.5) will hit exactly this.
**Why:** the TS mirror of the recipe skips the clamp.
**Fix:** `Math.min(Math.max(wants, 2000), 100000)` in `preview()`, or fetch the recipe from the core once per dialog open.

### Recordings and screenshots

#### REC-1 — Sleep or a long stall during a recording ruins A/V sync for good
**Severity:** medium–high · **Verified:** Confirmed (needs QPC to advance across sleep; commit fe9a979 and `pipeline.rs:811-815` describe exactly that) · **Where:** `pipeline.rs:816-833`; `recorder.rs:300-305`, `366-372`, `459-472`, `625`; `convert.rs:762-767`; `muxer.rs:251-264`
**When** a recording runs and the lid is closed, **then** the video pts jump by the sleep, the mixer advances the recorder's timeline over the gap, and the writer pads it with real zeros. 60 min of frames + 10 min of sleep: everything is spread over 70 min, the file plays at ~51 fps, and just after the wake the picture is 5 min early. 15 min + 60 min of sleep + 15 min: nominal rate, span 30 min, the audio is cut to 30 min, so everything after the wake is silent. Meanwhile 192 kB/s of zeros per track are written (8 h × 4 tracks ≈ 22 GB) while post-wake packets queue in RAM (REC-7).
**Why:** the span includes the gap and only one `-r` is used; `finalize` deletes the raw folder on success, so nothing can be repaired.
**Fix:** detect the gap on the video side: a pts jump above `max(1 s, MAX_WINDOW)` in `Recorder::video` records `Gap { at_us, len_us }` in `Meta` (persisted); `finish` subtracts it from `span_us`; `finalize` skips the matching PCM ranges while aligning. Let the mixer's gap branch send a virtual-silence marker instead of gigabytes of zeros, materialised only where no video gap matches. Do not simply stop advancing the recorder's timeline in the gap branch: a mixer-only stall would then shift the audio for the rest of the file.

#### REC-3 — The disk-space auto-stop fails past ~47 minutes when two or more sources feed the main mix
**Severity:** medium · **Verified:** Confirmed · **Where:** `lib.rs:906-911`, `932-935`; `recorder.rs:246-250`, `485`, `601-604`
**When** a recording longer than 2 796 s with game + mic in the main mix (the default) is auto-stopped at `free < needed + 512 MiB`, **then** `finalize` first writes `mix.pcm` (192 kB/s × length, 691 MB per hour) on the clip drive, then the `.part.mp4`, and fails with "disk full". The partial `mix.pcm` stays and eats the last free bytes on a drive the game shares; the stop fails again at every start until space is freed.
**Why:** the estimate counts video and AAC only, not the temporary mix.
**Fix:** add `192 000 × seconds` to both thresholds when two or more tracks feed the mix, and remove `mix.pcm` on error. Long term, drop `mix.pcm`: feed the raw tracks to ffmpeg and sum them in the filter graph (`amix=normalize=0`, `asplit` into the clip track and the "Main mix" stem).

#### REC-7 — The writer queue is unbounded; a slow drive grows memory until the process aborts
**Severity:** low–medium · **Verified:** Plausible (needs a clip drive slower than the data rate, e.g. 4K at ~100 Mbit/s onto USB 2, an SD card or an SMR HDD past its cache) · **Where:** `recorder.rs:155`, `215-219`, `232`, `307`
**When** the drive sustains less than bitrate/8 + 192 kB/s per track, **then** the queue grows at the deficit (5 MB/s ≈ 18 GB/h) with no report until allocation fails and minutes of recording are lost. Stop and Quit drain the whole backlog while holding `lifecycle`, so they can hang for minutes.
**Why:** `channel::<Job>()` is an unbounded `mpsc`.
**Fix:** count bytes in flight (an `AtomicU64` added on send, subtracted in the writer); above ~512 MB set `failure = "the clip drive cannot keep up"`, which goes through the normal auto-stop. Never block in `video()`, which runs under the ring lock. Show the queue in the health line.

#### REC-6 — A recording recovered after a crash or a Windows shutdown is muxed at the nominal rate
**Severity:** low · **Verified:** Confirmed · **Where:** `recorder.rs:69-78`, `558-567`; `muxer.rs:259-262`; `lib.rs:1409-1423`
**When** frames were lost during the recording and the app then crashes or Windows logs off, **then** `frames`/`span_us` are missing at recovery, the nominal rate is used, and the picture runs ahead of its sound (36 s per hour at 1 % loss) with the audio tail cut off. The unflushed writer tails (~1.4 s) are lost too.
**Why:** `frames` and `span_us` are written only by a clean `finish`; there is no `WM_ENDSESSION` handling, so `abandon_recording` never runs.
**Fix:** rewrite `frames`/`span_us` (and REC-1's gaps) into `meta.json` every ~10 s, or write a `video.pts` sidecar (8 B per frame). Hook `WM_QUERYENDSESSION`/`WM_ENDSESSION` in a window procedure (Tauri's `RunEvent` does not expose it, an inference to confirm) and call `abandon_recording`.

#### REC-8 — Duplicate stops show "No recording is running" while the real stop is still muxing
**Severity:** low · **Verified:** Plausible (the first stop thread must take over 1 s to reach `close_recording`, or the user presses Stop in that window) · **Where:** `lib.rs:1083-1089`, `236-239`; `state.rs:1148-1150`
**When** a write fails, **then** the 1 s tick spawns a stop every second while `failure` is set; a second stop gets "No recording is running", an error sound and banner, and its unconditional `recording_progress(1.0)` drops the progress bar of the stop that is still encoding.
**Why:** no in-flight guard, unlike `STOPPING` at `lib.rs:923`, `936-938`.
**Fix:** one shared "stop in flight" flag for all stop paths; skip `recording_progress(1.0)` and the error UI when no recording was running.

#### REC-9 — Two screenshots in the same millisecond write into the same PNG
**Severity:** low · **Verified:** Plausible (needs two triggers in the same ms, e.g. hotkey plus Stream Deck) · **Where:** `state.rs:945-946`, `967`, `974`, `1283-1288`; `shot.rs:267`
**When** two `take_screenshot` calls reach `destination()` in the same millisecond, **then** both get the same path, two `File::create` handles write one file and the PNG can be corrupt; the second library insert fails on the unique path. A crash mid-write leaves a broken PNG with no row.
**Why:** the stamp has ms resolution, Rust's default share mode lets a second writer in, and the PNG is written straight to its final name.
**Fix:** `OpenOptions::new().write(true).create_new(true)` with a suffix on `AlreadyExists`, and write via `.part` + rename as clips do.

#### REC-10 — The screenshot callback reads back frames after the first
**Severity:** low · **Verified:** Confirmed · **Where:** `shot.rs:541-543`; `wgc.rs:222`, `249`
**When** a screenshot is taken during a high-fps game, **then** usually one more frame runs a full readback (a 33 MB staging copy, a `Map` stall, a 25 MB allocation and a CPU conversion at 4K) before the full channel is noticed, exactly while gaming.
**Why:** `sender.try_send(read_back(...))` evaluates the readback first.
**Fix:** an `AtomicBool taken`, checked with `swap(true)` before `read_back`.

#### REC-11 — A source renamed mid-recording keeps its old track label
**Severity:** low · **Verified:** Confirmed · **Where:** `recorder.rs:327-331`, `373`
**When** a separate-track source is renamed during a recording, **then** its stem keeps the old label. Main-mix sources are labelled "Main mix" anyway.
**Why:** the label is taken once at `open_track`; the comment at `recorder.rs:373` ("labels as they stand now") is not true.
**Fix:** keep the `Arc<TrackRing>` in `TrackState` and refresh `meta.tracks[i].label` from `ring.label()` in `finish`.

#### REC-12 — The same recording folder can be finalized twice
**Severity:** low · **Verified:** Plausible (needs ffmpeg on PATH while the bundled one downloads, and a stop still muxing when the download ends) · **Where:** `tools.rs:156-158`; `state.rs:1035`, `1205-1212`; `recorder.rs:602-604`
**When** a stop's `finalize` is still encoding as the bundled ffmpeg finishes downloading, **then** `recover_recordings` finalizes the same folder again: two MP4s (one without a row), both writing `mix.pcm` and the same stems, both calling `remove_dir_all`.
**Why:** `recover_recordings` excludes only the live recorder; the stopped one was already taken.
**Fix:** a process-wide `Mutex<HashSet<PathBuf>>` of folders being finalized (or a `create_new` lock file), checked by `recover_recordings`.

#### REC-V1 — Changing the clip folder during a recording watches the wrong drive and orphans the recording
**Severity:** low · **Verified:** Confirmed (mechanism; rare) · **Where:** `commands.rs:69-86`; `lib.rs:898-903`; `recorder.rs:148`; `state.rs:1178`, `1206-1209`
**When** a recording runs to D: and the user moves the clip folder to E:, **then** the disk guard measures E: while D: fills; if D: fills, the finalize fails and keeps the folder on D:, and recovery only looks in E:, so the recording is never finished or shown although its files remain. The same follows a quit, a crash or any failed finalize after such a change.
**Why:** only the capture source is locked while recording; free space and leftovers use the current `clip_dir`.
**Fix:** measure free space on `recorder.dir`'s drive while recording; let `recover_recordings` scan recent clip folders too (or keep a list of work folders in the data dir); or lock `clip_dir` while recording, like the source.

#### REC-V2 — An empty recording folder is retried at every start and never cleared
**Severity:** very low · **Verified:** Confirmed · **Where:** `recorder.rs:558-565`; `state.rs:1220-1225`
**When** the app crashes within ~0.5 s of starting a recording (before the first 1 MiB flush), **then** `meta.json` sits next to an empty `video.h264`, `finalize` fails with a warning at every start, and `.recording` stays visible in the clip folder.
**Why:** `count_frames` returns `None` or 0, `finalize` returns an error and keeps the folder.
**Fix:** when `frames == 0` or the video cannot be read and the folder is older than this run, remove it (or move it to `failed/`).

### Console and preview

#### CON-1 — The console cannot call `link_exists`, so "Delete with its link?" never appears
**Severity:** medium · **Verified:** Confirmed · **Where:** `Console.tsx:764-775`; `capabilities/console.json:9-34`; `build.rs:78`
**When** a player deletes a clip with a live share link from the console, **then** the `link_exists` call is refused by the permissions, the refusal is swallowed by `.catch(() => {})` (`Console.tsx:773`), the plain question appears, and `delete_clip` takes the public link down without warning (`commands.rs:774`).
**Why:** fb1a795 narrowed the console's grants and missed this one; it is the only console call without a grant.
**Fix:** add `"allow-link-exists"` to `console.json` (it is read-only). Add a check that every command a window's bundle calls is granted to it; a grep for `api.<name>` misses this call, which is split over two lines.

#### CON-2 — Alt+F4 on the open console destroys it for the rest of the session
**Severity:** medium · **Verified:** Plausible (that WebView2 forwards Alt+F4 to the console window as `CloseRequested`, as `lib.rs:1314-1315` says it does for the main window) · **Where:** `lib.rs:1313-1322`; `console.rs:56-58`, `90-104`, `174-177`
**When** the console has the focus over the game and the player presses Alt+F4, **then** nothing prevents the close, the window is destroyed, and from then on the hotkey only logs "console window missing" until ClippiBoy restarts. A cam preview dies with it.
**Why:** `CloseRequested` is prevented only for `main`; `create` runs once at startup.
**Fix:** in `console::create`'s `on_window_event`, handle `CloseRequested { api, .. }` with `api.prevent_close()` and `close(app)`; the same for `overlay`. Optionally re-create a missing window in `open()` (fine on the hotkey path, which runs in `run_on_main_thread`; never from a sync command or a WebView2 event, where building a window can deadlock).

#### CON-3 — The console and banner capabilities grant much more than they use
**Severity:** low (needs injected script in those webviews first) · **Verified:** Confirmed (that window commands can target other windows by label is plausible) · **Where:** `capabilities/console.json:9-12`; `capabilities/overlay.json:9`; `store.ts:305-308`
**When** a script runs in the console (it renders friend names, avatar URLs, clip titles and share state) or in the banner, **then** it can create or re-menu the tray icon, build menus, read PNG files from any path (`core:image:default`), hide, show and focus windows, and emit events. A forged `config-changed` reaches the main window, which takes it wholesale and persists it with its next `patchConfig`. The console can also insert new audio sources through `update_audio_source` (`state.rs:496-499`).
**Why:** `core:default` and `core:event:default` include all of that; no console or banner code emits events or imports the window, tray or menu APIs.
**Fix:** console: replace `core:default` and the three `core:window:*` grants with `core:event:allow-listen` and `core:event:allow-unlisten`; banner: the same instead of `core:event:default`. On `config-changed`, the main window re-fetches with `get_config` instead of trusting the payload; the console's source update refuses unknown ids. Test the console by hand afterwards; no test covers the permissions.
(also: CFG-6)

#### CON-4 — `emit_to` does not keep the 20 Hz level stream out of other windows
**Severity:** low · **Verified:** Plausible (rests on how Tauri 2.11 matches `emit_to` against listeners registered for any target) · **Where:** `lib.rs:986-1011`; `store.ts:275`; `console/Audio.tsx:40`
**When** the main window is visible on a second monitor while gaming, **then** the hidden console still evaluates `audio-levels` 20 times a second (its listener lives for the page's whole life); when the console is open, the hidden main window runs `set({ levels })` 20 times a second.
**Why:** both pages use the global `listen()`, which matches every `emit_to`; the `window_awake` gate saves only the serialisation.
**Fix:** give each target its own event name (`audio-levels:console`), so a webview without a handler is skipped, and listen window-scoped (`getCurrentWebviewWindow().listen`). Verify with a log in `store.ts:275` while only the console is open.

#### CON-5 — The hidden console re-renders its whole tree up to three times a second for unchanged data
**Severity:** low · **Verified:** Confirmed · **Where:** `Trouble.tsx:41-42`; `Console.tsx:349-353`; `Trend.tsx:64-68`; `lib.rs:1105-1124`
**When** the app runs, the whole game included, **then** `audio-errors` and `audio-warnings` arrive every second as new objects (usually `{}`), plus `engine-status`; each re-renders the 1 528-line `Console` (1–3 renders a second, React may merge them), on a renderer that is never backgrounded and draws on the CPU.
**Why:** `setErrors(e.payload)` with a fresh object; `trend.push` itself sets state owned by `Console`.
**Fix:** `setX(prev => sameMap(prev, next) ? prev : next)` for errors and warnings; while `hidden.current`, collect trend samples and `status` in refs and commit them in `begin()`.

#### CON-6 — A screen change during the closing fade brings the console back; while open it resets the player
**Severity:** low · **Verified:** Confirmed · **Where:** `console.rs:238-253`; `Console.tsx:299-310`, `394-443`; `lib.rs:553-580`
**When** the player presses Escape and `watch_monitors` notices a change on another monitor within the ~160–460 ms fade, **then** `relayout` emits `console-opened`, `begin()` clears `closingRef`, the pending close does nothing, and the console comes back up and needs a second Escape. While a clip plays in the console, the same relayout resets the player and the panel.
**Why:** every `console-opened` is treated as a fresh opening.
**Fix:** a separate `console-relayout` event that updates scale, inset, glow and style without `begin()`. It matters for the facecam's own relayout fields.

### Audio

#### AUD-2 — One late timestamp inserts a fake gap, then the source plays late for up to minutes
**Severity:** medium · **Verified:** Confirmed (the trigger depends on drivers: devices on the fallback clock or with 0 stamps, which the code says "happens often") · **Where:** `audio/capture.rs:67-78`, `659-663`, `673-686`; `audio/ring.rs:145-163`
**When** such a stream's thread stalls 60 ms or more (CPU load in a game; MJPEG decoding once the cam exists), **then** the first packet is stamped "now", S − 10 ms of zeros go in, and the burst that follows sits S − 10 ms ahead, pulled back only at 2 ms/s: a 100 ms stall gives a 90 ms dropout and ~90 ms lag for ~45 s; the worst case (the 200 ms WASAPI buffer) is ~190 ms lag for ~95 s. A `TIMESTAMP_ERROR` stamp 50 ms–1 s in the future does the same.
**Why:** only SILENT (0x2) is honoured; TIMESTAMP_ERROR (0x4) and DATA_DISCONTINUITY (0x1) are ignored; positive drift is zero-filled at once, negative only slewed.
**Fix:** pass "no stamp" for 0 stamps and for `flags & 0x4` instead of "now"; on the fallback clock estimate the capture time as `now − queued_frames/rate` (`GetNextPacketSize`/`GetCurrentPadding` before draining). Test: fallback clock, a 300 ms stall with 20 queued packets → ring end within 1 ms of the true time after the burst.

#### AUD-3 — A "Windows default" output source never follows a change of the default device
**Severity:** medium · **Verified:** Confirmed · **Where:** `audio/capture.rs:349-356`, `519-525`; `audio/engine.rs:39-42`, `195-211`
**When** the user plugs in a headset or switches the default output, **then** Windows routes the game to the new device, but the stream stays on the old endpoint, which is still valid and raises no error: the track goes silent with no warning, while `configuration_warnings` names the new default as what it "follows" (`audio/mod.rs:415-426`).
**Why:** the default endpoint is resolved once per attempt, and the stream key `out:` never changes, so `refresh` never restarts it.
**Fix:** in `AudioEngine::refresh`, rewrite `OutputDevice{device_id: ""}` to the resolved `default_output_id()` before building streams and keys (never in the config); keep the last good id when enumeration returns `""`; add `default_input_id()` for input. The 2 s tick then restarts the stream. The same normalisation fixes AUD-11.

#### AUD-4 — A `capture::start` timeout leaks an unstoppable capture thread that keeps the device open
**Severity:** medium (privacy: the mic stays open) · **Verified:** Confirmed · **Where:** `audio/capture.rs:127-136`, `418`, `602-604`; `lib.rs:1027-1029`
**When** opening a device takes more than 5 s (a Bluetooth headset switching to HFP, a slow driver, an activation finishing at 4.9 s), **then** `start` returns an error without setting `stop`; the thread finishes its init, its `ready.send` fails silently, and it pumps forever into an orphan ring. The retry 10 s later opens a second client. The mic indicator and HFP stay on after the source is removed, until the app exits; an always-slow driver leaks one thread per retry.
**Why:** `Err(_) => Err("timed out starting the audio source".into())` drops the `stop` flag unset and detaches the thread.
**Fix:** `stop.store(true)` before returning on timeout (enough: `attempt` checks it right after `Start`, `capture.rs:613`); stop and return when `ready.send` fails; make the activation wait shorter than the receive timeout.

#### AUD-V2 — SILENT packets from non-48 kHz devices are not resampled
**Severity:** medium (can be high) · **Verified:** Plausible (the code defect is certain; it needs SILENT-flagged packets from a whole-device loopback or a mic with a non-48 kHz mix format) · **Where:** `audio/capture.rs:676-678`
**When** a 96 kHz device sends SILENT packets with honest stamps, **then** each 10 ms packet adds 20 ms of timeline, the drift falls until the ring restarts at −1 s, and when sound resumes the source lags by up to ~1 s for up to ~8 min, or reads as silence past ~920 ms. At 44.1 kHz the residue is up to 50 ms early for ~25 s.
**Why:** `converted.resize(frames as usize * CHANNELS, 0.0)` uses the device's frame count; non-silent packets go through `convert`, which resamples.
**Fix:** derive the output frame count with the same rule as `convert`, or run the zeros through the resampler (and its phase, once AUD-7 is fixed). Test at 44.1 and 96 kHz with alternating silent and non-silent packets: ring end within 1 ms of the stamps.

#### AUD-6 — The mixer timeline accumulates rounding, so recording audio drifts late ~8–12 ms per hour
**Severity:** low · **Verified:** Confirmed · **Where:** `pipeline.rs:806`, `868`; `recorder.rs:306-316`, `566-577`
**When** a recording runs for hours, **then** each mixer window loses on average a third of a 100 ns tick, the source rings are read by QPC while the recorder counts samples, and the sound sits ~8–12 ms late per hour (~50–70 ms after a 6 h stream, where a talking face shows it). One sample is read twice every ~6–10 s.
**Why:** `next = from + ⌊frames·625/3⌋` per window. Buffer clips of continuously fed tracks are not affected (the trim at `pipeline.rs:165` cancels it); silent stretches are AUD-V1.
**Fix:** compute `next = origin + ⌊total_frames·625/3⌋` from running totals, counting the gap path (`pipeline.rs:831`) too. Do not make only the trim at `pipeline.rs:165` exact: buffer clips would then drift instead; change both or only the mixer. Test: 1 h of random 10–11 ms windows, `|Σframes·1e7/48000 − (next − origin)| ≤ 1 tick`.
(also: REC-5)

#### AUD-V1 — Counted silence makes buffer-clip tracks drift late
**Severity:** low · **Verified:** Confirmed (arithmetic traced) · **Where:** `pipeline.rs:125-143`, `150-157`, `160-166`, `868`
**When** a track is often silent but never for a whole ring length (Discord voices between sentences, a gated mic), **then** its mapping error grows by ~1/3 tick per silent window; in a 3 h session with ~70 % silence that track plays ~25 ms late in buffer clips.
**Why:** silent windows are only counted, and the single trim later does not cancel the mixer's per-window floor, as it does for continuous tracks.
**Fix:** the totals-based `next_100ns` from AUD-6. Test: alternate 1 s of sound and 1 s of zeros for 1 h of simulated windows; `TrackRing::window(from_k)` returns the block pushed at `from_k` within one frame.

#### AUD-7 — 44.1 kHz devices: the per-packet resampler drops the fractional frame and has no continuity
**Severity:** low · **Verified:** Plausible (needs packets other than the usual 441 frames, which resample exactly to 480) · **Where:** `audio/capture.rs:327-341`
**When** a 44.1 kHz endpoint delivers 448-frame packets, **then** 0.62 frames are lost per packet (~1 270 ppm, most of the ring's 2 000 ppm slew); with smaller or odd packets the loss exceeds the slew, the drift crosses 50 ms and silence is inserted every few seconds, plus a discontinuity at every packet boundary.
**Why:** `target_frames = (frames·ratio) as usize`, and each packet is resampled on its own.
**Fix:** a stateful resampler that carries the phase and the previous input frame across packets and emits `floor(phase + frames·ratio)`, keeping the rest; it must cover the SILENT path too (AUD-V2).

#### AUD-9 — `StreamHandle::stop` can block for up to ~5 s and delays the replacement stream
**Severity:** low · **Verified:** Confirmed · **Where:** `audio/capture.rs:90-96`, `527`; `audio/engine.rs:92-96`, `191-193`
**When** a stream in back-off (device unplugged) is replaced, e.g. the source is switched to another device, **then** `apply` joins it while it sleeps up to 5 s, before starting the new stream; audio is missing for that long and other refreshes are dropped meanwhile.
**Why:** `std::thread::sleep(wait)` ignores `stop`; stale streams are stopped one after another before new ones start.
**Fix:** sleep in ≤ 50 ms slices checking `stop` (or wait on an event); start missing streams before joining stale ones.

#### AUD-10 — Event handles leak on process-loopback error paths
**Severity:** low · **Verified:** Confirmed · **Where:** `audio/capture.rs:564-601` (event from `565`, returns at `567-572`, `586-593`, `596-601`); `audio/capture.rs:384`, `411-416`
**When** a process tap keeps failing in `GetService`, `Start` or `ActivateAudioInterfaceAsync`, **then** one event handle leaks per retry, ~8 600–17 000 a day.
**Why:** the handle is closed only at the normal end (`693-695`).
**Fix:** an RAII guard around the handle in both functions.

#### AUD-11 — The "playing nowhere goes to the default" rule never matches a Windows-default bucket
**Severity:** low · **Verified:** Confirmed · **Where:** `audio/mod.rs:243-248`; `audio/engine.rs:197`
**When** "System = Windows default" and a "Chat" device are configured and Steam idles on both, **then** the rule fails and Steam lands in the first bucket in mixer order, possibly the chat channel.
**Why:** the bucket's device is `""`, `default_device` is the real id.
**Fix:** normalise `""` to `default_device` before comparing (the AUD-3 normalisation does it).

#### AUD-V3 — The startup `apply` blocks the main thread up to 5 s per stream that times out
**Severity:** low · **Verified:** Confirmed · **Where:** `lib.rs:1225`; `state.rs:361-367`; `audio/capture.rs:127`
**When** a slow Bluetooth mic and a Bluetooth output are configured, **then** the app freezes ~10 s before its window exists, and the timed-out streams leak per AUD-4.
**Why:** `AppState::new` runs inside `.manage(...)` on the main thread and calls `audio.apply` synchronously.
**Fix:** run the startup `apply` through `refresh_async`, or spawn it.

### Settings and permissions

#### CFG-1 — A source muted from the console is silently un-muted by the next settings change in the main window
**Severity:** high (privacy) · **Verified:** Confirmed · **Where:** `Console.tsx:552-567` → `commands.rs:340-343` → `state.rs:494-501`, `419-457`; `store.ts:371-388`; `commands.rs:54-90`
**When** the player mutes the mic in the console and later changes any setting in the main window (a banner toggle, a slider, a mixer reorder), or the usage notice writes `usageStatsNoticed` by itself (`UsageNotice.tsx:41`), **then** `patchConfig` sends the whole stale config, `set_config` takes `sources` wholesale, and `replace_config` re-applies them and pushes them into a running recording. The mic is recorded again and shows as unmuted.
**Why:** `upsert_source` → `replace_config` emits nothing; the only `config-changed` emitter is `repair_screens` (`lib.rs:542`); the main window reads the config once (`store.ts:266`).
**Fix:** emitting `config-changed` alone is not enough: a write already in flight still carries the old sources, and echoes of the window's own writes would reset newer optimistic edits. Add a config revision number that every `set_config` sends back; the core merges an older revision field by field instead of replacing. Emit `config-changed` only to `main` and only from write paths it did not start (`emit_to("main", …)`). The spec's "Cam off" depends on this (section 5.1).

#### CFG-3 — "Export for Discord" re-sends an old export after the clip was edited
**Severity:** medium (privacy) · **Verified:** Confirmed · **Where:** `commands.rs:943-960`
**When** the user exports a clip at 10 MB, then trims it or mutes a voice track, and exports again, **then** `"<stem> (10 MB).mp4"` exists and is under the limit, so the old file, with the removed part, goes on the clipboard. An export cut short by a crash also stays as an unplayable file that is handed out every time.
**Why:** `let ready = metadata(&output).map(|m| m.len() > 0 && m.len() <= target_bytes)`; nothing compares it with the clip.
**Fix:** reuse only when `stems::is_newer(&output, &source)` (a save's rename keeps the new file's mtime); make `shrink` write to a temp name and rename on success; delete the `"{stem} (* MB).mp4"` siblings in `store()` and `delete_clip` (otherwise `filing::prune` cannot remove the game folder).
(also: EDIT-2)

#### CFG-2 — The config file can be torn or emptied, and a broken file is overwritten with defaults at the next start
**Severity:** medium impact, very low probability · **Verified:** Confirmed (mechanism; the crash case is plausible) · **Where:** `config.rs:167-172`, `176-184`; `state.rs:421-424`, `453-455`; `lib.rs:499`, `534`; `control.rs:443-447`
**When** a real screen re-enumeration (or `set_clip_dir` on a worker thread) saves at the same moment as a settings write, **then** both write the same `config.json.tmp` and torn JSON can be renamed into place; more likely, a BSOD or power loss after a write leaves a renamed file of NUL bytes. At the next start `load()` falls back to defaults and `ensure_token` saves them over the broken file: every setting, source, hotkey and the clip folder are gone, with no backup.
**Why:** `replace_config` saves after releasing the config lock, while other writers save under it; there is no `sync_all` before the rename.
**Fix:** call `config::save(&next)` inside the guard in `replace_config`, as the direct writers do (a new `SAVE` mutex taken before `config` would invert the lock order); write `File::create` → `write_all` → `sync_all` → `rename`; in `load()`, treat only `NotFound` as a first start and move a broken file to `config.json.broken-<ts>` (or keep a `config.json.bak`) with a warning.

#### CFG-4 — "Export for Discord" always fails when the clip folder is a network share
**Severity:** low–medium · **Verified:** Confirmed (a regression from 8d18a92) · **Where:** `commands.rs:943`, `952-958`, `314-324`, `1235`
**When** the clip folder is `\\nas\videos\ClippiBoy` (which `usable_clip_dir` accepts), **then** the console's Discord export builds a sibling UNC path and passes it through `export_clip` → `export_target`, which rejects UNC with "Export to a .mp4 file on a local drive".
**Why:** the check meant for paths from the window also runs on a path the core built.
**Fix:** move the tail of `export_clip` (shrink, `progress(1.0)`, notice) into a private `export_to(state, app, id, target_bytes, &Path)`; the command validates with `export_target` and calls it, `export_for_discord` calls it directly.

#### CFG-5 — The whole-drive clip-folder check is bypassed with `..`
**Severity:** low (needs a compromised main webview) · **Verified:** Confirmed (the Tauri scope part checked against upstream source) · **Where:** `commands.rs:84-86`, `303-307`; `lib.rs:1135-1139`
**When** `set_config` receives `clipDir = "C:\\.."`, **then** `parent()` is `Some("C:\")`, the check passes, and the clip-folder grant becomes the canonical `\\?\C:\**`: the whole drive is readable through `asset:`, renewed at every start.
**Why:** Rust does not normalise `..`; Tauri canonicalises the pattern.
**Fix:** reject any `Component::ParentDir` in `usable_clip_dir`; let `set_config` always keep `previous.clip_dir` (the UI changes it only through `set_clip_dir`); in `set_clip_dir`, canonicalise after `create_dir_all` and check `parent()` on the canonical path. Add `r"C:\.."` and `r"D:\x\..\.."` to the test at `commands.rs:1528-1535`.

#### CFG-7 — Resolution, fps, quality, encoder and keyframe changes do not reach a running buffer, and nothing says so
**Severity:** low · **Verified:** Confirmed · **Where:** `commands.rs:114-132`; `state.rs:546-556`; `config.rs:159-163`; `Recording.tsx:121-122`, `295-376`
**When** the buffer runs at 1080p60 and the user picks 720p30, **then** clips stay 1080p60 until the buffer restarts, and `bitrate_kbps` stays stale until the next app start, so the "MB per clip" and memory estimates and the live ring resize use the old bitrate (BUF-V2).
**Why:** `set_config` restarts only on a target change; `bitrate_kbps` is re-derived only at load.
**Fix:** restart the buffer off-thread on these fields like a target change (not while recording), or show "applies at the next buffer start"; recompute `bitrate_kbps = encode::bitrate_for(w, h, fps)` in `replace_config`.

#### CFG-8 — Two quick source changes can leave the buffer on the first source
**Severity:** low (window targets; monitor targets heal through `screen_drift`) · **Verified:** Confirmed · **Where:** `commands.rs:119-132`; `state.rs:546`, `563`, `607-611`
**When** the user clicks window A, then window B within the few hundred ms the capture takes to start, **then** the second `set_config` spawns no restart, and the buffer records A while the UI shows B.
**Why:** `is_buffering()` is false for the whole start.
**Fix:** after `start_pipeline`, the restart thread compares the running source with the config and restarts again if they differ, or use a restart generation counter.

#### CFG-9 — If `getConfig` fails, the mock config becomes the real one, including a public Stream Deck token
**Severity:** low · **Verified:** Confirmed (code path; the trigger is very unlikely) · **Where:** `store.ts:167`, `266`, `371-381`; `mock.ts:121`; `commands.rs:66-68`
**When** `get_config` fails at init and the user then changes a setting, **then** `mockConfig` is sent and persisted: mock sources (pid 8899), a mock clip folder and `control.token = "mock-token"`, a constant from the repo, as the control-port token.
**Why:** the store falls back to `get().config`, which starts as the mock even in Tauri; `set_config` replaces only an empty token.
**Fix:** in Tauri, never fall back to the mock and block `patchConfig` until a real config has loaded; in `set_config`, always keep `previous.control.token` (only `regenerate_token` changes it).

#### CFG-10 — Exporting onto the clip itself under a differently spelled path destroys the clip
**Severity:** low (high impact, very low likelihood) · **Verified:** Plausible (depends on ffmpeg's same-file check being a string compare and on the save dialog returning the typed case) · **Where:** `commands.rs:1166-1169`, `314-325`, `1190`, `1200`, `1206`, `1212`; `export.rs:124`
**When** the user types the clip's own name in another case in the save dialog and confirms "replace", or the game folder's case on disk drifted from `clip.path`, or the path goes through an 8.3 name, a junction, `subst` or a hard link, **then** `output == source` is false, ffmpeg runs with `-y` and truncates the clip it is reading, and the cleanups delete what is left (with a hardware encoder, before the x264 retry). A drive-letter case difference does not trigger it: std compares drive letters case-insensitively.
**Why:** `Path ==` compares the remaining components byte-wise; nothing is canonicalised.
**Fix:** when `output` exists, compare `std::fs::canonicalize` of both (or volume serial + file index), plus a case-insensitive compare on Windows; refuse outputs inside the data folder; write to a `.part` and rename on success, so a miss turns into "replace", not "destroy".
(also: EDIT-4, SAVE-5)

#### CFG-11 — Every config write does heavy work on the main thread, and some controls write on every step
**Severity:** low · **Verified:** Plausible (mechanism confirmed; the cost was not measured) · **Where:** `state.rs:420` → `encode.rs:87-93`, `164-171` → `mft.rs:205-246`; `config.rs:176-184`; `AudioMixer.tsx:335-336`, `384-390`
**When** the user drags the mixer gain slider (0.5 dB steps) or types a source label, **then** every step runs `upsert_source` → `replace_config` on the main thread: `MFTEnumEx` (twice when the encoder is unavailable), a disk write, an audio refresh and `set_sources` into a running pipeline. The Sounds and clip-length sliders go through `set_config`, which adds a registry read and the asset-scope grant.
**Why:** these controls call the core on `onChange`, not on release.
**Fix:** `onCommit` or a debounce on the gain slider and the label input; cache the encoder list (`OnceLock`); re-grant the clip folder only when it changed; make `list_encoders` async.

#### CFG-V1 — Any read error at startup silently replaces the config with defaults, which are then saved over it
**Severity:** low · **Verified:** Confirmed (code path; rare) · **Where:** `config.rs:172`; `control.rs:69`, `443-447`; `lib.rs:1303`
**When** another program holds `config.json` at autostart (sharing violation), an ACL problem blocks it, or it is not valid UTF-8, **then** `load()` returns defaults without a log line and `ensure_token` saves them over the real file at the same start. No corruption is needed.
**Why:** `Err(_) => default_config()` covers every read error, not only `NotFound`.
**Fix:** match `ErrorKind::NotFound` for the first start; for any other error, log it and keep the file (move it aside, or set a "do not save" flag until a good load).

## What the facecam could break

"Section" means a section of `docs/superpowers/specs/2026-10-20-facecam-design.md`; "Open item" means its "Open for Fabi" list.

### Capture and encoding
- **Watermark and colour.** `Converter` always builds a watermark stamp and treats its input as full-range RGB (`convert.rs:162-170`, `183-189`, `323-343`), while the encoder claims BT.709 limited (`mft.rs:535-538`). → handled in section 2.2 (own device, `Converter` not reused). The stage-2 plan must still take the input colour space from the camera's `MF_MT_YUV_MATRIX`/`MF_MT_VIDEO_NOMINAL_RANGE` (MJPEG is usually full-range BT.601), check `CheckVideoProcessorFormat`, and switch off driver auto-processing (CAP-V1).
- **Texture assumptions.** The video processor declares input size = output size (`convert.rs:143-152`), input views are cached by pointer with array slice 0 (`convert.rs:262-276`), and `feed` always uses subresource 0 (`mft.rs:953`); texture arrays from a source reader would encode the wrong slice. → the stage-2 plan must upload into its own single-slice texture (the path section 2.2 describes) or key views by `(pointer, subresource)`.
- **Constants and knobs shared by both encoders.** `SLOTS`, `QUEUE_DEPTH` and the env variables (`convert.rs:75`; `mft.rs:393`, `462-463`, `1100`) apply to every instance, and so do CAP-2 and CAP-6. → only partly in the stage-2 plan: Task 11 gives the cam its own texture pool (no `convert::SLOTS`) and its own B-frame count (`bframes: Some(0)` over `CLIPPIBOY_BFRAMES`), but sizes that pool from the constant `mft::QUEUE_DEPTH`, while `CLIPPIBOY_QUEUE_DEPTH` and the other knobs (`mft.rs:462-463`, `1100`) still reach both encoders; ageing out `sent` (CAP-2) is `docs/extras/plans/2026-10-22-fix-data-safety-and-silent-failures.md` Task 1, per encoder instance; CAP-6 is in no plan. Open: worth adding to stage 2 — size the cam pool from the encoder's real `queue_depth` (`mft.rs:1075-1078`).
- **Hardware sessions.** Buffer, cam and composite run three encoder sessions (`edit.rs:778`, `export.rs:167`), and the MFT is never shut down (CAP-V2). → handled in section 2.3 (session count, software MFT as fallback, CAP-V2 fixed first).
- **Silent failure.** A dead cam encoder would only log (CAP-1). → handled in section 2.8 (errors through the capture channel); BUF-2's fix is stage 2 Task 1, and Task 11 gives the cam its own error slot, so it fails without touching the game buffer. CAP-1's error hook is not in the stage-2 plan: it is `docs/extras/plans/2026-10-22-fix-data-safety-and-silent-failures.md` Task 1 (`EncoderStats::failure`, per encoder instance). Open: worth adding to stage 2 — the cam worker reads its own encoder's `failure()` and restarts only the cam.
- **Anchor.** Copying `EncoderSink`'s anchor would copy CAP-4's race and the "age of the newest frame" error, random per session, up to one cam period (~33 ms at 30 fps). → handled in section 2.3 (a grid on the cam's own timestamps).
- **Shared immediate context.** Cam work on the game's device would queue behind the game's D2D section under the D3D lock (`gpu.rs:229-232`, `convert.rs:560-586`). → handled in section 2.2 (own D3D11 device).
- **COM and Media Foundation start.** The encoder thread has no COM init (CAP-8), and the camera APIs need it. → the stage-2 plan must call `CoInitializeEx(MTA)` on the camera thread and reuse `crate::mft::startup()` (`mft.rs:149`) instead of a second `MFStartup`.
- **Start and stop.** `VideoEncoder::start` can wait 10 s (`mft.rs:1135`), and `win::stop` drops the encoder before the capture (`pipeline.rs:770-776`, against the comment at `600`). → handled in section 2.2 (cam lifecycle outside the pipeline); the plan must stop camera source, grid and encoder in that order.
- **Polling.** Each encoder thread wakes ~1 000 times a second (`mft.rs:1327-1355`); a cam encoder doubles that. → not in the stage-2 plan (Task 22 measures composites, sessions, the preview and the ring, not this). Open: worth adding to stage 2 — measure it in a game, or move to `BeginGetEvent` with a callback.
- **Preview tap.** `Latest.converter` is private (`convert.rs:603`), and a slot copied outside the converter lock can be overwritten mid-copy. → handled in section 5.3 (preview from the raw camera frames); any GPU readback must copy under its lock.

### Buffer, pipeline and state
- **Cutting by "last N seconds".** `ReplayBuffer::snapshot` counts back from its own newest packet (`buffer.rs:181-200`). → handled in section 2.4 (`snapshot_range` by absolute time; stage 2 Task 2).
- **Cam ring too short.** The game cut reaches up to a GOP behind `newest − seconds` (BUF-1). → handled in section 2.3 (ring = buffer + game GOP + cam GOP + delay range + margin).
- **Waiting under the pipeline lock.** A wait for the cam ring right after the game snapshot would run under `AppState.pipeline` (`state.rs:883-892`) and freeze stops and the status tick (BUF-7), and before `muxer::build` it would make BUF-1 worse. → the spec (2.4) cuts the cam only after the clip is written, outside every lock, with a 10 s save margin in the cam ring. The cam tail can still lack its encoder latency: overlay `eof_action=repeat` at the end, not `pass`.
- **Track 0 everywhere.** Cam packets come out as track 0 (`mft.rs:1051-1057`); `Recorder::video` takes every track-0 packet (`recorder.rs:223-233`) and keyframes count only on track 0 (`buffer.rs:75-79`). → handled in section 2.3 (own ring, never through the game's callback).
- **Lock order.** The game encoder callback takes `packets` then `recorder` (`pipeline.rs:667-671`, `1004-1015`). → handled in section 2.3 (own lock, never taken with `Shared.packets`).
- **The game picture still drifts.** BUF-6 and SAVE-4 stay on the game side: face and voice stay in sync with each other while the game drifts against both mid-clip, and a cam encoder plus background composites make stalls more frequent. → not in the stage-2 plan; `docs/extras/plans/2026-10-22-fix-sync-and-timestamps.md` Task 7 fixes BUF-6 with real timestamps (Task 2 fixes SAVE-4) and should land before compositing — raising `CATCH_UP` instead would make it worse, since under a sustained shortfall the replayed ticks fall further and further behind the clock (`convert.rs:734`, `754-767`), which is why that plan forbids it.
- **Snapshot fields.** The clip size is read outside the snapshot (BUF-8), and `fit_to_target` also snaps the fps (`capture.rs:641`). → the cam's size, fps and offset travel with the cut, not the config (stage 2 Task 14, `CamCutOut`); the composite's GOP (Task 16) must come from the fitted fps of the running recording (`active_recording`, the value `Shared.fps` also holds), not from the config's `recording.fps`, since `fit_to_target` can change it.
- **Memory held during saves.** `muxer::build` keeps every packet of the snapshot until ffmpeg ends (`muxer.rs:115-225`). → not in the stage-2 plan or the fix plans. Open: worth adding to stage 2 — drop `snapshot.packets` once the elementary streams are written.
- **Lock-screen grace.** `poll_lock` waits 5 polls (10 s) and stops only an active buffer (`state.rs:187`, `228-251`; `lib.rs:671-675`). → handled in section 2.2: the cam follows the buffer's own lock rule on purpose, 10 s grace included (a UAC prompt raises the same secure desktop and must not cost the cam), and stops also while a recording keeps the pipeline alive (stage 2 Task 10, `AutoBuffer::screen_locked`).
- **Errors, metrics, budget.** Errors are invisible until BUF-2; health and status know one encoder (`pipeline.rs:370-387`, `520-540`); `effective_memory_bytes` has a 64 MB floor (`config.rs:100-110`). → handled in sections 2.8, 5.4 and 2.3, and Open item 7.
- **Sleep and one-keyframe streams.** BUF-4 and BUF-5 apply to the cam ring. → handled in section 2.3.
- **Cam off.** With the cam off, `ClipSnapshot.cam` must be `None` and the ffmpeg command byte for byte today's. → handled in sections 2.2 and 2.4; stage 2 leaves `muxer::build` untouched (Task 14 cuts the cam after it, in a run of its own, and `save_clip` returns a `CamWindow` only with the master switch and *in clips* on), but the test is not in the stage-2 plan; `docs/extras/plans/2026-10-22-fix-sync-and-timestamps.md` Task 6 pins today's save command (`the_clip_command_is_the_one_encode_built_before`). Open: worth adding to stage 2 — a test that the cam off gives no `CamWindow`.

### Saving, tracks, clip database, export
- **Zero point.** `-avoid_negative_ts make_zero` applies to the clip output only (`muxer.rs:356`, SAVE-7): in the written clip, the content at `start_100ns` sits at about +21.3 ms, while a separately muxed `cam.mp4` (section 2.6) starts at 0. → the stage-2 plan must settle SAVE-7 first, or measure each file's `start_time` and store it in `facecam.json`.
- **Input numbering.** Audio is assumed to start at input 1 and video is `0:v:0` (`muxer.rs:306-308`, `335`, `387-388`; `edit.rs:694`, `711`; `export.rs:138`), and the test at `edit.rs:1440-1445` counts every other map as audio. → handled in section 3.4 for the editor (cam as last input); the phase-2 command must follow the same rule, and that test must change.
- **No argument builder in `muxer::encode`.** The ffmpeg command is filled in inline and untested. → not in the stage-2 plan, which leaves `muxer::encode` as it is; `docs/extras/plans/2026-10-22-fix-sync-and-timestamps.md` Task 6 pulls out a pure `arguments()`, as `edit.rs` and `export.rs` have, with a golden test.
- **Crash or quit during phase 2.** SAVE-3's window grows to tens of seconds, with a new `<clip>.<pid>.cam.mp4` temp. → handled in section 2.6 item 4 (repair re-queues the job); SAVE-3's sweep must include the new name — `docs/extras/plans/2026-10-22-fix-data-safety-and-silent-failures.md` Task 7 sweeps `*.<pid>.cam.mp4` and phase 1's `originals/<id>/cam.part.*`.
- **Races with the background job.** `delete_clip`, `file_clip` (a heart moves the file, `commands.rs:817-842`) and `shrink` take no lock (EDIT-1, SAVE-6). → handled in section 2.6 item 5 for edits, deletes, exports and shares; the plan must add `file_clip`.
- **Stems folder wipes.** `stems::remove` deletes all of `tracks/<id>` (`stems.rs:146-152`, called at `muxer.rs:405`, `436`, `edit.rs:430`, `share.rs:688-788`). → handled in section 2.7 (cam files not in `tracks/`).
- **The data folder is Roaming.** `config::data_dir()` is `%APPDATA%\ClippiBoy` (`config.rs:10-13`), usually on C: and part of a roaming profile; a clean copy of every clip lands there. → Open item 2; the decision should take this into account.
- **PATH ffmpeg.** Saves may run with an untested ffmpeg from PATH before the pinned one is ready (`state.rs:872`; `muxer.rs:46-49`), which may lack `overlay`, `alphamerge`, d3d11va or the hardware encoders. → not in the stage-2 plan (`docs/extras/plans/2026-10-22-fix-data-safety-and-silent-failures.md` Task 10 only makes the PATH fallback need ffprobe as well). Open: worth adding to stage 2 — phase 2 requires `tools::is_ready()` and checks that the pinned build has every filter it uses.
- **Edit data dropped.** `store()`, `settle()` and the share path rebuild `ClipEdit` (`commands.rs:1413-1417`, `edit.rs:939-948`, `share.rs:887`), and `decode_edit` throws the whole edit away on any parse error (`clips.rs:376-387`). → handled in section 2.7 (own `facecam` column, also in `insert`); the plan must add it to `list`'s columns too and decode it field by field.
- **Thumbnail and received clips.** The thumbnail comes from the clip file (`muxer.rs:440`), and a shared clip arrives without the cam files (`share.rs:307-316`). → handled in section 2.6 (thumbnail after the swap); stage 3 must hide the cam controls when `facecam.json` is missing.
- **Recordings.** Compositing an hour at stop is minutes of re-encoding, and recovery would run it again (`recorder.rs:616-624`, `lib.rs:273-294`). → stage-4 plan (keep the cam as its own file and composite later).

### Editor
- **Double cam and undo.** The first cut archives the delivered file (`edit.rs:589-601`), front cuts re-encode from it (`edit.rs:318-322`), undo copies it back (`edit.rs:340-349`). → handled in sections 3.4 and 3.6 (always from the clean sources; undo re-composites) and 2.7 (`swap_in` never archives a composite).
- **`originals/<id>/` is wiped often.** `edit::remove` ends in `remove_dir_all` (`edit.rs:130-138`) on every untrimmed save (`625-627`), undo, `settle` (`911-914`), four `repair` arms (`975-981`, `988-997`, `1010-1011`, `1028-1033`) and delete; `trim_originals_on_disk` counts every `video.mp4` (`edit.rs:180-196`); `storage_usage` files other names there under "screenshot edits" (`commands.rs:1293-1298`); screenshot originals share the folder (`edit.rs:154-158`). → handled in section 2.7 for the untrimmed save, undo, "Clear all trims", `repair` and `swap_in`; the plan must also cover `settle`, delete, the storage categories and the screenshot files (Open item 2).
- **Stale plans.** EDIT-7 becomes reachable through the phase-2 job and layout-only saves. → re-read the clip and plan inside the per-clip lock `edit::clip_lock(id)` (spec 2.6 item 5), not `WORKING`, which the composite worker never takes; stage 2's job re-reads its row under that lock (Task 16), and an editor save should do the same.
- **Lock held for a whole composite.** `WORKING` is global (`edit.rs:53`, `406`), so a 40 s composite blocks every other edit and "Clear all trims". → handled in section 2.6 item 5: the job, edits, deletes, exports and shares of one clip take a per-clip lock (`edit::clip_lock(id)`, stage 2 Task 4); the job never takes `WORKING`, which stays for the all-clips operations such as "Clear all trims".
- **Editor texts.** "The video stays untouched" and the re-encode hint (`ClipEditor.tsx:92`, `180-185`, `256-259`) become wrong once a layout change re-encodes. → the stage-3 plan must update them from the table in section 3.4.
- **Save veil.** `veil.freeze` captures the clip's `<video>` (`ClipPlayer.tsx:581`), whose picture section 3.2 hides; the frozen still would show nothing or the old composite. → the stage-3 plan must freeze what is visible (a canvas from the clean and cam elements).
- **Rect layer.** `CropLayer` (`ShotViewer.tsx:1338-1514`) starts a new box on any click outside it (`1469`), covers the whole picture and swallows click-to-pause (`ClipPlayer.tsx:754-755`), resets a too-small box to the whole picture (`1445-1447`) and darkens around it (`1484`). → handled in section 3.3 (pulled out as `RectLayer`); stage 3 must change those four behaviours for the cam box.
- **Shared progress event.** `clip-progress` is keyed only by clip id and used by edit and export (`commands.rs:1369-1380`); a background composite and a console export of one clip would mix in one bar. → not in the stage-2 plan, whose job shows only the "Adding cam" pill (Task 16) and leaves open what its progress callback emits; stage 3 lists the risk and stage 4 adds a percentage. Open: worth adding to stage 2 — the job gets its own event or kind.
- **Portrait export.** The recipe steps on the source height (`export.rs:78-112`), `-vf` cannot sit next to `-filter_complex` (`export.rs:140-144`), the TS mirror lacks the 2 000 kbit/s floor (EDIT-V1), and `export_for_discord` reuses one fixed name (`commands.rs:943`, CFG-3). → handled in section 3.5 for the recipe; stage 3 must keep the floor in the mirror and give portrait files their own name.

### Recordings and screenshots (stage 4)
- **Old drifts become visible.** AUD-6 (~10 ms/h) and REC-1 (sleep) hide behind game audio; a talking face shows them. → the stage-4 plan must fix both first.
- **Keyframe at the start.** With the buffer off the video ring holds 2 s (`state.rs:14-16`, `558`, `595`), and the cam needs a cam keyframe at or before `start_100ns` (`pipeline.rs:1008-1012`). → stage-4 plan: size the cam ring for both GOPs, or force a cam IDR and accept a short lead-in.
- **Attach contract.** "No packet missed or written twice" rests on attaching under `packets` (`pipeline.rs:325-331`, `1004-1016`, `1022-1025`); the mixer clones `shared.recorder` without it (`pipeline.rs:819`, `849`), against that doc. → stage-4 plan: no joint order — the cam ring's lock is never taken together with `Shared.packets` (spec 2.3) or `Shared.recorder` (stage-2 Global Constraints); the game recorder is attached under `packets`, the cam tap after it under the cam ring's own lock, and nothing relies on the doc.
- **Writer.** One unbounded queue (REC-7) and file I/O under the ring lock (BUF-3) would carry the cam too. → the stage-4 plan must fix both before adding `cam.h264`.
- **Crash recovery.** `Meta.cam` must be written at start with `#[serde(default)]` (`recorder.rs:53-85`), and `finalize` must survive a missing or short `cam.h264` without deleting the folder (`recorder.rs:577-579`). → stage-4 plan.
- **Disk guard.** The estimate and thresholds ignore the cam and any composite (`recorder.rs:246-250`, `lib.rs:906-965`), and REC-3 already fails past ~47 min. → stage-4 plan.
- **Screenshot timing.** `grab` drops the frame's QPC (`shot.rs:541-543`), and capture callbacks are serialised by a mutex (`wgc.rs:249`), so a cam lookup inside the callback would hold up later frames. → stage-4 plan: return `qpc_100ns` from `grab` and match the cam frame outside the callback.
- **Screenshot with nothing running.** Screenshots are not among the reasons in section 2.2 for the cam to run, and opening a camera costs 0.5–2 s plus exposure settling (inference). → the stage-4 plan must decide: leave the cam out, or keep it open while *Facecam in screenshots* is on.
- **Watermark and alpha in screenshots.** The mark must stay on top (`state.rs:972`, `commands.rs:619-621`); `Shot` is RGB and `blend` takes full-size layers only (`shot.rs:17-25`, `104-121`); `forget` would delete cam pieces (`shot.rs:443-448`). → stage-4 plan: composite before `watermark::stamp`, add a positioned RGBA blend, keep cam pieces in `forget`.
- **Privacy behind the lock screen.** A recording keeps the pipeline alive on the secure desktop (`state.rs:588-602`). → handled in section 2.2 and Open item 8.

### Console and preview
- **Camera opened twice.** `getUserMedia` would make WebView2 a second camera client and put a prompt into an always-on-top window. → handled in section 5.3 (pull command instead).
- **Never throttled while hidden.** The browser flags switch backgrounding off (`overlay.rs:53-58`), so a timer-paced pump keeps running. → handled in section 5.3 (the page stops asking, core gate `CONSOLE_SHOWN`); the plan must also clear the gate on `WindowEvent::Destroyed` and fix CON-2.
- **Silent permission refusals.** A command missing from the capability is refused and swallowed by the console's `.catch(() => {})` (`Console.tsx:337`, `347`, `442`, `773`), as in CON-1. → handled in section 5.3 (granted to console and main window); the plan must add the "commands per window" check. A permission granted but missing from `build.rs` fails the build; a command missing from `generate_handler!` fails at runtime with "command not found".
- **Main-thread stalls.** A sync command or a `window_awake` call per frame would stall the main thread (`commands.rs:400-402`, `lib.rs:975-983`). → handled in section 5.3 (async command); the gate must be an atomic, never `window_awake`.
- **Re-renders on a CPU renderer.** Frame state in React would multiply CON-5 on a `--disable-gpu` page (`overlay.rs:57`). → handled in section 5.3 (canvas, 320 px, ≤ 15 fps); the plan must draw through a ref and should fix CON-5.
- **Events are broadcast.** `window.emit` reaches every webview, and `emit_to` reaches pages that listen for any target (CON-4). → handled in section 5.3 (no frames as events); any "console only" facecam event needs `emit_to` with its own event name.
- **Relayout resets the console.** `relayout` runs `begin()`, which resets panels and the player (CON-6). → handled in section 5.3 (Cam off triggers no relayout); the plan should add `console-relayout` before adding facecam fields to the relayout diff (`commands.rs:149-161`).
- **Cam off on the main thread.** The console's existing source command is sync and ends in `replace_config` with a disk write (`commands.rs:340-343`, `state.rs:419-458`). → the stage-2 plan must make `set_facecam_enabled` async and leave the open and close to the camera thread (section 5.1).
- **CSP.** `img-src` allows `asset:`, `http://asset.localhost`, `data:` and the Discord CDN, `media-src` only `asset:` and `http://asset.localhost`; neither allows `blob:` (`tauri.conf.json:31`). → the stage-2 plan must draw with `createImageBitmap` into a canvas (no `blob:` URL) or widen the CSP.
- **Banner over the preview.** The default banner corner is bottom right (`model.rs:271`), the default cam corner too ("What the research changed", item 6), and the banner is raised above the console (`overlay.rs:455-465`). → not in the stage-2 plan (Task 21 does not check it). Open: worth adding to stage 2 — check that a banner does not cover the in-place preview.
- **Visible in screen recordings; exclusive fullscreen.** With `console_in_capture` the preview would land in clips (`console.rs:255-267`); over exclusive fullscreen the console does not open (`console.rs:164-173`). → handled in sections 5.3 and 5.2 (settings-tab preview).

### Audio
- **The cam's timestamps.** `usable_stamp` (`audio/capture.rs:67-78`) falls back to arrival time for good after one stamp more than 2 s off, which for a cam pins the whole session late by exposure, USB and decode time; `MediaFrameReference.SystemRelativeTime` can be null. → handled in section 2.2 (plausibility check, flagged fallback); the plan must never anchor on a 0, null or flagged stamp (skip to the next frame) instead of copying the permanent fallback.
- **CPU contention with the audio threads.** MJPEG decode (10–25 % of a core, section 2.9) competes with normal-priority capture threads and the mixer, none of them on MMCSS (`audio/capture.rs:118-123`, `613-619`; `pipeline.rs:742-745`): stalls over ~60 ms trigger AUD-2, over ~80 ms the mixer reads silence that arrives later (`pipeline.rs:36-42`). → the stage-2 plan must keep the decode off those threads, consider MMCSS "Pro Audio" for the audio threads, and measure in a game.
- **A timed-out open keeps the device on.** AUD-4's leak, copied to the camera, keeps the webcam LED on after "Cam off". → the stage-2 plan must set the stop flag on every open timeout.
- **COM apartment.** Media Foundation or WinRT on the main thread breaks Tauri (`state.rs:355-360`, `audio/engine.rs:158-163`). → handled in section 5.1 (own thread); the plan must not open the cam in `AppState::new`, where `audio.apply` runs today (AUD-V3).
- **Delay default and offset.** Mic stamps are capture time (`audio/capture.rs:644-654`), and a cam on arrival time lags the voice; a stream-copied cam starts at the keyframe before the clip, so its offset is not 0. → handled in sections 2.5 (−40 ms, −80 ms on the fallback, clap test) and 2.4/2.7 (`cam_offset_us` stored).

### Settings and permissions
- **"Cam off" undone by the main window.** CFG-1 applied to the master switch. → handled in section 5.1 (revision number, `config-changed` to the main window only); the plan should also let `set_config` always keep `previous.facecam.enabled`, so only the dedicated command switches it.
- **Default polarity.** A field named after the label "Cam off" with serde's default `false` would switch the camera on for every existing config. → handled in section 5.1 (`enabled`, default false).
- **Config loss through serde.** A missing container default or an unknown enum value makes `load()` discard the whole config (`config.rs:167-170`), and CFG-2/CFG-V1 then save defaults over it. → handled in sections 5.1 and 3.1 (container default, unknown shapes load as `Rect`); the plan must also let a wrong-typed `facecam` field fall back on its own (a lenient `deserialize_with`) and test it.
- **Where the clamp runs.** The console's switch and other paths reach only `replace_config`, not `set_config`. → handled in section 5.1: `clean_facecam` runs in `replace_config` (next to `encode::resolve`, `state.rs:420`; stage 2 Task 6), so every write path is clamped, and a config loaded from disk needs it too; it must never enumerate camera formats there (main thread, `state.rs:355-360`); snapping to the device's formats belongs to the camera thread.
- **Which lock.** `lifecycle` (`state.rs:96`) is held while a stop waits for a save and during capture start-up. → the stage-2 plan must give the cam its own lock, since section 2.2 keeps its lifecycle outside the pipeline.
- **Floats into integers.** A float sent for an integer field makes serde reject `set_config`; the store keeps the bad optimistic value and every later write fails until a restart (`store.ts:372-387`). → handled in sections 3.1 and 5.1 (`f32` layout, `i32` delay in steps of 10); the plan should re-fetch the config after a `set_config` error.
- **Device identity.** A camera's symbolic link can change with the USB port (inference). → handled in section 5.1 (`device_id` + `device_name`); stage 2 finds a re-plugged camera by name when it opens it (Task 7 `pick_device`, Task 11) but writes nothing back, so a repair step is not in the stage-2 plan. Open: worth adding to stage 2 — a startup and 2 s repair step that stores the new id and emits `config-changed`, like `repair_screens` (`lib.rs:496-543`).
- **Registration.** Each new command needs `generate_handler!`, `build.rs`, the capabilities and the committed generated files (`permissions/autogenerated/*.toml`, `gen/schemas/*`). → the stage-2 plan lists all four (Tasks 7, 17, 19), but the test that they agree is not in it; `docs/extras/plans/2026-10-22-fix-data-safety-and-silent-failures.md` Task 6 (`grants.rs`) only checks that each window's calls are granted. Open: worth adding to stage 2 — a test that `generate_handler!`, `build.rs` and the capabilities name the same commands.
- **Store start and cleanup.** Camera enumeration in `init`'s `Promise.all` (`store.ts:264-273`) would hold back the whole app; `delete_clip` (`commands.rs:758-764`), `storage_usage` (`1276-1304`), `originals_still_needed` (`1328-1335`) and `StorageUsage` with its TS mirror (`model.rs:813-828`, `types.ts:461-470`) must know the cam files. → handled in sections 5.2, 2.7 and 3.6; the plan must name each site.

## Checked and fine

- Lock order on the frame path: capture callback mutex → `Latest.encoder` (released before the next is taken) → `Latest.converter` → D3D lock; the encoder thread takes `packets` → `recorder`. App locks: `lifecycle` → `pipeline` → `packets` → `recorder`, `status` → `shared`, `recorder` → `shared`, `status` → `recorder`, `tracks` → `recorder`. No inversion anywhere.
- Audio: no lock is held across a WASAPI call, the ring lock covers only queue operations, and `AudioEngine.running` is not held across `capture::start`.
- No `unwrap`, `expect` or `panic!` outside tests in `buffer.rs`, `pipeline.rs`, `state.rs`, `console.rs`, `overlay.rs` and `preview.rs`. The panic candidates in `capture.rs`, `gpu.rs` and `mft.rs` are guarded; the clamp in `edit::plan` cannot panic.
- COM ownership in `enumerate`, `take_output`, `feed` and the `ManuallyDrop` stream in `convert`: no leak. A timed-out `VideoEncoder::start` leaves no thread behind. Late capture frames during the encoder drain cannot corrupt it.
- `opens_a_gop` overriding `CleanPoint = 0` is the safer direction; only its comment is off.
- A second `VideoEncoder` needs no change inside `mft.rs`: thread, channel, stats, device manager and transform are per instance.
- Pts are exact per tick, with no accumulating rounding; the pacer's period truncation costs ~0.14 ms per hour at 60 fps.
- Mixer and `TrackRing` rounding cancel for continuously fed tracks (< 1 ms); the 1 s gap threshold matches the 1 s capture rings; silence accounting is sample-exact; trims always remove an even sample count; `head_seq`/`seq` arithmetic is consistent across trim, floor and clear.
- 441-frame packets at 44.1 kHz resample to exactly 480 frames.
- Two parallel saves never share a temp file (names are unique per clip id).
- The `muxer::encode`, thumbnail, stem extraction, export and ffprobe argument lists match what the code builds; stems stay untrimmed in original coordinates.
- The console's and banner's listeners do not leak; the `Focused(false)`/close race on `PREVIOUS` is handled; apart from `link_exists`, no window calls a command it is not granted.
- `generate_handler!`, `build.rs` and `default.json` hold the same 81 commands; the Rust and TS `AppConfig` have the same 33 fields.
- The control port runs one thread per connection, the hotkey rollback works, and `usable_clip_dir` rejects `C:`, `\foo` and `\\?\C:\`.
