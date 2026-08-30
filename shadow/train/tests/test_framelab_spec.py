"""Unit tests for `framelab.spec.FramelabSpec` — the `framelab` profile
block that replaced MK2-shaped constants formerly hardcoded in
`framelab.observables`/`framelab.kit` (docs/frames.md §3.1/§4.1/§4.2;
docs/game-profiles.md "The framelab block"; CLAUDE.md: "never hardcode a
game address in code again").

Three things this file exists to prove:

  1. **MK2's shipped block round-trips to the values the harness used
     before** (`ShippedMk2BlockTest`) -- the behaviour-identity check the
     task asked for, at the schema level.
  2. **Absent is a first-class, distinct outcome.** A port with no
     `framelab` block at all (asurabld, mk2 Genesis -- neither calibrated)
     declines with `FramelabNotConfigured`; so does a block missing its
     anchor, its observables, or an active observable's calibration.
  3. **`observables.py`'s profile-driven sampler/contact-read reproduce the
     byte-for-byte behaviour of the code they replaced**
     (`ObservablesBehaviorIdentityTest`) -- the harness-level half of the
     same check, run against a synthetic memory image rather than a live
     emulator.
"""

from __future__ import annotations

import struct as pystruct
import unittest

from shadow_train import profile as game_profile
from shadow_train.framelab import observables as obs
from shadow_train.framelab.spec import (
    Addressing,
    AnchorSpec,
    FramelabError,
    FramelabNotConfigured,
    FramelabSpec,
    ObservableSpec,
)


class _FakeProfile:
    """The minimal surface `FramelabSpec.from_profile` needs: `family`,
    `port`, and the raw parsed JSON `port_raw` carries `framelab` under."""

    def __init__(self, raw, family="fake", port="test"):
        self.family = family
        self.port = port
        self.port_raw = raw


def _mk2_profile():
    return game_profile.load("library/mk2")


class ShippedMk2BlockTest(unittest.TestCase):
    """§ behaviour-identity: `library/mk2/mk2.profile.json`'s `framelab`
    block must resolve to EXACTLY the constants `kit.py`/`observables.py`
    used to hardcode:

      * `DEFAULT_OBSERVABLES = (STRUCT_VELOCITY, POINTER_X)` (declared order)
      * `STRUCT_VELOCITY_RANGE = (0x0B, 0x0E)`
      * the contact anchor default (`CONTACT_STRUCT_HEALTH`, i.e. `health`)
      * `DEFAULT_QUIET_FRAMES = 20`
      * docs/frames.md §3.1's measured probe-shape table (neutral/attacker
        1/2, guarded defender 10/11)
    """

    def setUp(self):
        self.spec = FramelabSpec.from_profile(_mk2_profile())

    def test_anchor_is_the_struct_health_field_not_the_hud_pair(self):
        self.assertEqual(self.spec.anchor, AnchorSpec(source="field", field="health"))

    def test_quiet_frames_matches_the_old_default(self):
        self.assertEqual(self.spec.quiet_frames, 20)

    def test_observable_order_matches_the_old_default_pair(self):
        self.assertEqual(
            self.spec.default_observable_names(), ("struct_velocity", "pointer_x")
        )

    def test_struct_velocity_addressing_matches_the_old_byte_range(self):
        sv = self.spec.observable("struct_velocity")
        self.assertEqual(sv.status, "active")
        self.assertEqual(sv.addressing, Addressing(kind="byte_range", off=0x0B, end=0x0E))

    def test_pointer_x_addressing_names_the_existing_x_fighter_field(self):
        px = self.spec.observable("pointer_x")
        self.assertEqual(px.status, "active")
        self.assertEqual(px.addressing, Addressing(kind="fighter_field", field="x"))

    def test_probe_shape_calibration_matches_docs_frames_md_table(self):
        sv, px = self.spec.observable("struct_velocity"), self.spec.observable("pointer_x")
        for shape in ("attacker/hit", "defender/hit", "attacker/block"):
            self.assertEqual(sv.latency_for(shape), 1)
            self.assertEqual(px.latency_for(shape), 2)
        # The guarded-defender shape is the one §3.1 says DIFFERS.
        self.assertEqual(sv.latency_for("defender/block"), 10)
        self.assertEqual(px.latency_for("defender/block"), 11)

    def test_disqualified_observables_carry_their_measured_reason(self):
        for name in ("struct_divergence", "action_counter"):
            o = self.spec.observable(name)
            self.assertEqual(o.status, "disqualified")
            self.assertTrue(o.reason)
            self.assertNotIn(name, self.spec.default_observable_names())

    def test_disqualified_observable_declines_a_latency_lookup(self):
        with self.assertRaises(FramelabNotConfigured):
            self.spec.observable("action_counter").latency_for("attacker/hit")

    def test_rig_and_spacing_are_optional_data_carried_through(self):
        self.assertEqual(self.spec.rig.walk_directions_by_port[0], ("left", "right"))
        self.assertEqual(self.spec.rig.walk_directions_by_port[1], ("right", "left"))
        self.assertEqual(self.spec.spacing.collision_floor_px, 62)


class UncalibratedPortDeclinesTest(unittest.TestCase):
    """Deliverable #2 / the "also" section: a port that has not been
    calibrated must DECLINE, never silently borrow MK2's numbers."""

    def test_asurabld_has_no_framelab_block(self):
        prof = game_profile.load("library/asurabld")
        with self.assertRaises(FramelabNotConfigured) as ctx:
            FramelabSpec.from_profile(prof)
        self.assertIn("asurabld/arcade", str(ctx.exception))
        self.assertIn("framelab", str(ctx.exception))

    def test_mk2_genesis_has_no_framelab_block(self):
        prof = game_profile.load("library/mk2/genesis")
        with self.assertRaises(FramelabNotConfigured) as ctx:
            FramelabSpec.from_profile(prof)
        self.assertIn("mk2/genesis", str(ctx.exception))

    def test_declining_names_what_is_missing_not_a_generic_message(self):
        """The whole point of a named decline: a reader can tell WHICH port
        and WHAT is missing without opening the profile."""
        prof = game_profile.load("library/asurabld")
        msg = ""
        try:
            FramelabSpec.from_profile(prof)
        except FramelabNotConfigured as e:
            msg = str(e)
        self.assertIn("library/mk2", msg)  # points at a port that DOES have one


class MalformedBlockDeclinesTest(unittest.TestCase):
    """A `framelab` block that exists but is missing a required piece
    declines with a message naming that piece -- never a default."""

    def test_missing_anchor(self):
        raw = {"framelab": {"quiet_frames": 20, "observables": [
            {"name": "x", "status": "active",
             "addressing": {"kind": "whole_struct"},
             "calibration": {"attacker/hit": 1}},
        ]}}
        with self.assertRaises(FramelabNotConfigured) as ctx:
            FramelabSpec.from_profile(_FakeProfile(raw))
        self.assertIn("anchor", str(ctx.exception))

    def test_missing_quiet_frames(self):
        raw = {"framelab": {"anchor": {"field": "health"}, "observables": [
            {"name": "x", "status": "active",
             "addressing": {"kind": "whole_struct"},
             "calibration": {"attacker/hit": 1}},
        ]}}
        with self.assertRaises(FramelabNotConfigured) as ctx:
            FramelabSpec.from_profile(_FakeProfile(raw))
        self.assertIn("quiet_frames", str(ctx.exception))

    def test_missing_observables(self):
        raw = {"framelab": {"anchor": {"field": "health"}, "quiet_frames": 20}}
        with self.assertRaises(FramelabNotConfigured) as ctx:
            FramelabSpec.from_profile(_FakeProfile(raw))
        self.assertIn("observables", str(ctx.exception))

    def test_active_observable_missing_calibration(self):
        raw = {"framelab": {"anchor": {"field": "health"}, "quiet_frames": 20,
               "observables": [
                   {"name": "x", "status": "active",
                    "addressing": {"kind": "whole_struct"}},
               ]}}
        with self.assertRaises(FramelabNotConfigured) as ctx:
            FramelabSpec.from_profile(_FakeProfile(raw))
        self.assertIn("calibration", str(ctx.exception))
        self.assertIn("'x'", str(ctx.exception))

    def test_active_observable_missing_addressing(self):
        raw = {"framelab": {"anchor": {"field": "health"}, "quiet_frames": 20,
               "observables": [
                   {"name": "x", "status": "active",
                    "calibration": {"attacker/hit": 1}},
               ]}}
        with self.assertRaises(FramelabNotConfigured) as ctx:
            FramelabSpec.from_profile(_FakeProfile(raw))
        self.assertIn("addressing", str(ctx.exception))

    def test_disqualified_observable_without_a_reason_is_a_schema_error(self):
        raw = {"framelab": {"anchor": {"field": "health"}, "quiet_frames": 20,
               "observables": [{"name": "x", "status": "disqualified"}]}}
        with self.assertRaises(FramelabError):
            FramelabSpec.from_profile(_FakeProfile(raw))

    def test_unknown_observable_lookup_declines(self):
        prof = game_profile.load("library/mk2")
        spec = FramelabSpec.from_profile(prof)
        with self.assertRaises(FramelabNotConfigured):
            spec.observable("nonexistent_observable")

    def test_unmeasured_shape_lookup_declines(self):
        prof = game_profile.load("library/mk2")
        spec = FramelabSpec.from_profile(prof)
        with self.assertRaises(FramelabNotConfigured) as ctx:
            spec.observable("pointer_x").latency_for("attacker/parry")
        self.assertIn("'attacker/parry'", str(ctx.exception))

    def test_addressing_needs_a_known_kind(self):
        with self.assertRaises(FramelabError):
            Addressing.from_json({"kind": "made_up"}, where="test")

    def test_anchor_needs_field_or_hitstun_sources_not_both(self):
        with self.assertRaises(FramelabError):
            AnchorSpec.from_json({"field": "health", "hitstun_sources": True})

    def test_anchor_needs_something(self):
        with self.assertRaises(FramelabError):
            AnchorSpec.from_json({})

    def test_all_disqualified_observables_has_no_default_list(self):
        raw = {"framelab": {"anchor": {"field": "health"}, "quiet_frames": 20,
               "observables": [
                   {"name": "x", "status": "disqualified", "reason": "no good"},
               ]}}
        spec = FramelabSpec.from_profile(_FakeProfile(raw))
        with self.assertRaises(FramelabNotConfigured):
            spec.default_observable_names()


# ── observables.py integration: profile-driven sampler behaviour-identity ──


class _FakeSession:
    def __init__(self, mem: bytes):
        self.mem = mem

    def read_memory(self, addr: int, length: int) -> bytes:
        return self.mem[addr : addr + length]


class ObservablesBehaviorIdentityTest(unittest.TestCase):
    """`observables.make_sampler`/`make_contact_read_from_spec`, driven by
    the profile's `framelab` block, must read the SAME bytes the old
    hardcoded MK2 constants did:

      * contact anchor -> `fighter.base + health_off` (was
        `make_contact_read(f, source=CONTACT_STRUCT_HEALTH)`'s default)
      * `struct_velocity` -> `struct[0x0B:0x0E]` (was `STRUCT_VELOCITY_RANGE`)
      * `pointer_x` -> the object-pool `x` field, pointer-resolved (was
        `POINTER_X`'s hardcoded branch in the old `make_sampler`)

    A synthetic memory image stands in for the emulator so this runs with no
    live session, exactly like every other framelab unit test.
    """

    def setUp(self):
        self.prof = game_profile.load("library/mk2")
        self.spec = FramelabSpec.from_profile(self.prof)
        self.f2 = obs.resolve_fighter(self.prof, "block2", 1)
        self.stride = self.prof.stride()
        self.ptr = self.f2.ptr

    def _build_memory(self, *, char_id=9, velocity=b"\x00\xfe\xff",
                       pointer_stale=False, x_value=4242) -> bytes:
        mem = bytearray(0x30000)
        struct_bytes = bytearray(self.stride)
        struct_bytes[0] = char_id
        struct_bytes[0x0B:0x0E] = velocity
        obj_addr = 0x5000
        raw_word = self.ptr.bias + (obj_addr << self.ptr.shift)
        base = self.f2.base
        mem[base + self.ptr.off : base + self.ptr.off + 4] = pystruct.pack(
            "<I", raw_word
        )
        mem[base : base + self.stride] = struct_bytes
        obj_entry = bytearray(self.ptr.span)
        obj_entry[self.ptr.char_off] = char_id if not pointer_stale else char_id + 1
        obj_entry[self.ptr.x_off : self.ptr.x_off + 2] = x_value.to_bytes(2, "little")
        mem[obj_addr : obj_addr + self.ptr.span] = obj_entry
        return bytes(mem)

    def test_contact_read_matches_the_old_struct_health_default(self):
        mem = bytearray(self._build_memory())
        mem[self.f2.base + self.f2.health_off] = 123
        session = _FakeSession(bytes(mem))
        new_read = obs.make_contact_read_from_spec(self.f2, self.spec)
        old_read = obs.make_contact_read(self.f2, source=obs.CONTACT_STRUCT_HEALTH)
        self.assertEqual(new_read(session), 123)
        self.assertEqual(new_read(session), old_read(session))

    def test_sampler_struct_velocity_matches_the_old_hardcoded_byte_range(self):
        session = _FakeSession(self._build_memory(velocity=b"\x00\xfe\xff"))
        sample = obs.make_sampler(self.f2, self.spec)
        self.assertEqual(sample(session)["struct_velocity"], b"\x00\xfe\xff")

    def test_sampler_pointer_x_matches_the_old_hardcoded_pointer_resolution(self):
        session = _FakeSession(self._build_memory(x_value=4242))
        sample = obs.make_sampler(self.f2, self.spec)
        self.assertEqual(sample(session)["pointer_x"], 4242)

    def test_sampler_pointer_x_is_none_on_a_stale_pointer_never_a_wrong_value(self):
        session = _FakeSession(self._build_memory(pointer_stale=True))
        sample = obs.make_sampler(self.f2, self.spec)
        self.assertIsNone(sample(session)["pointer_x"])

    def test_sampler_declines_a_fighter_field_addressing_absent_from_the_port(self):
        raw = dict(self.prof.port_raw)
        raw["framelab"] = {
            "anchor": {"field": "health"},
            "quiet_frames": 20,
            "observables": [
                {"name": "nope", "status": "active",
                 "addressing": {"kind": "fighter_field", "field": "not_a_real_field"},
                 "calibration": {"attacker/hit": 1}},
            ],
        }
        bogus_spec = FramelabSpec.from_profile(_FakeProfile(raw, family="mk2", port="arcade"))
        sample = obs.make_sampler(self.f2, bogus_spec)
        session = _FakeSession(self._build_memory())
        with self.assertRaises(FramelabNotConfigured):
            sample(session)


if __name__ == "__main__":
    unittest.main()
