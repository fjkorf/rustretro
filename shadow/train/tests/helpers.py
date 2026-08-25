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
