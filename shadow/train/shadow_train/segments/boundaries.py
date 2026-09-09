"""Segment boundary detectors (SEGMENT_SHADOW.md §2).

Boundaries are placed at events keyed on values the recording already stores
— no new RE, but a NEW offline pass (`dataset.py` does not compute these).
Every threshold is DATA from `library/<family>/segments.json`; nothing here
hardcodes a number. All detectors honor ABSENT fields (MK2's `x`/`y` are
`via: object_ptr` and omitted, never zero, on a stale-pointer frame — absent
is not zero): an absent value is "no information", never a 0 to compare.

Each detector takes the round's controllable rows and the demonstrator side
(1 = block1, 2 = block2 — the P1-anchor rule) and returns the row indices at
which a NEW segment starts. `boundaries_for_round` unions them, plus the
implicit round-start at row 0, and reports which detectors could not run so
the caller can name them in the index meta (`detectors_unavailable`).
"""

from __future__ import annotations

from collections import Counter
from dataclasses import dataclass

from ..dataset import fields

WAKEUP = "wakeup"
CONTACT = "contact"
NEUTRAL = "neutral"
ROUND_START = "round_start"
CAP = "cap"


@dataclass(frozen=True)
class Boundary:
    row: int      # index into the round's rows where a new segment STARTS
    kind: str


def _blocks(side: int) -> tuple[str, str]:
    """(me_block, opp_block) names for the demonstrator side."""
    return ("block1", "block2") if side == 1 else ("block2", "block1")


def _health(row: dict, block: str):
    return fields(row, block).get("health")


def _xy(row: dict, block: str, name: str):
    """x/y for a block, or None when absent (stale object pointer)."""
    return fields(row, block).get(name)


def has_field(view, name: str) -> bool:
    return name in view.field_names


def contact_boundaries(rows: list[dict]) -> list[int]:
    """A boundary wherever EITHER fighter's struct health decreases — the
    shipped contact_signal (decrease-only, so refill/regen never fires it;
    mk2.md). Health is a static struct field, always present."""
    out = []
    for i in range(1, len(rows)):
        for block in ("block1", "block2"):
            h0, h1 = _health(rows[i - 1], block), _health(rows[i], block)
            if h0 is not None and h1 is not None and h1 < h0:
                out.append(i)
                break
    return out


def wakeup_boundaries(rows: list[dict], side: int, cfg: dict) -> list[int]:
    """A boundary at an airborne→grounded transition of the demonstrator that
    was preceded by a contact within `knockdown_window` frames — a wakeup.
    Resting-y is estimated per round as the mode of PRESENT y values (there is
    no scalar GROUND_Y; it is character- and stage-dependent). An absent y at
    the transition frame yields no boundary (never synthesize). The caller
    must only invoke this when y is a mapped field."""
    me, _ = _blocks(side)
    ys = [_xy(r, me, "y") for r in rows]
    present = [y for y in ys if y is not None]
    if not present:
        return []
    resting = Counter(present).most_common(1)[0][0]
    y_eps = cfg["y_eps"]

    def airborne(i: int):
        y = ys[i]
        return None if y is None else abs(y - resting) > y_eps

    contacts = set(contact_boundaries(rows))
    window = cfg["knockdown_window"]
    out = []
    for i in range(1, len(rows)):
        a0, a1 = airborne(i - 1), airborne(i)
        if a0 is True and a1 is False:  # landed (both frames' y present)
            f_now = rows[i]["frame"]
            if any(f_now - rows[c]["frame"] <= window and rows[c]["frame"] <= f_now
                   for c in contacts):
                out.append(i)
    return out


def neutral_boundaries(rows: list[dict], side: int, cfg: dict) -> list[int]:
    """A boundary when the fighters DISENGAGE — dist_x crosses from close to
    far and holds far. Hysteresis: engage when dist < dist_close; fire when
    engaged AND dist > dist_far for `neutral_hold` consecutive PRESENT frames.
    An absent-x frame FREEZES the counter (no information — neither reset nor
    advance), never resets it (that would treat absence as engagement)."""
    me, opp = _blocks(side)
    d_close, d_far, hold = cfg["dist_close"], cfg["dist_far"], cfg["neutral_hold"]
    engaged = False
    far_run = 0
    out = []
    for i, r in enumerate(rows):
        xm, xo = _xy(r, me, "x"), _xy(r, opp, "x")
        if xm is None or xo is None:
            continue  # absence freezes both `engaged` and `far_run`
        dist = abs(xm - xo)
        if dist < d_close:
            engaged = True
            far_run = 0
        elif dist > d_far:
            far_run += 1
            if engaged and far_run >= hold:
                out.append(i)
                engaged = False
                far_run = 0
        else:
            far_run = 0
    return out


def boundaries_for_round(rows: list[dict], view, side: int, cfg: dict):
    """Union every detector's boundaries (plus the implicit round-start at row
    0) into a sorted, deduped list of `Boundary`, and report which detectors
    could not run. `seg_max` capping is applied later, during tiling.

    Returns (boundaries, detectors_unavailable) where detectors_unavailable is
    a set of detector names skipped because a required field is unmapped for
    this port (named in the index meta so `—` never silently means "ran, found
    nothing")."""
    b = cfg["boundaries"] if "boundaries" in cfg else cfg
    marks: dict[int, str] = {0: ROUND_START}
    unavailable: set[str] = set()

    for i in contact_boundaries(rows):
        marks.setdefault(i, CONTACT)

    if has_field(view, "y"):
        for i in wakeup_boundaries(rows, side, b):
            marks.setdefault(i, WAKEUP)
    else:
        unavailable.add(WAKEUP)

    if has_field(view, "x"):
        for i in neutral_boundaries(rows, side, b):
            marks.setdefault(i, NEUTRAL)
    else:
        unavailable.add(NEUTRAL)

    ordered = [Boundary(row=i, kind=marks[i]) for i in sorted(marks)]
    return ordered, unavailable
