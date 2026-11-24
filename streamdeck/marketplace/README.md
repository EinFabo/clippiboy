# Elgato Marketplace listing

Everything the Maker Console asks for when the plugin is submitted.

| What | Where | Rule it meets |
|---|---|---|
| Plugin file | `../dist/com.einfabo.clippiboy.streamDeckPlugin` (`npm run pack`) | passes `streamdeck validate` |
| Thumbnail | `out/thumbnail.png` | PNG 1920 × 960, static, English |
| Gallery (4) | `out/gallery-1-lengths.png` … `out/gallery-4-actions.png` | PNG 1920 × 960, 3–10 items |
| Marketplace icon | `out/app-icon-288.png` | PNG 288 × 288, static, no text or URL |
| Description | `description.txt` | 250–1500 characters, English, requirement named in the first lines |

The images come from `slides.html` and are rendered with `python3 render.py` (Pillow plus a
Chromium). The keys in them use the plugin's own images and the titles the plugin really
writes, so the pictures show what a user gets.

## Listing fields

- **Name:** ClippiBoy
- **Category:** Gaming (Streaming as second choice, if the console allows two)
- **Platforms:** Windows 10, Windows 11. Stream Deck 6.9 or newer. Keypad keys only (no dials or touch strip).
- **Price:** Free
- **Version:** 1.0.0.0

## Links

- Website / setup guide: https://clippiboy.com
- Support: https://github.com/EinFabo/clippiboy/issues
- Privacy policy: https://clippiboy.com/privacy.html (deletion requests: privacy@clippiboy.com)
- Source: https://github.com/EinFabo/clippiboy

## Release notes — 1.0.0

First release on the Marketplace.

- Save Clip, Screenshot, Replay Buffer and Recording keys
- Per-key clip length for Save Clip, with the length a press saves shown on the key
- Finds ClippiBoy by itself; the key settings show whether it is reachable, check again on their own, and link to clippiboy.com if ClippiBoy is not installed yet
- New monochrome icons in the action list
