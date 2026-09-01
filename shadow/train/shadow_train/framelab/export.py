"""docs/frames.md §6's consumption side: `library/<family>/<port>.frames.json`.

"Consumption: an exported `library/<family>/<port>.frames.json` that the app
and Lua read. Rust never opens the database." The shape is kept small and
flat on purpose — it's read by a Lua overlay (§9), not a query engine — and
every row keeps the provenance fields (`observable`, `method`,
`input_latency_frames`, `core_id`, `rom_id`, `measured_at`) a reader needs to
judge whether a number is trustworthy, per §7.
"""

from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional, Union

from .store import SCHEMA_VERSION, FrameStore

class ExportVerificationError(RuntimeError):
    """The written export does not match the store it was generated from."""


__all__ = ["default_export_path", "export_frames"]

# shadow_train/framelab/export.py -> framelab/ -> shadow_train/ -> train/ ->
# shadow/ -> repo root (matches profile.py's REPO_ROOT convention, one level
# deeper since this module lives one package down).
REPO_ROOT = Path(__file__).resolve().parents[4]


def default_export_path(family: str, port: str) -> Path:
    return REPO_ROOT / "library" / family / f"{port}.frames.json"


def export_frames(
    store: FrameStore,
    family: str,
    port: str,
    out_path: Optional[Union[str, Path]] = None,
) -> Path:
    """Write every `move_frames` row for `(family, port)` to a flat JSON
    file — `library/<family>/<port>.frames.json` by default, matching the
    profile pair's own `<port>.profile.json` naming. Returns the path
    written.

    Each entry in `moves` is the row exactly as stored (a flat dict, one
    level, no nesting) — including its `id` and every provenance column —
    so a reader can decide per-row whether to trust a number instead of the
    exporter deciding for it.
    """
    rows = store.rows_for(family, port)
    payload = {
        "schema_version": SCHEMA_VERSION,
        "family": family,
        "port": port,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "row_count": len(rows),
        "moves": rows,
    }
    path = Path(out_path) if out_path is not None else default_export_path(family, port)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")

    # Read back and compare VALUES, not counts. The store and the committed
    # export have silently diverged three times, and the third time the row
    # counts MATCHED (142 == 142) while `hitstop` was stale for a whole
    # character — 2 of 48 in the export against 30 of 48 in the store. That
    # went into a merged commit message as a coverage claim. A count check is
    # exactly the check that cannot catch a stale-value drift.
    written = json.loads(path.read_text())["moves"]
    if written != rows:
        stale = [
            (a.get("char"), a.get("move"), k)
            for a, b in zip(written, rows)
            for k in set(a) | set(b)
            if a.get(k) != b.get(k)
        ]
        raise ExportVerificationError(
            f"{path} does not match the store after writing: "
            f"{len(written)} rows written vs {len(rows)} in store, "
            f"first mismatches {stale[:5]}"
        )
    return path
