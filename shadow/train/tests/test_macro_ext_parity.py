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
import unittest
from pathlib import Path

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


if __name__ == "__main__":
    unittest.main()
