"""The frame lab's transport layer: a session that owns the MCP primitives
`docs/frames.md` §3 makes preconditions, and enforces them.

Everything in `framelab` that touches a running emulator goes through this
module. It exists because §3's preconditions are not advice — each one is a
measured failure that produced a confident wrong number:

  * `step` used to be FIRE-AND-FORGET. 30 rapid `step` calls were measured
    landing **1** frame (`library/mk2/mk2.md`, "Toolkit friction"). A protocol
    that did not confirm each step counted frames that never happened, and the
    resulting "the held input did nothing" false negative is indistinguishable
    from a real result — it briefly convinced one agent that held input cannot
    reach the core while stepping. It can: +72 and +63 units over 30
    CONFIRMED frames, control 0.
  * `load_state` used to require resuming around it (loads did not drain
    while paused, same GUI-frame mechanism), which let an uncapped core run a
    VARIABLE number of free frames inside the resume window (docs/frames.md
    §4.6: 10-15 frames over 16 loads on the old three-round-trip
    `resume → load → poll → pause` protocol). `load_state(pause_after=True)`
    (task G5) closed this: the load and the pause happen atomically in one
    lock scope on the emulation thread, so `LabSession.load_state` never
    calls `resume`/`pause` at all — measured after: `[0]` free frames over 16
    loads, 0/16 whole-struct determinism alarms (previously 12/16 and 16/16
    in two separate measurements). The residual hazard is the plain `pause`
    tool, which stays fire-and-forget (sets a flag without confirming the
    in-flight frame finished) — a `pause_after` load observed picking up a
    stray frame when it directly followed one. The lab's protection is
    never calling `resume`/`pause` around a load at all, not fixing `pause`.
  * `press_buttons` is BANNED (§3.3): its countdown decrements on every GUI
    frame including while paused, so a chord can evaporate between the press
    and the step. The ban is enforced by construction here — `LabSession.call`
    raises `PreconditionError` on any attempt to call it, so no code that
    routes through a session can reintroduce it.

## The transport changed under this module (task F1), and it changed the shape

`step` is now SYNCHRONOUS: `src/mcp/server.rs` bumps `step_generation` at the
very END of `Frontend::run_frame` (after all post-processing) and the tool
blocks on that, so the call returns only when the emulated frame is entirely
FINISHED. Two things follow, and both are load-bearing here:

  1. **The poll loop is gone.** `confirm_step` no longer reads `get_state`'s
     `frame_count` in a loop; it reads the `landed`/`frame_count` the `step`
     response itself carries, and REFUSES a response that does not say it
     landed. §3.5's requirement ("every step confirmed to have LANDED") is
     unchanged — the confirmation moved from the client to the server, where
     it can be exact instead of inferred from a counter that moved.
  2. **The settle is gone by default** (`input_settle_s=0`), and something
     exact replaced it. §3.6's 8 ms settle existed because a confirmed
     `frame_count` was NOT proof the frame had finished, so a `hold_buttons`
     issued right after could be read by the frame that was supposed to be
     over. `step` closes that particular gap at the source. But the flake
     §3.6 describes did NOT go away with it, because the settle was never its
     mechanism — see `confirm_fold`, which is what actually closes it, and
     which measured the settle making things slightly WORSE rather than
     better. `input_settle_s` stays a parameter so it can be reinstated
     without a code change if some future symptom wants it.

`run_frames(count, port0?, port1?)` advances up to 600 frames in ONE call with
optional per-port held masks (same replace-not-OR semantics as
`hold_buttons`), requiring the emulator to be paused. It is not a shortcut
around §3.5: the response reports `landed` and `all_landed`, and
`LabSession.run_frames` raises unless every requested frame landed. Frames
that nothing observes (a replay's whole pre-window prefix) cost one round trip
for the segment instead of one per frame. **Its per-port masks are not used
to change input** — that is the race `confirm_fold` documents.

## `get_input` grew a third reading (task G1), and it changed only ONE thing

`get_input` now reports `executed_mask`/`executed_buttons` beside
`asserted_*` and `folded_*`: what the last frame that actually ran `core.run()`
saw, written atomically with the decision to run and therefore STICKY.
`folded_*` is re-folded on every host-loop tick whether or not that tick ran a
frame, so while paused it drifts back to agreeing with the held set — which
makes it useless for asking, afterwards, what a specific landed frame saw.

The fold ORACLE (`confirm_fold`) did NOT change: it runs before any frame
exists, and `executed_*` cannot report a frame that has not happened yet, so
`asserted == folded` remains the only evidence available at that moment (and
the one with a 0/400 record). What is new is the after-the-fact check —
`confirm_executed` / `LabSession.verify_executed` — which `folded_*`
structurally could not provide. See `confirm_fold`'s docstring for the full
argument.

Wall-clock note (§2.4): waiting for a step or a batch to land is transport
bookkeeping, not measurement. Nothing in this module expresses a DURATION in
wall-clock; frames are the only unit that leaves it.

This module talks to a `client` that needs exactly one method,
`client.call(tool, **kwargs) -> dict` — `McpClient.call`'s signature. That
keeps every protocol above it testable against a bare fake.
"""

from __future__ import annotations

import time
from dataclasses import dataclass
from typing import Any, Callable, Dict, Iterable, Optional, Sequence, Tuple, Union

__all__ = [
    "LabError",
    "PreconditionError",
    "ExecutedInputError",
    "LabSession",
    "Preconditions",
    "BANNED_TOOLS",
    "MAX_RUN_FRAMES",
    "call_ok",
    "confirm_executed",
    "confirm_fold",
    "confirm_step",
    "executed_input",
    "frame_count",
    "hold_buttons",
    "release_all",
    "run_frames",
    "set_training_enforcement",
]

# §3.3. Enforced by construction, not by convention.
BANNED_TOOLS = frozenset({"press_buttons"})

# `src/mcp/server.rs::MAX_RUN_FRAMES` — a single `run_frames` call may not ask
# for more than this (~10 s at 60 fps). Batches longer than one segment are
# split here rather than refused, so callers never have to know the cap.
MAX_RUN_FRAMES = 600

# `confirm_fold`: how long to wait for an asserted held set to reach the
# core's input fold, and how often to ask. Both are transport bookkeeping
# against a real oracle (`get_input`'s `folded` vs `asserted`), not a settle.
_FOLD_POLL_S = 0.0005
_FOLD_TIMEOUT_S = 2.0

# Settle before CHANGING a port's held set. DEFAULT 0 since `step` became
# synchronous: the old 8 ms existed because a moved `frame_count` was not
# proof the emulated frame had FINISHED, so an input change issued right after
# the confirmation could still be read by the frame that was supposed to be
# over (measured: 1 flake in 14 identical runs with no settle, 0 in 14 with
# 8 ms). `step` now returns only after `Frontend::run_frame` has completed all
# of its post-processing, which removes the window the flake lived in.
# Kept as a parameter, not deleted: if the flake ever reappears, reinstating
# the settle is a constructor argument rather than a code change. This is
# transport bookkeeping, not a measured duration (§2.4).
_INPUT_SETTLE_S = 0.0


class LabError(RuntimeError):
    """A frame-lab operation failed in a way that voids the measurement."""


class PreconditionError(LabError):
    """A `docs/frames.md` §3 precondition is not satisfied. Never downgraded
    to a warning: "a measurement run that skips any of these is void.\""""


class ExecutedInputError(LabError):
    """The frame that ACTUALLY RAN did not see the input we asserted.

    Distinct from `PreconditionError` because it is not a setup mistake: it
    is the §3.6 failure caught after the fact rather than before it, and the
    frames already run under the wrong input cannot be un-run. Whatever they
    measured is void."""


# ── free functions: the raw MCP primitives ────────────────────────────────
#
# `calibrate.py` predates `LabSession` and drives these directly with its own
# `error_cls`, so the primitives stay free functions with an injectable error
# type rather than methods. One implementation of "confirm the step landed",
# used by both protocols.


def call_ok(client: Any, tool: str, *, error_cls: type = LabError, **kwargs: Any) -> dict:
    """Call one MCP tool and raise `error_cls` unless it clearly succeeded.

    Two failure shapes exist server-side: most tools answer
    `{"ok": false, "error": ...}`, but the write gate short-circuits
    `load_state`/`load_shadow` to a bare `{"error": ...}` with no `"ok"` key
    at all. Treating an ABSENT `"ok"` as success would swallow exactly the
    "you forgot enable_writes" case, so both shapes are failures here.
    """
    if tool in BANNED_TOOLS:
        raise PreconditionError(
            f"{tool!r} is BANNED in the frame lab (docs/frames.md §3.3): its "
            "countdown decrements on every GUI frame including while paused, "
            "so a chord can evaporate between the press and the step. Use "
            "hold_buttons/release_buttons."
        )
    r = client.call(tool, **kwargs)
    failed = isinstance(r, dict) and (
        r.get("ok") is False or ("error" in r and "ok" not in r)
    )
    if failed:
        raise error_cls(f"{tool} failed: {r.get('error', r)}")
    return r


def frame_count(client: Any, *, error_cls: type = LabError) -> Optional[int]:
    return call_ok(client, "get_state", error_cls=error_cls).get("frame_count")


def confirm_step(
    client: Any, *, error_cls: type = LabError, before: Optional[int] = None
) -> Optional[int]:
    """Advance exactly one core frame and CONFIRM it landed. Returns the new
    `frame_count` (or None if the server does not report one).

    §3.5's confirmation, now read off the `step` response instead of polled:
    `step` is synchronous (`src/mcp/server.rs`), so it returns `landed: true`
    only after the emulated frame is entirely finished. A response that does
    NOT say it landed is a hard failure here — never a retry and never a
    "probably fine". That is the same rule as before; only the mechanism moved.

    `before` is accepted for call compatibility and is ignored: the server's
    own answer supersedes any client-side notion of what the frame count was.
    """
    r = client.call("step")
    if not isinstance(r, dict):
        raise error_cls(f"step() returned {r!r}, not a response object")
    failed = r.get("ok") is False or ("error" in r and "ok" not in r)
    if failed or not r.get("landed", False):
        raise error_cls(
            f"step() did not land: {r.get('error', r)}. The session may be "
            "unresponsive, the emulator may not actually be paused (a held "
            "pause is what makes `step` mean exactly one frame), or this "
            "server predates the synchronous `step` that reports `landed`. "
            "docs/frames.md §3.5 -- a step that is not confirmed is a frame "
            "that never happened."
        )
    return r.get("frame_count")


def run_frames(
    client: Any,
    count: int,
    *,
    port0: Optional[Sequence[str]] = None,
    port1: Optional[Sequence[str]] = None,
    error_cls: type = LabError,
) -> Optional[int]:
    """Advance `count` core frames in ONE call, holding `port0`/`port1` for
    all of them, and confirm that EVERY requested frame landed.

    This is `step` batched, not `step` weakened. The server reports `landed`
    and `all_landed`; anything short of all of them raises, because a batch
    that ran 59 of 60 frames is exactly the "counts frames that never
    happened" failure §3.5 exists to prevent — just wholesale.

    `port0`/`port1` REPLACE that port's held set (identical semantics to
    `hold_buttons`; `[]` releases everything on that port) and are applied
    before the first frame runs. A port not named keeps whatever it held.

    **Do not use the masks to CHANGE a port's input.** They are set under the
    same lock acquisition that arms the batch, which leaves no window for the
    host loop's input fold — so the batch's first frame can run on the
    previous input. Measured at 13 wrong answers in 200 (see `confirm_fold`).
    Change input with `hold_buttons`/`release_buttons` + `confirm_fold`
    (which is what `LabSession.run_frames` does) and use these masks only to
    RE-assert an unchanged set.

    Counts above `MAX_RUN_FRAMES` are split into consecutive calls. Only the
    FIRST carries the masks — the rest inherit them, which is what a "held for
    the whole segment" mask means.
    """
    if count < 1:
        raise ValueError("`count` must be >= 1")
    last: Optional[int] = None
    remaining = int(count)
    first = True
    while remaining > 0:
        chunk = min(remaining, MAX_RUN_FRAMES)
        kwargs: dict = {"count": chunk}
        if first and port0 is not None:
            kwargs["port0"] = list(port0)
        if first and port1 is not None:
            kwargs["port1"] = list(port1)
        r = client.call("run_frames", **kwargs)
        if not isinstance(r, dict):
            raise error_cls(f"run_frames returned {r!r}, not a response object")
        landed = r.get("landed")
        short = r.get("all_landed") is False or (
            landed is not None and int(landed) != chunk
        )
        if short or r.get("ok") is False or ("error" in r and "ok" not in r):
            raise error_cls(
                f"run_frames({chunk}) landed {landed} of {chunk} frames: "
                f"{r.get('error', r)}. docs/frames.md §3.5 -- a partially "
                "landed batch counts frames that never happened, wholesale."
            )
        last = r.get("end_frame", last)
        remaining -= chunk
        first = False
    return last


def hold_buttons(
    client: Any, buttons: Sequence[str], port: int, *, error_cls: type = LabError
) -> None:
    """Assert `buttons` as port `port`'s ENTIRE held set. `hold_buttons`
    REPLACES rather than ORs (src/mcp/server.rs
    `hold_buttons_asserts_until_release_and_replaces_not_ors`), so one call
    fully describes the port's input for the frames that follow."""
    call_ok(client, "hold_buttons", buttons=list(buttons), port=port, error_cls=error_cls)


def release_all(client: Any, port: int, *, error_cls: type = LabError) -> None:
    """Clear port `port`'s whole held set (`release_buttons` with an empty
    list means "everything" — server.rs)."""
    call_ok(client, "release_buttons", buttons=[], port=port, error_cls=error_cls)


def confirm_fold(
    client: Any,
    port: int,
    *,
    error_cls: type = LabError,
    timeout_s: float = _FOLD_TIMEOUT_S,
    poll_s: float = _FOLD_POLL_S,
) -> None:
    """Block until `port`'s asserted held set has actually been FOLDED into
    the input the core will read. §3.6, made exact.

    ## Why this is not paranoia — it is a measured one-frame error

    The host loop folds input and then runs a frame, in that order and in
    separate lock acquisitions (`src/main.rs` step (a0), then
    `Frontend::run_frame`). So "the held set is written" and "the core will
    see it on the next frame" are two different facts, and there is a window
    between them: if a frame is armed after the loop has already passed its
    fold for that iteration, that frame runs on the PREVIOUS input.

    Measured on `gap-45.state`, Reptile far HP, 200 identical probe/control
    pairs per configuration:

    | how the hold was asserted | spurious TRUE at N=0 |
    |---|---|
    | `run_frames(port0=...)` masks | **13 / 200** |
    | `hold_buttons` then `run_frames` | 0 / 200 |
    | `hold_buttons` then per-frame `step` | 0 / 200 |

    `run_frames`' per-port masks set the held input and arm the batch in ONE
    lock acquisition, which leaves NO window for the fold — so the batch's
    first frame is the one that runs on stale input. The failure is not
    subtle once you look at the right byte: in every flake the ATTACKER's
    move came out one frame late (the victim's health register was still
    161 on the frame it is normally already 150), which leaves the defender
    genuinely free on the frame the probe holds at, and the defender
    genuinely walks. Both observables agree, the predicate is plausible, and
    the number is wrong — the exact signature §3.6 describes, with a
    different cause than §3.6 assumed.

    A separate `hold_buttons` call is empirically enough (a round trip is far
    longer than a loop iteration), but "empirically enough" is what the 8 ms
    settle was. This polls the ORACLE instead: `get_input` reports `folded`
    (what the last fold gave the core) alongside `asserted` (what the next
    one will), so waiting for them to agree is a real confirmation, not a
    duration. §2.4 permits it for the same reason it permits confirming a
    load landed — it is transport bookkeeping, and no wall-clock leaves it.

    ## Why `executed_*` did NOT replace the predicate here (task G1)

    `get_input` now also reports `executed_mask`/`executed_buttons`: what the
    last frame that actually ran `core.run()` saw, updated atomically with the
    decision to run and therefore STICKY. `folded_*` by contrast is refreshed
    on every host-loop tick whether or not that tick's frame ran, so it drifts
    back to matching the held set while paused.

    That makes `executed_*` strictly better for reading BACKWARDS — see
    `confirm_executed` — and strictly UNUSABLE for the wait above, which runs
    FORWARDS: this function is called while paused, before any frame runs, and
    `executed_*` cannot report a frame that has not happened yet. Waiting for
    `asserted == executed` here would block until the timeout on every single
    input change in a paused session. `asserted == folded` is the only
    evidence that exists before the frame, so the predicate is unchanged, and
    the measured 0/400 record it earned stands. The addition is a second,
    AFTER-the-fact check (`confirm_executed`), which `folded_*` structurally
    could not provide: a post-`step` read of `folded_*` shows the current held
    set, not what the frame that ran saw, so it agrees with itself even when
    the frame ran on stale input.
    """
    deadline = time.monotonic() + timeout_s
    while True:
        r = call_ok(client, "get_input", error_cls=error_cls, port=port)
        asserted, folded = r.get("asserted_mask"), r.get("folded_mask")
        if asserted is None or folded is None:
            raise error_cls(
                f"get_input(port={port}) returned no asserted/folded masks "
                f"({r!r}) -- without them there is no way to confirm the held "
                "set reached the core, and a frame run on stale input is a "
                "silently wrong measurement."
            )
        if asserted == folded:
            return
        if time.monotonic() >= deadline:
            raise error_cls(
                f"port {port}'s held set ({asserted}) never reached the "
                f"core's input fold (still {folded}) within {timeout_s}s. The "
                "host loop may not be running frames at all."
            )
        time.sleep(poll_s)


def executed_input(
    client: Any, port: int, *, error_cls: type = LabError
) -> Optional[frozenset]:
    """What the LAST FRAME THAT ACTUALLY RAN saw on `port`, as a frozenset of
    button names — or `None` if this server does not report it.

    `None` is "this build predates `executed_*`", NOT "the port held nothing":
    an empty held set is `frozenset()`, and conflating the two would turn a
    missing instrument into a measurement (§2.5, "absent means absent").

    Read from `get_input`'s `executed_buttons`, which `src/mcp/server.rs`
    updates only on frames that ran `core.run()`, atomically with the decision
    to run. That is what makes it safe to read AFTER a `step`/`run_frames`
    without racing a later host-loop tick — the property `folded_buttons`
    does not have, because it is re-folded every tick whether a frame ran or
    not and therefore drifts back to agreeing with the held set.
    """
    r = call_ok(client, "get_input", error_cls=error_cls, port=port)
    names = r.get("executed_buttons")
    if names is None:
        return None
    return frozenset(str(n).lower() for n in names)


def confirm_executed(
    client: Any,
    port: int,
    expected: Sequence[str],
    *,
    error_cls: type = ExecutedInputError,
    where: str = "",
) -> Optional[frozenset]:
    """Assert that the frame that actually ran saw exactly `expected` on
    `port`. Returns the executed set, or `None` when the server does not
    report one (older build — the check is SKIPPED, and the caller is told so
    by the `None` rather than being told it passed).

    This is the after-the-fact half of §3.6. `confirm_fold` proves the input
    reached the fold BEFORE the frame; this proves the frame that ran saw it.
    The two are not redundant: the fold oracle cannot see a frame that runs
    later on a re-fold, and `folded_*` read afterwards cannot see it either
    (it is overwritten every host tick). Only `executed_*` is sticky enough
    to answer "what did THAT frame see".
    """
    got = executed_input(client, port, error_cls=error_cls)
    if got is None:
        return None
    want = frozenset(str(b).lower() for b in expected)
    if got != want:
        raise error_cls(
            f"port {port}: the frame that actually ran saw {sorted(got)}, not "
            f"the asserted {sorted(want)}"
            + (f" ({where})" if where else "")
            + ". docs/frames.md §3.6 -- a frame run on the wrong input is a "
            "silently wrong measurement, and it has already run."
        )
    return got


def set_training_enforcement(
    client: Any, enabled: bool, *, error_cls: type = LabError
) -> None:
    """§3.1. Not because the dummy stomps the probe, but because the health
    refill rewrites `0xBCA0`/`0xBC88` — simultaneously the contact anchor and
    the damage reading. Refill and anchor are the same bytes."""
    flag = "true" if enabled else "false"
    call_ok(client, "run_lua", script=f"training.set_enabled({flag})", error_cls=error_cls)


# ── the session ───────────────────────────────────────────────────────────

VerifyFn = Callable[["LabSession"], bool]


@dataclass
class Preconditions:
    """What `LabSession.enforce_preconditions` actually established, recorded
    so a stored row can say so rather than assume it."""

    training_enforcement: str      # "off" (verified) — anything else raises
    shadow_runner: str             # "off" | "no-model" (both fine)
    writes_armed: bool
    arena_verified: bool


class LabSession:
    """A frame-lab session over one MCP client.

    Owns: the write gate, the §3 preconditions, confirmed `step`, confirmed
    `load_state`, and the held-input surface. Owns NOTHING game-specific —
    the arena verifier and every observable are injected (CLAUDE.md: "never
    hardcode a game address in code again").

        session = LabSession(client, verify_fn=mk2_arena_verifier(profile, ...))
        session.enforce_preconditions()
        session.load_state("shadow/arenas/mk2/r-v-r.state")
        session.set_held(0, ["right"]); session.step()
    """

    def __init__(
        self,
        client: Any,
        *,
        verify_fn: Optional[VerifyFn] = None,
        ports: Sequence[int] = (0, 1),
        input_settle_s: float = _INPUT_SETTLE_S,
        confirm_folds: bool = True,
        verify_executed: bool = False,
        error_cls: type = LabError,
    ):
        self.client = client
        self.verify_fn = verify_fn
        self.ports = tuple(ports)
        self.input_settle_s = input_settle_s
        self.confirm_folds = confirm_folds
        # OFF by default, deliberately. It costs one `get_input` round trip
        # per port per advance (against ~0.72 ms/frame batched, that is not
        # free at ~200 replays per cell), and it is WRONG for any port this
        # session does not itself drive -- a `play_inputs` playback replaces
        # the held set from inside the frame, so the session's notion of "what
        # I asserted" is not what that port executed, by design. Turn it on
        # for a run whose whole point is transport integrity.
        self.verify_executed = verify_executed
        self.error_cls = error_cls
        self._asserted: Dict[int, Tuple[str, ...]] = {}
        self._frame: Optional[int] = None
        self.preconditions: Optional[Preconditions] = None
        # Counters, purely for the report ("what did this run actually do").
        # `steps_taken` counts core FRAMES advanced however they were
        # advanced, so it stays comparable across the transport change (it
        # used to be one call per frame by construction); `step_calls` and
        # `batch_calls` say how many round trips those frames cost.
        self.steps_taken = 0
        self.loads_done = 0
        self.step_calls = 0
        self.batch_calls = 0
        self.frames_batched = 0

    # ── raw ──────────────────────────────────────────────────────────────
    def call(self, tool: str, **kwargs: Any) -> dict:
        return call_ok(self.client, tool, error_cls=self.error_cls, **kwargs)

    def run_lua(self, script: str) -> str:
        return str(self.call("run_lua", script=script).get("output", ""))

    def read_memory(self, addr: int, length: int) -> bytes:
        r = self.call("read_memory", addr=addr, len=length)
        if "hex" not in r:
            raise self.error_cls(f"read_memory(0x{addr:X}, {length}) returned {r!r}")
        return bytes.fromhex(r["hex"].replace(" ", ""))

    # ── preconditions (§3) ───────────────────────────────────────────────
    def enforce_preconditions(self) -> Preconditions:
        """§3.1 (training enforcement OFF) and §3.2 (shadow runner off),
        both SET and then READ BACK — a write whose effect is never verified
        is not enforcement. Also arms the write gate `load_state` needs.

        Raises `PreconditionError` on any of them. §3.3 (`press_buttons`
        banned) is enforced by construction in `call_ok`; §3.4/§3.5/§3.6 are
        enforced per-operation in `load_state`/`step`; §3.7 (calibration
        current) belongs to the caller, which must pass the
        `input_latency_frames` it measured with `calibrate`.
        """
        # The write gate first: training.set_enabled is itself write-gated.
        self.call("enable_writes")

        set_training_enforcement(self.client, False, error_cls=self.error_cls)
        state = self.run_lua("training.enabled()").strip().lower()
        if state != "false":
            raise PreconditionError(
                "docs/frames.md §3.1: training enforcement must be OFF (its "
                "health refill rewrites the very bytes that are the contact "
                f"anchor), but training.enabled() reads {state!r} after "
                "set_enabled(false). Refusing to measure."
            )

        # `shadow.on()` -> bool | nil; nil ("ok" through eval_to_string) means
        # no model is loaded at all, which is the strongest form of "off".
        shadow = self.run_lua("shadow.on()").strip().lower()
        if shadow == "true":
            raise PreconditionError(
                "docs/frames.md §3.2: a shadow model is currently driving a "
                "port (shadow.on() == true). Any model driving a port "
                "invalidates the run. Toggle it off (Shift+F5 / shadow."
                "toggle()) and re-run."
            )
        shadow_state = "no-model" if shadow in ("ok", "nil", "") else "off"

        self.preconditions = Preconditions(
            training_enforcement="off",
            shadow_runner=shadow_state,
            writes_armed=True,
            arena_verified=False,
        )
        return self.preconditions

    # ── state ────────────────────────────────────────────────────────────
    def pause(self) -> None:
        self.call("pause")
        self._frame = None

    def resume(self) -> None:
        self.call("resume")
        self._frame = None

    def frame(self) -> Optional[int]:
        self._frame = frame_count(self.client, error_cls=self.error_cls)
        return self._frame

    def load_state(self, spec: Union[str, int]) -> None:
        """§3.4 + §4.6, in one atomic operation: load with `pause_after=True`
        so the load and the pause happen in the SAME lock scope on the
        emulation thread (`src/mcp/server.rs::state_op_roundtrip`), then
        confirm it LANDED and run the injected arena verifier.

        This never calls `resume`/`pause` around the load. That bracket is
        the defect §4.6 measured: an uncapped core ran a VARIABLE number of
        free frames (10-15 over 16 loads) inside the old `resume → load →
        poll → pause` window, so every "identical" replay actually started
        from the saved state plus a variable-length prefix. `pause_after`
        removes the window instead of narrowing it — there is nothing to
        poll for here, because the response's own `"paused"` field (read in
        the same lock acquisition as the load result, server-side) already
        confirms the atomic pause landed. The residual hazard is calling the
        plain `pause` tool anywhere near a load (it is fire-and-forget and
        can leave one stray frame) — so this method, and every other load
        path in the lab, must never do that either.

        Raises `PreconditionError` if the verifier says the arena is not the
        live situation it is supposed to be — §3.4 requires this after EVERY
        load, not once at capture: "the same object-pool instability that
        breaks `x` can invalidate it later."
        """
        # Every port released BEFORE the load so no stale hold bleeds across
        # runs (a held direction surviving into the next replay is a silent
        # cross-contamination between probe and control).
        for port in self.ports:
            release_all(self.client, port, error_cls=self.error_cls)
            self._asserted[port] = ()

        try:
            slot = int(spec)
            r = self.call("load_state", slot=slot, pause_after=True)
        except (TypeError, ValueError):
            r = self.call("load_state", path=str(spec), pause_after=True)
        self.loads_done += 1
        self._frame = None

        # `call_ok` already raised if `r` was `{"ok": false, ...}` or a bare
        # `{"error": ...}` (write gate). What is left to check is the atomic
        # guarantee itself: a successful `pause_after=True` load forces
        # `paused: true` in that same lock scope, so anything else here means
        # the guarantee did not hold and this is a §4.6 SESSION ALARM, not a
        # retryable hiccup.
        if not r.get("paused"):
            raise self.error_cls(
                f"load_state({spec!r}, pause_after=True) reported ok but not "
                f"paused=true (got {r!r}) -- docs/frames.md §4.6's atomic "
                "load-and-pause guarantee did not hold. Treat this as a "
                "session alarm: nothing measured after it is trustworthy."
            )

        if self.verify_fn is not None and not self.verify_fn(self):
            raise PreconditionError(
                f"arena {spec!r} failed its liveness/identity check "
                "immediately after load_state -- docs/frames.md §3.4. "
                "Nothing measured from here would be trustworthy."
            )
        if self.preconditions is not None:
            self.preconditions.arena_verified = True

    # ── input ────────────────────────────────────────────────────────────
    def set_held(self, port: int, buttons: Iterable[str]) -> None:
        """Assert `buttons` as `port`'s entire held set, from the next
        emulated frame on.

        Settles first IF `input_settle_s` is non-zero — and it is 0 by
        default now. The settle was load-bearing while `step` only proved
        `frame_count` had MOVED: if the count was bumped before the frame's
        input fold, an input change issued the instant the confirmation
        arrived could still be read by the frame we believed was over. That is
        a one-frame-early hold, and it is exactly the shape of the flake this
        lab hit live — a single spurious divergence at an N where the fighter
        is demonstrably still stunned, roughly 1 run in 50, never reproducing
        on re-run. Measured A/B on `r-v-r.state`, 14 identical probe/control
        pairs each: 1 flake with no settle, 0 with 8 ms.

        `step` is now synchronous and returns only once the frame is entirely
        finished, which closes that particular window at the source rather
        than waiting it out — the second half of §3.6's own "settle, or make
        `step` synchronous". It did not close the flake, though: measured
        head to head, the 8 ms settle made the spurious-TRUE rate slightly
        WORSE (16/100 with, 7/100 without, same rig, same session). So the
        settle is off and `input_settle_s=8e-3` is only a way to put it back
        without touching code.

        What does the work instead is `confirm_fold`: the change is not
        "issued" until the core's own input fold reports it. That is the
        exact version of what the settle was approximating, and the numbers
        that motivated it are in `confirm_fold`'s docstring.
        """
        if self.input_settle_s:
            time.sleep(self.input_settle_s)
        buttons = list(buttons)
        if buttons:
            hold_buttons(self.client, buttons, port, error_cls=self.error_cls)
        else:
            release_all(self.client, port, error_cls=self.error_cls)
        self._asserted[port] = tuple(buttons)
        if self.confirm_folds:
            confirm_fold(self.client, port, error_cls=self.error_cls)

    def release(self, port: int) -> None:
        release_all(self.client, port, error_cls=self.error_cls)
        self._asserted[port] = ()
        if self.confirm_folds:
            confirm_fold(self.client, port, error_cls=self.error_cls)

    def release_all_ports(self) -> None:
        for port in self.ports:
            release_all(self.client, port, error_cls=self.error_cls)
            self._asserted[port] = ()

    # ── what the frame that RAN actually saw (task G1) ───────────────────
    def executed(self, port: int) -> Optional[frozenset]:
        """`executed_input` for `port` — the sticky record of the last frame
        that really ran. `None` when the server does not report it."""
        return executed_input(self.client, port, error_cls=self.error_cls)

    def _verify_executed(self, where: str) -> None:
        """Post-frame half of §3.6, opt-in via `verify_executed`. Checks only
        ports this session has actually asserted something on: a port driven
        by something else (a `play_inputs` playback) legitimately executes an
        input this session never asserted, and flagging that would be a false
        alarm, not a finding."""
        if not self.verify_executed:
            return
        for port, buttons in self._asserted.items():
            confirm_executed(
                self.client, port, buttons, error_cls=ExecutedInputError, where=where
            )

    # ── stepping ─────────────────────────────────────────────────────────
    def step(self) -> None:
        self._frame = confirm_step(self.client, error_cls=self.error_cls)
        self.steps_taken += 1
        self.step_calls += 1
        self._verify_executed("after step")

    def run_frames(
        self, count: int, holds: "Optional[dict[int, Iterable[str]]]" = None
    ) -> None:
        """Advance `count` frames in one call, optionally asserting `holds`
        (port -> its entire held set) for the whole segment first.

        Use this for any run of frames NOTHING observes — the prefix of a
        replay before the sampled window is most of the lab's frame budget,
        and paying a round trip per frame for it buys nothing. A single frame
        still goes through `step`: the batch tool has a strictly larger
        contract (it requires the emulator paused) for no gain at count 1.

        **`holds` goes through `set_held`, NOT through the `run_frames`
        tool's own `port0`/`port1` masks, and that is deliberate.** The tool's
        masks set the held input and arm the batch under ONE lock
        acquisition, which leaves no window for the host loop's input fold
        (`src/main.rs` step (a0), which runs before the frame gate) — so the
        batch's FIRST frame can run on the previous input. Measured: 13
        spurious TRUEs in 200 identical evaluations with the masks, 0 in 200
        with `hold_buttons` + `confirm_fold`, at an N where the answer is
        certainly FALSE. See `confirm_fold`. The masks stay available on the
        free `run_frames` function for callers that assert no CHANGE of input
        across the call, where the race cannot arise.
        """
        if count < 1:
            return
        for port, buttons in (holds or {}).items():
            self.set_held(port, buttons)
        if count == 1:
            self.step()
            return
        self._frame = run_frames(self.client, count, error_cls=self.error_cls)
        self.steps_taken += count
        self.batch_calls += 1
        self.frames_batched += count
        self._verify_executed(f"after run_frames({count})")

    def step_n(self, n: int) -> None:
        self.run_frames(n)
