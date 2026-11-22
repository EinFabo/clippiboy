# Round 7: Friends page rework in every state (branch `ui-compromise`)

**Goal:** Build the approved Friends proposal: a header like Audio's instead of two cards, a "Pending" group for
everything waiting for an answer, transfers shown in the friend's row (with Retry), status in one menu, a multi-select
send picker, and calm empty/offline/streamer states. Released with the UI rework as 1.0.0.

**Spec:** the approved mockups, `<section class="r7">` in `.superpowers/ui-compromise/review/layout-review.html`
(blocks A–H, each with "Heute"/"Vorschlag"; replica CSS under "/* Runde 7 */"; control screenshots
`review/r7/look-*.png`, today's `review/r7/now-*.jpg`). Read the block notes ("Neu: …") — they are the requirements.

## Global Constraints

- **Frontend only.** No Rust, no server changes, no cargo (the owner's dev build is running and must not be disturbed;
  Vite hot-reloads). The server already provides what is needed (`server/src/index.ts` lists `friends`, `incoming`,
  `outgoing`, `blocked`; remove = "declines, withdraws or unfriends"; presence carries `lastSeen`). If something truly
  needs a server/Rust change, leave it out and report it.
- Same look and building blocks as the rest of the app (glass nav, white segmented, 20 px cards, rows, toggles, chips,
  round avatars, popover like "Add source", `placeMenu`, `FLOATING` surface, `SettingsGroup`); colours from tokens.
- English UI, no hint lines under settings/rows. Follow the content rules in `.superpowers/HOUSE-RULES.md`.
- Keep every existing capability working: sign-in/out, invites (incl. arriving via an invite link → popover opens with
  the code filled in), requests on/off, block/unblock, presence, streamer mode (names in the main window stay visible;
  console and banners keep "A friend"), receiving and sending clips, the console's friends panel and banners
  (`src/console/`, `src/overlay/`) unchanged.
- Pure logic (grouping/sorting, pending count, transfer state per friend, picker filters/selection) in `src/lib/` with
  tests. `npx tsc --noEmit`, `npm test`; browser-mode screenshots via Playwright (own build folder
  `scratch\ui-dist-r7-<task>`, own port, kill servers by PID — never `pkill -f`). Browser-mode mocks may be extended so
  every state can be shown.
- Git: branch `ui-compromise`, stage by name, English house-style commits, no trailers.

## Task 1: Header, status menu, Add friend popover, signed-out/empty/offline/streamer states, Settings › Friends

- Blocks A, B, D, E, H. Header: title left; right the status chip (avatar + coloured dot + Online/Busy/Appear offline,
  "Connecting…" with a yellow dot while the hub is unreachable) opening a menu (Online, Busy, Appear offline, the status
  text, a link to Friends settings) and the white "Add friend" button opening a popover (add by name/code with inline
  result: sent / already friends / unknown name / doesn't take requests / errors; own code and Copy invite link;
  "Requests are off · Turn on" when the user turned them off). The old profile and add cards and the red error card go.
- Signed out: one card, one sentence, the faint list behind it; signing in: "Finish in your browser" with Cancel and
  Open browser again; failed: reason in red under the button, button "Try again".
- No friends: a card with own code, "Copy invite link" (white) and the code button. Hub unreachable: a notice card like
  the Overview's update card on top, the list below greyed as one group with last known state, sending off.
- Streamer mode: a small "Streamer mode" chip next to the title; code and link hidden with an eye to reveal the code
  briefly; copying still works.
- Settings › Friends: no hint lines; Busy and Appear offline removed there (they live in the status menu); the rest in
  three small groups (what friends see / who may send requests / who may send you clips); Blocked and Account stay
  collapsed as today.

## Task 2: The list — Pending, In game, Online, Offline; transfers in the row

- Block C and G. Groups with small titles and counts like the Clips day groups: "Pending" (incoming requests with
  Decline/Accept, incoming clip offers with a small thumbnail and Decline/Accept, then own outgoing requests faint with
  "Withdraw"), "In game" (game name + duration, status text after the name), "Online" (incl. Busy in red text, Busy
  can't receive), "Offline" (collapsible like the console's "N offline", "Last seen …" from `lastSeen`).
- The send button is always visible for reachable friends; the context menu stays; "Remove friend" asks once like
  Delete for clips.
- The nav badge on Friends counts only what needs an answer (incoming requests + incoming clip offers).
- Transfers on the Friends page live in the friend's row: sending shows a thin bar with percent and an X to cancel;
  done shows a green check "Has your <clip> · just now" until the friend's state changes; an error stays with "Retry"
  and an X until dismissed. Receiving shows the bar in the sender's row, then "Open" jumps to the clip. On other pages
  the bottom-right cards stay; failed ones there also get "Retry" and no longer vanish on their own.

## Task 3: Sending a clip — from the friend and from the clip

- Block F. From a friend: the picker shows what the friend is playing under their name (no explanatory sentence),
  filter chips (Recent, the friend's game, Favorites), gallery tiles with a check for multi-select, and "Send N".
  From a clip: the friend list shows Busy greyed and Offline below with "Last seen", instead of hiding them; nobody
  reachable → the same list, all greyed, no lone sentence.
- Multi-select sends each clip as today's single send does (sequentially or as the existing send API allows); each
  shows its own progress in the friend's row (Task 2).
