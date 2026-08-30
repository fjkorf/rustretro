"""Deploy-time runtime helpers for `shadow/play.py` (SPEC v2 §1a/§2/§3c/§4).

This module does NOT modify `dataset.py`, `knn.py`, or `__main__.py` — it only
imports their public constants/functions and adds the small amount of new
logic a streaming (online, one-tick-at-a-time) deploy loop needs that the
offline batch dataset builder does not: memory-blob parsing, an incremental
feature stacker, hitstun edge-tracking at decision-tick granularity, the
intent -> RETRO bits inverse map (§3c), and a live/meta calibration
drift check.

Two deliberate approximations vs the exact §1a/§4 spec (both documented at
the point of use, both required because a deploy loop only ever *polls* the
game at the ~8 Hz decision rate — it cannot see the 60 Hz frames between
polls that the offline dataset builder has full access to):

  1. **Opponent staleness.** The spec asks for opponent-sourced features read
     2-4 frames old (`STALE` in `dataset.py`). At deploy we only have one
     opponent observation per decision tick (~`P`=8 frames apart), so the
     nearest equivalent is "the opponent snapshot from the PREVIOUS decision
     tick" — i.e. ~P frames stale rather than ~STALE frames stale. This is
     stricter (more staleness) than the spec's minimum, which is within the
     spirit of the humanness rule (§4) even if not bit-exact. A future
     refinement could poll opponent-only fields at a higher rate than the
     full decision cadence to hit the exact 2-4 frame target.
  2. **Hitstun recent-change window.** `HITSTUN_RECENT_FRAMES` (dataset.py)
     is a frame-count threshold; at deploy we only observe the combo-counter
     byte once per tick. `WINDOW_TICKS = HITSTUN_RECENT_FRAMES // P` converts
     it to the nearest tick-granularity threshold. Events that happen to land
     exactly on decision-tick boundaries reproduce the frame-level answer
     exactly (see the parity test in test_runtime.py); events between polls
     are inherently unobservable at decision rate, same as any other
     opponent-sourced signal here.

Both approximations only affect *observation timing*, never the formulas
(scales, thresholds, the side-agnostic transform) — those are reused/mirrored
exactly from `dataset.py` and pinned by the parity test.

A third thing this module has to handle that `dataset.py` does not: a fighter
field can be POINTER-RESOLVED (docs/frames.md §2.5 -- MK2 arcade's world
`x`/`y`), meaning it may be ABSENT from `me`/`opp` on any given tick (never
zero-filled) when its pointer fails to dereference that frame. `dataset.py`'s
offline fitter answers this by dropping and counting the affected DECISION
(see its module docstring); a live loop is asked for a fresh action every
tick and has no "drop a decision" option. `build_scalars`' `pointer_fields`
guard and `RoundBuffers.compute_scalars`/`PointerStaleness` below are the
streaming answer -- see the design note above `PointerStaleness` for the
reasoning and `.meta.json`'s `pointer_resolved_fields` (`pointer_fields_from_meta`)
for where the declared field set comes from. Empty for every model whose
recordings declare no such fields (all of them today), which is the
zero-added-cost fast path, mirroring `dataset.py`'s own framing of the same
guarantee.
"""

from __future__ import annotations

import math
import warnings
from collections import deque
from dataclasses import dataclass, field

import numpy as np

from . import asurabld as gm
from . import dataset
from . import profile as _profile
from .__main__ import CALIBRATION_KEYS
from .dataset import (
    ANIM_SCALE,
    BIT_A,
    BIT_B,
    BIT_DOWN,
    BIT_LEFT,
    BIT_RIGHT,
    BIT_UP,
    BIT_Y,
    CORNER_PX,
    GROUND_Y,
    HEALTH_MAX,
    HITSTUN_RECENT_FRAMES,
    K,
    P,
    SCALAR_FEATURES,
    SCREEN_W,
    TIMER_SCALE,
    X_SCALE,
    Y_SCALE,
)

# ── RETRO_DEVICE_ID_JOYPAD button-name order (SPEC §3c; matches
# src/mcp/server.rs::joypad_button_index exactly) ───────────────────────────
RETRO_BUTTON_NAMES = [
    "b", "y", "select", "start", "up", "down", "left", "right",
    "a", "x", "l", "r",
]

# ── memory read plan (addresses/offsets from the game profile --
# library/asurabld/{family,asurabld.profile}.json via shadow_train.profile;
# asurabld.md remains the literate evidence document for how each value was
# verified) ──────────────────────────────────────────────────────────────
# Batched so one tick needs 5 read_memory calls total (<6, per the harness
# requirement): two big per-block reads (each block's own struct span also
# happens to contain the OTHER block's "combo landing on me" counter — see
# asurabld.md's note that the combo counters are cross-block addresses that
# fall inside the neighboring block's stride-byte span), one combined
# match_end+abort read (they're close enough together to share one call),
# and two small reads (round_over, round timer BCD).
BLOCK1_ADDR = gm.BLOCK1
BLOCK2_ADDR = gm.BLOCK2
COMBO_ON_B2_ADDR = gm.COMBO_ON_B2  # block1's combo landing on block2 -- inside block1's span
COMBO_ON_B1_ADDR = gm.COMBO_ON_B1  # block2's combo landing on block1 -- inside block2's span
COMBO_ON_B2_OFFSET = COMBO_ON_B2_ADDR - BLOCK1_ADDR
COMBO_ON_B1_OFFSET = COMBO_ON_B1_ADDR - BLOCK2_ADDR

# fighter struct layout: (name, offset, size in bytes); all big-endian
# (68k byte order, per record.rs's u16be/u8g helpers). Offsets come straight
# from the profile's `fighter_fields` (this is the same field subset the
# read plan has always used -- notably NOT `wins`, which the profile also
# carries but which decision features never read).
_FIGHTER_FIELD_NAMES = [
    "timer", "anim", "action", "x", "y", "facing", "weapon",
    "health", "health2", "meter", "meter_max", "char_id",
]
FIGHTER_LAYOUT = [
    (name, *_profile.get().field_off(name)) for name in _FIGHTER_FIELD_NAMES
]
_FIGHTER_END = max(off + size for _, off, size in FIGHTER_LAYOUT)

BLOCK1_LEN = max(_FIGHTER_END, COMBO_ON_B2_OFFSET + 1)
BLOCK2_LEN = max(_FIGHTER_END, COMBO_ON_B1_OFFSET + 1)

MATCH_END_ADDR = gm.MATCH_END
ABORT_ADDR = gm.ABORT
MATCH_END_ABORT_OFFSET = ABORT_ADDR - MATCH_END_ADDR
MATCH_END_ABORT_LEN = MATCH_END_ABORT_OFFSET + 2  # abort is a u16

ROUND_OVER_ADDR = gm.ROUND_OVER
ROUND_TIMER_ADDR = gm.ROUND_TIMER  # BCD seconds
CHAR_SELECT_ADDR = gm.CHAR_SELECT  # select-screen countdown (gate v3)

CREDITS_ADDR = gm.CREDITS  # startup-only (not part of the per-tick read plan)

# (name, addr, len) -- the exact 5 reads issued each decision tick.
READ_PLAN = [
    ("block1", BLOCK1_ADDR, BLOCK1_LEN),
    ("block2", BLOCK2_ADDR, BLOCK2_LEN),
    ("match_end_abort", MATCH_END_ADDR, MATCH_END_ABORT_LEN),
    ("round_over", ROUND_OVER_ADDR, 2),
    # One read spans $400006..$40000A: char-select countdown at [0] (gate v3
    # -- the v2 gate is TRUE on the select screen), round clock at [4].
    ("clock", CHAR_SELECT_ADDR, 5),
]


def _be(blob: bytes, offset: int, size: int) -> int:
    return int.from_bytes(blob[offset:offset + size], "big")


def parse_fighter(blob: bytes) -> dict:
    """Decode one fighter block's blob (>= BLOCK*_LEN bytes) per FIGHTER_LAYOUT."""
    return {name: _be(blob, off, size) for name, off, size in FIGHTER_LAYOUT}


@dataclass
class TickSnapshot:
    block1: dict
    block2: dict
    combo_on_b1: int
    combo_on_b2: int
    round_over: int
    abort: int
    match_end: int
    timer_bcd: int
    char_sel: int


def parse_tick(blobs: dict) -> TickSnapshot:
    """blobs: {name: bytes} for every entry in READ_PLAN."""
    b1_blob = blobs["block1"]
    b2_blob = blobs["block2"]
    me_blob = blobs["match_end_abort"]
    return TickSnapshot(
        block1=parse_fighter(b1_blob),
        block2=parse_fighter(b2_blob),
        combo_on_b2=b1_blob[COMBO_ON_B2_OFFSET],
        combo_on_b1=b2_blob[COMBO_ON_B1_OFFSET],
        round_over=_be(blobs["round_over"], 0, 2),
        abort=_be(me_blob, MATCH_END_ABORT_OFFSET, 2),
        match_end=_be(me_blob, 0, 2),
        timer_bcd=blobs["clock"][4],
        char_sel=blobs["clock"][0],
    )


# ── gate (mirrors src/record.rs's `controllable` formula exactly) ──────────
def timer_bcd_valid(t: int) -> bool:
    return t != 0 and (t >> 4) <= 9 and (t & 0xF) <= 9


def is_controllable(snap: TickSnapshot) -> bool:
    def healthy(f: dict) -> bool:
        return 1 <= f["health"] <= HEALTH_MAX

    return (
        snap.round_over == 0
        and snap.abort == 0
        and snap.match_end == 0
        and healthy(snap.block1)
        and healthy(snap.block2)
        and timer_bcd_valid(snap.timer_bcd)
        and snap.char_sel == 0  # gate v3: NOT on the char-select screen
    )


# ── per-round anchor (§ requirement 1: bot = the block with the LARGER X,
# the mirror of the recorder's p1_block = smaller X) ────────────────────────
def resolve_me_block(x1: int, x2: int) -> str:
    return "block1" if x1 > x2 else "block2"


def other_block(name: str) -> str:
    return "block2" if name == "block1" else "block1"


# ── hitstun recent-change tracker, tick-granular (see module docstring #2) ──
WINDOW_TICKS = max(1, HITSTUN_RECENT_FRAMES // P)


class HitstunTracker:
    """Streaming equivalent of dataset._recent_change_mask, at tick (not
    frame) granularity -- see the module docstring for why."""

    def __init__(self) -> None:
        self._prev: int | None = None
        self._last_change_tick: int | None = None

    def reset(self) -> None:
        self._prev = None
        self._last_change_tick = None

    def update(self, tick: int, value: int) -> bool:
        if self._prev is not None and value != self._prev:
            self._last_change_tick = tick
        self._prev = value
        return (
            value != 0
            and self._last_change_tick is not None
            and tick - self._last_change_tick <= WINDOW_TICKS
        )


# ── me_fwd_hold / me_back_hold from the bot's own EMITTED mask (§1a #4/#5;
# NOT from re-feeding the intent class -- the mask is the actual injected
# input, the deploy-side analog of the recorder's p1_input) ────────────────
def hold_fractions(last_emitted_mask: int, s: int) -> tuple[float, float]:
    fwd_bit = BIT_RIGHT if s > 0 else BIT_LEFT
    back_bit = BIT_LEFT if s > 0 else BIT_RIGHT
    fwd = float(last_emitted_mask >> fwd_bit & 1)
    back = float(last_emitted_mask >> back_bit & 1)
    return fwd, back


# ── the §1a scalar vector, reimplemented for streaming use (kept honest by
# the parity test in test_runtime.py, which checks this against
# dataset._decisions_for_round's per-decision formulas on synthetic rows) ──
def build_scalars(
    me: dict,
    opp: dict,
    s: int,
    fwd_hold: float,
    back_hold: float,
    me_hitstun: bool,
    opp_hitstun: bool,
    pointer_fields: frozenset[str] = frozenset(),
) -> dict | None:
    """`pointer_fields` (RECORDER_V3.md §1.2 rule 1 / docs/frames.md §2.5 --
    the live-play analog of dataset.py's `pointer_resolved_fields`): names of
    fighter fields that may be intermittently ABSENT from `me`/`opp` (never
    zero-filled) because they are resolved through a live pointer that can
    fail to dereference on any given tick. When any of them is missing from
    either dict, this returns None instead of raising a KeyError or
    computing on a fabricated value -- see the module docstring and the
    design note above PointerStaleness for what a caller should do with
    that (hold the previous action for this tick; never invent the missing
    coordinate). Empty (the default -- every model that declares no
    pointer-resolved fields) short-circuits the `and` below before either
    generator runs, so this costs nothing on the common path and the
    function always returns a dict, exactly as before this guard existed."""
    if pointer_fields and (
        any(f not in me for f in pointer_fields)
        or any(f not in opp for f in pointer_fields)
    ):
        return None
    return {
        "dist_x": s * (opp["x"] - me["x"]) / X_SCALE,
        "dy": (opp["y"] - me["y"]) / Y_SCALE,
        "me_airborne": 1.0 if GROUND_Y - me["y"] > 4 else 0.0,
        "me_height": max(0, GROUND_Y - me["y"]) / Y_SCALE,
        "me_fwd_hold": fwd_hold,
        "me_back_hold": back_hold,
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
        "me_hitstun": 1.0 if me_hitstun else 0.0,
        "opp_hitstun": 1.0 if opp_hitstun else 0.0,
        "me_corner": 1.0
        if me["x"] <= CORNER_PX or me["x"] >= SCREEN_W - CORNER_PX
        else 0.0,
    }


def scalars_to_vector(scal: dict) -> np.ndarray:
    return np.array([scal[k] for k in SCALAR_FEATURES], dtype=np.float32)


# ── K-step decision stacking (§4) -- same granularity as dataset.build(),
# no approximation needed here since our ticks ARE decisions ───────────────
class FeatureStacker:
    def __init__(self, k: int = K) -> None:
        self.k = k
        self._buf: deque = deque(maxlen=k)

    def reset(self) -> None:
        self._buf.clear()

    def push(self, scal: dict) -> None:
        self._buf.append(scalars_to_vector(scal))

    def ready(self) -> bool:
        return len(self._buf) == self.k

    def vector(self) -> np.ndarray:
        # oldest -> newest, matching dataset.build()'s
        # np.concatenate([ds[j].scalars for j in range(i-K+1, i+1)]).
        return np.concatenate(list(self._buf))


# ── intent (move_class, attack_class) -> RETRO 12-bit mask (§3c) ───────────
# Move class ids (dataset.MOVE_CLASSES order):
#   0 Neutral 1 Forward 2 Back 3 Up 4 Down 5 UpForward 6 UpBack
#   7 DownForward 8 DownBack
_UP_MOVES = {3, 5, 6}
_DOWN_MOVES = {4, 7, 8}
_FWD_MOVES = {1, 5, 7}
_BACK_MOVES = {2, 6, 8}


def intent_to_mask(move_class: int, attack_class: int, s: int) -> int:
    m = 0
    if move_class in _UP_MOVES:
        m |= 1 << BIT_UP
    if move_class in _DOWN_MOVES:
        m |= 1 << BIT_DOWN
    fwd_bit = BIT_RIGHT if s > 0 else BIT_LEFT
    back_bit = BIT_LEFT if s > 0 else BIT_RIGHT
    if move_class in _FWD_MOVES:
        m |= 1 << fwd_bit
    if move_class in _BACK_MOVES:
        m |= 1 << back_bit

    # attack_class ids (dataset.ATTACK_CLASSES order):
    #   0 None 1 Light 2 Medium 3 Heavy 4 Launcher 5 Toss
    if attack_class == 1:
        m |= 1 << BIT_B
    elif attack_class == 2:
        m |= 1 << BIT_A
    elif attack_class == 3:
        m |= 1 << BIT_Y
    elif attack_class == 4:
        m |= (1 << BIT_B) | (1 << BIT_A)
    elif attack_class == 5:
        m |= (1 << BIT_B) | (1 << BIT_A) | (1 << BIT_Y)
    return m


def mask_to_button_names(mask: int) -> list[str]:
    return [RETRO_BUTTON_NAMES[i] for i in range(12) if mask >> i & 1]


def intent_to_button_names(move_class: int, attack_class: int, s: int) -> list[str]:
    return mask_to_button_names(intent_to_mask(move_class, attack_class, s))


def frames_per_decision(hz: float) -> int:
    return max(1, math.ceil(60.0 / hz))


# ── model/dataset calibration drift guard (requirement: "assert they match
# the dataset module's values (drift warning if not)") ─────────────────────
def check_calibration_drift(meta: dict) -> list[str]:
    """Compare a fitted model's meta.json calibration block against the
    currently-imported dataset.py's live constants. Returns a list of
    human-readable mismatch strings (empty = no drift)."""
    cal = meta.get("calibration", {})
    mismatches = []
    for key in CALIBRATION_KEYS:
        live = getattr(dataset, key, None)
        got = cal.get(key)
        if got != live:
            mismatches.append(f"{key}: meta={got!r} != live dataset.py={live!r}")
    if meta.get("feature_names") != SCALAR_FEATURES:
        mismatches.append(
            f"feature_names: meta={meta.get('feature_names')!r} != "
            f"live dataset.SCALAR_FEATURES={SCALAR_FEATURES!r}"
        )
    return mismatches


def pointer_fields_from_meta(meta: dict) -> frozenset[str]:
    """The live-play read of a model's `.meta.json` `pointer_resolved_fields`
    key (RECORDER_V3.md §1.2 rule 1): names of fighter fields this model's
    recordings declared as possibly-absent-per-frame. Empty for every model
    fit from recordings that name none (all of them today) -- the value to
    hand to `RoundBuffers(pointer_fields=...)`/`build_scalars`' guard for the
    zero-cost fast path."""
    return frozenset(meta.get("pointer_resolved_fields", []))


# ── pointer-resolved-field absence handling for LIVE play (see the module
# docstring; this is the streaming analog of dataset.py's pointer_resolved_
# fields drop-and-count) ────────────────────────────────────────────────────
# The offline fitter's answer to a pointer that failed to resolve on a given
# row is to drop the DECISION and count it. A live loop is asked for a fresh
# action every tick and has no "drop a decision" option -- it must do
# something every tick, or explicitly do nothing. The behaviour chosen here:
#
#   1. On a tick where a declared pointer-resolved field is missing,
#      build_scalars() returns None -- the caller's signal to HOLD the
#      previous action for this one tick (skip stacking, skip a fresh
#      policy.predict, leave last_emitted_mask alone). A one-tick freeze is
#      invisible to a human opponent; inventing the missing coordinate (a
#      zeroed `x` reads as "cornered at the left edge") plays wrong in a way
#      nobody can see, which is worse than doing nothing; raising the
#      KeyError this guard replaces ends the match outright, which is worse
#      still.
#   2. A SUSTAINED run of missing-field ticks is a different situation: not
#      a momentary hole but a broken pointer chain for the rest of the
#      session, and holding position silently for that long is its own kind
#      of lie -- a human watching would call that "the bot stopped playing",
#      not "a dropped frame". PointerStaleness tracks the consecutive count
#      and flips `escalated` the tick it crosses POINTER_STALE_FRAMES worth
#      of ticks; RoundBuffers.compute_scalars fires exactly one
#      `warnings.warn` (visible on stderr regardless of caller) per stale
#      episode when that happens -- loud, but not spammed every tick.
#
# A model that declares no pointer-resolved fields (`pointer_fields` empty,
# every model today) never touches any of this: build_scalars' guard is a
# single `if pointer_fields and ...` truthiness check that short-circuits
# before either generator runs, so the fast path costs nothing new and
# behaves byte-for-byte like before this guard existed.
POINTER_STALE_FRAMES = 300  # ~5s @ 60Hz -- see the design note above


def stale_ticks_for_frames(frames: int, frames_per_tick: int) -> int:
    """Frame-count threshold -> tick-granularity (same conversion pattern as
    WINDOW_TICKS above for HITSTUN_RECENT_FRAMES). A caller that knows its
    real --hz should use this to size PointerStaleness precisely; the
    class's own default assumes the SPEC default ~7.5 Hz decision rate."""
    return max(1, frames // max(1, frames_per_tick))


class PointerStaleness:
    """Tracks consecutive live-play ticks where a declared pointer-resolved
    field failed to resolve (see the design note above). Owned by
    RoundBuffers and deliberately NOT reset by RoundBuffers.reset() -- a
    broken pointer chain is a property of the SESSION, not of any one round,
    so a round boundary must not quietly clear an in-progress escalation."""

    def __init__(self, stale_after_ticks: int | None = None) -> None:
        # Default: POINTER_STALE_FRAMES at the SPEC default ~7.5 Hz decision
        # rate (frames_per_decision(7.5) == 8 -> 300 // 8 == 37). A caller
        # running at a different --hz should pass
        # stale_ticks_for_frames(POINTER_STALE_FRAMES,
        # frames_per_decision(actual_hz)) explicitly instead.
        if stale_after_ticks is None:
            stale_after_ticks = stale_ticks_for_frames(
                POINTER_STALE_FRAMES, frames_per_decision(7.5)
            )
        self.stale_after_ticks = stale_after_ticks
        self._consecutive = 0
        self.total_dropped = 0
        self.escalated = False

    def reset(self) -> None:
        self._consecutive = 0
        self.total_dropped = 0
        self.escalated = False

    @property
    def consecutive(self) -> int:
        return self._consecutive

    def observe(self, resolved: bool) -> bool:
        """Record one tick's outcome (`resolved` = the declared fields were
        all present this tick). Returns True exactly on the tick this call
        newly crosses the escalation threshold (edge-triggered, so a caller
        can warn/log without re-deriving the edge itself); `self.escalated`
        stays True for as long as the run of missing ticks continues, for
        any caller that wants to react further without re-checking counts."""
        if resolved:
            self._consecutive = 0
            self.escalated = False
            return False
        self._consecutive += 1
        self.total_dropped += 1
        crossed_now = self._consecutive == self.stale_after_ticks and not self.escalated
        if self._consecutive >= self.stale_after_ticks:
            self.escalated = True
        return crossed_now


@dataclass
class RoundBuffers:
    """Everything that must reset at each round-start edge (requirement 4).

    `pointer_fields`/`pointer_staleness` are the exception: pointer
    resolution health is a property of the whole SESSION (see
    PointerStaleness's docstring), not of any one round, so `.reset()`
    deliberately leaves them alone -- a round boundary must not quietly
    clear an in-progress escalation. `pointer_fields` is normally set once,
    from `pointer_fields_from_meta(meta)`, when a model is loaded.
    """

    me_block: str | None = None
    stacker: FeatureStacker = field(default_factory=FeatureStacker)
    me_hitstun: HitstunTracker = field(default_factory=HitstunTracker)
    opp_hitstun: HitstunTracker = field(default_factory=HitstunTracker)
    prev_opp: dict | None = None
    prev_opp_combo: int = 0
    last_emitted_mask: int = 0
    tick: int = 0
    pointer_fields: frozenset[str] = field(default_factory=frozenset)
    pointer_staleness: PointerStaleness = field(default_factory=PointerStaleness)

    def reset(self, me_block: str | None = None) -> None:
        self.me_block = me_block
        self.stacker.reset()
        self.me_hitstun.reset()
        self.opp_hitstun.reset()
        self.prev_opp = None
        self.prev_opp_combo = 0
        self.last_emitted_mask = 0
        self.tick = 0

    def compute_scalars(
        self,
        me: dict,
        opp: dict,
        s: int,
        fwd_hold: float,
        back_hold: float,
        me_hitstun: bool,
        opp_hitstun: bool,
    ) -> dict | None:
        """build_scalars(), guarded by self.pointer_fields, with the
        sustained-failure escalation wired in (see the design note above
        PointerStaleness). Returns None on ticks where a live caller should
        hold its previous action -- never raises, never fabricates a
        coordinate. Fires exactly one `warnings.warn` per stale episode, on
        the first tick it crosses the escalation threshold;
        `self.pointer_staleness.escalated` stays True for the rest of the
        episode for any caller that wants to react further (e.g. stop
        pressing buttons, surface a UI banner) without re-deriving the
        threshold logic itself. Zero-cost, identical-dict-out fast path when
        `self.pointer_fields` is empty (the default)."""
        scal = build_scalars(
            me, opp, s, fwd_hold, back_hold, me_hitstun, opp_hitstun,
            pointer_fields=self.pointer_fields,
        )
        if self.pointer_staleness.observe(scal is not None):
            warnings.warn(
                f"pointer-resolved field(s) {sorted(self.pointer_fields)} "
                f"have not resolved for {self.pointer_staleness.stale_after_ticks} "
                "consecutive decision ticks -- this looks like a broken "
                "pointer chain for the rest of the session, not a momentary "
                "hole. The shadow is holding its last action instead of "
                "inventing a position; it will resume normal play the tick "
                "the field resolves again, but this session may need a "
                "restart if it doesn't.",
                RuntimeWarning,
                stacklevel=2,
            )
        return scal
