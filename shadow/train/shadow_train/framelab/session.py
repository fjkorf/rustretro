"""The frame lab's transport layer: a session that owns the MCP primitives
`docs/frames.md` §3 makes preconditions, and enforces them.

Everything in `framelab` that touches a running emulator goes through this
module. It exists because §3's preconditions are not advice — each one is a
measured failure that produced a confident wrong number:

  * `step` is FIRE-AND-FORGET. 30 rapid `step` calls were measured landing
    **1** frame (`library/mk2/mk2.md`, "Toolkit friction"). A protocol that
    does not confirm each step counts frames that never happened, and the
    resulting "the held input did nothing" false negative is indistinguishable
    from a real result — it briefly convinced one agent that held input cannot
    reach the core while stepping. It can: +72 and +63 units over 30
    CONFIRMED frames, control 0.
  * `load_state` does not drain while paused (same GUI-frame mechanism), so a
    probe that loads while paused silently measures the PREVIOUS state.
    `LabSession.load_state` therefore resumes, loads, VERIFIES, and re-pauses.
  * `press_buttons` is BANNED (§3.3): its countdown decrements on every GUI
    frame including while paused, so a chord can evaporate between the press
    and the step. The ban is enforced by construction here — `LabSession.call`
    raises `PreconditionError` on any attempt to call it, so no code that
    routes through a session can reintroduce it.

Wall-clock note (§2.4): polling `get_state`'s `frame_count` until a step lands
is transport bookkeeping, not measurement. Nothing in this module expresses a
DURATION in wall-clock; frames are the only unit that leaves it.

This module talks to a `client` that needs exactly one method,
`client.call(tool, **kwargs) -> dict` — `McpClient.call`'s signature. That
keeps every protocol above it testable against a bare fake.
"""

from __future__ import annotations

import time
from dataclasses import dataclass
from typing import Any, Callable, Iterable, Optional, Sequence, Union

__all__ = [
    "LabError",
    "PreconditionError",
    "LabSession",
    "Preconditions",
    "BANNED_TOOLS",
    "call_ok",
    "confirm_step",
    "frame_count",
    "hold_buttons",
    "release_all",
    "set_training_enforcement",
]

# §3.3. Enforced by construction, not by convention.
BANNED_TOOLS = frozenset({"press_buttons"})

_STEP_POLL_INTERVAL_S = 0.002
_STEP_TIMEOUT_S = 5.0
_LOAD_SETTLE_POLL_S = 0.01
_LOAD_TIMEOUT_S = 5.0

# Settle before CHANGING a port's held set. See `LabSession.set_held`: the
# `step` confirmation (frame_count moved) is not proof the emulated frame is
# FINISHED, so an input change issued immediately after it can land on the
# frame that was supposed to be already over. Measured: 1 flake in 14
# identical runs with no settle, 0 in 14 with 8 ms. This is transport
# bookkeeping, not a measured duration (§2.4).
_INPUT_SETTLE_S = 0.008


class LabError(RuntimeError):
    """A frame-lab operation failed in a way that voids the measurement."""


class PreconditionError(LabError):
    """A `docs/frames.md` §3 precondition is not satisfied. Never downgraded
    to a warning: "a measurement run that skips any of these is void.\""""


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
    client: Any,
    *,
    error_cls: type = LabError,
    before: Optional[int] = None,
    timeout_s: float = _STEP_TIMEOUT_S,
    poll_s: float = _STEP_POLL_INTERVAL_S,
) -> Optional[int]:
    """Advance exactly one core frame and CONFIRM it landed. Returns the new
    `frame_count` (or None if the client does not report one).

    `before` lets a caller that already knows the current frame skip a
    round-trip; the loop still re-reads until the count actually moves.
    """
    if before is None:
        before = frame_count(client, error_cls=error_cls)
    call_ok(client, "step", error_cls=error_cls)
    if before is None:
        return None  # client reports no frame_count; best effort
    deadline = time.monotonic() + timeout_s
    while True:
        after = frame_count(client, error_cls=error_cls)
        if after != before:
            return after
        if time.monotonic() >= deadline:
            raise error_cls(
                f"step() did not advance frame_count within {timeout_s}s "
                "-- the session may be unresponsive, or the emulator is not "
                "actually paused (a held pause is what makes `step` mean "
                "exactly one frame). docs/frames.md §3.5."
            )
        time.sleep(poll_s)


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
        step_timeout_s: float = _STEP_TIMEOUT_S,
        input_settle_s: float = _INPUT_SETTLE_S,
        error_cls: type = LabError,
    ):
        self.client = client
        self.verify_fn = verify_fn
        self.ports = tuple(ports)
        self.step_timeout_s = step_timeout_s
        self.input_settle_s = input_settle_s
        self.error_cls = error_cls
        self._frame: Optional[int] = None
        self.preconditions: Optional[Preconditions] = None
        # Counters, purely for the report ("what did this run actually do").
        self.steps_taken = 0
        self.loads_done = 0

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
        """§3.6 + §3.4, in one operation: RESUME (loads do not drain while
        paused), load, confirm the load LANDED by advancing at least one real
        frame and running the injected arena verifier, then PAUSE (frame-exact
        stepping is only meaningful while paused).

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

        self.resume()
        before = frame_count(self.client, error_cls=self.error_cls)
        try:
            slot = int(spec)
            self.call("load_state", slot=slot)
        except (TypeError, ValueError):
            self.call("load_state", path=str(spec))
        self.loads_done += 1

        # Two independent confirmations, because `load_state` returning ok is
        # not one (§3.6 — loads do not drain while paused):
        #   a) the core actually ran a frame across the load, so the queue the
        #      load sits in is being serviced at all;
        #   b) `verify_fn` below, which reads the loaded state and checks it is
        #      the arena we asked for. (b) is the real check; (a) catches the
        #      "everything returns ok but nothing is running" case in which (b)
        #      would pass on the PREVIOUS state.
        if before is not None:
            deadline = time.monotonic() + _LOAD_TIMEOUT_S
            while frame_count(self.client, error_cls=self.error_cls) == before:
                if time.monotonic() >= deadline:
                    raise self.error_cls(
                        f"load_state({spec!r}) never advanced frame_count while "
                        "resumed -- the load did not drain, so anything read "
                        "from here is the PREVIOUS state (docs/frames.md §3.6)."
                    )
                time.sleep(_LOAD_SETTLE_POLL_S)

        self.pause()

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

        Settles first, and that settle is load-bearing. Confirming a `step`
        proves `frame_count` MOVED; it does not prove the emulated frame has
        FINISHED — if the count is bumped before the frame's input fold, an
        input change issued the instant the confirmation arrives can still be
        read by the frame we believed was over. That is a one-frame-early
        hold, and it is exactly the shape of the flake this lab hit live: a
        single spurious divergence at an N where the fighter is demonstrably
        still stunned, roughly 1 run in 50, never reproducing on re-run.
        Measured A/B on `r-v-r.state`, 14 identical probe/control pairs each:
        1 flake with no settle, 0 with 8 ms.

        Frames-per-input-change is small (a script has a handful of
        transitions), so the cost is a few tens of ms per run. Set
        `input_settle_s=0` to disable it — and expect `repeats>1` in
        `sweep_actionable` to start raising.
        """
        if self.input_settle_s:
            time.sleep(self.input_settle_s)
        buttons = list(buttons)
        if buttons:
            hold_buttons(self.client, buttons, port, error_cls=self.error_cls)
        else:
            release_all(self.client, port, error_cls=self.error_cls)

    def release(self, port: int) -> None:
        release_all(self.client, port, error_cls=self.error_cls)

    def release_all_ports(self) -> None:
        for port in self.ports:
            release_all(self.client, port, error_cls=self.error_cls)

    # ── stepping ─────────────────────────────────────────────────────────
    def step(self) -> None:
        self._frame = confirm_step(
            self.client,
            error_cls=self.error_cls,
            before=self._frame,
            timeout_s=self.step_timeout_s,
        )
        self.steps_taken += 1

    def step_n(self, n: int) -> None:
        for _ in range(n):
            self.step()
