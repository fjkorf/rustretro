"""A general two-port CHOREOGRAPHY engine for MK2 arcade — `docs/frames.md`
§4/§4.5's machinery, generalized from "one attacker script + one guarded
defender + one probe" (`probe.py`) into an arbitrary caller-supplied
sequence of BEATS, each a held-set program on either or both ports, each with
its own differential expectation.

`probe.replay` hardwires its schedule to the (script, guard, probe) triple
`_schedule` builds. This module builds the identical SHAPE of schedule
(`Dict[frame, Dict[port, Tuple[str, ...]]]`) from a caller-supplied `Event`
list instead, and drives it through `probe.run_schedule` — the executor loop
extracted out of `probe.replay` for exactly this reuse (next-stop /
pending-held / batching unchanged; see `probe.run_schedule`'s own docstring
and the existing `test_framelab_probe.py`/`test_framelab_replay.py` suites,
which are the proof nothing about `replay`'s own behaviour moved).

## What a Beat is, and is not

A `Beat` is a named span of frames with a held-set program (`Event`s, never
taps — see `Event`'s own docstring) and an optional `Expect`: a DIFFERENTIAL
check against what `execute()` observed. `verify()` is deliberately coarser
than `probe.py`'s §4 act-again sweep — it does not derive a frame-exact
advantage number, it confirms a choreographed sequence reproduces the SHAPE
`library/mk2/arcade.frames.json` already measured (contact happened or
didn't, a struct-health delta matched, a counter connected). The frame-exact
numbers live in that file; this module's job is to show the numbers holding
up as *play*, not to re-measure them.

## The two laws this module bakes in, not merely documents

`punish_beat` and `special_beat` exist because two MK2 input laws are easy to
violate by construction and hard to debug after the fact:

  * **Guard-release-before-counter** (`punish.py`'s header): dropping Block
    and pressing the counter on the SAME frame produces NO ATTACK AT ALL, not
    a late one. `punish_beat` defaults `guard_release_at` to
    `contact_frame + 1` (punish.py's own default) and RAISES if a caller asks
    for `counter_at == guard_release_at`.
  * **Fresh-onset motion inputs** (this module's and `kit.MoveSpec`'s shared
    finding): a direction chorded with the trigger button on the frame the
    direction first appears does not register — the direction needs to
    SETTLE first. `special_beat` always inserts `settle_frames` of the final
    motion direction alone before the trigger chords in.

## Slot export

`to_slot` writes `src/playback.rs`'s `InputSlot` schema directly (version 1;
all six fields; `state_note_at_start: null` unless given) from a compiled
`Schedule`, reusing `replay.JOYPAD_NAMES` for the bit order — that table is
`src/mcp/server.rs::JOYPAD_NAMES` / `src/record.rs::pack_mask`'s order,
already imported once for exactly this purpose (mask <-> button-name
round-tripping for a slot's frames); a third copy of a 12-entry Rust table
was judged worse than the one import.

## Beat sheets are DATA

A `.beats.json` file (schema documented at `load_beat_sheet`) names an arena,
a family/port, and a list of beats-as-JSON. `load_beat_sheet` resolves move
names ("HP", "Block", ...) to low-level button names via the PROFILE's own
`attack_chords` — never hardcoded (CLAUDE.md) — so the sheet itself names
moves in the family's vocabulary, exactly like `kit.MoveSpec`.
"""

from __future__ import annotations

import json
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import (
    Any,
    Callable,
    Dict,
    Hashable,
    List,
    Mapping,
    Optional,
    Sequence,
    Tuple,
)

from .probe import run_schedule
from .replay import JOYPAD_NAMES
from .session import LabSession

__all__ = [
    "Event",
    "Expect",
    "Beat",
    "Schedule",
    "ChoreoTrace",
    "BeatResult",
    "Report",
    "BeatSheet",
    "ChoreoError",
    "compile",
    "execute",
    "verify",
    "to_slot",
    "punish_beat",
    "special_beat",
    "load_beat_sheet",
    "load_frames_index",
    "expected_damage",
    "save_slot",
]


class ChoreoError(ValueError):
    """A beat sheet, a compiled schedule, or an exported slot could not be
    made trustworthy — a bad beat sheet is a bug in the DATA, never
    something this module should paper over with a guess (§7: no silent
    caps, no invented numbers)."""


Sampler = Callable[[LabSession], Mapping[str, Hashable]]


# ── the event/beat model ────────────────────────────────────────────────


@dataclass(frozen=True)
class Event:
    """One held-set ASSERTION: from `frame` (local to whatever `Beat` it
    belongs to) on, port `port`'s ENTIRE held set becomes `held` — REPLACES,
    never ORs, exactly `ScriptStep.buttons` / `hold_buttons` semantics, so
    one event fully describes a port's input from that frame forward.

    Never a tap. `docs/frames.md` §3.3 bans `press_buttons` in this lab
    (`LabSession`/`call_ok` enforce it by construction); an `Event` is a
    LEVEL, not an edge, so a tap is not expressible here even by mistake —
    the caller must assert-then-release across two events, same as a
    `ScriptStep`.
    """

    frame: int
    port: int
    held: Tuple[str, ...] = ()

    def __post_init__(self) -> None:
        if self.frame < 0:
            raise ChoreoError(f"Event.frame must be >= 0, got {self.frame}")
        object.__setattr__(self, "held", tuple(self.held))


@dataclass(frozen=True)
class Expect:
    """A beat's DIFFERENTIAL expectation. Every field is optional and
    independently checked — an unset field is NOT CHECKED, never a silent
    pass (§2 rule 5: absent means absent). Ports are always named EXPLICITLY
    rather than via an "attacker"/"defender" role, because a punish beat's
    offense is the ORIGINAL defender — a fixed role name would have to flip
    per beat kind, which is exactly the kind of implicit convention this lab
    has been burned by before.

    `health_deltas` is `{port: expected frames lost}`, meant to be sourced
    from `library/mk2/arcade.frames.json`'s own `damage` column via
    `expected_damage` — verify() only compares measured vs declared, it does
    not itself decide what a move "should" deal.
    """

    contact_port: Optional[int] = None
    contact: Optional[bool] = None

    guard_port: Optional[int] = None
    guard_buttons: Tuple[str, ...] = ()
    guard: Optional[bool] = None
    guard_check_frame: Optional[int] = None

    health_deltas: Mapping[int, int] = field(default_factory=dict)

    punish_port: Optional[int] = None
    punish_from: Optional[int] = None
    punish: Optional[bool] = None

    def __post_init__(self) -> None:
        object.__setattr__(
            self, "health_deltas", dict(self.health_deltas)
        )
        object.__setattr__(self, "guard_buttons", tuple(self.guard_buttons))


@dataclass(frozen=True)
class Beat:
    """A named span of `frames` local frames with a held-set program and an
    optional `Expect`. `frames` defaults to `max(event.frame) + 1` when not
    given — a hand-authored beat that just lists its events does not also
    have to count them."""

    name: str
    events: Tuple[Event, ...]
    expect: Expect = field(default_factory=Expect)
    frames: Optional[int] = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "events", tuple(self.events))

    @property
    def length(self) -> int:
        if self.frames is not None:
            return self.frames
        if not self.events:
            raise ChoreoError(
                f"beat {self.name!r} has no events and no explicit `frames` — "
                "there is nothing to infer a duration from."
            )
        return max(e.frame for e in self.events) + 1


# ── compile: beats -> one merged schedule ─────────────────────────────────


@dataclass(frozen=True)
class Schedule:
    """The compiled form of a beat list: one merged `frame -> port ->
    held-set` map (`probe._schedule`'s own shape, generalized), plus the
    per-beat frame spans (`[start, end)`, global/compiled numbering) that
    `verify()` needs to slice a trace back into beats."""

    frames: Dict[int, Dict[int, Tuple[str, ...]]]
    total_frames: int
    beats: Tuple[Beat, ...]
    beat_spans: Dict[str, Tuple[int, int]]


def compile(beats: Sequence[Beat]) -> Schedule:  # noqa: A001 - matches the plan's API name
    """Lay `beats` out back-to-back into one global schedule. Two beats
    touching the same port on the same global frame with DIFFERENT held sets
    is refused (ambiguous authorship — §7's "no silent guess" applied to
    scheduling); the identical held set twice is harmless and allowed.

    A trailing release is appended for every port any beat touched, exactly
    like `probe._schedule`'s own `put(cursor, attacker_port, ())` — so a beat
    sheet never has to remember to let go of a held button at the very end.
    """
    if not beats:
        raise ChoreoError("compile() needs at least one beat")
    merged: Dict[int, Dict[int, Tuple[str, ...]]] = {}
    spans: Dict[str, Tuple[int, int]] = {}
    ports: set = set()
    seen_names: set = set()
    cursor = 0

    for beat in beats:
        if beat.name in seen_names:
            raise ChoreoError(
                f"duplicate beat name {beat.name!r} — beat_spans/verify need "
                "unique names to address a beat's own window in the trace."
            )
        seen_names.add(beat.name)
        length = beat.length
        for e in beat.events:
            if e.frame > length:
                raise ChoreoError(
                    f"beat {beat.name!r}: event at local frame {e.frame} is "
                    f"past the beat's own span (0..{length}) — widen `frames` "
                    "or move the event."
                )
            g = cursor + e.frame
            ports.add(e.port)
            slot = merged.setdefault(g, {})
            if e.port in slot and slot[e.port] != e.held:
                raise ChoreoError(
                    f"beat {beat.name!r}: two different held sets asserted "
                    f"for port {e.port} at the same global frame {g} "
                    f"({slot[e.port]!r} vs {e.held!r})."
                )
            slot[e.port] = e.held
        spans[beat.name] = (cursor, cursor + length)
        cursor += length

    for port in sorted(ports):
        if port not in merged.get(cursor, {}):
            merged.setdefault(cursor, {})[port] = ()

    return Schedule(frames=merged, total_frames=cursor, beats=tuple(beats), beat_spans=spans)


def _held_at(
    frames: Mapping[int, Mapping[int, Tuple[str, ...]]], port: int, frame: int
) -> Tuple[str, ...]:
    """What `port` holds at global `frame`, per the compiled schedule alone
    — no emulator read. This is what makes an `Expect.guard` check "known
    from the rig" (docs/frames.md §2 rule 6): the lab DROVE both ports, so
    it already knows what it told them to hold."""
    held: Tuple[str, ...] = ()
    for f in sorted(k for k in frames if k <= frame):
        if port in frames[f]:
            held = frames[f][port]
    return held


# ── execute: drive the schedule through a live session ────────────────────


@dataclass(frozen=True)
class ChoreoTrace:
    """The raw per-frame record `execute()` produced. `frames[f]` is the
    sample dict `sample_fn` returned for global frame `f` (or `None` if
    unsampled, matching `probe.run_schedule`'s own `trace` shape) — index 0
    is the loaded arena before any step, same convention as `probe.replay`.
    """

    schedule: Schedule
    frames: Tuple[Optional[Mapping[str, Any]], ...]


def execute(
    session: LabSession,
    arena: str,
    schedule: Schedule,
    *,
    sample_fn: Optional[Sampler] = None,
    sample_from: int = 0,
) -> ChoreoTrace:
    """Load `arena`, drive `schedule` through `probe.run_schedule` (the
    battle-tested executor loop, unchanged — see that function's docstring),
    and release both ports at the end.

    `sample_fn` is expected to return a dict shaped like
    `{"health": {port: int, ...}, "contact": {port: Hashable, ...}}` if the
    caller wants `verify()`'s health/contact/punish checks to do anything —
    `verify()` treats a missing key as "not sampled" (never a false pass),
    so a sparser `sample_fn` just means fewer of a beat's `Expect` fields get
    checked, not an error.
    """
    session.load_state(arena)
    frames = run_schedule(
        session, schedule.frames, schedule.total_frames,
        sample_fn=sample_fn, sample_from=sample_from,
    )
    session.release_all_ports()
    return ChoreoTrace(schedule=schedule, frames=tuple(frames))


# ── verify: differential checks against a trace ────────────────────────────


@dataclass(frozen=True)
class BeatResult:
    name: str
    passed: bool
    checks: Dict[str, Optional[bool]]
    notes: Tuple[str, ...] = ()


@dataclass(frozen=True)
class Report:
    results: Tuple[BeatResult, ...]

    @property
    def all_passed(self) -> bool:
        return all(r.passed for r in self.results)

    @property
    def refused(self) -> Tuple[str, ...]:
        """Beats with at least one CHECKED expectation that failed — never
        a beat that merely had nothing to check."""
        return tuple(r.name for r in self.results if not r.passed)


def _window(frames: Sequence[Optional[Mapping[str, Any]]], start: int, end: int) -> list:
    return list(frames[start : end + 1])


def _health_series(window: Sequence[Optional[Mapping[str, Any]]], port: int) -> List[int]:
    out = []
    for f in window:
        if f is None:
            continue
        v = (f.get("health") or {}).get(port)
        if v is not None:
            out.append(v)
    return out


def _contact_series(window: Sequence[Optional[Mapping[str, Any]]], port: int) -> List[Hashable]:
    out = []
    for f in window:
        if f is None:
            continue
        v = (f.get("contact") or {}).get(port)
        if v is not None:
            out.append(v)
    return out


def verify(trace: ChoreoTrace, beats: Sequence[Beat]) -> Report:
    """Run every `beat.expect` in `beats` against `trace`. `beats` is
    ordinarily `trace.schedule.beats`, taken explicitly (matching the plan's
    `verify(trace, beats)` sketch) so a caller can verify a SUBSET without
    re-compiling.

    Coarser than `probe.py`'s §4 sweep by design (see the module docstring):
    contact is "did the observable change at all inside the beat's window",
    not a frame-exact anchor — the frame-exact numbers already live in
    `library/mk2/arcade.frames.json`; this checks the choreography
    reproduces their SHAPE.
    """
    spans = trace.schedule.beat_spans
    results: List[BeatResult] = []

    for beat in beats:
        if beat.name not in spans:
            raise ChoreoError(
                f"beat {beat.name!r} is not part of this trace's compiled "
                "schedule (spans: {}); verify a schedule you actually "
                "executed.".format(sorted(spans))
            )
        start, end = spans[beat.name]
        window = _window(trace.frames, start, end)
        exp = beat.expect
        checks: Dict[str, Optional[bool]] = {}
        notes: List[str] = []

        if exp.contact is not None:
            if exp.contact_port is None:
                checks["contact"] = None
                notes.append("expect.contact set but no contact_port — not checked")
            else:
                vals = _contact_series(window, exp.contact_port)
                if len(vals) < 2:
                    checks["contact"] = None
                    notes.append(
                        f"port {exp.contact_port}: fewer than 2 contact samples "
                        "in the beat window — not checked"
                    )
                else:
                    observed = any(vals[i] != vals[i - 1] for i in range(1, len(vals)))
                    checks["contact"] = observed == exp.contact
                    if observed != exp.contact:
                        notes.append(
                            f"contact {'observed' if observed else 'NOT observed'} "
                            f"on port {exp.contact_port}; expected "
                            f"{'contact' if exp.contact else 'no contact'}"
                        )

        if exp.guard is not None:
            if exp.guard_port is None or exp.guard_check_frame is None:
                checks["guard"] = None
                notes.append(
                    "expect.guard set but guard_port/guard_check_frame missing "
                    "— not checked"
                )
            else:
                g = start + exp.guard_check_frame
                held = _held_at(trace.schedule.frames, exp.guard_port, g)
                has_guard = (
                    set(exp.guard_buttons).issubset(set(held))
                    if exp.guard_buttons
                    else bool(held)
                )
                checks["guard"] = has_guard == exp.guard
                if has_guard != exp.guard:
                    notes.append(
                        f"port {exp.guard_port} at frame {g}: held={held!r} — "
                        f"expected {'guard held' if exp.guard else 'guard released'}"
                    )

        for port, want in exp.health_deltas.items():
            vals = _health_series(window, port)
            key = f"health_delta[{port}]"
            if len(vals) < 2:
                checks[key] = None
                notes.append(
                    f"port {port}: fewer than 2 health samples in the beat "
                    "window — not checked"
                )
                continue
            got = vals[0] - vals[-1]
            checks[key] = got == want
            if got != want:
                notes.append(
                    f"port {port}: expected to lose {want}, measured {got} "
                    f"({vals[0]} -> {vals[-1]})"
                )

        if exp.punish is not None:
            if exp.punish_port is None or exp.punish_from is None:
                checks["punish"] = None
                notes.append(
                    "expect.punish set but punish_port/punish_from missing "
                    "— not checked"
                )
            else:
                pwindow = _window(trace.frames, start + exp.punish_from, end)
                vals = _health_series(pwindow, exp.punish_port)
                if len(vals) < 2:
                    checks["punish"] = None
                    notes.append(
                        f"port {exp.punish_port}: fewer than 2 health samples "
                        "after the counter frame — not checked"
                    )
                else:
                    # punish.py's own damage-register read: lost = seen[0] -
                    # min(seen); connected iff lost > 0.
                    connected = (vals[0] - min(vals)) > 0
                    checks["punish"] = connected == exp.punish
                    if connected != exp.punish:
                        notes.append(
                            f"punish {'connected' if connected else 'did NOT connect'} "
                            f"({vals[0]} -> min {min(vals)}); expected "
                            f"{'connect' if exp.punish else 'no connect'}"
                        )

        if not checks:
            notes.append("this beat declares no `expect` — nothing was verified")

        checked = [v for v in checks.values() if v is not None]
        passed = all(checked) if checked else True
        results.append(BeatResult(name=beat.name, passed=passed, checks=checks, notes=tuple(notes)))

    return Report(results=tuple(results))


# ── law-encoding beat helpers ────────────────────────────────────────────


def punish_beat(
    name: str,
    *,
    attacker_port: int,
    defender_port: int,
    counter: Sequence[str],
    contact_frame: int = 0,
    counter_at: int,
    guard_release_at: Optional[int] = None,
    guard_buttons: Sequence[str] = (),
    counter_hold: int = 2,
    frames: Optional[int] = None,
    expect: Optional[Expect] = None,
) -> Beat:
    """One counter-attack window, punish.py-rig naming: `attacker_port` is
    the ORIGINAL mover (who takes the punish damage), `defender_port` is who
    was defending and now throws `counter`.

    Encodes punish.py's measured law by construction: releasing guard and
    pressing the counter on the SAME frame produces NO ATTACK AT ALL on
    MK2's block stance (punish.py's module docstring — measured on Mileena's
    blocked cHK: guard held to the counter frame gave zero contact at every
    n from +8 to +30). `guard_release_at` defaults to `contact_frame + 1`
    (punish.py's own `guard_release_lead` default) and this function RAISES
    if `counter_at == guard_release_at` rather than compile a beat that is
    known not to attack.
    """
    if guard_release_at is None:
        guard_release_at = contact_frame + 1
    if counter_at == guard_release_at:
        raise ChoreoError(
            f"punish_beat {name!r}: counter_at ({counter_at}) == "
            f"guard_release_at ({guard_release_at}) — punish.py's header: "
            "dropping guard and pressing the counter on the SAME frame "
            "produces NO ATTACK AT ALL on MK2's block stance. Press the "
            "counter at least one frame after the guard release."
        )
    if counter_at < guard_release_at:
        raise ChoreoError(
            f"punish_beat {name!r}: counter_at ({counter_at}) is before "
            f"guard_release_at ({guard_release_at}) — the counter would be "
            "pressed while still guarding, which cannot be an attack input "
            "either (Block replaces the held set)."
        )
    events = (
        Event(0, defender_port, tuple(guard_buttons)),
        Event(guard_release_at, defender_port, ()),
        Event(counter_at, defender_port, tuple(counter)),
        Event(counter_at + counter_hold, defender_port, ()),
    )
    total = frames if frames is not None else counter_at + counter_hold
    exp = expect if expect is not None else Expect(
        punish_port=attacker_port, punish_from=counter_at, punish=True
    )
    return Beat(name=name, events=events, expect=exp, frames=total)


def special_beat(
    name: str,
    *,
    port: int,
    steps: Sequence[Tuple[Sequence[str], int]],
    trigger: Sequence[str],
    settle_frames: int = 6,
    hold_frames: int = 2,
    frames: Optional[int] = None,
    expect: Optional[Expect] = None,
) -> Beat:
    """One special-move input program: `steps` (each `(held_buttons,
    duration)`, MoveScript-`ScriptStep` shape) plays the motion in sequence,
    then the FINAL step's held set stays alone for `settle_frames` before
    `trigger` chords in for `hold_frames`.

    The settle is the law, not decoration: `kit.MoveSpec`'s own measured
    finding (asserting `down + button` on the same frame from a standing
    start enters *something* that then contacts NOTHING at any ladder rung)
    and punish.py's guard-release finding are the same shape — MK2 does not
    register a direction chorded with a trigger button on the frame the
    direction first appears. `settle_frames=6` mirrors `MoveSpec`'s own
    default and carries the same caveat: it is the THRESHOLD measured on one
    ladder, not a safe default for every arena (see `kit.MoveSpec`'s
    docstring) — a caller with a different arena's own measured threshold
    should pass it explicitly.
    """
    if not steps:
        raise ChoreoError(f"special_beat {name!r}: `steps` must not be empty")
    if settle_frames < 1:
        raise ChoreoError(
            f"special_beat {name!r}: settle_frames must be >= 1 — the fresh-"
            "onset law this helper exists to encode is that the final motion "
            "direction must be held ALONE for at least one frame before the "
            "trigger chords in; settle_frames=0 would chord them on the same "
            "frame, exactly the shape that does not register."
        )
    events: List[Event] = []
    cursor = 0
    held: Tuple[str, ...] = ()
    for buttons, dur in steps:
        if dur < 1:
            raise ChoreoError(f"special_beat {name!r}: every step duration must be >= 1")
        held = tuple(buttons)
        events.append(Event(cursor, port, held))
        cursor += dur
    settle_at = cursor
    cursor += settle_frames
    trigger_at = cursor
    events.append(Event(trigger_at, port, held + tuple(trigger)))
    cursor += hold_frames
    events.append(Event(cursor, port, ()))
    total = frames if frames is not None else cursor
    del settle_at  # documented via settle_frames; kept named for readability
    return Beat(name=name, events=tuple(events), expect=expect or Expect(), frames=total)


# ── slot export (src/playback.rs InputSlot, version 1) ─────────────────────


def _mask_from_buttons(buttons: Sequence[str]) -> int:
    mask = 0
    for b in buttons:
        bl = b.lower()
        if bl not in JOYPAD_NAMES:
            raise ChoreoError(
                f"unknown retropad button {b!r} — must be one of {JOYPAD_NAMES} "
                "(src/mcp/server.rs::JOYPAD_NAMES bit order)."
            )
        mask |= 1 << JOYPAD_NAMES.index(bl)
    return mask


def to_slot(
    schedule: Schedule,
    total_frames: Optional[int] = None,
    *,
    family: str,
    port: str,
    state_note_at_start: Optional[str] = None,
    created_at: Optional[int] = None,
) -> Dict[str, Any]:
    """A compiled `Schedule` -> `src/playback.rs`'s `InputSlot` JSON
    (version 1; all six fields required; `state_note_at_start: null` unless
    given). Verified against `src/playback.rs:111-122`: `frames` is
    `[[p1_mask, p2_mask], ...]`, 12-bit masks, bit order from
    `replay.JOYPAD_NAMES` (== `src/mcp/server.rs::JOYPAD_NAMES` ==
    `src/record.rs::pack_mask`'s order).

    `total_frames` defaults to `schedule.total_frames`; a larger value pads
    with released input (both ports empty) past the choreography's own end,
    a smaller value truncates (dropping any beat spans past it — the
    caller's call, not silently done for them).
    """
    n = schedule.total_frames if total_frames is None else int(total_frames)
    if n < 0:
        raise ChoreoError(f"to_slot: total_frames must be >= 0, got {n}")
    frames_out: List[List[int]] = []
    held: Dict[int, Tuple[str, ...]] = {0: (), 1: ()}
    for f in range(n):
        if f in schedule.frames:
            held.update(schedule.frames[f])
        frames_out.append([
            _mask_from_buttons(held.get(0, ())),
            _mask_from_buttons(held.get(1, ())),
        ])
    return {
        "version": 1,
        "family": family,
        "port": port,
        "created_at": int(created_at if created_at is not None else time.time()),
        "state_note_at_start": state_note_at_start,
        "frames": frames_out,
    }


def save_slot(slot: Mapping[str, Any], name: str, *, root: str = "shadow/inputs") -> Path:
    """Write `slot` (as returned by `to_slot`) to
    `shadow/inputs/<family>/<name>.slot.json`, matching `src/playback.rs`'s
    own layout and its `sanitize_name` rule (no `/`, `\\`, `.`/`..`)."""
    n = name.strip()
    if not n or "/" in n or "\\" in n or n in (".", ".."):
        raise ChoreoError(f"invalid slot name {name!r}: no path separators or '..'")
    family = slot["family"]
    out_dir = Path(root) / family
    out_dir.mkdir(parents=True, exist_ok=True)
    path = out_dir / f"{n}.slot.json"
    path.write_text(json.dumps(dict(slot), indent=2) + "\n")
    return path


# ── beat sheets are DATA: schema + loader ───────────────────────────────────


@dataclass(frozen=True)
class BeatSheet:
    name: str
    arena: str
    family: str
    port: str
    beats: Tuple[Beat, ...]
    citation: str = ""


def _resolve_buttons(move: str, attack_chords: Mapping[str, Sequence[str]]) -> Tuple[str, ...]:
    if move not in attack_chords:
        raise ChoreoError(
            f"move {move!r} is not in this profile's attack_chords "
            f"({sorted(attack_chords)}) — CLAUDE.md: never hardcode a button "
            "mapping, and this beat sheet named a move the profile doesn't know."
        )
    return tuple(attack_chords[move])


def _expect_from_json(raw: Optional[Mapping[str, Any]]) -> Expect:
    if not raw:
        return Expect()
    hd = raw.get("health_deltas") or {}
    return Expect(
        contact_port=raw.get("contact_port"),
        contact=raw.get("contact"),
        guard_port=raw.get("guard_port"),
        guard_buttons=tuple(raw.get("guard_buttons", ())),
        guard=raw.get("guard"),
        guard_check_frame=raw.get("guard_check_frame"),
        health_deltas={int(k): int(v) for k, v in hd.items()},
        punish_port=raw.get("punish_port"),
        punish_from=raw.get("punish_from"),
        punish=raw.get("punish"),
    )


def _beat_from_json(raw: Mapping[str, Any], *, attack_chords: Mapping[str, Sequence[str]]) -> Beat:
    """One `.beats.json` beat entry -> a `Beat`. `kind` dispatches:

    - `"raw"`: an explicit `events: [{frame, port, held}, ...]` list.
    - `"attack"`: a single move press (`port`, `move`, `hold_frames`),
      optionally with an opponent `guard` block (`port`, `buttons?`,
      `release_at?`) — the `kit.move_script` shape, generalized to two ports.
    - `"punish"`: dispatches to `punish_beat`.
    - `"special"`: dispatches to `special_beat`.
    """
    name = raw["name"]
    kind = raw.get("kind", "raw")
    expect = _expect_from_json(raw.get("expect"))
    frames = raw.get("frames")

    if kind == "raw":
        events = tuple(
            Event(int(e["frame"]), int(e["port"]), tuple(e.get("held", ())))
            for e in raw["events"]
        )
        return Beat(name=name, events=events, expect=expect, frames=frames)

    if kind == "attack":
        port = int(raw["port"])
        buttons = _resolve_buttons(raw["move"], attack_chords)
        hold = int(raw.get("hold_frames", 2))
        events = [Event(0, port, buttons), Event(hold, port, ())]
        guard = raw.get("guard")
        if guard:
            g_port = int(guard["port"])
            g_buttons = (
                tuple(guard["buttons"])
                if guard.get("buttons") is not None
                else _resolve_buttons("Block", attack_chords)
            )
            events.append(Event(0, g_port, g_buttons))
            release_at = guard.get("release_at")
            if release_at is not None:
                events.append(Event(int(release_at), g_port, ()))
        return Beat(name=name, events=tuple(events), expect=expect, frames=frames)

    if kind == "punish":
        return punish_beat(
            name,
            attacker_port=int(raw["attacker_port"]),
            defender_port=int(raw["defender_port"]),
            counter=_resolve_buttons(raw["counter"], attack_chords),
            contact_frame=int(raw.get("contact_frame", 0)),
            counter_at=int(raw["counter_at"]),
            guard_release_at=(
                int(raw["guard_release_at"]) if "guard_release_at" in raw else None
            ),
            guard_buttons=_resolve_buttons("Block", attack_chords),
            counter_hold=int(raw.get("counter_hold", 2)),
            frames=frames,
            expect=expect if raw.get("expect") else None,
        )

    if kind == "special":
        steps = [
            (tuple(s["buttons"]), int(s["frames"])) for s in raw["steps"]
        ]
        return special_beat(
            name,
            port=int(raw["port"]),
            steps=steps,
            trigger=_resolve_buttons(raw["trigger"], attack_chords),
            settle_frames=int(raw.get("settle_frames", 6)),
            hold_frames=int(raw.get("hold_frames", 2)),
            frames=frames,
            expect=expect,
        )

    raise ChoreoError(f"beat {name!r}: unknown kind {kind!r}")


def load_beat_sheet(path: str, *, attack_chords: Mapping[str, Sequence[str]]) -> BeatSheet:
    """Load a `.beats.json` file:

        {
          "name": "...", "arena": "shadow/arenas/mk2/gap-45.state",
          "family": "mk2", "port": "arcade", "citation": "...",
          "beats": [ {"name": ..., "kind": "raw"|"attack"|"punish"|"special",
                       "frames": <int, optional>, "expect": {...}, ...}, ... ]
        }

    `attack_chords` is the profile's own dict (never hardcoded here —
    CLAUDE.md), used to resolve move names to low-level button names for the
    `"attack"`/`"punish"`/`"special"` beat kinds.
    """
    data = json.loads(Path(path).read_text())
    beats = tuple(
        _beat_from_json(b, attack_chords=attack_chords) for b in data["beats"]
    )
    return BeatSheet(
        name=data["name"],
        arena=data["arena"],
        family=data.get("family", "mk2"),
        port=data.get("port", "arcade"),
        beats=beats,
        citation=data.get("citation", ""),
    )


# ── library/<family>/<port>.frames.json damage lookups ─────────────────────


def load_frames_index(path: str) -> List[Mapping[str, Any]]:
    """The `moves` rows of a `<port>.frames.json` export (§6's flat-JSON
    schema) — used by beat-sheet authors to cite `expected_damage` rather
    than hand-typing a number that can drift from the measured table."""
    data = json.loads(Path(path).read_text())
    return list(data.get("moves", []))


def expected_damage(
    rows: Sequence[Mapping[str, Any]],
    *,
    char: str,
    move: str,
    variant: Optional[str] = None,
    gap_px: Optional[float] = None,
) -> Optional[int]:
    """The `damage` column of the first row matching `char`/`move` (and
    `variant`/`gap_px` when given). `None` — never 0 — when nothing matches
    (§2 rule 5): a beat sheet citing a move this table never measured is a
    beat sheet with a hole in it, not a beat sheet claiming zero damage."""
    for row in rows:
        if row.get("char") != char or row.get("move") != move:
            continue
        if variant is not None and row.get("variant") != variant:
            continue
        if gap_px is not None and row.get("gap_px") != gap_px:
            continue
        dmg = row.get("damage")
        return int(dmg) if dmg is not None else None
    return None
