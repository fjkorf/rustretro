#!/usr/bin/env python3
"""Behavioural acceptance runs for library/mk2/confirm_bot.lua.

WHY THIS IS NOT A PYTEST TEST
-----------------------------
It needs the real ROM (`~/games/roms/mk2.zip`), the from-source FBNeo core, and
a live emulator process — none of which exist in CI.  It is deliberately named
`validate_*` (not `test_*`) so a bare `pytest` from the repo root never collects
it.  Run it by hand when the bot changes.

WHAT IT MEASURES
----------------
1. THE CONFIRM TEST (differential).  Two families of runs from identical
   arenas, differing in exactly ONE setting: the training dummy's guard mode
   (`training.set_guard("all")` vs `("none")`) with the dummy mode pinned to
   plain `block` so nothing else about P2 differs — a pure Block dummy never
   drops guard for a punish macro, so the guard mode is the only variable.
   The observed discriminator is the jab's struct-health DELTA AMOUNT: 11
   (`mileena/HP/far` hit, rows 21/22) vs 3 (a quarter, the blocked chip).  The
   assertion is that the bot commits its −20 move (far HK) in the HIT family
   and never in the BLOCKED family.

2. SAFETY UNDER PUNISHMENT.  BlockPunish dummy, `set_reversal("fast")`,
   sustained fights.  Counts every full-damage hit the bot took while not
   guarding, and separately how many of those landed inside a genuine punish
   window (within |on_block| frames of one of the bot's own blocked moves
   becoming actionable — the only interval in which "punish" is the right
   word for what happened).

3. TURN ALTERNATION.  A timeline built from contact events only (the bot
   dealing damage/chip vs the bot taking it), plus the longest stretch in
   which NEITHER side made contact — the deadlock check.

USAGE
    source shadow/train/.venv/bin/activate
    python shadow/demos/mk2/validate_confirm_bot.py            # launches its own emu
    python shadow/demos/mk2/validate_confirm_bot.py --port 4026 --attach
    python shadow/demos/mk2/validate_confirm_bot.py --record   # + a v3 recording

NEVER point --port at 4025 (the user's live session); the default is 4026.
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import subprocess
import sys
import time
from dataclasses import dataclass, field

REPO = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / "shadow" / "train"))

from shadow_train.mcpclient import McpClient  # noqa: E402

CORE = REPO.parent / "FBNeo/src/burner/libretro/fbneo_libretro.dylib"
ROM = pathlib.Path(os.path.expanduser("~/games/roms/mk2.zip"))
SCRIPT = REPO / "library/mk2/confirm_bot.lua"
ARENAS = [f"shadow/arenas/mk2/m-gap-{k}.state" for k in (30, 35, 39, 45)]

# The dummy's punish pool for test 2.  `slide` is the profile's live-verified
# Reptile special; close `HP` is the fastest thing he owns (contact at press+8,
# mk2.md).  Both are what a human would actually try.
PUNISH_POOL = "{{weight=3, move='slide'},{weight=2, attack='HP'}}"


# ───────────────────────────── session plumbing ─────────────────────────────


class Session:
    def __init__(self, port: int, attach: bool, record: str | None = None):
        self.port = port
        self.proc = None
        if not attach:
            self.proc = self._launch(record)
        self.c = McpClient(f"http://127.0.0.1:{port}/mcp")
        self.c.call("enable_writes")

    def _launch(self, record: str | None):
        for p in (CORE, ROM, SCRIPT):
            if not p.exists():
                sys.exit(f"missing: {p}")
        cmd = [
            str(REPO / "target/release-dev/rustretro"),
            "--core", str(CORE), "--rom", str(ROM),
            "--game", "library/mk2",
            "--headless", "--mcp-port", str(self.port),
            # --pace 0 is NOT optional: at the default pace every MCP round trip
            # waits on the host clock and a 1 s measurement costs minutes
            # (mk2.md, "Three loose ends, closed", first finding).
            "--pace", "0",
            "--training", "--script", str(SCRIPT),
        ]
        if record:
            cmd += ["--record", record]
        proc = subprocess.Popen(cmd, cwd=REPO, stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL)
        deadline = time.time() + 60
        while time.time() < deadline:
            time.sleep(0.5)
            try:
                McpClient(f"http://127.0.0.1:{self.port}/mcp").call("get_state")
                return proc
            except Exception:
                continue
        proc.kill()
        sys.exit("emulator did not come up")

    def lua(self, script: str) -> str:
        r = self.c.call("run_lua", script=script)
        if not r.get("ok"):
            raise RuntimeError(f"run_lua failed: {r}\n  script: {script}")
        return r["output"]

    def load(self, arena: str):
        r = self.c.call("load_state", path=arena, pause_after=True)
        if not r.get("ok"):
            raise RuntimeError(f"load_state failed: {r}")
        # CLAUDE.md: a load that is not verified silently measures the PREVIOUS
        # state.  Verify against a field the arena's sidecar pins.
        gap = json.loads(pathlib.Path(REPO / arena.replace(".state", ".gap.json")).read_text())
        got = int(self.lua("tostring(math.abs(game.read_field(2,'x')-game.read_field(1,'x')))"))
        if abs(got - gap["gap_px"]) > 2:
            raise RuntimeError(f"{arena}: gap {got} px != sidecar {gap['gap_px']} px")
        return got

    def run(self, frames: int):
        left = frames
        while left > 0:
            n = min(left, 600)  # MAX_RUN_FRAMES
            r = self.c.call("run_frames", count=n)
            if not r.get("ok") or not r.get("all_landed"):
                raise RuntimeError(f"run_frames stalled: {r}")
            left -= n

    def fetch(self, kind: str) -> list[str]:
        total = int(self.lua(f"BOT.count('{kind}')"))
        out: list[str] = []
        i = 1
        while i <= total:
            chunk = self.lua(f"BOT.dump('{kind}',{i},400)")
            lines = chunk.split("\n") if chunk else []
            out.extend(lines)
            i += 400
        return [l for l in out if l]

    def close(self):
        if self.proc:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.proc.kill()


def parse_events(lines: list[str]) -> list[dict]:
    out = []
    for line in lines:
        d = {}
        for tok in line.split(" "):
            if "=" in tok:
                k, _, v = tok.partition("=")
                d[k] = v
        if d:
            out.append(d)
    return out


# ──────────────────────────── test 1: the confirm ────────────────────────────


@dataclass
class ConfirmRun:
    arena: str
    guard: str
    phase: int
    jab_classes: list[str] = field(default_factory=list)
    jab_deltas: list[int] = field(default_factory=list)
    commits: int = 0
    refusals: int = 0


def test_confirm(s: Session, frames: int, phases: list[int]) -> list[ConfirmRun]:
    runs: list[ConfirmRun] = []
    for guard in ("all", "none"):
        for arena in ARENAS:
            for phase in phases:
                s.load(arena)
                # dummy mode PINNED: only the guard mode differs between families.
                s.lua(f"BOT.setup('block','{guard}')")
                s.lua("BOT.reset()")
                if phase:
                    s.run(phase)          # phase offset: de-synchronise the runs
                    s.lua("BOT.reset()")
                s.run(frames)
                ev = parse_events(s.fetch("events"))
                r = ConfirmRun(arena=arena.split("/")[-1], guard=guard, phase=phase)
                for e in ev:
                    if e["ev"] == "confirm" and e["move"].startswith("HP"):
                        r.jab_classes.append(e["class"])
                        r.jab_deltas.append(int(e["dmg"]))
                    elif e["ev"] == "press" and e["move"] == "HK/far":
                        r.commits += 1
                    elif e["ev"] == "commit_refused":
                        r.refusals += 1
                runs.append(r)
    return runs


# ─────────────────────── test 2 + 3: sustained fights ────────────────────────


@dataclass
class FightRun:
    arena: str
    frames: int
    events: list[dict]

    @property
    def hurt(self):
        return [e for e in self.events if e["ev"] == "hurt"]

    @property
    def clean(self):
        return [e for e in self.hurt if e["guarding"] == "false"]

    @property
    def chip(self):
        return [e for e in self.hurt if e["guarding"] == "true"]


# on_block of every move the bot throws, from arcade.frames.json.  A clean hit
# is only a PUNISH if it lands inside the disadvantage the bot actually owed.
ON_BLOCK = {"HP/far": 13, "HP/close": -2, "HK/far": -20, "cLK": -2}


def classify_punishes(ev: list[dict]) -> list[dict]:
    """Clean hits taken, tagged with whether they landed inside a real punish
    window: within |on_block| frames of one of the bot's own BLOCKED moves."""
    out = []
    last_blocked: tuple[int, str] | None = None
    for e in ev:
        if e["ev"] == "confirm" and e["class"] == "block":
            last_blocked = (int(e["f"]), e["move"])
        elif e["ev"] == "hurt" and e["guarding"] == "false":
            tag, why = "not-a-punish", "no blocked move in flight"
            if last_blocked:
                bf, mv = last_blocked
                minus = -ON_BLOCK.get(mv, 0)
                dt = int(e["f"]) - bf
                if minus > 0 and dt <= minus + 20:
                    tag, why = "punish", f"{dt}f after a blocked {mv} ({ON_BLOCK[mv]})"
                else:
                    why = (f"{dt}f after a blocked {mv} "
                           f"({ON_BLOCK.get(mv, '?')}) — outside any punish window")
            out.append({**e, "tag": tag, "why": why})
    return out


def turn_timeline(ev: list[dict]) -> list[tuple[str, int, int]]:
    """Contact-driven turn segments: (owner, first_frame, last_frame).
    The bot owns the turn while IT is the one making contact; the opponent owns
    it while the bot is the one taking contact."""
    beats = []
    for e in ev:
        if e["ev"] == "confirm" and e["class"] in ("hit", "block"):
            beats.append(("bot", int(e["f"])))
        elif e["ev"] == "hurt":
            beats.append(("opp", int(e["f"])))
    segs: list[list] = []
    for owner, f in beats:
        if segs and segs[-1][0] == owner:
            segs[-1][2] = f
        else:
            segs.append([owner, f, f])
    return [(o, a, b) for o, a, b in segs]


def test_fights(s: Session, arenas: list[str], frames: int) -> list[FightRun]:
    runs = []
    for arena in arenas:
        s.load(arena)
        s.lua("BOT.setup('block_punish','all','fast')")
        s.lua(f"training.set_punish({PUNISH_POOL})")
        s.lua("BOT.reset()")
        s.run(frames)
        runs.append(FightRun(arena.split("/")[-1], frames,
                             parse_events(s.fetch("events"))))
    return runs


# ─────────────────────────────────── main ────────────────────────────────────


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=4026)
    ap.add_argument("--attach", action="store_true",
                    help="use an already-running instance instead of launching")
    ap.add_argument("--confirm-frames", type=int, default=260)
    ap.add_argument("--fight-frames", type=int, default=2400)
    ap.add_argument("--record", default=None,
                    help="path for a v3 session recording (bot-vs-dummy feedstock)")
    ap.add_argument("--out", default=None, help="write the raw traces here")
    args = ap.parse_args()
    if args.port == 4025:
        sys.exit("refusing to touch port 4025 (the user's live session)")

    s = Session(args.port, args.attach, args.record)
    fail = 0
    try:
        # ── 1 ──────────────────────────────────────────────────────────────
        print("═" * 74)
        print("TEST 1 — the confirm: guard ON vs guard OFF, everything else identical")
        print("═" * 74)
        runs = test_confirm(s, args.confirm_frames, [0, 5, 11])
        blocked = [r for r in runs if r.guard == "all"]
        hit = [r for r in runs if r.guard == "none"]
        for fam, rs in (("guard=all  (BLOCKED family)", blocked),
                        ("guard=none (HIT family)", hit)):
            print(f"\n  {fam}")
            print(f"  {'arena':<12}{'phase':>6}{'jab deltas':>28}{'commits':>9}{'refused':>9}")
            for r in rs:
                deltas = ",".join(str(d) for d in r.jab_deltas[:8]) or "—"
                print(f"  {r.arena:<12}{r.phase:>6}{deltas:>28}{r.commits:>9}{r.refusals:>9}")
        b_commits = sum(r.commits for r in blocked)
        h_commits = sum(r.commits for r in hit)
        b_deltas = sorted({d for r in blocked for d in r.jab_deltas})
        h_deltas = sorted({d for r in hit for d in r.jab_deltas})
        b_runs_with = sum(1 for r in blocked if r.commits)
        h_runs_with = sum(1 for r in hit if r.commits)
        print(f"\n  BLOCKED family: {len(blocked)} runs, jab deltas seen {b_deltas}, "
              f"{b_commits} commits in {b_runs_with} runs")
        print(f"  HIT     family: {len(hit)} runs, jab deltas seen {h_deltas}, "
              f"{h_commits} commits in {h_runs_with} runs")
        ok1 = b_commits == 0 and h_runs_with == len(hit)
        print(f"  => {'PASS' if ok1 else 'FAIL'}: commits only in the HIT family, "
              f"and in every run of it")
        fail += 0 if ok1 else 1

        # ── 2 + 3 ──────────────────────────────────────────────────────────
        print()
        print("═" * 74)
        print("TEST 2 — safety under punishment (BlockPunish + reversal fast)")
        print("═" * 74)
        fights = test_fights(s, ARENAS[1:], args.fight_frames)
        total_clean = total_punish = total_chip = 0
        for fr in fights:
            punishes = classify_punishes(fr.events)
            real = [p for p in punishes if p["tag"] == "punish"]
            total_clean += len(punishes)
            total_punish += len(real)
            total_chip += len(fr.chip)
            print(f"\n  {fr.arena} — {fr.frames} frames: "
                  f"{len(fr.chip)} contacts absorbed ON GUARD, "
                  f"{len(punishes)} clean hits taken, {len(real)} of them punishes")
            for p in punishes:
                print(f"      f={p['f']} dmg={p['dmg']} state={p['state']} "
                      f"[{p['tag']}] {p['why']}")
        print(f"\n  TOTAL over {len(fights)}×{args.fight_frames} frames: "
              f"{total_chip} blocked, {total_clean} clean hits taken, "
              f"{total_punish} of them CLEAN PUNISHES off the bot's own blocked pressure")
        ok2 = total_punish == 0
        print(f"  => {'PASS' if ok2 else 'FAIL'}: zero clean punishes")
        fail += 0 if ok2 else 1

        print()
        print("═" * 74)
        print("TEST 3 — turn alternation")
        print("═" * 74)
        ok3 = True
        for fr in fights:
            segs = turn_timeline(fr.events)
            switches = len(segs) - 1
            gaps = [segs[i + 1][1] - segs[i][2] for i in range(len(segs) - 1)]
            worst = max(gaps) if gaps else 0
            print(f"\n  {fr.arena}: {len(segs)} turn segments, {switches} hand-offs, "
                  f"longest silence between contacts {worst} frames "
                  f"({worst / 54.71:.1f}s)")
            print("      " + " ".join(f"{o}[{a}..{b}]" for o, a, b in segs[:26]))
            if len(segs) > 26:
                print(f"      … {len(segs) - 26} more segments")
            if switches < 4 or worst > 300:
                ok3 = False
        print(f"\n  => {'PASS' if ok3 else 'FAIL'}: turns alternate (>=4 hand-offs "
              f"per fight) and no silence over 300 frames (~5.5 s)")
        fail += 0 if ok3 else 1

        if args.out:
            pathlib.Path(args.out).write_text(json.dumps({
                "confirm": [r.__dict__ for r in runs],
                "fights": [{"arena": f.arena, "events": f.events} for f in fights],
            }, indent=1))
            print(f"\n  raw traces -> {args.out}")

        print()
        print("═" * 74)
        print(f"{'ALL ACCEPTANCE RUNS PASSED' if fail == 0 else f'{fail} ACCEPTANCE RUN(S) FAILED'}")
        print("═" * 74)
    finally:
        s.close()
    return 1 if fail else 0


if __name__ == "__main__":
    raise SystemExit(main())
