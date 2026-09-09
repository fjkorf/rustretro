import json
import tempfile
import unittest
from pathlib import Path

from shadow_train.segments import Segment, SegmentIndex, SegmentsConfig
from shadow_train.segments.config import SegmentsConfigError


def _seg(**kw):
    base = dict(
        file="rec.jsonl", round_id=1, start_row=0, end_row=10, start_frame=100,
        side=1, boundary_kind="round_start", char_id=7, opp_char_id=3,
        matchup_key="7v3", seq=0, recency_rank=0, n_frames=10, contact_count=1,
        absent_x_frames=0, start_features={"dist_x": 0.4},
    )
    base.update(kw)
    return Segment(**base)


class SegmentModelTest(unittest.TestCase):
    def test_index_round_trips_byte_stable(self):
        idx = SegmentIndex(
            segments=[_seg(seq=0), _seg(seq=1, boundary_kind="contact")],
            meta={"format": "segments-v1", "family": "mk2"},
        )
        with tempfile.TemporaryDirectory() as d:
            idx.write(d)
            again = SegmentIndex.read(d)
            self.assertEqual([s.to_json() for s in idx.segments],
                             [s.to_json() for s in again.segments])
            self.assertEqual(idx.meta, again.meta)
            # Rewriting the same index produces byte-identical jsonl.
            first = (Path(d) / "segments.jsonl").read_bytes()
            idx.write(d)
            self.assertEqual(first, (Path(d) / "segments.jsonl").read_bytes())

    def test_start_features_omits_absent_never_zero_fills(self):
        # A feature the builder could not compute is simply absent from the
        # dict; round-tripping must not invent a 0 for it.
        s = _seg(start_features={"dist_x": 0.4})  # no me_airborne key
        with tempfile.TemporaryDirectory() as d:
            SegmentIndex(segments=[s], meta={}).write(d)
            back = SegmentIndex.read(d).segments[0]
        self.assertNotIn("me_airborne", back.start_features)


class SegmentsConfigTest(unittest.TestCase):
    def test_loads_shipped_mk2_and_asurabld_seeds(self):
        for fam in ("mk2", "asurabld"):
            cfg = SegmentsConfig.load(Path("library") / fam)
            self.assertEqual(cfg.family, fam)
            self.assertIn("seg_max", cfg.boundaries)
            self.assertTrue(cfg.weights)  # non-empty

    def test_missing_file_is_an_error_not_a_default(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(SegmentsConfigError):
                SegmentsConfig.load(d)

    def test_missing_key_names_what_is_missing(self):
        with tempfile.TemporaryDirectory() as d:
            fam = Path(d) / "fam"
            fam.mkdir()
            (fam / "segments.json").write_text(json.dumps({
                "boundaries": {"seg_max": 180},  # missing the rest
                "similarity": {"select_band": 0.1, "match_floor": 3.0,
                               "stickiness_bias": 1.0, "swap_debounce": 3,
                               "recency_half_life": 10,
                               "boundary_kind_mismatch_penalty": 1.0,
                               "weights": {"dist_x": 1.0}},
            }))
            with self.assertRaises(SegmentsConfigError) as cm:
                SegmentsConfig.load(fam)
            self.assertIn("y_eps", str(cm.exception))


if __name__ == "__main__":
    unittest.main()
