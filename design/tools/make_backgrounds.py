"""Generates the ambient background images (dark and light, grayscale, 1280x720 WebP < 10 KB).

Pipeline: procedural grayscale -> Gaussian blur -> tone-map (highlights stay dim) ->
~1 level of noise against banding -> WebP. Run: python design/tools/make_backgrounds.py
"""
import math
import os
import random

import numpy as np
from PIL import Image, ImageFilter

W, H = 320, 180  # drawn small, blurred, then upscaled (smooth by construction)
OUT = os.path.join(os.path.dirname(__file__), "..", "assets", "bg")


def grid():
    y, x = np.mgrid[0:H, 0:W].astype(np.float32)
    return x / W, y / H


def light_pools(rng):
    x, y = grid()
    img = np.zeros((H, W), np.float32)
    for _ in range(6):
        cx, cy, r, a = rng.random(), rng.random(), 0.12 + rng.random() * 0.3, 0.4 + rng.random() * 0.6
        img += a * np.exp(-(((x - cx) * 1.78) ** 2 + (y - cy) ** 2) / (2 * r * r))
    return img


def silk_ribbons(rng):
    x, y = grid()
    img = np.zeros((H, W), np.float32)
    for _ in range(4):
        ph, fr, amp, w = rng.random() * 6.28, 1.5 + rng.random() * 2, 0.08 + rng.random() * 0.12, 0.02 + rng.random() * 0.05
        c = 0.25 + rng.random() * 0.5 + amp * np.sin(x * fr * 6.28 + ph)
        img += np.exp(-((y - c) ** 2) / (2 * w * w)) * (0.5 + 0.5 * np.sin(x * 3.1 + ph))
    return img


def frosted_prisms(rng):
    x, y = grid()
    img = np.zeros((H, W), np.float32)
    for _ in range(5):
        ang = rng.random() * math.pi
        off = rng.random()
        d = (x * 1.78 * math.cos(ang) + y * math.sin(ang)) - off * 1.5
        img += np.clip(1 - np.abs(d) * (3 + rng.random() * 5), 0, 1) * (0.3 + rng.random() * 0.7)
    return img


def light_curtains(rng):
    x, y = grid()
    img = np.zeros((H, W), np.float32)
    for _ in range(7):
        cx, w = rng.random(), 0.02 + rng.random() * 0.06
        sway = 0.04 * np.sin(y * 5 + rng.random() * 6)
        img += np.exp(-((x - cx - sway) ** 2) / (2 * w * w)) * (1 - y) ** 1.5
    return img


def finish(arr, light, seed):
    arr = arr - arr.min()
    arr = arr / max(arr.max(), 1e-6)
    im = Image.fromarray((arr * 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(9))
    im = im.resize((1280, 720), Image.BICUBIC).filter(ImageFilter.GaussianBlur(6))
    a = np.asarray(im).astype(np.float32) / 255.0
    if light:
        # Light theme: pale greys, darkest areas stay light enough for black text.
        # Light theme: the dark design inverted into the 196..250 range.
        a = (250 - 54 * (a ** 1.2)) / 255.0
    else:
        # Dark theme: tone-map so the brightest pool stays dim (text stays readable).
        a = (3 + 61 * (a ** 1.4)) / 255.0  # brightest ~#40
    noise = np.random.default_rng(seed).normal(0, 1.0 / 255, a.shape)
    a = np.clip(a + noise, 0, 1)
    return Image.fromarray((a * 255).round().astype(np.uint8), "L")


def save(img, path):
    for q in (84, 80, 76, 70, 64, 56):
        img.save(path, "WEBP", quality=q, method=6)
        if os.path.getsize(path) < 10_000:
            return q
    return 5


def main():
    os.makedirs(OUT, exist_ok=True)
    gens = [light_pools, silk_ribbons, frosted_prisms, light_curtains]
    for i, g in enumerate(gens, 1):
        base = g(random.Random(1000 + i))
        for theme in ("dark", "light"):
            im = finish(base, theme == "light", i)
            p = os.path.join(OUT, f"{theme}-{i}.webp")
            q = save(im, p)
            print(f"{p}: {os.path.getsize(p)} bytes (q={q})")


if __name__ == "__main__":
    main()
