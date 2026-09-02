import unittest

from shadow_train.segments import mirror as M


class MirrorMaskTest(unittest.TestCase):
    def test_involution_over_all_4096_masks_only_bits_6_7_change(self):
        for m in range(4096):
            mm = M.mirror_mask(m)
            self.assertEqual(M.mirror_mask(mm), m, f"not an involution at {m}")
            # every bit except 6/7 is unchanged
            other = ~((1 << 6) | (1 << 7)) & 0xFFF
            self.assertEqual(m & other, mm & other, f"non-LR bit changed at {m}")

    def test_swaps_left_and_right(self):
        self.assertEqual(M.mirror_mask(1 << 6), 1 << 7)
        self.assertEqual(M.mirror_mask(1 << 7), 1 << 6)
        # up (bit 4) + a face button (bit 1) pass through
        keep = (1 << 4) | (1 << 1)
        self.assertEqual(M.mirror_mask(keep), keep)


class FacingLatchTest(unittest.TestCase):
    def test_pins_on_first_present_frame(self):
        latch = M.FacingLatch(swap_debounce=3)
        self.assertIsNone(latch.update(None, None))   # absent -> no pin yet
        self.assertEqual(latch.update(100, 200), 1)    # opp to the right -> +1

    def test_holds_last_known_on_absent_never_zero(self):
        latch = M.FacingLatch(swap_debounce=3)
        latch.update(100, 200)                         # +1
        self.assertEqual(latch.update(None, 200), 1)   # me absent -> hold +1
        self.assertEqual(latch.update(100, None), 1)   # opp absent -> hold +1

    def test_single_frame_crossover_does_not_flip(self):
        latch = M.FacingLatch(swap_debounce=3)
        latch.update(100, 200)                         # +1
        self.assertEqual(latch.update(200, 100), 1)    # 1 opposite frame: hold
        self.assertEqual(latch.update(100, 200), 1)    # back -> run resets

    def test_relatches_after_debounced_confirmed_swap(self):
        latch = M.FacingLatch(swap_debounce=3)
        latch.update(100, 200)                         # +1
        self.assertEqual(latch.update(200, 100), 1)    # opp run 1
        self.assertEqual(latch.update(200, 100), 1)    # opp run 2
        self.assertEqual(latch.update(200, 100), -1)   # opp run 3 -> flip

    def test_equal_x_is_no_confirmation(self):
        latch = M.FacingLatch(swap_debounce=2)
        latch.update(100, 200)                         # +1
        self.assertEqual(latch.update(150, 150), 1)    # tie -> hold, run reset
        self.assertEqual(latch.update(200, 100), 1)    # only 1 opposite so far
        self.assertEqual(latch.update(200, 100), -1)   # 2 -> flip


class MirrorStreamTest(unittest.TestCase):
    def test_opposite_side_swaps_every_frame(self):
        masks = [1 << 6, 1 << 7, 1 << 4]  # left, right, up
        out = M.mirror_stream(masks, recorded_sign_trace=[1, 1, 1], live_sign=-1)
        self.assertEqual(out, (1 << 7, 1 << 6, 1 << 4))
        self.assertIsInstance(out, tuple)

    def test_same_side_passes_through_unchanged(self):
        masks = [1 << 6, 1 << 7]
        out = M.mirror_stream(masks, recorded_sign_trace=[1], live_sign=1)
        self.assertEqual(out, (1 << 6, 1 << 7))

    def test_unknown_facing_passes_through_not_guessed(self):
        masks = [1 << 6]
        self.assertEqual(M.mirror_stream(masks, [None], live_sign=-1), (1 << 6,))
        self.assertEqual(M.mirror_stream(masks, [1], live_sign=None), (1 << 6,))

    def test_start_sign_holds_through_leading_absence(self):
        # a leading absent frame does not lose the start facing
        out = M.mirror_stream([1 << 6], recorded_sign_trace=[None, 1], live_sign=-1)
        self.assertEqual(out, (1 << 7,))


if __name__ == "__main__":
    unittest.main()
