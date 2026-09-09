import random
import unittest

from shadow_train.segments import Segment
from shadow_train.segments import similarity as S
from shadow_train.segments import stickiness as ST

W = {"dist_x": 1.0}


class DeviationTest(unittest.TestCase):
    def test_zero_on_identical(self):
        self.assertEqual(ST.deviation({"dist_x": 0.4}, {"dist_x": 0.4}, W), 0.0)

    def test_none_when_no_shared_feature_never_zero(self):
        # a None (not 0) means "no information" — 0 would read as a perfect
        # match and pin the segment forever (the absorbing-state trap).
        self.assertIsNone(ST.deviation({"dist_x": 0.4}, {"me_airborne": 1.0}, W))

    def test_monotone_in_perturbation(self):
        d1 = ST.deviation({"dist_x": 0.0}, {"dist_x": 0.1}, W)
        d2 = ST.deviation({"dist_x": 0.0}, {"dist_x": 0.5}, W)
        self.assertLess(d1, d2)


class ShouldContinueTest(unittest.TestCase):
    def test_break_is_strict_greater_than(self):
        self.assertTrue(ST.should_continue(1.0, bias=1.0))   # equality continues
        self.assertFalse(ST.should_continue(1.0001, bias=1.0))

    def test_none_deviation_continues_latch_holds(self):
        self.assertTrue(ST.should_continue(None, bias=0.0))


class WalkSegmentTest(unittest.TestCase):
    def test_breaks_at_first_offset_exceeding_bias(self):
        seg = [{"dist_x": 0.0}] * 5
        live = [{"dist_x": 0.0}, {"dist_x": 0.0}, {"dist_x": 0.5},
                {"dist_x": 0.0}, {"dist_x": 0.0}]
        # deviation 0,0,0.5,... bias 0.2 -> break at offset 2 (not earlier, not later)
        self.assertEqual(ST.walk_segment(seg, live, W, bias=0.2), ("break", 2))

    def test_exhausts_when_never_exceeds(self):
        seg = [{"dist_x": 0.0}] * 4
        live = [{"dist_x": 0.1}] * 4
        self.assertEqual(ST.walk_segment(seg, live, W, bias=0.2), ("exhausted", 4))

    def test_absent_feature_frame_does_not_break(self):
        seg = [{"dist_x": 0.0}, {"me_airborne": 1.0}, {"dist_x": 0.0}]
        live = [{"dist_x": 0.0}, {"dist_x": 5.0}, {"dist_x": 0.0}]
        # offset 1 shares no feature -> None -> hold, never a break
        self.assertEqual(ST.walk_segment(seg, live, W, bias=0.2), ("exhausted", 3))


class AbsorbingStateControlTest(unittest.TestCase):
    """§8.4 — the control that bit the kNN shadow (dataset.py's neutral cap):
    a segment engine must not AMPLIFY idleness beyond its own corpus. Named
    control: run a closed retrieve->walk->break simulation over a corpus with
    a measured idle fraction and assert the simulated zero-input fraction does
    not exceed the corpus's own idle fraction (+ margin). A segment engine is
    structurally more resistant than the kNN (it replays whole movement
    segments, so 'stand still forever' is not reachable) — but VERIFY it."""

    def _corpus(self):
        # 10 segments spread evenly across the feature space; 2 are IDLE
        # (all-zero masks). Idle segments sit in the MIDDLE of the space, not
        # at an edge, so band effects are symmetric. Corpus idle frame
        # fraction = 2/10 = 0.2.
        specs = []
        idle_seqs = {4, 5}
        for i in range(10):
            ms = [0] * 10 if i in idle_seqs else [1 << 7] * 10  # idle vs hold-Right
            specs.append((i, {"dist_x": i / 10.0}, ms))
        segs, masks = [], {}
        for seq, feats, ms in specs:
            segs.append(Segment(
                file="r.jsonl", round_id=1, start_row=0, end_row=10, start_frame=0,
                side=1, boundary_kind="neutral", char_id=7, opp_char_id=3,
                matchup_key="7v3", seq=seq, recency_rank=0, n_frames=10,
                contact_count=0, absent_x_frames=0, start_features=feats))
            masks[seq] = ms
        return segs, masks, idle_seqs

    def test_engine_does_not_amplify_idleness(self):
        segs, masks, idle_seqs = self._corpus()
        corpus_idle_frac = sum(len(masks[s]) for s in idle_seqs) / \
            sum(len(ms) for ms in masks.values())   # 0.2

        # A TIGHT band so retrieval returns the genuine nearest — a wide band
        # would let any query pull in idle neighbors and measure band width,
        # not idleness. Queries are drawn from the corpus's OWN situation
        # distribution (a random segment's feature + small noise), which is
        # what "the engine faces the situations the player faced" means; the
        # emitted idle fraction must then track the corpus's, not exceed it.
        cfg = {"weights": W, "select_band": 0.01, "match_floor": 100.0,
               "stickiness_bias": 1000.0, "recency_half_life": 10,
               "boundary_kind_mismatch_penalty": 0.0}
        rng = random.Random(0)
        emitted = []
        for _ in range(2000):
            base = segs[rng.randrange(len(segs))].start_features["dist_x"]
            q = {"dist_x": base + rng.uniform(-0.02, 0.02)}
            r = S.retrieve(q, "neutral", 7, 3, segs, cfg, rng)
            emitted.extend(masks[r.segment.seq])
        sim_idle_frac = sum(1 for m in emitted if m == 0) / len(emitted)
        # The engine replays whole recorded segments, so it emits at most the
        # idleness its corpus contains — it never amplifies it (the kNN's
        # absorbing-state failure the neutral cap fixed).
        self.assertLessEqual(sim_idle_frac, corpus_idle_frac + 0.05,
                             f"sim idle {sim_idle_frac:.3f} > corpus {corpus_idle_frac:.3f}")


if __name__ == "__main__":
    unittest.main()
