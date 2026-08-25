from __future__ import annotations

import tempfile
import unittest

import numpy as np

from shadow_train.knn import KnnPolicy


class KnnSaveLoadTest(unittest.TestCase):
    """Task 3: a loaded policy must produce identical predictions to the
    freshly fitted one it was saved from."""

    def setUp(self):
        rng = np.random.default_rng(0)
        self.X = rng.normal(size=(200, 12)).astype(np.float32)
        self.y_move = rng.integers(0, 9, size=200)
        self.y_attack = rng.integers(0, 6, size=200)
        self.policy = KnnPolicy(k=7, temperature=0.5).fit(
            self.X, self.y_move, self.y_attack
        )
        self.queries = rng.normal(size=(20, 12)).astype(np.float32)

    def test_argmax_predictions_match(self):
        with tempfile.TemporaryDirectory() as d:
            self.policy.save(d)
            loaded = KnnPolicy.load(d)

        self.assertEqual(loaded.k, self.policy.k)
        self.assertEqual(loaded.temperature, self.policy.temperature)
        for q in self.queries:
            self.assertEqual(self.policy.predict(q), loaded.predict(q))
            pm1, pa1 = self.policy.predict_proba(q)
            pm2, pa2 = loaded.predict_proba(q)
            np.testing.assert_array_equal(pm1, pm2)
            np.testing.assert_array_equal(pa1, pa2)

    def test_sampled_predictions_match_under_identical_rng_state(self):
        with tempfile.TemporaryDirectory() as d:
            self.policy.save(d)
            loaded = KnnPolicy.load(d)

        for q in self.queries:
            r1 = np.random.default_rng(42)
            r2 = np.random.default_rng(42)
            self.assertEqual(
                self.policy.predict(q, rng=r1), loaded.predict(q, rng=r2)
            )

    def test_case_store_values_round_trip_exactly(self):
        with tempfile.TemporaryDirectory() as d:
            self.policy.save(d)
            loaded = KnnPolicy.load(d)

        np.testing.assert_array_equal(self.policy.X, loaded.X)
        np.testing.assert_array_equal(self.policy.mu, loaded.mu)
        np.testing.assert_array_equal(self.policy.sd, loaded.sd)
        np.testing.assert_array_equal(self.policy.y_move, loaded.y_move)
        np.testing.assert_array_equal(self.policy.y_attack, loaded.y_attack)


if __name__ == "__main__":
    unittest.main()
