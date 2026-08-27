"""jsonl-v2/v3 recordings -> decision dataset (SPEC v2 §1-§5, RECORDER_V3.md
§4 for the v3 additions).

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

Calibration constants and the move/attack class lists below are loaded from
the game profile (`library/asurabld/asurabld.profile.json`'s `calibration`
block and `library/asurabld/family.json`'s `move_classes`/`attack_classes`,
via `shadow_train.profile`) rather than hand-kept here -- see
docs/game-profiles.md. The module-level names are kept exactly as before
(other code and `meta.json` writing reference them by name), and the JSON's
numeric literals preserve the same int/float types the old hardcoded
constants had, so a fit's output is unaffected by this indirection.

RECORDER_V3.md §4 turns this into a two-format reader. v2 files are the
asurabld-shaped fixed struct (`block1`/`block2`/`gate`, ad-hoc global names)
and are read EXACTLY as before -- nothing about the v2 code path may change
behavior (that is what the G1 golden-refit gate checks). v3 files carry
profile-shaped rows (`"v":3`, named `block1`/`block2` maps, a `globals` dict)
plus a `.meta.json` sidecar that snapshots the recording port's own
fighter_fields/calibration/recorded-globals -- v3 files are self-describing
and must be read from THAT sidecar, never from the process-loaded profile
(see `_view_for`/`_RecordingView`), so a mixed arcade+Genesis fit normalizes
each file with its own scaling constants (§4.3). `fields()`/`global_value()`
are the row-accessor layer both formats go through; everything below them
consumes accessors, never raw `block1`/`gate` dict shapes directly.
"""

from __future__ import annotations

import json
import warnings
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

from . import profile as _profile

_PROF = _profile.get()
_CAL = _PROF.calibration

# ── calibration constants (SPEC §1) ─────────────────────────────────────────
GROUND_Y = _CAL["GROUND_Y"]
X_SCALE = _CAL["X_SCALE"]
Y_SCALE = _CAL["Y_SCALE"]
TIMER_SCALE = _CAL["TIMER_SCALE"]
ANIM_SCALE = _CAL["ANIM_SCALE"]
CORNER_PX = _CAL["CORNER_PX"]
HEALTH_MAX = _CAL["HEALTH_MAX"]
P = _CAL["P"]              # decision period, frames (§4)
K = _CAL["K"]              # stacked decision-step snapshots (§4)
STALE = _CAL["STALE"]      # opponent observation delay, frames (§4)
SCREEN_W = _CAL["SCREEN_W"]

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
HITSTUN_RECENT_FRAMES = _CAL["HITSTUN_RECENT_FRAMES"]

# RETRO mask bits
BIT_B, BIT_Y, BIT_SELECT, BIT_START = 0, 1, 2, 3
BIT_UP, BIT_DOWN, BIT_LEFT, BIT_RIGHT = 4, 5, 6, 7
BIT_A = 8
ATTACK_BITS = (BIT_B, BIT_A, BIT_Y)  # Light, Medium, Heavy (§3c)

# Class lists (dataset/model head sizes): family.json's vocabulary, shared by
# every port of this game family (docs/game-profiles.md rule 3 -- nothing may
# hardcode 9 moves / 6 attacks; everything sizes from these lists' lengths).
MOVE_CLASSES = list(_PROF.move_classes)
ATTACK_CLASSES = list(_PROF.attack_classes)

# scalar feature names, in canonical vector order (§1a minus the categorical
# columns). This is the FULL order RECORDER_V3.md §4.2 filters down per
# recording -- order never changes, entries only drop out (`scalar_features_
# for` below). v2 files (the only format today) always resolve to this full
# list, which is what keeps G1's golden refit byte-identical.
SCALAR_FEATURES = [
    "dist_x", "dy", "me_airborne", "me_height", "me_fwd_hold", "me_back_hold",
    "me_anim", "me_timer", "opp_airborne", "opp_height", "opp_anim",
    "opp_timer", "facing_sign", "me_health", "opp_health", "health_lead",
    "me_meter", "opp_meter", "me_hitstun", "opp_hitstun", "me_corner",
]

# Fighter fields the v2 recorder always emitted (src/record.rs's hardcoded
# Fighter struct) -- v2 has no "unmapped field" concept, so this is the fixed
# availability set for every v2 file, independent of whatever the CURRENTLY
# loaded profile's fighter_fields happen to say (that could drift after a
# profile edit; v2 files must not care -- see the module docstring).
_V2_FIGHTER_FIELDS = frozenset({
    "x", "y", "facing", "weapon", "health", "health2", "meter", "meter_max",
    "char_id", "wins", "timer", "anim", "action", "opp_right_hold", "opp_left_hold",
})

# v2's gate object always carried these two under their v2 names -- hitstun
# is unconditionally available for v2 (see _view_for).
_V2_RECORDED_GLOBALS = frozenset({
    "round_over", "abort", "match_end", "timer_bcd", "demo_flag",
    "combo_on_b1", "combo_on_b2", "credits",
})

# RECORDER_V3.md item 2: "hitstun via the hitstun_sources profile key with
# fallback to the current hardcoded behavior when absent (asurabld back-
# compat before A1's profile edit lands)". Until asurabld.profile.json grows
# its §2.4 `hitstun_sources` key, this is the map dataset.py used to hardcode
# implicitly (combo_on_b1/b2, block1/block2). Once a profile declares its own
# hitstun_sources, that value is used instead (see _view_for).
_LEGACY_HITSTUN_SOURCES = {"block1": "combo_on_b1", "block2": "combo_on_b2"}

# Fields that MUST be mapped for any fit regardless of which SCALAR_FEATURES
# end up available (§4.2's "required fields per fit" bullet) -- matchup
# machinery (char_id) and demo/gate semantics (x, health) need them even when
# their calibration constants are missing and the derived feature is dropped.
_REQUIRED_FIELDS = ("x", "char_id", "health")

_WARNED_MISSING_SIDECAR: set = set()


@dataclass
class _RecordingView:
    """Everything one file's decisions need to know about its own port,
    resolved ONCE per file (§4.1/§4.3) -- v2's fixed shape, or v3's own
    `.meta.json` sidecar (never the process-loaded profile, except as the
    documented v3-sidecar-missing/legacy-hitstun fallback)."""

    version: str                  # "v2" or "v3"
    field_names: frozenset        # mapped fighter_fields names
    calibration: dict             # this recording's own scaling constants
    hitstun_map: dict | None      # block name -> global name, or None (unavailable)
    port: str


def _detect_version(path: Path) -> str:
    """First-parseable-row format detection (§1.1). `"v":3` -> v3; a bare
    `block1` (no `v`) -> v2; neither -> the v1-rejection error."""
    with open(path) as f:
        for line in f:
            try:
                r = json.loads(line)
            except json.JSONDecodeError:
                continue
            if r.get("v") == 3:
                return "v3"
            if "block1" in r:
                return "v2"
            raise SystemExit(
                f"{path.name}: not jsonl-v2/v3 (no \"v\":3, no block1) — v1 "
                "recordings are not supported; re-record with a current recorder"
            )
    raise SystemExit(f"{path.name}: no parseable rows")


def fields(row: dict, block: str) -> dict:
    """Named-field accessor for a fighter block (§4.1). v2's fixed Fighter
    struct and v3's profile-shaped map are both already {name: value} dicts
    on the row -- the only real difference is a v3 row simply OMITS names
    its profile doesn't map (§1.2 rule 1), never zero-fills them."""
    return row.get(block) or {}


_V3_TO_V2_GLOBAL_NAME = {"round_timer": "timer_bcd", "char_select": "char_sel"}


def global_value(row: dict, name: str):
    """Named global accessor (§4.1). v3 rows key `globals` by the profile's
    own names; v2 rows used ad-hoc names in `gate` for the same two renamed
    here (the exact renames G2's transcoder reverses) -- everything else
    (combo_on_b1/b2, round_over, ...) already shares its v2 and v3 name."""
    if "globals" in row:
        return row["globals"].get(name)
    return row.get("gate", {}).get(_V3_TO_V2_GLOBAL_NAME.get(name, name))


def _load_meta_sidecar(path: Path) -> dict | None:
    """v3's `.meta.json` provenance sidecar (§1.3), or None if missing/
    unreadable -- warns once per file (§4.3: v3 files without a sidecar fall
    back to the loaded profile, same as v2)."""
    meta_path = Path(str(path).removesuffix(".jsonl") + ".meta.json")
    try:
        return json.loads(meta_path.read_text())
    except (OSError, json.JSONDecodeError):
        if str(path) not in _WARNED_MISSING_SIDECAR:
            _WARNED_MISSING_SIDECAR.add(str(path))
            warnings.warn(
                f"{path.name}: v3 recording has no readable .meta.json sidecar -- "
                "falling back to the loaded profile for field/calibration availability"
            )
        return None


def _view_for(path: Path, version: str, prof) -> _RecordingView:
    """Resolve one file's `_RecordingView` (§4.1/§4.3). v2: the fixed v2
    shape, always fully available, using the loaded profile's calibration
    (identical to what the v2 recorder used, so G1 holds). v3 with a sidecar:
    self-describing, honest degradation from that sidecar's own fighter_
    fields/calibration/recorded-globals. v3 without a sidecar: the legacy
    fallback (item 2) -- trust the loaded profile fully, exactly like v2."""
    hitstun_default = prof.hitstun_sources or _LEGACY_HITSTUN_SOURCES

    if version == "v2":
        return _RecordingView(
            version="v2",
            field_names=_V2_FIGHTER_FIELDS,
            calibration=prof.calibration,
            hitstun_map=(
                hitstun_default
                if all(g in _V2_RECORDED_GLOBALS for g in hitstun_default.values())
                else None
            ),
            port=prof.port,
        )

    meta = _load_meta_sidecar(path)
    if meta is None:
        return _RecordingView(
            version="v3",
            field_names=frozenset(prof.fighter_fields),
            calibration=prof.calibration,
            hitstun_map=hitstun_default,
            port=prof.port,
        )

    recorded = {g["name"] for g in meta.get("globals_recorded", [])}
    hitstun_map = (
        hitstun_default
        if all(g in recorded for g in hitstun_default.values())
        else None
    )
    return _RecordingView(
        version="v3",
        field_names=frozenset(f["name"] for f in meta.get("fighter_fields", [])),
        calibration=dict(meta.get("calibration", {})),
        hitstun_map=hitstun_map,
        port=meta.get("port", prof.port),
    )


def scalar_features_for(view: _RecordingView) -> list[str]:
    """§4.2's availability table: SCALAR_FEATURES filtered to what this
    recording actually supports, order preserved. Raises (SystemExit, this
    project's abort convention -- see build()) when a required field/
    calibration constant is missing outright."""
    for name in _REQUIRED_FIELDS:
        if name not in view.field_names:
            raise SystemExit(
                f"profile maps no {name!r} fighter field — cannot build features"
            )
    if "X_SCALE" not in view.calibration:
        raise SystemExit(
            "profile calibration missing 'X_SCALE' — cannot build features"
        )

    fn, cal = view.field_names, view.calibration
    has = lambda *names: all(n in fn for n in names)
    ck = lambda *keys: all(k in cal for k in keys)

    xy_ok = has("y") and ck("GROUND_Y", "Y_SCALE")
    anim_ok = has("anim") and ck("ANIM_SCALE")
    timer_ok = has("timer") and ck("TIMER_SCALE")
    health_ok = has("health") and ck("HEALTH_MAX")
    meter_ok = has("meter", "meter_max")
    hitstun_ok = view.hitstun_map is not None
    corner_ok = has("x") and ck("CORNER_PX", "SCREEN_W")

    avail = {
        "dist_x": True,
        "dy": xy_ok, "me_airborne": xy_ok, "me_height": xy_ok,
        "me_fwd_hold": True, "me_back_hold": True,
        "me_anim": anim_ok, "me_timer": timer_ok,
        "opp_airborne": xy_ok, "opp_height": xy_ok,
        "opp_anim": anim_ok, "opp_timer": timer_ok,
        "facing_sign": True,
        "me_health": health_ok, "opp_health": health_ok, "health_lead": health_ok,
        "me_meter": meter_ok, "opp_meter": meter_ok,
        "me_hitstun": hitstun_ok, "opp_hitstun": hitstun_ok,
        "me_corner": corner_ok,
    }
    return [f for f in SCALAR_FEATURES if avail[f]]


@dataclass
class Decision:
    """One 8 Hz training example (pre-stacking)."""

    scalars: np.ndarray          # len(this file's scalar_features_for(view))
    me_action: int | None
    opp_action: int | None
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
    """Situation bucket for eval/coverage (§7.3/§7.4). `.get()` rather than
    direct indexing: a sparse recording (RECORDER_V3.md §4.2) may not have
    computed me_hitstun/opp_hitstun/me_airborne/me_corner at all -- missing
    means "can't tell", which degrades to the bucket falling through, never
    a KeyError (§4.2: "_bucket() loses offense/defense buckets" when
    hitstun is unavailable; the same honesty applies to air/corner)."""
    if scal.get("me_hitstun"):
        return "defense"
    if scal.get("opp_hitstun"):
        return "offense"
    if scal.get("me_airborne"):
        return "air"
    if scal.get("me_corner"):
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
    vals = [global_value(r, key) for r in rows]
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
    """Yield (round_key, rows) for controllable, non-demo rounds. Version
    detection (§1.1) only guards the v1-rejection error here -- row access
    below goes through `fields()`/`global_value()`, which self-dispatch per
    row (v2's `gate` vs v3's `globals`), so no version threading is needed
    for the row-grouping walk itself."""
    _detect_version(path)
    rounds: dict = {}
    with open(path) as f:
        for line in f:
            try:
                r = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not r.get("controllable") or r.get("p1_block") is None:
                continue
            rounds.setdefault(r["round_id"], []).append(r)
    for rid, rows in sorted(rounds.items()):
        if sum(x["p1_input"] for x in rows) == 0:
            continue  # attract demo (§5)
        if len(rows) < P * (K + 1):
            continue  # too short to stack
        yield (str(path.name), rid), rows


def _decisions_for_round(round_key: tuple, rows: list[dict],
                          view: _RecordingView | None = None,
                          feats: list[str] | None = None):
    """`view`/`feats` default to "everything available" (asurabld's full v2
    shape) when omitted -- this keeps the 2-arg call signature test_runtime.py
    depends on (its streaming-vs-batch parity test) working unchanged; real
    callers (`build`/`load_decisions`) always pass both, resolved per file."""
    if view is None:
        view = _RecordingView(
            version="v2", field_names=_V2_FIGHTER_FIELDS,
            calibration=_PROF.calibration, hitstun_map=_LEGACY_HITSTUN_SOURCES,
            port=_PROF.port,
        )
    if feats is None:
        feats = scalar_features_for(view)
    fs = set(feats)
    prof = _profile.get()

    p1b = "block1" if rows[0]["p1_block"] == 1 else "block2"
    oppb = "block2" if p1b == "block1" else "block1"
    has_facing = "facing" in view.field_names
    have_action = "action" in view.field_names
    have_xy = "dy" in fs
    have_anim = "me_anim" in fs
    have_timer = "me_timer" in fs
    have_health = "me_health" in fs
    have_meter = "me_meter" in fs
    have_hitstun = "me_hitstun" in fs
    have_corner = "me_corner" in fs
    cal = view.calibration
    X_SCALE_ = cal["X_SCALE"]

    # active-hitstun masks (task 2) -- "recently changed", not "nonzero"
    if have_hitstun:
        hmap = view.hitstun_map
        b1_active = _recent_change_mask(rows, hmap["block1"], HITSTUN_RECENT_FRAMES)
        b2_active = _recent_change_mask(rows, hmap["block2"], HITSTUN_RECENT_FRAMES)
        me_active = b1_active if p1b == "block1" else b2_active
        opp_active = b2_active if p1b == "block1" else b1_active

    out = []
    for i in range(P * 1, len(rows), P):
        row = rows[i]
        me = fields(row, p1b)
        opp = fields(rows[max(0, i - STALE)], oppb)  # stale opponent (§4)
        if has_facing:
            s = 1 if me["facing"] == 1 else -1
        else:
            # §4.2 facing fallback: s = sign(opp.x - me.x). With this s,
            # s*(opp.x-me.x) == |opp.x-me.x|, so dist_x below is |Δx| exactly
            # as the contract note says, and fwd/back holds become
            # position-relative for free.
            s = 1 if (opp["x"] - me["x"]) >= 0 else -1
        # me-holds from own mask history (§1a #4/#5)
        hist = [rows[j]["p1_input"] for j in range(max(0, i - P), i)]
        fwd_bit = BIT_RIGHT if s > 0 else BIT_LEFT
        back_bit = BIT_LEFT if s > 0 else BIT_RIGHT
        n = max(1, len(hist))
        scal = {
            "dist_x": s * (opp["x"] - me["x"]) / X_SCALE_,
            "me_fwd_hold": sum(m >> fwd_bit & 1 for m in hist) / n,
            "me_back_hold": sum(m >> back_bit & 1 for m in hist) / n,
            "facing_sign": float(s),
        }
        if have_xy:
            GROUND_Y_, Y_SCALE_ = cal["GROUND_Y"], cal["Y_SCALE"]
            scal["dy"] = (opp["y"] - me["y"]) / Y_SCALE_
            scal["me_airborne"] = 1.0 if GROUND_Y_ - me["y"] > 4 else 0.0
            scal["me_height"] = max(0, GROUND_Y_ - me["y"]) / Y_SCALE_
            scal["opp_airborne"] = 1.0 if GROUND_Y_ - opp["y"] > 4 else 0.0
            scal["opp_height"] = max(0, GROUND_Y_ - opp["y"]) / Y_SCALE_
        if have_anim:
            ANIM_SCALE_ = cal["ANIM_SCALE"]
            scal["me_anim"] = me["anim"] / ANIM_SCALE_
            scal["opp_anim"] = opp["anim"] / ANIM_SCALE_
        if have_timer:
            TIMER_SCALE_ = cal["TIMER_SCALE"]
            scal["me_timer"] = me["timer"] / TIMER_SCALE_
            scal["opp_timer"] = opp["timer"] / TIMER_SCALE_
        if have_health:
            HEALTH_MAX_ = cal["HEALTH_MAX"]
            scal["me_health"] = me["health"] / HEALTH_MAX_
            scal["opp_health"] = opp["health"] / HEALTH_MAX_
            scal["health_lead"] = (me["health"] - opp["health"]) / HEALTH_MAX_
        if have_meter:
            scal["me_meter"] = me["meter"] / max(1, me["meter_max"])
            scal["opp_meter"] = opp["meter"] / max(1, opp["meter_max"])
        if have_hitstun:
            # me_hitstun: self-feature, stays current (§4 -- "you know your
            # own hands"); opp_hitstun: opponent-sourced, so it's read STALE
            # like the rest of the opp_* block (§4 humanness rule, §1a #21).
            scal["me_hitstun"] = 1.0 if me_active[i] else 0.0
            scal["opp_hitstun"] = 1.0 if opp_active[max(0, i - STALE)] else 0.0
        if have_corner:
            CORNER_PX_, SCREEN_W_ = cal["CORNER_PX"], cal["SCREEN_W"]
            scal["me_corner"] = (
                1.0 if me["x"] <= CORNER_PX_ or me["x"] >= SCREEN_W_ - CORNER_PX_
                else 0.0
            )
        # label = what the user held over the NEXT decision window (§4)
        masks = [rows[j]["p1_input"] for j in range(i, min(len(rows), i + P))]
        move, attack = _window_label(masks, s)
        out.append(
            Decision(
                scalars=np.array([scal[k] for k in feats], dtype=np.float32),
                me_action=me.get("action") if have_action else None,
                opp_action=opp.get("action") if have_action else None,
                opp_char=prof.canon_char_id(opp["char_id"]),
                me_char=prof.canon_char_id(me["char_id"]),
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


def _decisions_from_file(path: Path) -> tuple[list[Decision], list[str], str]:
    """One file's (decisions, its resolved feature-name list, its port) --
    the per-file unit both load_decisions() and build() are built from."""
    version = _detect_version(path)
    view = _view_for(path, version, _profile.get())
    feats = scalar_features_for(view)
    decisions: list[Decision] = []
    for round_key, rows in _rounds(path):
        decisions.extend(_segment(_decisions_for_round(round_key, rows, view, feats)))
    return decisions, feats, view.port


def _load_decisions_with_meta(paths: list[Path]):
    decisions: list[Decision] = []
    feature_sets: dict = {}
    ports: list[str] = []
    for p in paths:
        decs, feats, port = _decisions_from_file(p)
        feature_sets[p] = feats
        ports.append(port)
        decisions.extend(decs)
    return decisions, feature_sets, ports


def _check_feature_parity(feature_sets: dict) -> None:
    """§4.3's mixed-port fit rule: every file in one fit must resolve to the
    SAME feature-name list, or abort naming the difference."""
    uniq = {tuple(v) for v in feature_sets.values()}
    if len(uniq) <= 1:
        return
    items = list(feature_sets.items())
    base_path, base_feats = items[0]
    diffs = []
    for p, feats in items[1:]:
        if feats == base_feats:
            continue
        lacks = [f for f in base_feats if f not in feats]
        extra = [f for f in feats if f not in base_feats]
        bits = []
        if lacks:
            bits.append(f"lacks {lacks}")
        if extra:
            bits.append(f"has extra {extra}")
        diffs.append(f"{p.name} {' and '.join(bits)} (vs {base_path.name})")
    raise SystemExit("feature sets differ: " + "; ".join(diffs))


def load_decisions(paths: list[Path], char_filter: int | None = None,
                   opp_filter: int | None = None) -> list[Decision]:
    """Recordings -> filtered Decision list (pre-stacking). The demo filter
    (zero p1_input rounds) is applied inside _rounds; matchup filters here.
    Shared by build() and the coverage command so both count identically.
    (No cross-file feature-parity check here -- coverage only ever reads
    d.me_char/d.opp_char, so per-file feature-vector length doesn't matter.)"""
    decisions, _feature_sets, _ports = _load_decisions_with_meta(paths)
    if char_filter is not None:
        decisions = [d for d in decisions if d.me_char == char_filter]
    if opp_filter is not None:
        decisions = [d for d in decisions if d.opp_char == opp_filter]
    return decisions


def build(paths: list[Path], char_filter: int | None = None,
          opp_filter: int | None = None):
    """Load recordings -> stacked dataset arrays.

    Returns dict with X (N, K*len(feature_names)), y_move, y_attack, buckets,
    round keys, the per-decision categorical columns, the actually-resolved
    `feature_names` (§4.2 -- SCALAR_FEATURES filtered per-recording), and the
    unique `ports` seen (§4.3 -- "mixed" meta when more than one).
    """
    decisions, feature_sets, ports = _load_decisions_with_meta(paths)
    _check_feature_parity(feature_sets)
    feature_names = next(iter(feature_sets.values()))
    if char_filter is not None:
        decisions = [d for d in decisions if d.me_char == char_filter]
    if opp_filter is not None:
        decisions = [d for d in decisions if d.opp_char == opp_filter]
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
        "feature_names": feature_names,
        "ports": sorted(set(ports)),
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
    # Only the per-decision columns are index-aligned with `keep` -- build()'s
    # other keys (feature_names, ports) describe the WHOLE dataset, not one
    # decision, and pass through unchanged.
    per_decision = {"X", "y_move", "y_attack", "buckets", "rounds"}
    out = dict(data)
    for k in per_decision & data.keys():
        v = data[k]
        out[k] = v[keep] if isinstance(v, np.ndarray) else [v[i] for i in keep]
    return out
