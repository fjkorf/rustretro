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

## Recording enablement (2026-08-27, consolidation follow-up)

`x` is now a GLOBAL-SOURCED fighter field (`p1_x` 0x6CBA / `p2_x` 0x6CFC,
RECORDER_V3 §2.5) — the recorder emits it as a normal per-block field, the
smaller-x anchor works, and the first arcade fit succeeded (family mk2,
port arcade, 7 features — no y). CORNER_PX/SCREEN_W removed from
calibration: arcade x is WORLD position (observed ~1100 vs screen 400), so
the corner feature is unusable until stage bounds are RE'd — same decision
as the Genesis port. Training-enforced smoke recording confirmed refill
works mid-ladder (the CPU opponent even advanced to char id 10 during it).

## Cross-port notes (2026-08-28, first transfer experiment)

- **goat-x-v1**: Genesis-trained (585 decisions, Reptile) restricted at fit
  time to the arcade-shared 7-feature subset (`--features`), deployed on
  arcade — runner acknowledged the cross-port run and re-encoded attack
  CLASSES into arcade chords (HK intent → arcade x/0x200 vs genesis l).
  Intents transfer; buttons re-encode per port.
- **Special-move encodings differ per port** (user observation, verified in
  the data): Reptile's slide is back+LK+HK on Genesis but back+LK+LP on
  arcade. Consequence today: multi-button specials label as a SINGLE attack
  class per the chord matcher (the arcade session showed 0 LK labels — the
  slides hid inside 47 "LP"s), and a transferred ghost cannot perform
  port-correct specials. This is the concrete motivating case for the
  long-queued MACRO-ACTION layer: specials as named intents in family.json
  with per-port input encodings, labeled and replayed as units.

## Hitstun / blockstun observables (2026-08-28, live grounding pass)

Session goal: find the signals a block-punish training dummy and a frame-data
lab need — "I was just struck" (hit or block) and, ideally, a general
animation/state-id field. Method: headless FBNeo on MCP port 4032, 1P
(Liu Kang) vs CPU (Baraka), a save-state checkpoint banked at a fresh
"ROUND 2, both full" frame (`/tmp/mk2_round2_full.state`, not committed —
regenerate via coin+start+select, see Method below), then fast MCP-polling
(200-1300 reads/sec achievable with small `read_memory` calls — no sleep
needed between them) through live combos, correlated against the frame when
`health` (`+0xE`) visibly drops. Two independent noise controls were used:
(a) a "fake hit times" pass over a pre-contact neutral window (rules out
ambient per-frame counters that trivially "differ before/after" any instant
regardless of combat), and (b) `stable_snapshot`/`static_diff` pairs
(neutral-vs-hit, no-block-vs-block) over a widened `0xB000-0xD800` window to
catch fields outside the narrow fighter struct.

### VERIFIED (live, correlated across two independent combo replays + a
chip-damage/blocking replay; passes the neutral-noise control)

| address | field | evidence |
|---|---|---|
| `0xD3FE` | **hit/combo counter** (u8, GLOBAL — not inside either fighter struct) | 0 at rest; increments by exactly 1 at the *same poll* `health` (P1, `0xC05E`) drops, for **every** hit in an 11-hit combo (both single pokes and the mid-combo launcher) and again for a run of **blocked/chip** hits (161→159→157→155, `0xD3FE` 0→1→2→3 in lock-step) — fires identically for hitstun and blockstun. Resets to 0 after a **~200-370 ms gap with no new hit** (12-22 frames @ ~55-60 fps) — closely matches the profile's existing `HITSTUN_RECENT_FRAMES: 20` calibration constant, which this finding now retroactively justifies. Zero false-fires: constant 0 across a 6-sample pre-contact neutral control (fake "hit" instants at t=0.15/.25/.35/.45/.55/.65s, all before the CPU's first real hit). |

This is exactly the asurabld-style "combo counter as hitstun proxy"
(CLAUDE.md: *"hitstun = counter changed within 20 frames"*) — a training
dummy/frame-data lab can use **`d3fe` incremented within the last ~20
frames → defender was just struck (hit OR block)**. Caveats, stated
honestly:
- **Does not distinguish hitstun from blockstun by value** — both increment
  the same counter the same way. Distinguishing needs either (a) the
  trainer's own knowledge of whether Block was held (it drives the dummy,
  so it already knows), or (b) more RE (see Open below).
- **Per-player scope untested.** Every sample this session was "P1 gets hit
  by CPU Baraka" — P1's `health` dropping is what triggered every read. I
  was unable to reliably land clean hits FROM P1 onto P2 to check the
  symmetric case (see Toolkit friction below), so whether `0xD3FE` is a
  P1-specific/P2-specific/shared-whoever-just-got-hit register is open. In
  a 1v1 match only one side can be mid-combo at a time, so "global" would
  still be a usable signal in practice, just not per-player-attributable
  from this byte alone.

### LIKELY (single-mechanism correlation, not yet cross-verified)

| address | field | evidence |
|---|---|---|
| `0xC050`+`0xC` / `+0xD` (P1 struct, u16 LE; presumably `0xC1CA`+`0xC`/`+0xD` for P2) | **airborne/juggle latch**, value `0xFFFC` (-4) while active, `0` at rest | 0 through the first 4 hits of an 11-hit combo, then latches to `0xFFFC` for ~366 ms starting the instant the 5th hit (mid-combo launcher) lands, clears back to 0, and repeats on the next launcher later in the same combo (2 independent activations, same replay). Does **NOT** fire on the ground-poke hits before/after the launcher — fails "must fire on every hit," so it is NOT the general hitstun signal, but is a plausible knockup/juggle-substate indicator (a small constant Y-nudge or "airborne" flag) worth chasing as a lead on the still-missing player-Y field. |

### NOT FOUND — searched, honestly open

- **Block-stance flag** (item 3): no live boolean found. Two lines of
  attack failed for different reasons: (a) offsets `0xBE4F/51/5A/62/63/6B/6C`
  etc. initially looked promising from a single before/after block-vs-noblock
  diff, but a longer time-series showed they **cycle continuously on their
  own** (an idle-animation loop, ~150-500 ms period) whether or not Block is
  held — a single-sample A/B comparison caught a phase coincidence, not a
  real flag; this is the exact "toggle-intersect catches the rendered echo"
  trap the re-probe SKILL warns about, just with an idle cycle standing in
  for the echo. (b) Inside P1's own struct, `+0x18/+0x1C/+0x1E` (`0xC068`,
  `0xC06C`, `0xC06D`) DO change the instant Block is first pressed, but they
  are **monotonic counters / one-shot latches** (`+0x18` steps up by 32 once
  per *press event*, `+0x1C`/`+0x1E` set once and never clear) — not a live
  "currently blocking" boolean. No candidate survived a press/hold/release
  cycle test.
- **General action/state-id word** (item 4): P1's entire struct (`0xC050`,
  the full `0x17A`-byte stride) was diffed neutral-vs-combo byte-for-byte —
  the ONLY bytes that ever move are `+0x0` (char_id), `+0xE` (health), the
  `+0xC/+0xD` juggle latch above, and the one-shot `+0x18/+0x1C/+0x1E` block
  counters. No animation/pose-id byte cycling through recognizable
  stand/walk/attack/hitstun/block values was found anywhere in the struct.
  This is a genuine negative result, not a gap in search effort — MK2's
  arcade fighter struct is far sparser than asurabld's; the state machine
  driving animation appears to live entirely outside this struct (object
  pool, T-Unit "gfx object" table, or CPU-local variables never DMA'd to a
  fixed RAM slot), consistent with x/facing already being GLOBAL-sourced
  rather than struct fields (see Positions, above).
- **Genesis cross-check** (item 5 / bonus): NOT ATTEMPTED. The session's time
  budget went entirely into arcade-side toolkit friction (below); this is an
  open follow-up, not a "checked, no result."
- **External MAME-cheat/community sweep**: one search + a few fetches
  (mamecheat.co.uk blocks non-browser fetches with 403; mortalkombatonline.com
  and a MAME-cheat-forum snippet were reachable). Found only the ALREADY-KNOWN
  char-id table (`0x0020C050`/`0x0020C1CA` old-format, matches `+0x0` above)
  and one unverified lead — `user1.mw@06B70` "Hit Anywhere Both Players" —
  whose `user1`/`mw` MAME region-and-width tag does NOT match the
  `maincpu`-bit-address convention this doc's conversion formula assumes, so
  it was NOT converted/trusted; flagging as an external candidate with
  provenance only, for a future session to map properly. No community
  address for "stun"/"hitstun"/"combo counter"/"attack state" turned up —
  unsurprising, since MAME cheat databases skew toward infinite-health/time,
  not frame-data internals.

### Toolkit friction (`shadow_train.re`, first real field test)

- **`Probe.press()` is fine; reading world X (`0x6CBA`) to verify it is
  NOT.** Across this entire fresh-boot session, `0x6CBA` (P1 world X, per
  the existing profile write-verified in a PRIOR session) read a frozen
  value (214) through repeated `left`/`right` holds that **visibly moved
  Liu Kang on screen** (screenshot-confirmed retreat), then spontaneously
  jumped to 546 in lockstep with an unrelated knockback. This matches the
  profile's own caution that `x` lives in a dynamic `0x42`-stride object
  pool rather than a fixed per-player slot — apparently the pool slot
  backing "P1" is not stable across boots/matches. Net effect: **do not
  trust `p1_x`/`p2_x` reads as a liveness check for input** — use a visible
  screenshot or a struct field (health, char_id) instead. This is a
  real risk for anything in the codebase that reads `p1_x`/`p2_x` for
  training/recording purposes; worth a follow-up session to find a
  stable-slot alternative or a slot-resolution indirection.
- **`press_buttons` needs `resume()` called first, obviously, but ALSO
  needs real wall-clock time to elapse AFTER the call returns** for the
  held frames to actually tick (the headless loop consumes the hold
  counter once per emulated frame, independent of the calling thread) —
  undocumented gotcha the first few attempts tripped on by reading
  immediately after `press()` with no `sleep`.
  Related: launching with `--pace 0` (uncapped) for "fast-forward to phase
  X" is exactly right for boot/menus, but is actively dangerous once a CPU
  opponent is live — a match can start and finish (KO) inside a single
  `sleep(0.5)` because uncapped means *far* more than 0.5 s of emulated
  time elapses. Switch to `--pace 1` before any live-fight interaction.
- **No `reset`/`restart` MCP tool** — the only way to get back to a clean
  boot (e.g., to re-test 2P-join semantics) is to kill and relaunch the
  headless process. Fine once known, but cost one relaunch cycle
  discovering it.
- Small **`read_memory` calls are cheap enough to poll at 200-1300 Hz**
  (no chunking needed under a few hundred bytes), which is what actually
  cracked this session open — far better temporal resolution than the
  `stable_snapshot`/1-second-apart default suggests. Worth calling out in
  the SKILL: for hitstun/frame-data-style work, prefer many small
  `read_memory` polls over periodic wide `read_region` snapshots; reserve
  the wide snapshot for an initial *candidate-narrowing* pass (its 2-sample
  `static_diff` is still exactly right for that), then confirm/characterize
  candidates with tight per-address polling.
- **2-human join (`start` on port 1) did not reliably convert P2 to human**
  in these attempts — the select screen locked in a 1P-vs-CPU match despite
  the join press (P2's HUD then intermittently showed "INSERT COIN" mid-fight,
  suggesting a late/second join attempt was queued and partially processed).
  Consequence: all data above is from a CPU defender, not a controllable
  one; the prompt's suggested "human defender holds Block" isolation
  protocol was not achieved this session. `mk2.md`'s existing gotcha ("P2
  joining mid-1P-game aborts the current match...") likely explains part of
  this; the clean 2P-from-boot path (coin x2 before either `start`) needs
  more careful sequencing than attempted here.

### Updated: What training-mode readiness still lacks

6. **Block-stance flag and a general animation/state-id field** — both
   searched for directly this session (see above), neither found. The
   combo counter (`0xD3FE`) covers "was just struck" adequately for a
   punish trainer; a true frame-data lab (startup/recovery measurement)
   still has no state-id field to key off.

## Arena recapture: `reptile-vs-reptile.state` (2026-08-28, A-RE)

The committed `shadow/arenas/mk2/reptile-vs-reptile.state` was an attract
demo (input-dead) prior to this session; recaptured as a real, INPUT-LIVE
1P-vs-CPU fight. Headless FBNeo, port 4032, `--game library/mk2 --pace 1`.

**Coin/char-select flow used** (fresh boot each time this session, since a
cold boot shows a one-time "CMOS INVALID — FACTORY SETTINGS RESTORED"
screen that eats the first button press): any button past the CMOS screen
→ `select` (coin) to skip the attract story → `select` again (2nd coin,
"2 CREDITS TO START") → `start`. Landed on the "CHOOSE YOUR FIGHTER" grid
with the cursor on Liu Kang (slot 0), confirming the existing "P1 default =
Liu Kang" doc fact. **3× `right` reaches Reptile** (row 1, col 4) — verified
both visually (portrait highlight + the animated preview model turning into
a green ninja) and by memory read (`0xC050` = `9` at the cursor, matching
the roster table). A `start` press locks P1's pick; this needed 1-6 retries
across different session attempts (the press did not always land the very
first time — no distinguishing symptom found, just poll `0xC1CA` (P2
char id) until it leaves `255` to confirm the lock actually took before
moving on, rather than trusting a single `start` press blindly).

**Full-health capture, and a real trap worth recording**: health fills from
0 in the "ROUND 1"/"FIGHT!!" banner window (already documented above), but
polling for "P1 health `0xC05E` >= 161" as the sole "fight is ready" signal
is **not sufficient** — a fast tight poll can catch a **transient overshoot**
to 161 seconds before the real, stable full-health frame (observed
repeatedly: detection at 161, then a settle read moments later showing
40-70, i.e. the fill was still climbing through its documented `0→38→...`
curve and the detector caught a one-frame glitch on the way up, not the
final value). Fix: after first seeing `hp1>=161 and hp2>=161`, sleep a
fixed ~2.2 s (comfortably past the documented ~1.5 s fill) and re-read
before trusting it; only save once health is STILL 161/161 and
`round_over==0` after that settle.

**Input-liveness verification** (the mission-critical check — the prior
committed state had none): `0x6CBA` (P1 world X) can appear **frozen**
across a short read window even with input genuinely live and landing — not
because the address is wrong (it is the same write-verified store from the
Positions section above), but because MK2's CPU AI is aggressive enough
that a short verification window has a real chance of landing entirely
inside **hitstun or a knockdown** (screenshot-confirmed: Reptile flat on the
ground, immovable by design, exactly when a 1-2 sample x-read looked
"stuck"). The reliable test, done both immediately before saving and again
against the saved file after a reload: refill both fighters' health every
iteration (removes the KO/long-hitstun confound) and hold alternating
`left`/`right` bursts for ~2-3 seconds while sampling `0x6CBA` — a genuinely
live P1 traces a real path (`390→452→426→401→391→196...` observed one run),
where a truly dead/wrong-slot address would stay bit-for-bit constant
across the whole window regardless of hitstun timing. Do not conclude
"input is dead" from 1-2 samples with this game; sample across several
seconds with health forced up.

**Final captured state**: Reptile (P1, char id 9) vs Scorpion (P2, char id
10, CPU-picked), both healths **161/161** (`0xC05E`/`0xC1D8`, plus the
secondary pair `0xBCA0`/`0xBC88` also topped off), `round_over` (`0xC360`)
`== 0`, screenshot-confirmed both health bars full green, ROUND 1 in
progress. Input-liveness re-confirmed on a fresh reload of the saved file
using the multi-sample method above. Saved via `save_state` over
`shadow/arenas/mk2/reptile-vs-reptile.state` (2,447,284 bytes, matching the
existing file's format/size).

## Macro-action encodings — live verification (2026-08-28, A-Rust, port 4033)

Rig: headless 2-HUMAN match (P1 Liu Kang port 0, P2 Reptile port 1 — joined
by `start` on port 1 during a live 1P fight, then both re-select; P2's
default cursor is Reptile), state-banked and reloaded per trial. The
BlockPunish dummy (port 1) provided the executor path: P1 jabs the guarding
dummy → chip → `p2_health_hud` change → punish macro plays.

| move | encoding (arcade) | verdict | evidence |
|---|---|---|---|
| slide | `back + LK+LP+Block`, 8f | **VERIFIED** (chord corrected) | The contract's back+LK+LP produced a NORMAL (pose screenshot); adding Block: slide pose + h1 161→148 (−13), 3/3 trials from round-start range. **Point-blank caveat**: the LP-bearing chord resolves to a close normal/throw (the §1 proximity phenomenon) — a point-blank punish slide whiffs by game rule. |
| acid_spit | `F` · `F+HP`, 3f steps | **VERIFIED** | Via the punish executor at point blank: h1 161→137 (−24), repeated across two runs; full mask trace in the session recording (`0x40`×3, 2f gap, `0x42`×4) with `p2_special:"acid_spit"` annotated. |
| force_ball | `B` · `B+HP+LP`, 3f steps | **VERIFIED** | Same path: h1 161→145 (−16), repeated; trace `0x80`×3, gap, `0x83`×4, annotated. |

Punish-timing findings (now constants in `src/training.rs`):
- **Inputs played into hit-freeze are eaten.** The macro must start
  ~26 frames after the contact (hit-freeze ≈10 + jab blockstun ≈14): a
  chord at +16 was swallowed while a motion whose chord lands at +21 came
  out.
- **Held Block bleeds into the chord.** The dummy's guard must be fully
  released ~4 frames before the first step or the game stays in block
  stance and ignores attack buttons (slide fires from a clean simultaneous
  press ≥4f after a Block release — release recovery itself is not the
  issue).

Contact-signal correction: `hit_counter 0xD3FE` **did not move** for hits
ON P2 — blocked (chip −6) or clean — in this 2-human rig; every prior
observation had P1 as the victim. It is a P1-victim counter at best (it
also stayed 0 when Reptile's slide/projectiles struck P1 here — possibly
1P-mode-only). The per-victim contact signal is the HUD damage pair
`0xBCA0`/`0xBC88` (`hitstun_sources` in the profile); caveat: the training
refill rewrites those bytes, so one spurious punish per refill is possible.

**New gate leak (needs an A-RE pass):** in a 2-human (challenger) match,
`screen_state 0xC37E` flips to **276** at the first contact and stays there
for the rest of the round while the fight visibly continues and accepts
input — `word_zero(screen_state)` reads not-controllable from that moment.
All 46 prior gate snapshots were 1P/attract phases; 276 never appeared
there. Effects: the recorder under-counts controllable frames in 2P
rounds, and round summaries for such rounds close early.

## Gate revision (SUPERSEDED — see the masked revision below): word_in for the 2-human screen_state leak (2026-08-28)

Live finding (A-Rust's 2-human punish rig): `screen_state` flips to **276**
(0x114) at first contact in a 2-HUMAN match and holds for the rest of the
round while the fight continues normally — `word_zero` then reads
not-controllable, which froze the recorder, training enforcement, and ALL
dummy injection (user-visible as "block-punish works once, then the dummy
goes limp" once the punish's gate-grace expired). New gate vocabulary
condition `word_in` (u16 ∈ values); arcade gate now
`word_in(screen_state, [0, 276])`. Caveat recorded honestly: 276 has only
been observed in 2-human-fight-after-contact; if it ever appears on a menu
this gate leaks there — no such observation across all phase sweeps to
date (menus read 0x9C01-family values).

## Gate revision 2: screen_state is a BITFIELD — word_masked_zero (2026-08-28)

The `word_in [0, 276]` allowlist above was whack-a-mole and broke on the
THIRD observed value. User QA (Reptile vs Reptile, 2-human) read
**260** (0x104) while the earlier smoke rig read **276** (0x114) — the
gate closed, and since ALL dummy injection, refill, timer hold, and
recorder capture sit behind the gate, the block-punish dummy went limp
after one punish (diagnosed live via MCP on the paused session:
`gate=false`, `screen_state=260`, dummy mode still block_punish).

Every value ever recorded, in binary:

| value | hex | bit 1 | phase |
|---|---|---|---|
| 0 | 0x000 | clear | 1P fight, post-KO |
| 259 | 0x103 | SET | attract |
| 260 | 0x104 | clear | 2-human fight (user QA) |
| 262 | 0x106 | SET | char select / ladder / bios |
| 263 | 0x107 | SET | attract |
| 276 | 0x114 | clear | 2-human fight (smoke rig) |

**Rule: bit 0x02 SET = not in a fight**; the 0x100 bit is set by 2-human
play and the other low bits vary within a match. New gate vocabulary
condition `word_masked_zero {global, mask}` (u16 & mask == 0); arcade
gate uses mask `0x2`. Live-verified 6/6 against the table above, plus a
regression test in `src/gate.rs`.

This also explains the user's "only a CLOSE HIGH PUNCH revives the
dummy" observation: under the old allowlist 260 was excluded but 276 was
allowed, and 260→276 differ by exactly bit 4 (0x10) — the close elbow's
reaction flips that bit, momentarily re-opening the gate; far HP and
knockdowns don't set it. Not a proximity-semantics effect in our code at
all. What sets bit 4 remains unmapped (harmless under the masked rule).
