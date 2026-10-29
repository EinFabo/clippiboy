# Extras (not asked for)

Everything in this folder goes beyond the brief. The brief's own results are elsewhere and stand on their own:

- `docs/superpowers/specs/2026-10-20-facecam-design.md` — the design, sections 2–5 filled in, with the decisions that are yours under "Open for Fabi"
- `docs/superpowers/plans/2026-10-22-facecam-stage-*.md` — one plan per stage (1 and 2 in detail, 3–6 rough)
- `docs/facecam-review.md` — the review of the code the facecam touches

Nothing here is needed for those. Take what is useful and delete the rest; every file stands alone. They came in their own commit, so `git revert` of that commit removes them all at once.

## What is here

| File | What it is | Worth it if you… |
|---|---|---|
| `review-beyond-facecam.md` | The same kind of review for the rest of the codebase: core, Stream Deck, friends and sharing, the server, the app UI. 36 findings, none high, a "worth doing first" table, 28 improvement ideas. | want to know what else is broken |
| `plans/2026-10-22-fix-sync-and-timestamps.md` | A plan for the confirmed timing bugs: silent clip starts, A/V shift around stalls, sleep during recordings, the 21 ms AAC offset, the mixer's slow drift. | want the review's sync findings fixed before or alongside the facecam |
| `plans/2026-10-22-fix-data-safety-and-silent-failures.md` | A plan for the bugs that lose data or fail silently: the config file, exports onto the clip itself, leftovers, encoder and recorder failures nobody hears about, the console's missing permission. | want the review's robustness findings fixed |
| `plans/2026-10-22-fix-beyond-facecam.md` | A plan for the most useful items of `review-beyond-facecam.md`. | want those fixed |
| `how-time-works.md` | How the picture's clock, the audio clock, the cut and the muxer's timestamps work today, with diagrams, where it goes wrong, and where the cam's clock slots in. | implement the facecam or the sync fixes |
| `facecam-test-checklist.md` | A manual test checklist for stages 2–5 and a hardware matrix (GPUs, cameras, Windows versions, Discord/Teams/OBS), plus a "must not change with the cam off" list. | test a stage before release |
| `ideas/facecam-ideas-for-later.md` | Rough designs for the spec's "Ideas for later" and a few more (auto sync from a clap, layout presets, background blur without an NPU, per-game position), ranked by value and effort. | plan what comes after stage 6 |
| `research/camera-sharing.md` | Sourced research: which Windows APIs share a webcam, how Discord probably captures, the Windows 11 multi-app switch, the stage-1 test protocol. | want to check a claim in spec section 1 notes or stage 1 |
| `research/camera-capture-and-clocks.md` | Sourced research: Media Foundation capture in Rust, timestamps, MJPEG, a second encoder, privacy errors. | implement the camera source |
| `research/virtual-camera.md` | Sourced research: `MFCreateVirtualCamera`, Frame Server, registration, the shared-memory protocol in full, the Windows 10 DirectShow path, licences. | implement stage 5 |
| `research/ffmpeg-compositing.md` | The filter-graph cookbook: corner overlay, masks, portrait, time alignment, GPU paths in the pinned build, speed estimates, encoder flags. | implement compositing or the portrait export |

## How it was made

Read-only. Nothing in the repository was built, run or tested, and no production code was changed. Each review finding was checked by a second, independent reviewer who tried to disprove it; findings that only hold under certain runtime conditions say so. The research used the web and is sourced claim by claim; untested filter graphs and estimates are marked as such. Line numbers are those of commit 401a320.
