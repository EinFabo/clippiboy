# Review beyond the facecam (extra)

This covers the rest of the codebase: the core, the Stream Deck plugin, friends and sharing, the server and the app UI. You did not ask for it, so take what is useful.
Method: read-only reviews, and a second reviewer re-checked every finding against the code. IDs with a V (NET-V1, UI-V1, …) are that second reviewer's own finds. Nothing was built or run. Line numbers are as of 401a320.
Severity is user impact (high, medium, low–medium, low). Items already in `docs/facecam-review.md` are only pointed to.

## Worth doing first

| ID | Severity | What | Effort |
|---|---|---|---|
| NET-3, NET-V1, NET-8 | medium / low–medium / low | Sharing gives away more than the user chose. Muted stems go to friends raw. Old multi-track clips go onto public links with every audio track. The sender's name shows on public pages. | S each |
| NET-1 | medium | A friend with a modified client can see through "Appear offline". | M |
| NET-4, NET-6 | medium | One signed-in user, or one or two plain IP addresses, can use up the free-tier daily write budget. That breaks presence, sign-in and links for everyone (if the account is on the Free plan). | S for a WAF rule and a global ceiling, M for the rest |
| CORE-1 | medium | The Stream Deck shows ⚠ for clip saves and recording stops that worked. | S |
| UI-4 | medium | Opening the Video page while the chosen screen is absent permanently lowers resolution and fps. | S |
| UI-3, UI-V2 | medium / low–medium | Escape on a context menu also closes the player, and the player throws away unsaved trim and mix. | S |
| UI-V1 | medium | Every export failure is silent. | S |
| CORE-2 | medium | Starting the app a second time destroys the previous session's log and opens the audio capture. | M |
| (improvement) | — | Check production D1 for the test accounts the old e2e script seeded with fixed tokens in this public repo. | S |

## App core and Stream Deck (CORE-n)

#### CORE-1 — The Stream Deck shows ⚠ for saves and recording stops that worked
**Severity:** medium · **Verified:** confirmed · **Where:** `streamdeck/src/client.ts:53`, `:162-188`; `streamdeck/src/keys.ts:45-58`; `src-tauri/src/control.rs:321-388`; `src-tauri/src/lib.rs:107-158`, `:233-268`
**When** a clip save, a screenshot in front of a still screen, or a recording stop takes more than 2 s, **then** the plugin aborts the request. It logs "ClippiBoy is not running" and shows ⚠, but the work still finishes. The server answers only after muxing, the thumbnail and the stems are done, and the code itself says that takes "several seconds". A recording stop is practically always over 2 s. `README.md:103`, `:106` and `streamdeck/README.md:22-23` promise a checkmark.
There is also no "finalizing" state. During the finalize the key shows "not recording", so a press starts a new recording, and `{"action":"stop"}` answers ok before the file exists. That is a UX gap, not a fault.
**Fix:** Give each route its own timeout in the plugin: minutes for `/v1/recording`, 30–60 s for `/v1/clip` and `/v1/screenshot`. On `TimeoutError`, keep the connection and show "…" until the poll says `saving == false`. Do not report "not running". In the core, set a `finalizing` flag around `finish_recording` and expose it in `status()`. Whether a toggle during the finalize should be refused is a product decision. Refusing would block a real new recording for up to a minute after a long one.

#### CORE-2 — A second launch destroys the previous session's log and opens audio capture and the DB
**Severity:** medium · **Verified:** confirmed (the Tauri and single-instance plugin order was checked upstream) · **Where:** `src-tauri/src/lib.rs:1193`, `:1201`, `:1225`; `src-tauri/src/logging.rs:59-74`; `src-tauri/src/state.rs:324-367`
**When** ClippiBoy is running and the user double-clicks the icon, opens a `clippiboy://` link (the Discord sign-in callback does this), or autostart runs twice, **then** the second process runs `logging::init()` and `AppState::new()` before the single-instance plugin exits it. It renames the live `clippiboy.log` over the previous session's `.log.1` and creates a stub `clippiboy.log`. The live session keeps writing into `.log.1`, and the next start moves the stub over it. The log of the session that had the problem is gone. The second process also loads the config, opens `clips.db`, runs the migration and starts the configured capture streams. The mic privacy indicator may flash (inferred).
**Fix:** Decide "am I the first instance?" with your own named mutex before `logging::init`, and keep the handle open. In a second process, log to stderr only. Do not try to fix this with share modes: they either block the rename or let `File::create` truncate the live log. Move `audio.apply(...)` out of `AppState::new()` into `setup`. That also removes the startup freeze in AUD-V3 (`docs/facecam-review.md`).

#### CORE-3 — An incomplete ffmpeg folder is never repaired
**Severity:** low (the trigger is external and rare, but the impact is total when it happens) · **Verified:** confirmed · **Where:** `src-tauri/src/tools.rs:166-168`, `:205`, `:290`; the back-off loop is at `:138-153`
**When** one of `ffmpeg.exe` and `ffprobe.exe` disappears from `%LOCALAPPDATA%\gg.clippiboy.app\ffmpeg\9.0.1\` (antivirus quarantine, a cleanup tool), **then** `provide()` downloads again and ends in `rename(staging, target)`. On Windows, a rename onto a non-empty directory always fails. The full archive is downloaded again at every back-off step, forever. Every clip fails with "not ready" unless ffmpeg is on PATH.
**Fix:** In `provide()`, after the presence check, run `remove_dir_all(target)` if the folder exists. Log which file was missing and tell the user. A missing `ffmpeg.exe` usually means antivirus.

#### CORE-4 — The handshake file outlives the app, and its token never changes
**Severity:** low · **Verified:** confirmed · **Where:** `src-tauri/src/control.rs:115-123` (`stop()`, called only from `restart`), `:435-451`; `src-tauri/src/tray.rs:139-149`; `src-tauri/src/lib.rs:1409-1412`; `src-tauri/src/updater.rs:77-84`
**When** ClippiBoy quits, updates or crashes, **then** `%APPDATA%\ClippiBoy\streamdeck.json` keeps the port and the token. The plugin keeps sending that token to that port about every 2 s. A process of another Windows user (fast user switching, a shared PC) that takes the port while ClippiBoy is down receives the token. The token is reused on the next start, so that process can then save clips and start recordings in the victim's session. A process of the same user gains nothing it could not already read.
**Fix:** Call `control::stop()` in `RunEvent::Exit` and in the updater's `on_before_exit`. A crash never clears the file, so the robust part is a fresh token on every `start()`. The handshake file hands the new token over anyway; only a manual entry in the property inspector would need re-entering. Do this together with CORE-5.

#### CORE-5 — The plugin sticks to a failed manual connection, and the handshake is written in place
**Severity:** low · **Verified:** confirmed (narrow trigger) · **Where:** `streamdeck/src/client.ts:183-185`, `:214-217`; `src-tauri/src/control.rs:469-470`
**When** the user once entered a manual port and token, and the handshake file was missing at a lookup, **then** the plugin caches the manual connection and never drops it after failures. The file is missing on a first run, with the control port off, during the ~100 ms inside `restart()`, or when a read lands during the truncate-then-write. Since 401a320 the real port moves whenever 47653 is taken (WSL). The keys then stay dead until the property inspector's Save button or a global-settings change. Opening the inspector does not reset it. This gets more common once CORE-4 clears the file on exit.
**Fix:** In the plugin's `catch`, always set `#connection = null`. The `#lookedUp` throttle still limits lookups. In `write_handoff`, write a `.tmp` file and rename it, as `config::save` does.

#### CORE-6 — The control port's "too slow" limit is per read, not per request
**Severity:** low · **Verified:** confirmed · **Where:** `src-tauri/src/control.rs:40-43`, `:182-186`, `:215-230`, `:255-262`
**When** a local process sends its request very slowly, **then** each read stays under the 2 s timeout, and one connection can live for hours until the 8 KiB header cap. No token is needed, because the token is checked after the read. Eight such connections fill `MAX_CONNECTIONS`, and the Stream Deck keys show "—" for hours. The test covers a stalled client, not a slow one.
**Fix:** Use one deadline per request (about 3 s). Wrap the stream so that each read gets only the time that is left, and skip zero durations.

#### CORE-7 — Drive-root clip folders from older versions are still granted to the webviews in full
**Severity:** low (defence in depth; it needs script execution in a webview first) · **Verified:** confirmed for the drive-root part; the claim that tokens were exposed was rejected · **Where:** `src-tauri/src/lib.rs:1134-1161`, `:1238-1239`; `src-tauri/src/commands.rs:304-307`; `src-tauri/tauri.conf.json:34`
**When** a config from before 8d18a92 has a whole drive as `clip_dir`, or library clips sit directly in a drive root, **then** `setup` grants the whole drive to the asset protocol, recursively, on every start. `allow_clip_dir` and `allow_existing_clip_dirs` skip the `usable_clip_dir` check that 8d18a92 added to `set_config` and `set_clip_dir`. UI-11 found the same thing. Related: CFG-5 in `docs/facecam-review.md` gets past the same check with `..`.
Two cleanups in the same place:
- The static `"$APPDATA/ClippiBoy/**"` entry resolves to `%APPDATA%\gg.clippiboy.app\ClippiBoy`, a folder nothing creates. It is dead and exposes nothing. Delete it.
- `"$VIDEO/**"` grants all of Videos, while the clip folder is granted at runtime anyway.

**Fix:** Put the `usable_clip_dir` check inside `allow_clip_dir`, so no caller can grant a drive root or an empty path. In `allow_existing_clip_dirs`, skip dirs whose `parent()` is `None`. Validate `clip_dir` in `config::load`. Remove the dead scope entry, and consider dropping `$VIDEO/**`.

#### CORE-8 — User overrides of built-in games in `games.json` apply at random
**Severity:** low · **Verified:** confirmed · **Where:** `src-tauri/src/game.rs:184-207`
**When** a user's `games.json` names a built-in exe in its real case, as Task Manager shows it, **then** both entries survive `extend`. They collide only when the keys are lowercased afterwards. `HashMap` iteration order is random per process, so the override applies on some starts and not on others. `README.md:513` says the file "is layered on top".
**Fix:** Lowercase both maps first, then `extend` with the user's entries.

#### CORE-9 — The auto-buffer can announce "Start" twice
**Severity:** low · **Verified:** plausible; it depends on `Pipeline::start` taking longer than one 2 s detection tick, which was not measured · **Where:** `src-tauri/src/lib.rs:352-372`, `:665-700`; `src-tauri/src/state.rs:512-531`, `:569-578`
**When** a game starts and bringing up capture and the encoder takes more than about 2 s, **then** the next tick still sees `buffer_active == false` and spawns a second start. The second start does nothing but returns `Ok`, so `buffer_started` plays the toast, the sound and the banner over the game again. A failed start is counted twice, so the back-off grows faster. The same race exists at app start, between `start_buffer_if_configured` and the first tick.
**Fix:** Let `start_pipeline` return whether it actually started anything, and call `buffer_started` only then. That fixes duplicate announcements on every route. Add an in-flight flag, checked before `poll*`, for the double failure count.

#### CORE-10 — Concurrent config saves share one temp file
Same as CFG-2 in `docs/facecam-review.md`.

#### CORE-11 — The startup sweep blocks the main thread inside `setup`
**Severity:** low · **Verified:** confirmed for the code; the cost depends on slow storage · **Where:** `src-tauri/src/lib.rs:1258-1270`; `src-tauri/src/filing.rs:117`, `:269-285`; `src-tauri/src/edit.rs:966-971`
**When** the clip folder is on a NAS or a mapped drive that is asleep or gone, or on an HDD during the boot autostart, **then** the four maintenance passes stat every clip on the main thread, under one library lock. Each locked file adds up to 600 ms of retry sleeps. Meanwhile the windows cannot paint, and the tray, the hotkeys and the buffer autostart wait. Most of the cost is SAVE-2 in `docs/facecam-review.md` (`edit::repair` is O(N²)). Fix that first.
**Fix:** In `filing::place()`, run the purely lexical `inside()` check before `is_file()`. Run the four passes on a background thread after `setup`, and take the library lock per clip.

#### CORE-12 — Uninstalling with "delete app data" leaves `%APPDATA%\ClippiBoy` behind
**Severity:** low · **Verified:** confirmed (the Tauri NSIS template was checked) · **Where:** `src-tauri/src/config.rs:10-13`; `src-tauri/tauri.conf.json:5`; `src-tauri/windows/hooks.nsh`
**When** the user uninstalls and ticks "delete app data", **then** the template removes only the `gg.clippiboy.app` folders. `%APPDATA%\ClippiBoy` stays. It holds `tracks/` and `originals/` (often GBs), thumbnails, `clips.db`, logs, `config.json`, and `friends.json` with the friends session token. The template already removes the autostart `Run` value.
**Fix:** Add `NSIS_HOOK_POSTUNINSTALL` with `RmDir /r "$APPDATA\ClippiBoy"`, guarded by `$DeleteAppDataCheckboxState = 1` and `$UpdateMode <> 1`. Never touch the clip folder. A clip folder that the user placed inside `%APPDATA%\ClippiBoy` would be deleted too, so check for that first.

#### CORE-13 — The recording-failure path can launch the stop twice
Same as REC-8 in `docs/facecam-review.md`.

## Friends, sharing, links and the server (NET-n)

A note on the quota findings (NET-4 to NET-7). Their impact assumes the Cloudflare Free plan. Code comments suggest that plan, but the reviewers could not confirm it. On Free, any script can also use up the 100k requests a day with plain requests. Only a Cloudflare WAF rate-limit rule (Free includes one) or the Paid plan fences that off. The per-route fixes are still worth doing, because each of these vectors is cheaper than that floor.

#### NET-1 — "Appear offline" can be seen through
**Severity:** medium (a privacy feature is defeated) · **Verified:** confirmed; the reviewer's fix would break sending while invisible · **Where:** `server/src/hub.ts:94-97`, `:161-166`, `:259-266`; `src-tauri/src/share.rs:500-507`, `:560-562`, `:580-582`
**When** a user has "Appear offline" on and a friend with a modified client sends them a harmless relay message, **then** the server answers "offline" only when no app of theirs is connected. `relay()` delivers to any ready socket and never looks at `invisible`. The victim's app drops the message silently, so the friend can log when the "invisible" user is online. The victim's app also declines some offers automatically, which gives the same signal. The privacy page says friends see you as offline.
**Fix:** Do not just add `if (this.current() === null) return false;`. An invisible user can still send clips, and the receiver's decline and cancel come back through `relay()`. With that line, a decline would show as "Expired", and a failure would leave the sender's card at "Moving" with its secret still valid. Instead:
- In the invisible user's hub, remember the peers that user relayed to (for about 10 minutes, in `ctx.storage`, because memory is lost on hibernation).
- While invisible, answer relays from anyone not on that list as if offline.
- On the client, while invisible, ignore offers silently instead of declining them.

#### NET-2 — Deleting the account resets the weekly link limit and drops a pending ban
**Severity:** low · **Verified:** confirmed in part; the "fills the bucket" and "unlimited hosting" parts are wrong · **Where:** `server/src/index.ts:415-423`; `server/migrations/0002_shares.sql:5`; `server/src/db.ts:107-117`; `server/src/links.ts:57-58`, `:91-97`
**When** a user makes 3 links, deletes the account and signs in again with the same Discord account, **then** the new account starts at 0/3. The cascade has removed the share rows, including the tombstones that should count for a week (`shares.ts:9-10` states that rule). A ban that was not yet applied is gone too, and reports point at a deleted share. Storage does not pile up, because the account's files leave R2 with it. A ban that is already set cannot be dodged this way.
**Fix:** Keep abuse state on a peppered hash of the Discord id, somewhere account deletion does not touch. For example a `deleted_accounts` row with the link times (kept 7–14 days) and a `bans` table checked in `authCallback`. Add one sentence to the privacy page, which today says that deletion removes everything immediately.

#### NET-3 — Muted tracks still go to the friend, raw
**Severity:** medium (privacy, often of other people) · **Verified:** confirmed · **Where:** `src-tauri/src/share.rs:307-319`, `:333-342`, `:1142-1167`
**When** a clip has separate game, mic and Discord stems, and the user mutes the Discord track or their own mic and sends the clip to a friend, **then** the friend receives every raw stem plus a mix that opens muted. One click in the friend's mixer plays the muted voices. The mixed clip file itself is fine; the extra stems are the surprise. Neither the send dialog nor the privacy page mentions separate tracks.
**Fix:** Do not cut or offer tracks that are muted or below `SILENT_DB`, and re-index `mix`, because the receiver stores stems by slot `0..n`. Or add a "Send separate tracks" switch that is off whenever a track is muted, and say so in the dialog.

#### NET-4 — The hub has no presence throttle, a relay budget per socket, and no socket cap
**Severity:** medium · **Verified:** confirmed in the code; the quota impact is plausible and depends on the plan · **Where:** `server/src/hub.ts:41-43`, `:63-81`, `:84-101`, `:222-241`, `:250-256`; `src-tauri/src/share.rs:609-622`; `src-tauri/src/friends.rs:559-563`
**When** a signed-in client keeps changing its presence, **then** every change costs a Durable Object storage write, a D1 read and one RPC per friend, and nothing limits the rate. Even with no friends, about 100k changes use up the account's daily DO row writes, and presence then breaks for every user.
**When** a malicious friend floods clip offers, **then** the relay limit does not hold, because it is per socket and sockets are not capped. With the default `accept_clips = All`, each offer plays a sound and puts a banner over the victim's game, ignoring "notify while playing". Switching `game` back and forth does the same with "X started <game>" alerts when the victim is not in a game.
**Fix:**
- One relay budget per hub (the hub is the user), not per socket.
- Cap the sockets per hub (about 4; close the oldest).
- Publish presence at most once a second, and limit `hello` the same way.
- Drop raw messages over 8 KB before `JSON.parse`; a DO WebSocket message can be 32 MiB.
- On the client, keep one open offer per friend, and drop further offers while one is still asking.

#### NET-5 — Signed-in routes have no limits, and strangers can repeat friend-request alerts
**Severity:** low–medium · **Verified:** confirmed · **Where:** `server/src/index.ts:116-122`, `:345-377`, `:392-397`, `:399-413`; `src-tauri/src/friends.rs:358-367`, `:568-571`
**When** a stranger who knows a Discord username sends and withdraws a request over and over, **then** the victim's app sees a "new" request each round and raises "X sent you a friend request" again (held back in a game by default). Blocking stops it until the attacker uses another Discord account.
`removeFriend` and `blockUser` notify the other user even when nothing changed. So even a blocked user can keep forcing silent snapshot rebuilds and refetches on the victim. Mutating routes have no per-user limit, which spends D1 writes, though not much faster than plain requests do.
**Fix:**
- A rate-limit binding keyed on the user for every mutating route (for example 30 a minute), and a daily cap on new outgoing requests.
- A cooldown before re-requesting someone you just withdrew from.
- Notify only when a row was actually deleted (`meta.changes > 0`).
- Write `PATCH /me` only when the value differs.
- On the client, alert once per requester per day.

#### NET-6 — The anonymous write limits are too loose, and IPv6 makes them looser
**Severity:** medium · **Verified:** confirmed in the code; the quota impact is plausible (plan) · **Where:** `server/src/index.ts:153-156`; `server/src/links.ts:323-324`; `server/src/telemetry.ts:9`, `:69`; `server/site-proxy/index.ts:68-76`
**When** someone scripts `/t/ping` with new install ids, or `/auth/start`, **then** each request writes rows (installs, install days, logins, plus index rows) at up to 10 a minute per address. One or two plain IPv4 addresses can use up D1's free 100k row writes a day. The key is the full IPv6 address, so anyone with an IPv6 prefix gets a fresh budget per address. The site proxy also creates a `downloads` row per day for any made-up ref. Once D1 writes run out, sign-in, links and the daily session update fail with 500 for everyone, and the usage statistics fill up with fake installs.
**Fix:** The essential part is a global daily ceiling on anonymous writes (a counter row or a small DO), or a Cloudflare WAF rate-limit rule on `/t/ping` and `/auth/start`. Keying IPv6 on the /64 is secondary. Count downloads only for refs on an allowlist.

#### NET-7 — Unauthenticated routes read request bodies with no size limit
**Severity:** low (hardening) · **Verified:** plausible; the reviewer's "takes the Worker down for everyone" is contradicted by Cloudflare's docs · **Where:** `server/src/telemetry.ts:85-86`; `server/src/links.ts:329`; `server/src/util.ts:16-22`; `server/src/index.ts:77`, `:244`
**When** a large body arrives at `/t/ping`, `/c/<id>/report`, `/auth/exchange` or another route that uses `body()`, **then** it is buffered in full. The ping's size check passes when `Content-Length` is missing. `/auth/exchange` has no rate limit at all. In practice mostly the sender's own request fails, but under sustained parallel load isolates keep being replaced.
**Fix:** Add one `readLimited(request, max)` helper. It refuses a missing or too-large `Content-Length` and stops reading past `max`. Use it in `body()`, `handlePing` and `reportShare` with 1–4 KB. Put `/auth/exchange` under `underLimit(..., "auth")`.

#### NET-8 — A received clip's "from <friend>" tag is published on public link pages
**Severity:** low · **Verified:** confirmed · **Where:** `src-tauri/src/share.rs:896`; `src-tauri/src/links.rs:544-548`; `src-tauri/src/clips.rs:347-351`
**When** the user shares a clip that a friend sent them as a public link, **then** the page shows a "from Luca" pill with the friend's Discord display name, even with "Show my name on share links" off. `is_friend_tag` only knows `"with "`. The comment at `links.rs:544` states the rule this breaks.
**Fix:** Treat `"from "` as a friend tag too. Better, mark the tags the app generates, so renamed ones are caught. Add a test next to `a_friend_tag_is_known_in_any_case`.

#### NET-9 — Transfers leave files behind and share one part-file name
**Severity:** low · **Verified:** confirmed · **Where:** `src-tauri/src/share.rs:329-331`, `:346-349`, `:671-675`, `:810`, `:1043-1045`, `:1143`, `:1206-1224`; `src-tauri/src/filing.rs:247-265`
**When** the app quits or crashes mid-receive, **then** `<name>.mp4.part` stays in the user's clip folder (up to 64 GB), and nothing removes it. A later receive of a different file with the same name uses that leftover's length as its resume offset. It fails the hash once, and only the resend works. Two receives with the same name at the same time write through each other.
**When** a send is declined, expires, fails or is cancelled, **then** the cut raw stems (mic, Discord) stay in `%TEMP%\clippiboy-share\<id>\`. A successful send leaves the empty folder. An offered `CON.mp4` makes the transfer fail on Windows 10.
**Fix:** Name the part after the offer id, and pick `free_name` right before the rename. At startup, delete ClippiBoy's stray `.part` files and clear `%TEMP%\clippiboy-share`. Delete the outgoing tracks when a transfer reaches a final state, and on the two early returns. Reuse the `RESERVED` check from `filing.rs` in `safe_file_name`.

#### NET-10 — A stalled peer hangs a transfer, and Cancel cannot interrupt it
**Severity:** low · **Verified:** confirmed for a malicious peer; whether an honest stalled app does the same depends on iroh's keep-alive defaults · **Where:** `src-tauri/src/share.rs:813-820`, `:831-839`, `:948-956`, `:1001`
**When** the sending friend's app keeps the QUIC connection alive but stops sending, **then** the receive task waits forever. Any packet from the peer resets QUIC's idle timer, and the cancel flag is checked only between reads. Cancel flips the card, but the task, the connection and the locked `.part` handle stay until the app quits. The same holds for a peer that connects to the sender and never sends its ask.
**Fix:** Race every network await against a cancel `Notify` and an idle timeout (for example 30 s since the last byte) with `tokio::select!`. Return an error, so the existing retry and resume take over. Put a 10 s timeout on `accept_bi`, the ask read, `open_bi` and the header read.

#### NET-11 — "Open the browser again" breaks sign-in, and a stray auth link spends the verifier
**Severity:** low (UX) · **Verified:** confirmed; the main mechanism is the cookie, not the verifier · **Where:** `src-tauri/src/friends.rs:146-164`, `:192-204`, `:289-291`; `server/src/index.ts:199-201`, `:277`; `src/routes/Friends.tsx:62`
**When** the user clicks "Open the browser again" while the first tab is still at Discord, **then** the second `/auth/start` overwrites the `cb_login` cookie, and the first tab fails the state check. `page()` clears the cookie on every response, so the second tab then fails too. If the first tab had already reached the browser's "Open ClippiBoy?" prompt, the new verifier is spent instead. Any `clippiboy://auth` link opened during a sign-in also spends the verifier.
**Fix:** This needs a server change: clear the cookie only on success, or keep the last few states in it. On the client, reuse a pending verifier, consume it only after a successful exchange, and ignore auth links when no sign-in is pending.

#### NET-12 — A stale username can send a friend request to the wrong account
**Severity:** low · **Verified:** confirmed (mechanism); how often it happens is unknown · **Where:** `server/migrations/0001_init.sql:7`, `:15`; `server/src/db.ts:45-47`, `:100-105`; `server/src/index.ts:349`
**When** someone gives up a Discord username and another user later takes it and signs in, **then** both rows hold the name until the first user signs in again. The index is not unique, and the lookup usually returns the older, stale account. The new owner of the name cannot be found by name. The requester does see the target's name and picture, so a mix-up can be noticed.
**Fix:** On login, clear the name from other rows (`... WHERE username = ? AND discord_id != ?`). Clean up the existing duplicates, then add a unique NOCASE partial index on non-empty names. Answer 404 when a name is ambiguous.

#### NET-V1 — Old multi-track clips go onto public links with their raw source tracks
**Severity:** low–medium (privacy, conditional) · **Verified:** found by the second reviewer · **Where:** `src-tauri/src/links.rs:443-447`, `:504-511`; `src-tauri/src/stems.rs:5-8`, `:154-157`
**When** a clip from before the stems change, which still carries the extra audio tracks in its MP4, is 50 MB or smaller and is shared as a link, **then** `faststart` copies every stream (`-map 0 -c copy`), and the fallback uploads the source unchanged. The page plays track 0, but anyone who downloads the `.mp4` gets the mic and Discord tracks. Those may not be in the mix at all. Clips over 50 MB go through `shrink` and are probably fine.
**Fix:** In `faststart`, map only `0:v:0` and `0:a:0?`, and drop metadata and chapters (`-map_metadata -1 -map_chapters -1`, new: nothing in the core uses them yet); the stream mapping matches `export.rs:138`. Send the fallback through the same mapping, or refuse instead of uploading the raw source.

#### NET-V2 — Turning friend requests off confirms that you use ClippiBoy
**Severity:** low (privacy; partly a design choice) · **Verified:** found by the second reviewer · **Where:** `server/src/index.ts:369`
**When** a user has requests turned off and someone types their Discord username, **then** the server answers 403 "They don't take friend requests right now". That confirms the account exists, and it does so for exactly the users who opted out of contact. A block, by contrast, is deliberately answered as "does not exist".
**Fix:** Answer requests-off with the same 404 as a block, or answer both with a neutral "Request not possible".

## App UI (UI-n)

#### UI-1 — Console audio changes are reverted by the main window
Same bug as CFG-1 in `docs/facecam-review.md`. One addition: the main mixer's own row controls (M, S, ⧉, enable, gain) send the whole row built from the stale copy. A click on any of them on the row the console changed reverts that row too (`src/routes/AudioMixer.tsx:390`, `:403`, `:412`, `:422`, `:435`).

#### UI-2 — There is no error boundary, and reload is blocked
**Severity:** low–medium (high impact if it happens; no reachable trigger is known) · **Verified:** confirmed (mechanism) · **Where:** `src/main.tsx:11-15`; `src/overlay/main.tsx:12-16`; `src/console/main.tsx`; `src/lib/lockdown.ts:17`, `:38`, `:41`
**When** any render throws, **then** React unmounts the whole root and the window goes blank. Examples are an unexpected payload shape, or a hook-order slip like the latent one at `ShotViewer.tsx:434`, which is unreachable today. F5, Ctrl+R and Ctrl+Shift+R are blocked, and hiding to the tray does not reload. The only way out is Quit from the tray, in an app that runs for days. The overlay and the console are affected the same way.
**Fix:** Wrap each route, the viewers, `<Overlay/>` and the console in a small error boundary with "Reload window" (`location.reload()` is not blocked) and "Back to Overview". Pass `onUncaughtError` and `onCaughtError` to `createRoot`, and send the errors to the core log. Add an `unhandledrejection` handler that logs the same way.

#### UI-3 — The context menu does not own its keys
**Severity:** medium · **Verified:** confirmed; worse than reported, because of UI-V2 · **Where:** `src/components/ui/Menu.tsx:156-182`; `src/components/ShotViewer.tsx:384-399`; `src/components/ClipPlayer.tsx:469-513`
**When** a context menu is open in the screenshot viewer or the player and the user presses Escape to close it, **then** the event goes on and closes the whole viewer or player as well. In the player, that throws away unsaved trim and mix (UI-V2). ↑ and ↓ in the menu also change the player volume by 10 % per press. The same leak hits `ExportDialog`'s Escape when a text menu is open over its number field.
**Fix:** In `Menu.tsx`'s capture-phase handler, call `stopPropagation()` for the keys the menu consumed. For Enter, do it only when an item is active. Add `if (event.defaultPrevented) return;` at the top of both viewer handlers.

#### UI-4 — Opening the Video page while the chosen screen is absent lowers resolution and fps for good
**Severity:** medium · **Verified:** confirmed, with a wider trigger · **Where:** `src/routes/Recording.tsx:117-119`, `:159-167`, `:189-203`
**When** the user records a 2560×1440 @ 165 Hz secondary monitor, unplugs it (or the target game window is closed) and opens the Video page, **then** the page falls back to the primary screen, and its auto-fit effect saves 1920×1080 @ 60. When the monitor returns, every clip stays worse, silently. The banner on that page says the fallback lasts only "until it is back".
The first render also uses the target list from app start. A game window opened since then counts as missing, and the downgrade is saved before the refreshed list arrives. The core already fits to the real source at capture time, so the write is not needed.
**Fix:** Add `if (!source || missing) return;` at the top of the effect. That covers the stale-list case too, because `missing` is true until the refresh lands.

#### UI-5 — Every `clips-changed` triggers a full `list_clips`, with no coalescing
**Severity:** medium for large libraries · **Verified:** confirmed (mechanism; the cost was not measured) · **Where:** `src/store.ts:311-319`, `:585-593`; `src-tauri/src/commands.rs:735-738`, `:839`, `:1014`; `src-tauri/src/clips.rs:145`
**When** the user removes a misdetected game chip that covers 200 clips, **then** `clearGame` causes two `clips-changed` per clip, because `file_clip` announces even when nothing moved. That is 400 full `list_clips` reads on the main thread, each with one or two stats per clip, and other sync commands queue behind them. Answers that land mid-loop put the game back on the clips not yet processed, so the chip flickers back with a falling count.
**Fix:** Port the console's coalescing (`src/console/Console.tsx:258-288`; its comment names `clearGame`): at most one read in flight, plus one trailing read. Make `list_clips` `#[tauri::command(async)]`. Give `clearGame` a bulk core command that emits once. SAVE-2 in `docs/facecam-review.md` covers the per-row cost.

#### UI-6 — The mixer label and gain write the whole config on every keystroke or step
**Severity:** low–medium · **Verified:** confirmed for the writes; the lost characters are plausible (they need an IPC round trip longer than the gap between keystrokes) · **Where:** `src/routes/AudioMixer.tsx:336`, `:390`; `src/store.ts:426-439`; `src/routes/Recording.tsx:438-454`
The per-step main-thread cost is CFG-11 in `docs/facecam-review.md`, whose verifier already names these two controls. What this adds: `upsertSource` has no ordering guard. **When** the core answers slower than the user types, **then** an older answer replaces the newer text, the next keystroke builds on it, and characters are lost. The gain slider can jitter the same way. `upsertSource` also has no catch, so a failure is an unhandled rejection and the optimistic value stays. The clip-length slider has a ticket and cannot snap back; it only costs writes.
**Fix:** For the label, keep a local draft and save it on blur or Enter, as `NameField` in `Clips.tsx` does. For gain and clip length, show the draft during the drag and write in `onCommit`; the `Slider` supports it. Optionally, add a per-source ticket in `upsertSource`.

#### UI-7 — The overlay's REC dot animates for the whole recording
**Severity:** low · **Verified:** plausible; the CPU cost has to be measured · **Where:** `src/overlay/overlay.css:320-328`; `src/overlay/Overlay.tsx:124-136`; `src-tauri/src/overlay.rs:52-58`
**When** a manual recording runs (it can run for an hour), **then** the badge's infinite CSS animation makes Chromium produce frames continuously. The window renders on the CPU (`--disable-gpu`) and is never throttled. `accent.ts:122-127` avoids exactly this cost for the RGB hue.
**Fix:** First measure the WebView2 CPU during a recording. If it matters, drive the blink from the once-a-second status tick: `opacity: Math.floor(recording) % 2 ? 0.35 : 1`. `recording` is in float seconds, so the reviewer's `recording % 2` would leave the dot dim.

#### UI-8 — Errors from store actions show only on Settings, and some actions fail silently
**Severity:** low · **Verified:** confirmed, with corrections · **Where:** `src/store.ts:191`, `:631-634`; `src/routes/settings/index.tsx:80-84`; `src-tauri/src/commands.rs:531-542`, `:864-870`; `src/routes/Clips.tsx:594`, `:602`; `src/components/clipMenu.tsx:136`, `:170`, `:180`, `:209`, `:216`
**When** a screenshot save fails, "Free up space" fails on a tile, or "Copy clip", "Copy picture", "Open in default player", "Show in folder" or "Copy path" fails, **then** the user sees nothing where they acted. The store's `lastError` shows only on the Settings page, and it is never cleared, so a stale error sits there for the rest of the session. The clip-menu actions are fire-and-forget. `copy_clip_file` can return "The clip file is no longer there." and nothing shows; success is not confirmed either, which matters for clipboard actions. The failed delete is EDIT-15 in `docs/facecam-review.md`.
**Fix:** Export a small `toast()` that pushes into the existing `Toasts` list. Use it from the store's catch blocks and for the clip-menu actions, on success and on error. Clear `lastError` when the next action of that kind succeeds, or give the card a dismiss button.

#### UI-9 — The screenshot viewer's shortcuts stop working after a slider is touched
**Severity:** low · **Verified:** confirmed · **Where:** `src/components/ShotViewer.tsx:386-388`
**When** the user drags the size slider or a ColorPicker range in draw mode, **then** focus stays on the `<input type="range">`, and Escape, Delete/Backspace and Ctrl+Z do nothing. The guard treats every `INPUT` as a text field. Drawing on the stage does not move the focus, so the shortcuts stay dead until a focusable element is clicked. `ClipPlayer.tsx:471-479` already fixed the same thing ("Sliders are not text").
**Fix:** Skip only text inputs: `(t instanceof HTMLInputElement && t.type !== "range") || t instanceof HTMLTextAreaElement`.

#### UI-10 — A custom export size with decimals fails silently
Same as EDIT-11 in `docs/facecam-review.md`. Rounding inside `api.exportClip` (`src/lib/ipc.ts:202-203`) protects every caller. UI-V1 covers the missing error message. Also, `screenshotHasOriginal` (`src/lib/ipc.ts:151-152`) calls a command that no longer exists, and nothing calls it. Delete it.

#### UI-11 — The asset protocol scope
Same as CORE-7 above.

#### UI-12 — Writes before `init()` finishes can send the mock config to the core
**Severity:** low · **Verified:** confirmed (mechanism), with corrections · **Where:** `src/store.ts:167`, `:266`, `:371-375`, `:390-408`, `:426-445`
The `getConfig`-failure route is CFG-9 in `docs/facecam-review.md`. This adds a second trigger. The store starts from `mock.mockConfig` in Tauri too, and `patchConfig`, `upsertSource`, `removeSource` and `setHotkeys` never check `ready`. **When** the user clicks a mixer toggle or changes a setting in the short init window, **then** mock sources, the mock clip folder and the mock hotkeys reach the real config on disk. A mixer click adds a mock source. No automatic writer fires before `ready`. The Stream Deck pairing does not break right away, because a token change through `set_config` does not restart the port. From the next start on, though, the control token is the repo's mock value, as CFG-9 says.
**Fix:** Start from a neutral config in Tauri. Add `if (inTauri && !get().ready) return;` to the write actions. Do not fall back to the mock for `getConfig`.

#### UI-V1 — Every export failure is silent
**Severity:** medium · **Verified:** found by the second reviewer · **Where:** `src/components/ExportDialog.tsx:110-114`; `src-tauri/src/commands.rs:320-322`, `:1148-1150`, `:1229-1263`
**When** an export fails for any reason, **then** the dialog stops being busy and nothing says why. Examples: an export to a network share (8d18a92 refuses it on purpose), a clip file that is gone, an ffmpeg or encoder failure, an empty output, the decimal size from EDIT-11. The catch assumes that "the core has already raised the failure as a notice". But `export_clip` notifies only on success, and `shrink` is documented as working "without its notices". EDIT-11 covers only the decimal case.
**Fix:** Show the error in the dialog (`catch (err) { setError(String(err)); setBusy(false); }`), or have `export_clip` call `notify(&app, "error", …)` on `Err`. Fix the comment either way.

#### UI-V2 — The player discards unsaved trim and mix on any close
**Severity:** low–medium · **Verified:** found by the second reviewer; the mechanism is certain, and it is probably unintended · **Where:** `src/components/ClipPlayer.tsx:171-205`, `:551-557`; callers at `:274`, `:504-509`, `:669`, `:701`
**When** the user sets in and out points or rebalances the tracks, and then presses Escape (perhaps only to close a context menu, UI-3) or clicks the dimmed backdrop or the X, **then** the player closes and the edits are gone. `close()` checks only `writing`, never `dirty`. `ShotViewer` deliberately asks first, and its comment calls the alternative "the very loss this is meant to prevent" (`ShotViewer.tsx:425-429`).
**Fix:** When `dirty` is set, ask "Discard changes?" with the existing confirm prompt, as `ShotViewer` does. At least ignore backdrop clicks while dirty. If discarding is intended, say so in a comment.

Also noticed: a clip file that cannot be deleted is left on disk while its row goes. That is SAVE-6 in `docs/facecam-review.md`.

## Improvement ideas (not bugs)

**Core and Stream Deck**
- **Shutdown and logoff.** Handle `WM_QUERYENDSESSION`/`WM_ENDSESSION` and run the Quit path; nothing handles them today. Where: a small subclass on the main window. Benefit: a Windows restart during a recording keeps the unflushed tail and clears the handshake. `recover_recordings` already finishes the rest.
- **Quit or update during a save.** Same as the Quit half of SAVE-3 in `docs/facecam-review.md`. Call `control::stop()` (CORE-4) at the same point.
- **Long exe paths.** Use a 32 768-unit buffer in `exe_name`. Where: `src-tauri/src/game.rs:408`. Benefit: detection and game audio keep working for install paths over 259 characters (rare).
- **Own-window titles.** Check `pid == std::process::id()` before fetching the window title. Where: `src-tauri/src/game.rs:480-493`. Benefit: one main-thread round trip less per tick. Small on its own, because `window_awake` already makes such round trips every 0.5 s.
- **Explicit buffer start/stop over the API.** Run the toggle's manual-start/stop bookkeeping in the `"start"`/`"stop"` branches. Where: `src-tauri/src/control.rs:359-360`. Benefit: in always-on mode, an API "stop" is no longer undone 2 s later. Only third-party callers hit this; the plugin sends "toggle".
- **Clipboard read bound.** Bound the read by `GlobalSize/2`. Where: `src-tauri/src/clipboard.rs:165-173`. Benefit: malformed clipboard text from another app cannot cause an access violation.
- **Accent parsing.** Check that the bytes are ASCII hex, or run `clean_accent` in `config::load`. Where: `src-tauri/src/tint.rs:30-37`. Benefit: a hand-edited non-ASCII accent no longer crashes the app on every start (`panic = "abort"`).
- **Bulky data in Roaming AppData.** Move tracks, originals, thumbnails and preview to `%LOCALAPPDATA%`, with a one-time move. Where: `src-tauri/src/stems.rs:39`, `edit.rs:56`, `thumbs.rs:17`, `preview.rs:16`. Benefit: no multi-GB profile sync on roaming profiles (mostly an enterprise setup) and a cleaner uninstall.
- **OneDrive.** When the clip folder is under `%OneDrive%`, suggest a local folder, or keep `.recording` outside it on the same volume. Where: `src-tauri/src/config.rs:19-23`, `recorder.rs:148`, `filing.rs:280`. Benefit: no syncing and no "Is it open right now?" lock errors during recordings, for users who include Videos in OneDrive backup (Videos is opt-in, not a default).
- **Log size cap.** Rotate at, for example, 20 MB. Where: `src-tauri/src/logging.rs:59-74`; the health line is at `lib.rs:1095-1104`. Benefit: bounded logs for an app that runs for days. Today that is about 3 MB a day, and far more with the per-frame warning in BUF-2 (`docs/facecam-review.md`).
- **`free_name` after 999 collisions.** Return `Err` instead of the existing target. Where: `src-tauri/src/filing.rs:264`. Benefit: keeps the "nothing gets overwritten" promise. Practically unreachable today.
- **Size caps on the ffmpeg download and archive entries.** Where: `src-tauri/src/tools.rs:235-247`, `:270-283`. Benefit: a bad archive cannot fill the disk before the hash check fails.
- **Testability.** Extract a small `Actions` trait from `control::handle`, and split `spawn_ui_updates` into named periodic jobs. Where: `src-tauri/src/control.rs`, `lib.rs:968-1129`. Benefit: unit tests for auth, routing and toggle semantics without Tauri, and uniform in-flight guards (CORE-9, REC-8).

**Friends, sharing and the server**
- **Old e2e test accounts.** The earlier version of `server/scripts/e2e.mjs` (d1fc89c, d936cef) seeded two accounts with fixed bearer tokens in this public repo, and its instructions said `--remote`. Sessions expire only after 180 days idle. Check production D1 for those users and their sessions, and delete them; their friend codes are public too. Benefit: closes a possible public sign-in on production.
- **Bind a transfer to the receiver's iroh identity.** Put the receiver's `EndpointId` in `ShareAnswer`, check `connection.remote_id()` in `share::serve`, and expire an outgoing offer that sits idle in `Moving` (`share.rs:965`). Benefit: a leaked offer secret (logs, a read-only compromise of the relay path) no longer lets someone else pull the clip. It does not protect against an actively malicious server.
- **Session token at rest.** Protect `friends.json` with DPAPI or the Windows Credential Manager. Consider an absolute session lifetime and "sign out everywhere". Where: `src-tauri/src/friends.rs:844-848`. Benefit: the token is no longer plain text in a file.
- **Reconnect jitter.** Add ±50 % jitter and a random delay before the first retry. Where: `src-tauri/src/friends.rs:38`, `:405-409`. Benefit: after a deploy, clients no longer all come back in lockstep at about 1 s.
- **Blocking work on the async runtime.** Move `file_clip`'s ffmpeg and SQLite work and the per-chunk `std::fs` I/O in `serve`/`fetch` to `spawn_blocking` or `tokio::fs`. Spawn the `refresh()` that `friends::serve` awaits inside its `select!`. Where: `src-tauri/src/share.rs:857-904`; `friends.rs`. Benefit: pings and the outbox no longer stall for up to the 20 s HTTP timeout.
- **Harden ffmpeg against media from friends.** Pass `-protocol_whitelist file` and an explicit demuxer per extension for thumbnails and the mixer, and keep the bundled ffmpeg current. Benefit: a smaller attack surface for crafted files.
- **Takedowns and caching.** Send `private`, or `no-cache` with the existing ETag. Where: `server/src/links.ts:399-400`. Benefit: a deleted or banned clip leaves browser and third-party caches at once. Cloudflare's own cache is not involved.
- **Suspensions.** Add `suspension_applied_at` and process only new bans. Where: `server/src/links.ts:428-443`. Benefit: no hourly re-processing of every banned account.
- **Privacy page.** Say plainly that a friend you send a clip to receives your device's IP addresses, LAN addresses included. Today the page only says "the connection details of both devices". Where: `site/privacy.html:96`. Benefit: accurate disclosure.

**App UI**
- **Config revisions.** The core stamps every `AppConfig` it returns or emits with a rising `revision`. The store keeps only the highest, and `set_config` rejects or merges older writes. Benefit: closes the CFG-1/UI-1 class for good and makes broadcasting `config-changed` after every write safe.
- **Gallery at scale.** Keep unchanged clip objects across re-reads, memoize the tile component, window the grid beyond about 500 clips, and build the search haystack once per `clips` change. Where: `src/routes/Clips.tsx:199-225`. Benefit: one rename no longer re-renders hundreds of tiles.
- **`init()` hygiene.** Register the event listeners before the initial reads, and make `init()` idempotent. Where: `src/store.ts:264-275`. Benefit: no lost `clip-saved` or `config-changed` in the gap, and no doubled listeners under StrictMode in dev.
- **Accessibility.** Give the Dashboard's "Latest clips" cards `role="button"`, `tabIndex` and key handling like `TargetCard`, or make them `<button>`s (`src/routes/Dashboard.tsx:250-257`). Show the tile buttons on `group-focus-within` (`Clips.tsx:560`, `:573`). Add an `aria-label` to the mixer label input (`AudioMixer.tsx:334-340`). Benefit: keyboard and screen-reader use.
- **Binary IPC for screenshot layers.** Send the PNG layers as a raw body or base64, not as a `number[]` in JSON. The layers are nested in `steps`, so the command needs restructuring. Where: `src/components/ShotViewer.tsx:1665-1682`. Benefit: about 3.6× fewer bytes on big pictures with many strokes.
- **Hidden-window timers.** Start the accent interval only in RGB mode (in solid mode it runs every 80 ms and does nothing), and stop a meter's interval once its level is 0. Where: `src/lib/accent.ts:157`; `src/components/ui/Controls.tsx:181-187`. Benefit: small; fewer timer wake-ups.

## Checked and fine

**Core and Stream Deck**
- **Control port against browsers:** the custom token header forces a CORS preflight, `OPTIONS` gets 403, and requests without the token are refused. That covers DNS rebinding and CSRF. The `Origin`/`Referer`/`Sec-Fetch-Site` refusal is an extra layer. The port binds to loopback only, and the token is compared in constant time.
- **Updater:** the minisign public key is pinned in `tauri.conf.json`, the endpoint is HTTPS, and downgrades are refused. `ffmpeg-*` releases are created with `--latest=false`, so `latest.json` stays reachable.
- **ffmpeg download:** a SHA-256 is pinned per binary. Files are staged and moved into place only after the hash check, and a partial download never counts as ready.
- **Asset scope:** the static `$APPDATA/ClippiBoy/**` entry does not expose `friends.json` or `config.json` (see CORE-7).
- **Uninstall:** the Tauri template already removes the autostart `Run` value.
- **Game detection:** each 2 s tick is a few cheap Win32 calls, and every handle is closed on every path.
- **Win32 resources:** `tray::big_icon` destroys the old `HICON`, `clipboard::put` frees the `HGLOBAL` only on failure, and `overlay::to_top` uses `SWP_ASYNCWINDOWPOS`.
- **Banner state machine, `set_logo_icon`, watermark maths:** consistent; the input size is checked, and nothing underflows.
- **Lock scopes in `lib.rs`:** no guard is held across `overlay::show` or a main-thread wait.

**Friends, sharing and the server**
- **Sign-in:** a PKCE-style verifier and challenge, a state cookie bound to the browser, and a one-time code. Login CSRF, session fixation and "finish my half-done login" do not work.
- **SQL:** every query is prepared and bound, and the interpolated fragments are constants.
- **Access control:** shares go through `owned()`. Friend, accept, block and unblock are keyed on the caller, and the poster upload checks the owner. No cross-user access was found.
- **Share page:** every user-controlled field is escaped, and width and height are clamped integers.
- **Link ids:** about 49 bits. Expired, deleted and unknown ids all return the same 410, so there is no existence oracle.
- **Received file and folder names:** no path traversal (`safe_file_name`, `filing::folder_name`).
- **Offer sizes:** `checked_add`, the per-track caps and the free-space margin hold.
- **Relay and ids:** only friends can relay to each other (checked on the server), and `checked_id` guards the ids that go into URL paths.
- **Client address:** an outside party cannot set `CF-Connecting-IP`.
- **Panics:** no panic paths in the reviewed async code. That matters with `panic = "abort"`.

**App UI**
- **XSS:** no `dangerouslySetInnerHTML`, `innerHTML` or `eval` in `src/`. Clip names, tags, friend names and banner titles render as React text.
- **CSP:** `default-src 'self'` with Tauri's nonces, no script gaps, and no object URLs.
- **Event listeners:** component-level `listen` calls use the cancelled-flag pattern correctly (`App.tsx`, `Toasts.tsx`, `Overlay.tsx`).
- **Levels:** audio levels are scoped to `SourceMeter`, and the core stops sending them to a hidden window.
- **Config writes:** `patchConfig` drops stale answers with its ticket.
- **Types:** `types.ts` matches `model.rs` (interfaces, enum casing, command argument keys, event names). The only runtime mismatch is the export size (EDIT-11).
