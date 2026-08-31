"""Measure a whole CHARACTER KIT across the spacing ladder — the operator
layer over `probe.py`.

`probe.py` measures one move at one gap under one rig. A kit is the cross
product of that with the ladder (`arenas.py`) and with hit-vs-block, plus the
two things that only exist once you look at the cross product:

  1. **The connect map.** MK2 has PROXIMITY NORMALS (docs/frames.md §5): the
     same button is a DIFFERENT MOVE by distance, and at long range it is no
     move at all. So the first pass over a (move, gap) grid is not an
     advantage measurement, it is a WHIFF/CONNECT map, built from the contact
     anchor alone — one replay per cell instead of ~120. Cells that whiff are
     a RESULT (§1.1: a whiff has no advantage number), and they bracket the
     move's connect range: "connected at 110 px, whiffed at 147 px".
  2. **Variant discrimination.** §4.4 forbids keying variants on FAF. What
     this module keys them on is what §5 permits — "measured differences in
     damage, advantage, or connect behaviour at explicit gaps" — and in
     practice DAMAGE is the sharpest of the three: Reptile's standing HP deals
     11 at 72 px and 24 at 62 px, which is not one move getting stronger, it
     is two moves. `variant` is therefore assigned from the damage/contact
     signature per rung, and rows are NEVER averaged across a boundary (§5).

Everything else is delegated: `session.LabSession` owns the §3 preconditions,
`probe` owns the differential act-again protocol and the §3.1 calibration,
`observables` owns the per-port observables, `store`/`export` own the schema.
This module owns only the ORDER those are called in and the honesty of what
comes out — in particular, it never invents a number for a cell it could not
measure (§2.5, §7).

## What one cell costs

A cell is one (move, gap) pair measured on hit AND on block: 2 contact
anchors + 4 exhaustive sweeps (attacker/defender × hit/block). The
guarded-defender sweep is the expensive one — it cannot share a control run,
because the guard must be RELEASED at the probe instant in both the probe and
the control run, which makes the control depend on N.

## Why `repeats=1` is defensible here, and what replaces it

`sweep_actionable(repeats=2)` exists to catch a measured ~1.5%/run transport
flake: a `hold_buttons` landing one frame early, which shows up as a single
spurious TRUE at an N where the fighter is still stunned. It doubles the cost
of every cell.

An EXHAUSTIVE sweep already detects that flake shape for free, and the
detector is `SweepResult.monotone`: a spurious TRUE below the boundary makes
the predicate `F..F T F..F T..T`, which is non-monotone, and a spurious FALSE
above it makes `F..F T..T F T..T`, also non-monotone. So this module runs
`repeats=1, exhaustive=True` and REFUSES any sweep whose predicate is not
monotone (`NonMonotoneError`) rather than paying 2x on every N to catch the
same thing. The residual hole is a flake landing exactly at `first_true - 1`,
which is monotone and moves the answer by one frame; that is what an
independent re-measurement is for — run the same cell a second time (a second
emulator process is better still) and `sample_n` records how many INDEPENDENT
full measurements are behind the row, rather than how many retries.

Live yield on the Reptile kit: 3 refusals in ~2,000 evaluations, every one a
single isolated TRUE far below the real boundary — exactly the flake shape,
caught rather than reported. Two of the three reproduced on a re-run and one
did not, so the refusal is not itself a reliable classifier of flake versus
genuinely non-monotone predicate; it only guarantees the number never ships.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Mapping, Optional, Sequence, Tuple

from . import observables as obs
from .spec import FramelabSpec
from .probe import (
    METHOD_LINEAR,
    Anchor,
    AdvantageMeasurement,
    MoveScript,
    NoContactError,
    ProbeError,
    Rig,
    ScriptStep,
    SweepResult,
    advantage_rows,
    calibrate_probe_latency,
    find_anchor,
    replay,
    sweep_actionable,
)
from .session import LabSession

__all__ = [
    "MoveSpec",
    "Rung",
    "ContactScan",
    "CellMeasurement",
    "NonMonotoneError",
    "CrossMethodError",
    "move_script",
    "make_rig",
    "scan_contact",
    "calibrate_shapes",
    "measure_cell",
    "cell_rows",
    "main",
]

# There is no module-level DEFAULT_OBSERVABLES here on purpose: §4.2's
# ordered candidate list (MK2 arcade: struct_velocity then pointer_x, the
# ONLY pair on this port that lives in different data structures, which is
# what makes §8.4's cross-method check a real check rather than two views of
# the same bytes) is now `FramelabSpec.default_observable_names()`, read from
# the profile's `framelab` block. `calibrate_shapes`/`measure_cell`/
# `cell_rows` below take `observables` as a required argument for exactly
# this reason — a silent per-port default here is the mistake CLAUDE.md
# forbids ("never hardcode a game address in code again") applied to an
# observable list instead of an address.

# Frames of contact-signal watching per anchor replay. Must exceed the slowest
# move's contact frame plus `Rig.quiet_frames`, or `find_anchor` refuses to
# call the cluster complete (and it is right to: `hits` would be wrong).
DEFAULT_ANCHOR_FRAMES = 48

# §4.3's MAX_SEARCH is 60. Reptile's normals were live-measured free within
# ~25 frames of contact, so 45 keeps the sweep exhaustive over a range that
# comfortably brackets every boundary seen while still cutting a quarter of
# the runtime. A sweep whose `first_true` lands within `_CAP_MARGIN` of the
# cap is REJECTED rather than reported (a boundary measured against the edge
# of the search is a silent cap, §7).
DEFAULT_MAX_SEARCH = 45
_CAP_MARGIN = 5


class NonMonotoneError(ProbeError):
    """An exhaustive sweep's predicate was not monotone, which on this port
    means either a transport flake (a one-frame-early hold) or an unsound
    observable — §4.2 measured exactly this shape out of the disqualified
    whole-struct diff. Either way the boundary is not a number yet."""


class CrossMethodError(ProbeError):
    """The two independent observables disagreed about the same advantage.
    §8.4 makes agreement REQUIRED, and §7 says a number that fails
    re-measurement is DELETED, not averaged — so this refuses the row."""


# ── the plan ──────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class MoveSpec:
    """One attack input, named in the family's own vocabulary.

    `buttons` is resolved from the profile's `attack_chords`, never spelled
    out here (CLAUDE.md: "never hardcode a game address in code again" — a
    button mapping is the same kind of fact).

    `stance_frames` is NOT cosmetic and was a measured rig bug. Asserting
    `down + button` on the SAME frame from a standing start makes MK2 enter
    *something* (the attacker's `action_counter` fires 160→192) that then
    contacts NOTHING at any rung of the ladder — a clean, plausible, entirely
    false "crouching normals never reach". Holding `down` alone for 6 frames
    first and only then adding the button produces the real crouching normal
    (Reptile's uppercut: contact at f14, 40 damage, defender launched). The
    lead-in is replayed identically in probe and control, so it cancels out of
    the differential exactly like a walk-in would.

    **The default 6 is not a safe default — it is the THRESHOLD on one
    ladder.** Swept live, three trials per value, fully deterministic: from
    Mileena's rungs the uppercut needs 6 held `down` frames and comes out at
    every value ≥ 6; from the Reptile mirror's rungs it needs 7, and at 6 it
    does not come out AT ALL. The difference is the ARENA, not the character —
    the Reptile rungs were saved without `settle_frames`, so the fighter is
    still mid-walk-animation on load and the residual walk eats a frame of the
    stance transition. Below the threshold every crouching cell prints `—`,
    which is exactly what a genuine whiff prints: the same §7 silent-cap shape
    as the anchor horizon, one layer down. No struct field was found that
    marks the stance transition (a whole-struct diff of a held `down` against
    neutral moves only the disqualified input echo at `+0x6C`), so there is no
    validator yet — sweep the lead-in at the collision floor before trusting a
    crouching row on a new ladder, and anchor well inside the threshold rather
    than on it (the re-scan that found this used 10).
    """

    name: str
    buttons: Tuple[str, ...]
    hold_frames: int = 2
    stance: str = "standing"          # "standing" | "crouching"
    stance_button: Optional[str] = None   # e.g. "down" for crouching
    stance_frames: int = 6

    @property
    def label(self) -> str:
        return self.name if self.stance == "standing" else f"c{self.name}"


@dataclass(frozen=True)
class Rung:
    """One rung of the §5 spacing ladder, as generated by `arenas.py`. `gap_px`
    is nullable by design — an arena whose object pointer did not resolve has
    an UNKNOWN gap, never 0."""

    arena: str
    gap_px: Optional[int]
    gap_walk_frames: Optional[int]

    @classmethod
    def from_sidecar(cls, state_path: str) -> "Rung":
        """Read `arenas.py`'s own `.gap.json` sidecar — NOT the app's
        `.meta.json`, whose `inputs_live` was measured through the disproven
        `p1_x`/`p2_x` globals (docs/frames.md §11)."""
        p = Path(state_path)
        side = p.parent / f"{p.stem}.gap.json"
        data = json.loads(side.read_text())
        return cls(
            arena=str(state_path),
            gap_px=data.get("gap_px"),
            gap_walk_frames=data.get("walk_frames"),
        )


@dataclass(frozen=True)
class ContactScan:
    """The cheap first pass: did this move reach at this gap, for how much,
    and did it KNOCK THE VICTIM DOWN. `damage` is read from the SAME
    struct-health anchor the advantage measurement uses (`block+0x0E`), so a
    variant's damage signature and its contact frame come from one replay.

    `knockdown` is the §1.1 gate on whether an `on_hit` number may exist at
    all: "airborne hit / juggle — no, the juggle owns the timing" and
    "knockdown — measure the WAKEUP WINDOW instead". It is derived from the
    victim's own `obj+0x16` y leaving its resting value after contact, which
    §10 says is the only honest test on arcade (resting y is character- AND
    stage-dependent, so there is no scalar GROUND_Y to compare against — the
    fighter's own pre-contact y is the reference).

    `anchor_frames`/`contact_horizon` are recorded on EVERY scan, connecting
    or not, because a `connected=False` is only a whiff RELATIVE TO A WINDOW.
    `find_anchor` needs `quiet_frames` of silence after the contact cluster
    inside the trace, so the horizon is `anchor_frames - quiet_frames` and a
    move contacting past it reports exactly like one that does not reach —
    the §7 failure this project has now made twice. A reader of a stored or
    printed whiff must be able to see which window produced it without
    reading the operator's command line.
    """

    move: str
    gap_px: Optional[int]
    connected: bool
    contact_frame: Optional[int] = None
    hits: Optional[int] = None
    damage: Optional[int] = None
    anchor: Optional[Anchor] = None
    note: str = ""
    knockdown: Optional[bool] = None
    airborne_until: Optional[int] = None
    anchor_frames: Optional[int] = None
    contact_horizon: Optional[int] = None


@dataclass
class CellMeasurement:
    """Everything one (move, gap) cell produced, including the raw sweeps —
    a stored row is a summary, and the report needs the predicate shapes."""

    move: MoveSpec
    rung: Rung
    variant: Optional[str]
    on_hit: Dict[str, AdvantageMeasurement] = field(default_factory=dict)
    on_block: Dict[str, AdvantageMeasurement] = field(default_factory=dict)
    scan_hit: Optional[ContactScan] = None
    scan_block: Optional[ContactScan] = None
    latencies: Mapping[str, Mapping[str, int]] = field(default_factory=dict)
    first_active_frame: Optional[int] = None
    connect_range: Optional[int] = None
    sample_n: int = 1
    notes: List[str] = field(default_factory=list)


# ── scripts and rigs ──────────────────────────────────────────────────────


def move_script(spec: MoveSpec) -> MoveScript:
    """The input program for one normal: assert the button (plus the stance
    direction, if any) at frame 0 and release it `hold_frames` later.

    There is no walk-in `lead_in`: the ladder arenas already ENCODE the
    spacing, so the move's input frame is frame 0 of the replay. That is not
    just tidier than walking in — it removes ~45 frames from every one of the
    ~200 replays a cell costs, and it removes the walk itself as a confound (a
    fighter that is still decelerating is not a fighter at rest). The only
    lead-in that survives is the STANCE one (see `MoveSpec.stance_frames`),
    which is a precondition of the move existing at all."""
    held = tuple(spec.buttons)
    lead: Tuple[ScriptStep, ...] = ()
    if spec.stance != "standing":
        if not spec.stance_button:
            raise ValueError(f"stance {spec.stance!r} needs a `stance_button`")
        held = (spec.stance_button,) + held
        lead = (ScriptStep(frames=spec.stance_frames, buttons=(spec.stance_button,)),)
    return MoveScript(
        name=spec.label,
        steps=(ScriptStep(frames=spec.hold_frames, buttons=held),),
        lead_in=lead,
    )


def make_rig(arena: str, *, guard_buttons: Sequence[str], attacker_port: int = 0,
             defender_port: int = 1, quiet_frames: int = 20,
             walk_directions_by_port: Optional[Mapping[int, Sequence[str]]] = None,
             ) -> Rig:
    """§4.2's blocked-direction hazard, wired: each port's FIRST walk
    candidate is AWAY from the opponent, because at contact range a fighter
    cannot walk into the other fighter's body and the probe would read that
    as "not actionable".

    `walk_directions_by_port` is normally sourced from the profile's
    `framelab.rig` block (`FramelabSpec.rig`, see `main()`), which is where
    the actual per-port convention for a given ladder lives now. The
    fallback below ("P1 stands on the left in every ladder arena", so P1
    walks left first and P2 right first) is kept as a generic default for
    callers that have not wired a profile through — every current unit test
    exercises exactly this default, unchanged — not as a silently-reused
    per-game assumption."""
    wdbp = (
        {p: tuple(ds) for p, ds in walk_directions_by_port.items()}
        if walk_directions_by_port is not None
        else {attacker_port: ("left", "right"), defender_port: ("right", "left")}
    )
    return Rig(
        arena=arena,
        attacker_port=attacker_port,
        defender_port=defender_port,
        guard_buttons=tuple(guard_buttons),
        walk_directions_by_port=wdbp,
        quiet_frames=quiet_frames,
    )


# ── pass 1: the connect map ───────────────────────────────────────────────


def scan_contact(
    session: LabSession,
    *,
    rig: Rig,
    spec: MoveSpec,
    gap_px: Optional[int],
    contact_read,
    defender_guard: bool,
    anchor_frames: int = DEFAULT_ANCHOR_FRAMES,
    victim_y_read=None,
    knockdown_frames: int = 100,
) -> ContactScan:
    """One replay: does this move's contact signal fire at this gap, and what
    does the victim's damage register do. Plus, when `victim_y_read` is given,
    a second short replay that answers §1.1's prior question — was the victim
    KNOCKED DOWN, in which case no `on_hit` number may be reported at all.

    A `NoContactError` is caught and returned as `connected=False` — §1.1: a
    whiff is an OUTCOME, and the honest record of it is "no contact at this
    gap", not a missing cell and certainly not a zero.
    """
    script = move_script(spec)
    horizon = anchor_frames - rig.quiet_frames
    window = dict(anchor_frames=anchor_frames, contact_horizon=horizon)
    try:
        anchor = find_anchor(
            session,
            rig=rig,
            script=script,
            contact_read=contact_read,
            total_frames=anchor_frames,
            defender_guard=defender_guard,
        )
    except NoContactError as exc:
        return ContactScan(
            move=spec.label, gap_px=gap_px, connected=False,
            note=(f"{str(exc).split('.')[0]} (searched {anchor_frames} frames; "
                  f"a contact later than f{horizon} cannot be confirmed inside "
                  f"it -- raise anchor_frames before calling this a whiff)"),
            **window,
        )
    except ProbeError as exc:
        # e.g. contact too close to the end of the trace for the quiet window.
        return ContactScan(
            move=spec.label, gap_px=gap_px, connected=False,
            note=f"contact seen but unusable: {exc}",
            **window,
        )
    before = anchor.trace[0]
    after = anchor.trace[anchor.contact_frame]
    damage = None
    if isinstance(before, int) and isinstance(after, int):
        damage = before - after

    knockdown: Optional[bool] = None
    airborne_until: Optional[int] = None
    if victim_y_read is not None:
        # A SECOND replay rather than one composite sampler, deliberately:
        # `find_anchor`'s clustering rule keys on "the contact value changed",
        # so folding y into that value would make every pixel of a knockdown
        # arc look like another hit. Two ~100-frame replays cost ~4 s and keep
        # the anchor exactly the anchor §4.1 specifies.
        ys = replay(
            session, rig=rig, script=script, total_frames=knockdown_frames,
            defender_guard=defender_guard,
            sample_fn=lambda s: {"y": victim_y_read(s)},
        )
        seq = [t["y"] for t in ys]
        rest = seq[0]
        off = [i for i, y in enumerate(seq) if y is not None and y != rest]
        knockdown = bool(off)
        if off:
            airborne_until = off[-1] + 1

    return ContactScan(
        move=spec.label,
        gap_px=gap_px,
        connected=True,
        contact_frame=anchor.contact_frame,
        hits=anchor.hits,
        damage=damage,
        anchor=anchor,
        knockdown=knockdown,
        airborne_until=airborne_until,
        **window,
    )


# ── calibration (§3.1, per probe SHAPE) ───────────────────────────────────


def calibrate_shapes(
    session: LabSession,
    *,
    rig: Rig,
    spec: MoveSpec,
    anchor: int,
    sample_fns: Mapping[int, Any],
    observables: Sequence[str],
    at_n: int = 70,
    confirm_at_n: Optional[int] = 100,
    trials: int = 5,
    max_window: int = 20,
) -> Dict[str, Dict[str, int]]:
    """The four probe shapes this module actually performs, each calibrated on
    ITS OWN input transition (§3.1, as corrected by the live failure recorded
    in `probe.calibrate_probe_latency`):

        attacker/hit, defender/hit, attacker/block, defender/block

    Returns `{shape: {observable: input_latency_frames}}`. The
    guarded-defender shape is the one that differs (it releases a held Block
    and walks on the same frame, and MK2's block stance does not drop when the
    button does); sizing the sweep's window from the neutral number made that
    sweep report NEVER ACTIONABLE across every candidate N.

    ## `confirm_at_n` — the hold-limited check §3.1 is missing

    A latency is only a latency if the measurement is HOLD-limited: the
    fighter must already be free at `anchor + at_n`, so that the first
    divergence is set by the injection, not by residual stun. §3.1 says "chosen
    far enough past the anchor that the fighter is certainly free" and gives
    no way to CHECK it — and the check is not optional, because a
    stun-limited calibration is silent and biases the advantage by the
    difference between the two sides' numbers.

    Live example that motivated this parameter: far HK's defender calibrates
    to 6/7 at `at_n=40` and 1/2 at `at_n=70` and 1/2 at `at_n=100` — the
    victim of a 32-damage roundhouse is simply not free 40 frames after
    contact. Taking the 40-frame number would have inflated that move's
    `on_hit` by 5 frames, in the safe direction, invisibly.

    So every shape is measured twice, at `at_n` and at `confirm_at_n`, and
    they must AGREE. A shape that is still shrinking is not calibrated, and
    §3.1's rule applies verbatim: STOP.
    """
    script = move_script(spec)
    out: Dict[str, Dict[str, int]] = {}
    for guard in (False, True):
        for port, who in ((rig.attacker_port, "attacker"), (rig.defender_port, "defender")):
            shape = f"{who}/{'block' if guard else 'hit'}"
            kw = dict(
                rig=rig, script=script, port=port, anchor=anchor,
                observables=list(observables), sample_fn=sample_fns[port],
                defender_guard=guard, trials=trials, max_window=max_window,
            )
            first = calibrate_probe_latency(session, at_n=at_n, **kw)
            if confirm_at_n is not None and confirm_at_n != at_n:
                second = calibrate_probe_latency(session, at_n=confirm_at_n, **kw)
                if second != first:
                    raise ProbeError(
                        f"probe shape {shape!r} is NOT hold-limited: latency at "
                        f"anchor+{at_n} is {first} but at anchor+{confirm_at_n} "
                        f"it is {second}. A latency that shrinks as the probe "
                        "moves later is residual STUN, not injection latency "
                        "(docs/frames.md §3.1). Move the calibration point "
                        "later; do not average."
                    )
            out[shape] = first
    return out


# ── pass 2: the cell ──────────────────────────────────────────────────────


def _check_sweep(sweep: SweepResult, *, who: str, cell: str) -> None:
    """Everything that makes a sweep's `first_true` NOT a number yet."""
    if sweep.first_true is None:
        return  # NULL is a legal outcome (§4.2's "record NULL"); caller reports it
    if sweep.monotone is False:
        raise NonMonotoneError(
            f"{cell} {who}/{sweep.observable}: predicate is not monotone "
            f"({''.join('T' if v else 'F' for v in sweep.predicate)}). On this "
            "port that is the signature of a one-frame-early hold or an "
            "unsound observable, not a boundary."
        )
    if sweep.first_true > sweep.max_search - _CAP_MARGIN:
        raise ProbeError(
            f"{cell} {who}/{sweep.observable}: first_true={sweep.first_true} is "
            f"within {_CAP_MARGIN} of max_search={sweep.max_search} -- the "
            "boundary was measured against the edge of the search window. "
            "Re-run with a larger max_search (docs/frames.md §7: no silent caps)."
        )


def measure_cell(
    session: LabSession,
    *,
    rig: Rig,
    spec: MoveSpec,
    rung: Rung,
    contact_read,
    sample_fns: Mapping[int, Any],
    latencies: Mapping[str, Mapping[str, int]],
    observables: Sequence[str],
    max_search: int = DEFAULT_MAX_SEARCH,
    anchor_frames: int = DEFAULT_ANCHOR_FRAMES,
    window_margin: int = 2,
    variant: Optional[str] = None,
    scans: Optional[Tuple[Optional[ContactScan], Optional[ContactScan]]] = None,
    victim_y_read=None,
    repeats: int = 1,
) -> CellMeasurement:
    """One (move, gap) cell: on_hit AND on_block, each measured by the full §4
    protocol, each from its OWN contact anchor.

    The two rigs get separate anchors on purpose. They are different runs of
    the game — the blocked one chips 3 where the clean one takes 11 — and
    reusing the hit rig's anchor for the block rig would silently assume the
    two moves connect on the same frame. (They did, on every cell measured so
    far; that agreement is evidence, and it is only evidence because it was
    not assumed.)

    `repeats` is passed straight to `sweep_actionable`. It defaults to 1 for
    the reason in this module's docstring — an exhaustive sweep already
    refuses the flake's non-monotone signature for free, so paying 2x on
    every N buys only the residual case. It is a PARAMETER rather than a
    constant because "did any evaluation fail its repeat check" is the direct
    evidence about whether the transport is flaky at all, and that question
    is worth answering deliberately (e.g. after a transport change) even
    though it is not worth answering on every run.
    """
    script = move_script(spec)
    cell = f"{spec.label}@{rung.gap_px}px"
    m = CellMeasurement(move=spec, rung=rung, variant=variant, latencies=dict(latencies))

    scan_hit, scan_block = scans if scans else (None, None)
    if scan_hit is None:
        scan_hit = scan_contact(session, rig=rig, spec=spec, gap_px=rung.gap_px,
                                contact_read=contact_read, defender_guard=False,
                                anchor_frames=anchor_frames, victim_y_read=victim_y_read)
    if scan_block is None:
        scan_block = scan_contact(session, rig=rig, spec=spec, gap_px=rung.gap_px,
                                  contact_read=contact_read, defender_guard=True,
                                  anchor_frames=anchor_frames)
    m.scan_hit, m.scan_block = scan_hit, scan_block
    if not (scan_hit.connected and scan_block.connected):
        m.notes.append(
            f"{cell}: no advantage measured -- connected(hit)="
            f"{scan_hit.connected}, connected(block)={scan_block.connected}. "
            "A whiff has no advantage number (docs/frames.md §1.1)."
        )
        return m

    rigs = [(False, scan_hit, m.on_hit), (True, scan_block, m.on_block)]
    if scan_hit.knockdown:
        # §1.1: a knockdown has no on-hit advantage — "it has a getup window,
        # measured by the same protocol but stored in a different column."
        # The on-BLOCK half is unaffected (a blocked hit leaves the defender
        # standing), so it is still measured; only the hit half is dropped.
        rigs = [(True, scan_block, m.on_block)]
        m.notes.append(
            f"{cell}: on_hit NOT measured -- the victim leaves its resting y "
            f"after contact and returns at frame {scan_hit.airborne_until} "
            "(knockdown/launch). docs/frames.md §1.1: that outcome has no hit-"
            "advantage number; the wakeup window is the measurement, and it is "
            "a different column. on_block is measured normally."
        )

    for guard, scan, target in rigs:
        shape_a = f"attacker/{'block' if guard else 'hit'}"
        shape_d = f"defender/{'block' if guard else 'hit'}"
        assert scan.anchor is not None
        attacker = sweep_actionable(
            session, rig=rig, script=script, port=rig.attacker_port,
            anchor=scan.anchor.contact_frame, observables=list(observables),
            sample_fn=sample_fns[rig.attacker_port],
            input_latency_frames=latencies[shape_a], defender_guard=guard,
            window_margin=window_margin, max_search=max_search,
            method=METHOD_LINEAR, exhaustive=True, repeats=repeats,
        )
        defender = sweep_actionable(
            session, rig=rig, script=script, port=rig.defender_port,
            anchor=scan.anchor.contact_frame, observables=list(observables),
            sample_fn=sample_fns[rig.defender_port],
            input_latency_frames=latencies[shape_d], defender_guard=guard,
            window_margin=window_margin, max_search=max_search,
            method=METHOD_LINEAR, exhaustive=True, repeats=repeats,
        )
        for o in observables:
            _check_sweep(attacker[o], who="attacker", cell=cell)
            _check_sweep(defender[o], who="defender", cell=cell)
            target[o] = AdvantageMeasurement(
                move=script.name, observable=o,
                rig_guard_state="held" if guard else "none",
                anchor=scan.anchor, attacker=attacker[o], defender=defender[o],
            )

    # §8.4: the two observables live in different data structures, so their
    # agreement is a real cross-method check. Disagreement is not averaged.
    for label, got in (("on_hit", m.on_hit), ("on_block", m.on_block)):
        vals = {o: got[o].advantage for o in observables if o in got}
        if len(set(vals.values())) > 1:
            raise CrossMethodError(
                f"{cell} {label}: observables disagree ({vals}). docs/frames.md "
                "§8.4 makes cross-method agreement REQUIRED and §7 forbids "
                "splitting the difference -- no row is written."
            )
    return m


def manifest_advantage(attacker: SweepResult, defender: SweepResult) -> Optional[int]:
    """The advantage this module STORES: the difference of the two sides'
    MANIFEST frames (`first_true + window`), not of their `first_true`s.

    ## Why this differs from `AdvantageMeasurement.advantage`, and how we know

    `probe.py` computes `defender.first_true - attacker.first_true`, on the
    argument that the injection latency `l` "is identical on both sides and
    cancels exactly". That is true only when both sides use the SAME probe
    shape. They do not in the on-block rig: the attacker's shape calibrates to
    `l = 1/2` and the guarded defender's to `l = 10/11`, because the defender
    must drop MK2's block stance before it can walk. Subtracting each side's
    own calibration then removes 9 frames from the defender that were never
    removed from the attacker, and every `on_block` number comes out 9 frames
    too favourable to the defender — i.e. every move looks 9 frames more
    punishable than it is.

    The 10/11 figure is measured with the fighter ALREADY FREE, which is what
    makes it a latency. During blockstun the stance drop runs CONCURRENTLY
    with the stun instead of after it, so it is not additional delay at all.

    This was settled by a third measurement that does not use the probe: a
    punish rig, in which the defender's counter-attack frame is swept and the
    attacker's damage register says whether it landed. "Earliest frame the
    defender can attack" came out at exactly `manifest − 2` in every
    configuration tested:

    | rig | move | walk manifest | earliest counter-attack |
    |---|---|---|---|
    | on block | close HP @62px | contact+19 | **contact+17** |
    | on block | far HP @72px   | contact+23 | **contact+21** |
    | on hit   | far HP @72px   | contact+14 | **contact+12** |

    The −2 is the same on both shapes, so it cancels out of a difference of
    manifests but a per-shape latency does not. `on_hit` rows are unaffected
    (both sides share one shape there, so the two formulas agree exactly);
    only `on_block` changes, and it changes by exactly `W_def − W_att = 9`.

    Consequence to state out loud: this REVISES the far-HP `on_block = +4`
    published in `library/mk2/mk2.md` on the same day to **+13**.
    """
    if attacker.first_true is None or defender.first_true is None:
        return None
    a = attacker.actionable_after_contact
    d = defender.actionable_after_contact
    if a is None or d is None:
        return None
    return d - a


def cell_rows(
    m: CellMeasurement,
    *,
    family: str,
    port: str,
    char: str,
    core_id: str,
    rom_id: str,
    observables: Sequence[str],
    confidence: str = "high",
) -> List[dict]:
    """One store row PER OBSERVABLE (§6: "a row measured by struct-divergence
    and one measured by `+0xC0` edge are different experiments"), carrying the
    columns this protocol actually measured and NULL — never 0 — for the rest.

    Filled here beyond `advantage_rows`: `damage` and `hits` from the anchor,
    `gap_px`/`gap_walk_frames` from the rung, `variant` from the damage
    signature, `connect_range` from the connect map, and `first_active_frame`
    ONLY for a minimum-gap row (§4.4 stores it nowhere else, because at larger
    gaps it is contaminated by travel).
    """
    rows: List[dict] = []
    for o in observables:
        if o not in m.on_hit and o not in m.on_block:
            continue
        row = advantage_rows(
            family=family, port=port, char=char, core_id=core_id, rom_id=rom_id,
            on_block=m.on_block.get(o), on_hit=m.on_hit.get(o),
            gap_walk_frames=m.rung.gap_walk_frames, gap_px=m.rung.gap_px,
            variant=m.variant, sample_n=m.sample_n, confidence=confidence,
        )[0]
        # The stored advantage is the MANIFEST difference (see
        # `manifest_advantage`), not `AdvantageMeasurement.advantage`. For
        # `on_hit` the two are identical by construction; for `on_block` they
        # differ by the 9-frame block-stance drop, and the punish rig says
        # which one the game agrees with.
        for col, meas in (("on_hit", m.on_hit.get(o)), ("on_block", m.on_block.get(o))):
            if meas is not None:
                row[col] = manifest_advantage(meas.attacker, meas.defender)
        scan = m.scan_hit
        if scan is not None and scan.connected:
            row["damage"] = scan.damage
            row["hits"] = scan.hits
            if scan.knockdown is not None:
                row["knockdown"] = int(scan.knockdown)
        row["first_active_frame"] = m.first_active_frame
        row["connect_range"] = m.connect_range
        rows.append(row)
    return rows


# ── operator entry point ──────────────────────────────────────────────────


def _fmt_predicate(sweep: SweepResult) -> str:
    return "".join("T" if v else "F" if v is not None else "." for v in sweep.predicate)


def main() -> None:  # pragma: no cover - the live-rig path
    """Drive a headless session over the ladder. Never point this at port
    4025 (CLAUDE.md: the user's session).

        python -m shadow_train.framelab.kit \\
            --url http://127.0.0.1:4047/mcp --game library/mk2 \\
            --core ../FBNeo/.../fbneo_libretro.dylib --rom ~/games/roms/mk2.zip \\
            --db shadow/framelab/frames.db \\
            --cell HP:shadow/arenas/mk2/gap-45.state:far
    """
    import argparse

    from shadow_train import profile as game_profile
    from shadow_train.mcpclient import McpClient

    from .identity import compute_core_id, compute_rom_id
    from .store import FrameStore

    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--url", default="http://127.0.0.1:4047/mcp")
    ap.add_argument("--game", default="library/mk2")
    ap.add_argument("--core", required=True)
    ap.add_argument("--rom", required=True)
    ap.add_argument("--db", default="shadow/framelab/frames.db")
    ap.add_argument("--char", default="reptile")
    ap.add_argument("--max-search", type=int, default=DEFAULT_MAX_SEARCH)
    ap.add_argument("--calibrate-on", default=None,
                    help="MOVE:ARENA to calibrate the four probe shapes on")
    ap.add_argument("--cell", action="append", default=[],
                    help="MOVE:ARENA[:VARIANT] — repeatable")
    ap.add_argument("--dry-run", action="store_true",
                    help="scan the connect map only; write nothing")
    args = ap.parse_args()

    prof = game_profile.load(args.game)
    # Everything §4.1/§4.2 measured for THIS port -- the anchor, the ordered
    # observable list + addressing, and the per-shape calibration table --
    # comes from the profile's `framelab` block from here on. A port with no
    # block (asurabld, mk2 Genesis) raises `FramelabNotConfigured` right
    # here, before anything touches the emulator.
    flspec = FramelabSpec.from_profile(prof)
    default_observables = flspec.default_observable_names()
    f1 = obs.resolve_fighter(prof, "block1", 0)
    f2 = obs.resolve_fighter(prof, "block2", 1)
    sample_fns = {0: obs.make_sampler(f1, flspec), 1: obs.make_sampler(f2, flspec)}
    contact_read = obs.make_contact_read_from_spec(f2, flspec)  # DEFENDER is the victim
    client = McpClient(args.url)
    session = LabSession(client, verify_fn=obs.make_arena_verifier(prof, expect={}))
    session.enforce_preconditions()
    rig_wdbp = flspec.rig.walk_directions_by_port if flspec.rig else None

    core_id, rom_id = compute_core_id(args.core), compute_rom_id(args.rom)
    specs = {name: MoveSpec(name=name, buttons=tuple(btns))
             for name, btns in prof.attack_chords.items() if name != "Block"}
    guard = tuple(prof.attack_chords["Block"])

    cells = []
    for raw in args.cell:
        parts = raw.split(":")
        move, arena = parts[0], parts[1]
        variant = parts[2] if len(parts) > 2 else None
        cells.append((specs[move], Rung.from_sidecar(arena), variant))

    latencies: Dict[str, Dict[str, int]] = {}
    if args.calibrate_on and not args.dry_run:
        move, arena = args.calibrate_on.split(":")
        rig = make_rig(arena, guard_buttons=guard, quiet_frames=flspec.quiet_frames,
                       walk_directions_by_port=rig_wdbp)
        scan = scan_contact(session, rig=rig, spec=specs[move], gap_px=None,
                            contact_read=contact_read, defender_guard=False)
        if not scan.connected:
            raise SystemExit(f"cannot calibrate on {args.calibrate_on}: it whiffs")
        assert scan.anchor is not None
        latencies = calibrate_shapes(session, rig=rig, spec=specs[move],
                                     anchor=scan.anchor.contact_frame,
                                     sample_fns=sample_fns,
                                     observables=default_observables)
        print("calibration:", json.dumps(latencies, sort_keys=True))

    out_rows: List[dict] = []
    for move_spec, rung, variant in cells:
        rig = make_rig(rung.arena, guard_buttons=guard, quiet_frames=flspec.quiet_frames,
                       walk_directions_by_port=rig_wdbp)
        if args.dry_run:
            s = scan_contact(session, rig=rig, spec=move_spec, gap_px=rung.gap_px,
                             contact_read=contact_read, defender_guard=False)
            print(f"{move_spec.label:>4} @ {rung.gap_px}px  connected={s.connected} "
                  f"contact={s.contact_frame} dmg={s.damage}")
            continue
        m = measure_cell(session, rig=rig, spec=move_spec, rung=rung,
                         contact_read=contact_read, sample_fns=sample_fns,
                         latencies=latencies, max_search=args.max_search,
                         variant=variant, observables=default_observables)
        for note in m.notes:
            print("NOTE:", note)
        for label, got in (("on_hit", m.on_hit), ("on_block", m.on_block)):
            for o, meas in got.items():
                print(f"{move_spec.label:>4} @ {rung.gap_px}px {label:>8} [{o:>15}] "
                      f"att={meas.attacker.first_true} def={meas.defender.first_true} "
                      f"adv={meas.advantage:+d} "
                      f"att[{_fmt_predicate(meas.attacker)}] "
                      f"def[{_fmt_predicate(meas.defender)}]")
        out_rows += cell_rows(m, family=prof.family, port=prof.port, char=args.char,
                              core_id=core_id, rom_id=rom_id, observables=default_observables)

    if out_rows:
        Path(args.db).parent.mkdir(parents=True, exist_ok=True)
        with FrameStore(args.db) as store:
            for row in out_rows:
                store.insert(row)
        print(f"wrote {len(out_rows)} rows to {args.db}")
    print(f"steps={session.steps_taken} loads={session.loads_done}")


if __name__ == "__main__":  # pragma: no cover
    main()
