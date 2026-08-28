---
schema_version: 1

rom:
  name: "Mortal Kombat II (World)"
  system: "Sega Genesis / Mega Drive"
  archive: "md_mk2.zip"
  crc32: "a9e013d8"
  fbneo_short_name: "md_mk2"   # FBNeo Megadrive driver

settings: {}

meta:
  genre: fighting
  year: 1994
  developer: "Probe Entertainment / Sculptured Software (port), Midway (original)"
  progress: "Memory-RE passes W1 + W2 (2026-08-27, headless FBNeo Megadrive
    driver on MCP port 4032). Fighter structs (char id, health x2 incl.
    mirror, WORLD X, Y), the AUTHORITATIVE ROUND TIMER, a menu-phase gate
    discriminator, and the full 6-button attack/block table are LIVE-VERIFIED
    (teleport/levitate/timeout write-tests where marked). Open unknowns:
    facing, wins-per-player, round_over write-test, roster ids beyond
    1/3/7/9."
  tags: [genesis, megadrive, 2d-fighter, m68k, fbneo, port]
---

## Overview

Mortal Kombat II on Genesis runs on FBNeo's Megadrive driver: a stock 68000
(`m68k`) at the standard console address map, with work RAM exposed
DIRECTLY at guest address `0xFF0000` (community "FFxxxx" addresses need no
translation, unlike the arcade port's TMS34010 bit-addressing). The MCP
`list_regions`/`get_state` tools show one region named `RAM`,
`addr_start=0xFF0000`, size 205348 bytes — bigger than the 68k's 64 KiB work
RAM, because it also carries Z80/sound and other debug-visible spans appended
after it. **Every address below is a plain byte offset in this window**,
confirmed to sit within the first 64 KiB (`0xFF0000-0xFFFFFF`), i.e. real
m68k work RAM. `has_vram`/`has_rom` are both `false` for this core — there is
no way to inspect VRAM or ROM bytes directly, only work RAM, CPU registers,
and PC breakpoints/run-to.

Multi-byte values are **little-endian** (verified: a 16-bit `write_memory`
of `5000` read back as bytes `88 13`, i.e. LE `0x1388`).

This doc was reorganized topically in a 2026-08-27 consolidation pass (no
new RE — see git history for the original chronological write-up). Every
address, value, and caveat below is preserved from that history; superseded
values are kept as explicit "superseded:" notes rather than deleted.

## Current truth — quick reference

Final state of every verified address, after W1 + W2 and the three later
correction passes (calibration, pad-mode pinning, pause-flag gate). Full
evidence and evolution live in the topical sections below.

| name | address | size | meaning | verification | date |
|---|---|---|---|---|---|
| P1 character id | `0xFFB5F0` block, `+0xE8` | u8 | character id (roster table below) | P1 constant `1` across 3 fresh-round dumps; cross-checked against P2 reads | 2026-08-27 |
| P2 character id | `0xFFB6E0` block, `+0xE8` | u8 | character id | P2 read 3/9/7 matching on-screen bar names | 2026-08-27 |
| P1 health | `0xFFB622` (block `+0x32`) | u8, max `0x78`=120 | health, authoritative | write-tested: forced low → DANGER; forced 0 → real KO/WINS | 2026-08-27 |
| P1 health mirror | `0xFFB624` (block `+0x34`) | u8 | mirrors health in lockstep | observed only — NOT independently write-tested; write both together | 2026-08-27 |
| P2 health | `0xFFB712` (block `+0x32`) | u8, max 120 | health, authoritative | write-tested | 2026-08-27 |
| P2 health mirror | `0xFFB714` (block `+0x34`) | u8 | mirrors health | observed lockstep, not independently write-tested | 2026-08-27 |
| P1 world X | `0xFFB6C8` (block `+0xD8`) | u16 LE | authoritative world/screen X | **write-verified teleport** (write 800 → full side-swap on screen) | 2026-08-27 |
| P1 X fraction | `0xFFB5F0` block, `+0xDA` | u16 LE, presumed | subpixel/fraction word | not separately verified; write 0 alongside X | 2026-08-27 |
| P2 world X | `0xFFB7B8` (block `+0xD8`) | u16 LE | world X | struct symmetry + decreasing-walk diff | 2026-08-27 |
| P1 Y | `0xFFB6CC` (block `+0xDC`) | u16 LE | vertical position; **GROUND_Y = 110** (superseded: 121 — see Fighter structs) | write-verified levitate (forced 50 mid-air) | ground value corrected 2026-08-27 (orchestrator, real-play recalibration) |
| P2 Y | `0xFFB7BC` (block `+0xDC`) | u16 LE | vertical position | struct symmetry | 2026-08-27 |
| round timer, seconds | `0xFFAB97` | u8, BCD | **AUTHORITATIVE** round timer | write-verified twice (display-follow + forced timeout) — see Round timer for the disprove/re-verify history | 2026-08-27 |
| timer sub-second | `0xFFAB96` | u8 | frame countdown within the current second; also the running/paused oracle | tick-pattern observation | 2026-08-27 |
| menu_state | `0xFFB2CE` | u16 | gate discriminator: `0x9C01` on menu-family screens, `0x0000` in-fight | 32 non-fight + 17 in-fight snapshots, corroborated by `0xFFB302`/`0xFFB306`; read-only, not write-tested | 2026-08-27 |
| round_over (LIKELY) | `0xFFB5E0` | u8 | match-decision flag: 0 live, 1 at KO through the WINS screen | 26 live samples across 3 matchups + 2 independent KO matches; NOT write-tested | 2026-08-27 |
| pause_flag | `0xFFD7D3` (twin `0xFFDA53`) | u8 | 1 = in-game paused, 0 running/post-fight/attract | 3 pause cycles + phase sweep, stable-diff of paused vs. unpaused arena state | 2026-08-27 (orchestrator, consolidation pass) |
| Port 1 pad-mode | `0xFFF9D1` | u8 | 1 = 6 BUTTON, 0 = ACTIVATOR/normal cycle | write-tested, 3 independent proofs (see Buttons & pad mode) | 2026-08-27 (orchestrator) |
| Port 2 pad-mode | `0xFFF9D0` | u8 | same | write-tested | 2026-08-27 (orchestrator) |

The controllable gate (final, 4 conditions — full evolution and rationale
in Gate, below):

```
word_zero(menu_state)        # 0xFFB2CE
byte_zero(round_over)        # 0xFFB5E0
health_in_range(1, 120)      # health max is 120 (0x78)
byte_zero(pause_flag)        # 0xFFD7D3 (twin 0xFFDA53)
```

Button table in 6-button mode (RETRO `b/y/r/l/a/x` → LP/HP/LK/HK/Block/Block2)
is in Buttons & pad mode, below.

## Method

1. **External candidates first.** Web search + WebFetch for Genesis Game
   Genie / Pro Action Replay codes and community RAM maps (method below).
2. **Save-state anchored diffing**, exactly like the arcade session:
   `save_state`/`load_state` work on this core; a state banked at a fresh
   "ROUND 1, both fighters full health" frame was reloaded repeatedly so
   every probe reran from identical conditions.
3. **Snapshot-diff on full 64 KiB dumps** (`read_memory` in 4096-byte
   chunks) taken via `pause` → dump → `resume`(short burst) → `pause` →
   dump, filtered by *shape* (monotonic-decreasing-then-flat for health,
   "0 for N samples then 1 forever" for a decision flag, etc.) rather than
   raw diff (raw diffs of this core's low RAM, `0xFF0000-0xFF0900`-ish, are
   dominated by ~2500 bytes/frame of sound-driver/DMA-queue churn totally
   unrelated to game state — every diff below excludes or filters past
   that noise).
4. **Write-tests for authority**: candidate found → `enable_writes` →
   `write_memory` → screenshot / re-read.
5. **A 2-human-controller rig for clean hit isolation.** Pressing `start`
   on controller port 1 mid-select converts P2 from CPU to a second human
   slot (both sides then pick a fighter and a normal "ROUND 1" starts).
   With P2 issuing no input, P1's attacks land on a stationary target,
   which is a far cleaner way to isolate "does this button deal damage and
   how much" than fighting the CPU's AI in real time. Caveat: P2 was
   observed to still throw an unprompted jump-kick in one trial — this
   join path does **not** reliably disable P2's CPU AI, so damage-based
   button tests via this rig were only trusted when P1's health stayed
   flat across the test window. (Buttons & pad mode, below, uses a
   different, more reliable join path for its own damage measurements.)

Workflow gotchas hit while applying this method (pacing, `enable_writes`
scope, menu timing, hex-string misreads, etc.) are consolidated in
**Session craft**, below, rather than scattered across passes.

## Fighter structs, positions & roster

Two per-player state blocks, **P1 = `0xFFB5F0`**, **P2 = `0xFFB6E0`**,
stride **`0xF0`** (240 bytes).

| offset in block | field | confirmed how |
|---|---|---|
| `+0xE8` | **character id** (u8; roster table below) | P1 read 1 (Liu Kang) constant across 3 independent fresh-round dumps (different P2 opponents each time). P2 read 3 with Baraka on screen, 9 with Reptile on screen (2-human rig), 7 with Rayden on screen (post-KO snapshot) — a clean 3-way comparison against small integer values, cross-checked so it isn't a coincidental match on any single sample. |
| `+0x32` | **health** (u8, max **0x78 = 120**) | P1 `0xFFB622`, P2 `0xFFB712`. Write-tested on both sides: forcing a low value produces the on-screen "DANGER" warning early, and forcing `0` fires the real KO / `<NAME> WINS` screen (verified for P1 vs Baraka and P2 vs Reptile, two independent matches). Values persist across frames once written (authoritative, not a recomputed display value). |
| `+0x34` | **health mirror** (u8) | `0xFFB624` (P1) / `0xFFB714` (P2). Tracks the primary health byte in lockstep in every sample taken (fresh-round full-120, mid-fight partial, post-KO 0). Not independently write-tested; write BOTH bytes together for enforcement, following the arcade port's own documented caution about independent dual health accumulators — this pairing is assumed, not proven, to behave the same way here. |
| `+0xD8` | **world X** (u16 LE) | P1 `0xFFB6C8`, P2 `0xFFB7B8`. Found by letting the CPU provide controlled motion: with no input at all, the CPU opponent walks toward P1, so its X must decrease monotonically — four paused full-WRAM snapshots at 0.3 s intervals were intersected for strictly-decreasing u16s with walk-plausible deltas (~34/step), which produced exactly this offset INSIDE the fighter struct (plus display-list churn elsewhere, easily excluded). Cross-checked against a walk-right/walk-left P1 session (values rise walking right, drop on knockback, and the P1/P2 separation matches the on-screen gap). **VERIFIED BY TELEPORT**: writing 800 (P1 was at ~627, P2 at ~768) visibly relocated Liu Kang to the RIGHT of the opponent — full side-swap on screen, camera followed, the value stuck (798→803 as physics continued from the new spot). This is the authoritative store the W1 candidate `0xFFB18B` was not (see Disproven & traps). The word at `+0xDA` moves in 0x4000-granularity steps and is almost certainly the subpixel/fraction word — write 0 alongside X for clean placement (not separately verified). |
| `+0xDC` | **Y** (u16 LE) | P1 `0xFFB6CC`, P2 `0xFFB7BC`. Jump-arc method: sampling the P1 struct every ~0.06 s through an `up` tap gives a clean parabola `121 → 113 → 87 → 67 → 55 → 47 → 43 → 42 → 46 → 52` — ground read as **121** in that sample, smaller = higher. **VERIFIED BY WRITE**: forcing 50 while standing visibly levitates the fighter mid-air (screenshot); the value holds (standing state applies no gravity). `+0xDE` is presumed the Y fraction, untested. **Superseded — GROUND_Y is 110, not 121:** the W2 jump-parabola baseline above (121) was a single stepped `up`-tap sample from one stance/stage snapshot. A later real-play calibration session found both fighters standing at y=110 across a real fight (2904/~3100 P1 frames; P2 identical). With GROUND_Y=121, the airborne test (`GROUND_Y − y > 4`) flagged 99.7% of decisions "air" — clearly wrong — so GROUND_Y was corrected to **110** (2026-08-27, orchestrator). See Calibration for the downstream training-data effect. |

Facing was not hunted this session (open). The disproven W1 X candidate
`0xFFB18B` stays disproven — the real X is the struct field above (full
story in Disproven & traps).

### Roster — character IDs (block `+0xE8`)

| id | name | verified how (this session) |
|---|---|---|
| 1 | Liu Kang | **YES** — P1 char_id constant `1` across 3 independent fresh-round dumps |
| 3 | Baraka | **YES** — P2 char_id `3` with "BARAKA" on the health bar |
| 7 | Rayden | **YES** — P2 char_id `7` with "RAYDEN" on the health bar (post-KO snapshot) |
| 9 | Reptile | **YES** — P2 char_id `9` with "REPTILE" on the health bar (2-human rig) |

**These are the SAME numeric ids already in `library/mk2/family.json`'s
roster** (`baraka=3, raiden/rayden=7, reptile=9, liukang=1`). No divergence
was observed for the four ids checked, so **no cross-port id_map is needed
for this subset** — but the remaining roster entries (Kung Lao, Johnny
Cage, Kitana, Sub-Zero, Scorpion, Jax, Mileena, Shang Tsung, the bosses)
were **not** independently re-verified on Genesis this session; treat the
match as strong evidence the two ports share one id table, not as proof
for every id.

## The round timer

`0xFFAB97` — **round timer** (u8, BCD seconds, **AUTHORITATIVE —
write-verified**). `0xFFAB96` is its sub-second frame countdown; `0xFFAB98`
is always `0x00` in every sample (which makes the training loop's 2-byte
`timer_hold` write `[0x99, 0x00]` safe).

### FOUND — W1's disproof was a misread

W1 found the BCD pair **`0xFFAB97`** / `0xFFAB9C` tracking the drawn
countdown, wrote `0x50` to both, read back `0x49`/`0x99` ~15 frames later
and called the store disproven. **Re-examined with a coherent write test,
that `0x49` was the written value legitimately ticking down** (0x50 → 0x49
BCD after one second): writing `0x50` to `0xFFAB97` makes the **on-screen
timer display 50 and keep counting** (screenshot at 48 two ticks later),
and writing `0x02` runs the clock out to the genuine timeout ending — the
2-human draw produced the real **GAME OVER** screen. `0xFFAB97` is the one
authoritative seconds store:

- `0xFFAB96` — frame countdown within the current second (drives the tick).
- `0xFFAB97` — **seconds, BCD, authoritative** (write-verified twice:
  display-follow and forced-timeout).
- `0xFFAB9C`, `0xFFABA0` — display-side copies (tick in sympathy, offset
  `+0x0C` on the latter); not stores, don't write them.

`timer_hold` in the profile is now **functional**: `round_timer =
0xFFAB97`, hold bytes `[0x99, 0x00]` (the second byte lands on the
always-zero `0xFFAB98`).

The full disprove-then-retract narrative (why this is worth remembering as
a method lesson, not just a corrected fact) is preserved in Disproven &
traps.

## The controllable gate

The gate accreted from 3 conditions to 4 across the RE session; each
condition was added to close a specific measured leak.

**Original 3-condition gate:**

```
word_zero(menu_state)        # 0xFFB2CE: kills title/menu/char-select/ladder/continue
byte_zero(round_over)        # 0xFFB5E0, LIKELY: kills the KO/WINS screen
health_in_range(1, 120)      # kills 0-health (dead/menu-garbage) frames
```

- `0xFFB2CE` **menu_state** (u16, the gate discriminator): reads `0x9C01`
  on intro, title, attract story screens, the dragon Start/Options menu,
  char select, the ladder screen, and the continue screen — in **every one
  of 32 non-fight WRAM snapshots** — and `0x0000` in **every one of 17
  in-fight snapshots** spanning three stages (portal arena, Dead Pool,
  Living Forest) and all three fight modes (1P-vs-CPU, 2-human duel,
  attract demo), plus a 40-sample rapid poll during live combat (all
  zero). The neighbouring words `0xFFB302`/`0xFFB306` (`0xFFFF` in menus,
  `0` in fights) look like parts of the same menu-context structure and
  corroborate. NOT write-tested (read-only discriminator).
- `0xFFB5E0` **match-decision flag / round_over** (u8, LIKELY): `0` across
  26 live-combat samples spanning 3 different matchups (Liu Kang vs
  Baraka / Rayden / Reptile) and a wide range of health values including
  near-death (as low as 25/120); becomes `1` at the moment a round is
  decided by KO and stays `1` through the "`<NAME>` WINS" screen,
  confirmed in 2 independent matches. Still **not write-tested**.
- `health_in_range(1, 120)` kills 0-health/menu-garbage frames.

**Leak measurement that motivated `menu_state`:** with only the bottom two
conditions, **16 of 38 non-fight snapshots leaked** (char select and the
ladder read healths 120/120 with `round_over=0`; the attract story screens
read 1/120; the dragon menu 16/117). `word_zero(menu_state)` kills every
one of those 16 while staying open in all 17 in-fight samples (including
the attract demo fight and the pre-round "ROUND N" banner — the gate opens
a few seconds before input is accepted, same benign banner window as the
arcade port). The gate stays open during the game's own start-button pause
in a 2-human game (menu_state stays 0 there — this turned out to be a
separate, unfixed leak; see pause_flag below). One known residual leak
remains even with all 4 conditions: the ~9 s draw-timeout **GAME OVER**
screen reads `0` for menu_state (with `round_over=0` and full healths)
before attract sets `0x9C01` — see Open gaps.

A sound-driver byte (`0xFF098F`) was tried and rejected as a discriminator
— see Disproven & traps.

**Added condition — `pause_flag` (2026-08-27, orchestrator, consolidation
pass):** headless playback verification found the committed arena state
(`shadow/arenas/mk2/genesis-probe.state`) is saved **in-game PAUSED** — a
frozen fight (timer stuck at BCD `0x98`, fighters inert) that the
3-condition gate read as OPEN, because in-game pause is invisible to
menu_state/round_over/health. **VERIFIED (3 pause cycles + phase sweep):**
`0xFFD7D3` (twin copy `0xFFDA53`) is 1 during in-game pause and 0 in
running fights, post-fight, and attract. Found by stable-diff between the
paused arena state and the unpaused fight (the arena being saved mid-pause
was, for once, useful). CAVEAT, recorded honestly: the address sits in
what looks like render scratch (the PAUSED overlay), so another overlay
could conceivably write it mid-fight — the failure mode is benign (gate
closes, recorder skips frames). **Same-day follow-up (A5 toolkit smoke):
that flicker WAS then observed — the flag briefly reads 1 during
genuinely live play, most likely HITSTOP (freeze-frames on hits), not
menu pause.** Consequence: the gate drops hitstop frames from recordings.
Judged acceptable — game state is frozen during hitstop so those rows are
near-duplicates, and held inputs bridge via surrounding frames — but if
fits ever look input-starved around hits, revisit this condition first.
Two candidate pause bytes were tried and rejected first —
see Disproven & traps.

**Final 4-condition gate:**

```
word_zero(menu_state)        # 0xFFB2CE
byte_zero(round_over)        # 0xFFB5E0
health_in_range(1, 120)      # kills 0-health (dead/menu-garbage) frames
byte_zero(pause_flag)        # 0xFFD7D3 (twin 0xFFDA53)
```

With this gate, the committed `genesis-probe.state` now correctly reads
`controllable=false` until unpaused (P1 Start).

## Buttons & pad mode — SOLVED (requires the game's 6-button setting)

Two discoveries unlocked this:

1. **The FBNeo core wires md_mk2 for a 6-button pad**
   (`FBNeo/src/burner/libretro/retro_input.cpp`, the md_mk2 special case):
   MD `A/B/C/X/Y/Z` → RETRO `b/a/r/y/x/l`. The same file carries the
   comment *"mk2 requires enabling 6-buttons in options, while others
   auto-detect it"* — meaning the **game's own menu**: dragon menu →
   Options → **Extra Controls** → set `Port 1` and `Port 2` from
   `ACTIVATOR` to **`6 BUTTON`** (the `a` button cycles the highlighted
   entry). The setting is menu-only (no SRAM) but **save states carry
   it** — `shadow/arenas/mk2/genesis-probe.state` is captured in 6-button
   mode, so state-anchored flows inherit it. On a cold boot without the
   setting, only `b/a/r` act: `b`=LP jab 6, `a`=mid kick 20, `r`=roundhouse
   24, and **no button blocks at all** (measured: every single RETRO button
   held by the defender still ate the full 24-damage roundhouse).
2. **A clean dummy rig**: `press_buttons(port=1, buttons=['start'])`
   during a live 1P round fires "PLAYER TWO HAS ENTERED THE TOURNAMENT" →
   both players re-pick → a normal 2-human round where P2 stands
   genuinely still (no CPU AI at all — the Method section's caveat about
   unprompted P2 attacks under the OTHER 2-human join path does not apply
   to this join path). All damage numbers below were measured against
   that idle human dummy with spacing controlled by writing P1's X
   (`p2x - 50` / `- 40` / `- 58`), which also avoids the throw-range
   confound.

### 6-button mode classification (damage out of 120)

| RETRO | MD | class | evidence |
|---|---|---|---|
| `b` | A | **LP** | jab, 6 dmg at kick range (also 6 in 3-button mode — same button both modes) |
| `y` | X | **HP** | 18 dmg close, 9 at longer range; **double-bound quirk**: HELDing `y` engages Block (block stance + chip numbers), tapping it fires the punch — the core binds two MD buttons onto RETRO `y`. Chords tap, so HP=`y` is safe; never hold `y` expecting only HP. |
| `r` | C | **LK** | 20 dmg standing kick at range 50, 15 close (knee) |
| `l` | Z | **HK** | 24 dmg roundhouse at range 50/58, 15 close (knee) |
| `a` | B | **Block** | zero damage output at every spacing; holding it cuts an incoming 24-damage roundhouse to **6 chip** — verified on BOTH sides (P2 holds vs P1 roundhouse, and P1 holds vs P2 roundhouse), block stance visible in screenshots |
| `x` | Y | **Block (second)** | identical chip test result and stance; a genuine second block button (matches the cartridge's stock 6-button layout, block on both middle columns) |

Reference numbers: unblocked roundhouse 24, blocked 6 chip; unblocked
attacks with the defender holding a non-block button land in full (and the
defender's own held attack often trades — held LP autorepeats jabs).

`attack_chords`: `LP: ["b"], HP: ["y"], LK: ["r"], HK: ["l"],
Block: ["a"]`. An earlier W1 entry (`HK: ["a","r"]`) described **3-button
mode** (a different game configuration) and is superseded.

### The pad-mode flags — pinned (2026-08-27, orchestrator)

**VERIFIED (write-tested, driver-level):** the Extra Controls per-port pad
type lives at `0xFFF9D1` (Port 1) / `0xFFF9D0` (Port 2): 1 = 6 BUTTON,
0 = the ACTIVATOR/normal cycle. Found via same-screen WRAM diff across the
menu's cycle button (the value cycles with Genesis A — RETRO `b` — NOT
left/right; left/right only reveal the cursor). Three independent proofs:

1. writing the byte re-renders the menu label live (the game polls it);
2. with flag=1 a held RETRO `l` decodes into the pad-state bytes
   `0xFFF9D5/D6/D8` (bit 0x20), with flag=0 the driver ignores the button
   entirely;
3. cold boot with the profile `pins` asserting both flags at 1 Hz gives
   full 6-button decode with no menu visit.

Nearby derived bytes (`0xFFF9D3/DE/E1/EC/ED`) are driver echoes — do not
pin them. Two other "49/50" label-tracking bytes were tried and rejected
as the source — see Disproven & traps.

The `pins` profile key holds both flags for every session, so cold boots
can no longer silently downgrade recordings to 3-button (which has no
Block and would poison attack labels).

## Calibration

**GROUND_Y = 110, not 121** (2026-08-27, orchestrator, first real fit).
Across a real play session both fighters stand at y=110 (2904/~3100 P1
frames; P2 identical); the W2 jump-parabola baseline of 121 was measured
from a different stance/stage snapshot (full story in Fighter structs,
above). With 121 the airborne test (`GROUND_Y − y > 4`) marked 99.7% of
decisions "air".

**World X vs. screen X — the corner feature is unusable until stage
bounds are RE'd**: `x` in the fighter struct is WORLD position, not
screen position; world x runs ~500–800 vs `SCREEN_W=320`, putting everyone
permanently "past the right edge" (87% corner bucket) under a naive
screen-space corner check. `CORNER_PX`/`SCREEN_W` were removed from
calibration so `me_corner` drops out via the availability table. OPEN:
find per-stage world bounds (or a camera-x global to derive screen x).

## Enforcement — what actually works

| lever | status |
|---|---|
| health refill | **Likely works** via `write_memory` to both `health` and `health_mirror` on the target side (`0xFFB622`+`0xFFB624` for P1, `0xFFB712`+`0xFFB714` for P2) — the primary byte is write-tested and authoritative; the mirror is written defensively by analogy with the arcade port's dual-accumulator finding, not because independent divergence was observed here. |
| health_max | **120** (`0x78`) — round-start fill value, and the write-verified full-bar value. |
| timer hold | **Functional** — `0xFFAB97` write-verified authoritative (above); hold bytes `[0x99, 0x00]`. |
| position write | **Works** — X (`+0xD8`) and Y (`+0xDC`) both accept writes and visibly relocate the fighter (teleport/levitate verified); write the fraction word (`+0xDA`/`+0xDE`) to 0 alongside. |
| credits | **N/A** — home-console cartridge, no coin/credit system; `start` joins/continues directly. |

## Special-move encodings (`special_inputs`, 2026-08-28)

Pasted verbatim from `shadow/MACRO_ACTIONS.md` §2 (user-verified reference
data, both MK2 ports) — profiled in `genesis.profile.json`:

```jsonc
"special_inputs": {
  "reptile": {
    "slide": [ { "dirs": ["back"], "press": ["LK", "HK"], "frames": 4 } ]
  }
}
```

Genesis Reptile's slide is `back+LK+HK` — **different** from arcade's
`back+LK+LP` (MACRO_ACTIONS.md's motivating cross-port-divergence case).

## Contact/hit signal hunt (2026-08-28, A-RE, live grounding pass)

Session goal: find genesis MK2's analog of the arcade port's verified global
`hit_counter` (`0xD3FE`, `mk2.md`) — a value that changes on every landed hit
AND on blocked/chip contact, quiet in neutral — for the block-punish dummy's
`contact_signal` (MACRO_ACTIONS.md §6). Headless FBNeo, port 4032, arena
`genesis-probe.state` (Liu Kang vs Baraka CPU), `shadow_train.re.Probe`.

**Method, refined from the arcade session's**: single-emulated-frame
precision throughout (`pause` once, then `step()` + a **mandatory ~20 ms
real-time sleep per step** — `step()` alone is a no-op if polled faster than
the main loop consumes the flag, the first and most expensive gotcha this
session hit; see Toolkit friction). Health-drop-bracketing full-window
snapshot diffs (`re.diff` across exactly one `step()`) isolate a hit's true
byte-level footprint far more tightly than a real-time `resume()`-and-poll
loop, which lets 10+ frames of unrelated churn leak into the diff (measured:
a real-time bracket produced 8000-11000 changed bytes per event out of
205 KB; single-frame bracketing cut this to 90-1300).

**The neutral bar was raised past the arcade session's own**: an early pass
(candidates `0xFFC726`/`0xFFC734`, see Disproven below) satisfied "fires on
every hit, fires on every block-chip, quiet while standing" and looked like
a clean find — but a dedicated single-frame-precision walking/jumping check
(not run by the arcade session, which only checked a pre-contact **standing**
window) showed it firing 15-30% of frames during ordinary movement with zero
contact. A byte that pulses on footsies would make the block-punish dummy
fire on nothing; "quiet in neutral" was redefined for this session to mean
standing **and** walking **and** jumping **and** block-held-with-no-incoming-
attack, not just standing.

### NOT FOUND — searched exhaustively, honestly open

No byte in the accessible WRAM window (`0xFF0000`-`0xFFFFFF`, all 65536
bytes swept) or the region's tail (`0x10000`-`0x321E4` offset, Z80/sound +
other spans per Overview) survives the full bar: fires on every one of 5-6
raw hits (P1 vs CPU Baraka, single-frame-bracketed), fires on every one of
3-4 blocked/chip hits (P1 holds `a`=Block while CPU attacks), AND is absent
from a union of single-frame-precision neutral sweeps (standing ~20-30
frames, walking left/right, jumping, block-held far from the opponent — all
zero contact). Two independent full-sweep passes (the 64 KiB work-RAM window
and the >64 KiB tail) both returned **zero** surviving candidates once the
neutral sweep was done at single-frame precision instead of coarse
before/after snapshots (see Toolkit friction — coarse sampling produces
false negatives in the noise set, which is what let the disproven candidate
through the first pass).

The tail region (`0x10000`+ offset) reconfirms the existing doc's own
caution: a **pure-standing** single-frame sweep there touched **8493** of
~139000 bytes in under a second — Z80 sound-driver/music churn, exactly as
already documented for the Overview's blob layout, not gameplay state.

### LIKELY (fires on contact, but NOT contact-exclusive — disqualified as `contact_signal`)

| address | field | evidence |
|---|---|---|
| `0xFFC726` / `0xFFC734` (part of a ~13-byte cluster `0xFFC71D`-`0xFFC734`) | shared VFX/collision-effect object, NOT a hitstun counter | Fires on **6/6** raw hits and **4/4** blocked/chip hits (single-frame-bracketed, both P1-gets-hit-by-CPU and P1-attacks-P2 directions — see Per-player vs global below) and is silent across a 30-frame pure-standing sweep, but ALSO fires on 21-27 of 120 single-frame-sampled walking frames and 12-16 of ~180 jumping frames (probe: `/tmp/mk2_genesis_probe8.py`-style value log, not committed). Values jump erratically between contact events (`10→26`, `0→255`, `116→2`) rather than incrementing — consistent with a rotating slot in a small shared particle/hit-spark/footstep-dust object pool, not a dedicated counter. **Do not adopt as `contact_signal`** — it would false-fire during ordinary footsies. |

**Per-player vs global**: the same `0xFFC726`/`0xFFC734` pair changed both
when P1 was hit by the CPU AND when P1 landed a hit on P2 (LP button, 4
events, 1 landed as `p2_hit_by_p1`) — no distinct P1-victim vs P2-victim
address pair emerged; whatever this cluster is, it looks **global/shared**
between the two fighters, not a per-player pair. (Caveat: only one clean
P1-attacks-P2 landed hit was captured this session — same "couldn't
reliably land clean P1→P2 hits" friction the arcade session reported.)

**`pause_flag` (`0xFFD7D3`/twin `0xFFDA53`) re-examined as a hitstop-based
contact signal**: mk2-genesis.md's own existing caveat ("flag briefly reads
1... most likely HITSTOP") was retested directly — a single-frame timeline
across 6 hits shows the flag flickering on-and-off constantly (not
correlated with the `<<HIT>>` markers specifically) AND flickers just as
often during a pure walking control with zero contact. **Reconfirmed
unreliable as a contact signal** — likely a display double-buffer or
similar per-frame render toggle unrelated to hitstop specifically; the
existing gate-leak caveat about it stands, but it is not usable for
`contact_signal`.

### Toolkit friction (this session)

- **`step()` needs real wall-clock time after the call, same as
  `press_buttons`** (already documented for `press`/`press_buttons` in
  mk2-genesis.md's Session craft) — but this was NOT previously documented
  for `step()` specifically, and it manifested differently: calling `step()`
  in a tight loop with no sleep produced almost no game progress at all
  (300 calls advanced world X by only ~9 units, no CPU approach) rather than
  an obviously-wrong read. A ~20 ms sleep after every `step()` call fixed it
  completely (CPU aggression resumed at its normal live-play rate). Add to
  the SKILL: `step()` shares `press_buttons`'s "consumed once per real
  emulated frame, not per call" behavior.
- **Coarse before/after neutral sampling under-reports noise.** A neutral
  "walk 6 bursts of press+sleep, snapshot only before and after" pass missed
  candidates that a single-frame-precision sweep of the SAME activity caught
  reliably 15-30% of frames — the byte returns to a value close to its
  starting point often enough that a two-sample bracket has a real chance of
  missing the whole excursion. Any "is this quiet in neutral" check needs
  the same per-frame precision as the hit-bracketing check, not a coarser
  one — asymmetric rigor here is what let `0xFFC726`/`0xFFC734` look
  contact-exclusive on the first pass.
- Small-window (`~0x1000`-`0x5000` byte) `read_region` snapshots at
  single-frame cadence are cheap enough (2-3 chunked reads) to run
  per-`step()` through an entire hit search without materially slowing the
  session; a full 64 KiB window (8 chunks) is still fast enough for this
  (a 4-6-hit search completed in 1-3 seconds of wall time). The >64 KiB tail
  (18 chunks for the full ~139 KB span) is the practical upper bound tried.

## Open gaps

1. **Facing** — not hunted; candidates likely near the X/Y fields.
2. **Wins-per-player** — not investigated.
3. **round_over (`0xFFB5E0`) write-test** — still correlation-only.
4. Full roster id cross-check beyond the 4 ids exercised (1, 3, 7, 9).
5. The ~9 s **GAME OVER gate window** (draw-timeout only) if it ever
   matters in practice.
6. **Per-stage world-X bounds** (or a camera-x global) needed before the
   corner feature can be reintroduced (see Calibration).
7. **Contact/hit signal (`contact_signal`) — NOT FOUND** (2026-08-28): no
   byte satisfies "fires on every hit and block, quiet through standing +
   walking + jumping neutral" (see Contact/hit signal hunt, above). The
   shared-object-pool lead (`0xFFC726`/`0xFFC734`) is a plausible next
   thread — mapping the actual object-pool structure (slot stride, spawn
   trigger) rather than treating it as a flat byte could recover a genuine
   per-hit spawn-index signal, but that is real RE work, not a quick
   follow-up.

## Disproven & traps

Preserved verbatim in substance — these are as useful as the confirmed
addresses, either because the trap could recur or because the disprove
(or, in one case, disprove-then-*retract*) teaches the method.

- **`0xFFC726`/`0xFFC734` as `contact_signal`** (2026-08-28, contact/hit
  signal hunt): passed a coarse "fires on hit, fires on block, quiet
  standing" check, then FAILED a follow-up single-frame-precision walking
  and jumping check (fired on 15-30% of frames with zero contact). The trap:
  the first-pass neutral control used coarse before/after snapshots around
  several presses, which under-sample fast-toggling bytes (see Toolkit
  friction). Root-caused as a likely shared VFX/collision-effect object pool
  slot (erratic, non-incrementing values on every touch), not a hitstun
  counter. Full evidence in Contact/hit signal hunt, above.
- **`0xFFD7D3`/`0xFFDA53` (`pause_flag`) as a hitstop-based `contact_signal`**
  (2026-08-28): re-tested directly against a single-frame hit timeline —
  flickers constantly, uncorrelated with hit instants, and flickers just as
  often during pure walking with zero contact. The gate's existing pause-flag
  caveat about a possible hitstop flicker is reconfirmed as real but useless
  for contact detection (too noisy). See Contact/hit signal hunt, above.
- **The region tail (`0x10000`-`0x321E4` offset, beyond the 64 KiB m68k work
  RAM window)** (2026-08-28): re-confirmed as sound-driver/music churn, NOT
  gameplay state — a pure-standing single-frame sweep touched 8493 of
  ~139000 bytes there in under a second. Do not hunt gameplay signals in
  this span; matches the Overview's existing blob-layout description.
- **Genesis Game Genie codes for this game** (`gamegenie.com`, e.g.
  `ALAA-AA9C` "P1 Infinite Health", `ABVT-BE64` "Infinite time"): decoded
  with a from-scratch implementation of the documented Genesis GG algorithm
  (5-bit alphabet `ABCDEFGHJKLMNPRSTVWXYZ0123456789`, bit-rearrangement
  table from `segakore.fr`'s `md_gg_conv_method.txt`; verified byte-exact
  against that document's own worked example, `SCRA-BJX0` → address
  `0x009C76`, value `0x5478`). Every MK2 code decoded to a **ROM code
  patch** (e.g. `ALAA-AA9C` → address `0x0080E2`, value `0x6002` =
  `BRA.S +2`, i.e. "skip the next instruction" — a classic
  infinite-health patch that skips a subtraction op), not a RAM data
  address. This core exposes no ROM region (`has_rom: false`), so the
  skipped instruction's operand (which would have been the real RAM
  address) can't be read. Two M68K breakpoints were planted at `0x0080E2`
  and `0x0080EC` (P1/P2 patch addresses) and left armed through a full
  fresh round of live combat — **neither ever fired**, most likely because
  these Game Genie codes target the USA release and this ROM is the World
  revision (CRC `a9e013d8`) with a different code layout. Abandoned this
  approach entirely in favor of live snapshot-diffing.
- **RetroAchievements forum topic 4652** ("Mortal Kombat II"): found via
  web search and initially looked like exactly the kind of community RAM
  map this session needed (`Timer: 0x00009f`, `1P HP: 0x0000a5`, etc.) —
  **this is the WRONG GAME**. Reading the actual forum content (not just
  the search snippet) shows it documents the unlicensed 8-bit NES/Famicom
  "Hummer Team" bootleg of Mortal Kombat II, a completely different game on
  completely different hardware, not the real Genesis cartridge covered by
  this doc. None of its addresses were adopted. Flagged here prominently
  because the search-engine summary alone was actively misleading — always
  open and read the source before trusting an address dump.
- **RetroAchievements code notes for the correct game**
  (`retroachievements.org/game/60`, `codenotes.php?g=60`, 27 notes
  advertised): the site's Cloudflare challenge was passable via a real
  browser session, but the actual note list renders from an
  authenticated API call and returned empty/blank for a logged-out
  session. Not pursued further (logging in was out of scope for this
  task).
- **`0xFFB18B`** (position candidate): `+90` after walking right, but a
  forced write of `5000` produced no visible teleport. Disproven — the
  real X is the struct field `+0xD8` (teleport-verified).
- **`0xFF098F`** (gate discriminator candidate): read 0 in 13 fight
  samples and nonzero in all 32 non-fight samples of the first capture
  batch, survived 45 fight snapshots total and a 50-sample rapid poll
  during arena-stage combat in two stages — and then read a fluctuating
  **68** during a live Living Forest duel/fight. It sits in the
  `0xFF09xx` sound-driver span; its "screen id"-looking values (96=title,
  136=menu, 32=char select) are music-state, not game-phase. Do not
  readopt.
- ~~`0xFFAB97` / `0xFFAB9C` timer disproof~~ — **retracted**. This is a
  disprove-then-*re-verify* worth keeping in full: W1 wrote `0x50` to both
  bytes, read back `0x49`/`0x99` ~15 frames later, and called the store
  disproven. Re-examined with a coherent write test, that `0x49` was
  simply the written value legitimately ticking down one BCD step (0x50 →
  0x49 after one second) — writing `0x50` to `0xFFAB97` made the on-screen
  timer display 50 and keep counting, and writing `0x02` ran the clock out
  to a genuine timeout ending (a real GAME OVER screen in a 2-human draw).
  `0xFFAB97` **is** the authoritative timer; the original disproof was a
  misread of a correctly-ticking write, not a bad address. See The round
  timer, above, for the current-truth version.
- **`0xFF07A8`/`0xFF07AC`** (pause-flag candidates): track pause only
  coincidentally in-fight — they read 62/60 and 43/44 across menus
  (sound/animation counters, not flags). Disproven; the real pause flag is
  `0xFFD7D3` (twin `0xFFDA53`), see The controllable gate.
- **`0xFFB2AE`/`0xFFB2F4`** ("49/50" bytes): track the pad-mode menu
  label's rendering and are DERIVED — a write-test showed no re-render.
  Disproven as the pad-mode source; the real flags are `0xFFF9D1`/`0xFFF9D0`,
  see Buttons & pad mode.

## Session craft

Workflow lessons from applying the Method above across two RE passes (W1,
W2) plus a headless-playback-verification pass. Kept together here so
future sessions don't relearn them.

**Probe gotchas (Genesis-MK2-specific, W1/W2):**

- Headless pacing here ran close to real-time (~51 fps measured over a 5 s
  window), NOT dramatically uncapped — `press_buttons(frames=N)` does not
  block for those N frames, it queues input that drains over the *next* N
  frames of whatever real time elapses in the background. Every
  multi-step menu script needs an explicit `time.sleep()` after each
  `press_buttons` (0.15-2.5 s depending on the transition) or the
  follow-up screenshot/read just re-observes the pre-press frame.
  (Editorial note, 2026-08-27 consolidation pass: a `--pace` CLI flag now
  exists — default `1.0`, paced to real-time; `0` or negative = uncapped
  — so a session that wants deterministic, human-realistic timing no
  longer has to fight the default uncapped-adjacent behavior described
  here and below.)
- `enable_writes` is per-session (Method #4) — a fresh script/process
  needs it again even if an earlier process already armed writes.
- The pre-round "ROUND N" / "FIGHT!!" banner ignores input and freezes the
  timer at 99 for **several real seconds** (longer than the arcade port's
  ~2 s leak) before live combat starts; scripts that press an attack too
  early land nothing.
- The CPU is fast and aggressive from the first live frame: an idle P1 was
  KO'd in as little as ~4-9 real seconds across several attempts. Bank a
  save state at the *exact* "ROUND N, timer 99, full green bars" frame or
  every later probe re-fights CPU aggression from scratch.
- "PUSH START" printed over P2's name during a live 1P-vs-CPU round is a
  **permanent 2P-join invitation overlay**, not a round-over indicator —
  it is visible from the very first live frame of a fresh round. Do not
  confuse it with the arcade port's win-declared state.
- Joining P2 via `start` on port 1 (Method #5's rig) does not
  deterministically hand P2 to human control: after a long idle stretch,
  P2 (Reptile) was seen to throw an unprompted jump-kick that knocked P1
  down while nothing was pressed on port 1. Treat this 2P-rig's damage
  tests as trustworthy only when the *other* side's health stayed
  provably flat for the whole window. (Contrast with the button-testing
  join path in Buttons & pad mode, which does not have this problem.)
- `read_memory`/`write_memory` hex fields are hex STRINGS (e.g. `"78"` =
  120 decimal) — several early probes in this session mis-read them as
  decimal before catching the mistake; every value in this doc has been
  re-derived correctly.

**W2 probe gotchas (additions):**

- **Screenshots right after `load_state` are stale**: RAM is restored
  immediately but `app://screen` still shows the last rendered frame until
  the core runs a frame. Read memory for truth, or run ≥1 frame first.
- **Queued input survives `load_state`** — a 50-frame `press_buttons`
  that hasn't fully drained keeps draining into the reloaded state; the
  first W2 pose gallery was contaminated this way (every trial showed the
  *previous* trial's move). Resume ~0.3 s and re-pause to drain, or keep
  presses short.
- **Pausing between presses eats the queue** (the known inject-while-
  running rule, restated for menus): a `press → pause/screenshot → press`
  sequence loses the second press; navigate menus with sleeps only.
- **Menus time out into attract in ~10 s**; save-state the target menu
  screen (`Extra Controls` here) and drive from reloads.
- Arena/duel states banked at the "FIGHT!!" banner ignore input for the
  first ~2 s after load — bank states a beat *after* the banner clears
  (verify with a walk-write X read-back) or sleep 2.5 s post-load.

**Session gotchas — headless playback verification (2026-08-27,
orchestrator):**

- **The committed arena state is saved in-game PAUSED.** Loading
  `shadow/arenas/mk2/genesis-probe.state` gives a frozen fight (timer
  stuck at BCD `0x98`, fighters inert) that the (then-)3-condition gate
  read as OPEN — in-game pause was invisible to it. One P1 Start press
  unpauses. **RESOLVED** — see The controllable gate: `0xFFD7D3`
  `pause_flag` was found and added as the gate's 4th condition; the
  committed state now correctly reads `controllable=false` until
  unpaused.
- **Start is heavily overloaded**: P1 Start = pause toggle; P2 Start
  mid-fight = join, which detours through P2 char select (fighter x reads
  28/292 there — those are select-screen values, not a crash). At
  uncapped headless speed this flow is fragile; in windowed play at human
  speed it's the normal "controller 2 presses Start, picks a character"
  flow. (See the `--pace` note above — running a session at `--pace 1.0`
  or lower narrows the gap between headless and windowed-human timing.)
- **Timer sub-second byte `0xFFAB96` is the reliable running/paused
  oracle** (advances every frame when the game runs, static under pause)
  — use it, not the gate, to decide whether the world is live.
- Verified this session: the generalized runner emits decisions on this
  profile (correct block2 anchoring, sensible masks) and port-2 injected
  input lands in-game (the join itself proves it). Full 2-human shadow
  fight is a windowed-play exercise.

## What training-mode readiness still lacks

See Open gaps, above — kept as one canonical list rather than duplicated
here.
