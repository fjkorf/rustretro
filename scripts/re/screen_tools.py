#!/usr/bin/env python3
"""RE helper: decode an `app://screen` PNG (from the MCP server) with pure stdlib
and detect a fighting-game match on screen.

Used by the live reverse-engineering workflow (see the project memory note
"fighting-game-re-methodology"): while driving a game via the `press_buttons`
MCP tool, tag captured frames as in-fight vs menu/story WITHOUT eyeballing every
screenshot, so RAM samples can be filtered to the contiguous fight window.

`app://screen` returns 8-bit RGBA, color-type-6, non-interlaced PNG — this reader
handles exactly that (no Pillow dependency).

CLI:  python3 screen_tools.py <screen.png>   # prints the green-bar score
A score of roughly >40 means health bars are on screen (a fight); 0 means not.
"""
import sys, zlib, struct


def load_rgba(path):
    """Return (width, height, rgba_bytes) for an 8-bit RGBA PNG."""
    d = open(path, "rb").read()
    assert d[:8] == b"\x89PNG\r\n\x1a\n", "not a PNG"
    pos = 8
    w = h = bitdepth = colortype = None
    idat = bytearray()
    while pos < len(d):
        (ln,) = struct.unpack(">I", d[pos:pos + 4]); typ = d[pos + 4:pos + 8]
        chunk = d[pos + 8:pos + 8 + ln]; pos += 12 + ln
        if typ == b"IHDR":
            w, h, bitdepth, colortype = struct.unpack(">IIBB", chunk[:10])
        elif typ == b"IDAT":
            idat += chunk
        elif typ == b"IEND":
            break
    assert bitdepth == 8 and colortype == 6, f"need 8-bit RGBA, got bd={bitdepth} ct={colortype}"
    raw = zlib.decompress(bytes(idat))
    stride = w * 4
    out = bytearray(w * h * 4)
    prev = bytearray(stride)
    p = 0

    def paeth(a, b, c):
        pp = a + b - c
        pa, pb, pc = abs(pp - a), abs(pp - b), abs(pp - c)
        return a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)

    for y in range(h):
        f = raw[p]; p += 1
        line = bytearray(raw[p:p + stride]); p += stride
        if f == 1:
            for i in range(4, stride):
                line[i] = (line[i] + line[i - 4]) & 0xFF
        elif f == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif f == 3:
            for i in range(stride):
                a = line[i - 4] if i >= 4 else 0
                line[i] = (line[i] + ((a + prev[i]) >> 1)) & 0xFF
        elif f == 4:
            for i in range(stride):
                a = line[i - 4] if i >= 4 else 0
                c = prev[i - 4] if i >= 4 else 0
                line[i] = (line[i] + paeth(a, prev[i], c)) & 0xFF
        out[y * stride:(y + 1) * stride] = line
        prev = line
    return w, h, out


def green_bar_score(path):
    """Longest horizontal run of green pixels in the top ~25% of the frame. A
    health BAR is a long contiguous green run; scattered character/stage green is
    short. Validated on TMNT TF (fight ~88, menus/story 0)."""
    w, h, px = load_rgba(path)
    band = max(1, int(h * 0.25))
    best = 0
    for y in range(band):
        row = y * w * 4
        run = 0
        for x in range(w):
            i = row + x * 4
            r, g, b = px[i], px[i + 1], px[i + 2]
            if g >= 110 and g > r + 40 and g > b + 40:
                run += 1
                if run > best:
                    best = run
            else:
                run = 0
    return best


if __name__ == "__main__":
    print(green_bar_score(sys.argv[1]))
