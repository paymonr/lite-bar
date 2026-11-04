#!/usr/bin/env python3
"""Draws Lite Bar's icon ("Lite L": a blue rounded square with a white L) and writes
crates/app/assets/lite-bar.ico. The build embeds that file in lite-bar.exe.

Usage: python3 scripts/make-icon.py   (needs Pillow)

The shapes are on a 32-unit grid, the same as the design mockup. Each size is drawn 16x larger
and scaled down, so edges are smooth; on that grid the L lands on whole pixels at 16, 32, 48 and
64 px. Sizes below 256 are stored as 32-bit bitmaps (read by every Windows version), 256 as PNG.
"""

import io
import struct
from pathlib import Path

from PIL import Image, ImageDraw

BLUE = (0x2F, 0x6F, 0xDB, 255)
WHITE = (255, 255, 255, 255)
SIZES = [16, 20, 24, 32, 40, 48, 64, 256]
SUPERSAMPLE = 16
OUT = Path(__file__).resolve().parent.parent / "crates" / "app" / "assets" / "lite-bar.ico"


def draw(size: int) -> Image.Image:
    big = size * SUPERSAMPLE
    k = big / 32  # one grid unit, in supersampled pixels
    img = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    # Rounded square: x, y 1..31, corner radius 7.
    d.rounded_rectangle([1 * k, 1 * k, 31 * k - 1, 31 * k - 1], radius=7 * k, fill=BLUE)
    # The L: a 4-unit stroke from (12, 8) down to (12, 23), then right to (22, 23), round caps
    # and a round join.
    half = 2 * k
    d.rectangle([12 * k - half, 8 * k, 12 * k + half - 1, 23 * k], fill=WHITE)
    d.rectangle([12 * k, 23 * k - half, 22 * k, 23 * k + half - 1], fill=WHITE)
    for cx, cy in [(12, 8), (12, 23), (22, 23)]:
        d.ellipse([cx * k - half, cy * k - half, cx * k + half - 1, cy * k + half - 1], fill=WHITE)
    return img.resize((size, size), Image.BOX)


def bmp_entry(img: Image.Image) -> bytes:
    """A 32-bit DIB as .ico stores it: BITMAPINFOHEADER (double height), bottom-up BGRA rows,
    then an all-zero AND mask (alpha does the masking)."""
    w, h = img.size
    header = struct.pack("<IiiHHIIiiII", 40, w, h * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    px = img.load()
    rows = bytearray()
    for y in range(h - 1, -1, -1):
        for x in range(w):
            r, g, b, a = px[x, y]
            rows += bytes((b, g, r, a))
    mask_row = ((w + 31) // 32) * 4
    return header + bytes(rows) + bytes(mask_row * h)


def png_entry(img: Image.Image) -> bytes:
    buf = io.BytesIO()
    img.save(buf, format="PNG", optimize=True)
    return buf.getvalue()


def main() -> None:
    images = [(s, png_entry(draw(s)) if s >= 256 else bmp_entry(draw(s))) for s in SIZES]
    out = bytearray(struct.pack("<HHH", 0, 1, len(images)))
    offset = 6 + 16 * len(images)
    for size, data in images:
        dim = 0 if size >= 256 else size
        out += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    for _, data in images:
        out += data
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_bytes(out)
    print(f"wrote {OUT} ({len(out)} bytes, sizes {SIZES})")


if __name__ == "__main__":
    main()
