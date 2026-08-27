"""Asura Blade (Fuuki FG-3) game-memory constants — loader view over the
game-profile JSON.

SOURCE OF TRUTH: `library/asurabld/family.json` + `library/asurabld/
asurabld.profile.json`, loaded through `shadow_train.profile` (see
docs/game-profiles.md for the two-tier schema and the contract this loader
implements). Those JSON files are themselves the machine-readable extract of
`library/asurabld/asurabld.md`, which remains the literate evidence document
for HOW each value was reverse-engineered and live-verified (see its
"Fighter data blocks — the corrected model", "round-timer", "health-blocks",
"system-control", and "Execution architecture" sections).

This module used to be a hand-kept Python mirror of a subset of asurabld.md,
with two OTHER hand-kept copies of the same table (`src/record.rs`,
`library/asurabld/training.lua`) that had to be updated in lockstep whenever
a value changed. The profile JSON is now the SINGLE source of truth shared
by the Rust runner, the Lua script, and this module (`src/profile.rs` reads
the same files on the Rust side) — this module is just this project's
loader view over it, kept import-cheap and preserving every constant name
existing callers already depend on.

`shadow_train.dataset` also defines calibration constants (GROUND_Y et al)
sourced from the profile's `calibration` block — a *feature-scaling*
concern, distinct from this module's raw memory addresses, but now backed
by the same JSON (see dataset.py's module docstring).
"""

from __future__ import annotations

from . import profile as _profile

_P = _profile.get()

# ── fighter allocation blocks ───────────────────────────────────────────────
# Two fighter blocks, STRIDE bytes apart; which one is "P1"/"the human"
# varies by mode and round -- anchor at round start via X (see asurabld.md's
# caveat), never assume fixed slot order.
BLOCK1 = _P.block1()
BLOCK2 = _P.block2()
STRIDE = _P.stride()
assert BLOCK2 - BLOCK1 == STRIDE, "block stride mismatch -- re-verify the profile"

# ── per-block field offsets ("Fighter data blocks — the corrected model") ──
TIMER = _P.field_off("timer")[0]           # u16: free-running frame timer, counts down every frame
ANIM = _P.field_off("anim")[0]             # u16: walk/animation frame counter
ACTION = _P.field_off("action")[0]         # u16: current command/action index
X = _P.field_off("x")[0]                   # u16: screen X, +right
Y = _P.field_off("y")[0]                   # u16: screen Y, +down; ground == GROUND_Y
FACING = _P.field_off("facing")[0]         # u8: 0 == facing left
WEAPON = _P.field_off("weapon")[0]         # u8: 0 == armed, 1 == disarmed
HEALTH = _P.field_off("health")[0]         # u8: max 0xEF; regenerates ~1%/1.5s standing neutral
HEALTH2 = _P.field_off("health2")[0]       # u8: paired health byte (2-stacked-bars hypothesis)
METER = _P.field_off("meter")[0]           # u8: super meter
METER_MAX = _P.field_off("meter_max")[0]   # u8: per-character max-meter constant
CHAR_ID = _P.field_off("char_id")[0]       # u8

# ── system / match-control addresses ────────────────────────────────────────
CHAR_SELECT = _P.global_addr("char_select")   # char-select countdown (BCD); 0 outside select.
                         # Gate v3 discriminator: the v2 composite gate is
                         # TRUE on the char-select screen (probe-verified
                         # 2026-08-25) -- require this byte == 0 too.
ROUND_TIMER = _P.global_addr("round_timer")   # BCD seconds; +1 byte is the subsecond countdown
DEMO_FLAG = _P.global_addr("demo_flag")       # scene-advance latch (coin/start); record.rs calls
                         # this "demo_flag" -- semantics still being pinned
                         # down, recorded raw rather than gated on
CREDITS = _P.global_addr("credits")
ROUND_OVER = _P.global_addr("round_over")     # per-round-end latch inside the fight loop
ABORT = _P.global_addr("abort")               # in-game abort -> game-over/continue path
MATCH_END = _P.global_addr("match_end")       # nonzero == match result posted

# Cross-block combo-landing counters. Nonzero doubles as "the OTHER fighter
# is in hitstun" (per asurabld.md's caveat + the FBNeo training-mode Lua);
# each lives inside the OTHER block's own STRIDE-byte span, not its own.
COMBO_ON_B2 = _P.global_addr("combo_on_b2")   # block1's combo landing on block2 (inside block1's span)
COMBO_ON_B1 = _P.global_addr("combo_on_b1")   # block2's combo landing on block1 (inside block2's span)

# ── world constants (training.lua's reset_positions(), profile `positions`) ─
GROUND_Y = _P.positions["round_start_y"]
ROUND_START_X_LEFT = _P.positions["round_start_x_left"]
ROUND_START_X_RIGHT = _P.positions["round_start_x_right"]
ROUND_START_Y = GROUND_Y

# ── roster (char id at CHAR_ID/+0x639) ──────────────────────────────────────
# 8 playable characters (ids 0-7) + 2 bosses (0x08/0x09, per the cheat DB).
# Sourced from library/asurabld/family.json's roster table (id -> name,
# select-screen slot, boss flag) -- the same table src/profile.rs's
# GameProfile::char_name reads on the Rust side.
CHAR_NAMES: dict[int, str] = {r.id: r.name for r in _P.roster}


def char_name(char_id: int) -> str:
    return _P.char_name(char_id)


def matchup_slug(me: int | None, opp: int | None) -> str:
    """Model-directory naming for matchup-filtered fits:
    'goat-vs-rosemary', 'goat' (any opponent), 'any-vs-rosemary', or 'all'."""
    return _P.matchup_slug(me, opp)
