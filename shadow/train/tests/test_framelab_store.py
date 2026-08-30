from __future__ import annotations

import sqlite3
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

from shadow_train.framelab.store import (
    MOVE_FRAMES_COLUMNS,
    REQUIRED_PROVENANCE_COLUMNS,
    SCHEMA_VERSION,
    FrameStore,
    ProvenanceError,
    SchemaVersionError,
)


def _valid_row(**overrides) -> dict:
    row = {
        "family": "asurabld",
        "port": "asurabld",
        "char": "rosemary",
        "move": "5A",
        "observable": "struct_divergence",
        "method": "linear_sweep",
        "input_latency_frames": 4,
        "core_id": "fbalpha2012.dylib:sha256:deadbeefcafef00d",
        "rom_id": "asurabld.zip:sha256:0123456789abcdef",
    }
    row.update(overrides)
    return row


class SchemaTest(unittest.TestCase):
    def test_fresh_db_creates_move_frames_with_expected_columns(self):
        with TemporaryDirectory() as d:
            path = Path(d) / "frames.sqlite3"
            with FrameStore(path):
                pass
            conn = sqlite3.connect(str(path))
            try:
                cols = [r[1] for r in conn.execute("PRAGMA table_info(move_frames)")]
                # id is the surrogate PK this module adds on top of §6's list.
                self.assertEqual(cols, ["id", *MOVE_FRAMES_COLUMNS])
                version = conn.execute("PRAGMA user_version").fetchone()[0]
                self.assertEqual(version, SCHEMA_VERSION)
            finally:
                conn.close()

    def test_reopening_same_db_is_idempotent(self):
        with TemporaryDirectory() as d:
            path = Path(d) / "frames.sqlite3"
            with FrameStore(path) as s1:
                rid = s1.insert(_valid_row())
            with FrameStore(path) as s2:
                self.assertEqual(s2.get(rid)["move"], "5A")

    def test_unversioned_legacy_db_with_move_frames_table_fails_loudly(self):
        with TemporaryDirectory() as d:
            path = Path(d) / "legacy.sqlite3"
            conn = sqlite3.connect(str(path))
            # A move_frames table exists, but PRAGMA user_version was never
            # stamped (default 0) -- simulates a pre-versioning database.
            conn.execute("CREATE TABLE move_frames (family TEXT)")
            conn.commit()
            conn.close()
            with self.assertRaises(SchemaVersionError):
                FrameStore(path)

    def test_newer_or_mismatched_schema_version_fails_loudly(self):
        with TemporaryDirectory() as d:
            path = Path(d) / "future.sqlite3"
            conn = sqlite3.connect(str(path))
            conn.execute("CREATE TABLE move_frames (family TEXT)")
            conn.execute(f"PRAGMA user_version = {SCHEMA_VERSION + 1}")
            conn.commit()
            conn.close()
            with self.assertRaises(SchemaVersionError):
                FrameStore(path)

    def test_stale_but_nonzero_schema_version_fails_loudly(self):
        with TemporaryDirectory() as d:
            path = Path(d) / "old.sqlite3"
            conn = sqlite3.connect(str(path))
            conn.execute("CREATE TABLE move_frames (family TEXT)")
            # 0 is reserved for "unversioned legacy" (tested above); this is
            # a distinct, still-wrong stamp standing in for "a real prior
            # schema version this code no longer knows how to read."
            conn.execute("PRAGMA user_version = 42")
            conn.commit()
            conn.close()
            with self.assertRaises(SchemaVersionError):
                FrameStore(path)


class NullRoundTripTest(unittest.TestCase):
    """docs/frames.md §2.5: 'Absent means absent... a zeroed startup reads
    as an instant move.' NULL must survive and must not be coerced to 0."""

    def test_explicit_none_and_omitted_measured_columns_round_trip_as_none(self):
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            rid = store.insert(_valid_row(
                move="unmeasured-move",
                first_active_frame=None,     # explicit None
                # damage, active, recovery, ... omitted entirely
            ))
            row = store.get(rid)
            self.assertIsNone(row["first_active_frame"])
            self.assertIsNone(row["damage"])
            self.assertIsNone(row["active"])
            self.assertIsNone(row["sample_n"])
            self.assertIsNone(row["gap_px"])

    def test_zero_is_distinct_from_null(self):
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            rid_zero = store.insert(_valid_row(move="instant", first_active_frame=0))
            rid_null = store.insert(_valid_row(move="unmeasured", first_active_frame=None))
            zero_row = store.get(rid_zero)
            null_row = store.get(rid_null)
            self.assertEqual(zero_row["first_active_frame"], 0)
            self.assertIsNone(null_row["first_active_frame"])
            # The bug §2.5 warns about: a NULL must never render/compare as 0.
            self.assertNotEqual(zero_row["first_active_frame"], null_row["first_active_frame"])
            self.assertIsNotNone(zero_row["first_active_frame"])

    def test_damage_zero_survives_as_zero_not_null(self):
        # The inverse direction: a real, measured zero (e.g. a whiff that
        # deals 0 damage) must NOT be reported back as NULL either.
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            rid = store.insert(_valid_row(damage=0))
            self.assertEqual(store.get(rid)["damage"], 0)


class ProvenanceTest(unittest.TestCase):
    def test_insert_accepts_a_fully_provenanced_row(self):
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            rid = store.insert(_valid_row())
            self.assertEqual(store.count(), 1)
            self.assertIsNotNone(store.get(rid)["measured_at"])

    def test_insert_rejects_each_missing_provenance_field_individually(self):
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            for field in REQUIRED_PROVENANCE_COLUMNS:
                row = _valid_row()
                del row[field]
                with self.assertRaises(ProvenanceError, msg=f"missing {field}"):
                    store.insert(row)
            # None of the rejected inserts left a row behind.
            self.assertEqual(store.count(), 0)

    def test_insert_rejects_explicit_none_provenance_same_as_omitted(self):
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            row = _valid_row(method=None)
            with self.assertRaises(ProvenanceError):
                store.insert(row)
            self.assertEqual(store.count(), 0)

    def test_insert_rejects_missing_identifying_key_field(self):
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            row = _valid_row()
            del row["char"]
            with self.assertRaises(ValueError):
                store.insert(row)

    def test_insert_rejects_unknown_column(self):
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            row = _valid_row(bogus_column=1)
            with self.assertRaises(ValueError):
                store.insert(row)

    def test_schema_level_not_null_backstops_the_five_provenance_columns(self):
        # Bypass FrameStore.insert()'s Python-level check entirely and hit
        # the table directly, to prove the enforcement is IN THE SCHEMA, not
        # just a convention the wrapper happens to follow.
        with TemporaryDirectory() as d:
            path = Path(d) / "frames.sqlite3"
            with FrameStore(path):
                pass
            conn = sqlite3.connect(str(path))
            try:
                for field in REQUIRED_PROVENANCE_COLUMNS:
                    row = _valid_row()
                    del row[field]
                    cols = list(row)
                    sql = (
                        f"INSERT INTO move_frames ({','.join(cols)}) "
                        f"VALUES ({','.join('?' for _ in cols)})"
                    )
                    with self.assertRaises(sqlite3.IntegrityError, msg=f"missing {field}"):
                        conn.execute(sql, [row[c] for c in cols])
            finally:
                conn.close()


class QueryTest(unittest.TestCase):
    def test_rows_for_filters_by_family_and_port(self):
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            store.insert(_valid_row(family="asurabld", port="asurabld", move="5A"))
            store.insert(_valid_row(family="mk2", port="genesis", move="5A"))
            rows = store.rows_for("asurabld", "asurabld")
            self.assertEqual(len(rows), 1)
            self.assertEqual(rows[0]["family"], "asurabld")

    def test_delete_removes_a_row(self):
        with TemporaryDirectory() as d:
            store = FrameStore(Path(d) / "frames.sqlite3")
            rid = store.insert(_valid_row())
            store.delete(rid)
            self.assertIsNone(store.get(rid))
            self.assertEqual(store.count(), 0)


if __name__ == "__main__":
    unittest.main()
