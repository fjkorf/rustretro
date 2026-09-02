"""`SegmentsConfig` — all segmentation/similarity/retrieval knobs as DATA,
loaded from `library/<family>/segments.json`.

Every threshold the segment engine uses is here, never a literal in code
(CLAUDE.md's "per-game knowledge is DATA" law; the measurement laws' "every
scaling bug has been a default nobody wrote as a decision"). A missing key is
a LOAD ERROR, not a silent default — the config must state every choice.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path


class SegmentsConfigError(ValueError):
    pass


# Required keys per section. Kept explicit (not derived from a dataclass) so
# the error message can name exactly what a game's segments.json is missing.
_BOUNDARY_KEYS = (
    "seg_max",            # frames: hard cap so a passive stretch still yields units
    "y_eps",              # px: |y - resting_y| above this = airborne
    "knockdown_window",   # frames: a wakeup boundary needs a contact within this
    "dist_close",         # px: engaged when dist_x < this
    "dist_far",           # px: neutral re-entry when dist_x > this for `neutral_hold`
    "neutral_hold",       # frames: consecutive PRESENT far frames to fire re-entry
    "start_slack",        # rows: shift a boundary forward up to this to find x
)
_SIMILARITY_KEYS = (
    "select_band",                  # pick randomly within (1+band)*best
    "match_floor",                  # best score above this = off-distribution
    "stickiness_bias",              # continue current segment while deviation <= this
    "swap_debounce",                # frames of confirmed opposite facing to re-latch
    "recency_half_life",            # segments: recency weight half-life in recency_rank
    "boundary_kind_mismatch_penalty",  # added to score when boundary kinds differ
    "weights",                      # {feature_name: weight}
)


@dataclass(frozen=True)
class SegmentsConfig:
    family: str
    boundaries: dict
    similarity: dict

    @property
    def weights(self) -> dict:
        return self.similarity["weights"]

    @classmethod
    def load(cls, family_dir: str | Path) -> "SegmentsConfig":
        d = Path(family_dir)
        path = d / "segments.json"
        if not path.exists():
            raise SegmentsConfigError(
                f"{path} not found — segmentation needs a per-family "
                f"segments.json (no code defaults; every knob is DATA)"
            )
        raw = json.loads(path.read_text())
        boundaries = raw.get("boundaries")
        similarity = raw.get("similarity")
        if not isinstance(boundaries, dict):
            raise SegmentsConfigError(f"{path}: missing 'boundaries' object")
        if not isinstance(similarity, dict):
            raise SegmentsConfigError(f"{path}: missing 'similarity' object")
        missing_b = [k for k in _BOUNDARY_KEYS if k not in boundaries]
        missing_s = [k for k in _SIMILARITY_KEYS if k not in similarity]
        if missing_b:
            raise SegmentsConfigError(f"{path}: boundaries missing {missing_b}")
        if missing_s:
            raise SegmentsConfigError(f"{path}: similarity missing {missing_s}")
        if not isinstance(similarity["weights"], dict) or not similarity["weights"]:
            raise SegmentsConfigError(f"{path}: similarity.weights must be a non-empty object")
        return cls(family=d.name, boundaries=boundaries, similarity=similarity)
