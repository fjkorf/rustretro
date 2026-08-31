"""`framelab.spacing` (the per-matchup walk curve and collision floor) and
`framelab.guard` (the `guard_height` column), tested where they are pure.

Both modules exist because a number that was measured for ONE matchup was
being reused for another: the profile's `collision_floor_px` is a Reptile
mirror's 62, Mileena-vs-Reptile floors at 61, and `guard_height` was NULL
everywhere because nobody had built a crouching-defender rig. The live halves
are exercised against the real emulator; what is tested here is the logic
that decides what those live readings MEAN — which is exactly where a wrong
answer would be silent.
"""

from __future__ import annotations

import struct
import unittest

from shadow_train.framelab.guard import classify_guard
from shadow_train.framelab.spacing import (
    WalkPoint,
    collision_floor,
    curve_segments,
    walk_curve,
)

BLOCK1 = 0x8000
BLOCK2 = 0x9000
OBJ1 = 0x6000
OBJ2 = 0x7000


def _ptr_bytes(obj: int) -> bytes:
    return struct.pack("<I", (obj << 3) + 0x01000000)


class FakeWalkSession:
    """The narrowest session `walk_curve` needs: a memory it can read two
    object X values out of, and a walk that only advances while a direction
    is held. Records the tool/method order so the test can assert that K=0 is
    sampled BEFORE any frame runs and that the direction is asserted once."""

    def __init__(self, *, x1: int = 900, x2: int = 1100, speed: int = 3,
                 floor_px: int | None = None):
        self.x1, self.x2, self.speed, self.floor_px = x1, x2, speed, floor_px
        self.held: dict[int, tuple[str, ...]] = {0: (), 1: ()}
        self.order: list[str] = []
        self.loads = 0
        self.steps = 0

    # -- the bits of LabSession spacing.walk_curve uses -------------------
    def load_state(self, spec) -> None:
        self.order.append(f"load:{spec}")
        self.loads += 1

    def release_all_ports(self) -> None:
        self.held = {0: (), 1: ()}
        self.order.append("release_all")

    def set_held(self, port: int, buttons) -> None:
        self.held[port] = tuple(buttons)
        self.order.append(f"hold:{port}:{','.join(buttons)}")

    def release(self, port: int) -> None:
        self.held[port] = ()
        self.order.append(f"release:{port}")

    def step(self) -> None:
        self.order.append("step")
        self.steps += 1
        if "right" in self.held[0]:
            self.x1 += self.speed
            if self.floor_px is not None and self.x2 - self.x1 < self.floor_px:
                self.x1 = self.x2 - self.floor_px

    def call(self, tool: str, **kwargs) -> dict:
        assert tool == "read_memory", f"unexpected tool {tool!r}"
        addr, length = kwargs["addr"], kwargs["len"]
        mem = {
            BLOCK1: bytes([5]), BLOCK2: bytes([9]),
            BLOCK1 - 0x0C: _ptr_bytes(OBJ1), BLOCK2 - 0x0C: _ptr_bytes(OBJ2),
            OBJ1 + 0x3E: bytes([5]), OBJ2 + 0x3E: bytes([9]),
            OBJ1 + 0x12: struct.pack("<H", self.x1),
            OBJ2 + 0x12: struct.pack("<H", self.x2),
        }
        return {"ok": True, "hex": mem[addr][:length].hex()}


def _pts(gaps, start_k: int = 0):
    return [WalkPoint(k=start_k + i, gap_px=g, x_walker=None, x_other=None)
            for i, g in enumerate(gaps)]


class CollisionFloorTest(unittest.TestCase):
    def test_reports_the_plateau_the_curve_actually_sat_on(self):
        f = collision_floor(_pts([100, 97, 94, 91, 61, 61, 61, 61, 61, 61, 61]),
                            plateau_frames=6)
        self.assertEqual((f.floor_px, f.first_k), (61, 4))
        self.assertEqual(f.plateau_frames, 7)

    def test_refuses_a_minimum_that_was_only_just_touched(self):
        # The curve was still falling when the walk ran out: the last value
        # seen is NOT the floor, and reporting it would be a silent cap (§7).
        f = collision_floor(_pts([100, 97, 94, 91, 88, 85]), plateau_frames=6)
        self.assertIsNone(f.floor_px)
        self.assertIsNone(f.first_k)
        self.assertEqual(f.plateau_frames, 1)

    def test_a_minimum_that_does_not_reach_the_end_is_not_a_floor(self):
        # Walking past the floor can OPEN the gap again (both bodies slide).
        # The floor is then not the tail, so this refuses rather than
        # reporting a plateau the curve left.
        f = collision_floor(_pts([90, 61, 61, 61, 61, 61, 61, 63, 63]),
                            plateau_frames=3)
        self.assertIsNone(f.floor_px)

    def test_unknown_gaps_are_skipped_not_treated_as_zero(self):
        f = collision_floor(_pts([70, None, 61, 61, 61]), plateau_frames=3)
        self.assertEqual((f.floor_px, f.first_k), (61, 2))

    def test_all_unknown_is_all_null(self):
        f = collision_floor(_pts([None, None]), plateau_frames=1)
        self.assertEqual((f.floor_px, f.first_k, f.plateau_frames), (None, None, 0))


class CurveSegmentsTest(unittest.TestCase):
    def test_run_length_encodes_equal_closing_rates(self):
        segs = curve_segments(_pts([192, 192, 189, 186, 183]))
        self.assertEqual(
            [(s["from_k"], s["to_k"], s["px_per_frame"]) for s in segs],
            [(0, 1, 0.0), (1, 4, 3.0)],
        )

    def test_too_short_to_have_a_rate(self):
        self.assertEqual(curve_segments(_pts([100])), [])


class WalkCurveTest(unittest.TestCase):
    def test_k0_is_sampled_before_any_frame_runs(self):
        s = FakeWalkSession()
        pts = walk_curve(s, base_arena="a.state", walk_port=0,
                         walk_direction="right", block1_addr=BLOCK1,
                         block2_addr=BLOCK2, max_k=3)
        self.assertEqual([p.gap_px for p in pts], [200, 197, 194, 191])
        # load, release, (K=0 read), hold once, then step per frame.
        self.assertEqual(s.order[:3], ["load:a.state", "release_all", "hold:0:right"])
        self.assertEqual(s.order.count("hold:0:right"), 1)
        self.assertEqual(s.steps, 3)
        self.assertEqual(s.order[-1], "release:0")

    def test_max_k_zero_reads_the_base_arena_and_never_holds(self):
        s = FakeWalkSession()
        pts = walk_curve(s, base_arena=3, walk_port=0, walk_direction="right",
                         block1_addr=BLOCK1, block2_addr=BLOCK2, max_k=0)
        self.assertEqual([(p.k, p.gap_px) for p in pts], [(0, 200)])
        self.assertEqual(s.steps, 0)
        self.assertNotIn("hold:0:right", s.order)

    def test_a_floor_shows_up_as_a_plateau_in_the_curve(self):
        s = FakeWalkSession(floor_px=61)
        pts = walk_curve(s, base_arena="a.state", walk_port=0,
                         walk_direction="right", block1_addr=BLOCK1,
                         block2_addr=BLOCK2, max_k=60)
        f = collision_floor(pts)
        self.assertEqual(f.floor_px, 61)

    def test_rejects_a_negative_max_k(self):
        with self.assertRaises(ValueError):
            walk_curve(FakeWalkSession(), base_arena="a", walk_port=0,
                       walk_direction="right", block1_addr=BLOCK1,
                       block2_addr=BLOCK2, max_k=-1)


class ClassifyGuardTest(unittest.TestCase):
    def test_mid_when_both_stances_chip(self):
        self.assertEqual(classify_guard(24, 6, 6), "mid")

    def test_low_when_standing_eats_it_whole(self):
        self.assertEqual(classify_guard(12, 12, 3), "low")

    def test_whiffing_against_a_standing_guard_is_not_a_guard_height(self):
        # Measured live on Mileena's far HP at 83 px: 11 open, chip against a
        # crouch-block, NO CONTACT against a standing block. Reading "took no
        # damage" as "blocked it" would label that `low`, backwards.
        self.assertEqual(classify_guard(11, None, 3), "whiffs_vs_guard")

    def test_overhead_when_crouching_eats_it_whole(self):
        self.assertEqual(classify_guard(24, 6, 24), "overhead")

    def test_unblockable_when_neither_stance_changes_anything(self):
        self.assertEqual(classify_guard(30, 30, 30), "unblockable")

    def test_high_when_it_whiffs_over_a_crouching_defender(self):
        # No contact at all is not "blocked" (§1.1) — it is a different
        # outcome, and calling it `mid` would be inferring block from a
        # health delta, which §2.6 forbids.
        self.assertEqual(classify_guard(11, 3, None), "high")

    def test_null_when_the_move_does_not_reach_an_open_defender(self):
        self.assertIsNone(classify_guard(None, None, None))


if __name__ == "__main__":
    unittest.main()
