"""The frame lab's authoring store — `docs/frames.md` §6, verbatim.

SQLite via stdlib `sqlite3`. No new dependency (project convention — see
`re.py`/`mcpclient.py`), and — the more load-bearing half of that sentence —
NO RUST DEPENDENCY: `Rust never opens this database` (§6). The only bridge to
the app/Lua is the flat JSON this package's `export` module produces.

Two honesty rules drive every design choice here, both from §2.5 and §7:

  1. **Absent means absent.** A quantity we could not measure is NULL, never
     0 — "a zeroed startup reads as an instant move." Every MEASURED column
     below is nullable with no default, so a caller must pass an explicit
     value to get anything other than true SQL NULL; `sqlite3` never coerces
     `None` to `0` on its own, but this module doesn't lean on that
     incidentally — the schema itself has no `DEFAULT 0` for these columns
     to fall back on, so there's nothing to coerce THROUGH.
  2. **Every row carries its provenance.** §7: "a row without provenance is
     the `action_counter` mistake in another costume." §6 names
     `observable`/`method` explicitly; the task that commissioned this store
     also named `calibration` — which in this schema IS `input_latency_frames`
     (§3.1: "An uncalibrated run is not a run," and it's the one calibration
     number every row's `on_hit`/`on_block`/etc. had already had subtracted
     out of it before being stored) — plus `core_id`/`rom_id`. Those five
     columns are `NOT NULL` at the SQL level (enforcement IN THE SCHEMA, not
     a docstring asking nicely) and `insert()` pre-checks them for a message
     that names the missing field instead of a bare `IntegrityError`.
"""

from __future__ import annotations

import sqlite3
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional, Union

__all__ = [
    "MOVE_FRAMES_COLUMNS",
    "REQUIRED_KEY_COLUMNS",
    "REQUIRED_PROVENANCE_COLUMNS",
    "SCHEMA_VERSION",
    "FrameStore",
    "ProvenanceError",
    "SchemaVersionError",
]

# Column order matches docs/frames.md §6's `move_frames(...)` block exactly
# (minus the surrogate `id` primary key, which the doc doesn't mention and
# this module adds purely as a row handle).
MOVE_FRAMES_COLUMNS: tuple[str, ...] = (
    "family", "port", "char", "move", "variant",
    "gap_walk_frames", "gap_px",
    "first_active_frame", "active", "recovery", "total", "hits",
    "hitstop", "on_hit", "on_block", "wakeup_window",
    "knockdown", "juggle", "guard_height", "connect_range",
    "rig_guard_state", "damage", "observable", "method",
    "input_latency_frames", "sample_n", "confidence", "measured_at",
    "core_id", "rom_id",
)

# The identifying key of a row (not "provenance" in §7's sense, but a row
# without these isn't a row about anything).
REQUIRED_KEY_COLUMNS: tuple[str, ...] = ("family", "port", "char", "move")

# §7's provenance list, resolved to concrete column names. "calibration" ==
# `input_latency_frames` — see the module docstring's rule 2.
REQUIRED_PROVENANCE_COLUMNS: tuple[str, ...] = (
    "method", "observable", "input_latency_frames", "core_id", "rom_id",
)

SCHEMA_VERSION = 1

# `update()`'s lock-contention retry (task P2: "the store may be written
# concurrently by another agent this wave -- keep transactions short and
# retry on lock contention rather than failing the run"). `sqlite3.connect`
# already busy-waits up to its own `timeout` (5s default) before raising
# `OperationalError("database is locked")`; this is a second, short layer on
# top of that for the rare case two writers's 5s windows still overlap. Not a
# sleep-as-a-fix (§2.4 is about measurement, not this) -- it is retrying an
# actual observed failure, with backoff, and it still raises if contention
# outlives it.
_UPDATE_RETRY_ATTEMPTS = 8
_UPDATE_RETRY_BACKOFF_S = 0.05

_CREATE_V1 = """
CREATE TABLE move_frames (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    family                TEXT NOT NULL,
    port                  TEXT NOT NULL,
    char                  TEXT NOT NULL,
    move                  TEXT NOT NULL,
    variant               TEXT,
    gap_walk_frames       INTEGER,
    gap_px                REAL,
    first_active_frame    INTEGER,
    active                INTEGER,
    recovery              INTEGER,
    total                 INTEGER,
    hits                  INTEGER,
    hitstop               INTEGER,
    on_hit                INTEGER,
    on_block              INTEGER,
    wakeup_window         INTEGER,
    knockdown             INTEGER,
    juggle                INTEGER,
    guard_height          TEXT,
    connect_range         INTEGER,
    rig_guard_state       TEXT,
    damage                INTEGER,
    observable            TEXT NOT NULL,
    method                TEXT NOT NULL,
    input_latency_frames  INTEGER NOT NULL,
    sample_n              INTEGER,
    confidence            TEXT,
    measured_at           TEXT NOT NULL,
    core_id               TEXT NOT NULL,
    rom_id                TEXT NOT NULL
);
"""


class ProvenanceError(ValueError):
    """An insert was rejected for missing a required provenance field."""


class SchemaVersionError(RuntimeError):
    """The database's `PRAGMA user_version` doesn't match this code's
    `SCHEMA_VERSION`, and no migration path is registered for the gap. This
    is deliberately a hard failure — "a database from an older schema must
    fail loudly, not silently mis-read" — never a best-effort read."""


def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def _ensure_schema(conn: sqlite3.Connection) -> None:
    version = conn.execute("PRAGMA user_version").fetchone()[0]
    if version == 0:
        existing = conn.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='move_frames'"
        ).fetchone()
        if existing is not None:
            raise SchemaVersionError(
                "this database has a move_frames table but no schema-version "
                "stamp (PRAGMA user_version=0) -- it predates this module's "
                "versioning, or is some other file entirely. Refusing to "
                "open it: a stale schema silently mis-read is worse than a "
                "loud failure here. Export what you need with whatever code "
                "wrote it, then start a fresh store."
            )
        conn.executescript(_CREATE_V1)
        conn.execute(f"PRAGMA user_version = {SCHEMA_VERSION}")
        conn.commit()
        return
    if version != SCHEMA_VERSION:
        raise SchemaVersionError(
            f"move_frames database is schema version {version}, but this "
            f"code is schema version {SCHEMA_VERSION} and no migration is "
            "registered for that gap. Refusing to open it rather than risk "
            "misinterpreting columns that may have moved, changed type, or "
            "changed meaning between versions."
        )


class FrameStore:
    """One SQLite file, one `move_frames` table, versioned via
    `PRAGMA user_version` (§6/§7). Opening a fresh path creates and stamps
    the current schema; opening an existing one validates the stamp and
    raises `SchemaVersionError` on any mismatch (see `_ensure_schema`).

    Usage:
        with FrameStore(path) as store:
            row_id = store.insert({...})
            row = store.get(row_id)
    """

    def __init__(self, path: Union[str, Path]):
        self.path = Path(path)
        self._conn = sqlite3.connect(str(self.path))
        self._conn.row_factory = sqlite3.Row
        _ensure_schema(self._conn)

    # ── lifecycle ────────────────────────────────────────────────────────
    def close(self) -> None:
        self._conn.close()

    def __enter__(self) -> "FrameStore":
        return self

    def __exit__(self, *exc_info: Any) -> None:
        self.close()

    # ── writes ───────────────────────────────────────────────────────────
    def insert(self, row: dict) -> int:
        """Insert one `move_frames` row. Returns the new row's `id`.

        Raises `ValueError` for an unknown column or a missing identifying
        field (`family`/`port`/`char`/`move`), and `ProvenanceError` —
        BEFORE any SQL runs — for a missing provenance field (§7). The
        table's own `NOT NULL` constraints back this up at the schema level
        for anything that reaches `sqlite3.execute` some other way.

        `measured_at` defaults to now (UTC, ISO-8601) if omitted; every
        other column is taken exactly as given — in particular, passing a
        key with value `None` (or omitting it) both produce SQL NULL, never
        a coerced `0`.
        """
        unknown = set(row) - set(MOVE_FRAMES_COLUMNS)
        if unknown:
            raise ValueError(f"unknown move_frames column(s): {sorted(unknown)}")

        missing_key = [c for c in REQUIRED_KEY_COLUMNS if not row.get(c)]
        if missing_key:
            raise ValueError(
                f"move_frames row missing identifying field(s): {missing_key}"
            )

        missing_prov = [c for c in REQUIRED_PROVENANCE_COLUMNS if row.get(c) is None]
        if missing_prov:
            raise ProvenanceError(
                "move_frames insert rejected -- missing provenance field(s) "
                f"{missing_prov}. docs/frames.md §7: 'every row carries its "
                "method, observable, and calibration' (calibration == "
                "input_latency_frames, §3.1); core_id/rom_id per §6. A row "
                "without these is not distinguishable from a stale or "
                "made-up one."
            )

        row = dict(row)
        row.setdefault("measured_at", _now_iso())

        cols = [c for c in MOVE_FRAMES_COLUMNS if c in row]
        placeholders = ",".join("?" for _ in cols)
        sql = f"INSERT INTO move_frames ({','.join(cols)}) VALUES ({placeholders})"
        cur = self._conn.execute(sql, [row[c] for c in cols])
        self._conn.commit()
        assert cur.lastrowid is not None
        return cur.lastrowid

    def update(self, row_id: int, values: dict) -> None:
        """UPDATE a subset of one row's already-NULL MEASURED columns in
        place -- the operation this store never had until `hitstop` needed
        one (§11 reserved the column; nothing before task P2 ever filled it
        after the fact for a row whose `on_hit`/`on_block` were already
        measured and shipped).

        Deliberately narrower than a raw SQL `UPDATE`:

          * The identifying key (`REQUIRED_KEY_COLUMNS`) and provenance
            (`REQUIRED_PROVENANCE_COLUMNS`) columns are REFUSED. This method
            fills in a quantity a row did not have yet; it must not let a
            caller change what the row is ABOUT or who vouches for it. Use
            `delete()` + `insert()` for that (§7: "a number that fails
            re-measurement is DELETED, not averaged" -- the same rule
            applies to identity, doubly).
          * `measured_at` is left untouched unless the caller explicitly
            includes it in `values`: filling in `hitstop` does not re-measure
            `on_hit`/`on_block`, and this table has only one timestamp column
            for the whole row, so silently bumping it would misrepresent
            when the OTHER columns were measured.
          * Retries `sqlite3.OperationalError: database is locked` with a
            short backoff (`_UPDATE_RETRY_ATTEMPTS`/`_UPDATE_RETRY_BACKOFF_S`)
            on top of `sqlite3.connect`'s own busy-wait: this store may be
            written CONCURRENTLY by another process's `FrameStore` this wave,
            and a transient lock is not this row's fault.

        Raises `ValueError` for an unknown column, a forbidden column, or a
        `row_id` that does not exist (silently updating 0 rows is exactly
        the kind of quiet no-op this project's honesty rules forbid).
        """
        if not values:
            return
        unknown = set(values) - set(MOVE_FRAMES_COLUMNS)
        if unknown:
            raise ValueError(f"unknown move_frames column(s): {sorted(unknown)}")
        forbidden = (set(REQUIRED_KEY_COLUMNS) | set(REQUIRED_PROVENANCE_COLUMNS)) & set(
            values
        )
        if forbidden:
            raise ValueError(
                f"update() refuses to touch identifying/provenance column(s) "
                f"{sorted(forbidden)} -- it fills in a quantity a row did not "
                "have yet, it does not change what the row is about or who "
                "vouches for it. Use delete() + insert() for that."
            )

        cols = list(values)
        set_clause = ",".join(f"{c} = ?" for c in cols)
        sql = f"UPDATE move_frames SET {set_clause} WHERE id = ?"
        params = [values[c] for c in cols] + [row_id]

        attempt = 0
        while True:
            try:
                cur = self._conn.execute(sql, params)
                self._conn.commit()
                break
            except sqlite3.OperationalError as exc:
                if "locked" not in str(exc).lower() or attempt >= _UPDATE_RETRY_ATTEMPTS - 1:
                    raise
                attempt += 1
                time.sleep(_UPDATE_RETRY_BACKOFF_S * attempt)

        if cur.rowcount == 0:
            raise ValueError(f"update(): no move_frames row with id={row_id}")

    # ── reads ────────────────────────────────────────────────────────────
    def get(self, row_id: int) -> Optional[dict]:
        cur = self._conn.execute("SELECT * FROM move_frames WHERE id = ?", (row_id,))
        r = cur.fetchone()
        return dict(r) if r is not None else None

    def all_rows(self) -> list[dict]:
        cur = self._conn.execute("SELECT * FROM move_frames ORDER BY id")
        return [dict(r) for r in cur.fetchall()]

    def rows_for(self, family: str, port: str) -> list[dict]:
        cur = self._conn.execute(
            "SELECT * FROM move_frames WHERE family = ? AND port = ? ORDER BY id",
            (family, port),
        )
        return [dict(r) for r in cur.fetchall()]

    def count(self) -> int:
        return self._conn.execute("SELECT COUNT(*) FROM move_frames").fetchone()[0]

    def delete(self, row_id: int) -> None:
        """§7: 'a number that fails re-measurement is DELETED, not
        averaged.' This is that operation — no soft-delete, no history."""
        self._conn.execute("DELETE FROM move_frames WHERE id = ?", (row_id,))
        self._conn.commit()
