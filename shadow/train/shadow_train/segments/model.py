"""The Segment record and the on-disk index (`segments.jsonl` + `meta.json`).

A segment is a REFERENCE into an existing jsonl-v3 recording, never a new
capture format (SEGMENT_SHADOW.md §1): the recording already stores both
fighters' struct fields and both raw input masks per 60 Hz frame, which is
exactly what playback (the masks) and divergence (the state rows) need.

The index is jsonl + a sibling meta.json, deliberately NOT the framelab
sqlite: wave-(b) Rust must read this dependency-free, and Rust never opens
the framelab store (CLAUDE.md). `start_features` is a NAMED object — an
unavailable feature is OMITTED, never written as 0 (absent is not zero).
"""

from __future__ import annotations

import json
from dataclasses import asdict, dataclass, field
from pathlib import Path

SEGMENTS_VERSION = 1
INDEX_FILE = "segments.jsonl"
META_FILE = "meta.json"


@dataclass(frozen=True)
class Segment:
    """One contiguous slice of ONE demonstrator side's recorded play.

    `start_row`/`end_row` are indices into the file's parsed controllable-row
    stream (what slot slicing needs); `[start_row, end_row)` is half-open.
    `start_frame` is the recording's own `frame` value at `start_row`
    (provenance only). `side` is 1 or 2 — which block was the demonstrator
    (the P1-anchor rule, SPEC §5). `start_features` carries only the features
    that resolved at the start row.
    """

    file: str
    round_id: int
    start_row: int
    end_row: int
    start_frame: int
    side: int
    boundary_kind: str
    char_id: int | None
    opp_char_id: int | None
    matchup_key: str
    seq: int
    recency_rank: int
    n_frames: int
    contact_count: int
    absent_x_frames: int
    start_features: dict
    v: int = SEGMENTS_VERSION

    def to_json(self) -> dict:
        return asdict(self)

    @classmethod
    def from_json(cls, d: dict) -> "Segment":
        known = {f for f in cls.__dataclass_fields__}  # type: ignore[attr-defined]
        return cls(**{k: v for k, v in d.items() if k in known})


@dataclass
class SegmentIndex:
    """An in-memory index: the segments plus the meta that describes how they
    were built (the model-dir pattern — arrays as jsonl rows, provenance in a
    sibling meta.json)."""

    segments: list[Segment] = field(default_factory=list)
    meta: dict = field(default_factory=dict)

    def write(self, out_dir: str | Path) -> None:
        out = Path(out_dir)
        out.mkdir(parents=True, exist_ok=True)
        with open(out / INDEX_FILE, "w") as f:
            for s in self.segments:
                # sort_keys keeps the jsonl byte-stable across rebuilds so a
                # no-change rebuild is a no-op diff (the goat-v2 discipline).
                f.write(json.dumps(s.to_json(), sort_keys=True) + "\n")
        with open(out / META_FILE, "w") as f:
            json.dump(self.meta, f, indent=1, sort_keys=True)

    @classmethod
    def read(cls, in_dir: str | Path) -> "SegmentIndex":
        d = Path(in_dir)
        segments: list[Segment] = []
        with open(d / INDEX_FILE) as f:
            for line in f:
                line = line.strip()
                if line:
                    segments.append(Segment.from_json(json.loads(line)))
        meta = json.loads((d / META_FILE).read_text())
        return cls(segments=segments, meta=meta)
