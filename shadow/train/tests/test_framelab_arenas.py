from __future__ import annotations

import json
import struct
import tempfile
import unittest
from pathlib import Path

from shadow_train.framelab.arenas import (
    ArenaLivenessError,
    ArenaReproductionError,
    build_gap_ladder,
    build_gap_ladder_arena,
    compute_facing,
    measure_gap_px,
    read_object_x,
)

# Fake-world layout -- arbitrary addresses, internally consistent with the
# object-pointer decode `arenas.py` implements (docs/frames.md §5):
#   obj = (u32_le(block - 0x0C) - 0x01000000) >> 3
BLOCK1 = 0x1000
BLOCK2 = 0x2000
OBJ1 = 0x100
OBJ2 = 0x200
OBJ_BASE = 0x01000000


def _ptr_bytes(obj_addr: int) -> bytes:
    return struct.pack("<I", OBJ_BASE + (obj_addr << 3))


def _invalid_ptr_bytes() -> bytes:
    return struct.pack("<I", 0)  # outside 0x01000000..0x01400000 -- "no object"


class FakeClient:
    """A deterministic stand-in for `McpClient`, simulating just enough of
    MK2 arcade's memory shape for `arenas.py`'s object-pointer protocol:
    two fighter blocks, each with a pointer to a `0x42`-stride object-pool
    entry carrying world X and a char-id cross-check byte. Implements
    exactly `.call(tool, **kwargs)`, `arenas.py`'s only client contract.

    `live_ports`: ports whose held direction actually moves their fighter's
    object X (simulates a 1P-vs-CPU rig where the CPU's pushback overrides a
    held direction on the dead port -- docs/frames.md §3's precondition
    about bad rigs).

    `walk_speed`: pixels of X moved per confirmed `step()` while a live
    port holds "left"/"right".

    `corrupt_block1_cid_at_step`: if set, the OBJ1 (`block1`'s object-pool
    entry) char-id cross-check byte is permanently corrupted (made to
    disagree with `block1`'s own char id) the moment the client's total
    step counter reaches this value -- and stays corrupted through any
    later `save_state`/`load_state` round-trip, exactly like a real
    mid-fight staleness event would (the corruption lives IN THE SAVED
    BYTES, so it reproduces deterministically rather than "healing" on
    reload).

    `jitter_reload_px`: if set, reloading any state saved via `save_state`
    (i.e. NOT the pristine "base" arena) perturbs `block1`'s object X by
    this many pixels immediately after the load -- simulates an inexact
    save/reload round-trip so the reproduction check can be exercised.
    """

    def __init__(
        self,
        *,
        char_id1: int = 5,
        char_id2: int = 9,
        x1: int = 469,
        x2: int = 661,
        live_ports=(0, 1),
        walk_speed: int = 3,
        ptr1_valid: bool = True,
        ptr2_valid: bool = True,
        corrupt_block1_cid_at_step: int | None = None,
        jitter_reload_px: int | None = None,
    ):
        self.char_id1 = char_id1
        self.char_id2 = char_id2
        self.live_ports = set(live_ports)
        self.walk_speed = walk_speed
        self.corrupt_block1_cid_at_step = corrupt_block1_cid_at_step
        self.jitter_reload_px = jitter_reload_px

        self.calls: list[tuple[str, dict]] = []
        self.training_enabled = True
        self.writes_enabled = False
        self.frame_count = 0
        self._steps_total = 0
        self._held: dict[int, str | None] = {0: None, 1: None}

        self.mem: dict[int, int] = {}
        self._write(BLOCK1, bytes([char_id1]))
        self._write(BLOCK2, bytes([char_id2]))
        self._write(BLOCK1 - 0x0C, _ptr_bytes(OBJ1) if ptr1_valid else _invalid_ptr_bytes())
        self._write(BLOCK2 - 0x0C, _ptr_bytes(OBJ2) if ptr2_valid else _invalid_ptr_bytes())
        self._write(OBJ1 + 0x3E, bytes([char_id1]))
        self._write(OBJ2 + 0x3E, bytes([char_id2]))
        self._write(OBJ1 + 0x12, struct.pack("<H", x1))
        self._write(OBJ2 + 0x12, struct.pack("<H", x2))

        # Named/pristine save slots, keyed exactly like real load_state
        # (slot ints as their str form, or an explicit path string).
        self.states: dict[str, dict[int, int]] = {"base": dict(self.mem)}

    # ── raw memory helpers ──────────────────────────────────────────────
    def _write(self, addr: int, data: bytes) -> None:
        for i, b in enumerate(data):
            self.mem[addr + i] = b

    def _read(self, addr: int, length: int) -> bytes:
        return bytes(self.mem.get(addr + i, 0) for i in range(length))

    # ── MCP surface ──────────────────────────────────────────────────────
    def call(self, tool: str, **kwargs) -> dict:
        self.calls.append((tool, dict(kwargs)))
        handler = getattr(self, f"_tool_{tool}", None)
        if handler is None:
            raise AssertionError(f"unexpected/banned MCP tool called: {tool!r}")
        return handler(**kwargs)

    def _tool_run_lua(self, script: str) -> dict:
        if "training.set_enabled(false)" in script:
            self.training_enabled = False
        return {"ok": True, "output": ""}

    def _tool_enable_writes(self) -> dict:
        self.writes_enabled = True
        return {"ok": True, "writes_enabled": True}

    def _tool_resume(self) -> dict:
        return {"ok": True, "paused": False}

    def _tool_pause(self) -> dict:
        return {"ok": True, "paused": True}

    def _tool_get_state(self) -> dict:
        return {"frame_count": self.frame_count}

    def _key_for(self, slot=None, path=None) -> str:
        return str(slot) if path is None else path

    def _tool_load_state(self, slot=None, path=None) -> dict:
        if not self.writes_enabled:
            return {"error": "writes are locked; call enable_writes first"}
        key = self._key_for(slot, path)
        if key not in self.states:
            return {"ok": False, "error": f"no such state: {key!r}"}
        self.mem = dict(self.states[key])
        self._held = {0: None, 1: None}
        if self.jitter_reload_px and key != "base":
            x = struct.unpack("<H", self._read(OBJ1 + 0x12, 2))[0]
            self._write(OBJ1 + 0x12, struct.pack("<H", x + self.jitter_reload_px))
        return {"ok": True, "op": "load"}

    def _tool_save_state(self, slot=None, path=None) -> dict:
        key = self._key_for(slot, path)
        self.states[key] = dict(self.mem)
        return {"ok": True, "op": "save", "bytes": len(self.mem)}

    def _tool_hold_buttons(self, buttons, port=0) -> dict:
        self._held[port] = buttons[0] if buttons else None
        return {"ok": True}

    def _tool_release_buttons(self, buttons=None, port=0) -> dict:
        self._held[port] = None
        return {"ok": True}

    def _tool_step(self) -> dict:
        self.frame_count += 1
        self._steps_total += 1
        for port, direction in self._held.items():
            if direction is None or port not in self.live_ports:
                continue
            obj_off = OBJ1 + 0x12 if port == 0 else OBJ2 + 0x12
            x = struct.unpack("<H", self._read(obj_off, 2))[0]
            delta = self.walk_speed if direction == "right" else -self.walk_speed
            self._write(obj_off, struct.pack("<H", max(0, x + delta)))
        if (
            self.corrupt_block1_cid_at_step is not None
            and self._steps_total == self.corrupt_block1_cid_at_step
        ):
            self._write(OBJ1 + 0x3E, bytes([self.char_id1 ^ 0xFF]))
        return {"ok": True}

    def _tool_read_memory(self, addr: int, len: int) -> dict:  # noqa: A002
        return {"hex": self._read(addr, len).hex()}


class ObjectPointerHelpersTest(unittest.TestCase):
    """The low-level pieces `arenas.py` builds everything on: §5's formula,
    the staleness cross-check, and null-not-zero on failure."""

    def test_read_object_x_resolves_a_valid_pointer(self):
        c = FakeClient(x1=469, x2=661)
        self.assertEqual(read_object_x(c, BLOCK1), 469)
        self.assertEqual(read_object_x(c, BLOCK2), 661)

    def test_read_object_x_is_none_when_pointer_out_of_range(self):
        c = FakeClient(ptr2_valid=False)
        self.assertIsNone(read_object_x(c, BLOCK2))

    def test_read_object_x_is_none_on_char_id_staleness_mismatch(self):
        c = FakeClient()
        c._write(OBJ1 + 0x3E, bytes([c.char_id1 ^ 0xFF]))  # corrupt cross-check
        self.assertIsNone(read_object_x(c, BLOCK1))

    def test_measure_gap_px_is_none_not_zero_when_unmeasurable(self):
        c = FakeClient(ptr1_valid=False)
        gap = measure_gap_px(c, BLOCK1, BLOCK2)
        self.assertIsNone(gap)
        self.assertNotEqual(gap, 0)

    def test_measure_gap_px_computes_absolute_distance(self):
        c = FakeClient(x1=469, x2=661)
        self.assertEqual(measure_gap_px(c, BLOCK1, BLOCK2), 192)

    def test_compute_facing_left_right(self):
        c = FakeClient(x1=469, x2=661)
        self.assertEqual(
            compute_facing(c, BLOCK1, BLOCK2),
            {"block1_side": "left", "block2_side": "right"},
        )

    def test_compute_facing_swapped_sides(self):
        c = FakeClient(x1=800, x2=200)
        self.assertEqual(
            compute_facing(c, BLOCK1, BLOCK2),
            {"block1_side": "right", "block2_side": "left"},
        )

    def test_compute_facing_unknown_when_x_unmeasurable(self):
        c = FakeClient(ptr1_valid=False)
        self.assertEqual(
            compute_facing(c, BLOCK1, BLOCK2),
            {"block1_side": None, "block2_side": None},
        )


class BuildGapLadderArenaTest(unittest.TestCase):
    def test_happy_path_sidecar_shape_and_reload(self):
        c = FakeClient(char_id1=5, char_id2=9, x1=469, x2=661, walk_speed=3)
        with tempfile.TemporaryDirectory() as tmp:
            out_dir = Path(tmp)
            result = build_gap_ladder_arena(
                c, base_arena="base", out_dir=out_dir, k=10,
                walk_port=0, walk_direction="right",
                block1_addr=BLOCK1, block2_addr=BLOCK2,
            )
            # FakeClient simulates `save_state`/`load_state` purely in
            # memory (it never touches the real filesystem) -- only the
            # sidecar is real disk I/O, done by `arenas.py` itself.
            self.assertIn("base", c.states)
            self.assertIn(str(result.state_path), c.states)
            self.assertTrue(result.sidecar_path.exists())
            self.assertEqual(result.state_path, out_dir / "gap-10.state")
            self.assertEqual(result.sidecar_path, out_dir / "gap-10.gap.json")

            # Walking P1 right by 10*3=30px shrinks the 192px gap to 162.
            self.assertEqual(result.gap_px, 162)
            self.assertEqual(result.char_id_block1, 5)
            self.assertEqual(result.char_id_block2, 9)
            self.assertEqual(
                result.facing, {"block1_side": "left", "block2_side": "right"}
            )
            self.assertEqual(result.inputs_live, {"p0": True, "p1": True})

            sidecar = json.loads(result.sidecar_path.read_text())
            for key in (
                "format", "family", "port", "walk_frames", "walk_port",
                "walk_direction", "gap_px", "char_id_block1", "char_id_block2",
                "facing", "inputs_live", "base_arena", "saved_at",
            ):
                self.assertIn(key, sidecar)
            self.assertEqual(sidecar["walk_frames"], 10)
            self.assertEqual(sidecar["gap_px"], 162)
            self.assertEqual(sidecar["inputs_live"], {"p0": True, "p1": True})

    def test_never_calls_press_buttons(self):
        c = FakeClient()
        with tempfile.TemporaryDirectory() as tmp:
            build_gap_ladder_arena(
                c, base_arena="base", out_dir=tmp, k=5,
                walk_port=0, walk_direction="right",
                block1_addr=BLOCK1, block2_addr=BLOCK2,
            )
        tool_names = {name for name, _ in c.calls}
        self.assertNotIn("press_buttons", tool_names)

    def test_point_blank_k_zero_needs_no_walk(self):
        c = FakeClient(x1=469, x2=661)
        with tempfile.TemporaryDirectory() as tmp:
            result = build_gap_ladder_arena(
                c, base_arena="base", out_dir=tmp, k=0,
                walk_port=0, walk_direction="right",
                block1_addr=BLOCK1, block2_addr=BLOCK2,
            )
        self.assertEqual(result.k, 0)
        self.assertEqual(result.gap_px, 192)

    def test_refuses_to_save_when_liveness_check_fails(self):
        # Port 1 is a dead/CPU-driven port: held input never moves it.
        c = FakeClient(live_ports=(0,))
        with tempfile.TemporaryDirectory() as tmp:
            out_dir = Path(tmp)
            with self.assertRaises(ArenaLivenessError):
                build_gap_ladder_arena(
                    c, base_arena="base", out_dir=out_dir, k=10,
                    walk_port=0, walk_direction="right",
                    block1_addr=BLOCK1, block2_addr=BLOCK2,
                )
            # Nothing was written -- a failed liveness check must not ship
            # an arena.
            self.assertEqual(list(out_dir.iterdir()), [])

    def test_unmeasurable_pointer_also_refuses_as_liveness_failure(self):
        # An arena where one side's object pointer never resolves (e.g. the
        # attract screen / character select, per mk2.md's pointer-hygiene
        # table) is not a valid live 2-human rig either.
        c = FakeClient(ptr2_valid=False)
        with tempfile.TemporaryDirectory() as tmp:
            out_dir = Path(tmp)
            with self.assertRaises(ArenaLivenessError):
                build_gap_ladder_arena(
                    c, base_arena="base", out_dir=out_dir, k=0,
                    walk_port=0, walk_direction="right",
                    block1_addr=BLOCK1, block2_addr=BLOCK2,
                )
            self.assertEqual(list(out_dir.iterdir()), [])

    def test_unknown_gap_recorded_as_null_not_zero_in_sidecar(self):
        # Corruption lands mid-walk (after both liveness probes complete,
        # which take 4*probe_frames=24 steps at the default probe_frames=6),
        # and -- because it corrupts the actual bytes rather than some
        # resettable counter -- it reproduces identically through the
        # save/reload cycle, so this does NOT trip the reproduction check.
        c = FakeClient(corrupt_block1_cid_at_step=26)
        with tempfile.TemporaryDirectory() as tmp:
            out_dir = Path(tmp)
            result = build_gap_ladder_arena(
                c, base_arena="base", out_dir=out_dir, k=5,
                walk_port=0, walk_direction="right",
                block1_addr=BLOCK1, block2_addr=BLOCK2,
                probe_frames=6,
            )
            self.assertIsNone(result.gap_px)
            # Char ids are read straight off the block, independent of the
            # stale object pointer -- still known even though gap is not.
            self.assertEqual(result.char_id_block1, 5)
            self.assertEqual(result.char_id_block2, 9)
            raw = json.loads(result.sidecar_path.read_text())
            self.assertIsNone(raw["gap_px"])
            # The literal JSON text must be `null`, never a bare `0`.
            self.assertIn('"gap_px": null', result.sidecar_path.read_text())

    def test_deletes_state_and_raises_when_reload_does_not_reproduce(self):
        c = FakeClient(jitter_reload_px=7)
        with tempfile.TemporaryDirectory() as tmp:
            out_dir = Path(tmp)
            with self.assertRaises(ArenaReproductionError):
                build_gap_ladder_arena(
                    c, base_arena="base", out_dir=out_dir, k=5,
                    walk_port=0, walk_direction="right",
                    block1_addr=BLOCK1, block2_addr=BLOCK2,
                )
            # A broken arena must be deleted, not shipped.
            self.assertEqual(list(out_dir.iterdir()), [])

    def test_deletes_the_apps_own_orphaned_meta_json_too(self):
        # `save_state` to an `.../arenas/...` path makes the REAL app
        # auto-write its own `<name>.meta.json` sidecar out-of-band, inside
        # the same MCP round-trip (`Frontend::write_arena_sidecar`) -- a
        # reproduction failure must not leave that orphan behind next to a
        # `.state` file that no longer exists.
        c = FakeClient(jitter_reload_px=7)
        with tempfile.TemporaryDirectory() as tmp:
            out_dir = Path(tmp)
            state_path = out_dir / "gap-5.state"
            app_meta_path = out_dir / "gap-5.meta.json"
            app_meta_path.write_text('{"format": "arena-meta-v1"}')
            with self.assertRaises(ArenaReproductionError):
                build_gap_ladder_arena(
                    c, base_arena="base", out_dir=out_dir, k=5,
                    walk_port=0, walk_direction="right",
                    block1_addr=BLOCK1, block2_addr=BLOCK2,
                )
            self.assertFalse(state_path.exists())
            self.assertFalse(app_meta_path.exists())


class BuildGapLadderTest(unittest.TestCase):
    def test_builds_every_rung_in_ascending_order(self):
        c = FakeClient(x1=469, x2=661, walk_speed=3)
        with tempfile.TemporaryDirectory() as tmp:
            out_dir = Path(tmp)
            results = build_gap_ladder(
                c, base_arena="base", out_dir=out_dir, ks=[40, 0, 15],
                walk_port=0, walk_direction="right",
                block1_addr=BLOCK1, block2_addr=BLOCK2,
            )
        self.assertEqual([r.k for r in results], [0, 15, 40])
        # Monotone: more walk-frames -> smaller gap (walking P1 toward P2).
        gaps = [r.gap_px for r in results]
        self.assertEqual(gaps, sorted(gaps, reverse=True))

    def test_stops_at_first_failing_rung(self):
        c = FakeClient(live_ports=(0,))  # port 1 dead -> every rung fails
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(ArenaLivenessError):
                build_gap_ladder(
                    c, base_arena="base", out_dir=tmp, ks=[0, 10],
                    walk_port=0, walk_direction="right",
                    block1_addr=BLOCK1, block2_addr=BLOCK2,
                )


if __name__ == "__main__":
    unittest.main()
