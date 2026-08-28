"""shadow/MACRO_ACTIONS.md §4 end-to-end: label space + decision labeling,
through the real `dataset.build()`/`cmd_fit` path against a hand-built
tempdir profile (contract-§2-shaped JSON, NOT library/mk2 -- that family's
files are being edited by a concurrent agent and this suite must not depend
on their timing).
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import tempfile
import unittest
from argparse import Namespace
from pathlib import Path

from shadow_train import __main__ as _main
from shadow_train import dataset, profile

LEFT, RIGHT = 0x40, 0x80
A, B = 0x100, 0x1  # LK, LP on the fixture's arcade-shaped attack_chords


def _family_json() -> str:
    return json.dumps({
        "family": "macrofam",
        "roster": [{"id": 0, "name": "reptile"}, {"id": 1, "name": "foe"}],
        "move_classes": ["Neutral", "Forward", "Back", "Up", "Down",
                          "UpForward", "UpBack", "DownForward", "DownBack"],
        "attack_classes": ["None", "LP", "LK", "HP", "HK"],
        "moves": {"reptile": [{"name": "slide", "tags": ["special", "low"]}]},
    })


def _port_json() -> str:
    return json.dumps({
        "family": "macrofam", "port": "test",
        "core": {"provenance_game": "macrofam", "provenance_core": "test"},
        "memory": {
            "endianness": "big",
            "blocks": {"block1": "0x0", "block2": "0x0", "stride": "0x0"},
            "fighter_fields": [
                {"name": "x", "off": "0x0", "size": 2},
                {"name": "char_id", "off": "0x2", "size": 1},
                {"name": "health", "off": "0x3", "size": 1},
            ],
            "globals": {},
        },
        "gate": [],
        "enforcement": {"health_max": 200, "refill_below": 1,
                         "timer_hold": [0, 0], "credits_target": 0, "credits_min": 0},
        "calibration": {"X_SCALE": 128.0, "P": 4, "K": 1, "STALE": 1,
                         "HITSTUN_RECENT_FRAMES": 20},
        "attack_chords": {"LP": ["b"], "LK": ["a"], "HP": ["y"], "HK": ["x"]},
        "special_inputs": {"reptile": {"slide": [
            {"dirs": ["back"], "press": ["LK", "LP"], "frames": 4},
        ]}},
    })


def _v3_row(frame: int, p1_input: int, me_x: int = 100, opp_x: int = 200,
            me_char: int = 0, opp_char: int = 1) -> dict:
    """v3-shaped row, no facing field mapped -- exercises the sign(opp.x -
    me.x) fallback for semantic dir resolution, the same code path both
    `_decisions_for_round`'s per-decision `s` and the macro matcher's
    per-frame `sides` array go through."""
    return {
        "v": 3, "frame": frame, "round_id": 1, "controllable": True,
        "p1_block": 1,
        "block1": {"x": me_x, "char_id": me_char, "health": 200},
        "block2": {"x": opp_x, "char_id": opp_char, "health": 200},
        "globals": {},
        "p1_input": p1_input, "p2_input": 0,
    }


class _MacroFamilyTestCase(unittest.TestCase):
    """Sets RUSTRETRO_GAME_DIR to a tempdir profile and refreshes dataset's
    profile-derived module state, mirroring test_profile_resolution.py's
    ProfileEnvTestCase -- restores both on tearDown so this doesn't leak
    MOVE_CLASSES/ATTACK_CLASSES into other test modules."""

    def setUp(self):
        self._orig_env = os.environ.get("RUSTRETRO_GAME_DIR")
        self._orig_calibration_keys = list(_main.CALIBRATION_KEYS)
        self._tmp = tempfile.TemporaryDirectory()
        d = Path(self._tmp.name) / "macrofam"
        d.mkdir()
        (d / "family.json").write_text(_family_json())
        (d / "macrofam.profile.json").write_text(_port_json())
        os.environ["RUSTRETRO_GAME_DIR"] = str(d)
        dataset.reload_profile()

    def tearDown(self):
        self._tmp.cleanup()
        if self._orig_env is None:
            os.environ.pop("RUSTRETRO_GAME_DIR", None)
        else:
            os.environ["RUSTRETRO_GAME_DIR"] = self._orig_env
        dataset.reload_profile()
        # cmd_fit (called by some tests below) also refreshes __main__'s own
        # CALIBRATION_KEYS module global -- restore it too, or a later test
        # file (alphabetically after this one) can see this fixture's keys
        # instead of asurabld's (see test_profile_resolution.py's identical
        # note on _resolve_profile_for's process-wide side effects).
        _main.CALIBRATION_KEYS = self._orig_calibration_keys


class LabelSpaceExtensionTest(_MacroFamilyTestCase):
    """§4: attack-head classes = family attack_classes + sorted "special"
    move names, family-wide."""

    def test_attack_classes_gains_the_sorted_special_names(self):
        self.assertEqual(
            dataset.ATTACK_CLASSES,
            ["None", "LP", "LK", "HP", "HK", "slide"],
        )

    def test_matches_profile_accessor(self):
        prof = profile.get()
        self.assertEqual(prof.all_special_names(), ["slide"])
        self.assertEqual(
            dataset.ATTACK_CLASSES,
            list(prof.attack_classes) + prof.all_special_names(),
        )


class SlideOverridesTheDecisionWindowTest(_MacroFamilyTestCase):
    """§4: "a special completing within a decision window overrides the
    base attack class for that decision" -- end to end through build()."""

    def _round_rows(self, n_frames: int, chord_at: tuple) -> list:
        lk_frames, lp_frames = chord_at
        rows = []
        for i in range(n_frames):
            mask = LEFT  # back held throughout -> side = +1 (opp.x > me.x)
            if i in lk_frames:
                mask |= A
            if i in lp_frames:
                mask |= B
            rows.append(_v3_row(i, mask))
        return rows

    def test_slide_chord_overrides_its_decision_attack_label(self):
        # LK held frames 10-13, LP held frames 12-13 -- a 2-frame stagger
        # between the two presses starting, but LK is still held when LP
        # arrives so they overlap and the chord completes at frame 12 (see
        # test_macros.py's identical fixture; §2: simultaneity, not a
        # trailing "recently pressed" window).
        # P=4: decisions at i=4,8,12,16,... -- frame 12 falls in the i=12
        # decision's window [12,16).
        rows = self._round_rows(40, (range(10, 14), range(12, 14)))
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "session.jsonl"
            with open(path, "w") as f:
                for r in rows:
                    f.write(json.dumps(r) + "\n")
            data = dataset.build([path])

        slide_idx = dataset.ATTACK_CLASSES.index("slide")
        # decision index m such that P*(m+1) == 12 -> m=2; K=1 so no offset.
        m = 12 // dataset.P - 1
        self.assertEqual(12, dataset.P * (m + 1))
        self.assertEqual(data["y_attack"][m], slide_idx)
        # every OTHER decision in this round must NOT be mislabeled slide.
        others = [v for i, v in enumerate(data["y_attack"]) if i != m]
        self.assertNotIn(slide_idx, others)

    def test_bare_lp_still_labels_lp_not_slide(self):
        # LP alone, no LK, no back -- must label as plain "LP", never "slide".
        rows = []
        for i in range(40):
            mask = B if i in (12, 13) else 0  # bare LP tap, no back hold
            rows.append(_v3_row(i, mask))
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "session.jsonl"
            with open(path, "w") as f:
                for r in rows:
                    f.write(json.dumps(r) + "\n")
            data = dataset.build([path])

        lp_idx = dataset.ATTACK_CLASSES.index("LP")
        slide_idx = dataset.ATTACK_CLASSES.index("slide")
        self.assertIn(lp_idx, data["y_attack"].tolist())
        self.assertNotIn(slide_idx, data["y_attack"].tolist())

    def test_fit_meta_reports_slide_in_attack_label_counts(self):
        # Task 4: attack_label_counts (already keyed by ATTACK_CLASSES names)
        # must surface "slide" once real specials exist in the label space,
        # AND meta gains a dedicated "specials" key `report` reads from.
        from shadow_train.__main__ import cmd_fit, cmd_report

        rows = self._round_rows(40, (range(10, 14), range(12, 14)))
        with tempfile.TemporaryDirectory() as d:
            rec_path = Path(d) / "session.jsonl"
            with open(rec_path, "w") as f:
                for r in rows:
                    f.write(json.dumps(r) + "\n")
            model_dir = Path(d) / "model"
            args = Namespace(recordings=[rec_path], out=model_dir, char=None, k=3)
            with contextlib.redirect_stdout(io.StringIO()):
                cmd_fit(args)
            meta = json.loads((model_dir / "meta.json").read_text())

            self.assertIn("slide", meta["attack_classes"])
            self.assertIn("slide", meta["attack_label_counts"])
            self.assertEqual(meta["attack_label_counts"]["slide"], 1)
            self.assertEqual(meta["specials"], ["slide"])

            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                cmd_report(Namespace(model_dir=model_dir))
            out = buf.getvalue()
            self.assertIn("specials:", out)
            self.assertIn("'slide': 1", out)


class AsurabldMetaOmitsSpecialsKeyTest(unittest.TestCase):
    """G1 preservation: a family with no `moves` table must not gain a new
    "specials" key in meta.json at all (empty-list would still change the
    key set the golden refit is compared against)."""

    def test_no_specials_key_when_family_has_no_moves(self):
        from shadow_train.__main__ import cmd_fit
        from .helpers import make_round_rows, write_jsonl

        rows = make_round_rows(dataset.P * (dataset.K + 1 + 20), round_id=1)
        with tempfile.TemporaryDirectory() as d:
            rec_path = Path(d) / "rec.jsonl"
            write_jsonl(rec_path, rows)
            model_dir = Path(d) / "model"
            args = Namespace(recordings=[rec_path], out=model_dir, char=None, k=9)
            with contextlib.redirect_stdout(io.StringIO()):
                cmd_fit(args)
            meta = json.loads((model_dir / "meta.json").read_text())

        self.assertNotIn("specials", meta)


if __name__ == "__main__":
    unittest.main()
