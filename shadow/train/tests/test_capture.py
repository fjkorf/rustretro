"""Unit tests for `shadow_train.capture` — the framebuffer-capture -> GIF
pipeline (CLAUDE.md's A2 brief: MK2 combat demo GIFs).

The duration-dithering math and caption-range logic are pure and tested
directly (no emulator). `capture_slot`'s pause/arm/step/grab protocol is
tested against a small in-process fake `McpClient` that models the three
facts the protocol depends on: `load_state(pause_after=True)` reports
`paused: true` atomically, `play_inputs(start)` only ARMS (frame 0 needs a
`step()` after it), and `step()` is synchronous with a `landed` flag. A live
smoke test against a real headless instance (port 4027) is run separately,
not as part of this suite.
"""

from __future__ import annotations

import contextlib
import tempfile
import unittest
from pathlib import Path

from PIL import Image

from shadow_train.capture import (
    Caption,
    CaptureError,
    assemble_gif,
    capture_slot,
    captions_for_frame,
    frame_durations_cs,
)


# ── frame_durations_cs ───────────────────────────────────────────────────


class FrameDurationsTests(unittest.TestCase):
    def test_empty(self):
        self.assertEqual(frame_durations_cs(0), [])

    def test_length_matches_frame_count(self):
        for n in (1, 2, 48, 200):
            self.assertEqual(len(frame_durations_cs(n)), n)

    def test_total_matches_expected_playback_time(self):
        """The whole point: total duration must track n_frames/fps, not
        drift the way a constant-20ms-per-frame scheme would (that runs
        MK2's ~54.71Hz content about 9% slow)."""
        for n in (1, 10, 48, 137, 600):
            fps = 54.706
            durations = frame_durations_cs(n, fps=fps)
            expected_total_cs = round(n * 100.0 / fps)
            self.assertEqual(sum(durations), expected_total_cs)

    def test_reptile_hp_g45_slot_duration_is_close_to_878ms(self):
        """The concrete slot this brief's smoke test uses: 48 frames native
        MK2 (~54.706 Hz) should total close to 48 * 18.28ms ~= 878ms."""
        durations_cs = frame_durations_cs(48, fps=54.706)
        total_ms = sum(durations_cs) * 10
        self.assertAlmostEqual(total_ms, 878, delta=15)

    def test_every_duration_at_least_one_centisecond(self):
        # A 0 is not a legal GIF duration; several viewers silently
        # substitute their own default (often 100ms) for it.
        for n in (1, 5, 48, 1000):
            self.assertTrue(all(d >= 1 for d in frame_durations_cs(n)))

    def test_no_single_frame_drifts_far_from_the_mean(self):
        # Bresenham-style dithering should keep every frame within about one
        # centisecond of round(cs_per_frame) -- never batching the error.
        n, fps = 48, 54.706
        cs_per_frame = 100.0 / fps
        for d in frame_durations_cs(n, fps=fps):
            self.assertLessEqual(abs(d - cs_per_frame), 1.5)

    def test_constant_20ms_would_run_slow_by_about_nine_percent(self):
        # Documents the failure mode this function exists to avoid.
        n, fps = 48, 54.706
        naive_total_ms = n * 20
        dithered_total_ms = sum(frame_durations_cs(n, fps=fps)) * 10
        expected_total_ms = n * 1000.0 / fps
        self.assertGreater(naive_total_ms, expected_total_ms * 1.05)
        self.assertAlmostEqual(dithered_total_ms, expected_total_ms, delta=15)

    def test_rejects_non_positive_fps(self):
        with self.assertRaises(ValueError):
            frame_durations_cs(10, fps=0)


# ── captions ──────────────────────────────────────────────────────────────


class CaptionRangeTests(unittest.TestCase):
    def test_from_dict(self):
        c = Caption.from_dict({"start_frame": 3, "end_frame": 9, "text": "far HK hits (+7)"})
        self.assertEqual((c.start_frame, c.end_frame, c.text), (3, 9, "far HK hits (+7)"))

    def test_inclusive_boundaries(self):
        c = Caption(start_frame=5, end_frame=8, text="x")
        for f in (5, 6, 7, 8):
            self.assertEqual(captions_for_frame([c], f), ["x"])
        for f in (4, 9):
            self.assertEqual(captions_for_frame([c], f), [])

    def test_no_captions_active_anywhere(self):
        self.assertEqual(captions_for_frame([], 0), [])

    def test_multiple_active_captions_stack_in_order(self):
        a = Caption(0, 10, "top line")
        b = Caption(3, 6, "second line")
        self.assertEqual(captions_for_frame([a, b], 4), ["top line", "second line"])
        self.assertEqual(captions_for_frame([a, b], 8), ["top line"])

    def test_single_frame_range(self):
        c = Caption(start_frame=7, end_frame=7, text="only")
        self.assertEqual(captions_for_frame([c], 7), ["only"])
        self.assertEqual(captions_for_frame([c], 6), [])
        self.assertEqual(captions_for_frame([c], 8), [])


# ── assemble_gif (Pillow round trip, no emulator) ─────────────────────────


def _write_png(path: Path, color: tuple) -> Path:
    Image.new("RGB", (16, 12), color=color).save(path)
    return path


class AssembleGifTests(unittest.TestCase):
    def test_round_trips_frame_count_and_upscale(self):
        tmp = Path(self.enterContext(_tmp_dir()))
        paths = [str(_write_png(tmp / f"frame_{i:04d}.png", (i * 10 % 255, 0, 0))) for i in range(5)]
        out = tmp / "out.gif"
        assemble_gif(paths, out, fps_native=54.706, scale=3)

        with Image.open(out) as gif:
            self.assertEqual(gif.n_frames, 5)
            self.assertEqual(gif.size, (16 * 3, 12 * 3))

    def test_duration_sum_matches_dithered_total(self):
        tmp = Path(self.enterContext(_tmp_dir()))
        paths = [str(_write_png(tmp / f"frame_{i:04d}.png", (0, i * 5 % 255, 0))) for i in range(48)]
        out = tmp / "out.gif"
        assemble_gif(paths, out, fps_native=54.706, scale=1)

        total_ms = 0
        with Image.open(out) as gif:
            for i in range(gif.n_frames):
                gif.seek(i)
                total_ms += gif.info["duration"]
        expected_ms = round(48 * 100.0 / 54.706) * 10
        # Pillow snaps duration to the nearest 10ms tick on write; our own
        # dithering already produces centisecond-granular values, so this
        # should match exactly.
        self.assertEqual(total_ms, expected_ms)

    def test_caption_banner_does_not_crash_and_darkens_the_band(self):
        tmp = Path(self.enterContext(_tmp_dir()))
        # A bright frame so a black caption banner is visually detectable.
        paths = [str(_write_png(tmp / f"frame_{i:04d}.png", (255, 255, 255))) for i in range(3)]
        out = tmp / "out.gif"
        assemble_gif(
            paths, out, scale=4,
            captions=[{"start_frame": 0, "end_frame": 2, "text": "test caption"}],
        )
        with Image.open(out) as gif:
            gif.seek(0)
            frame = gif.convert("RGB")
            bottom_row = [frame.getpixel((x, frame.height - 1)) for x in range(frame.width)]
            self.assertTrue(any(sum(px) < 200 for px in bottom_row), "caption banner not drawn")

    def test_rejects_empty_frame_list(self):
        with self.assertRaises(ValueError):
            assemble_gif([], "/tmp/should-not-be-written.gif")

    def test_rejects_scale_below_one(self):
        with self.assertRaises(ValueError):
            assemble_gif(["/nonexistent.png"], "/tmp/x.gif", scale=0)


@contextlib.contextmanager
def _tmp_dir():
    with tempfile.TemporaryDirectory() as d:
        yield d


# ── capture_slot protocol, against a fake client ──────────────────────────


class FakeMcpClient:
    """Models exactly the three protocol facts `capture_slot` depends on:
    `load_state(pause_after=True)` reports `paused: true` atomically,
    `play_inputs(start)` only ARMS (no frame lands until the next `step()`),
    and `step()` is synchronous and reports `landed`. Screens are distinct
    per grab so a test can tell frames apart by content.
    """

    def __init__(self, *, n_frames: int = 4):
        self.calls: list[tuple] = []
        self.n_frames = n_frames
        self._playing = False
        self._grab_count = 0

    def call(self, tool: str, **kwargs) -> dict:
        self.calls.append((tool, dict(kwargs)))
        if tool == "enable_writes":
            return {"ok": True}
        if tool == "load_state":
            assert kwargs.get("pause_after") is True
            return {"ok": True, "paused": True}
        if tool == "play_inputs":
            if kwargs.get("action") == "start":
                self._playing = True
                return {"ok": True, "name": kwargs.get("name"), "frames": self.n_frames}
            if kwargs.get("action") == "stop":
                was_playing, self._playing = self._playing, False
                if not was_playing:
                    return {"ok": False, "error": "no playback active"}
                return {"ok": True, "stopped": True}
            raise AssertionError(f"unexpected play_inputs action {kwargs!r}")
        if tool == "step":
            return {"ok": True, "stepped": True, "landed": True, "frame_count": 1}
        raise AssertionError(f"unexpected tool {tool!r}")

    def read_resource(self, uri: str) -> bytes:
        assert uri == "app://screen"
        self._grab_count += 1
        return f"PNG-{self._grab_count}".encode()


class FakeLoadNotPaused(FakeMcpClient):
    def call(self, tool, **kwargs):
        if tool == "load_state":
            self.calls.append((tool, dict(kwargs)))
            return {"ok": True, "paused": False}
        return super().call(tool, **kwargs)


class FakeStepNeverLands(FakeMcpClient):
    def call(self, tool, **kwargs):
        if tool == "step":
            self.calls.append((tool, dict(kwargs)))
            return {"ok": False, "stepped": True, "landed": False, "error": "timed out"}
        return super().call(tool, **kwargs)


class CaptureSlotTests(unittest.TestCase):
    def test_writes_one_preroll_plus_n_frames_plus_one_postroll(self):
        tmp = Path(self.enterContext(_tmp_dir()))
        client = FakeMcpClient(n_frames=6)
        paths = capture_slot(client, slot_name="reptile-hp-g45", state_path="shadow/arenas/mk2/gap-45.state", out_dir=tmp)
        self.assertEqual(len(paths), 6 + 2)
        for p in paths:
            self.assertTrue(Path(p).is_file())

    def test_frames_have_distinct_content_in_order(self):
        tmp = Path(self.enterContext(_tmp_dir()))
        client = FakeMcpClient(n_frames=3)
        paths = capture_slot(client, slot_name="s", state_path="a.state", out_dir=tmp)
        contents = [Path(p).read_bytes() for p in paths]
        self.assertEqual(len(contents), len(set(contents)), "frames should be distinct grabs")

    def test_protocol_order(self):
        tmp = Path(self.enterContext(_tmp_dir()))
        client = FakeMcpClient(n_frames=2)
        capture_slot(client, slot_name="s", state_path="a.state", out_dir=tmp)
        tools = [c[0] for c in client.calls]
        self.assertEqual(tools[0], "enable_writes")
        self.assertEqual(tools[1], "load_state")
        # a defensive stop, then arm, then step x (n_frames + 1), then a
        # final stop.
        self.assertIn("play_inputs", tools[2:4])
        start_idx = next(i for i, (t, kw) in enumerate(client.calls) if t == "play_inputs" and kw.get("action") == "start")
        start_kwargs = client.calls[start_idx][1]
        self.assertEqual(start_kwargs["port"], "both")
        self.assertEqual(start_kwargs["trigger"], "manual")
        self.assertEqual(start_kwargs["name"], "s")
        step_count = sum(1 for t, _ in client.calls if t == "step")
        self.assertEqual(step_count, 2 + 1)  # n_frames + the release step
        self.assertEqual(tools[-1], "play_inputs")
        self.assertEqual(client.calls[-1][1]["action"], "stop")

    def test_load_state_uses_atomic_pause_after(self):
        tmp = Path(self.enterContext(_tmp_dir()))
        client = FakeMcpClient(n_frames=1)
        capture_slot(client, slot_name="s", state_path="my/arena.state", out_dir=tmp)
        load_call = next(kw for t, kw in client.calls if t == "load_state")
        self.assertEqual(load_call["path"], "my/arena.state")
        self.assertIs(load_call["pause_after"], True)

    def test_refuses_when_load_state_does_not_report_paused(self):
        tmp = Path(self.enterContext(_tmp_dir()))
        client = FakeLoadNotPaused(n_frames=1)
        with self.assertRaises(CaptureError):
            capture_slot(client, slot_name="s", state_path="a.state", out_dir=tmp)

    def test_refuses_when_a_step_does_not_land(self):
        tmp = Path(self.enterContext(_tmp_dir()))
        client = FakeStepNeverLands(n_frames=1)
        with self.assertRaises(CaptureError):
            capture_slot(client, slot_name="s", state_path="a.state", out_dir=tmp)

    def test_stops_playback_even_when_a_step_fails(self):
        tmp = Path(self.enterContext(_tmp_dir()))
        client = FakeStepNeverLands(n_frames=1)
        with self.assertRaises(CaptureError):
            capture_slot(client, slot_name="s", state_path="a.state", out_dir=tmp)
        # the try/finally must still have issued a stop after the failure
        stop_calls = [kw for t, kw in client.calls if t == "play_inputs" and kw.get("action") == "stop"]
        self.assertTrue(stop_calls)


if __name__ == "__main__":
    unittest.main()
