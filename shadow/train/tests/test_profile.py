from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from shadow_train import asurabld, dataset, evaluate, knn, profile


class ProfileLoadingTest(unittest.TestCase):
    """Spot-checks against library/asurabld's shipped profile pair -- mirrors
    src/profile.rs's own `shipped_asurabld_profile_parses_and_matches_the_old_
    constants` test so the two loaders agree on the same JSON."""

    def setUp(self):
        self.p = profile.get()

    def test_family_and_port(self):
        self.assertEqual(self.p.family, "asurabld")
        self.assertEqual(self.p.port, "arcade")

    def test_blocks(self):
        self.assertEqual(self.p.block1(), 0x403798)
        self.assertEqual(self.p.block2(), 0x40454C)
        self.assertEqual(self.p.stride(), 0xDB4)
        self.assertEqual(self.p.block2() - self.p.block1(), self.p.stride())

    def test_globals(self):
        self.assertEqual(self.p.global_addr("round_timer"), 0x40000A)
        self.assertEqual(self.p.global_addr("char_select"), 0x400006)
        self.assertEqual(self.p.global_addr("credits"), 0x40655D)
        self.assertIsNone(self.p.global_addr("nonexistent_global"))

    def test_field_off(self):
        self.assertEqual(self.p.field_off("health"), (0x177, 1))
        self.assertEqual(self.p.field_off("char_id"), (0x639, 1))
        self.assertIsNone(self.p.field_off("nonexistent_field"))

    def test_char_name(self):
        self.assertEqual(self.p.char_name(1), "goat")
        self.assertEqual(self.p.char_name(7), "rosemary")
        self.assertEqual(self.p.char_name(9), "sgeist")
        self.assertEqual(self.p.char_name(11), "c11")

    def test_matchup_slug(self):
        self.assertEqual(self.p.matchup_slug(1, 7), "goat-vs-rosemary")
        self.assertEqual(self.p.matchup_slug(1, None), "goat")
        self.assertEqual(self.p.matchup_slug(None, 7), "any-vs-rosemary")
        self.assertEqual(self.p.matchup_slug(None, None), "all")

    def test_stage_selector_roundtrip(self):
        self.assertEqual(self.p.stage_value_for_opponent(7), 5)
        self.assertEqual(self.p.opponent_for_stage_value(9), 9)
        self.assertIsNone(self.p.stage_value_for_opponent(3))  # footee has no stage value

    def test_gate_and_chords(self):
        self.assertEqual(len(self.p.gate), 6)
        globals_named = {c.get("global") for c in self.p.gate if c.get("global")}
        self.assertIn("char_select", globals_named)
        for cls in self.p.attack_classes:
            if cls != "None":
                self.assertIn(cls, self.p.attack_chords, f"{cls} chord missing")

    def test_enforcement_and_calibration(self):
        self.assertEqual(self.p.enforcement["health_max"], 0xEF)
        self.assertEqual(self.p.enforcement["timer_hold"], [0x85, 0x03])
        self.assertEqual(self.p.calibration_value("GROUND_Y"), 216)

    def test_class_lists(self):
        self.assertEqual(len(self.p.move_classes), 9)
        self.assertEqual(len(self.p.attack_classes), 6)

    def test_env_var_override(self, ):
        import os

        with tempfile.TemporaryDirectory() as d:
            game_dir = Path(d) / "mygame"
            game_dir.mkdir()
            (game_dir / "family.json").write_text(json.dumps({
                "family": "mygame",
                "roster": [{"id": 0, "name": "hero", "select_slot": 0}],
                "move_classes": ["Neutral", "Forward"],
                "attack_classes": ["None", "Light"],
            }))
            (game_dir / "mygame.profile.json").write_text(json.dumps({
                "family": "mygame", "port": "test",
                "core": {"provenance_game": "mygame", "provenance_core": "test"},
                "memory": {
                    "endianness": "big",
                    "blocks": {"block1": "0x1000", "block2": "0x2000", "stride": "0x1000"},
                    "fighter_fields": [{"name": "x", "off": "0x10", "size": 2}],
                    "globals": {"round_timer": "0x50"},
                },
                "gate": [],
                "enforcement": {"health_max": 100, "refill_below": 10,
                                 "timer_hold": [0, 0], "credits_target": 1, "credits_min": 1},
                "calibration": {"GROUND_Y": 100},
                "attack_chords": {"Light": ["b"]},
            }))

            loaded_explicit = profile.load(game_dir)
            self.assertEqual(loaded_explicit.family, "mygame")
            self.assertEqual(loaded_explicit.block1(), 0x1000)

            old = os.environ.get("RUSTRETRO_GAME_DIR")
            try:
                os.environ["RUSTRETRO_GAME_DIR"] = str(game_dir)
                got = profile.get()
                self.assertEqual(got.family, "mygame")
            finally:
                if old is None:
                    os.environ.pop("RUSTRETRO_GAME_DIR", None)
                else:
                    os.environ["RUSTRETRO_GAME_DIR"] = old

    def test_family_mismatch_raises(self):
        with tempfile.TemporaryDirectory() as d:
            game_dir = Path(d) / "mismatched"
            game_dir.mkdir()
            (game_dir / "family.json").write_text(json.dumps({
                "family": "gameA", "roster": [], "move_classes": [], "attack_classes": [],
            }))
            (game_dir / "mismatched.profile.json").write_text(json.dumps({
                "family": "gameB", "port": "test",
                "core": {"provenance_game": "x", "provenance_core": "y"},
                "memory": {"blocks": {"block1": "0x1", "block2": "0x2", "stride": "0x1"},
                           "fighter_fields": [], "globals": {}},
                "gate": [], "enforcement": {"health_max": 1, "refill_below": 1,
                                             "timer_hold": [0, 0], "credits_target": 1,
                                             "credits_min": 1},
                "calibration": {}, "attack_chords": {},
            }))
            with self.assertRaises(profile.ProfileError):
                profile.load(game_dir)

    def test_hex_addr_parsing_matches_rust_adapter(self):
        self.assertEqual(profile._parse_addr("0x403798"), 0x403798)
        self.assertEqual(profile._parse_addr("403798"), 0x403798)
        self.assertEqual(profile._parse_addr(0x403798), 0x403798)


class AsurabldParityTest(unittest.TestCase):
    """asurabld.py's public constants must exactly equal the values the
    profile loader resolves -- the loader-view contract (task item 2)."""

    def test_constants_match_profile(self):
        p = profile.get()
        self.assertEqual(asurabld.BLOCK1, 0x403798)
        self.assertEqual(asurabld.BLOCK1, p.block1())
        self.assertEqual(asurabld.BLOCK2, p.block2())
        self.assertEqual(asurabld.STRIDE, p.stride())
        self.assertEqual(asurabld.HEALTH, p.field_off("health")[0])
        self.assertEqual(asurabld.CHAR_ID, p.field_off("char_id")[0])
        self.assertEqual(asurabld.ROUND_TIMER, p.global_addr("round_timer"))
        self.assertEqual(asurabld.CREDITS, p.global_addr("credits"))

    def test_char_name_and_matchup_slug(self):
        self.assertEqual(asurabld.char_name(7), "rosemary")
        self.assertEqual(asurabld.CHAR_NAMES[7], "rosemary")
        self.assertEqual(asurabld.matchup_slug(1, 7), "goat-vs-rosemary")

    def test_world_constants(self):
        self.assertEqual(asurabld.GROUND_Y, 216)
        self.assertEqual(asurabld.ROUND_START_Y, asurabld.GROUND_Y)
        self.assertEqual(asurabld.ROUND_START_X_LEFT, 84)
        self.assertEqual(asurabld.ROUND_START_X_RIGHT, 232)


class HeadSizeDerivationTest(unittest.TestCase):
    """Task item 4: nothing may hardcode 9 moves / 6 attacks -- every head
    size must equal len(MOVE_CLASSES)/len(ATTACK_CLASSES) as loaded from the
    profile's family.json, not a literal."""

    def test_class_list_lengths(self):
        self.assertEqual(len(dataset.MOVE_CLASSES), 9)
        self.assertEqual(len(dataset.ATTACK_CLASSES), 6)
        self.assertEqual(dataset.MOVE_CLASSES, profile.get().move_classes)
        self.assertEqual(dataset.ATTACK_CLASSES, profile.get().attack_classes)

    def test_evaluate_minlength_derives_from_class_lists(self):
        self.assertEqual(evaluate.N_MOVE_CLASSES, len(dataset.MOVE_CLASSES))
        self.assertEqual(evaluate.N_ATTACK_CLASSES, len(dataset.ATTACK_CLASSES))

    def test_knn_predict_proba_head_sizes_track_class_lists(self):
        import numpy as np

        rng = np.random.default_rng(0)
        n = 50
        X = rng.normal(size=(n, 4)).astype(np.float32)
        y_move = rng.integers(0, len(dataset.MOVE_CLASSES), size=n)
        y_attack = rng.integers(0, len(dataset.ATTACK_CLASSES), size=n)
        policy = knn.KnnPolicy(k=5).fit(X, y_move, y_attack)
        pm, pa = policy.predict_proba(X[0])
        self.assertEqual(len(pm), len(dataset.MOVE_CLASSES))
        self.assertEqual(len(pa), len(dataset.ATTACK_CLASSES))

    def test_calibration_keys_and_meta_family_port(self):
        from shadow_train.__main__ import CALIBRATION_KEYS

        prof = profile.get()
        self.assertEqual(CALIBRATION_KEYS, list(prof.calibration.keys()))
        for key in CALIBRATION_KEYS:
            self.assertEqual(getattr(dataset, key), prof.calibration[key])


if __name__ == "__main__":
    unittest.main()
