# Fixes Beyond the Facecam Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the review's most valuable findings outside the facecam. Sharing should send only what the user chose. The friends server should stop leaking "Appear offline" and stop letting anyone spend its daily quota. The Stream Deck should stop showing ⚠ for work that succeeded. A second start, a broken ffmpeg folder and a few UI paths should stop losing data. Each fix comes with a test that pins it.

**Architecture:** Each fix goes where the fault is, and no new subsystem is added. Decisions that a test can reach move into small pure modules that import nothing but types. On the server these are `server/src/hubrules.ts`, `limits.ts` and `login.ts`, which `node --test` loads the way it loads `telemetry.ts`. The plugin gets `streamdeck/src/timing.ts` and `labels.ts`, and the UI gets `src/lib/fit.ts`, `viewer.ts` and `coalesce.ts`, with a `node --test` script of the same kind. The hub limits with in-memory token buckets. Anonymous writes are limited with one D1 counter row per day and route.

**Tech Stack:** Cloudflare Workers, Durable Objects and D1 (TypeScript, `node --test`); Tauri 2 / Rust (`cargo test --lib`); React 19 + TypeScript; Stream Deck SDK (Node 20 runtime).

**Spec:** `docs/extras/review-beyond-facecam.md` (IDs CORE-n, NET-n, UI-n; line numbers as of 401a320).

## Global Constraints

- All user-visible text is in English. Commits are in English, **without** a `Co-Authored-By` line, one per task, in the log's `Area: what changed, and why` style.
- Server: deploying a worker, applying a remote D1 migration, running any `--remote` D1 command and changing anything in the Cloudflare dashboard are outward-facing: ask Fabi before each. Local only: `npm test`, `npx tsc --noEmit`, `npx wrangler d1 migrations apply clippiboy-friends --local`, `npx wrangler dev --local`, and `scripts/e2e.mjs` against it as `server/README.md` → "Test" describes.
- App: `cargo test --lib` and `npm run build` run via PowerShell on the Windows side. `npx tsc --noEmit` also works from WSL.
- `node --test` on `.ts` files needs Node ≥ 22.18 (type stripping), as the server's `npm test` already does.
- No new npm or cargo dependencies. The `windows` crate already has `Win32_System_Threading`.
- A new Tauri command goes into `generate_handler!` (`lib.rs`), into the list in `src-tauri/build.rs`, and into the capability of each window that calls it (`allow-<name>`). Otherwise calling it is refused.
- Leave alone what `docs/facecam-review.md` owns: CFG-1 (UI-1), CFG-2 (CORE-10), REC-8 (CORE-13), EDIT-11 (UI-10) and SAVE-2/3/6.
- Left out on purpose:
  - NET-2: needs a retention period and new privacy-page text.
  - CORE-7: needs a decision on what an old drive-root clip folder becomes, or its clips stop playing.
  - CORE-9: unmeasured.
  - CORE-11: SAVE-2 comes first.
  - CORE-12: an uninstaller that deletes `%APPDATA%\ClippiBoy` must first rule out a clip folder inside it.
  - UI-7: measure first.
  - UI-8 and the review's improvement ideas.

## Review Focus

- **Sending a clip while "Appear offline" is on:** the friend's decline, cancel and "done" must still reach the sender, or the sender's card hangs at "Moving". → Task 2 test `an invisible user can be reached only by friends they wrote to in the last ten minutes`, plus its e2e check where B writes first.
- **A restarted app next to a second PC:** the new connection must stay and the oldest must go, so the user's own apps do not push each other out. → Task 3 test `the newest four connections stay, the oldest goes`.
- **A burst of real presence changes** (game start, status edit and busy toggle within seconds): friends must end on the last state, not on a throttled one in the middle. → Task 3 e2e check `a burst of presence changes ends on the last one`.
- **ClippiBoy restarts, maybe on another port, while the Stream Deck runs:** the keys must work again within seconds without a visit to the property inspector, even when a manual entry exists. → Task 15 test `a timeout means busy, anything else means gone` and Task 16's manual check.
- **A busy real day under the new ceilings:** real pings and sign-ins must not be refused. The ceilings are set from production numbers before the deploy, and a refused ping keeps its counts in the app. → Task 4's local check with a ceiling of 2, and Task 8 Step 2.

---

## Server

### Task 1: Ops — the old e2e test accounts on production (Fabi, by hand)

`scripts/e2e.mjs` at d1fc89c and d936cef seeded two accounts with fixed tokens. That file is public, and its instructions said `--remote`. Sessions only expire after 180 days idle. No code changes in this task.

- [ ] **Step 1: Ask Fabi, then look.**

```sh
cd server
npx wrangler d1 execute clippiboy-friends --remote --command "SELECT id, discord_id, username, created_at FROM users WHERE discord_id LIKE 'test-%' OR id IN ('00000000-0000-4000-8000-00000000000a','00000000-0000-4000-8000-00000000000b')"
npx wrangler d1 execute clippiboy-friends --remote --command "SELECT user_id, created_at, last_used FROM sessions WHERE user_id IN ('00000000-0000-4000-8000-00000000000a','00000000-0000-4000-8000-00000000000b')"
npx wrangler d1 execute clippiboy-friends --remote --command "SELECT id, owner, deleted_at FROM shares WHERE owner IN ('00000000-0000-4000-8000-00000000000a','00000000-0000-4000-8000-00000000000b')"
```

Expected: no rows. If there are rows, go to Step 2.

- [ ] **Step 2: Ask Fabi again, then delete.** Run `--command "DELETE FROM users WHERE discord_id LIKE 'test-%'"`. The delete cascades to sessions, friendships, blocks and share rows. Any R2 files of live shares go with the bucket's 6-day lifecycle rule. Re-run Step 1; expected: no rows.

### Task 2: "Appear offline" holds against relays (NET-1, server)

**Files:**
- Create: `server/src/hubrules.ts`, `server/src/hubrules.test.ts`
- Modify: `server/src/hub.ts:161-166` (`relay`), `:244-267` (`relayFrom`)
- Modify: `server/scripts/e2e.mjs` (after the check "going invisible stamps last seen")

**Interfaces:**
- Produces, in `hubrules.ts` (no imports, with a comment like the one in `telemetry.ts`): `CONTACT_MS = 600_000`, `CONTACT_MAX = 50`, `type Contacts = Record<string, number>` (peer id → expiry in ms), `remember(contacts: Contacts, to: string, now: number): Contacts`, `mayReach(contacts: Contacts | undefined, from: string, now: number): boolean`.

- [ ] **Step 1: Write the failing tests** in `hubrules.test.ts`, in the style of `shares.test.ts`:

```ts
test("an invisible user can be reached only by friends they wrote to in the last ten minutes", () => {
  const contacts = remember({}, "f1", now);
  assert.equal(mayReach(contacts, "f1", now + CONTACT_MS - 1), true);
  assert.equal(mayReach(contacts, "f1", now + CONTACT_MS), false);
  assert.equal(mayReach(contacts, "f2", now), false);
  assert.equal(mayReach(undefined, "f1", now), false);
});

test("the list drops what expired and keeps the newest fifty", () => {
  let contacts: Contacts = { old: now - 1 };
  for (let i = 0; i < 60; i++) contacts = remember(contacts, `f${i}`, now + i);
  assert.equal(Object.keys(contacts).length, CONTACT_MAX);
  assert.equal("old" in contacts || "f0" in contacts, false);
  assert.equal("f59" in contacts, true);
});
```

- [ ] **Step 2: Run** `cd server && npm test`. Expected: FAIL, module `./hubrules.ts` not found.

- [ ] **Step 3: Implement.** In `hub.ts`, add a storage key `CONTACTS = "contacts"`. Use storage, not memory: the hub hibernates between messages.
  - `relayFrom`: when `this.current() === null`, the sender is invisible (a ready socket just sent this). Before the relay call, store `remember(stored ?? {}, to, now)`.
  - `relay(from, body)`: when `this.current() === null` and `!mayReach(stored, from, Date.now())`, `return false`. The sender's hub then answers `relayFailed "offline"`, exactly as it does for an absent user.
  - Do not add a plain `if (this.current() === null) return false;`. That would cut off the replies to an invisible user who sends clips.

- [ ] **Step 4: Add the e2e checks.** All three run while B is invisible:
  1. A relays `{kind: "shareCancel", id: "probe"}` to B. Check `invisible B looks offline to a relay too`: A gets `relayFailed` with `ref === "probe"` and `reason === "offline"`.
  2. B relays `{kind: "shareCancel", id: "hi"}` to A, and A receives it.
  3. A relays `{kind: "shareCancel", id: "answer"}` to B. Check `a friend invisible B wrote to still reaches B`: B's inbox gets `answer`.

- [ ] **Step 5: Run** `npm test` and `npx tsc --noEmit`. Expected: pass. Then run the local e2e. Expected: `ALL PASSED`, including both new checks.

- [ ] **Step 6: Commit** `Friends server: while "Appear offline" is on, a relay reaches the user only from friends they wrote to in the last ten minutes — to everyone else they look offline, as their presence already did`

### Task 3: One budget per user, capped sockets, throttled presence (NET-4, server)

**Files:**
- Modify: `server/src/hubrules.ts`, `server/src/hubrules.test.ts`
- Modify: `server/src/hub.ts`: `:39-43` (constants), `:54` (`relayed`), `:63-81` (`fetch`), `:84-101` (`webSocketMessage`), `:111` and `:117` (`relayed.delete`), `:250-256` (budget)
- Modify: `server/scripts/e2e.mjs` (two checks at the end), `server/README.md` ("Relay" paragraph)

**Interfaces:**
- Produces, in `hubrules.ts`:
  - `type Bucket = { tokens: number; at: number }` and `type Rule = { capacity: number; refillMs: number }`.
  - Rules: `RELAY: Rule = { capacity: 30, refillMs: 333 }` (the old 30 per 10 s), `PRESENCE: Rule = { capacity: 10, refillMs: 30_000 }`, `HELLO: Rule = { capacity: 5, refillMs: 60_000 }`. Limits: `RAW_MAX = 8192`, `SOCKET_CAP = 4`.
  - `spend(bucket: Bucket | undefined, rule: Rule, now: number): { bucket: Bucket; ok: boolean; nextAt: number }`. `undefined` means a full bucket. Tokens refill continuously and never go above `capacity`. `nextAt` is when the next token is there (`now` when `ok`).
  - `beyondCap<T extends { opened?: number }>(sockets: T[], cap: number): T[]` returns the oldest sockets past `cap`. A missing `opened` counts as oldest.

- [ ] **Step 1: Write the failing tests:**

```ts
const rule = { capacity: 3, refillMs: 1000 };

test("a fresh budget lets a burst through, then says when the next one may go", () => {
  let b = spend(undefined, rule, now);
  b = spend(spend(b.bucket, rule, now).bucket, rule, now);
  assert.equal(b.ok, true);
  const fourth = spend(b.bucket, rule, now);
  assert.deepEqual([fourth.ok, fourth.nextAt], [false, now + 1000]);
});

test("a budget fills up with time, never above its capacity", () => {
  let bucket = spend(undefined, rule, now).bucket;
  const later = now + 60_000;
  const oks = [1, 2, 3, 4].map(() => { const r = spend(bucket, rule, later); bucket = r.bucket; return r.ok; });
  assert.deepEqual(oks, [true, true, true, false]);
});

test("the newest four connections stay, the oldest goes", () => {
  const sockets = [5, 1, 3, 4, 2].map((opened) => ({ opened }));
  assert.deepEqual(beyondCap(sockets, 4), [{ opened: 1 }]);
  assert.deepEqual(beyondCap([{ opened: 9 }, {}], 1), [{}]);
  assert.deepEqual(beyondCap(sockets.slice(0, 3), 4), []);
});
```

- [ ] **Step 2: Run** `npm test`. Expected: FAIL, `spend` is not exported.

- [ ] **Step 3: Implement in `hub.ts`:**
  - `Attachment` gains `opened: number`, set in `fetch`. After `acceptWebSocket`, close every socket in `beyondCap(open sockets with their attachments, SOCKET_CAP)` with `ws.close(4002, "too many connections")`. The app treats 4002 as a dropped line and dials again.
  - `private buckets = new Map<"relay" | "presence" | "hello", Bucket>()` replaces `relayed`. It lives in memory only and is lost on hibernation, which only follows quiet.
  - `webSocketMessage`: return early when `message.length > RAW_MAX`, before `JSON.parse`. A `hello` over `HELLO` gets `ws.close(4003, "slow down")`.
  - Every hello and presence message still updates the attachment, then calls `publishSoon(user)`. That spends `PRESENCE`. If `ok`, it calls `publish`. Otherwise it sets the alarm to `nextAt`, unless an alarm is already due earlier. `alarm()` already publishes the current state, so the last change always goes out.
  - Closing and errors still publish at once.
  - `relayFrom` spends `RELAY` instead of the per-socket window, and still answers "slow down".
  - README: "30 messages per 10 s per user, at most four connections, presence published at most ten at once and then one per 30 s, the last one always".

- [ ] **Step 4: Add the e2e checks** at the end of the script, before `sa.ws.close()`:
  1. `a burst of presence changes ends on the last one`: a fresh B socket says hello, then sends 15 presence messages with status `s0`…`s14`. Within 35 s, A's newest presence for B has status `s14`.
  2. `a fifth connection closes the oldest`: open four more A sockets. `sa` closes with code 4002.

- [ ] **Step 5: Run** `npm test` and `npx tsc --noEmit`. Expected: pass. Then run the local e2e. Expected: `ALL PASSED`.

- [ ] **Step 6: Commit** `Friends server: one relay budget per user instead of per connection, at most four connections per user, presence published ten at once and then every 30 s (the last change always), raw messages over 8 KB dropped unread`

### Task 4: Anonymous writes — daily ceilings, /64 keys, bounded bodies, known download links (NET-6, NET-7)

**Files:**
- Create: `server/src/limits.ts`, `server/src/limits.test.ts`, `server/migrations/0007_anon_budget.sql`
- Modify: `server/src/index.ts`: `:45-63` (cron), `:72-77`, `:89-92`, `:153-156`
- Modify: `server/src/util.ts:16-22`, `server/src/links.ts:322-329`
- Modify: `server/src/telemetry.ts:69-72` (`refFromPath`), `:83-87` (`handlePing`), `server/src/telemetry.test.ts`
- Modify: `server/site-proxy/index.ts:7-11`, `:27-29`, `:68-73`, `server/wrangler.jsonc`, `server/site-proxy/wrangler.jsonc`, and `worker-configuration.d.ts` via `npm run types`

**Interfaces:**
- Produces, in `limits.ts` (no imports):
  - `readLimited(request: Pick<Request, "headers" | "body">, max: number): Promise<string | null>`. Returns `null` when the declared `Content-Length` is over `max` (nothing is read) or when the body grows past `max` while it is read (the reader is cancelled). Returns `""` for no body. A missing `Content-Length` is fine: the running count decides.
  - `addressKey(address: string): string`. IPv4 and `"unknown"` stay as they are. An IPv6 address becomes its normalised /64.
- Changes, in `telemetry.ts`: `refFromPath(pathname: string, known?: readonly string[]): string` (with `known`, a valid ref that is not in it counts as `"site"`), and `handlePing(text: string | null, env: Env)` (the router reads the body).

- [ ] **Step 1: Write the failing tests** in `limits.test.ts`, plus one in `telemetry.test.ts`:

```ts
const req = (body: string | ReadableStream | null, headers: Record<string, string> = {}) =>
  ({ headers: new Headers(headers), body: typeof body === "string" ? new Response(body).body : body });

test("a body over the limit is refused, by its declared length or by what arrives", async () => {
  assert.equal(await readLimited(req("{}"), 4096), "{}");
  assert.equal(await readLimited(req("a".repeat(5000)), 4096), null);
  assert.equal(await readLimited(req("{}", { "content-length": "999999" }), 4096), null);
  const flood = new ReadableStream({ start(c) { for (let i = 0; i < 10; i++) c.enqueue(new Uint8Array(1024)); c.close(); } });
  assert.equal(await readLimited(req(flood), 4096), null);
  assert.equal(await readLimited(req(null), 4096), "");
});

test("an IPv6 address counts as its /64, an IPv4 address as itself", () => {
  assert.equal(addressKey("2001:db8:1:2:3:4:5:6"), "2001:db8:1:2::/64");
  assert.equal(addressKey("2001:DB8:1:2::9"), "2001:db8:1:2::/64");
  assert.equal(addressKey("2001:db8::1"), "2001:db8:0:0::/64");
  assert.equal(addressKey("203.0.113.7"), "203.0.113.7");
  assert.equal(addressKey("unknown"), "unknown");
});

// telemetry.test.ts
test("only known links count as their own source", () => {
  assert.equal(refFromPath("/get/tiktok1", ["site", "tiktok1"]), "tiktok1");
  assert.equal(refFromPath("/get/made-up", ["site", "tiktok1"]), "site");
  assert.equal(refFromPath("/get/made-up"), "made-up");
});
```

- [ ] **Step 2: Run** `npm test`. Expected: FAIL, module `./limits.ts` not found.

- [ ] **Step 3: Write the migration** `0007_anon_budget.sql`:

```sql
-- How many writes the routes anyone may call made today, per route. Counted
-- before the write; past the day's ceiling they answer 429, so a flood cannot
-- spend the database's daily writes for everybody.
CREATE TABLE anon_budget (
  day TEXT NOT NULL,
  route TEXT NOT NULL,
  count INTEGER NOT NULL,
  PRIMARY KEY (day, route)
);
```

  In `server/wrangler.jsonc`, add `vars` `"ANON_PINGS_PER_DAY": "6000"` and `"ANON_SIGNINS_PER_DAY": "1500"`, with a comment. A new install's ping writes about 7 rows (two tables, their indexes and this counter), and a sign-in start about 3. Together that stays under half of D1's free 100k rows a day. Set both above the real daily numbers (Task 8, Step 2). Then run `npm run types`.

- [ ] **Step 4: Implement:**
  - `underLimit` keys on `${route}:${addressKey(address)}`. The route type becomes `"auth" | "ping" | "exchange"`, and `/auth/exchange` goes under it as `"exchange"`.
  - Add `withinCeiling(env, route: "auth" | "ping"): Promise<boolean>`. It runs after `underLimit`. A missing or unparsable var falls back to 6000 for pings and 1500 for sign-ins. The decision is:

    ```sql
    INSERT INTO anon_budget (day, route, count) VALUES (?1, ?2, 1)
    ON CONFLICT(day, route) DO UPDATE SET count = count + 1 WHERE count < ?3
    ```

    with `?1 = utcDay(Date.now())`. The route is allowed when `meta.changes === 1`.
  - `/t/ping`: `handlePing(await readLimited(request, 1024), env)`, where `null` means 400. Past the ceiling it answers 429 "Too many pings.", and the app keeps its counts for later.
  - `/auth/start` past the ceiling: `page("Sign-in is busy", "Too many sign-ins today. Try again later.")`.
  - The cron also runs `DELETE FROM anon_budget WHERE day < ?` for the day before yesterday.
  - `body()` uses `readLimited(request, 4096)`: `null` → 413 "Request too large.", unparsable JSON → 400 as now.
  - `reportShare` keys `REPORT_LIMIT` on `addressKey` and reads with `readLimited(request, 2048)`.
  - Site proxy: `Env` gains `DOWNLOAD_REFS: string`. The proxy calls `refFromPath(url.pathname, (env.DOWNLOAD_REFS ?? "site").split(","))`, and `DOWNLOAD_LIMIT` keys on `addressKey`. In its `wrangler.jsonc`, set `"vars": { "DOWNLOAD_REFS": "site" }` with a comment: every ref a campaign link uses, comma-separated. A ref that is not listed counts as the site, in the count and in the setup's file name.

- [ ] **Step 5: Run** `npm test` and `npx tsc --noEmit`. Expected: pass. Then apply the migration locally and run `npx wrangler dev --local --var ANON_PINGS_PER_DAY:2`.
  - Three pings with new UUIDs answer `{"ok":true}`, `{"ok":true}` and then 429.
  - `/auth/start?challenge=<43 chars>` still answers 302.
  - A 5 KB body to `/auth/exchange` answers 413.
  - `anon_budget` holds `(today, 'ping', 2)`.
  - The local e2e still ends with `ALL PASSED`.

- [ ] **Step 6: Commit** `Friends server: the routes anyone may call have a daily ceiling each, IPv6 addresses count per /64, request bodies are read only up to a few KB and /auth/exchange is limited per address; the site counts downloads only for known links`

### Task 5: Friend routes — a per-user limit, notices only for real changes, requests-off looks like nobody (NET-5 server, NET-V2)

**Files:**
- Modify: `server/wrangler.jsonc` (`ratelimits`), and `worker-configuration.d.ts` via `npm run types`
- Modify: `server/src/index.ts`: `:94-145` (route), `:116-122`, `:345-377`, `:392-397`, `:399-413`
- Modify: `server/scripts/e2e.mjs`

**Interfaces:**
- Produces: a rate-limit binding `USER_LIMIT` (`namespace_id "1004"`, 30 per 60 s), keyed on the signed-in user's id.

- [ ] **Step 1: Write the failing e2e checks** (this task has no pure logic):
  1. `requests-off looks like nobody by name`: B sends `PATCH /me {allowRequests:false}`. A's request for `"testbravo"` gets 404, and A's request by B's friend code gets 403. Then B switches requests back on.
  2. `unfriending a stranger tells nobody`: A sends `DELETE /friends/<random uuid>` and gets 200. A's socket receives no `{t:"friends"}` within 2 s.

- [ ] **Step 2: Run** the local e2e. Expected: both new checks FAIL (403 by name, and a `friends` message arrives).

- [ ] **Step 3: Implement:**
  - After `authenticate`, every non-GET route except `/auth/logout` checks `env.USER_LIMIT.limit({ key: me })`. When it fails, throw `HttpError(429, "Slow down — try again in a minute.")`.
  - `PATCH /me`: write only when the value differs (`... WHERE id = ? AND allow_requests != ?`).
  - `removeFriend` notifies only when `meta.changes > 0`.
  - `blockUser` notifies both users when the friendship delete changed a row. Otherwise it notifies only `me` when the block row is new, and nobody when nothing changed.
  - `sendRequest`, requests switched off: a request by name gets the same 404 as a block (one `NOBODY` message constant). A request by friend code keeps its 403, because whoever has the code was given it.

- [ ] **Step 4: Run** `npm test`, `npx tsc --noEmit` and the local e2e. Expected: `ALL PASSED`.

- [ ] **Step 5: Commit** `Friends server: changes are limited to 30 a minute per user, removing and blocking notify only when something changed, and someone who takes no requests looks like nobody to a search by name`

### Task 6: A username belongs to one account (NET-12)

**Files:**
- Create: `server/migrations/0008_unique_usernames.sql`
- Modify: `server/src/db.ts:93-119` (`upsertDiscordUser`)

- [ ] **Step 1: Write the migration:**

```sql
-- Discord hands a released name to someone else. Until the old owner signed in
-- again both rows held it, and a request by name could reach the wrong one.
-- Names held twice are cleared; each comes back with its owner's next sign-in.
UPDATE users SET username = '' WHERE username IN (
  SELECT username FROM users WHERE username != '' GROUP BY username HAVING COUNT(*) > 1
);
DROP INDEX users_username;
CREATE UNIQUE INDEX users_username ON users(username) WHERE username != '';
```

- [ ] **Step 2: Implement.** In both branches of `upsertDiscordUser`, run one `db.batch`. Its first statement is `UPDATE users SET username = '' WHERE username = ? AND discord_id != ?`, bound to `(discord.username, discord.id)`. Its second is the existing update or insert. The insert loop reads `meta.changes` from the second result.

- [ ] **Step 3: Verify locally.**
  - Move 0008 out of `migrations/` for a moment, apply the rest to a fresh local database (`--local`), and insert two users whose names differ only in case. Put 0008 back and apply it. Expected: both names are `''`, and `PRAGMA index_list(users)` shows the unique `users_username`.
  - Insert a third user with an existing non-empty name. Expected: it fails.
  - Insert two more with `''`. Expected: both succeed.
  - `npx tsc --noEmit` passes.

- [ ] **Step 4: Commit** `Friends server: a username belongs to one account — signing in takes the name off any stale row, and names held twice are cleared until their owners sign in again`

### Task 7: Sign-in survives a second browser tab and a stray link (NET-11)

**Files:**
- Create: `server/src/login.ts`, `server/src/login.test.ts`
- Modify: `server/src/index.ts`: `:72-75`, `:167-189` (`authStart`), `:191-201` (callback), `:265-280` (`page`)
- Modify: `server/src/util.ts:39-45` (`sameText` moves to `login.ts`, and `util.ts` re-exports it)
- Modify: `src-tauri/src/friends.rs`: `:147-164` (`handle_url`), `:193-204` (`friends_sign_in`), `:289-291` (`finish_sign_in`)

**Interfaces:**
- Produces, in `login.ts` (no imports): `STATES_KEPT = 3`, `sameText(a: string, b: string): boolean`, `statesIn(cookieHeader: string | null, name: string): string[]`, and `withState(states: string[], state: string): string` (newest last, at most three, joined by `.`; base64url never contains a `.`).
- Produces, in `friends.rs`: `fn pending(slot: &mut Option<String>) -> String`, which returns the waiting verifier, or a new one that it keeps in the slot.

- [ ] **Step 1: Write the failing tests:**

```ts
test("a second sign-in start keeps the first tab's state", () => {
  const first = withState(statesIn("x=1", "cb_login"), "aaa");
  const both = withState(statesIn(`x=1; cb_login=${first}`, "cb_login"), "bbb");
  assert.deepEqual(statesIn(`cb_login=${both}`, "cb_login"), ["aaa", "bbb"]);
});
test("only the last three starts are kept", () => assert.equal(withState(["a", "b", "c"], "d"), "b.c.d"));
test("no cookie, or someone else's, holds no state", () => {
  assert.deepEqual(statesIn(null, "cb_login"), []);
  assert.deepEqual(statesIn("other=abc", "cb_login"), []);
});
```

```rust
#[test]
fn a_second_sign_in_reuses_the_waiting_verifier() {
    let mut slot = None;
    let first = pending(&mut slot);
    assert_eq!(pending(&mut slot), first);
    assert_eq!(slot.as_deref(), Some(first.as_str()));
}
```

- [ ] **Step 2: Run** `npm test` and `cargo test --lib friends`. Expected: both FAIL; the module is missing and `pending` is not defined.

- [ ] **Step 3: Implement.**
  - Server:
    - `authStart(request, url, env)` sets `cb_login=${withState(statesIn(cookie), state)}`.
    - The callback accepts a `state` that is in the list (`some(s => sameText(s, state))`).
    - `page(title, text, redirect?, clear = false)` clears the cookie only for the "Signed in" page.
  - Client:
    - `friends_sign_in` uses `pending`.
    - `finish_sign_in` clones the verifier and clears it only after a successful exchange.
    - An auth link that arrives while no sign-in is waiting is logged at info level and ignored, without a notice.

- [ ] **Step 4: Run** `npm test`, `npx tsc --noEmit`, `cargo test --lib` and the local e2e (`auth/start` still answers 302). Expected: all pass.

- [ ] **Step 5: Manual check after Task 8's deploy.** Click Sign in, press "Open the browser again", then finish in the first tab. Expected: ClippiBoy signs in. Finishing the second tab afterwards says the sign-in expired, and the app stays signed in.

- [ ] **Step 6: Commit** `Sign-in: "Open the browser again" no longer breaks the first tab — the server keeps the last three starts, the app keeps its verifier until a sign-in succeeds and ignores auth links when none is waiting`

### Task 8: Ship the server (ask Fabi before each step)

- [ ] **Step 1: Run the full checks.** In `server/`: `npm test` and `npx tsc --noEmit`. Then the local e2e. Expected: `ALL PASSED`.
- [ ] **Step 2: Read the real numbers** (`--remote`; read-only, but still ask).
  - Pings: `SELECT day, COUNT(*) FROM install_days WHERE day >= date('now','-7 day') GROUP BY day`.
  - Sign-ins: `SELECT date(created_at/1000,'unixepoch') d, COUNT(*) FROM sessions GROUP BY d ORDER BY d DESC LIMIT 7`.
  - Download refs: `SELECT DISTINCT ref FROM downloads`.

  Set `ANON_PINGS_PER_DAY` and `ANON_SIGNINS_PER_DAY` to at least three times the highest day. Put the real campaign refs into `DOWNLOAD_REFS` and leave out the junk. Commit as `Friends server: ceilings and known download links set from the real numbers`.
- [ ] **Step 3: Add the outer fence.** In the Cloudflare dashboard, open the clippiboy.com zone and go to Security → WAF → Rate limiting rules. Add one rule (the Free plan includes one): expression `(http.host eq "api.clippiboy.com" and http.request.uri.path in {"/t/ping" "/auth/start"})`, counted per IP, 20 requests per 10 s, action Block.
- [ ] **Step 4: Apply the migrations:** `npx wrangler d1 migrations apply clippiboy-friends --remote` (0007 and 0008).
- [ ] **Step 5: Deploy** `npx wrangler deploy` in `server/`, then in `server/site-proxy/`.
- [ ] **Step 6: Smoke test.**
  - One ping with a throwaway UUID answers `{"ok":true}`. Delete that install's rows afterwards.
  - `curl -sI https://clippiboy.com/get/made-up` names a file ending in `.site.exe`.
  - In the app, the friends list, presence, a clip to a friend and a share link all still work.
  - Run Task 7's manual check.

## Sharing in the core

### Task 9: Muted tracks stay home (NET-3)

**Files:**
- Modify: `src-tauri/src/share.rs`: `:307-342` (`share_send`), a new fn next to `cut_tracks` (`:1142`), and tests at `:1246`
- Modify: `site/privacy.html:96`

**Interfaces:**
- Produces: `fn audible_stems(stems: Vec<ClipTrack>, mix: &[TrackMix]) -> (Vec<ClipTrack>, Vec<TrackMix>)`.
  - Stems come back sorted by `index`. With an empty mix, everything is returned as it is.
  - Otherwise it drops a stem whose mix entry is `muted` or not `stems::audible(gain_db)`. It keeps a stem without an entry, at 0 dB and unmuted.
  - The returned mix is renumbered `0..n` in the order of the returned stems. That is the slot order `cut_tracks` writes and the receiver stores.

- [ ] **Step 1: Write the failing tests:**

```rust
fn stem(index: u32, label: &str) -> ClipTrack {
    ClipTrack { index, label: label.into(), channels: 2, preview_path: Some(format!("{index}.m4a")) }
}
fn level(index: u32, gain_db: f32, muted: bool) -> TrackMix { TrackMix { index, gain_db, muted } }

#[test]
fn muted_and_silent_tracks_stay_home() {
    let stems = vec![stem(0, "Game"), stem(1, "Mic"), stem(2, "Discord"), stem(3, "Music")];
    let mix = [level(0, 0.0, false), level(1, -3.0, true), level(2, -60.0, false), level(3, 4.0, false)];
    let (kept, mix) = audible_stems(stems, &mix);
    assert_eq!(kept.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(), ["Game", "Music"]);
    assert_eq!(mix, [level(0, 0.0, false), level(1, 4.0, false)]);
}

#[test]
fn without_a_mix_every_track_goes() {
    let (kept, mix) = audible_stems(vec![stem(1, "Mic"), stem(0, "Game")], &[]);
    assert_eq!(kept.iter().map(|t| t.index).collect::<Vec<_>>(), [0, 1]);
    assert!(mix.is_empty());
}

#[test]
fn a_clip_with_every_track_muted_goes_without_tracks() {
    let (kept, mix) = audible_stems(vec![stem(0, "Game")], &[level(0, 0.0, true)]);
    assert!(kept.is_empty() && mix.is_empty());
}
```

- [ ] **Step 2: Run** `cargo test --lib share` (PowerShell). Expected: FAIL to compile, `audible_stems` is not defined.

- [ ] **Step 3: Implement.** In `share_send`, pass the stored stems through `audible_stems` with `clip.edit`'s tracks. An empty result means `None` (no tracks). Line 337 uses the returned mix instead of `edit.tracks`.

  In the privacy page, after "The clip itself never reaches our server.", add: "If a clip has separate tracks (game, microphone, voice chat), the ones you can hear in it go along, so your friend can change the mix. Tracks you muted or turned all the way down stay on your PC."

- [ ] **Step 4: Run** `cargo test --lib`. Expected: all green.

- [ ] **Step 5: Manual check.** Mute the Discord track of a clip and send it to a test friend. Expected: the receiver's mixer shows Game and Mic only, and `%TEMP%\clippiboy-share\<id>\` held two files.

- [ ] **Step 6: Commit** `Sending a clip: tracks muted or turned all the way down stay on the PC instead of reaching the friend raw; the privacy page says the other tracks go along`

### Task 10: A public link carries one picture, one sound and no friends' names (NET-V1, NET-8)

**Files:**
- Modify: `src-tauri/src/links.rs`: `:436-450` (fallback), `:504-511` (`faststart`), plus a test module at `:671`
- Modify: `src-tauri/src/clips.rs:340-352`, `src-tauri/src/share.rs:896`
- Modify: `site/privacy.html` ("Share links" paragraph)

**Interfaces:**
- Produces, in `links.rs`: `fn faststart_args(source: &Path, output: &Path) -> Vec<std::ffi::OsString>`, which yields `-map 0:v:0 -map 0:a:0? -map_metadata -1 -map_chapters -1 -c copy -movflags +faststart`.
- Produces, in `clips.rs`: `pub fn from_tag(name: &str) -> String` (`"from {name}"`). `is_friend_tag` becomes true for `with ` and `from ` in any case.

- [ ] **Step 1: Write the failing tests:**

```rust
#[test]
fn a_link_copy_keeps_one_picture_and_one_sound() {
    let args: Vec<String> = faststart_args(Path::new("in.mp4"), Path::new("out.mp4"))
        .iter().map(|a| a.to_string_lossy().into_owned()).collect();
    for pair in [["-map", "0:v:0"], ["-map", "0:a:0?"], ["-map_metadata", "-1"], ["-map_chapters", "-1"]] {
        assert!(args.windows(2).any(|w| w == pair), "{pair:?} missing in {args:?}");
    }
    assert!(!args.windows(2).any(|w| w == ["-map", "0"]), "{args:?}");
}
// clips.rs, in a_friend_tag_is_known_in_any_case:
assert!(is_friend_tag(&from_tag("Luca")));
assert!(is_friend_tag("From Luca"));
assert!(!is_friend_tag("fromage"));
```

- [ ] **Step 2: Run** `cargo test --lib links clips`. Expected: FAIL to compile.

- [ ] **Step 3: Implement.**
  - `faststart` runs `faststart_args`.
  - In `upload`, a `faststart` that fails, or whose copy is over `MAX_BYTES`, no longer uploads the source as it is. It goes the `shrink` route, which maps only `0:v:0`/`0:a:0?` (`export.rs:138`), and keeps its log line.
  - `share.rs:896` uses `from_tag`.
  - In the privacy page, change "together with its title, game, tags" to "…tags (not the ones that name friends, “with …” and “from …”)".

- [ ] **Step 4: Run** `cargo test --lib`. Expected: all green.

- [ ] **Step 5: Manual check.**
  - Share an old multi-track clip of 50 MB or less as a link. Expected: `ffprobe` on the downloaded `.mp4` lists one audio stream.
  - Share a clip a friend sent you. Expected: the page shows no "from …" pill.

- [ ] **Step 6: Commit** `Share links: the copy that goes up holds the picture and the one mixed sound only — an old clip's raw source tracks and its metadata stay home — and a received clip's "from <friend>" tag stays off the public page`

### Task 11: Client guards — no answer while invisible, one offer per friend, one request alert a day (NET-1, NET-4, NET-5 client parts)

**Files:**
- Modify: `src-tauri/src/share.rs:543-562` (`offered`), plus tests
- Modify: `src-tauri/src/friends.rs`: `:108-119` (`Friends`), `:358-367` (`refresh`'s announce), plus tests at `:862`

**Interfaces:**
- Produces, in `share.rs`:
  - `#[derive(Debug, PartialEq)] enum Verdict { Ask, Decline, Ignore }`
  - `fn verdict(friends: &FriendsConfig, from: &str, asking_already: bool, sane: bool) -> Verdict`. Invisible gives `Ignore`. An offer from that friend that is still `Asking` gives `Ignore`. Busy, unwelcome under `accept_clips`, or not `sane` (`id.len() <= 64 && sha256.len() == 64`) gives `Decline`. Anything else gives `Ask`.
- Produces, in `friends.rs`: `fn first_today(seen: &mut HashMap<String, i64>, id: &str, now: i64) -> bool`. It forgets entries older than 24 h, then records `id` and is true only if `id` was not there. `Friends` gains `announced: Mutex<HashMap<String, i64>>`.

- [ ] **Step 1: Write the failing tests:**

```rust
#[test]
fn an_invisible_user_neither_asks_nor_answers() {
    let config = FriendsConfig { invisible: true, busy: true, ..Default::default() };
    assert_eq!(verdict(&config, "f", false, true), Verdict::Ignore);
}

#[test]
fn a_second_offer_from_the_same_friend_waits_for_the_first() {
    assert_eq!(verdict(&FriendsConfig::default(), "f", true, true), Verdict::Ignore);
}

#[test]
fn busy_or_unwelcome_is_a_quiet_no() {
    let favorites = FriendsConfig { accept_clips: AcceptClips::Favorites, favorites: vec!["g".into()], ..Default::default() };
    assert_eq!(verdict(&FriendsConfig { busy: true, ..Default::default() }, "f", false, true), Verdict::Decline);
    assert_eq!(verdict(&FriendsConfig { accept_clips: AcceptClips::Off, ..Default::default() }, "f", false, true), Verdict::Decline);
    assert_eq!(verdict(&favorites, "f", false, true), Verdict::Decline);
    assert_eq!(verdict(&favorites, "g", false, true), Verdict::Ask);
    assert_eq!(verdict(&FriendsConfig::default(), "f", false, false), Verdict::Decline);
}

// friends.rs
#[test]
fn a_request_that_comes_back_is_announced_once_a_day() {
    let (mut seen, day) = (HashMap::new(), 24 * 3600 * 1000);
    assert!(first_today(&mut seen, "u", 0));
    assert!(!first_today(&mut seen, "u", day - 1));
    assert!(first_today(&mut seen, "u", day));
}
```

- [ ] **Step 2: Run** `cargo test --lib share friends`. Expected: FAIL to compile.

- [ ] **Step 3: Implement.**
  - `offered` computes `asking_already` from `incoming` (same `friend_id`, stage `Asking`), then follows the verdict. `Ignore` logs at info level and returns without a relay, a sound or a banner. `Decline` is today's quiet no. `Ask` behaves as now.
  - `refresh` raises the friend-request alert only when `first_today(...)` is true.

- [ ] **Step 4: Run** `cargo test --lib`. Expected: all green.

- [ ] **Step 5: Commit** `Friends: while "Appear offline" is on, offers are left unanswered instead of declined; a friend gets one open offer at a time, and a friend request that is withdrawn and sent again is announced once a day`

### Task 12: Part files named after the offer, and nothing left behind (NET-9)

**Files:**
- Modify: `src-tauri/src/share.rs`:
  - `:671-675` and `:715` (part file and target)
  - `:322-325`, `:330` and `:346-349` (early returns)
  - `:457-473` (`share_dismiss`, `share_clear`)
  - `:1042-1045` (cleanup after success)
  - `:1206-1224` (`safe_file_name`)
- Modify: `src-tauri/src/filing.rs:62-71` (the reserved-name check moves into a fn)
- Modify: `src-tauri/src/lib.rs:712-723` (`discard_leftovers`) and `setup` (start the sweep)

**Interfaces:**
- Produces, in `filing.rs`: `pub(crate) fn unreserved(name: &str) -> String`. It puts `_` in front of a Windows device name, with or without an extension, by the rule `folder_name` uses today. `folder_name` then calls it.
- Produces, in `share.rs`:
  - `fn part_path(dir: &Path, offer_id: &str) -> PathBuf` → `dir/.clippiboy-<muxer::sanitize(id)>.part`
  - `pub fn sweep_parts(clip_dir: &Path, older_than: Duration) -> usize` removes our own part files in the clip folder and up to two levels below it, and returns how many it removed.
  - `fn drop_cuts(id: &str)` removes `%TEMP%\clippiboy-share\<sanitize(id)>`.

- [ ] **Step 1: Write the failing tests:**

```rust
#[test]
fn a_device_name_from_a_friend_is_not_a_file_name() {
    assert_eq!(safe_file_name("CON.mp4"), "_CON.mp4");
    assert_eq!(safe_file_name("com1.MKV"), "_com1.mkv");
    assert_eq!(safe_file_name("Console.mp4"), "Console.mp4");
}

#[test]
fn two_offers_of_one_name_get_parts_of_their_own() {
    let dir = Path::new("C:/clips/CS2/Videos");
    assert_ne!(part_path(dir, "a1"), part_path(dir, "b2"));
    assert_eq!(part_path(dir, "../x").file_name().unwrap(), ".clippiboy-___x.part");
}

#[test]
fn only_our_own_old_parts_are_swept() {
    // A temp clip folder with CS2/Videos/.clippiboy-a.part and Videos/.clippiboy-b.part
    // (both aged one hour with File::set_modified), CS2/Videos/.clippiboy-c.part (fresh)
    // and CS2/Videos/movie.mp4.part (aged, a browser's).
    assert_eq!(sweep_parts(&dir, Duration::from_secs(600)), 2);
    assert!(c.exists() && movie.exists() && !a.exists() && !b.exists());
}
```

- [ ] **Step 2: Run** `cargo test --lib share filing`. Expected: FAIL to compile.

- [ ] **Step 3: Implement.**
  - `receive` writes into `part_path(&dir, id)` and chooses `free_name` right before the rename, after the hash check. Two receives of one name then no longer share a part, and a leftover can no longer give a different file its resume offset.
  - `safe_file_name` returns `unreserved(...)` of its result.
  - `discard_leftovers` also clears `std::env::temp_dir().join("clippiboy-share")`.
  - `setup` starts `std::thread::spawn(move || share::sweep_parts(Path::new(&clip_dir), Duration::from_secs(600)))` and logs the count. It runs on a thread because the clip folder can be a sleeping NAS.
  - `drop_cuts` is called from `share_dismiss` (that id), from `share_clear` (every outgoing transfer it removes), from each early return in `share_send` after the cut, and after a successful send instead of the per-file loop.

- [ ] **Step 4: Run** `cargo test --lib`. Expected: all green, including the existing `filing` tests.

- [ ] **Step 5: Commit** `Receiving a clip: the part file is named after the offer and stray ones are swept at start; cut tracks for a transfer that ended any way are deleted, and a friend's file named like a Windows device gets a _ in front`

### Task 13: A peer that goes quiet is given up on (NET-10)

**Files:**
- Modify: `src-tauri/src/share.rs`: `:813-839` (`fetch`), `:947-956` and `:1001` (`serve`)

**Interfaces:**
- Produces: `const HANDSHAKE: Duration = Duration::from_secs(10)`, `const IDLE: Duration = Duration::from_secs(30)`, and `async fn within<T, E: std::fmt::Debug>(limit: Duration, what: &str, work: impl std::future::Future<Output = Result<T, E>>) -> Result<T, String>`. It gives `Err(format!("{what} timed out"))` when the limit passes and `net(err)` on an error.

- [ ] **Step 1: Write the failing test:**

```rust
#[test]
fn a_peer_that_goes_quiet_is_given_up_on() {
    let quiet = std::future::pending::<Result<(), String>>();
    let outcome = tauri::async_runtime::block_on(within(Duration::from_millis(50), "reading", quiet));
    assert_eq!(outcome, Err("reading timed out".to_string()));
}
```

- [ ] **Step 2: Run** `cargo test --lib share`. Expected: FAIL to compile.

- [ ] **Step 3: Implement.**
  - In `fetch`, `open_bi` and the header `read_exact` run under `HANDSHAKE`, and each `recv.read` runs under `IDLE` as "they stopped sending".
  - In `serve`, `accept_bi` and both `read_exact` calls run under `HANDSHAKE`, and each `send.write_all` runs under `IDLE`.
  - Errors take the existing path: the receive loop retries and resumes, and checks the cancel flag before every attempt. So Cancel lands within `IDLE` even against a peer that keeps the line alive without sending, and no `Notify` is needed.

- [ ] **Step 4: Run** `cargo test --lib`. Expected: all green.

- [ ] **Step 5: Commit** `Sending clips: a friend whose app keeps the line open but stops sending is given up on after 30 s, and the handshake reads have a 10 s limit — Cancel no longer leaves a transfer and its part file hanging until the app quits`

## Stream Deck and control port

### Task 14: The status says when a recording is still being written (CORE-1, core)

**Files:**
- Modify: `src-tauri/src/state.rs`: `:85-86` (field), `:108-117` (`SavingGuard` → `FlagGuard`), `:389`, `:466-478`, `:1145-1155` (`stop_recording`), plus tests at `:1320`
- Modify: `src-tauri/src/control.rs:400-418` (`status`)

**Interfaces:**
- Produces:
  - `pub struct FlagGuard<'a>`, renamed from `SavingGuard`, with `pub fn raise(flag: &'a AtomicBool) -> Self`. It sets the flag to true, and its `Drop` sets it to false. `begin_save` keeps returning it.
  - `AppState::is_finalizing(&self) -> bool`.
  - `GET /v1/status` gains `"finalizing": bool`.

- [ ] **Step 1: Write the failing test:**

```rust
#[test]
fn a_raised_flag_comes_down_with_its_guard() {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    let flag = AtomicBool::new(false);
    {
        let _guard = FlagGuard::raise(&flag);
        assert!(flag.load(SeqCst));
    }
    assert!(!flag.load(SeqCst));
}
```

- [ ] **Step 2: Run** `cargo test --lib state`. Expected: FAIL to compile.

- [ ] **Step 3: Implement.** Add `finalizing: AtomicBool` to `AppState`. `stop_recording` raises it after `close_recording` and holds the guard across `finish_recording`. Whether a toggle during the finalize should be refused stays Fabi's call: refusing would block a new recording for up to a minute after a long one. This task only reports the state.

- [ ] **Step 4: Run** `cargo test --lib`. Expected: all green.

- [ ] **Step 5: Commit** `Control port: the status says when a stopped recording is still being written, so a key can show it is busy instead of "not recording"`

### Task 15: Long presses get their time, a dead connection is dropped (CORE-1 plugin, CORE-5 plugin)

**Files:**
- Create: `streamdeck/src/timing.ts`, `streamdeck/src/labels.ts`, `streamdeck/src/timing.test.ts`, `streamdeck/src/labels.test.ts`
- Modify: `streamdeck/src/client.ts`: `:8-22` (`Status`), `:52-53`, `:128-141` (`post`), `:143-188` (`#poll`, `#send`)
- Modify: `streamdeck/src/keys.ts`: `:45-58` (`press`), `:61-105` (the labels move out)
- Modify: the imports in `streamdeck/src/actions/record.ts`, `save-clip.ts` and `toggle-buffer.ts`
- Modify: `streamdeck/package.json` (`"test": "node --test src/*.test.ts"`), `streamdeck/tsconfig.json` (`"exclude": ["src/**/*.test.ts"]`)

**Interfaces:**
- Produces, in `timing.ts` (no imports):
  - `timeoutFor(method: string, path: string): number`. GET gets 2000. POST `/v1/clip` gets 60_000, `/v1/screenshot` 30_000, `/v1/buffer` 15_000 and `/v1/recording` 600_000; any other POST gets 15_000.
  - `failureOf(error: unknown): "slow" | "gone"`. It is "slow" only for an error named `TimeoutError`.
- Produces, in `labels.ts` (type imports only): `bufferLabel`, `durationLabel` and `recordingLabel`, moved unchanged, except that `recordingLabel` returns `"…"` while `status.finalizing` is set.
- Changes, in `client.ts`: `Status.finalizing?: boolean` (missing on older ClippiBoy versions), and `post()` returns `Promise<{ ok: boolean; error?: string } | "slow" | null>`.

- [ ] **Step 1: Write the failing tests:**

```ts
test("a status question gives up after two seconds", () => assert.equal(timeoutFor("GET", "/v1/status"), 2000));
test("saving a clip or a picture may take its time", () => {
  assert.equal(timeoutFor("POST", "/v1/clip"), 60_000);
  assert.equal(timeoutFor("POST", "/v1/screenshot"), 30_000);
});
test("stopping a recording may take ten minutes", () => assert.equal(timeoutFor("POST", "/v1/recording"), 600_000));
test("a timeout means busy, anything else means gone", () => {
  assert.equal(failureOf(new DOMException("t", "TimeoutError")), "slow");
  assert.equal(failureOf(new TypeError("fetch failed")), "gone");
  assert.equal(failureOf(new DOMException("a", "AbortError")), "gone");
});
// labels.test.ts — base: a Status with recording false
test("a recording still being written shows dots, not a stopped key", () =>
  assert.equal(recordingLabel({ ...base, finalizing: true }), "…"));
test("an older ClippiBoy without the flag reads as before", () => assert.equal(recordingLabel(base), ""));
```

- [ ] **Step 2: Run** `cd streamdeck && npm test`. Expected: FAIL, the modules are missing.

- [ ] **Step 3: Implement.**
  - `#send` uses `timeoutFor`.
  - In its `catch`, a `"slow"` failure returns `"slow"` and keeps the connection: the app is there, only busy.
  - Any other failure sets `this.#connection = null`, whatever the source. The `#lookedUp` pause still limits handshake reads, and `#connect` tries the handshake file first again.
  - `#poll` and `describe` treat `"slow"` as no status.
  - `press()` logs `${path}: still working` for `"slow"` and shows no alert. The poll's "…" title shows the work.

- [ ] **Step 4: Run** `npm test` and `npm run build` in `streamdeck/`. Expected: pass.

- [ ] **Step 5: Manual check** (`install.ps1`).
  - Stop a 5-minute recording from the key. Expected: "…", then ✓, never ⚠.
  - Save a 120 s clip. Expected: ✓.
  - Quit ClippiBoy. Expected: the keys show "—".
  - With a manual entry saved in the inspector, occupy the port (for example `python -m http.server 47653 --bind 127.0.0.1`) and start ClippiBoy. Expected: the keys work within about 2 s.

- [ ] **Step 6: Commit** `Stream Deck: a clip, a screenshot or a recording stop gets as long as it needs instead of 2 s, so a save that worked no longer shows ⚠; a key shows … while a recording is written, and a connection that failed is always dropped so a new port or token is picked up`

### Task 16: The handshake goes with the app, carries a fresh token each start and is written whole (CORE-4, CORE-5 core)

**Files:**
- Modify: `src-tauri/src/control.rs`: `:60-107` (`start`), `:130-146` (`regenerate_token`), `:434-451` (`ensure_token` → `fresh_token`), `:457-474` (`write_handoff`)
- Modify: `src-tauri/src/lib.rs:1409-1412`, `src-tauri/src/updater.rs:77-84`
- Modify: `src/routes/settings/StreamDeck.tsx`, `streamdeck/com.einfabo.clippiboy.sdPlugin/ui/settings.html:104-107`

**Interfaces:**
- Produces:
  - `fn fresh_token(app: &tauri::AppHandle) -> String` makes a new UUID v4 on every call and saves it straight into the config, as `ensure_token` does now.
  - `fn write_file_whole(path: &Path, text: &str) -> std::io::Result<()>` writes `<name>.tmp`, then renames it over the target, like `config::save`.

- [ ] **Step 1: Write the failing test:**

```rust
#[test]
fn the_handshake_is_replaced_whole() {
    let dir = std::env::temp_dir().join(format!("cb-handoff-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(HANDOFF);
    write_file_whole(&path, "first").unwrap();
    write_file_whole(&path, "second").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "a temp file was left behind");
    let _ = std::fs::remove_dir_all(dir);
}
```

- [ ] **Step 2: Run** `cargo test --lib control`. Expected: FAIL to compile.

- [ ] **Step 3: Implement.**
  - `start` uses `fresh_token`. A crash can never clear the file, but a token from an earlier run no longer opens anything.
  - `regenerate_token` becomes `restart(app)`, since `start` makes a new token anyway.
  - `write_handoff` goes through `write_file_whole`.
  - Call `control::stop()` in `RunEvent::Exit` next to `telemetry::flush()`, and in the updater's `on_before_exit` after `stop_pipeline()`.
  - `StreamDeck.tsx` takes the token it shows and copies from `api.getConfig()`, both when the page opens and when Copy is pressed, because the store's copy can be older than this start. The hint becomes "Changes every time ClippiBoy starts — the plugin picks it up by itself".
  - The inspector note gains "The token changes every time ClippiBoy starts."

- [ ] **Step 4: Run** `cargo test --lib`, `npx tsc --noEmit` and `npm run build`. Expected: pass.

- [ ] **Step 5: Manual check.**
  - Start ClippiBoy, note the token in `%APPDATA%\ClippiBoy\streamdeck.json`, then Quit from the tray. Expected: the file is gone.
  - Start it again. Expected: a new token, and the keys work without touching the inspector.
  - Run an update. Expected: the file is gone while the installer runs.

- [ ] **Step 6: Commit** `Control port: the handshake file goes when the app quits or updates, is replaced in one piece, and carries a new token every start — a process that takes the port while ClippiBoy is down learns nothing that still works`

### Task 17: One deadline per request (CORE-6)

**Files:**
- Modify: `src-tauri/src/control.rs`: `:39-43`, `:181-190` (`connection`), and tests `:535-569`

**Interfaces:**
- Produces:
  - `const REQUEST_DEADLINE: Duration = Duration::from_secs(3)`, which replaces `READ_TIMEOUT`.
  - `struct Deadline<'a> { stream: &'a TcpStream, until: Instant }` with `Deadline::new(stream, limit)` and `impl Read`. Each read gets only the time that is left. With no time left it returns `ErrorKind::TimedOut`. It never calls `set_read_timeout(Some(Duration::ZERO))`, which errors.

- [ ] **Step 1: Write the failing test.** Add a helper `trickled(head: &'static [u8], every: Duration)` next to `served`. Its client writes `head` one byte per `every` and stops at the first failed write. Its server reads through `Deadline::new(&stream, REQUEST_DEADLINE)` and drops its stream before joining the client, so the test ends at the deadline. Switch `served` to read through a `Deadline` too.

```rust
#[test]
fn a_client_that_trickles_is_let_go_at_the_deadline() {
    let (outcome, took) = trickled(b"GET /v1/status HTTP/1.1\r\nX-Pad: aaaaaaaaaaaaaaaa", Duration::from_millis(400));
    assert_eq!(outcome.unwrap_err().0, 408);
    assert!(took >= REQUEST_DEADLINE && took < REQUEST_DEADLINE + Duration::from_secs(1), "took {took:?}");
}
```

  `a_client_that_stalls_is_let_go_after_the_timeout` keeps its assertions, with `REQUEST_DEADLINE` in place of `READ_TIMEOUT`.

- [ ] **Step 2: Run** `cargo test --lib control`. Expected: FAIL to compile, `Deadline` is not defined.

- [ ] **Step 3: Implement.** `connection()` reads through `Deadline::new(&stream, REQUEST_DEADLINE)` and keeps the 5 s write timeout.

- [ ] **Step 4: Run** `cargo test --lib`. Expected: all green.

- [ ] **Step 5: Commit** `Control port: a request has 3 s in all to arrive, not 2 s per read — eight slow connections can no longer lock the Stream Deck out for hours`

## App core

### Task 18: A second start leaves the log and the microphone alone (CORE-2)

**Files:**
- Modify: `src-tauri/src/logging.rs:27-92`
- Modify: `src-tauri/src/lib.rs`: `:1191-1193` (`run`), `:1228-1236` (start of `setup`)
- Modify: `src-tauri/src/state.rs:350-367` (`AppState::new`)

**Interfaces:**
- Produces, in `logging.rs`:
  - `pub fn init(first: bool)`
  - `pub fn attach_file()` rotates and opens the file once; it is a no-op once a file is attached.
  - The internal `fn open_for(path: &Path, first: bool) -> Option<File>`.
  - The file lives in `static FILE: parking_lot::Mutex<Option<File>>`, which `Fork` writes to.
- Produces, in `lib.rs`: `fn first_instance() -> bool` (always true off Windows).

- [ ] **Step 1: Write the failing tests** (a temp dir per test):

```rust
#[test]
fn a_second_start_leaves_both_logs_alone() {
    std::fs::write(&path, "live").unwrap();
    std::fs::write(path.with_extension("log.1"), "previous").unwrap();
    assert!(open_for(&path, false).is_none());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "live");
    assert_eq!(std::fs::read_to_string(path.with_extension("log.1")).unwrap(), "previous");
}

#[test]
fn the_first_start_keeps_one_generation() {
    std::fs::write(&path, "old").unwrap();
    drop(open_for(&path, true).unwrap());
    assert_eq!(std::fs::read_to_string(path.with_extension("log.1")).unwrap(), "old");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
}
```

- [ ] **Step 2: Run** `cargo test --lib logging`. Expected: FAIL to compile.

- [ ] **Step 3: Implement.** The mutex check, with a name of our own so the single-instance plugin's mutex is left untouched:

```rust
#[cfg(windows)]
fn first_instance() -> bool {
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    // The handle is never closed: the name stays taken for as long as this runs.
    match unsafe { CreateMutexW(None, false, windows::core::w!("Local\\gg.clippiboy.app-first")) } {
        Ok(_) => unsafe { GetLastError() } != ERROR_ALREADY_EXISTS,
        Err(_) => true,
    }
}
```

  - `run()` calls `logging::init(first_instance())` before the builder. A second process logs to stderr only.
  - `setup` calls `logging::attach_file()` first. Only the instance that got past single-instance reaches `setup`, which covers the moment the old instance quit between the two checks.
  - `AppState::new` drops the `audio.apply` block. `setup` calls `app.state::<AppState>().apply_audio()` right after `repair_screens`. That runs on its own thread with the full enumeration (`audio/engine.rs:169`), which also ends AUD-V3's startup freeze.
  - The second process still opens and migrates `clips.db`. That is idempotent and is not changed here.

- [ ] **Step 4: Run** `cargo test --lib`. Expected: all green.

- [ ] **Step 5: Manual check.** With ClippiBoy running, double-click its shortcut, then open a `clippiboy://auth?code=x` link. Expected: the window comes up, `clippiboy.log` keeps growing, `.log.1` is still the previous session's, and the microphone indicator does not flash.

- [ ] **Step 6: Commit** `Start: a second start (shortcut, clippiboy:// link, double autostart) no longer rotates the running session's log away or opens the audio devices before it hands over and ends`

### Task 19: An incomplete ffmpeg folder is cleared and fetched again (CORE-3)

**Files:**
- Modify: `src-tauri/src/tools.rs:163-175` (`provide`), plus a test module

**Interfaces:**
- Produces:
  - `fn missing_programs(target: &Path) -> Vec<&'static str>`
  - `fn clear_incomplete(target: &Path) -> Result<Vec<&'static str>, String>` returns an empty list when the folder is absent. Otherwise it removes the folder and names what was missing.

- [ ] **Step 1: Write the failing tests:**

```rust
#[test]
fn an_incomplete_ffmpeg_folder_is_cleared_and_named() {
    let target = dir.join("9.0.1");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("ffprobe.exe"), b"x").unwrap();
    assert_eq!(clear_incomplete(&target).unwrap(), ["ffmpeg.exe"]);
    assert!(!target.exists());
}

#[test]
fn no_folder_is_nothing_to_clear() {
    assert!(clear_incomplete(&dir.join("absent")).unwrap().is_empty());
}
```

- [ ] **Step 2: Run** `cargo test --lib tools`. Expected: FAIL to compile.

- [ ] **Step 3: Implement.** In `provide`, after the presence check, call `clear_incomplete`. When something was missing, log a warning that names it and calls an antivirus quarantine the usual cause. Then `crate::notify(app, "error", ...)` with, for example, "ffmpeg.exe was missing — often an antivirus quarantine. Fetching it again." Without the clear, the rename onto the non-empty folder failed on every retry, forever.

- [ ] **Step 4: Run** `cargo test --lib`. Expected: all green.

- [ ] **Step 5: Commit** `ffmpeg: a version folder with one of its programs gone is cleared and fetched again, with a word that antivirus is the usual cause — the rename onto it failed on every retry, forever`

### Task 20: games.json overrides apply on every start (CORE-8)

**Files:**
- Modify: `src-tauri/src/game.rs:184-207`, plus tests at `:585`

**Interfaces:**
- Produces: `fn layered(base: HashMap<String, String>, user: HashMap<String, String>) -> HashMap<String, String>`. It lower-cases both maps first, then puts the user's entries on top.

- [ ] **Step 1: Write the failing test:**

```rust
#[test]
fn a_user_entry_overrides_a_builtin_in_any_case() {
    let base = HashMap::from([("cs2.exe".to_string(), "Counter-Strike 2".to_string())]);
    let user = HashMap::from([("CS2.exe".to_string(), "CS2 (mine)".to_string())]);
    let map = layered(base, user);
    assert_eq!(map.len(), 1);
    assert_eq!(map["cs2.exe"], "CS2 (mine)");
}
```

- [ ] **Step 2: Run** `cargo test --lib game`. Expected: FAIL to compile.
- [ ] **Step 3: Implement.** `games()` builds its map with `layered`.
- [ ] **Step 4: Run** `cargo test --lib`. Expected: all green.
- [ ] **Step 5: Commit** `Game names: an entry in the user's games.json wins over the built-in one whatever its case — before, which of the two applied changed from start to start`

## App UI

### Task 21: The Video page never saves a stand-in screen's limits (UI-4), plus UI unit tests

**Files:**
- Create: `src/lib/fit.ts`, `src/lib/fit.test.ts`
- Modify: `src/routes/Recording.tsx`: `:16-54` (`FPS`, `fpsChoices`, `fitFps` and `fit` move out), `:186-203` (effect)
- Modify: `package.json` (`"test": "node --test \"src/**/*.test.ts\""`), `tsconfig.json` (`"exclude": ["src/**/*.test.ts"]`), `.github/workflows/ci.yml` (`- run: npm test` after `npm run build`)

**Interfaces:**
- Produces, in `fit.ts` (type imports only):
  - `FPS`, `fpsChoices`, `fitFps` and `fit`, moved unchanged.
  - `autoFit(rec: { width: number; height: number; fps: number }, source: CaptureTarget | null, missing: boolean): { width: number; height: number; fps: number } | null`. It returns `null` when there is no source, when the chosen source is missing (the one shown only stands in) and when nothing would change.

- [ ] **Step 1: Write the failing tests:**

```ts
const screen = (width: number, height: number, refreshHz: number) => ({ width, height, refreshHz }) as CaptureTarget;
const rec = { width: 2560, height: 1440, fps: 165 };

test("a screen that only stands in never changes the saved size", () =>
  assert.equal(autoFit(rec, screen(1920, 1080, 60), true), null));
test("the chosen screen still pulls size and rate down to what it has", () =>
  assert.deepEqual(autoFit(rec, screen(1920, 1080, 60), false), { width: 1920, height: 1080, fps: 60 }));
test("a setting that already fits is left alone", () =>
  assert.equal(autoFit(rec, screen(2560, 1440, 165), false), null));
```

- [ ] **Step 2: Run** `npm test` (PowerShell). Expected: FAIL, module `./fit.ts` not found.

- [ ] **Step 3: Implement.** The effect becomes `const next = autoFit(rec, source, missing); if (next) setRec(next);`. `missing` is also true while the list from app start is stale, so the refresh no longer arrives too late.

- [ ] **Step 4: Run** `npm test`, `npx tsc --noEmit` and `npm run build`. Expected: pass.

- [ ] **Step 5: Manual check.** Record a 2560×1440 @ 165 Hz second screen, unplug it and open Video. Expected: the banner shows, and `config.json` keeps 1440p and 165 fps. Plug the screen back in. Expected: capture runs at 1440p165.

- [ ] **Step 6: Commit** `Video page: opening it while the chosen screen or window is away no longer saves the stand-in's resolution and frame rate for good; the UI's pure helpers get unit tests (npm test)`

### Task 22: An open menu owns its keys; viewer shortcuts survive a slider (UI-3, UI-9)

**Files:**
- Create: `src/lib/viewer.ts`, `src/lib/viewer.test.ts`
- Modify: `src/components/ui/Menu.tsx:156-178`, `src/components/ShotViewer.tsx:385-388`, `src/components/ClipPlayer.tsx:470-479`, `src/components/ExportDialog.tsx:72-76`

**Interfaces:**
- Produces, in `viewer.ts` (no imports):
  - `menuOwns(key: string, active: number): boolean`: Escape, ArrowUp and ArrowDown always; Enter only when `active >= 0`.
  - `typesText(target: { tagName?: string; type?: string } | null): boolean`: true for a textarea, or an input that is not `type="range"`.

- [ ] **Step 1: Write the failing tests:**

```ts
test("Escape and the arrows belong to an open menu", () =>
  assert.deepEqual(["Escape", "ArrowUp", "ArrowDown"].map((k) => menuOwns(k, -1)), [true, true, true]));
test("Enter belongs to the menu only with an entry picked", () => {
  assert.equal(menuOwns("Enter", -1), false);
  assert.equal(menuOwns("Enter", 0), true);
});
test("other keys pass on", () => assert.equal(menuOwns("k", 2), false));
test("a slider is not a text field", () => {
  assert.equal(typesText({ tagName: "INPUT", type: "range" }), false);
  assert.equal(typesText({ tagName: "INPUT", type: "text" }), true);
  assert.equal(typesText({ tagName: "TEXTAREA" }), true);
  assert.equal(typesText(null), false);
});
```

- [ ] **Step 2: Run** `npm test`. Expected: FAIL.

- [ ] **Step 3: Implement.**
  - `Menu`'s `onKey` calls `event.stopPropagation()` as well as `preventDefault()` when `menuOwns(...)` is true. Do not use `stopImmediatePropagation`: the lockdown listener sits on `window` in the same phase and must still run.
  - The keydown handlers in `ShotViewer`, `ClipPlayer` and `ExportDialog` start with `if (event.defaultPrevented) return;`.
  - The text-field guards in `ShotViewer` and `ClipPlayer` use `typesText(event.target as …)`.

- [ ] **Step 4: Run** `npm test`, `npx tsc --noEmit` and `npm run build`. Expected: pass.

- [ ] **Step 5: Manual check.**
  - Right-click in the player and press Escape. Expected: only the menu closes.
  - Press ↓ in the menu. Expected: the volume stays.
  - In the screenshot viewer's draw mode, drag the size slider, then press Ctrl+Z. Expected: the last mark is undone.

- [ ] **Step 6: Commit** `Menus: Escape and the arrows stop at an open menu instead of also closing the player or changing its volume; the screenshot viewer's shortcuts keep working after a slider was touched`

### Task 23: The player asks before throwing away trim and mix (UI-V2)

**Files:**
- Modify: `src/lib/viewer.ts`, `src/lib/viewer.test.ts`
- Modify: `src/components/ClipPlayer.tsx`: `:171-205` (`close`), `:504-509` (Escape), `:551-557` (`dirty`), `:669` (backdrop), `:700-701` (✕)

**Interfaces:**
- Produces: `leaving(writing: boolean, dirty: boolean): "wait" | "ask" | "go"`.

- [ ] **Step 1: Write the failing tests:**

```ts
test("unapplied edits are asked about before the player closes", () => assert.equal(leaving(false, true), "ask"));
test("a running write keeps the player open", () => assert.equal(leaving(true, false), "wait"));
test("with nothing changed it just closes", () => assert.equal(leaving(false, false), "go"));
```

- [ ] **Step 2: Run** `npm test`. Expected: FAIL.

- [ ] **Step 3: Implement.**
  - A `dirtyRef` is updated on every render, because `close` is defined above `dirty`.
  - Escape, the backdrop and ✕ go through `requestClose`, which follows `leaving(...)`.
  - `"ask"` sets `askingDiscard`. A `ConfirmDelete` then stands in place of the ✕: `origin="right"`, question "Discard trim and mix?", confirm "Discard changes", cancel "Keep editing". Escape returns early while it is open, because `ConfirmDelete` takes Escape.
  - `removeClip` still calls `close()` directly, since the clip is gone.
  - Paging to another clip (`step`) loses edits the same way. It is unchanged here; mention it to Fabi.

- [ ] **Step 4: Run** `npm test`, `npx tsc --noEmit` and `npm run build`. Expected: pass.

- [ ] **Step 5: Manual check.**
  - Set in and out points, then press Escape. Expected: the question appears, "Keep editing" keeps the edits, and confirming closes.
  - After "Apply", Escape closes without asking.

- [ ] **Step 6: Commit** `Player: closing with trim or mix not yet applied asks first, as the screenshot viewer does — Escape, a click beside the player or ✕ used to throw them away`

### Task 24: Export failures are shown (UI-V1)

**Files:**
- Modify: `src/components/ExportDialog.tsx:97-115` and its render

- [ ] **Step 1: Implement.** Add `const [error, setError] = useState<string | null>(null)`. `run()` clears it at the start, and `catch (err)` does `setError(String(err)); setBusy(false);`. Show the message under the buttons in the error colour. Fix the comment: `export_clip` raises a notice only on success. The decimal size (EDIT-11) stays with `docs/facecam-review.md`.

- [ ] **Step 2: Run** `npx tsc --noEmit` and `npm run build`. Expected: pass.

- [ ] **Step 3: Manual check.**
  - Export to `\\server\share\x.mp4`. Expected: "Export to a .mp4 file on a local drive." stands in the dialog.
  - Delete the clip file in Explorer, then export. Expected: "The clip file is no longer there."

- [ ] **Step 4: Commit** `Export: a failed export says why in the dialog — the core only ever reported success, so every failure went silent`

### Task 25: One clip read at a time, and clearing a game is one command (UI-5)

**Files:**
- Create: `src/lib/coalesce.ts`, `src/lib/coalesce.test.ts`
- Modify: `src/store.ts:309-319` and `:574-597`, `src/lib/ipc.ts` (`clearGame`)
- Modify: `src-tauri/src/commands.rs:735-738` (`list_clips` becomes `#[tauri::command(async)]`), plus a new `clear_game` next to `file_clip`
- Modify: `src-tauri/src/clips.rs` (`Library::clear_game`, plus a test), `src-tauri/src/lib.rs` (`generate_handler!`), `src-tauri/build.rs`, `src-tauri/capabilities/default.json`

**Interfaces:**
- Produces:
  - `coalesce(run: () => Promise<unknown>): () => void`. At most one run is in flight; any calls meanwhile cause exactly one more run after it. A failed run does not block the next.
  - `Library::clear_game(&self, game: &str) -> rusqlite::Result<Vec<Clip>>` clears the game from every clip with exactly that game, in one transaction, and returns those clips as they now are.
  - `#[tauri::command(async)] pub fn clear_game(state, app, game: String) -> Result<()>` clears, files each clip the way `file_clip` does, and calls `announce_change` once.
  - `api.clearGame(game: string): Promise<void>`.

- [ ] **Step 1: Write the failing tests:**

```ts
test("a burst of changes makes one read and one more after it", async () => {
  let runs = 0;
  let release!: () => void;
  const reread = coalesce(() => { runs++; return new Promise<void>((r) => (release = r)); });
  reread(); reread(); reread();
  assert.equal(runs, 1);
  release(); await new Promise((r) => setImmediate(r));
  assert.equal(runs, 2);
  release(); await new Promise((r) => setImmediate(r));
  assert.equal(runs, 2);
});
test("a failed read does not stop the next one", async () => {
  let runs = 0;
  const reread = coalesce(() => { runs++; return Promise.reject(new Error("x")); });
  reread(); await new Promise((r) => setImmediate(r));
  reread(); await new Promise((r) => setImmediate(r));
  assert.equal(runs, 2);
});
```

```rust
#[test]
fn clearing_a_game_takes_it_off_its_clips_only() {
    // a and b with game "Chrome", c with "CS2", via the module's clip() helper
    let cleared = lib.clear_game("Chrome").unwrap();
    assert_eq!(cleared.len(), 2);
    assert!(lib.list().unwrap().iter().all(|c| c.game.as_deref() != Some("Chrome")));
    assert_eq!(lib.get("c").unwrap().unwrap().game.as_deref(), Some("CS2"));
}
```

- [ ] **Step 2: Run** `npm test` and `cargo test --lib clips`. Expected: both FAIL.

- [ ] **Step 3: Implement.**
  - The store's `clips-changed` handler becomes `coalesce(async () => set({ clips: await api.listClips() }))`, and it swallows errors as now. The ticket goes, because reads no longer overlap.
  - `clearGame` keeps its optimistic update, then calls `await api.clearGame(game)` instead of the per-clip loop.
  - Register the command in `lib.rs`, `build.rs` and `default.json`.

- [ ] **Step 4: Run** `npm test`, `cargo test --lib`, `npx tsc --noEmit` and `npm run build`. Expected: pass.

- [ ] **Step 5: Manual check.** Remove a game chip that covers about 200 clips. Expected: the chip is gone at once, does not flicker back, and the window stays responsive.

- [ ] **Step 6: Commit** `Gallery: a run of clip changes makes one read of the list plus one after it, off the main thread, and removing a game chip is one command instead of two events per clip`

### Task 26: A render error shows a way back instead of a blank window (UI-2)

**Files:**
- Create: `src/components/ErrorBoundary.tsx`
- Modify: `src/main.tsx:11-15`, `src/overlay/main.tsx:12-16`, `src/console/main.tsx:12-16`, `src/App.tsx:78-89`, and `src/routes/Clips.tsx` (around the viewers, `:654-660`)
- Modify: `src/lib/ipc.ts` (`uiError`), `src-tauri/src/commands.rs` (`ui_error`), `src-tauri/src/lib.rs`, `src-tauri/build.rs`, and `src-tauri/capabilities/default.json`, `overlay.json`, `console.json`

**Interfaces:**
- Produces:
  - `<ErrorBoundary onBack?: () => void>`, a class component.
    - Its fallback says "Something broke here." It offers "Reload window" (`location.reload()`; the lockdown blocks only the keys) and, when `onBack` is given, "Back".
    - `componentDidCatch` reports to the core.
  - `#[tauri::command] pub fn ui_error(window: tauri::Window, message: String)` logs `log::error!("ui {}: {}", window.label(), <first 4000 chars>)`.
  - `api.uiError(message: string): Promise<void>`.

- [ ] **Step 1: Implement.**
  - `App` wraps the route in `<ErrorBoundary key={route} onBack={() => setRoute("dashboard")}>`.
  - `Clips` wraps each viewer, and its `onBack` closes the viewer.
  - The overlay and console roots are wrapped too.
  - All three `createRoot` calls pass `onUncaughtError` and `onCaughtError`, which send the error and its component stack to `api.uiError`.
  - Each root adds a `window` `unhandledrejection` listener that logs the same way.

- [ ] **Step 2: Run** `npx tsc --noEmit`, `cargo test --lib` and `npm run build`. Expected: pass.

- [ ] **Step 3: Manual check.** In a dev build, temporarily add `throw new Error("test")` to `Dashboard`. Expected: the fallback shows instead of a blank window, "Reload window" works, and `clippiboy.log` holds `ui main: Error: test`. Remove the throw.

- [ ] **Step 4: Commit** `UI: a render error shows a way back (reload, back to the overview) instead of a blank window that only Quit could fix, and goes into the log`

### Task 27: Mixer label and sliders write once; nothing is written before the real config arrived (UI-6, UI-12)

**Files:**
- Modify: `src/routes/AudioMixer.tsx:333-340` and `:384-391`, `src/routes/Recording.tsx:437-454`
- Modify: `src/store.ts:371-445` (`patchConfig`, `setHotkeys`, `upsertSource`, `removeSource`)

- [ ] **Step 1: Implement.**
  - A `LabelField` keeps a local draft. It saves on blur or Enter, and Escape puts the name back, as `NameField` in `Clips.tsx` does. Its `aria-label` is `` `Name of ${label}` ``.
  - The gain slider and the clip-length slider show a draft while dragged and write in `onCommit`, as the buffer sliders do.
  - `upsertSource` and `removeSource` take a ticket on `configWrites` the way `patchConfig` does, and a `catch` that sets `lastError`.
  - `patchConfig`, `setHotkeys`, `upsertSource` and `removeSource` start with `if (inTauri && !get().ready) return;`. Until `init` finishes, the store still holds the mock config.

- [ ] **Step 2: Run** `npx tsc --noEmit` and `npm run build`. Expected: pass.

- [ ] **Step 3: Manual check.**
  - Type a source name quickly. Expected: no characters are lost, and `config.json` changes once, on blur.
  - Drag a gain slider. Expected: one write, on release.
  - Click a mixer toggle during the first second after a start. Expected: no mock source appears in `config.json`.

- [ ] **Step 4: Commit** `Mixer and Video page: a source's name is saved when the field is left and sliders write when let go, not on every keystroke or step; writes made before the settings were read are dropped instead of sending the placeholder config`
