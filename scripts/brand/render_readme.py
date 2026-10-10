#!/usr/bin/env python3
"""Render the README artwork as PNG, so GitHub shows exactly these pixels with the
wallet's own fonts (Inter, Unbounded, JetBrains Mono) instead of substituting
fonts in an SVG. Drawn at 2x for sharp text on high-density screens.
Requires Pillow. Run from the repository root:

    python scripts/brand/render_readme.py
"""
import pathlib

from PIL import Image, ImageDraw, ImageFont

from render_logo import INK, raster

ROOT = pathlib.Path(__file__).resolve().parents[2]
FONTS = ROOT / "apps" / "genoryn-wallet" / "assets" / "fonts"
OUT = ROOT / "docs" / "images"
SCALE = 2
BACKGROUND = "#050505"
CARD = "#0B0B0B"
BORDER = "#242424"
GRID_LINE = "#0E0E0E"
GRAY = "#9A9A9A"
DIM = "#6A6A6A"


def font(name, size):
    return ImageFont.truetype(str(FONTS / name), size * SCALE)


INTER = lambda size: font("Inter-Regular.ttf", size)  # noqa: E731
SEMI = lambda size: font("Inter-SemiBold.ttf", size)  # noqa: E731
DISPLAY = lambda size: font("Unbounded-Bold.ttf", size)  # noqa: E731
MONO = lambda size: font("JetBrainsMono-Regular.ttf", size)  # noqa: E731


def canvas(width, height):
    image = Image.new("RGB", (width * SCALE, height * SCALE), BACKGROUND)
    draw = ImageDraw.Draw(image)
    step = 64 * SCALE
    for x in range(0, image.width, step):
        draw.line((x, 0, x, image.height), fill=GRID_LINE, width=SCALE)
    for y in range(0, image.height, step):
        draw.line((0, y, image.width, y), fill=GRID_LINE, width=SCALE)
    return image, draw


def s(value):
    return int(value * SCALE)


def spaced(draw, xy, text, typeface, fill, spacing):
    """Letter-spaced text; Pillow has no tracking option."""
    x, y = xy
    for character in text:
        draw.text((s(x), s(y)), character, font=typeface, fill=fill)
        x += draw.textlength(character, font=typeface) / SCALE + spacing


def mark(image, x, y, size):
    tile = raster(size * SCALE)
    image.paste(tile, (s(x), s(y)), tile)


def rounded(draw, box, radius, fill=CARD, outline=BORDER):
    draw.rounded_rectangle(tuple(s(v) for v in box), radius=s(radius), fill=fill, outline=outline, width=SCALE)


def hero():
    width, height = 1280, 400
    image, draw = canvas(width, height)
    rounded(draw, (0, 0, width - 1, height - 1), 24, fill=None, outline=BORDER)
    mark(image, 72, 72, 88)
    spaced(draw, (184, 92), "GENORYN", SEMI(30), INK, 9)
    spaced(draw, (186, 136), "MAINNET LIVE · FAIR LAUNCH · NO PREMINE", MONO(13), GRAY, 2.4)
    draw.text((s(72), s(206)), "Privacy is not a crime.", font=DISPLAY(44), fill=INK)
    draw.text((s(72), s(262)), "It's a right.", font=DISPLAY(44), fill=GRAY)
    chips = ["21,000,000 GNR cap", "Halo 2 shielded notes", "RandomX CPU mining", "Your own full node"]
    x = 72
    for chip in chips:
        label = INTER(15)
        w = draw.textlength(chip, font=label) / SCALE + 36
        rounded(draw, (x, 334, x + w, 366), 16, fill="#0E0E0E", outline="#2E2E2E")
        draw.text((s(x + 18), s(341)), chip, font=label, fill="#D6D6D6")
        x += w + 12
    image.save(OUT / "readme-hero.png", optimize=True)


def lock(draw, x, y):
    stroke = s(3.5)
    draw.rounded_rectangle((s(x), s(y + 18), s(x + 40), s(y + 50)), radius=s(6), outline=INK, width=stroke)
    draw.arc((s(x + 8), s(y), s(x + 32), s(y + 30)), start=180, end=360, fill=INK, width=stroke)
    draw.line((s(x + 8), s(y + 15), s(x + 8), s(y + 19)), fill=INK, width=stroke)
    draw.line((s(x + 32), s(y + 15), s(x + 32), s(y + 19)), fill=INK, width=stroke)
    draw.ellipse((s(x + 17), s(y + 30), s(x + 23), s(y + 36)), fill=INK)


def server(draw, x, y):
    stroke = s(3.5)
    for top in (y + 2, y + 28):
        draw.rounded_rectangle((s(x), s(top), s(x + 46), s(top + 20)), radius=s(5), outline=INK, width=stroke)
        draw.ellipse((s(x + 33), s(top + 7), s(x + 39), s(top + 13)), fill=INK)


def globe(draw, x, y):
    stroke = s(3.5)
    draw.ellipse((s(x), s(y + 2), s(x + 46), s(y + 48)), outline=INK, width=stroke)
    draw.ellipse((s(x + 13), s(y + 2), s(x + 33), s(y + 48)), outline=INK, width=stroke)
    draw.line((s(x + 1), s(y + 25), s(x + 45), s(y + 25)), fill=INK, width=stroke)


def security():
    width, height = 1280, 500
    image, draw = canvas(width, height)
    rounded(draw, (0, 0, width - 1, height - 1), 24, fill=None, outline=BORDER)
    mark(image, 64, 46, 40)
    spaced(draw, (118, 52), "HOW GENORYN PROTECTS YOU", MONO(13), GRAY, 2.6)
    draw.text((s(118), s(74)), "Your keys, your node, shielded notes.", font=INTER(15), fill=DIM)
    cards = [
        (lock, "01 · YOUR DEVICE", "Keys never leave it", [
            "Argon2id and XChaCha20-Poly1305",
            "encrypt the wallet file. The",
            "recovery phrase is never stored",
            "in plain text."]),
        (server, "02 · YOUR FULL NODE", "Trusts no one", [
            "Checks every block, proof,",
            "signature and nullifier itself.",
            "Seeds only introduce peers;",
            "the heaviest valid chain wins."]),
        (globe, "03 · THE NETWORK", "Sees only shielded notes", [
            "Amounts and receivers stay",
            "hidden. A spend reveals only a",
            "nullifier. No balance lookup,",
            "no rich list."]),
    ]
    for index, (icon, kicker, title, lines) in enumerate(cards):
        x = 64 + index * 392
        rounded(draw, (x, 120, x + 360, 452), 20)
        icon(draw, x + 32, 150)
        spaced(draw, (x + 32, 226), kicker, MONO(12), GRAY, 2.2)
        draw.text((s(x + 32), s(250)), title, font=SEMI(24), fill=INK)
        for row, line in enumerate(lines):
            draw.text((s(x + 32), s(300 + row * 28)), line, font=INTER(16), fill="#BDBDBD")
        if index < 2:
            ax = x + 368
            draw.line((s(ax), s(286), s(ax + 16), s(286)), fill=DIM, width=s(2))
            draw.polygon([(s(ax + 16), s(280)), (s(ax + 24), s(286)), (s(ax + 16), s(292))], fill=DIM)
    image.save(OUT / "readme-security.png", optimize=True)


def explorer():
    """The call-out for genoryn.io, the public block explorer."""
    width, height = 1280, 220
    image, draw = canvas(width, height)
    rounded(draw, (0, 0, width - 1, height - 1), 24, fill=None, outline=BORDER)
    mark(image, 64, 58, 104)
    spaced(draw, (200, 62), "GENORYN.IO · LIVE MAINNET EXPLORER", MONO(13), GRAY, 2.4)
    draw.text((s(200), s(88)), "Watch the chain, block by block.", font=DISPLAY(30), fill=INK)
    draw.text((s(200), s(138)), "Blocks, transactions, difficulty, hashrate, supply and peers, served by a",
              font=INTER(16), fill="#BDBDBD")
    draw.text((s(200), s(162)), "validating full node. Amounts and receivers stay shielded there too.",
              font=INTER(16), fill="#BDBDBD")
    button = (1040, 86, 1216, 134)
    draw.rounded_rectangle(tuple(s(v) for v in button), radius=s(24), fill="#EEEEEE")
    label = SEMI(16)
    text = "Open genoryn.io"
    w = draw.textlength(text, font=label) / SCALE
    draw.text((s(1128 - w / 2), s(99)), text, font=label, fill="#0A0A0A")
    image.save(OUT / "readme-explorer.png", optimize=True)


def installer():
    """Inno Setup wizard art for its dark style: the tall welcome panel at the
    100% size (164x314 points, drawn 2x) and a transparent header mark."""
    target = ROOT / "branding" / "installer"
    target.mkdir(parents=True, exist_ok=True)
    width, height = 164, 314
    panel, draw = canvas(width, height)
    mark(panel, 40, 70, 84)
    spaced(draw, (26, 178), "GENORYN", SEMI(17), INK, 4.6)
    for row, line in enumerate(["Private by design.", "Finite by protocol."]):
        draw.text((s(26), s(212 + row * 20)), line, font=INTER(12), fill=GRAY)
    spaced(draw, (26, 280), "MAINNET", MONO(9), DIM, 2.2)
    panel.save(target / "wizard-large.png", optimize=True)
    small = raster(110)
    small.save(target / "wizard-small.png", optimize=True)


if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    hero()
    security()
    explorer()
    installer()
    print("Rendered README and installer artwork.")
