"""docs/frames.md §1.2 / §11 — `hitstop`, measured.

`hitstop` has been NULL in every shipped row since the schema was written
(§11: "Hitstop is unmeasured, though §1.2 reserves a column for it"). §1.2:
"On contact both fighters freeze for N frames. Because it applies to both
lanes equally it CANCELS OUT of advantage — but it does not cancel out of
measurement". This module is that measurement.

## Two absolute methods were tried live and rejected before this one

**1. "Did anything in the fighter struct change frame-to-frame."** Ruled out
by evidence already on record, not by theory: `library/mk2/mk2.md`'s own
"twin-counter hunt" section states "a fighter's struct is entirely static
when untouched (0 of `0x17A` bytes change)" and, separately, that a
CONTINUOUSLY-GUARDING defender under live contact shows "idle churn = 0
bytes" for six trials straight. This module re-confirmed it directly: a
45-frame whole-struct trace of BOTH fighters around a real contact (Reptile
HK, `gap-60.state`, no input held on either port at all) changes exactly TWO
bytes on the contact frame (`health` and one `struct_velocity` byte) and then
**shows zero further byte activity on either fighter for the next 24
frames** — the entire hitstun/blockstun window is, on this port, exactly as
silent as genuine idle. The one exception seen (a periodic 2-byte tick 25
frames later, on the ATTACKER only) reproduces mk2.md's own documented
"idle-animation loop, ~150-500 ms period" (line 365) — unrelated to the hit,
present whether or not one occurred.

The consequence: a naive freeze-span counter over this struct bundle cannot
merely be FOOLED by an idle span, the way the task brief's warned-against
failure mode goes (a free-running counter that keeps ticking through a real
freeze, making the span look shorter than it is). On MK2 arcade it is
WORSE than that: there is no free-running counter to expose the difference
in the first place, so "hitstop-frozen" and "ordinary quiet stun" produce
**byte-for-byte identical evidence** — a silent false negative a control
comparison cannot catch, because the control would read identically too.
`_naive_freeze_span` below exists only to demonstrate this in a unit test
against a fake, and is never used to produce a stored number.

**2. Borrowing asurabld's free-running counter.** The task brief that
commissioned this module cited "MK2 has a documented free-running animation
counter (`block2+0x12` free-runs even at total idle)". That sentence is
`library/asurabld/asurabld.md`'s finding (block2's `+0x12`, `0`→`63` even at
total idle), not MK2's — `library/mk2/mk2.md` documents no per-frame
free-running byte in either fighter struct, and finding (1) above confirms
why: MK2's struct is measured SILENT, not merely absolute-observation-unsafe.
docs/frames.md §7 ("check the evidence doc before asserting a signal") and
CLAUDE.md ("never hardcode a game address in code again") both apply here to
a FACT, not just an address: a byte-behaviour measured on one game's memory
layout does not transfer to another's, and this module does not borrow it.

## What actually works — the whiff-differenced attacker manifest

`probe.sweep_actionable` already answers "the first frame after ANCHOR this
port's held input diverges from a no-input control" for any anchor, not just
a contact frame. Two runs of the identical attack SCRIPT, both fed through
that exact machinery:

  * **connecting** — anchored on the move's own contact frame (`find_anchor`,
    §4.1): the attacker's manifest is `contact_frame + first_true`.
  * **whiff reference** — the SAME script thrown at a gap far enough that it
    provably does not connect (`scan_contact` reports `connected=False`),
    anchored on `script.total_frames` (the frame the scripted input ends):
    the attacker's manifest is `script.total_frames + first_true`.

§1.2 says the freeze fires "on contact" — a script that never connects never
freezes, so its attacker manifest is the move's intrinsic startup+recovery
length with hitstop otherwise wired in at exactly zero. Hitstop pads the
SAME animation clock recovery is measured against (the fact §1.2 states
from the other side — "cancels out of advantage, not measurement" is only
true because the attacker's OWN manifest carries it, and only the
DIFFERENCE of two manifests cancels it). Subtracting:

    hitstop(outcome) = manifest(connecting, outcome) - manifest(whiff)

This is the differential shape §4.2 requires, applied one level up: it is
never "did X change", it is "does the identical script take longer to free
its own thrower when the same script also happens to connect". Anything
that would move on its own regardless of contact — pushback, an animation
tick, `probe.py`'s own injection latency/margin `l + m` for whichever
observable is in use — appears in BOTH manifests (same script, same port,
same observable, same window) and cancels out of the subtraction exactly the
way it cancels out of `probe.py`'s `advantage`. Measured live and
reproduced 4/4 identical trials once the whiff anchor was moved off frame 0
(see `measure_whiff_reference`'s docstring for why frame 0 itself is
unusable — a probe/schedule interaction, not a flake).

docs/frames.model §8.4 classifies `hitstop` as an ANCHOR-based duration
field (with `active`/`recovery`/`total`), held to EXACT cross-observable
agreement, not the one-sided margin rule. This falls out of the algebra
above for free: `window` (which carries the per-observable margin) appears
once in each of the two manifests being subtracted and cancels regardless of
which observable it belongs to, so both of this port's observables
(`struct_velocity`, `pointer_x`) must return the identical integer. Measured:
they did, 12 == 12, on the first live cell tried. `measure_hitstop_outcome`
raises `CrossObservableHitstopError` rather than average a disagreement.

## Where this is NOT attempted, by construction

  * **Close-range variants.** MK2's proximity-normal selection (docs/frames.md
    §5) puts the close-range floor (61-63 px, per matchup) at the SAME
    distance a close normal's own reach ends — there is no gap left over to
    manufacture a "still resolves to the close move, still misses" whiff
    reference the way the ladder's own farthest rung (`gap-0`/`m-gap-0`/
    `b-gap-0`, ~192 px) safely does for far-range and non-variant moves.
    Building one would mean a new arena engineered right at a move's connect
    edge — exactly the "measure specials at a rung well inside the connect
    region, never at its edge" hazard §5 already names as unreproducible.
    Judged out of this task's "where it is cheap" scope; `hitstop` stays
    NULL on every close-variant row, and `plan_hitstop_cells` reports them
    as `skip_reason="close-range variant has no safe whiff reference"`.
  * **A launcher's on-hit side** (`cHP` here) already has no `on_hit`
    advantage (§1.1's knockdown gate) — that gate is about the DEFENDER's
    manifest, and does not stop the ATTACKER's own manifest (what this
    module reads) from being measured on either outcome. Where a cell's
    existing row is block-only (its on_hit column is already NULL for the
    knockdown reason), this module measures hitstop for the outcome that
    row actually represents and leaves the rest alone.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Callable, Dict, Hashable, List, Mapping, Optional, Sequence, Tuple

from .kit import MoveSpec, Rung, make_rig, move_script, scan_contact
from .probe import ProbeError, Rig, SweepResult, sweep_actionable
from .session import LabSession
from .spec import FramelabSpec
from .store import FrameStore

__all__ = [
    "HitstopError",
    "WhiffNotAWhiffError",
    "CrossObservableHitstopError",
    "WhiffReference",
    "HitstopOutcome",
    "HitstopCell",
    "naive_freeze_span",
    "compute_hitstop",
    "collapse_hitstop",
    "measure_whiff_reference",
    "measure_hitstop_outcome",
    "measure_hitstop_cell",
    "plan_hitstop_cells",
    "update_store_for_character",
    "main",
]

# Same bound `kit.py`/`probe.py` use elsewhere in this lab (§4.3's
# MAX_SEARCH=60, `kit.py`'s DEFAULT_MAX_SEARCH=45 as the practical figure
# already validated live on this port's normals).
DEFAULT_MAX_SEARCH = 45
DEFAULT_WINDOW_MARGIN = 2


class HitstopError(ProbeError):
    """A hitstop measurement could not produce a trustworthy number."""


class WhiffNotAWhiffError(HitstopError):
    """The arena chosen as a whiff reference actually connected. Using it
    would silently measure hitstop against a NON-zero baseline and call the
    difference hitstop=0 by construction — worse than refusing."""


class CrossObservableHitstopError(HitstopError):
    """docs/frames.md §8.4: `hitstop` is an anchor-based duration field and
    is held to EXACT cross-observable agreement, not the one-sided margin
    rule. A disagreement here means the protocol was wrong somewhere, not
    that the truth is in the middle (§7) — never averaged."""


# ── the naive detector, kept ONLY to demonstrate why it is unsound ────────


def naive_freeze_span(
    trace: Sequence[Mapping[str, Hashable]], keys: Sequence[str], start: int = 0
) -> int:
    """The detector docs/frames.md's task brief warns against: count
    consecutive frames from `start` (inclusive) during which NONE of `keys`
    changes frame-to-frame in `trace`. This is an ABSOLUTE observation
    ("did X change") — exactly the shape §4.2's law forbids trusting on a
    game where things move on their own — and it is included here only so a
    unit test can show it false-firing on an ordinary idle span (see
    `test_framelab_hitstop.py`). No code path in this module uses it to
    produce a stored value.

    Returns the number of frames from `start` for which the bundle stayed
    frozen (0 if `trace[start]` already differs from `trace[start + 1]`).
    """
    n = 0
    f = start
    while f + 1 < len(trace):
        cur, nxt = trace[f], trace[f + 1]
        if any(cur.get(k) != nxt.get(k) for k in keys):
            break
        n += 1
        f += 1
    return n


# ── the sound differential comparison ──────────────────────────────────────


def compute_hitstop(
    *,
    contact_frame: int,
    connecting_first_true: Optional[int],
    whiff_anchor: int,
    whiff_first_true: Optional[int],
) -> Optional[int]:
    """The one comparison this module trusts: the difference of two attacker
    manifests, both already margin-free at the `first_true` stage
    (`probe.py`'s module docstring: "first_true" is the quantity every
    observable returns identically for the same underlying event). NULL
    propagates (§2.5) — either side missing means hitstop is not a number
    here, never a guessed one.
    """
    if connecting_first_true is None or whiff_first_true is None:
        return None
    return (contact_frame + connecting_first_true) - (whiff_anchor + whiff_first_true)


def collapse_hitstop(per_observable: Mapping[str, Optional[int]], *, where: str) -> Optional[int]:
    """docs/frames.md §8.4: `hitstop` is an anchor-based duration field, held
    to EXACT cross-observable agreement (unlike a one-sided manifest, which
    is allowed to differ by the observables' own latency margins). NULL
    entries are ignored (an observable that produced no number is not a
    disagreement); a real disagreement among the rest raises rather than
    averages (§7).
    """
    measured = {o: v for o, v in per_observable.items() if v is not None}
    if not measured:
        return None
    vals = set(measured.values())
    if len(vals) > 1:
        raise CrossObservableHitstopError(
            f"{where}: observables disagree on hitstop ({measured}). "
            "docs/frames.md §8.4 requires exact agreement for this "
            "anchor-based duration field; §7 forbids averaging."
        )
    return next(iter(vals))


def _check_monotone(sweep: SweepResult, *, where: str) -> None:
    """The same non-monotone-is-not-a-number rule `kit.py._check_sweep`
    applies to advantage sweeps, reproduced here (not imported: that
    function is private to `kit.py`, which this task does not edit) because
    an attacker-manifest sweep for hitstop is exposed to the identical
    transport hazard (docs/frames.md §4.3/§3.6)."""
    if sweep.first_true is None:
        return
    if sweep.monotone is False:
        raise HitstopError(
            f"{where}: predicate is not monotone "
            f"({''.join('T' if v else 'F' for v in sweep.predicate)}) — "
            "docs/frames.md §4.3: this is the signature of a one-frame-early "
            "hold or an unsound observable, not a boundary."
        )
    if sweep.first_true > sweep.max_search - 5:
        raise HitstopError(
            f"{where}: first_true={sweep.first_true} is within 5 frames of "
            f"max_search={sweep.max_search} — boundary measured against the "
            "edge of the search (docs/frames.md §7: no silent caps)."
        )


@dataclass(frozen=True)
class WhiffReference:
    """`R` — the intrinsic, hitstop-free attacker manifest for one
    (character, move) script, shared by every gap that throws the SAME
    variant (§5: "far" is one move usable across a range; MK2's proximity
    selection only forks close-vs-far, never far-vs-farther, so one whiff
    reference at the ladder's farthest rung covers every far-range row)."""

    move: str
    anchor: int  # = script.total_frames, NOT 0 -- see measure_whiff_reference
    arena: str
    gap_px: Optional[float]
    first_true: Dict[str, Optional[int]]
    sweeps: Dict[str, SweepResult] = field(repr=False, default_factory=dict)


def measure_whiff_reference(
    session: LabSession,
    *,
    rig: Rig,
    spec: MoveSpec,
    contact_read: Callable,
    observables: Sequence[str],
    sample_fn: Callable,
    input_latency_frames: Mapping[str, int],
    max_search: int = DEFAULT_MAX_SEARCH,
    window_margin: int = DEFAULT_WINDOW_MARGIN,
    anchor_frames: int = 48,
) -> WhiffReference:
    """Measure `R` on a gap where `spec` provably does not connect.

    **The anchor is `script.total_frames`, never `0`.** `probe._schedule`'s
    precedence rule is "script < guard release < probe" — the PROBE wins a
    tie. Anchoring at N=0 means the walk-direction probe is asserted on the
    exact frame the attack's own button is, which on this port REPLACES the
    attack input rather than adding to it (`hold_buttons` replaces, not ORs),
    so the move never throws in either the probe OR the control run. That
    produced a perfectly REPRODUCIBLE (4/4 trials) but wrong `first_true=0`
    live — not the known one-frame transport flake (that one is documented
    as non-reproducing on re-run; this was 4/4 identical), a harness bug.
    Anchoring at `script.total_frames`, well past the button's own hold,
    avoids the collision by construction and reproduced cleanly (3/3 trials,
    monotone, `first_true` identical across trials) once corrected.

    Raises `WhiffNotAWhiffError` if the move actually connects at `rig`'s
    arena — using a connecting run as a "hitstop-free" reference would
    silently bake a real hitstop into what this module treats as zero.
    """
    scan = scan_contact(
        session, rig=rig, spec=spec, gap_px=None, contact_read=contact_read,
        defender_guard=False, anchor_frames=anchor_frames,
    )
    if scan.connected:
        raise WhiffNotAWhiffError(
            f"whiff reference for {spec.label!r} at {rig.arena!r} actually "
            f"connected (contact frame {scan.contact_frame}, damage "
            f"{scan.damage}) -- this arena is not far enough out to serve as "
            "a hitstop=0 baseline."
        )
    script = move_script(spec)
    anchor = script.total_frames
    sweeps = sweep_actionable(
        session, rig=rig, script=script, port=rig.attacker_port, anchor=anchor,
        observables=list(observables), sample_fn=sample_fn,
        input_latency_frames=dict(input_latency_frames), defender_guard=False,
        window_margin=window_margin, max_search=max_search, exhaustive=True,
    )
    for o in observables:
        _check_monotone(sweeps[o], where=f"whiff reference {spec.label!r}/{o}")
    return WhiffReference(
        move=spec.label, anchor=anchor, arena=rig.arena, gap_px=None,
        first_true={o: sweeps[o].first_true for o in observables}, sweeps=sweeps,
    )


@dataclass(frozen=True)
class HitstopOutcome:
    """One outcome (hit or block) of one cell, per observable and collapsed."""

    move: str
    rig_guard_state: str  # "none" (hit) | "held" (block)
    gap_px: Optional[float]
    contact_frame: int
    per_observable: Dict[str, Optional[int]]
    hitstop: Optional[int]  # the cross-checked, collapsed value


def measure_hitstop_outcome(
    session: LabSession,
    *,
    rig: Rig,
    spec: MoveSpec,
    gap_px: Optional[float],
    contact_read: Callable,
    observables: Sequence[str],
    sample_fn: Callable,
    input_latency_frames: Mapping[str, int],
    defender_guard: bool,
    reference: WhiffReference,
    max_search: int = DEFAULT_MAX_SEARCH,
    window_margin: int = DEFAULT_WINDOW_MARGIN,
    anchor_frames: int = 48,
) -> HitstopOutcome:
    """Measure hitstop for one outcome (hit if `defender_guard=False`, block
    if `True`) of one (move, gap) cell, cross-checked across every observable
    in `observables` (docs/frames.md §8.4 — EXACT agreement required for this
    anchor-based duration field, never averaged on disagreement).
    """
    scan = scan_contact(
        session, rig=rig, spec=spec, gap_px=gap_px, contact_read=contact_read,
        defender_guard=defender_guard, anchor_frames=anchor_frames,
    )
    if not scan.connected:
        raise HitstopError(
            f"{spec.label!r} at {gap_px}px (guard={defender_guard}) did not "
            f"connect on this run ({scan.note}) -- cannot measure hitstop for "
            "a cell that has no contact this time."
        )
    assert scan.anchor is not None
    shape = f"attacker/{'block' if defender_guard else 'hit'}"
    sweeps = sweep_actionable(
        session, rig=rig, script=move_script(spec), port=rig.attacker_port,
        anchor=scan.anchor.contact_frame, observables=list(observables),
        sample_fn=sample_fn, input_latency_frames=dict(input_latency_frames),
        defender_guard=defender_guard, window_margin=window_margin,
        max_search=max_search, exhaustive=True,
    )
    per_obs: Dict[str, Optional[int]] = {}
    for o in observables:
        _check_monotone(sweeps[o], where=f"{spec.label!r}/{shape}/{o}")
        per_obs[o] = compute_hitstop(
            contact_frame=scan.anchor.contact_frame,
            connecting_first_true=sweeps[o].first_true,
            whiff_anchor=reference.anchor,
            whiff_first_true=reference.first_true.get(o),
        )
    where = f"{spec.label!r} at {gap_px}px ({shape})"
    collapsed = collapse_hitstop(per_obs, where=where)
    if collapsed is not None and collapsed < 0:
        # Physically impossible: a connecting replay can only take LONGER
        # to free the attacker than the same script's hitstop-free whiff
        # reference, never shorter (hitstop only ADDS frames, §1.2). A
        # negative result means the whiff reference does not describe the
        # SAME move as this connecting replay -- almost always a proximity
        # variant this pass's `move` label does not distinguish (§5: the
        # button can resolve to a different animation at a different range,
        # exactly the close/far fork already known for standing normals; a
        # move recorded under one label with no `variant` column may still
        # fork this way even though nothing here decided it does). Refusing
        # rather than storing a number that cannot be true.
        raise HitstopError(
            f"{where}: computed hitstop={collapsed} < 0, which is physically "
            "impossible (hitstop only adds frames on top of a move's own "
            "recovery, docs/frames.md §1.2). The whiff reference almost "
            "certainly does not describe the same move/variant as this "
            "connecting replay -- refusing to store a negative number "
            f"(per-observable: {per_obs})."
        )
    return HitstopOutcome(
        move=spec.label,
        rig_guard_state="held" if defender_guard else "none",
        gap_px=gap_px,
        contact_frame=scan.anchor.contact_frame,
        per_observable=per_obs,
        hitstop=collapsed,
    )


@dataclass(frozen=True)
class HitstopCell:
    """Both outcomes of one (move, gap) cell, plus the single value this
    schema's one `hitstop` column can actually hold.

    §1.2/the task brief: hitstop may differ by hit vs block. This store has
    exactly one `hitstop` column per row (docs/frames.md §6, unmodified by
    this task), so when the two outcomes disagree this dataclass keeps BOTH
    (`on_hit`/`on_block`) and `stored` picks on-hit as the schema's canonical
    value — documented here, not silently decided by whichever ran last.
    When only one outcome was measured for this cell (a knockdown row is
    block-only), `stored` is that one value.
    """

    on_hit: Optional[HitstopOutcome]
    on_block: Optional[HitstopOutcome]

    @property
    def stored(self) -> Optional[int]:
        if self.on_hit is not None and self.on_hit.hitstop is not None:
            return self.on_hit.hitstop
        if self.on_block is not None:
            return self.on_block.hitstop
        return None

    @property
    def agrees(self) -> Optional[bool]:
        """None if only one outcome exists to compare."""
        if self.on_hit is None or self.on_block is None:
            return None
        if self.on_hit.hitstop is None or self.on_block.hitstop is None:
            return None
        return self.on_hit.hitstop == self.on_block.hitstop


def measure_hitstop_cell(
    session: LabSession,
    *,
    rig: Rig,
    spec: MoveSpec,
    gap_px: Optional[float],
    contact_read: Callable,
    observables: Sequence[str],
    sample_fn: Callable,
    latencies: Mapping[str, Mapping[str, int]],
    reference: WhiffReference,
    measure_hit: bool = True,
    measure_block: bool = True,
    max_search: int = DEFAULT_MAX_SEARCH,
    window_margin: int = DEFAULT_WINDOW_MARGIN,
) -> HitstopCell:
    on_hit = None
    on_block = None
    if measure_hit:
        on_hit = measure_hitstop_outcome(
            session, rig=rig, spec=spec, gap_px=gap_px, contact_read=contact_read,
            observables=observables, sample_fn=sample_fn,
            input_latency_frames=latencies["attacker/hit"], defender_guard=False,
            reference=reference, max_search=max_search, window_margin=window_margin,
        )
    if measure_block:
        on_block = measure_hitstop_outcome(
            session, rig=rig, spec=spec, gap_px=gap_px, contact_read=contact_read,
            observables=observables, sample_fn=sample_fn,
            input_latency_frames=latencies["attacker/block"], defender_guard=True,
            reference=reference, max_search=max_search, window_margin=window_margin,
        )
    return HitstopCell(on_hit=on_hit, on_block=on_block)


# ── selecting which already-measured rows are eligible ─────────────────────


def plan_hitstop_cells(rows: Sequence[Mapping]) -> Tuple[List[dict], List[dict]]:
    """Split a character's existing `move_frames` rows into
    `(eligible, skipped)`. `eligible` entries carry enough to drive
    `measure_hitstop_cell`/`measure_whiff_reference`; `skipped` entries carry
    a `skip_reason` (docs/frames.md §7: "no silent caps" — every skip is
    named, not just absent from the output).

    A row is INELIGIBLE when its own `variant` is a proximity-selected
    `"close"` (no safe whiff reference exists on this port, see the module
    docstring) or when it already carries a `hitstop` value (idempotent
    re-runs do not re-measure a cell for free).
    """
    eligible: List[dict] = []
    skipped: List[dict] = []
    for row in rows:
        if row.get("hitstop") is not None:
            skipped.append({**dict(row), "skip_reason": "already has a hitstop value"})
            continue
        if row.get("variant") == "close":
            skipped.append(
                {**dict(row), "skip_reason": "close-range variant has no safe whiff reference"}
            )
            continue
        eligible.append(dict(row))
    return eligible, skipped


# ── orchestration against the real store ────────────────────────────────


def _spec_for_move(move_name: str, prof) -> MoveSpec:
    """Rebuild the `MoveSpec` a stored row's `move` name came from
    (`kit.MoveSpec.label`'s own encoding: a crouching move is `"c" + base`).
    Chords come from `prof.attack_chords`, never spelled out here (CLAUDE.md).
    Raises `ValueError` for a move this pass does not know how to re-throw
    (a special/macro move — out of scope for this pass, see module
    docstring's "what this module does NOT attempt")."""
    if move_name in prof.attack_chords and move_name != "Block":
        return MoveSpec(name=move_name, buttons=tuple(prof.attack_chords[move_name]))
    if move_name.startswith("c") and move_name[1:] in prof.attack_chords:
        base = move_name[1:]
        # `MoveSpec.stance_frames`'s docstring (kit.py, live-updated this
        # same wave): the ladder arenas under `shadow/arenas/mk2/gap-*`
        # (the Reptile mirror) were saved WITHOUT `settle_frames`, so a
        # 6-frame `down` hold -- the dataclass default -- does not reliably
        # produce a crouching normal there (the residual walk animation eats
        # a frame of the stance transition; measured live: needs 7 on this
        # ladder, ≥6 on the settled `m-gap-*`/`b-gap-*` ladders). 7 satisfies
        # both, so it is used unconditionally rather than re-deriving which
        # ladder a row's arena belongs to.
        return MoveSpec(
            name=base, buttons=tuple(prof.attack_chords[base]),
            stance="crouching", stance_button="down", stance_frames=7,
        )
    raise ValueError(
        f"{move_name!r} is not a plain button chord or crouching normal this "
        "pass can rebuild a script for (a special/macro move needs its own "
        "MoveScript, out of scope for this pass)."
    )


def _arena_for_row(row: Mapping, *, ladder_prefix: str, arena_dir: str) -> str:
    gwf = row.get("gap_walk_frames")
    if gwf is None:
        raise ValueError(
            f"row {row.get('move')!r}@{row.get('gap_px')}px has no "
            "gap_walk_frames -- cannot resolve which ladder arena it lives on."
        )
    return f"{arena_dir}/{ladder_prefix}gap-{gwf}.state"


def update_store_for_character(
    session: LabSession,
    *,
    store: FrameStore,
    prof,
    flspec: FramelabSpec,
    char: str,
    whiff_arena: str,
    ladder_prefix: str,
    arena_dir: str,
    observables: Sequence[str],
    sample_fns: Mapping[int, Callable],
    contact_read: Callable,
    attacker_port: int = 0,
    defender_port: int = 1,
    max_search: int = DEFAULT_MAX_SEARCH,
    window_margin: int = DEFAULT_WINDOW_MARGIN,
    dry_run: bool = False,
) -> dict:
    """Measure and (unless `dry_run`) UPDATE `hitstop` for every eligible
    cell of `char` already in `store`, sharing one `WhiffReference` per MOVE
    across every gap that shares its variant (§5).

    Returns `{"updated": [...], "skipped": [...], "errors": [...]}` — every
    row this pass touched, declined, or failed on, named (docs/frames.md §7:
    "no silent caps").
    """
    rows = [r for r in store.rows_for(prof.family, prof.port) if r["char"] == char]
    eligible, skipped = plan_hitstop_cells(rows)
    report: dict = {"updated": [], "skipped": list(skipped), "errors": []}

    guard = tuple(prof.attack_chords["Block"])
    rig_wdbp = flspec.rig.walk_directions_by_port if flspec.rig else None
    latencies = {
        "attacker/hit": {
            o: flspec.observable(o).latency_for("attacker/hit") for o in observables
        },
        "attacker/block": {
            o: flspec.observable(o).latency_for("attacker/block") for o in observables
        },
    }

    def _rig(arena: str) -> Rig:
        return make_rig(
            arena, guard_buttons=guard, quiet_frames=flspec.quiet_frames,
            walk_directions_by_port=rig_wdbp, attacker_port=attacker_port,
            defender_port=defender_port,
        )

    by_move: Dict[str, List[dict]] = {}
    for row in eligible:
        by_move.setdefault(row["move"], []).append(row)

    for move_name, move_rows in sorted(by_move.items()):
        try:
            spec = _spec_for_move(move_name, prof)
        except ValueError as exc:
            for row in move_rows:
                report["skipped"].append({**row, "skip_reason": str(exc)})
            continue

        try:
            reference = measure_whiff_reference(
                session, rig=_rig(whiff_arena), spec=spec, contact_read=contact_read,
                observables=observables, sample_fn=sample_fns[attacker_port],
                input_latency_frames=latencies["attacker/hit"],
                max_search=max_search, window_margin=window_margin,
            )
        except HitstopError as exc:
            for row in move_rows:
                report["errors"].append({"row": row, "error": f"whiff reference: {exc}"})
            continue

        by_cell: Dict[tuple, List[dict]] = {}
        for row in move_rows:
            key = (row.get("variant"), row.get("gap_walk_frames"), row.get("gap_px"),
                   row.get("rig_guard_state"))
            by_cell.setdefault(key, []).append(row)

        for (variant, gwf, gap_px, rgs), cell_rows in sorted(
            by_cell.items(), key=lambda kv: (kv[0][1] if kv[0][1] is not None else -1)
        ):
            try:
                arena = _arena_for_row(cell_rows[0], ladder_prefix=ladder_prefix, arena_dir=arena_dir)
                measure_hit = rgs in ("held+none", "none")
                measure_block = rgs in ("held+none", "held")
                cell = measure_hitstop_cell(
                    session, rig=_rig(arena), spec=spec, gap_px=gap_px,
                    contact_read=contact_read, observables=observables,
                    sample_fn=sample_fns[attacker_port], latencies=latencies,
                    reference=reference, measure_hit=measure_hit,
                    measure_block=measure_block, max_search=max_search,
                    window_margin=window_margin,
                )
            except HitstopError as exc:
                for row in cell_rows:
                    report["errors"].append({"row": row, "error": str(exc)})
                continue

            value = cell.stored
            if value is None:
                for row in cell_rows:
                    report["skipped"].append(
                        {**row, "skip_reason": "measured but produced no value (NULL propagated)"}
                    )
                continue

            for row in cell_rows:
                if not dry_run:
                    store.update(row["id"], {"hitstop": value})
                report["updated"].append({
                    "id": row["id"], "char": char, "move": move_name, "variant": variant,
                    "gap_px": gap_px, "observable": row["observable"], "hitstop": value,
                    "on_hit": cell.on_hit.hitstop if cell.on_hit else None,
                    "on_block": cell.on_block.hitstop if cell.on_block else None,
                    "hit_eq_block": cell.agrees,
                })

    return report


def main() -> None:  # pragma: no cover - the live-rig path
    """Drive a headless session and UPDATE `hitstop` for one character's
    already-measured, eligible rows. Never point this at port 4025
    (CLAUDE.md: the user's session).

        python -m shadow_train.framelab.hitstop \\
            --url http://127.0.0.1:4077/mcp --game library/mk2 \\
            --db shadow/framelab/frames.db --char reptile \\
            --whiff-arena shadow/arenas/mk2/gap-0.state --ladder-prefix ""
    """
    import argparse
    import json as _json

    from shadow_train import profile as game_profile
    from shadow_train.mcpclient import McpClient

    from . import observables as obs

    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--url", default="http://127.0.0.1:4077/mcp")
    ap.add_argument("--game", default="library/mk2")
    ap.add_argument("--db", default="shadow/framelab/frames.db")
    ap.add_argument("--char", required=True)
    ap.add_argument("--whiff-arena", required=True)
    ap.add_argument("--ladder-prefix", default="")
    ap.add_argument("--arena-dir", default="shadow/arenas/mk2")
    ap.add_argument("--max-search", type=int, default=DEFAULT_MAX_SEARCH)
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    if "4025" in args.url:
        raise SystemExit("refusing to run against port 4025 -- the user's live session")

    prof = game_profile.load(args.game)
    flspec = FramelabSpec.from_profile(prof)
    f1 = obs.resolve_fighter(prof, "block1", 0)
    f2 = obs.resolve_fighter(prof, "block2", 1)
    sample_fns = {0: obs.make_sampler(f1, flspec), 1: obs.make_sampler(f2, flspec)}
    contact_read = obs.make_contact_read_from_spec(f2, flspec)
    observables = flspec.default_observable_names()

    client = McpClient(args.url)
    session = LabSession(client, verify_fn=obs.make_arena_verifier(prof, expect={}))
    session.enforce_preconditions()

    with FrameStore(args.db) as store:
        report = update_store_for_character(
            session, store=store, prof=prof, flspec=flspec, char=args.char,
            whiff_arena=args.whiff_arena, ladder_prefix=args.ladder_prefix,
            arena_dir=args.arena_dir, observables=observables, sample_fns=sample_fns,
            contact_read=contact_read, max_search=args.max_search, dry_run=args.dry_run,
        )

    print(_json.dumps(report, indent=2, default=str))
    print(
        f"updated={len(report['updated'])} skipped={len(report['skipped'])} "
        f"errors={len(report['errors'])} steps={session.steps_taken} "
        f"loads={session.loads_done}"
    )


if __name__ == "__main__":  # pragma: no cover
    main()
