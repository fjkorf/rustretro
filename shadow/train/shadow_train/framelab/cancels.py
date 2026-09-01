"""Is a move CANCELLABLE — can a second action start before the first one's
own recovery would allow?

docs/frames.md §1 measures when a fighter becomes ACTIONABLE, defined as "the
earliest frame it can start a WALK". That definition is deliberate and it is
also incomplete: §1 already records that GUARD returns before the walk does.
This module measures the third thing on that list — the earliest frame an
ATTACK can start — and it turns out to be a different number again.

## The question, stated so it cannot be answered by accident

"Move B cancels move A" means: **B starts sooner than A's own recovery
permits.** That is a comparison, not an observation, and it needs a FLOOR to
compare against. A measurement that only shows "B came out after A" has shown
a LINK, which is the null hypothesis, not a finding.

So every verdict here is a difference between two numbers measured in the
SAME rig:

* `onset(N)` — the frame the follow-up manifests when its input is asserted
  at frame `N` after the lead move's own press.
* `control(N)` — the same, with the lead move's button removed and nothing
  else changed. This is the follow-up's own unimpeded schedule.

`delay(N) = onset(N) - control(N)`. A LINK is forced to have `delay(N) > 0`
for every `N` below the lead's recovery — the follow-up cannot start until the
fighter is released, so its onset is CLAMPED and the clamp is visible as a
positive delay that shrinks to zero as `N` rises. A CANCEL has `delay(N) == 0`
at an `N` where a gated action is still clamped.

## The gate is the other half, and it is what makes this honest

`delay(N) == 0` on its own proves nothing: a fighter that is simply FREE at
frame `N` also shows zero delay, and then "cancel" is just a slow move being
described dramatically. So a verdict requires a GATE — an action measured in
the same rig, at the same `N`, that is demonstrably NOT available yet:

* `walk_floor` — the §1 actionability probe's answer for the lead move.
* `normal_floor` — the earliest `N` at which a follow-up NORMAL comes out.

A cancel verdict is only issued when the follow-up is undelayed strictly below
a gate that is itself closed. Both gates are measured, never assumed, and
`CancelVerdict` records which one it cleared, so a reader can see the
comparison rather than take the word "cancel" on trust.

## Why the follow-up is identified by SIGNATURE, not by "a button was pressed"

MK2 arcade has no animation-id field (`library/mk2/mk2.md`, "General
action/state-id word" — a genuine negative result), and `action_counter`
(`block+0xC0`) is disqualified: it fires on ENTERING an action but cannot say
WHICH, and mk2.md records it firing identically for a roll and for the
crouching normal a failed roll degenerates into. This module therefore never
asks "did something start"; it asks for the follow-up's own measured
signature — a damage step, a contact frame, or an x displacement no other
action produces — exactly like `specials.check_signature`.

The `lead_landed` flag exists for the same reason and guards the same
retraction: the project has already once credited a NORMAL's damage to a
special (`acid_spit`). A trial where the lead move never connected is NOT a
cancel of that move; it is a trial in which the follow-up simply replaced it.
Those trials are kept and reported separately (`overrides`), never folded into
the cancel window.

## Two claims that look like one, and are not (the `startup` half)

"Cancelling works" can mean either of two things, and conflating them is the
easiest mistake in this file:

* **(A) earlier permission, unchanged startup.** The special's input is
  accepted sooner than a normal's would be, but from its own TRIGGER PRESS it
  runs its normal course. Cancelling buys the lead's recovery, nothing more.
* **(B) shortened startup.** The special reaches its active frame in FEWER
  frames from its trigger press when cancelled. The cancel would be skipping
  part of the special's own startup.

`delay(N)` above cannot separate them on its own, because it is measured
against a control that shares the schedule: it answers "was the special held
up", not "was the special's own clock shortened". `StartupArm` /
`compare_startup` answer the second question directly, by measuring
`onset - trigger_frame` in each lead condition and comparing the values.

**Measured on MK2 arcade rev L3.1: (A), with no exception found.** Reptile's
slide is `trigger + 3` (attacker `pointer_x`) and `trigger + 11` (defender
contact at 72 px) in EVERY condition measured — no lead, a punch lead on hit,
a punch lead on whiff, a kick lead on hit, a kick lead on block, a kick lead
on whiff, at five gaps, and again from a cold second emulator process.
`force_ball` agrees: `trigger + 31` at a 76 px gap with and without a punch
lead, `trigger + 56` at a 188 px gap with and without a whiffing kick lead.

**The trigger press is the origin, never the macro start.** `force_ball` is
`B · B+HP+LP`: its trigger lands 5 frames after the macro begins. Measured
from the macro start the two specials look like different mechanisms;
measured from the trigger they agree on the same gate frame to the frame.

## Hitstop is NOT bypassed, and that is what the gate shift shows

`hitstop_shift` compares the same lead's gate on CONTACT against its gate on
WHIFF. On MK2 the two differ by exactly the lead's measured hitstop — far HK
and far LK (hitstop 12) gate the slide at trigger frame 33 whiffing and 45
connecting, at two whiff gaps (147/180 px) and two contact gaps (72/110 px),
so gap is excluded and contact is the cause. A BLOCKED far HK gates at 45 as
well, which is what hitstop should do (§1.2: it fires on contact, hit or
block) and is not what a hit-only reaction would do.

The startup is unchanged across that same shift: the special starts LATER,
never SLOWER.

## What this module deliberately does not do

It does not write to the frame store. `store.MOVE_FRAMES_COLUMNS` has no
cancel column, and a per-move `cancellable` boolean would be the wrong shape
for what was actually found on MK2 (a global button-class rule, not a per-move
property). Adding a column is a schema decision that needs its own evidence
pass, not a side effect of a measurement run.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Callable, Dict, Hashable, List, Mapping, Optional, Sequence, Tuple

from .probe import MoveScript, ProbeError, ScriptStep, replay
from .session import LabSession

__all__ = [
    "CancelError",
    "ControlDriftError",
    "StartupError",
    "Trial",
    "Gate",
    "CancelSweep",
    "CancelVerdict",
    "StartupArm",
    "StartupVerdict",
    "HitstopShift",
    "classify",
    "arm_from_sweep",
    "compare_startup",
    "hitstop_shift",
    "onset_from_trace",
    "lead_in_for",
    "trigger_frame",
    "measure_trial",
    "sweep_cancel",
]

# `ScriptStep.buttons` is the port's whole held set, so a step is a DIRECTION
# step exactly when every button in it is one of these.
DIRECTIONS = frozenset({"up", "down", "left", "right"})


class CancelError(ProbeError):
    """A cancel measurement that must refuse rather than report."""


class ControlDriftError(CancelError):
    """The control condition did not behave like an unimpeded move.

    The whole method rests on `control(N)` being the follow-up's own schedule,
    which means it must advance one frame per frame of `N`. If it does not,
    the follow-up is being influenced by something the rig is not modelling
    and every `delay` computed against it is meaningless.
    """


# ── one trial ────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class Trial:
    """One `N`: the follow-up's input begins `n` frames after the lead's press.

    `onset` and `control_onset` are ABSOLUTE replay frames (frame 0 = the
    loaded state), both nullable — a follow-up that never manifested is NULL,
    never 0 (docs/frames.md §2.5).

    `lead_landed` answers "did the lead move still connect", which is what
    separates a cancel (both actions happen) from an OVERRIDE (the follow-up
    replaced the lead before it became active). Both are real behaviours and
    they are reported separately.
    """

    n: int
    onset: Optional[int]
    control_onset: Optional[int]
    lead_landed: Optional[bool] = None
    contacts: Tuple[int, ...] = ()
    damage: Optional[int] = None

    @property
    def came_out(self) -> bool:
        return self.onset is not None

    @property
    def delay(self) -> Optional[int]:
        """Frames the lead move cost the follow-up. NULL if either side is
        NULL — "the follow-up never came out" is not a delay of 0."""
        if self.onset is None or self.control_onset is None:
            return None
        return self.onset - self.control_onset

    @property
    def undelayed(self) -> bool:
        return self.delay == 0


@dataclass(frozen=True)
class Gate:
    """An action measured in the SAME rig that is NOT available at low `N`.

    A cancel verdict is a comparison against one of these. `floor` is the
    earliest `n` at which the gated action came out; `kind` names it so the
    verdict can say what was cleared rather than asserting a bare adjective.
    """

    kind: str
    floor: Optional[int]
    evidence: str = ""


@dataclass
class CancelSweep:
    """Every trial for one (lead, follow-up) pair at one rig."""

    lead: str
    follow: str
    arena: str
    gap_px: Optional[int]
    trials: List[Trial] = field(default_factory=list)

    def by_n(self) -> Dict[int, Trial]:
        return {t.n: t for t in self.trials}

    @property
    def undelayed_from(self) -> Optional[int]:
        """The smallest `n` from which EVERY trial is undelayed. Requiring the
        whole tail rather than a single `n` is what stops one flaky frame from
        creating a window: `specials`' `preemption_scan` learned the same
        lesson about isolated TRUEs."""
        ns = sorted(t.n for t in self.trials)
        run: Optional[int] = None
        for n in reversed(ns):
            t = self.by_n()[n]
            if t.undelayed:
                run = n
            else:
                break
        return run

    @property
    def overrides(self) -> Tuple[int, ...]:
        """The `n` at which the follow-up came out but the lead did NOT land."""
        return tuple(t.n for t in self.trials if t.came_out and t.lead_landed is False)

    @property
    def both_landed_from(self) -> Optional[int]:
        ns = [t.n for t in self.trials if t.undelayed and t.lead_landed]
        return min(ns) if ns else None


@dataclass(frozen=True)
class CancelVerdict:
    """`cancel` | `link` | `no-followup`, plus the numbers behind it."""

    lead: str
    follow: str
    verdict: str
    undelayed_from: Optional[int]
    both_landed_from: Optional[int]
    gate: Optional[Gate]
    margin: Optional[int]
    overrides: Tuple[int, ...] = ()
    note: str = ""


def classify(
    sweep: CancelSweep, gates: Sequence[Gate], *, min_margin: int = 2
) -> CancelVerdict:
    """The whole verdict, as a pure function of measured numbers.

    Pure on purpose: `active.window_from_trials` and `hitstop.compute_hitstop`
    are the precedent, and the reason is that the logic that turns frames into
    a WORD is the part most worth unit-testing without an emulator.

    Rules, in order:

    1. No trial produced the follow-up at all → `no-followup`. Not a link and
       not a cancel; the pair was not measured.
    2. Refuse (`ControlDriftError`) if the control onsets are not strictly
       increasing with `n` — see that exception's docstring.
    3. Take the strictest CLOSED gate (the largest `floor`). If the follow-up
       is undelayed at some `n` strictly below it → `cancel`, with
       `margin = floor - undelayed_from`.
    4. Otherwise → `link`.

    A gate with a NULL floor is a gate that never opened in the measured
    range; it cannot bound anything and is skipped rather than treated as
    infinite.

    ## `min_margin`, and why a 1-frame cancel is not reported as one

    The gate and the follow-up are, in general, measured through DIFFERENT
    observables — a walk gate through `pointer_x`, a follow-up attack through
    the contact anchor — and docs/frames.md §8.4 is explicit that two
    observables measuring the same one-sided truth legitimately differ by the
    difference in their `input_latency_frames`. A margin of one or two frames
    is therefore inside the units slop and cannot support the word "cancel".

    This is not hypothetical: the first live run classified MK2's
    `HP -> LK` as a cancel on a margin of 1 (attacks return at N=19, walking
    at N=20), which is exactly the artefact this threshold exists to catch. A
    sub-threshold margin returns `inconclusive`, which is a result — never
    silently rounded to either answer.
    """
    if not sweep.trials:
        raise CancelError(f"{sweep.lead}->{sweep.follow}: no trials")

    ordered = sorted(sweep.trials, key=lambda t: t.n)
    ctl = [(t.n, t.control_onset) for t in ordered if t.control_onset is not None]
    for (n0, c0), (n1, c1) in zip(ctl, ctl[1:]):
        if c1 - c0 != n1 - n0:
            raise ControlDriftError(
                f"{sweep.lead}->{sweep.follow}: control onset moved {c1 - c0} "
                f"frames while N moved {n1 - n0} (N={n0}->{n1}). The control "
                "is supposed to be the follow-up's own unimpeded schedule; if "
                "it is not, every delay measured against it is meaningless."
            )

    if not any(t.came_out for t in ordered):
        return CancelVerdict(
            lead=sweep.lead, follow=sweep.follow, verdict="no-followup",
            undelayed_from=None, both_landed_from=None, gate=None, margin=None,
            note="the follow-up never manifested at any N in the swept range",
        )

    ufrom = sweep.undelayed_from
    closed = [g for g in gates if g.floor is not None]
    strictest = max(closed, key=lambda g: g.floor) if closed else None

    if ufrom is not None and strictest is not None and ufrom < strictest.floor:
        margin = strictest.floor - ufrom
        inconclusive = margin < min_margin
        return CancelVerdict(
            lead=sweep.lead, follow=sweep.follow,
            verdict="inconclusive" if inconclusive else "cancel",
            undelayed_from=ufrom, both_landed_from=sweep.both_landed_from,
            gate=strictest, margin=margin,
            overrides=sweep.overrides,
            note=(f"undelayed from N={ufrom}; {strictest.kind} still closed "
                  f"until N={strictest.floor}"
                  + (f" -- margin {margin} < min_margin {min_margin}, inside "
                     "the cross-observable slop (docs/frames.md §8.4)"
                     if inconclusive else "")),
        )

    return CancelVerdict(
        lead=sweep.lead, follow=sweep.follow, verdict="link",
        undelayed_from=ufrom, both_landed_from=sweep.both_landed_from,
        gate=strictest, margin=None, overrides=sweep.overrides,
        note=("the follow-up is never undelayed below a closed gate — every "
              "connection is explained by the lead move having finished"),
    )


# ── (A) vs (B): the special's OWN startup ────────────────────────────────


class StartupError(CancelError):
    """A startup comparison that must refuse rather than report.

    The commonest cause is a startup that is not CONSTANT across the trials of
    one arm. For a projectile timed by the defender's health step that means
    the target moved — `force_ball` after a connecting far HK reads 43…46
    because the HK threw the defender 83 px downrange, so the number is travel
    time, not startup. Averaging it would publish a fabricated frame count
    (§7: "a number that fails re-measurement is DELETED, not averaged").
    """


def trigger_frame(script) -> int:
    """The frame the follow-up's TRIGGER is pressed — the last step in the
    macro that presses an attack button, not the frame the macro starts.

    `MoveScript.attack_input_frame` is the macro START, which for a multi-step
    special is a different instant: `force_ball` is `B · B+HP+LP` and its
    trigger lands 5 frames later. Two specials with different input lengths
    only agree about anything when both are measured from here.
    """
    frame = script.attack_input_frame
    found: Optional[int] = None
    for step in script.steps:
        if any(b not in DIRECTIONS for b in step.buttons):
            found = frame
        frame += step.frames
    if found is None:
        raise StartupError(
            f"{script.name}: no step presses an attack button, so the move has "
            "no trigger frame to measure from"
        )
    return found


@dataclass(frozen=True)
class StartupArm:
    """One LEAD CONDITION's answer to "when can the special start, and how
    long does it then take".

    The two numbers are deliberately separate, because they are the two claims
    §1's cancel clause has to keep apart:

    * `gate` — the earliest TRIGGER frame at which the special comes out at
      all. This is permission.
    * `startup` — `onset - trigger`, the special's own clock. This is speed.

    `outcome` and `hitstop` describe the LEAD, not the follow-up, and they are
    what makes `hitstop_shift` a comparison rather than an assertion.
    """

    lead: str
    outcome: str  # "none" | "hit" | "block" | "whiff"
    gate: Optional[int]
    startups: Tuple[int, ...] = ()
    hitstop: Optional[int] = None
    gap_px: Optional[int] = None
    observable: str = ""
    note: str = ""

    @property
    def startup(self) -> Optional[int]:
        """The arm's single startup value, or NULL if it never came out.

        Raises `StartupError` when the trials disagree — see that exception.
        """
        if not self.startups:
            return None
        distinct = sorted(set(self.startups))
        if len(distinct) > 1:
            raise StartupError(
                f"{self.lead}/{self.outcome}: startup is not constant across "
                f"the arm ({distinct}). Either the observable is contaminated "
                "(a projectile timed by contact against a target the lead "
                "displaced) or the arm mixes two different moves; it is not a "
                "number to average."
            )
        return distinct[0]


def arm_from_sweep(
    sweep: CancelSweep,
    *,
    outcome: str,
    triggers: Mapping[int, int],
    hitstop: Optional[int] = None,
    observable: str = "",
) -> StartupArm:
    """Collapse a `CancelSweep` into one `StartupArm`.

    `triggers` maps each trial's `n` to that trial's TRIGGER frame, because
    only the caller knows the follow-up's encoding. The gate is the smallest
    `n` whose trigger begins an UNBROKEN tail of trials that came out — the
    same whole-tail rule as `undelayed_from`, and for the same reason: an
    isolated success below a clean floor is a flake, not a window.
    """
    ordered = sorted(sweep.trials, key=lambda t: t.n)
    gate_n: Optional[int] = None
    for t in reversed(ordered):
        if t.came_out:
            gate_n = t.n
        else:
            break
    startups: List[int] = []
    for t in ordered:
        if t.came_out and gate_n is not None and t.n >= gate_n:
            tg = triggers.get(t.n)
            if tg is None:
                raise StartupError(
                    f"{sweep.lead}->{sweep.follow}: no trigger frame recorded "
                    f"for N={t.n}; startup cannot be measured from the macro "
                    "start (see `trigger_frame`)"
                )
            startups.append(t.onset - tg)  # type: ignore[operator]
    return StartupArm(
        lead=sweep.lead,
        outcome=outcome,
        gate=None if gate_n is None else triggers.get(gate_n),
        startups=tuple(startups),
        hitstop=hitstop,
        gap_px=sweep.gap_px,
        observable=observable,
    )


@dataclass(frozen=True)
class StartupVerdict:
    """`unchanged` | `shortened` | `lengthened` | `not-comparable`.

    `unchanged` is claim (A) — the cancel bought permission only.
    `shortened` is claim (B) — the cancel skipped part of the special's own
    startup, which is the strictly stronger statement and the one this project
    has NOT been able to produce on MK2.
    """

    lead: str
    outcome: str
    verdict: str
    baseline_startup: Optional[int]
    arm_startup: Optional[int]
    delta: Optional[int]
    note: str = ""


def compare_startup(baseline: StartupArm, arm: StartupArm) -> StartupVerdict:
    """(A) vs (B), as a pure function of two arms' measured startups.

    `baseline` must be the NO-LEAD arm: the special's own unimpeded clock in
    the identical rig. Comparing two led arms to each other answers a
    different question and is refused by nothing here — the caller is trusted
    to pass the control, exactly as `measure_trial` is trusted to remove only
    the lead's button.

    **The gaps must match.** A contact-timed follow-up carries travel time, so
    an arm measured at a different gap from the baseline is `not-comparable`
    rather than a finding — this is the `force_ball`-after-HK case, where the
    lead knocked the target 83 px downrange and the raw numbers would have
    read as a 15-frame SLOWER special.
    """
    if arm.startup is None or baseline.startup is None:
        return StartupVerdict(
            lead=arm.lead, outcome=arm.outcome, verdict="not-comparable",
            baseline_startup=baseline.startup, arm_startup=arm.startup,
            delta=None,
            note="one side never produced the follow-up, so there is no clock "
                 "to compare (NULL is not zero -- docs/frames.md §2.5)",
        )
    if (
        baseline.gap_px is not None
        and arm.gap_px is not None
        and baseline.gap_px != arm.gap_px
    ):
        return StartupVerdict(
            lead=arm.lead, outcome=arm.outcome, verdict="not-comparable",
            baseline_startup=baseline.startup, arm_startup=arm.startup,
            delta=arm.startup - baseline.startup,
            note=f"gap differs ({baseline.gap_px} px vs {arm.gap_px} px); for "
                 "a contact-timed follow-up the difference is travel, not "
                 "startup",
        )
    delta = arm.startup - baseline.startup
    verdict = "unchanged" if delta == 0 else ("shortened" if delta < 0 else "lengthened")
    return StartupVerdict(
        lead=arm.lead, outcome=arm.outcome, verdict=verdict,
        baseline_startup=baseline.startup, arm_startup=arm.startup, delta=delta,
        note=("the special's own clock is identical with and without the lead: "
              "the cancel bought PERMISSION, not speed"
              if delta == 0 else
              f"the special's own clock moved {delta:+d} frames under the lead"),
    )


@dataclass(frozen=True)
class HitstopShift:
    """How much CONTACT alone moved a lead's gate, and whether that is hitstop."""

    lead: str
    whiff_gate: Optional[int]
    contact_gate: Optional[int]
    shift: Optional[int]
    hitstop: Optional[int]
    absorbed: Optional[bool]
    note: str = ""


def hitstop_shift(whiff: StartupArm, contact: StartupArm) -> HitstopShift:
    """`contact.gate - whiff.gate`, compared against the lead's measured
    hitstop.

    The whiff arm is the control that makes this a measurement: a whiffing
    lead has no contact and therefore no hitstop (§1.2), so everything else
    about the lead — its animation, its length, its input class — is identical
    in both terms and cancels.

    `absorbed=False` means the cancel did NOT bypass hitstop: the shift equals
    the freeze, so the "cancel window" opens exactly `hitstop` frames later on
    contact than on whiff. `absorbed=True` (shift 0 with a non-zero hitstop)
    would be the much stronger claim that the cancel steps over the freeze —
    not observed on MK2.

    The arms must be the same lead; comparing two different leads' gates is a
    different experiment and is refused.
    """
    if whiff.lead != contact.lead:
        raise StartupError(
            f"hitstop_shift compares one lead's contact gate against its own "
            f"whiff gate, not {whiff.lead!r} against {contact.lead!r}"
        )
    if whiff.outcome != "whiff" or contact.outcome not in ("hit", "block"):
        raise StartupError(
            "hitstop_shift needs a whiff arm and a hit/block arm, got "
            f"{whiff.outcome!r} and {contact.outcome!r}"
        )
    if whiff.gate is None or contact.gate is None:
        return HitstopShift(
            lead=whiff.lead, whiff_gate=whiff.gate, contact_gate=contact.gate,
            shift=None, hitstop=contact.hitstop, absorbed=None,
            note="a gate that never opened in the swept range cannot bound "
                 "anything (NULL, not 0)",
        )
    shift = contact.gate - whiff.gate
    hs = contact.hitstop
    absorbed: Optional[bool] = None
    if hs is not None:
        absorbed = shift == 0 and hs > 0
    return HitstopShift(
        lead=whiff.lead, whiff_gate=whiff.gate, contact_gate=contact.gate,
        shift=shift, hitstop=hs, absorbed=absorbed,
        note=(f"contact moved the gate {shift:+d} frames"
              + ("" if hs is None else
                 f"; the lead's measured hitstop is {hs} -- "
                 + ("they agree, so hitstop is NOT bypassed"
                    if shift == hs else
                    "they DISAGREE, so something other than hitstop is moving "
                    "the gate and the difference must be explained before "
                    "either number is published"))),
    )


# ── live measurement ─────────────────────────────────────────────────────


def onset_from_trace(
    values: Sequence[Optional[int]], *, tolerance: int = 2, start: int = 0
) -> Optional[int]:
    """First frame a positional trace leaves its resting value by more than
    `tolerance`.

    The tolerance is not noise-suppression, it is the §5 spacing convention:
    an idle MK2 fighter's `obj+0x12` breathes by a pixel or two, and a
    zero-tolerance edge would report that as movement.
    """
    if not values:
        return None
    base = values[start]
    if base is None:
        return None
    for i in range(start, len(values)):
        v = values[i]
        if v is not None and abs(v - base) > tolerance:
            return i
    return None


def lead_in_for(n: int, buttons: Sequence[str], *, hold: int) -> Tuple[ScriptStep, ...]:
    """The lead move as a `lead_in`: hold its button for `hold` frames, then
    idle until frame `n`, so the follow-up's script starts exactly at `n`.

    Refuses `n < hold` rather than truncating the lead's press. A silently
    shortened lead is a different move, and this module's whole verdict rests
    on the lead being the same move in every trial.
    """
    if n < hold:
        raise CancelError(
            f"N={n} is inside the lead move's own {hold}-frame press; "
            "shortening it would measure a different move"
        )
    steps = [ScriptStep(frames=hold, buttons=tuple(buttons))]
    if n > hold:
        steps.append(ScriptStep(frames=n - hold, buttons=()))
    return tuple(steps)


def measure_trial(
    session: LabSession,
    *,
    rig,
    arena: str,
    n: int,
    build_script: Callable[[Tuple[ScriptStep, ...]], MoveScript],
    lead_buttons: Sequence[str],
    lead_hold: int,
    contact_read: Callable[[LabSession], Hashable],
    position_read: Callable[[LabSession], Optional[int]],
    tail_frames: int,
    lead_contact_frame: Optional[int] = None,
    tolerance: int = 2,
) -> Trial:
    """One `N`, both conditions, from a freshly loaded arena each time.

    Two replays, never one: the control must differ from the cancel condition
    in EXACTLY the lead move's button and in nothing else — same schedule,
    same idle frames, same follow-up script — because any other difference
    reappears in `delay` as a fake cancel.
    """
    out: List[Tuple[Optional[int], Tuple[int, ...], Optional[int]]] = []
    for with_lead in (True, False):
        lead = lead_in_for(n, lead_buttons if with_lead else (), hold=lead_hold)
        script = build_script(lead)
        session.load_state(arena)
        trace = replay(
            session, rig=rig, script=script,
            total_frames=script.total_frames + tail_frames,
            defender_guard=False,
            sample_fn=lambda s: {"x": position_read(s), "c": contact_read(s)},
        )
        xs = [t["x"] for t in trace]
        cs = [t["c"] for t in trace]
        contacts = tuple(i for i in range(1, len(cs)) if cs[i] != cs[i - 1])
        damage = (int(cs[0]) - int(cs[contacts[-1]])) if contacts and isinstance(cs[0], int) else None
        out.append((onset_from_trace(xs, tolerance=tolerance), contacts, damage))

    (onset, contacts, damage), (control_onset, _, _) = out
    lead_landed = None
    if lead_contact_frame is not None:
        lead_landed = lead_contact_frame in contacts
    return Trial(
        n=n, onset=onset, control_onset=control_onset, lead_landed=lead_landed,
        contacts=contacts, damage=damage,
    )


def sweep_cancel(
    session: LabSession,
    *,
    rig,
    arena: str,
    gap_px: Optional[int],
    lead_name: str,
    follow_name: str,
    build_script: Callable[[Tuple[ScriptStep, ...]], MoveScript],
    lead_buttons: Sequence[str],
    lead_hold: int,
    contact_read: Callable[[LabSession], Hashable],
    position_read: Callable[[LabSession], Optional[int]],
    n_range: Sequence[int],
    tail_frames: int = 60,
    lead_contact_frame: Optional[int] = None,
    tolerance: int = 2,
) -> CancelSweep:
    """`measure_trial` over `n_range`, collected into one `CancelSweep`."""
    sweep = CancelSweep(lead=lead_name, follow=follow_name, arena=arena, gap_px=gap_px)
    for n in n_range:
        sweep.trials.append(measure_trial(
            session, rig=rig, arena=arena, n=n, build_script=build_script,
            lead_buttons=lead_buttons, lead_hold=lead_hold,
            contact_read=contact_read, position_read=position_read,
            tail_frames=tail_frames, lead_contact_frame=lead_contact_frame,
            tolerance=tolerance,
        ))
    return sweep
