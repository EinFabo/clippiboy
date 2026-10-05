"""Builds src-tauri/assets/watermark.png: the logo plus "ClippiBoy", white with a
soft shadow, fully opaque. The opacity (65 %) is applied when it is drawn, so
the picture itself stays reusable.

Run once from WSL or Windows when the logo changes:
    python3 scripts/make-watermark.py
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parent.parent
LOGO = ROOT / "src-tauri" / "icons" / "512x512.png"
OUT = ROOT / "src-tauri" / "assets" / "watermark.png"
# Segoe UI is what the app falls back to on Windows (Inter is not bundled).
FONTS = [Path("/mnt/c/Windows/Fonts/segoeuib.ttf"), Path("C:/Windows/Fonts/segoeuib.ttf")]

HEIGHT = 128  # drawn at about 3 % of the frame — 32 px at 1080p, 65 at 4K
PAD = 12  # room for the shadow
GAP = 26

font_path = next(p for p in FONTS if p.exists())
font = ImageFont.truetype(str(font_path), int(HEIGHT * 0.62))
text = "ClippiBoy"
left, top, right, bottom = font.getbbox(text)
text_w = right - left

logo = Image.open(LOGO).convert("RGBA").resize((HEIGHT, HEIGHT), Image.LANCZOS)
width = PAD + HEIGHT + GAP + text_w + PAD
canvas = (width, HEIGHT + 2 * PAD)

# Text centred on the logo's middle, by its ink, not its line box.
text_x = PAD + HEIGHT + GAP - left
text_y = PAD + (HEIGHT - (bottom - top)) // 2 - top

shadow = Image.new("RGBA", canvas, (0, 0, 0, 0))
draw = ImageDraw.Draw(shadow)
draw.ellipse((PAD + 2, PAD + 4, PAD + HEIGHT + 2, PAD + HEIGHT + 4), fill=(0, 0, 0, 150))
draw.text((text_x + 2, text_y + 4), text, font=font, fill=(0, 0, 0, 170))
shadow = shadow.filter(ImageFilter.GaussianBlur(5))

out = Image.new("RGBA", canvas, (0, 0, 0, 0))
out.alpha_composite(shadow)
out.alpha_composite(logo, (PAD, PAD))
ImageDraw.Draw(out).text((text_x, text_y), text, font=font, fill=(255, 255, 255, 255))

OUT.parent.mkdir(parents=True, exist_ok=True)
out.save(OUT, optimize=True)
print(f"{OUT.relative_to(ROOT)}: {out.size[0]}x{out.size[1]}")
