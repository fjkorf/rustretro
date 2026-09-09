import unittest
from types import SimpleNamespace

from shadow_train.segments import boundaries as B


def view(field_names):
    return SimpleNamespace(field_names=frozenset(field_names))


def row(i, *, h1=161, h2=161, x1=None, x2=None, y1=None, y2=None, frame=None):
    b1 = {"char_id": 7, "health": h1}
    b2 = {"char_id": 3, "health": h2}
    if x1 is not None:
        b1["x"] = x1
    if x2 is not None:
        b2["x"] = x2
    if y1 is not None:
        b1["y"] = y1
    if y2 is not None:
        b2["y"] = y2
    return {"frame": i if frame is None else frame, "round_id": 1,
            "controllable": True, "p1_block": 1,
            "block1": b1, "block2": b2, "p1_input": 0, "p2_input": 0}


CFG = {"seg_max": 180, "y_eps": 8, "knockdown_window": 60,
       "dist_close": 90, "dist_far": 150, "neutral_hold": 3, "start_slack": 6}


class ContactTest(unittest.TestCase):
    def test_one_boundary_per_health_drop_either_side(self):
        rows = [row(0), row(1, h2=155), row(2, h2=155), row(3, h1=150)]
        self.assertEqual(B.contact_boundaries(rows), [1, 3])

    def test_health_increase_never_fires(self):
        # refill / regen: health goes UP -> no contact (decrease-only).
        rows = [row(0, h1=100), row(1, h1=120), row(2, h1=161)]
        self.assertEqual(B.contact_boundaries(rows), [])


class NeutralHysteresisTest(unittest.TestCase):
    def test_fires_only_after_close_then_sustained_far(self):
        # close (dist 40) then far (dist 200) held for neutral_hold=3 frames.
        rows = [
            row(0, x1=100, x2=140),   # dist 40 close -> engaged
            row(1, x1=100, x2=300),   # far run 1
            row(2, x1=100, x2=300),   # far run 2
            row(3, x1=100, x2=300),   # far run 3 -> FIRE
            row(4, x1=100, x2=300),   # already disengaged, no re-fire
        ]
        self.assertEqual(B.neutral_boundaries(rows, 1, CFG), [3])

    def test_absent_x_freezes_the_counter_not_resets_it(self):
        # far run interrupted by an ABSENT-x frame must not reset — the run
        # resumes and still fires (absence is no information, not a reset).
        rows = [
            row(0, x1=100, x2=140),   # engaged
            row(1, x1=100, x2=300),   # far run 1
            row(2),                   # x absent -> freeze (run stays 1)
            row(3, x1=100, x2=300),   # far run 2
            row(4, x1=100, x2=300),   # far run 3 -> FIRE at row 4
        ]
        self.assertEqual(B.neutral_boundaries(rows, 1, CFG), [4])

    def test_never_fires_without_a_prior_close(self):
        rows = [row(i, x1=100, x2=300) for i in range(5)]  # far the whole time
        self.assertEqual(B.neutral_boundaries(rows, 1, CFG), [])


class WakeupTest(unittest.TestCase):
    def test_landing_after_a_contact_within_window_fires(self):
        # resting y = 0 (mode). A knockdown contact at row 1, airborne rows
        # 2-4, land at row 5 -> wakeup.
        rows = [
            row(0, y1=0), row(1, y1=0, h1=150),   # contact
            row(2, y1=40), row(3, y1=40), row(4, y1=40),  # airborne
            row(5, y1=0),                          # landed
        ]
        self.assertEqual(B.wakeup_boundaries(rows, 1, CFG), [5])

    def test_landing_without_a_preceding_contact_does_not_fire(self):
        # a plain jump (no health drop) is not a wakeup.
        rows = [row(0, y1=0), row(1, y1=40), row(2, y1=40), row(3, y1=0)]
        self.assertEqual(B.wakeup_boundaries(rows, 1, CFG), [])

    def test_absent_y_at_the_transition_yields_no_boundary(self):
        rows = [row(0, y1=0), row(1, y1=0, h1=150), row(2, y1=40), row(3)]
        # row 3 has no y -> the airborne->grounded transition can't be seen.
        self.assertEqual(B.wakeup_boundaries(rows, 1, CFG), [])


class RoundLevelTest(unittest.TestCase):
    def test_unavailable_detectors_are_named_when_fields_unmapped(self):
        rows = [row(0), row(1, h1=150), row(2)]
        # view maps neither x nor y -> both spacing detectors decline BY NAME.
        bs, unavail = B.boundaries_for_round(rows, view({"char_id", "health"}),
                                             1, {"boundaries": CFG})
        self.assertEqual(unavail, {B.WAKEUP, B.NEUTRAL})
        kinds = {b.kind for b in bs}
        self.assertIn(B.ROUND_START, kinds)
        self.assertIn(B.CONTACT, kinds)

    def test_row_zero_is_always_round_start(self):
        rows = [row(0), row(1)]
        bs, _ = B.boundaries_for_round(rows, view({"char_id", "health"}),
                                       1, {"boundaries": CFG})
        self.assertEqual(bs[0].row, 0)
        self.assertEqual(bs[0].kind, B.ROUND_START)


if __name__ == "__main__":
    unittest.main()
