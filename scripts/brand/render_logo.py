#!/usr/bin/env python3
"""Render the GENORYN mark from one geometry: SVG for documentation, PNG for the
wallet window and a multi-size ICO for the Windows executable.

The mark is a currency sign: a bold geometric G whose two short vertical strokes
cross the top and bottom of the letter, in the tradition of the Bitcoin sign.
The wallet draws the same geometry as vectors (`apps/genoryn-wallet/src/icons.rs`),
so keep both in sync. Requires Pillow. Run from the repository root:

    python scripts/brand/render_logo.py
"""
import math
import pathlib

from PIL import Image, ImageDraw

ROOT = pathlib.Path(__file__).resolve().parents[2]
WALLET_ASSETS = ROOT / "apps" / "genoryn-wallet" / "assets"
DOC_IMAGES = ROOT / "docs" / "images"
BRANDING = ROOT / "branding"

GRID = 256
TILE = "#0B0B0B"
BORDER = "#2A2A2A"
INK = "#F2F2F2"
LIGHT_TILE = "#F2F2F2"
LIGHT_BORDER = "#D6D6D6"
DARK_INK = "#0B0B0B"
CORNER = 56

# Glyph geometry on the 256 grid; mirrored in icons.rs.
C = GRID / 2
RADIUS = 58  # ring centerline
STROKE = 30  # ring and crossbar weight
OPENING_END = 318  # ring runs clockwise from three o'clock to this angle
BAR_START = C + 6  # crossbar's inner end
TICK_WIDTH = 14
TICK_OFFSET = 15  # tick centers left and right of the vertical axis
TICK_OVERHANG = 24  # how far ticks extend beyond the ring


def tick_rects():
    outer = RADIUS + STROKE / 2
    inner = RADIUS - STROKE / 2
    rects = []
    for center in (C - TICK_OFFSET, C + TICK_OFFSET):
        left, right = center - TICK_WIDTH / 2, center + TICK_WIDTH / 2
        rects.append((left, C - outer - TICK_OVERHANG, right, C - inner))
        rects.append((left, C + inner, right, C + outer + TICK_OVERHANG))
    return rects


def svg_glyph(ink=INK):
    start = (C + RADIUS, C)
    angle = math.radians(OPENING_END)
    end = (C + RADIUS * math.cos(angle), C + RADIUS * math.sin(angle))
    outer = RADIUS + STROKE / 2
    parts = [
        f'<path d="M{start[0]:.2f} {start[1]:.2f} A{RADIUS} {RADIUS} 0 1 1 {end[0]:.2f} {end[1]:.2f}" '
        f'fill="none" stroke="{ink}" stroke-width="{STROKE}"/>',
        f'<rect x="{BAR_START:.0f}" y="{C - STROKE / 2:.0f}" width="{C + outer - BAR_START:.0f}" '
        f'height="{STROKE}" fill="{ink}"/>',
    ]
    for left, top, right, bottom in tick_rects():
        parts.append(
            f'<rect x="{left:.0f}" y="{top:.0f}" width="{right - left:.0f}" '
            f'height="{bottom - top:.0f}" rx="2" fill="{ink}"/>'
        )
    return "".join(parts)


def svg_tile(tile=TILE, border=BORDER, ink=INK):
    return (
        f'<rect width="{GRID}" height="{GRID}" rx="{CORNER}" fill="{tile}"/>'
        f'<rect x="2" y="2" width="{GRID - 4}" height="{GRID - 4}" rx="{CORNER - 2}" '
        f'fill="none" stroke="{border}" stroke-width="4"/>' + svg_glyph(ink)
    )


def svg_document(width, height, body, label="GENORYN"):
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" '
        f'role="img" aria-label="{label}">{body}</svg>\n'
    )


def svg_wordmark(ink):
    """The sign beside the GENORYN wordmark, for headers and documents."""
    font = "Inter, 'Segoe UI', 'Helvetica Neue', Arial, sans-serif"
    return svg_document(
        1000,
        256,
        f'<g transform="translate(0 0)">{svg_glyph(ink)}</g>'
        f'<text x="268" y="166" fill="{ink}" font-family="{font}" font-size="118" '
        f'font-weight="600" letter-spacing="20">GENORYN</text>',
    )


def write_branding():
    """Every logo variant, plus PNG exports, in one folder for designers."""
    logo = BRANDING / "logo"
    png = BRANDING / "png"
    logo.mkdir(parents=True, exist_ok=True)
    png.mkdir(parents=True, exist_ok=True)
    files = {
        "genoryn-mark-dark.svg": svg_document(GRID, GRID, svg_tile()),
        "genoryn-mark-light.svg": svg_document(
            GRID, GRID, svg_tile(LIGHT_TILE, LIGHT_BORDER, DARK_INK)
        ),
        "genoryn-sign-white.svg": svg_document(GRID, GRID, svg_glyph(INK)),
        "genoryn-sign-black.svg": svg_document(GRID, GRID, svg_glyph(DARK_INK)),
        "genoryn-wordmark-white.svg": svg_wordmark(INK),
        "genoryn-wordmark-black.svg": svg_wordmark(DARK_INK),
    }
    for name, text in files.items():
        (logo / name).write_text(text, encoding="utf-8")
    (logo / "genoryn-banner.svg").write_text(
        (DOC_IMAGES / "genoryn-banner.svg").read_text(encoding="utf-8"), encoding="utf-8"
    )
    for size in [16, 32, 48, 64, 128, 256, 512, 1024]:
        raster(size).save(png / f"genoryn-mark-dark-{size}.png", optimize=True)
    for size in [256, 512, 1024]:
        raster(size, LIGHT_TILE, LIGHT_BORDER, DARK_INK).save(
            png / f"genoryn-mark-light-{size}.png", optimize=True
        )
        raster(size, None, None, INK).save(png / f"genoryn-sign-white-{size}.png", optimize=True)
        raster(size, None, None, DARK_INK).save(
            png / f"genoryn-sign-black-{size}.png", optimize=True
        )
    (BRANDING / "genoryn.ico").write_bytes((WALLET_ASSETS / "genoryn.ico").read_bytes())



def write_svgs():
    DOC_IMAGES.mkdir(parents=True, exist_ok=True)
    (DOC_IMAGES / "genoryn-mark.svg").write_text(
        svg_document(GRID, GRID, svg_tile()), encoding="utf-8"
    )
    width, height = 1280, 320
    font = "Inter, 'Segoe UI', 'Helvetica Neue', Arial, sans-serif"
    mono = "'JetBrains Mono', 'Cascadia Mono', Consolas, monospace"
    (DOC_IMAGES / "genoryn-banner.svg").write_text(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" '
        f'role="img" aria-label="GENORYN. Private by design. Finite by protocol.">'
        f'<rect width="{width}" height="{height}" rx="24" fill="#050505"/>'
        f'<rect x="1" y="1" width="{width - 2}" height="{height - 2}" rx="23" fill="none" stroke="#222"/>'
        f'<g transform="translate(96 64) scale(0.75)">{svg_tile()}</g>'
        f'<text x="336" y="150" fill="{INK}" font-family="{font}" font-size="76" '
        f'font-weight="600" letter-spacing="14">GENORYN</text>'
        f'<text x="340" y="204" fill="#9A9A9A" font-family="{font}" font-size="28" '
        f'letter-spacing="2">Private by design. Finite by protocol.</text>'
        f'<text x="340" y="252" fill="#6A6A6A" font-family="{mono}" font-size="18" '
        f'letter-spacing="3">RANDOMX PROOF OF WORK · HALO 2 SHIELDED NOTES · 21,000,000 GNR</text>'
        f"</svg>\n",
        encoding="utf-8",
    )


def raster(size, tile=TILE, border=BORDER, ink=INK):
    """Draw at 16x and downsample for clean antialiased edges; `tile=None` draws the
    sign alone on a transparent background."""
    scale = min(16, 4096 / size) * size / GRID
    big = int(GRID * scale)
    image = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    s = lambda value: value * scale  # noqa: E731
    if tile:
        draw.rounded_rectangle((0, 0, big - 1, big - 1), radius=s(CORNER), fill=tile)
        if border and size > 32:
            draw.rounded_rectangle(
                (s(2), s(2), big - 1 - s(2), big - 1 - s(2)),
                radius=s(CORNER - 2),
                outline=border,
                width=max(1, round(s(4))),
            )
    outer = RADIUS + STROKE / 2
    draw.arc(
        (s(C - outer), s(C - outer), s(C + outer), s(C + outer)),
        start=0,
        end=OPENING_END,
        fill=ink,
        width=round(s(STROKE)),
    )
    draw.rectangle((s(BAR_START), s(C - STROKE / 2), s(C + outer), s(C + STROKE / 2)), fill=ink)
    for left, top, right, bottom in tick_rects():
        draw.rounded_rectangle((s(left), s(top), s(right), s(bottom)), radius=s(2), fill=ink)
    return image.resize((size, size), Image.LANCZOS)


def write_rasters():
    WALLET_ASSETS.mkdir(parents=True, exist_ok=True)
    raster(256).save(WALLET_ASSETS / "genoryn-256.png", optimize=True)
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    images = [raster(size) for size in sizes]
    images[-1].save(
        WALLET_ASSETS / "genoryn.ico",
        format="ICO",
        sizes=[(size, size) for size in sizes],
        append_images=images[:-1],
    )


if __name__ == "__main__":
    write_svgs()
    write_rasters()
    write_branding()
    print("Rendered the GENORYN mark: documentation, wallet assets and branding/.")
    print("Installer and README artwork: scripts/brand/render_readme.py")
