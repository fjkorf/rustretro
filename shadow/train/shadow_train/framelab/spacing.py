"""Measure ONE MATCHUP's own walk curve and its collision floor — the input
to `arenas.py`'s ladder, and a per-matchup fact rather than a per-game one.

`docs/frames.md` §5 publishes a measured K → gap curve ("a ~1.6 px/frame
startup ramp, a ~2.5 px/frame cruise from K≈5 to K≈45, then a hard floor at
62 px") and the profile's `framelab.spacing.collision_floor_px` carries the
62. **Both were measured on a Reptile mirror, and neither transfers.** Walk
speed is a property of the CHARACTER doing the walking and the floor is a
property of the two bodies involved (§4.4: "point-blank is a measured value
per matchup, never an assumed zero"). Anything that reuses another matchup's
numbers is asserting a measurement it did not make — the same failure shape
as reusing another port's calibration.

So this module measures both, for whatever pair is loaded:

  * `walk_curve` — hold one direction from a fixed reset and read the
    pointer-resolved gap after EVERY frame, giving K → px for every K in one
    pass of `max_k` frames instead of `max_k` reload-and-walk runs.
  * `verify_k` — the cross-check that makes that shortcut legitimate: an
    INDEPENDENT reload-and-walk to a named K must reproduce the continuous
    pass's gap at that K exactly. `arenas.build_gap_ladder_arena` walks the
    reload-and-walk way, so if the two ever disagreed the curve would be
    describing a trajectory no arena can reproduce.
  * `collision_floor` — pure logic over the curve: the gap at which more
    walking stops closing distance, plus the K at which the plateau begins.
    NULL (never a number) when no plateau is observed inside `max_k`, because
    "the floor is further out than I looked" is not "the floor is the last
    thing I saw" (§7, no silent caps).

Reading the gap between steps is free of the §3.6 fold hazard: the direction
is asserted ONCE with `set_held` + fold confirmation before the first frame,
and never changed mid-walk, so no frame ever runs on a stale input mask.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any, Dict, List, Optional, Sequence, Union

from .arenas import measure_gap_px, read_char_id, read_object_x

__all__ = [
    "WalkPoint",
    "FloorResult",
    "walk_curve",
    "verify_k",
    "collision_floor",
    "curve_segments",
    "main",
]


@dataclass(frozen=True)
class WalkPoint:
    """The gap after `k` confirmed walk-frames from the base arena. `gap_px`
    is Optional by design — an unresolvable object pointer means the gap is
    UNKNOWN for that frame (§5), never 0."""

    k: int
    gap_px: Optional[int]
    x_walker: Optional[int]
    x_other: Optional[int]


@dataclass(frozen=True)
class FloorResult:
    """`floor_px` is the collision floor; `first_k` the smallest K that
    reaches it; `plateau_frames` how many consecutive points held it. All
    None when no plateau was observed (see `collision_floor`)."""

    floor_px: Optional[int]
    first_k: Optional[int]
    plateau_frames: int


def walk_curve(
    session: Any,
    *,
    base_arena: Union[str, int],
    walk_port: int,
    walk_direction: str,
    block1_addr: int,
    block2_addr: int,
    char_id_off: int = 0x0,
    max_k: int,
) -> List[WalkPoint]:
    """K → gap for every K in `0..max_k`, from one continuous hold.

    K=0 is read BEFORE any frame runs, so it is the base arena's own gap.
    The walk is one uninterrupted hold (asserted once, fold-confirmed by
    `LabSession.set_held`), stepped one frame at a time so the gap can be
    sampled in between; the sampling reads memory only and cannot perturb
    the walk.
    """
    if max_k < 0:
        raise ValueError(f"max_k must be >= 0, got {max_k}")
    session.load_state(base_arena)
    session.release_all_ports()

    def sample(k: int) -> WalkPoint:
        x_w = read_object_x(
            session, block1_addr if walk_port == 0 else block2_addr, char_id_off
        )
        x_o = read_object_x(
            session, block2_addr if walk_port == 0 else block1_addr, char_id_off
        )
        gap = None if (x_w is None or x_o is None) else abs(x_o - x_w)
        return WalkPoint(k=k, gap_px=gap, x_walker=x_w, x_other=x_o)

    points = [sample(0)]
    if max_k == 0:
        return points
    session.set_held(walk_port, [walk_direction])
    for k in range(1, max_k + 1):
        session.step()
        points.append(sample(k))
    session.release(walk_port)
    return points


def verify_k(
    session: Any,
    *,
    base_arena: Union[str, int],
    walk_port: int,
    walk_direction: str,
    block1_addr: int,
    block2_addr: int,
    char_id_off: int = 0x0,
    k: int,
    settle_frames: int = 0,
) -> Optional[int]:
    """An INDEPENDENT reload-and-walk to `k`, returning the achieved gap.

    This is the shape `arenas.build_gap_ladder_arena` uses (one batched walk
    from a fresh load), so agreeing with `walk_curve` at the same K is what
    licenses reading a whole ladder off one continuous pass.

    `settle_frames` runs that many NEUTRAL frames after the walk before
    reading. It exists because a gap read on a frame where the walk is still
    held is not the gap a fight starts from: once the two bodies touch, MK2's
    anti-overlap resolution and the walk animation between them make the
    measured gap oscillate frame to frame (measured on Mileena-vs-Reptile:
    60–66 px while walking, settling to a single value the moment the
    direction is released). A rung whose gap is one sample of an oscillation
    is reproducible but not meaningful; a settled one is both.
    """
    session.load_state(base_arena)
    session.release_all_ports()
    if k > 0:
        session.set_held(walk_port, [walk_direction])
        session.run_frames(k)
        session.release(walk_port)
    if settle_frames > 0:
        session.run_frames(settle_frames)
    return measure_gap_px(session, block1_addr, block2_addr, char_id_off)


def collision_floor(
    points: Sequence[WalkPoint], *, plateau_frames: int = 6
) -> FloorResult:
    """The gap beyond which walking closes nothing, from a walk curve.

    A floor is only a floor if the curve SAT on it: the minimum gap must be
    held by at least `plateau_frames` consecutive points, all the way to the
    end of the curve. A minimum touched once at the last sampled K is a
    curve that had not finished falling, and this returns all-None for it
    rather than reporting the last value seen (§7).
    """
    known = [p for p in points if p.gap_px is not None]
    if not known:
        return FloorResult(None, None, 0)
    lo = min(p.gap_px for p in known)  # type: ignore[type-var]
    tail: List[WalkPoint] = []
    for p in reversed(known):
        if p.gap_px != lo:
            break
        tail.append(p)
    if len(tail) < plateau_frames:
        return FloorResult(None, None, len(tail))
    return FloorResult(floor_px=lo, first_k=tail[-1].k, plateau_frames=len(tail))


def curve_segments(points: Sequence[WalkPoint]) -> List[Dict[str, Any]]:
    """Per-frame closing rate, run-length encoded — the honest way to show a
    curve §5 explicitly says not to fit a line through. Each segment is
    `{from_k, to_k, px_per_frame, frames}` over a run of equal deltas."""
    known = [p for p in points if p.gap_px is not None]
    if len(known) < 2:
        return []
    segs: List[Dict[str, Any]] = []
    start = known[0]
    prev = known[0]
    cur_rate: Optional[float] = None
    for p in known[1:]:
        dk = p.k - prev.k
        rate = (prev.gap_px - p.gap_px) / dk  # type: ignore[operator]
        if cur_rate is None:
            cur_rate = rate
        elif rate != cur_rate:
            segs.append({"from_k": start.k, "to_k": prev.k,
                         "px_per_frame": cur_rate, "frames": prev.k - start.k})
            start, cur_rate = prev, rate
        prev = p
    segs.append({"from_k": start.k, "to_k": prev.k,
                 "px_per_frame": cur_rate, "frames": prev.k - start.k})
    return segs


def main() -> None:  # pragma: no cover - the live-rig path
    """Print one matchup's walk curve, its collision floor, and the
    reload-and-walk cross-checks.

        python -m shadow_train.framelab.spacing \\
            --url http://127.0.0.1:4066/mcp --game library/mk2 \\
            --base shadow/arenas/mk2/m-v-r.state --max-k 110 \\
            --verify 0 --verify 30 --verify 60

    Never point `--url` at port 4025 (CLAUDE.md: the user's session).
    """
    import argparse
    import time

    from shadow_train import profile as game_profile
    from shadow_train.mcpclient import McpClient

    from . import observables as obs
    from .session import LabSession

    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--url", default="http://127.0.0.1:4066/mcp")
    ap.add_argument("--game", default="library/mk2")
    ap.add_argument("--base", required=True, help="the base arena .state")
    ap.add_argument("--max-k", type=int, default=110)
    ap.add_argument("--walk-port", type=int, default=0)
    ap.add_argument("--walk-direction", default="right")
    ap.add_argument("--plateau-frames", type=int, default=6)
    ap.add_argument("--verify", action="append", type=int, default=[],
                    help="K to re-derive by an independent reload-and-walk")
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    if ":4025" in args.url:
        raise SystemExit("refusing port 4025 — that is the user's live session.")

    prof = game_profile.load(args.game)
    char_id_off, _ = prof.field_off("char_id")
    b1, b2 = prof.block1(), prof.block2()
    session = LabSession(
        McpClient(args.url), verify_fn=obs.make_arena_verifier(prof, expect={})
    )
    session.enforce_preconditions()
    started = time.monotonic()

    kw = dict(base_arena=args.base, walk_port=args.walk_port,
              walk_direction=args.walk_direction, block1_addr=b1,
              block2_addr=b2, char_id_off=char_id_off or 0x0)
    points = walk_curve(session, max_k=args.max_k, **kw)  # type: ignore[arg-type]
    floor = collision_floor(points, plateau_frames=args.plateau_frames)

    print(f"char ids: block1={read_char_id(session, b1, char_id_off or 0)} "
          f"block2={read_char_id(session, b2, char_id_off or 0)}")
    for p in points:
        print(f"  K={p.k:>4}  gap={p.gap_px!r:>6}  "
              f"x_walker={p.x_walker!r:>6} x_other={p.x_other!r:>6}")
    print("segments:", json.dumps(curve_segments(points)))
    print(f"floor: {floor}")

    checks = []
    for k in args.verify:
        got = verify_k(session, k=k, **kw)  # type: ignore[arg-type]
        want = next((p.gap_px for p in points if p.k == k), None)
        checks.append({"k": k, "continuous": want, "reload_and_walk": got,
                       "agree": got == want})
        print(f"verify K={k}: continuous={want} reload_and_walk={got} "
              f"{'OK' if got == want else 'DISAGREE'}")

    print(f"steps={session.steps_taken} loads={session.loads_done} "
          f"elapsed={time.monotonic() - started:.1f}s")
    if args.out:
        from pathlib import Path
        Path(args.out).write_text(json.dumps({
            "base": args.base,
            "points": [p.__dict__ for p in points],
            "segments": curve_segments(points),
            "floor": floor.__dict__,
            "verify": checks,
            "steps": session.steps_taken, "loads": session.loads_done,
        }, indent=2) + "\n")


if __name__ == "__main__":  # pragma: no cover
    main()
