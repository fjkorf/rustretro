"""jsonl-v2 recordings -> decision dataset (SPEC v2 §1-§5).

Pipeline per file:
  1. keep controllable rows; group into rounds by (file, round_id)
  2. demo filter (§5): drop rounds whose total p1_input mass is zero
  3. anchor (§5): me = the P1 block per the recorded p1_block; opp = the other
  4. one decision every P frames (§4); label = mode of the window's p1 masks,
     mapped to (move_class, attack_class) with chord rules (§3d)
  5. features per §1a, opponent fields STALE frames old (§4), stacked over the
     last K decision steps (§4); me_hitstun/opp_hitstun require the combo
     counter to have CHANGED within HITSTUN_RECENT_FRAMES, not just be
     nonzero -- see the evidence comment above _recent_change_mask()
  6. segment rounds longer than SEGMENT_DECISIONS decisions into pseudo-round
     split units (file, round_id, seg_k) -- see _segment()

Raw fields stay raw on disk; everything here is train-time (§5).
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

# ── calibration constants (SPEC §1) ─────────────────────────────────────────
GROUND_Y = 216
X_SCALE = 128.0
Y_SCALE = 128.0
TIMER_SCALE = 256.0
ANIM_SCALE = 64.0
CORNER_PX = 24
HEALTH_MAX = 0xEF
P = 8          # decision period, frames (§4)
K = 4          # stacked decision-step snapshots (§4)
STALE = 3      # opponent observation delay, frames (§4)
SCREEN_W = 320

# Task 1 (pseudo-round segmentation): training-mode sessions freeze the round
# timer, so a single round_id can run tens of thousands of frames -- with only
# 1-2 real rounds per file, splitting train/test by round_id is degenerate
# (observed on the two real recordings: 933 train vs 5611 held-out decisions
# from just 2 rounds total). Chop any round longer than SEGMENT_DECISIONS
# decisions into fixed-size pseudo-rounds so the number of split units scales
# with session length instead of "however many real KOs happened". ~150
# decisions * 1/8s each ~= 20s of play -- long enough to still contain a
# coherent chunk of a exchange, short enough that a session yields many units.
SEGMENT_DECISIONS = 150

# Task 2 (hitstun bucket audit): see the evidence comment above _bucket() and
# _recent_change_mask() below. combo_on_b1/b2 ($4041E7/$40470B) are cross-block
# "combo counter" bytes that increment once per hit landed but do NOT reset to
# 0 when hitstun ends -- they linger at the last combo's hit count as a
# display value. HITSTUN_RECENT_FRAMES bounds how long ago the counter must
# have last changed for it to still count as "opponent is actively in
# hitstun right now" rather than "combo counter is showing a stale number".
# Chosen from the observed frame-gap distribution between counter increments:
# within an actual combo, increments land ~7-25 frames apart; between combos
# the counter sits constant for hundreds to thousands of frames (max observed
# run: 7582 frames / ~126s in session-2026-08-24-training-v1.jsonl). 20 frames
# (2.5x the P=8 decision period) sits cleanly in the gap between those two
# populations.
HITSTUN_RECENT_FRAMES = 20

# RETRO mask bits
BIT_B, BIT_Y, BIT_SELECT, BIT_START = 0, 1, 2, 3
BIT_UP, BIT_DOWN, BIT_LEFT, BIT_RIGHT = 4, 5, 6, 7
BIT_A = 8
ATTACK_BITS = (BIT_B, BIT_A, BIT_Y)  # Light, Medium, Heavy (§3c)

MOVE_CLASSES = [
    "Neutral", "Forward", "Back", "Up", "Down",
    "UpForward", "UpBack", "DownForward", "DownBack",
]
ATTACK_CLASSES = ["None", "Light", "Medium", "Heavy", "Launcher", "Toss"]

# scalar feature names, in vector order (§1a minus the categorical columns)
SCALAR_FEATURES = [
    "dist_x", "dy", "me_airborne", "me_height", "me_fwd_hold", "me_back_hold",
    "me_anim", "me_timer", "opp_airborne", "opp_height", "opp_anim",
    "opp_timer", "facing_sign", "me_health", "opp_health", "health_lead",
    "me_meter", "opp_meter", "me_hitstun", "opp_hitstun", "me_corner",
]


@dataclass
class Decision:
    """One 8 Hz training example (pre-stacking)."""

    scalars: np.ndarray          # len(SCALAR_FEATURES)
    me_action: int
    opp_action: int
    opp_char: int
    move_class: int
    attack_class: int
    bucket: str
    round_key: tuple             # (file, round_id) — split unit
    me_char: int


def _attack_class(mask: int) -> int:
    pressed = [b for b in ATTACK_BITS if mask >> b & 1]
    if not pressed:
        return 0
    if len(pressed) == 3:
        return 5  # Toss
    if len(pressed) == 2:
        return 4  # Launcher
    return {BIT_B: 1, BIT_A: 2, BIT_Y: 3}[pressed[0]]


def _move_class(mask: int, s: int) -> int:
    up = mask >> BIT_UP & 1
    down = mask >> BIT_DOWN & 1
    left = mask >> BIT_LEFT & 1
    right = mask >> BIT_RIGHT & 1
    fwd = right if s > 0 else left
    back = left if s > 0 else right
    if up and fwd:
        return 5
    if up and back:
        return 6
    if down and fwd:
        return 7
    if down and back:
        return 8
    if up:
        return 3
    if down:
        return 4
    if fwd:
        return 1
    if back:
        return 2
    return 0


def _window_label(masks: list[int], s: int) -> tuple[int, int]:
    """Mode of the window's per-frame class pairs (§4: the input held most)."""
    pairs = [(_move_class(m, s), _attack_class(m)) for m in masks]
    # attacks are taps: prefer the most common NON-None attack if one occurs
    # on >=2 frames, else the modal pair's attack.
    moves = [p[0] for p in pairs]
    move = max(set(moves), key=moves.count)
    attacks = [p[1] for p in pairs if p[1] != 0]
    if len(attacks) >= 2:
        attack = max(set(attacks), key=attacks.count)
    else:
        attack = 0
    return move, attack


def _bucket(scal: dict, me_action_mask_fwdback: tuple[int, int]) -> str:
    """Situation bucket for eval/coverage (§7.3/§7.4)."""
    if scal["me_hitstun"]:
        return "defense"
    if scal["opp_hitstun"]:
        return "offense"
    if scal["me_airborne"]:
        return "air"
    if scal["me_corner"]:
        return "corner"
    return "neutral"


def _recent_change_mask(rows: list[dict], key: str, window: int) -> np.ndarray:
    """True where `gate[key]` is nonzero AND changed within the last `window`
    frames (see the HITSTUN_RECENT_FRAMES comment above for the evidence).

    Naive "nonzero" gating on combo_on_b1/b2 was the bug the audit found: the
    first eval run against the two real recordings reported 84% of ALL
    decisions in the "defense" bucket (me_hitstun nonzero), which is not
    plausible for real play. Dumping the raw byte confirmed why -- run-length
    analysis of session-2026-08-24-training-v1.jsonl round 1 (44916
    controllable frames):

        combo_on_b1: nonzero 92.0% of the round; longest constant-nonzero run
                     = 7582 frames (~126s); only 60 value changes total
        combo_on_b2: nonzero  3.2% of the round; longest run = 93 frames

    and session-2026-08-24-recorder-v2.jsonl round 1 (7493 frames):
    combo_on_b1 nonzero 34.9% (longest run 1483 frames), combo_on_b2 nonzero
    28.8% (longest run 1310 frames). A byte that stays pinned at a constant
    nonzero value for 100+ seconds is not tracking "is this fighter currently
    in hitstun" (hitstun is at most a couple seconds); it is a last-combo hit
    count that the game leaves on screen after the combo ends and only
    overwrites when the next combo starts. The value-change events themselves
    cluster tightly (7-25 frames apart) exactly while a combo is actually
    landing, then go quiet for hundreds-to-thousands of frames between
    combos -- a clean bimodal gap distribution, which is what
    HITSTUN_RECENT_FRAMES=20 is picked to split.
    """
    vals = [r["gate"][key] for r in rows]
    frames = [r["frame"] for r in rows]
    out = np.zeros(len(rows), dtype=bool)
    last_change_frame = None
    prev = None
    for i, v in enumerate(vals):
        if prev is not None and v != prev:
            last_change_frame = frames[i]
        prev = v
        if v != 0 and last_change_frame is not None and frames[i] - last_change_frame <= window:
            out[i] = True
    return out


def _rounds(path: Path):
    """Yield (round_key, rows) for controllable, non-demo rounds."""
    rounds: dict = {}
    with open(path) as f:
        for line in f:
            try:
                r = json.loads(line)
            except json.JSONDecodeError:
                continue
            if "block1" not in r:
                raise SystemExit(
                    f"{path.name}: not jsonl-v2 (no block1) — v1 recordings are "
                    "not supported; re-record with the v2 recorder"
                )
            if not r["controllable"] or r["p1_block"] is None:
                continue
            rounds.setdefault(r["round_id"], []).append(r)
    for rid, rows in sorted(rounds.items()):
        if sum(x["p1_input"] for x in rows) == 0:
            continue  # attract demo (§5)
        if len(rows) < P * (K + 1):
            continue  # too short to stack
        yield (str(path.name), rid), rows


def _decisions_for_round(round_key: tuple, rows: list[dict]):
    p1b = "block1" if rows[0]["p1_block"] == 1 else "block2"
    oppb = "block2" if p1b == "block1" else "block1"
    # active-hitstun masks (task 2) -- "recently changed", not "nonzero"
    b1_active = _recent_change_mask(rows, "combo_on_b1", HITSTUN_RECENT_FRAMES)
    b2_active = _recent_change_mask(rows, "combo_on_b2", HITSTUN_RECENT_FRAMES)
    me_active = b1_active if p1b == "block1" else b2_active
    opp_active = b2_active if p1b == "block1" else b1_active
    out = []
    for i in range(P * 1, len(rows), P):
        row = rows[i]
        me = row[p1b]
        opp = rows[max(0, i - STALE)][oppb]  # stale opponent (§4)
        s = 1 if me["facing"] == 1 else -1
        # me-holds from own mask history (§1a #4/#5)
        hist = [rows[j]["p1_input"] for j in range(max(0, i - P), i)]
        fwd_bit = BIT_RIGHT if s > 0 else BIT_LEFT
        back_bit = BIT_LEFT if s > 0 else BIT_RIGHT
        n = max(1, len(hist))
        scal = {
            "dist_x": s * (opp["x"] - me["x"]) / X_SCALE,
            "dy": (opp["y"] - me["y"]) / Y_SCALE,
            "me_airborne": 1.0 if GROUND_Y - me["y"] > 4 else 0.0,
            "me_height": max(0, GROUND_Y - me["y"]) / Y_SCALE,
            "me_fwd_hold": sum(m >> fwd_bit & 1 for m in hist) / n,
            "me_back_hold": sum(m >> back_bit & 1 for m in hist) / n,
            "me_anim": me["anim"] / ANIM_SCALE,
            "me_timer": me["timer"] / TIMER_SCALE,
            "opp_airborne": 1.0 if GROUND_Y - opp["y"] > 4 else 0.0,
            "opp_height": max(0, GROUND_Y - opp["y"]) / Y_SCALE,
            "opp_anim": opp["anim"] / ANIM_SCALE,
            "opp_timer": opp["timer"] / TIMER_SCALE,
            "facing_sign": float(s),
            "me_health": me["health"] / HEALTH_MAX,
            "opp_health": opp["health"] / HEALTH_MAX,
            "health_lead": (me["health"] - opp["health"]) / HEALTH_MAX,
            "me_meter": me["meter"] / max(1, me["meter_max"]),
            "opp_meter": opp["meter"] / max(1, opp["meter_max"]),
            # me_hitstun: self-feature, stays current (§4 -- "you know your own
            # hands"); opp_hitstun: opponent-sourced, so it's read STALE like
            # the rest of the opp_* block (§4 humanness rule, spec §1a #21).
            "me_hitstun": 1.0 if me_active[i] else 0.0,
            "opp_hitstun": 1.0 if opp_active[max(0, i - STALE)] else 0.0,
            "me_corner": 1.0
            if me["x"] <= CORNER_PX or me["x"] >= SCREEN_W - CORNER_PX
            else 0.0,
        }
        # label = what the user held over the NEXT decision window (§4)
        masks = [rows[j]["p1_input"] for j in range(i, min(len(rows), i + P))]
        move, attack = _window_label(masks, s)
        out.append(
            Decision(
                scalars=np.array([scal[k] for k in SCALAR_FEATURES], dtype=np.float32),
                me_action=me["action"],
                opp_action=opp["action"],
                opp_char=opp["char_id"],
                me_char=me["char_id"],
                move_class=move,
                attack_class=attack,
                bucket=_bucket(scal, (fwd_bit, back_bit)),
                round_key=round_key,
            )
        )
    return out


def _segment(round_decisions: list[Decision]) -> list[Decision]:
    """Split an over-long round's decisions into pseudo-round split units
    (task 1). Rekeys round_key from (file, round_id) to
    (file, round_id, seg_k), chunking every SEGMENT_DECISIONS decisions.
    Rounds at or under the threshold get a single seg_k=0 -- unchanged in
    substance, just uniformly 3-tuples so evaluate.split_by_round doesn't need
    to special-case key shape.
    """
    for idx, d in enumerate(round_decisions):
        file, rid = d.round_key
        d.round_key = (file, rid, idx // SEGMENT_DECISIONS)
    return round_decisions


def load_decisions(paths: list[Path], char_filter: int | None = None,
                   opp_filter: int | None = None) -> list[Decision]:
    """Recordings -> filtered Decision list (pre-stacking). The demo filter
    (zero p1_input rounds) is applied inside _rounds; matchup filters here.
    Shared by build() and the coverage command so both count identically."""
    decisions: list[Decision] = []
    for p in paths:
        for round_key, rows in _rounds(p):
            decisions.extend(_segment(_decisions_for_round(round_key, rows)))
    if char_filter is not None:
        decisions = [d for d in decisions if d.me_char == char_filter]
    if opp_filter is not None:
        decisions = [d for d in decisions if d.opp_char == opp_filter]
    return decisions


def build(paths: list[Path], char_filter: int | None = None,
          opp_filter: int | None = None):
    """Load recordings -> stacked dataset arrays.

    Returns dict with X (N, K*len(SCALAR_FEATURES)), y_move, y_attack, buckets,
    round keys, and the per-decision categorical columns.
    """
    decisions = load_decisions(paths, char_filter, opp_filter)
    # K-step stacking within each round (§4)
    X, y_move, y_attack, buckets, keys = [], [], [], [], []
    by_round: dict = {}
    for d in decisions:
        by_round.setdefault(d.round_key, []).append(d)
    for rk, ds in by_round.items():
        for i in range(K - 1, len(ds)):
            stack = np.concatenate([ds[j].scalars for j in range(i - K + 1, i + 1)])
            X.append(stack)
            y_move.append(ds[i].move_class)
            y_attack.append(ds[i].attack_class)
            buckets.append(ds[i].bucket)
            keys.append(rk)
    if not X:
        raise SystemExit("no usable decisions — record some real play first")
    return {
        "X": np.stack(X),
        "y_move": np.array(y_move),
        "y_attack": np.array(y_attack),
        "buckets": np.array(buckets),
        "rounds": keys,
    }


# Cap on fully-idle decisions relative to everything else at FIT time. Real
# demonstrations are ~90% (Neutral, None), which makes ~80% of kNN
# neighborhoods vote unanimously idle — temperature sampling can't produce an
# action from a unanimous jury, and a shadow that stands still stays in
# standing-range states forever (an absorbing state; observed live: the first
# deployed shadow never pressed a button). Subsampling idle decisions to
# NEUTRAL_CAP_RATIO x the active count re-weights the case store without
# inventing anything the user never did.
NEUTRAL_CAP_RATIO = 2.5


def subsample_neutral(data: dict, cap_ratio: float = NEUTRAL_CAP_RATIO, seed: int = 11) -> dict:
    """Return a copy of `data` with (move=Neutral AND attack=None) decisions
    randomly capped at cap_ratio x the count of all other decisions.
    cap_ratio <= 0 disables. Apply at FIT time only — eval's held-out side
    must keep the true distribution."""
    if cap_ratio <= 0:
        return data
    idle = (data["y_move"] == 0) & (data["y_attack"] == 0)
    n_active = int((~idle).sum())
    n_keep = int(n_active * cap_ratio)
    if idle.sum() <= n_keep or n_active == 0:
        return data
    rng = np.random.default_rng(seed)
    idle_idx = np.flatnonzero(idle)
    kept_idle = rng.choice(idle_idx, n_keep, replace=False)
    keep = np.sort(np.concatenate([np.flatnonzero(~idle), kept_idle]))
    out = {k: (v[keep] if isinstance(v, np.ndarray) else [v[i] for i in keep])
           for k, v in data.items()}
    return out
