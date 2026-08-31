"""Unit tests for `framelab.specials` — docs/frames.md applied to SPECIAL
moves, and the three assumptions Mileena's kit breaks (charge/projectile,
airborne, side-swap + knockdown).

Everything here runs against fakes: `special_script` and the facing /
signature / manifest logic are pure, and the two functions that drive the
emulator (`observe_move`, `preemption_scan`, `sweep_side`) are exercised by
substituting the one collaborator they call (`replay` / `sweep_actionable`)
with a stub. That keeps the refusal rules — which are the point of this
module — under test without a live rig.
"""

from __future__ import annotations

import unittest
from types import SimpleNamespace

from shadow_train.framelab import specials as sp
from shadow_train.framelab.probe import ProbeError, Rig, ScriptStep, SweepResult


def fake_profile(**overrides):
    """A profile-shaped object with MK2 arcade's chords and Mileena's three
    encodings. `special_inputs` (the COMPILED view) deliberately carries the
    loader's lossy form — no `hold`/`min_frames` — so a test can prove
    `special_encoding` reads `port_raw` instead."""
    port_raw = {
        "special_inputs": {
            "mileena": {
                "sai_throw": [
                    {"hold": ["HP"], "min_frames": 34},
                    {"release": ["HP"]},
                ],
                "teleport_kick": [
                    {"dirs": ["forward"], "frames": 3},
                    {"dirs": ["forward"], "frames": 3},
                    {"press": ["LK"], "frames": 3},
                ],
                "roll": [
                    {"dirs": ["back"], "frames": 3},
                    {"dirs": ["back"], "frames": 3},
                    {"dirs": ["down"], "frames": 3},
                    {"press": ["HK"], "frames": 3},
                ],
            }
        }
    }
    port_raw.update(overrides.pop("port_raw", {}))
    return SimpleNamespace(
        attack_chords={"HP": ["y"], "LP": ["b"], "HK": ["x"], "LK": ["a"],
                       "Block": ["l"]},
        # what `profile.load` actually produces: the §10.1 kinds are gone.
        special_inputs={"mileena": {
            "sai_throw": [{"dirs": [], "press": [], "frames": 3},
                          {"dirs": [], "press": [], "frames": 3}]}},
        port_raw=port_raw,
        family="mk2", port="arcade",
        **overrides,
    )


def sweep(first_true, *, observable="struct_velocity", window=3, max_search=45,
          predicate=None, monotone=True):
    if predicate is None:
        predicate = tuple(i >= first_true for i in range(max_search + 1)) \
            if first_true is not None else tuple([False] * (max_search + 1))
    return SweepResult(
        observable=observable, method="linear_sweep", direction="left",
        first_true=first_true, predicate=tuple(predicate), monotone=monotone,
        window=window, input_latency_frames=window - 2, max_search=max_search,
        port=0, rig_guard_state="none", runs=1,
    )


class ScriptFromEncoding(unittest.TestCase):
    def test_charge_step_becomes_one_long_hold_and_the_release_frame(self):
        s = sp.special_script(fake_profile(), "mileena", "sai_throw", facing="right")
        self.assertEqual([(st.frames, st.buttons) for st in s.steps],
                         [(34, ("y",))])
        # The release is the schedule's own trailing release, so the frame the
        # attacker's clock starts at is exactly `total_frames`.
        self.assertEqual(s.total_frames, 34)

    def test_the_compiled_profile_view_would_have_lost_the_charge(self):
        """Regression: `profile.special_inputs` drops `hold`/`min_frames`, so
        a charge read through it holds nothing at all."""
        prof = fake_profile()
        compiled = prof.special_inputs["mileena"]["sai_throw"]
        self.assertEqual([st["press"] for st in compiled], [[], []])
        s = sp.special_script(prof, "mileena", "sai_throw", facing="right")
        self.assertEqual(s.steps[0].buttons, ("y",))

    def test_motion_steps_are_separated_by_the_measured_two_frame_gap(self):
        s = sp.special_script(fake_profile(), "mileena", "teleport_kick",
                              facing="right")
        self.assertEqual([(st.frames, st.buttons) for st in s.steps],
                         [(3, ("right",)), (2, ()), (3, ("right",)), (2, ()),
                          (3, ("a",))])

    def test_gap_of_one_is_never_produced_by_default(self):
        self.assertEqual(sp.STEP_GAP, 2)

    def test_semantic_directions_resolve_against_the_pinned_facing(self):
        left = sp.special_script(fake_profile(), "mileena", "roll", facing="left")
        right = sp.special_script(fake_profile(), "mileena", "roll", facing="right")
        self.assertEqual(left.steps[0].buttons, ("right",))   # back, facing left
        self.assertEqual(right.steps[0].buttons, ("left",))   # back, facing right
        # `down` is absolute in both.
        self.assertEqual(left.steps[4].buttons, ("down",))
        self.assertEqual(right.steps[4].buttons, ("down",))

    def test_lead_in_is_kept_out_of_the_move_and_shifts_the_release(self):
        s = sp.special_script(fake_profile(), "mileena", "sai_throw",
                              facing="right",
                              lead_in=(ScriptStep(frames=20, buttons=("right",)),
                                       ScriptStep(frames=3, buttons=())))
        self.assertEqual(s.attack_input_frame, 23)
        self.assertEqual(s.total_frames, 57)

    def test_an_unknown_step_kind_raises_instead_of_being_dropped(self):
        prof = fake_profile()
        prof.port_raw["special_inputs"]["mileena"]["roll"][0]["charge"] = ["HP"]
        with self.assertRaises(sp.SpecialEncodingError):
            sp.special_script(prof, "mileena", "roll", facing="right")

    def test_a_non_terminal_release_is_refused_rather_than_guessed(self):
        prof = fake_profile()
        prof.port_raw["special_inputs"]["mileena"]["sai_throw"] = [
            {"hold": ["HP"], "min_frames": 34},
            {"release": ["HP"]},
            {"press": ["HP"], "frames": 3},
        ]
        with self.assertRaises(sp.SpecialEncodingError):
            sp.special_script(prof, "mileena", "sai_throw", facing="right")

    def test_an_unknown_attack_class_raises(self):
        prof = fake_profile()
        prof.port_raw["special_inputs"]["mileena"]["roll"][3]["press"] = ["SUPER"]
        with self.assertRaises(sp.SpecialEncodingError):
            sp.special_script(prof, "mileena", "roll", facing="right")

    def test_a_move_this_port_does_not_encode_is_a_refusal_not_a_default(self):
        with self.assertRaises(sp.SpecialEncodingError):
            sp.special_script(fake_profile(), "mileena", "fatality", facing="right")

    def test_an_unresolvable_facing_is_refused_never_guessed(self):
        with self.assertRaises(sp.SpecialEncodingError):
            sp.special_script(fake_profile(), "mileena", "roll", facing=None)


class Facing(unittest.TestCase):
    def test_derived_from_relative_position(self):
        self.assertEqual(sp.facing_from_x(927, 1119), "right")
        self.assertEqual(sp.facing_from_x(1192, 1002), "left")

    def test_null_never_a_guess(self):
        self.assertIsNone(sp.facing_from_x(None, 1119))
        self.assertIsNone(sp.facing_from_x(927, None))
        self.assertIsNone(sp.facing_from_x(1000, 1000))   # exact overlap

    def test_walk_directions_are_away_from_the_opponent_first(self):
        self.assertEqual(sp.walk_directions_after(927, 1119), ("left", "right"))

    def test_walk_directions_flip_after_a_side_swap(self):
        """The roll ends with Mileena at 1192 and the victim at 1002 — the
        pre-move order would send her walking into his body."""
        self.assertEqual(sp.walk_directions_after(1192, 1002), ("right", "left"))
        self.assertEqual(sp.walk_directions_after(1002, 1192), ("left", "right"))

    def test_unknown_facing_still_tries_both_directions(self):
        self.assertEqual(set(sp.walk_directions_after(None, 5)), {"left", "right"})


class Observation(unittest.TestCase):
    """`observe_move`'s derivations, driven by a canned trace."""

    def _observe(self, trace, monkeypatched_frames=None):
        script = sp.special_script(fake_profile(), "mileena", "roll", facing="right")
        rig = Rig(arena="a.state", attacker_port=0, defender_port=1,
                  guard_buttons=("l",))
        orig = sp.replay
        sp.replay = lambda *a, **k: trace
        try:
            return sp.observe_move(
                None, rig=rig, script=script, total_frames=len(trace) - 1,
                defender_guard=False,
                contact_read=lambda s: None, attacker_x_read=lambda s: None,
                attacker_y_read=lambda s: None, victim_x_read=lambda s: None,
                victim_y_read=lambda s: None,
            )
        finally:
            sp.replay = orig

    @staticmethod
    def _frame(c, ax, ay, vx, vy):
        return {"c": c, "ax": ax, "ay": ay, "vx": vx, "vy": vy}

    def test_damage_contacts_gap_and_the_side_swap(self):
        trace = ([self._frame(161, 915, 87, 1119, 89)] * 34
                 + [self._frame(140, 1045, 143, 1119, 89)]
                 + [self._frame(140, 1192, 87, 1002, 89)] * 20)
        obs = self._observe(trace)
        self.assertEqual(obs.contacts, (34,))
        self.assertEqual(obs.damage, 21)
        self.assertEqual(obs.gap_px, 204)
        self.assertTrue(obs.crossed)
        self.assertEqual((obs.facing_before, obs.facing_after), ("right", "left"))

    def test_knockdown_is_derived_from_the_victims_own_resting_y(self):
        """docs/frames.md §10: there is no scalar GROUND_Y on arcade."""
        trace = ([self._frame(161, 915, 87, 1119, 89)] * 34
                 + [self._frame(140, 1045, 143, 1119, -6)] * 10
                 + [self._frame(140, 1192, 87, 1002, 89)] * 10)
        obs = self._observe(trace)
        self.assertTrue(obs.knockdown)
        self.assertEqual(obs.victim_airborne_until, 44)

    def test_a_victim_that_never_leaves_its_resting_y_is_not_a_knockdown(self):
        trace = ([self._frame(161, 927, 87, 1119, 89)] * 20
                 + [self._frame(129, 1043, 21, 1119, 89)] * 20)
        obs = self._observe(trace)
        self.assertFalse(obs.knockdown)
        self.assertEqual(obs.victim_airborne_until, 0)
        # ... but the ATTACKER left hers (the teleport), which is what makes
        # her act-again probe a different question.
        self.assertEqual(obs.attacker_airborne_until, 40)

    def test_a_whiff_is_a_result_not_a_crash(self):
        obs = self._observe([self._frame(161, 927, 87, 1119, 89)] * 30)
        self.assertFalse(obs.connected)
        self.assertIsNone(obs.damage)


class SignatureChecks(unittest.TestCase):
    def _obs(self, **kw):
        base = dict(move="roll", contacts=(33,), contact_values=(140,), damage=21,
                    attacker_x=(915, 1192), victim_x=(1119, 1002), gap_px=204,
                    attacker_airborne_until=49, victim_airborne_until=73,
                    crossed=True, facing_before="right", facing_after="left")
        base.update(kw)
        return sp.MoveObservation(**base)

    def test_a_matching_move_reports_nothing(self):
        self.assertEqual(
            sp.check_signature(self._obs(), sp.Signature(
                damage=21, hits=1, crossed=True, victim_knockdown=True,
                min_attacker_travel_px=200)),
            (),
        )

    def test_the_crouching_normal_a_failed_roll_degenerates_into_is_caught(self):
        """`block+0xC0` fires identically for both; damage and travel do not."""
        bad = self._obs(damage=0, contacts=(), attacker_x=(915, 915),
                        crossed=False, victim_airborne_until=0)
        problems = sp.check_signature(bad, sp.Signature(
            damage=21, hits=1, crossed=True, victim_knockdown=True,
            min_attacker_travel_px=200))
        self.assertEqual(len(problems), 5)

    def test_unreadable_positions_are_a_problem_not_a_pass(self):
        bad = self._obs(attacker_x=(None, None))
        self.assertTrue(sp.check_signature(
            bad, sp.Signature(min_attacker_travel_px=200)))


class Manifests(unittest.TestCase):
    def test_manifest_is_absolute_and_carries_its_origin(self):
        sm = sp.SideManifest(who="attacker", origin=35, origin_kind="release+1",
                             sweep=sweep(47, window=3))
        self.assertEqual(sm.manifest, 85)

    def test_never_actionable_is_null_not_zero(self):
        sm = sp.SideManifest(who="attacker", origin=35, origin_kind="release+1",
                             sweep=sweep(None))
        self.assertIsNone(sm.manifest)

    def test_two_different_origins_still_difference_correctly(self):
        """The projectile case: the attacker's clock starts at the release and
        the defender's at contact, 24 frames later."""
        att = sp.SideManifest(who="attacker", origin=35, origin_kind="release+1",
                              sweep=sweep(47, window=3))
        dfn = sp.SideManifest(who="defender", origin=58, origin_kind="contact",
                              sweep=sweep(23, window=3))
        self.assertEqual(sp.advantage_between(att, dfn), -1)

    def test_different_probe_shapes_are_allowed_because_latency_cancels(self):
        """kit.manifest_advantage's rule: a bigger window is exactly offset by
        a smaller first_true, so the guarded defender differences correctly."""
        att = sp.SideManifest(who="attacker", origin=33, origin_kind="contact",
                              sweep=sweep(50, window=3))
        dfn = sp.SideManifest(who="defender", origin=33, origin_kind="contact",
                              sweep=sweep(7, window=12))
        self.assertEqual(sp.advantage_between(att, dfn), -34)

    def test_differencing_two_different_observables_is_refused(self):
        att = sp.SideManifest(who="attacker", origin=0, origin_kind="contact",
                              sweep=sweep(10, observable="struct_velocity"))
        dfn = sp.SideManifest(who="defender", origin=0, origin_kind="contact",
                              sweep=sweep(10, observable="pointer_x", window=4))
        with self.assertRaises(ProbeError):
            sp.advantage_between(att, dfn)

    def test_a_null_side_makes_the_advantage_null(self):
        att = sp.SideManifest(who="attacker", origin=0, origin_kind="contact",
                              sweep=sweep(None))
        dfn = sp.SideManifest(who="defender", origin=0, origin_kind="contact",
                              sweep=sweep(10))
        self.assertIsNone(sp.advantage_between(att, dfn))


class SweepRefusals(unittest.TestCase):
    """`sweep_side`'s three gates, with `sweep_actionable` stubbed."""

    def _run(self, result, **kw):
        rig = Rig(arena="a.state", attacker_port=0, defender_port=1,
                  guard_buttons=("l",))
        script = sp.special_script(fake_profile(), "mileena", "roll", facing="right")
        orig = sp.sweep_actionable
        sp.sweep_actionable = lambda *a, **k: {"struct_velocity": result}
        try:
            return sp.sweep_side(
                None, rig=rig, script=script, who="attacker", port=0, origin=33,
                origin_kind="contact", observables=["struct_velocity"],
                sample_fn=None, input_latency_frames={"struct_velocity": 1},
                defender_guard=False, max_search=45, **kw)
        finally:
            sp.sweep_actionable = orig

    def test_a_monotone_boundary_is_returned(self):
        out = self._run(sweep(20))
        self.assertEqual(out["struct_velocity"].manifest, 56)

    def test_a_non_monotone_predicate_is_refused(self):
        pred = [False] * 46
        pred[3] = True
        pred[30:] = [True] * 16
        with self.assertRaises(ProbeError):
            self._run(sweep(3, predicate=pred, monotone=False))

    def test_a_boundary_against_the_edge_of_the_search_is_refused(self):
        with self.assertRaises(ProbeError):
            self._run(sweep(44))

    def test_a_boundary_where_the_probe_cancelled_the_move_is_refused(self):
        """The sai's N=0: the sweep says TRUE, but there was no sai in that
        run at all."""
        with self.assertRaises(sp.PreemptedProbeError):
            self._run(sweep(0), excluded_n=(0,))

    def test_never_actionable_is_null_rather_than_an_error(self):
        out = self._run(sweep(None))
        self.assertIsNone(out["struct_velocity"].manifest)


class PreemptionScan(unittest.TestCase):
    def test_it_reports_which_probe_frames_killed_the_move(self):
        rig = Rig(arena="a.state", attacker_port=0, defender_port=1,
                  guard_buttons=("l",))
        script = sp.special_script(fake_profile(), "mileena", "sai_throw",
                                   facing="right")

        def fake_replay(session, **kw):
            fired = kw["probe_at"] > 34        # N=0 at origin 34 kills it
            vals = [161] * 40 + ([138] * 10 if fired else [161] * 10)
            return [{"c": v} for v in vals]

        orig = sp.replay
        sp.replay = fake_replay
        try:
            got = sp.preemption_scan(
                None, rig=rig, script=script, origin=34, n_range=range(0, 3),
                directions=("left", "right"), contact_read=lambda s: None,
                tail_frames=20)
        finally:
            sp.replay = orig
        self.assertEqual(got, {0: False, 1: True, 2: True})


class Rows(unittest.TestCase):
    def _row(self, **kw):
        base = dict(family="mk2", port="arcade", char="mileena", move="roll",
                    core_id="c", rom_id="r", observable="struct_velocity",
                    method="linear_sweep", input_latency_frames=1)
        base.update(kw)
        return sp.special_row(**base)

    def _obs(self, victim_airborne_until):
        return sp.MoveObservation(
            move="roll", contacts=(33,), contact_values=(140,), damage=21,
            attacker_x=(915, 1192), victim_x=(1119, 1002), gap_px=204,
            attacker_airborne_until=49,
            victim_airborne_until=victim_airborne_until, crossed=True,
            facing_before="right", facing_after="left")

    def test_a_knockdown_forces_on_hit_to_null_whatever_the_caller_passed(self):
        row = self._row(obs_hit=self._obs(73), on_hit=42, wakeup_window=77)
        self.assertIsNone(row["on_hit"])
        self.assertEqual(row["knockdown"], 1)
        self.assertEqual(row["wakeup_window"], 77)

    def test_a_grounded_hit_keeps_its_advantage(self):
        row = self._row(move="teleport_kick", obs_hit=self._obs(0), on_hit=-5)
        self.assertEqual(row["on_hit"], -5)
        self.assertEqual(row["knockdown"], 0)

    def test_absent_is_null_never_zero(self):
        row = self._row(obs_hit=self._obs(0))
        for col in ("on_hit", "on_block", "wakeup_window", "gap_px",
                    "gap_walk_frames"):
            self.assertIsNone(row[col], col)

    def test_the_row_carries_its_provenance(self):
        row = self._row(obs_hit=self._obs(0), on_hit=-5)
        for col in ("observable", "method", "input_latency_frames", "core_id",
                    "rom_id"):
            self.assertIsNotNone(row[col], col)

    def test_the_row_is_storable(self):
        from shadow_train.framelab.store import MOVE_FRAMES_COLUMNS
        row = self._row(obs_hit=self._obs(73), wakeup_window=77)
        self.assertFalse(set(row) - set(MOVE_FRAMES_COLUMNS))


class Collapsing(unittest.TestCase):
    def _m(self):
        return sp.SpecialMeasurement(move="roll", arena="a.state")

    def test_agreeing_observables_collapse_to_one_number(self):
        m = self._m()
        self.assertEqual(m.agreed({"struct_velocity": -5, "pointer_x": -5}), -5)

    def test_disagreeing_observables_are_refused_not_averaged(self):
        m = self._m()
        with self.assertRaises(sp.CrossObservableError):
            m.agreed({"struct_velocity": -5, "pointer_x": -4})

    def test_all_null_stays_null(self):
        m = self._m()
        self.assertIsNone(m.agreed({"struct_velocity": None, "pointer_x": None}))


class Conventions(unittest.TestCase):
    def test_the_wakeup_window_convention_is_stated_not_implied(self):
        self.assertIn("NOT an advantage", sp.WAKEUP_WINDOW_CONVENTION)

    def test_charge_persistence_records_the_reload_caveat(self):
        cp = sp.ChargePersistence(
            banked_frames=20, fresh_threshold_frames=34, extra_frames_needed=14,
            fires_with_no_further_input=False,
            note="a pre-charged arena is only sound if the reload runs ZERO "
                 "frames with the chord released")
        self.assertTrue(cp.survives)
        self.assertEqual(cp.banked_total, 34)
        self.assertIn("ZERO frames", cp.note)

    def test_a_charge_that_needs_the_full_hold_again_did_NOT_survive(self):
        cp = sp.ChargePersistence(banked_frames=20, fresh_threshold_frames=34,
                                  extra_frames_needed=34,
                                  fires_with_no_further_input=False)
        self.assertFalse(cp.survives)

    def test_a_charge_that_never_fired_is_null_not_false(self):
        cp = sp.ChargePersistence(banked_frames=20, fresh_threshold_frames=34,
                                  extra_frames_needed=None,
                                  fires_with_no_further_input=False)
        self.assertIsNone(cp.survives)
        self.assertIsNone(cp.banked_total)


if __name__ == "__main__":
    unittest.main()
