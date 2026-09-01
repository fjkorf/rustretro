"""Unit tests for `framelab.cancels` — the verdict logic, with no emulator.

The point of the module is that "cancel" is a COMPARISON, so these tests are
mostly about refusing to say the word when the comparison does not support it:
a free fighter, a missing gate, a drifting control, an override mistaken for a
cancel.

The numbers in `test_measured_mk2_shape` are the real ones measured on
`mk2.zip` rev L3.1 (Reptile mirror, `shadow/arenas/mk2/gap-45.state`, 72 px);
they are here as a regression on the classifier, not as a stored frame row.
"""

import unittest

from shadow_train.framelab.cancels import (
    CancelError,
    CancelSweep,
    ControlDriftError,
    Gate,
    StartupArm,
    StartupError,
    Trial,
    arm_from_sweep,
    classify,
    compare_startup,
    hitstop_shift,
    lead_in_for,
    onset_from_trace,
    trigger_frame,
)
from shadow_train.framelab.probe import MoveScript, ScriptStep


def sweep(pairs, *, lead="HP", follow="slide", landed=None):
    """`pairs` is [(n, onset, control_onset)]."""
    tr = [
        Trial(n=n, onset=o, control_onset=c,
              lead_landed=(None if landed is None else landed(n)))
        for n, o, c in pairs
    ]
    return CancelSweep(lead=lead, follow=follow, arena="a", gap_px=72, trials=tr)


class TrialArithmetic(unittest.TestCase):
    def test_delay_is_null_when_the_followup_never_came_out(self):
        t = Trial(n=5, onset=None, control_onset=8)
        self.assertIsNone(t.delay)
        self.assertFalse(t.undelayed)
        self.assertFalse(t.came_out)

    def test_delay_is_null_when_the_control_is_null(self):
        self.assertIsNone(Trial(n=5, onset=8, control_onset=None).delay)

    def test_undelayed_requires_exactly_zero(self):
        self.assertTrue(Trial(n=5, onset=8, control_onset=8).undelayed)
        self.assertFalse(Trial(n=5, onset=9, control_onset=8).undelayed)


class UndelayedRun(unittest.TestCase):
    def test_only_the_unbroken_tail_counts(self):
        # a single undelayed trial deep in the clamped region must NOT open
        # the window -- that is the isolated-TRUE flake shape.
        s = sweep([(2, 20, 5), (3, 6, 6), (4, 20, 7), (5, 8, 8), (6, 9, 9)])
        self.assertEqual(s.undelayed_from, 5)

    def test_all_clamped_gives_none(self):
        self.assertIsNone(sweep([(2, 20, 5), (3, 20, 6)]).undelayed_from)


class Classification(unittest.TestCase):
    def test_cancel_needs_a_closed_gate(self):
        # undelayed from N=2, but nothing is gated -> the fighter is just free
        s = sweep([(n, n + 3, n + 3) for n in range(2, 10)])
        v = classify(s, [Gate(kind="walk", floor=None)])
        self.assertEqual(v.verdict, "link")
        self.assertIsNone(v.margin)

    def test_cancel_when_undelayed_below_a_closed_gate(self):
        s = sweep([(n, n + 3, n + 3) for n in range(2, 10)])
        v = classify(s, [Gate(kind="normal", floor=19), Gate(kind="walk", floor=20)])
        self.assertEqual(v.verdict, "cancel")
        self.assertEqual(v.undelayed_from, 2)
        self.assertEqual(v.gate.kind, "walk")     # strictest gate wins
        self.assertEqual(v.margin, 18)

    def test_link_when_the_window_opens_only_at_the_gate(self):
        pairs = [(n, max(n, 19) + 3, n + 3) for n in range(2, 25)]
        v = classify(sweep(pairs), [Gate(kind="normal", floor=19)])
        self.assertEqual(v.verdict, "link")
        self.assertEqual(v.undelayed_from, 19)

    def test_no_followup_is_its_own_verdict(self):
        v = classify(sweep([(n, None, n + 3) for n in range(2, 8)]), [Gate("walk", 20)])
        self.assertEqual(v.verdict, "no-followup")
        self.assertIsNone(v.undelayed_from)

    def test_empty_sweep_refuses(self):
        with self.assertRaises(CancelError):
            classify(CancelSweep(lead="HP", follow="slide", arena="a", gap_px=None), [])

    def test_drifting_control_refuses_rather_than_reporting(self):
        # control onset must advance one frame per frame of N
        pairs = [(2, 5, 5), (3, 6, 6), (4, 7, 9)]
        with self.assertRaises(ControlDriftError):
            classify(sweep(pairs), [Gate("walk", 20)])


class OverridesAreNotCancels(unittest.TestCase):
    def test_override_region_is_reported_separately(self):
        # below N=10 the follow-up replaces the lead; at/above it both land
        s = sweep([(n, n + 3, n + 3) for n in range(2, 16)], landed=lambda n: n >= 10)
        self.assertEqual(s.overrides, tuple(range(2, 10)))
        self.assertEqual(s.both_landed_from, 10)
        v = classify(s, [Gate("normal", 19)])
        self.assertEqual(v.verdict, "cancel")
        self.assertEqual(v.both_landed_from, 10)
        self.assertEqual(v.overrides, tuple(range(2, 10)))


class MeasuredMk2Shape(unittest.TestCase):
    """The real Reptile numbers, as a regression on the classifier."""

    def test_measured_mk2_shape(self):
        # HP@0 (contacts f11) -> slide chord at N: onset == control at every N,
        # and the HP still lands from N=10. Gates measured in the same rig:
        # a follow-up KICK is impossible until N=19; a WALK until N=20.
        s = sweep([(n, n + 3, n + 3) for n in range(2, 27)],
                  lead="HP", follow="slide", landed=lambda n: n >= 10)
        v = classify(s, [Gate("normal-kick", 19, "HP->LK first lands at N=19"),
                         Gate("walk", 20, "pointer_x manifest 22, latency 2")])
        self.assertEqual(v.verdict, "cancel")
        self.assertEqual(v.undelayed_from, 2)
        self.assertEqual(v.both_landed_from, 10)
        self.assertEqual(v.margin, 18)

    def test_punch_into_kick_is_inconclusive_not_a_cancel(self):
        # HP@0 -> LK@N: nothing at all below 19, then undelayed. Against the
        # walk gate at 20 that is a 1-frame margin measured through a
        # DIFFERENT observable -- inside §8.4's slop, so it must not be called
        # a cancel. This is the exact case the live run produced.
        pairs = [(n, None, n + 8) for n in range(2, 19)]
        pairs += [(n, n + 8, n + 8) for n in range(19, 25)]
        v = classify(sweep(pairs, lead="HP", follow="LK"), [Gate("walk", 20)])
        self.assertEqual(v.verdict, "inconclusive")
        self.assertEqual(v.margin, 1)
        self.assertEqual(v.undelayed_from, 19)

    def test_a_generous_min_margin_can_still_be_cleared(self):
        s = sweep([(n, n + 3, n + 3) for n in range(2, 27)], lead="HP", follow="slide")
        self.assertEqual(classify(s, [Gate("walk", 20)], min_margin=17).verdict, "cancel")
        self.assertEqual(classify(s, [Gate("walk", 20)], min_margin=19).verdict,
                         "inconclusive")


class Helpers(unittest.TestCase):
    def test_onset_tolerates_idle_breathing(self):
        self.assertIsNone(onset_from_trace([100, 101, 99, 102, 98]))
        self.assertEqual(onset_from_trace([100, 100, 100, 110, 120]), 3)

    def test_onset_is_null_on_an_unreadable_base(self):
        self.assertIsNone(onset_from_trace([None, 5, 60]))

    def test_lead_in_shapes(self):
        self.assertEqual([s.frames for s in lead_in_for(2, ("b",), hold=2)], [2])
        steps = lead_in_for(10, ("b",), hold=2)
        self.assertEqual([s.frames for s in steps], [2, 8])
        self.assertEqual(steps[0].buttons, ("b",))
        self.assertEqual(steps[1].buttons, ())

    def test_lead_in_refuses_to_truncate_the_lead(self):
        with self.assertRaises(CancelError):
            lead_in_for(1, ("b",), hold=2)


# ── the startup half: (A) earlier permission vs (B) shortened startup ─────


def arm(lead, outcome, gate, startups, **kw):
    return StartupArm(lead=lead, outcome=outcome, gate=gate,
                      startups=tuple(startups), **kw)


class TriggerFrame(unittest.TestCase):
    """The origin, and why it is not `attack_input_frame`."""

    def test_single_step_special_triggers_at_the_macro_start(self):
        # Reptile's slide is one step: `back + LK+LP+Block`.
        sc = MoveScript(name="slide",
                        lead_in=(ScriptStep(frames=2, buttons=("y",)),
                                 ScriptStep(frames=8, buttons=())),
                        steps=(ScriptStep(frames=8, buttons=("left", "a", "b", "l")),))
        self.assertEqual(sc.attack_input_frame, 10)
        self.assertEqual(trigger_frame(sc), 10)

    def test_multi_step_special_triggers_after_its_direction_steps(self):
        # force_ball is `B . B+HP+LP` with a 2-frame neutral gap: the trigger
        # is 5 frames after the macro starts, and measuring from the macro
        # start would compare two different instants.
        sc = MoveScript(name="force_ball",
                        lead_in=(ScriptStep(frames=10, buttons=()),),
                        steps=(ScriptStep(frames=3, buttons=("left",)),
                               ScriptStep(frames=2, buttons=()),
                               ScriptStep(frames=3, buttons=("left", "y", "b"))))
        self.assertEqual(sc.attack_input_frame, 10)
        self.assertEqual(trigger_frame(sc), 15)

    def test_a_pure_direction_script_has_no_trigger(self):
        sc = MoveScript(name="walk", steps=(ScriptStep(frames=6, buttons=("left",)),))
        with self.assertRaises(StartupError):
            trigger_frame(sc)


class ArmCollapse(unittest.TestCase):
    def test_gate_is_the_trigger_of_the_unbroken_tail(self):
        # HK@0 -> slide@N on hit: nothing below N=45, everything from 45.
        trials = [Trial(n=n, onset=None, control_onset=n + 3) for n in range(30, 45)]
        trials += [Trial(n=n, onset=n + 3, control_onset=n + 3) for n in range(45, 50)]
        s = CancelSweep(lead="HK", follow="slide", arena="a", gap_px=72, trials=trials)
        a = arm_from_sweep(s, outcome="hit", triggers={n: n for n in range(30, 50)},
                           hitstop=12, observable="pointer_x")
        self.assertEqual(a.gate, 45)
        self.assertEqual(a.startup, 3)

    def test_an_isolated_success_below_the_floor_does_not_open_the_gate(self):
        trials = [Trial(n=30, onset=33, control_onset=33)]
        trials += [Trial(n=n, onset=None, control_onset=n + 3) for n in range(31, 45)]
        trials += [Trial(n=n, onset=n + 3, control_onset=n + 3) for n in range(45, 48)]
        s = CancelSweep(lead="HK", follow="slide", arena="a", gap_px=72, trials=trials)
        a = arm_from_sweep(s, outcome="hit", triggers={n: n for n in range(30, 48)})
        self.assertEqual(a.gate, 45)

    def test_a_gate_that_never_opened_is_null(self):
        s = CancelSweep(lead="HK", follow="slide", arena="a", gap_px=72,
                        trials=[Trial(n=n, onset=None, control_onset=n + 3)
                                for n in range(2, 31)])
        a = arm_from_sweep(s, outcome="hit", triggers={n: n for n in range(2, 31)})
        self.assertIsNone(a.gate)
        self.assertIsNone(a.startup)

    def test_startup_is_measured_from_the_trigger_not_the_macro_start(self):
        # force_ball: trigger at N+5, contact at trigger+31.
        trials = [Trial(n=n, onset=n + 36, control_onset=n + 36) for n in range(2, 8)]
        s = CancelSweep(lead="HP", follow="force_ball", arena="a", gap_px=76, trials=trials)
        self.assertEqual(
            arm_from_sweep(s, outcome="hit",
                           triggers={n: n + 5 for n in range(2, 8)}).startup, 31)

    def test_a_missing_trigger_refuses_rather_than_using_n(self):
        s = CancelSweep(lead="HP", follow="force_ball", arena="a", gap_px=76,
                        trials=[Trial(n=3, onset=39, control_onset=39)])
        with self.assertRaises(StartupError):
            arm_from_sweep(s, outcome="hit", triggers={})


class StartupConstancy(unittest.TestCase):
    def test_a_drifting_startup_refuses_instead_of_averaging(self):
        # force_ball after a connecting far HK really reads 43,44,45,46: the
        # HK threw the target 83 px downrange, so this is travel time.
        a = arm("HK", "hit", 45, [43, 44, 44, 45, 46], gap_px=159)
        with self.assertRaises(StartupError):
            a.startup


class StartupComparison(unittest.TestCase):
    """The measured MK2 answer: (A). Regression on the comparator, not a row."""

    def setUp(self):
        self.base = arm("none", "none", 2, [3] * 8, gap_px=72, observable="pointer_x")

    def test_punch_lead_leaves_the_startup_unchanged(self):
        v = compare_startup(self.base, arm("HP", "hit", 2, [3] * 8, gap_px=72))
        self.assertEqual(v.verdict, "unchanged")
        self.assertEqual(v.delta, 0)

    def test_kick_lead_is_gated_far_later_but_still_unchanged_startup(self):
        # gate 2 -> 45 (permission), startup 3 -> 3 (speed).
        v = compare_startup(self.base, arm("HK", "hit", 45, [3] * 6, gap_px=72,
                                           hitstop=12))
        self.assertEqual(v.verdict, "unchanged")

    def test_a_genuinely_shortened_startup_would_be_reported_as_such(self):
        v = compare_startup(self.base, arm("HP", "hit", 2, [1] * 5, gap_px=72))
        self.assertEqual(v.verdict, "shortened")
        self.assertEqual(v.delta, -2)

    def test_a_mismatched_gap_is_not_comparable(self):
        v = compare_startup(self.base, arm("HK", "hit", 45, [46] * 4, gap_px=159))
        self.assertEqual(v.verdict, "not-comparable")
        self.assertIn("travel", v.note)

    def test_an_arm_that_never_came_out_is_not_comparable(self):
        v = compare_startup(self.base, arm("HK", "hit", None, [], gap_px=72))
        self.assertEqual(v.verdict, "not-comparable")
        self.assertIsNone(v.arm_startup)


class HitstopShiftTests(unittest.TestCase):
    def test_the_measured_mk2_shift_equals_the_measured_hitstop(self):
        whiff = arm("HK", "whiff", 33, [3] * 6, hitstop=12, gap_px=180)
        hit = arm("HK", "hit", 45, [3] * 6, hitstop=12, gap_px=72)
        s = hitstop_shift(whiff, hit)
        self.assertEqual(s.shift, 12)
        self.assertFalse(s.absorbed)
        self.assertIn("NOT bypassed", s.note)

    def test_block_counts_as_contact(self):
        whiff = arm("HK", "whiff", 33, [3] * 4, hitstop=12)
        blocked = arm("HK", "block", 45, [3] * 4, hitstop=12)
        self.assertEqual(hitstop_shift(whiff, blocked).shift, 12)

    def test_a_zero_shift_against_a_real_hitstop_is_the_stronger_claim(self):
        whiff = arm("HK", "whiff", 33, [3] * 4, hitstop=12)
        hit = arm("HK", "hit", 33, [3] * 4, hitstop=12)
        s = hitstop_shift(whiff, hit)
        self.assertTrue(s.absorbed)
        self.assertIn("DISAGREE", s.note)

    def test_a_null_gate_yields_a_null_shift_not_zero(self):
        s = hitstop_shift(arm("HK", "whiff", 33, [3]), arm("HK", "hit", None, []))
        self.assertIsNone(s.shift)
        self.assertIsNone(s.absorbed)

    def test_two_different_leads_are_refused(self):
        with self.assertRaises(StartupError):
            hitstop_shift(arm("HP", "whiff", 2, [3]), arm("HK", "hit", 45, [3]))

    def test_a_whiff_arm_is_required_on_the_left(self):
        with self.assertRaises(StartupError):
            hitstop_shift(arm("HK", "hit", 45, [3]), arm("HK", "hit", 45, [3]))


if __name__ == "__main__":
    unittest.main()
