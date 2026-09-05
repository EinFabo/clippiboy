"""Generates every icon size from `src-tauri/icons/icon.png` (1024x1024, RGBA).

Run from the project root: `python3 scripts/make-icons.py`

The tray icon is the same picture as the rest, only at 256 px — the mark already
sits on a round dark ground, so at 16 px it reads without any further cropping.
`round_mark` below cuts the bare mark out and is what the tray icon used to be;
its `MARK` no longer matches the master that is in the repo today, so running it
would cut the mark off at the edges. It is kept because the crop itself is still
useful, not because anything uses it.

The logo in the title bar is independent of all this: `src/assets/logo.svg` is
drawn by hand and stays sharp at any size.

The Stream Deck plugin's key pictures have a script of their own,
`make-streamdeck-icons.py` — running this one must not depend on it, and the
other way round.
"""

from PIL import Image, ImageDraw

MASTER = "src-tauri/icons/icon.png"
ICONS = "src-tauri/icons/"
# Crop of the mark within the 1024 master.
MARK = (185, 60, 755, 630)


def round_mark(master: Image.Image, size: int) -> Image.Image:
    crop = master.crop(MARK)
    # The mask is supersampled 4x, otherwise the edge frays.
    mask = Image.new("L", (crop.width * 4, crop.height * 4), 0)
    ImageDraw.Draw(mask).ellipse((0, 0, mask.width - 1, mask.height - 1), fill=255)
    crop.putalpha(mask.resize(crop.size, Image.LANCZOS))
    return crop.resize((size, size), Image.LANCZOS)


def main() -> None:
    master = Image.open(MASTER).convert("RGBA")
    for size, name in [
        (32, "32x32.png"),
        (128, "128x128.png"),
        (256, "128x128@2x.png"),
        (256, "256x256.png"),
        (512, "512x512.png"),
    ]:
        master.resize((size, size), Image.LANCZOS).save(ICONS + name)
    master.save(
        ICONS + "icon.ico",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    # Deliberately not `round_mark`: see the note at the top.
    master.resize((256, 256), Image.LANCZOS).save(ICONS + "tray.png")


if __name__ == "__main__":
    main()
