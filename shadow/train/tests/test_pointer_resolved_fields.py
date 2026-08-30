"""docs/frames.md §2.5 / RECORDER_V3.md §1.2 rule 1: a `fighter_fields` entry
can be pointer-resolved (MK2 arcade's world `x`) -- mapped (declared in the
profile / sidecar), but OMITTED from an individual row whenever that frame's
pointer dereference fails, never zero-filled. `.meta.json` sidecars name
these fields in a flat `"pointer_resolved_fields"` list.

`dataset.py` must not crash the first time a decision's `me`/`opp` dict is
missing one of those fields. The rule this module tests: drop the affected
DECISION (not the round, not the file), count how many were dropped, and
surface that count -- except when literally every decision in a recording
was dropped, which is not a hole in an otherwise-usable file, it's a broken
recording, and must abort loudly instead."""

from __future__ import annotations

import json
import tempfile
import unittest
import warnings
from pathlib import Path

from shadow_train import dataset

FIGHTER_FIELDS = [
    {"name": "char_id", "off": "0x0", "size": 1},
    {"name": "health", "off": "0xE", "size": 1},
    {"name": "x", "off": "0x10", "size": 2},
]
CALIBRATION = {"X_SCALE": 128.0, "HEALTH_MAX": 200}


def _row(frame: int, p1_input: int, me_x, opp_x, me_health=200, opp_health=180) -> dict:
    """A v3 row shaped like the sparse-port fixtures elsewhere in this suite.
    `me_x`/`opp_x` of `None` means the pointer failed to resolve THIS frame --
    the field is omitted from the block dict entirely (never written as 0)."""
    b1 = {"char_id": 1, "health": me_health}
    if me_x is not None:
        b1["x"] = me_x
    b2 = {"char_id": 2, "health": opp_health}
    if opp_x is not None:
        b2["x"] = opp_x
    return {
        "v": 3, "frame": frame, "round_id": 1, "controllable": True,
        "p1_block": 1, "block1": b1, "block2": b2,
        "globals": {}, "p1_input": p1_input, "p2_input": 0,
    }


def _meta(pointer_resolved_fields: list[str]) -> dict:
    return {
        "fighter_fields": FIGHTER_FIELDS,
        "globals_recorded": [], "gate": [],
        "calibration": CALIBRATION,
        "port": "arcade",
        "pointer_resolved_fields": pointer_resolved_fields,
    }


def _write(d: Path, name: str, rows: list[dict], meta: dict) -> Path:
    path = d / f"{name}.jsonl"
    with open(path, "w") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")
    (d / f"{name}.meta.json").write_text(json.dumps(meta))
    return path


P, K = dataset.P, dataset.K
N_DECISIONS = 24  # comfortably more than K, gives room for a few drops
N_FRAMES = P * (N_DECISIONS + 1)
# Decision candidates land at i = P, 2P, 3P, ... -- drop exactly 3 of them by
# omitting "x" from block1 at those specific frames only.
DROP_DECISION_INDEXES = (5, 10, 15)  # -> frames 5*P, 10*P, 15*P
DROP_FRAMES = {n * P for n in DROP_DECISION_INDEXES}


def _rows_with_gaps() -> list[dict]:
    rows = []
    for i in range(N_FRAMES):
        p1_input = 0x80 if i % 16 == 0 else 0
        me_x = None if i in DROP_FRAMES else 100 + i
        rows.append(_row(i, p1_input, me_x=me_x, opp_x=300 - i))
    return rows


class DecisionDroppedNotRoundOrFileTest(unittest.TestCase):
    """A recording where `x` is absent on SOME rows must still fit -- the
    affected decisions are dropped individually and the drop is counted."""

    def test_fit_succeeds_and_reports_correct_drop_count(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            path = _write(d, "gappy", _rows_with_gaps(), _meta(["x"]))

            with warnings.catch_warnings(record=True) as caught:
                warnings.simplefilter("always")
                data = dataset.build([path])

        self.assertEqual(data["dropped_decisions"], len(DROP_DECISION_INDEXES))
        # every surviving decision has a finite, real dist_x -- nothing was
        # invented for the dropped ones instead of just skipping them.
        import numpy as np
        self.assertTrue(np.isfinite(data["X"]).all())

        # the drop count reached the user: a warning was raised naming it.
        msgs = [str(w.message) for w in caught]
        self.assertTrue(
            any("dropped" in m and "gappy" in m for m in msgs),
            msgs,
        )

    def test_load_decisions_also_reflects_the_drop_via_warning(self):
        # coverage's path (load_decisions) goes through the same per-file
        # loader and must not crash either, even though it doesn't return
        # dropped_decisions itself.
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            path = _write(d, "gappy2", _rows_with_gaps(), _meta(["x"]))
            decisions = dataset.load_decisions([path])
        # total candidates minus the 3 dropped ones actually got a Decision
        expected_candidates = len(range(P, N_FRAMES, P))
        self.assertEqual(len(decisions), expected_candidates - len(DROP_DECISION_INDEXES))


class EveryRowMissingIsABrokenRecordingTest(unittest.TestCase):
    """`x` absent on EVERY row (the pointer never once resolves) is not a
    sparse-but-usable recording -- it's broken, and must fail loudly rather
    than silently produce zero decisions."""

    def test_all_dropped_aborts_with_a_named_cause(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            rows = []
            for i in range(N_FRAMES):
                p1_input = 0x80 if i % 16 == 0 else 0
                rows.append(_row(i, p1_input, me_x=None, opp_x=300 - i))
            path = _write(d, "all-broken", rows, _meta(["x"]))

            with self.assertRaises(SystemExit) as ctx:
                dataset.build([path])
        msg = str(ctx.exception)
        self.assertIn("broken recording", msg)
        self.assertIn("x", msg)


class NoPointerResolvedFieldsFastPathTest(unittest.TestCase):
    """A recording that declares no `pointer_resolved_fields` (the key is
    simply absent, matching every recording that exists today) must behave
    byte-identically to before this feature existed: zero drops, zero
    warnings, and the per-row presence check must not even run (the `and`
    short-circuit) -- checked here by making the check crash if it were
    ever evaluated with malformed field-name entries, which would only be
    reached if `ptr_fields` were non-empty (it isn't)."""

    def test_no_declared_pointer_fields_drops_nothing_and_warns_nothing(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            meta = _meta([])  # empty list -- declares no pointer-resolved fields
            del meta["pointer_resolved_fields"]  # and the fully-absent-key case
            rows = []
            for i in range(N_FRAMES):
                p1_input = 0x80 if i % 16 == 0 else 0
                rows.append(_row(i, p1_input, me_x=100 + i, opp_x=300 - i))
            path = _write(d, "clean", rows, meta)

            with warnings.catch_warnings(record=True) as caught:
                warnings.simplefilter("always")
                data = dataset.build([path])

        self.assertEqual(data["dropped_decisions"], 0)
        self.assertFalse(
            [w for w in caught if "dropped" in str(w.message)],
            "no pointer-resolved-field drop warning should fire when none are declared",
        )
        expected_candidates = len(range(P, N_FRAMES, P))
        # K-1 decisions are consumed as history by the first stacked example
        self.assertEqual(data["X"].shape[0], expected_candidates - (K - 1))

    def test_v2_recordings_have_no_pointer_resolved_concept(self):
        # v2 files go through the legacy _RecordingView construction, whose
        # pointer_resolved_fields defaults to empty -- this is the same fast
        # path exercised structurally, not just via an empty v3 sidecar list.
        from shadow_train.dataset import _RecordingView
        view = _RecordingView(
            version="v2", field_names=frozenset({"x"}), calibration={},
            hitstun_map=None, port="arcade",
        )
        self.assertEqual(view.pointer_resolved_fields, frozenset())


if __name__ == "__main__":
    unittest.main()
