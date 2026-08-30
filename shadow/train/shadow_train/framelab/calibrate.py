"""docs/frames.md §3.1 — zero-point calibration — and §2.3's sprite-lag
measurement (deferred; see `sprite_lag_frames`'s docstring).

The whole protocol boils down to one differential measurement, repeated:

    1. Neutral, both fighters idle, nothing driving either port.
    2. Hold a walk direction at a known frame F.
    3. Step; find the first frame the fighter's observable diverges from a
       no-input control run.
    4. `input_latency_frames = that frame - F`. Repeat >=5 times; it MUST be
       constant. If it varies, STOP and raise -- never average.

This module drives that over a `client` argument that only needs ONE method:
`client.call(tool_name, **kwargs) -> dict`, exactly `McpClient.call`'s
signature (see `shadow_train.mcpclient.McpClient` and `shadow_train.re`).
That keeps the calibration logic testable against a bare stub — no
`Probe`-style constructor round-trip (`list_regions` etc.) required — while
a real session can pass an actual `McpClient` (or `Probe.client`) straight
through.

Two things this module is deliberately ignorant of, by design, per
CLAUDE.md's "never hardcode a game address in code again":

  - **The observable.** §4.2 ranks fighter-struct divergence, the
    `action_counter` edge, and mapped `x` in preference order, and which one
    applies is per-port. The caller supplies `observable_fn(client)`, reading
    whatever the profile says for this port.
  - **Arena liveness.** What "the arena is still live" means (a running-byte
    oracle, an `inputs_live` check, ...) is also per-port/per-rig. The caller
    supplies `liveness_fn(client)`.

Preconditions from §3, and which of them this module enforces:

  1. Training enforcement OFF — ENFORCED: `run_lua("training.set_enabled
     (false)")` is issued once before any trial runs.
  2. Shadow runner disabled — NOT ENFORCEABLE FROM HERE: no MCP tool reports
     whether a shadow model is currently driving a port. The operator must
     ensure this (e.g. don't pass `--shadow`, or make sure Shift+F5 is off).
  3. `hold_buttons`/`release_buttons` only, `press_buttons` BANNED — ENFORCED
     BY CONSTRUCTION: this module calls exactly `hold_buttons`/
     `release_buttons`/`load_state`/`step`/`run_lua`/`enable_writes`/`pause`/
     `get_state` (the last three only to arm `load_state`, to make `step`
     frame-exact, and to CONFIRM a step landed — see `_load_state`/`_step`)
     and nothing else; grep this file for "press_buttons" and find no call
     site.

     A live smoke test surfaced why `_step` needs that confirmation: `step`
     is fire-and-forget server-side (it sets a flag and returns immediately
     — the same shape as `press()`'s documented "schedules ... does NOT
     block" gotcha in `shadow_train.re`'s module docstring), so firing many
     `step` calls back-to-back with no wait between them can have several
     collapse into a single real frame advance before the host's own
     Update loop ever consumes the flag. Measured live: 10 bare `step()`
     calls back-to-back moved `frame_count` by 0; the same 10 calls spaced
     50ms apart moved it by exactly 10. `_step` below polls `get_state`'s
     `frame_count` until it actually increments (bounded, `_STEP_TIMEOUT`)
     instead of guessing a sleep duration — this is, in the letter, a
     "sleep" that §2.4 ("Wall-clock is never a unit... Everything steps")
     reads as banned; in practice it's unavoidable AS LONG AS `step` stays
     fire-and-forget, and polling a real oracle for step-landing is a much
     smaller wall-clock dependency than a guessed fixed delay would be. See
     this task's final report for the fuller writeup — flagged there as a
     genuine tension in the contract worth resolving upstream (either make
     `step` synchronous, or §2.4 should carve out an explicit exception for
     confirming step-landing).
  4. Arena liveness re-verified after EVERY `load_state` — ENFORCED: every
     `_load_state` call in this module is immediately followed by a
     `liveness_fn` check that raises on failure.
  5. Zero-point calibration current — N/A recursively: this module IS what
     produces that currency for everything downstream.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Callable, Optional, Sequence, Union

from .session import (
    _STEP_TIMEOUT_S,
    call_ok,
    confirm_step,
    hold_buttons,
    release_all,
)
from .session import set_training_enforcement as _session_set_training_enforcement

__all__ = [
    "CalibrationError",
    "CalibrationResult",
    "zero_point_calibration",
    "sprite_lag_frames",
]

# §4.3: "MAX_SEARCH is ~60 frames; the cost is bounded and the result is
# unconditionally correct" (linear sweep, the default method).
DEFAULT_MAX_SEARCH = 60

ObservableFn = Callable[[Any], Any]
LivenessFn = Callable[[Any], bool]


class CalibrationError(RuntimeError):
    """Calibration is not sound: no divergence found, an arena died after
    `load_state`, or (§3.1's central rule) the measured latency was not
    constant across trials."""


@dataclass(frozen=True)
class CalibrationResult:
    port: int
    walk_direction: str
    input_latency_frames: int
    trials: int
    samples: tuple[int, ...]


# ── thin MCP-tool wrappers (the ONLY calls this module makes) ──────────────
#
# Each takes the same `client` with a `.call(tool, **kwargs)` method as
# `McpClient`/`Probe.client`. Deliberately not `hold_buttons`/`release_buttons`
# convenience methods on `McpClient` itself (it doesn't have any — only
# `press()`, which is BANNED here) — going through raw `.call()` means a bare
# stub client needs to implement exactly one method to be testable.


def _call_ok(client: Any, tool: str, **kwargs: Any) -> dict:
    """`session.call_ok` with this module's error type. The shared
    implementation is in `session` so there is exactly ONE definition of
    "did this MCP tool succeed" (and exactly one `press_buttons` ban) across
    the lab; the `error_cls` indirection is what lets this module keep
    raising `CalibrationError` for its own callers and tests."""
    return call_ok(client, tool, error_cls=CalibrationError, **kwargs)


def _set_training_enforcement(client: Any, enabled: bool) -> None:
    _session_set_training_enforcement(client, enabled, error_cls=CalibrationError)


def _arm_writes(client: Any) -> None:
    # `load_state` is write-gated ("REPLACES the entire game state, so it
    # REQUIRES enable_writes first" — src/mcp/server.rs); `hold_buttons` /
    # `release_buttons` / `step` / `run_lua` are NOT gated. Arming once per
    # calibration run is enough — the session stays armed until
    # `disable_writes`.
    _call_ok(client, "enable_writes")


def _pause(client: Any) -> None:
    _call_ok(client, "pause")


def _load_state(client: Any, spec: Union[str, int], liveness_fn: LivenessFn) -> None:
    try:
        slot = int(spec)
        _call_ok(client, "load_state", slot=slot)
    except (TypeError, ValueError):
        _call_ok(client, "load_state", path=str(spec))
    # §3 precondition 4: re-verify EVERY time, not once at capture. Whatever
    # `liveness_fn` does internally (it may itself resume/sleep to watch a
    # free-running oracle byte — see CLAUDE.md's "MCP / agent workflow"), we
    # explicitly (re-)pause afterward, unconditionally: the trace this feeds
    # is stepped one core frame at a time (`_step`), which is only
    # frame-EXACT while paused ("For frame-exact work: pause -> step/reads
    # -> resume" — CLAUDE.md). Skipping this made a live smoke test of this
    # module measure real-time jitter instead of frames (input_latency
    # samples of 32/21/47/44/38 across 5 supposedly-identical trials);
    # pausing first fixed it.
    if not liveness_fn(client):
        raise CalibrationError(
            f"arena {spec!r} failed its liveness check immediately after "
            "load_state -- docs/frames.md §3 precondition 4. Nothing "
            "measured from here would be trustworthy."
        )
    _pause(client)


def _hold(client: Any, buttons: Sequence[str], port: int) -> None:
    hold_buttons(client, buttons, port, error_cls=CalibrationError)


def _release(client: Any, port: int) -> None:
    # Empty `buttons` releases the whole port's held set (server.rs:
    # "hold_buttons(0, []) releases everything").
    release_all(client, port, error_cls=CalibrationError)


def _step(client: Any) -> None:
    """Advance exactly one core frame and CONFIRM it landed before
    returning, by polling `get_state`'s `frame_count` (see the module
    docstring's precondition-3 note for why this confirmation is
    necessary — `step` itself is fire-and-forget). Shared implementation:
    `session.confirm_step`."""
    confirm_step(client, error_cls=CalibrationError, timeout_s=_STEP_TIMEOUT_S)


# ── the differential trace ──────────────────────────────────────────────


def _record_trace(
    client: Any,
    *,
    arena: Union[str, int],
    liveness_fn: LivenessFn,
    observable_fn: ObservableFn,
    port: int,
    other_port: int,
    warmup_frames: int,
    max_search: int,
    hold_direction: Optional[str],
) -> list:
    """Neutral -> optional warmup -> (optionally hold `hold_direction`) ->
    step `max_search` times, recording `observable_fn(client)` after each
    step. `hold_direction=None` is the no-input CONTROL run."""
    _load_state(client, arena, liveness_fn)
    _release(client, port)
    _release(client, other_port)
    for _ in range(warmup_frames):
        _step(client)
    if hold_direction is not None:
        _hold(client, [hold_direction], port=port)
    trace = []
    for _ in range(max_search):
        _step(client)
        trace.append(observable_fn(client))
    if hold_direction is not None:
        _release(client, port)
    return trace


def _first_divergence(probe_trace: list, control_trace: list) -> Optional[int]:
    """1-indexed frame number of the first divergence, matching §3.1's
    "input_latency_frames = that frame - F" when F is the hold-assertion
    instant (frame 0 of this trace)."""
    for i, (p, c) in enumerate(zip(probe_trace, control_trace), start=1):
        if p != c:
            return i
    return None


def _run_trial(
    client: Any,
    *,
    arena: Union[str, int],
    observable_fn: ObservableFn,
    liveness_fn: LivenessFn,
    port: int,
    other_port: int,
    directions: Sequence[str],
    warmup_frames: int,
    max_search: int,
) -> tuple[int, str]:
    control_trace = _record_trace(
        client, arena=arena, liveness_fn=liveness_fn, observable_fn=observable_fn,
        port=port, other_port=other_port, warmup_frames=warmup_frames,
        max_search=max_search, hold_direction=None,
    )
    for direction in directions:
        probe_trace = _record_trace(
            client, arena=arena, liveness_fn=liveness_fn, observable_fn=observable_fn,
            port=port, other_port=other_port, warmup_frames=warmup_frames,
            max_search=max_search, hold_direction=direction,
        )
        latency = _first_divergence(probe_trace, control_trace)
        if latency is not None:
            return latency, direction
    # §4.2 corner hazard: "if neither direction diverges, record NULL rather
    # than 'never actionable'" — that's the general actionability probe's
    # contract. Calibration itself has no NULL to fall back to (§3.1: "an
    # uncalibrated run is not a run"), so exhausting every candidate
    # direction here is a hard failure, not a null result.
    raise CalibrationError(
        f"no divergence within {max_search} frames for any of "
        f"{list(directions)} on port {port} -- the probe is not sound on "
        "this port/arena (docs/frames.md §3.1 step 4)."
    )


def zero_point_calibration(
    client: Any,
    *,
    arena: Union[str, int],
    observable_fn: ObservableFn,
    liveness_fn: LivenessFn,
    port: int = 0,
    directions: Sequence[str] = ("left", "right"),
    warmup_frames: int = 0,
    max_search: int = DEFAULT_MAX_SEARCH,
    trials: int = 5,
) -> CalibrationResult:
    """docs/frames.md §3.1, implemented exactly: run >=5 independent trials
    from the same `arena` save state, each a probe-vs-control differential
    walk, and require the resulting `input_latency_frames` be IDENTICAL
    across all of them. Raises `CalibrationError` the moment it isn't —
    never averages (§7: "a number that fails re-measurement is DELETED, not
    averaged").

    `directions` are tried in order on the FIRST trial only (§4.2's corner
    hazard: a cornered fighter can't walk into the wall); whichever one
    diverges first is then reused for every subsequent trial, so trials 2..N
    are testing STABILITY of that one measurement, not shopping for a
    direction that happens to work that time.

    `arena` is a save-state slot (int, or a numeric string) or a path,
    exactly like `McpClient.load_state`.
    """
    if trials < 5:
        raise ValueError(
            f"docs/frames.md §3.1 step 4 requires >=5 trials, got {trials}"
        )
    if not directions:
        raise ValueError("`directions` must name at least one walk direction")

    other_port = 1 - port
    if other_port not in (0, 1):
        raise ValueError(f"`port` must be 0 or 1, got {port}")

    # §3 precondition 1.
    _set_training_enforcement(client, False)
    # `load_state` needs writes armed (see `_arm_writes`); do it once, up
    # front, rather than re-arming inside every trial's `_load_state` call.
    _arm_writes(client)

    samples: list[int] = []
    candidate_directions = directions
    chosen_direction: Optional[str] = None
    for _ in range(trials):
        latency, chosen_direction = _run_trial(
            client, arena=arena, observable_fn=observable_fn,
            liveness_fn=liveness_fn, port=port, other_port=other_port,
            directions=candidate_directions, warmup_frames=warmup_frames,
            max_search=max_search,
        )
        samples.append(latency)
        # Lock in the direction that worked so later trials measure
        # stability of THAT probe, not direction availability.
        candidate_directions = (chosen_direction,)

    if len(set(samples)) != 1:
        raise CalibrationError(
            f"zero-point calibration is not constant across {trials} trials "
            f"(samples={samples}) -- docs/frames.md §3.1: 'it MUST be "
            "constant. If it is not, STOP -- the probe is not sound on this "
            "port and nothing downstream can be trusted.' Not averaging."
        )

    assert chosen_direction is not None
    return CalibrationResult(
        port=port,
        walk_direction=chosen_direction,
        input_latency_frames=samples[0],
        trials=trials,
        samples=tuple(samples),
    )


def sprite_lag_frames(*_args: Any, **_kwargs: Any) -> int:
    """UNIMPLEMENTED. docs/frames.md §2.3 defines sprite lag as a measured
    calibration between the core's LOGIC-frame divergence (what
    `zero_point_calibration` measures) and the first frame the RENDERED
    SPRITE visibly moves — so a video-frame-counted community number can be
    reconciled with ours by a documented constant instead of a silently
    applied fudge factor.

    That's a frame-exact IMAGE comparison: capture `app://screen` every core
    frame from the same hold-instant `zero_point_calibration` uses, and find
    the first frame whose pixels (in the walking sprite's on-screen rect)
    diverge from a no-input control capture — the same §4.2 differential
    protocol, just over PNG bytes instead of a memory read.

    This project DOES expose a screenshot path (`McpClient.screenshot` /
    `app://screen`), so this isn't a hard "no video path" negative the way
    Genesis's missing contact signal is (§4.1). It's deferred anyway, for
    reasons of correctness rather than availability:

      1. No sprite on-screen bounding box exists anywhere in the profile
         schema (`docs/game-profiles.md`) for either port. This module's
         house rule is "never hardcode a game address in code again" — a
         pixel rect is exactly that kind of fact, and adding one would mean
         a profile schema change, which is out of this task's file scope
         (`shadow_train/framelab/` only).
      2. `Probe.screenshot`'s own docstring already flags a known frame-
         registration hazard: right after `load_state`, `app://screen`
         still shows the PREVIOUS rendered frame until the core runs >=1
         frame — the screenshot pipeline has a documented off-by-one risk
         that has never been resolved for frame-EXACT (as opposed to
         "eyeball it") use.
      3. §8's acceptance criteria call out exactly this failure mode: "a
         3-frame systematic error passes criteria 1-3 unnoticed." This task
         explicitly forbids running a real measurement to validate anything
         against ("Do NOT run a real measurement or populate real data"),
         so shipping an unvalidated `sprite_lag_frames` would assert a
         number nobody has checked — worse than not shipping it.

    Raises unconditionally. Do not substitute a fudge-factor offset in its
    place (§2.3: "do NOT silently apply an offset to reconcile with an
    external table").
    """
    raise NotImplementedError(
        "sprite_lag_frames is deliberately unimplemented -- see this "
        "function's docstring (docs/frames.md §2.3) for why."
    )
