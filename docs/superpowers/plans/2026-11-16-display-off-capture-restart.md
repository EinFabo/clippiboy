# Display off/on restarts the capture

## Problem

On 2026-11-15 the display went off on the idle timeout at 11:50 local
(Kernel-Power 566, Reason=12) and came back at 14:06 (Reason=36). No lock, no
sleep. Windows.Graphics.Capture never delivered another frame after that and
never raised `Closed`; `convert.rs` repeated the last (black) frame, and every
clip until the pipeline restart at 22:02 had a black game area while cam and
watermark were live.

The lock fix (item 151) covers a locked session, the secure desktop and a
system sleep (`AutoBuffer::poll_lock`). A display that only powers off and on
again is none of these. A generic "no new frame for N s" watchdog stays
rejected: a static captured screen is normal when the game runs on another
screen.

## Approach

Ask Windows for the console display state and feed it into `poll_lock` as two
more `LockSignals`, next to `session`, `secure_desktop` and `woke`.

- **Source.** `PowerSettingRegisterNotification(GUID_CONSOLE_DISPLAY_STATE,
  DEVICE_NOTIFY_CALLBACK, ...)` from powrprof. It takes a callback instead of
  a window handle, so no hidden message window and no message pump are needed
  (the lock checks have none either). Windows calls it once at registration
  with the current state and then on every change: 0 off, 1 on, 2 dimmed.
  The callback only stores into atomics and logs the change once
  (`display: off` / `display: on` / `display: dimmed`). It needs the
  `Win32_System_Power` feature of `windows` 0.58; the GUID itself lives in
  `Win32_System_SystemServices`, so it is written out as a constant rather
  than pulling in that whole feature.
- **Signals.** The callback counts off→on transitions. Each two-second check
  (`lock_signals` in `lib.rs`) reads the current state (`display_off`) and
  whether the count moved since the last check (`display_back`). The counter
  means an off→on that fits between two checks is not lost.
- **Rule** (pure, in `AutoBuffer::poll_lock`, behind the lock handling):
  - Seeing the display off, or back since the last check, arms a pending
    restart and resets its settle count.
  - Once the display is on again, the restart waits `DISPLAY_SETTLE_POLLS`
    (2) checks, i.e. 2–4 s after the display came back. Monitors, DisplayPort
    ones in particular, can drop out of the topology while off and take a
    moment to come back; starting a capture against a screen that is not
    there yet would fail and stay failed.
  - Then a running buffer gets `AutoAction::Restart`, exactly as after a
    sleep. Without a running buffer the pending restart is simply dropped.
  - Any restart or fresh start (wake, unlock) clears the pending one, so a
    sleep, which also turns the display off and on, restarts once, not twice.
    A wake while the display is still off restarts now and once more after
    the display is back, which is harmless.
  - A lock wins over everything: while locked the pending restart is dropped,
    since the unlock starts the buffer fresh anyway.
- **Display off does not stop the buffer.** On 2026-11-15 the earlier
  display-off (10:28–11:19) recovered on its own, so a capture across an
  off/on is not always lost, and nothing is recorded while the screen is
  dark that a stop would protect: the ring keeps what came before and the
  restart only happens once the screen is back. Stopping would also need a
  start path (like `for_lock`), which adds state for no gain.
- The restart's log line names its reason (`awake again` vs.
  `the display is back on`).

Accepted cost: the restart throws away the ring from before the display went
off. The display only goes off on the idle timeout, so nobody was playing.

## Files

- `src-tauri/Cargo.toml`: `Win32_System_Power` feature.
- `src-tauri/src/capture.rs`: `watch_display()` (registers the callback,
  Windows only) and `display_state()` (off now, off→on count); no-op stubs
  elsewhere.
- `src-tauri/src/state.rs`: `LockSignals.display_off`/`display_back`,
  `AutoBuffer` pending state, `DISPLAY_SETTLE_POLLS`, the rule in
  `poll_lock`, unit tests.
- `src-tauri/src/lib.rs`: `watch_display()` in setup; `lock_signals` fills the
  two fields; the restart log line names the reason.

## Tests (`cargo test --lib`)

- Display off→on restarts a running buffer after the settle checks, not
  before, and only once.
- While the display is off nothing happens (no stop).
- Off→on between two checks (only `display_back`) still restarts.
- Display back with no buffer running: nothing, and nothing later either.
- Display back while locked: the lock stops, the unlock starts fresh, no
  extra restart afterwards.
- Wake plus display back: one restart, not two.
- Existing lock/wake tests unchanged.

## Manual test for the owner

1. Settings → System → Power: "Turn off my screen after" 1 minute (plugged
   in). Make sure "require sign-in when the display turns off"/lock on idle
   is off, so this is a plain display-off without a lock.
2. Start ClippiBoy with the buffer running on the main screen (a game or a
   video playing on it helps to see motion).
3. Leave mouse, keyboard and controller alone until the screen goes dark;
   wait one more minute.
4. Move the mouse. Within about 5 s the log
   (`%APPDATA%\ClippiBoy\logs\clippiboy.log`) should show `display: off`,
   `display: on`, then `the display is back on — the capture starts over`.
5. Wait ~15 s, save a clip, check the game area is not black and moves.
   In the log the `pipeline:` line's `frames − repeated` should keep rising.
6. Repeat once with the lock-on-idle setting on: expect the lock lines and a
   fresh start at sign-in, and no second restart right after it.
7. Set the screen timeout back.
