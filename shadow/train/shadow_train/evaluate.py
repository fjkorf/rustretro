"""Evaluation stack (SPEC §7.4) + coverage report (§7.6).

Split by ROUND (never by frame — adjacent decisions are near-duplicates), then:
  (a) held-out next-action accuracy per situation bucket, vs the majority-class
      baseline for the same bucket
  (b) conditional action-frequency match per bucket (Jensen-Shannon distance
      between predicted-sample and user distributions)
  coverage: per-bucket example counts, flagged under MIN_EXAMPLES.
"""

from __future__ import annotations

import numpy as np

from .dataset import ATTACK_CLASSES, MOVE_CLASSES
from .knn import KnnPolicy

MIN_EXAMPLES = 25

N_MOVE_CLASSES = len(MOVE_CLASSES)
N_ATTACK_CLASSES = len(ATTACK_CLASSES)


def split_by_round(data: dict, holdout_frac: float = 0.2, seed: int = 0):
    """Split unit = round_key, which after dataset._segment (task 1) is a
    pseudo-round (file, round_id, seg_k) rather than a bare (file, round_id) --
    long training-mode rounds get chopped into many segments so this split
    isn't degenerate (see dataset.SEGMENT_DECISIONS). With dozens of pseudo-
    round units instead of 1-2 real rounds, holding out a chronological tail
    (the old behavior) would sample only the endgame of the last file/round;
    shuffle the unique keys with a fixed seed instead so the held-out set
    covers the whole session while still never splitting a round/segment
    across train and test (no frame-level leakage, §7.4)."""
    keys = data["rounds"]
    uniq = list(dict.fromkeys(keys))  # order of first appearance
    order = np.random.default_rng(seed).permutation(len(uniq))
    shuffled = [uniq[i] for i in order]
    n_hold = max(1, int(len(uniq) * holdout_frac))
    hold = set(shuffled[:n_hold])
    test_idx = np.array([i for i, k in enumerate(keys) if k in hold])
    train_idx = np.array([i for i, k in enumerate(keys) if k not in hold])
    return train_idx, test_idx


def _js_distance(p: np.ndarray, q: np.ndarray) -> float:
    p = p / max(p.sum(), 1e-9)
    q = q / max(q.sum(), 1e-9)
    m = (p + q) / 2

    def kl(a, b):
        mask = a > 0
        return float((a[mask] * np.log(a[mask] / np.maximum(b[mask], 1e-12))).sum())

    return float(np.sqrt(max(0.0, (kl(p, m) + kl(q, m)) / 2)))


def evaluate(data: dict, k: int = 15, holdout_frac: float = 0.2, seed: int = 7,
             neutral_cap: float | None = None):
    from .dataset import NEUTRAL_CAP_RATIO, subsample_neutral

    if neutral_cap is None:
        neutral_cap = NEUTRAL_CAP_RATIO
    train_idx, test_idx = split_by_round(data, holdout_frac, seed=seed)
    X, ym, ya, buckets = data["X"], data["y_move"], data["y_attack"], data["buckets"]
    # Fit-side neutral cap mirrors what `fit` ships (deploy parity); the
    # held-out side keeps the true distribution.
    train = subsample_neutral(
        {"X": X[train_idx], "y_move": ym[train_idx], "y_attack": ya[train_idx],
         "buckets": buckets[train_idx], "rounds": [data["rounds"][i] for i in train_idx]},
        cap_ratio=neutral_cap,
    )
    policy = KnnPolicy(k=k).fit(train["X"], train["y_move"], train["y_attack"])
    rng = np.random.default_rng(seed)

    pred_m = np.empty(len(test_idx), dtype=int)
    pred_a = np.empty(len(test_idx), dtype=int)
    samp_m = np.empty(len(test_idx), dtype=int)
    samp_a = np.empty(len(test_idx), dtype=int)
    for j, i in enumerate(test_idx):
        pred_m[j], pred_a[j] = policy.predict(X[i])
        samp_m[j], samp_a[j] = policy.predict(X[i], rng=rng)

    report = {"n_train": len(train["X"]), "n_test": len(test_idx), "buckets": {}}
    tb = buckets[test_idx]
    for b in sorted(set(buckets)):
        mask = tb == b
        n = int(mask.sum())
        entry = {"n_test": n, "n_train": int((train["buckets"] == b).sum())}
        if n:
            tm, ta = ym[test_idx][mask], ya[test_idx][mask]
            entry["move_acc"] = float((pred_m[mask] == tm).mean())
            entry["move_majority"] = float(
                (tm == np.bincount(tm, minlength=N_MOVE_CLASSES).argmax()).mean()
            )
            entry["attack_acc"] = float((pred_a[mask] == ta).mean())
            entry["attack_majority"] = float(
                (ta == np.bincount(ta, minlength=N_ATTACK_CLASSES).argmax()).mean()
            )
            entry["move_jsd"] = _js_distance(
                np.bincount(samp_m[mask], minlength=N_MOVE_CLASSES).astype(float),
                np.bincount(tm, minlength=N_MOVE_CLASSES).astype(float),
            )
            entry["attack_jsd"] = _js_distance(
                np.bincount(samp_a[mask], minlength=N_ATTACK_CLASSES).astype(float),
                np.bincount(ta, minlength=N_ATTACK_CLASSES).astype(float),
            )
        report["buckets"][b] = entry
    return report


def print_report(report: dict, data: dict):
    print(f"train decisions: {report['n_train']}   held-out: {report['n_test']}")
    print(f"{'bucket':<9} {'n_tr':>5} {'n_te':>5} | {'move acc':>8} {'(maj)':>6} "
          f"{'atk acc':>8} {'(maj)':>6} | {'moveJSD':>7} {'atkJSD':>7}")
    for b, e in report["buckets"].items():
        if "move_acc" in e:
            print(f"{b:<9} {e['n_train']:>5} {e['n_test']:>5} | "
                  f"{e['move_acc']:>8.2f} {e['move_majority']:>6.2f} "
                  f"{e['attack_acc']:>8.2f} {e['attack_majority']:>6.2f} | "
                  f"{e['move_jsd']:>7.3f} {e['attack_jsd']:>7.3f}")
        else:
            print(f"{b:<9} {e['n_train']:>5} {e['n_test']:>5} | (no held-out examples)")
    # coverage (§7.6): the drill list
    print("\ncoverage (drill list — demonstrate more of anything flagged):")
    from collections import Counter

    counts = Counter(data["buckets"].tolist())
    for b in ["defense", "offense", "air", "corner", "neutral"]:
        n = counts.get(b, 0)
        flag = "  <-- record more!" if n < MIN_EXAMPLES else ""
        print(f"  {b:<9} {n:>6}{flag}")
    # label distribution overall — sanity that chords/attacks appear
    am = np.bincount(data["y_attack"], minlength=N_ATTACK_CLASSES)
    print("\nattack-label distribution:",
          {ATTACK_CLASSES[i]: int(n) for i, n in enumerate(am) if n})
    mm = np.bincount(data["y_move"], minlength=N_MOVE_CLASSES)
    print("move-label distribution:  ",
          {MOVE_CLASSES[i]: int(n) for i, n in enumerate(mm) if n})
