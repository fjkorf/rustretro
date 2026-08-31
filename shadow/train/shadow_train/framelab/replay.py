"""Measuring against RECORDED INPUT SLOTS instead of synthesized scripts —
docs/frames.md §4, with the move source swapped out from under it.

`probe.MoveScript` describes a move as a program ("hold back+LK+LP for 8
frames"). `src/playback.rs`'s input slots describe one instead as a
transcript: both ports' post-fold masks, one entry per real emulated frame,
captured off an actual session and replayed byte-identically from a save
state (MCP `record_inputs` / `play_inputs` / `list_input_slots`, stored under
`shadow/inputs/<family>/`). A slot is a REAL execution, which makes it a
legitimate move source and connects this lab to actual play rather than to
the lab's own idea of what a move is.

## The rule this module exists to enforce

**A row is anchored on OBSERVED contact, never on the replay's expected
contact.** A slot is only valid against the state it was recorded from: the
same transcript replayed from a different rung of the spacing ladder can come
out later, whiff, or not come out at all. So every replay is CLASSIFIED
before anything is derived from it, and the classifications that this module
cannot vouch for produce no row at all:

    replay slot from arena
       ├─ executed input stream != the slot's own frames ──► DIVERGED
       │      the replay did not actually replay. No row; counted.
       ├─ no attack signal at all ─────────────────────────► NO_EXECUTE
       │      the transcript ran but the move never came out. No row; counted.
       ├─ attack, but the contact signal never fires ──────► WHIFF
       │      a legitimate RESULT (§1.1) — and still not an advantage row.
       └─ contact observed at frame C
            ├─ C == expected ───────────────────────────────► ON_TIME
            └─ C != expected ───────────────────────────────► RETIMED

**RETIMED is not an error.** It is the interesting case: the measurement is
valid, it simply happens at a different frame than the recording did. The
`Anchor` this module hands downstream is built on the OBSERVED C in both
cases, and `contact_delta = C − expected` is recorded so a reader can see how
far the replay drifted from its origin state. Silently reusing the origin's
contact frame is exactly the failure the rule above forbids: it would anchor
a whole sweep on a frame the game did not agree with, and every number
downstream would be wrong by the delta with nothing in the row to say so.

## Why the divergence test is an INPUT test, not a state test

The obvious reading of "the trace diverged" is "the game state went somewhere
else". That test cannot be used across arenas, because a different starting
gap is *supposed* to produce a different state trace — it would report
DIVERGED for every legitimate cross-arena replay, which is the entire point
of doing this.

What must be identical across arenas is the INPUT: the same transcript, frame
for frame. So `DIVERGED` here means the frames that actually ran did not see
the slot's own masks — the playback did not start, dropped frames, was
overridden by another writer (the training dummy, a shadow runner, a stale
`hold_buttons`), or ended early. That is arena-independent, and it is
readable only because `get_input` now reports `executed_buttons`: STICKY, and
updated only on frames that really ran. `folded_buttons` cannot answer it —
it is re-folded every host tick whether a frame ran or not, so a read after
the fact shows the current held set rather than what the frame saw (see
`session.confirm_fold`'s own note on this).

The state-trace comparison is not discarded; it moves to where it IS valid —
`determinism_check`, which replays ONE slot TWICE from ONE state and requires
the traces to be identical. That is a different kind of claim, and it is
reported differently: see `DeterminismReport`.

## The one-frame lead-in

`playback::tick` runs at the END of `Frontend::run_frame`, after `core.run()`,
so the mask it applies for slot index `i` is folded into the NEXT emulated
frame. A `Manual` playback armed while paused therefore executes:

    frame 1: whatever was held before (nothing — `load_state` releases)
    frame 2: slot.frames[0]
    frame k: slot.frames[k - 2]

i.e. a constant offset of `INPUT_OFFSET = 2` frames between a replay-relative
frame index and the slot index it executes. It is a constant, it is the same
on every replay, and it cancels out of any comparison between two replays —
but it must be right for the DIVERGED check to align, so it is a named
parameter that `run_replay` reports back rather than a buried literal. Note
`playback.rs`'s own warning: this is frame-exact ONLY under pause → arm →
step, which is what `run_replay` does. Against a live session the arm/tick
race makes the start frame nondeterministic.

## What this module does NOT do yet, said plainly

It produces a classified, re-anchored `probe.Anchor` plus provenance. It does
not run the §4.2 act-again sweep from a slot, and the gap is not cosmetic:
`probe.replay` drives the attacker port from a `MoveScript`, so a sweep whose
move comes from a slot needs the attacker port driven by `play_inputs`
instead. That is feasible for the DEFENDER's sweep (playback owns p1, the
probe holds on p2 — two different ports, no conflict) and NOT feasible as
written for the ATTACKER's own sweep, where the probe must hold a walk on the
very port the playback is replacing the held set on every frame. Until that
is resolved, a replay-sourced `on_hit`/`on_block` would be half script-driven
and half slot-driven, which is two experiments in one row (§6), so this
module stops at the anchor rather than shipping one.

## Name collision, stated so it is not discovered the hard way

`framelab/__init__.py` re-exports `probe.replay` (the script-driven replay
FUNCTION) under the bare name `replay`, which is also this MODULE's name.
Importing this module rebinds `shadow_train.framelab.replay` from that
function to this module. Nothing in the tree reads the package attribute
today — every caller imports `from .probe import replay` directly — but the
re-export should lose the bare name (it is outside this module's remit to
change). Import THIS module as
`from shadow_train.framelab.replay import ...`, never
`from shadow_train.framelab import replay`.
"""

from __future__ import annotations

import json
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

from .probe import Anchor, DEFAULT_QUIET_FRAMES, ProbeError, _cluster_first_contact
from .session import LabError, LabSession

__all__ = [
    "DIVERGED",
    "NO_EXECUTE",
    "WHIFF",
    "ON_TIME",
    "RETIMED",
    "CLASSIFICATIONS",
    "ROW_CLASSIFICATIONS",
    "REFUSAL_CLASSIFICATIONS",
    "INPUT_OFFSET",
    "JOYPAD_NAMES",
    "ReplayError",
    "ReplayOriginError",
    "DeterminismAlarm",
    "InputSlot",
    "ReplayObservation",
    "ReplayOrigin",
    "ReplayMeasurement",
    "ReplayLedger",
    "DeterminismReport",
    "buttons_from_mask",
    "start_playback",
    "stop_playback",
    "run_replay",
    "classify_replay",
    "establish_origin",
    "measure_replay",
    "determinism_check",
]


# ── classifications ───────────────────────────────────────────────────────

DIVERGED = "DIVERGED"
NO_EXECUTE = "NO-EXECUTE"
WHIFF = "WHIFF"
ON_TIME = "ON-TIME"
RETIMED = "RETIMED"

CLASSIFICATIONS: Tuple[str, ...] = (DIVERGED, NO_EXECUTE, WHIFF, ON_TIME, RETIMED)

#: The only two classifications an advantage row may be built from. WHIFF is
#: deliberately NOT here: §1.1 makes a whiff a legitimate OUTCOME with no
#: advantage number, so it is a result that is not a row.
ROW_CLASSIFICATIONS: Tuple[str, ...] = (ON_TIME, RETIMED)

#: Classifications the lab cannot vouch for at all. These are not results —
#: nothing was measured — so they are counted and nothing else.
REFUSAL_CLASSIFICATIONS: Tuple[str, ...] = (DIVERGED, NO_EXECUTE)

#: See the module docstring: `playback::tick` applies slot index `i` after the
#: frame runs, so replay-relative frame `f` executes `slot.frames[f - 2]`.
INPUT_OFFSET = 2

#: `src/mcp/server.rs::JOYPAD_NAMES` — RETRO_DEVICE_ID_JOYPAD index order,
#: which is also `src/record.rs::pack_mask`'s bit order, which is what an
#: `InputSlot`'s frames are packed in. One table, because a slot mask and an
#: `executed_buttons` list have to be comparable and they come from the two
#: different sides of that table.
JOYPAD_NAMES: Tuple[str, ...] = (
    "b", "y", "select", "start", "up", "down", "left", "right", "a", "x", "l", "r",
)


class ReplayError(LabError):
    """A replay-sourced measurement could not be made trustworthy."""


class ReplayOriginError(ReplayError):
    """A slot's ORIGIN measurement failed, so there is no expected contact
    frame to classify later replays against. Refused rather than defaulted:
    an expected contact of 0 (or of "whatever the first replay said") would
    make every subsequent replay ON-TIME or RETIMED against a fiction."""


class DeterminismAlarm(LabError):
    """Two replays of one slot from one state produced different traces.

    This is a SYSTEM ALARM, not a measurement result. Nothing about the moves
    was learned; what was learned is that the rig is not reproducible, which
    makes EVERY measurement taken on that session suspect — including ones
    that already looked clean. It is deliberately a different type from
    `ReplayError` so a caller cannot handle it as "this cell failed".
    """


def buttons_from_mask(mask: int) -> frozenset:
    """A slot frame's packed `u16` -> the button-name set `get_input`'s
    `executed_buttons` reports. Both sides of `JOYPAD_NAMES`."""
    return frozenset(
        JOYPAD_NAMES[i] for i in range(len(JOYPAD_NAMES)) if mask & (1 << i)
    )


# ── the slot on disk ──────────────────────────────────────────────────────


@dataclass(frozen=True)
class InputSlot:
    """One `shadow/inputs/<family>/<name>.slot.json`, as written by
    `src/playback.rs`. `frames[i]` is `(p0_mask, p1_mask)` for the i-th real
    emulated frame of the recording.

    The slot file is read HERE rather than being asked for over MCP because
    the expected-input stream is the reference the DIVERGED check compares
    against, and reading it from the same server that is replaying it would
    make the check compare the server against itself.
    """

    name: str
    family: str
    port: str
    frames: Tuple[Tuple[int, int], ...]
    created_at: Optional[int] = None
    state_note_at_start: Optional[str] = None
    path: Optional[str] = None

    @classmethod
    def load(cls, family: str, name: str, *, root: str = "shadow/inputs") -> "InputSlot":
        p = Path(root) / family / f"{name}.slot.json"
        try:
            data = json.loads(p.read_text())
        except FileNotFoundError as exc:
            raise ReplayError(
                f"input slot {name!r} not found at {p} -- record it first "
                "(MCP `record_inputs`), or check the family."
            ) from exc
        if data.get("family") != family:
            raise ReplayError(
                f"{p}: slot was recorded for family {data.get('family')!r}, not "
                f"{family!r}. Cross-family replay is meaningless -- the button "
                "map, the arenas and the anchor are all per-port."
            )
        return cls(
            name=name,
            family=str(data.get("family", family)),
            port=str(data.get("port", "")),
            frames=tuple((int(a), int(b)) for a, b in data.get("frames", [])),
            created_at=data.get("created_at"),
            state_note_at_start=data.get("state_note_at_start"),
            path=str(p),
        )

    def __len__(self) -> int:
        return len(self.frames)

    def executed_expected(self, frame: int, port: int, *, offset: int = INPUT_OFFSET):
        """What port `port` should have executed on replay-relative frame
        `frame`, or `None` where the slot makes no claim.

        `None` (not `frozenset()`) outside the transcript's own span: before
        the first applied mask the port holds whatever preceded the replay,
        and after the last one `playback::tick` releases the ports one tick
        late (see its own comment), so those frames are UNCONSTRAINED. Calling
        them "expected empty" would manufacture a DIVERGED out of a documented
        boundary behaviour.
        """
        i = frame - offset
        if i < 0 or i >= len(self.frames):
            return None
        return buttons_from_mask(self.frames[i][port])


# ── driving a playback ────────────────────────────────────────────────────


def _ports_driven(port: str) -> Tuple[int, ...]:
    return {"p1": (0,), "p2": (1,), "both": (0, 1)}[port]


def start_playback(
    session: LabSession, slot: str, *, port: str = "both", trigger: str = "manual"
) -> int:
    """Arm `slot` on `port`. Returns the slot's frame count.

    Any playback already in flight is stopped first: `play_inputs` refuses to
    arm over one, and a leftover playback from a previous trial driving a port
    is precisely the cross-contamination `LabSession.load_state` releases held
    input to prevent.
    """
    stop_playback(session)
    r = session.call("play_inputs", action="start", name=slot, port=port, trigger=trigger)
    return int(r.get("frames", 0))


def stop_playback(session: LabSession) -> bool:
    """Stop any active/armed playback. `False` if there was none — that is the
    normal case and not an error, so the server's "no playback active" is
    swallowed here and nowhere else."""
    try:
        session.call("play_inputs", action="stop")
        return True
    except LabError:
        return False


# ── one replay, observed ──────────────────────────────────────────────────

Sampler = Callable[[LabSession], Hashable]


@dataclass(frozen=True)
class ReplayObservation:
    """The raw per-frame record of ONE replay. No classification, no anchor —
    everything derived is derived from here so that the derivation can be
    re-run (and argued with) without touching the emulator again."""

    slot: str
    arena: str
    port: str
    frames: int
    input_offset: int
    contact_trace: Tuple[Hashable, ...]
    contact_frames: Tuple[int, ...]
    attack_frames: Tuple[int, ...]
    executed: Tuple[Optional[frozenset], ...]
    expected_inputs: Tuple[Optional[Dict[int, frozenset]], ...] = ()
    input_divergence_frame: Optional[int] = None
    input_divergence_note: str = ""
    executed_available: bool = True
    state_trace: Tuple[Hashable, ...] = ()

    @property
    def input_matched(self) -> bool:
        return self.input_divergence_frame is None


def run_replay(
    session: LabSession,
    *,
    slot: InputSlot,
    arena: str,
    total_frames: int,
    contact_read: Callable[[LabSession], Hashable],
    attack_read: Optional[Callable[[LabSession], Hashable]] = None,
    state_read: Optional[Sampler] = None,
    port: str = "p1",
    input_offset: int = INPUT_OFFSET,
    check_inputs: bool = True,
) -> ReplayObservation:
    """Load `arena`, replay `slot` onto `port`, and watch every frame.

    Frame-exact by `playback.rs`'s own protocol: `LabSession.load_state`
    leaves the emulator PAUSED, the playback is armed while paused, and every
    frame after that is a confirmed `step`. Nothing here batches — the whole
    point is the per-frame record, and `run_frames` cannot be interrogated
    between its frames.

    Per frame it reads, in this order: the contact signal (§4.1's anchor), the
    attack signal, an optional state fingerprint, and — for every port the
    playback drives — `executed_buttons`. That last one is what makes
    `DIVERGED` detectable at all; `check_inputs=False` turns it off for a
    server too old to report it, at the cost of not being able to tell a
    replay that ran from one that did not.
    """
    driven = _ports_driven(port)
    stop_playback(session)
    session.load_state(arena)
    start_playback(session, slot.name, port=port, trigger="manual")

    contact: List[Hashable] = []
    states: List[Hashable] = []
    executed: List[Optional[frozenset]] = []
    expected: List[Optional[Dict[int, frozenset]]] = []
    attacks: List[int] = []
    prev_attack: Any = None
    divergence: Optional[int] = None
    div_note = ""
    executed_available = True

    try:
        for f in range(total_frames + 1):
            if f:
                session.step()
            contact.append(contact_read(session))
            if attack_read is not None:
                cur = attack_read(session)
                if prev_attack is not None and cur != prev_attack:
                    attacks.append(f)
                prev_attack = cur
            if state_read is not None:
                states.append(state_read(session))

            want = {
                p: slot.executed_expected(f, p, offset=input_offset) for p in driven
            }
            want = {p: v for p, v in want.items() if v is not None}
            expected.append(want or None)
            if check_inputs and want and executed_available:
                got_any: Optional[frozenset] = None
                for p, w in want.items():
                    got = session.executed(p)
                    if got is None:
                        # Server predates `executed_*`. Stop asking, and say
                        # so -- "we did not look" must not read as "it matched".
                        executed_available = False
                        break
                    got_any = got
                    if got != w and divergence is None:
                        divergence = f
                        div_note = (
                            f"frame {f}: port {p} executed {sorted(got)}, the "
                            f"slot's frame {f - input_offset} says "
                            f"{sorted(w)}"
                        )
                executed.append(got_any)
            else:
                executed.append(None)
    finally:
        stop_playback(session)
        session.release_all_ports()

    edges = [
        i for i in range(1, len(contact)) if contact[i] != contact[i - 1]
    ]
    if not check_inputs or not executed_available:
        div_note = div_note or (
            "input stream NOT checked: this server does not report "
            "get_input.executed_buttons, so a replay that silently did not "
            "run is indistinguishable from one that did."
        )
    return ReplayObservation(
        slot=slot.name,
        arena=arena,
        port=port,
        frames=total_frames,
        input_offset=input_offset,
        contact_trace=tuple(contact),
        contact_frames=tuple(edges),
        attack_frames=tuple(attacks),
        executed=tuple(executed),
        expected_inputs=tuple(expected),
        input_divergence_frame=divergence,
        input_divergence_note=div_note,
        executed_available=check_inputs and executed_available,
        state_trace=tuple(states),
    )


# ── classification ────────────────────────────────────────────────────────


@dataclass(frozen=True)
class ReplayOrigin:
    """What one slot measured AT THE STATE IT WAS RECORDED FROM — the only
    state a slot is unconditionally valid against, and therefore the only
    honest definition of its "expected" contact frame."""

    slot: str
    arena: str
    port: str
    expected_contact: int
    hits: int
    input_offset: int
    contact_frames: Tuple[int, ...] = ()
    state_trace: Tuple[Hashable, ...] = ()


@dataclass(frozen=True)
class ReplayMeasurement:
    """One classified replay. `observed_contact` is the ONLY contact frame
    anything downstream may use — see `anchor`."""

    slot: str
    arena: str
    classification: str
    observed_contact: Optional[int]
    expected_contact: Optional[int]
    contact_delta: Optional[int]
    hits: Optional[int]
    contact_frames: Tuple[int, ...]
    attack_frames: Tuple[int, ...]
    origin_arena: Optional[str]
    quiet_frames: int
    note: str = ""
    observation: Optional[ReplayObservation] = field(default=None, repr=False)

    @property
    def produces_row(self) -> bool:
        return self.classification in ROW_CLASSIFICATIONS

    @property
    def is_refusal(self) -> bool:
        """DIVERGED / NO-EXECUTE: nothing was measured. A WHIFF is NOT a
        refusal — it is a result that happens to have no advantage number."""
        return self.classification in REFUSAL_CLASSIFICATIONS

    def anchor(self) -> Anchor:
        """A `probe.Anchor` on the OBSERVED contact frame, for a downstream
        sweep. Refuses for every classification that has no contact, rather
        than falling back to `expected_contact` — falling back is the exact
        mistake this module exists to make impossible."""
        if not self.produces_row or self.observed_contact is None:
            raise ReplayError(
                f"replay {self.slot!r} on {self.arena} classified "
                f"{self.classification}: there is no observed contact to "
                "anchor on, and the recording's expected contact "
                f"({self.expected_contact}) is NOT a substitute -- a slot is "
                "only valid against the state it was recorded from."
            )
        return Anchor(
            contact_frame=self.observed_contact,
            hits=self.hits or 1,
            contact_frames=self.contact_frames,
            quiet_frames=self.quiet_frames,
            trace=self.observation.contact_trace if self.observation else (),
        )

    def provenance(self) -> Dict[str, Any]:
        """What a row measured from this replay must carry so a reader can
        tell it from a script-sourced one — and, for a RETIMED row, how far
        the replay drifted from its origin.

        §6's schema has no columns for any of this yet (§12 already lists "no
        arena / evidence column" as an open gap; this is the same gap, one
        step wider). Returned as a dict rather than silently dropped so the
        caller can store it beside the row instead of losing it.
        """
        return {
            "move_source": "replay",
            "replay_slot": self.slot,
            "replay_arena": self.arena,
            "replay_origin_arena": self.origin_arena,
            "replay_classification": self.classification,
            "replay_expected_contact": self.expected_contact,
            "replay_observed_contact": self.observed_contact,
            "replay_contact_delta": self.contact_delta,
        }


def classify_replay(
    obs: ReplayObservation,
    *,
    expected_contact: Optional[int],
    origin_arena: Optional[str] = None,
    quiet_frames: int = DEFAULT_QUIET_FRAMES,
    require_attack_signal: bool = True,
) -> ReplayMeasurement:
    """The classification tree, and nothing else — pure, so every branch is
    testable without an emulator.

    Order matters and is not arbitrary. DIVERGED comes first because a replay
    that did not replay tells you nothing about the move; NO-EXECUTE before
    WHIFF because "the move never came out" and "the move came out and
    missed" are different facts and only the second is a result about the
    move.

    `require_attack_signal=False` is for a port with no attack signal at all:
    it collapses NO-EXECUTE into WHIFF and says so in the note, rather than
    reporting a NO-EXECUTE the rig cannot actually distinguish (§7:
    "unmeasurable is a result", and mislabelling it is not).
    """
    base = dict(
        slot=obs.slot,
        arena=obs.arena,
        expected_contact=expected_contact,
        contact_frames=obs.contact_frames,
        attack_frames=obs.attack_frames,
        origin_arena=origin_arena,
        quiet_frames=quiet_frames,
        observation=obs,
    )

    if obs.input_divergence_frame is not None:
        return ReplayMeasurement(
            classification=DIVERGED,
            observed_contact=None,
            contact_delta=None,
            hits=None,
            note=(
                "the replayed input stream is not the slot's own: "
                f"{obs.input_divergence_note}. Nothing about the move was "
                "measured -- what diverged is the transcript, before the game "
                "ever got a say."
            ),
            **base,
        )

    if require_attack_signal and not obs.attack_frames:
        return ReplayMeasurement(
            classification=NO_EXECUTE,
            observed_contact=None,
            contact_delta=None,
            hits=None,
            note=(
                f"the transcript ran clean for {obs.frames} frames but the "
                "attack signal never fired: the move did not come out from "
                "this state. A slot is only valid against the state it was "
                "recorded from."
            ),
            **base,
        )

    if not obs.contact_frames:
        note = (
            "attack executed, contact signal never changed -- a WHIFF, which "
            "docs/frames.md §1.1 makes a legitimate outcome with NO advantage "
            "number. It is a result; it is not a row."
        )
        if not require_attack_signal:
            note += (
                " (No attack signal is configured on this port, so a whiff "
                "and a move that never came out are not distinguishable here.)"
            )
        return ReplayMeasurement(
            classification=WHIFF,
            observed_contact=None,
            contact_delta=None,
            hits=0,
            note=note,
            **base,
        )

    contact, hits, cluster = _cluster_first_contact(obs.contact_frames, quiet_frames)
    if contact + quiet_frames >= len(obs.contact_trace):
        raise ProbeError(
            f"replay {obs.slot!r} on {obs.arena}: contact at frame {contact} is "
            f"within {quiet_frames} frames of the end of a {obs.frames}-frame "
            "replay, so the quiet window that defines 'last contact' was never "
            "observed and `hits` may be truncated. Lengthen total_frames."
        )

    if expected_contact is None:
        # The ORIGIN run: this measurement is what DEFINES the expectation, so
        # there is nothing yet to be on time against. Labelled ON-TIME because
        # it trivially is, and the note says why rather than letting a reader
        # infer that a comparison happened.
        return ReplayMeasurement(
            classification=ON_TIME,
            observed_contact=contact,
            contact_delta=None,
            hits=hits,
            note=(
                f"origin run: contact observed at frame {contact}. No expected "
                "contact was supplied, so this run DEFINES one -- nothing was "
                "compared, and the ON-TIME label is trivial here."
            ),
            **{**base, "contact_frames": tuple(cluster)},
        )

    delta = contact - expected_contact
    if delta == 0:
        note = (
            f"contact observed at frame {contact}, the same frame the "
            "recording contacted at -- ON-TIME."
        )
    else:
        note = (
            f"contact observed at frame {contact}; the recording contacted at "
            f"{expected_contact}. The measurement is ANCHORED ON {contact} and "
            f"the {delta:+d}-frame drift is recorded. RETIMED is not an error: "
            "the replay is a valid execution that happens at a different frame "
            "from this state."
        )
    return ReplayMeasurement(
        classification=ON_TIME if delta == 0 else RETIMED,
        observed_contact=contact,
        contact_delta=delta,
        hits=hits,
        note=note,
        **{**base, "contact_frames": tuple(cluster)},
    )


def establish_origin(
    session: LabSession,
    *,
    slot: InputSlot,
    arena: str,
    total_frames: int,
    contact_read: Callable[[LabSession], Hashable],
    attack_read: Optional[Callable[[LabSession], Hashable]] = None,
    state_read: Optional[Sampler] = None,
    port: str = "p1",
    quiet_frames: int = DEFAULT_QUIET_FRAMES,
    input_offset: int = INPUT_OFFSET,
) -> ReplayOrigin:
    """Measure a slot at the state it was recorded from, to establish the
    EXPECTED contact frame every later replay is compared against.

    Refuses (`ReplayOriginError`) on anything but a clean contact. A slot that
    diverges, does not execute, or whiffs at its OWN origin has no expected
    contact frame — and defaulting one would make "expected" a fiction that
    every subsequent ON-TIME/RETIMED verdict inherits.
    """
    obs = run_replay(
        session,
        slot=slot,
        arena=arena,
        total_frames=total_frames,
        contact_read=contact_read,
        attack_read=attack_read,
        state_read=state_read,
        port=port,
        input_offset=input_offset,
    )
    m = classify_replay(
        obs,
        expected_contact=None,
        origin_arena=arena,
        quiet_frames=quiet_frames,
        require_attack_signal=attack_read is not None,
    )
    if m.observed_contact is None:
        raise ReplayOriginError(
            f"slot {slot.name!r} classified {m.classification} at its own "
            f"origin arena {arena}: {m.note} There is therefore no expected "
            "contact frame, and no later replay of this slot can be called "
            "ON-TIME or RETIMED against anything."
        )
    return ReplayOrigin(
        slot=slot.name,
        arena=arena,
        port=port,
        expected_contact=m.observed_contact,
        hits=m.hits or 1,
        input_offset=input_offset,
        contact_frames=m.contact_frames,
        state_trace=obs.state_trace,
    )


def measure_replay(
    session: LabSession,
    *,
    slot: InputSlot,
    arena: str,
    origin: ReplayOrigin,
    total_frames: int,
    contact_read: Callable[[LabSession], Hashable],
    attack_read: Optional[Callable[[LabSession], Hashable]] = None,
    state_read: Optional[Sampler] = None,
    port: Optional[str] = None,
    quiet_frames: int = DEFAULT_QUIET_FRAMES,
    ledger: "Optional[ReplayLedger]" = None,
) -> ReplayMeasurement:
    """Replay `slot` from `arena` and classify it against `origin`. One call,
    because running and classifying must not drift apart."""
    obs = run_replay(
        session,
        slot=slot,
        arena=arena,
        total_frames=total_frames,
        contact_read=contact_read,
        attack_read=attack_read,
        state_read=state_read,
        port=port or origin.port,
        input_offset=origin.input_offset,
    )
    m = classify_replay(
        obs,
        expected_contact=origin.expected_contact,
        origin_arena=origin.arena,
        quiet_frames=quiet_frames,
        require_attack_signal=attack_read is not None,
    )
    if ledger is not None:
        ledger.record(m)
    return m


# ── determinism: a SYSTEM ALARM, not a measurement ────────────────────────


@dataclass(frozen=True)
class DeterminismReport:
    """Two replays of one slot from one state, compared frame for frame.

    Kept structurally apart from `ReplayMeasurement` because the two answer
    different questions and a caller must not be able to confuse them. A
    `ReplayMeasurement` says something about a MOVE. This says something about
    the RIG: if `identical` is false, no measurement taken on this session
    means anything, including the ones that already looked fine. That is why
    the accessor is `alarm` and the escalation is `DeterminismAlarm`, not a
    `ReplayError`.

    ## `scope` is part of the answer, not decoration

    "Identical traces" is only as strong as what the trace covered, and a
    WIDER trace is not automatically a better health check. Measured live on
    MK2 arcade, one Reptile HP slot replayed from `gap-45.state`:

    | scope | alarms |
    |---|---|
    | both fighters' whole structs (`0x17A` each) | **12 / 16 pairs** |
    | the port's ACTIVE observables + the anchor  | **0 / 16 pairs** |

    Every whole-struct alarm was the same byte, `block1+0x1C` (occasionally
    with `+0xC4`), first differing at frame 3 and then for the rest of the
    run — while the contact frame agreed at 12 in every single pair. And
    `+0x1C` is one of the exact bytes `library/mk2/mk2.profile.json` already
    DISQUALIFIES from `struct_divergence`.

    ## The root cause, measured rather than guessed -- and now FIXED

    `block1+0x1C` is a frame counter, and it counted the frames that ran FREE
    inside `LabSession.load_state`'s old resume window. That window was not a
    fixed length: `load_state` used to resume (loads were believed not to
    drain while paused), then poll `frame_count` until it moved, then pause
    -- and on an uncapped headless session the core ran many frames in the
    time that round trip took. Correlating the two directly over 16 loads:

        free frames in the resume window : 14  15  17
        block1+0x1C four frames later    : 15  16  18

    i.e. `+0x1C == free_frames + 1`, exactly, with no exceptions. The rig was
    not "nondeterministic"; it started each replay from the saved state PLUS
    a variable-length free-running prefix, and any field that counts frames
    recorded the difference.

    **Fixed (task G5): `load_state(pause_after=True)`.** `LabSession.load_state`
    (and every other load path in the lab) now passes `pause_after=True`
    instead of bracketing the load with `resume`/`pause` -- the load and the
    pause happen atomically in one lock scope on the emulation thread, so
    there is no window left for a free-running core to advance anything.
    Measured after: free frames `[0]` over 16 loads, whole-struct scope 0/16
    alarms (previously 12/16 and 16/16 in two separate measurements). The
    residual hazard is the plain `pause` tool, still fire-and-forget -- a
    `pause_after` load observed picking up a stray frame when it directly
    followed an old-style plain `pause()`, which is exactly why the lab must
    never call `resume`/`pause` around a load at all, not just narrow the
    window.

    So the scope is still recorded WITH the verdict -- a caller is still
    expected to run both narrow and wide checks, because a narrow observable
    can agree by luck while the run underneath it did something else -- but
    the wide (whole-struct) scope is no longer too noisy to use on its own:
    it is clean enough now to be the DEFAULT determinism check, not just a
    second opinion run alongside a narrow one.
    """

    slot: str
    arena: str
    frames: int
    identical: bool
    first_divergence_frame: Optional[int]
    scope: str = "state_read"
    divergence_frames: Tuple[int, ...] = ()
    trace_a: Tuple[Hashable, ...] = field(repr=False, default=())
    trace_b: Tuple[Hashable, ...] = field(repr=False, default=())
    contact_a: Tuple[int, ...] = ()
    contact_b: Tuple[int, ...] = ()

    @property
    def alarm(self) -> bool:
        return not self.identical

    def raise_if_alarm(self) -> "DeterminismReport":
        if self.alarm:
            raise DeterminismAlarm(str(self))
        return self

    def __str__(self) -> str:
        if self.identical:
            return (
                f"determinism OK [{self.scope}]: {self.slot!r} replayed twice "
                f"from {self.arena} produced identical {self.frames}-frame "
                f"traces (contact {list(self.contact_a)})."
            )
        f = self.first_divergence_frame
        return (
            f"SYSTEM ALARM [{self.scope}] -- replay determinism FAILED: "
            f"{self.slot!r} replayed twice from {self.arena} first diverged at "
            f"frame {f}, and at frames {list(self.divergence_frames)}; contact "
            f"frames {list(self.contact_a)} vs {list(self.contact_b)}. Not a "
            "measurement result -- every measurement taken on this session is "
            "suspect, including ones that looked clean. Do not record rows "
            "from this session (docs/frames.md §7: a number that fails "
            "re-measurement is DELETED, not averaged)."
        )


def determinism_check(
    session: LabSession,
    *,
    slot: InputSlot,
    arena: str,
    total_frames: int,
    state_read: Sampler,
    contact_read: Optional[Callable[[LabSession], Hashable]] = None,
    port: str = "p1",
    input_offset: int = INPUT_OFFSET,
    scope: str = "state_read",
) -> DeterminismReport:
    """Deliverable 1: replay ONE slot TWICE from ONE state and require the
    traces to be identical.

    This is the ONE place a state-trace comparison is valid (see the module
    docstring): same slot, same starting state, so anything that differs is
    the rig, not the game.

    `state_read` decides what the verdict is ABOUT, and `scope` names it in
    the report. Run it twice with two scopes: once as wide as is cheap (both
    fighters' whole structs), because a narrow observable can agree by luck
    while the run underneath it did something else; and once over exactly the
    fields the measurement reads, because a wide trace can alarm on a field
    the profile already disqualified. On MK2 arcade those two give different
    verdicts — see `DeterminismReport`.
    """
    reads = contact_read or (lambda s: 0)
    runs = []
    for _ in range(2):
        runs.append(
            run_replay(
                session,
                slot=slot,
                arena=arena,
                total_frames=total_frames,
                contact_read=reads,
                state_read=state_read,
                port=port,
                input_offset=input_offset,
            )
        )
    a, b = runs[0].state_trace, runs[1].state_trace
    diverged = tuple(i for i in range(min(len(a), len(b))) if a[i] != b[i])
    if len(a) != len(b):
        # Different trace LENGTHS is itself a divergence, and pointing at the
        # first missing frame is more useful than reporting "identical prefix".
        diverged = diverged or (min(len(a), len(b)),)
    return DeterminismReport(
        slot=slot.name,
        arena=arena,
        frames=total_frames,
        identical=not diverged,
        first_divergence_frame=diverged[0] if diverged else None,
        scope=scope,
        divergence_frames=diverged,
        trace_a=a,
        trace_b=b,
        contact_a=runs[0].contact_frames,
        contact_b=runs[1].contact_frames,
    )


# ── the ledger: what the run refused, counted ─────────────────────────────


class ReplayLedger:
    """Every classification a run produced, counted — including the ones that
    produced no row.

    §7's "no silent caps" applied to replay sourcing: a run that quietly drops
    a DIVERGED replay and reports only its ON-TIME ones looks better than it
    is. `render()` prints every bucket, always, including the zeros.
    """

    def __init__(self) -> None:
        self.counts: Dict[str, int] = {c: 0 for c in CLASSIFICATIONS}
        self.measurements: List[ReplayMeasurement] = []
        # A LIST, because one session can be health-checked at more than one
        # scope and the scopes can disagree (see `DeterminismReport`). Keeping
        # only the last would let a narrow all-clear overwrite a wide alarm.
        self.determinism_checks: List[DeterminismReport] = []

    def record(self, m: ReplayMeasurement) -> ReplayMeasurement:
        self.counts[m.classification] = self.counts.get(m.classification, 0) + 1
        self.measurements.append(m)
        return m

    def note_determinism(self, report: DeterminismReport) -> DeterminismReport:
        self.determinism_checks.append(report)
        return report

    @property
    def rows(self) -> List[ReplayMeasurement]:
        return [m for m in self.measurements if m.produces_row]

    @property
    def refusals(self) -> List[ReplayMeasurement]:
        return [m for m in self.measurements if m.is_refusal]

    @property
    def retimed(self) -> List[ReplayMeasurement]:
        return [m for m in self.measurements if m.classification == RETIMED]

    @property
    def suspect(self) -> bool:
        """True when the session failed its determinism health check. A caller
        that stores rows must consult this FIRST: a clean classification on a
        nondeterministic rig is not evidence of anything."""
        return any(d.alarm for d in self.determinism_checks)

    def summary(self) -> Dict[str, Any]:
        return {
            "counts": dict(self.counts),
            "rows": len(self.rows),
            "refusals": len(self.refusals),
            "determinism": {d.scope: d.identical for d in self.determinism_checks},
            "suspect": self.suspect,
        }

    def render(self) -> str:
        lines = [str(d) for d in self.determinism_checks]
        for c in CLASSIFICATIONS:
            n = self.counts.get(c, 0)
            tag = (
                "rows" if c in ROW_CLASSIFICATIONS
                else "no row, counted" if c in REFUSAL_CLASSIFICATIONS
                else "result, no row"
            )
            lines.append(f"  {c:<11} {n:>3}   ({tag})")
        for m in self.retimed:
            lines.append(
                f"  RETIMED   {m.slot} @ {m.arena}: contact {m.observed_contact} "
                f"vs expected {m.expected_contact} ({m.contact_delta:+d})"
            )
        if self.suspect:
            lines.append(
                "  !! every row above is SUSPECT: the session failed its "
                "determinism health check."
            )
        return "\n".join(lines)
