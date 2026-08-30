"""Unit tests for `framelab.ladder` — the connect map, `connect_range`, and
the re-measurement comparison docs/frames.md §8.1 makes an acceptance
criterion.

The comparison is the part that must not be clever. §7: "a number that fails
re-measurement is DELETED, not averaged." A comparator that rounds, that
tolerates a frame, or that quietly prefers the fresh value would turn the one
criterion that tests ACCURACY into a rubber stamp — so these tests pin that
it reports disagreement rather than resolving it.
"""

from __future__ import annotations

import unittest

from shadow_train.framelab import ladder
from shadow_train.framelab.kit import ContactScan

from .test_framelab_probe import FakeGame, contact_read, make_session, sampler
from .test_framelab_kit import DIRS, PUNCH, fake_rig


def row(**kw):
    base = dict(
        char="reptile", move="HP", variant="far", gap_px=72.0,
        observable="struct_velocity", on_hit=4, on_block=13, damage=11,
        hits=1, knockdown=0, first_active_frame=None, connect_range=72,
        gap_walk_frames=45, input_latency_frames=1, method="linear_sweep",
        rig_guard_state="held+none", core_id="core:sha256:aaaa",
        rom_id="rom:sha256:bbbb",
    )
    base.update(kw)
    return base


class ConnectRangeTest(unittest.TestCase):
    def _map(self, *entries):
        return {
            (move, f"arena-{gap}"): ContactScan(
                move=move, gap_px=gap, connected=dmg is not None, damage=dmg,
            )
            for move, gap, dmg in entries
        }

    def test_connect_range_is_the_largest_connecting_rung(self):
        r = ladder.connect_ranges(self._map(
            ("HK", 62, 16), ("HK", 72, 32), ("HK", 110, 32), ("HK", 147, None),
        ))
        self.assertEqual(r["HK"], 110)

    def test_a_move_that_connects_nowhere_is_null_not_zero(self):
        # §2.5: absent means absent. A 0 here reads as "connects at point
        # blank only", which is a completely different claim.
        r = ladder.connect_ranges(self._map(("cLK", 110, None), ("cLK", 147, None)))
        self.assertIsNone(r["cLK"])

    def test_an_unknown_gap_does_not_become_a_range(self):
        # An arena whose object pointer did not resolve has gap_px None; it
        # must not be counted as a rung the move reached.
        r = ladder.connect_ranges(self._map(("HP", None, 11), ("HP", 62, 24)))
        self.assertEqual(r["HP"], 62)


class CompareRowsTest(unittest.TestCase):
    def test_identical_rows_are_identical(self):
        v = ladder.compare_rows([row()], [row()])
        self.assertTrue(v["identical"])
        self.assertEqual(v["compared"], 1)
        self.assertEqual(v["differing"], [])

    def test_a_single_frame_of_disagreement_fails(self):
        v = ladder.compare_rows([row(on_block=12)], [row(on_block=13)])
        self.assertFalse(v["identical"])
        self.assertEqual(
            v["differing"][0]["columns"]["on_block"],
            {"stored": 13, "fresh": 12},
        )

    def test_provenance_that_is_expected_to_differ_is_not_compared(self):
        # `measured_at`/`id`/`sample_n` differ between any two runs by
        # construction; comparing them would make every re-measurement fail.
        a = dict(row(), id=1, measured_at="2026-08-30T00:00:00", sample_n=1)
        b = dict(row(), id=99, measured_at="2026-08-31T00:00:00", sample_n=2)
        self.assertTrue(ladder.compare_rows([b], [a])["identical"])

    def test_a_different_core_or_rom_is_a_disagreement(self):
        # §6: a number measured against different bytes is a different
        # number. Silently comparing across builds is the stale-row failure
        # core_id exists to prevent.
        v = ladder.compare_rows([row(core_id="core:sha256:zzzz")], [row()])
        self.assertFalse(v["identical"])
        self.assertIn("core_id", v["differing"][0]["columns"])

    def test_a_stored_row_this_run_did_not_produce_is_reported_not_ignored(self):
        v = ladder.compare_rows([], [row()])
        self.assertFalse(v["identical"])
        self.assertEqual(len(v["missing_from_this_run"]), 1)
        self.assertEqual(v["missing_from_this_run"][0]["move"], "HP")

    def test_a_newly_measured_cell_is_reported_but_is_not_a_failure(self):
        # Newly affordable coverage is not a re-measurement failure: nothing
        # it could disagree with exists.
        v = ladder.compare_rows([row(), row(move="cLK", variant="close")], [row()])
        self.assertTrue(v["identical"])
        self.assertEqual(len(v["new_in_this_run"]), 1)

    def test_rows_are_keyed_per_observable(self):
        # §6 stores one row per observable because they are different
        # experiments; the comparison must not collapse them.
        stored = [row(observable="struct_velocity"), row(observable="pointer_x")]
        fresh = [row(observable="struct_velocity"),
                 row(observable="pointer_x", on_hit=5)]
        v = ladder.compare_rows(fresh, stored)
        self.assertEqual(v["compared"], 2)
        self.assertEqual(len(v["differing"]), 1)
        self.assertEqual(v["differing"][0]["key"]["observable"], "pointer_x")


class ConnectMapTest(unittest.TestCase):
    """The map against the toy fighter: a whiff is a recorded OUTCOME."""

    def test_a_whiff_is_recorded_as_a_cell_not_omitted(self):
        game = FakeGame(reach=1)      # nothing can connect
        s = make_session(game)
        rung = ladder.Rung(arena="fake.state", gap_px=180, gap_walk_frames=0)
        cmap = ladder.connect_map(
            s, specs=[PUNCH], rungs=[rung],
            guard_buttons=(FakeGame.GUARD,), contact_read=contact_read,
            quiet_frames=20, walk_directions_by_port=DIRS,
        )
        scan = cmap[("HP", "fake.state")]
        self.assertFalse(scan.connected)
        self.assertIsNone(scan.damage)
        self.assertIsNone(ladder.connect_ranges(cmap)["HP"])

    def test_a_connecting_cell_carries_its_damage_and_contact_frame(self):
        game = FakeGame()
        s = make_session(game)
        rung = ladder.Rung(arena="fake.state", gap_px=62, gap_walk_frames=60)
        cmap = ladder.connect_map(
            s, specs=[PUNCH], rungs=[rung],
            guard_buttons=(FakeGame.GUARD,), contact_read=contact_read,
            quiet_frames=20, walk_directions_by_port=DIRS,
        )
        scan = cmap[("HP", "fake.state")]
        self.assertTrue(scan.connected)
        self.assertEqual(scan.damage, 11)
        self.assertEqual(scan.hits, 1)
        self.assertEqual(ladder.connect_ranges(cmap)["HP"], 62)


class MeasureLadderTest(unittest.TestCase):
    def test_a_refused_cell_is_reported_and_does_not_abort_the_rest(self):
        # §7's "no silent caps" both ways: one unmeasurable cell must not
        # cost the others, and must not vanish.
        game = FakeGame()
        s = make_session(game)
        rung = ladder.Rung(arena="fake.state", gap_px=62, gap_walk_frames=60)
        lat = {sh: {"x": 1, "struct": 1}
               for sh in ("attacker/hit", "defender/hit",
                          "attacker/block", "defender/block")}
        ms, refusals = ladder.measure_ladder(
            s, cells=[(PUNCH, rung, "close")],
            guard_buttons=(FakeGame.GUARD,), contact_read=contact_read,
            sample_fns={0: sampler(0), 1: sampler(1)}, latencies=lat,
            observables=["x", "struct"], quiet_frames=20,
            walk_directions_by_port=DIRS, ranges={"HP": 62},
            faf_at_px=62, max_search=45,
        )
        self.assertEqual(refusals, [])
        self.assertEqual(len(ms), 1)
        self.assertEqual(ms[0].connect_range, 62)
        # FAF is the contact frame relative to the MOVE's own input frame.
        self.assertEqual(ms[0].first_active_frame, ms[0].scan_hit.contact_frame)

    def test_faf_is_stored_only_at_the_minimum_gap(self):
        # §4.4: at larger gaps FAF is contaminated by travel, so it is NULL
        # there rather than a slightly-wrong number.
        game = FakeGame()
        s = make_session(game)
        rung = ladder.Rung(arena="fake.state", gap_px=110, gap_walk_frames=30)
        lat = {sh: {"x": 1} for sh in ("attacker/hit", "defender/hit",
                                       "attacker/block", "defender/block")}
        ms, _ = ladder.measure_ladder(
            s, cells=[(PUNCH, rung, "far")], guard_buttons=(FakeGame.GUARD,),
            contact_read=contact_read,
            sample_fns={0: sampler(0), 1: sampler(1)}, latencies=lat,
            observables=["x"], quiet_frames=20, walk_directions_by_port=DIRS,
            ranges={"HP": 110}, faf_at_px=62, max_search=45,
        )
        self.assertIsNone(ms[0].first_active_frame)


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
