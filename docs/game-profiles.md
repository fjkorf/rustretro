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
   vocabulary is closed: `byte_zero`, `word_zero`, `word_in`, `health_in_range`,
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
7. **`pins` hold volatile game settings for the whole session.** A pin is a
   named global plus a byte value the app re-asserts once a second from
   boot, independent of training mode and the fight gate (settings matter
   in menus). Use them for options the game keeps in plain RAM that a cold
   boot would silently reset — the reference case is MK2 Genesis's
   per-port 6-button pad flags (`p1_pad_six_button`/`p2_pad_six_button`),
   without which recordings degrade to 3-button (no Block button) and
   attack labels are poisoned. Every pin global must resolve at load;
   startup logs each active pin. Evidence for the pinned address and the
   poll-vs-latch behavior belongs in the game's `.md`, like any address.

## What stays out of the profile

Genre logic (feature formulas, kNN policy, decision cadence, dummy modes,
matchup grid) is engine code. Game-specific *behavior* beyond the schema
(overlays, scripted drills) goes in the per-game Lua script via API v3
bindings — reading the profile, never restating it.

## Controls contract (the Controls phase)

Controls are a four-layer pipeline; only layer 1→2 is stored, everything
else is resolved for display:

1. **Physical → RETRO** lives in `keymap.json` (schema v2: keyboard values
   are chord lists; `gamepad_by_device` keys whole maps by pad name with
   `gamepad` as the generic fallback). The RETRO 12-bit mask remains the
   wire format everywhere (recordings, shadow, Lua, MCP).
2. **Action vocabulary** = `input_config::action_rows(port, descriptors)`:
   directions + Start/Coin + the profile's attack classes (chords) + any
   remaining button the core described. Name chain: profile → core
   descriptor → raw RETRO. Buttons neither profiled nor described are
   omitted (the core not naming them is evidence the game ignores them).
3. **Core descriptors** are captured from
   `RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS` into
   `DebugState::input_descriptors` (FBNeo sends them per game;
   fbalpha2012 sends none — the chain degrades gracefully).

Every human-facing surface (Controls panel, calibration wizard, Help,
Input monitor) renders action rows — never raw maps.
