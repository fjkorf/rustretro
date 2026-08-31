"""Unit tests for `framelab.hitstop` — docs/frames.md §1.2/§11 (`hitstop`,
measured for the first time) and the `FrameStore.update` it needed.

Reuses `tests.test_framelab_probe`'s `FakeGame`/`LabSession` harness rather
than reinventing a fake transport; `_HitstopFakeGame` below only adds what
that fixture does not model: an EXPLICIT, injectable freeze delay separate
from recovery/stun, and a whiff-path recovery clock (the base fixture is
built for post-contact actionability and does not arm the attacker's own
recovery unless something actually landed).
"""

from __future__ import annotations

import sqlite3
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

from tests.test_framelab_probe import FakeGame, contact_read, sampler
from shadow_train.framelab.hitstop import (
    CrossObservableHitstopError,
    HitstopError,
    WhiffNotAWhiffError,
    collapse_hitstop,
    compute_hitstop,
    measure_hitstop_cell,
    measure_hitstop_outcome,
    measure_whiff_reference,
    naive_freeze_span,
    plan_hitstop_cells,
)
from shadow_train.framelab.kit import MoveSpec
from shadow_train.framelab.probe import Rig
from shadow_train.framelab.session import LabSession
from shadow_train.framelab.store import FrameStore


# ── the naive detector: demonstrated unsound, never shipped ────────────────


class NaiveFreezeSpanTest(unittest.TestCase):
    """docs/frames.md's task brief: "a naive freeze detector will also fire
    on any frame where nothing happened to change." This is that detector,
    and this is it firing wrongly."""

    def test_false_fires_on_an_ordinary_idle_span_with_no_contact_at_all(self):
        # A fighter standing in neutral: nothing ever changes. No hit was
        # ever thrown, so the TRUE hitstop here is 0 -- but the naive
        # detector cannot tell "frozen because of a hit" from "frozen
        # because nobody did anything", because both produce IDENTICAL
        # evidence (docs/frames.md's own live finding for MK2 arcade: "a
        # fighter's struct is entirely static when untouched").
        idle_frame = {"health": 100, "velocity": 0, "anim": 0}
        trace = [dict(idle_frame) for _ in range(40)]
        span = naive_freeze_span(trace, keys=("health", "velocity", "anim"), start=0)
        # False positive: it reports the ENTIRE idle span as "frozen", which
        # would be reported as hitstop=39 for a contact that never happened.
        self.assertEqual(span, len(trace) - 1)

    def test_stops_at_the_first_real_change(self):
        trace = [
            {"health": 100, "velocity": 0},
            {"health": 100, "velocity": 0},
            {"health": 89, "velocity": 0},   # contact: health steps
            {"health": 89, "velocity": 0},
        ]
        self.assertEqual(naive_freeze_span(trace, keys=("health", "velocity"), start=0), 1)

    def test_cannot_distinguish_real_hitstop_from_the_silent_hitstun_after_it(self):
        # MK2 arcade's measured shape: the struct changes ONCE on contact,
        # then reads identically frozen for the ENTIRE hitstun window (not
        # just the hitstop portion of it). A naive span-counter starting
        # right after the contact edge cannot tell where hitstop ends and
        # ordinary (but real) stun begins -- it reports the whole remaining
        # silence as one span.
        hitstop_frames, extra_silent_stun_frames = 6, 18
        frozen = {"health": 89, "velocity": 0}
        trace = (
            [{"health": 100, "velocity": 0}]
            + [dict(frozen) for _ in range(hitstop_frames + extra_silent_stun_frames)]
            + [{"health": 89, "velocity": 1}]  # fighter finally acts again
        )
        span = naive_freeze_span(trace, keys=("health", "velocity"), start=1)
        # Wrong by construction: reports the WHOLE quiet window (as a count
        # of frozen ADJACENT PAIRS, hence -1), not just the true hitstop
        # portion of it.
        self.assertEqual(span, hitstop_frames + extra_silent_stun_frames - 1)
        self.assertNotEqual(span, hitstop_frames)


# ── the sound differential comparison (pure) ────────────────────────────────


class ComputeHitstopTest(unittest.TestCase):
    def test_basic_difference(self):
        # contact at frame 8, attacker free 36 frames after contact (per
        # `first_true`'s own frame of reference); the same script, thrown
        # from anchor=2 (script.total_frames) and never connecting, frees
        # the attacker 30 frames after ITS anchor. hitstop = (8+36)-(2+30)=12.
        self.assertEqual(
            compute_hitstop(
                contact_frame=8, connecting_first_true=36,
                whiff_anchor=2, whiff_first_true=30,
            ),
            12,
        )

    def test_null_propagates_when_either_side_missing(self):
        self.assertIsNone(
            compute_hitstop(
                contact_frame=8, connecting_first_true=None,
                whiff_anchor=2, whiff_first_true=30,
            )
        )
        self.assertIsNone(
            compute_hitstop(
                contact_frame=8, connecting_first_true=36,
                whiff_anchor=2, whiff_first_true=None,
            )
        )

    def test_zero_hitstop_is_not_null(self):
        value = compute_hitstop(
            contact_frame=10, connecting_first_true=20, whiff_anchor=0, whiff_first_true=30,
        )
        self.assertEqual(value, 0)
        self.assertIsNotNone(value)


class CollapseHitstopTest(unittest.TestCase):
    def test_agreement_collapses_to_the_shared_value(self):
        self.assertEqual(collapse_hitstop({"a": 12, "b": 12}, where="x"), 12)

    def test_all_null_collapses_to_null(self):
        self.assertIsNone(collapse_hitstop({"a": None, "b": None}, where="x"))

    def test_a_null_observable_does_not_count_as_disagreement(self):
        self.assertEqual(collapse_hitstop({"a": 12, "b": None}, where="x"), 12)

    def test_real_disagreement_raises_rather_than_averaging(self):
        with self.assertRaises(CrossObservableHitstopError) as ctx:
            collapse_hitstop({"a": 12, "b": 13}, where="cell X")
        self.assertIn("§8.4", str(ctx.exception))
        self.assertIn("cell X", str(ctx.exception))


# ── the fake: FakeGame + an explicit, injectable freeze ────────────────────


class _HitstopFakeGame(FakeGame):
    """`FakeGame` (see `tests.test_framelab_probe`) plus a ground-truth
    hitstop the test controls directly, so the whiff-vs-connecting
    differencing method can be checked against a KNOWN answer -- including
    hit and block hitstop DIFFERING, which is the case the task brief warns
    against assuming away.

    Two behaviours the base fixture does not have, both needed here:

      1. `_land()` adds `hitstop_hit`/`hitstop_block` as an extra delay
         layered UNDER BOTH sides' existing recovery/stun clocks (base
         `FakeGame` applies recovery/stun the instant contact resolves,
         i.e. hitstop=0 baked in).
      2. `_advance()` arms the attacker's OWN recovery the moment the
         attack starts, win or miss -- required for a WHIFF replay (`reach`
         too small to ever land) to have a real "attacker becomes free"
         frame at all, which is what makes it usable as `R`.
    """

    def __init__(self, *, hitstop_hit: int = 0, hitstop_block: int = 0, **kw):
        super().__init__(**kw)
        self.hitstop_hit = hitstop_hit
        self.hitstop_block = hitstop_block

    def _advance(self) -> None:
        self.frame += 1
        self.gframe += 1
        self.anim += 1
        self.trail.append({p: self.held[p] for p in (0, 1)})

        for f, _ in list(self.pending_contacts):
            if f == self.gframe:
                self._land()
        self.pending_contacts = [c for c in self.pending_contacts if c[0] > self.gframe]

        a_eff = self._effective(0)
        if (
            self.ATTACK in a_eff
            and not self.attack_started
            and self.gframe >= self.free_at[0]
        ):
            self.attack_started = True
            # Whiff-path default: committed through startup+recovery even
            # if nothing ever lands (§1.2: hitstop only fires ON CONTACT).
            self.free_at[0] = self.gframe + self.STARTUP + self.recovery
            for i in range(self.hits):
                self.pending_contacts.append(
                    (self.gframe + self.STARTUP + i * self.hit_gap, i)
                )

        for p in (0, 1):
            if self.gframe < self.pushed_until[p]:
                self.x[p] += 3
                continue
            if self.gframe < self.free_at[p]:
                continue
            eff = self._effective(p)
            if "right" in eff:
                self.x[p] += 2
            elif "left" in eff:
                self.x[p] -= 2

    def _land(self) -> None:
        if abs(self.x[0] - self.x[1]) > self.reach:
            return
        guarding = self.GUARD in self._effective(1)
        self.contacts.append(self.gframe)
        self.health[1] -= 3 if guarding else 11
        hitstop = self.hitstop_block if guarding else self.hitstop_hit
        stun = self.block_stun if guarding else self.stun
        self.free_at[1] = self.gframe + hitstop + stun
        self.free_at[0] = self.gframe + hitstop + self.recovery
        self.pushed_until[1] = self.gframe + hitstop + self.pushback_frames


def _make_session(game: FakeGame) -> LabSession:
    s = LabSession(game, verify_fn=lambda _s: True, input_settle_s=0)
    s.enforce_preconditions()
    return s


def _rig() -> Rig:
    # The fake's `load_state` ignores its `path`/`spec` argument entirely
    # (ports reset to fixed positions regardless), so the arena string here
    # is a label, not a live reference -- exactly like `probe.py`'s own
    # `FakeGame`-based tests.
    return Rig(
        arena="fake.state", attacker_port=0, defender_port=1,
        guard_buttons=(FakeGame.GUARD,), walk_directions=("right",),
        quiet_frames=20,
    )


_SPEC = MoveSpec(name="poke", buttons=(FakeGame.ATTACK,), hold_frames=4)
_OBSERVABLES = ("x", "struct")
_LATENCIES = {"x": 1, "struct": 1}


class MeasureWhiffReferenceTest(unittest.TestCase):
    def test_raises_if_the_reference_arena_actually_connects(self):
        game = _HitstopFakeGame(reach=200)  # well within contact range
        session = _make_session(game)
        with self.assertRaises(WhiffNotAWhiffError):
            measure_whiff_reference(
                session, rig=_rig(), spec=_SPEC, contact_read=contact_read,
                observables=_OBSERVABLES, sample_fn=sampler(0),
                input_latency_frames=_LATENCIES,
            )

    def test_measures_the_intrinsic_recovery_length_with_no_contact(self):
        recovery, startup_gap = 18, 6
        game = _HitstopFakeGame(reach=10, recovery=recovery)  # far apart: whiffs
        session = _make_session(game)
        ref = measure_whiff_reference(
            session, rig=_rig(), spec=_SPEC, contact_read=contact_read,
            observables=_OBSERVABLES, sample_fn=sampler(0),
            input_latency_frames=_LATENCIES,
        )
        for o in _OBSERVABLES:
            self.assertIsNotNone(ref.first_true[o])
        # Reproducible: measuring it again from a fresh load gives the same
        # answer (docs/frames.md §7's re-measurement bar, at unit scale).
        game2 = _HitstopFakeGame(reach=10, recovery=recovery)
        session2 = _make_session(game2)
        ref2 = measure_whiff_reference(
            session2, rig=_rig(), spec=_SPEC, contact_read=contact_read,
            observables=_OBSERVABLES, sample_fn=sampler(0),
            input_latency_frames=_LATENCIES,
        )
        self.assertEqual(ref.first_true, ref2.first_true)


class MeasureHitstopEndToEndTest(unittest.TestCase):
    """The whole method, against a KNOWN ground truth injected into the
    fake -- including hit and block hitstop DIFFERING, which is the case
    the task explicitly warns against assuming away."""

    def _reference(self, **kw):
        whiff_game = _HitstopFakeGame(reach=10, **kw)  # never connects
        whiff_session = _make_session(whiff_game)
        return measure_whiff_reference(
            whiff_session, rig=_rig(), spec=_SPEC, contact_read=contact_read,
            observables=_OBSERVABLES, sample_fn=sampler(0),
            input_latency_frames=_LATENCIES,
        )

    def test_recovers_the_injected_hitstop_and_it_can_differ_by_outcome(self):
        common = dict(recovery=18, stun=24, block_stun=14, pushback=9)
        hitstop_hit, hitstop_block = 10, 15
        reference = self._reference(**common)

        connect_game = _HitstopFakeGame(
            reach=200, hitstop_hit=hitstop_hit, hitstop_block=hitstop_block, **common,
        )
        session = _make_session(connect_game)

        hit = measure_hitstop_outcome(
            session, rig=_rig(), spec=_SPEC, gap_px=None, contact_read=contact_read,
            observables=_OBSERVABLES, sample_fn=sampler(0),
            input_latency_frames=_LATENCIES, defender_guard=False, reference=reference,
        )
        self.assertEqual(hit.hitstop, hitstop_hit)
        for o in _OBSERVABLES:
            self.assertEqual(hit.per_observable[o], hitstop_hit)

        # A fresh connecting run for the BLOCK outcome (the fake's `_land`
        # is a one-shot per load, so this mirrors the real protocol's "two
        # separate runs, one per rig_guard_state").
        connect_game2 = _HitstopFakeGame(
            reach=200, hitstop_hit=hitstop_hit, hitstop_block=hitstop_block, **common,
        )
        session2 = _make_session(connect_game2)
        block = measure_hitstop_outcome(
            session2, rig=_rig(), spec=_SPEC, gap_px=None, contact_read=contact_read,
            observables=_OBSERVABLES, sample_fn=sampler(0),
            input_latency_frames=_LATENCIES, defender_guard=True, reference=reference,
        )
        self.assertEqual(block.hitstop, hitstop_block)
        self.assertNotEqual(hit.hitstop, block.hitstop)

    def test_zero_hitstop_round_trips_to_zero_not_null(self):
        common = dict(recovery=18, stun=24, block_stun=14, pushback=9)
        reference = self._reference(**common)
        game = _HitstopFakeGame(reach=200, hitstop_hit=0, hitstop_block=0, **common)
        session = _make_session(game)
        hit = measure_hitstop_outcome(
            session, rig=_rig(), spec=_SPEC, gap_px=None, contact_read=contact_read,
            observables=_OBSERVABLES, sample_fn=sampler(0),
            input_latency_frames=_LATENCIES, defender_guard=False, reference=reference,
        )
        self.assertEqual(hit.hitstop, 0)
        self.assertIsNotNone(hit.hitstop)

    def test_a_cell_with_only_a_block_outcome_reports_stored_from_block(self):
        # Mirrors a knockdown row (on_hit already NULL for a different
        # reason, §1.1): only `measure_block=True` is requested, and
        # `HitstopCell.stored` must fall back to the block value rather than
        # reporting NULL just because on_hit was never asked for.
        common = dict(recovery=18, stun=24, block_stun=14, pushback=9)
        reference = self._reference(**common)
        game = _HitstopFakeGame(reach=200, hitstop_hit=10, hitstop_block=15, **common)
        session = _make_session(game)
        cell = measure_hitstop_cell(
            session, rig=_rig(), spec=_SPEC, gap_px=None, contact_read=contact_read,
            observables=_OBSERVABLES, sample_fn=sampler(0), latencies={
                "attacker/hit": _LATENCIES, "attacker/block": _LATENCIES,
            },
            reference=reference, measure_hit=False, measure_block=True,
        )
        self.assertIsNone(cell.on_hit)
        self.assertEqual(cell.on_block.hitstop, 15)
        self.assertEqual(cell.stored, 15)
        self.assertIsNone(cell.agrees)  # only one outcome to compare

    def test_a_mismatched_whiff_reference_refuses_a_negative_hitstop(self):
        # A whiff reference measured against a DIFFERENT (longer) recovery
        # than the connecting move actually has -- the live-measured shape
        # of a proximity-variant mismatch (§5): the whiff describes a
        # different animation than the one that connected, so subtracting
        # them can come out negative, which is physically impossible
        # (hitstop only ADDS frames). Must refuse, never store the number.
        mismatched_reference = self._reference(recovery=40, stun=24, block_stun=14, pushback=9)
        connect_game = _HitstopFakeGame(
            reach=200, hitstop_hit=10, hitstop_block=10,
            recovery=10, stun=24, block_stun=14, pushback=9,
        )
        session = _make_session(connect_game)
        with self.assertRaises(HitstopError) as ctx:
            measure_hitstop_outcome(
                session, rig=_rig(), spec=_SPEC, gap_px=None, contact_read=contact_read,
                observables=_OBSERVABLES, sample_fn=sampler(0),
                input_latency_frames=_LATENCIES, defender_guard=False,
                reference=mismatched_reference,
            )
        self.assertIn("physically impossible", str(ctx.exception))


class MeasureHitstopOutcomeContactRequiredTest(unittest.TestCase):
    def test_raises_if_the_scripted_move_does_not_connect_here(self):
        common = dict(recovery=18, stun=24, block_stun=14, pushback=9)
        whiff_game = _HitstopFakeGame(reach=10, **common)
        whiff_session = _make_session(whiff_game)
        reference = measure_whiff_reference(
            whiff_session, rig=_rig(), spec=_SPEC, contact_read=contact_read,
            observables=_OBSERVABLES, sample_fn=sampler(0),
            input_latency_frames=_LATENCIES,
        )
        # Ask for a "connecting" measurement against an arena that ALSO
        # whiffs -- must refuse rather than report a fabricated hitstop.
        also_whiff_game = _HitstopFakeGame(reach=10, **common)
        session = _make_session(also_whiff_game)
        with self.assertRaises(HitstopError):
            measure_hitstop_outcome(
                session, rig=_rig(), spec=_SPEC, gap_px=None, contact_read=contact_read,
                observables=_OBSERVABLES, sample_fn=sampler(0),
                input_latency_frames=_LATENCIES, defender_guard=False, reference=reference,
            )


# ── planning which existing rows are eligible ──────────────────────────────


class PlanHitstopCellsTest(unittest.TestCase):
    def _row(self, **overrides):
        row = {
            "id": 1, "family": "mk2", "port": "arcade", "char": "reptile",
            "move": "HK", "variant": "far", "gap_walk_frames": 45, "gap_px": 72.0,
            "hitstop": None, "observable": "struct_velocity",
        }
        row.update(overrides)
        return row

    def test_close_variant_is_skipped_with_a_named_reason(self):
        eligible, skipped = plan_hitstop_cells([self._row(variant="close")])
        self.assertEqual(eligible, [])
        self.assertEqual(len(skipped), 1)
        self.assertIn("close-range", skipped[0]["skip_reason"])

    def test_already_measured_row_is_skipped_not_remeasured(self):
        eligible, skipped = plan_hitstop_cells([self._row(hitstop=12)])
        self.assertEqual(eligible, [])
        self.assertIn("already has", skipped[0]["skip_reason"])

    def test_far_variant_unmeasured_row_is_eligible(self):
        eligible, skipped = plan_hitstop_cells([self._row()])
        self.assertEqual(skipped, [])
        self.assertEqual(len(eligible), 1)

    def test_no_variant_unmeasured_row_is_eligible(self):
        eligible, skipped = plan_hitstop_cells([self._row(variant=None, move="cHK")])
        self.assertEqual(skipped, [])
        self.assertEqual(len(eligible), 1)


# ── FrameStore.update, the store change this task needed ──────────────────


def _valid_row(**overrides) -> dict:
    row = {
        "family": "mk2", "port": "arcade", "char": "reptile", "move": "HK",
        "observable": "struct_velocity", "method": "linear_sweep",
        "input_latency_frames": 1, "core_id": "core:sha256:deadbeef",
        "rom_id": "rom:sha256:cafef00d",
    }
    row.update(overrides)
    return row


class FrameStoreUpdateTest(unittest.TestCase):
    def test_fills_in_a_previously_null_measured_column(self):
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            rid = store.insert(_valid_row(hitstop=None))
            self.assertIsNone(store.get(rid)["hitstop"])
            store.update(rid, {"hitstop": 12})
            self.assertEqual(store.get(rid)["hitstop"], 12)

    def test_does_not_touch_measured_at_unless_asked(self):
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            rid = store.insert(_valid_row())
            before = store.get(rid)["measured_at"]
            store.update(rid, {"hitstop": 5})
            self.assertEqual(store.get(rid)["measured_at"], before)

    def test_refuses_to_touch_identifying_columns(self):
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            rid = store.insert(_valid_row())
            with self.assertRaises(ValueError):
                store.update(rid, {"char": "mileena"})

    def test_refuses_to_touch_provenance_columns(self):
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            rid = store.insert(_valid_row())
            with self.assertRaises(ValueError):
                store.update(rid, {"core_id": "different"})

    def test_refuses_unknown_column(self):
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            rid = store.insert(_valid_row())
            with self.assertRaises(ValueError):
                store.update(rid, {"bogus_column": 1})

    def test_raises_on_a_row_id_that_does_not_exist(self):
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            with self.assertRaises(ValueError):
                store.update(999, {"hitstop": 1})

    def test_zero_hitstop_survives_as_zero_not_null(self):
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            rid = store.insert(_valid_row())
            store.update(rid, {"hitstop": 0})
            row = store.get(rid)
            self.assertEqual(row["hitstop"], 0)
            self.assertIsNotNone(row["hitstop"])

    def test_no_op_on_empty_values(self):
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            rid = store.insert(_valid_row())
            store.update(rid, {})  # must not raise
            self.assertIsNone(store.get(rid)["hitstop"])


if __name__ == "__main__":
    unittest.main()
