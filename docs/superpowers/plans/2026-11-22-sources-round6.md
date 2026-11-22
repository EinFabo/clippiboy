# Round 6: per-app audio fixes, monitor map, in-track levels, lift-and-move (branch `ui-compromise`)

**Goal:** Make per-application audio sources reliable (incl. Windows 10, where they cannot work), and build the
approved round-6 UI: the monitor map on the Video page, levels inside the volume slider track with an "In your clip"
row, and lift-and-move reordering of audio sources. Released together with the UI rework as 1.0.0 (owner decision) —
no release now.

**Spec:** the approved mockups, section `<section class="r6">` in `.superpowers/ui-compromise/review/layout-review.html`
("A · Video" and "B · Audio", revised version). The add-source list stays as it is (owner). Findings this plan fixes:
`~/.claude/skills/clippiboy/PLAN.md` points 181 and 182.

## Global Constraints

- Same look and building blocks as the rest of the app (vertical list = the settings `.vseg` look, cards, rows,
  segmented, toggles, the slider); only app colours (violet family), no green/yellow/red meters.
- **Rust changes are allowed in this plan.** New Tauri commands must be added to `src-tauri/build.rs`, registered in
  `lib.rs`'s `invoke_handler`, get their permission file and be granted in `src-tauri/capabilities/default.json`
  (the `grants` test fails otherwise). New config fields: `#[serde(default)]`, mirrored in `src/lib/types.ts`, mocks in
  `src/lib/mock.ts`. Pure Rust logic gets unit tests.
- **One Rust build at a time.** The owner's dev build must be closed before any cargo command (it rebuilds on every
  Rust change). Rust tests via PowerShell as `.superpowers/HOUSE-RULES.md` describes; read the log, not the exit code.
  Never run a Linux cargo in WSL. Never start the app.
- Frontend: `npx tsc --noEmit`, `npm test`; pure TS logic in `src/lib/` with tests; visual checks via
  `.superpowers/ui-compromise/tool/shoot.mjs` (kill servers by PID).
- Console (`src/console/`) and banner (`src/overlay/`) keep working; if a shared payload changes (levels), adapt them.
- English UI, no new hint lines. Git: branch `ui-compromise`, stage by name, English house-style commits, no trailers;
  follow `.superpowers/HOUSE-RULES.md` content rules (its branch name does not apply).

## Task 1: Per-app capture only where Windows supports it (point 181)

- Read the OS build once (`RtlGetVersion`); process loopback needs build ≥ 20348 (Windows 11). Pure rule
  `app_capture_supported(build) -> bool` with tests.
- When unsupported: `resolve` leaves Game and Process sources out, `records_single_applications` is false (devices are
  recorded whole again), and each such source gets a fixed error note "Per-app audio needs Windows 11" (via the existing
  source notes/errors path). Desktop and game sound must never go silent because of it. Tests for the resolve rule.
- New command `app_capture_supported() -> bool`. Frontend: the add list's Apps tab shows a short line "Per-app audio
  needs Windows 11" instead of the list when unsupported; the Discord row in Quick setup (setup wizard and empty Audio
  page) is hidden; the old dormant `appCaptureUnavailable` regex and its "20348" note go away in favour of this flag.

## Task 2: Choosing and finding apps reliably (point 182, part 1)

- `list_audio_processes`: one entry per application — group sessions by `app_root`, return the root pid and exe, a
  process count, and whether any session is active; never list ClippiBoy's own process tree (`std::process::id()` and its
  WebView2 children); drop display names starting with "@" (fall back to the exe name).
- Helper processes: for known audio helpers (`msedgewebview2.exe`, `steamwebhelper.exe`, `EpicWebHelper.exe`,
  `CefSharp.BrowserSubprocess.exe`, …) walk up to the parent with a different exe and store that exe; `live_pid` never
  picks a process inside ClippiBoy's own tree. Tests with fake process trees.
- `resolve_for`: the same pid tapped by two process sources is tapped once (log it).
- Taps carry names: the core sends the tapped process's name with `audio-taps` so the UI never shows "PID 1234".
- Frontend (add list stays visually as it is): an app already added shows "Added" and cannot be added twice; the
  detected game is marked; the list refreshes every 2 s while open.
- Status line: an application source counts as running when the core taps it (`audio-taps`), not when the process list
  last saw a session; when no matching process lives during buffering, the core writes a source note "<App> is not
  running — this track stays silent" (shown in the row as today's notes are).

## Task 3: New apps are picked up at once (point 182, part 2)

- With application sources active, the output devices' leftovers are refreshed every 2 s, so a newly playing app
  misses ~2 s. Register `IAudioSessionNotification::OnSessionCreated` per output device and trigger the existing
  refresh immediately (keep the 2 s poll as a fallback). Unregister cleanly on stop. Note in the report what still
  cannot be captured in leftovers mode (system sounds pid 0, exclusive mode) and whether pid 0 can be included.

## Task 4: Levels inside the slider track and "In your clip"

- Track look (mockup B): the slider's filled part up to the thumb is the dark accent (`accent-deep`), the live level is
  a bright accent bar inside the track (accent → accent-bright with a soft glow), drawn by the existing shared meter loop
  (`Meter variant="track"`); no separate bar. The thumb gets a warm ring (`warn`, ~55 %) only while the level is near
  clipping. Muted/disabled sources keep a dark track. Keep the slider input fully usable and accessible.
- Levels may exceed 1.0 so near-clipping is visible: remove the hard `.min(1.0)` in `audio/engine.rs` levels or add a
  separate "hot" flag — check console and banner consumers first and keep their look.
- "In your clip": the core emits the main-mix level as an approximation (power sum `sqrt(Σ (peak_i·gain_i)²)` over the
  enabled sources that are not on their own track), in a separate payload field (not a fake source id); L/R only if
  cheap (otherwise one value drawn as two equal lines is not acceptable — then draw one line). Frontend: the first row of
  the list "In your clip" with the same track look (non-interactive), the peak in dB in the dB column, sub line
  "Main track · N own tracks". Update the console if it consumes the payload type.

## Task 5: Lift-and-move reordering of audio sources

- Replace the HTML5 drag (`draggable`, `setDragImage`, insert line) in `src/routes/AudioMixer.tsx` with a pointer-based
  hook `src/lib/useReorder.ts`: pointer down on the grip + ~4 px movement starts the drag (pointer capture,
  `touch-action: none`, no text selection); the dragged card follows the pointer via `transform` (no React render per
  move) with lifted look (accent border ring, shadow, slight scale); the others slide aside with a short transform
  transition and a gap opens at the target; release writes the new order once (same `patchConfig({sources})` as today)
  and the card settles; Escape/pointercancel restores without writing; auto-scroll near the edges of the scroll
  container; reduced motion = no transitions. Coordinate with `useFlip` so nothing animates twice. Keyboard reordering
  (arrow keys on the grip) stays, plus an `aria-live` "Moved to position N of M". Pure `insertIndex(mids, y)` with tests.
- Sources become separate cards with a small gap (mockup B) so lifting reads naturally.

## Task 6: Monitor map on the Video page

- Rust: `CaptureTarget` gains `x`, `y` (virtual-desktop origin, from `GetMonitorInfoW`'s `rcMonitor` already read in
  `monitor_proc`), and for windows `icon: Option<String>` (PNG data URL) resolved in the command wrapper
  (`commands.rs` `list_capture_targets`, not in the internal `list_targets`) via `QueryFullProcessImageNameW` →
  `IShellItemImageFactory::GetImage` (64 px, icon only) → PNG, cached per exe path (also negative results). Check the
  process DPI awareness (per-monitor v2) and keep coordinates in one system; log it.
- Frontend (`src/routes/Recording.tsx`): the Source card becomes two columns — left a vertical list in the settings
  `.vseg` look (screens with number and resolution, divider, windows with their app icon; the detected game marked),
  right the monitors drawn at their real relative positions and sizes on the plain card surface, each with its number
  pill, the selected one with the accent ring and check; a line below with the selected screen's details. Click on a
  monitor or a list row selects it (same config writes as today, `targetStableId` first). An unplugged saved monitor
  stays as a dashed outline at its last position. Selecting a window shows that window's icon/name on the right instead
  of the map. Windows stay in the list only (not on the map).

## Task 7: Still images on the monitor map

- Rust command `capture_thumbs(ids) -> Vec<{id, jpeg: Option<String>}>`: monitors via GDI (`GetDC(NULL)` +
  `StretchBlt` with `HALFTONE` straight down to ~320 px wide) — no WGC, so no yellow capture border; JPEG (~70) via the
  existing `jpeg-encoder`, returned as data URLs; sequential, cheap. Skip when the screen is unrecordable (lock/UAC:
  `capture::screen_unrecordable()`) and in streamer mode (return none). Content-protected windows simply appear black —
  no special case.
- Frontend: fetch every 3 s only while the Video page is open and visible, never overlapping; show the image dimmed
  (unselected) / full (selected) as in the mockup; fall back to the plain surface without an image.
