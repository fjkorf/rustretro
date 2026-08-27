# Game profiles — the design contract

Per-game knowledge is DATA, not compiled constants. This is simultaneously
the fix for the "hand-kept in four places" drift problem and the portability
architecture for game #2 (sf2ce) and beyond (MK2 arcade/Genesis).

## Two tiers

- **`library/<game>/family.json`** — port-independent vocabulary: roster
  (ids → names, select slots, bosses), move/attack class lists, block style
  (`back_hold` vs `button`). Shared by every port of the game and stamped
  into trained models.
- **`library/<game>/<game>.profile.json`** — one PORT of the game: core
  identity + capability prerequisites, memory map (endianness, fighter
  blocks + field offsets, named globals), the controllable-gate condition
  list, enforcement values, stage/opponent selector, feature calibration,
  attack-class → button-chord table, per-core button-name table.

Loaded once at startup: `--game library/<game>` (default `library/asurabld`)
→ `profile::init(dir)`; consumers call `profile::current()`. The Python side
(`shadow_train.profile`) reads the SAME files. Model `meta.json` carries
`family` + `port`; deploy warns on port mismatch (cross-port shadows are a
supported experiment, not an accident).

## Rules

1. **Code refers to globals by NAME** (`profile.global("round_timer")`),
   never by raw address. New addresses go in the profile, not in code.
2. **Logic lives once.** The gate is evaluated from the condition list by
   one evaluator per language surface (Rust native; Python mirrors it;
   Lua ASKS via a binding — it never re-implements). The condition
   vocabulary is closed: `byte_zero`, `word_zero`, `health_in_range`,
   `bcd_valid_nonzero`. A game that needs more gets a Lua adapter hook —
   an explicit decision, not a schema creep.
3. **Class lists size the model heads.** Nothing may hardcode 9 moves /
   6 attacks; trainer and runner size from the class lists (meta.json is
   authoritative for a loaded model; the profile for new fits).
4. **Chords are data.** intent→mask compiles from `attack_chords` (button
   names → RETRO bits) on both Rust and Python sides.
5. **`library/<game>/<game>.md` remains the literate evidence document**
   (how each value was verified); the profile is its machine-readable
   extract. asurabld.md's tables are the reference example.
6. **Endianness/addressing is a memory-map property**, not an assumption:
   read helpers consult `memory.endianness`. (Bit-addressed spaces — the
   TMS34010 — will extend this when the MK2-arcade bridge lands; the field
   exists so that change is additive.)

## What stays out of the profile

Genre logic (feature formulas, kNN policy, decision cadence, dummy modes,
matchup grid) is engine code. Game-specific *behavior* beyond the schema
(overlays, scripted drills) goes in the per-game Lua script via API v3
bindings — reading the profile, never restating it.
