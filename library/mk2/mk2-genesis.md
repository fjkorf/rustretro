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
  progress: "Memory-RE pass (2026-08-27, headless FBNeo Megadrive driver on MCP
    port 4032). Fighter structs (char id, health x2 incl. mirror), the
    match-decision flag, and the block/stride layout are LIVE-VERIFIED
    (write-tests where possible). Open unknowns: authoritative round timer,
    world/screen position, facing, wins, a menu-phase gate discriminator,
    and 4 of 6 pad buttons (block especially)."
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

World X / screen position and facing were **not found**: a walk produced a
`+90` shift in one u16 candidate (`0xFFB18B`), but a forced-teleport write
test (`write_memory` to 5000) produced no visible on-screen jump —
**disproven**, not adopted.

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
| `0xFFB5E0` | **match-decision flag** (u8, LIKELY) | `0` across 26 live-combat samples spanning 3 different matchups (Liu Kang vs Baraka / Rayden / Reptile) and a wide range of health values including near-death (as low as 25/120); becomes `1` at the moment a round is decided by KO and stays `1` through the "`<NAME>` WINS" screen, confirmed in 2 independent matches. **Not write-tested** (no attempt was made to force it and observe an effect) and **not checked against char-select / title / attract** — unlike the arcade port's `screen_state`, there is no confirmed discriminator here to rule out menu-phase false positives in the gate below. |

### The round timer — not found

Two BCD-shaped byte candidates, **`0xFFAB97`** and **`0xFFAB9C`**, were
found by diffing two dumps taken ~1 real second apart during live combat:
both read `0x99` then `0x98` in the same window the on-screen timer went
99→98 (cross-checked against an *independent* second fight, where the same
two addresses read `152→151` decimal = `0x98→0x97` hex, matching that
fight's own 98→97 timer tick). This looked like a strong two-copy timer
candidate, matching the arcade port's own pattern of small runs of
identical mirrored values — **but it is disproven for enforcement**:
writing `0x50` to both addresses and letting ~15 frames pass produced
`0x49` and `0x99` respectively — neither the written value nor a clean
revert to the pre-write value, consistent with a fast-changing counter
(more than one tick per 15 frames) that only coincidentally matched the
displayed BCD digits at the two sampled instants. No authoritative,
write-stable timer store was found. This is the same open problem the
arcade port hit (`mk2.md`'s "The round timer — partially open" section);
`timer_hold` in the profile is a **non-functional placeholder**.

## The controllable gate

```
byte_zero(round_over)        # 0xFFB5E0, LIKELY: kills the KO/WINS screen
health_in_range(1, 120)      # kills 0-health (dead/menu-garbage) frames
```

This is **thinner than the arcade port's 3-condition gate**: no
`screen_state`-equivalent byte was found to positively identify "we are in
a live round" versus char-select / ladder / attract. The arcade port
documented an almost-identical leak before `screen_state` was found (both
`round_over`-style and health-range conditions read "true" on the
char-select screen there too). **This profile's gate has not been checked
against a genuine Genesis char-select or title screen** — treat it as an
open risk, not a verified-clean gate, until that check is done.

## Enforcement — what actually works

| lever | status |
|---|---|
| health refill | **Likely works** via `write_memory` to both `health` and `health_mirror` on the target side (`0xFFB622`+`0xFFB624` for P1, `0xFFB712`+`0xFFB714` for P2) — the primary byte is write-tested and authoritative; the mirror is written defensively by analogy with the arcade port's dual-accumulator finding, not because independent divergence was observed here. |
| health_max | **120** (`0x78`) — round-start fill value, and the write-verified full-bar value. |
| timer hold | **Not functional** — no authoritative store found (above). |
| credits | **N/A** — home-console cartridge, no coin/credit system; `start` joins/continues directly. |

## Buttons — partially open

The core reports only generic Genesis pad descriptors (`P1 Button A/B/C/X/Y/Z`,
`P1 Mode`, directions, Start) at boot — no game-specific labels to read off,
unlike the arcade FBNeo driver.

| RETRO button | observed effect | confidence |
|---|---|---|
| `a` | raised-leg kick animation; connects for 15-24 damage (max 120) against a stationary target, verified twice | **Kick, verified (damage-tested)** |
| `r` | visually **identical** raised-leg kick to `a`; also connects for damage, verified twice | **Kick, verified (damage-tested)** — not distinguished from `a`; may be the same logical kick reachable from two RETRO buttons, or a second kick move whose recovery pose happens to look the same in the captured frames |
| `b` | showed a straight-arm punch pose in one *uncontrolled* trial (no proximity/timing control); landed **zero** measurable damage in a range-controlled trial | **inconclusive** — plausibly a shorter-range punch that whiffed at kick range, not disproven |
| `x`, `y`, `l` | no visible pose change and zero damage in every trial | **no effect observed** — possibly unmapped by this core/pad-mode, or simply require a setup this session didn't hit |
| Block | **not verified either way** — every hold-during-CPU-attack trial happened to land in a window where the CPU never actually swung (confirmed via screenshots: full health, "FIGHT!!"/neutral poses throughout), so no trial actually tested whether damage was prevented |

`attack_chords` in the profile therefore only populates `HK: ["a", "r"]`;
`LP`, `HP`, `LK`, and `Block` are empty placeholders, not guesses — filling
them incorrectly would actively mis-drive the training/shadow input layer
per `docs/game-profiles.md`'s chord contract, which is worse than leaving
them honestly blank.

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
  forced write of `5000` produced no visible teleport. Disproven.
- **`0xFFAB97` / `0xFFAB9C`** (timer candidates): see "The round timer —
  not found" above. Read-correlated, write-disproven.

## What training-mode readiness still lacks

1. **Authoritative round timer** — needed for `timer_hold`; no store found.
2. **World/screen position and facing** — completely open; the one
   position candidate tried was disproven.
3. **A menu-phase gate discriminator** (arcade's `screen_state`
   equivalent) — this profile's 2-condition gate has not been checked
   against a real char-select/title/attract screen and may leak the same
   way the arcade port's did before that fix.
4. **Block, and 3 of the remaining 4 attack buttons** (`LP`/`HP`/`LK`
   unresolved, `b`/`x`/`y`/`l` inconclusive-to-unmapped) — see Buttons
   above.
5. **Wins-per-player** — not investigated this session.
6. Full roster id cross-check beyond the 4 ids exercised (1, 3, 7, 9).
