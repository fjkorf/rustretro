"""Asura Blade (Fuuki FG-3) game-memory address constants.

SOURCE OF TRUTH: `library/asurabld/asurabld.md` — every value below was
reverse-engineered and live-verified there; see that file for how ("Fighter
data blocks — the corrected model", "round-timer", "health-blocks",
"system-control", and the "Execution architecture" hop-flag table).

This module is a hand-kept Python mirror of a SUBSET of asurabld.md, not a
generated one (full generation from the markdown is out of scope for now).
Two OTHER hand-kept copies of this same table exist and must be updated
in lockstep whenever these values change:

  * src/record.rs                    (GameMap::default(), struct Fighter)
  * library/asurabld/training.lua    (BLOCK1/BLOCK2/OFF/ADDR tables)

`shadow_train.dataset` also defines its own `GROUND_Y = 216` — that is a
*calibration* constant for feature scaling, a separate concern from this
module's raw memory addresses (see `runtime.py`'s module docstring for why
`dataset.py` is treated as frozen/unmodified). It is not wired to this
module on purpose, even though the numeric value is the same fact.
"""

from __future__ import annotations

# ── fighter allocation blocks ───────────────────────────────────────────────
# Two 0x0DB4-stride fighter blocks; which one is "P1"/"the human" varies by
# mode and round -- anchor at round start via X (see asurabld.md's caveat),
# never assume fixed slot order.
BLOCK1 = 0x403798
BLOCK2 = 0x40454C  # == BLOCK1 + STRIDE
STRIDE = 0x0DB4
assert BLOCK2 - BLOCK1 == STRIDE, "block stride mismatch -- re-verify against asurabld.md"

# ── per-block field offsets ("Fighter data blocks — the corrected model") ──
TIMER = 0x00       # u16: free-running frame timer, counts down every frame
ANIM = 0x12        # u16: walk/animation frame counter
ACTION = 0x50      # u16: current command/action index
X = 0x54           # u16: screen X, +right
Y = 0x56           # u16: screen Y, +down; ground == GROUND_Y
FACING = 0x61      # u8: 0 == facing left
WEAPON = 0x65      # u8: 0 == armed, 1 == disarmed
HEALTH = 0x177     # u8: max 0xEF; regenerates ~1%/1.5s standing neutral
HEALTH2 = 0x179    # u8: paired health byte (2-stacked-bars hypothesis)
METER = 0x17B      # u8: super meter
METER_MAX = 0x17F  # u8: per-character max-meter constant
CHAR_ID = 0x639    # u8

# ── system / match-control addresses ────────────────────────────────────────
ROUND_TIMER = 0x40000A   # BCD seconds; +1 byte is the subsecond countdown
DEMO_FLAG = 0x4065D8     # scene-advance latch (coin/start); record.rs calls
                         # this "demo_flag" -- semantics still being pinned
                         # down, recorded raw rather than gated on
CREDITS = 0x40655D
ROUND_OVER = 0x40646E    # per-round-end latch inside the fight loop
ABORT = 0x403678         # in-game abort -> game-over/continue path
MATCH_END = 0x402A32     # nonzero == match result posted

# Cross-block combo-landing counters. Nonzero doubles as "the OTHER fighter
# is in hitstun" (per asurabld.md's caveat + the FBNeo training-mode Lua);
# each lives inside the OTHER block's own 0xDB4 span, not its own.
COMBO_ON_B2 = 0x4041E7   # block1's combo landing on block2 (inside block1's span)
COMBO_ON_B1 = 0x40470B   # block2's combo landing on block1 (inside block2's span)

# ── world constants (training.lua's reset_positions()) ──────────────────────
GROUND_Y = 216
ROUND_START_X_LEFT = 84
ROUND_START_X_RIGHT = 232
ROUND_START_Y = GROUND_Y

# ── roster (char id at CHAR_ID/+0x639) ──────────────────────────────────────
# 8 playable characters (ids 0-7) + 2 bosses (0x08/0x09, per the cheat DB).
# COMPLETE mapping, live-verified 2026-08-25 by the headless roster probe:
# one boot per char-select slot, id read in-fight, name read from the select
# screen + health bar (the three previously known ids all reconfirmed).
# Select-screen slot order (cursor Rights from default): 0=yashaou 1=taros
# 2=zamb 3=goat 4=footee 5=rosemary 6=lightning 7=alice.
# Keep in lockstep with record.rs::char_name and asurabld.md's roster table.
CHAR_NAMES: dict[int, str] = {
    0: "yashaou",
    1: "goat",
    2: "lightning",
    3: "footee",
    4: "alice",
    5: "taros",
    6: "zamb",
    7: "rosemary",
    8: "curfue",     # boss; playable via hold Down+Start through the map screen
    9: "sgeist",     # boss; playable via hold Up+Start through the map screen
}


def char_name(char_id: int) -> str:
    return CHAR_NAMES.get(char_id, f"c{char_id}")


def matchup_slug(me: int | None, opp: int | None) -> str:
    """Model-directory naming for matchup-filtered fits:
    'goat-vs-rosemary', 'goat' (any opponent), 'any-vs-rosemary', or 'all'."""
    if me is None and opp is None:
        return "all"
    if opp is None:
        return char_name(me)
    if me is None:
        return f"any-vs-{char_name(opp)}"
    return f"{char_name(me)}-vs-{char_name(opp)}"
