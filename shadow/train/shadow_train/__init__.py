"""Shadow trainer (PLAN Wave 2d): dataset builder, kNN baseline, evaluation.

Implements shadow/SPEC.md rev. v2 exactly: jsonl-v2 rows -> 8 Hz decisions with
chord-aware labels, side-agnostic features with stale opponent observations and
K-step stacking, a kNN case-retrieval policy (the "everything it does, you did"
baseline), and the situation-bucket evaluation + coverage report.
"""
