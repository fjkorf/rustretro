"""framelab — the frame lab's storage, calibration, and export code.

Normative contract: `docs/frames.md`. This package owns the SQLite
authoring store (`store`), the zero-point calibration protocol
(`calibrate`), core/ROM identity for provenance (`identity`), and the
flat-JSON export consumed by the app/Lua (`export`). It never opens a
connection to the running emulator itself — everything here is either pure
logic or driven through an injected client object (see `calibrate`'s module
docstring) — and Rust never opens the SQLite file this package writes.
"""

from __future__ import annotations

from .calibrate import (
    CalibrationError,
    CalibrationResult,
    sprite_lag_frames,
    zero_point_calibration,
)
from .export import default_export_path, export_frames
from .identity import compute_core_id, compute_rom_id
from .store import (
    MOVE_FRAMES_COLUMNS,
    REQUIRED_KEY_COLUMNS,
    REQUIRED_PROVENANCE_COLUMNS,
    SCHEMA_VERSION,
    FrameStore,
    ProvenanceError,
    SchemaVersionError,
)

__all__ = [
    "CalibrationError",
    "CalibrationResult",
    "sprite_lag_frames",
    "zero_point_calibration",
    "default_export_path",
    "export_frames",
    "compute_core_id",
    "compute_rom_id",
    "MOVE_FRAMES_COLUMNS",
    "REQUIRED_KEY_COLUMNS",
    "REQUIRED_PROVENANCE_COLUMNS",
    "SCHEMA_VERSION",
    "FrameStore",
    "ProvenanceError",
    "SchemaVersionError",
]
