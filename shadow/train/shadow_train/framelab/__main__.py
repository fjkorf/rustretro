"""CLI: python -m shadow_train.framelab demo <subcommand> ...

  demo compile <beats.json> [--game library/mk2] [--out schedule.json]
      Load a beat sheet, resolve its moves via the profile's attack_chords,
      compile it (`choreo.compile`), and print the beat spans + total frame
      count. No emulator touched.

  demo execute <beats.json> --url URL [--game library/mk2] [--out trace.json]
      compile() + execute() against a live headless MCP session, sampling
      each port's struct health + contact signal via the profile's own
      framelab observables (never hardcoded — CLAUDE.md). NEVER point --url
      at port 4025 (the user's live session); this refuses it outright.

  demo verify <beats.json> --url URL [--game library/mk2]
      compile + execute + verify() in one shot; prints each beat's checks
      and exits non-zero if any beat's CHECKED expectations failed.

  demo export <beats.json> --name SLOT_NAME [--total-frames N]
      compile() + to_slot() + save to shadow/inputs/<family>/<name>.slot.json
      — no emulator needed, the compiled schedule alone is enough to pack
      the masks.

Every subcommand needs `--game` only to resolve `attack_chords` for move
names in the beat sheet ("HP", "Block", ...) — it never reads a per-game
fact any other way.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def _load_sheet(args):
    from shadow_train import profile as game_profile

    from .choreo import load_beat_sheet

    prof = game_profile.load(args.game)
    sheet = load_beat_sheet(args.beats, attack_chords=prof.attack_chords)
    return prof, sheet


def cmd_compile(args) -> None:
    from .choreo import compile as choreo_compile

    _, sheet = _load_sheet(args)
    sched = choreo_compile(sheet.beats)
    print(f"{sheet.name}: {len(sheet.beats)} beat(s), {sched.total_frames} frames total")
    for name, (start, end) in sched.beat_spans.items():
        print(f"  {name:<28} frames [{start}, {end})")
    if args.out:
        Path(args.out).write_text(
            json.dumps(
                {
                    "name": sheet.name,
                    "total_frames": sched.total_frames,
                    "beat_spans": sched.beat_spans,
                    "frames": {
                        str(f): {str(p): list(h) for p, h in ports.items()}
                        for f, ports in sorted(sched.frames.items())
                    },
                },
                indent=2,
            )
            + "\n"
        )
        print(f"wrote {args.out}")


def cmd_export(args) -> None:
    from .choreo import compile as choreo_compile
    from .choreo import save_slot, to_slot

    _, sheet = _load_sheet(args)
    sched = choreo_compile(sheet.beats)
    slot = to_slot(sched, args.total_frames, family=sheet.family, port=sheet.port)
    path = save_slot(slot, args.name)
    print(f"wrote {path} ({len(slot['frames'])} frames)")


def _build_session(args):
    if ":4025" in args.url:
        raise SystemExit("refusing port 4025 — that is the user's live session (CLAUDE.md).")
    from shadow_train.mcpclient import McpClient

    from . import observables as obs
    from .session import LabSession
    from .spec import FramelabSpec

    prof, sheet = _load_sheet(args)
    flspec = FramelabSpec.from_profile(prof)
    f1 = obs.resolve_fighter(prof, "block1", 0)
    f2 = obs.resolve_fighter(prof, "block2", 1)
    # The profile's `framelab.anchor` register (struct health on MK2 arcade,
    # docs/frames.md §4.1's corrected contact signal) IS the damage register
    # — one read per port serves both `verify()`'s "contact" edge-detection
    # and its "health" delta checks, since they are the same byte.
    contact1 = obs.make_contact_read_from_spec(f1, flspec)
    contact2 = obs.make_contact_read_from_spec(f2, flspec)

    def sample_fn(session):
        h0, h1 = contact1(session), contact2(session)
        return {"health": {0: h0, 1: h1}, "contact": {0: h0, 1: h1}}

    session = LabSession(McpClient(args.url), verify_fn=obs.make_arena_verifier(prof, expect={}))
    session.enforce_preconditions()
    return session, sheet, sample_fn


def _compile_and_execute(args):
    from .choreo import compile as choreo_compile
    from .choreo import execute as choreo_execute

    session, sheet, sample_fn = _build_session(args)
    sched = choreo_compile(sheet.beats)
    trace = choreo_execute(session, sheet.arena, sched, sample_fn=sample_fn)
    print(f"executed {sched.total_frames} frames from {sheet.arena} — steps_taken={session.steps_taken}")
    if args.out:
        Path(args.out).write_text(
            json.dumps(list(trace.frames), default=str, indent=2) + "\n"
        )
        print(f"wrote {args.out}")
    return trace, sched


def cmd_execute(args) -> None:
    _compile_and_execute(args)


def cmd_verify(args) -> None:
    from .choreo import verify as choreo_verify

    trace, sched = _compile_and_execute(args)
    report = choreo_verify(trace, sched.beats)
    ok = True
    for r in report.results:
        status = "PASS" if r.passed else "REFUSED"
        print(f"[{status}] {r.name}: {r.checks}")
        for note in r.notes:
            print(f"    {note}")
        ok = ok and r.passed
    if not ok:
        sys.exit(1)


def main() -> None:
    ap = argparse.ArgumentParser(prog="python -m shadow_train.framelab")
    sub = ap.add_subparsers(dest="cmd", required=True)

    demo = sub.add_parser("demo", help="choreo.py: compile/execute/verify/export a beat sheet")
    dsub = demo.add_subparsers(dest="demo_cmd", required=True)

    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("beats", help="path to a .beats.json file")
    common.add_argument("--game", default="library/mk2")

    p = dsub.add_parser("compile", parents=[common])
    p.add_argument("--out", default=None)
    p.set_defaults(func=cmd_compile)

    p = dsub.add_parser("execute", parents=[common])
    p.add_argument("--url", default="http://127.0.0.1:4026/mcp")
    p.add_argument("--out", default=None)
    p.set_defaults(func=cmd_execute)

    p = dsub.add_parser("verify", parents=[common])
    p.add_argument("--url", default="http://127.0.0.1:4026/mcp")
    p.add_argument("--out", default=None)
    p.set_defaults(func=cmd_verify)

    p = dsub.add_parser("export", parents=[common])
    p.add_argument("--name", required=True)
    p.add_argument("--total-frames", type=int, default=None)
    p.set_defaults(func=cmd_export)

    args = ap.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
