"""Rasterize the MIDI Party icon into assets/icon/icon-<size>.png.

Draws the same design as icon.svg (keep the constants below in step with it)
using only the standard library, so the PNGs can be regenerated anywhere.
"""
import math
import os
import struct
import zlib

DESIGN = 1024
TILE_RADIUS = 180
FRAME = 112
CELL = 200
CELLS = 4
LINE = 8
NOTE_W, NOTE_H, NOTE_RADIUS = 160, 120, 12
# Grid row (0 = top) of the note in each column.
MELODY = [3, 2, 1, 0]

TILE = (0x1B, 0x19, 0x17)
GRID = (0x27, 0x23, 0x20)
FRAME_LINE = (0x4A, 0x42, 0x38)
NOTE = (0xE6, 0xA2, 0x54)

SIZES = [16, 32, 64, 128, 256, 512, 1024]
SUBROWS = 4


def rect(x, y, w, h, radius=0):
    return (x, y, w, h, radius)


def scene():
    """Paint layers as (color, shapes), back to front."""
    half = LINE / 2
    frame_size = CELL * CELLS
    lo = FRAME - half
    hi = FRAME + frame_size - half

    grid = []
    for i in range(1, CELLS):
        p = FRAME + i * CELL - half
        grid.append(rect(p, FRAME, LINE, frame_size))
        grid.append(rect(FRAME, p, frame_size, LINE))

    frame = [
        rect(lo, lo, frame_size + LINE, LINE),
        rect(lo, hi, frame_size + LINE, LINE),
        rect(lo, lo, LINE, frame_size + LINE),
        rect(hi, lo, LINE, frame_size + LINE),
    ]

    notes = []
    for col, row in enumerate(MELODY):
        x = FRAME + col * CELL + (CELL - NOTE_W) / 2
        y = FRAME + row * CELL + (CELL - NOTE_H) / 2
        notes.append(rect(x, y, NOTE_W, NOTE_H, NOTE_RADIUS))

    return [
        (TILE, [rect(0, 0, DESIGN, DESIGN, TILE_RADIUS)]),
        (GRID, grid),
        (FRAME_LINE, frame),
        (NOTE, notes),
    ]


def span(shape, y):
    """Horizontal extent [x0, x1) of the shape on design row y, or None."""
    x, top, w, h, radius = shape
    if y < top or y >= top + h:
        return None
    inset = 0.0
    if radius:
        dy = max(top + radius - y, y - (top + h - radius), 0.0)
        if dy:
            inset = radius - math.sqrt(radius * radius - dy * dy)
    return x + inset, x + w - inset


def add_span(cov, x0, x1, weight):
    """Add the pixel-space span [x0, x1) to the per-pixel coverage list."""
    size = len(cov)
    x0 = max(x0, 0.0)
    x1 = min(x1, float(size))
    if x1 <= x0:
        return
    first = int(math.floor(x0))
    last = int(math.ceil(x1)) - 1
    if first == last:
        cov[first] += (x1 - x0) * weight
        return
    cov[first] += (first + 1 - x0) * weight
    cov[last] += (x1 - last) * weight
    for px in range(first + 1, last):
        cov[px] += weight


def render_row(py, size, layers):
    scale = size / DESIGN
    weight = 1.0 / SUBROWS
    red = [0.0] * size
    green = [0.0] * size
    blue = [0.0] * size
    alpha = [0.0] * size
    for (cr, cg, cb), shapes in layers:
        cov = [0.0] * size
        for k in range(SUBROWS):
            y = (py + (k + 0.5) / SUBROWS) / scale
            for shape in shapes:
                s = span(shape, y)
                if s:
                    add_span(cov, s[0] * scale, s[1] * scale, weight)
        for px in range(size):
            c = min(cov[px], 1.0)
            if c:
                keep = 1.0 - c
                red[px] = cr * c + red[px] * keep
                green[px] = cg * c + green[px] * keep
                blue[px] = cb * c + blue[px] * keep
                alpha[px] = c + alpha[px] * keep

    row = bytearray()
    for px in range(size):
        a = alpha[px]
        if a <= 0.0:
            row += b"\x00\x00\x00\x00"
            continue
        row.append(channel(red[px] / a))
        row.append(channel(green[px] / a))
        row.append(channel(blue[px] / a))
        row.append(channel(a * 255.0))
    return bytes(row)


def channel(value):
    return max(0, min(255, int(round(value))))


def png_chunk(tag, data):
    crc = zlib.crc32(tag + data) & 0xFFFFFFFF
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", crc)


def write_png(path, size, rows):
    raw = b"".join(b"\x00" + row for row in rows)
    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    png = (
        b"\x89PNG\r\n\x1a\n"
        + png_chunk(b"IHDR", ihdr)
        + png_chunk(b"IDAT", zlib.compress(raw, 9))
        + png_chunk(b"IEND", b"")
    )
    with open(path, "wb") as f:
        f.write(png)


def main():
    out_dir = os.path.dirname(os.path.abspath(__file__))
    layers = scene()
    for size in SIZES:
        rows = [render_row(py, size, layers) for py in range(size)]
        path = os.path.join(out_dir, "icon-%d.png" % size)
        write_png(path, size, rows)
        print(path)


if __name__ == "__main__":
    main()
