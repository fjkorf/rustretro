"""Unit tests for `framelab.kit` — the kit/ladder layer over the act-again
probe (`docs/frames.md` §5's spacing ladder, §1.1's outcome table, §7's
honesty rules).

They run against `test_framelab_probe.FakeGame`, the same toy fighter the
probe's own tests use, because the properties that matter here are the ones
it already models: pushback that moves the defender with no input, a stun
that outlives it, and a `reach` beyond which nothing connects.

Each test below corresponds to a mistake that was made — or nearly made —
during the live Reptile run this module was written for:

  * `down + button` asserted on the same frame produces a move that contacts
    NOTHING at any distance, which reads exactly like "crouching normals have
    no range". The stance lead-in is the fix, and `test_move_script_*` pins it.
  * A move that KNOCKS THE VICTIM DOWN has no on-hit advantage number (§1.1),
    and the probe will happily return one anyway — it measures when the
    defender can walk, and a defender who is getting up eventually can.
  * A calibration taken while the fighter is still stunned is not a latency,
    and it biases the advantage by the difference between the two sides'
    numbers. §3.1 asserts "far enough past the anchor" and offers no check;
    `test_calibration_must_be_hold_limited` is that check.
"""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from shadow_train.framelab import kit
from shadow_train.framelab.probe import ProbeError, SweepResult
from shadow_train.framelab.store import FrameStore

from .test_framelab_probe import FakeGame, contact_read, make_session, sampler

GAP = kit.Rung(arena="fake.state", gap_px=62, gap_walk_frames=60)
PUNCH = kit.MoveSpec(name="HP", buttons=(FakeGame.ATTACK,))
CROUCH_PUNCH = kit.MoveSpec(
    name="HP", buttons=(FakeGame.ATTACK,), stance="crouching",
    stance_button="down", stance_frames=6,
)
# `FakeGame` only knows "right"/"left"; the rig must not offer it a direction
# it has never heard of, so both ports get the same single candidate here.
DIRS = {0: ("right",), 1: ("right",)}


def fake_rig(**kw):
    base = dict(guard_buttons=(FakeGame.GUARD,))
    base.update(kw)
    rig = kit.make_rig("fake.state", **base)
    return type(rig)(
        arena=rig.arena, attacker_port=rig.attacker_port,
        defender_port=rig.defender_port, guard_buttons=rig.guard_buttons,
        walk_directions=("right",), walk_directions_by_port=DIRS,
        quiet_frames=rig.quiet_frames,
    )


def latencies(value=1):
    """`FakeGame(pipeline=0)` has `L_obs = 1` for both observables, on both
    ports and in both guard states — the toy has no block stance to drop."""
    return {shape: {"x": value, "struct": value}
            for shape in ("attacker/hit", "attacker/block",
                          "defender/hit", "defender/block")}


SAMPLE_FNS = {0: sampler(0), 1: sampler(1)}
OBSERVABLES = ("x", "struct")


class MoveScriptTest(unittest.TestCase):
    def test_standing_move_has_no_lead_in(self):
        script = kit.move_script(PUNCH)
        self.assertEqual(script.lead_in, ())
        self.assertEqual(script.attack_input_frame, 0)
        self.assertEqual(script.steps[0].buttons, (FakeGame.ATTACK,))

    def test_crouching_move_holds_the_stance_first_then_adds_the_button(self):
        script = kit.move_script(CROUCH_PUNCH)
        self.assertEqual(script.lead_in[0].buttons, ("down",))
        self.assertEqual(script.attack_input_frame, 6)
        self.assertEqual(script.steps[0].buttons, ("down", FakeGame.ATTACK))
        self.assertEqual(script.name, "cHP")

    def test_crouching_move_without_a_stance_button_is_refused(self):
        spec = kit.MoveSpec(name="HP", buttons=("a",), stance="crouching")
        with self.assertRaises(ValueError):
            kit.move_script(spec)


class RigTest(unittest.TestCase):
    def test_each_port_walks_away_from_the_opponent_first(self):
        """§4.2's blocked-direction hazard: at contact range a fighter cannot
        walk into the other fighter's body, and the probe would read that as
        'not actionable'. P1 stands on the left in every ladder arena."""
        rig = kit.make_rig("a.state", guard_buttons=("l",))
        self.assertEqual(rig.walk_directions_by_port[0], ("left", "right"))
        self.assertEqual(rig.walk_directions_by_port[1], ("right", "left"))


class ScanContactTest(unittest.TestCase):
    def test_a_whiff_is_a_result_not_an_exception(self):
        game = FakeGame(reach=10)          # too far to connect
        session = make_session(game)
        scan = kit.scan_contact(session, rig=fake_rig(), spec=PUNCH, gap_px=180,
                                contact_read=contact_read, defender_guard=False)
        self.assertFalse(scan.connected)
        self.assertIsNone(scan.damage)     # §2.5: absent, never 0
        self.assertIsNone(scan.contact_frame)

    def test_connect_records_damage_and_hits_from_the_anchor(self):
        session = make_session(FakeGame())
        scan = kit.scan_contact(session, rig=fake_rig(), spec=PUNCH, gap_px=62,
                                contact_read=contact_read, defender_guard=False)
        self.assertTrue(scan.connected)
        self.assertEqual(scan.damage, 11)
        self.assertEqual(scan.hits, 1)

    def test_blocked_and_clean_contact_are_separate_rigs(self):
        """§2.6: hit-vs-block is a property of the RIG. The blocked run must
        show the chip number, not an inference from the clean one."""
        session = make_session(FakeGame())
        blocked = kit.scan_contact(session, rig=fake_rig(), spec=PUNCH, gap_px=62,
                                   contact_read=contact_read, defender_guard=True)
        self.assertEqual(blocked.damage, 3)

    def test_knockdown_is_detected_from_the_victims_own_resting_y(self):
        game = FakeGame()
        session = make_session(game)
        scan = kit.scan_contact(
            session, rig=fake_rig(), spec=PUNCH, gap_px=62,
            contact_read=contact_read, defender_guard=False,
            victim_y_read=_launch_after_contact(game), knockdown_frames=40,
        )
        self.assertTrue(scan.knockdown)
        self.assertIsNotNone(scan.airborne_until)

    def test_a_grounded_hit_is_not_a_knockdown(self):
        game = FakeGame()
        session = make_session(game)
        scan = kit.scan_contact(
            session, rig=fake_rig(), spec=PUNCH, gap_px=62,
            contact_read=contact_read, defender_guard=False,
            victim_y_read=lambda s: 85, knockdown_frames=40,
        )
        self.assertFalse(scan.knockdown)
        self.assertIsNone(scan.airborne_until)


def _launch_after_contact(game):
    """A victim whose y leaves its resting value for 20 frames after the hit —
    what Reptile's crouching HP actually does (85 -> -101, back at f78)."""

    def read(session):
        hit = game.contacts[0] if game.contacts else None
        if hit is not None and hit < game.gframe <= hit + 20:
            return -101
        return 85

    return read


class SweepGuardTest(unittest.TestCase):
    def _sweep(self, predicate, monotone, first_true, max_search=45):
        return SweepResult(
            observable="x", method="linear_sweep", direction="right",
            first_true=first_true, predicate=tuple(predicate), monotone=monotone,
            window=3, input_latency_frames=1, max_search=max_search, port=1,
            rig_guard_state="none", runs=1,
        )

    def test_non_monotone_predicate_is_refused(self):
        """An exhaustive sweep is its own flake detector: a hold that lands one
        frame early shows up as an isolated TRUE below the boundary, which is
        exactly a non-monotone predicate."""
        pred = [False, True] + [False] * 10 + [True] * 33
        with self.assertRaises(kit.NonMonotoneError):
            kit._check_sweep(self._sweep(pred, False, 1), who="defender", cell="c")

    def test_boundary_against_the_edge_of_the_search_is_refused(self):
        with self.assertRaises(ProbeError) as ctx:
            kit._check_sweep(self._sweep([False] * 44 + [True], True, 44),
                             who="defender", cell="c")
        self.assertIn("no silent caps", str(ctx.exception))

    def test_a_never_actionable_sweep_is_left_for_the_caller_to_report(self):
        """§4.2: 'If neither diverges, record NULL rather than "never
        actionable"' — the guard must not turn that into an exception."""
        kit._check_sweep(self._sweep([], None, None), who="defender", cell="c")


class MeasureCellTest(unittest.TestCase):
    def _measure(self, game, **kw):
        session = make_session(game)
        return kit.measure_cell(
            session, rig=fake_rig(), spec=PUNCH, rung=GAP,
            contact_read=contact_read, sample_fns=SAMPLE_FNS,
            latencies=latencies(), observables=OBSERVABLES,
            max_search=kw.pop("max_search", 45), **kw,
        )

    def test_hit_and_block_are_measured_separately_and_agree_across_observables(self):
        game = FakeGame(stun=24, block_stun=14, recovery=18)
        m = self._measure(game)
        self.assertEqual({o: m.on_hit[o].advantage for o in OBSERVABLES},
                         {"x": 6, "struct": 6})     # 24 - 18
        self.assertEqual({o: m.on_block[o].advantage for o in OBSERVABLES},
                         {"x": -4, "struct": -4})   # 14 - 18
        self.assertGreater(m.on_hit["x"].advantage, m.on_block["x"].advantage)

    def test_a_whiff_produces_no_advantage_and_says_so(self):
        m = self._measure(FakeGame(reach=10))
        self.assertEqual(m.on_hit, {})
        self.assertEqual(m.on_block, {})
        self.assertTrue(any("§1.1" in n for n in m.notes))

    def test_a_knockdown_drops_on_hit_but_keeps_on_block(self):
        game = FakeGame()
        m = self._measure(game, victim_y_read=_launch_after_contact(game))
        self.assertEqual(m.on_hit, {}, "a knockdown has no hit-advantage number")
        self.assertTrue(m.on_block)
        self.assertTrue(any("knockdown" in n for n in m.notes))

    def test_observables_that_disagree_refuse_the_row(self):
        """§8.4 makes cross-method agreement REQUIRED, and §7 forbids splitting
        the difference. `coarse_x` here is the classic way to break it: an
        observable with a bigger manifestation margin, measured through a
        window sized from the OTHER observable's latency."""
        game = FakeGame()
        session = make_session(game)
        def coarse(port):
            base = sampler(port)
            def read(s):
                out = dict(base(s))
                out["coarse_x"] = out["x"] // 16
                return out
            return read
        with self.assertRaises(kit.CrossMethodError):
            kit.measure_cell(
                session, rig=fake_rig(), spec=PUNCH, rung=GAP,
                contact_read=contact_read,
                sample_fns={0: coarse(0), 1: coarse(1)},
                latencies={k: {"x": 1, "coarse_x": 1} for k in latencies()},
                observables=("x", "coarse_x"), max_search=45,
            )


class CalibrationTest(unittest.TestCase):
    def test_calibration_must_be_hold_limited(self):
        """A latency measured while the fighter is still stunned SHRINKS as the
        probe moves later. Measured live: far HK's defender calibrates to 6/7 at
        anchor+40 and 1/2 at anchor+70 and +100."""
        game = FakeGame(stun=30)
        session = make_session(game)
        scan = kit.scan_contact(session, rig=fake_rig(), spec=PUNCH, gap_px=62,
                                contact_read=contact_read, defender_guard=False)
        with self.assertRaises(ProbeError) as ctx:
            kit.calibrate_shapes(
                session, rig=fake_rig(), spec=PUNCH,
                anchor=scan.contact_frame, sample_fns=SAMPLE_FNS,
                observables=OBSERVABLES, at_n=10, confirm_at_n=60,
                trials=2, max_window=30,
            )
        self.assertIn("hold-limited", str(ctx.exception))

    def test_a_hold_limited_calibration_agrees_at_both_points(self):
        session = make_session(FakeGame(stun=30))
        scan = kit.scan_contact(session, rig=fake_rig(), spec=PUNCH, gap_px=62,
                                contact_read=contact_read, defender_guard=False)
        got = kit.calibrate_shapes(
            session, rig=fake_rig(), spec=PUNCH, anchor=scan.contact_frame,
            sample_fns=SAMPLE_FNS, observables=OBSERVABLES,
            at_n=45, confirm_at_n=60, trials=2, max_window=20,
        )
        self.assertEqual(got["defender/hit"], {"x": 1, "struct": 1})
        self.assertEqual(sorted(got), ["attacker/block", "attacker/hit",
                                       "defender/block", "defender/hit"])


class ManifestAdvantageTest(unittest.TestCase):
    """`kit.manifest_advantage`: the stored advantage is a difference of
    MANIFEST frames, so that two probe shapes with different latencies (the
    guarded defender's stance drop) do not bias it."""

    def _sweep(self, first_true, window):
        return SweepResult(
            observable="x", method="linear_sweep", direction="right",
            first_true=first_true, predicate=(), monotone=True, window=window,
            input_latency_frames=window - 2, max_search=45, port=1,
            rig_guard_state="none", runs=1,
        )

    def test_same_shape_agrees_with_the_first_true_difference(self):
        att, dfn = self._sweep(15, 3), self._sweep(23, 3)
        self.assertEqual(kit.manifest_advantage(att, dfn), 8)   # == 23 - 15

    def test_different_shapes_do_not_cancel_and_the_manifest_form_is_used(self):
        """attacker window 3, guarded-defender window 12: `first_true`
        differencing loses the 9-frame stance drop, manifest differencing
        keeps it."""
        att, dfn = self._sweep(15, 3), self._sweep(7, 12)
        self.assertEqual(dfn.first_true - att.first_true, -8)   # the biased form
        self.assertEqual(kit.manifest_advantage(att, dfn), 1)   # 19 - 18

    def test_null_when_either_side_never_became_actionable(self):
        self.assertIsNone(kit.manifest_advantage(self._sweep(None, 3),
                                                 self._sweep(7, 12)))


class CellRowsTest(unittest.TestCase):
    def _rows(self, m, **kw):
        return kit.cell_rows(m, family="mk2", port="arcade", char="reptile",
                             core_id="core:sha256:dead", rom_id="rom:sha256:beef",
                             observables=OBSERVABLES, **kw)

    def test_one_row_per_observable_with_full_provenance(self):
        game = FakeGame()
        session = make_session(game)
        m = kit.measure_cell(session, rig=fake_rig(), spec=PUNCH, rung=GAP,
                             contact_read=contact_read, sample_fns=SAMPLE_FNS,
                             latencies=latencies(), observables=OBSERVABLES,
                             max_search=45, variant="close")
        m.connect_range = 72
        m.first_active_frame = 8
        rows = self._rows(m)
        self.assertEqual([r["observable"] for r in rows], list(OBSERVABLES))
        for row in rows:
            self.assertEqual(row["variant"], "close")
            self.assertEqual(row["gap_px"], 62)
            self.assertEqual(row["gap_walk_frames"], 60)
            self.assertEqual(row["damage"], 11)
            self.assertEqual(row["connect_range"], 72)
            self.assertEqual(row["first_active_frame"], 8)
            self.assertEqual(row["rig_guard_state"], "held+none")
            self.assertEqual(row["input_latency_frames"], 1)
            self.assertTrue(row["core_id"] and row["rom_id"] and row["method"])

    def test_a_knockdown_row_keeps_on_block_and_NULLs_on_hit(self):
        game = FakeGame()
        session = make_session(game)
        m = kit.measure_cell(session, rig=fake_rig(), spec=PUNCH, rung=GAP,
                             contact_read=contact_read, sample_fns=SAMPLE_FNS,
                             latencies=latencies(), observables=OBSERVABLES,
                             max_search=45, victim_y_read=_launch_after_contact(game))
        rows = self._rows(m)
        with tempfile.TemporaryDirectory() as tmp:
            with FrameStore(Path(tmp) / "frames.db") as store:
                ids = [store.insert(r) for r in rows]
                stored = [store.get(i) for i in ids]
        for row in stored:
            self.assertIsNone(row["on_hit"], "§2.5: unmeasured is NULL, never 0")
            self.assertIsNotNone(row["on_block"])
            self.assertEqual(row["knockdown"], 1)
            self.assertEqual(row["rig_guard_state"], "held")


if __name__ == "__main__":
    unittest.main()
