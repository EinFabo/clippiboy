"""Draws the pictures for the Stream Deck plugin's keys.

Run from the project root: `python3 scripts/make-streamdeck-icons.py`

Deliberately its own script and not part of `make-icons.py`: that one rewrites
the app's whole icon set, and nobody wanting a new key picture should have to
touch the tray icon to get it.

The keys do not get the app icon shrunk down. Three keys carrying the same logo
are indistinguishable at arm's length on a lit desk, which is exactly the
distance a Stream Deck is used from — so each one gets a glyph of its own. Only
the plugin's own picture in the action list is the mark, because there it stands
for the app rather than for a key.
"""

import os

from PIL import Image, ImageDraw

MASTER = "src-tauri/icons/icon.png"
OUT = "streamdeck/com.einfabo.clippiboy.sdPlugin/imgs/"

# The brand purple, and a lighter tone for whatever sits in front of it. Both are
# picked for black: a Stream Deck key is unlit glass around the picture.
PURPLE = (139, 92, 246, 255)
LIGHT = (196, 181, 253, 255)
GREY = (95, 89, 112, 255)
# What a lens looks through.
DARK = (26, 21, 35, 255)

# Everything is drawn this many times too large and scaled down afterwards — PIL
# draws no anti-aliased edges of its own.
OVERSAMPLE = 8

# How much of the key the glyph takes when the title needs room underneath.
TITLE_ROOM = 0.74


def draw_save(art: ImageDraw.ImageDraw, n: float) -> None:
    """An arrow onto a line: keep this."""
    art.rounded_rectangle((0.42 * n, 0.16 * n, 0.58 * n, 0.52 * n), 0.03 * n, fill=LIGHT)
    art.polygon([(0.28 * n, 0.44 * n), (0.72 * n, 0.44 * n), (0.50 * n, 0.72 * n)], fill=LIGHT)
    art.rounded_rectangle((0.24 * n, 0.80 * n, 0.76 * n, 0.88 * n), 0.04 * n, fill=PURPLE)


def draw_screenshot(art: ImageDraw.ImageDraw, n: float) -> None:
    """A camera, seen from the front."""
    art.rounded_rectangle((0.30 * n, 0.20 * n, 0.58 * n, 0.36 * n), 0.04 * n, fill=PURPLE)
    art.rounded_rectangle((0.10 * n, 0.30 * n, 0.90 * n, 0.82 * n), 0.11 * n, fill=PURPLE)
    centre, lens = (0.50 * n, 0.56 * n), 0.19 * n
    art.ellipse((centre[0] - lens, centre[1] - lens, centre[0] + lens, centre[1] + lens), fill=DARK)
    inner = 0.13 * n
    art.ellipse(
        (centre[0] - inner, centre[1] - inner, centre[0] + inner, centre[1] + inner), fill=LIGHT
    )
    flash = 0.035 * n
    art.ellipse((0.74 * n - flash, 0.40 * n - flash, 0.74 * n + flash, 0.40 * n + flash), fill=LIGHT)


def draw_buffer(art: ImageDraw.ImageDraw, n: float, running: bool) -> None:
    """The recording dot — filled while the buffer runs, an empty ring when not."""
    colour = PURPLE if running else GREY
    ring = 0.36 * n
    art.ellipse(
        (0.5 * n - ring, 0.5 * n - ring, 0.5 * n + ring, 0.5 * n + ring),
        outline=colour,
        width=int(0.07 * n),
    )
    if not running:
        return
    dot = 0.20 * n
    art.ellipse((0.5 * n - dot, 0.5 * n - dot, 0.5 * n + dot, 0.5 * n + dot), fill=LIGHT)


# Which glyph goes in which file, and whether the bottom of the picture has to
# stay clear. It does on the keys themselves: the buffer level is written across
# the lower quarter, and a glyph drawn into it would show through the digits. The
# small pictures for the action list carry no writing and use the whole square.
#
# Every one is written twice: `x.png` at the plain size and `x@2x.png` at double,
# which is what the Stream Deck software expects to find beside it.
KEYS = [
    ("actions/save/key", 72, draw_save, True),
    ("actions/save/icon", 20, draw_save, False),
    ("actions/screenshot/key", 72, draw_screenshot, True),
    ("actions/screenshot/icon", 20, draw_screenshot, False),
    ("actions/buffer/on", 72, lambda art, n: draw_buffer(art, n, True), True),
    ("actions/buffer/off", 72, lambda art, n: draw_buffer(art, n, False), True),
    ("actions/buffer/icon", 20, lambda art, n: draw_buffer(art, n, True), False),
]


def glyph(draw, size: int, headroom: bool) -> Image.Image:
    big = size * OVERSAMPLE
    canvas = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    draw(ImageDraw.Draw(canvas), float(big))
    if not headroom:
        return canvas.resize((size, size), Image.LANCZOS)

    shrunk = int(size * TITLE_ROOM)
    framed = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    framed.paste(canvas.resize((shrunk, shrunk), Image.LANCZOS), ((size - shrunk) // 2, 0))
    return framed


def main() -> None:
    for name, size, draw, headroom in KEYS:
        os.makedirs(os.path.dirname(OUT + name), exist_ok=True)
        glyph(draw, size, headroom).save(f"{OUT}{name}.png")
        glyph(draw, size * 2, headroom).save(f"{OUT}{name}@2x.png")

    # The plugin itself is the app, so here the mark does belong.
    master = Image.open(MASTER).convert("RGBA")
    os.makedirs(OUT + "plugin", exist_ok=True)
    for name, size in [("marketplace", 256), ("category", 28)]:
        master.resize((size, size), Image.LANCZOS).save(f"{OUT}plugin/{name}.png")
        master.resize((size * 2, size * 2), Image.LANCZOS).save(f"{OUT}plugin/{name}@2x.png")


if __name__ == "__main__":
    main()
