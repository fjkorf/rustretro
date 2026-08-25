"""CLI: python -m shadow_train <fit|eval|report> ...

  fit <recordings...> --out DIR [--char N] [--k K]
      Build the decision dataset, fit the kNN baseline on ALL of it (no
      holdout -- fit is for shipping a model, eval is for measuring one),
      and persist it to DIR (SPEC §7.5: retrain from scratch each session).

  eval <recordings...> [--char N] [--k K] [--holdout F]
      Build the dataset, fit/evaluate with a round-held-out split, and print
      the §7.4 evaluation + §7.6 coverage report. (Previously the only mode;
      unchanged behavior.)

  report <model-dir>
      Print the §7.6 coverage drill list stored in a fitted model's meta.json
      -- no recordings needed, just what's already on disk.
"""

import argparse
import json
from datetime import datetime, timezone
from pathlib import Path

from . import dataset
from .dataset import ATTACK_CLASSES, MOVE_CLASSES, SCALAR_FEATURES, build
from .evaluate import MIN_EXAMPLES, evaluate, print_report
from .knn import KnnPolicy

META_FILE = "meta.json"

CALIBRATION_KEYS = [
    "GROUND_Y", "X_SCALE", "Y_SCALE", "TIMER_SCALE", "ANIM_SCALE",
    "CORNER_PX", "HEALTH_MAX", "SCREEN_W", "P", "K", "STALE",
    "SEGMENT_DECISIONS", "HITSTUN_RECENT_FRAMES",
]


def _bucket_counts(data: dict) -> dict:
    from collections import Counter

    return dict(Counter(data["buckets"].tolist()))


def _label_counts(y, classes: list[str]) -> dict:
    import numpy as np

    counts = np.bincount(y, minlength=len(classes))
    return {classes[i]: int(n) for i, n in enumerate(counts) if n}


def cmd_fit(args) -> None:
    data = build(args.recordings, char_filter=args.char)
    policy = KnnPolicy(k=args.k).fit(data["X"], data["y_move"], data["y_attack"])

    out_dir = Path(args.out)
    policy.save(out_dir)
    meta = {
        "feature_names": SCALAR_FEATURES,
        "calibration": {name: getattr(dataset, name) for name in CALIBRATION_KEYS},
        "move_classes": MOVE_CLASSES,
        "attack_classes": ATTACK_CLASSES,
        "k": policy.k,
        "temperature": policy.temperature,
        "char_filter": args.char,
        "source_files": [str(p) for p in args.recordings],
        "n_decisions": int(len(data["X"])),
        "n_rounds": int(len(set(data["rounds"]))),
        "bucket_counts": _bucket_counts(data),
        "move_label_counts": _label_counts(data["y_move"], MOVE_CLASSES),
        "attack_label_counts": _label_counts(data["y_attack"], ATTACK_CLASSES),
        "created": datetime.now(timezone.utc).isoformat(),
    }
    (out_dir / META_FILE).write_text(json.dumps(meta, indent=2) + "\n")
    print(f"fit {meta['n_decisions']} decisions from {meta['n_rounds']} rounds "
          f"({len(args.recordings)} file(s)) -> {out_dir}")
    print(f"  wrote {out_dir / 'cases.npz'} and {out_dir / META_FILE}")


def cmd_eval(args) -> None:
    data = build(args.recordings, char_filter=args.char)
    print(f"decisions: {len(data['X'])} from {len(set(data['rounds']))} rounds "
          f"({data['X'].shape[1]} features)")
    report = evaluate(data, k=args.k, holdout_frac=args.holdout)
    print_report(report, data)


def cmd_report(args) -> None:
    model_dir = Path(args.model_dir)
    meta = json.loads((model_dir / META_FILE).read_text())
    print(f"model: {model_dir}")
    print(f"  source files: {', '.join(meta['source_files'])}")
    print(f"  fitted {meta['n_decisions']} decisions from {meta['n_rounds']} rounds "
          f"(k={meta['k']}, char_filter={meta['char_filter']})")
    print(f"  created: {meta['created']}")
    print("\ncoverage (drill list — demonstrate more of anything flagged):")
    counts = meta["bucket_counts"]
    for b in ["defense", "offense", "air", "corner", "neutral"]:
        n = counts.get(b, 0)
        flag = "  <-- record more!" if n < MIN_EXAMPLES else ""
        print(f"  {b:<9} {n:>6}{flag}")
    print("\nattack-label distribution:", meta["attack_label_counts"])
    print("move-label distribution:  ", meta["move_label_counts"])


def main():
    ap = argparse.ArgumentParser(prog="shadow_train")
    sub = ap.add_subparsers(dest="cmd", required=True)

    common = dict(nargs="+", type=Path)
    p_fit = sub.add_parser("fit", help="fit a model and persist it to disk")
    p_fit.add_argument("recordings", **common)
    p_fit.add_argument("--out", type=Path, required=True,
                        help="output model directory, e.g. shadow/models/footee/")
    p_fit.add_argument("--char", type=int, default=None,
                        help="me character id filter (per-char models, SPEC §6)")
    p_fit.add_argument("--k", type=int, default=15)
    p_fit.set_defaults(func=cmd_fit)

    p_eval = sub.add_parser("eval", help="build + round-holdout evaluate")
    p_eval.add_argument("recordings", **common)
    p_eval.add_argument("--char", type=int, default=None,
                         help="me character id filter (per-char models, SPEC §6)")
    p_eval.add_argument("--k", type=int, default=15)
    p_eval.add_argument("--holdout", type=float, default=0.2)
    p_eval.set_defaults(func=cmd_eval)

    p_report = sub.add_parser("report", help="print the coverage drill list for a fitted model")
    p_report.add_argument("model_dir", type=Path)
    p_report.set_defaults(func=cmd_report)

    args = ap.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
