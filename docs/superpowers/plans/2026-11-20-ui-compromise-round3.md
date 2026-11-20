# UI compromise, round 3: tidy filters, collapsible settings, menus that fit (branch `ui-compromise`)

**Goal:** After testing round 2 the owner found two things: the settings pages need too much scrolling, and the
gallery's chip row looks unsorted ("+N more" mixes games, tags and "No game"; the menu is too full and is cut off at
the bottom of the window). Fix both without changing the look.

**Spec:** the approved mockups, section "Runde 3" at the top of `.superpowers/ui-compromise/mockups-r3.html` (plain
HTML/CSS from the app's own values; German text, the UI stays English): A · Clips ("Game ▾" / "Tags ▾"),
B · Notifications (collapsible Banner and Sounds), C · Hotkeys (one row per action, collapsed Controller and Stream
Deck), and the list "Regeln für den Bau". Screenshots of the current branch state: `.superpowers/ui-compromise/r3-today/`.

## Global Constraints

- **No new look.** Use only existing building blocks: the glass surface (`var(--glass)` + `backdrop-filter: blur(20px)`
  + `border-white/10` + the NavBar shadow), `Segmented`, `Toggle`, `Slider`, `Select`, `Button` (primary is white),
  `Card` (20 px radius), `Row` (`routes/settings/shared.tsx`), `Pill`, the existing chip look in `Clips.tsx`, the
  `Menu` styles, the tokens in `src/styles/tokens.css`. A new component is allowed only when it is built from these
  (the settings group below is a `Card` + `Row` with a chevron button in the existing round icon-button style).
- **Frontend only.** No Rust changes, no config schema changes, no new Tauri commands, no new npm packages. Every
  setting keeps its exact behaviour; only where and how it is shown changes. UI state such as "group open" may live in
  `localStorage` (wrap every access in try/catch; the UI must work when it throws).
- All visible text English. No new explanatory hint lines (the owner dislikes them); a collapsed group's summary text
  is a status, not a hint.
- Console (`src/console/`) and banner (`src/overlay/`) untouched.
- Tests: `npx tsc --noEmit` and `npm test` from WSL must pass. Pure logic (popover placement maths, facet sorting and
  counting, summary strings, open-state storage keys) goes into `src/lib/` with a `*.test.ts` beside it.
- Visual self-check: build and screenshot in browser mode as described in
  `.superpowers/ui-compromise/tool/shoot.mjs` (header comment); look at your screenshots before reporting. Check at
  1280×820 and at the minimum width 1080.
- Never start the app, never `npm install` from WSL, never drive the ClippiBoy window.
- Git: branch `ui-compromise` only; never switch branches, rebase, push or merge. Stage your files by name; untracked
  `*-liste.md`, `docs/review/`, `.superpowers/` stay uncommitted. English commits in house style, no trailers. Also
  follow the content rules in `.superpowers/HOUSE-RULES.md` (its branch name `fixes` does not apply here).

## Task 1: Every menu stays inside the window

- `src/components/ui/Menu.tsx` today clamps the menu's position to the window but has no maximum height, so a menu
  taller than the space below its anchor is cut off. Change the placement so that:
  - the menu opens below its anchor when it fits; otherwise above when more space is there; otherwise on the side with
    more space;
  - its height is at most the available space on that side minus 8 px, and the item list scrolls inside (the header
    or search field of a menu, if any, stays put);
  - it never extends past any window edge (8 px margin), at every window size down to 1080×680;
  - keyboard navigation inside a scrolled menu scrolls the focused item into view.
- This applies to every menu built on `Menu` (right-click menu, Share menu, sort menu, …).
- Put the placement maths in a pure function in `src/lib/` with tests (fits below, flips above, neither fits → larger
  side with max-height, horizontal clamp).

## Task 2: Clips filters as "Game ▾" and "Tags ▾"

Mockup section "A · Clips".

- The filter row is always one line: the type `Segmented` (unchanged), the Favorites chip (unchanged), a **Game ▾**
  button and a **Tags ▾** button in the chip style, and at the right end a ghost button "Recently deleted N" (only when
  the trash has clips; it opens the trash view as today). The two-line chip wrap, the per-game and per-tag chips and the
  "+N more" chip with its menu go away (remove `fitChips` and the hidden measuring copy if nothing else uses them).
- Game ▾ / Tags ▾ open a glass popover (Task 1's placement rules, same surface as the Share menu) with:
  - a search field at the top when the list has more than 8 entries (typing filters the list; Escape clears the field
    first, then closes);
  - one row per value with a checkbox (accent fill when checked, the existing check style), the name (one line,
    ellipsis) and its count; sorted by count descending, then alphabetically; "No game" (Game list only, only if such
    clips exist) at the end below a divider;
  - a footer "Clear" that unchecks this facet's values.
  Counts are computed within the current type selection (All/Clips/Recordings/Screenshots), like today's chip counts.
- Button labels: no selection → "Game ▾" / "Tags ▾" in the normal chip look; one selected → that name (ellipsis at
  ~180 px) + "▾" + "✕"; several → "2 games ▾ ✕" / "3 tags ▾ ✕"; selected state uses today's active-chip look (white).
  Clicking ✕ clears that facet without opening the popover.
- Filter semantics stay as built in round 2: type AND favorites AND (any selected game, incl. No game) AND (all
  selected tags) AND search. "Clear" in the "Nothing matches" empty state still resets everything.
- Detaching a wrongly detected game name (today the hover-× on an unselected game chip, which asks first) moves into the
  Game popover: right-click on a game row opens a small menu "Remove game name…" which asks first (same question text as
  today) and then does exactly what the old ✕ did.
- Keep everything that hooked into the old chip row working: filter persistence across page switches, selection
  pruning, the roving tabindex, the player's next/previous order.
- Pure logic (facet values with counts and sorting, label text for the buttons) in `src/lib/` with tests.

## Task 3: Collapsible settings groups; Notifications and Hotkeys

Mockup sections "B · Notifications" and "C · Hotkeys", and "Regeln für den Bau".

- New `SettingsGroup` component (in `src/routes/settings/shared.tsx` or beside it): a row inside a `Card` with the
  group title on the left and on the right an optional summary text (muted, one line), an optional master `Toggle`, and
  a round chevron button (existing round icon-button look) that opens and closes the group. Clicking the title area also
  toggles. Open: the group's rows follow below inside the same card, with the existing row dividers. Rules:
  - one level only (no group inside a group); several groups may be open at once; nothing closes on its own;
  - the open state persists per group key in `localStorage` (try/catch); default per group as stated below;
  - with a master toggle that is off, the rows inside are disabled (dimmed), not hidden; the group can still be opened;
  - keyboard: the chevron is a button with `aria-expanded` and `aria-controls`; Enter/Space toggles; respects
    `prefers-reduced-motion` (no height animation then; otherwise reuse an existing motion token).
- **Notifications** (`src/routes/settings/Notifications.tsx`):
  - First card: group "Banner over the game" (master toggle = `overlay.enabled`; summary "<screen> · <corner> ·
    <duration>", e.g. "Main screen · bottom right · 3.5 s"; inside: Banner position, Duration, REC badge while
    recording; default closed), group "Sounds" (master toggle = sounds on; summary "<device> · <volume> %"; inside:
    Sound output device and master volume; default closed), then the row "Notify me while I'm playing".
  - The per-event list stays open as it is, with flatter rows (~52 px instead of ~65 px: smaller vertical padding, 30 px
    preview button).
  - Target height at 1280×820 with groups closed: about 1100 px or less.
- **Hotkeys** (`src/routes/settings/Hotkeys.tsx`):
  - Each action is one row: label, Tap/Hold, the first binding, the second binding if set (today it sits on its own line
    below with "+ Add"), and a small round "+" button that adds the second slot when there is none. Align the Tap/Hold
    controls and bindings in columns so the rows line up.
  - "Controller" becomes a group (summary e.g. "Xbox · 2 buttons bound" or "No controller seen"; default closed) holding
    today's controller card content; it only appears when it appears today.
  - "Stream Deck" becomes a group (summary "Allowed · port N" or "Off"; default closed).
- Summary strings come from pure functions in `src/lib/` with tests.

## Task 4: Collapsible groups on Camera, Friends and Storage

Uses Task 3's `SettingsGroup`.

- **Camera** (`src/routes/settings/Facecam.tsx`): the "Camera" master switch stays at the top with the live placement
  preview; the rest becomes groups: "Where it appears" (In clips, In recordings, In screenshots, In the console;
  summary e.g. "Clips · Recordings"), "Position" (Shape, Mirror, Delay; summary e.g. "Round · mirrored"), "Virtual
  camera" (toggle + Update/Remove; summary "On"/"Off"). Device and Resolution stay visible right under the master
  switch. Default: groups closed. When the camera is off, the groups are disabled (dimmed).
- **Friends** (`src/routes/settings/Friends.tsx`): Privacy and Clips from friends stay open sections as today; "Blocked"
  becomes a group (summary "N blocked" / "Nobody"; default closed) and "Account" a group (summary "Signed in as
  <name>"; default closed; Sign out and Delete account inside, Delete account keeps its confirmation).
- **Storage** (`src/routes/settings/Storage.tsx`): the read-only breakdown "Everything ClippiBoy takes" becomes a group
  whose summary is the total size; default closed. Storage location, Clear all trims, Bake all cams and Clean up stay
  visible.
- Check every settings page at 1280×820 and 1080 wide; report each page's height with groups closed.
