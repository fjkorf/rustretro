"""docs/frames.md, applied to SPECIAL moves — and the three places the
contract does not reach.

`kit.py` measures normals: one button, one frame, one gap, the attacker and
the defender both standing on the ground on the same sides they started on.
Every one of those is an assumption, and Mileena breaks a different one per
move (`library/mk2/mk2.md`, "What will complicate measuring Mileena later"):

  1. **`sai_throw` is a CHARGE move and a PROJECTILE.** The input that starts
     it is 34 frames before the input that fires it, and contact is another
     15-24 frames after THAT — so the anchor is nowhere near the press, and
     "the attacker's clock" and "the defender's clock" no longer start at the
     same instant. §4.3's `advantage = manifest(defender, contact) −
     manifest(attacker, contact)` sweeps both sides from ONE origin, which
     silently assumes the attacker cannot already have recovered when contact
     happens. For a projectile that assumption is exactly what is being
     measured. `SideManifest` gives each side its own ORIGIN and differences
     the two ABSOLUTE manifest frames instead (§4.3's "difference the raw
     MANIFEST frames", with the origin made explicit rather than assumed).
  2. **`teleport_kick` goes AIRBORNE** — `y` +200 (underground) to −131
     (above the screen). The act-again probe is a WALK, and she cannot walk
     underground, so during flight the probe answers FALSE for a reason that
     is not stun. The number that comes out is therefore "the first frame she
     can WALK AGAIN AFTER LANDING", which is a legitimate advantage number
     with a different meaning, and its probe shape needs its own §3.1
     calibration rather than the grounded one (`calibrate_for_move`).
  3. **`roll` SWAPS SIDES and KNOCKS DOWN.** The gap key's sign flips
     mid-move (MACRO_ACTIONS §10.2), so the probe's walk direction must be
     re-derived from the POST-move positions (`walk_directions_after`), and
     §1.1/§4.3's knockdown gate means `on_hit` is NULL with `knockdown` set —
     the wakeup window is the measurement (`WAKEUP_WINDOW_CONVENTION`).

## The new hazard specials introduce, which normals cannot have

**A probe input can CANCEL the move it is measuring.** §4.3 already requires
a move to be identified by its measured SIGNATURE rather than by the buttons
pressed; that rule is about the SCRIPT. For a charge/release move the probe
frame can land INSIDE the move, and then the walk that is supposed to observe
the attacker's recovery instead preempts the attack. Measured live, and it is
not subtle once looked for: for the sai, a walk asserted on the exact release
frame produces **0 damage** (both directions, 4/4) while the same walk one
frame later produces the full 23 — and the sweep reported `actionable(0) =
TRUE`, a perfectly plausible T-then-F-then-F predicate that says the attacker
is free on the release frame. She is not; there was simply no sai.

`preemption_scan` finds those N by their signature (the contact anchor), and
`sweep_side` REFUSES to report a boundary that sits inside the preempted
range. §7's "no silent caps" applied to the probe itself.

## What this module deliberately does NOT do

  * It does not gap-key anything across a move that moves the fighter
    (§5 is discontinuous across the teleport and the roll: `x` jumps 945→1013
    in one frame, and the roll ends 265 px away on the other side). The
    ladder for `sai_throw` is built by WALKING IN before the charge, inside
    one replay, so gap is a property of the rung and every rung's gap is read
    at the frame the move's own input starts.
  * It does not invent a `first_active_frame` (§4.4: not a by-product of this
    probe), and it does not report an `on_hit` for a knockdown (§1.1).
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import (Any, Callable, Dict, Hashable, List, Mapping, Optional,
                    Sequence, Tuple)

from .probe import (MoveScript, ProbeError, Rig, ScriptStep, SweepResult,
                    _cluster_first_contact, calibrate_probe_latency, replay,
                    sweep_actionable)
from .session import LabSession

__all__ = [
    "STEP_GAP",
    "SPECIAL_STEP_KEYS",
    "WAKEUP_WINDOW_CONVENTION",
    "SpecialEncodingError",
    "PreemptedProbeError",
    "special_encoding",
    "special_script",
    "facing_from_x",
    "walk_directions_after",
    "MoveObservation",
    "Signature",
    "observe_move",
    "check_signature",
    "preemption_scan",
    "SideManifest",
    "advantage_between",
    "sweep_side",
    "ChargePersistence",
    "charge_persistence",
    "charge_release_tolerance",
    "SpecialMeasurement",
    "calibrate_for_move",
    "measure_special",
    "special_row",
    "main",
]

# MACRO_ACTIONS §11, measured: "the inter-step gap must be >= 2 frames -- every
# motion special fails at gap 1 (the taps are not seen as separate) and at gap
# 0 (they are one continuous hold). STEP_GAP = 2 sits exactly on that
# boundary." This is a property of the GAME's input parser, not a tunable.
STEP_GAP = 2

# The §10.1 step vocabulary. `hold`/`min_frames`/`release`/`while_held` are the
# charge and release kinds; `dirs`/`press`/`frames` are §2's original three.
SPECIAL_STEP_KEYS = frozenset(
    {"dirs", "press", "frames", "hold", "min_frames", "release", "while_held"}
)

# What this module means by a wakeup window, stated once because §1.1 reserves
# the column without defining the number: frames from the CONTACT anchor to the
# first frame the knocked-down victim's own walk manifests, measured by the
# identical differential probe an advantage row uses. It is NOT an advantage
# (there is no attacker clock in it) and must never be stored in `on_hit`.
WAKEUP_WINDOW_CONVENTION = (
    "frames from contact to the victim's first manifest walk (act-again probe, "
    "same observable and window as an advantage row); NOT an advantage"
)


class SpecialEncodingError(ProbeError):
    """A `special_inputs` entry uses a step kind this module cannot execute.
    Raised rather than skipped: silently dropping a step turns a special into
    a normal, which is exactly how `acid_spit` came to be 'verified'."""


class PreemptedProbeError(ProbeError):
    """The act-again probe CANCELLED the move it was measuring — the sweep's
    boundary sits inside the range where the attack never came out, so the
    'divergence' is the probe's own walk, not a recovery."""


# ── encodings -> a replayable script ──────────────────────────────────────


def special_encoding(profile: Any, char: str, move: str) -> Tuple[dict, ...]:
    """This port's RAW `special_inputs[char][move]` step list.

    Read from `profile.port_raw`, deliberately NOT from `profile.
    special_inputs` / `macro_steps_for`: the Python profile loader compiles a
    step down to `{dirs, press, frames}` and DROPS the §10.1 kinds, so
    Mileena's `sai_throw` (`hold HP min_frames 34` then `release HP`) compiles
    to two steps that hold nothing. A charge move read through the compiled
    view is silently a no-op — see this module's report in
    `library/mk2/mk2.md`.
    """
    raw = (getattr(profile, "port_raw", None) or {}).get("special_inputs") or {}
    steps = (raw.get(char) or {}).get(move)
    if not steps:
        raise SpecialEncodingError(
            f"this port's profile has no special_inputs[{char!r}][{move!r}] "
            "(MACRO_ACTIONS §2). Omission is meaningful: a port simply offers "
            "less, and there is no cross-port default to fall back on."
        )
    return tuple(steps)


def _resolve_dir(name: str, facing: str) -> str:
    """Semantic direction -> a pad direction, resolved against the facing
    PINNED AT MACRO START (MACRO_ACTIONS §10.2: "semantic directions resolve
    against the facing at the frame the macro STARTED ... otherwise a teleport
    retroactively invalidates its own inputs"). `up`/`down` are absolute."""
    if name in ("up", "down"):
        return name
    if facing not in ("left", "right"):
        raise SpecialEncodingError(
            f"cannot resolve semantic direction {name!r}: facing is {facing!r}. "
            "docs/frames.md §5: where position is unmeasurable the facing is "
            "NULL, never a guess."
        )
    if name == "forward":
        return facing
    if name == "back":
        return "left" if facing == "right" else "right"
    raise SpecialEncodingError(f"unknown semantic direction {name!r}")


def special_script(
    profile: Any,
    char: str,
    move: str,
    *,
    facing: str,
    lead_in: Sequence[ScriptStep] = (),
    step_gap: int = STEP_GAP,
    name: Optional[str] = None,
) -> MoveScript:
    """One port-profile `special_inputs` encoding, as a `MoveScript` that
    `probe.replay` can execute — the Python twin of `src/macros.rs::MacroExec`
    playback: each step's mask held for its `frames`, `step_gap` NEUTRAL frames
    between steps.

    The neutral gap is not padding. `F · F+HP` produces 0 damage in all 16
    configurations tried; the same taps separated by >= 2 neutral frames
    produce the move every time (MACRO_ACTIONS §11) — a direction chorded with
    its trigger on the SAME frame does not register on this port.

    Step kinds (MACRO_ACTIONS §2 + §10.1):
      * `dirs` / `press` / `frames` — hold that mask for `frames` frames.
      * `hold` + `min_frames` — a CHARGE: hold the chord for `min_frames`.
      * `release` — the falling edge. It is not a held mask, so it emits no
        step of its own: the schedule's own trailing release does it, and the
        release FRAME is `MoveScript.total_frames` (which is why the sai's
        attacker sweep can use it as an origin).
      * `while_held` — a chord held ACROSS this step (Reptile's `[BLK] U U D`).

    A step naming a kind not in `SPECIAL_STEP_KEYS`, or a `release` that is not
    the final step, raises rather than being dropped.
    """
    chords: Mapping[str, Sequence[str]] = getattr(profile, "attack_chords", {}) or {}
    encoded = special_encoding(profile, char, move)
    steps: List[ScriptStep] = []
    for i, raw in enumerate(encoded):
        unknown = set(raw) - SPECIAL_STEP_KEYS
        if unknown:
            raise SpecialEncodingError(
                f"{char}/{move} step {i} uses unknown key(s) {sorted(unknown)} "
                f"(known: {sorted(SPECIAL_STEP_KEYS)})"
            )
        if "release" in raw:
            if i != len(encoded) - 1:
                raise SpecialEncodingError(
                    f"{char}/{move} step {i} is a `release` that is not the "
                    "final step. A non-terminal release (Reptile's "
                    "invisibility) needs the chord that follows it to be "
                    "modelled explicitly; this module refuses to guess."
                )
            continue  # the schedule releases everything after the last step
        buttons: List[str] = []
        for d in raw.get("dirs", ()):
            buttons.append(_resolve_dir(d, facing))
        for cls in tuple(raw.get("press", ())) + tuple(raw.get("hold", ())) + tuple(
            raw.get("while_held", ())
        ):
            if cls not in chords:
                raise SpecialEncodingError(
                    f"{char}/{move} step {i} names attack class {cls!r}, which "
                    f"is not in this port's attack_chords ({sorted(chords)})"
                )
            buttons.extend(chords[cls])
        frames = int(raw.get("min_frames", raw.get("frames", 3)))
        if frames < 1:
            raise SpecialEncodingError(f"{char}/{move} step {i} has frames={frames}")
        if steps:
            steps.append(ScriptStep(frames=step_gap, buttons=()))
        steps.append(ScriptStep(frames=frames, buttons=tuple(buttons)))
    if not steps:
        raise SpecialEncodingError(f"{char}/{move} encodes no executable step")
    return MoveScript(
        name=name or move, steps=tuple(steps), lead_in=tuple(lead_in)
    )


# ── facing, and the direction the probe may walk ──────────────────────────


def facing_from_x(me_x: Optional[int], opp_x: Optional[int]) -> Optional[str]:
    """docs/frames.md §5: "On a port with no verified facing field, facing is
    DERIVED from relative position (sign of opp.x − me.x) and the sidecar must
    say it was derived, not read." MK2 arcade has no confirmed facing byte
    (`0xBE81` reads constant through a crossover; `obj+0x18` does not flip).

    NULL — never a guess — when either position is unreadable, and NULL on an
    exact tie, which is a real state during a crossover and is not a facing."""
    if me_x is None or opp_x is None or me_x == opp_x:
        return None
    return "right" if opp_x > me_x else "left"


def walk_directions_after(
    me_x: Optional[int], opp_x: Optional[int]
) -> Tuple[str, ...]:
    """§4.2's blocked-direction hazard, re-derived from the positions the
    fighters are ACTUALLY at when the probe runs — which for a side-swapping
    move is not the side they started on.

    Returns the preference order AWAY-from-the-opponent first: a fighter
    cannot walk into the opponent's body, and at contact range that direction
    is dead, so trying it first would read as "not actionable". Falls back to
    both directions in a fixed order when the facing is NULL, because the
    sweep tries every candidate anyway — it just costs a direction's worth of
    replays.
    """
    facing = facing_from_x(me_x, opp_x)
    if facing is None:
        return ("left", "right")
    return ("left", "right") if facing == "right" else ("right", "left")


# ── observing a move, and validating it BY SIGNATURE ──────────────────────


@dataclass(frozen=True)
class MoveObservation:
    """One replay, watched frame by frame: what the victim's damage register
    did, where both fighters were, and whether either left its own resting
    `y`.

    `*_airborne_until` is derived from that fighter's OWN pre-move resting
    `y` (docs/frames.md §10: "there is no scalar GROUND_Y for arcade —
    resting y is character- AND stage-dependent"), and it is what §1.1's
    knockdown gate reads.
    """

    move: str
    contacts: Tuple[int, ...]
    contact_values: Tuple[int, ...]
    damage: Optional[int]
    attacker_x: Tuple[Optional[int], Optional[int]]   # (at move input, at end)
    victim_x: Tuple[Optional[int], Optional[int]]
    gap_px: Optional[int]
    attacker_airborne_until: Optional[int]
    victim_airborne_until: Optional[int]
    crossed: Optional[bool]
    facing_before: Optional[str]
    facing_after: Optional[str]
    trace: Tuple[Mapping[str, Any], ...] = field(repr=False, default=())

    @property
    def connected(self) -> bool:
        return bool(self.contacts)

    @property
    def knockdown(self) -> Optional[bool]:
        if self.victim_airborne_until is None:
            return None
        return self.victim_airborne_until > 0


def observe_move(
    session: LabSession,
    *,
    rig: Rig,
    script: MoveScript,
    total_frames: int,
    defender_guard: bool,
    contact_read: Callable[[LabSession], Hashable],
    attacker_x_read: Callable[[LabSession], Optional[int]],
    attacker_y_read: Callable[[LabSession], Optional[int]],
    victim_x_read: Callable[[LabSession], Optional[int]],
    victim_y_read: Callable[[LabSession], Optional[int]],
) -> MoveObservation:
    """Replay the move once and record everything a signature needs.

    One composite sampler rather than `kit.scan_contact`'s two replays: the
    reason `kit` splits them is that folding `y` into the ANCHOR's value would
    make every pixel of a knockdown arc look like another hit. Here the
    contact value is kept as its own key and the clustering is done on that key
    alone, so one replay is enough and the positional trace is exactly
    contemporaneous with the damage — which matters for a side swap, where
    "which side was she on when this hit landed" is part of the answer.
    """
    def sample(s: LabSession) -> Mapping[str, Any]:
        return {
            "c": contact_read(s),
            "ax": attacker_x_read(s), "ay": attacker_y_read(s),
            "vx": victim_x_read(s), "vy": victim_y_read(s),
        }

    trace = replay(
        session, rig=rig, script=script, total_frames=total_frames,
        defender_guard=defender_guard, sample_fn=sample,
    )
    vals = [t["c"] for t in trace]
    contacts = tuple(i for i in range(1, len(vals)) if vals[i] != vals[i - 1])
    damage = None
    if contacts and isinstance(vals[0], int):
        damage = int(vals[0]) - int(vals[contacts[-1]])

    def airborne_until(key: str) -> Optional[int]:
        seq = [t[key] for t in trace]
        rest = seq[0]
        if rest is None:
            return None
        off = [i for i, y in enumerate(seq) if y is not None and y != rest]
        return (off[-1] + 1) if off else 0

    at_input = trace[script.attack_input_frame]
    end = trace[-1]
    gap = None
    if at_input["ax"] is not None and at_input["vx"] is not None:
        gap = abs(int(at_input["vx"]) - int(at_input["ax"]))
    f_before = facing_from_x(at_input["ax"], at_input["vx"])
    f_after = facing_from_x(end["ax"], end["vx"])
    crossed = None if (f_before is None or f_after is None) else f_before != f_after

    return MoveObservation(
        move=script.name,
        contacts=contacts,
        contact_values=tuple(vals[i] for i in contacts),
        damage=damage,
        attacker_x=(at_input["ax"], end["ax"]),
        victim_x=(at_input["vx"], end["vx"]),
        gap_px=gap,
        attacker_airborne_until=airborne_until("ay"),
        victim_airborne_until=airborne_until("vy"),
        crossed=crossed,
        facing_before=f_before,
        facing_after=f_after,
        trace=tuple(trace),
    )


@dataclass(frozen=True)
class Signature:
    """What a move must LOOK like to be recorded under its own name. Every
    field is optional; only the ones given are checked, and each was chosen
    because it discriminates this move from what it degenerates into."""

    damage: Optional[int] = None
    hits: Optional[int] = None
    crossed: Optional[bool] = None
    victim_knockdown: Optional[bool] = None
    min_attacker_travel_px: Optional[int] = None


def check_signature(obs: MoveObservation, expect: Signature) -> Tuple[str, ...]:
    """Every way `obs` fails to be the move `expect` describes. Empty tuple ==
    validated.

    §4.3: "a move must be identified by its measured SIGNATURE, not by the
    buttons pressed." On MK2 `block+0xC0` fires identically (160 -> 192) for
    the roll and for the crouching normal a FAILED roll degenerates into
    (mk2.md, task M1), so only damage, travel and the victim's `y` can tell
    them apart — which is why this returns a LIST of specific mismatches
    rather than a boolean.

    It never decides what a mismatch means: for the sai's ladder, "damage 34,
    2 contacts" is not a broken run, it is the measurement that locates the
    charge's own HP normal. The caller reports it."""
    bad: List[str] = []
    if expect.damage is not None and obs.damage != expect.damage:
        bad.append(f"damage {obs.damage} != {expect.damage}")
    if expect.hits is not None and len(obs.contacts) != expect.hits:
        bad.append(f"{len(obs.contacts)} contact(s) != {expect.hits}")
    if expect.crossed is not None and obs.crossed != expect.crossed:
        bad.append(f"crossed={obs.crossed} != {expect.crossed}")
    if expect.victim_knockdown is not None and obs.knockdown != expect.victim_knockdown:
        bad.append(f"victim knockdown={obs.knockdown} != {expect.victim_knockdown}")
    if expect.min_attacker_travel_px is not None:
        x0, x1 = obs.attacker_x
        if x0 is None or x1 is None:
            bad.append("attacker travel unreadable (object pointer did not resolve)")
        elif abs(int(x1) - int(x0)) < expect.min_attacker_travel_px:
            bad.append(
                f"attacker travelled {abs(int(x1) - int(x0))}px < "
                f"{expect.min_attacker_travel_px}px"
            )
    return tuple(bad)


# ── the probe that cancels the move it measures ───────────────────────────


def preemption_scan(
    session: LabSession,
    *,
    rig: Rig,
    script: MoveScript,
    origin: int,
    n_range: Sequence[int],
    directions: Sequence[str],
    contact_read: Callable[[LabSession], Hashable],
    tail_frames: int,
    port: Optional[int] = None,
) -> Dict[int, bool]:
    """`{N: did the move still come out}` for a walk probe asserted at
    `origin + N` on the attacker's port.

    This has no analogue in `kit.py` and it is not optional here. For a normal
    the probe starts AT contact — the move is over, and a walk cannot un-throw
    it. For a charge/release move the probe frame can land INSIDE the move:
    measured on Mileena's sai, a walk asserted on the exact release frame
    gives **0 damage in both directions**, while the same walk one frame later
    gives the full 23. The act-again sweep saw that as `actionable(0) = TRUE`
    — a plausible predicate reporting that the attacker is free on the release
    frame, when in fact there was no attack at all.

    A move is "still out" iff the contact signal fires at all in that run;
    that is the same anchor §4.1 uses, so this scan needs no new instrument.
    """
    port = rig.attacker_port if port is None else port
    out: Dict[int, bool] = {}
    for n in n_range:
        fired = False
        for d in directions:
            trace = replay(
                session, rig=rig, script=script,
                total_frames=origin + n + tail_frames, defender_guard=False,
                probe_port=port, probe_buttons=(d,), probe_at=origin + n,
                sample_fn=lambda s: {"c": contact_read(s)},
            )
            vals = [t["c"] for t in trace]
            if any(vals[i] != vals[i - 1] for i in range(1, len(vals))):
                fired = True
                break
        out[n] = fired
    return out


# ── per-side manifests, each with its OWN origin ──────────────────────────


@dataclass(frozen=True)
class SideManifest:
    """One side's act-again result, carrying the ORIGIN its `N` is relative to.

    §4.3 differences the two sides' manifest frames and says nothing about the
    origin because for a normal there is only one candidate: contact. A
    projectile has two — the attacker committed at the RELEASE and the
    defender was hit at CONTACT, tens of frames apart — so the origin becomes
    part of the measurement and is stored with it. Differencing ABSOLUTE
    manifests is the same arithmetic §4.3 prescribes; making the origin
    explicit is what keeps it honest when the two are not the same frame.
    """

    who: str                 # "attacker" | "defender"
    origin: int              # absolute replay frame N is measured from
    origin_kind: str         # "contact" | "release+1" | "input" | ...
    sweep: SweepResult
    excluded_n: Tuple[int, ...] = ()   # N where the probe preempted the move

    @property
    def manifest(self) -> Optional[int]:
        """The absolute replay frame this side's walk first manifests —
        `origin + first_true + window`, the same quantity
        `SweepResult.actionable_after_contact` returns, expressed on the
        replay's own clock so two different origins can be compared. NULL
        when the sweep never diverged (§2.5: absent means absent)."""
        if self.sweep.first_true is None:
            return None
        return self.origin + self.sweep.first_true + self.sweep.window


def advantage_between(
    attacker: SideManifest, defender: SideManifest
) -> Optional[int]:
    """`defender.manifest − attacker.manifest`, both ABSOLUTE frames of the
    same replay clock (§4.3: "Difference the raw MANIFEST frames").

    ## Why the two sides' WINDOWS may differ, and why that is fine

    A manifest is `origin + first_true + window`, and `probe.py`'s own algebra
    makes that equal to `A_abs + m` — the frame the fighter regains control
    plus the OBSERVABLE's manifestation margin. The injection latency `l`
    drops out entirely: a larger `l` inflates the window and depresses
    `first_true` by exactly the same amount. So two sides measured with
    different probe SHAPES (the guarded defender's `l = 10/11` against the
    attacker's `1/2`) still difference correctly, which is precisely what
    `kit.manifest_advantage` exists to say and what its punish rig confirmed
    independently (earliest counter-attack = manifest − 2 on BOTH shapes).

    What must match is the OBSERVABLE, because `m` is the observable's own and
    cancels only against itself — differencing `struct_velocity` against
    `pointer_x` would bake in a 1-frame offset forever. That is refused."""
    if attacker.sweep.observable != defender.sweep.observable:
        raise ProbeError(
            "advantage across two different observables "
            f"({attacker.sweep.observable!r} vs {defender.sweep.observable!r}) "
            "is not a difference of comparable clocks: each observable's "
            "manifestation margin cancels only against itself."
        )
    a, d = attacker.manifest, defender.manifest
    if a is None or d is None:
        return None
    return d - a


def sweep_side(
    session: LabSession,
    *,
    rig: Rig,
    script: MoveScript,
    who: str,
    port: int,
    origin: int,
    origin_kind: str,
    observables: Sequence[str],
    sample_fn,
    input_latency_frames: Mapping[str, int],
    defender_guard: bool,
    max_search: int,
    window_margin: int = 2,
    cap_margin: int = 5,
    excluded_n: Sequence[int] = (),
    walk_directions: Optional[Sequence[str]] = None,
) -> Dict[str, SideManifest]:
    """One side's exhaustive sweep, with the three refusals that make its
    `first_true` a number: non-monotone predicate, a boundary measured against
    the edge of the search (§7's "no silent caps"), and — new here — a
    boundary that lands inside `excluded_n`, where the probe cancelled the
    move (`preemption_scan`).

    Never returns a "never actionable"; a sweep that found nothing comes back
    with `manifest = None` and the caller reports NULL (§4.2/§2.5).
    """
    rig_for_side = rig
    if walk_directions is not None:
        rig_for_side = Rig(
            arena=rig.arena, attacker_port=rig.attacker_port,
            defender_port=rig.defender_port, guard_buttons=rig.guard_buttons,
            walk_directions=rig.walk_directions,
            walk_directions_by_port={**dict(rig.walk_directions_by_port),
                                     port: tuple(walk_directions)},
            quiet_frames=rig.quiet_frames,
        )
    results = sweep_actionable(
        session, rig=rig_for_side, script=script, port=port, anchor=origin,
        observables=list(observables), sample_fn=sample_fn,
        input_latency_frames=dict(input_latency_frames),
        defender_guard=defender_guard, window_margin=window_margin,
        max_search=max_search, exhaustive=True,
    )
    out: Dict[str, SideManifest] = {}
    excluded = tuple(sorted(set(excluded_n)))
    for obs_name, sweep in results.items():
        # Preemption is checked FIRST because it is the more specific
        # diagnosis. The two gates overlap but neither subsumes the other: a
        # preempted TRUE below the real boundary also makes the predicate
        # non-monotone (which is how the sai's N=0 was first caught), but a
        # preempted range that runs contiguously into the actionable range
        # produces a perfectly MONOTONE predicate with a boundary that is
        # still not a recovery.
        if sweep.first_true is not None and sweep.first_true in excluded:
            raise PreemptedProbeError(
                f"{script.name} {who}/{obs_name}: first_true="
                f"{sweep.first_true} is an N at which the probe's own walk "
                "CANCELS the move (no contact signal in that run). The "
                "divergence is the probe walking instead of attacking, not "
                "a recovery. Move the origin past the preempted range."
            )
        if sweep.monotone is False:
            raise ProbeError(
                f"{script.name} {who}/{obs_name}: predicate is not monotone "
                f"({''.join('T' if v else 'F' for v in sweep.predicate)}). On "
                "this port that is a one-frame-early hold or an unsound "
                "observable, not a boundary (kit.py's NonMonotoneError rule)."
            )
        if sweep.first_true is not None:
            if sweep.first_true > sweep.max_search - cap_margin:
                raise ProbeError(
                    f"{script.name} {who}/{obs_name}: first_true="
                    f"{sweep.first_true} is within {cap_margin} of "
                    f"max_search={sweep.max_search} -- the boundary was "
                    "measured against the edge of the search window "
                    "(docs/frames.md §7: no silent caps)."
                )
        out[obs_name] = SideManifest(
            who=who, origin=origin, origin_kind=origin_kind, sweep=sweep,
            excluded_n=excluded,
        )
    return out


# ── the charge, and whether a save state can bank it ──────────────────────


@dataclass(frozen=True)
class ChargePersistence:
    """Whether a CHARGE survives `save_state`/`load_state`, and what it costs
    to reuse one. The question decides the cost model for every charge move in
    every future game, so it is answered by measurement and stored, not
    assumed either way."""

    banked_frames: int
    fresh_threshold_frames: int
    extra_frames_needed: Optional[int]     # NULL if it never fired
    fires_with_no_further_input: bool
    release_tolerance_frames: Optional[int] = None
    note: str = ""

    @property
    def survives(self) -> Optional[bool]:
        """TRUE iff the reloaded state needed FEWER further held frames than a
        cold charge does — i.e. the banked frames were still counted. NULL
        when the move never fired at all, because "it did not fire" and "the
        charge was discarded" are not the same claim (§2.5)."""
        if self.extra_frames_needed is None:
            return None
        return self.extra_frames_needed < self.fresh_threshold_frames

    @property
    def banked_total(self) -> Optional[int]:
        if self.extra_frames_needed is None:
            return None
        return self.banked_frames + self.extra_frames_needed


def charge_release_tolerance(
    session: LabSession,
    *,
    rig: Rig,
    chord: Sequence[str],
    hold_before: int,
    hold_after: int,
    gaps: Sequence[int],
    contact_read: Callable[[LabSession], Hashable],
    tail_frames: int = 90,
) -> Optional[int]:
    """The CONTROL for `charge_persistence`, and it is not optional.

    Split one charge into `hold_before` + G neutral frames + `hold_after`,
    with `hold_before + hold_after` chosen BELOW the fresh threshold, and
    return the largest G that still fires the move (`None` if even G=1 does
    not).

    Without this, "the charge survived a save state" and "this game does not
    reset a charge on a short release" produce the identical reading, and the
    save state gets the credit for a property of the game. Measured on
    Mileena's sai: G=1 fires and G>=2 does not — so the reload is only free
    because `load_state(pause_after=True)` runs ZERO released frames, not
    because save states are magic.
    """
    fired_at: Optional[int] = None
    for g in sorted(set(gaps)):
        script = MoveScript(
            name="charge_release_tolerance",
            steps=(ScriptStep(frames=hold_before, buttons=tuple(chord)),
                   ScriptStep(frames=g, buttons=()),
                   ScriptStep(frames=hold_after, buttons=tuple(chord))),
        )
        trace = replay(
            session, rig=rig, script=script,
            total_frames=script.total_frames + tail_frames, defender_guard=False,
            sample_fn=lambda s: {"c": contact_read(s)},
        )
        vals = [t["c"] for t in trace]
        if any(vals[i] != vals[i - 1] for i in range(1, len(vals))):
            fired_at = g
        else:
            break
    return fired_at


def charge_persistence(
    session: LabSession,
    *,
    arena: str,
    port: int,
    chord: Sequence[str],
    banked_frames: int,
    fresh_threshold_frames: int,
    scratch_path: str,
    candidates: Sequence[int],
    contact_read: Callable[[LabSession], Hashable],
    tail_frames: int = 90,
) -> ChargePersistence:
    """Bank `banked_frames` of charge into a save state, then reload it and
    find the smallest number of FURTHER held frames that still fires the move.

    Read the result together with the no-save-state control (a plain replay
    that releases the chord for k frames mid-charge and resumes): "the charge
    survived a save state" and "this game does not reset a charge on a short
    release" produce the same reading, and only the control separates them.
    `release_tolerance_frames` is where the caller records that control.

    The load must be `load_state(pause_after=True)` with the chord re-asserted
    before any frame runs — which `LabSession` guarantees, because it never
    brackets a load with resume/pause (§4.6). A reload that lets released
    frames execute is measuring a different thing.
    """
    session.load_state(arena)
    session.set_held(port, list(chord))
    session.run_frames(banked_frames)
    session.call("save_state", path=scratch_path)
    session.release_all_ports()

    def fires(extra: int) -> bool:
        session.load_state(scratch_path)
        before = contact_read(session)
        if extra:
            session.set_held(port, list(chord))
            session.run_frames(extra)
        session.set_held(port, [])
        session.run_frames(tail_frames)
        after = contact_read(session)
        session.release_all_ports()
        return after != before

    idle = fires(0)
    need: Optional[int] = None
    for k in sorted(set(candidates)):
        if k and fires(k):
            need = k
            break
    return ChargePersistence(
        banked_frames=banked_frames,
        fresh_threshold_frames=fresh_threshold_frames,
        extra_frames_needed=need,
        fires_with_no_further_input=idle,
        note=(
            "a pre-charged arena is only sound if the reload runs ZERO frames "
            "with the chord released (load_state(pause_after=True) + "
            "hold_buttons before the first frame)"
        ),
    )


# ── one whole special, measured ───────────────────────────────────────────


class CrossObservableError(ProbeError):
    """The two independent observables disagreed about the same number. §8.4
    makes agreement REQUIRED and §7 forbids splitting the difference. (The
    twin of `kit.CrossMethodError`, defined here rather than imported so this
    module does not depend on `kit` at import time.)"""


def calibrate_for_move(
    session: LabSession,
    *,
    rig: Rig,
    script: MoveScript,
    port: int,
    origin: int,
    observables: Sequence[str],
    sample_fn,
    defender_guard: bool,
    at_n: int = 70,
    confirm_at_n: int = 100,
    trials: int = 5,
    max_window: int = 20,
) -> Dict[str, int]:
    """§3.1's calibration, run on THIS move's own probe shape and confirmed
    hold-limited at two points (`kit.calibrate_shapes`' rule, applied to a
    special).

    It is run per MOVE rather than reused from the profile's table because a
    special's probe shape is not obviously one of the four `kit` measured. The
    teleport's attacker probe is the case in point: she is UNDERGROUND, then
    above the screen, then landing, and only then walking — "a wrong-shape
    calibration produced a confident silent 'never actionable' once already"
    (docs/frames.md §3.1). Measuring it costs ~20 replays and either confirms
    the profile's number or refuses the run; assuming it costs nothing and can
    be silently wrong.
    """
    kw = dict(
        rig=rig, script=script, port=port, anchor=origin,
        observables=list(observables), sample_fn=sample_fn,
        defender_guard=defender_guard, trials=trials, max_window=max_window,
    )
    first = calibrate_probe_latency(session, at_n=at_n, **kw)
    second = calibrate_probe_latency(session, at_n=confirm_at_n, **kw)
    if first != second:
        raise ProbeError(
            f"{script.name} port {port} (guard={defender_guard}) is NOT "
            f"hold-limited: latency at origin+{at_n} is {first} but at "
            f"origin+{confirm_at_n} it is {second}. A latency that shrinks as "
            "the probe moves later is residual STUN, not injection latency "
            "(docs/frames.md §3.1). Move the point later; do not average."
        )
    return first


@dataclass
class SpecialMeasurement:
    """Everything one special produced on one rig — including what was
    REFUSED, which for these three moves is most of the interesting part."""

    move: str
    arena: str
    gap_px: Optional[int] = None
    gap_walk_frames: Optional[int] = None
    obs_hit: Optional[MoveObservation] = None
    obs_block: Optional[MoveObservation] = None
    signature_problems: Tuple[str, ...] = ()
    contact_hit: Optional[int] = None
    contact_block: Optional[int] = None
    hits: Optional[int] = None
    excluded_n: Tuple[int, ...] = ()
    latencies: Dict[str, Dict[str, int]] = field(default_factory=dict)
    cal_points: Dict[str, Tuple[int, int]] = field(default_factory=dict)
    manifests: Dict[str, Dict[str, SideManifest]] = field(default_factory=dict)
    on_hit: Dict[str, Optional[int]] = field(default_factory=dict)
    on_block: Dict[str, Optional[int]] = field(default_factory=dict)
    wakeup_window: Dict[str, Optional[int]] = field(default_factory=dict)
    notes: List[str] = field(default_factory=list)

    def agreed(self, table: Mapping[str, Optional[int]]) -> Optional[int]:
        """The one value every observable agreed on, or a refusal. §8.4."""
        vals = {o: v for o, v in table.items() if v is not None}
        if not vals:
            return None
        if len(set(vals.values())) > 1:
            raise CrossObservableError(
                f"{self.move}: observables disagree ({vals}). docs/frames.md "
                "§8.4 makes cross-method agreement REQUIRED and §7 forbids "
                "splitting the difference -- no row is written."
            )
        return next(iter(vals.values()))


def measure_special(
    session: LabSession,
    *,
    rig: Rig,
    script: MoveScript,
    observables: Sequence[str],
    sample_fns: Mapping[int, Any],
    contact_read: Callable[[LabSession], Hashable],
    reads: Mapping[str, Callable[[LabSession], Optional[int]]],
    expect: Signature,
    attacker_origin_kind: str = "contact",
    observe_frames: int = 150,
    attacker_max_search: int = 90,
    defender_max_search: int = 90,
    wakeup_max_search: int = 130,
    measure_block: bool = True,
    window_margin: int = 2,
    cal_at_n: int = 70,
    cal_confirm_at_n: int = 100,
    preemption_tail: int = 90,
) -> SpecialMeasurement:
    """The whole §4 protocol for one special on one rig, with the three
    special-specific gates: signature validation, the preemption scan, and
    §1.1's knockdown gate.

    `attacker_origin_kind` is the thing normals never have to choose:
      * `"contact"` — the attacker committed at or before contact (the
        teleport, the roll). Same origin as the defender, as §4.3 assumes.
      * `"release+1"` — a charge/release move, where the attacker's clock
        starts at the RELEASE and contact is many frames later. The `+1` is
        not a fudge: a probe on the release frame ITSELF cancels the move
        (`preemption_scan`), so the first frame at which "can she walk" is
        even a well-posed question is the one after it.
    """
    m = SpecialMeasurement(move=script.name, arena=rig.arena)

    m.obs_hit = observe_move(
        session, rig=rig, script=script, total_frames=observe_frames,
        defender_guard=False, contact_read=contact_read,
        attacker_x_read=reads["attacker_x"], attacker_y_read=reads["attacker_y"],
        victim_x_read=reads["victim_x"], victim_y_read=reads["victim_y"],
    )
    m.gap_px = m.obs_hit.gap_px
    # A whiff is checked FIRST and reported as itself: run through the
    # signature it would come out as "damage None != 23", which is true and
    # tells the reader the wrong thing. §1.1: a whiff is an OUTCOME.
    if not m.obs_hit.connected:
        m.notes.append(
            f"{script.name}: the contact signal never fired -- a whiff, which "
            "has no advantage number (docs/frames.md §1.1). Not a failed run."
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

    anchor_hit, hits, _ = _cluster_first_contact(
        list(m.obs_hit.contacts), rig.quiet_frames
    )
    m.contact_hit, m.hits = anchor_hit, hits

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
        else:
            m.notes.append(
                f"{script.name}: the guarded rig saw no contact at all -- no "
                "on_block number (a whiff, not a block)."
            )

    origin_att = (
        script.total_frames + 1 if attacker_origin_kind == "release+1" else anchor_hit
    )

    # The probe that cancels the move it measures — only possible where the
    # attacker's probe frames precede contact.
    if origin_att < anchor_hit:
        scan = preemption_scan(
            session, rig=rig, script=script, origin=origin_att,
            n_range=range(0, anchor_hit - origin_att), directions=("left", "right"),
            contact_read=contact_read, tail_frames=preemption_tail,
        )
        m.excluded_n = tuple(n for n, fired in scan.items() if not fired)
        if m.excluded_n:
            m.notes.append(
                f"{script.name}: the walk probe CANCELS the move at N="
                f"{list(m.excluded_n)} (relative to {attacker_origin_kind}); "
                "those N are excluded and a boundary landing on one is refused."
            )

    passes: List[Tuple[bool, int, Optional[int]]] = [(False, anchor_hit, origin_att)]
    if m.contact_block is not None:
        passes.append((True, m.contact_block, None))

    for guard, anchor, att_origin in passes:
        tag = "block" if guard else "hit"
        obs_for_dirs = m.obs_block if guard else m.obs_hit
        assert obs_for_dirs is not None
        att_dirs = walk_directions_after(
            obs_for_dirs.attacker_x[1], obs_for_dirs.victim_x[1]
        )
        def_dirs = walk_directions_after(
            obs_for_dirs.victim_x[1], obs_for_dirs.attacker_x[1]
        )
        origin_a = anchor if att_origin is None else att_origin
        for who, port, origin, kind, dirs, guard_flag, max_s in (
            ("attacker", rig.attacker_port, origin_a,
             attacker_origin_kind if att_origin is not None else "contact",
             att_dirs, guard, attacker_max_search),
            ("defender", rig.defender_port, anchor, "contact", def_dirs, guard,
             wakeup_max_search if (not guard and m.obs_hit.knockdown)
             else defender_max_search),
        ):
            shape = f"{who}/{tag}"
            # §3.1's calibration point must be HOLD-limited, and "far enough
            # past the anchor that the fighter is certainly free" is not a
            # constant when the fighter was knocked down: measured, the roll's
            # victim calibrates to 7/8 at contact+70 and 1/2 at contact+100 —
            # he is simply still getting up at +70, and taking that number
            # would have inflated the wakeup window by 6 frames, silently.
            # So the point is derived from THIS run's own airborne window
            # rather than assumed.
            at_n, confirm_n = cal_at_n, cal_confirm_at_n
            floor = 0
            if who == "defender" and obs_for_dirs.victim_airborne_until:
                floor = obs_for_dirs.victim_airborne_until - anchor
            elif who == "attacker" and obs_for_dirs.attacker_airborne_until:
                floor = obs_for_dirs.attacker_airborne_until - origin
            if floor + 40 > at_n:
                at_n = floor + 40
                confirm_n = at_n + 30
            m.cal_points[shape] = (at_n, confirm_n)
            m.latencies[shape] = calibrate_for_move(
                session, rig=rig, script=script, port=port, origin=origin,
                observables=observables, sample_fn=sample_fns[port],
                defender_guard=guard_flag, at_n=at_n, confirm_at_n=confirm_n,
            )
            m.manifests[shape] = sweep_side(
                session, rig=rig, script=script, who=who, port=port,
                origin=origin, origin_kind=kind, observables=observables,
                sample_fn=sample_fns[port],
                input_latency_frames=m.latencies[shape],
                defender_guard=guard_flag, max_search=max_s,
                window_margin=window_margin,
                excluded_n=m.excluded_n if who == "attacker" else (),
                walk_directions=dirs,
            )

    # §8.4's cross-method check, applied to the quantity that is actually
    # comparable across observables. `first_true` is (see probe.py's algebra)
    # `A_rel − l − c` with the observable's manifestation margin `m` REMOVED;
    # the absolute manifest still carries `m`, so two sound observables
    # legitimately differ by 1 there and agreeing "to the frame" is only
    # meaningful for `first_true` and for DIFFERENCES of manifests.
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
        att, dfn = m.manifests["attacker/hit"][o], m.manifests["defender/hit"][o]
        if m.obs_hit.knockdown:
            # §1.1: a knockdown has a WAKEUP clock, not an advantage. The probe
            # will happily return one and it is meaningless.
            m.on_hit[o] = None
            m.wakeup_window[o] = (
                None if dfn.manifest is None else dfn.manifest - anchor_hit
            )
        else:
            m.on_hit[o] = advantage_between(att, dfn)
        if "attacker/block" in m.manifests:
            m.on_block[o] = advantage_between(
                m.manifests["attacker/block"][o], m.manifests["defender/block"][o]
            )
    if m.obs_hit.knockdown:
        m.notes.append(
            f"{script.name}: on_hit is NULL -- the victim leaves its own resting "
            f"y after contact and returns at frame "
            f"{m.obs_hit.victim_airborne_until}. docs/frames.md §1.1: that "
            f"outcome has no hit-advantage number. wakeup_window is "
            f"{WAKEUP_WINDOW_CONVENTION}."
        )
    return m


# ── rows ──────────────────────────────────────────────────────────────────


def special_row(
    *,
    family: str,
    port: str,
    char: str,
    move: str,
    core_id: str,
    rom_id: str,
    observable: str,
    method: str,
    input_latency_frames: int,
    obs_hit: Optional[MoveObservation] = None,
    on_hit: Optional[int] = None,
    on_block: Optional[int] = None,
    wakeup_window: Optional[int] = None,
    gap_px: Optional[int] = None,
    gap_walk_frames: Optional[int] = None,
    variant: Optional[str] = None,
    sample_n: Optional[int] = None,
    confidence: Optional[str] = None,
) -> dict:
    """A `store.FrameStore` row for one special, with §1.1's knockdown gate
    ENFORCED rather than documented: if the observation says the victim left
    its resting `y`, `on_hit` is dropped to NULL and `knockdown` is set, no
    matter what the caller passed. A knockdown has a wakeup clock, not an
    advantage — and the probe will happily return a number for it."""
    knockdown = None if obs_hit is None else obs_hit.knockdown
    if knockdown:
        on_hit = None
    return {
        "family": family, "port": port, "char": char, "move": move,
        "variant": variant,
        "gap_px": gap_px, "gap_walk_frames": gap_walk_frames,
        "hits": None if obs_hit is None else len(obs_hit.contacts),
        "damage": None if obs_hit is None else obs_hit.damage,
        "on_hit": on_hit,
        "on_block": on_block,
        "wakeup_window": wakeup_window,
        "knockdown": None if knockdown is None else int(knockdown),
        "rig_guard_state": (
            "held+none" if (on_block is not None and (on_hit is not None
                                                      or wakeup_window is not None))
            else ("held" if on_block is not None else "none")
        ),
        "observable": observable,
        "method": method,
        "input_latency_frames": input_latency_frames,
        "sample_n": sample_n,
        "confidence": confidence,
        "core_id": core_id,
        "rom_id": rom_id,
    }


# ── operator entry point ──────────────────────────────────────────────────

# The signatures each of Mileena's specials must reproduce to be recorded under
# its own name. These are NOT configuration and NOT a source of addresses: they
# are the live-audited facts from `library/mk2/mk2.md`'s "Special-move
# encodings, live-audited" table, used here as a CHECK. A run that cannot
# reproduce them refuses to write a row, which is the whole point -- `roll`'s
# failure mode is a crouching normal that fires `block+0xC0` identically, so
# damage, travel and the victim's `y` are the only discriminators.
MK2_MILEENA_SIGNATURES: Dict[str, Signature] = {
    "sai_throw": Signature(damage=23, hits=1, crossed=False),
    "teleport_kick": Signature(damage=32, hits=1, crossed=False),
    "roll": Signature(damage=21, hits=1, crossed=True, victim_knockdown=True,
                      min_attacker_travel_px=200),
}

# Frames of neutral between a ladder rung's walk-in and the charge. A fighter
# that is still decelerating is not a fighter at rest (kit.py's argument for
# not walking in at all); the sai's ladder cannot avoid the walk, so it pauses
# after it and records that it did.
SAI_LADDER_SETTLE = 3


def main() -> None:  # pragma: no cover - the live-rig path
    """Measure a character's SPECIALS. Never point this at port 4025
    (CLAUDE.md: the user's session).

        python -m shadow_train.framelab.specials \\
            --url http://127.0.0.1:4068/mcp --game library/mk2 \\
            --core ../FBNeo/.../fbneo_libretro.dylib --rom ~/games/roms/mk2.zip \\
            --arena shadow/arenas/mk2/m-v-r.state --char mileena \\
            --move teleport_kick --move roll \\
            --move sai_throw --rung 0 --rung 20 --rung 33 \\
            --charge-probe --report /tmp/specials.json
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

    ap = argparse.ArgumentParser(description="framelab: special-move measurement")
    ap.add_argument("--url", default="http://127.0.0.1:4068/mcp")
    ap.add_argument("--game", default="library/mk2")
    ap.add_argument("--core", required=True)
    ap.add_argument("--rom", required=True)
    ap.add_argument("--arena", required=True)
    ap.add_argument("--char", default="mileena")
    ap.add_argument("--move", action="append", default=[])
    ap.add_argument("--rung", action="append", type=int, default=[],
                    help="sai_throw only: walk-in frames before the charge")
    ap.add_argument("--charge-probe", action="store_true",
                    help="answer the pre-charged-arena question empirically")
    ap.add_argument("--charge-move", default="sai_throw")
    ap.add_argument("--scratch", default="shadow/framelab/scratch")
    ap.add_argument("--db", default=None, help="write rows to this FrameStore")
    ap.add_argument("--report", default=None, help="write a JSON report here")
    ap.add_argument("--sample-n", type=int, default=1,
                    help="how many INDEPENDENT full measurements this run's "
                         "rows summarize (kit.py's convention: a second, "
                         "cold-started process reproducing a cell makes it 2). "
                         "Never a retry count.")
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

    session = LabSession(
        client=McpClient(args.url),
        verify_fn=obsmod.make_arena_verifier(prof, expect={}),
    )
    session.enforce_preconditions()
    session.load_state(args.arena)
    facing = facing_from_x(reads["attacker_x"](session), reads["victim_x"](session))
    if facing is None:
        raise SystemExit(
            "cannot derive the attacker's facing from the arena's positions "
            "(docs/frames.md §5: facing is NULL, never a guess) -- refusing."
        )
    rig = Rig(arena=args.arena, attacker_port=0, defender_port=1,
              guard_buttons=tuple(prof.attack_chords["Block"]),
              quiet_frames=flspec.quiet_frames)
    core_id, rom_id = compute_core_id(args.core), compute_rom_id(args.rom)
    report: Dict[str, Any] = {"arena": args.arena, "facing": facing,
                             "core_id": core_id, "rom_id": rom_id,
                             "observables": observables, "moves": [], "rows": []}

    if args.charge_probe:
        Path(args.scratch).mkdir(parents=True, exist_ok=True)
        chord = prof.attack_chords["HP"]
        threshold = int(
            special_encoding(prof, args.char, args.charge_move)[0]["min_frames"]
        )
        cp = charge_persistence(
            session, arena=args.arena, port=0, chord=chord, banked_frames=20,
            fresh_threshold_frames=threshold,
            scratch_path=str(Path(args.scratch) / "precharge-20.state"),
            candidates=range(1, threshold + 1), contact_read=contact_read,
        )
        tol = charge_release_tolerance(
            session, rig=rig, chord=chord, hold_before=20,
            hold_after=threshold - 21, gaps=range(1, 5),
            contact_read=contact_read,
        )
        cp = ChargePersistence(**{**cp.__dict__, "release_tolerance_frames": tol})
        report["charge_persistence"] = {
            **cp.__dict__, "survives": cp.survives,
            "banked_total": cp.banked_total,
        }
        print("charge persistence:", cp)

    rungs = args.rung or [0]
    for move in args.move:
        expect = MK2_MILEENA_SIGNATURES.get(move, Signature())
        for k in (rungs if move == args.charge_move else [0]):
            lead: Tuple[ScriptStep, ...] = ()
            if k:
                lead = (ScriptStep(frames=k, buttons=(facing,)),
                        ScriptStep(frames=SAI_LADDER_SETTLE, buttons=()))
            script = special_script(prof, args.char, move, facing=facing,
                                    lead_in=lead, name=move)
            kind = "release+1" if any(
                "hold" in s for s in special_encoding(prof, args.char, move)
            ) else "contact"
            m = measure_special(
                session, rig=rig, script=script, observables=observables,
                sample_fns=sample_fns, contact_read=contact_read, reads=reads,
                expect=expect, attacker_origin_kind=kind,
            )
            m.gap_walk_frames = k
            for note in m.notes:
                print("NOTE:", note)
            print(f"{move} K={k} gap={m.gap_px}px contact={m.contact_hit} "
                  f"hits={m.hits} dmg={m.obs_hit.damage if m.obs_hit else None} "
                  f"on_hit={m.on_hit} on_block={m.on_block} "
                  f"wakeup={m.wakeup_window}")
            report["moves"].append({
                "move": move, "rung": k, "gap_px": m.gap_px,
                "contact_hit": m.contact_hit, "contact_block": m.contact_block,
                "hits": m.hits, "damage": m.obs_hit.damage if m.obs_hit else None,
                "signature_problems": list(m.signature_problems),
                "excluded_n": list(m.excluded_n),
                "latencies": m.latencies,
                "cal_points": {k: list(v) for k, v in m.cal_points.items()},
                "manifests": {
                    shape: {o: {"origin": sm.origin, "origin_kind": sm.origin_kind,
                                "first_true": sm.sweep.first_true,
                                "window": sm.sweep.window,
                                "direction": sm.sweep.direction,
                                "manifest": sm.manifest,
                                "monotone": sm.sweep.monotone,
                                "predicate": "".join(
                                    "T" if v else "F" for v in sm.sweep.predicate)}
                          for o, sm in per.items()}
                    for shape, per in m.manifests.items()},
                "on_hit": m.on_hit, "on_block": m.on_block,
                "wakeup_window": m.wakeup_window,
                "knockdown": None if m.obs_hit is None else m.obs_hit.knockdown,
                "crossed": None if m.obs_hit is None else m.obs_hit.crossed,
                "positions": {
                    tag: None if o is None else {
                        "attacker_x": list(o.attacker_x),
                        "victim_x": list(o.victim_x),
                        "attacker_airborne_until": o.attacker_airborne_until,
                        "victim_airborne_until": o.victim_airborne_until,
                        "crossed": o.crossed,
                        "facing_before": o.facing_before,
                        "facing_after": o.facing_after,
                        "contacts": list(o.contacts),
                        "damage": o.damage,
                    }
                    for tag, o in (("hit", m.obs_hit), ("block", m.obs_block))},
                "notes": m.notes,
            })
            if m.obs_hit is None or m.signature_problems:
                continue
            for o in observables:
                report["rows"].append(special_row(
                    family=prof.family, port=prof.port, char=args.char, move=move,
                    core_id=core_id, rom_id=rom_id, observable=o,
                    method="linear_sweep",
                    input_latency_frames=m.latencies["attacker/hit"][o],
                    obs_hit=m.obs_hit, on_hit=m.on_hit.get(o),
                    on_block=m.on_block.get(o),
                    wakeup_window=m.wakeup_window.get(o),
                    gap_px=m.gap_px, gap_walk_frames=k,
                    variant=None if not k else f"walk-in-{k}",
                    sample_n=args.sample_n, confidence="high",
                ))

    if args.report:
        Path(args.report).write_text(json.dumps(report, indent=1, default=str))
        print("report ->", args.report)
    if args.db and report["rows"]:
        Path(args.db).parent.mkdir(parents=True, exist_ok=True)
        with FrameStore(args.db) as store:
            for row in report["rows"]:
                store.insert(row)
        print(f"wrote {len(report['rows'])} rows to {args.db}")
    print(f"steps={session.steps_taken} loads={session.loads_done}")


if __name__ == "__main__":  # pragma: no cover
    main()
