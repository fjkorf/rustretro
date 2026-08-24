"""CLI: python -m shadow_train <recordings...> [--char N] [--k K] [--holdout F]

Builds the decision dataset from jsonl-v2 recordings, fits the kNN baseline,
and prints the §7.4 evaluation + §7.6 coverage report.
"""

import argparse
from pathlib import Path

from .dataset import build
from .evaluate import evaluate, print_report


def main():
    ap = argparse.ArgumentParser(prog="shadow_train")
    ap.add_argument("recordings", nargs="+", type=Path)
    ap.add_argument("--char", type=int, default=None,
                    help="me character id filter (per-char models, SPEC §6)")
    ap.add_argument("--k", type=int, default=15)
    ap.add_argument("--holdout", type=float, default=0.2)
    args = ap.parse_args()

    data = build(args.recordings, char_filter=args.char)
    print(f"decisions: {len(data['X'])} from {len(set(data['rounds']))} rounds "
          f"({data['X'].shape[1]} features)")
    report = evaluate(data, k=args.k, holdout_frac=args.holdout)
    print_report(report, data)


if __name__ == "__main__":
    main()
