"""`guard_height` — which guard STANCE stops a move, measured rather than
looked up.

`docs/frames.md` §6 reserves a `guard_height` column and §12 records that it
is NULL in every row anyone has measured, with the reason: "it needs a
CROUCHING defender rig, which was not built". This module is that rig, and it
is three replays wide.

The measurement is a DAMAGE signature, read off the same struct-health anchor
(`block+0x0E`) everything else in the lab anchors on, under three defender
stances that differ only in what the defender's port holds from frame 0:

    open      : nothing held           -> full damage if it connects
    standing  : Block held             -> chip if that stance stops it
    crouching : Block + down held      -> chip if that stance stops it

`classify_guard` reads the three numbers, and its vocabulary is deliberately
larger than "high/mid/low" because the rig can distinguish an outcome those
three words cannot:

| verdict | what was measured |
|---|---|
| `mid` | both stances chip |
| `low` | standing takes FULL damage, crouching chips |
| `overhead` | crouching takes FULL damage, standing chips |
| `unblockable` | both take full damage — on MK2 that is a THROW |
| `high` | the move WHIFFS entirely against the crouching stance |
| `whiffs_vs_guard` | it reaches an OPEN defender but not a standing GUARDING one |
| NULL | it did not connect against an open defender: nothing to classify |

`whiffs_vs_guard` is not a guard height at all, and that is the point. Live on
Mileena's far HP at 83 px: 11 damage against an open defender, chip against a
crouch-blocking one, and **no contact whatsoever** against a standing
blocking one. MK2's standing block stance leans the fighter back, so at the
outer edge of a move's range the same input reaches an idle opponent and
misses a blocking one — the connect map is guard-state-dependent there. A
classifier that read "standing block took no damage" as "standing block
stopped it" would have called that move `low`, which is exactly backwards.

`high` is the case a damage-only reading would get wrong. A move that sails
over a ducking opponent produces NO contact at all, and "no contact" is not
"blocked" — §1.1 makes a whiff an outcome in its own right. Reporting it as
`mid` because "the defender took no damage" would be the same class of error
as inferring block from a health delta, which §2.6 already forbids for the
same reason.

Chip is not assumed to be any particular fraction: the test is `damage <
open_damage`, because the only claim being made is that the stance CHANGED
the outcome. MK2 arcade's measured chip is a quarter of the hit (a 24-damage
close HP chips 6), but that ratio is a fact about this game, not a definition.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Dict, Mapping, Optional, Sequence

from .kit import ContactScan, MoveSpec, Rung, make_rig, scan_contact
from .session import LabSession

__all__ = [
    "GuardMeasurement",
    "classify_guard",
    "measure_guard_height",
    "main",
]


@dataclass(frozen=True)
class GuardMeasurement:
    """The three replays and the verdict they support. Every damage is
    Optional: `None` means the move did not connect against that stance,
    which is information (see `high`), never 0."""

    move: str
    gap_px: Optional[int]
    damage_open: Optional[int]
    damage_standing: Optional[int]
    damage_crouching: Optional[int]
    contact_open: Optional[int]
    contact_standing: Optional[int]
    contact_crouching: Optional[int]
    verdict: Optional[str]
    note: str = ""

    def as_dict(self) -> Dict[str, Any]:
        return dict(self.__dict__)


def classify_guard(
    damage_open: Optional[int],
    damage_standing: Optional[int],
    damage_crouching: Optional[int],
) -> Optional[str]:
    """The pure half: three damage readings in, one verdict out.

    Returns None when the move did not connect against an OPEN defender —
    there is no guard question to answer about a move that does not reach.
    """
    if damage_open is None:
        return None
    if damage_standing is None:
        # Reaches an open defender, misses a standing guarding one: a
        # GEOMETRY result, not a guard height (see the module docstring).
        return "whiffs_vs_guard"
    if damage_crouching is None:
        # It reached a standing defender and vanished against a ducking one.
        return "high"
    stopped_standing = damage_standing is not None and damage_standing < damage_open
    stopped_crouching = damage_crouching < damage_open
    if stopped_standing and stopped_crouching:
        return "mid"
    if stopped_crouching and not stopped_standing:
        return "low"
    if stopped_standing and not stopped_crouching:
        return "overhead"
    return "unblockable"


def measure_guard_height(
    session: LabSession,
    *,
    spec: MoveSpec,
    rung: Rung,
    contact_read,
    guard_buttons: Sequence[str],
    crouch_buttons: Sequence[str] = ("down",),
    quiet_frames: int = 20,
    walk_directions_by_port: Optional[Mapping[int, Sequence[str]]] = None,
    anchor_frames: int = 48,
    scan_open: Optional[ContactScan] = None,
) -> GuardMeasurement:
    """Three anchor replays at one (move, gap): open, standing-guard,
    crouching-guard. `scan_open` may be reused from an existing connect map —
    it is exactly the same replay.

    The crouching stance is expressed as a rig whose `guard_buttons` are
    Block AND down, because `probe.replay` asserts `rig.guard_buttons` on the
    defender's port from frame 0 and holds them for the whole replay; that is
    what "the defender is crouch-blocking" means to this protocol.
    """
    stand_rig = make_rig(
        rung.arena, guard_buttons=tuple(guard_buttons), quiet_frames=quiet_frames,
        walk_directions_by_port=walk_directions_by_port,
    )
    crouch_rig = make_rig(
        rung.arena, guard_buttons=tuple(guard_buttons) + tuple(crouch_buttons),
        quiet_frames=quiet_frames, walk_directions_by_port=walk_directions_by_port,
    )

    def scan(rig, guard: bool) -> ContactScan:
        return scan_contact(
            session, rig=rig, spec=spec, gap_px=rung.gap_px,
            contact_read=contact_read, defender_guard=guard,
            anchor_frames=anchor_frames,
        )

    s_open = scan_open if scan_open is not None else scan(stand_rig, False)
    s_stand = scan(stand_rig, True)
    s_crouch = scan(crouch_rig, True)

    def dmg(s: ContactScan) -> Optional[int]:
        return s.damage if s.connected else None

    def cf(s: ContactScan) -> Optional[int]:
        return s.contact_frame if s.connected else None

    verdict = classify_guard(dmg(s_open), dmg(s_stand), dmg(s_crouch))
    note = ""
    if verdict is None:
        note = "no contact against an open defender at this gap (§1.1: a whiff)"
    return GuardMeasurement(
        move=spec.label, gap_px=rung.gap_px,
        damage_open=dmg(s_open), damage_standing=dmg(s_stand),
        damage_crouching=dmg(s_crouch),
        contact_open=cf(s_open), contact_standing=cf(s_stand),
        contact_crouching=cf(s_crouch),
        verdict=verdict, note=note,
    )


def main() -> None:  # pragma: no cover - the live-rig path
    """Fill the `guard_height` column for a set of (move, arena) pairs.

        python -m shadow_train.framelab.guard \\
            --url http://127.0.0.1:4066/mcp --game library/mk2 \\
            --cell HP:shadow/arenas/mk2/m-gap-45.state \\
            --cell LK:crouch:shadow/arenas/mk2/m-gap-45.state

    Never point `--url` at port 4025 (CLAUDE.md: the user's session).
    """
    import argparse
    import json
    import time
    from pathlib import Path

    from shadow_train import profile as game_profile
    from shadow_train.mcpclient import McpClient

    from . import observables as obs
    from .ladder import _parse_move
    from .spec import FramelabSpec

    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--url", default="http://127.0.0.1:4066/mcp")
    ap.add_argument("--game", default="library/mk2")
    ap.add_argument("--cell", action="append", default=[],
                    help="MOVE[:crouch]:ARENA — repeatable")
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    if ":4025" in args.url:
        raise SystemExit("refusing port 4025 — that is the user's live session.")

    prof = game_profile.load(args.game)
    flspec = FramelabSpec.from_profile(prof)
    f2 = obs.resolve_fighter(prof, "block2", 1)
    contact_read = obs.make_contact_read_from_spec(f2, flspec)
    guard = tuple(prof.attack_chords["Block"])
    wdbp = flspec.rig.walk_directions_by_port if flspec.rig else None

    session = LabSession(
        McpClient(args.url), verify_fn=obs.make_arena_verifier(prof, expect={})
    )
    session.enforce_preconditions()
    started = time.monotonic()

    out = []
    for raw in args.cell:
        parts = raw.split(":")
        spec = _parse_move(":".join(parts[:-1]), prof.attack_chords)
        rung = Rung.from_sidecar(parts[-1])
        g = measure_guard_height(
            session, spec=spec, rung=rung, contact_read=contact_read,
            guard_buttons=guard, quiet_frames=flspec.quiet_frames,
            walk_directions_by_port=wdbp,
        )
        print(f"{g.move:>4} @ {g.gap_px}px  open={g.damage_open!r} "
              f"stand={g.damage_standing!r} crouch={g.damage_crouching!r} "
              f"contact={g.contact_open!r}/{g.contact_standing!r}/"
              f"{g.contact_crouching!r}  -> {g.verdict!r} {g.note}")
        out.append(g.as_dict())

    print(f"steps={session.steps_taken} loads={session.loads_done} "
          f"elapsed={time.monotonic() - started:.1f}s")
    if args.out:
        Path(args.out).write_text(json.dumps(out, indent=2) + "\n")


if __name__ == "__main__":  # pragma: no cover
    main()
