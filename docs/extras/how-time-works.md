# How time works in ClippiBoy (extra)

A reference for building the facecam and for the review fixes that are about time. You did not ask for it, so take what is useful.
It covers the one clock, how the picture and the sound are put on it, how a clip and a recording are cut, how edits keep the stems in line, what goes wrong today, and where the cam's clock goes.
Method: read from the code at 401a320 (the working tree differs only in `docs/`). Every `file:line` here was opened and checked. Where a review report and its second reviewer disagreed, the second reviewer's reading is used, unless the code said otherwise. Nothing was built or run. Paths are under `src-tauri/src/` unless they say otherwise. Finding IDs are those in `docs/facecam-review.md`.

---

## 1. One clock

Everything in ClippiBoy that carries a time carries **QPC in 100 ns units**. There is no second clock to convert from, and no per-stream epoch.

| Who | What it stamps | Where |
|---|---|---|
| Windows.Graphics.Capture | Each game frame, with `Direct3D11CaptureFrame::SystemRelativeTime().Duration`. | `wgc.rs:247`. Carried as `CapturedFrame::qpc_100ns` (`wgc.rs:41-44`) into `Latest::submit` (`pipeline.rs:700-705`) and stored as `slot_qpc` (`convert.rs:716`). |
| WASAPI | Each audio packet, with `pu64QPCPosition` from `GetBuffer`. | `audio/capture.rs:644-654`. Checked by `usable_stamp` (`audio/capture.rs:67-78`, applied at `660-671`), then written into the stream's ring (`685`). |
| ClippiBoy | "Now": `QueryPerformanceCounter` scaled to 100 ns. Whole seconds and the remainder are scaled apart, so nothing overflows. | `now_100ns()`, `audio/capture.rs:23-44`. |
| The pacer | Nothing. It only keeps a schedule, with `std::time::Instant` (QPC on Windows, per the std docs). | `convert.rs:740-751` |
| The encoder | Nothing new. It carries the pacer's pts through: `SetSampleTime(pts)` in, `GetSampleTime()` out. | `mft.rs:970`, `mft.rs:1032`, `mft.rs:1053` |
| The camera (planned, no code yet) | MF `MFSampleExtension_DeviceTimestamp`, or WinRT `MediaFrameReference.SystemRelativeTime`. Both are documented as QPC in 100 ns; the WinRT one can be null. `IMFSample::GetSampleTime` is probably rebased to the start of streaming (unconfirmed). | Spec 2.2. The stage-1 probe's `--log-raw` step logs all of them (`docs/superpowers/plans/2026-10-22-facecam-stage-1-camera-sharing-test.md`). |

The units:

| Quantity | Unit | Where it lives |
|---|---|---|
| Absolute time | QPC, 100 ns | stamps, `base_100ns`, ring starts, `start_100ns` |
| Video pts | 100 ns into the encoder, µs in the ring (`pts_us = pts_100ns / 10`, `mft.rs:1053`) | `EncodedPacket::pts_us` (`buffer.rs:17-23`) |
| Audio | samples at 48 kHz, stereo. One sample = 1e7 / 48000 = 625/3 ticks. | the rings, the recorder |
| Editor | ms, of the current file or of the original | `ClipEdit`, `ClipOriginal` |

The 625/3 is why integer division keeps coming up below. Every sample-to-tick conversion throws away 0, 1/3 or 2/3 of a tick.

`README.md:205-206` says "the zero point for both tracks is the first frame that arrived". The code is simpler than that. Audio is never anchored to the video. Both sit on absolute QPC. The only translation is the video's: `base_100ns` says which QPC video pts 0 falls on (section 2).

---

## 2. The picture

WGC delivers a frame only when the picture changes. The pacer `pace` (`convert.rs:737-795`) turns that into a stream at a constant rate:

- **Schedule.** `period = 1e9 / fps` ns, truncated (`convert.rs:739`). Tick `index` is due at `start + period·index` (`convert.rs:748`). Because it sleeps against the absolute start, wake-up delays do not add up (`convert.rs:745-751`).
- **Pick-up.** Under the converter lock it reads the newest converted slot and its generation, and marks it as taken (`convert.rs:772-776`). `index += 1` happens here, on every tick.
- **Before the first frame** there is nothing to send (`convert.rs:778-781`). The index still counts, so the first pts is not 0.
- **pts by index.** `pts_100ns = (index − 1) · 1e7 / fps` (`convert.rs:790`). It is computed from the index each time, so no rounding builds up.
- **Still picture.** WGC sends nothing, the generation does not move, and the same slot goes out again, counted as `duplicated` (`convert.rs:782-786`). A frame let go because no slot was free (`convert.rs:694-702`) also ends up as a repeat.
- **Catch-up.** A late tick counts as an overrun. Up to 250 ms late (`CATCH_UP`, `convert.rs:734`), the missed ticks are caught up in a burst, with their normal pts. Later than that, they are skipped: `index += behind / period` (`convert.rs:762-767`). The pts then jump with real time and stay on QPC, but the stream has a hole in it.
- **Hand-over.** The lock is dropped and the frame goes to the sink (`convert.rs:792-793`).

**The anchor.** `EncoderSink::on_frame` (`pipeline.rs:617-637`) sets the time origin on the first frame it gets: `base_100ns = latest.frame_qpc() − pts` (`pipeline.rs:621-626`). From then on, `qpc_of(pts_us) = base_100ns + pts_us·10` (`pipeline.rs:557-559`) turns any packet's pts into QPC. A frame the encoder will not take is counted as `dropped` (`pipeline.rs:628-632`). `submit` gives up after waiting 500 ms (`mft.rs:134`, `1163-1201`).

```
QPC time ─────────────────────────────────────────────────────────────────────────────────►
WGC frames      f1@q1      f2@q2                                     f3@q3
pacer ticks   t1   t2   t3   t4   t5   t6   t7 ┄┄┄┄┄ stall > 250 ms ┄┄┄┄┄ t24   t25
sent          –    f1   f1   f2   f2   f2   f2      (t8 … t23 skipped)    f3    f3
new / repeat       new  rep  new  rep  rep  rep                           new   rep
pts / period       1    2    3    4    5    6                             23    24
                   ▲                                                      ▲
                   base_100ns = q1 − 1·period                             pts jump: a hole of 16 ticks
(at 60 fps; 16 ticks ≈ 267 ms)
```

**What a pts means.** It is the tick's place on the output timeline, not the capture time of the picture in it. The picture sent on a tick is whatever was newest at that moment, so it is up to one capture interval older than its pts. That is jitter of under a frame, not drift.

**The anchor's own error.** `base_100ns` ties the tick timeline to the capture time of the first frame sent. That frame was already some age when its tick came. The first sent tick usually follows one that found no frame yet (`convert.rs:778-781`), so the age lies between 0 and about min(capture interval, pacer period). It becomes a constant offset for the whole session, different on every start, about ± half a frame around the mean. At 60 fps that is a few ms. CAP-4 adds a rare race on top (section 6).

**What resets it.** `Shared`, with `base_100ns` and the packet ring, is made new on every `Pipeline::start` (`pipeline.rs:934-942`). The ring is cleared on stop (`pipeline.rs:994`), and the audio engine's stream rings are cleared at start (`pipeline.rs:930-932`). So each buffer start is a new origin, and nothing in the game path carries time across a restart.

**The ring.** `ReplayBuffer::push` keeps `newest_pts` and records `(seq, pts)` for every video keyframe (`buffer.rs:75-84`). `trim` drops whole GOPs from the front while the *next* keyframe is at least the buffer length old, or while the ring is over its byte budget. It never drops the last GOP (`buffer.rs:92-111`). So in steady state the front keyframe is between one buffer length and one buffer length plus one GOP old.

---

## 3. The sound

Sound passes through two rings before it reaches a clip.

```
WASAPI packet ──stamp──► SampleRing        one per stream, 1 s           audio/ring.rs
   (QPC)                     │  read by time
                             ▼
                        mixer               every 10 ms, 80 ms behind     pipeline.rs:781-870
                             │  one block per source per window
                             ▼
                        TrackRing          one per source, buffer + 2 s   pipeline.rs:52-231
                             │  window(start_100ns, audio_frames), on save
                             ▼
                        WAV ──► ffmpeg
```

### The stream ring (`SampleRing`, `audio/ring.rs`)

There is one per WASAPI stream: 48 kHz stereo after conversion, 1 s long (`audio/ring.rs:44`, `82`). The first block after a clear sets `start = stamp` (`audio/ring.rs:136-138`). After that, every block is compared with where it ought to sit:

```
expected = start + held_frames / 48 kHz                                 audio/ring.rs:141-143
drift    = stamp − expected

|drift| > 1 s    → drop everything, restart the timeline at the stamp     145-150
drift  > +50 ms  → append drift's worth of zeros, then the block          151-155
otherwise        → start += clamp(drift, ±slew)                           156-163
                   slew = block length × 2000 ppm, at least 1 tick        113-115
                   = 200 ticks for a 10 ms block, just under one sample
```

That pull is the README's "at most one frame per block" (`README.md:227-237`). It follows a crystal that is off by tens of ppm, and it averages out the jitter of single stamps. Note that it is **asymmetric**. A block 50 ms too late gets zeros at once; a block 50 ms too early is only pulled at 2 ms per second (AUD-2). Trimming the front moves `start` on by the frames dropped (`audio/ring.rs:168-173`). `read_window(from)` reads by time, takes nothing out, and returns zeros outside the ring (`audio/ring.rs:183-211`).

The stamp itself goes through `usable_stamp` (`audio/capture.rs:67-78`):
- A 0 means "not filled in", and is replaced by `now`.
- A stamp more than 2 s away from `now` (`audio/capture.rs:52`) drops trust for the rest of that attempt. From then on every packet carries the read-out time (`fallback_clock`, `audio/capture.rs:660-671`).
- A packet flagged SILENT becomes zeros of the same length (`audio/capture.rs:676-678`). The two other flags, timestamp error and discontinuity, are not looked at.

### The mixer (`mix_loop`, `pipeline.rs:781-870`)

This is the audio clock.
- It wakes every 10 ms (`pipeline.rs:45`, `792`) and mixes up to `target = now − 80 ms` (`pipeline.rs:42`, `801`). WASAPI hands a block over only once it is full. Mixing closer to now would read silence where real audio lands a moment later (`pipeline.rs:36-41`, `README.md:222-225`).
- The windows have no gaps between them. `from = next_100ns`, and `frames = (target − from)·48000 / 1e7` (`pipeline.rs:802-806`). After the window, `next_100ns = from + frames·1e7 / 48000` (`pipeline.rs:868`). Both divisions are integer.
- It reads every stream's ring at `from` (`audio/engine.rs:392`), sums each source's streams with its gain, and clamps (`audio/engine.rs:337-375`).
- Every track ring gets a block in every window: silence for a muted source, so nothing slides forward (`pipeline.rs:837-858`).
- A window longer than 1 s (`pipeline.rs:778`, `816`) means the thread did not run, because the PC slept or something stalled it. It is not mixed. Each track ring counts it as silence through `skip` (`pipeline.rs:816-833`).

### The track ring (`TrackRing`, `pipeline.rs:52-231`)

There is one per enabled source, `(buffer seconds + 2) × 48000 × 2` samples long (`pipeline.rs:48-50`).
- The first block that is not silent sets `start_100ns = from` (`pipeline.rs:152-154`). After that the ring trusts only the sample count: blocks are appended, and their `from` is ignored (`pipeline.rs:109-110`).
- Silent blocks are counted, not stored (`pipeline.rs:125-143`). When sound comes back, the count turns into real zeros first, so the new block lands where it belongs (`pipeline.rs:150-157`). Silence as long as the ring empties it, and it is primed again later (`pipeline.rs:136-141`).
- Trimming the front moves `start_100ns` on by `frames·1e7 / 48000` (`pipeline.rs:160-166`).
- `window(from, frames)` reads by time, with zeros before the start and after the end (`pipeline.rs:194-220`).

The two truncations, the mixer's at `pipeline.rs:868` and the trim's at `pipeline.rs:165`, cancel for a track that never goes silent: a full ring trims as many frames as each window adds, with the same rounding, so the error stays under a tick. They do not cancel in recordings, and not across counted silence (AUD-6, AUD-V1; section 6).

### Gaps

| What happens | Result | Where |
|---|---|---|
| A device is lost | The stream is rebuilt, with back-off from 300 ms to 5 s, into the same stream ring. | `audio/capture.rs:475-476`, `505-528` |
| It comes back within 1 s | The gap is filled with zeros. | `audio/ring.rs:151-155` |
| It comes back after more than 1 s | The stream's timeline starts over. | `audio/ring.rs:145-150` |
| A source has no stream (the game is closed) | `mix_window` hands back zeros; the track ring counts silence. | `audio/engine.rs:379-395`, `pipeline.rs:125-143` |
| The mixer did not run for more than 1 s | Counted as silence. | `pipeline.rs:816-833` |
| Silence as long as the track ring | The ring is emptied and starts over. | `pipeline.rs:136-141`, `181-186` |

---

## 4. Cutting a clip

`AppState::save_clip` (`state.rs:863-939`) takes the snapshot under the `AppState.pipeline` lock (`state.rs:883-892`). `Pipeline::snapshot(seconds)` (`pipeline.rs:1033-1077`) then does this:

1. **Keyframe cut** (`buffer.rs:181-200`). `want_from = newest_pts − seconds`. The start is the newest keyframe at or before `want_from`, or the front keyframe if there is none. Every packet from there on is copied (cheap `Arc` clones, `pipeline.rs:1034`).
2. `first_pts` is the keyframe, and `last_pts` is the highest pts (`pipeline.rs:1039-1042`).
3. **Span.** `span_of(first, last, fps) = last − first + period`, with the period in whole µs (`muxer.rs:268-271`).
4. **Timing** picks the `-r` and the length (`muxer::timing`, `muxer.rs:251-264`). See below.
5. **Audio window.** `start_100ns = qpc_of(first_pts)` (`pipeline.rs:1072`), and `audio_frames = span_us · 48000 / 1e6` (`pipeline.rs:1054`).
6. The track rings are **not** copied, only their `Arc`s (`pipeline.rs:1069`). They are read later.

`muxer::build` (`muxer.rs:114-226`) then:
- writes the `.h264` (SPS/PPS, then every packet; `muxer.rs:139-153`),
- cuts every WAV at `(start_100ns, audio_frames)` from the *live* rings (`muxer.rs:178-209`),
- runs ffmpeg once.

Picture and sound are aligned in Rust. ffmpeg sees both inputs start at 0, and gets no `-itsoffset` and no `-ss`.

### `muxer::timing`: the three cases

Take `N` frames, and `nominal = N · 1e6 / fps` µs (integer division).

| Case | Condition (`muxer.rs:255`) | `-r` | Length for picture and audio |
|---|---|---|---|
| A. nothing missing | `span < nominal` | `fps` | `nominal` |
| B. some ticks missing | `nominal ≤ span ≤ nominal·3/2` | `N·1e6/span` (a fraction) | `span` |
| C. "a gap" | `span > nominal·3/2`, or no span | `fps` | `nominal` |

The code has two match arms; A and C share the second one. Two exact points:
- `span_of` truncates the period to whole µs, and the pts are truncated too. So a run with nothing missing comes out within about a µs of `nominal`, on either side, depending on where its first pts falls (worked through by hand for 30 and 60 fps). It lands in A or B, and the rate comes out the same. With 50 fps, where the period is exact, it lands in B.
- The B/C border is an achieved rate of **2/3 of fps**, averaged over the whole clip. Below that, the clip is muxed as if nothing were missing (SAVE-4).

In B, the frames are spread evenly over the span. Both ends meet the audio. Anything missing in the middle is smeared over the whole clip (BUF-6/AUD-5).

**`-r` is an input option.** It is `-f h264 -r <rate> -i <video>` (`muxer.rs:316-317`). The elementary stream has no timestamps, so ffmpeg's raw H.264 reader gives frame n the time n / rate. The video is then copied (`-c:v copy`, `muxer.rs:350`). This also assumes frames arrive in display order. B-frames are left at the driver's default unless `CLIPPIBOY_BFRAMES` is set (`mft.rs:383-401`).

**The zero point in the file.** `-avoid_negative_ts make_zero` is set on the clip output only (`muxer.rs:356`). If SAVE-7 holds, the content at `start_100ns` sits at about +21.3 ms in the written clip, and at 0 in the stems. So never take the clip's t = 0 for `start_100ns`.

### Game ring, audio ring, clip window

A full-length clip, the default (`clip_seconds = 0`, `config.rs:41-42`, `87-92`):

```
QPC time ───────────────────────────────────────────────────────────────────────────────►  now
audio ring    A0 ════════════════════════════════════════════════════════════════╗ A_end
              │◄────────────────────────────── S + 2 s ─────────────────────────►│ ≈ now − 80 ms
game ring            K0 ━━━━━━━━━━━━━━━━━━━━━━━━ K1 ━━━━━━━━ … ━━━━━━━━ Kn ━━━━━━━━━━━━━ newest pts
                     │◄───── x ─────►┆                                                │ ≈ now − enc. latency
                                     ┆ want_from = newest − S                         │
                                     ┆◄──────────────────────── S ───────────────────►│
clip window          [K0 ═════════════════════════════════════════════════════════════════ last + period)
                     └ start_100ns = qpc_of(K0)               audio_frames = span × 48 kHz

S = buffer length.  x = how far K0 lies before want_from, 0 ≤ x < one GOP.
Slack at the snapshot = K0 − A0 = 2 s − x + (80 ms − encoder latency).
```

While `muxer::build` writes the `.h264`, the mixer keeps pushing and trimming, so `A0` moves right by the write time. Whatever of the clip lies left of `A0` when the WAV is read comes back as zeros. At the other end, `A_end` lies about 80 ms before now, and the clip runs to `last + period`. So the mixer has to catch up during the write, or the clip ends in zeros. Both are BUF-1/AUD-1.

---

## 5. Recordings and edits

### Recordings

A recording has no clock of its own. It taps the buffer's encoder and mixer (`recorder.rs:1-6`).
- **Start.** Under the packet lock it takes the backlog from the newest keyframe (`buffer.rs:212-222`), and sets `start_100ns = qpc_of(first.pts_us)` (`pipeline.rs:1004-1016`, `1012`).
- **Picture.** Every encoder packet also goes to the recorder (`pipeline.rs:667-671`). `Recorder::video` counts the frames and keeps the first and last pts (`recorder.rs:223-233`).
- **Sound.** The first mixer window at or after `start_100ns` attaches the recorder. The stretch `[start_100ns, from)` is read once out of the track rings, and the recorder's `timeline` is set to that many frames (`recorder.rs:269-289`). From then on every block is appended by count (`recorder.rs:292-308`). A source that joins late first gets the silence it missed (`recorder.rs:302-305`). `end_window` moves the timeline on (`recorder.rs:311-316`), and it does so in the mixer's gap branch too (`pipeline.rs:827-829`).
- **Stop.** `span_us = span_of(first, last, fps)` (`recorder.rs:366-372`). `finalize` runs the same `muxer::timing` (`recorder.rs:566`). It cuts or pads every raw PCM track to `span × 48 kHz` (`recorder.rs:567`, `577`; `align` at `459-472`), and muxes with `-r` (`recorder.rs:616-624`). On success, the raw folder is deleted (`recorder.rs:625`).

So in a recording, the picture's length comes from QPC (through the pts), while the sound's position comes from counting samples. Anything that makes the two disagree in the middle, such as rounding or a gap, is baked in at stop and cannot be redone (AUD-6, REC-1).

```
QPC time ─────────────────────────────────────────────────────────────────────────►
video      Kr ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ last
           └ start_100ns                  span_us = last − first + period
audio      [ backlog, read once ][block][block][block] …   appended by count
           start_100ns           from₁
finalize   every track cut or padded to span_us × 48 kHz; muxed at muxer::timing's -r
```

### Edits

Two timelines meet in the editor:
- **Original coordinates:** the untouched recording. The stems live here. They stay untrimmed and are never replaced (`edit.rs:17-21`, `README.md:361-364`).
- **Current-file coordinates:** the clip as it lies in the folder now. The player's handles and the trim live here.

`ClipOriginal { duration_ms, start_ms, end_ms }` links the two (`model.rs:756-768`). `start_ms` is where the current file starts inside the original, and so it is the stems offset. It lives in the database and outlives the file.

`edit::plan` (`edit.rs:289-337`) does the arithmetic in original coordinates:
- `offset = base.start_ms`, or 0 if the clip was never trimmed (`edit.rs:295-298`).
- A front cut only counts above 250 ms (`edit.rs:43`, `303`).
- `absolute_start = offset + (front cut ? trim.start : 0)` (`edit.rs:306`); the end likewise (`307`).
- The video source (`edit.rs:318-322`):
  - no front cut: copy the clip, from 0;
  - a front cut, and an archived original: re-encode from the original at `absolute_start`;
  - a front cut, and no archive: the clip, at the *relative* `trim.start`.
- `stems_start_ms = absolute_start`, always (`edit.rs:328`).

`edit::arguments` puts a `-ss` before **each** input's `-i` (`edit.rs:663-686`):
- The video gets one only when `video_start_ms > 0` (`edit.rs:670`).
- Each stem gets `stems_start_ms` (`edit.rs:679-682`).
- Audio that is only inside the file shares that file's timeline, and so inherits the video's `-ss` (invariant 2, `edit.rs:22-24`).
- The output gets `make_zero` again (`edit.rs:729`).

The preview does the same in the browser. Each stem is held at `video.currentTime + original.startMs / 1000`, and seeked again when it is more than 120 ms off (`src/lib/useClipMix.ts:141`, `318-320`).

```
original (the stems live here)
0 ──────────────────────────────────────────────────────────────────────── duration_ms
               start_ms ┃═══════════ current clip file ═══════════┃ end_ms
                        0 ──────────────────────────────────── clip duration   (handles)

trim [a, b] on the current file, with a front cut:
   video   -ss (start_ms + a)  -i originals/<id>/video.mp4      re-encoded
   stems   -ss (start_ms + a)  -i tracks/<id>/<n>.m4a           each one
   then    ClipOriginal.start_ms = start_ms + a
mix only, or a back cut:
   video   -i <clip>                                            copied from 0
   stems   -ss start_ms  -i tracks/<id>/<n>.m4a
```

---

## 6. Where it goes wrong today

These are the confirmed or plausible review findings that are about time. The details are in `docs/facecam-review.md`. IDs with a V (AUD-V1) are the second reviewer's own finds.

**BUF-1 / AUD-1: a full-length clip can start mute.** Medium, confirmed; high with a 4 s GOP. The track rings hold only `seconds + 2 s` (`pipeline.rs:48-50`). The keyframe cut reaches up to one GOP further back (`buffer.rs:181-189`). The rings are read live only after the whole `.h264` is written (`pipeline.rs:1069`, `muxer.rs:139-209`). So the missing start is max(0, write time + encoder latency + x − (2 s + mixer lag)): about half of all full saves at a 4 s GOP lose up to ~2 s, and at 2 s the loss is occasional and short. **Fix:** size the rings `seconds + keyframe_seconds + a margin` (needed in any case). Then, outside the pipeline lock, wait (bounded) until the mixer has passed `start + span`, and copy the windows.

**BUF-5: after a wake without a restart, clips come out silent or stretched.** Plausible: the code path is certain, and it needs QPC to keep running through sleep, which the code's own comments say it does. The pts jump by the sleep (`convert.rs:762-767`). The last keyframe before the sleep stays in the ring until the first one after it is a buffer length old (`buffer.rs:93-99`), and a save in that time cuts from it. The span is then hours, case C applies, and the audio window lies before the wake, in rings that `skip` has emptied (`pipeline.rs:816-833`, `175-187`). A short sleep stretches the clip in case B instead. **Fix:** in `ReplayBuffer::push`, treat a forward pts jump larger than max(1 s, a few GOPs) as a discontinuity, and drop everything before the first keyframe after it. `snapshot` must never start before it.

**BUF-6 / AUD-5: a stall in the middle of a clip shifts picture against sound.** Medium, confirmed. A stall over 250 ms becomes a pts hole (`convert.rs:762-767`), and a frame `submit` gives up on leaves a one-frame hole (`mft.rs:1184-1199`). Case B then spreads all frames evenly over the span. So around the hole the picture is off by up to half the hole: 0.5 s for a 1 s stall in the middle. **Fix:** give ffmpeg real timestamps (a timestamped container written from Rust, or one input per run with its own offset). Do not raise `CATCH_UP` or replay missed ticks instead: the comment at `convert.rs:754-761` says why the jump is there (under a sustained shortfall, replayed ticks fall further and further behind the clock until the audio cut lies outside the ring). Pair real timestamps with BUF-5's cut for real gaps; `docs/extras/plans/2026-10-22-fix-sync-and-timestamps.md` Task 7 plans it.

**AUD-2: one late stamp makes a source play late for up to a minute and a half.** Medium. The mechanism is confirmed; the trigger depends on drivers. On the fallback clock, or with a 0 stamp, the first packet read after a stall of the capture thread carries the read-out time, which is late (`audio/capture.rs:67-78`, `660-663`). A drift over +50 ms is filled with zeros at once. The packets queued behind it then sit that much late in the ring, and that negative drift is only pulled back at 2 ms per second (`audio/ring.rs:151-163`). TIMESTAMP_ERROR (0x4) is ignored (`audio/capture.rs:676`). With the 200 ms WASAPI buffer (`audio/capture.rs:371`) the worst case is ~190 ms late, taking ~95 s to recover. **Fix:** pass "no stamp" for 0 and for flagged stamps, and append those by count. On the fallback clock, estimate the capture time as `now − queued frames / rate`.

**AUD-6 / AUD-V1: rounding makes audio drift late, in recordings and across counted silence.** Low, confirmed. `next = from + ⌊frames·625/3⌋` (`pipeline.rs:868`) loses about 1/3 tick per window, ~8–12 ms per hour. Recordings append by count (`recorder.rs:306`) and are cut to the QPC-exact span (`recorder.rs:566-577`), so their sound runs late: one sample is read twice every ~6–10 s. Buffer clips cancel this through the trim at `pipeline.rs:165`, but not for tracks with counted silence (`pipeline.rs:125-157`): a chat tap or a gated mic ends up ~25 ms late over a 3 h session. **Fix:** compute `next = origin + ⌊total_frames·625/3⌋` from running totals, including the gap path at `pipeline.rs:831`. Do not change only the trim at `pipeline.rs:165`; that would make buffer clips drift instead.

**REC-1: sleep during a recording ruins its sync for good.** Medium-high, confirmed. The pts jump by the sleep. The mixer's gap branch moves the recorder's timeline on (`pipeline.rs:827-829`), and the next block writes the whole gap as zeros (`recorder.rs:302-304`), while `span_us` includes it (`recorder.rs:366-372`). A gap under half the recording stretches the whole file (60 min + 10 min asleep plays at ~51 fps, minutes off around the sleep). A longer one goes to case C, which cuts the audio so that everything after the wake is silent. The raw folder is then deleted (`recorder.rs:625`). **Fix:** record the video-side gaps in `Meta`, subtract them from the span, and skip the matching PCM ranges in `finalize`. Keep the mixer's timeline advance, or a stall of the mixer alone would shift the audio for the rest of the file.

**SAVE-4: a sustained encoder shortfall is taken for a sleep.** Medium impact, low likelihood; confirmed. Case B only stretches while the achieved rate is at least 2/3 of fps (`muxer.rs:255`). At 36 of 60 fps for 30 s, the clip is muxed at 60 fps: it lasts 18 s, plays 1.67× fast, and its audio window covers only the first 18 s (`pipeline.rs:1054`). **Fix:** tell a gap from a shortfall by the largest single pts gap, not by the ratio. Best done together with BUF-5 and BUF-6, so one change covers all three.

**SAVE-7: the stems may sit one AAC frame (21.3 ms) off the picture, and mix-only edits add another each time.** Plausible, likely; it rests on ffmpeg behaviour that was read in its source, not run. `make_zero` on the clip output alone (`muxer.rs:356`) shifts the clip by the AAC priming (−1024 samples), so its video starts at +21.33 ms. The stem outputs (`muxer.rs:387-393`) keep an edit list and start at 0. An edit that copies the clip's video and mixes from the stems (`edit.rs:724-729`) moves the video another 21.33 ms each time. **Check:** `ffprobe -show_entries stream=codec_type,start_time` on a multi-source clip, on its `tracks/<id>/0.m4a`, and on the clip after two mix-only saves. **Fix:** one zero-point policy for every output (drop `make_zero` at `muxer.rs:356` and `edit.rs:729`, or set it on every output), with an argument-list test.

**CAP-4: the A/V anchor reads the capture QPC outside the lock that chose the frame.** Very low today, confirmed; the same as AUD-8. `pace` drops the converter lock (`convert.rs:792`) before `EncoderSink` reads `frame_qpc()` (`pipeline.rs:622`). A WGC frame converted in between makes `base_100ns` the QPC of a frame that was not sent: a constant offset of one capture interval for the session. **Fix:** read `slot_qpc` under the lock, next to `slot`, and pass it through `FrameSink::on_frame`. Do it before the pattern is copied for anything else.

### How they hang together

| Root cause | Findings |
|---|---|
| A raw elementary stream with one `-r` per stream: no room for a hole | BUF-6/AUD-5, SAVE-4, BUF-5, REC-1 |
| pts that jump on catch-up or sleep | BUF-5, REC-1, and the holes behind BUF-6 and SAVE-4 |
| Audio read late, from live rings with less slack than a GOP | BUF-1/AUD-1 |
| Integer rounding per mixer window | AUD-6, AUD-V1 |
| Drift handling that is quick one way and slow the other | AUD-2 |
| The anchor read outside its lock | CAP-4 |
| A zero point set per ffmpeg output | SAVE-7 |

The first two rows are one design question: whether a clip carries real timestamps, or one rate plus a rule for gaps. Settling it once (a discontinuity cut in the ring, plus per-run timestamps or offsets at mux time) fixes four findings, and it is the question the facecam's composite asks again.

---

## 7. Where the facecam slots in

This summarises spec sections 2.3–2.5 in the terms used above, then lists what the cam inherits from section 6.

**Its own origin, on the same clock.** The cam's frames carry `DeviceTimestamp` (MF) or `SystemRelativeTime` (WinRT). They are checked against `now_100ns()` the way `usable_stamp` checks audio, and the arrival time is a flagged fallback (spec 2.2). The cam has its own `origin_100ns`, which plays the part of `base_100ns`, and its own pts. It is held by `AppState` next to the pipeline, so a game restart, with its new `base_100ns`, does not touch it. The two only ever meet on absolute QPC.

**A grid instead of `pace`** (spec 2.3). At 30 fps, `slot = round((qpc − origin) / period)`. Several frames in one slot keep the newest. An empty slot repeats the last frame. A slot is sent once a later frame has arrived, or 100 ms after its time has passed. `pts = slot × period`.

```
cam stamps     c1      c2      c3               c4                  (frames dropping out, as in a dim room)
30 fps grid    │ s0    │ s1    │ s2    │ s3    │ s4    │ s5    │ s6    │ …
sent           c1      c2      c3      c3      c4      c4      c4
new / repeat   new     new     new     rep     new     rep     rep
pts / period   0       1       2       3       4       5       6
origin_100ns = qpc(c1)
```

This differs from the game in two ways that matter. First, each cam pts comes from that frame's own stamp, so there is no "age of the first frame" in the anchor (section 2) and no CAP-4 race. Second, the grid never skips, so the cam stream has no holes for `-r` to smear (BUF-6).

**Dropouts and sleep** (spec 2.3). A gap under 2 s repeats the last frame. A longer one sends black filler frames, so the stream stays continuous, and the lost QPC range goes into a list next to the ring. Sleep is told by comparing QPC with `QueryUnbiasedInterruptTime`, which stops while the PC sleeps — not by a gap alone, which a camera open or an encoder start on a busy machine can also cause. A sleep clears the cam ring and starts a new origin and a new encoder session; a clip that reaches back across it gets the cam from the wake-up on. The game ring drops what lies before a sleep too (BUF-5's fix).

**The ring** (spec 2.3). A second `ReplayBuffer` with its own lock, never held together with `Shared.packets`. Its length is buffer + game keyframe interval + 1 s cam GOP + 0.3 s delay range + 10 s save margin, because the cam is cut only after the clip is written: the slack the audio rings lack (BUF-1). Memory is `1.5 × 3 Mbit/s × length`, with a 16 MB floor, about 70 MB at 120 s. The encoder's packets come out as track 0 (`mft.rs:1052`). That is fine in their own ring, but they must never reach the game ring or `Recorder::video` (`recorder.rs:223-226`). The BUF-4 and BUF-5 fixes apply here too.

**The cut** (spec 2.4). It is by absolute time, in the same save:

```
QPC time ──────────────────────────────────────────────────────────────────────────────►
game clip                        G0 ══════════════════════════════════════════ G0 + L
                                 └ qpc_of(first_pts)                     L = span
cam cut          Kc ┄┄┄┄┄┄┄ G0 − 300 ms ════════════════════════════════════════════ G0 + L + 300 ms
                 ▲ last cam keyframe at or before G0 − 300 ms   (cam GOP 1 s)

cam_offset_us = (cam_origin + first_cam_pts·10 − G0) / 10      normally −0.3 s … −1.3 s
a cam frame stamped q is shown at clip time (q − G0) + delay
```

- `G0 = qpc_of(first_pts)` and `L = span`, as today (`pipeline.rs:1033-1077`).
- The cam packets run from the last cam keyframe at or before `G0 − 300 ms` up to `G0 + L + 300 ms`. That needs a new `ReplayBuffer::snapshot_range(from_us, to_us)`. "The last N seconds" would be wrong: it is relative to the cam ring's own newest pts (`buffer.rs:181-182`).
- The cut happens after `muxer::build` has written the clip (spec 2.4), so the cam ring has normally passed `G0 + L + 300 ms` already; only after a very quick write does it wait, at most 500 ms and outside every lock. Whatever is still missing at the end is the last frame held (`eof_action=repeat`). The cam ring keeps a 10 s save margin so `G0 − 300 ms` is still in it.
- Gaps that overlap `[G0, G0 + L)` are stored in clip coordinates. With no cam frame in the window at all, the clip is saved exactly as today.

**Sync and the delay** (spec 2.5). Picture, sound and cam share QPC, so sync is the offset above and nothing else. What is left is the camera's own latency: the stamp is taken when the frame reaches the host, an estimated 40–90 ms after the light hit the sensor. The delay `delay_ms` runs from −300 to +300 ms in 10 ms steps; negative shows the cam earlier. The default is −40 ms (−80 ms on the arrival-time fallback) until the clap test measures it. It is applied when compositing, never baked into the ring, so each clip keeps its own value and can be corrected later.

### What the cam inherits from section 6

These follow from the code and the spec together. They are my reading, not something the spec says.

- **BUF-1/AUD-1.** The cam ring has its slack; the voice does not. A wait for the cam *before* `muxer::build` would add to the write time in BUF-1's formula (at a 2 s GOP, 0.5 s of a 2 s window: about 25 percentage points more full saves would lose their start). That is why the spec cuts the cam only after the clip is written (2.4). Fixing the audio ring size stays necessary either way.
- **BUF-6/AUD-5 and SAVE-4.** The game picture is still spread by one `-r`, while the cam and the voice are placed by QPC. After a mid-clip stall, face and voice stay in sync with each other, and both are off against the game. Fix this before compositing, or give the composite per-frame timestamps for the game too.
- **The game's anchor error and CAP-4.** These now show between the game picture and the cam and voice: a few ms at 60 fps, below what the delay slider resolves.
- **SAVE-7.** Phase 2 composites from the clip file and `cam.mp4` (spec 2.6). If SAVE-7 holds, the clip's video starts at +21.33 ms and `cam.mp4` at 0. So place the cam against the clip's video `start_time`, not against t = 0, or fix SAVE-7 first. Every output of the composite run must follow one zero-point policy, or `facecam.json` must record each file's measured `start_time`.
- **Encoder drops.** With the grid, the only holes left in the cam stream are frames the cam encoder's `submit` gives up on (`mft.rs:1184-1199`). The stream the grid hands the encoder is exact CFR, and `cam.mp4` is muxed at `-r 30` (spec 2.6), which cannot carry such a hole. So the holes are recorded, and a cut never spans one: it starts after the newest hole inside the window, and the cam then starts a little later in that clip (spec 2.3). Never mux across a cleared ring as one input.
- **BUF-5 not yet fixed.** Until it is, a game cut can still reach back across a sleep. The cam then simply starts late, with a positive offset, as the spec says.
- **Recordings (stage 4).** AUD-6 (~10 ms per hour) and REC-1 were hard to notice next to a game. With a talking face they are visible. Fix both before cam recordings.
