"""The segment-shadow offline engine (SEGMENT_SHADOW.md wave a): segment
index, similarity, retrieval, stickiness, and the facing mirror.

Explicit re-exports only — never a bare-name package re-export (the
`framelab.replay` trap: `from shadow_train.framelab import replay` binds a
FUNCTION, not the module). Import submodules by full path.
"""

from .config import SegmentsConfig, SegmentsConfigError
from .model import INDEX_FILE, META_FILE, SEGMENTS_VERSION, Segment, SegmentIndex

__all__ = [
    "SegmentsConfig",
    "SegmentsConfigError",
    "Segment",
    "SegmentIndex",
    "SEGMENTS_VERSION",
    "INDEX_FILE",
    "META_FILE",
]
