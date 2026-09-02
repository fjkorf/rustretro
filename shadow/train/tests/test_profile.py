from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from shadow_train import asurabld, dataset, evaluate, knn, profile


class ProfileLoadingTest(unittest.TestCase):
    """Spot-checks against library/asurabld's shipped profile pair -- mirrors
    src/profile.rs's own `shipped_asurabld_profile_parses_and_matches_the_old_
    constants` test so the two loaders agree on the same JSON."""

    def setUp(self):
        self.p = profile.get()

    def test_family_and_port(self):
        self.assertEqual(self.p.family, "asurabld")
        self.assertEqual(self.p.port, "arcade")

    def test_blocks(self):
        self.assertEqual(self.p.block1(), 0x403798)
        self.assertEqual(self.p.block2(), 0x40454C)
        self.assertEqual(self.p.stride(), 0xDB4)
        self.assertEqual(self.p.block2() - self.p.block1(), self.p.stride())

    def test_globals(self):
        self.assertEqual(self.p.global_addr("round_timer"), 0x40000A)
        self.assertEqual(self.p.global_addr("char_select"), 0x400006)
        self.assertEqual(self.p.global_addr("credits"), 0x40655D)
        self.assertIsNone(self.p.global_addr("nonexistent_global"))

    def test_field_off(self):
        self.assertEqual(self.p.field_off("health"), (0x177, 1))
        self.assertEqual(self.p.field_off("char_id"), (0x639, 1))
        self.assertIsNone(self.p.field_off("nonexistent_field"))

    def test_char_name(self):
        self.assertEqual(self.p.char_name(1), "goat")
        self.assertEqual(self.p.char_name(7), "rosemary")
        self.assertEqual(self.p.char_name(9), "sgeist")
        self.assertEqual(self.p.char_name(11), "c11")

    def test_matchup_slug(self):
        self.assertEqual(self.p.matchup_slug(1, 7), "goat-vs-rosemary")
        self.assertEqual(self.p.matchup_slug(1, None), "goat")
        self.assertEqual(self.p.matchup_slug(None, 7), "any-vs-rosemary")
        self.assertEqual(self.p.matchup_slug(None, None), "all")

    def test_stage_selector_roundtrip(self):
        self.assertEqual(self.p.stage_value_for_opponent(7), 5)
        self.assertEqual(self.p.opponent_for_stage_value(9), 9)
        self.assertIsNone(self.p.stage_value_for_opponent(3))  # footee has no stage value

    def test_gate_and_chords(self):
        self.assertEqual(len(self.p.gate), 6)
        globals_named = {c.get("global") for c in self.p.gate if c.get("global")}
        self.assertIn("char_select", globals_named)
        for cls in self.p.attack_classes:
            if cls != "None":
                self.assertIn(cls, self.p.attack_chords, f"{cls} chord missing")

    def test_enforcement_and_calibration(self):
        self.assertEqual(self.p.enforcement["health_max"], 0xEF)
        self.assertEqual(self.p.enforcement["timer_hold"], [0x85, 0x03])
        self.assertEqual(self.p.calibration_value("GROUND_Y"), 216)

    def test_class_lists(self):
        self.assertEqual(len(self.p.move_classes), 9)
        self.assertEqual(len(self.p.attack_classes), 6)

    def test_env_var_override(self, ):
        import os

        with tempfile.TemporaryDirectory() as d:
            game_dir = Path(d) / "mygame"
            game_dir.mkdir()
            (game_dir / "family.json").write_text(json.dumps({
                "family": "mygame",
                "roster": [{"id": 0, "name": "hero", "select_slot": 0}],
                "move_classes": ["Neutral", "Forward"],
                "attack_classes": ["None", "Light"],
            }))
            (game_dir / "mygame.profile.json").write_text(json.dumps({
                "family": "mygame", "port": "test",
                "core": {"provenance_game": "mygame", "provenance_core": "test"},
                "memory": {
                    "endianness": "big",
                    "blocks": {"block1": "0x1000", "block2": "0x2000", "stride": "0x1000"},
                    "fighter_fields": [{"name": "x", "off": "0x10", "size": 2}],
                    "globals": {"round_timer": "0x50"},
                },
                "gate": [],
                "enforcement": {"health_max": 100, "refill_below": 10,
                                 "timer_hold": [0, 0], "credits_target": 1, "credits_min": 1},
                "calibration": {"GROUND_Y": 100},
                "attack_chords": {"Light": ["b"]},
            }))

            loaded_explicit = profile.load(game_dir)
            self.assertEqual(loaded_explicit.family, "mygame")
            self.assertEqual(loaded_explicit.block1(), 0x1000)

            old = os.environ.get("RUSTRETRO_GAME_DIR")
            try:
                os.environ["RUSTRETRO_GAME_DIR"] = str(game_dir)
                got = profile.get()
                self.assertEqual(got.family, "mygame")
            finally:
                if old is None:
                    os.environ.pop("RUSTRETRO_GAME_DIR", None)
                else:
                    os.environ["RUSTRETRO_GAME_DIR"] = old

    def test_family_mismatch_raises(self):
        with tempfile.TemporaryDirectory() as d:
            game_dir = Path(d) / "mismatched"
            game_dir.mkdir()
            (game_dir / "family.json").write_text(json.dumps({
                "family": "gameA", "roster": [], "move_classes": [], "attack_classes": [],
            }))
            (game_dir / "mismatched.profile.json").write_text(json.dumps({
                "family": "gameB", "port": "test",
                "core": {"provenance_game": "x", "provenance_core": "y"},
                "memory": {"blocks": {"block1": "0x1", "block2": "0x2", "stride": "0x1"},
                           "fighter_fields": [], "globals": {}},
                "gate": [], "enforcement": {"health_max": 1, "refill_below": 1,
                                             "timer_hold": [0, 0], "credits_target": 1,
                                             "credits_min": 1},
                "calibration": {}, "attack_chords": {},
            }))
            with self.assertRaises(profile.ProfileError):
                profile.load(game_dir)

    def test_hex_addr_parsing_matches_rust_adapter(self):
        self.assertEqual(profile._parse_addr("0x403798"), 0x403798)
        self.assertEqual(profile._parse_addr("403798"), 0x403798)
        self.assertEqual(profile._parse_addr(0x403798), 0x403798)


class AsurabldParityTest(unittest.TestCase):
    """asurabld.py's public constants must exactly equal the values the
    profile loader resolves -- the loader-view contract (task item 2)."""

    def test_constants_match_profile(self):
        p = profile.get()
        self.assertEqual(asurabld.BLOCK1, 0x403798)
        self.assertEqual(asurabld.BLOCK1, p.block1())
        self.assertEqual(asurabld.BLOCK2, p.block2())
        self.assertEqual(asurabld.STRIDE, p.stride())
        self.assertEqual(asurabld.HEALTH, p.field_off("health")[0])
        self.assertEqual(asurabld.CHAR_ID, p.field_off("char_id")[0])
        self.assertEqual(asurabld.ROUND_TIMER, p.global_addr("round_timer"))
        self.assertEqual(asurabld.CREDITS, p.global_addr("credits"))

    def test_char_name_and_matchup_slug(self):
        self.assertEqual(asurabld.char_name(7), "rosemary")
        self.assertEqual(asurabld.CHAR_NAMES[7], "rosemary")
        self.assertEqual(asurabld.matchup_slug(1, 7), "goat-vs-rosemary")

    def test_world_constants(self):
        self.assertEqual(asurabld.GROUND_Y, 216)
        self.assertEqual(asurabld.ROUND_START_Y, asurabld.GROUND_Y)
        self.assertEqual(asurabld.ROUND_START_X_LEFT, 84)
        self.assertEqual(asurabld.ROUND_START_X_RIGHT, 232)


class HeadSizeDerivationTest(unittest.TestCase):
    """Task item 4: nothing may hardcode 9 moves / 6 attacks -- every head
    size must equal len(MOVE_CLASSES)/len(ATTACK_CLASSES) as loaded from the
    profile's family.json, not a literal."""

    def test_class_list_lengths(self):
        self.assertEqual(len(dataset.MOVE_CLASSES), 9)
        self.assertEqual(len(dataset.ATTACK_CLASSES), 6)
        self.assertEqual(dataset.MOVE_CLASSES, profile.get().move_classes)
        self.assertEqual(dataset.ATTACK_CLASSES, profile.get().attack_classes)

    def test_evaluate_minlength_derives_from_class_lists(self):
        self.assertEqual(evaluate.N_MOVE_CLASSES, len(dataset.MOVE_CLASSES))
        self.assertEqual(evaluate.N_ATTACK_CLASSES, len(dataset.ATTACK_CLASSES))

    def test_knn_predict_proba_head_sizes_track_class_lists(self):
        import numpy as np

        rng = np.random.default_rng(0)
        n = 50
        X = rng.normal(size=(n, 4)).astype(np.float32)
        y_move = rng.integers(0, len(dataset.MOVE_CLASSES), size=n)
        y_attack = rng.integers(0, len(dataset.ATTACK_CLASSES), size=n)
        policy = knn.KnnPolicy(k=5).fit(X, y_move, y_attack)
        pm, pa = policy.predict_proba(X[0])
        self.assertEqual(len(pm), len(dataset.MOVE_CLASSES))
        self.assertEqual(len(pa), len(dataset.ATTACK_CLASSES))

    def test_calibration_keys_and_meta_family_port(self):
        from shadow_train.__main__ import CALIBRATION_KEYS

        prof = profile.get()
        self.assertEqual(CALIBRATION_KEYS, list(prof.calibration.keys()))
        for key in CALIBRATION_KEYS:
            self.assertEqual(getattr(dataset, key), prof.calibration[key])


def _minimal_port_json(port: str, **extra) -> str:
    base = {
        "family": "fam", "port": port,
        "core": {"provenance_game": "fam", "provenance_core": "test"},
        "memory": {
            "blocks": {"block1": "0x0", "block2": "0x0", "stride": "0x0"},
            "fighter_fields": [], "globals": {},
        },
        "gate": [],
        "enforcement": {"health_max": 255, "refill_below": 1,
                         "timer_hold": [0, 0], "credits_target": 0, "credits_min": 0},
        "calibration": {}, "attack_chords": {},
    }
    base.update(extra)
    return json.dumps(base)


def _minimal_family_json(roster=None) -> str:
    return json.dumps({
        "family": "fam", "roster": roster or [], "move_classes": [], "attack_classes": [],
    })


class PathResolutionTest(unittest.TestCase):
    """RECORDER_V3.md §5.2, mirrored from src/profile.rs's `resolve_game_dir`
    tests -- same three cases (bare dir / port-segment path / neither)."""

    def test_single_profile_fallback(self):
        with tempfile.TemporaryDirectory() as d:
            game_dir = Path(d) / "onlyone"
            game_dir.mkdir()
            (game_dir / "family.json").write_text(_minimal_family_json())
            (game_dir / "weird_name.profile.json").write_text(_minimal_port_json("only"))
            prof = profile.load(game_dir)
            self.assertEqual(prof.port, "only")

    def test_multiple_profiles_no_default_errors(self):
        with tempfile.TemporaryDirectory() as d:
            game_dir = Path(d) / "multi"
            game_dir.mkdir()
            (game_dir / "family.json").write_text(_minimal_family_json())
            (game_dir / "a.profile.json").write_text(_minimal_port_json("a"))
            (game_dir / "b.profile.json").write_text(_minimal_port_json("b"))
            with self.assertRaises(profile.ProfileError) as ctx:
                profile.load(game_dir)
            self.assertIn("multiple port profiles", str(ctx.exception))

    def test_port_segment_by_filename(self):
        with tempfile.TemporaryDirectory() as d:
            game_dir = Path(d) / "multi2"
            game_dir.mkdir()
            (game_dir / "family.json").write_text(_minimal_family_json())
            (game_dir / "genesis.profile.json").write_text(_minimal_port_json("genesis"))
            (game_dir / "arcade.profile.json").write_text(_minimal_port_json("arcade"))
            prof = profile.load(game_dir / "genesis")
            self.assertEqual(prof.port, "genesis")
            self.assertEqual(prof.dir, game_dir)

    def test_port_segment_by_field_match(self):
        with tempfile.TemporaryDirectory() as d:
            game_dir = Path(d) / "multi3"
            game_dir.mkdir()
            (game_dir / "family.json").write_text(_minimal_family_json())
            # filename doesn't match the selector -- must fall back to
            # scanning each profile's own "port" field.
            (game_dir / "multi3.profile.json").write_text(_minimal_port_json("v2"))
            prof = profile.load(game_dir / "v2")
            self.assertEqual(prof.port, "v2")

    def test_port_segment_not_found_errors(self):
        with tempfile.TemporaryDirectory() as d:
            game_dir = Path(d) / "multi4"
            game_dir.mkdir()
            (game_dir / "family.json").write_text(_minimal_family_json())
            (game_dir / "arcade.profile.json").write_text(_minimal_port_json("arcade"))
            with self.assertRaises(profile.ProfileError) as ctx:
                profile.load(game_dir / "nonexistent")
            self.assertIn("no port 'nonexistent'", str(ctx.exception))

    def test_port_segment_ambiguous_errors(self):
        with tempfile.TemporaryDirectory() as d:
            game_dir = Path(d) / "multi5"
            game_dir.mkdir()
            (game_dir / "family.json").write_text(_minimal_family_json())
            (game_dir / "one.profile.json").write_text(_minimal_port_json("dup"))
            (game_dir / "two.profile.json").write_text(_minimal_port_json("dup"))
            with self.assertRaises(profile.ProfileError) as ctx:
                profile.load(game_dir / "dup")
            self.assertIn("ambiguous", str(ctx.exception))

    def test_no_such_game_directory(self):
        with self.assertRaises(profile.ProfileError) as ctx:
            profile.load("/nonexistent/parent/child")
        self.assertIn("no such game directory", str(ctx.exception))


class SchemaAdditionsTest(unittest.TestCase):
    """RECORDER_V3.md §2: record_globals / hitstun_sources / id_map --
    tolerate absence, validate presence, mirrors src/profile.rs's own tests."""

    def _load(self, d: Path, port_extra: dict, roster=None):
        (d / "family.json").write_text(_minimal_family_json(roster))
        (d / "fam.profile.json").write_text(_minimal_port_json("test", **port_extra))
        return profile.load(d)

    def test_absent_keys_are_identity(self):
        with tempfile.TemporaryDirectory() as d:
            prof = self._load(Path(d), {})
            self.assertEqual(prof.canon_char_id(5), 5)
            self.assertIsNone(prof.hitstun_sources)
            self.assertEqual(prof.record_globals, [])

    def test_id_map_present_and_mapped(self):
        with tempfile.TemporaryDirectory() as d:
            roster = [{"id": 0, "name": "a"}, {"id": 1, "name": "b"}, {"id": 2, "name": "c"}]
            prof = self._load(Path(d), {"id_map": {"5": 0, "6": 1, "7": 2}}, roster)
            self.assertEqual(prof.canon_char_id(5), 0)
            self.assertEqual(prof.canon_char_id(6), 1)
            self.assertEqual(prof.canon_char_id(99), 99)  # unmapped -> identity

    def test_id_map_invalid_roster_id_raises(self):
        with tempfile.TemporaryDirectory() as d:
            roster = [{"id": 0, "name": "a"}]
            with self.assertRaises(profile.ProfileError) as ctx:
                self._load(Path(d), {"id_map": {"5": 99}}, roster)
            self.assertIn("id_map maps to unknown roster id", str(ctx.exception))

    def test_record_globals_valid(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            (d / "family.json").write_text(_minimal_family_json())
            (d / "fam.profile.json").write_text(json.dumps({
                "family": "fam", "port": "test",
                "core": {"provenance_game": "fam", "provenance_core": "test"},
                "memory": {
                    "blocks": {"block1": "0x0", "block2": "0x0", "stride": "0x0"},
                    "fighter_fields": [],
                    "globals": {"combo": "0x1000", "demo": "0x2000"},
                    "record_globals": [{"name": "combo", "size": 1},
                                        {"name": "demo", "size": 2}],
                },
                "gate": [],
                "enforcement": {"health_max": 255, "refill_below": 1,
                                 "timer_hold": [0, 0], "credits_target": 0, "credits_min": 0},
                "calibration": {}, "attack_chords": {},
            }))
            prof = profile.load(d)
            self.assertEqual([rg["name"] for rg in prof.record_globals], ["combo", "demo"])

    def test_record_globals_unknown_global_raises(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            (d / "family.json").write_text(_minimal_family_json())
            (d / "fam.profile.json").write_text(json.dumps({
                "family": "fam", "port": "test",
                "core": {"provenance_game": "fam", "provenance_core": "test"},
                "memory": {
                    "blocks": {"block1": "0x0", "block2": "0x0", "stride": "0x0"},
                    "fighter_fields": [], "globals": {},
                    "record_globals": [{"name": "nope", "size": 1}],
                },
                "gate": [],
                "enforcement": {"health_max": 255, "refill_below": 1,
                                 "timer_hold": [0, 0], "credits_target": 0, "credits_min": 0},
                "calibration": {}, "attack_chords": {},
            }))
            with self.assertRaises(profile.ProfileError) as ctx:
                profile.load(d)
            self.assertIn("record_globals names unknown global", str(ctx.exception))

    def test_hitstun_sources_valid(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            (d / "family.json").write_text(_minimal_family_json())
            (d / "fam.profile.json").write_text(json.dumps({
                "family": "fam", "port": "test",
                "core": {"provenance_game": "fam", "provenance_core": "test"},
                "memory": {
                    "blocks": {"block1": "0x0", "block2": "0x0", "stride": "0x0"},
                    "fighter_fields": [], "globals": {"combo1": "0x1", "combo2": "0x2"},
                    "record_globals": [{"name": "combo1", "size": 1},
                                        {"name": "combo2", "size": 1}],
                },
                "gate": [],
                "enforcement": {"health_max": 255, "refill_below": 1,
                                 "timer_hold": [0, 0], "credits_target": 0, "credits_min": 0},
                "calibration": {}, "attack_chords": {},
                "hitstun_sources": {"block1": "combo1", "block2": "combo2"},
            }))
            prof = profile.load(d)
            self.assertEqual(prof.hitstun_sources, {"block1": "combo1", "block2": "combo2"})

    def test_hitstun_sources_unrecorded_global_raises(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            (d / "family.json").write_text(_minimal_family_json())
            (d / "fam.profile.json").write_text(json.dumps({
                "family": "fam", "port": "test",
                "core": {"provenance_game": "fam", "provenance_core": "test"},
                "memory": {
                    "blocks": {"block1": "0x0", "block2": "0x0", "stride": "0x0"},
                    "fighter_fields": [], "globals": {"combo1": "0x1"},
                },
                "gate": [],
                "enforcement": {"health_max": 255, "refill_below": 1,
                                 "timer_hold": [0, 0], "credits_target": 0, "credits_min": 0},
                "calibration": {}, "attack_chords": {},
                "hitstun_sources": {"block1": "combo1"},  # never gated or record_globals'd
            }))
            with self.assertRaises(profile.ProfileError) as ctx:
                profile.load(d)
            self.assertIn("hitstun_sources names unrecorded global", str(ctx.exception))


class MacroActionsSchemaTest(unittest.TestCase):
    """shadow/MACRO_ACTIONS.md §1/§2/§6: family `moves`, port `special_
    inputs`, and `contact_signal` -- absence is today's exact meaning
    (no specials), presence is load-validated."""

    def _write(self, d: Path, moves=None, special_inputs=None, contact_signal=None,
               roster=None, attack_chords=None, record_globals=None,
               globals_map=None):
        roster = roster or [{"id": 0, "name": "reptile"}, {"id": 1, "name": "foe"}]
        fam = {
            "family": "fam", "roster": roster,
            "move_classes": [], "attack_classes": ["None", "LP", "LK"],
        }
        if moves is not None:
            fam["moves"] = moves
        (d / "family.json").write_text(json.dumps(fam))

        port = json.loads(_minimal_port_json(
            "test", attack_chords=attack_chords or {"LP": ["b"], "LK": ["a"]},
        ))
        port["memory"]["globals"] = globals_map or {}
        if record_globals is not None:
            port["memory"]["record_globals"] = record_globals
        if special_inputs is not None:
            port["special_inputs"] = special_inputs
        if contact_signal is not None:
            port["contact_signal"] = contact_signal
        (d / "fam.profile.json").write_text(json.dumps(port))
        return profile.load(d)

    def test_absent_moves_and_special_inputs_are_identity(self):
        with tempfile.TemporaryDirectory() as d:
            prof = self._write(Path(d))
            self.assertEqual(prof.moves, {})
            self.assertEqual(prof.special_inputs, {})
            self.assertIsNone(prof.contact_signal)
            self.assertEqual(prof.all_special_names(), [])
            self.assertEqual(prof.special_names_for("reptile"), [])
            self.assertIsNone(prof.macro_steps_for("reptile", "slide"))

    def test_moves_parsed_and_special_names_accessors(self):
        moves = {"reptile": [
            {"name": "slide", "tags": ["special", "low"]},
            {"name": "roll", "tags": ["low"]},  # not tagged special
        ]}
        with tempfile.TemporaryDirectory() as d:
            prof = self._write(Path(d), moves=moves)
            self.assertEqual(prof.special_names_for("reptile"), ["slide"])
            self.assertEqual(prof.all_special_names(), ["slide"])
            self.assertEqual(prof.special_names_for("foe"), [])  # no moves entry

    def test_moves_unknown_character_raises(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(profile.ProfileError) as ctx:
                self._write(Path(d), moves={"nobody": [{"name": "x", "tags": ["special"]}]})
            self.assertIn("moves names unknown character", str(ctx.exception))

    def test_special_inputs_parsed_and_macro_steps_for(self):
        moves = {"reptile": [{"name": "slide", "tags": ["special"]}]}
        special_inputs = {"reptile": {"slide": [
            {"dirs": ["back"], "press": ["LK", "LP"], "frames": 4},
        ]}}
        with tempfile.TemporaryDirectory() as d:
            prof = self._write(Path(d), moves=moves, special_inputs=special_inputs)
            steps = prof.macro_steps_for("reptile", "slide")
            self.assertEqual(steps, [{
                "dirs": ["back"], "press": ["LK", "LP"], "frames": 4,
                "hold": [], "release": [], "while_held": [], "min_frames": 0,
            }])
            self.assertIsNone(prof.macro_steps_for("reptile", "nonexistent"))

    def test_special_inputs_default_frames_is_3(self):
        moves = {"reptile": [{"name": "slide", "tags": ["special"]}]}
        special_inputs = {"reptile": {"slide": [{"dirs": ["back"], "press": ["LK"]}]}}
        with tempfile.TemporaryDirectory() as d:
            prof = self._write(Path(d), moves=moves, special_inputs=special_inputs)
            self.assertEqual(prof.macro_steps_for("reptile", "slide")[0]["frames"], 3)

    def test_special_inputs_carries_hold_release_while_held(self):
        """MACRO_ACTIONS.md §10.1: the compiled step dict must carry `hold`/
        `min_frames`, `release`, and `while_held` through -- the bug this
        test pins is the compiler silently dropping all three, which made
        every hold/release move compile down to a step that presses
        nothing (invisible to every downstream consumer, never an error)."""
        moves = {"reptile": [
            {"name": "sai_throw", "tags": ["special"]},
            {"name": "invisibility", "tags": ["special"]},
        ]}
        special_inputs = {"reptile": {
            "sai_throw": [
                {"hold": ["LP"], "min_frames": 34},
                {"release": ["LP"]},
            ],
            "invisibility": [
                {"dirs": ["up"], "while_held": ["LK"]},
                {"release": ["LK"]},
                {"press": ["LP"]},
            ],
        }}
        with tempfile.TemporaryDirectory() as d:
            prof = self._write(Path(d), moves=moves, special_inputs=special_inputs)
            sai = prof.macro_steps_for("reptile", "sai_throw")
            self.assertEqual(sai[0]["hold"], ["LP"])
            self.assertEqual(sai[0]["min_frames"], 34)
            self.assertEqual(sai[1]["release"], ["LP"])
            invis = prof.macro_steps_for("reptile", "invisibility")
            self.assertEqual(invis[0]["while_held"], ["LK"])
            self.assertEqual(invis[1]["release"], ["LK"])

    def test_special_inputs_hold_needs_positive_min_frames(self):
        moves = {"reptile": [{"name": "sai_throw", "tags": ["special"]}]}
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(profile.ProfileError) as ctx:
                self._write(Path(d), moves=moves, special_inputs={
                    "reptile": {"sai_throw": [{"hold": ["LP"]}]},
                })
            self.assertIn("needs a positive min_frames", str(ctx.exception))

    def test_special_inputs_min_frames_without_hold_raises(self):
        moves = {"reptile": [{"name": "sai_throw", "tags": ["special"]}]}
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(profile.ProfileError) as ctx:
                self._write(Path(d), moves=moves, special_inputs={
                    "reptile": {"sai_throw": [{"press": ["LP"], "min_frames": 5}]},
                })
            self.assertIn("min_frames set without a hold step", str(ctx.exception))

    def test_special_inputs_step_mixing_press_and_hold_raises(self):
        moves = {"reptile": [{"name": "sai_throw", "tags": ["special"]}]}
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(profile.ProfileError) as ctx:
                self._write(Path(d), moves=moves, special_inputs={
                    "reptile": {"sai_throw": [
                        {"press": ["LP"], "hold": ["LK"], "min_frames": 5},
                    ]},
                })
            self.assertIn("mixes press/hold/release", str(ctx.exception))

    def test_special_inputs_unknown_class_in_hold_or_while_held_raises(self):
        moves = {"reptile": [{"name": "sai_throw", "tags": ["special"]}]}
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(profile.ProfileError) as ctx:
                self._write(Path(d), moves=moves, special_inputs={
                    "reptile": {"sai_throw": [{"hold": ["HP"], "min_frames": 5}]},
                })
            self.assertIn("unknown attack-chord class 'HP'", str(ctx.exception))
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(profile.ProfileError) as ctx:
                self._write(Path(d), moves=moves, special_inputs={
                    "reptile": {"sai_throw": [
                        {"dirs": ["up"], "while_held": ["HP"]},
                        {"press": ["LP"]},
                    ]},
                })
            self.assertIn("unknown attack-chord class 'HP'", str(ctx.exception))

    def test_special_inputs_empty_step_list_raises(self):
        moves = {"reptile": [{"name": "slide", "tags": ["special"]}]}
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(profile.ProfileError) as ctx:
                self._write(Path(d), moves=moves,
                             special_inputs={"reptile": {"slide": []}})
            self.assertIn("has no steps", str(ctx.exception))

    def test_special_inputs_unknown_character_raises(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(profile.ProfileError) as ctx:
                self._write(Path(d), special_inputs={"ghost": {"slide": []}})
            self.assertIn("no family moves entry", str(ctx.exception))

    def test_special_inputs_unknown_move_raises(self):
        moves = {"reptile": [{"name": "slide", "tags": ["special"]}]}
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(profile.ProfileError) as ctx:
                self._write(Path(d), moves=moves,
                             special_inputs={"reptile": {"not_a_move": []}})
            self.assertIn("not in family moves", str(ctx.exception))

    def test_special_inputs_unknown_direction_raises(self):
        moves = {"reptile": [{"name": "slide", "tags": ["special"]}]}
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(profile.ProfileError) as ctx:
                self._write(Path(d), moves=moves, special_inputs={
                    "reptile": {"slide": [{"dirs": ["sideways"], "press": ["LK"]}]},
                })
            self.assertIn("unknown direction", str(ctx.exception))

    def test_special_inputs_unknown_press_class_raises(self):
        moves = {"reptile": [{"name": "slide", "tags": ["special"]}]}
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(profile.ProfileError) as ctx:
                self._write(Path(d), moves=moves, special_inputs={
                    "reptile": {"slide": [{"dirs": ["back"], "press": ["NoSuchClass"]}]},
                })
            self.assertIn("unknown attack-chord class", str(ctx.exception))

    def test_shipped_mk2_sai_throw_hold_and_release_survive_compilation(self):
        """Regression pin for the §10.1 compiler-drop bug: Mileena's real
        shipped `sai_throw` (`library/mk2/mk2.profile.json`) is
        `hold HP for 34 frames, then release` -- driven live (see
        MACRO_ACTIONS.md §10.1's docstring note: 34 frames, not the
        transcribed ~180, was bisected against the real charge threshold).
        Before the fix, this compiled to two steps holding/releasing
        NOTHING; this test loads the actual on-disk profile (no hand-built
        fixture) so a regression here is a real profile, not a synthetic
        stand-in, failing to compile correctly."""
        prof = profile.load(profile.REPO_ROOT / "library" / "mk2")
        steps = prof.macro_steps_for("mileena", "sai_throw")
        self.assertIsNotNone(steps)
        self.assertEqual(len(steps), 2)
        self.assertEqual(steps[0]["hold"], ["HP"])
        self.assertEqual(steps[0]["min_frames"], 34)
        self.assertEqual(steps[0]["press"], [])
        self.assertEqual(steps[1]["release"], ["HP"])

        # And the compiled view round-trips through the matcher compiler
        # (shadow_train.macros.compile_macros) without raising and without
        # losing the kinds -- the actual path _macro_override_events (in
        # dataset.py) exercises for every recorded round.
        from shadow_train import macros as _macros
        macro_list = _macros.compile_macros({"sai_throw": steps})
        self.assertEqual(len(macro_list), 1)
        compiled = macro_list[0].steps
        self.assertEqual(compiled[0].kind, "hold")
        self.assertEqual(compiled[0].hold, ("HP",))
        self.assertEqual(compiled[0].min_frames, 34)
        self.assertEqual(compiled[1].kind, "release")
        self.assertEqual(compiled[1].release, ("HP",))

    def test_contact_signal_valid(self):
        with tempfile.TemporaryDirectory() as d:
            prof = self._write(
                Path(d),
                globals_map={"hit_counter": "0x1000"},
                record_globals=[{"name": "hit_counter", "size": 1}],
                contact_signal={"global": "hit_counter"},
            )
            self.assertEqual(prof.contact_signal, {"global": "hit_counter"})

    def test_contact_signal_unknown_global_raises(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(profile.ProfileError) as ctx:
                self._write(Path(d), contact_signal={"global": "hit_counter"})
            self.assertIn("contact_signal names unknown global", str(ctx.exception))

    def test_contact_signal_valid_without_being_recorded_yet(self):
        # Unlike hitstun_sources, contact_signal has no Python-side consumer
        # today (Rust-only, src/training.rs) -- a global that EXISTS but
        # isn't (yet) in record_globals must still load, not hard-fail.
        with tempfile.TemporaryDirectory() as d:
            prof = self._write(
                Path(d),
                globals_map={"hit_counter": "0x1000"},
                contact_signal={"global": "hit_counter"},
            )
            self.assertEqual(prof.contact_signal, {"global": "hit_counter"})

    def test_contact_signal_direction_decrease_carried_through(self):
        # Optional `direction: "decrease"` (mk2 arcade ships it on struct
        # `health`) must survive normalization -- only a DROP counts as
        # contact on the Rust side (refill/round-intro INCREASE immunity).
        with tempfile.TemporaryDirectory() as d:
            prof = self._write(
                Path(d),
                globals_map={"hit_counter": "0x1000"},
                contact_signal={"global": "hit_counter", "direction": "decrease"},
            )
            self.assertEqual(
                prof.contact_signal,
                {"global": "hit_counter", "direction": "decrease"},
            )

    def test_contact_signal_bad_direction_raises(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(profile.ProfileError) as ctx:
                self._write(
                    Path(d),
                    globals_map={"hit_counter": "0x1000"},
                    contact_signal={"global": "hit_counter", "direction": "increase"},
                )
            self.assertIn("direction", str(ctx.exception))


if __name__ == "__main__":
    unittest.main()
