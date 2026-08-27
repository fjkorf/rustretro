---
page:
  name: PortingAGame
  label: "Porting a Game"
  default: false
---
<!-- Once mounted in litui, this page inherits shared styles via parent: "_tutorials.md". -->

# Porting a Game

**What you'll do:** take a second fighting game from "boots under a core" to a fully
working training/shadow target, following the same path `library/asurabld/` already
walked.

## The profile schema

Per-game knowledge lives as DATA under `library/<game>/`, never as compiled constants —
see [Game Profiles](/docs/game-profiles.md) for the full contract. Two files per game:

- **`family.json`** — port-independent vocabulary: roster (ids → names, select slots,
  bosses), move/attack class lists, block style (`back_hold` vs `button`). Shared by
  every port of the game.
- **`<game>.profile.json`** — one port: core identity, the memory map (endianness, CPU
  family, fighter blocks + field offsets, named globals), the controllable-gate
  condition list, enforcement values, stage/opponent selector, feature calibration,
  and the attack-class → button-chord table.

`library/mk2/` is a live **stub** example — `family.json` with an empty roster and
`mk2.profile.json` with an empty gate, zeroed blocks, and a `_STATUS` field spelling out
that nothing is mapped yet. `library/asurabld/` is the complete reference: every field
filled, backed by `asurabld.md`'s evidence log.

## 1. Get it booting

Point `--game` at your (initially stub) profile directory:

```bash
./target/release-dev/rustretro --core <core.dylib> --rom <game.zip> --game library/<game>
```

A malformed or missing profile is a hard startup error, so start minimal: `family`,
`port`, `core` (with `provenance_game`/`provenance_core` matching what the core
expects), empty `gate: []` (vacuously controllable — fine for a boot check, **not**
fine to record training data against), placeholder `memory.blocks` at `0x0`, and
`requires: { "memory_regions": false, "save_states": false }` until you know otherwise.

Two things to get right immediately:

- **`memory.cpu`** — defaults to `"m68k"`. If the core's main CPU isn't 68k (e.g. the
  TMS34010 driving arcade MK2), set `memory.cpu` to match. FBNeo exports its Sek
  (68k) debug symbols for every game regardless of driver, so calling them against a
  non-68k CPU dereferences an uninitialized context and segfaults — this field is what
  gates that capture off.
- **`requires`** — if the profile declares `memory_regions: true` but no bus window is
  installed (`--bus-map`, or a Lua script installing one), the app warns loudly at
  startup: reads return 0 and the controllable gate stays closed. Leave both `false`
  until you've wired a bus map.

## 2. RE the fighter blocks, gate, and roster

This is the bulk of the work, and `library/asurabld/asurabld.md`'s "Method" section is
the worked example — read it alongside this list:

- **Snapshot-diff.** Pause, snapshot a Work RAM bus window, do one thing in-game (take
  damage, block, jump), step, snapshot again, diff. This is the primary discovery loop
  for fighter blocks and field offsets.
- **Write-tests.** Once a candidate address looks right (e.g. "this byte tracks
  health"), write it directly and confirm the on-screen effect matches — asurabld's
  health/facing/credits fields were all confirmed this way, not just inferred from
  correlation.
- **Headless roster probes.** To map character ids without a human at the controls:
  boot headless, drive the character-select cursor with `press_buttons`, and read the
  candidate id field once in a fight. asurabld's roster table was built by one cold
  boot per select slot this way.
- **Strict-gate discipline.** Character ids (and other in-fight-only fields) can read
  stale during menus and transition screens. Only trust a read once your gate
  condition list holds true for a couple of seconds continuously — asurabld's roster
  probe hit this directly: the plain controllable check was still `true` on the
  char-select screen, so the working probe added an extra strict condition
  (`char_select == 0`) on top before trusting an id read.

Use `--headless --mcp` for scripted probes (agents launch their own instance on port
4026+, never the user's 4025 session) and the MCP snapshot/read/pause/step tools to
drive this loop without a window.

## 3. Fill the profile, write the literate doc

Once blocks, fields, globals, the gate list, and enforcement values are confirmed:

1. Fill `family.json` (roster, move/attack classes, block style) and
   `<game>.profile.json` (memory map, gate, enforcement, stage selector if the game has
   one, calibration, attack chords) with the confirmed values.
2. Write `library/<game>/<game>.md` — the literate evidence document recording *how*
   each value was verified (which probe, which write-test, which live read), not just
   the final numbers. `asurabld.md`'s tables (roster, stages, fighter data blocks) are
   the reference shape to copy.
3. Wire a busmap sidecar (`<game>.busmap.json`) if the core needs one for the bus
   bridge, following `library/asurabld/asurabld.busmap.json`.

## Why it matters

Every piece of RustRetro that touches "which byte is health" or "which character is
id 3" reads the profile by name — none of it is compiled per-game. A game that's fully
ported this way gets training mode, the shadow loop, and the matchup grid for free,
with zero code changes.

## See also

- [Game Profiles](/docs/game-profiles.md) — the full two-tier schema reference.
- [Training Mode](/docs/tutorials/training-mode.md) — what a filled-in profile powers.
- [RAM Search](/docs/tutorials/ram-search.md) / [Tracking Changes](/docs/tutorials/tracking-changes.md) — the manual half of the snapshot-diff loop, for when you're not scripting it.

<!-- litui:live
This page is a process walkthrough spanning multiple sessions of RE work, not a single
live control surface — it has no meaningful live embed. It stays a static document page.
-->
