from __future__ import annotations

import contextlib
import io
import json
import tempfile
import unittest
from argparse import Namespace
from pathlib import Path

from shadow_train.__main__ import cmd_fit, cmd_report
from shadow_train.dataset import P, K, SCALAR_FEATURES
from shadow_train.knn import KnnPolicy

from .helpers import make_round_rows, write_jsonl


class FitReportRoundtripTest(unittest.TestCase):
    """Task 3 CLI: `fit` persists a case store + meta.json; `report` reads it
    back without needing the original recordings."""

    def test_fit_writes_case_store_and_meta(self):
        rows = make_round_rows(P * (K + 1 + 60), round_id=1)
        with tempfile.TemporaryDirectory() as d:
            rec_path = Path(d) / "rec.jsonl"
            write_jsonl(rec_path, rows)
            model_dir = Path(d) / "model"

            fit_args = Namespace(recordings=[rec_path], out=model_dir, char=None, k=9)
            with contextlib.redirect_stdout(io.StringIO()):
                cmd_fit(fit_args)

            self.assertTrue((model_dir / "cases.npz").exists())
            meta = json.loads((model_dir / "meta.json").read_text())

            self.assertEqual(meta["k"], 9)
            self.assertEqual(meta["feature_names"], SCALAR_FEATURES)
            self.assertGreater(meta["n_decisions"], 0)
            self.assertEqual(meta["n_rounds"], 1)
            self.assertIn("bucket_counts", meta)
            self.assertIn("calibration", meta)
            self.assertEqual(meta["source_files"], [str(rec_path)])

            # the persisted policy must be loadable and usable
            policy = KnnPolicy.load(model_dir)
            import numpy as np

            q = np.zeros(len(SCALAR_FEATURES) * K, dtype=np.float32)
            move, attack = policy.predict(q)
            self.assertIsInstance(move, int)
            self.assertIsInstance(attack, int)

            report_args = Namespace(model_dir=model_dir)
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                cmd_report(report_args)
            out = buf.getvalue()
            self.assertIn("coverage", out)
            self.assertIn(str(rec_path), out)


if __name__ == "__main__":
    unittest.main()
