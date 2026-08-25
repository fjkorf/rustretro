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

  coverage [recordings...]
      Print the MATCHUP coverage matrix (decisions per me-char x opp-char,
      demo rounds excluded, exactly as fit would count them) plus a per-style
      breakdown from the recording meta sidecars. Defaults to
      shadow/recordings/*.jsonl (v2 only). The fill-the-gaps view.

  index [recordings...] [--force]
      Backfill .rounds.jsonl sidecars (the per-round matchup index the app's
      recorder writes for new recordings) for recordings that lack one.
      Same schema as src/record.rs emit_round_summary.
"""

import argparse
import json
from datetime import datetime, timezone
from pathlib import Path

from . import asurabld, dataset
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
    opp = getattr(args, "opp", None)  # absent in hand-built Namespaces (tests)
    data = build(args.recordings, char_filter=args.char, opp_filter=opp)
    n_raw = len(data["X"])
    neutral_cap = getattr(args, "neutral_cap", dataset.NEUTRAL_CAP_RATIO)
    data = dataset.subsample_neutral(data, cap_ratio=neutral_cap)
    if len(data["X"]) < n_raw:
        print(f"neutral cap: {n_raw} -> {len(data['X'])} decisions "
              f"(idle capped at {neutral_cap}x active)")
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
        "neutral_cap": neutral_cap,
        "char_filter": args.char,
        "opp_filter": opp,
        "matchup": asurabld.matchup_slug(args.char, opp),
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
    data = build(args.recordings, char_filter=args.char,
                 opp_filter=getattr(args, "opp", None))
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
          f"(k={meta['k']}, matchup={meta.get('matchup', 'all')})")
    print(f"  created: {meta['created']}")
    print("\ncoverage (drill list — demonstrate more of anything flagged):")
    counts = meta["bucket_counts"]
    for b in ["defense", "offense", "air", "corner", "neutral"]:
        n = counts.get(b, 0)
        flag = "  <-- record more!" if n < MIN_EXAMPLES else ""
        print(f"  {b:<9} {n:>6}{flag}")
    print("\nattack-label distribution:", meta["attack_label_counts"])
    print("move-label distribution:  ", meta["move_label_counts"])


def _recording_style(p: Path) -> str | None:
    """The style tag the recorder wrote into the .meta.json sidecar."""
    mp = Path(str(p).removesuffix(".jsonl") + ".meta.json")
    try:
        return json.loads(mp.read_text()).get("style")
    except (OSError, json.JSONDecodeError):
        return None


def cmd_coverage(args) -> None:
    from collections import Counter

    recs = args.recordings or sorted(Path("shadow/recordings").glob("*.jsonl"))
    files = []
    for p in recs:
        try:
            if '"block1"' in open(p).readline():
                files.append(p)
        except OSError:
            pass
    if not files:
        raise SystemExit("no v2 recordings found")

    per = Counter()          # (me, opp) -> decisions
    per_style = Counter()    # (me, opp, style) -> decisions
    for p in files:
        style = _recording_style(p) or "(untagged)"
        for d in dataset.load_decisions([p]):
            per[(d.me_char, d.opp_char)] += 1
            per_style[(d.me_char, d.opp_char, style)] += 1

    mes = sorted({m for m, _ in per})
    opps = sorted({o for _, o in per})
    name = asurabld.char_name
    print(f"matchup coverage — decisions per cell, demo rounds excluded "
          f"({len(files)} recording(s))")
    header = " " * 10 + "".join(f"{name(o):>10}" for o in opps)
    print(header)
    for m in mes:
        row = f"{name(m):<10}"
        for o in opps:
            n = per.get((m, o), 0)
            row += f"{n:>10}" if n else f"{'.':>10}"
        print(row)
    styles = {s for _, _, s in per_style}
    if styles - {"(untagged)"}:
        print("\nby style:")
        for (m, o, st), n in sorted(per_style.items(), key=lambda kv: -kv[1]):
            print(f"  {name(m)} vs {name(o):<10} {st:<12} {n:>7}")
    print("\n(cells you have never demonstrated print as '.'; "
          "unnamed characters print as c<N> — see asurabld.CHAR_NAMES)")


def cmd_index(args) -> None:
    recs = args.recordings or sorted(Path("shadow/recordings").glob("*.jsonl"))
    done = skipped = 0
    for p in recs:
        if str(p).endswith(".rounds.jsonl"):
            continue
        try:
            if '"block1"' not in open(p).readline():
                continue
        except OSError:
            continue
        out = Path(str(p).removesuffix(".jsonl") + ".rounds.jsonl")
        if out.exists() and not args.force:
            skipped += 1
            continue
        style = _recording_style(p)
        lines = []
        cur = None  # (round_id, b1c, b2c, p1_block, frames, mass)
        prev_controllable = False
        for line in open(p):
            try:
                r = json.loads(line)
            except json.JSONDecodeError:
                continue
            c = bool(r.get("controllable"))
            if c and not prev_controllable:
                cur = {"round_id": r["round_id"],
                       "block1_char": r["block1"].get("char_id", 0),
                       "block2_char": r["block2"].get("char_id", 0),
                       "p1_block": r.get("p1_block"),
                       "frames": 0, "p1_input_mass": 0}
            if not c and prev_controllable and cur is not None:
                lines.append(cur)
                cur = None
            if c and cur is not None:
                cur["frames"] += 1
                cur["p1_input_mass"] += r.get("p1_input", 0)
            prev_controllable = c
        if cur is not None:  # file ended mid-round
            lines.append(cur)
        with open(out, "w") as f:
            for row in lines:
                row["demo"] = row["p1_input_mass"] == 0
                row["style"] = style
                f.write(json.dumps(row) + chr(10))
        print(f"  {out.name}: {len(lines)} round(s)")
        done += 1
    print(f"indexed {done} recording(s), {skipped} already had sidecars")


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
    p_fit.add_argument("--opp", type=int, default=None,
                        help="opponent character id filter (per-matchup models)")
    p_fit.add_argument("--k", type=int, default=15)
    p_fit.add_argument("--neutral-cap", type=float, default=dataset.NEUTRAL_CAP_RATIO,
                        help="cap idle (Neutral,None) decisions at this ratio x "
                             "active decisions; 0 disables (default %(default)s)")
    p_fit.set_defaults(func=cmd_fit)

    p_eval = sub.add_parser("eval", help="build + round-holdout evaluate")
    p_eval.add_argument("recordings", **common)
    p_eval.add_argument("--char", type=int, default=None,
                         help="me character id filter (per-char models, SPEC §6)")
    p_eval.add_argument("--opp", type=int, default=None,
                         help="opponent character id filter (per-matchup models)")
    p_eval.add_argument("--k", type=int, default=15)
    p_eval.add_argument("--holdout", type=float, default=0.2)
    p_eval.set_defaults(func=cmd_eval)

    p_report = sub.add_parser("report", help="print the coverage drill list for a fitted model")
    p_report.add_argument("model_dir", type=Path)
    p_report.set_defaults(func=cmd_report)

    p_cov = sub.add_parser("coverage",
                           help="matchup coverage matrix across recordings")
    p_cov.add_argument("recordings", nargs="*", type=Path,
                       help="recordings to scan (default: shadow/recordings/*.jsonl)")
    p_cov.set_defaults(func=cmd_coverage)

    p_idx = sub.add_parser("index",
                           help="backfill .rounds.jsonl matchup-index sidecars")
    p_idx.add_argument("recordings", nargs="*", type=Path)
    p_idx.add_argument("--force", action="store_true",
                       help="rewrite sidecars that already exist")
    p_idx.set_defaults(func=cmd_index)

    args = ap.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
