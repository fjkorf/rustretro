"""Shared-fixture parity gate for MACRO_ACTIONS.md §10 (the Mileena/Reptile
step-vocabulary extensions: hold/release/while_held steps, §10.1; pinned
facing across a side swap, §10.2).

`shadow/train/tests/fixtures/macro_ext_golden.json` is the AUTHORITATIVE
truth, derived from the CONTRACT's rules (not from either implementation's
output -- see this file's sibling `test_matcher_parity.py` for the §2
original, and `src/macros.rs`'s `golden_ext_fixture_parity` test for the
Rust twin that runs the SAME file). A divergence between the two languages
fails one of these two tests rather than being discovered live.
"""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from shadow_train import macros as _macros
from shadow_train import profile as _profile
from shadow_train.macros import compile_macros, match_all

# MK2 arcade attack chords (library/mk2/mk2.profile.json) -- matches
# test_matcher_parity.py's ARCADE_CHORDS so both fixtures share one truth
# about what "HP"/"LK"/"Block"/etc. mean in button terms.
ARCADE_CHORDS = {
    "HP": ["y"],
    "LP": ["b"],
    "HK": ["x"],
    "LK": ["a"],
    "Block": ["l"],
}


class MacroExtFixtureParityTest(unittest.TestCase):
    """Load `macro_ext_golden.json` and validate each case matches the
    contract's expectations for the new hold/release/while_held steps and
    for pinned-facing resolution."""

    @classmethod
    def setUpClass(cls):
        fixture_path = Path(__file__).parent / "fixtures" / "macro_ext_golden.json"
        with open(fixture_path) as f:
            cls.golden = json.load(f)

    def _sides(self, facing, n: int) -> list:
        if isinstance(facing, str):
            side = 1 if facing == "right" else -1
            return [side] * n
        return [1 if f == "right" else -1 for f in facing]

    def test_fixture_is_not_empty(self):
        # A gate that silently exercises zero cases isn't a gate.
        self.assertGreaterEqual(len(self.golden), 6)

    def test_all_ext_golden_cases(self):
        for case in self.golden:
            with self.subTest(case=case["name"]):
                macros = compile_macros({case["move_name"]: case["macro"]})
                self.assertEqual(len(macros), 1)
                masks = [int(f, 16) for f in case["frames"]]
                sides = self._sides(case["facing"], len(masks))

                events = match_all(macros, masks, sides, ARCADE_CHORDS)

                actual = [(f, m) for f, m in events]
                expected = [(e["frame"], e["move"]) for e in case["expected"]]
                self.assertEqual(
                    actual, expected,
                    f"case '{case['name']}': expected {expected}, got {actual}",
                )


class MacroExtGoldenThroughProfileCompilerTest(unittest.TestCase):
    """Same golden fixture as `MacroExtFixtureParityTest` above, but routed
    through `shadow_train.profile`'s LOAD-TIME compilation of a real
    `special_inputs` JSON block (profile.py's `special_inputs` ->
    `GameProfile.macro_steps_for`) instead of handing hand-built step dicts
    straight to `compile_macros`.

    This is the path `dataset._macro_override_events` actually walks for
    every recorded round (`prof.special_inputs[char_name]` ->
    `compile_macros`). `MacroExtFixtureParityTest` above never touches
    `shadow_train.profile` at all, so a compiler that silently dropped
    `hold`/`release`/`while_held`/`min_frames` (the bug this test guards
    against) would sail straight through it -- the whole point of a shared
    golden fixture is that a divergence fails a test, and it cannot do that
    if the fixture bypasses the very code that diverged. Each case is
    written out as a tiny on-disk family.json + port profile pair and
    loaded for real via `profile.load()`."""

    @classmethod
    def setUpClass(cls):
        fixture_path = Path(__file__).parent / "fixtures" / "macro_ext_golden.json"
        with open(fixture_path) as f:
            cls.golden = json.load(f)

    def _sides(self, facing, n: int) -> list:
        if isinstance(facing, str):
            side = 1 if facing == "right" else -1
            return [side] * n
        return [1 if f == "right" else -1 for f in facing]

    def _load_profile_with_move(self, move_name: str, steps: list, tmpdir: str):
        family = {
            "family": "macroext",
            "roster": [{"id": 0, "name": "fighter"}],
            "move_classes": [],
            "attack_classes": ["None", "LP", "LK", "HP", "HK", "Block"],
            "moves": {"fighter": [{"name": move_name, "tags": ["special"]}]},
        }
        (Path(tmpdir) / "family.json").write_text(json.dumps(family))
        port = {
            "family": "macroext", "port": "test",
            "core": {"library_name": "", "provenance_game": "t", "provenance_core": "t"},
            "memory": {
                "blocks": {"block1": "0x0", "block2": "0x0", "stride": "0x0"},
                "fighter_fields": [], "globals": {},
            },
            "gate": [],
            "enforcement": {
                "health_max": 1, "refill_below": 1, "timer_hold": [0, 0],
                "credits_target": 0, "credits_min": 0,
            },
            "calibration": {},
            "attack_chords": ARCADE_CHORDS,
            "special_inputs": {"fighter": {move_name: steps}},
        }
        (Path(tmpdir) / "macroext.profile.json").write_text(json.dumps(port))
        return _profile.load(tmpdir)

    def test_fixture_is_not_empty(self):
        self.assertGreaterEqual(len(self.golden), 6)

    def test_all_ext_golden_cases_through_profile_compilation(self):
        for case in self.golden:
            with self.subTest(case=case["name"]):
                with tempfile.TemporaryDirectory() as d:
                    prof = self._load_profile_with_move(case["move_name"], case["macro"], d)
                    compiled_steps = prof.macro_steps_for("fighter", case["move_name"])
                    self.assertIsNotNone(compiled_steps)

                    macros = _macros.compile_macros({case["move_name"]: compiled_steps})
                    self.assertEqual(len(macros), 1)
                    masks = [int(f, 16) for f in case["frames"]]
                    sides = self._sides(case["facing"], len(masks))

                    events = _macros.match_all(macros, masks, sides, prof.attack_chords)

                    actual = [(f, m) for f, m in events]
                    expected = [(e["frame"], e["move"]) for e in case["expected"]]
                    self.assertEqual(
                        actual, expected,
                        f"case '{case['name']}' (via profile compiler): "
                        f"expected {expected}, got {actual}",
                    )


if __name__ == "__main__":
    unittest.main()
