#!/usr/bin/env python3
"""Recreate Asura Blade art assets as they appear in game.

Combines TWO sources the running core can't combine for us:
  * live arrangement data — tilemap RAM, sprite list, palette RAM, scroll and
    priority registers — read over the rustretro MCP server's bus windows
    (the Sek snapshot bridge; launch with
    --bus-map library/asurabld/asurabld.busmap.json), and
  * tile pixel data decoded straight from the ROM files (fbalpha2012 never
    exposes graphics ROM), reusing decode_asurabld.py's verified Fuuki FG-3
    decoders.

Hardware facts from the MAME fuukifg3 driver (drivers/fuukifg3.c +
vidhrdw/fuukifg3_vidhrdw.c, mame2003-plus tree):
  tilemap cell u32   = code<<16 | attr; 64x32 cells, scan_rows
  16x16 8bpp layers    L0 gfx=bg1012(lo)+bg1113(hi) pens base 0x000
                       L1 gfx=bg2022(lo)+bg2123(hi) pens base 0x400
                       color=(attr&0x3f)>>4, pen=base+color*256+pix, pix 0xFF clear
  8x8 4bpp layer       two double-buffered PAGES (0x504000 / 0x506000) of one
                       layer, page picked by vregs[0x1e]&0x40; gfx=map.u5,
                       color=attr&0x3f, pen=0xC00+color*16+pix, pix 0xF clear
  sprite entry 2xu32 = [sx16|sy16],[attr16|code16]; xnum=((sx>>12)&0xf)+1,
                       ynum likewise; skip when sx&0x400; flips sx/sy bit 0x800;
                       code bank = top 2 bits -> tilebank nibble lookup;
                       pal=attr&0x3f -> pens 0x800+pal*16, pix 15 clear;
                       cel k of an assembled sprite sits at (k%xnum, k//xnum),
                       cels increment code
  palette RAM        = xRGB555, one 16-bit color per 2 bytes (big-endian)
  scroll             = vregs[0]=L0 (Y hi16, X lo16), vregs[4]=L1, vregs[8]=8x8
                       layer; global offs vregs[0xc] minus (0x1f3, 0x3f6)
  priority           = (priority[0]>>16)&0xf indexes a 6-entry order table

Usage (instance from the repo root, MCP on port 4011):
  python3 scripts/re/asura_assets.py palettes
  python3 scripts/re/asura_assets.py bg
  python3 scripts/re/asura_assets.py sprites [--watch 10]   # per-entry strips
  python3 scripts/re/asura_assets.py poses   [--watch 20]   # assembled, deduped poses
  python3 scripts/re/asura_assets.py verify
Common flags: --port N (default 4011), --out DIR (default
library/asurabld/assets), --rom-dir DIR (default from decode_asurabld).
"""

import argparse
import json
import os
import struct
import sys
import time
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import decode_asurabld as dec  # noqa: E402  (swizzle/expand4/combine8 + ROMDIR)
import screen_tools  # noqa: E402  (stdlib PNG reader for app://screen)

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                 "..", "..", "shadow", "train"))
from shadow_train.mcpclient import McpClient  # noqa: E402


# ---------------------------------------------------------------- MCP client
class Mcp(McpClient):
    """asura_assets' Mcp -- constructed by port, eager-handshakes like before;
    adds the region-read and screen_rgba helpers this script needs."""

    def __init__(self, port):
        super().__init__(f"http://127.0.0.1:{port}/mcp", timeout=30.0,
                          client_name="asura-assets")
        self.connect()

    def read_region(self, name, offset, length):
        """Chunked region read (server caps read_region at 8 KiB per call)."""
        out = bytearray()
        while len(out) < length:
            n = min(8192, length - len(out))
            r = self.call("read_region", region_name=name,
                          offset=offset + len(out), len=n)
            if "hex" not in r:
                raise RuntimeError(f"read_region {name}: {r}")
            out += bytes.fromhex(r["hex"].replace(" ", ""))
        return bytes(out)

    def screen_rgba(self):
        """(w, h, rgba bytes) of the live framebuffer via app://screen."""
        png = self.read_resource("app://screen")
        tmp = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                           ".screen_tmp.png")
        with open(tmp, "wb") as f:
            f.write(png)
        try:
            return screen_tools.load_rgba(tmp)
        finally:
            os.unlink(tmp)


# ---------------------------------------------------------------- RGBA PNG out
def write_png_rgba(path, width, height, rgba):
    raw = b"".join(b"\x00" + rgba[y * width * 4:(y + 1) * width * 4]
                   for y in range(height))

    def chunk(typ, data):
        c = typ + data
        return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c))

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", ihdr))
        f.write(chunk(b"IDAT", zlib.compress(raw, 6)))
        f.write(chunk(b"IEND", b""))


# ---------------------------------------------------------------- ROM pixels
class RomPixels:
    """Lazy per-file tile pixel providers (1 byte/pixel, tile-major)."""

    # Sprite region: first 4 MB slot is EMPTY on asurabld (sp01.u13 exists only
    # on asurabus), so sprite codes 0x0000-0x7FFF are blank.
    SPRITE_FILES = [None, "sp23.u14", "sp45.u15", "sp67.u16",
                    "sp89.u17", "spab.u18", "spcd.u19"]

    def __init__(self, romdir):
        self.romdir = romdir
        self._sprites = {}
        self._bg = {}
        self._map = None

    def _load(self, name):
        with open(os.path.join(self.romdir, name), "rb") as f:
            return f.read()

    def sprite_tile(self, code):
        """256 pixel bytes (16x16) for a sprite tile code, or None if blank."""
        fi, local = code >> 15, code & 0x7FFF
        if fi >= len(self.SPRITE_FILES) or self.SPRITE_FILES[fi] is None:
            return None
        if fi not in self._sprites:
            self._sprites[fi] = dec.expand4(self._load(self.SPRITE_FILES[fi]))
        return self._sprites[fi][local * 256:local * 256 + 256]

    def bg_tile(self, layer, code):
        """256 pixel bytes (16x16, 8bpp pens) for a bg-layer tile code."""
        if layer not in self._bg:
            lo, hi = (("bg1012.u22", "bg1113.u23") if layer == 0
                      else ("bg2022.u25", "bg2123.u24"))
            self._bg[layer] = dec.combine8(self._load(lo), self._load(hi))
        return self._bg[layer][code * 256:code * 256 + 256]

    def map_tile(self, code):
        """64 pixel bytes (8x8) for a map.u5 tile code."""
        if self._map is None:
            self._map = dec.expand4(self._load("map.u5"))
        return self._map[code * 64:code * 64 + 64]


# ---------------------------------------------------------------- live state
class Live:
    def __init__(self, mcp):
        self.mcp = mcp

    def palette(self):
        """8192 (r,g,b) tuples from palette RAM (xRGB555, 2 bytes/color)."""
        raw = self.mcp.read_region("Palette RAM", 0, 0x4000)
        out = []
        for i in range(0, len(raw), 2):
            v = (raw[i] << 8) | raw[i + 1]
            out.append((((v >> 10) & 0x1F) * 255 // 31,
                        ((v >> 5) & 0x1F) * 255 // 31,
                        (v & 0x1F) * 255 // 31))
        return out

    def tilemap(self, region):
        """List of (code, attr) for a 64x32 tilemap region."""
        raw = self.mcp.read_region(region, 0, 0x2000)
        return [((raw[i] << 8) | raw[i + 1], (raw[i + 2] << 8) | raw[i + 3])
                for i in range(0, len(raw), 4)]

    def sprites(self):
        """Raw (sx, sy, attr, code) entries from sprite RAM."""
        raw = self.mcp.read_region("Sprite RAM", 0, 0x2000)
        out = []
        for i in range(0, len(raw), 8):
            sx = (raw[i] << 8) | raw[i + 1]
            sy = (raw[i + 2] << 8) | raw[i + 3]
            attr = (raw[i + 4] << 8) | raw[i + 5]
            code = (raw[i + 6] << 8) | raw[i + 7]
            out.append((sx, sy, attr, code))
        return out

    def dword(self, region):
        raw = self.mcp.read_region(region, 0, 4)
        return struct.unpack(">I", raw)[0]

    def vregs(self):
        raw = self.mcp.read_region("Video Regs", 0, 0x20)
        return [struct.unpack(">H", raw[i:i + 2])[0] for i in range(0, 0x20, 2)]


def _sprite_screen_score(rom, base_code, pal, sxr, syr, pens, scr):
    """(matched, compared) opaque sprite pixels vs the live framebuffer."""
    scr_w, scr_h, scr_px = scr
    xnum, ynum = ((sxr >> 12) & 0xF) + 1, ((syr >> 12) & 0xF) + 1
    px0 = (sxr & 0x1FF) - (sxr & 0x200)
    py0 = (syr & 0x1FF) - (syr & 0x200)
    flipx, flipy = bool(sxr & 0x800), bool(syr & 0x800)
    w, h = xnum * 16, ynum * 16
    matched = compared = 0
    for k in range(xnum * ynum):
        tile = rom.sprite_tile(base_code + k)
        if tile is None:
            continue
        cx, cy = (k % xnum) * 16, (k // xnum) * 16
        for ty in range(16):
            for tx in range(16):
                p = tile[ty * 16 + tx]
                if p == 15:
                    continue
                x, y = cx + tx, cy + ty
                dx = px0 + (w - 1 - x if flipx else x)
                dy = py0 + (h - 1 - y if flipy else y)
                if not (0 <= dx < scr_w and 0 <= dy < scr_h):
                    continue
                r, g, b = pens[0x800 + pal * 16 + p]
                o = (dy * scr_w + dx) * 4
                compared += 1
                if (abs(scr_px[o] - r) + abs(scr_px[o + 1] - g)
                        + abs(scr_px[o + 2] - b)) <= 24:
                    matched += 1
    return matched, compared


def calibrate_tilebank(entries, rom, pens, scr):
    """Infer the sprite tilebank nibble for each of the 4 code banks.

    The tilebank register (0xA00000) is WRITE-ONLY on the bus — FBA installs no
    read handler, so a bus read returns 0, not the live value. But the screen
    itself is a decisive oracle: for each bank, the nibble whose decoded tiles
    pixel-match the framebuffer at the sprites' own positions and palettes is
    the one the game programmed. Zoomed sprites are excluded from scoring
    (their screen footprint differs). Returns {bank: nibble} for banks whose
    sprites produced any comparable pixels.
    """
    scr_w, scr_h, _ = scr
    by_bank = {}
    for sx, sy, attr, code in entries:
        if sx & 0x400:
            continue
        unzoomed = ((attr >> 10) & 0x3C) == 0 and ((attr >> 6) & 0x3C) == 0
        # Only sprites actually ON screen can vote: games park machinery at
        # off-screen positions (still "visible" by flag), and a parked sprite
        # matched against background pixels yields a confidently wrong nibble.
        px = (sx & 0x1FF) - (sx & 0x200)
        py = (sy & 0x1FF) - (sy & 0x200)
        w, h = (((sx >> 12) & 0xF) + 1) * 16, (((sy >> 12) & 0xF) + 1) * 16
        on_screen = (min(px + w, scr_w) - max(px, 0) >= 8
                     and min(py + h, scr_h) - max(py, 0) >= 8)
        if unzoomed and on_screen:
            by_bank.setdefault((code >> 14) & 3, []).append((sx, sy, attr, code))
    nibbles = {}
    for bank, ents in by_bank.items():
        best, best_frac, best_cmp = None, 0.0, 0
        for n in range(16):
            matched = compared = 0
            for sx, sy, attr, code in ents:
                base = (code & 0x3FFF) | (n << 14)
                m, c = _sprite_screen_score(rom, base, attr & 0x3F,
                                            sx, sy, pens, scr)
                matched += m
                compared += c
            frac = matched / compared if compared else 0.0
            if frac > best_frac:
                best, best_frac, best_cmp = n, frac, compared
        # Require a majority match over a meaningful pixel count — below that
        # we'd be guessing, and a wrong bank poisons the asset dump.
        if best is not None and best_frac >= 0.5 and best_cmp >= 200:
            nibbles[bank] = best
    return nibbles


def sprite_code_banked(code, nibbles):
    """Apply the calibrated tilebank mapping to a raw sprite code field."""
    bank = (code >> 14) & 3
    if bank not in nibbles:
        return None
    return (code & 0x3FFF) | (nibbles[bank] << 14)


# ---------------------------------------------------------------- renderers
def render_cell(img, width, px_bytes, tile_px, x0, y0, pens, pen_base,
                clear_pix, flipx=False, flipy=False):
    """Blit one tile's pens through the palette into an RGBA bytearray."""
    for ty in range(tile_px):
        sy = tile_px - 1 - ty if flipy else ty
        row = px_bytes[sy * tile_px:(sy + 1) * tile_px]
        dst = ((y0 + ty) * width + x0) * 4
        for tx in range(tile_px):
            p = row[tile_px - 1 - tx if flipx else tx]
            if p == clear_pix:
                continue
            r, g, b = pens[pen_base + p]
            o = dst + tx * 4
            img[o:o + 4] = bytes((r, g, b, 255))


def render_16x16_layer(cells, rom, layer, pens):
    """Full 1024x512 RGBA of a 16x16 8bpp tilemap layer."""
    width, height = 64 * 16, 32 * 16
    img = bytearray(width * height * 4)
    base = 0x000 if layer == 0 else 0x400
    for i, (code, attr) in enumerate(cells):
        color = (attr & 0x3F) >> 4
        flipx, flipy = bool(attr & 0x40), bool(attr & 0x80)
        px = rom.bg_tile(layer, code)
        render_cell(img, width, px, 16, (i % 64) * 16, (i // 64) * 16,
                    pens, base + color * 256, 0xFF, flipx, flipy)
    return width, height, bytes(img)


def render_8x8_layer(cells, rom, pens):
    """Full 512x256 RGBA of one 8x8 4bpp tilemap page."""
    width, height = 64 * 8, 32 * 8
    img = bytearray(width * height * 4)
    for i, (code, attr) in enumerate(cells):
        color = attr & 0x3F
        flipx, flipy = bool(attr & 0x40), bool(attr & 0x80)
        px = rom.map_tile(code)
        render_cell(img, width, px, 8, (i % 64) * 8, (i // 64) * 8,
                    pens, 0xC00 + color * 16, 0xF, flipx, flipy)
    return width, height, bytes(img)


def render_sprite(rom, code, pal, xnum, ynum, pens):
    """Canonical (unflipped, unzoomed) assembled sprite frame -> RGBA."""
    width, height = xnum * 16, ynum * 16
    img = bytearray(width * height * 4)
    blank = True
    for k in range(xnum * ynum):
        px = rom.sprite_tile(code + k)
        if px is None:
            continue
        if blank and any(p != 15 for p in px):
            blank = False
        render_cell(img, width, px, 16, (k % xnum) * 16, (k // xnum) * 16,
                    pens, 0x800 + pal * 16, 15)
    return None if blank else (width, height, bytes(img))


# ---------------------------------------------------------------- subcommands
def cmd_palettes(live, rom, out):
    pens = live.palette()
    cell, per_row = 8, 64
    rows = (len(pens) + per_row - 1) // per_row
    width, height = per_row * cell, rows * cell
    img = bytearray(width * height * 4)
    for i, (r, g, b) in enumerate(pens):
        x0, y0 = (i % per_row) * cell, (i // per_row) * cell
        for y in range(cell):
            o = ((y0 + y) * width + x0) * 4
            img[o:o + cell * 4] = bytes((r, g, b, 255)) * cell
    path = os.path.join(out, "palette_ram.png")
    write_png_rgba(path, width, height, bytes(img))
    print(f"wrote {path} ({width}x{height}, {len(pens)} colors, "
          f"row = 64 colors; sprite pens start at row {0x800 // 64})")


def cmd_bg(live, rom, out):
    # Pause for a same-frame snapshot; the page-select flag is write-only on
    # this core, so render BOTH 8x8 pages and let the reader pick.
    live.mcp.call("pause")
    try:
        pens = live.palette()
        grabs = [(label, region, kind, live.tilemap(region)) for label, region, kind in (
            ("bg_layer0", "Tilemap L0", 0),
            ("bg_layer1", "Tilemap L1", 1),
            ("bg_8x8_page0", "Tilemap BG", None),
            ("bg_8x8_page1", "Tilemap BG2", None))]
    finally:
        live.mcp.call("resume")
    for label, region, kind, cells in grabs:
        if kind is None:
            w, h, img = render_8x8_layer(cells, rom, pens)
        else:
            w, h, img = render_16x16_layer(cells, rom, kind, pens)
        path = os.path.join(out, f"{label}.png")
        write_png_rgba(path, w, h, img)
        print(f"wrote {path} ({w}x{h}) from {region}")


def cmd_sprites(live, rom, out, watch=0.0):
    os.makedirs(os.path.join(out, "sprites"), exist_ok=True)
    seen = {}
    deadline = time.time() + watch
    while True:
        pens = live.palette()
        entries = live.sprites()
        scr = live.mcp.screen_rgba()
        nibbles = calibrate_tilebank(entries, rom, pens, scr)
        for sx, sy, attr, rawcode in entries:
            if sx & 0x400:
                continue
            code = sprite_code_banked(rawcode, nibbles)
            if code is None:
                continue
            pal = attr & 0x3F
            xnum, ynum = ((sx >> 12) & 0xF) + 1, ((sy >> 12) & 0xF) + 1
            key = (code, pal, xnum, ynum)
            if key in seen:
                continue
            rendered = render_sprite(rom, code, pal, xnum, ynum, pens)
            if rendered is None:
                continue
            w, h, img = rendered
            name = f"spr_{code:05X}_p{pal:02X}_{xnum}x{ynum}.png"
            write_png_rgba(os.path.join(out, "sprites", name), w, h, img)
            seen[key] = name
        if time.time() >= deadline:
            break
        time.sleep(0.25)
    manifest = os.path.join(out, "sprites", "manifest.json")
    with open(manifest, "w") as f:
        json.dump({"count": len(seen),
                   "sprites": sorted(seen.values())}, f, indent=1)
    print(f"wrote {len(seen)} sprite frame(s) to {os.path.join(out, 'sprites')}")


def capture_frame(live):
    """Atomically capture palette + screen + sprite list from ONE paused frame.

    Everything a pose render needs must come from the same frame: attract mode
    swaps scenes (and palettes, and the tilebank) faster than sequential reads.
    """
    live.mcp.call("pause")
    try:
        pens = live.palette()
        scr = live.mcp.screen_rgba()
        entries = live.sprites()
    finally:
        live.mcp.call("resume")
    return pens, scr, entries


def decode_entries(entries, nibbles):
    """Visible, unzoomed, bank-resolved sprite entries as dicts.

    Zoomed entries are excluded on purpose: in practice they are the shadow
    blobs, drawn by the hardware as a darkening blend we cannot reproduce
    from pens (and their bank often can't be calibrated for the same reason).
    """
    rows = []
    for i, (sx, sy, attr, raw) in enumerate(entries):
        if sx & 0x400:
            continue
        if ((attr >> 10) & 0x3C) or ((attr >> 6) & 0x3C):
            continue
        code = sprite_code_banked(raw, nibbles)
        if code is None:
            continue
        rows.append(dict(
            i=i,
            x=(sx & 0x1FF) - (sx & 0x200), y=(sy & 0x1FF) - (sy & 0x200),
            xn=((sx >> 12) & 0xF) + 1, yn=((sy >> 12) & 0xF) + 1,
            fx=bool(sx & 0x800), fy=bool(sy & 0x800),
            pal=attr & 0x3F, code=code))
    return rows


def cluster_poses(rows, scr_w=320, scr_h=240, idx_gap=8, slack=8):
    """Group strips into poses: consecutive list indices + touching boxes.

    Empirically (see library/asurabld/asurabld.md) a pose is a contiguous
    index run whose strip boxes tile together. Palette is deliberately NOT a
    grouping key — a character's weapon/effect strips interleave the body run
    under a different palette. Groups mostly off screen (parked banner
    machinery at index 1000+) are dropped.
    """
    def bbox(r):
        return (r['x'], r['y'], r['x'] + r['xn'] * 16, r['y'] + r['yn'] * 16)

    def touches(a, b):
        ax0, ay0, ax1, ay1 = bbox(a)
        bx0, by0, bx1, by1 = bbox(b)
        return not (ax1 + slack < bx0 or bx1 + slack < ax0
                    or ay1 + slack < by0 or by1 + slack < ay0)

    groups = []
    for r in rows:
        merged = None
        for g in groups:
            if (r['i'] - g[-1]['i'] <= idx_gap and any(touches(r, m) for m in g)):
                if merged is None:
                    g.append(r)
                    merged = g
                else:
                    merged.extend(g)   # r bridges two earlier groups
                    merged.sort(key=lambda m: m['i'])
                    g.clear()
        groups = [g for g in groups if g]
        if merged is None:
            groups.append([r])

    kept = []
    for g in groups:
        x0 = min(m['x'] for m in g); y0 = min(m['y'] for m in g)
        x1 = max(m['x'] + m['xn'] * 16 for m in g)
        y1 = max(m['y'] + m['yn'] * 16 for m in g)
        vis_w = min(x1, scr_w) - max(x0, 0)
        vis_h = min(y1, scr_h) - max(y0, 0)
        if vis_w >= 8 and vis_h >= 8:
            kept.append(g)
    return kept


def canonicalize(group):
    """Relative strip layout, mirrored to unflipped when uniformly flipped.

    Returns (strips, W, H) where strips are (code, relx, rely, xn, yn, fx, fy,
    pal) — a left-facing capture and its right-facing twin normalize to the
    same layout, so they dedupe together.
    """
    x0 = min(m['x'] for m in group); y0 = min(m['y'] for m in group)
    W = max(m['x'] + m['xn'] * 16 for m in group) - x0
    H = max(m['y'] + m['yn'] * 16 for m in group) - y0
    flip_all_x = all(m['fx'] for m in group)
    flip_all_y = all(m['fy'] for m in group)
    strips = []
    for m in sorted(group, key=lambda m: m['i']):
        rx, ry = m['x'] - x0, m['y'] - y0
        w, h = m['xn'] * 16, m['yn'] * 16
        fx, fy = m['fx'], m['fy']
        if flip_all_x:
            rx, fx = W - (rx + w), False
        if flip_all_y:
            ry, fy = H - (ry + h), False
        strips.append((m['code'], rx, ry, m['xn'], m['yn'], fx, fy, m['pal']))
    return strips, W, H


def render_pose(rom, strips, W, H, pens):
    """RGBA canvas of a canonical pose; back-to-front = reverse list order."""
    img = bytearray(W * H * 4)
    blank = True
    for code, rx, ry, xn, yn, fx, fy, pal in reversed(strips):
        r = render_sprite(rom, code, pal, xn, yn, pens)
        if r is None:
            continue
        blank = False
        w, h, px = r
        for y in range(h):
            dy = ry + (h - 1 - y if fy else y)
            for x in range(w):
                dx = rx + (w - 1 - x if fx else x)
                o = (y * w + x) * 4
                if px[o + 3]:
                    d = (dy * W + dx) * 4
                    img[d:d + 4] = px[o:o + 4]
    return None if blank else bytes(img)


def cmd_poses(live, rom, out, watch=0.0, slots=None):
    """Harvest deduplicated pose-level assets (assembled multi-strip sprites).

    `slots` restricts to a sprite-list index range. The game slot-allocates
    the list (HUD ~70-110 and ~700+, shadows ~280, characters ~300-699), and
    the HUD's timer digits change every frame — each tick would otherwise
    mint a spuriously "unique" HUD pose. `--slots 300-699` keeps the harvest
    to characters and their effects.
    """
    pose_dir = os.path.join(out, "poses")
    os.makedirs(pose_dir, exist_ok=True)
    seen = {}
    frames = 0
    deadline = time.time() + watch
    while True:
        pens, scr, entries = capture_frame(live)
        frames += 1
        nibbles = calibrate_tilebank(entries, rom, pens, scr)
        rows = decode_entries(entries, nibbles)
        if slots:
            rows = [r for r in rows if slots[0] <= r['i'] <= slots[1]]
        for group in cluster_poses(rows):
            strips, W, H = canonicalize(group)
            sig = tuple(sorted(strips))
            if sig in seen:
                seen[sig]["hits"] += 1
                continue
            img = render_pose(rom, strips, W, H, pens)
            if img is None:
                continue
            name = (f"pose_{strips[0][0]:05X}_{W}x{H}_{len(strips)}s"
                    f"_{abs(hash(sig)) % 0xFFFF:04X}.png")
            write_png_rgba(os.path.join(pose_dir, name), W, H, img)
            seen[sig] = {
                "file": name, "hits": 1, "strips": len(strips),
                "size": [W, H],
                "codes": sorted({s[0] for s in strips}),
                "palettes": sorted({s[7] for s in strips}),
            }
        if time.time() >= deadline:
            break
        time.sleep(0.25)
    manifest = {
        "frames_sampled": frames,
        "unique_poses": len(seen),
        "poses": sorted(seen.values(), key=lambda p: p["file"]),
    }
    with open(os.path.join(pose_dir, "manifest.json"), "w") as f:
        json.dump(manifest, f, indent=1)
    print(f"{len(seen)} unique pose(s) from {frames} frame(s) -> {pose_dir}")


PRI_TABLE = [(0, 1, 2), (0, 2, 1), (1, 0, 2), (1, 2, 0), (2, 0, 1), (2, 1, 0)]


def infer_scroll(layer, scr, step=2, samples=120):
    """Find the (sx, sy) layer scroll that best explains the screen.

    The scroll registers (0x8C0000) are write-only through FBA's bus handlers
    (reads return 0), so — like the tilebank — the framebuffer is the oracle:
    coarse-to-fine search of the offset whose layer pixels best match the
    screen at sampled points. Occlusion by sprites/front layers just costs a
    few sampled points; the true offset still wins.
    """
    import random
    lw, lh, img = layer
    scr_w, scr_h, scr_px = scr
    rng = random.Random(1)
    pts = [(rng.randrange(scr_w), rng.randrange(scr_h)) for _ in range(samples)]

    def score(sx, sy):
        total = 0
        for x, y in pts:
            o = (((y + sy) % lh) * lw + (x + sx) % lw) * 4
            if img[o + 3]:
                s = (y * scr_w + x) * 4
                total += (abs(img[o] - scr_px[s]) + abs(img[o + 1] - scr_px[s + 1])
                          + abs(img[o + 2] - scr_px[s + 2]))
            else:
                total += 96 * 3  # transparent: neutral penalty
        return total

    best, best_s = (0, 0), None
    for sy in range(0, lh, step):
        for sx in range(0, lw, step):
            s = score(sx, sy)
            if best_s is None or s < best_s:
                best, best_s = (sx, sy), s
    bx, by = best
    for sy in range(by - step, by + step + 1):
        for sx in range(bx - step, bx + step + 1):
            s = score(sx % lw, sy % lh)
            if s < best_s:
                best, best_s = (sx % lw, sy % lh), s
    return best


def cmd_verify(live, rom, out):
    """Reassemble the visible 320x240 from RAM and diff against app://screen."""
    sw, sh = 320, 240

    # Pause so palette, tilemaps, sprites, and framebuffer are all snapshots
    # of the SAME frame — attract mode changes scenes faster than we read.
    live.mcp.call("pause")
    try:
        pens = live.palette()
        scr_for_cal = live.mcp.screen_rgba()
        entries = live.sprites()
        cells = {0: live.tilemap("Tilemap L0"),
                 1: live.tilemap("Tilemap L1"),
                 2: live.tilemap("Tilemap BG")}
    finally:
        live.mcp.call("resume")

    layers = {
        0: render_16x16_layer(cells[0], rom, 0, pens),
        1: render_16x16_layer(cells[1], rom, 1, pens),
        2: render_8x8_layer(cells[2], rom, pens),
    }
    scroll = {li: infer_scroll(layers[li], scr_for_cal) for li in layers}
    print(f"inferred scrolls: {scroll}")

    def composite(order):
        comp = bytearray(sw * sh * 4)
        for li in (order[2], order[1], order[0]):  # back to front
            lw, lh, img = layers[li]
            sx, sy = scroll[li]
            for y in range(sh):
                src_y = (y + sy) % lh
                for x in range(sw):
                    o = (src_y * lw + (x + sx) % lw) * 4
                    if img[o + 3]:
                        d = (y * sw + x) * 4
                        comp[d:d + 4] = img[o:o + 4]
        return comp

    # Priority is write-only too: try all six hardware orders, keep the best.
    scr_w0, scr_h0, scr_px0 = scr_for_cal
    best_comp, best_order, best_mad = None, None, None
    for order in PRI_TABLE:
        comp = composite(order)
        total = n = 0
        for i in range(0, sw * sh * 4, 16 * 4):  # sample every 16th pixel
            if comp[i + 3]:
                y, x = divmod(i // 4, sw)
                if x < scr_w0 and y < scr_h0:
                    s = (y * scr_w0 + x) * 4
                    total += (abs(comp[i] - scr_px0[s])
                              + abs(comp[i + 1] - scr_px0[s + 1])
                              + abs(comp[i + 2] - scr_px0[s + 2]))
                    n += 1
        mad = total / (n * 3) if n else 255.0
        if best_mad is None or mad < best_mad:
            best_comp, best_order, best_mad = comp, order, mad
    comp = best_comp
    print(f"best layer order (front,middle,back): {best_order}")

    # All visible sprites on top (sprite<->layer priority approximated as
    # front; good enough for a match score, stated honestly below).
    nibbles = calibrate_tilebank(entries, rom, pens, scr_for_cal)
    for sxr, syr, attr, rawcode in entries:
        if sxr & 0x400:
            continue
        code = sprite_code_banked(rawcode, nibbles)
        if code is None:
            continue
        xnum, ynum = ((sxr >> 12) & 0xF) + 1, ((syr >> 12) & 0xF) + 1
        rendered = render_sprite(rom, code, attr & 0x3F, xnum, ynum, pens)
        if rendered is None:
            continue
        w, h, img = rendered
        px = (sxr & 0x1FF) - (sxr & 0x200)
        py = (syr & 0x1FF) - (syr & 0x200)
        flipx, flipy = bool(sxr & 0x800), bool(syr & 0x800)
        for y in range(h):
            dy = py + (h - 1 - y if flipy else y)
            if not 0 <= dy < sh:
                continue
            for x in range(w):
                dx = px + (w - 1 - x if flipx else x)
                if not 0 <= dx < sw:
                    continue
                o = (y * w + x) * 4
                if img[o + 3]:
                    d = (dy * sw + dx) * 4
                    comp[d:d + 4] = img[o:o + 4]

    scr_w, scr_h, scr = scr_for_cal
    side = bytearray((sw * 2 + 8) * sh * 4)
    row_w = sw * 2 + 8
    diff_total = diff_n = 0
    for y in range(sh):
        for x in range(sw):
            c = comp[(y * sw + x) * 4:(y * sw + x) * 4 + 4]
            s = (scr[(y * scr_w + x) * 4:(y * scr_w + x) * 4 + 4]
                 if x < scr_w and y < scr_h else b"\x00\x00\x00\xff")
            side[(y * row_w + x) * 4:(y * row_w + x) * 4 + 4] = s
            side[(y * row_w + sw + 8 + x) * 4:(y * row_w + sw + 8 + x) * 4 + 4] = \
                c if c[3] else b"\x00\x00\x00\xff"
            if c[3]:
                diff_total += (abs(c[0] - s[0]) + abs(c[1] - s[1])
                               + abs(c[2] - s[2]))
                diff_n += 1
    path = os.path.join(out, "verify_side_by_side.png")
    write_png_rgba(path, row_w, sh, bytes(side))
    mad = diff_total / (diff_n * 3) if diff_n else 255.0
    print(f"wrote {path} (left = live screen, right = reconstruction)")
    print(f"mean |channel delta| over reconstructed pixels: {mad:.1f}/255 "
          f"(coverage {100.0 * diff_n / (sw * sh):.1f}%; sprite-vs-layer "
          f"priority approximated, per-sprite zoom ignored)")


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("cmd", choices=["palettes", "bg", "sprites", "poses", "verify"])
    ap.add_argument("--port", type=int, default=4011)
    ap.add_argument("--out", default="library/asurabld/assets")
    ap.add_argument("--rom-dir", default=dec.ROMDIR)
    ap.add_argument("--watch", type=float, default=0.0,
                    help="sprites/poses: keep sampling for N seconds")
    ap.add_argument("--slots", default=None, metavar="LO-HI",
                    help="poses: only sprite-list indices LO..HI "
                         "(asurabld characters: 300-699)")
    args = ap.parse_args()

    os.makedirs(args.out, exist_ok=True)
    live = Live(Mcp(args.port))
    rom = RomPixels(args.rom_dir)
    if args.cmd == "palettes":
        cmd_palettes(live, rom, args.out)
    elif args.cmd == "bg":
        cmd_bg(live, rom, args.out)
    elif args.cmd == "sprites":
        cmd_sprites(live, rom, args.out, args.watch)
    elif args.cmd == "poses":
        slots = None
        if args.slots:
            lo, hi = args.slots.split("-")
            slots = (int(lo), int(hi))
        cmd_poses(live, rom, args.out, args.watch, slots)
    elif args.cmd == "verify":
        cmd_verify(live, rom, args.out)


if __name__ == "__main__":
    main()
