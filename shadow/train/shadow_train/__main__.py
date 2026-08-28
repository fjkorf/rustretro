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
      -- no recordings needed, just what's already on disk. Also prints a
      MACRO_ACTIONS.md §8 string/juggle summary read from the source
      recordings' `.rounds.jsonl` sidecars, when any carry one.

  coverage [recordings...]
      Print the MATCHUP coverage matrix (decisions per me-char x opp-char,
      demo rounds excluded, exactly as fit would count them) plus a per-style
      breakdown from the recording meta sidecars. Defaults to
      shadow/recordings/<family>/*.jsonl (v2 only). The fill-the-gaps view.

  index [recordings...] [--force]
      Backfill .rounds.jsonl sidecars (the per-round matchup index the app's
      recorder writes for new recordings) for recordings that lack one.
      Same schema as src/record.rs emit_round_summary.
"""

import argparse
import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path

from . import dataset
from . import profile as _profile
from .dataset import ATTACK_CLASSES, MOVE_CLASSES, SCALAR_FEATURES, build
from .evaluate import MIN_EXAMPLES, evaluate, print_report
from .knn import KnnPolicy

META_FILE = "meta.json"

# Calibration key set: whatever the profile's `calibration` block names
# (docs/game-profiles.md) -- dataset.py exports a module attribute of the
# same name for each one. This used to be a hardcoded list mirroring
# dataset.py's constants; now it's the profile's own key order (which is
# where those constants come from), so a game/port with a different
# calibration set doesn't need this file touched. Refreshed by
# _resolve_profile_for() below whenever fit-time auto-resolution swaps in a
# different profile than whatever was loaded at import.
CALIBRATION_KEYS = list(_profile.get().calibration.keys())


def _resolve_profile_for(paths: list[Path], game_arg: str | None) -> None:
    """Fit-time profile auto-resolution (footgun fix, see CLAUDE.md "The
    shadow loop"). jsonl-v3 recordings are self-describing: each `.meta.json`
    sidecar carries `family`/`port`/`profile_file` (RECORDER_V3.md §1.3). A
    fit used to always label attacks/moves with whatever profile
    RUSTRETRO_GAME_DIR happened to name (default asurabld) -- fine for
    cross-family mistakes (dataset.py's family-mismatch guard aborts those),
    silent for same-family/wrong-port ones (e.g. mk2 arcade loaded while
    fitting mk2 Genesis recordings), since arcade and Genesis share one
    family.json and the guard never looks at `port`.

    Precedence: `--game` / RUSTRETRO_GAME_DIR is an explicit OVERRIDE -- if
    given, it wins outright (a loud warning fires if it disagrees with what
    the recordings' own sidecars say, but the override is still obeyed; the
    family-mismatch guard remains the backstop for a badly wrong override).
    Otherwise every v3 sidecar found among `paths` must agree on
    (family, port); the resolved profile is `library/<family>/<profile_file
    stem>`, loaded via the existing `--game`-path-segment mechanism (RECORDER_
    V3.md §5.2), so `library/mk2/genesis.profile.json` resolves the same way
    `--game library/mk2/genesis` would. v2 files (no sidecar) don't
    participate -- this whole function no-ops when none of `paths` carries a
    v3 sidecar, which is today's exact behavior (loaded profile / asurabld
    default)."""
    sidecars: dict[Path, tuple] = {}
    for p in paths:
        if p.name.endswith(".rounds.jsonl"):
            continue
        meta_path = Path(str(p).removesuffix(".jsonl") + ".meta.json")
        try:
            meta = json.loads(meta_path.read_text())
        except (OSError, json.JSONDecodeError):
            continue  # v2, or a v3 file missing its sidecar -- not resolvable
        if meta.get("format") != "jsonl-v3":
            continue
        sidecars[p] = (meta.get("family"), meta.get("port"), meta.get("profile_file"))

    resolved = None
    if sidecars:
        uniq = set(sidecars.values())
        if len(uniq) > 1:
            detail = "; ".join(
                f"{p.name} (family={fam!r} port={port!r})"
                for p, (fam, port, _pf) in sorted(sidecars.items())
            )
            raise SystemExit(
                "recordings disagree on profile (family/port) -- fit one "
                f"profile at a time, or pass --game to force one: {detail}"
            )
        resolved = next(iter(uniq))  # (family, port, profile_file)

    override = game_arg or os.environ.get("RUSTRETRO_GAME_DIR")
    if override:
        if resolved is not None:
            fam, port, _pf = resolved
            try:
                ov_prof = _profile.load(Path(override))
            except _profile.ProfileError as e:
                raise SystemExit(f"--game {override}: {e}") from e
            if (ov_prof.family, ov_prof.port) != (fam, port):
                print(
                    f"WARNING: --game/RUSTRETRO_GAME_DIR override "
                    f"({ov_prof.family}/{ov_prof.port}) disagrees with the "
                    f"recordings' own profile ({fam}/{port}) -- obeying the "
                    "override because it is explicit",
                    file=sys.stderr,
                )
        os.environ["RUSTRETRO_GAME_DIR"] = str(override)
    elif resolved is not None:
        fam, port, profile_file = resolved
        stem = profile_file.removesuffix(".profile.json") if profile_file else port
        os.environ["RUSTRETRO_GAME_DIR"] = str(_profile.REPO_ROOT / "library" / fam / stem)
    # else: no v3 sidecars and no override -- leave the process default alone.

    dataset.reload_profile()
    global CALIBRATION_KEYS
    CALIBRATION_KEYS = list(_profile.get().calibration.keys())


def _bucket_counts(data: dict) -> dict:
    from collections import Counter

    return dict(Counter(data["buckets"].tolist()))


def _label_counts(y, classes: list[str]) -> dict:
    import numpy as np

    counts = np.bincount(y, minlength=len(classes))
    return {classes[i]: int(n) for i, n in enumerate(counts) if n}


def cmd_fit(args) -> None:
    opp = getattr(args, "opp", None)  # absent in hand-built Namespaces (tests)
    _resolve_profile_for(args.recordings, getattr(args, "game", None))
    features = getattr(args, "features", None)
    restrict = frozenset(f.strip() for f in features.split(",")) if features else None
    data = build(args.recordings, char_filter=args.char, opp_filter=opp, restrict=restrict)
    n_raw = len(data["X"])
    neutral_cap = getattr(args, "neutral_cap", dataset.NEUTRAL_CAP_RATIO)
    data = dataset.subsample_neutral(data, cap_ratio=neutral_cap)
    if len(data["X"]) < n_raw:
        print(f"neutral cap: {n_raw} -> {len(data['X'])} decisions "
              f"(idle capped at {neutral_cap}x active)")
    policy = KnnPolicy(k=args.k).fit(data["X"], data["y_move"], data["y_attack"])

    out_dir = Path(args.out)
    policy.save(out_dir)
    prof = _profile.get()
    # §4.3: uniform port -> that port (today's behavior); mixed -> "mixed" +
    # the ports list, so deploy's port-mismatch warning can treat it as
    # matching every port of the family.
    ports = data.get("ports") or [prof.port]
    # MACRO_ACTIONS.md §4: the specials appended onto ATTACK_CLASSES beyond
    # this profile's own family attack_classes -- stored so `report` (which
    # only ever reads a persisted meta.json, no recordings/profile reload)
    # can print a dedicated specials line without re-deriving the family/
    # port split from scratch. Omitted entirely (not an empty list) for any
    # family shipping no `moves` table (asurabld) -- keeps meta.json's key
    # set byte-for-byte what it always was for the G1 golden gate.
    specials = [n for n in ATTACK_CLASSES if n not in prof.attack_classes]
    meta = {
        "feature_names": data.get("feature_names", SCALAR_FEATURES),
        "calibration": {name: getattr(dataset, name) for name in CALIBRATION_KEYS},
        "move_classes": MOVE_CLASSES,
        "attack_classes": ATTACK_CLASSES,
        **({"specials": specials} if specials else {}),
        "family": prof.family,
        "port": ports[0] if len(ports) == 1 else "mixed",
        **({"ports": ports} if len(ports) > 1 else {}),
        "k": policy.k,
        "temperature": policy.temperature,
        "neutral_cap": neutral_cap,
        "char_filter": args.char,
        "opp_filter": opp,
        "matchup": prof.matchup_slug(args.char, opp),
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
    _resolve_profile_for(args.recordings, getattr(args, "game", None))
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
    # MACRO_ACTIONS.md §4: a dedicated specials line -- omitted entirely for
    # a model with no specials in its label space (asurabld today), never
    # printed as an empty/zeroed line (per-feature degradation, house style).
    specials = meta.get("specials") or []
    if specials:
        counts = meta.get("attack_label_counts", {})
        print("specials:                 ", {name: counts.get(name, 0) for name in specials})
    # MACRO_ACTIONS.md §8: string/juggle stats aggregated from the
    # `.rounds.jsonl` sidecars of every recording that fed this model --
    # these are per-round facts the RUST recorder wrote, not fit-time
    # features, so `report` reads them straight off disk rather than through
    # meta.json. Omitted entirely when no round carries a `strings` object
    # (unmapped contact source, or nothing recorded yet) -- never a zeroed
    # line for an unaffected game (same per-feature-degradation house style
    # as the specials line above).
    strings_line = _string_stats_line(meta.get("source_files", []))
    if strings_line:
        print(strings_line)


def _string_stats_line(source_files: list) -> str | None:
    """Aggregate MACRO_ACTIONS.md §8 per-round `strings` objects across the
    `.rounds.jsonl` sidecar of every recording in `source_files`. Returns
    None when no round anywhere carries a `strings` object."""
    total = 0
    longest_hits = 0
    longest_damage = 0
    block_strings = 0
    juggle_hits = 0
    juggle_seen = False
    seen_any = False
    for f in source_files:
        sidecar = Path(str(f).removesuffix(".jsonl") + ".rounds.jsonl")
        if not sidecar.exists():
            continue
        for line in sidecar.read_text().splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            s = row.get("strings")
            if not s:
                continue
            seen_any = True
            total += s.get("count", 0)
            if s.get("longest_hits", 0) > longest_hits:
                longest_hits = s["longest_hits"]
                longest_damage = s.get("longest_damage", 0)
            block_strings += s.get("block_strings", 0)
            if "juggle_hits" in s:
                juggle_seen = True
                juggle_hits += s["juggle_hits"]
    if not seen_any:
        return None
    line = f"strings: {total} (longest {longest_hits} hits / {longest_damage} dmg)"
    if block_strings:
        line += f" · block strings: {block_strings}"
    if juggle_seen:
        line += f" · juggle hits: {juggle_hits}"
    return line


def _recording_style(p: Path) -> str | None:
    """The style tag the recorder wrote into the .meta.json sidecar."""
    mp = Path(str(p).removesuffix(".jsonl") + ".meta.json")
    try:
        return json.loads(mp.read_text()).get("style")
    except (OSError, json.JSONDecodeError):
        return None


def cmd_coverage(args) -> None:
    from collections import Counter

    from . import profile as game_profile
    if args.recordings:
        _resolve_profile_for(args.recordings, getattr(args, "game", None))
    fam = game_profile.get().family
    recs = args.recordings or sorted(Path("shadow/recordings").joinpath(fam).glob("*.jsonl"))
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
    name = _profile.get().char_name
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
    from . import profile as game_profile
    if args.recordings:
        _resolve_profile_for(args.recordings, getattr(args, "game", None))
    prof = game_profile.get()
    fam = prof.family
    recs = args.recordings or sorted(Path("shadow/recordings").joinpath(fam).glob("*.jsonl"))
    done = skipped = 0
    for p in recs:
        if str(p).endswith(".rounds.jsonl"):
            continue
        try:
            version = dataset._detect_version(p)
        except SystemExit:
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
                b1, b2 = dataset.fields(r, "block1"), dataset.fields(r, "block2")
                # §1.4: block1_char/block2_char are CANONICAL ids (§6),
                # translated at write time; null (never 0) if unmapped.
                b1_char = prof.canon_char_id(b1["char_id"]) if "char_id" in b1 else None
                b2_char = prof.canon_char_id(b2["char_id"]) if "char_id" in b2 else None
                cur = {"round_id": r["round_id"],
                       "block1_char": b1_char,
                       "block2_char": b2_char,
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
                if version == "v3":
                    row["family"] = fam
                    row["port"] = prof.port
                    row["v"] = 3
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
    p_fit.add_argument("--features", type=str, default=None,
                       help="comma-separated feature names to restrict the fit to "
                            "(canonical order kept; aborts if a recording can't "
                            "supply one) — use the ports' shared subset for a "
                            "cross-port-deployable model")
    p_fit.add_argument("--game", type=str, default=None,
                        help="force this game/port profile (overrides the "
                             "recordings' own v3 sidecars and RUSTRETRO_GAME_DIR; "
                             "e.g. library/mk2/genesis)")
    p_fit.set_defaults(func=cmd_fit)

    p_eval = sub.add_parser("eval", help="build + round-holdout evaluate")
    p_eval.add_argument("recordings", **common)
    p_eval.add_argument("--char", type=int, default=None,
                         help="me character id filter (per-char models, SPEC §6)")
    p_eval.add_argument("--opp", type=int, default=None,
                         help="opponent character id filter (per-matchup models)")
    p_eval.add_argument("--k", type=int, default=15)
    p_eval.add_argument("--holdout", type=float, default=0.2)
    p_eval.add_argument("--game", type=str, default=None,
                         help="force this game/port profile (see fit --game)")
    p_eval.set_defaults(func=cmd_eval)

    p_report = sub.add_parser("report", help="print the coverage drill list for a fitted model")
    p_report.add_argument("model_dir", type=Path)
    p_report.set_defaults(func=cmd_report)

    p_cov = sub.add_parser("coverage",
                           help="matchup coverage matrix across recordings")
    p_cov.add_argument("recordings", nargs="*", type=Path,
                       help="recordings to scan (default: shadow/recordings/<family>/*.jsonl)")
    p_cov.add_argument("--game", type=str, default=None,
                       help="force this game/port profile (see fit --game)")
    p_cov.set_defaults(func=cmd_coverage)

    p_idx = sub.add_parser("index",
                           help="backfill .rounds.jsonl matchup-index sidecars")
    p_idx.add_argument("recordings", nargs="*", type=Path)
    p_idx.add_argument("--force", action="store_true",
                       help="rewrite sidecars that already exist")
    p_idx.add_argument("--game", type=str, default=None,
                       help="force this game/port profile (see fit --game)")
    p_idx.set_defaults(func=cmd_index)

    args = ap.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
