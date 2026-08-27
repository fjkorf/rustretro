"""RECORDER_V3.md §3 gates (Python side, A2): G2's v2==v3 feature-matrix
equality on asurabld, plus a v3-native sparse-family fixture proving §4.2's
honest-degradation contract (a family missing fields gets a smaller feature
vector -- never NaN, never zero-fill)."""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

import numpy as np

from shadow_train import dataset

from .helpers import transcode_v2_to_v3

FIXTURES = Path(__file__).parent / "fixtures"
V2_FIXTURE = FIXTURES / "v2-asurabld-sample.jsonl"


class TranscodedV3MatchesV2Test(unittest.TestCase):
    """G2: build([v2 fixture]) vs build([mechanically-transcoded v3 fixture])
    must produce identical y_move/y_attack/buckets/rounds and X within
    max|ΔX| <= 1e-12 -- proof the v3 row reorganization changes nothing for
    a family (asurabld) whose fighter_fields/gate already match what v2
    hardcoded (RECORDER_V3.md §3 G2)."""

    def _transcoded_copy(self, tmp_path: Path) -> Path:
        v3_path = tmp_path / "v3-transcoded.jsonl"
        with open(V2_FIXTURE) as src, open(v3_path, "w") as dst:
            for line in src:
                row = json.loads(line)
                dst.write(json.dumps(transcode_v2_to_v3(row)) + "\n")
        return v3_path

    def test_v2_and_transcoded_v3_build_identically(self):
        with tempfile.TemporaryDirectory() as d:
            v3_path = self._transcoded_copy(Path(d))
            # No .meta.json sidecar next to v3_path -- exercises the
            # documented fallback (§4.3 / item 2): trust the loaded profile
            # fully, exactly like v2, which is what makes this equality hold
            # even though the profile's own hitstun_sources/record_globals
            # keys describe MORE than this hand-cut fixture's raw content.
            data_v2 = dataset.build([V2_FIXTURE])
            data_v3 = dataset.build([v3_path])

        self.assertEqual(data_v2["feature_names"], data_v3["feature_names"])
        self.assertEqual(data_v2["feature_names"], dataset.SCALAR_FEATURES)

        np.testing.assert_array_equal(data_v2["y_move"], data_v3["y_move"])
        np.testing.assert_array_equal(data_v2["y_attack"], data_v3["y_attack"])
        np.testing.assert_array_equal(data_v2["buckets"], data_v3["buckets"])
        # round keys carry the filename as their first element -- compare
        # everything BUT that (v2-asurabld-sample.jsonl vs v3-transcoded.jsonl).
        self.assertEqual(
            [k[1:] for k in data_v2["rounds"]], [k[1:] for k in data_v3["rounds"]]
        )
        max_abs_diff = float(np.max(np.abs(data_v2["X"] - data_v3["X"])))
        self.assertLessEqual(max_abs_diff, 1e-12, f"max|ΔX| = {max_abs_diff}")
        self.max_abs_diff_seen = max_abs_diff


def _sparse_row(frame: int, round_id: int, p1_input: int,
                 me_x: int, opp_x: int, me_health: int, opp_health: int) -> dict:
    """A v3 row shaped like RECORDER_V3.md §1.2's illustrative sparse-port
    example (MK2-Genesis-ish): only char_id/health/x are mapped, `globals`
    carries no hitstun evidence -- everything else must degrade, not fake."""
    return {
        "v": 3, "frame": frame, "round_id": round_id, "controllable": True,
        "p1_block": 1,
        "block1": {"char_id": 1, "health": me_health, "x": me_x},
        "block2": {"char_id": 9, "health": opp_health, "x": opp_x},
        "globals": {"screen_state": 0, "round_over": 0},
        "p1_input": p1_input, "p2_input": 0,
    }


class SparseFamilyHonestDegradationTest(unittest.TestCase):
    """§4.2: a port whose profile maps only x/char_id/health (no y, anim,
    timer, meter, facing; no hitstun_sources) gets an honestly SMALLER
    feature vector -- not NaN, not zero-filled placeholders for the missing
    fields. Uses a real `.meta.json` sidecar (§1.3) so this exercises the
    self-describing v3 path, not the no-sidecar profile-fallback branch the
    G2 test above uses."""

    def _write_recording(self, d: Path) -> Path:
        rows = []
        n_frames = dataset.P * (dataset.K + 1 + 20)  # comfortably >= the stack minimum
        for i in range(n_frames):
            p1_input = 0x80 if i % 16 == 0 else 0  # nonzero mass -> not a demo round
            rows.append(_sparse_row(
                frame=i, round_id=1, p1_input=p1_input,
                me_x=100 + i, opp_x=300 - i,
                me_health=161 - (i % 20), opp_health=140 - (i % 15),
            ))
        path = d / "sparse-session.jsonl"
        with open(path, "w") as f:
            for r in rows:
                f.write(json.dumps(r) + "\n")

        meta = {
            "format": "jsonl-v3", "family": "mk2", "port": "genesis",
            "profile_file": "genesis.profile.json", "profile_sha256": "deadbeef",
            "game": "mk2", "core": "genesis_plus_gx", "style": None, "fps": 60,
            "anchor": "smaller_x",
            "blocks": {"block1": "0xFF8000", "block2": "0xFF8200", "stride": "0x200"},
            "fighter_fields": [
                {"name": "char_id", "off": "0x0", "size": 1},
                {"name": "health", "off": "0xE", "size": 1},
                {"name": "x", "off": "0x10", "size": 2},
            ],
            # No combo/hitstun globals recorded -- hitstun must drop.
            "globals_recorded": [
                {"name": "screen_state", "size": 2}, {"name": "round_over", "size": 1},
            ],
            "gate": [{"kind": "word_zero", "global": "screen_state"}],
            # X_SCALE + HEALTH_MAX only -- no GROUND_Y/Y_SCALE/ANIM_SCALE/
            # TIMER_SCALE/CORNER_PX/SCREEN_W, so y/anim/timer/corner all drop
            # while health survives (proving PARTIAL, field-by-field
            # degradation, not an all-or-nothing cliff).
            "calibration": {"X_SCALE": 128.0, "HEALTH_MAX": 161},
            "created": "2026-08-27T00:00:00Z",
        }
        meta_path = d / "sparse-session.meta.json"
        meta_path.write_text(json.dumps(meta))
        return path

    def test_feature_vector_shrinks_honestly(self):
        with tempfile.TemporaryDirectory() as d:
            path = self._write_recording(Path(d))
            data = dataset.build([path])

        expected = ["dist_x", "me_fwd_hold", "me_back_hold", "facing_sign",
                    "me_health", "opp_health", "health_lead"]
        self.assertEqual(data["feature_names"], expected)
        self.assertLess(len(data["feature_names"]), len(dataset.SCALAR_FEATURES))
        self.assertEqual(data["ports"], ["genesis"])

        # X has exactly K * len(expected) columns -- no padding to the full
        # canonical width, and no NaN/inf anywhere (the "never zero-fill/
        # fabricate a missing signal" rule manifests as ABSENT columns, not
        # as present-but-fake ones).
        self.assertEqual(data["X"].shape[1], dataset.K * len(expected))
        self.assertTrue(np.isfinite(data["X"]).all())

        # buckets degrade too: no hitstun evidence recorded -> never
        # offense/defense; no y -> never air (me_airborne unavailable); no
        # corner calibration -> never corner. Only "neutral" can appear.
        self.assertEqual(set(data["buckets"].tolist()), {"neutral"})

    def test_missing_required_field_aborts_loudly(self):
        """The three §4.2 required fields (x, char_id, health) abort the fit
        by name rather than silently degrading -- x is missing here."""
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            rows = [{
                "v": 3, "frame": i, "round_id": 1, "controllable": True,
                "p1_block": 1,
                "block1": {"char_id": 1, "health": 100},
                "block2": {"char_id": 9, "health": 90},
                "globals": {}, "p1_input": 0x80 if i % 16 == 0 else 0, "p2_input": 0,
            } for i in range(dataset.P * (dataset.K + 1 + 5))]
            path = d / "no-x.jsonl"
            with open(path, "w") as f:
                for r in rows:
                    f.write(json.dumps(r) + "\n")
            meta = {
                "fighter_fields": [{"name": "char_id", "off": "0x0", "size": 1},
                                    {"name": "health", "off": "0xE", "size": 1}],
                "globals_recorded": [], "gate": [], "calibration": {},
                "port": "genesis",
            }
            (d / "no-x.meta.json").write_text(json.dumps(meta))

            with self.assertRaises(SystemExit) as ctx:
                dataset.build([path])
            self.assertIn("'x'", str(ctx.exception))


class MixedPortFitRuleTest(unittest.TestCase):
    """§4.3: a fit spanning files whose resolved feature sets differ must
    abort naming the difference, never silently truncate/pad to match."""

    def test_differing_feature_sets_across_files_abort(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            full_rows = [{
                "v": 3, "frame": i, "round_id": 1, "controllable": True,
                "p1_block": 1,
                "block1": {"char_id": 1, "health": 200, "x": 100, "y": 216},
                "block2": {"char_id": 2, "health": 180, "x": 200, "y": 216},
                "globals": {}, "p1_input": 0x80 if i % 16 == 0 else 0, "p2_input": 0,
            } for i in range(dataset.P * (dataset.K + 1 + 5))]
            full_path = d / "full.jsonl"
            with open(full_path, "w") as f:
                for r in full_rows:
                    f.write(json.dumps(r) + "\n")
            (d / "full.meta.json").write_text(json.dumps({
                "fighter_fields": [{"name": "char_id", "off": "0x0", "size": 1},
                                    {"name": "health", "off": "0xE", "size": 1},
                                    {"name": "x", "off": "0x10", "size": 2},
                                    {"name": "y", "off": "0x12", "size": 2}],
                "globals_recorded": [], "gate": [],
                "calibration": {"X_SCALE": 128.0, "HEALTH_MAX": 239,
                                 "GROUND_Y": 216, "Y_SCALE": 128.0},
                "port": "arcade",
            }))

            sparse_path = self._sparse_copy(d)

            with self.assertRaises(SystemExit) as ctx:
                dataset.build([full_path, sparse_path])
            self.assertIn("feature sets differ", str(ctx.exception))

    @staticmethod
    def _sparse_copy(d: Path) -> Path:
        rows = [{
            "v": 3, "frame": i, "round_id": 1, "controllable": True,
            "p1_block": 1,
            "block1": {"char_id": 1, "health": 161, "x": 213},
            "block2": {"char_id": 9, "health": 140, "x": 301},
            "globals": {}, "p1_input": 0x80 if i % 16 == 0 else 0, "p2_input": 0,
        } for i in range(dataset.P * (dataset.K + 1 + 5))]
        path = d / "sparse.jsonl"
        with open(path, "w") as f:
            for r in rows:
                f.write(json.dumps(r) + "\n")
        (d / "sparse.meta.json").write_text(json.dumps({
            "fighter_fields": [{"name": "char_id", "off": "0x0", "size": 1},
                                {"name": "health", "off": "0xE", "size": 1},
                                {"name": "x", "off": "0x10", "size": 2}],
            "globals_recorded": [], "gate": [],
            "calibration": {"X_SCALE": 128.0, "HEALTH_MAX": 161},
            "port": "genesis",
        }))
        return path


if __name__ == "__main__":
    unittest.main()
