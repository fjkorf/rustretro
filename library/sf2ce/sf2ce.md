---
schema_version: 1

rom:
  name: "Street Fighter II': Champion Edition"
  system: cps1
  archive: "sf2ce.zip"
  fbneo_short_name: "sf2ce"   # confirmed from the core's own boot log ("Game: sf2ce")

settings: {}

meta:
  genre: fighting
  year: 1992
  developer: "Capcom"
  progress: "profile-schema proof (game #2). Roster + 6-button family facts sourced
    and cross-checked from two independent community sources. Fighter-struct
    base/stride and the health field are LIVE-VERIFIED against this exact
    romset. Position/timer/char_id offsets sourced from a cheat table for a
    DIFFERENT CPS1 revision were tested and found WRONG for this romset —
    correctly omitted from the profile rather than carried over."
  tags: [arcade, 2d-fighter, cps1, m68000, 6-button]
---

## Overview

Street Fighter II': Champion Edition runs on Capcom's CPS-1 hardware: a 68000
main CPU plus a Z80/OKI-M6295 sound subsystem. In RustRetro it runs under
**fbalpha2012**; the core publishes no libretro memory map, so — same as
asurabld — everything below is reached through the **Sek snapshot bridge**:
one WRAM window declared in `sf2ce.busmap.json` (`0xFF0000`, 64 KiB, flagged
`RETRO_MEMDESC_SYSTEM_RAM`) covering the entire CPS-1 work-RAM address space
that every known SF2 RAM fact (ours and the community's) lives inside.

This session's job was narrower than a full RE pass: **prove the two-tier
profile schema on a second game family** (`docs/game-profiles.md`), using
whatever could be verified live in the time available, and being scrupulously
honest — in the profile's `_STATUS` and here — about what is sourced-but-
unverified vs. actually tested vs. unknown. Do not treat `sf2ce.profile.json`
as production-ready; the "Unknown / next steps" section below is the todo
list for the real RE pass.

## Method

1. **Research.** Web search turned up two credible, independently-authored,
   and internally-consistent sources for CPS-1 SF2 RAM facts:
   - `sf2ceua.dat`, a Kawaks/FBA cheat table for the sf2ce**ua** (USA)
     revision, mirrored at
     `github.com/Eignar17/kawaks-cheats-Fba-MAME/blob/master/cheats/sf2ceua.dat`.
     Gives per-player struct addresses for health, char-select cursor,
     win-count, and dozens of move/motion cheats, always in P1/P2 pairs
     exactly `0x300` apart.
   - The mame-rr / Fightcade2 "Street Fighter II hitbox viewer" Lua script
     (`gist.github.com/cracyc/01b6d1c93b3b9937eb500dff157fc832`, profile
     `games = {"sf2"}`), which independently states `player = 0xFF83C6`,
     `player_space = 0x300`, and gives struct-relative field code:
     `pos_x = read_i16(base+0x06) - screen_left`, `pos_y = read_i16(base+0x0A)`,
     `flip_x = read_u8(base+0x12)`, with `screen_left` at `0xFF8BD8` and
     `match_active` a bit of the word at `0xFF8008`.
   - `fightcadeorg/fightcade-detectors`' `sf2ce.inf` — Fightcade's own,
     production-used round-detector definition for this exact `sf2ce` short
     name. This is the single most credible source found (it is live
     infrastructure, not a fan cheat table) and gives: round-timer byte
     `0xFF8ABF` (value `153`=`0x99` at round start), P1/P2 win-count bytes
     `0xFF864F`/`0xFF894F`, P1/P2 char-id bytes `0xFF864E`/`0xFF894E`, and the
     12-character roster with ids 0–11.
   - These three sources **agree with each other exactly** on the `0xFF83C6`
     / `+0x300` stride and on the roster id→name mapping. That cross-source
     agreement is why the roster and the block/stride numbers are trusted
     more than the individual field offsets (see Verified vs. sourced below).
2. **Empirical pass**, headless MCP on port 4035 only, against the real
   `~/games/roms/sf2ce.zip` (FBNeo short name confirmed `sf2ce` in the boot
   log — the **base/World** revision, not `sf2ceua`). Coin (`select`) +
   `start` reliably reaches character select and a real fight; attract mode
   also free-runs demo fights (CPU vs CPU) without spending credits, which is
   where most verification reads below were taken. Screenshots via
   `app://screen` were used throughout to confirm "this is really a live
   fight" before trusting any read (menu/attract-card reads are garbage —
   confirmed directly: `0xFF83C6` reads wildly out-of-range values like 229,
   240, 5 while sitting on the character-select screen, only settling into a
   plausible 0–150 health range once a round is actually in progress).
3. **Headless runs uncapped** (faster than real wall-clock time) — a
   discovery from this session worth recording for the next RE pass. Several
   early reads landed mid-KO-tally or already-back-at-title because a `time.
   sleep()` of a couple of seconds corresponds to much more than a couple of
   seconds of emulated match time. Pause/resume bursts and `pause()` +
   `read_memory()` (frozen, no race) were used once this was understood.

## Verified this session (live memory read, screenshot-correlated)

| Fact | Value | Evidence |
|---|---|---|
| Fighter struct base (P1) | `0xFF83C6` | During a live demo fight (Ryu vs Ken, confirmed via screenshot — health bars visibly non-full, no "INSERT COIN"/"PUSH START" overlay), a before/after 64 KiB WRAM diff across ~6s of real combat showed this byte drop `148 -> 124`. |
| Fighter struct base (P2) | `0xFF86C6` | Same diff, same instant: `144 -> 80`. Exactly `0xFF86C6 - 0xFF83C6 = 0x300` from P1's base — matches the stride independently claimed by both sourced tables. |
| Struct stride | `0x300` | As above; also the only stride consistent with every P1/P2 address pair in the sourced cheat table (health, char-select, win-count all differ by exactly `0x300` between P1 and P2 entries). |
| Health field offset | struct **base + 0x00** (1 byte) | The bytes that dropped during the diff above ARE the struct base addresses themselves — i.e., health lives at offset 0, not the sourced `sf2ceua` offset of `+0x22` (`0xFF83E8`). Directly re-tested: `0xFF83E8`/`0xFF86E8` stayed flat at 0 through the same live fight where `0xFF83C6`/`0xFF86C6` moved. |
| Health value range | plausible **0–~144–148** | `0xFF86C6` was caught at exactly `144` (`0x90`, the value the sourced cheat table calls the energy-cheat fill value) early in a round; `0xFF83C6` was caught at `148`. Neither the literal round-start frame nor the literal zero-health KO frame was pinned down cleanly (see Unknown below) — treat `144` as "consistent with", not "confirmed as", the hard ceiling. |
| Screen resolution | `384x224` | The core's own `retro_get_system_av_info` boot log: `base_width: 384, base_height: 224` — this is a hardware fact from the emulator itself, not a memory read, but it is a directly observed and 100% certain number (used as `calibration.SCREEN_W`). |
| Roster ordering (partial) | id 0 = Ryu, id 1 = E.Honda (adjacent) | On the character-select screen, cursor defaults to "1P RYU"; pressing `right` once moved the highlighted portrait+name to "1P E.HONDA". This confirms the roster's declared order for at least the first two entries. |
| Roster names (visual) | Ryu, Ken, Chun-Li, Zangief, Balrog all seen on-screen | Multiple demo fights and one credited fight showed on-screen character-name HUD labels exactly matching family.json's roster names (Ryu vs Ken; Ryu vs Chun-Li; a Zangief/Balrog matchup card). This is a visual/textual confirmation of the name spellings, not a memory-address confirmation of numeric ids beyond the 0/1 pair above. |
| Coin/start button mapping | `select`=coin, `start`=start work | `select` presses incremented the title screen's `CREDIT=` counter (observed 0→1→2 across sessions); `start` reliably advances title→char-select→fight. |
| No asurabld leakage | confirmed | See "No-leakage proof" below. |

## Sourced but NOT verified this session

These come from the sources in Method above, are internally consistent with
each other, but were not (or could not yet be) checked against a live read
of *this* romset:

- **Roster ids 2–11** (Blanka, Guile, Ken, Chun-Li, Zangief, Dhalsim, Bison,
  Sagat, Balrog, Vega) and their numeric char-ids — from `fightcade-
  detectors/sf2ce.inf`'s `char=` list, cross-checked against the `sf2ceua.
  dat` char-select cheat's hex values (both agree exactly). Only ids 0/1
  were exercised via the select-screen cursor this session.
- **6-button chord mapping** (`LP=y MP=x HP=l LK=b MK=a HK=r`) — this is the
  standard, widely-documented FBNeo CPS-1 6-button-to-RETRO-pad convention,
  but no button press was correlated to an on-screen hit/animation this
  session, so it is unverified in the "does pressing y actually throw a
  jab" sense.
- **`enforcement.timer_hold`/`health_max`/etc.** — policy constants in the
  shape of asurabld's, not independently discovered facts; `health_max=144`
  has the partial support noted above.

## Disproven this session (sourced address, tested, found wrong — NOT carried into the profile)

The `sf2ceua` cheat table's field offsets, applied to the `sf2ce` (World/base)
romset actually running here, do not hold what they claim to. CPS-1 games
frequently shift absolute static-RAM layout between regions/revisions even
when the rest of the engine is identical, and this is a clean example:

- **`char_id` at struct base `+0x288`** (`0xFF864E` for P1): selected
  E.Honda (id 1 per the sourced roster) at the character-select screen, then
  read this address in the resulting fight — it read `0`, i.e. "Ryu", not
  `1`. Wrong for this romset.
- **`round_timer` at `0xFF8ABF`**: read during a live, ongoing demo fight
  across a resume/pause pair — the value *increased* (`20 -> 36`) instead of
  counting down. A round timer cannot increase during active play. Wrong for
  this romset (or at minimum, not a simple monotonic countdown byte at this
  address in this build).
- **`x`/`y`/`facing` at struct `+0x06`/`+0x0A`/`+0x12`**: read flat/constant
  (`0`, `0`/`256`, `0`/`0`) through multiple live fights with visibly moving
  characters. Wrong for this romset (or the true base has itself shifted the
  same way `health` did, and these relative offsets need to be re-derived
  against the *empirically found* base rather than the sourced one — this
  is the most promising lead for the next session: since `health` moved from
  sourced-relative `+0x22` to actual `+0x00`, a uniform re-anchoring attempt
  is worth trying before a full re-scan).

None of these three are in `sf2ce.profile.json` — they are recorded here,
disproven, so the next RE session doesn't re-waste time re-sourcing and
re-trusting the same wrong cheat-table numbers.

## Unknown / not found this session

- **Credits/coin-counter address.** No credible source located (web search
  came up empty specifically for this fact); not read live either.
- **Round-state / demo-attract / match-end / abort flags** — i.e. anything
  playing asurabld's `round_over`/`abort`/`match_end`/`demo_flag` role. The
  hitbox script's `word@0xFF8008 & 0x08` "match active" bit is a candidate
  but (a) it's a bitmask test, outside the profile's closed gate vocabulary
  (`byte_zero`/`word_zero`/`health_in_range`/`bcd_valid_nonzero`) and (b) it
  was never independently confirmed against this romset. Left out.
- **The real health field's exact semantics** — is struct-base a *pure*
  1-byte 0–144 health value, or does it share the byte with something else
  (the P1 reading of 148 > the P2 reading of 144's presumed max is a small
  but real red flag)? A clean round-start capture (paused at the literal
  first frame of "ROUND 1", both bars full) is the next thing to get.
- **char-select grid position (`select_slot`)** for every roster entry —
  `family.json` intentionally leaves these `null`; only the "Ryu is default,
  right-arrow goes to E.Honda" adjacency was checked.
- **x/y/facing/char_id/wins/round_timer real addresses** for this specific
  `sf2ce` (World) revision — see Disproven above. The likely next move is a
  paired snapshot-diff during controlled P1 movement (now that input control
  is confirmed working) rather than reusing the wrong-revision cheat table.

## No-leakage proof

With `--game library/sf2ce` loaded (port 4035, this session's instance):

```
run_lua: return tostring(game.char_name(0))        -> "ryu"      (NOT "yashaou")
run_lua: return tostring(game.addr('credits'))     -> "nil"      (NOT 0x40655D)
```

Confirmed twice — once against the first draft profile, once again against
the final `sf2ce.profile.json` after the health-offset fix, both times after
a fresh process restart on port 4035 (`--game library/sf2ce`). No asurabld
data leaked into either the family vocabulary or the memory map.
