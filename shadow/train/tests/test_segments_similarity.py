import json
import random
import unittest
from pathlib import Path

from shadow_train.segments import Segment
from shadow_train.segments import similarity as S

GOLDEN = Path(__file__).parent / "fixtures" / "segment_similarity_golden.json"


def seg(seq, feats, kind="neutral", me=7, opp=3, recency=0):
    return Segment(
        file="r.jsonl", round_id=1, start_row=0, end_row=10, start_frame=0,
        side=1, boundary_kind=kind, char_id=me, opp_char_id=opp,
        matchup_key=f"{me}v{opp}", seq=seq, recency_rank=recency, n_frames=10,
        contact_count=0, absent_x_frames=0, start_features=feats,
    )


CFG = {
    "weights": {"dist_x": 1.0, "me_airborne": 3.0},
    "select_band": 0.10, "match_floor": 0.5, "stickiness_bias": 1.0,
    "recency_half_life": 10, "boundary_kind_mismatch_penalty": 1.0,
}


class ScoreTest(unittest.TestCase):
    def test_weighted_l1_over_shared_features(self):
        r = S.score({"dist_x": 0.0, "me_airborne": 0.0},
                    {"dist_x": 0.2, "me_airborne": 1.0}, CFG["weights"])
        self.assertAlmostEqual(r.score, 1.0 * 0.2 + 3.0 * 1.0)
        self.assertEqual(r.n_used, 2)
        self.assertEqual(r.n_skipped, 0)

    def test_absent_feature_is_skipped_and_counted_not_zeroed(self):
        r = S.score({"dist_x": 0.0}, {"dist_x": 0.3, "me_airborne": 1.0},
                    CFG["weights"])
        self.assertAlmostEqual(r.score, 0.3)   # me_airborne contributes nothing
        self.assertEqual(r.n_used, 1)
        self.assertEqual(r.n_skipped, 1)

    def test_boundary_penalty_added(self):
        r = S.score({"dist_x": 0.0}, {"dist_x": 0.0}, CFG["weights"],
                    boundary_penalty=1.0)
        self.assertAlmostEqual(r.score, 1.0)


class RetrieveTest(unittest.TestCase):
    def test_band_membership_is_exact(self):
        segs = [seg(0, {"dist_x": 1.0}), seg(1, {"dist_x": 1.05}),
                seg(2, {"dist_x": 1.5})]
        # query 0.0 -> scores 1.0/1.05/1.5, best 1.0, cutoff 1.1 -> band {0,1}
        picks = set()
        for s in range(200):
            r = S.retrieve({"dist_x": 0.0}, "neutral", 7, 3, segs, CFG, random.Random(s))
            self.assertEqual(r.band_size, 2)
            picks.add(r.segment.seq)
        self.assertEqual(picks, {0, 1})   # everything in band selectable, nothing else

    def test_off_distribution_flips_at_match_floor(self):
        near = [seg(0, {"dist_x": 0.4})]   # best 0.4 < 0.5
        far = [seg(0, {"dist_x": 0.6})]    # best 0.6 > 0.5
        self.assertFalse(S.retrieve({"dist_x": 0.0}, "neutral", 7, 3, near, CFG, random.Random(0)).off_distribution)
        self.assertTrue(S.retrieve({"dist_x": 0.0}, "neutral", 7, 3, far, CFG, random.Random(0)).off_distribution)

    def test_recency_is_preferred_among_equal_scores(self):
        segs = [seg(0, {"dist_x": 1.0}, recency=50), seg(1, {"dist_x": 1.0}, recency=0)]
        counts = {0: 0, 1: 0}
        for s in range(400):
            r = S.retrieve({"dist_x": 0.0}, "neutral", 7, 3, segs, CFG, random.Random(s))
            counts[r.segment.seq] += 1
        self.assertGreater(counts[1], counts[0])   # recency_rank 0 wins more often

    def test_fallback_tier_is_reported(self):
        # no exact (me=7,opp=3); a per-char (me=7,opp=9) exists
        segs = [seg(0, {"dist_x": 0.0}, me=7, opp=9)]
        r = S.retrieve({"dist_x": 0.0}, "neutral", 7, 3, segs, CFG, random.Random(0))
        self.assertEqual(r.tier, "per-char")

    def test_empty_pool_is_honest_not_a_fabrication(self):
        r = S.retrieve({"dist_x": 0.0}, "neutral", 7, 3, [], CFG, random.Random(0))
        self.assertIsNone(r.segment)
        self.assertEqual(r.tier, "none")
        self.assertTrue(r.off_distribution)


class GoldenTest(unittest.TestCase):
    def test_matches_committed_golden(self):
        data = json.loads(GOLDEN.read_text())
        segs = [seg(**s) for s in data["segments"]]
        for case in data["cases"]:
            r = S.retrieve(case["query"], case["query_kind"], case["me"],
                           case["opp"], segs, data["cfg"], random.Random(case["seed"]))
            self.assertEqual(r.tier, case["expect"]["tier"], case)
            self.assertAlmostEqual(r.score, case["expect"]["score"], places=6, msg=case)
            self.assertEqual(r.band_size, case["expect"]["band_size"], case)
            self.assertEqual(r.off_distribution, case["expect"]["off_distribution"], case)
            picked = None if r.segment is None else r.segment.seq
            self.assertEqual(picked, case["expect"]["picked_seq"], case)


if __name__ == "__main__":
    unittest.main()
