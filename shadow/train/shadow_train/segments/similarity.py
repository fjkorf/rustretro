"""Weighted similarity + retrieval over segment start-features
(SEGMENT_SHADOW.md §3/§4).

Three deliberate departures from the kNN, all from KI:
1. A WEIGHTED L1 metric (not uniform L2) — categorical/knockdown states
   weighted far above raw position, so defense is not swamped (§3.1). Weights
   are DATA (segments.json).
2. Matchup-partitioned retrieval with an exact->per-char->per-opp->general
   fallback, and the answering TIER surfaced (§9.5) — never hidden.
3. Selection NOISE: pick randomly within a band of the best score (§3.3), and
   NO-MATCH HONESTY: if the best score is beyond match_floor the shadow is
   off-distribution — flagged, never fabricated (§4.3).

Pure functions with an injected RNG, so wave-(b) Rust golden-matches them.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

# Float-robustness epsilon for the selection band, so best==0 (a perfect
# match) doesn't collapse the band to a single element via float equality.
_BAND_EPS = 1e-9


@dataclass(frozen=True)
class ScoreResult:
    score: float
    n_used: int       # features compared (present on BOTH sides)
    n_skipped: int    # features a weight named but one side lacked


def score(a: dict, b: dict, weights: dict, scales: dict | None = None,
          *, boundary_penalty: float = 0.0) -> ScoreResult:
    """Weighted L1 over features present on BOTH sides:
    Σ w_f · |a_f − b_f| / scale_f, plus `boundary_penalty`. A feature absent
    on either side contributes NOTHING (it is not renormalized away silently —
    n_skipped surfaces how many were missing, for honesty)."""
    scales = scales or {}
    total = 0.0
    n_used = n_skipped = 0
    for f, w in weights.items():
        if f in a and f in b:
            sc = scales.get(f) or 1.0
            total += w * abs(a[f] - b[f]) / sc
            n_used += 1
        else:
            n_skipped += 1
    return ScoreResult(score=total + boundary_penalty, n_used=n_used, n_skipped=n_skipped)


@dataclass(frozen=True)
class Retrieval:
    segment: object | None   # the chosen Segment, or None if the pool is empty
    score: float             # the BEST score in the answering tier
    tier: str                # exact | per-char | per-opp | general | none
    off_distribution: bool   # best > match_floor -> "improvising", surfaced
    band_size: int


_TIERS = ("exact", "per-char", "per-opp", "general")


def _tier_pred(tier: str, me_char, opp_char):
    if tier == "exact":
        return lambda s: s.char_id == me_char and s.opp_char_id == opp_char
    if tier == "per-char":
        return lambda s: s.char_id == me_char
    if tier == "per-opp":
        return lambda s: s.opp_char_id == opp_char
    return lambda s: True


def _recency_weight(recency_rank: int, half_life: float) -> float:
    # rank 0 = most recent -> weight 1; decays by half every `half_life` ranks.
    return 0.5 ** (recency_rank / half_life) if half_life > 0 else 1.0


def retrieve(query_feats: dict, query_kind: str, me_char, opp_char,
             segments: list, cfg: dict, rng, scales: dict | None = None) -> Retrieval:
    """Retrieve one segment for the live situation. `cfg` is the
    SegmentsConfig.similarity dict (weights, select_band, match_floor,
    recency_half_life, boundary_kind_mismatch_penalty). `rng` is an injected
    random.Random (seeded) so selection is reproducible/golden-matchable."""
    weights = cfg["weights"]
    penalty = cfg["boundary_kind_mismatch_penalty"]
    band_frac = cfg["select_band"]
    half_life = cfg["recency_half_life"]

    cands = []
    tier = "none"
    for t in _TIERS:
        pred = _tier_pred(t, me_char, opp_char)
        cands = [s for s in segments if pred(s)]
        if cands:
            tier = t
            break
    if not cands:
        return Retrieval(None, math.inf, "none", True, 0)

    scored = []
    for s in cands:
        pen = penalty if s.boundary_kind != query_kind else 0.0
        r = score(query_feats, s.start_features, weights, scales, boundary_penalty=pen)
        scored.append((r.score, s))
    best = min(sc for sc, _ in scored)

    cutoff = best + max(best * band_frac, _BAND_EPS)
    band = [s for sc, s in scored if sc <= cutoff]

    weights_b = [_recency_weight(s.recency_rank, half_life) for s in band]
    pick = _weighted_choice(band, weights_b, rng)
    off = best > cfg["match_floor"]
    return Retrieval(pick, best, tier, off, len(band))


def _weighted_choice(items: list, weights: list, rng):
    """Deterministic given a seeded rng: cumulative-weight draw. Falls back to
    uniform if all weights are zero (degenerate half_life)."""
    if not items:
        return None
    total = sum(weights)
    if total <= 0:
        return items[rng.randrange(len(items))]
    x = rng.random() * total
    acc = 0.0
    for it, w in zip(items, weights):
        acc += w
        if x < acc:
            return it
    return items[-1]
