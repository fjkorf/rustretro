from __future__ import annotations

import json
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

from shadow_train.framelab.export import default_export_path, export_frames
from shadow_train.framelab.identity import compute_core_id, compute_rom_id
from shadow_train.framelab.store import FrameStore


def _row(**overrides) -> dict:
    row = {
        "family": "asurabld",
        "port": "asurabld",
        "char": "rosemary",
        "move": "5A",
        "observable": "struct_divergence",
        "method": "linear_sweep",
        "input_latency_frames": 4,
        "core_id": "core:sha256:aaaa",
        "rom_id": "rom:sha256:bbbb",
    }
    row.update(overrides)
    return row


class IdentityTest(unittest.TestCase):
    def test_core_id_is_stable_for_identical_bytes(self):
        with TemporaryDirectory() as d:
            p = Path(d) / "core.dylib"
            p.write_bytes(b"some core bytes")
            self.assertEqual(compute_core_id(p), compute_core_id(p))

    def test_core_id_changes_when_the_file_changes(self):
        with TemporaryDirectory() as d:
            p = Path(d) / "core.dylib"
            p.write_bytes(b"version one")
            id1 = compute_core_id(p)
            p.write_bytes(b"version two, rebuilt")
            id2 = compute_core_id(p)
            self.assertNotEqual(id1, id2)

    def test_rom_id_includes_the_filename(self):
        with TemporaryDirectory() as d:
            p = Path(d) / "asurabld.zip"
            p.write_bytes(b"zip bytes")
            self.assertIn("asurabld.zip", compute_rom_id(p))

    def test_core_id_and_rom_id_of_same_bytes_differ_by_filename_only(self):
        with TemporaryDirectory() as d:
            core_p = Path(d) / "core.dylib"
            rom_p = Path(d) / "rom.zip"
            core_p.write_bytes(b"identical content")
            rom_p.write_bytes(b"identical content")
            core_id = compute_core_id(core_p)
            rom_id = compute_rom_id(rom_p)
            self.assertNotEqual(core_id, rom_id)
            # Same content -> same digest suffix, different filename prefix.
            self.assertEqual(core_id.split(":", 1)[1], rom_id.split(":", 1)[1])


class ExportTest(unittest.TestCase):
    def test_export_writes_flat_json_with_provenance_fields(self):
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            store.insert(_row(move="5A"))
            store.insert(_row(move="2A", char="footee"))
            store.insert(_row(family="mk2", port="genesis", move="5A"))  # different game

            out_path = Path(d) / "asurabld.frames.json"
            written = export_frames(store, "asurabld", "asurabld", out_path=out_path)
            self.assertEqual(written, out_path)

            payload = json.loads(out_path.read_text())
            self.assertEqual(payload["family"], "asurabld")
            self.assertEqual(payload["port"], "asurabld")
            self.assertEqual(payload["row_count"], 2)
            self.assertEqual(len(payload["moves"]), 2)

            for move in payload["moves"]:
                # Flat: no nested dict/list values.
                for v in move.values():
                    self.assertNotIsInstance(v, (dict, list))
                # Provenance a reader needs to trust (or not trust) a number.
                for field in ("observable", "method", "input_latency_frames",
                              "core_id", "rom_id", "measured_at"):
                    self.assertIn(field, move)
                    self.assertIsNotNone(move[field])

    def test_default_export_path_matches_library_family_port_convention(self):
        path = default_export_path("asurabld", "asurabld")
        self.assertTrue(str(path).endswith("library/asurabld/asurabld.frames.json"))

    def test_export_creates_parent_directories(self):
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            store.insert(_row())
            out_path = Path(d) / "nested" / "dir" / "out.frames.json"
            export_frames(store, "asurabld", "asurabld", out_path=out_path)
            self.assertTrue(out_path.exists())


if __name__ == "__main__":
    unittest.main()
