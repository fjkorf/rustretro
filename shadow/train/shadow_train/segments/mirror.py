"""Facing-mirroring — the one novel mechanism the segment shadow needs that
KI's did not (SEGMENT_SHADOW.md §5).

KI replayed the player's OWN side. Ours replays a human-demonstrated side
through whichever side the shadow is on, so a segment recorded on the left
must play correctly on the right: LEFT<->RIGHT swapped.

ONE implementation, consumed by BOTH the injected stream and the DIVERGED
expected stream (the §8.1 validation glue). Two implementations would make
the replay classifier compare a mirror against itself and prove nothing.

Facing on MK2 is DERIVED (sign(opp.x - me.x)) and can be ABSENT on a stale
object pointer — never synthesize 0. Per MACRO_ACTIONS.md §10.2 facing is
PINNED at segment start and re-latched only on a debounced, CONFIRMED side
swap, never resolved per frame. Because playback is LITERAL (the exact
recorded masks), a within-segment crossup rides along automatically — the
deploy fighter presses the mirrored buttons and crosses up at the same
frame — so the mirror is a CONSTANT L<->R swap decided by the start facing;
the recorded trace's internal swaps need no per-frame handling.
"""

from __future__ import annotations

# Bit positions in the 12-bit RETRO mask (matches dataset.BIT_LEFT/RIGHT and
# src/record.rs pack_mask order). Only these two swap under a mirror; Up/Down
# are side-neutral, and MK2's Block is a button (also side-neutral).
BIT_LEFT = 6
BIT_RIGHT = 7


def mirror_mask(mask: int) -> int:
    """Swap the LEFT and RIGHT bits of a 12-bit mask, leaving every other bit
    untouched. An involution: mirror_mask(mirror_mask(m)) == m."""
    left = (mask >> BIT_LEFT) & 1
    right = (mask >> BIT_RIGHT) & 1
    mask &= ~((1 << BIT_LEFT) | (1 << BIT_RIGHT))
    mask |= left << BIT_RIGHT
    mask |= right << BIT_LEFT
    return mask


class FacingLatch:
    """Derived facing (sign(opp.x - me.x)) with the pin-and-debounce policy of
    MACRO_ACTIONS.md §10.2. `update(me_x, opp_x)` returns the current latched
    sign (+1 / -1), or None until the first frame with BOTH x present.

    - Pins on the first frame both x are present.
    - HOLDS the last-known sign on any absent frame (either x None) — absence
      is no information, never a synthesized 0.
    - Re-latches only after `swap_debounce` consecutive PRESENT frames of the
      opposite sign (a single-frame crossover, or the undefined sign exactly
      at opp.x == me.x, never flips it).
    """

    def __init__(self, swap_debounce: int):
        if swap_debounce < 1:
            raise ValueError("swap_debounce must be >= 1")
        self.swap_debounce = swap_debounce
        self._sign: int | None = None
        self._opp_run = 0

    def update(self, me_x, opp_x) -> int | None:
        if me_x is None or opp_x is None:
            return self._sign  # absent -> hold last known
        delta = opp_x - me_x
        # opp.x == me.x is an undefined/oscillating facing exactly at a
        # cross-up: treat it as "no confirmation", hold last known.
        if delta == 0:
            self._opp_run = 0
            if self._sign is None:
                return None
            return self._sign
        cur = 1 if delta > 0 else -1
        if self._sign is None:
            self._sign = cur
            self._opp_run = 0
        elif cur != self._sign:
            self._opp_run += 1
            if self._opp_run >= self.swap_debounce:
                self._sign = cur
                self._opp_run = 0
        else:
            self._opp_run = 0
        return self._sign

    @property
    def sign(self) -> int | None:
        return self._sign


def start_sign(sign_trace) -> int | None:
    """The recorded segment's START facing = its first known sign (holding
    through any leading absent frames — never a synthesized 0)."""
    for s in sign_trace:
        if s is not None:
            return s
    return None


def mirror_stream(masks, recorded_sign_trace, live_sign):
    """Return ONE concrete mask stream to inject on the shadow's port. If the
    deploy side (`live_sign`) faces opposite to the segment's recorded start
    facing, every frame's LEFT/RIGHT is swapped; otherwise the masks pass
    through unchanged. Because playback is literal, a within-segment crossup
    is preserved by this constant swap (see the module docstring).

    `live_sign` None (deploy facing not yet known) or a recorded start facing
    of None (the whole segment had absent x) means we cannot orient the
    mirror — pass through unchanged rather than guess (never synthesize a
    facing). Returns a tuple so the SAME object can be written to the slot
    file and wrapped as the expected stream."""
    rec_start = start_sign(recorded_sign_trace)
    swap = (live_sign is not None and rec_start is not None and live_sign != rec_start)
    if not swap:
        return tuple(masks)
    return tuple(mirror_mask(m) for m in masks)
