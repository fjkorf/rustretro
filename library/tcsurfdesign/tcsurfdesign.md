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

## Work RAM — RE session 2026-09-09 (headless, fceumm, port 4028)

Live memory-RE pass on the fceumm core (10×1KB CPU-RAM pages, addresses
$0000-$07FF read/write by address; PPU regions still virtual-descriptor
unreadable, unchanged from the Core-capability table). Scope: Step 0
write-liveness + Targets 1-3 (screen/mode gate, input shadow, player entity).
Targets 4 (camera/level) and 5 (physics) deferred; camera bytes noted only as
byproducts. All addresses are guest-absolute CPU RAM ($0000-$07FF).

**Step 0 — write-liveness PASSED.** Paused `write_memory` to $33 and $7F0
both read back exactly; poked values persist across `step` (title-screen
$7F0=A7 held 3 frames). Direct RAM writes land on this core — the whole
session used direct pokes, no Lua-rewrite fallback needed. `list_regions`
now reports 10 "Memory"/Unmapped 1KB CPU pages + PPUREG/NTARAM/PALRAM/OAM
VRAM descriptors (differs from the nestopia single-2KB-region assumption in
older plans; the VRAM descriptors remain unreadable).

### Confirmed globals (write-test or landmark-regression)

| name | addr | width | sign | confidence | how confirmed |
|---|---|---|---|---|---|
| gameplay_gate | $0047 | 1 | unsigned | confirmed | =$04 in all 4 gameplay save-states (Street/BigWave/Wood, A- and B-entry), =$00 in all 6 title/menu/charsel states; flips $00→$04 exactly at round-start (poll at charsel→A). 11-state regression. |
| gate_inverse | $0058 | 1 | unsigned | confirmed | Complement of $0047: =$00 in gameplay, =$02 in all non-gameplay states. Cross-check for the gate. |
| mode_index | $005A | 1 | unsigned | confirmed | Level/mode selector: 0=Street Skate, 1=Big Wave, 2=Wood & Water Rage. Set at char-select, persists into gameplay; matches the SELECTION-menu row entered. |
| player_x_screen | $0478 | 1 | unsigned | confirmed | Write-test: poke +$30 → player OAM-X ($0217) 79→A0; poke −$30 → 40. The derived OAM shadow tracks the poke exactly next step (independent observable). |
| player_y_screen | $048C | 1 | unsigned | confirmed | Write-test: poke +$30 → player OAM-Y ($0214) 78→A8 (down); poke −$30 → up. Also traces a clean jump V-arc A0→80(apex ~f16)→A0 while holding A, absent from the no-input control. |
| pad_latch_p1 | $0703 | 1 | bitfield | confirmed | Raw joypad-1 latch. Held-frame bit per button: right=$01 left=$02 down=$04 up=$08 start=$10 select=$20 B=$40 A=$80; clears on release; ABSENT in the idle negative control; verified across all 8 buttons. This is the game's own $4016 bit order (bit0=right … bit7=A) — NOT the RustRetro RetroPad mask order, so an overlay must remap. Immediate poke reads back (=$01); reverts to 0 after one `step` because the pad is re-polled every frame before use (rebuilt latch, not disproof). |
| frame_oracle | $0033 | 1 | unsigned | confirmed | Free-running frame counter, +$04/frame mod 256 sawtooth; monotone in every state incl. menus. Use as the running/settle oracle — BUT it keeps ticking during in-game pause (see below), so it is NOT a pause oracle. |

### Derived / echo bytes (recorded so a future session doesn't re-chase them)

- **$040A** — decoded/prioritised input nibble (right=$01 left=$02 down=$03
  up=$04 A=$06 B=$05; start/select do not appear). An echo of $0703, mirrored
  again at **$0568**. Useful as a "current action" read but derived, not the raw latch.
- **$0400-$0405** — player metasprite / animation tile indices. Poking $0400
  to its airborne value reverted after one `step` (re-derived), so these are
  render outputs, not the state input.
- **$0214-$026B** — player OAM sprite shadow (Y/tile/attr/X quads); tracks
  $0478/$048C. (Belongs to Target 4's OAM sub-probe, seen here as byproduct.)

### Menu / flow map (Target 1)

- Title ("PUSH START BUTTON", ©1987 LJN) → **Start** → SELECTION menu (3 rows:
  STREET SKATE SESSION / BIG WAVE ENCOUNTER / WOOD AND WATER RAGE, each with a
  1P and 2P variant; row of 8 face portraits on top is decorative).
- **SELECT** is the primary cursor advance on the SELECTION menu (Street-1P →
  Street-2P → BigWave-1P → …); Down/Up barely move it. Menu cursor candidate
  **$008D** changes on every SELECT/Down (likely, not write-confirmed: a poke
  to $8D was consumed/re-derived within 0.5s).
- **Start** confirms into the char-select ("PUSH A-B BUTTON") screen, then
  **A or B** picks the A-button or B-button character and, after a ~4s "ROUND 01
  / GAME START" splash, enters gameplay.
- **In-game pause:** Start in gameplay toggles pause — $0058 goes 0→1 and the
  engine clock **$0004** freezes, but the frontend oracle $0033 keeps ticking.
  Use $0004 as the running/paused oracle in gameplay; $0033 only tells you the
  frontend is running.

### Disproven / negative results

- **Char-select is a fixed 2-choice screen**, not a cursor selector: at "PUSH
  A-B BUTTON" the B-pedestal and A-pedestal each show one preset character
  (lower pedestals are the empty 2P slots); Up/Down/Left/Right change nothing
  (snapshot-diff = animation noise only). Each of the 3 modes presents its own
  A-char and B-char.
- **$002D / $0612 / $0654 / $065A are level AUTO-SCROLL counters, not player X**
  — identical across idle/left/right trials (advance on their own every frame);
  poking $02D or $612 shifts the background scroll (write-test screenshot),
  confirming camera not player. Filed for Target 4.

### TO-VERIFY (blocked observations, not guesses)

- **Character-id byte / roster count** — the A-vs-B-entry RAM diff is confounded
  by auto-scroll phase (states not frame-aligned), so no clean char-id isolated.
  Operator's "4 characters" neither confirmed nor refuted; measured structure is
  2 selectable characters per mode via A/B. Needs a frame-aligned capture (same
  scroll phase) or the char-id read at the charsel screen before entry.
- **Round timer** — ZP $0037/$0039/$003E/$0044 all decrement over ~2.5s of play
  (candidates for the min:sec:tenths HUD "TIME"); not individually mapped.
- **Score / lives** — not located (score display stayed 000000; the 4 LIFE
  hearts not tied to a byte). BCD-vs-binary of score untested.
- **Pause/high-score/game-over screens** not visited — the $0047 gate is
  validated only against title/menu/charsel/gameplay; an unmapped screen could
  in principle also read $0047≠0.

### Gate-condition draft (closed vocabulary)

Primary: **`byte_nonzero $0047`** (OPEN in gameplay only). Optional
belt-and-suspenders inverse cross-check: `byte_zero $0058`. Landmark evidence:

| landmark | $0047 | gate byte_nonzero($47) | classification |
|---|---|---|---|
| title | $00 | closed | non-gameplay ✓ |
| menu (SELECTION) | $00 | closed | non-gameplay ✓ |
| charsel Street | $00 | closed | non-gameplay ✓ |
| charsel BigWave | $00 | closed | non-gameplay ✓ |
| charsel Wood | $00 | closed | non-gameplay ✓ |
| mode1 Street (A-entry) | $04 | OPEN | gameplay ✓ |
| mode1 Street (B-entry) | $04 | OPEN | gameplay ✓ |
| mode2 Big Wave | $04 | OPEN | gameplay ✓ |
| mode3 Wood & Water | $04 | OPEN | gameplay ✓ |

Handoff to Step G (profile authoring; this session did NOT edit the profile or
family.json): the seven confirmed globals above are ready to transcribe into
`memory.globals`, and the gate list to `byte_nonzero $0047`. Player fields are
single-actor GLOBALS ($0478/$048C), not `fighter_fields` — `blocks.block2`/
`stride` stay stubs.
