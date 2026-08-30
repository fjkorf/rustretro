"""docs/frames.md §5 — the spacing ladder.

Proximity normals mean the same input is a DIFFERENT move by distance, so
gap is part of the frame table's key, and arenas that pin a gap are
GENERATED, not hand-saved. This module drives that generation:

    1. Reset to a known (2-human, live) position -- `base_arena`.
    2. Walk K frames toward the opponent on one port.
    3. Save `shadow/arenas/<family>/gap-K.state` plus a JSON sidecar
       recording K, the achieved pixel gap (or NULL if unmeasurable), both
       characters' ids, which side of the screen each occupies ("facing"),
       and per-port input liveness.

Like `calibrate.py`, this module is deliberately ignorant of any game
address: every RAM location it touches (`block1_addr`/`block2_addr`, and
optionally `char_id_off`) is a caller-supplied parameter, sourced in
production from `shadow_train.profile.GameProfile.block1()`/`.block2()` --
never hardcoded here (CLAUDE.md: "never hardcode a game address in code
again"). The one thing this module DOES hardcode is the object-pointer
DECODE ALGORITHM itself (the `-0x0C` relative offset, the
`(v - 0x01000000) >> 3` TMS34010 bit-address conversion, and the `+0x12`/
`+0x3E` object-relative offsets) -- that is protocol-level, documented
verbatim in `docs/frames.md` §5 and `library/mk2/mk2.md` ("Stable
per-fighter position") as the one true formula for this core, not a
per-game tunable.

`client` is exactly `calibrate.py`'s contract: any object with a
`.call(tool_name, **kwargs) -> dict` method (`McpClient.call`'s signature).
Every MCP call this module makes is one of `run_lua`, `enable_writes`,
`resume`, `pause`, `get_state`, `load_state`, `save_state`, `read_memory`,
`hold_buttons`, `release_buttons`, `step` -- `press_buttons` is BANNED here
exactly as it is in `calibrate.py` (docs/frames.md §3 precondition 3).

Preconditions from §3, and how this module honors them:

  1. Training enforcement OFF -- enforced once per `build_gap_ladder` call.
  2. Shadow runner disabled -- not enforceable from here (no MCP tool
     reports it); the operator's job, same as `calibrate.py`.
  3. `hold_buttons`/`release_buttons` only -- enforced by construction; grep
     this file for "press_buttons" and find no call site.
  4. Arena liveness re-verified after EVERY `load_state` -- `_load_state_raw`
     always precedes a `_probe_port_liveness` pass before anything is
     measured or saved from that load.
  5. Every `step` confirmed to have landed -- `_step` polls `get_state`'s
     `frame_count`, identically to `calibrate.py`'s `_step`.
  6. Every `load_state` confirmed to have landed -- `_load_state_raw` resumes
     first (loads do not drain while paused), then re-pauses.
  7. Zero-point calibration -- N/A: this module measures POSITION, not
     act-again timing, so §3.1's input-latency number is not consumed here.

Honesty rule this module exists to enforce (docs/frames.md §2.5 / the
task's own restatement): "Never write 0 for an unmeasured gap." Every gap
this module cannot trust the object pointer for comes out as Python `None`
-> JSON `null`, never `0`.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional, Sequence, Union

# One implementation of "advance frames, confirmed landed" — and of "the held
# input actually reached the core" — for the whole lab.
from .session import confirm_fold as _confirm_fold
from .session import confirm_step as _confirm_step
from .session import run_frames as _run_frames

__all__ = [
    "ArenaGenerationError",
    "ArenaLivenessError",
    "ArenaReproductionError",
    "LadderArenaResult",
    "read_char_id",
    "resolve_object_ptr",
    "read_object_x",
    "measure_gap_px",
    "compute_facing",
    "build_gap_ladder_arena",
    "build_gap_ladder",
]

# ── the object-pointer protocol constants (docs/frames.md §5) ─────────────
# These are NOT game addresses -- they are the fixed TMS34010 bit-addressing
# decode and the object-pool layout that formula resolves through, identical
# for every fighter/arena on this core. The per-game facts (block1/block2)
# are always caller-supplied.
_OBJ_PTR_REL_OFF = -0x0C          # object pointer lives at block + this
_OBJ_BASE = 0x01000000            # valid pointer range floor
_OBJ_MAX = 0x01400000             # valid pointer range ceiling (exclusive)
_OBJ_X_OFF = 0x12                 # world X within the object entry, u16 LE
_OBJ_CHARID_OFF = 0x3E            # char id within the object entry, u8

# `step` used to be fire-and-forget server-side, which forced a `get_state`
# poll (docs/frames.md §3 precondition 5). It is synchronous now and reports
# `landed` itself; see `_step`.

# The liveness micro-probe (see `_probe_port_liveness`): how far a live port
# must move, in its held direction's sign, per probe leg (1 unit = 1 pixel
# per §5). Deliberately well under the smallest real per-leg delta measured
# live on this rig (backward walk, the slower of the two: +5px over 6
# frames) so genuine backward-walk asymmetry never reads as "not live".
_LIVE_THRESHOLD_PX = 2
_DEFAULT_PROBE_FRAMES = 6


class ArenaGenerationError(RuntimeError):
    """Base class for every way `build_gap_ladder_arena` can refuse to
    produce (or ship) an arena."""


class ArenaLivenessError(ArenaGenerationError):
    """A port failed docs/frames.md §5's liveness re-check after
    `load_state` -- e.g. a 1P-vs-CPU rig, or a pool-slot pointer gone stale.
    Nothing is saved to disk when this is raised."""


class ArenaReproductionError(ArenaGenerationError):
    """The saved `.state` file's pixel gap did not reproduce on a fresh
    reload. The task's acceptance criterion is explicit: 'An arena that
    does not reproduce is a broken arena; delete it rather than shipping
    it' -- the caller's `.state` file is removed before this raises."""


@dataclass(frozen=True)
class LadderArenaResult:
    """Everything recorded for one rung of the ladder, mirroring the JSON
    sidecar 1:1 (see `build_gap_ladder_arena`'s `sidecar` dict)."""

    k: int
    state_path: Path
    sidecar_path: Path
    gap_px: Optional[int]
    char_id_block1: Optional[int]
    char_id_block2: Optional[int]
    facing: dict
    inputs_live: dict


# ── thin MCP-tool wrappers -- the ONLY calls this module makes ────────────


def _call_ok(client: Any, tool: str, **kwargs: Any) -> dict:
    r = client.call(tool, **kwargs)
    failed = isinstance(r, dict) and (
        r.get("ok") is False or ("error" in r and "ok" not in r)
    )
    if failed:
        raise ArenaGenerationError(f"{tool} failed: {r.get('error', r)}")
    return r


def _set_training_enforcement(client: Any, enabled: bool) -> None:
    flag = "true" if enabled else "false"
    _call_ok(client, "run_lua", script=f"training.set_enabled({flag})")


def _arm_writes(client: Any) -> None:
    _call_ok(client, "enable_writes")


def _resume(client: Any) -> None:
    _call_ok(client, "resume")


def _pause(client: Any) -> None:
    _call_ok(client, "pause")


def _load_state_raw(client: Any, spec: Union[str, int]) -> None:
    """`load_state` does NOT drain while paused (docs/frames.md §3
    precondition 6 / CLAUDE.md's MCP workflow note): resume, load, then
    re-pause. Landing is confirmed by the caller via a liveness/position
    read immediately after -- there is no game-agnostic "known field" this
    module can check on its own."""
    _resume(client)
    try:
        slot = int(spec)
        _call_ok(client, "load_state", slot=slot)
    except (TypeError, ValueError):
        _call_ok(client, "load_state", path=str(spec))
    _pause(client)


def _hold(client: Any, buttons: Sequence[str], port: int) -> None:
    _call_ok(client, "hold_buttons", buttons=list(buttons), port=port)


def _release(client: Any, port: int) -> None:
    _call_ok(client, "release_buttons", buttons=[], port=port)


def _step(client: Any) -> None:
    """Advance exactly one core frame, confirmed landed (docs/frames.md §3
    precondition 5) -- identical contract to `calibrate.py`'s `_step`, and
    now the same one-call implementation: `step` is synchronous and reports
    `landed`, so there is nothing left to poll."""
    _confirm_step(client, error_cls=ArenaGenerationError)


def _hold_step_release(client: Any, direction: str, port: int, frames: int) -> None:
    """Walk `port` `frames` frames in `direction`.

    Nothing samples the intermediate frames, so the whole walk is ONE
    `run_frames` call instead of `frames` round trips — a ladder rung of 70
    walk frames used to cost 70 confirmed steps.

    The direction is asserted with `hold_buttons` and then CONFIRMED to have
    reached the core's input fold before any frame runs (`confirm_fold`), not
    passed as one of `run_frames`' per-port masks: those are applied under the
    same lock acquisition that arms the batch, so the batch's first frame can
    run on the previous input. On a walk that shows up as a rung landing one
    frame short of its K — a silently wrong gap."""
    if frames <= 0:
        return
    _hold(client, [direction], port)
    _confirm_fold(client, port, error_cls=ArenaGenerationError)
    if frames == 1:
        _step(client)
        return
    _run_frames(client, frames, error_cls=ArenaGenerationError)
    _release(client, port)


def _read_bytes(client: Any, addr: int, length: int) -> bytes:
    r = _call_ok(client, "read_memory", addr=addr, len=length)
    return bytes.fromhex(r["hex"].replace(" ", ""))


def _u16_le(b: bytes) -> int:
    return b[0] | (b[1] << 8)


def _u32_le(b: bytes) -> int:
    return b[0] | (b[1] << 8) | (b[2] << 16) | (b[3] << 24)


# ── the object-pointer protocol (docs/frames.md §5) ────────────────────────


def read_char_id(client: Any, block_addr: int, char_id_off: int = 0x0) -> int:
    """`block_addr + char_id_off` -- the fighter-struct char id, the
    write-verified field every RE session in `library/mk2/mk2.md` uses as
    the staleness cross-check target."""
    return _read_bytes(client, block_addr + char_id_off, 1)[0]


def resolve_object_ptr(client: Any, block_addr: int) -> Optional[int]:
    """`obj = (u32_le(block - 0x0C) - 0x01000000) >> 3`, or `None` if the
    raw pointer falls outside the valid range (§5 / mk2.md's "pointer
    hygiene" table: 0 at boot/before P2 selects, valid only in a live
    fight)."""
    raw = _u32_le(_read_bytes(client, block_addr + _OBJ_PTR_REL_OFF, 4))
    if not (_OBJ_BASE <= raw < _OBJ_MAX):
        return None
    return (raw - _OBJ_BASE) >> 3


def read_object_x(
    client: Any, block_addr: int, char_id_off: int = 0x0
) -> Optional[int]:
    """World X in pixels for the fighter at `block_addr`, via the object
    pointer, `None` if the pointer is invalid OR the staleness cross-check
    (`obj+0x3E` must equal `block+char_id_off`) fails -- §5's "the pixel gap
    is UNKNOWN (null) for that arena" rule, applied at the single point
    every caller in this module reads X through."""
    obj = resolve_object_ptr(client, block_addr)
    if obj is None:
        return None
    cid_obj = _read_bytes(client, obj + _OBJ_CHARID_OFF, 1)[0]
    cid_block = read_char_id(client, block_addr, char_id_off)
    if cid_obj != cid_block:
        return None
    return _u16_le(_read_bytes(client, obj + _OBJ_X_OFF, 2))


def measure_gap_px(
    client: Any, block1_addr: int, block2_addr: int, char_id_off: int = 0x0
) -> Optional[int]:
    """Absolute pixel gap between the two fighters, `None` (never `0`) if
    either side's X is unmeasurable this frame."""
    x1 = read_object_x(client, block1_addr, char_id_off)
    x2 = read_object_x(client, block2_addr, char_id_off)
    if x1 is None or x2 is None:
        return None
    return abs(x2 - x1)


def compute_facing(
    client: Any, block1_addr: int, block2_addr: int, char_id_off: int = 0x0
) -> dict:
    """Which side of the screen each block occupies, derived from world X
    (docs/frames.md §5 / MACRO_ACTIONS.md §10.2: "a side swap flips the sign
    of everything gap-keyed... record it per arena so a later consumer can
    detect a swap rather than silently mis-key"). MK2 arcade ships no
    write-verified facing byte (`library/mk2/mk2.md`: `0xBE81` read a
    constant through a forced crossover, `obj+0x18` did not flip either) --
    relative X position is therefore the only honest source, and is exactly
    what a "did the sides swap" check needs. `None`/`None` when X is
    unmeasurable for either side (never guessed)."""
    x1 = read_object_x(client, block1_addr, char_id_off)
    x2 = read_object_x(client, block2_addr, char_id_off)
    if x1 is None or x2 is None:
        return {"block1_side": None, "block2_side": None}
    if x1 == x2:
        return {"block1_side": "overlap", "block2_side": "overlap"}
    side1 = "left" if x1 < x2 else "right"
    side2 = "right" if side1 == "left" else "left"
    return {"block1_side": side1, "block2_side": side2}


def _moved_in_direction(before: int, after: int, direction: str, threshold_px: int) -> bool:
    if direction == "right":
        return (after - before) >= threshold_px
    if direction == "left":
        return (before - after) >= threshold_px
    raise ValueError(f"_moved_in_direction only knows 'left'/'right', got {direction!r}")


def _probe_port_liveness(
    client: Any,
    *,
    port: int,
    block_addr: int,
    char_id_off: int,
    probe_frames: int = _DEFAULT_PROBE_FRAMES,
    direction_pair: tuple[str, str] = ("left", "right"),
    live_threshold_px: int = _LIVE_THRESHOLD_PX,
) -> Optional[bool]:
    """docs/frames.md §3 precondition 4 / the task's "Verify liveness
    yourself after each load regardless": hold `direction_pair[0]` for
    `probe_frames`, confirm the resolved X moved at least `live_threshold_px`
    in that direction's sign, then hold `direction_pair[1]` and confirm it
    moved back the other way by the same threshold. Returns `None` (not
    `False`) when the object pointer itself is unmeasurable -- that is
    "unknown", not "not live".

    Deliberately NOT a symmetric round-trip: a live 2026-08-30 measurement
    on this rig found holding `right` (P1's forward, toward P2) for 6
    confirmed frames moved X by +12px, while the immediately following
    `left` (P1's backward, away from P2) for the same 6 frames only
    recovered -5px -- MK2's forward/backward walk speeds are NOT
    symmetric, so a probe that required "returns within N px of start"
    produced a false NOT-LIVE on a rig later confirmed live by hand. Only
    the SIGN and magnitude of each leg's own delta is checked, never a
    net-displacement/return-to-origin comparison."""
    x0 = read_object_x(client, block_addr, char_id_off)
    if x0 is None:
        return None
    out_dir, back_dir = direction_pair
    _hold_step_release(client, out_dir, port, probe_frames)
    x1 = read_object_x(client, block_addr, char_id_off)
    if x1 is None:
        return None
    moved_out = _moved_in_direction(x0, x1, out_dir, live_threshold_px)
    _hold_step_release(client, back_dir, port, probe_frames)
    x2 = read_object_x(client, block_addr, char_id_off)
    if x2 is None:
        return None
    moved_back = _moved_in_direction(x1, x2, back_dir, live_threshold_px)
    return bool(moved_out and moved_back)


def _utcnow_iso() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _delete_if_exists(path: Path) -> None:
    try:
        path.unlink()
    except FileNotFoundError:
        pass


# ── the ladder generator ────────────────────────────────────────────────


def build_gap_ladder_arena(
    client: Any,
    *,
    base_arena: Union[str, int],
    out_dir: Union[str, Path],
    k: int,
    walk_port: int,
    walk_direction: str,
    block1_addr: int,
    block2_addr: int,
    char_id_off: int = 0x0,
    name: Optional[str] = None,
    probe_frames: int = _DEFAULT_PROBE_FRAMES,
    family: str = "mk2",
    port_name: str = "arcade",
) -> LadderArenaResult:
    """Build and save one rung of the spacing ladder (docs/frames.md §5):

        1. Reset to `base_arena` (a save-state slot or path).
        2. Re-verify liveness on BOTH ports (§3 precondition 4) -- refuses
           to save (`ArenaLivenessError`) rather than shipping a 1P-vs-CPU
           or stale-pointer arena.
        3. Walk `k` confirmed frames toward the opponent on `walk_port`.
        4. Save `<out_dir>/gap-<k>.state` (or `<name>.state` if given).
        5. Reload the just-saved file fresh and re-measure -- if the pixel
           gap or either char id does not reproduce EXACTLY, the `.state`
           file is deleted and `ArenaReproductionError` is raised (the
           task's "delete it rather than shipping it" rule).
        6. Write the JSON sidecar (`<out_dir>/gap-<k>.gap.json`, or
           `<name>.gap.json`) with `walk_frames`, `gap_px` (nullable),
           both char ids, `facing`, and `inputs_live` for both ports.

    NOTE on the sidecar's filename: this deliberately does NOT reuse the
    `.meta.json` extension the app itself auto-writes for any `save_state`
    call whose path contains an `arenas` path component
    (`Frontend::write_arena_sidecar`, `src/frontend.rs`) -- that happens
    out-of-band, inside the same `save_state` MCP round-trip, and this
    module has no way to suppress it (file scope forbids touching
    `frontend.rs`). Worse, that auto-written sidecar's own `inputs_live`
    reads MK2 arcade's `x` fighter field, which `mk2.profile.json` still
    sources from the DISPROVEN globals (`p1_x`/`p2_x`, `0x6CBA`/`0x6CFC`) --
    exactly the pool-slot-instability bug this module's object-pointer
    reads exist to route around. So the two sidecars coexist by design:
    `.meta.json` is the app's (possibly-stale) auto-write, `.gap.json` is
    this module's own, trustworthy one carrying the fields the task
    actually asked for. See this task's final report for the fuller
    writeup -- flagged as a place docs/frames.md §5 underspecified the
    sidecar's filename/collision contract.
    """
    other_port = 1 - walk_port
    if other_port not in (0, 1):
        raise ValueError(f"walk_port must be 0 or 1, got {walk_port}")
    if k < 0:
        raise ValueError(f"k (walk-frames) must be >= 0, got {k}")

    out_dir = Path(out_dir)
    stem = name if name is not None else f"gap-{k}"
    state_path = out_dir / f"{stem}.state"
    sidecar_path = out_dir / f"{stem}.gap.json"

    # §3 precondition 1.
    _set_training_enforcement(client, False)
    _arm_writes(client)

    # ── load + re-verify liveness (§3 precondition 4) ──────────────────
    _load_state_raw(client, base_arena)
    _release(client, 0)
    _release(client, 1)

    live_walk = _probe_port_liveness(
        client, port=walk_port, block_addr=(block1_addr if walk_port == 0 else block2_addr),
        char_id_off=char_id_off, probe_frames=probe_frames,
        direction_pair=(walk_direction, _opposite_direction(walk_direction)),
    )
    live_other = _probe_port_liveness(
        client, port=other_port, block_addr=(block2_addr if walk_port == 0 else block1_addr),
        char_id_off=char_id_off, probe_frames=probe_frames,
    )
    live_p0 = live_walk if walk_port == 0 else live_other
    live_p1 = live_other if walk_port == 0 else live_walk
    if not (live_p0 and live_p1):
        raise ArenaLivenessError(
            f"{stem}: liveness check failed after load_state({base_arena!r}) "
            f"-- p0={live_p0!r} p1={live_p1!r} (docs/frames.md §3 "
            "precondition 4) -- refusing to save."
        )

    # ── the real walk ───────────────────────────────────────────────────
    _hold_step_release(client, walk_direction, walk_port, k)

    gap_pre = measure_gap_px(client, block1_addr, block2_addr, char_id_off)
    cid1_pre = read_char_id(client, block1_addr, char_id_off)
    cid2_pre = read_char_id(client, block2_addr, char_id_off)

    # ── save ─────────────────────────────────────────────────────────
    _call_ok(client, "save_state", path=str(state_path))

    # ── reproduce-on-reload check (task acceptance) ────────────────────
    _load_state_raw(client, str(state_path))
    gap_post = measure_gap_px(client, block1_addr, block2_addr, char_id_off)
    cid1_post = read_char_id(client, block1_addr, char_id_off)
    cid2_post = read_char_id(client, block2_addr, char_id_off)
    facing = compute_facing(client, block1_addr, block2_addr, char_id_off)

    reproduces = (
        gap_pre == gap_post and cid1_pre == cid1_post and cid2_pre == cid2_post
    )
    if not reproduces:
        _delete_if_exists(state_path)
        # `save_state` (above) also triggered the APP'S OWN auto-arena-
        # sidecar write (`Frontend::write_arena_sidecar`, `src/frontend.rs`
        # -- any `save_state` path containing an `arenas` component gets
        # one, out of band, inside that same MCP round-trip) before we
        # ever got here. Clean up that orphan too, or a broken arena's
        # `.meta.json` outlives its now-deleted `.state` file.
        _delete_if_exists(state_path.parent / f"{state_path.stem}.meta.json")
        raise ArenaReproductionError(
            f"{stem}: did not reproduce on reload -- pre=(gap={gap_pre}, "
            f"cid1={cid1_pre}, cid2={cid2_pre}) post=(gap={gap_post}, "
            f"cid1={cid1_post}, cid2={cid2_post}) -- deleted {state_path}, "
            "not shipping a broken arena."
        )

    inputs_live = {"p0": live_p0, "p1": live_p1}
    sidecar = {
        "format": "gap-ladder-v1",
        "family": family,
        "port": port_name,
        "walk_frames": k,
        "walk_port": walk_port,
        "walk_direction": walk_direction,
        "gap_px": gap_post,
        "char_id_block1": cid1_post,
        "char_id_block2": cid2_post,
        "facing": facing,
        "inputs_live": inputs_live,
        "base_arena": str(base_arena),
        "saved_at": _utcnow_iso(),
    }
    out_dir.mkdir(parents=True, exist_ok=True)
    sidecar_path.write_text(json.dumps(sidecar, indent=2) + "\n")

    return LadderArenaResult(
        k=k,
        state_path=state_path,
        sidecar_path=sidecar_path,
        gap_px=gap_post,
        char_id_block1=cid1_post,
        char_id_block2=cid2_post,
        facing=facing,
        inputs_live=inputs_live,
    )


def _opposite_direction(direction: str) -> str:
    pairs = {"left": "right", "right": "left", "up": "down", "down": "up"}
    try:
        return pairs[direction]
    except KeyError:
        raise ValueError(f"no opposite direction known for {direction!r}") from None


def build_gap_ladder(
    client: Any,
    *,
    base_arena: Union[str, int],
    out_dir: Union[str, Path],
    ks: Sequence[int],
    walk_port: int = 0,
    walk_direction: str = "right",
    block1_addr: int,
    block2_addr: int,
    char_id_off: int = 0x0,
    probe_frames: int = _DEFAULT_PROBE_FRAMES,
    family: str = "mk2",
    port_name: str = "arcade",
) -> list[LadderArenaResult]:
    """Build every rung named in `ks` (ascending order, so the point-blank
    K=0 rung -- if present -- establishes the base gap first). A rung that
    raises (`ArenaLivenessError`/`ArenaReproductionError`) stops the whole
    ladder rather than silently skipping it -- §7's "no silent caps": the
    caller sees exactly which rung failed and why."""
    results = []
    for k in sorted(ks):
        results.append(
            build_gap_ladder_arena(
                client,
                base_arena=base_arena,
                out_dir=out_dir,
                k=k,
                walk_port=walk_port,
                walk_direction=walk_direction,
                block1_addr=block1_addr,
                block2_addr=block2_addr,
                char_id_off=char_id_off,
                probe_frames=probe_frames,
                family=family,
                port_name=port_name,
            )
        )
    return results


# ── operator entry point ───────────────────────────────────────────────


def main() -> None:
    """Drive a real MK2 headless session (port 4042 per this task's
    constraints -- 4025 is the user's live session and must never be
    touched) to generate the spacing ladder into `shadow/arenas/mk2/`.
    Not exercised by the unit tests (those pass a fake `client`); this is
    the operator path CLAUDE.md's per-task README would point at."""
    import sys

    from shadow_train import profile as game_profile
    from shadow_train.mcpclient import McpClient

    url = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:4042/mcp"
    base_arena = sys.argv[2] if len(sys.argv) > 2 else "shadow/arenas/mk2/r-v-r.state"
    ks = [int(x) for x in sys.argv[3:]] if len(sys.argv) > 3 else [0, 15, 40, 80]

    prof = game_profile.load("library/mk2")
    char_id_off, _size = prof.field_off("char_id")
    client = McpClient(url)
    results = build_gap_ladder(
        client,
        base_arena=base_arena,
        out_dir="shadow/arenas/mk2",
        ks=ks,
        walk_port=0,
        walk_direction="right",
        block1_addr=prof.block1(),
        block2_addr=prof.block2(),
        char_id_off=char_id_off or 0x0,
        family=prof.family,
        port_name=prof.port,
    )
    for r in results:
        print(f"K={r.k:>4}  gap_px={r.gap_px!r:>6}  "
              f"chars=({r.char_id_block1},{r.char_id_block2})  "
              f"facing={r.facing}  inputs_live={r.inputs_live}  "
              f"-> {r.state_path}")


if __name__ == "__main__":
    main()
