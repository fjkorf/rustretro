"""Unit tests for `framelab.specials` — docs/frames.md applied to SPECIAL
moves, and the five assumptions two kits break: Mileena's charge/projectile,
airborne and side-swap+knockdown, plus Reptile's damageless move (no anchor
at all) and his low (a guard-stance question a normal never asks).

Everything here runs against fakes: `special_script` and the facing /
signature / manifest / verdict logic are pure, and every function that drives
the emulator (`observe_move`, `preemption_scan`, `sweep_side`,
`origin_invariance`, `measure_guard_height`, `screen_preemption_scan`) is
exercised by substituting the one collaborator it calls (`replay` /
`sweep_actionable`) with a stub. That keeps the refusal rules — which are the
point of this module — under test without a live rig.
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
    # Reptile's two shapes that Mileena has none of: a NON-TERMINAL release
    # (`invisibility`) and a projectile that is not a charge (`acid_spit`).
    port_raw["special_inputs"]["reptile"] = {
        "acid_spit": [
            {"dirs": ["forward"], "frames": 3},
            {"dirs": ["forward"], "frames": 3},
            {"press": ["HP"], "frames": 3},
        ],
        "slide": [
            {"dirs": ["back"], "press": ["LK", "LP", "Block"], "frames": 8},
        ],
        "force_ball": [
            {"dirs": ["back"], "frames": 3},
            {"dirs": ["back"], "press": ["HP", "LP"], "frames": 3},
        ],
        "invisibility": [
            {"dirs": ["up"], "while_held": ["Block"], "frames": 3},
            {"dirs": ["up"], "while_held": ["Block"], "frames": 3},
            {"dirs": ["down"], "while_held": ["Block"], "frames": 3},
            {"release": ["Block"]},
            {"press": ["HP"], "frames": 3},
        ],
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

    def test_a_release_followed_by_a_re_hold_of_the_same_class_is_refused(self):
        """A falling edge this playback cannot express: every step's mask is
        the port's WHOLE held set, so the release and the re-hold would
        collapse into one continuous hold."""
        prof = fake_profile()
        prof.port_raw["special_inputs"]["mileena"]["sai_throw"] = [
            {"hold": ["HP"], "min_frames": 34},
            {"release": ["HP"]},
            {"press": ["HP"], "frames": 3},
        ]
        with self.assertRaises(sp.SpecialEncodingError):
            sp.special_script(prof, "mileena", "sai_throw", facing="right")

    def test_a_non_terminal_release_of_a_different_class_is_the_neutral_gap(self):
        """Reptile's `invisibility`: `[BLK] U U D`, release BLK, then HP. The
        `step_gap` neutral frames between steps ARE the released window, and
        the HP step already holds only HP because a step's mask is the port's
        entire held set."""
        s = sp.special_script(fake_profile(), "reptile", "invisibility",
                              facing="right")
        self.assertEqual([(st.frames, st.buttons) for st in s.steps],
                         [(3, ("up", "l")), (2, ()),
                          (3, ("up", "l")), (2, ()),
                          (3, ("down", "l")), (2, ()),
                          (3, ("y",))])
        self.assertEqual(s.total_frames, 18)
        # The frames where Block is NOT held are exactly the neutral gaps --
        # nothing silently carries the chord into the trigger step.
        self.assertNotIn("l", s.steps[-1].buttons)

    def test_a_non_terminal_release_needs_a_gap_to_be_released_on(self):
        with self.assertRaises(sp.SpecialEncodingError):
            sp.special_script(fake_profile(), "reptile", "invisibility",
                              facing="right", step_gap=0)

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


class DifferentialCollisionRetry(unittest.TestCase):
    """Reptile's `force_ball`: the anti-overlap push moves the attacker at
    almost exactly walking speed, so in ONE direction the probe and the
    control produce identical `obj+0x12` over the whole window and the
    predicate acquires a FALSE island 40 frames past the boundary. In the
    other direction (walk and push have opposite signs) both observables are
    monotone and return the SAME boundary."""

    def _run(self, by_direction, dirs=("left", "right"), **kw):
        rig = Rig(arena="a.state", attacker_port=0, defender_port=1,
                  guard_buttons=("l",),
                  walk_directions_by_port={0: dirs})
        script = sp.special_script(fake_profile(), "reptile", "force_ball",
                                   facing="right")
        seen = []

        def fake_sweep(session, **k):
            d = k["rig"].walk_directions_by_port[0]
            seen.append((tuple(d), tuple(k["observables"])))
            # `sweep_actionable` picks the first candidate that diverges.
            chosen = next((c for c in d if c in by_direction), d[0])
            return {o: by_direction[chosen][o] for o in k["observables"]}

        orig = sp.sweep_actionable
        sp.sweep_actionable = fake_sweep
        try:
            out = sp.sweep_side(
                None, rig=rig, script=script, who="attacker", port=0,
                origin=19, origin_kind="input_end+1",
                observables=["struct_velocity", "pointer_x"], sample_fn=None,
                input_latency_frames={"struct_velocity": 1, "pointer_x": 2},
                defender_guard=False, max_search=90, **kw)
        finally:
            sp.sweep_actionable = orig
        return out, seen

    @staticmethod
    def _island(first_true, lo, hi, max_search=90):
        pred = [i >= first_true for i in range(max_search + 1)]
        for i in range(lo, hi + 1):
            pred[i] = False
        return sweep(first_true, predicate=pred, monotone=False,
                     max_search=max_search)

    def test_only_the_broken_observable_is_re_swept_and_it_agrees(self):
        left = {"struct_velocity": sweep(44, max_search=90),
                "pointer_x": self._island(44, 83, 87)}
        right = {"struct_velocity": sweep(44, max_search=90),
                 "pointer_x": sweep(44, window=4, max_search=90)}
        out, seen = self._run({"left": left, "right": right})
        self.assertEqual(out["pointer_x"].sweep.first_true, 44)
        self.assertEqual(out["pointer_x"].rejected_directions, ("left",))
        # The clean observable was NOT re-swept, and the retry asked only for
        # the broken one.
        self.assertEqual(out["struct_velocity"].rejected_directions, ())
        self.assertEqual(seen[1][1], ("pointer_x",))

    def test_a_direction_that_is_also_broken_still_refuses(self):
        broken = {"struct_velocity": sweep(44, max_search=90),
                  "pointer_x": self._island(44, 83, 87)}
        with self.assertRaises(ProbeError):
            self._run({"left": broken, "right": broken})

    def test_the_retry_can_be_turned_off(self):
        left = {"struct_velocity": sweep(44, max_search=90),
                "pointer_x": self._island(44, 83, 87)}
        right = {"struct_velocity": sweep(44, max_search=90),
                 "pointer_x": sweep(44, window=4, max_search=90)}
        with self.assertRaises(ProbeError):
            self._run({"left": left, "right": right},
                      retry_other_direction=False)


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


class AttackerOrigin(unittest.TestCase):
    def _script(self):
        return sp.special_script(fake_profile(), "reptile", "acid_spit",
                                 facing="right")

    def test_contact_origin_is_the_anchor(self):
        self.assertEqual(sp.attacker_origin(self._script(), "contact", 51), 51)

    def test_a_projectile_starts_its_clock_one_frame_after_its_own_input(self):
        s = self._script()
        self.assertEqual(s.total_frames, 13)
        self.assertEqual(sp.attacker_origin(s, "input_end+1", 51), 14)

    def test_a_charge_uses_the_same_arithmetic_under_a_different_name(self):
        s = self._script()
        self.assertEqual(sp.attacker_origin(s, "release+1", 51),
                         sp.attacker_origin(s, "input_end+1", 51))

    def test_an_unknown_origin_kind_is_refused_not_defaulted(self):
        with self.assertRaises(ProbeError):
            sp.attacker_origin(self._script(), "whenever", 51)


class OriginInvariance(unittest.TestCase):
    """The check that validates a whiff anchor against the contact anchor."""

    def _run(self, per_origin, **kw):
        rig = Rig(arena="a.state", attacker_port=0, defender_port=1,
                  guard_buttons=("l",))
        script = sp.special_script(fake_profile(), "reptile", "acid_spit",
                                   facing="right")
        calls = []

        def fake_sweep(session, **k):
            calls.append(k["anchor"])
            ft = per_origin[k["anchor"]]
            return {"struct_velocity": sweep(ft, window=3,
                                             max_search=k["max_search"])}

        orig = sp.sweep_actionable
        sp.sweep_actionable = fake_sweep
        try:
            return sp.origin_invariance(
                None, rig=rig, script=script, who="attacker", port=0,
                origins=[(14, "input_end+1"), (51, "contact")], end_frame=141,
                observables=["struct_velocity"], sample_fn=None,
                input_latency_frames={"struct_velocity": 1},
                defender_guard=False, **kw)
        finally:
            sp.sweep_actionable = orig

    def test_two_origins_that_land_on_the_same_absolute_frame_agree(self):
        # 14 + 46 + 3 == 51 + 9 + 3 == 63
        got = self._run({14: 46, 51: 9})
        self.assertEqual(got["struct_velocity"],
                         {"input_end+1": 63, "contact": 63})

    def test_a_manifest_that_depends_on_the_origin_is_refused(self):
        with self.assertRaises(sp.OriginDependenceError):
            self._run({14: 46, 51: 12})

    def test_an_origin_clipped_from_below_cannot_participate(self):
        """first_true=0 cannot tell 'free exactly here' from 'free earlier' —
        a sweep starts at N=0 and has no negative side."""
        with self.assertRaises(ProbeError):
            self._run({14: 46, 51: 0})

    def test_a_side_that_never_diverged_cannot_participate(self):
        with self.assertRaises(ProbeError):
            self._run({14: 46, 51: None})

    def test_one_origin_is_not_an_invariance_check(self):
        rig = Rig(arena="a.state", attacker_port=0, defender_port=1,
                  guard_buttons=("l",))
        script = sp.special_script(fake_profile(), "reptile", "acid_spit",
                                   facing="right")
        with self.assertRaises(ValueError):
            sp.origin_invariance(
                None, rig=rig, script=script, who="attacker", port=0,
                origins=[(14, "input_end+1")], end_frame=141,
                observables=["struct_velocity"], sample_fn=None,
                input_latency_frames={"struct_velocity": 1},
                defender_guard=False)


class GuardHeightVerdict(unittest.TestCase):
    """The pure half: which stance actually stopped the move, read off damage."""

    @staticmethod
    def _t(variant, connected, damage):
        return sp.GuardTrial(variant=variant, guard_buttons=("l",),
                             connected=connected, damage=damage,
                             contact_frame=21 if connected else None,
                             victim_knockdown=False)

    def test_chip_under_both_stances_is_a_mid(self):
        v, _ = sp.guard_height_verdict(
            13, [self._t("standing", True, 3), self._t("crouching", True, 3)])
        self.assertEqual(v, "mid")

    def test_full_damage_through_a_standing_guard_is_a_low(self):
        v, note = sp.guard_height_verdict(
            13, [self._t("standing", True, 13), self._t("crouching", True, 3)])
        self.assertEqual(v, "low")
        self.assertIn("NOT blocked", note)

    def test_full_damage_through_a_crouching_guard_is_an_overhead(self):
        v, _ = sp.guard_height_verdict(
            13, [self._t("standing", True, 3), self._t("crouching", True, 13)])
        self.assertEqual(v, "overhead")

    def test_a_stance_that_made_the_move_whiff_did_not_block_it(self):
        """§2.6 in the other direction: zero damage means WHIFF, not block."""
        v, note = sp.guard_height_verdict(
            13, [self._t("standing", True, 3),
                 self._t("crouching", False, None)])
        self.assertIsNone(v)
        self.assertIn("did not connect at all", note)

    def test_no_unguarded_reference_means_no_verdict(self):
        v, note = sp.guard_height_verdict(None, [self._t("standing", True, 3)])
        self.assertIsNone(v)
        self.assertIn("whiff", note)

    def test_neither_stance_blocking_is_null_not_unblockable_by_default(self):
        v, note = sp.guard_height_verdict(
            13, [self._t("standing", True, 13), self._t("crouching", True, 13)])
        self.assertIsNone(v)
        self.assertIn("unblockable", note)


class GuardHeightRig(unittest.TestCase):
    def test_each_stance_is_driven_and_never_inferred(self):
        """§2.6: the lab holds the stance, so it KNOWS it — the damage only
        has to say whether the hit was reduced."""
        rig = Rig(arena="a.state", attacker_port=0, defender_port=1,
                  guard_buttons=("l",))
        script = sp.special_script(fake_profile(), "reptile", "slide",
                                   facing="right")
        seen = []

        def fake_replay(session, **k):
            held = tuple(k["rig"].guard_buttons) if k["defender_guard"] else ()
            seen.append(held)
            dmg = {(): 13, ("l",): 13, ("l", "down"): 3}[held]
            return ([{"c": 161, "ax": 474, "ay": 85, "vx": 654, "vy": 85}] * 21
                    + [{"c": 161 - dmg, "ax": 580, "ay": 85,
                        "vx": 700, "vy": 85}] * 10)

        orig = sp.replay
        sp.replay = fake_replay
        try:
            gh = sp.measure_guard_height(
                None, rig=rig, script=script, contact_read=lambda s: None,
                reads={k: (lambda s: None) for k in
                       ("attacker_x", "attacker_y", "victim_x", "victim_y")},
                stances={"standing": ("l",), "crouching": ("l", "down")})
        finally:
            sp.replay = orig
        self.assertEqual(seen, [(), ("l",), ("l", "down")])
        self.assertEqual(gh.unguarded_damage, 13)
        self.assertEqual(gh.verdict, "low")
        self.assertEqual(gh.by_variant("standing").damage, 13)


class WhiffRecoveryAnchor(unittest.TestCase):
    def _rig_script(self):
        rig = Rig(arena="a.state", attacker_port=0, defender_port=1,
                  guard_buttons=("l",))
        script = sp.special_script(fake_profile(), "reptile", "invisibility",
                                   facing="right")
        return rig, script

    def test_an_anchor_inside_the_moves_own_input_is_refused(self):
        """`hold_buttons` REPLACES: a probe on the move's own button frame
        means no move came out in either run, and the sweep reports a
        REPRODUCIBLE first_true=0 that is the fighter walking."""
        rig, script = self._rig_script()
        for bad in (0, 5, script.total_frames):
            with self.assertRaises(sp.WhiffAnchorError):
                sp.measure_whiff_recovery(
                    None, rig=rig, script=script, port=0,
                    observables=["struct_velocity"], sample_fn=None,
                    origin=bad)

    def test_a_run_that_connects_is_not_a_whiff(self):
        rig, script = self._rig_script()
        orig = sp.replay
        sp.replay = lambda session, **k: (
            [{"c": 161, "ax": None, "ay": None, "vx": None, "vy": None}] * 30
            + [{"c": 150, "ax": None, "ay": None, "vx": None, "vy": None}] * 30)
        try:
            with self.assertRaises(ProbeError):
                sp.measure_whiff_recovery(
                    None, rig=rig, script=script, port=0,
                    observables=["struct_velocity"], sample_fn=None,
                    contact_read=lambda s: None)
        finally:
            sp.replay = orig

    def test_total_is_input_relative_and_null_when_nothing_diverged(self):
        _, script = self._rig_script()
        wr = sp.WhiffRecovery(
            move="invisibility", arena="a.state", origin=19,
            origin_kind="input_end+1", attack_input_frame=0,
            sweeps={"struct_velocity": sp.SideManifest(
                who="attacker", origin=19, origin_kind="input_end+1",
                sweep=sweep(20, window=3))},
            latencies={"struct_velocity": 1}, cal_points=(70, 100))
        self.assertEqual(wr.total("struct_velocity"), 42)
        null = sp.WhiffRecovery(
            move="invisibility", arena="a.state", origin=19,
            origin_kind="input_end+1", attack_input_frame=0,
            sweeps={"struct_velocity": sp.SideManifest(
                who="attacker", origin=19, origin_kind="input_end+1",
                sweep=sweep(None))},
            latencies={"struct_velocity": 1}, cal_points=(70, 100))
        self.assertIsNone(null.total("struct_velocity"))


def _png(width, height, fill):
    """A minimal 8-bit RGBA, non-interlaced, filter-0 PNG — the shape
    `app://screen` serves."""
    import struct
    import zlib

    rows = []
    for y in range(height):
        row = bytearray(b"\x00")
        for x in range(width):
            row += bytes(fill(x, y))
        rows.append(bytes(row))
    raw = b"".join(rows)

    def chunk(typ, data):
        return (struct.pack(">I", len(data)) + typ + data
                + struct.pack(">I", zlib.crc32(typ + data) & 0xFFFFFFFF))

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw))
            + chunk(b"IEND", b""))


class ScreenWitness(unittest.TestCase):
    """The framebuffer, for the one move with no memory observable at all."""

    def test_the_decoder_round_trips_an_rgba_png(self):
        blob = _png(4, 3, lambda x, y: (x * 10, y * 10, 5, 255))
        w, h, px = sp.decode_png_rgba(blob)
        self.assertEqual((w, h), (4, 3))
        self.assertEqual(px[:4], bytes((0, 0, 5, 255)))
        self.assertEqual(px[(2 * 4 + 3) * 4:(2 * 4 + 3) * 4 + 4],
                         bytes((30, 20, 5, 255)))

    def test_a_non_png_is_a_refusal(self):
        with self.assertRaises(sp.ScreenWitnessError):
            sp.decode_png_rgba(b"not a png at all")

    def test_the_region_read_crops_and_refuses_an_out_of_bounds_rect(self):
        blob = _png(4, 3, lambda x, y: (x, y, 0, 255))
        client = SimpleNamespace(read_resource=lambda uri: blob)
        read = sp.make_screen_region_read(client, region=(1, 0, 3, 2))
        self.assertEqual(read(None), bytes((1, 0, 0, 255, 2, 0, 0, 255,
                                            1, 1, 0, 255, 2, 1, 0, 255)))
        bad = sp.make_screen_region_read(client, region=(0, 0, 99, 2))
        with self.assertRaises(sp.ScreenWitnessError):
            bad(None)

    def test_pixel_diff_counts_pixels_not_bytes(self):
        a = bytes((1, 2, 3, 4)) * 3
        b = bytes((1, 2, 3, 4)) + bytes((9, 9, 9, 9)) * 2
        self.assertEqual(sp.region_pixel_diff(a, b), 2)
        with self.assertRaises(sp.ScreenWitnessError):
            sp.region_pixel_diff(a, a + b"\x00\x00\x00\x00")

    def _scan(self, **kw):
        rig = Rig(arena="a.state", attacker_port=0, defender_port=1,
                  guard_buttons=("l",))
        script = sp.special_script(fake_profile(), "reptile", "invisibility",
                                   facing="right")
        control = sp.MoveScript(name="trigger-only", steps=(script.steps[-1],))
        runs = []

        def fake_replay(session, **k):
            runs.append((k["script"].name, k["total_frames"], k.get("probe_at")))
            # A toy fighter drawn at his own x -- unless the move came out, in
            # which case nothing is drawn at all and the crop is constant.
            invisible = (k["script"] is script
                         and (k.get("probe_at") is None
                              or k["probe_at"] > script.total_frames))
            walked = 7 if k.get("probe_at") is not None else 0
            crop = bytes(4000) if invisible else bytes(
                [walked] * 400 + [0] * 3600)
            return [None] * k["total_frames"] + [{"screen": crop}]

        orig = sp.replay
        sp.replay = fake_replay
        try:
            got = sp.screen_preemption_scan(
                None, rig=rig, script=script, control_script=control,
                origin=script.total_frames, n_range=(0, 1, 2),
                directions=("left",), screen_read=lambda s: None, **kw)
        finally:
            sp.replay = orig
        return got, runs, script

    def test_zero_displacement_means_the_fighter_is_not_drawn(self):
        got, _, script = self._scan(capture_after=6)
        # N=0 puts the probe on the move's own last input frame, where it
        # replaces the trigger: he stays DRAWN, so walking him moves pixels.
        self.assertEqual(got[0]["left"], 100)
        # One frame later the move is already out and the walk moves nothing.
        self.assertEqual(got[1]["left"], 0)
        self.assertEqual(got[2]["left"], 0)

    def test_the_witness_carries_its_own_validity_check_at_every_n(self):
        """If the walk does not move a fighter who IS drawn, a zero on the
        real script means nothing."""
        got, _, _ = self._scan(capture_after=6)
        for n in (0, 1, 2):
            self.assertEqual(got[n]["left_drawn_control"], 100)

    def test_every_pair_it_compares_is_the_same_number_of_frames(self):
        _, runs, script = self._scan(capture_after=6)
        for n in (0, 1, 2):
            end = script.total_frames + n + 6
            at_end = [r for r in runs if r[1] == end]
            # one still/moved pair for the script, one for the drawn control
            self.assertEqual(len(at_end), 4, f"N={n}")
            self.assertEqual({r[0] for r in at_end},
                             {"invisibility", "trigger-only"})

    def test_a_capture_on_the_probe_frame_itself_is_refused(self):
        with self.assertRaises(sp.ScreenWitnessError):
            self._scan(capture_after=0)


class ReptileSignatures(unittest.TestCase):
    def test_the_retracted_move_is_gated_on_the_damage_that_discriminates_it(self):
        """`acid_spit`'s previous 'verification' was a close HP normal at
        point blank (24 damage). The signature is 15."""
        self.assertEqual(sp.MK2_REPTILE_SIGNATURES["acid_spit"].damage, 15)
        self.assertNotEqual(sp.MK2_REPTILE_SIGNATURES["acid_spit"].damage, 24)

    def test_a_damageless_move_has_no_signature_to_check(self):
        self.assertNotIn("invisibility", sp.MK2_REPTILE_SIGNATURES)

    def test_the_slide_is_gated_on_travel_and_the_victims_own_y(self):
        s = sp.MK2_REPTILE_SIGNATURES["slide"]
        self.assertTrue(s.victim_knockdown)
        self.assertIsNotNone(s.min_attacker_travel_px)


class PerPassRefusal(unittest.TestCase):
    """§4.3: `on_hit` and `on_block` are separate columns and must not be
    derived from each other. That cuts both ways — a refusal on one outcome
    must not throw the other one away. Measured need: `force_ball` launches
    its victim onto the attacker, and the separation that follows makes the
    ATTACKER'S own predicate non-monotone on the hit rig while the blocked rig
    sweeps cleanly."""

    def test_a_refused_hit_pass_still_leaves_the_block_pass(self):
        rig = Rig(arena="a.state", attacker_port=0, defender_port=1,
                  guard_buttons=("l",))
        script = sp.special_script(fake_profile(), "reptile", "force_ball",
                                   facing="right")
        frame = {"c": 161, "ax": 474, "ay": 85, "vx": 656, "vy": 85}
        hit = ([frame] * 71 + [{**frame, "c": 145}] * 60)
        blk = ([frame] * 71 + [{**frame, "c": 157}] * 60)

        def fake_replay(session, **k):
            return blk if k["defender_guard"] else hit

        def fake_cal(session, **k):
            return {"struct_velocity": 1}

        def fake_sweep(session, **k):
            if k["who"] == "attacker" and not k["defender_guard"]:
                raise ProbeError("predicate is not monotone (the separation)")
            n = {("attacker", True): 40, ("defender", False): 30,
                 ("defender", True): 26}[(k["who"], k["defender_guard"])]
            return {"struct_velocity": sp.SideManifest(
                who=k["who"], origin=k["origin"], origin_kind=k["origin_kind"],
                sweep=sweep(n, window=3, max_search=90))}

        saved = (sp.replay, sp.calibrate_for_move, sp.sweep_side)
        sp.replay, sp.calibrate_for_move, sp.sweep_side = (
            fake_replay, fake_cal, fake_sweep)
        try:
            m = sp.measure_special(
                None, rig=rig, script=script, observables=["struct_velocity"],
                sample_fns={0: None, 1: None}, contact_read=lambda s: None,
                reads={k: (lambda s: None) for k in
                       ("attacker_x", "attacker_y", "victim_x", "victim_y")},
                expect=sp.Signature(damage=16, hits=1),
                attacker_origin_kind="input_end+1")
        finally:
            sp.replay, sp.calibrate_for_move, sp.sweep_side = saved
        self.assertIn("attacker/hit", m.refusals)
        self.assertIn("not monotone", m.refusals["attacker/hit"])
        self.assertIsNone(m.on_hit["struct_velocity"])
        self.assertIsNotNone(m.on_block["struct_velocity"])
        self.assertTrue(any("REFUSED" in n for n in m.notes))


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
