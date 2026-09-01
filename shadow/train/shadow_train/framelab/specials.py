"""docs/frames.md, applied to SPECIAL moves — and the places the contract
does not reach.

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

## Reptile's four, and the two further shapes they add (task Q2)

  4. **A move that deals NO DAMAGE has no anchor at all.** `invisibility`
     never touches the opponent, so §4.1's contact signal never fires, and
     §1.1 says the honest advantage is NULL — not a number. Its RECOVERY is
     still a real quantity, and it is measured by a WHIFF-ANCHORED probe
     (`measure_whiff_recovery`), anchored on `script.total_frames` rather
     than on contact.

     That anchor style is new and dangerous in exactly one way, which
     `hitstop.measure_whiff_reference` found the hard way: anchoring at
     frame 0 puts the probe's walk on the same frame as the move's own
     button, and `hold_buttons` REPLACES rather than ORs, so the move never
     comes out in EITHER the probe or the control run — and the sweep
     reports a perfectly reproducible `first_true = 0`. Anchoring past the
     script's own last input frame avoids the collision by construction, and
     `measure_whiff_recovery` REFUSES an earlier anchor rather than trusting
     the caller.

     Two independent validations are provided rather than assumed:
     `origin_invariance` (the same side's ABSOLUTE manifest measured from
     two different origins must be the same frame — run on a move that HAS a
     contact anchor, so the whiff anchor is checked against the established
     one) and `screen_preemption_scan` (the framebuffer as the witness of
     last resort for a move with no memory observable at all).

  5. **A LOW.** `slide` must be blocked, and whether STANDING guard is
     enough is a property of the move, not of the rig — `measure_guard_height`
     runs the identical script against each guard stance the caller names and
     reads the verdict off the damage, never off an assumption about the
     genre.
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
    "ATTACKER_ORIGIN_KINDS",
    "LADDER_SETTLE",
    "WAKEUP_WINDOW_CONVENTION",
    "SpecialEncodingError",
    "PreemptedProbeError",
    "WhiffAnchorError",
    "OriginDependenceError",
    "ScreenWitnessError",
    "special_encoding",
    "special_script",
    "attacker_origin",
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
    "origin_invariance",
    "GuardTrial",
    "GuardHeight",
    "guard_height_verdict",
    "measure_guard_height",
    "WhiffRecovery",
    "measure_whiff_recovery",
    "decode_png_rgba",
    "make_screen_region_read",
    "region_pixel_diff",
    "observe_screen",
    "screen_probe_effect",
    "screen_preemption_scan",
    "ChargePersistence",
    "charge_persistence",
    "charge_release_tolerance",
    "SpecialMeasurement",
    "calibrate_for_move",
    "measure_special",
    "special_row",
    "MK2_MILEENA_SIGNATURES",
    "MK2_REPTILE_SIGNATURES",
    "MK2_SIGNATURES",
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

# Where the ATTACKER's act-again clock starts, and therefore what `N` in that
# side's sweep is relative to. `measure_special` takes one of these by name so
# a caller cannot smuggle in an origin the module does not know how to defend.
#
#   "contact"      — the attacker committed at or before contact (a normal,
#                    the teleport, the roll, the slide). §4.3's own assumption.
#   "release+1"    — a CHARGE/release move: the clock starts at the release,
#                    which `MoveScript` puts at `total_frames`. The `+1` is the
#                    preemption gap (a probe on the release frame itself
#                    cancels the move).
#   "input_end+1"  — a projectile that is NOT a charge (Reptile's `acid_spit`
#                    and `force_ball`): the attacker's input finishes long
#                    before the projectile arrives, so his clock starts at the
#                    end of the input, arithmetically the same frame as
#                    "release+1" and kept as a SEPARATE name because it is a
#                    different claim about the move. Recorded per row: a reader
#                    must be able to tell which clock a manifest is on.
ATTACKER_ORIGIN_KINDS = frozenset({"contact", "release+1", "input_end+1"})

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


class WhiffAnchorError(ProbeError):
    """A whiff-anchored sweep was asked to anchor at or before the frame the
    move's own input ends.

    `hitstop.measure_whiff_reference` measured what that produces: the probe's
    walk lands on the same frame as the move's own button, `hold_buttons`
    REPLACES rather than ORs, so no move comes out in either the probe or the
    control run — and the sweep returns a perfectly REPRODUCIBLE `first_true=0`
    (4/4 trials) that is the fighter walking, not recovering. `PreemptedProbeError`
    catches the same failure where a contact anchor exists to witness it; this
    one is structural, and it is the only defence available to a move that
    deals no damage."""


class OriginDependenceError(ProbeError):
    """The same side's ABSOLUTE manifest came out differently depending on
    which origin its sweep was swept from.

    An origin is a re-parameterisation of one predicate — `N` counts from a
    different frame, and `origin + first_true + window` must land on the same
    absolute frame either way. When it does not, at least one of the two
    anchors is inside something (the move's own input window, residual stun)
    and §7 forbids picking the one you like."""


class ScreenWitnessError(ProbeError):
    """The framebuffer classifier could not tell the two reference states
    apart, so it is not a witness. §2.5: a classifier whose two poles overlap
    reports a coin flip, not a measurement."""


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

    A step naming a kind not in `SPECIAL_STEP_KEYS` raises rather than being
    dropped.

    ## A NON-TERMINAL release (Reptile's `invisibility`) — modelled, not guessed

    `[BLK] U U D`, release BLK, then HP. An earlier draft refused this
    outright, because "drop the step" and "hold the chord through the next
    step" are different moves and the module would have had to pick one.

    It is modelled now, and the model is forced rather than chosen: a
    `ScriptStep`'s `buttons` is the port's ENTIRE held set (`hold_buttons`
    replaces, it does not OR — `probe.ScriptStep`), so the `step_gap` NEUTRAL
    frames this function already inserts between every pair of steps ARE the
    released window, and the step that follows the release already holds only
    its own chord. The release therefore emits no mask of its own, exactly
    like a terminal one — but the check is not vacuous: if any LATER step
    still holds a class the release names, the encoding is self-contradictory
    and this raises. The M1 live audit played this same playback and the move
    fired ("fires at gap >= 2 for every `frames` value, with or without the
    `release` step" — mk2.md), which is the evidence that the neutral gap is
    an adequate release on this port.
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
            released = {str(c) for c in raw.get("release", ())}
            later_holds = {
                str(c)
                for nxt in encoded[i + 1 :]
                for key in ("press", "hold", "while_held")
                for c in nxt.get(key, ())
            }
            still_held = sorted(released & later_holds)
            if still_held:
                raise SpecialEncodingError(
                    f"{char}/{move} step {i} RELEASES {still_held} and a later "
                    "step holds the same class again. That is not a falling "
                    "edge this playback can express: every step's mask is the "
                    "port's entire held set, so the release and the re-hold "
                    "would collapse into one continuous hold. Encode the "
                    "re-press as its own step with an explicit gap."
                )
            if i != len(encoded) - 1 and step_gap < 1:
                raise SpecialEncodingError(
                    f"{char}/{move} step {i} is a non-terminal `release`, but "
                    f"step_gap={step_gap} leaves no neutral frame for the "
                    "chord to actually be released on."
                )
            continue  # terminal: the schedule's trailing release.
            # non-terminal: the step_gap neutral frames before the next step.
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


def attacker_origin(script: MoveScript, kind: str, contact_frame: int) -> int:
    """The absolute replay frame the ATTACKER's act-again sweep counts `N`
    from, for one of `ATTACKER_ORIGIN_KINDS`.

    The `+1` shared by the two non-contact kinds is not a fudge and it is not
    cosmetic: a probe asserted on the move's own last input frame REPLACES
    that input (`hold_buttons` replaces, it does not OR), so the first frame at
    which "can he walk yet" is even a well-posed question is the one after it.
    `preemption_scan` measures that where a contact anchor can witness it;
    this is the arithmetic that keeps the origin out of the hazard by
    construction where one cannot.
    """
    if kind not in ATTACKER_ORIGIN_KINDS:
        raise ProbeError(
            f"unknown attacker origin kind {kind!r} (known: "
            f"{sorted(ATTACKER_ORIGIN_KINDS)}). The origin is part of the "
            "measurement, not a default."
        )
    if kind == "contact":
        return contact_frame
    return script.total_frames + 1


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
    origin_kind: str         # "contact" | "release+1" | "input_end+1" | ...
    sweep: SweepResult
    excluded_n: Tuple[int, ...] = ()   # N where the probe preempted the move
    # The direction the FIRST attempt used, when this observable had to fall
    # back to the other one (see `sweep_side`'s differential-collision retry).
    # Empty when the first direction worked, so a row can say which it was.
    rejected_directions: Tuple[str, ...] = ()

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
    retry_other_direction: bool = True,
) -> Dict[str, SideManifest]:
    """One side's exhaustive sweep, with the three refusals that make its
    `first_true` a number: non-monotone predicate, a boundary measured against
    the edge of the search (§7's "no silent caps"), and — new here — a
    boundary that lands inside `excluded_n`, where the probe cancelled the
    move (`preemption_scan`).

    Never returns a "never actionable"; a sweep that found nothing comes back
    with `manifest = None` and the caller reports NULL (§4.2/§2.5).

    ## The differential-collision retry, and why it is not "try until it works"

    §4.2's blocked-direction hazard is written as a STATIC property — a
    fighter cannot walk into a wall or into the opponent's body, so try the
    other direction. Reptile's `force_ball` shows the DYNAMIC form, and it
    does not look like a blocked direction at all: after the ball launches the
    victim, the anti-overlap separation pushes the ATTACKER left at very
    nearly walking speed, so for five consecutive N a leftward walk and the
    control's free push produce the SAME `obj+0x12` over the whole comparison
    window. Measured, gap-5, absolute frames 102-106: probe x
    `475,473,472,470,468,466` and control x `475,473,472,470,468,466` —
    identical — while `struct_velocity` reads `00feff` in the probe and
    `000000` in the control on every one of those frames. The fighter is
    walking; the position just cannot tell, because the differential cancels
    the push and the walk together.

    That produces a FALSE island 40 frames PAST a boundary both observables
    otherwise agree on (44 and 44), and the non-monotone gate correctly
    refuses the whole cell. Swept in the OTHER direction — where the walk and
    the push have opposite signs — both observables are monotone and both
    return the same 44.

    So the retry is the existing per-observable direction rule (§4.2) extended
    from "this direction never diverged" to "this direction produced a
    predicate that is not a predicate". It is bounded by the rig's own
    candidate list, the alternate result must itself be monotone, and it is
    still subject to every other gate — including the caller's §8.4
    cross-observable check on `first_true`, which is what would catch a
    direction that produced a DIFFERENT boundary rather than a cleaner one.
    """
    def rig_with(dirs: Optional[Sequence[str]]) -> Rig:
        if dirs is None:
            return rig
        return Rig(
            arena=rig.arena, attacker_port=rig.attacker_port,
            defender_port=rig.defender_port, guard_buttons=rig.guard_buttons,
            walk_directions=rig.walk_directions,
            walk_directions_by_port={**dict(rig.walk_directions_by_port),
                                     port: tuple(dirs)},
            quiet_frames=rig.quiet_frames,
        )

    def sweep_with(dirs: Optional[Sequence[str]], obs: Sequence[str]):
        return sweep_actionable(
            session, rig=rig_with(dirs), script=script, port=port,
            anchor=origin, observables=list(obs), sample_fn=sample_fn,
            input_latency_frames={o: input_latency_frames[o] for o in obs},
            defender_guard=defender_guard, window_margin=window_margin,
            max_search=max_search, exhaustive=True,
        )

    candidates = tuple(
        walk_directions
        if walk_directions is not None
        else (rig.walk_directions_by_port.get(port) or rig.walk_directions)
    )
    results = sweep_with(candidates, observables)
    rejected: Dict[str, Tuple[str, ...]] = {o: () for o in results}
    if retry_other_direction:
        broken = [o for o, s in results.items() if s.monotone is False]
        for obs_name in broken:
            used = results[obs_name].direction
            for d in candidates:
                if d == used:
                    continue
                alt = sweep_with((d,), [obs_name])[obs_name]
                if alt.monotone is not False:
                    results[obs_name] = alt
                    rejected[obs_name] = (used,) if used else ()
                    break

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
            excluded_n=excluded, rejected_directions=rejected[obs_name],
        )
    return out


def origin_invariance(
    session: LabSession,
    *,
    rig: Rig,
    script: MoveScript,
    who: str,
    port: int,
    origins: Sequence[Tuple[int, str]],
    end_frame: int,
    observables: Sequence[str],
    sample_fn,
    input_latency_frames: Mapping[str, int],
    defender_guard: bool,
    window_margin: int = 2,
    cap_margin: int = 5,
    walk_directions: Optional[Sequence[str]] = None,
    excluded_n: Optional[Mapping[str, Sequence[int]]] = None,
) -> Dict[str, Dict[str, int]]:
    """Sweep ONE side from two or more different origins over the SAME
    absolute frame range, and require the resulting ABSOLUTE manifests to be
    the same frame. Returns `{observable: {origin_kind: manifest}}`.

    ## Why this is the validation the whiff anchor needed

    An origin is only a re-parameterisation: `N` counts from a different
    frame, `window` is unchanged, and `origin + first_true + window` must land
    on the same absolute frame whichever origin was used. Anything that makes
    it not — the probe colliding with the move's own input, residual stun at
    one origin's calibration point, a non-monotone predicate seen from one end
    and not the other — shows up here as a DISAGREEMENT and refuses.

    Run it on a move that HAS a contact anchor, with `("contact", …)` as one
    of the origins, and it checks the new anchor against the established one
    on the same rig, in the same run, with the same observables. That is the
    only way a whiff anchor (which by definition has no contact to check
    against) can be validated at all — you validate the STYLE on a move that
    can witness it, then apply the style where nothing can.

    `end_frame` is the absolute frame both sweeps must be able to reach, so
    each origin's `max_search` is derived from it rather than guessed: two
    sweeps with different search depths do not cover the same predicate, and
    "they disagreed" would then be a statement about the search, not the move.
    """
    if len(origins) < 2:
        raise ValueError(
            "origin_invariance needs at least two origins -- one origin cannot "
            "tell an invariant apart from a coincidence."
        )
    out: Dict[str, Dict[str, int]] = {o: {} for o in observables}
    for origin, kind in origins:
        max_search = end_frame - origin
        if max_search < cap_margin + 1:
            raise ProbeError(
                f"origin {kind!r} at frame {origin} leaves only {max_search} "
                f"frames before end_frame={end_frame} -- not enough search "
                "depth to place a boundary away from the edge (§7)."
            )
        per = sweep_side(
            session, rig=rig, script=script, who=who, port=port, origin=origin,
            origin_kind=kind, observables=observables, sample_fn=sample_fn,
            input_latency_frames=input_latency_frames,
            defender_guard=defender_guard, max_search=max_search,
            window_margin=window_margin, cap_margin=cap_margin,
            excluded_n=tuple((excluded_n or {}).get(kind, ())),
            walk_directions=walk_directions,
        )
        for obs_name, sm in per.items():
            if sm.manifest is None:
                raise ProbeError(
                    f"{script.name} {who}/{obs_name}: the sweep from origin "
                    f"{kind!r} never diverged, so there is no manifest to "
                    "compare. NULL is a legitimate result for a cell, but it "
                    "cannot participate in an invariance check."
                )
            if sm.sweep.first_true == 0:
                raise ProbeError(
                    f"{script.name} {who}/{obs_name}: the sweep from origin "
                    f"{kind!r} (frame {origin}) has first_true=0 -- the "
                    "boundary is the FIRST N looked at, so 'free exactly at "
                    "this origin' and 'free some frames BEFORE it' are "
                    "indistinguishable. A sweep starts at N=0 and cannot "
                    "return a negative boundary; this origin is clipped from "
                    "below and cannot participate in an invariance check. "
                    "(docs/frames.md §7's cap rule, at the other end of the "
                    "search: it is stated only for the upper edge.)"
                )
            out[obs_name][kind] = sm.manifest
    for obs_name, by_kind in out.items():
        if len(set(by_kind.values())) > 1:
            raise OriginDependenceError(
                f"{script.name} {who}/{obs_name}: the absolute manifest frame "
                f"depends on which origin the sweep started from ({by_kind}). "
                "An origin is a re-parameterisation of one predicate; if it "
                "changes the answer, at least one anchor is inside something "
                "(the move's own input window, or residual stun) and neither "
                "number may be published."
            )
    return out


# ── a LOW, and what stance actually blocks it ─────────────────────────────


@dataclass(frozen=True)
class GuardTrial:
    """One run of the identical script against ONE guard stance."""

    variant: str
    guard_buttons: Tuple[str, ...]
    connected: bool
    damage: Optional[int]
    contact_frame: Optional[int]
    victim_knockdown: Optional[bool]


@dataclass(frozen=True)
class GuardHeight:
    unguarded_damage: Optional[int]
    trials: Tuple[GuardTrial, ...]
    verdict: Optional[str]
    note: str = ""

    def by_variant(self, name: str) -> Optional[GuardTrial]:
        return next((t for t in self.trials if t.variant == name), None)


def guard_height_verdict(
    unguarded_damage: Optional[int], trials: Sequence[GuardTrial]
) -> Tuple[Optional[str], str]:
    """The pure half of `measure_guard_height`: which stances actually stopped
    the move, read off the damage rather than off genre folklore.

    A stance STOPPED the move iff it connected for strictly less than the
    unguarded damage — MK2 chips a blocked hit for a few points, so §2.6's
    warning applies in both directions: "zero damage means WHIFF, not block".
    A stance that took the FULL damage did not block it; a stance under which
    the move did not connect AT ALL is neither (the stance moved the hurtbox
    out of the way), and is reported as such rather than counted as a block.

    Verdicts, and they are deliberately few:
      * `"mid"`   — every stance tried stopped it.
      * `"low"`   — crouching stopped it and standing did not.
      * `"overhead"` — standing stopped it and crouching did not.
      * `None`    — anything else, including "it whiffed against a stance",
                    with the reason in the note. §2.5: no verdict is NULL, and
                    NULL is never a default guess.
    """
    if unguarded_damage is None:
        return None, ("the move did not connect against an unguarded defender, "
                      "so there is no full-damage reference to compare a "
                      "blocked run against (docs/frames.md §1.1: a whiff).")
    stopped: Dict[str, Optional[bool]] = {}
    notes: List[str] = []
    for t in trials:
        if not t.connected:
            stopped[t.variant] = None
            notes.append(
                f"{t.variant}: the move did not connect at all -- that stance "
                "moved the hurtbox, it did not block"
            )
        elif t.damage is not None and t.damage < unguarded_damage:
            stopped[t.variant] = True
            notes.append(f"{t.variant}: chip {t.damage} of {unguarded_damage} -- blocked")
        else:
            stopped[t.variant] = False
            notes.append(
                f"{t.variant}: full damage {t.damage} through held guard -- NOT blocked"
            )
    note = "; ".join(notes)
    if any(v is None for v in stopped.values()):
        return None, note
    stand, crouch = stopped.get("standing"), stopped.get("crouching")
    if stand is None or crouch is None:
        return None, note + "; needs both a standing and a crouching stance"
    if stand and crouch:
        return "mid", note
    if crouch and not stand:
        return "low", note
    if stand and not crouch:
        return "overhead", note
    return None, note + "; neither stance blocked it -- unblockable, or the rig is wrong"


def measure_guard_height(
    session: LabSession,
    *,
    rig: Rig,
    script: MoveScript,
    contact_read: Callable[[LabSession], Hashable],
    reads: Mapping[str, Callable[[LabSession], Optional[int]]],
    stances: Mapping[str, Sequence[str]],
    total_frames: int = 150,
) -> GuardHeight:
    """Run the identical script against an UNGUARDED defender and then against
    each named guard stance, and read `guard_height` off the damages.

    `stances` maps a variant name to that stance's entire held set, e.g.
    `{"standing": ("l",), "crouching": ("l", "down")}` — the buttons come from
    the caller (the profile's `attack_chords["Block"]` plus a direction), never
    from a constant here.

    §2.6 is the whole design: the lab DRIVES the defender, so the stance is
    ground truth and nothing is inferred from the health delta except the one
    thing a health delta genuinely says — whether the hit was reduced.
    """
    def run(guard_buttons: Sequence[str]) -> MoveObservation:
        r = Rig(
            arena=rig.arena, attacker_port=rig.attacker_port,
            defender_port=rig.defender_port, guard_buttons=tuple(guard_buttons),
            walk_directions=rig.walk_directions,
            walk_directions_by_port=rig.walk_directions_by_port,
            quiet_frames=rig.quiet_frames,
        )
        return observe_move(
            session, rig=r, script=script, total_frames=total_frames,
            defender_guard=bool(guard_buttons), contact_read=contact_read,
            attacker_x_read=reads["attacker_x"], attacker_y_read=reads["attacker_y"],
            victim_x_read=reads["victim_x"], victim_y_read=reads["victim_y"],
        )

    free = run(())
    trials: List[GuardTrial] = []
    for name, buttons in stances.items():
        obs = run(tuple(buttons))
        trials.append(GuardTrial(
            variant=name, guard_buttons=tuple(buttons),
            connected=obs.connected, damage=obs.damage,
            contact_frame=obs.contacts[0] if obs.contacts else None,
            victim_knockdown=obs.knockdown,
        ))
    verdict, note = guard_height_verdict(free.damage, trials)
    return GuardHeight(
        unguarded_damage=free.damage, trials=tuple(trials), verdict=verdict,
        note=note,
    )


# ── a move with NO contact: whiff-anchored recovery ───────────────────────


@dataclass(frozen=True)
class WhiffRecovery:
    """When the attacker can act again after a move that never touches anyone.

    Not an advantage and never storable as one: there is no defender clock in
    it, because there is no defender event. §1.1's "whiff" row — attacker's
    recovery only, advantage NOT meaningful.
    """

    move: str
    arena: str
    origin: int
    origin_kind: str
    attack_input_frame: int
    sweeps: Dict[str, SideManifest]
    latencies: Dict[str, int]
    cal_points: Tuple[int, int]
    witness: str = ""

    def total(self, observable: str) -> Optional[int]:
        """`total` in docs/frames.md §6's sense, input-relative: "the earliest
        frame the attacker can START A WALK", counted from the frame the
        move's own input begins. NULL if that observable never diverged."""
        m = self.sweeps[observable].manifest
        return None if m is None else m - self.attack_input_frame

    def first_true(self, observable: str) -> Optional[int]:
        return self.sweeps[observable].sweep.first_true


def measure_whiff_recovery(
    session: LabSession,
    *,
    rig: Rig,
    script: MoveScript,
    port: int,
    observables: Sequence[str],
    sample_fn,
    contact_read: Optional[Callable[[LabSession], Hashable]] = None,
    walk_directions: Optional[Sequence[str]] = None,
    max_search: int = 90,
    window_margin: int = 2,
    cal_at_n: int = 70,
    cal_confirm_at_n: int = 100,
    origin: Optional[int] = None,
    observe_frames: int = 150,
    witness: str = "",
) -> WhiffRecovery:
    """The attacker's act-again sweep for a move with no contact anchor,
    anchored on the end of the move's own input.

    Two refusals, and both are the point of the function:

      1. **The anchor may not sit at or before `script.total_frames`.** A probe
         asserted while the move's own button is still held REPLACES it, so the
         move never comes out and the sweep measures the probe walking. That
         failure is REPRODUCIBLE, not flaky (`hitstop.measure_whiff_reference`
         measured 4/4), which is why it needs a structural guard rather than a
         repeat check.
      2. **If a `contact_read` is supplied, the move must actually NOT
         connect.** A run that connects has hitstop and stun in it and is not
         a whiff; calling it one would fold a real contact into "recovery".

    `witness` is free text recording what evidence says the move CAME OUT at
    all. For a damageless move nothing in memory says so (mk2.md: "there is no
    invisibility flag to watch"), so a row measured with an empty `witness` is
    a row that cannot distinguish the move from a whiffing normal — the caller
    is expected to fill it from `screen_preemption_scan` or equivalent.
    """
    anchor = script.total_frames + 1 if origin is None else int(origin)
    if anchor <= script.total_frames:
        raise WhiffAnchorError(
            f"{script.name}: a whiff-anchored sweep may not anchor at frame "
            f"{anchor}, which is at or before the frame the move's own input "
            f"ends ({script.total_frames}). The probe's walk would REPLACE the "
            "move's own button (hold_buttons replaces, it does not OR) and the "
            "sweep would report the fighter walking instead of recovering -- "
            "reproducibly, 4/4 trials, when this was last measured."
        )
    if contact_read is not None:
        obs = observe_move(
            session, rig=rig, script=script, total_frames=observe_frames,
            defender_guard=False, contact_read=contact_read,
            attacker_x_read=lambda s: None, attacker_y_read=lambda s: None,
            victim_x_read=lambda s: None, victim_y_read=lambda s: None,
        )
        if obs.connected:
            raise ProbeError(
                f"{script.name}: the contact signal fired at frames "
                f"{list(obs.contacts)} (damage {obs.damage}), so this run is "
                "not a whiff. A connecting run carries hitstop and stun; "
                "measuring it as 'recovery' would fold both into the number."
            )
    cal = calibrate_for_move(
        session, rig=rig, script=script, port=port, origin=anchor,
        observables=observables, sample_fn=sample_fn, defender_guard=False,
        at_n=cal_at_n, confirm_at_n=cal_confirm_at_n,
    )
    sweeps = sweep_side(
        session, rig=rig, script=script, who="attacker", port=port,
        origin=anchor, origin_kind="input_end+1", observables=observables,
        sample_fn=sample_fn, input_latency_frames=cal, defender_guard=False,
        max_search=max_search, window_margin=window_margin,
        walk_directions=walk_directions,
    )
    return WhiffRecovery(
        move=script.name, arena=rig.arena, origin=anchor,
        origin_kind="input_end+1", attack_input_frame=script.attack_input_frame,
        sweeps=sweeps, latencies=cal,
        cal_points=(cal_at_n, cal_confirm_at_n), witness=witness,
    )


# ── the framebuffer as the witness of last resort ─────────────────────────


def decode_png_rgba(blob: bytes) -> Tuple[int, int, bytes]:
    """8-bit RGBA, colour-type 6, non-interlaced PNG -> `(w, h, pixels)`.

    That is exactly what `app://screen` serves (`src/mcp/server.rs`), and it is
    decoded here with stdlib `zlib` for the same reason the rest of this
    package uses stdlib `sqlite3`: no new dependency. `scripts/re/screen_tools.py`
    has the same reader for the RE workflow; this one exists because
    `shadow_train` may not import from `scripts/`.
    """
    import struct
    import zlib

    if blob[:8] != b"\x89PNG\r\n\x1a\n":
        raise ScreenWitnessError("app://screen did not return a PNG")
    pos, w, h, bitdepth, colortype = 8, 0, 0, 0, 0
    idat = bytearray()
    while pos < len(blob):
        (ln,) = struct.unpack(">I", blob[pos:pos + 4])
        typ = blob[pos + 4:pos + 8]
        chunk = blob[pos + 8:pos + 8 + ln]
        pos += 12 + ln
        if typ == b"IHDR":
            w, h, bitdepth, colortype = struct.unpack(">IIBB", chunk[:10])
        elif typ == b"IDAT":
            idat += chunk
        elif typ == b"IEND":
            break
    if bitdepth != 8 or colortype != 6:
        raise ScreenWitnessError(
            f"need 8-bit RGBA (bitdepth 8, colour type 6), got {bitdepth}/{colortype}"
        )
    raw = zlib.decompress(bytes(idat))
    stride = w * 4
    out = bytearray(w * h * 4)
    prev = bytearray(stride)
    p = 0
    for y in range(h):
        f = raw[p]
        p += 1
        line = bytearray(raw[p:p + stride])
        p += stride
        if f == 1:
            for i in range(4, stride):
                line[i] = (line[i] + line[i - 4]) & 0xFF
        elif f == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif f == 3:
            for i in range(stride):
                a = line[i - 4] if i >= 4 else 0
                line[i] = (line[i] + ((a + prev[i]) >> 1)) & 0xFF
        elif f == 4:
            for i in range(stride):
                a = line[i - 4] if i >= 4 else 0
                c = prev[i - 4] if i >= 4 else 0
                b = prev[i]
                pp = a + b - c
                pa, pb, pc = abs(pp - a), abs(pp - b), abs(pp - c)
                pred = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pred) & 0xFF
        elif f != 0:
            raise ScreenWitnessError(f"unknown PNG row filter {f}")
        out[y * stride:(y + 1) * stride] = line
        prev = line
    return w, h, bytes(out)


def make_screen_region_read(
    client: Any, *, region: Tuple[int, int, int, int]
) -> Callable[[LabSession], bytes]:
    """`app://screen`, cropped to `region` (`x0, y0, x1, y1`, half-open).

    The rect is a CALLER-supplied fact and deliberately not a constant here:
    no port profile carries a sprite bounding box (docs/game-profiles.md), and
    CLAUDE.md's "never hardcode a game address in code again" covers a pixel
    rect exactly as it covers a byte offset. Locate it by differencing two
    screens and pass it in.
    """
    x0, y0, x1, y1 = region

    def read(_session: LabSession) -> bytes:
        w, h, px = decode_png_rgba(client.read_resource("app://screen"))
        if not (0 <= x0 < x1 <= w and 0 <= y0 < y1 <= h):
            raise ScreenWitnessError(
                f"region {region} is outside the {w}x{h} framebuffer"
            )
        rows = [px[(y * w + x0) * 4:(y * w + x1) * 4] for y in range(y0, y1)]
        return b"".join(rows)

    return read


def region_pixel_diff(a: bytes, b: bytes) -> int:
    """How many 4-byte pixels differ between two equal-length crops."""
    if len(a) != len(b):
        raise ScreenWitnessError(
            f"cannot compare crops of different sizes ({len(a)} vs {len(b)})"
        )
    return sum(1 for i in range(0, len(a), 4) if a[i:i + 4] != b[i:i + 4])


def observe_screen(
    session: LabSession,
    *,
    rig: Rig,
    script: MoveScript,
    total_frames: int,
    screen_read: Callable[[LabSession], bytes],
    defender_guard: bool = False,
    probe_port: Optional[int] = None,
    probe_buttons: Sequence[str] = (),
    probe_at: Optional[int] = None,
) -> bytes:
    """One replay, sampled ONLY at its last frame, returning the screen crop.

    The framebuffer is not frame-exact on this transport (`app://screen` still
    shows the previous rendered frame until the core has run one — see
    `calibrate.sprite_lag_frames`), so it is read once at a settle point far
    past the event rather than per frame. That is enough for a STATE question
    ("is he drawn at all") and not enough for a TIMING one; nothing here asks
    it a timing question.
    """
    trace = replay(
        session, rig=rig, script=script, total_frames=total_frames,
        defender_guard=defender_guard, probe_port=probe_port,
        probe_buttons=tuple(probe_buttons), probe_at=probe_at,
        sample_fn=lambda s: {"screen": screen_read(s)},
        sample_from=total_frames,
    )
    return trace[total_frames]["screen"]


def screen_probe_effect(
    session: LabSession,
    *,
    rig: Rig,
    script: MoveScript,
    total_frames: int,
    screen_read: Callable[[LabSession], bytes],
    probe_port: int,
    probe_buttons: Sequence[str],
    probe_at: int,
) -> int:
    """How many pixels of the crop the PROBE's own walk moves: one run of
    `script` with the walk asserted at `probe_at`, against the identical run
    without it.

    ## Why this, and not either of the two obvious alternatives

    The question is "is this fighter DRAWN", and there is no memory answer to
    it (mk2.md's invisibility search is an explicit negative). Two attempts
    failed before this one, and both failed in ways §4.2 already names:

      1. **Two stored reference screens** ("is this nearer the invisible pole
         or the visible one"). Sound for the positive case, useless for the
         negative one, because a VISIBLE fighter's crop depends on his pose
         and position: a run in which the probe cancelled the move came out
         4,840 px from one pole and 4,292 px from the other — a third state,
         and the classifier correctly refused to call it (that refusal is the
         only reason this is a paragraph rather than a wrong number).
      2. **Differencing the move against its own trigger** (the same script
         minus the motion steps). That measures whether the MOTION changed the
         screen, which it does whether or not the move came out: `[BLK] U U D`
         visibly crouches him. Measured: 4,254 px of "effect" in a run where
         the probe had already cancelled the move.

    What actually answers it is moving the fighter and seeing whether the
    picture notices. **If he is drawn, walking him changes his pixels; if he
    is not drawn, walking him changes nothing.** Measured on Reptile's
    `invisibility`, crop `(77,99)-(139,232)`: a walk asserted one frame after
    the move's input leaves the crop BIT-IDENTICAL (0 px, at capture points
    from +1 to +40 frames), while the same walk asserted three frames earlier —
    inside the HP's own hold, where it replaces the trigger — changes 4,840 px.

    The validity check comes free and is not optional: run the same
    displacement on a script where the fighter is KNOWN to be drawn (the
    trigger alone). If that is also ~0, the walk is not moving him and the
    crop witnesses nothing.
    """
    kw = dict(rig=rig, script=script, total_frames=total_frames,
              screen_read=screen_read)
    still = observe_screen(session, **kw)
    moved = observe_screen(session, probe_port=probe_port,
                           probe_buttons=tuple(probe_buttons),
                           probe_at=probe_at, **kw)
    return region_pixel_diff(still, moved)


def screen_preemption_scan(
    session: LabSession,
    *,
    rig: Rig,
    script: MoveScript,
    control_script: MoveScript,
    origin: int,
    n_range: Sequence[int],
    directions: Sequence[str],
    screen_read: Callable[[LabSession], bytes],
    capture_after: int,
    port: Optional[int] = None,
) -> Dict[int, Dict[str, int]]:
    """`preemption_scan` for a move the contact anchor cannot see.

    `{N: did the move still come out}` for a walk probe asserted at
    `origin + N`, classified against two reference screens instead of against
    the health register. Identical question, identical refusal rule; only the
    witness differs, because for a damageless move there is no other one —
    mk2.md's memory search for an invisibility flag is an explicit negative
    (188 candidate bytes, all of them in a sprite display list that REORDERS
    when any sprite leaves).

    Returns `{N: {"<direction>": px, "<direction>_drawn_control": px, ...}}` —
    RAW pixel counts, never a verdict (§7 forbids a classifier that reports
    only its own conclusion). Read them as:

      * `px == 0` for `script` — the walk moved the fighter and the picture did
        not notice, so he is NOT DRAWN and the move came out.
      * `px > 0` for `script` — he is drawn, so the probe cancelled the move.
      * `..._drawn_control` is the identical displacement on `control_script`
        (the trigger alone, where he is known to be drawn). It must be LARGE.
        If it is not, the walk is not moving him and neither number means
        anything — that is the witness's own validity check, measured per N
        rather than assumed once.

    **Every pair compared here is taken at the SAME frame count.** The round
    timer, the stage and the defender's idle animation all advance on their
    own, so two screens taken at different frame counts differ everywhere:
    measured, the same two runs compared at different lengths differ in 9,765
    pixels across the FULL WIDTH of the screen, and at the same length in
    4,268 pixels inside one 62x133 sprite box and nowhere else.

    **`capture_after` must be small.** The probe's hold does not end — a walk
    asserted at `origin + N` is still held at the capture — so a long tail
    walks the fighter out of a fixed crop, or scrolls the camera under it. A
    few frames is enough for a state question, and this is never asked a
    timing question (the framebuffer is not frame-exact on this transport).
    """
    port = rig.attacker_port if port is None else port
    if capture_after < 1:
        raise ScreenWitnessError(
            f"capture_after={capture_after} leaves no frame between the probe "
            "and the capture for the move to fail to come out in."
        )
    out: Dict[int, Dict[str, int]] = {}
    for n in n_range:
        end = origin + n + capture_after
        per: Dict[str, int] = {}
        for d in directions:
            for label, scr in ((d, script),
                               (f"{d}_drawn_control", control_script)):
                per[label] = screen_probe_effect(
                    session, rig=rig, script=scr, total_frames=end,
                    screen_read=screen_read, probe_port=port,
                    probe_buttons=(d,), probe_at=origin + n)
        out[n] = per
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
    # shape ("attacker/hit", "defender/block", ...) -> why that ONE sweep was
    # refused. §4.3: `on_hit` and `on_block` "are separate columns and MUST
    # NOT be derived from each other", which cuts both ways -- a refusal on
    # one outcome is not a reason to throw the other one away.
    refusals: Dict[str, str] = field(default_factory=dict)
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
      * `"input_end+1"` — a projectile that is not a charge (Reptile's two).
        Same arithmetic, different claim; see `ATTACKER_ORIGIN_KINDS`.
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

    origin_att = attacker_origin(script, attacker_origin_kind, anchor_hit)

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

    # The BLOCK pass gets the SAME attacker clock as the hit pass, recomputed
    # against its own contact frame. An earlier draft anchored the blocked
    # attacker on contact unconditionally; that is only harmless while the
    # attacker's recovery happens to land AFTER contact. For a projectile
    # thrown from far enough away it does not — the thrower is free while the
    # ball is still travelling — and a contact-anchored sweep cannot return a
    # negative N, so it would have reported `first_true = 0` and made
    # `on_block` too positive by however long he had already been free.
    passes: List[Tuple[bool, int, Optional[int]]] = [(False, anchor_hit, origin_att)]
    if m.contact_block is not None:
        passes.append((
            True, m.contact_block,
            attacker_origin(script, attacker_origin_kind, m.contact_block),
        ))

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
             attacker_origin_kind,
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
            # One shape's refusal is recorded and the rest of the cell
            # continues. Measured need: `force_ball` LAUNCHES its victim onto
            # the attacker, and the anti-overlap separation that follows makes
            # the ATTACKER'S OWN act-again predicate genuinely non-monotone at
            # the close rungs -- while the blocked rig, where nobody is
            # launched, sweeps cleanly. Aborting the cell on the hit pass threw
            # away an `on_block` that was never in doubt, which is the same
            # mistake in miniature as deriving one column from the other.
            try:
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
            except ProbeError as exc:
                m.refusals[shape] = str(exc)
                m.notes.append(f"{script.name}: {shape} REFUSED -- {exc}")

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
        hit_ok = ("attacker/hit" in m.manifests and "defender/hit" in m.manifests)
        if m.obs_hit.knockdown:
            # §1.1: a knockdown has a WAKEUP clock, not an advantage. The probe
            # will happily return one and it is meaningless. The wakeup window
            # needs only the DEFENDER's side, so an attacker-side refusal does
            # not cost it.
            m.on_hit[o] = None
            dfn = m.manifests.get("defender/hit", {}).get(o)
            m.wakeup_window[o] = (
                None if dfn is None or dfn.manifest is None
                else dfn.manifest - anchor_hit
            )
        elif hit_ok:
            m.on_hit[o] = advantage_between(
                m.manifests["attacker/hit"][o], m.manifests["defender/hit"][o]
            )
        else:
            m.on_hit[o] = None
        if "attacker/block" in m.manifests and "defender/block" in m.manifests:
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
    guard_height: Optional[str] = None,
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
        "guard_height": guard_height,
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

# The same table for Reptile, from this module's own live run (task Q2) rather
# than from M1's audit -- M1 measured these at ONE gap and reported damage; the
# fields below add the two discriminators M1 had no rig for (the victim's own
# `y`, and the attacker's travel), which are what separate the slide from the
# crouching normal the same chord degenerates into.
#
# `acid_spit` carries no `crossed`/`knockdown` beyond what was measured: it is
# the move whose PREVIOUS "verification" was a close HP normal wearing a
# projectile's name (mk2.md, retracted 2026-08-30), so its signature is checked
# at gaps where no normal of Reptile's reaches (his longest, HK/LK, stops at
# 110 px) and the damage that discriminates them is 15 vs the normals' 8/11/24.
MK2_REPTILE_SIGNATURES: Dict[str, Signature] = {
    "acid_spit": Signature(damage=15, hits=1, crossed=False,
                           victim_knockdown=False),
    "force_ball": Signature(damage=16, hits=1, victim_knockdown=True),
    # The slide's travel threshold is deliberately WELL BELOW what it covers
    # at the far rungs: it stops when it connects, so the same move travels
    # 112 px from 182 px away and 62 px from 107 px away. A threshold tuned at
    # one rung refuses the same move at another (measured: 100 px passed at
    # gap-0 and refused the identical 13-damage slide at gap-30). What it has
    # to discriminate is the crouching normal the chord degenerates into,
    # which travels ~0 px.
    "slide": Signature(damage=13, hits=1, crossed=False, victim_knockdown=True,
                       min_attacker_travel_px=40),
    # `invisibility` deliberately has NO signature: it deals no damage, so
    # there is nothing for `check_signature` to check and `measure_special`
    # would report it as the whiff it is. Its witness is the framebuffer
    # (`screen_preemption_scan`), and its number is a `WhiffRecovery`.
}

MK2_SIGNATURES: Dict[str, Dict[str, Signature]] = {
    "mileena": MK2_MILEENA_SIGNATURES,
    "reptile": MK2_REPTILE_SIGNATURES,
}

# Frames of neutral between a ladder rung's walk-in and the charge. A fighter
# that is still decelerating is not a fighter at rest (kit.py's argument for
# not walking in at all); the sai's ladder cannot avoid the walk, so it pauses
# after it and records that it did.
SAI_LADDER_SETTLE = 3

# Frames of neutral inserted before a move's own input when the rung's ARENA
# was saved mid-walk. This is not padding either, and it is not the same
# constant as `SAI_LADDER_SETTLE`: measured on Reptile's `gap-*` ladder (saved
# without settle frames, docs/frames.md §5's own warning), `acid_spit` produced
# NO contact at all at 147 px and a plain HP NORMAL at 110/72/62 px, because
# the residual walk leaves `forward` already latched and the macro's first tap
# is therefore not a fresh onset (mk2.md M1: "the direction must be down at
# least one frame BEFORE the press"). With 10 neutral frames first, the same
# script produced the acid spit at all six rungs. The lead-in is replayed
# identically in probe and control, so it cancels out of the differential
# exactly as a walk-in ladder's walk does.
LADDER_SETTLE = 10


def main() -> None:  # pragma: no cover - the live-rig path
    """Measure a character's SPECIALS. Never point this at port 4025
    (CLAUDE.md: the user's session).

        python -m shadow_train.framelab.specials \\
            --url http://127.0.0.1:4068/mcp --game library/mk2 \\
            --core ../FBNeo/.../fbneo_libretro.dylib --rom ~/games/roms/mk2.zip \\
            --arena shadow/arenas/mk2/m-v-r.state --char mileena \\
            --move teleport_kick --move roll \\
            --move sai_throw --rung 0 --rung 20 --rung 33 \\
            --charge-probe --report specials.json

    Reptile's kit (task Q2) uses the ARENA ladder rather than walk-in rungs,
    and needs `--settle` because those arenas were saved mid-walk:

        python -m shadow_train.framelab.specials ... --char reptile \\
            --arena shadow/arenas/mk2/gap-0.state \\
            --arena shadow/arenas/mk2/gap-30.state --settle 10 \\
            --move acid_spit --move force_ball --move slide \\
            --guard-height slide --origin-check \\
            --whiff-move invisibility --screen-region 225,77,313,240
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
    ap.add_argument("--arena", action="append", required=True,
                    help="repeatable: one saved arena per ladder rung (§5). "
                         "The charge probe uses the first.")
    ap.add_argument("--char", default="mileena")
    ap.add_argument("--move", action="append", default=[])
    ap.add_argument("--rung", action="append", type=int, default=[],
                    help="sai_throw only: walk-in frames before the charge")
    ap.add_argument("--settle", type=int, default=0,
                    help=f"neutral frames before the move's own input, for "
                         f"arenas saved mid-walk ({LADDER_SETTLE} on Reptile's "
                         "gap-* ladder -- see LADDER_SETTLE)")
    ap.add_argument("--projectile", action="append", default=[],
                    help="this move's hitbox is a separate object, so the "
                         "attacker's clock starts at the END OF HIS INPUT, not "
                         "at contact (`input_end+1`). Declared, then CHECKED by "
                         "--origin-check where both origins are usable.")
    ap.add_argument("--guard-height", action="append", default=[],
                    help="also measure guard_height for this move")
    ap.add_argument("--crouch-dir", default="down",
                    help="the direction that makes the guard stance crouching")
    ap.add_argument("--origin-check", action="store_true",
                    help="for every move whose attacker clock is not `contact`, "
                         "re-sweep the attacker from the CONTACT origin too and "
                         "require the absolute manifests to agree "
                         "(`origin_invariance`)")
    ap.add_argument("--whiff-move", action="append", default=[],
                    help="a damageless move: measure its whiff-anchored "
                         "recovery instead of an advantage")
    ap.add_argument("--screen-region", default=None,
                    help="x0,y0,x1,y1 -- the framebuffer crop that witnesses a "
                         "damageless move (no profile carries a sprite box)")
    ap.add_argument("--screen-settle", type=int, default=6,
                    help="frames AFTER the probe at which the screen is read. "
                         "Small on purpose: the probe's walk is still held, and "
                         "a long tail scrolls the camera out from under the "
                         "crop (see screen_preemption_scan).")
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
    arenas = list(args.arena)
    first_arena = arenas[0]
    session.load_state(first_arena)
    facing = facing_from_x(reads["attacker_x"](session), reads["victim_x"](session))
    if facing is None:
        raise SystemExit(
            "cannot derive the attacker's facing from the arena's positions "
            "(docs/frames.md §5: facing is NULL, never a guess) -- refusing."
        )
    guard_chord = tuple(prof.attack_chords["Block"])

    def make_rig(arena: str) -> Rig:
        return Rig(arena=arena, attacker_port=0, defender_port=1,
                   guard_buttons=guard_chord, quiet_frames=flspec.quiet_frames)

    def rung_of(arena: str) -> Optional[int]:
        """`gap_walk_frames` from the arena's own `.gap.json` sidecar, so a
        special's row keys onto the same ladder grid the normals use. NULL
        when the arena has no sidecar -- never a guessed 0 (§2.5)."""
        side = Path(arena).with_suffix(".gap.json")
        if not side.exists():
            return None
        try:
            return int(json.loads(side.read_text()).get("walk_frames"))
        except (ValueError, TypeError):
            return None

    def settle_lead() -> Tuple[ScriptStep, ...]:
        return (ScriptStep(frames=args.settle, buttons=()),) if args.settle else ()

    rig = make_rig(first_arena)
    core_id, rom_id = compute_core_id(args.core), compute_rom_id(args.rom)
    report: Dict[str, Any] = {"arenas": arenas, "facing": facing,
                              "settle": args.settle,
                              "core_id": core_id, "rom_id": rom_id,
                              "observables": observables, "moves": [],
                              "guard_height": [], "whiff_recovery": [],
                              "origin_invariance": [], "rows": []}

    if args.charge_probe:
        Path(args.scratch).mkdir(parents=True, exist_ok=True)
        chord = prof.attack_chords["HP"]
        threshold = int(
            special_encoding(prof, args.char, args.charge_move)[0]["min_frames"]
        )
        cp = charge_persistence(
            session, arena=first_arena, port=0, chord=chord, banked_frames=20,
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

    # ── guard_height first (§6's column, NULL in every row so far) ───────
    # Measured BEFORE the advantage rows so a row can carry the verdict
    # instead of the operator having to join two reports by hand.
    guard_by_cell: Dict[Tuple[str, str], Optional[str]] = {}
    for move in args.guard_height:
        for arena in arenas:
            script = special_script(prof, args.char, move, facing=facing,
                                    lead_in=settle_lead(), name=move)
            gh = measure_guard_height(
                session, rig=make_rig(arena), script=script,
                contact_read=contact_read, reads=reads,
                stances={"standing": guard_chord,
                         "crouching": guard_chord + (args.crouch_dir,)},
            )
            guard_by_cell[(move, arena)] = gh.verdict
            entry = {"move": move, "arena": arena, "rung": rung_of(arena),
                     "unguarded_damage": gh.unguarded_damage,
                     "verdict": gh.verdict, "note": gh.note,
                     "trials": [t.__dict__ for t in gh.trials]}
            print("guard-height:", json.dumps(entry, default=str))
            report["guard_height"].append(entry)

    signatures = MK2_SIGNATURES.get(args.char, {})
    walk_rungs = args.rung or [0]
    for move in args.move:
        expect = signatures.get(move, Signature())
        # Two ladder shapes, and which one applies is a property of the MOVE.
        # A charge cannot use saved arenas at all (walking resets it -- M5), so
        # it walks in inside one replay; everything else uses the arenas, which
        # is §5's own recipe and does not accumulate momentum.
        is_walk_ladder = move == args.charge_move and args.rung
        rungs: List[Tuple[str, Optional[int], int]] = (
            [(first_arena, k, k) for k in walk_rungs] if is_walk_ladder
            else [(a, rung_of(a), 0) for a in arenas]
        )
        for arena, k, walk_in in rungs:
            rig = make_rig(arena)
            lead: Tuple[ScriptStep, ...] = settle_lead()
            if walk_in:
                lead = (ScriptStep(frames=walk_in, buttons=(facing,)),
                        ScriptStep(frames=SAI_LADDER_SETTLE, buttons=()))
            script = special_script(prof, args.char, move, facing=facing,
                                    lead_in=lead, name=move)
            enc = special_encoding(prof, args.char, move)
            if any("hold" in s for s in enc):
                kind = "release+1"
            elif move in args.projectile:
                kind = "input_end+1"
            else:
                kind = "contact"
            # A REFUSAL is a result about that cell, not a reason to abandon
            # the ladder (§7's "no silent caps" has a converse: a run that
            # dies on one cell silently drops every cell after it). It is
            # recorded with its message and the run continues.
            try:
                m = measure_special(
                    session, rig=rig, script=script, observables=observables,
                    sample_fns=sample_fns, contact_read=contact_read, reads=reads,
                    expect=expect, attacker_origin_kind=kind,
                )
            except ProbeError as exc:
                entry = {"move": move, "arena": arena, "rung": k,
                         "origin_kind": kind, "refused": type(exc).__name__,
                         "refusal": str(exc)}
                print("REFUSED:", json.dumps(entry, default=str))
                report["moves"].append(entry)
                continue
            m.gap_walk_frames = k
            for note in m.notes:
                print("NOTE:", note)
            print(f"{move} arena={arena} K={k} gap={m.gap_px}px "
                  f"contact={m.contact_hit} "
                  f"hits={m.hits} dmg={m.obs_hit.damage if m.obs_hit else None} "
                  f"on_hit={m.on_hit} on_block={m.on_block} "
                  f"wakeup={m.wakeup_window}")
            report["moves"].append({
                "move": move, "arena": arena, "rung": k,
                "origin_kind": kind, "gap_px": m.gap_px,
                "contact_hit": m.contact_hit, "contact_block": m.contact_block,
                "hits": m.hits, "damage": m.obs_hit.damage if m.obs_hit else None,
                "signature_problems": list(m.signature_problems),
                "excluded_n": list(m.excluded_n),
                "refusals": m.refusals,
                "latencies": m.latencies,
                "cal_points": {k: list(v) for k, v in m.cal_points.items()},
                "manifests": {
                    shape: {o: {"origin": sm.origin, "origin_kind": sm.origin_kind,
                                "first_true": sm.sweep.first_true,
                                "window": sm.sweep.window,
                                "direction": sm.sweep.direction,
                                "rejected_directions": list(sm.rejected_directions),
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
            if m.obs_hit is None or m.signature_problems or not m.latencies:
                continue
            for o in observables:
                report["rows"].append(special_row(
                    family=prof.family, port=prof.port, char=args.char, move=move,
                    core_id=core_id, rom_id=rom_id, observable=o,
                    method="linear_sweep",
                    input_latency_frames=(m.latencies.get("attacker/hit")
                                          or m.latencies["attacker/block"])[o],
                    obs_hit=m.obs_hit, on_hit=m.on_hit.get(o),
                    on_block=m.on_block.get(o),
                    wakeup_window=m.wakeup_window.get(o),
                    gap_px=m.gap_px, gap_walk_frames=k,
                    guard_height=guard_by_cell.get((move, arena)),
                    variant=(f"walk-in-{walk_in}" if walk_in else None),
                    sample_n=args.sample_n, confidence="high",
                ))

            # §8.4-style validation of the ORIGIN itself: re-sweep the
            # attacker from the contact anchor and require the same absolute
            # manifest. Only meaningful where the declared origin is not
            # already `contact`.
            if args.origin_check and kind != "contact" and m.contact_hit:
                origin_att = attacker_origin(script, kind, m.contact_hit)
                end_frame = max(origin_att, m.contact_hit) + 90
                entry: Dict[str, Any] = {
                    "move": move, "arena": arena, "rung": k,
                    "origins": [[origin_att, kind], [m.contact_hit, "contact"]],
                    "end_frame": end_frame,
                }
                try:
                    entry["manifests"] = origin_invariance(
                        session, rig=rig, script=script, who="attacker", port=0,
                        origins=[(origin_att, kind), (m.contact_hit, "contact")],
                        end_frame=end_frame, observables=observables,
                        sample_fn=sample_fns[0],
                        input_latency_frames=m.latencies["attacker/hit"],
                        defender_guard=False,
                        walk_directions=walk_directions_after(
                            m.obs_hit.attacker_x[1], m.obs_hit.victim_x[1]),
                        excluded_n={kind: m.excluded_n},
                    )
                    entry["agreed"] = True
                except ProbeError as exc:
                    entry["agreed"] = False
                    entry["refusal"] = str(exc)
                print("origin-invariance:", json.dumps(entry, default=str))
                report["origin_invariance"].append(entry)

    # ── a damageless move: recovery, and the witness that it came out ────
    for move in args.whiff_move:
        arena = first_arena
        rig = make_rig(arena)
        script = special_script(prof, args.char, move, facing=facing,
                                lead_in=settle_lead(), name=move)
        witness = ""
        entry = {"move": move, "arena": arena, "rung": rung_of(arena),
                 "input_end": script.total_frames}
        if args.screen_region:
            region = tuple(int(v) for v in args.screen_region.split(","))
            screen_read = make_screen_region_read(session.client, region=region)
            # The control is the SAME script minus the move's own motion
            # steps: the trigger alone, which M1's 64-encoding sweep showed
            # does not produce the move. Everything else -- lead-in, length,
            # probe -- is identical, so the difference is the motion's effect.
            control = MoveScript(
                name=f"{move}-trigger-only",
                steps=(script.steps[-1],), lead_in=script.lead_in,
            )
            cap = args.screen_settle
            entry["capture_after"] = cap
            scan = screen_preemption_scan(
                session, rig=rig, script=script, control_script=control,
                origin=script.total_frames + 1, n_range=(0, 1, 2),
                directions=("left", "right"), screen_read=screen_read,
                capture_after=cap,
            )
            entry["screen_probe_effect_px"] = scan
            # The negative control: a probe asserted INSIDE the trigger's own
            # hold replaces the trigger, so the move does NOT come out and the
            # fighter stays drawn -- the displacement must become LARGE. If it
            # does not, the crop is not witnessing the move.
            inside = script.total_frames - script.steps[-1].frames
            entry["probe_inside_trigger_px"] = screen_preemption_scan(
                session, rig=rig, script=script, control_script=control,
                origin=inside, n_range=(0,), directions=("left",),
                screen_read=screen_read, capture_after=cap)
            witness = (
                f"framebuffer crop {region}, captured {cap} frames after the "
                f"probe. Pixels the probe's own walk moves (0 == the fighter "
                f"is not drawn == the move came out; the `_drawn_control` "
                f"figure is the same displacement where he is known to be "
                f"drawn): at probe N {scan}; with the probe INSIDE the "
                f"trigger's hold (frame {inside}) -> "
                f"{entry['probe_inside_trigger_px']}"
            )
        wr = measure_whiff_recovery(
            session, rig=rig, script=script, port=0, observables=observables,
            sample_fn=sample_fns[0], contact_read=contact_read, witness=witness,
        )
        entry.update({
            "origin": wr.origin, "origin_kind": wr.origin_kind,
            "latencies": wr.latencies, "cal_points": list(wr.cal_points),
            "first_true": {o: wr.first_true(o) for o in observables},
            "manifest": {o: wr.sweeps[o].manifest for o in observables},
            "total": {o: wr.total(o) for o in observables},
            "predicate": {o: "".join("T" if v else "F"
                                     for v in wr.sweeps[o].sweep.predicate)
                          for o in observables},
            "witness": wr.witness,
        })
        print("whiff-recovery:", json.dumps(entry, default=str))
        report["whiff_recovery"].append(entry)

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
