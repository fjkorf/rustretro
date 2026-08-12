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
`assets/palette_ram.png`, `assets/verify_side_by_side.png`, and
`assets/sprites/` (578 per-entry strips with manifest; characters are stacks
of 1-row strips — pose-level grouping of adjacent entries is the natural next
step).

## Regions

(Newly confirmed findings from live sessions get appended here by
`add_rom_map_region`.)
