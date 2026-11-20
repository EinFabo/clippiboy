# UI polish, round 4: smooth, no pop-in, a cleaner game list (branch `ui-compromise`)

**Goal:** The owner tested round 3 and reports: settings content loads after a tab opens (e.g. Storage), the game
selection is "not clean enough", and the UI feels a bit laggy. A measured audit (300 clips, headless Chromium) found
the causes. Fix them without changing the look. (The Audio/Video redesign is a separate, later plan.)

**Spec:** this plan; the audit findings are summarised in each task with the measured numbers. The look stays exactly
as it is (screens in `.superpowers/ui-compromise/r5-today/`).

## Global Constraints

- **Frontend only, no new look.** No Rust changes, no config schema changes, no new Tauri commands, no new npm
  packages. Every existing behaviour stays; only cost and timing change, plus the small list polish in Task 5.
- Console (`src/console/`) and banner (`src/overlay/`) untouched, except where a shared primitive they import changes
  in a backward-compatible way.
- All visible text English; no new hint lines.
- Tests: `npx tsc --noEmit` and `npm test` from WSL must pass; pure logic you add goes to `src/lib/` with tests.
- Measure before and after where the task names a number: the audit's harness is in
  `/mnt/c/Users/fabia/projects/clippiboy/scratch/perf/` (`measure.mjs`, `profile.mjs`, `trace*.mjs`, a perf build config
  `vite.perf.config.ts` with `perf-entry.ts`/`perf.html` that exposes the stores and seeds 300 clips). Build via
  PowerShell, serve with `python3 -m http.server <port> --bind 127.0.0.1` started in the background and killed by its
  PID (never `pkill -f`), drive with `playwright-core` from `.superpowers/ui-compromise/tool/` and Chromium at
  `~/.cache/ms-playwright/chromium-1194/chrome-linux/chrome`. Report before/after numbers.
- Visual self-check with `.superpowers/ui-compromise/tool/shoot.mjs`; nothing may look different (except Task 5).
- A dev build of the app is running on the owner's PC; never start or stop the app, never `npm install` from WSL.
- Git: branch `ui-compromise`; stage your files by name; English house-style commits, no trailers; follow the content
  rules in `.superpowers/HOUSE-RULES.md` (its branch name does not apply).

## Task 1: Clips gallery cost

- Replace the Tailwind `group-has-[:focus-visible]` rules on the clip tile (`src/routes/Clips.tsx` ~865 and ~893): they
  make every DOM insertion on the gallery restyle the whole document (~50 ms instead of ~2 ms per insert with 300
  clips). Use the existing pattern `group-data-[nav-focus]`: the tile sets a `data-kbd-focus` attribute on focus when
  `event.target.matches(":focus-visible")` and removes it on blur; the classes become `group-data-[kbd-focus]:…`. Same
  look for keyboard users. Grep the main window for other `has-[` rules with descendant arguments and treat them the same
  if they sit on many elements.
- Stop the hover storm while scrolling: `<main>` (`src/App.tsx` ~84) sets `data-scrolling` on scroll and removes it
  ~150 ms after the last scroll event (passive listener, one timer); CSS `main[data-scrolling] [data-flip] {
  pointer-events: none }` (or the tile root selector actually used). Measured: 2.8 s → 0.6 s main thread per 3 s of
  wheel scrolling over the grid.
- `content-visibility: auto` with `contain-intrinsic-size: auto <typical section height>` on each day section of the
  grid. Check `useFlip` (`src/lib/useFlip.ts` ~60 measures `offsetTop`) still animates correctly when filters change
  and sections are off-screen; if it breaks, scope the property so FLIP keeps working (or leave it out and say why).
- `React.memo(ClipCard)` with stable handlers: pass the clip id to shared callbacks instead of creating closures per
  tile; memoise the menu options. A heart click or a `clips-changed` must re-render only the affected tiles
  (measured today: 300 tiles, 45–70 ms).
- `formatAgo` (`src/lib/format.ts` ~24) builds a new `Intl.DateTimeFormat` per call via `toLocaleDateString` options —
  create the formatter(s) once at module level (15.6 ms of every gallery render today). Keep output identical (test).
- `applyRoving` + `MutationObserver` run on every render via a layout effect without deps (`Clips.tsx` ~285–294): run
  them only when the tile order/visibility changes.

## Task 2: Page switches

- Every page starts at the top: on a route change scroll `<main>` to 0 (today Video can open half-scrolled because
  the previous page was scrolled).
- Switching to Clips mounts all tiles at once (~340 ms with 300 clips; first visit adds a 225 ms long task from the
  staggered `cb-tile-in` animation for all tiles). Render the first ~24 tiles immediately and the rest in the next
  frame(s) via `startTransition`/`useDeferredValue` or a small incremental render; limit the staggered entrance
  animation to the first visible row(s). The look of the first screen must not change; no visible pop-in of tiles
  below the fold beyond what scrolling shows. Target: under 60 ms to the first frame with 300 clips.
- Keep round-2/3 behaviours: filter persistence, trash reset on leaving, selection, roving tabindex, player order.

## Task 3: Settings without pop-in

- **Storage** (`src/routes/settings/Storage.tsx` ~72, ~147, ~185, ~194, ~346): today three cards appear one after the
  other as `storageUsage()`, `trimOriginals()` and `defaultClipDir()` answer (the usage walk covers ~12 000 files).
  Add a small store (e.g. `useStorageInfo`) holding `{usage, trims, defaultDir, at}` with stale-while-revalidate:
  prefetch when the Settings page opens and when the pointer enters the Storage tab; keep the last result in
  `localStorage` (try/catch) so the first visit after a restart already has numbers; always render every card with its
  final height — usage group summary "—" until known, Clear all trims with a placeholder and a disabled button, Bake all
  cams based on the last known value — so nothing appears, disappears or moves after the tab opens.
- **Camera** (`Facecam.tsx` ~48, ~57): until `list_cameras` answered once, show the saved device name without
  "(not connected)"; prefetch the camera list when Settings opens.
- **Hotkeys** (`Hotkeys.tsx` ~182–219, ~238, ~326, ~343, ~405): cache the last `Controllers` answer at module level and
  prefetch it when Settings opens; decide whether the Controller group shows only after the first answer, so it does not
  pop in.
- **About** (`About.tsx` ~47): fetch `appVersion` once at app start (store) so it never shows "0.1.0" first.
- Pure parts (staleness rule, placeholder decisions) in `src/lib/` with tests.

## Task 4: Audio meters and the engine tick

- `Meter` (`src/components/ui/Controls.tsx` ~236–251) runs one `setInterval` (60 ms) per meter and animates `width`
  (layout every frame): measured 94 ms/s main-thread time, 60 layouts/s with 3 sources. Replace with one shared
  `requestAnimationFrame` loop for all meters that reads levels from the store via `subscribe` into refs and draws with
  `transform: scaleX()` + `transform-origin: left` (no React re-render per frame). Keep the decay curve (×0.78 per 60 ms,
  converted to elapsed time) so it looks the same. The console has its own meters in `src/console/Audio.tsx` — leave them
  unless they use this primitive; if they do, keep their look identical.
- Engine tick: `engine-status`, `audio-errors`, `audio-warnings`, `audio-taps` arrive as separate events each second,
  errors/warnings/taps as new objects even when empty, so `AudioMixer` and `SourceTrouble` re-render up to three times a
  second for nothing. In the store (`src/store.ts` ~453–455) only `set` when the value changed (shallow/deep-equal for
  these small objects). Test the equality helper.

## Task 5: Cleaner Game/Tags list

- Popovers and menus are too transparent: card text shows through the Game list (and the Share/right-click menus).
  Give the floating surfaces (FacetPopover, `Menu`) a nearly opaque background in the same colour family (e.g. the
  `elevated` surface at ~96 % opacity) while keeping the blur, border and shadow. The NavBar pill and the selection bar
  keep today's glass.
- Picked values first: when the list opens, the picked values are listed first (in count order) as one block,
  followed by the rest (count order, then name), "No game" last as today. The order is computed when the list opens and
  does not change while it stays open (no rows jumping on click). Pure ordering function with tests.
- The search field gets no heavy accent ring when focused programmatically on open; use the same focused style as the
  gallery's own search field (border `line-strong`), keyboard focus elsewhere keeps the global ring.
- The "Clear" footer shows only when something is picked.
- Check the merged highlight of adjacent picked rows (commit 705ba5b) still works with the new order.

## Task 6: Video page redesign

Owner approved the mockup "A · Video" in `.superpowers/ui-compromise/mockups-r4.html` (section "Runde 4", classes
`.tiles`, `.tile.sel`, `.tchk`, `.row.est`, `.row.xp`). Page: `src/routes/Recording.tsx`. Same settings, same
behaviour, new arrangement; uses `SettingsGroup` (`src/routes/settings/shared.tsx`) for collapsible parts.

- Header "Video" only. The long explanatory sentences under rows go away (owner dislikes hint text); keep warnings
  that matter (unplugged screen, encoder fallback, memory too small) as one short line in the affected place.
- **Source:** one grid of selectable tiles — monitors first, then capturable windows — each tile name + one meta line
  ("2560×1440 · 165 Hz", "Primary" as a small chip; windows "Window · 2560×1440"). The selected tile gets an accent
  ring and a round check at the top right instead of the "Selected" badge. "Refresh" as a ghost button beside the
  section title. Many windows: the window tiles stay in the same grid, the grid caps its height and scrolls inside, as
  today. The unplugged-screen state shows on the selected tile itself.
- **Quality card:** Resolution and Frame rate as `Segmented` with the options available today for the source (no
  options above the source); Quality as `Segmented` with today's five steps ("Smallest", "Small", "Balanced", "High",
  "Highest" — shorten "Smallest files" to fit); a summary row (muted background, `.row.est`) "1080p · 60 fps ·
  Balanced" left and "about N MB per minute" right, N from today's bitrate maths (`clipMegabytes` per 60 s); an
  "Advanced" `SettingsGroup` (default closed) holding Encoder and Keyframe interval, summary "<encoder name> · keyframe
  every N s"; an encoder fallback shows in that summary.
- **Replay buffer card:** "Keep the last" as `Segmented` 30 s · 1 min · 2 min · 5 min · Custom ("Custom" is selected
  when the value is none of these and reveals today's slider in the same row area); "A save writes" = today's clip
  length slider (max = buffer); a summary row "About N MB per clip" left, "Uses up to N MB memory · camera +N MB" right
  (camera part only when the camera adds memory); a "Memory limit" `SettingsGroup` (default closed) holding today's
  memory budget slider, summary "<N> MB · enough for the full <length>" or, when the limit is smaller than the estimate
  for the chosen length, a warn-coloured "<N> MB · only about <s> at this quality".
- Pure helpers (preset matching for buffer length, MB per minute, the memory summary and warning) in `src/lib/` with
  tests. No config changes: every control writes the same fields as today.

## Task 7: Audio page redesign

Owner approved the mockup "B · Audio" (same file) and decided: **Solo is removed from the Audio page.** Page:
`src/routes/AudioMixer.tsx`.

- Header "Audio" with the primary "+ Add source" button on the same line; drop the subtitle.
- **One card, one row per source** (~60 px): drag grip, round icon, name + one status line (application: "<app> ·
  running" / "Not running · waiting"; device: device name / "Unplugged"), then the volume slider with the level meter
  drawn as a thin bar behind it (Task 4's meter primitive), the dB value, a round mute button (speaker icon, `aria-label`
  "Mute <name>", active state as today), a labelled pill toggle "Main mix" / "Own track" (today's ⧉, same field), the
  enable `Toggle`, and a "⋯" button opening a `Menu` with Rename and Remove (today's inline rename and trash).
- **Solo:** remove the S button. Sources saved with `solo: true` would silence the others in recordings with no way
  to undo it in the UI, so when the page (or the app at start) sees a source with `solo: true`, write it back as
  `false` once. The clip player's per-track Solo (`useClipMix`) stays — it is a different feature.
- Problem states in the row: a missing app or device turns the row's status line warn-coloured and the row slightly
  tinted (today's hint/error line content, shortened); the row stays editable.
- Remove the "Give them their own tracks" hint card and the two grey footer lines; the Windows-build note appears only
  when application capture is actually unavailable (as an error in the add list); "experimental" appears once (in the
  add list's Apps tab), not per row.
- **Add source:** today's inline add panel becomes a list with two tabs `Apps` · `Devices` (Segmented): Apps shows
  "Detected game: <name> [Add]" on top (when there is one), then running applications with audio; Devices shows outputs
  and inputs in two groups. Use the glass popover approach of `FacetPopover` (placement via `placeMenu`) or keep it
  inline under the header — your call, but it must stay inside the window and close on Escape/outside click.
- Empty state (no sources): today's QuickSetup card stays.
- The name "Mikrofon" in the mock is a Windows device label; leave names as the user/Windows sets them.
- Pure helpers (status line text, solo clean-up decision) in `src/lib/` with tests. No config schema changes.
