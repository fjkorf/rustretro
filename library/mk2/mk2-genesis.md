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
   `write_memory` → screenshot / re-read. `enable_writes` is
   **per-MCP-session**; a fresh Python process opening a new McpClient must
   call it again or every `write_memory` silently returns
   `{"error": "writes are locked"}` while `read_memory` keeps returning the
   old value — this cost real time during the session (see Gotchas) and is
   flagged here so it isn't relearned.
5. **A 2-human-controller rig for clean hit isolation.** Pressing `start`
   on controller port 1 mid-select converts P2 from CPU to a second human
   slot (both sides then pick a fighter and a normal "ROUND 1" starts).
   With P2 issuing no input, P1's attacks land on a stationary target,
   which is a far cleaner way to isolate "does this button deal damage and
   how much" than fighting the CPU's AI in real time. Caveat: P2 was
   observed to still throw an unprompted jump-kick in one trial (see
   Gotchas) — the human-join does not reliably disable P2's CPU AI, so
   damage-based button tests were only trusted when P1's health stayed
   flat across the test window.

**Probe gotchas (Genesis-MK2-specific):**
- Headless pacing here ran close to real-time (~51 fps measured over a 5 s
  window), NOT dramatically uncapped the way the task brief warned it might
  be — `press_buttons(frames=N)` does not block for those N frames, it
  queues input that drains over the *next* N frames of whatever real time
  elapses in the background. Every multi-step menu script needs an
  explicit `time.sleep()` after each `press_buttons` (0.15-2.5 s depending
  on the transition) or the follow-up screenshot/read just re-observes the
  pre-press frame.
- `enable_writes` is per-session (see Method #4) — a fresh script/process
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
- Joining P2 via `start` on port 1 does not deterministically hand P2 to
  human control: after a long idle stretch, P2 (Reptile) was seen to throw
  an unprompted jump-kick that knocked P1 down while nothing was pressed on
  port 1. Treat 2P-rig damage tests as trustworthy only when the *other*
  side's health stayed provably flat for the whole window.
- `read_memory`/`write_memory` hex fields are hex STRINGS (e.g. `"78"` =
  120 decimal) — several early probes in this session mis-read them as
  decimal before catching the mistake; every value in this doc has been
  re-derived correctly.

## Fighter data — verified

Two per-player state blocks, **P1 = `0xFFB5F0`**, **P2 = `0xFFB6E0`**,
stride **`0xF0`** (240 bytes).

| offset in block | field | confirmed how |
|---|---|---|
| `+0xE8` | **character id** (u8; table below) | P1 read 1 (Liu Kang) constant across 3 independent fresh-round dumps (different P2 opponents each time). P2 read 3 with Baraka on screen, 9 with Reptile on screen (2-human rig), 7 with Rayden on screen (post-KO snapshot) — a clean 3-way comparison against small integer values, cross-checked so it isn't a coincidental match on any single sample. |
| `+0x32` | **health** (u8, max **0x78 = 120**) | P1 `0xFFB622`, P2 `0xFFB712`. Write-tested on both sides: forcing a low value produces the on-screen "DANGER" warning early, and forcing `0` fires the real KO / `<NAME> WINS` screen (verified for P1 vs Baraka and P2 vs Reptile, two independent matches). Values persist across frames once written (authoritative, not a recomputed display value). |
| `+0x34` | **health mirror** (u8) | `0xFFB624` (P1) / `0xFFB714` (P2). Tracks the primary health byte in lockstep in every sample taken (fresh-round full-120, mid-fight partial, post-KO 0). Not independently write-tested; write BOTH bytes together for enforcement, following the arcade port's own documented caution about independent dual health accumulators — this pairing is assumed, not proven, to behave the same way here. |
| `+0xD8` | **world X** (u16 LE) | P1 `0xFFB6C8`, P2 `0xFFB7B8`. Found by letting the CPU provide controlled motion: with no input at all, the CPU opponent walks toward P1, so its X must decrease monotonically — four paused full-WRAM snapshots at 0.3 s intervals were intersected for strictly-decreasing u16s with walk-plausible deltas (~34/step), which produced exactly this offset INSIDE the fighter struct (plus display-list churn elsewhere, easily excluded). Cross-checked against a walk-right/walk-left P1 session (values rise walking right, drop on knockback, and the P1/P2 separation matches the on-screen gap). **VERIFIED BY TELEPORT**: writing 800 (P1 was at ~627, P2 at ~768) visibly relocated Liu Kang to the RIGHT of the opponent — full side-swap on screen, camera followed, the value stuck (798→803 as physics continued from the new spot). This is the authoritative store the W1 candidate `0xFFB18B` was not. The word at `+0xDA` moves in 0x4000-granularity steps and is almost certainly the subpixel/fraction word — write 0 alongside X for clean placement (not separately verified). |
| `+0xDC` | **Y** (u16 LE) | P1 `0xFFB6CC`, P2 `0xFFB7BC`. Jump-arc method: sampling the P1 struct every ~0.06 s through an `up` tap gives a clean parabola `121 → 113 → 87 → 67 → 55 → 47 → 43 → 42 → 46 → 52` — ground is **121**, smaller = higher. **VERIFIED BY WRITE**: forcing 50 while standing visibly levitates the fighter mid-air (screenshot); the value holds (standing state applies no gravity). `+0xDE` is presumed the Y fraction, untested. |

Facing was not hunted this session (open). The disproven W1 X candidate
`0xFFB18B` stays disproven — the real X is the struct field above.

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

## Match-state globals

| address | name | semantics (observed) |
|---|---|---|
| `0xFFB5E0` | **match-decision flag / round_over** (u8, LIKELY) | `0` across 26 live-combat samples spanning 3 different matchups (Liu Kang vs Baraka / Rayden / Reptile) and a wide range of health values including near-death (as low as 25/120); becomes `1` at the moment a round is decided by KO and stays `1` through the "`<NAME>` WINS" screen, confirmed in 2 independent matches. Still **not write-tested**. The menu-phase discriminator gap flagged by W1 is now closed by `menu_state` (below). |
| `0xFFB2CE` | **menu_state** (u16, the gate discriminator) | Reads `0x9C01` on intro, title, attract story screens, the dragon Start/Options menu, char select, the ladder screen, and the continue screen — in **every one of 32 non-fight WRAM snapshots** — and `0x0000` in **every one of 17 in-fight snapshots** spanning three stages (portal arena, Dead Pool, Living Forest) and all three fight modes (1P-vs-CPU, 2-human duel, attract demo), plus a 40-sample rapid poll during live combat (all zero). The neighbouring words `0xFFB302`/`0xFFB306` (`0xFFFF` in menus, `0` in fights) look like parts of the same menu-context structure and corroborate. Known residual: the draw-timeout **GAME OVER** screen reads `0` for ~9 s (with `round_over=0` and full healths) before attract sets `0x9C01` — the only observed gate window, static screen, zero p1_input. NOT write-tested (read-only discriminator). |
| `0xFFAB97` | **round timer** (u8, BCD seconds, **AUTHORITATIVE — write-verified**) | See below. `0xFFAB96` is its sub-second frame countdown; `0xFFAB98` is always `0x00` in every sample (which makes the training loop's 2-byte `timer_hold` write `[0x99, 0x00]` safe). |

### The round timer — FOUND (W1's disproof was a misread)

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

## The controllable gate

```
word_zero(menu_state)        # 0xFFB2CE: kills title/menu/char-select/ladder/continue
byte_zero(round_over)        # 0xFFB5E0, LIKELY: kills the KO/WINS screen
health_in_range(1, 120)      # kills 0-health (dead/menu-garbage) frames
```

The W1 leak is measured and closed: with only the bottom two conditions,
**16 of 38 non-fight snapshots leaked** (char select and the ladder read
healths 120/120 with `round_over=0`; the attract story screens read 1/120;
the dragon menu 16/117). `word_zero(menu_state)` kills every one of those
16 while staying open in all 17 in-fight samples (including the attract
demo fight and the pre-round "ROUND N" banner — the gate opens a few
seconds before input is accepted, same benign banner window as the arcade
port). The gate stays open during the game's own start-button pause in a
2-human game (menu_state stays 0 there), and the one known residual leak
is the ~9 s draw-timeout GAME OVER screen (see the globals table).

**Disproven discriminator — do not readopt `0xFF098F`**: it read 0 in 13
fight samples and nonzero in all 32 non-fight samples of the first capture
batch, survived a 50-sample rapid poll during arena-stage combat — and
then read a fluctuating **68** during a live Living Forest duel. It sits
in the `0xFF09xx` sound-driver span; its "screen id"-looking values
(96=title, 136=menu, 32=char select) are music-state, not game-phase.

## Enforcement — what actually works

| lever | status |
|---|---|
| health refill | **Likely works** via `write_memory` to both `health` and `health_mirror` on the target side (`0xFFB622`+`0xFFB624` for P1, `0xFFB712`+`0xFFB714` for P2) — the primary byte is write-tested and authoritative; the mirror is written defensively by analogy with the arcade port's dual-accumulator finding, not because independent divergence was observed here. |
| health_max | **120** (`0x78`) — round-start fill value, and the write-verified full-bar value. |
| timer hold | **Functional** — `0xFFAB97` write-verified authoritative (above); hold bytes `[0x99, 0x00]`. |
| position write | **Works** — X (`+0xD8`) and Y (`+0xDC`) both accept writes and visibly relocate the fighter (teleport/levitate verified); write the fraction word (`+0xDA`/`+0xDE`) to 0 alongside. |
| credits | **N/A** — home-console cartridge, no coin/credit system; `start` joins/continues directly. |

## Buttons — SOLVED (requires the game's 6-button setting)

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
   genuinely still (no CPU AI at all — W1's caveat about unprompted P2
   attacks does not apply to this join path). All damage numbers below
   were measured against that idle human dummy with spacing controlled by
   writing P1's X (`p2x - 50` / `- 40` / `- 58`), which also avoids the
   throw-range confound.

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
Block: ["a"]`. W1's `HK: ["a","r"]` entry described **3-button mode** (a
different game configuration) and is superseded.

## Disproven / dead ends (don't re-chase these)

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
- **`0xFF098F`** (gate discriminator candidate): survived 45 fight
  snapshots and a 50-sample combat poll in two stages, then read nonzero
  (68, fluctuating) during a live Living Forest fight — sound-driver
  state, not game phase. Do not readopt.
- ~~`0xFFAB97` / `0xFFAB9C` timer disproof~~ — **retracted**: `0xFFAB97`
  IS the authoritative timer; W1's post-write read caught the written
  value after one legitimate BCD tick. See "The round timer — FOUND".

## W2 probe gotchas (additions)

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

## What training-mode readiness still lacks

1. **Facing** — not hunted; candidates likely near the X/Y fields.
2. **Wins-per-player** — not investigated.
3. **round_over (`0xFFB5E0`) write-test** — still correlation-only.
4. Full roster id cross-check beyond the 4 ids exercised (1, 3, 7, 9).
5. The ~9 s **GAME OVER gate window** (draw-timeout only) if it ever
   matters in practice.
