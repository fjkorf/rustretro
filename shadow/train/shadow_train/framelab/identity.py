"""`core_id`/`rom_id` for docs/frames.md §6 — "not bureaucracy: a frame
number measured on a different core build is a different number, and
without them a stale row is indistinguishable from a fresh one."

Neither identifier is exposed as a single MCP tool value today (`rom_info`
is iNES-specific and returns no hash for arcade/Genesis ROMs; the ROM's
`sha1` that `frontend.rs` computes is only ever written into a `.md`
evidence doc's frontmatter, never returned to an MCP client). Profile JSON's
`core.provenance_core` ("fbneo", "fbalpha2012") is a family name, not a
build — CLAUDE.md notes MK2 runs an FBNeo core built from `../FBNeo` source,
which changes underneath that string every time the tree is rebuilt.

So this module does the honest thing instead: hash the ACTUAL core/ROM
FILES the harness was launched with. Both are known to the operator (they
are the `--core`/`--rom` flags), require no MCP round-trip, and change
automatically — with no manual version bump to forget — the moment either
file's bytes change.
"""

from __future__ import annotations

import hashlib
from pathlib import Path
from typing import Union

__all__ = ["compute_core_id", "compute_rom_id"]

# Enough of a sha256 to be practically unique for this purpose while staying
# short in exported JSON / DB rows; the full digest buys nothing here since
# collisions aren't an adversarial concern.
_DIGEST_CHARS = 16


def _file_id(path: Union[str, Path]) -> str:
    p = Path(path)
    digest = hashlib.sha256(p.read_bytes()).hexdigest()[:_DIGEST_CHARS]
    return f"{p.name}:sha256:{digest}"


def compute_core_id(core_path: Union[str, Path]) -> str:
    """`core_id` for a `move_frames` row: hashes the core binary file at
    `core_path` (the same path passed to `--core`). Two runs against a
    byte-identical core file always agree; a rebuild changes the id with no
    action required from the caller."""
    return _file_id(core_path)


def compute_rom_id(rom_path: Union[str, Path]) -> str:
    """`rom_id` for a `move_frames` row: hashes the ROM/romset file at
    `rom_path` (the same path passed to `--rom`). This is computed fresh
    rather than trusted from a `.md` evidence doc's hand-copied `sha1:`
    frontmatter, so it can't silently drift from what was actually loaded
    for THIS run."""
    return _file_id(rom_path)
