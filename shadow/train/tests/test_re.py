from __future__ import annotations

import unittest

from shadow_train.re import (
    BUTTON_MASKS,
    diff,
    intersect_changes,
    lua_macro,
    static_diff,
)


class DiffTest(unittest.TestCase):
    def test_finds_changed_bytes_only(self):
        a = bytes([1, 2, 3, 4])
        b = bytes([1, 9, 3, 8])
        self.assertEqual(diff(a, b), [(1, 2, 9), (3, 4, 8)])

    def test_no_changes(self):
        a = bytes([1, 2, 3])
        self.assertEqual(diff(a, a), [])

    def test_base_shifts_addresses_to_guest_absolute(self):
        a = bytes([0, 0])
        b = bytes([0, 5])
        self.assertEqual(diff(a, b, base=0x1000), [(0x1001, 0, 5)])

    def test_truncates_to_shorter_snapshot(self):
        a = bytes([1, 2, 3])
        b = bytes([9, 2])
        self.assertEqual(diff(a, b), [(0, 1, 9)])


class StaticDiffTest(unittest.TestCase):
    """Static-diff: stable-in-both-states AND differing across states."""

    def test_finds_config_byte_while_pruning_unstable_noise(self):
        # offset 0: a config byte, stable in both states, and differs
        #   between them -> should be reported.
        # offset 1: a frame counter, unstable within state A -> pruned even
        #   though it also differs between snapshot pairs.
        # offset 2: stable in both states but identical value -> not a diff.
        state_a = (bytes([10, 50, 7]), {0, 2})
        state_b = (bytes([20, 51, 7]), {0, 1, 2})
        self.assertEqual(static_diff(state_a, state_b), [(0, 10, 20)])

    def test_empty_when_no_common_stable_offsets(self):
        state_a = (bytes([10, 50]), {1})
        state_b = (bytes([20, 51]), {0})
        self.assertEqual(static_diff(state_a, state_b), [])

    def test_base_shifts_addresses(self):
        state_a = (bytes([1]), {0})
        state_b = (bytes([2]), {0})
        self.assertEqual(static_diff(state_a, state_b, base=0xFF0000), [(0xFF0000, 1, 2)])


class IntersectChangesTest(unittest.TestCase):
    """Toggle-intersect: survives only offsets that changed at EVERY step."""

    def test_survives_only_offsets_changed_every_step(self):
        # offset 0: changes every step (1 -> 2 -> 3 -> 4): survives.
        # offset 1: stays constant: pruned.
        # offset 2: changes once then holds (5 -> 6 -> 6): pruned (fails the
        #   second step).
        snaps = [
            bytes([1, 9, 5]),
            bytes([2, 9, 6]),
            bytes([3, 9, 6]),
            bytes([4, 9, 6]),
        ]
        result = intersect_changes(snaps)
        self.assertEqual(result, [(0, [1, 2, 3, 4])])

    def test_single_snapshot_yields_nothing(self):
        self.assertEqual(intersect_changes([bytes([1, 2, 3])]), [])

    def test_base_shifts_addresses(self):
        snaps = [bytes([1]), bytes([2])]
        self.assertEqual(intersect_changes(snaps, base=0x400000), [(0x400000, [1, 2])])


class LuaMacroTest(unittest.TestCase):
    def test_button_masks_table_matches_documented_bits(self):
        self.assertEqual(BUTTON_MASKS["start"], 0x8)
        self.assertEqual(BUTTON_MASKS["b"], 0x1)
        self.assertEqual(BUTTON_MASKS["down"], 0x20)

    def test_schedule_entries_present_with_resolved_masks(self):
        src = lua_macro([(5, "start", 3), (90, ["down", "b"], 4)])
        self.assertIn("{at=5, mask=0x8, hold=3}", src)
        self.assertIn("{at=90, mask=0x21, hold=4}", src)

    def test_accepts_raw_int_mask(self):
        src = lua_macro([(0, 0x100, 2)])
        self.assertIn("{at=0, mask=0x100, hold=2}", src)

    def test_unknown_button_name_raises(self):
        with self.assertRaises(ValueError):
            lua_macro([(0, "nonexistent", 2)])

    def test_terminates_with_a_done_flag_and_timeout(self):
        src = lua_macro([(5, "start", 3)])
        self.assertIn("local done = false", src)
        self.assertIn("done = true", src)
        # default timeout = last event's end (5+3=8) + 20 frames of slack.
        self.assertIn("f > 28", src)

    def test_explicit_timeout_frames_overrides_default(self):
        src = lua_macro([(5, "start", 3)], timeout_frames=500)
        self.assertIn("f > 500", src)

    def test_registers_onframeend_and_uses_input_set(self):
        src = lua_macro([(0, "a", 1)], port=1)
        self.assertIn("event.onframeend(function()", src)
        self.assertIn("input.set(port, s.mask)", src)
        self.assertIn("local port = 1", src)


if __name__ == "__main__":
    unittest.main()
