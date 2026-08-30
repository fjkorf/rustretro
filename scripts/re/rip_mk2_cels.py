#!/usr/bin/env python3
"""Rip Mortal Kombat II character cels out of LIVE VRAM into sprite sheets.

Unlike asura_assets.py, this needs NO graphics-ROM decoding. Midway T-Unit
hardware makes the job trivial once you know the trick: the DMA blitter stamps
a PALETTE BANK into the high byte of every pixel it writes, so a fighter's
pixels are already tagged as belonging to that fighter.

Hardware facts (FBNeo `src/burn/drv/midway/midtunit.cpp`, verified live —
see library/mk2/mk2.md "Ripping character cels from VRAM"):
  VRAM word        = palette_bank<<8 | pixel; 512 words per row (`rowaddr << 9`)
  displayed pixel  = DrvPaletteB[word & 0x7FFF]        (ScanlineRender, :547)
  palette entry    = xRGB1555, little-endian, 2 bytes  (TUnitPalRecalc, :497)
  blitter write    = pixel | (DMA_PALETTE & 0xff) << 8 (TUnitDmaWrite, :469)
Because the blitter SKIPS transparent pixels rather than writing them, every
pixel carrying a bank is opaque — the mask comes out clean, with alpha, for
free. The cost is that a cel is only ever captured as DRAWN: anything painted
over the fighter (foreground scenery) punches a hole, and frames the game
never displays are simply absent. For the complete asset set you would have to
decode the `*-vid` ROMs instead.

Region offsets come from `library/mk2/<port>.profile.json` -> "video"; block /
global addresses and the character roster come from the same profile, via
shadow_train.profile. No addresses are hardcoded here.

Usage (from the repo root; the app must be running with --mcp):
  python3 scripts/re/rip_mk2_cels.py banks --port 4026
  python3 scripts/re/rip_mk2_cels.py rip   --port 4026 --samples 240 --step 2
  python3 scripts/re/rip_mk2_cels.py rip   --port 4026 --watch --interval 0.2
  python3 scripts/re/rip_mk2_cels.py selftest
`rip` writes <out>/<character>.png + .json, keyed by character (both players
feed the same sheet, so a mirror match doubles coverage).

PORT SAFETY: 4025 is the human's session by convention. Frame-exact mode
pauses and steps the emulator, so it refuses to touch 4025 without --force.
"""

import argparse
import hashlib
import json
import os
import struct
import sys
import time
import zlib
from collections import Counter

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import screen_tools  # noqa: E402  (stdlib PNG reader for app://screen)

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                "..", "..", "shadow", "train"))
from shadow_train.mcpclient import McpClient  # noqa: E402
from shadow_train import profile as profile_mod  # noqa: E402

USER_SESSION_PORT = 4025


# ---------------------------------------------------------------- MCP client
class Mcp(McpClient):
    """Constructed by port; adds chunked region reads and a screen grab."""

    def __init__(self, port, timeout=30.0):
        super().__init__(f"http://127.0.0.1:{port}/mcp", timeout=timeout,
                         client_name="rip-mk2-cels")
        self.port = port
        self.connect()

    def region_name(self):
        """The one region this core publishes (FBNeo names it, we don't guess)."""
        regions = self.call("list_regions")
        if not regions:
            raise RuntimeError("core publishes no memory regions")
        return regions[0]["name"]

    def read_region_bytes(self, name, offset, length):
        """Chunked region read (the server caps read_region at 8 KiB/call)."""
        out = bytearray()
        while len(out) < length:
            n = min(8192, length - len(out))
            r = self.call("read_region", region_name=name,
                          offset=offset + len(out), len=n)
            if "hex" not in r:
                raise RuntimeError(f"read_region {name}+{offset:#x}: {r}")
            out += bytes.fromhex(r["hex"].replace(" ", ""))
        return bytes(out)

    def screen_rgba(self, tmp_dir):
        """(w, h, rgba) of the live framebuffer via app://screen."""
        tmp = os.path.join(tmp_dir, ".screen_tmp.png")
        with open(tmp, "wb") as f:
            f.write(self.read_resource("app://screen"))
        try:
            return screen_tools.load_rgba(tmp)
        finally:
            os.unlink(tmp)


# ---------------------------------------------------------------- video map
class VideoMap:
    """The profile's "video" block: where VRAM and palette live in the blob."""

    def __init__(self, raw):
        try:
            pal, vram = raw["palette"], raw["vram"]
            self.pal_off = profile_mod._parse_addr(pal["off"])
            self.pal_entries = int(pal["entries"])
            self.vram_off = profile_mod._parse_addr(vram["off"])
            self.row_words = int(vram["row_words"])
            self.rows = int(vram["rows"])
            self.visible_width = int(vram["visible_width"])
            self.index_mask = profile_mod._parse_addr(vram["index_mask"])
            # Read past the visible height so a nonzero display origin still
            # has rows to point at (the origin is CPU state we can't read).
            self.rows_total = int(vram.get("rows_total", self.rows))
            self.capture_rows = min(self.rows_total, self.rows + 128)
        except (KeyError, TypeError, ValueError) as e:
            raise SystemExit(f"profile 'video' block is malformed: {e}")

    @classmethod
    def from_profile(cls, prof):
        raw = prof.port_raw.get("video")
        if raw is None:
            raise SystemExit(
                f"profile {prof.dir} has no 'video' block — this ripper needs "
                "video.palette/vram offsets (see library/mk2/mk2.profile.json)")
        return cls(raw)

    def as_json(self):
        return {"palette_off": hex(self.pal_off), "vram_off": hex(self.vram_off),
                "row_words": self.row_words, "rows": self.rows,
                "visible_width": self.visible_width,
                "index_mask": hex(self.index_mask),
                "capture_rows": self.capture_rows}


def rgb555_table(pal_raw, full_range=False):
    """xRGB1555 LE bytes -> [(r, g, b)].

    Default is EMULATOR-EXACT: `v << 3`, low bits zero, exactly what reaches
    the screen. Chasing that down is what makes the alignment check strict —
    FBNeo's RGB555_2_888 masks with 0xF8 (midtunit.cpp:86), BurnHighCol packs
    to RGB565, and rustretro's decode_to_rgba re-expands r5<<3 / g6<<2 / b5<<3
    (src/debug/mod.rs:1333). Green survives as (v<<1)<<2 == v<<3, so the whole
    chain is the identity on v<<3 and ripped pixels match a screenshot exactly.

    `full_range` instead bit-replicates ((v<<3)|(v>>2)), the conventional
    sprite-rip expansion where 31 -> 255 rather than 248. Prettier, but then
    the cels no longer compare equal to the emulator's own output.
    """
    out = []
    for i in range(0, len(pal_raw) - 1, 2):
        v = pal_raw[i] | (pal_raw[i + 1] << 8)
        r, g, b = (v >> 10) & 31, (v >> 5) & 31, v & 31
        if full_range:
            out.append(((r << 3) | (r >> 2), (g << 3) | (g >> 2), (b << 3) | (b >> 2)))
        else:
            out.append((r << 3, g << 3, b << 3))
    return out


# ---------------------------------------------------------------- capture
class Frame:
    """One captured video frame: VRAM words + the palette that colors them.

    `hi` is the high byte of every word — i.e. one palette-bank id per pixel,
    as a flat `bytes`. Keeping it separate lets the census run at C speed
    (`bytes.count`/`find`) instead of looping 100k pixels in Python.
    """

    def __init__(self, raw, pal, vm, frame_count, coherent, align=(0, 0)):
        self.words = struct.unpack(f"<{len(raw) // 2}H", raw)
        self.hi = raw[1::2]
        self.pal = pal
        self.vm = vm
        self.frame_count = frame_count
        self.coherent = coherent
        self.captured_rows = len(self.words) // vm.row_words
        self.align = align

    def color(self, word):
        return self.pal[word & self.vm.index_mask]

    def word_at(self, x, y):
        """VRAM word under DISPLAY pixel (x, y), honoring the display origin."""
        ry, cx = y + self.align[0], self.align[1]
        return self.words[ry * self.vm.row_words
                          + ((cx + x) & (self.vm.row_words - 1))]

    def hi_row(self, y):
        """The palette-bank byte of every VISIBLE pixel on display row `y`."""
        rw, vw = self.vm.row_words, self.vm.visible_width
        base = (y + self.align[0]) * rw
        row = self.hi[base:base + rw]
        c = self.align[1]
        if c + vw <= rw:
            return row[c:c + vw]
        return row[c:] + row[:c + vw - rw]      # wraps, exactly as src[col & 0x1FF]


def capture(mcp, region, vm, pal_cache=None, coherent=True, full_range=False,
            align=(0, 0), rows=None):
    """Read palette + VRAM. `coherent` records whether emulation was paused:
    a running game advances between our chunked reads, so an unpaused capture
    can be TORN across frames (half of one pose, half of the next)."""
    pal = pal_cache
    if pal is None:
        pal = rgb555_table(mcp.read_region_bytes(region, vm.pal_off,
                                                 vm.pal_entries * 2), full_range)
    rows = rows or vm.capture_rows
    raw = mcp.read_region_bytes(region, vm.vram_off, rows * vm.row_words * 2)
    frame_count = mcp.call("get_state").get("frame_count", -1)
    return Frame(raw, pal, vm, frame_count, coherent, align)


# ---------------------------------------------------------------- alignment
def find_alignment(frame, w, h, scr, probes=(0.25, 0.5, 0.75)):
    """Locate the display origin: (row_off, col_off) into the captured VRAM.

    ScanlineRender draws from `DrvVRAM16[(rowaddr << 9) & 0x3FE00]` starting at
    column `coladdr << 1` (midtunit.cpp:531-547). Both come from the TMS34010's
    display registers, which are CPU state — NOT in the exposed RAM region — so
    we cannot read the origin, we have to find it. It is (0, 0) during a fight
    but demonstrably not on every screen, and a wrong origin silently rips
    shifted garbage, so this runs before any rip.

    Method: reduce a screen row and every VRAM row to a one-byte-per-pixel
    green-channel signature, then let `bytes.find` locate the screen row inside
    the doubled VRAM row (doubling handles the `& 0x1FF` column wrap). Several
    probe rows must agree on the same origin.
    """
    vm = frame.vm
    sig_rows = []
    for r in range(frame.captured_rows):
        base = r * vm.row_words
        sig_rows.append(bytes(frame.color(frame.words[base + x])[1]
                              for x in range(vm.row_words)))
    votes = Counter()
    for frac in probes:
        sy = int(h * frac)
        if sy >= h:
            continue
        target = bytes(scr[(sy * w + x) * 4 + 1] for x in range(w))
        for r, sig in enumerate(sig_rows):
            p = (sig + sig).find(target)
            if 0 <= p < vm.row_words:
                votes[(r - sy, p)] += 1
                break
    if not votes:
        return None
    (row_off, col_off), n = votes.most_common(1)[0]
    if row_off < 0 or row_off + h > frame.captured_rows:
        return None
    return row_off, col_off, n, len(probes)


def score_alignment(frame, w, h, scr):
    """Fraction of sampled visible pixels where our decode == app://screen."""
    vm = frame.vm
    n = hit = 0
    for y in range(min(h, vm.rows)):
        for x in range(0, min(w, vm.visible_width), 3):
            r, g, b = frame.color(frame.word_at(x, y))
            o = (y * w + x) * 4
            n += 1
            if (scr[o], scr[o + 1], scr[o + 2]) == (r, g, b):
                hit += 1
    return (hit / n) if n else 0.0


# ---------------------------------------------------------------- bank cels
class Cel:
    """One palette bank's pixels in one frame, cropped to its bounding box."""

    __slots__ = ("bank", "x0", "y0", "x1", "y1", "px", "idx", "rgba",
                 "frame_count", "player", "mirrored", "seen")

    def __init__(self, bank, x0, y0, x1, y1, px, idx, rgba, frame_count):
        self.bank, self.px, self.idx, self.rgba = bank, px, idx, rgba
        self.x0, self.y0, self.x1, self.y1 = x0, y0, x1, y1
        self.frame_count = frame_count
        self.player = None
        self.mirrored = False
        self.seen = 1

    @property
    def w(self):
        return self.x1 - self.x0 + 1

    @property
    def h(self):
        return self.y1 - self.y0 + 1


def bank_census(frame, y0=0, y1=None):
    """{bank: [count, x0, y0, x1, y1]} over the VISIBLE columns only.

    Runs on the `hi` plane with bytes.count/find/rfind, which keeps a full
    254-row census at a few milliseconds — the difference between ripping
    hundreds of frames and giving up.
    """
    vm = frame.vm
    y1 = vm.rows - 1 if y1 is None else min(y1, vm.rows - 1)
    vw = vm.visible_width
    rows = [frame.hi_row(y) for y in range(y0, y1 + 1)]
    counts = Counter()
    for r in rows:
        counts.update(r)
    stats = {}
    for bank, n in counts.items():
        needle = bytes((bank,))
        bx0, bx1, by0, by1 = vw, -1, None, None
        for i, r in enumerate(rows):
            p = r.find(needle)
            if p < 0:
                continue
            q = r.rfind(needle)
            if p < bx0:
                bx0 = p
            if q > bx1:
                bx1 = q
            if by0 is None:
                by0 = y0 + i
            by1 = y0 + i
        stats[bank] = [n, bx0, by0, bx1, by1]
    return stats


def extract_cel(frame, bank, box):
    """Crop `bank`'s pixels to their bbox: index map (for hashing) + RGBA."""
    vm = frame.vm
    _, x0, y0, x1, y1 = box
    w, h = x1 - x0 + 1, y1 - y0 + 1
    idx = bytearray(w * h)
    rgba = bytearray(w * h * 4)
    px = 0
    for y in range(y0, y1 + 1):
        dst = (y - y0) * w
        for x in range(x0, x1 + 1):
            word = frame.word_at(x, y)
            if (word >> 8) != bank:
                continue
            r, g, b = frame.color(word)
            o = (dst + (x - x0)) * 4
            rgba[o:o + 4] = bytes((r, g, b, 255))
            idx[dst + (x - x0)] = (word & 0xFF) or 0xFF  # 0 would read as "empty"
            px += 1
    return Cel(bank, x0, y0, x1, y1, px, bytes(idx), bytes(rgba),
               frame.frame_count)


def looks_like_fighter(box, vm, min_px, max_px, max_w_frac=0.45):
    """A fighter is a compact tall-ish blob; scenery layers span the screen.

    Deliberately geometric, not a bank whitelist: bank numbers are just
    whatever the DMA palette register held, so they differ per character,
    per stage and per match.
    """
    count, x0, y0, x1, y1 = box
    w, h = x1 - x0 + 1, y1 - y0 + 1
    if not (min_px <= count <= max_px):
        return False
    if w > vm.visible_width * max_w_frac:
        return False
    if h < 24 or w < 8:
        return False
    return count >= w * h * 0.12       # drop sparse spatter (chains, debris)


def mirror_idx(idx, w, h):
    out = bytearray(len(idx))
    for y in range(h):
        row = idx[y * w:(y + 1) * w]
        out[y * w:(y + 1) * w] = row[::-1]
    return bytes(out)


def cel_key(cel, mirror=True):
    """(digest, stored_mirrored) — content hash of the index map.

    Hashing INDICES rather than RGBA means a hit-flash palette swap does not
    masquerade as a new pose. With `mirror`, a cel and its left-facing twin
    collapse to one entry (the lexicographically smaller map wins), which
    roughly halves a sheet.
    """
    a = cel.idx
    head = struct.pack("<HH", cel.w, cel.h)
    if not mirror:
        return hashlib.sha1(head + a).hexdigest()[:16], False
    b = mirror_idx(a, cel.w, cel.h)
    if b < a:
        return hashlib.sha1(head + b).hexdigest()[:16], True
    return hashlib.sha1(head + a).hexdigest()[:16], False


def flip_rgba(rgba, w, h):
    out = bytearray(len(rgba))
    for y in range(h):
        for x in range(w):
            s = (y * w + x) * 4
            d = (y * w + (w - 1 - x)) * 4
            out[d:d + 4] = rgba[s:s + 4]
    return bytes(out)


# ---------------------------------------------------------------- attribution
def read_players(mcp, prof, vis_w):
    """Per-player (char_id, health, screen_x or None) straight from the profile.

    Screen X for P2 is derived: the camera offset is p1_world_x - p1_screen_x,
    and mk2.md warns the world-X slots can read stale, so every value here is
    best-effort and attribution falls back to left/right ordering.
    """
    def u(addr, size):
        try:
            return int.from_bytes(mcp.read_memory(addr, size), "little")
        except Exception:
            return None

    out = {}
    coff = prof.field_off("char_id")
    hoff = prof.field_off("health")
    for pi, blk in ((1, prof.block1()), (2, prof.block2())):
        cid = u(blk + coff[0], coff[1]) if coff else None
        out[pi] = {"char_id": prof.canon_char_id(cid) if cid is not None else None,
                   "health": u(blk + hoff[0], hoff[1]) if hoff else None,
                   "screen_x": None}
    p1sx = u(prof.global_addr("p1_screen_x") or 0, 2)
    p1wx = u(prof.global_addr("p1_x") or 0, 2)
    p2wx = u(prof.global_addr("p2_x") or 0, 2)
    if p1sx is not None and 0 <= p1sx < vis_w:
        out[1]["screen_x"] = p1sx
        if None not in (p1wx, p2wx) and p1wx:
            cam = p1wx - p1sx
            p2sx = p2wx - cam
            if 0 <= p2sx < vis_w:
                out[2]["screen_x"] = p2sx
    return out


class Attributor:
    """Sticky bank -> player mapping.

    Per-frame geometry alone is fragile: `p1_screen_x` goes stale (mk2.md), and
    left/right ordering inverts the moment someone jumps over their opponent.
    But a fighter keeps its DMA palette bank for the whole round, so we decide
    ONCE per bank set — preferring a sane screen_x, else left/right — and hold
    that mapping until the cast changes (new banks, or new char ids).
    """

    def __init__(self):
        self.bank_player = {}
        self.signature = None

    def __call__(self, cels, players):
        sig = (tuple(sorted(c.bank for c in cels)),
               players[1]["char_id"], players[2]["char_id"])
        if sig != self.signature:
            self.signature = sig
            self.bank_player = self._decide(cels, players)
        for cel in cels:
            cel.player = self.bank_player.get(cel.bank)
        return cels

    @staticmethod
    def _decide(cels, players):
        known = {p: d["screen_x"] for p, d in players.items()
                 if d["screen_x"] is not None}
        mapping = {}
        if known:
            for cel in cels:
                # 24 px slack: screen_x tracks the sprite origin, not the bbox
                best = min(known.items(), key=lambda kv: abs(cel.x0 - kv[1]))
                if abs(cel.x0 - best[1]) <= 24:
                    mapping[cel.bank] = best[0]
        if len(mapping) != len(cels) and len(cels) == 2:
            mapping = {c.bank: p for p, c in
                       zip((1, 2), sorted(cels, key=lambda c: c.x0))}
        return mapping


# ---------------------------------------------------------------- sheet out
def write_png_rgba(path, width, height, rgba):
    raw = b"".join(b"\x00" + bytes(rgba[y * width * 4:(y + 1) * width * 4])
                   for y in range(height))

    def chunk(typ, data):
        c = typ + data
        return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c))

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(raw, 9)))
        f.write(chunk(b"IEND", b""))


def pack_sheet(cels, cols, gutter=1):
    """Uniform grid, each cel BOTTOM-CENTER anchored in its cell.

    Bottom-center keeps the fighter's feet on a common baseline, so flipping
    through the sheet reads as animation instead of jitter. Exact per-cel
    rects go in the JSON index for anything that needs true placement.
    """
    if not cels:
        return 0, 0, b"", []
    cw = max(c.w for c in cels)
    ch = max(c.h for c in cels)
    rows = (len(cels) + cols - 1) // cols
    W = cols * cw + (cols + 1) * gutter
    H = rows * ch + (rows + 1) * gutter
    sheet = bytearray(W * H * 4)
    places = []
    for i, cel in enumerate(cels):
        col, row = i % cols, i // cols
        ox = gutter + col * (cw + gutter) + (cw - cel.w) // 2
        oy = gutter + row * (ch + gutter) + (ch - cel.h)
        for y in range(cel.h):
            s = y * cel.w * 4
            d = ((oy + y) * W + ox) * 4
            sheet[d:d + cel.w * 4] = cel.rgba[s:s + cel.w * 4]
        places.append((ox, oy))
    return W, H, bytes(sheet), places


def write_sheet(out_dir, name, cels, meta, cols, gutter, keep_cels=False):
    os.makedirs(out_dir, exist_ok=True)
    cels = sorted(cels, key=lambda c: (c.frame_count, c.x0))
    W, H, rgba, places = pack_sheet(cels, cols, gutter)
    png_path = os.path.join(out_dir, f"{name}.png")
    write_png_rgba(png_path, W, H, rgba)
    index = dict(meta)
    index["sheet"] = {"file": f"{name}.png", "width": W, "height": H,
                      "cols": cols, "gutter": gutter,
                      "cell_w": max(c.w for c in cels), "cell_h": max(c.h for c in cels),
                      "anchor": "bottom-center", "count": len(cels)}
    index["cels"] = [
        {"i": i, "sheet_x": places[i][0], "sheet_y": places[i][1],
         "w": c.w, "h": c.h, "px": c.px, "bank": f"0x{c.bank:02X}",
         "screen_bbox": [c.x0, c.y0, c.x1, c.y1],
         "first_frame": c.frame_count, "seen": c.seen,
         "player": c.player, "mirrored": c.mirrored}
        for i, c in enumerate(cels)]
    with open(os.path.join(out_dir, f"{name}.json"), "w") as f:
        json.dump(index, f, indent=1)
    if keep_cels:
        cel_dir = os.path.join(out_dir, name)
        os.makedirs(cel_dir, exist_ok=True)
        for i, c in enumerate(cels):
            write_png_rgba(os.path.join(cel_dir, f"{i:04d}.png"), c.w, c.h, c.rgba)
    return png_path, W, H


# ---------------------------------------------------------------- verify
def lock_alignment(mcp, region, vm, tmp_dir, full_range=False, tries=3):
    """Find and score the display origin. Returns (align, score, frame).

    MUST be measured paused — on a running game the chunked VRAM read and the
    screen grab are simply different instants, which scores near zero and means
    nothing. Restores the emulator's prior pause state.

    Retries by stepping a frame: straight after a `load_state`, or mid screen
    transition, the framebuffer and VRAM can disagree enough that no probe row
    matches. One step re-syncs them.
    """
    was_paused = bool(mcp.call("get_state").get("paused"))
    if not was_paused:
        mcp.pause()
    try:
        for attempt in range(tries):
            frame = capture(mcp, region, vm, full_range=full_range)
            w, h, rgba = mcp.screen_rgba(tmp_dir)
            found = find_alignment(frame, w, h, rgba)
            if found is not None:
                frame.align = found[:2]
                return found[:2], score_alignment(frame, w, h, rgba), frame
            if attempt + 1 < tries:
                mcp.call("step")          # the MCP step tool takes no args:
                                          # exactly one frame per call
    finally:
        if not was_paused:
            mcp.resume()
    return None, 0.0, frame


# ---------------------------------------------------------------- subcommands
def cmd_banks(mcp, prof, vm, args):
    region = mcp.region_name()
    align, match, frame = lock_alignment(mcp, region, vm, args.out)
    print(f"frame {frame.frame_count}  display origin {align}  "
          f"VRAM/screen agreement: {match * 100:.1f}%")
    if align is None or match < 0.95:
        print("  WARNING: could not lock the display origin — the census below "
              "may be reading a shifted or stale buffer.")
    players = read_players(mcp, prof, vm.visible_width)
    for p, d in players.items():
        nm = prof.char_name(d["char_id"]) if d["char_id"] is not None else "?"
        print(f"  P{p}: {nm} (id {d['char_id']}) health={d['health']} "
              f"screen_x={d['screen_x']}")
    stats = bank_census(frame)
    print(f"\n{'bank':>5} {'px':>7}  {'bbox':>21}  {'w×h':>9}  fighter?")
    for bank, box in sorted(stats.items(), key=lambda kv: -kv[1][0])[:args.top]:
        count, x0, y0, x1, y1 = box
        w, h = x1 - x0 + 1, y1 - y0 + 1
        fit = looks_like_fighter(box, vm, args.min_px, args.max_px)
        print(f" 0x{bank:02X} {count:7d}  ({x0:3d},{y0:3d},{x1:3d},{y1:3d})  "
              f"{w:3d}×{h:<3d}  {'YES' if fit else ''}")
        if args.write and count >= args.min_px:
            cel = extract_cel(frame, bank, box)
            write_png_rgba(os.path.join(args.out, f"bank_{bank:02X}.png"),
                           cel.w, cel.h, cel.rgba)
    if args.write:
        print(f"\nwrote per-bank crops to {args.out}/bank_*.png")


def cmd_rip(mcp, prof, vm, args):
    region = mcp.region_name()
    # Both stepping AND the alignment lock pause the emulator, so either one
    # needs consent on the human's session.
    if mcp.port == USER_SESSION_PORT and not args.force:
        if args.frame_exact:
            raise SystemExit(
                f"refusing to pause/step port {USER_SESSION_PORT} (the human's "
                "session). Use --watch to sample passively, or --force.")
        if args.verify:
            raise SystemExit(
                f"the alignment lock briefly pauses port {USER_SESSION_PORT} "
                "(the human's session). Pass --no-verify to sample without "
                "pausing (assumes display origin 0,0 — check it with `banks` "
                "on a scratch instance first), or --force.")

    # Alignment gate — rip nothing rather than rip shifted garbage.
    align = (0, 0)
    if args.verify:
        align, match, _ = lock_alignment(mcp, region, vm, args.out,
                                         args.full_range)
        if align is None or match < 0.95:
            raise SystemExit(
                f"could not lock the display origin (best agreement "
                f"{match * 100:.1f}%) — refusing to rip. Check the profile's "
                "video offsets, or pass --no-verify to assume (0, 0).")
        print(f"display origin {align}, agreement {match * 100:.1f}% — ripping "
              f"{args.samples} samples "
              f"({'step ' + str(args.step) if args.frame_exact else 'watch'})")

    min_seen = args.min_seen if args.min_seen is not None else (
        1 if args.frame_exact else 2)
    by_char = {}          # char name -> {hash: Cel}
    pal_cache = None
    seen_frames = []
    attributor = Attributor()
    if args.frame_exact:
        mcp.pause()
    try:
        for s in range(args.samples):
            if args.frame_exact:
                for _ in range(args.step):   # one frame per `step` call
                    mcp.call("step")
            elif s:
                time.sleep(args.interval)
            if args.pal_every and s % args.pal_every == 0:
                pal_cache = None
            if args.realign_every and s and s % args.realign_every == 0:
                new_align, match, _ = lock_alignment(mcp, region, vm, args.out,
                                                     args.full_range)
                if new_align is not None and match >= 0.95:
                    if new_align != align:
                        print(f"  display origin moved {align} -> {new_align}")
                    align = new_align
            frame = capture(mcp, region, vm, pal_cache=pal_cache,
                            coherent=args.frame_exact,
                            full_range=args.full_range, align=align)
            pal_cache = frame.pal
            players = read_players(mcp, prof, vm.visible_width)
            stats = bank_census(frame, y0=args.y0, y1=args.y1)
            cels = [extract_cel(frame, b, box) for b, box in stats.items()
                    if looks_like_fighter(box, vm, args.min_px, args.max_px)]
            attributor(cels, players)
            seen_frames.append(frame.frame_count)
            for cel in cels:
                pid = cel.player
                cid = players.get(pid, {}).get("char_id") if pid else None
                name = prof.char_name(cid) if cid is not None else "unattributed"
                digest, mirrored = cel_key(cel, mirror=not args.no_mirror)
                if mirrored:
                    cel.rgba = flip_rgba(cel.rgba, cel.w, cel.h)
                    cel.mirrored = True
                bucket = by_char.setdefault(name, {})
                if digest in bucket:
                    bucket[digest].seen += 1
                else:
                    bucket[digest] = cel
            if args.verbose or s % 20 == 0:
                tot = sum(len(v) for v in by_char.values())
                print(f"  sample {s + 1}/{args.samples} frame {frame.frame_count}: "
                      f"{len(cels)} cels, {tot} unique so far")
    finally:
        if args.frame_exact and not args.stay_paused:
            mcp.resume()

    if not by_char:
        raise SystemExit("no fighter cels found — is a fight actually on screen? "
                         "Run the `banks` subcommand to inspect.")
    print()
    for name, bucket in sorted(by_char.items()):
        kept = [c for c in bucket.values() if c.seen >= min_seen]
        dropped = len(bucket) - len(kept)
        if not kept:
            print(f"{name:>14}: nothing survived --min-seen {min_seen} "
                  f"({dropped} one-off cels dropped)")
            continue
        meta = {"game": prof.family, "port": prof.port, "character": name,
                "source": {"mcp_port": mcp.port,
                           "mode": "step" if args.frame_exact else "watch",
                           "coherent": bool(args.frame_exact),
                           "samples": args.samples,
                           "step": args.step if args.frame_exact else None,
                           "frame_first": seen_frames[0] if seen_frames else None,
                           "frame_last": seen_frames[-1] if seen_frames else None},
                "video": vm.as_json(), "display_origin": list(align),
                "mirror_dedupe": not args.no_mirror,
                "min_seen": min_seen, "dropped_below_min_seen": dropped}
        path, W, H = write_sheet(args.out, name, kept, meta,
                                 args.cols, args.gutter, args.keep_cels)
        note = f" ({dropped} dropped below --min-seen {min_seen})" if dropped else ""
        print(f"{name:>14}: {len(kept):3d} unique cels -> {path} ({W}×{H}){note}")


# ---------------------------------------------------------------- selftest
def cmd_selftest(args):
    """Pure-function checks — no emulator, no MCP. Exercised in CI-by-hand."""
    ok = 0

    # xRGB1555 decode: emulator-exact (v<<3, so 31 -> 248) vs full-range.
    pal = rgb555_table(bytes([0xFF, 0x7F, 0x00, 0x7C, 0x00, 0x00]))
    assert pal[0] == (248, 248, 248), pal[0]
    assert pal[1] == (248, 0, 0), pal[1]
    assert pal[2] == (0, 0, 0), pal[2]
    wide = rgb555_table(bytes([0xFF, 0x7F]), full_range=True)
    assert wide[0] == (255, 255, 255), wide[0]
    ok += 1

    # A synthetic 4×4 VRAM: bank 0x02 draws an L in the bottom-left 2×2.
    class VM:
        row_words, rows, visible_width, index_mask = 4, 4, 4, 0x7FFF
    vm = VM()
    words = [0] * 16
    for x, y in ((0, 2), (0, 3), (1, 3)):
        words[y * 4 + x] = (0x02 << 8) | 0x05
    f = Frame(struct.pack("<16H", *words), [(0, 0, 0)] * 0x8000, vm, 7, True)
    f.pal[0x0205] = (10, 20, 30)
    stats = bank_census(f)
    assert stats[0x02] == [3, 0, 2, 1, 3], stats[0x02]
    cel = extract_cel(f, 0x02, stats[0x02])
    assert (cel.w, cel.h, cel.px) == (2, 2, 3), (cel.w, cel.h, cel.px)
    assert cel.rgba[0:4] == bytes((10, 20, 30, 255))     # (0,2) opaque
    assert cel.rgba[4:8] == bytes((0, 0, 0, 0))          # (1,2) transparent
    ok += 1

    # Mirror dedupe: a cel and its mirror hash identically, exactly one flagged.
    m = Cel(2, 0, 0, 1, 1, 3, mirror_idx(cel.idx, 2, 2),
            flip_rgba(cel.rgba, 2, 2), 7)
    k1, f1 = cel_key(cel)
    k2, f2 = cel_key(m)
    assert k1 == k2, (k1, k2)
    assert f1 != f2, "exactly one of the pair should be stored mirrored"
    assert cel_key(cel, mirror=False)[0] != cel_key(m, mirror=False)[0]
    ok += 1

    # Packing geometry: bottom-center anchoring inside a uniform grid.
    big = Cel(2, 0, 0, 3, 5, 1, b"\0" * 24, b"\0" * 96, 1)
    W, H, sheet, places = pack_sheet([cel, big], cols=2, gutter=1)
    assert (W, H) == (2 * 4 + 3, 1 * 6 + 2), (W, H)
    assert places[0] == (1 + (4 - 2) // 2, 1 + (6 - 2)), places[0]
    assert places[1] == (1 + 4 + 1, 1), places[1]
    assert len(sheet) == W * H * 4
    ok += 1

    # PNG round-trip through the repo's own reader.
    tmp = os.path.join(args.out, ".selftest.png")
    os.makedirs(args.out, exist_ok=True)
    write_png_rgba(tmp, 2, 2, cel.rgba)
    w, h, rgba = screen_tools.load_rgba(tmp)
    os.unlink(tmp)
    assert (w, h) == (2, 2) and bytes(rgba) == cel.rgba
    ok += 1

    # The fighter heuristic: accept a fighter-shaped blob, reject a backdrop.
    class VM2:
        visible_width = 400
    assert looks_like_fighter([3000, 160, 110, 250, 225], VM2(), 500, 20000)
    assert not looks_like_fighter([69000, 0, 0, 399, 250], VM2(), 500, 20000)
    assert not looks_like_fighter([600, 0, 0, 300, 200], VM2(), 500, 20000)
    ok += 1

    # Attribution sticks to the bank, so a jump-over does NOT swap players.
    def mkcel(bank, x0):
        return Cel(bank, x0, 100, x0 + 40, 220, 3000, b"", b"", 1)
    players = {1: {"char_id": 9, "screen_x": None},
               2: {"char_id": 10, "screen_x": None}}
    att = Attributor()
    left, right = mkcel(0x02, 80), mkcel(0x03, 260)
    att([left, right], players)
    assert (left.player, right.player) == (1, 2), (left.player, right.player)
    crossed = [mkcel(0x02, 300), mkcel(0x03, 60)]      # they swapped sides
    att(crossed, players)
    assert (crossed[0].player, crossed[1].player) == (1, 2), "bank mapping held"
    players[2]["char_id"] = 4                            # new round, new cast
    fresh = [mkcel(0x02, 300), mkcel(0x03, 60)]
    att(fresh, players)
    assert (fresh[0].player, fresh[1].player) == (2, 1), "re-attributed"
    # A sane screen_x wins over left/right ordering.
    att2 = Attributor()
    players2 = {1: {"char_id": 9, "screen_x": 262}, 2: {"char_id": 10, "screen_x": 78}}
    a, b = mkcel(0x02, 80), mkcel(0x03, 260)
    att2([a, b], players2)
    assert (a.player, b.player) == (2, 1), (a.player, b.player)
    ok += 1

    # Alignment recovery: plant a known display origin and find it again.
    class VM3:
        row_words, rows, visible_width, index_mask = 16, 4, 8, 0x7FFF
    vm3 = VM3()
    vwords = [r * 16 + x + 1 for r in range(8) for x in range(16)]
    pal3 = [(0, i % 256, 0) for i in range(0x8000)]
    f3 = Frame(struct.pack(f"<{len(vwords)}H", *vwords), pal3, vm3, 1, True)
    assert f3.captured_rows == 8
    ROW_OFF, COL_OFF = 2, 5                     # COL_OFF+8 > 16? no — but it wraps below
    scr = bytearray(8 * 4 * 4)
    for y in range(4):
        for x in range(8):
            wd = vwords[(y + ROW_OFF) * 16 + ((COL_OFF + x) & 15)]
            r, g, b = pal3[wd]
            scr[(y * 8 + x) * 4:(y * 8 + x) * 4 + 4] = bytes((r, g, b, 255))
    got = find_alignment(f3, 8, 4, scr)
    assert got is not None and got[:2] == (ROW_OFF, COL_OFF), got
    f3.align = got[:2]
    assert score_alignment(f3, 8, 4, scr) == 1.0
    # And a column origin that WRAPS past the end of the row is still found.
    COL_OFF = 12
    for y in range(4):
        for x in range(8):
            wd = vwords[(y + ROW_OFF) * 16 + ((COL_OFF + x) & 15)]
            r, g, b = pal3[wd]
            scr[(y * 8 + x) * 4:(y * 8 + x) * 4 + 4] = bytes((r, g, b, 255))
    got = find_alignment(f3, 8, 4, scr)
    assert got is not None and got[:2] == (ROW_OFF, COL_OFF), got
    f3.align = got[:2]
    assert score_alignment(f3, 8, 4, scr) == 1.0
    assert len(f3.hi_row(0)) == 8, "wrapped hi_row keeps the visible width"
    ok += 1

    print(f"selftest: {ok}/8 groups passed")


# ---------------------------------------------------------------- main
def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("cmd", choices=["banks", "rip", "selftest"])
    ap.add_argument("--port", type=int, default=4026,
                    help="MCP port (default 4026; 4025 is the human's session)")
    ap.add_argument("--game", default="library/mk2",
                    help="game profile dir (default library/mk2)")
    ap.add_argument("--out", default="library/mk2/assets",
                    help="output dir for <character>.png/.json "
                         "(default library/mk2/assets)")
    ap.add_argument("--samples", type=int, default=120,
                    help="how many frames to sample (default 120)")
    ap.add_argument("--step", type=int, default=2,
                    help="emulator frames per sample in frame-exact mode")
    ap.add_argument("--watch", dest="frame_exact", action="store_false",
                    help="sample the RUNNING game instead of pausing/stepping")
    ap.add_argument("--interval", type=float, default=0.2,
                    help="seconds between samples in --watch mode")
    ap.add_argument("--force", action="store_true",
                    help="allow pausing/stepping port 4025")
    ap.add_argument("--stay-paused", action="store_true",
                    help="leave the emulator paused when done")
    ap.add_argument("--min-px", type=int, default=500,
                    help="smallest bank pixel count treated as a fighter")
    ap.add_argument("--max-px", type=int, default=20000,
                    help="largest bank pixel count treated as a fighter")
    ap.add_argument("--y0", type=int, default=0,
                    help="first VRAM row to scan (raise it to skip the HUD)")
    ap.add_argument("--y1", type=int, default=None,
                    help="last VRAM row to scan (default: the last visible)")
    ap.add_argument("--cols", type=int, default=8,
                    help="cels per sheet row (default 8)")
    ap.add_argument("--gutter", type=int, default=1,
                    help="transparent pixels between sheet cells (default 1)")
    ap.add_argument("--pal-every", type=int, default=1,
                    help="re-read palette RAM every N samples (0 = once)")
    ap.add_argument("--no-mirror", action="store_true",
                    help="keep left- and right-facing cels as separate entries")
    ap.add_argument("--min-seen", type=int, default=None,
                    help="drop cels seen fewer than N times (default 1 when "
                         "stepping, 2 in --watch mode, where a torn read can "
                         "invent a one-off pose)")
    ap.add_argument("--no-verify", dest="verify", action="store_false",
                    help="skip the paused VRAM-vs-screen alignment check")
    ap.add_argument("--realign-every", type=int, default=25,
                    help="re-lock the display origin every N samples "
                         "(0 = lock once)")
    ap.add_argument("--full-range", action="store_true",
                    help="bit-replicate 5-bit channels (31 -> 255) instead of "
                         "the emulator-exact v<<3 (31 -> 248)")
    ap.add_argument("--keep-cels", action="store_true",
                    help="also write each individual cel PNG next to the sheet")
    ap.add_argument("--top", type=int, default=14, help="banks to list (banks cmd)")
    ap.add_argument("--write", action="store_true",
                    help="banks cmd: also write per-bank crop PNGs")
    ap.add_argument("--verbose", action="store_true",
                    help="log every sample instead of every 20th")
    args = ap.parse_args()

    os.makedirs(args.out, exist_ok=True)
    if args.cmd == "selftest":
        return cmd_selftest(args)

    prof = profile_mod.load(args.game)
    vm = VideoMap.from_profile(prof)
    mcp = Mcp(args.port)
    if args.cmd == "banks":
        return cmd_banks(mcp, prof, vm, args)
    return cmd_rip(mcp, prof, vm, args)


if __name__ == "__main__":
    main()
