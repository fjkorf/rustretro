#!/usr/bin/env bash
#
# The shadow loop, one command: fit a fresh model from recent recordings,
# print the coverage drill list, and send the shadow into the running game.
#
#   shadow/loop.sh                 # fit goat-vNEXT from recent v2 recordings, fight
#   shadow/loop.sh --fit-only      # refit + report, don't launch the shadow
#   shadow/loop.sh --model NAME    # skip fitting, fight an existing model
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

while [[ $# -gt 0 ]]; do
  case "$1" in
    --fit-only) FIT=2; shift ;;
    --model) MODEL="$2"; FIT=0; shift 2 ;;
    *) echo "usage: shadow/loop.sh [--fit-only | --model NAME]" >&2; exit 2 ;;
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
  (cd shadow/train && .venv/bin/python3 -m shadow_train fit \
      $(printf '../../%s ' "${RECS[@]}") --out "../models/$MODEL/")
  echo
  (cd shadow/train && .venv/bin/python3 -m shadow_train report "../models/$MODEL/")
  [[ $FIT -eq 2 ]] && exit 0
fi

echo
echo "── the shadow ($MODEL) enters — Ctrl-C to stop it ──"
exec "$PY" -u shadow/play.py --model "shadow/models/$MODEL" \
    --state "$ARENA" --port "$PORT"
