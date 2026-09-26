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
