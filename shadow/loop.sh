#!/usr/bin/env bash
#
# The shadow loop, one command: fit a fresh model from recent recordings,
# print the coverage drill list, and send the shadow into the running game.
#
#   shadow/loop.sh                 # fit goat-vNEXT from recent v2 recordings, fight
#   shadow/loop.sh --fit-only      # refit + report, don't launch the shadow
#   shadow/loop.sh --model NAME    # skip fitting, fight an existing model
#   shadow/loop.sh --me N --opp M  # matchup-filtered fit (per-matchup model,
#                                  # named via shadow_train.asurabld slugs)
#   shadow/loop.sh --push          # fit, then load the model into the running
#                                  # app's NATIVE runner (Shift+F5 / 🎯 panel)
#                                  # instead of fighting over MCP via play.py
#
# Assumes the game is already running with --mcp (default port 4025). The
# arena state and port can be overridden via ARENA / PORT env vars.

set -euo pipefail
cd "$(dirname "$0")/.."

PORT="${PORT:-4025}"
# Arena resolution: ARENA env wins; else the current-arena pointer (set from
# the 🎯 Training panel's Arena section); else the committed canonical one.
if [[ -z "${ARENA:-}" ]]; then
  if [[ -f shadow/arenas/current.state ]]; then
    ARENA=shadow/arenas/current.state
  else
    ARENA=shadow/arenas/goat-vs-rosemary.state
  fi
fi
PY=shadow/train/.venv/bin/python3
FIT=1
MODEL=""
PUSH=0
ME=""
OPP=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --fit-only) FIT=2; shift ;;
    --model) MODEL="$2"; FIT=0; shift 2 ;;
    --push) PUSH=1; shift ;;
    --me) ME="$2"; shift 2 ;;
    --opp) OPP="$2"; shift 2 ;;
    *) echo "usage: shadow/loop.sh [--fit-only | --model NAME | --push | --me N --opp M]" >&2; exit 2 ;;
  esac
done

if [[ $FIT -ge 1 ]]; then
  # jsonl-v2 recordings only (v1 files lack "block1"), newest 12.
  RECS=()
  for f in $(ls -t shadow/recordings/*.jsonl 2>/dev/null | head -12); do
    head -c 4096 "$f" | grep -q '"block1"' && RECS+=("$f")
  done
  [[ ${#RECS[@]} -gt 0 ]] || { echo "no v2 recordings found" >&2; exit 1; }
  # Matchup filters name the model by slug (goat-vs-rosemary); the legacy
  # unfiltered fit keeps the goat-vN series.
  FILTERS=()
  PREFIX=goat
  if [[ -n "$ME" || -n "$OPP" ]]; then
    [[ -n "$ME" ]] && FILTERS+=(--char "$ME")
    [[ -n "$OPP" ]] && FILTERS+=(--opp "$OPP")
    PREFIX=$("$PY" -c "from shadow_train.asurabld import matchup_slug; print(matchup_slug(${ME:-None}, ${OPP:-None}))")
  fi
  # next version number
  N=1
  while [[ -d "shadow/models/$PREFIX-v$N" ]]; do N=$((N + 1)); done
  MODEL="$PREFIX-v$N"
  echo "── fitting $MODEL from ${#RECS[@]} recording(s) ──"
  printf '    %s\n' "${RECS[@]}"
  # shadow_train is `pip install -e`d into .venv (see shadow/train/pyproject.toml),
  # so `-m shadow_train` resolves without cd-ing into shadow/train first.
  "$PY" -m shadow_train fit "${RECS[@]}" ${FILTERS[@]+"${FILTERS[@]}"} --out "shadow/models/$MODEL/"
  echo
  "$PY" -m shadow_train report "shadow/models/$MODEL/"
  [[ $FIT -eq 2 ]] && exit 0
fi

if [[ $PUSH -eq 1 ]]; then
  # Native path: reload the WHOLE models dir as a SET over MCP — the fresh
  # model folds in via newest-per-key dedup and auto-matchup-switching stays
  # armed (pushing just the single model would replace a loaded set).
  # load_shadow needs the write gate; both calls share one MCP session so
  # the arm sticks. A disabled shadow stays disabled (Shift+F5 to fight);
  # an active one swaps brains at the next round start.
  echo
  echo "── pushing $MODEL (as part of the shadow/models set) on port $PORT ──"
  "$PY" -u - "$(pwd)/shadow/models" "$PORT" <<'PYEOF'
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
