---
name: fit-ghost
description: Fit a behavioral-clone model from recorded gameplay and deploy it — the record → fit → report → fight ritual for any game family
---

# fit-ghost — the shadow loop, any family

## Record

Play with `--record shadow/recordings/<family>/session-$(date +%Y%m%d-%H%M).jsonl`
(or the 🎯 panel's recorder — auto-named, optional style tag). jsonl-v3 recordings are
self-describing: a `.meta.json` sidecar snapshots the profile's fields/gate/calibration.
The gate only captures in-fight frames; demo rounds are filtered at fit time by zero
p1_input, so attract data is harmless.

## Fit

```sh
shadow/train/.venv/bin/python3 -m shadow_train fit \
  shadow/recordings/<family>/session-*.jsonl \
  --out shadow/models/<family>/<name>
```

- The fit resolves its profile (family AND port — chord labels!) from the recordings'
  own sidecars; `--game` / `RUSTRETRO_GAME_DIR` are overrides only. A family or feature
  mismatch aborts loudly — read the message, don't force it.
- `.rounds.jsonl` sidecars in the glob are skipped automatically.
- Matchup-scoped models: `--me <id> --opp <id>` (canonical roster ids from family.json).

## Sanity-check the model before fighting it

```sh
python3 -c "import json; m=json.load(open('shadow/models/<family>/<name>/meta.json')); \
print(m['family'], m['port'], m['n_decisions'], m['bucket_counts'], m['move_label_counts'])"
```

Red flags that mean CALIBRATION, not style: one bucket swallowing ~everything
(air ≈ all → GROUND_Y wrong; corner ≈ all → x is world-position and screen constants
are wrong — drop CORNER_PX/SCREEN_W until stage bounds are mapped). A healthy model
shows mostly neutral, plausible move/attack spreads, and the player's known fingerprint.

## Deploy

- In-app: 🎯 Training panel → model picker → Shift+F5 (runtime loads arrive disabled).
- At launch: `--shadow shadow/models/<family>/<name>` (enabled, fatal on error).
- The shadow drives controller port 2 — on console ports that means a 2-HUMAN fight
  (controller 2 joins and picks the ghost's body); vs-CPU fights the CPU owns that
  character and the shadow's input is ignored.
- `shadow/loop.sh` wraps fit+report+fight for the standing ritual; `FAMILY=<family>` scopes it.

## Grow it

`python -m shadow_train report --model ...` → sparse buckets are the drill list.
`python -m shadow_train coverage` → the me×opp matrix. Every recorded session compounds;
refit under the next version name and let the model-set loader pick winners per matchup.
