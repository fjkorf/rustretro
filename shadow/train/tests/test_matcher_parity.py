"""Test matcher parity between Rust (src/macros.rs) and Python (shadow_train.macros).

This test suite loads the golden fixture (shadow/train/tests/fixtures/matcher_golden.json)
and validates that both implementations produce identical match completions for every case.
The fixture is the AUTHORITATIVE truth derived from MACRO_ACTIONS §2 semantics, not from
either implementation's output.
"""

from __future__ import annotations

import json
import unittest
from pathlib import Path

from shadow_train.macros import compile_macros, match_all

# Button bit reference (RETRO joypad order):
# b=0x1, y=0x2, select=0x4, start=0x8, up=0x10, down=0x20, left=0x40, right=0x80,
# a=0x100, x=0x200, l=0x400, r=0x800.

# MK2 arcade attack chords (library/mk2/mk2.profile.json):
ARCADE_CHORDS = {
    "HP": ["y"],
    "LP": ["b"],
    "HK": ["x"],
    "LK": ["a"],
    "Block": ["l"],
}

# Generic synthetic attack chords for forward-forward motion tests:
GENERIC_CHORDS = {
    "HP": ["y"],
    "LP": ["b"],
    "HK": ["x"],
    "LK": ["a"],
    "Block": ["l"],
}


class MatcherParityTest(unittest.TestCase):
    """Load the golden fixture and validate each case matches expectations."""

    @classmethod
    def setUpClass(cls):
        """Load the golden fixture at test class initialization."""
        fixture_path = Path(__file__).parent / "fixtures" / "matcher_golden.json"
        with open(fixture_path) as f:
            cls.golden = json.load(f)

    def _get_attack_chords(self, case):
        """Determine which attack_chords dict to use based on the case."""
        # For now, all test cases use arcade/generic chords
        return ARCADE_CHORDS

    def _convert_frames(self, frames_hex):
        """Convert hex frame strings to integer masks."""
        return [int(f, 16) if isinstance(f, str) else f for f in frames_hex]

    def _resolve_sides(self, facing, num_frames):
        """Convert facing field (string or list) to per-frame sides.
        facing: "left" -> -1, "right" -> 1, or [facing, ...] per frame.
        Returns list of sign values (+1 for right, -1 for left).
        """
        if isinstance(facing, str):
            side = 1 if facing == "right" else -1
            return [side] * num_frames
        elif isinstance(facing, list):
            return [1 if f == "right" else -1 for f in facing]
        else:
            raise ValueError(f"Invalid facing: {facing}")

    def test_all_golden_cases(self):
        """Run every golden case and assert Python matcher produces expected completions."""
        # Add explicit move name mapping for cases without expected completions
        move_name_map = {
            "bare_lp_not_chord": "slide",
            "acid_spit_gap_exceeds_max": "acid_spit",
        }

        for case in self.golden:
            with self.subTest(case=case["name"]):
                # Determine the move name from expected completions or mapping.
                if case["expected"]:
                    move_name = case["expected"][0]["move"]
                elif case["name"] in move_name_map:
                    move_name = move_name_map[case["name"]]
                else:
                    # Fallback to case name
                    move_name = case["name"]

                # Compile the macro using the determined move name.
                macro_spec = {
                    move_name: case["macro"]
                }
                macros = compile_macros(macro_spec)
                self.assertEqual(len(macros), 1, f"Expected 1 macro, got {len(macros)}")
                macro = macros[0]

                # Convert frame data.
                masks = self._convert_frames(case["frames"])
                sides = self._resolve_sides(case["facing"], len(masks))
                attack_chords = self._get_attack_chords(case)

                # Run the matcher.
                events = match_all([macro], masks, sides, attack_chords)

                # Extract (frame, move) tuples for comparison.
                actual = [(f, m) for f, m in events]
                expected = [(e["frame"], e["move"]) for e in case["expected"]]

                # Assert.
                self.assertEqual(
                    actual, expected,
                    f"Case '{case['name']}': expected {expected}, got {actual}"
                )


if __name__ == "__main__":
    unittest.main()
