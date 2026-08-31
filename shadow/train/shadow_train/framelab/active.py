"""The ACTIVE window — measured by moving the target, not by reading a hitbox.

docs/frames.md §6 has three columns that are NULL in every measured row:
`active`, `recovery` and `total`. The lab can already see when a fighter can
act again (§4) and when contact first occurs (§4.4), but MK2 arcade exposes no
hitbox data in RAM, so "how long is the hitbox out" had no observable at all.

## The method

`obj+0x12` (the pointer-resolved world X, `library/mk2/mk2.md` "Stable
per-fighter position") is a WRITE-AUTHORITATIVE store, not a recomputed
output. So the hitbox can be probed with the only thing that is guaranteed to
interact with it — a body:

    start the attack with the defender OUT of range
    write the defender's x to just-inside-range at the END of frame N
    observe whether contact occurs

Contact at N means the hitbox was live on frame N+1 at that distance. Sweeping
N gives a predicate whose TRUE run IS the active window:

    first_active_frame = the contact frame of the earliest connecting trial
    last_active_frame  = (last connecting N) + 1
    active             = last - first + 1

and then, from a separately measured whiff actionability (§4.2's differential
probe run against a move that never connects):

    total    = the attacker's walk-manifest frame, input-relative
    recovery = total - last_active_frame

## What was tested before this was believed (the four kill criteria)

Measured live, Mileena vs Reptile, `m-gap-*` ladder, two cold processes:

1. **Does the write stick during an attack?** YES, on this port, for BOTH
   fighters. A defender teleported at any frame of the animation holds the
   written x, bit-exact, for 20+ frames; so does the ATTACKER, mid-swing. The
   "positions are recomputed outputs" warning that motivated this doubt is an
   `asurabld` finding (`block+0x54/0x56`); MK2's object-pool x is authoritative.
   The one exception is structural and is enforced here: **below the
   anti-overlap floor** the game pushes the two apart at 6 px/frame, so a
   target gap under ~62 px does not exist after the frame it is written
   (`CollisionFloorError`).
2. **Is a teleported defender a valid target?** YES, and exactly as valid as a
   natively-positioned one. Teleporting the defender at frame 0 from 192 px to
   G reproduced the NATIVE ladder arenas cell for cell — 61 px → contact f8 /
   24 dmg (the close HP), 71 and 83 px → f11 / 11 dmg (the far HP), 99 px+ →
   whiff — matching `m-gap-45/39/35/30/25` measured with no writes at all.
3. **Is the window really the defender's animation phase?** NO — at usable
   distances. Delaying the attack input by k = 0..7 idle frames (which slides
   both fighters' idle phase against the attack) moved the window by exactly k
   and never changed its length, in two different arenas; holding Block on the
   defender (a completely different defender animation, chip damage instead of
   damage) gave a bit-identical window. **But it IS the phase within ~3 px of
   the reach boundary**: at G = 85 px the window's START pinned to run-frame 15
   regardless of k — an absolute clock, i.e. the defender's idle animation, not
   the attack — and the same G whiffed at every N from a different arena. That
   is the documented drifting-reference hazard, reproduced and localised. The
   defence is `require_gap_agreement`: measure at ≥2 target gaps and refuse
   unless they agree.
4. **Does the teleport itself alter the interaction?** NO. A NULL write (the
   defender's own current x, written back) at any frame left a connecting move
   connecting at the same frame for the same damage, and a whiffing move
   whiffing.

Plus one cross-check the kill list did not ask for: writing the ATTACKER's x
instead of the defender's — a different object-pool entry, the opposite sign,
a different failure mode — returns the SAME window on every move tried.

## What this method does NOT measure, and must not be read as

* **"The hitbox exists" — it measures "the hitbox covers THIS distance".** The
  two coincide over the inner range (the window was bit-identical at every
  target gap from the collision floor out to 1 px short of the reach boundary,
  on every move tried) but not at the fringe, where a still-extending limb
  arrives late: Mileena's far HP covers 61..84 px from frame 11 and only
  reaches 85..87 px at frame 15. A window measured at the fringe is a REACH
  measurement wearing the active window's clothes.
* **A proximity CLOSE variant's window.** Moving the defender out of a close
  normal's range also moves it out of the proximity bucket that SELECTS the
  close normal, so the move under measurement changes identity (measured:
  teleporting out at frames 0..7 turned the 24-damage close HP into the
  11-damage far HP). Variant re-resolution was measured to happen after frame
  1 and before frame 8, so `MIN_TELEPORT_FRAME = 2` keeps a far-variant sweep
  honest — but a close variant is simply not measurable this way, and
  `damage_stable` refuses the row rather than reporting the other move's
  window.
* **Projectiles and travelling specials.** For a move whose hitbox is a
  separate object (Mileena's sai, the roll, the teleport kick), "frames during
  which something overlaps distance G" is the PROJECTILE'S TRAVEL, not the
  move's active frames. Untested here; treat as out of scope until it is.
* **Vertical extent.** Every measurement is a horizontal sweep at the
  defender's resting y. A hitbox that is live but only above/below the
  defender reads as inactive.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Callable, Dict, List, Mapping, Optional, Sequence, Tuple

from .probe import MoveScript, ProbeError, Rig

__all__ = [
    "MIN_TELEPORT_FRAME",
    "ActiveError",
    "CollisionFloorError",
    "ClippedWindowError",
    "NonMonotoneWindowError",
    "VariantDriftError",
    "GapDisagreementError",
    "TeleportTrial",
    "ActiveWindow",
    "PositionIO",
    "resolve_object",
    "teleport_trial",
    "window_from_trials",
    "sweep_active_window",
    "measure_active",
    "derive_recovery",
]


# Writes at frame 0 or 1 land BEFORE MK2 has resolved which proximity variant
# of the button it is running, so they change the move instead of probing it
# (measured: an m-gap-30 → 61 px teleport at N=1 produced the 24-damage close
# HP, at N=2 the 11-damage far HP). The sweep therefore starts at 2, and
# `ClippedWindowError` refuses any window that would have begun earlier.
MIN_TELEPORT_FRAME = 2

# A window whose last TRUE sits this close to `max_search` was measured against
# the edge of the search, which docs/frames.md §7 calls a silent cap. Same
# constant and same reason as `kit._CAP_MARGIN`.
CAP_MARGIN = 5


class ActiveError(ProbeError):
    """The active window is not a number yet."""


class CollisionFloorError(ActiveError):
    """The requested target gap is inside the anti-overlap floor, where the
    game pushes the fighters apart at ~6 px/frame — the written gap exists for
    exactly one frame, so the trial does not test the distance it claims to."""


class ClippedWindowError(ActiveError):
    """The earliest trial in the sweep already connected on the frame right
    after its own teleport, so the window's START is the sweep's lower bound
    rather than the hitbox's. Nothing distinguishes "active from frame 3" from
    "active from frame 3 because we could not look earlier"."""


class NonMonotoneWindowError(ActiveError):
    """The connect predicate was not one contiguous run of TRUEs. A hitbox
    that goes out, off and out again is possible in principle (multi-hit), but
    it is indistinguishable here from a transport flake, so it refuses rather
    than reports — `kit.NonMonotoneError`'s rule applied to this predicate."""


class VariantDriftError(ActiveError):
    """The connecting trials did not all deal the same damage, so they were
    not all the same move (docs/frames.md §5 keys variants on damage). This is
    what a close-variant sweep looks like from the inside."""


class GapDisagreementError(ActiveError):
    """Two target gaps produced different windows. Either one of them is at the
    reach fringe (where the window is the defender's animation phase, not the
    attack's clock) or the method is not sound for this move. §7: a number that
    fails re-measurement is DELETED, not averaged."""


# ── the position surface (injected — never a hardcoded address) ───────────


def resolve_object(session: Any, fighter: Any) -> Optional[int]:
    """This fighter's object-pool entry address, or `None`.

    The read half of this already exists as `observables.make_pointer_field_read`,
    but that returns a VALUE and the write needs the ADDRESS. Same declaration,
    same staleness rule: the pointer must be in the profile's declared range and
    `obj+cid_check_off` must equal the struct's own char id, or the answer is
    `None` — never a stale address, which here would mean poking a stranger.
    """
    p = fighter.ptr
    lead = -p.off
    buf = session.read_memory(fighter.base + p.off, lead + fighter.stride)
    raw = int.from_bytes(buf[: p.size], "little")
    if not (p.valid_lo <= raw < p.valid_hi):
        return None
    obj = (raw - p.bias) >> p.shift
    if session.read_memory(obj + p.char_off, 1)[0] != buf[lead]:
        return None
    return obj


@dataclass(frozen=True)
class PositionIO:
    """Reading and WRITING the gap between the two fighters, for one rig.

    Injected exactly like every other observable in this package: the callers
    build it from the profile's own `object_ptr` declaration (see
    `from_fighters`), never from a literal address. `side` picks which body the
    sweep teleports — `"defender"` by default, `"attacker"` as the independent
    cross-check, and the two were measured to agree.
    """

    read_gap: Callable[[Any], Optional[int]]
    write_gap: Callable[[Any, int], None]
    collision_floor_px: int
    side: str = "defender"

    @classmethod
    def from_fighters(
        cls,
        attacker: Any,
        defender: Any,
        *,
        collision_floor_px: int,
        side: str = "defender",
        read_x: Optional[Callable[[Any, Any], Optional[int]]] = None,
        write_x: Optional[Callable[[Any, Any, int], None]] = None,
    ) -> "PositionIO":
        """Build the surface from two `observables.FighterAddrs`.

        `read_x`/`write_x` take `(session, fighter[, value])` and default to
        the object-pointer path in `observables` — passed in rather than
        imported at call time so a test can drive this with no emulator.
        """
        if side not in ("defender", "attacker"):
            raise ValueError(f"side must be 'defender' or 'attacker', not {side!r}")
        if read_x is None or write_x is None:
            from . import observables as _obs

            rd_a = _obs.make_pointer_field_read(attacker, "x")
            rd_d = _obs.make_pointer_field_read(defender, "x")

            def _read(session: Any, fighter: Any) -> Optional[int]:
                return (rd_a if fighter is attacker else rd_d)(session)

            def _write(session: Any, fighter: Any, value: int) -> None:
                obj = resolve_object(session, fighter)
                if obj is None:
                    raise ActiveError(
                        f"{fighter.name}: the object pointer did not resolve (or "
                        "its char id did not cross-check), so there is no x to "
                        "write. docs/frames.md §4.2: discard, never synthesize."
                    )
                session.call(
                    "write_memory", addr=obj + fighter.ptr.x_off, len=2,
                    value=int(value),
                )

            read_x, write_x = _read, _write

        def read_gap(session: Any) -> Optional[int]:
            xa = read_x(session, attacker)
            xd = read_x(session, defender)
            if xa is None or xd is None:
                return None
            return xd - xa

        def write_gap(session: Any, gap: int) -> None:
            xa = read_x(session, attacker)
            xd = read_x(session, defender)
            if xa is None or xd is None:
                raise ActiveError(
                    "a fighter's x did not resolve, so the teleport target "
                    "cannot be computed from the live positions."
                )
            if side == "defender":
                write_x(session, defender, xa + gap)
            else:
                write_x(session, attacker, xd - gap)

        return cls(
            read_gap=read_gap,
            write_gap=write_gap,
            collision_floor_px=int(collision_floor_px),
            side=side,
        )


# ── one trial ─────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class TeleportTrial:
    teleport_at: int
    target_gap: int
    contact_frame: Optional[int]
    damage: Optional[int]
    frames: int

    @property
    def connected(self) -> bool:
        return self.contact_frame is not None


def _schedule(script: MoveScript, rig: Rig, *, defender_guard: bool,
              extra_lead: int = 0) -> Dict[int, Dict[int, Tuple[str, ...]]]:
    """frame -> port -> that port's ENTIRE held set from that frame on — the
    same contract as `probe._schedule`, minus the probe/guard-release entries
    this sweep never uses. `extra_lead` delays the attack by that many idle
    frames; it exists for the §3 phase test (shift the attack against the
    defender's idle animation and the window must shift with the attack)."""
    sched: Dict[int, Dict[int, Tuple[str, ...]]] = {}
    cursor = extra_lead
    for step in tuple(script.lead_in) + tuple(script.steps):
        sched.setdefault(cursor, {})[rig.attacker_port] = tuple(step.buttons)
        cursor += step.frames
    sched.setdefault(cursor, {})[rig.attacker_port] = ()
    sched.setdefault(0, {})[rig.defender_port] = (
        tuple(rig.guard_buttons) if defender_guard else ()
    )
    return sched


def teleport_trial(
    session: Any,
    *,
    rig: Rig,
    script: MoveScript,
    positions: PositionIO,
    contact_read: Callable[[Any], Any],
    teleport_at: int,
    target_gap: int,
    frames: int,
    defender_guard: bool = False,
    extra_lead: int = 0,
) -> TeleportTrial:
    """One replay with ONE teleport, written at the END of frame
    `teleport_at` — i.e. in place for frame `teleport_at + 1`, which is why a
    connecting trial's contact frame is `max(first_active, teleport_at + 1)`.

    The contact signal is sampled every frame (§4.1's anchor: the victim's
    struct health, which steps by the whole damage in one frame), so the
    returned `contact_frame` is the frame the register moved.
    """
    if teleport_at < MIN_TELEPORT_FRAME:
        raise ClippedWindowError(
            f"teleport_at={teleport_at} is below MIN_TELEPORT_FRAME="
            f"{MIN_TELEPORT_FRAME}: MK2 has not yet resolved which proximity "
            "variant of this button it is running, so the write would change "
            "the move instead of probing it."
        )
    if target_gap < positions.collision_floor_px:
        raise CollisionFloorError(
            f"target_gap={target_gap} px is inside the measured anti-overlap "
            f"floor ({positions.collision_floor_px} px): the game separates the "
            "two bodies at ~6 px/frame, so the written gap survives one frame "
            "and the trial does not test the distance it claims to."
        )

    sched = _schedule(script, rig, defender_guard=defender_guard,
                      extra_lead=extra_lead)
    session.load_state(rig.arena)
    baseline = contact_read(session)
    values = [baseline]
    pending = dict(sched.get(0, {}))
    for f in range(1, frames + 1):
        session.run_frames(1, holds=pending or None)
        pending = dict(sched.get(f, {}))
        values.append(contact_read(session))
        if f == teleport_at:
            positions.write_gap(session, target_gap)
    session.release_all_ports()

    hit = next((i for i in range(1, len(values)) if values[i] != values[i - 1]), None)
    damage = None
    if hit is not None and isinstance(baseline, int) and isinstance(values[hit], int):
        damage = baseline - values[hit]
    return TeleportTrial(
        teleport_at=teleport_at, target_gap=target_gap, contact_frame=hit,
        damage=damage, frames=frames,
    )


# ── the sweep ─────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class ActiveWindow:
    """`first_active_frame`/`last_active_frame` are RUN-relative (frame 0 = the
    loaded arena); `input_relative` subtracts the move's own lead-in so a
    crouching normal is comparable with a standing one and with the stored
    `first_active_frame` column."""

    move: str
    target_gap: int
    first_active_frame: int
    last_active_frame: int
    predicate: str
    damage: int
    attack_input_frame: int = 0
    trials: Tuple[TeleportTrial, ...] = field(default=(), repr=False)
    side: str = "defender"

    @property
    def active(self) -> int:
        return self.last_active_frame - self.first_active_frame + 1

    @property
    def input_relative(self) -> Tuple[int, int]:
        k = self.attack_input_frame
        return (self.first_active_frame - k, self.last_active_frame - k)


def sweep_active_window(
    session: Any,
    *,
    rig: Rig,
    script: MoveScript,
    positions: PositionIO,
    contact_read: Callable[[Any], Any],
    target_gap: int,
    max_search: int = 45,
    frames: Optional[int] = None,
    defender_guard: bool = False,
    extra_lead: int = 0,
    cap_margin: int = CAP_MARGIN,
) -> ActiveWindow:
    """Sweep the teleport frame N over `MIN_TELEPORT_FRAME..max_search` and
    read the active window off the connect predicate.

    Exhaustive by construction (§4.3: a linear sweep is the default and is
    unconditionally correct), and it REFUSES rather than reports on all four
    ways the predicate can fail to be a window: not monotone, clipped at the
    low end, capped at the high end, or not one move throughout.
    """
    total = frames if frames is not None else max_search + cap_margin + 20
    trials: List[TeleportTrial] = []
    for n in range(MIN_TELEPORT_FRAME, max_search + 1):
        trials.append(teleport_trial(
            session, rig=rig, script=script, positions=positions,
            contact_read=contact_read, teleport_at=n, target_gap=target_gap,
            frames=total, defender_guard=defender_guard, extra_lead=extra_lead,
        ))
    return window_from_trials(
        trials, move=script.name, attack_input_frame=script.attack_input_frame,
        max_search=max_search, cap_margin=cap_margin, side=positions.side,
    )


def window_from_trials(
    trials: Sequence[TeleportTrial],
    *,
    move: str,
    attack_input_frame: int = 0,
    max_search: Optional[int] = None,
    cap_margin: int = CAP_MARGIN,
    side: str = "defender",
) -> ActiveWindow:
    """The pure half of `sweep_active_window` — every refusal lives here, so
    the rules are testable without an emulator."""
    if not trials:
        raise ActiveError("no trials")
    pred = "".join("T" if t.connected else "F" for t in trials)
    hits = [t for t in trials if t.connected]
    if not hits:
        raise ActiveError(
            f"{move}: no teleport frame produced contact at "
            f"{trials[0].target_gap} px. docs/frames.md §1.1: that is a WHIFF "
            "at this distance, a result — not a zero-length active window."
        )
    if "F" in pred.strip("F"):
        raise NonMonotoneWindowError(
            f"{move} @ {trials[0].target_gap}px: connect predicate {pred} is not "
            "one contiguous run. A flake and a genuinely interrupted hitbox look "
            "identical here, so no window is reported."
        )
    damages = sorted({t.damage for t in hits if t.damage is not None})
    if len(damages) > 1:
        raise VariantDriftError(
            f"{move} @ {trials[0].target_gap}px: connecting trials dealt "
            f"{damages} damage, so they were not all the same move (§5 keys "
            "variants on damage). A proximity CLOSE variant cannot be measured "
            "this way — moving the defender out of range also moves it out of "
            "the bucket that selects the move."
        )
    first = hits[0].contact_frame
    assert first is not None
    if first <= hits[0].teleport_at:
        raise ActiveError(
            f"{move}: the earliest connecting trial contacted at frame {first}, "
            f"at or before its own teleport at {hits[0].teleport_at} — the move "
            "already connected without the teleport, so this rig does not "
            "isolate the hitbox. Start further out of range."
        )
    if any(t.contact_frame != max(first, t.teleport_at + 1) for t in hits):
        raise ActiveError(
            f"{move}: a connecting trial's contact frame was neither the "
            f"window's start ({first}) nor its own teleport+1, so contact is not "
            "being driven by the teleport. This rig does not isolate the hitbox."
        )
    if first == hits[0].teleport_at + 1:
        raise ClippedWindowError(
            f"{move} @ {trials[0].target_gap}px: the earliest trial "
            f"(N={hits[0].teleport_at}) connected on the very next frame, so the "
            "window's start is the sweep's lower bound, not the hitbox's."
        )
    last_n = hits[-1].teleport_at
    if max_search is not None and last_n > max_search - cap_margin:
        raise ActiveError(
            f"{move} @ {trials[0].target_gap}px: the last connecting teleport "
            f"(N={last_n}) is within {cap_margin} of max_search={max_search}. "
            "docs/frames.md §7 forbids a boundary measured against the edge of "
            "its own search — widen max_search and re-run."
        )
    return ActiveWindow(
        move=move, target_gap=trials[0].target_gap, first_active_frame=first,
        last_active_frame=last_n + 1, predicate=pred,
        damage=damages[0] if damages else 0,
        attack_input_frame=attack_input_frame, trials=tuple(trials), side=side,
    )


def measure_active(
    session: Any,
    *,
    rig: Rig,
    script: MoveScript,
    positions: PositionIO,
    contact_read: Callable[[Any], Any],
    target_gaps: Sequence[int],
    require_gap_agreement: bool = True,
    **kw: Any,
) -> ActiveWindow:
    """`sweep_active_window` at two or more distances, refusing unless they
    agree — the defence against kill criterion 3.

    This is not belt-and-braces. Within ~3 px of a move's reach the window's
    START pins to the DEFENDER'S idle animation (an absolute clock that ignores
    when the attack was thrown) instead of to the attack, and the boundary
    itself drifts between arenas. Two agreeing gaps, at least one of them well
    inside the reach, is what separates a real window from that artifact. The
    returned window is the one from the FIRST (innermost) gap.
    """
    if require_gap_agreement and len(target_gaps) < 2:
        raise ValueError(
            "require_gap_agreement needs at least two target gaps: one gap "
            "cannot tell an active window from the reach fringe."
        )
    windows = [
        sweep_active_window(
            session, rig=rig, script=script, positions=positions,
            contact_read=contact_read, target_gap=g, **kw,
        )
        for g in target_gaps
    ]
    if require_gap_agreement:
        spans = {(w.first_active_frame, w.last_active_frame) for w in windows}
        if len(spans) > 1:
            raise GapDisagreementError(
                f"{script.name}: gaps {list(target_gaps)} gave windows "
                + ", ".join(
                    f"{g}px→[{w.first_active_frame},{w.last_active_frame}]"
                    for g, w in zip(target_gaps, windows)
                )
                + ". At least one is at the reach fringe, where the window is "
                "the defender's animation phase. No row is written."
            )
    return windows[0]


# ── recovery and total ────────────────────────────────────────────────────


def derive_recovery(
    window: ActiveWindow, *, whiff_manifest_frame: int
) -> Dict[str, int]:
    """`active`, `recovery` and `total` from a window plus ONE more number.

    `whiff_manifest_frame` is the attacker's own walk-manifest frame, measured
    input-relative by §4.2's differential probe against a rig where the move
    NEVER CONNECTS. It must come from a whiff: on hit or on block the timeline
    contains hitstop, which §1.2 stores in its own column and never folds into
    recovery.

    Convention, stated because §4.3 rule 1 says every number must name one:
    `total` is "the earliest frame the attacker can START A WALK", the same
    convention the rest of this lab publishes (earliest-attack was measured at
    walk-manifest − 2). `recovery` is the frames from the last active frame to
    that, so `total = last_active + recovery = first_active + active - 1 +
    recovery` holds by construction.
    """
    first, last = window.input_relative
    if whiff_manifest_frame < last:
        raise ActiveError(
            f"{window.move}: whiff walk-manifest frame {whiff_manifest_frame} is "
            f"before the last active frame {last} — docs/frames.md §8.2's "
            "`total >= FAF + active` is violated, so one of the two "
            "measurements is wrong. Neither is reported."
        )
    return {
        "first_active_frame": first,
        "active": window.active,
        "last_active_frame": last,
        "recovery": whiff_manifest_frame - last,
        "total": whiff_manifest_frame,
    }
