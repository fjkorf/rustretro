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
| acid_spit | `F` · `F+HP`, 3f steps | ~~VERIFIED~~ **RETRACTED 2026-08-30** — see "Special-move encodings, live-audited" below. This chord produces NO special (0 damage, 16/16 configurations at a range where only a projectile can reach); the −24 measured here is Reptile's CLOSE HP NORMAL, which does exactly 24 at 62 px in `arcade.frames.json`. Shipped encoding is now `F` · `F` · `HP`. | Via the punish executor at point blank: h1 161→137 (−24), repeated across two runs; full mask trace in the session recording (`0x40`×3, 2f gap, `0x42`×4) with `p2_special:"acid_spit"` annotated. **The trace is real; the attribution was not** — a point-blank trial cannot tell a projectile from a normal. |
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

## Gate revision 2 (SUPERSEDED by revision 3 below): single-bit mask (2026-08-28)

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

## `action_counter` — an ACTION counter, NOT the contact signal (2026-08-28, twin-counter hunt; conclusion CORRECTED below)

**VERIFIED (live, user's 2-human Reptile-mirror session).** Fighter field
**`+0xC0`** (P1 `0xC110`, P2 `0xC28A`), u8, increments by **+32** (a count
in the high bits) each time that fighter starts a new action — its own
swing, OR a reaction to being struck.

Why it matters: it is the per-victim contact signal BlockPunish needed.
Decisive test — hold Block on the defender continuously, let it settle,
then attack mid-hold, 4/4 rounds:

| settled | after 0.35 s idle-blocking | after the attack |
|---|---|---|
| 112 | 112 (quiet) | 144 **fired** |
| 144 | 144 (quiet) | 176 **fired** |
| 176 | 176 (quiet) | 208 **fired** |
| 208 | 208 (quiet) | 240 **fired** |

It fires on blocked contact that deals **zero chip** (observed 123→123
and 134→134 health), which the previous health-delta trigger
(`hitstun_sources`) cannot see — that was the cause of the user-reported
"the dummy punishes some hits but not others". It is quiet while the
fighter merely holds guard, so it does not false-fire in neutral.

Also mapped on the way (attacker side, same field): the counter fires on
every swing INCLUDING whiffs — so it is an action counter, not a hit
counter. Useful corollary discovered by the whiff control: **a fighter's
struct is entirely static when untouched** (0 of 0x17A bytes change), so
any change in an idle fighter's struct means contact.

Profile: `action_counter` added as a fighter field (+0xC0) and
`contact_signal: {"field": "action_counter"}`; `contact_signal` now takes
PRECEDENCE over `hitstun_sources` for the punish trigger, and gains a
per-fighter `field` variant alongside the old shared `global` (MK2's
`hit_counter` 0xD3FE remains disproven for this use — P1-victim only).
`hitstun_sources` stays as the health-delta fallback and as the hitstun
FEATURE source (where "took damage" is the correct meaning).

Rig note: `shadow/arenas/mk2/reptile-vs-reptile.state` is 1P-vs-CPU, so
injected dummy input CANNOT drive P2 there — BlockPunish end-to-end
testing requires a 2-human match (controller 2 joins).

## CORRECTION: the contact signal is the health delta after all (2026-08-28)

The section above over-claimed from a rig where the defender was struck
shortly after a FRESH block press. Re-tested in the configuration that
actually matters — the training dummy holding guard CONTINUOUSLY — the
result reverses:

- The dummy's `action_counter` (+0xC0) moved on only **1 of 4** blocked
  contacts. It fires when a fighter ENTERS an action (including entering
  block), not when an already-blocking fighter is struck.
- Full-struct diff while guarding, 6 trials: **idle churn = 0 bytes** (a
  blocking MK2 fighter's whole 0x17A struct is frozen), and the ONLY byte
  that changed on blocked contact was **`block+0xE` — health itself**,
  5 of 5.
- Every blocked contact in these trials chipped (−3 or −6). The one
  "no change" trial was a WHIFF (no damage, no struct change at all).

So for MK2 arcade the health delta (`hitstun_sources`, the HUD pair) IS
the contact event, and the earlier "zero-chip blocked contact" reading
was a fresh-block-press artifact. `contact_signal` was removed from the
arcade profile; the trigger uses the hitstun_sources fallback.

**Consequence for the user-reported "punishes some hits but not others":
the likely causes are WHIFFS (which correctly produce no punish — note
MK2's proximity normals mean a far attack can whiff where a close one
connects) and the post-punish window (≈1 s of delay + macro + recovery
during which the dummy is not guarding).** Not a signal bug.

`action_counter` is KEPT as a recorded fighter field — it is honest,
useful data (action transitions, incl. the attacker's swings and whiffs)
and costs nothing. The `contact_signal` schema keeps its per-fighter
`field` variant for games that do have a true contact counter.

## Gate revision 3: bits 1 AND 2 TOGETHER mark a menu — word_masked_not_all (2026-08-28)

Revision 2's single-bit rule (0x02 set = not in a fight) fit six values and
was broken by the seventh: the user's live 2-human fight read **259**
(0x103) — which HAS bit 1 set, and which the original RE had recorded as an
attract value. The gate closed mid-fight; the dummy stopped guarding and
both fighters stood still (the user-visible "punish: slide never came out"
freeze, made worse by a stale phase label — see below).

Every screen_state value observed to date:

| value | hex | &0x6 | phase |
|---|---|---|---|
| 0 | 0x000 | 0 | 1P fight, post-KO (also seen on attract) |
| 257 | 0x101 | 0 | 2-human fight |
| 259 | 0x103 | 2 | **2-human fight (live)** — also recorded on attract |
| 260 | 0x104 | 4 | 2-human fight |
| 262 | 0x106 | **6** | char select / ladder / bios |
| 263 | 0x107 | **6** | attract |
| 276 | 0x114 | 4 | 2-human fight |

**Rule: bits 1 and 2 BOTH set (mask 0x06) = not in a fight.** New gate
condition `word_masked_not_all {global, mask}` — `v & mask != mask`;
replaces `word_masked_zero` (which no game used once this landed, and the
vocabulary stays small with every member live-verified). Verified 7/7 live
plus a regression test.

**Honest limits, third time asking:** 0 and 259 have BOTH been observed
in fights and on attract screens, so screen_state cannot fully separate
them alone — the gate's `health_in_range` + `round_over` conditions carry
the rest, and an attract-demo leak is possible (harmless: demo rounds are
dropped at fit time by their zero p1_input). Char select, the leak that
made this global worth gating on at all, still closes correctly (262).
If an eighth value appears, prefer finding a DIFFERENT discriminator over
a fourth revision of this mask.

**Also fixed here:** while the gate is closed, BlockPunish's phase string
now reads "gate closed — not in a fight" instead of freezing on the stale
"punishing: <move>" label. The old behaviour actively misled diagnosis —
the mode was not running at all. (The in-flight punish grace was working
correctly the whole time; only the label lied.)

## Ripping character cels from VRAM (2026-08-29)

**The finding: MK2 sprites can be extracted, in color and with clean alpha,
from the ONE exposed RAM region — no graphics-ROM decoding required.**

`get_state` reports `has_vram: false, has_rom: false` and warns that
"sprite/ROM provenance [is] unavailable on this core". That is true about what
libretro *labels*; it is not true about what the blob *contains*. FBNeo's
`MemIndex()` puts three video structures inside `AllRam..RamEnd`:

| blob offset | size | contents |
|---|---|---|
| `0x102100` | `0x20000` | `DrvPalette` — xRGB1555, little-endian, 2 B/color |
| `0x122100` | `0x20000` | `DrvPaletteB` — the same colors pre-converted to host format |
| `0x142100` | `0x100000` | `DrvVRAM` — the framebuffer, 512 words per row |

### Why the pixels come pre-segmented

`TUnitDmaWrite` (midtunit.cpp:469) writes every blitter pixel as
`pixel | (DMA_PALETTE & 0xff) << 8`, and `ScanlineRender` (:547) displays
`DrvPaletteB[word & 0x7FFF]`. So a VRAM word is `palette_bank<<8 | pixel`, and
because the blitter SKIPS transparent pixels instead of writing them, masking
on the high byte yields exactly one sprite's opaque pixels — alpha included,
for free. In a live fight each fighter owns a bank (0x02 / 0x03 in the matches
observed) while scenery layers own others.

### Verified live

Reconstructing a paused frame from `DrvVRAM` + `DrvPalette` and diffing it
against `app://screen` scores **99.6%** pixel agreement. Masking bank 0x02 vs
0x03 cleanly separates P1 from P2; bank 0x02's bbox began at x=159 while
`p1_screen_x` (`0x3E40`) read 162.

Three traps cost real time, all now handled in `scripts/re/rip_mk2_cels.py`:

1. **Color expansion must be `v << 3`, not `v * 255 / 31`.** FBNeo's
   `RGB555_2_888` masks with `0xF8` (:86), `BurnHighCol` packs to RGB565, and
   rustretro's `decode_to_rgba` re-expands `r5<<3 / g6<<2 / b5<<3`
   (src/debug/mod.rs:1333) — green survives as `(v<<1)<<2 == v<<3`. The chain
   is the identity on `v<<3`, so ripped pixels compare EQUAL to a screenshot.
   The `*255/31` rounding differs by ±1 on most colors and drops exact
   agreement to ~1%, which reads exactly like a broken layout.
2. **The display origin is NOT always (0, 0).** `ScanlineRender` starts at
   `DrvVRAM16[(rowaddr << 9) & 0x3FE00]`, column `coladdr << 1` — TMS34010
   display registers, i.e. CPU state OUTSIDE the exposed region. Measured
   (0, 0) in one fight and **(0, 56)** on the same instance later. It cannot be
   read, so it must be SEARCHED: reduce a screen row and every VRAM row to a
   one-byte-per-pixel signature and `bytes.find` the screen row inside the
   doubled VRAM row (doubling covers the `& 0x1FF` column wrap). Ripping at an
   assumed origin silently produces shifted garbage.
3. **Alignment must be measured PAUSED.** Running, the chunked VRAM read and
   the screen grab are different instants and score near zero regardless of
   correctness. The same tearing corrupts unpaused rips, which is why
   `--watch` mode defaults to `--min-seen 2`: a torn read rarely repeats.

Straight after a `load_state` the framebuffer and VRAM can disagree enough
that no probe row matches; one `step` re-syncs them (the MCP `step` tool takes
NO arguments — it advances exactly one frame per call).

### Limits

A cel is only ever captured AS DRAWN. Foreground scenery punches holes —
Reptile ripped in front of the Dead Pool chains comes out with dashed vertical
gaps — and poses the game never displays are simply absent. The complete asset
set still requires decoding the twelve `*-vid` ROMs (4-way byte-interleaved
into `DrvGfxROM`, midtunit.cpp:346) per `midtunit_dma.h`.

### Roster addition

`char_id 10 = scorpion`, added to family.json: block2 `+0x0` read `0x0A` while
the health bar drew "SCORPION" (screenshot) — the same standard as the other
verified roster entries.
### Roster: Mileena = char_id 5 (live, 2026-08-29)

Read from the user's own paused session (MCP 4025) at the character-select
screen with P1 on Mileena and P2 on Reptile:

| addr | value | reading |
|---|---|---|
| `0xC050` (block1 `+0x0`) | `0x05` | **Mileena = 5** |
| `0xC1CA` (block2 `+0x0`) | `0x09` | Reptile = 9 — MATCHES the existing roster |

The Reptile agreement is the corroboration: the same read that yields 5 for
the unknown character independently reproduces a known id for the other
slot, so the bytes reflect the SELECTED characters and not stale round data.
Confidence **VERIFIED** — promoted the same day. The promotion criterion
stated here was "confirm the id persists into the fight", and the user's own
arena capture supplied it: `shadow/arenas/mk2/m-v-r.meta.json` records
`char_id_block1: 5, char_id_block2: 9, gate_open: true`, both fighters at
full health with `inputs_live` on both ports — i.e. the app's own
profile-driven capture read Mileena as 5 during a live fight, independently
of the character-select read. `select_slot` still unmapped for her.

**Johnny Cage = char_id 2** (same method, same session, P1 on Johnny Cage
with P2 unselected). `0xFF` is the NO-SELECTION sentinel — block2 read `0xFF`
while P2's cursor was not confirmed, which is a useful liveness check in its
own right: a char_id of 255 means "nobody chosen yet", not a character.

**Sub-Zero = char_id 8** (same method, P1 on Sub-Zero, P2 unselected =
`0xFF`). This read also VALIDATES the 1:1 gap argument below: the prediction
was that the three unmapped characters must occupy exactly ids 6, 8 and 10,
and the read landed on 8 — a value the prediction could not force. A stale
or unrelated byte had ~9 chances in 12 of falsifying it.

**Shang Tsung = char_id 6** (same method). Eleven of twelve now read
directly: 0 kunglao, 1 liukang, 2 johnnycage, 3 baraka, 4 kitana, 5 mileena,
6 shangtsung, 7 raiden, 8 subzero, 9 reptile, 11 jax.

**Scorpion = char_id 10 — READ, not deduced.** It was briefly shipped as a
deduction (only unclaimed id, only unmapped character) and then confirmed by
direct read minutes later. The full selectable roster is now 12/12 measured.

**The elimination argument that produced it was UNSOUND, and the read is the
only reason it landed.** MK2 has two boss characters, Kintaro and Shao Kahn,
which are not selectable and were therefore invisible to the "12 selectable
fighters must fill ids 0-11" premise. Bosses must occupy ids somewhere; had
one taken a low id, a selectable character would have sat above 11 and the
deduction would have been confidently wrong. **Do not run this style of
elimination argument on a roster again without first establishing the full
id space, bosses included.** (Same failure shape as the `action_counter`
over-claim: a clean-looking inference from an incomplete control set.)

**Open: boss ids are unmapped.** Read `block2+0x0` during a 1P-ladder
Kintaro or Shao Kahn fight. They are wanted for the Matchup panel's
force-matchup buttons, which already special-case bosses on asurabld. MK2 has 12 selectable fighters, so
ids 0-11 are the full selectable set and no id is spare. Three char-select
reads of `block1+0x0` close it; do NOT guess the assignment, the select
grid order does not match id order (kunglao=0 sits at select_slot 1, while
liukang=1 sits at slot 0).

**`screen_state` reads 260 (`0x0104`) at CHARACTER SELECT.** 260 is pinned in
`src/gate.rs`'s regression test as an in-fight value (observed live in a
2-human fight), so `screen_state` alone does NOT discriminate phase — the
combination does. The gate correctly stayed CLOSED here on the other two
conditions (`round_over` = 1, block2 health = 0). This is evidence FOR the
bitfield reading (display/2P-ness bits, not a phase enum) and against ever
adding another screen_state value to the enum; do not "fix" the mask.

## Stable per-fighter position: the object POINTER at `block-0x0C` (2026-08-29, P1)

**RESULT: acceptable-outcome #2 — a stable indirection.** The fighter struct
carries a pointer to that fighter's entry in the `0x42`-stride object pool.
Follow the pointer and you get x, y and a char_id cross-check for the right
fighter, on every boot, through round resets and through pool-slot moves.
Session: headless FBNeo on MCP port **4030** (the user's 4025 was never
touched), rig `shadow/arenas/mk2/r-v-r.state` (2-human Reptile mirror) plus
three from-scratch cold boots.

### The finding

| what | address | form |
|---|---|---|
| **P1 object pointer** | **`0xC044`** = `block1 - 0x0C` | u32 LE, a TMS34010 **bit address** |
| **P2 object pointer** | **`0xC1BE`** = `block2 - 0x0C` | ditto (`0xC1BE - 0xC044 = 0x17A`, exactly the fighter stride) |

```
obj  = (u32_le(block - 0x0C) - 0x01000000) >> 3     # the doc's master formula
x    = u16_le(obj + 0x12)      # world X, 1 unit = 1 pixel
y    = s16_le(obj + 0x16)      # SIGNED; resting ~83-89, smaller = higher
cid  =  u8   (obj + 0x3E)      # char id — must equal block+0x0 (cross-check)
link =  u32  (obj + 0x00)      # pointer to another pool entry (display list)
```

Object stride is `0x42`, as the old notes said — the pool is real, only the
*slot* was never stable. This also retro-explains two old entries:
`0x6CBA - 0x12 = 0x6CA8` and `0x6CFC - 0x12 = 0x6CEA = 0x6CA8 + 0x42`, i.e.
the write-verified `p1_x`/`p2_x` of the original session were exactly this
same `obj+0x12` — for the two objects that happened to sit at `0x6CA8` /
`0x6CEA` **that run**. And the "disproven" `0x6D3E` ("monotone-increasing
position-like — not camera, not a fighter") is `0x6D2C + 0x12`; `0x6D2C` is
a base this session actually observed P1 occupying. It WAS a fighter's x.
Nothing was wrong with those addresses except the assumption that a slot
belongs to a player.

### Per-criterion results (all five)

**1. Monotone under a held direction — PASS.** `hold_buttons` / 30-ish frames
per direction, resolved x sampled every ~0.16 s. 2-human rig, Reptile mirror:

| hold | P1 x | P2 x |
|---|---|---|
| port 0 `right` | 469 → 491 → 516 → 541 → 569 | (constant 661) |
| port 0 `left` | 569 → 553 → 531 → 511 → 491 | (constant 661) |
| port 1 `right` | (constant 489) | 661 → 679 → 699 → 719 → 739 |
| port 1 `left` | (constant 489) | 739 → 718 → 691 → 666 → 641 |

Cold boot, 1P Liu Kang vs CPU Scorpion, no state loaded: `right`
469 → 484 → 504 → 524 → 541; `left` (after retreating out of the CPU's
range first) 755 → 746 → 729 → 709 → 703. Walking into the far corner
bottoms out at x = 357 with the fighter visibly against the wall
(screenshot), and 357 vs the opponent's 661 = a 304-unit gap that measures
~300 px on the 400-px screen — **x is in pixels, 1:1**, which is exactly
what the pixel-gap keys of `docs/frames.md` need.

**2. Correct fighter — PASS.** See the table above: eight samples of P1
motion left P2's resolved x bit-identical, and eight samples of P2 motion
left P1's untouched. Independently, `obj+0x3E` matched `block+0x0` in every
matchup observed across three boots: 9/9 (Reptile mirror), 1/3 (Liu Kang vs
Baraka), 1/10 (vs Scorpion), 1/9 (vs Reptile).

**3. Survives a round reset — PASS.** P2 KO'd by writing 0 to `0xC1D8` +
`0xBC88`; waited out FINISH HIM / WINS / ROUND 2. After the reset
`round_num 0xC35E` = 2, both healths back to 161, `round_over` back to 0 —
and both pointers still read `0x69D2` / `0x6A14` with x back at the
round-start 469 / 661. (Meanwhile `0x6CBA`/`0x6CFC` read 214 / 232.)

**4. Survives a fresh boot — PASS, including a live slot MOVE.** Three cold
launches (process killed between each), each driven from the CMOS screen
through coin/coin/start/select into a real fight. The pointer resolved
correctly every time. Better than that: within one boot, after the first
match ended and a second began on a different stage, the pool slot **moved
from `0x69D2` to `0x6D2C`** (Δ `0x35A` = 21 × `0x42`) and the pointers at
`0xC044`/`0xC1BE` had already followed — resolved x/char stayed correct
(Liu Kang 1201, Reptile 1085) while the fixed addresses went to garbage.
That is the whole point of the indirection, observed happening.

**5. Disagrees with `0x6CBA` when that slot is stale — PASS, and the stale
condition was easy to reproduce (three independent ways).**

| situation | `0x6CBA` / `0x6CFC` | resolved truth |
|---|---|---|
| `r-v-r.state`, both fighters walked visibly in both directions on both ports | **frozen 215 / 233** through every sample | 469→569 / 661→739 |
| cold boot, Liu Kang vs Baraka | 546 / 241 | 409 / 478 |
| …a few seconds later, same match | 240 / 546 (**the two swapped**) | — |
| cold boot, 2nd match, Liu Kang vs Reptile | 1312 / 34004 (garbage) | 1201 / 1085 |

The swap is the tell: `0x6CBA`/`0x6CFC` are not broken addresses, they are
*pool slots* that hold whatever object is currently parked there.

**Write authority (bonus, both write-verified with screenshots).**
`write_memory(obj+0x12, 900)` teleported P1 across the stage and the game
kept walking from the written value. `write_memory(obj+0x16, 0xFFC0)` (−64)
left Reptile hanging in mid-air with his shadow on the ground below. Unlike
the previously-tested Y candidates these are authoritative stores, not
recomputed outputs.

Confidence: **VERIFIED**.

### Player Y — FOUND (`obj+0x16`), the long-open item

`obj+0x16` is a **signed** 16-bit height. A held-`up`+`right` jump (Reptile,
2-human rig, ~18 ms polling) traced a clean symmetric arc:

```
85 75 66 58 50 42 35 29 23 17 12 8 4 0 -3 -5 -7 -9 -10 -10 -10 -10
-9 -7 -5 -3 0 4 8 12 17 23 29 35 42 50 58 66 75 85
```

~0.74 s of airtime (≈40 frames @ 54.71 Hz), a 4-sample hang at the apex, and
an exact return to the resting value. x advanced linearly throughout (jump
arc + forward drift), so the two fields are independent. **Smaller = higher**
and it goes negative above a certain height, so any "airborne" test must be
signed. Write-verified (levitation screenshot above).

**Honest limit on GROUND_Y**: the resting value is NOT one constant. Observed
resting values: Reptile 85 and Liu Kang 83 on the Dead Pool-style stage,
Liu Kang 87 / Reptile 89 on the second stage of the ladder, Scorpion 85.
So it is character- *and* stage-dependent (both a per-fighter sprite-origin
offset and a per-stage floor). Do **not** ship a scalar `GROUND_Y` for
arcade — derive "airborne" as *y below this fighter's own resting y*
(sample at round start, or treat any y < resting−4 as airborne), or
calibrate per stage. Jump height above rest measured 95 units.

### Disproven / dead ends from this session (do not re-chase)

- **No position field inside the `0x17A` fighter struct** — DISPROVEN with a
  byte-for-byte diff of all `0x17A` bytes of block1 across a 6-snapshot
  held-`right` walk, and the same for block2 across a held-walk on port 1:
  **0 bytes changed**, both fighters. This extends the earlier
  "struct is static when untouched" finding to walking, and closes
  acceptable-outcome #1 (the struct's only *link* to position is the
  pointer at `-0x0C`).
- `0xBA59`/`0xBA5A`/`0xBA5D`/`0xBA5E`/`0xBA61`/`0xBA62`/`0xBA69`/`0xBA6A` —
  reverse cleanly under held direction and look like positions (`0xBA5A`
  ran 359 → 370 / 359 → 296), but they respond to **either** player's
  movement. Camera / scroll registers, not per-fighter. Not adopted.
- `obj+0x18` (reads 80 for one fighter, 79 for the other) — **not facing**.
  It did not flip when P1 was force-teleported to the far side of P2, and
  the 80/79 assignment swapped between the two fighters after ordinary play
  with no crossover. Unmapped, harmless.
- The `0x3800-0x3F00` per-scanline sprite table produces ~55 reversing
  monotone u16s under any walk. It is the rendered echo (the exact trap the
  signal-hunt doc warns about); everything there was excluded by inspection.
- `0xBE81` (the old LIKELY "P1 facing") read a constant **2** through a
  forced crossover in this rig, not 1→0. Still unverified; leaving as-is.

### Pointer hygiene (a null guard is required)

The pointer is not always valid, and that is useful rather than a problem:

| phase | `0xC044` | `0xC1BE` |
|---|---|---|
| attract / boot | `0x00000000` | `0x00000000` |
| character select (P1 chosen, P2 not) | `0x01035F10` (valid) | `0x00000000` |
| live fight | valid | valid |

So: treat a value outside `0x01000000..0x01400000` as "no fighter object"
and emit no x/y that frame. Cheap, and it makes the same field double as a
liveness check — which is what `p1_x` was wrongly used for before.

### Toolkit friction (new, worth adding to the RE skill)

- **CORRECTED — `hold_buttons` DOES reach the core while stepping.** An
  earlier draft of this section claimed the opposite ("60 frames of `step`
  with `right` held produced zero movement"). That was a MISDIAGNOSIS, and it
  would have forced the entire frame lab onto real-time measurement, which
  `docs/frames.md` §2.4 forbids. The real cause is below.
- **`step` is FIRE-AND-FORGET: rapid `step` calls silently collapse.**
  Measured on a 2-human arena, P1 world X via the `block-0xC` pointer:

  | trial | steps requested | frames landed | x delta |
  |---|---|---|---|
  | hold `right`, each step confirmed landed | 30 | 30 | **+72** |
  | hold `right`, each step confirmed landed (repeat) | 30 | 30 | **+63** |
  | no input, each step confirmed landed | 30 | 30 | **0** |
  | hold `right`, rapid unconfirmed `step` calls | 30 | **1** | 0 |

  The bottom row is the false negative: the input was fine, the FRAMES never
  happened. Any stepping protocol MUST confirm each step landed by polling
  `get_state`'s `frame_count` before issuing the next one. Independently hit
  by two agents on the same day (the framelab calibration run measured "10
  back-to-back steps moved frame_count by 0; the same 10 spaced 50 ms apart
  moved it by exactly 10").
- **`load_state` does not drain while paused**, same GUI-frame mechanism.
  Resume, load, verify the load landed by reading a known field, then pause.
  A probe that skips the verification silently measures the PREVIOUS state —
  observed live while testing the above.
- A 256 KiB `read_region` snapshot costs **~70 ms**, so a 5-6 snapshot
  series across a walk is ~1 s of game time — plenty of resolution for
  position work, and vastly cheaper than the whole 2.3 MB region.
- 1P-vs-CPU is a *bad* rig for monotonicity: the CPU's pushback and
  blockstun overrode a held direction repeatedly (a hold-`left` measured
  541 → 613 because Scorpion was walking into P1). Health refills do not fix
  it — pushback is not damage. Use the 2-human arena, or retreat first.

### Bonus: Scorpion = char_id 10 is now READ, not deduced

The cold-boot ladder put P1 (Liu Kang, `block1+0x0` = 1) against a CPU whose
`block2+0x0` read **10** while the HUD bar drew **SCORPION** and the round
end drew **SCORPION WINS** (screenshots). That closes the last roster
deduction flagged above; `_roster_provenance` in `family.json` can be
updated from "deduced" to "read" for Scorpion.

### Controls used

`hold_buttons` (ports 0 and 1: `right`, `left`, `up`, `y`, `start`,
`select`), `release_buttons`, `read_memory`, `read_region` (via
`shadow_train.re.Probe.snapshot`), `write_memory`, `load_state`,
`save_state`, `pause` / `resume` / `step`, `screenshot`. `press_buttons` was
not used. Analysis with `shadow_train.re` (`Probe`, `diff`) plus a local
weak-monotone-reversal filter and a u32-pointer scan over the exposed
region. No profile JSON was modified.

### Proposed profile change (NOT applied — orchestrator's call)

`mk2.profile.json` currently sources `x` from the globals `p1_x 0x6CBA` /
`p2_x 0x6CFC`, which this session shows are wrong on most runs. The proposal
needs one new schema concept — a *pointer-relative* fighter field:

1. Add to `memory.blocks` (or alongside `fighter_fields`) an object-pointer
   declaration: `object_ptr: {"off": "-0xC", "size": 4, "encoding":
   "tms34010_bitaddr"}` — i.e. `block1` resolves via `0xC044`, `block2` via
   `0xC1BE`, decoded as `(v - 0x01000000) >> 3`, invalid outside
   `0x01000000..0x01400000`.
2. Change `fighter_fields` `x` from its `globals` form to
   `{"name": "x", "via": "object_ptr", "off": "0x12", "size": 2}`.
3. Add `{"name": "y", "via": "object_ptr", "off": "0x16", "size": 2,
   "signed": true}` — arcade gets a `y` for the first time.
4. Optionally add `{"name": "char_id_obj", "via": "object_ptr",
   "off": "0x3E", "size": 1}` as a runtime consistency check against
   `char_id`; a mismatch means the pointer went stale and the frame should
   be dropped.
5. Keep `p1_x`/`p2_x`/`p1_screen_x` in `globals` **only** if something still
   reads them, and mark them DISPROVEN in `_STATUS`; nothing should use them
   for position or for liveness.
6. `calibration.GROUND_Y` must stay **0** / unused for arcade — see the
   honest limit above; a per-fighter resting-y baseline is the right shape
   and does not exist in the schema yet.

Consequence if adopted: `src/training.rs` `resolve()` gains `x` and `y` at
the fighter-block stride (its actual contract), so arcade training
enforcement stops no-op'ing on the position side; `docs/frames.md` §10's
first two stated limitations both close.

## First measured frame data — Reptile HP (2026-08-30)

The frame lab's first real numbers, via the act-again probe
(`docs/frames.md` §4). Rig: `shadow/arenas/mk2/r-v-r.state`, 2-human Reptile
mirror, headless FBNeo. Move: **HP** thrown after a 44-frame walk-in, a FAR
HP at a ~82 px gap (MK2 has proximity normals — this is not the close HP).
43,712 confirmed steps, 507 verified loads.

| outcome | attacker free | defender free | **advantage** |
|---|---|---|---|
| on block (P2 holds Block) | N*=9 | N*=13 | ~~+4~~ → **+13** |
| on hit (P2 holds nothing) | N*=9 | N*=13 | **+4** |

> **CORRECTED 2026-08-30 (same day), on_block only: +4 was WRONG BY 9
> FRAMES; the value is +13.** The probe differenced each side's OWN
> calibrated latency, which cancels only when both sides share a probe
> shape. On block they do not (attacker 1, guarded defender 10), and that 10
> was measured while the fighter was FREE — during blockstun the
> block-stance drop runs concurrently, so subtracting it made every move
> look 9 frames more punishable than it is. Settled by a THIRD rig that uses
> no probe at all (sweep the defender's counter-attack frame, read the
> attacker's damage register): "earliest frame the defender can attack" =
> walk manifest − 2, in 4/4 configurations across both probe shapes. Stored
> advantage is now the manifest difference. `on_hit` was never affected.
>
> Note what did NOT catch this: two observables agreeing to the frame on all
> four sweeps, monotone predicates, and reproduction from a fresh process.
> Both observables shared the same flawed subtraction, so the cross-method
> check (§8.4) confirmed precision perfectly while the number was wrong.
> Only an independent RIG — different readout, no probe — found it.

Anchor `contact_frame=55, hits=1` from struct health `block+0x0E`
(161→150 on hit, 161→158 on block — so both rigs genuinely connected, and
genuinely differed).

**Confidence is high for these two numbers specifically**: each was measured
TWICE by two observables living in different data structures — the walk
velocity word (`block+0x0B..0x0D`) and the pointer-resolved `x`
(`obj+0x12`) — and they **agree to the frame on all four sweeps**. Every
predicate was monotone; every sweep passed on first attempt with repeats=2;
a third re-measurement from a FRESH EMULATOR PROCESS reproduced the
defender's 13 exactly. This satisfies `docs/frames.md` §8.4, the criterion
that tests accuracy rather than precision.

**ANSWERED by the full-kit run (see below): the equality was an artifact of
the calibration bug, not a property of the game.** With the correction,
far HP is +4 on hit and +13 on block. Advantage varies by move in BOTH
directions. Original question preserved below for the reasoning.

**Open question for the full-kit run: `on_hit == on_block` here.** That is a
legitimate measurement (the health deltas prove the two rigs differed), and
old engines do often give hitstun and blockstun the same length — but it is
also exactly what a rig bug would look like. Do NOT generalise from one
move. If the whole kit comes back with `on_hit == on_block` everywhere,
suspect the protocol; if it varies by move, this was real.

Note the absolute frames carry a labelled margin that the ADVANTAGE does
not: the on-block defender's absolute (23/24) includes the ~9-frame
block-stance drop, which cancels out of the difference. The advantage is the
trustworthy number; the absolutes are not directly comparable across
different probe shapes.

## Reptile's normals across the spacing ladder (2026-08-30, task B3)

Ten measured (move, gap) cells — every standing normal that connects, at
every rung of the ladder where it connects, on hit AND on block — plus the
crouching uppercut. It supersedes the single-move section above on one
point, flagged in bold below: **the on-block numbers there were 9 frames too
generous to the defender**, and this section shows the experiment that
settles it.

### Rig

Ladder arenas `shadow/arenas/mk2/gap-{60,45,30}.state` (62 / 72 / 110 px, the
`.gap.json` sidecars), 2-human Reptile mirror, P1 on the left. Two headless
FBNeo instances on MCP ports 4047 and 4048 (never 4025). Contact anchored on
the fighter-struct health `block+0x0E` per `docs/frames.md` §4.1, one anchor
per rig — the blocked run gets its own, so "hit and block connect on the same
frame" is a result (it does, in all ten cells) rather than an assumption.

No walk-in: the arena encodes the gap, so the move's input is frame 0 of the
replay. That removes ~45 frames from every one of the ~220 replays a cell
costs and removes a decelerating fighter as a confound.

Both observables were sampled from the same runs and **agreed to the frame on
all 62 sweeps of both runs, without a single exception** — the walk-velocity word `block+0x0B..0x0D` and the
pointer-resolved `x` (`obj+0x12`), which live in different data structures
(§8.4).

Probe latencies, calibrated per shape on this rig and confirmed HOLD-LIMITED
(see "the calibration must be hold-limited" below):

| probe shape | `struct_velocity` | `pointer_x` |
|---|---|---|
| attacker (either rig) | 1 | 2 |
| defender, on hit | 1 | 2 |
| defender, on block (drops Block, walks) | 10 | 11 |

### The connect map — where the proximity variants actually are

One anchor replay per cell, before any sweep. `—` = the contact signal never
fired: a whiff, which is an OUTCOME (§1.1), not a missing measurement.

| gap | HP | LP | HK | LK | cHP | cLK |
|---|---|---|---|---|---|---|
| 180 px (K=0) | — | — | — | — | — | — |
| 147 px (K=15) | — | — | — | — | — | — |
| 110 px (K=30) | — | — | 32 | 26 | — | — |
| 72 px (K=45) | 11 | 8 | 32 | 26 | 40 | 6 |
| 62 px (K=60) | **24** | *throw* | **16** | **16** | 40 | 6 |

Damage alone locates every variant boundary, and it is between 62 and 72 px
for **every button**: HP 11 → 24, HK 32 → 16, LK 26 → 16, LP → a throw. The
same boundary for four different buttons is itself evidence that MK2 resolves
proximity once, per input, at one distance threshold — and it is why 62 px
rows are stored as `variant: close` and 72/110 px rows as `variant: far`,
never averaged (§5).

Kicks reach 110 px and whiff at 147; punches reach 72 px and whiff at 110;
the crouching normals reach 72 px and whiff at 110. `connect_range` in the
store is that largest CONNECTING rung — a bracket (`110 < range ≤ 147`), not
a measured edge.

### The table

Frames are relative to the contact frame. "att free" / "def free" are the
frames at which that fighter's walk manifests; the advantage is their
difference (see the convention note below). `n` = independent full
measurements; `n=2` means a second, cold-started emulator process reproduced
the cell exactly.

| move | variant | gap | damage | chip | contact f | att free | def free (hit) | def free (blk) | **on hit** | **on block** | n |
|---|---|---|---|---|---|---|---|---|---|---|---|
| HP | close | 62 px | 24 | 6 | 8 | +18 | +26 | +19 | **+8** | **+1** | 1 |
| HP | far | 72 px | 11 | 3 | 11 | +10 | +14 | +23 | **+4** | **+13** | 1 |
| LP | far | 72 px | 8 | 2 | 11 | +10 | +14 | +23 | **+4** | **+13** | 2 |
| HK | close | 62 px | 16 | 4 | 11 | +33 | +26 | +19 | **−7** | **−14** | 2 |
| HK | far | 72 px | 32 | 8 | 8 | +39 | +46 | +23 | **+7** | **−16** | 2 |
| HK | far | 110 px | 32 | 8 | 8 | +39 | +46 | +23 | **+7** | **−16** | 2 |
| LK | close | 62 px | 16 | 4 | 11 | +33 | +26 | +19 | **−7** | **−14** | 2 |
| LK | far | 72 px | 26 | 6 | 8 | +39 | +18 | +23 | **−21** | **−16** | 1 |
| LK | far | 110 px | 26 | 6 | 8 | +39 | +18 | +23 | **−21** | **−16** | 2 |
| cHP | close | 62 px | 40 | 10 | 14¹ | +28 | NULL² | +23 | **NULL²** | **−5** | 1 |

¹ replay-relative; the move's own input is at frame 6 (the crouch lead-in), so
`first_active_frame` is **8**. ² the uppercut LAUNCHES: the victim's `obj+0x16`
y leaves its resting 85 and does not return until frame 78, and §1.1 gives a
knockdown no on-hit advantage number — the wakeup window is the measurement,
and it is a different column (unmeasured, NULL).

`first_active_frame` is stored ONLY for the 62 px rows (§4.4 — at larger gaps
it is contaminated by travel): close HP **8**, close HK/LK **11**, cHP **8**.
Those are the first frames the contact signal fires; the ±1 question of
whether the damage register is written on the overlap frame or the frame after
is not resolved here.

Two more things fall out of the table. Close HK and close LK are the **same
move**: identical damage (16), identical contact frame (11), identical
advantage on both rigs, measured twice each. And far HK and far LK are
gap-INVARIANT — 72 px and 110 px give byte-identical numbers, measured on two
different emulator processes, which is the strongest evidence in this section
that the protocol is measuring the move and not the arena.

### **Correction: the block-stance drop is NOT probe overhead**

The section above (Reptile HP, earlier the same day) says the on-block
defender's ~9-frame block-stance drop "cancels out of the difference". **It
does not**, and the same paragraph is wrong here in the same way if left
uncorrected.

The cancellation argument (`probe.py`'s module docstring) requires both sides
to share a probe shape. They do not in the on-block rig: the attacker's shape
calibrates to `l = 1`, the guarded defender's to `l = 10`. Subtracting each
side's own calibration removes 9 frames from the defender that were never
removed from the attacker, so every `on_block` number comes out 9 frames too
favourable to the defender. The 10-frame figure is measured with the fighter
ALREADY FREE; during blockstun the stance drop runs CONCURRENTLY with the stun
instead of after it, so it is not additional delay at all.

Settled by a third measurement that does not use the act-again probe. A
**punish rig**: P1 throws the move, P2 blocks it, P2's counter-attack frame is
swept, and P1's damage register says what happened (full damage = clean
punish, chip = P1's guard was up first, nothing = no contact). "Earliest frame
the defender can attack" came out at exactly `walk manifest − 2` every time:

| rig | move | walk manifest | earliest counter-attack |
|---|---|---|---|
| on block | close HP @62 px | contact+19 | **contact+17** |
| on block | far HP @72 px | contact+23 | **contact+21** |
| on block | far HK @110 px | contact+23 | **contact+21** |
| on hit | far HP @72 px | contact+14 | **contact+12** |

The −2 is the same on both probe shapes, so it cancels out of a difference of
MANIFEST frames — which is therefore what this table stores, and what
`framelab.kit.manifest_advantage` computes. `on_hit` is unaffected (one shape
on both sides, so the two formulas agree exactly); `on_block` changes by
exactly `W_def − W_att` = **+9**. **The far-HP `on_block = +4` published in
the section above is +13.**

One assumption is left standing and named: the attacker's own "can attack
again" frame was NOT measured. A second attack from the same fighter never
reaches at these spacings (verified at every re-press frame out to +60), and
`+0xC0` is not a trustworthy attack signal (it moves on guard release and on
inputs that produce no attack), so the attacker side rests on its probe shape
being identical to the defender's on-hit shape, which was verified.

### The punish the table predicts, thrown

`docs/frames.md` §8.3, on the most unsafe cell and a safe one:

- **far HK @110 px, −16 on block.** Blocked, then countered with HK (the
  fastest normal that reaches at that gap, contact frame 8). Pressed at
  contact+21 and +22 it lands **clean, 32 damage**, contacting on frame 37
  against a P1 whose guard does not come up until frame 47. Pressed at +19 or
  +20 — one and two frames before the table says the defender is free —
  **nothing comes out at all.** The predicted window opens on the predicted
  frame.
- **far HP @72 px, +13 on block.** Same protocol, every counter frame from
  +21 to +24: **chip only (3)**. It cannot be punished, as the table says.
- Unexplained, and recorded rather than smoothed over: the far-HK punish also
  stops connecting from contact+25 onward. It is not the attacker's guard
  (frame 47) and the gap is static at 131 px from frame 21, so the obvious
  range explanation does not fit either. Whatever closes that window is
  unmeasured.

### on_hit vs on_block: the open question is answered

The earlier section flagged `on_hit == on_block` for far HP as possibly a rig
bug. **It was real, and it was a coincidence of that one move.** Across the
kit the two columns are independent and differ in BOTH directions:

- close HP: hit +8, block +1 — hitstun longer than blockstun.
- close HK/LK: hit −7, block −14.
- far HP/LP: hit +4, block **+13** — blockstun LONGER than hitstun.
- far LK: hit −21, block −16.

The physical story the numbers tell: **blockstun takes only two values
across the whole kit** — the defender's walk manifests at contact+19 after the
three close standing normals and at contact+23 after everything else (the four
far normals AND the uppercut), regardless of which button or how much chip —
while **hitstun varies per move** (8 dmg → +14, 11 → +14, 16 → +26, 24 → +26,
26 → +18, 32 → +46). It broadly tracks damage, but not monotonically: far LK
(26) frees the victim at +18 while close HK (16) holds it to +26, so damage is
a correlate, not the rule.

That breaks `docs/frames.md` §8.2's `on_hit ≥ on_block` for the far punches,
and the violation is not a measurement artifact: the punish rig independently
puts the far-HP defender's counter-attack at contact+12 on hit and contact+21
on block. §8.2 encodes a modern-fighting-game convention, not a law; MK2's
block recovery is simply longer than its light-hit reaction.

### What was NOT measured, and why

- **LP at 62 px is a THROW, not a normal.** 34 damage, contact at frame 48,
  and the damage is IDENTICAL with the defender holding Block — unblockable,
  which is the proof. §1.1 gives a throw/knockdown no advantage number. (This
  is the proximity override already noted under "Macro-action encodings".)
- **cLK @62 px: refused, twice.** Both attempts produced a NON-MONOTONE
  attacker predicate (`F…F T F…F T…T`, the isolated TRUE at N=13 on one
  process and N=16 on the other, real boundary at 20). That is either the
  known one-frame-early-hold transport flake or a genuine T…F…T predicate;
  either way §4.3 says a first_true read off it is not a boundary. No row.
- **Jumping normals: not attempted.** The act-again probe's observable is a
  WALK, and an airborne MK2 fighter cannot walk, so the probe cannot answer
  "is this fighter actionable" mid-jump. Measuring jump-ins needs a different
  observable (§1.1 also gives airborne hits no advantage number).
- **Reptile's specials**: out of this task's scope (Invisibility needs a DSL
  extension).
- **`hitstop`, `active`, `recovery`, `total`, `wakeup_window`,
  `guard_height`**: NULL in every row. None of them is a by-product of this
  protocol; `guard_height` in particular needs a CROUCHING defender rig, which
  was not built.
- **Gaps 147 px and 180 px**: every button whiffs, so they carry no rows —
  they are the whiff half of the connect map above.
- **One character, one matchup.** Reptile mirror only.

### The calibration must be hold-limited (a gap in `docs/frames.md` §3.1)

§3.1 says the calibration point must be "far enough past the anchor that the
fighter is certainly free" and gives no way to check. It matters: far HK's
defender calibrates to **6/7** at anchor+40 and to **1/2** at anchor+70 and
anchor+100. The 40-frame number is not a latency at all — it is residual
hitstun (that victim is stuck for 46 frames) — and taking it would have
inflated that move's `on_hit` by 5 frames, silently, in the safe direction.

The check is cheap and now enforced in `framelab.kit.calibrate_shapes`:
measure every shape at TWO `at_n` values and require them to agree. A latency
that shrinks as the probe moves later is stun, not latency.

### The input shape must be validated, not assumed

Asserting `down + button` on the same frame from a standing start makes the
game enter *something* (the attacker's `+0xC0` moves 160→192) that contacts
NOTHING at any rung of the ladder. The first pass through this kit therefore
reported "crouching normals never reach", which is clean, plausible, and
false. Holding `down` alone for 6 frames first produces the real crouching
normal: uppercut, 40 damage, launch. **A move must be identified by its
measured signature (damage, contact frame, connect behaviour), never by the
buttons the harness believes it pressed.**

### Cost and provenance

Three headless processes (two measuring cells in parallel, ~5,400 s each, one
for the punish rigs): **287k confirmed steps, 5.9k verified loads** over the
whole task (both measurement runs, the connect map, the
calibration checks, the knockdown scan and the punish rigs). Rows live in
`shadow/framelab/frames.db` (`framelab.store`) and export to
`library/mk2/arcade.frames.json` — 20 rows, one per observable per cell,
each carrying `observable`, `method`, `input_latency_frames`, `core_id`
(`fbneo_libretro.dylib:sha256:972e8fb8c8394979`) and `rom_id`
(`mk2.zip:sha256:e8d3f2f8cefe1aab`).

## Re-measurement of Reptile's kit on the fast transport (2026-08-30, task F3)

The whole kit above, measured again from scratch on the synchronous
`step` / `run_frames` transport and the profile-driven `framelab` block, and
compared to `library/mk2/arcade.frames.json` **row by row**.

**All 20 stored rows reproduced to the frame.** Same `on_hit`, `on_block`,
`damage`, `hits`, `knockdown`, `first_active_frame`, `connect_range`,
`gap_px`, `gap_walk_frames`, `input_latency_frames`, `method`, `core_id`,
`rom_id`. Nothing was rewritten to make it agree, and no cell was re-run to
get a better answer — the comparator (`framelab.ladder.compare_rows`) reports
disagreement and refuses; it never resolves it. This satisfies
`docs/frames.md` §8.1 for the whole table rather than for the required random
sample of five.

The connect map reproduced identically too, including every whiff:

| gap | HP | LP | HK | LK | cHP | cLK |
|---|---|---|---|---|---|---|
| 180 px | — | — | — | — | — | — |
| 147 px | — | — | — | — | — | — |
| 110 px | — | — | 32@f8 | 26@f8 | — | — |
| 72 px | 11@f11 | 8@f11 | 32@f8 | 26@f8 | 40@f14 (KD) | 6@f18 |
| 62 px | 24@f8 | *throw* | 16@f11 | 16@f11 | 40@f14 (KD) | 6@f18 |

…as did all four probe-shape calibrations (attacker 1/2 on both rigs,
defender-on-hit 1/2, guarded defender 10/11), each confirmed hold-limited at
two probe points.

### The one-frame flake is NOT the `hold_buttons`-after-`step` race, and the 8 ms settle never fixed it

`docs/frames.md` §3 precondition 6 attributes the ~1-in-50 spurious TRUE to a
`hold_buttons` issued immediately after a step confirmation being read by the
frame that was supposed to be over, and prescribes "settle, or make `step`
synchronous". `step` is synchronous now, and the flake did not go away — so
it was measured properly. **Both halves of that precondition turn out to be
wrong**, and the real cause is a different, sharper thing.

Rig: far HP on `gap-45.state`, defender probe at N=0 (the contact frame — the
victim is certainly still stunned, so the answer is certainly FALSE), 200
identical probe/control pairs per configuration.

| how the probe's hold was asserted | spurious TRUE at N=0 |
|---|---|
| `run_frames(port1=[…])` per-port mask | **13 / 200** |
| `hold_buttons`, then `run_frames` | 0 / 200 |
| `hold_buttons`, then one confirmed `step` per frame | 0 / 200 |
| `hold_buttons` + fold confirmation, then `run_frames` | **0 / 400** |

And, separately, on the pre-`run_frames` protocol the 8 ms settle made it
**worse, not better**: 16/100 spurious with the settle against 7/100 without,
on the same rig in the same session. The A/B behind §3.6 was 14 pairs per
arm — far too few to see a ~7% effect, and the two arms differed by less than
the noise. The settle was never the mechanism.

**What the mechanism actually is.** `src/main.rs`'s host loop folds the
injected/held input into the core's input state at step (a0), and only then
calls `Frontend::run_frame`, which checks the pause/step/batch gate. Those are
separate lock acquisitions. `run_frames` sets the per-port held mask **and**
arms `step_batch_remaining` in ONE acquisition, so there is no window for a
fold in between: if the batch is armed while the loop is already past its
fold, the batch's FIRST frame runs on the PREVIOUS input. `hold_buttons` is a
separate MCP call, and a round trip is far longer than a loop iteration, so it
almost always folds in time — which is why the old protocol looked clean.

The symptom was not the probe's hold landing early at all. In every captured
flake it was the **attacker's move coming out one frame late**: the victim's
`block+0x0E` still read 161 on the frame it normally already reads 150, so
contact happened at f12 instead of f11, which leaves the defender genuinely
free on the frame the probe holds at — and it genuinely walks one frame
(`block+0x0B..0x0D` = `00 02 00`, `obj+0x12` +2 px) before hitstun takes over.
Both observables agree, the predicate looks plausible, and the number is
wrong by however far that shifts the boundary. §8.4's cross-method check
cannot see it, because both observables are watching the same real walk.

**The fix is client-side and it is an oracle, not a wait.** `get_input`
reports `folded` (what the last fold gave the core) next to `asserted` (what
the next one will), so every input change now ends by polling until the two
agree (`framelab.session.confirm_fold`), and `run_frames`' per-port masks are
never used to CHANGE input. 0 spurious in 400 at the config that was 13/200.

The general rule, which is the same shape as the "never anchor on a DRAWN
value" rule: **an input is not asserted until the thing that consumes it says
it has it.** A write to the held-input mask is a request; the fold is the
event.

### What that fixed, beyond the flake rate

`cLK @62 px` was **refused twice** by the earlier run, both times on a
non-monotone attacker predicate with an isolated TRUE at N=13 / N=16 against
a real boundary at 20 — the exact signature of one late attack. With the fold
confirmed it measures cleanly, first attempt, both observables agreeing, at
both rungs it reaches, with every actionable(N) evaluated twice:

| move | variant | gap | damage | contact f | att free | def free (hit) | def free (blk) | **on hit** | **on block** |
|---|---|---|---|---|---|---|---|---|---|
| cLK | — | 62 px | 6 | 18¹ | +23 | +10 | +19 | **−13** | **−4** |
| cLK | — | 72 px | 6 | 18¹ | +23 | +10 | +19 | **−13** | **−4** |
| cHP | close | 72 px | 40 | 14¹ | +28 | NULL² | +23 | **NULL²** | **−5** |

¹ replay-relative; both crouching moves have a 6-frame stance lead-in, so
cLK's `first_active_frame` is **12** and cHP's is **8** (stored only at the
62 px rung, §4.4). ² the uppercut launches — §1.1 gives a knockdown no
on-hit number, at 72 px exactly as at 62 px.

`variant` is NULL for cLK on purpose: its damage and contact frame are
identical at 62 and 72 px, so there is no proximity boundary to name and
labelling one rung "close" would invent a variant the signature does not
support (§5). Every OTHER button has its boundary between 62 and 72 px; the
crouching ones do not have one at all in this ladder's range.

cLK is negative on hit (−13) and nearly neutral on block (−4), and its
hitstun of +10 is the **shortest in the kit** — consistent with its 6 damage
being the smallest (8 → +14, 11 → +14, 16 → +26, 24 → +26, 26 → +18,
32 → +46, and now 6 → +10).

**This revises the blockstun rule stated above.** That section reads the two
blockstun values as a distance rule — "+19 after the three close standing
normals and +23 after everything else (the four far normals AND the
uppercut)". cLK breaks that: it frees the defender at **+19 at BOTH 62 and
72 px**, while cHP gives +23 at both. So blockstun is a property of the MOVE,
not of the gap; the earlier phrasing held only because every move measured
then happened to have its variant boundary at the same distance. Reptile's
blockstun still takes exactly two values (+19, +23) across everything
measured — that part survives.

`cHP @72 px` matches `cHP @62 px` exactly on every column, which is the same
gap-invariance far HK and far LK show — more evidence that the protocol is
measuring the move rather than the arena.

**Jumping normals are still out**, for the unchanged reason: the act-again
observable is a WALK and an airborne MK2 fighter cannot walk, so the probe
cannot answer "is this fighter actionable" mid-jump. Nothing about the speed
work changes that; it needs a different observable.

### Cost

One headless FBNeo process, `--pace 0`, MCP port 4055. The full re-measure
(connect map + all four probe-shape calibrations + all 10 cells + the two
far-HK cells at a wider `max_search`) ran **230,336 frames and 5,301 verified
loads in 335 s** — and it did it at `repeats=2`, i.e. every actionable(N)
evaluated twice and required to agree, which is roughly double the work the
original run did.

The original task measured 287k confirmed steps at 41.1 ms and 5.9k loads at
12.3 ms: **~3.3 hours**. The same 230k frames on that transport would have
been ~2.6 hours. Measured now: 5.6 minutes, a ~28× speedup, from a
synchronous `step` at 0.74 ms and `run_frames` at 0.71 ms/frame with the
replay's unobserved prefix batched into one call per segment (132,718 of
167,564 frames in the main run were batched, at 8,217 calls instead of
132,718).

Flake evidence from the real protocol, no settle anywhere: **52 exhaustive
sweeps, 2,626 repeat-checked actionable(N) evaluations, 0 repeat-check
failures and 0 non-monotone refusals.** The two refusals the run did produce
were the cap guard doing its job — far HK's defender frees at `first_true`
43 against a default `max_search` of 45, inside `_CAP_MARGIN`, so the row was
refused rather than reported; re-run at `max_search=58` it reproduces the
stored numbers exactly. That is worth keeping in mind for the next kit: 45 is
too tight for a 32-damage roundhouse's victim.

## Health RAMPS at round start — not damage (2026-08-30)

Found by the harvest tool while mining real recordings, and worth knowing
well beyond it. **Every MK2 arcade round after the first opens with several
dozen zero-input frames during which `health` visibly ramps** `4 → 6 → 8 →
… → 161`, +2 per frame — a round-intro fill animation, not damage.

Anything that watches health EDGES will see a large fake contact at every
round boundary. The harvest tool manufactured exactly that until it was
fixed; the fix is to start scanning only from the round's first frame with
any input on either port — the same zero-input signal this profile's own
evidence already uses to detect demo rounds.

Consumers to keep in mind: the block-punish dummy triggers on
`hitstun_sources` (the HUD health pair) and could fire spuriously in that
window, the same class as the already-recorded "one spurious punish per
refill". The frame lab is NOT affected — it anchors on health drops from a
mid-fight arena, never across a round boundary — but a future measurement
that spans a round start would be.

Also found in the same pass: **`Block` appears in both MK2 ports'
`attack_chords`** (it is needed for macro chords like the slide), so any
code that treats every entry in that table as an ATTACK will attribute
contacts to "Block". Attribution must exclude it.

## Special-move encodings, live-audited (2026-08-30, task M1)

Every `special_inputs` entry in `library/mk2/mk2.profile.json` executed live
and required to PRODUCE ITS MOVE. Four of the seven had never been
demonstrated; two of those four were wrong, and one entry that *had* been
"verified" (Reptile's `acid_spit`) turns out to have been verified against
the wrong evidence and does not work at all.

**Rig.** Headless FBNeo, MCP port 4064, `shadow/arenas/mk2/m-v-r.state` —
Mileena (`block1`, port 0, char_id 5) at `x` 927 vs Reptile (`block2`,
port 1, char_id 9) at `x` 1119, **192 px apart**. That gap is past the whiff
edge of the entire measured normal-move connect map (§"Reptile's normals
across the spacing ladder": nothing reaches at 180 px), so *any* damage is a
special. Transport: `framelab.session.LabSession` — `load_state(pause_after)`,
`hold_buttons` + `confirm_fold` for every input change, confirmed `step`.
Playback mirrors `src/macros.rs::MacroExec` exactly: each step's mask held for
its `frames`, `STEP_GAP = 2` neutral frames between steps. Controls run in
every batch (no input; bare button; wrong button; wrong direction; one tap
instead of two) and every one of them produced 0 damage.

### The rule that decides three of the four questions

**A direction chorded with the trigger button on the SAME FRAME does not
register. The direction must be down at least one frame BEFORE the press —
and it need not still be held AT the press.**

This is one measurement, not an interpretation. `F . F+HP` (Reptile) gives 0
damage in **all 16** frames×gap configurations tried; `F . F . HP` gives 15
in every configuration with gap ≥ 2. `B . B . D+HK` (Mileena) gives 0; move
the `down` one frame earlier — `B . B . D(1f) . D+HK`, or drop it to a bare
`HK` — and the roll comes out. So MK2's parser keeps a buffer of directional
taps and fires when a button edge arrives with the right taps already in it;
a direction pressed on the trigger frame is not yet in the buffer.

Two exemptions, both measured:

- **Single-frame chords are exempt.** Reptile's slide is
  `back+LK+LP+Block` on one frame and still lands (13 damage, victim
  launched). A chord special is not a motion; it has no buffer to be late for.
- **`force_ball` (`B . B+HP+LP`) fires chorded anyway** — 16 damage, victim
  launched, both chorded and bare. Its two-button chord evidently needs a
  coincidence window that delays the trigger past the direction. Mechanism
  NOT isolated; recorded as an exception rather than explained away.

### Verdicts

| move | published | shipped now | verdict | evidence |
|---|---|---|---|---|
| mileena `sai_throw` | hold HP ~3 s, release | `hold HP` **`min_frames: 34`**, `release` | **VERIFIED, timing corrected** | 23 dmg at 192 px, projectile. HP-specific (LP/HK/LK/Block held 60 f: 0). Fires on RELEASE only — held 60 f and 120 f without releasing: 0 dmg. |
| mileena `teleport_kick` | `F F LK` | `F` · `F` · `LK` (unchanged) | **VERIFIED, no change** | 32 dmg; `y` dives to +200 (underground) then to −131 (above the screen) and she lands kicking. |
| mileena `roll` | `B B D+HK` | `B` · `B` · `D` · `HK` (**changed**) | **VERIFIED after correction** | 21 dmg at contact f32, victim's `y` 89 → −6 (launched) and pushed 1119 → 1002, Mileena rolls 915 → 1192 **through** him. |
| reptile `invisibility` | `[BLK] U U D HP` | `while_held BLK` over `U U D`, `release`, `HP` (unchanged) | **VERIFIED, no change** | Framebuffer: Reptile's sprite is gone (see below). |
| reptile `acid_spit` | `F F HP` | `F` · `F` · `HP` (**changed**) | **prior verification RETRACTED** | see below |
| reptile `slide` | — | unchanged | re-confirmed | 13 dmg, victim launched −45. |
| reptile `force_ball` | `B B HP+LP` | unchanged | re-confirmed | 16 dmg, victim launched −82. |

`teleport_kick`'s other candidate reading, `F` · `F+LK` (the acid_spit
shape), **also fires** — 32 damage, and its contact lands 27 frames after
the LK onset in both readings, i.e. the same move with the same startup.
The bare-button encoding is kept because it is the one the *matcher* can
recognise from either human input: `src/macros.rs`'s `sat()` requires a
non-first step's `dirs` to have a FRESH onset, so `F . F+LK` only matches a
player who re-taps forward on the button frame, while a dirs-less final step
matches whether or not forward is still held. Same argument applies to
`acid_spit` and `roll`: encode the trigger as a bare button.

Known cost of that choice, stated rather than discovered later: a dirs-less
trigger step is satisfied on any frame the chord is down, so the matcher
advances through it one frame after the direction step. A player who presses
`D+HK` simultaneously — which the game REJECTS, per the rule above — will
still be annotated as having rolled if HK stays down for a second frame. The
matcher is therefore slightly more permissive than the game on exactly the
input the game refuses. Distinguishing them needs a "this step must not be
satisfiable on the same frame as the previous one" rule that the DSL does
not have; until then, annotation counts for these three moves are an upper
bound.

### Disproven readings — do not re-derive these

- **`acid_spit` = `F` · `F+HP` — DISPROVEN.** 0 damage, 16/16 configurations
  (`frames` ∈ {2,3,5,8} × gap ∈ {1,2,3,5}). What comes out is a whiffing HP
  normal. **The 2026-08-28 "VERIFIED (h1 161→137, −24)" above is retracted**:
  that trial ran at point blank, and 24 is exactly what Reptile's *close HP
  normal* does at 62 px in `library/mk2/arcade.frames.json`. The measurement
  was real; the attribution was not. Verifying a projectile at a range where
  a normal also connects cannot distinguish them — this is the same failure
  shape as the retracted `action_counter` and the disproven `p1_x`.
- **`roll` = `B` · `B` · `D+HK` — DISPROVEN.** 0 damage across `frames` ∈
  {2,3,5,8} × gap ∈ {0,1,2,4} and across four step shapes. The game performs
  a crouching normal (`block+0xC0` still goes 160 → 192, so the *button*
  registers — only the special does not).
- **`roll` = `B` · `B+D+HK` — DISPROVEN**, same sweep, same reason.
- **`sai_throw` `min_frames: 180` — wrong by 5×**, see below.
- **Any multi-step special at inter-step gap ≤ 1 — DISPROVEN.** Every motion
  special fails at gap 1 (the taps are not seen as separate) and at gap 0
  (they are one continuous hold). `STEP_GAP = 2` sits exactly on the
  boundary; do not lower it, and do not "optimise" a macro by removing the
  neutral frames.

### Sai Throw's charge is 34 frames, not 180

Bisected at 2-frame resolution, 3 repeats at the boundary: **33 held frames
fails 3/3, 34 fires 3/3.** The projectile then contacts ~22 frames after the
RELEASE, linearly in the hold length (hold 40 → contact f62, hold 180 →
contact f202), which is what proves release is the trigger rather than the
charge completing. 34 frames is ≈0.62 s at this core's 54.71 fps — the
published "hold ~3 seconds" is folklore, and 180 was a transcription of it.

34 is shipped rather than a padded number because `min_frames` is *also* the
matcher's recognition threshold: padding it would make the matcher miss real
sais. It re-fires from six different starting phases (prefix walks of
0/7/13/21/34/55 frames), so the boundary is a property of the move, not of
this one save state. The 55-frame-prefix trial deals 47 = 24 + 23 — she
walked into range, the held HP came out as a close normal *and* the release
still threw the sai. Charge and normal are not exclusive.

### Invisibility has NO memory observable — the framebuffer is the only one

It deals no damage, so the health anchor is useless. Searched for a flag and
did not find one:

- **Framebuffer (works).** `app://screen` at rest, 45 frames after the
  macro: Reptile is simply **not drawn** (Mileena, both health bars, the
  stage and the "REPTILE" nameplate all still render). Quantified as the
  pixel count differing from a standing-control screenshot inside his sprite
  box (`y` 77–239, `x` 225–312): 5255 changed for a successful attempt vs
  ~0–500 against an invisible reference. Swept 64 encodings on that
  classifier: fires at gap ≥ 2 for every `frames` value, with or without the
  `release` step, and whether or not Block is still held with the HP.
  Negative controls stay visible: no Block held (`U U D HP` alone), wrong
  direction sequence (`U U U`), bare HP.
- **Memory (honest negative).** Three invisible runs vs three visible runs,
  240 settle frames each, intersected over `0x0000..0x30000`: 188 candidate
  bytes and **all 188 lie in `0x3270..0x3317`**, a sprite/display list whose
  entries visibly REORDER when a sprite leaves (adjacent 0x18-stride slots
  swap values). Nothing in either fighter struct differs. There is no
  invisibility flag to watch; anything that needs to know Reptile is
  invisible must read the framebuffer or VRAM.

The `release` step is not required by the game — it is kept because it costs
nothing and it is what the published notation says. Note it does mean the
matcher only completes on a Block *release*, so a player who holds Block
through the HP will not be annotated even though his move came out; if that
shows up in real recordings, drop the step rather than "fixing" the matcher.

### Side-swap: it is the ROLL, not the teleport

`MACRO_ACTIONS` §10.2 is written around Mileena's Teleport Kick crossing the
opponent. **It does not cross** — 4/4 trials (starting gap 192 px and after
30/50/65-frame walk-ins) end with Mileena still on the left. She teleports
*vertically*: `y` 87 → 200 (into the ground) → −37 (above the screen) → down
onto the opponent's near side, and the hit pushes HIM away (1119 → 1215)
rather than passing through.

**The `roll` crosses, 3/3**, at every range tried: Mileena ends at `x` 1192
with Reptile at 1002. `side_swap` has been added to its `family.json` tags.
`teleport_kick`'s `side_swap` tag is left in place — the facing pin it
implies is conservative — but it is UNCONFIRMED, and §10.2's motivating
example is the wrong move.

### What will complicate measuring Mileena later

1. **Both her damaging specials are geometry-destroying.** The roll ends
   ~265 px from where it started and on the other side; the teleport moves
   her ~116 px and swings `y` over a 331-unit range (−131 … +200) with
   `x` DISCONTINUOUS (945 → 1013 in one frame). Any gap-keyed protocol
   (docs/frames.md §5) is invalid across either, and the frame lab's
   act-again probe is a WALK, which she cannot do while underground — same
   exclusion that already rules out jumping normals.
2. **The sai's charge collides with normals.** Holding HP to charge also
   throws an HP normal; at any range where that normal connects, the health
   anchor sees two contacts. Measure the sai only from outside normal range
   (192 px works).
3. **`block+0xC0` is a poor discriminator.** It reads 160 → 192 on *entering
   an attack* and then stays 192 — it fires for the crouching normal the
   failed roll produced exactly as it does for the roll. It cannot tell a
   special from the normal it degenerated into; only damage, travel and the
   victim's `y` can.

## Mileena's ladder and her normals (2026-08-31, task M3+M4)

Her own spacing ladder, generated from `shadow/arenas/mk2/m-v-r.state`, and
every standing and crouching normal measured across it on hit AND on block.
Reptile's ladder does not transfer and was not reused: **walk speed is a
property of the character walking and the collision floor is a property of
the two bodies**, and both came out different here.

New in this run, beyond one more character: `guard_height` is no longer NULL
(`docs/frames.md` §12 listed it as unmeasured in every row), the punish rig
is a module rather than an ad-hoc script, and the per-matchup walk curve has
its own tool.

### Rig

Mileena = `block1`, port 0, char_id 5, on the LEFT; Reptile = `block2`,
port 1, char_id 9. Both ports human-live, re-verified after every load.
Headless FBNeo, `--pace 0`, MCP port 4066 (measurement) and 4067 (guard
height, punish rigs, the cold re-measure ran on a second 4066 process). Never
4025. Contact anchored on the fighter-struct health `block+0x0E` (§4.1), one
anchor per rig — the blocked run gets its own, so "hit and block connect on
the same frame" stays a result rather than an assumption. It held in 24 of
the 25 cells; the exception is the first row of the "what the guard changes"
section below, and it is not a timing difference but a WHIFF.

Probe-shape calibrations, measured on this matchup and confirmed hold-limited
at anchor+70 and anchor+100:

| probe shape | `struct_velocity` | `pointer_x` |
|---|---|---|
| attacker (hit rig and block rig) | 1 | 2 |
| defender, on hit | 1 | 2 |
| defender, on block (drops Block, walks) | 10 | 11 |

Identical to Reptile's, which is evidence that these are properties of the
PORT and the probe shape rather than of the character — the first independent
character to test that.

### Her walk curve: 3.125 px/frame, no startup ramp

One continuous hold from the base arena, gap read after every frame
(`framelab.spacing.walk_curve`), cross-checked at K = 0/30/60/90 against an
independent reload-and-walk — all four agree exactly.

| K (walk frames) | gap, mid-walk | K | gap |
|---|---|---|---|
| 0 | 192 px | 30 | 102 px |
| 1 | 192 px | 35 | 86 px |
| 5 | 180 px | 40 | 71 px |
| 10 | 164 px | 42 | 64 px |
| 15 | 149 px | **43** | **61 px** |
| 20 | 133 px | 50 | 63 px |
| 25 | 117 px | 90 | 63 px |

The shape: **one dead frame** (K=0→1 closes nothing), then a flat 3 px/frame
with a 4 px frame every eighth (K = 8, 16, 24, 32, 40) — a mean of
**3.125 px/frame** held from K=1 all the way to contact. There is no
acceleration phase at all.

Against Reptile's own curve (§5: "a ~1.6 px/frame startup ramp, a ~2.5
px/frame cruise from K≈5 to K≈45"), **Mileena walks ~25% faster and starts at
full speed**. Reusing his numbers to place her arenas would have missed every
target gap by a growing margin — at K=45 his curve predicts ~110 px where she
is actually at the floor.

### Her collision floor is 61 px, not 62

She reaches 61 px at **K=43** and no amount of further walking closes it. What
happens past K=43 is worth stating precisely, because a naive "minimum gap
seen" reading gets it wrong: from K=44 the walk starts PUSHING Reptile — his
`x` climbs with hers — and the measured gap then oscillates between 60 and 66
px while both bodies slide right together. Released and settled, the pair
sits at 63 px, i.e. **walking past the floor opens the gap slightly rather
than closing it**.

So `spacing.collision_floor` refuses a "floor" that the curve did not sit on
to the end (the tail must hold the minimum for `plateau_frames`), and the
floor reported here is the settled 61 px at K=43–45 — one pixel tighter than
the 62 px the profile's `framelab.spacing.collision_floor_px` carries for the
Reptile mirror. One pixel is small; the point is that it is a MEASUREMENT and
the ladder tooling now takes it as an argument (`ladder.py --faf-at-px`)
instead of inheriting another matchup's constant.

### The ladder as shipped

`shadow/arenas/mk2/m-gap-{0,15,25,30,35,39,45}.state`, each with a
`.gap.json` sidecar (K, achieved gap, both char ids, facing, `inputs_live`
for both ports, and now `settle_frames` / `reload_after_liveness`). Every one
was re-loaded fresh after saving and required to reproduce its gap and both
char ids exactly, then re-verified again in a separate pass with a fresh
liveness probe on both ports:

| arena | K | gap | arena | K | gap |
|---|---|---|---|---|---|
| `m-gap-0` | 0 | 192 px | `m-gap-35` | 35 | 83 px |
| `m-gap-15` | 15 | 146 px | `m-gap-39` | 39 | 71 px |
| `m-gap-25` | 25 | 114 px | `m-gap-45` | 45 | **61 px** |
| `m-gap-30` | 30 | 99 px | | | |

Two generator fixes were needed to make those gaps mean what they say, and
both are worth knowing because they silently affected the shipped Reptile
ladder:

- **The liveness probe moves the fighters.** It walks each port 6 frames out
  and 6 back, and MK2's forward/backward walk speeds are asymmetric, so it
  leaves the pair closer than the base state did. Measured: `r-v-r.state` is
  a **192 px** arena, and the shipped `gap-0.state` — "K=0", i.e. no walk at
  all — is **180 px**. That 12 px is the probe. `build_gap_ladder_arena` now
  takes `reload_after_liveness`, which re-loads the base state after the
  check (liveness is a property of the RIG, not of a position), and with it
  every Mileena rung's achieved gap matches the walk curve exactly.
- **A rung must be settled.** Saving on the frame after the last held walk
  frame captures a fighter mid-walk-animation, and near the floor the
  measured gap oscillates. `settle_frames=8` (8 neutral frames before the
  save) makes the saved gap the gap the fight starts from; 8 and 20 settle
  frames agree at every K, so 8 is enough.

### The connect map

One anchor replay per (move, rung), `damage@contact-frame`, `—` = the contact
signal never fired. Reproduced identically by the cold re-measure on the
rungs it covered.

| gap | HP | LP | HK | LK | cHP | cLP | cHK | cLK |
|---|---|---|---|---|---|---|---|---|
| 192 px | — | — | — | — | — | — | — | — |
| 146 px | — | — | — | — | — | — | — | — |
| 114 px | — | — | — | 26@f8 | — | — | — | — |
| 99 px | — | — | 32@f8 | 26@f8 | — | — | 12@f20 | — |
| 83 px | 11@f11 | 8@f11 | 32@f8 | 26@f8 | 40@f14 [KD] | — | 12@f20 | 6@f16 |
| 71 px | 11@f11 | 8@f11 | 32@f8 | 26@f8 | 40@f14 [KD] | 6@f17 | 12@f20 | 6@f16 |
| 61 px | **24@f8** | *throw* | **16@f11** | **16@f11** | 40@f14 [KD] | 6@f17 | 12@f20 | 6@f16 |

Her proximity boundary sits between 71 and 61 px for HP, HK and LK — the same
place Reptile's sits for the same buttons (his ladder brackets it as 62/72),
which is more evidence that MK2 resolves proximity once per input at one
distance. The crouching normals have no boundary anywhere in this ladder:
identical damage and contact frame at every rung they reach, so their
`variant` is NULL rather than an invented "close".

`connect_range` (the largest CONNECTING rung, a bracket and not an edge):
LK 114, HK 99, cHK 99, HP 83, LP 83, cHP 83, cLK 83, cLP 71.

**cHK is the reach surprise.** Her crouching HK connects out to 99 px — as
far as her standing roundhouse and further than any punch — for 12 damage at
contact frame 20. Nothing in Reptile's kit was measured at that shape (his
crouching normals were the uppercut and cLK, both short).

**LP at 61 px is a THROW, not a normal.** 30 damage, contact at frame 24, and
the guard rig below settles it: 30 damage with the defender standing-blocking
AND 30 with him crouch-blocking — unblockable. §1.1 gives a throw no
advantage number, so it has no row. (Reptile's own LP-at-62 throw does 34 at
frame 48; hers is faster and weaker.)

### The table

Frames are relative to the contact frame. "att free"/"def free" are the
frames at which that fighter's WALK manifests; advantage is their difference
(§4.3 — raw manifests, no per-side calibration subtracted). Both observables
(`block+0x0B..0x0D` walk velocity and the pointer-resolved `obj+0x12`) were
sampled on every sweep and **agreed to the frame on all 94 sweeps**. `n` = how
many independent full measurements are behind the row; `n=2` means a
cold-started second emulator process reproduced it exactly.

| move | variant | gap | dmg | chip | contact f | FAF | att free | def free (hit) | def free (blk) | **on hit** | **on block** | guard | n |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| HP | far | 71 px | 11 | 3 | 11 | — | +10 | +14 | +23 | **+4** | **+13** | mid | 1 |
| HP | close | 61 px | 24 | 6 | 8 | 8 | +21 | +46 | +19 | **+25** | **−2** | mid | 2 |
| LP | far | 83 px | 8 | 2 | 11 | — | +10 | +14 | +23 | **+4** | **+13** | mid | 1 |
| LP | far | 71 px | 8 | 2 | 11 | — | +10 | +14 | +23 | **+4** | **+13** | mid | 1 |
| HK | far | 99 px | 32 | 8 | 8 | — | +43 | +46 | +23 | **+3** | **−20** | mid | 1 |
| HK | far | 83 px | 32 | 8 | 8 | — | +43 | +46 | +23 | **+3** | **−20** | mid | 1 |
| HK | far | 71 px | 32 | 8 | 8 | — | +43 | +46 | +23 | **+3** | **−20** | mid | 1 |
| HK | close | 61 px | 16 | 4 | 11 | 11 | +33 | +26 | +19 | **−7** | **−14** | mid | 1 |
| LK | far | 114 px | 26 | 6 | 8 | — | +43 | +18 | +23 | **−25** | **−20** | mid | 2 |
| LK | far | 99 px | 26 | 6 | 8 | — | +43 | +18 | +23 | **−25** | **−20** | mid | 1 |
| LK | far | 83 px | 26 | 6 | 8 | — | +43 | +18 | +23 | **−25** | **−20** | mid | 1 |
| LK | far | 71 px | 26 | 6 | 8 | — | +43 | +18 | +23 | **−25** | **−20** | mid | 1 |
| LK | close | 61 px | 16 | 4 | 11 | 11 | +33 | +26 | +19 | **−7** | **−14** | mid | 1 |
| cHP | — | 83 px | 40 | 10 | 14¹ | — | +28 | NULL² | +23 | **NULL²** | **−5** | mid | 1 |
| cHP | — | 71 px | 40 | 10 | 14¹ | — | +28 | NULL² | +23 | **NULL²** | **−5** | mid | 1 |
| cHP | — | 61 px | 40 | 10 | 14¹ | 8 | +28 | NULL² | +23 | **NULL²** | **−5** | mid | 1 |
| cLP | — | 71 px | 6 | 2 | 17¹ | — | +16 | +23 | +19 | **+7** | **+3** | mid | 2 |
| cLP | — | 61 px | 6 | 2 | 17¹ | 11 | +16 | +23 | +19 | **+7** | **+3** | mid | 1 |
| cHK | — | 99 px | 12 | 3 | 20¹ | — | +39 | +14 | +19 | **−25** | **−20** | mid | 1 |
| cHK | — | 83 px | 12 | 3 | 20¹ | — | +39 | +14 | +19 | **−25** | **−20** | mid | 1 |
| cHK | — | 71 px | 12 | 3 | 20¹ | — | +39 | +14 | +19 | **−25** | **−20** | mid | 1 |
| cHK | — | 61 px | 12 | 3 | 20¹ | 14 | +39 | +14 | +19 | **−25** | **−20** | mid | 2 |
| cLK | — | 83 px | 6 | 2 | 16¹ | — | +21 | +10 | +19 | **−11** | **−2** | mid | 1 |
| cLK | — | 71 px | 6 | 2 | 16¹ | — | +21 | +10 | +19 | **−11** | **−2** | mid | 1 |
| cLK | — | 61 px | 6 | 2 | 16¹ | 10 | +21 | +10 | +19 | **−11** | **−2** | mid | 1 |

¹ replay-relative: every crouching normal has a 6-frame stance lead-in, so
its `first_active_frame` is the contact frame minus 6. ² the uppercut
LAUNCHES (the victim's `obj+0x16` leaves its resting y and does not return
until frame 78), and §1.1 gives a knockdown no on-hit advantage — the wakeup
window is the measurement and it is a different, still-unmeasured column.

`first_active_frame` is stored only at the 61 px rung (§4.4): **HP 8, HK 11,
LK 11, cHP 8, cLP 11, cHK 14, cLK 10**.

Every far variant is gap-INVARIANT: HK at 71/83/99 px and LK at 71/83/99/114
px are byte-identical across four rungs, as are all three cHP rows and all
four cHK rows. That is the strongest internal evidence that the protocol
measures the move rather than the arena.

### Blockstun takes exactly two values for her too — the same two

Her defender's walk manifests at **+19** after close HP, close HK, close LK,
cLP, cHK and cLK, and at **+23** after far HP, far LP, far HK, far LK and
cHP. Nothing else. Two values, and they are the SAME two Reptile's kit
produced (+19 and +23) with the same non-alignment to distance — cHK is +19
at 99 px and cLK is +19 at 83 px, while far LP is +23 at 71 px.

So the answer to the question this task asked: **no third value.** Blockstun
on MK2 arcade looks like a two-state property of the MOVE, now measured
across two characters, 13 distinct moves and 25 (move, gap) cells. What it
keys on is not damage (6 damage gives +19 as cLK and +19 as cLP but far LP's
8 gives +23), not the button, and not the gap.

Hitstun, by contrast, takes six values across her kit (+10, +14, +18, +23,
+26, +46) and does not track damage monotonically: her 26-damage far LK frees
the victim at +18 while her 16-damage close HK holds him to +26, and her
6-damage cLP holds him to +23 while her 6-damage cLK frees him at +10. Two
moves, same damage, 13 frames apart — damage is a correlate at best, and the
per-move table is the only honest form.

**`on_hit ≥ on_block` fails again, in both directions**, exactly as §8.2
allows: far HP and far LP are +4 on hit and **+13** on block, while close HP
is +25 on hit and −2 on block. The checker must flag, never reject.

### `guard_height`: measured, and the column is no longer empty

`docs/frames.md` §12: "`guard_height` … NULL in every row measured so far …
needs a CROUCHING defender rig, which was not built." It is built
(`framelab.guard`): three anchor replays per cell — defender open, defender
holding Block, defender holding Block+down — classified from the damage
signature.

**Every one of Mileena's 25 row-bearing cells is `mid`**: standing Block and
crouching Block both reduce the hit to chip, chip being exactly a quarter of
the damage (24→6, 16→4, 40→10, 12→3, 8→2, 6→2, 11→3, 32→8, 26→6). She has no
overhead and no low among her normals — her sweep-shaped cHK is stopped by a
standing block like everything else. The two non-`mid` results are the
interesting ones:

- **LP at 61 px: `unblockable`** — 30 damage against an open defender, 30
  against a standing block, 30 against a crouching block. That is the throw,
  proven rather than assumed.
- **HP at 83 px: `whiffs_vs_guard`** — 11 damage against an open defender,
  chip against a CROUCH-blocking one, and **no contact at all** against a
  standing blocking one. MK2's standing block stance leans the fighter back,
  so at the outer edge of a move's range the connect map is guard-state
  dependent. This is why that cell has no advantage row: the hit rig
  connected and the block rig whiffed, and `measure_cell` correctly refused
  to report an advantage for a move that did not connect (§1.1). A classifier
  that read "the standing blocker took no damage" as "the standing block
  stopped it" would have labelled that move `low` — backwards — so
  `classify_guard` reports the whiff as its own verdict.

### The punish the table predicts, thrown

`framelab.punish`, a rig with no act-again probe in it: the defender's
counter-attack frame is swept and the ATTACKER's damage register says what
happened (full damage = clean punish, chip = her guard was up first, nothing
= no contact).

**First, a protocol correction that the doc needs.** Dropping Block and
pressing the counter on the SAME frame produces **no attack at all**. Swept
against her blocked cHK at 61 px: HP, HK, LK and LP all give zero contact at
every counter frame from contact+8 to contact+30, and Reptile's
`action_counter` never leaves its blocking value — the button never became an
attack. Release the guard at contact+1 and the identical sweep lands. The
first-landing frame is then INVARIANT to when the release happens (measured
identical for release at contact+0, +1, +5 and +10), so this is not a stance
drop that has to finish — it is the same "chorded on the trigger frame does
not register" rule the special-move audit found, applied to Block. A punish
rig without it reports every move in the game as unpunishable, which is
clean, plausible and false.

With that fixed, on the two extremes of the table:

| rig | move | on block | def free (blk) | counter | first landing | attacker's guard up? |
|---|---|---|---|---|---|---|
| block | far HP @71 px | **+13** | +23 | HK | contact+**21** | chip 8 — SAFE |
| block | cHK @61 px | **−20** | +19 | HK | contact+**24** | chip 8 |
| block | cHK @61 px | −20 | +19 | LK | contact+**22** | — |
| block | cHK @61 px | −20 | +19 | HP / LP | **never** | out of range |

Two things fall out, and the second is a caveat on the whole table.

1. **Far HP reproduces Reptile's `manifest − 2` rule exactly**: the defender's
   walk manifests at +23 and his earliest connecting counter is +21. cHK does
   not — its defender manifests at +19 and the earliest counter is +22 (LK) or
   +24 (HK). The difference is that the blocked cHK PUSHES him from 61 px to
   93 px, out of punch range entirely (HP and LP never connect at any counter
   frame), and the remaining kicks arrive later than the −2 rule predicts.
   Recorded rather than smoothed: the −2 rule is not universal.
2. **The counter lands as CHIP in both cases, including on the "−20" move.**
   Mileena holds Block from the frame she threw the move; against cHK the
   counter contacts at +32 and she blocks it, even though her own walk
   manifests at +39. So **her guard comes back before her walk does**, by at
   least 7 frames, and "unsafe by the walk clock" overstates real
   punishability. §1's `punishable` predicate needs both the pushback (which
   it names) and a guard clock (which nobody has measured). What can be said
   from this rig is a bound: her guard is effective by contact+32 after cHK,
   and no counter that reaches can arrive earlier than contact+30.

### Her safest and her most unsafe normal

- **Safest: far HP (and far LP), +13 on block, +4 on hit.** They are the only
  normals in her kit that are PLUS on block, and the punish rig confirms it —
  the fastest counter that reaches cannot beat her guard, it chips. Her close
  HP is the safest close-range option at −2 with the biggest close-range
  reward (+25 on hit, 24 damage), and cLP is the safest crouching poke (+3 on
  block, +7 on hit).
- **Most unsafe: a three-way tie at −20 on block — far HK, far LK, cHK.** By
  the on-HIT column the worst are far LK and cHK at **−25**, i.e. they are
  negative even when they connect. cHK is the one to single out: 12 damage
  for 25 frames of disadvantage on hit, the longest committal in the kit
  (attacker free at +39) — and yet the punish rig cannot punish it at the
  floor, because it shoves the blocker to 93 px. **The most unsafe number in
  the table is not the most punishable move on the screen**, which is exactly
  the range clause §1 warns is not a footnote.

### What was NOT measured, and why

- **HP at 83 px**: no row. It connects against an open defender and WHIFFS
  against a standing-blocking one (above), so there is no on-block number and
  §4.3 forbids deriving one from the on-hit run.
- **LP at 61 px**: the throw. §1.1 gives it no advantage number; it is
  measured as damage + unblockability, nothing more.
- **`on_hit` for cHP at any rung**: the uppercut launches, and a knockdown
  has a wakeup window rather than a hit advantage.
- **Jumping normals**: still out, for the unchanged reason — the act-again
  observable is a WALK and an airborne fighter cannot walk.
- **Her specials**: another task's scope (and `mk2.md`'s special-move audit
  already records why the roll and the teleport break any gap-keyed
  protocol).
- **`hitstop`, `active`, `recovery`, `total`, `wakeup_window`**: still NULL
  in every row. None is a by-product of this protocol. `guard_height` is no
  longer on that list.
- **Gaps 146 px and 192 px**: every button whiffs; they are the whiff half of
  the connect map and carry no rows.
- **`cLP` at 83 px and above, `cLK` at 99 px and above**: they do not reach,
  which the connect map records.

### Cost and provenance

| phase | steps | loads | wall clock |
|---|---|---|---|
| walk curve + settled-gap scan | 3,013 | 62 | ~4 s |
| ladder generation (7 arenas, each verified on reload) | 413 | 21 | ~6 s |
| arena re-verification pass (fresh liveness, both ports) | 168 | 14 | ~2 s |
| connect map (8 moves × 7 rungs, with knockdown probes) | 5,388 | 83 | 9 s |
| **the kit: 4 calibrations + 26 cells at `repeats=2`** | **756,319** | **14,777** | **1,070 s** |
| cold-process re-measure (4 cells) | 135,757 | 2,558 | 190 s |
| `guard_height` (27 cells × 3 stances) | 3,888 | 81 | 7 s |
| punish rigs (7 sweeps) | ~13,000 | ~160 | ~20 s |

**~920k frames and ~17.8k verified loads, ~22 minutes of measurement.** The
kit ran 94 exhaustive sweeps with every `actionable(N)` evaluated TWICE and
required to agree: **0 repeat-check failures, 0 non-monotone refusals, 0
cross-method disagreements, 0 refusals of any kind.** `max_search` was 60
(45 is too tight — her far HK's victim frees at +46).

Rows live in `shadow/framelab/frames.db` and export to
`library/mk2/arcade.frames.json`: **50 Mileena rows** (25 cells × 2
observables) alongside the 20 Reptile rows, each carrying `observable`,
`method`, `input_latency_frames`, `guard_height`, `sample_n`, `core_id`
(`fbneo_libretro.dylib:sha256:972e8fb8c8394979`) and `rom_id`
(`mk2.zip:sha256:e8d3f2f8cefe1aab`).

**One consumer needs updating, and it is not this data.**
`src/profile.rs`'s `mk2_frames_json_parses_and_collapses_agreeing_observables`
asserts the shipped export has exactly **10** cells and that
`table.chars() == ["reptile"]` — a snapshot of a one-character table that any
second character was always going to break. The export now holds **35** cells
across two characters; every one still collapses to exactly two AGREEING
observations, which is the property that test actually exists to check, and
the loader keys cells on `(char, move, variant, gap_walk_frames)` so nothing
collides. The two counts want to become 35 and `["mileena", "reptile"]`. Left
alone here deliberately: this task's file scope excludes Rust.

**Re-measurement (§8.1).** Four cells — close HP @61, far LK @114, cHK @61,
cLP @71 — were measured again from scratch on a COLD emulator process, with
its own calibration, and reproduced every measured column to the frame
(`on_hit`, `on_block`, `damage`, `hits`, `knockdown`, `first_active_frame`,
`gap_px`, `gap_walk_frames`, `input_latency_frames`, `method`, `core_id`,
`rom_id`). The comparator reported two columns differing, both
`connect_range`, and both because the re-run was given three rungs instead of
seven — a smaller ladder brackets the range tighter. That is a difference in
what was ASKED, not in what was measured, and it is the one place
`compare_rows` cannot tell those apart.

### Confidence, per row

- **High** for every `on_block` number and for the `on_hit` numbers of the
  standing normals: two observables in different data structures agreeing on
  94 sweeps, monotone predicates everywhere, every evaluation doubled, and
  four cells reproduced from a cold process.
- **High** for the connect map, damage and chip: single replays, but the cold
  re-measure reproduced every cell it covered and the far variants are
  identical across four rungs each.
- **Medium** for `first_active_frame`: it is the contact frame at the 61 px
  floor minus the stance lead-in, and the ±1 question of whether the damage
  register is written on the overlap frame or the frame after is still
  unresolved (same caveat as Reptile's).
- **Medium** for the punish rig's cHK numbers: the first-landing frames are
  reproducible and invariant to the guard-release lead, but the +22/+24 split
  between LK and HK at the same 8-frame contact delay is unexplained.
- **NULL, not low confidence**, for everything in the "not measured" list.

### What this run says `docs/frames.md` still gets wrong

1. **§4.4/§5 treat the collision floor as a per-PORT constant** and the
   profile stores one (`framelab.spacing.collision_floor_px: 62`). It is
   per-MATCHUP: 61 px here. `ladder.py` now takes `--faf-at-px`, and without
   it this run would have stored `first_active_frame` nowhere and said
   nothing about why.
2. **§5's ladder recipe ("reset to a known position, walk K frames") omits
   that the liveness check itself walks both fighters.** The shipped Reptile
   K=0 rung is 180 px from a 192 px base for exactly that reason. Either
   re-load after the check (what `reload_after_liveness` does) or stop
   calling the base gap "K=0".
3. **§5 says to record the achieved gap and does not say it can OSCILLATE.**
   Within the collision floor the gap swings 60–66 px frame to frame while a
   direction is held. A rung saved on an arbitrary frame is reproducible but
   not meaningful; settle it.
4. **§8.3's punish test cannot be run as written.** Release the guard on the
   counter frame and no counter ever comes out; the acceptance criterion
   would be met vacuously by declaring everything safe.
5. **§1's `punishable` predicate is missing a clock.** It compares advantage
   against the opponent's fastest `first_active_frame` and range, both of
   which this lab measures — but the defender being able to GUARD is what
   actually decides it, and guard returns before the walk this lab probes
   (bound: ≥7 frames earlier for Mileena after cHK). Until there is a guard
   observable, "−20 on block" is an upper bound on punishability, not a
   verdict.
6. **§12's second item (two rows per cell, one per observable) is still
   open**, and this run is more evidence for collapsing them: 94 more sweeps,
   zero disagreements, now across two characters.

## Mileena's three specials — and the three assumptions each one breaks (2026-08-31, task M5)

`docs/frames.md` was written for normals: one button, one frame, both
fighters grounded, both on the sides they started on. Each of Mileena's
specials breaks a different one of those, so this section is as much about
what REFUSED to be measured as about what was.

New code: `shadow_train/framelab/specials.py` only. The differential act-again
protocol (`probe.py`), the §3 preconditions (`session.py`) and the profile's
`framelab` block are unchanged and do the actual work.

### Rig

`shadow/arenas/mk2/m-v-r.state` — Mileena (`block1`, port 0, char_id 5) at
`x` 927, Reptile (`block2`, port 1, char_id 9) at `x` 1119: **192 px**, past
the whiff edge of every measured normal. Headless FBNeo, `--pace 0`, MCP
ports 4068 (first pass) and 4069 (cold-start re-measurement). Never 4025.
Contact anchored on the victim's fighter-struct health `block+0x0E` (§4.1),
one anchor per rig. Both observables (`struct_velocity` `block+0x0B..0x0D`
and pointer-resolved `x` `obj+0x12`) sampled from the same runs.

Encodings come from the profile's `special_inputs` and are played back
exactly as `src/macros.rs::MacroExec` does — each step's mask for its
`frames`, `STEP_GAP = 2` neutral frames between. **All three moves came out on
the first attempt with the shipped encodings**, and the M1 audit's charge
threshold reproduced under this harness's own step convention (33 held frames
fails 2/2, 34 fires 2/2).

**A second, cold-started emulator process reproduced all six measured cells
EXACTLY** — every `first_true`, every direction, every predicate shape, every
advantage, and the charge experiment. `n = 2` for every row below.

### What was measured

Frames are relative to that side's own origin; "att free" / "def free" are
the frames at which that fighter's walk manifests. Advantage is the
difference of the two ABSOLUTE manifest frames (§4.3), which is what makes
the attacker's release-relative clock and the defender's contact-relative
clock comparable.

| move | gap | dmg | chip | contact | att free | def free (hit) | def free (blk) | **on hit** | **on block** | wakeup |
|---|---|---|---|---|---|---|---|---|---|---|
| `sai_throw` | 192 px | 23 | 5 | f58 | release+51 | contact+26 | contact+19 | **−1** | **−8** | — |
| `sai_throw` | 161 px | 23 | 5 | f67 | release+51 | contact+26 | contact+19 | **−5** | **−11** | — |
| `sai_throw` | 130 px | 23 | 5 | f75 | release+51 | contact+26 | contact+19 | **−7** | **−14** | — |
| `sai_throw` | 89 px | 23 | 5 | f85 | release+51 | contact+26 | contact+19 | **−10** | **−17** | — |
| `teleport_kick` | 192 px | 32 | 4 | f38 | contact+31 | contact+26 | contact+19 | **−5** | **−25** | — |
| `roll` | 192 px | 21 | 5 | f33 | contact+23 | *knockdown* | contact+19 | **NULL** | **−34** | **77** |

Twelve rows in `shadow/framelab/frames.db` (six cells × two observables).
`first_active_frame`, `active`, `recovery`, `total`, `hitstop`,
`guard_height` and `connect_range` are NULL in all of them — none of them is
a by-product of this protocol (§4.4, §12).

---

### 1. `sai_throw` — a charge move AND a projectile

**The advantage is a curve, and it decomposes into two constants plus travel
time.** Her recovery is `release + 51` at EVERY rung and the victim's is
`contact + 26` at every rung, so

```
on_hit(gap)   = travel(gap) − 25        on_block(gap) = travel(gap) − 32
travel = contact − release:  24 f @192 px, 20 @161, 18 @130, 15 @89
```

which reproduces all eight measured numbers exactly. The "attacker recovers
while the sai travels" intuition is **false on this port**: 51 frames of
recovery outlast a 24-frame flight, so the sai is negative at every range she
can throw it from — it is merely *less* negative the further away she is.

Her recovery is byte-identical on hit and on block (`release + 51` in both),
which is what a projectile should do — she never touches him — and is a
falsifiable prediction the block rig confirmed rather than assumed.

**The ladder is walk-in, not arenas.** §5's ladder is saved states; a charge
cannot use them (see the pre-charge result below — walking resets the
charge), so each rung is a `lead_in` walk of K frames plus a 3-frame settle
inside ONE replay, replayed identically in probe and control so it cancels
exactly like pushback does. K = 0/10/20/33 → 192/161/130/89 px, gap read at
the frame the charge starts.

**Closer than 89 px is REFUSED, not measured.** Holding HP to charge also
throws an HP normal, and where that normal connects the health anchor sees
two contacts:

| K | gap | contacts | damage | verdict |
|---|---|---|---|---|
| 33 | 89 px | f85 | 23 | measured |
| 35 | 83 px | f49, f87 | 34 = 11 + 23 | **refused** — far HP + sai |
| 40 | 67 px | f54, f92 | 34 = 11 + 23 | **refused** |
| 45 | 61 px | f56, f104 | 47 = 24 + 23 | would refuse — close HP + sai |

The first three rows are the harness's own verdicts; the K=45 row is from the
exploratory boundary scan (same script, same rig) and is included because it
shows the second regime — past the proximity boundary the contaminating
normal is the CLOSE HP (24), not the far one (11).

The refusal is by SIGNATURE (`damage 34 != 23`, `2 contact(s) != 1`) and it
is enforced in code, not in prose: `measure_special` returns with no row.
Mileena's own HP normal connect boundary is therefore between 89 px (clean)
and 83 px (contaminated) — measured here as a by-product, and the reason the
sai's ladder has a floor.

#### The probe CANCELS the move — a hazard normals cannot have

The attacker's sweep from the release frame produced the predicate
`T F F F …`: actionable at N=0, then locked for 45 frames. That is not a
flake and not a recovery. A walk asserted on the exact release frame
**produces 0 damage, in both directions** — the walk preempts the throw, and
the "divergence" the probe saw was her walking instead of attacking. One
frame later the full 23 lands, at every N tried (0..11, both directions).

For a normal this cannot happen: the probe starts AT contact, and a walk
cannot un-throw a move that already hit. For a charge/release move the probe
frame lands INSIDE the move. `preemption_scan` now measures which N kill the
move (by the same contact anchor, so no new instrument), and `sweep_side`
refuses a boundary that lands on one of them. With the origin moved to
`release + 1`, the scan says the move survives at every N from there to
contact, and the sweep is monotone with `first_true = 47` at all four rungs.

Re-verified through the module's own API on the cold-started process:
`{0: False, 1: True, 2: True, 3: True}`, and `sweep_side` refuses the N=0
boundary with the preemption message.

#### The pre-charged arena question, answered — and the control that changes the answer

**The charge lives in the machine state and survives `save_state`/
`load_state` exactly.** Banked at 20 held frames: reloading and holding 13
more does NOT fire; 14 more DOES (20 + 14 = 34, the fresh threshold). Banked
at 33: **one** further frame fires it. Reloaded and left alone for 90 frames:
nothing comes out, so a stale pre-charged arena does not spontaneously throw
a sai — it silently changes the next HP press instead.

**But the save state is not what made that work**, and the control is the
whole finding. Without any save state, splitting a charge as
`20 held | G neutral | 13 held`:

| G | fires? |
|---|---|
| 1 | **yes** (the released frame still counts toward the 34) |
| 2 | no |
| 5, 20, 60 | no (and a fresh 34 always fires) |

An interposed LP, or a 2-frame walk, also resets it. So MK2's charge is an
elapsed-frames counter with a **1-frame release tolerance**, and the reload
is free only because `load_state(pause_after=True)` executes ZERO frames with
the button released before `hold_buttons` + `confirm_fold` re-asserts it.
That is a property of §4.6's atomic load, not of save states in general: a
harness that resumed around the load would lose the charge.

**Cost model for every future charge move.** A pre-charged arena is viable
and cheap — 33 of 34 charge frames banked, one frame per replay instead of 34
— under three conditions: the load never runs a released frame; the arena is
recorded as ARMED (loading it and pressing HP for one frame throws a sai);
and it cannot be combined with a walk-in ladder, because the walk resets the
charge. A pre-charged ladder therefore needs one arena per rung. For this run
the 34 frames were simply paid: at ~0.72 ms/frame batched they cost ~25 ms
per replay against a ~2-minute cell.

---

### 2. `teleport_kick` — airborne, so "actionable" means something else

The flight, per frame: `y` 87 → 200 by f25 (underground), then `x` jumps
945 → 1007 with `y` = −44 (above the screen) in ONE frame, contact at f38 on
the way down, resting `y` regained at **f62**, `x` settling at 995.

The victim's `y` never leaves 89. **This is not a knockdown**, so unlike the
roll it has a real on-hit advantage — the airborne fighter is the ATTACKER.

The act-again probe is a walk and she cannot walk underground, so the
predicate is FALSE through the whole flight for a reason that is not stun.
The honest reading of `first_true = 28` is therefore **"the first frame she
can walk again after landing"**: contact + 31, which is landing + 7. It is a
legitimate advantage number and it is not the same quantity a grounded move's
is; the table says so.

**Its calibration was measured, not inherited.** §3.1 warns that a
wrong-shape calibration produced a confident silent "never actionable" once
already, so the landing-recovery shape was calibrated on this move at
contact+70 and contact+100 and required to agree: `struct_velocity` 1,
`pointer_x` 2 — identical to the grounded attacker shape. That is a result
(the transition really is the same once she is standing), not an assumption,
and it cost ~20 replays to know rather than hope.

Two things the exhaustive sweep bought that an early exit would not:

- **Holding a direction mid-teleport does nothing observable.** Both
  predicates are cleanly monotone — 28 F, then T for all 63 remaining N, in
  both observables, with no divergence at any airborne N. There is no air control to contaminate the probe.
- **She recovers 12 frames LATER when it is blocked** (contact+44 vs
  contact+31), which is why `on_block` is −25 while `on_hit` is −5. On block
  she also lands short (ends at `x` 966 rather than 995) and stays airborne
  until f74 rather than f62. Both observables agree; recorded as measured, and
  flagged as surprising.

---

### 3. `roll` — it swaps sides, and only when it hits

Damage 21 at f33; she rolls from `x` 915 at 10 px/frame, crosses him around
f41, and ends at 1192 with him pushed to 1002. The victim is LAUNCHED —
`y` 89 → −6, back to resting at f73 — so §1.1's knockdown gate applies:
**`on_hit` is NULL with `knockdown` set**, enforced in `special_row` rather
than left to the operator. The probe will happily produce a number there and
it is meaningless.

What is meaningful instead:

- **`wakeup_window` = 77** (`struct_velocity`; 78 by `pointer_x`, see the
  convention note below) — frames from contact to the victim's first manifest
  walk, measured by the identical differential probe, stored in its own
  column. It is the first non-NULL `wakeup_window` in the store.
- **She is free at contact+23**, 54 frames before he can walk. That is the
  wakeup pressure the roll buys, and it is a different number from an
  advantage; it is recorded here in prose rather than squeezed into `on_hit`.

**The side swap is only a swap on hit.** Blocked, she is stopped dead: she
ends at `x` 1016 — still on his LEFT, `crossed = False`, airborne (roll
stance) until f79 instead of f49. So the same move needs OPPOSITE probe walk
directions on the two rigs, and the harness derives them per pass from that
pass's own end positions (`walk_directions_after`, sign of `opp.x − me.x`,
§5's derived facing). The sweeps picked `right` for her on hit and `left` on
block, exactly as the positions require; the pre-move order would have walked
her into his body on the hit rig and read as "not actionable".

**The signature check is load-bearing here.** `block+0xC0` reads 160 → 192
for the roll and for the crouching normal a failed roll degenerates into
(mk2.md, M1). The row is gated on damage 21, exactly one contact, ≥200 px of
attacker travel, a crossing, and the victim leaving its own resting `y` — five
conditions, all measured, and `check_signature` reports every one that fails.

**The calibration point had to move, and the harness moved it by measurement.**
§3.1 says "far enough past the anchor that the fighter is certainly free" and
gives no way to pick that point. For a knocked-down victim contact+70 is
still stun-limited — it calibrates to 7/8 — and contact+100 gives 1/2. Taking
the +70 number would have inflated the wakeup window by 6 frames, silently
and plausibly. `kit.calibrate_shapes`' two-point rule CAUGHT it (the run
refused rather than reporting), and `measure_special` now derives the point
from the observation's own airborne window (`victim_airborne_until + 40`)
instead of a constant.

---

### Cross-checks nobody designed in

1. **Blockstun is `contact + 19` for all three specials** — the sai, the
   teleport and the roll, on a rig none of them shares with a normal. That is
   one of the exactly two blockstun values Reptile's whole kit produced
   (task B3: "+19 close, +23 everything else"), measured here on a different
   character with three different moves.
2. **Hitstun is `contact + 26` for both the sai and the teleport** — two
   moves with completely different geometry and 9 damage between them.
3. **The two observables agreed on `first_true` in all 24 sweeps**, across
   both runs, with no exceptions (48 sweeps counting the cold re-measurement).
   `struct_velocity` and `pointer_x` live in different data structures (§8.4).
4. **The four probe-shape latencies came out identical to the profile's**
   (1/2 attacker, 1/2 defender-on-hit, 10/11 defender-on-block) on every
   move, including the airborne one — but they were re-measured per move
   rather than read from the table, which is what made the knocked-down
   victim's failure visible instead of silent.

### What was NOT measured, and why

- **`sai_throw` closer than 89 px** — refused: her own HP normal connects and
  the anchor sees two contacts (above). This is a property of the move, not a
  gap in the run: the sai has no measurable point-blank row.
- **`teleport_kick` and `roll` at any other gap.** Both moves make `x`
  discontinuous (the teleport jumps 68 px in one frame; the roll ends 265 px
  away on the other side), so §5's gap key is not defined across them. One
  rung, honestly labelled, beats a ladder of numbers keyed on a quantity that
  does not survive the move.
- **`on_hit` for the roll** — NULL by §1.1, with `knockdown` set and the
  wakeup window in its own column.
- **`first_active_frame`** — §4.4 measures it at the minimum reproducible
  gap, which for the sai is inside the contaminated zone and for the other two
  is not a gap that exists.
- **Hitstop, active, recovery, total, `guard_height`.** Still NULL (§12).
- **Reptile's specials.** Out of scope for this task; the harness is
  character-agnostic (`--char`, `--move`) and `acid_spit`'s corrected encoding
  is the obvious next cell.

### Contract gaps in `docs/frames.md` these three moves expose

1. **§4.3's advantage formula assumes ONE origin for both sides.**
   `advantage = manifest(defender, contact) − manifest(attacker, contact)`
   sweeps both sides from the anchor, which silently assumes the attacker
   cannot have committed long before contact. A projectile breaks that: the
   attacker's clock starts at the RELEASE, 24 frames earlier. The fix is
   small and is implemented here — each side carries its own `origin` and
   `origin_kind`, and the difference is taken between ABSOLUTE manifest
   frames — but §4.3 should say so, and the schema has no column for it (§12).
2. **§4.3's signature rule covers the SCRIPT but not the PROBE.** "A move must
   be identified by its measured signature" stops a mislabelled move; nothing
   in the contract stops the probe's own input from CANCELLING the move, which
   is exactly what a walk on the sai's release frame does. Proposed rule: any
   probe frame that falls inside the move's own input window must be validated
   by the contact anchor, and a boundary landing on an invalidated N is void.
3. **§8.4's "agree to the frame" is only true for DIFFERENCES.** An absolute
   per-side number (`actionable_after_contact`, and now `wakeup_window`)
   carries that observable's manifestation margin `m`, so `struct_velocity`
   77 and `pointer_x` 78 are AGREEMENT, not a 1-frame discrepancy — the
   comparable quantity is `first_true` (74 for both). The cross-method check
   is implemented against `first_true` here; §8.4 should name it.
4. **§1.1 reserves `wakeup_window` without defining it.** Proposed, and used
   here: frames from contact to the victim's first manifest walk, same
   observable and window as an advantage row, never stored in `on_hit`.
5. **§3.1's calibration point is not a constant.** "Far enough past the anchor
   that the fighter is certainly free" is 70 frames for a normal and not
   enough for a knockdown. It should be DERIVED from the victim's own airborne
   window, and the two-point agreement check should be mandatory rather than
   `kit`-local — it is the only thing that caught this.
6. **§5's ladder cannot be arenas for a charge move.** Two released frames
   reset the charge, so a walk-in ladder must live inside one replay, and a
   pre-charged ladder needs one arena per rung, each recorded as ARMED.

### One loader gap, outside `docs/frames.md`

`shadow_train.profile` compiles `special_inputs` down to
`{dirs, press, frames}` and **drops the §10.1 kinds** (`hold`, `min_frames`,
`release`, `while_held`). Mileena's `sai_throw` therefore reads, through
`profile.special_inputs` / `macro_steps_for`, as two steps that hold nothing
— a charge move silently compiled into a no-op. `specials.special_encoding`
reads `port_raw` instead and there is a regression test for it, but any other
Python consumer of the compiled view has the same hole.

### Cost and provenance

The full production run — 6 measured cells (each: 2 contact observations, 4
calibrations × 2 points × 5 trials, 4 exhaustive sweeps, a preemption scan),
2 refused rungs, and the charge experiment — cost **400,135 core frames over
3,372 loads in ~12 minutes** on one headless process. `core_id`
`fbneo_libretro.dylib:sha256:972e8fb8c8394979`, `rom_id`
`mk2.zip:sha256:e8d3f2f8cefe1aab`.

Reproduce:

```sh
python -m shadow_train.framelab.specials \
  --url http://127.0.0.1:4069/mcp --game library/mk2 \
  --core ../FBNeo/src/burner/libretro/fbneo_libretro.dylib \
  --rom ~/games/roms/mk2.zip --arena shadow/arenas/mk2/m-v-r.state \
  --char mileena --move teleport_kick --move roll --move sai_throw \
  --rung 0 --rung 10 --rung 20 --rung 33 --rung 35 --rung 40 \
  --charge-probe --db shadow/framelab/frames.db
```

## Baraka: the four moves that broke the DSL (2026-08-30, task A1)

Mileena's audit found mis-transcribed inputs. Baraka's four published moves
are all real and all now verified — but three of them need vocabulary the
macro DSL does not have, and the fourth resolved the one rule
`MACRO_ACTIONS` §11 recorded and could not explain. The encodings are the
smaller half of this section; the specification in "What the DSL must gain"
is the larger one.

### Rig — built, because none existed

No committed arena and no recording had Baraka in it. `shadow/arenas/mk2/
b-v-r.state` was built from a **cold boot** on MCP port **4072** (4025 never
touched): CMOS screen → 8 × `select` (coin) → `start` on port 0 → `start` on
port 1 → both cursors live on CHOOSE YOUR FIGHTER → P1 `down, down, right`
onto **Baraka** → an attack button to lock. **`start` does NOT lock a pick
here** — two `start` presses left both cursors sitting on the grid, and
tapping `y` (HP) locked *both* sides at once. P2 was left on its default
Reptile. (mk2.md's earlier "a `start` press locks P1's pick, this needed 1-6
retries" was written from a 1P flow; in a 2-human select `start` is not the
confirm button at all, which is a simpler explanation than flaky presses.)

**Baraka = char_id 3**, read at the cursor and again in the fight, with
Reptile reading 9 in the same reads — the same corroboration standard as the
rest of the roster. `block1` x 469, `block2` x 661, **192 px apart**, both at
161 health, Dead Pool stage. Liveness verified after saving: port 0 `right`
moved block1 469 → 526 over 20 frames while block2 stayed at 661, port 1
`left` moved block2 661 → 613 while block1 stayed at 469, and a fresh load
reproduced 469/661 and both char ids exactly. `obj+0x3E == block+0x0` on
both fighters, every load.

**Free by-product: `select_slot` is now complete for all 12 fighters.**
Walking P1's cursor over the whole grid and reading `block1+0x0` at every
cell gives, by row: `[liukang, kunglao, johnnycage, reptile]`,
`[subzero, shangtsung, kitana, jax]`, `[mileena, baraka, scorpion, raiden]`
— `slot = row*4 + col`. The two slots family.json already carried (liukang
0, kunglao 1) came out identical; that is the cross-check.

### Baraka's own spacing, measured on this rig

His walk-in ladder (port 0 holding `right` for K frames from the base
arena), and what each of his bare normals does there — this is the whiff map
every verdict below is anchored on. Nothing of Reptile's or Mileena's
transfers; his walk is ~3.0 px/frame and his floor against Reptile is 63 px.

| K | gap | bare HP | bare HK | bare LP |
|---|---|---|---|---|
| 0 | 192 | — | — | — |
| 20 | 132 | — | — | — |
| 25 | 117 | — | — | — |
| 30 | 102 | — | **32** | — |
| 35 | 87 | **11** | 32 | **8** |
| 40 | 72 | 11 | 32 | 8 |
| 42 | 66 | 11 | 32 (far, contact f8) | 8 |
| 43+ | 63 (floor) | 24 (close) | 16 (close, contact f11) | 34 (throw, contact f40) |

### Verdicts

| move | published | shipped | verdict | signature |
|---|---|---|---|---|
| `blade_swipe` | `B + HP` | `[{dirs:["back"], press:["HP"], frames:3}]` | **VERIFIED** | 32 dmg at **102 px**, contact 10 frames after the chord's first frame — a range at which *every* Baraka normal whiffs. Screenshot: the arm blade extends all the way across and lands on Reptile's head. |
| `blade_spark` | `D B HP` | `D` · `B` · `HP`, 3f steps | **VERIFIED** | 24 dmg at **192 px** (only a projectile reaches), contact 24 frames after the HP onset. Screenshot: a purple energy bolt leaves the blades. |
| `double_kick` | *(close, quickly)* `HK HK` | `HK` (9f) · `HK`, + two keys the DSL lacks | **VERIFIED** | **2 hits, 16 + 10 = 26**, at a starting gap ≤ 64 px, with the second press 11–16 frames after the first. The second kick's contact is **5** frames after its own press, against 7 for a far HK and 10 for a close one — a distinct move, not a repeat of the normal. |
| `blade_shredder` | `B B B LP` | `B` · `B` · `B` · `LP`, 3f steps | **VERIFIED** | **40 dmg**, contact **6** frames after the LP onset. Identified by damage and startup rather than by reach (see the note below): at the same 97 px trigger gap the LP normal does 8 at +10, HK 32 at +7, LK 26 at +7, HP and Block nothing, and the close throw does 34 at +39 — no other Baraka input on this rig produces 40, and none produces contact at +6. Screenshot: a low lunging double-blade stab. |

All four were then played exactly the way `src/macros.rs::MacroExec` plays a
macro — each step's mask held for its `frames`, `STEP_GAP = 2` neutral frames
between steps — **3/3 each**, against 5–6 negative controls per move (no
input, bare button, wrong button, wrong direction, one tap short, held
instead of tapped, too-early and too-late repeats). **Zero controls leaked.**

### The §11 exemption, resolved: there is no such thing as a single-frame chord

`MACRO_ACTIONS` §11 records that a direction chorded with its trigger on the
same frame does not register, with two measured exemptions it explicitly
declined to explain: "single-frame chords are exempt" (Reptile's slide) and
"`force_ball`'s two-button chord fires anyway". **Both readings were wrong,
and for different reasons.**

**A chord special needs the direction and the button down TOGETHER for at
least two consecutive frames.** Measured on Blade Swipe at 102 px, where
nothing else connects:

| `back+HP` held for | 1 | 2 | 3 | 4 | 5 | 8 | 12 |
|---|---|---|---|---|---|---|---|
| damage | 0 | 32 | 32 | 32 | 32 | 32 | 32 |

and reproduced **independently on Reptile's slide** (`back+LK+LP+Block` on
`m-v-r.state`, 192 px): 1 frame → 0, 2–12 frames → 13 damage every time. The
slide has always shipped `frames: 8`; it was never a single-frame chord, and
it does not work as one. §11's first exemption described a hold length nobody
had varied.

Three further results pin the mechanism, all at 102 px:

- **It is the CONJUNCTION that needs two frames, not the direction.** Back
  held for 1, 3 or 5 frames and *then* joined by HP for exactly one frame:
  **0 damage, 3/3**. The same prefixes joined by HP for two frames: 32, 3/3.
  Reproduced on the slide (back held 5 frames, chord 1 frame → 0; chord 2
  frames → 13).
- **The trigger is not the button's rising edge.** `HP` pressed one frame
  *before* back, then `back+HP` held two frames, **fires** (32). Order of
  arrival is irrelevant; only two frames of overlap matter.
- **The direction must still be down at the press.** `B` · `HP` sequential,
  at neutral gap 0 and at gap 2, produces a *normal* — 0 at 102 px, and 11
  (the far HP) at 72 px. §11's "the direction need not still be held at the
  press" is a statement about MOTION specials and does not carry over.

**`force_ball` is not an exemption either.** Its `B . B+HP+LP` fires with the
chord held a single frame — because that chord's `back` is the **second tap
of a `B,B` motion**, not a held-direction chord. The controls say so: `B .
HP+LP` = 0, `HP+LP` alone = 0, `F . HP+LP` = 0, `B . F . HP+LP` = 0, `B .
F+HP+LP` = 0, `B . D+HP+LP` = 0, and `B+HP+LP` as a lone chord = 0 at every
hold length 1–8. The move genuinely needs two backs and the chorded one
genuinely counts as the second.

### What is left of the exemption: chorded FINAL taps are per-move

Which leaves a real, still-unexplained split — whether a motion special
accepts its **last direction tap chorded with the trigger**. Six measured
cases, four of them re-measured or newly measured here:

| move | motion | trigger | last tap chorded |
|---|---|---|---|
| reptile `force_ball` | B, B | HP+LP | **fires** |
| reptile `acid_spit` | F, F | HP | does not |
| mileena `teleport_kick` | F, F | LK | **fires** |
| mileena `roll` | B, B, D | HK | does not |
| baraka `blade_spark` | D, B | HP | does not |
| baraka `blade_shredder` | B, B, B | LP | **fires** |

Neither "repeated direction" nor "back-only" survives that table (`acid_spit`
kills the first, `blade_spark` and `teleport_kick` kill the second). The one
predicate that fits all six is **the trigger contains a LOW attack button**:
LP or LK accept the chorded tap, HP or HK alone do not, and `force_ball`'s
`HP+LP` contains LP. That is a hypothesis with a 6/6 fit and **no mechanism**
— recorded as such rather than promoted. It is falsified by any LP/LK-trigger
move that rejects a chorded tap, or any HP/HK-trigger move that accepts one;
whoever finds either should say so here.

It could not be tested by adding LP to `blade_spark`'s trigger, because
`HP+LP` is its own trigger class on this port: `D . B . HP+LP` = 0 and
`B . B . B+LP+HP` = 8 (a normal). Adding a button does not weaken a trigger,
it replaces it.

### Timing, measured — and none of it is expressible today

Every number below was bisected with 3 repeats at the boundary, on the rung
named, with the confound named.

**`double_kick`'s "quickly" is an ONSET-TO-ONSET window of 11–16 frames.**
At the floor (63 px), varying the neutral gap between two 3-frame HK steps:

| onset-to-onset | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 |
|---|---|---|---|---|---|---|---|---|---|
| hits | 1 | 1 | **2** | 2 | 2 | 2 | 2 | **2** | 1 |

10 fails 3/3, 11 fires 3/3, 16 fires 3/3, 17 fails. **It is invariant under
the first press's hold length**: holds of 1, 2, 3, 5 and 8 frames with the
gap compensated to keep onset-to-onset at 13 all give the identical 2-hit
result (16 + 10, contacts at f11 and f19), and the same five holds at
onset-to-onset 9 all fail. So the controlling quantity is the interval
between the two presses' **onsets**, not the neutral gap between steps — the
gap is an executor artifact, the onset interval is the game's rule.

**`blade_spark` is capped by TOTAL SPAN, not by a per-step gap.** From the
first direction onset to the trigger onset, at 192 px:

| span | 6 | 15 | 16 | 17 | 18 | 20 | 22 | 26 |
|---|---|---|---|---|---|---|---|---|
| fires | ✓ | ✓ | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |

Bisected from both ends independently — stretching only `D`→`B` (span 16 ✓,
17 ✗) and stretching only `B`→`HP` (span 16 ✓, 17 ✗) — and confirmed by two
configurations that keep every *individual* interval well inside its own
limit and still fail: `[9, 9]` (span 18) and `[13, 7]` / `[7, 13]` (span 20)
are all misses, while `[13, 3]` and `[3, 13]` (span 16) both fire. **§2's
`max_gap` of 12 frames per step would admit a 24-frame span this move
refuses.**

**`blade_shredder` needs BOTH a per-step cap and a span cap.** Its three back
taps must be ≤ 13 frames apart onset-to-onset (14 fails), its last direction
→ LP interval may be as long as 28 frames (29 fails), and the whole macro
must span ≤ 34 frames:

| intervals | span | result |
|---|---|---|
| `[3, 3, 28]` | 34 | fires |
| `[3, 3, 29]` | 35 | miss |
| `[13, 13, 8]` | 34 | fires |
| `[13, 13, 9]` | 35 | miss |
| `[14, 14, 3]` | 31 | **miss** — span is fine, the direction interval is not |
| `[9, 9, 9]` | 27 | fires |

The last row is what forces two separate rules: a span cap alone would have
admitted `[14, 14, 3]`, and a per-step cap alone would have admitted
`[3, 3, 29]`.

**Fresh onsets are required, and 2 neutral frames is what makes one.** Back
held continuously for 9 or 11 frames then LP → 8 damage (a normal). The three
taps at neutral gap 0 or 1 → 8. At gap 2 (onset-to-onset 3, with 1-frame
taps) → 40. This is the §2 matcher's "fresh direction onset per step"
requirement, confirmed for a three-repeat motion.

**`double_kick`'s "close" is the game's own proximity-normal switch.** Gaps
built by walking BOTH ports (port 0 right, port 1 left) so the ladder is not
quantised to Baraka's 3 px stride:

| gap at the first press | 61 | 62 | 63 | 64 | 65 | 66 | 67 | 69 | 72 |
|---|---|---|---|---|---|---|---|---|---|
| `HK HK` | 2 hits | 2 | 2 | **2** | **1** | 1 | 1 | 1 | 1 |
| bare `HK` | 16 @f11 | 16 | 16 | 16 | **32 @f8** | 32 | 32 | 32 | 32 |

**≤ 64 px available, ≥ 65 px not** — and the boundary is *exactly* the frame
at which the bare HK stops being the close variant (16 damage, 10-frame
startup) and becomes the far one (32 damage, 7-frame startup). The follow-up
links off the CLOSE kick only. At 66 px no inter-press interval works at all
(swept 4, 6, 8, 10, 12, 14, 16, 20, 25, 30 — every one gives a single 32).
The gap that matters is the one at the **first** press: at 63 px the first
kick pushes Reptile out to 69 px before the second press and the link still
fires.

**"Came out and whiffed" and "did not come out" are NOT distinguishable by
`action_counter`.** `block+0xC0` fires twice in *every* `HK HK` trial — at 63
px and at 192 px, inside the timing window and outside it. Both presses always
start an action; what changes is whether the second one is the linked kick or
an ordinary roundhouse. Only the two-hit damage signature and the 5-frame
startup separate them, which is why the DSL needs to express the precondition
rather than hoping a counter will report it.

### Hazard: never measure a special at its reach boundary

Blade Swipe at 117 px is *not reproducible*, and the way it fails is worth
knowing because it looks exactly like a real result. Holding the rig, the
positions and the macro byte-identical and varying only the number of idle
frames between the walk-in and the chord:

| settle frames | 0 | 1–8 | 9 | 10 | 11 |
|---|---|---|---|---|---|
| damage | 32 | 0 | 32 | 32 | 32 |

The defender's idle animation moves his hurtbox in and out of the blade's
last pixel. Worse, the phase reference **drifts across a session**: the same
`settle=3` trial that fired for 32 damage in eight consecutive configurations
early in this session produced 0 damage six times in a row an hour later,
from the same save state, with `block1` x = 544 and `block2` x = 661 in both
— the whole batch that first "verified" the swipe at 117 px no longer
reproduces. `load_state`
does not restore whatever carries that phase. Every verdict above was
therefore re-measured at a rung well inside the connect region (102 px for
the swipe, 192 px for the spark, 97 px for the shredder, the floor for the
double kick), where the result is phase-independent (4/4 across settle 0, 3,
6, 9). **Sitting a special on its whiff edge buys a boundary number and costs
the whole measurement.**

### Disproven readings — do not re-derive these

- **`blade_swipe` as a MOTION (`B` · `HP`) — DISPROVEN.** 0 damage at 102 px
  and at 117 px, at neutral gap 0 and 2; at 72 px it produces exactly the far
  HP normal (11). The back must be held *at* the press.
- **`blade_swipe` as a one-frame chord — DISPROVEN**, 3/3, and the same for
  Reptile's slide. See above.
- **`blade_spark` chorded (`D` · `B+HP`) — DISPROVEN**, 0 damage; also 0 with
  `B+HP+LP` and `B+HP+LK`, and 0 for `D+HP` · `B+HP`. The two-button chord is
  not the mechanism that rescues `force_ball`.
- **`blade_spark` with one direction — DISPROVEN.** `D` · `HP` = 0,
  `B` · `HP` = 0, `D` · `F` · `HP` = 0, `D` · `B` · `LP` = 0 at 192 px.
- **`double_kick` at `STEP_GAP = 2` — DISPROVEN**, and this one matters
  because it is what the executor does by default: two 3-frame HK steps
  separated by 2 neutral frames put the second press 5 frames after the
  first, which is inside the first kick's startup, and the result is a single
  close HK (16 damage) indistinguishable from pressing HK once. The shipped
  encoding uses `frames: 9` on the first step purely to push the second onset
  to +11.
- **`blade_shredder` with two backs, or with a held back — DISPROVEN.**
  `B` · `B` · `LP` = 8 (far LP normal); `B` held 9 or 11 frames then `LP` = 8;
  three taps at neutral gap 0 or 1 = 8. Three *fresh* back onsets are the
  move.
- **`blade_shredder` with a substituted direction — DISPROVEN.**
  `B` · `B` · `F` · `LP`, `B` · `F` · `B` · `LP` and `B` · `B` · `D` · `LP`
  all give 8. Three *identical* backs.
- **`MACRO_ACTIONS` §11's "every motion special fails at gap 1" — CORRECTED,
  not disproven.** It holds for REPEATED-direction motions, where gap 0 and 1
  merge the two taps into one hold. `blade_spark` is `down` then `back` — two
  *different* directions, nothing to merge — and it fires at neutral gap 0
  and 1 as well as 2 (24 damage, 192 px, contact simply arrives earlier).
  `STEP_GAP = 2` is still the right executor constant because it is the one
  value that works for both kinds; the claim that motion specials fail at gap
  1 should be narrowed to repeated directions.

### What the DSL must gain

Three step-scoped keys, all optional, all no-ops when absent, none of which
requires changing the shape of a macro (a macro stays a **list of steps**, so
`Vec<StepSpec>` and the Python compiler keep deserialising). They are already
shipped in `mk2.profile.json` under `baraka`, where both loaders currently
ignore them.

**1. `onset_after: [min, max]`** — the number of frames between the PREVIOUS
step's first frame and THIS step's first frame, inclusive, replacing §2's
single implicit `max_gap` for the steps that need it.

```jsonc
{ "press": ["HK"], "frames": 3, "onset_after": [11, 16] }
```

Justified by: `double_kick` fires at onset-to-onset 11–16 and at nothing
else, 3 repeats at each of the four boundary values, invariant under the
first press's hold length (1/2/3/5/8). **Matcher**: reject a completion whose
step onsets fall outside the interval. **Executor**: choose the neutral gap
so the onset lands inside it, instead of always emitting `STEP_GAP`. Note
that today's `STEP_GAP = 2` cannot play this move at all — the encoding
compensates with `frames: 9`, which is a workaround, not a fix.

**2. `max_span_frames: N`** on the step that completes the macro — the
maximum number of frames from the FIRST step's onset to this one's.

```jsonc
{ "press": ["HP"], "frames": 3, "max_span_frames": 16 }
```

Justified by: `blade_spark` fires at span ≤ 16 and fails at ≥ 17, bisected
independently from both ends, with `[9,9]` and `[13,7]` and `[7,13]` failing
despite every individual interval being legal; `blade_shredder` fires at span
≤ 34 and fails at ≥ 35, from two different interval shapes. A per-step
`max_gap` cannot express either, and §2's 12 frames per step is *looser* than
both moves allow. Both `max_span_frames` and `onset_after` must be checked —
`blade_shredder`'s `[14, 14, 3]` (span 31, legal) fails on the per-step rule
while `[3, 3, 29]` (every interval legal) fails on the span rule.

**3. `requires: { gap_px_max: N }`** on the step it gates — a precondition
evaluated at that step's onset against the live `|opp.x − me.x|` the matcher
and executor already resolve for facing.

```jsonc
{ "press": ["HK"], "frames": 9, "requires": { "gap_px_max": 64 } }
```

Justified by: `double_kick` at ≤ 64 px produces its two hits and at ≥ 65 px
produces nothing but a far normal, at every inter-press interval tried. It is
the first move whose *validity* depends on distance rather than its outcome,
and it needs three consumers to agree:

- **the matcher** must not annotate `double_kick` for a player who pressed
  `HK HK` in rhythm at 120 px — he pressed the input, the move did not exist;
- **the block-punish dummy** (§6) must not offer `double_kick` in its option
  pool when the gap is outside the precondition, or its punish silently
  becomes a whiffing normal;
- **the label space** (§4) should keep the move as a family-level label
  regardless — a character who *can* double-kick has the label even in rounds
  where he was never close enough to use it.

`gap_px_max` is deliberately the only comparator this needs today; a future
`gap_px_min` (for moves that require space) should reuse the same block
rather than inventing a second key.

**What must NOT be added.** A `close`/`far` boolean. The threshold is a
measured pixel count that coincides with this port's proximity-normal switch
for *this* button — 64 px for Baraka's HK against Reptile — and it is a
property of the two bodies, not a mode. `docs/frames.md` §5's gap keys are
the right unit and the measurement already produces them.

### Cost

Eighteen live batches, ~250 macro trials plus the rig build, on one headless
process at `--pace 0`; the whole audit ran in under four minutes of emulated
time. The identification screenshots (blade extended, purple bolt, two kicks,
lunging stab) were taken frame-exactly through the same paused-step protocol.

**One Rust change this data REQUIRES and which task A1 was scoped out of
making:** `src/profile.rs:2061` asserts
`assert_eq!(p.all_specials().len(), 7)`. With Baraka's four encodings shipped
the count is **11**. That single integer is the only thing standing between
this profile and a green `cargo test --profile release-dev`; nothing else in
the suite enumerates specials.
## Baraka's ladder and his normals (2026-08-31, task A3)

His own spacing ladder, generated from a fresh 2-human
`shadow/arenas/mk2/b-v-r.state`, and every standing and crouching normal
measured across it on hit AND on block. Neither existing ladder transfers and
neither was reused: the walk curve is a property of the character walking and
the collision floor of the two bodies, and both came out different again —
**63 px here, against 62 for the Reptile mirror and 61 for Mileena-vs-Reptile.
Three matchups, three floors.**

This was run as the CONVERGENCE TEST: Mileena's run surfaced four corrections
to the measurement contract, and the question this task asked was whether a
third character surfaces none. It surfaced **two**, and one of them is a
property of the GAME rather than of the protocol — Baraka's walk cannot be
aborted for its first 13 frames, which breaks the shipped liveness probe and
makes the ladder's K → px curve discontinuous. Details in "What this run says
`docs/frames.md` still gets wrong", below.

### Rig

Baraka = `block1`, port 0, char_id **3**, on the LEFT; Reptile = `block2`,
port 1, char_id 9. Both ports human-live, re-verified after every load.
Headless FBNeo, `--pace 0`, MCP port 4073 (ladder + the kit) and 4074 (guard
heights, punish rigs, determinism, and a cold-process re-measure). Never 4025.
Contact anchored on the fighter-struct health `block+0x0E` (§4.1), one anchor
per rig.

**The rig had to be built from scratch — no arena or recording had Baraka.**
Cold boot → past the CMOS screen → 4 coins on `select` → `start` on port 0 →
`start` on port 1 (this is what makes it 2-human; a single start gives a
1P-vs-CPU rig, the confusion §4.2 records as having wasted two earlier
sessions) → P1's cursor `down, down, right` from the Liu Kang default. The
select grid is 4 wide, not 6: row 0 = Liu Kang / Kung Lao / Johnny Cage /
Reptile, row 1 = Sub-Zero / Shang Tsung / Kitana / Jax, row 2 = Mileena /
**Baraka** / Scorpion / Raiden. Corroborated two ways before locking, exactly
as mk2.md's roster reads are: the portrait/preview screenshot showed Baraka
and Reptile, and `block1+0x0`/`block2+0x0` read `3`/`9` at the same instant.

**Character select cannot be driven in wall-clock on `--pace 0`.** The first
attempt pressed buttons with `press_buttons` and `sleep`s and watched the
select screen TIME OUT between two presses — uncapped headless runs the
select countdown out in a fraction of a second of host time. Everything from
the CMOS screen onwards is therefore driven paused, with
`hold_buttons` + `run_frames` + `release_buttons`, which is the same
frame-exact discipline §3.3 already requires of the measurement itself.

Probe-shape calibrations, measured fresh on this matchup and confirmed
hold-limited at anchor+70 and anchor+100:

| probe shape | `struct_velocity` | `pointer_x` |
|---|---|---|
| attacker (hit rig and block rig) | 1 | 2 |
| defender, on hit | 1 | 2 |
| defender, on block (drops Block, walks) | 10 | 11 |

**Identical to Reptile's and to Mileena's, on all four shapes and both
observables.** Two characters made that plausible; three make it the
established reading — these are properties of the PORT and the probe shape,
not of the character.

### His walk: exactly 3.0 px/frame, and it CANNOT BE STOPPED for 13 frames

One continuous hold from the base arena, gap read after every frame
(`framelab.spacing.walk_curve`, `max_k=110`):

| K | gap | K | gap | K | gap |
|---|---|---|---|---|---|
| 0 | 192 px | 15 | 150 px | 40 | 75 px |
| 1 | 192 px | 20 | 135 px | 43 | 66 px |
| 5 | 180 px | 25 | 120 px | **44** | **63 px** |
| 10 | 165 px | 30 | 105 px | 60 | 63 px |
| 11 | 162 px | 35 | 90 px | 110 | 63 px |

One dead frame (K=0→1 closes nothing), then a **flat 3.0 px/frame with no
ramp and no irregular frame at all** from K=1 to K=44, then the floor.
`curve_segments` reports exactly three segments: `0/1 @ 0 px`, `1→44 @ 3.0`,
`44→110 @ 0.0`. Against the other two: Reptile ~1.6 ramping to ~2.5, Mileena a
flat 3.125 (3 px/frame with a 4 px frame every eighth). Baraka is the only one
of the three whose curve is a single straight line.

**And then the finding that is not about the curve at all.** Hold a direction
for K frames, release it, and hold NOTHING: Baraka keeps walking.

| release after | travel during hold | travel AFTER release | extra moving frames |
|---|---|---|---|
| 1 frame | +0 px | **+39 px** | **12** |
| 3 frames | +6 px | +39 px | 12 |
| 6 frames | +15 px | +39 px | 12 |
| 10 frames | +27 px | +39 px | 12 |
| **11 frames** | +30 px | **+3 px** | **0** |
| 20 frames | +57 px | +3 px | 0 |

The boundary is exactly between 10 and 11, and it is reproducible. The same
measurement on the same rig shape for the other two characters, from their own
base arenas:

| character | after release, any hold length |
|---|---|
| Baraka (hold ≤ 10) | +39 px over 12 frames |
| Mileena | +3 px, 0 frames |
| Reptile | +2 px, 0 frames |

So Baraka's walk has a **committed opening step of 13 frames** that a release
cannot interrupt; the other two stop on the frame the button does. Two things
follow, and both would have produced confident wrong numbers:

1. **The shipped liveness probe reports him NOT LIVE.** `_probe_port_liveness`
   walks 6 frames out and 6 back and requires ≥2 px of motion in each leg's own
   direction. Baraka's back leg is swamped by the forward step still
   completing, so the probe reads `p0=False` on a rig that is provably live
   (per-frame: −2.15 px/frame holding `left`, +2.85 holding `right`, both
   directions clean, 20/20 frames). Measured threshold: `probe_frames` 6 and 10
   → FALSE; 14, 16, 20 → TRUE. This whole ladder used `probe_frames=20`.
   Since §3 makes the lab REFUSE an arena whose sidecar does not assert
   liveness, the shipped default would have refused to build a Baraka ladder at
   all, for a reason that has nothing to do with liveness.
2. **The settled K → gap curve is DISCONTINUOUS and non-monotone.** The gap a
   rung actually starts a fight from is the SETTLED one (walk K, release, run
   neutral frames), and it is not the continuous curve:

   | K | settled gap | K | settled gap |
   |---|---|---|---|
   | 0 | 192 px | **10** | **126 px** |
   | 1 | 153 px | **11** | **159 px** |
   | 5 | 141 px | 20 | 132 px |
   | | | 43 | 63 px |

   Settled gap is `156 − 3K` for K ≤ 10 and `192 − 3K` for K ≥ 11. **Walking
   one frame LONGER leaves him 33 px further away**, and no K reaches a settled
   gap between 159 and 192 except K=0. Mileena's run added `settle_frames=8`
   and verified 8 and 20 agree; 8 is not enough here — the tail is 12 frames —
   and this ladder used 20.

### His collision floor is 63 px

Reached at K=44 (continuous) / K=43 (settled) and held flat to K=110: 67
consecutive points on 63 px, so `spacing.collision_floor`'s plateau rule
accepts it. **Unlike Mileena's, it does not oscillate**: hers swings 60–66 px
frame to frame inside the floor while a direction is held, and walking past it
opens the gap; his sits on 63 for 67 straight frames of held walk. The floor
is the third distinct value in three matchups (62 / 61 / 63), which is the
point — the profile's `framelab.spacing.collision_floor_px: 62` is one
matchup's measurement and `ladder.py --faf-at-px` exists because of it.

### The ladder as shipped

`shadow/arenas/mk2/b-gap-{0,15,26,31,36,40,43}.state`, each with a `.gap.json`
sidecar (K, achieved gap, both char ids, facing, `inputs_live` for both ports,
`settle_frames`, `reload_after_liveness`). `settle_frames=20`,
`reload_after_liveness=True`, `probe_frames=20`. Ks were chosen to land on
Mileena's gaps where the two floors allow it, so the two tables are comparable
rung for rung.

| arena | K | gap | arena | K | gap |
|---|---|---|---|---|---|
| `b-gap-0` | 0 | 192 px | `b-gap-36` | 36 | 84 px |
| `b-gap-15` | 15 | 147 px | `b-gap-40` | 40 | 72 px |
| `b-gap-26` | 26 | 114 px | `b-gap-43` | 43 | **63 px** |
| `b-gap-31` | 31 | 99 px | | | |

Every rung reproduced its gap and both char ids on a fresh reload at build
time, and again in a separate pass that re-probed liveness on both ports from
the saved file. Every achieved gap matches the settled walk curve exactly.

**Filename collision to be aware of when merging.** Task A1 (Baraka's
specials, same wave) independently built a Baraka-vs-Reptile base arena at the
SAME path, `shadow/arenas/mk2/b-v-r.state`, within the same minute, and its
`.meta.json` on disk now carries A1's provenance rather than this run's. The
two are geometrically identical and the collision is benign — verified on a
fresh cold process at the end of this run, the file on disk loads as char ids
(3, 9) at x 469 / 661, gap 192 px, which is exactly the base every rung here
was walked from, and each rung independently reproduces its own gap from its
own file (`b-gap-43` → 63 px, `b-gap-31` → 99 px, `b-gap-0` → 192 px). But two
tasks writing one arena filename is luck, not design; a matchup base arena
wants the task or the date in its name the way the ladder rungs carry their
`b-` prefix.

The app's own auto-written `b-gap-*.meta.json` sidecars were DELETED, for the
reason §11 already records: they assert `inputs_live: {p0: false, p1: false}`
— they read MK2's disproven `p1_x`/`p2_x` globals — while this module's
`.gap.json` correctly says true/true. A wrong sidecar is worse than an absent
one. (`b-v-r.meta.json` is KEPT: the base arena was saved while the emulator
was running, and the app's probe got it right there — true/true, healths
161/161, `gate_open: true`.)

### The connect map

One anchor replay per (move, rung), `damage@contact-frame`, `—` = the contact
signal never fired. **Measured at `anchor_frames=90`, not the shipped 48** —
see the throw, below.

| gap | HP | LP | HK | LK | cHP | cLP | cHK | cLK |
|---|---|---|---|---|---|---|---|---|
| 192 px | — | — | — | — | — | — | — | — |
| 147 px | — | — | — | — | — | — | — | — |
| 114 px | — | — | — | 26@f8 | — | — | — | — |
| 99 px | 11@f11 | 8@f11 | 32@f8 | 26@f8 | — | — | — | — |
| 84 px | 11@f11 | 8@f11 | 32@f8 | 26@f8 | 40@f14 [KD] | 6@f14 | 12@f20 | 6@f16 |
| 72 px | 11@f11 | 8@f11 | 32@f8 | 26@f8 | 40@f14 [KD] | 6@f14 | 12@f20 | 6@f16 |
| 63 px | **24@f14** | ***34@f40 [KD]*** | **16@f11** | **16@f11** | 40@f14 [KD] | 6@f14 | 12@f20 | 6@f16 |

`connect_range` (the largest CONNECTING rung — a bracket, not an edge):
LK 114, HP 99, LP 99, HK 99, cHP 84, cLP 84, cHK 84, cLK 84.

His proximity boundary for HP, HK and LK sits between 72 and 63 px, the same
place Mileena's (71/61) and Reptile's (72/62) sit — a third character agreeing
that MK2 resolves proximity once per input at one distance. His crouching
normals have no boundary anywhere in the ladder (identical damage and contact
frame at every rung they reach), so their `variant` is NULL rather than an
invented "close".

**Two differences from Mileena worth naming.** His crouching normals reach only
to 84 px where hers reached to 99 (her cHK was the reach surprise; his is not).
And his close HP contacts at **f14** where hers contacts at f8 — same 24
damage, six frames slower.

**`LP` at 63 px is a THROW, and the shipped connect map called it a whiff.**
34 damage, contact at frame **40**, and the guard rig settles it: 34 against an
open defender, 34 against a standing block, 34 against a crouching block —
`unblockable`. It also knocks down. §1.1 gives a throw no advantage number, so
it has no row. What matters beyond Baraka is HOW it was missed: `find_anchor`
needs `quiet_frames` (20) of silence after the contact cluster inside the
trace, so `DEFAULT_ANCHOR_FRAMES = 48` can only see a contact at frame ≤ 28.
Mileena's throw contacts at f24 and squeaked in; Baraka's at f40 does not, and
the map printed `—`, the identical glyph a genuine whiff gets. Reptile's own
close-LP throw is recorded in mk2.md at frame 48 and is outside the window
too. A full re-scan of all 56 (move, rung) cells at `anchor_frames=90` found
exactly one cell the 48-frame map had hidden — this one — so nothing else in
the table is affected. `framelab.ladder.connect_map` and both CLIs now take
`--anchor-frames`; the default is unchanged, so every earlier row stays
comparable.

### The table

Frames are relative to the contact frame. "att free"/"def free" are the frames
at which that fighter's WALK manifests; advantage is their difference (§4.3 —
raw manifests, no per-side calibration subtracted). Both observables
(`block+0x0B..0x0D` walk velocity and the pointer-resolved `obj+0x12`) were
sampled on every sweep and **agreed to the frame on all 90 sweeps**.

| move | variant | gap | dmg | chip | contact f | FAF | att free | def free (hit) | def free (blk) | **on hit** | **on block** | guard | n |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| HP | far | 84 px | 11 | 3 | 11 | — | +10 | +14 | +23 | **+4** | **+13** | mid | 1 |
| HP | far | 72 px | 11 | 3 | 11 | — | +10 | +14 | +23 | **+4** | **+13** | mid | 1 |
| HP | close | 63 px | 24 | 6 | 14 | 14 | +24 | +46 | +19 | **+22** | **−5** | mid | 2 |
| LP | far | 84 px | 8 | 2 | 11 | — | +10 | +14 | +23 | **+4** | **+13** | mid | 1 |
| LP | far | 72 px | 8 | 2 | 11 | — | +10 | +14 | +23 | **+4** | **+13** | mid | 1 |
| HK | far | 84 px | 32 | 8 | 8 | — | +39 | +46 | +23 | **+7** | **−16** | mid | 1 |
| HK | far | 72 px | 32 | 8 | 8 | — | +39 | +46 | +23 | **+7** | **−16** | mid | 2 |
| HK | close | 63 px | 16 | 4 | 11 | 11 | +20 | +26 | +19 | **+6** | **−1** | mid | 1 |
| LK | far | 99 px | 26 | 6 | 8 | — | +39 | +18 | +23 | **−21** | **−16** | mid | 2 |
| LK | far | 84 px | 26 | 6 | 8 | — | +39 | +18 | +23 | **−21** | **−16** | mid | 1 |
| LK | far | 72 px | 26 | 6 | 8 | — | +39 | +18 | +23 | **−21** | **−16** | mid | 1 |
| LK | close | 63 px | 16 | 4 | 11 | 11 | +20 | +26 | +19 | **+6** | **−1** | mid | 1 |
| cHP | — | 84 px | 40 | 10 | 14¹ | — | +24 | NULL² | +23 | **NULL²** | **−1** | mid | 1 |
| cHP | — | 72 px | 40 | 10 | 14¹ | — | +24 | NULL² | +23 | **NULL²** | **−1** | mid | 1 |
| cHP | — | 63 px | 40 | 10 | 14¹ | 8 | +24 | NULL² | +23 | **NULL²** | **−1** | mid | 1 |
| cLP | — | 84 px | 6 | 2 | 14¹ | — | +13 | +23 | +19 | **+10** | **+6** | mid | 1 |
| cLP | — | 72 px | 6 | 2 | 14¹ | — | +13 | +23 | +19 | **+10** | **+6** | mid | 2 |
| cLP | — | 63 px | 6 | 2 | 14¹ | 8 | +13 | +23 | +19 | **+10** | **+6** | mid | 1 |
| cHK | — | 84 px | 12 | 3 | 20¹ | — | +39 | +14 | +19 | **−25** | **−20** | mid | 1 |
| cHK | — | 72 px | 12 | 3 | 20¹ | — | +39 | +14 | +19 | **−25** | **−20** | mid | 1 |
| cHK | — | 63 px | 12 | 3 | 20¹ | 14 | +39 | +14 | +19 | **−25** | **−20** | mid | 2 |
| cLK | — | 84 px | 6 | 2 | 16¹ | — | +21 | +10 | +19 | **−11** | **−2** | mid | 2 |
| cLK | — | 72 px | 6 | 2 | 16¹ | — | +21 | +10 | +19 | **−11** | **−2** | mid | 1 |
| cLK | — | 63 px | 6 | 2 | 16¹ | 10 | +21 | +10 | +19 | **−11** | **−2** | mid | 1 |

¹ replay-relative: every crouching normal has a 6-frame stance lead-in, so its
`first_active_frame` is the contact frame minus 6. ² the uppercut LAUNCHES
(the victim's `obj+0x16` leaves its resting y and does not return until frame
78) and §1.1 gives a knockdown no on-hit advantage.

`first_active_frame` is stored only at the 63 px rung (§4.4): **HP 14, HK 11,
LK 11, cHP 8, cLP 8, cHK 14, cLK 10.**

Every far variant is gap-INVARIANT: HK at 72/84 px, LK at 72/84/99 px, HP and
LP at 72/84 px, and all three rows each of cHP/cLP/cHK/cLK are byte-identical
across their rungs.

**Where he differs from Mileena, move for move.** Same far punches to the
frame (+4/+13). His far HK recovers 4 frames sooner (+39 vs +43) so it is
+7/−16 where hers is +3/−20; his far LK likewise −21/−16 against her −25/−20.
His close kicks are a different move entirely in frame terms: att free +20
against her +33, giving **+6/−1** where she has −7/−14. His close HP is slower
to contact (f14 vs f8) and slightly worse (+22/−5 vs +25/−2). His cLK and his
cHK reproduce her numbers EXACTLY (−11/−2 and −25/−20). His cLP is 3 frames
better (+10/+6 vs +7/+3), his cHP 4 frames better on block (−1 vs −5).

### Blockstun takes exactly two values for him too — the same two

His defender's walk manifests at **+19** after close HP, close HK, close LK,
cLP, cHK and cLK, and at **+23** after far HP, far LP, far HK, far LK and cHP.
Nothing else.

**So: no third value.** Blockstun on MK2 arcade is now measured across THREE
characters and 59 (move, gap) cells — Reptile's 10, Mileena's 25 and Baraka's
24 — and it has taken exactly two values — +19 and +23 — with the same non-alignment to distance,
damage or button. Baraka's split is identical to Mileena's, move class for
move class, including the oddity that the far normals (23) are stunnier than
the close ones (19).

Hitstun is the contrast, again: **+10, +14, +18, +23, +26, +46** — six values,
and the SAME six Mileena's kit produced. It does not track damage (his
6-damage cLK frees the victim at +10 while his 6-damage cLP holds him to +23;
his 32-damage far HK holds to +46 while his 26-damage far LK frees at +18).

**`on_hit ≥ on_block` fails again, in both directions**, exactly as §8.2
allows: far HP and far LP are +4 on hit and **+13** on block, while close HP is
+22 on hit and −5 on block. Flag, never reject.

### `guard_height`

Three anchor replays per cell (`framelab.guard`): defender open, defender
holding Block, defender holding Block+down.

**Every one of Baraka's 24 row-bearing cells is `mid`**, and chip is exactly a
quarter of the damage in all of them (40→10, 32→8, 26→6, 24→6, 16→4, 12→3,
11→3, 8→2, 6→2). No overhead, no low; his sweep-shaped cHK is stopped by a
standing block like everything else. The non-`mid` results are the interesting
ones:

- **LP at 63 px: `unblockable`** — 34 / 34 / 34. That is the throw, proven
  rather than assumed (and only visible at `anchor_frames=90`).
- **`whiffs_vs_guard` at the OUTER rung of every standing normal**: HP@99,
  HK@99 and LK@114 reach an open defender and make no contact at all against a
  standing-blocking one; LP@99 whiffs against a standing block but chips a
  CROUCH-blocking one. Mileena's kit produced exactly one such cell (HP@83);
  Baraka's produces four, one for each standing normal, all at that move's
  outermost connecting rung. This is the same mechanism — MK2's standing block
  stance leans the fighter back — and it is why those four cells have no
  advantage rows: the hit rig connected and the block rig whiffed, and
  `measure_cell` correctly refuses to report an advantage for a move that did
  not connect. A classifier reading "the standing blocker took no damage" as
  "the standing block stopped it" would have labelled all four `low`.

### The punish the table predicts, thrown

`framelab.punish` (no act-again probe in it): sweep the defender's
counter-attack frame and let the ATTACKER's damage register say what happened
— full damage = clean punish, chip = the attacker's guard was up first,
nothing = no contact. Guard released at contact+1, one frame before the
earliest counter, per §8.3's correction.

| rig | move | on block | def free (blk) | counter | first landing | attacker took |
|---|---|---|---|---|---|---|
| block | cHK @63 px | **−20** | +19 | HK | contact+**24** | chip 8 |
| block | cHK @63 px | −20 | +19 | LK | contact+**24** | chip 6 |
| block | cHK @63 px | −20 | +19 | HP | contact+**24** | chip 3 (only +24…+31) |
| block | cHK @63 px | −20 | +19 | LP | **never** | — |
| block | cHK @63 px | −20 | +19 | HK, guard HELD to the counter frame | **never** | — |
| block | far HP @72 px | **+13** | +23 | HK | contact+**21** | chip 8 |
| block | far HP @72 px | +13 | +23 | LK | contact+**21** | chip 6 |
| **control** | cHK @63 px, attacker does NOT guard | −20 | +19 | HK | contact+**24** | **full 32** |
| **control** | far HP @72 px, attacker does NOT guard | +13 | +23 | HK | contact+**21** | **full 32** |

Four things fall out.

1. **§8.3's guard-release correction reproduces on a third character.** Hold
   Block to the counter frame and the counter never comes out — zero contact at
   every N from +8 to +40. Release it one frame after contact and the identical
   sweep lands from +24. A punish rig without that rule reports Baraka's entire
   kit as unpunishable.
2. **Far HP reproduces the `manifest − 2` rule exactly**: defender manifests at
   +23, earliest connecting counter +21. cHK does not — manifest +19, earliest
   counter +24 — the same direction Mileena's cHK missed it in (+22/+24 against
   a +19 manifest). Two characters now, same move shape, same failure of the
   rule; it is not universal and cHK is where it breaks.
3. **The counter LANDS, and it lands as chip, and the two controls prove which
   clause is doing the work.** With the attacker guarding after the move, every
   counter that connects does exactly a quarter damage. With
   `--no-attacker-guard`, the SAME counter at the SAME first-landing frame does
   the full 32. So the counter is in range and on time; what stops it is that
   **Baraka's guard is back by contact+24 while his walk does not manifest
   until +39 — a ≥15-frame gap**, against the ≥7 frames Mileena's cHK bounded.
   This is a cleaner instance of §1's missing guard clock than Mileena's,
   because hers was confounded with range: her blocked cHK shoved the defender
   to 93 px where her punches could not reach at all, and the question of
   whether the kicks were stopped by range or by guard was left open. Baraka's
   is not confounded — the no-guard control lands full damage from the same
   distance.
4. **Pushback is real but not decisive here.** Measured through the object
   pointer during the blocked replay: cHK @63 px shoves the pair to **95 px**
   (Mileena's shoves to 93), far HP @72 px to 81 px, and close HP @63 px not at
   all. 95 px is still inside HK's 99 px connect range, which is why his kicks
   keep landing where hers stopped. His HP counter connects only from +24 to
   +31 and then stops — recorded, not explained; 3 damage is chip either way.

### His safest and his most unsafe normal

- **Safest: far HP and far LP, +13 on block, +4 on hit.** They are the only
  normals in his kit that are PLUS on block, and the punish rig confirms it:
  the fastest counter that reaches arrives at +21, eleven frames after he is
  already free at +10, and it chips. **His close kicks are the real surprise of
  the kit**: close HK and close LK are **+6 on hit and −1 on block** for 16
  damage at FAF 11 — a nearly-neutral, 11-frame close-range poke that Mileena
  does not have (hers are −7/−14). And cLP is the safest crouching poke in
  either character's table at **+6 on block, +10 on hit**.
- **Most unsafe: cHK, −20 on block and −25 on hit**, for 12 damage and the
  longest committal in the kit (attacker free at +39). Second are far HK and
  far LK at −16 on block; by the on-HIT column far LK is −21, i.e. negative
  even when it connects.
- **Is the most unsafe one actually punishable? No — and this time it is
  purely the guard clock.** By the walk clock cHK is −20 and the defender's
  earliest counter (+24) beats the attacker's walk (+39) by 15 frames. The
  counter is in range (95 px after pushback, inside HK's 99 px) and it does
  land. It has never done more than chip damage in any sweep, because Baraka's
  guard is effective by contact+24. The bound this rig can state: **his guard
  is up by contact+24 after a blocked cHK, and no counter that reaches can
  arrive earlier than contact+24.** So "−20 on block" is, for a third
  character, an upper bound on punishability rather than a verdict — the third
  clause §1 grew after Mileena's run, now demonstrated without the range
  confound that motivated it.

### What was NOT measured, and why

- **HP@99, LP@99, HK@99, LK@114**: no rows. Each connects against an open
  defender and WHIFFS against a standing-blocking one, so there is no on-block
  number and §4.3 forbids deriving one from the on-hit run.
- **LP at 63 px**: the throw. §1.1 gives it no advantage number; it is measured
  as damage + contact frame + unblockability, nothing more.
- **`on_hit` for cHP at any rung**: the uppercut launches, and a knockdown has
  a wakeup window rather than a hit advantage.
- **`wakeup_window`**: not measured for any Baraka move, including the two that
  knock down (cHP and the LP throw). The column stays NULL.
- **Jumping normals**: still out — the act-again observable is a WALK and an
  airborne fighter cannot walk.
- **His specials** (Blade Fury, Blade Swipe, Blade Spin): another task's scope.
- **`hitstop`, `active`, `recovery`, `total`**: still NULL in every row. None
  is a by-product of this protocol.
- **Gaps 192 px and 147 px**: every button whiffs; they are the whiff half of
  the connect map and carry no rows.
- **The K ≤ 10 half of his settled walk curve**: characterised (above) but no
  arena was built there. The rungs at 126–153 px are reachable, and would need
  their K chosen off the `156 − 3K` branch rather than the `192 − 3K` one.

### Cost and provenance

| phase | steps | loads | wall clock |
|---|---|---|---|
| cold boot → char select → 2-human fight, verified | ~2,000 | 0 | ~40 s |
| walk curve + settled-gap scan (K 0–60 + 5 more) | 3,675 | 67 | 4.7 s |
| walk-commitment characterisation (3 characters) | ~3,700 | 70 | ~10 s |
| liveness-probe threshold sweep (`probe_frames` 6→20) | ~800 | 10 | ~2 s |
| ladder generation (7 arenas, each verified on reload) | ~1,100 | 21 | 1.4 s |
| arena re-verification pass (fresh liveness, both ports) | ~600 | 21 | ~2 s |
| connect map, 48-frame (8 moves × 7 rungs) | 5,488 | 84 | 9.4 s |
| connect map, 90-frame re-scan (the throw hunt) | 7,940 | 85 | 13.7 s |
| **the kit: 4 calibrations + 28 cells at `repeats=2`** | **726,644** | **14,167** | **1,025.9 s** |
| `guard_height` (29 cells × 3 stances) | ~4,600 | 90 | ~8 s |
| determinism check (2 scopes × 2 rigs) | 480 | 8 | ~1 s |
| punish rigs (9 sweeps) + pushback replays | ~24,600 | ~310 | ~36 s |
| cold-process re-measure (6 cells) | 202,672 | 3,849 | 278.1 s |

**≈985,000 frames and ≈18,800 verified loads, ≈24 minutes of
measurement.** The kit ran 90 exhaustive sweeps with every `actionable(N)`
evaluated TWICE and required to agree: **0 repeat-check failures, 0
non-monotone refusals, 0 cross-method disagreements, 0 refusals of any kind.**
`max_search` was 60 (his far HK's victim frees at +46, so 45 is too tight —
same reason as Mileena's).

Rows live in `shadow/framelab/frames.db` and export to
`library/mk2/arcade.frames.json`: **48 Baraka rows** (24 cells × 2 observables)
alongside Mileena's 62 and Reptile's 20, 130 rows total, each carrying
`observable`, `method`, `input_latency_frames`, `guard_height`, `sample_n`,
`core_id` (`fbneo_libretro.dylib:sha256:972e8fb8c8394979`) and `rom_id`
(`mk2.zip:sha256:e8d3f2f8cefe1aab`). The export was regenerated from the store
at the end of the run and verified to contain all 48 — the store and the export
silently diverged once before.

**Determinism (§4.6): all clear, and at two scopes.** One scripted replay run
twice from one state, on `b-gap-43`, hit rig and block rig, compared over (a)
both fighters' whole 0xD0-byte structs and (b) exactly the two profile
observables: identical in all four pairs, 60 frames each.

**Re-measurement (§8.1).** Six cells — close HP @63, far HK @72, far LK @99,
cHK @63, cLP @72 and cLK @84 — were measured again from scratch on a COLD
emulator process (killed and relaunched), with its own fresh calibration
(which came out identical again) and the same seven-rung connect map so that
`connect_range` would be comparable. `compare_rows` reports **0 of 12 compared
rows disagree**: every measured column reproduced to the frame (`on_hit`,
`on_block`, `damage`, `hits`, `knockdown`, `first_active_frame`,
`connect_range`, `gap_px`, `gap_walk_frames`, `input_latency_frames`,
`method`, `core_id`, `rom_id`). The verdict line still prints NOT IDENTICAL
because the comparison is against the WHOLE export and this run was not asked
for Mileena's or Reptile's cells — those are listed as `MISSING`, which is
"not produced by this run", not "disagreed". Those twelve rows carry
`sample_n = 2`.

### Confidence, per row

- **High** for every `on_block` number and for the `on_hit` numbers of the
  standing normals: two observables in different data structures agreeing on 90
  sweeps, monotone predicates everywhere, every evaluation doubled, and six
  cells reproduced from a cold process.
- **High** for the connect map, damage and chip: single replays, but the far
  variants are identical across their rungs and the cold re-measure reproduced
  every cell it covered.
- **High** for the walk-commitment finding: it reproduces at every hold length
  1–26, the boundary is a single frame, and the two other characters measured
  on the identical rig shape do not show it.
- **Medium** for `first_active_frame`: it is the contact frame at the 63 px
  floor minus the stance lead-in, and the ±1 question of whether the damage
  register is written on the overlap frame or the frame after is still
  unresolved (same caveat as Reptile's and Mileena's).
- **Medium** for the punish rig's cHK numbers: first-landing frames are
  reproducible and the guarded/unguarded controls bracket the verdict cleanly,
  but HP's connecting window (+24…+31 and then nothing) is unexplained.
- **NULL, not low confidence**, for everything in the "not measured" list.

### What this run says `docs/frames.md` still gets wrong

The convergence question this task asked was whether a third character
surfaces any NEW contract gap. It surfaced **two**, and they are of different
kinds — one is a property of the game the protocol had never met, the other is
a silent cap in the tooling.

1. **§5's ladder recipe assumes a walk STOPS when the button is released.**
   Baraka's does not: his opening step is committed for 13 frames, so a rung
   saved with the shipped `settle_frames=8` is captured mid-glide, and the
   settled K → gap curve is discontinuous and NON-MONOTONE (K=10 settles 33 px
   closer than K=11). §5 already says "settle before saving" because of
   Mileena's oscillation; the settle it needs here is a different quantity —
   the character's own walk-stop commitment — and 8 frames is not it. The same
   commitment breaks `_probe_port_liveness` outright at its default
   `probe_frames=6`: Baraka reads NOT LIVE on a provably live port, and §3
   makes the lab refuse such an arena. A liveness probe whose leg is shorter
   than the walk it probes is measuring the wrong thing; the leg must exceed
   the character's commitment (14 frames is the measured threshold here, 20 was
   used).
2. **`DEFAULT_ANCHOR_FRAMES = 48` is a silent cap, and it hid a whole move.**
   With `quiet_frames = 20` the reachable contact window is 28 frames, and MK2's
   throws are outside it: Baraka's contacts at f40, Reptile's at f48. The
   connect map printed `—` — the identical glyph a genuine whiff gets — for an
   unblockable 34-damage knockdown. §7's "no silent caps" is about a run
   REPORTING what it skipped; this is the shape where the run does not know it
   skipped anything. `framelab.ladder.connect_map` and the `ladder`/`guard`
   CLIs now take an explicit `--anchor-frames` (default unchanged, so earlier
   rows stay comparable), and the operator rule is: a `—` at the collision
   floor is the least likely place for a genuine whiff, so widen the window
   before believing it.

Everything else was MECHANICAL, and that is the more important half of the
answer. The four corrections Mileena's run produced all held without
amendment: the two-point hold-limited calibration, the raw-manifest advantage,
the guard-release lead in the punish rig, and the "negative on block is an
upper bound" clause (which this run demonstrated more cleanly than she did).
The per-shape calibrations came out identical for a third character. Blockstun
took the same two values. The cross-observable check passed on all 90 sweeps.
No new failure mode appeared anywhere in the sweep, the store, or the export.

Two items from §12 are also worth re-affirming with a third character's data:
**two rows per cell, one per observable, is still open** and this run adds 90
more sweeps with zero disagreements; and `guard_height` is now populated for
three characters and has produced exactly one non-`mid` verdict per character
kind — `unblockable` for the throw, `whiffs_vs_guard` at the geometric edge.
