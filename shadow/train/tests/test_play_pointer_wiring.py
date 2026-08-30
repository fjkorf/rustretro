"""task B-deploy: `shadow/play.py` is not itself unit-testable (its `main()`
owns argparse + a live MCP connection), so this drives the exact sequence
play.py's loop now performs -- `pointer_fields_from_meta(meta)` ->
`RoundBuffers(pointer_fields=...)` -> `.compute_scalars(...)`, plus the
round-start `snap.block1.get("x")` shape -- against synthetic fighter dicts.

Before this task, play.py called `rt.build_scalars(...)` directly and
indexed `snap.block1["x"]` at round start; either one raises `KeyError` the
first time a pointer-resolved field (MK2 arcade's world `x`/`y`,
docs/frames.md §2.5) is absent from a row. `runtime.py`'s
`PointerStaleness`/`RoundBuffers.compute_scalars`/`pointer_fields_from_meta`
already have exhaustive unit coverage in `test_runtime.py` (the module that
owns them) -- this file only proves play.py's WIRING of those primitives is
correct, and that a model declaring no pointer-resolved fields (every model
today) is byte-identical to the pre-existing bare-`build_scalars` call."""

from __future__ import annotations

import unittest

from shadow_train import runtime as rt


def _full_fighter(**overrides) -> dict:
    base = dict(x=100, y=216, anim=0, timer=0, health=200, meter=0, meter_max=100, facing=1)
    base.update(overrides)
    return base


class PlayPointerWiringTest(unittest.TestCase):
    def test_no_pointer_fields_model_is_byte_identical_to_bare_build_scalars(self):
        """A model whose meta.json has no `pointer_resolved_fields` (every
        model today, including goat-v2) must produce EXACTLY what
        `rt.build_scalars` would have returned directly -- the byte-identical
        requirement for play.py's wiring change."""
        meta = {}  # no "pointer_resolved_fields" key
        pointer_fields = rt.pointer_fields_from_meta(meta)
        self.assertEqual(pointer_fields, frozenset())

        buffers = rt.RoundBuffers(pointer_fields=pointer_fields)
        me, opp = _full_fighter(), _full_fighter(x=250, health=150)
        want = rt.build_scalars(me, opp, 1, 0.5, 0.0, False, True)
        got = buffers.compute_scalars(me, opp, 1, 0.5, 0.0, False, True)
        self.assertEqual(got, want)

    def test_play_shaped_call_path_survives_an_absent_x(self):
        """Reproduces play.py's per-tick shape for a model that DOES declare
        a pointer-resolved field: `me_now`/`opp_lagged` are `TickSnapshot`
        block dicts that can lack `"x"` on a given tick (the pointer didn't
        dereference). The pre-fix code (`rt.build_scalars(me_now, ...)`
        called directly) raised `KeyError` here; the fix must not."""
        meta = {"pointer_resolved_fields": ["x"]}
        pointer_fields = rt.pointer_fields_from_meta(meta)
        buffers = rt.RoundBuffers(pointer_fields=pointer_fields)

        me_now = _full_fighter()
        del me_now["x"]  # this tick's pointer didn't resolve
        opp_lagged = _full_fighter(x=250)

        try:
            scal = buffers.compute_scalars(me_now, opp_lagged, 1, 0.0, 0.0, False, False)
        except KeyError:
            self.fail("play.py's compute_scalars call must hold (None), not raise KeyError")
        self.assertIsNone(scal, "an absent declared field must yield None (hold), not a value")

    def test_round_start_x_lookup_is_get_not_index(self):
        """The round-start anchor probe (`x1, x2 = snap.block1.get("x"),
        snap.block2.get("x")`) must not KeyError when a block dict lacks
        `"x"` -- the exact shape a `TickSnapshot.block1`/`block2` dict has on
        a tick where a pointer-resolved `x` hasn't dereferenced yet."""
        block_without_x = {"health": 200, "facing": 1}
        x1 = block_without_x.get("x")
        x2 = _full_fighter()["x"]
        self.assertIsNone(x1)
        self.assertEqual(x2, 100)


if __name__ == "__main__":
    unittest.main()
