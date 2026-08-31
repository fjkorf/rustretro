"""Unit tests for `framelab.replay` — measuring from RECORDED INPUT SLOTS,
and for the `executed_*` hardening in `framelab.session` that makes the
DIVERGED branch detectable at all (task G1).

Everything runs against `FakePlaybackGame`, a toy that models the three
things `src/playback.rs` actually does and that the classification tree turns
on:

  1. **The one-frame lead-in.** `playback::tick` runs AFTER `core.run()`, so
     slot index `i` is executed by emulated frame `i + 2`. The fake applies
     the mask in exactly that order, so `INPUT_OFFSET` is exercised rather
     than asserted.
  2. **`executed_*` vs `folded_*`.** `get_input` reports both, and the fake's
     `folded` deliberately tracks the CURRENT held set (which is what the
     real server does, and why `folded` cannot answer "what did that frame
     see"). Only `executed` is sticky.
  3. **Failure modes that are not the game's fault**: a playback that stops
     mid-transcript (DIVERGED), a move that never comes out (NO-EXECUTE), and
     a rig that does not reproduce (the determinism alarm).

The whole classification tree is also tested PURELY, against hand-built
`ReplayObservation`s, because a branch that can only be reached by arranging
an emulator is a branch nobody re-checks.
"""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from shadow_train.framelab.replay import (
    DIVERGED,
    INPUT_OFFSET,
    NO_EXECUTE,
    ON_TIME,
    RETIMED,
    WHIFF,
    DeterminismAlarm,
    InputSlot,
    ReplayError,
    ReplayLedger,
    ReplayObservation,
    ReplayOrigin,
    ReplayOriginError,
    buttons_from_mask,
    classify_replay,
    determinism_check,
    establish_origin,
    measure_replay,
    run_replay,
)
from shadow_train.framelab.session import (
    ExecutedInputError,
    LabSession,
    confirm_executed,
    executed_input,
)


HP = 1 << 1  # "y" — `record::pack_mask` bit order, same as JOYPAD_NAMES


# ── the fake ──────────────────────────────────────────────────────────────


class FakePlaybackGame:
    """A toy with an input slot, a contact signal and an attack signal.

    Contact fires `startup` frames after the attack's input frame, but only
    when `gap <= reach` — that is the whole spacing story the ladder demo
    exercises, reduced to one comparison. `startup_by_gap` lets a test make
    the SAME transcript contact at a different frame from a different state,
    which is the RETIMED case.
    """

    def __init__(
        self,
        *,
        slot_frames,
        gap: int = 70,
        reach: int = 80,
        startup: int = 10,
        startup_by_gap=None,
        attack_delay: int = 3,
        stop_playback_at=None,
        jitter_frames=(),
        report_executed: bool = True,
    ):
        self.slot_frames = list(slot_frames)
        self.gap = gap
        self.reach = reach
        self.startup = startup
        self.startup_by_gap = dict(startup_by_gap or {})
        self.attack_delay = attack_delay
        self.stop_playback_at = stop_playback_at
        self.jitter_frames = set(jitter_frames)
        self.report_executed = report_executed

        self.writes_enabled = False
        self.paused = False
        self.frame = 0
        self.loads = 0
        self.run_count = 0
        self.calls: list = []
        self._reset()

    # ── per-run state ────────────────────────────────────────────────────
    def _reset(self) -> None:
        self.gframe = 0
        self.held = {0: (), 1: ()}
        self.executed = {0: (), 1: ()}
        self.hp2 = 161
        self.ac = 160
        self.jitter = 0
        self.playing = False
        self.play_ports = (0, 1)
        self.idx = 0
        self.attack_at = None
        self.contact_at = None

    def _startup(self) -> int:
        return self.startup_by_gap.get(self.gap, self.startup)

    def _advance(self) -> None:
        self.frame += 1
        self.gframe += 1
        # (a) the fold: what THIS frame executes is the current held set.
        self.executed = dict(self.held)
        # (b) the frame runs.
        if "y" in self.executed[0] and self.attack_at is None:
            self.attack_at = self.gframe + self.attack_delay
            if self.gap <= self.reach:
                self.contact_at = self.gframe + self._startup()
        if self.attack_at == self.gframe:
            self.ac = 192
        if self.contact_at == self.gframe:
            self.hp2 -= 11
        if self.gframe in self.jitter_frames:
            # A field that is NOT a function of the save state: it differs
            # per replay, which is what a determinism alarm must catch.
            self.jitter = self.run_count
        # (c) playback::tick, at the END of the frame -> next frame sees it.
        if self.playing:
            if self.stop_playback_at == self.gframe:
                self.playing = False
                self.held = {0: (), 1: ()}
            elif self.idx < len(self.slot_frames):
                masks = self.slot_frames[self.idx]
                self.idx += 1
                for p in self.play_ports:
                    self.held[p] = tuple(sorted(buttons_from_mask(masks[p])))

    # ── transport ────────────────────────────────────────────────────────
    def call(self, tool: str, **kwargs):
        self.calls.append((tool, dict(kwargs)))
        h = getattr(self, f"_tool_{tool}", None)
        if h is None:
            raise AssertionError(f"unexpected/banned MCP tool called: {tool!r}")
        return h(**kwargs)

    def _tool_get_state(self):
        return {"frame_count": self.frame, "paused": self.paused}

    def _tool_enable_writes(self):
        self.writes_enabled = True
        return {"ok": True}

    def _tool_run_lua(self, script: str):
        if "training.enabled()" in script:
            return {"ok": True, "output": "false"}
        if "shadow.on()" in script:
            return {"ok": True, "output": "ok"}
        return {"ok": True, "output": "ok"}

    def _tool_pause(self):
        self.paused = True
        return {"ok": True}

    def _tool_resume(self):
        self.paused = False
        return {"ok": True}

    def _tool_load_state(self, slot=None, path=None, pause_after=False):
        if not self.writes_enabled:
            return {"error": "writes are locked; call enable_writes first"}
        self.loads += 1
        self.run_count += 1
        self._reset()
        self.frame += 1
        # Mirrors `src/mcp/server.rs::state_op_roundtrip`: a successful load
        # with `pause_after=True` forces `paused` in the same response.
        if pause_after:
            self.paused = True
        return {"ok": True, "op": "load", "paused": self.paused}

    def _tool_hold_buttons(self, buttons, port=0):
        self.held[port] = tuple(buttons)
        return {"ok": True}

    def _tool_release_buttons(self, buttons=None, port=0):
        self.held[port] = ()
        return {"ok": True}

    def _tool_get_input(self, port=0):
        held = "|".join(sorted(self.held[port]))
        out = {
            "ok": True,
            "port": port,
            # `folded` tracks the CURRENT held set, exactly as the real server
            # does -- which is why it cannot answer "what did that frame see".
            "asserted_mask": held,
            "folded_mask": held,
            "folded_buttons": sorted(self.held[port]),
        }
        if self.report_executed:
            out["executed_buttons"] = sorted(self.executed[port])
            out["executed_mask"] = "|".join(sorted(self.executed[port]))
        return out

    def _tool_step(self):
        self._advance()
        return {"ok": True, "landed": True, "frame_count": self.frame}

    def _tool_run_frames(self, count, port0=None, port1=None):
        if not self.paused:
            return {"ok": False, "error": "run_frames requires the emulator paused"}
        for _ in range(count):
            self._advance()
        return {"ok": True, "landed": count, "all_landed": True, "end_frame": self.frame}

    def _tool_play_inputs(self, action, name=None, port="both", trigger="manual"):
        if not self.writes_enabled:
            return {"error": "writes are locked"}
        if action == "start":
            if self.playing:
                return {"ok": False, "error": "a playback is already active"}
            self.playing = True
            self.play_ports = {"p1": (0,), "p2": (1,), "both": (0, 1)}[port]
            self.idx = 0
            return {"ok": True, "name": name, "frames": len(self.slot_frames)}
        if not self.playing:
            return {"ok": False, "error": "no playback active"}
        self.playing = False
        self.held = {0: (), 1: ()}
        return {"ok": True, "stopped": True}


def make_session(game, **kw) -> LabSession:
    s = LabSession(game, verify_fn=lambda _s: True, input_settle_s=0, **kw)
    s.enforce_preconditions()
    return s


def make_slot(frames, name="fake-slot") -> InputSlot:
    return InputSlot(name=name, family="mk2", port="arcade", frames=tuple(frames))


# A transcript that holds HP on its first two frames and nothing after —
# the exact shape of the live Reptile HP slot.
HP_FRAMES = [(HP, 0), (HP, 0)] + [(0, 0)] * 30


def contact_read(session):
    return session.client.hp2


def attack_read(session):
    return session.client.ac


def state_read(session):
    g = session.client
    return (g.hp2, g.ac, g.jitter)


# ── slot decoding ─────────────────────────────────────────────────────────


class InputSlotTest(unittest.TestCase):
    def test_mask_decoding_matches_the_joypad_bit_order(self):
        # `record::pack_mask` bit i == RETRO_DEVICE_ID_JOYPAD index i, and
        # `mcp/server.rs::JOYPAD_NAMES` is the inverse table.
        self.assertEqual(buttons_from_mask(0), frozenset())
        self.assertEqual(buttons_from_mask(1 << 1), frozenset({"y"}))
        self.assertEqual(
            buttons_from_mask((1 << 0) | (1 << 7) | (1 << 11)),
            frozenset({"b", "right", "r"}),
        )

    def test_expected_input_is_none_outside_the_transcripts_own_span(self):
        slot = make_slot(HP_FRAMES)
        # Before the first applied mask the port holds whatever preceded the
        # replay; after the last one `playback::tick` releases a tick late.
        # Both are UNCONSTRAINED -- calling them "expected empty" would
        # manufacture a DIVERGED out of documented boundary behaviour.
        self.assertIsNone(slot.executed_expected(0, 0))
        self.assertIsNone(slot.executed_expected(INPUT_OFFSET - 1, 0))
        self.assertEqual(slot.executed_expected(INPUT_OFFSET, 0), frozenset({"y"}))
        self.assertIsNone(slot.executed_expected(INPUT_OFFSET + len(slot), 0))

    def test_load_reads_a_real_slot_file_and_refuses_a_foreign_family(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / "mk2").mkdir()
            (root / "mk2" / "s.slot.json").write_text(
                json.dumps({"version": 1, "family": "mk2", "port": "arcade",
                            "created_at": 7, "frames": [[HP, 0], [0, 0]]})
            )
            slot = InputSlot.load("mk2", "s", root=str(root))
            self.assertEqual(len(slot), 2)
            self.assertEqual(slot.frames[0], (HP, 0))

            (root / "asurabld").mkdir()
            (root / "asurabld" / "s.slot.json").write_text(
                json.dumps({"version": 1, "family": "mk2", "frames": []})
            )
            with self.assertRaises(ReplayError):
                InputSlot.load("asurabld", "s", root=str(root))

    def test_a_missing_slot_names_the_path_instead_of_raising_oserror(self):
        with tempfile.TemporaryDirectory() as td:
            with self.assertRaises(ReplayError) as ctx:
                InputSlot.load("mk2", "nope", root=td)
            self.assertIn("nope", str(ctx.exception))


# ── the classification tree, purely ───────────────────────────────────────


def observation(**kw) -> ReplayObservation:
    base = dict(
        slot="s", arena="a.state", port="p1", frames=60,
        input_offset=INPUT_OFFSET, contact_trace=tuple(range(61)),
        contact_frames=(), attack_frames=(), executed=(),
    )
    base.update(kw)
    return ReplayObservation(**base)


class ClassificationTreeTest(unittest.TestCase):
    def test_diverged_produces_no_row_and_no_anchor(self):
        m = classify_replay(
            observation(
                input_divergence_frame=4,
                input_divergence_note="frame 4: port 0 executed [] not ['y']",
                contact_frames=(12,),
                attack_frames=(5,),
            ),
            expected_contact=12,
        )
        self.assertEqual(m.classification, DIVERGED)
        self.assertFalse(m.produces_row)
        self.assertTrue(m.is_refusal)
        self.assertIsNone(m.observed_contact)
        # It had a contact edge and an attack edge; neither rescues it. The
        # transcript diverged before the game ever got a say.
        with self.assertRaises(ReplayError):
            m.anchor()

    def test_no_execute_beats_whiff_when_the_move_never_came_out(self):
        m = classify_replay(observation(attack_frames=()), expected_contact=12)
        self.assertEqual(m.classification, NO_EXECUTE)
        self.assertFalse(m.produces_row)
        self.assertTrue(m.is_refusal)

    def test_whiff_is_a_result_not_a_refusal(self):
        m = classify_replay(
            observation(attack_frames=(5,), contact_frames=()), expected_contact=12
        )
        self.assertEqual(m.classification, WHIFF)
        self.assertFalse(m.produces_row)   # §1.1: a whiff has no advantage number
        self.assertFalse(m.is_refusal)     # ...but something WAS measured
        self.assertEqual(m.hits, 0)

    def test_no_attack_signal_configured_collapses_into_whiff_and_says_so(self):
        m = classify_replay(
            observation(attack_frames=(), contact_frames=()),
            expected_contact=12,
            require_attack_signal=False,
        )
        self.assertEqual(m.classification, WHIFF)
        self.assertIn("not distinguishable", m.note)

    def test_on_time_anchors_on_the_observed_frame(self):
        m = classify_replay(
            observation(attack_frames=(5,), contact_frames=(12,)), expected_contact=12
        )
        self.assertEqual(m.classification, ON_TIME)
        self.assertTrue(m.produces_row)
        self.assertEqual(m.contact_delta, 0)
        self.assertEqual(m.anchor().contact_frame, 12)

    def test_retimed_anchors_on_OBSERVED_contact_and_records_the_delta(self):
        m = classify_replay(
            observation(attack_frames=(5,), contact_frames=(9,)), expected_contact=12
        )
        self.assertEqual(m.classification, RETIMED)
        self.assertTrue(m.produces_row, "RETIMED is a valid measurement, not an error")
        self.assertEqual(m.observed_contact, 9)
        self.assertEqual(m.contact_delta, -3)
        # THE rule: the anchor is the OBSERVED frame, never the expected one.
        self.assertEqual(m.anchor().contact_frame, 9)
        self.assertNotEqual(m.anchor().contact_frame, m.expected_contact)

    def test_multi_hit_anchors_on_the_last_contact_of_the_first_cluster(self):
        m = classify_replay(
            observation(attack_frames=(5,), contact_frames=(12, 15, 40)),
            expected_contact=12,
            quiet_frames=20,
        )
        self.assertEqual(m.observed_contact, 15)   # §4.1's clustering rule
        self.assertEqual(m.hits, 2)
        self.assertEqual(m.contact_delta, 3)

    def test_the_origin_run_defines_the_expectation_rather_than_comparing(self):
        m = classify_replay(
            observation(attack_frames=(5,), contact_frames=(12,)), expected_contact=None
        )
        self.assertEqual(m.classification, ON_TIME)
        self.assertIsNone(m.contact_delta)
        self.assertIn("DEFINES", m.note)

    def test_provenance_distinguishes_a_replay_row_from_a_script_row(self):
        m = classify_replay(
            observation(attack_frames=(5,), contact_frames=(9,)),
            expected_contact=12,
            origin_arena="origin.state",
        )
        p = m.provenance()
        self.assertEqual(p["move_source"], "replay")
        self.assertEqual(p["replay_slot"], "s")
        self.assertEqual(p["replay_classification"], RETIMED)
        self.assertEqual(p["replay_origin_arena"], "origin.state")
        self.assertEqual(p["replay_observed_contact"], 9)
        self.assertEqual(p["replay_contact_delta"], -3)


# ── the ledger ────────────────────────────────────────────────────────────


class LedgerTest(unittest.TestCase):
    def test_refusals_are_counted_even_though_they_produce_no_row(self):
        led = ReplayLedger()
        led.record(classify_replay(
            observation(input_divergence_frame=3), expected_contact=12))
        led.record(classify_replay(observation(attack_frames=()), expected_contact=12))
        led.record(classify_replay(
            observation(attack_frames=(5,), contact_frames=()), expected_contact=12))
        led.record(classify_replay(
            observation(attack_frames=(5,), contact_frames=(9,)), expected_contact=12))

        self.assertEqual(led.counts[DIVERGED], 1)
        self.assertEqual(led.counts[NO_EXECUTE], 1)
        self.assertEqual(led.counts[WHIFF], 1)
        self.assertEqual(led.counts[RETIMED], 1)
        self.assertEqual(len(led.rows), 1)
        self.assertEqual(len(led.refusals), 2)
        # §7's "no silent caps": every bucket renders, including the zeros.
        rendered = led.render()
        for c in (DIVERGED, NO_EXECUTE, WHIFF, ON_TIME, RETIMED):
            self.assertIn(c, rendered)
        self.assertFalse(led.suspect)


# ── driving a real (fake) replay ──────────────────────────────────────────


class RunReplayTest(unittest.TestCase):
    def test_the_transcript_executes_at_INPUT_OFFSET_and_is_verified(self):
        game = FakePlaybackGame(slot_frames=HP_FRAMES, gap=70)
        session = make_session(game)
        obs = run_replay(
            session, slot=make_slot(HP_FRAMES), arena="a.state", total_frames=45,
            contact_read=contact_read, attack_read=attack_read, port="p1",
        )
        self.assertTrue(obs.input_matched)
        self.assertTrue(obs.executed_available)
        self.assertEqual(obs.executed[INPUT_OFFSET], frozenset({"y"}))
        self.assertEqual(obs.attack_frames, (INPUT_OFFSET + 3,))
        self.assertEqual(obs.contact_frames, (INPUT_OFFSET + 10,))

    def test_a_playback_that_stops_mid_transcript_is_DIVERGED_not_a_whiff(self):
        # The failure that has nothing to do with the move: the transcript
        # stopped running. Without `executed_*` this is invisible -- the game
        # simply never gets hit, and a naive rig would call it a WHIFF and
        # store "this move does not reach", which is false.
        long_frames = [(HP, 0)] * 20
        game = FakePlaybackGame(
            slot_frames=long_frames, gap=70, stop_playback_at=5, startup=10
        )
        session = make_session(game)
        obs = run_replay(
            session, slot=make_slot(long_frames), arena="a.state", total_frames=45,
            contact_read=contact_read, attack_read=attack_read, port="p1",
        )
        self.assertIsNotNone(obs.input_divergence_frame)
        m = classify_replay(obs, expected_contact=12)
        self.assertEqual(m.classification, DIVERGED)
        self.assertFalse(m.produces_row)

    def test_a_server_without_executed_buttons_reports_that_it_did_not_look(self):
        game = FakePlaybackGame(slot_frames=HP_FRAMES, gap=70, report_executed=False)
        session = make_session(game)
        obs = run_replay(
            session, slot=make_slot(HP_FRAMES), arena="a.state", total_frames=45,
            contact_read=contact_read, attack_read=attack_read, port="p1",
        )
        self.assertFalse(obs.executed_available)
        self.assertIn("NOT checked", obs.input_divergence_note)
        # "We did not look" must never read as "it matched": no DIVERGED is
        # claimed, and the note says the check was unavailable.
        self.assertIsNone(obs.input_divergence_frame)

    def test_replaying_a_p1_transcript_onto_p2_is_NO_EXECUTE(self):
        # A real operator mistake, and exactly the shape NO-EXECUTE names: the
        # transcript ran clean (p2's masks are all empty and all matched), but
        # the move never came out.
        game = FakePlaybackGame(slot_frames=HP_FRAMES, gap=70)
        session = make_session(game)
        origin = ReplayOrigin(
            slot="fake-slot", arena="a.state", port="p1",
            expected_contact=12, hits=1, input_offset=INPUT_OFFSET,
        )
        m = measure_replay(
            session, slot=make_slot(HP_FRAMES), arena="a.state", origin=origin,
            total_frames=45, contact_read=contact_read, attack_read=attack_read,
            port="p2",
        )
        self.assertEqual(m.classification, NO_EXECUTE)


class LadderTest(unittest.TestCase):
    """The whole point: one transcript, several starting states."""

    def _session(self, **kw):
        game = FakePlaybackGame(
            slot_frames=HP_FRAMES,
            gap=70,
            reach=80,
            startup_by_gap={70: 10, 62: 7},
            **kw,
        )
        return game, make_session(game)

    def test_one_slot_across_three_arenas_retimes_whiffs_and_stays_on_time(self):
        game, session = self._session()
        slot = make_slot(HP_FRAMES)
        led = ReplayLedger()
        origin = establish_origin(
            session, slot=slot, arena="gap-45.state", total_frames=45,
            contact_read=contact_read, attack_read=attack_read, port="p1",
        )
        self.assertEqual(origin.expected_contact, INPUT_OFFSET + 10)

        on_time = measure_replay(
            session, slot=slot, arena="gap-45.state", origin=origin,
            total_frames=45, contact_read=contact_read, attack_read=attack_read,
            ledger=led,
        )
        self.assertEqual(on_time.classification, ON_TIME)

        game.gap = 62          # closer: the same transcript connects EARLIER
        retimed = measure_replay(
            session, slot=slot, arena="gap-60.state", origin=origin,
            total_frames=45, contact_read=contact_read, attack_read=attack_read,
            ledger=led,
        )
        self.assertEqual(retimed.classification, RETIMED)
        self.assertEqual(retimed.contact_delta, -3)
        self.assertEqual(retimed.anchor().contact_frame, origin.expected_contact - 3)

        game.gap = 180         # too far: the same transcript reaches nothing
        whiff = measure_replay(
            session, slot=slot, arena="gap-0.state", origin=origin,
            total_frames=45, contact_read=contact_read, attack_read=attack_read,
            ledger=led,
        )
        self.assertEqual(whiff.classification, WHIFF)

        self.assertEqual(led.counts[ON_TIME], 1)
        self.assertEqual(led.counts[RETIMED], 1)
        self.assertEqual(led.counts[WHIFF], 1)
        self.assertEqual(len(led.rows), 2)

    def test_an_origin_that_whiffs_at_its_own_arena_is_refused(self):
        game, session = self._session()
        game.gap = 180
        with self.assertRaises(ReplayOriginError) as ctx:
            establish_origin(
                session, slot=make_slot(HP_FRAMES), arena="gap-0.state",
                total_frames=45, contact_read=contact_read, attack_read=attack_read,
                port="p1",
            )
        self.assertIn("no expected contact frame", str(ctx.exception))


# ── determinism: a SYSTEM ALARM, not a measurement result ─────────────────


class DeterminismTest(unittest.TestCase):
    def test_two_identical_replays_pass_and_are_not_an_alarm(self):
        game = FakePlaybackGame(slot_frames=HP_FRAMES, gap=70)
        session = make_session(game)
        rep = determinism_check(
            session, slot=make_slot(HP_FRAMES), arena="a.state", total_frames=45,
            state_read=state_read, contact_read=contact_read, port="p1",
            scope="toy-struct",
        )
        self.assertTrue(rep.identical)
        self.assertFalse(rep.alarm)
        self.assertIsNone(rep.first_divergence_frame)
        self.assertIs(rep.raise_if_alarm(), rep)
        self.assertIn("toy-struct", str(rep))

    def test_a_nonreproducible_rig_is_an_alarm_and_escalates_separately(self):
        # `jitter` is a field that is not a function of the save state, so the
        # two replays differ although the CONTACT frames agree. That is the
        # dangerous shape: the measurement looks fine and the rig is not.
        game = FakePlaybackGame(slot_frames=HP_FRAMES, gap=70, jitter_frames=(6,))
        session = make_session(game)
        rep = determinism_check(
            session, slot=make_slot(HP_FRAMES), arena="a.state", total_frames=45,
            state_read=state_read, contact_read=contact_read, port="p1",
        )
        self.assertFalse(rep.identical)
        self.assertTrue(rep.alarm)
        self.assertEqual(rep.first_divergence_frame, 6)
        self.assertEqual(rep.contact_a, rep.contact_b)
        self.assertIn("SYSTEM ALARM", str(rep))
        # Not a `ReplayError`: a caller must not be able to handle this as
        # "this cell failed".
        with self.assertRaises(DeterminismAlarm):
            rep.raise_if_alarm()
        self.assertNotIsInstance(rep, ReplayError)

    def test_an_alarm_makes_the_whole_ledger_suspect_including_clean_rows(self):
        game = FakePlaybackGame(slot_frames=HP_FRAMES, gap=70, jitter_frames=(6,))
        session = make_session(game)
        led = ReplayLedger()
        led.note_determinism(determinism_check(
            session, slot=make_slot(HP_FRAMES), arena="a.state", total_frames=45,
            state_read=state_read, port="p1",
        ))
        led.record(classify_replay(
            observation(attack_frames=(5,), contact_frames=(12,)), expected_contact=12))
        self.assertEqual(led.counts[ON_TIME], 1)
        self.assertTrue(led.suspect)
        self.assertIn("SUSPECT", led.render())
        self.assertTrue(led.summary()["suspect"])

    def test_two_scopes_can_disagree_and_both_verdicts_survive(self):
        # A wide trace can alarm on a field no measurement reads while the
        # measured observables are clean. Keeping only the last verdict would
        # let the all-clear overwrite the alarm.
        game = FakePlaybackGame(slot_frames=HP_FRAMES, gap=70, jitter_frames=(6,))
        session = make_session(game)
        led = ReplayLedger()
        led.note_determinism(determinism_check(
            session, slot=make_slot(HP_FRAMES), arena="a.state", total_frames=45,
            state_read=state_read, port="p1", scope="wide"))
        led.note_determinism(determinism_check(
            session, slot=make_slot(HP_FRAMES), arena="a.state", total_frames=45,
            state_read=lambda s: s.client.hp2, port="p1", scope="measured"))
        self.assertEqual(led.summary()["determinism"], {"wide": False, "measured": True})
        self.assertTrue(led.suspect)


# ── task G5: `pause_after` adoption (docs/frames.md §4.6) ─────────────────


class PauseAfterAdoptionTest(unittest.TestCase):
    """`LabSession.load_state` (and every replay path that goes through it)
    must request the atomic `pause_after=True` load and must never bracket a
    load with the plain `resume`/`pause` tools -- that bracket is exactly the
    defect §4.6 measured (a variable free-frame count inside the old
    resume-load-poll-pause window), and its residual hazard (`pause_after`
    landing right after an old-style plain `pause()` can pick up one stray
    frame) means the fix is to stop calling `resume`/`pause` near a load at
    all, not to narrow the window."""

    def test_load_state_requests_pause_after_and_never_brackets_with_resume_pause(self):
        game = FakePlaybackGame(slot_frames=HP_FRAMES, gap=70)
        session = make_session(game)
        session.load_state("a.state")
        session.load_state("a.state")  # a second, back-to-back load
        load_calls = [(name, kw) for name, kw in game.calls if name == "load_state"]
        self.assertEqual(len(load_calls), 2)
        for _, kwargs in load_calls:
            self.assertIs(kwargs.get("pause_after"), True)
        tool_names = [name for name, _ in game.calls]
        self.assertNotIn("resume", tool_names)
        self.assertNotIn("pause", tool_names)

    def test_run_replay_never_brackets_its_load_with_resume_pause(self):
        game = FakePlaybackGame(slot_frames=HP_FRAMES, gap=70)
        session = make_session(game)
        run_replay(
            session, slot=make_slot(HP_FRAMES), arena="a.state", total_frames=10,
            contact_read=contact_read, attack_read=attack_read, port="p1",
        )
        load_calls = [(name, kw) for name, kw in game.calls if name == "load_state"]
        self.assertGreaterEqual(len(load_calls), 1)
        for _, kwargs in load_calls:
            self.assertIs(kwargs.get("pause_after"), True)
        tool_names = [name for name, _ in game.calls]
        self.assertNotIn("resume", tool_names)
        self.assertNotIn("pause", tool_names)


# ── the `executed_*` hardening in session.py ──────────────────────────────


class ExecutedInputTest(unittest.TestCase):
    def test_executed_reports_what_the_frame_that_ran_saw_not_the_held_set(self):
        game = FakePlaybackGame(slot_frames=HP_FRAMES, gap=70)
        session = make_session(game)
        session.load_state("a.state")
        session.set_held(0, ["right"])
        # No frame has run under the new hold yet: `folded` already agrees
        # with `asserted` (that is what the fold oracle waits for), but
        # `executed` still reports the PREVIOUS frame -- which is the whole
        # difference between the two readings.
        self.assertEqual(session.executed(0), frozenset())
        session.step()
        self.assertEqual(session.executed(0), frozenset({"right"}))

    def test_confirm_executed_raises_when_the_frame_ran_on_the_wrong_input(self):
        game = FakePlaybackGame(slot_frames=HP_FRAMES, gap=70)
        session = make_session(game)
        session.load_state("a.state")
        session.set_held(0, ["right"])
        with self.assertRaises(ExecutedInputError) as ctx:
            confirm_executed(game, 0, ["right"], where="unit test")
        self.assertIn("§3.6", str(ctx.exception))
        session.step()
        self.assertEqual(confirm_executed(game, 0, ["right"]), frozenset({"right"}))

    def test_a_server_without_executed_returns_None_rather_than_passing(self):
        game = FakePlaybackGame(slot_frames=HP_FRAMES, gap=70, report_executed=False)
        session = make_session(game)
        self.assertIsNone(executed_input(game, 0))
        # `None` is "we could not look", not "it matched" -- and it is also
        # not "the port held nothing", which is `frozenset()`.
        self.assertIsNone(confirm_executed(game, 0, ["right"]))
        self.assertIsNone(session.executed(0))

    def test_verify_executed_is_opt_in_and_catches_a_stale_input_frame(self):
        game = FakePlaybackGame(slot_frames=HP_FRAMES, gap=70)
        session = make_session(game, verify_executed=True)
        session.load_state("a.state")
        session.set_held(0, ["right"])
        session.step()          # this frame saw "right" -- fine
        # Simulate the §3.6 failure: something else replaced the port's input
        # under us, so the next frame runs on an input we never asserted.
        game.held[0] = ("left",)
        with self.assertRaises(ExecutedInputError):
            session.step()

    def test_verify_executed_defaults_off_so_a_driven_port_is_not_a_false_alarm(self):
        game = FakePlaybackGame(slot_frames=HP_FRAMES, gap=70)
        session = make_session(game)
        self.assertFalse(session.verify_executed)
        # A replay drives the port from inside the frame; the session asserted
        # nothing, and nothing raises.
        run_replay(
            session, slot=make_slot(HP_FRAMES), arena="a.state", total_frames=10,
            contact_read=contact_read, attack_read=attack_read, port="p1",
        )


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
