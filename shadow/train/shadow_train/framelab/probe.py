"""docs/frames.md §4 — the act-again probe, and the advantage measurement
built on it.

    probe_run   : load state -> step to anchor+N -> HOLD walk -> step W frames
    control_run : load state -> step to anchor+N -> hold NOTHING -> step W frames
    actionable(N) := observable(probe_run) != observable(control_run)

**The differential form is the whole protocol.** An ABSOLUTE test ("did x
change?") reports TRUE during pushback, during hitstun animation, and during
any scripted motion — none of which mean the fighter has control. Differencing
against an identical no-input replay cancels pushback, hitstop and animation
churn in one stroke. There is exactly one comparison in this module
(`_actionable_from_traces`) and it compares probe against control; if a future
edit introduces `trace[i] != trace[i-1]` as an actionability test, that is the
defect two reviewers already caught, reintroduced.

## The window W is NOT a free parameter (this is not in §4.2, and it should be)

§4.2 writes "step W frames" without saying what W is. W is load-bearing, and
picking it wrong makes two observables disagree by a constant while each
looks internally consistent — exactly the failure §8.4 exists to catch.

Let `A` be the absolute frame the fighter regains control, `l` the true
injection latency (frames from `hold_buttons` to the core reading the button),
`m` the OBSERVABLE's manifestation margin (frames from the fighter acting to
that observable moving), and `W` the comparison window. A hold asserted at
frame `H = anchor + N` cannot reach the core before `H + l`, so the first
divergence lands at

    D(N) = max(H + l, A) + m

and `actionable(N)` is TRUE exactly when `D(N) <= H + W`:

  * `H + l >= A` (hold-limited): TRUE iff `l + m <= W`.
  * `H + l <  A` (stun-limited): TRUE iff `N >= A_rel + m - W`.

§3.1's zero-point calibration measures the first divergence frame under a
hold in neutral, which is `l + m` — i.e. `input_latency_frames` is already
PER-OBSERVABLE and already contains that observable's margin. Call it
`L_obs`. Setting

    W_obs = L_obs + c          (c a margin shared by all observables)

makes the sweep's first TRUE

    N* = A_rel + m - W_obs = A_rel - l - c

which does not contain `m` at all. Every observable then returns the SAME
`N*` — cross-observable agreement is a real check on the protocol instead of
an artifact of a shared W. Measured live on MK2 arcade: `L_struct = 1`,
`L_pointer_x = 2` (5/5 trials each, both ports), so a single shared W would
have made those two observables differ by exactly 1 frame forever.

Converting `N*` to an absolute frame count re-introduces the margin:

    first_true + W_obs = A_rel + m

which is what `SweepResult.actionable_after_contact` returns, labelled with
its observable. For the FASTEST observable on a port (the one with the
smallest `L_obs`, `struct_divergence` here) `m` is 0 by construction, so that
row is the accurate one; a slower observable's absolute is high by its own
`m`. The comparable quantity across observables is `first_true`.

The ADVANTAGE is immune to all of it:

    advantage = N*(defender) - N*(attacker)

`l`, `c` and `m` are identical on both sides and cancel exactly. The absolute
per-side frame carries a margin; the advantage does not.

## What this module does not do

`first_active_frame` is NOT a by-product of this probe (§4.4) — it is the
first input-relative frame at which the contact signal fires at gap 0, a
separate measurement. Rows written from here leave it NULL.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Callable, Dict, Hashable, Mapping, Optional, Sequence, Tuple

from .session import LabSession, LabError, PreconditionError

__all__ = [
    "MAX_SEARCH",
    "METHOD_LINEAR",
    "METHOD_BINARY",
    "ProbeError",
    "NoContactError",
    "ScriptStep",
    "MoveScript",
    "Rig",
    "Anchor",
    "SweepResult",
    "AdvantageMeasurement",
    "MonotonicityEvidence",
    "ProbeCalibrationError",
    "calibrate_probe_latency",
    "find_anchor",
    "replay",
    "sweep_actionable",
    "measure_advantage",
    "advantage_rows",
]

# §4.3: "MAX_SEARCH is ~60 frames; the cost is bounded and the result is
# unconditionally correct."
MAX_SEARCH = 60

METHOD_LINEAR = "linear_sweep"
METHOD_BINARY = "binary_search"

# §4.1's multi-hit rule: contacts closer together than this belong to the same
# move. MK2's own calibration constant (HITSTUN_RECENT_FRAMES) is 20; callers
# pass the profile's value rather than trusting this default.
DEFAULT_QUIET_FRAMES = 20


class ProbeError(LabError):
    """The probe could not produce a trustworthy number."""


class NoContactError(ProbeError):
    """The contact signal never fired. §4.1: "a NO-change trial is a WHIFF" —
    a whiff has no advantage number (§1.1), so this is a RESULT, not a bug to
    be worked around by loosening the anchor."""


# ── the rig and the script ────────────────────────────────────────────────


@dataclass(frozen=True)
class ScriptStep:
    """`buttons` is the port's ENTIRE held set for `frames` frames
    (`hold_buttons` replaces rather than ORs), so a step fully describes the
    input — no implicit carry-over from the previous step."""

    frames: int
    buttons: Tuple[str, ...] = ()


@dataclass(frozen=True)
class MoveScript:
    """A deterministic, replayable input program for the ATTACKER port.

    `lead_in` is setup that is not the move — walking into range, mostly. It
    is replayed identically in every run (probe, control, anchor discovery),
    so it cancels out of the differential exactly like pushback does. Keeping
    it separate from `steps` is what lets `attack_input_frame` mean anything:
    it is the frame the move's own input is first asserted, which is what
    §4.4's FAF measurement will need later.
    """

    name: str
    steps: Tuple[ScriptStep, ...]
    lead_in: Tuple[ScriptStep, ...] = ()

    @property
    def attack_input_frame(self) -> int:
        return sum(s.frames for s in self.lead_in)

    @property
    def total_frames(self) -> int:
        return self.attack_input_frame + sum(s.frames for s in self.steps)


@dataclass(frozen=True)
class Rig:
    """§2.6: "Hit-vs-block is a property of the RIG, not an inference." The
    lab drives both ports, so `guard_buttons` held on `defender_port` IS the
    ground truth for on_block; nothing is inferred from a health delta."""

    arena: str
    attacker_port: int
    defender_port: int
    guard_buttons: Tuple[str, ...]
    walk_directions: Tuple[str, ...] = ("left", "right")
    # §4.2's corner hazard generalised: a fighter cannot walk into a wall, and
    # it cannot walk into the OPPONENT'S BODY either. At point-blank the
    # attacker's forward direction and the defender's forward direction are
    # both blocked, so each port gets its own preference order — away from the
    # opponent first — and the sweep still falls through to the other one.
    walk_directions_by_port: Mapping[int, Tuple[str, ...]] = field(
        default_factory=dict
    )
    quiet_frames: int = DEFAULT_QUIET_FRAMES


# ── anchor (§4.1) ─────────────────────────────────────────────────────────


@dataclass(frozen=True)
class Anchor:
    """`contact_frame` is run-relative (frame 0 = the loaded arena)."""

    contact_frame: int
    hits: int
    contact_frames: Tuple[int, ...]
    quiet_frames: int
    trace: Tuple[Any, ...] = field(repr=False, default=())


def _cluster_first_contact(
    contacts: Sequence[int], quiet_frames: int
) -> Tuple[int, int, Tuple[int, ...]]:
    """§4.1's multi-hit rule: "consecutive contacts inside the counter's
    ~20-frame window do not reset it, so anchoring on the FIRST fire while
    the defender's stun is set by the LAST makes advantage too negative by
    the inter-hit gap. Anchor on the LAST contact before the quiet window,
    and store `hits`."" """
    cluster = [contacts[0]]
    for f in contacts[1:]:
        if f - cluster[-1] < quiet_frames:
            cluster.append(f)
        else:
            break
    return cluster[-1], len(cluster), tuple(cluster)


def find_anchor(
    session: LabSession,
    *,
    rig: Rig,
    script: MoveScript,
    contact_read: Callable[[LabSession], Hashable],
    total_frames: int,
    defender_guard: bool,
) -> Anchor:
    """Replay the move once, watching the port's contact signal every frame,
    and return the LAST contact of the first cluster.

    `contact_read` is injected — on MK2 arcade it is the per-victim HUD damage
    pair `0xBCA0`/`0xBC88` (`hitstun_sources` in the profile). It is NOT
    `hit_counter 0xD3FE`, which live 2-human testing disproved (P1-victim
    only) and which is not in the shipped profile.
    """
    trace = replay(
        session,
        rig=rig,
        script=script,
        total_frames=total_frames,
        defender_guard=defender_guard,
        sample_fn=lambda s: {"contact": contact_read(s)},
    )
    values = [t["contact"] for t in trace]
    contacts = [i for i in range(1, len(values)) if values[i] != values[i - 1]]
    if not contacts:
        raise NoContactError(
            f"the contact signal never changed across {total_frames} frames of "
            f"'{script.name}' (rig_guard_state="
            f"{'held' if defender_guard else 'none'}). docs/frames.md §4.1: a "
            "NO-change trial is a WHIFF, and a whiff has no advantage number "
            "(§1.1). Fix the spacing or the move, do not loosen the anchor."
        )
    anchor, hits, cluster = _cluster_first_contact(contacts, rig.quiet_frames)
    if anchor + rig.quiet_frames >= len(values):
        raise ProbeError(
            f"contact at frame {anchor} is within {rig.quiet_frames} frames of "
            f"the end of a {total_frames}-frame trace, so the quiet window that "
            "defines 'last contact' was never observed -- the cluster may be "
            "truncated and `hits` wrong. Lengthen total_frames."
        )
    return Anchor(
        contact_frame=anchor,
        hits=hits,
        contact_frames=cluster,
        quiet_frames=rig.quiet_frames,
        trace=tuple(values),
    )


# ── the replay engine ─────────────────────────────────────────────────────

Sampler = Callable[[LabSession], Mapping[str, Hashable]]


def _schedule(
    rig: Rig,
    script: MoveScript,
    *,
    defender_guard: bool,
    probe_port: Optional[int],
    probe_buttons: Tuple[str, ...],
    probe_at: Optional[int],
    guard_release_at: Optional[int],
) -> Dict[int, Dict[int, Tuple[str, ...]]]:
    """frame -> port -> the port's entire held set from that frame on.

    Order of precedence at a shared frame: script < guard release < probe.
    That ordering is what makes the defender's probe legal: releasing guard
    and holding a walk direction on the same frame must end up holding the
    WALK, not nothing.
    """
    sched: Dict[int, Dict[int, Tuple[str, ...]]] = {}

    def put(frame: int, port: int, buttons: Tuple[str, ...]) -> None:
        sched.setdefault(frame, {})[port] = buttons

    cursor = 0
    for step in tuple(script.lead_in) + tuple(script.steps):
        put(cursor, rig.attacker_port, tuple(step.buttons))
        cursor += step.frames
    put(cursor, rig.attacker_port, ())

    put(0, rig.defender_port, tuple(rig.guard_buttons) if defender_guard else ())
    if guard_release_at is not None:
        put(guard_release_at, rig.defender_port, ())

    if probe_at is not None and probe_port is not None:
        put(probe_at, probe_port, tuple(probe_buttons))
    return sched


def _baseline_held_at(
    rig: Rig, script: MoveScript, *, defender_guard: bool, port: int, frame: int
) -> Tuple[str, ...]:
    """What `port` would be holding at `frame` with no probe entry at all —
    used to decide whether one shared control trace is valid for every N."""
    sched = _schedule(
        rig,
        script,
        defender_guard=defender_guard,
        probe_port=None,
        probe_buttons=(),
        probe_at=None,
        guard_release_at=None,
    )
    held: Tuple[str, ...] = ()
    for f in sorted(sched):
        if f > frame:
            break
        if port in sched[f]:
            held = sched[f][port]
    return held


def replay(
    session: LabSession,
    *,
    rig: Rig,
    script: MoveScript,
    total_frames: int,
    defender_guard: bool,
    probe_port: Optional[int] = None,
    probe_buttons: Sequence[str] = (),
    probe_at: Optional[int] = None,
    guard_release_at: Optional[int] = None,
    sample_fn: Optional[Sampler] = None,
    sample_from: int = 0,
) -> list:
    """One replay from the arena state. Returns `trace`, indexed by frame,
    where frame 0 is the loaded state before any step. Entries before
    `sample_from` are `None` (the sweep only needs the window, and skipping
    the reads elsewhere is most of the runtime).

    Every frame is confirmed (§3.5) and the load is confirmed + verified
    (§3.4/§3.6) — both by `LabSession`, which is the only way this module
    touches the emulator.

    ## Frames nothing observes are BATCHED, and that is not a shortcut

    A replay only has to stop at frames that something looks at or acts on:
    a frame the schedule changes an input at, and a frame the sampler reads.
    Everything between two such frames is a fixed held mask running forward,
    which `run_frames` does in ONE call for the whole segment (still
    confirming that every requested frame landed — `LabSession.run_frames`
    refuses a partial batch).

    That is most of the lab's frame budget: a sweep at N=45 replays ~55
    frames to reach the hold and then samples ~4, so 55 of 59 frames were
    round trips that produced nothing.

    A pending held-set change is applied by the call that runs the next
    frames, through `LabSession.set_held` — which CONFIRMS the change reached
    the core's input fold before any frame runs (`session.confirm_fold`). It
    is deliberately NOT passed as one of `run_frames`' per-port masks: those
    are applied under the same lock acquisition that arms the batch, leaving
    no window for the host loop's fold, and the batch's first frame can then
    run on the previous input. Measured at 13 wrong answers in 200 identical
    evaluations — this is §3.6's flake, with a different cause than §3.6
    assumed and a fix that is an oracle rather than a wait.
    """
    sched = _schedule(
        rig,
        script,
        defender_guard=defender_guard,
        probe_port=probe_port,
        probe_buttons=tuple(probe_buttons),
        probe_at=probe_at,
        guard_release_at=guard_release_at,
    )
    session.load_state(rig.arena)

    trace: list = [None] * (total_frames + 1)
    sampling = sample_fn is not None

    def sample(f: int) -> None:
        if sampling and f >= sample_from:
            trace[f] = dict(sample_fn(session))

    def next_stop(after: int) -> int:
        """The first frame > `after` the replay must stop at: one the
        schedule touches, or one the sampler reads. Frames strictly between
        `after` and this can be run in a single batch."""
        stop = total_frames
        for f in sched:
            if after < f < stop:
                stop = f
        if sampling and after < sample_from < stop:
            stop = sample_from
        if sampling and sample_from <= after:
            stop = min(stop, after + 1)
        return stop

    # Pending held-set changes, applied by the next call that RUNS frames
    # (folded into `run_frames`' masks, or issued via `set_held` before a
    # single `step`). Deferring is safe because a hold cannot affect a read:
    # it only changes what the NEXT frame folds in.
    pending: Dict[int, Tuple[str, ...]] = dict(sched.get(0, {}))
    sample(0)
    f = 0
    while f < total_frames:
        stop = next_stop(f)
        session.run_frames(stop - f, holds=pending or None)
        pending = {}
        f = stop
        pending.update(sched.get(f, {}))
        sample(f)
    session.release_all_ports()
    return trace


# ── the differential comparison — the ONE comparison in this module ───────


def _actionable_from_traces(
    probe_window: Sequence[Optional[Mapping[str, Hashable]]],
    control_window: Sequence[Optional[Mapping[str, Hashable]]],
    observable: str,
) -> bool:
    """§4.2, and nothing else: TRUE iff the observable differs between the
    probe run and the identical no-input control run over the same window.

    A control that also changes (pushback, hitstun animation, a scripted
    move) does NOT make this TRUE — that is exactly what differencing buys.
    """
    for p, c in zip(probe_window, control_window):
        if p is None or c is None:
            raise ProbeError(
                "differential comparison hit an unsampled frame -- the probe "
                "and control windows must both be sampled at every frame they "
                "are compared over."
            )
        if p[observable] != c[observable]:
            return True
    return False


# ── probe-shape calibration (§3.1, corrected by a live failure) ───────────


class ProbeCalibrationError(ProbeError):
    """The probe's own latency was not constant across trials, so §3.1's rule
    applies: "STOP -- the probe is not sound on this port and nothing
    downstream can be trusted." Never averaged."""


def calibrate_probe_latency(
    session: LabSession,
    *,
    rig: Rig,
    script: MoveScript,
    port: int,
    anchor: int,
    at_n: int,
    observables: Sequence[str],
    sample_fn: Sampler,
    defender_guard: bool,
    trials: int = 5,
    max_window: int = 16,
    direction: Optional[str] = None,
) -> Dict[str, int]:
    """§3.1's zero-point calibration, run on the PROBE'S OWN INPUT SHAPE
    instead of on a bare neutral walk. Returns per-observable
    `input_latency_frames`.

    This exists because the neutral calibration was live-measured to be WRONG
    for one of the two probes. `calibrate.zero_point_calibration` asserts a
    walk from a fighter that is standing free and holding nothing; the
    defender's on-block probe instead RELEASES A HELD GUARD BUTTON and starts
    walking on the same frame, and MK2's block stance does not drop on the
    frame the button does. Measured on `r-v-r.state`: the neutral numbers are
    `struct_divergence` 1 / `pointer_x` 2, but with `window` set from those,
    the guarded defender's `pointer_x` sweep reported NEVER ACTIONABLE across
    all 46 candidate N and its `struct_divergence` predicate came back
    non-monotone noise. The window was simply shorter than that probe's real
    latency, so the divergence always happened just past the end of it.

    The fix is to calibrate the transition you are actually going to perform:
    run the identical probe/control pair at `at_n`, chosen far enough past the
    anchor that the fighter is certainly free (so the measurement is
    hold-limited, which is what a latency IS), with a generous window, and
    take the first divergence frame. Repeat `trials` times and require it to
    be CONSTANT — §3.1's central rule, restated: if it varies, STOP.

    Note this is the same quantity §3.1 defines (`l + m` for that observable),
    measured under the conditions of use. It replaces the neutral number for
    this probe; it does not "correct" it, and the two are stored separately.
    """
    if trials < 1:
        raise ValueError("`trials` must be >= 1")
    hold_at = anchor + at_n
    end = hold_at + max_window
    guard_release_at = (
        hold_at if (defender_guard and port == rig.defender_port) else None
    )
    cand = (direction,) if direction else _direction_candidates(rig, None, port)

    samples: Dict[str, list] = {obs: [] for obs in observables}
    chosen: Dict[str, Optional[str]] = {obs: None for obs in observables}
    for d in cand:
        for _ in range(trials):
            probe_trace = replay(
                session, rig=rig, script=script, total_frames=end,
                defender_guard=defender_guard, probe_port=port,
                probe_buttons=(d,), probe_at=hold_at,
                guard_release_at=guard_release_at,
                sample_fn=sample_fn, sample_from=hold_at + 1,
            )
            control_trace = replay(
                session, rig=rig, script=script, total_frames=end,
                defender_guard=defender_guard, probe_port=port,
                probe_buttons=(), probe_at=hold_at,
                guard_release_at=guard_release_at,
                sample_fn=sample_fn, sample_from=hold_at + 1,
            )
            for obs in observables:
                if chosen[obs] not in (None, d):
                    continue
                first = None
                for i in range(1, max_window + 1):
                    p, c = probe_trace[hold_at + i], control_trace[hold_at + i]
                    if p[obs] != c[obs]:
                        first = i
                        break
                if first is not None:
                    chosen[obs] = d
                    samples[obs].append(first)
        if all(chosen[obs] is not None for obs in observables):
            break

    out: Dict[str, int] = {}
    for obs in observables:
        vals = samples[obs]
        if not vals:
            raise ProbeCalibrationError(
                f"observable {obs!r} never diverged within {max_window} frames "
                f"of a hold at anchor+{at_n} on port {port} (directions "
                f"{list(cand)}). Either the fighter is still not free that far "
                "past contact, or this observable does not respond to a walk "
                "on this port -- in both cases the probe is not calibrated and "
                "nothing measured with it is a number (docs/frames.md §3.1)."
            )
        if len(set(vals)) != 1:
            raise ProbeCalibrationError(
                f"probe latency for {obs!r} on port {port} was not constant "
                f"across {len(vals)} trials (samples={vals}). docs/frames.md "
                "§3.1: 'it MUST be constant. If it is not, STOP.' Not averaging."
            )
        out[obs] = vals[0]
    return out


# ── monotonicity evidence (§4.3) ──────────────────────────────────────────


@dataclass(frozen=True)
class MonotonicityEvidence:
    """§4.3: binary search is "permitted only where `actionable(N)` has been
    demonstrated monotone for that move class".

    "Demonstrated" here means an actual exhaustive predicate vector from a
    linear sweep, not an assertion — the first draft's N-1/N/N+1
    confirmation "does not detect a predicate of shape T...T F...F T...T,
    which is exactly what an absolute (non-differential) observable
    produces."
    """

    move_class: str
    observable: str
    samples: Tuple[Tuple[int, bool], ...]
    source: str = ""

    def demonstrates(self, move_class: str, observable: str, max_search: int) -> bool:
        if self.move_class != move_class or self.observable != observable:
            return False
        by_n = dict(self.samples)
        if any(n not in by_n for n in range(max_search + 1)):
            return False
        seen_true = False
        for n in range(max_search + 1):
            if by_n[n]:
                seen_true = True
            elif seen_true:
                return False  # T ... F : not monotone
        return seen_true


# ── the sweep (§4.3) ──────────────────────────────────────────────────────


@dataclass(frozen=True)
class SweepResult:
    observable: str
    method: str
    direction: Optional[str]
    first_true: Optional[int]
    predicate: Tuple[Optional[bool], ...]
    monotone: Optional[bool]
    window: int
    input_latency_frames: int
    max_search: int
    port: int
    rig_guard_state: str
    runs: int

    @property
    def actionable_after_contact(self) -> Optional[int]:
        """Frames from contact to actionable, as `first_true + window` — which
        the module docstring derives as `A_rel + m`, exact for the port's
        FASTEST observable and high by `m` for a slower one. NULL when the
        sweep found nothing — §2.5: absent means absent, never 0."""
        if self.first_true is None:
            return None
        return self.first_true + self.window

    def as_provenance(self) -> dict:
        return {
            "observable": self.observable,
            "method": self.method,
            "input_latency_frames": self.input_latency_frames,
        }


def _direction_candidates(
    rig: Rig, explicit: Optional[str], port: Optional[int] = None
) -> Tuple[str, ...]:
    if explicit is not None:
        return (explicit,)
    if port is not None and port in rig.walk_directions_by_port:
        return tuple(rig.walk_directions_by_port[port])
    return tuple(rig.walk_directions)


def sweep_actionable(
    session: LabSession,
    *,
    rig: Rig,
    script: MoveScript,
    port: int,
    anchor: int,
    observables: Sequence[str],
    sample_fn: Sampler,
    input_latency_frames: "int | Mapping[str, int]",
    defender_guard: bool,
    window: "Optional[int | Mapping[str, int]]" = None,
    window_margin: int = 0,
    max_search: int = MAX_SEARCH,
    method: str = METHOD_LINEAR,
    monotonicity: Optional[MonotonicityEvidence] = None,
    direction: Optional[str] = None,
    move_class: Optional[str] = None,
    exhaustive: bool = True,
    repeats: int = 1,
) -> Dict[str, SweepResult]:
    """Sweep N and return one `SweepResult` PER OBSERVABLE, all extracted
    from the same runs (§8.4's cross-method agreement, for the price of one
    sweep — the runs are shared, the extraction is not).

    `exhaustive=True` (the default) evaluates every N in `0..max_search`
    instead of stopping at the first TRUE. That costs the full sweep but buys
    two things §4.3 asks for and an early exit cannot give: the predicate's
    actual SHAPE (so `monotone` is measured, not assumed) and a
    `MonotonicityEvidence` that a later binary search can be gated on.

    `repeats > 1` re-runs every evaluation and requires the answers to agree
    (see `evaluate`); a disagreement raises rather than voting.

    `input_latency_frames` and `window` are PER OBSERVABLE (pass a mapping;
    an int applies to all). `window` defaults to that observable's own
    `input_latency_frames + window_margin`, which is what makes different
    observables return the same `first_true` — see the module docstring.
    """
    if method not in (METHOD_LINEAR, METHOD_BINARY):
        raise ValueError(f"unknown method {method!r}")
    if not observables:
        raise ValueError("`observables` must name at least one observable")

    latency = {
        obs: (
            input_latency_frames[obs]
            if isinstance(input_latency_frames, Mapping)
            else int(input_latency_frames)
        )
        for obs in observables
    }
    windows = {
        obs: (
            latency[obs] + window_margin
            if window is None
            else (window[obs] if isinstance(window, Mapping) else int(window))
        )
        for obs in observables
    }
    for obs in observables:
        if windows[obs] < 1:
            raise ValueError(f"`window` for {obs!r} must be >= 1")
        if windows[obs] < latency[obs]:
            raise ProbeError(
                f"window={windows[obs]} for observable {obs!r} is smaller than "
                f"its calibrated input_latency_frames={latency[obs]}: the "
                "injected hold cannot diverge inside a window shorter than the "
                "latency, so the sweep would report 'never actionable' for "
                "every move. docs/frames.md §3.1/§4.2."
            )
    max_window = max(windows.values())

    guard_release_at: Optional[int] = None
    rig_guard_state = "held" if defender_guard else "none"

    if method == METHOD_BINARY:
        if len(observables) != 1:
            raise ValueError(
                "binary search takes exactly one observable: two observables "
                "have two different boundaries, and bisecting on 'either one "
                "flipped' converges on the earlier of them and then reports it "
                "for both. Sweep them together linearly, or bisect them "
                "separately."
            )
        cls = move_class or script.name
        for obs in observables:
            if monotonicity is None or not monotonicity.demonstrates(
                cls, obs, max_search
            ):
                raise PreconditionError(
                    "docs/frames.md §4.3: binary search is OPT-IN and "
                    "permitted only where actionable(N) has been demonstrated "
                    f"monotone for this move class ({cls!r}) and observable "
                    f"({obs!r}). No such demonstration was supplied "
                    f"({'none' if monotonicity is None else 'evidence does not cover it'}). "
                    "Linear sweep from N=0 is the DEFAULT and is "
                    "unconditionally correct."
                )

    # Is one shared control trace legal? Only when the control's inputs on the
    # probed port do not themselves depend on N. They do depend on N whenever
    # the defender must drop guard at the probe instant (a fighter holding MK2's
    # Block button stands still, so a walk probe through held guard can never
    # diverge — the guard MUST be released, in BOTH runs, at the same frame).
    probe_is_guarding_defender = defender_guard and port == rig.defender_port
    if probe_is_guarding_defender:
        guard_release_at = None  # set per-N below

    shared_control: Optional[list] = None
    control_n_independent = not probe_is_guarding_defender and all(
        _baseline_held_at(
            rig, script, defender_guard=defender_guard, port=port, frame=anchor + n
        )
        == ()
        for n in range(max_search + 1)
    )

    runs = 0
    if control_n_independent:
        total = anchor + max_search + max_window
        shared_control = replay(
            session,
            rig=rig,
            script=script,
            total_frames=total,
            defender_guard=defender_guard,
            sample_fn=sample_fn,
            sample_from=anchor,
        )
        runs += 1

    def _evaluate_once(n: int, cand: str) -> Dict[str, bool]:
        """One `actionable(N)` evaluation: a probe run and the control it is
        differenced against, sliced per observable."""
        nonlocal runs
        hold_at = anchor + n
        end = hold_at + max_window
        release_at = hold_at if probe_is_guarding_defender else guard_release_at

        probe_trace = replay(
            session,
            rig=rig,
            script=script,
            total_frames=end,
            defender_guard=defender_guard,
            probe_port=port,
            probe_buttons=(cand,),
            probe_at=hold_at,
            guard_release_at=release_at,
            sample_fn=sample_fn,
            sample_from=hold_at + 1,
        )
        runs += 1

        if shared_control is not None:
            control_slice = shared_control[hold_at + 1 : end + 1]
        else:
            # The control must be input-identical to the probe EXCEPT for
            # the walk: same guard release, at the same frame, holding
            # nothing instead of a direction.
            control_trace = replay(
                session,
                rig=rig,
                script=script,
                total_frames=end,
                defender_guard=defender_guard,
                probe_port=port,
                probe_buttons=(),
                probe_at=hold_at,
                guard_release_at=release_at,
                sample_fn=sample_fn,
                sample_from=hold_at + 1,
            )
            runs += 1
            control_slice = control_trace[hold_at + 1 : end + 1]

        probe_slice = probe_trace[hold_at + 1 : end + 1]
        # Each observable is compared over ITS OWN window, sliced out of the
        # one (max_window-long) pair of runs.
        return {
            obs: _actionable_from_traces(
                probe_slice[: windows[obs]], control_slice[: windows[obs]], obs
            )
            for obs in observables
        }

    def evaluate(n: int, cand: str) -> Dict[str, bool]:
        """`_evaluate_once` repeated `repeats` times, requiring agreement.

        This exists because of a measured transport hazard, not theory: on
        ~1.5% of live runs a `hold_buttons` landed ONE FRAME EARLY relative to
        the `step` that had already been confirmed, which shows up as a single
        spurious TRUE at an N where the fighter is demonstrably still stunned
        (seen twice in ~140 runs, and NOT reproducible on re-run: 4/4 clean
        both times). One such flake below the real boundary moves `first_true`
        by several frames, and it does it silently.

        `repeats > 1` turns that into a loud failure. It is deliberately NOT a
        majority vote: §7 says a number that fails re-measurement is DELETED,
        not averaged, and a 2-of-3 vote is exactly the averaging that rule
        forbids."""
        first = _evaluate_once(n, cand)
        for _ in range(repeats - 1):
            again = _evaluate_once(n, cand)
            disagreed = [o for o in observables if again[o] != first[o]]
            if disagreed:
                raise ProbeError(
                    f"actionable(N={n}) did not reproduce for {disagreed} "
                    f"(direction {cand!r}, port {port}): the same probe/control "
                    "pair, replayed from the same save state, gave different "
                    "answers. docs/frames.md §7: a number that fails "
                    "re-measurement is DELETED, not averaged. Re-run; if it "
                    "persists, the protocol is wrong."
                )
        return first

    candidates = _direction_candidates(rig, direction, port)
    per_direction: Dict[str, Dict[str, list]] = {}

    for cand in candidates:
        predicate: Dict[str, list] = {obs: [] for obs in observables}
        if method == METHOD_BINARY:
            # Bisection over a predicate whose monotonicity was DEMONSTRATED
            # above (never assumed). Unevaluated N stay None so the stored
            # `predicate` can never be mistaken for a linear sweep's.
            obs = observables[0]
            vals: list = [None] * (max_search + 1)
            lo, hi = 0, max_search
            vals[hi] = evaluate(hi, cand)[obs]
            if vals[hi]:
                while lo < hi:
                    mid = (lo + hi) // 2
                    vals[mid] = evaluate(mid, cand)[obs]
                    if vals[mid]:
                        hi = mid
                    else:
                        lo = mid + 1
                if vals[lo] is None:
                    vals[lo] = evaluate(lo, cand)[obs]
            predicate = {obs: vals}
        else:
            for n in range(max_search + 1):
                got = evaluate(n, cand)
                for obs in observables:
                    predicate[obs].append(got[obs])
                if not exhaustive and all(got[obs] for obs in observables):
                    break

        per_direction[cand] = predicate
        # §4.2's corner hazard, per observable: move on to the next direction
        # only while some observable still has no divergence at all. A noisy
        # observable must not lock in a direction a clean one cannot walk.
        if all(any(v for v in vals if v) for vals in predicate.values()):
            break

    # Per observable, the first candidate direction in which IT diverged.
    # If none did, §4.2 says record NULL, not "never actionable".
    chosen_per_obs: Dict[str, Optional[str]] = {}
    for obs in observables:
        chosen_per_obs[obs] = next(
            (
                cand
                for cand in candidates
                if cand in per_direction
                and any(v for v in per_direction[cand][obs] if v)
            ),
            None,
        )

    results: Dict[str, SweepResult] = {}
    for obs in observables:
        chosen = chosen_per_obs[obs]
        vals = per_direction[chosen][obs] if chosen else []
        first_true = next((i for i, v in enumerate(vals) if v), None)
        monotone = None
        if method == METHOD_LINEAR and vals:
            monotone = all(
                vals[i] <= vals[i + 1] for i in range(len(vals) - 1)
            )
        results[obs] = SweepResult(
            observable=obs,
            method=method,
            direction=chosen,
            first_true=first_true,
            predicate=tuple(vals),
            monotone=monotone,
            window=windows[obs],
            input_latency_frames=latency[obs],
            max_search=max_search,
            port=port,
            rig_guard_state=rig_guard_state,
            runs=runs,
        )
    return results


# ── advantage (§4.3) ──────────────────────────────────────────────────────


@dataclass(frozen=True)
class AdvantageMeasurement:
    move: str
    observable: str
    rig_guard_state: str
    anchor: Anchor
    attacker: SweepResult
    defender: SweepResult

    @property
    def advantage(self) -> Optional[int]:
        """`actionable(defender, contact) - actionable(attacker, contact)`.

        Computed from the RAW sweep results because the injection latency,
        the window and the observable's manifestation margin are identical on
        both sides and cancel exactly — this number needs no calibration
        subtracted, and adding one would only inject noise. NULL if either
        side never became actionable (§2.5)."""
        if self.attacker.first_true is None or self.defender.first_true is None:
            return None
        return self.defender.first_true - self.attacker.first_true


def measure_advantage(
    session: LabSession,
    *,
    rig: Rig,
    script: MoveScript,
    observables: Sequence[str],
    sample_fn: Sampler,
    contact_read: Callable[[LabSession], Hashable],
    input_latency_frames: "int | Mapping[str, int]",
    defender_guard: bool,
    anchor_total_frames: int,
    window: "Optional[int | Mapping[str, int]]" = None,
    window_margin: int = 0,
    max_search: int = MAX_SEARCH,
    method: str = METHOD_LINEAR,
    monotonicity: Optional[MonotonicityEvidence] = None,
    anchor: Optional[Anchor] = None,
) -> Dict[str, AdvantageMeasurement]:
    """One `on_hit` OR one `on_block` measurement — the two are the identical
    protocol differing ONLY in `defender_guard` (§4.3: "they are separate
    columns and MUST NOT be derived from each other").

    Returns one measurement per observable, from the same runs.
    """
    if anchor is None:
        anchor = find_anchor(
            session,
            rig=rig,
            script=script,
            contact_read=contact_read,
            total_frames=anchor_total_frames,
            defender_guard=defender_guard,
        )

    attacker = sweep_actionable(
        session,
        rig=rig,
        script=script,
        port=rig.attacker_port,
        anchor=anchor.contact_frame,
        observables=observables,
        sample_fn=sample_fn,
        input_latency_frames=input_latency_frames,
        defender_guard=defender_guard,
        window=window,
        window_margin=window_margin,
        max_search=max_search,
        method=method,
        monotonicity=monotonicity,
    )
    defender = sweep_actionable(
        session,
        rig=rig,
        script=script,
        port=rig.defender_port,
        anchor=anchor.contact_frame,
        observables=observables,
        sample_fn=sample_fn,
        input_latency_frames=input_latency_frames,
        defender_guard=defender_guard,
        window=window,
        window_margin=window_margin,
        max_search=max_search,
        method=method,
        monotonicity=monotonicity,
    )
    return {
        obs: AdvantageMeasurement(
            move=script.name,
            observable=obs,
            rig_guard_state="held" if defender_guard else "none",
            anchor=anchor,
            attacker=attacker[obs],
            defender=defender[obs],
        )
        for obs in observables
    }


def advantage_rows(
    *,
    family: str,
    port: str,
    char: str,
    core_id: str,
    rom_id: str,
    on_block: Optional[AdvantageMeasurement] = None,
    on_hit: Optional[AdvantageMeasurement] = None,
    gap_walk_frames: Optional[int] = None,
    gap_px: Optional[float] = None,
    variant: Optional[str] = None,
    sample_n: Optional[int] = None,
    confidence: Optional[str] = None,
) -> list:
    """Shape `AdvantageMeasurement`s into `framelab.store.FrameStore` rows.

    Every column this probe does not measure is omitted, which the store
    turns into SQL NULL — §2.5's "absent means absent, never 0". In
    particular `first_active_frame` is NULL here BY CONSTRUCTION: §4.4 says
    it is not a by-product of the actionability probe, and claiming it was
    was a defect in the contract's first draft.
    """
    ref = on_block or on_hit
    if ref is None:
        raise ValueError("advantage_rows needs at least one of on_block/on_hit")
    if on_block is not None and on_hit is not None:
        if on_block.observable != on_hit.observable:
            raise ValueError(
                "on_hit and on_block rows must share an observable "
                f"({on_hit.observable!r} != {on_block.observable!r}) -- §6 "
                "stores observable per row because two observables are two "
                "different experiments."
            )

    row = {
        "family": family,
        "port": port,
        "char": char,
        "move": ref.move,
        "variant": variant,
        "gap_walk_frames": gap_walk_frames,
        "gap_px": gap_px,
        "hits": ref.anchor.hits,
        "on_block": on_block.advantage if on_block is not None else None,
        "on_hit": on_hit.advantage if on_hit is not None else None,
        # §2.6: the rig's own guard state, never an inference from health.
        "rig_guard_state": (
            "held+none" if (on_block and on_hit) else ref.rig_guard_state
        ),
        "observable": ref.observable,
        "method": ref.attacker.method,
        "input_latency_frames": ref.attacker.input_latency_frames,
        "sample_n": sample_n,
        "confidence": confidence,
        "core_id": core_id,
        "rom_id": rom_id,
    }
    return [row]
