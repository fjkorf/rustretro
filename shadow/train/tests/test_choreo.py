"""Unit tests for `framelab.choreo` — the general two-port choreography
engine (task A1, `docs/frames.md`'s §4/§4.5 machinery generalized beyond
`probe.py`'s hardwired attacker/guard/probe triple).

`FakeGame` here is deliberately smaller than `test_framelab_probe.py`'s or
`test_framelab_replay.py`'s: `verify()` is a COARSER check than the §4 act-
again sweep (see `choreo.py`'s module docstring), so the fake only needs a
rising-edge attack -> delayed contact model, not a full stun/pushback state
machine. It implements exactly `client.call(tool, **kwargs)`, the same
contract `LabSession` needs, and both ports share one health/contact
register per port (matching MK2 arcade's own struct-health-doubles-as-
contact-signal fact, `observables.make_contact_read_from_spec`'s docstring).
"""

from __future__ import annotations

import json
import unittest
from pathlib import Path

from shadow_train.framelab import choreo as c
from shadow_train.framelab.probe import run_schedule
from shadow_train.framelab.replay import buttons_from_mask
from shadow_train.framelab.session import LabSession


# ── the fake ────────────────────────────────────────────────────────────


class FakeGame:
    STARTUP = 4
    ATTACK = "atk"
    GUARD = "grd"

    def __init__(self):
        self.writes_enabled = False
        self.paused = False
        self.frame = 0
        self.calls: list = []
        self.loads = 0
        self.training_enabled = True
        self._reset()

    def _reset(self) -> None:
        self.gframe = 0
        self.health = {0: 200, 1: 200}
        self.held: dict = {0: (), 1: ()}
        self._prev_held: dict = {0: (), 1: ()}
        self.pending: list = []

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
        if "training.set_enabled(false)" in script:
            self.training_enabled = False
        elif "training.set_enabled(true)" in script:
            self.training_enabled = True
        elif "training.enabled()" in script:
            return {"ok": True, "output": "true" if self.training_enabled else "false"}
        elif "shadow.on()" in script:
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
            return {"error": "writes locked; call enable_writes first"}
        self.loads += 1
        self._reset()
        self.frame += 1
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
        mask = "|".join(sorted(self.held[port]))
        return {"ok": True, "asserted_mask": mask, "folded_mask": mask}

    def _tool_step(self):
        self._advance()
        return {"ok": True, "landed": True, "frame_count": self.frame}

    def _tool_run_frames(self, count, port0=None, port1=None):
        if not self.paused:
            return {"ok": False, "error": "run_frames requires the emulator paused"}
        if port0 is not None:
            self.held[0] = tuple(port0)
        if port1 is not None:
            self.held[1] = tuple(port1)
        landed = 0
        for _ in range(count):
            self._advance()
            landed += 1
        return {
            "ok": landed == count, "landed": landed, "all_landed": landed == count,
            "start_frame": self.frame - landed, "end_frame": self.frame,
        }

    def _advance(self) -> None:
        self.frame += 1
        self.gframe += 1
        for port in (0, 1):
            if self.ATTACK in self.held[port] and self.ATTACK not in self._prev_held[port]:
                self.pending.append((self.gframe + self.STARTUP, port))
        self._prev_held = dict(self.held)
        still = []
        for fire, attacker in self.pending:
            if fire == self.gframe:
                victim = 1 - attacker
                guarding = self.GUARD in self.held[victim]
                self.health[victim] -= 3 if guarding else 10
            else:
                still.append((fire, attacker))
        self.pending = still


def make_session(game: FakeGame) -> LabSession:
    s = LabSession(game, verify_fn=lambda s: True, input_settle_s=0)
    s.enforce_preconditions()
    return s


def sample_fn(session):
    g = session.client
    return {"health": dict(g.health), "contact": dict(g.health)}


ATTACK_CHORDS = {"HP": ("atk",), "LP": ("atk",), "HK": ("atk",), "LK": ("atk",), "Block": ("grd",)}


# ── Event / Beat ────────────────────────────────────────────────────────


class EventTest(unittest.TestCase):
    def test_negative_frame_is_refused(self):
        with self.assertRaises(c.ChoreoError):
            c.Event(-1, 0, ("atk",))

    def test_held_is_normalized_to_a_tuple(self):
        e = c.Event(0, 0, ["atk", "grd"])
        self.assertEqual(e.held, ("atk", "grd"))


class BeatLengthTest(unittest.TestCase):
    def test_length_is_inferred_from_the_last_event_when_frames_is_none(self):
        beat = c.Beat("b", events=(c.Event(0, 0, ("atk",)), c.Event(5, 0, ())))
        self.assertEqual(beat.length, 6)

    def test_explicit_frames_overrides_inference(self):
        beat = c.Beat("b", events=(c.Event(0, 0, ("atk",)),), frames=20)
        self.assertEqual(beat.length, 20)

    def test_no_events_and_no_frames_is_refused(self):
        with self.assertRaises(c.ChoreoError):
            c.Beat("b", events=()).length


# ── compile ─────────────────────────────────────────────────────────────


class CompileTest(unittest.TestCase):
    def test_beats_are_laid_out_back_to_back(self):
        b1 = c.Beat("first", events=(c.Event(0, 0, ("atk",)),), frames=10)
        b2 = c.Beat("second", events=(c.Event(0, 1, ("grd",)),), frames=5)
        sched = c.compile([b1, b2])
        self.assertEqual(sched.beat_spans["first"], (0, 10))
        self.assertEqual(sched.beat_spans["second"], (10, 15))
        self.assertEqual(sched.total_frames, 15)
        self.assertEqual(sched.frames[0][0], ("atk",))
        self.assertEqual(sched.frames[10][1], ("grd",))

    def test_a_trailing_release_is_appended_for_every_touched_port(self):
        beat = c.Beat("b", events=(c.Event(0, 0, ("atk",)), c.Event(3, 1, ("grd",))), frames=8)
        sched = c.compile([beat])
        self.assertEqual(sched.frames[8][0], ())
        self.assertEqual(sched.frames[8][1], ())

    def test_duplicate_beat_names_are_refused(self):
        b1 = c.Beat("x", events=(c.Event(0, 0, ()),), frames=1)
        b2 = c.Beat("x", events=(c.Event(0, 0, ()),), frames=1)
        with self.assertRaises(c.ChoreoError):
            c.compile([b1, b2])

    def test_conflicting_held_sets_at_the_same_global_frame_are_refused(self):
        # Two beats' own events never collide (they're offset by the
        # cursor), but two events WITHIN one beat at the same local frame
        # for the same port, asserting different things, must be refused
        # rather than one silently winning.
        beat = c.Beat(
            "b",
            events=(c.Event(0, 0, ("atk",)), c.Event(0, 0, ("grd",))),
            frames=5,
        )
        with self.assertRaises(c.ChoreoError):
            c.compile([beat])

    def test_the_identical_held_set_twice_at_one_frame_is_harmless(self):
        beat = c.Beat(
            "b", events=(c.Event(0, 0, ("atk",)), c.Event(0, 0, ("atk",))), frames=5,
        )
        sched = c.compile([beat])  # must not raise
        self.assertEqual(sched.frames[0][0], ("atk",))

    def test_an_event_past_the_beats_own_span_is_refused(self):
        beat = c.Beat("b", events=(c.Event(9, 0, ("atk",)),), frames=5)
        with self.assertRaises(c.ChoreoError):
            c.compile([beat])

    def test_compile_needs_at_least_one_beat(self):
        with self.assertRaises(c.ChoreoError):
            c.compile([])


# ── punish_beat / special_beat: the law-encoding helpers ──────────────────


class PunishBeatTest(unittest.TestCase):
    def test_guard_release_defaults_to_contact_frame_plus_one(self):
        beat = c.punish_beat(
            "p", attacker_port=0, defender_port=1, counter=("atk",),
            contact_frame=10, counter_at=15,
        )
        release_events = [e for e in beat.events if e.frame == 11 and e.held == ()]
        self.assertEqual(len(release_events), 1)

    def test_pressing_the_counter_on_the_guard_release_frame_is_refused(self):
        # punish.py's header: dropping guard and pressing the counter on the
        # SAME frame produces NO ATTACK AT ALL on MK2's block stance.
        with self.assertRaises(c.ChoreoError) as ctx:
            c.punish_beat(
                "p", attacker_port=0, defender_port=1, counter=("atk",),
                contact_frame=0, counter_at=1, guard_release_at=1,
            )
        self.assertIn("NO ATTACK AT ALL", str(ctx.exception))

    def test_a_counter_before_the_guard_release_is_refused(self):
        with self.assertRaises(c.ChoreoError):
            c.punish_beat(
                "p", attacker_port=0, defender_port=1, counter=("atk",),
                contact_frame=0, counter_at=0, guard_release_at=1,
            )

    def test_default_expect_targets_the_attacker_port(self):
        beat = c.punish_beat(
            "p", attacker_port=0, defender_port=1, counter=("atk",),
            contact_frame=0, counter_at=6,
        )
        self.assertEqual(beat.expect.punish_port, 0)
        self.assertEqual(beat.expect.punish_from, 6)
        self.assertTrue(beat.expect.punish)


class SpecialBeatTest(unittest.TestCase):
    def test_the_trigger_never_chords_with_the_direction_on_its_first_frame(self):
        beat = c.special_beat(
            "uppercut", port=0, steps=[(("down",), 6)], trigger=("y",),
            settle_frames=6, hold_frames=2,
        )
        first_direction_frame = min(e.frame for e in beat.events if "down" in e.held)
        trigger_events = [e for e in beat.events if "y" in e.held]
        self.assertEqual(len(trigger_events), 1)
        self.assertGreater(trigger_events[0].frame, first_direction_frame)
        # And the trigger event still holds the direction (a fresh onset,
        # not a replacement) -- MK2 uppercuts want down+HP, not bare HP.
        self.assertIn("down", trigger_events[0].held)

    def test_zero_settle_is_refused_rather_than_silently_chording(self):
        with self.assertRaises(c.ChoreoError):
            c.special_beat(
                "bad", port=0, steps=[(("down",), 6)], trigger=("y",), settle_frames=0,
            )

    def test_empty_steps_is_refused(self):
        with self.assertRaises(c.ChoreoError):
            c.special_beat("bad", port=0, steps=[], trigger=("y",))

    def test_compiles_cleanly_and_releases_at_the_end(self):
        beat = c.special_beat(
            "uppercut", port=0, steps=[(("down",), 6)], trigger=("y",),
            settle_frames=6, hold_frames=2,
        )
        sched = c.compile([beat])
        self.assertEqual(sched.frames[sched.total_frames][0], ())


# ── to_slot / save_slot ─────────────────────────────────────────────────


class ToSlotTest(unittest.TestCase):
    def test_masks_match_the_retropad_bit_table(self):
        # B=0 Y=1 SELECT=2 START=3 UP=4 DOWN=5 LEFT=6 RIGHT=7 A=8 X=9 L=10 R=11
        beat = c.Beat("b", events=(c.Event(0, 0, ("up", "y")), c.Event(2, 0, ())), frames=3)
        sched = c.compile([beat])
        slot = c.to_slot(sched, family="mk2", port="arcade")
        self.assertEqual(slot["version"], 1)
        self.assertEqual(slot["family"], "mk2")
        self.assertEqual(slot["port"], "arcade")
        self.assertIsNone(slot["state_note_at_start"])
        self.assertIn("created_at", slot)
        # up (bit 4 = 16) | y (bit 1 = 2) = 18
        self.assertEqual(slot["frames"][0], [18, 0])
        self.assertEqual(slot["frames"][2], [0, 0])

    def test_round_trips_through_buttons_from_mask(self):
        beat = c.Beat("b", events=(c.Event(0, 1, ("l", "down")),), frames=2)
        sched = c.compile([beat])
        slot = c.to_slot(sched, family="mk2", port="arcade")
        p2_mask = slot["frames"][0][1]
        self.assertEqual(buttons_from_mask(p2_mask), frozenset({"l", "down"}))

    def test_total_frames_can_pad_past_the_compiled_schedule(self):
        beat = c.Beat("b", events=(c.Event(0, 0, ("y",)),), frames=2)
        sched = c.compile([beat])
        slot = c.to_slot(sched, total_frames=5, family="mk2", port="arcade")
        self.assertEqual(len(slot["frames"]), 5)
        self.assertEqual(slot["frames"][4], [0, 0])  # padded with released input

    def test_an_unknown_button_name_is_refused(self):
        beat = c.Beat("b", events=(c.Event(0, 0, ("not-a-button",)),), frames=1)
        sched = c.compile([beat])
        with self.assertRaises(c.ChoreoError):
            c.to_slot(sched, family="mk2", port="arcade")

    def test_save_slot_sanitizes_the_name_and_writes_under_family(self):
        import tempfile

        beat = c.Beat("b", events=(c.Event(0, 0, ()),), frames=1)
        sched = c.compile([beat])
        slot = c.to_slot(sched, family="mk2", port="arcade")
        with tempfile.TemporaryDirectory() as td:
            path = c.save_slot(slot, "smoke-example", root=td)
            self.assertTrue(path.exists())
            self.assertEqual(path.parent.name, "mk2")
            got = json.loads(path.read_text())
            self.assertEqual(got, slot)
            for bad in ("..", "a/b", "."):
                with self.assertRaises(c.ChoreoError):
                    c.save_slot(slot, bad, root=td)


# ── beat sheets are data: schema + loader ───────────────────────────────


class LoadBeatSheetTest(unittest.TestCase):
    FIXTURE = Path(__file__).parent / "fixtures" / "example.beats.json"

    def test_loads_the_example_fixture(self):
        sheet = c.load_beat_sheet(str(self.FIXTURE), attack_chords=ATTACK_CHORDS)
        self.assertEqual(sheet.name, "example-jab-then-punish")
        self.assertEqual(sheet.family, "mk2")
        self.assertEqual(len(sheet.beats), 2)
        names = [b.name for b in sheet.beats]
        self.assertEqual(names, ["p1-jab-blocked", "p2-punish"])

    def test_attack_kind_resolves_move_to_low_level_buttons(self):
        sheet = c.load_beat_sheet(str(self.FIXTURE), attack_chords=ATTACK_CHORDS)
        jab = sheet.beats[0]
        press = next(e for e in jab.events if e.port == 0 and e.held)
        self.assertEqual(press.held, ("atk",))  # ATTACK_CHORDS["HP"]

    def test_attack_guard_block_resolves_Block_by_default(self):
        sheet = c.load_beat_sheet(str(self.FIXTURE), attack_chords=ATTACK_CHORDS)
        jab = sheet.beats[0]
        guard_events = [e for e in jab.events if e.port == 1]
        self.assertTrue(any(e.held == ("grd",) for e in guard_events))

    def test_punish_kind_dispatches_to_punish_beat(self):
        sheet = c.load_beat_sheet(str(self.FIXTURE), attack_chords=ATTACK_CHORDS)
        punish = sheet.beats[1]
        self.assertEqual(punish.expect.punish_port, 0)
        self.assertTrue(punish.expect.punish)

    def test_unknown_move_name_is_refused(self):
        data = json.loads(self.FIXTURE.read_text())
        data["beats"][0]["move"] = "NOT-A-MOVE"
        import tempfile

        with tempfile.TemporaryDirectory() as td:
            p = Path(td) / "bad.beats.json"
            p.write_text(json.dumps(data))
            with self.assertRaises(c.ChoreoError):
                c.load_beat_sheet(str(p), attack_chords=ATTACK_CHORDS)

    def test_a_beat_sheet_compiles(self):
        sheet = c.load_beat_sheet(str(self.FIXTURE), attack_chords=ATTACK_CHORDS)
        sched = c.compile(sheet.beats)
        self.assertGreater(sched.total_frames, 0)
        self.assertEqual(set(sched.beat_spans), {"p1-jab-blocked", "p2-punish"})


# ── execute + verify, end to end against the fake ─────────────────────────


class ExecuteVerifyTest(unittest.TestCase):
    def _sheet(self):
        jab = c.Beat(
            "p1-jab",
            events=(
                c.Event(0, 0, (FakeGame.ATTACK,)),
                c.Event(2, 0, ()),
                c.Event(0, 1, (FakeGame.GUARD,)),
            ),
            frames=20,
            expect=c.Expect(
                contact_port=1, contact=True,
                guard_port=1, guard_buttons=(FakeGame.GUARD,), guard=True, guard_check_frame=2,
                health_deltas={1: 3},
            ),
        )
        punish = c.punish_beat(
            "p2-punish", attacker_port=0, defender_port=1, counter=(FakeGame.ATTACK,),
            contact_frame=0, counter_at=6, guard_buttons=(FakeGame.GUARD,),
            frames=16,
        )
        idle = c.Beat("idle", events=(c.Event(0, 0, ()),), frames=5)
        return [jab, punish, idle]

    def test_every_beat_verifies_clean_against_the_fake(self):
        game = FakeGame()
        session = make_session(game)
        sched = c.compile(self._sheet())
        trace = c.execute(session, "fake.state", sched, sample_fn=sample_fn)
        report = c.verify(trace, sched.beats)

        by_name = {r.name: r for r in report.results}
        self.assertTrue(by_name["p1-jab"].passed, by_name["p1-jab"].notes)
        self.assertTrue(by_name["p2-punish"].passed, by_name["p2-punish"].notes)
        self.assertTrue(by_name["idle"].passed)
        self.assertIn("nothing was verified", by_name["idle"].notes[0])
        self.assertTrue(report.all_passed)
        self.assertEqual(report.refused, ())
        # p1's chip damage actually landed (guarded), p2's counter landed full.
        self.assertEqual(game.health[1], 200 - 3)
        self.assertEqual(game.health[0], 200 - 10)

    def test_a_wrong_expectation_is_flagged_refused_not_silently_passed(self):
        game = FakeGame()
        session = make_session(game)
        beats = self._sheet()
        # Claim the jab was NOT blocked (guard False) when it actually was --
        # verify() must catch this, not rubber-stamp a plausible-looking beat.
        beats[0] = c.Beat(
            "p1-jab", events=beats[0].events, frames=beats[0].frames,
            expect=c.Expect(
                contact_port=1, contact=True,
                guard_port=1, guard_buttons=(FakeGame.GUARD,), guard=False,
                guard_check_frame=2,
            ),
        )
        sched = c.compile(beats)
        trace = c.execute(session, "fake.state", sched, sample_fn=sample_fn)
        report = c.verify(trace, sched.beats)
        self.assertIn("p1-jab", report.refused)
        self.assertFalse(report.all_passed)

    def test_load_state_is_called_with_the_given_arena(self):
        game = FakeGame()
        session = make_session(game)
        sched = c.compile([c.Beat("b", events=(c.Event(0, 0, ()),), frames=3)])
        c.execute(session, "shadow/arenas/mk2/gap-45.state", sched, sample_fn=sample_fn)
        load_calls = [kw for t, kw in game.calls if t == "load_state"]
        self.assertEqual(load_calls[0].get("path"), "shadow/arenas/mk2/gap-45.state")

    def test_verify_raises_for_a_beat_not_in_the_trace(self):
        game = FakeGame()
        session = make_session(game)
        sched = c.compile([c.Beat("only", events=(c.Event(0, 0, ()),), frames=3)])
        trace = c.execute(session, "fake.state", sched, sample_fn=sample_fn)
        stray = c.Beat("nope", events=(c.Event(0, 0, ()),), frames=3)
        with self.assertRaises(c.ChoreoError):
            c.verify(trace, [stray])

    def test_a_missing_sample_fn_leaves_every_check_unverified_not_failed(self):
        game = FakeGame()
        session = make_session(game)
        sched = c.compile(self._sheet())
        trace = c.execute(session, "fake.state", sched, sample_fn=None)
        report = c.verify(trace, sched.beats)
        self.assertTrue(report.all_passed)  # nothing FAILS -- it just wasn't checked
        jab = next(r for r in report.results if r.name == "p1-jab")
        self.assertTrue(any(v is None for v in jab.checks.values()))


# ── the extracted executor loop still behaves like probe.replay's own ─────


class RunScheduleReuseTest(unittest.TestCase):
    """`probe.run_schedule` is the loop `probe.replay` used to inline;
    `choreo.execute` calls it with a schedule built from `Event`s instead of
    from a `Rig`/`MoveScript`. This exercises it directly, the way
    `choreo.execute` does, to prove the extraction still batches frames
    nothing observes into one `run_frames` call."""

    def test_batching_still_happens_through_the_extracted_function(self):
        game = FakeGame()
        session = make_session(game)
        session.load_state("fake.state")
        sched = {0: {0: (FakeGame.ATTACK,)}, 40: {0: ()}}
        trace = run_schedule(session, sched, 50, sample_fn=sample_fn, sample_from=45)
        self.assertEqual(len(trace), 51)
        self.assertIsNone(trace[10])
        self.assertIsNotNone(trace[45])
        self.assertLess(session.step_calls + session.batch_calls, 10)
        self.assertEqual(session.steps_taken, 50)


if __name__ == "__main__":
    unittest.main()
