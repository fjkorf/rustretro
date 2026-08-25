#!/usr/bin/env bash
#
# The shadow loop, one command: fit a fresh model from recent recordings,
# print the coverage drill list, and send the shadow into the running game.
#
#   shadow/loop.sh                 # fit goat-vNEXT from recent v2 recordings, fight
#   shadow/loop.sh --fit-only      # refit + report, don't launch the shadow
#   shadow/loop.sh --model NAME    # skip fitting, fight an existing model
#   shadow/loop.sh --push          # fit, then load the model into the running
#                                  # app's NATIVE runner (Shift+F5 / 🎯 panel)
#                                  # instead of fighting over MCP via play.py
#
# Assumes the game is already running with --mcp (default port 4025). The
# arena state and port can be overridden via ARENA / PORT env vars.

set -euo pipefail
cd "$(dirname "$0")/.."

PORT="${PORT:-4025}"
ARENA="${ARENA:-shadow/arenas/goat-vs-rosemary.state}"
PY=shadow/train/.venv/bin/python3
FIT=1
MODEL=""
PUSH=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --fit-only) FIT=2; shift ;;
    --model) MODEL="$2"; FIT=0; shift 2 ;;
    --push) PUSH=1; shift ;;
    *) echo "usage: shadow/loop.sh [--fit-only | --model NAME | --push]" >&2; exit 2 ;;
  esac
done

if [[ $FIT -ge 1 ]]; then
  # jsonl-v2 recordings only (v1 files lack "block1"), newest 12.
  RECS=()
  for f in $(ls -t shadow/recordings/*.jsonl 2>/dev/null | head -12); do
    head -c 4096 "$f" | grep -q '"block1"' && RECS+=("$f")
  done
  [[ ${#RECS[@]} -gt 0 ]] || { echo "no v2 recordings found" >&2; exit 1; }
  # next version number
  N=1
  while [[ -d "shadow/models/goat-v$N" ]]; do N=$((N + 1)); done
  MODEL="goat-v$N"
  echo "── fitting $MODEL from ${#RECS[@]} recording(s) ──"
  # shadow_train is `pip install -e`d into .venv (see shadow/train/pyproject.toml),
  # so `-m shadow_train` resolves without cd-ing into shadow/train first.
  "$PY" -m shadow_train fit "${RECS[@]}" --out "shadow/models/$MODEL/"
  echo
  "$PY" -m shadow_train report "shadow/models/$MODEL/"
  [[ $FIT -eq 2 ]] && exit 0
fi

if [[ $PUSH -eq 1 ]]; then
  # Native path: swap the in-app runner's model over MCP (load_shadow needs
  # the write gate; both calls share one MCP session so the arm sticks).
  # A disabled shadow stays disabled (Shift+F5 to fight); an active one
  # swaps brains mid-fight.
  echo
  echo "── pushing $MODEL into the app on port $PORT (native runner) ──"
  "$PY" -u - "$(pwd)/shadow/models/$MODEL" "$PORT" <<'PYEOF'
import sys
from shadow_train.mcpclient import McpClient
path, port = sys.argv[1], sys.argv[2]
c = McpClient(f"http://127.0.0.1:{port}/mcp")
c.call("enable_writes")
r = c.call("load_shadow", path=path)
print(r)
sys.exit(0 if r.get("ok") else 1)
PYEOF
  echo "Shift+F5 (or the 🎯 Training panel) toggles it."
  exit 0
fi

echo
echo "── the shadow ($MODEL) enters — Ctrl-C to stop it ──"
exec "$PY" -u shadow/play.py --model "shadow/models/$MODEL" \
    --state "$ARENA" --port "$PORT"
