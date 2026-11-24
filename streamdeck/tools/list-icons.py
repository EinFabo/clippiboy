"""Renders the action-list and category icons from tools/list-icons/*.svg.

    python3 tools/list-icons.py        # from streamdeck/, needs Pillow and a Chromium

Elgato wants these monochrome: a white (#FFFFFF) line on a transparent ground,
no colour and no backdrop - the Stream Deck app tints them itself. The coloured
images stay on the keys (key.png, on.png, off.png), where colour is allowed.

Each SVG is rendered once at 512 px by headless Chromium and shrunk from there,
so the 20 and 28 px versions come out smooth instead of pixel-stepped.
"""

import glob
import os
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent
PLUGIN = HERE.parent / "com.einfabo.clippiboy.sdPlugin" / "imgs"

# svg name -> (target without extension, 1x size)
TARGETS = {
    "save": ("actions/save/icon", 20),
    "screenshot": ("actions/screenshot/icon", 20),
    "buffer": ("actions/buffer/icon", 20),
    "record": ("actions/record/icon", 20),
    "category": ("plugin/category", 28),
}

BIG = 512


def chromium() -> str:
    found = os.environ.get("CHROMIUM") or next(
        iter(sorted(glob.glob(os.path.expanduser("~/.cache/ms-playwright/chromium-*/chrome-linux/chrome")))), None
    )
    if not found:
        sys.exit("no Chromium found - set CHROMIUM to its path")
    return found


def render(svg: Path, out: Path) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        page = Path(tmp) / "page.html"
        page.write_text(
            "<!doctype html><style>html,body{margin:0;background:transparent}"
            f"img{{display:block;width:{BIG}px;height:{BIG}px}}</style>"
            f'<img src="{svg.as_uri()}">',
            encoding="utf-8",
        )
        subprocess.run(
            [
                chromium(),
                "--headless",
                "--disable-gpu",
                "--hide-scrollbars",
                "--default-background-color=00000000",
                f"--window-size={BIG},{BIG}",
                f"--screenshot={out}",
                page.as_uri(),
            ],
            check=True,
            capture_output=True,
        )


def main() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        for name, (target, size) in TARGETS.items():
            big = Path(tmp) / f"{name}.png"
            render(HERE / "list-icons" / f"{name}.svg", big)
            image = Image.open(big).convert("RGBA").crop((0, 0, BIG, BIG))
            # Whatever Chromium anti-aliased stays white; only the alpha carries the shape.
            alpha = image.getchannel("A")
            white = Image.new("RGBA", image.size, (255, 255, 255, 0))
            white.putalpha(alpha)
            for scale, suffix in ((1, ""), (2, "@2x")):
                dest = PLUGIN / f"{target}{suffix}.png"
                white.resize((size * scale, size * scale), Image.LANCZOS).save(dest)
                print(dest.relative_to(HERE.parent))


if __name__ == "__main__":
    main()
