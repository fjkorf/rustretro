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
   vocabulary is closed: `byte_zero`, `word_zero`, `word_masked_not_all`, `health_in_range`,
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
   read helpers consult `memory.endianness`. Bit-addressed spaces — the
   TMS34010 — extend this additively via the pointer-resolved field form
   below rather than a new addressing mode of their own.
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

## Fighter-field forms

`memory.fighter_fields` entries name a value read per fighter block (1 or
2). Every entry has a `name` and a `size` (1 or 2 bytes, guest order per
`memory.endianness`); exactly one of the following supplies the address, and
they compose freely within one profile — a game can mix all three:

1. **Plain `off`** — the common case: offset from the fighter block base
   (`block1`/`block2` + `off`). asurabld's `health` (`off: "0x177"`).
2. **`globals`** — a per-block pair of named globals, for a value that lives
   OUTSIDE the fighter structs at a fixed address per player (`{"block1":
   "name_a", "block2": "name_b"}`). Consumers see a normal named field
   either way; `field_addr`/`field_off` resolve both forms to an absolute
   address.
3. **`via: "object_ptr"`** — pointer-resolved: the value lives behind a
   pointer stored IN the fighter block, and that pointer can move at
   runtime (a dynamic object pool). There is no fixed address to hand back
   — `field_off`/`field_addr` return `None` for these fields on purpose —
   so every read is a live, multi-step operation:
   dereference → cross-check → read. `off` still applies, but relative to
   the DECODED pointer target, not the block.

   The per-block pointer itself is declared once, on `memory.blocks.
   object_ptr`, and applied relative to EACH block's own base:

   ```json
   "blocks": {
     "block1": "0xC050", "block2": "0xC1CA", "stride": "0x17A",
     "object_ptr": {
       "off": "-0xC",
       "size": 4,
       "encoding": "tms34010_bitaddr",
       "valid_range": ["0x01000000", "0x01400000"],
       "cid_check_off": "0x3E"
     }
   },
   "fighter_fields": [
     { "name": "char_id", "off": "0x0", "size": 1 },
     { "name": "x", "via": "object_ptr", "off": "0x12", "size": 2 },
     { "name": "y", "via": "object_ptr", "off": "0x16", "size": 2, "signed": true }
   ]
   ```

   This is MK2 arcade's world position (docs/frames.md §5): the fighter
   struct carries a pointer at `block - 0xC` into a separate, moving object
   pool. Reading `x` for one block means:

   1. Read the raw pointer word at `block ± object_ptr.off`.
   2. **Validity check**: the raw word must fall inside
      `object_ptr.valid_range` (`[lo, hi)`) or it isn't live this frame.
   3. **Decode**: `encoding` names a closed-vocabulary transform from the
      raw word to an absolute object address — currently just
      `"tms34010_bitaddr"`: `(word - 0x01000000) >> 3`.
   4. **Staleness cross-check**: the byte at `obj + cid_check_off` MUST
      equal the byte at `block + 0` (the fighter's own `char_id`). A
      mismatch means the pool slot was reused by a different object since
      the pointer was captured — the pointer is stale even though it
      decoded to a plausible address.
   5. Only then read `obj + off` for the field's own value.

   Any failure at steps 2 or 4 makes the field **ABSENT for that read**,
   never a zero or a stale value (the RECORDER_V3 law, docs/frames.md
   §2.5) — "we don't know where the fighter is" must never be
   indistinguishable from "the fighter is at the left edge". Rust code
   performs this through [`GameProfile::object_ptr_field`], which takes a
   caller-supplied `read(addr, size) -> value` closure so `profile.rs`
   itself never touches live memory or endianness.

**`signed`** (default `false`, any field form): sign-extend the read value
at its own width instead of zero-extending. MK2 arcade's `y` is signed —
smaller is higher, and the value goes negative mid-jump.

**Consumer impact**: a pointer-resolved field has no cacheable address, so a
consumer that resolves fighter-field addresses ONCE (e.g. a fixed-slot
recorder) cannot record it until it grows a live per-frame resolver;
`src/training.rs`'s `resolve()` — called fresh every tick already — reads it
live on every use instead (see `read_object_ptr`/`read_via_x`).

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
