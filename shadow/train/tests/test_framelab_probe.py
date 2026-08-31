"""Unit tests for `framelab.session` + `framelab.probe` — docs/frames.md §3
(preconditions) and §4 (the act-again probe).

Everything runs against `FakeGame`, a toy fighter with the three properties
that broke real measurements before:

  1. **Pushback.** The defender's `x` moves for several frames after contact
     with NO input at all. An absolute "did x change?" test calls that
     actionable; the differential must not. `FakeGame` also runs a free
     animation counter that ticks every frame in both runs, for the same
     reason.
  2. **A stun that outlives the pushback**, so the correct answer is
     strictly later than the naive one.
  3. **A transport that can drop frames.** `FakeGame` can be told to swallow
     steps or loads, so the §3.5/§3.6 confirmations are tested rather than
     assumed. It models the CURRENT server contract: `step` and `run_frames`
     are synchronous and report `landed` themselves, and `run_frames` refuses
     unless the emulator is paused.
"""

from __future__ import annotations

import unittest

from shadow_train.framelab.probe import (
    METHOD_BINARY,
    METHOD_LINEAR,
    ProbeCalibrationError,
    Anchor,
    AdvantageMeasurement,
    MonotonicityEvidence,
    MoveScript,
    NoContactError,
    ProbeError,
    Rig,
    ScriptStep,
    SweepResult,
    _actionable_from_traces,
    advantage_rows,
    calibrate_probe_latency,
    find_anchor,
    measure_advantage,
    replay,
    sweep_actionable,
)
from shadow_train.framelab.session import (
    LabError,
    LabSession,
    PreconditionError,
    call_ok,
    confirm_fold,
)


# ── the fake ──────────────────────────────────────────────────────────────


class FakeGame:
    """A 1-D two-fighter toy with hitstun, blockstun, pushback and an
    injection pipeline. Implements exactly `client.call(tool, **kwargs)`.

    Timeline of one contact at frame `c`:
        defender: cannot act until `c + stun` (or `c + block_stun` when
                  guarding), but is PUSHED BACK (x moves, input or not)
                  until `c + pushback`.
        attacker: cannot act until `c + recovery`.
    """

    ATTACK = "atk"
    GUARD = "grd"
    STARTUP = 6

    def __init__(
        self,
        *,
        pipeline: int = 0,          # frames of injection delay; L_obs = pipeline + 1
        stun: int = 24,
        block_stun: int = 14,
        recovery: int = 18,
        pushback: int = 9,
        hits: int = 1,
        hit_gap: int = 5,
        reach: int = 200,
        swallow_steps: int = 0,
        swallow_loads: bool = False,
        shadow_on: bool = False,
        training_sticks_on: bool = False,
        arena_alive: bool = True,
    ):
        self.pipeline = pipeline
        self.pipeline_schedule = None   # set by a test to vary L per load
        self.stun, self.block_stun = stun, block_stun
        self.recovery, self.pushback_frames = recovery, pushback
        self.hits, self.hit_gap, self.reach = hits, hit_gap, reach
        self.swallow_steps = swallow_steps
        self.swallow_loads = swallow_loads
        self.shadow_on = shadow_on
        self.training_sticks_on = training_sticks_on
        self.training_enabled = True
        self.arena_alive = arena_alive

        self.writes_enabled = False
        self.paused = False
        self.frame = 0
        self.calls: list = []
        self.loads = 0
        self._reset()

    # ── state ────────────────────────────────────────────────────────────
    def _reset(self) -> None:
        self.gframe = 0        # run-relative game frame (0 = loaded state)
        self.x = {0: 0, 1: 100}
        self.health = {0: 100, 1: 100}
        self.held = {0: (), 1: ()}
        self.trail: list = []
        self.free_at = {0: 0, 1: 0}
        self.pushed_until = {0: 0, 1: 0}
        self.pending_contacts: list = []
        self.contacts: list = []
        self.anim = 0
        self.attack_started = False

    def struct(self, port: int) -> tuple:
        """A struct-like composite: health, the fighter's own walk velocity,
        and a free-running animation counter that is IDENTICAL in probe and
        control (so it must cancel in the differential)."""
        return (self.health[port], self._velocity(port), self.anim)

    def _velocity(self, port: int) -> int:
        eff = self._effective(port)
        if self.gframe < self.free_at[port]:
            return 0
        return (1 if "right" in eff else 0) - (1 if "left" in eff else 0)

    def _effective(self, port: int) -> tuple:
        idx = len(self.trail) - 1 - self.pipeline
        if idx < 0:
            return ()
        return self.trail[idx][port]

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
        if "training.set_enabled(false)" in script:
            if not self.training_sticks_on:
                self.training_enabled = False
        elif "training.set_enabled(true)" in script:
            self.training_enabled = True
        elif "training.enabled()" in script:
            return {"ok": True, "output": "true" if self.training_enabled else "false"}
        elif "shadow.on()" in script:
            return {"ok": True, "output": "true" if self.shadow_on else "ok"}
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
        if self.swallow_loads:
            # Simulates the atomic guarantee NOT holding (docs/frames.md
            # §4.6): "ok" but never actually `paused`.
            return {"ok": True, "op": "load"}
        if self.pipeline_schedule:
            self.pipeline = self.pipeline_schedule[
                min(self.loads, len(self.pipeline_schedule) - 1)
            ]
        self.loads += 1
        self._reset()
        self.frame += 1          # a real load lets the core run a frame
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
        """`asserted` (what the next fold will feed the core) and `folded`
        (what the last one did). They agree instantly here — the fake has no
        host loop to race — so `confirm_fold` returns on its first poll."""
        mask = "|".join(sorted(self.held[port]))
        return {"ok": True, "port": port,
                "asserted_mask": mask, "folded_mask": mask}

    def _tool_step(self):
        # The real `step` is SYNCHRONOUS: it returns only once the emulated
        # frame is entirely finished, and says so (`landed`). A swallowed
        # step is therefore not a silent no-op any more — it is the server's
        # timeout shape, `ok: false` + `landed: false`.
        if self.swallow_steps > 0:
            self.swallow_steps -= 1
            return {
                "ok": False,
                "stepped": True,
                "landed": False,
                "error": "timed out waiting for the emulation thread",
            }
        self._advance()
        return {
            "ok": True,
            "stepped": True,
            "landed": True,
            "frame_count": self.frame,
        }

    def _tool_run_frames(self, count, port0=None, port1=None):
        """`step` batched: apply the per-port masks (replace, not OR), then
        run `count` frames, reporting how many actually landed."""
        if not self.paused:
            return {
                "ok": False,
                "error": "run_frames requires the emulator to be paused first",
            }
        if port0 is not None:
            self.held[0] = tuple(port0)
        if port1 is not None:
            self.held[1] = tuple(port1)
        start = self.frame
        landed = 0
        for _ in range(count):
            if self.swallow_steps > 0:
                self.swallow_steps -= 1
                break
            self._advance()
            landed += 1
        return {
            "ok": landed == count,
            "start_frame": start,
            "end_frame": self.frame,
            "requested": count,
            "landed": landed,
            "all_landed": landed == count,
            "error": None if landed == count else "timed out mid-batch",
        }

    # ── the toy game ─────────────────────────────────────────────────────
    def _advance(self) -> None:
        self.frame += 1        # transport frame_count: monotonic across loads
        self.gframe += 1       # game frame: what a run-relative trace indexes
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
            for i in range(self.hits):
                self.pending_contacts.append(
                    (self.gframe + self.STARTUP + i * self.hit_gap, i)
                )

        for p in (0, 1):
            if self.gframe < self.pushed_until[p]:
                self.x[p] += 3          # pushback: moves with NO input
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
        stun = self.block_stun if guarding else self.stun
        self.free_at[1] = self.gframe + stun
        self.free_at[0] = self.gframe + self.recovery
        self.pushed_until[1] = self.gframe + self.pushback_frames


def sampler(port: int):
    def read(session):
        g = session.client
        return {"x": g.x[port], "struct": g.struct(port)}

    return read


def contact_read(session):
    return session.client.health[1]


def make_session(game: FakeGame, *, verify: bool = True) -> LabSession:
    verify_fn = (lambda s: s.client.arena_alive) if verify else None
    # `input_settle_s=0`: the settle is a live-transport workaround (see
    # `LabSession.set_held`); against a synchronous fake it would only make
    # the suite slow.
    s = LabSession(game, verify_fn=verify_fn, input_settle_s=0)
    s.enforce_preconditions()
    return s


def make_rig(**kw) -> Rig:
    base = dict(
        arena="fake.state",
        attacker_port=0,
        defender_port=1,
        guard_buttons=(FakeGame.GUARD,),
        walk_directions=("right",),
        quiet_frames=20,
    )
    base.update(kw)
    return Rig(**base)


SCRIPT = MoveScript(
    name="poke",
    steps=(ScriptStep(4, (FakeGame.ATTACK,)),),
    lead_in=(ScriptStep(30, ("right",)),),
)


# ── §3: preconditions ─────────────────────────────────────────────────────


class PreconditionsTest(unittest.TestCase):
    def test_press_buttons_is_banned_by_construction(self):
        game = FakeGame()
        with self.assertRaises(PreconditionError) as ctx:
            call_ok(game, "press_buttons", buttons=["y"], frames=4)
        self.assertIn("BANNED", str(ctx.exception))
        self.assertEqual(game.calls, [], "the banned call never reached the client")

    def test_training_enforcement_is_verified_not_just_requested(self):
        game = FakeGame(training_sticks_on=True)
        with self.assertRaises(PreconditionError) as ctx:
            LabSession(game, input_settle_s=0).enforce_preconditions()
        self.assertIn("§3.1", str(ctx.exception))

    def test_training_enforcement_off_is_recorded(self):
        game = FakeGame()
        pre = LabSession(game, input_settle_s=0).enforce_preconditions()
        self.assertFalse(game.training_enabled)
        self.assertEqual(pre.training_enforcement, "off")
        self.assertTrue(pre.writes_armed)

    def test_shadow_runner_driving_a_port_is_refused(self):
        game = FakeGame(shadow_on=True)
        with self.assertRaises(PreconditionError) as ctx:
            LabSession(game, input_settle_s=0).enforce_preconditions()
        self.assertIn("§3.2", str(ctx.exception))

    def test_arena_liveness_is_rechecked_after_every_load(self):
        game = FakeGame()
        checks = []

        def flaky(session):
            checks.append(len(checks))
            return len(checks) < 3

        s = LabSession(game, verify_fn=flaky, input_settle_s=0)
        s.enforce_preconditions()
        s.load_state("fake.state")
        s.load_state("fake.state")
        with self.assertRaises(PreconditionError) as ctx:
            s.load_state("fake.state")
        self.assertIn("§3.4", str(ctx.exception))
        self.assertEqual(len(checks), 3)

    def test_a_step_that_never_lands_raises_instead_of_miscounting(self):
        game = FakeGame(swallow_steps=99)
        s = make_session(game)
        s.load_state("fake.state")
        with self.assertRaises(LabError) as ctx:
            s.step()
        self.assertIn("§3.5", str(ctx.exception))

    def test_a_partially_landed_batch_raises_instead_of_miscounting(self):
        # A batch that runs 3 of 10 frames is the same failure as a step that
        # never landed, wholesale: the replay would believe 10 frames of game
        # time that never happened.
        game = FakeGame()
        s = make_session(game)
        s.load_state("fake.state")
        game.swallow_steps = 3
        with self.assertRaises(LabError) as ctx:
            s.run_frames(10)
        self.assertIn("§3.5", str(ctx.exception))

    def test_run_frames_holds_the_mask_for_the_whole_batch(self):
        # The mask REPLACES the port's held set and is applied before the
        # first frame of the batch -- so all 5 frames walk.
        game = FakeGame(pipeline=0)
        s = make_session(game)
        s.load_state("fake.state")
        x0 = game.x[0]
        s.run_frames(5, holds={0: ("right",)})
        self.assertEqual(game.x[0] - x0, 10)   # 2 px/frame * 5 frames
        self.assertEqual(game.held[0], ("right",))
        self.assertEqual(s.steps_taken, 5, "batched frames still count as steps")

    def test_an_input_change_is_confirmed_to_have_reached_the_fold(self):
        # The host loop folds input BEFORE it checks the frame gate, so
        # "held_input was written" is not "the next frame will see it".
        # Measured live: passing the change as one of `run_frames`' own
        # per-port masks put 13 wrong answers in 200 identical evaluations.
        # Every input change therefore ends at the `get_input` oracle.
        game = FakeGame()
        s = make_session(game)
        s.load_state("fake.state")
        game.calls.clear()
        s.set_held(0, ["right"])
        s.run_frames(4, holds={1: ["left"]})
        order = [t for t, _ in game.calls]
        self.assertEqual(order[:2], ["hold_buttons", "get_input"])
        # The batch's own masks are NOT used — the hold is a separate,
        # confirmed call before `run_frames`.
        batch = [kw for t, kw in game.calls if t == "run_frames"]
        self.assertEqual(len(batch), 1)
        self.assertNotIn("port0", batch[0])
        self.assertNotIn("port1", batch[0])

    def test_a_hold_that_never_reaches_the_fold_raises(self):
        game = FakeGame()
        s = make_session(game)
        # A fold that never catches up: the game reports the port empty no
        # matter what is asserted.
        game._tool_get_input = lambda port=0: {
            "ok": True, "asserted_mask": "right", "folded_mask": ""
        }
        with self.assertRaises(LabError) as ctx:
            confirm_fold(game, 0, timeout_s=0.05)
        self.assertIn("never reached", str(ctx.exception))

    def test_run_frames_requires_a_paused_emulator(self):
        game = FakeGame()
        s = make_session(game)
        s.load_state("fake.state")
        game.paused = False
        with self.assertRaises(LabError):
            s.run_frames(5)

    def test_a_load_that_never_lands_raises_instead_of_measuring_the_old_state(self):
        # docs/frames.md §4.6: a `pause_after=True` load that comes back "ok"
        # but without `paused: true` means the atomic guarantee did not hold
        # -- this is now the failure mode a swallowed load simulates.
        game = FakeGame(swallow_loads=True)
        s = make_session(game)
        with self.assertRaises(LabError) as ctx:
            s.load_state("fake.state")
        self.assertIn("§4.6", str(ctx.exception))

    def test_load_state_requests_pause_after_and_never_calls_resume_pause(self):
        # task G5 / docs/frames.md §4.6: the load and the pause happen
        # atomically via `pause_after=True` -- `LabSession.load_state` must
        # never bracket the load with the plain `resume`/`pause` tools
        # (that bracket is exactly the free-frame defect §4.6 measured, and
        # its residual hazard is a `pause_after` load picking up a stray
        # frame right after an old-style plain `pause()`).
        game = FakeGame()
        s = make_session(game)
        s.load_state("fake.state")
        order = [t for t, _ in game.calls]
        self.assertNotIn("resume", order)
        self.assertNotIn("pause", order)
        load_calls = [kw for t, kw in game.calls if t == "load_state"]
        self.assertEqual(len(load_calls), 1)
        self.assertIs(load_calls[0].get("pause_after"), True)
        self.assertTrue(game.paused)

    def test_load_state_never_calls_press_buttons(self):
        game = FakeGame()
        s = make_session(game)
        s.load_state("fake.state")
        s.set_held(0, ["right"])
        s.step()
        self.assertNotIn("press_buttons", {t for t, _ in game.calls})


# ── the replay engine: batching must not change what is measured ─────────


class ReplayBatchingTest(unittest.TestCase):
    """`replay` runs every frame nothing observes in ONE `run_frames` call.
    That is a transport change, so it has to be shown to be invisible to the
    measurement: the batched replay must produce the identical trace, and it
    must still stop at every frame the schedule touches."""

    def test_batched_replay_matches_a_fully_sampled_one_frame_for_frame(self):
        # `sample_from=0` forces a stop at every frame (no batching possible);
        # `sample_from=40` lets the first 40 frames collapse into one call.
        # The overlapping window must be byte-identical, or batching moved
        # the game.
        full = replay(
            make_session(FakeGame()), rig=make_rig(), script=SCRIPT,
            total_frames=60, defender_guard=False, probe_port=1,
            probe_buttons=("right",), probe_at=45,
            sample_fn=sampler(1), sample_from=0,
        )
        batched = replay(
            make_session(FakeGame()), rig=make_rig(), script=SCRIPT,
            total_frames=60, defender_guard=False, probe_port=1,
            probe_buttons=("right",), probe_at=45,
            sample_fn=sampler(1), sample_from=40,
        )
        self.assertEqual(full[40:], batched[40:])
        self.assertTrue(any(t is not None for t in batched[40:]))

    def test_a_batch_never_runs_past_an_input_change(self):
        # The probe hold at frame 45 must be asserted for frame 46 onward and
        # NOT earlier -- a batch that overshot it would hold the walk during
        # frames the fighter was supposed to be input-free, which is the
        # one-frame-early hold in a new costume.
        game = FakeGame()
        s = make_session(game)
        replay(
            s, rig=make_rig(), script=SCRIPT, total_frames=50,
            defender_guard=False, probe_port=1, probe_buttons=("right",),
            probe_at=45, sample_fn=sampler(1), sample_from=46,
        )
        # `trail[i]` is the held set folded on frame i+1.
        self.assertTrue(all("right" not in t[1] for t in game.trail[:45]))
        self.assertTrue(all("right" in t[1] for t in game.trail[45:50]))

    def test_batching_actually_happened(self):
        # If this ever regresses to one call per frame the numbers stay right
        # and the run takes hours, so the cost has to be asserted too.
        game = FakeGame()
        s = make_session(game)
        replay(
            s, rig=make_rig(), script=SCRIPT, total_frames=60,
            defender_guard=False, probe_port=1, probe_buttons=("right",),
            probe_at=55, sample_fn=sampler(1), sample_from=56,
        )
        self.assertEqual(s.steps_taken, 60)
        self.assertGreater(s.frames_batched, 40)
        self.assertLess(s.step_calls + s.batch_calls, 20)


# ── §4.2: the differential comparison ─────────────────────────────────────


class DifferentialComparisonTest(unittest.TestCase):
    def test_identical_traces_are_not_actionable(self):
        probe = [{"x": 5}, {"x": 8}, {"x": 11}]
        control = [{"x": 5}, {"x": 8}, {"x": 11}]
        self.assertFalse(_actionable_from_traces(probe, control, "x"))

    def test_a_control_that_also_changes_is_not_actionable(self):
        # Pushback: x moves identically in both runs. An absolute
        # "did x change?" test says TRUE; the differential must say FALSE.
        probe = [{"x": 5}, {"x": 8}, {"x": 11}]
        control = [{"x": 5}, {"x": 8}, {"x": 11}]
        self.assertTrue(any(probe[i]["x"] != probe[i - 1]["x"] for i in (1, 2)))
        self.assertFalse(_actionable_from_traces(probe, control, "x"))

    def test_divergence_anywhere_in_the_window_is_actionable(self):
        probe = [{"x": 5}, {"x": 8}, {"x": 13}]
        control = [{"x": 5}, {"x": 8}, {"x": 11}]
        self.assertTrue(_actionable_from_traces(probe, control, "x"))

    def test_unsampled_frames_raise_rather_than_compare_none(self):
        with self.assertRaises(ProbeError):
            _actionable_from_traces([None], [{"x": 1}], "x")


# ── §4.1: the anchor ──────────────────────────────────────────────────────


class AnchorTest(unittest.TestCase):
    def test_single_hit_anchor_and_hit_count(self):
        game = FakeGame(hits=1)
        s = make_session(game)
        a = find_anchor(
            s, rig=make_rig(), script=SCRIPT, contact_read=contact_read,
            total_frames=90, defender_guard=False,
        )
        self.assertEqual(a.hits, 1)
        self.assertEqual(a.contact_frame, game.contacts[0])

    def test_multi_hit_anchors_on_the_LAST_contact_of_the_cluster(self):
        game = FakeGame(hits=3, hit_gap=5)
        s = make_session(game)
        a = find_anchor(
            s, rig=make_rig(), script=SCRIPT, contact_read=contact_read,
            total_frames=110, defender_guard=False,
        )
        self.assertEqual(a.hits, 3)
        self.assertEqual(len(a.contact_frames), 3)
        self.assertEqual(a.contact_frame, a.contact_frames[-1])
        self.assertEqual(a.contact_frame, max(game.contacts))
        # Anchoring on the FIRST fire would be 10 frames early -- exactly the
        # "advantage too negative by the inter-hit gap" error §4.1 names.
        self.assertEqual(a.contact_frame - a.contact_frames[0], 10)

    def test_contacts_separated_by_more_than_the_quiet_window_are_separate_moves(self):
        game = FakeGame(hits=3, hit_gap=25)
        s = make_session(game)
        a = find_anchor(
            s, rig=make_rig(), script=SCRIPT, contact_read=contact_read,
            total_frames=150, defender_guard=False,
        )
        self.assertEqual(a.hits, 1)
        self.assertEqual(a.contact_frame, min(game.contacts))

    def test_a_whiff_raises_rather_than_inventing_an_anchor(self):
        game = FakeGame(reach=0)   # nothing is ever in range
        s = make_session(game)
        with self.assertRaises(NoContactError):
            find_anchor(
                s, rig=make_rig(), script=SCRIPT, contact_read=contact_read,
                total_frames=90, defender_guard=False,
            )

    def test_a_truncated_quiet_window_raises_rather_than_guessing_hits(self):
        game = FakeGame(hits=3, hit_gap=5)
        s = make_session(game)
        with self.assertRaises(ProbeError):
            find_anchor(
                s, rig=make_rig(), script=SCRIPT, contact_read=contact_read,
                total_frames=50, defender_guard=False,
            )


# ── §4.3: the sweep ───────────────────────────────────────────────────────


class LinearSweepTest(unittest.TestCase):
    def _sweep(self, game, port, *, guard, max_search=40, **kw):
        s = make_session(game)
        rig = make_rig()
        anchor = find_anchor(
            s, rig=rig, script=SCRIPT, contact_read=contact_read,
            total_frames=110, defender_guard=guard,
        )
        return anchor, sweep_actionable(
            s, rig=rig, script=SCRIPT, port=port, anchor=anchor.contact_frame,
            observables=("x", "struct"), sample_fn=sampler(port),
            input_latency_frames={"x": game.pipeline + 1, "struct": game.pipeline + 1},
            defender_guard=guard, max_search=max_search, **kw,
        )

    def test_defender_first_true_matches_the_simulated_stun(self):
        game = FakeGame(stun=24, pushback=9, pipeline=0)
        anchor, res = self._sweep(game, 1, guard=False)
        # A_rel = 24; W = L = 1; N* = A_rel - l - c with l = 1, c = 0.
        self.assertEqual(res["x"].first_true, 23)
        self.assertEqual(res["struct"].first_true, 23)
        self.assertEqual(res["x"].actionable_after_contact, 24)

    def test_pushback_does_not_register_as_actionable(self):
        # The defender's x moves for 9 frames after contact with no input at
        # all. Every one of those N is FALSE in the predicate; an absolute
        # observable would have made them TRUE.
        game = FakeGame(stun=24, pushback=9, pipeline=0)
        _, res = self._sweep(game, 1, guard=False)
        self.assertEqual(res["x"].predicate[:9], (False,) * 9)

    def test_attacker_first_true_matches_the_simulated_recovery(self):
        game = FakeGame(recovery=18, pipeline=0)
        _, res = self._sweep(game, 0, guard=False)
        self.assertEqual(res["x"].first_true, 17)

    def test_predicate_is_monotone_and_reported_as_such(self):
        game = FakeGame(pipeline=0)
        _, res = self._sweep(game, 1, guard=False)
        self.assertTrue(res["x"].monotone)
        self.assertEqual(res["x"].method, METHOD_LINEAR)

    def test_blockstun_is_shorter_than_hitstun_with_the_same_protocol(self):
        game_hit = FakeGame(stun=24, block_stun=14, pipeline=0)
        _, hit = self._sweep(game_hit, 1, guard=False)
        game_blk = FakeGame(stun=24, block_stun=14, pipeline=0)
        _, blk = self._sweep(game_blk, 1, guard=True)
        self.assertEqual(hit["x"].first_true, 23)
        self.assertEqual(blk["x"].first_true, 13)
        self.assertEqual(blk["x"].rig_guard_state, "held")
        self.assertEqual(hit["x"].rig_guard_state, "none")

    def test_a_longer_pipeline_shifts_every_first_true_by_the_same_amount(self):
        a = FakeGame(stun=24, pipeline=0)
        _, ra = self._sweep(a, 1, guard=False)
        b = FakeGame(stun=24, pipeline=2)
        _, rb = self._sweep(b, 1, guard=False)
        self.assertEqual(ra["x"].first_true - rb["x"].first_true, 2)
        # ...and the absolute, which adds the window back, is unchanged.
        self.assertEqual(
            ra["x"].actionable_after_contact, rb["x"].actionable_after_contact
        )

    def test_never_actionable_is_null_not_zero(self):
        game = FakeGame(stun=500, pipeline=0)
        _, res = self._sweep(game, 1, guard=False, max_search=10)
        self.assertIsNone(res["x"].first_true)
        self.assertIsNone(res["x"].actionable_after_contact)

    def test_a_window_shorter_than_the_latency_is_refused(self):
        game = FakeGame(pipeline=3)
        s = make_session(game)
        rig = make_rig()
        with self.assertRaises(ProbeError) as ctx:
            sweep_actionable(
                s, rig=rig, script=SCRIPT, port=1, anchor=40,
                observables=("x",), sample_fn=sampler(1),
                input_latency_frames={"x": 4}, window={"x": 2},
                defender_guard=False, max_search=2,
            )
        self.assertIn("never actionable", str(ctx.exception))


class BinarySearchGateTest(unittest.TestCase):
    def _call(self, monotonicity):
        game = FakeGame()
        s = make_session(game)
        return sweep_actionable(
            s, rig=make_rig(), script=SCRIPT, port=1, anchor=40,
            observables=("x",), sample_fn=sampler(1),
            input_latency_frames=1, defender_guard=False, max_search=5,
            method=METHOD_BINARY, monotonicity=monotonicity, move_class="poke",
        )

    def test_binary_search_without_any_evidence_is_refused(self):
        with self.assertRaises(PreconditionError) as ctx:
            self._call(None)
        self.assertIn("§4.3", str(ctx.exception))

    def test_binary_search_with_evidence_for_another_move_class_is_refused(self):
        ev = MonotonicityEvidence(
            move_class="sweep", observable="x",
            samples=tuple((n, n >= 3) for n in range(6)),
        )
        with self.assertRaises(PreconditionError):
            self._call(ev)

    def test_binary_search_with_a_non_monotone_demonstration_is_refused(self):
        # T...T F...F T...T -- exactly the shape §4.3 says the first draft's
        # N-1/N/N+1 confirmation cannot detect.
        vals = {0: True, 1: True, 2: False, 3: False, 4: True, 5: True}
        ev = MonotonicityEvidence(
            move_class="poke", observable="x",
            samples=tuple(vals.items()),
        )
        with self.assertRaises(PreconditionError):
            self._call(ev)

    def test_binary_search_with_an_incomplete_demonstration_is_refused(self):
        ev = MonotonicityEvidence(
            move_class="poke", observable="x",
            samples=tuple((n, n >= 3) for n in range(4)),   # 0..3, max_search 5
        )
        with self.assertRaises(PreconditionError):
            self._call(ev)

    def test_evidence_that_never_goes_true_does_not_demonstrate_monotonicity(self):
        ev = MonotonicityEvidence(
            move_class="poke", observable="x",
            samples=tuple((n, False) for n in range(6)),
        )
        self.assertFalse(ev.demonstrates("poke", "x", 5))

    def test_a_complete_monotone_demonstration_is_accepted(self):
        ev = MonotonicityEvidence(
            move_class="poke", observable="x",
            samples=tuple((n, n >= 3) for n in range(6)),
        )
        res = self._call(ev)
        self.assertEqual(res["x"].method, METHOD_BINARY)

    def test_linear_sweep_needs_no_evidence_at_all(self):
        game = FakeGame()
        s = make_session(game)
        res = sweep_actionable(
            s, rig=make_rig(), script=SCRIPT, port=1, anchor=40,
            observables=("x",), sample_fn=sampler(1),
            input_latency_frames=1, defender_guard=False, max_search=3,
        )
        self.assertEqual(res["x"].method, METHOD_LINEAR)


# ── §4.2's corner hazard ──────────────────────────────────────────────────


class CornerHazardTest(unittest.TestCase):
    def test_a_direction_that_cannot_move_falls_through_to_the_other(self):
        class Walled(FakeGame):
            def _advance(self):
                super()._advance()
                self.x[1] = max(self.x[1], 100)   # cannot walk left of 100

        game = Walled(stun=12, pushback=0, pipeline=0)
        s = make_session(game)
        rig = make_rig(walk_directions=("left", "right"))
        anchor = find_anchor(
            s, rig=rig, script=SCRIPT, contact_read=contact_read,
            total_frames=110, defender_guard=False,
        )
        res = sweep_actionable(
            s, rig=rig, script=SCRIPT, port=1, anchor=anchor.contact_frame,
            observables=("x",), sample_fn=sampler(1),
            input_latency_frames=1, defender_guard=False, max_search=25,
        )
        self.assertEqual(res["x"].direction, "right")
        self.assertEqual(res["x"].first_true, 11)


# ── advantage + rows ──────────────────────────────────────────────────────


class AdvantageTest(unittest.TestCase):
    def test_measure_advantage_drives_both_sides_from_one_anchor(self):
        game = FakeGame(stun=24, block_stun=14, recovery=18, pipeline=0)
        s = make_session(game)
        res = measure_advantage(
            s, rig=make_rig(), script=SCRIPT, observables=("x",),
            sample_fn=sampler(1), contact_read=contact_read,
            input_latency_frames=1, defender_guard=True,
            anchor_total_frames=110, max_search=40,
        )
        # NOTE: sample_fn reads the DEFENDER here, so only the defender's own
        # sweep is meaningful; this asserts the plumbing (one anchor, two
        # sweeps, one measurement per observable), not the attacker number.
        self.assertEqual(set(res), {"x"})
        self.assertEqual(res["x"].defender.first_true, 13)
        self.assertEqual(res["x"].rig_guard_state, "held")
        self.assertEqual(res["x"].anchor.hits, 1)

    def test_advantage_is_defender_minus_attacker(self):
        game = FakeGame(stun=24, block_stun=14, recovery=18, pipeline=0)
        s = make_session(game)
        rig = make_rig()
        anchor = find_anchor(
            s, rig=rig, script=SCRIPT, contact_read=contact_read,
            total_frames=110, defender_guard=True,
        )
        att = sweep_actionable(
            s, rig=rig, script=SCRIPT, port=0, anchor=anchor.contact_frame,
            observables=("x",), sample_fn=sampler(0), input_latency_frames=1,
            defender_guard=True, max_search=40,
        )
        dfn = sweep_actionable(
            s, rig=rig, script=SCRIPT, port=1, anchor=anchor.contact_frame,
            observables=("x",), sample_fn=sampler(1), input_latency_frames=1,
            defender_guard=True, max_search=40,
        )
        m = AdvantageMeasurement(
            move="poke", observable="x", rig_guard_state="held",
            anchor=anchor, attacker=att["x"], defender=dfn["x"],
        )
        # blockstun 14, recovery 18 -> the attacker is -4 on block.
        self.assertEqual(m.advantage, 13 - 17)

    def test_advantage_is_null_when_either_side_never_becomes_actionable(self):
        blank = SweepResult(
            observable="x", method=METHOD_LINEAR, direction="right",
            first_true=None, predicate=(), monotone=None, window=1,
            input_latency_frames=1, max_search=5, port=1,
            rig_guard_state="held", runs=1,
        )
        good = SweepResult(
            observable="x", method=METHOD_LINEAR, direction="right",
            first_true=3, predicate=(), monotone=True, window=1,
            input_latency_frames=1, max_search=5, port=0,
            rig_guard_state="held", runs=1,
        )
        anchor = Anchor(contact_frame=1, hits=1, contact_frames=(1,), quiet_frames=20)
        m = AdvantageMeasurement(
            move="poke", observable="x", rig_guard_state="held",
            anchor=anchor, attacker=good, defender=blank,
        )
        self.assertIsNone(m.advantage)


class AdvantageRowsTest(unittest.TestCase):
    def _measurement(self, guard_state, att, dfn, observable="x", hits=1):
        anchor = Anchor(
            contact_frame=50, hits=hits, contact_frames=(50,), quiet_frames=20
        )
        mk = lambda n, port: SweepResult(  # noqa: E731
            observable=observable, method=METHOD_LINEAR, direction="right",
            first_true=n, predicate=(), monotone=True, window=1,
            input_latency_frames=1, max_search=40, port=port,
            rig_guard_state=guard_state, runs=1,
        )
        return AdvantageMeasurement(
            move="poke", observable=observable, rig_guard_state=guard_state,
            anchor=anchor, attacker=mk(att, 0), defender=mk(dfn, 1),
        )

    def test_row_carries_provenance_and_leaves_unmeasured_columns_absent(self):
        rows = advantage_rows(
            family="mk2", port="arcade", char="reptile",
            core_id="core:sha256:aaaa", rom_id="rom:sha256:bbbb",
            on_block=self._measurement("held", 17, 13),
            on_hit=self._measurement("none", 17, 23),
        )
        row = rows[0]
        self.assertEqual(row["on_block"], -4)
        self.assertEqual(row["on_hit"], 6)
        self.assertEqual(row["method"], METHOD_LINEAR)
        self.assertEqual(row["observable"], "x")
        self.assertEqual(row["input_latency_frames"], 1)
        self.assertNotIn("first_active_frame", row)   # §4.4: never from here
        self.assertEqual(row["hits"], 1)

    def test_row_inserts_into_the_real_store_with_nulls_intact(self):
        import tempfile, pathlib
        from shadow_train.framelab.store import FrameStore

        with tempfile.TemporaryDirectory() as td:
            path = pathlib.Path(td) / "frames.sqlite"
            with FrameStore(path) as store:
                rid = store.insert(
                    advantage_rows(
                        family="mk2", port="arcade", char="reptile",
                        core_id="c", rom_id="r",
                        on_block=self._measurement("held", 17, 13),
                    )[0]
                )
                got = store.get(rid)
        self.assertEqual(got["on_block"], -4)
        self.assertIsNone(got["on_hit"])
        self.assertIsNone(got["first_active_frame"])   # absent, never 0
        self.assertIsNone(got["hitstop"])

    def test_mixing_observables_across_on_hit_and_on_block_is_refused(self):
        with self.assertRaises(ValueError):
            advantage_rows(
                family="mk2", port="arcade", char="reptile",
                core_id="c", rom_id="r",
                on_block=self._measurement("held", 17, 13, observable="x"),
                on_hit=self._measurement("none", 17, 23, observable="struct"),
            )


# ── §3.1 re-applied to the probe's own input shape ────────────────────────


class ProbeShapeCalibrationTest(unittest.TestCase):
    """The neutral calibration is not automatically the probe's calibration.
    Live on MK2 arcade the guarded defender's probe (release guard AND walk on
    the same frame) has a longer latency than a bare neutral walk, and a
    window sized from the neutral number reported the defender NEVER
    ACTIONABLE across all 46 candidate N."""

    def _calibrate(self, game, port, *, guard=False, **kw):
        s = make_session(game)
        rig = make_rig()
        anchor = find_anchor(
            s, rig=rig, script=SCRIPT, contact_read=contact_read,
            total_frames=110, defender_guard=guard,
        )
        return calibrate_probe_latency(
            s, rig=rig, script=SCRIPT, port=port, anchor=anchor.contact_frame,
            at_n=40, observables=("x", "struct"), sample_fn=sampler(port),
            defender_guard=guard, **kw,
        )

    def test_returns_the_simulated_latency_per_observable(self):
        game = FakeGame(pipeline=2)
        got = self._calibrate(game, 1, trials=5)
        self.assertEqual(got, {"x": 3, "struct": 3})

    def test_a_latency_that_varies_across_trials_raises_and_never_averages(self):
        game = FakeGame(pipeline=0)
        # Loads alternate probe/control; bump the pipeline partway through.
        game.pipeline_schedule = [0] * 6 + [3] * 40
        with self.assertRaises(ProbeCalibrationError) as ctx:
            self._calibrate(game, 1, trials=5)
        self.assertIn("not constant", str(ctx.exception))

    def test_an_observable_that_never_responds_is_a_hard_failure(self):
        game = FakeGame(pipeline=0)
        s = make_session(game)
        rig = make_rig()
        with self.assertRaises(ProbeCalibrationError) as ctx:
            calibrate_probe_latency(
                s, rig=rig, script=SCRIPT, port=1, anchor=40, at_n=40,
                observables=("frozen",),
                sample_fn=lambda sess: {"frozen": 7},
                defender_guard=False, trials=2, max_window=6,
            )
        self.assertIn("never diverged", str(ctx.exception))

    def test_a_window_from_this_calibration_makes_the_sweep_work(self):
        game = FakeGame(pipeline=3, stun=24)
        s = make_session(game)
        rig = make_rig()
        anchor = find_anchor(
            s, rig=rig, script=SCRIPT, contact_read=contact_read,
            total_frames=110, defender_guard=False,
        )
        lat = calibrate_probe_latency(
            s, rig=rig, script=SCRIPT, port=1, anchor=anchor.contact_frame,
            at_n=40, observables=("x",), sample_fn=sampler(1),
            defender_guard=False, trials=2,
        )
        res = sweep_actionable(
            s, rig=rig, script=SCRIPT, port=1, anchor=anchor.contact_frame,
            observables=("x",), sample_fn=sampler(1),
            input_latency_frames=lat, defender_guard=False, max_search=40,
        )
        self.assertEqual(res["x"].window, 4)
        self.assertEqual(res["x"].actionable_after_contact, 24)


class BinarySearchExecutionTest(unittest.TestCase):
    """Once §4.3's gate is satisfied, `method="binary_search"` must actually
    BISECT -- recording the method while running a linear sweep would be a
    provenance lie (§7)."""

    def _run(self, method, **kw):
        game = FakeGame(stun=24, pipeline=0)
        s = make_session(game)
        rig = make_rig()
        anchor = find_anchor(
            s, rig=rig, script=SCRIPT, contact_read=contact_read,
            total_frames=110, defender_guard=False,
        )
        res = sweep_actionable(
            s, rig=rig, script=SCRIPT, port=1, anchor=anchor.contact_frame,
            observables=("x",), sample_fn=sampler(1), input_latency_frames=1,
            defender_guard=False, max_search=40, method=method, **kw,
        )
        return res["x"]

    def test_bisection_finds_the_same_frame_as_the_linear_sweep(self):
        linear = self._run(METHOD_LINEAR)
        evidence = MonotonicityEvidence(
            move_class="poke", observable="x",
            samples=tuple(enumerate(linear.predicate)),
            source="the linear sweep in this test",
        )
        binary = self._run(METHOD_BINARY, monotonicity=evidence)
        self.assertEqual(binary.first_true, linear.first_true)
        self.assertLess(binary.runs, linear.runs)
        # A bisected predicate is SPARSE and says so; monotonicity is not
        # re-claimed from a run that never looked at most of the range.
        self.assertIn(None, binary.predicate)
        self.assertIsNone(binary.monotone)

    def test_bisection_refuses_more_than_one_observable(self):
        game = FakeGame()
        s = make_session(game)
        ev = MonotonicityEvidence(
            move_class="poke", observable="x",
            samples=tuple((n, n >= 2) for n in range(6)),
        )
        with self.assertRaises(ValueError) as ctx:
            sweep_actionable(
                s, rig=make_rig(), script=SCRIPT, port=1, anchor=40,
                observables=("x", "struct"), sample_fn=sampler(1),
                input_latency_frames=1, defender_guard=False, max_search=5,
                method=METHOD_BINARY, monotonicity=ev,
            )
        self.assertIn("exactly one observable", str(ctx.exception))


class RepeatedEvaluationTest(unittest.TestCase):
    """A live transport flake (`hold_buttons` landing one frame early on ~1.5%
    of runs) put a single spurious TRUE below the real boundary and moved
    `first_true` by several frames, silently. `repeats > 1` must turn that
    into a loud failure -- and must NOT vote."""

    def _sweep(self, repeats, *, flake_load=None):
        game = FakeGame(pipeline=2, stun=24)
        s = make_session(game)
        rig = make_rig()
        anchor = find_anchor(
            s, rig=rig, script=SCRIPT, contact_read=contact_read,
            total_frames=110, defender_guard=False,
        )
        base = sampler(1)

        def flaky(session):
            got = dict(base(session))
            # One single run, deep inside the stun where the true answer is
            # FALSE, reads one unit off -- the shape of the live flake.
            if flake_load is not None and session.client.loads == flake_load:
                got["x"] = got["x"] + 1
            return got

        return sweep_actionable(
            s, rig=rig, script=SCRIPT, port=1, anchor=anchor.contact_frame,
            observables=("x",), sample_fn=flaky, input_latency_frames=3,
            defender_guard=False, max_search=40, repeats=repeats,
        )

    def test_repeats_one_is_the_default_and_costs_one_run_per_n(self):
        clean = self._sweep(1)
        self.assertEqual(clean["x"].first_true, 21)

    def test_one_flaky_run_silently_corrupts_first_true_without_repeats(self):
        # loads: 0 = find_anchor, 1 = the shared control, then one probe run
        # per N at repeats=1 -- so load 5 is N=3's probe... except the flaky
        # sampler also runs during the control, which shifts it by one. What
        # matters is the SHAPE: a single flake far below the real boundary of
        # 21 is reported as `first_true` and nothing complains.
        corrupted = self._sweep(1, flake_load=5)
        self.assertEqual(corrupted["x"].first_true, 2)
        self.assertLess(corrupted["x"].first_true, 21)
        self.assertFalse(corrupted["x"].monotone)

    def test_the_same_flake_raises_under_repeats(self):
        with self.assertRaises(ProbeError) as ctx:
            self._sweep(2, flake_load=5)
        self.assertIn("did not reproduce", str(ctx.exception))
        self.assertIn("DELETED, not averaged", str(ctx.exception))

    def test_repeats_agreeing_returns_the_same_answer_as_a_single_run(self):
        self.assertEqual(self._sweep(2)["x"].first_true, 21)


class PerObservableDirectionTest(unittest.TestCase):
    def test_a_noisy_observable_cannot_lock_in_a_direction_a_clean_one_needs(self):
        """Live regression: on the guarded defender, `struct_divergence`
        diverged walking one way while `pointer_x` could only diverge the
        other. A single shared direction choice reported the clean observable
        as NEVER ACTIONABLE."""

        class OneWay(FakeGame):
            def _advance(self):
                super()._advance()
                self.x[1] = max(self.x[1], 100)   # cannot walk left of 100

        game = OneWay(stun=12, pushback=0, pipeline=0)
        s = make_session(game)
        rig = make_rig(walk_directions=("left", "right"))
        anchor = find_anchor(
            s, rig=rig, script=SCRIPT, contact_read=contact_read,
            total_frames=110, defender_guard=False,
        )

        def two_observables(session):
            g = session.client
            # `struct` sees the walk INTENT even when the wall stops the walk,
            # so it diverges on "left"; `x` can only diverge on "right".
            return {"x": g.x[1], "struct": g.struct(1)}

        res = sweep_actionable(
            s, rig=rig, script=SCRIPT, port=1, anchor=anchor.contact_frame,
            observables=("x", "struct"), sample_fn=two_observables,
            input_latency_frames=1, defender_guard=False, max_search=25,
        )
        self.assertEqual(res["struct"].direction, "left")
        self.assertEqual(res["x"].direction, "right")
        self.assertEqual(res["x"].first_true, 11)
        self.assertEqual(res["struct"].first_true, 11)


if __name__ == "__main__":
    unittest.main()
