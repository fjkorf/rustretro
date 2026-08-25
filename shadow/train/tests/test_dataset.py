from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from shadow_train.dataset import (
    HITSTUN_RECENT_FRAMES,
    K,
    P,
    SEGMENT_DECISIONS,
    _recent_change_mask,
    build,
)

from .helpers import make_round_rows, write_jsonl


class RecentChangeMaskTest(unittest.TestCase):
    """Task 2: the combo counter is a lingering last-combo-count display, not
    a live hitstun flag (see the evidence comment above _recent_change_mask
    in dataset.py, and the byte dump against the real recordings in the PR
    report). These pin down the exact behavior the fix depends on."""

    def test_lingering_value_stops_counting_as_active_past_the_window(self):
        # Jumps to 2 once at frame 100, then holds at 2 for 300 frames -- the
        # exact shape observed live (combo_on_b1 held a constant nonzero
        # value for a run of 7582 frames in the real recording).
        vals = [0] * 100 + [2] * 300
        rows = [{"frame": i, "gate": {"combo_on_x": v}} for i, v in enumerate(vals)]
        mask = _recent_change_mask(rows, "combo_on_x", HITSTUN_RECENT_FRAMES)

        self.assertFalse(mask[99])  # still zero
        self.assertTrue(mask[100])  # just changed
        self.assertTrue(mask[100 + HITSTUN_RECENT_FRAMES])  # boundary: still active
        self.assertFalse(mask[100 + HITSTUN_RECENT_FRAMES + 1])  # one past: stale
        self.assertFalse(mask[200])  # deep into the lingering run: NOT hitstun

    def test_zero_is_never_active(self):
        rows = [{"frame": i, "gate": {"combo_on_x": 0}} for i in range(50)]
        mask = _recent_change_mask(rows, "combo_on_x", HITSTUN_RECENT_FRAMES)
        self.assertFalse(mask.any())

    def test_successive_increments_within_a_real_combo_stay_active(self):
        # gaps of 1-5 frames between increments, as seen live during actual
        # combos (7-25 frame gaps in the real recordings).
        vals = [0, 0, 2, 2, 2, 3, 3, 3, 3, 3, 4]
        rows = [{"frame": i, "gate": {"combo_on_x": v}} for i, v in enumerate(vals)]
        mask = _recent_change_mask(rows, "combo_on_x", window=5)
        self.assertTrue(mask[-1])
        self.assertTrue(all(mask[2:]))  # active from the first hit onward


class SegmentationTest(unittest.TestCase):
    """Task 1: long (training-mode, frozen-timer) rounds must be chopped into
    multiple pseudo-round split units; short rounds stay single units."""

    def test_long_round_splits_into_multiple_pseudo_rounds(self):
        n_decisions = SEGMENT_DECISIONS * 2 + 20  # -> 3 segments: 150,150,20
        n_frames = P * (n_decisions + 1)
        long_rows = make_round_rows(n_frames, round_id=1, start_frame=0)

        short_frames = P * (K + 1 + 10)
        short_rows = make_round_rows(short_frames, round_id=2, start_frame=n_frames)

        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "session.jsonl"
            write_jsonl(path, long_rows + short_rows)
            data = build([path])

        rounds = data["rounds"]
        long_keys = {k for k in rounds if k[1] == 1}
        short_keys = {k for k in rounds if k[1] == 2}

        self.assertEqual(len(long_keys), 3)
        self.assertEqual(len(short_keys), 1)
        for k in long_keys | short_keys:
            self.assertEqual(len(k), 3)  # (file, round_id, seg_k) uniformly

    def test_split_by_round_is_no_longer_degenerate(self):
        # Regression guard for the exact bug this task fixes: 2 real rounds
        # (one of them a frozen-timer training round) must not collapse the
        # split to "almost everything in one bucket".
        from shadow_train.evaluate import split_by_round

        n_decisions = SEGMENT_DECISIONS * 6
        n_frames = P * (n_decisions + 1)
        rows = make_round_rows(n_frames, round_id=1)
        short_rows = make_round_rows(
            P * (K + 1 + 30), round_id=2, start_frame=n_frames
        )
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "session.jsonl"
            write_jsonl(path, rows + short_rows)
            data = build([path])

        train_idx, test_idx = split_by_round(data, holdout_frac=0.2)
        self.assertGreater(len(train_idx), 0)
        self.assertGreater(len(test_idx), 0)
        # neither side should be a vanishing sliver of the total
        total = len(train_idx) + len(test_idx)
        self.assertGreater(len(test_idx) / total, 0.05)
        self.assertGreater(len(train_idx) / total, 0.5)


class BucketIntegrationTest(unittest.TestCase):
    """End-to-end: the me_hitstun (current-index, self) / opp_hitstun (STALE-
    index, opponent) wiring in _decisions_for_round, through _bucket, through
    build()'s K-stacking -- using recent-change combo bursts, not bare
    nonzero, exactly like the real fix."""

    def test_hitstun_bursts_produce_correct_buckets(self):
        def combo_on_b1(i):  # "me" (block1) takes a hit around frame 85-95
            return 2 if 85 <= i <= 95 else 0

        def combo_on_b2(i):  # opponent (block2) takes a hit around frame 122-132
            return 3 if 122 <= i <= 132 else 0

        rows = make_round_rows(
            P * 50, round_id=1, combo_on_b1=combo_on_b1, combo_on_b2=combo_on_b2
        )
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "session.jsonl"
            write_jsonl(path, rows)
            data = build([path])

        buckets = data["buckets"]
        # decision m -> frame P*(m+1); global bucket index j = m - (K-1)
        def bucket_at(m):
            return buckets[m - (K - 1)]

        self.assertEqual(bucket_at(10), "defense")  # frame 88: me_hitstun active
        self.assertEqual(bucket_at(15), "offense")  # frame 128: opp_hitstun active (stale-read at 125)
        self.assertEqual(bucket_at(30), "neutral")  # frame 248: neither active

    def test_opp_hitstun_uses_stale_read(self):
        # opp_hitstun (opponent-sourced, §4) must be read STALE frames behind
        # "now", like the rest of the opp_* block -- not off the current row.
        # Burst active for frames [121, 141]; decision frame 144 is just past
        # the end of the burst (so the CURRENT frame already reverted to 0),
        # but frame 144 - STALE(3) = 141 is still inside the burst.
        def combo_on_b2(i):
            return 9 if 121 <= i <= 141 else 0

        rows = make_round_rows(P * 50, round_id=1, combo_on_b2=combo_on_b2)
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "session.jsonl"
            write_jsonl(path, rows)
            data = build([path])

        m = 144 // P - 1  # decision whose frame is P*(m+1) == 144
        self.assertEqual(144, P * (m + 1))
        self.assertEqual(data["buckets"][m - (K - 1)], "offense")


if __name__ == "__main__":
    unittest.main()


class TestSubsampleNeutral(unittest.TestCase):
    def test_caps_idle_and_keeps_all_active(self):
        import numpy as np
        from shadow_train.dataset import subsample_neutral
        n = 1000
        y_move = np.zeros(n, dtype=int)
        y_attack = np.zeros(n, dtype=int)
        y_move[:50] = 1          # 50 active-by-move
        y_attack[50:80] = 2      # 30 active-by-attack
        data = {"X": np.arange(n * 2, dtype=np.float32).reshape(n, 2),
                "y_move": y_move, "y_attack": y_attack,
                "buckets": np.array(["neutral"] * n),
                "rounds": [("f", 1, 0)] * n}
        out = subsample_neutral(data, cap_ratio=2.0)
        idle = (out["y_move"] == 0) & (out["y_attack"] == 0)
        self.assertEqual(int((~idle).sum()), 80)          # every active kept
        self.assertEqual(int(idle.sum()), 160)            # 2.0 x 80
        self.assertEqual(len(out["rounds"]), len(out["X"]))
        # disabled path returns data unchanged
        self.assertIs(subsample_neutral(data, cap_ratio=0), data)
