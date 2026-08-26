"""kNN case-retrieval policy — the SPEC §7.1 baseline.

Stores every training decision (standardized feature vector -> the user's
(move, attack) label). Prediction retrieves the k nearest cases and samples
from their label distribution with temperature. The support constraint (§7.2)
is satisfied by construction: only demonstrated actions can be emitted.
"""

from __future__ import annotations

from pathlib import Path

import numpy as np

from . import dataset as _dataset

CASES_FILE = "cases.npz"


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

    def save(self, out_dir: str | Path) -> None:
        """Persist the fitted case store (task 3). Saves exactly the state
        `predict`/`predict_proba` read: the standardization params (mu, sd),
        the already-standardized case matrix, both label arrays, and k/
        temperature -- so `load` reconstructs a policy that is bit-for-bit
        equivalent to this one (see the round-trip test in test_knn.py)."""
        out_dir = Path(out_dir)
        out_dir.mkdir(parents=True, exist_ok=True)
        np.savez(
            out_dir / CASES_FILE,
            X=self.X,
            mu=self.mu,
            sd=self.sd,
            y_move=self.y_move,
            y_attack=self.y_attack,
            k=np.array(self.k),
            temperature=np.array(self.temperature),
        )

    @classmethod
    def load(cls, model_dir: str | Path) -> "KnnPolicy":
        """Inverse of save(). Does NOT call fit() -- it restores the fitted
        arrays directly so no data or standardization is recomputed."""
        model_dir = Path(model_dir)
        with np.load(model_dir / CASES_FILE) as data:
            policy = cls(k=int(data["k"]), temperature=float(data["temperature"]))
            policy.X = data["X"]
            policy.mu = data["mu"]
            policy.sd = data["sd"]
            policy.y_move = data["y_move"]
            policy.y_attack = data["y_attack"]
        return policy

    # Soft retrieval widens the electorate beyond the top-k: the k nearest
    # cases set the distance scale (sigma = the k-th neighbor's distance), and
    # every case within WIDE_K votes with weight exp(-(d/sigma)^2). Cure for
    # the absorbing-state failure: in a "both standing at range" state whose
    # top-15 are unanimously idle, the nearest ATTACK case a little further
    # out now gets real (small) probability instead of exactly zero — while
    # states with nearby active cases behave as before.
    WIDE_K = 100

    def _weighted_neighbors(self, x: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
        d = np.linalg.norm(self.X - (x - self.mu) / self.sd, axis=1)
        order = np.argsort(d)[: max(self.k, self.WIDE_K)]
        sigma = max(float(d[order[min(self.k, len(order)) - 1]]), 1e-6)
        w = np.exp(-((d[order] / sigma) ** 2))
        return order, w

    def _vote(self, labels: np.ndarray, weights: np.ndarray, n_classes: int) -> np.ndarray:
        counts = np.zeros(n_classes)
        np.add.at(counts, labels, weights)
        if self.temperature <= 0:  # argmax
            p = np.zeros(n_classes)
            p[counts.argmax()] = 1.0
            return p
        logits = np.log(counts + 1e-9) / self.temperature
        p = np.exp(logits - logits.max())
        return p / p.sum()

    def predict_proba(self, x: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
        idx, w = self._weighted_neighbors(x)
        n_move = len(_dataset.MOVE_CLASSES)
        n_attack = len(_dataset.ATTACK_CLASSES)
        return (
            self._vote(self.y_move[idx], w, n_move),
            self._vote(self.y_attack[idx], w, n_attack),
        )

    def predict(self, x: np.ndarray, rng: np.random.Generator | None = None):
        pm, pa = self.predict_proba(x)
        if rng is None:  # deterministic eval: argmax
            return int(pm.argmax()), int(pa.argmax())
        n_move = len(_dataset.MOVE_CLASSES)
        n_attack = len(_dataset.ATTACK_CLASSES)
        return int(rng.choice(n_move, p=pm)), int(rng.choice(n_attack, p=pa))
