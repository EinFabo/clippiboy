"""Photographs slides.html into the Marketplace images (1920x960) and the app icon (288x288).

    python3 streamdeck/marketplace/render.py     # needs Pillow and a Chromium

Chromium comes from CHROMIUM or Playwright's cache.
"""

import glob
import os
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent
OUT = HERE / "out"
SLIDES = {"thumb": "thumbnail.png", "lengths": "gallery-1-lengths.png", "states": "gallery-2-states.png",
          "setup": "gallery-3-setup.png", "actions": "gallery-4-actions.png"}


def chromium() -> str:
    found = os.environ.get("CHROMIUM") or next(
        iter(sorted(glob.glob(os.path.expanduser("~/.cache/ms-playwright/chromium-*/chrome-linux/chrome")))), None)
    if not found:
        sys.exit("no Chromium found - set CHROMIUM to its path")
    return found


def shoot(url: str, out: Path, w: int, h: int, transparent: bool = False) -> None:
    args = [chromium(), "--headless", "--disable-gpu", "--hide-scrollbars", "--allow-file-access-from-files",
            "--virtual-time-budget=3000", f"--window-size={w},{h + 200}", f"--screenshot={out}", url]
    if transparent:
        args.insert(2, "--default-background-color=00000000")
    subprocess.run(args, check=True, capture_output=True)
    Image.open(out).convert("RGBA" if transparent else "RGB").crop((0, 0, w, h)).save(out)


def main() -> None:
    OUT.mkdir(exist_ok=True)
    for slide, name in SLIDES.items():
        shoot(f"{(HERE / 'slides.html').as_uri()}#{slide}", OUT / name, 1920, 960)
        print(name)
    # The Marketplace icon: the app logo on its own, static, nothing written on it.
    with tempfile.TemporaryDirectory() as tmp:
        page = Path(tmp) / "icon.html"
        logo = (HERE.parent.parent / "site" / "img" / "logo.svg").as_uri()
        page.write_text("<!doctype html><style>html,body{margin:0;background:transparent}"
                        f"img{{display:block;width:288px;height:288px}}</style><img src=\"{logo}\">", encoding="utf-8")
        shoot(page.as_uri(), OUT / "app-icon-288.png", 288, 288, transparent=True)
    print("app-icon-288.png")


if __name__ == "__main__":
    main()
