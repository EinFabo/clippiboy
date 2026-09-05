# ClippiBoy for Stream Deck

Three keys — save a clip, take a screenshot, switch the replay buffer — that show
what ClippiBoy is doing rather than only poking at it.

For users there is nothing to build: the finished
`com.einfabo.clippiboy.streamDeckPlugin` hangs at every
[release](../../releases) and a double-click installs it. This file is for
working on the plugin.

## How it talks to ClippiBoy

ClippiBoy runs a small HTTP port on `127.0.0.1` (`src-tauri/src/control.rs`) and
writes port and token to `%APPDATA%\ClippiBoy\streamdeck.json`. The plugin reads
that file, so pairing needs no setup at all — both programs run on this machine
under this user, and a step nobody has to perform is a step nobody can get wrong.
Entering both by hand in the property inspector is the fallback for when the file
is not there.

One poller serves every key: `GET /v1/status` twice a second while at least one
ClippiBoy key is on a visible page, and nothing at all when none is. A press goes
to `POST /v1/clip`, `/v1/screenshot` or `/v1/buffer` and only answers once the
work is done — which is what lets the key show a checkmark or a cross.

## Installing it here

```
powershell -ExecutionPolicy Bypass -File streamdeck\install.ps1
```

Fetches what is missing, compiles, and copies the folder into
`%APPDATA%\Elgato\StreamDeck\Plugins\`. The Stream Deck software is closed and
started again on the way — it reads that directory once at startup and holds the
running plugin open, so there is no way around it.

The `.streamDeckPlugin` file from a release is the other route: double-click,
confirm, done. Right for installing once, wrong for changing a line and wanting
to see it — hence the script.

Either way the keys show a dash until ClippiBoy itself is running **with the
control port on**, under *Settings → Stream Deck*. A ClippiBoy older than that
setting has no port at all; the plugin needs the version this folder ships with.

## Building

```
npm run setup     # both installs, see below
npm run build     # TypeScript into com.einfabo.clippiboy.sdPlugin/bin
npm run pack      # the installable .streamDeckPlugin into dist/
npm run link      # register this folder with the Stream Deck software
```

Two things here look odd on purpose.

**Two `package.json`.** The `.sdPlugin` folder is what gets zipped and shipped,
so what the plugin needs *at runtime* — `@elgato/streamdeck` — has to be
installed inside it. The outer one carries the build tools, which stay behind.
`tsconfig.json` has a `paths` entry so the compiler finds the SDK in the inner
one.

**No bundler.** The usual template bundles with rollup, and rollup ships a
different binary per platform. This repository is built from PowerShell but
edited from WSL, and an `npm install` from the wrong side then breaks the build
with `Cannot find module` — the same trap the root project already has with the
Tauri CLI. `tsc` is plain JavaScript and does not care which side installed it,
and the packed result is the same either way.

## Testing

After `install.ps1` the loop is `npm run build` plus
`npx streamdeck restart com.einfabo.clippiboy` — the Stream Deck software does
not have to close for that, only the plugin process. The plugin's own log lands
next to the software's, under `%APPDATA%\Elgato\StreamDeck\logs\`.

Worth checking by hand, because none of it shows up in a compiler:

- the buffer key follows along when the buffer is switched by hotkey or from the
  tray, not just when the key itself was pressed
- quitting ClippiBoy leaves a dash on the keys and no flood in the log, and
  starting it again picks them back up on its own
- a second press while a clip is being written gives a cross rather than a second
  clip — ClippiBoy refuses it, and the key has to say so
