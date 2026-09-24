"""Draws the six buffer images: the rewind pair over a bar.

    python3 tools/buffer-icons.py        # from streamdeck/, needs Pillow

The key has to say from two metres whether the buffer runs, so the two states
differ in weight and not only in colour: off is a hollow grey pair over a
hairline, on a solid violet pair over a full bar. Round is left to the record
key - its red dot was what the old ring got confused with.

Everything is laid out on a 144-wide canvas and rendered at four times that
before being shrunk down, which is where the smooth edges come from. The motif
sits in the upper two thirds because the key carries the buffer level as text
along its bottom edge (TitleAlignment "bottom" in the manifest).
"""

from PIL import Image, ImageDraw

S = 4  # supersampling

ACCENT = (139, 92, 246, 255)   # --color-accent   #8b5cf6
BRIGHT = (196, 181, 253, 255)  # --color-accent-bright #c4b5fd
MUTED = (95, 89, 112, 255)     # the grey the old off state used


def chevron(x_left, x_right, y_top, y_bot):
    """One left-pointing triangle, as a closed path.

    The first corner comes round a second time at the end: PIL rounds the joints
    between segments, not the spot where the path closes, and without the extra
    segment a notch stays behind in the top right corner.
    """
    y_mid = (y_top + y_bot) / 2
    pts = [(x_right, y_top), (x_left, y_mid), (x_right, y_bot)]
    return pts + [pts[0], pts[1]]


def draw_motif(size, *, filled, colour, bar_colour, bar_height, box):
    """box = (x0, y0, x1, y1) for the chevron pair; the bar hangs below it."""
    im = Image.new("RGBA", (size * S, size * S), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    x0, y0, x1, y1 = box
    gap = (x1 - x0) * 0.07
    half = (x1 - x0 - gap) / 2
    round_to = max(2, round((y1 - y0) * 0.16))   # rounds the corners
    stroke = max(2, round((y1 - y0) * 0.20))     # the hollow state's line

    # A stroke sits half outside the path it follows, so the path is pulled in by
    # that half - otherwise the motif grows past the box it was given.
    pad = (round_to if filled else stroke) / 2

    for start in (x0, x0 + half + gap):
        pts = [
            (p[0] * S, p[1] * S)
            for p in chevron(start + pad, start + half - pad, y0 + pad, y1 - pad)
        ]
        if filled:
            d.polygon(pts, fill=colour)
            d.line(pts, fill=colour, width=round_to * S, joint="curve")
        else:
            d.line(pts, fill=colour, width=stroke * S, joint="curve")

    if bar_height:
        gap_below = (y1 - y0) * 0.19
        by0 = y1 + gap_below
        by1 = by0 + bar_height
        r = bar_height / 2
        d.rounded_rectangle(
            [x0 * S, by0 * S, x1 * S, by1 * S], radius=r * S, fill=bar_colour
        )

    return im.resize((size, size), Image.LANCZOS)


def key(size, state):
    """72/144: the key itself. Scale everything off the 144 design."""
    k = size / 144
    box = (31 * k, 16 * k, 113 * k, 72 * k)
    if state == "on":
        return draw_motif(size, filled=True, colour=BRIGHT, bar_colour=ACCENT,
                          bar_height=12 * k, box=box)
    return draw_motif(size, filled=False, colour=MUTED, bar_colour=MUTED,
                      bar_height=4 * k, box=box)


def listicon(size):
    """20/40: the action list. One picture, no states, so the full colour."""
    k = size / 40
    box = (5 * k, 6 * k, 35 * k, 26 * k)
    return draw_motif(size, filled=True, colour=ACCENT, bar_colour=BRIGHT,
                      bar_height=4 * k, box=box)


if __name__ == "__main__":
    import sys
    out = sys.argv[1] if len(sys.argv) > 1 else \
        "com.einfabo.clippiboy.sdPlugin/imgs/actions/buffer"
    key(72, "on").save(f"{out}/on.png")
    key(144, "on").save(f"{out}/on@2x.png")
    key(72, "off").save(f"{out}/off.png")
    key(144, "off").save(f"{out}/off@2x.png")
    listicon(20).save(f"{out}/icon.png")
    listicon(40).save(f"{out}/icon@2x.png")
    print("written to", out)
