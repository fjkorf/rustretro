---
schema_version: 1

rom:
  name: "Mortal Kombat II (rev L3.1)"
  system: "Midway T-Unit"
  archive: "mk2.zip"
  fbneo_short_name: "mk2"   # FBNeo parent set = rev L3.1 (d_tunit.cpp)

settings: {}

meta:
  genre: fighting
  year: 1993
  developer: "Midway"
  progress: "Memory-RE pass (2026-08-27, headless FBNeo on MCP port 4036).
    Fighter structs (char id, health), P1/P2 world X, the round-over flag,
    the select-pipeline flag, the HUD timer digits, and health max are all
    LIVE-VERIFIED (write-tests where possible). The controllable gate is
    composed entirely from the existing closed vocabulary and was checked
    against 40+ labeled snapshots across every reachable phase. Open
    unknowns: player Y/height, an authoritative round-timer store, credits
    (lives outside the exposed region), wins."
  tags: [arcade, 2d-fighter, t-unit, tms34010, fbneo]
---

## Overview

Mortal Kombat II runs on Midway T-Unit hardware: a TMS34010 (a 32-bit CPU
with **bit-addressed** memory) plus a 6809 sound board. Under FBNeo the core
publishes ONE libretro memory region, `System RAM (fallback)`, 2,367,868
bytes. That blob is FBNeo's `AllRam..RamEnd` span from
`src/burn/drv/midway/midtunit.cpp` (`MemIndex()`), laid out as:

| blob offset | contents |
|---|---|
| `0x000000-0x0FFFFF` | `DrvRAM` — main CPU work RAM, mapped at TMS bit-address `0x01000000-0x013FFFFF` (only the first 512 KiB is addressable by the game) |
| `0x100000-0x1020FF` | sound CPU RAM + protection RAM |
| `~0x102100-0x1420FF` | palette RAM (two banks) |
| `~0x142100-0x2420FB` | `DrvVRAM` — the framebuffer |

**Address conversion — the master formula of this whole session**: FBNeo's
TMS34010 fast path (`src/cpu/tms34010_intf.cpp`, `tms_fast_read`) indexes
block-mapped memory as `(bit_address & PAGE_MASK) >> 3` per 0x1000-bit page,
which collapses to

```
blob_offset = (tms_bit_address - 0x01000000) >> 3
```

for the work-RAM window. Every MAME/fightcade address below was converted
with this. Multi-byte values are **little-endian**; game variables observed
are u8/u16 at byte-aligned offsets (no sub-byte fields encountered).

All offsets below are plain byte offsets into the `System RAM (fallback)`
region — usable directly with `read_memory`/`write_memory`/watches.

## Method

1. **Save-state anchored diffing.** `save_state`/`load_state` work on this
   core (unlike the fbalpha2012 games) and were the session's superpower: a
   state banked at the literal "ROUND 1" frame (timer 99, both bars full)
   was reloaded ~15 times to rerun probes under identical conditions.
   Replays are deterministic — two reloads with the same sleep produce
   byte-identical screenshots.
2. **Snapshot series + trajectory filters.** 8 full-RAM snapshots at ~1.2 s
   intervals across a live fight (screenshot at every pause), then offline
   searches for bytes matching the *shape* of each variable (constant →
   monotone-decreasing for health, etc.). A single-frame step-noise set
   (pause / snap / step / snap) subtracts per-frame churn.
3. **External anchors, converted then live-checked.** The MAME cheat DB's
   old-format line `:mk2:00000000:20BCA0:000000A1:...:Infinite Energy PL1`
   and fightcade-detectors' `mk2.inf` (raw TMS bit-addresses + the full
   roster id table) both target this exact revision. Everything adopted
   from them was re-verified against live reads before being trusted;
   one adopted address (MK1's `0x1051300`, mis-attributed to MK2 by a
   search result) was tested, found dead (reads 0), and discarded.
4. **Write-tests for authority.** Candidate found → write it → screenshot.
   Health, both X positions, and the timer stores got this treatment; the
   teleports are visually unambiguous.
5. **Phase sweeps for the gate.** 46 labeled snapshots: live fight, ROUND-N
   banner, FIGHT!!, post-KO "RAIDEN WINS", FINISH HIM, fatality, continue
   countdown, game over, attract (including an attract fatality demo),
   title, char select, ladder ("battle plan"), pre-fight bios. The gate
   conditions below hold across all of them.

**Probe gotchas (MK2-specific):**
- Real time runs between agent turns; a CPU opponent KO's an idle P1 in
  ~15 s. Bank a state FIRST, and do every multi-step interaction inside one
  script.
- At the ROUND-N banner, health *fills* 0→38→150→161 over ~1.5 s while the
  drawn bar is already full. Reads during the banner see mid-fill values.
- The CPU (esp. Raiden) crosses sides constantly (teleport); never assume
  P1 is the left fighter.
- `freeze` (the watch-based re-writer) does NOT write on this core's
  fallback region: the watch registers (`frozen: true`) but the value never
  lands (verified on an untouched address — frozen 77, read back 0).
  `write_memory` works (host-pointer write) and one-shot writes stick until
  the game itself rewrites the address. Enforcement therefore needs
  periodic rewrites, not freezes.
- P2 joining mid-1P-game (start on port 1) aborts the current match and
  puts the *interrupted CPU character* on the P1 side. Fine for probes,
  surprising the first time.

## Fighter data — verified

Two per-player state blocks, **P1 = `0xC050`**, **P2 = `0xC1CA`**, stride
**`0x17A`** (= fightcade's `char1`/`char2` at bit `0x01060280`/`0x01060E50`).

| offset in block | field | confirmed how |
|---|---|---|
| `+0x00` | **character id** (u8; table below) | read 1 (Liu Kang) / 7 (Raiden) in fight 1, 0 (Kung Lao) / 3 (Baraka) in fight 2, 3 / 9 (Baraka vs Reptile) in fight 3 — every read matched the on-screen health-bar name. Tracks the select cursor live on the choose-your-fighter screen (1→0 on one Right press). |
| `+0x0E` | **health** (u8, max **0xA1 = 161**) | fills 38→150→161 during the ROUND banner, steps down with visible hits, 0 at KO. Write-tested: values written mid-fight stick and keep taking damage; refilling to 161 restores a full bar. |

The health bars are backed by a SECOND, parallel pair of health bytes —
**P1 `0xBCA0`, P2 `0xBC88`** (the MAME cheat DB's "Infinite Energy"
addresses, old-format `0x20BCA0` → blob `0xBCA0`). Both pairs fill/drain in
near-lockstep but are decremented **independently** by the damage code
(write-tested: writing one does not resync the other, and both continue to
take damage from their written values). A refill enforcement must write
BOTH pairs. KO fires when they reach 0 (loser reads 0 through FINISH
HIM/fatality/continue).

### Positions

Player X lives in a different structure — a `0x42`-stride object array:

| address | field | confirmed how |
|---|---|---|
| `0x6CBA` | **P1 world X** (u16) | ramps smoothly under held Right (866→991 over 5 bursts); write-tested TWICE: `write_memory 0x6CBA=1250` mid-fight visually teleported Liu Kang past Raiden (screenshot), and the game kept walking from the written value. |
| `0x6CFC` | **P2 world X** (u16) | = `0x6CBA + 0x42`; write-tested: `0x6CFC=780` teleported Raiden from P1's right to P1's left (screenshot); responds to P2 (human) held-left movement in a 2P fight. |
| `0x3E40` | P1 **screen X** (u16, derived) | recomputes from world X minus camera (delta ≈ 728-736 in the fight observed) within a frame of the world-X teleport. Read-only. |
| `0xBE81` | P1 facing? (u8, **likely — not write-verified**) | 1→0 exactly when P1 crossed to P2's right side, in two independent fights (mirror copy at `0xD2A3`). |

World X is stage-relative (values 800-1100 on the Dead Pool, ~240 center on
the arena stage of fight 3); the screen is 400 px wide (`retro_get_system_av_info`:
400x254 @ 54.71 Hz).

**Player Y / jump height: NOT FOUND — honestly open.** A stepped jump-arc
probe (6 snapshots, 8 frames apart, screenshot-verified airborne) found no
u16/u32 anywhere in work RAM tracing the arc. The changes during a jump are
confined to animation-pointer fields (`+0x0E..0x12` of the object entry)
and a 0x18-stride per-scanline sprite table (`0x39xx-0x3Cxx`) whose entries
move with screen Y. Three arc-shaped candidates (`0x6DE0`, `0xDFC8`,
`0x33B4`, all stepping 256→416 in 32s) were freeze/write-tested: all are
recomputed outputs (game value wins within a frame). Working hypothesis:
MK2's table-driven jumps keep height as (table index, base) rather than a
plain coordinate. Next session: trace the scanline-table writer, or diff
against the leaked MK2 GSP source's OYVAL semantics.

### Roster — character IDs (block `+0x00`)

Id table from fightcade-detectors `mk2.inf` (production round-detection for
this exact short name). Verification status per id, this session:

| id | name | verified? |
|---|---|---|
| 0 | Kung Lao | **YES** — in-fight id read + "KUNG LAO" health bar; select cursor 1→0 on one Right from default |
| 1 | Liu Kang | **YES** — in-fight id read + "LIU KANG" health bar (two fights); select default highlight |
| 2 | Johnny Cage | sourced (grid row 1 col 3, unvisited) |
| 3 | Baraka | **YES** — in-fight id read + "BARAKA" health bar, twice (CPU opponent, then P1-side CPU) |
| 4 | Kitana | visual — attract fatality demo read char2=4 with Kitana on screen (no bar name) |
| 5 | Mileena | sourced |
| 6 | Shang Tsung | sourced |
| 7 | Raiden | **YES** — in-fight id read + "RAIDEN" health bar |
| 8 | Sub-Zero | sourced |
| 9 | Reptile | **YES** — P2 select cursor read 9 on the Reptile portrait, then in-fight id read + "REPTILE" health bar |
| 10 | Scorpion | sourced |
| 11 | Jax | visual — attract fatality demo read char1=11 with Jax on screen |
| 12-16 | Kintaro, Shao Kahn, Smoke, Noob Saibot, Jade | sourced (bosses/secret — not selectable) |

Select grid (4 wide x 3 tall), P1 default = Liu Kang (slot 0), one Right =
Kung Lao (slot 1). P2's default cursor = Reptile. Full slot map unmapped.

## Match-state globals — verified

All in the `0xC33x-0xC3Ax` cluster (bit `0x1061Axx-0x1061Dxx`), plus timer:

| address | name | semantics (observed) |
|---|---|---|
| `0xC360` | **round_over** (u8) | **0 during live rounds** (25+ live samples incl. two matches, both rounds); 2 the moment a round is decided (through "X WINS", FINISH HIM, fatality, continue screen), 3 after the match-deciding KO. 0-2 wander in attract. The gate's kill switch. |
| `0xC37E` | **screen_state** (u16) | 262 continuously from char select through ladder/bio/loading screens; **0 during fights** and post-KO; 0/259/263 across attract screens. Kills the char-select gate leak (both healths read in-range garbage there: 11/133). |
| `0xC35E` | round_num (u8) | 0 outside matches, 1 during round 1 (from the ROUND 1 banner), 2 during round 2, stays nonzero through the fatality/win screens until the continue screen. Fightcade's `start eq 1` match detector. NOT a clean "controllable" flag (nonzero post-KO). |
| `0xC370` | round_index? (u8) | 1 during round 1, 2 from the round-2 KO on; 2 on the continue screen. Semantics fuzzy — documented, not relied on. |
| `0xBD74` / `0xBD76` | **timer tens / ones digit** (u16 each, binary 0-9) | match the drawn countdown in every screenshot-paired read across two fights (99→90 traced tick-for-tick, incl. the 9x→8x borrow). READ verified; writes revert within ~1 s (see below). |
| `0xBC20` | timer_master (u16, derived) | ~`0xA1B4` at round start, drains ~1000/s, resets on round transitions. Writing it is overwritten from elsewhere; NOT authoritative. |
| `0xD396` | timer_frames (u16) | 1/frame countdown from ~4096 at round start. Accepts writes (keeps decrementing from the written value) but the drawn timer does NOT follow it. Some third store drives the digits. |
| `0xBCA0` / `0xBC88` | p1/p2 HUD health | see Fighter data above. |
| `0x1704D` | credits HUD digit (ASCII) | `'1'`→`'2'`→`'3'` byte-exact with the drawn CREDITS counter across coin presses. Display cell, read-only use. |

### The round timer — partially open

The visible countdown's *authoritative* store was not pinned down: digits
(`0xBD74/76`), master (`0xBC20`), and the frame counter (`0xD396`) were each
write-tested and each reverts/decouples; a fourth store (or a
tick-event chain) regenerates the digits. No byte anywhere in work RAM holds
the display value as one number (exhaustive sequence search over the
snapshot series, binary and BCD). Consequences:
- `timer_hold` enforcement is **not functional** — the profile carries a
  placeholder and this caveat.
- The timer digits are still perfectly good *reads* for overlays/features.

## The controllable gate

```
word_zero(screen_state)      # kills char select / ladder / bio screens
byte_zero(round_over)        # kills FINISH HIM / fatality / win / continue
health_in_range(1, 161)      # kills attract, title, demos, post-KO, menus
```

Checked against every labeled phase snapshot (46 total):

| phase | screen_state | round_over | healths in 1..161 | gate |
|---|---|---|---|---|
| live round (25+ samples, 3 matchups, rounds 1+2) | 0 | 0 | yes | **TRUE** |
| ROUND-N banner / FIGHT!! (~2 s, incl. health fill) | 0 | 0 | mid-fill: yes | TRUE (**known leak**, see below) |
| post-KO → fatality → "X WINS" | 0 | 2/3 | loser = 0 | false |
| continue countdown | 0 | 2 | P1 = 0 | false |
| char select / ladder / bios | 262 | 0 | garbage in range (11/133) | false (screen_state) |
| attract, title, fatality demo, game over | varies | varies | P1 health = 0 in all samples | false |

Known imperfections, stated honestly:
- **Pre-round banner leak (~2 s)**: all three conditions are already true
  during the ROUND-N banner while inputs are ignored. Same class of leak
  asurabld had; zero-input frames are filtered downstream by the recorder.
  No trimming flag was found (searched: nothing is nonzero across all
  three pre-round samples and zero across all live samples).
- **Timeout draws untested**: if a round times out with both healths > 0,
  gate correctness depends on `round_over` going nonzero at TIME UP —
  plausible (it fires on every KO-decided round end) but not observed.
- **Attract gameplay demos**: only a fatality demo was captured (healths
  read 0 there → gate false). If MK2's rotation includes a full HUD demo
  fight, the gate may be true during it; recordings filter demo rounds by
  zero P1 input, same as asurabld.

## Enforcement — what actually works

| lever | status |
|---|---|
| health refill | **WORKS** via `write_memory` — write 161 to all four health bytes (`0xC05E`, `0xC1D8`, `0xBCA0`, `0xBC88`). Verified live: bar refills, fight continues. Must be a periodic rewrite (freeze is a no-op on this core, see gotchas). |
| health_max | **161** (`0xA1`) — fill target observed at round start, MAME cheat's fill value, and the write-verified full-bar value. |
| timer hold | **NOT functional** — no authoritative store found (above). |
| credits top-up | **NOT possible via memory** — credits live in T-Unit CMOS (`0x01400000` bit range, handler-mapped), which is OUTSIDE the exposed region. Two coin presses produced exactly two byte deltas in work RAM: the ASCII HUD digit and a transient counter (`0x5E0C`, disproven — dropped 161→24 on a screen change). Coin-up works fine through input (`select`), which is how a training harness should do it. |

## Disproven / dead ends (don't re-chase these)

- `0xA260` (= MAME `maincpu.pb@1051300`, "Infinite Energy PL1"): that cheat
  line is **MK1's** (`mk.xml`); on MK2 it reads constant 0. The MK2 line is
  old-format `20BCA0` → `0xBCA0` (works, above).
- `0x5E0C` as credits: incremented 159→160→161 across two coins, then read
  24 after a screen change. Coincidence, not credits.
- `0xBA12`: ramps smoothly during walks AND at ~46/s while idle — some
  clock, not a position (write-test: no effect on anything visible).
- `0x6D3E`: monotone-increasing "position-like" — not camera (camera delta
  computed from world-vs-screen X is ~10x smaller), not a fighter.
- Fightcade's win counters `0xC36A`/`0xC36C` (`p1win`/`p2win`): stayed 0
  through TWO CPU round wins and a match end in 1P-vs-CPU play. Either
  2P-only or differently encoded; not adopted.
- `freeze`-based holds of any address: the freeze writer never lands on
  this core's fallback region (see gotchas). Use `write_memory` loops.
- Y candidates `0x6DE0`/`0xDFC8`/`0x33B4`: recomputed outputs (game value
  wins within a frame of any write).

## Controls (prior session, Wave W2)

`HP=y LP=b HK=x LK=a Block=l` — static proof from FBNeo's own
`retro_input.cpp` mk-driver special case, plus live pose screenshots for
HP/LP/HK. `Block 2` (r) intentionally unmodeled (redundant button; surfaces
via core descriptors). Coin=`select`, Start=`start`, both live-verified.

## What training-mode readiness still lacks

1. **Player Y** — the one missing fighter field (jump features blind).
2. **Authoritative round timer** — needed for `timer_hold`; reads work.
3. `src/training.rs` `resolve()` requires `health2`/`x`/`y` fields and
   `round_timer`/`round_state`/`credits`/`abort`/`match_end` globals to arm
   at all — MK2's map can't satisfy that contract yet (x lives at a
   different stride than the fighter blocks; credits aren't in the region),
   so full training enforcement no-ops by design. The RECORDER gate,
   however, is fully served by this profile.
4. Timeout-draw behavior of `round_over` (one boring 160-second probe).
5. Select-slot map for the remaining roster; boss-id confirmation.
