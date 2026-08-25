# shadow/train — Wave 2d trainer

Implements `shadow/SPEC.md` rev. v2: jsonl-v2 recordings → 8 Hz decisions
(chord-aware labels, side-agnostic features, stale opponent observations,
K=4 stacking) → kNN case-retrieval baseline (§7.1, support-constrained by
construction) → situation-bucket evaluation + coverage drill list (§7.4/§7.6).

## Setup

```sh
python3 -m venv .venv && .venv/bin/pip install numpy
```

## Run

```sh
# build + round-holdout evaluate (§7.4 report + §7.6 coverage)
.venv/bin/python3 -m shadow_train eval ../recordings/<session>.jsonl [more.jsonl ...] \
    [--char 0]      # per-character model filter (me char id; Yashaou = 0)
    [--k 15] [--holdout 0.2]

# fit on ALL of the given recordings (no holdout) and persist to disk
.venv/bin/python3 -m shadow_train fit ../recordings/<session>.jsonl [more.jsonl ...] \
    --out ../models/<name>/ [--char 0] [--k 15]

# print the coverage drill list for an already-fitted model (no recordings needed)
.venv/bin/python3 -m shadow_train report ../models/<name>/
```

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

## Not here yet (per PLAN)

- The MLP that must beat this baseline (§7.1)
- Macro-action mining from the command ring (§3d)
- Deploy harness (Wave 2e; needs the VS-mode verification session)
