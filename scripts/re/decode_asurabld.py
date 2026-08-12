#!/usr/bin/env python3
"""Decode Asura Blade (Fuuki FG-3) graphics ROMs into indexed-color PNG tile sheets.

Pure stdlib. Specs from MAME fuukifg3.c GfxLayouts:
- byte swizzle per 4-byte group [b0 b1 b2 b3] -> pixels b1>>4,b1&15,b0>>4,b0&15,b3>>4,b3&15,b2>>4,b2&15
- sprites: 16x16 4bpp, 128 bytes/tile
- bg: 16x16 8bpp = two 4bpp files combined, pen = (hi_nib<<4)|lo_nib
- map.u5: 8x8 4bpp, 32 bytes/tile
"""
import os
import struct
import sys
import zlib
import colorsys

ROMDIR = "/Users/frankkorf/games/roms/asurabld"
OUTDIR = os.path.dirname(os.path.abspath(__file__))

# ---------------------------------------------------------------- LUTs
LUT = [bytes((b >> 4, b & 15)) for b in range(256)]
# LUT2[h][l] -> two 8bpp pixels from one hi-file byte and one lo-file byte
LUT2 = [
    [bytes(((h & 0xF0) | (l >> 4), ((h & 15) << 4) | (l & 15))) for l in range(256)]
    for h in range(256)
]


def swizzle(data):
    """Reorder bytes within each 32-bit group: [b1, b0, b3, b2]."""
    out = bytearray(len(data))
    out[0::4] = data[1::4]
    out[1::4] = data[0::4]
    out[2::4] = data[3::4]
    out[3::4] = data[2::4]
    return bytes(out)


def expand4(data):
    """4bpp ROM data -> one byte per pixel (values 0..15), row-major per tile."""
    return b"".join(map(LUT.__getitem__, swizzle(data)))


def combine8(lo_data, hi_data):
    """Two 4bpp ROMs -> one byte per pixel (0..255), pen=(hi<<4)|lo."""
    lswz = swizzle(lo_data)
    hswz = swizzle(hi_data)
    return b"".join([LUT2[h][l] for h, l in zip(hswz, lswz)])


# ---------------------------------------------------------------- palettes
def hsv_bytes(h, s, v):
    r, g, b = colorsys.hsv_to_rgb(h % 1.0, s, v)
    return bytes((int(r * 255 + 0.5), int(g * 255 + 0.5), int(b * 255 + 0.5)))


def palette4():
    pal = [b"\x00\x00\x00"]
    for i in range(1, 16):
        t = (i - 1) / 14.0
        hue = 0.66 - 0.66 * t          # blue -> red -> yellow-ish sweep
        val = 0.35 + 0.65 * t
        sat = 0.85 - 0.35 * t
        pal.append(hsv_bytes(hue, sat, val))
    return b"".join(pal)


def palette8():
    pal = [b"\x00\x00\x00"]
    for i in range(1, 256):
        t = i / 255.0
        hue = 0.75 - 0.75 * t          # violet -> blue -> green -> red
        val = 0.25 + 0.75 * t
        sat = 0.7
        pal.append(hsv_bytes(hue, sat, val))
    return b"".join(pal)


PAL4 = palette4()
PAL8 = palette8()


# ---------------------------------------------------------------- PNG writer
def write_png(path, width, height, plte, scanlines):
    raw = b"".join(b"\x00" + bytes(s) for s in scanlines)
    assert len(raw) == (width + 1) * height, (path, len(raw), width, height)
    comp = zlib.compress(raw, 6)

    def chunk(typ, data):
        c = typ + data
        return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c))

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 3, 0, 0, 0)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", ihdr))
        f.write(chunk(b"PLTE", plte))
        f.write(chunk(b"IDAT", comp))
        f.write(chunk(b"IEND", b""))


# ---------------------------------------------------------------- sheet layout
def sheet_scanlines(px, tile_base, tiles_per_row, tile_size, tile_rows):
    """Yield scanlines (bytes-like, width = tiles_per_row*tile_size) for a sheet."""
    mv = memoryview(px)
    tp = tile_size * tile_size
    for trow in range(tile_rows):
        base = tile_base + trow * tiles_per_row
        for ry in range(tile_size):
            off = ry * tile_size
            yield b"".join(
                [mv[(base + tx) * tp + off:(base + tx) * tp + off + tile_size]
                 for tx in range(tiles_per_row)]
            )


def downsample2x(scanlines):
    """2x2 max-pool a list of equal-width scanlines."""
    out = []
    for i in range(0, len(scanlines) - 1, 2):
        m = bytes(map(max, scanlines[i], scanlines[i + 1]))
        out.append(bytes(map(max, m[0::2], m[1::2])))
    return out


# ---------------------------------------------------------------- blank stats
def blank_fraction(data, tile_bytes):
    n = len(data) // tile_bytes
    z = bytes(tile_bytes)
    f = b"\xff" * tile_bytes
    blank = 0
    for t in range(n):
        c = data[t * tile_bytes:(t + 1) * tile_bytes]
        if c == z or c == f:
            blank += 1
    return blank, n


# ---------------------------------------------------------------- drivers
manifest = []      # (filename, width, height)
blank_report = []  # (rom, blank, total)


def emit(name_prefix, px, tile_size, tiles_per_row, sheet_tile_rows, plte):
    """Write sheets + overview PNGs for a fully expanded pixel buffer."""
    tp = tile_size * tile_size
    total_tiles = len(px) // tp
    tiles_per_sheet = tiles_per_row * sheet_tile_rows
    n_sheets = total_tiles // tiles_per_sheet
    width = tiles_per_row * tile_size
    all_small = []
    for s in range(n_sheets):
        lines = list(sheet_scanlines(px, s * tiles_per_sheet, tiles_per_row,
                                     tile_size, sheet_tile_rows))
        fn = "%s_sheet%02d.png" % (name_prefix, s)
        write_png(os.path.join(OUTDIR, fn), width, len(lines), plte, lines)
        manifest.append((fn, width, len(lines)))
        all_small.extend(downsample2x(lines))
        print("  wrote", fn, flush=True)
    # overview: full concatenated sheet downsampled 2x, split into <=2048-tall parts
    ow = width // 2
    part = 0
    for start in range(0, len(all_small), 2048):
        seg = all_small[start:start + 2048]
        fn = "%s_overview_%s.png" % (name_prefix, chr(ord("a") + part))
        write_png(os.path.join(OUTDIR, fn), ow, len(seg), plte, seg)
        manifest.append((fn, ow, len(seg)))
        print("  wrote", fn, flush=True)
        part += 1


def load(name):
    with open(os.path.join(ROMDIR, name), "rb") as f:
        return f.read()


def main():
    # sprites: 16x16 4bpp
    for name in ("sp23.u14", "sp45.u15", "sp67.u16", "sp89.u17",
                 "spab.u18", "spcd.u19"):
        print(name, flush=True)
        data = load(name)
        b, n = blank_fraction(data, 128)
        blank_report.append((name, b, n))
        px = expand4(data)
        emit(name.split(".")[0], px, 16, 64, 64, PAL4)
        del px, data

    # bg layers: 16x16 8bpp from two 4bpp files
    for prefix, lo_name, hi_name in (("bg1", "bg1012.u22", "bg1113.u23"),
                                     ("bg2", "bg2022.u25", "bg2123.u24")):
        print(prefix, flush=True)
        lo = load(lo_name)
        hi = load(hi_name)
        for nm, d in ((lo_name, lo), (hi_name, hi)):
            b, n = blank_fraction(d, 128)
            blank_report.append((nm, b, n))
        px = combine8(lo, hi)
        emit(prefix, px, 16, 64, 64, PAL8)
        del px, lo, hi

    # text/bg map layer: 8x8 4bpp
    print("map.u5", flush=True)
    data = load("map.u5")
    b, n = blank_fraction(data, 32)
    blank_report.append(("map.u5", b, n))
    px = expand4(data)
    emit("map", px, 8, 128, 128, PAL4)
    del px, data

    print("\n=== MANIFEST ===")
    for fn, w, h in manifest:
        size = os.path.getsize(os.path.join(OUTDIR, fn))
        print("%-24s %5dx%-5d %9d bytes" % (fn, w, h, size))
    print("\n=== BLANK TILES (all-0x00 or all-0xFF) ===")
    for nm, b, n in blank_report:
        print("%-12s %6d / %6d  (%.1f%%)" % (nm, b, n, 100.0 * b / n))


if __name__ == "__main__":
    main()
