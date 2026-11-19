# UI compromise: same look, better structure (branch `ui-compromise`)

**Goal:** Fix the places in the main window where people have to hunt or click too much, without changing how
ClippiBoy looks. The owner is happy with the current design and rejected a redesign.

**Spec:** the approved mockups, `.superpowers/ui-compromise/mockups.html` (open it in a text editor; every mockup is
plain HTML/CSS built from the app's own values). Sections "1 · Clips", "2 · Settings", "3 · Player",
"4 · Hinweise" and the list "Plan, angepasst" are binding. The page is German; the UI stays English.
Screenshots of today's UI: `.superpowers/ui-compromise/today/*.png`.

**Owner decisions (2026-11-19):** settings navigation vertical on the left; day headings in Clips: yes; no search
in Settings for now; the accent colour stays a hidden easter egg (Colors tab unchanged).

## Global Constraints

- **No new look.** Use only existing building blocks: the glass pill (`NavBar.tsx`, `var(--glass)` +
  `backdrop-filter: blur(20px)` + `border-white/10` + the shadow), `Segmented`, `Toggle`, `Slider`, `Select`,
  `Button` (primary is white), `Card` (20 px radius), `Row` (`routes/settings/shared.tsx`), `Pill`, the chips of
  `Clips.tsx`, `Menu`/context-menu styles, the tokens in `src/styles/tokens.css`. No new colours, radii, fonts,
  shadows or animations beyond what these already use. When something new is needed, it is an existing piece
  turned or reused (e.g. the vertical settings list = `Segmented` turned 90°).
- Unchanged: the NavBar (six items, buffer badge), Overview/Dashboard with its hero, the Audio and Video pages,
  the player's side column layout, the console (`src/console/`), the banner (`src/overlay/`).
- **Frontend only.** No Rust changes, no config schema changes (no new or renamed fields in `AppConfig`), no new
  Tauri commands, no new npm packages. Every existing setting keeps its exact behaviour; only where it is shown
  changes.
- All visible text English. No explanatory hint line under every setting (the owner finds that looks AI-made):
  label + control, at most a short status. Keep existing hints where they already are, don't add new ones.
- `prefers-reduced-motion` keeps working (`src/styles/motion.css`).
- Tests: `npx tsc --noEmit` and `npm test` (node --test, `src/**/*.test.ts`) from WSL must pass. Pure logic
  you add (filter combination, sorting, grouping by day, notice priority, selection ranges) goes into a small
  module under `src/lib/` with a `*.test.ts` beside it, following the existing tests there.
- Visual self-check: build and screenshot in browser mode as described in
  `.superpowers/ui-compromise/tool/shoot.mjs` (header comment). Browser mode uses `src/lib/mock.ts`; extend the
  mocks only if a screen cannot be shown otherwise. Look at your screenshots before reporting.
- Never start the app (`npm run app`, `tauri dev`), never run `npm install`/`npm ci` from WSL, never drive the
  ClippiBoy window with mouse or screen.
- Git: branch `ui-compromise` only; never switch branches, rebase, push or merge. Stage only your files by name
  (never `git add -A`/`.`); untracked `*-liste.md`, `docs/review/`, `.superpowers/` stay uncommitted. Commits in
  English, house style (see `git log`: "Area: what the user gets — why", one long paragraph is fine), **no
  `Co-Authored-By` or any other trailer**. Also follow the content rules in `.superpowers/HOUSE-RULES.md`.

## Task 1: Small fixes, one notice at a time, shorter setup

Plan points 170, 171, 179.

1. `src/components/Setup.tsx` (around line 434): the text says "Recording page"; the page is called "Video".
2. `src/routes/Clips.tsx`: an empty library and an empty search/filter result show the same "No clips found."
   (also the trash list). Give them different states, in the existing `Card` style:
   - empty library: "No clips yet" plus the Save-clip hotkey as shown elsewhere (the Dashboard's empty card does
     this already — reuse its wording/markup);
   - nothing matches: "Nothing matches" plus a "Clear" button that resets search and filters;
   - empty trash: "Nothing deleted".
3. `src/styles/tokens.css`: `--color-ink-faint` from `#5c5c68` to `#7d7d8a` (4.6:1 on surface; still visibly
   darker than ink-muted `#8a8a96`). Check the console and banner still read fine (they may use it too).
4. `src/components/ClipPlayer.tsx` key map (around line 597): add `j` = seek −10 s, `l` = seek +10 s. Update the
   shortcut hint line in the player footer to mention J/L. Do the same in `ShotViewer.tsx` only if it has a
   matching seek concept (it does not for stills — then skip).
5. One notice at a time (`App.tsx` renders `FfmpegNotice`, `UpdateNotice`, `UsageNotice`, `DiscordNotice`
   stacked, up to four cards). Show only the most important one that currently wants to show, priority
   ffmpeg > update > usage consent > Discord. When more than one wants to show, the visible card gets a small
   pager "‹ 1 / 3 ›" at its left edge (see mockup section 4) to look at the others; the order is the priority
   order. Each notice keeps its own text, buttons and behaviour (UsageNotice still stays until answered,
   DiscordNotice still counts as said only once answered). Card look unchanged.
6. Toasts (`src/components/Toasts.tsx`, bottom centre, unlimited) move to the bottom-right corner, stacked above
   the transfer cards of `Share.tsx` (`Transfers`, bottom right, which already move up when the player or setup
   is open — toasts follow the same rule). At most 3 toasts visible; older ones drop off first. Error toasts stay
   until clicked away; ok toasts keep their 3 s.
7. Setup's hotkey step (step "hotkeys" in `Setup.tsx`) renders the full `<Hotkeys/>` settings page. Show only the
   "Save clip" binding (same control as in Settings) and a ghost link/button "More hotkeys in Settings" that
   finishes nothing but tells where; it does not need to navigate.

## Task 2: Share button in the player and the screenshot viewer

Plan point 174. Mockup section "3 · Player".

- Add a white primary `Button` "Share" with the share icon next to the close button in the header of
  `ClipPlayer.tsx` and `ShotViewer.tsx`. It opens a menu in the glass style (same surface, border and blur as the
  NavBar pill, rounded like the existing context menu) listing, in this order, exactly the share entries the
  context menu (`src/components/clipMenu.tsx`) offers for that clip, with the same enabled/disabled logic and the
  same reason text when disabled: Copy for Discord (with the size limit note it has today), Copy picture / Copy
  clip / Copy file as applicable, Share as link (or Copy share link · time left / Delete share link), Send to a
  friend…, Post to <Discord channel> (only with a webhook), divider, Export a copy… (videos only).
- Build the entries from one shared source so the context menu and the Share menu cannot drift: extract the
  share part of `clipMenu.tsx` into a function both use. The context menu keeps all its entries and order.
- Export a copy opens the existing `ExportDialog` as today.
- The menu closes on Escape (without closing the player), on outside click, and after choosing an entry. Escape
  handling in the player is layered (`ClipPlayer.tsx` onKey: fullscreen, delete question, export dialog …) — the
  open menu must take Escape first.
- Expose a small reusable `ShareMenuButton` (or similar) so Task 4 can put the same menu behind a "Share" button
  on clip cards.

## Task 3: Clips: All, combinable filters, sort, day headings

Plan point 172. Mockup section "1 · Clips".

- Type filter becomes a `Segmented` "All · Clips · Recordings · Screenshots" with counts, default **All** (all
  three types). It replaces the type chips ("Clips", "Recordings", "Screenshots") of today's chip row.
- The remaining chips (Favorites, one per game, "No game", tags) become toggles that combine: type AND
  (favorites if on) AND (any selected game) AND (all selected tags). Selected chips look like today's active
  chip (white) with a ✕. Search combines with all of it. Keep the game chip's existing hover-✕ that detaches a
  game name (it asks first) — it must not be confused with the deselect ✕: deselect is a click on a selected
  chip; detaching stays where it is today, on hover of an unselected chip.
- The chip row wraps to at most two lines instead of scrolling sideways; chips that don't fit collapse into a
  last chip "+N more" which opens a menu (existing menu style) listing the hidden chips as toggles.
- "Recently deleted" leaves the chip row and becomes the last entry of that "+N more" menu if chips overflow,
  otherwise a ghost button at the end of the row. Its view stays as it is today.
- Sort as a ghost `Button` "Newest ▾" right of the count: Newest, Oldest, Longest, Largest. Default Newest.
  Count text "N of M" as today.
- Day headings above groups when sorted by Newest/Oldest: "Today", "Yesterday", then weekday names for the
  last 7 days ("Monday"), then dates ("November 3" / with year when not the current year). Style: like the
  mockup — 15 px semibold `text-ink` at ~80 % plus a faint count ("3 clips"); no new component look. No headings
  for Longest/Largest.
- Grid: keep 3 columns at the default window width; it may go to 4 when the window is wide enough for cards of
  today's width (≥ ~1500 px content) — responsive, no smaller cards.
- Cards stay as they are, plus a second button "Share" (secondary, share icon) beside "Open" — leave it
  rendered but disabled until Task 4 wires it, or wire it directly if Task 2's `ShareMenuButton` exists (it
  will: Task 2 runs first).
- Filter state persists while the app runs (as today), not across restarts.
- Pure logic (filter combination, sort, day grouping) in `src/lib/` with tests (day boundaries in local time,
  year change, empty groups).

## Task 4: Multi-select in Clips

Plan point 173. Mockup section "1 · Clips" (selection bar).

- Ctrl-click toggles a card; Shift-click selects the range from the last clicked card in the current visible
  order; Ctrl+A selects all visible (only while the grid has focus, not in the search field); Escape clears.
  A plain click on a card while a selection exists toggles it instead of opening (like file managers in selection
  mode); with no selection a click opens as today.
- Selected cards: accent border (1 px ring with `accent`) and a round check (white ring, filled accent when
  selected) at the top right; the hover tools there (show in folder, delete) hide while a selection exists.
- Selection bar: the NavBar's glass pill, floating at the bottom centre of the window above toasts/transfers'
  row (not covering them): "N selected" · ♥ Favorite (toggles: if all selected are favourites, remove, else
  add) · Send to a friend (only when signed in; opens the existing SharePicker flow for several clips if it
  supports several, otherwise send one after another the same way the context menu does for one) · Delete
  (moves to Recently deleted, asks once inline like today's delete confirm, then shows one toast) · ✕ clears.
- Selection is cleared when the filter/search changes in a way that hides selected cards (hidden ones drop out).
- Range/toggle logic in `src/lib/` with tests.

## Task 5: Settings: vertical tabs, recut tabs, direct links

Plan point 176. Mockup section "2 · Settings".

- `src/routes/settings/index.tsx`: the horizontal `Segmented` of tabs becomes a vertical list on the left
  (~210 px column, content on the right): same surface/border as `Segmented` but with 20 px radius, items 36 px
  high, white active item with black text, each item with a 15 px icon from `src/components/icons.tsx` (add
  simple stroke icons there in the existing style if one is missing). Dividers between groups as in the mockup:
  Hotkeys, General, Notifications, Console, Camera | Sharing, Friends | Storage, About. The hidden Colors tab
  still appears at the end when unlocked (5 clicks on the heading), unchanged.
- Tabs recut (`SettingsTab` in `src/store.ts`):
  - `behaviour` → `general`: sections Behaviour (start buffer automatically, only buffer in game, start with
    Windows, usage stats) and Streaming (streamer mode) and the Watermark toggle (moved from Banner).
  - new `sharing`: the Sharing section from Behaviour (Discord upload limit, mute Discord voices, post to a
    Discord channel, post every clip automatically) unchanged.
  - `banner` + `sounds` → `notifications` (built in Task 6; for this task create the tab and render today's
    Banner content and Sounds content one under the other so nothing is lost).
  - Friends tab: unchanged in this task.
- Anything that referred to old tab keys (store default, Colors' `setTab`, Dashboard/other links) is updated.
- Direct links: text that sends people to a settings place becomes a link that goes there. At least
  `UsageNotice.tsx` ("You can change it in Settings → Behaviour" → link to General). Navigating to a settings tab
  from outside Settings needs the route switched too — add one helper (e.g. `openSettings(tab)`) using the
  existing `setSettingsTab` and the route setter in `App.tsx`; don't add a router.
- Narrow windows (min width 1080): the vertical list stays; nothing overflows.

## Task 6: Notifications as one list

Plan point 177. Mockup section "2 · Settings" (right side).

- The Notifications tab shows:
  1. A card with: "Banner over the game" (`overlay.enabled`), "Banner position" (today's Screen select and the
     corner pad side by side), "Duration" (today's control), "Play sounds" (`sounds.enabled` or however it is
     named), "Sound output" (device select + master volume slider in one row as in the mockup), "REC badge while
     recording".
  2. A card "per event": a header row (Event · Volume · Banner · Sound) and one row per event: ▶ preview button,
     event name, volume slider + %, Banner toggle, Sound toggle — the Sounds tab's row look. Events and fields:
     - Clip saved: banner `overlay.onClipSaved`, sound kind for clip saved
     - Screenshot taken: `overlay.onScreenshot`, screenshot sound
     - Recording started: `overlay.onRecording`, recording-started sound
     - Recording saved: `overlay.onRecording` (same field), recording-saved sound
     - Buffer on: `overlay.onBufferToggle`, buffer-on sound
     - Buffer off: `overlay.onBufferToggle` (same field), buffer-off sound
     - A friend comes online: the Friends tab's "A friend comes online" notification field, friend-online sound
     - A friend starts a game: the Friends tab's field, no sound (empty cells)
     - Friend requests: the Friends tab's field, no sound
     - Clip from a friend: no banner field today → empty banner cell, clip-from-friend sound
     - Errors: `overlay.onError`, error sound
     Where an event has no banner or no sound setting, the cell is empty (no disabled toggle). A shared field
     appears in both rows and both toggles show the same state.
  3. "Notify me while I'm playing" (moved from Friends → Notifications) below, as a normal `Row`.
- Banner toggles are disabled while the banner is off, sound toggles/sliders while sounds are off — as today.
- Remove the Notifications section from the Friends tab (it moved). Friends keeps Privacy, Clips from friends,
  Blocked, Account.
- Verify field names in `src/lib/types.ts` / current `Banner.tsx`, `Sounds.tsx`, `settings/Friends.tsx`; the names
  above are descriptive. No config changes.

## Task 7: Accessibility pass

Plan point 180.

- Every interactive element in the main window (not console/banner) shows a visible focus ring on keyboard focus
  (`:focus-visible`, 2 px, accent-bright, offset 2) — add it centrally (base styles in `src/index.css` or the
  primitives in `src/components/ui/`), not per call site.
- Click targets at least 24×24 px in the main window (icon buttons on cards and in the player footer are the
  likely offenders); grow the hit area, not the icon.
- No text below 12 px in the main window (`text-[10px]`, `text-[11px]` → 12 px), except numeric badges inside
  pills where 11 px is needed to fit — list any you keep in the report.
- The clip grid is one tab stop with arrow-key movement between cards (roving tabindex), Enter opens, Space
  toggles selection (Task 4).
- Don't touch `src/console/` and `src/overlay/`.
