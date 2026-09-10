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
| PPU regions readable? | n/a | **YES since the H1 fix (PR #49, commit e355711, 2026-09-09).** Was NO — the frontend's `host_ptr_for_addr` stripped the `disconnect` mask (0x80000000 PPU-space bit) from the address but not the region start, underflowing the offset so OAM/NTARAM/PALRAM/PPUREG read as "virtual descriptors". Fixed to mask both sides; all four now read live (proven: PALRAM 32 valid bytes, NTARAM changes 988/1024 title→gameplay, OAM shows the Big Wave surfer's 5-region metasprite). |
| RAM[0..32] content | `00 00 24 00 00 00 04 00 …` | identical — same reads by address |
| save/load round trip | ✓ (armed load; RAM within 19 bytes of save point on a running instance — timers re-run, exact revert not observable without pausing) | ✓ (within 7 bytes, same method) |
| core name string (live boot line) | `Nestopia` | `FCEUmm` |

**Decision:** `provenance_core: "fceumm"` — everything nestopia does works
identically, plus from-source trust and the now-live PPU regions (H1).
nestopia remains a verified fallback. `requires.save_states: true`.

## Character rendering — BOTH modes use OAM metasprites (corrected 2026-09-10)

**Correction of a wrong earlier finding.** An initial probe (2026-09-09)
concluded "Street Skate's skater is background-rendered, zero player OAM
sprites." **That was an under-sampling error and is FALSE.** A sustained
re-probe (2026-09-10, port 4031, Phase A independently caught it first) shows
Street Skate DOES have a player metasprite. The trap: **OAM intermittently
reads all-parked (0 active sprites) on clear/DMA frames** — sampled 16×, the
counts were `[0,17,17,17,17,0,17,0,17,...]`. The original single read hit a
0-frame and I published the absolute observation without the sustained control.
The law held in the breach: never conclude from one absolute read on a system
that moves on its own — sample and believe the sustained majority.

The corrected picture — **both modes render the player as an OAM metasprite**
(so both are sprite-swappable):

- **Street Skate (mode 0):** ~17-sprite skater metasprite (OAM slots 5-21,
  tiles ~$41-$74). Sprite X's track `player_x_screen $478` pokes (poke +$20 →
  sprite Xs 104→136), confirming they ARE the player. The skater is
  screen-locked at cruise and the BACKGROUND scrolls behind it — "auto-scroller"
  is true, but that means scrolling background + a sprite character, NOT a
  background-drawn character.
- **Big Wave (mode 1): surfer metasprite, CHR bank 2 / PT0** (8×8 sprites,
  PPUCTRL $90). Base/idle ≈19 sprites; grows to ~26 for carves. 4 characters =
  2 skaters + 2 surfers; surfer A/B share the same 170 tiles and differ only by
  PALRAM subpalette (board color: A red $16, B green $2a). Full contract in
  `assets/swap/sprite_contract.json`; reference PNGs in `assets/swap/`.
- **Wood & Water (mode 2):** opens on a skate segment (same skater metasprite as
  Street); its surf segment reuses the Big Wave surfer — TO-VERIFY.
- **Swap collateral (Big Wave surfer): CLEANLY SWAPPABLE.** No foreign OAM
  slots use the surfer tiles; the 810 NTARAM cells sharing an index are FALSE
  collateral — background uses PT1 ($1000), sprites PT0 ($0000), so the physical
  CHR bytes are disjoint within the 8KB bank. Caveat: a tile edit reskins BOTH
  surfer A and B (shared tiles).
- **CNROM active bank is a RUNTIME fact, not in the file** (write-only latch);
  inferred visually (compose-from-bank vs framebuffer crop → bank 2 for the
  surfer). The panels' "pick the bank" selector is the right handling.
- **`$0400-$0407` is NOT the animation key** (session-1 guess disproven): it's
  static across Big Wave poses. Poses are keyed by the normalized OAM
  (tile,attr,dx,dy) set instead.

## Regions

_(region blocks accumulate here as you explore)_

::: region kind=interrupt_handler id=ai01 addr=0x00FFFA-0x00FFFF author=ai confidence=confirmed label="6502 vectors"
NMI=$814F (frame loop), RESET=$8001, IRQ=$8000 — parsed from PRG file bytes, probe-verified 2026-09-09 (headless, port 4026).
:::

::: region kind=sprite_sheet id=ai02 addr=0x008010-0x010010 author=ai confidence=confirmed label="CHR-ROM (rom_file offset, NOT a live PPU address)"
Plain 2bpp planar, 2048 tiles, ~490-508/512 non-blank per bank (sprites, score font, 'SKATEBOARDING' text tiles, T&C logo). Mapper 3 (CNROM) bank-switches this as four 8KB CHR banks into PPU $0000-$1FFF — no single live CPU/PPU address covers the span, hence file-offset addressing. rom_info + render_tiles(source=rom_file:chr, format=nes_chr) decode it. Verified 2026-09-09.
:::

::: region kind=character_sprite id=ai03 addr=0x00C010-0x00D010 author=ai confidence=confirmed label="Big Wave surfer metasprite — CHR bank 2, sprite pattern table PT0 (file offset = 0x8010 + 2*0x2000)"
The Big Wave surfer (mode 1) is an 8x8 OAM metasprite (PPUCTRL=$90: bit5=0 → 8x8 sprites, bit3=0 → sprite pattern table $0000). Its animation frames live in **CHR bank 2, PT0** (file offset 0xC010, i.e. 0x8010 + 2*0x2000, spanning the first 0x1000 of the bank). Active player bank inferred VISUALLY (CNROM latch is write-only, not exposed by fceumm): the live metasprite composed from each bank and cropped-compared to app://screen matches bank 2 exactly (idle + a 23-sprite carve both reproduced); banks 0/1 incoherent, bank 3 wrong body. Tiles observed across a ~40s ride (union of surfer A+B, 170 of 256 PT0 tiles — the sheet is nearly all surfer): 00 02 03 08 09 0a 0b 0c 0d 0e 0f 10 11 12 13 14 18 19 1a 1b 1c 1d 1e 1f 20 21 22 23 28 29 30 31 32 33 38 39 46 47 48 49 4a 4b 53 54 55 56 57 58 59 5a 5b 5c 5d 5e 5f 60 61 62 63 64 65 66 67 68 69 6a 6b 6c 6d 6e 6f 70 71 72 73 74 75 76 77 78 79 7a 7b 7c 7d 7e 7f 81 82 83 84 8a 8b 8c 91 92 93 96 97 98 99 9a 9b a1 a2 a3 a5 a6 a7 a8 a9 aa ab b0 b1 b2 b5 b6 b7 b9 ba bb bd be c0 c1 c2 c6 c7 c9 ca cb cd ce d0 d1 d2 d4 d5 d8 d9 da db dc dd de e0 e1 e2 e4 e5 e6 e7 e8 e9 f0 f6 f7 f8 f9. Base/idle pose (matches doc): slots 60-63 tiles 08/09/18/19 over the carve extension. Live-probed 2026-09-10, port 4028. Full contract: assets/swap/sprite_contract.json.
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

- ~~**Character-id byte / roster count**~~ — **RESOLVED (session #3).** Frame-
  aligned (gate-open anchored) capture isolated the selection latch **$0704**
  (mirror $0709): 0=A-pedestal char, 1=B-pedestal char. Roster = **4 distinct
  characters (2 skaters + 2 surfers)** reused across the 3 modes (Street=skaters,
  BigWave=surfers, Wood=all 4), proven by $0330–$034F HUD-block byte-identity
  across modes — confirms the operator's "4 characters." See the session-#3
  Roster entry.
- **Round timer** — ZP $0037/$0039/$003E/$0044 all decrement over ~2.5s of play
  (candidates for the min:sec:tenths HUD "TIME"); not individually mapped.
  **RESOLVED (session #2):** the HUD "TIME" is stored as separate display
  DIGITS — $0036=seconds-tens, $0037=seconds-ones, $0038=tenths — see the
  session-#2 Timer entry. $003E/$0044 are lockstep-offset copies (derived);
  $003F/$0045 hold a constant 59 (round limit, not live); $0039 is noise.
- **Score / lives** — not located (score display stayed 000000; the 4 LIFE
  hearts not tied to a byte). BCD-vs-binary of score untested.
  **STILL TO-VERIFY (session #2 tried, failed):** P1 score stayed 000000 the
  whole session (no score event was scriptable — see below), so no digit-carry
  test was possible. LIFE hearts stayed 4; poke-tests of every stably-4 byte
  and of 0x0F-bitmask candidates left the hearts display unchanged. Neither
  isolated.
  **PARTIALLY RESOLVED (session #3):** the 000000 was a STREET-mode artifact —
  score DOES accrue in **Big Wave** via tricks (000040→000100→…). Hi-score is a
  6-digit-per-byte array at $0380/$0388/$0390 (=010000) but poking it didn't
  re-render → derived copy; the live current-score byte is still not isolated
  (candidate +1 event-counter $072E). LIFE: **$0477** is the lives register
  (=0 at charsel, inits to 4 Street / 3 Big Wave, matches the hearts at stable
  play; the drawn hearts misreport during recoverable wipeouts). A true
  death-decrement was not captured. See the session-#3 Score & lives entries.
- **Pause/high-score/game-over screens** not visited — the $0047 gate is
  validated only against title/menu/charsel/gameplay; an unmapped screen could
  in principle also read $0047≠0.
  **PARTIALLY RESOLVED (session #2):** in-game PAUSE was visited — $0047 stays
  $04 (gate correctly OPEN; you are still in a gameplay session), $0058 goes to
  $01. Timer-zero was reached and RESETS the round (does NOT end the game;
  $0047 stays $04, hearts stay 4). Game-over (all lives) and high-score/continue
  screens still NOT reached — no scriptable crash/lose mechanic found. See the
  session-#2 Gate entry for the inverse-cross-check revision this forced.
  **FURTHER RESOLVED (session #3):** Big Wave rounds END back at the TITLE
  ($0047=$00,$0058=$02) — no distinct game-over or high-score-entry screen exists
  on the reachable path. $0047 is multi-valued (0/1/2/3/4/5): 1/2/3/5 are
  wipeout/transition sub-states WITHIN a gameplay session, so `byte_nonzero
  $0047` stays correct (OPEN through the wipeout animation, CLOSED at round-end
  and title/menu). No non-gameplay SCREEN reads $0047≠0. All-lives-lost
  game-over still not forced (wipeouts all recovered). See the session-#3 phase-
  register table.

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

## Work RAM — RE session #2 (headless, fceumm, port 4028, 2026-09-09)

Second live-RE pass. Scope: velocity bytes (A), state-machine + ground flag
(B), gate hardening across new states (C), timer/score/lives (D). All work
was done from a single committed checkpoint reached by menu-macro (title →
Start → Start → A) into **Street Skate** gameplay ($005A mode 0), saved as a
scratch state and `load`-ed for every trial (deterministic replay). Frame-
accurate capture gates on `get_state.frame_count` (NOT $0033 — it ticks during
pause and falsely gates capture on a paused world); the game needs a brief
settle after `load_state` before $0004 begins ticking, so trials `wait_live`
(poll $0004 until it moves) before applying input.

**Method note (paid for twice this session):** `Probe.screenshot()` PAUSES and
stays paused — a `sleep` after it elapses on a frozen world. Two "static
timer" false reads came from screenshotting between two memory snapshots. Read
memory live, or pause ONCE and read bytes + shoot at the same frozen instant.

**Genre correction:** Street Skate is an AUTO-SCROLLER. The skater is
screen-locked horizontally ($0478 sits at ~128 at cruise) and forward progress
is the world scroll ($002D et al., the "camera" counters from session #1 — they
ARE forward progress). Holding RIGHT does nothing extra at cruise (idle==right
scroll rate); holding LEFT brakes (scroll ~74 vs 86 per 70 frames) and drifts
$0478 left at exactly −1 px/frame; the skater re-centers at cruise.

### Confirmed globals (write-test or difference-based control)

| name | addr | width | sign | confidence | how confirmed |
|---|---|---|---|---|---|
| engine_clock | $0004 | 1 | unsigned | confirmed | Free-running while the world advances; FREEZES at in-game pause. Pause test: Start in gameplay → $0004 5→5 static while $0033 kept ticking (186→205); resume → runs again. The gameplay running/paused oracle (session #1 named it; here it is write-test-grade against the pause state). |
| action_state | $040A | 1 | enum | confirmed | Current-action register. GROUNDED it is re-derived each frame from input (right=1 left=2 down=3 up=4 B=5 A=6, idle=0 — the session-#1 "decoded input nibble"). AIRBORNE it LATCHES **6** for the whole jump arc INDEPENDENT of input: tap-A jump (A released frame 3, $0703→0) kept $040A=6 through frames 1–42, clearing to 0 exactly on landing (frame 43). So it is the ACTION/STATE byte, not a raw input echo. Poke persists across steps (not re-derived while latched) but does NOT by itself drive physics → state indicator/output. Mirrored at $0568. |
| ground_air_flag | $0428 | 1 | enum | confirmed | **2 = grounded, 1 = airborne.** Flips at takeoff (frame 1) and landing (frame 43) exactly, INDEPENDENT of input (same tap-A control: A gone by frame 3, flag stayed 1 until landing). Poke persists (not re-derived); poking it alone does not levitate the skater → it is a read-flag the physics/animation consults, not the jump trigger. |
| time_sec_tens | $0036 | 1 | unsigned | confirmed | HUD "TIME" seconds TENS digit (0–9). Difference-confirmed at 3 screenshot anchors: 0:59:0→5, 0:54:1→5, 0:49:1→**4** (crossed the tens boundary). |
| time_sec_ones | $0037 | 1 | unsigned | confirmed | Seconds ONES digit. Same 3 anchors: 9, 4, 9. Poking $0036/$0037 lower drove the on-screen countdown and it kept ticking down from the poked value → these display digits are the authoritative countdown (not a rendered copy). |
| time_tenths | $0038 | 1 | unsigned | confirmed | Tenths digit. Anchors: .0→0, .1→1, .1→1. |

### Derived / disproven (recorded so a future session doesn't re-chase)

- **VELOCITY (VX/VY) — COMPUTED, NOT STORED (TO-VERIFY table/routine in PRG).**
  No persistent signed velocity byte exists. Vertical: player Y $048C traces a
  clean gravity parabola (per-frame dY sweeps −3…+3 smoothly) written DIRECTLY
  to the position; there is no adjacent subpixel byte ($048B/$048D are 0/160
  constant) and no jump-phase timer that resets to 0 at takeoff (the only
  monotone bytes across the arc — $002D,$0033,$045A,$067E/F — are the same
  free-running counters seen at idle). Horizontal: $0478 is screen-locked, and
  when it does move (left-brake) it steps a fixed −1 px/frame. Conclusion:
  velocity is applied by an arc/gravity routine (likely a PRG table), not held
  in RAM. Disprove/confirm by disassembly, not more RAM diffing.
- **$04C8** — jump-pose animation index, tracks HEIGHT not velocity (7 on
  ground → 4 at apex → 7 on landing). Derived render output.
- **$0446 / $0450 / $062F** — flip only at the landing frame (single
  transition), consistent with a landing-SFX/one-shot, not a clean state flag.
- **$003E / $0044** — decrement in lockstep with $0037 (constant offsets: $3E =
  $37+1, $44 = $37+17). Redundant timer copies, derived. **$003F / $0045** hold
  a constant 59 across the whole countdown = the round time LIMIT, not the live
  clock. **$0039** is animation noise, not a timer.
- **$0568** — mirror of $040A (same values every frame). Use $040A.

### Gate hardening (Target C) — new states visited

| landmark | $0047 | $0058 | gate byte_nonzero($47) | classification |
|---|---|---|---|---|
| gameplay (running) | $04 | $00 | OPEN | gameplay ✓ |
| **gameplay PAUSED** (Start in-game) | $04 | $01 | OPEN | gameplay ✓ (correct — still a gameplay session; $0004 frozen) |
| **timer hit 0:00** | $04 | $00 | OPEN | gameplay ✓ (round RESETS — timer refills, hearts stay 4; NOT game-over) |
| game-over (all lives) | — | — | — | NOT REACHED this session |
| high-score / continue | — | — | — | NOT REACHED this session |

**Verdict — primary gate HELD, inverse cross-check needs revision.** The
primary `byte_nonzero $0047` classified pause and the timer-zero round-reset
correctly (both are gameplay, both read $04). BUT the optional inverse
cross-check `byte_zero $0058` FAILS at pause: $0058 is not a pure complement —
it reads **0 = active gameplay, 1 = paused gameplay, 2 = menu/non-gameplay**.
Revise the inverse to **`$0058 < 2`** (i.e. gameplay incl. pause) or drop the
inverse and trust $0047 alone. The gate is still NOT proven exhaustive:
game-over and high-score screens remain unvisited, so an unmapped screen could
in principle also read $0047≠0 — carry this forward.

**New behavioral finding:** timing out (TIME → 0:00) in Street Skate does NOT
cost a life or end the game — it resets the round timer to full with all 4
hearts intact. Game-over therefore requires losing all hearts, and no
scriptable crash/lose mechanic was found (holding a direction skates past the
barrel obstacles without collision; a 20 s idle run never dropped a heart or
left Y=160). Reaching game-over needs a real crash trigger (specific
obstacle/enemy/fall) identified first.

### Score / lives (Target D) — NOT isolated (honest negative)

P1 score display stayed **000000** the entire session (no trick/obstacle score
event could be scripted), so no digit-carry / BCD-vs-binary test was possible.
The LIFE hearts stayed **4**: every byte that read a stable 4 across a 20 s run
($009F,$00D0,$030C,$0409,$0477,$0684–$0686,$06A2) and the 0x0F-bitmask
candidates ($0333,$0603) were poke-tested — none changed the on-screen hearts
($009F/$00D0/$06A2 re-derived back to 4; the rest held the poke but the hearts
render was unaffected). Both remain TO-VERIFY; they need a real heart-loss /
scoring event to anchor a difference-based search.

## Work RAM — RE session #3 (headless, fceumm, port 4028, 2026-09-09)

Third live-RE pass. Scope: char-id/roster (A), score (B), lives (C),
game-over/gate (D), physics/gravity (E). All navigation was deterministic-
replay from three char-select save-states (`cs_street`/`cs_bigwave`/`cs_wood`,
saved by menu-macro from a `menu.state` at the SELECTION screen); there is NO
MCP `reset`/`power` tool and no Lua reset, so a fresh title requires a process
relaunch — the branch point is a `save_state` at the menu, not a reset. The
`load_state`-doesn't-drain-while-paused trap bit once (a "fresh" entry that was
really stale prior gameplay); every load below is `resume → load_state → verify
$0047` (charsel=$00 / gameplay=$04) BEFORE acting. Frame-aligned captures anchor
on the $0047 gate-open transition, which removed the auto-scroll-phase confound
that blocked session #1's char-id diff.

### Confirmed globals (write-test / difference-based / negative-control)

| name | addr | width | sign | confidence | how confirmed |
|---|---|---|---|---|---|
| char_select_latch | $0704 | 1 | enum | confirmed | **0 = A-pedestal char, 1 = B-pedestal char.** Difference-based A-vs-B entry from the same charsel state is deterministic across ALL 3 modes; persists ≥90 frames into live gameplay; NEGATIVE CONTROL: pressing B during live gameplay does NOT flip it (stays at the selection) — so it is the latched selection, not an input echo. |
| char_select_latch_mirror | $0709 | 1 | enum | confirmed | Exact mirror of $0704 (identical 0/1 in every trial). Use $0704. |
| lives_count | $0477 | 1 | unsigned | probable | =$00 at char-select; INITIALIZES $00→N at gameplay start (Street N=**4**, Big Wave N=**3**) matching the on-screen LIFE hearts at stable gameplay. Holds constant through RECOVERABLE wipeouts where the DRAWN hearts transiently show one fewer (e.g. $47=2 wipeout frame renders 2 hearts while $0477=3, and recovery restores the 3-heart render) — anchor on $0477, not the display. A true death-decrement was NOT captured (the game recovers from every wipeout reached); decrement-on-death therefore unconfirmed. Prior "stably-4" byte; prior pokes didn't move the drawn hearts because the HUD redraws only on a change event. |

### Phase register — $0047 is multi-valued (extends the gate model)

$0047 is NOT binary. Observed value set:

| $0047 | meaning | $0058 |
|---|---|---|
| $00 | non-gameplay: title / SELECTION menu / char-select / **round-end** | $02 (menu/title) or **$00 (round-end frame)** |
| $04 | active gameplay | $00 |
| $01/$02/$03/$05 | wipeout / transition sub-states WITHIN a gameplay session ($02 = wiped-out/foam; surfer + HUD still on screen) | $00 |

**Gate verdict:** primary `byte_nonzero $0047` HOLDS — OPEN across active play
AND the wipeout animation (all still "in a gameplay session"), CLOSED ($00) at
true round-end and at title/menu/charsel. No non-gameplay SCREEN was found
reading $0047≠0. Caveat for consumers that treat $47 as strictly 0/4: the
1/2/3/5 sub-states exist. The round-END frame reads $0047=$00 / $0058=$00, so
the session-#2 inverse `$0058 < 2` STILL misclassifies it as gameplay — keep
$0047 as the sole primary; do not trust the $0058 inverse.

### Roster (A) — RESOLVED: 4 characters

**Measured answer: 4 distinct characters = 2 skaters + 2 surfers, reused across
the 3 modes; the pick is a single A/B bit ($0704).** Layout at char-select:
- Street Skate (mode $5A=0): TOP pedestals only = 2 skaters (B-skater / A-skater).
- Big Wave (mode $5A=1): BOTTOM pedestals only = 2 surfers (B-surfer / A-surfer).
- Wood & Water (mode $5A=2): ALL 4 pedestals (both skaters + both surfers) —
  you play skate AND surf segments.

Character SHARING proven by byte-identity of frame-aligned gate-open snapshots:
the skater HUD block **$0330–$034F** is identical `street_a==wood_a` and
`street_b==wood_b` (skaters shared Street↔Wood) and `street_a != street_b` (the
A and B skaters are genuinely different); the surfer sprite block **$0245–$0263**
is identical `bigwave==wood` (surfers shared BigWave↔Wood). So the roster is 4,
not 6 (modes reuse characters) and not 2 — **confirms the operator's "4
characters."** (Surfer A vs B are visually distinct on the charsel screen —
orange vs green creature — but their sprite block did not differ at the gate-open
frame, so A/B surfer identity rests on $0704 + the charsel render, not a sprite-
byte diff.)

Menu/flow refinement: SELECTION-menu cursor = **$005A cycling 0..5** via SELECT
(3 modes × 1P/2P rows); at char-select-entry $005A COLLAPSES to the mode index
(rows 0/1→0 Street, 2/3→1 BigWave, 4/5→2 Wood). During the ~4 s "GAME START"
splash $0047 is already $04 but the pad is NOT live and $0703 holds the
selection button ($80=A / $40=B); it becomes the live pad ($0703 tracks input,
verified via get_input) only once gameplay proper begins. Snapshots taken at the
gate-open frame are during the splash — settle past it before reading live input.

### Score (B) — advanced, live byte not isolated

- Score ACCRUES in Big Wave via tricks (000040 → 000100 → … observed, HUD top-
  left with the surfboard icon). Street score stays **000000** (re-confirmed) —
  session #1/#2's zero was a Street-mode artifact, not a game-wide fact.
- **High score** is stored as a 6-digit-per-byte array (one decimal digit per
  byte, most-significant first); THREE identical copies at **$0380 / $0388 /
  $0390** each = 010000. WRITE-TEST: poking the $0380 array did NOT change the
  displayed HI → these are working/derived copies (or the HUD only redraws HI on
  change), NOT the confirmed render source.
- Live CURRENT-score byte not cleanly isolated: it is not stored as a digit
  array next to the hi-score, and toggle-intersect of two trick-bumps left a
  clean +1 event-counter at **$072E** (increments once per scoring action) as the
  only tidy candidate — not the displayed 6-digit value itself.

### Game-over / high-score screens (D)

Big Wave rounds END by returning straight to the **TITLE** screen
($0047=$00, $0058=$02). No distinct game-over or high-score-ENTRY screen was
reached (dense per-~18-frame capture through the whole end sequence found only:
active → wipeout sub-states $47=2/3/1 → round-end $47=0 → title). Game-over via
all-lives-lost still not forced (no irrecoverable death mechanic found; every
wipeout recovered). Gate classification verified correct at every screen visited
(see the phase-register table).

### Physics / gravity (E) — one confirmed cell

Street / skater-A jump (hold A from grounded $0428=2), difference-based vs a
no-input control from the identical `street_play.state` (control $048C is
FLAT at 160 — pure difference, satisfies the no-absolute-motion law):
- Apex $048C ≈ **121–122** (rise ≈ **38–39 px** above ground 160).
- Airtime ≈ **29–30 frames**, arc roughly symmetric.
- $0428 reads 1 (airborne) through the arc, flips to 2 exactly at landing.

Determinism-repeat (second rig): the arc is reproducible in SHAPE but NOT
frame-exact — per-frame dY jitters ±1–2 px run-to-run from the same load. This
matches session #2's "velocity computed-not-stored, no subpixel byte": the drawn
$048C rounds a fractional position, so a per-frame gravity CONSTANT is not
extractable at integer precision here. The ballistic ENVELOPE (apex ≈38 px /
airtime ≈29 f, Street·skater-A) is the confirmed cell; the per-frame constant
needs the pause→step→let-frame-finish discipline (my resume+poll stepping slips
sub-frame).

## Sprite-swap Phase A — Big Wave surfer metasprite (2026-09-10, headless fceumm, port 4028)

Deliverable: the metasprite CONTRACT an external image generator must satisfy
to reskin the Big Wave surfer, plus reference PNGs and a tile-sharing scan.
Contract + assets live under `assets/swap/` (`sprite_contract.json`, 12 pose
PNGs). All facts live-probed; PPU OAM/PALRAM/PPUREG read directly via
`read_region` (H1 fix confirmed working).

### What was established (confidence: confirmed unless tagged)

- **Sprite size = 8x8, sprite pattern table = $0000.** PPUCTRL ($2000 shadow /
  PPUREG[0]) = `$90` throughout: bit5=0 (8x8), bit3=0 (PT0), bit4=1 (background
  uses PT1 $1000). This PT0/PT1 split is the whole swappability story (below).
- **Player CHR bank = 2** (probable — visual inference; the CNROM latch is
  write-only and fceumm does not expose it, and the CHR pattern tables are NOT a
  live-readable region so `vram_to_rom` cannot corroborate). Method: compose the
  live metasprite (exact OAM tile/attr/x/y) from each of the 4 banks' PT0 and
  crop-compare to `app://screen`. Bank 2 reproduces the surfer exactly on both an
  idle frame and a 23-sprite carve frame; banks 0/1 are incoherent, bank 3
  renders a wrong body. Bank 2 PT1 (rendered) holds the HUD font
  ("0123456789 POINT SURF LIFE") + wave tiles, not surfer art.
- **Roster / A-vs-B.** Surfer A ($704=0, A-pedestal) and Surfer B ($704=1)
  **share one sprite tile set** in bank 2 (168 tiles identical; the 2-tile delta
  is sampling noise) and are distinguished **purely by PALETTE**: PALRAM sprite
  subpalettes 4/6/7 differ. Board color = subpalette 7 index2: A=$16 (red),
  B=$2a (green). One tile swap therefore reskins BOTH surfers.
- **Poses.** The surfer is screen-ANCHORED on the wave — the doc's
  poke-$478/$48C player-ID test does NOT apply in Big Wave (both re-derive each
  frame; the metasprite does not translate with them). Player sprites = ALL
  active OAM except slot 0 (a parked tile-$FF junk sprite at x0/y190); across a
  ~40s ride for A and B, NO active OAM sprite ever appeared outside the surfer
  cluster (nonplayer_oam scan empty). The metasprite is driven by wave momentum
  + input over continuous play (~165-172 distinct normalized (tile,attr,dx,dy)
  configs per character; ~25-28 single-cluster). $0400-$0407 is STATIC
  (`589804d000143cf0`) in Big Wave — NOT the per-pose key the session-1 doc
  guessed. 6 curated single-cluster reference poses per character were captured
  (tuck=10, idle/base=19, carve=20/23/25/26 sprites); the compose pipeline was
  validated by exact crop-match to the live framebuffer on a non-idle pose.

### Tile-sharing scan — Phase-A GATE — VERDICT: cleanly swappable

- **OAM:** in Big Wave the only active sprites are the surfer + the inert slot-0
  junk sprite; zero foreign sprites reference the surfer tiles →
  `tiles_shared_with_nonplayer = []`.
- **NTARAM:** 810 / 2048 background cells hold an index that also appears in the
  surfer tile set, but this is **FALSE collateral**: background uses pattern
  table 1 ($1000) while sprites use PT0 ($0000). Nametable value V →
  bank_base+$1000+V*$10; surfer sprite tile V → bank_base+V*$10 — DISJOINT
  physical bytes. Swapping the surfer's PT0 tiles cannot touch the HUD/wave
  graphics. **Caveat:** the edit reskins both surfer A and B (shared tiles) and,
  if bank 2 is reused by the Wood surf segment, that segment too.

### Correction / discrepancy (measurement over doc)

- **Street Skate (mode 0) is NOT zero-OAM.** Contradicting this doc's
  "Character rendering" section and the Phase-A brief: live PPU OAM in Street
  gameplay (mode 0, gate=$04) shows a ~17-sprite skater metasprite (OAM slots
  5-21, tiles $41-$74) clustered exactly at the on-screen skater
  (x104-136,y120-152) — the skater IS OAM sprites at the skate-segment START,
  not background-rendered. The prior "zero player OAM in Street" claim was not
  reproduced; it may have described a later auto-scroll phase. Flagged
  TO-VERIFY. (The doc's Big Wave surfer claims WERE independently reproduced.)
- **Wood & Water (mode 2)** OPENS on a skate segment visually identical to
  Street (same skater metasprite, tiles $41-$74), not the surfer; its surf
  segment was not reached this session (TO-VERIFY — expected to reuse the Big
  Wave surfer per the roster-sharing finding).
