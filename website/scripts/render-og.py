#!/usr/bin/env python3
"""Render the 1200x630 Open Graph card from the brand geometry (monochrome only).

Usage: python3 scripts/render-og.py <Unbounded.ttf> <Inter.ttf> <out.png>
"""
import math
import sys

from PIL import Image, ImageDraw, ImageFilter, ImageFont

W, H = 1200, 630
BG, INK, GRAY, LINE = (11, 11, 11), (242, 242, 242), (154, 154, 154), (42, 42, 42)
display_path, sans_path, out = sys.argv[1], sys.argv[2], sys.argv[3]

S = 2  # supersample
img = Image.new('RGB', (W * S, H * S), BG)
d = ImageDraw.Draw(img)

# Grid
for x in range(0, W * S, 64 * S):
    d.line([(x, 0), (x, H * S)], fill=(20, 20, 20), width=S)
for y in range(0, H * S, 64 * S):
    d.line([(0, y), (W * S, y)], fill=(20, 20, 20), width=S)

# Glow behind the sign
glow = Image.new('RGB', (W * S, H * S), (0, 0, 0))
gd = ImageDraw.Draw(glow)
cx, cy = 960 * S, 300 * S
gd.ellipse([cx - 330 * S, cy - 330 * S, cx + 330 * S, cy + 330 * S], fill=(60, 60, 60))
glow = glow.filter(ImageFilter.GaussianBlur(120 * S))
img = Image.blend(img, Image.composite(glow, img, glow.convert('L')), 0.55)
d = ImageDraw.Draw(img)


def sign(cx, cy, scale, ink):
    """Brand glyph on a 256 grid centred at (128,128)."""
    k = scale
    R, STROKE = 58 * k, 30 * k
    box = [cx - R - STROKE / 2, cy - R - STROKE / 2, cx + R + STROKE / 2, cy + R + STROKE / 2]
    # ring from 0deg clockwise (screen) to 318deg: PIL angles go clockwise from 3 o'clock
    d.arc(box, start=0, end=318, fill=ink, width=int(STROKE))
    tx = lambda v: cx + (v - 128) * k
    ty = lambda v: cy + (v - 128) * k
    d.rectangle([tx(134), ty(113), tx(201), ty(143)], fill=ink)
    for x0 in (106, 136):
        d.rounded_rectangle([tx(x0), ty(31), tx(x0 + 14), ty(85)], radius=2 * k, fill=ink)
        d.rounded_rectangle([tx(x0), ty(171), tx(x0 + 14), ty(225)], radius=2 * k, fill=ink)


# Orbit ring of blocks
orbit = 205 * S
for i in range(28):
    a = i / 28 * math.tau
    x, y = cx + math.cos(a) * orbit, cy + math.sin(a) * orbit * 0.32
    s = 9 * S
    d.rectangle([x - s, y - s, x + s, y + s], outline=(120, 120, 120), width=S)
sign(cx, cy, 1.85 * S, INK)

display = ImageFont.truetype(display_path, 74 * S)
try:
    display.set_variation_by_axes([800])
except Exception:
    pass
sans = ImageFont.truetype(sans_path, 26 * S)
try:
    sans.set_variation_by_axes([14, 500])
except Exception:
    pass
small = ImageFont.truetype(sans_path, 20 * S)

x0 = 72 * S
d.text((x0, 92 * S), 'PRIVATE BY DESIGN · FINITE BY PROTOCOL', font=small, fill=GRAY)
lines = ['PRIVACY IS', 'NOT A CRIME.', "IT'S A RIGHT."]
y = 140 * S
for line in lines:
    d.text((x0, y), line, font=display, fill=INK)
    y += 86 * S
d.text((x0, y + 26 * S), 'Shielded Halo 2 notes · 21,000,000 GNR cap · CPU-fair RandomX', font=sans, fill=GRAY)

# Footer bar
d.line([(x0, 548 * S), (W * S - x0, 548 * S)], fill=LINE, width=S)
d.text((x0, 566 * S), 'GENORYN', font=sans, fill=INK)
d.text((W * S - x0 - d.textlength('genoryn.io', font=sans), 566 * S), 'genoryn.io', font=sans, fill=GRAY)

img = img.resize((W, H), Image.LANCZOS)
img.save(out, optimize=True)
print('wrote', out)
