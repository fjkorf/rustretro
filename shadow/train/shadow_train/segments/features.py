"""start_features for a segment — the ADDITIVE layer over dataset's scalar
vector (SEGMENT_SHADOW.md §3).

The scalar half is `dataset.decision_scalars` verbatim (the ONE feature
implementation — the segment engine never ships a second copy that could
drift from the kNN's). This module only resolves the me/opp/history inputs at
a segment's START row the same way `_decisions_for_round` resolves them at a
decision, so the values match frame-for-frame (the parity test guards it).

`boundary_kind` is NOT folded into the feature dict — it is a `Segment` field
compared separately in retrieval (a mismatch penalty), so the scalar vector
stays exactly the kNN's and absent-is-not-zero is preserved (an unavailable
feature is simply missing from the dict).
"""

from __future__ import annotations

from .. import dataset as _ds


def demonstrator_mask_key(rows: list[dict], side: int) -> str:
    """Which per-frame mask is the demonstrator's. `side` is the block number
    (1/2) that demonstrated; p1 controls block `p1_block`, so if the segment's
    side is p1's block the demonstrator is p1 (`p1_input`), else p2
    (`p2_input`). Human-vs-CPU recordings only ever yield p1-side segments
    (the CPU's mask is all zeros — the demo filter), but this keeps the
    resolution correct for a human-vs-human corpus too."""
    p1_block = rows[0]["p1_block"]
    return "p1_input" if side == p1_block else "p2_input"


def start_features_for(rows: list[dict], view, side: int, row_idx: int):
    """Named start-feature dict for a segment beginning at `rows[row_idx]`
    with demonstrator `side` (block number). Returns None when a
    pointer-resolved field (MK2 x/y) is ABSENT at this row on either fighter —
    the spatial features cannot be computed, so the caller drops-and-counts
    the segment rather than zero-filling (absent is not zero)."""
    me_block = "block1" if side == 1 else "block2"
    opp_block = "block2" if side == 1 else "block1"
    me = _ds.fields(rows[row_idx], me_block)
    opp = _ds.fields(rows[max(0, row_idx - _ds.STALE)], opp_block)

    ptr = view.pointer_resolved_fields
    if ptr and (any(f not in me for f in ptr) or any(f not in opp for f in ptr)):
        return None

    feats = _ds.scalar_features_for(view)
    mask_key = demonstrator_mask_key(rows, side)
    hist = [rows[j][mask_key] for j in range(max(0, row_idx - _ds.P), row_idx)]

    me_hs = opp_hs = False
    if "me_hitstun" in feats and view.hitstun_map:
        hmap = view.hitstun_map
        b1 = _ds._recent_change_mask(rows, hmap["block1"], _ds.HITSTUN_RECENT_FRAMES)
        b2 = _ds._recent_change_mask(rows, hmap["block2"], _ds.HITSTUN_RECENT_FRAMES)
        me_active = b1 if side == 1 else b2
        opp_active = b2 if side == 1 else b1
        me_hs = bool(me_active[row_idx])
        opp_hs = bool(opp_active[max(0, row_idx - _ds.STALE)])

    scal, _s, _fb, _bb = _ds.decision_scalars(
        view, feats, me, opp, hist, me_hitstun=me_hs, opp_hitstun=opp_hs)
    return scal
