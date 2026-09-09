---
schema_version: 1

rom:
  name: "tcsurfdesign"
  system: "nes"
  sha1: ""
  crc32: ""
  size: 0

settings:
  scale: 3
  volume: 0.8
  muted: false
  breakpoints: []
  watches: []

meta:
  genre: ""
  year: ""
  developer: ""
  progress: "new"
  tags: []
---

# tcsurfdesign — map

## Overview

T&C Surf Designs: Wood & Water Rage (NES, LJN 1988 per the cart file's date —
title TO-VERIFY on screen). iNES, 32KB PRG + 32KB CHR-ROM, **mapper 3
(CNROM)**: PRG fixed at CPU $8000-$FFFF (no PRG banking — every disassembly
address is stable), CHR switched as four 8KB banks. Vertical mirroring, no
battery, no trainer. File: 65552 bytes, sha1
`7114bc8b0aad3094e59ccedba2845c1c032b15cc` (size+CRC32 also match FBNeo's
`nes_tcsurfdesign` romset entry — not byte-verified against it).

**TO-VERIFY:** operator describes this as a 4-character, 2-mode
sports/platformer (skateboarding + surfing per the CHR text tiles) —
unconfirmed on-screen; roster stays out of family.json until read off a live
select screen.

## Core capability (measured 2026-09-09, headless ports 4026/4027)

| | nestopia 1.53.2 (RetroArch dylib) | FCEUmm (SVN 236ccdf, built from source at ~/Playspaces/libretro-fceumm) |
|---|---|---|
| boots + runs 60fps | ✓ | ✓ |
| memory map | none — `get_memory_data` fallback: one region, "System RAM (fallback)" 2KB @0x0 | `SET_MEMORY_MAPS`: 10×1KB CPU-RAM pages (unnamed → classified "Unmapped", `has_system_ram:false` — cosmetic; reads by address work) + **PPUREG/NTARAM/PALRAM/OAM** classified VRAM |
| PPU regions readable? | n/a | **NO — "declared but not backed by readable memory (virtual descriptor)"**: the frontend does not capture host pointers for address-space-selected descriptors. Frontend enhancement would unlock live OAM/nametable/palette. |
| RAM[0..32] content | `00 00 24 00 00 00 04 00 …` | identical — same reads by address |
| save/load round trip | ✓ (armed load; RAM within 19 bytes of save point on a running instance — timers re-run, exact revert not observable without pausing) | ✓ (within 7 bytes, same method) |
| core name string (live boot line) | `Nestopia` | `FCEUmm` |

**Decision:** `provenance_core: "fceumm"` — everything nestopia does works
identically, plus from-source trust and the declared PPU regions that one
frontend fix unlocks. nestopia remains a verified fallback.
`requires.save_states: true` (both verified); `requires.memory_regions`
stays `false` until PPU regions actually read.

## Regions

_(region blocks accumulate here as you explore)_

::: region kind=interrupt_handler id=ai01 addr=0x00FFFA-0x00FFFF author=ai confidence=confirmed label="6502 vectors"
NMI=$814F (frame loop), RESET=$8001, IRQ=$8000 — parsed from PRG file bytes, probe-verified 2026-09-09 (headless, port 4026).
:::

::: region kind=sprite_sheet id=ai02 addr=0x008010-0x010010 author=ai confidence=confirmed label="CHR-ROM (rom_file offset, NOT a live PPU address)"
Plain 2bpp planar, 2048 tiles, ~490-508/512 non-blank per bank (sprites, score font, 'SKATEBOARDING' text tiles, T&C logo). Mapper 3 (CNROM) bank-switches this as four 8KB CHR banks into PPU $0000-$1FFF — no single live CPU/PPU address covers the span, hence file-offset addressing. rom_info + render_tiles(source=rom_file:chr, format=nes_chr) decode it. Verified 2026-09-09.
:::
