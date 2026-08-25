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
Player-1 actor (base $40454C). Field offsets below were labeled by controlled
live diffing — driving one input at a time in a verified-controllable round
(gate: tap right, confirm X moved) and watching which word ramps. Offsets are
from the struct base; P2 mirrors at +0x0DB4 (see [[actor-p2]], [[actor-init]]).

| offset | field | how confirmed |
|---|---|---|
| +0x00 | free-running frame timer | counts down every frame in all states |
| +0x12/+0x14 | walk / animation frame counter (dup word) | ramps while moving, resets on state change |
| +0x28 | right-movement hold accumulator | ramps only while holding right |
| +0x2A | left-movement hold accumulator | ramps only while holding left |
| +0x4C/+0x50 | current command / action index (dup word) | tracks decoded pad each frame (right→9, left→…, neutral→0) |
| +0x54 | **X position** (screen px, +→right) | ramps 0x78→0xB6 on right-walk, plateaus at wall |
| +0x56 | **Y position** (screen px, +→down, ground≈0xD8) | dips-and-returns through a jump arc |
| +0x5A | secondary X ref (+0x60 from X) | ramps in lockstep with +0x54 |
| +0x5C | secondary Y ref | arcs in lockstep with +0x56 |

| +0x47 | ~~health~~ **DISPROVEN 2026-08-24** | in live 1P play `$404593` bounces with animation state and never tracks the bar; real health is at `$40390F`/`$4046C3` — see [[health-blocks]] |
| +0x4F | ~~paired health~~ DISPROVEN with +0x47 | same |

Still not isolated: facing bit, meter. The absolute-vs-camera X split is now
resolved — see [[world-positions]]. The input-history ring at [[cmd-ring-p1]]
updates in lockstep with the command field. The 2026-08-20 "+0x47 health"
finding was demo-phase state, disproven live 2026-08-24 — real health is in
the separate per-fighter blocks [[health-blocks]] (P1 `$40390F`, opp
`$4046C3`, stride 0x0DB4).
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

::: region kind=lookup_table id=round-timer addr=0x40000A-0x40000B label="round timer (BCD seconds + subsecond)" confidence=confirmed
`$40000A` = round seconds in **BCD** (displays "58" → holds 0x58); `$40000B` =
subsecond countdown. Found by frame-exact step-diff (single 0x58→0x57 decrement
across 80 stepped frames, matching the HUD). **Freezing both bytes holds the
round indefinitely** (no timeout); the freeze survives round and stage
transitions. Live 1P session 2026-08-24, author=ai.
:::

::: region kind=lookup_table id=health-blocks addr=0x40390F-0x4046C5 label="per-fighter health blocks (1P mode)" confidence=confirmed
Health does **NOT** live at actor+0x47 (that demo-derived claim is disproven —
`$404593` bounces with animation state in live play and never tracks the bar).
Real health, live-verified by damage-correlation AND write-tests (writing the
byte moves the on-screen bar; game's own round refill re-inits it):

| addr | field |
|---|---|
| `$40390F` | **P1 health** (byte). Full-bar observed 0x66 (Yashaou); per-char max |
| `$403911` | P1 displayed-bar value (chases actual downward) |
| `$4046C3` | **opponent health** (byte) = P1 + 0x0DB4. Full 0xEF (Footee, Zam-B) |
| `$4046C5` | opponent displayed-bar value |

The 0x0DB4 stride reappears here (same per-fighter allocation unit as the
actor structs). Freezing `$40390F`/`$4046C3` = both fighters unkillable — the
"held fight" sandbox is round-timer freeze + these two. Live 1P session
2026-08-24, author=ai.
:::

::: region kind=lookup_table id=world-positions addr=0x4027CE-0x4032EF label="world-space X positions + camera (1P mode)" confidence=confirmed
`$4032EE` = **P1 world X** — equals screen X (`$40454C+0x54`) + camera X.
`$4027CE` = **opponent world X** (verified: chases P1's world X as P1
retreats). Camera X readable from the scroll table at `$400024+` (all entries
equal when the view is settled; `$400032` used as reference). This resolves
the "absolute-vs-camera X split" pending item: `+0x54` block fields are
SCREEN-space. (Superseded detail: both fighters' screen X/Y are the per-block
`+0x54/+0x56` fields — block1 `$4037EC/EE`, block2 `$4045A0/A2` — see the
corrected block model below.) Live 1P session 2026-08-24, author=ai.
:::

## Fighter data blocks — the corrected model (2026-08-24, live + external)

The per-fighter allocation unit is a **0x0DB4-stride block**, and there are two
live ones per match: **block1 = `$403798`** and **block2 = `$40454C`**
(= block1 + 0xDB4). External sources (pugsy cheat DB, peon2's FBNeo
training-mode Lua) label block1 = P1 and block2 = P2, and every P1/P2 cheat
pair differs by exactly 0xDB4. Live confirmations (attract demo + 1P fights):

| offset in block | field | confirmed how |
|---|---|---|
| +0x54 / +0x56 | screen X / Y of this block's fighter | attract demo: block1 = left fighter, block2 = right; 1P: injected P1 right-walk ramps block1 X |
| +0x61 | **facing** (0 = facing left) | flips live as fighters cross (`$4037F9` / `$4045AD`) |
| +0x65 | weapon flag (0=armed, 1=disarmed; only Yashaou/Lightning/Zam-B/Goat safely disarmable) | cheat DB |
| +0x177 / +0x179 | **health pair** (max 0xEF) | write-tested both sides (`$40390F`/`$403911`, `$4046C3`/`$4046C5`); game may keep "2 stacked bars" per round (FAQ) — pair semantics still to pin down |
| +0x17B | **super meter** | `$403913`/`$4046C7`; observed == per-char max when bar shows MAX |
| +0x17F | per-character **max meter** constant | Yashaou 0x51, Footee 0x36 (cheat DB "typical 0x36" matches) |
| +0x639 | **character ID** | `$403DD1`/`$404B85`; live: demo read 3 vs 2, 1P read 0 (Yashaou) vs 3; bosses 08/09 per cheat DB |
| +0xA4C | win count | cheat DB (`$4041E4`/`$404F98`) |
| +0xCF1 | "Magic Boost" flag | cheat DB |

**CRITICAL caveat — the `+0x28`/`+0x2A` hold accumulators track the
*opponent's* held direction, not this block's own fighter** (live 2026-08-24:
injected P1 right/left holds ramped **block2's** rhold/lhold while block1's X
did the walking). This is what fooled the demo-era controlled-diffing into
labeling `$40454C` as "P1's actor struct" — the whole [[actor-p1]] field table
above should be re-read with block-slot skepticism; kinematics (+0x54/+0x56)
and input-decode fields may belong to different fighters within one block.
Combo counters are cross-block too (`$4041E7` = P1's combo landing on P2,
`$40470B` = P2's; ≠ 0 doubles as "opponent in hitstun", per the FBNeo
training-mode Lua). Which block a fighter occupies per mode/match — and the
demo-era `$405300` (= block2 + 0xDB4, a third slot?) — still needs one
dedicated session; anchor at round start via X (left = smaller) + char ID +
facing rather than assuming slot order.

Also disproven: `+0x47`/`+0x4F` as health (see [[health-blocks]]). Also
measured: the recorder's `controllable` gate (hop flags all zero) is **true on
the title screen** — it needs a positive in-fight signal before training data
is cut from recordings (open item; `$406485` and `$40FF67` were tested and are
scratch, not flags).

### Roster — character IDs (`+0x639`) — COMPLETE

8 playable (ids 0–7) + 2 bosses (0x08/0x09, cheat DB). **Fully mapped
2026-08-25** by the headless roster probe: one cold boot per char-select
slot, cursor walked Right ×k, id read in-fight (strict gate: recorder gate
AND `$400006` == 0 — the plain gate is TRUE on char select!), name read from
the select screen + health bar. The three previously known ids (0/1/7) all
reconfirmed, plus four ids double-confirmed from fight health bars.
Hand-kept mirrors: `CHAR_NAMES` in `shadow/train/shadow_train/asurabld.py`
and `char_name` in `src/record.rs` — update all three together.

| id | name | select slot (Rights from default) |
|---|---|---|
| 0 | Yashaou | 0 |
| 1 | Goat | 3 |
| 2 | Lightning | 6 |
| 3 | Footee | 4 |
| 4 | Alice | 7 |
| 5 | Taros | 1 |
| 6 | Zam-B | 2 |
| 7 | Rose Mary | 5 |
| 8 | Curfue (boss) | code: hold Down+Start from confirm until the battle begins (per fight) |
| 9 | S. Geist (boss) | code: hold Up+Start likewise (GameFAQs codes; both probe-verified live, health-bar names + in-fight id reads) |

Probe gotchas (for future probes): char ids at `+0x639` are STALE during
char select and the VS splash — only trust them once the strict gate holds
for ~2 s. `$400006` (char-select countdown, BCD) doubles as the phase
discriminator: live BCD = on select, 0 = in fight. From a cold boot, press
Start repeatedly until `$400006` reads live BCD — one early press just sits
on the title screen.

### Stages — `$40364D` is a WRITE-VERIFIED opponent+venue selector (1–9)

Freezing `$40364D` through the post-select map screen forces both the stage
AND its home character as the next opponent (paired-control verified
2026-08-25: same player char, values 2/6/none → reproducibly different
venues; value 2 reproduced the same venue+opponent across independent runs).
The cheat DB's "stage select (1–8)" undersells it — 9 works too and forces
the S. Geist fight. Unwritten, the byte stays 0 (it is a selector input, NOT
a live stage indicator) and the ladder picks normally.

| value | home char (forced opponent) | venue (probe screenshots) |
|---|---|---|
| 0 | — unset/default (first ladder fight often Footee) | Footee's beach courtyard |
| 1 | Zam-B | torch-lit dungeon, gargoyle gate |
| 2 | Lightning | floating-rock water cavern |
| 3 | Alice | sunken-shipwreck graveyard |
| 4 | Taros | skull-pile foundry hall |
| 5 | Rose Mary | skull-pile hall (visually near-identical to 4) |
| 6 | Goat | red desert castle at sunset |
| 7 | Yashaou | fiery inferno (forces a mirror match) |
| 8 | Curfue | (venue matched the shipwreck in the probe) |
| 9 | S. Geist | direct boss fight |
| 10+ | overflow: Yashaou mirror on the desert stage — invalid | |

Footee's beach has no dedicated selector value in 1–9 (open oddity — it may
be the "default" venue only). Probe artifacts (screenshots + jsonl log):
scratchpad/roster-probe-20260825/.

::: region kind=lookup_table id=system-control addr=0x400000-0x40655D label="match/system control bytes" confidence=confirmed
From the pugsy cheat DB + FBNeo training-mode Lua, live-verified where noted:
`$400000` write 00 = finish round now · `$400006` char-select timer (BCD) ·
`$40364D` stage select (1–8) · `$40655D` **credits** (write-verified live:
writing 9 showed CREDIT 8 after one start consumed) · `$406431/3` max-hit
records. Live round timer is [[round-timer]] (`$40000A` BCD, starts 0x90;
final tie-breaker round starts at 40 per FAQ).
:::

## Training script

`library/asurabld/training.lua` is a standalone Lua v2 training surface —
launch it with `--script library/asurabld/training.lua`, with or without
`--training`. It rides entirely on the sandboxed Lua API (`src/lua_engine.rs`;
`table`/`string`/`math` only, no `io`/`os`/`package`) and duplicates the native
`--training` enforcement (`src/training.rs`) in Lua so a script-only launch
still gets a held round: the constants match field-for-field (block1
`$403798`, block2 `$40454C`, timer `$40000A/B` pinned `0x85/0x03`, credits
`$40655D`, health pair `+0x177/+0x179` max `0xEF`), so running both at once is
a no-op overlap rather than a fight over the RAM. Writes are gated the normal
way (`memory.writebyte`/`writeword` need `--training` or the MCP
`enable_writes` tool); every write in the script goes through a `pcall`
wrapper that flips a "writes locked" flag instead of throwing, and the
overlay shows a red **WRITES LOCKED** line whenever it's set — verified live
by loading the script *without* `--training` and confirming the overlay text
and a `false` return from `finish_round()`, then calling MCP `enable_writes`
and confirming both flip.

**CONFIG table** (top of the file; edit + reload via F10, or mutate live with
MCP `run_lua` — e.g. `run_lua("CONFIG.dummy = 'crouch'")` — since `run_lua`
executes in the *same* VM as the loaded script and every CONFIG field is read
fresh each frame):

| field | default | effect |
|---|---|---|
| `credits_enabled` / `credits_target` | `true` / `9` | tops up `$40655D` once/second |
| `timer_hold_enabled` / `timer_hold_sec` / `timer_hold_sub` | `true` / `0x85` / `0x03` | pins the round timer |
| `health_refill_enabled` / `refill_below` / `refill_value` | `true` / `0x40` / `0xEF` | refills both health bytes on either fighter when either drops below the threshold |
| `dummy` | `"off"` | `off\|stand\|crouch\|jump\|block\|replay` — drives port 1 each frame |
| `record` | `false` | arms in-memory capture of port-0 input (cap 600 frames); flip back to `false` to stop |
| `overlay_enabled` | `true` | top-left HUD: round-live state, both blocks' HP/meter/position/facing, hitstun (combo-counter-changed-in-last-20-frames), dummy/record state, write-lock warning |

Enforcement (`credits`/`timer`/`health`) only runs while a round is judged
live (`round_live()`: hop flags `$40646E`/`$403678`/`$402A32` all clear, both
blocks' health in `1..0xEF`, timer byte valid BCD). `dummy` releases port 1
outside a live round so it doesn't wander through menus.

**Record/replay caveat**: the sandbox has no `io`, and reloading the script
(`F10` Reload, or a fresh `--script` load) throws away the whole Lua VM
(`LuaEngine::reload()`), so an in-memory recording does **not** survive a
reload. To go from `record=true` to `dummy="replay"` without losing the
buffer, flip both live via MCP `run_lua` instead of editing-and-reloading the
file. Live-verified: 27 frames captured from `input.get(0)` over a burst of
`right` presses, read back with `run_lua("#recording.buffer")`, then replayed
onto port 1 (`input.get(1)` sampled the recorded mask on a loop).

**One-shot helpers** (globals, callable from the F10 panel or MCP `run_lua`):
`reset_positions()` writes round-start X/Y (block1 84/216, block2 232/216 —
ground `y=216`); `finish_round()` writes 0 to `$400000`; `arena_save(slot)` /
`arena_load(slot)` wrap `savestate.save/load`. All log their result via
`console.log`. `reset_positions()` was verified with the MCP `pause`/`step`
tools: while paused it lands both blocks exactly on 84/232, holds through one
single-frame `step`, and only reverts after `resume` — because `+0x54/+0x56`
are live-recomputed screen positions once the fight is actually running, not
free-standing state, so the helper is most useful at a genuine round start or
while paused, not mid-scramble.

## Input mapping & controls (source + live verified 2026-08-24)

fbalpha2012's driver (`d_fuukifg3.cpp` AsurabldInputList) declares exactly
**3 fire buttons** per player; the libretro layer's generic mapping binds
**fire1→RETRO B (Light), fire2→A (Medium), fire3→Y (Heavy)**; coin→Select,
start→Start; **X/L/R are never polled** (live injection test agrees: A
attacks, X/L/R do nothing — an earlier "{B,Y,R}" calibration note was a
pad-row misattribution). Game mechanics (FAQ-verified): weapon toss =
**L+M+H chord**; universal launcher ("Bash Attack") = **any two attack
buttons**; EX = motion + two buttons; block = hold back / down-back (air
block ok); throw = f/b+M/H close; dash f,f; **health regenerates ~1%/1.5s
standing neutral** (health is NOT monotone within a round).
