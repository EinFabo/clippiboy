# ClippiBoy — diagnostic report

A program that runs once and writes everything needed for debugging into **one**
file on the desktop: `clippiboy-report.txt`.

Meant for the case where the person who sees the bug is not the person who can
read a log. They run it and send back the one file.

```
npm run report
```

builds `tools/report/target/release/clippiboy-report.exe` — a single exe of
about 290 KB, with no dependencies, built in a few seconds. It can be sent
around; it needs nothing else on the target machine.

## What it contains

- System, graphics cards with driver date, displays with their arrangement
- Installed version, whether it is running right now and whether it responds
- Autostart entry in the registry and in the startup folder
- `config.json` in full — **without** the Stream Deck token
- Which ffmpeg version was fetched
- All crashes and hangs from the Windows event log of the last 30 days
- **Whether the picture froze** — see below
- The logs: all event lines, plus the last pipeline lines

## The question of the frozen picture

Windows.Graphics.Capture only delivers a frame when something on the screen has
changed. A display on which nothing happens therefore produces nothing but
repeats for minutes — perfectly normal, especially when recording and working
happen on different displays.

The difference is not in standing still but in **never coming back**. A
recording stuck on a dead display never recovers; its stretch runs to the end
of the log. That is exactly what the report says in its last line for each
log — everything above it is just material.

## It only reads

Nothing is installed, nothing is changed, nothing is sent. The file lands on
the desktop, and whoever created it decides what happens with it. The control
port's token is removed along the way: a key that travels through a chat is a
key given away.
