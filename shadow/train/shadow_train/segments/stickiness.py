"""The stickiness bias — continue the current segment until the world
diverges, then re-select (SEGMENT_SHADOW.md §4).

`deviation` is `similarity.score` on the current-offset pair (ONE metric, two
uses — no second implementation). A segment keeps playing while its deviation
from the live world stays <= the stickiness_bias; it "naturally breaks" the
first frame deviation EXCEEDS the bias (KI's exact mechanism). Absent-feature
frames yield None deviation (skipped — the latch holds), never 0, which would
read as a perfect match and pin the segment forever.
"""

from __future__ import annotations

from . import similarity


def deviation(live_feats: dict, seg_feats: dict, weights: dict,
              scales: dict | None = None):
    """Weighted deviation between the live world and the segment's recorded
    world at the corresponding offset. Returns None when the two share no
    scored feature (no information — the caller holds, never treats it as 0)."""
    r = similarity.score(live_feats, seg_feats, weights, scales)
    if r.n_used == 0:
        return None
    return r.score


def should_continue(dev, bias: float) -> bool:
    """Continue while deviation <= bias; break on strict >. A None deviation
    (no shared feature this frame) continues — the latch holds."""
    if dev is None:
        return True
    return dev <= bias


def walk_segment(seg_feats_by_offset, live_feats_by_offset, weights, bias,
                 scales: dict | None = None):
    """Offline replay of the decision cadence: walk offsets 0,1,2,... and
    return (outcome, offset) — ("break", i) at the first offset whose
    deviation exceeds `bias`, or ("exhausted", n) if the segment plays to the
    end. This is the §8.2 harness and the reference wave-(b) Rust golden-
    matches; it must break neither early (absorbing) nor late (robotic)."""
    n = min(len(seg_feats_by_offset), len(live_feats_by_offset))
    for i in range(n):
        dev = deviation(live_feats_by_offset[i], seg_feats_by_offset[i], weights, scales)
        if not should_continue(dev, bias):
            return ("break", i)
    return ("exhausted", n)
