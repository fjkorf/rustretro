---
schema_version: 1

rom:
  name: "Asura Blade - Sword of Dynasty"
  system: fuuki-fg3
  sha1: "e86d38e1cff3e5358fb2f8df97391475beaedd2b"   # of the asurabld.zip romset container
  crc32: "70535b10"                                   # of the asurabld.zip romset container
  size: 17959564

settings: {}

meta:
  genre: fighting
  year: 1998
  developer: "Fuuki"
  progress: "full graphics pipeline mapped: tilemaps, sprites, palettes reconstructed and screen-verified"
  tags: [arcade, 2d-fighter, fuuki-fg3, m68020]
---

## Overview

Asura Blade runs on Fuuki FG-3 hardware: a 68EC020 driving four tilemap layers
and a multi-cel zooming sprite engine, with a Z80 + YMF278B for sound. In
RustRetro it runs under **fbalpha2012** (MAME 2003-Plus loads the set but
renders a permanently black frame). The core publishes no libretro memory map;
everything below is reached through the **Sek snapshot bridge** — bus windows
declared in `asurabld.busmap.json` and served by the core's exported
`SekReadByte/Word/Long`.

All decode facts were taken from the MAME driver source
(`drivers/fuukifg3.c`, `vidhrdw/fuukifg3_vidhrdw.c`, mame2003-plus tree; FBA's
`d_fuukifg3.cpp` for the banking/scroll differences) and then **verified
against the live screen**: `scripts/re/asura_assets.py verify` recomposites the
visible frame from these regions and matched at a mean channel delta of
~21/255 during a held fight (residue: per-scanline floor raster, shadow-sprite
blending, 2-frame sprite buffering).

## Memory map (68020 bus)

Windows snapshotted by the bridge. A crucial FBA quirk: the video registers,
layer priority, and sprite tilebank at 0x8C0000/0x8E0000/0xA00000 are
**write-only through this core's bus handlers** — reads return zeros, unlike
MAME which maps them as readable RAM. Their live values must be inferred from
the framebuffer (see Method).

::: region kind=ram id=wram addr=0x400000-0x40FFFF label="Work RAM" confidence=confirmed
64 KiB main work RAM. Untouched by the asset pipeline so far; the natural next
target for gameplay RE (health, positions, action state) with the standard
value-search loop, which the bridge now makes possible on this core.
:::

::: region kind=tilemap id=tm0 addr=0x500000-0x501FFF label="Tilemap layer 0" format=cell_u32_code16_attr16 confidence=confirmed
64x32 cells of 16x16 8bpp tiles (layer pixel size 1024x512). Cell = code<<16 |
attr; palette bank = (attr&0x3F)>>4, flips = attr bits 6 (X) and 7 (Y). Tile
pixels come from [[gfx-bg1]]; pens at [[pal]] base 0x000 + bank*256; pen 0xFF
transparent. In fights this held the distant vista (mountains/sky); on the
title screen, the floating fortress.
:::

::: region kind=tilemap id=tm1 addr=0x502000-0x503FFF label="Tilemap layer 1" format=cell_u32_code16_attr16 confidence=confirmed
Same cell format as [[tm0]]; tiles from [[gfx-bg2]], pens at base 0x400. Held
the main stage art (temple courtyard, floor) during fights. The floor's
perspective sway is a per-scanline scrollx raster effect (FBA buffers scroll
per line), so a single-scroll recomposite shows mild floor smear.
:::

::: region kind=tilemap id=tmbg addr=0x504000-0x505FFF label="Tilemap 8x8 page A" format=cell_u32_code16_attr16 confidence=confirmed
64x32 cells of 8x8 4bpp tiles (512x256). Palette bank = attr&0x3F, pens at
0xC00 + bank*16, pen 0xF transparent; tiles from [[gfx-map]]. Pages A/B are
double-buffered alternates of ONE layer — vregs bit 0x40 (write-only) selects
the displayed page. Carries HUD text and dialogue.
:::

::: region kind=tilemap id=tmbg2 addr=0x506000-0x507FFF label="Tilemap 8x8 page B" format=cell_u32_code16_attr16 confidence=confirmed
The other page of [[tmbg]].
:::

::: region kind=sprite_table id=spr addr=0x600000-0x601FFF label="Sprite RAM" format=entry_8b_sx_sy_attr_code confidence=confirmed
1024 entries x 8 bytes: sx.w, sy.w, attr.w, code.w. Skip when sx&0x400. Multi-
cel sprites: xnum=((sx>>12)&0xF)+1, ynum likewise from sy; cel k sits at (k%
xnum, k//xnum) and cels use code, code+1, …. Position = 10-bit signed (v&0x1FF
- v&0x200). Flips: sx/sy bit 0x800. attr: pal = attr&0x3F (pens 0x800+pal*16,
pen 15 transparent), priority-vs-layers bits 6-7, yzoom bits 8-11, xzoom bits
12-15 (zoom = shrink from 16px). code: bits 14-15 select a tilebank nibble
(see [[tilebank]]); effective code = (code&0x3FFF) | nibble<<14, tile bytes at
[[gfx-sp]] offset code*128. Hardware displays a copy buffered ~2 frames, so a
live snapshot leads the visible frame slightly.

Pose-level structure (verified live): the list is slot-allocated — HUD at
~70-110 and ~700+, the zoomed shadow blobs at ~280, characters and their
effects at ~300-699, and banner machinery PARKED OFF-SCREEN at ~1000+ (not
skip-flagged). A character pose is a contiguous index run whose strip boxes
tile together; palette varies per STRIP within one pose (weapon/effect strips
interleave the body run under a different palette); draw order is descending
index, back to front. Within one run, codes advance by strip width — a pose is
one or more contiguous code intervals in [[gfx-sp]], and animation frames sit
at evenly spaced intervals (e.g. Footee's idle at stride 0x19). Plain pose
tables were NOT found in program ROM by direct/LE/shifted searches; the
metadata is packed or computed (write-trace or disassembly needed to go
ROM-side). `asura_assets.py poses --slots 300-699` harvests deduplicated,
flip-normalized pose assets (see assets/poses/).
:::

::: region kind=palette id=pal addr=0x700000-0x703FFF label="Palette RAM" format=xrgb555_be confidence=confirmed
8192 colors, one xRGB555 in each big-endian 16-bit word. Pen allocation:
0x000-0x3FF layer 0 (4 banks x 256), 0x400-0x7FF layer 1, 0x800-0xBFF sprites
(64 banks x 16), 0xC00-0xFFF 8x8 layer (64 banks x 16). Rendered live by
`asura_assets.py palettes` (capture: assets/palette_ram.png).
:::

::: region kind=io id=vregs addr=0x8C0000-0x8C001F label="Video registers" confidence=confirmed
Scroll/raster registers: 00.w L0 scrollY, 02.w L0 scrollX, 04.w/06.w L1,
08.w/0A.w 8x8 layer, 0C.w/0E.w global Y/X offsets (minus 0x1F3/0x3F6), 1C.w
raster IRQ line, 1E.w flip + page-select bit 0x40. WRITE-ONLY through FBA's
handlers — the bridge window reads zeros; infer scroll from the screen.
:::

::: region kind=io id=priority addr=0x8E0000-0x8E0003 label="Layer priority" confidence=confirmed
(value>>16)&0xF indexes a 6-entry layer-order table (fights use order
front=L1, mid=L0, back=8x8 — index 2 "Most Levels"). Write-only on this core;
infer by trying all six orders against the screen.
:::

::: region kind=io id=tilebank addr=0xA00000-0xA00003 label="Sprite tilebank" confidence=confirmed
Four nibbles remapping the sprite code's top 2 bits: nibble = (value >>
(bank*4)) & 0xF (FBA uses the LOW word of the 32-bit write). Write-only on
this core — `asura_assets.py` calibrates it per frame by pixel-matching
candidate decodes against the framebuffer at each sprite's position.
:::

## Graphics ROM (file-resident; the core never exposes these)

Common pixel layout for all three sets: within each 32-bit group the pixel
bytes are ordered [b1, b0, b3, b2], high nibble = left pixel (from the MAME
GfxLayouts; implemented in `scripts/re/decode_asurabld.py`).

::: region kind=sprite_sheet id=gfx-sp addr=0x000000-0x1FFFFFF label="Sprite tiles (sp23-spcd.u14-u19)" space=rom format=4bpp_16x16_swizzled confidence=confirmed
16x16 4bpp, 128 bytes/tile, region offset = code*128. The first 4 MiB slot is
EMPTY on asurabld (sp01.u13 exists only on Asura Buster), so codes below
0x8000 are blank; sp23.u14 starts at code 0x8000, each file spans 0x8000
codes. Blank areas are 0xFF-filled (pen 15 = transparent). ~196k tiles, ~50%
art.
:::

::: region kind=background id=gfx-bg1 addr=0x000000-0x7FFFFF label="Layer-0 tiles (bg1012.u22 + bg1113.u23)" space=rom format=8bpp_16x16_two_roms confidence=confirmed
16x16 8bpp built from two 4bpp files: pen = (bg1113 nibble)<<4 | (bg1012
nibble); tile t at offset t*128 in EACH file. 32768 tiles.
:::

::: region kind=background id=gfx-bg2 addr=0x000000-0x7FFFFF label="Layer-1 tiles (bg2022.u25 + bg2123.u24)" space=rom format=8bpp_16x16_two_roms confidence=confirmed
Same scheme as [[gfx-bg1]] for the other 16x16 layer.
:::

::: region kind=background id=gfx-map addr=0x000000-0x1FFFFF label="8x8 tiles (map.u5)" space=rom format=4bpp_8x8_swizzled confidence=confirmed
8x8 4bpp, 32 bytes/tile, 65536 codes; ~91% blank. Holds far-background scenery
strips (sky/clouds/mountains/sea) plus HUD/dialogue glyphs.
:::

## Method

The working loop, end to end (all tools in-repo):

1. Launch headless with the busmap: `rustretro --core fbalpha2012 --rom
   asurabld.zip --headless --mcp-port 4011 --bus-map
   library/asurabld/asurabld.busmap.json`.
2. Drive to the scene you want with `press_buttons` (coin = select).
3. `scripts/re/asura_assets.py bg|sprites|palettes` — pauses the emulator for
   a same-frame snapshot, reads the regions above over MCP, decodes tile
   pixels from the ROM files, and writes true-palette RGBA assets to
   `assets/`. Write-only registers are inferred from the framebuffer
   (tilebank: per-bank pixel-match calibration; scroll: coarse-to-fine
   correlation; priority: best of six orders).
4. `asura_assets.py verify` — recomposites the visible frame from the data
   above and pixel-diffs against `app://screen`. A held fight scored
   ~21/255 mean channel delta at 100% coverage; the gap is explained by
   per-scanline floor raster, shadow-sprite blending, palette cycling, and
   the 2-frame sprite buffer.

Captured assets so far: `assets/bg_layer0.png`, `assets/bg_layer1.png`
(complete stages, wider than the camera ever shows), `assets/bg_8x8_page0/1.png`,
`assets/palette_ram.png`, `assets/verify_side_by_side.png`, `assets/sprites/`
(578 per-entry strips), and `assets/poses/` (62 deduplicated assembled poses —
stances, specials with effects, throws, splash text — harvested by
`asura_assets.py poses --watch 35 --slots 300-699` over one driven match).

## Execution architecture

Mapped live (2026-08-17) by stack-walking: the frontend samples PC/registers
at frame end, which always lands in the level-1 IRQ handler — but the
**interrupted PC sits in the exception frame on the stack** (SR.w @ A7, PC.l @
A7+2), and jsr return addresses below it give a heuristic backtrace
(`scripts/re/execmap.py bt`). Per-frame flag watching and frame-stepping
through a start press confirmed every hop below.

The program is NOT a state-machine dispatcher: it is a **top-level script of
blocking scene routines**. Each scene runs its own per-frame loop (wait for an
IRQ flag bit, do frame work, check hop flags); scenes end by returning to the
script, which runs a fade/cleanup helper and calls the next scene.

::: region kind=interrupt_handler id=irq-vbl addr=0x02010E-0x02011E vector=25 confidence=confirmed
Level-1 (vblank) autovector handler: sets bit 5 of the IRQ flag byte
[[irq-flags-ram]] ($4064F2) and rte. Frame-end PC always samples here.
Sibling handlers: $020120 sets bit 7 and acks the raster line via $8C0012
(level 5, per the driver's vregs 0x1C); $020140 sets bit 2 (+ extra work via
$406558); $020160-$2059A sets bit 1 (large body, saves d0-d5/a0-a1).
:::

::: region kind=subroutine id=wait-vblank addr=0x0237B4-0x0237C8 label="wait_vblank" confidence=confirmed
The sync API: `btst #5,$4064F2; beq self; bclr #5; rts`. Variants: $0237CA
(saves regs, also clears the frame bookkeeping words $403678/$40636C on the
way out), $023C4E (waits bit 2), $023C64 (waits bit 1), $02379E-ish (bit 7).
~70 jsr callers of $0237B4 and ~34 of $0237CA mark every per-frame loop in
the game. Menus/attract sync on bit 5 (and bit 2 for transitions); the fight
engine uses the $0237CA/$0237D6 clearing variant.
:::

::: region kind=game_loop id=master-script addr=0x0207B0-0x021070 label="master game-flow script" confidence=confirmed
The top-level flow, written as straight-line code with blocking scene calls
and hop-flag checks between them. Observed anchors (return addresses):
$0207E2 attract/title block; $020892 = `jsr $6FBF6` (char select + loading);
$0208FA = `jsr $8F6A` (pre-fight dialogue; runs sub-loop at ~$30042);
$020920 = `jsr $64DEE` (round intro + fight loop A); $020930 = `jsr $2EA3C`
(fight loop B); $020940 = `jsr $3B824` (post-round). After each fight call:
`tst $403678 → bne $20FEC` (game-over path); after post-round:
`tst $402A32 → bne $2106E` (match-end path).
:::

::: region kind=game_loop id=loop-attract addr=0x06B6BC-0x06C288 label="attract/title scenes" confidence=confirmed
Blocking attract-family scenes called from [[master-script]]. $6B6BC entry:
writes tilebank #$7AAA, clears the advance latch $4065D8, checks credits
($40655C/E). The title choreography at $6C070+ is the canonical scene shape:
timed dbra sequences (32-step palette fade streamed to $7013xx, #$B3-frame
hold, #$1DF-frame page) where EVERY frame does `jsr frame_tick($6DEC4)` then
`tst $4065D8 → bne exit`. Distinct inner loops observed live: demo-fight
($6B8C6/$6E39E), credited "press start" hold ($6B788/$6DD68), title pages
($6C080-$6C120).
:::

::: region kind=subroutine id=frame-tick addr=0x06DEC4-0x06DEE0 label="frame_tick" confidence=confirmed
Menu-side per-frame helper: `jsr wait_vblank`, decrements the 1/8 prescaler
$40633E, on underflow reloads #7 and increments the global frame counter
$4032BE. All attract/menu loops route through here.
:::

::: region kind=game_loop id=loop-charselect addr=0x06FBF6-0x070DC0 label="char select scene" confidence=confirmed
`$6FBF6`, called at [[master-script]] $2088C. Entry animation runs a bit-2
synced sub-loop ($83F4A ← $6FD56, ~30 frames); steady state parks in
wait_vblank from $6FDCE. Post-select loading loops at $71894-$719A2 use the
clearing wait variant.
:::

::: region kind=game_loop id=loop-fight addr=0x064DEE-0x065256 label="fight round loop" confidence=confirmed
Round loop A (called at $020920): unrolled 8-frame chunks — per frame:
`jsr $2FF52; jsr wait_vblank_clr($237CA); tst $403678 (abort); tst $40636C /
$40636E (per-side round-end) → latch $40646E and exit; jsr $2FFAC; jsr
$65256`. Loop B at $2EA3C (called $020930) has its own frame loop
($2EBD8/$2ECFC callers). Victory/continue scenes loop via $2FEA8/$31DDE with
wait variant $23A4E.
:::

Hop flags (work RAM), all verified live by per-frame watching:

| addr | meaning |
|---|---|
| $4064F2 | IRQ flag bits: 1,2,5(vblank),7(raster) — set by handlers, consumed by waits |
| $4065D8 | scene-advance latch (coin/start); checked every frame by menu loops |
| $403678 | in-game abort → master-script $20FEC (game over/continue) |
| $40636C/E | per-side round-end pulses (cleared next frame) |
| $40646E | round-over latch inside fight loop |
| $402A32 | match end (nonzero = result; observed 2) → $2106E |
| $40655C/E | credits (P1/P2 words) |
| $40633E / $4032BE | 1/8 frame prescaler / global frame counter |
| $406558 | checked by bit-2 IRQ handler (extra work gate) |

Transition anatomy (frame-stepped through a start press): scene loop sees
$4065D8=1 → branches to scene epilogue → returns into [[master-script]] →
script runs fade helper $241B4 (own wait at $241C4, 2 frames) → next scene
init (1 frame) → entry animation loop (~30 frames, bit-2 sync) → steady-state
loop. The "hop" is always a RETURN up to the script, never a jump table.

## Control architecture — ONE shared controller, two actor instances

Answer to "shared character controller vs separate 1P/2P code": there is a
**single shared controller**. P1 and P2 are two instances of the *same* actor
struct, updated by the same pointer-parameterized code; they differ only in
what fills each actor's input fields (human pad vs CPU AI). Not separate
codebases.

Evidence (2026-08-17 live + disasm):

::: region kind=lookup_table id=actor-p1 addr=0x40454C-0x4052FF label="P1 fighter actor struct" confidence=confirmed
Player-1 actor. Distinctive live field run at +0x00..: `... 00 31 00 31 00 01
00 09 00 0f 00 38 ...` (state/anim/timer fields). Pos/velocity churn during
movement at ~$404543-$404561. Responds to the pad (verified: left/right/down/b
each change specific fields; input-history ring at [[cmd-ring-p1]] updates in
lockstep).
:::

::: region kind=lookup_table id=actor-p2 addr=0x405300-0x4060B3 label="P2 fighter actor struct" confidence=confirmed
Player-2 (CPU) actor — BYTE-IDENTICAL layout to [[actor-p1]] at a constant
stride of **0x0DB4**. The init routine [[actor-init]] proves it: fields are
written as P1/P2 pairs ($40454C↔$405300, $40454E↔$405302, $404550↔$405304,
each +0x0DB4). During a fight both structs churn identically at the same
internal offsets — the AI drives the SAME fields the human's pad does.
:::

::: region kind=subroutine id=actor-init addr=0x020098-0x0200CE label="two-actor field init" confidence=confirmed
Boot/round init: writes each actor field twice, P1 then P2 at +0x0DB4
($40454C=1;$405300=1; $40454E=0;$405302=0; $404550=0;$405304=0). Unrolled
per-player, but the identical field set + fixed stride is the proof the two
actors are one type.
:::

::: region kind=subroutine id=input-service addr=0x023BB2-0x023C48 label="coin/start service (both players)" confidence=confirmed
Reads the single gamepad word $810000 and splits it per player by nibble:
`andi #$00F0` = P1 start/coin ($23BBC), `andi #$F000` = P2 ($23BFE). Both
players' inputs come from ONE hardware port — low bits P1, high bits P2. The
in-game movement pad is likewise the same word, consumed per actor.
:::

::: region kind=lookup_table id=cmd-ring-p1 addr=0x400FD8-0x401038 label="P1 command-history ring" confidence=confirmed
Special-move detector's input buffer: ~0x10-byte records tagged with a
decreasing age counter ($2E,$2D,$2C…) and the frame's decoded direction/button
(offset +0x0F in each record tracks the pad: right→3, left→4, down→1). A
parallel P2 ring exists for the AI's synthesized commands (the shared detector
reads whichever ring belongs to the actor being updated).
:::

::: region kind=game_loop id=actor-update addr=0x02E3A2-0x02E4EC label="per-frame actor processor" confidence=likely
Fight per-frame routine (called from [[loop-fight]]). Uses table-indexed
addressing — `lea $2E4EC,a0; lsl.l #7,d4; adda.l d4,a0; move.w -0x56(a0,a2.l)`
— i.e. an actor/state index scales into per-actor tables rather than absolute
per-player code. Consistent with one routine run for each actor pointer;
exact per-actor dispatch not yet fully traced.
:::

The picture: the pad is read once into the shared input word; each actor's
command comes from its own source (pad-derived for P1, AI for P2) but lands in
the same struct fields; a shared, pointer/index-parameterized update processes
both. To retarget the AI or the human onto either slot you'd change the
command SOURCE feeding an actor, not the controller.

## Regions

(Newly confirmed findings from live sessions get appended here by
`add_rom_map_region`.)
