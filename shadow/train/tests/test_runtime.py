from __future__ import annotations

import tempfile
import unittest
import warnings
from pathlib import Path

import numpy as np

from shadow_train import dataset
from shadow_train import runtime as rt
from shadow_train.dataset import ATTACK_CLASSES, MOVE_CLASSES

from .helpers import fighter, make_round_rows, write_jsonl


class IntentBitsRoundTripTest(unittest.TestCase):
    """§3c: intent (move, attack) -> RETRO mask, decoded back through
    dataset.py's OWN label-extraction functions (_move_class/_attack_class)
    must recover the original classes, for all 9 moves x both facings x all
    6 attacks -- the full matrix the harness spec calls for."""

    def test_full_matrix_round_trips(self):
        for move in range(len(MOVE_CLASSES)):
            for s in (1, -1):
                for attack in range(len(ATTACK_CLASSES)):
                    mask = rt.intent_to_mask(move, attack, s)
                    got_move = dataset._move_class(mask, s)
                    got_attack = dataset._attack_class(mask)
                    self.assertEqual(
                        got_move, move,
                        f"move mismatch: move={MOVE_CLASSES[move]} s={s} "
                        f"attack={ATTACK_CLASSES[attack]} mask={mask:012b} "
                        f"decoded={MOVE_CLASSES[got_move]}",
                    )
                    self.assertEqual(
                        got_attack, attack,
                        f"attack mismatch: move={MOVE_CLASSES[move]} s={s} "
                        f"attack={ATTACK_CLASSES[attack]} mask={mask:012b} "
                        f"decoded={ATTACK_CLASSES[got_attack]}",
                    )

    def test_spec_table_spot_checks(self):
        # Forward, s>0 -> Right bit (7); Forward, s<0 -> Left bit (6).
        self.assertEqual(rt.intent_to_mask(1, 0, 1), 1 << dataset.BIT_RIGHT)
        self.assertEqual(rt.intent_to_mask(1, 0, -1), 1 << dataset.BIT_LEFT)
        # Back is the mirror.
        self.assertEqual(rt.intent_to_mask(2, 0, 1), 1 << dataset.BIT_LEFT)
        self.assertEqual(rt.intent_to_mask(2, 0, -1), 1 << dataset.BIT_RIGHT)
        # Attacks: Light=B, Medium=A, Heavy=Y, Launcher=B+A, Toss=B+A+Y.
        self.assertEqual(rt.intent_to_mask(0, 1, 1), 1 << dataset.BIT_B)
        self.assertEqual(rt.intent_to_mask(0, 2, 1), 1 << dataset.BIT_A)
        self.assertEqual(rt.intent_to_mask(0, 3, 1), 1 << dataset.BIT_Y)
        self.assertEqual(
            rt.intent_to_mask(0, 4, 1), (1 << dataset.BIT_B) | (1 << dataset.BIT_A)
        )
        self.assertEqual(
            rt.intent_to_mask(0, 5, 1),
            (1 << dataset.BIT_B) | (1 << dataset.BIT_A) | (1 << dataset.BIT_Y),
        )
        # Never touch Select/Start.
        for move in range(len(MOVE_CLASSES)):
            for attack in range(len(ATTACK_CLASSES)):
                for s in (1, -1):
                    mask = rt.intent_to_mask(move, attack, s)
                    self.assertEqual(mask >> dataset.BIT_SELECT & 1, 0)
                    self.assertEqual(mask >> dataset.BIT_START & 1, 0)

    def test_mask_to_button_names(self):
        mask = (1 << dataset.BIT_RIGHT) | (1 << dataset.BIT_B)
        self.assertEqual(set(rt.mask_to_button_names(mask)), {"right", "b"})
        self.assertEqual(rt.intent_to_button_names(1, 1, 1), ["b", "right"])


class MemoryParsingTest(unittest.TestCase):
    """parse_fighter / parse_tick / is_controllable against synthetic byte
    blobs -- no live app needed."""

    def _fighter_blob(self, **vals) -> bytearray:
        blob = bytearray(rt.BLOCK1_LEN)
        for name, off, size in rt.FIGHTER_LAYOUT:
            v = vals.get(name, 0)
            blob[off:off + size] = v.to_bytes(size, "big")
        return blob

    def test_parse_fighter_roundtrip(self):
        blob = self._fighter_blob(x=200, y=216, facing=1, health=239, anim=12)
        f = rt.parse_fighter(bytes(blob))
        self.assertEqual(f["x"], 200)
        self.assertEqual(f["y"], 216)
        self.assertEqual(f["facing"], 1)
        self.assertEqual(f["health"], 239)
        self.assertEqual(f["anim"], 12)

    def test_parse_tick_and_controllable(self):
        b1 = self._fighter_blob(x=100, y=216, health=200, facing=1)
        b1[rt.COMBO_ON_B2_OFFSET] = 3
        b2 = self._fighter_blob(x=200, y=216, health=180, facing=0)
        b2[rt.COMBO_ON_B1_OFFSET] = 0
        blobs = {
            "block1": bytes(b1),
            "block2": bytes(b2),
            "match_end_abort": bytes(rt.MATCH_END_ABORT_LEN),
            "round_over": bytes(2),
            "clock": bytes([0, 0, 0, 0, 0x58]),
        }
        snap = rt.parse_tick(blobs)
        self.assertEqual(snap.block1["x"], 100)
        self.assertEqual(snap.block2["x"], 200)
        self.assertEqual(snap.combo_on_b2, 3)
        self.assertEqual(snap.combo_on_b1, 0)
        self.assertEqual(snap.round_over, 0)
        self.assertEqual(snap.abort, 0)
        self.assertEqual(snap.match_end, 0)
        self.assertEqual(snap.timer_bcd, 0x58)
        self.assertTrue(rt.is_controllable(snap))

    def test_not_controllable_when_health_out_of_range(self):
        b1 = self._fighter_blob(health=0)  # dead / not a live round
        b2 = self._fighter_blob(health=180)
        blobs = {
            "block1": bytes(b1), "block2": bytes(b2),
            "match_end_abort": bytes(rt.MATCH_END_ABORT_LEN),
            "round_over": bytes(2), "clock": bytes([0, 0, 0, 0, 0x58]),
        }
        self.assertFalse(rt.is_controllable(rt.parse_tick(blobs)))

    def test_not_controllable_when_timer_invalid(self):
        b1 = self._fighter_blob(health=200)
        b2 = self._fighter_blob(health=180)
        blobs = {
            "block1": bytes(b1), "block2": bytes(b2),
            "match_end_abort": bytes(rt.MATCH_END_ABORT_LEN),
            "round_over": bytes(2), "clock": bytes([0, 0, 0, 0, 0x00]),
        }
        self.assertFalse(rt.is_controllable(rt.parse_tick(blobs)))

    def test_abort_flag_blocks_controllable(self):
        b1 = self._fighter_blob(health=200)
        b2 = self._fighter_blob(health=180)
        me_blob = bytearray(rt.MATCH_END_ABORT_LEN)
        me_blob[rt.MATCH_END_ABORT_OFFSET:rt.MATCH_END_ABORT_OFFSET + 2] = (1).to_bytes(2, "big")
        blobs = {
            "block1": bytes(b1), "block2": bytes(b2),
            "match_end_abort": bytes(me_blob),
            "round_over": bytes(2), "clock": bytes([0, 0, 0, 0, 0x58]),
        }
        self.assertFalse(rt.is_controllable(rt.parse_tick(blobs)))


class AnchorTest(unittest.TestCase):
    def test_bot_is_the_larger_x_block(self):
        # Recorder anchors p1_block = smaller X (left = P1). The deploy bot
        # is P2, the mirror: the LARGER X block.
        self.assertEqual(rt.resolve_me_block(100, 200), "block2")
        self.assertEqual(rt.resolve_me_block(200, 100), "block1")

    def test_other_block(self):
        self.assertEqual(rt.other_block("block1"), "block2")
        self.assertEqual(rt.other_block("block2"), "block1")


class HitstunTrackerTest(unittest.TestCase):
    """Tick-granular equivalent of dataset._recent_change_mask (see the
    runtime.py module docstring for the frame->tick conversion)."""

    def test_boundary_aligned_change_matches_frame_level_threshold(self):
        # WINDOW_TICKS = HITSTUN_RECENT_FRAMES // P = 20 // 8 = 2.
        self.assertEqual(rt.WINDOW_TICKS, 2)
        tr = rt.HitstunTracker()
        vals = [0, 0, 2, 2, 2, 2]  # changes to 2 at tick index 2
        active = [tr.update(tick, v) for tick, v in enumerate(vals)]
        self.assertEqual(active, [False, False, True, True, True, False])

    def test_zero_never_active(self):
        tr = rt.HitstunTracker()
        for tick in range(10):
            self.assertFalse(tr.update(tick, 0))

    def test_reset_clears_state(self):
        tr = rt.HitstunTracker()
        tr.update(0, 5)
        tr.reset()
        self.assertFalse(tr.update(0, 5))  # first-seen value is never "changed"


class HoldFractionsTest(unittest.TestCase):
    def test_from_emitted_mask_not_class_label(self):
        right_mask = 1 << dataset.BIT_RIGHT
        left_mask = 1 << dataset.BIT_LEFT
        self.assertEqual(rt.hold_fractions(right_mask, s=1), (1.0, 0.0))
        self.assertEqual(rt.hold_fractions(right_mask, s=-1), (0.0, 1.0))
        self.assertEqual(rt.hold_fractions(left_mask, s=1), (0.0, 1.0))
        self.assertEqual(rt.hold_fractions(0, s=1), (0.0, 0.0))


class FeatureStackerTest(unittest.TestCase):
    def test_order_is_oldest_to_newest_and_matches_manual_concat(self):
        stacker = rt.FeatureStacker(k=3)
        scalars = [
            {name: float(i) for name, i in zip(dataset.SCALAR_FEATURES, range(len(dataset.SCALAR_FEATURES)))}
            for _ in range(3)
        ]
        # make them distinguishable
        vecs = []
        for i, s in enumerate(scalars):
            s2 = {k: v + i * 100 for k, v in s.items()}
            stacker.push(s2)
            vecs.append(rt.scalars_to_vector(s2))
        self.assertTrue(stacker.ready())
        expected = np.concatenate(vecs)
        np.testing.assert_array_equal(stacker.vector(), expected)

    def test_not_ready_until_k_pushes(self):
        stacker = rt.FeatureStacker(k=4)
        for _ in range(3):
            stacker.push({k: 0.0 for k in dataset.SCALAR_FEATURES})
            self.assertFalse(stacker.ready())
        stacker.push({k: 0.0 for k in dataset.SCALAR_FEATURES})
        self.assertTrue(stacker.ready())

    def test_reset(self):
        stacker = rt.FeatureStacker(k=2)
        stacker.push({k: 0.0 for k in dataset.SCALAR_FEATURES})
        stacker.push({k: 0.0 for k in dataset.SCALAR_FEATURES})
        self.assertTrue(stacker.ready())
        stacker.reset()
        self.assertFalse(stacker.ready())


class FeatureParityTest(unittest.TestCase):
    """The core drift guard (requirement 6): build a synthetic jsonl-v2
    round, compute the SPEC v2 scalar vector two ways -- dataset.py's batch
    pipeline (ground truth) and runtime.py's streaming build_scalars fed the
    same rows tick-by-tick -- and assert they match, decision for decision.

    Fighter fields step once per P-frame block (constant within a decision
    window) so that dataset's exact STALE=3-frame-old opponent read and
    runtime's "previous decision's opponent snapshot" approximation
    (module docstring #1) land on the exact same value -- letting this test
    assert bit-for-bit equality rather than an approximate match.
    """

    @staticmethod
    def _window_mask(n: int) -> int:
        # Held for the WHOLE window, like a deploy bot's press_buttons hold
        # (not a human's brief per-frame tap) -- see the class docstring:
        # this is what makes runtime.hold_fractions's binary approximation
        # exact rather than merely approximate for this test.
        return (1 << dataset.BIT_RIGHT) if n % 2 == 0 else 0

    def _build_stepped_round(self, n_decisions: int):
        """Round where block1 (me) and block2 (opp) fields both step to a
        new deterministic value at the start of every P-frame decision
        window, and combo_on_b1/b2 hitstun bursts land on decision
        boundaries (so the tick-quantized HitstunTracker matches the
        frame-level _recent_change_mask exactly -- see its own unit test)."""
        P = dataset.P
        rows = []
        # me = block1 (p1_block=1), so dataset's `me` reads current block1,
        # `opp` reads block2 STALE frames behind.
        #
        # Hitstun bursts: the value must CHANGE every window during the
        # burst (like a real combo's successive hit increments), not just
        # sit at one constant nonzero value for many windows -- a constant
        # value only counts as "active" within HITSTUN_RECENT_FRAMES=20
        # frames of when it last CHANGED (see dataset.py's
        # _recent_change_mask evidence comment), so a burst held unchanged
        # across more than ~2 decision windows (2*P=16 <= 20 < 3*P=24) would
        # go frame-level-inactive partway through window 3 while the
        # tick-quantized HitstunTracker (WINDOW_TICKS=2) is still catching
        # up -- an approximation gap, not a bug, but not what this parity
        # test is checking. Incrementing every window keeps "last changed"
        # continuously fresh so both trackers agree throughout the burst.
        combo_b1_hit_at = 3  # decision index at which block1 enters hitstun
        combo_b2_hit_at = 6  # decision index at which block2 enters hitstun
        burst_len = 3
        for n in range(n_decisions + 2):  # a couple extra windows of padding
            x1 = 100 + n * 2
            y1 = 216
            x2 = 220 - n * 2
            y2 = 216 if n % 5 != 2 else 200  # occasional jump
            anim1 = n * 3 % 64
            anim2 = n * 5 % 64
            timer1 = n * 7 % 256
            timer2 = n * 11 % 256
            health1 = 200 - n
            health2 = 180 - n
            c_b1 = (2 + n - combo_b1_hit_at) if combo_b1_hit_at <= n < combo_b1_hit_at + burst_len else 0
            c_b2 = (2 + n - combo_b2_hit_at) if combo_b2_hit_at <= n < combo_b2_hit_at + burst_len else 0
            p1_input = self._window_mask(n)
            for f in range(P):
                frame_idx = n * P + f
                rows.append({
                    "frame": frame_idx,
                    "round_id": 1,
                    "controllable": True,
                    "p1_block": 1,
                    "block1": fighter(x=x1, y=y1, facing=1, anim=anim1,
                                       timer=timer1, health=health1, meter=10,
                                       meter_max=100, char_id=0),
                    "block2": fighter(x=x2, y=y2, facing=0, anim=anim2,
                                       timer=timer2, health=health2, meter=20,
                                       meter_max=100, char_id=1),
                    "gate": {
                        "round_over": 0, "abort": 0, "match_end": 0,
                        "timer_bcd": 0x30, "demo_flag": 0,
                        "combo_on_b1": c_b1, "combo_on_b2": c_b2,
                        "credits": 8,
                    },
                    "p1_input": p1_input,
                    "p2_input": 0,
                })
        return rows

    def test_streaming_scalars_match_dataset_batch_scalars(self):
        n_decisions = 12
        rows = self._build_stepped_round(n_decisions)

        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "session.jsonl"
            write_jsonl(path, rows)
            # Ground truth: dataset.py's own per-decision Decision objects
            # (pre-stacking scalars), via its internal round grouping.
            (round_key, round_rows), = list(dataset._rounds(path))
            decisions = dataset._decisions_for_round(round_key, round_rows)

        # Streaming replay: feed runtime.py the *decision-boundary* raw
        # fighter dicts + combo values tick by tick, exactly like play.py's
        # main loop would after parsing MCP reads.
        buffers = rt.RoundBuffers()
        buffers.reset(me_block="block1")
        streamed = []
        P = dataset.P
        # Prime the "previous window" state from window 0 -- dataset's stale
        # opponent read for decision n=1 (i=P, STALE=3) lands at frame P-3,
        # still inside window 0, and its fwd/back-hold history (rows[0:P])
        # is exactly window 0's held mask. Neither is visible yet at the
        # first real decision without this priming step.
        buffers.prev_opp = rows[0]["block2"]
        buffers.prev_opp_combo = rows[0]["gate"]["combo_on_b2"]
        buffers.last_emitted_mask = self._window_mask(0)

        # dataset._decisions_for_round emits one decision per i=n*P for
        # n=1..floor((len(rows)-1)/P); with n_decisions+2 padding windows of
        # P rows each that's n=1..n_decisions+1.
        for n in range(1, n_decisions + 2):
            row = rows[n * P]  # the exact frame dataset.py reads at index i=n*P
            me = row["block1"]
            opp_now = row["block2"]
            s = 1 if me["facing"] == 1 else -1

            me_combo_now = row["gate"]["combo_on_b1"]
            opp_combo_now = row["gate"]["combo_on_b2"]

            opp_lagged = buffers.prev_opp
            opp_combo_lagged = buffers.prev_opp_combo

            fwd, back = rt.hold_fractions(buffers.last_emitted_mask, s)
            me_hitstun = buffers.me_hitstun.update(buffers.tick, me_combo_now)
            opp_hitstun = buffers.opp_hitstun.update(buffers.tick, opp_combo_lagged)

            scal = rt.build_scalars(me, opp_lagged, s, fwd, back, me_hitstun, opp_hitstun)
            streamed.append(scal)

            # This window's OWN held mask becomes "last emitted" for the
            # NEXT decision's hold-fraction read, exactly mirroring how
            # dataset.py's hist window (rows[i-P:i]) is THIS window once we
            # move one decision forward.
            buffers.last_emitted_mask = self._window_mask(n)
            buffers.prev_opp = opp_now
            buffers.prev_opp_combo = opp_combo_now
            buffers.tick += 1

        self.assertEqual(len(streamed), len(decisions))
        for n, (scal, dec) in enumerate(zip(streamed, decisions)):
            got = rt.scalars_to_vector(scal)
            want = dec.scalars
            np.testing.assert_allclose(
                got, want, atol=1e-6,
                err_msg=f"decision {n} scalar mismatch\ngot ={dict(zip(dataset.SCALAR_FEATURES, got))}\nwant={dict(zip(dataset.SCALAR_FEATURES, want))}",
            )


def _full_fighter(**overrides) -> dict:
    base = dict(x=100, y=216, anim=0, timer=0, health=200, meter=0, meter_max=100)
    base.update(overrides)
    return base


class PointerResolvedFieldsTest(unittest.TestCase):
    """B-rt: MK2 arcade's x/y (and any future pointer-resolved field) can be
    ABSENT from `me`/`opp` on a live tick when the pointer fails to resolve
    (never zero-filled -- RECORDER_V3.md §1.2 rule 1 / docs/frames.md §2.5).
    Live play can't "drop a decision" like dataset.py's offline fitter, so
    the chosen behaviour is: hold the previous action for a lone miss
    (build_scalars/compute_scalars returns None, never raises, never
    fabricates the missing coordinate), and escalate loudly (one
    warnings.warn) once a run of misses is long enough to be a broken
    session rather than a momentary hole. See the design note above
    PointerStaleness in runtime.py for the full reasoning."""

    def test_pointer_fields_from_meta_empty_by_default(self):
        self.assertEqual(rt.pointer_fields_from_meta({}), frozenset())

    def test_pointer_fields_from_meta_reads_declared_list(self):
        meta = {"pointer_resolved_fields": ["x", "y"]}
        self.assertEqual(rt.pointer_fields_from_meta(meta), frozenset({"x", "y"}))

    def test_stale_ticks_for_frames(self):
        self.assertEqual(rt.stale_ticks_for_frames(300, 8), 37)
        self.assertEqual(rt.stale_ticks_for_frames(1, 8), 1)  # max(1, ...) floor

    # ── build_scalars' guard ────────────────────────────────────────────
    def test_missing_declared_field_returns_none_not_raise(self):
        me = _full_fighter()
        del me["x"]
        opp = _full_fighter()
        scal = rt.build_scalars(
            me, opp, s=1, fwd_hold=0.0, back_hold=0.0,
            me_hitstun=False, opp_hitstun=False,
            pointer_fields=frozenset({"x"}),
        )
        self.assertIsNone(scal)

    def test_missing_declared_field_on_opponent_also_returns_none(self):
        me = _full_fighter()
        opp = _full_fighter()
        del opp["y"]
        scal = rt.build_scalars(
            me, opp, s=1, fwd_hold=0.0, back_hold=0.0,
            me_hitstun=False, opp_hitstun=False,
            pointer_fields=frozenset({"y"}),
        )
        self.assertIsNone(scal)

    def test_present_declared_field_computes_normally(self):
        me = _full_fighter(x=120)
        opp = _full_fighter(x=200)
        scal = rt.build_scalars(
            me, opp, s=1, fwd_hold=0.0, back_hold=0.0,
            me_hitstun=False, opp_hitstun=False,
            pointer_fields=frozenset({"x"}),
        )
        self.assertIsNotNone(scal)
        self.assertIn("dist_x", scal)

    def test_fast_path_default_matches_no_kwarg_call(self):
        """A model that declares NO pointer-resolved fields must take the
        existing path with zero behaviour change: the default (no
        pointer_fields at all) and an explicit empty frozenset produce
        byte-identical output."""
        me = _full_fighter(x=120)
        opp = _full_fighter(x=200)
        no_kwarg = rt.build_scalars(me, opp, 1, 0.5, 0.0, False, True)
        explicit_empty = rt.build_scalars(
            me, opp, 1, 0.5, 0.0, False, True, pointer_fields=frozenset()
        )
        self.assertEqual(no_kwarg, explicit_empty)

    def test_fast_path_does_not_guard_undeclared_absence(self):
        """The guard only ever looks at DECLARED fields -- an undeclared
        missing key (pointer_fields empty, the fast path every model uses
        today) must still surface exactly like it always has, not be
        silently swallowed into a None."""
        me = _full_fighter()
        del me["x"]
        opp = _full_fighter()
        with self.assertRaises(KeyError):
            rt.build_scalars(
                me, opp, s=1, fwd_hold=0.0, back_hold=0.0,
                me_hitstun=False, opp_hitstun=False,
            )

    # ── RoundBuffers.compute_scalars: hold + escalate ────────────────────
    def test_single_miss_holds_without_warning(self):
        buffers = rt.RoundBuffers(pointer_fields=frozenset({"x"}))
        me = _full_fighter()
        del me["x"]
        opp = _full_fighter()
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            scal = buffers.compute_scalars(me, opp, 1, 0.0, 0.0, False, False)
        self.assertIsNone(scal)
        self.assertEqual(caught, [])
        self.assertFalse(buffers.pointer_staleness.escalated)
        self.assertEqual(buffers.pointer_staleness.consecutive, 1)

    def test_recovery_resets_consecutive_count(self):
        buffers = rt.RoundBuffers(pointer_fields=frozenset({"x"}))
        bad_me = _full_fighter()
        del bad_me["x"]
        good_me = _full_fighter()
        opp = _full_fighter()
        with warnings.catch_warnings(record=True):
            warnings.simplefilter("always")
            buffers.compute_scalars(bad_me, opp, 1, 0.0, 0.0, False, False)
            buffers.compute_scalars(bad_me, opp, 1, 0.0, 0.0, False, False)
            self.assertEqual(buffers.pointer_staleness.consecutive, 2)
            scal = buffers.compute_scalars(good_me, opp, 1, 0.0, 0.0, False, False)
        self.assertIsNotNone(scal)
        self.assertEqual(buffers.pointer_staleness.consecutive, 0)
        self.assertFalse(buffers.pointer_staleness.escalated)

    def test_sustained_failure_escalates_exactly_once(self):
        threshold = 5
        buffers = rt.RoundBuffers(
            pointer_fields=frozenset({"x"}),
            pointer_staleness=rt.PointerStaleness(stale_after_ticks=threshold),
        )
        me = _full_fighter()
        del me["x"]
        opp = _full_fighter()
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            for i in range(threshold * 3):  # a long run, well past the threshold
                scal = buffers.compute_scalars(me, opp, 1, 0.0, 0.0, False, False)
                self.assertIsNone(scal)  # every tick holds -- never raises,
                                          # never fabricates a value
        runtime_warnings = [w for w in caught if issubclass(w.category, RuntimeWarning)]
        self.assertEqual(
            len(runtime_warnings), 1,
            "escalation must fire exactly once per stale episode, not every tick",
        )
        self.assertTrue(buffers.pointer_staleness.escalated)
        self.assertEqual(buffers.pointer_staleness.total_dropped, threshold * 3)

    def test_escalation_can_refire_after_a_fresh_stale_episode(self):
        threshold = 3
        buffers = rt.RoundBuffers(
            pointer_fields=frozenset({"x"}),
            pointer_staleness=rt.PointerStaleness(stale_after_ticks=threshold),
        )
        bad_me = _full_fighter()
        del bad_me["x"]
        good_me = _full_fighter()
        opp = _full_fighter()
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            for _ in range(threshold):
                buffers.compute_scalars(bad_me, opp, 1, 0.0, 0.0, False, False)
            buffers.compute_scalars(good_me, opp, 1, 0.0, 0.0, False, False)  # recover
            for _ in range(threshold):
                buffers.compute_scalars(bad_me, opp, 1, 0.0, 0.0, False, False)
        runtime_warnings = [w for w in caught if issubclass(w.category, RuntimeWarning)]
        self.assertEqual(len(runtime_warnings), 2, "a new stale episode after recovery re-warns")

    def test_round_reset_does_not_clear_pointer_staleness(self):
        """Pointer health is a SESSION property, not a round property: a
        round-start edge must not quietly hide an in-progress escalation."""
        threshold = 2
        buffers = rt.RoundBuffers(
            pointer_fields=frozenset({"x"}),
            pointer_staleness=rt.PointerStaleness(stale_after_ticks=threshold),
        )
        me = _full_fighter()
        del me["x"]
        opp = _full_fighter()
        with warnings.catch_warnings(record=True):
            warnings.simplefilter("always")
            for _ in range(threshold):
                buffers.compute_scalars(me, opp, 1, 0.0, 0.0, False, False)
        self.assertTrue(buffers.pointer_staleness.escalated)
        buffers.reset(me_block="block1")
        self.assertTrue(buffers.pointer_staleness.escalated)
        self.assertEqual(buffers.pointer_staleness.consecutive, threshold)

    def test_no_declared_fields_never_touches_the_guard(self):
        """A model with no pointer-resolved fields (the default, every model
        today) must behave identically to before this change existed:
        RoundBuffers.compute_scalars is just build_scalars, full stop."""
        buffers = rt.RoundBuffers()  # pointer_fields defaults to frozenset()
        me = _full_fighter(x=120)
        opp = _full_fighter(x=200)
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            scal = buffers.compute_scalars(me, opp, 1, 0.5, 0.0, False, True)
        self.assertEqual(
            scal,
            rt.build_scalars(me, opp, 1, 0.5, 0.0, False, True),
        )
        self.assertEqual(caught, [])
        self.assertFalse(buffers.pointer_staleness.escalated)


class CalibrationDriftTest(unittest.TestCase):
    def test_no_drift_when_meta_matches_live_constants(self):
        meta = {
            "feature_names": list(dataset.SCALAR_FEATURES),
            "calibration": {name: getattr(dataset, name) for name in rt.CALIBRATION_KEYS},
        }
        self.assertEqual(rt.check_calibration_drift(meta), [])

    def test_drift_detected_on_mismatch(self):
        meta = {
            "feature_names": list(dataset.SCALAR_FEATURES),
            "calibration": {name: getattr(dataset, name) for name in rt.CALIBRATION_KEYS},
        }
        meta["calibration"]["X_SCALE"] = 1.0  # wrong on purpose
        mismatches = rt.check_calibration_drift(meta)
        self.assertEqual(len(mismatches), 1)
        self.assertIn("X_SCALE", mismatches[0])

    def test_drift_detected_on_feature_name_mismatch(self):
        meta = {
            "feature_names": ["not", "the", "right", "list"],
            "calibration": {name: getattr(dataset, name) for name in rt.CALIBRATION_KEYS},
        }
        mismatches = rt.check_calibration_drift(meta)
        self.assertTrue(any("feature_names" in m for m in mismatches))


if __name__ == "__main__":
    unittest.main()
