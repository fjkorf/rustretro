"""The punish rig — `docs/frames.md` §8.3, and the only measurement in this
lab that uses NO act-again probe at all.

Everything else here measures "when can this fighter start a WALK", and the
whole table is a difference of two such numbers. That is a convention, and a
convention can be wrong in a way no amount of internal agreement will show:
the on-block column was published 9 frames too generous, both observables
agreed on it, every predicate was monotone, and a fresh process reproduced it
exactly. What found the error was a rig with a different readout — sweep the
DEFENDER's counter-attack frame and ask the ATTACKER's damage register what
happened:

    full damage : the counter landed — the defender was free
    chip only   : it landed on a raised guard
    nothing     : no contact — the defender was still stunned (or out of range)

So this module answers the question the table is FOR, in the game's own
terms, and §8.3 states the acceptance test in exactly those terms: "take the
most unsafe move in the table, block it, punish it with the fastest normal
the table says reaches: it must connect. Take one the table calls safe by ≥3
frames: the same punish must NOT connect."

The counter is issued through `probe.replay`'s probe slot, which REPLACES the
defender's held set from that frame on. The counter is held for the rest of
the replay rather than tapped: a `press` that can evaporate is banned in this
lab (§3.3), and a hold's press EDGE is what the game reads.

## The guard must be released BEFORE the counter frame, and that is measured

Dropping Block and pressing the counter on the SAME frame produces **no
attack at all** — not a late one, none. Measured on Mileena's blocked cHK at
61 px: with the defender's guard held until the counter frame, HP, HK, LK and
LP all produce zero contact at every counter frame from contact+8 to
contact+30, and the defender's `action_counter` (`block2+0xC0`) never leaves
its blocking value, so the BUTTON never registered as an attack. Release the
guard at contact+1 and the identical sweep lands from contact+24 on.

That is the same shape as the special-move rule in this file's sibling
evidence ("a direction chorded with the trigger button on the same frame does
not register"), and it is the punish-rig face of the block-stance drop that
`docs/frames.md` §3.1 already measured as a 10/11-frame probe latency: MK2's
block stance does not end when the button does. A punish rig that ignores it
reports EVERY move as unpunishable, which is exactly the kind of clean,
plausible, false answer §7 exists to catch.

`guard_release_lead` therefore defaults to releasing the guard one frame after
CONTACT — constant across the sweep, so the rig does not vary with n, and
inside blockstun, where the defender could not act anyway.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Sequence

from .probe import MoveScript, Rig, ScriptStep, replay
from .kit import MoveSpec, move_script
from .session import LabSession

__all__ = ["PunishSweep", "punish_at", "punish_sweep", "main"]


@dataclass
class PunishSweep:
    """One (move, gap, guard-state) cell's counter-attack sweep. `damage[n]`
    is what the ATTACKER lost when the defender's counter was pressed at
    contact+n — `None`, never 0, when nothing connected."""

    move: str
    gap_px: Optional[int]
    counter: str
    rig_guard_state: str
    contact_frame: int
    damage: Dict[int, Optional[int]] = field(default_factory=dict)

    @property
    def first_landing(self) -> Optional[int]:
        """The smallest n whose counter connected AND stayed connecting for
        the rest of the sweep is NOT what this returns — that would hide a
        gap. This returns the smallest connecting n, and `holes` reports any
        non-connecting n above it, because on the far-HK rig Reptile's punish
        window measurably CLOSED again from contact+25 and nothing in the
        model explains it. A rig that silently smoothed that over would have
        deleted the finding."""
        hits = [n for n, d in sorted(self.damage.items()) if d]
        return hits[0] if hits else None

    @property
    def holes(self) -> List[int]:
        first = self.first_landing
        if first is None:
            return []
        return [n for n, d in sorted(self.damage.items()) if n > first and not d]


def punish_at(
    session: LabSession,
    *,
    rig: Rig,
    spec: MoveSpec,
    counter: MoveSpec,
    contact_frame: int,
    n: int,
    attacker_health_read,
    defender_guard: bool = True,
    tail_frames: int = 40,
    attacker_guards_after: bool = True,
    guard_release_at: Optional[int] = None,
) -> Optional[int]:
    """One replay: the defender presses `counter` at `contact_frame + n`.
    Returns the damage the ATTACKER took, or None if nothing connected.

    `tail_frames` must outlast the counter's own startup at this spacing, or
    a landing punish reads as a whiff. It is checked against nothing — the
    caller sizes it, and the sweep's shape (a clean F…F T…T boundary) is what
    says it was sized right.

    `attacker_guards_after` appends a Block hold to the ATTACKER's script for
    the rest of the replay, and it is what makes the result mean "safe" or
    "unsafe" rather than merely "eventually hittable". A recovering fighter
    who holds Block from the frame she threw the move blocks the instant she
    is able to, so the three damage outcomes separate cleanly: full damage =
    the counter beat her recovery, chip = her guard was up first (the move
    was SAFE), nothing = the counter did not reach. Without it, every move in
    the table looks punishable if you wait long enough, because nobody is
    guarding — which is the trivial answer to a question nobody asked.
    """
    total = contact_frame + n + tail_frames
    script = move_script(spec)
    if attacker_guards_after:
        script = MoveScript(
            name=script.name,
            steps=tuple(script.steps)
            + (ScriptStep(frames=total + 1, buttons=tuple(rig.guard_buttons)),),
            lead_in=script.lead_in,
        )
    trace = replay(
        session, rig=rig, script=script, total_frames=total,
        defender_guard=defender_guard,
        guard_release_at=guard_release_at,
        probe_port=rig.defender_port, probe_buttons=tuple(counter.buttons),
        probe_at=contact_frame + n,
        sample_fn=lambda s: {"h": attacker_health_read(s)},
        sample_from=contact_frame + n,
    )
    seen = [t["h"] for t in trace if t is not None and t.get("h") is not None]
    if len(seen) < 2:
        return None
    lost = seen[0] - min(seen)
    return lost if lost > 0 else None


def punish_sweep(
    session: LabSession,
    *,
    rig: Rig,
    spec: MoveSpec,
    counter: MoveSpec,
    contact_frame: int,
    ns: Sequence[int],
    attacker_health_read,
    defender_guard: bool = True,
    tail_frames: int = 40,
    gap_px: Optional[int] = None,
    attacker_guards_after: bool = True,
    guard_release_lead: Optional[int] = 1,
) -> PunishSweep:
    """`guard_release_lead` is frames AFTER contact at which the defender's
    Block is released, constant for the whole sweep (see the module
    docstring: a counter chorded with the guard release does not come out at
    all). `None` keeps the guard held until the counter frame, which is the
    configuration that measures that finding rather than working around it."""
    out = PunishSweep(
        move=spec.label, gap_px=gap_px, counter=counter.label,
        rig_guard_state="held" if defender_guard else "none",
        contact_frame=contact_frame,
    )
    for n in ns:
        out.damage[n] = punish_at(
            session, rig=rig, spec=spec, counter=counter,
            contact_frame=contact_frame, n=n,
            attacker_health_read=attacker_health_read,
            defender_guard=defender_guard, tail_frames=tail_frames,
            attacker_guards_after=attacker_guards_after,
            guard_release_at=(
                None if guard_release_lead is None or not defender_guard
                else contact_frame + guard_release_lead
            ),
        )
    return out


def main() -> None:  # pragma: no cover - the live-rig path
    """Throw the punish the table predicts.

        python -m shadow_train.framelab.punish \\
            --url http://127.0.0.1:4067/mcp --game library/mk2 \\
            --arena shadow/arenas/mk2/m-gap-30.state \\
            --move HK --counter LK --from 14 --to 30

    Never point `--url` at port 4025 (CLAUDE.md: the user's session).
    """
    import argparse
    import json
    import time
    from pathlib import Path

    from shadow_train import profile as game_profile
    from shadow_train.mcpclient import McpClient

    from . import observables as obs
    from .kit import Rung, make_rig, scan_contact
    from .ladder import _parse_move
    from .spec import FramelabSpec

    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--url", default="http://127.0.0.1:4067/mcp")
    ap.add_argument("--game", default="library/mk2")
    ap.add_argument("--arena", required=True)
    ap.add_argument("--move", required=True, help="MOVE[:crouch] — the move being punished")
    ap.add_argument("--counter", required=True, help="MOVE — the counter-attack")
    ap.add_argument("--from", dest="lo", type=int, required=True)
    ap.add_argument("--to", dest="hi", type=int, required=True)
    ap.add_argument("--tail-frames", type=int, default=40)
    ap.add_argument("--guard-release-lead", type=int, default=1,
                    help="frames after contact at which the defender drops "
                         "Block; -1 keeps it held to the counter frame (which "
                         "measures the no-attack-comes-out finding)")
    ap.add_argument("--no-attacker-guard", action="store_true",
                    help="do NOT have the attacker hold Block after the move")
    ap.add_argument("--on-hit", action="store_true",
                    help="sweep the ON-HIT rig (defender holds no guard)")
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    if ":4025" in args.url:
        raise SystemExit("refusing port 4025 — that is the user's live session.")

    prof = game_profile.load(args.game)
    flspec = FramelabSpec.from_profile(prof)
    f1 = obs.resolve_fighter(prof, "block1", 0)
    f2 = obs.resolve_fighter(prof, "block2", 1)
    victim_read = obs.make_contact_read_from_spec(f2, flspec)     # defender
    attacker_read = obs.make_contact_read_from_spec(f1, flspec)   # attacker
    guard = tuple(prof.attack_chords["Block"])
    wdbp = flspec.rig.walk_directions_by_port if flspec.rig else None

    session = LabSession(
        McpClient(args.url), verify_fn=obs.make_arena_verifier(prof, expect={})
    )
    session.enforce_preconditions()
    started = time.monotonic()

    rung = Rung.from_sidecar(args.arena)
    rig = make_rig(args.arena, guard_buttons=guard, quiet_frames=flspec.quiet_frames,
                   walk_directions_by_port=wdbp)
    spec = _parse_move(args.move, prof.attack_chords)
    counter = _parse_move(args.counter, prof.attack_chords)

    guarded = not args.on_hit
    scan = scan_contact(session, rig=rig, spec=spec, gap_px=rung.gap_px,
                        contact_read=victim_read, defender_guard=guarded)
    if not scan.connected or scan.contact_frame is None:
        raise SystemExit(f"{spec.label} does not connect at {rung.gap_px}px")
    print(f"anchor: contact f{scan.contact_frame}, "
          f"{'chip' if guarded else 'damage'} {scan.damage}")

    sweep = punish_sweep(
        session, rig=rig, spec=spec, counter=counter,
        contact_frame=scan.contact_frame, ns=range(args.lo, args.hi + 1),
        attacker_health_read=attacker_read, defender_guard=guarded,
        tail_frames=args.tail_frames, gap_px=rung.gap_px,
        attacker_guards_after=not args.no_attacker_guard,
        guard_release_lead=(None if args.guard_release_lead < 0
                            else args.guard_release_lead),
    )
    for n, d in sorted(sweep.damage.items()):
        print(f"  counter at contact+{n:>3}: "
              + (f"attacker took {d}" if d else "nothing"))
    print(f"first landing: {sweep.first_landing!r}  holes above it: {sweep.holes}")
    print(f"steps={session.steps_taken} loads={session.loads_done} "
          f"elapsed={time.monotonic() - started:.1f}s")
    if args.out:
        Path(args.out).write_text(json.dumps({
            "move": sweep.move, "counter": sweep.counter, "gap_px": sweep.gap_px,
            "rig_guard_state": sweep.rig_guard_state,
            "contact_frame": sweep.contact_frame,
            "damage": {str(k): v for k, v in sweep.damage.items()},
            "first_landing": sweep.first_landing, "holes": sweep.holes,
        }, indent=2) + "\n")


if __name__ == "__main__":  # pragma: no cover
    main()
