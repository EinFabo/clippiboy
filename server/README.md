# ClippiBoy friends server

A Cloudflare Worker behind the Friends page: sign-in with Discord, friend lists
in D1, and one Durable Object per user (`Hub`) that holds the app's live
connection and passes presence — online, which game, since when — to friends.

Live at `https://api.clippiboy.com`; the old `clippiboy-friends.fabian081964.workers.dev`
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
   `https://clippiboy-friends.fabian081964.workers.dev/auth/callback`
2. The Client ID goes into `vars.DISCORD_CLIENT_ID` in `wrangler.jsonc`.
3. The Client Secret: `npx wrangler secret put DISCORD_CLIENT_SECRET`
4. `npm run types && npm run deploy`

Until then `/auth/start` answers 503 and the app shows the error.

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
most 4 KB and 30 messages per 10 s per connection; otherwise, or when the
friend has no app online, the sender gets `{t: "relayFailed", to, ref, reason}`.

Presence also carries `status` (a line of the user's own, cut to 60 characters)
and `busy`; going offline stamps `lastSeen` in the user's hub, which snapshots
and presence messages pass on.
