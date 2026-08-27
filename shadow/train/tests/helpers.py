"""Synthetic jsonl-v2 row builders for tests -- matches the row shape shipped
in `src/record.rs` / SPEC.md §5 exactly (field names, nesting, types)."""

from __future__ import annotations

import json
from pathlib import Path


def fighter(**overrides) -> dict:
    base = dict(
        x=150, y=216, facing=1, weapon=0, health=200, health2=200,
        meter=10, meter_max=100, char_id=0, wins=0, timer=0, anim=0,
        action=0, opp_right_hold=0, opp_left_hold=0,
    )
    base.update(overrides)
    return base


def make_round_rows(
    n_frames: int,
    round_id: int = 1,
    p1_block: int = 1,
    p1_input_period: int = 16,
    combo_on_b1=None,
    combo_on_b2=None,
    block1_kw: dict | None = None,
    block2_kw: dict | None = None,
    start_frame: int = 0,
    timer_bcd: int = 0x30,
) -> list[dict]:
    """n_frames controllable frames of one round, jsonl-v2 schema.

    p1_input taps Right (bit 7 = 0x80) every `p1_input_period` frames so the
    round always has nonzero p1_input mass (passes the §5 attract-demo
    filter). combo_on_b1/combo_on_b2, if given, are `frame_idx -> int`
    callables (default: always 0).
    """
    combo_on_b1 = combo_on_b1 or (lambda i: 0)
    combo_on_b2 = combo_on_b2 or (lambda i: 0)
    rows = []
    for i in range(n_frames):
        p1_input = 0x80 if (i % p1_input_period) == 0 else 0
        rows.append({
            "frame": start_frame + i,
            "round_id": round_id,
            "controllable": True,
            "p1_block": p1_block,
            "block1": fighter(**(block1_kw or {})),
            "block2": fighter(**{"x": 200, **(block2_kw or {})}),
            "gate": {
                "round_over": 0, "abort": 0, "match_end": 0,
                "timer_bcd": timer_bcd, "demo_flag": 0,
                "combo_on_b1": combo_on_b1(i), "combo_on_b2": combo_on_b2(i),
                "credits": 8,
            },
            "p1_input": p1_input,
            "p2_input": 0,
        })
    return rows


def write_jsonl(path: Path, rows: list[dict]) -> None:
    with open(path, "w") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")


# RECORDER_V3.md §3 G2's transcoder: a mechanical v2 -> v3 shape rewrite for
# the ASURABLD profile, proving the v3 row reorganization changes nothing
# for a family whose fighter_fields/gate are already what v2 hardcoded.
_V2_TO_V3_GLOBAL_RENAME = {"timer_bcd": "round_timer", "char_sel": "char_select"}

# §1.2 rule 2 order for asurabld post-§2.4: gate-condition globals (in gate
# order: round_over, abort, match_end, round_timer, char_select -- health_
# in_range has no `global`), then record_globals (§2.1/§2.4's own example
# order: combo_on_b2, combo_on_b1, demo_flag, credits). No duplicates.
_V3_GLOBAL_ORDER = [
    "round_over", "abort", "match_end", "round_timer", "char_select",
    "combo_on_b2", "combo_on_b1", "demo_flag", "credits",
]


def transcode_v2_to_v3(row: dict) -> dict:
    """One v2 row -> its v3-shaped equivalent (RECORDER_V3.md §3 G2): add
    `"v":3`; blocks pass through untouched (v2's Fighter key set already
    equals the profile's post-§2.4 fighter_fields names -- key order doesn't
    matter to the Python reader); `gate` becomes `globals`, renaming the two
    ad-hoc v2 names and reordering per §1.2 rule 2. This is a mechanical
    shape transform over whatever keys the row actually has -- it does not
    synthesize globals a v2 row never recorded (e.g. real v2 rows have no
    `char_sel`, so a transcoded row has no `char_select` either)."""
    gate = row.get("gate", {})
    renamed = {_V2_TO_V3_GLOBAL_RENAME.get(k, k): v for k, v in gate.items()}
    globals_out = {name: renamed[name] for name in _V3_GLOBAL_ORDER if name in renamed}
    # any renamed key _V3_GLOBAL_ORDER doesn't know about still gets carried,
    # appended after the known order (keeps this generic beyond asurabld).
    for name, v in renamed.items():
        if name not in globals_out:
            globals_out[name] = v
    return {
        "v": 3,
        "frame": row["frame"],
        "round_id": row["round_id"],
        "controllable": row["controllable"],
        "p1_block": row["p1_block"],
        "block1": dict(row["block1"]),
        "block2": dict(row["block2"]),
        "globals": globals_out,
        "p1_input": row["p1_input"],
        "p2_input": row["p2_input"],
    }
