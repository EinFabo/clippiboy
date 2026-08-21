"""Generates every icon size from `src-tauri/icons/icon.png` (1024x1024, RGBA).

Run from the project root: `python3 scripts/make-icons.py`

The tray icon is a round crop of the mark without the wordmark —
at 16 px "Clippiboy" would be nothing but a smudge. The logo in the title bar is
independent of this: `src/assets/logo.svg` is drawn by hand and stays sharp at
any size.
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
    round_mark(master, 256).save(ICONS + "tray.png")


if __name__ == "__main__":
    main()
