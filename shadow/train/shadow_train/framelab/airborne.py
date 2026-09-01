"""docs/frames.md applied to an AIRBORNE ATTACKER — the neutral jump punch,
and the retraction of §10's "jumping normals are unmeasurable".

`kit.py` measures normals from the ground; `specials.py` measures moves that
charge, cross up, or knock down. Both assume the attacker's clock starts at
contact and runs on the ground. A jumping normal breaks that in one specific
way, and §10 has said three times that the break is fatal:

    "the observable is a WALK and an airborne fighter cannot walk; speed does
     not help, it needs a different observable."

**That claim is FALSE on this port, and the evidence that refutes it predates
it.** `specials.py` measured Mileena's `teleport_kick`, which is airborne —
`y` swings +200 (underground) to −131 (above the screen) — and its number came
out clean: `first_true = 28`, read honestly as "the first frame she can walk
again AFTER LANDING". The probe is not blind mid-flight; it is *deferred*,
and a deferred probe still answers a well-posed question. This module
establishes that the same deferral is sound for a jumping normal, and it does
it by MEASUREMENT rather than by analogy, because a teleport and a jump are
not the same object.

## The three things that make a deferred probe honest

1. **No air control.** The whole deferral rests on this. If holding a
   direction while airborne moved the fighter at all, the differential probe
   would diverge MID-FLIGHT and report "actionable" for a fighter who is
   simply drifting — the §4.2 absolute-vs-differential failure in a new
   costume, except that differencing does NOT save you here: the control
   replay does not drift, so a real air-drift is a real difference. So it is
   scanned, not assumed (`air_control_scan`), at every airborne N, in both
   directions, over a window that ENDS BEFORE LANDING so that any divergence
   found is unambiguously mid-air. Measured on Reptile's neutral jump: zero
   divergence in either observable at every airborne N in both directions.
   The same hold asserted after landing diverges normally, which is what
   proves the scan can see anything at all.
2. **The manifest must land after the LANDING.** Even with (1) clean, a
   reported boundary that sits inside the flight is not a recovery — it is
   whatever noise got past the scan. `measure_njp` refuses one
   (`MidAirManifestError`). This is the airborne twin of `specials.py`'s
   preemption gate: a specific, named way for the probe to answer a question
   other than the one asked.
3. **The probe shape is NEW, so it is CALIBRATED, not inherited.** §3.1's
   rule, and the landing transition is exactly the kind of shape that could
   have differed. Its calibration point is derived from THIS run's own
   landing frame rather than from a constant, the way `specials.py` derives a
   knocked-down victim's — a fighter still in the air at the calibration
   point reads residual airtime as injection latency.

## What "advantage" means for a jump-in, stated once

The attacker's number is `landing + k`, not `contact + k`: she cannot walk
until she is standing, whatever her attack was doing. So a jump-in's
advantage is dominated by REMAINING AIRTIME, and that is a property of WHERE
IN THE ARC the punch connected rather than of the move:

    on_hit(J)   = defender_manifest(contact) − attacker_manifest(landing)

Hit shallow (high, early in the descent) and a long fall separates you from
your own recovery; hit deep (just before landing) and you are hugely plus.
The stored rows are therefore keyed by the arc frame the punch was thrown at
(`throw_at`) and carry the measured CONTACT HEIGHT with them, and a table that
prints one scalar for "jump HP" is printing the midpoint of a curve.

## What is deliberately NOT here

  * No new observable. §10 asked for one; the measurement did not need one.
    Both of the port's existing observables (`struct_velocity`, `pointer_x`)
    work unchanged, and agree.
  * No `active`/`hitstop` column. The whiff boundary of a J-sweep BRACKETS the
    active window arithmetically (see `WhiffBoundary`), and this module
    reports that bracket — but it does not store it: `active.py` measures the
    hitbox window directly by teleporting the defender, and a derived bracket
    must not be shipped in the same column as a measured value.
  * No claim about a jump-in with horizontal movement. A NEUTRAL jump was
    chosen precisely because the gap is constant through the whole arc, which
    removes the side-swap and gap-key discontinuities that made the teleport
    and the roll awkward. A forward jump re-introduces them and is a
    different measurement.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import (Any, Callable, Dict, Hashable, List, Mapping, Optional,
                    Sequence, Tuple)

from .probe import (MoveScript, ProbeError, Rig, ScriptStep,
                    _cluster_first_contact, replay)
from .session import LabSession
from .specials import (CrossObservableError, MoveObservation, SideManifest,
                       Signature, advantage_between, calibrate_for_move,
                       check_signature, observe_move, special_row, sweep_side,
                       walk_directions_after)

__all__ = [
    "JUMP_HOLD_FRAMES",
    "AIRBORNE_ATTACKER_CONVENTION",
    "AirborneError",
    "NotAirborneError",
    "AirControlError",
    "MidAirManifestError",
    "JumpArc",
    "measure_jump_arc",
    "njp_script",
    "AirControlEvidence",
    "air_control_scan",
    "WhiffBoundary",
    "throw_scan",
    "whiff_boundary",
    "AirborneMeasurement",
    "measure_njp",
    "njp_row",
    "curve_first_active_frame",
    "main",
]

# Frames the jump direction must be HELD for the jump to happen at all.
# Measured (Reptile, `shadow/arenas/mk2/gap-60.state`): hold=1 leaves `y` at
# its resting value for 70 frames — no jump, and a script built on it would
# have measured a STANDING punch under a jumping name. hold=2, 3 and 6 all
# produce the identical arc, so 2 is the threshold and not a tuning knob.
# It is a constant here rather than profile data for the same reason
# `specials.STEP_GAP` is: it is a property of the game's input parser, and
# `measure_jump_arc` REFUSES a hold that does not leave the ground, so a port
# where 2 is wrong fails loudly instead of measuring the wrong move.
JUMP_HOLD_FRAMES = 2

# What an airborne attacker's stored advantage MEANS, written down because it
# is not the same quantity a grounded move's is (docs/frames.md §4.3's rule:
# "state the convention"). Mileena's teleport row carries the same one.
AIRBORNE_ATTACKER_CONVENTION = (
    "the attacker is AIRBORNE at contact, so her side of the advantage is the "
    "first frame she can walk again AFTER LANDING (act-again probe, same "
    "observable and window as a grounded row); the defender's side is "
    "contact-relative as usual. The difference is a real advantage number and "
    "it is dominated by the attacker's REMAINING AIRTIME at contact."
)


class AirborneError(ProbeError):
    """An airborne measurement is not a number yet."""


class NotAirborneError(AirborneError):
    """The fighter never left its own resting `y` — so whatever was measured,
    it was not a jumping move. Refused rather than recorded, because a
    standing punch measured under a jumping name is exactly the §4.3 failure
    ("a move must be identified by its measured SIGNATURE, not by the buttons
    pressed") with the buttons being a direction instead of an attack."""


class AirControlError(AirborneError):
    """A held direction moved the fighter, or its observable, WHILE AIRBORNE.

    That kills the deferred probe: the act-again sweep would report TRUE at
    an airborne N for a fighter who is drifting rather than acting, and
    differencing against the control does NOT cancel it (the control is not
    drifting). §10's "it needs a different observable" would be right after
    all, and this exception is where that conclusion would be reached — with
    the specific N and direction that produced it, so the next attempt starts
    from evidence instead of from the claim."""


class MidAirManifestError(AirborneError):
    """The sweep's boundary sits BEFORE the fighter landed. Whatever diverged
    there, it was not a fighter regaining control on the ground, and the
    number would silently be an airtime rather than a recovery."""


# ── the arc ───────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class JumpArc:
    """One neutral jump, traced. Every frame number is replay-relative
    (frame 0 = the loaded arena, before any step).

    `resting_y` is MEASURED on this rig, never a constant: docs/frames.md §10
    — "there is no scalar GROUND_Y for arcade; resting y is character- AND
    stage-dependent (85/83 on one stage, 89/87 on another)". Everything else
    in this dataclass is derived relative to it.

    `x_drift_px` is what makes the jump NEUTRAL rather than merely vertical:
    if the fighter drifts, the gap is not constant through the arc and every
    gap-keyed number in the run is keyed to the wrong gap. It is measured over
    the AIRBORNE window only, and `settle_px` carries what was deliberately
    excluded from it — the movement between the loaded frame and take-off.

    That split is not fussiness; measuring it as one number gets the answer
    wrong. Measured on `gap-30.state`, the attacker's `x` steps 546 -> 549 on
    the FIRST frame after the load and is then constant for all 38 airborne
    frames: 3 px of ARENA SETTLE (§5's "the gap oscillates inside the floor;
    settle before saving") and 0 px of jump. Folded together it reads as a
    3 px drift and impugns the jump.
    """

    resting_y: int
    takeoff: int          # first frame y differs from resting_y
    landing: int          # first frame y is back at resting_y for good
    apex_y: int
    apex_frames: Tuple[int, ...]
    x_drift_px: Optional[int]
    settle_px: Optional[int] = None
    y_trace: Tuple[Optional[int], ...] = field(repr=False, default=())

    @property
    def airtime(self) -> int:
        return self.landing - self.takeoff

    @property
    def airborne_frames(self) -> Tuple[int, ...]:
        return tuple(range(self.takeoff, self.landing))

    def height_above_rest(self, frame: int) -> Optional[int]:
        """How far above its own resting `y` the fighter is at `frame`.
        Positive is HIGHER (the raw field is signed and inverted — §5: "SIGNED;
        smaller = higher"), 0 is standing. NULL where `y` was unreadable."""
        if frame >= len(self.y_trace):
            return None
        y = self.y_trace[frame]
        return None if y is None else self.resting_y - int(y)


def measure_jump_arc(
    session: LabSession,
    *,
    rig: Rig,
    port: int,
    y_read: Callable[[LabSession], Optional[int]],
    x_read: Optional[Callable[[LabSession], Optional[int]]] = None,
    jump_hold: int = JUMP_HOLD_FRAMES,
    jump_button: str = "up",
    total_frames: int = 80,
) -> JumpArc:
    """Trace one neutral jump and return its `JumpArc`.

    This is a PRECONDITION of every other function here, not diagnostics. The
    landing frame it produces is what the calibration point is derived from,
    what `air_control_scan` bounds its comparison windows by, and what
    `measure_njp` checks the attacker's manifest against — none of which can
    be constants, because the arc is per character and the resting `y` it is
    measured against is per character AND per stage.

    Refuses (`NotAirborneError`) a hold that does not leave the ground: a
    1-frame `up` measured on Reptile produced 70 frames of resting `y`, and a
    script built on it would have thrown a STANDING punch under a jumping
    name.
    """
    if port != rig.attacker_port:
        # `probe.replay`'s schedule drives the ATTACKER port; jumping any other
        # port would need its own schedule, which nothing here wants yet.
        raise ValueError(
            f"measure_jump_arc drives the rig's attacker port "
            f"({rig.attacker_port}), not port {port}"
        )
    script = MoveScript(
        name="neutral_jump",
        steps=(ScriptStep(frames=jump_hold, buttons=(jump_button,)),),
    )

    def sample(s: LabSession) -> Mapping[str, Hashable]:
        return {"y": y_read(s), "x": None if x_read is None else x_read(s)}

    trace = replay(
        session, rig=rig, script=script, total_frames=total_frames,
        defender_guard=False, sample_fn=sample,
    )
    ys = [t["y"] for t in trace]
    rest = ys[0]
    if rest is None:
        raise AirborneError(
            "the attacker's `y` did not resolve on the loaded arena (object "
            "pointer out of range or a stale cid) -- docs/frames.md §5: a "
            "mismatch means the row must be discarded, not recorded."
        )
    rest = int(rest)
    off = [i for i, y in enumerate(ys) if y is not None and int(y) != rest]
    if not off:
        raise NotAirborneError(
            f"holding {jump_button!r} for {jump_hold} frame(s) never moved the "
            f"attacker off its resting y={rest} across {total_frames} frames. "
            "That is not a jump, and an attack scripted on top of it would be a "
            "GROUNDED normal recorded under a jumping name (docs/frames.md "
            f"§4.3). Measured threshold on MK2 arcade: {JUMP_HOLD_FRAMES}."
        )
    takeoff, landing = off[0], off[-1] + 1
    airborne = [ys[i] for i in range(takeoff, landing) if ys[i] is not None]
    apex_y = min(int(y) for y in airborne)
    apex_frames = tuple(
        i for i in range(takeoff, landing)
        if ys[i] is not None and int(ys[i]) == apex_y
    )
    drift = settle = None
    if x_read is not None:
        air = [t["x"] for t in trace[takeoff:landing + 1] if t["x"] is not None]
        if air:
            drift = max(int(x) for x in air) - min(int(x) for x in air)
        pre = [t["x"] for t in trace[: takeoff + 1] if t["x"] is not None]
        if pre:
            settle = max(int(x) for x in pre) - min(int(x) for x in pre)
    return JumpArc(
        resting_y=rest, takeoff=takeoff, landing=landing, apex_y=apex_y,
        apex_frames=apex_frames, x_drift_px=drift, settle_px=settle,
        y_trace=tuple(ys),
    )


# ── the script ────────────────────────────────────────────────────────────


def njp_script(
    *,
    throw_at: int,
    buttons: Sequence[str],
    jump_hold: int = JUMP_HOLD_FRAMES,
    jump_button: str = "up",
    hold_frames: int = 2,
    name: Optional[str] = None,
) -> MoveScript:
    """A NEUTRAL jump with one attack button thrown at arc frame `throw_at`.

    The jump is `lead_in`, the punch is the move. That split is not cosmetic:
    `MoveScript.attack_input_frame` becomes `throw_at`, which is what
    `first_active_frame` is measured relative to (§4.4), and the lead-in is
    replayed identically in probe and control so the whole flight cancels out
    of the differential exactly like a walk-in does.

    `throw_at` is the frame the BUTTON is first asserted, counted from the
    jump input — not the frame of the arc the fighter is at, which lags it by
    the pre-jump frames (`JumpArc.takeoff`). The two are one subtraction
    apart and the report prints both.
    """
    if throw_at < jump_hold:
        raise ValueError(
            f"throw_at={throw_at} is inside the {jump_hold}-frame jump input: "
            "the punch would be CHORDED with the jump direction, which on this "
            "port does not produce the move (MACRO_ACTIONS §11's same-frame "
            "chord rule). Throw it at or after frame "
            f"{jump_hold}."
        )
    lead = [ScriptStep(frames=jump_hold, buttons=(jump_button,))]
    if throw_at > jump_hold:
        lead.append(ScriptStep(frames=throw_at - jump_hold, buttons=()))
    return MoveScript(
        name=name or f"nj@{throw_at}",
        steps=(ScriptStep(frames=hold_frames, buttons=tuple(buttons)),),
        lead_in=tuple(lead),
    )


# ── (1) the air-control scan — what licenses the whole approach ───────────


@dataclass(frozen=True)
class AirControlEvidence:
    """Whether a held direction does ANYTHING observable while the fighter is
    airborne. `samples` is `(frame, direction, observable, first_divergence)`
    with `first_divergence` NULL for "nothing, within the window".

    `windows` records the per-frame comparison window, which SHRINKS towards
    landing by construction: a window that ran past the landing frame would
    catch the fighter's legitimate post-landing walk and report it as air
    control. Every window here ends at or before `landing`, so a divergence
    inside one is unambiguously mid-air.
    """

    airborne_frames: Tuple[int, ...]
    directions: Tuple[str, ...]
    observables: Tuple[str, ...]
    samples: Tuple[Tuple[int, str, str, Optional[int]], ...]
    windows: Mapping[int, int]
    ground_control: Tuple[Tuple[int, str, str, Optional[int]], ...] = ()

    @property
    def clean(self) -> bool:
        return all(first is None for _, _, _, first in self.samples)

    @property
    def divergences(self) -> Tuple[Tuple[int, str, str, Optional[int]], ...]:
        return tuple(s for s in self.samples if s[3] is not None)

    @property
    def sensitive(self) -> Optional[bool]:
        """Did the SAME scan detect anything at the ground control frames? A
        clean airborne scan means nothing if the scan cannot see a divergence
        it should see -- that is the §4.2 liveness-probe mistake (an absolute
        test reporting a CPU-driven port as live) with the polarity flipped.
        NULL when no control frames were scanned."""
        if not self.ground_control:
            return None
        return any(first is not None for _, _, _, first in self.ground_control)


def air_control_scan(
    session: LabSession,
    *,
    rig: Rig,
    script: MoveScript,
    arc: JumpArc,
    port: int,
    observables: Sequence[str],
    sample_fn,
    directions: Sequence[str] = ("left", "right"),
    frames: Optional[Sequence[int]] = None,
    max_window: int = 12,
    landing: Optional[int] = None,
    ground_control_frames: Sequence[int] = (),
    defender_guard: bool = False,
    raise_on_divergence: bool = True,
) -> AirControlEvidence:
    """Hold each direction at each AIRBORNE frame and difference against an
    identical no-input replay. The result licenses (or kills) every number
    this module produces.

    Why it is not optional, restated as the failure it prevents: the act-again
    sweep starts at contact and walks forward, and for a jump-in the fighter is
    still in the air for the first several N. If a held direction drifted her,
    the sweep's FIRST TRUE would be that drift — a monotone, plausible,
    cross-observable-agreeing predicate reporting a recovery that has not
    happened. Neither §4.2's differencing nor §8.4's cross-method check can see
    it, because both observables would move and the control genuinely does not.

    Two properties make the scan's `clean` mean something:

      * **Every window ends at or before `arc.landing`** (`min(max_window,
        landing − n)`), so nothing it sees can be a post-landing walk.
      * **`ground_control_frames`** run the identical scan AFTER landing, where
        the answer must be a divergence. A scan that finds nothing anywhere is
        a broken scan, not a clean one, and `AirControlEvidence.sensitive`
        reports which of the two it was.
    """
    ground = arc.landing if landing is None else int(landing)
    airborne = tuple(frames) if frames is not None else tuple(
        range(arc.takeoff, ground)
    )
    obs = tuple(observables)
    samples: List[Tuple[int, str, str, Optional[int]]] = []
    control: List[Tuple[int, str, str, Optional[int]]] = []
    windows: Dict[int, int] = {}

    def scan_one(n: int, w: int, into: List) -> None:
        windows[n] = w
        for d in directions:
            probe = replay(
                session, rig=rig, script=script, total_frames=n + w,
                defender_guard=defender_guard, probe_port=port,
                probe_buttons=(d,), probe_at=n, sample_fn=sample_fn,
                sample_from=n + 1,
            )
            ctl = replay(
                session, rig=rig, script=script, total_frames=n + w,
                defender_guard=defender_guard, probe_port=port,
                probe_buttons=(), probe_at=n, sample_fn=sample_fn,
                sample_from=n + 1,
            )
            for o in obs:
                first = next(
                    (i for i in range(1, w + 1) if probe[n + i][o] != ctl[n + i][o]),
                    None,
                )
                into.append((n, d, o, first))

    for n in airborne:
        w = min(max_window, ground - n)
        if w < 1:
            continue  # no room for a comparison that is still mid-air
        scan_one(n, w, samples)
    for n in ground_control_frames:
        scan_one(n, max_window, control)

    ev = AirControlEvidence(
        airborne_frames=tuple(n for n in airborne if windows.get(n)),
        directions=tuple(directions), observables=obs,
        samples=tuple(samples), windows=windows,
        ground_control=tuple(control),
    )
    if raise_on_divergence and not ev.clean:
        raise AirControlError(
            f"a held direction diverged from the no-input control WHILE "
            f"AIRBORNE: {list(ev.divergences)[:4]} (frame, direction, "
            "observable, first divergence). The act-again probe cannot be "
            "deferred to landing on this port -- an airborne N would report "
            "TRUE for a fighter that is drifting rather than acting, and "
            "differencing does not cancel it because the control is not "
            "drifting. docs/frames.md §10's 'it needs a different observable' "
            "stands for this move."
        )
    return ev


# ── (2) where the punch connects in the arc ───────────────────────────────


@dataclass(frozen=True)
class WhiffBoundary:
    """The J-sweep's connect map, and the ACTIVE-window bracket it implies.

    A jumping normal has two independent reasons not to connect, and telling
    them apart is the whole value of scanning J rather than picking one:

      * thrown too LATE — the startup has not finished before the fighter
        lands (`contact = throw_at + first_active_frame` in this regime);
      * thrown too EARLY — the hitbox's active window EXPIRES before the arc
        brings the fighter down to the height at which the boxes overlap.

    The second one brackets `active` arithmetically. Let `G` be the first frame
    at which the geometry overlaps (constant for a given arc and gap — measured
    identical at 62/72/110 px, which is what a downward-angled air normal
    against a standing hurtbox looks like) and `F` the measured
    `first_active_frame`. A throw at `J` connects iff `J + F <= G <= J + F +
    active − 1`, so the LAST whiffing J below the connecting band gives
    `active <= G − J_whiff − F + 1` and the first connecting J gives
    `active >= G − J_connect − F + 1`.

    This class REPORTS that bracket and this module does not store it.
    `active.py` measures the hitbox window directly (by teleporting the
    defender to a target gap on a chosen frame); a value derived from a
    boundary must not be shipped in the same column as one that was measured,
    and §7's provenance rule is what makes that distinction matter.
    """

    throw_frames: Tuple[int, ...]
    contact: Mapping[int, Optional[int]]
    damage: Mapping[int, Optional[int]]
    contact_height: Mapping[int, Optional[int]]
    geometry_frame: Optional[int]
    first_active_frame: Optional[int]
    active_lo: Optional[int] = None
    active_hi: Optional[int] = None

    @property
    def connecting(self) -> Tuple[int, ...]:
        return tuple(j for j in self.throw_frames if self.contact.get(j) is not None)


def throw_scan(
    session: LabSession,
    *,
    rig: Rig,
    arc: JumpArc,
    buttons: Sequence[str],
    throw_frames: Sequence[int],
    contact_read: Callable[[LabSession], Hashable],
    attacker_y_read: Callable[[LabSession], Optional[int]],
    total_frames: int = 110,
    defender_guard: bool = False,
    jump_hold: int = JUMP_HOLD_FRAMES,
    hold_frames: int = 2,
) -> Dict[int, Tuple[Optional[int], Optional[int], Optional[int]]]:
    """One replay per `throw_at`: `{J: (contact_frame, damage, y_at_contact)}`.

    The cheap first pass, exactly as `kit.scan_contact` is for the ladder — one
    replay per cell instead of the ~200 a full cell costs. It is what locates
    the connecting band of the arc, and a J outside that band is a WHIFF, which
    is a RESULT (§1.1) and not a gap in the table.
    """
    out: Dict[int, Tuple[Optional[int], Optional[int], Optional[int]]] = {}
    for j in throw_frames:
        script = njp_script(throw_at=j, buttons=buttons, jump_hold=jump_hold,
                            hold_frames=hold_frames, name=f"nj@{j}")
        trace = replay(
            session, rig=rig, script=script, total_frames=total_frames,
            defender_guard=defender_guard,
            sample_fn=lambda s: {"c": contact_read(s), "y": attacker_y_read(s)},
        )
        vals = [t["c"] for t in trace]
        contacts = [i for i in range(1, len(vals)) if vals[i] != vals[i - 1]]
        if not contacts:
            out[j] = (None, None, None)
            continue
        dmg = None
        if isinstance(vals[0], int):
            dmg = int(vals[0]) - int(vals[contacts[-1]])
        y = trace[contacts[0]]["y"]
        height = None if y is None else arc.resting_y - int(y)
        out[j] = (contacts[0], dmg, height)
    return out


def whiff_boundary(
    scan: Mapping[int, Tuple[Optional[int], Optional[int], Optional[int]]],
) -> WhiffBoundary:
    """Turn a `throw_scan` into the connect map plus the two derived numbers
    it supports: `first_active_frame` and the `active` BRACKET.

    `first_active_frame` is `min(contact − throw_at)` over the connecting
    throws, and it is only claimed when at least TWO different throws achieve
    that minimum — one J hitting at `J + 9` could be a coincidence of the
    geometry; two consecutive ones tracking `J + 9` is a startup. That is the
    §4.4 spirit ("stored only where the measurement is not contaminated")
    applied to the contaminant this move actually has, which is the geometric
    window rather than travel.
    """
    js = tuple(sorted(scan))
    contact = {j: scan[j][0] for j in js}
    damage = {j: scan[j][1] for j in js}
    height = {j: scan[j][2] for j in js}
    connecting = [j for j in js if contact[j] is not None]
    faf: Optional[int] = None
    geometry: Optional[int] = None
    lo = hi = None
    if connecting:
        deltas = {j: int(contact[j]) - j for j in connecting}
        best = min(deltas.values())
        if sum(1 for v in deltas.values() if v == best) >= 2:
            faf = best
        # The geometric window: the earliest contact frame seen at all. In the
        # startup-limited regime contact tracks J; in the geometry-limited one
        # it is pinned, and the pinned value IS the first overlap frame.
        pinned = [int(contact[j]) for j in connecting]
        geometry = min(pinned)
        if faf is not None:
            first_connect = min(connecting)
            whiffs_below = [j for j in js if j < first_connect and contact[j] is None]
            lo = geometry - first_connect - faf + 1
            if whiffs_below:
                hi = geometry - max(whiffs_below) - faf
    return WhiffBoundary(
        throw_frames=js, contact=contact, damage=damage, contact_height=height,
        geometry_frame=geometry, first_active_frame=faf,
        active_lo=lo, active_hi=hi,
    )


# ── (3) one cell ──────────────────────────────────────────────────────────


@dataclass
class AirborneMeasurement:
    """Everything one (arena, throw_at) cell produced — including what was
    REFUSED, which for a first airborne normal is most of the point."""

    move: str
    arena: str
    throw_at: int
    gap_px: Optional[int] = None
    gap_walk_frames: Optional[int] = None
    arc: Optional[JumpArc] = None
    air_control: Optional[AirControlEvidence] = None
    obs_hit: Optional[MoveObservation] = None
    obs_block: Optional[MoveObservation] = None
    signature_problems: Tuple[str, ...] = ()
    contact_hit: Optional[int] = None
    contact_block: Optional[int] = None
    hits: Optional[int] = None
    contact_height: Optional[int] = None
    landing: Dict[str, Optional[int]] = field(default_factory=dict)
    latencies: Dict[str, Dict[str, int]] = field(default_factory=dict)
    cal_points: Dict[str, Tuple[int, int]] = field(default_factory=dict)
    manifests: Dict[str, Dict[str, SideManifest]] = field(default_factory=dict)
    on_hit: Dict[str, Optional[int]] = field(default_factory=dict)
    on_block: Dict[str, Optional[int]] = field(default_factory=dict)
    notes: List[str] = field(default_factory=list)

    @property
    def remaining_airtime(self) -> Optional[int]:
        """Frames from contact to the attacker's landing — the quantity a
        jump-in's advantage is actually a function of."""
        land = self.landing.get("hit")
        if land is None or self.contact_hit is None:
            return None
        return land - self.contact_hit

    def agreed(self, table: Mapping[str, Optional[int]]) -> Optional[int]:
        """The one value every observable agreed on, or a refusal (§8.4)."""
        vals = {o: v for o, v in table.items() if v is not None}
        if not vals:
            return None
        if len(set(vals.values())) > 1:
            raise CrossObservableError(
                f"{self.move}@{self.throw_at}: observables disagree ({vals}). "
                "docs/frames.md §8.4 makes cross-method agreement REQUIRED and "
                "§7 forbids splitting the difference -- no row is written."
            )
        return next(iter(vals.values()))


def _landing_of(obs: MoveObservation, who: str) -> Optional[int]:
    """The frame this fighter's `y` is back at its own resting value for good.

    `MoveObservation.*_airborne_until` is exactly that frame (0 when the
    fighter never left the ground), derived from the fighter's OWN pre-move
    resting `y` — §10 forbids a scalar GROUND_Y here.
    """
    return (
        obs.attacker_airborne_until if who == "attacker"
        else obs.victim_airborne_until
    )


def measure_njp(
    session: LabSession,
    *,
    rig: Rig,
    script: MoveScript,
    arc: JumpArc,
    observables: Sequence[str],
    sample_fns: Mapping[int, Any],
    contact_read: Callable[[LabSession], Hashable],
    reads: Mapping[str, Callable[[LabSession], Optional[int]]],
    expect: Signature = Signature(),
    air_control: Optional[AirControlEvidence] = None,
    observe_frames: int = 120,
    attacker_max_search: int = 60,
    defender_max_search: int = 60,
    measure_block: bool = True,
    window_margin: int = 2,
    cal_at_n: int = 70,
    cal_margin: int = 40,
    cal_confirm_gap: int = 30,
) -> AirborneMeasurement:
    """The whole §4 protocol for one jumping normal at one arc frame.

    Order, and why each step is where it is:

      1. Observe once on hit. This is the run every later decision is derived
         from — the contact anchor, the damage signature, the attacker's own
         LANDING frame (which moves: contact freezes both fighters, so a
         connecting jump lands later than a whiffing one by the hitstop), and
         §1.1's knockdown gate on the victim.
      2. Refuse a signature mismatch and refuse a whiff, as results.
      3. Require the air-control evidence to be CLEAN. Passed in rather than
         re-run per cell: it is a property of the arc and the port, and it
         costs ~150 replays.
      4. Calibrate the probe's own shape, at two points DERIVED FROM THIS
         RUN'S LANDING rather than from a constant. §3.1's rule is that the
         calibration point must be hold-limited; for an airborne attacker "far
         enough past contact" is not a constant, because the fighter is still
         in the air.
      5. Sweep both sides exhaustively (`specials.sweep_side` brings the
         non-monotone and silent-cap refusals with it), then apply the gate
         that is new here: the attacker's manifest must be at or after her
         landing frame.
    """
    m = AirborneMeasurement(move=script.name, arena=rig.arena,
                            throw_at=script.attack_input_frame, arc=arc)

    m.obs_hit = observe_move(
        session, rig=rig, script=script, total_frames=observe_frames,
        defender_guard=False, contact_read=contact_read,
        attacker_x_read=reads["attacker_x"], attacker_y_read=reads["attacker_y"],
        victim_x_read=reads["victim_x"], victim_y_read=reads["victim_y"],
    )
    m.gap_px = m.obs_hit.gap_px
    if not m.obs_hit.connected:
        m.notes.append(
            f"{script.name}: the contact signal never fired -- the punch was "
            "thrown too early (its active window expires before the arc brings "
            "her into range) or too late (she lands first). A whiff has no "
            "advantage number (docs/frames.md §1.1). Not a failed run."
        )
        return m
    m.signature_problems = check_signature(m.obs_hit, expect)
    if m.signature_problems:
        m.notes.append(
            f"{script.name}: SIGNATURE MISMATCH {list(m.signature_problems)} -- "
            "docs/frames.md §4.3: a move is identified by its measured "
            "signature, not by the buttons pressed. No row written."
        )
        return m

    m.contact_hit, m.hits, _ = _cluster_first_contact(
        list(m.obs_hit.contacts), rig.quiet_frames
    )
    m.landing["hit"] = _landing_of(m.obs_hit, "attacker")
    y_at_contact = m.obs_hit.trace[m.contact_hit]["ay"]
    m.contact_height = (
        None if y_at_contact is None else arc.resting_y - int(y_at_contact)
    )
    if m.landing["hit"] is None or m.landing["hit"] <= m.contact_hit:
        # Not fatal by itself (a punch can connect on the landing frame), but
        # it means the attacker was NOT airborne at contact and the row is a
        # grounded normal wearing a jump's name.
        m.notes.append(
            f"{script.name}: the attacker is back at her resting y by frame "
            f"{m.landing['hit']}, at or before contact (f{m.contact_hit}) -- "
            "this cell is a GROUNDED contact at the end of a jump, not a "
            "jump-in. Recorded, with contact_height "
            f"{m.contact_height}, and it is the deep end of the curve."
        )

    if air_control is None:
        raise AirborneError(
            "measure_njp refuses to run without air-control evidence for this "
            "arc: an airborne N in the sweep would report a DRIFT as a "
            "recovery, and neither differencing (§4.2) nor cross-observable "
            "agreement (§8.4) can see that. Run `air_control_scan` once per "
            "arc and pass its evidence in."
        )
    if not air_control.clean:
        raise AirControlError(
            f"{script.name}: the air-control scan for this arc found "
            f"{len(air_control.divergences)} mid-air divergence(s); no "
            "deferred-probe number may be reported from it."
        )
    if air_control.sensitive is False:
        raise AirborneError(
            f"{script.name}: the air-control scan found nothing airborne AND "
            "nothing at its post-landing control frames, so it is not "
            "sensitive enough to have found anything at all. A clean scan that "
            "cannot see a divergence it should see is the §4.2 liveness-probe "
            "mistake with the polarity flipped."
        )
    m.air_control = air_control

    if measure_block:
        m.obs_block = observe_move(
            session, rig=rig, script=script, total_frames=observe_frames,
            defender_guard=True, contact_read=contact_read,
            attacker_x_read=reads["attacker_x"], attacker_y_read=reads["attacker_y"],
            victim_x_read=reads["victim_x"], victim_y_read=reads["victim_y"],
        )
        if m.obs_block.connected:
            m.contact_block = _cluster_first_contact(
                list(m.obs_block.contacts), rig.quiet_frames
            )[0]
            m.landing["block"] = _landing_of(m.obs_block, "attacker")
        else:
            m.notes.append(
                f"{script.name}: the guarded rig saw no contact at all -- no "
                "on_block number (a whiff, not a block)."
            )

    passes: List[Tuple[bool, int, Optional[MoveObservation]]] = [
        (False, m.contact_hit, m.obs_hit)
    ]
    if m.contact_block is not None:
        passes.append((True, m.contact_block, m.obs_block))

    for guard, anchor, obs_for in passes:
        tag = "block" if guard else "hit"
        assert obs_for is not None
        att_dirs = walk_directions_after(obs_for.attacker_x[1], obs_for.victim_x[1])
        def_dirs = walk_directions_after(obs_for.victim_x[1], obs_for.attacker_x[1])
        landing = _landing_of(obs_for, "attacker") or 0
        for who, port, dirs, max_s in (
            ("attacker", rig.attacker_port, att_dirs, attacker_max_search),
            ("defender", rig.defender_port, def_dirs, defender_max_search),
        ):
            shape = f"{who}/{tag}"
            # §3.1's calibration point must be HOLD-limited, and for an
            # AIRBORNE attacker "far enough past contact" is not a constant:
            # she is still in the air. Derive the floor from this run's own
            # landing (the victim's, when the victim is the one who left the
            # ground), exactly as specials.py derives a knocked-down victim's.
            floor = 0
            if who == "attacker":
                floor = max(0, landing - anchor)
            else:
                vict = _landing_of(obs_for, "victim") or 0
                floor = max(0, vict - anchor)
            at_n = max(cal_at_n, floor + cal_margin)
            confirm_n = at_n + cal_confirm_gap
            m.cal_points[shape] = (at_n, confirm_n)
            m.latencies[shape] = calibrate_for_move(
                session, rig=rig, script=script, port=port, origin=anchor,
                observables=observables, sample_fn=sample_fns[port],
                defender_guard=guard, at_n=at_n, confirm_at_n=confirm_n,
            )
            m.manifests[shape] = sweep_side(
                session, rig=rig, script=script, who=who, port=port,
                origin=anchor, origin_kind="contact", observables=observables,
                sample_fn=sample_fns[port],
                input_latency_frames=m.latencies[shape],
                defender_guard=guard, max_search=max_s,
                window_margin=window_margin, walk_directions=dirs,
            )
            if who == "attacker":
                for o, sm in m.manifests[shape].items():
                    if sm.manifest is not None and sm.manifest < landing:
                        raise MidAirManifestError(
                            f"{script.name} {shape}/{o}: the sweep's manifest "
                            f"is replay frame {sm.manifest}, BEFORE the "
                            f"attacker lands at f{landing}. A fighter cannot "
                            "walk in the air, so that boundary is not a "
                            "recovery -- it is whatever the air-control scan "
                            "did not catch. No row."
                        )

    # §8.4, applied to the quantity that is comparable across observables
    # (`first_true`, with the observable's manifestation margin removed --
    # see probe.py's algebra).
    for shape, per in m.manifests.items():
        seen = {o: sm.sweep.first_true for o, sm in per.items()}
        if len(set(seen.values())) > 1:
            raise CrossObservableError(
                f"{script.name} {shape}: the two independent observables "
                f"disagree about first_true ({seen}). docs/frames.md §8.4 "
                "makes agreement REQUIRED and §7 forbids splitting the "
                "difference -- no row is written."
            )

    for o in observables:
        m.on_hit[o] = advantage_between(
            m.manifests["attacker/hit"][o], m.manifests["defender/hit"][o]
        )
        if "attacker/block" in m.manifests:
            m.on_block[o] = advantage_between(
                m.manifests["attacker/block"][o], m.manifests["defender/block"][o]
            )
    m.notes.append(
        f"{script.name}: attacker AIRBORNE at contact "
        f"(height {m.contact_height} above her own resting y="
        f"{arc.resting_y}), lands at f{m.landing.get('hit')}, "
        f"{m.remaining_airtime} frames of airtime remaining at contact. "
        + AIRBORNE_ATTACKER_CONVENTION
    )
    return m


# ── rows ──────────────────────────────────────────────────────────────────


def njp_row(
    m: AirborneMeasurement,
    *,
    family: str,
    port: str,
    char: str,
    move: str,
    core_id: str,
    rom_id: str,
    observable: str,
    first_active_frame: Optional[int] = None,
    connect_range: Optional[int] = None,
    gap_walk_frames: Optional[int] = None,
    sample_n: Optional[int] = None,
    confidence: str = "high",
) -> dict:
    """One `store.FrameStore` row for one airborne cell.

    Built on `specials.special_row` so §1.1's knockdown gate stays ENFORCED in
    exactly one place, then given the two columns that are meaningful here and
    not there: `first_active_frame` (measurable for this move, because contact
    tracks the throw frame in the startup-limited regime — §4.4's "minimum
    reproducible gap" rule does not bind, since contact is gap-INDEPENDENT for
    this move) and `connect_range`.

    `variant` carries the arc frame AND the measured contact height, because
    two rows of this move at the same gap are genuinely different measurements
    and §5 forbids averaging them into one.
    """
    row = special_row(
        family=family, port=port, char=char, move=move,
        core_id=core_id, rom_id=rom_id, observable=observable,
        method="linear_sweep",
        input_latency_frames=m.latencies["attacker/hit"][observable],
        obs_hit=m.obs_hit, on_hit=m.on_hit.get(observable),
        on_block=m.on_block.get(observable),
        gap_px=m.gap_px, gap_walk_frames=gap_walk_frames,
        variant=f"throw@{m.throw_at}/h{m.contact_height}",
        sample_n=sample_n, confidence=confidence,
    )
    row["first_active_frame"] = first_active_frame
    row["connect_range"] = connect_range
    return row


def curve_first_active_frame(
    cells: Sequence[AirborneMeasurement],
) -> Optional[int]:
    """`min(contact − throw_at)` across a curve, claimed only when at least two
    different throw frames achieve it (see `whiff_boundary` for the argument).

    Returns NULL rather than a guess — §2.5, and §4.4's rule that FAF is a
    separate measurement rather than a by-product of the act-again probe. What
    makes it available at all here is that the anchor already IS input-relative
    for this move: the punch's own input frame is `MoveScript.
    attack_input_frame`, and the contact anchor is measured in the same clock.
    """
    deltas = [
        c.contact_hit - c.throw_at
        for c in cells
        if c.contact_hit is not None
    ]
    if not deltas:
        return None
    best = min(deltas)
    return best if deltas.count(best) >= 2 else None


# ── operator entry point ──────────────────────────────────────────────────


def main() -> None:  # pragma: no cover - the live-rig path
    """Measure a NEUTRAL JUMP normal across the arc. Never point this at port
    4025 (CLAUDE.md: the user's session).

        python -m shadow_train.framelab.airborne \\
            --url http://127.0.0.1:4080/mcp --game library/mk2 \\
            --core ../FBNeo/.../fbneo_libretro.dylib --rom ~/games/roms/mk2.zip \\
            --arena shadow/arenas/mk2/gap-60.state --char reptile --button HP \\
            --scan 14:41 --throw 16 --throw 31 --throw 35 \\
            --report tmp/njp.json
    """
    import argparse
    import json
    from pathlib import Path

    from shadow_train import profile as game_profile
    from shadow_train.mcpclient import McpClient

    from . import observables as obsmod
    from .identity import compute_core_id, compute_rom_id
    from .spec import FramelabSpec
    from .store import FrameStore

    ap = argparse.ArgumentParser(description="framelab: airborne-normal measurement")
    ap.add_argument("--url", default="http://127.0.0.1:4080/mcp")
    ap.add_argument("--game", default="library/mk2")
    ap.add_argument("--core", required=True)
    ap.add_argument("--rom", required=True)
    ap.add_argument("--arena", action="append", default=[], required=True)
    ap.add_argument("--char", default="reptile")
    ap.add_argument("--button", default="HP")
    ap.add_argument("--move", default=None, help="stored move name (default nj<BUTTON>)")
    ap.add_argument("--scan", default=None, metavar="LO:HI",
                    help="throw-frame connect scan over range(LO, HI)")
    ap.add_argument("--throw", action="append", type=int, default=[],
                    help="arc frame(s) to fully measure -- repeatable")
    ap.add_argument("--air-control", action="store_true",
                    help="run the mid-air divergence scan (required to measure)")
    ap.add_argument("--connect-range", type=int, default=None,
                    help="§5's bracket: the largest CONNECTING rung's gap in "
                         "px, measured elsewhere in the ladder. Stored as-is; "
                         "never inferred from the arenas this run happened to "
                         "visit.")
    ap.add_argument("--db", default=None)
    ap.add_argument("--report", default=None)
    ap.add_argument("--sample-n", type=int, default=1)
    args = ap.parse_args()

    prof = game_profile.load(args.game)
    flspec = FramelabSpec.from_profile(prof)
    observables = list(flspec.default_observable_names())
    f_att = obsmod.resolve_fighter(prof, "block1", 0)
    f_def = obsmod.resolve_fighter(prof, "block2", 1)
    sample_fns = {0: obsmod.make_sampler(f_att, flspec),
                  1: obsmod.make_sampler(f_def, flspec)}
    contact_read = obsmod.make_contact_read_from_spec(f_def, flspec)

    def signed(read):
        def r(s):
            v = read(s)
            return None if v is None else (v - 65536 if v >= 32768 else v)
        return r

    reads = {
        "attacker_x": obsmod.make_pointer_field_read(f_att, "x"),
        "attacker_y": signed(obsmod.make_pointer_field_read(f_att, "y")),
        "victim_x": obsmod.make_pointer_field_read(f_def, "x"),
        "victim_y": signed(obsmod.make_pointer_field_read(f_def, "y")),
    }
    buttons = tuple(prof.attack_chords[args.button])
    move_name = args.move or f"nj{args.button}"

    session = LabSession(client=McpClient(args.url),
                         verify_fn=obsmod.make_arena_verifier(prof, expect={}))
    session.enforce_preconditions()
    core_id, rom_id = compute_core_id(args.core), compute_rom_id(args.rom)
    rig_wdbp = flspec.rig.walk_directions_by_port if flspec.rig else None
    report: Dict[str, Any] = {
        "move": move_name, "core_id": core_id, "rom_id": rom_id,
        "observables": observables, "convention": AIRBORNE_ATTACKER_CONVENTION,
        "arenas": [], "rows": [],
    }

    for arena in args.arena:
        rig = Rig(arena=arena, attacker_port=0, defender_port=1,
                  guard_buttons=tuple(prof.attack_chords["Block"]),
                  walk_directions_by_port=dict(rig_wdbp or {}),
                  quiet_frames=flspec.quiet_frames)
        arc = measure_jump_arc(session, rig=rig, port=0,
                               y_read=reads["attacker_y"], x_read=reads["attacker_x"])
        entry: Dict[str, Any] = {
            "arena": arena,
            "arc": {"resting_y": arc.resting_y, "takeoff": arc.takeoff,
                    "landing": arc.landing, "airtime": arc.airtime,
                    "apex_y": arc.apex_y, "apex_frames": list(arc.apex_frames),
                    "x_drift_px": arc.x_drift_px, "settle_px": arc.settle_px},
            "cells": [],
        }
        print(f"{arena}: resting_y={arc.resting_y} takeoff=f{arc.takeoff} "
              f"landing=f{arc.landing} airtime={arc.airtime} apex={arc.apex_y} "
              f"x_drift={arc.x_drift_px}px settle={arc.settle_px}px")

        boundary = None
        if args.scan:
            lo, hi = (int(v) for v in args.scan.split(":"))
            scan = throw_scan(session, rig=rig, arc=arc, buttons=buttons,
                              throw_frames=range(lo, hi),
                              contact_read=contact_read,
                              attacker_y_read=reads["attacker_y"])
            boundary = whiff_boundary(scan)
            entry["scan"] = {
                "contact": {str(k): v for k, v in boundary.contact.items()},
                "damage": {str(k): v for k, v in boundary.damage.items()},
                "contact_height": {str(k): v
                                   for k, v in boundary.contact_height.items()},
                "geometry_frame": boundary.geometry_frame,
                "first_active_frame": boundary.first_active_frame,
                "active_bracket": [boundary.active_lo, boundary.active_hi],
            }
            print(f"  connect band J={boundary.connecting[:1]}..."
                  f"{boundary.connecting[-1:]} FAF={boundary.first_active_frame} "
                  f"geometry=f{boundary.geometry_frame} "
                  f"active in [{boundary.active_lo},{boundary.active_hi}]")

        air = None
        if args.air_control and args.throw:
            probe_script = njp_script(throw_at=args.throw[0], buttons=buttons,
                                      name=f"{move_name}@{args.throw[0]}")
            air = air_control_scan(
                session, rig=rig, script=probe_script, arc=arc, port=0,
                observables=observables, sample_fn=sample_fns[0],
                # Far enough past the arc's own landing that a CONNECTING run
                # (whose flight is stretched by contact hitstop) is grounded
                # there too -- a control frame that is still airborne would
                # read as "the scan is blind" rather than "the fighter is
                # airborne", which is the opposite conclusion.
                ground_control_frames=(arc.landing + 20,),
                raise_on_divergence=False,
            )
            entry["air_control"] = {
                "frames": list(air.airborne_frames),
                "windows": {str(k): v for k, v in air.windows.items()},
                "clean": air.clean, "sensitive": air.sensitive,
                "divergences": [list(d) for d in air.divergences],
                "ground_control": [list(d) for d in air.ground_control],
            }
            print(f"  air control: clean={air.clean} sensitive={air.sensitive} "
                  f"over {len(air.airborne_frames)} airborne frames "
                  f"x {len(air.directions)} directions x {len(observables)} obs")
            if not air.clean:
                print(f"  DIVERGENCES: {air.divergences}")

        cells: List[AirborneMeasurement] = []
        for j in args.throw:
            script = njp_script(throw_at=j, buttons=buttons, name=f"{move_name}@{j}")
            m = measure_njp(session, rig=rig, script=script, arc=arc,
                            observables=observables, sample_fns=sample_fns,
                            contact_read=contact_read, reads=reads,
                            air_control=air)
            cells.append(m)
            for note in m.notes:
                print("  NOTE:", note)
            print(f"  J={j} gap={m.gap_px}px contact={m.contact_hit}/"
                  f"{m.contact_block} h={m.contact_height} land={m.landing} "
                  f"air_left={m.remaining_airtime} "
                  f"dmg={m.obs_hit.damage if m.obs_hit else None} "
                  f"on_hit={m.on_hit} on_block={m.on_block}")
            entry["cells"].append({
                "throw_at": j, "gap_px": m.gap_px,
                "contact_hit": m.contact_hit, "contact_block": m.contact_block,
                "contact_height": m.contact_height,
                "landing": m.landing, "remaining_airtime": m.remaining_airtime,
                "hits": m.hits,
                "damage": m.obs_hit.damage if m.obs_hit else None,
                "chip": m.obs_block.damage if m.obs_block else None,
                "knockdown": None if m.obs_hit is None else m.obs_hit.knockdown,
                "signature_problems": list(m.signature_problems),
                "latencies": m.latencies,
                "cal_points": {k: list(v) for k, v in m.cal_points.items()},
                "manifests": {
                    shape: {o: {"origin": sm.origin, "first_true": sm.sweep.first_true,
                                "window": sm.sweep.window, "manifest": sm.manifest,
                                "direction": sm.sweep.direction,
                                "monotone": sm.sweep.monotone,
                                "predicate": "".join("T" if v else "F"
                                                     for v in sm.sweep.predicate)}
                          for o, sm in per.items()}
                    for shape, per in m.manifests.items()},
                "on_hit": m.on_hit, "on_block": m.on_block,
                "notes": m.notes,
            })

        faf = (boundary.first_active_frame if boundary is not None
               else curve_first_active_frame(cells))
        for m in cells:
            if m.obs_hit is None or not m.obs_hit.connected or m.signature_problems:
                continue
            startup_limited = (
                faf is not None and m.contact_hit is not None
                and m.contact_hit - m.throw_at == faf
            )
            for o in observables:
                if m.on_hit.get(o) is None and m.on_block.get(o) is None:
                    continue
                report["rows"].append(njp_row(
                    m, family=prof.family, port=prof.port, char=args.char,
                    move=move_name, core_id=core_id, rom_id=rom_id,
                    observable=o,
                    first_active_frame=faf if startup_limited else None,
                    connect_range=args.connect_range,
                    sample_n=args.sample_n,
                ))
        entry["first_active_frame"] = faf
        report["arenas"].append(entry)

    if args.report:
        Path(args.report).parent.mkdir(parents=True, exist_ok=True)
        Path(args.report).write_text(json.dumps(report, indent=1, default=str))
        print("report ->", args.report)
    if args.db and report["rows"]:
        # The store is written CONCURRENTLY by other characters' runs, so the
        # transaction is opened as late as possible, held for as short as
        # possible, and a transient `database is locked` is retried rather
        # than voiding a measured run (`store.update`'s own rule, applied to
        # the insert path).
        import sqlite3
        import time as _time
        Path(args.db).parent.mkdir(parents=True, exist_ok=True)
        for attempt in range(8):
            try:
                with FrameStore(args.db) as store:
                    for row in report["rows"]:
                        store.insert(row)
                break
            except sqlite3.OperationalError as exc:
                if "locked" not in str(exc).lower() or attempt == 7:
                    raise
                _time.sleep(0.05 * (attempt + 1))
        print(f"wrote {len(report['rows'])} rows to {args.db}")
    print(f"steps={session.steps_taken} loads={session.loads_done}")


if __name__ == "__main__":  # pragma: no cover
    main()
