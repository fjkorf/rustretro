"""kNN case-retrieval policy — the SPEC §7.1 baseline.

Stores every training decision (standardized feature vector -> the user's
(move, attack) label). Prediction retrieves the k nearest cases and samples
from their label distribution with temperature. The support constraint (§7.2)
is satisfied by construction: only demonstrated actions can be emitted.
"""

from __future__ import annotations

import numpy as np


class KnnPolicy:
    def __init__(self, k: int = 15, temperature: float = 1.0):
        self.k = k
        self.temperature = temperature

    def fit(self, X: np.ndarray, y_move: np.ndarray, y_attack: np.ndarray):
        self.mu = X.mean(axis=0)
        self.sd = X.std(axis=0) + 1e-6
        self.X = (X - self.mu) / self.sd
        self.y_move = y_move
        self.y_attack = y_attack
        return self

    def _neighbors(self, x: np.ndarray) -> np.ndarray:
        d = np.linalg.norm(self.X - (x - self.mu) / self.sd, axis=1)
        return np.argsort(d)[: self.k]

    def _vote(self, labels: np.ndarray, n_classes: int) -> np.ndarray:
        counts = np.bincount(labels, minlength=n_classes).astype(np.float64)
        if self.temperature <= 0:  # argmax
            p = np.zeros(n_classes)
            p[counts.argmax()] = 1.0
            return p
        logits = np.log(counts + 1e-9) / self.temperature
        p = np.exp(logits - logits.max())
        return p / p.sum()

    def predict_proba(self, x: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
        idx = self._neighbors(x)
        return self._vote(self.y_move[idx], 9), self._vote(self.y_attack[idx], 6)

    def predict(self, x: np.ndarray, rng: np.random.Generator | None = None):
        pm, pa = self.predict_proba(x)
        if rng is None:  # deterministic eval: argmax
            return int(pm.argmax()), int(pa.argmax())
        return int(rng.choice(9, p=pm)), int(rng.choice(6, p=pa))
