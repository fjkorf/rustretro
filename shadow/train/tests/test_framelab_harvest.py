from __future__ import annotations

import json
import unittest
from pathlib import Path

from shadow_train.framelab.harvest import (
    AuditFinding,
    Observation,
    audit_against_table,
    harvest_file,
    rank_candidates,
)
from shadow_train.macros import _BUTTON_MASKS

B_Y = _BUTTON_MASKS["y"]      # HP
B_B = _BUTTON_MASKS["b"]      # LP
B_X = _BUTTON_MASKS["x"]      # HK
B_A = _BUTTON_MASKS["a"]      # LK
B_LEFT = _BUTTON_MASKS["left"]
B_RIGHT = _BUTTON_MASKS["right"]
B_DOWN = _BUTTON_MASKS["down"]


class FakeProfile:
    """Minimal stand-in for `shadow_train.profile.GameProfile`, carrying
    only what `harvest.py` actually touches -- kept independent of any real
    `library/*.profile.json` so these tests exercise harvest.py's own logic,
    not the loader."""

    def __init__(self, family="testfam", port="arcade", attack_chords=None,
                 chars=None):
        self.family = family
        self.port = port
        self.hitstun_sources = None
        self.fighter_fields = {"char_id": (0x0, 1), "health": (0xE, 1), "x": (0x12, 2)}
        self.calibration = {}
        self.attack_chords = attack_chords or {
            "HP": ["y"], "LP": ["b"], "HK": ["x"], "LK": ["a"],
        }
        self._chars = chars or {1: "alice", 2: "bob"}

    def char_name(self, char_id):
        return self._chars.get(char_id, f"c{char_id}")

    def canon_char_id(self, raw):
        return raw


def _row(frame, round_id=1, p1_block=1, h1=100, h2=100, x1=100, x2=200,
         cid1=1, cid2=2, m1=0, m2=0, p1_special=None, p2_special=None,
         drop_x1=False, drop_x2=False):
    b1 = {"char_id": cid1, "health": h1}
    if not drop_x1:
        b1["x"] = x1
    b2 = {"char_id": cid2, "health": h2}
    if not drop_x2:
        b2["x"] = x2
    row = {
        "v": 3, "frame": frame, "round_id": round_id, "controllable": True,
        "p1_block": p1_block, "block1": b1, "block2": b2, "globals": {},
        "p1_input": m1, "p2_input": m2,
    }
    if p1_special is not None:
        row["p1_special"] = p1_special
    if p2_special is not None:
        row["p2_special"] = p2_special
    return row


def _write_jsonl(path: Path, rows: list) -> None:
    with open(path, "w") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")


class CleanContactTest(unittest.TestCase):
    """A defender health drop with both sides eventually pressing something
    new yields a usable, attributed observation."""

    def test_clean_contact_yields_observation(self):
        rows = []
        for i in range(40):
            m1 = 0
            m2 = 0
            if 20 <= i <= 25:
                m1 = B_Y                      # attacker holds HP through contact
            if i == 26:
                m1 = 0                        # release
            if i >= 30:
                m1 = B_RIGHT                  # attacker's new action
            if i >= 28:
                m2 = B_LEFT                   # defender's new action
            h2 = 100 if i < 25 else 84        # contact lands at frame 25
            rows.append(_row(i, h1=100, h2=h2, m1=m1, m2=m2))

        from tempfile import TemporaryDirectory
        with TemporaryDirectory() as d:
            path = Path(d) / "clean.jsonl"
            _write_jsonl(path, rows)
            obs, stats = harvest_file(path, FakeProfile())

        self.assertEqual(stats.contacts_found, 1)
        self.assertEqual(stats.observations_usable, 1)
        self.assertEqual(len(obs), 1)
        o = obs[0]
        self.assertEqual(o.frame, 25)
        self.assertEqual(o.attacker_char, "alice")
        self.assertEqual(o.defender_char, "bob")
        self.assertEqual(o.move, "HP")
        self.assertEqual(o.move_source, "chord")
        self.assertEqual(o.attacker_next_frame, 30)
        self.assertEqual(o.defender_next_frame, 28)
        self.assertEqual(o.observed_advantage, 28 - 30)
        self.assertEqual(o.gap_px, abs(200 - 100))
        self.assertEqual(stats.observations_attributed, 1)


class PointerUnresolvedXTest(unittest.TestCase):
    """A contact whose pointer-resolved `x` is absent on the frame needed is
    skipped and counted, never silently emitted with gap_px=None."""

    def test_contact_with_absent_pointer_resolved_x_is_skipped_and_counted(self):
        rows = []
        for i in range(40):
            m1 = B_Y if 20 <= i <= 25 else (B_RIGHT if i >= 30 else 0)
            m2 = B_LEFT if i >= 28 else 0
            h2 = 100 if i < 25 else 84
            # defender's x pointer fails to resolve exactly at the contact
            # frame (and stays resolved everywhere else) -- a realistic
            # per-row pointer hiccup, not a structural absence.
            drop_x2 = (i == 25)
            rows.append(_row(i, h1=100, h2=h2, m1=m1, m2=m2, drop_x2=drop_x2))

        from tempfile import TemporaryDirectory
        with TemporaryDirectory() as d:
            tmp = Path(d)
            path = tmp / "ptrx.jsonl"
            _write_jsonl(path, rows)
            meta = {
                "format": "jsonl-v3", "family": "testfam", "port": "arcade",
                "fighter_fields": [
                    {"name": "char_id", "off": "0x0", "size": 1},
                    {"name": "health", "off": "0xE", "size": 1},
                    {"name": "x", "off": "0x12", "size": 2},
                ],
                "calibration": {}, "globals_recorded": [],
                "pointer_resolved_fields": ["x"],
            }
            (tmp / "ptrx.meta.json").write_text(json.dumps(meta))

            obs, stats = harvest_file(path, FakeProfile())

        self.assertEqual(stats.contacts_found, 1)
        self.assertEqual(stats.contacts_skipped_pointer_unresolved_x, 1)
        self.assertEqual(stats.observations_usable, 0)
        self.assertEqual(obs, [])


class UnattributableMoveTest(unittest.TestCase):
    """A simultaneous two-button press that matches no single attack_chords
    class is recorded as UNATTRIBUTED, not guessed at."""

    def test_unattributable_move_is_recorded_unattributed(self):
        rows = []
        for i in range(40):
            m1 = 0
            if 20 <= i <= 25:
                m1 = B_Y | B_X             # HP+HK together: no class matches
            if i >= 30:
                m1 = B_RIGHT
            m2 = B_LEFT if i >= 28 else 0
            h2 = 100 if i < 25 else 84
            rows.append(_row(i, h1=100, h2=h2, m1=m1, m2=m2))

        from tempfile import TemporaryDirectory
        with TemporaryDirectory() as d:
            path = Path(d) / "unattr.jsonl"
            _write_jsonl(path, rows)
            obs, stats = harvest_file(path, FakeProfile())

        self.assertEqual(len(obs), 1)
        o = obs[0]
        self.assertIsNone(o.move)
        self.assertEqual(o.move_source, "unattributed")
        self.assertIsNotNone(o.observed_advantage)
        self.assertEqual(stats.observations_usable, 1)
        self.assertEqual(stats.observations_attributed, 0)


class SpecialAnnotationAttributionTest(unittest.TestCase):
    """The recorder's own p1_special/p2_special annotation is used directly
    when present, ahead of chord-based inference."""

    def test_special_annotation_is_attributed(self):
        rows = []
        for i in range(40):
            # the motion inputs that complete the special (forward taps into
            # HP) -- a real recording never shows a special annotation with
            # no preceding input on that port.
            m1 = B_RIGHT if 15 <= i <= 22 else 0
            p1_special = "acid_spit" if i == 22 else None
            if i >= 30:
                m1 = B_DOWN                   # attacker's new action, distinct
                                               # from the pre-special B_RIGHT
            m2 = B_LEFT if i >= 28 else 0
            h2 = 100 if i < 25 else 84
            rows.append(_row(i, h1=100, h2=h2, m1=m1, m2=m2, p1_special=p1_special))

        from tempfile import TemporaryDirectory
        with TemporaryDirectory() as d:
            path = Path(d) / "special.jsonl"
            _write_jsonl(path, rows)
            obs, stats = harvest_file(path, FakeProfile())

        self.assertEqual(len(obs), 1)
        self.assertEqual(obs[0].move, "acid_spit")
        self.assertEqual(obs[0].move_source, "special")


class NoActionObservedTest(unittest.TestCase):
    """A contact where a side never presses anything new within the wait
    window contributes no observation, but is counted."""

    def test_attacker_never_acting_again_is_skipped_and_counted(self):
        rows = []
        for i in range(140):
            m1 = B_Y if 20 <= i <= 25 else 0   # attacker never presses anew
            m2 = B_LEFT if i >= 28 else 0
            h2 = 100 if i < 25 else 84
            rows.append(_row(i, h1=100, h2=h2, m1=m1, m2=m2))

        from tempfile import TemporaryDirectory
        with TemporaryDirectory() as d:
            path = Path(d) / "noaction.jsonl"
            _write_jsonl(path, rows)
            obs, stats = harvest_file(path, FakeProfile())

        self.assertEqual(stats.contacts_found, 1)
        self.assertEqual(stats.contacts_skipped_no_action_attacker, 1)
        self.assertEqual(stats.observations_usable, 0)
        self.assertEqual(obs, [])


class UnsupportedVersionTest(unittest.TestCase):
    """A v1-shaped recording (no "v", no "block1") is rejected and counted,
    never crashes the run."""

    def test_v1_recording_is_skipped_and_counted(self):
        from tempfile import TemporaryDirectory
        with TemporaryDirectory() as d:
            path = Path(d) / "v1.jsonl"
            with open(path, "w") as f:
                f.write(json.dumps({"frame": 0, "p1x": 1}) + "\n")
            obs, stats = harvest_file(path, FakeProfile())

        self.assertEqual(obs, [])
        self.assertEqual(stats.files_skipped_unsupported_version, 1)
        self.assertEqual(len(stats.skipped_files), 1)


class RankCandidatesTest(unittest.TestCase):
    def test_ranks_by_count_descending(self):
        obs = [
            Observation(file="f", round_id=1, frame=1, hits=1, family="fam",
                        port="arcade", attacker_char="alice", defender_char="bob",
                        move="HP", move_source="chord", observed_advantage=-5,
                        attacker_next_frame=10, defender_next_frame=5, gap_px=None),
            Observation(file="f", round_id=1, frame=2, hits=1, family="fam",
                        port="arcade", attacker_char="alice", defender_char="bob",
                        move="HP", move_source="chord", observed_advantage=-3,
                        attacker_next_frame=10, defender_next_frame=7, gap_px=None),
            Observation(file="f", round_id=1, frame=3, hits=1, family="fam",
                        port="arcade", attacker_char="alice", defender_char="bob",
                        move="LP", move_source="chord", observed_advantage=2,
                        attacker_next_frame=10, defender_next_frame=12, gap_px=None),
        ]
        ranked = rank_candidates(obs)
        self.assertEqual(ranked[0].move, "HP")
        self.assertEqual(ranked[0].count, 2)
        self.assertEqual(ranked[1].move, "LP")
        self.assertEqual(ranked[1].count, 1)


class AuditContradictionTest(unittest.TestCase):
    """An observation more extreme than the measured table's worst-case
    bound is flagged; one that merely agrees is not."""

    def _obs(self, advantage):
        return Observation(
            file="f", round_id=1, frame=100, hits=1, family="fam", port="arcade",
            attacker_char="alice", defender_char="bob", move="HP",
            move_source="chord", observed_advantage=advantage,
            attacker_next_frame=110, defender_next_frame=110 + advantage,
            gap_px=None,
        )

    def _table(self):
        return {
            ("fam", "arcade", "alice", "HP"): [
                {"on_block": -16, "on_hit": 7},
            ],
        }

    def test_more_extreme_than_worst_case_is_flagged(self):
        findings = audit_against_table([self._obs(-25)], self._table(), tolerance=3)
        self.assertEqual(len(findings), 1)
        f = findings[0]
        self.assertIsInstance(f, AuditFinding)
        self.assertEqual(f.measured_bound, -16)
        self.assertEqual(f.observed_advantage, -25)

    def test_agreement_is_not_flagged(self):
        findings = audit_against_table([self._obs(-10)], self._table(), tolerance=3)
        self.assertEqual(findings, [])

    def test_within_tolerance_is_not_flagged(self):
        findings = audit_against_table([self._obs(-18)], self._table(), tolerance=3)
        self.assertEqual(findings, [])


if __name__ == "__main__":
    unittest.main()
