from __future__ import annotations

import unittest

from shadow_train.framelab.calibrate import (
    CalibrationError,
    zero_point_calibration,
)


class FakeClient:
    """A deterministic stand-in for `McpClient`/`Probe.client`, driving a
    toy 1-D walk so `zero_point_calibration`'s control flow is exercised
    with no live emulator. It implements exactly `.call(tool, **kwargs)` —
    the only method the calibration module requires.

    Simulated physics: holding a direction on `port` moves a 1-D `position`
    by +1 per step, but only once `frame` (steps since the hold was
    asserted) exceeds the port's currently-configured latency. `directions`
    not in `live_directions` never move the position at all (simulates a
    cornered fighter that can't walk into a wall, §4.2).

    `latencies`, if given, is consulted once per PROBE trace (every other
    `load_state`, since each trial does control-then-probe) so a test can
    make the simulated latency vary trial-to-trial and exercise the "not
    constant -> raise" path.
    """

    def __init__(self, *, latency=3, latencies=None, live_directions=("left", "right")):
        self.fixed_latency = latency
        self.latencies = list(latencies) if latencies is not None else None
        self.live_directions = set(live_directions)

        self.calls: list[tuple[str, dict]] = []
        self.training_enabled = True
        self.writes_enabled = False
        self.live = True

        self._load_count = -1
        self._probe_latency = latency
        self._frame = 0
        self._position = {0: 0, 1: 0}
        self._held = {0: None, 1: None}

    def call(self, tool: str, **kwargs) -> dict:
        self.calls.append((tool, dict(kwargs)))
        handler = getattr(self, f"_tool_{tool}", None)
        if handler is None:
            raise AssertionError(f"unexpected/banned MCP tool called: {tool!r}")
        return handler(**kwargs)

    # ── tool handlers ────────────────────────────────────────────────────
    def _tool_run_lua(self, script: str) -> dict:
        if "training.set_enabled(false)" in script:
            self.training_enabled = False
        elif "training.set_enabled(true)" in script:
            self.training_enabled = True
        return {"ok": True, "output": ""}

    def _tool_enable_writes(self) -> dict:
        self.writes_enabled = True
        return {"ok": True, "writes_enabled": True}

    def _tool_pause(self) -> dict:
        return {"ok": True, "paused": True}

    def _tool_get_state(self) -> dict:
        # Real get_state has no "ok"/"error" wrapper -- just the snapshot.
        return {"frame_count": self._frame}

    def _tool_load_state(self, slot=None, path=None) -> dict:
        if not self.writes_enabled:
            # Matches the real server's write-gate short-circuit shape for
            # load_state: {"error": ...} with NO "ok" key at all.
            return {"error": "writes are locked; call enable_writes first"}
        self._load_count += 1
        if self.latencies is not None and self._load_count % 2 == 1:
            trial_num = self._load_count // 2
            idx = min(trial_num, len(self.latencies) - 1)
            self._probe_latency = self.latencies[idx]
        self._frame = 0
        self._position = {0: 0, 1: 0}
        self._held = {0: None, 1: None}
        return {"ok": True}

    def _tool_hold_buttons(self, buttons, port=0) -> dict:
        self._held[port] = buttons[0] if buttons else None
        return {"ok": True}

    def _tool_release_buttons(self, buttons=None, port=0) -> dict:
        self._held[port] = None
        return {"ok": True}

    def _tool_get_input(self, port=0) -> dict:
        # `asserted` vs `folded` (session.confirm_fold). No host loop here,
        # so the fold is instantaneous and the confirmation returns at once.
        mask = self._held[port] or ""
        return {"ok": True, "port": port,
                "asserted_mask": mask, "folded_mask": mask}

    def _tool_step(self) -> dict:
        # Synchronous `step`: it returns only once the frame is finished, and
        # reports that it landed (src/mcp/server.rs).
        self._frame += 1
        for port, direction in self._held.items():
            if (
                direction is not None
                and direction in self.live_directions
                and self._frame > self._probe_latency
            ):
                self._position[port] += 1
        return {
            "ok": True,
            "stepped": True,
            "landed": True,
            "frame_count": self._frame,
        }

    # ── observable / liveness callbacks for the test ────────────────────
    def position_of(self, port: int) -> int:
        return self._position[port]


def observable_p0(client: FakeClient):
    return client.position_of(0)


def always_live(client: FakeClient) -> bool:
    return client.live


class ZeroPointCalibrationTest(unittest.TestCase):
    def test_constant_latency_across_trials_returns_it(self):
        client = FakeClient(latency=3)
        result = zero_point_calibration(
            client, arena="0", observable_fn=observable_p0, liveness_fn=always_live,
        )
        # Divergence is first detected at frame (latency + 1): position only
        # moves once `frame > latency`.
        self.assertEqual(result.input_latency_frames, 4)
        self.assertEqual(result.trials, 5)
        self.assertEqual(result.samples, (4, 4, 4, 4, 4))
        self.assertEqual(result.walk_direction, "left")

    def test_varying_latency_across_trials_raises_not_averages(self):
        client = FakeClient(latencies=[3, 3, 3, 5, 3])
        with self.assertRaises(CalibrationError):
            zero_point_calibration(
                client, arena="0", observable_fn=observable_p0, liveness_fn=always_live,
            )

    def test_never_calls_press_buttons(self):
        client = FakeClient(latency=2)
        zero_point_calibration(
            client, arena="0", observable_fn=observable_p0, liveness_fn=always_live,
        )
        tool_names = {name for name, _ in client.calls}
        self.assertNotIn("press_buttons", tool_names)
        # Only the documented tool set was used.
        self.assertTrue(tool_names <= {
            "run_lua", "enable_writes", "load_state", "pause", "hold_buttons",
            "release_buttons", "step", "get_state", "get_input",
        })

    def test_disables_training_enforcement_before_measuring(self):
        client = FakeClient(latency=2)
        zero_point_calibration(
            client, arena="0", observable_fn=observable_p0, liveness_fn=always_live,
        )
        self.assertFalse(client.training_enabled)
        first_tool, _ = client.calls[0]
        self.assertEqual(first_tool, "run_lua")

    def test_corner_hazard_falls_back_to_the_next_direction(self):
        # "left" is walled off (never diverges); "right" works.
        client = FakeClient(latency=2, live_directions=("right",))
        result = zero_point_calibration(
            client, arena="0", observable_fn=observable_p0, liveness_fn=always_live,
            directions=("left", "right"),
        )
        self.assertEqual(result.walk_direction, "right")
        self.assertEqual(result.input_latency_frames, 3)

    def test_no_direction_diverges_raises(self):
        client = FakeClient(latency=2, live_directions=())
        with self.assertRaises(CalibrationError):
            zero_point_calibration(
                client, arena="0", observable_fn=observable_p0, liveness_fn=always_live,
                directions=("left", "right"),
            )

    def test_liveness_check_runs_after_every_load_state_and_raises_on_failure(self):
        client = FakeClient(latency=2)
        seen_checks = []

        def flaky_liveness(c):
            seen_checks.append(len(seen_checks))
            # Die on the 3rd liveness check (a load_state partway through).
            return len(seen_checks) < 3

        with self.assertRaises(CalibrationError):
            zero_point_calibration(
                client, arena="0", observable_fn=observable_p0,
                liveness_fn=flaky_liveness,
            )
        self.assertGreaterEqual(len(seen_checks), 3)

    def test_requires_at_least_five_trials(self):
        client = FakeClient(latency=2)
        with self.assertRaises(ValueError):
            zero_point_calibration(
                client, arena="0", observable_fn=observable_p0,
                liveness_fn=always_live, trials=3,
            )
        # Rejected before touching the client at all.
        self.assertEqual(client.calls, [])

    def test_arms_writes_before_the_first_load_state(self):
        # Regression: the real server's write-gate failure for load_state is
        # shaped {"error": ...} with NO "ok" key at all (unlike every other
        # tool's {"ok": false, "error": ...}) -- calibration must still
        # treat it as failure, and must arm writes so it never happens.
        client = FakeClient(latency=2)
        zero_point_calibration(
            client, arena="0", observable_fn=observable_p0, liveness_fn=always_live,
        )
        self.assertTrue(client.writes_enabled)
        tool_names_in_order = [name for name, _ in client.calls]
        self.assertIn("enable_writes", tool_names_in_order)
        self.assertLess(
            tool_names_in_order.index("enable_writes"),
            tool_names_in_order.index("load_state"),
        )

    def test_port_must_be_0_or_1(self):
        client = FakeClient(latency=2)
        with self.assertRaises(ValueError):
            zero_point_calibration(
                client, arena="0", observable_fn=observable_p0,
                liveness_fn=always_live, port=2,
            )


if __name__ == "__main__":
    unittest.main()
