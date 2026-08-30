"""Run a whole spacing ladder end to end, and RE-MEASURE one that already
shipped — the operator layer over `kit.py`.

`kit.main()` measures the cells you name, one at a time, and prints them.
This module adds the two things a full run needs and a single cell does not:

  1. **The connect map first** (docs/frames.md §5, `kit.scan_contact`): one
     replay per (move, gap), before any sweep. It is what tells you which
     cells EXIST — a whiff is an outcome, not a missing measurement — and it
     is where `connect_range` and the damage signature that separates
     proximity variants come from. Sweeping a cell the map says whiffs is
     ~200 replays spent to rediscover that nothing happened.
  2. **Comparison against a previous export** (`--compare`). §8.1 makes
     re-measurement an ACCEPTANCE CRITERION ("an independent re-run of a
     random sample of ≥5 rows reproduces them to the frame, from a cold
     start"), and §7 says what to do when it fails: "a number that fails
     re-measurement is DELETED, not averaged". So the comparison is
     row-by-row and its verdict is identical-or-not — this module never
     rewrites a stored value to match what it just measured, and never
     re-runs a disagreeing cell hoping for a better answer. It prints the
     disagreement.

Everything measured here is per-port DATA (`spec.FramelabSpec`, the profile's
`framelab` block): the anchor, the observables and their addressing, the
per-probe-shape calibrations, the rig's walk directions and the collision
floor at which `first_active_frame` is stored (§4.4 — "the stored row records
the gap it was measured at"). A port with no `framelab` block declines here
with `FramelabNotConfigured` before anything touches the emulator.

    python -m shadow_train.framelab.ladder \\
        --url http://127.0.0.1:4055/mcp --game library/mk2 \\
        --core ../FBNeo/src/burner/libretro/fbneo_libretro.dylib \\
        --rom ~/games/roms/mk2.zip \\
        --arena shadow/arenas/mk2/gap-60.state \\
        --arena shadow/arenas/mk2/gap-45.state \\
        --move HP --move HK --move 'HP:crouch' \\
        --calibrate-on HK:shadow/arenas/mk2/gap-45.state \\
        --cell HP:shadow/arenas/mk2/gap-60.state:close \\
        --compare library/mk2/arcade.frames.json \\
        --out /tmp/remeasure.json

Never point `--url` at port 4025 (CLAUDE.md: the user's session).
"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence, Tuple

from . import observables as obs
from .identity import compute_core_id, compute_rom_id
from .kit import (
    DEFAULT_MAX_SEARCH,
    CellMeasurement,
    ContactScan,
    MoveSpec,
    Rung,
    cell_rows,
    calibrate_shapes,
    make_rig,
    measure_cell,
    move_script,
    scan_contact,
)
from .probe import ProbeError
from .session import LabSession
from .spec import FramelabSpec

__all__ = [
    "COMPARED_COLUMNS",
    "connect_map",
    "connect_ranges",
    "compare_rows",
    "measure_ladder",
    "main",
]

# The columns a re-measurement is a re-measurement OF. Everything else in an
# exported row is provenance that is EXPECTED to differ between runs
# (`id`, `measured_at`, `sample_n`, `confidence`) or is a label the operator
# supplied (`variant`, `char`) rather than something the protocol measured.
# `core_id`/`rom_id` are compared because §6 is explicit that a number
# measured against different bytes is a different number — a disagreement
# there means the two runs are not comparable at all, which is the single
# most important thing a comparison can say.
COMPARED_COLUMNS = (
    "on_hit",
    "on_block",
    "damage",
    "hits",
    "knockdown",
    "first_active_frame",
    "connect_range",
    "gap_px",
    "gap_walk_frames",
    "input_latency_frames",
    "method",
    "observable",
    "rig_guard_state",
    "core_id",
    "rom_id",
)

_KEY_COLUMNS = ("char", "move", "variant", "gap_px", "observable")


def _key(row: Dict[str, Any]) -> Tuple:
    return tuple(row.get(c) for c in _KEY_COLUMNS)


# ── pass 1: the connect map ───────────────────────────────────────────────


def connect_map(
    session: LabSession,
    *,
    specs: Sequence[MoveSpec],
    rungs: Sequence[Rung],
    guard_buttons: Sequence[str],
    contact_read,
    quiet_frames: int,
    walk_directions_by_port,
    victim_y_read=None,
) -> Dict[Tuple[str, str], ContactScan]:
    """One anchor replay per (move, rung): did it reach, for how much, on
    what frame. `(move.label, rung.arena) -> ContactScan`.

    With `victim_y_read`, each CONNECTING cell also gets §1.1's knockdown
    probe — a second ~100-frame replay watching the victim's own `y` leave
    its resting value. That is the gate on whether an `on_hit` number may be
    reported at all, so it belongs to the map rather than to the sweep: a
    launcher's on-hit advantage is not a worse number, it is not a number.
    """
    out: Dict[Tuple[str, str], ContactScan] = {}
    for rung in rungs:
        rig = make_rig(
            rung.arena, guard_buttons=guard_buttons, quiet_frames=quiet_frames,
            walk_directions_by_port=walk_directions_by_port,
        )
        for spec in specs:
            scan = scan_contact(
                session, rig=rig, spec=spec, gap_px=rung.gap_px,
                contact_read=contact_read, defender_guard=False,
                victim_y_read=victim_y_read,
            )
            out[(spec.label, rung.arena)] = scan
    return out


def connect_ranges(
    cmap: Dict[Tuple[str, str], ContactScan]
) -> Dict[str, Optional[int]]:
    """`connect_range` per move: the LARGEST gap at which the contact signal
    fired. §5's bracket, not a measured edge — the real range is between this
    rung and the next one out, and the ladder's own step size is the
    resolution. NULL for a move that connected nowhere, never 0."""
    best: Dict[str, Optional[int]] = {}
    for (move, _), scan in cmap.items():
        if not scan.connected or scan.gap_px is None:
            best.setdefault(move, None)
            continue
        cur = best.get(move)
        best[move] = scan.gap_px if cur is None else max(cur, scan.gap_px)
    return best


# ── pass 2: the cells ─────────────────────────────────────────────────────


def measure_ladder(
    session: LabSession,
    *,
    cells: Sequence[Tuple[MoveSpec, Rung, Optional[str]]],
    guard_buttons: Sequence[str],
    contact_read,
    sample_fns,
    latencies,
    observables: Sequence[str],
    quiet_frames: int,
    walk_directions_by_port,
    ranges: Dict[str, Optional[int]],
    scans: Optional[Dict[Tuple[str, str], ContactScan]] = None,
    faf_at_px: Optional[int] = None,
    max_search: int = DEFAULT_MAX_SEARCH,
    repeats: int = 1,
    victim_y_read=None,
    on_progress=None,
) -> Tuple[List[CellMeasurement], List[str]]:
    """Measure every cell, returning the measurements and the REFUSALS.

    A cell that raises (`NonMonotoneError`, a capped sweep, a cross-method
    disagreement) is recorded as a refusal string and the run CONTINUES —
    §7's "no silent caps" reads both ways: one unmeasurable cell must not
    silently cost the other nine, and it must not silently vanish either.
    """
    out: List[CellMeasurement] = []
    refusals: List[str] = []
    for spec, rung, variant in cells:
        rig = make_rig(
            rung.arena, guard_buttons=guard_buttons, quiet_frames=quiet_frames,
            walk_directions_by_port=walk_directions_by_port,
        )
        pre = None
        if scans is not None and (spec.label, rung.arena) in scans:
            # Reuse the map's HIT scan; the block scan is a different run of
            # the game and `measure_cell` takes its own (see its docstring).
            pre = (scans[(spec.label, rung.arena)], None)
        try:
            m = measure_cell(
                session, rig=rig, spec=spec, rung=rung,
                contact_read=contact_read, sample_fns=sample_fns,
                latencies=latencies, observables=observables,
                max_search=max_search, variant=variant, scans=pre,
                victim_y_read=victim_y_read, repeats=repeats,
            )
        except ProbeError as exc:
            refusals.append(f"{spec.label}@{rung.gap_px}px [{variant}]: {exc}")
            if on_progress:
                on_progress(f"REFUSED {spec.label}@{rung.gap_px}px: {exc}")
            continue
        m.connect_range = ranges.get(spec.label)
        # §4.4: FAF is stored ONLY at the minimum reproducible gap, because at
        # larger gaps it is contaminated by travel. It is relative to the
        # MOVE'S OWN input frame, not to frame 0 of the replay — a crouching
        # normal's stance lead-in is setup, not startup.
        if (
            faf_at_px is not None
            and rung.gap_px == faf_at_px
            and m.scan_hit is not None
            and m.scan_hit.connected
            and m.scan_hit.contact_frame is not None
        ):
            m.first_active_frame = (
                m.scan_hit.contact_frame - move_script(spec).attack_input_frame
            )
        out.append(m)
        if on_progress:
            on_progress(_describe(m))
    return out, refusals


def _describe(m: CellMeasurement) -> str:
    bits = [f"{m.move.label}@{m.rung.gap_px}px [{m.variant}]"]
    for label, got in (("hit", m.on_hit), ("blk", m.on_block)):
        for o, meas in got.items():
            bits.append(
                f"{label}/{o}: att={meas.attacker.first_true}"
                f"->{meas.attacker.actionable_after_contact} "
                f"def={meas.defender.first_true}"
                f"->{meas.defender.actionable_after_contact}"
            )
    return "  ".join(bits)


# ── the comparison (§8.1 / §7) ────────────────────────────────────────────


def compare_rows(
    fresh: Sequence[Dict[str, Any]], stored: Sequence[Dict[str, Any]]
) -> Dict[str, Any]:
    """Row-by-row, keyed on (char, move, variant, gap_px, observable).

    Returns a verdict dict; it NEVER edits either side. Three ways to fail,
    all reported separately because they mean different things: a cell that
    disagrees (the protocol or the thing it measures changed), a cell that is
    missing from this run (it refused, or was not asked for), and a cell this
    run produced that was not stored before (new coverage, not a failure).
    """
    fresh_by = {_key(r): r for r in fresh}
    stored_by = {_key(r): r for r in stored}

    diffs: List[Dict[str, Any]] = []
    for k in sorted(stored_by.keys() & fresh_by.keys(), key=lambda t: [str(x) for x in t]):
        a, b = stored_by[k], fresh_by[k]
        cols = {
            c: {"stored": a.get(c), "fresh": b.get(c)}
            for c in COMPARED_COLUMNS
            if a.get(c) != b.get(c)
        }
        if cols:
            diffs.append({"key": dict(zip(_KEY_COLUMNS, k)), "columns": cols})

    missing = sorted(stored_by.keys() - fresh_by.keys(), key=lambda t: [str(x) for x in t])
    added = sorted(fresh_by.keys() - stored_by.keys(), key=lambda t: [str(x) for x in t])
    return {
        "identical": not diffs and not missing,
        "compared": len(stored_by.keys() & fresh_by.keys()),
        "differing": diffs,
        "missing_from_this_run": [dict(zip(_KEY_COLUMNS, k)) for k in missing],
        "new_in_this_run": [dict(zip(_KEY_COLUMNS, k)) for k in added],
    }


def _print_verdict(v: Dict[str, Any]) -> None:
    print()
    print("=" * 72)
    if v["identical"]:
        print(f"VERDICT: IDENTICAL — all {v['compared']} stored rows reproduced "
              "to the frame.")
    else:
        print(f"VERDICT: NOT IDENTICAL — {len(v['differing'])} of "
              f"{v['compared']} compared rows disagree.")
    for d in v["differing"]:
        k = d["key"]
        print(f"  DIFFERS {k['move']}/{k['variant']}@{k['gap_px']}px "
              f"[{k['observable']}]")
        for col, sides in sorted(d["columns"].items()):
            print(f"      {col}: stored={sides['stored']!r} "
                  f"fresh={sides['fresh']!r}")
    for k in v["missing_from_this_run"]:
        print(f"  MISSING {k['move']}/{k['variant']}@{k['gap_px']}px "
              f"[{k['observable']}] — stored, not produced by this run")
    for k in v["new_in_this_run"]:
        print(f"  NEW     {k['move']}/{k['variant']}@{k['gap_px']}px "
              f"[{k['observable']}] — measured here, not in the stored export")
    print("=" * 72)


# ── operator entry point ──────────────────────────────────────────────────


def _parse_move(raw: str, chords: Dict[str, Sequence[str]]) -> MoveSpec:
    """`HP` or `HP:crouch`. The buttons come from the profile's
    `attack_chords`, never from this file."""
    name, _, mod = raw.partition(":")
    if name not in chords:
        raise SystemExit(f"unknown move {name!r} (profile knows {sorted(chords)})")
    buttons = tuple(chords[name])
    if not mod:
        return MoveSpec(name=name, buttons=buttons)
    if mod != "crouch":
        raise SystemExit(f"unknown move modifier {mod!r} (only 'crouch')")
    return MoveSpec(
        name=name, buttons=buttons, stance="crouching", stance_button="down",
    )


def main() -> None:  # pragma: no cover - the live-rig path
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--url", default="http://127.0.0.1:4055/mcp")
    ap.add_argument("--game", default="library/mk2")
    ap.add_argument("--core", required=True)
    ap.add_argument("--rom", required=True)
    ap.add_argument("--char", default="reptile")
    ap.add_argument("--arena", action="append", default=[],
                    help="a ladder rung's .state (its .gap.json supplies the gap)")
    ap.add_argument("--move", action="append", default=[],
                    help="MOVE[:crouch] — repeatable; drives the connect map")
    ap.add_argument("--cell", action="append", default=[],
                    help="MOVE[:crouch]:ARENA:VARIANT — repeatable. VARIANT '-' "
                         "means NULL: use it when the connect map shows no "
                         "proximity boundary for that move, rather than "
                         "labelling one rung 'close' and inventing a variant "
                         "the damage signature does not support (§5).")
    ap.add_argument("--calibrate-on", default=None,
                    help="MOVE[:crouch]:ARENA to calibrate the four probe shapes on")
    ap.add_argument("--latencies", default=None,
                    help="JSON {shape: {observable: frames}} to use INSTEAD of "
                         "measuring them (only for a run that is explicitly "
                         "reusing a calibration)")
    ap.add_argument("--max-search", type=int, default=DEFAULT_MAX_SEARCH)
    ap.add_argument("--repeats", type=int, default=1,
                    help="evaluations per actionable(N); >1 requires them to "
                         "agree and raises otherwise (docs/frames.md §7)")
    ap.add_argument("--map-only", action="store_true",
                    help="connect map only; measure nothing")
    ap.add_argument("--compare", default=None,
                    help="a previous <port>.frames.json to compare against")
    ap.add_argument("--out", default=None, help="write this run's rows here")
    args = ap.parse_args()

    if ":4025" in args.url:
        raise SystemExit("refusing port 4025 — that is the user's live session "
                         "(CLAUDE.md). Use 4055+.")

    from shadow_train import profile as game_profile
    from shadow_train.mcpclient import McpClient

    prof = game_profile.load(args.game)
    flspec = FramelabSpec.from_profile(prof)
    observable_names = flspec.default_observable_names()
    f1 = obs.resolve_fighter(prof, "block1", 0)
    f2 = obs.resolve_fighter(prof, "block2", 1)
    sample_fns = {0: obs.make_sampler(f1, flspec), 1: obs.make_sampler(f2, flspec)}
    contact_read = obs.make_contact_read_from_spec(f2, flspec)   # defender = victim
    victim_y_read = obs.make_pointer_field_read(f2, "y")
    guard = tuple(prof.attack_chords["Block"])
    wdbp = flspec.rig.walk_directions_by_port if flspec.rig else None
    quiet = flspec.quiet_frames
    floor_px = flspec.spacing.collision_floor_px if flspec.spacing else None

    session = LabSession(
        McpClient(args.url), verify_fn=obs.make_arena_verifier(prof, expect={})
    )
    session.enforce_preconditions()

    specs = [_parse_move(m, prof.attack_chords) for m in args.move]
    rungs = [Rung.from_sidecar(a) for a in args.arena]
    started = time.monotonic()

    cmap: Dict[Tuple[str, str], ContactScan] = {}
    if specs and rungs:
        print(f"connect map: {len(specs)} moves x {len(rungs)} rungs")
        cmap = connect_map(
            session, specs=specs, rungs=rungs, guard_buttons=guard,
            contact_read=contact_read, quiet_frames=quiet,
            walk_directions_by_port=wdbp, victim_y_read=victim_y_read,
        )
        for rung in rungs:
            row = [f"{rung.gap_px:>4}px"]
            for spec in specs:
                s = cmap[(spec.label, rung.arena)]
                row.append(
                    f"{spec.label}={s.damage}@f{s.contact_frame}"
                    + ("[KD]" if s.knockdown else "")
                    if s.connected else f"{spec.label}=—"
                )
            print("  " + "  ".join(row))
    ranges = connect_ranges(cmap)
    if ranges:
        print("connect_range:", json.dumps(ranges, sort_keys=True))

    if args.map_only:
        print(f"steps={session.steps_taken} loads={session.loads_done} "
              f"elapsed={time.monotonic() - started:.1f}s")
        return

    latencies: Dict[str, Dict[str, int]] = {}
    if args.latencies:
        latencies = json.loads(args.latencies)
        print("calibration (SUPPLIED, not measured):",
              json.dumps(latencies, sort_keys=True))
    elif args.calibrate_on:
        raw_move, _, arena = args.calibrate_on.rpartition(":")
        spec = _parse_move(raw_move, prof.attack_chords)
        rig = make_rig(arena, guard_buttons=guard, quiet_frames=quiet,
                       walk_directions_by_port=wdbp)
        scan = scan_contact(session, rig=rig, spec=spec, gap_px=None,
                            contact_read=contact_read, defender_guard=False)
        if not scan.connected:
            raise SystemExit(f"cannot calibrate on {args.calibrate_on}: it whiffs")
        assert scan.anchor is not None
        latencies = calibrate_shapes(
            session, rig=rig, spec=spec, anchor=scan.anchor.contact_frame,
            sample_fns=sample_fns, observables=observable_names,
        )
        print("calibration:", json.dumps(latencies, sort_keys=True))
    else:
        raise SystemExit("need --calibrate-on or --latencies to measure cells")

    cells = []
    for raw in args.cell:
        parts = raw.split(":")
        variant = None if parts[-1] == "-" else parts[-1]
        arena = parts[-2]
        spec = _parse_move(":".join(parts[:-2]), prof.attack_chords)
        cells.append((spec, Rung.from_sidecar(arena), variant))

    ms, refusals = measure_ladder(
        session, cells=cells, guard_buttons=guard, contact_read=contact_read,
        sample_fns=sample_fns, latencies=latencies,
        observables=observable_names, quiet_frames=quiet,
        walk_directions_by_port=wdbp, ranges=ranges, scans=cmap or None,
        faf_at_px=floor_px, max_search=args.max_search, repeats=args.repeats,
        victim_y_read=victim_y_read, on_progress=print,
    )

    core_id, rom_id = compute_core_id(args.core), compute_rom_id(args.rom)
    rows: List[dict] = []
    for m in ms:
        for note in m.notes:
            print("NOTE:", note)
        rows += cell_rows(
            m, family=prof.family, port=prof.port, char=args.char,
            core_id=core_id, rom_id=rom_id, observables=observable_names,
        )

    elapsed = time.monotonic() - started
    print(f"\nmeasured {len(rows)} rows from {len(ms)} cells "
          f"({len(refusals)} refusals)")
    for r in refusals:
        print("  REFUSAL:", r)
    print(f"steps={session.steps_taken} loads={session.loads_done} "
          f"step_calls={session.step_calls} batch_calls={session.batch_calls} "
          f"frames_batched={session.frames_batched} elapsed={elapsed:.1f}s")

    if args.out:
        Path(args.out).write_text(json.dumps(
            {"rows": rows, "refusals": refusals, "latencies": latencies,
             "steps": session.steps_taken, "loads": session.loads_done,
             "elapsed_s": elapsed}, indent=2, sort_keys=True) + "\n")

    if args.compare:
        stored = json.loads(Path(args.compare).read_text())["moves"]
        verdict = compare_rows(rows, stored)
        _print_verdict(verdict)
        if not verdict["identical"]:
            raise SystemExit(1)


if __name__ == "__main__":  # pragma: no cover
    main()
