"""Fit-time profile auto-resolution (A1 consolidation task): a v3 recording's
`.meta.json` sidecar already carries `family`/`port`/`profile_file`
(RECORDER_V3.md §1.3) -- `shadow_train.__main__._resolve_profile_for` uses
that to pick the right `library/<family>/<port>` profile BEFORE `build()`
runs, instead of trusting whatever RUSTRETRO_GAME_DIR happened to be at
process start (the footgun: same-family/wrong-port silently mislabels
attacks, since the old family-only guard never looks at `port`).

These tests mutate real process-wide state (RUSTRETRO_GAME_DIR,
`dataset`'s profile-derived module globals) via `_resolve_profile_for`, so
every test restores both in tearDown -- other test files in the same pytest
process assume the asurabld default.
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import tempfile
import unittest
from argparse import Namespace
from pathlib import Path

from shadow_train import dataset
from shadow_train import profile as _profile
from shadow_train.__main__ import _resolve_profile_for, cmd_fit

REPO_ROOT = _profile.REPO_ROOT
MK2_DIR = REPO_ROOT / "library" / "mk2"


def _mk2_row(frame: int, round_id: int, p1_input: int,
             me_x: int, opp_x: int, me_health: int, opp_health: int) -> dict:
    """A v3 row shaped like the real mk2/genesis recorder output (see
    shadow/recordings/mk2/*.jsonl) -- just the three §4.2-required fields."""
    return {
        "v": 3, "frame": frame, "round_id": round_id, "controllable": True,
        "p1_block": 1,
        "block1": {"char_id": 1, "health": me_health, "x": me_x},
        "block2": {"char_id": 3, "health": opp_health, "x": opp_x},
        "globals": {"menu_state": 0, "round_over": 0},
        "p1_input": p1_input, "p2_input": 0,
    }


def _write_recording(d: Path, name: str, family: str, port: str,
                      profile_file: str | None, extra_meta: dict | None = None) -> Path:
    n_frames = dataset.P * (dataset.K + 1 + 20)
    rows = [
        _mk2_row(i, 1, 0x80 if i % 16 == 0 else 0,
                 100 + i, 300 - i, 120 - (i % 20), 100 - (i % 15))
        for i in range(n_frames)
    ]
    path = d / f"{name}.jsonl"
    with open(path, "w") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")

    meta = {
        "format": "jsonl-v3", "family": family, "port": port,
        "profile_file": profile_file, "game": "mk2", "core": "fbneo",
        "style": None, "fps": 60, "anchor": "smaller_x",
        "blocks": {"block1": "0xFFB5F0", "block2": "0xFFB6E0", "stride": "0xF0"},
        "fighter_fields": [
            {"name": "char_id", "off": "0xE8", "size": 1},
            {"name": "health", "off": "0x32", "size": 1},
            {"name": "x", "off": "0xD8", "size": 2},
        ],
        "globals_recorded": [{"name": "menu_state", "size": 2},
                              {"name": "round_over", "size": 1}],
        "gate": [{"kind": "word_zero", "global": "menu_state"}],
        "calibration": {"X_SCALE": 128.0, "HEALTH_MAX": 120},
        "created": "2026-08-27T00:00:00Z",
    }
    if extra_meta:
        meta.update(extra_meta)
    (d / f"{name}.meta.json").write_text(json.dumps(meta))
    return path


class _ProfileEnvTestCase(unittest.TestCase):
    """Save/restore RUSTRETRO_GAME_DIR and dataset's profile-derived module
    globals around each test -- _resolve_profile_for mutates process-wide
    state that other test files assume is asurabld's."""

    def setUp(self):
        self._orig_env = os.environ.get("RUSTRETRO_GAME_DIR")

    def tearDown(self):
        if self._orig_env is None:
            os.environ.pop("RUSTRETRO_GAME_DIR", None)
        else:
            os.environ["RUSTRETRO_GAME_DIR"] = self._orig_env
        dataset.reload_profile()


class AutoResolutionTest(_ProfileEnvTestCase):
    """No --game, no RUSTRETRO_GAME_DIR: a fit resolves its profile from the
    recordings' own v3 sidecar alone."""

    def test_resolves_family_port_and_env_from_sidecar(self):
        os.environ.pop("RUSTRETRO_GAME_DIR", None)
        with tempfile.TemporaryDirectory() as d:
            path = _write_recording(Path(d), "genesis-rec", "mk2", "genesis",
                                     "genesis.profile.json")
            fit_args = Namespace(recordings=[path], out=Path(d) / "model", char=None, k=9)
            with contextlib.redirect_stdout(io.StringIO()):
                cmd_fit(fit_args)
            meta = json.loads((Path(d) / "model" / "meta.json").read_text())

        self.assertEqual(meta["family"], "mk2")
        self.assertEqual(meta["port"], "genesis")
        self.assertEqual(meta["attack_classes"], ["None", "HP", "LP", "HK", "LK", "Block"])
        self.assertEqual(
            Path(os.environ["RUSTRETRO_GAME_DIR"]).resolve(),
            (MK2_DIR / "genesis").resolve(),
        )
        # dataset's frozen-at-import globals actually moved to mk2 too.
        self.assertEqual(dataset.ATTACK_CLASSES, ["None", "HP", "LP", "HK", "LK", "Block"])


class DisagreementAbortTest(_ProfileEnvTestCase):
    """Recordings whose sidecars disagree on family/port abort naming both
    files, rather than silently picking one (or falling through to whatever
    RUSTRETRO_GAME_DIR/default happens to be)."""

    def test_conflicting_ports_abort_naming_files(self):
        os.environ.pop("RUSTRETRO_GAME_DIR", None)
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            p_genesis = _write_recording(d, "a-genesis", "mk2", "genesis",
                                          "genesis.profile.json")
            p_arcade = _write_recording(d, "b-arcade", "mk2", "arcade",
                                         "mk2.profile.json")
            fit_args = Namespace(recordings=[p_genesis, p_arcade],
                                  out=d / "model", char=None, k=9)
            with self.assertRaises(SystemExit) as ctx:
                with contextlib.redirect_stdout(io.StringIO()):
                    cmd_fit(fit_args)

        msg = str(ctx.exception)
        self.assertIn("a-genesis.jsonl", msg)
        self.assertIn("b-arcade.jsonl", msg)
        self.assertIn("disagree", msg)


class OverrideWithWarningTest(_ProfileEnvTestCase):
    """--game / RUSTRETRO_GAME_DIR is an explicit override: obeyed even when
    it disagrees with the recordings' own sidecars, but loudly warned."""

    def test_game_flag_overrides_and_warns(self):
        os.environ.pop("RUSTRETRO_GAME_DIR", None)
        with tempfile.TemporaryDirectory() as d:
            path = _write_recording(Path(d), "genesis-rec", "mk2", "genesis",
                                     "genesis.profile.json")
            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr):
                _resolve_profile_for([path], str(MK2_DIR))  # arcade is the default port

        warning = stderr.getvalue()
        self.assertIn("disagrees", warning)
        self.assertIn("mk2/arcade", warning)
        self.assertIn("mk2/genesis", warning)
        # the override won: RUSTRETRO_GAME_DIR is exactly what was passed,
        # and the active profile is now arcade's, not genesis's.
        self.assertEqual(os.environ["RUSTRETRO_GAME_DIR"], str(MK2_DIR))
        self.assertEqual(dataset._PROF.port, "arcade")

    def test_agreeing_override_is_silent(self):
        os.environ.pop("RUSTRETRO_GAME_DIR", None)
        with tempfile.TemporaryDirectory() as d:
            path = _write_recording(Path(d), "genesis-rec", "mk2", "genesis",
                                     "genesis.profile.json")
            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr):
                _resolve_profile_for([path], str(MK2_DIR / "genesis"))

        self.assertEqual(stderr.getvalue(), "")
        self.assertEqual(dataset._PROF.port, "genesis")


class V2UnchangedTest(_ProfileEnvTestCase):
    """Recordings with no v3 sidecar (jsonl-v2, or a v3 file missing its
    sidecar) don't participate in resolution at all -- today's behavior
    (loaded profile / asurabld default) is untouched."""

    def test_v2_recording_leaves_env_and_profile_alone(self):
        os.environ.pop("RUSTRETRO_GAME_DIR", None)
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            # A v2-shaped file (has "block1", no "v":3) and deliberately NO
            # .meta.json sidecar at all.
            path = d / "v2-rec.jsonl"
            path.write_text(json.dumps({
                "frame": 0, "round_id": 1, "controllable": True, "p1_block": 1,
                "block1": {"x": 100, "char_id": 1, "health": 200},
                "block2": {"x": 200, "char_id": 2, "health": 200},
                "gate": {}, "p1_input": 0, "p2_input": 0,
            }) + "\n")

            _resolve_profile_for([path], None)

        self.assertIsNone(os.environ.get("RUSTRETRO_GAME_DIR"))
        self.assertEqual(dataset._PROF.family, "asurabld")


if __name__ == "__main__":
    unittest.main()
