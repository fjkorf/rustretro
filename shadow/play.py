#!/usr/bin/env python3
"""shadow/play.py -- the P2 shadow bot deploy harness (SPEC.md v2).

Loads a fitted kNN policy (shadow_train.knn.KnnPolicy), connects to a running
RustRetro's MCP server, reads game state at ~8 Hz, computes the SPEC v2
feature vector (via shadow_train.runtime, which mirrors shadow_train.dataset
without modifying it), samples a (move, attack) intent, and injects it on
controller port 1 (P2).

Usage:
    python shadow/play.py --model shadow/models/<name> [--port 4025]
        [--mcp-url http://127.0.0.1:4025/mcp] [--state 1|/path/to/state]
        [--temperature 1.0] [--hz 7.5] [--me-block auto|1|2] [--dry-run]

See shadow/SPEC.md for the feature/action-space contract this implements,
and shadow/train/shadow_train/runtime.py for the deploy-time reimplementation
of dataset.py's per-decision math (kept honest by
shadow/train/tests/test_runtime.py's parity tests).
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

import numpy as np

# shadow_train is normally available via `pip install -e shadow/train` into
# shadow/train/.venv (see shadow/train/pyproject.toml); fall back to adding
# shadow/train/ to sys.path directly so an uninstalled checkout still works.
_TRAIN_DIR = Path(__file__).resolve().parent / "train"
try:
    import shadow_train  # noqa: F401
except ImportError:
    if str(_TRAIN_DIR) not in sys.path:
        sys.path.insert(0, str(_TRAIN_DIR))

from shadow_train import runtime as rt  # noqa: E402
from shadow_train.knn import KnnPolicy  # noqa: E402
from shadow_train.mcpclient import McpClient  # noqa: E402

META_FILE = "meta.json"

# Repo root = two parents up from this file (rustretro/shadow/play.py).
# --model/--state may be given relative to the repo root (e.g. from
# shadow/loop.sh) even when the cwd running this script is somewhere else.
REPO_ROOT = Path(__file__).resolve().parent.parent


def _resolve_repo_path(p: str) -> Path:
    """Resolve a relative CLI path against REPO_ROOT rather than cwd."""
    path = Path(p)
    return path if path.is_absolute() else (REPO_ROOT / path)


def read_tick(mcp: McpClient) -> rt.TickSnapshot:
    """The 5 batched reads per decision (<6 HTTP calls/tick, per plan)."""
    blobs = {name: mcp.read_memory(addr, length) for name, addr, length in rt.READ_PLAN}
    return rt.parse_tick(blobs)


# ── main loop ────────────────────────────────────────────────────────────
def main() -> None:
    ap = argparse.ArgumentParser(description="Shadow AI deploy harness (P2 bot).")
    ap.add_argument("--model", required=True, help="Fitted model dir (shadow/models/<name>/)")
    ap.add_argument("--port", type=int, default=4025, help="RustRetro MCP TCP port")
    ap.add_argument("--mcp-url", default=None, help="Override full MCP URL (default http://127.0.0.1:<port>/mcp)")
    ap.add_argument("--state", default=None, help="Save-state slot (1-9) or path to load at startup")
    ap.add_argument("--temperature", type=float, default=1.0, help="Sampling temperature (0 = argmax)")
    ap.add_argument("--hz", type=float, default=7.5, help="Decision rate (SPEC §4: ~8 Hz)")
    ap.add_argument("--me-block", default="auto", choices=["auto", "1", "2"],
                     help="Which fighter block the bot controls; auto = larger-X block at each round start")
    ap.add_argument("--dry-run", action="store_true",
                     help="Read + decide + print intents, but inject nothing")
    ap.add_argument("--seed", type=int, default=None, help="RNG seed for intent sampling")
    args = ap.parse_args()

    model_dir = _resolve_repo_path(args.model)
    policy = KnnPolicy.load(model_dir)
    policy.temperature = args.temperature
    meta = json.loads((model_dir / META_FILE).read_text())

    drift = rt.check_calibration_drift(meta)
    if drift:
        print("[drift-guard] WARNING: model meta.json disagrees with the "
              "live shadow_train.dataset constants:", file=sys.stderr)
        for m in drift:
            print(f"  - {m}", file=sys.stderr)
    else:
        print("[drift-guard] OK: model calibration matches dataset.py exactly.")

    mcp_url = args.mcp_url or f"http://127.0.0.1:{args.port}/mcp"
    mcp = McpClient(mcp_url, client_name="shadow-play")
    mcp.connect()
    print(f"[mcp] connected to {mcp_url}")

    if args.state:
        mcp.enable_writes()
        # load_state itself distinguishes a save-state slot (int) from a
        # path; only a path needs repo-root resolution.
        try:
            int(args.state)
            state_spec = args.state
        except ValueError:
            state_spec = str(_resolve_repo_path(args.state))
        r = mcp.load_state(state_spec)
        print(f"[mcp] load_state({state_spec}) -> {r}")

    override = None if args.me_block == "auto" else ("block1" if args.me_block == "1" else "block2")

    period_frames = rt.frames_per_decision(args.hz)
    period_s = 1.0 / args.hz
    rng = np.random.default_rng(args.seed)

    buffers = rt.RoundBuffers()
    was_live = False
    tick_i = 0
    latencies: list[float] = []
    trace_budget = 20  # print a trace line for the first N decisions

    print(f"[play] model={model_dir} hz={args.hz} period_frames={period_frames} "
          f"temperature={args.temperature} me_block_override={override or 'auto'} "
          f"dry_run={args.dry_run}")

    try:
        while True:
            t0 = time.monotonic()

            snap = read_tick(mcp)
            live = rt.is_controllable(snap)

            if live and not was_live:
                x1, x2 = snap.block1["x"], snap.block2["x"]
                me_block = override or rt.resolve_me_block(x1, x2)
                buffers.reset(me_block=me_block)
                print(f"[round] start -- me_block={me_block} "
                      f"(x1={x1} x2={x2})")
            was_live = live

            if not live:
                # Off-gate: inject nothing, buffers already reset on the
                # next rising edge (requirement 4).
                latencies.append(time.monotonic() - t0)
                _sleep_to_period(t0, period_s)
                continue

            me_block = buffers.me_block or override or rt.resolve_me_block(
                snap.block1["x"], snap.block2["x"]
            )
            opp_block = rt.other_block(me_block)
            me_now = getattr(snap, me_block)
            opp_now = getattr(snap, opp_block)
            me_combo_now = snap.combo_on_b1 if me_block == "block1" else snap.combo_on_b2
            opp_combo_now = snap.combo_on_b2 if me_block == "block1" else snap.combo_on_b1

            s = 1 if me_now["facing"] == 1 else -1

            # Opponent view is read one tick stale (~P frames; see the
            # runtime.py module docstring for why this, not the spec's exact
            # 2-4 frames, is what a decision-rate poll loop can offer).
            opp_lagged = buffers.prev_opp if buffers.prev_opp is not None else opp_now
            opp_combo_lagged = buffers.prev_opp_combo

            fwd_hold, back_hold = rt.hold_fractions(buffers.last_emitted_mask, s)
            me_hitstun = buffers.me_hitstun.update(buffers.tick, me_combo_now)
            opp_hitstun = buffers.opp_hitstun.update(buffers.tick, opp_combo_lagged)

            scal = rt.build_scalars(
                me_now, opp_lagged, s, fwd_hold, back_hold, me_hitstun, opp_hitstun
            )
            buffers.stacker.push(scal)

            move, attack, dist_x = None, None, scal["dist_x"]
            if buffers.stacker.ready():
                x = buffers.stacker.vector()
                move, attack = policy.predict(x, rng=rng)
                mask = rt.intent_to_mask(move, attack, s)
                buttons = rt.mask_to_button_names(mask)
                if not args.dry_run and buttons:
                    mcp.press(buttons, frames=period_frames, port=1)
                buffers.last_emitted_mask = mask if buttons else 0

            buffers.prev_opp = opp_now
            buffers.prev_opp_combo = opp_combo_now
            buffers.tick += 1
            tick_i += 1

            if trace_budget > 0:
                trace_budget -= 1
                mv = rt.dataset.MOVE_CLASSES[move] if move is not None else "-"
                at = rt.dataset.ATTACK_CLASSES[attack] if attack is not None else "-"
                print(f"[trace] tick={tick_i:4d} live={live} me={me_block} "
                      f"dist_x={dist_x:+.3f} s={s:+d} move={mv:<12} attack={at}")

            latencies.append(time.monotonic() - t0)
            _sleep_to_period(t0, period_s)

    except KeyboardInterrupt:
        pass
    finally:
        if latencies:
            avg_ms = 1000.0 * sum(latencies) / len(latencies)
            max_ms = 1000.0 * max(latencies)
            print(f"[exit] {len(latencies)} ticks, avg latency {avg_ms:.1f} ms, "
                  f"max {max_ms:.1f} ms (budget {1000.0/args.hz:.1f} ms/tick)")


def _sleep_to_period(t0: float, period_s: float) -> None:
    remaining = period_s - (time.monotonic() - t0)
    if remaining > 0:
        time.sleep(remaining)


if __name__ == "__main__":
    main()
