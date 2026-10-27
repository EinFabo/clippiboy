# ClippiBoy friends server

A Cloudflare Worker behind the Friends page: sign-in with Discord, friend lists
in D1, and one Durable Object per user (`Hub`) that holds the app's live
connection and passes presence — online, which game, since when — to friends.

Live at `https://api.clippiboy.com`; the old `clippiboy-friends.<account>.workers.dev`
stays on for builds from before the domain. The app talks to it from
`src-tauri/src/friends.rs`.

## Deploy

```sh
npm install
npm run deploy        # applies migrations/ to D1, then deploys
```

## Discord

Sign-in needs a Discord application (https://discord.com/developers/applications):

1. OAuth2 → Redirects, both of them: `https://api.clippiboy.com/auth/callback` and
   `https://clippiboy-friends.<account>.workers.dev/auth/callback`
2. The Client ID goes into `vars.DISCORD_CLIENT_ID` in `wrangler.jsonc`.
3. The Client Secret: `npx wrangler secret put DISCORD_CLIENT_SECRET`
4. `npm run types && npm run deploy`

Until then `/auth/start` answers 503 and the app shows the error.

## Deleted accounts

Deleting an account keeps, for seven days, a fingerprint of its Discord id
(HMAC-SHA256 under a secret pepper) with the times of its links that still
count against the week and a suspension if one stood (`departed`, migration
0010). Signing in again with that Discord account takes them over, so deleting
and coming back neither starts the weekly link limit over nor lifts a
suspension. The hourly cron deletes rows older than seven days.

The pepper is a secret of its own; set it once before deploying, and keep it —
a new one makes every row left so far unmatchable:

```sh
openssl rand -base64 32 | npx wrangler secret put DEPARTED_PEPPER
```

Without it accounts still sign in and delete as before, but nothing outlives a
deletion (the log says so). For `wrangler dev`, put a local value in
`.dev.vars` (ignored by git): `DEPARTED_PEPPER="local-dev-pepper-not-a-secret"`.

## How sign-in works

The app opens `/auth/start?challenge=…` with the hash of a verifier it keeps.
Discord returns the browser to `/auth/callback`, which hands the app a one-time
code through `clippiboy://auth?code=…`. Only the app holding the verifier can
trade that code for a session at `/auth/exchange`, and a cookie ties the
callback to the browser that started the login. Sessions are stored as hashes
and expire after 180 days without use.

## Test

`scripts/e2e.mjs` runs the whole flow with two seeded test accounts — see the
comment at the top.

Against a local `wrangler dev` instead of the deployed worker:

```sh
npx wrangler d1 migrations apply clippiboy-friends --local
node scripts/e2e.mjs --seed > seed.sql && npx wrangler d1 execute clippiboy-friends --local --file seed.sql
npx wrangler dev --port 8799 &
FRIENDS_SERVER=http://127.0.0.1:8799 node scripts/e2e.mjs
```

## Relay

Besides presence, the socket carries `{t: "relay", to, body}`: the hub passes
`body` on to that friend's apps as `{t: "relay", from, body}` and keeps nothing.
It is the handshake for sending a clip (`src-tauri/src/share.rs`) — the clip
itself goes PC to PC over iroh, never through here. Only accepted friends, at
most 4 KB and 30 messages per 10 s per user (all connections together); otherwise, or when the
friend has no app online, the sender gets `{t: "relayFailed", to, ref, reason}`.

A user has at most four connections (a fifth closes the oldest with 4002),
and raw messages over 8 KB are dropped unread. Presence is published at most
ten at once and then one per 30 s, the last change always; hellos are
limited to five at once and then one per minute (the connection closes with
4003). Both budgets are kept in the hub's storage, so they hold across its sleep.

Presence also carries `status` (a line of the user's own, cut to 60 characters)
and `busy`; going offline stamps `lastSeen` in the user's hub, which snapshots
and presence messages pass on.
