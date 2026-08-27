# shadow/train — Wave 2d trainer

Implements `shadow/SPEC.md` rev. v2: jsonl-v2 recordings → 8 Hz decisions
(chord-aware labels, side-agnostic features, stale opponent observations,
K=4 stacking) → kNN case-retrieval baseline (§7.1, support-constrained by
construction) → situation-bucket evaluation + coverage drill list (§7.4/§7.6).

## Setup

```sh
python3 -m venv .venv && .venv/bin/pip install -e .
```

`pip install -e .` (via `pyproject.toml`) installs `shadow_train` into `.venv`
in editable mode, so `.venv/bin/python3 -m shadow_train ...` and
`import shadow_train` work from **any** working directory — not just from
inside `shadow/train/` — and `shadow/play.py` (which lives one level up) can
import it too. If you only ever ran `pip install numpy` in an older checkout,
re-run `pip install -e .` once to pick this up (numpy is still pulled in as a
dependency).

## Run

```sh
# from the repo root (or anywhere — shadow_train is installed, not path-hacked):
PY=shadow/train/.venv/bin/python3

# build + round-holdout evaluate (§7.4 report + §7.6 coverage)
"$PY" -m shadow_train eval shadow/recordings/<session>.jsonl [more.jsonl ...] \
    [--char 0]      # per-character model filter (me char id; Yashaou = 0)
    [--k 15] [--holdout 0.2]

# fit on ALL of the given recordings (no holdout) and persist to disk
"$PY" -m shadow_train fit shadow/recordings/<session>.jsonl [more.jsonl ...] \
    --out shadow/models/<name>/ [--char 0] [--k 15]

# print the coverage drill list for an already-fitted model (no recordings needed)
"$PY" -m shadow_train report shadow/models/<name>/
```

(If invoking from inside `shadow/train/` itself, as the commands looked
before the package was installed, use `../recordings/...` / `../models/...`
paths instead — both styles work now, since the paths are just argv strings
resolved by argparse's `Path`, not by where the package is imported from.)

Only jsonl-v2 recordings (recorder v2, 2026-08-24+) are accepted. The eval
splits by round — never by frame — and reports per-bucket accuracy vs the
majority baseline plus Jensen-Shannon distance between the sampled and
demonstrated action distributions. The coverage list is the §7.6 drill list:
whatever it flags, demonstrate more of it in training mode.

The split unit is a *pseudo-round*, not a raw round: training-mode sessions
freeze the round timer, so a single `round_id` can run tens of thousands of
frames; `dataset.py` chops any round over `SEGMENT_DECISIONS` (150) decisions
into fixed-size chunks so held-out data isn't degenerate. `report` reads
`meta.json` from a `fit` output directory — no need to re-load recordings
just to see the drill list again.

Run the tests (stdlib `unittest`, no extra deps):

```sh
.venv/bin/python3 -m unittest discover -s tests -t . -v
```

## Interpreting early runs

Until there are several sessions of deliberate play, expect Neutral-dominated
labels and accuracies pinned at the majority baseline — that is the honest
signal to record more focused demonstrations, not a modeling failure. The kNN
earns its keep once buckets have real decision diversity.

## Also in this package

`shadow_train.mcpclient.McpClient` is the shared MCP HTTP client (initialize
handshake, `tools/call`, `resources/read`) other scripts import instead of
reimplementing. `shadow_train.re` builds on it with the live memory-RE
session protocols (`Probe.rd8`/`wr8`, `snapshot`/`stable_snapshot`, the
`running()` phase oracle, `diff`/`static_diff`/`intersect_changes`,
`lua_macro`) — see `.claude/skills/re-probe/SKILL.md` for the protocol
write-up and `library/mk2/mk2-genesis.md` "Session craft" for the gotchas
it encodes.

## Not here yet (per PLAN)

- The MLP that must beat this baseline (§7.1)
- Macro-action mining from the command ring (§3d)
- Deploy harness (Wave 2e; needs the VS-mode verification session)
