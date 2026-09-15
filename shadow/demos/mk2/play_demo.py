#!/usr/bin/env python
"""Prep and run one combat demo in a LIVE RustRetro session.

    shadow/train/.venv/bin/python shadow/demos/mk2/play_demo.py 2 [--port 4025]

Finds `shadow/demos/mk2/<n>-*.beats.json`, loads its arena atomically
(`load_state pause_after=True` — never bracketed with resume/pause), arms the
matching `demo-<slug>` slot on BOTH ports (manual trigger only ARMS; nothing
plays while paused), then resumes so the fight runs at native speed. Run it
from the repo root — slot names resolve against the emulator's cwd.

The pause→arm→resume order is what makes a manual-trigger replay
deterministic against a live session (VERIFICATION.md "Slot replay").
"""

from __future__ import annotations

import argparse
import glob
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "train"))
from shadow_train.mcpclient import McpClient  # noqa: E402


def call_ok(client: McpClient, tool: str, **kwargs):
    r = client.call(tool, **kwargs)
    if not r.get("ok", True):
        raise SystemExit(f"{tool} failed: {r!r}")
    return r


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("n", type=int, help="demo number 1-5")
    ap.add_argument("--port", type=int, default=4025, help="MCP port of the LIVE session")
    args = ap.parse_args()

    matches = sorted(glob.glob(f"shadow/demos/mk2/{args.n}-*.beats.json"))
    if len(matches) != 1:
        raise SystemExit(f"expected exactly one beats file for demo {args.n}, got {matches!r}")
    beats = json.loads(Path(matches[0]).read_text())
    slot = "demo-" + Path(matches[0]).name.removesuffix(".beats.json")
    arena = beats["arena"]

    client = McpClient(f"http://127.0.0.1:{args.port}/mcp")
    call_ok(client, "enable_writes")
    load = call_ok(client, "load_state", path=arena, pause_after=True)
    if not load.get("paused"):
        raise SystemExit(f"load_state did not report paused=true: {load!r}")
    client.call("play_inputs", action="stop")  # defensive; refusal here is fine
    start = call_ok(
        client, "play_inputs", action="start", name=slot, port="both", trigger="manual"
    )
    call_ok(client, "resume")
    print(f"demo {args.n}: {beats['name']!r} — {start.get('frames')} frames from {arena}")
    print(f"claim: {beats['claim']}")


if __name__ == "__main__":
    main()
