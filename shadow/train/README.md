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
.venv/bin/python3 -m shadow_train ../recordings/<session>.jsonl [more.jsonl ...] \
    [--char 0]      # per-character model filter (me char id; Yashaou = 0)
    [--k 15] [--holdout 0.2]
```

Only jsonl-v2 recordings (recorder v2, 2026-08-24+) are accepted. The eval
splits by round — never by frame — and reports per-bucket accuracy vs the
majority baseline plus Jensen-Shannon distance between the sampled and
demonstrated action distributions. The coverage list is the §7.6 drill list:
whatever it flags, demonstrate more of it in training mode.

## Interpreting early runs

Until there are several sessions of deliberate play, expect Neutral-dominated
labels and accuracies pinned at the majority baseline — that is the honest
signal to record more focused demonstrations, not a modeling failure. The kNN
earns its keep once buckets have real decision diversity.

## Not here yet (per PLAN)

- The MLP that must beat this baseline (§7.1)
- Macro-action mining from the command ring (§3d)
- Deploy harness (Wave 2e; needs the VS-mode verification session)
