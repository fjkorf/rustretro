"""Unit tests for `framelab.airborne` — docs/frames.md applied to an
AIRBORNE ATTACKER, and the retraction of §10's "jumping normals need a
different observable".

Everything here runs against fakes. The three functions that drive the
emulator (`measure_jump_arc`, `air_control_scan`, `throw_scan`) call exactly
one collaborator — `probe.replay`, imported into this module — and
`measure_njp` calls three (`observe_move`, `calibrate_for_move`,
`sweep_side`), all imported by name. Substituting those keeps the module's
REFUSALS under test without a live rig, which is the point: the numbers came
off the rig, the refusals are what stops the next number being wrong.

The canned arc is Reptile's, measured on `shadow/arenas/mk2/gap-60.state`:
resting y 85, take-off f6, apex −10 at f23–26, landing f44.
"""

from __future__ import annotations

import unittest

from shadow_train.framelab import airborne as ab
from shadow_train.framelab.probe import ProbeError, Rig, SweepResult
from shadow_train.framelab.specials import MoveObservation, SideManifest

# The measured arc, frame by frame (docs/frames.md §5's signed `obj+0x16`:
# smaller = higher). Frames 0-5 are the pre-jump; y returns to 85 at f44.
ARC_Y = ([85] * 6
         + [75, 66, 58, 50, 42, 35, 29, 23, 17, 12, 8, 4, 0, -3, -5, -7, -9]
         + [-10, -10, -10, -10]
         + [-9, -7, -5, -3, 0, 4, 8, 12, 17, 23, 29, 35, 42, 50, 58, 66, 75]
         + [85] * 30)

RIG = Rig(arena="gap-60.state", attacker_port=0, defender_port=1,
          guard_buttons=("l",), quiet_frames=20)


def patched(module, **names):
    """A tiny context manager for the monkeypatching this file does a lot of."""
    class _P:
        def __enter__(self):
            self.old = {k: getattr(module, k) for k in names}
            for k, v in names.items():
                setattr(module, k, v)

        def __exit__(self, *exc):
            for k, v in self.old.items():
                setattr(module, k, v)
    return _P()


def arc(**overrides):
    kw = dict(resting_y=85, takeoff=6, landing=44, apex_y=-10,
              apex_frames=(23, 24, 25, 26), x_drift_px=2,
              y_trace=tuple(ARC_Y))
    kw.update(overrides)
    return ab.JumpArc(**kw)


def sweep(first_true, *, observable="struct_velocity", window=3, max_search=60,
          monotone=True):
    predicate = tuple(
        first_true is not None and i >= first_true for i in range(max_search + 1)
    )
    return SweepResult(
        observable=observable, method="linear_sweep", direction="left",
        first_true=first_true, predicate=predicate, monotone=monotone,
        window=window, input_latency_frames=window - 2, max_search=max_search,
        port=0, rig_guard_state="none", runs=1,
    )


def observation(*, contacts=(39,), damage=16, attacker_airborne_until=52,
                victim_airborne_until=0, ay_at_contact=42):
    """A `MoveObservation` shaped like a connecting NJP: one contact at f39,
    the attacker still airborne there and landing at f52 (the arc's own f44
    plus the contact hitstop), the victim never leaving the ground."""
    trace = tuple(
        {"c": 161 if i < 39 else 145, "ax": 607, "ay": (ay_at_contact if i == 39
                                                        else ARC_Y[min(i, len(ARC_Y) - 1)]),
         "vx": 669, "vy": 85}
        for i in range(120)
    )
    return MoveObservation(
        move="njHP@24", contacts=tuple(contacts), contact_values=(145,),
        damage=damage, attacker_x=(607, 607), victim_x=(669, 669), gap_px=62,
        attacker_airborne_until=attacker_airborne_until,
        victim_airborne_until=victim_airborne_until,
        crossed=False, facing_before="right", facing_after="right", trace=trace,
    )


# ── the arc ───────────────────────────────────────────────────────────────


class Arc(unittest.TestCase):
    def _measure(self, ys, xs=None, **kw):
        trace = [{"y": y, "x": (xs[i] if xs else 607)} for i, y in enumerate(ys)]
        with patched(ab, replay=lambda *a, **k: trace):
            return ab.measure_jump_arc(
                None, rig=RIG, port=0, y_read=lambda s: None,
                x_read=lambda s: None, total_frames=len(ys) - 1, **kw)

    def test_takeoff_landing_and_apex_come_out_of_the_trace(self):
        a = self._measure(ARC_Y)
        self.assertEqual((a.resting_y, a.takeoff, a.landing), (85, 6, 44))
        self.assertEqual(a.airtime, 38)
        self.assertEqual((a.apex_y, a.apex_frames), (-10, (23, 24, 25, 26)))

    def test_resting_y_is_read_off_the_rig_not_assumed(self):
        """docs/frames.md §10: resting y is character- AND stage-dependent
        (85/83 on one stage, 89/87 on another), so there is no scalar
        GROUND_Y to compare against."""
        shifted = [y + 4 for y in ARC_Y]
        a = self._measure(shifted)
        self.assertEqual(a.resting_y, 89)
        self.assertEqual((a.takeoff, a.landing), (6, 44))
        self.assertEqual(a.height_above_rest(23), 95)

    def test_height_above_rest_is_positive_upwards(self):
        a = self._measure(ARC_Y)
        self.assertEqual(a.height_above_rest(0), 0)
        self.assertEqual(a.height_above_rest(39), 43)   # 85 − 42
        self.assertEqual(a.height_above_rest(23), 95)   # the measured jump height

    def test_a_hold_that_never_leaves_the_ground_is_refused(self):
        """A 1-frame `up` measured live: 70 frames of resting y. An attack
        scripted on top of that is a STANDING punch under a jumping name."""
        with self.assertRaises(ab.NotAirborneError):
            self._measure([85] * 70)

    def test_x_drift_is_reported_because_a_neutral_jump_must_not_have_one(self):
        a = self._measure(ARC_Y, xs=[607] * 20 + [612] * 60)
        self.assertEqual(a.x_drift_px, 5)

    def test_the_arena_s_first_frame_settle_is_kept_out_of_the_jump_s_drift(self):
        """Live on `gap-30.state`: x steps 546 -> 549 on the FIRST frame after
        the load and is then constant for all 38 airborne frames. Folded into
        one number that reads as a 3 px drift and impugns the jump."""
        a = self._measure(ARC_Y, xs=[546] + [549] * (len(ARC_Y) - 1))
        self.assertEqual(a.x_drift_px, 0)
        self.assertEqual(a.settle_px, 3)


# ── the script ────────────────────────────────────────────────────────────


class Script(unittest.TestCase):
    def test_the_jump_is_lead_in_and_the_punch_is_the_move(self):
        s = ab.njp_script(throw_at=24, buttons=("y",))
        self.assertEqual([(st.frames, st.buttons) for st in s.lead_in],
                         [(2, ("up",)), (22, ())])
        self.assertEqual([(st.frames, st.buttons) for st in s.steps],
                         [(2, ("y",))])
        # This is what `first_active_frame` is measured relative to (§4.4).
        self.assertEqual(s.attack_input_frame, 24)

    def test_throwing_on_the_jump_frame_is_refused_not_silently_chorded(self):
        with self.assertRaises(ValueError):
            ab.njp_script(throw_at=1, buttons=("y",))

    def test_the_jump_hold_threshold_is_the_measured_one(self):
        self.assertEqual(ab.JUMP_HOLD_FRAMES, 2)
        s = ab.njp_script(throw_at=2, buttons=("y",))
        self.assertEqual(len(s.lead_in), 1)      # no neutral filler at the floor


# ── the air-control scan ──────────────────────────────────────────────────


class AirControl(unittest.TestCase):
    def _scan(self, diverge_at=None, **kw):
        """`diverge_at` maps probe frame -> the offset at which the probe run
        differs from the control. Anything else is byte-identical."""
        diverge_at = diverge_at or {}

        def fake_replay(session, **k):
            n = k["probe_at"]
            probing = bool(k["probe_buttons"])
            total = k["total_frames"]
            off = diverge_at.get(n)
            out = []
            for i in range(total + 1):
                moved = probing and off is not None and i >= n + off
                out.append({"struct_velocity": b"\x01" if moved else b"\x00",
                            "pointer_x": 608 if moved else 607})
            return out

        with patched(ab, replay=fake_replay):
            return ab.air_control_scan(
                None, rig=RIG, script=ab.njp_script(throw_at=24, buttons=("y",)),
                arc=arc(), port=0,
                observables=("struct_velocity", "pointer_x"),
                sample_fn=lambda s: {}, **kw)

    def test_a_clean_scan_covers_every_airborne_frame_in_both_directions(self):
        ev = self._scan()
        self.assertTrue(ev.clean)
        self.assertEqual(ev.airborne_frames[0], 6)
        self.assertEqual(ev.airborne_frames[-1], 43)
        # 38 airborne frames x 2 directions x 2 observables.
        self.assertEqual(len(ev.samples), 38 * 2 * 2)

    def test_no_comparison_window_ever_reaches_the_landing_frame(self):
        """A window that ran past landing would catch the fighter's legitimate
        post-landing walk and report it as air control."""
        ev = self._scan()
        for n, w in ev.windows.items():
            self.assertLessEqual(n + w, 44, f"frame {n} window {w} crosses landing")

    def test_a_mid_air_divergence_raises_with_the_frame_that_produced_it(self):
        with self.assertRaises(ab.AirControlError) as cm:
            self._scan(diverge_at={20: 2})
        self.assertIn("20", str(cm.exception))

    def test_divergences_are_reported_rather_than_raised_when_asked(self):
        ev = self._scan(diverge_at={20: 2}, raise_on_divergence=False)
        self.assertFalse(ev.clean)
        self.assertEqual({d[0] for d in ev.divergences}, {20})

    def test_a_ground_control_frame_is_what_makes_clean_mean_anything(self):
        """A scan that finds nothing ANYWHERE is a broken scan, not a clean
        one — the §4.2 liveness-probe mistake with the polarity flipped."""
        blind = self._scan(ground_control_frames=(64,))
        self.assertTrue(blind.clean)
        self.assertIs(blind.sensitive, False)
        live = self._scan(diverge_at={64: 3}, ground_control_frames=(64,))
        self.assertTrue(live.clean)
        self.assertIs(live.sensitive, True)

    def test_sensitive_is_null_when_no_control_frame_was_scanned(self):
        self.assertIsNone(self._scan().sensitive)


# ── the connect map over the arc ──────────────────────────────────────────


class WhiffBoundaryDerivations(unittest.TestCase):
    """The live Reptile numbers: contact pinned at f39 for J=16..30 (the
    geometry is what limits it), then tracking J+9 for J=31..35."""

    LIVE = {
        **{j: (None, None, None) for j in (12, 13, 14, 15)},
        **{j: (39, 16, 43) for j in range(16, 31)},
        31: (40, 16, 35), 32: (41, 16, 27), 33: (42, 16, 19),
        34: (43, 16, 10), 35: (44, 16, 0),
        **{j: (None, None, None) for j in (36, 37, 38, 39)},
    }

    def test_faf_is_the_minimum_input_relative_contact(self):
        b = ab.whiff_boundary(self.LIVE)
        self.assertEqual(b.first_active_frame, 9)
        self.assertEqual(b.geometry_frame, 39)

    def test_faf_needs_two_throws_to_agree_before_it_is_claimed(self):
        """One J hitting at J+9 could be the geometry coinciding; two of them
        tracking J+9 is a startup."""
        one = {16: (39, 16, 43), 31: (40, 16, 35)}
        self.assertIsNone(ab.whiff_boundary(one).first_active_frame)

    def test_the_active_window_bracket_closes_on_a_single_value(self):
        b = ab.whiff_boundary(self.LIVE)
        self.assertEqual((b.active_lo, b.active_hi), (15, 15))

    def test_whiffs_are_a_result_and_stay_in_the_map(self):
        b = ab.whiff_boundary(self.LIVE)
        self.assertEqual(b.connecting[0], 16)
        self.assertEqual(b.connecting[-1], 35)
        self.assertIsNone(b.contact[14])
        self.assertIsNone(b.contact[38])

    def test_a_scan_that_never_connects_claims_nothing(self):
        b = ab.whiff_boundary({j: (None, None, None) for j in range(10, 20)})
        self.assertEqual(b.connecting, ())
        self.assertIsNone(b.first_active_frame)
        self.assertIsNone(b.geometry_frame)
        self.assertIsNone(b.active_lo)


class ThrowScan(unittest.TestCase):
    def test_each_throw_reports_contact_damage_and_contact_height(self):
        def fake_replay(session, **k):
            j = k["script"].attack_input_frame
            contact = 39 if j >= 16 else None
            return [{"c": 161 if (contact is None or i < contact) else 145,
                     "y": ARC_Y[min(i, len(ARC_Y) - 1)]}
                    for i in range(k["total_frames"] + 1)]

        with patched(ab, replay=fake_replay):
            got = ab.throw_scan(
                None, rig=RIG, arc=arc(), buttons=("y",),
                throw_frames=(14, 24), contact_read=lambda s: None,
                attacker_y_read=lambda s: None)
        self.assertEqual(got[14], (None, None, None))
        # contact f39, 16 damage, 43 units above her own resting y.
        self.assertEqual(got[24], (39, 16, 43))


# ── one cell, and its refusals ────────────────────────────────────────────


class Cell(unittest.TestCase):
    def _measure(self, *, obs=None, manifests=None, air=None, **kw):
        obs = obs or observation()
        manifests = manifests or {
            "attacker/hit": 17, "defender/hit": 23,
            "attacker/block": 18, "defender/block": 7,
        }
        calls = {}

        def fake_sweep_side(session, *, who, port, origin, observables, **k):
            tag = "block" if k["defender_guard"] else "hit"
            shape = f"{who}/{tag}"
            calls.setdefault("shapes", []).append(shape)
            win = 3 if who == "attacker" or tag == "hit" else 12
            return {
                o: SideManifest(who=who, origin=origin, origin_kind="contact",
                                sweep=sweep(manifests[shape], observable=o,
                                            window=win))
                for o in observables
            }

        def fake_calibrate(session, *, port, at_n, confirm_at_n, observables, **k):
            calls.setdefault("cal", []).append((port, at_n, confirm_at_n))
            return {o: 1 for o in observables}

        air_ev = air if air is not None else ab.AirControlEvidence(
            airborne_frames=(6,), directions=("left",),
            observables=("struct_velocity",), samples=((6, "left", "sv", None),),
            windows={6: 12}, ground_control=((64, "left", "sv", 3),),
        )
        def fake_observe(session, **k):
            # The guarded rig is a DIFFERENT run of the game and gets its own
            # anchor (`kit.measure_cell`'s rule): live, the blocked NJP
            # connects one frame earlier than the clean one.
            if k.get("defender_guard"):
                return observation(contacts=(38,), damage=3)
            return obs

        with patched(ab, observe_move=fake_observe,
                     sweep_side=fake_sweep_side,
                     calibrate_for_move=fake_calibrate):
            m = ab.measure_njp(
                None, rig=RIG, script=ab.njp_script(throw_at=24, buttons=("y",),
                                                    name="njHP@24"),
                arc=arc(), observables=("struct_velocity", "pointer_x"),
                sample_fns={0: None, 1: None}, contact_read=lambda s: None,
                reads={k: (lambda s: None) for k in
                       ("attacker_x", "attacker_y", "victim_x", "victim_y")},
                air_control=air_ev, **kw)
        return m, calls

    def test_the_advantage_is_the_difference_of_absolute_manifests(self):
        m, _ = self._measure()
        # attacker: contact 39 + 17 + 3 = 59 (landing 52 + 7).
        # defender: contact 39 + 23 + 3 = 65 on hit; on block its own anchor is
        # f38, so 38 + 18 + 3 = 59 against 38 + 7 + 12 = 57.
        self.assertEqual(m.on_hit, {"struct_velocity": 6, "pointer_x": 6})
        self.assertEqual(m.on_block, {"struct_velocity": -2, "pointer_x": -2})

    def test_remaining_airtime_is_recorded_because_it_is_the_variable(self):
        m, _ = self._measure()
        self.assertEqual(m.contact_hit, 39)
        self.assertEqual(m.landing["hit"], 52)
        self.assertEqual(m.remaining_airtime, 13)
        self.assertEqual(m.contact_height, 43)

    def test_the_calibration_point_is_derived_from_this_run_s_landing(self):
        """§3.1's point must be HOLD-limited, and 'far enough past contact' is
        not a constant when the fighter is still in the air."""
        m, calls = self._measure()
        # attacker: floor = landing(52) − contact(39) = 13, so max(70, 13+40).
        self.assertIn((0, 70, 100), calls["cal"])
        self.assertEqual(m.cal_points["attacker/hit"], (70, 100))
        # A longer flight moves the point OUT with the airtime rather than
        # leaving it at 70, where the fighter would still be in the air.
        deep, _ = self._measure(
            obs=observation(attacker_airborne_until=90),
            manifests={"attacker/hit": 60, "defender/hit": 23,
                       "attacker/block": 18, "defender/block": 7})
        self.assertEqual(deep.cal_points["attacker/hit"], (91, 121))

    def test_a_manifest_before_landing_is_refused(self):
        """A boundary inside the flight is not a recovery: a fighter cannot
        walk in the air, so it is whatever the air-control scan missed."""
        with self.assertRaises(ab.MidAirManifestError) as cm:
            self._measure(manifests={"attacker/hit": 2, "defender/hit": 23,
                                     "attacker/block": 18, "defender/block": 7})
        self.assertIn("f52", str(cm.exception))

    def test_measuring_without_air_control_evidence_is_refused(self):
        with self.assertRaises(ab.AirborneError) as cm:
            with patched(ab, observe_move=lambda *a, **k: observation()):
                ab.measure_njp(
                    None, rig=RIG,
                    script=ab.njp_script(throw_at=24, buttons=("y",)),
                    arc=arc(), observables=("struct_velocity",),
                    sample_fns={0: None, 1: None}, contact_read=lambda s: None,
                    reads={k: (lambda s: None) for k in
                           ("attacker_x", "attacker_y", "victim_x", "victim_y")},
                    air_control=None)
        self.assertIn("air-control", str(cm.exception))

    def test_a_dirty_air_control_scan_blocks_the_cell(self):
        dirty = ab.AirControlEvidence(
            airborne_frames=(20,), directions=("left",),
            observables=("struct_velocity",),
            samples=((20, "left", "struct_velocity", 2),), windows={20: 12})
        with self.assertRaises(ab.AirControlError):
            self._measure(air=dirty)

    def test_an_insensitive_scan_blocks_the_cell_too(self):
        blind = ab.AirControlEvidence(
            airborne_frames=(20,), directions=("left",),
            observables=("struct_velocity",),
            samples=((20, "left", "struct_velocity", None),), windows={20: 12},
            ground_control=((64, "left", "struct_velocity", None),))
        with self.assertRaises(ab.AirborneError):
            self._measure(air=blind)

    def test_a_whiff_is_a_result_not_a_failure(self):
        m, _ = self._measure(obs=observation(contacts=(), damage=None))
        self.assertEqual(m.on_hit, {})
        self.assertIsNone(m.contact_hit)
        self.assertTrue(any("whiff" in n for n in m.notes))

    def test_a_grounded_contact_at_the_end_of_the_jump_is_flagged(self):
        m, _ = self._measure(
            obs=observation(contacts=(44,), attacker_airborne_until=44,
                            ay_at_contact=85))
        self.assertEqual(m.contact_height, 0)
        self.assertTrue(any("GROUNDED contact" in n for n in m.notes))

    def test_cross_observable_disagreement_refuses_the_row(self):
        def fake_sweep_side(session, *, who, port, origin, observables, **k):
            offset = {"struct_velocity": 0, "pointer_x": 4}
            return {o: SideManifest(who=who, origin=origin, origin_kind="contact",
                                    sweep=sweep(17 + offset[o], observable=o))
                    for o in observables}
        with patched(ab, observe_move=lambda *a, **k: observation(),
                     sweep_side=fake_sweep_side,
                     calibrate_for_move=lambda *a, **k: {"struct_velocity": 1,
                                                         "pointer_x": 2}):
            with self.assertRaises(ProbeError):
                ab.measure_njp(
                    None, rig=RIG,
                    script=ab.njp_script(throw_at=24, buttons=("y",)),
                    arc=arc(), observables=("struct_velocity", "pointer_x"),
                    sample_fns={0: None, 1: None}, contact_read=lambda s: None,
                    reads={k: (lambda s: None) for k in
                           ("attacker_x", "attacker_y", "victim_x", "victim_y")},
                    air_control=ab.AirControlEvidence(
                        airborne_frames=(6,), directions=("left",),
                        observables=("struct_velocity",),
                        samples=((6, "left", "sv", None),), windows={6: 12},
                        ground_control=((64, "left", "sv", 3),)))


# ── rows ──────────────────────────────────────────────────────────────────


class Rows(unittest.TestCase):
    def _cell(self, **kw):
        m = ab.AirborneMeasurement(move="njHP@24", arena="gap-60.state",
                                   throw_at=24, gap_px=62)
        m.obs_hit = observation()
        m.contact_hit, m.hits, m.contact_height = 39, 1, 43
        m.landing["hit"] = 52
        m.latencies = {"attacker/hit": {"struct_velocity": 1, "pointer_x": 2}}
        m.on_hit = {"struct_velocity": 6, "pointer_x": 6}
        m.on_block = {"struct_velocity": -1, "pointer_x": -1}
        for k, v in kw.items():
            setattr(m, k, v)
        return m

    def _row(self, m, **kw):
        base = dict(family="mk2", port="arcade", char="reptile", move="njHP",
                    core_id="c", rom_id="r", observable="struct_velocity")
        base.update(kw)
        return ab.njp_row(m, **base)

    def test_the_variant_carries_the_arc_frame_and_the_contact_height(self):
        """Two rows of this move at the same gap are different measurements,
        and §5 forbids averaging them into one."""
        row = self._row(self._cell())
        self.assertEqual(row["variant"], "throw@24/h43")
        self.assertEqual((row["on_hit"], row["on_block"]), (6, -1))
        self.assertEqual(row["gap_px"], 62)

    def test_provenance_columns_are_present_so_the_store_accepts_it(self):
        from shadow_train.framelab.store import REQUIRED_PROVENANCE_COLUMNS
        row = self._row(self._cell())
        for col in REQUIRED_PROVENANCE_COLUMNS:
            self.assertIsNotNone(row[col], col)

    def test_first_active_frame_and_connect_range_are_null_unless_given(self):
        row = self._row(self._cell())
        self.assertIsNone(row["first_active_frame"])
        self.assertIsNone(row["connect_range"])
        row = self._row(self._cell(), first_active_frame=9, connect_range=110)
        self.assertEqual((row["first_active_frame"], row["connect_range"]), (9, 110))

    def test_the_knockdown_gate_still_drops_on_hit(self):
        """Inherited from `specials.special_row` rather than reimplemented:
        §1.1's gate lives in exactly one place."""
        m = self._cell(obs_hit=observation(victim_airborne_until=70))
        row = self._row(m)
        self.assertIsNone(row["on_hit"])
        self.assertEqual(row["knockdown"], 1)
        self.assertEqual(row["on_block"], -1)


class CurveFaf(unittest.TestCase):
    def _cell(self, throw_at, contact):
        m = ab.AirborneMeasurement(move="x", arena="a", throw_at=throw_at)
        m.contact_hit = contact
        return m

    def test_two_agreeing_throws_are_needed(self):
        cells = [self._cell(31, 40), self._cell(32, 41), self._cell(24, 39)]
        self.assertEqual(ab.curve_first_active_frame(cells), 9)

    def test_one_throw_alone_claims_nothing(self):
        self.assertIsNone(ab.curve_first_active_frame([self._cell(31, 40)]))

    def test_no_contact_anywhere_is_null_not_zero(self):
        self.assertIsNone(ab.curve_first_active_frame([self._cell(31, None)]))


class Conventions(unittest.TestCase):
    def test_the_airborne_convention_says_what_the_number_means(self):
        """§4.3: "state the convention" — an airborne attacker's side of the
        advantage is landing-relative, not contact-relative, and a reader
        comparing it against a grounded row must be able to see that."""
        text = ab.AIRBORNE_ATTACKER_CONVENTION
        self.assertIn("AFTER LANDING", text)
        self.assertIn("REMAINING AIRTIME", text)


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
