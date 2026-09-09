import unittest

import numpy as np

from shadow_train import dataset as ds
from shadow_train.segments import features as F
from .helpers import make_round_rows


def default_v2_view():
    """The exact view _decisions_for_round builds when view is omitted."""
    return ds._RecordingView(
        version="v2", field_names=ds._V2_FIGHTER_FIELDS,
        calibration=ds._PROF.calibration, hitstun_map=ds._LEGACY_HITSTUN_SOURCES,
        port=ds._PROF.port,
    )


class FeatureParityTest(unittest.TestCase):
    """start_features_for must produce the SAME numbers dataset computes on the
    same frame — one feature implementation, proven, not two (the discipline
    that a byte-identical refit and the matcher unification both enforce)."""

    def test_start_features_match_dataset_decisions_frame_for_frame(self):
        # A round with position/health/hitstun variation so the parity check
        # exercises every scalar block, not just the constant ones.
        rows = make_round_rows(
            120, round_id=1, p1_block=1,
            block1_kw={"x": 150, "health": 180, "anim": 3, "timer": 5},
            block2_kw={"x": 205, "health": 160, "anim": 1, "timer": 2},
            combo_on_b1=lambda i: (i // 10) % 3,   # changes -> me_hitstun active
            combo_on_b2=lambda i: (i // 7) % 2,
        )
        view = default_v2_view()
        feats = ds.scalar_features_for(view)

        decisions = ds._decisions_for_round(("rec.jsonl", 1), rows)
        self.assertTrue(decisions, "expected some decisions")

        # decisions are emitted at grid indices P, 2P, ... — reconstruct them.
        P = ds.P
        grid = list(range(P, len(rows), P))
        self.assertEqual(len(grid), len(decisions))

        for i, d in zip(grid, decisions):
            scal = F.start_features_for(rows, view, side=1, row_idx=i)
            self.assertIsNotNone(scal)
            got = np.array([scal[k] for k in feats], dtype=np.float32)
            np.testing.assert_array_equal(
                got, d.scalars,
                err_msg=f"feature mismatch at grid row {i}")

    def test_absent_pointer_field_returns_none_not_zero(self):
        rows = make_round_rows(40, block1_kw={"x": 150}, block2_kw={"x": 205})
        # Declare x pointer-resolved and delete it from one row's block1.
        view = ds._RecordingView(
            version="v3", field_names=ds._V2_FIGHTER_FIELDS,
            calibration=ds._PROF.calibration, hitstun_map=ds._LEGACY_HITSTUN_SOURCES,
            port=ds._PROF.port, pointer_resolved_fields=frozenset({"x"}),
        )
        del rows[16]["block1"]["x"]
        self.assertIsNone(F.start_features_for(rows, view, side=1, row_idx=16))
        # a row WITH x still resolves
        self.assertIsNotNone(F.start_features_for(rows, view, side=1, row_idx=8))


if __name__ == "__main__":
    unittest.main()
