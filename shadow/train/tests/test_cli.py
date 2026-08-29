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


class ReportStringStatsTest(unittest.TestCase):
    """MACRO_ACTIONS.md §8 item 5: `report` prints a compact string/juggle
    line read from the source recordings' `.rounds.jsonl` sidecars -- never
    from the fit's own meta.json (these are per-round facts the Rust
    recorder wrote, not features) -- and omits it entirely when no round
    anywhere carries a `strings` object."""

    def _fit(self, d, rec_path):
        rows = make_round_rows(P * (K + 1 + 60), round_id=1)
        write_jsonl(rec_path, rows)
        model_dir = Path(d) / "model"
        fit_args = Namespace(recordings=[rec_path], out=model_dir, char=None, k=9)
        with contextlib.redirect_stdout(io.StringIO()):
            cmd_fit(fit_args)
        return model_dir

    def test_report_prints_string_and_juggle_summary_when_sidecar_has_one(self):
        with tempfile.TemporaryDirectory() as d:
            rec_path = Path(d) / "rec.jsonl"
            model_dir = self._fit(d, rec_path)
            sidecar = Path(d) / "rec.rounds.jsonl"
            sidecar.write_text(json.dumps({
                "round_id": 1, "v": 3,
                "strings": {"count": 12, "longest_hits": 4, "longest_damage": 68,
                            "block_strings": 3, "juggle_hits": 5},
            }) + "\n")

            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                cmd_report(Namespace(model_dir=model_dir))
            out = buf.getvalue()
            self.assertIn("strings: 12 (longest 4 hits / 68 dmg)", out)
            self.assertIn("block strings: 3", out)
            self.assertIn("juggle hits: 5", out)

    def test_report_omits_juggle_when_no_round_has_the_key(self):
        # y unmapped (mk2 arcade-shaped) -- juggle_hits absent from every
        # round's strings object -- the line must not claim a juggle count.
        with tempfile.TemporaryDirectory() as d:
            rec_path = Path(d) / "rec.jsonl"
            model_dir = self._fit(d, rec_path)
            sidecar = Path(d) / "rec.rounds.jsonl"
            sidecar.write_text(json.dumps({
                "round_id": 1, "v": 3,
                "strings": {"count": 2, "longest_hits": 0, "longest_damage": 0,
                            "block_strings": 2},
            }) + "\n")

            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                cmd_report(Namespace(model_dir=model_dir))
            out = buf.getvalue()
            self.assertIn("strings: 2 (longest 0 hits / 0 dmg)", out)
            self.assertNotIn("juggle", out)

    def test_report_omits_strings_line_entirely_when_no_sidecar_has_one(self):
        # No `.rounds.jsonl` sidecar at all (or one with no `strings` key,
        # e.g. sf2ce -- no contact source mapped): never a zeroed line.
        with tempfile.TemporaryDirectory() as d:
            rec_path = Path(d) / "rec.jsonl"
            model_dir = self._fit(d, rec_path)

            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                cmd_report(Namespace(model_dir=model_dir))
            out = buf.getvalue()
            self.assertNotIn("strings:", out)


if __name__ == "__main__":
    unittest.main()
